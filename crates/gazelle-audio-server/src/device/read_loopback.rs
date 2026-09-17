//! A loopback that answers every read with a reply of its declared layout.
//!
//! The emulating loopback answers a request by echoing it with `cmd + 1`, and a `get_*` request
//! carries no payload, so every read came back empty: a fixed-size reply failed to decode and a
//! variable-length one quietly decoded as nothing (P74). This wrapper replaces each read's reply
//! with contents sized to the command's `returns` layout, holding what a fresh device plausibly
//! reports: zeros where zero is the natural default (0 dB, no emulation, nothing assigned), every
//! licence bit set in `get_feature_mask` (P77: a zero mask would grey every microphone), and the
//! few non-zero defaults listed in [`default_reply`]. An effect's parameter read answers the panel's
//! starting values, which its schema carries as the reply fields' defaults.
//!
//! A handful of reads follow their `set_*` command, where the set's parameters map straight onto
//! the reply: panning law, mic emulations, reverb config and returns, and Thunderbolt latency.
//! Routing, mixer strips and links are kept by [`super::routing_loopback`] and
//! [`super::mixer_loopback`], which sit outside this layer and overwrite its replies for those.
//! Everything else reads its default whatever was set. This is test data, not a device simulator.

use std::collections::HashMap;

use gazelle_audio_protocol::field::Field;
use gazelle_audio_protocol::payload::{Payload, PayloadValues};
use gazelle_audio_protocol::registry::Registry;
use gazelle_audio_protocol::wire::WireError;
use gazelle_audio_transport::{Device, RawPacket, Report};

/// The number of bytes a reply of this layout takes. No read's reply has a bit field at the top
/// level (struct arrays pack theirs inside `Field::size`), so the fields' sizes add up; the table
/// test in `tests/loopback_reads.rs` would catch a layout where they do not.
fn layout_len(fields: &[Field]) -> usize {
    fields.iter().map(Field::size).sum()
}

/// Where a byte-aligned top-level field starts, when every field before it is byte-aligned too.
fn offset_of(fields: &[Field], name: &str) -> Option<usize> {
    let mut at = 0;
    for field in fields {
        if field.name() == name {
            return Some(at);
        }
        at += field.size();
    }
    None
}

/// The element size of a reply that is one `entries` struct array.
fn entry_size(fields: &[Field]) -> Option<usize> {
    match fields {
        [Field::StructArray { name, fields, .. }] if name == "entries" => Some(fields.iter().map(Field::bit_len).sum::<usize>().div_ceil(8)),
        _ => None,
    }
}

/// Each entry of a reply that is one `entries` struct array, filled from its fields' declared
/// defaults (zero where a field has none), when any field declares one. The effect parameter reads
/// carry the panels' starting values this way (`refs/schemas/afx_parameters.json`).
fn declared_defaults(returns: &[Field]) -> Option<Vec<u8>> {
    let [Field::StructArray { name, fields, count }] = returns else { return None };
    let declares = fields.iter().any(|f| matches!(f, Field::Scalar { default: Some(_), .. }));
    if name != "entries" || !declares {
        return None;
    }
    let entry = Payload::new(None, fields.clone()).ok()?.to_bytes(&PayloadValues::default()).ok()?;
    Some(entry.repeat(*count))
}

/// What a fresh device reports for `name`: zeros of the layout's length, except where a zero would
/// be implausible or unhelpful.
fn default_reply(name: &str, returns: &[Field]) -> Vec<u8> {
    if let Some(bytes) = declared_defaults(returns).filter(|bytes| bytes.len() == layout_len(returns)) {
        return bytes;
    }
    let mut bytes = vec![0; layout_len(returns)];
    match name {
        // Every feature reported available, so every page can be tried (the emulator only).
        "get_feature_mask" => bytes.fill(0xFF),
        // `density` has the schema default 100 on `set_reverb_config`; the reverb starts off.
        "get_reverb_config" => {
            if let Some(at) = offset_of(returns, "density") {
                bytes[at] = 100;
            }
        }
        // Reverb sends share the mixer strip layout: level byte, then pan (6 bits) | mute | solo.
        // Centred, like the mixer loopback's strips.
        "get_reverb_sends" => {
            if let Some(size) = entry_size(returns).filter(|&size| size >= 2) {
                bytes.chunks_mut(size).for_each(|entry| entry[1] = 32);
            }
        }
        // Mode 1 is Normal (P75: 0 Fast, 1 Normal, 2 Safe).
        "get_tb_latency" => {
            if let Some(at) = offset_of(returns, "mode") {
                bytes[at] = 1;
            }
        }
        // The Quadro build's fixed AFX2DAW split point (`FIXED_AFX2DAW_COMP_SPLIT_POINT = 16`).
        "get_daw_mode" => {
            if let Some(at) = offset_of(returns, "split_point") {
                bytes[at] = 16;
            }
        }
        // Effect catalogues list each type id once, with no instances.
        "get_afx_available_instances" | "get_afx_max_available_instances" | "get_afx_remaining_featured_instances" => {
            if let Some(size) = entry_size(returns) {
                bytes.chunks_mut(size).enumerate().for_each(|(i, entry)| entry[0] = i as u8);
            }
        }
        _ => {}
    }
    bytes
}

/// How a remembered set changes its read's reply, given the set's parameter bytes.
#[derive(Clone, Copy)]
enum Follow {
    /// The reply is the parameters, byte for byte (same layout).
    Whole,
    /// `set_mic_emulation`: the first byte picks an entry, the rest is that entry.
    Entry,
    /// `set_reverb_return`: the first byte (signed) picks a one-byte entry, the second is it.
    SignedEntry,
}

/// Each followed set, with the read it changes.
const FOLLOWS: &[(&str, &str, Follow)] = &[
    ("set_panning_law", "get_panning_law", Follow::Whole),
    ("set_mic_emulation", "get_mic_emulations", Follow::Entry),
    ("set_reverb_config", "get_reverb_config", Follow::Whole),
    ("set_reverb_return", "get_reverb_returns", Follow::SignedEntry),
    ("set_tb_latency", "get_tb_latency", Follow::Whole),
];

struct Read {
    name: String,
    /// The reply's `cmd` and `ext2`, and its `ext3` when another read shares both.
    response: u32,
    ext2: u32,
    ext3: Option<u32>,
}

struct Set {
    response: u32,
    /// The payload id the echo's first byte carries, if the command has a payload header.
    payload_id: Option<u8>,
    /// Header bytes before the parameters, and the parameters' length.
    offset: usize,
    len: usize,
    read: String,
    follow: Follow,
}

pub struct ReadLoopback {
    inner: Box<dyn Device + Send>,
    reads: Vec<Read>,
    sets: Vec<Set>,
    /// The current reply to each read, by name.
    replies: HashMap<String, Vec<u8>>,
}

impl ReadLoopback {
    /// Wrap `inner`, or return it unchanged when there is no registry to size replies from.
    pub fn wrap(inner: Box<dyn Device + Send>, registry: Option<&Registry>) -> Box<dyn Device + Send> {
        let Some(registry) = registry else { return inner };
        let mut names: Vec<&String> = registry.names().filter(|n| n.starts_with("get_")).collect();
        names.sort();
        let commands: Vec<_> = names.iter().filter_map(|n| registry.get(n)).filter(|c| !c.returns.is_empty()).collect();
        let reads = commands
            .iter()
            .map(|c| {
                let shared = commands.iter().any(|o| o.name != c.name && (o.report_id, o.ext2) == (c.report_id, c.ext2));
                Read { name: c.name.clone(), response: c.report_id + 1, ext2: c.ext2, ext3: shared.then_some(c.ext3) }
            })
            .collect();
        let replies = commands.iter().map(|c| (c.name.clone(), default_reply(&c.name, &c.returns))).collect();
        let sets = FOLLOWS
            .iter()
            .filter_map(|&(set, read, follow)| {
                let command = registry.get(set)?;
                registry.get(read)?;
                let payload = Payload::new(command.payload_id, command.params.clone()).ok()?;
                Some(Set {
                    response: command.report_id + 1,
                    payload_id: command.payload_id.map(|id| (id & 0x3F) as u8),
                    offset: payload.bytesize - payload.user_bytes,
                    len: payload.user_bytes,
                    read: read.to_string(),
                    follow,
                })
            })
            .collect();
        Box::new(ReadLoopback { inner, reads, sets, replies })
    }

    fn read_for(&self, report: &Report) -> Option<&str> {
        let h = report.header;
        self.reads
            .iter()
            .find(|r| (r.response, r.ext2) == (h.cmd, h.ext2) && r.ext3.is_none_or(|ext3| ext3 == h.ext3))
            .map(|r| r.name.as_str())
    }

    /// Apply a set's echo to the reply it changes.
    fn remember(&mut self, report: &Report) {
        let contents = &report.contents;
        let Some(set) = self.sets.iter().find(|s| {
            s.response == report.header.cmd
                && contents.len() >= s.offset + s.len
                && s.payload_id.is_none_or(|id| contents.first().is_some_and(|b| b & 0x3F == id))
        }) else {
            return;
        };
        let params = &contents[set.offset..set.offset + set.len];
        let Some(reply) = self.replies.get_mut(&set.read) else { return };
        match set.follow {
            Follow::Whole if reply.len() == params.len() => reply.copy_from_slice(params),
            Follow::Whole => {}
            Follow::Entry => {
                let size = params.len() - 1;
                let at = usize::from(params[0]) * size;
                if at + size <= reply.len() {
                    reply[at..at + size].copy_from_slice(&params[1..]);
                }
            }
            Follow::SignedEntry => {
                if let (Ok(at), [_, value, ..]) = (usize::try_from(params[0] as i8), params) {
                    if at < reply.len() {
                        reply[at] = *value;
                    }
                }
            }
        }
    }
}

impl Device for ReadLoopback {
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
        for report in &mut reports {
            if let Some(name) = self.read_for(report) {
                report.contents = self.replies[name].clone();
            } else {
                self.remember(report);
            }
        }
        reports
    }
}
