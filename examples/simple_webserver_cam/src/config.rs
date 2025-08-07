#[derive(Debug)]
pub struct Config {
    pub wifi_ssid: &'static str,
    pub wifi_psk: &'static str,
}

include!("wifi_config.rs");
