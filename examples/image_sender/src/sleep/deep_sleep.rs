use crate::config::AppConfig;
use esp_idf_svc::sntp::{EspSntp, OperatingMode, SntpConf, SyncStatus};
use esp_idf_svc::wifi::{AuthMethod, BlockingWifi, ClientConfiguration, Configuration, EspWifi};
use log::{debug, error, info, warn};
use std::sync::Arc;
use std::time::{Duration as StdDuration, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Duration as ChronoDuration, NaiveDate, TimeZone, Timelike, Utc};
use chrono_tz::{ParseError, Tz};

// Constants for SNTP
const MAX_SNTP_RETRIES: u32 = 30;
const SNTP_RETRY_DELAY_MS_IN_PROGRESS: u64 = 2000; // 2 seconds
const SNTP_RETRY_DELAY_MS_OTHER: u64 = 5000; // 5 seconds

// Constant for sleep logic
const MIN_SLEEP_SECONDS_IN_TSLD_MODE: i64 = 11; // Minimum sleep if target_second_last_digit is used

/// エラーの定義
#[derive(Debug, thiserror::Error)]
pub enum DeepSleepError {
    #[error("スリープ時間が不正です: {0}")]
    InvalidDuration(String),
    #[error("システム時刻の取得に失敗しました: {0}")]
    SystemTimeError(String),
    #[error("Chrono型への変換に失敗しました: {0}")]
    ChronoConversionError(String),
    #[error("WiFiエラー: {0}")]
    WifiError(String),
    #[error("SNTPエラー: {0}")]
    SntpError(String),
    #[error("WiFi接続に失敗しました: {0}")]
    WifiConnectionFailed(String), // Added
    #[error("時刻同期に失敗しました: {0}")]
    TimeSyncFailed(String), // Added
    #[error("設定が不正です: {0}")]
    InvalidConfiguration(String), // Added
}

/// ディープスリープ機能を提供するプラットフォームごとの実装を抽象化するトレイト
pub trait DeepSleepPlatform {
    fn deep_sleep(&self, duration_us: u64);
}

/// ESP-IDF環境用のDeepSleepPlatform実装
pub struct EspIdfDeepSleep;

impl DeepSleepPlatform for EspIdfDeepSleep {
    fn deep_sleep(&self, duration_us: u64) {
        unsafe {
            esp_idf_sys::esp_deep_sleep(duration_us);
        }
    }
}

/// ディープスリープ管理
pub struct DeepSleep<P: DeepSleepPlatform> {
    config: Arc<AppConfig>,
    platform: P,
}

impl<P: DeepSleepPlatform> DeepSleep<P> {
    pub fn new(config: Arc<AppConfig>, platform: P) -> Self {
        DeepSleep { config, platform }
    }

    /// 現在の時刻をWi-Fi経由で同期する必要があるか確認し、必要であれば同期します。
    /// 同期は、現在のシステム時刻が2025年1月1日より前の場合にのみ実行されます。
    pub fn ensure_time_sync_if_needed(
        &mut self,
        wifi: &mut BlockingWifi<EspWifi<'static>>,
        ssid: &str,
        password: &str,
    ) -> Result<(), DeepSleepError> {
        let now_unix_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| {
                DeepSleepError::SystemTimeError(format!(
                    "システム時刻の取得に失敗 (同期前チェック): {}",
                    e
                ))
            })?
            .as_secs();

        let current_time_chrono =
            DateTime::from_timestamp(now_unix_secs as i64, 0).ok_or_else(|| {
                DeepSleepError::ChronoConversionError(
                    "DateTimeへの変換に失敗 (同期前チェック)".to_string(),
                )
            })?;

        // 2025年1月1日 00:00:00 UTC のDateTimeオブジェクトを作成
        let threshold_naive_date = NaiveDate::from_ymd_opt(2025, 1, 1).unwrap();
        let threshold_naive_datetime = threshold_naive_date.and_hms_opt(0, 0, 0).unwrap();
        let threshold_time = Utc.from_utc_datetime(&threshold_naive_datetime);

        info!(
            "現在のシステム時刻 (同期前チェック): {}",
            current_time_chrono.format("%Y-%m-%d %H:%M:%S")
        );

        if current_time_chrono < threshold_time {
            info!(
                "現在時刻が {} より前なので、時刻同期を実行します。",
                threshold_time.format("%Y-%m-%d %H:%M:%S")
            );
            self.synchronize_time(wifi, ssid, password)?;
        } else {
            info!(
                "現在時刻が {} 以降なので、時刻同期をスキップします。",
                threshold_time.format("%Y-%m-%d %H:%M:%S")
            );
        }
        Ok(())
    }

    /// 現在の時刻をWi-Fi経由で同期します。 (この関数は ensure_time_sync_if_needed から呼ばれます)
    fn synchronize_time(
        &mut self,
        wifi: &mut BlockingWifi<EspWifi<'static>>,
        ssid: &str,
        password: &str,
    ) -> Result<(), DeepSleepError> {
        info!("時刻同期を開始します。SSID: {}", ssid);

        // 既存のWiFi設定をクリアし、停止する
        match wifi.stop() {
            Ok(_) => info!("既存のWiFiセッションを停止しました。"),
            Err(e) if e.code() == esp_idf_sys::ESP_ERR_WIFI_NOT_INIT => {
                info!("WiFiはまだ初期化されていません。");
            }
            Err(e) => {
                warn!(
                    "既存のWiFiセッションの停止に失敗しました: {:?}。処理を続行します。",
                    e
                );
            }
        }

        let auth_method = if password.is_empty() {
            info!("WiFiパスワードが空のため、認証方式を None に設定します。");
            AuthMethod::None
        } else {
            AuthMethod::WPA2Personal
        };

        let client_config = ClientConfiguration {
            ssid: ssid
                .try_into()
                .map_err(|e| DeepSleepError::WifiError(format!("無効なSSID: {:?}", e)))?,
            password: password
                .try_into()
                .map_err(|e| DeepSleepError::WifiError(format!("無効なパスワード: {:?}", e)))?,
            auth_method,
            ..Default::default()
        };
        wifi.set_configuration(&Configuration::Client(client_config))
            .map_err(|e| DeepSleepError::WifiError(format!("WiFi設定の適用に失敗: {:?}", e)))?;

        wifi.start()
            .map_err(|e| DeepSleepError::WifiError(format!("WiFiの開始に失敗: {:?}", e)))?;
        info!("WiFiを開始しました。接続を試みます...");

        wifi.connect()
            .map_err(|e| DeepSleepError::WifiError(format!("WiFi接続に失敗: {:?}", e)))?;
        info!("WiFiに接続しました。IPアドレスの取得を待ちます...");
        let ip_info = wifi.wifi().sta_netif().get_ip_info().map_err(|e| {
            error!("IPアドレスの取得に失敗しました: {:?}", e);
            DeepSleepError::WifiConnectionFailed(format!("Failed to get IP info: {:?}", e))
        })?;
        info!("IPアドレスを取得しました: {:?}", ip_info);

        info!("SNTPを初期化しています...");
        let sntp_config = SntpConf {
            operating_mode: OperatingMode::Poll,
            servers: ["ntp.nict.jp"],
            ..Default::default()
        };
        let sntp = match EspSntp::new(&sntp_config) {
            Ok(s) => s,
            Err(e) => {
                error!("SNTPの初期化に失敗しました: {:?}", e);
                Self::disconnect_wifi_static_helper(wifi)?; // Use associated function
                return Err(DeepSleepError::TimeSyncFailed(format!(
                    "SNTP initialization failed: {:?}",
                    e
                )));
            }
        };

        let mut retry_count = 0;
        let mut synchronized = false;
        info!(
            "SNTP初期化完了。時刻同期を開始します... (最大試行回数: {})",
            MAX_SNTP_RETRIES
        );

        while retry_count < MAX_SNTP_RETRIES {
            let status = sntp.get_sync_status();
            match status {
                SyncStatus::Completed => {
                    info!(
                        "SNTPステータス: Completed. 時刻同期に成功しました。 (試行 {}/{})",
                        retry_count + 1,
                        MAX_SNTP_RETRIES
                    );
                    synchronized = true;
                    break;
                }
                SyncStatus::InProgress => {
                    info!(
                        "SNTPステータス: InProgress. 同期処理中... (試行 {}/{})",
                        retry_count + 1,
                        MAX_SNTP_RETRIES
                    );
                    std::thread::sleep(StdDuration::from_millis(SNTP_RETRY_DELAY_MS_IN_PROGRESS));
                }
                SyncStatus::Reset => {
                    info!(
                        "SNTPステータス: Reset. リトライします... (試行 {}/{})",
                        retry_count + 1,
                        MAX_SNTP_RETRIES
                    );
                    std::thread::sleep(StdDuration::from_millis(SNTP_RETRY_DELAY_MS_OTHER));
                }
            }
            retry_count += 1;

            if !synchronized && retry_count == MAX_SNTP_RETRIES / 2 {
                warn!(
                    "SNTP同期が試行の半数 ({}/{}) に達しましたが、まだ完了していません。",
                    retry_count, MAX_SNTP_RETRIES
                );
            }
        }

        if synchronized {
            let now = Utc::now();
            info!("SNTP時刻同期成功。現在のUTC時刻: {}", now);
            let local_time_check = Utc::now().with_timezone(
                &self
                    .config
                    .timezone
                    .parse::<Tz>()
                    .unwrap_or(chrono_tz::Asia::Tokyo),
            );
            info!("現在のローカル時刻 (確認用): {}", local_time_check);
        } else {
            error!(
                "SNTP時刻同期タイムアウト。最大試行回数 ({}) を超えました。",
                MAX_SNTP_RETRIES
            );
            Self::disconnect_wifi_static_helper(wifi)?; // Use associated function
            return Err(DeepSleepError::TimeSyncFailed(
                "SNTP sync timeout after max retries".to_string(),
            ));
        }

        Self::disconnect_wifi_static_helper(wifi)?; // Use associated function
        Ok(())
    }

    pub fn sleep(
        &self,
        elapsed_time_in_current_loop: StdDuration,
        min_sleep_duration: StdDuration,
    ) -> Result<(), DeepSleepError> {
        let timezone_str = self.config.timezone.as_str();

        if self.config.target_minute_last_digit.is_some()
            || self.config.target_second_last_digit.is_some()
        {
            info!("Entering target digit sleep mode.");
            self.sleep_until_target_digits_match(
                timezone_str,
                min_sleep_duration,
                elapsed_time_in_current_loop,
            )
        } else {
            info!("Entering fixed interval sleep mode.");
            let interval_total_secs = self.config.sleep_duration_seconds as u64;
            let mut sleep_for_secs = if interval_total_secs > elapsed_time_in_current_loop.as_secs()
            {
                interval_total_secs - elapsed_time_in_current_loop.as_secs()
            } else {
                warn!(
                    "Processing time ({:?}) exceeded sleep interval ({}s). Using minimum sleep duration ({:?}).",
                    elapsed_time_in_current_loop, interval_total_secs, min_sleep_duration
                );
                min_sleep_duration.as_secs()
            };

            if sleep_for_secs == 0 {
                warn!(
                    "Calculated interval sleep duration is zero. Overriding to min_sleep_duration."
                );
                sleep_for_secs = min_sleep_duration.as_secs();
            }
            if sleep_for_secs == 0 && min_sleep_duration.as_secs() == 0 {
                warn!("Min sleep duration is also zero. Setting to 1 second to avoid issues.");
                sleep_for_secs = 1;
            }

            info!(
                "Deep sleeping for {} seconds (interval mode, timezone: {}).",
                sleep_for_secs, timezone_str
            );
            self.platform.deep_sleep(sleep_for_secs * 1_000_000);
            // deep_sleep からは復帰しないため、Ok(()) は実際には返らない
            // しかし、シグネチャ上は Result を返す必要がある
            #[allow(unreachable_code)]
            Ok(())
        }
    }

    pub fn sleep_for_duration_long(&self, duration_seconds: u64) -> Result<(), DeepSleepError> {
        if duration_seconds == 0 {
            return Err(DeepSleepError::InvalidDuration(
                "スリープ時間は0より大きくなければなりません".to_string(),
            ));
        }
        info!("{}秒間のディープスリープに入ります...", duration_seconds);
        self.platform.deep_sleep(duration_seconds * 1_000_000);
        // deep_sleep からは復帰しないため、Ok(()) は実際には返らない
        // しかし、シグネチャ上は Result を返す必要がある
        #[allow(unreachable_code)]
        Ok(())
    }

    fn sleep_until_target_digits_match(
        &self,
        timezone_str: &str,
        min_sleep_duration_param: StdDuration,
        elapsed_time_in_current_loop: StdDuration,
    ) -> Result<(), DeepSleepError> {
        let tz: Tz = timezone_str.parse().map_err(|e: ParseError| {
            DeepSleepError::InvalidConfiguration(format!(
                "Invalid timezone string: {} ({})",
                timezone_str, e
            ))
        })?;

        let now_in_tz = Utc::now().with_timezone(&tz);
        info!(
            "sleep_until_target_digits_match called. now_in_tz: {}, tz_str: {}, min_sleep_param: {}s, elapsed_loop: {:.3}s",
            now_in_tz, timezone_str, min_sleep_duration_param.as_secs(), elapsed_time_in_current_loop.as_secs_f32()
        );
        info!(
            "Config values: target_minute_last_digit: {:?}, target_second_last_digit: {:?}",
            self.config.target_minute_last_digit, self.config.target_second_last_digit
        );

        let mut search_from_dt = now_in_tz;
        // ... (search_from_dt の調整ロジック) ...
        let current_minute_val = now_in_tz.minute();
        let current_second_val = now_in_tz.second();

        if let Some(target_s_tens_digit_u8) = self.config.target_second_last_digit {
            let target_s_tens_digit = target_s_tens_digit_u8 as u32;
            let current_s_tens = current_second_val / 10;
            let minute_matches_if_set = self
                .config
                .target_minute_last_digit
                .map_or(true, |m_digit| current_minute_val % 10 == m_digit as u32);

            if current_s_tens == target_s_tens_digit && minute_matches_if_set {
                debug!("Current time {} is within a target slot (min_match: {}, sec_tens: {}). Adjusting search start.", now_in_tz, minute_matches_if_set, current_s_tens);
                let next_10s_window_start_second = (current_s_tens + 1) * 10;

                if next_10s_window_start_second < 60 {
                    if let Some(adjusted_dt) = now_in_tz
                        .with_second(next_10s_window_start_second)
                        .and_then(|t| t.with_nanosecond(0))
                    {
                        search_from_dt = adjusted_dt;
                    }
                } else {
                    if let Some(adjusted_dt) = (now_in_tz + ChronoDuration::minutes(1))
                        .with_second(0)
                        .and_then(|t| t.with_nanosecond(0))
                    {
                        search_from_dt = adjusted_dt;
                    }
                }
                info!(
                    "Adjusted search_from_dt to {} to skip current target second-slot.",
                    search_from_dt
                );
            }
        } else if let Some(target_m_digit_u8) = self.config.target_minute_last_digit {
            let target_m_digit = target_m_digit_u8 as u32;
            if current_minute_val % 10 == target_m_digit {
                debug!(
                    "Current time {} is within a target minute slot. Adjusting search start.",
                    now_in_tz
                );
                if let Some(adjusted_dt) = (now_in_tz + ChronoDuration::minutes(1))
                    .with_second(0)
                    .and_then(|t| t.with_nanosecond(0))
                {
                    search_from_dt = adjusted_dt;
                }
                info!(
                    "Adjusted search_from_dt to {} to skip current target minute-slot.",
                    search_from_dt
                );
            }
        }

        if search_from_dt <= now_in_tz {
            search_from_dt = now_in_tz + ChronoDuration::nanoseconds(1);
            debug!(
                "Ensured search_from_dt {} is after now_in_tz {}.",
                search_from_dt, now_in_tz
            );
        }

        let Some(target_datetime) = Self::find_next_target_datetime_from_start(
            search_from_dt,
            self.config.target_minute_last_digit.map(|d| d as u32),
            self.config.target_second_last_digit.map(|d| d as u32),
            &tz,
        ) else {
            error!("Could not determine a target datetime. Falling back to fixed sleep interval based on sleep_duration_seconds.");
            let fallback_sleep_micros = (self.config.sleep_duration_seconds as u64) * 1_000_000;
            info!(
                "Deep sleeping for {} seconds (target calculation failed, using fixed interval).",
                self.config.sleep_duration_seconds
            );
            self.platform.deep_sleep(fallback_sleep_micros);
            unreachable!(); // deep_sleep からは復帰しないため、ここは到達不能
        };

        info!("Found target datetime: {}", target_datetime);
        let duration_to_target = target_datetime.signed_duration_since(now_in_tz);
        info!(
            "Current local time: {}, Determined target local time: {}. Calculated raw duration to target: {}s.",
            now_in_tz.format("%Y-%m-%d %H:%M:%S"),
            target_datetime.format("%Y-%m-%d %H:%M:%S"),
            duration_to_target.num_seconds()
        );

        if duration_to_target.num_seconds() <= 0 {
            error!(
                "Target datetime {} is not in the future from {}. Fallback to min_sleep_duration_param.",
                target_datetime, now_in_tz
            );
            let sleep_micros = min_sleep_duration_param.as_micros() as u64;
            info!(
                "Deep sleeping for {} seconds (target calculation fallback - not in future).",
                min_sleep_duration_param.as_secs()
            );
            self.platform.deep_sleep(sleep_micros);
            unreachable!(); // deep_sleep からは復帰しないため、ここは到達不能
        }

        let mut sleep_duration_seconds = duration_to_target.num_seconds();
        info!(
            "Initial sleep duration based on target: {}s.",
            sleep_duration_seconds
        );

        if self.config.target_second_last_digit.is_some()
            && sleep_duration_seconds < MIN_SLEEP_SECONDS_IN_TSLD_MODE
        {
            info!(
                "Calculated sleep {}s is < {}s in TSLD mode. Adjusting to {}s.",
                sleep_duration_seconds,
                MIN_SLEEP_SECONDS_IN_TSLD_MODE,
                MIN_SLEEP_SECONDS_IN_TSLD_MODE
            );
            sleep_duration_seconds = MIN_SLEEP_SECONDS_IN_TSLD_MODE;
        }

        let min_sleep_param_secs = min_sleep_duration_param.as_secs() as i64;
        if sleep_duration_seconds < min_sleep_param_secs {
            info!(
                "Calculated sleep {}s is less than min_sleep_duration_param {}s. Adjusting to {}s.",
                sleep_duration_seconds, min_sleep_param_secs, min_sleep_param_secs
            );
            sleep_duration_seconds = min_sleep_param_secs;
        }

        if sleep_duration_seconds <= 0 {
            error!(
                "Final sleep duration is non-positive ({}s). Setting to minimal {}s.",
                sleep_duration_seconds,
                min_sleep_param_secs.max(1)
            );
            sleep_duration_seconds = min_sleep_param_secs.max(1);
        }

        let sleep_micros = (sleep_duration_seconds as u64) * 1_000_000;
        info!(
            "Deep sleeping for {} seconds (target digits mode). Target: {}, Elapsed this loop: {:.3}s, Configured interval (floor): {}s, Min sleep param: {}s.",
            sleep_duration_seconds,
            target_datetime.format("%Y-%m-%d %H:%M:%S"),
            elapsed_time_in_current_loop.as_secs_f32(),
            self.config.sleep_duration_seconds,
            min_sleep_duration_param.as_secs()
        );
        self.platform.deep_sleep(sleep_micros);
        // deep_sleep からは復帰しないため、Ok(()) は実際には返らない
        // しかし、シグネチャ上は Result を返す必要がある
        #[allow(unreachable_code)]
        Ok(())
    }

    fn find_next_target_datetime_from_start<TzZone: TimeZone>(
        // ... (実装は変更なし) ...
        start_dt: DateTime<TzZone>,
        target_minute_digit: Option<u32>,
        target_second_tens_digit: Option<u32>,
        _tz: &TzZone,
    ) -> Option<DateTime<TzZone>>
    where
        <TzZone as TimeZone>::Offset: Copy + std::fmt::Debug,
    {
        let mut current_dt_iter = start_dt.with_nanosecond(0).unwrap_or(start_dt);
        if current_dt_iter < start_dt {
            current_dt_iter = current_dt_iter + ChronoDuration::seconds(1);
        }
        debug!("find_next_target_datetime_from_start: initial search iteration time: {:?}, requested start_dt: {:?}", current_dt_iter, start_dt);

        for i in 0..(2 * 60 * 60) {
            if i > 0 {
                current_dt_iter = current_dt_iter + ChronoDuration::seconds(1);
            }

            let minute_val = current_dt_iter.minute();
            let second_val = current_dt_iter.second();

            let minute_match = target_minute_digit.map_or(true, |d| minute_val % 10 == d);

            let second_match_criteria = if let Some(s_tens_digit) = target_second_tens_digit {
                second_val / 10 == s_tens_digit
            } else {
                second_val == 0
            };

            if minute_match && second_match_criteria {
                debug!(
                    "Found potential target: {:?} (min_match: {}, sec_match: {}) at iteration {}",
                    current_dt_iter, minute_match, second_match_criteria, i
                );
                return Some(current_dt_iter);
            }
        }
        warn!("No target datetime found within the search window (2 hours) from requested start_dt {:?}", start_dt);
        None
    }

    pub fn sleep_until_target_time(
        &self,
        target_datetime: DateTime<Utc>,
        _elapsed_time_in_current_loop: StdDuration,
        min_sleep_duration: StdDuration,
    ) -> Result<(), DeepSleepError> {
        let now_utc = Utc::now();
        info!(
            "現在のUTC時刻: {}, スリープ開始時刻: {}",
            now_utc,
            target_datetime.format("%Y-%m-%d %H:%M:%S")
        );

        let duration_to_target = target_datetime.signed_duration_since(now_utc);
        let mut sleep_duration_seconds = duration_to_target.num_seconds();

        if sleep_duration_seconds <= 0 {
            warn!("指定された時刻が現在時刻を過ぎています。スリープしません。");
            return Ok(());
        }

        let min_sleep_secs = min_sleep_duration.as_secs();
        if sleep_duration_seconds < min_sleep_secs as i64 {
            sleep_duration_seconds = min_sleep_secs as i64;
        }

        let sleep_micros = (sleep_duration_seconds as u64) * 1_000_000;
        info!(
            "ディープスリープに入ります: {} 秒後に {} に復帰予定。",
            sleep_duration_seconds,
            target_datetime.format("%Y-%m-%d %H:%M:%S")
        );
        self.platform.deep_sleep(sleep_micros);
        // deep_sleep からは復帰しないため、Ok(()) は実際には返らない
        // しかし、シグネチャ上は Result を返す必要がある
        #[allow(unreachable_code)]
        Ok(())
    }

    fn disconnect_wifi_helper(
        &self,
        wifi: &mut BlockingWifi<EspWifi<'static>>,
    ) -> Result<(), DeepSleepError> {
        info!("WiFiを切断します...");
        wifi.disconnect()
            .map_err(|e| DeepSleepError::WifiError(format!("WiFi切断に失敗: {:?}", e)))?;
        info!("WiFiを切断しました。");
        Ok(())
    }

    // synchronize_time から Self::disconnect_wifi_static_helper を呼ぶように変更
    fn disconnect_wifi_static_helper(
        wifi: &mut BlockingWifi<EspWifi<'static>>,
    ) -> Result<(), DeepSleepError> {
        info!("(Static helper) WiFiを切断します...");
        wifi.disconnect().map_err(|e| {
            DeepSleepError::WifiError(format!("WiFi切断に失敗 (static_helper): {:?}", e))
        })?;
        info!("(Static helper) WiFiを切断しました。");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex; // Add Mutex
    use std::time::Duration;

    /// テスト用のモックDeepSleepPlatform
    struct MockDeepSleepPlatform {
        called: AtomicBool,
        last_duration_us: Mutex<u64>, // Change AtomicU64 to Mutex<u64>
    }

    impl MockDeepSleepPlatform {
        fn new() -> Self {
            MockDeepSleepPlatform {
                called: AtomicBool::new(false),
                last_duration_us: Mutex::new(0), // Initialize Mutex
            }
        }
    }

    impl DeepSleepPlatform for &MockDeepSleepPlatform {
        // 参照で実装
        fn deep_sleep(&self, duration_us: u64) {
            self.called.store(true, Ordering::SeqCst);
            *self.last_duration_us.lock().unwrap() = duration_us; // Lock and set value
                                                                  // 実際のディープスリープは行わない
        }
    }

    fn setup_test_config() -> Arc<AppConfig> {
        Arc::new(AppConfig {
            receiver_mac: crate::mac_address::MacAddress([0x11, 0x22, 0x33, 0x44, 0x55, 0x66]),
            sleep_duration_seconds: 60,
            sleep_duration_seconds_for_long: 3600,
            frame_size: "SVGA".to_string(),
            auto_exposure_enabled: false,
            camera_warmup_frames: None,
            target_minute_last_digit: None,
            target_second_last_digit: None,
            wifi_ssid: "test_ssid".to_string(),
            wifi_password: "test_password".to_string(),
            timezone: "Asia/Tokyo".to_string(),
        })
    }

    #[test]
    fn test_sleep_for_duration_long_valid_duration() {
        let config = setup_test_config();
        let mock_platform = MockDeepSleepPlatform::new();
        let deep_sleep = DeepSleep::new(config, &mock_platform); // 参照を渡す
        let duration_seconds = 10;

        // sleep_for_duration_long は Ok(()) を返す前に deep_sleep を呼ぶ
        // deep_sleep からは戻らない想定だが、モックなので戻ってくる
        let _ = deep_sleep.sleep_for_duration_long(duration_seconds);

        assert!(mock_platform.called.load(Ordering::SeqCst));
        assert_eq!(
            *mock_platform.last_duration_us.lock().unwrap(), // Lock and get value
            duration_seconds * 1_000_000
        );
    }

    #[test]
    fn test_sleep_for_duration_long_zero_duration() {
        let config = setup_test_config();
        let mock_platform = MockDeepSleepPlatform::new();
        let deep_sleep = DeepSleep::new(config, &mock_platform);
        let result = deep_sleep.sleep_for_duration_long(0);

        assert!(result.is_err());
        match result {
            Err(DeepSleepError::InvalidDuration(msg)) => {
                assert!(msg.contains("スリープ時間は0より大きくなければなりません"));
            }
            _ => panic!("Expected InvalidDuration error for zero sleep duration"),
        }
        assert!(!mock_platform.called.load(Ordering::SeqCst)); // 0秒の場合は deep_sleep は呼ばれない
    }

    // 他のテストケースも同様に MockDeepSleepPlatform を使用するように更新が必要
    // 例えば、sleep メソッドのテストなど

    #[test]
    fn test_sleep_fixed_interval() {
        let config = setup_test_config(); // sleep_duration_seconds = 60
        let mock_platform = MockDeepSleepPlatform::new();
        let deep_sleep = DeepSleep::new(Arc::clone(&config), &mock_platform);

        let elapsed_time = Duration::from_secs(10);
        let min_sleep = Duration::from_secs(1);

        // この呼び出しは deep_sleep を行うため、通常は戻らない
        let _ = deep_sleep.sleep(elapsed_time, min_sleep);

        assert!(mock_platform.called.load(Ordering::SeqCst));
        // 60 (config) - 10 (elapsed) = 50 seconds
        assert_eq!(
            *mock_platform.last_duration_us.lock().unwrap(), // Lock and get value
            50 * 1_000_000
        );
    }

    #[test]
    fn test_sleep_fixed_interval_elapsed_exceeds_interval() {
        let mut cfg_values = (*setup_test_config()).clone();
        cfg_values.sleep_duration_seconds = 30;
        let config = Arc::new(cfg_values);

        let mock_platform = MockDeepSleepPlatform::new();
        let deep_sleep = DeepSleep::new(config, &mock_platform);

        let elapsed_time = Duration::from_secs(40); // 経過時間がインターバルより長い
        let min_sleep = Duration::from_secs(5); // 最小スリープ時間

        let _ = deep_sleep.sleep(elapsed_time, min_sleep);

        assert!(mock_platform.called.load(Ordering::SeqCst));
        // 経過時間がインターバルを超えたので、min_sleep (5秒) が使われる
        assert_eq!(
            *mock_platform.last_duration_us.lock().unwrap(), // Lock and get value
            5 * 1_000_000
        );
    }

    // sleep_until_target_digits_match のテストはより複雑で、
    // 時刻のモックや Chrono との連携が必要になります。
    // ここでは、DeepSleepPlatform のモックが使われることを示す基本的な構造のみ示します。
    #[test]
    #[ignore] // このテストは時刻の扱いや find_next_target_datetime_from_start の詳細なモックが必要
    fn test_sleep_until_target_digits_mode() {
        let mut cfg_values = (*setup_test_config()).clone();
        cfg_values.target_minute_last_digit = Some(5); // 例: 毎時 X5 分
        cfg_values.target_second_last_digit = Some(0); // 例: 毎分 0X 秒
        let config = Arc::new(cfg_values);

        let mock_platform = MockDeepSleepPlatform::new();
        let deep_sleep_controller = DeepSleep::new(config, &mock_platform);

        let elapsed_time = Duration::from_secs(2);
        let min_sleep_duration = Duration::from_secs(1);

        // 現在時刻に依存するため、安定したテストには時刻のモックが必要
        // ここでは、呼び出しが行われることの確認に留める (実際にはより詳細な検証が必要)
        let _ = deep_sleep_controller.sleep(elapsed_time, min_sleep_duration);

        // 実際の呼び出しとdurationの検証は、find_next_target_datetime_from_start の挙動と
        // 現在時刻に大きく依存するため、ここでは called フラグのみ確認
        assert!(mock_platform.called.load(Ordering::SeqCst));
        // LAST_SLEEP_DURATION_US の値は実行タイミングによって変わるため、ここでは検証しない
    }
}
