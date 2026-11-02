//! Channel simulator between emulated ESP32 timer nodes.
//!
//! Each QEMU instance connects its NIC (QEMU "socket" netdev) to one of the
//! ports listed on the command line. Frames are forwarded to all other
//! nodes after a configurable delay, with optional jitter and loss, so the
//! timer's clock-sync behavior can be tested under controlled link
//! conditions.
//!
//! Wire format per connection (QEMU stream socket netdev framing):
//! 4-byte big-endian length, then one raw Ethernet frame.
//!
//! The hub also tracks ground truth for the clock-sync protocol: it knows
//! the exact delay applied to every forwarded Ping, and the matching Pong
//! carries the receive timestamp, so `offset_true ≈ t_recv - t_send -
//! applied_delay`. Logged as OFFSET_TRUE per ping/pong pair, and as
//! OFFSET_CHECK (estimate vs truth) when a StartEvent passes.
//!
//! Usage: channel-sim [--delay-ms D] [--jitter-ms J] [--loss-pct P]
//!                    [--seed S] [--quiet] PORT [PORT...]

use std::collections::BinaryHeap;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{channel, Sender};
use std::time::{Duration, Instant};

/// EtherType used by the timer firmware's raw-Ethernet link (QEMU mode).
const TIMER_ETHERTYPE: u16 = 0x88B5;

struct Config {
    delay: Duration,
    jitter: Duration,
    loss_pct: f64,
    seed: u64,
    quiet: bool,
    ports: Vec<u16>,
}

/// Frame due for delivery to a node.
struct Due {
    at: Instant,
    dst: usize,
    frame: Vec<u8>,
}

impl PartialEq for Due {
    fn eq(&self, other: &Self) -> bool {
        self.at == other.at
    }
}
impl Eq for Due {}
impl PartialOrd for Due {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Due {
    // Reversed: smallest `at` has highest priority.
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other.at.cmp(&self.at)
    }
}

fn main() {
    let cfg = parse_args();
    let t0 = Instant::now();

    // Deterministic PRNG (xorshift64*), seeded from --seed.
    let mut rng = cfg.seed.max(1);
    let mut next_rand = move || {
        rng ^= rng >> 12;
        rng ^= rng << 25;
        rng ^= rng >> 27;
        rng = rng.wrapping_mul(0x2545_F491_4F6C_DD1D);
        (rng >> 32) as f64 / (u32::MAX as f64 + 1.0)
    };

    // Bind all ports first (nodes connect at once), then accept.
    let listeners: Vec<(u16, TcpListener)> = cfg
        .ports
        .iter()
        .map(|&port| {
            let listener = TcpListener::bind(("127.0.0.1", port)).expect("bind failed");
            eprintln!("[hub] listening on port {port}");
            (port, listener)
        })
        .collect();

    let mut conns: Vec<TcpStream> = Vec::new();
    for (port, listener) in listeners {
        let (stream, peer) = listener.accept().expect("accept failed");
        eprintln!("[hub] node connected on port {port} from {peer}");
        stream.set_nodelay(true).ok();
        conns.push(stream);
    }
    eprintln!("[hub] all {} nodes connected", conns.len());

    // One writer channel per node.
    let mut writers: Vec<Sender<Vec<u8>>> = Vec::new();
    let (central_tx, central_rx) = channel::<(usize, Vec<u8>)>();

    for (idx, stream) in conns.iter().enumerate() {
        let port = cfg.ports[idx];

        let (wtx, wrx) = channel::<Vec<u8>>();
        writers.push(wtx);
        let mut wstream = stream.try_clone().expect("clone failed");
        std::thread::spawn(move || {
            while let Ok(frame) = wrx.recv() {
                let len = (frame.len() as u32).to_be_bytes();
                if wstream
                    .write_all(&len)
                    .and_then(|_| wstream.write_all(&frame))
                    .is_err()
                {
                    eprintln!("[hub] writer for port {port}: connection closed");
                    return;
                }
            }
        });

        let mut rstream = stream.try_clone().expect("clone failed");
        let rtx = central_tx.clone();
        std::thread::spawn(move || loop {
            let mut len_buf = [0u8; 4];
            if rstream.read_exact(&mut len_buf).is_err() {
                eprintln!("[hub] reader for port {port}: connection closed");
                return;
            }
            let len = u32::from_be_bytes(len_buf) as usize;
            if len > 65536 {
                eprintln!("[hub] reader for port {port}: bogus frame length {len}");
                return;
            }
            let mut frame = vec![0u8; len];
            if rstream.read_exact(&mut frame).is_err() {
                eprintln!("[hub] reader for port {port}: connection closed mid-frame");
                return;
            }
            if rtx.send((idx, frame)).is_err() {
                return;
            }
        });
    }

    // Scheduler: apply loss, delay frames, fan out to all other nodes.
    let mut queue: BinaryHeap<Due> = BinaryHeap::new();

    // Ground-truth clock-offset tracking. For each forwarded Ping, remember
    // its send timestamp (start clock) and the exact delay applied to its
    // delivery; the matching Pong's receive timestamp (end clock)
    let mut pings: HashMap<(usize, u32), (u64, i64)> = HashMap::new();
    let mut offset_truth: Option<i64> = None;
    let mut offset_est: Option<i64> = None;

    loop {
        // Wait for an inbound frame or the next delivery deadline.
        let inbound = match queue.peek() {
            Some(due) => {
                let now = Instant::now();
                if due.at > now {
                    central_rx.recv_timeout(due.at - now).ok()
                } else {
                    None
                }
            }
            None => central_rx.recv().ok(),
        };

        if let Some((src, frame)) = inbound {
            let msg = decode_frame(&frame);
            log(&cfg, t0, || {
                format!(
                    "recv port={} ({} B){}",
                    cfg.ports[src],
                    frame.len(),
                    msg_suffix(&msg)
                )
            });
            if cfg.loss_pct > 0.0 && next_rand() * 100.0 < cfg.loss_pct {
                log(&cfg, t0, || format!("DROP from port {}", cfg.ports[src]));
                continue;
            }
            let jitter = (next_rand() * 2.0 - 1.0) * cfg.jitter.as_secs_f64();
            let delay = (cfg.delay.as_secs_f64() + jitter).max(0.0);
            let at = Instant::now() + Duration::from_secs_f64(delay);
            let delay_us = (delay * 1e6).round() as i64;
            for dst in 0..conns.len() {
                if dst != src {
                    if let Some(timer_protocol::Message::Ping { seq, t_send_us }) = msg {
                        pings.insert((dst, seq), (t_send_us, delay_us));
                    }
                    queue.push(Due {
                        at,
                        dst,
                        frame: frame.clone(),
                    });
                }
            }
            match msg {
                Some(timer_protocol::Message::Pong { seq, t_recv_us, .. }) => {
                    if let Some((t_send_us, applied_us)) = pings.remove(&(src, seq)) {
                        let truth = t_recv_us as i64 - t_send_us as i64 - applied_us;
                        offset_truth = Some(truth);
                        log(&cfg, t0, || {
                            format!(
                                "OFFSET_TRUE offset={truth} us (seq={seq}, applied_delay={applied_us} us)"
                            )
                        });
                    }
                }
                Some(timer_protocol::Message::SyncInfo { offset_us, .. }) => {
                    offset_est = Some(offset_us);
                }
                Some(timer_protocol::Message::StartEvent { .. }) => {
                    if let (Some(est), Some(truth)) = (offset_est, offset_truth) {
                        let error = est - truth;
                        log(&cfg, t0, || {
                            format!("OFFSET_CHECK est={est} us true={truth} us error={error} us")
                        });
                    }
                }
                _ => {}
            }
        }

        // Deliver due frames.
        let now = Instant::now();
        while let Some(due) = queue.peek() {
            if due.at > now {
                break;
            }
            let due = queue.pop().expect("peeked");
            log(&cfg, t0, || {
                format!(
                    "send port={} ({} B){}",
                    cfg.ports[due.dst],
                    due.frame.len(),
                    msg_suffix(&decode_frame(&due.frame))
                )
            });
            let _ = writers[due.dst].send(due.frame);
        }
    }
}

/// Decode a frame carrying a timer protocol message.
fn decode_frame(frame: &[u8]) -> Option<timer_protocol::Message> {
    if frame.len() >= 14 && u16::from_be_bytes([frame[12], frame[13]]) == TIMER_ETHERTYPE {
        timer_protocol::decode(&frame[14..]).ok()
    } else {
        None
    }
}

/// Describe a decoded message for logging.
fn msg_suffix(msg: &Option<timer_protocol::Message>) -> String {
    msg.as_ref()
        .map(|m| format!(" {}", fmt_msg(m)))
        .unwrap_or_default()
}

fn fmt_msg(msg: &timer_protocol::Message) -> String {
    use timer_protocol::Message::*;
    match *msg {
        Ping { seq, t_send_us } => format!("Ping(seq={seq}, t_send={t_send_us})"),
        Pong {
            seq,
            t_recv_us,
            t_send_us,
        } => format!("Pong(seq={seq}, t_recv={t_recv_us}, t_send={t_send_us})"),
        SyncInfo { offset_us, rtt_us } => format!("SyncInfo(offset={offset_us}, rtt={rtt_us})"),
        StartEvent { t_start_us } => format!("StartEvent(t_start={t_start_us})"),
        EndEvent { t_end_us } => format!("EndEvent(t_end={t_end_us})"),
    }
}

fn log(cfg: &Config, t0: Instant, msg: impl FnOnce() -> String) {
    if !cfg.quiet {
        eprintln!(
            "[hub +{:10.3}ms] {}",
            t0.elapsed().as_secs_f64() * 1e3,
            msg()
        );
    }
}

fn parse_args() -> Config {
    let mut cfg = Config {
        delay: Duration::from_millis(5),
        jitter: Duration::from_millis(1),
        loss_pct: 0.0,
        seed: 1,
        quiet: false,
        ports: Vec::new(),
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--delay-ms" => cfg.delay = Duration::from_millis(parse_val(&mut args, "--delay-ms")),
            "--jitter-ms" => {
                cfg.jitter = Duration::from_millis(parse_val(&mut args, "--jitter-ms"))
            }
            "--loss-pct" => cfg.loss_pct = parse_val(&mut args, "--loss-pct"),
            "--seed" => cfg.seed = parse_val(&mut args, "--seed"),
            "--quiet" => cfg.quiet = true,
            "--help" | "-h" => {
                eprintln!(
                    "usage: channel-sim [--delay-ms D] [--jitter-ms J] [--loss-pct P]\n                   [--seed S] [--quiet] PORT [PORT...]"
                );
                std::process::exit(0);
            }
            _ => cfg.ports.push(arg.parse().unwrap_or_else(|_| {
                eprintln!("invalid port: {arg}");
                std::process::exit(2);
            })),
        }
    }
    if cfg.ports.len() < 2 {
        eprintln!("error: at least two PORTs are required (try --help)");
        std::process::exit(2);
    }
    cfg
}

fn parse_val<T: std::str::FromStr>(args: &mut impl Iterator<Item = String>, name: &str) -> T {
    let v = args.next().unwrap_or_else(|| {
        eprintln!("missing value for {name}");
        std::process::exit(2);
    });
    v.parse().unwrap_or_else(|_| {
        eprintln!("invalid value for {name}: {v}");
        std::process::exit(2);
    })
}
