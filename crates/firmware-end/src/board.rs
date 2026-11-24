//! Board map — end node.
//!
//! GPIO assignments differ per chip and live in the [`chip`] module
//! the [`Board`] struct below stays chip-agnostic

#[cfg(not(feature = "qemu"))]
use esp_idf_svc::hal::adc::ADC1;
#[cfg(feature = "qemu")]
use esp_idf_svc::hal::mac::MAC;
#[cfg(not(feature = "qemu"))]
use esp_idf_svc::hal::modem::Modem;
use esp_idf_svc::hal::peripherals::Peripherals;

#[cfg(not(feature = "qemu"))]
use chip::take_pins;

/// GPIO assignments: Adafruit Feather ESP32 V2.
#[cfg(all(not(feature = "qemu"), esp32))]
mod chip {
    use esp_idf_svc::hal::gpio::{Gpio13, Gpio34, Pins};

    /// Pressure/vibration sensor input (GPIO34, labelled A2; input-only).
    pub type PressureAdcPin<'d> = Gpio34<'d>;
    /// Onboard red LED: on while a measurement is running.
    pub type LedPin<'d> = Gpio13<'d>;

    /// The chip's GPIOs, renamed by function.
    pub struct ChipPins {
        pub pressure_adc: PressureAdcPin<'static>,
        pub led: LedPin<'static>,
    }

    pub fn take_pins(p: Pins) -> ChipPins {
        ChipPins {
            pressure_adc: p.gpio34,
            led: p.gpio13,
        }
    }
}

/// GPIO assignments: Adafruit Feather ESP32-S3.
#[cfg(all(not(feature = "qemu"), esp32s3))]
mod chip {
    use esp_idf_svc::hal::gpio::{Gpio13, Gpio8, Pins};

    /// Pressure/vibration sensor input (GPIO8, labelled A5; ADC1_CH7 —
    /// A0-A4 are ADC2, unusable while WiFi is up).
    pub type PressureAdcPin<'d> = Gpio8<'d>;
    /// Onboard red LED: on while a measurement is running.
    pub type LedPin<'d> = Gpio13<'d>;

    /// The chip's GPIOs, renamed by function.
    pub struct ChipPins {
        pub pressure_adc: PressureAdcPin<'static>,
        pub led: LedPin<'static>,
    }

    pub fn take_pins(p: Pins) -> ChipPins {
        ChipPins {
            pressure_adc: p.gpio8,
            led: p.gpio13,
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

/// Stop trigger connection: the pressure/vibration sensor on ADC1
/// (ADC2 cannot be sampled while the WiFi link is up).
#[cfg(not(feature = "qemu"))]
pub type TriggerConnection =
    firmware_common::sensor::AdcConnection<ADC1<'static>, chip::PressureAdcPin<'static>>;
/// Trigger connection under emulation (scheduled virtual trigger).
#[cfg(feature = "qemu")]
pub type TriggerConnection = firmware_common::trigger::TriggerConnection;

/// Onboard red LED connection (see [`chip::LedPin`]).
#[cfg(not(feature = "qemu"))]
pub type LedConnection = firmware_common::led::LedConnection<chip::LedPin<'static>>;
/// LED connection under emulation (no usable GPIO; logs instead).
#[cfg(feature = "qemu")]
pub type LedConnection = firmware_common::led::LedConnection;

/// The board's peripheral connections, named by function. Each field is
/// a connection struct; `connect()` turns it into the running driver.
pub struct Board {
    pub radio: Radio<'static>,
    pub trigger: TriggerConnection,
    pub led: LedConnection,
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
        trigger: TriggerConnection {
            adc: p.adc1,
            pin: pins.pressure_adc,
            threshold_mv: firmware_common::PressureSensor::DEFAULT_THRESHOLD_MV,
        },
        #[cfg(feature = "qemu")]
        trigger: firmware_common::trigger::TriggerConnection,
        #[cfg(not(feature = "qemu"))]
        led: LedConnection { pin: pins.led },
        #[cfg(feature = "qemu")]
        led: firmware_common::led::LedConnection,
    }
}
