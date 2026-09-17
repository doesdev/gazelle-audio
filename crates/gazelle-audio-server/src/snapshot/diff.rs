//! What differs between a snapshot and another (usually the present).
//!
//! The diff is the part that has to be excellent: recall (phase 6) is only as trustworthy as the
//! preview it shows before it writes anything, and a diff is useful on its own long before recall
//! exists — "what did I change since the take?" is a question the app can otherwise not answer.
//!
//! It is a plain function over two documents, so it needs no device and no hardware to test, and
//! so recall can ask "what would change?" with exactly the same code the user was shown.

use serde::Serialize;
use serde_json::Value as Json;
use std::collections::BTreeSet;

use crate::snapshot::model::{DeviceSnapshot, Snapshot, Unreadable, SECTIONS};

/// One difference, in one section.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Change {
    /// The full path inside the section, for recall to act on: `mixes[0].entries[3].level`.
    pub path: String,
    /// The same thing, for a person: "Mix 1 · strip 3 · level".
    pub label: String,
    pub kind: ChangeKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<Json>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<Json>,
    /// Why a value is unknown on one side or the other.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    /// Both sides have a value and they differ.
    Changed,
    /// The snapshot has it and the other side does not.
    OnlyInSnapshot,
    /// The other side has it and the snapshot does not.
    OnlyNow,
    /// One side could not read it, so whether it differs is not known.
    Unknown,
}

#[derive(Clone, Debug, Serialize)]
pub struct SectionDiff {
    pub section: String,
    /// "Inputs", "Mixer", …
    pub title: String,
    pub changes: Vec<Change>,
}

#[derive(Clone, Debug, Serialize)]
pub struct DeviceDiff {
    pub device_id: String,
    pub model: String,
    pub family: String,
    /// The snapshot has this device and the compared side does not: nothing of it can be applied.
    pub missing: bool,
    /// The compared side has it and the snapshot does not: it was attached since.
    pub added: bool,
    pub sections: Vec<SectionDiff>,
    pub changes: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct Diff {
    /// The snapshot being compared, as the list shows it.
    pub snapshot: Json,
    /// When the other side was read.
    pub compared_at: String,
    pub workspace: Vec<Change>,
    pub devices: Vec<DeviceDiff>,
    /// Everything counted: the number the page leads with.
    pub changes: usize,
    /// True when the two sides are the same in every section.
    pub same: bool,
}

/// Compare `snapshot` with `other` (the present, captured the same way).
pub fn diff(snapshot: &Snapshot, other: &Snapshot) -> Diff {
    let workspace = diff_json(
        "workspace",
        &serde_json::to_value(&snapshot.workspace).unwrap_or(Json::Null),
        &serde_json::to_value(&other.workspace).unwrap_or(Json::Null),
    );

    let ids: BTreeSet<&crate::device::descriptor::DeviceId> = snapshot.devices.keys().chain(other.devices.keys()).collect();
    let mut devices = Vec::new();
    for id in ids {
        let then = snapshot.devices.get(id);
        let now = other.devices.get(id);
        let describe = then.or(now);
        let sections = match (then, now) {
            (Some(then), Some(now)) => diff_device(then, now),
            // A device on one side only: its sections are not differences to apply, they are a
            // device that is not there. The page says so rather than listing every value.
            _ => Vec::new(),
        };
        let changes = sections.iter().map(|s| s.changes.len()).sum();
        devices.push(DeviceDiff {
            device_id: id.to_string(),
            model: describe.map(|d| d.model.clone()).unwrap_or_default(),
            family: describe.map(|d| d.family.clone()).unwrap_or_default(),
            missing: now.is_none(),
            added: then.is_none(),
            sections,
            changes,
        });
    }

    let changes = workspace.len() + devices.iter().map(|d| d.changes).sum::<usize>();
    let same = changes == 0 && devices.iter().all(|d| !d.missing && !d.added);
    Diff { snapshot: snapshot.summary(), compared_at: other.created.clone(), workspace, devices, changes, same }
}

fn diff_device(then: &DeviceSnapshot, now: &DeviceSnapshot) -> Vec<SectionDiff> {
    let mut sections = Vec::new();
    let names: Vec<String> = SECTIONS
        .iter()
        .map(|s| (*s).to_string())
        .chain(then.sections.keys().chain(now.sections.keys()).filter(|s| !SECTIONS.contains(&s.as_str())).cloned())
        .collect();
    let mut seen = BTreeSet::new();
    for section in names {
        if !seen.insert(section.clone()) {
            continue;
        }
        let mut changes = Vec::new();
        if section == "settings" && then.current_preset != now.current_preset {
            changes.push(Change {
                path: "current_preset".into(),
                label: "Settings · the device's current preset slot".into(),
                kind: ChangeKind::Changed,
                from: then.current_preset.map(Json::from),
                to: now.current_preset.map(Json::from),
                reason: None,
            });
        }
        changes.extend(diff_json(
            &section,
            then.sections.get(&section).unwrap_or(&Json::Null),
            now.sections.get(&section).unwrap_or(&Json::Null),
        ));
        changes.extend(unknowns(&section, &then.unreadable, &now.unreadable));
        if !changes.is_empty() {
            changes.sort_by(|a, b| a.path.cmp(&b.path));
            sections.push(SectionDiff { section: section.clone(), title: title(&section), changes });
        }
    }
    sections
}

/// A value one side could not read is not "unchanged": it is not known. Reported once per path,
/// with the reason from whichever side failed, so it can never pass for agreement.
fn unknowns(section: &str, then: &[Unreadable], now: &[Unreadable]) -> Vec<Change> {
    let prefix = format!("{section}.");
    let mut paths: BTreeSet<&str> = BTreeSet::new();
    for entry in then.iter().chain(now.iter()) {
        if let Some(rest) = entry.path.strip_prefix(&prefix) {
            paths.insert(rest);
        }
    }
    paths
        .into_iter()
        .map(|path| {
            let full = format!("{prefix}{path}");
            let when = |side: &[Unreadable], what: &str| {
                side.iter().find(|u| u.path == full).map(|u| format!("{what} {}", u.reason))
            };
            let reason = when(then, "when the snapshot was taken,")
                .into_iter()
                .chain(when(now, "now,"))
                .collect::<Vec<_>>()
                .join(" — ");
            Change {
                path: path.to_string(),
                label: label(section, path),
                kind: ChangeKind::Unknown,
                from: None,
                to: None,
                reason: Some(reason),
            }
        })
        .collect()
}

/// Every leaf at which two JSON values differ.
fn diff_json(section: &str, then: &Json, now: &Json) -> Vec<Change> {
    let mut changes = Vec::new();
    walk(section, String::new(), then, now, &mut changes);
    changes
}

fn walk(section: &str, path: String, then: &Json, now: &Json, out: &mut Vec<Change>) {
    if then == now {
        return;
    }
    // A value a whole side is missing (a section not captured this time, a device that answered
    // half of one) still reports value by value, so "what differs" is never a single line saying
    // the section does. An absent side counts as empty, not as a value of its own.
    let empty_object = Json::Object(serde_json::Map::new());
    let empty_array = Json::Array(Vec::new());
    let pair = match (then, now) {
        (Json::Object(_), Json::Object(_)) => Some((then, now)),
        (Json::Object(_), Json::Null) => Some((then, &empty_object)),
        (Json::Null, Json::Object(_)) => Some((&empty_object, now)),
        (Json::Array(_), Json::Array(_)) => Some((then, now)),
        (Json::Array(_), Json::Null) => Some((then, &empty_array)),
        (Json::Null, Json::Array(_)) => Some((&empty_array, now)),
        _ => None,
    };
    match pair {
        Some((Json::Object(a), Json::Object(b))) => {
            let keys: BTreeSet<&String> = a.keys().chain(b.keys()).collect();
            for key in keys {
                walk(section, join(&path, key), a.get(key).unwrap_or(&Json::Null), b.get(key).unwrap_or(&Json::Null), out);
            }
        }
        Some((Json::Array(a), Json::Array(b))) => {
            for i in 0..a.len().max(b.len()) {
                walk(section, format!("{path}[{i}]"), a.get(i).unwrap_or(&Json::Null), b.get(i).unwrap_or(&Json::Null), out);
            }
        }
        _ => out.push(Change {
            path: path.clone(),
            label: label(section, &path),
            kind: match (then.is_null(), now.is_null()) {
                (false, true) => ChangeKind::OnlyInSnapshot,
                (true, false) => ChangeKind::OnlyNow,
                _ => ChangeKind::Changed,
            },
            from: (!then.is_null()).then(|| then.clone()),
            to: (!now.is_null()).then(|| now.clone()),
            reason: None,
        }),
    }
}

fn join(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_string()
    } else {
        format!("{path}.{key}")
    }
}

/// The heading a section is shown under.
pub fn title(section: &str) -> String {
    match section {
        "workspace" => "Workspace".into(),
        "mixer" => "Mixer".into(),
        "routing" => "Routing".into(),
        "inputs" => "Inputs".into(),
        "outputs" => "Outputs".into(),
        "clock" => "Clock".into(),
        "settings" => "Settings".into(),
        other => words(other),
    }
}

/// What a person calls the value at this path.
///
/// A diff of raw paths is a diff nobody reads twice, and the names on the wire are the panels'
/// (`in_periph_id`, `emu_model`), not the app's. Anything with no entry here falls back to its own
/// name with the underscores taken out, so a value added to a schema is still legible.
fn label(section: &str, path: &str) -> String {
    let mut parts = vec![title(section)];
    let mixer = section == "mixer";
    for (at, part) in path.split('.').enumerate() {
        // A routing section's first part is the destination group's topology id, which is the
        // device's own name for it and not worth rewording beyond its underscores.
        if section == "routing" && at == 0 {
            parts.push(group_name(part));
            continue;
        }
        let (name, indices) = split_indices(part);
        parts.push(match (name, indices.as_slice()) {
            // A mix is the master and 32 strips, master first (P47, P48).
            ("mixes", [mix]) => format!("Mix {}", mix + 1),
            ("mixes", [mix, 0]) if mixer => format!("Mix {} · master", mix + 1),
            ("mixes", [mix, strip]) if mixer => format!("Mix {} · strip {strip}", mix + 1),
            // 64 link flags: sixteen pairs for each of the four mixes.
            ("links", [flag]) if mixer => format!("Mix {} · link pair {}", flag / 16 + 1, flag % 16 + 1),
            ("bank_configs", [slot]) => format!("slot {}", slot + 1),
            ("preamps" | "preamp_gains", [channel]) => format!("preamp {}", channel + 1),
            ("emulations", [channel]) => format!("preamp {} emulation", channel + 1),
            (name, []) => field_name(name),
            (name, indices) => format!("{} {}", field_name(name), indices.iter().map(|i| (i + 1).to_string()).collect::<Vec<_>>().join(" · ")),
        });
    }
    parts.join(" · ")
}

/// `mixes[0][1]` as `("mixes", [0, 1])`: a captured reply of one field is that field, so a mix is
/// an array of entries rather than an object with one named key.
fn split_indices(part: &str) -> (&str, Vec<usize>) {
    let mut name = part;
    let mut indices: Vec<usize> = Vec::new();
    while let Some(open) = name.rfind('[') {
        let Some(inner) = name[open..].strip_prefix('[').and_then(|s| s.strip_suffix(']')) else { break };
        let Ok(index) = inner.parse::<usize>() else { break };
        indices.insert(0, index);
        name = &name[..open];
    }
    (name, indices)
}

/// `MIXER_IN0` as `MIXER IN 0`.
fn group_name(id: &str) -> String {
    let spaced = id.replace('_', " ");
    match spaced.char_indices().rev().take_while(|(_, c)| c.is_ascii_digit()).last() {
        Some((at, _)) if at > 0 => format!("{} {}", &spaced[..at], &spaced[at..]),
        _ => spaced,
    }
}

fn words(name: &str) -> String {
    let spaced = name.replace('_', " ");
    let mut chars = spaced.chars();
    chars.next().map_or(spaced.clone(), |first| first.to_uppercase().collect::<String>() + chars.as_str())
}

/// The panels' field names, said the way the app says them elsewhere.
fn field_name(name: &str) -> String {
    match name {
        "phantom" | "pre_phantom" => "48V".into(),
        "phase_inv" | "phase" => "phase invert".into(),
        "hpf" => "high-pass filter".into(),
        "pre_type" | "type" => "type".into(),
        "in_periph_id" => "source".into(),
        "in_chann" => "source channel".into(),
        "bank_idx" => "destination".into(),
        "linked" => "link".into(),
        "emu_model" => "emulation".into(),
        "ch_swap" => "swap".into(),
        "target" => "microphone".into(),
        "pattern" => "polar pattern".into(),
        "rate_index" => "sample rate".into(),
        "sync_source" => "clock source".into(),
        "spdif_src" => "S/PDIF sample-rate converter".into(),
        "panning_law" => "panning law".into(),
        "hard_mute" => "hard mute".into(),
        "dc_coupled_in" => "DC coupling, inputs".into(),
        "dc_coupled_out" => "DC coupling, outputs".into(),
        "mic_volume" => "microphone level".into(),
        "to_hp1" => "to HP1".into(),
        "to_hp2" => "to HP2".into(),
        "to_monitor" => "to Monitor".into(),
        "hp1" => "HP1".into(),
        "hp2" => "HP2".into(),
        "adc" => "ADC".into(),
        "spdif" => "S/PDIF".into(),
        "adat" => "ADAT".into(),
        // Anything not named here reads as itself with its underscores taken out, lower case like
        // the rest of a label's tail, so a value added to a schema is still legible.
        other => other.replace('_', " "),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::descriptor::DeviceId;
    use crate::snapshot::model::DeviceSnapshot;
    use crate::workspace::model::Workspace;
    use serde_json::json;
    use std::collections::BTreeMap;

    fn device() -> DeviceSnapshot {
        DeviceSnapshot {
            family: "quadro".into(),
            model: "Zen Quadro".into(),
            read_at: "2026-09-17T20:15:00Z".into(),
            current_preset: Some(1),
            sections: BTreeMap::from([
                ("mixer".into(), json!({"mixes[0]": [{"level": 64, "pan": 32, "mute": 0, "solo": 0}, {"level": 40, "pan": 32, "mute": 0, "solo": 0}], "links": [{"linked": 0}]})),
                ("routing".into(), json!({"MIXER_IN0": {"bank_idx": 7, "bank_configs": [{"in_periph_id": 3, "in_chann": 0}]}})),
                ("inputs".into(), json!({"preamps": [{"type": 0, "phantom": 0}], "preamp_gains": [20], "links": {"preamp": [0]}})),
                ("outputs".into(), json!({"volumes": [64, 64, 64, 64], "hard_mute": 0})),
                ("clock".into(), json!({"sync_source": 0, "rate_index": 2})),
                ("settings".into(), json!({"panning_law": 0, "brightness": 60})),
            ]),
            unreadable: Vec::new(),
        }
    }

    fn snapshot(devices: BTreeMap<DeviceId, DeviceSnapshot>) -> Snapshot {
        let mut s = Snapshot::new("Take 1".into(), String::new(), Workspace::default());
        s.devices = devices;
        s
    }

    fn only(diff: &Diff, section: &str) -> Vec<Change> {
        diff.devices
            .iter()
            .flat_map(|d| d.sections.iter())
            .filter(|s| s.section == section)
            .flat_map(|s| s.changes.clone())
            .collect()
    }

    #[test]
    fn a_snapshot_compared_with_itself_shows_nothing() {
        let one = snapshot(BTreeMap::from([(DeviceId::loopback(0), device())]));
        let same = diff(&one, &one);
        assert!(same.same, "{:?}", same.devices);
        assert_eq!(same.changes, 0);
        assert!(same.workspace.is_empty());
        assert!(same.devices[0].sections.is_empty(), "a section with nothing in it is not listed");
    }

    #[test]
    fn a_change_in_each_section_is_found_and_named_where_it_belongs() {
        let before = snapshot(BTreeMap::from([(DeviceId::loopback(0), device())]));
        let mut after = before.clone();
        {
            let d = after.devices.get_mut(&DeviceId::loopback(0)).expect("device");
            d.sections.get_mut("mixer").unwrap()["mixes[0]"][1]["level"] = json!(50);
            d.sections.get_mut("routing").unwrap()["MIXER_IN0"]["bank_configs"][0]["in_chann"] = json!(1);
            d.sections.get_mut("inputs").unwrap()["preamps"][0]["phantom"] = json!(1);
            d.sections.get_mut("outputs").unwrap()["volumes"][2] = json!(50);
            d.sections.get_mut("clock").unwrap()["sync_source"] = json!(5);
            d.sections.get_mut("settings").unwrap()["panning_law"] = json!(2);
            d.current_preset = Some(3);
        }
        after.workspace.aliases.insert(DeviceId::loopback(0), "Desk".into());

        let changed = diff(&before, &after);
        assert!(!changed.same);
        let sections: Vec<&str> = changed.devices[0].sections.iter().map(|s| s.section.as_str()).collect();
        assert_eq!(sections, ["inputs", "mixer", "routing", "outputs", "clock", "settings"], "sections keep their reading order");

        let mixer = only(&changed, "mixer");
        assert_eq!(mixer.len(), 1);
        assert_eq!(mixer[0].path, "mixes[0][1].level");
        assert_eq!(mixer[0].label, "Mixer · Mix 1 · strip 1 · level");
        assert_eq!((mixer[0].from.clone(), mixer[0].to.clone()), (Some(json!(40)), Some(json!(50))));

        assert_eq!(only(&changed, "routing")[0].label, "Routing · MIXER IN 0 · slot 1 · source channel");
        assert_eq!(only(&changed, "inputs")[0].label, "Inputs · preamp 1 · 48V");
        assert_eq!(only(&changed, "outputs")[0].label, "Outputs · volumes 3");
        assert_eq!(only(&changed, "clock")[0].label, "Clock · clock source");
        let settings = only(&changed, "settings");
        assert_eq!(settings.len(), 2, "the panning law and the preset slot: {settings:?}");
        assert!(settings.iter().any(|c| c.path == "current_preset" && c.to == Some(json!(3))));

        assert_eq!(changed.workspace.len(), 1);
        assert_eq!(changed.workspace[0].label, "Workspace · aliases · loopback-0");
        assert_eq!(changed.changes, 8);
    }

    #[test]
    fn a_value_neither_side_could_read_is_unknown_rather_than_unchanged() {
        let mut before = snapshot(BTreeMap::from([(DeviceId::loopback(0), device())]));
        before.devices.get_mut(&DeviceId::loopback(0)).unwrap().unreadable.push(Unreadable {
            path: "inputs.emulations".into(),
            reason: "device loopback-0 refused 'get_mic_emulations'".into(),
        });
        let after = snapshot(BTreeMap::from([(DeviceId::loopback(0), device())]));
        let d = diff(&before, &after);
        let inputs = only(&d, "inputs");
        assert_eq!(inputs.len(), 1);
        assert_eq!(inputs[0].kind, ChangeKind::Unknown);
        assert_eq!(inputs[0].label, "Inputs · emulations");
        assert!(inputs[0].reason.as_deref().unwrap().starts_with("when the snapshot was taken,"));
        assert!(!d.same, "an unread value is not agreement");
    }

    #[test]
    fn a_device_the_snapshot_has_and_the_present_does_not_is_marked_missing_not_diffed() {
        let before = snapshot(BTreeMap::from([(DeviceId::loopback(0), device())]));
        let after = snapshot(BTreeMap::new());
        let d = diff(&before, &after);
        assert!(d.devices[0].missing && !d.devices[0].added);
        assert!(d.devices[0].sections.is_empty(), "nothing of a device that is not there can be applied");
        assert!(!d.same);

        let back = diff(&after, &before);
        assert!(back.devices[0].added && !back.devices[0].missing);
    }

    #[test]
    fn a_value_one_side_has_and_the_other_does_not_says_which() {
        let before = snapshot(BTreeMap::from([(DeviceId::loopback(0), device())]));
        let mut after = before.clone();
        after.devices.get_mut(&DeviceId::loopback(0)).unwrap().sections.remove("clock");
        let d = diff(&before, &after);
        let clock = only(&d, "clock");
        assert_eq!(clock.len(), 2);
        assert!(clock.iter().all(|c| c.kind == ChangeKind::OnlyInSnapshot && c.to.is_none()));
        assert_eq!(diff(&after, &before).devices[0].sections[0].changes[0].kind, ChangeKind::OnlyNow);
    }

    #[test]
    fn group_and_field_names_read_as_the_app_says_them() {
        assert_eq!(group_name("MIXER_IN0"), "MIXER IN 0");
        assert_eq!(group_name("SPDIF_OUT0"), "SPDIF OUT 0");
        assert_eq!(group_name("MUTE0"), "MUTE 0");
        assert_eq!(label("inputs", "links.preamp"), "Inputs · links · preamp");
        assert_eq!(label("outputs", "talkback.to_hp1"), "Outputs · talkback · to HP1");
        assert_eq!(label("clock", "spdif_src"), "Clock · S/PDIF sample-rate converter");
        assert_eq!(label("settings", "dc_coupled_out"), "Settings · DC coupling, outputs");
        // A name with no entry is still legible.
        assert_eq!(label("settings", "some_new_thing"), "Settings · some new thing");
    }
}
