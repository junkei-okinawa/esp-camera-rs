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
use log::{error, info};
use mac_address::MacAddress;
use sleep::DeepSleep;

use esp_idf_svc::{
    eventloop::EspSystemEventLoop,
    hal::peripherals::Peripherals,
    nvs::EspDefaultNvsPartition,
    wifi::{BlockingWifi, EspWifi},
};

/// アプリケーションのメインエントリーポイント
fn main() -> anyhow::Result<()> {
    // ESP-IDFの各種初期化
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    // 設定をロードする
    let config = AppConfig::load()?;
    info!("受信機MACアドレス: {}", config.receiver_mac);
    info!(
        "設定値としてのMACアドレス: {:?}",
        config.receiver_mac.config_rs_mac_address()
    );

    // ペリフェラルを初期化
    info!("ペリフェラルを初期化しています");
    let peripherals = Peripherals::take().unwrap();
    let sysloop = EspSystemEventLoop::take()?;
    let nvs = EspDefaultNvsPartition::take()?;

    // WiFiを初期化（ESP-NOWに必要）
    info!("ESP-NOW用のWiFiペリフェラルを初期化しています");
    let mut wifi = BlockingWifi::wrap(
        EspWifi::new(peripherals.modem, sysloop.clone(), Some(nvs))?,
        sysloop,
    )?;
    wifi.start()?;
    info!("WiFiペリフェラルが起動しました");

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
    let target_interval = Duration::from_secs(60); // 目標間隔: 60秒
    let min_sleep_duration = Duration::from_secs(1); // 最小スリープ時間: 1秒

    // メインループ
    loop {
        let loop_start_time = Instant::now(); // ループ開始時間を記録

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

                // 撮影成功を示すLEDパターン
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
                        error!("ハッシュ計算エラー: {:?}", e);
                        continue;
                    }
                };

                // ハッシュメッセージを準備
                let hash_payload = ImageFrame::prepare_hash_message(&hash_hex);

                // ハッシュを送信
                info!("画像ハッシュを送信します");
                if let Err(e) = esp_now.send(&config.receiver_mac, &hash_payload, 1000) {
                    error!("ハッシュ送信エラー: {:?}", e);
                    led.blink_error()?;
                    continue; // ハッシュ送信に失敗した場合は画像チャンクは送信しない
                }

                // 画像チャンクを送信
                info!("画像チャンクを送信します...");
                match esp_now.send_image_chunks(&config.receiver_mac, data, 250, 5) {
                    Ok(_) => {
                        info!("画像送信完了");
                        led.indicate_sending()?;
                    }
                    Err(e) => {
                        error!("画像送信エラー: {:?}", e);
                        led.blink_error()?;
                    }
                }
            }
            Err(e) => {
                error!("画像撮影エラー: {:?}", e);
                led.blink_error()?;
            }
        }

        // ディープスリープ処理
        info!("ディープスリープに入ります");
        let _ = DeepSleep::sleep_with_timing(loop_start_time, target_interval, min_sleep_duration);

        // ディープスリープから復帰した場合（通常は実行されない）
        info!("ディープスリープから復帰しました");
    }
}
