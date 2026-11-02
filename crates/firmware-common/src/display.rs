//! The start node's 128x64 screen.
//!
//! Two render targets are supported, selected by the `qemu` cargo feature:
//! * default: a physical SSD1306 OLED over I2C
//! * `qemu`: the virtual RGB panel of Espressif's QEMU fork (not a real
//!   peripheral; pixel data is written to its dedicated framebuffer and
//!   shown in an SDL window when QEMU runs with `-display sdl`)

use std::string::String;

use embedded_graphics::mono_font::ascii::{FONT_10X20, FONT_6X10};
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use embedded_graphics::text::{Baseline, Text};

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

#[cfg(not(feature = "qemu"))]
mod imp {
    use super::*;

    use esp_idf_svc::hal::gpio::{InputPin, OutputPin};
    use esp_idf_svc::hal::i2c::{config::Config as I2cConfig, I2c, I2cDriver};
    use esp_idf_svc::hal::units::Hertz;
    use esp_idf_svc::sys::{EspError, ESP_FAIL};

    use ssd1306::mode::BufferedGraphicsMode;
    use ssd1306::prelude::*;
    use ssd1306::{I2CDisplayInterface, Ssd1306};

    /// 7-bit I2C address of the SSD1306 (try 0x3D if the screen stays dark).
    pub const SCREEN_I2C_ADDR: u8 = 0x3C;

    /// I2C clock; the full 128x64 framebuffer is 1 KiB, so a flush takes
    /// ~21 ms at this speed.
    const I2C_BAUDRATE: Hertz = Hertz(400_000);

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

    impl Display<Oled> {
        /// Bring up the given I2C peripheral and initialize the screen.
        pub fn new(
            i2c: impl I2c + 'static,
            sda: impl InputPin + OutputPin + 'static,
            scl: impl InputPin + OutputPin + 'static,
        ) -> Result<Self, EspError> {
            let config = I2cConfig::new().baudrate(I2C_BAUDRATE);
            let driver = I2cDriver::new(i2c, sda, scl, &config)?;
            let interface = I2CDisplayInterface::new_custom_address(driver, SCREEN_I2C_ADDR);
            let mut oled = Ssd1306::new(interface, DisplaySize128x64, DisplayRotation::Rotate0)
                .into_buffered_graphics_mode();
            // display_interface::DisplayError has no EspError conversion.
            oled.init()
                .map_err(|_| EspError::from_infallible::<ESP_FAIL>())?;
            Ok(Self { target: oled })
        }
    }
}

#[cfg(not(feature = "qemu"))]
pub use imp::SCREEN_I2C_ADDR;

/// Render target for Espressif QEMU's virtual RGB panel: pixel data is
/// written straight into its dedicated framebuffer (no guest RAM cost) and
/// blitted to the QEMU window on present.
#[cfg(feature = "qemu")]
mod imp {
    use super::*;
    use core::convert::Infallible;

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
    /// Logical screen size (matches the SSD1306).
    const SCREEN_W: u32 = 128;
    const SCREEN_H: u32 = 64;
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
            // Hand the frame to QEMU and return immediately. The device
            // latches these registers and performs the copy from its
            // display-refresh callback, which only runs when QEMU has a
            // graphical display (-display sdl)
            //
            // waiting for the completion flag would hang headless runs. If we
            // present again before a refresh happens, the latest frame simply
            // wins
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

    impl Display<QemuPanel> {
        /// Initialize the virtual panel. Needs no pins or peripherals.
        pub fn new_qemu_panel() -> Self {
            Self {
                target: QemuPanel::new(),
            }
        }
    }
}
