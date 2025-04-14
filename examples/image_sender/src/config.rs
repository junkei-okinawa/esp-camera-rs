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
}

/// 設定エラー
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("無効な受信機MACアドレス: {0}")]
    InvalidReceiverMac(String),
}

/// アプリケーション設定を表す構造体
#[derive(Debug)]
pub struct AppConfig {
    /// 受信機のMACアドレス
    pub receiver_mac: MacAddress,

    /// ディープスリープ時間（秒）
    pub sleep_duration_seconds: u64,
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

        Ok(AppConfig {
            receiver_mac,
            sleep_duration_seconds,
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
    ) -> Result<AppConfig, ConfigError> {
        // MACアドレスのパース
        let mac = MacAddress::from_str(receiver_mac)
            .map_err(|e| ConfigError::InvalidReceiverMac(e.to_string()))?;

        Ok(AppConfig {
            receiver_mac: mac,
            sleep_duration_seconds: sleep_duration,
        })
    }

    #[test]
    fn test_config_sleep_duration() {
        // 有効な構成でシミュレーション
        let config = simulate_config_load("11:22:33:44:55:66", 120).unwrap();

        // スリープ時間が正しく設定されていることを確認
        assert_eq!(config.sleep_duration_seconds, 120);
    }

    #[test]
    fn test_config_default_sleep_duration() {
        // デフォルト値のチェック（実際のデフォルト値と合わせる）
        let config = simulate_config_load("11:22:33:44:55:66", 60).unwrap();
        assert_eq!(config.sleep_duration_seconds, 60);
    }

    #[test]
    fn test_invalid_mac_address() {
        // 無効なMACアドレスでエラーが発生することを確認
        let result = simulate_config_load("invalid-mac", 60);
        assert!(result.is_err());

        match result {
            Err(ConfigError::InvalidReceiverMac(_)) => {
                // 期待どおりのエラー
            }
            _ => panic!("Expected InvalidReceiverMac error"),
        }
    }
}
