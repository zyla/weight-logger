use esp_idf_hal::adc::{self, AdcChannelDriver, AdcConfig, AdcDriver, Atten11dB, ADC1};
use esp_idf_hal::gpio::PinDriver;
use std::time::Duration;

use anyhow::anyhow;
use embedded_svc::mqtt::client::QoS;
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

    let mut adc1 = AdcDriver::new(
        peripherals.adc1,
        &AdcConfig::new()
            .calibration(true)
            .resolution(adc::config::Resolution::Resolution12Bit),
    )?;
    let mut vcc_adc_channel = AdcChannelDriver::<_, Atten11dB<ADC1>>::new(peripherals.pins.gpio36)?;
    let mut vbat_adc_channel =
        AdcChannelDriver::<_, Atten11dB<ADC1>>::new(peripherals.pins.gpio33)?;

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

    loop {
        let mut acc_vcc: u32 = 0;
        let mut acc_vbat: u32 = 0;
        let mut acc_value = 0;
        let mut acc_chb = 0;
        const N: u32 = 1;

        hx711.enable().map_err(|_| anyhow!("can't enable HX711"))?;

        for _ in 0..N {
            acc_vcc += adc1.read(&mut vcc_adc_channel)? as u32 * 2;
            acc_vbat += adc1.read(&mut vbat_adc_channel)? as u32 * 2;
            block!(hx711.set_mode(hx711::Mode::ChAGain128))
                .map_err(|_| anyhow!("can't set HX711 mode"))?;
            acc_value += block!(hx711.retrieve()).map_err(|_| anyhow!("can't read from HX711"))?;
            block!(hx711.set_mode(hx711::Mode::ChBGain32))
                .map_err(|_| anyhow!("can't set HX711 mode"))?;
            acc_chb += block!(hx711.retrieve()).map_err(|_| anyhow!("can't read from HX711"))?;

            std::thread::sleep(Duration::from_millis(500));
        }

        hx711
            .disable()
            .map_err(|_| anyhow!("can't disable HX711"))?;

        let vcc = acc_vcc / N;
        let vbat = acc_vbat / N;
        let value = acc_value / (N as i32);
        let chb = acc_chb / (N as i32);

        println!("VCC: {}mV", vcc);
        match mqtt_client.enqueue(
            "waga1/vcc",
            QoS::AtLeastOnce,
            true,
            format!("{vcc}").as_bytes(),
        ) {
            Ok(_) => {}
            Err(e) => {
                error!("publish failed: {e}");
            }
        }

        println!("VBAT: {}mV", vbat);
        match mqtt_client.enqueue(
            "waga1/vbat",
            QoS::AtLeastOnce,
            true,
            format!("{vbat}").as_bytes(),
        ) {
            Ok(_) => {}
            Err(e) => {
                error!("publish failed: {e}");
            }
        }

        println!("Value: {}", value);
        match mqtt_client.enqueue(
            "waga1/value",
            QoS::AtLeastOnce,
            true,
            format!("{value}").as_bytes(),
        ) {
            Ok(_) => {}
            Err(e) => {
                error!("publish failed: {e}");
            }
        }

        println!("Channel B: {}", chb);
        match mqtt_client.enqueue(
            "waga1/chb",
            QoS::AtLeastOnce,
            true,
            format!("{chb}").as_bytes(),
        ) {
            Ok(_) => {}
            Err(e) => {
                error!("publish failed: {e}");
            }
        }

        // std::thread::sleep(Duration::from_secs(10));
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
