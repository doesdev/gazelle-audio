//! Owns the session, the running probe and its capture; publishes one shared state that the
//! panel and (sub-project 3) the agent adapters both watch.

use std::collections::HashMap;
use std::io::BufWriter;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;

use serde::Serialize;
use tokio::sync::watch;

use super::authority::OperatorAuthority;
use super::clock::Clock;
use super::marks::{Mark, PacketClock};
use super::model::{Parameter, PlanError, ProbePlan};
use super::plan::{expand, StepKind, StepSpec};
use super::step::{OperatorCommand, ProbeRun, RunStatus, StepError, StepState, StepTiming};
use super::store::{hex, SessionError, SessionStore};
use crate::capture::pipeline::{DeviceFilter, PayloadPolicy, Pipeline};
use crate::capture::rate::RateMeter;
use crate::capture::writer::CaptureWriter;
use crate::capture::{CaptureError, CaptureSource, StopHandle};

#[derive(Debug, thiserror::Error)]
pub enum ControlError {
    #[error(transparent)]
    Session(#[from] SessionError),
    #[error(transparent)]
    Plan(#[from] PlanError),
    #[error(transparent)]
    Step(#[from] StepError),
    #[error(transparent)]
    Capture(#[from] CaptureError),
    #[error("a probe is already running")]
    ProbeRunning,
    #[error("no planned probe {0}")]
    UnknownProbe(String),
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlannedProbe {
    pub probe_id: String,
    pub seed: u64,
    pub steps: Vec<StepSpec>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct CaptureView {
    pub running: bool,
    pub source: Option<String>,
    pub packets: u64,
    pub packets_per_second: f64,
    pub decode_errors: u64,
    pub failure: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProbeView {
    pub probe_id: String,
    pub status: RunStatus,
    pub step_index: usize,
    pub step_count: usize,
    pub progress: String,
    pub kind: StepKind,
    /// Instruction with `**bold**` spans, e.g. `Set **Monitor level** from **0 dB** to **-6 dB**, then press Done`.
    pub instruction: String,
    /// `armed` or `done` while running.
    pub step_state: String,
    pub ready_in_ms: u64,
    pub attempt: u32,
    pub actual_value: Option<String>,
    pub accepts_actual_value: bool,
    pub clock_suspect: bool,
    /// Steps flagged `clock_suspect` so far.
    pub flagged_steps: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PanelState {
    pub seq: u64,
    pub vid: u16,
    pub pid: u16,
    /// `None` where elevation cannot be determined (non-Windows).
    pub elevated: Option<bool>,
    pub capture: CaptureView,
    pub probe: Option<ProbeView>,
}

/// Instruction text for a step, using parameter labels.
pub fn instruction(spec: &StepSpec, parameters: &[Parameter]) -> String {
    let label = |id: &Option<String>| {
        let id = id.as_deref().unwrap_or_default();
        parameters.iter().find(|p| p.id == id).map_or(id.to_string(), |p| p.label.clone())
    };
    let to = spec.to.as_deref().unwrap_or_default();
    match (spec.kind, spec.from.as_deref()) {
        (StepKind::Idle, _) => "Do nothing — idle window".to_string(),
        (StepKind::Set, Some(from)) => {
            format!("Set **{}** from **{from}** to **{to}**, then press Done", label(&spec.parameter))
        }
        (StepKind::Set, None) => format!("Set **{}** to **{to}**, then press Done", label(&spec.parameter)),
        (StepKind::NoOp, _) => {
            format!("Touch **{}** and leave it at **{to}**, then press Done", label(&spec.parameter))
        }
        (StepKind::Control, _) => {
            format!("Change **{}** to any other value, then press Done", label(&spec.parameter))
        }
    }
}

struct CaptureStats {
    packets: u64,
    decode_errors: u64,
    last_packet: Option<PacketClock>,
    rate: RateMeter,
    failure: Option<String>,
    descriptor: Option<Vec<u8>>,
}

struct Active {
    run: ProbeRun,
    source: String,
    stats: Arc<Mutex<CaptureStats>>,
    stop: Option<StopHandle>,
    thread: Option<JoinHandle<()>>,
}

struct Inner {
    store: SessionStore,
    parameters: Vec<Parameter>,
    clock: Arc<dyn Clock>,
    timing: StepTiming,
    elevated: Option<bool>,
    seq: u64,
    planned: HashMap<String, (ProbePlan, u64)>,
    active: Option<Active>,
}

#[derive(Clone)]
pub struct Controller {
    inner: Arc<Mutex<Inner>>,
    tx: Arc<watch::Sender<PanelState>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Controller {
    pub fn new(store: SessionStore, clock: Arc<dyn Clock>, timing: StepTiming, elevated: Option<bool>) -> Result<Self, ControlError> {
        let parameters = store.parameters()?;
        let mut inner = Inner { store, parameters, clock, timing, elevated, seq: 0, planned: HashMap::new(), active: None };
        let state = snapshot(&mut inner)?;
        Ok(Self { inner: Arc::new(Mutex::new(inner)), tx: Arc::new(watch::channel(state).0) })
    }

    pub fn subscribe(&self) -> watch::Receiver<PanelState> {
        self.tx.subscribe()
    }

    pub fn state(&self) -> PanelState {
        self.tx.borrow().clone()
    }

    pub fn declare_parameter(&self, parameter: Parameter) -> Result<(), ControlError> {
        let mut inner = lock(&self.inner);
        inner.store.declare_parameter(parameter)?;
        inner.parameters = inner.store.parameters()?;
        Ok(())
    }

    /// Validates and expands a plan; the probe id is reserved until started.
    pub fn plan_probe(&self, plan: ProbePlan, seed: u64) -> Result<PlannedProbe, ControlError> {
        let mut inner = lock(&self.inner);
        plan.validate(&inner.parameters)?;
        let started: usize = inner.store.next_probe_id()?[1..].parse::<usize>().unwrap_or(1) - 1;
        let probe_id = format!("p{}", started + inner.planned.len() + 1);
        let steps = expand(&plan, seed);
        inner.planned.insert(probe_id.clone(), (plan, seed));
        Ok(PlannedProbe { probe_id, seed, steps })
    }

    /// Starts the capture, then arms the first step.
    ///
    /// Every fallible step that would otherwise need to be undone — starting the source,
    /// creating the capture file, building the pcapng writer around it, and recording the
    /// `ProbeStarted`/`Armed` marks — happens before the capture thread is spawned, in that
    /// order, so the marks append is the *last* thing that can fail before the thread exists.
    /// On any failure up to and including the marks append, the source is stopped and any file
    /// this call created is removed (`create_capture` uses `create_new`, so removing it here is
    /// always safe — the file did not exist before this call), so a failed call leaves neither a
    /// capture file nor a `ProbeStarted` mark behind, and a retry with the same probe id sees a
    /// clean slate: no skipped id (`next_probe_id` counts `ProbeStarted` marks), no orphan entry
    /// in `probe_timelines`. `ProbeRun::start` is called with `last_packet: None`, which is
    /// always correct here: no packets can have been read before the thread that reads them
    /// exists.
    ///
    /// After the thread spawns, `self.publish` (below) can still fail — it re-reads
    /// `session.json` via `store.info()` — but that is not a leak: `inner.active` is set
    /// immediately before `publish` runs, so the `Controller` already owns and tracks the
    /// running thread and the source's stop handle even if this call returns `Err`; a later
    /// `abandon_probe`/`operator`/`tick` can still reach and clean them up via `finish_capture`.
    pub fn start_probe(&self, probe_id: &str, mut source: Box<dyn CaptureSource>) -> Result<(), ControlError> {
        let mut inner = lock(&self.inner);
        if inner.active.as_ref().is_some_and(|a| a.run.status() == RunStatus::Running) {
            return Err(ControlError::ProbeRunning);
        }
        let (plan, seed) = inner.planned.get(probe_id).cloned().ok_or_else(|| ControlError::UnknownProbe(probe_id.into()))?;
        let info = inner.store.info()?;
        let stream = source.start()?;
        let file = match inner.store.create_capture(probe_id) {
            Ok(f) => f,
            Err(e) => {
                stream.stop.stop();
                return Err(e.into());
            }
        };
        let writer = match CaptureWriter::new(BufWriter::new(file)) {
            Ok(w) => w,
            Err(e) => {
                stream.stop.stop();
                let _ = std::fs::remove_file(inner.store.capture_path(probe_id));
                return Err(e.into());
            }
        };
        let stats = Arc::new(Mutex::new(CaptureStats {
            packets: 0,
            decode_errors: 0,
            last_packet: None,
            rate: RateMeter::new(1_000_000_000),
            failure: None,
            descriptor: None,
        }));
        let now = inner.clock.now_ns();
        let (run, marks) = ProbeRun::start(probe_id, &plan, seed, inner.timing, now, None);
        if let Err(e) = inner.store.append_marks(&marks) {
            stream.stop.stop();
            drop(writer);
            let _ = std::fs::remove_file(inner.store.capture_path(probe_id));
            return Err(e.into());
        }
        let thread = {
            let stats = Arc::clone(&stats);
            let clock = Arc::clone(&inner.clock);
            let mut pipeline = Pipeline::new(
                DeviceFilter::Target { vid: info.vid, pid: info.pid },
                PayloadPolicy { keep_stream_payloads: info.keep_stream_payloads },
            );
            let mut writer = writer;
            let (vid, pid) = (info.vid, info.pid);
            std::thread::spawn(move || {
                let result = (|| -> Result<(), CaptureError> {
                    for frame in stream.frames {
                        let frame = frame?;
                        let host_ns = clock.now_ns();
                        let stored = match pipeline.process(frame) {
                            Ok(stored) => stored,
                            Err(_) => {
                                lock(&stats).decode_errors += 1;
                                continue;
                            }
                        };
                        for (frame, ev) in stored {
                            writer.write(&frame)?;
                            let mut s = lock(&stats);
                            s.packets += 1;
                            s.rate.record(host_ns);
                            s.last_packet = Some(PacketClock { ts_ns: ev.ts_ns, host_ns });
                            if s.descriptor.is_none() {
                                if let Some(addr) = pipeline.device_map().find(vid, pid) {
                                    s.descriptor = pipeline.device_map().get(addr.0, addr.1).map(|d| d.descriptor.clone());
                                }
                            }
                        }
                    }
                    writer.finish()?;
                    Ok(())
                })();
                if let Err(e) = result {
                    lock(&stats).failure = Some(e.to_string());
                }
            })
        };
        inner.planned.remove(probe_id);
        inner.active = Some(Active { run, source: source.describe(), stats, stop: Some(stream.stop), thread: Some(thread) });
        self.publish(&mut inner)
    }

    pub fn abandon_probe(&self) -> Result<(), ControlError> {
        let mut inner = lock(&self.inner);
        let now = inner.clock.now_ns();
        let active = inner.active.as_mut().ok_or(StepError::NotRunning)?;
        let clock = lock(&active.stats).last_packet;
        let marks = active.run.abandon(now, clock)?;
        // `abandon` above only returns `Ok` after already setting the run's in-memory status to
        // `Abandoned`, so the run is unconditionally no longer `Running` here.
        self.conclude(&mut inner, &marks, true)
    }

    /// Operator commands. Requires the proof only the panel holds.
    pub fn operator(&self, authority: &OperatorAuthority, command: OperatorCommand) -> Result<(), ControlError> {
        let mut inner = lock(&self.inner);
        let now = inner.clock.now_ns();
        let active = inner.active.as_mut().ok_or(StepError::NotRunning)?;
        let clock = lock(&active.stats).last_packet;
        let marks = active.run.operator(authority, command, now, clock)?;
        let finished = active.run.status() != RunStatus::Running;
        self.conclude(&mut inner, &marks, finished)
    }

    /// Advances step timers and refreshes the packet rate. Call every ~100 ms.
    pub fn tick(&self) -> Result<(), ControlError> {
        let mut inner = lock(&self.inner);
        let now = inner.clock.now_ns();
        let Some(active) = inner.active.as_mut() else {
            return self.publish(&mut inner);
        };
        if active.run.status() != RunStatus::Running {
            return self.publish(&mut inner);
        }
        let clock = lock(&active.stats).last_packet;
        let marks = active.run.tick(now, clock);
        let finished = active.run.status() != RunStatus::Running;
        self.conclude(&mut inner, &marks, finished)
    }

    /// Final review F2: once a run has left `Running` (`finished`), `finish_capture` — stopping
    /// the source and joining the capture thread — must run regardless of whether `marks` (which
    /// describe that transition) actually made it to disk: a full disk or an AV lock on
    /// `marks.jsonl` must not leave the source running and the thread unjoined (on Windows, the
    /// underlying process still running) just because the very last append failed. So: try the
    /// append, finish the capture if the run is done, publish the resulting state either way, and
    /// only then surface whichever error happened — `finish_capture`'s takes priority, since it
    /// reflects the capture itself possibly still not being torn down, which matters more than a
    /// mark that failed to log an already-in-memory transition.
    fn conclude(&self, inner: &mut Inner, marks: &[Mark], finished: bool) -> Result<(), ControlError> {
        let appended = inner.store.append_marks(marks);
        let finished_result = if finished { finish_capture(inner) } else { Ok(()) };
        let published = self.publish(inner);
        finished_result?;
        appended?;
        published
    }

    fn publish(&self, inner: &mut Inner) -> Result<(), ControlError> {
        let state = snapshot(inner)?;
        self.tx.send_replace(state);
        Ok(())
    }
}

/// Stops the source, waits for the writer to flush, records the device descriptor.
fn finish_capture(inner: &mut Inner) -> Result<(), ControlError> {
    let Some(active) = inner.active.as_mut() else {
        return Ok(());
    };
    if let Some(stop) = active.stop.take() {
        stop.stop();
    }
    if let Some(thread) = active.thread.take() {
        // Final review F3: a join `Err` means the capture thread panicked — without this, the
        // panel would show a clean stop (no `failure`) even though nothing after the panic point
        // ran, including `writer.finish()`.
        if thread.join().is_err() {
            lock(&active.stats).failure = Some("capture thread panicked".to_string());
        }
    }
    let descriptor = lock(&active.stats).descriptor.clone();
    if let Some(d) = descriptor {
        inner.store.update_info(|i| i.device_descriptor_hex = Some(hex(&d)))?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "controller_tests.rs"]
mod tests;

fn snapshot(inner: &mut Inner) -> Result<PanelState, ControlError> {
    inner.seq += 1;
    let info = inner.store.info()?;
    let now = inner.clock.now_ns();
    let mut capture = CaptureView::default();
    let mut probe = None;
    if let Some(active) = inner.active.as_ref() {
        let mut s = lock(&active.stats);
        capture = CaptureView {
            running: active.thread.as_ref().is_some_and(|t| !t.is_finished()),
            source: Some(active.source.clone()),
            packets: s.packets,
            packets_per_second: s.rate.per_second(now),
            decode_errors: s.decode_errors,
            failure: s.failure.clone(),
        };
        drop(s);
        let run = &active.run;
        let steps = run.steps();
        let flagged_steps = steps.iter().filter(|s| s.clock_suspect).map(|s| s.spec.index).collect();
        let shown = run.current().unwrap_or_else(|| steps.last().expect("a plan has steps"));
        probe = Some(ProbeView {
            probe_id: run.probe_id().to_string(),
            status: run.status(),
            step_index: shown.spec.index,
            step_count: steps.len(),
            progress: shown.spec.progress(),
            kind: shown.spec.kind,
            instruction: instruction(&shown.spec, &inner.parameters),
            step_state: match shown.state {
                StepState::Armed { .. } => "armed",
                StepState::Done { .. } => "done",
                StepState::Closed { .. } => "closed",
                StepState::Skipped { .. } => "skipped",
                StepState::Pending => "pending",
            }
            .to_string(),
            ready_in_ms: run.ready_in_ns(now).div_ceil(1_000_000),
            attempt: shown.attempt,
            actual_value: shown.actual_value.clone(),
            accepts_actual_value: matches!(shown.spec.kind, StepKind::Set | StepKind::NoOp),
            clock_suspect: shown.clock_suspect,
            flagged_steps,
        });
    }
    Ok(PanelState { seq: inner.seq, vid: info.vid, pid: info.pid, elevated: inner.elevated, capture, probe })
}
