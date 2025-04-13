// Config struct is no longer needed as we don't read from cfg.toml for SSID/PSK

fn main() {
    // No need to check cfg.toml for SSID/PSK anymore for ESP-NOW receiver.
    // We still need embuild for other environment variables if required by esp-idf-sys.
    // Make App_config available as a system environment variable.
    embuild::espidf::sysenv::output();
}
