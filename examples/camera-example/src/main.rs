use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::{delay::FreeRtos, gpio::PinDriver, peripherals::Peripherals};
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::wifi::{EspWifi, BlockingWifi}; // BlockingWifiを追加
use esp_idf_sys::{
    esp_deep_sleep, esp_now_add_peer, esp_now_init, esp_now_peer_info_t,
    esp_now_register_send_cb, esp_now_send, esp_now_send_status_t,
    esp_now_send_status_t_ESP_NOW_SEND_SUCCESS,
};
use log::{error, info};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

const ESP_NOW_TARGET_MAC: [u8; 6] = [0x00, 0x00, 0x00, 0x00, 0x00, 0x00]; // 受信側のMACアドレスに更新

// 送信完了フラグ
static SEND_COMPLETE: AtomicBool = AtomicBool::new(true); // 初期値は送信可能
// 送信失敗フラグ（一度でも失敗したらtrue）
static SEND_FAILED: AtomicBool = AtomicBool::new(false);

extern "C" fn esp_now_send_cb(_mac_addr: *const u8, status: esp_now_send_status_t) {
    if status == esp_now_send_status_t_ESP_NOW_SEND_SUCCESS {
        info!("ESP-NOW: Send success");
    } else {
        error!("ESP-NOW: Send failed");
        SEND_FAILED.store(true, Ordering::SeqCst); // 失敗フラグを立てる
    }
    SEND_COMPLETE.store(true, Ordering::SeqCst); // 送信完了（成功・失敗問わず）
}

fn main() -> anyhow::Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    info!("initializing peripherals");
    let peripherals = Peripherals::take().unwrap();
    let sysloop = EspSystemEventLoop::take()?;
    let nvs = EspDefaultNvsPartition::take()?;

    info!("initializing WiFi peripheral for ESP-NOW");
    // Wi-Fiペリフェラルを初期化するだけで、接続はしない
    let mut wifi = BlockingWifi::wrap( // BlockingWifiでラップ
        EspWifi::new(peripherals.modem, sysloop.clone(), Some(nvs))?,
        sysloop,
    )?;
    wifi.start()?; // Wi-Fiを開始してペリフェラルを有効化
    info!("WiFi peripheral started for ESP-NOW");


    let mut led = PinDriver::output(peripherals.pins.gpio4)?;
    led.set_low()?;

    info!("Initialize the camera");
    let camera_params = esp_camera_rs::CameraParams::new()
        .set_clock_pin(peripherals.pins.gpio27)
        .set_d0_pin(peripherals.pins.gpio32)
        .set_d1_pin(peripherals.pins.gpio35)
        .set_d2_pin(peripherals.pins.gpio34)
        .set_d3_pin(peripherals.pins.gpio5)
        .set_d4_pin(peripherals.pins.gpio39)
        .set_d5_pin(peripherals.pins.gpio18)
        .set_d6_pin(peripherals.pins.gpio36)
        .set_d7_pin(peripherals.pins.gpio19)
        .set_vertical_sync_pin(peripherals.pins.gpio22)
        .set_horizontal_reference_pin(peripherals.pins.gpio26)
        .set_pixel_clock_pin(peripherals.pins.gpio21)
        .set_sda_pin(peripherals.pins.gpio25)
        .set_scl_pin(peripherals.pins.gpio23)
        .set_frame_size(esp_idf_svc::sys::camera::framesize_t_FRAMESIZE_SVGA)
        .set_fb_location(esp_idf_svc::sys::camera::camera_fb_location_t_CAMERA_FB_IN_DRAM);

    let camera = Arc::new(esp_camera_rs::Camera::new(&camera_params)?);

    info!("Initializing ESP-NOW");
    unsafe {
        esp_now_init();
        esp_now_register_send_cb(Some(esp_now_send_cb));

        let mut peer_info = esp_now_peer_info_t::default();
        peer_info.channel = 0; // 0を指定すると現在のチャンネルを使用
        peer_info.ifidx = esp_idf_sys::wifi_interface_t_WIFI_IF_STA; // STAインターフェースを使用
        peer_info.encrypt = false;
        peer_info.peer_addr = ESP_NOW_TARGET_MAC;
        esp_now_add_peer(&peer_info);
    }

    loop {
        SEND_FAILED.store(false, Ordering::SeqCst); // ループ開始時に失敗フラグをリセット
        info!("Taking a picture");
        led.set_high()?;
        let _ = camera.get_framebuffer(); // 1枚目は捨てる
        FreeRtos::delay_ms(100);
        if let Some(framebuffer) = camera.get_framebuffer() {
            info!(
                "Took picture: {width}x{height} {size} bytes",
                width = framebuffer.width(),
                height = framebuffer.height(),
                size = framebuffer.data().len(),
            );
            led.set_low()?;
            FreeRtos::delay_ms(100);
            led.set_high()?;
            FreeRtos::delay_ms(100);
            led.set_low()?;

            let data = framebuffer.data();
            let chunk_size = 250; // ESP-NOWの最大データ長
            let mut send_error_in_loop = false;

            for chunk in data.chunks(chunk_size) {
                // 前回の送信完了を待つ
                while !SEND_COMPLETE.load(Ordering::SeqCst) {
                    FreeRtos::delay_ms(1); // CPU負荷軽減のため少し待つ
                }
                SEND_COMPLETE.store(false, Ordering::SeqCst); // 送信開始

                unsafe {
                    let result = esp_now_send(ESP_NOW_TARGET_MAC.as_ptr(), chunk.as_ptr(), chunk.len());
                    if result != 0 {
                        error!("ESP-NOW send queue failed: {}", result);
                        SEND_COMPLETE.store(true, Ordering::SeqCst); // エラーでもフラグを立てる
                        SEND_FAILED.store(true, Ordering::SeqCst); // 失敗フラグを立てる
                        send_error_in_loop = true;
                        break; // キューイングエラーが発生したら送信中断
                    }
                    // 送信成功時はコールバックでSEND_COMPLETEがtrueになるのを待つ
                }
            }

            // 最後のチャンクの送信完了を待つ (キューイングエラーがなかった場合のみ)
            if !send_error_in_loop {
                while !SEND_COMPLETE.load(Ordering::SeqCst) {
                    FreeRtos::delay_ms(1);
                }
            }

            // コールバックで設定された失敗フラグを確認
            if SEND_FAILED.load(Ordering::SeqCst) {
                error!("Failed to send all chunks (Callback reported failure).");
            } else if send_error_in_loop {
                 error!("Failed to send all chunks (Queueing failed).");
            }
             else {
                info!("All chunks sent successfully.");
            }

        } else {
            error!("Failed to take image");
        }

        info!("Entering Deep Sleep for 10 minutes...");
        unsafe {
            esp_deep_sleep(10 * 60 * 1000 * 1000); // 10分
        }
    }
}
