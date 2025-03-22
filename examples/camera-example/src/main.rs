use embedded_svc::http::Method;
use embedded_svc::io::Write;
use embedded_svc::wifi::{AuthMethod, ClientConfiguration, Configuration};
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::{gpio::PinDriver, peripherals::Peripherals};
use esp_idf_svc::http::server::{Configuration as HttpServerConfig, EspHttpServer};
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::wifi::{BlockingWifi, EspWifi, PmfConfiguration, ScanMethod};
use log::{error, info};
use std::time::Duration;

const WIFI_SSID: &str = "nozomitsu";
const WIFI_PASS: &str = "chigemotsu";

fn main() -> anyhow::Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    info!("initializing peripherals");
    let peripherals = Peripherals::take().unwrap();
    let sysloop = EspSystemEventLoop::take()?;
    let nvs = EspDefaultNvsPartition::take()?;

    info!("initializing WiFi");
    let mut wifi = BlockingWifi::wrap(
        EspWifi::new(peripherals.modem, sysloop.clone(), Some(nvs))?,
        sysloop,
    )?;

    wifi.set_configuration(&Configuration::Client(ClientConfiguration {
        ssid: WIFI_SSID.try_into().unwrap(),
        bssid: None,
        auth_method: AuthMethod::None,
        password: WIFI_PASS.try_into().unwrap(),
        channel: None,
        pmf_cfg: PmfConfiguration::Capable { required: true },
        scan_method: ScanMethod::FastScan,
    }))?;

    wifi.start()?;
    wifi.connect()?;
    wifi.wait_netif_up()?;
    info!(
        "Wifi is ready, ip {:?}",
        wifi.wifi().sta_netif().get_ip_info()?
    );

    // let mut led = PinDriver::output(peripherals.pins.gpio2)?;
    let mut led = PinDriver::output(peripherals.pins.gpio4)?;
    led.set_low()?;

    info!("Initialize the camera");

    // let camera_params = esp_camera_rs::CameraParams::new()
    //     .set_clock_pin(peripherals.pins.gpio15)
    //     .set_d0_pin(peripherals.pins.gpio11)
    //     .set_d1_pin(peripherals.pins.gpio9)
    //     .set_d2_pin(peripherals.pins.gpio8)
    //     .set_d3_pin(peripherals.pins.gpio10)
    //     .set_d4_pin(peripherals.pins.gpio12)
    //     .set_d5_pin(peripherals.pins.gpio18)
    //     .set_d6_pin(peripherals.pins.gpio17)
    //     .set_d7_pin(peripherals.pins.gpio16)
    //     .set_vertical_sync_pin(peripherals.pins.gpio6)
    //     .set_horizontal_reference_pin(peripherals.pins.gpio7)
    //     .set_pixel_clock_pin(peripherals.pins.gpio13)
    //     .set_sda_pin(peripherals.pins.gpio4)
    //     .set_scl_pin(peripherals.pins.gpio5);

    // example from micropython M5CAMERA settings
    // # camera.init(0, format=camera.JPEG, framesize=camera.FRAME_QQVGA,
    // #         sioc=23, siod=25, xclk=27, vsync=22, href=26, pclk=21,
    // #         d0=32, d1=35, d2=34, d3=5, d4=39, d5=18, d6=36, d7=19,
    // #         reset=15)
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

    let camera = esp_camera_rs::Camera::new(&camera_params)?;

    info!("initializing http servert");
    //It's better to use camera from main loop, but for simplicity it is passed it to handler
    let state = std::sync::Arc::new(std::sync::Mutex::new((led, camera)));
    let mut httpserver = EspHttpServer::new(&HttpServerConfig::default())?;

    info!("preocessing http requests");
    httpserver.fn_handler("/", Method::Get, move |request| {
        info!("taking a picture");
        let (ref mut led, camera) = &mut *state.lock().unwrap();

        led.set_high()?;
        let _ = camera.get_framebuffer(); // 1枚目は捨てる
        std::thread::sleep(Duration::from_millis(100));
        let framebuffer = camera.get_framebuffer();
        std::thread::sleep(Duration::from_millis(100));
        led.set_low()?;
        std::thread::sleep(Duration::from_millis(100));
        led.set_high()?;
        std::thread::sleep(Duration::from_millis(100));
        led.set_low()?;
        if let Some(framebuffer) = framebuffer {
            info!(
                "took picture: {width}x{height} {size} bytes",
                width = framebuffer.width(),
                height = framebuffer.height(),
                size = framebuffer.data().len(),
            );
            let mut response =
                request.into_response(200, Some("Ok"), &[("Content-Type", "image/jpeg")])?;
            response.write_all(framebuffer.data())
        } else {
            error!("failed to take image");
            let mut response = request.into_status_response(500)?;
            response.write_all(b"camera failed")
        }
        .map(|_| ())
    })?;

    loop {
        std::thread::sleep(Duration::from_millis(1000));
    }
}
