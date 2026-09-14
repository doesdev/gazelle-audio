//! Command attribution by the fixed rules of spec §8 step 5. A byte of a host→device channel
//! is attributed to parameter P only if it
//! 1. is sent on every Set step of P, with the same raw value for the same UI value and
//!    different raw values for different UI values;
//! 2. is not changed by a No-op step of P;
//! 3. is not changed by a Control step (if it is, the field is reported shared, not P's);
//! 4. is not changed during Idle.
//!
//! "Changed" compares the last message of the channel in a step's window with the raw value of
//! P's current UI value; before P's first Set step the current value is unknown and nothing is
//! judged.

use std::collections::HashMap;

use serde::Serialize;

use super::channel::{message, ChannelKey, Message};
use super::segment::Segment;
use crate::capture::event::Direction;
use crate::session::plan::StepKind;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommandField {
    pub channel: ChannelKey,
    /// Offset into the message bytes (see [`Message::bytes`]).
    pub byte: usize,
    /// Smallest bit range `(low, high)` covering every difference between observed raw values.
    pub bits: (u8, u8),
    /// Raw byte per UI value, in first-seen order.
    pub values: Vec<(String, u8)>,
    /// `(step, packet_index)` of the message each Set step contributed.
    pub evidence: Vec<(usize, u64)>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct CommandAttribution {
    pub fields: Vec<CommandField>,
    /// Fields meeting rules 1, 2 and 4 that a Control step also changed.
    pub shared: Vec<CommandField>,
}

/// Last host→device message per channel within a segment.
fn last_out_messages<'a>(segment: &Segment<'a>) -> HashMap<ChannelKey, Message<'a>> {
    let mut last = HashMap::new();
    for m in segment.events.iter().filter_map(message).filter(|m| m.channel.direction == Direction::Out) {
        last.insert(m.channel, m);
    }
    last
}

pub fn attribute_commands(segments: &[Segment<'_>], parameter: &str) -> CommandAttribution {
    let sent: Vec<HashMap<ChannelKey, Message<'_>>> = segments.iter().map(last_out_messages).collect();
    let set_steps: Vec<usize> =
        (0..segments.len()).filter(|&i| segments[i].kind == StepKind::Set && segments[i].parameter.as_deref() == Some(parameter)).collect();
    let Some(&first) = set_steps.first() else {
        return CommandAttribution::default();
    };
    let mut channels: Vec<ChannelKey> = sent[first].keys().copied().filter(|c| set_steps.iter().all(|&i| sent[i].contains_key(c))).collect();
    channels.sort_by_key(ChannelKey::sort_key);

    let mut result = CommandAttribution::default();
    for channel in channels {
        let len = set_steps.iter().map(|&i| sent[i][&channel].bytes.len()).min().unwrap_or(0);
        for byte in 0..len {
            match judge(segments, &sent, &set_steps, parameter, channel, byte) {
                Some((field, false)) => result.fields.push(field),
                Some((field, true)) => result.shared.push(field),
                None => {}
            }
        }
    }
    result
}

/// `Some((field, shared))` when the byte satisfies the rules.
fn judge(
    segments: &[Segment<'_>],
    sent: &[HashMap<ChannelKey, Message<'_>>],
    set_steps: &[usize],
    parameter: &str,
    channel: ChannelKey,
    byte: usize,
) -> Option<(CommandField, bool)> {
    // Rule 1.
    let mut values: Vec<(String, u8)> = Vec::new();
    let mut evidence = Vec::new();
    for &i in set_steps {
        let value = segments[i].value.clone()?;
        let m = &sent[i][&channel];
        let raw = *m.bytes.get(byte)?;
        match values.iter().find(|(v, _)| *v == value) {
            Some((_, known)) if *known != raw => return None,
            Some(_) => {}
            None if values.iter().any(|(_, r)| *r == raw) => return None,
            None => values.push((value, raw)),
        }
        evidence.push((segments[i].window.step, m.event.packet_index));
    }
    if values.len() < 2 {
        return None;
    }

    // Rules 2–4, walking the steps in order with P's current raw value.
    let raw_of = |value: &str| values.iter().find(|(v, _)| v == value).map(|(_, r)| *r);
    let mut current: Option<u8> = None;
    let mut shared = false;
    for (i, segment) in segments.iter().enumerate() {
        let observed = sent[i].get(&channel).and_then(|m| m.bytes.get(byte).copied());
        let changed = matches!((observed, current), (Some(o), Some(c)) if o != c);
        let on_parameter = segment.parameter.as_deref() == Some(parameter);
        match segment.kind {
            StepKind::Set if on_parameter => current = segment.value.as_deref().and_then(raw_of),
            StepKind::NoOp if on_parameter => {
                if changed {
                    return None;
                }
            }
            StepKind::Idle => {
                if changed {
                    return None;
                }
            }
            StepKind::Control | StepKind::Set | StepKind::NoOp => shared |= changed,
        }
    }

    let mask = values.iter().flat_map(|(_, a)| values.iter().map(move |(_, b)| a ^ b)).fold(0u8, |acc, x| acc | x);
    let bits = (mask.trailing_zeros() as u8, 7 - mask.leading_zeros() as u8);
    Some((CommandField { channel, byte, bits, values, evidence }, shared))
}
