//! A loopback that also pushes cyclic reports (`--loopback-cyclic-ms`).
//!
//! Real devices report their state and meters unprompted, many times a second; the plain
//! emulating loopback only answers commands, so clients and the web UI would otherwise see no
//! cyclic traffic without hardware. Every `interval` this emits one report per cyclic layout the
//! device's registry declares, sized to the layout, with `seq` set to the contents' CRC32 as a
//! device sets it. The contents are a byte pattern that shifts on each report so values visibly
//! move; they are test data, not device state.

use std::time::{Duration, Instant};

use gazelle_audio_protocol::crc32::crc32;
use gazelle_audio_protocol::wire::{Header, WireError};
use gazelle_audio_transport::{Device, LoopbackDevice, RawPacket, Report};

pub struct CyclicLoopback {
    inner: LoopbackDevice,
    /// `(report id, contents length)` for every declared cyclic layout.
    reports: Vec<(u32, usize)>,
    interval: Duration,
    last: Instant,
    tick: u8,
}

impl CyclicLoopback {
    pub fn new(inner: LoopbackDevice, reports: Vec<(u32, usize)>, interval: Duration) -> Self {
        CyclicLoopback { inner, reports, interval, last: Instant::now(), tick: 0 }
    }
}

impl Device for CyclicLoopback {
    fn send(&mut self, report: &Report) -> Result<bool, WireError> {
        self.inner.send(report)
    }

    fn on_received_data(&mut self, packet: RawPacket) {
        self.inner.on_received_data(packet);
    }

    fn max_packet_size(&self) -> usize {
        self.inner.max_packet_size()
    }

    fn vid(&self) -> u16 {
        self.inner.vid()
    }

    fn pid(&self) -> u16 {
        self.inner.pid()
    }

    fn poll_reports(&mut self) -> Vec<Report> {
        let mut reports = self.inner.poll_reports();
        if self.last.elapsed() >= self.interval {
            self.last = Instant::now();
            self.tick = self.tick.wrapping_add(1);
            for &(report_id, len) in &self.reports {
                let contents: Vec<u8> = (0..len).map(|i| (i as u8).wrapping_add(self.tick)).collect();
                reports.push(Report { header: Header::new(report_id, crc32(&contents), 0, 0), contents });
            }
        }
        reports
    }
}
