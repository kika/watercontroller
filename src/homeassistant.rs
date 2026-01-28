//! Home Assistant MQTT integration
//!
//! Publishes sensor data to Home Assistant via MQTT with auto-discovery.
//!
//! # Topics
//! - Discovery: `homeassistant/sensor/watercontroller_<name>/config`
//! - State: `watercontroller/state`

use esp_idf_svc::mqtt::client::{EspMqttClient, MqttClientConfiguration, QoS};
use log::*;

/// Default MQTT port
const MQTT_PORT: u16 = 1883;

/// Device identifier for Home Assistant
const DEVICE_ID: &str = "watercontroller";

/// Home Assistant MQTT client wrapper
pub struct HomeAssistant {
    client: EspMqttClient<'static>,
    discovery_sent: bool,
}

/// Sensor state to publish
#[derive(Default)]
pub struct WaterState {
    /// Tank capacity percentage (0-100)
    pub capacity_percent: u8,
    /// Tank capacity in gallons
    pub capacity_gallons: u16,
    /// Water pressure in PSI
    pub pressure_psi: u16,
}

impl HomeAssistant {
    /// Create a new Home Assistant MQTT client
    ///
    /// Connects to MQTT broker at homeassistant.local:1883
    pub fn new() -> Result<Self, esp_idf_svc::sys::EspError> {
        let broker_url = format!("mqtt://homeassistant.local:{}", MQTT_PORT);
        info!("Connecting to MQTT broker at {}", broker_url);

        let mqtt_config = MqttClientConfiguration {
            client_id: Some(DEVICE_ID),
            ..Default::default()
        };

        let client = EspMqttClient::new_cb(
            &broker_url,
            &mqtt_config,
            |_event| {
                // Event callback - could add connection status handling here
            },
        )?;

        info!("MQTT client created");

        Ok(Self {
            client,
            discovery_sent: false,
        })
    }

    /// Send Home Assistant MQTT discovery messages
    ///
    /// This configures the sensors in Home Assistant automatically.
    /// Should be called once after connection is established.
    pub fn send_discovery(&mut self) -> Result<(), esp_idf_svc::sys::EspError> {
        if self.discovery_sent {
            return Ok(());
        }

        info!("Sending Home Assistant discovery messages...");

        // Common device info (shared by all sensors)
        let device_info = r#""dev":{"ids":"watercontroller","name":"Water Controller","mf":"DIY","mdl":"wESP32"}"#;

        // Capacity percent sensor
        self.publish_discovery(
            "capacity_percent",
            &format!(
                r#"{{"name":"Water Capacity","uniq_id":"wc_capacity_pct","stat_t":"watercontroller/state","val_tpl":"{{{{ value_json.capacity_pct }}}}","unit_of_meas":"%","dev_cla":"battery","stat_cla":"measurement",{}}}"#,
                device_info
            ),
        )?;

        // Capacity gallons sensor
        self.publish_discovery(
            "capacity_gallons",
            &format!(
                r#"{{"name":"Water Volume","uniq_id":"wc_capacity_gal","stat_t":"watercontroller/state","val_tpl":"{{{{ value_json.gallons }}}}","unit_of_meas":"gal","ic":"mdi:water","stat_cla":"measurement",{}}}"#,
                device_info
            ),
        )?;

        // Pressure sensor
        self.publish_discovery(
            "pressure",
            &format!(
                r#"{{"name":"Water Pressure","uniq_id":"wc_pressure","stat_t":"watercontroller/state","val_tpl":"{{{{ value_json.pressure_psi }}}}","unit_of_meas":"psi","dev_cla":"pressure","stat_cla":"measurement",{}}}"#,
                device_info
            ),
        )?;

        self.discovery_sent = true;
        info!("Discovery messages sent");
        Ok(())
    }

    /// Publish a discovery message for a sensor
    fn publish_discovery(
        &mut self,
        sensor_name: &str,
        config_payload: &str,
    ) -> Result<(), esp_idf_svc::sys::EspError> {
        let topic = format!("homeassistant/sensor/{}_{}/config", DEVICE_ID, sensor_name);
        debug!("Publishing discovery to {}: {}", topic, config_payload);

        self.client
            .publish(&topic, QoS::AtLeastOnce, true, config_payload.as_bytes())?;
        Ok(())
    }

    /// Publish current sensor state
    pub fn publish_state(&mut self, state: &WaterState) -> Result<(), esp_idf_svc::sys::EspError> {
        // Ensure discovery is sent first
        if !self.discovery_sent {
            self.send_discovery()?;
        }

        let payload = format!(
            r#"{{"capacity_pct":{},"gallons":{},"pressure_psi":{}}}"#,
            state.capacity_percent, state.capacity_gallons, state.pressure_psi
        );

        debug!("Publishing state: {}", payload);

        self.client
            .publish("watercontroller/state", QoS::AtMostOnce, false, payload.as_bytes())?;

        Ok(())
    }
}
