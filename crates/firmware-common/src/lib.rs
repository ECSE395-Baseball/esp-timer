//! Shared firmware plumbing for the two-part ESP-NOW timer nodes.
//!
//! * [`link`] — the radio the node talks on
//! * [`trigger`] — start/stop trigger inputs, timestamped at the source
//! * [`time`] — local monotonic clock helpers
//! * [`display`] — the start node's 128x64 screen
//! * [`sensor`] — the end node's analog pressure/vibration stop trigger
//! * [`player`] — player-ID scan format shared by all scan sources
//! * [`scanner`] — USB HID keyboard-wedge barcode reader (feature `usb`;
//!   ESP32-S3 host; start node)
//! * [`camera`] — onboard-camera QR reader (feature `camera`)

// esp-idf-sys emits the chip cfgs (`esp32`/`esp32s3`) without check-cfg
// declarations.
#![allow(unexpected_cfgs)]

pub mod alloc_trace;
pub mod buzzer;
pub mod display;
pub mod led;
pub mod link;
pub mod player;
pub mod scores;
pub mod sensor;
pub mod time;
pub mod trigger;

#[cfg(feature = "camera")]
pub mod camera;
#[cfg(feature = "usb")]
pub mod scanner;

/// Driver construction for a board's peripherals: a connection struct
/// holds the pins/units a driver needs; `connect` builds the driver.
pub trait Connect {
    type Output;
    fn connect(self) -> Result<Self::Output, esp_idf_svc::sys::EspError>;
}

pub use buzzer::Buzzer;
pub use display::Display;
pub use led::Led;
pub use link::{init_link, Link, RxFrame, ESPNOW_MAX_LEN};
pub use player::{PlayerIdSource, PlayerScan};
pub use scores::{Score, ScoreLog};
pub use sensor::PressureSensor;
pub use time::{format_elapsed, now_us};
pub use trigger::Trigger;

#[cfg(feature = "camera")]
pub use camera::{Camera, CameraPins};
#[cfg(feature = "usb")]
pub use scanner::Scanner;
