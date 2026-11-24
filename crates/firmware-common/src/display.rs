//! The start node's screen.
//!
//! The UI draws into a logical 128x64 monochrome canvas shared by all
//! backends; the connection struct chosen in the node's board.rs decides
//! where the pixels land:
//!
//! * [`I2cConnection`] (default): an external SSD1306 OLED over I2C
//! * [`SpiConnection`] (feature `tft-display`, ESP32-S3 only): the
//!   Feather TFT ESP32-S3's onboard 240x135 ST7789
//! * [`QemuConnection`] (feature `qemu`): the virtual RGB panel of
//!   Espressif's QEMU fork (not a real peripheral; pixel data is written
//!   to its dedicated framebuffer and shown in an SDL window when QEMU
//!   runs with `-display sdl`)

use std::string::String;

use embedded_graphics::mono_font::ascii::{FONT_10X20, FONT_6X10};
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use embedded_graphics::text::{Baseline, Text};

/// Logical screen size the UI is laid out for (the SSD1306 gets its size
/// from its own driver; the TFT stretches, the QEMU panel upscales).
#[cfg(any(feature = "qemu", all(feature = "tft-display", esp32s3)))]
const SCREEN_W: u32 = 128;
#[cfg(any(feature = "qemu", all(feature = "tft-display", esp32s3)))]
const SCREEN_H: u32 = 64;

const SMALL_TEXT: MonoTextStyle<'static, BinaryColor> =
    MonoTextStyle::new(&FONT_6X10, BinaryColor::On);
const BIG_TEXT: MonoTextStyle<'static, BinaryColor> =
    MonoTextStyle::new(&FONT_10X20, BinaryColor::On);

/// Pushes the drawn framebuffer to the physical (or virtual) screen.
pub trait Present {
    fn present(&mut self);
}

pub struct Display<T: DrawTarget<Color = BinaryColor> + Present> {
    target: T,
}

impl<T: DrawTarget<Color = BinaryColor> + Present> Display<T> {
    /// Idle/armed screen: player identity and a start hint.
    pub fn show_idle(&mut self, player: Option<&str>) {
        let _ = self.target.clear(BinaryColor::Off);
        self.text((0, 2), "Ready to go!", SMALL_TEXT);
        let player_line = match player {
            Some(name) => std::format!("Player: {name}"),
            None => String::from("Player: -"),
        };
        self.text((0, 18), &player_line, SMALL_TEXT);
        self.text((0, 54), "Press start to run", SMALL_TEXT);
        self.target.present();
    }

    /// Shown while clock sync with the end node is being established.
    pub fn show_syncing(&mut self) {
        let _ = self.target.clear(BinaryColor::Off);
        self.text((0, 2), "Syncing clocks...", SMALL_TEXT);
        self.text((0, 18), "Please wait", SMALL_TEXT);
        self.target.present();
    }

    /// Countdown cue shown while the 3-2-1-go beeps play before a run.
    pub fn show_countdown(&mut self, secs: u8) {
        let _ = self.target.clear(BinaryColor::Off);
        self.text((0, 2), "Get ready...", SMALL_TEXT);
        let digit = std::format!("{secs}");
        self.text((58, 20), &digit, BIG_TEXT);
        self.target.present();
    }

    /// Live counting timer while a measurement is running.
    pub fn show_running(&mut self, elapsed_us: i64) {
        let _ = self.target.clear(BinaryColor::Off);
        self.text((0, 2), "Timing...", SMALL_TEXT);
        let time = format_secs_millis(elapsed_us);
        self.text((0, 22), &time, BIG_TEXT);
        self.target.present();
    }

    /// Final elapsed time after a run.
    pub fn show_result(&mut self, elapsed_us: i64) {
        let _ = self.target.clear(BinaryColor::Off);
        self.text((0, 2), "Time:", SMALL_TEXT);
        let time = format_secs_millis(elapsed_us);
        self.text((0, 22), &time, BIG_TEXT);
        self.target.present();
    }

    /// Draw `content` at the top of line `y` (pixels from the top).
    fn text(&mut self, pos: (i32, i32), content: &str, style: MonoTextStyle<'static, BinaryColor>) {
        // Drawing into a framebuffer is infallible for both targets.
        let _ = Text::with_baseline(content, Point::new(pos.0, pos.1), style, Baseline::Top)
            .draw(&mut self.target);
    }
}

/// Formats microseconds as a string `s.mmm`
fn format_secs_millis(elapsed_us: i64) -> String {
    let elapsed_us = elapsed_us.max(0);
    let secs = elapsed_us / 1_000_000;
    let millis = (elapsed_us % 1_000_000) / 1_000;
    std::format!("{secs}.{millis}s")
}

/// External SSD1306 OLED over I2C (default backend). ESP32-S3 +
/// `tft-display` is rejected by firmware-start with a compile_error.
#[cfg(all(not(feature = "qemu"), any(not(feature = "tft-display"), esp32)))]
mod oled {
    use super::*;

    use esp_idf_svc::hal::gpio::{InputPin, OutputPin, PinDriver};
    use esp_idf_svc::hal::i2c::{config::Config as I2cConfig, I2c, I2cDriver};
    use esp_idf_svc::hal::units::Hertz;
    use esp_idf_svc::sys::{EspError, ESP_FAIL};

    use ssd1306::mode::BufferedGraphicsMode;
    use ssd1306::prelude::*;
    use ssd1306::{I2CDisplayInterface, Ssd1306};

    use crate::Connect;

    /// 7-bit I2C address of the SSD1306 (try 0x3D if the screen stays dark).
    pub const SCREEN_I2C_ADDR: u8 = 0x3C;

    /// I2C clock; the full 128x64 framebuffer is 1 KiB, so a flush takes
    /// ~21 ms at this speed.
    const I2C_BAUDRATE: Hertz = Hertz(400_000);

    /// Placeholder `PWR` type for boards without a display power switch
    /// (`power: None`). Never instantiated.
    pub type NoPowerPin = esp_idf_svc::hal::gpio::Gpio0<'static>;

    /// External SSD1306 OLED on an I2C bus. `power` is the board's
    /// display/I2C power switch (if any), driven high before bus init.
    pub struct I2cConnection<I2C, SDA, SCL, PWR> {
        pub i2c: I2C,
        pub sda: SDA,
        pub scl: SCL,
        pub power: Option<PWR>,
    }

    type Oled = Ssd1306<
        I2CInterface<I2cDriver<'static>>,
        DisplaySize128x64,
        BufferedGraphicsMode<DisplaySize128x64>,
    >;

    impl Present for Oled {
        fn present(&mut self) {
            if let Err(e) = self.flush() {
                log::warn!("oled flush failed: {e:?}");
            }
        }
    }

    impl<I2C, SDA, SCL, PWR> Connect for I2cConnection<I2C, SDA, SCL, PWR>
    where
        I2C: I2c + 'static,
        SDA: InputPin + OutputPin + 'static,
        SCL: InputPin + OutputPin + 'static,
        PWR: OutputPin + 'static,
    {
        type Output = Display<Oled>;
        fn connect(self) -> Result<Self::Output, EspError> {
            if let Some(power) = self.power {
                let mut pwr = PinDriver::output(power)?;
                pwr.set_high()?;
                // The bus dies if the handle drops; this runs once at
                // boot, so leak the handle to keep the pin driven.
                let _ = Box::leak(Box::new(pwr));
            }
            let config = I2cConfig::new().baudrate(I2C_BAUDRATE);
            let driver = I2cDriver::new(self.i2c, self.sda, self.scl, &config)?;
            let interface = I2CDisplayInterface::new_custom_address(driver, SCREEN_I2C_ADDR);
            let mut oled = Ssd1306::new(interface, DisplaySize128x64, DisplayRotation::Rotate0)
                .into_buffered_graphics_mode();
            // display_interface::DisplayError has no EspError conversion.
            oled.init()
                .map_err(|_| EspError::from_infallible::<ESP_FAIL>())?;
            Ok(Display { target: oled })
        }
    }
}

/// The Feather TFT ESP32-S3's onboard 240x135 ST7789 over SPI (feature
/// `tft-display`).
#[cfg(all(not(feature = "qemu"), feature = "tft-display", esp32s3))]
mod tft {
    use super::*;
    use core::convert::Infallible;

    use embedded_graphics::pixelcolor::raw::RawU16;
    use embedded_graphics::pixelcolor::Rgb565;

    use esp_idf_svc::hal::delay::Ets;
    use esp_idf_svc::hal::gpio::{Gpio0, Output, OutputPin, PinDriver};
    use esp_idf_svc::hal::spi::config::{
        Config as SpiDeviceConfig, DriverConfig as SpiDriverConfig,
    };
    use esp_idf_svc::hal::spi::{SpiAnyPins, SpiDeviceDriver, SpiDriver};
    use esp_idf_svc::hal::units::Hertz;
    use esp_idf_svc::sys::{EspError, ESP_FAIL};

    use mipidsi::interface::SpiInterface;
    use mipidsi::models::ST7789;
    use mipidsi::options::{ColorInversion, Orientation, Rotation};
    use mipidsi::Builder;

    use crate::player::PreviewFrame;
    use crate::Connect;

    /// SPI clock for the panel.
    const SPI_BAUDRATE: Hertz = Hertz(40_000_000);
    /// Scratch buffer the SPI interface batches pixel data into (bigger =
    /// fewer transactions, diminishing returns). Leaked once at boot.
    const SPI_BUF_LEN: usize = 4096;

    /// Physical panel resolution in the logical (landscape) orientation
    /// the 90-degree rotation produces.
    const PANEL_W: u16 = 240;
    const PANEL_H: u16 = 135;
    /// The visible window in the controller's native (portrait) space:
    /// 135x240 at (52, 40) — the standard 1.14" values.
    const NATIVE_W: u16 = 135;
    const NATIVE_H: u16 = 240;
    const OFFSET_X: u16 = 52;
    const OFFSET_Y: u16 = 40;

    /// RGB565 pixel values for the monochrome UI.
    const COLOR_ON: u16 = 0xFFFF; // white
    const COLOR_OFF: u16 = 0x0000; // black

    /// Physical pixels per full-screen present().
    const PHYS_PIXELS: usize = PANEL_W as usize * PANEL_H as usize;
    /// Logical pixels in the framebuffer (SCREEN_W x SCREEN_H).
    const LOGICAL_PIXELS: usize = SCREEN_W as usize * SCREEN_H as usize;
    /// Logical framebuffer size in bytes (1 bit per pixel).
    const LOGICAL_BYTES: usize = LOGICAL_PIXELS / 8;

    /// The onboard 240x135 ST7789 on SPI. `power` is the board's
    /// TFT/I2C power switch, driven high before panel init.
    pub struct SpiConnection<SPI, SCK, MOSI, CS, DC, RST, BL, PWR> {
        pub spi: SPI,
        pub sck: SCK,
        pub mosi: MOSI,
        pub cs: CS,
        pub dc: DC,
        pub rst: RST,
        pub backlight: BL,
        pub power: Option<PWR>,
    }

    type TftInterface = SpiInterface<
        'static,
        SpiDeviceDriver<'static, SpiDriver<'static>>,
        PinDriver<'static, Output>,
    >;

    /// Render target for the ST7789: the logical 128x64 canvas is a 1-bit
    /// bitmap (1 KiB); present() stretches+colorizes it onto the panel.
    pub struct TftPanel {
        display: mipidsi::Display<TftInterface, ST7789, PinDriver<'static, Output>>,
        _backlight: PinDriver<'static, Output>,
        buf: Box<[u8]>,
        /// Staging copy of the scanner preview frame, so the shared slot
        /// isn't locked during text compositing and the SPI blit.
        scan_buf: Vec<u8>,
    }

    impl DrawTarget for TftPanel {
        type Color = BinaryColor;
        type Error = Infallible;

        fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
        where
            I: IntoIterator<Item = Pixel<Self::Color>>,
        {
            for Pixel(point, color) in pixels {
                if point.x < 0
                    || point.y < 0
                    || point.x as u32 >= SCREEN_W
                    || point.y as u32 >= SCREEN_H
                {
                    continue;
                }
                let (x, y) = (point.x as usize, point.y as usize);
                let byte = &mut self.buf[y * (SCREEN_W as usize / 8) + x / 8];
                if color.is_on() {
                    *byte |= 1 << (x % 8);
                } else {
                    *byte &= !(1 << (x % 8));
                }
            }
            Ok(())
        }

        fn clear(&mut self, color: Self::Color) -> Result<(), Self::Error> {
            self.buf.fill(if color.is_on() { 0xFF } else { 0x00 });
            Ok(())
        }
    }

    impl OriginDimensions for TftPanel {
        fn size(&self) -> Size {
            Size::new(SCREEN_W, SCREEN_H)
        }
    }

    impl Present for TftPanel {
        fn present(&mut self) {
            // Nearest-neighbor stretch + 1-bit-to-RGB565 colorization;
            // the lazy iterator lets mipidsi batch into its SPI scratch buffer.
            let buf = &self.buf;
            let iter = (0..PHYS_PIXELS).map(|i| {
                let px = i % PANEL_W as usize;
                let py = i / PANEL_W as usize;
                let lx = px * SCREEN_W as usize / PANEL_W as usize;
                let ly = py * SCREEN_H as usize / PANEL_H as usize;
                let bit = (buf[ly * (SCREEN_W as usize / 8) + lx / 8] >> (lx % 8)) & 1;
                Rgb565::from(RawU16::new(if bit != 0 { COLOR_ON } else { COLOR_OFF }))
            });
            if let Err(e) = self
                .display
                .set_pixels(0, 0, PANEL_W - 1, PANEL_H - 1, iter)
            {
                log::warn!("tft flush failed: {e:?}");
            }
        }
    }

    /// Tiny 1-bit-bitmap DrawTarget for compositing text onto the scanner
    /// preview before blitting (LSB-first bytes, like the UI canvas).
    struct BitBuf<'a> {
        w: usize,
        h: usize,
        buf: &'a mut [u8],
    }

    impl DrawTarget for BitBuf<'_> {
        type Color = BinaryColor;
        type Error = Infallible;

        fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
        where
            I: IntoIterator<Item = Pixel<Self::Color>>,
        {
            for Pixel(point, color) in pixels {
                if point.x < 0
                    || point.y < 0
                    || point.x as usize >= self.w
                    || point.y as usize >= self.h
                {
                    continue;
                }
                let (x, y) = (point.x as usize, point.y as usize);
                let byte = &mut self.buf[y * (self.w / 8) + x / 8];
                if color.is_on() {
                    *byte |= 1 << (x % 8);
                } else {
                    *byte &= !(1 << (x % 8));
                }
            }
            Ok(())
        }
    }

    impl OriginDimensions for BitBuf<'_> {
        fn size(&self) -> Size {
            Size::new(self.w as u32, self.h as u32)
        }
    }

    impl Display<TftPanel> {
        /// Scanner-aiming screen: the camera's live binarized view,
        /// centered, with the player line composited at the bottom.
        pub fn show_scanner(&mut self, frame: &PreviewFrame, player: Option<&str>) {
            self.target.present_scanner(frame, player);
        }
    }

    impl TftPanel {
        fn present_scanner(&mut self, frame: &PreviewFrame, player: Option<&str>) {
            let (fw, fh) = (frame.w, frame.h);
            debug_assert_eq!(frame.bits.len(), fw * fh / 8);
            self.scan_buf.clear();
            self.scan_buf.extend_from_slice(&frame.bits);

            // Bottom strip: black it, then write the player line into it.
            let strip_y = fh.saturating_sub(12);
            self.scan_buf[strip_y * (fw / 8)..].fill(0);
            let line = match player {
                Some(name) if !name.is_empty() => std::format!("Player: {name}"),
                _ => String::from("Scan your card"),
            };
            {
                let mut bits = BitBuf {
                    w: fw,
                    h: fh,
                    buf: &mut self.scan_buf,
                };
                let _ = Text::with_baseline(
                    &line,
                    Point::new(2, strip_y as i32 + 1),
                    SMALL_TEXT,
                    Baseline::Top,
                )
                .draw(&mut bits);
            }

            // Blit the whole panel in one address window: the image
            // centered horizontally, everything around it black.
            let x0 = (PANEL_W as usize - fw) / 2;
            let buf = &self.scan_buf;
            let iter = (0..PHYS_PIXELS).map(|i| {
                let px = i % PANEL_W as usize;
                let py = i / PANEL_W as usize;
                let on = py < fh
                    && px >= x0
                    && px < x0 + fw
                    && (buf[py * (fw / 8) + (px - x0) / 8] >> ((px - x0) % 8)) & 1 != 0;
                Rgb565::from(RawU16::new(if on { COLOR_ON } else { COLOR_OFF }))
            });
            if let Err(e) = self
                .display
                .set_pixels(0, 0, PANEL_W - 1, PANEL_H - 1, iter)
            {
                log::warn!("tft scanner flush failed: {e:?}");
            }
        }
    }

    impl<SPI, SCK, MOSI, CS, DC, RST, BL, PWR> Connect
        for SpiConnection<SPI, SCK, MOSI, CS, DC, RST, BL, PWR>
    where
        SPI: SpiAnyPins + 'static,
        SCK: OutputPin + 'static,
        MOSI: OutputPin + 'static,
        CS: OutputPin + 'static,
        DC: OutputPin + 'static,
        RST: OutputPin + 'static,
        BL: OutputPin + 'static,
        PWR: OutputPin + 'static,
    {
        type Output = Display<TftPanel>;
        fn connect(self) -> Result<Self::Output, EspError> {
            if let Some(power) = self.power {
                let mut pwr = PinDriver::output(power)?;
                pwr.set_high()?;
                // The panel dies if the handle drops; this runs once at
                // boot, so leak the handle to keep the pin driven.
                let _ = Box::leak(Box::new(pwr));
            }

            // No MISO: the ST7789 is write-only; its MISO header pin
            // (GPIO37) stays with the buzzer.
            let spi = SpiDeviceDriver::new_single(
                self.spi,
                self.sck,
                self.mosi,
                None::<Gpio0>,
                Some(self.cs),
                &SpiDriverConfig::new(),
                &SpiDeviceConfig::new().baudrate(SPI_BAUDRATE),
            )?;
            // The scratch buffer lives as long as the interface
            // ('static by leak, allocated once at boot).
            let di = SpiInterface::new(
                spi,
                PinDriver::output(self.dc)?,
                Box::leak(vec![0u8; SPI_BUF_LEN].into_boxed_slice()),
            );
            let display = Builder::new(ST7789, di)
                .display_size(NATIVE_W, NATIVE_H)
                .display_offset(OFFSET_X, OFFSET_Y)
                .orientation(Orientation::new().rotate(Rotation::Deg90))
                .invert_colors(ColorInversion::Inverted)
                .reset_pin(PinDriver::output(self.rst)?)
                .init(&mut Ets)
                // mipidsi's InitError has no EspError conversion.
                .map_err(|_| EspError::from_infallible::<ESP_FAIL>())?;

            let mut backlight = PinDriver::output(self.backlight)?;
            backlight.set_high()?;

            Ok(Display {
                target: TftPanel {
                    display,
                    _backlight: backlight,
                    buf: vec![0u8; LOGICAL_BYTES].into_boxed_slice(),
                    scan_buf: Vec::new(),
                },
            })
        }
    }
}

/// Render target for Espressif QEMU's virtual RGB panel: pixels go
/// straight into its framebuffer, blitted to the window on present.
#[cfg(feature = "qemu")]
mod qemu_panel {
    use super::*;
    use core::convert::Infallible;

    use esp_idf_svc::sys::EspError;

    use crate::Connect;

    /// No pins: pixels go to the virtual panel's framebuffer.
    pub struct QemuConnection;

    impl Connect for QemuConnection {
        type Output = Display<QemuPanel>;
        fn connect(self) -> Result<Self::Output, EspError> {
            Ok(Display {
                target: QemuPanel::new(),
            })
        }
    }

    /// Register block of the virtual RGB panel.
    const REGS: *mut u32 = 0x2100_0000 as *mut u32;
    /// The panel's dedicated framebuffer.
    const VRAM: *mut u32 = 0x2000_0000 as *mut u32;

    // Register word offsets (layout from espressif/idf-extra-components'
    // esp_lcd_qemu_rgb driver).
    const REG_VERSION: usize = 0;
    const REG_SIZE: usize = 1;
    const REG_UPDATE_FROM: usize = 2;
    const REG_UPDATE_TO: usize = 3;
    const REG_UPDATE_CONTENT: usize = 4;
    const REG_UPDATE_ST: usize = 5;
    const REG_BPP: usize = 6;

    const BPP_ARGB8888: u32 = 32;
    /// Integer upscale of the logical screen for a viewable host window.
    const SCALE: usize = 4;
    const WIN_W: usize = SCREEN_W as usize * SCALE;
    const WIN_H: usize = SCREEN_H as usize * SCALE;

    const COLOR_ON: u32 = 0xFFFF_FFFF; // opaque white
    const COLOR_OFF: u32 = 0xFF00_0000; // opaque black

    /// Cap on VRAM bounds: the device allows at most 800x600x4 bytes.
    const _: () = assert!(WIN_W * WIN_H * 4 <= 800 * 600 * 4);

    pub struct QemuPanel {
        _private: (),
    }

    impl QemuPanel {
        fn new() -> Self {
            unsafe {
                let version = REGS.add(REG_VERSION).read_volatile();
                log::debug!("qemu rgb panel: version register {version:#010x}");
                // size: height in the low half, width in the high half.
                REGS.add(REG_SIZE)
                    .write_volatile(((WIN_W as u32) << 16) | WIN_H as u32);
                REGS.add(REG_BPP).write_volatile(BPP_ARGB8888);
            }
            Self { _private: () }
        }
    }

    impl DrawTarget for QemuPanel {
        type Color = BinaryColor;
        type Error = Infallible;

        fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
        where
            I: IntoIterator<Item = Pixel<Self::Color>>,
        {
            for Pixel(point, color) in pixels {
                if point.x < 0
                    || point.y < 0
                    || point.x as u32 >= SCREEN_W
                    || point.y as u32 >= SCREEN_H
                {
                    continue;
                }
                let (x, y) = (point.x as usize, point.y as usize);
                let word = if color.is_on() { COLOR_ON } else { COLOR_OFF };
                for dy in 0..SCALE {
                    for dx in 0..SCALE {
                        let offset = (y * SCALE + dy) * WIN_W + x * SCALE + dx;
                        unsafe { VRAM.add(offset).write_volatile(word) };
                    }
                }
            }
            Ok(())
        }

        fn clear(&mut self, color: Self::Color) -> Result<(), Self::Error> {
            let word = if color.is_on() { COLOR_ON } else { COLOR_OFF };
            for i in 0..WIN_W * WIN_H {
                unsafe { VRAM.add(i).write_volatile(word) };
            }
            Ok(())
        }
    }

    impl OriginDimensions for QemuPanel {
        fn size(&self) -> Size {
            Size::new(SCREEN_W, SCREEN_H)
        }
    }

    impl Present for QemuPanel {
        fn present(&mut self) {
            // Hand the frame to QEMU and return immediately: the copy runs
            // from the device's refresh callback (only with -display sdl).
            unsafe {
                // update_from/update_to: y in the low half, x in the high
                // half; the end coordinate is exclusive.
                REGS.add(REG_UPDATE_FROM).write_volatile(0);
                REGS.add(REG_UPDATE_TO)
                    .write_volatile(((WIN_W as u32) << 16) | WIN_H as u32);
                REGS.add(REG_UPDATE_CONTENT).write_volatile(VRAM as u32);
                REGS.add(REG_UPDATE_ST).write_volatile(1);
            }
        }
    }
}

#[cfg(all(not(feature = "qemu"), any(not(feature = "tft-display"), esp32)))]
pub use oled::{I2cConnection, NoPowerPin, SCREEN_I2C_ADDR};
#[cfg(feature = "qemu")]
pub use qemu_panel::QemuConnection;
#[cfg(all(not(feature = "qemu"), feature = "tft-display", esp32s3))]
pub use tft::SpiConnection;
