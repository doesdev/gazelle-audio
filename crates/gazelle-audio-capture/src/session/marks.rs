//! The append-only step timeline written to `marks.jsonl`.

use serde::{Deserialize, Serialize};

use super::model::ProbePlan;
use super::plan::StepSpec;

/// The newest packet seen when a mark was made: its capture timestamp and the host wall
/// clock when the helper received it. Both nanoseconds since the Unix epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PacketClock {
    pub ts_ns: u64,
    pub host_ns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum MarkKind {
    ProbeStarted { plan: ProbePlan, seed: u64, steps: Vec<StepSpec> },
    Armed,
    Done,
    Closed,
    Redo,
    Skipped { reason: String },
    ActualValue { value: String },
    Completed,
    Abandoned,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mark {
    pub probe: String,
    /// Zero-based step index; `None` for probe-level marks.
    pub step: Option<usize>,
    /// Zero-based attempt of the step; increments on every redo.
    pub attempt: u32,
    /// Host wall clock (Windows: `GetSystemTimePreciseAsFileTime` via `SystemTime::now`).
    pub wall_ns: u64,
    pub last_packet: Option<PacketClock>,
    pub clock_suspect: bool,
    pub kind: MarkKind,
}

/// True when the capture clock and the host clock disagree by more than `tolerance_ns`
/// (the pre-roll), measured on the newest packet. No packet yet means nothing to compare.
pub fn clock_suspect(last_packet: Option<PacketClock>, tolerance_ns: u64) -> bool {
    last_packet.is_some_and(|p| p.host_ns.abs_diff(p.ts_ns) > tolerance_ns)
}
