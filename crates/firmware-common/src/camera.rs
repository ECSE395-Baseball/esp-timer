//! Onboard-camera QR reader (start node), behind the `camera` feature.
//!
//! Frames come from an image sensor (e.g. OV5640) driven by the vendored
//! `esp32-camera` ESP-IDF component (grayscale QVGA over SCCB + I2S/DMA);
//! QR codes are decoded from the frames with the pure-Rust `rqrr` crate.
//! Decoded text uses the shared [`player`] scan-line format
//! (`<uuid> <player name>`), so this is interchangeable with the USB
//! [`Scanner`]: both implement [`PlayerIdSource`].
//!
//! [`Scanner`]: crate::Scanner

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use esp_idf_svc::sys::camera::{
    camera_config_t, camera_fb_location_t_CAMERA_FB_IN_DRAM,
    camera_grab_mode_t_CAMERA_GRAB_WHEN_EMPTY, esp_camera_fb_get, esp_camera_fb_return,
    esp_camera_init, esp_camera_sensor_get, framesize_t_FRAMESIZE_QVGA,
    ledc_channel_t_LEDC_CHANNEL_0, ledc_timer_t_LEDC_TIMER_0, pixformat_t_PIXFORMAT_GRAYSCALE,
};
use esp_idf_svc::sys::{esp, EspError};

use crate::player::{parse_scan_line, PlayerIdSource, PlayerScan, PreviewFrame};

/// Pause between scan attempts; the actual scan rate is dominated by the
/// decode time (hundreds of ms for a 320x240 frame).
const SCAN_PERIOD: Duration = Duration::from_millis(100);

const DEBUG_FRAMEDUMP: bool = false;
/// Debug aid: dump every Nth frame over the log UART as base64 grayscale
/// (see `tools/grab_frame.py`); 0 disables. Pauses scanning while printing.
const FRAME_DUMP_PERIOD: u32 = 150;

/// GPIO numbers for the image sensor, named after the labels on the
/// OV2640-style 2x9-header module (`d9` = bus MSB, `d2` = LSB; -1 = unwired):
///
/// ```text
///  PD  D2  D4  D6  D8  XC  HS  SDA  G
///  RT  D3  D5  D7  D9  PC  VS  SCL  3V
/// ```
#[derive(Debug, Clone, Copy)]
pub struct CameraPins {
    pub pd: i32,
    pub rt: i32,
    pub xc: i32,
    pub sda: i32,
    pub scl: i32,
    pub d9: i32,
    pub d8: i32,
    pub d7: i32,
    pub d6: i32,
    pub d5: i32,
    pub d4: i32,
    pub d3: i32,
    pub d2: i32,
    pub vs: i32,
    pub hs: i32,
    pub pc: i32,
}

/// Camera QR reader handle; the capture task lives as long as this handle
/// is not leaked.
pub struct Camera {
    rx: Receiver<PlayerScan>,
    capture_enabled: Arc<AtomicBool>,
    preview: Arc<Mutex<PreviewFrame>>,
    _task: JoinHandle<()>,
}

impl PlayerIdSource for Camera {
    fn take(&self) -> Option<PlayerScan> {
        self.rx.try_recv().ok()
    }

    fn set_enable(&self, enable: bool) {
        // Pause frame polling while a run/countdown is active
        self.capture_enabled.store(enable, Ordering::Relaxed);
    }
}

impl Camera {
    /// Configure the sensor and start the capture/decode task.
    pub fn new(pins: CameraPins) -> Result<Self, EspError> {
        init_sensor(&pins)?;
        // Bounded channel, as in link.rs: allocated once up front.
        let (tx, rx) = sync_channel::<PlayerScan>(4);
        let capture_enabled = Arc::new(AtomicBool::new(true));
        let task_enabled = Arc::clone(&capture_enabled);
        let preview = Arc::new(Mutex::new(PreviewFrame {
            seq: 0,
            w: 0,
            h: 0,
            bits: Vec::new(),
        }));
        let task_preview = Arc::clone(&preview);
        let task = std::thread::Builder::new()
            .name("camera-qr".into()) // FreeRTOS task names cap at 16 chars
            // 24 KiB: a successful rqrr decode is the stack peak. (The
            // task shows up as "pthread"; ESP-IDF ignores the Rust name.)
            .stack_size(24 * 1024)
            .spawn(move || capture_task(tx, task_enabled, task_preview))
            .expect("failed to spawn camera task");
        Ok(Self {
            rx,
            capture_enabled,
            preview,
            _task: task,
        })
    }

    /// The shared slot the capture task publishes its binarized
    /// detector-view frames to (for the TFT's scanner-aiming preview).
    pub fn preview_slot(&self) -> Arc<Mutex<PreviewFrame>> {
        Arc::clone(&self.preview)
    }
}

/// Configure and start the image sensor: grayscale QVGA (rqrr needs >=4
/// px per QR module), decoded in place in the ~77 KiB driver buffer.
fn init_sensor(pins: &CameraPins) -> Result<(), EspError> {
    // Zeroed base: camera_config_t is plain ints/enums. esp32-camera
    // numbers the bus pin_d7 (MSB) .. pin_d0; the module labels them D9..D2.
    let mut config: camera_config_t = unsafe { core::mem::zeroed() };
    config.pin_pwdn = pins.pd;
    config.pin_reset = pins.rt;
    config.pin_xclk = pins.xc;
    config.pin_d7 = pins.d9;
    config.pin_d6 = pins.d8;
    config.pin_d5 = pins.d7;
    config.pin_d4 = pins.d6;
    config.pin_d3 = pins.d5;
    config.pin_d2 = pins.d4;
    config.pin_d1 = pins.d3;
    config.pin_d0 = pins.d2;
    config.pin_vsync = pins.vs;
    config.pin_href = pins.hs;
    config.pin_pclk = pins.pc;
    // The SCCB pin fields are anonymous unions in C, hence the bindgen
    // indirection. (Union field writes are safe; only reads are unsafe.)
    config.__bindgen_anon_1.pin_sccb_sda = pins.sda;
    config.__bindgen_anon_2.pin_sccb_scl = pins.scl;
    config.xclk_freq_hz = 10_000_000; // half the usual 20 MHz: slows the OV5640's pixel clock so the capture path keeps up at QVGA
    config.ledc_timer = ledc_timer_t_LEDC_TIMER_0;
    config.ledc_channel = ledc_channel_t_LEDC_CHANNEL_0;
    config.pixel_format = pixformat_t_PIXFORMAT_GRAYSCALE;
    config.frame_size = framesize_t_FRAMESIZE_QVGA;
    config.jpeg_quality = 0; // unused for grayscale
    config.fb_count = 1;
    config.fb_location = camera_fb_location_t_CAMERA_FB_IN_DRAM;
    config.grab_mode = camera_grab_mode_t_CAMERA_GRAB_WHEN_EMPTY;
    config.sccb_i2c_port = -1; // dedicated bit-banged SCCB pins
    esp!(unsafe { esp_camera_init(&config) })?;
    // The driver itself logs the probe details; reaching here means the
    // SCCB probe, sensor init and DMA setup all succeeded.
    log::info!("camera: sensor initialized");

    // The module delivers the image vertically mirrored, and QR codes are
    // chiral, so flip it in the sensor's ISP.
    let sensor = unsafe { esp_camera_sensor_get() };
    // Safety: esp_camera_init succeeded above, so the sensor state exists.
    let set_vflip = unsafe { sensor.as_ref() }.and_then(|s| s.set_vflip);
    if let Some(set_vflip) = set_vflip {
        esp!(unsafe { set_vflip(sensor, 1) })?;
    }
    Ok(())
}

/// Print raw grayscale pixels to the log as base64 lines (prefix `|`),
/// wrapped in BEGIN/END markers, for host tools (tools/grab_frame.py).
fn print_b64(label: &str, w: usize, h: usize, px: impl Iterator<Item = u8>) {
    const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    log::info!(
        "camera: {label} BEGIN ({w}x{h}, {} B, base64 raw gray)",
        w * h
    );
    let mut line = String::with_capacity(128);
    let mut chunk = [0u8; 3];
    let mut chunk_len = 0;
    for b in px {
        chunk[chunk_len] = b;
        chunk_len += 1;
        if chunk_len < 3 {
            continue;
        }
        let n = (chunk[0] as u32) << 16 | (chunk[1] as u32) << 8 | chunk[2] as u32;
        for shift in [18, 12, 6, 0] {
            line.push(B64[(n >> shift) as usize & 63] as char);
        }
        chunk_len = 0;
        if line.len() == 128 {
            log::info!("camera: |{line}");
            line.clear();
        }
    }
    if chunk_len > 0 {
        // Tail chunk: unused slots hold stale bytes — read them as 0.
        let b1 = if chunk_len > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk_len > 2 { chunk[2] as u32 } else { 0 };
        let n = (chunk[0] as u32) << 16 | b1 << 8 | b2;
        line.push(B64[(n >> 18) as usize & 63] as char);
        line.push(B64[(n >> 12) as usize & 63] as char);
        line.push(if chunk_len > 1 {
            B64[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        line.push(if chunk_len > 2 {
            B64[n as usize & 63] as char
        } else {
            '='
        });
    }
    if !line.is_empty() {
        log::info!("camera: |{line}");
    }
    log::info!("camera: {label} END");
}

/// Full-resolution sample frame for off-board decode debugging
/// (tools/grab_frame.py).
fn dump_frame(w: usize, h: usize, data: &[u8]) {
    print_b64("frame dump", w, h, data.iter().copied());
}

/// `rqrr::ImageBuffer` view over the camera driver's frame buffer, so QR
/// detection and binarization run in place (no ~77 KiB frame clone).
struct FrameView<'a> {
    w: usize,
    h: usize,
    pixels: &'a mut [u8],
}

impl rqrr::ImageBuffer for FrameView<'_> {
    fn width(&self) -> usize {
        self.w
    }

    fn height(&self) -> usize {
        self.h
    }

    fn get_pixel(&self, x: usize, y: usize) -> u8 {
        self.pixels[y * self.w + x]
    }

    fn set_pixel(&mut self, x: usize, y: usize, val: u8) {
        self.pixels[y * self.w + x] = val;
    }
}

/// Publish the just-processed (in-place binarized) frame as a 1-bit
/// half-resolution preview for the on-device aiming display.
fn publish_preview(slot: &Mutex<PreviewFrame>, w: usize, h: usize, frame: &[u8]) {
    let (tw, th) = (w / 2, h / 2);
    // Ignore lock poisoning: a panicked previous writer must not take
    // down the preview path for good.
    let mut guard = slot.lock().unwrap_or_else(|e| e.into_inner());
    let out = &mut *guard;
    out.bits.clear();
    out.bits.resize(tw * th / 8, 0);
    for ty in 0..th {
        for tx in 0..tw {
            let mut dark = 0;
            for dy in 0..2 {
                for dx in 0..2 {
                    // Binarized pixels: 0 = white; 1 = black; >=5 are
                    // rqrr's recolored (claimed) regions, also dark.
                    if frame[(ty * 2 + dy) * w + tx * 2 + dx] != 0 {
                        dark += 1;
                    }
                }
            }
            if dark < 2 {
                out.bits[ty * (tw / 8) + tx / 8] |= 1 << (tx % 8);
            }
        }
    }
    out.w = tw;
    out.h = th;
    out.seq += 1;
}

/// Capture frames forever, decoding QR codes into player scans; polls
/// only while `capture_enabled` is set and publishes the preview.
fn capture_task(
    tx: SyncSender<PlayerScan>,
    capture_enabled: Arc<AtomicBool>,
    preview: Arc<Mutex<PreviewFrame>>,
) -> ! {
    let mut first_frame = true;
    let mut frame_count = 0u32;
    let mut grids_since_heartbeat = 0u32;
    let mut qr_in_view = false;
    // rqrr's region cache, carried across frames so its ~4.6 KiB table
    // is allocated once.
    let mut region_cache: Option<rqrr::RegionCache> = None;
    loop {
        if !capture_enabled.load(Ordering::Relaxed) {
            std::thread::sleep(SCAN_PERIOD);
            continue;
        }

        // Safety: the frame buffer is owned by the driver until returned
        // via esp_camera_fb_return below.
        let fb_ptr = unsafe { esp_camera_fb_get() };
        let Some(fb) = (unsafe { fb_ptr.as_ref() }) else {
            log::warn!("camera: frame capture failed");
            std::thread::sleep(SCAN_PERIOD);
            continue;
        };

        let (w, h) = (fb.width, fb.height);
        if first_frame {
            first_frame = false;
            log::info!(
                "camera: capturing frames ({w}x{h} grayscale, {} ms scan period)",
                SCAN_PERIOD.as_millis()
            );
        }
        // Liveness heartbeat (~every 5 s): stack high-water mark + heap.
        frame_count += 1;
        if frame_count % 25 == 0 {
            // Safety: NULL = the calling task.
            let stack_free =
                unsafe { esp_idf_svc::sys::uxTaskGetStackHighWaterMark(core::ptr::null_mut()) };
            let (heap_free, heap_max) = crate::scores::internal_heap_stats();
            log::info!(
                "camera: alive, {frame_count} frames, {grids_since_heartbeat} grids seen, stack free {stack_free} B, heap {heap_free} B free / {heap_max} B max block"
            );
            grids_since_heartbeat = 0;
        }
        // Sample-frame dump for off-board debugging (see dump_frame).
        if DEBUG_FRAMEDUMP
            && (frame_count == 3
                || (FRAME_DUMP_PERIOD != 0 && frame_count % FRAME_DUMP_PERIOD == 0))
        {
            // Safety: buf holds len bytes and the frame is alive until
            // esp_camera_fb_return below.
            dump_frame(w, h, unsafe { core::slice::from_raw_parts(fb.buf, fb.len) });
        }
        debug_assert_eq!(fb.len, w * h); // 1 byte/pixel grayscale
                                         // Safety: the frame is exclusively ours until esp_camera_fb_return
                                         // (fb_count=1); in-place pixel mutation is harmless.
        let frame = unsafe { core::slice::from_raw_parts_mut(fb.buf, fb.len) };
        let cache = region_cache.take().unwrap_or_default();
        let mut image = rqrr::PreparedImage::prepare_with_region_cache(
            FrameView {
                w,
                h,
                pixels: frame,
            },
            cache,
        );
        let grids = image.detect_grids();
        // Aiming feedback: surface whether a QR-like pattern is in view
        // at all (grids found but undecodable = too far/blurry/clipped).
        if !grids.is_empty() && !qr_in_view {
            log::info!("camera: QR-like pattern in view, decoding...");
        }
        qr_in_view = !grids.is_empty();
        grids_since_heartbeat += grids.len() as u32;
        for grid in grids {
            let Ok((_meta, content)) = grid.decode() else {
                // Spotted but not decodable: too far, blurry, or glare.
                log::debug!("camera: QR-like pattern not decodable");
                continue;
            };
            match parse_scan_line(&content) {
                Some(scan) => {
                    log::info!(
                        "player scanned (camera): {} (uuid {})",
                        scan.name,
                        scan.uuid
                    );
                    // If the main loop is wedged we simply drop scans.
                    let _ = tx.try_send(scan);
                }
                None => log::debug!("camera: QR content is not a player scan: {content:?}"),
            }
            break; // one player per frame is plenty
        }
        // Take the cache's allocation back for the next frame before the
        // image (and with it the frame borrow) is dropped.
        region_cache = Some(image.into_region_cache());
        // Publish the (by now binarized, in place) aiming preview frame.
        publish_preview(&preview, w, h, frame);

        // Safety: hands the buffer back to the driver.
        unsafe { esp_camera_fb_return(fb_ptr) };
        std::thread::sleep(SCAN_PERIOD);
    }
}
