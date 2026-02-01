use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

#[cfg(feature = "ethernet")]
use std::net::Ipv4Addr;
#[cfg(feature = "ethernet")]
use std::sync::mpsc::{self, Receiver};

#[cfg(feature = "display")]
use embedded_graphics::geometry::{Point, Size};
#[cfg(feature = "display")]
use embedded_graphics::{
  Drawable,
  mono_font::{MonoTextStyleBuilder, ascii::FONT_10X20},
  pixelcolor::BinaryColor,
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
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use log::*;

#[cfg(feature = "display")]
use watercontroller::ls027b7dh01::Ls027b7dh01;
#[cfg(feature = "display")]
use watercontroller::ui::{WaterTank, Manometer};
#[cfg(feature = "radar")]
use watercontroller::sen0676::{DEFAULT_ADDRESS, Sen0676};
#[cfg(feature = "pressure")]
use watercontroller::pressure::PressureSensor;
#[cfg(feature = "mqtt")]
use watercontroller::homeassistant::{ConfigCommand, HomeAssistant, WaterState};
use watercontroller::config::Config;
#[cfg(feature = "ethernet")]
use watercontroller::web::WebServer;

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

  let result = run();

  if let Err(ref e) = result {
    error!("Fatal error: {:#}", e);
  }

  result
}

fn run() -> anyhow::Result<()> {
  // Log enabled features
  #[cfg(feature = "display")]
  info!("Feature enabled: display");
  #[cfg(feature = "ethernet")]
  info!("Feature enabled: ethernet");
  #[cfg(feature = "radar")]
  info!("Feature enabled: radar");
  #[cfg(feature = "pressure")]
  info!("Feature enabled: pressure");
  #[cfg(feature = "mqtt")]
  info!("Feature enabled: mqtt");

  let peripherals = Peripherals::take()?;
  let sysloop = EspSystemEventLoop::take()?;

  // ============================================================
  // NVS configuration
  // ============================================================
  let nvs_partition = EspDefaultNvsPartition::take()?;
  let config = Arc::new(Mutex::new(Config::load(nvs_partition)?));

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
      .baudrate(2.MHz().into())
      .data_mode(esp_idf_svc::hal::spi::config::MODE_1)
      .write_only(true)
      .bit_order(BitOrder::LsbFirst);

    let spi_device = SpiDeviceDriver::new(spi_driver, Option::<esp_idf_svc::hal::gpio::AnyIOPin>::None, &spi_config)?;

    // CS is manually controlled (active HIGH for this display)
    let cs_pin = PinDriver::output(peripherals.pins.gpio5)?;

    let mut display = Ls027b7dh01::new(spi_device, cs_pin);
    display.init()?;
    info!("Display initialized");

    display
  };

  // Create UI components
  #[cfg(feature = "display")]
  let mut tank = WaterTank::new(Point::new(20, 20), Size::new(120, 200));

  #[cfg(feature = "display")]
  let mut manometer = Manometer::new(Point::new(280, 120), 100);

  // Boot status display helper
  #[cfg(feature = "display")]
  let boot_text_style = MonoTextStyleBuilder::new()
    .font(&FONT_10X20)
    .text_color(BinaryColor::Off)
    .build();

  #[cfg(feature = "display")]
  let mut boot_line = 0i32;

  /// Show a boot progress line on the display, appending each new line
  macro_rules! boot_status {
    ($($arg:tt)*) => {
      #[cfg(feature = "display")]
      {
        use core::fmt::Write;
        let mut buf = [0u8; 40];
        let mut w = LineBuf::new(&mut buf);
        let _ = write!(w, $($arg)*);
        let len = w.pos;
        if boot_line == 0 {
          display.clear_framebuffer();
        }
        let y = 26 + boot_line * 26;
        Text::new(
          unsafe { core::str::from_utf8_unchecked(&buf[..len]) },
          Point::new(10, y as i32),
          boot_text_style,
        ).draw(&mut display).ok();
        display.flush().ok();
        boot_line += 1;
      }
    };
  }

  boot_status!("Water Controller v{}", env!("CARGO_PKG_VERSION"));

  // From here on, errors can be shown on the display.
  // Wrap the rest in a closure so we can catch errors.
  let result: anyhow::Result<()> = (|| {

  // ============================================================
  // Ethernet initialization (feature: ethernet)
  // ============================================================
  #[cfg(feature = "ethernet")]
  let (rx, _ip_addr, _eth, _eth_subscription, _ip_subscription) = {
    // RTL8201 PHY for wESP32 rev7+
    // Pin mapping:
    //   MDC: GPIO16, MDIO: GPIO17, Clock: GPIO0 (input from PHY), PHY Address: 0
    //   https://wesp32.com/files/wESP32-Product-Brief.pdf
    boot_status!("Ethernet...");
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
    boot_status!("Waiting for DHCP...");
    info!("Waiting for network...");
    let (ip, gateway) = wait_for_network(&rx)?;
    boot_status!("IP: {}", ip);
    info!("Network ready!");
    info!("  IP address: {}", ip);
    info!("  Gateway: {}", gateway);

    // Log DNS servers received from DHCP
    let dns1 = eth.netif().get_dns();
    let dns2 = eth.netif().get_secondary_dns();
    info!("  DNS primary: {}", dns1);
    info!("  DNS secondary: {}", dns2);

    (rx, ip, eth, eth_subscription, ip_subscription)
  };

  // ============================================================
  // Radar sensor initialization (feature: radar)
  // ============================================================
  #[cfg(feature = "radar")]
  let mut radar = {
    // TX: GPIO12, RX: GPIO13, 115200 baud, 8N1
    boot_status!("Radar sensor...");
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

    let radar = Sen0676::new(uart, DEFAULT_ADDRESS);
    info!("Radar sensor initialized");

    radar
  };

  // ============================================================
  // Pressure sensor initialization (feature: pressure)
  // ============================================================
  #[cfg(feature = "pressure")]
  let mut pressure_sensor = {
    // GPIO36 (A0) with 10k/12k voltage divider
    // Sensor: 0.5V = 0 PSI, 4.5V = 100 PSI
    boot_status!("Pressure sensor...");
    info!("Initializing pressure sensor on GPIO36...");
    let sensor = PressureSensor::new(peripherals.adc1, peripherals.pins.gpio36)?;
    info!("Pressure sensor ready");
    sensor
  };

  // ============================================================
  // Web server (feature: ethernet) — always available for config
  // ============================================================
  boot_status!("Web server...");
  #[cfg(feature = "ethernet")]
  let _web_server = WebServer::start(config.clone())?;

  // ============================================================
  // MQTT / Home Assistant initialization (feature: mqtt)
  // ============================================================
  #[cfg(feature = "mqtt")]
  let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<ConfigCommand>();

  #[cfg(feature = "mqtt")]
  let mqtt_configured = config.lock().unwrap().mqtt_configured();

  #[cfg(feature = "mqtt")]
  let mut ha_client: Option<HomeAssistant> = if mqtt_configured {
    let (broker, port, username, password) = {
      let cfg = config.lock().unwrap();
      (cfg.mqtt_broker.clone(), cfg.mqtt_port, cfg.mqtt_username.clone(), cfg.mqtt_password.clone())
    };

    // Verify DNS resolution before attempting MQTT connection
    {
      use std::net::ToSocketAddrs;

      boot_status!("DNS: {}...", broker);
      info!("Resolving {}...", broker);
      let mut resolved = false;
      for attempt in 1..=5 {
        match (broker.as_str(), port).to_socket_addrs() {
          Ok(addrs) => {
            let addrs: Vec<_> = addrs.collect();
            info!("DNS resolved {} -> {:?}", broker, addrs);
            resolved = true;
            break;
          }
          Err(e) => {
            warn!("DNS attempt {}/5 failed: {}", attempt, e);
            thread::sleep(Duration::from_secs(2));
          }
        }
      }
      if !resolved {
        anyhow::bail!("DNS: can't resolve {}", broker);
      }
    }

    boot_status!("MQTT connecting...");
    info!("Initializing MQTT client for Home Assistant...");
    let mut client = HomeAssistant::new(&broker, port, &username, &password, cmd_tx)
      .map_err(|e| anyhow::anyhow!("MQTT init failed: {}", e))?;
    // Give MQTT time to connect before sending discovery
    thread::sleep(Duration::from_secs(2));
    // Check if connection failed during the wait
    if let Some(err) = client.connection_error() {
      anyhow::bail!("MQTT: {}", err);
    }
    client.send_discovery()
      .map_err(|e| anyhow::anyhow!("MQTT discovery failed: {}", e))?;
    client.subscribe()
      .map_err(|e| anyhow::anyhow!("MQTT subscribe failed: {}", e))?;
    info!("Home Assistant MQTT ready");
    Some(client)
  } else {
    boot_status!("Setup: http://{}/", _ip_addr);
    info!("MQTT not configured — visit http://{}/", _ip_addr);
    None
  };

  // ============================================================
  // Finalize boot screen with MQTT status + config parameters
  // ============================================================
  #[cfg(feature = "mqtt")]
  if ha_client.is_some() {
    boot_status!("MQTT: connected");
  }

  {
    let cfg = config.lock().unwrap();
    boot_status!("Tank:{} gal  H:{} ft", cfg.tank_capacity_gallons, cfg.sensor_height_feet);
    boot_status!("PSI:{}  Radar:{} cm", cfg.max_psi, cfg.radar_height_cm);
  }

  // Keep boot screen visible for 2 seconds before switching to normal display
  #[cfg(feature = "display")]
  let mut info_until: Option<std::time::Instant> =
    Some(std::time::Instant::now() + Duration::from_secs(2));

  // ============================================================
  // Main loop
  // ============================================================
  info!("Entering main loop...");

  #[cfg(feature = "ethernet")]
  let mut network_up = true;

  // Demo values (will be replaced with real sensor data)
  #[cfg(any(feature = "display", feature = "mqtt"))]
  let mut demo_percent: u8 = 0;
  #[cfg(all(any(feature = "display", feature = "mqtt"), not(feature = "pressure")))]
  let mut demo_psi: u16 = 0;
  #[cfg(any(feature = "display", feature = "mqtt"))]
  let mut demo_rising = true;

  // MQTT publish interval
  #[cfg(feature = "mqtt")]
  const MQTT_INTERVAL: Duration = Duration::from_secs(5);
  #[cfg(feature = "mqtt")]
  let mut last_mqtt_publish = std::time::Instant::now();

  loop {
    // Check for network events (non-blocking)
    #[cfg(feature = "ethernet")]
    while let Ok(event) = rx.try_recv() {
      match event {
        NetEvent::LinkDown => {
          warn!("Ethernet link lost");
          network_up = false;
          #[cfg(feature = "display")]
          {
            display.clear_framebuffer();
            Text::new("Ethernet disconnected", Point::new(10, 120), boot_text_style)
              .draw(&mut display).ok();
            display.flush().ok();
            info_until = Some(std::time::Instant::now() + Duration::from_secs(3600));
          }
        }
        NetEvent::LostIp => {
          warn!("IP address lost");
          network_up = false;
          #[cfg(feature = "display")]
          {
            display.clear_framebuffer();
            Text::new("Waiting for DHCP...", Point::new(10, 120), boot_text_style)
              .draw(&mut display).ok();
            display.flush().ok();
            info_until = Some(std::time::Instant::now() + Duration::from_secs(3600));
          }
        }
        NetEvent::LinkUp => {
          info!("Ethernet link restored");
        }
        NetEvent::GotIp { ip, gateway } => {
          info!("Network restored: {} (gateway: {})", ip, gateway);
          network_up = true;
          #[cfg(feature = "display")]
          {
            // Clear overlay so normal display resumes
            info_until = None;
            display.clear_framebuffer();
            display.mark_all_dirty();
          }
        }
      }
    }

    // Process MQTT configuration commands
    #[cfg(feature = "mqtt")]
    if ha_client.is_some() {
      while let Ok(cmd) = cmd_rx.try_recv() {
        let msg: Option<&str> = {
          let mut cfg = config.lock().unwrap();
          match cmd {
            ConfigCommand::SetTankCapacity(val) => {
              if let Err(e) = cfg.set_tank_capacity(val) {
                warn!("Failed to set tank capacity: {:?}", e);
              }
              Some("Tank Capacity")
            }
            ConfigCommand::SetSensorHeight(val) => {
              if let Err(e) = cfg.set_sensor_height(val) {
                warn!("Failed to set sensor height: {:?}", e);
              }
              Some("Sensor Height")
            }
            ConfigCommand::SetMaxPsi(val) => {
              if let Err(e) = cfg.set_max_psi(val) {
                warn!("Failed to set max PSI: {:?}", e);
              }
              Some("Max PSI")
            }
            ConfigCommand::SetRadarHeight(val) => {
              if let Err(e) = cfg.set_radar_height(val) {
                warn!("Failed to set radar height: {:?}", e);
              }
              Some("Radar Height")
            }
          }
        };

        // Show config change on display
        #[cfg(feature = "display")]
        if let Some(label) = msg {
          use core::fmt::Write;

          let text_style = MonoTextStyleBuilder::new()
            .font(&FONT_10X20)
            .text_color(BinaryColor::Off)
            .build();

          display.clear_framebuffer();

          let cfg = config.lock().unwrap();
          let mut line_buf = [0u8; 40];
          let value = match label {
            "Tank Capacity" => cfg.tank_capacity_gallons,
            "Sensor Height" => cfg.sensor_height_feet,
            "Max PSI" => cfg.max_psi,
            "Radar Height" => cfg.radar_height_cm,
            _ => 0,
          };
          let unit = match label {
            "Tank Capacity" => " gal",
            "Sensor Height" => " ft",
            "Radar Height" => " cm",
            _ => "",
          };
          let mut w = LineBuf::new(&mut line_buf);
          let _ = write!(w, "{}: {}{}", label, value, unit);
          let len = w.pos;
          Text::new(
            unsafe { core::str::from_utf8_unchecked(&line_buf[..len]) },
            Point::new(10, 120),
            text_style,
          ).draw(&mut display)?;

          display.flush()?;
          info_until = Some(std::time::Instant::now() + Duration::from_secs(2));
        }
        #[cfg(not(feature = "display"))]
        let _ = msg;
      }
    }

    // Read radar sensor
    #[cfg(feature = "radar")]
    match radar.read_empty_height() {
      Ok(height) => info!("Empty height: {} mm", height),
      Err(e) => warn!("Radar read error: {:?}", e),
    }

    // Read pressure sensor
    #[cfg(feature = "pressure")]
    let current_psi = match pressure_sensor.read_psi_u16(config.lock().unwrap().sensor_height_feet as f32) {
      Ok(psi) => {
        debug!("Pressure: {} PSI", psi);
        psi
      }
      Err(e) => {
        warn!("Pressure read error: {:?}", e);
        0
      }
    };
    #[cfg(not(feature = "pressure"))]
    let current_psi: u16 = {
      #[cfg(any(feature = "display", feature = "mqtt"))]
      {
        if demo_rising {
          demo_psi = demo_psi.saturating_add(8);
        } else {
          demo_psi = demo_psi.saturating_sub(8);
        }
        demo_psi.min(config.lock().unwrap().max_psi)
      }
      #[cfg(not(any(feature = "display", feature = "mqtt")))]
      0
    };

    // Demo animation for tank (will be replaced with radar data)
    #[cfg(any(feature = "display", feature = "mqtt"))]
    {
      if demo_rising {
        demo_percent = demo_percent.saturating_add(5);
        if demo_percent >= 100 {
          demo_rising = false;
        }
      } else {
        demo_percent = demo_percent.saturating_sub(5);
        if demo_percent == 0 {
          demo_rising = true;
        }
      }
    }

    // Calculate gallons from config tank capacity
    #[cfg(any(feature = "display", feature = "mqtt"))]
    let gallons = {
      let cfg = config.lock().unwrap();
      (cfg.tank_capacity_gallons as u32 * demo_percent as u32 / 100) as u16
    };

    // Publish to Home Assistant via MQTT (skip when network is down)
    #[cfg(feature = "mqtt")]
    if let Some(ref mut client) = ha_client {
      #[cfg(feature = "ethernet")]
      let can_publish = network_up;
      #[cfg(not(feature = "ethernet"))]
      let can_publish = true;
      if can_publish && last_mqtt_publish.elapsed() >= MQTT_INTERVAL {
        last_mqtt_publish = std::time::Instant::now();
        let cfg = config.lock().unwrap();
        let state = WaterState {
          capacity_percent: demo_percent,
          capacity_gallons: gallons,
          pressure_psi: current_psi,
          tank_capacity: cfg.tank_capacity_gallons,
          sensor_height: cfg.sensor_height_feet,
          max_psi: cfg.max_psi,
          radar_height: cfg.radar_height_cm,
        };
        drop(cfg);
        if let Err(e) = client.publish_state(&state) {
          warn!("MQTT publish error: {:?}", e);
        }
      }
    }

    // Update display
    #[cfg(feature = "display")]
    {
      // Check if info overlay is active
      let showing_info = match info_until {
        Some(until) if std::time::Instant::now() < until => true,
        Some(_) => {
          // Info expired, clear and resume normal display
          info_until = None;
          display.clear_framebuffer();
          display.mark_all_dirty();
          false
        }
        None => false,
      };

      if !showing_info {
        let max_psi = config.lock().unwrap().max_psi;

        // Update UI component values
        tank.set_level(demo_percent, gallons);
        manometer.set_pressure(current_psi.min(max_psi));

        // Draw UI (components clear their own areas)
        tank.draw(&mut display)?;
        manometer.draw(&mut display)?;
        display.flush()?;
      }
    }

    thread::sleep(Duration::from_millis(200));
  }

  })(); // end of error-catching closure

  // Show fatal error on display if available
  #[cfg(feature = "display")]
  if let Err(ref e) = result {
    use core::fmt::Write;

    let text_style = MonoTextStyleBuilder::new()
      .font(&FONT_10X20)
      .text_color(BinaryColor::Off)
      .build();

    display.clear_framebuffer();

    Text::new("FATAL ERROR", Point::new(10, 30), text_style)
      .draw(&mut display).ok();

    // Format error message, truncated to fit display
    let mut line_buf = [0u8; 60];
    let mut w = LineBuf::new(&mut line_buf);
    let _ = write!(w, "{:#}", e);
    let len = w.pos;
    let msg = unsafe { core::str::from_utf8_unchecked(&line_buf[..len]) };

    // Split long messages across lines
    let mut y = 70i32;
    let chars_per_line = 38; // 400px / 10px per char ≈ 38
    for chunk in msg.as_bytes().chunks(chars_per_line) {
      let s = unsafe { core::str::from_utf8_unchecked(chunk) };
      Text::new(s, Point::new(10, y), text_style)
        .draw(&mut display).ok();
      y += 26;
    }

    display.flush().ok();

    // Keep error visible, then reboot
    error!("Rebooting in 30 seconds...");
    thread::sleep(Duration::from_secs(30));
    unsafe { esp_idf_svc::sys::esp_restart(); }
  }

  result
}

/// Helper for formatting text into a fixed buffer without allocation
#[cfg(feature = "display")]
struct LineBuf<'a> {
  buf: &'a mut [u8],
  pos: usize,
}

#[cfg(feature = "display")]
impl<'a> LineBuf<'a> {
  fn new(buf: &'a mut [u8]) -> Self {
    Self { buf, pos: 0 }
  }
}

#[cfg(feature = "display")]
impl core::fmt::Write for LineBuf<'_> {
  fn write_str(&mut self, s: &str) -> core::fmt::Result {
    let bytes = s.as_bytes();
    let remaining = self.buf.len() - self.pos;
    let len = bytes.len().min(remaining);
    self.buf[self.pos..self.pos + len].copy_from_slice(&bytes[..len]);
    self.pos += len;
    Ok(())
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
