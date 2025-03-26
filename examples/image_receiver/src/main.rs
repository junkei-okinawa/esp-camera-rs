use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::{delay::FreeRtos, peripherals::Peripherals};
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::wifi::{EspWifi, BlockingWifi};
use esp_idf_sys::{
    esp_now_init, esp_now_recv_info_t, esp_now_register_recv_cb,
};
use log::{error, info};
use std::slice;

// 受信コールバック関数
extern "C" fn esp_now_recv_cb(info: *const esp_now_recv_info_t, data: *const u8, data_len: i32) {
    if info.is_null() || data.is_null() || data_len <= 0 {
        error!("ESP-NOW: Received invalid data");
        return;
    }

    let info = unsafe { &*info };
    let src_addr = unsafe { slice::from_raw_parts(info.src_addr, 6) }; // ESP_NOW_ETH_ALEN は 6
    let data_slice = unsafe { slice::from_raw_parts(data, data_len as usize) };

    info!(
        "ESP-NOW: Received {} bytes from {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        data_len,
        src_addr[0], src_addr[1], src_addr[2], src_addr[3], src_addr[4], src_addr[5]
    );

    // 受信データの一部を表示（例として最初の10バイト）
    let display_len = std::cmp::min(data_slice.len(), 10);
    info!("ESP-NOW: Data sample: {:?}", &data_slice[..display_len]);

    // TODO: ここで画像チャンクを結合するロジックを実装
}

fn main() -> anyhow::Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    info!("initializing peripherals");
    let peripherals = Peripherals::take().unwrap();
    let sysloop = EspSystemEventLoop::take()?;
    let nvs = EspDefaultNvsPartition::take()?;

    info!("initializing WiFi peripheral for ESP-NOW");
    let mut wifi = BlockingWifi::wrap(
        EspWifi::new(peripherals.modem, sysloop.clone(), Some(nvs))?,
        sysloop,
    )?;
    wifi.start()?;
    info!("WiFi peripheral started for ESP-NOW");

    // 自身のMACアドレスを取得してログ出力（デバッグ用）
    let mac = wifi.wifi().ap_netif().get_mac()?;
    info!("Receiver MAC Address: {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
          mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]);


    info!("Initializing ESP-NOW");
    unsafe {
        esp_now_init();
        // 受信コールバックを登録
        esp_now_register_recv_cb(Some(esp_now_recv_cb));
    }
    info!("ESP-NOW Initialized. Waiting for data...");

    // 受信待機ループ
    loop {
        FreeRtos::delay_ms(1000); // 1秒ごとに待機
    }
}