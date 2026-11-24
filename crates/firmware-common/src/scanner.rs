//! USB HID keyboard-wedge barcode scanner reader, behind the `usb`
//! feature.

#[cfg(esp32s3)]
pub use hid::Scanner;
#[cfg(all(not(esp32s3), feature = "qemu"))]
pub use mock::Scanner;
#[cfg(all(not(esp32s3), not(feature = "qemu")))]
pub use stub::Scanner;

/// ESP32-S3: real USB host implementation.
#[cfg(esp32s3)]
mod hid {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{channel, Receiver, Sender};
    use std::sync::Arc;
    use std::thread::JoinHandle;
    use std::time::Duration;

    use esp_idf_svc::sys::usbhid::*;
    use esp_idf_svc::sys::{esp, EspError};

    use crate::player::{parse_scan_line, PlayerIdSource, PlayerScan};

    /// Longest accepted scan line (uuid + space + name) before the line is
    /// discarded as garbage.
    const MAX_LINE_LEN: usize = 128;

    /// Events handed from the HID driver's callbacks to the client task
    /// (handles cross as `usize` to stay `Send`).
    enum HidEvent {
        /// A HID interface was found on a newly connected device.
        Connected(usize),
        /// Boot-protocol keyboard input report (always 8 bytes).
        Report(usize, [u8; 8]),
        /// The interface went away (disconnected or transfer error).
        Gone(usize),
    }

    /// USB HID keyboard-wedge scanner handle; the USB host daemon and HID
    /// client tasks live as long as this handle is not leaked.
    pub struct Scanner {
        rx: Receiver<PlayerScan>,
        enabled: Arc<AtomicBool>,
        _daemon: JoinHandle<()>,
        _client: JoinHandle<()>,
    }

    impl Scanner {
        /// Install the USB host library + HID class driver and start the
        /// daemon/client tasks (scanner on the native USB port, GPIO19/20).
        pub fn new() -> Result<Self, EspError> {
            let (tx, rx) = channel::<PlayerScan>();
            let (hid_tx, hid_rx) = channel::<HidEvent>();
            let enabled = Arc::new(AtomicBool::new(true));
            // The callbacks need a 'static sender; Scanner is never
            // dropped in practice (same lifetime contract as the tasks).
            let cb_arg =
                Box::leak(Box::new(hid_tx)) as *mut Sender<HidEvent> as *mut core::ffi::c_void;

            // All-zero usb_host_config_t is the right default: internal
            // PHY, root port powered, default FIFO sizing, peripheral 0.
            let usb_cfg: usb_host_config_t = unsafe { core::mem::zeroed() };
            esp!(unsafe { usb_host_install(&usb_cfg) })?;

            let mut hid_cfg: hid_host_driver_config_t = unsafe { core::mem::zeroed() };
            hid_cfg.create_background_task = true;
            hid_cfg.task_priority = 5;
            hid_cfg.stack_size = 4096;
            hid_cfg.core_id = i32::MAX as _; // tskNO_AFFINITY
            hid_cfg.callback = Some(driver_event_cb);
            hid_cfg.callback_arg = cb_arg;
            if let Err(e) = esp!(unsafe { hid_host_install(&hid_cfg) }) {
                unsafe { usb_host_uninstall() };
                return Err(e);
            }

            let daemon = std::thread::Builder::new()
                .name("scanner_usb".into()) // FreeRTOS task names cap at 16 chars
                .stack_size(4096)
                .spawn(move || usb_daemon_task())
                .expect("failed to spawn USB host daemon task");
            let task_enabled = Arc::clone(&enabled);
            let cb_arg_usize = cb_arg as usize;
            let client = std::thread::Builder::new()
                .name("scanner_hid".into())
                .stack_size(4096)
                .spawn(move || hid_client_task(hid_rx, cb_arg_usize, tx, task_enabled))
                .expect("failed to spawn HID client task");
            Ok(Self {
                rx,
                enabled,
                _daemon: daemon,
                _client: client,
            })
        }
    }

    impl PlayerIdSource for Scanner {
        fn take(&self) -> Option<PlayerScan> {
            self.rx.try_recv().ok()
        }

        fn set_enable(&self, enable: bool) {
            // Ignore scans while a run/countdown is active. Re-asserted
            // every main-loop pass, so this is an idempotent hint.
            self.enabled.store(enable, Ordering::Relaxed);
        }
    }

    /// USB Host Library daemon: pump library events forever (the library
    /// never creates tasks itself).
    fn usb_daemon_task() -> ! {
        loop {
            let mut flags: u32 = 0;
            // u32::MAX ticks: effectively portMAX_DELAY (which bindgen
            // cannot translate — the macro contains a cast).
            let res = unsafe { usb_host_lib_handle_events(u32::MAX as _, &mut flags) };
            if res != 0 {
                // Never expected (the library is never uninstalled); don't
                // busy-spin if it somehow happens.
                log::warn!("scanner: usb_host_lib_handle_events: err {res}");
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }

    /// Driver-level callback (HID background task context): a device with
    /// at least one HID interface was connected.
    unsafe extern "C" fn driver_event_cb(
        handle: hid_host_device_handle_t,
        event: hid_host_driver_event_t,
        arg: *mut core::ffi::c_void,
    ) {
        let tx = &*(arg as *const Sender<HidEvent>);
        if event == hid_host_driver_event_t_HID_HOST_DRIVER_EVENT_CONNECTED {
            let _ = tx.send(HidEvent::Connected(handle as usize));
        }
    }

    /// Interface-level callback (HID background task context).
    unsafe extern "C" fn iface_event_cb(
        handle: hid_host_device_handle_t,
        event: hid_host_interface_event_t,
        arg: *mut core::ffi::c_void,
    ) {
        let tx = &*(arg as *const Sender<HidEvent>);
        let h = handle as usize;
        if event == hid_host_interface_event_t_HID_HOST_INTERFACE_EVENT_INPUT_REPORT {
            let mut data = [0u8; 9];
            let mut len: usize = 0;
            let ok = esp!(unsafe {
                hid_host_device_get_raw_input_report_data(
                    handle,
                    data.as_mut_ptr(),
                    data.len(),
                    &mut len,
                )
            })
            .is_ok();
            if !ok {
                return;
            }
            // Boot protocol is a fixed 8-byte report; composite devices
            // may lead with a report-ID byte.
            let offset = match len {
                8 => 0,
                9 => 1,
                _ => return,
            };
            let mut report = [0u8; 8];
            report.copy_from_slice(&data[offset..offset + 8]);
            let _ = tx.send(HidEvent::Report(h, report));
        } else if event == hid_host_interface_event_t_HID_HOST_INTERFACE_EVENT_DISCONNECTED
            || event == hid_host_interface_event_t_HID_HOST_INTERFACE_EVENT_TRANSFER_ERROR
        {
            let _ = tx.send(HidEvent::Gone(h));
        }
    }

    /// Open + configure a connected interface if it is a keyboard.
    /// Returns the handle when the device was claimed.
    fn open_keyboard(
        handle: hid_host_device_handle_t,
        cb_arg: *mut core::ffi::c_void,
    ) -> Option<hid_host_device_handle_t> {
        let dev_cfg = hid_host_device_config_t {
            callback: Some(iface_event_cb),
            callback_arg: cb_arg,
        };
        if let Err(e) = esp!(unsafe { hid_host_device_open(handle, &dev_cfg) }) {
            log::warn!("scanner: HID open failed: {e}");
            return None;
        }
        let mut params: hid_host_dev_params_t = unsafe { core::mem::zeroed() };
        if esp!(unsafe { hid_host_device_get_params(handle, &mut params) }).is_err() {
            unsafe { hid_host_device_close(handle) };
            return None;
        }
        log::info!(
            "scanner: HID iface {}, sub-class {}, proto {}",
            params.iface_num,
            params.sub_class,
            params.proto
        );
        if params.proto != hid_protocol_t_HID_PROTOCOL_KEYBOARD as u8 {
            // Wedge scanners sometimes expose a second interface
            // (consumer control etc.); only the keyboard one types.
            log::debug!("scanner: not a keyboard interface, ignoring");
            unsafe { hid_host_device_close(handle) };
            return None;
        }
        if params.sub_class == hid_subclass_t_HID_SUBCLASS_BOOT_INTERFACE as u8 {
            // Boot protocol gives the fixed 8-byte report; best-effort
            // (the report path above also tolerates report-ID prefixes).
            let _ = esp!(unsafe {
                hid_class_request_set_protocol(
                    handle,
                    hid_report_protocol_t_HID_REPORT_PROTOCOL_BOOT,
                )
            });
            let _ = esp!(unsafe { hid_class_request_set_idle(handle, 0, 0) });
        }
        if let Err(e) = esp!(unsafe { hid_host_device_start(handle) }) {
            log::warn!("scanner: HID start failed: {e}");
            unsafe { hid_host_device_close(handle) };
            return None;
        }
        Some(handle)
    }

    /// Client task: own the connected keyboard, decode its reports into
    /// lines, and parse complete scans (a dead queue parks the task).
    fn hid_client_task(
        rx: Receiver<HidEvent>,
        cb_arg: usize,
        tx: Sender<PlayerScan>,
        enabled: Arc<AtomicBool>,
    ) -> ! {
        let cb_arg = cb_arg as *mut core::ffi::c_void;
        let mut kbd: Option<hid_host_device_handle_t> = None;
        let mut prev = [0u8; 8];
        let mut line = [0u8; MAX_LINE_LEN];
        let mut len = 0usize;
        loop {
            let ev = match rx.recv() {
                Ok(ev) => ev,
                Err(_) => loop {
                    std::thread::sleep(Duration::from_secs(3600));
                },
            };
            match ev {
                HidEvent::Connected(h) => {
                    if kbd.is_some() {
                        log::debug!("scanner: extra HID interface ignored");
                        continue;
                    }
                    kbd = open_keyboard(h as hid_host_device_handle_t, cb_arg);
                    if kbd.is_some() {
                        prev = [0; 8];
                        len = 0;
                        log::info!("scanner: keyboard-wedge device connected");
                    }
                }
                HidEvent::Report(h, report) => {
                    if kbd != Some(h as hid_host_device_handle_t) {
                        continue;
                    }
                    if enabled.load(Ordering::Relaxed) {
                        process_report(&report, &mut prev, &mut line, &mut len, &tx);
                    } else {
                        // Disabled: keep the pressed-key snapshot fresh so
                        // held keys don't re-fire when re-enabled.
                        prev = report;
                    }
                }
                HidEvent::Gone(h) => {
                    if kbd == Some(h as hid_host_device_handle_t) {
                        unsafe { hid_host_device_close(h as hid_host_device_handle_t) };
                        kbd = None;
                        log::info!("scanner: device disconnected");
                    }
                }
            }
        }
    }

    /// Diff one boot report against the previous one and feed newly
    /// pressed keys into the line buffer (bytes 2..8 = pressed usage IDs).
    fn process_report(
        report: &[u8; 8],
        prev: &mut [u8; 8],
        line: &mut [u8; MAX_LINE_LEN],
        len: &mut usize,
        tx: &Sender<PlayerScan>,
    ) {
        let keys = &report[2..8];
        // Phantom state on rollover overflow: ignore the report.
        if keys.iter().all(|&k| k == 1) {
            *prev = *report;
            return;
        }
        let shift = report[0] & 0x22 != 0; // left or right shift
        for &k in keys {
            if k == 0 || k == 1 || prev[2..8].contains(&k) {
                continue;
            }
            let Some(c) = key_to_ascii(k, shift) else {
                continue;
            };
            match c {
                '\n' => {
                    if *len > 0 {
                        emit(&line[..*len], tx);
                        *len = 0;
                    }
                }
                _ => {
                    if *len < MAX_LINE_LEN {
                        line[*len] = c as u8;
                        *len += 1;
                    } else {
                        // Overlong line: drop it entirely.
                        log::warn!("scanner: discarding overlong line");
                        *len = 0;
                    }
                }
            }
        }
        *prev = *report;
    }

    /// Map a HID keyboard usage ID (page 0x07) to ASCII, US layout.
    /// Enter keys map to '\n'; non-printable keys map to None.
    fn key_to_ascii(key: u8, shift: bool) -> Option<char> {
        let c = match key {
            // a-z / A-Z
            0x04..=0x1d => {
                let lower = (key - 0x04) + b'a';
                if shift {
                    lower.to_ascii_uppercase() as char
                } else {
                    lower as char
                }
            }
            // 1-9, shifted: !@#$%^&*(
            0x1e..=0x26 => {
                if shift {
                    b"!@#$%^&*("[(key - 0x1e) as usize] as char
                } else {
                    ((key - 0x1e) + b'1') as char
                }
            }
            0x27 => {
                if shift {
                    ')'
                } else {
                    '0'
                }
            }
            0x28 => '\n', // Enter
            0x2c => ' ',
            0x2d => {
                if shift {
                    '_'
                } else {
                    '-'
                }
            }
            0x2e => {
                if shift {
                    '+'
                } else {
                    '='
                }
            }
            0x2f => {
                if shift {
                    '{'
                } else {
                    '['
                }
            }
            0x30 => {
                if shift {
                    '}'
                } else {
                    ']'
                }
            }
            0x31 => {
                if shift {
                    '|'
                } else {
                    '\\'
                }
            }
            0x33 => {
                if shift {
                    ':'
                } else {
                    ';'
                }
            }
            0x34 => {
                if shift {
                    '"'
                } else {
                    '\''
                }
            }
            0x35 => {
                if shift {
                    '~'
                } else {
                    '`'
                }
            }
            0x36 => {
                if shift {
                    '<'
                } else {
                    ','
                }
            }
            0x37 => {
                if shift {
                    '>'
                } else {
                    '.'
                }
            }
            0x38 => {
                if shift {
                    '?'
                } else {
                    '/'
                }
            }
            // Keypad
            0x54 => '/',
            0x55 => '*',
            0x56 => '-',
            0x57 => '+',
            0x58 => '\n', // keypad Enter
            0x59..=0x61 => ((key - 0x59) + b'1') as char,
            0x62 => '0',
            0x63 => '.',
            _ => return None,
        };
        Some(c)
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
}

/// Plain esp32 (also the QEMU target): no USB host peripheral, so the
/// HID scanner can never work — a silent no-op.
#[cfg(all(not(esp32s3), not(feature = "qemu")))]
mod stub {
    use esp_idf_svc::sys::EspError;

    use crate::player::{PlayerIdSource, PlayerScan};

    /// No-op stand-in for the HID scanner on chips without a USB host
    /// peripheral: `take()` never yields a scan.
    pub struct Scanner;

    impl Scanner {
        pub fn new() -> Result<Self, EspError> {
            log::warn!("scanner: no USB host on this chip; player scanning disabled");
            Ok(Self)
        }
    }

    impl PlayerIdSource for Scanner {
        fn take(&self) -> Option<PlayerScan> {
            None
        }
    }
}

/// QEMU: no emulated USB host, so inject one canned scan the first time
/// the timer goes idle, keeping the player flow exercised in simulation.
#[cfg(all(not(esp32s3), feature = "qemu"))]
mod mock {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{channel, Receiver, Sender};

    use esp_idf_svc::sys::EspError;

    use crate::player::{PlayerIdSource, PlayerScan};

    /// Scripted stand-in for the HID scanner under emulation.
    pub struct Scanner {
        rx: Receiver<PlayerScan>,
        tx: Sender<PlayerScan>,
        injected: AtomicBool,
    }

    impl Scanner {
        pub fn new() -> Result<Self, EspError> {
            let (tx, rx) = channel();
            Ok(Self {
                rx,
                tx,
                injected: AtomicBool::new(false),
            })
        }
    }

    impl PlayerIdSource for Scanner {
        fn take(&self) -> Option<PlayerScan> {
            self.rx.try_recv().ok()
        }

        fn set_enable(&self, enable: bool) {
            if enable && !self.injected.swap(true, Ordering::Relaxed) {
                let scan = PlayerScan {
                    uuid: "01234567-89ab-cdef-0123-456789abcdef".into(),
                    name: "Sim Player".into(),
                };
                log::info!("player scanned (mock): {} (uuid {})", scan.name, scan.uuid);
                let _ = self.tx.send(scan);
            }
        }
    }
}
