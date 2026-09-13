//! The step state machine. Pure: time and packet clocks are passed in, marks are returned.

use serde::{Deserialize, Serialize};

use super::authority::OperatorAuthority;
use super::marks::{clock_suspect, Mark, MarkKind, PacketClock};
use super::model::ProbePlan;
use super::plan::{expand, StepKind, StepSpec};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepTiming {
    pub pre_roll_ns: u64,
    pub post_roll_ns: u64,
    pub idle_ns: u64,
}

impl Default for StepTiming {
    fn default() -> Self {
        Self { pre_roll_ns: 1_500_000_000, post_roll_ns: 1_500_000_000, idle_ns: 5_000_000_000 }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum StepState {
    Pending,
    Armed { armed_ns: u64 },
    Done { armed_ns: u64, done_ns: u64 },
    Closed { armed_ns: u64, done_ns: u64, closed_ns: u64 },
    Skipped { reason: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Running,
    Completed,
    Abandoned,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepRecord {
    pub spec: StepSpec,
    pub state: StepState,
    pub attempt: u32,
    pub actual_value: Option<String>,
    pub clock_suspect: bool,
}

/// What the operator can do from the panel. Agents have no path to these.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "op")]
pub enum OperatorCommand {
    Done,
    Redo,
    Skip { reason: String },
    ActualValue { value: String },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StepError {
    #[error("no probe is running")]
    NotRunning,
    #[error("pre-roll still running: wait {wait_ms} ms")]
    TooEarly { wait_ms: u64 },
    #[error("idle steps finish on their own")]
    IdleStep,
    #[error("step is already done")]
    AlreadyDone,
    #[error("this step has no value to record")]
    NoValueToRecord,
    #[error("a skip needs a reason and an actual value needs a value")]
    Empty,
}

#[derive(Debug, Clone)]
pub struct ProbeRun {
    probe_id: String,
    steps: Vec<StepRecord>,
    current: usize,
    status: RunStatus,
    timing: StepTiming,
}

impl ProbeRun {
    /// Expands `plan` with `seed` and arms the first step. Returns the `ProbeStarted` and
    /// first `Armed` marks.
    pub fn start(
        probe_id: &str,
        plan: &ProbePlan,
        seed: u64,
        timing: StepTiming,
        now_ns: u64,
        clock: Option<PacketClock>,
    ) -> (Self, Vec<Mark>) {
        let specs = expand(plan, seed);
        let mut run = Self {
            probe_id: probe_id.to_string(),
            steps: specs
                .iter()
                .cloned()
                .map(|spec| StepRecord {
                    spec,
                    state: StepState::Pending,
                    attempt: 0,
                    actual_value: None,
                    clock_suspect: false,
                })
                .collect(),
            current: 0,
            status: RunStatus::Running,
            timing,
        };
        let mut marks = vec![run.mark(
            None,
            now_ns,
            clock,
            MarkKind::ProbeStarted { plan: plan.clone(), seed, steps: specs },
        )];
        run.arm_current(now_ns, clock, &mut marks);
        (run, marks)
    }

    pub fn probe_id(&self) -> &str {
        &self.probe_id
    }

    pub fn status(&self) -> RunStatus {
        self.status
    }

    pub fn steps(&self) -> &[StepRecord] {
        &self.steps
    }

    pub fn timing(&self) -> StepTiming {
        self.timing
    }

    /// The step awaiting the operator, while running.
    pub fn current(&self) -> Option<&StepRecord> {
        (self.status == RunStatus::Running).then(|| &self.steps[self.current])
    }

    /// Nanoseconds until Done is accepted on the current step (0 when ready or not armed).
    pub fn ready_in_ns(&self, now_ns: u64) -> u64 {
        match self.current().map(|s| &s.state) {
            Some(StepState::Armed { armed_ns }) => {
                (armed_ns + self.timing.pre_roll_ns).saturating_sub(now_ns)
            }
            _ => 0,
        }
    }

    /// Applies an operator command to the current step.
    pub fn operator(
        &mut self,
        _authority: &OperatorAuthority,
        command: OperatorCommand,
        now_ns: u64,
        clock: Option<PacketClock>,
    ) -> Result<Vec<Mark>, StepError> {
        if self.status != RunStatus::Running {
            return Err(StepError::NotRunning);
        }
        let mut marks = Vec::new();
        let i = self.current;
        let kind = self.steps[i].spec.kind;
        match command {
            OperatorCommand::Done => match self.steps[i].state {
                StepState::Armed { .. } if kind == StepKind::Idle => return Err(StepError::IdleStep),
                StepState::Armed { armed_ns } => {
                    let ready = armed_ns + self.timing.pre_roll_ns;
                    if now_ns < ready {
                        return Err(StepError::TooEarly { wait_ms: (ready - now_ns).div_ceil(1_000_000) });
                    }
                    self.finish_action(armed_ns, now_ns, clock, &mut marks);
                }
                StepState::Done { .. } => return Err(StepError::AlreadyDone),
                _ => unreachable!("current step is always armed or done while running"),
            },
            OperatorCommand::Redo => {
                marks.push(self.step_mark(now_ns, clock, MarkKind::Redo));
                let step = &mut self.steps[i];
                step.attempt += 1;
                step.actual_value = None;
                step.clock_suspect = false;
                self.arm_current(now_ns, clock, &mut marks);
            }
            OperatorCommand::Skip { reason } => {
                if reason.trim().is_empty() {
                    return Err(StepError::Empty);
                }
                marks.push(self.step_mark(now_ns, clock, MarkKind::Skipped { reason: reason.clone() }));
                self.steps[i].state = StepState::Skipped { reason };
                self.advance(now_ns, clock, &mut marks);
            }
            OperatorCommand::ActualValue { value } => {
                if matches!(kind, StepKind::Idle | StepKind::Control) {
                    return Err(StepError::NoValueToRecord);
                }
                if value.trim().is_empty() {
                    return Err(StepError::Empty);
                }
                marks.push(self.step_mark(now_ns, clock, MarkKind::ActualValue { value: value.clone() }));
                self.steps[i].actual_value = Some(value);
            }
        }
        Ok(marks)
    }

    /// Advances timers: idle steps finish after pre-roll + idle window; done steps close
    /// after the post-roll.
    pub fn tick(&mut self, now_ns: u64, clock: Option<PacketClock>) -> Vec<Mark> {
        let mut marks = Vec::new();
        while self.status == RunStatus::Running {
            let step = &self.steps[self.current];
            match step.state {
                StepState::Armed { armed_ns }
                    if step.spec.kind == StepKind::Idle
                        && now_ns >= armed_ns + self.timing.pre_roll_ns + self.timing.idle_ns =>
                {
                    self.finish_action(armed_ns, now_ns, clock, &mut marks);
                }
                StepState::Done { armed_ns, done_ns } if now_ns >= done_ns + self.timing.post_roll_ns => {
                    marks.push(self.step_mark(now_ns, clock, MarkKind::Closed));
                    self.steps[self.current].state = StepState::Closed { armed_ns, done_ns, closed_ns: now_ns };
                    self.advance(now_ns, clock, &mut marks);
                    // The next step was armed at `now_ns`; nothing more can happen this tick.
                    break;
                }
                _ => break,
            }
        }
        marks
    }

    /// Cancels the probe. The capture is kept; the timeline records the abandonment.
    pub fn abandon(&mut self, now_ns: u64, clock: Option<PacketClock>) -> Result<Vec<Mark>, StepError> {
        if self.status != RunStatus::Running {
            return Err(StepError::NotRunning);
        }
        let mark = self.step_mark(now_ns, clock, MarkKind::Abandoned);
        self.status = RunStatus::Abandoned;
        Ok(vec![mark])
    }

    fn finish_action(&mut self, armed_ns: u64, now_ns: u64, clock: Option<PacketClock>, marks: &mut Vec<Mark>) {
        marks.push(self.step_mark(now_ns, clock, MarkKind::Done));
        self.steps[self.current].state = StepState::Done { armed_ns, done_ns: now_ns };
    }

    fn arm_current(&mut self, now_ns: u64, clock: Option<PacketClock>, marks: &mut Vec<Mark>) {
        marks.push(self.step_mark(now_ns, clock, MarkKind::Armed));
        self.steps[self.current].state = StepState::Armed { armed_ns: now_ns };
    }

    fn advance(&mut self, now_ns: u64, clock: Option<PacketClock>, marks: &mut Vec<Mark>) {
        if self.current + 1 == self.steps.len() {
            self.status = RunStatus::Completed;
            marks.push(self.mark(None, now_ns, clock, MarkKind::Completed));
        } else {
            self.current += 1;
            self.arm_current(now_ns, clock, marks);
        }
    }

    /// A mark on the current step; folds its clock check into the step's flag.
    fn step_mark(&mut self, now_ns: u64, clock: Option<PacketClock>, kind: MarkKind) -> Mark {
        let mark = self.mark(Some(self.current), now_ns, clock, kind);
        let step = &mut self.steps[self.current];
        step.clock_suspect |= mark.clock_suspect;
        mark
    }

    fn mark(&self, step: Option<usize>, now_ns: u64, clock: Option<PacketClock>, kind: MarkKind) -> Mark {
        Mark {
            probe: self.probe_id.clone(),
            step,
            attempt: step.map_or(0, |i| self.steps[i].attempt),
            wall_ns: now_ns,
            last_packet: clock,
            clock_suspect: clock_suspect(clock, self.timing.pre_roll_ns),
            kind,
        }
    }
}

#[cfg(test)]
#[path = "step_tests.rs"]
mod tests;
