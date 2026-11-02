//! Player identification shared by all scan sources (USB barcode scanner
//! today, onboard camera later).
//!
//! Every source yields the same payload format, one scan per line:
//! `<uuid> <player name>` — a canonical 36-character UUID, a space, then
//! the player's display name (which may itself contain spaces). Example:
//! `550e8400-e29b-41d4-a716-446655440000 Ada Lovelace`

use std::string::String;

/// A source of player-ID scans. Implemented by the USB barcode scanner
/// (feature `usb`) and the onboard camera (feature `camera`) so the two
/// can be swapped without touching the timer logic — obtaining player
/// information is the only thing these peripherals are for.
pub trait PlayerIdSource {
    /// Take the most recently obtained scan, if any.
    fn take(&self) -> Option<PlayerScan>;

    /// Toggles whether we are actively polling for player information
    /// from the information provider (Camera/Reader)
    fn set_enable(&self, _timing: bool) {}
}

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

/// Parse one scan line (`<uuid> <name>`) into a [`PlayerScan`].
///
/// Returns `None` for lines that do not start with a well-formed UUID —
/// e.g. garbage from a mis-aimed scan, so the reader task can just skip.
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
