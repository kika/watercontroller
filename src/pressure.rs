//! Pressure sensor driver using ADC
//!
//! Reads a 0.5V-4.5V pressure transducer via voltage divider.
//! Sensor range: 0.5V = 0 PSI, 4.5V = 100 PSI
//!
//! # Voltage Divider
//! With 10kΩ/12kΩ divider (ratio 0.545):
//! - 0.5V sensor → 0.27V at ADC
//! - 4.5V sensor → 2.45V at ADC
//!
//! ```text
//! Sensor out ──[10kΩ]──┬── GPIO36 (ADC1_CH0)
//!                      │
//!                    [12kΩ]
//!                      │
//!                     GND
//! ```

use esp_idf_svc::hal::{
    adc::{
        attenuation::DB_11,
        oneshot::{config::AdcChannelConfig, AdcChannelDriver, AdcDriver},
        ADC1,
    },
    gpio::Gpio36,
};

/// Voltage divider ratio: R2/(R1+R2) = 12/(10+12)
const DIVIDER_RATIO: f32 = 0.545;

/// Sensor minimum voltage (0 PSI)
const SENSOR_MIN_MV: f32 = 500.0;
/// Sensor maximum voltage (100 PSI)
const SENSOR_MAX_MV: f32 = 4500.0;
/// Sensor pressure range
const SENSOR_MAX_PSI: f32 = 100.0;

/// PSI per foot of water column (hydrostatic pressure)
const PSI_PER_FOOT: f32 = 0.433;

/// Pressure sensor driver for GPIO36 (ADC1_CH0)
pub struct PressureSensor<'d> {
    channel: AdcChannelDriver<'d, Gpio36, AdcDriver<'d, ADC1>>,
}

impl<'d> PressureSensor<'d> {
    /// Create a new pressure sensor
    ///
    /// # Arguments
    /// * `adc` - ADC1 peripheral
    /// * `pin` - GPIO36 pin
    pub fn new(
        adc: impl esp_idf_svc::hal::peripheral::Peripheral<P = ADC1> + 'd,
        pin: impl esp_idf_svc::hal::peripheral::Peripheral<P = Gpio36> + 'd,
    ) -> Result<Self, esp_idf_svc::sys::EspError> {
        let adc_driver = AdcDriver::new(adc)?;

        // Configure channel with 11dB attenuation for 150-2450mV range
        let config = AdcChannelConfig {
            attenuation: DB_11,
            ..Default::default()
        };
        let channel = AdcChannelDriver::new(adc_driver, pin, &config)?;

        Ok(Self { channel })
    }

    /// Read raw ADC value in millivolts (at the ADC pin, after divider)
    pub fn read_raw_mv(&mut self) -> Result<u16, esp_idf_svc::sys::EspError> {
        self.channel.read()
    }

    /// Read sensor voltage in millivolts (before divider, actual sensor output)
    pub fn read_sensor_mv(&mut self) -> Result<u32, esp_idf_svc::sys::EspError> {
        let raw_mv = self.read_raw_mv()? as f32;
        // Compensate for voltage divider
        let sensor_mv = raw_mv / DIVIDER_RATIO;
        Ok(sensor_mv as u32)
    }

    /// Read pressure in PSI
    ///
    /// Returns pressure clamped to 0-100 PSI range.
    /// Includes averaging for stability.
    ///
    /// # Arguments
    /// * `height_feet` - Sensor height above ground level in feet (for hydrostatic compensation)
    pub fn read_psi(&mut self, height_feet: f32) -> Result<f32, esp_idf_svc::sys::EspError> {
        // Average multiple readings for stability
        const SAMPLES: u32 = 8;
        let mut sum: u32 = 0;

        for _ in 0..SAMPLES {
            sum += self.read_raw_mv()? as u32;
        }

        let avg_raw_mv = sum as f32 / SAMPLES as f32;

        // Compensate for voltage divider
        let sensor_mv = avg_raw_mv / DIVIDER_RATIO;

        // Convert to PSI: linear interpolation from 500mV-4500mV to 0-100 PSI
        let psi = (sensor_mv - SENSOR_MIN_MV) / (SENSOR_MAX_MV - SENSOR_MIN_MV) * SENSOR_MAX_PSI;

        // Compensate for sensor height above ground level
        let psi = psi + (height_feet * PSI_PER_FOOT);

        // Clamp to valid range
        Ok(psi.clamp(0.0, SENSOR_MAX_PSI))
    }

    /// Read pressure as integer PSI (rounded)
    pub fn read_psi_u16(&mut self, height_feet: f32) -> Result<u16, esp_idf_svc::sys::EspError> {
        let psi = self.read_psi(height_feet)?;
        Ok(psi.round() as u16)
    }
}
