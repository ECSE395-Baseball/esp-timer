//! Shared firmware plumbing for the two-part ESP-NOW timer nodes.
//!
//! * [`link`] — the radio the node talks on
//! * [`trigger`] — start/stop trigger inputs, timestamped at the source
//! * [`time`] — local monotonic clock helpers
//! * [`display`] — the start node's 128x64 screen
//! * [`sensor`] — the end node's analog pressure/vibration stop trigger
//! * [`player`] — player-ID scan format shared by all scan sources
//! * [`scanner`] — UART QR/barcode reader (feature `usb`; start node)
//! * [`camera`] — onboard-camera QR reader (feature `camera`)

pub mod buzzer;
pub mod display;
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

pub use buzzer::Buzzer;
pub use display::Display;
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
