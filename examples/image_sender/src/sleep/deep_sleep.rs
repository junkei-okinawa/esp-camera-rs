use esp_idf_sys::esp_deep_sleep;
use log::info;
use std::time::{Duration, Instant};

/// ディープスリープ制御エラー
#[derive(Debug, thiserror::Error)]
pub enum DeepSleepError {
    #[error("スリープ時間が不正です: {0}")]
    InvalidDuration(String),
}

/// ディープスリープ管理
pub struct DeepSleep;

impl DeepSleep {
    /// 指定された期間ディープスリープに入ります
    ///
    /// # 引数
    ///
    /// * `duration` - スリープする期間
    ///
    /// この関数はディープスリープに入るため、通常は戻りません。
    /// エラーを返す場合は、スリープに入る前にエラーが発生した場合です。
    pub fn sleep_for(duration: Duration) -> Result<(), DeepSleepError> {
        if duration.as_micros() == 0 || duration.as_micros() > u64::MAX as u128 {
            return Err(DeepSleepError::InvalidDuration(format!(
                "スリープ時間が範囲外です: {:?}",
                duration
            )));
        }

        let duration_us = duration.as_micros() as u64;
        info!("ディープスリープに入ります: {} マイクロ秒", duration_us);

        unsafe {
            esp_deep_sleep(duration_us);
        }

        // このコードは実行されません（ディープスリープに入るため）
        // コンパイラの警告を抑制するためのコード
        #[allow(unreachable_code)]
        {
            panic!("ディープスリープから復帰することはありません");
        }
    }

    /// ループの実行時間を考慮してディープスリープに入ります
    ///
    /// # 引数
    ///
    /// * `loop_start_time` - ループの開始時間
    /// * `target_interval` - 目標とする合計時間間隔
    /// * `min_sleep_duration` - 最小スリープ時間
    ///
    /// この関数はディープスリープに入るため、通常は戻りません。
    /// エラーを返す場合は、スリープに入る前にエラーが発生した場合です。
    pub fn sleep_with_timing(
        loop_start_time: Instant,
        target_interval: Duration,
        min_sleep_duration: Duration,
    ) -> Result<(), DeepSleepError> {
        let elapsed_time = loop_start_time.elapsed();
        let sleep_duration = target_interval.saturating_sub(elapsed_time);

        // 最小スリープ時間を確保
        let final_sleep_duration = std::cmp::max(sleep_duration, min_sleep_duration);

        info!(
            "ループ実行時間: {:?}, 計算されたスリープ時間: {:?}, 最終スリープ時間: {:?}",
            elapsed_time, sleep_duration, final_sleep_duration
        );

        Self::sleep_for(final_sleep_duration)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    // モック関数のフラグ
    static mut DEEP_SLEEP_CALLED: bool = false;
    static mut LAST_SLEEP_DURATION: u64 = 0;

    // esp_deep_sleepのモック実装
    #[cfg(test)]
    fn mock_esp_deep_sleep(duration_us: u64) {
        unsafe {
            DEEP_SLEEP_CALLED = true;
            LAST_SLEEP_DURATION = duration_us;
        }
    }

    #[test]
    fn test_invalid_duration_zero() {
        // ゼロ時間のディープスリープは無効
        let result = DeepSleep::sleep_for(Duration::from_secs(0));
        assert!(result.is_err());

        if let Err(DeepSleepError::InvalidDuration(msg)) = result {
            assert!(msg.contains("範囲外"));
        } else {
            panic!("Expected InvalidDuration error");
        }
    }

    #[test]
    fn test_sleep_with_timing_calculation() {
        // 現在時刻を記録
        let start_time = Instant::now();

        // 100msだけスリープしたと仮定
        std::thread::sleep(Duration::from_millis(100));

        // ターゲット間隔を5秒、最小スリープを1秒に設定
        let target = Duration::from_secs(5);
        let min_sleep = Duration::from_secs(1);

        // sleep_with_timing関数の処理をシミュレート
        let elapsed = start_time.elapsed();
        let sleep_duration = target.saturating_sub(elapsed);
        let final_duration = std::cmp::max(sleep_duration, min_sleep);

        // 経過時間が100ms程度なので、スリープ時間は約4.9秒になるはず
        assert!(final_duration < Duration::from_secs(5));
        assert!(final_duration > Duration::from_secs(4));
    }

    // 注: 実際のディープスリープ機能をテストするには統合テストが必要
    // 以下は単体テストでできる範囲のテストです
    #[test]
    #[ignore = "ESP32実機環境でのみ実行可能"]
    fn test_sleep_for_duration() {
        // このテストはESP32実機でのみ実行可能なので通常は無視されます
    }
}
