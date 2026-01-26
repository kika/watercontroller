//! Sharp LS027B7DH01 Memory LCD Driver
//!
//! 2.7" 400x240 monochrome memory display with SPI interface.
//!
//! # Features
//! - Ultra-low power (display retains image without power)
//! - 1-bit per pixel (black/white)
//! - SPI Mode 1 (CPOL=0, CPHA=1), LSB-first bit order
//!
//! # Wiring
//! - SCLK: SPI clock
//! - MOSI: SPI data (SI on display)
//! - CS: Chip select (active HIGH - directly controlled, not via SPI driver)
//! - DISP: Display on/off (directly controlled, active high)
//! - EXTCOMIN: VCOM toggle (optional, can use software instead)

use embedded_graphics::{
  Pixel,
  draw_target::DrawTarget,
  geometry::{OriginDimensions, Size},
  pixelcolor::BinaryColor,
};
use esp_idf_svc::hal::{
  gpio::{Output, PinDriver},
  spi::{SpiDeviceDriver, SpiDriver},
};

/// Display width in pixels
pub const WIDTH: u16 = 400;
/// Display height in pixels
pub const HEIGHT: u16 = 240;
/// Bytes per line (400 pixels / 8 bits)
const BYTES_PER_LINE: usize = 50;
/// Total framebuffer size
const FRAMEBUFFER_SIZE: usize = BYTES_PER_LINE * HEIGHT as usize;

/// Mode bits (LSB-first format)
mod cmd {
  /// Write line command (M0=1)
  pub const WRITE: u8 = 0x01;
  /// VCOM bit (M1=1) - toggle periodically
  pub const VCOM: u8 = 0x02;
  /// Clear display command (M2=1)
  pub const CLEAR: u8 = 0x04;
}

/// Sharp Memory LCD driver
pub struct Ls027b7dh01<'d, SPI, CS>
where
  SPI: std::borrow::Borrow<SpiDriver<'d>>,
  CS: esp_idf_svc::hal::gpio::Pin,
{
  spi: SpiDeviceDriver<'d, SPI>,
  cs: PinDriver<'d, CS, Output>,
  framebuffer: [u8; FRAMEBUFFER_SIZE],
  vcom: bool,
}

impl<'d, SPI, CS> Ls027b7dh01<'d, SPI, CS>
where
  SPI: std::borrow::Borrow<SpiDriver<'d>>,
  CS: esp_idf_svc::hal::gpio::OutputPin,
{
  /// Create a new display driver
  pub fn new(spi: SpiDeviceDriver<'d, SPI>, cs: PinDriver<'d, CS, Output>) -> Self {
    Self {
      spi,
      cs,
      framebuffer: [0xFF; FRAMEBUFFER_SIZE], // White (all 1s)
      vcom: false,
    }
  }

  /// Initialize the display
  pub fn init(&mut self) -> Result<(), esp_idf_svc::sys::EspError> {
    self.cs.set_low()?;
    std::thread::sleep(std::time::Duration::from_millis(10));
    self.clear_display()?;
    Ok(())
  }

  /// Clear the entire display to white
  pub fn clear_display(&mut self) -> Result<(), esp_idf_svc::sys::EspError> {
    self.framebuffer.fill(0xFF);

    self.cs.set_high()?;
    let mode = cmd::CLEAR | if self.vcom { cmd::VCOM } else { 0 };
    self.spi.write(&[mode, 0x00])?;
    self.cs.set_low()?;

    self.vcom = !self.vcom;
    Ok(())
  }

  /// Fill display with black
  pub fn fill_black(&mut self) -> Result<(), esp_idf_svc::sys::EspError> {
    self.framebuffer.fill(0x00);
    self.flush()
  }

  /// Toggle VCOM (call periodically, at least once per second)
  pub fn toggle_vcom(&mut self) -> Result<(), esp_idf_svc::sys::EspError> {
    self.cs.set_high()?;
    let mode = if self.vcom { cmd::VCOM } else { 0 };
    self.spi.write(&[mode, 0x00])?;
    self.cs.set_low()?;

    self.vcom = !self.vcom;
    Ok(())
  }

  /// Write the framebuffer to the display
  pub fn flush(&mut self) -> Result<(), esp_idf_svc::sys::EspError> {
    self.cs.set_high()?;

    // Send mode byte
    let mode = cmd::WRITE | if self.vcom { cmd::VCOM } else { 0 };
    self.spi.write(&[mode])?;

    // Send each line: address + data + dummy
    for line in 0..HEIGHT {
      let mut line_buf = [0u8; 1 + BYTES_PER_LINE + 1];
      line_buf[0] = (line + 1) as u8; // Line address (1-indexed)

      // Copy pixel data
      let start = line as usize * BYTES_PER_LINE;
      line_buf[1..1 + BYTES_PER_LINE].copy_from_slice(&self.framebuffer[start..start + BYTES_PER_LINE]);

      // Trailing dummy byte already 0
      self.spi.write(&line_buf)?;
    }

    // Final dummy byte
    self.spi.write(&[0x00])?;

    self.cs.set_low()?;
    self.vcom = !self.vcom;
    Ok(())
  }

  /// Set a pixel in the framebuffer (call flush() to update display)
  pub fn set_pixel(&mut self, x: u16, y: u16, color: bool) {
    if x >= WIDTH || y >= HEIGHT {
      return;
    }

    let byte_idx = y as usize * BYTES_PER_LINE + (x / 8) as usize;
    let bit_idx = x % 8; // LSB is leftmost pixel (Sharp Memory LCD format)

    if color {
      // White = 1
      self.framebuffer[byte_idx] |= 1 << bit_idx;
    } else {
      // Black = 0
      self.framebuffer[byte_idx] &= !(1 << bit_idx);
    }
  }

  /// Get raw framebuffer access
  pub fn framebuffer(&self) -> &[u8] {
    &self.framebuffer
  }

  /// Get mutable raw framebuffer access
  pub fn framebuffer_mut(&mut self) -> &mut [u8] {
    &mut self.framebuffer
  }
}

/// embedded-graphics DrawTarget implementation
impl<'d, SPI, CS> DrawTarget for Ls027b7dh01<'d, SPI, CS>
where
  SPI: std::borrow::Borrow<SpiDriver<'d>>,
  CS: esp_idf_svc::hal::gpio::OutputPin,
{
  type Color = BinaryColor;
  type Error = core::convert::Infallible;

  fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
  where
    I: IntoIterator<Item = Pixel<Self::Color>>,
  {
    for Pixel(coord, color) in pixels.into_iter() {
      if coord.x >= 0
        && coord.x < WIDTH as i32
        && coord.y >= 0
        && coord.y < HEIGHT as i32
      {
        self.set_pixel(coord.x as u16, coord.y as u16, color.is_on());
      }
    }
    Ok(())
  }
}

impl<'d, SPI, CS> OriginDimensions for Ls027b7dh01<'d, SPI, CS>
where
  SPI: std::borrow::Borrow<SpiDriver<'d>>,
  CS: esp_idf_svc::hal::gpio::OutputPin,
{
  fn size(&self) -> Size {
    Size::new(WIDTH as u32, HEIGHT as u32)
  }
}
