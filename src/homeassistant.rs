//! Home Assistant MQTT integration
//!
//! Publishes sensor data to Home Assistant via MQTT with auto-discovery.
//! Supports bidirectional communication: publishes sensor state and
//! receives configuration commands (tank capacity, sensor height, max PSI).
//!
//! # Topics
//! - Discovery (sensors): `homeassistant/sensor/watercontroller_<name>/config`
//! - Discovery (numbers): `homeassistant/number/watercontroller_<name>/config`
//! - State: `watercontroller/state`
//! - Commands: `watercontroller/set/<parameter>`

use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use esp_idf_svc::mqtt::client::{EspMqttClient, EspMqttEvent, MqttClientConfiguration, QoS};
use log::*;

/// Device identifier for Home Assistant
const DEVICE_ID: &str = "watercontroller";

/// Command topics to subscribe to
const CMD_TOPIC_TANK_CAPACITY: &str = "watercontroller/set/tank_capacity";
const CMD_TOPIC_SENSOR_HEIGHT: &str = "watercontroller/set/sensor_height";
const CMD_TOPIC_MAX_PSI: &str = "watercontroller/set/max_psi";
const CMD_TOPIC_RADAR_HEIGHT: &str = "watercontroller/set/radar_height";
const CMD_TOPIC_RADAR_DEADZONE: &str = "watercontroller/set/radar_deadzone";

/// Configuration command received from Home Assistant
#[derive(Debug)]
pub enum ConfigCommand {
    SetTankCapacity(u16),
    SetSensorHeight(u16),
    SetMaxPsi(u16),
    SetRadarHeight(u16),
    SetRadarDeadzone(u16),
}

/// Home Assistant MQTT client wrapper
pub struct HomeAssistant {
    client: EspMqttClient<'static>,
    discovery_sent: bool,
    /// Last connection error from the MQTT event callback
    conn_error: Arc<Mutex<Option<String>>>,
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
    /// Configured tank capacity (gallons)
    pub tank_capacity: u16,
    /// Configured sensor height (feet)
    pub sensor_height: u16,
    /// Configured manometer max PSI
    pub max_psi: u16,
    /// Configured radar installation height (cm)
    pub radar_height: u16,
    /// Configured radar deadzone (cm) — distance from sensor to max water level
    pub radar_deadzone: u16,
}

impl HomeAssistant {
    /// Create a new Home Assistant MQTT client
    ///
    /// Commands received on `watercontroller/set/*` topics are parsed and
    /// forwarded to the main loop via the provided `cmd_tx` channel.
    pub fn new(
        broker: &str,
        port: u16,
        username: &str,
        password: &str,
        cmd_tx: Sender<ConfigCommand>,
    ) -> Result<Self, esp_idf_svc::sys::EspError> {
        let broker_url = format!("mqtt://{}:{}", broker, port);
        info!("Connecting to MQTT broker at {}", broker_url);

        let mqtt_config = MqttClientConfiguration {
            client_id: Some(DEVICE_ID),
            username: if username.is_empty() { None } else { Some(username) },
            password: if password.is_empty() { None } else { Some(password) },
            ..Default::default()
        };

        let conn_error: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let conn_error_cb = conn_error.clone();

        let client = EspMqttClient::new_cb(
            &broker_url,
            &mqtt_config,
            move |event| {
                Self::handle_event(&event, &cmd_tx, &conn_error_cb);
            },
        )?;

        info!("MQTT client created");

        Ok(Self {
            client,
            discovery_sent: false,
            conn_error,
        })
    }

    /// Handle incoming MQTT events
    fn handle_event(
        event: &EspMqttEvent,
        cmd_tx: &Sender<ConfigCommand>,
        conn_error: &Arc<Mutex<Option<String>>>,
    ) {
        use esp_idf_svc::mqtt::client::EventPayload;

        match event.payload() {
            EventPayload::Received { topic, data, .. } => {
                let Some(topic) = topic else { return };
                let Ok(value_str) = std::str::from_utf8(data) else {
                    warn!("MQTT: non-UTF8 payload on {}", topic);
                    return;
                };
                let Ok(value) = value_str.trim().parse::<f32>() else {
                    warn!("MQTT: invalid number '{}' on {}", value_str, topic);
                    return;
                };
                let value = value.round() as u16;

                let cmd = match topic {
                    CMD_TOPIC_TANK_CAPACITY => ConfigCommand::SetTankCapacity(value),
                    CMD_TOPIC_SENSOR_HEIGHT => ConfigCommand::SetSensorHeight(value),
                    CMD_TOPIC_MAX_PSI => ConfigCommand::SetMaxPsi(value),
                    CMD_TOPIC_RADAR_HEIGHT => ConfigCommand::SetRadarHeight(value),
                    CMD_TOPIC_RADAR_DEADZONE => ConfigCommand::SetRadarDeadzone(value),
                    _ => {
                        debug!("MQTT: unknown topic {}", topic);
                        return;
                    }
                };

                info!("MQTT command: {:?}", cmd);
                let _ = cmd_tx.send(cmd);
            }
            EventPayload::Connected(_) => {
                info!("MQTT connected");
                // Clear any previous error on successful connection
                if let Ok(mut err) = conn_error.lock() {
                    *err = None;
                }
            }
            EventPayload::Disconnected => {
                warn!("MQTT disconnected");
            }
            EventPayload::Error(_) => {
                // Extract detailed error from the raw event's error_handle
                let msg = Self::extract_error_detail(event);
                warn!("MQTT error: {}", msg);
                if let Ok(mut err) = conn_error.lock() {
                    *err = Some(msg);
                }
            }
            _ => {}
        }
    }

    /// Extract a human-readable error from the raw MQTT event
    fn extract_error_detail(event: &EspMqttEvent) -> String {
        // EspMqttEvent is a newtype: struct EspMqttEvent<'a>(&'a esp_mqtt_event_t)
        // We transmute to get the inner pointer since the field is private.
        // Safety: EspMqttEvent is a single-field newtype wrapping a reference.
        let raw: &esp_idf_svc::sys::esp_mqtt_event_t =
            unsafe { std::mem::transmute_copy::<EspMqttEvent, &esp_idf_svc::sys::esp_mqtt_event_t>(event) };

        if raw.error_handle.is_null() {
            return "Unknown error".to_string();
        }

        let err = unsafe { &*raw.error_handle };
        let sock_errno = err.esp_transport_sock_errno;

        if sock_errno != 0 {
            // Convert socket errno to string
            let cstr = unsafe { esp_idf_svc::sys::strerror(sock_errno) };
            if !cstr.is_null() {
                let msg = unsafe { std::ffi::CStr::from_ptr(cstr) };
                return msg.to_string_lossy().into_owned();
            }
            return format!("Socket error {}", sock_errno);
        }

        if err.esp_tls_last_esp_err != 0 {
            return format!("TLS error 0x{:x}", err.esp_tls_last_esp_err);
        }

        "Connection failed".to_string()
    }

    /// Return the last connection error, if any
    pub fn connection_error(&self) -> Option<String> {
        self.conn_error.lock().ok().and_then(|e| e.clone())
    }

    /// Subscribe to command topics
    pub fn subscribe(&mut self) -> Result<(), esp_idf_svc::sys::EspError> {
        info!("Subscribing to command topics...");
        const CMD_TOPICS: &[&str] = &[
            CMD_TOPIC_TANK_CAPACITY,
            CMD_TOPIC_SENSOR_HEIGHT,
            CMD_TOPIC_MAX_PSI,
            CMD_TOPIC_RADAR_HEIGHT,
            CMD_TOPIC_RADAR_DEADZONE,
        ];
        for topic in CMD_TOPICS {
            self.client.subscribe(topic, QoS::AtLeastOnce)?;
        }
        info!("Subscribed to command topics");
        Ok(())
    }

    /// Send Home Assistant MQTT discovery messages
    ///
    /// This configures the sensors and number entities in Home Assistant automatically.
    /// Should be called once after connection is established.
    pub fn send_discovery(&mut self) -> Result<(), esp_idf_svc::sys::EspError> {
        if self.discovery_sent {
            return Ok(());
        }

        info!("Sending Home Assistant discovery messages...");

        // Common device info (shared by all entities)
        let device_info = r#""dev":{"ids":"watercontroller","name":"Water Controller","mf":"DIY","mdl":"wESP32"}"#;

        // Sensor entities (read-only)
        const SENSORS: &[(&str, &str, &str, &str, &str, &str)] = &[
            // (discovery_name, ha_name, unique_id, value_key, unit, extra)
            ("capacity_percent", "Water Capacity", "wc_capacity_pct", "capacity_pct", "%", r#""dev_cla":"battery","stat_cla":"measurement""#),
            ("capacity_gallons", "Water Volume", "wc_capacity_gal", "gallons", "gal", r#""ic":"mdi:water","stat_cla":"measurement""#),
            ("pressure", "Water Pressure", "wc_pressure", "pressure_psi", "psi", r#""dev_cla":"pressure","stat_cla":"measurement""#),
        ];

        for &(disc_name, name, uid, val_key, unit, extra) in SENSORS {
            self.publish_discovery(
                "sensor",
                disc_name,
                &format!(
                    r#"{{"name":"{name}","uniq_id":"{uid}","stat_t":"watercontroller/state","val_tpl":"{{{{ value_json.{val_key} }}}}","unit_of_meas":"{unit}",{extra},{device_info}}}"#,
                ),
            )?;
        }

        // Number entities (configurable parameters)
        const NUMBERS: &[(&str, &str, &str, &str, &str, u16, u16, u16, &str, &str)] = &[
            // (disc_name, ha_name, unique_id, value_key, cmd_topic_suffix, min, max, step, unit, icon)
            ("tank_capacity", "Tank Capacity", "wc_tank_cap", "tank_capacity", "tank_capacity", 100, 2000, 10, "gal", "mdi:storage-tank"),
            ("sensor_height", "Pressure sensor Height", "wc_height", "sensor_height", "sensor_height", 0, 50, 1, "ft", "mdi:arrow-expand-vertical"),
            ("max_psi", "Manometer Range", "wc_max_psi", "max_psi", "max_psi", 50, 300, 10, "psi", "mdi:gauge"),
            ("radar_height", "Radar Height", "wc_radar_ht", "radar_height", "radar_height", 10, 500, 1, "cm", "mdi:signal-distance-variant"),
            ("radar_deadzone", "Radar Deadzone", "wc_radar_dz", "radar_deadzone", "radar_deadzone", 0, 200, 1, "cm", "mdi:arrow-collapse-down"),
        ];

        for &(disc_name, name, uid, val_key, cmd_suffix, min, max, step, unit, icon) in NUMBERS {
            self.publish_discovery(
                "number",
                disc_name,
                &format!(
                    r#"{{"name":"{name}","uniq_id":"{uid}","stat_t":"watercontroller/state","val_tpl":"{{{{ value_json.{val_key} }}}}","cmd_t":"watercontroller/set/{cmd_suffix}","min":{min},"max":{max},"step":{step},"mode":"box","unit_of_meas":"{unit}","ic":"{icon}",{device_info}}}"#,
                ),
            )?;
        }

        self.discovery_sent = true;
        info!("Discovery messages sent");
        Ok(())
    }

    /// Publish a discovery message for an entity
    fn publish_discovery(
        &mut self,
        entity_type: &str,
        entity_name: &str,
        config_payload: &str,
    ) -> Result<(), esp_idf_svc::sys::EspError> {
        let topic = format!(
            "homeassistant/{}/{}_{}/config",
            entity_type, DEVICE_ID, entity_name
        );
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
            r#"{{"capacity_pct":{},"gallons":{},"pressure_psi":{},"tank_capacity":{},"sensor_height":{},"max_psi":{},"radar_height":{},"radar_deadzone":{}}}"#,
            state.capacity_percent,
            state.capacity_gallons,
            state.pressure_psi,
            state.tank_capacity,
            state.sensor_height,
            state.max_psi,
            state.radar_height,
            state.radar_deadzone
        );

        debug!("Publishing state: {}", payload);

        self.client
            .publish("watercontroller/state", QoS::AtMostOnce, false, payload.as_bytes())?;

        Ok(())
    }
}
