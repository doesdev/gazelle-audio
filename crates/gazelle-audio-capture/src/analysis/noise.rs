//! Idle noise model (spec §8 step 3): how each byte of each channel behaves while the operator
//! does nothing.

use std::collections::HashMap;

use serde::Serialize;

use super::channel::{ChannelKey, Channels};
use super::segment::Segment;
use crate::session::plan::StepKind;

/// Fewest idle messages a channel needs before a byte is classed as a counter or derived.
pub const MIN_MESSAGES: usize = 4;
/// Share of consecutive idle message pairs that must step by the same amount for a counter.
pub const COUNTER_SHARE: f64 = 0.9;
/// Share of idle messages in which a byte must equal a checksum of the others to be derived.
pub const DERIVED_SHARE: f64 = 0.95;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Checksum {
    /// Sum of the other bytes modulo 256.
    Sum8,
    /// XOR of the other bytes.
    Xor8,
}

impl Checksum {
    pub fn of(self, bytes: &[u8], skip: usize) -> u8 {
        bytes.iter().enumerate().filter(|(i, _)| *i != skip).fold(0u8, |acc, (_, b)| match self {
            Self::Sum8 => acc.wrapping_add(*b),
            Self::Xor8 => acc ^ b,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "class")]
pub enum ByteClass {
    /// Never changes within an idle window (it may differ between windows).
    Constant,
    /// Steps by a fixed amount, modulo 256, between consecutive idle messages.
    Counter { step: u8 },
    /// Equals a checksum of the message's other bytes.
    Derived { checksum: Checksum },
    /// Varies while idle in no recognised way: meters, timestamps.
    Noisy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChannelNoise {
    pub channel: ChannelKey,
    /// Idle messages the classes rest on.
    pub messages: usize,
    /// One class per byte offset up to the shortest idle message.
    pub classes: Vec<ByteClass>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct NoiseModel {
    pub channels: Vec<ChannelNoise>,
}

impl NoiseModel {
    /// `None` when the channel, or that byte of it, was not seen while idle.
    pub fn class(&self, channel: &ChannelKey, byte: usize) -> Option<ByteClass> {
        self.channels.iter().find(|c| c.channel == *channel)?.classes.get(byte).copied()
    }

    pub fn channel(&self, channel: &ChannelKey) -> Option<&ChannelNoise> {
        self.channels.iter().find(|c| c.channel == *channel)
    }
}

/// Classifies every channel seen in idle segments. Counters are judged on consecutive messages
/// within one idle window, never across the gap between windows.
pub fn noise_model(segments: &[Segment<'_>], channels: &Channels) -> NoiseModel {
    let mut runs: HashMap<ChannelKey, Vec<Vec<Vec<u8>>>> = HashMap::new();
    for segment in segments.iter().filter(|s| s.kind == StepKind::Idle) {
        let mut in_window: HashMap<ChannelKey, Vec<Vec<u8>>> = HashMap::new();
        for m in segment.events.iter().filter_map(|e| channels.message(e)) {
            in_window.entry(m.channel).or_default().push(m.bytes);
        }
        for (channel, messages) in in_window {
            runs.entry(channel).or_default().push(messages);
        }
    }
    let mut classified: Vec<ChannelNoise> = runs.into_iter().map(|(channel, runs)| classify(channel, &runs)).collect();
    classified.sort_by_key(|c| c.channel.sort_key());
    NoiseModel { channels: classified }
}

fn classify(channel: ChannelKey, runs: &[Vec<Vec<u8>>]) -> ChannelNoise {
    let all: Vec<&[u8]> = runs.iter().flatten().map(Vec::as_slice).collect();
    let pairs: Vec<(&[u8], &[u8])> = runs.iter().flat_map(|run| run.windows(2).map(|w| (w[0].as_slice(), w[1].as_slice()))).collect();
    let len = all.iter().map(|m| m.len()).min().unwrap_or(0);
    let classes = (0..len).map(|byte| classify_byte(byte, &all, &pairs)).collect();
    ChannelNoise { channel, messages: all.len(), classes }
}

fn classify_byte(byte: usize, all: &[&[u8]], pairs: &[(&[u8], &[u8])]) -> ByteClass {
    if pairs.iter().all(|(a, b)| a[byte] == b[byte]) {
        return ByteClass::Constant;
    }
    if all.len() >= MIN_MESSAGES {
        for checksum in [Checksum::Sum8, Checksum::Xor8] {
            let hits = all.iter().filter(|m| m[byte] == checksum.of(m, byte)).count();
            if hits as f64 >= DERIVED_SHARE * all.len() as f64 {
                return ByteClass::Derived { checksum };
            }
        }
        let mut steps: HashMap<u8, usize> = HashMap::new();
        for (a, b) in pairs {
            *steps.entry(b[byte].wrapping_sub(a[byte])).or_default() += 1;
        }
        if let Some((step, n)) = steps.into_iter().max_by_key(|&(step, n)| (n, std::cmp::Reverse(step))) {
            if step != 0 && n as f64 >= COUNTER_SHARE * pairs.len() as f64 {
                return ByteClass::Counter { step };
            }
        }
    }
    ByteClass::Noisy
}
