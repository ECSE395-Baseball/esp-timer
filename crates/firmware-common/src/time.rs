//! Local-clock helpers shared by the link, triggers and sensors.

use esp_idf_svc::sys::esp_timer_get_time;

/// Microseconds since boot on this node's free-running clock.
pub fn now_us() -> u64 {
    // esp_timer_get_time is ISR-safe and monotonic.
    (unsafe { esp_timer_get_time() }) as u64
}

/// Format a microsecond duration as `s.uuuuuu` for logging.
pub fn format_elapsed(elapsed_us: i64) -> std::string::String {
    std::format!("{}.{:06} s", elapsed_us / 1_000_000, elapsed_us % 1_000_000)
}
