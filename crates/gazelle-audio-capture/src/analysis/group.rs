//! Command sequences (spec §8 step 5): the host→device messages one change emits, grouped
//! around the message that carries the value.

use std::collections::HashMap;

use super::attribute::Field;
use super::channel::{ChannelKey, Channels};
use super::segment::Segment;
use crate::capture::event::Direction;

/// Messages this close to the value-carrying message, on either side, belong to its sequence.
pub const GROUP_WINDOW_NS: u64 = 50_000_000;

#[derive(Debug, Clone, PartialEq)]
pub struct SequenceMessage {
    pub channel: ChannelKey,
    /// Bytes identical in every Set step in hex, others `??`.
    pub template: String,
    /// Median time relative to the value-carrying message, in milliseconds.
    pub offset_ms: f64,
    pub carries_value: bool,
}

/// The messages that accompany `field` on every Set step it was observed on, ordered by time.
/// Each channel contributes the message closest to the value-carrying one.
pub fn command_sequence(segments: &[Segment<'_>], channels: &Channels, field: &Field) -> Vec<SequenceMessage> {
    let mut steps: Vec<HashMap<ChannelKey, (i64, Vec<u8>)>> = Vec::new();
    for &(step, packet) in &field.evidence {
        let Some(segment) = segments.iter().find(|s| s.window.step == step) else {
            continue;
        };
        let messages: Vec<_> = segment.events.iter().filter_map(|e| channels.message(e)).filter(|m| m.channel.direction == Direction::Out).collect();
        let Some(anchor) = messages.iter().find(|m| m.event.packet_index == packet) else {
            continue;
        };
        let t0 = anchor.event.ts_ns as i64;
        let mut nearest: HashMap<ChannelKey, (i64, Vec<u8>)> = HashMap::new();
        nearest.insert(anchor.channel, (0, anchor.bytes.clone()));
        for m in &messages {
            let dt = m.event.ts_ns as i64 - t0;
            if dt.unsigned_abs() > GROUP_WINDOW_NS || m.channel == anchor.channel {
                continue;
            }
            match nearest.get(&m.channel) {
                Some((best, _)) if best.abs() <= dt.abs() => {}
                _ => {
                    nearest.insert(m.channel, (dt, m.bytes.clone()));
                }
            }
        }
        steps.push(nearest);
    }
    let Some(first) = steps.first() else {
        return Vec::new();
    };
    let mut sequence: Vec<SequenceMessage> = first
        .keys()
        .filter(|c| steps.iter().all(|s| s.contains_key(c)))
        .map(|&channel| {
            let observed: Vec<&(i64, Vec<u8>)> = steps.iter().map(|s| &s[&channel]).collect();
            let len = observed.iter().map(|(_, b)| b.len()).min().unwrap_or(0);
            let template = (0..len)
                .map(|p| {
                    let v = observed[0].1[p];
                    if observed.iter().all(|(_, b)| b[p] == v) { format!("{v:02x}") } else { "??".to_string() }
                })
                .collect::<Vec<_>>()
                .join(" ");
            let mut offsets: Vec<i64> = observed.iter().map(|(dt, _)| *dt).collect();
            offsets.sort_unstable();
            let offset_ms = offsets[offsets.len() / 2] as f64 / 1e6;
            SequenceMessage { channel, template, offset_ms, carries_value: channel == field.channel }
        })
        .collect();
    sequence.sort_by(|a, b| a.offset_ms.total_cmp(&b.offset_ms).then_with(|| a.channel.sort_key().cmp(&b.channel.sort_key())));
    sequence
}
