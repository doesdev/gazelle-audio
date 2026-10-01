//! Whether the path the driver measures a follower's phase over is there in the interfaces' own
//! routing, and when it is not, what is wrong, in words, and how to put it back.
//!
//! The driver writes a short burst (four samples, about -42 dBFS) into the callback master's USB
//! playback channel named by a follower's `phase.master_output` at the start of every session, and
//! listens for it on the follower's USB record channel named by `phase.input`. It never routes
//! anything: whether the burst reaches the cable, and only the cable, is the interfaces' routing. On
//! the owner's PC it did not. The playback channel had been put into two mixes for a headphone amp,
//! and the S/PDIF output it should have left on was muted, so the burst went towards the monitors,
//! the measurement heard nothing, and nothing said so. This says so.
//!
//! Three things make the path, all read from what Gazelle last knew of each interface's routing
//! (`AggregateKnown::routing`, kept current by the server whenever routing is read or written):
//!
//! - the master's digital output, on one of the cable's channels, plays that playback channel directly;
//! - that playback channel goes nowhere else, neither into a mix nor to another output, since the burst
//!   plays wherever it goes;
//! - the follower's record channel records the matching channel of its digital input.
//!
//! A part whose routing Gazelle has not seen is not judged. A cable the person has dedicated to the
//! phase measurement ([`crate::workspace::model::Cable::dedicated`]) gets a fix that puts every part
//! back; one that is not dedicated gets the reason alone, since putting it back would take the
//! channel out of mixes somebody chose to put it in, and dedicating a cable is the way to give the
//! measurement channels of its own.

use serde_json::json;

use super::naming::{group_channel, hardware_channel, source_name, usb_groups, Routing};
use super::readiness::{Fix, Reason, ReasonCode, Severity};
use super::DeviceReport;
use crate::device::descriptor::DeviceId;
use crate::workspace::model::{AggregateDevice, Cable, DeviceMixer, Workspace};
use crate::workspace::topology::{self, Group};

/// One routing write a fix makes: one destination channel of one device, and the source it takes,
/// `[source group position, channel]`, or MUTE for none. The page makes each through its routing
/// model, which reads the group before it writes it, so a fix never undoes a change made elsewhere
/// to the group's other channels.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Write {
    pub device_id: DeviceId,
    /// The destination group's wire position, which `set_routing` takes as its `bank_idx`.
    pub destination: u32,
    pub channel: u32,
    pub source: Option<[u8; 2]>,
}

/// What a fix that puts a phase path back is called, so the page can recognise it.
pub const RESTORE_KIND: &str = "restore_phase_path";

/// Every reason about a phase path: one per follower whose path is broken, and one per dedicated
/// cable whose dedication no longer means anything.
pub fn reasons(devices: &[DeviceReport], workspace: &Workspace) -> Vec<Reason> {
    let mut reasons = Vec::new();
    if let Some(master) = devices.iter().find(|device| device.is_master) {
        for follower in devices.iter().filter(|device| !device.is_master) {
            reasons.extend(broken(master, follower, workspace));
        }
    }
    reasons.extend(stale(devices, workspace));
    reasons
}

/// The cable a follower's phase is measured over: a dedicated one from the master first, else the
/// first cable declared from the master into it.
pub fn cable_between<'a>(cables: &'a [Cable], master: &DeviceId, follower: &DeviceId) -> Option<&'a Cable> {
    let between = || cables.iter().filter(move |cable| &cable.from.device_id == master && &cable.to.device_id == follower);
    between().find(|cable| cable.dedicated.is_some()).or_else(|| between().next())
}

/// A device's model: what the report says, else what was last known of the entry.
fn family<'a>(report: &'a DeviceReport, entry: &'a AggregateDevice) -> Option<&'a str> {
    report.family.as_deref().or_else(|| entry.known.as_ref().and_then(|known| known.family.as_deref()))
}

fn routing_of(entry: &AggregateDevice) -> Option<&Routing> {
    entry.known.as_ref().map(|known| &known.routing)
}

/// "S/PDIF" or "ADAT", for a cable's port.
fn port_words(port: &str) -> &'static str {
    if port.starts_with("ADAT") {
        "ADAT"
    } else {
        "S/PDIF"
    }
}

/// A channel of a stereo pair as L or R, and of anything else as its number from one.
fn side(channel: u32, channels: u32) -> String {
    match (channels, channel) {
        (2, 0) => "L".into(),
        (2, 1) => "R".into(),
        _ => (channel + 1).to_string(),
    }
}

/// One channel of a digital input as a person names it: "S/PDIF In L", "ADAT In 3".
fn input_words(group: &Group, channel: u32) -> String {
    format!("{} {}", group.display_name(), side(channel, group.channels))
}

/// A mix as a person names it: their name for it, else "Mix 1".
fn mix_name(mixer: Option<&DeviceMixer>, mix: usize) -> String {
    mixer
        .and_then(|mixer| mixer.mixes.get(mix)?.name.clone())
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| format!("Mix {}", mix + 1))
}

/// What a routing slot plays, in the person's words: a mix's side by the mix's name ("Mix 1 L"),
/// else the source as every page names it ("AFX Out 3", "Preamp 1"). MUTE, or a slot not
/// known, is nothing.
fn slot_words(family: &str, slot: Option<&[u8; 2]>, mixer: Option<&DeviceMixer>) -> Option<String> {
    let [source, channel] = *slot?;
    let group = topology::source_groups(family)?.get(usize::from(source))?;
    if let Some(mix) = topology::mix_outputs(family).and_then(|outputs| outputs.iter().position(|id| *id == group.id)) {
        return Some(format!("{} {}", mix_name(mixer, mix), side(u32::from(channel), group.channels)));
    }
    source_name(family, source, channel, mixer)
}

/// Where a destination channel is, as a person names it: a mix by its name ("Mix 1"), a hardware
/// output by its socket ("Monitor L"), anything else as the Routing page shows it.
fn place_words(family: &str, group: &Group, channel: u32, mixer: Option<&DeviceMixer>) -> String {
    if let Some(mix) = topology::mix_inputs(family).and_then(|inputs| inputs.iter().position(|id| *id == group.id)) {
        return mix_name(mixer, mix);
    }
    hardware_channel(group, channel).unwrap_or_else(|| group_channel(group, channel))
}

/// "A", "A and B", "A, B and C".
fn listed(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// The reason one follower's phase path is broken, or nothing when it is whole or cannot be judged.
fn broken(master: &DeviceReport, follower: &DeviceReport, workspace: &Workspace) -> Option<Reason> {
    let config = workspace.aggregate.as_ref()?;
    let entry = config.devices.get(follower.index)?;
    let phase = entry.phase?;
    let (output, input) = (phase.master_output?, phase.input?);
    let master_entry = config.devices.get(master.index)?;
    let (master_id, follower_id) = (master.device_id.as_ref()?, follower.device_id.as_ref()?);
    let cable = cable_between(&workspace.cables, master_id, follower_id)?;
    let (master_family, follower_family) = (family(master, master_entry)?, family(follower, entry)?);
    let master_mixer = workspace.mixers.get(master_id);
    let follower_mixer = workspace.mixers.get(follower_id);

    let play = usb_groups(master_family)?;
    if output >= play.playback.channels {
        return None;
    }
    let destinations = topology::destination_groups_whole(master_family)?;
    let (out_at, out_group) = destinations.iter().enumerate().find(|(_, group)| group.kind == cable.from.port)?;
    let record = usb_groups(follower_family)?;
    let sources = topology::source_groups(follower_family)?;
    let in_at = sources.iter().position(|group| group.kind == cable.to.port)?;
    let playback = [u8::try_from(play.playback_position).ok()?, u8::try_from(output).ok()?];
    let usb_out = group_channel(&play.playback, output);
    let dedicated = cable.dedicated.is_some_and(|d| d.phase_output == output && d.phase_input == input);
    let empty = Routing::new();
    let master_routing = routing_of(master_entry).unwrap_or(&empty);
    let follower_routing = routing_of(entry).unwrap_or(&empty);

    let mut problems: Vec<String> = Vec::new();
    let mut writes: Vec<Write> = Vec::new();
    let mut path_broken = false;

    // The master: which of the cable's channels plays the playback channel, directly.
    let out_slots = master_routing.get(&out_group.id);
    let carrying = out_slots.and_then(|slots| (0..cable.channels).find(|lane| slots.get((cable.from.first + lane) as usize) == Some(&playback)));
    if let (Some(slots), None) = (out_slots, carrying) {
        let socket = format!("{}'s {}", master.name, hardware_channel(out_group, cable.from.first).unwrap_or_else(|| group_channel(out_group, cable.from.first)));
        let now = slot_words(master_family, slots.get(cable.from.first as usize), master_mixer);
        problems.push(match (dedicated, now) {
            (true, Some(now)) => format!("{socket} no longer plays {usb_out}: it plays {now} now"),
            (true, None) => format!("{socket} no longer plays {usb_out}: it is muted now"),
            (false, Some(now)) => format!("{socket} plays {now}, not {usb_out}"),
            (false, None) => format!("{socket} is muted, not {usb_out}"),
        });
        writes.push(Write { device_id: master_id.clone(), destination: out_at as u32, channel: cable.from.first, source: Some(playback) });
        path_broken = true;
    }
    let lane = carrying.unwrap_or(0);

    // Everywhere else the playback channel goes, which is everywhere the burst goes.
    let mut elsewhere: Vec<String> = Vec::new();
    for (at, group) in destinations.iter().enumerate() {
        let Some(slots) = master_routing.get(&group.id) else { continue };
        for channel in 0..group.channels {
            if slots.get(channel as usize) != Some(&playback) || (carrying.is_some() && at == out_at && channel == cable.from.first + lane) {
                continue;
            }
            let place = place_words(master_family, group, channel, master_mixer);
            if !elsewhere.contains(&place) {
                elsewhere.push(place);
            }
            writes.push(Write { device_id: master_id.clone(), destination: at as u32, channel, source: None });
        }
    }
    if !elsewhere.is_empty() {
        problems.push(format!(
            "{usb_out} also goes to {}, and the short burst the driver plays into it at the start of every session plays wherever it goes",
            listed(&elsewhere)
        ));
    }

    // The follower: its record channel records the matching channel of its digital input.
    let wanted = [u8::try_from(in_at).ok()?, u8::try_from(cable.to.first + lane).ok()?];
    if let Some(slots) = follower_routing.get(&record.record.id) {
        let slot = slots.get(input as usize);
        if input < record.record.channels && slot != Some(&wanted) {
            let channel = format!("{}'s {}", follower.name, group_channel(&record.record, input));
            let arriving = input_words(&sources[in_at], cable.to.first + lane);
            let now = slot_words(follower_family, slot, follower_mixer);
            problems.push(match (dedicated, now) {
                (true, Some(now)) => format!("{channel} no longer records {arriving}: it records {now} now"),
                (true, None) => format!("{channel} no longer records {arriving}: it records nothing now"),
                (false, Some(now)) => format!("{channel} records {now}, not {arriving}"),
                (false, None) => format!("{channel} records nothing, not {arriving}"),
            });
            writes.push(Write { device_id: follower_id.clone(), destination: record.record_position, channel: input, source: Some(wanted) });
            path_broken = true;
        }
    }

    if problems.is_empty() {
        return None;
    }
    let port = port_words(&cable.from.port);
    let mut message = format!("The phase path over the {port} cable is broken: {}.", problems.join("; "));
    if path_broken {
        message.push_str(" Until it is back the measurement hears nothing, and each session is lined up by the figures the drivers report.");
    }
    if dedicated {
        message.push_str(&format!(" The {port} cable is dedicated to it, so it can be put back from here."));
    } else {
        message.push_str(&format!(" Dedicating the {port} cable, on the Workspace page, gives the measurement channels of its own."));
    }
    let reason = Reason::new(ReasonCode::PhasePathBroken, Severity::Warning, message).about(follower);
    Some(if dedicated { reason.with(restore_fix(&writes)) } else { reason })
}

/// The fix that puts a dedicated path back: every write, in order, each one channel of one group.
fn restore_fix(writes: &[Write]) -> Fix {
    let writes: Vec<serde_json::Value> = writes
        .iter()
        .map(|write| json!({ "device_id": write.device_id, "destination": write.destination, "channel": write.channel, "source": write.source }))
        .collect();
    Fix { kind: RESTORE_KIND, method: "PUT", route: "routing".into(), body: json!({ "writes": writes }), label: "Put the phase path back".into() }
}

/// Dedicated cables that no longer mean anything: one of its devices has left the aggregate, the
/// sending one does not drive the callback now, or the follower's phase setup names other channels.
/// The routing stays as it is and nothing guards it, which is worth knowing; the dedication is the
/// person's to turn off, on the Workspace page.
fn stale(devices: &[DeviceReport], workspace: &Workspace) -> Vec<Reason> {
    let mut reasons = Vec::new();
    let config = workspace.aggregate.as_ref();
    for cable in workspace.cables.iter().filter(|cable| cable.dedicated.is_some()) {
        let Some(dedication) = cable.dedicated else { continue };
        let report = |id: &DeviceId| devices.iter().find(|device| device.device_id.as_ref() == Some(id));
        let (from, to) = (report(&cable.from.device_id), report(&cable.to.device_id));
        let name = |id: &DeviceId, found: Option<&DeviceReport>| {
            found.map(|device| device.name.clone()).or_else(|| workspace.aliases.get(id).map(|alias| alias.trim().to_string()).filter(|alias| !alias.is_empty())).unwrap_or_else(|| id.to_string())
        };
        let (sender, receiver) = (name(&cable.from.device_id, from), name(&cable.to.device_id, to));
        let why = match (from, to) {
            (None, _) => format!("{sender} is not in the aggregate now"),
            (_, None) => format!("{receiver} is not in the aggregate now"),
            (Some(from), _) if !from.is_master => format!("{sender} does not drive the callback now, and a phase is measured from the interface that does"),
            (_, Some(to)) => {
                let phase = config.and_then(|config| config.devices.get(to.index)).and_then(|entry| entry.phase);
                let same = phase.is_some_and(|phase| phase.master_output == Some(dedication.phase_output) && phase.input == Some(dedication.phase_input));
                if same {
                    continue;
                }
                if phase.is_none() {
                    format!("{receiver} has no phase setup now")
                } else {
                    format!("{receiver}'s phase setup names other channels now")
                }
            }
        };
        let port = port_words(&cable.from.port);
        let reason = Reason::new(
            ReasonCode::PhaseDedicationStale,
            Severity::Warning,
            format!(
                "The {port} cable from {sender} to {receiver} is dedicated to the phase measurement, and {why}, so the dedication means nothing: its routing stays as it is and nothing guards it. Turn it off on the Workspace page to release its channels, or dedicate the cable again."
            ),
        );
        reasons.push(match to.or(from) {
            Some(device) => reason.about(device),
            None => reason,
        });
    }
    reasons
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aggregate::{DriverSummary, MatchedBy};
    use crate::workspace::model::{Aggregate, AggregateKnown, AggregatePhase, CableDedication, CableEnd, MixConfig};

    const QUADRO: &str = "Q";
    const STUDIO: &str = "S";
    // Quadro sources: USB 1 PLAY 1, MIXER_OUT0 6, MUTE 10. Destinations: SPDIF_OUT0 6, MIXER_IN0..3 8..11.
    const COM_PLAY: u8 = 1;
    const MIX1_OUT: u8 = 6;
    const Q_MUTE: u8 = 10;
    // Studio+ sources: PREAMP 0, SPDIF_IN 5, MUTE 11. Destinations: USB_REC0 6.
    const SPDIF_IN: u8 = 5;
    const S_MUTE: u8 = 11;

    fn report(index: usize, name: &str, serial: &str, family: &str, is_master: bool) -> DeviceReport {
        DeviceReport {
            index,
            name: name.into(),
            daw_name: name.into(),
            key: Some(name.into()),
            clsid: None,
            registered: true,
            entry_key: Some(name.into()),
            device_id: Some(DeviceId::from_serial(serial)),
            attached: true,
            matched_by: MatchedBy::Chosen,
            match_note: None,
            channels: None,
            family: Some(family.into()),
            clock: None,
            driver: DriverSummary::default(),
            controller: None,
            controller_error: None,
            is_master,
            phase_configured: false,
        }
    }

    fn devices() -> Vec<DeviceReport> {
        vec![report(0, "Quadro", QUADRO, "quadro", true), report(1, "Studio+", STUDIO, "studio", false)]
    }

    /// Every group the names come from, routed from MUTE.
    fn silent(family: &str) -> Routing {
        let mute = if family == "quadro" { Q_MUTE } else { S_MUTE };
        super::super::naming::naming_groups(family)
            .into_iter()
            .map(|(id, _)| {
                let channels = topology::destination_groups_whole(family).unwrap().iter().find(|group| group.id == id).unwrap().channels;
                (id, vec![[mute, 0]; channels as usize])
            })
            .collect()
    }

    struct Rig {
        quadro: Routing,
        studio: Routing,
        phase: Option<AggregatePhase>,
        dedicated: Option<CableDedication>,
    }

    impl Rig {
        fn workspace(&self) -> Workspace {
            let entry = |key: &str, serial: &str, family: &str, routing: &Routing| AggregateDevice {
                key: Some(key.into()),
                device_id: Some(DeviceId::from_serial(serial)),
                known: Some(AggregateKnown { device_id: Some(DeviceId::from_serial(serial)), family: Some(family.into()), model: None, routing: routing.clone(), rate: None }),
                ..AggregateDevice::default()
            };
            let mut studio = entry("Studio+", STUDIO, "studio", &self.studio);
            studio.phase = self.phase;
            Workspace {
                aggregate: Some(Aggregate { devices: vec![entry("Quadro", QUADRO, "quadro", &self.quadro), studio], ..Aggregate::default() }),
                cables: vec![Cable {
                    id: "c1".into(),
                    from: CableEnd { device_id: DeviceId::from_serial(QUADRO), port: "SPDIF_OUT".into(), first: 0 },
                    to: CableEnd { device_id: DeviceId::from_serial(STUDIO), port: "SPDIF_IN".into(), first: 0 },
                    channels: 2,
                    dedicated: self.dedicated,
                }],
                ..Workspace::default()
            }
        }

        /// USB 1 Play 16 straight to S/PDIF Out L, and nowhere else; the Studio+ records S/PDIF In L on USB Rec 24.
        fn dedicated() -> Rig {
            let mut quadro = silent("quadro");
            quadro.get_mut("SPDIF_OUT0").unwrap()[0] = [COM_PLAY, 15];
            let mut studio = silent("studio");
            studio.get_mut("USB_REC0").unwrap()[23] = [SPDIF_IN, 0];
            Rig {
                quadro,
                studio,
                phase: Some(AggregatePhase { master_output: Some(15), input: Some(23), reference: None }),
                dedicated: Some(CableDedication { phase_output: 15, phase_input: 23 }),
            }
        }

        /// The owner's PC: USB 1 Play 3 in Mix 1 and Mix 4 for a headphone amp, S/PDIF Out muted, and the
        /// Studio+ still recording S/PDIF In L on USB Rec 21. Nothing is dedicated.
        fn owners() -> Rig {
            let mut quadro = silent("quadro");
            quadro.get_mut("MIXER_IN0").unwrap()[4] = [COM_PLAY, 2];
            quadro.get_mut("MIXER_IN3").unwrap()[9] = [COM_PLAY, 2];
            quadro.get_mut("HEADPHONES0").unwrap()[0] = [MIX1_OUT, 0];
            let mut studio = silent("studio");
            studio.get_mut("USB_REC0").unwrap()[20] = [SPDIF_IN, 0];
            Rig { quadro, studio, phase: Some(AggregatePhase { master_output: Some(2), input: Some(20), reference: Some(-37) }), dedicated: None }
        }
    }

    fn only(reasons: Vec<Reason>) -> Reason {
        assert_eq!(reasons.len(), 1, "{reasons:#?}");
        reasons.into_iter().next().unwrap()
    }

    fn writes(reason: &Reason) -> serde_json::Value {
        reason.fix.as_ref().expect("a fix").body["writes"].clone()
    }

    #[test]
    fn a_whole_dedicated_path_says_nothing() {
        assert!(reasons(&devices(), &Rig::dedicated().workspace()).is_empty());
    }

    /// The owner's case, exactly: the burst goes into the two mixes and the S/PDIF output is muted.
    #[test]
    fn the_owners_phase_channel_in_two_mixes_with_the_spdif_output_muted_is_said_plainly_and_offers_no_fix() {
        let reason = only(reasons(&devices(), &Rig::owners().workspace()));
        assert_eq!(reason.code, ReasonCode::PhasePathBroken);
        assert_eq!(reason.severity, Severity::Warning);
        assert_eq!(reason.device.as_deref(), Some("Studio+"));
        assert_eq!(reason.device_index, Some(1));
        assert_eq!(
            reason.message,
            "The phase path over the S/PDIF cable is broken: Quadro's S/PDIF Out L is muted, not USB 1 Play 3; USB 1 Play 3 also goes to Mix 1 and Mix 4, and the short burst the driver plays into it at the start of every session plays wherever it goes. Until it is back the measurement hears nothing, and each session is lined up by the figures the drivers report. Dedicating the S/PDIF cable, on the Workspace page, gives the measurement channels of its own."
        );
        assert!(reason.fix.is_none(), "taking the channel out of mixes somebody chose is not a fix to offer");
    }

    #[test]
    fn the_persons_names_for_the_mixes_are_used() {
        let mut workspace = Rig::owners().workspace();
        let mixes = vec![MixConfig { name: Some("Phones".into()), mono: None }, MixConfig::default(), MixConfig::default(), MixConfig { name: Some("Amp".into()), mono: None }];
        workspace.mixers.insert(DeviceId::from_serial(QUADRO), DeviceMixer { mixes, ..DeviceMixer::default() });
        assert!(only(reasons(&devices(), &workspace)).message.contains("USB 1 Play 3 also goes to Phones and Amp,"));
    }

    #[test]
    fn a_dedicated_path_whose_output_plays_something_else_names_what_and_the_fix_puts_it_back() {
        let mut rig = Rig::dedicated();
        rig.quadro.get_mut("SPDIF_OUT0").unwrap()[0] = [MIX1_OUT, 0];
        let reason = only(reasons(&devices(), &rig.workspace()));
        assert_eq!(
            reason.message,
            "The phase path over the S/PDIF cable is broken: Quadro's S/PDIF Out L no longer plays USB 1 Play 16: it plays Mix 1 L now. Until it is back the measurement hears nothing, and each session is lined up by the figures the drivers report. The S/PDIF cable is dedicated to it, so it can be put back from here."
        );
        let fix = reason.fix.as_ref().unwrap();
        assert_eq!((fix.kind, fix.method, fix.route.as_str(), fix.label.as_str()), (RESTORE_KIND, "PUT", "routing", "Put the phase path back"));
        assert_eq!(writes(&reason), json!([{ "device_id": "serial:Q", "destination": 6, "channel": 0, "source": [COM_PLAY, 15] }]));
    }

    #[test]
    fn a_muted_dedicated_output_says_so() {
        let mut rig = Rig::dedicated();
        rig.quadro.get_mut("SPDIF_OUT0").unwrap()[0] = [Q_MUTE, 0];
        assert!(only(reasons(&devices(), &rig.workspace())).message.starts_with("The phase path over the S/PDIF cable is broken: Quadro's S/PDIF Out L no longer plays USB 1 Play 16: it is muted now."));
    }

    /// Every part broken at once: the playback channel into a mix and to the monitors, and the record
    /// channel taken for a preamp. The fix takes it out of both and records the cable again.
    #[test]
    fn every_part_of_a_broken_dedicated_path_is_named_and_put_back() {
        let mut rig = Rig::dedicated();
        rig.quadro.get_mut("MIXER_IN1").unwrap()[7] = [COM_PLAY, 15];
        rig.quadro.get_mut("MONITOR0").unwrap()[1] = [COM_PLAY, 15];
        rig.studio.get_mut("USB_REC0").unwrap()[23] = [0, 2];
        let reason = only(reasons(&devices(), &rig.workspace()));
        assert!(reason.message.contains("USB 1 Play 16 also goes to Monitor R and Mix 2, and the short burst"), "{}", reason.message);
        assert!(reason.message.contains("Studio+'s USB Rec 24 no longer records S/PDIF In L: it records Preamp 3 now."), "{}", reason.message);
        assert_eq!(
            writes(&reason),
            json!([
                { "device_id": "serial:Q", "destination": 3, "channel": 1, "source": null },
                { "device_id": "serial:Q", "destination": 9, "channel": 7, "source": null },
                { "device_id": "serial:S", "destination": 6, "channel": 23, "source": [SPDIF_IN, 0] },
            ])
        );
    }

    #[test]
    fn a_follower_recording_nothing_on_its_phase_channel_is_said_so() {
        let mut rig = Rig::owners();
        rig.quadro = silent("quadro");
        rig.quadro.get_mut("SPDIF_OUT0").unwrap()[0] = [COM_PLAY, 2];
        rig.studio.get_mut("USB_REC0").unwrap()[20] = [S_MUTE, 0];
        let reason = only(reasons(&devices(), &rig.workspace()));
        assert!(reason.message.starts_with("The phase path over the S/PDIF cable is broken: Studio+'s USB Rec 21 records nothing, not S/PDIF In L."), "{}", reason.message);
    }

    /// The burst on the cable's right channel is a path too, as long as the follower records that side.
    #[test]
    fn a_path_over_the_cables_second_channel_is_whole() {
        let mut rig = Rig::owners();
        rig.quadro = silent("quadro");
        rig.quadro.get_mut("SPDIF_OUT0").unwrap()[1] = [COM_PLAY, 2];
        rig.studio.get_mut("USB_REC0").unwrap()[20] = [SPDIF_IN, 1];
        assert!(reasons(&devices(), &rig.workspace()).is_empty());
    }

    #[test]
    fn routing_that_has_not_been_seen_is_not_judged() {
        let mut rig = Rig::owners();
        rig.quadro = Routing::new();
        rig.studio = Routing::new();
        assert!(reasons(&devices(), &rig.workspace()).is_empty());
        let mut no_setup = Rig::owners();
        no_setup.phase = None;
        assert!(reasons(&devices(), &no_setup.workspace()).is_empty(), "no phase setup, no path to judge");
    }

    #[test]
    fn a_dedication_whose_sender_no_longer_drives_the_callback_says_it_means_nothing() {
        let flipped = vec![report(0, "Quadro", QUADRO, "quadro", false), report(1, "Studio+", STUDIO, "studio", true)];
        let reason = only(reasons(&flipped, &Rig::dedicated().workspace()));
        assert_eq!(reason.code, ReasonCode::PhaseDedicationStale);
        assert_eq!(reason.severity, Severity::Warning);
        assert_eq!(
            reason.message,
            "The S/PDIF cable from Quadro to Studio+ is dedicated to the phase measurement, and Quadro does not drive the callback now, and a phase is measured from the interface that does, so the dedication means nothing: its routing stays as it is and nothing guards it. Turn it off on the Workspace page to release its channels, or dedicate the cable again."
        );
        assert!(reason.fix.is_none());
    }

    #[test]
    fn a_dedication_whose_device_left_the_aggregate_or_whose_phase_setup_moved_says_so() {
        let alone = vec![report(0, "Quadro", QUADRO, "quadro", true)];
        assert!(only(reasons(&alone, &Rig::dedicated().workspace())).message.contains("and serial:S is not in the aggregate now"));
        let mut aliased = Rig::dedicated().workspace();
        aliased.aliases.insert(DeviceId::from_serial(STUDIO), "Drums".into());
        assert!(only(reasons(&alone, &aliased)).message.contains("from Quadro to Drums is dedicated"));

        let mut moved = Rig::dedicated();
        moved.phase = Some(AggregatePhase { master_output: Some(2), input: Some(23), reference: None });
        let said = reasons(&devices(), &moved.workspace());
        assert!(said.iter().any(|r| r.code == ReasonCode::PhaseDedicationStale && r.message.contains("Studio+'s phase setup names other channels now")), "{said:#?}");
        let mut cleared = Rig::dedicated();
        cleared.phase = None;
        assert!(only(reasons(&devices(), &cleared.workspace())).message.contains("Studio+ has no phase setup now"));
    }
}
