//! A loopback that also pushes cyclic reports (`--loopback-cyclic-ms`).
//!
//! Real devices report their state and meters unprompted, many times a second; the plain
//! emulating loopback only answers commands, so clients and the web UI would otherwise see no
//! cyclic traffic without hardware. Every `interval` this emits one report per cyclic layout the
//! device's registry declares, sized to the layout, with `seq` set to the contents' CRC32 as a
//! device sets it. Every byte cycles through 6..=53 and shifts on each report, so values visibly
//! move and meters (whose bytes are dB below full scale) swing without ever clipping; this is
//! test data, not device state.

use std::time::{Duration, Instant};

use gazelle_audio_protocol::crc32::crc32;
use gazelle_audio_protocol::wire::{Header, WireError};
use gazelle_audio_transport::{Device, LoopbackDevice, RawPacket, Report};

use super::read_loopback::LOOPBACK_CHAIN;

/// Chains each model meters in its effect-meter report: the Quadro's six user chains (its
/// AFX2DAW chains hold nothing here), the Studio+'s sixteen.
const QUADRO_CHAINS: usize = 6;
const STUDIO_CHAINS: usize = 16;
/// Slots a chain has, which is how many the Studio+ reports whether or not they are loaded.
const CHAIN_SLOTS: usize = 8;
/// Mic emulation meters, one per Quadro preamp, which follow the effect meters in its 0x83.
const QUADRO_MIC_EMU_METERS: usize = 4;
/// The meter byte for silence: dB below full scale, as the devices report it.
const SILENCE: u8 = 96;

/// What one loopback report's contents look like.
#[derive(Clone, Copy)]
pub enum Shape {
    /// Bytes cycling through 6..=53, shifting each tick, so every field visibly moves.
    Sweep(usize),
    /// The Quadro's effect meters: two bytes (peak, gain reduction) per loaded effect, chain by
    /// chain, then the mic emulation meters. Its length is the loaded effects', not the declared
    /// 304 (`reference/devices.md`, "Effects (AFX) and reverb").
    QuadroEffectMeters,
    /// The Studio+'s effect meters: 16 chains of 8 slot peaks, then the same of gain reductions.
    /// Slots with no effect meter silence.
    StudioEffectMeters,
}

impl Shape {
    /// The contents of one report at `tick`.
    fn contents(self, tick: u8) -> Vec<u8> {
        // A peak that swings with the tick without ever reaching full scale, and a gain reduction
        // that follows it: louder signal, more reduction.
        let peak = |n: usize| ((n * 7 + usize::from(tick)) % 48 + 6) as u8;
        match self {
            Shape::Sweep(len) => (0..len).map(peak).collect(),
            Shape::QuadroEffectMeters => {
                let loaded = QUADRO_CHAINS * LOOPBACK_CHAIN.len();
                let mut bytes: Vec<u8> = (0..loaded).flat_map(|n| [peak(n), peak(n) / 4]).collect();
                bytes.extend(std::iter::repeat_n(SILENCE, QUADRO_MIC_EMU_METERS));
                bytes
            }
            Shape::StudioEffectMeters => {
                let slots = STUDIO_CHAINS * CHAIN_SLOTS;
                let loaded = |n: usize| n % CHAIN_SLOTS < LOOPBACK_CHAIN.len();
                // An empty slot meters silence and reduces nothing.
                let peaks = (0..slots).map(|n| if loaded(n) { peak(n) } else { SILENCE });
                let reductions = (0..slots).map(|n| if loaded(n) { peak(n) / 4 } else { 0 });
                peaks.chain(reductions).collect()
            }
        }
    }
}

pub struct CyclicLoopback {
    inner: LoopbackDevice,
    /// `(report id, contents)` for every declared cyclic layout.
    reports: Vec<(u32, Shape)>,
    interval: Duration,
    last: Instant,
    tick: u8,
}

impl CyclicLoopback {
    pub fn new(inner: LoopbackDevice, reports: Vec<(u32, Shape)>, interval: Duration) -> Self {
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
            for &(report_id, shape) in &self.reports {
                let contents = shape.contents(self.tick);
                reports.push(Report { header: Header::new(report_id, crc32(&contents), 0, 0), contents });
            }
        }
        reports
    }
}
