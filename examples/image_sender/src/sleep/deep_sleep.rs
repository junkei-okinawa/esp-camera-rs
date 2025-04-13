use esp_idf_sys::esp_deep_sleep;
use std::time::{Duration, Instant};
use log::info;

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
    // ハードウェア依存のためテストは省略
}
