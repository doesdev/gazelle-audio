//! Marks → per-step time windows. The seam `analysis` (sub-project 2) segments on: each
//! closed step yields an action window (armed → done) and a settle window (done → closed).

use std::collections::HashMap;
use std::ops::Range;

use serde::{Deserialize, Serialize};

use super::marks::{Mark, MarkKind};
use super::model::ProbePlan;
use super::plan::StepSpec;
use super::step::RunStatus;
use crate::capture::event::UsbEvent;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepWindow {
    pub step: usize,
    /// The attempt that closed; earlier attempts were discarded by redo.
    pub attempt: u32,
    pub armed_ns: u64,
    pub done_ns: u64,
    pub closed_ns: u64,
    pub actual_value: Option<String>,
    pub clock_suspect: bool,
}

impl StepWindow {
    pub fn action(&self) -> Range<u64> {
        self.armed_ns..self.done_ns
    }

    pub fn settle(&self) -> Range<u64> {
        self.done_ns..self.closed_ns
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeTimeline {
    pub probe: String,
    pub plan: ProbePlan,
    pub seed: u64,
    pub steps: Vec<StepSpec>,
    /// Closed steps in step order.
    pub windows: Vec<StepWindow>,
    /// `(step, reason)` for skipped steps.
    pub skipped: Vec<(usize, String)>,
    /// Redo count per step index (absent = 0).
    pub redos: HashMap<usize, u32>,
    /// `None` while the probe is still running (or the helper stopped mid-probe).
    pub outcome: Option<RunStatus>,
}

#[derive(Default)]
struct Partial {
    attempt: u32,
    armed_ns: Option<u64>,
    done_ns: Option<u64>,
    actual_value: Option<String>,
    clock_suspect: bool,
}

/// Rebuilds every probe's timeline from `marks.jsonl` order.
pub fn probe_timelines(marks: &[Mark]) -> Vec<ProbeTimeline> {
    let mut timelines: Vec<ProbeTimeline> = Vec::new();
    let mut partial: HashMap<(String, usize), Partial> = HashMap::new();
    for mark in marks {
        if let MarkKind::ProbeStarted { plan, seed, steps } = &mark.kind {
            timelines.push(ProbeTimeline {
                probe: mark.probe.clone(),
                plan: plan.clone(),
                seed: *seed,
                steps: steps.clone(),
                windows: Vec::new(),
                skipped: Vec::new(),
                redos: HashMap::new(),
                outcome: None,
            });
            continue;
        }
        let Some(tl) = timelines.iter_mut().rev().find(|t| t.probe == mark.probe) else {
            continue;
        };
        match (&mark.kind, mark.step) {
            (MarkKind::Completed, _) => tl.outcome = Some(RunStatus::Completed),
            (MarkKind::Abandoned, _) => tl.outcome = Some(RunStatus::Abandoned),
            (_, None) => {}
            (kind, Some(step)) => {
                let p = partial.entry((mark.probe.clone(), step)).or_default();
                if mark.attempt != p.attempt {
                    *p = Partial { attempt: mark.attempt, ..Partial::default() };
                }
                p.clock_suspect |= mark.clock_suspect;
                match kind {
                    MarkKind::Armed => p.armed_ns = Some(mark.wall_ns),
                    MarkKind::Done => p.done_ns = Some(mark.wall_ns),
                    MarkKind::ActualValue { value } => p.actual_value = Some(value.clone()),
                    MarkKind::Redo => *tl.redos.entry(step).or_default() += 1,
                    MarkKind::Skipped { reason } => tl.skipped.push((step, reason.clone())),
                    MarkKind::Closed => {
                        if let (Some(armed_ns), Some(done_ns)) = (p.armed_ns, p.done_ns) {
                            tl.windows.push(StepWindow {
                                step,
                                attempt: p.attempt,
                                armed_ns,
                                done_ns,
                                closed_ns: mark.wall_ns,
                                actual_value: p.actual_value.clone(),
                                clock_suspect: p.clock_suspect,
                            });
                        }
                    }
                    MarkKind::ProbeStarted { .. } | MarkKind::Completed | MarkKind::Abandoned => {}
                }
            }
        }
    }
    timelines
}

/// Events with `ts_ns` in `range`; `events` must be sorted by `ts_ns`.
pub fn events_in(events: &[UsbEvent], range: Range<u64>) -> &[UsbEvent] {
    let start = events.partition_point(|e| e.ts_ns < range.start);
    let end = events.partition_point(|e| e.ts_ns < range.end);
    &events[start..end.max(start)]
}
