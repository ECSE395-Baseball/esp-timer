//! Start node of the two-part ESP-NOW timer.
//!
//! * Acts as the clock-sync master: periodically pings the end node and
//!   keeps a best-offset estimate
//! * On its trigger input (the onboard user button on hardware; a scheduled
//!   virtual trigger in QEMU builds) it plays a 3-2-1-go countdown on the
//!   buzzer, then stamps `t_start` at the "go" beep and broadcasts it.
//! * On `EndEvent` from the end node it converts the remote timestamp into
//!   its own clock domain and reports the elapsed time.
//! * Records every finished run as a score (player + elapsed time) and
//!   serves them over BLE for a phone/laptop to sync to the database
//! * Drives the screen
//! * Identifies players via a USB HID or camera QR scanner

// esp-idf-sys emits the chip cfgs (`esp32`/`esp32s3`) without check-cfg
// declarations.
#![allow(unexpected_cfgs)]

use std::collections::VecDeque;
use std::time::Duration;

use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::log::EspLogger;
use esp_idf_svc::nvs::EspDefaultNvsPartition;

#[cfg(feature = "qemu")]
use firmware_common::Trigger;
use firmware_common::{
    format_elapsed, init_link, now_us, Connect, Link, PlayerIdSource, PlayerScan, RxFrame, ScoreLog,
};

// All peripheral-unit and GPIO assignments live in board.rs.
mod board;
use timer_protocol::{compute_offset_rtt, Message};

// Trace large heap allocations (see firmware_common::alloc_trace).
#[global_allocator]
static ALLOC_TRACE: firmware_common::alloc_trace::AllocTrace =
    firmware_common::alloc_trace::AllocTrace;

/// How often a sync ping is broadcast (us).
const PING_INTERVAL_US: u64 = 500_000;
/// Sync samples needed before the node considers itself armed.
const MIN_SYNC_SAMPLES: usize = 5;
/// Sliding window for the lowest-RTT offset estimate.
const SYNC_WINDOW: usize = 16;
/// Samples with an RTT above this are discarded as wild outliers (us).
const MAX_GOOD_RTT_US: u64 = 100_000;
/// How many times event messages are broadcast (no ACKs on broadcast).
const EVENT_REPEATS: u32 = 3;
/// Main-loop wakeup period while not running; only affects reporting
/// latency (timestamps are taken in the trigger ISR / receive callback).
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
/// Short beep length and the beep-to-beep spacing.
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

    // Trigger input: the onboard user button (to ground when pressed) on
    // hardware, a scheduled virtual trigger under QEMU.
    let mut trigger = board.trigger.connect().expect("trigger setup failed");

    // The screen: SSD1306 OLED by default, onboard ST7789 with
    // `--features tft-display`, QEMU's virtual RGB panel under emulation.
    let mut display = board.display.connect().expect("display setup failed");

    // Player ID source: USB HID barcode scanner by default (stub on plain
    // esp32, mock under QEMU), the camera with `--features camera`.
    let player_source = board.player_id.connect().expect("player id setup failed");
    // Most recently scanned player, shown on the idle screen; cleared when
    // a run completes so every run needs a fresh scan.
    let mut player: Option<PlayerScan> = None;

    // Live detector-view feed for aiming cards on the TFT; only
    // camera+TFT builds have the two ends of it.
    #[cfg(all(feature = "camera", feature = "tft-display", not(feature = "qemu")))]
    let preview_slot = player_source.preview();
    #[cfg(all(feature = "camera", feature = "tft-display", not(feature = "qemu")))]
    let mut shown_preview_seq = 0u64;

    let mut shown_screen: Option<Screen> = None;
    // Result on screen until this time: (elapsed_us, show-until us).
    let mut result_until: Option<(i64, u64)> = None;
    // Buzzer silenced at this time (end of the current beep).
    let mut buzzer_off_at: Option<u64> = None;

    // On-board red LED: on while a measurement is running. (No usable
    // GPIO under QEMU; log instead there.)
    let mut led = board.led.connect().expect("led setup failed");

    // Start-cue buzzer: passive piezo on LEDC PWM (silent under QEMU).
    let mut buzzer = board.buzzer.connect().expect("buzzer setup failed");

    // Finished-run scores, served over BLE (hardware only; QEMU keeps
    // the log). This thread only sets up GATT and blocks on log changes.
    let scores = ScoreLog::new();
    #[cfg(not(feature = "qemu"))]
    std::thread::Builder::new()
        .stack_size(8 * 1024)
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
            } else if !matches!(state, State::Armed) {
                // Also swallows spurious edges latched while the pin was
                // floating/unpowered during boot.
                log::warn!("trigger ignored: not armed yet (still syncing)");
            } else {
                log::info!("start cue at {t_press_us} us: 3-2-1-go");
                let run_player = player.as_ref().map(|p| p.uuid.clone());
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

        // Handle received frames promptly; wake more often while
        // running/counting down so the timer rolls and beeps are on time.
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
                    // The card pays for exactly one run; the next player
                    // scans their own card.
                    player = None;
                    result_until = Some((elapsed_us, now_us() + RESULT_HOLD_US));
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => unreachable!(),
        }

        // On-board red LED: re-asserted from state every pass, so it
        // follows transitions wherever they happen above.
        match state {
            State::Running { .. } => led.set(true),
            _ => led.set(false),
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

        // Tell the player-ID source whether a run is active so expensive
        // sources (the camera) pause mid-run; idempotent, re-asserted every pass.
        player_source.set_enable(!matches!(
            state,
            State::Running { .. } | State::Countdown { .. }
        ));

        // Player ID: a new scan replaces the current player; forcing a
        // screen "change" redraws so the player shows on the idle screen.
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
            // The scanner preview repaints whenever a fresh frame lands.
            #[allow(unused_mut)]
            let mut refresh = shown_screen != Some(screen);
            #[cfg(all(feature = "camera", feature = "tft-display", not(feature = "qemu")))]
            if screen == Screen::Idle {
                if let Some(slot) = &preview_slot {
                    let seq = slot.lock().map(|f| f.seq).unwrap_or(0);
                    if seq != 0 && seq != shown_preview_seq {
                        refresh = true;
                        shown_preview_seq = seq;
                    }
                }
            }
            if refresh {
                match screen {
                    Screen::Syncing => display.show_syncing(),
                    Screen::Countdown(n) => display.show_countdown(n),
                    Screen::Idle => {
                        #[cfg(not(all(feature = "qemu", feature = "camera")))]
                        let player_name = player.as_ref().map(PlayerScan::display_name);
                        #[cfg(all(feature = "qemu", feature = "camera"))]
                        let player_name = None;
                        #[cfg(all(
                            feature = "camera",
                            feature = "tft-display",
                            not(feature = "qemu")
                        ))]
                        match &preview_slot {
                            Some(slot) => {
                                let frame = slot.lock().unwrap_or_else(|e| e.into_inner());
                                if frame.seq != 0 {
                                    display.show_scanner(&frame, player_name);
                                } else {
                                    // No camera frame yet.
                                    display.show_idle(player_name);
                                }
                            }
                            None => display.show_idle(player_name),
                        }
                        #[cfg(not(all(
                            feature = "camera",
                            feature = "tft-display",
                            not(feature = "qemu")
                        )))]
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
