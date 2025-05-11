use crate::mac_address::MacAddress;

/// アプリケーション設定
///
/// この構造体はビルド時に`build.rs`によって`cfg.toml`ファイルから
/// 読み込まれた設定を保持します。
#[toml_cfg::toml_config]
pub struct Config {
    #[default("11:22:33:44:55:66")]
    receiver_mac: &'static str,

    #[default(60)]
    sleep_duration_seconds: u64,

    #[default(3600)]
    sleep_duration_seconds_for_long: u64,

    #[default("SVGA")]
    frame_size: &'static str,

    #[default(false)]
    auto_exposure_enabled: bool,

    #[default(255)]
    camera_warmup_frames: u8,

    #[default(255)]
    target_minute_last_digit: u8,

    #[default(255)]
    target_second_last_digit: u8,

    #[default("")]
    wifi_ssid: &'static str,

    #[default("")]
    wifi_password: &'static str,

    #[default("Asia/Tokyo")] // Default to Tokyo timezone
    timezone: &'static str,
}

/// 設定エラー
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("無効な受信機MACアドレス: {0}")]
    InvalidReceiverMac(String),
    #[error("camera_warmup_frames の値が無効です (0-10): {0}")]
    InvalidCameraWarmupFrames(u8),
    #[error("target_minute_last_digit の値が無効です (0-9): {0}")]
    InvalidTargetMinuteLastDigit(u8),
    #[error("target_second_last_digit の値が無効です (0-5): {0}")]
    InvalidTargetSecondLastDigit(u8),
    #[error("WiFi SSIDが設定されていません")]
    MissingWifiSsid,
    #[error("WiFi パスワードが設定されていません")]
    MissingWifiPassword,
}

/// アプリケーション設定を表す構造体
#[derive(Debug, Clone)]
pub struct AppConfig {
    /// 受信機のMACアドレス
    pub receiver_mac: MacAddress,

    /// ディープスリープ時間（秒）
    pub sleep_duration_seconds: u64,

    /// ディープスリープ時間（長時間用、秒）
    pub sleep_duration_seconds_for_long: u64,

    /// フレームサイズ
    pub frame_size: String,

    /// 自動露出設定
    pub auto_exposure_enabled: bool,

    /// カメラウォームアップフレーム数
    pub camera_warmup_frames: Option<u8>,

    /// 目標とする分の下一桁
    pub target_minute_last_digit: Option<u8>,

    /// 目標とする秒の下一桁
    pub target_second_last_digit: Option<u8>,

    /// WiFi SSID
    pub wifi_ssid: String,

    /// WiFi パスワード
    pub wifi_password: String,

    /// タイムゾーン
    pub timezone: String,
}

impl AppConfig {
    /// 設定ファイルから設定をロードします
    pub fn load() -> Result<Self, ConfigError> {
        // toml_cfg によって生成された定数
        let config = CONFIG;

        // 受信機のMACアドレスをパース
        let receiver_mac_str = config.receiver_mac;
        if receiver_mac_str == "11:22:33:44:55:66" || receiver_mac_str == "" {
            return Err(ConfigError::InvalidReceiverMac(
                "空のMACアドレス".to_string(),
            ));
        }

        let receiver_mac = MacAddress::from_str(receiver_mac_str)
            .map_err(|e| ConfigError::InvalidReceiverMac(e.to_string()))?;

        // ディープスリープ時間を設定
        let sleep_duration_seconds = config.sleep_duration_seconds;
        let sleep_duration_seconds_for_long = config.sleep_duration_seconds_for_long;

        // フレームサイズを設定
        let frame_size = config.frame_size.to_string();

        // 自動露出設定を取得
        let auto_exposure_enabled = config.auto_exposure_enabled;

        // カメラウォームアップフレーム数を取得・検証
        let camera_warmup_frames_val = config.camera_warmup_frames;
        if !(camera_warmup_frames_val <= 10 || camera_warmup_frames_val == 255) {
            return Err(ConfigError::InvalidCameraWarmupFrames(
                camera_warmup_frames_val,
            ));
        }
        let camera_warmup_frames = if camera_warmup_frames_val == 255 {
            None
        } else {
            Some(camera_warmup_frames_val)
        };

        // 目標とする分の下一桁を取得・検証
        let target_minute_last_digit_val = config.target_minute_last_digit;
        if !(target_minute_last_digit_val <= 9 || target_minute_last_digit_val == 255) {
            return Err(ConfigError::InvalidTargetMinuteLastDigit(
                target_minute_last_digit_val,
            ));
        }
        let target_minute_last_digit = if target_minute_last_digit_val == 255 {
            None
        } else {
            Some(target_minute_last_digit_val)
        };

        // 目標とする秒の下一桁を取得・検証
        let target_second_last_digit_val = config.target_second_last_digit;
        if !(target_second_last_digit_val <= 5 || target_second_last_digit_val == 255) {
            return Err(ConfigError::InvalidTargetSecondLastDigit(
                target_second_last_digit_val,
            ));
        }
        let target_second_last_digit = if target_second_last_digit_val == 255 {
            None
        } else {
            Some(target_second_last_digit_val)
        };

        // WiFi設定を取得
        let wifi_ssid = config.wifi_ssid.to_string();
        if wifi_ssid.is_empty() {
            return Err(ConfigError::MissingWifiSsid);
        }
        let wifi_password = config.wifi_password.to_string();
        if wifi_password.is_empty() {
            return Err(ConfigError::MissingWifiPassword);
        }

        // タイムゾーンを取得
        let timezone = config.timezone.to_string();

        Ok(AppConfig {
            receiver_mac,
            sleep_duration_seconds,
            sleep_duration_seconds_for_long,
            frame_size,
            auto_exposure_enabled,
            camera_warmup_frames,
            target_minute_last_digit,
            target_second_last_digit,
            wifi_ssid,
            wifi_password,
            timezone,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // AppConfig のフィールドをすべて含むようにシミュレーション関数を更新
    // 注意: このテストは toml_cfg によって生成される CONFIG 定数を直接モックできないため、
    // AppConfig::load() のロジックを部分的に再現する形になります。
    // 実際の toml_cfg の動作確認はビルドと実行を通じて行われます。
    fn simulate_app_config_creation(
        receiver_mac_str: &str,
        sleep_duration: u64,
        sleep_duration_for_long: u64,
        frame_size_str: &str,
        auto_exposure: bool,
        warmup_frames_val: u8,
        minute_digit_val: u8,
        second_digit_val: u8,
        wifi_ssid_str: &str,
        wifi_password_str: &str,
        timezone_str: &str, // Add timezone to simulation
    ) -> Result<Box<AppConfig>, ConfigError> {
        // Changed return type to Box<AppConfig>
        let mac = MacAddress::from_str(receiver_mac_str)
            .map_err(|e| ConfigError::InvalidReceiverMac(e.to_string()))?;

        let cam_warmup = if warmup_frames_val == 255 {
            None
        } else if warmup_frames_val <= 10 {
            Some(warmup_frames_val)
        } else {
            return Err(ConfigError::InvalidCameraWarmupFrames(warmup_frames_val));
        };

        let min_digit_opt = if minute_digit_val == 255 {
            None
        } else if minute_digit_val <= 9 {
            // Corrected upper bound for minute_digit_val
            Some(minute_digit_val)
        } else {
            return Err(ConfigError::InvalidTargetMinuteLastDigit(minute_digit_val));
        };

        let sec_digit_opt = if second_digit_val == 255 {
            None
        } else if second_digit_val <= 5 {
            // Corrected upper bound for second_digit_val
            Some(second_digit_val)
        } else {
            return Err(ConfigError::InvalidTargetSecondLastDigit(second_digit_val));
        };

        if wifi_ssid_str.is_empty() {
            return Err(ConfigError::MissingWifiSsid);
        }
        if wifi_password_str.is_empty() {
            return Err(ConfigError::MissingWifiPassword);
        }

        Ok(Box::new(AppConfig {
            // Wrap AppConfig in Box::new()
            receiver_mac: mac,
            sleep_duration_seconds: sleep_duration,
            sleep_duration_seconds_for_long: sleep_duration_for_long,
            frame_size: frame_size_str.to_string(),
            auto_exposure_enabled: auto_exposure,
            camera_warmup_frames: cam_warmup,
            target_minute_last_digit: min_digit_opt,
            target_second_last_digit: sec_digit_opt,
            wifi_ssid: wifi_ssid_str.to_string(),
            wifi_password: wifi_password_str.to_string(),
            timezone: timezone_str.to_string(),
        }))
    }

    #[test]
    fn test_config_valid_values() {
        let config = simulate_app_config_creation(
            "11:22:33:44:55:66",
            60,
            3600,
            "SVGA",
            true,
            3,
            0,
            5,
            "test_ssid",
            "test_pass",
            "Asia/Tokyo", // Add timezone to test
        )
        .unwrap();
        assert_eq!(config.target_minute_last_digit, Some(0));
        assert_eq!(config.target_second_last_digit, Some(5));
        assert_eq!(config.camera_warmup_frames, Some(3));
        assert_eq!(config.timezone, "Asia/Tokyo"); // Assert timezone
    }

    #[test]
    fn test_config_values_as_none() {
        let config = simulate_app_config_creation(
            "11:22:33:44:55:66",
            60,
            3600,
            "SVGA",
            true,
            255,
            255,
            255,
            "test_ssid",
            "test_pass",
            "America/New_York", // Add timezone to test
        )
        .unwrap();
        assert_eq!(config.target_minute_last_digit, None);
        assert_eq!(config.target_second_last_digit, None);
        assert_eq!(config.camera_warmup_frames, None);
        assert_eq!(config.timezone, "America/New_York"); // Assert timezone
    }

    #[test]
    fn test_config_invalid_camera_warmup_frames() {
        let result = simulate_app_config_creation(
            "11:22:33:44:55:66",
            60,
            3600,
            "SVGA",
            true,
            11,
            0,
            0,
            "test_ssid",
            "test_pass",
            "UTC", // Added missing timezone argument
        ); // 11は不正 (0-10 or 255)
        assert!(result.is_err());
        match result {
            Err(ConfigError::InvalidCameraWarmupFrames(val)) => assert_eq!(val, 11),
            _ => panic!("Expected InvalidCameraWarmupFrames error"),
        }
    }

    #[test]
    fn test_config_invalid_target_minute_digit() {
        let result = simulate_app_config_creation(
            "11:22:33:44:55:66",
            60,
            3600,
            "SVGA",
            true,
            3,
            10, // Test with a value > 9 for minute
            5,
            "test_ssid",
            "test_pass",
            "UTC",
        );
        assert!(result.is_err());
        match result {
            Err(ConfigError::InvalidTargetMinuteLastDigit(val)) => assert_eq!(val, 10),
            _ => panic!("Expected InvalidTargetMinuteLastDigit error"),
        }
    }

    #[test]
    fn test_config_invalid_target_second_digit() {
        let result = simulate_app_config_creation(
            "11:22:33:44:55:66",
            60,
            3600,
            "SVGA",
            true,
            3,
            0,
            6, // Test with a value > 5 for second
            "test_ssid",
            "test_pass",
            "UTC",
        );
        assert!(result.is_err());
        match result {
            Err(ConfigError::InvalidTargetSecondLastDigit(val)) => assert_eq!(val, 6),
            _ => panic!("Expected InvalidTargetSecondLastDigit error"),
        }
    }

    #[test]
    fn test_original_config_sleep_duration() {
        let config = simulate_app_config_creation(
            "11:22:33:44:55:66",
            120,
            3600,
            "SVGA",
            true,
            255,
            255,
            255,
            "test_ssid",
            "test_pass",
            "UTC", // Added missing timezone argument
        )
        .unwrap();
        assert_eq!(config.sleep_duration_seconds, 120);
        assert_eq!(config.sleep_duration_seconds_for_long, 3600);
        assert_eq!(config.frame_size, "SVGA");
    }

    #[test]
    fn test_original_config_default_sleep_duration() {
        let config = simulate_app_config_creation(
            "11:22:33:44:55:66",
            60,
            3600,
            "SVGA",
            true,
            255,
            255,
            255,
            "test_ssid",
            "test_pass",
            "UTC", // Added missing timezone argument
        )
        .unwrap();
        assert_eq!(config.sleep_duration_seconds, 60);
    }

    #[test]
    fn test_original_invalid_mac_address() {
        let result = simulate_app_config_creation(
            "invalid-mac",
            60,
            3600,
            "SVGA",
            true,
            255,
            255,
            255,
            "test_ssid",
            "test_pass",
            "UTC", // Added missing timezone argument
        );
        assert!(result.is_err());
        match result {
            Err(ConfigError::InvalidReceiverMac(_)) => {}
            _ => panic!("Expected InvalidReceiverMac error"),
        }
    }

    #[test]
    fn test_missing_wifi_ssid() {
        let result = simulate_app_config_creation(
            "11:22:33:44:55:66",
            60,
            3600,
            "SVGA",
            true,
            3,
            0,
            5,
            "",
            "test_pass",
            "UTC", // Added missing timezone argument
        );
        assert!(result.is_err());
        match result {
            Err(ConfigError::MissingWifiSsid) => {}
            _ => panic!("Expected MissingWifiSsid error"),
        }
    }

    #[test]
    fn test_missing_wifi_password() {
        let result = simulate_app_config_creation(
            "11:22:33:44:55:66",
            60,
            3600,
            "SVGA",
            true,
            3,
            0,
            5,
            "test_ssid",
            "",
            "UTC", // Added missing timezone argument
        );
        assert!(result.is_err());
        match result {
            Err(ConfigError::MissingWifiPassword) => {}
            _ => panic!("Expected MissingWifiPassword error"),
        }
    }
}
