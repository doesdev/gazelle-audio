use gazelle_audio_capture::capture::event::{Direction, TransferType, UrbStage, UsbEvent};
use gazelle_audio_capture::session::marks::{clock_suspect, Mark, MarkKind, PacketClock};
use gazelle_audio_capture::session::model::ProbePlan;
use gazelle_audio_capture::session::plan::expand;
use gazelle_audio_capture::session::step::RunStatus;
use gazelle_audio_capture::session::timeline::{events_in, probe_timelines, StepWindow};

fn plan() -> ProbePlan {
    ProbePlan { parameter: "level".into(), value_a: "0 dB".into(), value_b: vec!["-6 dB".into()], sweep: vec![], repeats: 1, control_parameter: "mute".into() }
}

fn m(step: Option<usize>, attempt: u32, wall_ns: u64, kind: MarkKind) -> Mark {
    Mark { probe: "p1".into(), step, attempt, wall_ns, last_packet: None, clock_suspect: false, kind }
}

#[test]
fn windows_use_the_last_attempt_and_record_flags() {
    use MarkKind::*;
    let mut suspect = m(Some(1), 1, 400, Armed);
    suspect.clock_suspect = true;
    let marks = vec![
        m(None, 0, 0, ProbeStarted { plan: plan(), seed: 9, steps: expand(&plan(), 9) }),
        m(Some(0), 0, 0, Armed),
        m(Some(0), 0, 100, Done),
        m(Some(0), 0, 200, Closed),
        m(Some(1), 0, 200, Armed),
        m(Some(1), 0, 300, ActualValue { value: "-1 dB".into() }),
        m(Some(1), 0, 350, Redo),
        suspect,
        m(Some(1), 1, 500, ActualValue { value: "-0.5 dB".into() }),
        m(Some(1), 1, 600, Done),
        m(Some(1), 1, 700, Closed),
        m(Some(2), 0, 700, Armed),
        m(Some(2), 0, 800, Skipped { reason: "froze".into() }),
        m(None, 0, 800, Completed),
    ];
    let tls = probe_timelines(&marks);
    assert_eq!(tls.len(), 1);
    let tl = &tls[0];
    assert_eq!((tl.probe.as_str(), tl.seed, tl.steps.len(), tl.outcome), ("p1", 9, 7, Some(RunStatus::Completed)));
    assert_eq!(
        tl.windows,
        vec![
            StepWindow { step: 0, attempt: 0, armed_ns: 0, done_ns: 100, closed_ns: 200, actual_value: None, clock_suspect: false },
            StepWindow { step: 1, attempt: 1, armed_ns: 400, done_ns: 600, closed_ns: 700, actual_value: Some("-0.5 dB".into()), clock_suspect: true },
        ]
    );
    assert_eq!(tl.windows[1].action(), 400..600);
    assert_eq!(tl.windows[1].settle(), 600..700);
    assert_eq!(tl.skipped, vec![(2, "froze".to_string())]);
    assert_eq!(tl.redos.get(&1), Some(&1));
}

#[test]
fn unfinished_probe_has_no_outcome() {
    let marks = vec![m(None, 0, 0, MarkKind::ProbeStarted { plan: plan(), seed: 1, steps: vec![] }), m(Some(0), 0, 0, MarkKind::Armed)];
    let tl = &probe_timelines(&marks)[0];
    assert_eq!((tl.outcome, tl.windows.len()), (None, 0));
}

#[test]
fn events_in_is_half_open() {
    let ev = |ts_ns| UsbEvent {
        ts_ns,
        packet_index: 0,
        bus: 1,
        device: 1,
        endpoint: 1,
        direction: Direction::In,
        transfer: TransferType::Interrupt,
        stage: UrbStage::Complete,
        urb_id: 0,
        setup: None,
        status: 0,
        data_len: 0,
        data: vec![],
        payload_dropped: false,
    };
    let events: Vec<UsbEvent> = [10, 20, 20, 30, 40].into_iter().map(ev).collect();
    let ts = |r: &[UsbEvent]| r.iter().map(|e| e.ts_ns).collect::<Vec<_>>();
    assert_eq!(ts(events_in(&events, 20..40)), vec![20, 20, 30]);
    assert_eq!(ts(events_in(&events, 41..50)), Vec::<u64>::new());
    assert_eq!(ts(events_in(&events, std::ops::Range { start: 30, end: 20 })), Vec::<u64>::new());
}

#[test]
fn clock_check_compares_packet_and_host_clocks() {
    assert!(!clock_suspect(None, 10));
    assert!(!clock_suspect(Some(PacketClock { ts_ns: 100, host_ns: 110 }), 10));
    assert!(clock_suspect(Some(PacketClock { ts_ns: 100, host_ns: 111 }), 10));
    assert!(clock_suspect(Some(PacketClock { ts_ns: 111, host_ns: 100 }), 10));
}
