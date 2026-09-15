//! Transport abstraction and a loopback device for offline validation.
//!
//! The real Antelope devices expose a tiny, fixed interface:
//!
//! * **send** a padded, framed byte string to the device (bulk/HID output report).
//! * **receive** a stream of raw packets (bulk/HID input reports) that the
//!   [`Device`] turns into reassembled reports via [`Device::on_received_data`].
//!
//! This module defines that interface as the [`Device`] trait so the transport
//! layer (libusb, Windows HID, Thunderbolt, macOS IOKit) is a thin adapter on top
//! of it. The [`LoopbackDevice`] implements the same trait without any hardware:
//! it echoes received packets back through a channel so the framing, reassembly,
//! and request/response correlation can be exercised end-to-end before real
//! hardware is attached (see the risk caveat in `.agent/STATUS.md`).

pub mod correlation;
pub mod framing;

use crate::framing::{parse_receive_segment, parse_send_segment, split_in_segments, Reassembly};
use gazelle_audio_protocol::wire::{Header, WireError, HEADER_SIZE};
use std::collections::VecDeque;
use std::sync::mpsc;

/// A packet received from the device, before reassembly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawPacket {
    /// The raw bytes as read off the wire (already un-padded by the transport).
    pub bytes: Vec<u8>,
}

/// A fully reassembled report ready for the protocol layer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    /// The parsed 16-byte header.
    pub header: Header,
    /// The report contents (everything after the header).
    pub contents: Vec<u8>,
}

impl Report {
    /// The report/command id (`cmd` header word).
    pub fn cmd(&self) -> u32 {
        self.header.cmd
    }

    /// The payload length (requests) or CRC32 (cyclic reports), from `seq`.
    pub fn seq(&self) -> u32 {
        self.header.seq
    }

    /// The `ext2` selector.
    pub fn ext2(&self) -> u32 {
        self.header.ext2
    }

    /// The `ext3` selector.
    pub fn ext3(&self) -> u32 {
        self.header.ext3
    }

    /// The report id as a hex string, e.g. `"0x70"`.
    pub fn cmd_hex(&self) -> String {
        format!("0x{:X}", self.header.cmd)
    }
}

/// A byte-oriented device transport.
///
/// Implementations read raw packets from the physical device and push them in via
/// [`Device::on_received_data`]; the trait reassembles segments and hands complete
/// reports to the caller through the receive channel.
pub trait Device {
    /// Send a fully-formed report (header + contents) to the device, applying
    /// per-packet padding and segmentation as the physical transport requires.
    ///
    /// Returns `Ok(true)` on a successful write, `Ok(false)` on a transport
    /// failure, or `Err` on a framing error.
    fn send(&mut self, report: &Report) -> Result<bool, WireError>;

    /// Feed a raw packet read from the device into the reassembler.
    ///
    /// Non-segmented reports are forwarded immediately; segmented reports are
    /// buffered until the full message arrives.
    fn on_received_data(&mut self, packet: RawPacket);

    /// The maximum packet size (output report length) of the device, in bytes.
    /// Used to bound segmentation.
    fn max_packet_size(&self) -> usize;

    /// The device's vendor id.
    fn vid(&self) -> u16;

    /// The device's product id.
    fn pid(&self) -> u16;

    /// Drain any reports that have been reassembled and are waiting for the caller.
    ///
    /// Reports arrive asynchronously: a real adapter's reader thread pushes raw packets in
    /// via [`Device::on_received_data`], and complete reports queue up here. Callers poll
    /// this to collect them, whether they are responses awaiting correlation or
    /// device-initiated cyclic reports.
    ///
    /// Returns an empty vector when nothing is pending; it never blocks.
    fn poll_reports(&mut self) -> Vec<Report>;
}

/// A [`Device`] that never touches hardware.
///
/// Packets pushed via [`Device::on_received_data`] are reassembled and delivered
/// to the caller's receive channel, exactly as a real device's reader thread would
/// deliver them. This is the offline validation path: drive it with captured or
/// synthetic traffic and assert on the [`Report`]s that come back.
pub struct LoopbackDevice {
    /// When true, a completed inbound report is answered the way a device answers:
    /// `cmd + 1`, same `ext2`. When false the report is echoed verbatim, which is what the
    /// framing tests want.
    emulate_responses: bool,
    vid: u16,
    pid: u16,
    max_packet_size: usize,
    tx: mpsc::Sender<Report>,
    reasm: Reassembly,
    /// Packets queued for the caller, drained by [`LoopbackDevice::poll`].
    rx: mpsc::Receiver<Report>,
    /// Reports moved off the channel by `pending`, still owed to the caller.
    buffered: VecDeque<Report>,
}

impl LoopbackDevice {
    /// Create a loopback device with its own receive channel.
    ///
    /// `max_packet_size` bounds how the device would segment outgoing reports;
    /// the loopback does not actually split writes, but the value is recorded so
    /// tests can assert on it.
    pub fn new(vid: u16, pid: u16, max_packet_size: usize) -> Self {
        let (tx, rx) = mpsc::channel();
        LoopbackDevice {
            emulate_responses: false,
            vid,
            pid,
            max_packet_size,
            tx,
            reasm: Reassembly::default(),
            rx,
            buffered: VecDeque::new(),
        }
    }

    /// A loopback that answers like a real device: `cmd + 1` with the request's `ext2`.
    ///
    /// Plain `new` echoes the request verbatim, which exercises framing but produces a
    /// report no correlator should accept — the device's own rule
    /// (`_sanitize_response`) requires `cmd == report_id + 1`. Use this when the
    /// request/response round trip is what is under test.
    pub fn emulating(vid: u16, pid: u16, max_packet_size: usize) -> Self {
        let mut d = LoopbackDevice::new(vid, pid, max_packet_size);
        d.emulate_responses = true;
        d
    }

    /// Drain any reassembled reports currently queued, including any that a previous
    /// [`LoopbackDevice::pending`] call moved into the buffer.
    pub fn poll(&mut self) -> Vec<Report> {
        self.absorb();
        self.buffered.drain(..).collect()
    }

    /// Drain queued reports into an internal buffer and return how many are waiting.
    ///
    /// Takes `&mut self` deliberately: counting requires draining the channel, so the
    /// reports are moved into `buffered` and handed out by the next [`LoopbackDevice::poll`].
    /// The previous version drained and *discarded* them behind a `&self` signature, so the
    /// natural `if dev.pending() > 0 { dev.poll() }` silently lost every report.
    pub fn pending(&mut self) -> usize {
        self.absorb();
        self.buffered.len()
    }

    /// Move anything the channel is holding into `buffered`.
    fn absorb(&mut self) {
        while let Ok(r) = self.rx.try_recv() {
            self.buffered.push_back(r);
        }
    }
}

impl Device for LoopbackDevice {
    fn send(&mut self, report: &Report) -> Result<bool, WireError> {
        // A loopback "writes" nothing, but we validate the report is well-formed
        // (header present, contents non-empty) so bad reports surface here.
        if report.contents.is_empty() {
            return Err(WireError::TruncatedPayload);
        }
        Ok(true)
    }

    fn on_received_data(&mut self, packet: RawPacket) {
        // Skip all-zero packets (idle USB/HID reports).
        if packet.bytes.is_empty() || packet.bytes.iter().all(|&b| b == 0) {
            return;
        }
        // The device reassembles host->device segments (8052). Non-segmented
        // reports (any other cmd) are forwarded as complete reports.
        match parse_send_segment(&packet.bytes) {
            Ok(seg) => {
                if let Ok(Some(msg)) = self.reasm.push(seg) {
                    // A message shorter than the header is malformed: `total_len` is
                    // device-controlled, so a value below HEADER_SIZE completes
                    // reassembly with a short buffer. Slicing it unconditionally
                    // panicked on the receive path.
                    let Ok(header) = Header::from_bytes(&msg) else {
                        return;
                    };
                    let header = if self.emulate_responses {
                        // Answer as the device does: cmd + 1, ext2 unchanged.
                        Header::new(
                            header.cmd.wrapping_add(1),
                            header.seq,
                            header.ext2,
                            header.ext3,
                        )
                    } else {
                        header
                    };
                    let _ = self.tx.send(Report {
                        header,
                        contents: msg[HEADER_SIZE..].to_vec(),
                    });
                }
            }
            // Not a segment: forward the raw packet as a complete report.
            Err(WireError::FieldOverflow) => {
                if let Ok(header) = Header::from_bytes(&packet.bytes) {
                    let _ = self.tx.send(Report {
                        header,
                        contents: packet.bytes[HEADER_SIZE..].to_vec(),
                    });
                }
            }
            Err(_) => {}
        }
    }

    fn max_packet_size(&self) -> usize {
        self.max_packet_size
    }

    fn vid(&self) -> u16 {
        self.vid
    }

    fn pid(&self) -> u16 {
        self.pid
    }

    fn poll_reports(&mut self) -> Vec<Report> {
        LoopbackDevice::poll(self)
    }
}

/// The host's side of the receive path: raw packets read from a real device become reports.
///
/// The device sends messages longer than a packet as 8053 segments (see [`framing`]) and every
/// other report whole, zero-padded to the packet size, so a whole report's contents keep that
/// padding. Idle all-zero packets and malformed segments yield nothing, and a bad segment resets
/// the reassembler so the next message still arrives. [`LoopbackDevice`] plays the device and
/// reassembles the host's 8052 segments instead.
#[derive(Debug, Default)]
pub struct HostReceiver {
    reasm: Reassembly,
}

impl HostReceiver {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds one packet read from the device; returns a report when this packet completes one.
    pub fn push(&mut self, packet: &[u8]) -> Option<Report> {
        if packet.len() < HEADER_SIZE || packet.iter().all(|&b| b == 0) {
            return None;
        }
        match parse_receive_segment(packet) {
            Ok(seg) => {
                let msg = match self.reasm.push(seg) {
                    Ok(Some(msg)) => msg,
                    Ok(None) | Err(_) => return None,
                };
                // `total_len` is device-controlled: a message shorter than a header is dropped.
                let header = Header::from_bytes(&msg).ok()?;
                Some(Report {
                    header,
                    contents: msg[HEADER_SIZE..].to_vec(),
                })
            }
            // Not a segment: a whole report.
            Err(WireError::FieldOverflow) => {
                let header = Header::from_bytes(packet).ok()?;
                Some(Report {
                    header,
                    contents: packet[HEADER_SIZE..].to_vec(),
                })
            }
            Err(_) => {
                self.reasm = Reassembly::Idle;
                None
            }
        }
    }
}

/// Build a report buffer (16-byte header + contents) from a header and contents.
pub fn build_report(cmd: u32, seq: u32, ext2: u32, ext3: u32, contents: &[u8]) -> Vec<u8> {
    let header = Header::new(cmd, seq, ext2, ext3);
    let mut buf = header.to_bytes().to_vec();
    buf.extend_from_slice(contents);
    buf
}

/// Split a report into the wire segments a real device would transmit, padding
/// each to `max_packet_size`. Returns the raw segment byte buffers.
pub fn segment_report(
    cmd: u32,
    seq: u32,
    ext2: u32,
    ext3: u32,
    contents: &[u8],
    max_packet_size: usize,
) -> Vec<Vec<u8>> {
    let report = build_report(cmd, seq, ext2, ext3, contents);
    split_in_segments(&report, max_packet_size)
        .into_iter()
        .map(|seg| {
            let chunk_len = seg.data.len();
            let header = Header::new(
                crate::framing::SEGMENT_SEND_ID,
                chunk_len as u32 + HEADER_SIZE as u32,
                seg.total_len as u32,
                seg.offset as u32,
            );
            let mut buf = header.to_bytes().to_vec();
            buf.extend_from_slice(&seg.data);
            buf
        })
        .collect()
}

/// Reassemble a list of raw segment byte buffers into the original report buffer.
pub fn reassemble_segments(segments: &[Vec<u8>]) -> Result<Vec<u8>, WireError> {
    let mut reasm = Reassembly::default();
    let mut out: VecDeque<Report> = VecDeque::new();
    for seg_bytes in segments {
        let seg = parse_send_segment(seg_bytes)?;
        if let Some(msg) = reasm.push(seg)? {
            let header = Header::from_bytes(&msg)?;
            out.push_back(Report {
                header,
                contents: msg[HEADER_SIZE..].to_vec(),
            });
        }
    }
    out.pop_front()
        .map(|r| {
            let mut buf = r.header.to_bytes().to_vec();
            buf.extend_from_slice(&r.contents);
            buf
        })
        .ok_or(WireError::TruncatedPayload)
}
