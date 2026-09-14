//! Analysis of a stored probe with the session I/O the pure pipeline leaves out: load marks and
//! the capture, build the field map, write `analysis/<parameter>.json` and `.md`. Shared by
//! `gazelle-capture analyze` and the `analyze_probe` operation.

use std::path::PathBuf;

use super::fieldmap::{field_map, report, FieldMap, ProbeInput};
use crate::capture::event::UsbEvent;
use crate::capture::import::ImportSource;
use crate::capture::pipeline::{DeviceFilter, PayloadPolicy, Pipeline};
use crate::capture::{CaptureError, CaptureSource};
use crate::session::store::{SessionError, SessionStore};
use crate::session::timeline::probe_timelines;

#[derive(Debug, thiserror::Error)]
pub enum AnalyzeError {
    #[error(transparent)]
    Session(#[from] SessionError),
    #[error(transparent)]
    Capture(#[from] CaptureError),
    #[error("i/o: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("no probe {0} in the session")]
    UnknownProbe(String),
    #[error("the session has no probes")]
    NoProbes,
}

#[derive(Debug, Clone)]
pub struct Analysis {
    pub probe: String,
    pub map: FieldMap,
    pub json: PathBuf,
    pub report: PathBuf,
}

/// The target's events from a probe's capture, sorted by time, with `packet_index` as stored.
pub fn load_probe_events(store: &SessionStore, probe: &str) -> Result<Vec<UsbEvent>, AnalyzeError> {
    let info = store.info()?;
    let mut pipeline = Pipeline::new(DeviceFilter::Target { vid: info.vid, pid: info.pid }, PayloadPolicy { keep_stream_payloads: info.keep_stream_payloads });
    let mut events = Vec::new();
    for frame in ImportSource::new(store.capture_path(probe)).start()?.frames {
        events.extend(pipeline.process(frame?).map_err(CaptureError::from)?.into_iter().map(|(_, ev)| ev));
    }
    events.sort_by_key(|e| e.ts_ns);
    Ok(events)
}

/// Parameter ids name files; keep them to a portable character set.
pub fn file_stem(parameter: &str) -> String {
    parameter.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect()
}

pub fn field_map_path(store: &SessionStore, parameter: &str) -> PathBuf {
    store.root().join("analysis").join(format!("{}.json", file_stem(parameter)))
}

/// Analyses `probe` (the latest when `None`) and writes its field map and report.
pub fn analyze_probe(store: &SessionStore, probe: Option<&str>) -> Result<Analysis, AnalyzeError> {
    let info = store.info()?;
    let marks = store.marks()?;
    let timelines = probe_timelines(&marks);
    let timeline = match probe {
        Some(id) => timelines.iter().find(|t| t.probe == id).ok_or_else(|| AnalyzeError::UnknownProbe(id.to_string()))?,
        None => timelines.last().ok_or(AnalyzeError::NoProbes)?,
    };
    let events = load_probe_events(store, &timeline.probe)?;
    let parameter = timeline.plan.parameter.clone();
    let input = ProbeInput {
        timeline,
        events: &events,
        capture: format!("captures/{}.pcapng", timeline.probe),
        vid: info.vid,
        pid: info.pid,
        descriptor_hex: info.device_descriptor_hex.as_deref(),
    };
    let map = field_map(&input, &parameter);
    std::fs::create_dir_all(store.root().join("analysis"))?;
    let json = field_map_path(store, &parameter);
    let md = json.with_extension("md");
    std::fs::write(&json, serde_json::to_vec_pretty(&map)?)?;
    std::fs::write(&md, report(&map))?;
    Ok(Analysis { probe: timeline.probe.clone(), map, json, report: md })
}
