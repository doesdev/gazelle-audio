//! A loopback that keeps routing state like a device.
//!
//! The emulating loopback answers a request with the request's own contents, which are empty for
//! `get_routing`, so a client could neither read routing nor see a write take effect. This wrapper
//! remembers each `set_routing` (one destination group's 32 slots) and fills `get_routing` replies
//! with the group named in the request's `ext3`, as the panel's `get_device_data` reads it. A group
//! never set reads as an identity-like routing ([`DEFAULTS`]): outputs play the computer, the
//! headphones their mixes, recordings take the inputs, and each mixer input takes the preamps (after
//! the Quadro's six effect returns) and the first computer pair; every other slot is MUTE. This is
//! test data, not device state: a real device starts from its stored routing, and the factory
//! routing is not known.

use std::collections::HashMap;

use gazelle_audio_protocol::field::Field;
use gazelle_audio_protocol::registry::Registry;
use gazelle_audio_protocol::wire::WireError;
use gazelle_audio_transport::{Device, RawPacket, Report};

use crate::registry_set::{PID_QUADRO, PID_STUDIO};

/// Slots per destination group in `set_routing` (`bank_configs` count in both schemas).
const SLOTS: usize = 32;

pub(crate) const QUADRO_TOPOLOGY: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../refs/schemas/quadro_topology.json"));
pub(crate) const STUDIO_TOPOLOGY: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../refs/schemas/studio_topology.json"));

/// A run of source channels: source group id, first channel, channel count.
type Span = (&'static str, u8, u8);
/// A destination group id and the sources its slots take in order.
type Destination = (&'static str, &'static [Span]);
/// Slots as `(source position, channel)`, by destination position.
type Routing = HashMap<u32, Vec<(u8, u8)>>;

/// The routing a fresh loopback reports, per family: destination group id, then the sources its
/// slots take in order. Groups are named by topology id and resolved to wire positions (a group's
/// position in the topology), so the table cannot drift from the panel's order.
const DEFAULTS: &[(&str, &[Destination])] = &[
    (
        "quadro",
        &[
            ("LINE_OUT0", &[("COM_PLAY0", 0, 2)]),
            ("HEADPHONES0", &[("MIXER_OUT0", 0, 2)]),
            ("HEADPHONES1", &[("MIXER_OUT1", 0, 2)]),
            ("MONITOR0", &[("COM_PLAY0", 0, 2)]),
            ("COM_REC0", &[("PREAMP0", 0, 4)]),
            ("USB_REC0", &[("PREAMP0", 0, 2)]),
            ("SPDIF_OUT0", &[("COM_PLAY0", 0, 2)]),
            ("MIXER_IN0", &[("AFX_OUT0", 0, 6), ("PREAMP0", 0, 4), ("COM_PLAY0", 0, 2)]),
            ("MIXER_IN1", &[("AFX_OUT0", 0, 6), ("PREAMP0", 0, 4), ("COM_PLAY0", 0, 2)]),
            ("MIXER_IN2", &[("AFX_OUT0", 0, 6), ("PREAMP0", 0, 4), ("COM_PLAY0", 0, 2)]),
            ("MIXER_IN3", &[("AFX_OUT0", 0, 6), ("PREAMP0", 0, 4), ("COM_PLAY0", 0, 2)]),
        ],
    ),
    (
        "studio",
        &[
            ("LINE_OUT0", &[("USB_PLAY0", 0, 8)]),
            ("HEADPHONES0", &[("MIXER_OUT0", 0, 2)]),
            ("HEADPHONES1", &[("MIXER_OUT1", 0, 2)]),
            ("MONITOR0", &[("USB_PLAY0", 0, 2)]),
            ("TB_REC0", &[("PREAMP0", 0, 12), ("LINE_IN0", 0, 8)]),
            ("USB_REC0", &[("PREAMP0", 0, 12), ("LINE_IN0", 0, 8)]),
            ("ADAT_OUT0", &[("USB_PLAY0", 8, 16)]),
            ("SPDIF_OUT0", &[("USB_PLAY0", 0, 2)]),
            ("MIXER_IN0", &[("PREAMP0", 0, 4), ("USB_PLAY0", 0, 2)]),
            ("MIXER_IN1", &[("PREAMP0", 0, 4), ("USB_PLAY0", 0, 2)]),
            ("MIXER_IN2", &[("PREAMP0", 0, 4), ("USB_PLAY0", 0, 2)]),
            ("MIXER_IN3", &[("PREAMP0", 0, 4), ("USB_PLAY0", 0, 2)]),
        ],
    ),
];

/// A family's MUTE source position and its default routing by destination position, resolved
/// from its topology. `None` for an unknown model, or if a named group or channel is missing.
fn defaults(pid: u16) -> Option<(u8, Routing)> {
    let (family, json) = match pid {
        PID_QUADRO => ("quadro", QUADRO_TOPOLOGY),
        PID_STUDIO => ("studio", STUDIO_TOPOLOGY),
        _ => return None,
    };
    let topology: serde_json::Value = serde_json::from_str(json).ok()?;
    let group = |kind: &str, id: &str| {
        let groups = topology[kind].as_array()?;
        let at = groups.iter().position(|g| g["id"] == id)?;
        Some((at, groups[at]["channels"].as_u64()?))
    };
    let mute = u8::try_from(group("inputs", "MUTE0")?.0).ok()?;
    let (_, table) = DEFAULTS.iter().find(|(f, _)| *f == family)?;
    let mut routing = HashMap::new();
    for (destination, spans) in table.iter() {
        let (at, channels) = group("outputs", destination)?;
        let mut slots = Vec::new();
        for &(source, first, count) in spans.iter() {
            let (position, available) = group("inputs", source)?;
            if u64::from(first) + u64::from(count) > available {
                return None;
            }
            slots.extend((first..first + count).map(|channel| (position as u8, channel)));
        }
        if slots.len() as u64 > channels {
            return None;
        }
        routing.insert(at as u32, slots);
    }
    Some((mute, routing))
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
    /// What a group never set reads as, by destination position; slots past its list are MUTE.
    defaults: Routing,
    groups: HashMap<u32, [(u8, u8); SLOTS]>,
}

impl RoutingLoopback {
    /// Wrap `inner`, or return it unchanged when the model's routing commands or MUTE source are
    /// not known.
    pub fn wrap(inner: Box<dyn Device + Send>, registry: Option<&Registry>) -> Box<dyn Device + Send> {
        let Some((mute, defaults)) = defaults(inner.pid()) else { return inner };
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
            defaults,
            groups: HashMap::new(),
        })
    }

    fn reply(&self, group: u32) -> Vec<u8> {
        let slots: &[(u8, u8)] = match self.groups.get(&group) {
            Some(set) => set,
            None => self.defaults.get(&group).map_or(&[], Vec::as_slice),
        };
        let mut contents = Vec::with_capacity(1 + 2 * self.reply_pairs);
        contents.push(group as u8);
        for i in 0..self.reply_pairs {
            let (source, channel) = slots.get(i).copied().unwrap_or((self.mute, 0));
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
