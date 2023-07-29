use esp_idf_hal::gpio::PinDriver;
use std::time::Duration;

use anyhow::anyhow;
use embedded_svc::mqtt::client::{Publish, QoS};
use embedded_svc::wifi::{AuthMethod, ClientConfiguration, Configuration};
use esp_idf_hal::delay::Delay;
use esp_idf_hal::prelude::*;
use esp_idf_svc::log::EspLogger;
use esp_idf_svc::mqtt::client::{EspMqttClient, MqttClientConfiguration};
use esp_idf_svc::wifi::{BlockingWifi, EspWifi};
use esp_idf_svc::{eventloop::EspSystemEventLoop, nvs::EspDefaultNvsPartition};
use esp_idf_sys::{self as _};
use log::*; // If using the `binstart` feature of `esp-idf-sys`, always keep this module imported
use nb::block;

const SSID: &str = env!("WIFI_SSID");
const PASSWORD: &str = env!("WIFI_PASS");

fn main() {
    match real_main() {
        Ok(_) => {}
        Err(e) => {
            error!("real_main() failed: {:?}", e);
        }
    }
    loop {
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn real_main() -> anyhow::Result<()> {
    // It is necessary to call this function once. Otherwise some patches to the runtime
    // implemented by esp-idf-sys might not link properly. See https://github.com/esp-rs/esp-idf-template/issues/71
    esp_idf_sys::link_patches();
    // Bind the log crate to the ESP Logging facilities
    EspLogger::initialize_default();

    unsafe {
        info!("WDT deinit = {}", esp_idf_sys::esp_task_wdt_deinit());
    }

    let peripherals = Peripherals::take().unwrap();

    let dout = PinDriver::input(peripherals.pins.gpio19)?;
    let pd_sck = PinDriver::output(peripherals.pins.gpio23)?;
    let mut hx711 =
        hx711::Hx711::new(Delay, dout, pd_sck).map_err(|_| anyhow!("can't initialize HX711"))?;
    info!("initialized HX711");
    let sys_loop = EspSystemEventLoop::take()?;
    let nvs = EspDefaultNvsPartition::take()?;

    let mut wifi = BlockingWifi::wrap(
        EspWifi::new(peripherals.modem, sys_loop.clone(), Some(nvs))?,
        sys_loop,
    )?;

    connect_wifi(&mut wifi)?;

    let ip_info = wifi.wifi().sta_netif().get_ip_info()?;

    info!("Wifi DHCP info: {:?}", ip_info);

    let broker_url = format!("mqtt://192.168.1.12");

    let mut mqtt_client =
        EspMqttClient::new(broker_url, &MqttClientConfiguration::default(), |_| {})?;

    let topic = "waga1";

    loop {
        let reading = block!(hx711.retrieve()).map_err(|_| anyhow!("can't read from HX711"))?;
        let value = (reading + 403300) / 20;
        println!("Weight: {}g", value);
        mqtt_client.publish(
            topic,
            QoS::AtLeastOnce,
            false,
            format!("{value}").as_bytes(),
        );
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn connect_wifi(wifi: &mut BlockingWifi<EspWifi<'static>>) -> anyhow::Result<()> {
    let wifi_configuration: Configuration = Configuration::Client(ClientConfiguration {
        ssid: SSID.into(),
        bssid: None,
        auth_method: AuthMethod::WPA2Personal,
        password: PASSWORD.into(),
        channel: None,
    });

    wifi.set_configuration(&wifi_configuration)?;

    wifi.start()?;
    info!("Wifi started");

    wifi.connect()?;
    info!("Wifi connected");

    wifi.wait_netif_up()?;
    info!("Wifi netif up");

    Ok(())
}
