use std::sync::Arc;
use std::time::{Duration, Instant};

use super::*;
use crate::capture::import::{ImportSource, MemorySource};
use crate::capture::CaptureStream;
use crate::session::clock::ManualClock;
use crate::session::marks::MarkKind;
use crate::session::model::{ParameterDomain, ParameterKind};
use crate::session::store::SessionInfo;
use crate::session::timeline::probe_timelines;
use crate::synth::device::{DeviceModel, SimpleDevice};
use crate::synth::frames::device_frames;

const S: u64 = 1_000_000_000;
const T0: u64 = 1_700_000_000 * S;

fn parameters() -> Vec<Parameter> {
    vec![
        Parameter { id: "monitor_level".into(), label: "Monitor level".into(), kind: ParameterKind::Continuous, domain: ParameterDomain::default(), location: String::new() },
        Parameter { id: "mute".into(), label: "Mute".into(), kind: ParameterKind::Toggle, domain: ParameterDomain { values: vec!["off".into(), "on".into()], unit: None }, location: String::new() },
    ]
}

fn plan() -> ProbePlan {
    ProbePlan { parameter: "monitor_level".into(), value_a: "0 dB".into(), value_b: vec!["-6 dB".into()], sweep: vec![], repeats: 1, control_parameter: "mute".into() }
}

/// Target 1:5 plus neighbour 1:3, one second of traffic at T0.
fn source() -> (Box<dyn CaptureSource>, u64) {
    let mut devices: Vec<Box<dyn DeviceModel>> = vec![Box::new(SimpleDevice::new(0x1234, 0xABCD, 1, 5)), Box::new(SimpleDevice::new(0x046D, 0xC52B, 1, 3))];
    let frames = device_frames(&mut devices, T0, S);
    let target = frames.iter().filter(|f| u16::from_le_bytes([f.data[19], f.data[20]]) == 5).count() as u64;
    (Box::new(MemorySource::new("memory", frames)), target)
}

fn setup(dir: &std::path::Path) -> (Controller, Arc<ManualClock>) {
    let store = SessionStore::create(dir, &SessionInfo { vid: 0x1234, pid: 0xABCD, ..SessionInfo::default() }).unwrap();
    for p in parameters() {
        store.declare_parameter(p).unwrap();
    }
    let clock = Arc::new(ManualClock::new(T0));
    (Controller::new(store, clock.clone(), StepTiming::default(), Some(false)).unwrap(), clock)
}

fn wait_packets(c: &Controller, n: u64) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while c.state().capture.packets < n {
        assert!(Instant::now() < deadline, "capture thread stalled at {}", c.state().capture.packets);
        std::thread::sleep(Duration::from_millis(5));
        c.tick().unwrap();
    }
}

fn probe(c: &Controller) -> ProbeView {
    c.state().probe.expect("probe running")
}

#[test]
fn plan_probe_validates_and_reserves_ids() {
    let dir = tempfile::tempdir().unwrap();
    let (c, _) = setup(dir.path());
    let mut bad = plan();
    bad.parameter = "gain".into();
    assert!(matches!(c.plan_probe(bad, 1), Err(ControlError::Plan(PlanError::UnknownParameter(_)))));
    let p1 = c.plan_probe(plan(), 1).unwrap();
    assert_eq!((p1.probe_id.as_str(), p1.steps.len()), ("p1", 7));
    assert_eq!(c.plan_probe(plan(), 2).unwrap().probe_id, "p2");
}

#[test]
fn probe_runs_to_completion_and_records_everything() {
    let dir = tempfile::tempdir().unwrap();
    let (c, clock) = setup(dir.path());
    c.plan_probe(plan(), 1).unwrap();
    let (src, target_frames) = source();
    c.start_probe("p1", src).unwrap();
    wait_packets(&c, target_frames);
    let state = c.state();
    assert_eq!((state.vid, state.pid, state.elevated), (0x1234, 0xABCD, Some(false)));
    assert_eq!(state.capture.source.as_deref(), Some("memory"));
    let p = probe(&c);
    assert_eq!((p.step_index, p.kind, p.instruction.as_str()), (0, StepKind::Idle, "Do nothing — idle window"));
    assert_eq!(p.progress, "repeat 1 of 1 · step 1 of 7");

    clock.advance(6_500_000_000);
    c.tick().unwrap();
    assert_eq!(probe(&c).step_state, "done");
    clock.advance(1_500_000_000);
    c.tick().unwrap();
    let p = probe(&c);
    assert_eq!(p.instruction, "Set **Monitor level** to **0 dB**, then press Done");
    assert_eq!((p.ready_in_ms, p.accepts_actual_value), (1500, true));
    assert!(matches!(
        c.operator(&OperatorAuthority::grant(), OperatorCommand::Done),
        Err(ControlError::Step(StepError::TooEarly { wait_ms: 1500 }))
    ));

    while c.state().probe.unwrap().status == RunStatus::Running {
        if probe(&c).kind == StepKind::Idle {
            clock.advance(6_500_000_000);
            c.tick().unwrap();
        } else {
            clock.advance(1_500_000_000);
            c.operator(&OperatorAuthority::grant(), OperatorCommand::Done).unwrap();
        }
        clock.advance(1_500_000_000);
        c.tick().unwrap();
    }
    let state = c.state();
    assert_eq!(state.probe.as_ref().unwrap().status, RunStatus::Completed);
    assert!(!state.capture.running);

    let store = SessionStore::open(dir.path()).unwrap();
    assert_eq!(probe_timelines(&store.marks().unwrap())[0].windows.len(), 7);
    assert!(store.info().unwrap().device_descriptor_hex.unwrap().starts_with("1201"));
    let stored = ImportSource::new(store.capture_path("p1")).start().unwrap().frames.count() as u64;
    assert_eq!(stored, target_frames, "only the target's frames are stored");
    assert!(matches!(c.start_probe("p1", source().0), Err(ControlError::UnknownProbe(_))));
}

#[test]
fn one_running_probe_at_a_time_and_abandon() {
    let dir = tempfile::tempdir().unwrap();
    let (c, _) = setup(dir.path());
    c.plan_probe(plan(), 1).unwrap();
    c.plan_probe(plan(), 2).unwrap();
    c.start_probe("p1", source().0).unwrap();
    assert!(matches!(c.start_probe("p2", source().0), Err(ControlError::ProbeRunning)));
    c.abandon_probe().unwrap();
    assert_eq!(probe(&c).status, RunStatus::Abandoned);
    assert!(matches!(c.abandon_probe(), Err(ControlError::Step(StepError::NotRunning))));
    c.start_probe("p2", source().0).unwrap();
    assert_eq!(probe(&c).probe_id, "p2");
}

#[test]
fn a_probe_never_reuses_a_capture_file() {
    let dir = tempfile::tempdir().unwrap();
    let (c, _) = setup(dir.path());
    c.plan_probe(plan(), 1).unwrap();
    std::fs::write(dir.path().join("captures/p1.pcapng"), b"old").unwrap();
    assert!(matches!(c.start_probe("p1", source().0), Err(ControlError::Session(SessionError::CaptureExists(_)))));
}

#[test]
fn subscribers_see_every_publish() {
    let dir = tempfile::tempdir().unwrap();
    let (c, _) = setup(dir.path());
    let mut rx = c.subscribe();
    assert!(!rx.has_changed().unwrap());
    c.tick().unwrap();
    assert!(rx.has_changed().unwrap());
    assert!(rx.borrow_and_update().seq > 1);
}

#[test]
fn instructions_name_parameters_by_label() {
    let steps = expand(&plan(), 1);
    let text: Vec<String> = steps.iter().map(|s| instruction(s, &parameters())).collect();
    assert_eq!(text[2], "Touch **Monitor level** and leave it at **0 dB**, then press Done");
    assert_eq!(text[3], "Set **Monitor level** from **0 dB** to **-6 dB**, then press Done");
    assert_eq!(text[5], "Change **Mute** to any other value, then press Done");
}

/// A source whose `start` always fails, for the failed-start amendment test below.
struct FailingSource;

impl CaptureSource for FailingSource {
    fn describe(&self) -> String {
        "failing".into()
    }

    fn start(&mut self) -> Result<CaptureStream, CaptureError> {
        Err(CaptureError::Tool("boom".into()))
    }
}

/// Amendment: the source is started before `pN.pcapng` is created, so a source that fails to
/// start leaves no capture file behind, and a retry with the same probe id does not hit
/// `CaptureExists`.
#[test]
fn a_failed_start_leaves_no_capture_file_and_a_retry_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    let (c, _) = setup(dir.path());
    c.plan_probe(plan(), 1).unwrap();
    assert!(matches!(c.start_probe("p1", Box::new(FailingSource)), Err(ControlError::Capture(_))));
    assert!(!dir.path().join("captures/p1.pcapng").exists());
    let (src, _) = source();
    c.start_probe("p1", src).unwrap();
    assert_eq!(probe(&c).probe_id, "p1");
}

/// Amendment: `running` reflects whether the capture thread is still alive, not merely whether
/// a `JoinHandle` was ever stored — a source that exhausts its frames on its own finishes the
/// thread while the probe itself is still running.
#[test]
fn running_is_false_once_the_capture_thread_finishes_on_its_own() {
    let dir = tempfile::tempdir().unwrap();
    let (c, _) = setup(dir.path());
    c.plan_probe(plan(), 1).unwrap();
    c.start_probe("p1", Box::new(MemorySource::new("memory", Vec::new()))).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while c.state().capture.running {
        assert!(Instant::now() < deadline, "capture thread never finished");
        std::thread::sleep(Duration::from_millis(5));
        c.tick().unwrap();
    }
    assert!(!c.state().capture.running);
    assert_eq!(c.state().probe.unwrap().status, RunStatus::Running);
}

/// Fix round 1: a failure that surfaces after the source has started but before the capture
/// thread is spawned (here, `marks.jsonl` refusing writes) must still stop the source and leave
/// no capture file — not just a source that fails to start outright.
///
/// Unix-only: relies on a non-root user actually being denied a write-mode open on a file with
/// its write bits cleared, which `set_readonly` gives on Unix but not on Windows.
#[cfg(unix)]
#[test]
fn a_failed_marks_append_stops_the_source_and_leaves_no_capture_file() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let (c, _) = setup(dir.path());
    c.plan_probe(plan(), 1).unwrap();
    let marks_path = dir.path().join("marks.jsonl");
    let original_mode = std::fs::metadata(&marks_path).unwrap().permissions().mode();
    std::fs::set_permissions(&marks_path, std::fs::Permissions::from_mode(0o400)).unwrap();

    let result = c.start_probe("p1", source().0);

    std::fs::set_permissions(&marks_path, std::fs::Permissions::from_mode(original_mode)).unwrap();

    assert!(matches!(result, Err(ControlError::Session(SessionError::Io(_)))), "{result:?}");
    assert!(!dir.path().join("captures/p1.pcapng").exists());
    c.start_probe("p1", source().0).unwrap();
    assert_eq!(probe(&c).probe_id, "p1");
}

/// Fix round 2: `a_probe_never_reuses_a_capture_file` (above) checks the returned error when
/// `create_capture` fails after the source has already started; this checks that the same
/// failure leaves no side effects in the marks log — no `ProbeStarted` mark for the id (which
/// would otherwise make `next_probe_id` permanently skip one) and no orphan entry, and that a
/// retry once the obstacle is gone records exactly one `ProbeStarted`.
#[test]
fn a_failed_create_capture_leaves_no_probe_started_mark_or_skipped_id() {
    let dir = tempfile::tempdir().unwrap();
    let (c, _) = setup(dir.path());
    c.plan_probe(plan(), 1).unwrap();
    std::fs::write(dir.path().join("captures/p1.pcapng"), b"old").unwrap();
    assert!(matches!(c.start_probe("p1", source().0), Err(ControlError::Session(SessionError::CaptureExists(_)))));

    let store = SessionStore::open(dir.path()).unwrap();
    assert!(store.marks().unwrap().is_empty(), "no marks should have been written for the failed attempt");

    // Nothing was counted as started, so the next plan mints "p2", not "p3".
    let p2 = c.plan_probe(plan(), 2).unwrap();
    assert_eq!(p2.probe_id, "p2");

    // Once the obstacle is gone, retrying "p1" succeeds and records exactly one ProbeStarted.
    std::fs::remove_file(dir.path().join("captures/p1.pcapng")).unwrap();
    c.start_probe("p1", source().0).unwrap();
    let started = store
        .marks()
        .unwrap()
        .iter()
        .filter(|m| m.probe == "p1" && matches!(m.kind, MarkKind::ProbeStarted { .. }))
        .count();
    assert_eq!(started, 1);
}
