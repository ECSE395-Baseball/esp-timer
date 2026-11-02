//! Finished-run scores and the BLE sync service that hands them to a
//! companion device (phone or laptop), which forwards them to the scores
//! database. Only the start node records and serves scores, but the log
//! itself is platform-neutral; under QEMU the log works and the BLE
//! server is compiled out.
//!
//! BLE profile (custom 128-bit UUIDs below):
//! * device name `ESPNOW Timer`, advertising the service UUID;
//! * `results` characteristic (READ + NOTIFY): all un-acked scores as
//!   `\n`-joined CSV lines `seq,player_uuid,elapsed_us,at_ms`, where
//!   `at_ms` is milliseconds since node boot when the run ended (the
//!   client adds its own wall-clock timestamp on receipt). A NOTIFY on
//!   this characteristic means a new score was appended; clients should
//!   read the full value (long read) rather than rely on the
//!   notification payload, which is truncated to the negotiated MTU.
//! * `command` characteristic (WRITE): `ACK <seq>` drops every score
//!   with `seq <= <seq>` (written by the client once those scores are
//!   safely in the database); `CLEAR` drops all scores.

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};

use crate::time::now_us;

/// Advertised device name.
pub const DEVICE_NAME: &str = "ESPNOW Timer";
/// GATT service carrying the score-sync characteristics.
pub const SERVICE_UUID: &str = "35540000-2495-4e4f-5721-54494d455221";
/// READ+NOTIFY characteristic: pending scores as CSV lines.
pub const RESULTS_UUID: &str = "35540001-2495-4e4f-5721-54494d455221";
/// WRITE characteristic: `ACK <seq>` / `CLEAR`.
pub const COMMAND_UUID: &str = "35540002-2495-4e4f-5721-54494d455221";

/// How many un-acked scores are kept before the oldest are dropped.
/// At ~30 bytes per CSV line this stays well under the GATT long-read
/// practical limits.
const MAX_PENDING: usize = 64;

/// One finished run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Score {
    /// Monotonic per-boot sequence number; also the ack cursor.
    pub seq: u32,
    /// Player UUID as scanned, or "-" when no player was scanned.
    pub player: String,
    /// Measured time in microseconds.
    pub elapsed_us: u64,
    /// Milliseconds since node boot when the run ended.
    pub at_ms: u64,
}

impl Score {
    /// `seq,player_uuid,elapsed_us,at_ms` (UUIDs contain no commas).
    pub fn to_csv(&self) -> String {
        std::format!(
            "{},{},{},{}",
            self.seq,
            self.player,
            self.elapsed_us,
            self.at_ms
        )
    }
}

/// Bounded, thread-safe log of scores awaiting database sync. Cloning
/// shares the same log.
#[derive(Clone)]
pub struct ScoreLog {
    shared: Arc<Shared>,
}

struct Shared {
    state: Mutex<State>,
    /// Signalled on every push/ack/clear so the BLE task can sleep
    /// instead of polling.
    changed: Condvar,
}

struct State {
    pending: VecDeque<Score>,
    next_seq: u32,
    /// Bumped on every mutation; the BLE task diffs against it.
    generation: u64,
}

impl ScoreLog {
    pub fn new() -> Self {
        Self {
            shared: Arc::new(Shared {
                state: Mutex::new(State {
                    pending: VecDeque::new(),
                    next_seq: 1,
                    generation: 0,
                }),
                changed: Condvar::new(),
            }),
        }
    }

    /// Record a finished run and return the stored score.
    pub fn push(&self, player: &str, elapsed_us: u64) -> Score {
        let mut st = self.shared.state.lock().unwrap();
        let score = Score {
            seq: st.next_seq,
            player: player.to_string(),
            elapsed_us,
            at_ms: now_us() / 1_000,
        };
        st.next_seq += 1;
        st.pending.push_back(score.clone());
        while st.pending.len() > MAX_PENDING {
            st.pending.pop_front();
        }
        st.generation += 1;
        drop(st);
        self.shared.changed.notify_all();
        score
    }

    /// Drop every score with `seq <= acked` (client confirmed sync).
    pub fn ack(&self, acked: u32) {
        let mut st = self.shared.state.lock().unwrap();
        let before = st.pending.len();
        st.pending.retain(|s| s.seq > acked);
        if st.pending.len() != before {
            st.generation += 1;
            drop(st);
            self.shared.changed.notify_all();
        }
    }

    /// Drop all scores.
    pub fn clear(&self) {
        let mut st = self.shared.state.lock().unwrap();
        if !st.pending.is_empty() {
            st.pending.clear();
            st.generation += 1;
            drop(st);
            self.shared.changed.notify_all();
        }
    }

    /// All pending scores as `\n`-joined CSV lines ("" when none).
    pub fn to_csv(&self) -> String {
        let st = self.shared.state.lock().unwrap();
        st.pending
            .iter()
            .map(Score::to_csv)
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Latest pushed sequence number (0 when nothing was ever pushed).
    #[cfg(not(feature = "qemu"))]
    fn latest_seq(&self) -> u32 {
        self.shared.state.lock().unwrap().next_seq - 1
    }

    /// Block until the log changes (or `timeout` elapses); returns the
    /// current generation counter.
    #[cfg(not(feature = "qemu"))]
    fn wait_change(&self, seen: u64, timeout: std::time::Duration) -> u64 {
        let st = self.shared.state.lock().unwrap();
        if st.generation != seen {
            return st.generation;
        }
        let (st, _) = self.shared.changed.wait_timeout(st, timeout).unwrap();
        st.generation
    }
}

/// Serve the score log over BLE; never returns. Runs on its own thread
/// (NimBLE spins its own host task internally).
#[cfg(not(feature = "qemu"))]
pub fn serve(log: ScoreLog) -> ! {
    use esp32_nimble::utilities::BleUuid;
    use esp32_nimble::{BLEAdvertisementData, BLEDevice, NimbleProperties};

    let device = BLEDevice::take();
    BLEDevice::set_device_name(DEVICE_NAME).expect("ble device name");
    let server = device.get_server();
    // Re-advertise when a client disconnects so the next sync can connect.
    server.advertise_on_disconnect(true);
    server.on_connect(|_server, desc| {
        log::info!("BLE peer connected: handle {}", desc.conn_handle());
    });

    let uuid = |s: &str| BleUuid::from_uuid128_string(s).expect("static UUID");
    let service = server.create_service(uuid(SERVICE_UUID));
    let results = service.lock().create_characteristic(
        uuid(RESULTS_UUID),
        NimbleProperties::READ | NimbleProperties::NOTIFY,
    );
    results.lock().set_value(b"");
    let command = service
        .lock()
        .create_characteristic(uuid(COMMAND_UUID), NimbleProperties::WRITE);
    let cmd_log = log.clone();
    command.lock().on_write(move |args| {
        let Ok(text) = core::str::from_utf8(args.recv_data()) else {
            log::warn!("BLE: non-UTF8 command ignored");
            return;
        };
        let text = text.trim();
        if let Some(seq) = text
            .strip_prefix("ACK ")
            .and_then(|s| s.trim().parse::<u32>().ok())
        {
            cmd_log.ack(seq);
            log::info!("BLE: scores acked through seq {seq}");
        } else if text.eq_ignore_ascii_case("CLEAR") {
            cmd_log.clear();
            log::info!("BLE: scores cleared");
        } else {
            log::warn!("BLE: unknown command {text:?}");
        }
    });

    let advertising = device.get_advertising();
    advertising
        .lock()
        .set_data(
            BLEAdvertisementData::new()
                .name(DEVICE_NAME)
                .add_service_uuid(uuid(SERVICE_UUID)),
        )
        .expect("ble advertisement");
    advertising.lock().start().expect("ble advertising start");
    log::info!("BLE score sync advertising as \"{DEVICE_NAME}\"");

    // Publish changes: refresh the readable value on any mutation and
    // notify subscribers when a new score was appended.
    let mut seen_generation = 0u64;
    let mut announced_seq = 0u32;
    loop {
        let generation = log.wait_change(seen_generation, std::time::Duration::from_millis(500));
        if generation == seen_generation {
            continue;
        }
        seen_generation = generation;
        let latest = log.latest_seq();
        let csv = log.to_csv();
        let mut results = results.lock();
        results.set_value(csv.as_bytes());
        if latest > announced_seq {
            announced_seq = latest;
            results.notify();
        }
    }
}
