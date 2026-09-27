//! **Auto-arm**: keep the recorder armed with one preset, from the moment Gazelle starts, and again
//! when the interfaces drop out and come back.
//!
//! This is the decision alone, as plain data, so every rule is tested without a recorder, a device
//! or a clock (`crate::recording` does what it decides). Each tick it is told what the recorder is
//! doing, whether a measurement is running and whether the interfaces are there, and answers wait,
//! arm or disarm.
//!
//! # The rules
//!
//! - **Off by default.** It only acts with the setting on and a preset named.
//! - **At start**, and whenever the setting is turned on, it arms as soon as it can.
//! - **It does not fight the person.** Disarming by hand **pauses** it: it does not arm again until
//!   Gazelle next starts, the person arms by hand (which resumes it), or the setting is turned off and
//!   on. A person who disarmed wanted the drivers let go, for a DAW or a measurement, and arming
//!   behind their back would take them away again.
//! - **It does not fight a measurement**, which needs the same aggregate: while one runs it waits,
//!   and a refusal because one has just started is a wait too, never a failure.
//! - **When the interfaces go away while armed**, and stay away for [`LOSS_GRACE`], it disarms,
//!   which finishes any take's files, and arms again once they are back. The grace keeps a brief
//!   hiccup on the control interface from cutting a take.
//! - **A refusal backs off**: 5 s, 15 s, 30 s, 1 min, 2 min, then every 5 min ([`backoff`]), with
//!   the reason kept for the page to show. There is never a tight retry loop. The interfaces coming
//!   back, a manual arm, or the setting changing starts the count again.
//! - Armed by hand with another preset, it leaves that alone: armed is armed.

use std::time::{Duration, Instant};

use serde::Serialize;

/// How long the interfaces must be gone before an armed recorder is disarmed for them.
pub const LOSS_GRACE: Duration = Duration::from_secs(5);
/// How long to wait before asking again after a measurement had the aggregate.
pub const MEASURING_WAIT: Duration = Duration::from_secs(5);

/// The waits after the first, second, ... refusal in a row. The last one repeats.
const BACKOFF: [u64; 6] = [5, 15, 30, 60, 120, 300];

/// How long to wait after `failures` refusals in a row (at least one).
pub fn backoff(failures: u32) -> Duration {
    let at = (failures.max(1) as usize - 1).min(BACKOFF.len() - 1);
    Duration::from_secs(BACKOFF[at])
}

/// What the recorder is doing, as auto-arm needs to know it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Recorder {
    Off,
    /// Arming or disarming: something is already happening.
    Busy,
    Armed,
    Recording,
}

impl Recorder {
    /// From the recorder's own word for its state.
    pub fn from_state(state: &str) -> Recorder {
        match state {
            "off" => Recorder::Off,
            "armed" => Recorder::Armed,
            "recording" => Recorder::Recording,
            _ => Recorder::Busy,
        }
    }
}

/// Everything one tick looks at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Inputs<'a> {
    /// The preset to arm with: `None` when auto-arm is off.
    pub preset: Option<&'a str>,
    pub recorder: Recorder,
    /// A measurement is running on the Aggregate page.
    pub measuring: bool,
    /// The interfaces the aggregate needs are attached.
    pub present: bool,
}

/// What to do now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    Wait,
    Arm(String),
    /// The interfaces have gone: disarm, finishing any take.
    Disarm,
}

/// Where auto-arm is, as the pages show it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// The setting is off.
    Off,
    /// The recorder is armed, by auto-arm or by hand.
    Armed,
    /// About to arm, or arming.
    Arming,
    /// Disarmed by hand: waiting for Gazelle's next start, or a manual arm.
    Paused,
    WaitingForInterfaces,
    WaitingForMeasurement,
    /// The last try was refused; the next one is at `retry_at`.
    BackingOff,
}

/// The decision, and what it remembers between ticks.
#[derive(Clone, Debug, Default)]
pub struct AutoArm {
    /// Disarmed by hand since it last armed.
    paused: bool,
    /// The last disarm was auto-arm's own, because the interfaces went away.
    lost: bool,
    failures: u32,
    next_try: Option<Instant>,
    absent_since: Option<Instant>,
    was_present: bool,
    /// The last refusal, in the recorder's words.
    reason: Option<String>,
    /// The phase the last tick left it in.
    phase: Option<Phase>,
    /// The preset last acted for, so a change of preset starts afresh.
    preset: Option<String>,
}

impl AutoArm {
    pub fn new() -> AutoArm {
        AutoArm { was_present: true, ..AutoArm::default() }
    }

    /// One tick.
    pub fn decide(&mut self, now: Instant, inputs: Inputs<'_>) -> Step {
        let Some(preset) = inputs.preset else {
            // Off: forget everything, so turning it on starts afresh.
            *self = AutoArm { was_present: inputs.present, phase: Some(Phase::Off), ..AutoArm::default() };
            return Step::Wait;
        };
        if self.preset.as_deref() != Some(preset) {
            self.preset = Some(preset.to_string());
            self.resume();
        }
        if inputs.present {
            if !self.was_present {
                // Back: whatever went wrong before is worth trying again at once.
                self.failures = 0;
                self.next_try = None;
            }
            self.absent_since = None;
        } else if self.absent_since.is_none() {
            self.absent_since = Some(now);
        }
        self.was_present = inputs.present;

        let (step, phase) = match inputs.recorder {
            Recorder::Armed | Recorder::Recording => {
                let gone = self.absent_since.is_some_and(|since| now.saturating_duration_since(since) >= LOSS_GRACE);
                if gone {
                    (Step::Disarm, Phase::WaitingForInterfaces)
                } else {
                    (Step::Wait, Phase::Armed)
                }
            }
            Recorder::Busy => (Step::Wait, Phase::Arming),
            Recorder::Off if self.paused => (Step::Wait, Phase::Paused),
            Recorder::Off if inputs.measuring => (Step::Wait, Phase::WaitingForMeasurement),
            Recorder::Off if !inputs.present => (Step::Wait, Phase::WaitingForInterfaces),
            Recorder::Off if self.next_try.is_some_and(|at| now < at) => (Step::Wait, if self.failures > 0 { Phase::BackingOff } else { Phase::WaitingForMeasurement }),
            Recorder::Off => (Step::Arm(preset.to_string()), Phase::Arming),
        };
        self.phase = Some(phase);
        step
    }

    /// The arm it asked for worked.
    pub fn armed(&mut self) {
        self.failures = 0;
        self.next_try = None;
        self.reason = None;
        self.lost = false;
        self.phase = Some(Phase::Armed);
    }

    /// The arm it asked for was refused. `measuring` when a measurement had the aggregate, which is
    /// a wait and not a failure. Returns how long until it tries again.
    pub fn refused(&mut self, now: Instant, measuring: bool, why: &str) -> Duration {
        let wait = if measuring {
            self.phase = Some(Phase::WaitingForMeasurement);
            MEASURING_WAIT
        } else {
            self.failures = self.failures.saturating_add(1);
            self.reason = Some(why.to_string());
            self.phase = Some(Phase::BackingOff);
            backoff(self.failures)
        };
        self.next_try = Some(now + wait);
        wait
    }

    /// It disarmed because the interfaces went away.
    pub fn disarmed_for_loss(&mut self) {
        self.lost = true;
        self.phase = Some(Phase::WaitingForInterfaces);
    }

    /// The person disarmed: pause until Gazelle next starts or they arm by hand.
    pub fn disarmed_by_hand(&mut self) {
        self.paused = true;
        self.lost = false;
        self.phase = Some(Phase::Paused);
    }

    /// The person armed by hand, or turned the setting on: whatever was holding it back is over.
    pub fn resume(&mut self) {
        self.paused = false;
        self.failures = 0;
        self.next_try = None;
        self.reason = None;
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// Where it is, for the pages. `retry_in` is the time to the next try, while backing off.
    pub fn status(&self, now: Instant) -> Status {
        let phase = self.phase.unwrap_or(Phase::Off);
        Status {
            phase,
            reason: self.reason.clone().filter(|_| phase == Phase::BackingOff),
            failures: self.failures,
            retry_in: self.next_try.filter(|_| phase == Phase::BackingOff).map(|at| at.saturating_duration_since(now)),
            lost: self.lost,
        }
    }
}

/// [`AutoArm::status`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    pub phase: Phase,
    pub reason: Option<String>,
    pub failures: u32,
    pub retry_in: Option<Duration>,
    /// The interfaces went away while it was armed, and it is waiting for them.
    pub lost: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    const BAND: Option<&str> = Some("band");

    fn inputs(recorder: Recorder) -> Inputs<'static> {
        Inputs { preset: BAND, recorder, measuring: false, present: true }
    }

    fn at(start: Instant, seconds: u64) -> Instant {
        start + Duration::from_secs(seconds)
    }

    #[test]
    fn off_does_nothing_whatever_the_recorder_is_doing() {
        let mut auto = AutoArm::new();
        let now = Instant::now();
        for recorder in [Recorder::Off, Recorder::Armed, Recorder::Recording] {
            assert_eq!(auto.decide(now, Inputs { preset: None, present: false, ..inputs(recorder) }), Step::Wait);
            assert_eq!(auto.status(now).phase, Phase::Off);
        }
    }

    #[test]
    fn at_start_it_arms_at_once_and_then_leaves_the_recorder_alone() {
        let mut auto = AutoArm::new();
        let now = Instant::now();
        assert_eq!(auto.decide(now, inputs(Recorder::Off)), Step::Arm("band".into()));
        auto.armed();
        for recorder in [Recorder::Armed, Recorder::Recording, Recorder::Busy] {
            assert_eq!(auto.decide(at(now, 1), inputs(recorder)), Step::Wait);
        }
        assert_eq!(auto.status(now).phase, Phase::Arming, "busy is on its way");
        auto.decide(at(now, 2), inputs(Recorder::Armed));
        assert_eq!(auto.status(now).phase, Phase::Armed);
    }

    #[test]
    fn a_hand_disarm_pauses_it_until_a_hand_arm_resumes_it() {
        let mut auto = AutoArm::new();
        let now = Instant::now();
        auto.decide(now, inputs(Recorder::Off));
        auto.armed();
        auto.disarmed_by_hand();
        for later in [1, 60, 3600] {
            assert_eq!(auto.decide(at(now, later), inputs(Recorder::Off)), Step::Wait, "never behind the person's back");
        }
        assert_eq!(auto.status(now).phase, Phase::Paused);
        // Even the interfaces going and coming back does not undo what the person did.
        auto.decide(at(now, 3601), Inputs { present: false, ..inputs(Recorder::Off) });
        assert_eq!(auto.decide(at(now, 3602), inputs(Recorder::Off)), Step::Wait);
        // Arming by hand resumes it: the next loss is followed again.
        auto.resume();
        auto.decide(at(now, 3603), inputs(Recorder::Armed));
        assert!(!auto.is_paused());
    }

    #[test]
    fn the_interfaces_going_away_disarms_after_the_grace_and_their_return_arms_again() {
        let mut auto = AutoArm::new();
        let now = Instant::now();
        auto.decide(now, inputs(Recorder::Off));
        auto.armed();
        let gone = |recorder| Inputs { present: false, ..inputs(recorder) };
        assert_eq!(auto.decide(at(now, 10), gone(Recorder::Recording)), Step::Wait, "a hiccup does not cut a take");
        assert_eq!(auto.decide(at(now, 12), gone(Recorder::Recording)), Step::Wait);
        // Back within the grace: nothing happened.
        assert_eq!(auto.decide(at(now, 13), inputs(Recorder::Recording)), Step::Wait);
        assert_eq!(auto.decide(at(now, 20), gone(Recorder::Recording)), Step::Wait, "the grace starts again");
        assert_eq!(auto.decide(at(now, 25), gone(Recorder::Recording)), Step::Disarm);
        auto.disarmed_for_loss();
        assert_eq!(auto.decide(at(now, 26), gone(Recorder::Off)), Step::Wait);
        let status = auto.status(at(now, 26));
        assert_eq!((status.phase, status.lost), (Phase::WaitingForInterfaces, true));
        assert_eq!(auto.decide(at(now, 90), inputs(Recorder::Off)), Step::Arm("band".into()), "back, and armed at once");
    }

    #[test]
    fn refusals_back_off_and_say_why_without_a_tight_loop() {
        let mut auto = AutoArm::new();
        let now = Instant::now();
        let mut clock = now;
        let mut waits = Vec::new();
        for _ in 0..8 {
            assert_eq!(auto.decide(clock, inputs(Recorder::Off)), Step::Arm("band".into()));
            let wait = auto.refused(clock, false, "a DAW has the interfaces");
            waits.push(wait.as_secs());
            // Every tick until then waits.
            for second in 1..wait.as_secs() {
                assert_eq!(auto.decide(clock + Duration::from_secs(second), inputs(Recorder::Off)), Step::Wait);
            }
            let status = auto.status(clock + Duration::from_secs(1));
            assert_eq!(status.phase, Phase::BackingOff);
            assert_eq!(status.reason.as_deref(), Some("a DAW has the interfaces"));
            assert_eq!(status.retry_in, Some(wait - Duration::from_secs(1)));
            clock += wait;
        }
        assert_eq!(waits, [5, 15, 30, 60, 120, 300, 300, 300]);
        assert_eq!(auto.status(clock).failures, 8);
        // Success forgets it all.
        auto.decide(clock, inputs(Recorder::Off));
        auto.armed();
        assert_eq!(auto.status(clock), Status { phase: Phase::Armed, reason: None, failures: 0, retry_in: None, lost: false });
    }

    #[test]
    fn the_interfaces_coming_back_or_a_new_preset_start_the_count_again() {
        let mut auto = AutoArm::new();
        let now = Instant::now();
        auto.decide(now, inputs(Recorder::Off));
        auto.refused(now, false, "no");
        auto.decide(at(now, 5), inputs(Recorder::Off));
        auto.refused(at(now, 5), false, "no");
        assert_eq!(auto.decide(at(now, 6), inputs(Recorder::Off)), Step::Wait, "15 s to go");
        auto.decide(at(now, 7), Inputs { present: false, ..inputs(Recorder::Off) });
        assert_eq!(auto.decide(at(now, 8), inputs(Recorder::Off)), Step::Arm("band".into()), "they came back: try now");
        auto.refused(at(now, 8), false, "no");
        assert_eq!(auto.decide(at(now, 9), Inputs { preset: Some("other"), ..inputs(Recorder::Off) }), Step::Arm("other".into()), "a new preset: try now");
    }

    #[test]
    fn a_measurement_is_waited_for_never_counted_against_it() {
        let mut auto = AutoArm::new();
        let now = Instant::now();
        let measuring = Inputs { measuring: true, ..inputs(Recorder::Off) };
        for second in 0..30 {
            assert_eq!(auto.decide(at(now, second), measuring), Step::Wait);
        }
        assert_eq!(auto.status(now).phase, Phase::WaitingForMeasurement);
        // It ended, and another started in the moment between: the recorder said so.
        assert_eq!(auto.decide(at(now, 30), inputs(Recorder::Off)), Step::Arm("band".into()));
        assert_eq!(auto.refused(at(now, 30), true, "a measurement"), MEASURING_WAIT);
        assert_eq!(auto.status(at(now, 31)).failures, 0);
        assert_eq!(auto.status(at(now, 31)).reason, None);
        assert_eq!(auto.decide(at(now, 32), inputs(Recorder::Off)), Step::Wait);
        assert_eq!(auto.decide(at(now, 35), inputs(Recorder::Off)), Step::Arm("band".into()));
    }

    #[test]
    fn turning_it_off_and_on_forgets_a_pause() {
        let mut auto = AutoArm::new();
        let now = Instant::now();
        auto.decide(now, inputs(Recorder::Off));
        auto.armed();
        auto.disarmed_by_hand();
        assert_eq!(auto.decide(at(now, 1), inputs(Recorder::Off)), Step::Wait);
        auto.decide(at(now, 2), Inputs { preset: None, ..inputs(Recorder::Off) });
        assert_eq!(auto.decide(at(now, 3), inputs(Recorder::Off)), Step::Arm("band".into()));
    }

    #[test]
    fn the_recorders_words_are_read_as_they_are_meant() {
        assert_eq!(Recorder::from_state("off"), Recorder::Off);
        assert_eq!(Recorder::from_state("armed"), Recorder::Armed);
        assert_eq!(Recorder::from_state("recording"), Recorder::Recording);
        assert_eq!(Recorder::from_state("arming"), Recorder::Busy);
        assert_eq!(Recorder::from_state("disarming"), Recorder::Busy);
    }
}
