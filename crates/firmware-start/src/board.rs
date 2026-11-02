//! Board map — Adafruit Feather ESP32 V2 (start node).
//!
//! Every peripheral unit and GPIO the firmware uses is owned here, renamed
//! by function, and handed to main as one [`Board`] struct. A port to
//! another board starts (and ideally ends) in this file; see the "Porting
//! to a new board" checklist in ARCHITECTURE.md.

#[cfg(all(not(feature = "qemu"), not(feature = "camera")))]
use esp_idf_svc::hal::gpio::Gpio26;
#[cfg(not(feature = "qemu"))]
use esp_idf_svc::hal::gpio::{Gpio13, Gpio20, Gpio22, Gpio25, Gpio38};
#[cfg(not(feature = "qemu"))]
use esp_idf_svc::hal::i2c::I2C0;
#[cfg(not(feature = "qemu"))]
use esp_idf_svc::hal::ledc::{CHANNEL0, TIMER0};
#[cfg(feature = "qemu")]
use esp_idf_svc::hal::mac::MAC;
#[cfg(not(feature = "qemu"))]
use esp_idf_svc::hal::modem::Modem;
use esp_idf_svc::hal::peripherals::Peripherals;
#[cfg(all(not(feature = "qemu"), not(feature = "camera")))]
use esp_idf_svc::hal::uart::UART1;

/// Link radio: the ESP-NOW modem on hardware, the OpenCores Ethernet MAC
/// under QEMU (WiFi is not emulated there).
#[cfg(not(feature = "qemu"))]
pub type Radio<'d> = Modem<'d>;
/// Link radio under emulation (QEMU).
#[cfg(feature = "qemu")]
pub type Radio<'d> = MAC<'d>;

/// The board's peripheral units and pins, named by function.
pub struct Board {
    pub radio: Radio<'static>,
    /// SSD1306 OLED I2C bus.
    #[cfg(not(feature = "qemu"))]
    pub display_i2c: I2C0<'static>,
    /// QR/barcode scanner UART (UART1; UART0 is the USB-serial console).
    /// Unused in camera builds (the camera replaces the scanner).
    #[cfg(all(not(feature = "qemu"), not(feature = "camera")))]
    pub scanner_uart: UART1<'static>,
    /// Start trigger: the SW38 user button (to ground when pressed;
    /// input-only pin, pull-up on board).
    #[cfg(not(feature = "qemu"))]
    pub trigger: Gpio38<'static>,
    /// Onboard red LED: on while a measurement is running.
    #[cfg(not(feature = "qemu"))]
    pub led: Gpio13<'static>,
    /// SSD1306 OLED I2C pins (also routed to the STEMMA QT connector).
    #[cfg(not(feature = "qemu"))]
    pub display_sda: Gpio22<'static>,
    #[cfg(not(feature = "qemu"))]
    pub display_scl: Gpio20<'static>,
    /// QR/barcode scanner UART RX (labelled A0; scanner TX wires here).
    /// Unused in camera builds (the camera replaces the scanner).
    #[cfg(all(not(feature = "qemu"), not(feature = "camera")))]
    pub scanner_rx: Gpio26<'static>,
    /// Start-cue buzzer: passive piezo driven by LEDC PWM (timer 0,
    /// channel 0).
    #[cfg(not(feature = "qemu"))]
    pub buzzer_timer: TIMER0<'static>,
    #[cfg(not(feature = "qemu"))]
    pub buzzer_channel: CHANNEL0<'static>,
    /// Buzzer pin. GPIO25 is free in both scanner and camera builds (the
    /// old prototype's buzzer pin A0/GPIO26 is the scanner RX here).
    #[cfg(not(feature = "qemu"))]
    pub buzzer: Gpio25<'static>,
}

/// Consume the raw peripherals and produce the function-named board map.
pub fn take(p: Peripherals) -> Board {
    Board {
        radio: {
            #[cfg(not(feature = "qemu"))]
            {
                p.modem
            }
            #[cfg(feature = "qemu")]
            {
                p.mac
            }
        },
        #[cfg(not(feature = "qemu"))]
        display_i2c: p.i2c0,
        #[cfg(all(not(feature = "qemu"), not(feature = "camera")))]
        scanner_uart: p.uart1,
        #[cfg(not(feature = "qemu"))]
        trigger: p.pins.gpio38,
        #[cfg(not(feature = "qemu"))]
        led: p.pins.gpio13,
        #[cfg(not(feature = "qemu"))]
        display_sda: p.pins.gpio22,
        #[cfg(not(feature = "qemu"))]
        display_scl: p.pins.gpio20,
        #[cfg(all(not(feature = "qemu"), not(feature = "camera")))]
        scanner_rx: p.pins.gpio26,
        #[cfg(not(feature = "qemu"))]
        buzzer_timer: p.ledc.timer0,
        #[cfg(not(feature = "qemu"))]
        buzzer_channel: p.ledc.channel0,
        #[cfg(not(feature = "qemu"))]
        buzzer: p.pins.gpio25,
    }
}

/// Image sensor wiring, used only when built with `--features camera`
#[cfg(all(not(feature = "qemu"), feature = "camera"))]
pub const CAMERA_PINS: firmware_common::CameraPins = firmware_common::CameraPins {
    pwdn: -1,
    reset: -1,
    xclk: 32,
    sccb_sda: 26,
    sccb_scl: 27,
    d7: 36,
    d6: 39,
    d5: 34,
    d4: 4,
    d3: 25,
    d2: 33,
    d1: 15,
    d0: 14,
    vsync: 12,
    href: 21,
    pclk: 5,
};
