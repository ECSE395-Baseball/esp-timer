//! Onboard red LED: on while a measurement is running.
//!
//! Hardware only: under QEMU there is no usable GPIO, so `set` only logs.

#[cfg(not(feature = "qemu"))]
mod imp {
    use esp_idf_svc::hal::gpio::{Output, OutputPin, PinDriver};
    use esp_idf_svc::sys::EspError;

    use crate::Connect;

    /// Connection: one GPIO, driven active-high.
    pub struct LedConnection<P> {
        pub pin: P,
    }

    impl<P: OutputPin + 'static> Connect for LedConnection<P> {
        type Output = Led;
        fn connect(self) -> Result<Self::Output, EspError> {
            let mut led = PinDriver::output(self.pin)?;
            led.set_low()?;
            Ok(Led(led))
        }
    }

    /// Active-high LED output handle.
    pub struct Led(PinDriver<'static, Output>);

    impl Led {
        /// `true` = lit.
        pub fn set(&mut self, on: bool) {
            let _ = self.0.set_level(on.into());
        }
    }
}

#[cfg(feature = "qemu")]
mod imp {
    use esp_idf_svc::sys::EspError;

    use crate::Connect;

    /// No usable GPIO under emulation.
    pub struct LedConnection;

    impl Connect for LedConnection {
        type Output = Led;
        fn connect(self) -> Result<Self::Output, EspError> {
            Ok(Led)
        }
    }

    /// Silent stand-in under emulation.
    pub struct Led;

    impl Led {
        pub fn set(&mut self, on: bool) {
            log::debug!("led: {} (silent under QEMU)", if on { "on" } else { "off" });
        }
    }
}

pub use imp::{Led, LedConnection};
