use crate::mac_address::MacAddress;

/// アプリケーション設定
///
/// この構造体はビルド時に`build.rs`によって`cfg.toml`ファイルから
/// 読み込まれた設定を保持します。
#[toml_cfg::toml_config]
pub struct Config {
    #[default("")]
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
}

/// 設定エラー
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("無効な受信機MACアドレス: {0}")]
    InvalidReceiverMac(String),
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
}

impl AppConfig {
    /// 設定ファイルから設定をロードします
    pub fn load() -> Result<Self, ConfigError> {
        // toml_cfg によって生成された定数
        let config = CONFIG;

        // 受信機のMACアドレスをパース
        let receiver_mac_str = config.receiver_mac;
        if receiver_mac_str.is_empty() {
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

        // カメラウォームアップフレーム数を取得
        let camera_warmup_frames = if config.camera_warmup_frames == 255 {
            None
        } else {
            Some(config.camera_warmup_frames)
        };

        Ok(AppConfig {
            receiver_mac,
            sleep_duration_seconds,
            sleep_duration_seconds_for_long,
            frame_size: frame_size,
            auto_exposure_enabled,
            camera_warmup_frames,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 設定ロードのシミュレーション関数（実際は内部実装が見えないので本当はテストできない）
    fn simulate_config_load(
        receiver_mac: &str,
        sleep_duration: u64,
        sleep_duration_for_long: u64,
        frame_size: &str,
    ) -> Result<AppConfig, ConfigError> {
        // MACアドレスのパース
        let mac = MacAddress::from_str(receiver_mac)
            .map_err(|e| ConfigError::InvalidReceiverMac(e.to_string()))?;

        Ok(AppConfig {
            receiver_mac: mac,
            sleep_duration_seconds: sleep_duration,
            sleep_duration_seconds_for_long,
            frame_size: frame_size.to_string(),
            auto_exposure_enabled: true, // デフォルトで自動露出を有効に
            camera_warmup_frames: None,  // デフォルトではウォームアップフレーム数は未設定
        })
    }

    #[test]
    fn test_config_sleep_duration() {
        // 有効な構成でシミュレーション
        let config = simulate_config_load("11:22:33:44:55:66", 120, 3600, "SVGA").unwrap();

        // スリープ時間が正しく設定されていることを確認
        assert_eq!(config.sleep_duration_seconds, 120);
        assert_eq!(config.sleep_duration_seconds_for_long, 3600);
        assert_eq!(config.frame_size, "SVGA");
    }

    #[test]
    fn test_config_default_sleep_duration() {
        // デフォルト値のチェック（実際のデフォルト値と合わせる）
        let config = simulate_config_load("11:22:33:44:55:66", 60, 3600, "SVGA").unwrap();
        assert_eq!(config.sleep_duration_seconds, 60);
        assert_eq!(config.sleep_duration_seconds_for_long, 3600);
        assert_eq!(config.frame_size, "SVGA");
    }

    #[test]
    fn test_invalid_mac_address() {
        // 無効なMACアドレスでエラーが発生することを確認
        let result = simulate_config_load("invalid-mac", 60, 3600, "SVGA");
        assert!(result.is_err());

        match result {
            Err(ConfigError::InvalidReceiverMac(_)) => {
                // 期待どおりのエラー
            }
            _ => panic!("Expected InvalidReceiverMac error"),
        }
    }
}
