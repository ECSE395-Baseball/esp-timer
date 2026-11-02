//! Onboard-camera QR reader (start node), behind the `camera` feature.
//!
//! Frames come from an image sensor (e.g. OV2640) driven by the vendored
//! `esp32-camera` ESP-IDF component (grayscale QVGA over SCCB + I2S/DMA);
//! QR codes are decoded from the frames with the pure-Rust `rqrr` crate.
//! Decoded text uses the shared [`player`] scan-line format
//! (`<uuid> <player name>`), so this is interchangeable with the USB
//! [`Scanner`]: both implement [`PlayerIdSource`].
//!
//! [`Scanner`]: crate::Scanner

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use esp_idf_svc::sys::camera::{
    camera_config_t, camera_fb_location_t_CAMERA_FB_IN_DRAM,
    camera_grab_mode_t_CAMERA_GRAB_WHEN_EMPTY, esp_camera_fb_get, esp_camera_fb_return,
    esp_camera_init, framesize_t_FRAMESIZE_QVGA, ledc_channel_t_LEDC_CHANNEL_0,
    ledc_timer_t_LEDC_TIMER_0, pixformat_t_PIXFORMAT_GRAYSCALE,
};
use esp_idf_svc::sys::{esp, EspError};

use crate::player::{parse_scan_line, PlayerIdSource, PlayerScan};

/// How often a frame is captured and scanned for QR codes. Frame capture
/// plus decode takes a few hundred ms on the ESP32
const SCAN_PERIOD: Duration = Duration::from_millis(200);

/// GPIO numbers for the image sensor, mirroring esp32-camera's
/// `camera_config_t` pin fields. Use -1 for unconnected pins (pwdn/reset).
#[derive(Debug, Clone, Copy)]
pub struct CameraPins {
    pub pwdn: i32,
    pub reset: i32,
    pub xclk: i32,
    pub sccb_sda: i32,
    pub sccb_scl: i32,
    pub d7: i32,
    pub d6: i32,
    pub d5: i32,
    pub d4: i32,
    pub d3: i32,
    pub d2: i32,
    pub d1: i32,
    pub d0: i32,
    pub vsync: i32,
    pub href: i32,
    pub pclk: i32,
}

/// Camera QR reader handle; the capture task lives as long as this handle
/// is not leaked.
pub struct Camera {
    rx: Receiver<PlayerScan>,
    capture_enabled: Arc<AtomicBool>,
    _task: JoinHandle<()>,
}

impl PlayerIdSource for Camera {
    fn take(&self) -> Option<PlayerScan> {
        self.rx.try_recv().ok()
    }

    fn set_enable(&self, enable: bool) {
        // Pause frame polling when not actively attempting to scan
        self.capture_enabled.store(!enable, Ordering::Relaxed);
    }
}

impl Camera {
    /// Configure the sensor and start the capture/decode task.
    pub fn new(pins: CameraPins) -> Result<Self, EspError> {
        init_sensor(&pins)?;
        let (tx, rx) = channel::<PlayerScan>();
        let capture_enabled = Arc::new(AtomicBool::new(true));
        let task_enabled = Arc::clone(&capture_enabled);
        let task = std::thread::Builder::new()
            .name("camera-qr".into()) // FreeRTOS task names cap at 16 chars
            .stack_size(32 * 1024) // QR detection needs headroom
            .spawn(move || capture_task(tx, task_enabled))
            .expect("failed to spawn camera task");
        Ok(Self {
            rx,
            capture_enabled,
            _task: task,
        })
    }
}

/// Configure and start the image sensor: grayscale QVGA (320x240, one
/// byte per pixel) with the frame buffer in internal RAM (~77 KiB).
fn init_sensor(pins: &CameraPins) -> Result<(), EspError> {
    // Zeroed base: camera_config_t is plain ints/enums
    let mut config: camera_config_t = unsafe { core::mem::zeroed() };
    config.pin_pwdn = pins.pwdn;
    config.pin_reset = pins.reset;
    config.pin_xclk = pins.xclk;
    config.pin_d7 = pins.d7;
    config.pin_d6 = pins.d6;
    config.pin_d5 = pins.d5;
    config.pin_d4 = pins.d4;
    config.pin_d3 = pins.d3;
    config.pin_d2 = pins.d2;
    config.pin_d1 = pins.d1;
    config.pin_d0 = pins.d0;
    config.pin_vsync = pins.vsync;
    config.pin_href = pins.href;
    config.pin_pclk = pins.pclk;
    // The SCCB pin fields are anonymous unions in C (with deprecated
    // aliases), hence the bindgen-generated indirection. (Union field
    // writes are safe; only reads are unsafe.)
    config.__bindgen_anon_1.pin_sccb_sda = pins.sccb_sda;
    config.__bindgen_anon_2.pin_sccb_scl = pins.sccb_scl;
    config.xclk_freq_hz = 20_000_000;
    config.ledc_timer = ledc_timer_t_LEDC_TIMER_0;
    config.ledc_channel = ledc_channel_t_LEDC_CHANNEL_0;
    config.pixel_format = pixformat_t_PIXFORMAT_GRAYSCALE;
    config.frame_size = framesize_t_FRAMESIZE_QVGA;
    config.jpeg_quality = 0; // unused for grayscale
    config.fb_count = 1;
    config.fb_location = camera_fb_location_t_CAMERA_FB_IN_DRAM;
    config.grab_mode = camera_grab_mode_t_CAMERA_GRAB_WHEN_EMPTY;
    config.sccb_i2c_port = -1; // dedicated bit-banged SCCB pins
    esp!(unsafe { esp_camera_init(&config) })
}

/// Capture frames forever, decoding QR codes into player scans. Polls the
/// sensor only while `capture_enabled`
fn capture_task(tx: Sender<PlayerScan>, capture_enabled: Arc<AtomicBool>) -> ! {
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
        debug_assert_eq!(fb.len, w * h); // 1 byte/pixel grayscale
        let mut image = rqrr::PreparedImage::prepare_from_greyscale(w, h, |x, y| {
            // Safety: rqrr only queries x < w, y < h, and buf holds w*h
            // bytes; the frame is alive for the whole call.
            unsafe { *fb.buf.add(y * w + x) }
        });
        for grid in image.detect_grids() {
            let Ok((_meta, content)) = grid.decode() else {
                continue; // not a readable QR code
            };
            match parse_scan_line(&content) {
                Some(scan) => {
                    log::info!(
                        "player scanned (camera): {} (uuid {})",
                        scan.name,
                        scan.uuid
                    );
                    // If the main loop is wedged we simply drop scans.
                    let _ = tx.send(scan);
                }
                None => log::debug!("camera: QR content is not a player scan: {content:?}"),
            }
            break; // one player per frame is plenty
        }

        // Safety: hands the buffer back to the driver.
        unsafe { esp_camera_fb_return(fb_ptr) };
        std::thread::sleep(SCAN_PERIOD);
    }
}
