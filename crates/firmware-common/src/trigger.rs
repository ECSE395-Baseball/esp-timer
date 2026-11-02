//! Start/stop trigger inputs, timestamped at the event source.
//!
//! * hardware: a GPIO edge interrupt, timestamped in the ISR
//! * `qemu`: a software trigger firing at a scheduled time on the local
//!   clock, simulating the button/sensor event
//!
//! Both expose `take()`, so the timer logic on top is identical.

use core::sync::atomic::Ordering;

#[cfg(not(feature = "qemu"))]
use esp_idf_svc::sys::EspError;
use portable_atomic::AtomicI64;

use crate::time::now_us;

/// Minimum time between accepted trigger events.
#[cfg(not(feature = "qemu"))]
const DEBOUNCE_US: i64 = 50_000;

#[cfg(feature = "qemu")]
static VIRTUAL_TRIGGER_AT_US: AtomicI64 = AtomicI64::new(-1);

/// A GPIO edge trigger, timestamped in the ISR.
///
/// The `Trigger` handle must be kept alive for the interrupt to stay
/// subscribed. Each instance owns its timestamp cell, so triggers on
/// different pins are independent (no shared global state).
///
/// esp-idf-hal disables the pin's interrupt after every trigger (its
/// `handle_isr` calls `gpio_intr_disable`), so [`Trigger::take`] re-arms
/// it; without that the trigger would fire exactly once per boot.
#[cfg(not(feature = "qemu"))]
pub struct Trigger {
    pin: esp_idf_svc::hal::gpio::PinDriver<'static, esp_idf_svc::hal::gpio::Input>,
    fired_at: &'static AtomicI64,
}

#[cfg(not(feature = "qemu"))]
impl Trigger {
    /// Subscribe a falling-edge interrupt on `pin` (internal pull-up on).
    pub fn on_falling_edge(
        pin: impl esp_idf_svc::hal::gpio::InputPin + 'static,
    ) -> Result<Self, EspError> {
        use esp_idf_svc::hal::gpio::{InterruptType, Pull};
        Self::on_edge(pin, InterruptType::NegEdge, Pull::Up)
    }

    /// Subscribe a rising-edge interrupt on `pin` (internal pull-down on).
    pub fn on_rising_edge(
        pin: impl esp_idf_svc::hal::gpio::InputPin + 'static,
    ) -> Result<Self, EspError> {
        use esp_idf_svc::hal::gpio::{InterruptType, Pull};
        Self::on_edge(pin, InterruptType::PosEdge, Pull::Down)
    }

    fn on_edge(
        pin: impl esp_idf_svc::hal::gpio::InputPin + 'static,
        interrupt_type: esp_idf_svc::hal::gpio::InterruptType,
        pull: esp_idf_svc::hal::gpio::Pull,
    ) -> Result<Self, EspError> {
        use esp_idf_svc::hal::gpio::PinDriver;
        let mut pin = PinDriver::input(pin, pull)?;
        pin.set_interrupt_type(interrupt_type)?;
        // The ISR may fire for as long as the subscription lives — past the
        // end of this function, and (if the handle is forgotten) forever —
        // so the cell it writes must be `'static`. Leaking one 8-byte cell
        // per trigger makes that true by construction.
        let fired_at: &'static AtomicI64 = Box::leak(Box::new(AtomicI64::new(-1)));
        // Safety: the closure only touches the leaked cell, which outlives
        // any possible subscription, so no dangling reference is possible.
        unsafe {
            pin.subscribe(move || {
                let now = now_us() as i64;
                let last = fired_at.load(Ordering::Relaxed);
                if now - last >= DEBOUNCE_US {
                    fired_at.store(now, Ordering::Relaxed);
                }
            })?;
        }
        pin.enable_interrupt()?;
        Ok(Self { pin, fired_at })
    }

    /// Take the pending trigger timestamp (us), if any.
    ///
    /// Re-arms the pin interrupt after a consumed event: esp-idf-hal disables
    /// it on every trigger, and it must be re-enabled from a non-ISR context
    pub fn take(&mut self) -> Option<u64> {
        match self.fired_at.swap(-1, Ordering::Relaxed) {
            -1 => None,
            t => {
                if let Err(e) = self.pin.enable_interrupt() {
                    log::warn!("trigger re-arm failed: {e}");
                }
                Some(t as u64)
            }
        }
    }
}

/// A software trigger used in QEMU builds: fires at a scheduled time on the
/// local clock, simulating the button/sensor event that hardware gets via
/// GPIO. Keeps the timing semantics identical to the GPIO trigger (the
/// timestamp is the scheduled "press" time, not when the main loop notices).
#[cfg(feature = "qemu")]
pub struct Trigger {
    _private: (),
}

#[cfg(feature = "qemu")]
impl Trigger {
    pub fn new_virtual() -> Self {
        VIRTUAL_TRIGGER_AT_US.store(-1, Ordering::Relaxed);
        Self { _private: () }
    }

    /// Schedule the trigger to fire at `t_us` on the local clock.
    pub fn schedule_at(t_us: u64) {
        VIRTUAL_TRIGGER_AT_US.store(t_us as i64, Ordering::Relaxed);
    }

    /// Take the trigger timestamp (us) once the scheduled time has passed.
    pub fn take(&self) -> Option<u64> {
        let at = VIRTUAL_TRIGGER_AT_US.load(Ordering::Relaxed);
        if at >= 0 && now_us() as i64 >= at {
            VIRTUAL_TRIGGER_AT_US.store(-1, Ordering::Relaxed);
            Some(at as u64)
        } else {
            None
        }
    }
}
