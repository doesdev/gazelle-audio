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
use crate::workspace::model::{Aggregate, AggregateDevice, AGGREGATE_CHANNEL_MAX, ALIGNMENTS, TRIM_MAX};

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
    for (name, value) in &device.extra {
        out.insert(name.clone(), value.clone());
    }
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::descriptor::DeviceId;

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
