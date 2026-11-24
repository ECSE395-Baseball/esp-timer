//! Board map — start node.
//!
//! GPIO assignments differ per chip and live in the [`chip`] module
//! the [`Board`] struct below stays chip-agnostic

#[cfg(all(feature = "tft-display", esp32, not(feature = "qemu")))]
compile_error!("tft-display is only supported on the Feather TFT ESP32-S3 (esp32s3)");

#[cfg(all(feature = "camera", feature = "qemu"))]
compile_error!("camera is not supported under QEMU (no emulated camera)");

#[cfg(all(not(feature = "qemu"), any(esp32, not(feature = "tft-display"))))]
use esp_idf_svc::hal::i2c::I2C0;
#[cfg(not(feature = "qemu"))]
use esp_idf_svc::hal::ledc::{CHANNEL0, TIMER0};
#[cfg(feature = "qemu")]
use esp_idf_svc::hal::mac::MAC;
#[cfg(not(feature = "qemu"))]
use esp_idf_svc::hal::modem::Modem;
use esp_idf_svc::hal::peripherals::Peripherals;
#[cfg(all(not(feature = "qemu"), feature = "tft-display", esp32s3))]
use esp_idf_svc::hal::spi::SPI2;

#[cfg(not(feature = "qemu"))]
use chip::take_pins;

/// GPIO assignments: Adafruit Feather ESP32 V2.
#[cfg(all(not(feature = "qemu"), esp32))]
mod chip {
    use esp_idf_svc::hal::gpio::{Gpio13, Gpio20, Gpio22, Gpio25, Gpio34, Pins};

    /// Start trigger: the SW38 user button (to ground when pressed;
    /// input-only pin, pull-up on board).
    pub type TriggerPin<'d> = Gpio34<'d>;
    /// Onboard red LED: on while a measurement is running.
    pub type LedPin<'d> = Gpio13<'d>;
    /// SSD1306 OLED I2C pins (also routed to the STEMMA QT connector).
    pub type DisplaySdaPin<'d> = Gpio22<'d>;
    pub type DisplaySclPin<'d> = Gpio20<'d>;
    /// Buzzer pin (free in both scanner and camera builds).
    pub type BuzzerPin<'d> = Gpio25<'d>;

    /// The chip's GPIOs, renamed by function.
    pub struct ChipPins {
        pub trigger: TriggerPin<'static>,
        pub led: LedPin<'static>,
        pub display_sda: DisplaySdaPin<'static>,
        pub display_scl: DisplaySclPin<'static>,
        pub buzzer: BuzzerPin<'static>,
    }

    pub fn take_pins(p: Pins) -> ChipPins {
        ChipPins {
            trigger: p.gpio34,
            led: p.gpio13,
            display_sda: p.gpio22,
            display_scl: p.gpio20,
            buzzer: p.gpio25,
        }
    }
}

/// GPIO assignments: Adafruit Feather TFT ESP32-S3. Same functions as
/// the V2 map; uses the board's labelled SDA/SCL pins (GPIO42/41).
#[cfg(all(not(feature = "qemu"), esp32s3))]
mod chip {
    #[cfg(not(feature = "tft-display"))]
    use esp_idf_svc::hal::gpio::{Gpio0, Gpio41, Gpio42};
    use esp_idf_svc::hal::gpio::{Gpio13, Gpio21, Gpio37, Pins};
    #[cfg(feature = "tft-display")]
    use esp_idf_svc::hal::gpio::{Gpio35, Gpio36, Gpio39, Gpio40, Gpio41, Gpio45, Gpio7};

    /// Start trigger: the BOOT button (to ground when pressed; internal
    /// pull-up). Holding BOOT during reset enters the ROM bootloader.
    #[cfg(not(feature = "tft-display"))]
    pub type TriggerPin<'d> = Gpio0<'d>;
    /// Start trigger in `tft-display` builds: an external button on the
    /// pin labelled SCL (to ground when pressed), freed by the onboard TFT.
    #[cfg(feature = "tft-display")]
    pub type TriggerPin<'d> = Gpio41<'d>;
    /// Onboard red LED: on while a measurement is running.
    pub type LedPin<'d> = Gpio13<'d>;
    /// SSD1306 OLED I2C pins (the labelled SDA/SCL header pins).
    #[cfg(not(feature = "tft-display"))]
    pub type DisplaySdaPin<'d> = Gpio42<'d>;
    #[cfg(not(feature = "tft-display"))]
    pub type DisplaySclPin<'d> = Gpio41<'d>;
    /// Onboard 240x135 ST7789 (feature `tft-display`): SPI + control pins.
    #[cfg(feature = "tft-display")]
    pub type TftSckPin<'d> = Gpio36<'d>;
    #[cfg(feature = "tft-display")]
    pub type TftMosiPin<'d> = Gpio35<'d>;
    #[cfg(feature = "tft-display")]
    pub type TftCsPin<'d> = Gpio7<'d>;
    #[cfg(feature = "tft-display")]
    pub type TftDcPin<'d> = Gpio39<'d>;
    #[cfg(feature = "tft-display")]
    pub type TftRstPin<'d> = Gpio40<'d>;
    #[cfg(feature = "tft-display")]
    pub type TftBacklightPin<'d> = Gpio45<'d>;
    /// Display/I2C power switch (TFT_I2C_POWER): powers the SDA/SCL
    /// pull-ups, the STEMMA QT connector and the onboard TFT.
    pub type I2cPowerPin<'d> = Gpio21<'d>;
    /// Buzzer pin (labelled MISO): the ST7789 is write-only, so the SPI
    /// bus is configured without MISO and the pin is free in every build.
    pub type BuzzerPin<'d> = Gpio37<'d>;

    /// The chip's GPIOs, renamed by function.
    pub struct ChipPins {
        pub trigger: TriggerPin<'static>,
        pub led: LedPin<'static>,
        #[cfg(not(feature = "tft-display"))]
        pub display_sda: DisplaySdaPin<'static>,
        #[cfg(not(feature = "tft-display"))]
        pub display_scl: DisplaySclPin<'static>,
        #[cfg(feature = "tft-display")]
        pub tft_sck: TftSckPin<'static>,
        #[cfg(feature = "tft-display")]
        pub tft_mosi: TftMosiPin<'static>,
        #[cfg(feature = "tft-display")]
        pub tft_cs: TftCsPin<'static>,
        #[cfg(feature = "tft-display")]
        pub tft_dc: TftDcPin<'static>,
        #[cfg(feature = "tft-display")]
        pub tft_rst: TftRstPin<'static>,
        #[cfg(feature = "tft-display")]
        pub tft_backlight: TftBacklightPin<'static>,
        pub i2c_power: I2cPowerPin<'static>,
        pub buzzer: BuzzerPin<'static>,
    }

    pub fn take_pins(p: Pins) -> ChipPins {
        ChipPins {
            #[cfg(not(feature = "tft-display"))]
            trigger: p.gpio0,
            #[cfg(feature = "tft-display")]
            trigger: p.gpio41,
            led: p.gpio13,
            #[cfg(not(feature = "tft-display"))]
            display_sda: p.gpio42,
            #[cfg(not(feature = "tft-display"))]
            display_scl: p.gpio41,
            #[cfg(feature = "tft-display")]
            tft_sck: p.gpio36,
            #[cfg(feature = "tft-display")]
            tft_mosi: p.gpio35,
            #[cfg(feature = "tft-display")]
            tft_cs: p.gpio7,
            #[cfg(feature = "tft-display")]
            tft_dc: p.gpio39,
            #[cfg(feature = "tft-display")]
            tft_rst: p.gpio40,
            #[cfg(feature = "tft-display")]
            tft_backlight: p.gpio45,
            i2c_power: p.gpio21,
            buzzer: p.gpio37,
        }
    }
}

/// Link radio: the ESP-NOW modem on hardware, the OpenCores Ethernet MAC
/// under QEMU (WiFi is not emulated there).
#[cfg(not(feature = "qemu"))]
pub type Radio<'d> = Modem<'d>;
/// Link radio under emulation (QEMU).
#[cfg(feature = "qemu")]
pub type Radio<'d> = MAC<'d>;

/// Start trigger connection (see [`chip::TriggerPin`]).
#[cfg(not(feature = "qemu"))]
pub type TriggerConnection = firmware_common::trigger::TriggerConnection<chip::TriggerPin<'static>>;
/// Trigger connection under emulation (scheduled virtual trigger).
#[cfg(feature = "qemu")]
pub type TriggerConnection = firmware_common::trigger::TriggerConnection;

/// Onboard red LED connection (see [`chip::LedPin`]).
#[cfg(not(feature = "qemu"))]
pub type LedConnection = firmware_common::led::LedConnection<chip::LedPin<'static>>;
/// LED connection under emulation (no usable GPIO; logs instead).
#[cfg(feature = "qemu")]
pub type LedConnection = firmware_common::led::LedConnection;

/// Buzzer connection: LEDC PWM timer 0 / channel 0 (see
/// [`chip::BuzzerPin`]).
#[cfg(not(feature = "qemu"))]
pub type BuzzerConnection = firmware_common::buzzer::BuzzerConnection<
    TIMER0<'static>,
    CHANNEL0<'static>,
    chip::BuzzerPin<'static>,
>;
/// Buzzer connection under emulation (silent stub).
#[cfg(feature = "qemu")]
pub type BuzzerConnection = firmware_common::buzzer::BuzzerConnection;

/// Display connection: SSD1306 OLED over I2C0 by default, onboard ST7789
/// over SPI2 with `tft-display`, QEMU's virtual RGB panel under emulation.
#[cfg(all(not(feature = "qemu"), esp32))]
pub type DisplayConnection = firmware_common::display::I2cConnection<
    I2C0<'static>,
    chip::DisplaySdaPin<'static>,
    chip::DisplaySclPin<'static>,
    firmware_common::display::NoPowerPin,
>;
#[cfg(all(not(feature = "qemu"), esp32s3, not(feature = "tft-display")))]
pub type DisplayConnection = firmware_common::display::I2cConnection<
    I2C0<'static>,
    chip::DisplaySdaPin<'static>,
    chip::DisplaySclPin<'static>,
    chip::I2cPowerPin<'static>,
>;
#[cfg(all(not(feature = "qemu"), esp32s3, feature = "tft-display"))]
pub type DisplayConnection = firmware_common::display::SpiConnection<
    SPI2<'static>,
    chip::TftSckPin<'static>,
    chip::TftMosiPin<'static>,
    chip::TftCsPin<'static>,
    chip::TftDcPin<'static>,
    chip::TftRstPin<'static>,
    chip::TftBacklightPin<'static>,
    chip::I2cPowerPin<'static>,
>;
#[cfg(feature = "qemu")]
pub type DisplayConnection = firmware_common::display::QemuConnection;

/// Player-ID connection: the USB HID barcode scanner by default, the
/// onboard camera with `--features camera` (see [`CAMERA_PINS`]).
#[cfg(all(not(feature = "qemu"), not(feature = "camera")))]
pub type PlayerIdConnection = firmware_common::player::UsbConnection;
#[cfg(all(not(feature = "qemu"), feature = "camera"))]
pub type PlayerIdConnection = firmware_common::CameraPins;
#[cfg(feature = "qemu")]
pub type PlayerIdConnection = firmware_common::player::UsbConnection;

/// The board's peripheral connections, named by function. Each field is
/// a connection struct; `connect()` turns it into the running driver.
pub struct Board {
    pub radio: Radio<'static>,
    pub trigger: TriggerConnection,
    pub led: LedConnection,
    pub display: DisplayConnection,
    pub buzzer: BuzzerConnection,
    pub player_id: PlayerIdConnection,
}

/// Consume the raw peripherals and produce the function-named board map.
pub fn take(p: Peripherals) -> Board {
    #[cfg(not(feature = "qemu"))]
    let pins = take_pins(p.pins);
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
        trigger: TriggerConnection { pin: pins.trigger },
        #[cfg(feature = "qemu")]
        trigger: firmware_common::trigger::TriggerConnection,
        #[cfg(not(feature = "qemu"))]
        led: LedConnection { pin: pins.led },
        #[cfg(feature = "qemu")]
        led: firmware_common::led::LedConnection,
        #[cfg(all(not(feature = "qemu"), esp32))]
        display: DisplayConnection {
            i2c: p.i2c0,
            sda: pins.display_sda,
            scl: pins.display_scl,
            power: None,
        },
        #[cfg(all(not(feature = "qemu"), esp32s3, not(feature = "tft-display")))]
        display: DisplayConnection {
            i2c: p.i2c0,
            sda: pins.display_sda,
            scl: pins.display_scl,
            power: Some(pins.i2c_power),
        },
        #[cfg(all(not(feature = "qemu"), esp32s3, feature = "tft-display"))]
        display: DisplayConnection {
            spi: p.spi2,
            sck: pins.tft_sck,
            mosi: pins.tft_mosi,
            cs: pins.tft_cs,
            dc: pins.tft_dc,
            rst: pins.tft_rst,
            backlight: pins.tft_backlight,
            power: Some(pins.i2c_power),
        },
        #[cfg(feature = "qemu")]
        display: firmware_common::display::QemuConnection,
        #[cfg(not(feature = "qemu"))]
        buzzer: BuzzerConnection {
            timer: p.ledc.timer0,
            channel: p.ledc.channel0,
            pin: pins.buzzer,
        },
        #[cfg(feature = "qemu")]
        buzzer: firmware_common::buzzer::BuzzerConnection,
        #[cfg(all(not(feature = "qemu"), not(feature = "camera")))]
        player_id: firmware_common::player::UsbConnection,
        #[cfg(all(not(feature = "qemu"), feature = "camera"))]
        player_id: CAMERA_PINS,
        #[cfg(feature = "qemu")]
        player_id: firmware_common::player::UsbConnection,
    }
}

/// Image sensor wiring, used only with `--features camera`. Fields are
/// in the module's 2x9-header order (see firmware_common::CameraPins).
#[cfg(all(not(feature = "qemu"), feature = "camera", esp32))]
pub const CAMERA_PINS: firmware_common::CameraPins = firmware_common::CameraPins {
    // Header row 1 (left→right): PD D2 D4 D6 D8 XC HS SDA G
    pd: -1, // not wired
    d2: 14,
    d4: 33,
    d6: 4,
    d8: 39,
    xc: 32,
    hs: 21,
    sda: 26,
    // Header row 2 (left→right): RT D3 D5 D7 D9 PC VS SCL 3V
    rt: -1, // not wired
    d3: 15,
    d5: 25,
    d7: 34,
    d9: 36,
    pc: 5,
    vs: 12,
    scl: 27,
};

/// Image sensor wiring for the Feather TFT ESP32-S3 (field order: see
/// firmware_common::CameraPins). Untested on hardware — verify before wiring.
#[cfg(all(not(feature = "qemu"), feature = "camera", esp32s3))]
pub const CAMERA_PINS: firmware_common::CameraPins = firmware_common::CameraPins {
    // Header row 1 (left→right): PD D2 D4 D6 D8 XC HS SDA G
    pd: -1, // not wired
    d2: 12, // D12
    d4: 11, // D11
    d6: 10, // D10
    d8: 9,  // D9
    xc: 6,  // D6
    hs: 5,  // D5
    sda: 1, // TX
    // Header row 2 (left→right): RT D3 D5 D7 D9 PC VS SCL 3V
    rt: -1, // not wired
    d3: 18, // A0
    d5: 17, // A1
    d7: 16, // A2
    d9: 15, // A3
    pc: 14, // A4
    vs: 8,  // A5
    scl: 2, // RX
};
