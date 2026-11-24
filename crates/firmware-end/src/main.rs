//! End node of the two-part ESP-NOW timer.
//!
//! * Replies to the start node's sync pings so it can estimate the clock
//!   offset, and tracks the offset itself from `SyncInfo` broadcasts.
//! * On its trigger input (an analog pressure/vibration sensor on
//!   hardware — pin in board.rs; a scheduled virtual trigger in QEMU
//!   builds) it stamps `t_end` and broadcasts it; the start node reports
//!   the elapsed time.
//! * If it has both a start event and a sync estimate it also reports the
//!   elapsed time locally.

// esp-idf-sys emits the chip cfgs (`esp32`/`esp32s3`) without check-cfg
// declarations.
#![allow(unexpected_cfgs)]

use std::time::Duration;

use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::log::EspLogger;
use esp_idf_svc::nvs::EspDefaultNvsPartition;

use firmware_common::{format_elapsed, init_link, now_us, Connect, Link, RxFrame};
use timer_protocol::Message;

// All peripheral-unit and GPIO assignments live in board.rs.
mod board;

/// Main-loop wakeup period; only affects reporting latency since all
/// timestamps are taken in the trigger ISR / receive callback.
const RECV_WAIT: Duration = Duration::from_millis(100);
/// How many times event messages are broadcast (no ACKs on broadcast).
const EVENT_REPEATS: u32 = 3;

/// In QEMU builds, fire the virtual trigger this long after the start event,
/// simulating a measurement of this duration.
#[cfg(feature = "qemu")]
const AUTO_RUN_US: i64 = 1_500_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// No sync estimate yet.
    WaitSync,
    /// Synced, waiting for a start event.
    Armed,
    /// Start event seen, waiting for the local end trigger.
    Running { t_start_us: u64 },
}

fn main() {
    // Link ESP-IDF patches and bring up logging first.
    esp_idf_svc::sys::link_patches();
    EspLogger::initialize_default();
    log::info!("two-part timer: END node");

    let peripherals = Peripherals::take().unwrap();
    let sysloop = EspSystemEventLoop::take().unwrap();
    let nvs = EspDefaultNvsPartition::take().unwrap();

    // The link consumes only the radio peripheral (modem on hardware,
    // OpenCores MAC under QEMU); the rest of the board map is local IO.
    let board = board::take(peripherals);
    let (link, rx) = init_link(board.radio, sysloop, nvs).expect("link setup failed");

    // Stop trigger: analog pressure/vibration sensor on hardware,
    // scheduled virtual trigger under QEMU.
    let mut trigger = board.trigger.connect().expect("trigger setup failed");

    // On-board red LED: on while a measurement is running. (No usable GPIO
    // under QEMU; log instead there.)
    let mut led = board.led.connect().expect("led setup failed");

    let mut state = State::WaitSync;
    let mut sync: Option<(i64, u64)> = None; // (offset_us, rtt_us)

    loop {
        if let Some(t_end_us) = trigger.take() {
            log::info!("timer STOPPED at {t_end_us} us");
            for _ in 0..EVENT_REPEATS {
                link.broadcast(&Message::EndEvent { t_end_us });
            }
            if let (Some((offset_us, _)), State::Running { t_start_us }) = (sync, state) {
                let elapsed =
                    timer_protocol::end_to_start_domain(t_end_us, offset_us) - t_start_us as i64;
                if elapsed >= 0 {
                    log::info!(
                        "======= ELAPSED: {} ({elapsed} us) =======",
                        format_elapsed(elapsed)
                    );
                } else {
                    log::warn!("negative elapsed time ({elapsed} us); clock sync lost?");
                }
            } else if state != State::WaitSync {
                log::warn!("no clock sync; cannot compute elapsed time locally");
            }
            state = if sync.is_some() {
                State::Armed
            } else {
                State::WaitSync
            };
        }

        match state {
            State::Running { t_start_us: _ } => led.set(true),
            _ => led.set(false),
        }

        match rx.recv_timeout(RECV_WAIT) {
            Ok(frame) => handle_frame(frame, &link, &mut state, &mut sync),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => unreachable!(),
        }
    }
}

fn handle_frame(frame: RxFrame, link: &Link, state: &mut State, sync: &mut Option<(i64, u64)>) {
    let msg = match timer_protocol::decode(frame.payload()) {
        Ok(msg) => msg,
        Err(_) => return, // not ours
    };

    match msg {
        Message::Ping { seq, .. } => {
            // Reply immediately; the receive timestamp was taken in the
            // receive callback.
            link.broadcast(&Message::Pong {
                seq,
                t_recv_us: frame.t_rx_us,
                t_send_us: now_us(),
            });
        }
        Message::SyncInfo { offset_us, rtt_us } => {
            *sync = Some((offset_us, rtt_us));
            if *state == State::WaitSync {
                *state = State::Armed;
            }
            log::info!("clock sync OK: offset {offset_us} us, rtt {rtt_us} us - timer ARMED");
        }
        Message::StartEvent { t_start_us } => {
            log::info!("start event at {t_start_us} us (start-node clock)");
            *state = State::Running { t_start_us };
            // Under QEMU, simulate the end sensor firing AUTO_RUN_US after
            // the true start (converted into our clock domain).
            #[cfg(feature = "qemu")]
            if let Some((offset_us, _)) = *sync {
                firmware_common::Trigger::schedule_at(
                    (t_start_us as i64 + offset_us + AUTO_RUN_US) as u64,
                );
            }
        }
        // We are the source of pongs/end events.
        Message::Pong { .. } | Message::EndEvent { .. } => {}
    }
}
