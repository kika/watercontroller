use std::thread;
use std::time::Duration;

#[cfg(feature = "ethernet")]
use std::net::Ipv4Addr;
#[cfg(feature = "ethernet")]
use std::sync::mpsc::{self, Receiver, TryRecvError};

#[cfg(feature = "display")]
use embedded_graphics::{
  mono_font::{MonoTextStyle, ascii::FONT_10X20},
  pixelcolor::BinaryColor,
  prelude::*,
  text::Text,
};
#[cfg(feature = "display")]
use esp_idf_svc::hal::spi::{
  SpiDeviceDriver, SpiDriver, SpiDriverConfig,
  config::{Config as SpiConfig, BitOrder},
};

#[cfg(feature = "ethernet")]
use esp_idf_svc::eth::{
  EspEth, EthDriver, EthEvent, RmiiClockConfig, RmiiEthChipset,
};
#[cfg(feature = "ethernet")]
use esp_idf_svc::ipv4::{self, ClientConfiguration, DHCPClientSettings};
#[cfg(feature = "ethernet")]
use esp_idf_svc::netif::{EspNetif, IpEvent, NetifConfiguration};

use esp_idf_svc::eventloop::EspSystemEventLoop;
#[cfg(feature = "display")]
use esp_idf_svc::hal::gpio::PinDriver;
#[cfg(feature = "ethernet")]
use esp_idf_svc::hal::gpio::{AnyIOPin, Gpio0, Gpio16, Gpio17};
#[cfg(all(feature = "radar", not(feature = "ethernet")))]
use esp_idf_svc::hal::gpio::AnyIOPin;
use esp_idf_svc::hal::prelude::*;
#[cfg(feature = "radar")]
use esp_idf_svc::hal::uart::{self, UartDriver};
use esp_idf_svc::log::EspLogger;
use log::*;

#[cfg(feature = "display")]
use watercontroller::ls027b7dh01::Ls027b7dh01;
#[cfg(feature = "radar")]
use watercontroller::sen0676::{DEFAULT_ADDRESS, Sen0676};

/// Network events communicated from event callbacks to main loop
#[cfg(feature = "ethernet")]
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
  let app_name = env!("CARGO_PKG_NAME");
  esp_idf_svc::log::set_target_level(app_name, log::LevelFilter::Debug).unwrap();
  log::set_max_level(log::LevelFilter::Debug);

  info!("----------------------------------------");
  info!("Water controller v{}", env!("CARGO_PKG_VERSION"));
  info!("Written by Kirill Pertsev kika@kikap.com in 2026");
  debug!("Debug output enabled");

  // Log enabled features
  #[cfg(feature = "display")]
  info!("Feature enabled: display");
  #[cfg(feature = "ethernet")]
  info!("Feature enabled: ethernet");
  #[cfg(feature = "radar")]
  info!("Feature enabled: radar");

  let peripherals = Peripherals::take()?;
  let sysloop = EspSystemEventLoop::take()?;

  // ============================================================
  // Display initialization (feature: display) - hardware SPI
  // ============================================================
  #[cfg(feature = "display")]
  let mut display = {
    // CS: GPIO5, SCLK: GPIO18, MOSI: GPIO23 (VSPI)
    info!("Initializing Sharp Memory Display (hardware SPI)...");

    // Create SPI driver (VSPI = SPI2)
    let spi_driver = SpiDriver::new(
      peripherals.spi2,
      peripherals.pins.gpio18, // SCLK
      peripherals.pins.gpio23, // MOSI
      Option::<esp_idf_svc::hal::gpio::AnyIOPin>::None, // MISO not used
      &SpiDriverConfig::default(),
    )?;

    // Configure SPI Mode 1 (CPOL=0, CPHA=1) and LSB-first
    let spi_config = SpiConfig::default()
      .baudrate(1.MHz().into())
      .data_mode(esp_idf_svc::hal::spi::config::MODE_1)
      .write_only(true)
      .bit_order(BitOrder::LsbFirst);

    let spi_device = SpiDeviceDriver::new(spi_driver, Option::<esp_idf_svc::hal::gpio::AnyIOPin>::None, &spi_config)?;

    // CS is manually controlled (active HIGH for this display)
    let cs_pin = PinDriver::output(peripherals.pins.gpio5)?;

    let mut display = Ls027b7dh01::new(spi_device, cs_pin);
    display.init()?;
    info!("Display cleared");

    // Test: fill with solid black
    info!("Filling display with black...");
    display.fill_black()?;
    info!("Display should be all black now");
    std::thread::sleep(std::time::Duration::from_secs(2));

    // Now clear to white
    info!("Clearing to white...");
    display.clear_display()?;
    info!("Display should be all white now");
    std::thread::sleep(std::time::Duration::from_secs(2));

    // Draw initial text
    info!("Drawing text...");
    let style = MonoTextStyle::new(&FONT_10X20, BinaryColor::Off);
    Text::new("Water Controller", Point::new(10, 30), style).draw(&mut display)?;
    Text::new("Initializing...", Point::new(10, 60), style).draw(&mut display)?;
    display.flush()?;
    info!("Display initialized");

    display
  };

  // ============================================================
  // Ethernet initialization (feature: ethernet)
  // ============================================================
  #[cfg(feature = "ethernet")]
  let (rx, _eth, _eth_subscription, _ip_subscription) = {
    // RTL8201 PHY for wESP32 rev7+
    // Pin mapping:
    //   MDC: GPIO16, MDIO: GPIO17, Clock: GPIO0 (input from PHY), PHY Address: 0
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
      None::<AnyIOPin>, // No reset pin
      RmiiEthChipset::RTL8201,
      Some(0), // PHY address
      sysloop.clone(),
    )?;

    let netif_config = NetifConfiguration {
      ip_configuration: Some(ipv4::Configuration::Client(
        ClientConfiguration::DHCP(DHCPClientSettings {
          hostname: Some("watercontroller".try_into().unwrap()),
        }),
      )),
      ..NetifConfiguration::eth_default_client()
    };

    let mut eth =
      EspEth::wrap_all(eth_driver, EspNetif::new_with_conf(&netif_config)?)?;
    info!("Ethernet driver initialized");

    // Set up event channel
    let (tx, rx) = mpsc::channel::<NetEvent>();

    // Subscribe to Ethernet events (link up/down)
    let tx_eth = tx.clone();
    let eth_subscription = sysloop.subscribe::<EthEvent, _>(move |event| {
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
    let ip_subscription =
      sysloop.subscribe::<IpEvent, _>(move |event| match event {
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
      })?;

    // Start ethernet
    info!("Starting Ethernet...");
    eth.start()?;

    // Wait for initial network connection
    info!("Waiting for network...");
    let (ip, gateway) = wait_for_network(&rx)?;
    info!("Network ready!");
    info!("  IP address: {}", ip);
    info!("  Gateway: {}", gateway);

    (rx, eth, eth_subscription, ip_subscription)
  };

  // ============================================================
  // Radar sensor initialization (feature: radar)
  // ============================================================
  #[cfg(feature = "radar")]
  let mut radar = {
    // TX: GPIO12, RX: GPIO13, 115200 baud, 8N1
    info!("Initializing UART1 for radar sensor...");
    let uart_config = uart::config::Config::default().baudrate(Hertz(115200));
    let uart = UartDriver::new(
      peripherals.uart1,
      peripherals.pins.gpio12, // TX
      peripherals.pins.gpio13, // RX
      Option::<AnyIOPin>::None,
      Option::<AnyIOPin>::None,
      &uart_config,
    )?;

    let mut radar = Sen0676::new(uart, DEFAULT_ADDRESS);
    info!("Radar sensor initialized, draining boot messages...");
    radar.drain_ascii_messages();
    info!("Radar ready");

    radar
  };

  // ============================================================
  // Main loop
  // ============================================================
  info!("Entering main loop...");

  loop {
    // Check for network events (non-blocking)
    #[cfg(feature = "ethernet")]
    match rx.try_recv() {
      Ok(NetEvent::LinkDown) | Ok(NetEvent::LostIp) => {
        warn!("Network lost, waiting for reconnection...");
        let (ip, gateway) = wait_for_network(&rx)?;
        info!("Network restored!");
        info!("  IP address: {}", ip);
        info!("  Gateway: {}", gateway);
      }
      Ok(NetEvent::GotIp { ip, gateway }) => {
        info!("IP address changed: {} (gateway: {})", ip, gateway);
      }
      Ok(NetEvent::LinkUp) => {
        info!("Ethernet link restored");
      }
      Err(TryRecvError::Empty) => {}
      Err(TryRecvError::Disconnected) => {
        anyhow::bail!("Event channel disconnected");
      }
    }

    // Read radar sensor
    #[cfg(feature = "radar")]
    match radar.read_empty_height() {
      Ok(height) => info!("Empty height: {} mm", height),
      Err(e) => warn!("Radar read error: {:?}", e),
    }

    // Toggle VCOM (required for Sharp Memory LCD - at least once per second)
    #[cfg(feature = "display")]
    display.toggle_vcom()?;

    thread::sleep(Duration::from_secs(1));
  }
}

/// Blocks until we have both link up and an IP address
#[cfg(feature = "ethernet")]
fn wait_for_network(
  rx: &Receiver<NetEvent>,
) -> anyhow::Result<(Ipv4Addr, Ipv4Addr)> {
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
        error!("Got IP but waiting for link...");
      }
      NetEvent::LostIp => {
        info!("Lost IP, continuing to wait...");
      }
    }
  }
}
