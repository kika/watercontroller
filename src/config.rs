//! Configuration storage using NVS (Non-Volatile Storage)
//!
//! Stores configurable parameters that persist across reboots.
//! Parameters can be updated via MQTT from Home Assistant.

use esp_idf_svc::nvs::{EspNvs, EspNvsPartition, NvsDefault};
use log::*;

const NVS_NAMESPACE: &str = "wc_config";

// NVS keys (max 15 chars)
const KEY_TANK_CAPACITY: &str = "tank_cap";
const KEY_SENSOR_HEIGHT: &str = "height_ft";
const KEY_MAX_PSI: &str = "max_psi";
const KEY_RADAR_HEIGHT: &str = "radar_ht_cm";

// Defaults
const DEFAULT_TANK_CAPACITY: u16 = 500;
const DEFAULT_SENSOR_HEIGHT: u16 = 11;
const DEFAULT_MAX_PSI: u16 = 150;
const DEFAULT_RADAR_HEIGHT: u16 = 200;

/// Persistent configuration
pub struct Config {
    nvs: EspNvs<NvsDefault>,
    pub tank_capacity_gallons: u16,
    pub sensor_height_feet: u16,
    pub max_psi: u16,
    pub radar_height_cm: u16,
}

impl Config {
    /// Load configuration from NVS, using defaults for missing values
    pub fn load(
        nvs_partition: EspNvsPartition<NvsDefault>,
    ) -> Result<Self, esp_idf_svc::sys::EspError> {
        let nvs = EspNvs::new(nvs_partition, NVS_NAMESPACE, true)?;

        let tank_capacity_gallons = nvs
            .get_u16(KEY_TANK_CAPACITY)?
            .unwrap_or(DEFAULT_TANK_CAPACITY);
        let sensor_height_feet = nvs
            .get_u16(KEY_SENSOR_HEIGHT)?
            .unwrap_or(DEFAULT_SENSOR_HEIGHT);
        let max_psi = nvs.get_u16(KEY_MAX_PSI)?.unwrap_or(DEFAULT_MAX_PSI);
        let radar_height_cm = nvs
            .get_u16(KEY_RADAR_HEIGHT)?
            .unwrap_or(DEFAULT_RADAR_HEIGHT);

        info!(
            "Config loaded: tank={}gal, height={}ft, max_psi={}, radar={}cm",
            tank_capacity_gallons, sensor_height_feet, max_psi, radar_height_cm
        );

        Ok(Self {
            nvs,
            tank_capacity_gallons,
            sensor_height_feet,
            max_psi,
            radar_height_cm,
        })
    }

    /// Set tank capacity and persist to NVS
    pub fn set_tank_capacity(
        &mut self,
        gallons: u16,
    ) -> Result<(), esp_idf_svc::sys::EspError> {
        let gallons = gallons.clamp(100, 2000);
        self.tank_capacity_gallons = gallons;
        self.nvs.set_u16(KEY_TANK_CAPACITY, gallons)?;
        info!("Config: tank capacity = {} gal", gallons);
        Ok(())
    }

    /// Set sensor height and persist to NVS
    pub fn set_sensor_height(
        &mut self,
        feet: u16,
    ) -> Result<(), esp_idf_svc::sys::EspError> {
        let feet = feet.clamp(0, 50);
        self.sensor_height_feet = feet;
        self.nvs.set_u16(KEY_SENSOR_HEIGHT, feet)?;
        info!("Config: sensor height = {} ft", feet);
        Ok(())
    }

    /// Set manometer max PSI and persist to NVS
    pub fn set_max_psi(
        &mut self,
        psi: u16,
    ) -> Result<(), esp_idf_svc::sys::EspError> {
        let psi = psi.clamp(50, 300);
        self.max_psi = psi;
        self.nvs.set_u16(KEY_MAX_PSI, psi)?;
        info!("Config: max PSI = {}", psi);
        Ok(())
    }

    /// Set radar installation height and persist to NVS
    pub fn set_radar_height(
        &mut self,
        cm: u16,
    ) -> Result<(), esp_idf_svc::sys::EspError> {
        let cm = cm.clamp(10, 500);
        self.radar_height_cm = cm;
        self.nvs.set_u16(KEY_RADAR_HEIGHT, cm)?;
        info!("Config: radar height = {} cm", cm);
        Ok(())
    }
}
