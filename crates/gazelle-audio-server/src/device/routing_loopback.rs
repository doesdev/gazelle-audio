//! A loopback that keeps routing state like a device.
//!
//! The emulating loopback answers a request with the request's own contents, which are empty for
//! `get_routing`, so a client could neither read routing nor see a write take effect. This wrapper
//! remembers each `set_routing` (one destination group's 32 slots) and fills `get_routing` replies
//! with the group named in the request's `ext3`, as the panel's `get_device_data` reads it. Slots
//! never set read as the family's MUTE source group. This is test data, not device state: a real
//! device starts from its stored routing.

use std::collections::HashMap;

use gazelle_audio_protocol::field::Field;
use gazelle_audio_protocol::registry::Registry;
use gazelle_audio_protocol::wire::WireError;
use gazelle_audio_transport::{Device, RawPacket, Report};

use crate::registry_set::{PID_QUADRO, PID_STUDIO};

/// Slots per destination group in `set_routing` (`bank_configs` count in both schemas).
const SLOTS: usize = 32;

/// The MUTE source group's position in the panel's input group list (bytecode research, 2026-09).
fn mute_source(pid: u16) -> Option<u8> {
    match pid {
        PID_QUADRO => Some(10),
        PID_STUDIO => Some(11),
        _ => None,
    }
}

pub struct RoutingLoopback {
    inner: Box<dyn Device + Send>,
    /// `get_routing`'s response `(cmd, ext2)`.
    get_response: (u32, u32),
    /// `set_routing`'s response `cmd` and payload id.
    set_response: (u32, u8),
    /// Pairs in a `get_routing` reply: the schema's count (Quadro declares 64, Studio+ 32).
    reply_pairs: usize,
    mute: u8,
    groups: HashMap<u32, [(u8, u8); SLOTS]>,
}

impl RoutingLoopback {
    /// Wrap `inner`, or return it unchanged when the model's routing commands or MUTE source are
    /// not known.
    pub fn wrap(inner: Box<dyn Device + Send>, registry: Option<&Registry>) -> Box<dyn Device + Send> {
        let Some(mute) = mute_source(inner.pid()) else { return inner };
        let Some(registry) = registry else { return inner };
        let (Some(get), Some(set)) = (registry.get("get_routing"), registry.get("set_routing")) else {
            return inner;
        };
        let Some(payload_id) = set.payload_id else { return inner };
        let reply_pairs = get
            .returns
            .iter()
            .find_map(|f| match f {
                Field::StructArray { name, count, .. } if name == "bank_configs" => Some(*count),
                _ => None,
            })
            .unwrap_or(SLOTS);
        Box::new(RoutingLoopback {
            inner,
            get_response: (get.report_id + 1, get.ext2),
            set_response: (set.report_id + 1, (payload_id & 0x3F) as u8),
            reply_pairs,
            mute,
            groups: HashMap::new(),
        })
    }

    fn reply(&self, group: u32) -> Vec<u8> {
        let slots = self.groups.get(&group);
        let mut contents = Vec::with_capacity(1 + 2 * self.reply_pairs);
        contents.push(group as u8);
        for i in 0..self.reply_pairs {
            let (source, channel) = slots.and_then(|s| s.get(i).copied()).unwrap_or((self.mute, 0));
            contents.extend([source, channel]);
        }
        contents
    }

    /// Record a `set_routing` echo: payload header (payload id | nparams, nbytes), then
    /// `bank_idx` and 32 `(source group, channel)` pairs.
    fn apply(&mut self, contents: &[u8]) {
        if contents.len() < 3 + 2 * SLOTS || contents[0] & 0x3F != self.set_response.1 {
            return;
        }
        let mut slots = [(0, 0); SLOTS];
        for (i, slot) in slots.iter_mut().enumerate() {
            *slot = (contents[3 + 2 * i], contents[4 + 2 * i]);
        }
        self.groups.insert(u32::from(contents[2]), slots);
    }
}

impl Device for RoutingLoopback {
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
            let header = report.header;
            if (header.cmd, header.ext2) == self.get_response {
                report.contents = self.reply(header.ext3);
            } else if header.cmd == self.set_response.0 {
                let contents = std::mem::take(&mut report.contents);
                self.apply(&contents);
                report.contents = contents;
            }
        }
        reports
    }
}
