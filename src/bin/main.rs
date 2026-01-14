use std::thread;
use std::time::Duration;

use esp_idf_svc::eth::{BlockingEth, EspEth, EthDriver, RmiiClockConfig, RmiiEthChipset};
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::gpio::{Gpio0, Gpio16, Gpio17};
use esp_idf_svc::hal::prelude::Peripherals;
use esp_idf_svc::log::EspLogger;
use esp_idf_svc::netif::{EspNetif, NetifConfiguration};
use log::info;

fn main() -> anyhow::Result<()> {
    // It is necessary to call this function once. Otherwise some patches to the runtime
    // implemented by esp-idf-sys might not link properly.
    esp_idf_svc::sys::link_patches();

    // Bind the log crate to the ESP Logging facilities
    EspLogger::initialize_default();

    info!("Water controller v{}", env!("CARGO_PKG_VERSION"));
    info!("Written by Kirill Pertsev kika@kikap.com in 2026");

    let peripherals = Peripherals::take()?;
    let sysloop = EspSystemEventLoop::take()?;

    // Initialize Ethernet with RTL8201 PHY for wESP32 rev7+
    // Pin mapping:
    //   MDC: GPIO16
    //   MDIO: GPIO17
    //   Clock: GPIO0 (input from PHY)
    //   PHY Address: 0
    info!("Initializing Ethernet (RTL8201 PHY)...");

    let eth_driver = EthDriver::new_rmii(
        peripherals.mac,
        peripherals.pins.gpio25, // RXD0
        peripherals.pins.gpio26, // RXD1
        peripherals.pins.gpio27, // CRS_DV
        peripherals.pins.gpio16, // MDC
        peripherals.pins.gpio22, // TXD1
        peripherals.pins.gpio21, // TX_EN
        peripherals.pins.gpio19, // TXD0
        peripherals.pins.gpio17, // MDIO
        RmiiClockConfig::<Gpio0, Gpio16, Gpio17>::Input(peripherals.pins.gpio0),
        None::<esp_idf_svc::hal::gpio::Gpio5>, // No reset pin
        RmiiEthChipset::RTL8201,
        Some(0), // PHY address
        sysloop.clone(),
    )?;

    let eth = EspEth::wrap_all(
        eth_driver,
        EspNetif::new_with_conf(&NetifConfiguration::eth_default_client())?,
    )?;

    info!("Ethernet driver initialized, starting...");

    let mut eth = BlockingEth::wrap(eth, sysloop.clone())?;
    eth.start()?;

    info!("Waiting for Ethernet link...");
    eth.wait_netif_up()?;

    // DHCP request loop - wait for IP address
    info!("Ethernet link up, waiting for DHCP lease...");
    loop {
        let ip_info = eth.eth().netif().get_ip_info()?;

        if !ip_info.ip.is_unspecified() {
            info!("DHCP lease acquired!");
            info!("  IP address: {}", ip_info.ip);
            info!("  Subnet mask: {}", ip_info.subnet.mask);
            info!("  Gateway: {:?}", ip_info.subnet.gateway);
            break;
        }

        thread::sleep(Duration::from_millis(500));
    }

    info!("Network ready, entering main loop...");

    // Main application loop
    loop {
        info!("Hello world!");
        thread::sleep(Duration::from_secs(1));
    }
}
