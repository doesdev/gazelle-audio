//! Closed step windows paired with the events inside them.

use crate::capture::event::UsbEvent;
use crate::session::plan::StepKind;
use crate::session::timeline::{events_in, ProbeTimeline, StepWindow};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment<'a> {
    pub window: StepWindow,
    pub kind: StepKind,
    /// Parameter the operator acted on; `None` for idle steps.
    pub parameter: Option<String>,
    /// UI value the step left the parameter at: the recorded actual value, else the requested
    /// one. `None` for idle and control steps.
    pub value: Option<String>,
    /// Events from armed to closed: the action window followed by the settle window.
    pub events: &'a [UsbEvent],
}

/// One segment per closed step, in step order. Skipped steps have no window and no segment.
/// `events` must be sorted by `ts_ns`.
pub fn segments<'a>(timeline: &ProbeTimeline, events: &'a [UsbEvent]) -> Vec<Segment<'a>> {
    timeline
        .windows
        .iter()
        .filter_map(|w| {
            let spec = timeline.steps.get(w.step)?;
            Some(Segment {
                window: w.clone(),
                kind: spec.kind,
                parameter: spec.parameter.clone(),
                value: w.actual_value.clone().or_else(|| spec.to.clone()),
                events: events_in(events, w.armed_ns..w.closed_ns),
            })
        })
        .collect()
}
