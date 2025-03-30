use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::{delay::FreeRtos, gpio::PinDriver, peripherals::Peripherals}; // PinDriverを再度インポート
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::wifi::{EspWifi, BlockingWifi};
use esp_idf_sys::{
    esp_deep_sleep, esp_now_add_peer, esp_now_init, esp_now_peer_info_t,
    esp_now_register_send_cb, esp_now_send, esp_now_send_status_t,
    esp_now_send_status_t_ESP_NOW_SEND_SUCCESS,
    wifi_interface_t_WIFI_IF_STA, // wifi_phy_rate_t_WIFI_PHY_RATE_MCS0_LGI, esp_wifi_config_espnow_rate, // コメントアウトのまま
};
use log::{error, info};
use sha2::{Digest, Sha256}; // Add sha2 imports
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc; // Arcを再度インポート
use std::time::{Duration, Instant}; // Add Instant for timing

const ESP_NOW_TARGET_MAC: [u8; 6] = [0x24, 0xEC, 0x4A, 0xCA, 0x91, 0x44]; // 受信側のSTA MACアドレス

// 送信完了フラグ
static SEND_COMPLETE: AtomicBool = AtomicBool::new(true);
// 送信失敗フラグ
static SEND_FAILED: AtomicBool = AtomicBool::new(false);

extern "C" fn esp_now_send_cb(_mac_addr: *const u8, status: esp_now_send_status_t) {
    if status == esp_now_send_status_t_ESP_NOW_SEND_SUCCESS {
        // info!("ESP-NOW: Send success"); // 送信成功のログをコメントアウト
    } else {
        error!("ESP-NOW: Send failed");
        SEND_FAILED.store(true, Ordering::SeqCst);
    }
    SEND_COMPLETE.store(true, Ordering::SeqCst);
}

fn main() -> anyhow::Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    info!("initializing peripherals");
    let peripherals = Peripherals::take().unwrap();
    let sysloop = EspSystemEventLoop::take()?;
    let nvs = EspDefaultNvsPartition::take()?;

    info!("initializing WiFi peripheral for ESP-NOW");
    let mut wifi = BlockingWifi::wrap(
        EspWifi::new(peripherals.modem, sysloop.clone(), Some(nvs))?,
        sysloop,
    )?;
    wifi.start()?;
    info!("WiFi peripheral started for ESP-NOW");

    // データレート設定をコメントアウト
    /*
    unsafe {
        let result = esp_wifi_config_espnow_rate(wifi_interface_t_WIFI_IF_STA, wifi_phy_rate_t_WIFI_PHY_RATE_MCS0_LGI);
        if result == esp_idf_svc::sys::ESP_OK {
            info!("Successfully set ESP-NOW rate");
        } else {
            error!("Failed to set ESP-NOW rate: {}", result);
        }
    }
    */

    // LEDを再度有効化
    let mut led = PinDriver::output(peripherals.pins.gpio4)?;
    led.set_low()?;

    // カメラ初期化を再度有効化
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
        peer_info.channel = 0; // Use current channel (like image_reciver branch)
        peer_info.ifidx = wifi_interface_t_WIFI_IF_STA;
        peer_info.encrypt = false;
        peer_info.peer_addr = ESP_NOW_TARGET_MAC;
        esp_now_add_peer(&peer_info);
    }

    // 画像送信ループに戻す
    let target_interval = Duration::from_secs(60); // Target interval: 60 seconds
    let min_sleep_duration = Duration::from_secs(1); // Minimum sleep duration: 1 second

    loop {
        let loop_start_time = Instant::now(); // Record loop start time

        SEND_FAILED.store(false, Ordering::SeqCst);
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

            // Calculate SHA256 hash
            let mut hasher = Sha256::new();
            hasher.update(data);
            let hash_result = hasher.finalize();
            let hash_hex = format!("{:x}", hash_result);
            info!("Image SHA256: {}", hash_hex);

            // Prepare hash message for ESP-NOW
            let hash_message = format!("HASH:{}", hash_hex);
            let hash_payload = hash_message.as_bytes();

            // Send hash before sending image chunks
            SEND_COMPLETE.store(false, Ordering::SeqCst);
            SEND_FAILED.store(false, Ordering::SeqCst);
            unsafe {
                let result = esp_now_send(ESP_NOW_TARGET_MAC.as_ptr(), hash_payload.as_ptr(), hash_payload.len());
                if result != 0 {
                    error!("ESP-NOW send hash failed: {}", result);
                    SEND_COMPLETE.store(true, Ordering::SeqCst);
                    SEND_FAILED.store(true, Ordering::SeqCst);
                    // If sending hash fails, maybe skip sending the image?
                    // For now, we'll just log the error and continue to image sending attempt.
                }
            }
            // Wait for hash send completion
            while !SEND_COMPLETE.load(Ordering::SeqCst) {
                FreeRtos::delay_ms(1);
            }

            // Proceed only if hash sending was successful (or handle error differently)
            if !SEND_FAILED.load(Ordering::SeqCst) {
                info!("Hash sent successfully. Proceeding with image chunks...");
                let chunk_size = 250;
                let mut send_error_in_loop = false;

                for chunk in data.chunks(chunk_size) {
                // 前回の送信完了を待つ
                while !SEND_COMPLETE.load(Ordering::SeqCst) {
                    FreeRtos::delay_ms(1);
                }
                // 送信間隔を調整するために遅延を追加
                FreeRtos::delay_ms(5); // 5msの遅延を追加
                SEND_COMPLETE.store(false, Ordering::SeqCst);

                unsafe {
                    let result = esp_now_send(ESP_NOW_TARGET_MAC.as_ptr(), chunk.as_ptr(), chunk.len());
                    if result != 0 {
                        error!("ESP-NOW send queue failed: {}", result);
                        SEND_COMPLETE.store(true, Ordering::SeqCst);
                        SEND_FAILED.store(true, Ordering::SeqCst);
                        send_error_in_loop = true;
                        break;
                    }
                }
            }

            // 最後のチャンクの送信完了を待つ
            if !send_error_in_loop {
                while !SEND_COMPLETE.load(Ordering::SeqCst) {
                    FreeRtos::delay_ms(1);
                }
            }

            if SEND_FAILED.load(Ordering::SeqCst) {
                error!("Failed to send all chunks (Callback reported failure).");
            } else if send_error_in_loop {
                error!("Failed to send all chunks (Queueing failed).");
            } else {
                info!("All chunks sent successfully. Sending EOF marker...");

                // Wait for the last data chunk send to complete
                while !SEND_COMPLETE.load(Ordering::SeqCst) {
                    FreeRtos::delay_ms(1);
                }
                // Add delay before sending the marker
                FreeRtos::delay_ms(15);
                SEND_COMPLETE.store(false, Ordering::SeqCst);
                SEND_FAILED.store(false, Ordering::SeqCst); // Reset failure flag for this send

                let eof_marker = b"EOF";
                unsafe {
                    let result = esp_now_send(ESP_NOW_TARGET_MAC.as_ptr(), eof_marker.as_ptr(), eof_marker.len());
                    if result != 0 {
                        error!("ESP-NOW send EOF marker failed: {}", result);
                        SEND_COMPLETE.store(true, Ordering::SeqCst); // Ensure completion flag is set on failure
                        SEND_FAILED.store(true, Ordering::SeqCst);
                    }
                }

                // Wait for the EOF marker send to complete
                while !SEND_COMPLETE.load(Ordering::SeqCst) {
                    FreeRtos::delay_ms(1);
                }

                if SEND_FAILED.load(Ordering::SeqCst) {
                    error!("Failed to send EOF marker.");
                } else {
                    info!("EOF marker sent successfully.");
                }
            }

        } else {
            error!("Failed to take image");
        }

        // Calculate elapsed time and determine sleep duration
        let elapsed_time = loop_start_time.elapsed();
        let sleep_duration = target_interval.saturating_sub(elapsed_time);

        // Ensure minimum sleep duration
        let final_sleep_duration = std::cmp::max(sleep_duration, min_sleep_duration);
        let sleep_duration_us = final_sleep_duration.as_micros() as u64;

        info!(
            "Loop took: {:?}. Calculated sleep: {:?}. Entering Deep Sleep for: {} us",
            elapsed_time, sleep_duration, sleep_duration_us
        );

        unsafe {
            esp_deep_sleep(sleep_duration_us);
        }
    } // End of main loop
} // Add closing brace for `if !SEND_FAILED.load(Ordering::SeqCst)` block
} // End of main function
