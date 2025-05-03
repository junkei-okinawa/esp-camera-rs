use std::time::{Duration, Instant};

mod camera;
mod config;
mod esp_now;
mod led;
mod mac_address;
mod sleep;

use camera::CameraController;
use config::AppConfig;
use esp_now::{EspNowSender, ImageFrame};
use led::StatusLed;
use log::{error, info, warn};
use sleep::DeepSleep;

use esp_idf_svc::{
    eventloop::EspSystemEventLoop,
    hal::{
        adc::{
            attenuation::DB_12,
            oneshot::config::{AdcChannelConfig, Calibration},
            oneshot::{AdcChannelDriver, AdcDriver},
        },
        peripherals::Peripherals,
    },
    nvs::EspDefaultNvsPartition,
    wifi::{BlockingWifi, EspWifi},
};

// --- 電圧測定用の定数 ---
const MIN_MV: f32 = 128.0; // UnitCam GPIO0 の実測値に合わせて調整
const MAX_MV: f32 = 3130.0; // UnitCam GPIO0 の実測値に合わせて調整
const RANGE_MV: f32 = MAX_MV - MIN_MV;
// --- ここまで 定数 ---
/// アプリケーションのメインエントリーポイント
fn main() -> anyhow::Result<()> {
    // ESP-IDFの各種初期化
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    // 設定をロードする
    let config = AppConfig::load()?;
    info!("受信機MACアドレス: {}", config.receiver_mac);

    // ペリフェラルを初期化
    info!("ペリフェラルを初期化しています");
    let peripherals = Peripherals::take().unwrap();
    let sysloop = EspSystemEventLoop::take()?;
    let nvs = EspDefaultNvsPartition::take()?;

    // --- ADC2 を初期化 ---
    info!("ADC2を初期化しています (GPIO0)");
    let adc2 = AdcDriver::new(peripherals.adc2)?;
    let adc_config = AdcChannelConfig {
        attenuation: DB_12,
        calibration: Calibration::Line,
        ..Default::default()
    };
    let mut adc2_ch1 = AdcChannelDriver::new(&adc2, peripherals.pins.gpio0, &adc_config)?;
    // --- ここまで ADC2 初期化 ---

    // --- 電圧測定 & パーセンテージ計算 (WiFi開始前) ---
    info!("電圧を測定しパーセンテージを計算します (WiFi開始前)...");
    let mut measured_voltage_percent: u8 = 0; // 送信失敗時用のデフォルト値 (0%)
    match adc2_ch1.read() {
        Ok(voltage_mv_u16) => {
            let voltage_mv = voltage_mv_u16 as f32; // f32 に変換して計算
            info!("電圧測定成功: {:.0} mV", voltage_mv);
            // パーセンテージ計算 (0-100 の範囲にクランプし、u8 に丸める)
            let percentage = if RANGE_MV <= 0.0 {
                0.0
            } else {
                ((voltage_mv - MIN_MV) / RANGE_MV * 100.0)
                    .max(0.0)
                    .min(100.0)
            };
            measured_voltage_percent = percentage.round() as u8; // u8 に丸める
            info!("計算されたパーセンテージ: {} %", measured_voltage_percent);
        }
        Err(e) => {
            error!("ADC読み取りエラー: {:?}. 電圧は0%として扱います。", e);
            // エラーでも続行するが、パーセンテージは0として扱う
        }
    }
    // ADCドライバはこの後不要になるので、ここでドロップしても良い
    // drop(adc2_ch1);
    // drop(adc2);
    // --- 電圧測定ここまで ---

    // WiFiを初期化（ESP-NOWに必要）
    info!("ESP-NOW用のWiFiペリフェラルを初期化しています - STAモード");
    let mut wifi = BlockingWifi::wrap(
        EspWifi::new(peripherals.modem, sysloop.clone(), Some(nvs))?,
        sysloop,
    )?;

    // Wi-Fi設定をRAMに保存（NVS書き込み回避）
    unsafe {
        esp_idf_svc::sys::esp_wifi_set_storage(esp_idf_svc::sys::wifi_storage_t_WIFI_STORAGE_RAM);
    }

    // STAモードで設定（接続は不要）
    wifi.set_configuration(&esp_idf_svc::wifi::Configuration::Client(
        esp_idf_svc::wifi::ClientConfiguration {
            ssid: "".try_into().unwrap(),                     // Empty SSID
            password: "".try_into().unwrap(),                 // Empty Password
            auth_method: esp_idf_svc::wifi::AuthMethod::None, // No auth needed
            ..Default::default()
        },
    ))?;

    // WiFiを起動 (ESP-NOWに必要。この時点でADC2は使えなくなる)
    wifi.start()?;
    info!("WiFiペリフェラルがSTAモードで起動しました");

    // Wi-Fiパワーセーブを無効化（ESP-NOWの応答性向上）
    unsafe {
        esp_idf_svc::sys::esp_wifi_set_ps(esp_idf_svc::sys::wifi_ps_type_t_WIFI_PS_NONE);
    }
    info!("Wi-Fi Power Save を無効化しました");

    // LEDを初期化 - 新しいインターフェースでは個別のピンを取得
    let mut led = StatusLed::new(peripherals.pins.gpio4)?;
    led.turn_off()?;

    // カメラを初期化 - 新しいインターフェースではすべてのピンを個別に渡す必要があります
    let camera = CameraController::new(
        peripherals.pins.gpio27,            // clock
        peripherals.pins.gpio32,            // d0
        peripherals.pins.gpio35,            // d1
        peripherals.pins.gpio34,            // d2
        peripherals.pins.gpio5,             // d3
        peripherals.pins.gpio39,            // d4
        peripherals.pins.gpio18,            // d5
        peripherals.pins.gpio36,            // d6
        peripherals.pins.gpio19,            // d7
        peripherals.pins.gpio22,            // vsync
        peripherals.pins.gpio26,            // href
        peripherals.pins.gpio21,            // pclk
        peripherals.pins.gpio25,            // sda
        peripherals.pins.gpio23,            // scl
        camera::M5UnitCamConfig::default(), // デフォルト設定
    )?;

    // ESP-NOW送信機を初期化
    let esp_now = EspNowSender::new()?;
    esp_now.add_peer(&config.receiver_mac)?;

    // 定期送信のためのパラメータ設定
    let target_interval = Duration::from_secs(config.sleep_duration_seconds); // 設定ファイルから読み込んだスリープ時間
    let min_sleep_duration = Duration::from_secs(1); // 最小スリープ時間: 1秒

    info!(
        "設定されたディープスリープ時間: {}秒",
        config.sleep_duration_seconds
    );

    // --- メイン処理 (Deep Sleep 前の1サイクル) ---
    let loop_start_time = Instant::now(); // 処理開始時間を記録

    // 画像を撮影
    info!("写真を撮影します");
    led.indicate_capture()?;

    match camera.capture_image() {
        Ok(framebuffer) => {
            info!(
                "撮影完了: {width}x{height} {size} バイト",
                width = framebuffer.width(),
                height = framebuffer.height(),
                size = framebuffer.data().len(),
            );
            led.blink_success()?;

            // 画像データ取得
            let data = framebuffer.data();

            // SHA256ハッシュを計算
            let hash_hex = match ImageFrame::calculate_hash(data) {
                Ok(hash) => {
                    info!("画像SHA256: {}", hash);
                    hash
                }
                Err(e) => {
                    error!("ハッシュ計算エラー: {:?}. スリープします。", e);
                    led.blink_error()?;
                    let _ = DeepSleep::sleep_with_timing(
                        loop_start_time,
                        target_interval,
                        min_sleep_duration,
                    );
                    return Ok(());
                }
            };

            // ハッシュメッセージを準備 (保存しておいた電圧パーセンテージを使用)
            let hash_payload =
                ImageFrame::prepare_hash_message(&hash_hex, measured_voltage_percent);

            // ハッシュを送信
            info!("画像ハッシュ (と電圧情報) を送信します");
            if let Err(e) = esp_now.send(&config.receiver_mac, &hash_payload, 1000) {
                error!("ハッシュ送信エラー: {:?}. スリープします。", e);
                led.blink_error()?;
                let _ = DeepSleep::sleep_with_timing(
                    loop_start_time,
                    target_interval,
                    min_sleep_duration,
                );
                return Ok(());
            }

            // 画像チャンクを送信
            info!("画像チャンクを送信します...");
            match esp_now.send_image_chunks(&config.receiver_mac, data, 250, 5) {
                Ok(_) => {
                    info!("画像送信完了");
                    led.indicate_sending()?;
                }
                Err(e) => {
                    error!("画像送信エラー: {:?}. スリープします。", e);
                    led.blink_error()?;
                    let _ = DeepSleep::sleep_with_timing(
                        loop_start_time,
                        target_interval,
                        min_sleep_duration,
                    );
                    return Ok(());
                }
            }
        }
        Err(e) => {
            error!("画像撮影エラー: {:?}. スリープします。", e);
            led.blink_error()?;
            let _ =
                DeepSleep::sleep_with_timing(loop_start_time, target_interval, min_sleep_duration);
            return Ok(());
        }
    }

    // ディープスリープ処理
    info!("ディープスリープに入ります");
    let _ = DeepSleep::sleep_with_timing(loop_start_time, target_interval, min_sleep_duration);

    // 通常はここまで到達しない
    Ok(())
}
