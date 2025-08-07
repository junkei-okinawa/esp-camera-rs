use esp_camera_rs::{Camera, CameraParams};

use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{bail, Result};

use esp_idf_svc::{
    eventloop::EspSystemEventLoop,
    hal::io::{EspIOError, Write},
    hal::peripherals::Peripherals,
    http::{server::EspHttpServer, Method},
};

mod config;
mod wifi_handler;

use config::get_config;
use wifi_handler::my_wifi;

fn main() -> Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    let sysloop = EspSystemEventLoop::take()?;

    let peripherals = Peripherals::take().unwrap();
    let modem_peripheral = peripherals.modem;

    let config = get_config();

    let _wifi = match my_wifi(config.wifi_ssid, config.wifi_psk, modem_peripheral, sysloop) {
        Ok(inner) => inner,
        Err(err) => {
            bail!("Could not connect to Wi-Fi network: {:?}", err)
        }
    };

    let camera_params = CameraParams::new()
        .set_clock_pin(peripherals.pins.gpio10)
        .set_d0_pin(peripherals.pins.gpio15)
        .set_d1_pin(peripherals.pins.gpio17)
        .set_d2_pin(peripherals.pins.gpio18)
        .set_d3_pin(peripherals.pins.gpio16)
        .set_d4_pin(peripherals.pins.gpio14)
        .set_d5_pin(peripherals.pins.gpio12)
        .set_d6_pin(peripherals.pins.gpio11)
        .set_d7_pin(peripherals.pins.gpio48)
        .set_vertical_sync_pin(peripherals.pins.gpio38)
        .set_horizontal_reference_pin(peripherals.pins.gpio47)
        .set_pixel_clock_pin(peripherals.pins.gpio13)
        .set_sda_pin(peripherals.pins.gpio40)
        .set_scl_pin(peripherals.pins.gpio39)
        .set_xclk_freq_hz(20_000_000)
        .set_frame_size(esp_idf_sys::camera::framesize_t_FRAMESIZE_UXGA) // Cast to u32
        // .set_frame_size(esp_idf_sys::camera::framesize_t_FRAMESIZE_SVGA) // Cast to u32
        .set_jpeg_quality(12) // 注意: この設定を有効にすると `cam_hal: NO-EOI` エラーが発生する (2025-05-09時点)
        .set_fb_count(2)
        .set_grab_mode(esp_idf_sys::camera::camera_grab_mode_t_CAMERA_GRAB_LATEST);

    let camera = Arc::new(Mutex::new(Camera::new(&camera_params).unwrap()));

    let mut server = EspHttpServer::new(&esp_idf_svc::http::server::Configuration::default())?;

    server.fn_handler("/", Method::Get, |request| {
        let mut response = request.into_ok_response()?;
        response.write_all("ok".as_bytes())?;
        Ok::<(), EspIOError>(())
    })?;

    let camera_jpg = camera.clone();
    server.fn_handler("/camera.jpg", Method::Get, move |request| {
        log::info!("camera.jpg requested");
        let camera = camera_jpg.lock().unwrap();
        camera.get_framebuffer();
        let framebuffer = camera.get_framebuffer();

        if let Some(framebuffer) = framebuffer {
            log::info!("Got framebuffer! len={}", framebuffer.data().len());
            let data = framebuffer.data();

            let headers = [
                ("Content-Type", "image/jpeg"),
                ("Content-Length", &data.len().to_string()),
            ];
            let mut response = request.into_response(200, Some("OK"), &headers).unwrap();
            response.write_all(data)?;
        } else {
            log::warn!("No framebuffer!");
            let mut response = request.into_ok_response()?;
            response.write_all("no framebuffer".as_bytes())?;
        }

        Ok::<(), EspIOError>(())
    })?;

    let camera_mjpeg = camera.clone();
    server.fn_handler("/camera.mjpeg", Method::Get, move |request| {
        log::info!("camera.mjpeg requested");

        let headers = [("Content-Type", "multipart/x-mixed-replace; boundary=frame")];
        let mut response = request.into_response(200, Some("OK"), &headers).unwrap();

        loop {
            let camera = camera_mjpeg.lock().unwrap();
            camera.get_framebuffer();
            if let Some(framebuffer) = camera.get_framebuffer() {
                let data = framebuffer.data();
                let frame_header = format!(
                    "--frame\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\n\r\n",
                    data.len()
                );
                response.write_all(frame_header.as_bytes())?;
                response.write_all(data)?;
                response.write_all(b"\r\n")?;
            } else {
                log::warn!("No framebuffer!");
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        response.write_all(b"--frame--\r\n")?;

        Ok::<(), EspIOError>(())
    })?;

    loop {
        std::thread::sleep(std::time::Duration::from_millis(1000));
    }
}
