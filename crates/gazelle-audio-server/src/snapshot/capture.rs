//! Reading a device's state into a snapshot.
//!
//! **Nothing here writes to a device.** Every read is a `get_*` through the ordinary request path
//! or a value from the last cyclic report, and each one is fresh: the mixer and routing caches the
//! pages keep (P80) are the client's, and a snapshot that quoted them would record what someone
//! looked at rather than what the device holds.
//!
//! What is captured is an **allow list per family**, not everything the device reports. A value
//! whose meaning is not established is not captured, because a snapshot is a promise that its
//! contents can one day be put back:
//!
//! - the **test oscillator** (`osc_*`, the Quadro's `freq_*`/`level`/`mute_*`): a tone is not a
//!   state anyone wants back (spec §2.1);
//! - **power**, `usb_mode`, `set_monitor_out` and `set_usb_channels` (meaning unknown, P63, P76);
//! - **talkback's momentary switch** (`talkback_on`): it is a button being held, not a setting.
//!   Its level and destinations are captured;
//! - **meters, lock and presence flags, sync frequency, available channel counts**: what the
//!   device is doing, not how it is set;
//! - **effects and reverb**: out of scope until the effects work lands;
//! - **`reserved*`**: unknown by name.
//!
//! The device's **current preset slot** is recorded but nothing that would recall it (spec §2.4).

use gazelle_audio_protocol::payload::{PayloadValues, Value};
use serde_json::{json, Map, Value as Json};
use std::collections::{BTreeMap, HashMap};

use crate::device::descriptor::{DeviceDescriptor, DeviceId};
use crate::device::manager::DeviceManager;
use crate::error::ServerError;
use crate::snapshot::model::{DeviceSnapshot, Snapshot, Unreadable};
use crate::snapshot::time::now_rfc3339;
use crate::value::value_to_json;
use crate::workspace::model::Workspace;
use crate::workspace::topology;

/// The cyclic report both models carry their state in.
const STATE_REPORT: u32 = 0x73;

/// How many mixes each model has.
const MIXES: u32 = 4;

/// Where a captured value comes from.
enum Source {
    /// A `get_*` command, with the header selector it takes.
    Command { name: &'static str, ext3: Option<u32> },
    /// A field of the last cyclic state report.
    Cyclic(&'static str),
}

/// One value a snapshot holds: its section, where it sits in that section, and how it is read.
struct Read {
    section: &'static str,
    /// Dotted key within the section; `links.preamp` nests.
    key: String,
    source: Source,
}

fn command(section: &'static str, key: &str, name: &'static str, ext3: Option<u32>) -> Read {
    Read { section, key: key.to_string(), source: Source::Command { name, ext3 } }
}

fn cyclic(section: &'static str, key: &str, field: &'static str) -> Read {
    Read { section, key: key.to_string(), source: Source::Cyclic(field) }
}

/// Every read a snapshot of this family makes, in the order it makes them.
///
/// Routing is keyed by topology group id, not wire position, so a snapshot survives a topology
/// re-extraction that reorders groups (spec §2.5).
fn plan(family: &str) -> Vec<Read> {
    let mut reads = Vec::new();

    for mix in 0..MIXES {
        reads.push(command("mixer", &format!("mixes[{mix}]"), "get_mixer", Some(mix)));
    }
    reads.push(command("mixer", "links", "get_mixer_links", None));

    for (id, position) in topology::destination_groups(family).unwrap_or_default() {
        reads.push(command("routing", &id, "get_routing", Some(position)));
    }

    reads.push(cyclic("inputs", "preamps", "preamps"));
    reads.push(cyclic("inputs", "preamp_gains", "preamp_gains"));
    reads.push(cyclic("inputs", "adat_gains", "adat_gains"));
    reads.push(cyclic("inputs", "spdif_gains", "spdif_gains"));
    reads.push(command("inputs", "links.preamp", "get_preamps_links", None));
    reads.push(command("inputs", "links.adat", "get_adats_links", None));
    reads.push(command("inputs", "links.spdif", "get_spdifs_links", None));

    reads.push(cyclic("outputs", "trims.adc", "adc_trim"));
    reads.push(cyclic("outputs", "trims.monitor", "monitor_trim"));
    reads.push(cyclic("outputs", "trims.line_out", "line_out_trim"));

    reads.push(cyclic("clock", "sync_source", "sync_source"));
    reads.push(cyclic("clock", "rate_index", "base_index"));

    reads.push(cyclic("settings", "brightness", "brightness"));

    match family {
        "quadro" => {
            // Preamps 1-2 only: the reply declares two entries, so an Edge Quadro across preamps
            // 3-4 is half-captured and must never be recalled from a guess (P81, spec §2.2).
            reads.push(command("inputs", "emulations", "get_mic_emulations", None));
            reads.push(cyclic("outputs", "volumes", "volumes"));
            reads.push(cyclic("outputs", "hard_mute", "hard_mute"));
            reads.push(command("outputs", "trim_configs", "get_trim_configs", None));
            reads.push(command("settings", "panning_law", "get_panning_law", None));
            reads.push(cyclic("settings", "dc_coupled_in", "dc_coupled_in"));
            reads.push(cyclic("settings", "dc_coupled_out", "dc_coupled_out"));
        }
        "studio" => {
            reads.push(cyclic("inputs", "line_gains", "line_gains"));
            reads.push(command("inputs", "links.line", "get_lines_links", None));
            for (key, field) in [
                ("monitor.volume", "monitor_vol"),
                ("monitor.mute", "monitor_mute"),
                ("hp1.volume", "hp1_vol"),
                ("hp1.mute", "hp1_mute"),
                ("hp2.volume", "hp2_vol"),
                ("hp2.mute", "hp2_mute"),
                ("line_out.volume", "line_out_vol"),
                ("line_out.mute", "line_out_mute"),
                ("reamp.volume", "reamp_vol"),
                ("reamp.mute", "reamp_mute"),
                // Talkback's level and where it goes are settings; the talk button is not.
                ("talkback.mic_volume", "tb_mic_volume"),
                ("talkback.to_hp1", "hp1_enabled"),
                ("talkback.to_hp2", "hp2_enabled"),
                ("talkback.to_monitor", "mon_enabled"),
            ] {
                reads.push(cyclic("outputs", key, field));
            }
            reads.push(cyclic("clock", "spdif_src", "spdif_src"));
        }
        _ => {}
    }
    reads
}

/// Capture the workspace and every attached device.
///
/// A device whose model is unknown is listed in the snapshot with no sections rather than guessed
/// at; a device that answers nothing gets a snapshot full of [`Unreadable`] rather than zeros.
pub async fn capture(
    devices: &DeviceManager,
    workspace: Workspace,
    name: String,
    note: String,
    force_dry_run: bool,
) -> Result<Snapshot, ServerError> {
    // In dry run the request path stops before the device, so every read answers nothing
    // (decision 0012, spec §2.2). A snapshot of nothing is worse than no snapshot: it would look
    // like a record of a device that had every value unset.
    if force_dry_run {
        return Err(ServerError::Unsupported(
            "a snapshot cannot be taken in dry run: the server answers no reads, so there would be nothing to record".into(),
        ));
    }
    let mut snapshot = Snapshot::new(name, note, workspace);
    for descriptor in devices.descriptors() {
        let id = descriptor.id.clone();
        snapshot.devices.insert(id.clone(), capture_device(devices, &id, &descriptor).await);
    }
    Ok(snapshot)
}

async fn capture_device(devices: &DeviceManager, id: &DeviceId, descriptor: &DeviceDescriptor) -> DeviceSnapshot {
    let read_at = now_rfc3339();
    let mut device = DeviceSnapshot {
        family: descriptor.family.clone().unwrap_or_default(),
        model: descriptor.model.clone().unwrap_or_default(),
        read_at,
        current_preset: None,
        sections: BTreeMap::new(),
        unreadable: Vec::new(),
    };
    let Some(family) = descriptor.family.clone() else {
        device.unreadable.push(Unreadable {
            path: String::new(),
            reason: "this server has no command registry for the device's model, so nothing can be read from it".into(),
        });
        return device;
    };
    let handle = match devices.handle(id) {
        Ok(handle) => handle,
        Err(e) => {
            device.unreadable.push(Unreadable { path: String::new(), reason: e.to_string() });
            return device;
        }
    };

    // One read of the state report for every cyclic value, so they are one consistent moment
    // rather than a value from each of several reports.
    let state = devices.cyclic(id, STATE_REPORT);
    device.current_preset = state.as_ref().and_then(|s| s.get("current_preset")).and_then(as_i64);

    for read in plan(&family) {
        let path = format!("{}.{}", read.section, read.key);
        let value = match &read.source {
            Source::Command { name, ext3 } => {
                match handle.request(name, PayloadValues::default(), *ext3, false).await {
                    Err(e) => Err(e.to_string()),
                    Ok(outcome) => match (outcome.response, outcome.response_error) {
                        (_, Some(error)) => Err(format!("{name} came back but could not be read: {error}")),
                        (Some(fields), None) => Ok(fields_json(&fields)),
                        (None, None) => Err(format!("{name} was answered with nothing")),
                    },
                }
            }
            Source::Cyclic(field) => match state.as_ref() {
                None => Err(format!(
                    "the device has not pushed its state report (0x{STATE_REPORT:X}) since the server started, so '{field}' is unknown"
                )),
                Some(fields) => match fields.get(*field) {
                    Some(value) => Ok(value_to_json(value)),
                    None => Err(format!("the device's state report does not carry '{field}'")),
                },
            },
        };
        match value {
            Ok(value) => insert(device.sections.entry(read.section.to_string()).or_insert_with(|| json!({})), &read.key, value),
            Err(reason) => device.unreadable.push(Unreadable { path, reason }),
        }
    }
    device
}

fn as_i64(value: &Value) -> Option<i64> {
    match value {
        Value::U64(n) => i64::try_from(*n).ok(),
        Value::I64(n) => Some(*n),
        _ => None,
    }
}

/// A decoded reply as JSON. A reply of one field is that field's value: `get_panning_law` answers
/// a panning law, and `{"panning": 2}` would put the wire's field name into every diff line.
fn fields_json(fields: &HashMap<String, Value>) -> Json {
    if fields.len() == 1 {
        if let Some(value) = fields.values().next() {
            return value_to_json(value);
        }
    }
    let mut map = Map::new();
    for (name, value) in fields {
        map.insert(name.clone(), value_to_json(value));
    }
    Json::Object(map)
}

/// Put `value` at a dotted `key` (`links.preamp`, `mixes[0]`), making the objects it passes
/// through. A bracketed tail stays part of its key: `mixes[0]` is one entry, not an array.
fn insert(into: &mut Json, key: &str, value: Json) {
    let Some((head, rest)) = key.split_once('.') else {
        if let Some(map) = into.as_object_mut() {
            map.insert(key.to_string(), value);
        }
        return;
    };
    let Some(map) = into.as_object_mut() else { return };
    insert(map.entry(head.to_string()).or_insert_with(|| json!({})), rest, value);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_plan_reads_every_section_of_both_models_and_nothing_it_cannot_explain() {
        for family in ["quadro", "studio"] {
            let plan = plan(family);
            let sections: std::collections::BTreeSet<&str> = plan.iter().map(|r| r.section).collect();
            assert_eq!(
                sections.into_iter().collect::<Vec<_>>(),
                ["clock", "inputs", "mixer", "outputs", "routing", "settings"],
                "{family} misses a section"
            );
            // Four mixes and the link flags, and one read per destination group.
            assert_eq!(plan.iter().filter(|r| r.section == "mixer").count(), 5);
            let groups = topology::destination_groups(family).expect("topology").len();
            assert_eq!(plan.iter().filter(|r| r.section == "routing").count(), groups);

            let cyclic: Vec<&str> = plan
                .iter()
                .filter_map(|r| match r.source {
                    Source::Cyclic(field) => Some(field),
                    Source::Command { .. } => None,
                })
                .collect();
            for never in ["osc_level", "osc_freq_left", "level", "freq_left", "power_on", "usb_mode", "talkback_on", "pm_bank_src"] {
                assert!(!cyclic.contains(&never), "{family} captures '{never}', which has no meaning worth putting back");
            }
            assert!(!cyclic.iter().any(|f| f.starts_with("peaks") || f.starts_with("reserved") || f.ends_with("_present")));
        }
        // Per-model reads go only to the model that has them.
        let quadro: Vec<String> = plan("quadro").iter().map(|r| r.key.clone()).collect();
        let studio: Vec<String> = plan("studio").iter().map(|r| r.key.clone()).collect();
        assert!(quadro.contains(&"emulations".to_string()) && !studio.contains(&"emulations".to_string()));
        assert!(quadro.contains(&"panning_law".to_string()) && !studio.contains(&"panning_law".to_string()));
        assert!(studio.contains(&"clock.spdif_src".to_string()) || studio.contains(&"spdif_src".to_string()));
        assert!(studio.contains(&"links.line".to_string()) && !quadro.contains(&"links.line".to_string()));
        assert!(quadro.contains(&"hard_mute".to_string()) && !studio.contains(&"hard_mute".to_string()));
        assert!(plan("zen").iter().all(|r| r.section != "routing"), "an unknown family plans no routing reads");
    }

    #[test]
    fn dotted_keys_nest_and_bracketed_ones_do_not() {
        let mut section = json!({});
        insert(&mut section, "links.preamp", json!([1]));
        insert(&mut section, "links.adat", json!([0]));
        insert(&mut section, "mixes[0]", json!({"entries": []}));
        assert_eq!(section, json!({"links": {"preamp": [1], "adat": [0]}, "mixes[0]": {"entries": []}}));
    }

    #[test]
    fn a_reply_of_one_field_is_that_value_rather_than_the_wire_s_name_for_it() {
        assert_eq!(fields_json(&HashMap::from([("panning".to_string(), Value::U64(2))])), json!(2));
        let two = HashMap::from([("bank_idx".to_string(), Value::U64(1)), ("bank_configs".to_string(), Value::List(vec![]))]);
        assert_eq!(fields_json(&two), json!({"bank_idx": 1, "bank_configs": []}));
    }
}
