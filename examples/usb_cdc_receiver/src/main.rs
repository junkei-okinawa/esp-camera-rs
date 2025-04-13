use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::delay::FreeRtos; // FreeRtos をインポート
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::hal::usb_serial::{UsbSerialConfig, UsbSerialDriver};
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::sys::{
    self, esp_now_init, esp_now_recv_info_t, esp_now_register_recv_cb, esp_wifi_set_ps,
    esp_wifi_set_storage, wifi_ps_type_t_WIFI_PS_NONE, wifi_storage_t_WIFI_STORAGE_RAM,
    ESP_NOW_ETH_ALEN,
};
// Use EspWifi for proper initialization and STA mode setting
use esp_idf_svc::wifi::{AuthMethod, ClientConfiguration, Configuration, EspWifi};
// use esp_idf_sys; // Already imported via esp_idf_svc::sys

use core::mem::MaybeUninit; // Import MaybeUninit
use heapless::spsc::{Consumer, Producer, Queue};
use log::{debug, error, info, warn}; // debugを追加
                                     // Removed unused imports: RefCell, mem
use anyhow::Result;
use std::slice;
use std::sync::Mutex; // Use anyhow::Result

// --- Configuration Constants ---
// const IMAGE_QUEUE_SIZE: usize = 20; // Increased queue size
const IMAGE_QUEUE_SIZE: usize = 512; // Increased queue size for larger data chunks

// --- Data Structure for Queue ---
// Store MAC address and received data chunk (now holds framed data)
#[derive(Debug, Clone)]
struct ReceivedData {
    mac: [u8; ESP_NOW_ETH_ALEN as usize],
    data: Vec<u8>, // This Vec<u8> now contains the fully framed data chunk
}

// --- Queue for Callback to Main Loop ---
const HEAPLESS_QUEUE_CAPACITY: usize = IMAGE_QUEUE_SIZE + 1; // Capacity

// Use Mutex<Option<...>> for safe static initialization
static RECEIVED_DATA_PRODUCER: Mutex<
    Option<Producer<'static, ReceivedData, HEAPLESS_QUEUE_CAPACITY>>,
> = Mutex::new(None);
static RECEIVED_DATA_CONSUMER: Mutex<
    Option<Consumer<'static, ReceivedData, HEAPLESS_QUEUE_CAPACITY>>,
> = Mutex::new(None);

// Static buffer for the queue itself using MaybeUninit
static mut Q_BUFFER: MaybeUninit<Queue<ReceivedData, HEAPLESS_QUEUE_CAPACITY>> =
    MaybeUninit::uninit();

// --- ESP-NOW Receive Callback ---
extern "C" fn esp_now_recv_cb(info: *const esp_now_recv_info_t, data: *const u8, data_len: i32) {
    if info.is_null() || (data.is_null() && data_len > 0) || data_len < 0 {
        error!("ESP-NOW CB: Invalid arguments received.");
        return;
    }

    let src_mac_ptr = unsafe { (*info).src_addr };
    if src_mac_ptr.is_null() {
        error!("ESP-NOW CB: Source MAC address pointer is null.");
        return;
    }

    let mac_array: [u8; ESP_NOW_ETH_ALEN as usize] = unsafe {
        match slice::from_raw_parts(src_mac_ptr, ESP_NOW_ETH_ALEN as usize).try_into() {
            Ok(arr) => arr,
            Err(_) => {
                error!("ESP-NOW CB: Failed to convert MAC address slice to array.");
                return;
            }
        }
    };

    let mac_str_log = format!(
        "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac_array[0], mac_array[1], mac_array[2], mac_array[3], mac_array[4], mac_array[5]
    );

    // Handle potential EOF marker (specific pattern) or regular data
    let data_slice = unsafe { slice::from_raw_parts(data, data_len as usize) };
    let is_eof = data_len == 4 && data_slice == b"EOF!"; // Check for specific EOF payload
    let is_hash = data_len > 5 && data_slice.starts_with(b"HASH:"); // Check for HASH payload

    // デバッグレベルでサマリーログを出力（重要な情報のみ）
    if is_eof {
        warn!(
            "ESP-NOW CB [{}]: Received EOF marker (b\"EOF!\").",
            mac_str_log
        );
    } else if is_hash {
        warn!("ESP-NOW CB [{}]: Received HASH marker.", mac_str_log);
    }

    // ----- 強化されたフレーム形式 -----
    // 改良：カメラを区別するためのメタデータをさらに強化

    // より明確に識別できるスタートマーカー (4バイト)
    let start_marker: [u8; 4] = 0xFACE_AABBu32.to_be_bytes();

    // より明確に識別できるエンドマーカー (4バイト)
    let end_marker: [u8; 4] = 0xCDEF_5678u32.to_be_bytes();

    // 各カメラのデータ送信シーケンス番号（EOFとHASHでリセット）
    // 静的なカウンター（MACアドレスごとに管理）
    static SEQUENCE_COUNTERS: Mutex<Option<std::collections::HashMap<[u8; 6], u32>>> =
        Mutex::new(None);

    // 初めて使用される場合はHashMapを初期化
    if SEQUENCE_COUNTERS.lock().unwrap().is_none() {
        *SEQUENCE_COUNTERS.lock().unwrap() = Some(std::collections::HashMap::new());
    }

    // シーケンスカウンターを取得または初期化
    let mut seq_num = 0;
    {
        let mut counters = SEQUENCE_COUNTERS.lock().unwrap();
        if let Some(ref mut counter_map) = *counters {
            // HASHまたはEOFマーカーを受け取った場合、シーケンス番号をリセット
            if is_hash || is_eof {
                counter_map.insert(mac_array, 0);
                seq_num = 0;
            } else {
                // 既存のカウンターを取得するか、新しいカウンターを作成
                let counter = counter_map.entry(mac_array).or_insert(0);
                *counter = counter.wrapping_add(1); // オーバーフロー対策
                seq_num = *counter;
            }
        }
    }

    // データ長情報 (4バイト)
    let data_len_bytes = (data_len as u32).to_be_bytes();

    // フレームタイプ（1バイト）: 1=HASH, 2=DATA, 3=EOF
    let frame_type: u8 = if is_hash {
        1
    } else if is_eof {
        3
    } else {
        2
    };

    // シーケンス番号 (4バイト)
    let seq_bytes = seq_num.to_be_bytes();

    // チェックサム（単純な実装として、データの最初の4バイトとXORする）
    let mut checksum: u32 = 0;
    for chunk in data_slice.chunks(4) {
        let mut val: u32 = 0;
        for (i, &b) in chunk.iter().enumerate() {
            val |= (b as u32) << (i * 8);
        }
        checksum ^= val;
    }
    let checksum_bytes = checksum.to_be_bytes();

    // ログでMACアドレスを確認するためのヘルパー関数
    let mac_hex_str = format!(
        "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac_array[0], mac_array[1], mac_array[2], mac_array[3], mac_array[4], mac_array[5]
    );
    debug!("MAC address for framing: {}", mac_hex_str);

    // フレームの合計長を計算
    let total_frame_len = start_marker.len() +   // 開始マーカー: 4バイト
        mac_array.len() +      // MACアドレス: 6バイト
        1 +                    // フレームタイプ: 1バイト
        seq_bytes.len() +      // シーケンス番号: 4バイト
        data_len_bytes.len() + // データ長: 4バイト
        data_slice.len() +     // 実データ: 可変長
        checksum_bytes.len() + // チェックサム: 4バイト
        end_marker.len(); // 終了マーカー: 4バイト

    let mut framed_data = Vec::with_capacity(total_frame_len);

    // フレームを構築
    framed_data.extend_from_slice(&start_marker); // 開始マーカー

    // MACアドレスのコピー - バイト配列をそのまま使用
    framed_data.extend_from_slice(&mac_array); // MACアドレス

    framed_data.push(frame_type); // フレームタイプ
    framed_data.extend_from_slice(&seq_bytes); // シーケンス番号
    framed_data.extend_from_slice(&data_len_bytes); // データ長
    framed_data.extend_from_slice(data_slice); // データ本体
    framed_data.extend_from_slice(&checksum_bytes); // チェックサム
    framed_data.extend_from_slice(&end_marker); // 終了マーカー

    // データ量が多いのでログレベルをdebugに下げる
    debug!(
        "ESP-NOW CB [{}]: Received chunk ({} bytes, type={}, seq={}). Framed: {} bytes.",
        mac_str_log,
        data_len,
        if is_hash {
            "HASH"
        } else if is_eof {
            "EOF"
        } else {
            "DATA"
        },
        seq_num,
        framed_data.len()
    );

    // For simplicity, let's reuse ReceivedData but 'data' now holds the *framed* chunk
    let framed_chunk_to_send = ReceivedData {
        mac: mac_array, // Keep MAC for potential logging in main loop if needed
        data: framed_data,
    };

    // Lock the producer end of the queue
    if let Ok(mut producer_guard) = RECEIVED_DATA_PRODUCER.lock() {
        if let Some(producer) = producer_guard.as_mut() {
            match producer.enqueue(framed_chunk_to_send) {
                // Enqueue the framed chunk
                Ok(_) => {
                    // Successful enqueue - no need for verbose logging here
                }
                Err(_) => {
                    // キューがいっぱいの場合の処理を改善
                    warn!(
                        "ESP-NOW CB [{}]: Data queue full! Dropping {} frame (seq={}).",
                        mac_str_log,
                        if is_hash {
                            "HASH"
                        } else if is_eof {
                            "EOF"
                        } else {
                            "DATA"
                        },
                        seq_num
                    );

                    // EOFフレームが落とされたことをエラーログに記録（重要なため）
                    if is_eof {
                        error!(
                            "ESP-NOW CB [{}]: CRITICAL! EOF frame dropped due to queue full!",
                            mac_str_log
                        );
                    }
                }
            }
        } else {
            error!(
                "ESP-NOW CB [{}]: Producer queue is not initialized.",
                mac_str_log
            );
        }
    } else {
        error!(
            "ESP-NOW CB [{}]: Failed to lock data queue producer.",
            mac_str_log
        );
    }
} // End of esp_now_recv_cb

fn main() -> Result<()> {
    // Use anyhow::Result
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();
    log::set_max_level(log::LevelFilter::Info);

    info!("Starting ESP-NOW USB CDC Receiver...");

    let peripherals = Peripherals::take().unwrap();
    let sysloop = EspSystemEventLoop::take()?;
    let nvs = EspDefaultNvsPartition::take()?; // NVS is needed for Wi-Fi init

    // --- Initialize Queue ---
    // This needs to be done before initializing WiFi/ESP-NOW which might use the callback
    // Initialize Queue using MaybeUninit
    unsafe {
        // Initialize the queue in the static buffer
        Q_BUFFER.write(Queue::new());
        // Get mutable reference to the initialized queue and split
        let (p, c) = Q_BUFFER.assume_init_mut().split();
        *RECEIVED_DATA_PRODUCER.lock().unwrap() = Some(p);
        *RECEIVED_DATA_CONSUMER.lock().unwrap() = Some(c);
    }
    info!("Data queue initialized.");

    // --- Wi-Fi Initialization (Required for ESP-NOW) ---
    info!("Initializing Wi-Fi in STA mode for ESP-NOW...");
    let mut wifi = EspWifi::new(peripherals.modem, sysloop.clone(), Some(nvs))?;

    // Set Wi-Fi storage to RAM to avoid NVS writes if not needed for connection persistence
    unsafe {
        esp_wifi_set_storage(wifi_storage_t_WIFI_STORAGE_RAM);
    }
    // Configure as STA mode (even without connecting) - SSID/Pass are not used for ESP-NOW RX
    wifi.set_configuration(&Configuration::Client(ClientConfiguration {
        ssid: heapless::String::new(),     // Empty SSID
        password: heapless::String::new(), // Empty Password
        auth_method: AuthMethod::None,     // No auth needed
        ..Default::default()
    }))?;

    wifi.start()?; // Start Wi-Fi in STA mode
    info!("Wi-Fi driver started in STA mode.");

    // Disable Wi-Fi power save for better ESP-NOW responsiveness
    unsafe {
        esp_wifi_set_ps(wifi_ps_type_t_WIFI_PS_NONE);
    }
    info!("Wi-Fi Power Save disabled.");

    // --- ESP-NOW Initialization ---
    info!("Initializing ESP-NOW...");
    unsafe {
        // Ensure ESP-NOW is initialized *after* Wi-Fi is started
        esp_now_init();
        esp_now_register_recv_cb(Some(esp_now_recv_cb));

        // ESP-NOWの最大ピア数を確認するためにログ出力
        let mut esp_now_peer_num = esp_idf_svc::sys::esp_now_peer_num_t {
            total_num: 0,
            encrypt_num: 0,
        };
        // 正しいポインタ型（i32）を使用
        if esp_idf_svc::sys::esp_now_get_peer_num(&mut esp_now_peer_num) == 0 {
            info!(
                "ESP-NOW: Current peer count: {}",
                esp_now_peer_num.total_num
            );
            info!("ESP-NOW: Maximum supported peers: 20"); // ESP-IDF 4.xでは20ピアをサポート
        } else {
            error!("ESP-NOW: Failed to get peer count");
        }

        // カメラのMACアドレスをESP-NOWピアとして登録
        let cameras = [
            // cfg.tomlから取得したカメラのMACアドレス
            "34:ab:95:fa:3a:6c", // cam1
            "34:ab:95:fb:3f:c4", // cam2
            "78:21:84:3e:d7:d4", // cam3 - この送信に失敗しているカメラ
            "34:ab:95:fb:d0:6c", // cam4
        ];

        for cam_mac_str in cameras.iter() {
            // MACアドレスをパースする
            let parts: Vec<&str> = cam_mac_str.split(':').collect();
            if parts.len() == 6 {
                let mut mac = [0u8; 6];
                let mut parse_success = true;

                for (i, part) in parts.iter().enumerate() {
                    match u8::from_str_radix(part, 16) {
                        Ok(val) => mac[i] = val,
                        Err(_) => {
                            parse_success = false;
                            error!("ESP-NOW: Failed to parse MAC address part: {}", part);
                            break;
                        }
                    }
                }

                if parse_success {
                    let mut peer_info = esp_idf_svc::sys::esp_now_peer_info_t::default();
                    peer_info.channel = 0; // Use current channel
                    peer_info.ifidx = esp_idf_svc::sys::wifi_interface_t_WIFI_IF_STA; // Use STA interface for receiving
                    peer_info.encrypt = false; // No encryption
                    peer_info.peer_addr = mac;

                    let add_result = esp_idf_svc::sys::esp_now_add_peer(&peer_info);
                    if add_result == 0 {
                        let mac_str = mac
                            .iter()
                            .enumerate()
                            .map(|(i, &b)| {
                                if i < 5 {
                                    format!("{:02x}:", b)
                                } else {
                                    format!("{:02x}", b)
                                }
                            })
                            .collect::<String>();
                        info!("ESP-NOW: Added camera as peer: {}", mac_str);
                    } else {
                        error!(
                            "ESP-NOW: Failed to add camera {} as peer: {}",
                            cam_mac_str, add_result
                        );
                    }
                }
            } else {
                error!("ESP-NOW: Invalid MAC address format: {}", cam_mac_str);
            }
        }

        // ESP-NOW添付ファイル(PMK)の最大数を拡大（デフォルトは6）
        let pmk: [u8; 16] = [
            0x50, 0x4d, 0x4b, 0x5f, 0x4b, 0x45, 0x59, 0x5f, 0x42, 0x59, 0x5f, 0x43, 0x55, 0x53,
            0x54, 0x4f,
        ];
        let pmk_result = esp_idf_svc::sys::esp_now_set_pmk(pmk.as_ptr());

        if pmk_result != 0 {
            error!("ESP-NOW: Failed to set PMK: {}", pmk_result);
        }
    }
    info!("ESP-NOW Initialized and receive callback registered with expanded peer capacity."); // --- USB CDC Initialization ---
    info!("Initializing USB CDC...");
    let mut config = UsbSerialConfig::new();
    // USB CDCではバッファサイズの設定が重要（ボーレートではなく）
    // 送受信バッファサイズを増加させてスループットを改善
    // より多くのカメラからのデータを同時に処理するために大きなバッファを使用
    config.tx_buffer_size = 4096; // 送信バッファを4096バイトに拡大
    config.rx_buffer_size = 4096; // 受信バッファを4096バイトに拡大

    let mut usb_serial_driver = UsbSerialDriver::new(
        peripherals.usb_serial,
        peripherals.pins.gpio18, // Check XIAO ESP32C3 pinout for USB D- (Should be correct for XIAO C3)
        peripherals.pins.gpio19, // Check XIAO ESP32C3 pinout for USB D+ (Should be correct for XIAO C3)
        &config,
    )?;
    info!("USB CDC Initialized with increased buffer sizes (TX/RX: 2048 bytes).");

    // --- Main Loop (Dequeue and Send via USB CDC) ---
    info!("Entering main loop...");
    loop {
        // Main loop start
        if let Ok(mut consumer_guard) = RECEIVED_DATA_CONSUMER.lock() {
            if let Some(consumer) = consumer_guard.as_mut() {
                while let Some(received_data) = consumer.dequeue() {
                    // Inner loop for consuming queue
                    let mac_str = format!(
                        "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                        received_data.mac[0],
                        received_data.mac[1],
                        received_data.mac[2],
                        received_data.mac[3],
                        received_data.mac[4],
                        received_data.mac[5]
                    );
                    // Log length of the *framed* data being sent
                    debug!( // Log levelをdebugに変更
                        "Main Loop: Dequeued framed data from {}. Sending via USB CDC ({} bytes total frame).",
                        mac_str,
                        received_data.data.len()
                    );
                    // Data is already framed from the callback, just get the Vec<u8>
                    let data_to_send = received_data.data; // data field now holds the framed chunk

                    // Write framed data to USB CDC with improved error handling and retry logic
                    let mut bytes_sent = 0;
                    let mut write_error = false;
                    let write_start_time = unsafe { sys::xTaskGetTickCount() };

                    // フレーム送信タイムアウトを延長 (30秒に延長)
                    const WRITE_TIMEOUT_MS: u32 = 30000; // 30 second timeout for sending one frame
                    let mut timeout_logged = false;
                    let mut retry_count = 0;
                    const MAX_RETRIES: u32 = 5;

                    // 送信バッファサイズを小さくして、より頻繁に書き込みを試みる
                    const MAX_CHUNK_SIZE: usize = 64; // USBバッファサイズに合わせて調整

                    while bytes_sent < data_to_send.len() {
                        // タイムアウトチェック
                        let write_timeout_ticks = (WRITE_TIMEOUT_MS as u64
                            * sys::configTICK_RATE_HZ as u64
                            / 1000) as u32;
                        if unsafe { sys::xTaskGetTickCount() } - write_start_time
                            > write_timeout_ticks
                        {
                            error!(
                                "Main Loop: Timeout sending framed chunk via USB CDC for {}. Sent {}/{}",
                                mac_str, bytes_sent, data_to_send.len()
                            );
                            write_error = true;
                            break; // Exit chunk sending loop
                        }

                        // 小さなバッファで書き込み
                        let remaining = data_to_send.len() - bytes_sent;
                        let write_size = if remaining > MAX_CHUNK_SIZE {
                            MAX_CHUNK_SIZE
                        } else {
                            remaining
                        };
                        let chunk_to_write = &data_to_send[bytes_sent..(bytes_sent + write_size)];

                        // タイムアウト10msで書き込み試行（完全に0ではなく少し待つ）
                        match usb_serial_driver.write(chunk_to_write, 10) {
                            Ok(written) => {
                                if written > 0 {
                                    bytes_sent += written;
                                    retry_count = 0; // リトライカウンタリセット
                                    timeout_logged = false;

                                    // データ書き込みに成功した場合のログ（詳細レベル）
                                    debug!(
                                        "USB Write: {} bytes (Total: {}/{} - {:.1}%)",
                                        written,
                                        bytes_sent,
                                        data_to_send.len(),
                                        (bytes_sent as f32 / data_to_send.len() as f32) * 100.0
                                    );
                                } else {
                                    // 書き込みは成功したが0バイト
                                    retry_count += 1;
                                    if retry_count >= MAX_RETRIES {
                                        warn!("Main Loop: Max retries ({}) reached with 0 bytes written", MAX_RETRIES);
                                        FreeRtos::delay_ms(50); // より長く待機
                                        retry_count = 0; // リトライカウンタリセット
                                    }
                                    FreeRtos::delay_ms(5);
                                }
                            }
                            Err(e) => {
                                if e.code() == esp_idf_svc::sys::ESP_ERR_TIMEOUT {
                                    // タイムアウト（バッファフル）の場合
                                    retry_count += 1;
                                    if !timeout_logged {
                                        debug!("USB Write Timeout (Buffer Full?) for {}", mac_str);
                                        timeout_logged = true;
                                    }

                                    if retry_count >= MAX_RETRIES {
                                        warn!(
                                            "Main Loop: Max retries ({}) reached due to timeouts",
                                            MAX_RETRIES
                                        );
                                        FreeRtos::delay_ms(50); // より長く待機
                                        retry_count = 0;
                                    } else {
                                        FreeRtos::delay_ms(10);
                                    }
                                } else {
                                    error!(
                                        "Main Loop: Error writing framed chunk to USB CDC for {}: {:?}",
                                        mac_str, e
                                    );
                                    write_error = true;
                                    break; // その他のエラーでは送信ループを終了
                                }
                            }
                        }
                    } // 送信ループの終了

                    if !write_error {
                        debug!( // Log levelをdebugに変更
                            "Main Loop: Successfully sent framed chunk ({} bytes total) via USB CDC for {}.",
                            bytes_sent, mac_str
                        );
                    } else {
                        // Error already logged
                        warn!(
                            "Main Loop: Failed to send complete framed chunk via USB CDC for {} (sent {} out of {} bytes).",
                            mac_str, bytes_sent, data_to_send.len()
                        );
                    }
                    // フレーム送信後の遅延は維持 (ホスト側の処理時間を考慮)
                    FreeRtos::delay_ms(5);
                } // End of while let Some(received_data)
            } else {
                // End of if let Some(consumer)
                // This case should ideally not happen if initialization is correct
                error!("Main Loop: Consumer queue is not initialized.");
                // Delay to prevent spamming logs if this state persists
                FreeRtos::delay_ms(1000);
            }
        } else {
            // End of if let Ok(mut consumer_guard)
            error!("Main Loop: Failed to lock data queue consumer.");
            // Delay if lock fails
            FreeRtos::delay_ms(100);
        }

        // Small delay to prevent busy-waiting and yield to other tasks
        FreeRtos::delay_ms(10);
    } // End of main loop
} // End of main function
