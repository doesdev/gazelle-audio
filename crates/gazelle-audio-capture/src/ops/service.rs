//! The helper state behind the operation set: at most one open session and its controller.
//! Every operation takes its request as JSON and returns JSON, so MCP, OpenAI tools and
//! `drive` share one dispatch ([`Ops::call`]). Calls may block on session I/O; async callers
//! run them on a blocking thread.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use serde::de::DeserializeOwned;
use serde_json::{json, Value};

use super::types::*;
use crate::analysis::run::{analyze_probe, field_map_path, load_probe_events, AnalyzeError};
use crate::capture::import::open_frames;
use crate::capture::writer::CaptureWriter;
use crate::capture::CaptureError;
use crate::session::clock::{Clock, SystemClock};
use crate::session::controller::{ControlError, Controller, Environment};
use crate::session::marks::{Mark, MarkKind};
use crate::session::step::StepTiming;
use crate::session::store::{hex, SessionError, SessionInfo, SessionStore};
use crate::session::timeline::probe_timelines;

/// Longest packet excerpt `get_evidence` returns, in bytes.
pub const EXCERPT_BYTES: usize = 64;
/// Most packets `get_evidence` returns.
pub const MAX_EXCERPTS: usize = 32;

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
        Self { open: Arc::new(Mutex::new(None)), env, clock }
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
            "start_probe" => Err(OpsError::NotYet("start_probe")),
            "abandon_probe" => Err(OpsError::NotYet("abandon_probe")),
            "await_progress" => Err(OpsError::NotYet("await_progress")),
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
