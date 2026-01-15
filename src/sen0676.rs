//! DFRobot SEN0676 80GHz mmWave Radar Liquid Level Sensor Driver
//!
//! Communicates via Modbus-RTU over UART.
//!
//! # Register Map
//! | Register | R/W | Name | Unit |
//! |----------|-----|------|------|
//! | 0x0001 | R | empty_height | mm |
//! | 0x0003 | R | water_level | mm |
//! | 0x0005 | R/W | installation_height | cm |
//! | 0x03F4 | R/W | device_address | - |
//! | 0x03F6 | R/W | baud_rate | baud/100 |
//! | 0x07D4 | R/W | range | m |

use log::debug;
use esp_idf_svc::hal::io::{Read, Write};

/// Modbus register addresses
mod registers {
    pub const EMPTY_HEIGHT: u16 = 0x0001;
    pub const WATER_LEVEL: u16 = 0x0003;
    pub const INSTALLATION_HEIGHT: u16 = 0x0005;
    pub const DEVICE_ADDRESS: u16 = 0x03F4;
    pub const BAUD_RATE: u16 = 0x03F6;
    pub const RANGE: u16 = 0x07D4;
}

/// Modbus function codes
mod function {
    pub const READ_HOLDING_REGISTERS: u8 = 0x03;
    pub const WRITE_SINGLE_REGISTER: u8 = 0x06;
}

/// Default communication parameters
pub const DEFAULT_ADDRESS: u8 = 0x01;
pub const DEFAULT_BAUD_RATE: u32 = 115200;

/// Errors that can occur during communication
#[derive(Debug)]
pub enum Error {
    /// UART I/O error
    Io,
    /// CRC mismatch in response
    CrcMismatch,
    /// Invalid response length
    InvalidLength,
    /// Unexpected device address in response
    AddressMismatch,
    /// Unexpected function code in response
    FunctionMismatch,
    /// Modbus exception response
    ModbusException(u8),
    /// Timeout waiting for response
    Timeout,
    /// Invalid baud rate value
    InvalidBaudRate,
    /// Invalid device address (must be 0x01-0xFD)
    InvalidAddress,
}

/// DFRobot SEN0676 80GHz mmWave Radar driver
pub struct Sen0676<U> {
    uart: U,
    address: u8,
}

impl<U> Sen0676<U>
where
    U: Read + Write,
{
    /// Create a new sensor instance
    ///
    /// # Arguments
    /// * `uart` - UART peripheral implementing Read + Write
    /// * `address` - Modbus device address (default: 0x01)
    pub fn new(uart: U, address: u8) -> Self {
        esp_idf_svc::log::set_target_level(module_path!(), log::LevelFilter::Debug).unwrap();
        Self { uart, address }
    }

    /// Create a new sensor instance with default address (0x01)
    pub fn new_default(uart: U) -> Self {
        Self::new(uart, DEFAULT_ADDRESS)
    }

    /// Read and log any ASCII messages from the sensor (for diagnostics)
    ///
    /// Some sensors output ASCII error/status messages on boot.
    /// Call this before normal Modbus communication to drain any such messages.
    pub fn drain_ascii_messages(&mut self) {
        use log::info;
        let mut buf = [0u8; 1];
        let mut line = String::new();

        loop {
            match self.uart.read(&mut buf) {
                Ok(1) => {
                    let ch = buf[0];
                    if ch == b'\n' {
                        if !line.is_empty() {
                            info!("Sensor: {}", line.trim());
                            line.clear();
                        }
                    } else if ch >= 0x20 && ch < 0x7F {
                        line.push(ch as char);
                    } else if ch == b'\r' {
                        // ignore CR
                    }
                }
                Ok(_) | Err(_) => {
                    // Timeout or error - no more data
                    if !line.is_empty() {
                        info!("Sensor: {}", line.trim());
                    }
                    break;
                }
            }
        }
    }

    /// Read the empty height (distance from sensor to liquid surface)
    ///
    /// Returns distance in millimeters (filtered data)
    pub fn read_empty_height(&mut self) -> Result<u16, Error> {
        self.read_register(registers::EMPTY_HEIGHT)
    }

    /// Read the calculated water level
    ///
    /// Returns water level in millimeters (filtered data)
    /// Note: Installation height must be set first for accurate readings
    pub fn read_water_level(&mut self) -> Result<u16, Error> {
        self.read_register(registers::WATER_LEVEL)
    }

    /// Read the configured installation height
    ///
    /// Returns height in centimeters
    pub fn read_installation_height(&mut self) -> Result<u16, Error> {
        self.read_register(registers::INSTALLATION_HEIGHT)
    }

    /// Set the installation height (distance from sensor to tank bottom)
    ///
    /// # Arguments
    /// * `cm` - Installation height in centimeters
    ///
    /// Water level = Installation height - Empty height
    pub fn set_installation_height(&mut self, cm: u16) -> Result<(), Error> {
        self.write_register(registers::INSTALLATION_HEIGHT, cm)
    }

    /// Read the current device address
    pub fn read_device_address(&mut self) -> Result<u8, Error> {
        let value = self.read_register(registers::DEVICE_ADDRESS)?;
        Ok(value as u8)
    }

    /// Set a new device address
    ///
    /// # Arguments
    /// * `addr` - New address (0x01-0xFD, broadcast 0xFF not allowed for setting)
    ///
    /// Note: After changing the address, create a new `Sen0676` instance with the new address
    pub fn set_device_address(&mut self, addr: u8) -> Result<(), Error> {
        if addr == 0 || addr > 0xFD {
            return Err(Error::InvalidAddress);
        }
        self.write_register(registers::DEVICE_ADDRESS, addr as u16)?;
        self.address = addr;
        Ok(())
    }

    /// Read the current baud rate
    ///
    /// Returns actual baud rate (e.g., 115200)
    pub fn read_baud_rate(&mut self) -> Result<u32, Error> {
        let value = self.read_register(registers::BAUD_RATE)?;
        Ok(value as u32 * 100)
    }

    /// Set the baud rate
    ///
    /// # Arguments
    /// * `baud` - Baud rate (supported: 4800, 9600, 14400, 19200, 38400, 56000, 57600, 115200, 129000)
    ///
    /// Note: After changing baud rate, reconfigure UART and create a new `Sen0676` instance
    pub fn set_baud_rate(&mut self, baud: u32) -> Result<(), Error> {
        let value = match baud {
            4800 => 48,
            9600 => 96,
            14400 => 144,
            19200 => 192,
            38400 => 384,
            56000 => 560,
            57600 => 576,
            115200 => 1152,
            129000 => 1290,
            _ => return Err(Error::InvalidBaudRate),
        };
        self.write_register(registers::BAUD_RATE, value)
    }

    /// Read the maximum measurement range
    ///
    /// Returns range in meters
    pub fn read_range(&mut self) -> Result<u16, Error> {
        self.read_register(registers::RANGE)
    }

    /// Set the maximum measurement range
    ///
    /// # Arguments
    /// * `meters` - Maximum range in meters (max depends on product model, default 10m)
    pub fn set_range(&mut self, meters: u16) -> Result<(), Error> {
        self.write_register(registers::RANGE, meters)
    }

    /// Read a single holding register
    fn read_register(&mut self, register: u16) -> Result<u16, Error> {
        // Build request: [addr] [0x03] [reg_hi] [reg_lo] [count_hi] [count_lo] [crc_lo] [crc_hi]
        let mut request = [0u8; 8];
        request[0] = self.address;
        request[1] = function::READ_HOLDING_REGISTERS;
        request[2] = (register >> 8) as u8;
        request[3] = register as u8;
        request[4] = 0x00; // Number of registers (high byte)
        request[5] = 0x01; // Number of registers (low byte) - reading 1 register

        let crc = crc16(&request[0..6]);
        request[6] = crc as u8; // CRC low byte
        request[7] = (crc >> 8) as u8; // CRC high byte

        self.uart.write(&request).map_err(|_| Error::Io)?;

        // Read response: [addr] [0x03] [byte_count] [data_hi] [data_lo] [crc_lo] [crc_hi]
        let mut response = [0u8; 7];
        self.read_exact(&mut response)?;

        debug!("TX: {:02X?}", &request);
        debug!("RX: {:02X?} (ASCII: {:?})", &response, core::str::from_utf8(&response).unwrap_or("N/A"));

        // Verify CRC
        let received_crc = (response[6] as u16) << 8 | response[5] as u16;
        let calculated_crc = crc16(&response[0..5]);
        if received_crc != calculated_crc {
            debug!("CRC mismatch: received 0x{:04X}, calculated 0x{:04X}", received_crc, calculated_crc);
            return Err(Error::CrcMismatch);
        }

        // Check for exception response
        if response[1] & 0x80 != 0 {
            return Err(Error::ModbusException(response[2]));
        }

        // Verify address and function
        if response[0] != self.address {
            return Err(Error::AddressMismatch);
        }
        if response[1] != function::READ_HOLDING_REGISTERS {
            return Err(Error::FunctionMismatch);
        }
        if response[2] != 2 {
            return Err(Error::InvalidLength);
        }

        // Extract value (big-endian)
        let value = (response[3] as u16) << 8 | response[4] as u16;
        Ok(value)
    }

    /// Write a single holding register
    fn write_register(&mut self, register: u16, value: u16) -> Result<(), Error> {
        // Build request: [addr] [0x06] [reg_hi] [reg_lo] [val_hi] [val_lo] [crc_lo] [crc_hi]
        let mut request = [0u8; 8];
        request[0] = self.address;
        request[1] = function::WRITE_SINGLE_REGISTER;
        request[2] = (register >> 8) as u8;
        request[3] = register as u8;
        request[4] = (value >> 8) as u8;
        request[5] = value as u8;

        let crc = crc16(&request[0..6]);
        request[6] = crc as u8; // CRC low byte
        request[7] = (crc >> 8) as u8; // CRC high byte

        self.uart.write(&request).map_err(|_| Error::Io)?;

        // Read response (echo of request): [addr] [0x06] [reg_hi] [reg_lo] [val_hi] [val_lo] [crc_lo] [crc_hi]
        let mut response = [0u8; 8];
        self.read_exact(&mut response)?;

        // Verify CRC
        let received_crc = (response[7] as u16) << 8 | response[6] as u16;
        let calculated_crc = crc16(&response[0..6]);
        if received_crc != calculated_crc {
            debug!("CRC mismatch: received 0x{:04X}, calculated 0x{:04X}", received_crc, calculated_crc);
            return Err(Error::CrcMismatch);
        }

        // Check for exception response
        if response[1] & 0x80 != 0 {
            return Err(Error::ModbusException(response[2]));
        }

        // Verify address and function
        if response[0] != self.address {
            return Err(Error::AddressMismatch);
        }
        if response[1] != function::WRITE_SINGLE_REGISTER {
            return Err(Error::FunctionMismatch);
        }

        Ok(())
    }

    /// Read exact number of bytes from UART
    fn read_exact(&mut self, buf: &mut [u8]) -> Result<(), Error> {
        let mut pos = 0;
        while pos < buf.len() {
            match self.uart.read(&mut buf[pos..]) {
                Ok(0) => return Err(Error::Timeout),
                Ok(n) => pos += n,
                Err(_) => return Err(Error::Io),
            }
        }
        Ok(())
    }
}

/// Calculate CRC16 with Modbus polynomial (0xA001)
fn crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for byte in data {
        crc ^= *byte as u16;
        for _ in 0..8 {
            if crc & 0x0001 != 0 {
                crc = (crc >> 1) ^ 0xA001;
            } else {
                crc >>= 1;
            }
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_crc16() {
        // Test vector from datasheet: read empty height command
        // 01 03 00 01 00 01 -> CRC should be D5 CA (0xCAD5 little-endian)
        let data = [0x01, 0x03, 0x00, 0x01, 0x00, 0x01];
        let crc = crc16(&data);
        assert_eq!(crc, 0xCAD5);
    }

    #[test]
    fn test_crc16_write_installation_height() {
        // Test vector: write installation height 1000cm
        // 01 06 00 05 03 E8 -> CRC should be 99 75 (0x7599 little-endian)
        let data = [0x01, 0x06, 0x00, 0x05, 0x03, 0xE8];
        let crc = crc16(&data);
        assert_eq!(crc, 0x7599);
    }
}
