//! Field attribution by the rules of spec §8 step 5, decided by majority vote (the user's choice
//! on 2026-09-14, over all-or-nothing). A byte of a channel is attributed to parameter P when more
//! than half of its votes agree that it
//! 1. carries P's value on Set steps: each UI value's raw is the most common raw among the Set
//!    steps that reached it, different UI values have different raws, and every Set step votes
//!    on whether it matched;
//! 2. is not changed by a No-op step of P (one vote per No-op window showing the message);
//! 3. is not changed by a Control step — if it is, the field is reported shared, not P's;
//! 4. is not changed during Idle (one vote per Idle window showing the message).
//!
//! The share of agreeing votes is the field's `consistency`.
//!
//! A channel's *template* is the bytes identical across the last messages of P's Set steps. A
//! message *matches* when it agrees with every template byte, so a channel that multiplexes
//! parameters by an id byte is judged per parameter, and a sequence number or the value itself
//! is never part of the template. A step's observation is its last matching message. "Changed"
//! compares it with the raw value of P's current UI value; before P's first Set step that value
//! is unknown and nothing is judged. A Control step that changes the field marks it shared and
//! its observed value becomes current, since the device state really moved. Readback
//! attribution also masks bytes that vary while idle.

use std::collections::HashMap;

use serde::Serialize;

use super::channel::{ChannelKey, Channels, Message};
use super::noise::{ByteClass, NoiseModel};
use super::segment::Segment;
use crate::capture::event::Direction;
use crate::session::plan::StepKind;

/// A field is attributed only when strictly more than this share of its votes agree.
pub const MAJORITY: f64 = 0.5;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Field {
    pub channel: ChannelKey,
    /// Offset into the message bytes (see [`Message::bytes`]).
    pub byte: usize,
    /// Smallest bit range `(low, high)` covering every difference between observed raw values.
    pub bits: (u8, u8),
    /// The message with template bytes in hex and every other byte, the field included, as
    /// `??`: `70 ?? 01 ?? 00 …`.
    pub template: String,
    /// Raw byte per UI value, in first-seen order.
    pub values: Vec<(String, u8)>,
    /// `(step, packet_index)` of the message each agreeing Set step contributed.
    pub evidence: Vec<(usize, u64)>,
    /// Share of the field's votes that agreed with the attribution; 1.0 when none disagreed.
    pub consistency: f64,
    /// Within some step window the byte returned to a value it had left. A setting moves once
    /// per change; a meter that tracks it dips and recovers (the Studio+ peak meters did).
    pub oscillates: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MaskedByte {
    pub channel: ChannelKey,
    pub byte: usize,
    pub class: ByteClass,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Attribution {
    pub fields: Vec<Field>,
    /// Fields meeting rules 1, 2 and 4 that a Control step also changed.
    pub shared: Vec<Field>,
    /// Candidate bytes excluded because the noise model saw them vary while idle.
    pub masked: Vec<MaskedByte>,
}

/// Host→device fields carrying `parameter`.
pub fn attribute_commands(segments: &[Segment<'_>], channels: &Channels, parameter: &str) -> Attribution {
    attribute(segments, channels, None, Direction::Out, parameter)
}

/// Device→host fields reporting `parameter`, with idle-varying bytes masked.
pub fn attribute_readback(segments: &[Segment<'_>], channels: &Channels, noise: &NoiseModel, parameter: &str) -> Attribution {
    attribute(segments, channels, Some(noise), Direction::In, parameter)
}

type ByChannel<'a> = HashMap<ChannelKey, Vec<Message<'a>>>;

/// One Set step's vote input: segment index, UI value, and the byte with its packet index when a
/// matching message carried it.
type SetObservation = (usize, String, Option<(u8, u64)>);

fn by_channel<'a>(segment: &Segment<'a>, channels: &Channels, direction: Direction) -> ByChannel<'a> {
    let mut map: ByChannel<'a> = HashMap::new();
    for m in segment.events.iter().filter_map(|e| channels.message(e)).filter(|m| m.channel.direction == direction) {
        map.entry(m.channel).or_default().push(m);
    }
    map
}

fn attribute(segments: &[Segment<'_>], channels: &Channels, noise: Option<&NoiseModel>, direction: Direction, parameter: &str) -> Attribution {
    let observed: Vec<ByChannel<'_>> = segments.iter().map(|s| by_channel(s, channels, direction)).collect();
    let set_steps: Vec<usize> =
        (0..segments.len()).filter(|&i| segments[i].kind == StepKind::Set && segments[i].parameter.as_deref() == Some(parameter)).collect();
    let Some(&first) = set_steps.first() else {
        return Attribution::default();
    };
    let mut keys: Vec<ChannelKey> = observed[first].keys().copied().filter(|c| set_steps.iter().all(|&i| observed[i].contains_key(c))).collect();
    keys.sort_by_key(ChannelKey::sort_key);

    let mut result = Attribution::default();
    for channel in keys {
        let finals: Vec<&Message<'_>> = set_steps.iter().filter_map(|&i| observed[i][&channel].last()).collect();
        let len = finals.iter().map(|m| m.bytes.len()).min().unwrap_or(0);
        let template: Vec<Option<u8>> = (0..len)
            .map(|p| {
                let v = finals[0].bytes[p];
                finals.iter().all(|m| m.bytes[p] == v).then_some(v)
            })
            .collect();
        for byte in 0..len {
            if template[byte].is_some() {
                continue;
            }
            if let Some(class) = noise.and_then(|n| n.class(&channel, byte)).filter(|c| *c != ByteClass::Constant) {
                result.masked.push(MaskedByte { channel, byte, class });
                continue;
            }
            match judge(segments, &observed, &set_steps, parameter, channel, &template, byte) {
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
    observed: &[ByChannel<'_>],
    set_steps: &[usize],
    parameter: &str,
    channel: ChannelKey,
    template: &[Option<u8>],
    byte: usize,
) -> Option<(Field, bool)> {
    let matches = |m: &Message<'_>| template.iter().enumerate().all(|(p, t)| t.is_none_or(|v| m.bytes.get(p) == Some(&v)));
    let last_match = |i: usize| observed[i].get(&channel).and_then(|ms| ms.iter().rev().find(|m| matches(m)));

    // Rule 1, by majority: each UI value's raw is the most common one among the Set steps that
    // reached it (ties go to the lowest raw), and different UI values need different raws.
    let observations: Vec<SetObservation> = set_steps
        .iter()
        .filter_map(|&i| {
            let value = segments[i].value.clone()?;
            let seen = last_match(i).and_then(|m| m.bytes.get(byte).map(|b| (*b, m.event.packet_index)));
            Some((i, value, seen))
        })
        .collect();
    let mut values: Vec<(String, u8)> = Vec::new();
    for (_, value, _) in &observations {
        if values.iter().any(|(v, _)| v == value) {
            continue;
        }
        let mut counts: Vec<(u8, usize)> = Vec::new();
        for (raw, _) in observations.iter().filter(|(_, v, _)| v == value).filter_map(|(_, _, seen)| *seen) {
            match counts.iter_mut().find(|(r, _)| *r == raw) {
                Some((_, n)) => *n += 1,
                None => counts.push((raw, 1)),
            }
        }
        if let Some(&(raw, _)) = counts.iter().max_by_key(|(r, n)| (*n, std::cmp::Reverse(*r))) {
            values.push((value.clone(), raw));
        }
    }
    if values.len() < 2 || values.iter().enumerate().any(|(k, (_, r))| values[..k].iter().any(|(_, other)| other == r)) {
        return None;
    }
    let raw_of = |value: &str| values.iter().find(|(v, _)| v == value).map(|(_, r)| *r);

    let (mut votes, mut agree) = (0usize, 0usize);
    let mut evidence = Vec::new();
    for (i, value, seen) in &observations {
        votes += 1;
        if let (Some((raw, packet)), Some(expected)) = (seen, raw_of(value)) {
            if *raw == expected {
                agree += 1;
                evidence.push((segments[*i].window.step, *packet));
            }
        }
    }

    // Rules 2–4, walking the steps in order with P's current raw value. No-op and Idle windows
    // that show the message vote; a Control change marks the field shared and moves the current
    // raw, since the device state really moved.
    let mut current: Option<u8> = None;
    let mut shared = false;
    for (i, segment) in segments.iter().enumerate() {
        let seen = last_match(i).and_then(|m| m.bytes.get(byte).copied());
        let changed = matches!((seen, current), (Some(s), Some(c)) if s != c);
        let on_parameter = segment.parameter.as_deref() == Some(parameter);
        match segment.kind {
            StepKind::Set if on_parameter => current = segment.value.as_deref().and_then(raw_of),
            StepKind::NoOp if on_parameter => {
                if current.is_some() && seen.is_some() {
                    votes += 1;
                    agree += usize::from(!changed);
                }
            }
            StepKind::Idle => {
                if current.is_some() && seen.is_some() {
                    votes += 1;
                    agree += usize::from(!changed);
                }
            }
            StepKind::Control | StepKind::Set | StepKind::NoOp => {
                if changed {
                    shared = true;
                    current = seen;
                }
            }
        }
    }
    let consistency = agree as f64 / votes as f64;
    if consistency <= MAJORITY {
        return None;
    }

    let oscillates = (0..segments.len()).any(|i| {
        let Some(messages) = observed[i].get(&channel) else {
            return false;
        };
        let mut runs: Vec<u8> = Vec::new();
        for v in messages.iter().filter(|m| matches(m)).filter_map(|m| m.bytes.get(byte).copied()) {
            if runs.last() != Some(&v) {
                runs.push(v);
            }
        }
        runs.iter().enumerate().any(|(k, v)| runs[..k].contains(v))
    });

    let mask = values.iter().flat_map(|(_, a)| values.iter().map(move |(_, b)| a ^ b)).fold(0u8, |acc, x| acc | x);
    let bits = (mask.trailing_zeros() as u8, 7 - mask.leading_zeros() as u8);
    let template = template.iter().map(|t| t.map_or_else(|| "??".to_string(), |v| format!("{v:02x}"))).collect::<Vec<_>>().join(" ");
    Some((Field { channel, byte, bits, template, values, evidence, consistency, oscillates }, shared))
}
