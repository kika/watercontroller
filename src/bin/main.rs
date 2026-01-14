use std::net::Ipv4Addr;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::Duration;

use esp_idf_svc::eth::{EspEth, EthDriver, EthEvent, RmiiClockConfig, RmiiEthChipset};
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::gpio::{Gpio0, Gpio16, Gpio17};
use esp_idf_svc::hal::prelude::Peripherals;
use esp_idf_svc::log::EspLogger;
use esp_idf_svc::ipv4::{self, ClientConfiguration, DHCPClientSettings};
use esp_idf_svc::netif::{EspNetif, IpEvent, NetifConfiguration};
use log::{error, info, warn};

/// Network events communicated from event callbacks to main loop
#[derive(Debug)]
enum NetEvent {
    LinkUp,
    LinkDown,
    GotIp { ip: Ipv4Addr, gateway: Ipv4Addr },
    LostIp,
}

fn main() -> anyhow::Result<()> {
    esp_idf_svc::sys::link_patches();
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
    //   https://wesp32.com/files/wESP32-Product-Brief.pdf
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

    let netif_config = NetifConfiguration {
        ip_configuration: Some(ipv4::Configuration::Client(ClientConfiguration::DHCP(
            DHCPClientSettings {
                hostname: Some("watercontroller".try_into().unwrap()),
            },
        ))),
        ..NetifConfiguration::eth_default_client()
    };

    let mut eth = EspEth::wrap_all(eth_driver, EspNetif::new_with_conf(&netif_config)?)?;

    info!("Ethernet driver initialized");

    // Set up event channel
    let (tx, rx) = mpsc::channel::<NetEvent>();

    // Subscribe to Ethernet events (link up/down)
    let tx_eth = tx.clone();
    let _eth_subscription = sysloop.subscribe::<EthEvent, _>(move |event| {
        let net_event = match event {
            EthEvent::Connected(_) => {
                info!("Event: Ethernet link connected");
                NetEvent::LinkUp
            }
            EthEvent::Disconnected(_) => {
                warn!("Event: Ethernet link disconnected");
                NetEvent::LinkDown
            }
            EthEvent::Started(_) => {
                info!("Event: Ethernet started");
                return;
            }
            EthEvent::Stopped(_) => {
                info!("Event: Ethernet stopped");
                return;
            }
        };
        let _ = tx_eth.send(net_event);
    })?;

    // Subscribe to IP events (DHCP)
    let tx_ip = tx.clone();
    let _ip_subscription = sysloop.subscribe::<IpEvent, _>(move |event| {
        match event {
            IpEvent::DhcpIpAssigned(assignment) => {
                let ip_info = assignment.ip_info();
                info!("Event: DHCP IP assigned - {}", ip_info.ip);
                let _ = tx_ip.send(NetEvent::GotIp {
                    ip: ip_info.ip,
                    gateway: ip_info.subnet.gateway,
                });
            }
            IpEvent::DhcpIpDeassigned(_) => {
                warn!("Event: DHCP IP deassigned");
                let _ = tx_ip.send(NetEvent::LostIp);
            }
            _ => {}
        }
    })?;

    // Start ethernet (this will trigger events)
    info!("Starting Ethernet...");
    eth.start()?;

    // Wait for initial network connection
    info!("Waiting for network...");
    let (ip, gateway) = wait_for_network(&rx)?;
    info!("Network ready!");
    info!("  IP address: {}", ip);
    info!("  Gateway: {}", gateway);

    info!("Entering main loop...");

    // Main application loop
    loop {
        // Check for network events (non-blocking)
        match rx.try_recv() {
            Ok(NetEvent::LinkDown) | Ok(NetEvent::LostIp) => {
                warn!("Network lost, waiting for reconnection...");
                let (ip, gateway) = wait_for_network(&rx)?;
                info!("Network restored!");
                info!("  IP address: {}", ip);
                info!("  Gateway: {}", gateway);
            }
            Ok(NetEvent::GotIp { ip, gateway }) => {
                // IP changed (e.g., DHCP renewal with different IP)
                info!("IP address changed: {} (gateway: {})", ip, gateway);
            }
            Ok(NetEvent::LinkUp) => {
                info!("Ethernet link restored");
            }
            Err(TryRecvError::Empty) => {
                // No events, continue main loop
            }
            Err(TryRecvError::Disconnected) => {
                anyhow::bail!("Event channel disconnected");
            }
        }

        info!("Hello world!");
        thread::sleep(Duration::from_secs(1));
    }
}

/// Blocks until we have both link up and an IP address
fn wait_for_network(rx: &Receiver<NetEvent>) -> anyhow::Result<(Ipv4Addr, Ipv4Addr)> {
    let mut link_up = false;

    loop {
        match rx.recv()? {
            NetEvent::LinkUp => {
                info!("Link up, waiting for DHCP...");
                link_up = true;
            }
            NetEvent::LinkDown => {
                warn!("Link down");
                link_up = false;
            }
            NetEvent::GotIp { ip, gateway } if link_up => {
                return Ok((ip, gateway));
            }
            NetEvent::GotIp { .. } => {
                // Got IP but link not up yet (shouldn't happen, but handle it)
                error!("Got IP but waiting for link...");
            }
            NetEvent::LostIp => {
                info!("Lost IP, continuing to wait...");
            }
        }
    }
}
