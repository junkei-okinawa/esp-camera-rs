mod config;
mod esp_now;
mod mac_address;
mod queue;
mod usb;

use anyhow::Result;
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::delay::FreeRtos;
use esp_idf_svc::hal::modem::Modem;
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::sys::{
    esp_now_init, esp_now_register_recv_cb, esp_wifi_set_ps, esp_wifi_set_storage,
    wifi_ps_type_t_WIFI_PS_NONE, wifi_storage_t_WIFI_STORAGE_RAM,
};
use esp_idf_svc::wifi::{AuthMethod, ClientConfiguration, Configuration, EspWifi};
use log::{debug, error, info, warn};
use mac_address::format_mac_address;
use usb::cdc::UsbCdc;

/// ESP-NOWの受信コールバック関数
///
/// ESP-NOWからのデータを受け取り、キューに追加します。
extern "C" fn esp_now_recv_cb(
    info: *const esp_idf_svc::sys::esp_now_recv_info_t,
    data: *const u8,
    data_len: i32,
) {
    let mut callback = |received_data| queue::data_queue::try_enqueue_from_callback(received_data);
    esp_now::receiver::process_esp_now_data(&mut callback, info, data, data_len);
}

/// ESP-NOWピアを登録する関数
///
/// カメラのMACアドレスをESP-NOWピアとして登録します。
fn register_esp_now_peers(cameras: &[config::CameraConfig]) -> Result<()> {
    info!("Registering {} cameras as ESP-NOW peers", cameras.len());

    unsafe {
        for camera in cameras {
            info!(
                "Registering camera {} with MAC: {}",
                camera.name, camera.mac_address
            );

            let mut peer_info = esp_idf_svc::sys::esp_now_peer_info_t::default();
            peer_info.channel = 0; // 現在のチャンネルを使用
            peer_info.ifidx = esp_idf_svc::sys::wifi_interface_t_WIFI_IF_STA; // STA interface
            peer_info.encrypt = false; // 暗号化なし
            peer_info.peer_addr = camera.mac_address.into_bytes();

            let add_result = esp_idf_svc::sys::esp_now_add_peer(&peer_info);
            if add_result == 0 {
                info!(
                    "ESP-NOW: Added camera {} as peer: {}",
                    camera.name, camera.mac_address
                );
            } else {
                error!(
                    "ESP-NOW: Failed to add camera {} as peer: {}",
                    camera.name, add_result
                );
            }
        }

        // ESP-NOW添付ファイル(PMK)の拡張設定
        let pmk: [u8; 16] = [
            0x50, 0x4d, 0x4b, 0x5f, 0x4b, 0x45, 0x59, 0x5f, 0x42, 0x59, 0x5f, 0x43, 0x55, 0x53,
            0x54, 0x4f,
        ];
        let pmk_result = esp_idf_svc::sys::esp_now_set_pmk(pmk.as_ptr());

        if pmk_result != 0 {
            error!("ESP-NOW: Failed to set PMK: {}", pmk_result);
        }
    }

    Ok(())
}

/// Wi-Fiを初期化する関数
///
/// ESP-NOWのためにWi-FiをSTAモードで初期化します。
///
/// # 引数
///
/// * `modem` - Wi-Fiモデムペリフェラル
///
/// # 戻り値
///
/// * `Result<EspWifi<'static>>` - 初期化されたWi-Fiインスタンス
fn initialize_wifi(modem: Modem) -> Result<EspWifi<'static>> {
    info!("Initializing Wi-Fi in STA mode for ESP-NOW...");

    let sysloop = EspSystemEventLoop::take()?;
    let nvs = EspDefaultNvsPartition::take()?; // NVSはWi-Fi初期化に必要

    let mut wifi = EspWifi::new(modem, sysloop.clone(), Some(nvs))?;

    // Wi-Fi設定をRAMに保存（NVS書き込み回避）
    unsafe {
        esp_wifi_set_storage(wifi_storage_t_WIFI_STORAGE_RAM);
    }

    // STAモードで設定（接続は不要）
    wifi.set_configuration(&Configuration::Client(ClientConfiguration {
        ssid: heapless::String::new(),     // Empty SSID
        password: heapless::String::new(), // Empty Password
        auth_method: AuthMethod::None,     // No auth needed
        ..Default::default()
    }))?;
    wifi.start()?; // Wi-FiをSTAモードで開始
    info!("Wi-Fi driver started in STA mode.");

    // Wi-Fiパワーセーブを無効化（ESP-NOWの応答性向上）
    unsafe {
        esp_wifi_set_ps(wifi_ps_type_t_WIFI_PS_NONE);
    }
    info!("Wi-Fi Power Save disabled.");

    Ok(wifi)
}

/// ESP-NOWを初期化する関数
///
/// ESP-NOWを初期化し、受信コールバックを登録します。
fn initialize_esp_now() -> Result<()> {
    info!("Initializing ESP-NOW...");

    unsafe {
        esp_now_init();
        esp_now_register_recv_cb(Some(esp_now_recv_cb));

        // ESP-NOWの最大ピア数を確認
        let mut esp_now_peer_num = esp_idf_svc::sys::esp_now_peer_num_t {
            total_num: 0,
            encrypt_num: 0,
        };

        if esp_idf_svc::sys::esp_now_get_peer_num(&mut esp_now_peer_num) == 0 {
            info!(
                "ESP-NOW: Current peer count: {}",
                esp_now_peer_num.total_num
            );
            info!("ESP-NOW: Maximum supported peers: 20"); // ESP-IDF 4.xでは20ピアをサポート
        } else {
            error!("ESP-NOW: Failed to get peer count");
        }
    }

    info!("ESP-NOW Initialized and receive callback registered.");
    Ok(())
}

/// メインデータ処理ループ
///
/// データキューからデータを取り出し、USB CDC経由で送信します。
fn process_data_loop(usb_cdc: &mut UsbCdc) -> Result<()> {
    info!("Entering main processing loop...");

    loop {
        // キューからデータを取得
        match queue::data_queue::dequeue() {
            Ok(received_data) => {
                let mac_str = format_mac_address(&received_data.mac);
                debug!(
                    "Main Loop: Processing framed data from {}. Sending via USB CDC ({} bytes).",
                    mac_str,
                    received_data.data.len()
                );

                // USB経由でデータを送信
                match usb_cdc.send_frame(&received_data.data, &mac_str) {
                    Ok(bytes_sent) => {
                        debug!(
                            "Main Loop: Successfully sent {} bytes via USB CDC for {}.",
                            bytes_sent, mac_str
                        );
                    }
                    Err(e) => {
                        warn!(
                            "Main Loop: Failed to send data via USB CDC for {}: {}",
                            mac_str, e
                        );
                    }
                }
            }
            Err(queue::QueueError::Empty) => {
                // キューが空の場合は少し待機
                FreeRtos::delay_ms(10);
            }
            Err(e) => {
                error!("Main Loop: Error dequeuing data: {}", e);
                FreeRtos::delay_ms(100);
            }
        }
    }
}

fn main() -> Result<()> {
    // ESP-IDFシステムの初期化
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();
    log::set_max_level(log::LevelFilter::Info);

    info!("Starting ESP-NOW USB CDC Receiver...");

    // キューの初期化
    queue::data_queue::initialize_data_queue();

    // 設定からカメラ情報を読み込み
    let cameras = config::load_camera_configs();

    // ペリフェラルの取得
    let peripherals = Peripherals::take().unwrap();

    // Wi-Fi初期化（モデムを渡す）
    let _wifi = initialize_wifi(peripherals.modem)?;

    // ESP-NOW初期化
    initialize_esp_now()?;

    // カメラをピアとして登録
    register_esp_now_peers(&cameras)?;

    // USB CDC初期化（Wi-Fi初期化で取得したペリフェラルを使用）
    let mut usb_cdc = UsbCdc::new(
        peripherals.usb_serial,
        peripherals.pins.gpio18, // XIAO ESP32C3のUSB D-ピン
        peripherals.pins.gpio19, // XIAO ESP32C3のUSB D+ピン
    )?;

    // メインデータ処理ループ
    process_data_loop(&mut usb_cdc)
}

#[cfg(test)]
mod tests {
    // インテグレーションテストは必要に応じて追加
}
