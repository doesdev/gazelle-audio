//! Field maps and reports from synthetic probes (spec §8, field map; §11 row 2).

use gazelle_audio_capture::analysis::encoding::Model;
use gazelle_audio_capture::analysis::fieldmap::{descriptor_hash, field_map, report, short_template, FieldMap, ProbeInput, SCHEMA_VERSION};
use gazelle_audio_capture::capture::event::Direction;
use gazelle_audio_capture::capture::pipeline::PayloadPolicy;
use gazelle_audio_capture::session::model::{Parameter, ParameterDomain, ParameterKind, ProbePlan};
use gazelle_audio_capture::session::step::StepTiming;
use gazelle_audio_capture::session::timeline::probe_timelines;
use gazelle_audio_capture::synth::device::*;
use gazelle_audio_capture::synth::session::{generate_session, ScriptedOperator, SynthSpec};

const LEVELS: &[&str] = &["0 dB", "-6 dB", "-12 dB"];

fn parameters() -> Vec<Parameter> {
    vec![
        Parameter { id: "monitor_level".into(), label: "Monitor level".into(), kind: ParameterKind::Discrete, domain: ParameterDomain { values: LEVELS.iter().map(|s| s.to_string()).collect(), unit: Some("dB".into()) }, location: String::new() },
        Parameter { id: "mute".into(), label: "Mute".into(), kind: ParameterKind::Toggle, domain: ParameterDomain { values: vec!["off".into(), "on".into()], unit: None }, location: String::new() },
    ]
}

fn plan() -> ProbePlan {
    ProbePlan { parameter: "monitor_level".into(), value_a: "0 dB".into(), value_b: vec!["-6 dB".into(), "-12 dB".into()], sweep: vec![], repeats: 3, control_parameter: "mute".into() }
}

fn map_for(target: RichDevice, operator: ScriptedOperator) -> FieldMap {
    map_with_plan(target, operator, plan())
}

fn map_with_plan(target: RichDevice, operator: ScriptedOperator, plan: ProbePlan) -> FieldMap {
    let dir = tempfile::tempdir().unwrap();
    let spec = SynthSpec {
        parameters: parameters(),
        plan,
        seed: 5,
        timing: StepTiming::default(),
        start_ns: 1_700_000_000_000_000_000,
        operator,
        link_type: 249,
        policy: PayloadPolicy::default(),
    };
    let devices: Vec<Box<dyn DeviceModel>> = vec![Box::new(target), Box::new(SimpleDevice::new(0x046D, 0xC52B, 1, 3))];
    let result = generate_session(&dir.path().join("session"), spec, devices).unwrap();
    let info = result.store.info().unwrap();
    let marks = result.store.marks().unwrap();
    let timeline = probe_timelines(&marks).remove(0);
    let input = ProbeInput {
        timeline: &timeline,
        events: &result.events,
        capture: format!("captures/{}.pcapng", timeline.probe),
        vid: info.vid,
        pid: info.pid,
        descriptor_hex: info.device_descriptor_hex.as_deref(),
    };
    field_map(&input, "monitor_level")
}

fn target() -> RichDevice {
    RichDevice::new(0x1234, 0xABCD, 1, 5).with_parameter("monitor_level", LEVELS).with_parameter("mute", &["off", "on"])
}

#[test]
fn a_clean_probe_yields_a_complete_field_map() {
    let map = map_for(target(), ScriptedOperator::default());
    assert_eq!((map.schema_version, map.parameter.as_str()), (SCHEMA_VERSION, "monitor_level"));
    assert_eq!((map.device.vid, map.device.pid), (0x1234, 0xABCD));
    assert!(map.device.descriptor_hash.as_deref().is_some_and(|h| h.starts_with("fnv1a64:")));

    let command = map.command.as_ref().expect("command attributed");
    assert_eq!((command.channel.endpoint.as_str(), command.channel.direction), ("0x01", Direction::Out));
    assert_eq!(command.channel.discriminator.as_deref(), Some("0x70"));
    assert_eq!((command.field.byte, command.field.bits), (RICH_VALUE, [0, 1]));
    assert!(matches!(command.encoding.model, Model::Linear { .. }), "{:?}", command.encoding.model);
    assert_eq!(command.confidence, 1.0);

    let readback = map.readback.as_ref().expect("readback attributed");
    assert_eq!(readback.channel.discriminator.as_deref(), Some("0x73"));
    assert_eq!(readback.field.byte, RICH_READBACK_BASE, "the steady readback wins over the peak meter");
    let meter_caveat = format!("byte {}", RICH_PEAK_BASE);
    assert!(map.caveats.iter().any(|c| c.contains("meter-like") && c.contains(&meter_caveat)), "{:?}", map.caveats);

    assert!(map.shared_with.is_empty());
    assert!(map.recommendations.is_empty(), "{:?}", map.recommendations);
    assert!(map.caveats.iter().any(|c| c.contains("masked") && c.contains("counter")), "{:?}", map.caveats);
    assert_eq!(map.evidence.len(), 1);
    assert_eq!(map.evidence[0].capture, "captures/p1.pcapng");
    assert!(!map.evidence[0].packet_indices.is_empty());

    let json = serde_json::to_value(&map).unwrap();
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["command"]["encoding"]["model"], "linear");
    assert_eq!(json["command"]["channel"]["transfer"], "interrupt");
    assert!(json["command"]["channel"].get("request").is_none(), "absent control fields are omitted");

    assert!(!map.caveats.iter().any(|c| c.contains("distinct values")), "three values are enough: {:?}", map.caveats);

    // One change emits a Set (carrying the value) and, 2 ms later, a Commit.
    let sequence = &command.sequence;
    assert_eq!(sequence.len(), 2, "{sequence:?}");
    assert!(sequence[0].carries_value && sequence[0].offset_ms == 0.0);
    assert_eq!(sequence[0].channel.discriminator.as_deref(), Some("0x70"));
    assert_eq!(sequence[1].channel.discriminator.as_deref(), Some("0x71"));
    assert!(!sequence[1].carries_value);
    assert!((sequence[1].offset_ms - 2.0).abs() < 1e-9, "{}", sequence[1].offset_ms);
    assert!(sequence[1].template.starts_with("71 ?? 01 00"), "{}", sequence[1].template);
    assert!(map.readback.as_ref().unwrap().sequence.is_empty());

    let md = report(&map);
    // RichDevice commands are 32 bytes: 4 header/value bytes then 28 zeros, collapsed for people.
    let short = format!("`70 ?? {:02x} ?? … 28 × 00`", 1);
    for needle in ["# Field map: monitor_level", "## Command", "ep 0x01 out interrupt disc 0x70", short.as_str(), "Sequence of 2 messages per change:", "+2.0 ms ep 0x01 out interrupt disc 0x71", "## Readback", "| -6 dB | 0x01 (1) |", "## Evidence"] {
        assert!(md.contains(needle), "{needle:?} missing from:\n{md}");
    }
    assert!(command.template.ends_with("00 00 00"), "the JSON keeps the full template");
}

#[test]
fn two_values_are_flagged_as_too_few_for_an_encoding() {
    let two = ProbePlan { value_b: vec!["-6 dB".into()], ..plan() };
    let map = map_with_plan(target(), ScriptedOperator::default(), two);
    assert_eq!(map.command.as_ref().unwrap().values.len(), 2);
    assert!(map.caveats.iter().any(|c| c.contains("only 2 distinct values")), "{:?}", map.caveats);
    assert!(map.recommendations.iter().any(|r| r.contains("Add a sweep of monitor_level")), "{:?}", map.recommendations);
}

#[test]
fn short_templates_collapse_only_long_trailing_runs() {
    assert_eq!(short_template("70 ?? 01 ?? 00 00 00 00 00 00 00 00"), "70 ?? 01 ?? … 8 × 00");
    assert_eq!(short_template("70 ?? 00 00 00"), "70 ?? 00 00 00");
    assert_eq!(short_template("?? ?? ?? ?? ?? ?? ?? ??"), "?? ?? ?? ?? ?? ?? ?? ??");
    assert_eq!(short_template("60 60 60 60 60 60 60 60"), "… 8 × 60");
}

#[test]
fn spillover_leaves_no_attribution_and_recommends_probing_the_control() {
    let map = map_for(target().with_spillover("mute", "monitor_level"), ScriptedOperator::default());
    assert!(map.command.is_none() && map.readback.is_none());
    // The command value byte, the readback byte and the peak meter all move with the spill.
    assert_eq!(map.shared_with.len(), 3, "{:?}", map.shared_with);
    assert!(map.shared_with.iter().all(|s| s.parameter == "mute"));
    assert!(map.caveats.iter().any(|c| c.contains("no command field")));
    assert!(map.recommendations.iter().any(|r| r.contains("declare and probe mute")), "{:?}", map.recommendations);
    assert!(report(&map).contains("## Shared fields"));
}

#[test]
fn an_outvoted_step_lowers_command_confidence_but_keeps_the_field() {
    let map = map_for(target().with_command_glitch(2), ScriptedOperator::default());
    let command = map.command.as_ref().expect("majority voting keeps the command field");
    assert_eq!(command.field.byte, RICH_VALUE);
    assert!(command.confidence < 1.0 && command.confidence > 0.9, "{}", command.confidence);
    assert_eq!(map.readback.as_ref().unwrap().confidence, 1.0);
    assert!(map.caveats.iter().any(|c| c.contains("outvoted")), "{:?}", map.caveats);
    assert!(map.recommendations.iter().any(|r| r.contains("repeat ×5")), "{:?}", map.recommendations);
}

#[test]
fn a_redo_lowers_confidence_and_asks_for_more_repeats() {
    // Step 3 is a Set step of the parameter (Idle, Set A, No-op A, Set b1, ...).
    let map = map_for(target(), ScriptedOperator { redo_once: vec![3], ..ScriptedOperator::default() });
    let command = map.command.as_ref().unwrap();
    assert!((command.confidence - 0.95).abs() < 1e-9, "{}", command.confidence);
    assert!(map.recommendations.iter().any(|r| r.contains("repeat ×5")));
}

#[test]
fn descriptor_hash_is_stable_and_case_insensitive() {
    assert_eq!(descriptor_hash("12010002EF"), descriptor_hash("12010002ef"));
    assert_ne!(descriptor_hash("12010002ef"), descriptor_hash("12010002f0"));
    assert_eq!(descriptor_hash("").len(), "fnv1a64:".len() + 16);
}
