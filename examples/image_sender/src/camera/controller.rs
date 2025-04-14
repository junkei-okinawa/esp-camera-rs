use esp_camera_rs::{Camera, CameraParams, FrameBuffer};
use esp_idf_svc::hal::gpio;
use esp_idf_sys::camera::camera_fb_location_t_CAMERA_FB_IN_DRAM;
use esp_idf_sys::camera::framesize_t_FRAMESIZE_SVGA;
use log::{error, info};
use std::sync::Arc;

/// カメラ制御に関するエラー
#[derive(Debug, thiserror::Error)]
pub enum CameraError {
    #[error("カメラの初期化に失敗しました: {0}")]
    InitFailed(String),

    #[error("画像キャプチャに失敗しました")]
    CaptureFailed,
}

/// M5Stack Unit Cam構成
///
/// M5Stack Unit Camの設定に必要なパラメータをまとめた構造体です。
pub struct M5UnitCamConfig {
    pub with_psram: bool,
    pub frame_size: u32,
}

impl Default for M5UnitCamConfig {
    fn default() -> Self {
        Self {
            with_psram: false,
            frame_size: framesize_t_FRAMESIZE_SVGA,
        }
    }
}

/// M5Stack Unit Cam (ESP32)向けのカメラコントローラー
pub struct CameraController {
    camera: Arc<Camera<'static>>,
}

impl CameraController {
    /// ペリフェラルから新しいカメラコントローラーを作成します
    ///
    /// # 引数
    ///
    /// * `clock_pin` - カメラクロックピン (gpio27)
    /// * `d0_pin` - データピン0 (gpio32)
    /// * `d1_pin` - データピン1 (gpio35)
    /// * `d2_pin` - データピン2 (gpio34)
    /// * `d3_pin` - データピン3 (gpio5)
    /// * `d4_pin` - データピン4 (gpio39)
    /// * `d5_pin` - データピン5 (gpio18)
    /// * `d6_pin` - データピン6 (gpio36)
    /// * `d7_pin` - データピン7 (gpio19)
    /// * `vsync_pin` - 垂直同期ピン (gpio22)
    /// * `href_pin` - 水平同期ピン (gpio26)
    /// * `pclk_pin` - ピクセルクロックピン (gpio21)
    /// * `sda_pin` - I2C SDAピン (gpio25)
    /// * `scl_pin` - I2C SCLピン (gpio23)
    /// * `config` - 追加の構成パラメータ
    ///
    /// # エラー
    ///
    /// カメラの初期化に失敗した場合にエラーを返します
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        clock_pin: gpio::Gpio27,
        d0_pin: gpio::Gpio32,
        d1_pin: gpio::Gpio35,
        d2_pin: gpio::Gpio34,
        d3_pin: gpio::Gpio5,
        d4_pin: gpio::Gpio39,
        d5_pin: gpio::Gpio18,
        d6_pin: gpio::Gpio36,
        d7_pin: gpio::Gpio19,
        vsync_pin: gpio::Gpio22,
        href_pin: gpio::Gpio26,
        pclk_pin: gpio::Gpio21,
        sda_pin: gpio::Gpio25,
        scl_pin: gpio::Gpio23,
        config: M5UnitCamConfig,
    ) -> Result<Self, CameraError> {
        info!("カメラを初期化しています");

        let camera_params = CameraParams::new()
            .set_clock_pin(clock_pin)
            .set_d0_pin(d0_pin)
            .set_d1_pin(d1_pin)
            .set_d2_pin(d2_pin)
            .set_d3_pin(d3_pin)
            .set_d4_pin(d4_pin)
            .set_d5_pin(d5_pin)
            .set_d6_pin(d6_pin)
            .set_d7_pin(d7_pin)
            .set_vertical_sync_pin(vsync_pin)
            .set_horizontal_reference_pin(href_pin)
            .set_pixel_clock_pin(pclk_pin)
            .set_sda_pin(sda_pin)
            .set_scl_pin(scl_pin)
            .set_frame_size(config.frame_size)
            .set_fb_location(camera_fb_location_t_CAMERA_FB_IN_DRAM);

        let camera =
            Camera::new(&camera_params).map_err(|e| CameraError::InitFailed(format!("{:?}", e)))?;

        Ok(Self {
            camera: Arc::new(camera),
        })
    }

    /// M5Stack Unit Cam用の簡易ファクトリーメソッド
    ///
    /// このメソッドは所有権を消費するため、Peripheralsクレートの所有権を必要とします。
    ///
    /// # 引数
    ///
    /// * `peripherals` - ESP32のペリフェラル
    /// * `config` - カメラ構成
    ///
    /// # エラー
    ///
    /// カメラの初期化に失敗した場合にエラーを返します
    pub fn from_peripherals(
        _peripherals: esp_idf_svc::hal::peripherals::Peripherals,
        _config: M5UnitCamConfig,
    ) -> Result<(Self, esp_idf_svc::hal::peripherals::Peripherals), CameraError> {
        // このメソッドは現在サポートされていないため、
        // 実装せずにエラーを返します

        // カメラコントローラーの作成はできません（所有権の問題）
        // このメソッドは実際には使えないので、エラーを返します
        Err(CameraError::InitFailed(
            "このメソッドは実装されていません。代わりにnewメソッドを使用してください。".to_string(),
        ))
    }

    /// 画像を撮影します
    ///
    /// 最初のフレームは捨てて、2枚目のフレームを返します。
    /// これは一部のカメラで最初のフレームが適切に露出調整されないことがあるためです。
    ///
    /// # エラー
    ///
    /// 画像キャプチャに失敗した場合にエラーを返します
    pub fn capture_image(&self) -> Result<FrameBuffer, CameraError> {
        // 1枚目のキャプチャ（破棄）
        let _ = self.camera.get_framebuffer();

        // 2枚目のキャプチャ（実際に使用）
        self.camera
            .get_framebuffer()
            .ok_or(CameraError::CaptureFailed)
    }

    /// カメラへの参照を取得します
    pub fn camera(&self) -> Arc<Camera> {
        self.camera.clone()
    }
}

#[cfg(test)]
mod tests {
    // テストはハードウェア依存のため省略
}
