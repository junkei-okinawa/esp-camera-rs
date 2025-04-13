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

    if is_eof {
        warn!(
            "ESP-NOW CB [{}]: Received EOF marker (b\"EOF!\").",
            mac_str_log
        );
    }

    // Frame each received ESP-NOW packet individually before enqueuing
    let start_marker: [u8; 2] = 0xAAAAu16.to_be_bytes();
    let end_marker: [u8; 2] = 0xBBBBu16.to_be_bytes();
    // Use actual received data_len for the length field in the frame
    let data_len_bytes = (data_len as u32).to_be_bytes();

    let total_frame_len = start_marker.len()
        + mac_array.len()
        + data_len_bytes.len()
        + data_slice.len() // Use actual data slice length here
        + end_marker.len();
    let mut framed_data = Vec::with_capacity(total_frame_len);

    framed_data.extend_from_slice(&start_marker);
    framed_data.extend_from_slice(&mac_array);
    framed_data.extend_from_slice(&data_len_bytes); // Send original data_len
    framed_data.extend_from_slice(data_slice);
    framed_data.extend_from_slice(&end_marker);

    // Enqueue the already framed data chunk
    // Log received chunk info (including EOF marker)
    info!(
        "ESP-NOW CB [{}]: Received chunk ({} bytes data, EOF={}). Enqueuing framed data ({} bytes total).",
        mac_str_log,
        data_len,
        is_eof,
        framed_data.len()
    );

    // Use a different struct or just Vec<u8> for the queue if only sending framed data
    // For simplicity, let's reuse ReceivedData but 'data' now holds the *framed* chunk
    let framed_chunk_to_send = ReceivedData {
        mac: mac_array, // Keep MAC for potential logging in main loop if needed
        data: framed_data,
    };
    // Removed extra closing brace and semicolon from previous diff attempt

    // Lock the producer end of the queue
    if let Ok(mut producer_guard) = RECEIVED_DATA_PRODUCER.lock() {
        if let Some(producer) = producer_guard.as_mut() {
            match producer.enqueue(framed_chunk_to_send) {
                // Enqueue the framed chunk
                Ok(_) => {
                    // info!("ESP-NOW CB [{}]: Framed chunk successfully enqueued.", mac_str_log)
                }
                Err(_) => warn!(
                    "ESP-NOW CB [{}]: Data queue full! Dropping framed chunk.",
                    mac_str_log
                ),
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
    }
    info!("ESP-NOW Initialized and receive callback registered."); // --- USB CDC Initialization ---
    info!("Initializing USB CDC...");
    let mut config = UsbSerialConfig::new();
    // USB CDCではバッファサイズの設定が重要（ボーレートではなく）
    // 送受信バッファサイズを増加させてスループットを改善
    config.tx_buffer_size = 2048; // 送信バッファを2048バイトに設定
    config.rx_buffer_size = 2048; // 受信バッファを2048バイトに設定

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
