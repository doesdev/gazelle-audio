//! The mirror of capture's read plan: which command puts each captured value back.
//!
//! **Nothing here sends anything.** A [`Writer`] is a description — a command name, the arguments
//! it would carry and the label a person reads — and [`plan`](crate::snapshot::plan) turns a set of
//! them into an ordered list that a later, hardware-gated step could send. Recall itself is phase 6
//! of `specs/2026-09-16-workspace-snapshots-and-cross-device-mixer.md` and waits for a session at
//! the devices (spec §2.3, decision 0012).
//!
//! The table is **exhaustive over what capture records**: every leaf path a snapshot can hold maps
//! to one of
//!
//! - [`Writer::Command`] — a command exists and recall would send it;
//! - [`Writer::NoWriter`] — the device reports the value and neither panel has a command for it
//!   (the high-pass filter, the Quadro's mono flag, the preset slot);
//! - [`Writer::Withheld`] — a command exists but recall does not send it until a hardware session
//!   has shown that it is safe or that it works at all (the Quadro's ADAT and S/PDIF input gains,
//!   the stereo link flags, `get_trim_configs`);
//! - [`Writer::Unmapped`] — nothing here knows this path. It is never silent: the plan lists it as
//!   excluded, and `writer_covers_every_path_a_capture_can_record` fails, so a field added to
//!   capture cannot slip through as "nothing to do".
//!
//! Paths are the diff's paths (`mixes[0][1].level`, `routing.MIXER_IN0.bank_configs[3].in_chann`,
//! `preamps[0].phantom`), because the diff is what recall acts on.

use serde_json::{json, Value as Json};
use std::collections::BTreeMap;

use crate::workspace::topology;

/// `set_volume` / `set_mute` / `set_dim` ids, in order (`reference/devices.md`, "Output ids").
pub const OUTPUT_NAMES: &[&str] = &["Monitor", "HP1", "HP2", "Line out", "Reamp"];

/// The Studio+'s output keys in the snapshot, in `set_volume` id order.
pub const STUDIO_OUTPUT_KEYS: &[&str] = &["monitor", "hp1", "hp2", "line_out", "reamp"];

/// Trim ids and their names; the Quadro's panel sets the first two, the Studio+'s all three.
pub const TRIM_KEYS: &[(&str, &str)] = &[("monitor", "Monitor"), ("line_out", "Line out"), ("adc", "ADC")];

/// `set_tbk_enable` ids, in the order the snapshot names them.
pub const TALKBACK_KEYS: &[(&str, &str)] = &[("to_hp1", "HP1"), ("to_hp2", "HP2"), ("to_monitor", "Monitor")];

/// `set_trim_config` carries 32 two-byte entries; the Quadro panel fills the first (P56).
const TRIM_LEVEL_ENTRIES: usize = 32;

/// `set_routing` replaces exactly 32 slots. The Quadro's `get_routing` answers **64** entries, so a
/// group read from a Quadro is written back from its first 32 (spec §2.2; hardware checklist).
pub const ROUTING_SLOTS: usize = 32;

/// What recall would do about one captured path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Writer {
    /// A command puts this value back.
    Command(Target),
    /// The device reports it and nothing sets it.
    NoWriter { reason: &'static str },
    /// A command exists, and recall does not use it until a hardware session says it may.
    Withheld { command: &'static str, reason: &'static str },
    /// This table has never heard of the path. Always reported, never skipped.
    Unmapped,
}

/// One thing a single command would set. Several diff paths can share a target — a mixer strip's
/// level and its mute are one `set_mixer` — and the plan writes each target once.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Target {
    /// One strip (or the master, channel 0) of one mix.
    MixerStrip { mix: u32, channel: u32 },
    /// One destination group's 32 slots, by its topology id.
    RoutingGroup { group: String },
    PreampType { id: u32 },
    PreampPhantom { id: u32 },
    PreampPhase { id: u32 },
    /// A whole gain array, which the plan expands into one [`Target::Gain`] per byte that differs.
    Gains { kind: GainKind },
    Gain { kind: GainKind, id: u32 },
    Emulation { channel: u32 },
    OutputVolume { id: u32 },
    OutputMute { id: u32 },
    OutputDim { id: u32 },
    HardMute,
    Trim { id: u32 },
    TalkbackVolume,
    TalkbackDestination { id: u32 },
    SyncSource,
    SampleRate,
    SpdifSrc,
    Brightness,
    PanningLaw,
    /// `set_dc_coupled`'s `dc_coupled_io`: inputs 0, outputs 1.
    DcCoupled { io: u32 },
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum GainKind {
    Preamp,
    Line,
    Adat,
    Spdif,
}

impl GainKind {
    fn key(self) -> &'static str {
        match self {
            GainKind::Preamp => "preamp_gains",
            GainKind::Line => "line_gains",
            GainKind::Adat => "adat_gains",
            GainKind::Spdif => "spdif_gains",
        }
    }

    fn command(self) -> &'static str {
        match self {
            GainKind::Preamp => "set_pre_gain",
            GainKind::Line => "set_line_gain",
            GainKind::Adat => "set_adat_gain",
            GainKind::Spdif => "set_spdif_gain",
        }
    }

    fn label(self) -> &'static str {
        match self {
            GainKind::Preamp => "preamp",
            GainKind::Line => "line in",
            GainKind::Adat => "ADAT in",
            GainKind::Spdif => "S/PDIF in",
        }
    }
}

/// Which opt-in group a step belongs to. The spec's per-section opt-in (§2.3.2) plus the two
/// dangerous parts that are their own tick: 48V inside Inputs, DC coupling inside Settings.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Part {
    Silence,
    Clock,
    Settings,
    DcCoupling,
    Inputs,
    Phantom,
    Routing,
    Mixer,
    Outputs,
    Restore,
}

impl Part {
    pub fn name(self) -> &'static str {
        match self {
            Part::Silence => "silence",
            Part::Clock => "clock",
            Part::Settings => "settings",
            Part::DcCoupling => "dc_coupling",
            Part::Inputs => "inputs",
            Part::Phantom => "phantom",
            Part::Routing => "routing",
            Part::Mixer => "mixer",
            Part::Outputs => "outputs",
            Part::Restore => "restore",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Part::Silence => "Silence the outputs",
            Part::Clock => "Clock",
            Part::Settings => "Device settings",
            Part::DcCoupling => "DC coupling",
            Part::Inputs => "Inputs",
            Part::Phantom => "48V",
            Part::Routing => "Routing",
            Part::Mixer => "Mixer",
            Part::Outputs => "Outputs",
            Part::Restore => "Restore the outputs",
        }
    }

    /// Whether the part is on unless the user turns it off (spec §2.3.2). 48V, clock, DC coupling
    /// and device settings are off; silencing and restoring are not the user's to choose.
    pub fn default_on(self) -> bool {
        !matches!(self, Part::Phantom | Part::Clock | Part::DcCoupling | Part::Settings)
    }

    /// Whether it needs its own confirmation every time, never remembered (spec §2.3.3).
    pub fn needs_confirming(self) -> bool {
        matches!(self, Part::Phantom | Part::Clock | Part::DcCoupling)
    }

    /// Where the part sits in the volume-safe order (spec §2.3.4).
    pub fn stage(self) -> u32 {
        match self {
            Part::Silence => 0,
            Part::Clock => 1,
            Part::Settings => 2,
            Part::DcCoupling => 3,
            Part::Inputs | Part::Phantom => 4,
            Part::Routing => 5,
            Part::Mixer => 6,
            Part::Outputs => 7,
            Part::Restore => 8,
        }
    }
}

/// Every opt-in part, in the order they run, for a client that wants to show the ticks before a
/// plan exists.
pub const PARTS: &[Part] = &[
    Part::Silence,
    Part::Clock,
    Part::Settings,
    Part::DcCoupling,
    Part::Inputs,
    Part::Phantom,
    Part::Routing,
    Part::Mixer,
    Part::Outputs,
    Part::Restore,
];

/// One command, built: what recall would send, before any bytes are rendered.
#[derive(Clone, Debug, PartialEq)]
pub struct Write {
    pub command: String,
    /// The header selector, for commands that take one. No writer does: every `set_*` carries its
    /// own `ext3` from the registry, and only the four `get_*` in `EXT3_SELECTOR_COMMANDS` take a
    /// per-request one. Carried anyway so a plan reads as the wire does.
    pub ext3: Option<u32>,
    pub args: Json,
    pub label: String,
    pub part: Part,
}

impl Target {
    /// Inputs run in the order the spec gives: 48V off, type, gain, phase, emulation, 48V on
    /// (§2.3.4). Within a part, this orders the steps.
    pub fn order(&self) -> (u32, u32, u32) {
        let within = match self {
            Target::PreampType { .. } => 1,
            Target::Gains { kind } | Target::Gain { kind, .. } => match kind {
                GainKind::Preamp => 2,
                _ => 3,
            },
            Target::PreampPhase { .. } => 4,
            Target::Emulation { .. } => 5,
            Target::PreampPhantom { .. } => 6,
            Target::SyncSource => 0,
            Target::SampleRate => 1,
            Target::SpdifSrc => 2,
            Target::OutputVolume { .. } => 0,
            Target::Trim { .. } => 1,
            Target::TalkbackVolume => 2,
            Target::TalkbackDestination { .. } => 3,
            Target::OutputDim { .. } => 3,
            Target::OutputMute { .. } => 4,
            Target::HardMute => 9,
            _ => 0,
        };
        (self.part().stage(), within, self.index())
    }

    /// The channel, output or slot number, so two steps of one kind keep the device's own order.
    fn index(&self) -> u32 {
        match self {
            Target::MixerStrip { mix, channel } => mix * 64 + channel,
            Target::PreampType { id }
            | Target::PreampPhantom { id }
            | Target::PreampPhase { id }
            | Target::Gain { id, .. }
            | Target::OutputVolume { id }
            | Target::OutputMute { id }
            | Target::OutputDim { id }
            | Target::Trim { id }
            | Target::TalkbackDestination { id } => *id,
            Target::Emulation { channel } => *channel,
            Target::DcCoupled { io } => *io,
            _ => 0,
        }
    }

    /// Which opt-in part this target belongs to.
    pub fn part(&self) -> Part {
        match self {
            Target::MixerStrip { .. } => Part::Mixer,
            Target::RoutingGroup { .. } => Part::Routing,
            Target::PreampPhantom { .. } => Part::Phantom,
            Target::PreampType { .. }
            | Target::PreampPhase { .. }
            | Target::Gains { .. }
            | Target::Gain { .. }
            | Target::Emulation { .. } => Part::Inputs,
            Target::OutputVolume { .. } | Target::Trim { .. } | Target::TalkbackVolume | Target::TalkbackDestination { .. } => Part::Outputs,
            // A mute, the Quadro's dim and its hard mute are what the silencing and the restoring
            // are made of: they are set at the end, once everything else is in place.
            Target::OutputMute { .. } | Target::OutputDim { .. } | Target::HardMute => Part::Restore,
            Target::SyncSource | Target::SampleRate | Target::SpdifSrc => Part::Clock,
            Target::Brightness | Target::PanningLaw => Part::Settings,
            Target::DcCoupled { .. } => Part::DcCoupling,
        }
    }

    /// The command and arguments that would put the snapshot's value back, read out of the device
    /// snapshot's `sections`. `Err` when the snapshot does not hold what the command needs — a
    /// half-read mixer strip, a routing group of the wrong width — which the plan lists as excluded
    /// rather than sending a command built from defaults.
    pub fn build(&self, family: &str, sections: &BTreeMap<String, Json>) -> Result<Write, String> {
        let section = |name: &str| sections.get(name).cloned().unwrap_or(Json::Null);
        match self {
            Target::MixerStrip { mix, channel } => {
                let strip = section("mixer")
                    .get(format!("mixes[{mix}]"))
                    .and_then(|m| m.get(*channel as usize))
                    .cloned()
                    .ok_or_else(|| format!("the snapshot has no mix {} strip {channel}", mix + 1))?;
                let field = |name: &str| strip.get(name).and_then(Json::as_i64).ok_or_else(|| format!("the snapshot's mix {} strip {channel} has no '{name}'", mix + 1));
                let mut args = json!({
                    "mixer_id": mix,
                    "channel": channel,
                    "level": field("level")?,
                    "pan": field("pan")?,
                    "mute": field("mute")?,
                    "solo": field("solo")?,
                });
                let command = if family == "studio" {
                    args["send"] = Json::from(field("send")?);
                    "set_mixer_cfg"
                } else {
                    "set_mixer"
                };
                Ok(Write { command: command.into(), ext3: None, args, label: mixer_label(*mix, *channel), part: self.part() })
            }
            Target::RoutingGroup { group } => {
                let captured = section("routing").get(group).cloned().ok_or_else(|| format!("the snapshot has no routing for {group}"))?;
                let slots = captured.get("bank_configs").and_then(Json::as_array).cloned().unwrap_or_default();
                if slots.len() < ROUTING_SLOTS {
                    return Err(format!("the snapshot has {} slots for {group}, and set_routing replaces {ROUTING_SLOTS}", slots.len()));
                }
                // The wire position is the topology's, not the snapshot's: a snapshot is keyed by
                // group id exactly so it survives a re-extraction that renumbers them (spec §2.5).
                let bank_idx = topology::destination_groups(family)
                    .unwrap_or_default()
                    .into_iter()
                    .find(|(id, _)| id == group)
                    .map(|(_, at)| at)
                    .ok_or_else(|| format!("{group} is not a destination group of this model"))?;
                let mut bytes = Vec::with_capacity(ROUTING_SLOTS * 2);
                for (at, slot) in slots.iter().take(ROUTING_SLOTS).enumerate() {
                    let byte = |name: &str| slot.get(name).and_then(Json::as_i64).ok_or_else(|| format!("{group} slot {} has no '{name}'", at + 1));
                    bytes.push(Json::from(byte("in_periph_id")?));
                    bytes.push(Json::from(byte("in_chann")?));
                }
                Ok(Write {
                    command: "set_routing".into(),
                    ext3: None,
                    args: json!({ "bank_idx": bank_idx, "bank_configs": Json::Array(bytes) }),
                    label: format!("Routing · {} · all {ROUTING_SLOTS} slots", crate::snapshot::diff::group_title(group)),
                    part: self.part(),
                })
            }
            Target::PreampType { id } => {
                let value = preamp_field(&section("inputs"), *id, &["type", "pretype"])?;
                Ok(Write {
                    command: "set_pre_type".into(),
                    ext3: None,
                    args: json!({ "id": id, "pretype": value }),
                    label: format!("Inputs · preamp {} · type", id + 1),
                    part: self.part(),
                })
            }
            Target::PreampPhantom { id } => {
                let value = preamp_field(&section("inputs"), *id, &["phantom"])?;
                Ok(Write {
                    command: "set_pre_phantom".into(),
                    ext3: None,
                    args: json!({ "id": id, "phantom": value }),
                    label: format!("48V · preamp {} · {}", id + 1, if value == 0 { "off" } else { "ON" }),
                    part: self.part(),
                })
            }
            Target::PreampPhase { id } => {
                let value = preamp_field(&section("inputs"), *id, &["phase_inv"])?;
                // The two panels spell the same command differently, one letter apart.
                let command = if family == "studio" { "set_pre_phaseinv" } else { "set_pre_phase_inv" };
                Ok(Write {
                    command: command.into(),
                    ext3: None,
                    args: json!({ "id": id, "phase_inv": value }),
                    label: format!("Inputs · preamp {} · phase invert", id + 1),
                    part: self.part(),
                })
            }
            // Expanded by the plan into one `Gain` per byte that differs: a captured gain array is
            // one hex string, and one command sets one channel.
            Target::Gains { kind } => Err(format!("{} are set one channel at a time", kind.label())),
            Target::Gain { kind, id } => {
                let gains = gain_bytes(&section("inputs"), *kind)?;
                let value = gains.get(*id as usize).copied().ok_or_else(|| format!("the snapshot has no {} gain {}", kind.label(), id + 1))?;
                Ok(Write {
                    command: kind.command().into(),
                    ext3: None,
                    args: json!({ "id": id, "gain": i64::from(value) }),
                    label: format!("Inputs · {} {} · gain", kind.label(), id + 1),
                    part: self.part(),
                })
            }
            Target::Emulation { channel } => {
                let entry = section("inputs")
                    .get("emulations")
                    .and_then(|e| e.get(*channel as usize))
                    .cloned()
                    .ok_or_else(|| format!("the snapshot has no mic emulation for preamp {}", channel + 1))?;
                let field = |name: &str| entry.get(name).and_then(Json::as_i64).ok_or_else(|| format!("the snapshot's emulation {} has no '{name}'", channel + 1));
                Ok(Write {
                    command: "set_mic_emulation".into(),
                    ext3: None,
                    args: json!({
                        "preamp_ch": channel,
                        "target": field("target")?,
                        "emu_model": field("emu_model")?,
                        "ch_swap": field("ch_swap")?,
                        // A pair-wide value the panel never changes on its own and every write
                        // carries back unchanged (P81).
                        "pattern": field("pattern")?,
                    }),
                    label: format!("Inputs · preamp {} · mic emulation", channel + 1),
                    part: self.part(),
                })
            }
            Target::OutputVolume { id } => {
                let value = output_field(&section("outputs"), family, *id, "volume")?;
                Ok(Write {
                    command: "set_volume".into(),
                    ext3: None,
                    args: json!({ "id": id, "volume": value }),
                    label: format!("Outputs · {} · volume", output_name(*id)),
                    part: self.part(),
                })
            }
            Target::OutputMute { id } => {
                let value = output_field(&section("outputs"), family, *id, "mute")?;
                Ok(Write {
                    command: "set_mute".into(),
                    ext3: None,
                    args: json!({ "id": id, "mute": value }),
                    label: format!("Outputs · {} · mute", output_name(*id)),
                    part: self.part(),
                })
            }
            Target::OutputDim { id } => {
                let value = output_field(&section("outputs"), family, *id, "dim_on")?;
                Ok(Write {
                    command: "set_dim".into(),
                    ext3: None,
                    args: json!({ "periph_id": id, "dim": value }),
                    label: format!("Outputs · {} · dim", output_name(*id)),
                    part: self.part(),
                })
            }
            Target::HardMute => {
                let value = section("outputs").get("hard_mute").and_then(Json::as_i64).ok_or("the snapshot has no hard mute")?;
                Ok(Write {
                    command: "set_hard_mute".into(),
                    ext3: None,
                    args: json!({ "value": value }),
                    label: "Outputs · hard mute".into(),
                    part: self.part(),
                })
            }
            Target::Trim { id } => {
                let (key, name) = TRIM_KEYS.get(*id as usize).copied().ok_or_else(|| format!("there is no trim {id}"))?;
                let value = section("outputs")
                    .get("trims")
                    .and_then(|t| t.get(key))
                    .and_then(Json::as_i64)
                    .ok_or_else(|| format!("the snapshot has no {name} trim"))?;
                let (command, args) = if family == "quadro" {
                    // The Quadro's panel sends the whole 32-entry level array with the step in the
                    // first entry and `control` 1 (P56, `outputs.ts`).
                    let mut level = vec![Json::from(0); TRIM_LEVEL_ENTRIES * 2];
                    level[0] = Json::from(value);
                    ("set_trim_config", json!({ "trim_id": id, "control": 1, "level": Json::Array(level) }))
                } else {
                    ("set_trim", json!({ "id": id, "trim_idx": value }))
                };
                Ok(Write { command: command.into(), ext3: None, args, label: format!("Outputs · {name} trim"), part: self.part() })
            }
            Target::TalkbackVolume => {
                let value = section("outputs").get("talkback").and_then(|t| t.get("mic_volume")).and_then(Json::as_i64).ok_or("the snapshot has no talkback level")?;
                Ok(Write {
                    command: "set_tbk_vol".into(),
                    ext3: None,
                    args: json!({ "volume": value }),
                    label: "Outputs · talkback · microphone level".into(),
                    part: self.part(),
                })
            }
            Target::TalkbackDestination { id } => {
                let (key, name) = TALKBACK_KEYS.get(*id as usize).copied().ok_or_else(|| format!("there is no talkback destination {id}"))?;
                let value = section("outputs").get("talkback").and_then(|t| t.get(key)).and_then(Json::as_i64).ok_or_else(|| format!("the snapshot has no talkback to {name}"))?;
                Ok(Write {
                    command: "set_tbk_enable".into(),
                    ext3: None,
                    args: json!({ "id": id, "enabled": value }),
                    label: format!("Outputs · talkback · to {name}"),
                    part: self.part(),
                })
            }
            Target::SyncSource => {
                let value = section("clock").get("sync_source").and_then(Json::as_i64).ok_or("the snapshot has no clock source")?;
                Ok(Write { command: "set_sync_source".into(), ext3: None, args: json!({ "src_index": value }), label: "Clock · source".into(), part: self.part() })
            }
            Target::SampleRate => {
                let value = section("clock").get("rate_index").and_then(Json::as_i64).ok_or("the snapshot has no sample rate")?;
                Ok(Write { command: "set_samp_rate".into(), ext3: None, args: json!({ "srate_idx": value }), label: "Clock · sample rate".into(), part: self.part() })
            }
            Target::SpdifSrc => {
                let value = section("clock").get("spdif_src").and_then(Json::as_i64).ok_or("the snapshot has no S/PDIF sample-rate converter")?;
                Ok(Write {
                    command: "set_spdif_src".into(),
                    ext3: None,
                    args: json!({ "spdif_src": value }),
                    label: "Clock · S/PDIF sample-rate converter".into(),
                    part: self.part(),
                })
            }
            Target::Brightness => {
                let value = section("settings").get("brightness").and_then(Json::as_i64).ok_or("the snapshot has no brightness")?;
                Ok(Write { command: "set_brightness".into(), ext3: None, args: json!({ "brightness": value }), label: "Settings · brightness".into(), part: self.part() })
            }
            Target::PanningLaw => {
                let value = section("settings").get("panning_law").and_then(Json::as_i64).ok_or("the snapshot has no panning law")?;
                Ok(Write { command: "set_panning_law".into(), ext3: None, args: json!({ "panning": value }), label: "Settings · panning law".into(), part: self.part() })
            }
            Target::DcCoupled { io } => {
                let key = if *io == 0 { "dc_coupled_in" } else { "dc_coupled_out" };
                let side = if *io == 0 { "inputs" } else { "outputs" };
                let value = section("settings").get(key).and_then(Json::as_i64).ok_or_else(|| format!("the snapshot has no DC coupling for the {side}"))?;
                Ok(Write {
                    command: "set_dc_coupled".into(),
                    ext3: None,
                    args: json!({ "dc_coupled": value, "dc_coupled_io": io }),
                    label: format!("DC coupling · {side}"),
                    part: self.part(),
                })
            }
        }
    }
}

pub fn output_name(id: u32) -> &'static str {
    OUTPUT_NAMES.get(id as usize).copied().unwrap_or("output")
}

fn mixer_label(mix: u32, channel: u32) -> String {
    if channel == 0 {
        format!("Mixer · Mix {} · master", mix + 1)
    } else {
        format!("Mixer · Mix {} · strip {}", mix + 1, channel)
    }
}

fn preamp_field(inputs: &Json, id: u32, names: &[&str]) -> Result<i64, String> {
    let entry = inputs.get("preamps").and_then(|p| p.get(id as usize)).ok_or_else(|| format!("the snapshot has no preamp {}", id + 1))?;
    names
        .iter()
        .find_map(|name| entry.get(*name).and_then(Json::as_i64))
        .ok_or_else(|| format!("the snapshot's preamp {} has no '{}'", id + 1, names.join("' or '")))
}

/// A captured gain array as bytes. Capture records `byte * N` fields as a hex string (`value.rs`),
/// so `preamp_gains` is one value in the snapshot and one command per channel on the wire.
pub fn gain_bytes(inputs: &Json, kind: GainKind) -> Result<Vec<i8>, String> {
    let text = inputs.get(kind.key()).and_then(Json::as_str).ok_or_else(|| format!("the snapshot has no {} gains", kind.label()))?;
    let bytes = crate::value::from_hex(text).map_err(|e| format!("the snapshot's {} gains are not readable: {e}", kind.label()))?;
    Ok(bytes.into_iter().map(|b| b as i8).collect())
}

fn output_field(outputs: &Json, family: &str, id: u32, field: &str) -> Result<i64, String> {
    let value = if family == "quadro" {
        outputs.get("volumes").and_then(|v| v.get(id as usize)).and_then(|o| o.get(field)).and_then(Json::as_i64)
    } else {
        let key = STUDIO_OUTPUT_KEYS.get(id as usize).copied().unwrap_or_default();
        // The Studio+ reports no dim, so a dim step is never planned for one.
        let field = if field == "dim_on" { "dim" } else { field };
        outputs.get(key).and_then(|o| o.get(field)).and_then(Json::as_i64)
    };
    value.ok_or_else(|| format!("the snapshot has no {field} for {}", output_name(id)))
}

/// `mixes[0][1]` as `("mixes", [0, 1])`, the diff's own spelling of an index.
fn split_indices(part: &str) -> (&str, Vec<u32>) {
    let mut name = part;
    let mut indices: Vec<u32> = Vec::new();
    while let Some(open) = name.rfind('[') {
        let Some(inner) = name[open..].strip_prefix('[').and_then(|s| s.strip_suffix(']')) else { break };
        let Ok(index) = inner.parse::<u32>() else { break };
        indices.insert(0, index);
        name = &name[..open];
    }
    (name, indices)
}

/// What recall would do about one path of one section of one family's snapshot.
///
/// This is the table the rest of recall is built on. It is total: an unrecognised path answers
/// [`Writer::Unmapped`], never nothing.
pub fn writer_for(family: &str, section: &str, path: &str) -> Writer {
    let parts: Vec<&str> = path.split('.').collect();
    let (head, indices) = split_indices(parts.first().copied().unwrap_or_default());
    let tail = parts.get(1).copied().unwrap_or_default();
    match section {
        "mixer" => match (head, indices.as_slice(), parts.len()) {
            ("mixes", [mix, channel], 2) if matches!(tail, "level" | "pan" | "mute" | "solo") || (family == "studio" && tail == "send") => {
                Writer::Command(Target::MixerStrip { mix: *mix, channel: *channel })
            }
            ("links", [_], 2) if tail == "linked" => Writer::Withheld {
                command: "set_stereo_link",
                reason: "a linked pair may mirror a write to its partner, which would undo the value recall had just put on the other channel. Until a hardware session shows whether it does, recall writes every channel its own captured value and never sets a link flag (spec §2.3.6)",
            },
            _ => Writer::Unmapped,
        },
        "routing" => {
            let group = parts.first().copied().unwrap_or_default().to_string();
            let (slot_name, slot_indices) = split_indices(parts.get(1).copied().unwrap_or_default());
            match (parts.len(), parts.get(1).copied().unwrap_or_default(), slot_name, slot_indices.as_slice()) {
                (2, "bank_idx", _, _) => Writer::Command(Target::RoutingGroup { group }),
                (3, _, "bank_configs", [_]) if matches!(parts[2], "in_periph_id" | "in_chann") => Writer::Command(Target::RoutingGroup { group }),
                _ => Writer::Unmapped,
            }
        }
        "inputs" => match (head, indices.as_slice()) {
            ("preamps", [id]) => match tail {
                "type" | "pretype" => Writer::Command(Target::PreampType { id: *id }),
                "phantom" => Writer::Command(Target::PreampPhantom { id: *id }),
                "phase_inv" => Writer::Command(Target::PreampPhase { id: *id }),
                "hpf" => Writer::NoWriter {
                    reason: "neither panel has a command for the high-pass filter: the device reports it and nothing sets it (spec §2.2)",
                },
                "zero_cross" => Writer::NoWriter { reason: "the device reports zero-crossing; no command in either registry sets it" },
                _ => Writer::Unmapped,
            },
            ("preamp_gains", []) => Writer::Command(Target::Gains { kind: GainKind::Preamp }),
            ("line_gains", []) => Writer::Command(Target::Gains { kind: GainKind::Line }),
            ("adat_gains", []) if family == "quadro" => Writer::Withheld {
                command: "set_adat_gain",
                reason: "the Quadro's panel never sends set_adat_gain, so nobody knows whether the Quadro accepts it. Captured, and not recalled until a hardware probe (P40, spec §2.2, Q11)",
            },
            ("spdif_gains", []) if family == "quadro" => Writer::Withheld {
                command: "set_spdif_gain",
                reason: "the Quadro's panel never sends set_spdif_gain, so nobody knows whether the Quadro accepts it. Captured, and not recalled until a hardware probe (P40, spec §2.2, Q11)",
            },
            ("adat_gains", []) => Writer::Command(Target::Gains { kind: GainKind::Adat }),
            ("spdif_gains", []) => Writer::Command(Target::Gains { kind: GainKind::Spdif }),
            ("links", []) => match (split_indices(tail).0, parts.len()) {
                ("preamp" | "line" | "adat" | "spdif", 3) if parts[2] == "linked" => Writer::Withheld {
                    command: "set_stereo_link",
                    reason: "a linked pair may mirror a write to its partner. Until a hardware session shows whether it does, recall writes every channel its own captured value and never sets a link flag (spec §2.3.6); on the Quadro only pair 1 is even readable (P47, P50)",
                },
                _ => Writer::Unmapped,
            },
            ("emulations", [channel]) if matches!(tail, "target" | "emu_model" | "ch_swap" | "pattern") => Writer::Command(Target::Emulation { channel: *channel }),
            _ => Writer::Unmapped,
        },
        "outputs" => match (head, indices.as_slice()) {
            ("volumes", [id]) => match tail {
                _ if *id >= topology::output_ids(family).unwrap_or(0) => Writer::NoWriter {
                    reason: "the Quadro reports six volume words and its set_volume ids stop at Line out: volumes 5 and 6 are bound to no command (spec Q12)",
                },
                "volume" => Writer::Command(Target::OutputVolume { id: *id }),
                "mute" => Writer::Command(Target::OutputMute { id: *id }),
                "dim_on" => Writer::Command(Target::OutputDim { id: *id }),
                "mono" => Writer::NoWriter { reason: "the Quadro reports mono; neither model has a command that sets it" },
                "trim" => Writer::NoWriter {
                    reason: "the trim inside the volume word is the device's own read-back; the trims recall writes come from monitor_trim, line_out_trim and adc_trim",
                },
                _ => Writer::Unmapped,
            },
            ("hard_mute", []) => Writer::Command(Target::HardMute),
            ("trims", []) => match (tail, family) {
                ("adc", "quadro") => Writer::NoWriter {
                    reason: "the Quadro's panel offers Monitor and Line out trims only; its ADC trim is reported and not set (P56)",
                },
                (key, _) => match TRIM_KEYS.iter().position(|(k, _)| *k == key) {
                    Some(id) => Writer::Command(Target::Trim { id: id as u32 }),
                    None => Writer::Unmapped,
                },
            },
            ("trim_configs", [_]) => Writer::Withheld {
                command: "set_trim_config",
                reason: "get_trim_configs answers one entry with no selector, so which trim it describes is not established. The trims recall writes come from the cyclic monitor_trim and line_out_trim instead; a hardware session should settle what this reply is",
            },
            ("talkback", []) => match tail {
                "mic_volume" => Writer::Command(Target::TalkbackVolume),
                key => match TALKBACK_KEYS.iter().position(|(k, _)| *k == key) {
                    Some(id) => Writer::Command(Target::TalkbackDestination { id: id as u32 }),
                    None => Writer::Unmapped,
                },
            },
            (key, []) if STUDIO_OUTPUT_KEYS.contains(&key) => {
                let id = STUDIO_OUTPUT_KEYS.iter().position(|k| *k == key).unwrap_or_default() as u32;
                match tail {
                    "volume" => Writer::Command(Target::OutputVolume { id }),
                    "mute" => Writer::Command(Target::OutputMute { id }),
                    _ => Writer::Unmapped,
                }
            }
            _ => Writer::Unmapped,
        },
        "clock" => match head {
            "sync_source" => Writer::Command(Target::SyncSource),
            "rate_index" => Writer::Command(Target::SampleRate),
            "spdif_src" => Writer::Command(Target::SpdifSrc),
            _ => Writer::Unmapped,
        },
        "settings" => match head {
            "brightness" => Writer::Command(Target::Brightness),
            "panning_law" => Writer::Command(Target::PanningLaw),
            "dc_coupled_in" => Writer::Command(Target::DcCoupled { io: 0 }),
            "dc_coupled_out" => Writer::Command(Target::DcCoupled { io: 1 }),
            // The diff puts the device's preset slot in the settings section.
            "current_preset" => Writer::NoWriter {
                reason: "the preset slot is recorded and never recalled: nobody has established what preset_recall changes on the device (spec §2.4)",
            },
            _ => Writer::Unmapped,
        },
        _ => Writer::Unmapped,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn inputs() -> Json {
        json!({
            "preamps": [{"type": 0, "phantom": 1, "hpf": 0, "phase_inv": 1, "zero_cross": 0}],
            "preamp_gains": "0a14",
            "adat_gains": "01ff",
            "emulations": [{"target": 1, "emu_model": 2, "ch_swap": 0, "pattern": 3}],
        })
    }

    fn quadro() -> BTreeMap<String, Json> {
        BTreeMap::from([
            ("mixer".into(), json!({"mixes[0]": [{"level": 0, "pan": 32, "mute": 0, "solo": 0}, {"level": 12, "pan": 30, "mute": 1, "solo": 0}]})),
            ("routing".into(), json!({"MIXER_IN0": {"bank_idx": 8, "bank_configs": (0..64).map(|i| json!({"in_periph_id": i % 7, "in_chann": i})).collect::<Vec<_>>()}})),
            ("inputs".into(), inputs()),
            ("outputs".into(), json!({"volumes": [{"volume": 11, "mute": 0, "dim_on": 1, "mono": 0, "trim": 2}], "hard_mute": 0, "trims": {"monitor": 3, "line_out": 1, "adc": 7}})),
            ("clock".into(), json!({"sync_source": 5, "rate_index": 2})),
            ("settings".into(), json!({"brightness": 60, "panning_law": 2, "dc_coupled_in": 0, "dc_coupled_out": 1})),
        ])
    }

    fn studio() -> BTreeMap<String, Json> {
        BTreeMap::from([
            ("mixer".into(), json!({"mixes[1]": [{"level": 0, "pan": 32, "mute": 0, "solo": 0, "send": 4}]})),
            ("inputs".into(), json!({"preamps": [{"pretype": 1, "phantom": 0, "hpf": 0, "phase_inv": 0, "zero_cross": 0}], "line_gains": "06", "adat_gains": "0102"})),
            ("outputs".into(), json!({"monitor": {"volume": 20, "mute": 0}, "hp1": {"volume": 30, "mute": 1}, "trims": {"monitor": 1, "line_out": 2, "adc": 4}, "talkback": {"mic_volume": 28, "to_hp1": 1, "to_hp2": 0, "to_monitor": 1}})),
            ("clock".into(), json!({"sync_source": 1, "rate_index": 3, "spdif_src": 1})),
            ("settings".into(), json!({"brightness": 40})),
        ])
    }

    #[test]
    fn a_mixer_strips_fields_share_one_command_that_carries_the_whole_strip() {
        // Four paths, one target: set_mixer replaces a strip, so recall sends it once.
        for field in ["level", "pan", "mute", "solo"] {
            assert_eq!(
                writer_for("quadro", "mixer", &format!("mixes[0][1].{field}")),
                Writer::Command(Target::MixerStrip { mix: 0, channel: 1 })
            );
        }
        // `send` is the Studio+'s alone.
        assert_eq!(writer_for("quadro", "mixer", "mixes[0][1].send"), Writer::Unmapped);
        assert_eq!(writer_for("studio", "mixer", "mixes[0][1].send"), Writer::Command(Target::MixerStrip { mix: 0, channel: 1 }));

        let write = Target::MixerStrip { mix: 0, channel: 1 }.build("quadro", &quadro()).expect("a strip");
        assert_eq!(write.command, "set_mixer");
        assert_eq!(write.args, json!({"mixer_id": 0, "channel": 1, "level": 12, "pan": 30, "mute": 1, "solo": 0}));
        assert_eq!(write.label, "Mixer · Mix 1 · strip 1");
        assert_eq!(Target::MixerStrip { mix: 0, channel: 0 }.build("quadro", &quadro()).expect("the master").label, "Mixer · Mix 1 · master");

        let studio = Target::MixerStrip { mix: 1, channel: 0 }.build("studio", &studio()).expect("a strip");
        assert_eq!(studio.command, "set_mixer_cfg");
        assert_eq!(studio.args["send"], json!(4));
    }

    #[test]
    fn a_routing_group_is_one_command_of_thirty_two_slots_at_its_topology_position() {
        for path in ["MIXER_IN0.bank_idx", "MIXER_IN0.bank_configs[3].in_periph_id", "MIXER_IN0.bank_configs[31].in_chann"] {
            assert_eq!(writer_for("quadro", "routing", path), Writer::Command(Target::RoutingGroup { group: "MIXER_IN0".into() }));
        }
        let write = Target::RoutingGroup { group: "MIXER_IN0".into() }.build("quadro", &quadro()).expect("a group");
        assert_eq!(write.command, "set_routing");
        // The Quadro answers 64 slots and set_routing replaces 32: the first 32 are written back.
        assert_eq!(write.args["bank_configs"].as_array().map(Vec::len), Some(64));
        assert_eq!(write.args["bank_idx"], json!(8), "the wire position comes from the topology, not the snapshot");
        assert_eq!(write.args["bank_configs"][2], json!(1), "slot 2's source");

        // A group the model does not have, and one read too short to replace, are refused rather
        // than sent half-built.
        assert!(Target::RoutingGroup { group: "ADAT_OUT0".into() }.build("quadro", &quadro()).is_err());
        let mut short = quadro();
        short.insert("routing".into(), json!({"MIXER_IN0": {"bank_configs": [{"in_periph_id": 0, "in_chann": 0}]}}));
        assert!(Target::RoutingGroup { group: "MIXER_IN0".into() }.build("quadro", &short).unwrap_err().contains("set_routing replaces 32"));
    }

    #[test]
    fn the_two_families_spell_preamp_type_and_phase_differently() {
        assert_eq!(writer_for("quadro", "inputs", "preamps[0].type"), Writer::Command(Target::PreampType { id: 0 }));
        assert_eq!(writer_for("studio", "inputs", "preamps[0].pretype"), Writer::Command(Target::PreampType { id: 0 }));
        assert_eq!(Target::PreampPhase { id: 0 }.build("quadro", &quadro()).expect("phase").command, "set_pre_phase_inv");
        assert_eq!(Target::PreampPhase { id: 0 }.build("studio", &studio()).expect("phase").command, "set_pre_phaseinv");
        assert_eq!(Target::PreampType { id: 0 }.build("studio", &studio()).expect("type").args, json!({"id": 0, "pretype": 1}));
    }

    #[test]
    fn a_gain_array_is_one_captured_value_and_one_command_per_channel() {
        assert_eq!(writer_for("quadro", "inputs", "preamp_gains"), Writer::Command(Target::Gains { kind: GainKind::Preamp }));
        assert_eq!(gain_bytes(&inputs(), GainKind::Preamp).expect("gains"), vec![10, 20]);
        // Gains are signed: -6 dB comes back as 0xfa, not 250.
        assert_eq!(gain_bytes(&inputs(), GainKind::Adat).expect("gains"), vec![1, -1]);
        let write = Target::Gain { kind: GainKind::Preamp, id: 1 }.build("quadro", &quadro()).expect("a gain");
        assert_eq!((write.command.as_str(), write.args), ("set_pre_gain", json!({"id": 1, "gain": 20})));
        assert!(Target::Gains { kind: GainKind::Preamp }.build("quadro", &quadro()).is_err(), "the array itself is not one command");
    }

    #[test]
    fn what_the_devices_report_and_nothing_sets_is_named_rather_than_skipped() {
        for (family, section, path, expect) in [
            ("quadro", "inputs", "preamps[0].hpf", "high-pass filter"),
            ("quadro", "inputs", "preamps[0].zero_cross", "zero-crossing"),
            ("quadro", "outputs", "volumes[0].mono", "mono"),
            ("quadro", "outputs", "volumes[4].volume", "bound to no command"),
            ("quadro", "outputs", "trims.adc", "reported and not set"),
            ("quadro", "settings", "current_preset", "never recalled"),
        ] {
            match writer_for(family, section, path) {
                Writer::NoWriter { reason } => assert!(reason.contains(expect), "{path}: {reason}"),
                other => panic!("{path} should have no writer, not {other:?}"),
            }
        }
        // The Studio+ has five outputs, so its fifth volume is written, not refused.
        assert_eq!(writer_for("studio", "outputs", "reamp.volume"), Writer::Command(Target::OutputVolume { id: 4 }));
    }

    #[test]
    fn what_waits_for_a_hardware_session_says_which_command_and_why() {
        for (family, section, path, command) in [
            ("quadro", "inputs", "adat_gains", "set_adat_gain"),
            ("quadro", "inputs", "spdif_gains", "set_spdif_gain"),
            ("quadro", "mixer", "links[3].linked", "set_stereo_link"),
            ("studio", "inputs", "links.preamp[1].linked", "set_stereo_link"),
            ("quadro", "outputs", "trim_configs[0].control", "set_trim_config"),
        ] {
            match writer_for(family, section, path) {
                Writer::Withheld { command: c, reason } => {
                    assert_eq!(c, command, "{path}");
                    assert!(reason.len() > 40, "{path} needs a reason a person can act on");
                }
                other => panic!("{path} should be withheld, not {other:?}"),
            }
        }
        // The Studio+'s panel does send them, so they are ordinary writes there.
        assert_eq!(writer_for("studio", "inputs", "adat_gains"), Writer::Command(Target::Gains { kind: GainKind::Adat }));
        assert_eq!(writer_for("studio", "inputs", "spdif_gains"), Writer::Command(Target::Gains { kind: GainKind::Spdif }));
        assert_eq!(writer_for("studio", "inputs", "line_gains"), Writer::Command(Target::Gains { kind: GainKind::Line }));
    }

    #[test]
    fn the_outputs_trims_talkback_clock_and_settings_build_the_panels_own_commands() {
        let q = quadro();
        assert_eq!(Target::OutputVolume { id: 0 }.build("quadro", &q).expect("volume").args, json!({"id": 0, "volume": 11}));
        assert_eq!(Target::OutputDim { id: 0 }.build("quadro", &q).expect("dim").args, json!({"periph_id": 0, "dim": 1}));
        assert_eq!(Target::HardMute.build("quadro", &q).expect("hard mute").args, json!({"value": 0}));
        let trim = Target::Trim { id: 0 }.build("quadro", &q).expect("trim");
        assert_eq!(trim.command, "set_trim_config");
        assert_eq!(trim.args["level"].as_array().map(Vec::len), Some(64));
        assert_eq!(trim.args["level"][0], json!(3), "the step goes in the first entry, as the panel sends it");
        assert_eq!(trim.args["control"], json!(1));
        assert_eq!(Target::DcCoupled { io: 1 }.build("quadro", &q).expect("dc").args, json!({"dc_coupled": 1, "dc_coupled_io": 1}));
        assert_eq!(Target::PanningLaw.build("quadro", &q).expect("panning").args, json!({"panning": 2}));

        let s = studio();
        assert_eq!(Target::OutputVolume { id: 1 }.build("studio", &s).expect("volume").args, json!({"id": 1, "volume": 30}));
        assert_eq!(Target::OutputMute { id: 1 }.build("studio", &s).expect("mute").args, json!({"id": 1, "mute": 1}));
        let trim = Target::Trim { id: 2 }.build("studio", &s).expect("trim");
        assert_eq!((trim.command.as_str(), trim.args), ("set_trim", json!({"id": 2, "trim_idx": 4})));
        assert_eq!(Target::TalkbackVolume.build("studio", &s).expect("talkback").args, json!({"volume": 28}));
        assert_eq!(Target::TalkbackDestination { id: 2 }.build("studio", &s).expect("talkback").args, json!({"id": 2, "enabled": 1}));
        assert_eq!(Target::SpdifSrc.build("studio", &s).expect("src").args, json!({"spdif_src": 1}));
        assert_eq!(Target::SyncSource.build("studio", &s).expect("clock").args, json!({"src_index": 1}));
        assert_eq!(Target::SampleRate.build("studio", &s).expect("rate").args, json!({"srate_idx": 3}));
        assert_eq!(Target::Brightness.build("studio", &s).expect("brightness").args, json!({"brightness": 40}));
        assert_eq!(Target::Emulation { channel: 0 }.build("quadro", &q).expect("emulation").args, json!({"preamp_ch": 0, "target": 1, "emu_model": 2, "ch_swap": 0, "pattern": 3}));
    }

    #[test]
    fn a_snapshot_missing_what_a_command_needs_refuses_rather_than_sending_a_default() {
        let empty = BTreeMap::new();
        for target in [Target::OutputVolume { id: 0 }, Target::SyncSource, Target::Brightness, Target::HardMute, Target::MixerStrip { mix: 0, channel: 1 }] {
            assert!(target.build("quadro", &empty).is_err(), "{target:?} built a command out of nothing");
        }
    }

    #[test]
    fn the_parts_carry_the_specs_defaults_and_its_order() {
        assert!(Part::Mixer.default_on() && Part::Routing.default_on() && Part::Inputs.default_on() && Part::Outputs.default_on());
        assert!(!Part::Phantom.default_on() && !Part::Clock.default_on() && !Part::DcCoupling.default_on() && !Part::Settings.default_on());
        assert!(Part::Phantom.needs_confirming() && Part::Clock.needs_confirming() && Part::DcCoupling.needs_confirming());
        assert!(!Part::Mixer.needs_confirming());
        let stages: Vec<u32> = PARTS.iter().map(|p| p.stage()).collect();
        assert!(stages.windows(2).all(|w| w[0] <= w[1]), "PARTS is in running order: {stages:?}");
        // 48V runs with the inputs but last among them; the mutes come after everything.
        assert_eq!(Part::Phantom.stage(), Part::Inputs.stage());
        assert!(Target::PreampPhantom { id: 0 }.order() > Target::Gain { kind: GainKind::Preamp, id: 31 }.order());
        assert!(Target::PreampType { id: 31 }.order() < Target::Gain { kind: GainKind::Preamp, id: 0 }.order());
        assert!(Target::OutputMute { id: 0 }.order() > Target::OutputVolume { id: 4 }.order());
        assert!(Target::HardMute.order() > Target::OutputMute { id: 4 }.order(), "hard mute comes off last of all");
    }

    #[test]
    fn a_path_this_table_has_never_heard_of_is_unmapped_rather_than_ignored() {
        assert_eq!(writer_for("quadro", "inputs", "preamps[0].something_new"), Writer::Unmapped);
        assert_eq!(writer_for("quadro", "outputs", "brand_new_thing"), Writer::Unmapped);
        assert_eq!(writer_for("quadro", "effects", "anything"), Writer::Unmapped);
        assert_eq!(writer_for("quadro", "clock", "new_clock_value"), Writer::Unmapped);
    }
}
