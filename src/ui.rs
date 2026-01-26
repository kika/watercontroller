//! Display UI components for water controller
//!
//! - Water tank visualization with fill level and text overlay
//! - Analog pressure gauge (manometer) with digital readout

use embedded_graphics::{
    draw_target::DrawTarget,
    geometry::{Point, Size},
    mono_font::{MonoTextStyle, MonoTextStyleBuilder, ascii::FONT_6X10, ascii::FONT_10X20},
    pixelcolor::BinaryColor,
    prelude::*,
    primitives::{Circle, Line, PrimitiveStyle, Rectangle},
    text::{Alignment, Text, TextStyleBuilder},
};

/// Water tank visualization
pub struct WaterTank {
    /// Top-left corner position
    pub position: Point,
    /// Tank dimensions (width, height)
    pub size: Size,
    /// Current fill percentage (0-100)
    pub fill_percent: u8,
    /// Current volume in gallons
    pub gallons: u16,
}

impl WaterTank {
    pub fn new(position: Point, size: Size) -> Self {
        Self {
            position,
            size,
            fill_percent: 0,
            gallons: 0,
        }
    }

    pub fn set_level(&mut self, percent: u8, gallons: u16) {
        self.fill_percent = percent.min(100);
        self.gallons = gallons;
    }

    pub fn draw<D>(&self, display: &mut D) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = BinaryColor>,
    {
        let x = self.position.x;
        let y = self.position.y;
        let w = self.size.width as i32;
        let h = self.size.height as i32;

        // Calculate fill height
        let fill_height = (h * self.fill_percent as i32) / 100;
        let fill_top = y + h - fill_height;

        // Draw filled water portion (black = water)
        if fill_height > 0 {
            Rectangle::new(
                Point::new(x, fill_top),
                Size::new(self.size.width, fill_height as u32),
            )
            .into_styled(PrimitiveStyle::with_fill(BinaryColor::Off))
            .draw(display)?;
        }

        // Draw tank outline
        Rectangle::new(self.position, self.size)
            .into_styled(PrimitiveStyle::with_stroke(BinaryColor::Off, 2))
            .draw(display)?;

        // Draw text overlay
        // Calculate center of tank for text placement
        let center_x = x + w / 2;
        let text_y_percent = y + h / 2 - 10;
        let text_y_gallons = y + h / 2 + 15;

        // Format text
        let mut percent_buf = [0u8; 8];
        let percent_str = format_percent(self.fill_percent, &mut percent_buf);

        let mut gallons_buf = [0u8; 12];
        let gallons_str = format_gallons(self.gallons, &mut gallons_buf);

        let text_style = TextStyleBuilder::new().alignment(Alignment::Center).build();

        // Draw percentage - determine color based on position relative to water level
        let percent_color = if text_y_percent > fill_top {
            BinaryColor::On // White text on black water
        } else {
            BinaryColor::Off // Black text on white background
        };
        let percent_font = MonoTextStyleBuilder::new()
            .font(&FONT_10X20)
            .text_color(percent_color)
            .build();
        Text::with_text_style(percent_str, Point::new(center_x, text_y_percent), percent_font, text_style)
            .draw(display)?;

        // Draw gallons - determine color based on position relative to water level
        let gallons_color = if text_y_gallons > fill_top {
            BinaryColor::On // White text on black water
        } else {
            BinaryColor::Off // Black text on white background
        };
        let gallons_font = MonoTextStyleBuilder::new()
            .font(&FONT_10X20)
            .text_color(gallons_color)
            .build();
        Text::with_text_style(gallons_str, Point::new(center_x, text_y_gallons), gallons_font, text_style)
            .draw(display)?;

        Ok(())
    }
}

/// Analog pressure gauge (manometer)
pub struct Manometer {
    /// Center position
    pub center: Point,
    /// Radius of the gauge
    pub radius: i32,
    /// Current pressure in PSI
    pub pressure_psi: u16,
    /// Maximum pressure (for scale)
    pub max_psi: u16,
}

impl Manometer {
    pub fn new(center: Point, radius: i32) -> Self {
        Self {
            center,
            radius,
            pressure_psi: 0,
            max_psi: 150,
        }
    }

    pub fn set_pressure(&mut self, psi: u16) {
        self.pressure_psi = psi.min(self.max_psi);
    }

    pub fn draw<D>(&self, display: &mut D) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = BinaryColor>,
    {
        // Draw outer circle
        Circle::new(
            Point::new(self.center.x - self.radius, self.center.y - self.radius),
            (self.radius * 2) as u32,
        )
        .into_styled(PrimitiveStyle::with_stroke(BinaryColor::Off, 2))
        .draw(display)?;

        // Draw tick marks and labels
        // Gauge arc from 225° (min) to -45° (max) = 270° sweep
        // 0 PSI at 225°, 150 PSI at -45° (315°)
        let start_angle: f32 = 225.0;
        let end_angle: f32 = -45.0;
        let sweep = start_angle - end_angle; // 270 degrees

        // Major ticks every 30 PSI (6 ticks: 0, 30, 60, 90, 120, 150)
        for i in 0..=5 {
            let psi = i * 30;
            let angle_deg = start_angle - (psi as f32 / self.max_psi as f32) * sweep;
            let angle_rad = angle_deg * core::f32::consts::PI / 180.0;

            let cos_a = libm::cosf(angle_rad);
            let sin_a = libm::sinf(angle_rad);

            // Tick mark from 85% to 95% of radius
            let inner_r = (self.radius as f32 * 0.80) as i32;
            let outer_r = (self.radius as f32 * 0.95) as i32;

            let x1 = self.center.x + (cos_a * inner_r as f32) as i32;
            let y1 = self.center.y - (sin_a * inner_r as f32) as i32;
            let x2 = self.center.x + (cos_a * outer_r as f32) as i32;
            let y2 = self.center.y - (sin_a * outer_r as f32) as i32;

            Line::new(Point::new(x1, y1), Point::new(x2, y2))
                .into_styled(PrimitiveStyle::with_stroke(BinaryColor::Off, 2))
                .draw(display)?;

            // Draw label
            let label_r = (self.radius as f32 * 0.65) as i32;
            let label_x = self.center.x + (cos_a * label_r as f32) as i32;
            let label_y = self.center.y - (sin_a * label_r as f32) as i32;

            let mut label_buf = [0u8; 4];
            let label_str = format_number(psi as u16, &mut label_buf);

            let label_style = MonoTextStyle::new(&FONT_6X10, BinaryColor::Off);
            let text_style = TextStyleBuilder::new().alignment(Alignment::Center).build();
            Text::with_text_style(label_str, Point::new(label_x, label_y + 3), label_style, text_style)
                .draw(display)?;
        }

        // Minor ticks every 10 PSI
        for i in 0..=15 {
            let psi = i * 10;
            if psi % 30 == 0 {
                continue; // Skip major tick positions
            }
            let angle_deg = start_angle - (psi as f32 / self.max_psi as f32) * sweep;
            let angle_rad = angle_deg * core::f32::consts::PI / 180.0;

            let cos_a = libm::cosf(angle_rad);
            let sin_a = libm::sinf(angle_rad);

            let inner_r = (self.radius as f32 * 0.88) as i32;
            let outer_r = (self.radius as f32 * 0.95) as i32;

            let x1 = self.center.x + (cos_a * inner_r as f32) as i32;
            let y1 = self.center.y - (sin_a * inner_r as f32) as i32;
            let x2 = self.center.x + (cos_a * outer_r as f32) as i32;
            let y2 = self.center.y - (sin_a * outer_r as f32) as i32;

            Line::new(Point::new(x1, y1), Point::new(x2, y2))
                .into_styled(PrimitiveStyle::with_stroke(BinaryColor::Off, 1))
                .draw(display)?;
        }

        // Draw needle
        let pressure_angle_deg = start_angle - (self.pressure_psi as f32 / self.max_psi as f32) * sweep;
        let pressure_angle_rad = pressure_angle_deg * core::f32::consts::PI / 180.0;

        let cos_p = libm::cosf(pressure_angle_rad);
        let sin_p = libm::sinf(pressure_angle_rad);

        let needle_len = (self.radius as f32 * 0.75) as i32;
        let needle_end_x = self.center.x + (cos_p * needle_len as f32) as i32;
        let needle_end_y = self.center.y - (sin_p * needle_len as f32) as i32;

        // Needle line
        Line::new(self.center, Point::new(needle_end_x, needle_end_y))
            .into_styled(PrimitiveStyle::with_stroke(BinaryColor::Off, 2))
            .draw(display)?;

        // Center hub
        Circle::new(
            Point::new(self.center.x - 5, self.center.y - 5),
            10,
        )
        .into_styled(PrimitiveStyle::with_fill(BinaryColor::Off))
        .draw(display)?;

        // Digital readout below center
        let mut psi_buf = [0u8; 8];
        let psi_str = format_psi(self.pressure_psi, &mut psi_buf);

        let psi_style = MonoTextStyle::new(&FONT_10X20, BinaryColor::Off);
        let text_style = TextStyleBuilder::new().alignment(Alignment::Center).build();
        Text::with_text_style(psi_str, Point::new(self.center.x, self.center.y + 35), psi_style, text_style)
            .draw(display)?;

        Ok(())
    }
}

// Helper functions for number formatting without std::fmt

fn format_number(n: u16, buf: &mut [u8]) -> &str {
    if n == 0 {
        buf[0] = b'0';
        return unsafe { core::str::from_utf8_unchecked(&buf[..1]) };
    }

    let mut num = n;
    let mut i = 0;

    // Write digits in reverse
    while num > 0 && i < buf.len() {
        buf[i] = b'0' + (num % 10) as u8;
        num /= 10;
        i += 1;
    }

    // Reverse
    buf[..i].reverse();
    unsafe { core::str::from_utf8_unchecked(&buf[..i]) }
}

fn format_percent(n: u8, buf: &mut [u8]) -> &str {
    let mut i = format_number(n as u16, buf).len();
    buf[i] = b'%';
    i += 1;
    unsafe { core::str::from_utf8_unchecked(&buf[..i]) }
}

fn format_gallons(n: u16, buf: &mut [u8]) -> &str {
    let mut i = format_number(n, buf).len();
    buf[i..i + 4].copy_from_slice(b" gal");
    i += 4;
    unsafe { core::str::from_utf8_unchecked(&buf[..i]) }
}

fn format_psi(n: u16, buf: &mut [u8]) -> &str {
    let mut i = format_number(n, buf).len();
    buf[i..i + 4].copy_from_slice(b" PSI");
    i += 4;
    unsafe { core::str::from_utf8_unchecked(&buf[..i]) }
}
