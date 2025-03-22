This example is set up for ESP32S3, but you can run it on other ESP32 chips by changing `.cargo/config.toml` and `sdkconfig.defaults` the camera's pins should be changed as well.

To build and run the example you need to set up all the tools necessary to compile Rust for ESP32. Follow [The Rust on ESP Book](https://docs.esp-rs.org/book/) for setup steps.

When you need to export `WIFI_SSID` and `WIFI_PASS` environment variables and run the example by using `cargo run -r`, it will compile and flash the program to an ESP32S3 with a camera. The pins are set up for this [Freenove board](https://github.com/Freenove/Freenove_ESP32_S3_WROOM_Board).

The program will:

1. Connect to your WiFi.
1. Initialize the onboard OV2640 camera as well as a LED.
1. Start HTTP server.
1. Take a jpeg-encoded picture using the camera for each GET on `/` url and send it.
1. Print trace information about each step and action.


note:::
    [Unit Cam Wi-Fi Camera (OV2640)](https://shop.m5stack.com/products/unit-cam-wi-fi-camera-ov2640?variant=39607138222252)で動作させるためにやったこと
:::
1. `.cargo/config.toml`を`esp32`用に修正
    - .cargo/config.toml
        ```diff
        [build]
        -target = "xtensa-esp32s3-espidf"
        +target = "xtensa-esp32-espidf"

        -[target.xtensa-esp32s3-espidf]
        +[target.xtensa-esp32-espidf]
        ...
        [env]
        -MCU="esp32s3"
        +MCU="esp32"
        ```
2. `sdkconfig.defaults`を`esp32`かつ`PSRAM`ナシ用に修正
    - sdkconfig.defaults
        ```diff
        -CONFIG_ESP32S3_SPIRAM_SUPPORT=y
        -CONFIG_SPIRAM_MODE_OCT=y
        -CONFIG_LWIP_LOCAL_HOSTNAME="esp-cam"
        CONFIG_PARTITION_TABLE_SINGLE_APP_LARGE=y
        ```
3. `esp-idf-hal v0.44.1`の`gpio.rs`のPinsの調整

    34,35,36,39 pin が`Input`のみで定義されているので`IO`に変更
    
    `esp-idf-hal`を Fork し`v0.44.1`タグの該当箇所を修正。`.cargo/config.toml`で patch を適用。
    - esp-idf-hal v0.44.1 gpio.rs Line 1673 - 1678 
        ```diff
        -pin!(Gpio34:34, Input, RTC:4, ADC1:6, NODAC:0, NOTOUCH:0);
        -pin!(Gpio35:35, Input, RTC:5, ADC1:7, NODAC:0, NOTOUCH:0);
        -pin!(Gpio36:36, Input, RTC:0, ADC1:0, NODAC:0, NOTOUCH:0);
        +pin!(Gpio34:34, IO, RTC:4, ADC1:6, NODAC:0, NOTOUCH:0);
        +pin!(Gpio35:35, IO, RTC:5, ADC1:7, NODAC:0, NOTOUCH:0);
        +pin!(Gpio36:36, IO, RTC:0, ADC1:0, NODAC:0, NOTOUCH:0);
        pin!(Gpio37:37, Input, RTC:1, ADC1:1, NODAC:0, NOTOUCH:0);
        pin!(Gpio38:38, Input, RTC:2, ADC1:2, NODAC:0, NOTOUCH:0);
        -pin!(Gpio39:39, Input, RTC:3, ADC1:3, NODAC:0, NOTOUCH:0);
        +pin!(Gpio39:39, IO, RTC:3, ADC1:3, NODAC:0, NOTOUCH:0);
        ```
    - `.cargo/config.toml`
        ```diff
        +[patch."https://github.com/esp-rs/esp-idf-hal.git"]
        +esp-idf-hal = { git = "https://github.com/junkei-okinawa/esp-idf-hal.git", branch = "custom-gpio-for-M5UnitCam" }
        ```
4. Pinアサインを変更
    - `src/main.rs`
        ```diff
        let camera_params = esp_camera_rs::CameraParams::new()
        -.set_clock_pin(peripherals.pins.gpio15)
        -.set_d0_pin(peripherals.pins.gpio11)
        -.set_d1_pin(peripherals.pins.gpio9)
        -.set_d2_pin(peripherals.pins.gpio8)
        -.set_d3_pin(peripherals.pins.gpio10)
        -.set_d4_pin(peripherals.pins.gpio12)
        -.set_d5_pin(peripherals.pins.gpio18)
        -.set_d6_pin(peripherals.pins.gpio17)
        -.set_d7_pin(peripherals.pins.gpio16)
        -.set_vertical_sync_pin(peripherals.pins.gpio6)
        -.set_horizontal_reference_pin(peripherals.pins.gpio7)
        -.set_pixel_clock_pin(peripherals.pins.gpio13)
        -.set_sda_pin(peripherals.pins.gpio4)
        -.set_scl_pin(peripherals.pins.gpio5);
        +.set_clock_pin(peripherals.pins.gpio27)
        +.set_d0_pin(peripherals.pins.gpio32)
        +.set_d1_pin(peripherals.pins.gpio35)
        +.set_d2_pin(peripherals.pins.gpio34)
        +.set_d3_pin(peripherals.pins.gpio5)
        +.set_d4_pin(peripherals.pins.gpio39)
        +.set_d5_pin(peripherals.pins.gpio18)
        +.set_d6_pin(peripherals.pins.gpio36)
        +.set_d7_pin(peripherals.pins.gpio19)
        +.set_vertical_sync_pin(peripherals.pins.gpio22)
        +.set_horizontal_reference_pin(peripherals.pins.gpio26)
        +.set_pixel_clock_pin(peripherals.pins.gpio21)
        +.set_sda_pin(peripherals.pins.gpio25)
        +.set_scl_pin(peripherals.pins.gpio23);
        ```

5. bufferの保持にメモリが不足するため`frame_size`を下げる。`fb_location`を`DRAM`に変更し`PSRAM`ナシにする。
    - `src/main.rs`

        `UXGA:1600×1200`だと`frame_size`が大きすぎて落ちる。
        `SVGA:800×600`であれば実行可能。
        ```diff
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
        +    .set_frame_size(esp_idf_svc::sys::camera::framesize_t_FRAMESIZE_SVGA)
        +    .set_fb_location(esp_idf_svc::sys::camera::camera_fb_location_t_CAMERA_FB_IN_DRAM);
        ```
