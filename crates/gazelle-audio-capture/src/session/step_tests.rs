use super::*;
use crate::session::marks::MarkKind as K;

const S: u64 = 1_000_000_000;
const T0: u64 = 1_000 * S;

fn plan(value_b: &[&str], sweep: &[&str], repeats: u32) -> ProbePlan {
    ProbePlan {
        parameter: "level".into(),
        value_a: "0 dB".into(),
        value_b: value_b.iter().map(|s| s.to_string()).collect(),
        sweep: sweep.iter().map(|s| s.to_string()).collect(),
        repeats,
        control_parameter: "mute".into(),
    }
}

fn run() -> ProbeRun {
    ProbeRun::start("p1", &plan(&["-6 dB"], &[], 1), 7, StepTiming::default(), T0, None).0
}

fn auth() -> OperatorAuthority {
    OperatorAuthority::grant()
}

fn kinds(marks: &[Mark]) -> Vec<MarkKind> {
    marks.iter().map(|m| m.kind.clone()).collect()
}

/// Drives the current step to `Done` at `now` (ticks idle steps, presses Done otherwise).
fn complete_current(run: &mut ProbeRun, now: &mut u64) {
    let t = run.timing();
    let idx = run.current().unwrap().spec.index;
    if run.current().unwrap().spec.kind == StepKind::Idle {
        *now += t.pre_roll_ns + t.idle_ns;
        run.tick(*now, None);
    } else {
        *now += t.pre_roll_ns;
        run.operator(&auth(), OperatorCommand::Done, *now, None).unwrap();
    }
    *now += t.post_roll_ns;
    run.tick(*now, None);
    assert!(run.status() != RunStatus::Running || run.current().unwrap().spec.index == idx + 1);
}

#[test]
fn start_emits_probe_started_and_arms_step_zero() {
    let (run, marks) = ProbeRun::start("p1", &plan(&["-6 dB"], &[], 1), 7, StepTiming::default(), T0, None);
    assert!(matches!(marks[0].kind, K::ProbeStarted { seed: 7, .. }));
    assert_eq!(marks[1].kind, K::Armed);
    assert_eq!((marks[1].step, marks[1].wall_ns), (Some(0), T0));
    assert_eq!(run.current().unwrap().state, StepState::Armed { armed_ns: T0 });
    assert_eq!(run.ready_in_ns(T0), 1_500_000_000);
}

#[test]
fn full_run_walks_every_step_and_completes() {
    let mut r = run();
    let mut now = T0;
    let n = r.steps().len();
    for _ in 0..n {
        complete_current(&mut r, &mut now);
    }
    assert_eq!(r.status(), RunStatus::Completed);
    assert!(r.steps().iter().all(|s| matches!(s.state, StepState::Closed { .. })));
}

#[test]
fn idle_step_finishes_on_its_own_after_pre_roll_and_idle_window() {
    let mut r = run();
    assert_eq!(r.current().unwrap().spec.kind, StepKind::Idle);
    assert!(r.tick(T0 + 6_499_999_999, None).is_empty());
    assert_eq!(kinds(&r.tick(T0 + 6_500_000_000, None)), vec![K::Done]);
    assert!(r.tick(T0 + 7_999_999_999, None).is_empty());
    assert_eq!(kinds(&r.tick(T0 + 8 * S, None)), vec![K::Closed, K::Armed]);
    assert_eq!(r.current().unwrap().spec.index, 1);
}

/// Every (state, command) pair. States: idle armed, set armed in pre-roll, set armed ready,
/// set done (post-roll), control armed ready, completed, abandoned.
#[test]
fn operator_commands_exhaustive_table() {
    #[derive(Clone, Copy, Debug)]
    enum At {
        IdleArmed,
        SetPreRoll,
        SetReady,
        SetDone,
        ControlReady,
        Completed,
        Abandoned,
    }
    fn setup(at: At) -> (ProbeRun, u64) {
        let mut r = run();
        let mut now = T0;
        let t = r.timing();
        let goto = |r: &mut ProbeRun, now: &mut u64, kind: StepKind| {
            while r.current().unwrap().spec.kind != kind {
                complete_current(r, now);
            }
        };
        match at {
            At::IdleArmed => {}
            At::SetPreRoll => goto(&mut r, &mut now, StepKind::Set),
            At::SetReady => {
                goto(&mut r, &mut now, StepKind::Set);
                now += t.pre_roll_ns;
            }
            At::SetDone => {
                goto(&mut r, &mut now, StepKind::Set);
                now += t.pre_roll_ns;
                r.operator(&auth(), OperatorCommand::Done, now, None).unwrap();
            }
            At::ControlReady => {
                goto(&mut r, &mut now, StepKind::Control);
                now += t.pre_roll_ns;
            }
            At::Completed => {
                for _ in 0..r.steps().len() {
                    complete_current(&mut r, &mut now);
                }
            }
            At::Abandoned => {
                r.abandon(now, None).unwrap();
            }
        }
        (r, now)
    }
    use OperatorCommand as C;
    let commands = [
        C::Done,
        C::Redo,
        C::Skip { reason: "ui froze".into() },
        C::Skip { reason: " ".into() },
        C::ActualValue { value: "-5.5 dB".into() },
        C::ActualValue { value: "".into() },
    ];
    // Expected result per command, in the order above. Ok(marks) lists mark kinds.
    type Expected = [Result<Vec<K>, StepError>; 6];
    let table: Vec<(At, Expected)> = vec![
        (At::IdleArmed, [
            Err(StepError::IdleStep),
            Ok(vec![K::Redo, K::Armed]),
            Ok(vec![K::Skipped { reason: "ui froze".into() }, K::Armed]),
            Err(StepError::Empty),
            Err(StepError::NoValueToRecord),
            Err(StepError::NoValueToRecord),
        ]),
        (At::SetPreRoll, [
            Err(StepError::TooEarly { wait_ms: 1500 }),
            Ok(vec![K::Redo, K::Armed]),
            Ok(vec![K::Skipped { reason: "ui froze".into() }, K::Armed]),
            Err(StepError::Empty),
            Ok(vec![K::ActualValue { value: "-5.5 dB".into() }]),
            Err(StepError::Empty),
        ]),
        (At::SetReady, [
            Ok(vec![K::Done]),
            Ok(vec![K::Redo, K::Armed]),
            Ok(vec![K::Skipped { reason: "ui froze".into() }, K::Armed]),
            Err(StepError::Empty),
            Ok(vec![K::ActualValue { value: "-5.5 dB".into() }]),
            Err(StepError::Empty),
        ]),
        (At::SetDone, [
            Err(StepError::AlreadyDone),
            Ok(vec![K::Redo, K::Armed]),
            Ok(vec![K::Skipped { reason: "ui froze".into() }, K::Armed]),
            Err(StepError::Empty),
            Ok(vec![K::ActualValue { value: "-5.5 dB".into() }]),
            Err(StepError::Empty),
        ]),
        (At::ControlReady, [
            Ok(vec![K::Done]),
            Ok(vec![K::Redo, K::Armed]),
            Ok(vec![K::Skipped { reason: "ui froze".into() }, K::Armed]),
            Err(StepError::Empty),
            Err(StepError::NoValueToRecord),
            Err(StepError::NoValueToRecord),
        ]),
        (At::Completed, std::array::from_fn(|_| Err(StepError::NotRunning))),
        (At::Abandoned, std::array::from_fn(|_| Err(StepError::NotRunning))),
    ];
    for (at, expected) in table {
        for (command, want) in commands.iter().zip(expected) {
            let (mut r, now) = setup(at);
            let got = r.operator(&auth(), command.clone(), now, None).map(|m| kinds(&m));
            assert_eq!(got, want, "state {at:?}, command {command:?}");
        }
    }
}

#[test]
fn redo_discards_the_window_and_counts_attempts() {
    let mut r = run();
    let mut now = T0;
    complete_current(&mut r, &mut now); // idle
    now += 2 * S;
    r.operator(&auth(), OperatorCommand::ActualValue { value: "-1 dB".into() }, now, None).unwrap();
    r.operator(&auth(), OperatorCommand::Done, now, None).unwrap();
    let marks = r.operator(&auth(), OperatorCommand::Redo, now + 100, None).unwrap();
    assert_eq!((marks[0].attempt, marks[1].attempt), (0, 1));
    let step = r.current().unwrap();
    assert_eq!(step.attempt, 1);
    assert_eq!(step.actual_value, None);
    assert_eq!(step.state, StepState::Armed { armed_ns: now + 100 });
}

#[test]
fn skipping_the_last_step_completes_the_probe() {
    let mut r = run();
    let mut now = T0;
    let n = r.steps().len();
    for _ in 0..n - 1 {
        complete_current(&mut r, &mut now);
    }
    let marks = r.operator(&auth(), OperatorCommand::Skip { reason: "done enough".into() }, now, None).unwrap();
    assert_eq!(kinds(&marks), vec![K::Skipped { reason: "done enough".into() }, K::Completed]);
    assert_eq!(r.status(), RunStatus::Completed);
    assert!(r.current().is_none());
}

#[test]
fn abandon_stops_the_run_once() {
    let mut r = run();
    assert_eq!(kinds(&r.abandon(T0 + 1, None).unwrap()), vec![K::Abandoned]);
    assert_eq!(r.status(), RunStatus::Abandoned);
    assert_eq!(r.abandon(T0 + 2, None), Err(StepError::NotRunning));
    assert!(r.tick(T0 + 100 * S, None).is_empty());
}

#[test]
fn clock_divergence_beyond_pre_roll_flags_the_step() {
    let mut r = run();
    let ok = PacketClock { ts_ns: T0, host_ns: T0 + 1_500_000_000 };
    let bad = PacketClock { ts_ns: T0, host_ns: T0 + 1_500_000_001 };
    let marks = r.tick(T0 + 6_500_000_000, Some(ok));
    assert!(!marks[0].clock_suspect);
    assert!(!r.current().unwrap().clock_suspect);
    let marks = r.tick(T0 + 8 * S, Some(bad));
    assert!(marks[0].clock_suspect, "closing mark on step 0");
    assert!(r.steps()[0].clock_suspect);
    // The brief's tautological assert replaced with direct assertions of the real values:
    // the `Armed` mark for step 1 (`marks[1]`) is made with the same `bad` clock, so both
    // the mark and the step should have the flag set.
    assert!(marks[1].clock_suspect);
    assert!(r.steps()[1].clock_suspect);
}

#[test]
fn redo_clears_clock_suspect_for_the_new_attempt() {
    let mut r = run();
    let bad = PacketClock { ts_ns: 0, host_ns: 10 * S };
    let _marks = r.operator(&auth(), OperatorCommand::Redo, T0 + 1, Some(bad)).unwrap();
    // The re-arm mark itself carries the flag; check the intermediate state.
    assert!(r.current().unwrap().clock_suspect, "step should be flagged after redo with bad clock");
    // A clean redo resets it.
    r.operator(&auth(), OperatorCommand::Redo, T0 + 2, None).unwrap();
    assert!(!r.current().unwrap().clock_suspect);
}
