use crate::mac_address::MacAddress;

/// アプリケーション設定
///
/// この構造体はビルド時に`build.rs`によって`cfg.toml`ファイルから
/// 読み込まれた設定を保持します。
#[toml_cfg::toml_config]
pub struct Config {
    #[default("")]
    receiver_mac: &'static str,
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

        Ok(AppConfig { receiver_mac })
    }
}

#[cfg(test)]
mod tests {
    // テストは環境が整ったタイミングで追加
}
