//! Analog pressure/vibration sensor on ADC
//!
//! Polling is used because esp-idf-hal exposes no analog threshold
//! interrupt; the stop timestamp therefore carries up to ~[`POLL_PERIOD`]
//! of latency

use core::sync::atomic::Ordering;
use std::thread::JoinHandle;
use std::time::Duration;

use esp_idf_svc::hal::adc::oneshot::{config::AdcChannelConfig, AdcChannelDriver, AdcDriver};
use esp_idf_svc::hal::adc::{attenuation, Adc, AdcChannel, AdcUnit};
use esp_idf_svc::hal::gpio::ADCPin;
use esp_idf_svc::sys::EspError;
use portable_atomic::AtomicI64;

use crate::now_us;

/// How often the ADC is sampled.
const POLL_PERIOD: Duration = Duration::from_millis(1);
/// Reads per sample; the median suppresses single-read spikes.
const MEDIAN_READS: usize = 3;
/// Re-arm level: the reading must fall this far below the threshold before
/// the sensor can fire again (protection against contact bounce/ringing).
const HYSTERESIS_MV: u16 = 150;

static FIRED_AT_US: AtomicI64 = AtomicI64::new(-1);

/// Pressure sensor trigger handle; dropping it detaches the polling task
pub struct PressureSensor {
    _task: JoinHandle<()>,
}

impl PressureSensor {
    /// Default trigger level (approximate mV at the divider midpoint).
    /// Tune for the actual sensor by watching the logged readings.
    pub const DEFAULT_THRESHOLD_MV: u16 = 1800;

    /// Start the polling task, reading the divider on any ADC channel pin
    ///
    /// Fires when the voltage rises through `threshold_mv`
    pub fn new<U: AdcUnit, C: AdcChannel<AdcUnit = U>>(
        adc: impl Adc<AdcUnit = U> + 'static,
        pin: impl ADCPin<AdcChannel = C> + 'static,
        threshold_mv: u16,
    ) -> Result<Self, EspError> {
        // 12 dB attenuation -> ~3.1 V full-scale for the 3.3 V divider.
        // Uncalibrated: `read` still returns approximate mV via the direct
        // raw->mV converter
        let config = AdcChannelConfig {
            attenuation: attenuation::DB_12,
            ..AdcChannelConfig::new()
        };
        // The channel driver takes ownership of the unit driver, so the
        // single handle moves into the polling task.
        let mut channel = AdcChannelDriver::new(AdcDriver::new(adc)?, pin, &config)?;

        let task = std::thread::Builder::new()
            .name("press-sensor".into()) // FreeRTOS task names cap at 16 chars
            .stack_size(3072)
            .spawn(move || {
                let mut reads = [0u16; MEDIAN_READS];
                let mut armed = true;
                loop {
                    for r in &mut reads {
                        *r = channel.read().unwrap_or(0);
                    }
                    reads.sort_unstable();
                    let mv = reads[MEDIAN_READS / 2];

                    if armed && mv >= threshold_mv {
                        FIRED_AT_US.store(now_us() as i64, Ordering::Relaxed);
                        armed = false;
                        log::info!("pressure sensor fired at {mv} mV");
                    } else if !armed && mv + HYSTERESIS_MV <= threshold_mv {
                        armed = true;
                        log::debug!("pressure sensor re-armed at {mv} mV");
                    }

                    std::thread::sleep(POLL_PERIOD);
                }
            })
            .expect("failed to spawn pressure sensor task");

        Ok(Self { _task: task })
    }

    /// Take the pending trigger timestamp (us), if any.
    pub fn take(&self) -> Option<u64> {
        match FIRED_AT_US.swap(-1, Ordering::Relaxed) {
            -1 => None,
            t => Some(t as u64),
        }
    }
}
