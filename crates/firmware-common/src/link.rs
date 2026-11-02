//! The radio link a node talks on.
//!
//! Two backends are provided, selected by the `qemu` cargo feature:
//! * default: real ESP-NOW broadcasts over WiFi (hardware)
//! * `qemu`: raw broadcast Ethernet frames over the emulated OpenCores MAC
//!   (for Espressif's QEMU fork, which does not emulate WiFi/ESP-NOW)
//!
//! Both backends deliver [`RxFrame`]s timestamped on arrival and broadcast
//! protocol frames, so the timer logic on top is identical.

use std::sync::mpsc::{channel, Receiver, Sender};
#[cfg(feature = "qemu")]
use std::sync::Mutex;

use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::sys::EspError;

use crate::time::now_us;

#[cfg(not(feature = "qemu"))]
mod imp {
    use super::*;
    use esp_idf_svc::espnow::{EspNow, PeerInfo, BROADCAST};
    use esp_idf_svc::sys::wifi_interface_t_WIFI_IF_STA;
    use esp_idf_svc::wifi::{ClientConfiguration, Configuration, EspWifi, WifiDeviceId};

    pub struct LinkBackend {
        _wifi: EspWifi<'static>, // keep alive
        espnow: EspNow<'static>,
    }

    impl LinkBackend {
        pub fn init(
            modem: esp_idf_svc::hal::modem::Modem<'static>,
            sysloop: EspSystemEventLoop,
            nvs: EspDefaultNvsPartition,
        ) -> Result<Self, EspError> {
            let mut wifi = EspWifi::new(modem, sysloop, Some(nvs))?;
            wifi.set_configuration(&Configuration::Client(ClientConfiguration::default()))?;
            wifi.start()?;

            let espnow = EspNow::take()?;
            espnow.add_peer(PeerInfo {
                peer_addr: BROADCAST,
                channel: 0, // current channel (both nodes sit on channel 1)
                ifidx: wifi_interface_t_WIFI_IF_STA,
                encrypt: false,
                ..Default::default()
            })?;

            let mac = wifi.get_mac(WifiDeviceId::Sta)?;
            log::info!("station MAC: {:02X?}", mac);

            Ok(Self {
                _wifi: wifi,
                espnow,
            })
        }

        pub fn register_rx(&mut self, tx: Sender<RxFrame>) -> Result<(), EspError> {
            self.espnow.register_recv_cb(move |_info, data| {
                let mut frame = RxFrame {
                    data: [0; ESPNOW_MAX_LEN],
                    len: data.len().min(ESPNOW_MAX_LEN),
                    t_rx_us: now_us(),
                };
                frame.data[..frame.len].copy_from_slice(&data[..frame.len]);
                // If the main loop is wedged we simply drop frames.
                let _ = tx.send(frame);
            })?;
            Ok(())
        }

        pub fn send_raw(&self, payload: &[u8]) {
            if let Err(e) = self.espnow.send(BROADCAST, payload) {
                log::warn!("esp-now send failed: {e}");
            }
        }
    }
}

#[cfg(feature = "qemu")]
mod imp {
    use super::*;
    use esp_idf_svc::eth::{EthDriver, OpenEth};
    use esp_idf_svc::handle::RawHandle;
    use esp_idf_svc::sys::{esp, esp_eth_io_cmd_t_ETH_CMD_G_MAC_ADDR, esp_eth_ioctl};

    /// EtherType carrying timer protocol frames (IEEE 802.1 local-experimental).
    const TIMER_ETHERTYPE: u16 = 0x88B5;
    const ETH_HEADER_LEN: usize = 14;
    const BROADCAST_ADDR: [u8; 6] = [0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF];

    pub struct LinkBackend {
        // esp_eth_transmit takes &mut self; the underlying driver is
        // internally synchronized, so a Mutex around it is all we need.
        eth: Mutex<EthDriver<'static, OpenEth>>,
        src_mac: [u8; 6],
    }

    impl LinkBackend {
        pub fn init(
            mac: esp_idf_svc::hal::mac::MAC<'static>,
            sysloop: EspSystemEventLoop,
            _nvs: EspDefaultNvsPartition,
        ) -> Result<Self, EspError> {
            let eth = EthDriver::new_openeth(mac, sysloop)?;

            // Read back the MAC assigned via the QEMU command line.
            let mut src_mac = [0u8; 6];
            esp!(unsafe {
                esp_eth_ioctl(
                    eth.handle(),
                    esp_eth_io_cmd_t_ETH_CMD_G_MAC_ADDR,
                    src_mac.as_mut_ptr() as *mut _,
                )
            })?;
            log::info!("ethernet MAC: {:02X?}", src_mac);

            Ok(Self {
                eth: Mutex::new(eth),
                src_mac,
            })
        }

        pub fn register_rx(&mut self, tx: Sender<RxFrame>) -> Result<(), EspError> {
            self.eth
                .get_mut()
                .expect("not shared yet")
                .set_rx_callback(move |eth_frame| {
                    let raw: &[u8] = &eth_frame;
                    if raw.len() < ETH_HEADER_LEN {
                        return;
                    }
                    if raw[0..6] != BROADCAST_ADDR
                        || u16::from_be_bytes([raw[12], raw[13]]) != TIMER_ETHERTYPE
                    {
                        return; // not ours
                    }
                    let data = &raw[ETH_HEADER_LEN..];
                    let mut frame = RxFrame {
                        data: [0; ESPNOW_MAX_LEN],
                        len: data.len().min(ESPNOW_MAX_LEN),
                        t_rx_us: now_us(),
                    };
                    frame.data[..frame.len].copy_from_slice(&data[..frame.len]);
                    let _ = tx.send(frame);
                })?;
            self.eth.get_mut().expect("not shared yet").start()?;
            Ok(())
        }

        pub fn send_raw(&self, payload: &[u8]) {
            let mut frame = [0u8; ETH_HEADER_LEN + ESPNOW_MAX_LEN];
            frame[0..6].copy_from_slice(&BROADCAST_ADDR);
            frame[6..12].copy_from_slice(&self.src_mac);
            frame[12..14].copy_from_slice(&TIMER_ETHERTYPE.to_be_bytes());
            frame[ETH_HEADER_LEN..][..payload.len()].copy_from_slice(payload);
            let frame_len = ETH_HEADER_LEN + payload.len();
            if let Err(e) = self
                .eth
                .lock()
                .expect("eth poisoned")
                .send(&frame[..frame_len])
            {
                log::warn!("eth send failed: {e}");
            }
        }
    }
}

/// Link payload size limit (ESP-NOW payload limit).
pub const ESPNOW_MAX_LEN: usize = 250;

/// One received frame, timestamped on arrival with the local clock.
pub struct RxFrame {
    pub data: [u8; ESPNOW_MAX_LEN],
    pub len: usize,
    /// Local receive timestamp (us), captured in the receive callback so
    /// the sync math is not skewed by main-loop scheduling.
    pub t_rx_us: u64,
}

impl RxFrame {
    pub fn payload(&self) -> &[u8] {
        &self.data[..self.len]
    }
}

/// The link (radio) the node talks on.
pub struct Link {
    backend: imp::LinkBackend,
}

impl Link {
    /// Broadcast a protocol message, logging (not propagating) send errors.
    pub fn broadcast(&self, msg: &timer_protocol::Message) {
        let mut buf = [0u8; timer_protocol::MAX_FRAME_LEN];
        match timer_protocol::encode(msg, &mut buf) {
            Ok(len) => self.backend.send_raw(&buf[..len]),
            Err(e) => log::warn!("message encode failed: {e:?}"),
        }
    }
}

/// Set up the link and a channel of timestamped receive frames.
///
/// Consumes only the radio peripheral
#[cfg(not(feature = "qemu"))]
pub fn init_link(
    modem: esp_idf_svc::hal::modem::Modem<'static>,
    sysloop: EspSystemEventLoop,
    nvs: EspDefaultNvsPartition,
) -> Result<(Link, Receiver<RxFrame>), EspError> {
    finish_init(imp::LinkBackend::init(modem, sysloop, nvs)?)
}

/// Set up the link and a channel of timestamped receive frames (QEMU)
#[cfg(feature = "qemu")]
pub fn init_link(
    mac: esp_idf_svc::hal::mac::MAC<'static>,
    sysloop: EspSystemEventLoop,
    nvs: EspDefaultNvsPartition,
) -> Result<(Link, Receiver<RxFrame>), EspError> {
    finish_init(imp::LinkBackend::init(mac, sysloop, nvs)?)
}

fn finish_init(mut backend: imp::LinkBackend) -> Result<(Link, Receiver<RxFrame>), EspError> {
    let (tx, rx) = channel::<RxFrame>();
    backend.register_rx(tx)?;
    Ok((Link { backend }, rx))
}
