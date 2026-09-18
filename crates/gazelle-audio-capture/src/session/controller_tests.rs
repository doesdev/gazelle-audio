use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::*;
use crate::capture::import::{ImportSource, MemorySource};
use crate::capture::{CaptureStream, FrameIter, RawFrame};
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
fn raw_frames() -> (Vec<RawFrame>, u64) {
    let mut devices: Vec<Box<dyn DeviceModel>> = vec![Box::new(SimpleDevice::new(0x1234, 0xABCD, 1, 5)), Box::new(SimpleDevice::new(0x046D, 0xC52B, 1, 3))];
    let frames = device_frames(&mut devices, T0, S);
    let target = frames.iter().filter(|f| u16::from_le_bytes([f.data[19], f.data[20]]) == 5).count() as u64;
    (frames, target)
}

fn source() -> (Box<dyn CaptureSource>, u64) {
    let (frames, target) = raw_frames();
    (Box::new(MemorySource::new("memory", frames)), target)
}

/// A source whose frame iterator repeats `frames` on a loop until `StopHandle::stop()` is
/// called, then ends, unlike `MemorySource`, whose frames run out on their own. Models a live
/// capture for the final review's F2 and F5: what actually happens once a stop signal has to
/// interrupt an otherwise-endless stream, rather than a source that was always going to end.
struct StoppableSource {
    frames: Vec<RawFrame>,
    stopped: Arc<AtomicBool>,
}

impl StoppableSource {
    /// Returns the source and a flag that becomes `true` once its `StopHandle` has been used.
    fn new(frames: Vec<RawFrame>) -> (Self, Arc<AtomicBool>) {
        let stopped = Arc::new(AtomicBool::new(false));
        (Self { frames, stopped: stopped.clone() }, stopped)
    }
}

impl CaptureSource for StoppableSource {
    fn describe(&self) -> String {
        "stoppable".into()
    }

    fn start(&mut self) -> Result<CaptureStream, CaptureError> {
        let for_stop = Arc::clone(&self.stopped);
        let for_iter = Arc::clone(&self.stopped);
        let mut cycle = self.frames.clone().into_iter().cycle();
        let frames: FrameIter = Box::new(std::iter::from_fn(move || -> Option<Result<RawFrame, CaptureError>> {
            if for_iter.load(Ordering::SeqCst) {
                return None;
            }
            // A small pace, like a real live source, so a test that lets "a few frames flow"
            // sees a bounded, sane number of them rather than spinning a tight loop.
            std::thread::sleep(Duration::from_micros(200));
            cycle.next().map(Ok)
        }));
        Ok(CaptureStream { frames, stop: StopHandle::new(move || for_stop.store(true, Ordering::SeqCst)) })
    }
}

/// A source whose capture thread panics as soon as it starts reading frames, for the final
/// review's F3 test.
struct PanickingSource;

impl CaptureSource for PanickingSource {
    fn describe(&self) -> String {
        "panicking".into()
    }

    fn start(&mut self) -> Result<CaptureStream, CaptureError> {
        let frames: FrameIter = Box::new(std::iter::from_fn(|| -> Option<Result<RawFrame, CaptureError>> {
            panic!("synthetic capture thread panic (final review F3 test)")
        }));
        Ok(CaptureStream { frames, stop: StopHandle::noop() })
    }
}

fn setup(dir: &std::path::Path) -> (Controller, Arc<ManualClock>) {
    let store = SessionStore::create(dir, &SessionInfo { vid: 0x1234, pid: 0xABCD, ..SessionInfo::default() }).unwrap();
    for p in parameters() {
        store.declare_parameter(p).unwrap();
    }
    let clock = Arc::new(ManualClock::new(T0));
    let env = Environment { elevated: Some(false), usbpcap_attached: Some(true) };
    (Controller::new(store, clock.clone(), StepTiming::default(), env).unwrap(), clock)
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
    assert_eq!((state.vid, state.pid, state.elevated, state.usbpcap_attached), (0x1234, 0xABCD, Some(false), Some(true)));
    assert_eq!(state.capture.source.as_deref(), Some("memory"));
    let p = probe(&c);
    assert_eq!((p.step_index, p.kind, p.instruction.as_str()), (0, StepKind::Idle, "Do nothing (idle window)"));
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

/// A capture file with no `ProbeStarted` mark is left by a crash mid-start. It is never
/// overwritten: it is moved aside as `pN.orphan-<wall ns>.pcapng`, kept as evidence, and the probe
/// starts (the user's choice, 2026-09-15). A file whose probe did start can't be reached here,
/// since started ids are never planned again, and `create_capture` still refuses to overwrite.
#[test]
fn an_unmarked_leftover_capture_is_moved_aside_and_the_probe_starts() {
    let dir = tempfile::tempdir().unwrap();
    let (c, _) = setup(dir.path());
    c.plan_probe(plan(), 1).unwrap();
    std::fs::write(dir.path().join("captures/p1.pcapng"), b"old").unwrap();
    c.start_probe("p1", source().0).unwrap();
    assert_eq!(probe(&c).probe_id, "p1");

    let aside: Vec<_> = std::fs::read_dir(dir.path().join("captures"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.file_name().unwrap().to_string_lossy().starts_with("p1.orphan-"))
        .collect();
    assert_eq!(aside.len(), 1, "one leftover moved aside: {aside:?}");
    assert_eq!(aside[0].file_name().unwrap().to_string_lossy(), format!("p1.orphan-{T0}.pcapng"));
    assert_eq!(std::fs::read(&aside[0]).unwrap(), b"old", "the leftover is kept as it was");
    assert_ne!(std::fs::read(dir.path().join("captures/p1.pcapng")).unwrap(), b"old", "the new capture is a fresh file");
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
/// a `JoinHandle` was ever stored: a source that exhausts its frames on its own finishes the
/// thread while the probe itself is still running.
/// A live source that stays running but never delivers a frame: the target's traffic is not
/// reaching the capture (for example USBPcap is not in its driver stack).
struct SilentSource;

impl CaptureSource for SilentSource {
    fn describe(&self) -> String {
        "silent".into()
    }

    fn start(&mut self) -> Result<CaptureStream, CaptureError> {
        let stopped = Arc::new(AtomicBool::new(false));
        let for_iter = Arc::clone(&stopped);
        let frames: FrameIter = Box::new(std::iter::from_fn(move || -> Option<Result<RawFrame, CaptureError>> {
            while !for_iter.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(2));
            }
            None
        }));
        Ok(CaptureStream { frames, stop: StopHandle::new(move || stopped.store(true, Ordering::SeqCst)) })
    }
}

#[test]
fn silence_is_measured_from_capture_start_and_resets_on_packets() {
    let dir = tempfile::tempdir().unwrap();
    let (c, clock) = setup(dir.path());
    c.plan_probe(plan(), 1).unwrap();
    c.start_probe("p1", Box::new(SilentSource)).unwrap();
    assert_eq!(c.state().capture.silent_ms, 0);
    clock.advance(4 * S);
    c.tick().unwrap();
    let capture = c.state().capture;
    assert!(capture.running);
    assert_eq!((capture.packets, capture.silent_ms), (0, 4_000));
    c.abandon_probe().unwrap();
    assert_eq!(c.state().capture.silent_ms, 0, "not running, so not silent");

    // With packets flowing, silence counts from the last one.
    let dir = tempfile::tempdir().unwrap();
    let (c, clock) = setup(dir.path());
    c.plan_probe(plan(), 1).unwrap();
    let (frames, _) = raw_frames();
    let (src, _) = StoppableSource::new(frames);
    c.start_probe("p1", Box::new(src)).unwrap();
    wait_packets(&c, 1);
    clock.advance(4 * S);
    c.tick().unwrap();
    let before = c.state().capture.packets;
    wait_packets(&c, before + 1);
    assert!(c.state().capture.silent_ms < 4_000, "a packet after the advance resets the silence");
    c.abandon_probe().unwrap();
}

#[test]
fn a_running_capture_is_flushed_to_a_readable_file() {
    let dir = tempfile::tempdir().unwrap();
    let (c, clock) = setup(dir.path());
    c.plan_probe(plan(), 1).unwrap();
    let (frames, _) = raw_frames();
    let (src, _) = StoppableSource::new(frames);
    c.start_probe("p1", Box::new(src)).unwrap();
    wait_packets(&c, 1);
    // Past the flush interval, the next stored packet flushes everything before it.
    clock.advance(2 * S);
    let before = c.state().capture.packets;
    wait_packets(&c, before + 2);
    assert!(c.state().capture.running, "still capturing: no finish() has run");
    let path = dir.path().join("captures").join("p1.pcapng");
    // Writing continues after the flush, and BufWriter may already have spilled part of a later
    // block to disk, so the file can end in a truncated block; count the whole ones before it.
    let readable = crate::capture::import::open_frames(&path).unwrap().take_while(Result::is_ok).count() as u64;
    assert!(readable >= before, "{readable} frames readable on disk, expected at least {before}");
    c.abandon_probe().unwrap();
}

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
/// no capture file, not just a source that fails to start outright.
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
/// failure leaves no side effects in the marks log: no `ProbeStarted` mark for the id (which
/// would otherwise make `next_probe_id` permanently skip one) and no orphan entry, and that a
/// retry once the obstacle is gone records exactly one `ProbeStarted`.
#[test]
fn a_failed_create_capture_leaves_no_probe_started_mark_or_skipped_id() {
    let dir = tempfile::tempdir().unwrap();
    let (c, _) = setup(dir.path());
    c.plan_probe(plan(), 1).unwrap();
    // An obstacle `create_capture` can't get past: `captures` is a file, not a directory.
    std::fs::remove_dir_all(dir.path().join("captures")).unwrap();
    std::fs::write(dir.path().join("captures"), b"not a directory").unwrap();
    assert!(matches!(c.start_probe("p1", source().0), Err(ControlError::Session(_))));

    let store = SessionStore::open(dir.path()).unwrap();
    assert!(store.marks().unwrap().is_empty(), "no marks should have been written for the failed attempt");

    // Nothing was counted as started, so the next plan mints "p2", not "p3".
    let p2 = c.plan_probe(plan(), 2).unwrap();
    assert_eq!(p2.probe_id, "p2");

    // Once the obstacle is gone, retrying "p1" succeeds and records exactly one ProbeStarted.
    std::fs::remove_file(dir.path().join("captures")).unwrap();
    std::fs::create_dir(dir.path().join("captures")).unwrap();
    c.start_probe("p1", source().0).unwrap();
    let started = store
        .marks()
        .unwrap()
        .iter()
        .filter(|m| m.probe == "p1" && matches!(m.kind, MarkKind::ProbeStarted { .. }))
        .count();
    assert_eq!(started, 1);
}

/// Final review F2: `abandon`'s marks already move the run's in-memory status to `Abandoned`
/// before `append_marks` is ever called, so a failure appending the very last mark (full disk, an
/// AV lock, ...) must not skip `finish_capture`. The source still needs stopping and the thread
/// still needs joining (so the pcapng gets `writer.finish()`ed and, on Windows, the underlying
/// process actually stops), and the panel still needs to see the resulting state. Before the fix,
/// the early `?` on `append_marks` returned before any of that ran.
///
/// Unix-only: relies on a non-root user actually being denied a write-mode open on a file with
/// its write bits cleared, matching the existing `a_failed_marks_append_...` test.
#[cfg(unix)]
#[test]
fn a_failed_final_marks_append_still_finishes_the_capture() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let (c, _) = setup(dir.path());
    c.plan_probe(plan(), 1).unwrap();
    let (frames, target) = raw_frames();
    let (src, stopped) = StoppableSource::new(frames);
    c.start_probe("p1", Box::new(src)).unwrap();
    wait_packets(&c, target);

    let marks_path = dir.path().join("marks.jsonl");
    let original_mode = std::fs::metadata(&marks_path).unwrap().permissions().mode();
    std::fs::set_permissions(&marks_path, std::fs::Permissions::from_mode(0o400)).unwrap();

    let result = c.abandon_probe();

    std::fs::set_permissions(&marks_path, std::fs::Permissions::from_mode(original_mode)).unwrap();

    assert!(matches!(result, Err(ControlError::Session(SessionError::Io(_)))), "{result:?}");
    assert!(stopped.load(Ordering::SeqCst), "the source's stop handle was never called");
    assert!(!c.state().capture.running, "the capture thread was not joined");
    assert!(c.state().probe.unwrap().status == RunStatus::Abandoned);
    let stored = ImportSource::new(dir.path().join("captures/p1.pcapng")).start().unwrap().frames.count() as u64;
    assert!(stored >= target, "expected at least {target} imported frames, got {stored}");
}

/// Final review F3: `finish_capture`'s `let _ = thread.join()` swallowed a capture-thread panic
/// outright, so the panel would show a clean stop with no `failure` even though nothing after the
/// panic point ran (including `writer.finish()`). A join `Err` must now surface as
/// `stats.failure`.
#[test]
fn a_panicking_capture_thread_is_reported_as_a_failure() {
    let dir = tempfile::tempdir().unwrap();
    let (c, _) = setup(dir.path());
    c.plan_probe(plan(), 1).unwrap();
    c.start_probe("p1", Box::new(PanickingSource)).unwrap();
    // Give the thread a moment to actually panic before abandoning it.
    std::thread::sleep(Duration::from_millis(20));
    c.abandon_probe().unwrap();
    assert_eq!(c.state().capture.failure.as_deref(), Some("capture thread panicked"));
}

/// Final review F5: the other controller tests all use sources whose frames run out on their
/// own; none covers a source that has to be *stopped* mid-stream. This starts a probe, lets a
/// few frames flow (bounded polling of stats, not a bare sleep), abandons it, and checks the
/// thread was joined, no failure was recorded, and the pcapng imports the frames that were
/// written before the stop.
#[test]
fn abandon_stops_a_live_source_cleanly_and_finalises_an_importable_capture() {
    let dir = tempfile::tempdir().unwrap();
    let (c, _) = setup(dir.path());
    c.plan_probe(plan(), 1).unwrap();
    let (frames, _target) = raw_frames();
    let (src, stopped) = StoppableSource::new(frames);
    c.start_probe("p1", Box::new(src)).unwrap();

    // Let a few frames flow: bounded polling of stats, not a bare sleep as sync.
    let deadline = Instant::now() + Duration::from_secs(5);
    while c.state().capture.packets < 3 {
        assert!(Instant::now() < deadline, "capture thread stalled before any frames flowed");
        std::thread::sleep(Duration::from_millis(5));
        c.tick().unwrap();
    }
    let packets_before_stop = c.state().capture.packets;

    c.abandon_probe().unwrap();

    assert!(stopped.load(Ordering::SeqCst), "the source's stop handle was never called");
    assert!(!c.state().capture.running, "the capture thread was not joined");
    assert_eq!(c.state().capture.failure, None);
    assert_eq!(c.state().probe.unwrap().status, RunStatus::Abandoned);
    let stored = ImportSource::new(dir.path().join("captures/p1.pcapng")).start().unwrap().frames.count() as u64;
    assert!(stored >= packets_before_stop, "expected at least {packets_before_stop} imported frames, got {stored}");
}
