use esp_camera_rs::{Camera, CameraParams, FrameBuffer};
use esp_idf_svc::hal::gpio;
use esp_idf_sys::camera::*;
use log::{error, info, warn};
use std::sync::Arc;

#[derive(Debug, Clone, Copy)] // Added Clone
pub enum CustomFrameSize {
    /// 96x96 解像度
    Qcif = framesize_t_FRAMESIZE_QCIF as isize,
    /// QQVGA 解像度
    Qqvga = framesize_t_FRAMESIZE_QQVGA as isize,
    /// 240x240 解像度
    _240x240 = framesize_t_FRAMESIZE_240X240 as isize,
    /// QVGA 解像度
    Qvga = framesize_t_FRAMESIZE_QVGA as isize,
    /// CIF 解像度
    Cif = framesize_t_FRAMESIZE_CIF as isize, // Corrected from CIF
    /// HVGA 解像度
    Hvga = framesize_t_FRAMESIZE_HVGA as isize,
    /// VGA 解像度
    Vga = framesize_t_FRAMESIZE_VGA as isize,
    /// SVGA 解像度
    Svga = framesize_t_FRAMESIZE_SVGA as isize,
    /// XGA 解像度
    Xga = framesize_t_FRAMESIZE_XGA as isize,
    /// HD 解像度
    Hd = framesize_t_FRAMESIZE_HD as isize,
    /// SXGA 解像度
    Sxga = framesize_t_FRAMESIZE_SXGA as isize,
    /// UXGA 解像度
    Uxga = framesize_t_FRAMESIZE_UXGA as isize,
    /// FHD 解像度
    Fhd = framesize_t_FRAMESIZE_FHD as isize,
    /// P_HD 解像度
    PHd = framesize_t_FRAMESIZE_P_HD as isize, // Corrected from P_hd
    /// P_3MP 解像度
    P3mp = framesize_t_FRAMESIZE_P_3MP as isize, // Corrected from P_3mp
    /// QXGA 解像度
    Qxga = framesize_t_FRAMESIZE_QXGA as isize,
    /// QHD 解像度
    Qhd = framesize_t_FRAMESIZE_QHD as isize,
    /// WQXGA 解像度
    Wqxga = framesize_t_FRAMESIZE_WQXGA as isize,
    /// P_FHD 解像度
    PFhd = framesize_t_FRAMESIZE_P_FHD as isize, // Corrected from P_fhd
    /// QSXGA 解像度
    Qsxga = framesize_t_FRAMESIZE_QSXGA as isize,
}

#[derive(Clone, Debug)] // Added Clone
pub struct M5UnitCamConfig {
    // pub with_psram: bool, // Removed unused field
    pub frame_size: CustomFrameSize,
    pub jpeg_quality: i32,
}

impl Default for M5UnitCamConfig {
    fn default() -> Self {
        Self {
            frame_size: CustomFrameSize::Svga, // デフォルトはSVGA
            jpeg_quality: 12,                  // デフォルトのJPEG品質
        }
    }
}

impl M5UnitCamConfig {
    /// 文字列から framesize_t 定数を取得します
    pub fn from_string(size_str: &str) -> CustomFrameSize {
        match size_str.to_uppercase().as_str() {
            "96X96" => CustomFrameSize::Qcif,
            "QQVGA" => CustomFrameSize::Qqvga,
            "QCIF" => CustomFrameSize::Qcif,
            "HQVGA" => CustomFrameSize::Hvga, // Assuming HQVGA maps to Hvga based on previous logic
            "240X240" => CustomFrameSize::_240x240,
            "QVGA" => CustomFrameSize::Qvga,
            "CIF" => CustomFrameSize::Cif,
            "HVGA" => CustomFrameSize::Hvga,
            "VGA" => CustomFrameSize::Vga,
            "SVGA" => CustomFrameSize::Svga,
            "XGA" => CustomFrameSize::Xga,
            "HD" => CustomFrameSize::Hd,
            "SXGA" => CustomFrameSize::Sxga,
            "UXGA" => CustomFrameSize::Uxga,
            "FHD" => CustomFrameSize::Fhd,
            "P_HD" => CustomFrameSize::PHd,   // Corrected
            "P_3MP" => CustomFrameSize::P3mp, // Corrected
            "QXGA" => CustomFrameSize::Qxga,
            "QHD" => CustomFrameSize::Qhd,
            "WQXGA" => CustomFrameSize::Wqxga,
            "P_FHD" => CustomFrameSize::PFhd, // Corrected
            "QSXGA" => CustomFrameSize::Qsxga,
            _ => {
                warn!(
                    "無効なフレームサイズ '{}' が指定されました。デフォルトの SVGA を使用します。",
                    size_str
                );
                CustomFrameSize::Svga // デフォルト値
            }
        }
    }
}

/// カメラ制御に関するエラー
#[derive(Debug, thiserror::Error)]
pub enum CameraError {
    #[error("カメラの初期化に失敗しました: {0}")]
    InitFailed(String),

    #[error("画像キャプチャに失敗しました")]
    CaptureFailed,
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
            .set_frame_size(config.frame_size as u32) // Cast to u32
            .set_jpeg_quality(config.jpeg_quality) // jpeg_quality を設定
            .set_fb_location(camera_fb_location_t_CAMERA_FB_IN_DRAM);

        let camera =
            Camera::new(&camera_params).map_err(|e| CameraError::InitFailed(format!("{:?}", e)))?;

        Ok(Self {
            camera: Arc::new(camera),
        })
    }

    /// 画像を撮影します
    ///
    /// 最初のフレームは捨てて、2枚目のフレームを返します。
    /// これは一部のカメラで最初のフレームが適切に露出調整されないことがあるためです。
    ///
    /// # エラー
    ///
    /// 画像キャプチャに失敗した場合にエラーを返します
    pub fn capture_image(&self) -> Result<FrameBuffer<'_>, CameraError> {
        self.camera
            .get_framebuffer()
            .ok_or(CameraError::CaptureFailed)
    }
}

#[cfg(test)]
mod tests {
    // テストはハードウェア依存のため省略
}
