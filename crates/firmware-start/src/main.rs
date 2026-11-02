//! Start node of the two-part ESP-NOW timer.
//!
//! * Acts as the clock-sync master: periodically pings the end node and
//!   keeps a best-offset estimate (lowest-RTT sample in a sliding window).
//! * On its trigger input (BOOT button on GPIO0 on hardware; a scheduled
//!   virtual trigger in QEMU builds) it plays a 3-2-1-go countdown on the
//!   buzzer, then stamps `t_start` at the "go" beep and broadcasts it.
//! * On `EndEvent` from the end node it converts the remote timestamp into
//!   its own clock domain and reports the elapsed time over serial.
//! * Records every finished run as a score (player + elapsed time) and
//!   serves them over BLE for a phone/laptop to sync to the database
//!   (see `firmware_common::scores` for the GATT profile).
//! * Drives the 128x64 screen: player info while idle, a countdown while
//!   the start cue plays, a live counting timer while running, and the
//!   final time after a run. On hardware this is an SSD1306 OLED; under
//!   QEMU it is the virtual RGB panel (visible with `-display sdl`).
//! * Identifies players via a UART QR/barcode scanner (UUID + name per
//!   scan) and shows the player on the idle screen.

use std::collections::VecDeque;
use std::time::Duration;

use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::log::EspLogger;
use esp_idf_svc::nvs::EspDefaultNvsPartition;

#[cfg(all(not(feature = "qemu"), feature = "camera"))]
use firmware_common::Camera;
#[cfg(all(not(feature = "qemu"), not(feature = "camera")))]
use firmware_common::Scanner;
use firmware_common::{
    format_elapsed, init_link, now_us, Buzzer, Display, Link, RxFrame, ScoreLog, Trigger,
};
#[cfg(not(feature = "qemu"))]
use firmware_common::{PlayerIdSource, PlayerScan};

// All peripheral-unit and GPIO assignments live in board.rs.
mod board;
use timer_protocol::{compute_offset_rtt, Message};

/// How often a sync ping is broadcast (us).
const PING_INTERVAL_US: u64 = 500_000;
/// Sync samples needed before the node considers itself armed.
const MIN_SYNC_SAMPLES: usize = 5;
/// Sliding window for the lowest-RTT offset estimate.
const SYNC_WINDOW: usize = 16;
/// Samples with an RTT above this are discarded as wild outliers (us).
/// Quality selection happens via the lowest-RTT window below, so this is
/// only a sanity bound.
const MAX_GOOD_RTT_US: u64 = 100_000;
/// How many times event messages are broadcast (no ACKs on broadcast).
const EVENT_REPEATS: u32 = 3;
/// Main-loop wakeup period while not running; only affects reporting
/// latency since all timestamps are taken in the trigger ISR / receive
/// callback.
const RECV_WAIT: Duration = Duration::from_millis(50);

/// In QEMU builds, fire the virtual trigger this long after arming,
/// simulating the start of a measurement.
#[cfg(feature = "qemu")]
const AUTO_START_DELAY_US: u64 = 2_000_000;

/// How long the final time stays on screen after a run (us).
const RESULT_HOLD_US: u64 = 5_000_000;

/// Start cue: three short low beeps, then a longer high "go" beep; the timer
/// starts at the onset of "go".
const COUNT_BEEPS: u8 = 3;
/// NOTE_C3 / NOTE_C4
const BEEP_FREQ_HZ: u32 = 131;
const GO_FREQ_HZ: u32 = 262;
/// Short beep length and the beep-to-beep spacing (250 ms beep + 325 ms
/// gap in the prototype).
const BEEP_MS: u64 = 250;
const BEEP_STEP_MS: u64 = 575;
/// "Go" sounds this long after the countdown started.
const GO_AT_MS: u64 = COUNT_BEEPS as u64 * BEEP_STEP_MS;
/// "Go" beep length.
const GO_MS: u64 = 500;

/// What the screen currently shows; redrawn only on change. The running
/// timer is quantized to milliseconds, matching what the display renders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Screen {
    Syncing,
    Idle,
    /// Seconds left in the start countdown.
    Countdown(u8),
    /// Elapsed time in milliseconds.
    Running(u64),
    /// Final elapsed time in microseconds.
    Result(i64),
}

/// `player` is the UUID scanned before the trigger, carried from the
/// countdown into the run so the score can name the player.
#[derive(Debug, Clone, PartialEq, Eq)]
enum State {
    Syncing,
    Armed,
    /// Playing the 3-2-1-go cue; the run starts at the "go" beep.
    Countdown {
        t0_us: u64,
        beeps_done: u8,
        player: Option<String>,
    },
    Running {
        t_start_us: u64,
        player: Option<String>,
    },
}

fn main() {
    // Link ESP-IDF patches and bring up logging first.
    esp_idf_svc::sys::link_patches();
    EspLogger::initialize_default();
    log::info!("two-part timer: START node");

    let peripherals = Peripherals::take().unwrap();
    let sysloop = EspSystemEventLoop::take().unwrap();
    let nvs = EspDefaultNvsPartition::take().unwrap();

    // The link consumes only the radio peripheral (modem on hardware,
    // OpenCores MAC under QEMU); the rest of the board map is local IO.
    let board = board::take(peripherals);
    let (link, rx) = init_link(board.radio, sysloop, nvs).expect("link setup failed");

    // Trigger input: BOOT button (to ground when pressed) on hardware,
    // scheduled virtual trigger under QEMU.
    #[cfg(not(feature = "qemu"))]
    let mut trigger = Trigger::on_falling_edge(board.trigger).expect("trigger setup failed");
    #[cfg(feature = "qemu")]
    let trigger = Trigger::new_virtual();

    // The screen: an SSD1306 OLED over I2C on hardware; QEMU's virtual RGB
    // panel under emulation (shown in an SDL window when QEMU runs with
    // `-display sdl`).
    #[cfg(not(feature = "qemu"))]
    let mut display = Display::new(board.display_i2c, board.display_sda, board.display_scl)
        .expect("display setup failed");
    #[cfg(feature = "qemu")]
    let mut display = Display::new_qemu_panel();

    // Player ID source: a UART QR/barcode scanner by default, the onboard
    // camera when built with `--features camera`. Both implement
    // PlayerIdSource; the rest of the code only ever calls `take()`.
    #[cfg(all(not(feature = "qemu"), not(feature = "camera")))]
    let player_source = Scanner::new(
        board.scanner_uart,
        board.scanner_rx,
        Scanner::DEFAULT_BAUDRATE,
    )
    .expect("scanner setup failed");
    #[cfg(all(not(feature = "qemu"), feature = "camera"))]
    let player_source = Camera::new(board::CAMERA_PINS).expect("camera setup failed");
    // Most recently scanned player, shown on the idle screen.
    #[cfg(not(feature = "qemu"))]
    let mut player: Option<PlayerScan> = None;

    let mut shown_screen: Option<Screen> = None;
    // Result on screen until this time: (elapsed_us, show-until us).
    let mut result_until: Option<(i64, u64)> = None;
    // Buzzer silenced at this time (end of the current beep).
    let mut buzzer_off_at: Option<u64> = None;

    // On-board red LED: on while a measurement is running. (No usable GPIO
    // under QEMU; log instead there.)
    #[cfg(not(feature = "qemu"))]
    let mut led = {
        let mut led =
            esp_idf_svc::hal::gpio::PinDriver::output(board.led).expect("led setup failed");
        led.set_low().unwrap();
        led
    };

    // Start-cue buzzer: passive piezo on LEDC PWM (silent under QEMU).
    #[cfg(not(feature = "qemu"))]
    let mut buzzer = Buzzer::new(board.buzzer_timer, board.buzzer_channel, board.buzzer)
        .expect("buzzer setup failed");
    #[cfg(feature = "qemu")]
    let mut buzzer = Buzzer::new();

    // Finished-run scores, served over BLE for database sync (hardware
    // only; QEMU builds just keep the log).
    let scores = ScoreLog::new();
    #[cfg(not(feature = "qemu"))]
    std::thread::Builder::new()
        .stack_size(16 * 1024)
        .spawn({
            let scores = scores.clone();
            move || firmware_common::scores::serve(scores)
        })
        .expect("score sync thread");

    let mut state = State::Syncing;
    let mut seq = 0u32;
    let mut last_ping_us = 0u64;
    let mut pending_ping: Option<(u32, u64)> = None; // (seq, t1 on start clock)
    let mut samples: VecDeque<(i64, u64)> = VecDeque::with_capacity(SYNC_WINDOW);
    #[cfg(feature = "qemu")]
    let mut auto_start_scheduled = false;

    loop {
        // A trigger press starts the countdown cue (when idle).
        if let Some(t_press_us) = trigger.take() {
            if matches!(state, State::Running { .. } | State::Countdown { .. }) {
                log::warn!("trigger ignored: timer already running");
            } else {
                log::info!("start cue at {t_press_us} us: 3-2-1-go");
                #[cfg(not(feature = "qemu"))]
                let run_player = player.as_ref().map(|p| p.uuid.clone());
                #[cfg(feature = "qemu")]
                let run_player = None;
                state = State::Countdown {
                    t0_us: t_press_us,
                    beeps_done: 0,
                    player: run_player,
                };
            }
        }

        // Sound the countdown; start the run at the onset of "go".
        if let State::Countdown {
            t0_us, beeps_done, ..
        } = state
        {
            let now = now_us();
            let at_ms = now.saturating_sub(t0_us) / 1_000;
            if beeps_done < COUNT_BEEPS && at_ms >= beeps_done as u64 * BEEP_STEP_MS {
                buzzer.tone(BEEP_FREQ_HZ);
                buzzer_off_at = Some(now + BEEP_MS * 1_000);
                if let State::Countdown { beeps_done: b, .. } = &mut state {
                    *b += 1;
                }
            } else if beeps_done == COUNT_BEEPS && at_ms >= GO_AT_MS {
                buzzer.tone(GO_FREQ_HZ);
                buzzer_off_at = Some(now + GO_MS * 1_000);
                let t_start_us = now;
                log::info!("timer STARTED at {t_start_us} us");
                for _ in 0..EVENT_REPEATS {
                    link.broadcast(&Message::StartEvent { t_start_us });
                }
                let State::Countdown { player, .. } = state else {
                    unreachable!()
                };
                state = State::Running { t_start_us, player };
            }
        }

        // Stop the buzzer once the current beep has sounded long enough.
        if let Some(off_at) = buzzer_off_at {
            if now_us() >= off_at {
                buzzer.silence();
                buzzer_off_at = None;
            }
        }

        // Handle any received frames promptly. While running (or counting
        // down), wake more often so the millisecond timer on screen rolls
        // smoothly and the beeps start on time; the slower idle wait only
        // affects reporting latency (timestamps are taken in the trigger
        // ISR / receive callback).
        let wait = if matches!(state, State::Running { .. } | State::Countdown { .. }) {
            Duration::from_millis(10)
        } else {
            RECV_WAIT
        };
        // Captured before handle_frame, which resets the state on a stop.
        let run_player = match state {
            State::Running { ref player, .. } => player.clone(),
            _ => None,
        };
        match rx.recv_timeout(wait) {
            Ok(frame) => {
                let elapsed =
                    handle_frame(frame, &link, &mut state, &mut samples, &mut pending_ping);
                if let Some(elapsed_us) = elapsed {
                    let score = scores.push(
                        run_player.as_deref().unwrap_or("-"),
                        elapsed_us.max(0) as u64,
                    );
                    log::info!(
                        "score #{}: player {} -> {}",
                        score.seq,
                        score.player,
                        format_elapsed(elapsed_us)
                    );
                    result_until = Some((elapsed_us, now_us() + RESULT_HOLD_US));
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => unreachable!(),
        }

        // On-board red LED: on while a measurement is running. Re-asserted
        // from state every pass (same pattern as the end node), so the LED
        // always follows state transitions wherever they happen above.
        #[cfg(not(feature = "qemu"))]
        match state {
            State::Running { .. } => led.set_high().unwrap(),
            _ => led.set_low().unwrap(),
        }

        #[cfg(feature = "qemu")]
        if state == State::Armed && !auto_start_scheduled {
            Trigger::schedule_at(now_us() + AUTO_START_DELAY_US);
            auto_start_scheduled = true;
        }

        // Broadcast a sync ping every PING_INTERVAL_US.
        let now = now_us();
        if now.saturating_sub(last_ping_us) >= PING_INTERVAL_US {
            seq = seq.wrapping_add(1);
            pending_ping = Some((seq, now));
            link.broadcast(&Message::Ping {
                seq,
                t_send_us: now,
            });
            last_ping_us = now;
        }

        // Tell the player-ID source whether a run is active, so expensive
        // sources (the camera) pause their polling mid-run. Idempotent
        // hint, re-asserted every pass; a no-op for the UART scanner.
        #[cfg(not(feature = "qemu"))]
        player_source.set_enable(!matches!(
            state,
            State::Running { .. } | State::Countdown { .. }
        ));

        // Player ID: a new scan replaces the current player. Forcing a
        // screen "change" redraws the current screen in place, so the new
        // player appears on the idle screen (and other states are simply
        // redrawn unchanged).
        #[cfg(not(feature = "qemu"))]
        if let Some(scan) = player_source.take() {
            player = Some(scan);
            shown_screen = None;
        }

        // Refresh the screen only when the content changes.
        {
            let now = now_us();
            if result_until.is_some_and(|(_, until)| now >= until) {
                result_until = None;
            }
            let screen = match result_until {
                Some((elapsed_us, _)) => Screen::Result(elapsed_us),
                None => match state {
                    State::Syncing => Screen::Syncing,
                    State::Armed => Screen::Idle,
                    State::Countdown { t0_us, .. } => {
                        let at_ms = now.saturating_sub(t0_us) / 1_000;
                        Screen::Countdown(3u8.saturating_sub((at_ms / BEEP_STEP_MS) as u8).max(1))
                    }
                    State::Running { t_start_us, .. } => {
                        Screen::Running(now.saturating_sub(t_start_us) / 1_000)
                    }
                },
            };
            if shown_screen != Some(screen) {
                match screen {
                    Screen::Syncing => display.show_syncing(),
                    Screen::Countdown(n) => display.show_countdown(n),
                    Screen::Idle => {
                        #[cfg(not(feature = "qemu"))]
                        let player_name = player.as_ref().map(PlayerScan::display_name);
                        #[cfg(feature = "qemu")]
                        let player_name = None;
                        display.show_idle(player_name);
                    }
                    Screen::Running(millis) => display.show_running(millis as i64 * 1_000),
                    Screen::Result(elapsed_us) => display.show_result(elapsed_us),
                }
                shown_screen = Some(screen);
            }
        }
    }
}

/// Returns the elapsed time (us) when an end event completed a run, so the
/// caller can put it on screen.
fn handle_frame(
    frame: RxFrame,
    link: &Link,
    state: &mut State,
    samples: &mut VecDeque<(i64, u64)>,
    pending_ping: &mut Option<(u32, u64)>,
) -> Option<i64> {
    let msg = match timer_protocol::decode(frame.payload()) {
        Ok(msg) => msg,
        Err(_) => return None, // not ours
    };

    match msg {
        Message::Pong {
            seq,
            t_recv_us,
            t_send_us,
        } => {
            let Some(&(pending_seq, t1)) = pending_ping.as_ref() else {
                return None; // stale pong
            };
            if seq != pending_seq {
                return None;
            }
            let t4 = frame.t_rx_us;
            *pending_ping = None;
            let (offset, rtt) = compute_offset_rtt(t1, t_recv_us, t_send_us, t4);
            if rtt > MAX_GOOD_RTT_US {
                log::debug!("sync sample dropped, rtt {rtt} us");
                return None;
            }
            samples.push_back((offset, rtt));
            if samples.len() > SYNC_WINDOW {
                samples.pop_front();
            }
            let (best_offset, best_rtt) = best_offset(samples).expect("samples non-empty");
            log::debug!(
                "sync sample: offset {offset} us, rtt {rtt} us (best {best_offset}/{best_rtt})"
            );
            link.broadcast(&Message::SyncInfo {
                offset_us: best_offset,
                rtt_us: best_rtt,
            });
            if samples.len() >= MIN_SYNC_SAMPLES {
                log::info!(
                    "clock sync OK: offset {best_offset} us, rtt {best_rtt} us - timer ARMED"
                );
                if *state == State::Syncing {
                    *state = State::Armed;
                }
            }
        }
        Message::EndEvent { t_end_us } => match state {
            State::Running { t_start_us, .. } => {
                let Some((offset, _)) = best_offset(samples) else {
                    log::error!("end event received but no clock sync yet");
                    return None;
                };
                let t_end_start_domain = timer_protocol::end_to_start_domain(t_end_us, offset);
                let elapsed = t_end_start_domain - *t_start_us as i64;
                if elapsed < 0 {
                    log::error!("negative elapsed time ({elapsed} us); clock sync lost?");
                } else {
                    log::info!(
                        "======= ELAPSED: {} ({elapsed} us) =======",
                        format_elapsed(elapsed)
                    );
                    *state = State::Armed;
                    return Some(elapsed);
                }
                *state = State::Armed;
            }
            _ => log::warn!("end event while not running; ignored"),
        },
        // We are the source of pings/start events; SyncInfo is for the end node.
        Message::Ping { .. } | Message::StartEvent { .. } | Message::SyncInfo { .. } => {}
    }
    None
}

/// Lowest-RTT sample in the window.
fn best_offset(samples: &VecDeque<(i64, u64)>) -> Option<(i64, u64)> {
    samples.iter().copied().min_by_key(|&(_, rtt)| rtt)
}
