//! Wire protocol shared by the start node and the end node of the two-part
//! ESP-NOW timer.
//!
//! All messages are exchanged as ESP-NOW broadcasts, so no MAC provisioning
//! is needed. Each frame on the air is:
//!
//! ```text
//! [MAGIC: u8][postcard-encoded Message]
//! ```
//!
//! Clock model: every node has a free-running microsecond counter
//! (`esp_timer_get_time()`, time since boot). The two clocks are related by
//!
//! ```text
//! end_clock = start_clock + offset
//! ```
//!
//! where `offset` is estimated by the start node with the usual
//! NTP-style ping/pong exchange and shared with the end node via
//! [`Message::SyncInfo`]. To convert a timestamp from the end node's clock
//! domain into the start node's clock domain: `t_start_domain = t - offset`.

#![no_std]

use serde::{Deserialize, Serialize};

/// First byte of every frame; guards against unrelated ESP-NOW traffic.
pub const MAGIC: u8 = 0x51;

/// ESP-NOW payload limit is 250 bytes
pub const MAX_FRAME_LEN: usize = 64;

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Message {
    /// Start -> end: clock-sync request.
    /// `t_send_us` is on the start node's clock.
    Ping { seq: u32, t_send_us: u64 },
    /// End -> start: reply to a [`Message::Ping`].
    /// `t_recv_us`/`t_send_us` are on the end node's clock.
    Pong {
        seq: u32,
        t_recv_us: u64,
        t_send_us: u64,
    },
    /// Start -> end: best current clock-sync estimate.
    /// `offset_us` is `end_clock - start_clock`.
    SyncInfo { offset_us: i64, rtt_us: u64 },
    /// Start -> end: the timer was started; `t_start_us` is on the start
    /// node's clock.
    StartEvent { t_start_us: u64 },
    /// End -> start: the timer was stopped; `t_end_us` is on the end
    /// node's clock.
    EndEvent { t_end_us: u64 },
}

/// Serialize a message into `buf`, returning the used length.
pub fn encode(msg: &Message, buf: &mut [u8]) -> postcard::Result<usize> {
    if buf.is_empty() {
        return Err(postcard::Error::SerializeBufferFull);
    }
    buf[0] = MAGIC;
    let used = postcard::to_slice(msg, &mut buf[1..])?;
    Ok(used.len() + 1)
}

/// Deserialize a frame previously written by [`encode`].
pub fn decode(frame: &[u8]) -> postcard::Result<Message> {
    match frame.split_first() {
        Some((&magic, rest)) if magic == MAGIC => postcard::from_bytes(rest),
        _ => Err(postcard::Error::DeserializeBadVarint),
    }
}

/// Compute the clock offset and round-trip time from one ping/pong exchange.
///
/// Timestamps follow the NTP convention:
/// * `t1`: ping sent (start clock)
/// * `t2`: ping received (end clock)
/// * `t3`: pong sent (end clock)
/// * `t4`: pong received (start clock)
///
/// Returns `(offset_us, rtt_us)` with `offset_us = end_clock - start_clock`.
pub fn compute_offset_rtt(t1: u64, t2: u64, t3: u64, t4: u64) -> (i64, u64) {
    let offset = ((t2 as i64 - t1 as i64) + (t3 as i64 - t4 as i64)) / 2;
    let rtt = (t4 - t1).saturating_sub(t3.saturating_sub(t2));
    (offset, rtt)
}

/// Convert a timestamp from the end node's clock domain into the start
/// node's clock domain.
pub fn end_to_start_domain(t_end_domain: u64, offset_us: i64) -> i64 {
    t_end_domain as i64 - offset_us
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offset_rtt_equal_delays() {
        // End clock is 100us ahead, one-way delay 10us, 5us processing.
        let (offset, rtt) = compute_offset_rtt(1000, 1110, 1115, 1025);
        assert_eq!(offset, 100);
        assert_eq!(rtt, 20);
    }

    #[test]
    fn offset_rtt_negative_offset() {
        // End clock is 100us *behind* the start clock.
        let (offset, rtt) = compute_offset_rtt(1000, 910, 915, 1025);
        assert_eq!(offset, -100);
        assert_eq!(rtt, 20);
    }

    #[test]
    fn roundtrip() {
        let mut buf = [0u8; MAX_FRAME_LEN];
        let msg = Message::Pong {
            seq: 42,
            t_recv_us: 123_456,
            t_send_us: 123_999,
        };
        let len = encode(&msg, &mut buf).unwrap();
        assert_eq!(decode(&buf[..len]).unwrap(), msg);
    }

    #[test]
    fn decode_rejects_bad_magic() {
        let mut buf = [0u8; MAX_FRAME_LEN];
        let len = encode(&Message::StartEvent { t_start_us: 7 }, &mut buf).unwrap();
        buf[0] = 0x00;
        assert!(decode(&buf[..len]).is_err());
    }
}
