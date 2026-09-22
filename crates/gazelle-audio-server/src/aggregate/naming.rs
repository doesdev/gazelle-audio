//! What the aggregate calls its interfaces and their channels: Gazelle's own names, worked out in
//! one place, so the driver's file, the answer and every message say the same thing.
//!
//! **An interface is called by Gazelle's name for its device**: the person's own name for it where
//! they gave one, else its model, exactly what the sidebar shows. Renaming the device is the one way
//! to rename it in the aggregate, so there is no second name to keep in step. In a DAW, where every
//! channel carries its interface's name inside 31 characters, a device nobody has named goes by its
//! model's short form instead ([`daw_names`]): "Quadro", "Studio+".
//!
//! **An aggregate channel is one of the interface's USB audio channels**, not a preamp or an output.
//! Aggregate input *k* is the interface's USB record channel *k* (a routing destination), and
//! aggregate output *k* is its USB playback channel *k* (a routing source). That was measured at the
//! hardware channel by channel, and the vendor drivers' own counts are exactly those groups' counts:
//! 16 each way on the Zen Quadro Synergy Core, 24 each way on the Zen Studio+ ([`USB_GROUPS`]).
//!
//! So a channel is named for what it carries, then by that USB channel. An input is named for what
//! Gazelle's routing sends to its record channel, in Gazelle's own words (the person's name for a
//! Mixer channel or a mix where they gave one, else the source as the Routing page shows it):
//! "Vocal mic, USB A REC 1". An output is named for where the routing sends its playback channel:
//! the hardware output it reaches, directly or through a mix ("Monitor L, USB 1 PLAY 1"), else the
//! mix channel it lands in ("Click in Cue, USB 1 PLAY 3"), else nowhere ("USB 1 PLAY 5, not routed").
//! Re-routing changes the name, which is the point: it says where the audio goes.
//!
//! The driver is given the short half of that, the label (`input_names` and `output_names` in its
//! file), and adds its own reference after it, "{interface name} {number}", inside the interface's
//! 31 characters. A name the person types for a channel wins over the automatic one, and the two are
//! never mixed up: only typed names are kept in the workspace, and the automatic ones are worked out
//! here whenever the file is written.

use std::collections::BTreeMap;

use crate::device::descriptor::DeviceId;
use crate::registry_set::model_facts;
use crate::workspace::model::{Aggregate, AggregateDevice, DeviceMixer, RouteSource, Workspace, CHANNEL_NAME_MAX};
use crate::workspace::topology::{self, Group};

/// Which routing groups a model's aggregate channels are: `(family, record group, playback group)`
/// by topology id. The Studio+'s `TB_REC` and `TB_PLAY` are its Thunderbolt audio, which Gazelle
/// does not use; over USB its channels are `USB_REC` and `USB_PLAY`.
pub const USB_GROUPS: &[(&str, &str, &str)] = &[("quadro", "COM_REC0", "COM_PLAY0"), ("studio", "USB_REC0", "USB_PLAY0")];

/// The destination group kinds that are sockets on the interface, which is where an output's audio
/// ends up, with what Gazelle calls each (`None` is the group's own name, as with HP1 and HP2).
const HARDWARE_OUTPUTS: &[(&str, Option<&str>)] = &[
    ("MONITOR", Some("Monitor")),
    ("HEADPHONES", None),
    ("LINE_OUT", Some("Line out")),
    ("REAMP", Some("Reamp")),
    ("SPDIF_OUT", Some("S/PDIF out")),
    ("ADAT_OUT", Some("ADAT out")),
];

/// The routing groups a device's channels are named from, by topology id, each as its slots.
pub type Routing = BTreeMap<String, Vec<[u8; 2]>>;

/// An interface's USB channels, as its topology has them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsbGroups {
    /// The destination group the aggregate's inputs record from.
    pub record: Group,
    /// Its wire position, which `get_routing` takes as its `ext3`.
    pub record_position: u32,
    /// The source group the aggregate's outputs play into.
    pub playback: Group,
    /// Its place among the sources, which is how a routing slot names it.
    pub playback_position: u32,
}

/// A model's USB channels, or nothing for a model Gazelle does not know.
pub fn usb_groups(family: &str) -> Option<UsbGroups> {
    let (_, record, playback) = USB_GROUPS.iter().find(|(known, _, _)| *known == family)?;
    let destinations = topology::destination_groups_whole(family)?;
    let sources = topology::source_groups(family)?;
    let record_position = destinations.iter().position(|group| group.id == *record)?;
    let playback_position = sources.iter().position(|group| group.id == *playback)?;
    Some(UsbGroups {
        record: destinations[record_position].clone(),
        record_position: record_position as u32,
        playback: sources[playback_position].clone(),
        playback_position: playback_position as u32,
    })
}

/// Every routing group a model's channel names are worked out from, as `(topology id, wire
/// position)`: its USB record group, its hardware outputs and its mix inputs. These are the groups
/// the server reads, once each, when it has not seen them.
pub fn naming_groups(family: &str) -> Vec<(String, u32)> {
    let Some(groups) = usb_groups(family) else { return Vec::new() };
    let mixes = topology::mix_inputs(family).unwrap_or_default();
    topology::destination_groups_whole(family)
        .unwrap_or_default()
        .iter()
        .enumerate()
        .filter(|(_, group)| group.id == groups.record.id || hardware_name(group).is_some() || mixes.contains(&group.id))
        .map(|(at, group)| (group.id.clone(), at as u32))
        .collect()
}

/// How many channels an interface of this model has in the aggregate, as `(inputs, outputs)`.
/// Exact, and known without a DAW: it is the USB groups' own counts.
pub fn channel_counts(family: &str) -> Option<(u32, u32)> {
    usb_groups(family).map(|groups| (groups.record.channels, groups.playback.channels))
}

/// What to call a model when nothing better is known, as the rest of the app names it.
pub fn family_words(family: &str) -> Option<&'static str> {
    model_facts(family).map(|facts| facts.model)
}

/// The Gazelle device an entry is: the one the person chose, else the one last worked out.
pub fn device_of(device: &AggregateDevice) -> Option<&DeviceId> {
    device.device_id.as_ref().or_else(|| device.known.as_ref().and_then(|known| known.device_id.as_ref()))
}

/// The person's own name for the device an entry is, when they gave one.
fn alias<'a>(device: &AggregateDevice, workspace: &'a Workspace) -> Option<&'a str> {
    device_of(device).and_then(|id| workspace.aliases.get(id)).map(|name| name.trim()).filter(|name| !name.is_empty())
}

/// Gazelle's name for the device one entry is, before two entries that come out the same are told
/// apart ([`interface_names`]).
///
/// The person's own name for the device, else its model, which is what the sidebar shows. With
/// neither known (an interface Gazelle has never been able to match), its model from the vendor
/// driver's entry where that was worked out, and only then the vendor driver's registry key.
pub fn gazelle_name(device: &AggregateDevice, workspace: &Workspace) -> String {
    let known = device.known.as_ref();
    alias(device, workspace)
        .map(str::to_string)
        .or_else(|| known.and_then(|known| known.model.clone()).filter(|model| !model.trim().is_empty()))
        .or_else(|| known.and_then(|known| known.family.as_deref()).and_then(family_words).map(str::to_string))
        .or_else(|| device.key.as_deref().map(str::trim).filter(|key| !key.is_empty()).map(str::to_string))
        .or_else(|| device.clsid.clone())
        .unwrap_or_else(|| "an interface with no key or class id".to_string())
}

/// Every entry's name in setup order, told apart.
///
/// Two interfaces of one model that nobody has named come out the same, so the second and later
/// ones have their place in the setup after them. Naming the devices in Gazelle is the way out of it.
pub fn interface_names(config: &Aggregate, workspace: &Workspace) -> Vec<String> {
    distinct(config.devices.iter().map(|device| gazelle_name(device, workspace)).collect())
}

/// Names made distinct without case, the later of two alike taking its place from one after it.
pub fn distinct(names: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(names.len());
    for (at, name) in names.iter().enumerate() {
        let taken = names[..at].iter().any(|earlier| earlier.eq_ignore_ascii_case(name));
        out.push(if taken { format!("{name} ({})", at + 1) } else { name.clone() });
    }
    out
}

/// What every entry is called in a DAW, in setup order: the person's own name for the device, else
/// its model's short form, "Quadro" or "Studio+", so a channel reads "Monitor L (Quadro 3)" and fits
/// the interface's 31 characters. Only the DAW's reference is shortened: the page and every message
/// keep Gazelle's full name. Two alike are told apart by a count, "Quadro" and "Quadro 2", and this
/// is the name the driver is given for each interface and for the callback master.
pub fn daw_names(config: &Aggregate, workspace: &Workspace) -> Vec<String> {
    let full = interface_names(config, workspace);
    let bases: Vec<String> = config
        .devices
        .iter()
        .zip(&full)
        .map(|(device, name)| {
            let short = device.known.as_ref().and_then(|known| known.family.as_deref()).and_then(model_facts).map(|facts| facts.short);
            alias(device, workspace).map(str::to_string).or_else(|| short.map(str::to_string)).unwrap_or_else(|| name.clone())
        })
        .collect();
    counted(bases)
}

/// Names made distinct without case by a count: the second of a name is "{name} 2", the third
/// "{name} 3".
pub fn counted(names: Vec<String>) -> Vec<String> {
    names
        .iter()
        .enumerate()
        .map(|(at, name)| {
            let before = names[..at].iter().filter(|earlier| earlier.eq_ignore_ascii_case(name)).count();
            if before == 0 {
                name.clone()
            } else {
                format!("{name} {}", before + 1)
            }
        })
        .collect()
}

/// What a routing source is called, in Gazelle's own words, with the person's own names first: the
/// Mixer channel they named that takes this source, else the mix they named when the source is a
/// mix's output, else the source as the Routing page and the Mixer show it ("PREAMP 1", "AFX OUT 3",
/// "USB 1 PLAY 5"). MUTE is nothing, and so is a source the model does not have.
pub fn source_name(family: &str, source: u8, channel: u8, mixer: Option<&DeviceMixer>) -> Option<String> {
    let group = topology::source_groups(family)?.get(usize::from(source))?;
    if group.kind == "MUTE" {
        return None;
    }
    let wanted = RouteSource { group: u32::from(source), channel: u32::from(channel) };
    if let Some(named) = mixer.and_then(|mixer| mixer.channels.iter().find(|one| one.source == Some(wanted) && !one.name.trim().is_empty())) {
        return Some(plain(&named.name));
    }
    let mix = topology::mix_outputs(family).and_then(|outputs| outputs.iter().position(|id| *id == group.id));
    if let Some(name) = mix.and_then(|mix| mixer?.mixes.get(mix)?.name.clone()).filter(|name| !name.trim().is_empty()) {
        return Some(format!("{} {}", plain(&name), side(u32::from(channel), 2)));
    }
    Some(group_channel(group, u32::from(channel)))
}

/// A channel of a stereo pair as L or R, and of anything else as its number from one.
fn side(channel: u32, channels: u32) -> String {
    match (channels, channel) {
        (2, 0) => "L".into(),
        (2, 1) => "R".into(),
        _ => (channel + 1).to_string(),
    }
}

/// A group's channel as the Routing page names it: the group, and its number from one when it has
/// more than one.
fn group_channel(group: &Group, channel: u32) -> String {
    if group.channels > 1 {
        format!("{} {}", group.name, channel + 1)
    } else {
        group.name.clone()
    }
}

/// What Gazelle calls a hardware output group, or nothing for a group that is not one.
fn hardware_name(group: &Group) -> Option<String> {
    HARDWARE_OUTPUTS.iter().find(|(kind, _)| *kind == group.kind).map(|(_, words)| words.map_or_else(|| group.name.clone(), str::to_string))
}

/// One channel of a hardware output as a person names it: "Monitor L", "HP1 R", "Line out 3".
fn hardware_channel(group: &Group, channel: u32) -> Option<String> {
    Some(format!("{} {}", hardware_name(group)?, side(channel, group.channels)))
}

/// A person's name made fit to go in a channel's name: one line, trimmed.
fn plain(text: &str) -> String {
    text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect::<String>().trim().to_string()
}

/// One channel's name, in its parts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelName {
    /// What it carries: what routing sends to an input, or where an output ends up, in the person's
    /// words where they gave some. Nothing when that is not known.
    pub carries: Option<String>,
    /// The USB channel it is: "USB A REC 1", "USB 1 PLAY 5".
    pub usb: String,
    /// True for an output the routing sends nowhere, which is said as such.
    pub unrouted: bool,
}

impl ChannelName {
    /// The whole name, as the page shows it: "Vocal mic, USB A REC 1", "USB 1 PLAY 5, not routed", or
    /// the USB channel alone.
    pub fn text(&self) -> String {
        match (&self.carries, self.unrouted) {
            (Some(carries), _) => format!("{carries}, {}", self.usb),
            (None, true) => format!("{}, not routed", self.usb),
            (None, false) => self.usb.clone(),
        }
    }

    /// The label the driver is given: what it carries, else "Not routed" or the USB channel, cut to
    /// what the interface carries. Short on purpose, because the driver puts its own reference after
    /// it ([`CHANNEL_NAME_MAX`]).
    pub fn label(&self) -> String {
        match (&self.carries, self.unrouted) {
            (Some(carries), _) => fit(carries),
            (None, true) => "Not routed".into(),
            (None, false) => fit(&self.usb),
        }
    }
}

/// As much of a label as the interface carries, in whole characters.
pub fn fit(text: &str) -> String {
    text.chars().take(CHANNEL_NAME_MAX).collect::<String>().trim_end().to_string()
}

/// Aggregate input `channel` of an interface of this model: its USB record channel, named first for
/// what routing sends to it. `routing` holds the groups seen so far; while the record group is not
/// among them the channel is named by its USB channel alone.
pub fn input_channel(family: &str, channel: u32, routing: &Routing, mixer: Option<&DeviceMixer>) -> Option<ChannelName> {
    let groups = usb_groups(family)?;
    if channel >= groups.record.channels {
        return None;
    }
    let slot = routing.get(&groups.record.id).and_then(|slots| slots.get(channel as usize));
    let carries = slot.and_then(|[source, from]| source_name(family, *source, *from, mixer));
    Some(ChannelName { carries, usb: group_channel(&groups.record, channel), unrouted: false })
}

/// Every place the routing sends one source: the destination groups whose slots take it, as
/// `(group, channel)`, in topology order, from the groups seen so far.
fn taking<'a>(destinations: &'a [Group], routing: &Routing, source: [u8; 2]) -> Vec<(&'a Group, u32)> {
    destinations
        .iter()
        .flat_map(|group| {
            let slots = routing.get(&group.id);
            (0..group.channels).filter(move |&at| slots.and_then(|slots| slots.get(at as usize)) == Some(&source)).map(move |at| (group, at))
        })
        .collect()
}

/// Aggregate output `channel`: its USB playback channel, which is a routing source, named for where
/// the routing sends it.
///
/// A hardware output it reaches, directly or through a mix, comes first: "Monitor L". Through a mix
/// the side is the playback channel's own within its pair (the first of each pair is L), which is how
/// a DAW's stereo pairs land in a stereo mix. With no hardware output reached, the mix channel it
/// lands in: "Click in Cue", with the person's names for the channel and the mix where they gave
/// them. Several places are the first and a count of the rest: "Monitor L +2". Nowhere, once every
/// group it could go to has been seen, is said as not routed; anything less is not known yet, and
/// the channel is its USB channel alone.
pub fn output_channel(family: &str, channel: u32, routing: &Routing, mixer: Option<&DeviceMixer>) -> Option<ChannelName> {
    let groups = usb_groups(family)?;
    if channel >= groups.playback.channels {
        return None;
    }
    let usb = group_channel(&groups.playback, channel);
    let destinations = topology::destination_groups_whole(family).unwrap_or_default();
    let sources = topology::source_groups(family).unwrap_or_default();
    let mix_inputs = topology::mix_inputs(family).unwrap_or_default();
    let mix_outputs = topology::mix_outputs(family).unwrap_or_default();
    let reached = |source: [u8; 2]| taking(destinations, routing, source);

    let mut hardware: Vec<String> = Vec::new();
    let mut mixed: Vec<String> = Vec::new();
    let add = |list: &mut Vec<String>, name: String| {
        if !list.contains(&name) {
            list.push(name);
        }
    };
    let own = [groups.playback_position as u8, channel as u8];
    for (group, at) in reached(own) {
        if let Some(name) = hardware_channel(group, at) {
            add(&mut hardware, name);
            continue;
        }
        let Some(mix) = mix_inputs.iter().position(|id| *id == group.id) else { continue };
        // Where that mix goes: its output group, on the side this channel takes in its pair.
        let out = mix_outputs.get(mix).and_then(|id| sources.iter().position(|g| g.id == *id).map(|at| (at, &sources[at])));
        let beyond: Vec<String> = out
            .map(|(position, group)| {
                let side = if group.channels >= 2 { channel % 2 } else { 0 };
                reached([position as u8, side as u8]).into_iter().filter_map(|(g, c)| hardware_channel(g, c)).collect()
            })
            .unwrap_or_default();
        if beyond.is_empty() {
            add(&mut mixed, mix_channel(mixer, mix, at));
        }
        for name in beyond {
            add(&mut hardware, name);
        }
    }
    let places: Vec<String> = hardware.into_iter().chain(mixed).collect();
    let carries = places.first().map(|first| if places.len() > 1 { format!("{first} +{}", places.len() - 1) } else { first.clone() });
    // Not routed is said only once every group it could reach has been seen.
    let all_seen = destinations.iter().filter(|g| hardware_name(g).is_some() || mix_inputs.contains(&g.id)).all(|g| routing.contains_key(&g.id));
    Some(ChannelName { unrouted: carries.is_none() && all_seen, carries, usb })
}

/// A mix channel as a person names it: their name for the channel and the mix, where they gave them.
fn mix_channel(mixer: Option<&DeviceMixer>, mix: usize, slot: u32) -> String {
    let channel = mixer
        .and_then(|mixer| mixer.channels.iter().find(|one| one.slot == slot && !one.name.trim().is_empty()))
        .map_or_else(|| format!("Ch {}", slot + 1), |named| plain(&named.name));
    let named = mixer.and_then(|mixer| mixer.mixes.get(mix)?.name.clone()).map(|name| plain(&name)).filter(|name| !name.is_empty());
    format!("{channel} in {}", named.unwrap_or_else(|| format!("Mix {}", mix + 1)))
}

/// Every channel's automatic label for one entry, as `(inputs, outputs)` by channel from zero.
/// Nothing at all for an interface whose model is not known yet.
pub fn automatic_labels(device: &AggregateDevice, workspace: &Workspace) -> (BTreeMap<u32, String>, BTreeMap<u32, String>) {
    let known = device.known.as_ref();
    let Some(family) = known.and_then(|known| known.family.as_deref()) else { return Default::default() };
    let Some((inputs, outputs)) = channel_counts(family) else { return Default::default() };
    let mixer = device_of(device).and_then(|id| workspace.mixers.get(id));
    let empty = Routing::new();
    let routing = known.map_or(&empty, |known| &known.routing);
    let ins = (0..inputs).filter_map(|at| Some((at, input_channel(family, at, routing, mixer)?.label()))).collect();
    let outs = (0..outputs).filter_map(|at| Some((at, output_channel(family, at, routing, mixer)?.label()))).collect();
    (ins, outs)
}

/// The labels the driver is given for one entry: the automatic ones, with every name the person
/// typed put over them. A typed name always wins, and a channel the person cleared is back to its
/// automatic name, because the two are only put together here.
pub fn driver_labels(device: &AggregateDevice, workspace: &Workspace) -> (BTreeMap<u32, String>, BTreeMap<u32, String>) {
    let (mut inputs, mut outputs) = automatic_labels(device, workspace);
    for (labels, typed) in [(&mut inputs, &device.input_names), (&mut outputs, &device.output_names)] {
        for (&channel, name) in typed {
            let name = name.trim();
            if !name.is_empty() {
                labels.insert(channel, fit(name));
            }
        }
    }
    (inputs, outputs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::model::{AggregateKnown, MixConfig, MixerChannel};

    fn known(family: &str, id: &str) -> AggregateKnown {
        AggregateKnown { device_id: Some(DeviceId::from_serial(id)), family: Some(family.into()), model: family_words(family).map(str::to_string), routing: Routing::new() }
    }

    fn entry(key: &str, family: &str, id: &str) -> AggregateDevice {
        AggregateDevice { key: Some(key.into()), known: Some(known(family, id)), ..AggregateDevice::default() }
    }

    fn channel(name: &str, group: u32, channel: u32) -> MixerChannel {
        MixerChannel { id: format!("c{group}-{channel}"), name: name.into(), group: None, color: None, slot: 0, source: Some(RouteSource { group, channel }), main_mix: Some(0), sends: Vec::new() }
    }

    /// A source or destination group's position in a model's topology, by id.
    fn source(family: &str, id: &str) -> u8 {
        topology::source_groups(family).unwrap().iter().position(|g| g.id == id).unwrap() as u8
    }

    /// Every group an output could reach, routed to MUTE, so that nothing routed is known to be so.
    fn silent(family: &str) -> Routing {
        let mute = source(family, "MUTE0");
        naming_groups(family)
            .into_iter()
            .map(|(id, _)| {
                let channels = topology::destination_groups_whole(family).unwrap().iter().find(|g| g.id == id).unwrap().channels;
                (id, vec![[mute, 0]; channels as usize])
            })
            .collect()
    }

    fn route(routing: &mut Routing, group: &str, channel: usize, from: [u8; 2]) {
        routing.get_mut(group).expect("a group every output could reach")[channel] = from;
    }

    /// The measured fact everything here rests on: an aggregate channel is one of the interface's USB
    /// channels, and the counts are those groups' own, known without a DAW.
    #[test]
    fn an_interfaces_aggregate_channels_are_its_usb_record_and_playback_groups() {
        let quadro = usb_groups("quadro").expect("the Quadro's topology");
        assert_eq!((quadro.record.id.as_str(), quadro.record.name.as_str(), quadro.record.channels), ("COM_REC0", "USB A REC", 16));
        assert_eq!((quadro.playback.id.as_str(), quadro.playback.name.as_str(), quadro.playback.channels), ("COM_PLAY0", "USB 1 PLAY", 16));
        let studio = usb_groups("studio").expect("the Studio+'s topology");
        assert_eq!((studio.record.id.as_str(), studio.record.name.as_str(), studio.record.channels), ("USB_REC0", "USB REC", 24));
        assert_eq!((studio.playback.id.as_str(), studio.playback.name.as_str(), studio.playback.channels), ("USB_PLAY0", "USB PLAY", 24));
        assert_eq!(channel_counts("quadro"), Some((16, 16)), "not fourteen, which is what counting its inputs gave");
        assert_eq!(channel_counts("studio"), Some((24, 24)));
        assert_eq!(channel_counts("octo"), None);
        let none = Routing::new();
        assert_eq!(input_channel("quadro", 15, &none, None).unwrap().usb, "USB A REC 16");
        assert!(input_channel("quadro", 16, &none, None).is_none(), "there is no seventeenth");
        assert_eq!(output_channel("quadro", 15, &none, None).unwrap().usb, "USB 1 PLAY 16");
        assert_eq!(output_channel("studio", 23, &none, None).unwrap().usb, "USB PLAY 24");
    }

    #[test]
    fn the_groups_read_for_names_are_the_record_group_the_outputs_and_the_mix_inputs() {
        let ids: Vec<String> = naming_groups("quadro").into_iter().map(|(id, _)| id).collect();
        assert_eq!(ids, ["LINE_OUT0", "HEADPHONES0", "HEADPHONES1", "MONITOR0", "COM_REC0", "SPDIF_OUT0", "MIXER_IN0", "MIXER_IN1", "MIXER_IN2", "MIXER_IN3"]);
        let studio: Vec<String> = naming_groups("studio").into_iter().map(|(id, _)| id).collect();
        assert!(studio.contains(&"REAMP0".to_string()) && studio.contains(&"ADAT_OUT0".to_string()) && studio.contains(&"USB_REC0".to_string()));
        assert!(!studio.contains(&"TB_REC0".to_string()), "Thunderbolt is not what the aggregate uses");
        assert!(naming_groups("octo").is_empty());
    }

    #[test]
    fn an_interface_is_called_by_the_persons_name_for_the_device_else_its_model() {
        let mut workspace = Workspace::default();
        let studio = entry("ZenStudioTB ASIO Driver", "studio", "S");
        assert_eq!(gazelle_name(&studio, &workspace), "Zen Studio+", "the model, as the sidebar shows it");
        workspace.aliases.insert(DeviceId::from_serial("S"), "Drum room".into());
        assert_eq!(gazelle_name(&studio, &workspace), "Drum room", "and the person's own name where they gave one");
        let named = AggregateDevice { name: Some("Studio+".into()), ..studio.clone() };
        assert_eq!(gazelle_name(&named, &workspace), "Drum room", "a setup's own old name is not a name any more");
        let unknown = AggregateDevice { key: Some("ZenStudioTB ASIO Driver".into()), ..AggregateDevice::default() };
        assert_eq!(gazelle_name(&unknown, &Workspace::default()), "ZenStudioTB ASIO Driver");
    }

    #[test]
    fn two_interfaces_that_come_out_alike_are_told_apart() {
        assert_eq!(distinct(vec!["Zen Quadro Synergy Core".into(), "zen quadro synergy core".into(), "Zen Studio+".into()]), ["Zen Quadro Synergy Core", "zen quadro synergy core (2)", "Zen Studio+"]);
        let config = Aggregate { devices: vec![entry("A", "quadro", "Q1"), entry("B", "quadro", "Q2")], ..Aggregate::default() };
        assert_eq!(interface_names(&config, &Workspace::default()), ["Zen Quadro Synergy Core", "Zen Quadro Synergy Core (2)"]);
        assert_eq!(daw_names(&config, &Workspace::default()), ["Quadro", "Quadro 2"], "and in a DAW, by a count on the short form");
    }

    /// In a DAW, a device nobody has named goes by its model's short form, which leaves room for the
    /// driver's reference; a name the person gave the device replaces it.
    #[test]
    fn a_device_nobody_named_goes_by_its_models_short_form_in_a_daw() {
        let config = Aggregate { devices: vec![entry("A", "quadro", "Q"), entry("B", "studio", "S")], ..Aggregate::default() };
        let mut workspace = Workspace::default();
        assert_eq!(daw_names(&config, &workspace), ["Quadro", "Studio+"]);
        assert_eq!(interface_names(&config, &workspace), ["Zen Quadro Synergy Core", "Zen Studio+"], "the page keeps the full name");
        workspace.aliases.insert(DeviceId::from_serial("S"), "Drums".into());
        assert_eq!(daw_names(&config, &workspace), ["Quadro", "Drums"], "the person's own name replaces the short form");
        let unknown = Aggregate { devices: vec![AggregateDevice { key: Some("Other".into()), ..AggregateDevice::default() }], ..Aggregate::default() };
        assert_eq!(daw_names(&unknown, &Workspace::default()), ["Other"], "a model Gazelle does not know keeps what it has");
        assert_eq!(counted(vec!["A".into(), "a".into(), "A".into(), "B".into()]), ["A", "a 2", "A 3", "B"]);
    }

    #[test]
    fn a_source_is_named_as_the_routing_page_names_it_and_by_the_persons_own_names_first() {
        assert_eq!(source_name("quadro", 0, 0, None).as_deref(), Some("PREAMP 1"));
        assert_eq!(source_name("quadro", 5, 2, None).as_deref(), Some("AFX OUT 3"));
        assert_eq!(source_name("quadro", 10, 0, None), None, "MUTE is nothing");
        let mixer = DeviceMixer { mixes: vec![MixConfig { name: Some("Cue".into()), mono: None }], groups: Vec::new(), channels: vec![channel("", 0, 1), channel("Vocal mic", 0, 0)] };
        assert_eq!(source_name("quadro", 0, 0, Some(&mixer)).as_deref(), Some("Vocal mic"));
        assert_eq!(source_name("quadro", 0, 1, Some(&mixer)).as_deref(), Some("PREAMP 2"));
        assert_eq!(source_name("quadro", 6, 0, Some(&mixer)).as_deref(), Some("Cue L"));
    }

    #[test]
    fn an_input_is_named_for_what_routing_sends_it_then_by_its_record_channel() {
        let routing: Routing = [("COM_REC0".to_string(), vec![[0u8, 0u8], [10, 0], [5, 2]])].into_iter().collect();
        let mixer = DeviceMixer { channels: vec![channel("Vocal mic", 0, 0)], ..DeviceMixer::default() };
        let first = input_channel("quadro", 0, &routing, Some(&mixer)).unwrap();
        assert_eq!(first.text(), "Vocal mic, USB A REC 1");
        assert_eq!(first.label(), "Vocal mic");
        assert_eq!(input_channel("quadro", 1, &routing, Some(&mixer)).unwrap().text(), "USB A REC 2", "nothing routed is the record channel alone");
        assert_eq!(input_channel("quadro", 2, &routing, None).unwrap().text(), "AFX OUT 3, USB A REC 3");
        assert_eq!(input_channel("quadro", 0, &Routing::new(), None).unwrap().text(), "USB A REC 1", "nothing before the routing is read");
    }

    /// The owner's case: an output reaching Monitor L through Mix 1 is called Monitor L.
    #[test]
    fn an_output_reaching_a_hardware_output_through_a_mix_is_named_for_it() {
        let play = source("quadro", "COM_PLAY0");
        let mix1 = source("quadro", "MIXER_OUT0");
        let mut routing = silent("quadro");
        // USB 1 PLAY 1 and 2 into Mix 1's channels 7 and 8, and Mix 1 out to the monitors.
        route(&mut routing, "MIXER_IN0", 6, [play, 0]);
        route(&mut routing, "MIXER_IN0", 7, [play, 1]);
        route(&mut routing, "MONITOR0", 0, [mix1, 0]);
        route(&mut routing, "MONITOR0", 1, [mix1, 1]);
        let left = output_channel("quadro", 0, &routing, None).unwrap();
        assert_eq!(left.text(), "Monitor L, USB 1 PLAY 1");
        assert_eq!(left.label(), "Monitor L");
        assert_eq!(output_channel("quadro", 1, &routing, None).unwrap().text(), "Monitor R, USB 1 PLAY 2", "the second of a pair is the right side");
        // Straight to a socket, with no mix between.
        route(&mut routing, "LINE_OUT0", 1, [play, 4]);
        assert_eq!(output_channel("quadro", 4, &routing, None).unwrap().text(), "Line out R, USB 1 PLAY 5");
    }

    #[test]
    fn an_output_that_only_lands_in_a_mix_is_named_for_the_mix_channel_in_the_persons_words() {
        let play = source("quadro", "COM_PLAY0");
        let mut routing = silent("quadro");
        route(&mut routing, "MIXER_IN1", 9, [play, 2]);
        assert_eq!(output_channel("quadro", 2, &routing, None).unwrap().text(), "Ch 10 in Mix 2, USB 1 PLAY 3");
        let mixer = DeviceMixer {
            mixes: vec![MixConfig::default(), MixConfig { name: Some("Cue".into()), mono: None }],
            groups: Vec::new(),
            channels: vec![MixerChannel { slot: 9, ..channel("Click", play.into(), 2) }],
        };
        assert_eq!(output_channel("quadro", 2, &routing, Some(&mixer)).unwrap().text(), "Click in Cue, USB 1 PLAY 3");
    }

    #[test]
    fn an_output_routed_nowhere_says_so_once_everything_it_could_reach_has_been_seen() {
        let unrouted = output_channel("quadro", 4, &silent("quadro"), None).unwrap();
        assert_eq!(unrouted.text(), "USB 1 PLAY 5, not routed");
        assert_eq!(unrouted.label(), "Not routed");
        // With a group not seen yet, where it goes is not known rather than nowhere.
        let mut partial = silent("quadro");
        partial.remove("MIXER_IN3");
        let unknown = output_channel("quadro", 4, &partial, None).unwrap();
        assert_eq!(unknown.text(), "USB 1 PLAY 5");
        assert!(!unknown.unrouted);
    }

    #[test]
    fn an_output_reaching_several_places_is_the_first_hardware_output_and_a_count_of_the_rest() {
        let play = source("quadro", "COM_PLAY0");
        let mut routing = silent("quadro");
        route(&mut routing, "HEADPHONES0", 0, [play, 0]);
        route(&mut routing, "MONITOR0", 0, [play, 0]);
        assert_eq!(output_channel("quadro", 0, &routing, None).unwrap().text(), "HP1 L +1, USB 1 PLAY 1");
        route(&mut routing, "MIXER_IN2", 0, [play, 0]);
        assert_eq!(output_channel("quadro", 0, &routing, None).unwrap().label(), "HP1 L +2", "a mix channel going nowhere counts as one of the rest");
    }

    #[test]
    fn a_typed_name_wins_over_the_automatic_one_and_clearing_it_brings_the_automatic_one_back() {
        let mut device = entry("Zen Quadro Synergy Core", "quadro", "Q");
        device.known.as_mut().unwrap().routing = [("COM_REC0".to_string(), vec![[0, 0], [0, 1]])].into_iter().collect();
        let mut workspace = Workspace::default();
        workspace.mixers.insert(DeviceId::from_serial("Q"), DeviceMixer { channels: vec![channel("Vocal mic", 0, 0)], ..DeviceMixer::default() });
        let (inputs, outputs) = driver_labels(&device, &workspace);
        assert_eq!((inputs.len(), outputs.len()), (16, 16));
        assert_eq!(inputs[&0], "Vocal mic");
        assert_eq!(inputs[&1], "PREAMP 2");
        assert_eq!(inputs[&2], "USB A REC 3");
        device.input_names.insert(0, "Lead vocal".into());
        assert_eq!(driver_labels(&device, &workspace).0[&0], "Lead vocal");
        device.input_names.clear();
        assert_eq!(driver_labels(&device, &workspace).0[&0], "Vocal mic");
    }

    /// With the short form in the reference, every automatic name of both models fits the interface's
    /// 31 characters with its reference whole, however the channel is routed.
    #[test]
    fn every_automatic_name_of_both_models_fits_with_its_short_reference() {
        for family in ["quadro", "studio"] {
            let short = model_facts(family).unwrap().short;
            let groups = usb_groups(family).unwrap();
            let sources = topology::source_groups(family).unwrap();
            let destinations = topology::destination_groups_whole(family).unwrap();
            let mut labels: Vec<String> = vec!["Not routed".into()];
            // Every source any input could take, as the Routing page names it.
            for (at, group) in sources.iter().enumerate() {
                for channel in 0..group.channels {
                    labels.extend(source_name(family, at as u8, channel as u8, None));
                }
            }
            // Every hardware output and every mix channel, with the most a count could say.
            for group in destinations {
                for channel in 0..group.channels {
                    labels.extend(hardware_channel(group, channel).map(|name| format!("{name} +{}", groups.playback.channels)));
                }
            }
            for mix in 0..4 {
                labels.push(mix_channel(None, mix, 31));
            }
            for count in [1, groups.record.channels.max(groups.playback.channels)] {
                for tag in [short.to_string(), format!("{short} 2")] {
                    for label in &labels {
                        let whole = format!("{label} ({tag} {count})");
                        assert!(whole.chars().count() <= CHANNEL_NAME_MAX, "{whole} is {} characters", whole.chars().count());
                    }
                }
            }
        }
    }

    #[test]
    fn every_label_fits_what_the_interface_carries() {
        let long = "A Mixer channel with a name far longer than any interface carries";
        let mut device = entry("Zen Studio+", "studio", "S");
        device.known.as_mut().unwrap().routing = [("USB_REC0".to_string(), vec![[0, 0]])].into_iter().collect();
        let mut workspace = Workspace::default();
        workspace.mixers.insert(DeviceId::from_serial("S"), DeviceMixer { channels: vec![channel(long, 0, 0)], ..DeviceMixer::default() });
        let (inputs, outputs) = driver_labels(&device, &workspace);
        assert!(inputs.values().chain(outputs.values()).all(|label| label.chars().count() <= CHANNEL_NAME_MAX));
        assert_eq!(inputs[&0], fit(long));
    }

    #[test]
    fn an_interface_of_a_model_not_known_yet_gets_no_automatic_names() {
        let device = AggregateDevice { key: Some("Zen Quadro".into()), input_names: [(0, "Vocal mic".to_string())].into_iter().collect(), ..AggregateDevice::default() };
        let (inputs, outputs) = driver_labels(&device, &Workspace::default());
        assert_eq!(inputs, [(0, "Vocal mic".to_string())].into_iter().collect());
        assert!(outputs.is_empty());
    }
}
