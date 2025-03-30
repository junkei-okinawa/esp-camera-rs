use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::{
    delay::FreeRtos,
    gpio,
    peripherals::Peripherals,
    uart::{config, UartDriver},
    // mutex::Mutex, // Removed, using std::sync::Mutex
    // task, // Removed, using std::thread instead
    // queue::Queue, // Removed, using std::sync::mpsc
};
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::wifi::{EspWifi, BlockingWifi, ClientConfiguration, Configuration};
use esp_idf_svc::sys::{
    esp_now_init, esp_now_recv_info_t, esp_now_register_recv_cb, // esp_wifi_set_channel removed
    /* wifi_second_chan_t_WIFI_SECOND_CHAN_NONE, */ esp_now_add_peer, esp_now_peer_info_t, // Add for peer registration
    wifi_interface_t_WIFI_IF_STA, // Need this for peer_info.ifidx
};
use log::{error, info, warn};
use std::cell::RefCell;
use std::slice;
use std::sync::{Arc, Mutex}; // Import std::sync::Mutex
use std::sync::mpsc::{channel, Sender, Receiver}; // Import mpsc channel
use sha2::{Digest, Sha256}; // Add sha2 imports
use hex; // To convert hash bytes to hex string for comparison/logging

const SENDER_MAC_ADDRESS: [u8; 6] = [0x78, 0x21, 0x84, 0x3e, 0xd7, 0xd4]; // MAC address of camera-example

// Global state to store the hash received from the sender
static RECEIVED_HASH: Mutex<RefCell<Option<String>>> = Mutex::new(RefCell::new(None));

// UARTドライバー、送信チャネル、画像バッファをスレッドセーフに共有するための構造体
struct SharedState {
    uart: Mutex<RefCell<Option<UartDriver<'static>>>>,
    tx_channel: Sender<Vec<u8>>, // UART送信タスクへの送信チャネル (mpsc::Sender)
    image_buffer: Mutex<RefCell<Vec<u8>>>, // ESP-NOW受信データを一時的に蓄積するバッファ
}

// static変数で状態を保持 (型は変更なし、中身の SharedState が変わる)
static SHARED_STATE: Mutex<RefCell<Option<Arc<SharedState>>>> = Mutex::new(RefCell::new(None));

// 受信コールバック関数
extern "C" fn esp_now_recv_cb(info: *const esp_now_recv_info_t, data: *const u8, data_len: i32) {
    if info.is_null() {
        error!("ESP-NOW CB: Received null info pointer");
        return;
    }
    // data can be null if data_len is 0 (empty packet marker)
    if data.is_null() && data_len > 0 {
         error!("ESP-NOW CB: Received null data pointer with non-zero length");
         return;
    }
    if data_len < 0 {
        error!("ESP-NOW CB: Received negative data length");
        return;
    }

    // グローバル状態へのアクセスを試みる
    if let Ok(state_opt_cell_guard) = SHARED_STATE.lock() { // Remove closure argument
        // LockResult<MutexGuard<RefCell<Option<Arc<SharedState>>>>>
        // MutexGuardを介してRefCellにアクセス
        if let Some(state_arc) = state_opt_cell_guard.borrow().as_ref() {
             // Clone the Arc<SharedState> for use inside the buffer lock
             let state_arc_clone = state_arc.clone();

            // Process received data
            let data_slice = unsafe { slice::from_raw_parts(data, data_len as usize) };

            // Check for HASH marker
            if data_slice.starts_with(b"HASH:") {
                if let Ok(hash_str) = std::str::from_utf8(&data_slice[5..]) {
                    info!("ESP-NOW CB: Received HASH: {}", hash_str);
                    if let Ok(mut received_hash_guard) = RECEIVED_HASH.lock() {
                        received_hash_guard.replace(Some(hash_str.to_string()));
                    } else {
                        error!("ESP-NOW CB: Failed to lock RECEIVED_HASH");
                    }
                    // Clear image buffer when hash is received, assuming it marks the start of a new image
                    if let Ok(buffer_guard) = state_arc_clone.image_buffer.lock() {
                         if !buffer_guard.borrow().is_empty() {
                            warn!("ESP-NOW CB: Received HASH while image buffer was not empty. Clearing buffer.");
                            buffer_guard.borrow_mut().clear();
                         }
                    } else {
                         error!("ESP-NOW CB: Failed to lock image_buffer to clear on HASH");
                    }
                } else {
                    error!("ESP-NOW CB: Received invalid UTF-8 in HASH message");
                }
            }
            // Check for EOF marker
            else if data_slice == b"EOF" {
                info!("ESP-NOW CB: Received EOF marker");
                // Lock the buffer to calculate hash, compare, send data, and clear
                if let Ok(buffer_guard) = state_arc_clone.image_buffer.lock() {
                    if !buffer_guard.borrow().is_empty() {
                        // Calculate hash of received data
                        let mut hasher = Sha256::new();
                        hasher.update(&*buffer_guard.borrow()); // Dereference Ref to pass &Vec<u8>
                        let calculated_hash_result = hasher.finalize();
                        let calculated_hash_hex = format!("{:x}", calculated_hash_result);
                        info!("ESP-NOW CB: Calculated hash: {}", calculated_hash_hex);

                        // Compare with received hash
                        if let Ok(received_hash_guard) = RECEIVED_HASH.lock() {
                             if let Some(ref received_hash) = *received_hash_guard.borrow() {
                                if *received_hash == calculated_hash_hex {
                                    info!("ESP-NOW CB: Hash verification successful!");
                                } else {
                                    error!("ESP-NOW CB: HASH MISMATCH! Received: {}, Calculated: {}", received_hash, calculated_hash_hex);
                                }
                             } else {
                                 warn!("ESP-NOW CB: No hash received before EOF.");
                              }
                              // received_hash_guard.replace(None); // Clear hash later in UART task
                         } else {
                              error!("ESP-NOW CB: Failed to lock RECEIVED_HASH for comparison");
                         }


                        // Send data to UART channel
                        let full_image_data = buffer_guard.borrow().clone();
                        info!("ESP-NOW CB: Sending full image ({} bytes) to UART channel", full_image_data.len());
                        if let Err(e) = state_arc_clone.tx_channel.send(full_image_data) {
                            error!("ESP-NOW CB: Failed to send full image data to channel: {}", e);
                        }
                        // Clear buffer after processing
                        buffer_guard.borrow_mut().clear();
                    } else {
                        warn!("ESP-NOW CB: Received EOF marker, but buffer was empty.");
                         // Clear received hash if it exists
                         if let Ok(mut received_hash_guard) = RECEIVED_HASH.lock() {
                            received_hash_guard.replace(None);
                         }
                    }
                } else {
                    error!("ESP-NOW CB: Failed to lock image_buffer to process EOF marker");
                }
            } else if data_len > 0 {
                 // Regular data chunk, append to buffer
                 if let Ok(buffer_guard) = state_arc_clone.image_buffer.lock() {
                     buffer_guard.borrow_mut().extend_from_slice(data_slice);
                 } else {
                     error!("ESP-NOW CB: Failed to lock image_buffer to append data");
                 }
            }
            // Ignore data_len == 0 case if it happens unexpectedly

        } else { // This else corresponds to `if let Some(state_arc) = ...`
            error!("ESP-NOW CB: SHARED_STATE not initialized");
        }
    } else {
        error!("ESP-NOW CB: Failed to lock SHARED_STATE");
    }
}

// UART送信タスク (Arc<SharedState> と Receiver を受け取る)
fn uart_sender_task(shared_state: Arc<SharedState>, rx_channel: Receiver<Vec<u8>>) {
    info!("UART sender task started");
    loop {
        // チャネルからデータを受信 (ブロック)
        match rx_channel.recv() {
            Ok(data_vec) => {
                let total_len = data_vec.len();
                if total_len == 0 {
                    warn!("UART Task: Received empty data vector from channel, ignoring.");
                    continue;
                }
                info!("UART Task: Received full image ({} bytes) from channel. Sending hash and chunks...", total_len);

                // Send HASH first
                let hash_to_send: Option<String> = if let Ok(mut guard) = RECEIVED_HASH.lock() {
                    // Clone the Option<String> inside the RefCell and clear it
                    let hash_opt = guard.borrow().clone();
                    guard.replace(None); // Clear the hash after reading
                    hash_opt
                } else {
                    error!("UART Task: Failed to lock RECEIVED_HASH to get hash for sending.");
                    None
                };

                if let Some(hash_str) = hash_to_send {
                    let hash_message = format!("HASH:{}\n", hash_str); // Add newline for easier parsing on Raspi
                    if let Ok(uart_guard) = shared_state.uart.lock() {
                        if let Some(uart) = uart_guard.borrow_mut().as_mut() {
                            if let Err(e) = uart.write(hash_message.as_bytes()) {
                                error!("UART Task: Failed to write HASH marker: {}", e);
                                continue; // Skip sending image data if hash fails
                            }
                            info!("UART Task: Sent HASH marker: {}", hash_str);
                        } else {
                             warn!("UART Task: Driver not available when sending HASH.");
                             continue; // Skip sending image data
                        }
                    } else {
                         error!("UART Task: Failed to lock UART Mutex when sending HASH.");
                         continue; // Skip sending image data
                    }
                } else {
                    warn!("UART Task: No hash available to send.");
                    // Skip sending the image if hash is missing.
                    continue;
                }


                // Send image data in chunks
                const UART_CHUNK_SIZE: usize = 250; // Define chunk size for UART
                // let mut success = true; // Removed unused variable

                for chunk in data_vec.chunks(UART_CHUNK_SIZE) {
                    let chunk_len = chunk.len();
                    if chunk_len == 0 { continue; }

                    // Lock UART driver for each chunk
                    if let Ok(uart_guard) = shared_state.uart.lock() {
                        if let Some(uart) = uart_guard.borrow_mut().as_mut() {
                            // Send chunk length (u16 little-endian)
                            let len_bytes = (chunk_len as u16).to_le_bytes();
                            if let Err(e) = uart.write(&len_bytes) {
                                error!("UART Task: Failed to write chunk length {}: {}", chunk_len, e);
                                break; // Exit inner loop on error
                            }

                            // Send chunk data
                            if let Err(e) = uart.write(chunk) {
                                error!("UART Task: Failed to write chunk data ({} bytes): {}", chunk_len, e);
                                break; // Exit inner loop on error
                            }
                            // info!("UART Task: Sent chunk: {} bytes (len + data)", len_bytes.len() + chunk_len);

                            // Add a small delay between chunks
                            FreeRtos::delay_ms(5); // 5ms delay

                        } else {
                            warn!("UART Task: Driver not available (Option is None) while sending chunk.");
                            break; // Exit inner loop if driver is None
                        }
                    } else {
                        error!("UART Task: Failed to lock UART driver Mutex while sending chunk.");
                        break; // Exit inner loop if lock fails
                    }
                }
                info!("UART Task: Finished sending chunks for the image.");

            } // ここが Ok(data_vec) アームの終わり
            Err(e) => {
                // チャネル受信エラー (送信側がドロップされたなど)
                error!("UART Task: Failed to receive from channel: {}. Exiting task.", e);
                break; // ループを抜けてタスクを終了
            }
        } // ここが match rx_channel.recv() の終わり
    } // ここが loop の終わり
} // ここが uart_sender_task 関数の終わり


fn main() -> anyhow::Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();
    log::set_max_level(log::LevelFilter::Off); // Turn off logging completely to avoid UART interference

    // info!("initializing peripherals"); // This log will not be shown now
    let peripherals = Peripherals::take().unwrap();
    let sysloop = EspSystemEventLoop::take()?;
    let nvs = EspDefaultNvsPartition::take()?;

    info!("initializing WiFi peripheral for ESP-NOW");
    let mut wifi = BlockingWifi::wrap(
        EspWifi::new(peripherals.modem, sysloop.clone(), Some(nvs))?,
        sysloop,
    )?;
    wifi.set_configuration(&Configuration::Client(ClientConfiguration::default()))?;
    wifi.start()?;
    info!("WiFi peripheral started in STA mode for ESP-NOW");

    // // Set WiFi channel to 1 for ESP-NOW (Commented out to match image_reciver branch behavior)
    // info!("Setting WiFi channel to 1 for ESP-NOW");
    // unsafe {
    //     // First argument is primary channel, second is secondary channel offset
    //     let result = esp_wifi_set_channel(1, wifi_second_chan_t_WIFI_SECOND_CHAN_NONE);
    //     if result == esp_idf_svc::sys::ESP_OK {
    //         info!("Successfully set WiFi channel to 1");
    //     } else {
    //         error!("Failed to set WiFi channel to 1: {}", result);
    //         // 必要であればここでエラー処理を追加
    //     }
    // }

    let mac = wifi.wifi().sta_netif().get_mac()?;
    info!("Receiver MAC Address (STA): {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]);

    info!("Initializing UART");
    let tx = peripherals.pins.gpio21;
    let rx = peripherals.pins.gpio20;
    let config = config::Config::new().baudrate(115200.into()); // ボーレートを115200に変更
    let uart = UartDriver::new(
        peripherals.uart0,
        tx,
        rx,
        Option::<gpio::AnyIOPin>::None,
        Option::<gpio::AnyIOPin>::None,
        &config,
    )?;
    info!("UART Initialized (TX: GPIO21, RX: GPIO20, Baud: 115200)");

    // mpscチャネルを作成 (バッファなしチャネル)
    let (tx, rx): (Sender<Vec<u8>>, Receiver<Vec<u8>>) = channel();

    // 共有状態を作成し、static変数に格納 (Senderを渡す)
    let shared_state = Arc::new(SharedState {
        uart: Mutex::new(RefCell::new(Some(uart))),
        tx_channel: tx, // Pass the sender to SharedState
        image_buffer: Mutex::new(RefCell::new(Vec::new())),
    });
    // SHARED_STATE のロックと設定は Mutex<RefCell<...>> のまま
    if let Ok(state_cell) = SHARED_STATE.lock() { // Remove mut
         state_cell.replace(Some(shared_state.clone()));
    } else {
        error!("Failed to lock SHARED_STATE for initialization");
        // エラー処理: ここで panic するか、他の方法で処理する
        panic!("Could not initialize shared state");
    }


    info!("Initializing ESP-NOW");
    unsafe {
        esp_now_init();
        esp_now_register_recv_cb(Some(esp_now_recv_cb));

        // Register the sender as a peer
        info!("Registering ESP-NOW peer: {:02X?}", SENDER_MAC_ADDRESS);
        let mut peer_info = esp_now_peer_info_t::default();
        peer_info.channel = 0; // Use current channel (like image_reciver branch)
        peer_info.ifidx = wifi_interface_t_WIFI_IF_STA;
        peer_info.encrypt = false;
        peer_info.peer_addr = SENDER_MAC_ADDRESS;

        let result = esp_now_add_peer(&peer_info);
        if result == esp_idf_svc::sys::ESP_OK {
            info!("ESP-NOW peer registered successfully");
        } else {
            error!("Failed to register ESP-NOW peer: {}", result);
            // Consider error handling, maybe panic?
        }
    }
    info!("ESP-NOW Initialized and peer registered. Waiting for image data...");
// UART送信タスクを生成 (std::threadを使用)
let _sender_task_handle = std::thread::Builder::new()
    .name("uart_sender".into()) // スレッド名を設定
    .stack_size(4096) // スタックサイズを設定 (必要に応じて調整)
    .spawn(move || {
        // Pass the whole Arc<SharedState> and the receiver channel
        uart_sender_task(shared_state, rx); // Pass shared_state directly
    })
    .expect("Failed to spawn UART sender thread");
    // Removed erroneous line: })?;


    // メインループは単純な待機
    loop {
        FreeRtos::delay_ms(10000); // 10秒ごと
    }
}