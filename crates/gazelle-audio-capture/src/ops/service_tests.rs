//! Live probes through operations. The operator's Done commands come from this in-crate test,
//! standing in for the panel; agents have no operation that can issue them.

use super::*;
use crate::capture::sources::SourceKind;
use crate::session::authority::OperatorAuthority;
use crate::session::clock::ManualClock;
use crate::session::plan::StepKind;
use crate::session::step::OperatorCommand;

const T0: u64 = 1_700_000_000_000_000_000;
const HALF_SECOND: u64 = 500_000_000;

fn demo_ops(dir: &Path) -> (Ops, Arc<ManualClock>) {
    let clock = Arc::new(ManualClock::new(T0));
    let ops = Ops::with_clock(Environment::default(), clock.clone()).with_sources(SourceSettings { kind: SourceKind::Demo, ..SourceSettings::default() });
    ops.call("session_open", json!({ "path": dir.join("s").display().to_string(), "vid": 0x1234, "pid": 0xABCD })).unwrap();
    for parameter in [
        json!({ "id": "monitor_level", "label": "Monitor level", "kind": "discrete", "domain": { "values": ["0 dB", "-6 dB"] } }),
        json!({ "id": "mute", "label": "Mute", "kind": "toggle", "domain": { "values": ["off", "on"] } }),
    ] {
        ops.call("declare_parameter", json!({ "parameter": parameter })).unwrap();
    }
    (ops, clock)
}

fn plan(ops: &Ops) -> String {
    let plan = json!({ "parameter": "monitor_level", "value_a": "0 dB", "value_b": ["-6 dB"], "repeats": 1, "control_parameter": "mute" });
    ops.call("plan_probe", json!({ "plan": plan, "seed": 1 })).unwrap()["probe_id"].as_str().unwrap().to_string()
}

/// Plays the operator on virtual time: presses Done on every ready non-idle step until the
/// probe leaves `running`.
fn operate_to_completion(ops: &Ops, clock: &ManualClock) {
    let controller = ops.controller().unwrap();
    let authority = OperatorAuthority::grant();
    for _ in 0..400 {
        clock.advance(HALF_SECOND);
        controller.tick().unwrap();
        let Some(probe) = controller.state().probe else {
            continue;
        };
        if probe.status != RunStatus::Running {
            return;
        }
        if probe.step_state == "armed" && probe.kind != StepKind::Idle && probe.ready_in_ms == 0 {
            controller.operator(&authority, OperatorCommand::Done).unwrap();
        }
    }
    panic!("the probe did not complete within 200 virtual seconds");
}

#[test]
fn a_live_probe_runs_through_operations_to_completion() {
    let dir = tempfile::tempdir().unwrap();
    let (ops, clock) = demo_ops(dir.path());
    let probe = plan(&ops);

    let started = ops.call("start_probe", json!({ "probe_id": probe })).unwrap();
    assert_eq!(started["started"], "p1");
    assert_eq!(started["state"]["probe"]["status"], "running");

    let waited = ops.call("await_progress", json!({ "timeout_ms": 60 })).unwrap();
    assert_eq!(waited["reason"], "timeout", "nobody has operated yet");

    operate_to_completion(&ops, &clock);
    let waited = ops.call("await_progress", json!({ "timeout_ms": 60 })).unwrap();
    assert_eq!(waited["reason"], "completed");

    let status = ops.call("session_status", Value::Null).unwrap();
    assert_eq!(status["probes"][0]["outcome"], "completed");
    assert_eq!(status["probes"][0]["closed_steps"], 7);
    assert!(matches!(ops.call("abandon_probe", Value::Null), Err(OpsError::Control(_))), "nothing is running");
}

#[test]
fn a_probe_can_be_abandoned_and_unknown_probes_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (ops, _clock) = demo_ops(dir.path());
    assert!(matches!(ops.call("start_probe", json!({ "probe_id": "p7" })), Err(OpsError::Control(_))));
    assert!(matches!(ops.call("await_progress", json!({ "timeout_ms": 10 })).unwrap()["reason"].as_str(), Some("no_probe")));

    let probe = plan(&ops);
    ops.call("start_probe", json!({ "probe_id": probe })).unwrap();
    let abandoned = ops.call("abandon_probe", json!({})).unwrap();
    assert_eq!(abandoned["state"]["probe"]["status"], "abandoned");
    assert_eq!(ops.call("await_progress", json!({ "timeout_ms": 10 })).unwrap()["reason"], "abandoned");
}

#[test]
fn await_progress_needs_an_open_session() {
    let ops = Ops::new(Environment::default());
    assert!(matches!(ops.call("await_progress", json!({})), Err(OpsError::NoSession)));
}
