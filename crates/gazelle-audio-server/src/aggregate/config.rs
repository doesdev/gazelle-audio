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

use std::collections::BTreeSet;

use serde_json::{Map, Value};

use crate::aggregate::{rate_index, RATES};
use crate::workspace::model::{Aggregate, AggregateDevice, AGGREGATE_CHANNEL_MAX, ALIGNMENTS, CHANNEL_NAME_MAX, PHASE_REFERENCE_MAX, TRIM_MAX};

/// The buffer sizes a configuration may ask for: powers of two the drivers offer.
pub const BUFFER_SIZES: &[u32] = &[16, 32, 64, 128, 256, 512, 1024, 2048, 4096, 8192];

/// The name a device goes by: what the configuration calls it, else its registry key, else its
/// class id. This is the name every message and every status line uses.
pub fn device_name(device: &AggregateDevice) -> String {
    device
        .name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .or(device.key.as_deref())
        .or(device.clsid.as_deref())
        .unwrap_or("a device with no key or class id")
        .to_string()
}

/// Which device drives the callback: the one `callback_master` names, else the first.
pub fn master_of(config: &Aggregate) -> Option<&AggregateDevice> {
    match config.callback_master.as_deref().map(str::trim).filter(|m| !m.is_empty()) {
        None => config.devices.first(),
        Some(master) => config.devices.iter().find(|device| names_device(device, master)),
    }
}

/// Whether `named` is this device's name, its registry key or its class id, without case.
pub fn names_device(device: &AggregateDevice, named: &str) -> bool {
    [device.name.as_deref(), device.key.as_deref(), device.clsid.as_deref()]
        .into_iter()
        .flatten()
        .any(|candidate| candidate.trim().eq_ignore_ascii_case(named.trim()))
}

/// Check an aggregate section as the rest of the workspace is checked: every message names the
/// device it is about and says what would have been right.
pub fn check(config: &Aggregate) -> Result<(), String> {
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
    let (mut names, mut keys, mut clsids, mut devices) = (BTreeSet::new(), BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
    for device in &config.devices {
        let named = device_name(device);
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
        if !names.insert(named.to_ascii_lowercase()) {
            return bad("this name is used twice, and a channel's name would then say nothing".into());
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
            if master_of(config).is_some_and(|master| device_name(master) == named) {
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

/// The configuration as the driver's file holds it.
///
/// Built by hand rather than by deriving a second set of structures, because the shapes differ in
/// exactly one way that matters: `device_id` is Gazelle's own and the file must not carry it. A
/// field that is not set is left out, so the file says only what the person chose.
pub fn export_document(config: &Aggregate) -> Value {
    let mut document = Map::new();
    if !config.devices.is_empty() {
        document.insert("devices".into(), Value::Array(config.devices.iter().map(export_device).collect()));
    }
    for (name, value) in [
        ("callback_master", config.callback_master.clone().map(Value::from)),
        ("alignment", config.alignment.clone().map(Value::from)),
        ("rate", config.rate.map(Value::from)),
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

fn export_device(device: &AggregateDevice) -> Value {
    let mut out = Map::new();
    for (name, value) in [
        ("key", device.key.clone().map(Value::from)),
        ("clsid", device.clsid.clone().map(Value::from)),
        ("name", device.name.clone().map(Value::from)),
        ("input_trim", device.input_trim.map(Value::from)),
        ("output_trim", device.output_trim.map(Value::from)),
        ("inputs", device.inputs.clone().map(Value::from)),
        ("outputs", device.outputs.clone().map(Value::from)),
    ] {
        if let Some(value) = value {
            out.insert(name.into(), value);
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
    // A channel the person has not named is not in the map, and a map with nothing in it is left
    // out entirely rather than written as an empty object.
    for (name, labels) in [("input_names", &device.input_names), ("output_names", &device.output_names)] {
        if labels.is_empty() {
            continue;
        }
        out.insert(name.into(), Value::Object(labels.iter().map(|(channel, label)| (channel.to_string(), Value::from(label.clone()))).collect()));
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
    use crate::workspace::model::AggregatePhase;

    fn device(key: &str, name: &str) -> AggregateDevice {
        AggregateDevice { key: Some(key.into()), name: Some(name.into()), ..AggregateDevice::default() }
    }

    fn pair() -> Aggregate {
        Aggregate {
            devices: vec![device("Zen Quadro Synergy Core", "Quadro"), device("ZenStudioTB", "Studio+")],
            callback_master: Some("Quadro".into()),
            alignment: Some("aligned".into()),
            rate: Some(96000),
            buffer_size: Some(512),
            extra: Default::default(),
        }
    }

    #[test]
    fn the_worked_example_is_accepted() {
        check(&pair()).expect("the setup phase 0 measured");
        assert_eq!(device_name(&pair().devices[0]), "Quadro");
        assert_eq!(master_of(&pair()).map(device_name).as_deref(), Some("Quadro"));
    }

    #[test]
    fn a_device_needs_a_key_or_a_class_id_and_a_class_id_must_look_like_one() {
        let mut config = pair();
        config.devices[1] = AggregateDevice { name: Some("Studio+".into()), ..AggregateDevice::default() };
        assert_eq!(check(&config).unwrap_err(), "device 'Studio+': it needs a key or a clsid, which is how the driver finds it");
        config.devices[1] = AggregateDevice { clsid: Some("AE4A4452".into()), ..AggregateDevice::default() };
        assert!(check(&config).unwrap_err().contains("must be a class id in braces"));
        config.devices[1] = AggregateDevice { clsid: Some("{AE4A4452-A316-11E5-A113-080027F6C1F4}".into()), ..AggregateDevice::default() };
        check(&config).expect("a class id alone is enough");
    }

    #[test]
    fn a_device_is_named_once() {
        let mut config = pair();
        config.devices[1].key = Some("zen quadro synergy core".into());
        assert_eq!(check(&config).unwrap_err(), "device 'Studio+': this key is named twice");
        let mut config = pair();
        config.devices[1].name = Some("quadro".into());
        assert_eq!(
            check(&config).unwrap_err(),
            "device 'quadro': this name is used twice, and a channel's name would then say nothing"
        );
        let mut config = pair();
        config.devices[0].device_id = Some(DeviceId::from_serial("1"));
        config.devices[1].device_id = Some(DeviceId::from_serial("1"));
        assert_eq!(check(&config).unwrap_err(), "device 'Studio+': serial:1 is already another device in this aggregate");
    }

    #[test]
    fn a_trim_and_a_channel_must_be_numbers_they_could_be() {
        let mut config = pair();
        config.devices[0].input_trim = Some(TRIM_MAX + 1);
        assert!(check(&config).unwrap_err().contains("input_trim is 192001 samples"));
        config.devices[0].input_trim = Some(-28);
        check(&config).expect("a real trim is tens of samples, either way");
        config.devices[0].outputs = Some(vec![0, 1, AGGREGATE_CHANNEL_MAX + 1]);
        assert!(check(&config).unwrap_err().contains("outputs names channel 1024"));
        config.devices[0].outputs = Some(vec![0, 1, 1]);
        assert_eq!(check(&config).unwrap_err(), "device 'Quadro': outputs names channel 1 twice");
    }

    /// What a channel may be called: the interface carries 31 characters and the name in a DAW is
    /// built from it, so anything longer would arrive cut in half.
    #[test]
    fn a_channel_label_must_be_a_channel_that_exists_and_a_name_that_fits_on_the_interface() {
        let mut config = pair();
        config.devices[0].input_names = [(0, "Vocal mic".to_string()), (3, "Room".to_string())].into_iter().collect();
        config.devices[0].output_names = [(0, "Main L".to_string())].into_iter().collect();
        check(&config).expect("a label per channel, on either side");

        config.devices[0].input_names.insert(AGGREGATE_CHANNEL_MAX + 1, "Nowhere".into());
        assert_eq!(check(&config).unwrap_err(), "device 'Quadro': input_names labels channel 1024, which is outside 0..1023");

        let mut config = pair();
        config.devices[0].input_names = [(2, "   ".to_string())].into_iter().collect();
        assert_eq!(
            check(&config).unwrap_err(),
            "device 'Quadro': input_names gives channel 2 a blank label. Give it something to be called, or leave the channel out."
        );

        config.devices[0].input_names = [(2, "a".repeat(CHANNEL_NAME_MAX))].into_iter().collect();
        check(&config).expect("exactly what the interface carries is still a label");
        config.devices[0].input_names = [(2, "a".repeat(CHANNEL_NAME_MAX + 1))].into_iter().collect();
        let too_long = check(&config).unwrap_err();
        assert!(too_long.contains("which is 32 characters"), "{too_long}");
        assert!(too_long.contains("at most 31"), "{too_long}");

        config.devices[0].input_names.clear();
        config.devices[0].output_names = [(1, "Main\tR".to_string())].into_iter().collect();
        assert!(check(&config).unwrap_err().contains("has a control character in it"));
    }

    #[test]
    fn a_channel_label_is_exported_to_the_driver_and_a_side_with_none_is_left_out() {
        let mut config = pair();
        config.devices[0].input_names = [(0, "Vocal mic".to_string()), (10, "Room".to_string())].into_iter().collect();
        let document = export_document(&config);
        assert_eq!(document["devices"][0]["input_names"], serde_json::json!({ "0": "Vocal mic", "10": "Room" }));
        assert!(document["devices"][0].get("output_names").is_none(), "a side nobody has named is not written as an empty object");
        assert!(document["devices"][1].get("input_names").is_none());
    }

    /// Where the cable a phase is measured over runs, and the two ways of getting it wrong.
    #[test]
    fn a_phase_needs_both_ends_of_its_cable_and_an_interface_that_is_not_the_master() {
        let mut config = pair();
        config.devices[1].phase = Some(AggregatePhase { master_output: Some(8), input: Some(16), reference: None });
        check(&config).expect("a follower with a cable declared into it");

        config.devices[1].phase = Some(AggregatePhase { master_output: Some(8), input: None, reference: None });
        assert_eq!(
            check(&config).unwrap_err(),
            "device 'Studio+': phase needs input, which is this interface's own input channel the cable arrives on"
        );
        config.devices[1].phase = Some(AggregatePhase { master_output: None, input: Some(16), reference: None });
        assert!(check(&config).unwrap_err().contains("phase needs master_output"));

        config.devices[1].phase = Some(AggregatePhase { master_output: Some(8), input: Some(AGGREGATE_CHANNEL_MAX + 1), reference: None });
        assert!(check(&config).unwrap_err().contains("phase names input 1024"));

        // The interface that drives the callback is what everything else is measured against.
        let mut master = pair();
        master.devices[0].phase = Some(AggregatePhase { master_output: Some(8), input: Some(16), reference: None });
        assert!(check(&master).unwrap_err().contains("cannot be measured against itself"), "{:?}", check(&master));
    }

    /// The reference a calibration run writes beside the trim: any phase the hardware could
    /// measure, which is a few hundred samples either way, and nothing that could never be one.
    #[test]
    fn a_phase_reference_is_any_phase_the_hardware_could_measure() {
        let mut config = pair();
        config.devices[1].phase = Some(AggregatePhase { master_output: Some(8), input: Some(16), reference: Some(-307) });
        check(&config).expect("what the hardware measured in one of its sessions");
        config.devices[1].phase = Some(AggregatePhase { master_output: Some(8), input: Some(16), reference: Some(PHASE_REFERENCE_MAX + 1) });
        let why = check(&config).unwrap_err();
        assert!(why.starts_with("device 'Studio+': phase has a reference of 192001 samples"), "{why}");
        assert!(why.contains("measure the interfaces again"), "{why}");
        // A reference with no cable is still no cable: the half that is missing is what is said.
        config.devices[1].phase = Some(AggregatePhase { master_output: Some(8), input: None, reference: Some(-84) });
        assert!(check(&config).unwrap_err().contains("phase needs input"));
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
        let document = export_document(&config);
        assert_eq!(document["devices"][1]["phase"], serde_json::json!({ "master_output": 8, "input": 16 }));
        assert_eq!(document["devices"][1]["input_trim"], 28);
        assert!(document["devices"][0].get("phase").is_none(), "an interface with no cable declared says nothing");

        // And the reference goes beside the cable, which is where the driver reads it from.
        config.devices[1].input_trim = Some(60);
        config.devices[1].phase = Some(AggregatePhase { master_output: Some(8), input: Some(16), reference: Some(-84) });
        let document = export_document(&config);
        assert_eq!(document["devices"][1]["phase"], serde_json::json!({ "master_output": 8, "input": 16, "reference": -84 }));
        assert_eq!(document["devices"][1]["input_trim"], 60);
    }

    #[test]
    fn the_master_the_alignment_the_rate_and_the_buffer_must_be_ones_that_exist() {
        let mut config = pair();
        config.callback_master = Some("Octo".into());
        assert_eq!(check(&config).unwrap_err(), "callback_master is \"Octo\", which is not one of the devices");
        config = pair();
        config.alignment = Some("sample_accurate".into());
        assert!(check(&config).unwrap_err().starts_with("alignment must be one of aligned, lowest_latency"));
        config = pair();
        config.rate = Some(97000);
        assert!(check(&config).unwrap_err().starts_with("rate must be one of 32000, 44100"), "a rate the devices cannot be put at");
        config = pair();
        config.buffer_size = Some(500);
        assert!(check(&config).unwrap_err().contains("buffer_size must be one of 16, 32"));
    }

    /// The file the driver reads, field for field against its README's worked example.
    #[test]
    fn the_export_is_the_shape_the_driver_reads() {
        let config = Aggregate {
            devices: vec![
                AggregateDevice {
                    key: Some("Zen Quadro Synergy Core".into()),
                    name: Some("Quadro".into()),
                    device_id: Some(DeviceId::from_serial("1000000000001")),
                    ..AggregateDevice::default()
                },
                AggregateDevice {
                    clsid: Some("{AE4A4452-A316-11E5-A113-080027F6C1F4}".into()),
                    name: Some("Studio+".into()),
                    input_trim: Some(28),
                    inputs: Some(vec![0, 1, 2, 3]),
                    ..AggregateDevice::default()
                },
            ],
            callback_master: Some("Quadro".into()),
            alignment: Some("aligned".into()),
            rate: Some(96000),
            buffer_size: Some(512),
            extra: Default::default(),
        };
        let document = export_document(&config);
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
                "buffer_size": 512
            })
        );
        assert!(document["devices"][0].get("device_id").is_none(), "Gazelle's own field is not the driver's business");
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
        let document = export_document(&config);
        assert_eq!(document["ring_buffers"], 8);
        assert_eq!(document["devices"][0]["resample"], true);
        assert_eq!(serde_json::to_value(&config).unwrap()["ring_buffers"], 8, "and it is written back to the workspace");
    }

    #[test]
    fn an_empty_section_exports_an_empty_document() {
        assert_eq!(export_document(&Aggregate::default()), serde_json::json!({}));
    }
}
