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

/// Configuration command received from Home Assistant
#[derive(Debug)]
pub enum ConfigCommand {
    SetTankCapacity(u16),
    SetSensorHeight(u16),
    SetMaxPsi(u16),
    SetRadarHeight(u16),
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
        self.client.subscribe(CMD_TOPIC_TANK_CAPACITY, QoS::AtLeastOnce)?;
        self.client.subscribe(CMD_TOPIC_SENSOR_HEIGHT, QoS::AtLeastOnce)?;
        self.client.subscribe(CMD_TOPIC_MAX_PSI, QoS::AtLeastOnce)?;
        self.client.subscribe(CMD_TOPIC_RADAR_HEIGHT, QoS::AtLeastOnce)?;
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

        // --- Sensor entities ---

        // Capacity percent sensor
        self.publish_discovery(
            "sensor",
            "capacity_percent",
            &format!(
                r#"{{"name":"Water Capacity","uniq_id":"wc_capacity_pct","stat_t":"watercontroller/state","val_tpl":"{{{{ value_json.capacity_pct }}}}","unit_of_meas":"%","dev_cla":"battery","stat_cla":"measurement",{}}}"#,
                device_info
            ),
        )?;

        // Capacity gallons sensor
        self.publish_discovery(
            "sensor",
            "capacity_gallons",
            &format!(
                r#"{{"name":"Water Volume","uniq_id":"wc_capacity_gal","stat_t":"watercontroller/state","val_tpl":"{{{{ value_json.gallons }}}}","unit_of_meas":"gal","ic":"mdi:water","stat_cla":"measurement",{}}}"#,
                device_info
            ),
        )?;

        // Pressure sensor
        self.publish_discovery(
            "sensor",
            "pressure",
            &format!(
                r#"{{"name":"Water Pressure","uniq_id":"wc_pressure","stat_t":"watercontroller/state","val_tpl":"{{{{ value_json.pressure_psi }}}}","unit_of_meas":"psi","dev_cla":"pressure","stat_cla":"measurement",{}}}"#,
                device_info
            ),
        )?;

        // --- Number entities (configurable parameters) ---

        // Tank capacity
        self.publish_discovery(
            "number",
            "tank_capacity",
            &format!(
                r#"{{"name":"Tank Capacity","uniq_id":"wc_tank_cap","stat_t":"watercontroller/state","val_tpl":"{{{{ value_json.tank_capacity }}}}","cmd_t":"watercontroller/set/tank_capacity","min":100,"max":2000,"step":10,"mode":"box","unit_of_meas":"gal","ic":"mdi:storage-tank",{}}}"#,
                device_info
            ),
        )?;

        // Sensor height
        self.publish_discovery(
            "number",
            "sensor_height",
            &format!(
                r#"{{"name":"Sensor Height","uniq_id":"wc_height","stat_t":"watercontroller/state","val_tpl":"{{{{ value_json.sensor_height }}}}","cmd_t":"watercontroller/set/sensor_height","min":0,"max":50,"step":1,"mode":"box","unit_of_meas":"ft","ic":"mdi:arrow-expand-vertical",{}}}"#,
                device_info
            ),
        )?;

        // Max PSI
        self.publish_discovery(
            "number",
            "max_psi",
            &format!(
                r#"{{"name":"Manometer Range","uniq_id":"wc_max_psi","stat_t":"watercontroller/state","val_tpl":"{{{{ value_json.max_psi }}}}","cmd_t":"watercontroller/set/max_psi","min":50,"max":300,"step":10,"mode":"box","unit_of_meas":"psi","ic":"mdi:gauge",{}}}"#,
                device_info
            ),
        )?;

        // Radar installation height
        self.publish_discovery(
            "number",
            "radar_height",
            &format!(
                r#"{{"name":"Radar Height","uniq_id":"wc_radar_ht","stat_t":"watercontroller/state","val_tpl":"{{{{ value_json.radar_height }}}}","cmd_t":"watercontroller/set/radar_height","min":10,"max":500,"step":1,"mode":"box","unit_of_meas":"cm","ic":"mdi:signal-distance-variant",{}}}"#,
                device_info
            ),
        )?;

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
            r#"{{"capacity_pct":{},"gallons":{},"pressure_psi":{},"tank_capacity":{},"sensor_height":{},"max_psi":{},"radar_height":{}}}"#,
            state.capacity_percent,
            state.capacity_gallons,
            state.pressure_psi,
            state.tank_capacity,
            state.sensor_height,
            state.max_psi,
            state.radar_height
        );

        debug!("Publishing state: {}", payload);

        self.client
            .publish("watercontroller/state", QoS::AtMostOnce, false, payload.as_bytes())?;

        Ok(())
    }
}
