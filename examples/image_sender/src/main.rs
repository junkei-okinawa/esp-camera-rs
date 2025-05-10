use std::time::{Duration, Instant};

mod camera;
mod config;
mod esp_now;
mod led;
mod mac_address;
mod sleep;

use camera::{CameraController, M5UnitCamConfig};
use config::AppConfig;
use esp_now::{EspNowSender, ImageFrame};
use led::StatusLed;
use log::{error, info};
use sleep::DeepSleep;

// 追加: FrameBuffer と Modem
use esp_camera_rs::FrameBuffer;
use esp_idf_svc::hal::modem::Modem;

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

const DUMMY_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000"; // 64 zeros for SHA256 dummy

// --- 電圧測定用の定数 ---
const MIN_MV: f32 = 128.0; // UnitCam GPIO0 の実測値に合わせて調整
const MAX_MV: f32 = 3130.0; // UnitCam GPIO0 の実測値に合わせて調整
const RANGE_MV: f32 = MAX_MV - MIN_MV;
const LOW_VOLTAGE_THRESHOLD_PERCENT: u8 = 8; // このパーセンテージ未満で低電圧モード
                                             // --- ここまで 定数 ---

// --- 画像送信タスク ---
fn transmit_data_task(
    framebuffer_option: &Option<FrameBuffer<'_>>,
    config: &AppConfig,
    measured_voltage_percent: u8,
    modem: Modem, // peripherals.modem を受け取る
    sysloop: EspSystemEventLoop,
    nvs: EspDefaultNvsPartition,
    led: &mut StatusLed,
) -> anyhow::Result<()> {
    info!("ESP-NOW用のWiFiペリフェラルを初期化しています - STAモード");
    let mut wifi = BlockingWifi::wrap(
        EspWifi::new(modem, sysloop.clone(), Some(nvs))?, // nvs.clone() を nvs に変更 (所有権移動)
        sysloop,
    )?;

    unsafe {
        esp_idf_svc::sys::esp_wifi_set_storage(esp_idf_svc::sys::wifi_storage_t_WIFI_STORAGE_RAM);
    }

    wifi.set_configuration(&esp_idf_svc::wifi::Configuration::Client(
        esp_idf_svc::wifi::ClientConfiguration {
            ssid: "".try_into().unwrap(),
            password: "".try_into().unwrap(),
            auth_method: esp_idf_svc::wifi::AuthMethod::None,
            ..Default::default()
        },
    ))?;
    wifi.start()?;
    info!("WiFiペリフェラルがSTAモードで起動しました");

    unsafe {
        esp_idf_svc::sys::esp_wifi_set_ps(esp_idf_svc::sys::wifi_ps_type_t_WIFI_PS_NONE);
    }
    info!("Wi-Fi Power Save を無効化しました");

    let esp_now = EspNowSender::new()?;
    esp_now.add_peer(&config.receiver_mac)?;
    info!("ESP-NOW送信機を初期化し、ピアを追加しました");

    match framebuffer_option {
        Some(framebuffer) => {
            let data = framebuffer.data();
            let hash_result = ImageFrame::calculate_hash(data);

            match hash_result {
                Err(e) => {
                    error!("ハッシュ計算エラー: {:?}", e);
                    led.blink_error()?;
                    return Err(e.into());
                }
                Ok(hash) => {
                    info!("画像SHA256: {}", hash);
                    let hash_payload =
                        ImageFrame::prepare_hash_message(&hash, measured_voltage_percent);

                    info!("画像ハッシュ (と電圧情報) を送信します");
                    if let Err(e) = esp_now.send(&config.receiver_mac, &hash_payload, 1000) {
                        error!("ハッシュ送信エラー: {:?}", e);
                        led.blink_error()?;
                        return Err(e.into());
                    }

                    info!("画像チャンクを送信します...");
                    match esp_now.send_image_chunks(&config.receiver_mac, data, 250, 5) {
                        Ok(_) => {
                            info!("画像送信完了");
                            led.indicate_sending()?;
                        }
                        Err(e) => {
                            error!("画像送信エラー: {:?}", e);
                            led.blink_error()?;
                            return Err(e.into());
                        }
                    }
                }
            }
        }
        None => {
            info!("送信する画像がありません。ダミーデータを送信します。");
            let hash_payload =
                ImageFrame::prepare_hash_message(DUMMY_HASH, measured_voltage_percent);

            info!("ダミーハッシュ (と電圧情報) を送信します");
            if let Err(e) = esp_now.send(&config.receiver_mac, &hash_payload, 1000) {
                error!("ダミーハッシュ送信エラー: {:?}", e);
                return Err(e.into());
            } else {
                info!("ダミーハッシュ送信成功");
            }
            info!("画像チャンクの送信はスキップします。");
        }
    }
    Ok(())
}

/// アプリケーションのメインエントリーポイント
fn main() -> anyhow::Result<()> {
    // ESP-IDFの各種初期化
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    // 設定をロードする
    let config = AppConfig::load()?;
    info!("受信機MACアドレス: {}", config.receiver_mac);
    info!("スリープ時間: {}秒", config.sleep_duration_seconds);
    info!(
        "スリープ時間 (長時間用): {}秒",
        config.sleep_duration_seconds_for_long
    );
    info!("フレームサイズ: {}", config.frame_size);

    // ペリフェラルを初期化
    info!("ペリフェラルを初期化しています");
    let peripherals_all = Peripherals::take().unwrap();
    let modem_peripheral = peripherals_all.modem;

    let sysloop = EspSystemEventLoop::take()?;
    let nvs_partition = EspDefaultNvsPartition::take()?;

    // --- ADC2 を初期化 ---
    info!("ADC2を初期化しています (GPIO0)");
    let adc2 = AdcDriver::new(peripherals_all.adc2)?;
    let adc_config = AdcChannelConfig {
        attenuation: DB_12,
        calibration: Calibration::Line,
        ..Default::default()
    };
    let mut adc2_ch1 = AdcChannelDriver::new(&adc2, peripherals_all.pins.gpio0, &adc_config)?;
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
    drop(adc2_ch1);
    drop(adc2);
    // --- 電圧測定ここまで ---

    if measured_voltage_percent == 0 {
        // ソーラーパネルの生産電圧が0Vの場合、後続処理を行わずにDeepSleepに入る
        info!("電圧が0Vのため、後続処理をスキップして長時間のディープスリープに入ります。");
        DeepSleep::sleep_with_timing(
            // `?` を追加
            Instant::now(),
            Duration::from_secs(config.sleep_duration_seconds_for_long),
            Duration::from_secs(1),
        )?;
        return Ok(()); // 早期リターン
    }

    // LEDを初期化 - 新しいインターフェースでは個別のピンを取得
    let mut led = StatusLed::new(peripherals_all.pins.gpio4)?; // peripherals_all を使用
    led.turn_off()?;

    // カメラ構成を作成
    let camera_config = camera::M5UnitCamConfig {
        frame_size: M5UnitCamConfig::from_string(&config.frame_size),
    };

    // 定期送信のためのパラメータ設定
    let target_interval = Duration::from_secs(config.sleep_duration_seconds); // 設定ファイルから読み込んだスリープ時間
    let min_sleep_duration = Duration::from_secs(1); // 最小スリープ時間: 1秒

    info!(
        "設定されたディープスリープ時間: {}秒",
        config.sleep_duration_seconds
    );

    // --- メイン処理 (Deep Sleep 前の1サイクル) ---
    let loop_start_time = Instant::now(); // 処理開始時間を記録

    #[allow(unused_assignments)]
    // camera_controller_holder は条件によって代入されないことがあるため許可
    let mut camera_controller_holder: Option<CameraController> = None; // カメラコントローラーのインスタンスを保持

    let framebuffer_option = if measured_voltage_percent >= LOW_VOLTAGE_THRESHOLD_PERCENT {
        info!(
            "電圧 {}% (>= {}%) は十分なため、カメラを初期化し画像をキャプチャします。",
            measured_voltage_percent, LOW_VOLTAGE_THRESHOLD_PERCENT
        );

        // カメラを初期化。失敗した場合は `?` により main 関数からエラーが返る。
        let initialized_camera = CameraController::new(
            peripherals_all.pins.gpio27, // clock
            peripherals_all.pins.gpio32, // d0
            peripherals_all.pins.gpio35, // d1
            peripherals_all.pins.gpio34, // d2
            peripherals_all.pins.gpio5,  // d3
            peripherals_all.pins.gpio39, // d4
            peripherals_all.pins.gpio18, // d5
            peripherals_all.pins.gpio36, // d6
            peripherals_all.pins.gpio19, // d7
            peripherals_all.pins.gpio22, // vsync
            peripherals_all.pins.gpio26, // href
            peripherals_all.pins.gpio21, // pclk
            peripherals_all.pins.gpio25, // sda
            peripherals_all.pins.gpio23, // scl
            camera_config.clone(),       // 設定をクローンして渡す
        )?;

        camera_controller_holder = Some(initialized_camera); // 初期化されたカメラをホルダーに格納

        // ホルダー内のカメラコントローラーの参照を使用する
        if let Some(camera_ref) = &camera_controller_holder {
            let current_aec_value = camera_ref.get_current_aec_value();
            let _ = camera_ref
                .configure_exposure(config.auto_exposure_enabled, Some(current_aec_value)); // 自動露出設定を適用
            if let Some(warmup_frames) = config.camera_warmup_frames {
                info!("カメラウォームアップフレーム数: {}", warmup_frames);
                for _ in 0..warmup_frames {
                    match camera_ref.capture_image() {
                        Ok(_) => {
                            info!("カメラウォームアップフレームキャプチャ成功");
                        }
                        Err(e) => {
                            error!("カメラウォームアップフレームキャプチャ失敗: {:?}", e);
                            led.blink_error()?;
                        }
                    }
                }
            }

            // 3回目の画像を framebuffer_option に保存
            match camera_ref.capture_image() {
                Ok(fb) => {
                    info!("画像キャプチャ成功: {} バイト", fb.data().len());
                    Some(fb) // この FrameBuffer は camera_ref (camera_controller_holder 内) から借用
                }
                Err(e) => {
                    error!("画像キャプチャ失敗 (最終): {:?}", e);
                    led.blink_error()?;
                    None
                }
            }
        } else {
            // camera_controller_holder に Some が代入された直後のため、通常このブロックには到達しない
            error!("カメラコントローラーホルダーが予期せずNoneです。");
            None
        }
    } else {
        info!(
            "電圧が低い ({}% < {}%) ため、カメラ処理をスキップします。",
            measured_voltage_percent, LOW_VOLTAGE_THRESHOLD_PERCENT
        );
        led.blink_error()?; // 低電圧状態を示す
        None // 画像データは None
    };

    // データ送信タスクを実行
    if let Err(e) = transmit_data_task(
        &framebuffer_option, // framebuffer_option の参照を渡す
        &config,
        measured_voltage_percent,
        modem_peripheral, // modem の所有権を渡す
        sysloop.clone(),  // sysloop をクローンして渡す
        nvs_partition,    // nvs_partition の所有権を渡す
        &mut led,
    ) {
        error!("データ送信タスクでエラーが発生: {:?}", e);
        // エラーが発生した場合でも、最終的にスリープ処理は main の最後で行われる
    }

    // camera_controller_holder と framebuffer_option はここでスコープを抜けて drop される。

    // --- ディープスリープ ---
    info!("処理完了。ディープスリープに入ります。");
    DeepSleep::sleep_with_timing(loop_start_time, target_interval, min_sleep_duration)?;

    // main 関数の最後 (通常は到達しないが、コンパイラのために必要)
    Ok(())
}
