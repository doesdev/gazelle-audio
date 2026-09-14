//! The helper state behind the operation set: at most one open session and its controller.
//! Every operation takes its request as JSON and returns JSON, so MCP, OpenAI tools and
//! `drive` share one dispatch ([`Ops::call`]). Calls may block on session I/O; async callers
//! run them on a blocking thread.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;
use serde_json::{json, Value};

use super::types::*;
use crate::analysis::run::{analyze_probe, field_map_path, load_probe_events, AnalyzeError};
use crate::capture::import::open_frames;
use crate::capture::sources::{build_source, SourceSettings};
use crate::capture::writer::CaptureWriter;
use crate::capture::CaptureError;
use crate::session::clock::{Clock, SystemClock};
use crate::session::controller::{ControlError, Controller, Environment};
use crate::session::marks::{Mark, MarkKind};
use crate::session::step::{RunStatus, StepTiming};
use crate::session::store::{hex, SessionError, SessionInfo, SessionStore};
use crate::session::timeline::probe_timelines;

/// Longest packet excerpt `get_evidence` returns, in bytes.
pub const EXCERPT_BYTES: usize = 64;
/// Most packets `get_evidence` returns.
pub const MAX_EXCERPTS: usize = 32;
/// `await_progress` wait when the request gives none.
pub const DEFAULT_AWAIT_MS: u64 = 30_000;
/// How often `await_progress` looks at the probe state.
const AWAIT_POLL: Duration = Duration::from_millis(50);

#[derive(Debug, thiserror::Error)]
pub enum OpsError {
    #[error("unknown operation {0}")]
    UnknownOperation(String),
    #[error("bad arguments for {operation}: {message}")]
    BadArguments { operation: String, message: String },
    #[error("no session is open; call session_open first")]
    NoSession,
    #[error("{0}")]
    Invalid(String),
    #[error("{0} is not available yet")]
    NotYet(&'static str),
    #[error(transparent)]
    Session(#[from] SessionError),
    #[error(transparent)]
    Control(#[from] ControlError),
    #[error(transparent)]
    Analyze(#[from] AnalyzeError),
    #[error(transparent)]
    Capture(#[from] CaptureError),
    #[error("i/o: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

struct Open {
    root: PathBuf,
    controller: Controller,
}

impl Open {
    fn store(&self) -> Result<SessionStore, OpsError> {
        Ok(SessionStore::open(&self.root)?)
    }
}

#[derive(Clone)]
pub struct Ops {
    open: Arc<Mutex<Option<Open>>>,
    env: Environment,
    clock: Arc<dyn Clock>,
    sources: SourceSettings,
}

fn parse<T: DeserializeOwned>(operation: &str, args: Value) -> Result<T, OpsError> {
    let args = if args.is_null() { json!({}) } else { args };
    serde_json::from_value(args).map_err(|e| OpsError::BadArguments { operation: operation.to_string(), message: e.to_string() })
}

impl Ops {
    pub fn new(env: Environment) -> Self {
        Self::with_clock(env, Arc::new(SystemClock))
    }

    pub fn with_clock(env: Environment, clock: Arc<dyn Clock>) -> Self {
        Self { open: Arc::new(Mutex::new(None)), env, clock, sources: SourceSettings::default() }
    }

    /// Where `start_probe` captures from.
    pub fn with_sources(mut self, sources: SourceSettings) -> Self {
        self.sources = sources;
        self
    }

    fn lock(&self) -> MutexGuard<'_, Option<Open>> {
        self.open.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn with_open<T>(&self, f: impl FnOnce(&Open) -> Result<T, OpsError>) -> Result<T, OpsError> {
        let guard = self.lock();
        f(guard.as_ref().ok_or(OpsError::NoSession)?)
    }

    /// The open session's controller, for the panel and live probes.
    pub fn controller(&self) -> Option<Controller> {
        self.lock().as_ref().map(|o| o.controller.clone())
    }

    /// Runs one operation by name with JSON arguments (`null` means no arguments).
    pub fn call(&self, operation: &str, args: Value) -> Result<Value, OpsError> {
        match operation {
            "session_open" => self.session_open(parse(operation, args)?),
            "session_status" => {
                let _: SessionStatus = parse(operation, args)?;
                self.session_status()
            }
            "import_capture" => self.import_capture(parse(operation, args)?),
            "declare_parameter" => self.declare_parameter(parse(operation, args)?),
            "list_parameters" => {
                let _: ListParameters = parse(operation, args)?;
                self.list_parameters()
            }
            "plan_probe" => self.plan_probe(parse(operation, args)?),
            "start_probe" => self.start_probe(parse(operation, args)?),
            "abandon_probe" => {
                let _: AbandonProbe = parse(operation, args)?;
                self.abandon_probe()
            }
            "await_progress" => self.await_progress(parse(operation, args)?),
            "analyze_probe" => self.analyze_probe(parse(operation, args)?),
            "get_field_map" => self.get_field_map(parse(operation, args)?),
            "get_evidence" => self.get_evidence(parse(operation, args)?),
            other => Err(OpsError::UnknownOperation(other.to_string())),
        }
    }

    fn session_open(&self, req: SessionOpen) -> Result<Value, OpsError> {
        let root = PathBuf::from(&req.path);
        let store = if root.join("session.json").is_file() {
            let store = SessionStore::open(&root)?;
            let info = store.info()?;
            if req.vid.is_some_and(|v| v != info.vid) || req.pid.is_some_and(|p| p != info.pid) {
                return Err(OpsError::Invalid(format!("{} already targets {:04x}:{:04x}", req.path, info.vid, info.pid)));
            }
            store
        } else {
            let (Some(vid), Some(pid)) = (req.vid, req.pid) else {
                return Err(OpsError::Invalid("creating a session needs vid and pid".into()));
            };
            SessionStore::create(&root, &SessionInfo { vid, pid, ..SessionInfo::default() })?
        };
        let controller = Controller::new(store, Arc::clone(&self.clock), StepTiming::default(), self.env)?;
        *self.lock() = Some(Open { root, controller });
        self.session_status()
    }

    fn session_status(&self) -> Result<Value, OpsError> {
        let guard = self.lock();
        let Some(open) = guard.as_ref() else {
            return Ok(json!({ "open": false }));
        };
        let store = open.store()?;
        let info = store.info()?;
        let probes: Vec<Value> = probe_timelines(&store.marks()?)
            .iter()
            .map(|t| json!({ "probe": t.probe, "parameter": t.plan.parameter, "outcome": t.outcome, "steps": t.steps.len(), "closed_steps": t.windows.len() }))
            .collect();
        Ok(json!({
            "open": true,
            "path": open.root.display().to_string(),
            "vid": info.vid,
            "pid": info.pid,
            "state": open.controller.state(),
            "probes": probes,
        }))
    }

    fn declare_parameter(&self, req: DeclareParameter) -> Result<Value, OpsError> {
        self.with_open(|open| {
            let id = req.parameter.id.clone();
            open.controller.declare_parameter(req.parameter)?;
            Ok(json!({ "declared": id }))
        })
    }

    fn list_parameters(&self) -> Result<Value, OpsError> {
        self.with_open(|open| Ok(serde_json::to_value(open.store()?.parameters()?)?))
    }

    fn plan_probe(&self, req: PlanProbe) -> Result<Value, OpsError> {
        self.with_open(|open| {
            let seed = req.seed.unwrap_or_else(|| self.clock.now_ns());
            Ok(serde_json::to_value(open.controller.plan_probe(req.plan, seed)?)?)
        })
    }

    /// Copies the capture into the session as the next probe (pcap becomes pcapng) and appends
    /// that probe's marks under the new id. Marks default to the `marks.jsonl` of the session
    /// the capture came from (`<session>/captures/<probe>.pcapng`).
    fn import_capture(&self, req: ImportCapture) -> Result<Value, OpsError> {
        self.with_open(|open| {
            let source = Path::new(&req.capture);
            let marks_path = match &req.marks {
                Some(path) => PathBuf::from(path),
                None => source
                    .parent()
                    .and_then(Path::parent)
                    .map(|session| session.join("marks.jsonl"))
                    .filter(|p| p.is_file())
                    .ok_or_else(|| OpsError::Invalid(format!("no marks given and none found beside {}", req.capture)))?,
            };
            let text = std::fs::read_to_string(&marks_path)?;
            let marks: Vec<Mark> = text.lines().filter(|l| !l.trim().is_empty()).map(serde_json::from_str).collect::<Result<_, _>>()?;
            let started: Vec<String> = marks.iter().filter(|m| matches!(m.kind, MarkKind::ProbeStarted { .. })).map(|m| m.probe.clone()).collect();
            let stem = source.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
            let source_probe = started.iter().find(|p| p.as_str() == stem).or(started.last()).cloned().ok_or_else(|| OpsError::Invalid(format!("{} holds no probe", marks_path.display())))?;

            let store = open.store()?;
            let probe = store.next_probe_id()?;
            let copied = (|| -> Result<u64, OpsError> {
                let mut writer = CaptureWriter::new(std::io::BufWriter::new(store.create_capture(&probe)?))?;
                let mut frames = 0;
                for frame in open_frames(source)? {
                    writer.write(&frame?)?;
                    frames += 1;
                }
                writer.finish()?;
                Ok(frames)
            })();
            let frames = match copied {
                Ok(n) => n,
                Err(e) => {
                    let _ = std::fs::remove_file(store.capture_path(&probe));
                    return Err(e);
                }
            };
            let rewritten: Vec<Mark> = marks
                .into_iter()
                .filter(|m| m.probe == source_probe)
                .map(|mut m| {
                    m.probe = probe.clone();
                    m
                })
                .collect();
            store.append_marks(&rewritten)?;
            Ok(json!({ "probe": probe, "source_probe": source_probe, "frames": frames, "marks": rewritten.len() }))
        })
    }

    /// Builds the configured capture source (USBPcap hub discovery can take seconds) and starts
    /// the planned probe. The operator then follows the panel; step timers advance only while
    /// the helper's ticker runs.
    fn start_probe(&self, req: StartProbe) -> Result<Value, OpsError> {
        self.with_open(|open| {
            let info = open.store()?.info()?;
            let source = build_source(&self.sources, info.vid, info.pid)?;
            open.controller.start_probe(&req.probe_id, source)?;
            Ok(json!({ "started": req.probe_id, "state": open.controller.state() }))
        })
    }

    fn abandon_probe(&self) -> Result<Value, OpsError> {
        self.with_open(|open| {
            open.controller.abandon_probe()?;
            Ok(json!({ "abandoned": true, "state": open.controller.state() }))
        })
    }

    /// Polls the probe state without holding the session lock. Reasons: `completed`,
    /// `abandoned`, `flagged` (a step became clock-suspect since the call began), `no_probe`, or
    /// `timeout`.
    fn await_progress(&self, req: AwaitProgress) -> Result<Value, OpsError> {
        let controller = self.controller().ok_or(OpsError::NoSession)?;
        let timeout = Duration::from_millis(req.timeout_ms.unwrap_or(DEFAULT_AWAIT_MS));
        let started = Instant::now();
        let flagged_at_start = controller.state().probe.map_or(0, |p| p.flagged_steps.len());
        loop {
            let state = controller.state();
            let reason = match &state.probe {
                None => Some("no_probe"),
                Some(p) if p.status == RunStatus::Completed => Some("completed"),
                Some(p) if p.status == RunStatus::Abandoned => Some("abandoned"),
                Some(p) if p.flagged_steps.len() > flagged_at_start => Some("flagged"),
                Some(_) if started.elapsed() >= timeout => Some("timeout"),
                Some(_) => None,
            };
            if let Some(reason) = reason {
                return Ok(json!({ "reason": reason, "state": state }));
            }
            std::thread::sleep(AWAIT_POLL);
        }
    }

    fn analyze_probe(&self, req: AnalyzeProbe) -> Result<Value, OpsError> {
        self.with_open(|open| {
            let analysis = analyze_probe(&open.store()?, req.probe_id.as_deref())?;
            Ok(json!({ "probe": analysis.probe, "field_map": analysis.map, "report": analysis.report.display().to_string() }))
        })
    }

    fn stored_field_map(open: &Open, parameter: &str) -> Result<Value, OpsError> {
        let path = field_map_path(&open.store()?, parameter);
        if !path.is_file() {
            return Err(OpsError::Invalid(format!("no field map for {parameter}; run analyze_probe first")));
        }
        Ok(serde_json::from_slice(&std::fs::read(path)?)?)
    }

    fn get_field_map(&self, req: GetFieldMap) -> Result<Value, OpsError> {
        self.with_open(|open| Self::stored_field_map(open, &req.parameter))
    }

    fn get_evidence(&self, req: GetEvidence) -> Result<Value, OpsError> {
        self.with_open(|open| {
            let map = Self::stored_field_map(open, &req.parameter)?;
            let evidence = map["evidence"].clone();
            if !req.include_packets {
                return Ok(json!({ "parameter": req.parameter, "evidence": evidence }));
            }
            let store = open.store()?;
            let mut packets = Vec::new();
            for entry in evidence.as_array().into_iter().flatten() {
                let Some(probe) = entry["probe"].as_str() else {
                    continue;
                };
                let wanted: Vec<u64> = entry["packet_indices"].as_array().into_iter().flatten().filter_map(Value::as_u64).collect();
                for ev in load_probe_events(&store, probe)?.iter().filter(|e| wanted.contains(&e.packet_index)) {
                    if packets.len() == MAX_EXCERPTS {
                        break;
                    }
                    packets.push(json!({
                        "probe": probe,
                        "packet_index": ev.packet_index,
                        "ts_ns": ev.ts_ns,
                        "endpoint": ev.endpoint,
                        "direction": ev.direction,
                        "transfer": ev.transfer,
                        "data_len": ev.data_len,
                        "data_hex": hex(&ev.data[..ev.data.len().min(EXCERPT_BYTES)]),
                    }));
                }
            }
            Ok(json!({ "parameter": req.parameter, "evidence": evidence, "packets": packets }))
        })
    }
}

#[cfg(test)]
#[path = "service_tests.rs"]
mod tests;
