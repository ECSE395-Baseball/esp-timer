//! Player identification shared by all scan sources
//!
//! QR format:
//! `<uuid> <player name>` — canonical 36-character UUID, a space, then
//! the player's display name

use std::string::String;
use std::sync::{Arc, Mutex};

/// A 1-bit, row-major preview of a camera source's binarized detector
/// view, shared between the camera task (writer) and the UI (reader).
pub struct PreviewFrame {
    /// Monotonic counter bumped per published frame; the UI redraws only
    /// when it changes.
    pub seq: u64,
    pub w: usize,
    pub h: usize,
    /// Row-major pixels, 1 bit each (1 = light), LSB-first within each
    /// byte (same convention as the display's 1-bit canvas).
    pub bits: Vec<u8>,
}

/// A source of player-ID scans (USB barcode scanner, onboard camera),
/// swappable without touching the timer logic.
pub trait PlayerIdSource {
    /// Take the most recently obtained scan, if any.
    fn take(&self) -> Option<PlayerScan>;

    /// Toggles whether we are actively polling for player information
    /// from the information provider (Camera/Reader)
    fn set_enable(&self, _timing: bool) {}

    /// Live detector-view preview for aiming, if this source is a camera.
    /// Non-camera sources yield no preview.
    fn preview(&self) -> Option<Arc<Mutex<PreviewFrame>>> {
        None
    }
}

// The start node's player-ID plumbing. Only the start node enables the
// `usb` feature; the end node never has a scan source.
#[cfg(feature = "usb")]
mod id {
    use esp_idf_svc::sys::EspError;
    use std::sync::{Arc, Mutex};

    use super::{PlayerIdSource, PlayerScan, PreviewFrame};
    use crate::Connect;

    /// The start node's player-ID source: the USB HID barcode scanner
    /// (mock under QEMU), or the camera with `--features camera`.
    pub enum PlayerId {
        #[cfg(not(feature = "camera"))]
        Scanner(crate::scanner::Scanner),
        #[cfg(all(feature = "camera", not(feature = "qemu")))]
        Camera(crate::camera::Camera),
    }

    impl PlayerIdSource for PlayerId {
        fn take(&self) -> Option<PlayerScan> {
            match self {
                #[cfg(not(feature = "camera"))]
                Self::Scanner(scanner) => scanner.take(),
                #[cfg(all(feature = "camera", not(feature = "qemu")))]
                Self::Camera(camera) => camera.take(),
                // firmware-start rejects this combo with a compile_error.
                #[cfg(all(feature = "camera", feature = "qemu"))]
                _ => unreachable!("camera is not supported under QEMU"),
            }
        }

        fn set_enable(&self, enable: bool) {
            match self {
                #[cfg(not(feature = "camera"))]
                Self::Scanner(scanner) => scanner.set_enable(enable),
                #[cfg(all(feature = "camera", not(feature = "qemu")))]
                Self::Camera(camera) => camera.set_enable(enable),
                // firmware-start rejects this combo with a compile_error.
                #[cfg(all(feature = "camera", feature = "qemu"))]
                _ => unreachable!("camera is not supported under QEMU"),
            }
        }

        fn preview(&self) -> Option<Arc<Mutex<PreviewFrame>>> {
            match self {
                #[cfg(not(feature = "camera"))]
                Self::Scanner(_) => None,
                #[cfg(all(feature = "camera", not(feature = "qemu")))]
                Self::Camera(camera) => Some(camera.preview_slot()),
                // firmware-start rejects this combo with a compile_error.
                #[cfg(all(feature = "camera", feature = "qemu"))]
                _ => unreachable!("camera is not supported under QEMU"),
            }
        }
    }

    /// Connection to the USB HID scanner (unit struct: the USB host stack
    /// drives the native USB-C port itself, so there are no pins to hold).
    #[cfg(not(feature = "camera"))]
    pub struct UsbConnection;

    #[cfg(not(feature = "camera"))]
    impl Connect for UsbConnection {
        type Output = PlayerId;
        fn connect(self) -> Result<Self::Output, EspError> {
            Ok(PlayerId::Scanner(crate::scanner::Scanner::new()?))
        }
    }

    #[cfg(all(feature = "camera", not(feature = "qemu")))]
    impl Connect for crate::camera::CameraPins {
        type Output = PlayerId;
        fn connect(self) -> Result<Self::Output, EspError> {
            Ok(PlayerId::Camera(crate::camera::Camera::new(self)?))
        }
    }
}

#[cfg(feature = "usb")]
pub use id::PlayerId;
#[cfg(all(feature = "usb", not(feature = "camera")))]
pub use id::UsbConnection;

/// One scanned player identity.
pub struct PlayerScan {
    /// Canonical lowercase UUID string (36 chars, hyphenated).
    pub uuid: String,
    /// Display name from the scan; empty if the payload had no name part.
    pub name: String,
}

impl PlayerScan {
    /// What the screen should show: the name, or a short UUID prefix when
    /// the scan carried no name.
    pub fn display_name(&self) -> &str {
        if self.name.is_empty() {
            &self.uuid[..8]
        } else {
            &self.name
        }
    }
}

/// Parse one scan line (`<uuid> <name>`) into a [`PlayerScan`]; `None`
/// for lines without a well-formed leading UUID (mis-aimed scans).
pub fn parse_scan_line(line: &str) -> Option<PlayerScan> {
    let line = line.trim();
    let (uuid, rest) = match line.split_once(char::is_whitespace) {
        Some((uuid, rest)) => (uuid, rest),
        None => (line, ""),
    };
    let uuid = normalize_uuid(uuid)?;
    Some(PlayerScan {
        uuid,
        name: String::from(rest.trim()),
    })
}

/// Validate and canonicalize (lowercase) a hyphenated UUID string.
fn normalize_uuid(s: &str) -> Option<String> {
    if s.len() != 36 {
        return None;
    }
    let mut out = String::with_capacity(36);
    for (i, c) in s.char_indices() {
        match i {
            8 | 13 | 18 | 23 => {
                if c != '-' {
                    return None;
                }
                out.push('-');
            }
            _ => {
                if !c.is_ascii_hexdigit() {
                    return None;
                }
                out.push(c.to_ascii_lowercase());
            }
        }
    }
    Some(out)
}
