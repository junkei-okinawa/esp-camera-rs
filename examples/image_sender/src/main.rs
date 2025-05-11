use esp_idf_svc::{
    eventloop::EspSystemEventLoop,
    hal::{
        adc::{
            attenuation::DB_12,
            oneshot::config::{AdcChannelConfig, Calibration},
            oneshot::{AdcChannelDriver, AdcDriver},
        },
        delay::FreeRtos,
        peripherals::Peripherals,
    },
    nvs::EspDefaultNvsPartition,
    wifi::{BlockingWifi, EspWifi},
};
use std::sync::Arc; // Added Arc
use std::time::{Duration, Instant}; // Removed SystemTime, UNIX_EPOCH

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
use log::{error, info, warn};
use sleep::{DeepSleep, EspIdfDeepSleep}; // EspIdfDeepSleep を追加

const DUMMY_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000"; // 64 zeros for SHA256 dummy

// --- 電圧測定用の定数 ---
const MIN_MV: f32 = 128.0; // UnitCam GPIO0 の実測値に合わせて調整
const MAX_MV: f32 = 3130.0; // UnitCam GPIO0 の実測値に合わせて調整
const RANGE_MV: f32 = MAX_MV - MIN_MV;
const LOW_VOLTAGE_THRESHOLD_PERCENT: u8 = 8; // このパーセンテージ未満で低電圧モード
                                             // --- ここまで 定数 ---

// --- 画像送信タスク ---
fn transmit_data_task(
    image_data_option: Option<Vec<u8>>,
    config: &AppConfig,
    measured_voltage_percent: u8,
    wifi: &mut BlockingWifi<EspWifi<'static>>, // modem, sysloop, nvs を BlockingWifi に置き換え
    led: &mut StatusLed,
) -> anyhow::Result<()> {
    unsafe {
        esp_idf_svc::sys::esp_wifi_set_ps(esp_idf_svc::sys::wifi_ps_type_t_WIFI_PS_NONE);
    }
    info!("Wi-Fi Power Save を無効化しました (ESP-NOW用)");

    let esp_now_sender = EspNowSender::new()?; // EspNowSender::new() は内部でesp_now_init()を呼ぶ
    esp_now_sender.add_peer(&config.receiver_mac)?;
    info!("ESP-NOW送信機を初期化し、ピアを追加しました");

    match image_data_option {
        Some(image_data) => {
            // image_data は Vec<u8>
            let hash_result = ImageFrame::calculate_hash(&image_data); // image_data の参照を渡す

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
                    if let Err(e) = esp_now_sender.send(&config.receiver_mac, &hash_payload, 1000) {
                        error!("ハッシュ送信エラー: {:?}", e);
                        led.blink_error()?;
                        return Err(e.into());
                    }

                    info!("画像チャンクを送信します...");
                    // image_data はここで使用終了なので Vec<u8> を直接渡す
                    match esp_now_sender.send_image_chunks(&config.receiver_mac, image_data, 250, 5)
                    {
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
            if let Err(e) = esp_now_sender.send(&config.receiver_mac, &hash_payload, 1000) {
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

    let loop_start_time = Instant::now(); // 処理開始時間を記録
    let min_sleep_duration = Duration::from_secs(1); // 最小スリープ時間: 1秒

    // 設定をロード
    let app_config = match AppConfig::load() {
        Ok(cfg) => Arc::new(cfg), // Wrap in Arc
        Err(e) => {
            error!("設定ファイルの読み込みに失敗しました: {}", e);
            panic!("設定ファイルの読み込みエラー: {}", e);
        }
    };
    // info!("設定をロードしました: {:?}", app_config);

    // ペリフェラルを初期化
    info!("ペリフェラルを初期化しています");
    let peripherals_all = Peripherals::take().unwrap();
    let modem_peripheral = peripherals_all.modem;

    let sysloop = EspSystemEventLoop::take()?;
    let nvs_partition = EspDefaultNvsPartition::take()?;

    // LEDを初期化
    let mut led = StatusLed::new(peripherals_all.pins.gpio4)?;
    led.turn_off()?; // LEDを消灯

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
    let mut measured_voltage_percent: u8 = u8::MAX; // 送信失敗時用のデフォルト値 (255%)
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
    // ADCドライバはこの後不要になるので、ここでドロップ
    drop(adc2_ch1);
    drop(adc2);
    // --- 電圧測定ここまで ---

    let mut deep_sleep_controller = DeepSleep::new(app_config.clone(), EspIdfDeepSleep); // EspIdfDeepSleep を渡すように変更

    // --- measured_voltage_percent が 0% の場合はlongスリープ ---
    if measured_voltage_percent == 0 {
        info!(
            "電圧が0%のため、{}秒間の長時間ディープスリープに入ります。",
            app_config.sleep_duration_seconds_for_long
        );
        // LEDを消灯させておく
        led.turn_off()?;
        // 長時間ディープスリープに入る
        match deep_sleep_controller
            .sleep_for_duration_long(app_config.sleep_duration_seconds_for_long)
        {
            Ok(_) => { /* 通常ここには到達しない */ }
            Err(e) => {
                error!("長時間ディープスリープの開始に失敗: {:?}", e);
                // エラーが発生した場合でも、フォールバックとして短時間のスリープを試みるか、
                // またはパニックするなどのエラー処理が必要かもしれないが、
                // DeepSleep::sleep_for_duration_long の現在の実装ではエラーから復帰しない想定
            }
        }
        // ディープスリープから復帰することはないため、以降のコードは実行されない
    }
    // --- ここまで measured_voltage_percent が 0% の場合の処理 ---

    // WiFiと時刻同期の準備
    info!("WiFiを初期化しています (時刻同期用)");
    let mut wifi = BlockingWifi::wrap(
        EspWifi::new(
            modem_peripheral,
            sysloop.clone(),
            Some(nvs_partition.clone()),
        )?,
        sysloop.clone(),
    )?;

    // 条件付き時刻同期処理
    match deep_sleep_controller.ensure_time_sync_if_needed(
        &mut wifi,
        &app_config.wifi_ssid,
        &app_config.wifi_password,
    ) {
        Ok(_) => info!("時刻同期（条件付き）が正常に完了またはスキップされました。"),
        Err(e) => {
            error!("時刻同期（条件付き）に失敗しました: {:?}", e);
            // エラーが発生しても処理を続行するが、LEDでエラーを示す
            led.blink_error()?;
            // ここでリターンするか、エラーを無視して続行するかは要件による
            // 今回は続行し、スリープに入る
        }
    }

    // --- 画像取得タスク (低電圧時はスキップ) ---
    let mut image_data_option: Option<Vec<u8>> = None; // Option<Vec<u8>> に変更

    if measured_voltage_percent >= LOW_VOLTAGE_THRESHOLD_PERCENT {
        info!(
            "電圧 {}% (>= {}%) は十分なため、カメラを初期化し画像をキャプチャします。",
            measured_voltage_percent, LOW_VOLTAGE_THRESHOLD_PERCENT
        );

        let camera_config = camera::M5UnitCamConfig {
            frame_size: M5UnitCamConfig::from_string(&app_config.frame_size),
        };

        // カメラを初期化。失敗した場合は `?` により main 関数からエラーが返る。
        let camera = CameraController::new(
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
            camera_config,
        )?;

        let current_aec_value = camera.get_current_aec_value();
        let _ =
            camera.configure_exposure(app_config.auto_exposure_enabled, Some(current_aec_value)); // 自動露出設定を適用
        if let Some(warmup_frames) = app_config.camera_warmup_frames {
            info!("カメラウォームアップフレーム数: {}", warmup_frames);
            for _ in 0..warmup_frames {
                match camera.capture_image() {
                    Ok(_) => {
                        info!("カメラウォームアップフレームキャプチャ成功");
                    }
                    Err(e) => {
                        error!("カメラウォームアップフレームキャプチャ失敗: {:?}", e);
                        led.blink_error()?;
                    }
                }
                FreeRtos::delay_ms(1000);
            }
        }

        // 3回目の画像をキャプチャし、データをコピーして image_data_for_task に保存
        match camera.capture_image() {
            Ok(fb) => {
                info!("画像キャプチャ成功: {} バイト", fb.data().len());
                image_data_option = Some(fb.data().to_vec()); // 画像データを Vec<u8> としてコピー
            }
            Err(e) => {
                error!("画像キャプチャ失敗 (最終): {:?}", e);
                led.blink_error()?;
                // image_data_for_task は None のまま
            }
        };
    } else {
        info!(
            "電圧が低い ({}% < {}%) ため、カメラ処理をスキップします。",
            measured_voltage_percent, LOW_VOLTAGE_THRESHOLD_PERCENT
        );
        led.blink_error()?;
        // image_data_for_task は None のまま
    };
    // camera インスタンスはここでドロップされる

    // ESP-NOW用にWiFiを再設定・起動
    info!("ESP-NOW用にWiFiをSTAモードで再起動します。");
    // ESP-NOWは特定のSSIDへの接続を必要としないため、ダミー設定で起動
    // ただし、esp_wifi_start() は必要
    // 既存の BlockingWifi インスタンスを再利用
    match wifi.stop() {
        Ok(_) => info!("時刻同期用のWiFiセッションを停止しました。"),
        Err(e) if e.code() == esp_idf_sys::ESP_ERR_WIFI_NOT_INIT => {
            info!("WiFiはまだ初期化されていません (ESP-NOW用)。");
        }
        Err(e) => {
            warn!(
                "時刻同期用WiFiセッションの停止に失敗: {:?}。処理を続行します。",
                e
            );
        }
    }
    // ESP-NOWのためだけにSTAモードで起動する。特定のAPへの接続はしない。
    // ESP-NOWのesp_now_init()が内部でWiFiがアクティブであることを期待するため。
    wifi.set_configuration(&esp_idf_svc::wifi::Configuration::Client(
        esp_idf_svc::wifi::ClientConfiguration {
            ssid: "".try_into().unwrap(),     // ESP-NOWではSSIDは通常関係ない
            password: "".try_into().unwrap(), // パスワードも同様
            auth_method: esp_idf_svc::wifi::AuthMethod::None,
            ..Default::default()
        },
    ))?;
    wifi.start()?;
    info!("WiFiがESP-NOW用にSTAモードで起動しました。");

    // --- データ送信タスク ---
    info!("データ送信タスクを開始します");
    if let Err(e) = transmit_data_task(
        image_data_option, // Option<Vec<u8>> を渡す
        &app_config,
        measured_voltage_percent,
        &mut wifi,
        &mut led,
    ) {
        error!("データ送信タスクでエラーが発生しました: {:?}", e);
        // エラーが発生してもスリープ処理は行う
    }

    // --- スリープ処理 ---
    let elapsed_time = loop_start_time.elapsed();
    info!("メインループ処理時間: {:?}", elapsed_time);

    // min_sleep_duration はループの最初の方で定義済み: Duration::from_secs(1)

    led.turn_off()?; // スリープ前にLEDを消灯

    // DeepSleepモジュールのスリープ関数を呼び出す
    // スリープ時間の計算は DeepSleep::sleep 内で行われる
    let _ = deep_sleep_controller.sleep(
        elapsed_time,       // StdDuration (std::time::Duration)
        min_sleep_duration, // StdDuration (std::time::Duration)
    );

    // スリープから復帰することはないはずなので、以下のコードは実行されない
    Ok(())
}
