//! The aggregate's configuration: what a workspace may hold, and the file the driver reads.
//!
//! The setup lives in the workspace, so it travels with a workspace backup and a person edits it
//! in one place. The driver cannot read a workspace: it runs inside a DAW, with Gazelle very
//! possibly closed, and one small file is all it asks for. So the workspace is the truth and the
//! file is an export of it, written in exactly the shape the driver's README describes.
//!
//! Nothing Gazelle knows and the driver does not goes into the file, and anything a newer Gazelle
//! wrote into the section that this one does not know goes through unchanged: the driver ignores
//! a field it does not know, so passing it on is what makes a newer setup work in an older
//! Gazelle.
//!
//! **Every name in the file is Gazelle's** (`crate::aggregate::naming`): each interface is called
//! by the person's name for its device, else its model's short form, the callback master by that
//! same name, and each channel by what it carries, with the names the person typed put over the
//! automatic ones. So the file is an
//! export of the whole workspace, not of the section alone: renaming a device, naming a Mixer
//! channel or changing the routing all change what the driver is given.

use std::collections::BTreeSet;

use serde_json::{Map, Value};

use crate::aggregate::naming::{daw_names, driver_labels, interface_names};
use crate::aggregate::{rate_index, RateFrom, RateInForce, RATES};
use crate::workspace::model::{Aggregate, AggregateDevice, Workspace, AGGREGATE_CHANNEL_MAX, ALIGNMENTS, CHANNEL_NAME_MAX, PHASE_REFERENCE_MAX, TRIM_MAX};

/// The buffer sizes a configuration may ask for: powers of two the drivers offer.
pub const BUFFER_SIZES: &[u32] = &[16, 32, 64, 128, 256, 512, 1024, 2048, 4096, 8192];

/// The rate the aggregate puts every interface at when it opens, and where it comes from.
///
/// The setup's own rate, when it names one. When it leaves the rate to the interfaces ("whatever the
/// interfaces are on"), the rate every configured interface was last seen running at, as the
/// interface reports it, provided they all agree and each has been seen: an interface's driver can
/// remember another rate, and put the interface back to it when a DAW opens the driver, so the
/// aggregate has to be told the rate the interfaces are actually on. With interfaces that disagree,
/// or one never seen, there is no rate in force, and each interface keeps what its driver says.
pub fn rate_in_force(config: &Aggregate) -> Option<RateInForce> {
    if let Some(hz) = config.rate {
        return Some(RateInForce { hz, from: RateFrom::Setup });
    }
    let mut seen = config.devices.iter().map(|device| device.known.as_ref().and_then(|known| known.rate));
    let first = seen.next()??;
    seen.all(|rate| rate == Some(first)).then_some(RateInForce { hz: first, from: RateFrom::Interfaces })
}

/// Which device drives the callback, by its place in the setup: the one `callback_master` names,
/// else the first.
pub fn master_index(config: &Aggregate) -> Option<usize> {
    match config.callback_master.as_deref().map(str::trim).filter(|m| !m.is_empty()) {
        None => (!config.devices.is_empty()).then_some(0),
        Some(master) => config.devices.iter().position(|device| names_device(device, master)),
    }
}

/// Which device drives the callback: the one `callback_master` names, else the first.
pub fn master_of(config: &Aggregate) -> Option<&AggregateDevice> {
    master_index(config).and_then(|at| config.devices.get(at))
}

/// What Gazelle writes into `callback_master` for a device: its registry key, else its class id.
/// Neither changes when the device is renamed, which is why a rename cannot lose the master.
pub fn master_reference(device: &AggregateDevice) -> Option<String> {
    device.key.as_deref().or(device.clsid.as_deref()).map(str::trim).filter(|reference| !reference.is_empty()).map(str::to_string)
}

/// Whether `named` is this device's registry key, its class id or the name an older setup gave it,
/// without case.
pub fn names_device(device: &AggregateDevice, named: &str) -> bool {
    [device.name.as_deref(), device.key.as_deref(), device.clsid.as_deref()]
        .into_iter()
        .flatten()
        .any(|candidate| candidate.trim().eq_ignore_ascii_case(named.trim()))
}

/// Check an aggregate section as the rest of the workspace is checked: every message names the
/// device it is about, by Gazelle's name for it, and says what would have been right.
pub fn check(config: &Aggregate, workspace: &Workspace) -> Result<(), String> {
    if let Some(alignment) = &config.alignment {
        if !ALIGNMENTS.contains(&alignment.as_str()) {
            return Err(format!("alignment must be one of {}, not {alignment:?}", ALIGNMENTS.join(", ")));
        }
    }
    if let Some(rate) = config.rate {
        if rate_index(rate).is_none() {
            return Err(format!(
                "rate must be one of {}, not {rate}",
                RATES.iter().map(u32::to_string).collect::<Vec<_>>().join(", ")
            ));
        }
    }
    if let Some(size) = config.buffer_size {
        if !BUFFER_SIZES.contains(&size) {
            return Err(format!("buffer_size must be one of {}, not {size}", BUFFER_SIZES.iter().map(u32::to_string).collect::<Vec<_>>().join(", ")));
        }
    }
    let (mut keys, mut clsids, mut devices) = (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
    let names = interface_names(config, workspace);
    let master = master_index(config);
    for (at, device) in config.devices.iter().enumerate() {
        let named = &names[at];
        let bad = |message: String| Err(format!("device '{named}': {message}"));
        let key = device.key.as_deref().map(str::trim).filter(|k| !k.is_empty());
        let clsid = device.clsid.as_deref().map(str::trim).filter(|c| !c.is_empty());
        if key.is_none() && clsid.is_none() {
            return bad("it needs a key or a clsid, which is how the driver finds it".into());
        }
        if let Some(clsid) = clsid {
            if !looks_like_clsid(clsid) {
                return bad(format!("clsid must be a class id in braces, not {clsid:?}"));
            }
            if !clsids.insert(clsid.to_ascii_lowercase()) {
                return bad("this class id is named twice".into());
            }
        }
        if let Some(key) = key {
            if !keys.insert(key.to_ascii_lowercase()) {
                return bad("this key is named twice".into());
            }
        }
        if let Some(id) = &device.device_id {
            if !devices.insert(id.clone()) {
                return bad(format!("{id} is already another device in this aggregate"));
            }
        }
        for (what, trim) in [("input_trim", device.input_trim), ("output_trim", device.output_trim)] {
            if let Some(trim) = trim.filter(|t| t.abs() > TRIM_MAX) {
                return bad(format!("{what} is {trim} samples, which is outside -{TRIM_MAX}..{TRIM_MAX}"));
            }
        }
        if let Some(phase) = &device.phase {
            // Both ends or neither: a signal with nowhere to leave from, or nowhere to arrive, is
            // not a measurement.
            match (phase.master_output, phase.input) {
                (None, _) => return bad(
                    "phase needs master_output, which is the output channel of the interface that drives the callback that the cable leaves from".into(),
                ),
                (_, None) => return bad("phase needs input, which is this interface's own input channel the cable arrives on".into()),
                (Some(output), Some(input)) => {
                    for (what, channel) in [("master_output", output), ("input", input)] {
                        if channel > AGGREGATE_CHANNEL_MAX {
                            return bad(format!("phase names {what} {channel}, which is outside 0..{AGGREGATE_CHANNEL_MAX}"));
                        }
                    }
                }
            }
            if let Some(reference) = phase.reference.filter(|r| r.saturating_abs() > PHASE_REFERENCE_MAX) {
                return bad(format!(
                    "phase has a reference of {reference} samples, which is outside -{PHASE_REFERENCE_MAX}..{PHASE_REFERENCE_MAX}. It is the phase measured beside the input trim, so measure the interfaces again rather than typing one in."
                ));
            }
            if master == Some(at) {
                return bad(
                    "it drives the callback, and a phase is measured against the interface that drives the callback, so it cannot be measured against itself".into(),
                );
            }
        }
        for (what, channels) in [("inputs", &device.inputs), ("outputs", &device.outputs)] {
            let Some(channels) = channels else { continue };
            let mut seen = BTreeSet::new();
            for &channel in channels {
                if channel > AGGREGATE_CHANNEL_MAX {
                    return bad(format!("{what} names channel {channel}, which is outside 0..{AGGREGATE_CHANNEL_MAX}"));
                }
                if !seen.insert(channel) {
                    return bad(format!("{what} names channel {channel} twice"));
                }
            }
        }
        for (what, labels) in [("input_names", &device.input_names), ("output_names", &device.output_names)] {
            for (&channel, label) in labels {
                if channel > AGGREGATE_CHANNEL_MAX {
                    return bad(format!("{what} labels channel {channel}, which is outside 0..{AGGREGATE_CHANNEL_MAX}"));
                }
                let label = label.trim();
                if label.is_empty() {
                    return bad(format!("{what} gives channel {channel} a blank label. Give it something to be called, or leave the channel out."));
                }
                let length = label.chars().count();
                if length > CHANNEL_NAME_MAX {
                    return bad(format!(
                        "{what} calls channel {channel} {label:?}, which is {length} characters. A channel's label is at most {CHANNEL_NAME_MAX}, because that is what the interface carries and the name in a DAW is built from it."
                    ));
                }
                if label.chars().any(char::is_control) {
                    return bad(format!("{what} calls channel {channel} {label:?}, which has a control character in it. A label is plain text."));
                }
            }
        }
    }
    if let Some(master) = config.callback_master.as_deref().map(str::trim).filter(|m| !m.is_empty()) {
        if !config.devices.iter().any(|device| names_device(device, master)) {
            return Err(format!("callback_master is {master:?}, which is not one of the devices"));
        }
    }
    Ok(())
}

/// Whether a string looks like `{8-4-4-4-12}`. Not a parse: it only catches a value that could
/// never be a class id, and the driver refuses the rest by name.
pub fn looks_like_clsid(text: &str) -> bool {
    let text = text.trim();
    let Some(inner) = text.strip_prefix('{').and_then(|t| t.strip_suffix('}')) else { return false };
    let parts: Vec<&str> = inner.split('-').collect();
    parts.len() == 5
        && parts.iter().map(|p| p.len()).eq([8usize, 4, 4, 4, 12])
        && parts.iter().all(|part| part.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// The configuration as the driver's file holds it, with every name Gazelle's.
///
/// Built by hand rather than by deriving a second set of structures, because the shapes differ in
/// the ways that matter: `device_id` and `known` are Gazelle's own and the file must not carry
/// them, each interface's `name` is what the DAW calls it (the person's name for the device, else
/// its model's short form, [`daw_names`]) rather than anything the section holds, `callback_master`
/// is turned into that same name, and each channel's label is the
/// automatic one with the person's typed one over it. A field that is not set is left out, so the
/// file says only what was chosen.
pub fn export_document(config: &Aggregate, workspace: &Workspace) -> Value {
    let mut document = Map::new();
    let names = daw_names(config, workspace);
    if !config.devices.is_empty() {
        document.insert("devices".into(), Value::Array(config.devices.iter().zip(&names).map(|(device, name)| export_device(device, name, workspace)).collect()));
    }
    // The master by the very name the driver will know it by, so a rename moves both together. One
    // the section names that matches nothing is passed on as it is, and the driver says so.
    let master = config.callback_master.as_ref().map(|asked| match master_index(config) {
        Some(at) => names[at].clone(),
        None => asked.clone(),
    });
    // The rate in force, the setup's or the one every interface is running at, so the driver puts
    // every interface there when it opens rather than wherever its driver last was. `rate_from`
    // says which, for a person reading the file; the driver passes over a field it does not know.
    let rate = rate_in_force(config);
    let from = rate.map(|rate| match rate.from {
        RateFrom::Setup => "setup",
        RateFrom::Interfaces => "interfaces",
    });
    for (name, value) in [
        ("callback_master", master.map(Value::from)),
        ("alignment", config.alignment.clone().map(Value::from)),
        ("rate", rate.map(|rate| Value::from(rate.hz))),
        ("rate_from", from.map(Value::from)),
        ("buffer_size", config.buffer_size.map(Value::from)),
    ] {
        if let Some(value) = value {
            document.insert(name.into(), value);
        }
    }
    for (name, value) in &config.extra {
        document.insert(name.clone(), value.clone());
    }
    Value::Object(document)
}

fn export_device(device: &AggregateDevice, name: &str, workspace: &Workspace) -> Value {
    let mut out = Map::new();
    for (field, value) in [
        ("key", device.key.clone().map(Value::from)),
        ("clsid", device.clsid.clone().map(Value::from)),
        ("name", Some(Value::from(name))),
        ("input_trim", device.input_trim.map(Value::from)),
        ("output_trim", device.output_trim.map(Value::from)),
        ("inputs", device.inputs.clone().map(Value::from)),
        ("outputs", device.outputs.clone().map(Value::from)),
    ] {
        if let Some(value) = value {
            out.insert(field.into(), value);
        }
    }
    // Both ends of the measurement cable or neither, which is what the check above settled, and the
    // reference beside them when a calibration run has written one.
    if let Some(phase) = device.phase {
        if let Some((output, input)) = phase.master_output.zip(phase.input) {
            let mut cable = serde_json::json!({ "master_output": output, "input": input });
            if let Some(reference) = phase.reference {
                cable["reference"] = Value::from(reference);
            }
            out.insert("phase".into(), cable);
        }
    }
    // Every channel's label: the automatic one from Gazelle's routing and names, with the person's
    // typed one over it. A map with nothing in it is left out rather than written as an empty object.
    let (inputs, outputs) = driver_labels(device, workspace);
    for (field, labels) in [("input_names", inputs), ("output_names", outputs)] {
        if labels.is_empty() {
            continue;
        }
        out.insert(field.into(), Value::Object(labels.into_iter().map(|(channel, label)| (channel.to_string(), Value::from(label))).collect()));
    }
    for (name, value) in &device.extra {
        out.insert(name.clone(), value.clone());
    }
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::descriptor::DeviceId;
    use crate::workspace::model::{AggregateKnown, AggregatePhase, DeviceMixer, MixerChannel, RouteSource};

    /// An entry for one of Gazelle's devices, which is how every entry the page makes looks once the
    /// device has been matched.
    fn device(key: &str, family: &str, serial: &str) -> AggregateDevice {
        AggregateDevice {
            key: Some(key.into()),
            device_id: Some(DeviceId::from_serial(serial)),
            known: Some(AggregateKnown { device_id: Some(DeviceId::from_serial(serial)), family: Some(family.into()), model: None, routing: Default::default(), rate: None }),
            ..AggregateDevice::default()
        }
    }

    fn pair() -> Aggregate {
        Aggregate {
            devices: vec![device("Zen Quadro Synergy Core", "quadro", "Q"), device("ZenStudioTB", "studio", "S")],
            callback_master: Some("Zen Quadro Synergy Core".into()),
            alignment: Some("aligned".into()),
            rate: Some(96000),
            buffer_size: Some(512),
            extra: Default::default(),
        }
    }

    /// The person's own names for the two devices, as the sidebar would show them.
    fn named() -> Workspace {
        let mut workspace = Workspace::default();
        workspace.aliases.insert(DeviceId::from_serial("Q"), "Quadro".into());
        workspace.aliases.insert(DeviceId::from_serial("S"), "Studio+".into());
        workspace
    }

    #[test]
    fn the_worked_example_is_accepted() {
        check(&pair(), &named()).expect("the setup phase 0 measured");
        assert_eq!(master_index(&pair()), Some(0));
        assert_eq!(interface_names(&pair(), &named()), ["Quadro", "Studio+"]);
    }

    #[test]
    fn a_device_needs_a_key_or_a_class_id_and_a_class_id_must_look_like_one() {
        let mut config = pair();
        config.devices[1] = AggregateDevice { device_id: Some(DeviceId::from_serial("S")), ..AggregateDevice::default() };
        assert_eq!(check(&config, &named()).unwrap_err(), "device 'Studio+': it needs a key or a clsid, which is how the driver finds it");
        config.devices[1] = AggregateDevice { clsid: Some("AE4A4452".into()), ..AggregateDevice::default() };
        assert!(check(&config, &named()).unwrap_err().contains("must be a class id in braces"));
        config.devices[1] = AggregateDevice { clsid: Some("{AE4A4452-A316-11E5-A113-080027F6C1F4}".into()), ..AggregateDevice::default() };
        check(&config, &named()).expect("a class id alone is enough");
    }

    #[test]
    fn a_device_is_in_the_aggregate_once() {
        let mut config = pair();
        config.devices[1].key = Some("zen quadro synergy core".into());
        assert_eq!(check(&config, &named()).unwrap_err(), "device 'Studio+': this key is named twice");
        let mut config = pair();
        config.devices[1].device_id = Some(DeviceId::from_serial("Q"));
        assert_eq!(check(&config, &named()).unwrap_err(), "device 'Quadro (2)': serial:Q is already another device in this aggregate");
    }

    /// Two devices of one model that nobody has named come out alike, which is the person's to put
    /// right in Gazelle, not a setup to refuse: the second is told apart by its place.
    #[test]
    fn two_devices_gazelle_names_alike_are_accepted_and_told_apart() {
        let mut config = pair();
        config.devices[1] = device("Zen Quadro Synergy Core 2", "quadro", "Q2");
        let workspace = Workspace::default();
        check(&config, &workspace).expect("the names are Gazelle's, and naming the devices is how to tell them apart");
        let document = export_document(&config, &workspace);
        assert_eq!(document["devices"][0]["name"], "Quadro", "the model's short form, in a DAW");
        assert_eq!(document["devices"][1]["name"], "Quadro 2");
        assert_eq!(interface_names(&config, &workspace), ["Zen Quadro Synergy Core", "Zen Quadro Synergy Core (2)"], "and the full name everywhere else");
    }

    #[test]
    fn a_trim_and_a_channel_must_be_numbers_they_could_be() {
        let mut config = pair();
        config.devices[0].input_trim = Some(TRIM_MAX + 1);
        assert!(check(&config, &named()).unwrap_err().contains("input_trim is 192001 samples"));
        config.devices[0].input_trim = Some(-28);
        check(&config, &named()).expect("a real trim is tens of samples, either way");
        config.devices[0].outputs = Some(vec![0, 1, AGGREGATE_CHANNEL_MAX + 1]);
        assert!(check(&config, &named()).unwrap_err().contains("outputs names channel 1024"));
        config.devices[0].outputs = Some(vec![0, 1, 1]);
        assert_eq!(check(&config, &named()).unwrap_err(), "device 'Quadro': outputs names channel 1 twice");
    }

    /// What a channel may be called: the interface carries 31 characters and the name in a DAW is
    /// built from it, so anything longer would arrive cut in half.
    #[test]
    fn a_channel_label_must_be_a_channel_that_exists_and_a_name_that_fits_on_the_interface() {
        let mut config = pair();
        config.devices[0].input_names = [(0, "Vocal mic".to_string()), (3, "Room".to_string())].into_iter().collect();
        config.devices[0].output_names = [(0, "Main L".to_string())].into_iter().collect();
        check(&config, &named()).expect("a label per channel, on either side");

        config.devices[0].input_names.insert(AGGREGATE_CHANNEL_MAX + 1, "Nowhere".into());
        assert_eq!(check(&config, &named()).unwrap_err(), "device 'Quadro': input_names labels channel 1024, which is outside 0..1023");

        let mut config = pair();
        config.devices[0].input_names = [(2, "   ".to_string())].into_iter().collect();
        assert_eq!(
            check(&config, &named()).unwrap_err(),
            "device 'Quadro': input_names gives channel 2 a blank label. Give it something to be called, or leave the channel out."
        );

        config.devices[0].input_names = [(2, "a".repeat(CHANNEL_NAME_MAX))].into_iter().collect();
        check(&config, &named()).expect("exactly what the interface carries is still a label");
        config.devices[0].input_names = [(2, "a".repeat(CHANNEL_NAME_MAX + 1))].into_iter().collect();
        let too_long = check(&config, &named()).unwrap_err();
        assert!(too_long.contains("which is 32 characters"), "{too_long}");
        assert!(too_long.contains("at most 31"), "{too_long}");

        config.devices[0].input_names.clear();
        config.devices[0].output_names = [(1, "Main\tR".to_string())].into_iter().collect();
        assert!(check(&config, &named()).unwrap_err().contains("has a control character in it"));
    }

    /// The driver is given a label for every channel: the automatic one from Gazelle's routing, with
    /// the person's typed one over it, and never anything but those.
    #[test]
    fn every_channel_is_exported_with_its_label_and_a_typed_one_wins() {
        let mut config = pair();
        config.devices[0].input_names = [(0, "Vocal mic".to_string()), (10, "Room".to_string())].into_iter().collect();
        let document = export_document(&config, &named());
        let inputs = document["devices"][0]["input_names"].as_object().expect("the Quadro's inputs");
        assert_eq!(inputs.len(), 16, "one for each of its sixteen USB record channels");
        assert_eq!(inputs["0"], "Vocal mic");
        assert_eq!(inputs["10"], "Room");
        assert_eq!(inputs["1"], "USB A REC 2", "a channel whose routing is not known yet is its record channel");
        assert_eq!(document["devices"][0]["output_names"]["15"], "USB 1 PLAY 16");
        assert_eq!(document["devices"][1]["input_names"].as_object().map(|m| m.len()), Some(24), "and the Studio+'s twenty four");
        // Every label fits what the interface carries.
        for device in document["devices"].as_array().unwrap() {
            for side in ["input_names", "output_names"] {
                assert!(device[side].as_object().unwrap().values().all(|label| label.as_str().unwrap().chars().count() <= CHANNEL_NAME_MAX));
            }
        }
    }

    #[test]
    fn a_device_whose_model_is_not_known_yet_is_given_only_what_the_person_typed() {
        let config = Aggregate {
            devices: vec![AggregateDevice { key: Some("Zen Quadro".into()), input_names: [(0, "Vocal mic".to_string())].into_iter().collect(), ..AggregateDevice::default() }],
            ..Aggregate::default()
        };
        let document = export_document(&config, &Workspace::default());
        assert_eq!(document["devices"][0]["input_names"], serde_json::json!({ "0": "Vocal mic" }));
        assert!(document["devices"][0].get("output_names").is_none(), "a side with nothing on it is not written as an empty object");
        assert_eq!(document["devices"][0]["name"], "Zen Quadro", "with nothing else known, the vendor driver's key");
    }

    /// The owner's case for outputs: re-routing a USB playback channel renames it in the file.
    #[test]
    fn a_reroute_changes_an_outputs_name_in_the_file() {
        let mut config = pair();
        let family = "quadro";
        let groups = crate::aggregate::naming::naming_groups(family);
        let destinations = crate::workspace::topology::destination_groups_whole(family).unwrap();
        let sources = crate::workspace::topology::source_groups(family).unwrap();
        let at = |id: &str| sources.iter().position(|g| g.id == id).unwrap() as u8;
        let mut routing: crate::aggregate::naming::Routing = groups
            .iter()
            .map(|(id, _)| (id.clone(), vec![[at("MUTE0"), 0]; destinations.iter().find(|g| &g.id == id).unwrap().channels as usize]))
            .collect();
        routing.get_mut("MIXER_IN0").unwrap()[6] = [at("COM_PLAY0"), 2];
        routing.get_mut("MONITOR0").unwrap()[0] = [at("MIXER_OUT0"), 0];
        config.devices[0].known.as_mut().unwrap().routing = routing.clone();
        let workspace = Workspace::default();
        assert_eq!(export_document(&config, &workspace)["devices"][0]["output_names"]["2"], "Monitor L");
        assert_eq!(export_document(&config, &workspace)["devices"][0]["output_names"]["3"], "Not routed");
        // Mix 1 goes to the headphones instead of the monitors.
        routing.get_mut("MONITOR0").unwrap()[0] = [at("MUTE0"), 0];
        routing.get_mut("HEADPHONES0").unwrap()[0] = [at("MIXER_OUT0"), 0];
        config.devices[0].known.as_mut().unwrap().routing = routing;
        assert_eq!(export_document(&config, &workspace)["devices"][0]["output_names"]["2"], "HP1 L");
    }

    /// The owner's case: an input named for what the routing sends it, changing with the routing,
    /// and a typed name left exactly where it was.
    #[test]
    fn a_reroute_changes_the_automatic_name_in_the_file_and_leaves_a_typed_one_alone() {
        let mut config = pair();
        let mut workspace = named();
        workspace.mixers.insert(
            DeviceId::from_serial("Q"),
            DeviceMixer {
                channels: vec![MixerChannel { id: "c".into(), name: "Vocal mic".into(), group: None, color: None, slot: 6, source: Some(RouteSource { group: 0, channel: 0 }), main_mix: Some(0), sends: Vec::new() }],
                ..DeviceMixer::default()
            },
        );
        config.devices[0].input_names = [(1, "Talkback".to_string())].into_iter().collect();
        // USB A REC 1 takes PREAMP 1, which the person's Mixer channel calls Vocal mic.
        config.devices[0].known.as_mut().unwrap().routing = [("COM_REC0".to_string(), vec![[0, 0], [0, 1]])].into_iter().collect();
        let before = export_document(&config, &workspace);
        assert_eq!(before["devices"][0]["input_names"]["0"], "Vocal mic");
        assert_eq!(before["devices"][0]["input_names"]["1"], "Talkback");
        // Routed from AFX OUT 3 instead.
        config.devices[0].known.as_mut().unwrap().routing = [("COM_REC0".to_string(), vec![[5, 2], [0, 1]])].into_iter().collect();
        let after = export_document(&config, &workspace);
        assert_eq!(after["devices"][0]["input_names"]["0"], "AFX OUT 3");
        assert_eq!(after["devices"][0]["input_names"]["1"], "Talkback", "the typed name is untouched");
        assert_ne!(before, after);
    }

    /// Renaming a device in Gazelle renames it in the file, and the callback master follows it,
    /// because the section names the master by something a rename cannot change.
    #[test]
    fn a_device_renamed_in_gazelle_is_renamed_in_the_file_and_stays_the_master() {
        let config = pair();
        let mut workspace = named();
        let first = export_document(&config, &workspace);
        assert_eq!(first["devices"][0]["name"], "Quadro");
        assert_eq!(first["callback_master"], "Quadro");
        workspace.aliases.insert(DeviceId::from_serial("Q"), "Desk".into());
        let renamed = export_document(&config, &workspace);
        assert_eq!(renamed["devices"][0]["name"], "Desk");
        assert_eq!(renamed["callback_master"], "Desk", "the master is the same device, under its new name");
        assert_eq!(config.callback_master.as_deref(), Some("Zen Quadro Synergy Core"), "and the section itself never had to change");
        // With the name taken off, it is its model's short form again, which leaves the reference room.
        workspace.aliases.remove(&DeviceId::from_serial("Q"));
        let model = export_document(&config, &workspace);
        assert_eq!(model["devices"][0]["name"], "Quadro");
        assert_eq!(model["callback_master"], "Quadro");
    }

    /// A setup an older Gazelle wrote names its master by the name it gave the device; that is still
    /// understood, and the driver is given the device's name as Gazelle calls it now.
    #[test]
    fn a_master_an_older_setup_named_by_its_own_name_still_finds_its_device() {
        let mut config = pair();
        config.devices[1].name = Some("Old Studio name".into());
        config.callback_master = Some("old studio name".into());
        assert_eq!(master_index(&config), Some(1));
        check(&config, &named()).expect("an older setup is still a setup");
        assert_eq!(export_document(&config, &named())["callback_master"], "Studio+");
        assert_eq!(master_reference(&config.devices[1]).as_deref(), Some("ZenStudioTB"), "what Gazelle writes now is the key");
        let by_class = AggregateDevice { clsid: Some("{AE4A4452-A316-11E5-A113-080027F6C1F4}".into()), ..AggregateDevice::default() };
        assert_eq!(master_reference(&by_class).as_deref(), Some("{AE4A4452-A316-11E5-A113-080027F6C1F4}"));
    }

    /// Where the cable a phase is measured over runs, and the two ways of getting it wrong.
    #[test]
    fn a_phase_needs_both_ends_of_its_cable_and_an_interface_that_is_not_the_master() {
        let mut config = pair();
        config.devices[1].phase = Some(AggregatePhase { master_output: Some(8), input: Some(16), reference: None });
        check(&config, &named()).expect("a follower with a cable declared into it");

        config.devices[1].phase = Some(AggregatePhase { master_output: Some(8), input: None, reference: None });
        assert_eq!(
            check(&config, &named()).unwrap_err(),
            "device 'Studio+': phase needs input, which is this interface's own input channel the cable arrives on"
        );
        config.devices[1].phase = Some(AggregatePhase { master_output: None, input: Some(16), reference: None });
        assert!(check(&config, &named()).unwrap_err().contains("phase needs master_output"));

        config.devices[1].phase = Some(AggregatePhase { master_output: Some(8), input: Some(AGGREGATE_CHANNEL_MAX + 1), reference: None });
        assert!(check(&config, &named()).unwrap_err().contains("phase names input 1024"));

        // The interface that drives the callback is what everything else is measured against.
        let mut master = pair();
        master.devices[0].phase = Some(AggregatePhase { master_output: Some(8), input: Some(16), reference: None });
        assert!(check(&master, &named()).unwrap_err().contains("cannot be measured against itself"), "{:?}", check(&master, &named()));
    }

    /// The reference a calibration run writes beside the trim: any phase the hardware could
    /// measure, which is a few hundred samples either way, and nothing that could never be one.
    #[test]
    fn a_phase_reference_is_any_phase_the_hardware_could_measure() {
        let mut config = pair();
        config.devices[1].phase = Some(AggregatePhase { master_output: Some(8), input: Some(16), reference: Some(-307) });
        check(&config, &named()).expect("what the hardware measured in one of its sessions");
        config.devices[1].phase = Some(AggregatePhase { master_output: Some(8), input: Some(16), reference: Some(PHASE_REFERENCE_MAX + 1) });
        let why = check(&config, &named()).unwrap_err();
        assert!(why.starts_with("device 'Studio+': phase has a reference of 192001 samples"), "{why}");
        assert!(why.contains("measure the interfaces again"), "{why}");
        // A reference with no cable is still no cable: the half that is missing is what is said.
        config.devices[1].phase = Some(AggregatePhase { master_output: Some(8), input: None, reference: Some(-84) });
        assert!(check(&config, &named()).unwrap_err().contains("phase needs input"));
    }

    /// The workspace keeps the reference as it came, and gives it back.
    #[test]
    fn a_phase_reference_is_kept_in_the_workspace_and_given_back() {
        let text = r#"{"key": "ZenStudioTB", "name": "Studio+", "input_trim": 60, "phase": {"master_output": 8, "input": 16, "reference": -84}}"#;
        let device: AggregateDevice = serde_json::from_str(text).expect("a device with a reference");
        assert_eq!(device.phase, Some(AggregatePhase { master_output: Some(8), input: Some(16), reference: Some(-84) }));
        let back = serde_json::to_value(&device).expect("it goes back out");
        assert_eq!(back["phase"], serde_json::json!({ "master_output": 8, "input": 16, "reference": -84 }));
        let without: AggregateDevice = serde_json::from_str(r#"{"key": "ZenStudioTB", "phase": {"master_output": 8, "input": 16}}"#).unwrap();
        assert_eq!(without.phase.and_then(|phase| phase.reference), None);
        assert!(serde_json::to_value(&without).unwrap()["phase"].get("reference").is_none(), "none is not written as null");
    }

    /// A trim and a phase are different things, and an interface may have both.
    #[test]
    fn a_phase_is_exported_to_the_driver_beside_the_trim_and_neither_stands_for_the_other() {
        let mut config = pair();
        config.devices[1].input_trim = Some(28);
        config.devices[1].phase = Some(AggregatePhase { master_output: Some(8), input: Some(16), reference: None });
        let document = export_document(&config, &named());
        assert_eq!(document["devices"][1]["phase"], serde_json::json!({ "master_output": 8, "input": 16 }));
        assert_eq!(document["devices"][1]["input_trim"], 28);
        assert!(document["devices"][0].get("phase").is_none(), "an interface with no cable declared says nothing");

        // And the reference goes beside the cable, which is where the driver reads it from.
        config.devices[1].input_trim = Some(60);
        config.devices[1].phase = Some(AggregatePhase { master_output: Some(8), input: Some(16), reference: Some(-84) });
        let document = export_document(&config, &named());
        assert_eq!(document["devices"][1]["phase"], serde_json::json!({ "master_output": 8, "input": 16, "reference": -84 }));
        assert_eq!(document["devices"][1]["input_trim"], 60);
    }

    #[test]
    fn the_master_the_alignment_the_rate_and_the_buffer_must_be_ones_that_exist() {
        let mut config = pair();
        config.callback_master = Some("Octo".into());
        assert_eq!(check(&config, &named()).unwrap_err(), "callback_master is \"Octo\", which is not one of the devices");
        config = pair();
        config.alignment = Some("sample_accurate".into());
        assert!(check(&config, &named()).unwrap_err().starts_with("alignment must be one of aligned, lowest_latency"));
        config = pair();
        config.rate = Some(97000);
        assert!(check(&config, &named()).unwrap_err().starts_with("rate must be one of 32000, 44100"), "a rate the devices cannot be put at");
        config = pair();
        config.buffer_size = Some(500);
        assert!(check(&config, &named()).unwrap_err().contains("buffer_size must be one of 16, 32"));
    }

    /// The file the driver reads, field for field against its README's worked example.
    #[test]
    fn the_export_is_the_shape_the_driver_reads() {
        let config = Aggregate {
            devices: vec![
                AggregateDevice { key: Some("Zen Quadro Synergy Core".into()), name: Some("An old name".into()), device_id: Some(DeviceId::from_serial("Q")), ..AggregateDevice::default() },
                AggregateDevice {
                    clsid: Some("{AE4A4452-A316-11E5-A113-080027F6C1F4}".into()),
                    device_id: Some(DeviceId::from_serial("S")),
                    input_trim: Some(28),
                    inputs: Some(vec![0, 1, 2, 3]),
                    ..AggregateDevice::default()
                },
            ],
            callback_master: Some("Zen Quadro Synergy Core".into()),
            alignment: Some("aligned".into()),
            rate: Some(96000),
            buffer_size: Some(512),
            extra: Default::default(),
        };
        let document = export_document(&config, &named());
        assert_eq!(
            document,
            serde_json::json!({
                "devices": [
                    { "key": "Zen Quadro Synergy Core", "name": "Quadro" },
                    { "clsid": "{AE4A4452-A316-11E5-A113-080027F6C1F4}", "name": "Studio+", "input_trim": 28, "inputs": [0, 1, 2, 3] }
                ],
                "callback_master": "Quadro",
                "alignment": "aligned",
                "rate": 96000,
                "rate_from": "setup",
                "buffer_size": 512
            })
        );
        assert!(document["devices"][0].get("device_id").is_none(), "Gazelle's own field is not the driver's business");
        assert!(document["devices"][0].get("known").is_none(), "and nor is what Gazelle last knew");
        assert!(document["devices"][0].get("input_trim").is_none(), "a field that was not set is left out, not written as a default");
    }

    /// A newer Gazelle's fields survive being loaded here and reach the driver, which ignores
    /// what it does not know in the same way.
    #[test]
    fn a_field_this_gazelle_does_not_know_goes_through_to_the_file() {
        let text = r#"{
            "devices": [{ "key": "Quadro", "resample": true }],
            "ring_buffers": 8
        }"#;
        let config: Aggregate = serde_json::from_str(text).unwrap();
        let document = export_document(&config, &Workspace::default());
        assert_eq!(document["ring_buffers"], 8);
        assert_eq!(document["devices"][0]["resample"], true);
        assert_eq!(serde_json::to_value(&config).unwrap()["ring_buffers"], 8, "and it is written back to the workspace");
    }

    /// "Whatever the interfaces are on" is the rate they are actually running at, when they all
    /// agree, so a driver that remembers another rate cannot move them when the aggregate opens.
    #[test]
    fn with_no_rate_chosen_the_file_carries_the_rate_every_interface_runs_at() {
        let mut config = pair();
        config.rate = None;
        let seen = |config: &mut Aggregate, rates: [Option<u32>; 2]| {
            for (device, rate) in config.devices.iter_mut().zip(rates) {
                device.known.as_mut().unwrap().rate = rate;
            }
        };
        seen(&mut config, [Some(96000), Some(96000)]);
        assert_eq!(rate_in_force(&config), Some(RateInForce { hz: 96000, from: RateFrom::Interfaces }));
        let document = export_document(&config, &named());
        assert_eq!((document["rate"].clone(), document["rate_from"].clone()), (serde_json::json!(96000), serde_json::json!("interfaces")));
        assert_eq!(config.rate, None, "and the setup itself still names no rate");

        // Interfaces that disagree, or one never seen, leave the rate out, as before.
        seen(&mut config, [Some(96000), Some(44100)]);
        assert_eq!(rate_in_force(&config), None);
        assert!(export_document(&config, &named()).get("rate").is_none());
        seen(&mut config, [Some(96000), None]);
        assert!(export_document(&config, &named()).get("rate").is_none());

        // A rate the setup names wins, and says so.
        config.rate = Some(48000);
        assert_eq!(rate_in_force(&config), Some(RateInForce { hz: 48000, from: RateFrom::Setup }));
        assert_eq!(export_document(&config, &named())["rate_from"], "setup");
        assert_eq!(crate::aggregate::khz(44100), "44.1 kHz");
        assert_eq!(crate::aggregate::khz(96000), "96 kHz");
        assert_eq!(crate::aggregate::khz(88200), "88.2 kHz");
    }

    #[test]
    fn an_empty_section_exports_an_empty_document() {
        assert_eq!(export_document(&Aggregate::default(), &Workspace::default()), serde_json::json!({}));
    }
}
