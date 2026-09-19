//! A recall **plan**: the ordered list of commands that *would* put a snapshot back, with the bytes
//! each one would send and every guard attached to it as data.
//!
//! **Preparing a plan sends nothing.** It reads every device fresh (the same capture a comparison
//! makes), diffs the snapshot against what came back, and turns each difference into the command
//! that would undo it, in a volume-safe order. Applying a plan is not built and waits for a
//! hardware session; [`crate::http::recall`] holds the seam, disabled.
//!
//! The order, and why:
//!
//! 1. **Silence the outputs**: the Quadro's hard mute, the Studio+'s five output mutes, as the
//!    vendor panel does around a session restore.
//! 2. **Clock**, which interrupts everything and has to settle before anything is judged by ear.
//! 3. **Device settings**, then **DC coupling**, both while silenced: DC on an output reaches
//!    whatever is connected.
//! 4. **Inputs**, in the panels' own order: 48V *off* first, then type (it sets the gain range),
//!    then gains, phase, mic emulation, and **48V *on* last**, still silenced, for its thump.
//! 5. **Routing**, one destination group at a time, since each `set_routing` replaces 32 slots.
//! 6. **Mixer**, quieter first: a strip whose level is being cut moves before one being raised.
//! 7. **Output volumes and trims**, still silenced.
//! 8. **Restore**: dim, the mutes the snapshot had, and the hard mute off **last**.
//!
//! Every step carries the guards it is subject to in `blocked_by`, so a plan is inspectable without
//! re-deriving why a step would or would not be sent.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value as Json};
use std::collections::{BTreeMap, BTreeSet};

use crate::device::manager::DeviceManager;
use crate::error::ServerError;
use crate::snapshot::capture::capture;
use crate::snapshot::diff::{diff, Change, ChangeKind, Diff};
use crate::snapshot::model::{DeviceSnapshot, Snapshot};
use crate::snapshot::time::now_rfc3339;
use crate::snapshot::writers::{writer_for, GainKind, Part, Target, Writer, PARTS, STUDIO_OUTPUT_KEYS};
use crate::value::to_hex;
use crate::workspace::model::Workspace;
use crate::workspace::topology;

/// An output raised by more than this needs its own tick.
pub const RAISE_THRESHOLD_DB: i64 = 6;

/// What the caller asked for. Everything is optional: with an empty request the plan takes the
/// defaults, which is what the Workspace page's preview shows.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct RecallRequest {
    /// Per-part opt-in, overriding [`Part::default_on`].
    pub parts: BTreeMap<String, bool>,
    /// The ticks the dangerous parts need every time, never remembered.
    pub confirm: BTreeMap<String, bool>,
    /// "I have checked" for outputs raised by more than [`RAISE_THRESHOLD_DB`].
    pub confirm_raised_outputs: bool,
    /// Only these devices, when given.
    pub devices: Option<Vec<String>>,
}

impl RecallRequest {
    fn chosen(&self, part: Part) -> bool {
        self.parts.get(part.name()).copied().unwrap_or_else(|| part.default_on())
    }

    fn confirmed(&self, part: Part) -> bool {
        !part.needs_confirming() || self.confirm.get(part.name()).copied().unwrap_or(false)
    }

    fn wants(&self, device_id: &str) -> bool {
        self.devices.as_ref().is_none_or(|ids| ids.iter().any(|id| id == device_id))
    }
}

/// One command the plan would send.
#[derive(Clone, Debug, Serialize)]
pub struct Step {
    pub device_id: String,
    pub model: String,
    /// The opt-in part, by [`Part::name`].
    pub part: String,
    pub title: String,
    /// What a person reads: "Mixer · Mix 1 · strip 3".
    pub label: String,
    pub command: String,
    /// The per-request header selector. Always `null`: every `set_*` carries its own.
    pub ext3: Option<u32>,
    pub args: Json,
    /// The exact bytes this step would send, as the server's own dry run renders them.
    pub bytes: String,
    pub bytes_len: usize,
    /// The diff paths this one command puts back.
    pub paths: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<Json>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<Json>,
    /// Anything a person should read before this is sent: "raised by 18 dB", "switches 48V ON".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// The guards not satisfied. Empty means this step would be sent as the plan stands.
    pub blocked_by: Vec<String>,
}

/// A captured value recall will not put back, and why. Never silent.
#[derive(Clone, Debug, Serialize)]
pub struct Excluded {
    pub device_id: String,
    pub section: String,
    pub path: String,
    pub label: String,
    /// `no_writer`, `withheld`, `unmapped`, `unreadable`, `incomplete`, `device_missing` or
    /// `not_chosen`.
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    pub reason: String,
}

/// One opt-in part, as the dialog draws it.
#[derive(Clone, Debug, Serialize)]
pub struct PartPlan {
    pub name: String,
    pub title: String,
    pub default_on: bool,
    pub chosen: bool,
    pub needs_confirming: bool,
    pub confirmed: bool,
    pub steps: usize,
}

/// An output the snapshot would make louder by more than [`RAISE_THRESHOLD_DB`].
#[derive(Clone, Debug, Serialize)]
pub struct RaisedOutput {
    pub device_id: String,
    pub output: String,
    /// Where it is now, in dB of attenuation (96 is -inf), or `null` when nobody could read it.
    pub now: Option<i64>,
    /// Where the snapshot puts it.
    pub snapshot: i64,
    /// How far this raises it, or `null` when the present is unknown.
    pub raised_db: Option<i64>,
}

/// A device in the snapshot, and whether anything of it can be recalled.
#[derive(Clone, Debug, Serialize)]
pub struct DevicePlan {
    pub device_id: String,
    pub model: String,
    pub family: String,
    pub missing: bool,
    pub steps: usize,
    pub excluded: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct RecallPlan {
    /// The snapshot being put back, as the list shows it.
    pub snapshot: Json,
    pub prepared_at: String,
    /// Whether the devices were read just now. False in dry run, where the server answers no reads:
    /// the plan then lists every recallable value, because nothing is known to be right already.
    pub current_state_read: bool,
    pub raise_threshold_db: i64,
    pub parts: Vec<PartPlan>,
    pub devices: Vec<DevicePlan>,
    pub steps: Vec<Step>,
    pub excluded: Vec<Excluded>,
    pub raised_outputs: Vec<RaisedOutput>,
    /// Inputs this plan would switch 48V **on** for, by name.
    pub phantom_on: Vec<String>,
    /// How many of `steps` no guard is holding back.
    pub ready: usize,
    /// Workspace-side differences. Layout only: recalling them sends nothing to a device,
    /// so they are counted here and are not steps.
    pub workspace_changes: usize,
    /// Always false. A plan is a description; sending it is phase 6.
    pub sent: bool,
    pub note: String,
}

/// Prepare a plan: read every device fresh, compare, and describe what would be sent.
///
/// Reads only. The bytes come from the server's own dry run, so what the plan shows is what the
/// wire would carry.
pub async fn prepare(
    devices: &DeviceManager,
    snapshot: &Snapshot,
    workspace: Workspace,
    force_dry_run: bool,
    request: &RecallRequest,
) -> Result<RecallPlan, ServerError> {
    // Guard 1: nothing is planned from a stale view. In dry run the server answers no reads
    // at all, so instead of pretending, the plan says the present is unknown and lists everything.
    let now = if force_dry_run {
        unread_present(devices, workspace)
    } else {
        capture(devices, workspace, "now".into(), String::new(), false).await?
    };
    let differences = diff(snapshot, &now);
    let mut plan = build(snapshot, &now, &differences, request, !force_dry_run);
    render(devices, &mut plan).await;
    Ok(plan)
}

/// What the present looks like when the server will not read it: the devices that are attached, and
/// not one of their values. Every captured value then reads as "only in the snapshot".
fn unread_present(devices: &DeviceManager, workspace: Workspace) -> Snapshot {
    let mut present = Snapshot::new("now".into(), String::new(), workspace);
    for descriptor in devices.descriptors() {
        present.devices.insert(
            descriptor.id.clone(),
            DeviceSnapshot {
                family: descriptor.family.clone().unwrap_or_default(),
                model: descriptor.model.clone().unwrap_or_default(),
                read_at: now_rfc3339(),
                current_preset: None,
                sections: BTreeMap::new(),
                unreadable: Vec::new(),
            },
        );
    }
    present
}

/// One command to be built, and the diff paths that asked for it.
struct Planned {
    target: Target,
    paths: Vec<String>,
    from: Option<Json>,
    to: Option<Json>,
    label: String,
}

fn build(snapshot: &Snapshot, now: &Snapshot, differences: &Diff, request: &RecallRequest, read: bool) -> RecallPlan {
    let mut steps: Vec<Step> = Vec::new();
    let mut excluded: Vec<Excluded> = Vec::new();
    let mut raised: Vec<RaisedOutput> = Vec::new();
    let mut phantom_on: Vec<String> = Vec::new();
    let mut device_plans: Vec<DevicePlan> = Vec::new();

    for device in &differences.devices {
        // A device attached now and not in the snapshot has nothing to put back.
        if device.added {
            continue;
        }
        let id = device.device_id.clone();
        let before = steps.len();
        let excluded_before = excluded.len();
        let Some(recorded) = snapshot.devices.get(&crate::device::descriptor::DeviceId(id.clone())) else { continue };
        if device.missing || !request.wants(&id) {
            excluded.push(Excluded {
                device_id: id.clone(),
                section: String::new(),
                path: String::new(),
                label: device.model.clone(),
                kind: if device.missing { "device_missing" } else { "not_chosen" },
                command: None,
                reason: if device.missing {
                    format!("{} is in the snapshot and is not attached now, so none of it can be put back", if device.model.is_empty() { &id } else { &device.model })
                } else {
                    "this device was not chosen for recall".into()
                },
            });
            device_plans.push(DevicePlan { device_id: id, model: device.model.clone(), family: device.family.clone(), missing: device.missing, steps: 0, excluded: 1 });
            continue;
        }
        let family = device.family.clone();
        // Both sides' unread paths, not only the snapshot's: a value the devices would not give up
        // just now is a stale view, and the first recall guard is that nothing is sent from one.
        let present = now.devices.get(&crate::device::descriptor::DeviceId(id.clone()));
        let unreadable: Vec<&str> = recorded
            .unreadable
            .iter()
            .chain(present.into_iter().flat_map(|d| d.unreadable.iter()))
            .map(|u| u.path.as_str())
            .collect();

        // Group the differences by the command that would undo them.
        let mut wanted: BTreeMap<Target, Planned> = BTreeMap::new();
        for section in &device.sections {
            for change in &section.changes {
                let full = format!("{}.{}", section.section, change.path);
                let mut exclude = |kind: &'static str, command: Option<String>, reason: String| {
                    excluded.push(Excluded {
                        device_id: id.clone(),
                        section: section.section.clone(),
                        path: change.path.clone(),
                        label: change.label.clone(),
                        kind,
                        command,
                        reason,
                    });
                };
                // Refused outright: a value one side could not read is not a value to put back.
                if change.kind == ChangeKind::Unknown || unreadable.iter().any(|u| covers(u, &full)) {
                    exclude(
                        "unreadable",
                        None,
                        change
                            .reason
                            .clone()
                            .filter(|r| !r.is_empty())
                            .map_or_else(|| "this value could not be read, so there is nothing trustworthy to put back".into(), |r| format!("this value could not be read, so there is nothing trustworthy to put back: {r}")),
                    );
                    continue;
                }
                // A value the snapshot does not have cannot be recalled either; one only the
                // snapshot has is exactly what recall is for.
                if change.kind == ChangeKind::OnlyNow {
                    exclude("incomplete", None, "the snapshot does not hold this value, so recall has nothing to write".into());
                    continue;
                }
                match writer_for(&family, &section.section, &change.path) {
                    Writer::NoWriter { reason } => exclude("no_writer", None, reason.to_string()),
                    Writer::Withheld { command, reason } => exclude("withheld", Some(command.to_string()), reason.to_string()),
                    Writer::Unmapped => exclude(
                        "unmapped",
                        None,
                        "no writer is mapped for this path: capture records it and recall does not know which command puts it back".into(),
                    ),
                    Writer::Command(target) => {
                        for target in expand(target, &family, change) {
                            let mixer_level = matches!(target, Target::MixerStrip { .. }) && change.path.ends_with(".level");
                            let entry = wanted.entry(target.clone()).or_insert_with(|| Planned {
                                target,
                                paths: Vec::new(),
                                from: change.from.clone(),
                                to: change.to.clone(),
                                label: change.label.clone(),
                            });
                            entry.paths.push(full.clone());
                            // A strip is one command; which way it moves is the level's to say, not
                            // the pan's, and the mixer stage is ordered by that.
                            if mixer_level {
                                entry.from = change.from.clone();
                                entry.to = change.to.clone();
                            }
                        }
                    }
                }
            }
        }

        // Silencing, and the restoring that undoes it, are not differences: they are how
        // a recall is made safe. They are planned whenever anything else would be sent.
        let silences = !wanted.is_empty();
        if silences {
            for target in restore_targets(&family) {
                wanted.entry(target.clone()).or_insert_with(|| Planned { target, paths: Vec::new(), from: None, to: None, label: String::new() });
            }
        }

        // In stage order, and within the mixer stage quieter first: level is dB of attenuation, so
        // a bigger number is a cut.
        let mut ordered: Vec<Planned> = wanted.into_values().collect();
        sort_mixer_quieter_first(&mut ordered);

        if silences {
            steps.extend(silence_steps(&id, &device.model, &family));
        }
        let mut restore_incomplete: Vec<String> = Vec::new();
        for planned in ordered {
            match planned.target.build(&family, &recorded.sections) {
                Err(reason) => excluded.push(Excluded {
                    device_id: id.clone(),
                    section: String::new(),
                    path: planned.paths.first().cloned().unwrap_or_default(),
                    label: planned.label.clone(),
                    kind: "incomplete",
                    command: None,
                    reason: {
                        // A restore that cannot be built is not one exclusion among many: it means
                        // this plan would silence an output and have nothing to unmute it with.
                        if matches!(planned.target, Target::OutputMute { .. } | Target::HardMute) {
                            restore_incomplete.push(reason.clone());
                        }
                        reason
                    },
                }),
                Ok(write) => {
                    let mut note = None;
                    let mut blocked = Vec::new();
                    if let Target::PreampPhantom { id: preamp } = planned.target {
                        if write.args["phantom"].as_i64() == Some(1) {
                            phantom_on.push(format!("{} · preamp {}", device.model, preamp + 1));
                            note = Some("switches 48V ON, which can damage a ribbon microphone or unbalanced gear".into());
                        }
                    }
                    if let Target::OutputVolume { id: output } = planned.target {
                        let name = crate::snapshot::writers::output_name(output);
                        // `from` is the snapshot's, `to` is now: recall writes the snapshot's, so
                        // the output is raised by however much attenuation it loses. A present
                        // nobody could read is the dangerous case, not the safe one: how far this
                        // would move the level is then not known, and it needs the same tick.
                        let (raise, unknown) = match (planned.from.as_ref().and_then(Json::as_i64), planned.to.as_ref().and_then(Json::as_i64)) {
                            (Some(from), Some(to)) => (Some((from, to, to - from)), false),
                            (Some(_), None) => (None, true),
                            _ => (None, false),
                        };
                        if let Some((from, to, raise)) = raise {
                            if raise > RAISE_THRESHOLD_DB {
                                raised.push(RaisedOutput { device_id: id.clone(), output: name.to_string(), now: Some(to), snapshot: from, raised_db: Some(raise) });
                                note = Some(format!("raises {name} by {raise} dB"));
                                if !request.confirm_raised_outputs {
                                    blocked.push("raised_outputs".to_string());
                                }
                            }
                        } else if unknown {
                            let snapshot = planned.from.as_ref().and_then(Json::as_i64).unwrap_or_default();
                            raised.push(RaisedOutput { device_id: id.clone(), output: name.to_string(), now: None, snapshot, raised_db: None });
                            note = Some(format!("{name} is at an unknown level now, so how far this moves it is not known"));
                            if !request.confirm_raised_outputs {
                                blocked.push("raised_outputs".to_string());
                            }
                        }
                    }
                    let part = write.part;
                    if !request.chosen(part) {
                        blocked.push(part.name().to_string());
                    } else if !request.confirmed(part) {
                        blocked.push(format!("{}:confirm", part.name()));
                    }
                    steps.push(Step {
                        device_id: id.clone(),
                        model: device.model.clone(),
                        part: part.name().to_string(),
                        title: part.title().to_string(),
                        label: write.label,
                        command: write.command,
                        ext3: write.ext3,
                        args: write.args,
                        bytes: String::new(),
                        bytes_len: 0,
                        paths: planned.paths,
                        from: planned.from,
                        to: planned.to,
                        note,
                        blocked_by: blocked,
                    });
                }
            }
        }
        // If anything the restore needs is missing, every step of this device is held back: a plan
        // that silences the outputs and cannot put them back is not one to run.
        if silences && !restore_incomplete.is_empty() {
            for step in steps.iter_mut().filter(|s| s.device_id == id) {
                step.blocked_by.push("restore_incomplete".to_string());
            }
        }
        device_plans.push(DevicePlan {
            device_id: id,
            model: device.model.clone(),
            family: device.family.clone(),
            missing: false,
            steps: steps.len() - before,
            excluded: excluded.len() - excluded_before,
        });
    }

    // Across devices, the order is in stages: every device is silenced before any of them
    // is written to, and nothing is unmuted until everything is in place.
    steps.sort_by_key(|s| PARTS.iter().position(|p| p.name() == s.part).unwrap_or(usize::MAX));
    let ready = steps.iter().filter(|s| s.blocked_by.is_empty()).count();

    let parts = PARTS
        .iter()
        .map(|part| PartPlan {
            name: part.name().to_string(),
            title: part.title().to_string(),
            default_on: part.default_on(),
            chosen: request.chosen(*part),
            needs_confirming: part.needs_confirming(),
            confirmed: request.confirmed(*part),
            steps: steps.iter().filter(|s| s.part == part.name()).count(),
        })
        .collect();

    phantom_on.sort();
    phantom_on.dedup();
    RecallPlan {
        snapshot: snapshot.summary(),
        prepared_at: now.created.clone(),
        current_state_read: read,
        raise_threshold_db: RAISE_THRESHOLD_DB,
        parts,
        devices: device_plans,
        steps,
        excluded,
        raised_outputs: raised,
        phantom_on,
        ready,
        workspace_changes: differences.workspace.len(),
        sent: false,
        note: "Nothing has been sent to any device. This is what recall would send, in order; applying it waits for a session at the hardware.".into(),
    }
}

/// Whether an unreadable path covers a diff path: itself, or anything under it. The next character
/// decides, because a capture's paths carry indexes (`inputs.preamps` covers `inputs.preamps[0].type`
/// and not `inputs.preamp_gains`).
fn covers(unreadable: &str, path: &str) -> bool {
    path == unreadable || (path.starts_with(unreadable) && matches!(path.as_bytes().get(unreadable.len()), Some(b'.') | Some(b'[')))
}

/// A gain array is one captured value and one command per channel, so the paths that name it fan
/// out here, and only where the two sides' bytes actually differ.
fn expand(target: Target, _family: &str, change: &Change) -> Vec<Target> {
    let Target::Gains { kind } = target else { return vec![target] };
    let bytes = |value: Option<&Json>| value.and_then(Json::as_str).and_then(|text| crate::value::from_hex(text).ok()).unwrap_or_default();
    let want = bytes(change.from.as_ref());
    let have = bytes(change.to.as_ref());
    (0..want.len())
        .filter(|i| have.get(*i) != want.get(*i))
        .map(|i| Target::Gain { kind, id: i as u32 })
        .collect()
}

/// The mixer moves quieter first: within the mixer stage, a step whose level rises (more
/// attenuation) comes before one whose level falls.
fn sort_mixer_quieter_first(ordered: &mut [Planned]) {
    ordered.sort_by_key(|p| {
        let louder = matches!(p.target, Target::MixerStrip { .. })
            && match (p.from.as_ref().and_then(Json::as_i64), p.to.as_ref().and_then(Json::as_i64)) {
                (Some(want), Some(now)) => want < now,
                _ => false,
            };
        (p.target.order().0, u32::from(louder), p.target.order().1, p.target.order().2)
    });
}

/// The mutes and the hard mute the restore stage puts back, whatever they were: every output was
/// silenced, so every output is restored, not only the ones that differed.
fn restore_targets(family: &str) -> Vec<Target> {
    let mut targets: Vec<Target> = (0..topology::output_ids(family).unwrap_or(0)).map(|id| Target::OutputMute { id }).collect();
    if family == "quadro" {
        targets.push(Target::HardMute);
    }
    targets
}

/// Silence every output before anything else is written, as the vendor panel does around a session
/// restore (`docs/protocol.md`, "Hard mute"). The Studio+ has no hard mute, so its five
/// outputs are muted one by one.
fn silence_steps(device_id: &str, model: &str, family: &str) -> Vec<Step> {
    let step = |label: String, command: &str, args: Json| Step {
        device_id: device_id.to_string(),
        model: model.to_string(),
        part: Part::Silence.name().to_string(),
        title: Part::Silence.title().to_string(),
        label,
        command: command.to_string(),
        ext3: None,
        args,
        bytes: String::new(),
        bytes_len: 0,
        paths: Vec::new(),
        from: None,
        to: None,
        note: None,
        blocked_by: Vec::new(),
    };
    if family == "quadro" {
        vec![step("Silence · hard mute on".into(), "set_hard_mute", json!({"value": 1}))]
    } else {
        STUDIO_OUTPUT_KEYS
            .iter()
            .enumerate()
            .map(|(id, _)| step(format!("Silence · {} muted", crate::snapshot::writers::output_name(id as u32)), "set_mute", json!({"id": id, "mute": 1})))
            .collect()
    }
}

/// Fill in each step's bytes, through the server's own dry run.
///
/// `dry_run = true` stops the worker before it writes: it builds the request from the registry and
/// answers with the bytes. Nothing reaches a device.
async fn render(devices: &DeviceManager, plan: &mut RecallPlan) {
    let mut failed: Vec<usize> = Vec::new();
    for (at, step) in plan.steps.iter_mut().enumerate() {
        let id = crate::device::descriptor::DeviceId(step.device_id.clone());
        let values = match crate::value::json_to_payload_values(&step.args) {
            Ok(values) => values,
            Err(e) => {
                step.blocked_by.push(format!("not buildable: {e}"));
                failed.push(at);
                continue;
            }
        };
        match devices.handle(&id) {
            Err(e) => {
                step.blocked_by.push(format!("not buildable: {e}"));
                failed.push(at);
            }
            Ok(handle) => match handle.request(&step.command, values, step.ext3, true).await {
                Ok(outcome) => {
                    step.bytes_len = outcome.sent.len();
                    step.bytes = to_hex(&outcome.sent);
                }
                Err(e) => {
                    step.blocked_by.push(format!("not buildable: {e}"));
                    failed.push(at);
                }
            },
        }
    }
    // A step whose bytes cannot even be built is not a step: it moves to the excluded list, where a
    // person can see the reason, rather than sitting in the plan with nothing to send.
    for at in failed.into_iter().rev() {
        let step = plan.steps.remove(at);
        plan.excluded.push(Excluded {
            device_id: step.device_id,
            section: String::new(),
            path: step.paths.first().cloned().unwrap_or_default(),
            label: step.label,
            kind: "incomplete",
            command: Some(step.command),
            reason: step.blocked_by.join("; "),
        });
    }
    plan.ready = plan.steps.iter().filter(|s| s.blocked_by.is_empty()).count();
}

/// Every path a snapshot of this family can hold, for the coverage test: the leaves of each
/// section, as a capture writes them.
pub fn leaf_paths(sections: &BTreeMap<String, Json>) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (section, value) in sections {
        walk(value, String::new(), &mut |path| out.push((section.clone(), path)));
    }
    out
}

fn walk(value: &Json, path: String, out: &mut impl FnMut(String)) {
    match value {
        Json::Object(map) if !map.is_empty() => {
            for (key, child) in map {
                walk(child, if path.is_empty() { key.clone() } else { format!("{path}.{key}") }, out);
            }
        }
        Json::Array(items) if !items.is_empty() => {
            for (at, child) in items.iter().enumerate() {
                walk(child, format!("{path}[{at}]"), out);
            }
        }
        _ => out(path),
    }
}

/// The gain kinds a family's capture records, so a coverage test can name them.
pub fn gain_kinds(family: &str) -> Vec<GainKind> {
    let mut kinds = vec![GainKind::Preamp, GainKind::Adat, GainKind::Spdif];
    if family == "studio" {
        kinds.push(GainKind::Line);
    }
    kinds
}

/// A set of every distinct diff path a device snapshot could produce, as a cheap coverage check.
pub fn paths_of(device: &DeviceSnapshot) -> BTreeSet<String> {
    leaf_paths(&device.sections).into_iter().map(|(section, path)| format!("{section}.{path}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::descriptor::DeviceId;
    use crate::snapshot::model::Unreadable;

    /// A Quadro as the snapshot recorded it: enough of every section to plan a step in each.
    fn recorded() -> DeviceSnapshot {
        DeviceSnapshot {
            family: "quadro".into(),
            model: "Zen Quadro".into(),
            read_at: "2026-09-17T20:15:00Z".into(),
            current_preset: Some(1),
            sections: BTreeMap::from([
                (
                    "mixer".into(),
                    json!({
                        "mixes[0]": [
                            {"level": 0, "pan": 32, "mute": 0, "solo": 0},
                            {"level": 40, "pan": 32, "mute": 0, "solo": 0},
                            {"level": 10, "pan": 32, "mute": 0, "solo": 0}
                        ],
                        "links": [{"linked": 1}]
                    }),
                ),
                (
                    "routing".into(),
                    json!({"MIXER_IN0": {"bank_idx": 8, "bank_configs": (0..64).map(|i| json!({"in_periph_id": 1, "in_chann": i})).collect::<Vec<_>>()}}),
                ),
                (
                    "inputs".into(),
                    json!({
                        "preamps": [{"type": 0, "phantom": 1, "hpf": 1, "phase_inv": 0, "zero_cross": 0}],
                        "preamp_gains": "0a14",
                        "adat_gains": "0102",
                        "spdif_gains": "0304",
                        "links": {"preamp": [{"linked": 1}]},
                        "emulations": [{"target": 1, "emu_model": 0, "ch_swap": 0, "pattern": 0}]
                    }),
                ),
                (
                    "outputs".into(),
                    json!({
                        "volumes": [
                            {"volume": 10, "mute": 0, "dim_on": 0, "mono": 0, "trim": 0},
                            {"volume": 20, "mute": 1, "dim_on": 0, "mono": 0, "trim": 0},
                            {"volume": 20, "mute": 0, "dim_on": 0, "mono": 0, "trim": 0},
                            {"volume": 20, "mute": 0, "dim_on": 0, "mono": 0, "trim": 0}
                        ],
                        "hard_mute": 0,
                        "trims": {"monitor": 3, "line_out": 1, "adc": 7}
                    }),
                ),
                ("clock".into(), json!({"sync_source": 0, "rate_index": 2})),
                ("settings".into(), json!({"brightness": 60, "panning_law": 0, "dc_coupled_in": 0, "dc_coupled_out": 0})),
            ]),
            unreadable: Vec::new(),
        }
    }

    fn snapshot_of(device: DeviceSnapshot) -> Snapshot {
        let mut s = Snapshot::new("Drum tracking".into(), String::new(), Workspace::default());
        s.devices.insert(DeviceId::loopback(0), device);
        s
    }

    /// The present: the same device with one thing changed in every section.
    fn present() -> Snapshot {
        let mut device = recorded();
        {
            let m = device.sections.get_mut("mixer").expect("mixer");
            m["mixes[0]"][1]["level"] = json!(10); // the snapshot cuts this strip
            m["mixes[0]"][2]["level"] = json!(60); // and raises this one
            m["links"][0]["linked"] = json!(0);
            let r = device.sections.get_mut("routing").expect("routing");
            r["MIXER_IN0"]["bank_configs"][3]["in_chann"] = json!(31);
            let i = device.sections.get_mut("inputs").expect("inputs");
            i["preamps"][0]["phantom"] = json!(0); // the snapshot switches 48V ON
            i["preamps"][0]["type"] = json!(1);
            i["preamps"][0]["hpf"] = json!(0);
            i["preamp_gains"] = json!("0a20"); // only the second byte differs
            i["adat_gains"] = json!("0105");
            let o = device.sections.get_mut("outputs").expect("outputs");
            o["volumes"][0]["volume"] = json!(30); // now -30 dB, the snapshot -10: a 20 dB raise
            o["volumes"][1]["volume"] = json!(24); // a 4 dB raise, under the threshold
            let c = device.sections.get_mut("clock").expect("clock");
            c["sync_source"] = json!(5);
            let s = device.sections.get_mut("settings").expect("settings");
            s["brightness"] = json!(20);
            s["dc_coupled_out"] = json!(1);
        }
        snapshot_of(device)
    }

    fn plan_with(request: RecallRequest) -> RecallPlan {
        let snapshot = snapshot_of(recorded());
        let now = present();
        let differences = diff(&snapshot, &now);
        build(&snapshot, &now, &differences, &request, true)
    }

    fn plan() -> RecallPlan {
        plan_with(RecallRequest::default())
    }

    fn commands(plan: &RecallPlan) -> Vec<String> {
        plan.steps.iter().map(|s| format!("{}:{}", s.part, s.command)).collect()
    }

    fn excluded_for<'a>(plan: &'a RecallPlan, path: &str) -> &'a Excluded {
        plan.excluded
            .iter()
            .find(|e| e.path == path)
            .unwrap_or_else(|| panic!("{path} is not in the excluded list: {:?}", plan.excluded.iter().map(|e| &e.path).collect::<Vec<_>>()))
    }

    #[test]
    fn nothing_is_sent_and_the_plan_says_so() {
        let plan = plan();
        assert!(!plan.sent);
        assert!(plan.note.contains("Nothing has been sent"));
        assert!(plan.current_state_read);
        assert_eq!(plan.raise_threshold_db, 6, "the threshold the user chose");
    }

    #[test]
    fn the_steps_run_in_the_specs_volume_safe_order() {
        let plan = plan();
        let parts: Vec<&str> = plan.steps.iter().map(|s| s.part.as_str()).collect();
        let stage = |name: &str| PARTS.iter().position(|p| p.name() == name).expect("a part");
        assert!(parts.windows(2).all(|w| stage(w[0]) <= stage(w[1])), "{parts:?}");
        assert_eq!(parts.first().copied(), Some("silence"));
        assert_eq!(parts.last().copied(), Some("restore"));
        assert_eq!(plan.steps[0].command, "set_hard_mute", "the Quadro is silenced with its hard mute");
        assert_eq!(plan.steps[0].args, json!({"value": 1}));
        assert_eq!(plan.steps.last().expect("a step").command, "set_hard_mute", "and it comes off last of all");
        assert_eq!(plan.steps.last().expect("a step").args, json!({"value": 0}));

        let at = |command: &str| plan.steps.iter().position(|s| s.command == command).unwrap_or_else(|| panic!("no {command} in {:?}", commands(&plan)));
        assert!(at("set_pre_type") < at("set_pre_gain"), "the type sets the gain's range");
        assert!(at("set_pre_gain") < at("set_pre_phantom"), "48V goes on last among the inputs");
        assert!(at("set_sync_source") < at("set_pre_type"), "the clock settles first");
        assert!(at("set_dc_coupled") < at("set_pre_type"), "DC coupling is changed while silenced");
        assert!(at("set_pre_phantom") < at("set_routing"));
        assert!(at("set_routing") < at("set_mixer"));
        assert!(at("set_mixer") < at("set_volume"));
        assert!(at("set_volume") < at("set_mute"), "the mutes are restored after the volumes are in place");

        let mixer: Vec<&str> = plan.steps.iter().filter(|s| s.part == "mixer").map(|s| s.label.as_str()).collect();
        assert_eq!(mixer, ["Mixer · Mix 1 · strip 1", "Mixer · Mix 1 · strip 2"], "quieter first");
    }

    #[test]
    fn every_output_is_restored_because_every_output_was_silenced() {
        let plan = plan();
        let restored: Vec<Json> = plan.steps.iter().filter(|s| s.command == "set_mute").map(|s| s.args.clone()).collect();
        assert_eq!(restored.len(), 4, "the Quadro's four outputs, not only the ones that differed");
        assert_eq!(restored[1], json!({"id": 1, "mute": 1}), "each goes back to the mute the snapshot recorded");
    }

    #[test]
    fn a_gain_array_becomes_one_command_for_each_byte_that_differs() {
        let plan = plan();
        let gains: Vec<Json> = plan.steps.iter().filter(|s| s.command == "set_pre_gain").map(|s| s.args.clone()).collect();
        assert_eq!(gains, vec![json!({"id": 1, "gain": 20})], "only the channel whose gain moved");
    }

    #[test]
    fn the_dangerous_parts_are_off_until_they_are_ticked_and_say_what_they_would_do() {
        let plan = plan();
        let part = |name: &str| plan.parts.iter().find(|p| p.name == name).expect("a part").clone();
        for name in ["phantom", "clock", "dc_coupling", "settings"] {
            assert!(!part(name).chosen, "{name} is off by default");
        }
        for name in ["mixer", "routing", "inputs", "outputs"] {
            assert!(part(name).chosen, "{name} is on by default");
        }
        for name in ["phantom", "clock", "dc_coupling"] {
            assert!(part(name).needs_confirming && !part(name).confirmed, "{name} needs its own tick");
        }
        let blocked = |command: &str| plan.steps.iter().find(|s| s.command == command).expect("a step").blocked_by.clone();
        assert_eq!(blocked("set_pre_phantom"), vec!["phantom"]);
        assert_eq!(blocked("set_sync_source"), vec!["clock"]);
        assert_eq!(blocked("set_dc_coupled"), vec!["dc_coupling"]);
        assert_eq!(blocked("set_brightness"), vec!["settings"]);
        assert!(blocked("set_mixer").is_empty(), "a part that is on holds nothing back");

        assert_eq!(plan.phantom_on, vec!["Zen Quadro · preamp 1"]);
        let phantom = plan.steps.iter().find(|s| s.command == "set_pre_phantom").expect("a step");
        assert!(phantom.note.as_deref().unwrap_or_default().contains("ribbon microphone"));

        let asked = RecallRequest {
            parts: BTreeMap::from([("phantom".into(), true), ("clock".into(), true)]),
            confirm: BTreeMap::from([("phantom".into(), true)]),
            ..RecallRequest::default()
        };
        let ticked = plan_with(asked);
        assert!(ticked.steps.iter().find(|s| s.command == "set_pre_phantom").expect("a step").blocked_by.is_empty());
        assert_eq!(
            ticked.steps.iter().find(|s| s.command == "set_sync_source").expect("a step").blocked_by,
            vec!["clock:confirm"],
            "chosen is not confirmed"
        );
    }

    #[test]
    fn an_output_raised_by_more_than_six_db_is_listed_and_needs_a_tick() {
        let plan = plan();
        assert_eq!(plan.raised_outputs.len(), 1, "only the one over the threshold: {:?}", plan.raised_outputs);
        let raised = &plan.raised_outputs[0];
        assert_eq!((raised.output.as_str(), raised.now, raised.snapshot, raised.raised_db), ("Monitor", Some(30), 10, Some(20)));
        let step = plan.steps.iter().find(|s| s.command == "set_volume" && s.args["id"] == json!(0)).expect("a step");
        assert_eq!(step.blocked_by, vec!["raised_outputs"]);
        assert_eq!(step.note.as_deref(), Some("raises Monitor by 20 dB"));
        let hp1 = plan.steps.iter().find(|s| s.command == "set_volume" && s.args["id"] == json!(1)).expect("a step");
        assert!(hp1.blocked_by.is_empty() && hp1.note.is_none());

        let ticked = plan_with(RecallRequest { confirm_raised_outputs: true, ..RecallRequest::default() });
        assert!(ticked.steps.iter().find(|s| s.command == "set_volume" && s.args["id"] == json!(0)).expect("a step").blocked_by.is_empty());
    }

    #[test]
    fn links_are_never_fanned_out_and_never_set() {
        let plan = plan();
        assert!(!plan.steps.iter().any(|s| s.command == "set_stereo_link"), "recall writes each channel its own value");
        let excluded = excluded_for(&plan, "links[0].linked");
        assert_eq!((excluded.kind, excluded.command.as_deref()), ("withheld", Some("set_stereo_link")));
        assert!(excluded.reason.contains("mirror"));
    }

    #[test]
    fn a_value_that_could_not_be_read_is_refused_outright() {
        let mut device = recorded();
        device.unreadable.push(Unreadable { path: "inputs.preamps".into(), reason: "device loopback-0 refused 'get_preamps'".into() });
        let snapshot = snapshot_of(device);
        let now = present();
        let plan = build(&snapshot, &now, &diff(&snapshot, &now), &RecallRequest::default(), true);
        assert!(
            !plan.steps.iter().any(|s| s.command == "set_pre_type" || s.command == "set_pre_phantom"),
            "nothing under an unread path is written: {:?}",
            commands(&plan)
        );
        let refused: Vec<&Excluded> = plan.excluded.iter().filter(|e| e.kind == "unreadable").collect();
        assert!(!refused.is_empty());
        assert!(refused.iter().all(|e| e.reason.contains("nothing trustworthy to put back")), "{refused:?}");
        assert!(plan.steps.iter().any(|s| s.command == "set_pre_gain"), "the gains beside it were readable");
    }

    #[test]
    fn a_device_the_snapshot_has_and_the_present_does_not_is_named_and_planned_for_nothing() {
        let snapshot = snapshot_of(recorded());
        let now = Snapshot::new("now".into(), String::new(), Workspace::default());
        let plan = build(&snapshot, &now, &diff(&snapshot, &now), &RecallRequest::default(), true);
        assert!(plan.steps.is_empty(), "nothing can be sent to a device that is not there");
        assert_eq!(plan.devices.len(), 1);
        assert!(plan.devices[0].missing);
        let excluded = &plan.excluded[0];
        assert_eq!(excluded.kind, "device_missing");
        assert!(excluded.reason.contains("not attached now"));
    }

    #[test]
    fn what_has_no_writer_is_named_with_its_reason_rather_than_dropped() {
        let plan = plan();
        assert_eq!(excluded_for(&plan, "preamps[0].hpf").kind, "no_writer");
        assert!(excluded_for(&plan, "preamps[0].hpf").reason.contains("high-pass filter"));
        let gains = excluded_for(&plan, "adat_gains");
        assert_eq!((gains.kind, gains.command.as_deref()), ("withheld", Some("set_adat_gain")));
        assert!(gains.reason.contains("hardware probe"));
        assert!(!plan.steps.iter().any(|s| s.command == "set_adat_gain"), "the Quadro's ADAT gains are not sent");
        assert!(plan.excluded.iter().all(|e| e.reason.len() > 20), "{:?}", plan.excluded);
    }

    #[test]
    fn an_unreadable_path_covers_what_is_under_it_and_nothing_beside_it() {
        assert!(covers("inputs.preamps", "inputs.preamps"));
        assert!(covers("inputs.preamps", "inputs.preamps[0].type"));
        assert!(covers("inputs.links", "inputs.links.preamp[0].linked"));
        assert!(!covers("inputs.preamps", "inputs.preamp_gains"));
        assert!(!covers("inputs.preamp", "inputs.preamps[0].type"));
    }

    #[test]
    fn a_value_the_devices_would_not_give_up_just_now_is_refused_too() {
        // The snapshot read fine; the present did not. Sending the snapshot's value would be
        // sending from a view nobody has, which is what guard 1 forbids.
        let snapshot = snapshot_of(recorded());
        let mut now = present();
        {
            let device = now.devices.get_mut(&DeviceId::loopback(0)).expect("device");
            device.sections.get_mut("outputs").expect("outputs").as_object_mut().expect("an object").remove("volumes");
            device.unreadable.push(Unreadable { path: "outputs.volumes".into(), reason: "the device has not reported its state".into() });
        }
        let plan = build(&snapshot, &now, &diff(&snapshot, &now), &RecallRequest::default(), true);
        assert!(!plan.steps.iter().any(|s| s.command == "set_volume"), "{:?}", commands(&plan));
        assert!(plan.excluded.iter().any(|e| e.kind == "unreadable" && e.path.starts_with("volumes")), "{:?}", plan.excluded);
    }

    #[test]
    fn a_plan_that_could_not_put_the_outputs_back_holds_every_step_of_that_device() {
        // A snapshot with no mutes recorded: recall would silence the outputs and have nothing to
        // unmute them with, so nothing of it is ready to run.
        let mut device = recorded();
        device.sections.get_mut("outputs").expect("outputs").as_object_mut().expect("an object").remove("hard_mute");
        let snapshot = snapshot_of(device);
        let now = present();
        let plan = build(&snapshot, &now, &diff(&snapshot, &now), &RecallRequest::default(), true);
        assert_eq!(plan.ready, 0, "nothing runs while the outputs cannot be unsilenced");
        assert!(plan.steps.iter().all(|s| s.blocked_by.iter().any(|g| g == "restore_incomplete")), "{:?}", plan.steps.iter().map(|s| &s.blocked_by).collect::<Vec<_>>());
    }

    #[test]
    fn an_output_whose_level_now_is_unknown_needs_the_same_tick_as_a_raise() {
        let snapshot = snapshot_of(recorded());
        let mut now = present();
        {
            let outputs = now.devices.get_mut(&DeviceId::loopback(0)).expect("device").sections.get_mut("outputs").expect("outputs");
            outputs["volumes"][0].as_object_mut().expect("an output").remove("volume");
        }
        let plan = build(&snapshot, &now, &diff(&snapshot, &now), &RecallRequest::default(), true);
        let step = plan.steps.iter().find(|s| s.command == "set_volume" && s.args["id"] == json!(0)).expect("a step");
        assert!(step.blocked_by.contains(&"raised_outputs".to_string()), "{step:?}");
        assert!(step.note.as_deref().unwrap_or_default().contains("unknown level"));
        let raised = plan.raised_outputs.iter().find(|r| r.output == "Monitor").expect("a raised output");
        assert_eq!((raised.now, raised.raised_db, raised.snapshot), (None, None, 10));
    }

    #[test]
    fn a_plan_only_for_one_device_leaves_the_others_alone() {
        let plan = plan_with(RecallRequest { devices: Some(vec!["loopback-9".into()]), ..RecallRequest::default() });
        assert!(plan.steps.is_empty());
        assert_eq!(plan.excluded[0].kind, "not_chosen");
    }
}
