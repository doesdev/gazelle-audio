//! What the aggregate calls its interfaces and their channels: Gazelle's own names, worked out in
//! one place, so the driver's file, the answer and every message say the same thing.
//!
//! **An interface is called by Gazelle's name for its device**: the person's own name for it where
//! they gave one, else its model, exactly what the sidebar shows. Renaming the device is the one way
//! to rename it in the aggregate, so there is no second name to keep in step.
//!
//! **An aggregate channel is one of the interface's USB audio channels**, not a preamp or an output.
//! Aggregate input *k* is the interface's USB record channel *k* (a routing destination), and
//! aggregate output *k* is its USB playback channel *k* (a routing source). That was measured at the
//! hardware channel by channel, and the vendor drivers' own counts are exactly those groups' counts:
//! 16 each way on the Zen Quadro Synergy Core, 24 each way on the Zen Studio+ ([`USB_GROUPS`]).
//!
//! So a channel is named for what it carries. An input is named first for what Gazelle's routing
//! sends to its record channel, in Gazelle's own words (the person's name for a Mixer channel or a
//! mix where they gave one, else the source as the Routing page shows it), then by the record
//! channel itself: "Vocal mic, USB A REC 1". Re-routing changes the name, which is the point: it
//! says what would be recorded. An output is the playback channel, named for the Mixer channel that
//! plays it where the person named one.
//!
//! The driver is given the short half of that, the label (`input_names` and `output_names` in its
//! file), and adds its own reference after it, "{interface name} {number}", inside the interface's
//! 31 characters. A name the person types for a channel wins over the automatic one, and the two are
//! never mixed up: only typed names are kept in the workspace, and the automatic ones are worked out
//! here whenever the file is written.

use std::collections::BTreeMap;

use crate::device::descriptor::DeviceId;
use crate::workspace::model::{Aggregate, AggregateDevice, DeviceMixer, RouteSource, Workspace, CHANNEL_NAME_MAX};
use crate::workspace::topology::{self, Group};

/// Which routing groups a model's aggregate channels are: `(family, record group, playback group)`
/// by topology id. The Studio+'s `TB_REC` and `TB_PLAY` are its Thunderbolt audio, which Gazelle
/// does not use; over USB its channels are `USB_REC` and `USB_PLAY`.
pub const USB_GROUPS: &[(&str, &str, &str)] = &[("quadro", "COM_REC0", "COM_PLAY0"), ("studio", "USB_REC0", "USB_PLAY0")];

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

/// How many channels an interface of this model has in the aggregate, as `(inputs, outputs)`.
/// Exact, and known without a DAW: it is the USB groups' own counts.
pub fn channel_counts(family: &str) -> Option<(u32, u32)> {
    usb_groups(family).map(|groups| (groups.record.channels, groups.playback.channels))
}

/// What to call a model when nothing better is known, as the rest of the app names it.
pub fn family_words(family: &str) -> Option<&'static str> {
    match family {
        "quadro" => Some("Zen Quadro Synergy Core"),
        "studio" => Some("Zen Studio+"),
        _ => None,
    }
}

/// The Gazelle device an entry is: the one the person chose, else the one last worked out.
pub fn device_of(device: &AggregateDevice) -> Option<&DeviceId> {
    device.device_id.as_ref().or_else(|| device.known.as_ref().and_then(|known| known.device_id.as_ref()))
}

/// Gazelle's name for the device one entry is, before two entries that come out the same are told
/// apart ([`interface_names`]).
///
/// The person's own name for the device, else its model, which is what the sidebar shows. With
/// neither known (an interface Gazelle has never been able to match), its model from the vendor
/// driver's entry where that was worked out, and only then the vendor driver's registry key.
pub fn gazelle_name(device: &AggregateDevice, workspace: &Workspace) -> String {
    let known = device.known.as_ref();
    let alias = device_of(device).and_then(|id| workspace.aliases.get(id)).map(|name| name.trim()).filter(|name| !name.is_empty());
    alias
        .map(str::to_string)
        .or_else(|| known.and_then(|known| known.model.clone()).filter(|model| !model.trim().is_empty()))
        .or_else(|| known.and_then(|known| known.family.as_deref()).and_then(family_words).map(str::to_string))
        .or_else(|| device.key.as_deref().map(str::trim).filter(|key| !key.is_empty()).map(str::to_string))
        .or_else(|| device.clsid.clone())
        .unwrap_or_else(|| "an interface with no key or class id".to_string())
}

/// Every entry's name in setup order, told apart.
///
/// Two interfaces of one model that nobody has named come out the same, and a driver given two
/// devices of one name would give their channels one name too, so the second and later ones have
/// their place in the setup after them. Naming the devices in Gazelle is the way out of it.
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
        let side = match channel {
            0 => "L".to_string(),
            1 => "R".to_string(),
            other => (u32::from(other) + 1).to_string(),
        };
        return Some(format!("{} {side}", plain(&name)));
    }
    Some(group_channel(group, u32::from(channel)))
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

/// A person's name made fit to go in a channel's name: one line, trimmed.
fn plain(text: &str) -> String {
    text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect::<String>().trim().to_string()
}

/// One channel's name, in its two parts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelName {
    /// What it carries: what routing sends to an input, or the Mixer channel that plays an output,
    /// in the person's words where they gave some. Nothing when nothing does, or when it is not known.
    pub carries: Option<String>,
    /// The USB channel it is: "USB A REC 1", "USB 1 PLAY 5".
    pub usb: String,
}

impl ChannelName {
    /// The whole name, as the page shows it: "Vocal mic, USB A REC 1", or the USB channel alone.
    pub fn text(&self) -> String {
        match &self.carries {
            Some(carries) => format!("{carries}, {}", self.usb),
            None => self.usb.clone(),
        }
    }

    /// The label the driver is given: what it carries, else the USB channel, cut to what the
    /// interface carries. Short on purpose, because the driver puts its own reference after it
    /// ([`CHANNEL_NAME_MAX`]).
    pub fn label(&self) -> String {
        fit(self.carries.as_deref().unwrap_or(&self.usb))
    }
}

/// As much of a label as the interface carries, in whole characters.
pub fn fit(text: &str) -> String {
    text.chars().take(CHANNEL_NAME_MAX).collect::<String>().trim_end().to_string()
}

/// Aggregate input `channel` of an interface of this model: its USB record channel, named first for
/// what routing sends to it. `record_routing` is that group's slots as `[source, channel]`, or
/// nothing while the routing is not known, which names the channel by its USB channel alone.
pub fn input_channel(family: &str, channel: u32, record_routing: Option<&[[u8; 2]]>, mixer: Option<&DeviceMixer>) -> Option<ChannelName> {
    let groups = usb_groups(family)?;
    if channel >= groups.record.channels {
        return None;
    }
    let slot = record_routing.and_then(|slots| slots.get(channel as usize));
    let carries = slot.and_then(|[source, from]| source_name(family, *source, *from, mixer));
    Some(ChannelName { carries, usb: group_channel(&groups.record, channel) })
}

/// Aggregate output `channel`: its USB playback channel, which is a routing source, named for the
/// Mixer channel that plays it where the person named one.
pub fn output_channel(family: &str, channel: u32, mixer: Option<&DeviceMixer>) -> Option<ChannelName> {
    let groups = usb_groups(family)?;
    if channel >= groups.playback.channels {
        return None;
    }
    let wanted = RouteSource { group: groups.playback_position, channel };
    let carries = mixer
        .and_then(|mixer| mixer.channels.iter().find(|one| one.source == Some(wanted) && !one.name.trim().is_empty()))
        .map(|named| plain(&named.name));
    Some(ChannelName { carries, usb: group_channel(&groups.playback, channel) })
}

/// Every channel's automatic label for one entry, as `(inputs, outputs)` by channel from zero.
/// Nothing at all for an interface whose model is not known yet.
pub fn automatic_labels(device: &AggregateDevice, workspace: &Workspace) -> (BTreeMap<u32, String>, BTreeMap<u32, String>) {
    let known = device.known.as_ref();
    let Some(family) = known.and_then(|known| known.family.as_deref()) else { return Default::default() };
    let Some((inputs, outputs)) = channel_counts(family) else { return Default::default() };
    let mixer = device_of(device).and_then(|id| workspace.mixers.get(id));
    let routing = known.and_then(|known| known.record_routing.as_deref());
    let ins = (0..inputs).filter_map(|at| Some((at, input_channel(family, at, routing, mixer)?.label()))).collect();
    let outs = (0..outputs).filter_map(|at| Some((at, output_channel(family, at, mixer)?.label()))).collect();
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
        AggregateKnown { device_id: Some(DeviceId::from_serial(id)), family: Some(family.into()), model: family_words(family).map(str::to_string), record_routing: None }
    }

    fn entry(key: &str, family: &str, id: &str) -> AggregateDevice {
        AggregateDevice { key: Some(key.into()), known: Some(known(family, id)), ..AggregateDevice::default() }
    }

    fn channel(name: &str, group: u32, channel: u32) -> MixerChannel {
        MixerChannel { id: format!("c{group}-{channel}"), name: name.into(), group: None, color: None, slot: 0, source: Some(RouteSource { group, channel }), main_mix: Some(0), sends: Vec::new() }
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
        // Channel k is slot k, both ways round.
        assert_eq!(input_channel("quadro", 0, None, None).unwrap().usb, "USB A REC 1");
        assert_eq!(input_channel("quadro", 15, None, None).unwrap().usb, "USB A REC 16");
        assert!(input_channel("quadro", 16, None, None).is_none(), "there is no seventeenth");
        assert_eq!(output_channel("quadro", 15, None).unwrap().usb, "USB 1 PLAY 16");
        assert_eq!(output_channel("studio", 23, None).unwrap().usb, "USB PLAY 24");
        assert_eq!(input_channel("studio", 7, None, None).unwrap().usb, "USB REC 8");
    }

    #[test]
    fn an_interface_is_called_by_the_persons_name_for_the_device_else_its_model() {
        let mut workspace = Workspace::default();
        let studio = entry("ZenStudioTB ASIO Driver", "studio", "S");
        assert_eq!(gazelle_name(&studio, &workspace), "Zen Studio+", "the model, as the sidebar shows it");
        workspace.aliases.insert(DeviceId::from_serial("S"), "Drum room".into());
        assert_eq!(gazelle_name(&studio, &workspace), "Drum room", "and the person's own name where they gave one");
        // A setup's own old name is not a name anything is called by any more.
        let named = AggregateDevice { name: Some("Studio+".into()), ..studio.clone() };
        assert_eq!(gazelle_name(&named, &workspace), "Drum room");
        // The device the person chose wins over one worked out before it.
        let chosen = AggregateDevice { device_id: Some(DeviceId::from_serial("Q")), ..studio.clone() };
        workspace.aliases.insert(DeviceId::from_serial("Q"), "Desk".into());
        assert_eq!(gazelle_name(&chosen, &workspace), "Desk");
        // With nothing known about the device, the vendor driver's key is all there is.
        let unknown = AggregateDevice { key: Some("ZenStudioTB ASIO Driver".into()), ..AggregateDevice::default() };
        assert_eq!(gazelle_name(&unknown, &Workspace::default()), "ZenStudioTB ASIO Driver");
    }

    #[test]
    fn two_interfaces_that_come_out_alike_are_told_apart_by_their_place() {
        assert_eq!(distinct(vec!["Zen Quadro Synergy Core".into(), "zen quadro synergy core".into(), "Zen Studio+".into()]), ["Zen Quadro Synergy Core", "zen quadro synergy core (2)", "Zen Studio+"]);
        let config = Aggregate { devices: vec![entry("A", "quadro", "Q1"), entry("B", "quadro", "Q2")], ..Aggregate::default() };
        assert_eq!(interface_names(&config, &Workspace::default()), ["Zen Quadro Synergy Core", "Zen Quadro Synergy Core (2)"]);
    }

    #[test]
    fn a_source_is_named_as_the_routing_page_names_it_and_by_the_persons_own_names_first() {
        // Quadro sources: PREAMP 0, USB 1 PLAY 1, ..., AFX OUT 5, MIXER_OUT0 6, MUTE 10.
        assert_eq!(source_name("quadro", 0, 0, None).as_deref(), Some("PREAMP 1"));
        assert_eq!(source_name("quadro", 5, 2, None).as_deref(), Some("AFX OUT 3"));
        assert_eq!(source_name("quadro", 1, 4, None).as_deref(), Some("USB 1 PLAY 5"));
        assert_eq!(source_name("quadro", 10, 0, None), None, "MUTE is nothing");
        assert_eq!(source_name("quadro", 99, 0, None), None, "and so is a source the model does not have");
        let mixer = DeviceMixer {
            mixes: vec![MixConfig { name: Some("Cue".into()), mono: None }],
            groups: Vec::new(),
            channels: vec![channel("", 0, 1), channel("Vocal mic", 0, 0)],
        };
        assert_eq!(source_name("quadro", 0, 0, Some(&mixer)).as_deref(), Some("Vocal mic"), "a Mixer channel the person named");
        assert_eq!(source_name("quadro", 0, 1, Some(&mixer)).as_deref(), Some("PREAMP 2"), "one they did not name is the source itself");
        assert_eq!(source_name("quadro", 6, 0, Some(&mixer)).as_deref(), Some("Cue L"), "a mix they named");
        assert_eq!(source_name("quadro", 7, 1, Some(&mixer)).as_deref(), Some("LOOPBACK HP2 2"), "and one they did not");
    }

    #[test]
    fn an_input_is_named_for_what_routing_sends_it_then_by_its_record_channel() {
        let routing = [[0u8, 0u8], [10, 0], [5, 2]];
        let mixer = DeviceMixer { channels: vec![channel("Vocal mic", 0, 0)], ..DeviceMixer::default() };
        let first = input_channel("quadro", 0, Some(&routing), Some(&mixer)).unwrap();
        assert_eq!(first.text(), "Vocal mic, USB A REC 1");
        assert_eq!(first.label(), "Vocal mic", "the driver adds its own reference after the label");
        let muted = input_channel("quadro", 1, Some(&routing), Some(&mixer)).unwrap();
        assert_eq!(muted.text(), "USB A REC 2", "nothing routed is the record channel alone");
        assert_eq!(muted.label(), "USB A REC 2");
        assert_eq!(input_channel("quadro", 2, Some(&routing), None).unwrap().text(), "AFX OUT 3, USB A REC 3");
        assert_eq!(input_channel("quadro", 3, Some(&routing), None).unwrap().text(), "USB A REC 4", "past what was read is not known");
        assert_eq!(input_channel("quadro", 0, None, None).unwrap().text(), "USB A REC 1", "and neither is anything before the routing is read");
    }

    #[test]
    fn an_output_is_its_playback_channel_named_for_the_mixer_channel_that_plays_it() {
        let mixer = DeviceMixer { channels: vec![channel("Click", 1, 0), channel("", 1, 1)], ..DeviceMixer::default() };
        assert_eq!(output_channel("quadro", 0, Some(&mixer)).unwrap().text(), "Click, USB 1 PLAY 1");
        assert_eq!(output_channel("quadro", 1, Some(&mixer)).unwrap().text(), "USB 1 PLAY 2", "an unnamed Mixer channel is not a name");
        assert_eq!(output_channel("quadro", 4, None).unwrap().label(), "USB 1 PLAY 5");
        // The Studio+'s playback group is its fourth source.
        let studio = DeviceMixer { channels: vec![channel("Reverb return", 3, 2)], ..DeviceMixer::default() };
        assert_eq!(output_channel("studio", 2, Some(&studio)).unwrap().text(), "Reverb return, USB PLAY 3");
    }

    #[test]
    fn a_typed_name_wins_over_the_automatic_one_and_clearing_it_brings_the_automatic_one_back() {
        let mut device = entry("Zen Quadro Synergy Core", "quadro", "Q");
        device.known.as_mut().unwrap().record_routing = Some(vec![[0, 0], [0, 1]]);
        let mut workspace = Workspace::default();
        workspace.mixers.insert(DeviceId::from_serial("Q"), DeviceMixer { channels: vec![channel("Vocal mic", 0, 0)], ..DeviceMixer::default() });
        let (inputs, outputs) = driver_labels(&device, &workspace);
        assert_eq!(inputs.len(), 16, "every channel the interface has");
        assert_eq!(outputs.len(), 16);
        assert_eq!(inputs[&0], "Vocal mic", "the person's name for the Mixer channel");
        assert_eq!(inputs[&1], "PREAMP 2", "the source, where they gave none");
        assert_eq!(inputs[&2], "USB A REC 3");

        device.input_names.insert(0, "Lead vocal".into());
        assert_eq!(driver_labels(&device, &workspace).0[&0], "Lead vocal", "a typed name wins over both");
        device.input_names.clear();
        assert_eq!(driver_labels(&device, &workspace).0[&0], "Vocal mic", "and clearing it brings the automatic one back");
    }

    #[test]
    fn every_label_fits_what_the_interface_carries() {
        let long = "A Mixer channel with a name far longer than any interface carries";
        let mut device = entry("Zen Studio+", "studio", "S");
        device.known.as_mut().unwrap().record_routing = Some(vec![[0, 0]]);
        let mut workspace = Workspace::default();
        workspace.mixers.insert(DeviceId::from_serial("S"), DeviceMixer { channels: vec![channel(long, 0, 0), channel("Tab\there", 3, 0)], ..DeviceMixer::default() });
        let (inputs, outputs) = driver_labels(&device, &workspace);
        assert!(inputs.values().chain(outputs.values()).all(|label| label.chars().count() <= CHANNEL_NAME_MAX), "{inputs:?} {outputs:?}");
        assert_eq!(inputs[&0], fit(long));
        assert_eq!(outputs[&0], "Tab here", "no control character reaches the driver");
    }

    #[test]
    fn an_interface_of_a_model_not_known_yet_gets_no_automatic_names() {
        let device = AggregateDevice { key: Some("Zen Quadro".into()), input_names: [(0, "Vocal mic".to_string())].into_iter().collect(), ..AggregateDevice::default() };
        let (inputs, outputs) = driver_labels(&device, &Workspace::default());
        assert_eq!(inputs, [(0, "Vocal mic".to_string())].into_iter().collect(), "only what the person typed");
        assert!(outputs.is_empty());
    }
}
