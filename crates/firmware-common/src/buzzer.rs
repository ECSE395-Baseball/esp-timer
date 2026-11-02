//! Passive piezo buzzer on an LEDC PWM channel: the 3-2-1-go start cue,
//!
//! Hardware only: under QEMU every call is a silent no-op so the
//! countdown logic runs unchanged in the simulator.

#[cfg(not(feature = "qemu"))]
mod imp {
    use esp_idf_svc::hal::gpio::OutputPin;
    use esp_idf_svc::hal::ledc::{
        config::TimerConfig, LedcChannel, LedcDriver, LedcTimer, LedcTimerDriver, Resolution,
        SpeedMode,
    };
    use esp_idf_svc::hal::units::Hertz;
    use esp_idf_svc::sys::EspError;

    /// Square-wave tone generator on one LEDC timer/channel pair.
    pub struct Buzzer<S: SpeedMode> {
        timer: LedcTimerDriver<'static, S>,
        channel: LedcDriver<'static>,
    }

    impl<S: SpeedMode> Buzzer<S> {
        pub fn new<T, C>(
            timer: T,
            channel: C,
            pin: impl OutputPin + 'static,
        ) -> Result<Self, EspError>
        where
            T: LedcTimer<SpeedMode = S> + 'static,
            C: LedcChannel<SpeedMode = S> + 'static,
        {
            let timer = LedcTimerDriver::new(
                timer,
                &TimerConfig::new()
                    .frequency(Hertz(1000))
                    .resolution(Resolution::Bits10),
            )?;
            let channel = LedcDriver::new(channel, &timer, pin)?;
            // Duty starts at 0: silent until the first `tone()`.
            Ok(Self { timer, channel })
        }

        /// Start sounding `freq_hz`; returns immediately.
        pub fn tone(&mut self, freq_hz: u32) {
            let _ = self.timer.set_frequency(Hertz(freq_hz.max(1)));
            let duty = self.channel.get_max_duty() / 2; // ~50% square wave
            let _ = self.channel.set_duty(duty);
        }

        /// Stop the tone (hold the pin low).
        pub fn silence(&mut self) {
            let _ = self.channel.set_duty(0);
        }
    }
}

#[cfg(feature = "qemu")]
mod imp {
    /// Silent stand-in under emulation.
    pub struct Buzzer;

    impl Buzzer {
        pub fn new() -> Self {
            Self
        }

        pub fn tone(&mut self, freq_hz: u32) {
            log::debug!("buzzer: tone {freq_hz} Hz (silent under QEMU)");
        }

        pub fn silence(&mut self) {}
    }
}

pub use imp::Buzzer;
