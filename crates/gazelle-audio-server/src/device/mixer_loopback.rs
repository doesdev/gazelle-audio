//! A loopback that keeps mixer strips and stereo links like a device.
//!
//! The emulating loopback answers a read with the request's own (empty) contents, so a client
//! could not read a mixer's state or links back. This wrapper remembers each `set_mixer` /
//! `set_mixer_cfg` strip and each `set_stereo_link`, and fills the replies the registry declares:
//! `get_mixer` for the mixer named in `ext3` (33 entries, master first) and each `get_*_links`
//! read (one byte per pair, for the peripheral kind in `ext3`). Strips start at 0 dB, centred;
//! links start unlinked. This is test data, not device state.

use std::collections::HashMap;

use gazelle_audio_protocol::field::Field;
use gazelle_audio_protocol::registry::Registry;
use gazelle_audio_protocol::wire::WireError;
use gazelle_audio_transport::{Device, RawPacket, Report};

/// `get_*_links` reads share this `ext2`; their `ext3` names the peripheral kind.
const LINKS_EXT2: u32 = 11;
/// A strip that was never set: level 0 (0 dB), pan 32 (centre), no mute or solo, send 0.
const DEFAULT_STRIP: [u8; 3] = [0, 32, 0];

/// The element size and count of a reply that is one `entries` list.
fn entries(returns: &[Field]) -> Option<(usize, usize)> {
    match returns {
        [Field::StructArray { name, fields, count }] if name == "entries" => Some((fields.iter().map(Field::bit_len).sum::<usize>().div_ceil(8), *count)),
        _ => None,
    }
}

/// Where a command's parameters start in its payload: after one header byte, or two once the
/// parameters are four bytes or more (payload id | nparams, then nbytes).
fn params_offset(param_bytes: usize) -> usize {
    if param_bytes >= 4 {
        2
    } else {
        1
    }
}

struct StripCommand {
    response: u32,
    payload_id: u8,
    /// Bytes per strip after mixer id and channel: level and the pan/mute/solo byte, plus send on Studio+.
    strip_bytes: usize,
}

pub struct MixerLoopback {
    inner: Box<dyn Device + Send>,
    /// `get_mixer`'s response `(cmd, ext2)`, entry bytes and entry count.
    get_mixer: Option<((u32, u32), usize, usize)>,
    set_mixer: Option<StripCommand>,
    /// Response cmd of the links reads, and each peripheral kind's pair count.
    links_response: u32,
    link_counts: HashMap<u32, usize>,
    /// `set_stereo_link`'s response cmd and payload id.
    set_link: Option<(u32, u8)>,
    strips: HashMap<(u8, u8), [u8; 3]>,
    links: HashMap<(u8, u8), u8>,
}

impl MixerLoopback {
    /// Wrap `inner`, or return it unchanged when the registry declares none of these commands.
    pub fn wrap(inner: Box<dyn Device + Send>, registry: Option<&Registry>) -> Box<dyn Device + Send> {
        let Some(registry) = registry else { return inner };
        let get_mixer = registry.get("get_mixer").and_then(|c| entries(&c.returns).map(|(size, count)| ((c.report_id + 1, c.ext2), size, count)));
        let set_mixer = registry.get("set_mixer").or_else(|| registry.get("set_mixer_cfg")).and_then(|c| {
            let strip_bits: usize = c.params.iter().skip(2).map(Field::bit_len).sum();
            Some(StripCommand { response: c.report_id + 1, payload_id: (c.payload_id? & 0x3F) as u8, strip_bytes: strip_bits.div_ceil(8) })
        });
        let mut link_counts = HashMap::new();
        let mut links_response = 0;
        for name in registry.names() {
            let Some(command) = registry.get(name) else { continue };
            if name.starts_with("get_") && name.ends_with("_links") && command.ext2 == LINKS_EXT2 {
                if let Some((1, count)) = entries(&command.returns) {
                    link_counts.insert(command.ext3, count);
                    links_response = command.report_id + 1;
                }
            }
        }
        let set_link = registry.get("set_stereo_link").and_then(|c| Some((c.report_id + 1, (c.payload_id? & 0x3F) as u8)));
        if get_mixer.is_none() && link_counts.is_empty() {
            return inner;
        }
        Box::new(MixerLoopback { inner, get_mixer, set_mixer, links_response, link_counts, set_link, strips: HashMap::new(), links: HashMap::new() })
    }

    fn remember(&mut self, cmd: u32, contents: &[u8]) {
        let Some(&payload) = contents.first() else { return };
        if let Some(strip) = &self.set_mixer {
            let at = params_offset(2 + strip.strip_bytes);
            if cmd == strip.response && payload & 0x3F == strip.payload_id && contents.len() >= at + 2 + strip.strip_bytes {
                let mut bytes = DEFAULT_STRIP;
                bytes[..strip.strip_bytes.min(3)].copy_from_slice(&contents[at + 2..at + 2 + strip.strip_bytes.min(3)]);
                self.strips.insert((contents[at], contents[at + 1]), bytes);
                return;
            }
        }
        if let Some((response, payload_id)) = self.set_link {
            let at = params_offset(3);
            if cmd == response && payload & 0x3F == payload_id && contents.len() >= at + 3 {
                self.links.insert((contents[at], contents[at + 1]), contents[at + 2]);
            }
        }
    }
}

impl Device for MixerLoopback {
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
            if let Some((selector, size, count)) = self.get_mixer {
                if (header.cmd, header.ext2) == selector {
                    let mixer = header.ext3 as u8;
                    report.contents = (0..count as u8).flat_map(|channel| self.strips.get(&(mixer, channel)).copied().unwrap_or(DEFAULT_STRIP)[..size.min(3)].to_vec()).collect();
                    continue;
                }
            }
            if header.cmd == self.links_response && header.ext2 == LINKS_EXT2 {
                if let Some(&count) = self.link_counts.get(&header.ext3) {
                    let periph = header.ext3 as u8;
                    report.contents = (0..count as u8).map(|pair| self.links.get(&(periph, pair)).copied().unwrap_or(0)).collect();
                    continue;
                }
            }
            let contents = std::mem::take(&mut report.contents);
            self.remember(header.cmd, &contents);
            report.contents = contents;
        }
        reports
    }
}
