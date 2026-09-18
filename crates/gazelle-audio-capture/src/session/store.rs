//! The session directory: `session.json`, `parameters.json`, `captures/`, `marks.jsonl`,
//! `analysis/`.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::marks::{Mark, MarkKind};
use super::model::{Parameter, PlanError};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionInfo {
    pub vid: u16,
    pub pid: u16,
    /// Hex of the 18-byte device descriptor, once captured.
    #[serde(default)]
    pub device_descriptor_hex: Option<String>,
    #[serde(default)]
    pub vendor_app: Option<String>,
    #[serde(default)]
    pub vendor_app_version: Option<String>,
    #[serde(default)]
    pub notes: String,
    /// Keep isochronous and bulk payload bytes (for parameters suspected to travel in-band).
    #[serde(default)]
    pub keep_stream_payloads: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("i/o: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{0} already holds a session")]
    Exists(PathBuf),
    #[error("{0} is not a session directory")]
    NotASession(PathBuf),
    #[error(transparent)]
    Plan(#[from] PlanError),
    #[error("probe {0} already has a capture")]
    CaptureExists(String),
}

pub struct SessionStore {
    root: PathBuf,
}

/// Lowercase hex without separators.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

impl SessionStore {
    pub fn create(root: impl Into<PathBuf>, info: &SessionInfo) -> Result<Self, SessionError> {
        let root = root.into();
        if root.join("session.json").exists() {
            return Err(SessionError::Exists(root));
        }
        fs::create_dir_all(root.join("captures"))?;
        fs::create_dir_all(root.join("analysis"))?;
        let store = Self { root };
        store.write_json("session.json", info)?;
        store.write_json("parameters.json", &Vec::<Parameter>::new())?;
        File::create(store.root.join("marks.jsonl"))?;
        Ok(store)
    }

    pub fn open(root: impl Into<PathBuf>) -> Result<Self, SessionError> {
        let root = root.into();
        if !root.join("session.json").is_file() {
            return Err(SessionError::NotASession(root));
        }
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn info(&self) -> Result<SessionInfo, SessionError> {
        Ok(serde_json::from_slice(&fs::read(self.root.join("session.json"))?)?)
    }

    pub fn update_info(&self, change: impl FnOnce(&mut SessionInfo)) -> Result<SessionInfo, SessionError> {
        let mut info = self.info()?;
        change(&mut info);
        self.write_json("session.json", &info)?;
        Ok(info)
    }

    pub fn parameters(&self) -> Result<Vec<Parameter>, SessionError> {
        Ok(serde_json::from_slice(&fs::read(self.root.join("parameters.json"))?)?)
    }

    pub fn declare_parameter(&self, parameter: Parameter) -> Result<(), SessionError> {
        parameter.validate()?;
        let mut all = self.parameters()?;
        if all.iter().any(|p| p.id == parameter.id) {
            return Err(PlanError::DuplicateParameter(parameter.id).into());
        }
        all.push(parameter);
        self.write_json("parameters.json", &all)
    }

    /// Appends marks as JSON lines and syncs, so a crash loses at most the marks in flight. A
    /// torn final line left by an earlier crash (no trailing newline) is cut off first, so the
    /// new marks start on a line of their own.
    pub fn append_marks(&self, marks: &[Mark]) -> Result<(), SessionError> {
        if marks.is_empty() {
            return Ok(());
        }
        let mut buf = Vec::new();
        for m in marks {
            serde_json::to_writer(&mut buf, m)?;
            buf.push(b'\n');
        }
        let path = self.root.join("marks.jsonl");
        let existing = fs::read(&path)?;
        if existing.last().is_some_and(|b| *b != b'\n') {
            let keep = existing.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
            tracing::warn!(path = %path.display(), dropped_bytes = existing.len() - keep, "cutting a torn final line from marks.jsonl");
            OpenOptions::new().write(true).open(&path)?.set_len(keep as u64)?;
        }
        let mut f = OpenOptions::new().append(true).open(&path)?;
        f.write_all(&buf)?;
        f.sync_data()?;
        Ok(())
    }

    /// Every mark in order. A final line that is unterminated and does not parse (a write
    /// torn by a crash) is skipped with a warning; any other bad line is an error.
    pub fn marks(&self) -> Result<Vec<Mark>, SessionError> {
        let path = self.root.join("marks.jsonl");
        let text = fs::read_to_string(&path)?;
        let torn_tail = !text.is_empty() && !text.ends_with('\n');
        let lines: Vec<&str> = text.lines().collect();
        let mut out = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str(line) {
                Ok(mark) => out.push(mark),
                Err(_) if torn_tail && i + 1 == lines.len() => {
                    tracing::warn!(path = %path.display(), "ignoring a torn final line in marks.jsonl");
                }
                Err(e) => return Err(e.into()),
            }
        }
        Ok(out)
    }

    /// `p1`, `p2`, and so on: one more than the probes already started.
    pub fn next_probe_id(&self) -> Result<String, SessionError> {
        let started = self.marks()?.iter().filter(|m| matches!(m.kind, MarkKind::ProbeStarted { .. })).count();
        Ok(format!("p{}", started + 1))
    }

    pub fn capture_path(&self, probe: &str) -> PathBuf {
        self.root.join("captures").join(format!("{probe}.pcapng"))
    }

    /// Creates the probe's capture file; one capture per probe, never overwritten.
    pub fn create_capture(&self, probe: &str) -> Result<File, SessionError> {
        match OpenOptions::new().write(true).create_new(true).open(self.capture_path(probe)) {
            Ok(f) => Ok(f),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Err(SessionError::CaptureExists(probe.into())),
            Err(e) => Err(e.into()),
        }
    }

    /// Write to a temporary sibling and rename, so readers never see a partial file.
    fn write_json<T: Serialize>(&self, name: &str, value: &T) -> Result<(), SessionError> {
        let tmp = self.root.join(format!(".{name}.tmp"));
        fs::write(&tmp, serde_json::to_vec_pretty(value)?)?;
        fs::rename(&tmp, self.root.join(name))?;
        Ok(())
    }
}
