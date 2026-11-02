//! Board map — Adafruit Feather ESP32 V2 (end node).
//!
//! Every peripheral unit and GPIO the firmware uses is owned here, renamed
//! by function, and handed to main as one [`Board`] struct. A port to
//! another board starts (and ideally ends) in this file; see the "Porting
//! to a new board" checklist in ARCHITECTURE.md.

#[cfg(not(feature = "qemu"))]
use esp_idf_svc::hal::adc::ADC1;
#[cfg(not(feature = "qemu"))]
use esp_idf_svc::hal::gpio::{Gpio13, Gpio34};
#[cfg(feature = "qemu")]
use esp_idf_svc::hal::mac::MAC;
#[cfg(not(feature = "qemu"))]
use esp_idf_svc::hal::modem::Modem;
use esp_idf_svc::hal::peripherals::Peripherals;

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
    /// ADC unit the pressure sensor sits on. (GPIO34 exists only on classic
    /// ESP32; on S3 use an ADC1 pin on GPIO1-10.)
    #[cfg(not(feature = "qemu"))]
    pub pressure_adc_unit: ADC1<'static>,
    /// Pressure/vibration sensor input (GPIO34, labelled A2; input-only).
    #[cfg(not(feature = "qemu"))]
    pub pressure_adc: Gpio34<'static>,
    /// Onboard red LED: on while a measurement is running.
    #[cfg(not(feature = "qemu"))]
    pub led: Gpio13<'static>,
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
        pressure_adc_unit: p.adc1,
        #[cfg(not(feature = "qemu"))]
        pressure_adc: p.pins.gpio34,
        #[cfg(not(feature = "qemu"))]
        led: p.pins.gpio13,
    }
}
