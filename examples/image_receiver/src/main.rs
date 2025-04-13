use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::{delay::FreeRtos, peripherals::Peripherals};
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::sys::{
    esp_now_init, esp_now_recv_info_t, esp_now_register_recv_cb, esp_wifi_set_ps,
    wifi_ps_type_t_WIFI_PS_NONE,
};
use esp_idf_svc::wifi::{AuthMethod, BlockingWifi, ClientConfiguration, EspWifi};

use esp_idf_svc::mqtt::client::{
    EspMqttClient, Message, MqttClientConfiguration, PublishResult, QoS,
};

use std::io::Write;

use heapless::spsc::{Consumer, Producer, Queue};

use log::{error, info, warn};
use std::cell::RefCell;
use std::collections::HashMap;
use std::mem;
use std::slice;
use std::sync::Mutex;

use anyhow::Result;
use esp_idf_hal::prelude::*;

// --- Configuration Constants ---
const WIFI_SSID: &str = "ESP32-Image-Network";
const WIFI_PASS: &str = "your_secure_password"; // CHANGE THIS!
const MQTT_BROKER_URL: &str = "mqtt://192.168.4.1:1883"; // MQTT Broker URLを設定
const MQTT_TOPIC_PREFIX: &str = "esp32/images/";
const IMAGE_QUEUE_SIZE: usize = 5; // Max number of images to queue before dropping

const MQTT_CLIENT_ID: &str = "esp32-image-receiver-client"; // MQTT Client IDを設定

// --- Sender State Management ---
#[derive(Default, Debug, Clone)]
struct SenderState {
    hash: Option<String>,
    buffer: Vec<u8>,
    receiving: bool,
}

const MAX_BUFFER_SIZE: usize = 65536; // 64KB buffer limit per sender

static SENDER_STATES: Mutex<RefCell<HashMap<[u8; 6], SenderState>>> =
    Mutex::new(RefCell::new(HashMap::new()));

// --- Data Structure for Queue ---
#[derive(Debug)]
struct ImageToPublish {
    mac: [u8; 6],
    data: Vec<u8>,
}

// --- Queue for Callback to Main Loop ---
const HEAPLESS_QUEUE_CAPACITY: usize = IMAGE_QUEUE_SIZE + 1; // Capacity 7
static IMAGE_QUEUE: (
    Mutex<Producer<'static, ImageToPublish, HEAPLESS_QUEUE_CAPACITY>>,
    Mutex<Consumer<'static, ImageToPublish, HEAPLESS_QUEUE_CAPACITY>>,
) = {
    static mut Q_BUFFER: Queue<ImageToPublish, HEAPLESS_QUEUE_CAPACITY> = Queue::new();
    let (p, c) = unsafe { Q_BUFFER.split() };
    (Mutex::new(p), Mutex::new(c))
};

// カスタムエラー型を作成（TcpErrorトレイトを実装）
#[derive(Debug)]
struct EspNetError(std::io::Error);

impl From<std::io::Error> for EspNetError {
    fn from(error: std::io::Error) -> Self {
        EspNetError(error)
    }
}

// --- ESP-NOW Receive Callback ---
extern "C" fn esp_now_recv_cb(info: *const esp_now_recv_info_t, data: *const u8, data_len: i32) {
    // ... (MAC address extraction and basic checks remain the same) ...
    if info.is_null() || data.is_null() && data_len > 0 {
        /* ... */
        return;
    }
    let src_mac_ptr = unsafe { (*info).src_addr };
    if src_mac_ptr.is_null() {
        /* ... */
        return;
    }
    let mac_array: [u8; 6] = unsafe {
        /* ... */
        match slice::from_raw_parts(src_mac_ptr, 6).try_into() {
            Ok(a) => a,
            Err(_) => return,
        }
    };
    let mac_str_log = format!(
        "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac_array[0], mac_array[1], mac_array[2], mac_array[3], mac_array[4], mac_array[5]
    );
    if data_len < 0 {
        /* ... */
        return;
    }

    // Access sender states
    if let Ok(states_map_guard) = SENDER_STATES.lock() {
        let mut states_map = states_map_guard.borrow_mut();
        let sender_state = states_map.entry(mac_array).or_default();
        let data_slice = unsafe { slice::from_raw_parts(data, data_len as usize) };

        if data_slice.starts_with(b"HASH:") {
            // ... (HASH processing logic remains the same) ...
            if let Ok(hash_str) = std::str::from_utf8(&data_slice[5..]) {
                if !sender_state.buffer.is_empty() {
                    warn!(
                        "ESP-NOW CB [{}]: Received HASH while buffer not empty. Forcing reset.",
                        mac_str_log
                    );
                    sender_state.buffer.clear();
                }
                sender_state.hash = Some(hash_str.to_string());
                sender_state.receiving = true;
                info!(
                    "ESP-NOW CB [{}]: Started receiving image (HASH received).",
                    mac_str_log
                );
            } else {
                error!(
                    "ESP-NOW CB [{}]: Received invalid UTF-8 in HASH message",
                    mac_str_log
                );
                sender_state.hash = None;
                sender_state.buffer.clear();
                sender_state.receiving = false;
            }
        } else if data_slice == b"EOF" {
            info!("ESP-NOW CB [{}]: Received EOF marker", mac_str_log);
            if sender_state.receiving {
                if !sender_state.buffer.is_empty() {
                    let image_data = mem::take(&mut sender_state.buffer); // Take ownership
                    let _received_hash = sender_state.hash.clone(); // For logging

                    // --- Enqueue data for publishing ---
                    info!(
                        "ESP-NOW CB [{}]: Image received ({} bytes). Enqueuing for MQTT publish.",
                        mac_str_log,
                        image_data.len()
                    );

                    // Optional: Hash verification log
                    // ... (hash verification logic remains the same) ...

                    let image_to_publish = ImageToPublish {
                        mac: mac_array,
                        data: image_data,
                    };

                    // Lock the producer end of the queue
                    if let Ok(mut producer) = IMAGE_QUEUE.0.lock() {
                        match producer.enqueue(image_to_publish) {
                            Ok(_) => {
                                info!("ESP-NOW CB [{}]: Image successfully enqueued.", mac_str_log)
                            }
                            Err(_) => warn!(
                                "ESP-NOW CB [{}]: Image queue full! Dropping image.",
                                mac_str_log
                            ),
                        }
                    } else {
                        error!(
                            "ESP-NOW CB [{}]: Failed to lock image queue producer.",
                            mac_str_log
                        );
                    }

                    // Reset state AFTER attempting enqueue
                    sender_state.hash = None;
                    sender_state.receiving = false;
                } else {
                    // EOF received but buffer empty
                    warn!(
                        "ESP-NOW CB [{}]: Received EOF marker, but buffer was empty.",
                        mac_str_log
                    );
                    sender_state.hash = None;
                    sender_state.receiving = false;
                }
            } else {
                // EOF received but not in receiving state
                warn!(
                    "ESP-NOW CB [{}]: Received EOF marker while not in receiving state. Ignoring.",
                    mac_str_log
                );
                sender_state.hash = None;
                sender_state.receiving = false;
            }
        } else if data_len > 0 && sender_state.receiving {
            // ... (Regular data chunk processing remains the same) ...
            if sender_state.buffer.len() + data_len as usize <= MAX_BUFFER_SIZE {
                sender_state.buffer.extend_from_slice(data_slice);
            } else {
                error!(
                    "ESP-NOW CB [{}]: Buffer overflow detected (current: {}, received: {}). Discarding image.",
                    mac_str_log, sender_state.buffer.len(), data_len
                );
                sender_state.hash = None;
                sender_state.buffer.clear();
                sender_state.receiving = false;
            }
        } else if data_len > 0 && !sender_state.receiving {
            warn!(
                "ESP-NOW CB [{}]: Received data chunk while not in receiving state. Discarding.",
                mac_str_log
            );
        }
        // Ignore data_len == 0
    } else {
        error!("ESP-NOW CB: Failed to lock SENDER_STATES");
    }
}

// --- Main Function ---
fn main() -> anyhow::Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();
    log::set_max_level(log::LevelFilter::Info);

    info!("Starting Image Receiver (MQTT Gateway - esp-mqtt)...");

    let peripherals = Peripherals::take().unwrap();
    let sysloop = EspSystemEventLoop::take()?;
    let nvs = EspDefaultNvsPartition::take()?;

    // --- Wi-Fi Connection ---
    info!("Initializing Wi-Fi...");
    let mut wifi = BlockingWifi::wrap(
        EspWifi::new(peripherals.modem, sysloop.clone(), None)?,
        sysloop.clone(),
    )?;
    connect_wifi(&mut wifi)?;
    let ip_info = wifi.wifi().sta_netif().get_ip_info()?;
    info!("Wi-Fi connected. IP info: {:?}", ip_info);

    // --- MQTT Configuration ---
    let mqtt_config = MqttClientConfiguration {
        client_id: Some(MQTT_CLIENT_ID),
        ..Default::default()
    };

    // --- MQTT Client Initialization ---
    info!("Initializing MQTT client...");
    let mqtt_client =
        EspMqttClient::new(
            MQTT_BROKER_URL,
            &mqtt_config,
            move |message_event| match message_event {
                Ok(msg) => match msg {
                    Message(payload) => match payload {
                        Received(rec) => {
                            info!("MQTT Received: {}", rec)
                        }
                        Connected(_con) => {
                            info!("MQTT Connected");
                        }
                        Disconnected => {
                            warn!("MQTT Disconnected");
                        }
                    },
                    _ => info!("MQTT Event: {:?}", msg),
                },
                Err(e) => error!("MQTT error: {:?}", e),
            },
        )?;

    info!("MQTT client initialized. Broker URL: {}", MQTT_BROKER_URL);

    // --- ESP-NOW Initialization ---
    info!("Initializing ESP-NOW...");
    unsafe {
        esp_now_init();
        esp_now_register_recv_cb(Some(esp_now_recv_cb));
    }
    info!("ESP-NOW Initialized.");

    // Power saving modeの設定 (必要に応じて)
    unsafe {
        esp_wifi_set_ps(wifi_ps_type_t_WIFI_PS_NONE); // 例: Power Save Mode None
    }

    // --- Main Loop (MQTT Poll and Publish from Queue) ---
    info!("Entering main loop...");
    let mut mqtt_connected = false;

    loop {
        // Check queue for images to publish
        if mqtt_connected {
            if let Ok(mut consumer) = IMAGE_QUEUE.1.lock() {
                while let Some(image_to_publish) = consumer.dequeue() {
                    let mac_str = format!(
                        "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                        image_to_publish.mac[0],
                        image_to_publish.mac[1],
                        image_to_publish.mac[2],
                        image_to_publish.mac[3],
                        image_to_publish.mac[4],
                        image_to_publish.mac[5]
                    );
                    let topic = format!("{}{}", MQTT_TOPIC_PREFIX, mac_str);
                    info!(
                        "Main Loop: Dequeued image from {}. Publishing ({} bytes) to {}",
                        mac_str,
                        image_to_publish.data.len(),
                        topic
                    );

                    // 最新のminimq APIに合わせて修正
                    match mqtt_client.publish(
                        &topic,
                        QoS::AtLeastOnce,
                        false,
                        &image_to_publish.data,
                    ) {
                        Ok(_) => info!("Main Loop: Published image successfully for {}", mac_str),
                        Err(e) => error!(
                            "Main Loop: Failed to publish image for {}: {:?}",
                            mac_str, e
                        ),
                    };
                }
            } else {
                error!("Main Loop: Failed to lock image queue consumer.");
            }
        }

        FreeRtos::delay_ms(100);

        // Delay before next loop iteration
        FreeRtos::delay_ms(100);
    }
}

// --- Helper Function for Wi-Fi Connection ---
fn connect_wifi(wifi: &mut BlockingWifi<EspWifi<'static>>) -> anyhow::Result<()> {
    // 直接espのConfigurationを使い、String型の問題を回避
    let wifi_configuration =
        esp_idf_svc::wifi::Configuration::Client(esp_idf_svc::wifi::ClientConfiguration {
            // heapless::Vecに直接変換
            ssid: heapless::String::from_utf8(
                heapless::Vec::from_slice(WIFI_SSID.as_bytes()).unwrap(),
            )
            .unwrap(),
            password: heapless::String::from_utf8(
                heapless::Vec::from_slice(WIFI_PASS.as_bytes()).unwrap(),
            )
            .unwrap(),
            auth_method: esp_idf_svc::wifi::AuthMethod::WPA2Personal,
            ..Default::default()
        });

    // 設定を適用
    wifi.set_configuration(&wifi_configuration)?;

    // ...existing code...
    info!("Starting Wi-Fi connection...");
    wifi.start()?;
    info!("Wi-Fi started, waiting for connection...");
    wifi.connect()?;
    info!("Wi-Fi connected!");
    wifi.wait_netif_up()?;
    info!("Wi-Fi network interface is up.");
    Ok(())
}
