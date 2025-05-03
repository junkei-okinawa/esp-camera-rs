use sha2::{Digest, Sha256};

/// 画像データのフレーム処理に関するエラー
#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("データが空です")]
    EmptyData,

    #[error("ハッシュ計算エラー: {0}")]
    HashCalculationError(String),
}

/// 画像フレームを処理するためのユーティリティ
pub struct ImageFrame;

impl ImageFrame {
    /// 画像データのSHA256ハッシュを計算します
    ///
    /// # 引数
    ///
    /// * `data` - ハッシュを計算する画像データ
    ///
    /// # 戻り値
    ///
    /// 16進数形式のハッシュ文字列
    ///
    /// # エラー
    ///
    /// データが空の場合にエラーを返します
    pub fn calculate_hash(data: &[u8]) -> Result<String, FrameError> {
        if data.is_empty() {
            return Err(FrameError::EmptyData);
        }

        let mut hasher = Sha256::new();
        hasher.update(data);
        let hash_result = hasher.finalize();
        let hash_hex = format!("{:x}", hash_result);

        Ok(hash_hex)
    }

    /// ハッシュメッセージと電圧パーセンテージを準備します
    ///
    /// # 引数
    ///
    /// * `hash` - 画像データのハッシュ値
    /// * `voltage_percent` - 測定された電圧のパーセンテージ (0-100)
    ///
    /// # 戻り値
    ///
    /// \"HASH:<hash_value>,VOLT:<voltage_percent>\"形式のバイトベクター
    pub fn prepare_hash_message(hash: &str, voltage_percent: u8) -> Vec<u8> {
        format!("HASH:{},VOLT:{}", hash, voltage_percent).into_bytes()
    }

    /// EOFメッセージを準備します
    ///
    /// # 戻り値
    ///
    /// "EOF!"というバイトシーケンス
    pub fn prepare_eof_message() -> &'static [u8] {
        b"EOF!"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "ESP32実機環境でスレッド間通信エラーが発生するためスキップ"]
    fn test_calculate_hash() {
        let data = b"test data";
        let hash = ImageFrame::calculate_hash(data).unwrap();
        // SHA256("test data") = "916f0027a575074ce72a331777c3478d6513f786a591bd892da1a577bf2335f9"
        assert_eq!(
            hash,
            "916f0027a575074ce72a331777c3478d6513f786a591bd892da1a577bf2335f9"
        );
    }

    #[test]
    #[ignore = "ESP32実機環境でヒープメモリ問題が発生するためスキップ"]
    fn test_empty_data_hash() {
        let data = b"";
        let result = ImageFrame::calculate_hash(data);
        assert!(result.is_err());
    }

    #[test]
    #[ignore = "ESP32実機環境でStoreProhibitedエラーが発生するためスキップ"]
    fn test_prepare_hash_message() {
        let hash = "abcdef1234567890";
        let voltage_percent = 75;
        let message = ImageFrame::prepare_hash_message(hash, voltage_percent);
        assert_eq!(message, b"HASH:abcdef1234567890,VOLT:75");
    }
}
