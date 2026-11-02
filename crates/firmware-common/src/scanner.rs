//! QR/barcode scanner reader, behind the `usb` feature.
//!
//! The ESP32 has no USB host peripheral, so a "USB" scanner is wired in
//! its serial mode: QR reader modules (GM65/GM67-style, and USB wedges
//! configured for USB-CDC/serial output) present scanned codes as lines of
//! ASCII on a UART. A reader task collects bytes into lines, parses them
//! with the shared [`player`] format, and queues [`PlayerScan`]s for the
//! main loop

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use esp_idf_svc::hal::delay;
use esp_idf_svc::hal::gpio::{AnyInputPin, AnyOutputPin, InputPin};
use esp_idf_svc::hal::uart::{config::Config as UartConfig, Uart, UartRxDriver};
use esp_idf_svc::hal::units::Hertz;
use esp_idf_svc::sys::EspError;

use crate::player::{parse_scan_line, PlayerIdSource, PlayerScan};

/// Longest accepted scan line (uuid + space + name) before the line is
/// discarded as garbage.
const MAX_LINE_LEN: usize = 128;
const SCAN_PERIOD: Duration = Duration::from_millis(200);

/// QR/barcode scanner handle; the reader task lives as long as this
/// handle is not leaked
pub struct Scanner {
    rx: Receiver<PlayerScan>,
    _task: JoinHandle<()>,
    reader_enabled: Arc<AtomicBool>,
}

impl Scanner {
    /// Default baud rate for QR reader modules (8N1); many ship at 9600.
    pub const DEFAULT_BAUDRATE: Hertz = Hertz(9600);

    /// Start the reader task on the given UART peripheral, receiving on
    /// `rx_pin`.
    pub fn new(
        uart: impl Uart + Send + 'static,
        rx_pin: impl InputPin + 'static,
        baudrate: Hertz,
    ) -> Result<Self, EspError> {
        let (tx, rx) = channel::<PlayerScan>();
        let reader_enabled = Arc::new(AtomicBool::new(true));
        let task_enabled = Arc::clone(&reader_enabled);
        let task = std::thread::Builder::new()
            .name("scanner".into()) // FreeRTOS task names cap at 16 chars
            .stack_size(3072)
            .spawn(move || reader_task(uart, rx_pin, baudrate, tx, task_enabled))
            .expect("failed to spawn scanner task");
        Ok(Self {
            rx,
            _task: task,
            reader_enabled,
        })
    }
}

impl PlayerIdSource for Scanner {
    fn take(&self) -> Option<PlayerScan> {
        self.rx.try_recv().ok()
    }

    fn set_enable(&self, enable: bool) {
        // Pause polling when not actively attempting to scan
        self.reader_enabled.store(!enable, Ordering::Relaxed);
    }
}

/// Read bytes when enabled, assembling lines and parsing complete scans.
///
/// The UART driver is constructed inside the task so nothing has to be
/// `Send`-shared with the main thread — only the channel crosses.
fn reader_task(
    uart: impl Uart + Send + 'static,
    rx_pin: impl InputPin + 'static,
    baudrate: Hertz,
    tx: Sender<PlayerScan>,
    reader_enabled: Arc<AtomicBool>,
) -> ! {
    let config = UartConfig::new().baudrate(baudrate);
    let uart = match UartRxDriver::new(
        uart,
        rx_pin,
        None::<AnyInputPin>,
        None::<AnyOutputPin>,
        &config,
    ) {
        Ok(uart) => uart,
        Err(e) => {
            log::error!("scanner UART setup failed: {e}");
            // Park the task; a dead scanner must not wedge the node.
            loop {
                std::thread::sleep(std::time::Duration::from_secs(3600));
            }
        }
    };

    let mut line = [0u8; MAX_LINE_LEN];
    let mut len = 0usize;
    let mut buf = [0u8; 64];
    loop {
        if !reader_enabled.load(Ordering::Relaxed) {
            std::thread::sleep(SCAN_PERIOD);
            continue;
        }
        let Ok(n) = uart.read(&mut buf, delay::BLOCK) else {
            continue;
        };
        for &b in &buf[..n] {
            match b {
                b'\r' | b'\n' => {
                    if len > 0 {
                        emit(&line[..len], &tx);
                        len = 0;
                    }
                }
                _ => {
                    if len < MAX_LINE_LEN {
                        line[len] = b;
                        len += 1;
                    } else {
                        // Overlong line: drop it entirely.
                        log::warn!("scanner: discarding overlong line");
                        len = 0;
                    }
                }
            }
        }
    }
}

fn emit(line: &[u8], tx: &Sender<PlayerScan>) {
    let text = core::str::from_utf8(line).unwrap_or("");
    match parse_scan_line(text) {
        Some(scan) => {
            log::info!("player scanned: {} (uuid {})", scan.name, scan.uuid);
            // If the main loop is wedged we simply drop scans.
            let _ = tx.send(scan);
        }
        None => log::debug!("scanner: ignoring unrecognized line: {text:?}"),
    }
}
