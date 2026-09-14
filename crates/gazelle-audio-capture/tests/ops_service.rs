//! The operation service without any transport: sessions, parameters, planning, importing a
//! recorded probe and analysing it.

use gazelle_audio_capture::capture::pipeline::PayloadPolicy;
use gazelle_audio_capture::ops::service::{Ops, OpsError, MAX_EXCERPTS};
use gazelle_audio_capture::session::controller::Environment;
use gazelle_audio_capture::session::model::{Parameter, ParameterDomain, ParameterKind, ProbePlan};
use gazelle_audio_capture::session::step::StepTiming;
use gazelle_audio_capture::synth::device::*;
use gazelle_audio_capture::synth::session::{generate_session, ScriptedOperator, SynthSpec};
use serde_json::{json, Value};

const VID: u16 = 0x1234;
const PID: u16 = 0xABCD;
const LEVELS: &[&str] = &["0 dB", "-6 dB", "-12 dB"];

fn parameters() -> Vec<Parameter> {
    vec![
        Parameter { id: "monitor_level".into(), label: "Monitor level".into(), kind: ParameterKind::Discrete, domain: ParameterDomain { values: LEVELS.iter().map(|s| s.to_string()).collect(), unit: Some("dB".into()) }, location: String::new() },
        Parameter { id: "mute".into(), label: "Mute".into(), kind: ParameterKind::Toggle, domain: ParameterDomain { values: vec!["off".into(), "on".into()], unit: None }, location: String::new() },
    ]
}

/// A recorded RichDevice probe in `<dir>/session`, as `gazelle-capture serve` would leave it.
fn recorded_session(dir: &std::path::Path) -> std::path::PathBuf {
    let root = dir.join("session");
    let spec = SynthSpec {
        parameters: parameters(),
        plan: ProbePlan { parameter: "monitor_level".into(), value_a: "0 dB".into(), value_b: vec!["-6 dB".into(), "-12 dB".into()], sweep: vec![], repeats: 3, control_parameter: "mute".into() },
        seed: 9,
        timing: StepTiming::default(),
        start_ns: 1_700_000_000_000_000_000,
        operator: ScriptedOperator::default(),
        link_type: 249,
        policy: PayloadPolicy::default(),
    };
    let target = RichDevice::new(VID, PID, 1, 5).with_parameter("monitor_level", LEVELS).with_parameter("mute", &["off", "on"]);
    let devices: Vec<Box<dyn DeviceModel>> = vec![Box::new(target), Box::new(SimpleDevice::new(0x046D, 0xC52B, 1, 3))];
    generate_session(&root, spec, devices).unwrap();
    root
}

fn path(p: &std::path::Path) -> String {
    p.display().to_string()
}

#[test]
fn operations_need_an_open_session_and_valid_arguments() {
    let ops = Ops::new(Environment::default());
    assert_eq!(ops.call("session_status", Value::Null).unwrap(), json!({ "open": false }));
    assert!(matches!(ops.call("list_parameters", Value::Null), Err(OpsError::NoSession)));
    assert!(matches!(ops.call("mark_step_done", Value::Null), Err(OpsError::UnknownOperation(_))));
    assert!(matches!(ops.call("session_open", json!({ "path": 5 })), Err(OpsError::BadArguments { .. })));
    assert!(matches!(ops.call("start_probe", json!({ "probe_id": "p1" })), Err(OpsError::NoSession)));

    let dir = tempfile::tempdir().unwrap();
    let missing_target = ops.call("session_open", json!({ "path": path(&dir.path().join("new")) }));
    assert!(matches!(missing_target, Err(OpsError::Invalid(ref m)) if m.contains("vid and pid")), "{missing_target:?}");
}

#[test]
fn a_recorded_probe_is_imported_and_analysed_through_operations() {
    let source = tempfile::tempdir().unwrap();
    let recorded = recorded_session(source.path());
    let target = tempfile::tempdir().unwrap();
    let session = target.path().join("s");

    let ops = Ops::new(Environment::default());
    let status = ops.call("session_open", json!({ "path": path(&session), "vid": VID, "pid": PID })).unwrap();
    assert_eq!((status["open"].clone(), status["vid"].clone()), (json!(true), json!(VID)));
    for parameter in parameters() {
        ops.call("declare_parameter", json!({ "parameter": parameter })).unwrap();
    }
    assert_eq!(ops.call("list_parameters", Value::Null).unwrap().as_array().unwrap().len(), 2);

    // Marks default to the recorded session's marks.jsonl.
    let imported = ops.call("import_capture", json!({ "capture": path(&recorded.join("captures").join("p1.pcapng")) })).unwrap();
    assert_eq!((imported["probe"].as_str(), imported["source_probe"].as_str()), (Some("p1"), Some("p1")));
    assert!(imported["frames"].as_u64().unwrap() > 1000 && imported["marks"].as_u64().unwrap() > 10, "{imported}");

    let analysed = ops.call("analyze_probe", json!({})).unwrap();
    assert_eq!(analysed["probe"], "p1");
    assert_eq!(analysed["field_map"]["command"]["field"]["byte"], json!(RICH_VALUE));
    assert_eq!(analysed["field_map"]["readback"]["field"]["byte"], json!(RICH_READBACK_BASE));
    assert!(analysed["report"].as_str().unwrap().ends_with("monitor_level.md"));

    let map = ops.call("get_field_map", json!({ "parameter": "monitor_level" })).unwrap();
    assert_eq!(map["schema_version"], 1);
    assert!(matches!(ops.call("get_field_map", json!({ "parameter": "mute" })), Err(OpsError::Invalid(_))));

    let evidence = ops.call("get_evidence", json!({ "parameter": "monitor_level" })).unwrap();
    assert!(evidence.get("packets").is_none(), "no raw bytes unless asked");
    let evidence = ops.call("get_evidence", json!({ "parameter": "monitor_level", "include_packets": true })).unwrap();
    let packets = evidence["packets"].as_array().unwrap();
    assert!(!packets.is_empty() && packets.len() <= MAX_EXCERPTS);
    assert!(packets.iter().any(|p| p["data_hex"].as_str().unwrap().starts_with("70")), "a Set command is cited: {evidence}");

    let status = ops.call("session_status", Value::Null).unwrap();
    assert_eq!(status["probes"][0]["probe"], "p1");
    assert_eq!(status["probes"][0]["outcome"], "completed");

    let plan = json!({ "parameter": "monitor_level", "value_a": "0 dB", "value_b": ["-6 dB", "-12 dB"], "repeats": 1, "control_parameter": "mute" });
    let planned = ops.call("plan_probe", json!({ "plan": plan, "seed": 3 })).unwrap();
    assert_eq!(planned["probe_id"], "p2", "the imported probe's id is taken");
    assert_eq!(planned["steps"].as_array().unwrap().len(), 9);

    let other = Ops::new(Environment::default());
    let wrong = other.call("session_open", json!({ "path": path(&session), "vid": 1, "pid": 2 }));
    assert!(matches!(wrong, Err(OpsError::Invalid(ref m)) if m.contains("already targets 1234:abcd")), "{wrong:?}");
}
