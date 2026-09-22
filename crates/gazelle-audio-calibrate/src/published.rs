//! What a run publishes while it goes, and what it leaves behind afterwards.
//!
//! Nothing here opens a driver, maps a real section or writes a real file. The aggregate is the
//! real one, over the aggregate crate's devices made of data; the shared section is a run of
//! memory with a reader over it, and the event log is a list.
//!
//! **Why this matters more than it looks.** The driver's phase measurement reports what it did
//! only through the record and the log. A calibration run that published neither was the one
//! session in which nobody could tell whether the phase had been measured, refused, or applied and
//! simply not helped, which is what the first run of it at the hardware met.

use std::cell::RefCell;
use std::sync::{Arc, Mutex};

use gazelle_aggregate::config::{Alignment, Config, DeviceConfig, PhaseConfig};
use gazelle_aggregate::delay::Delay;
use gazelle_aggregate::fake::{FakeDevice, FakeHost, FakePc, Spec};
use gazelle_aggregate::phase;
use gazelle_aggregate::status::{Clock, LogSink, Reporter};
use gazelle_aggregate::sub::Host;
use gazelle_audio_aggregate_status::map::Scratch;
use gazelle_audio_aggregate_status::record::phase as codes;
use gazelle_audio_aggregate_status::{Publisher, Reader, Snapshot};

use crate::rig::{Direction, Pick, Rig, Settings};
use crate::session::{measure_reporting, Outcome, RunLog, MARK};
use crate::trim::PhaseReference;

const BLOCK: i32 = 64;
const RATE: f64 = 48_000.0;

/// What both interfaces report each way, which is what a phase measurement is residual to.
const LATENCY: i32 = 100;

/// The interface cabled late for the click, in samples, as in the end to end test.
const CABLED_LATE: usize = 28;

/// The phase B's trim was last measured at, which is already in the file when a run starts. A run
/// never lines itself up to it: it is what the run replaces.
const OLD_REFERENCE: i32 = -84;

/// Two interfaces with three channels each way: two for the click, and a third on each for the
/// driver's own phase measurement, which the DAW's list, and so this run's, never sees.
fn two_interfaces() -> Arc<FakePc> {
    let spec = || {
        Spec { min: 8, max: 4096, preferred: BLOCK, rate: RATE, rates: vec![RATE], ..Spec::default() }
            .with_channels(3, 3)
            .with_latency(LATENCY, LATENCY)
    };
    Arc::new(
        FakePc::new()
            .with("Device A", "{AAAAAAAA-0000-0000-0000-000000000001}", r"c:\antelope\a.dll", spec())
            .with("Device B", "{BBBBBBBB-0000-0000-0000-000000000002}", r"c:\antelope\b.dll", spec()),
    )
}

/// Each interface exposes its first two channels, so the aggregate's are A's 0 and 1 and then
/// B's 0 and 1, whether or not B's phase is measured over its third. `phased` gives B the cable
/// from A's third output into its own third input, and the reference its trim was last measured at.
fn config(phased: bool) -> Config {
    let exposed = |key: &str, name: &str| DeviceConfig {
        key: Some(key.into()),
        name: Some(name.into()),
        inputs: Some(vec![0, 1]),
        outputs: Some(vec![0, 1]),
        ..DeviceConfig::default()
    };
    let mut b = exposed("Device B", "B");
    if phased {
        b.phase = Some(PhaseConfig { master_output: Some(2), input: Some(2), reference: Some(OLD_REFERENCE) });
    }
    Config { devices: vec![exposed("Device A", "A"), b], alignment: Alignment::Aligned, ..Config::default() }
}

/// A's two outputs, one into A's first input and one into B's first input.
fn rig() -> Rig {
    Rig::new(Direction::Inputs, vec![Pick::new(0, 0), Pick::new(0, 1)], vec![Pick::new(0, 0), Pick::new(1, 0)])
}

/// Long enough to settle for longer than the driver listens for its phase signal, so that every
/// measurement it starts has come to something before the first click is played, which is how a
/// real run is too: a quarter of a second of listening inside half a second of settling.
fn settings() -> Settings {
    Settings {
        clicks: 4,
        spacing_seconds: 0.02,
        settle_seconds: 0.3,
        click_samples: 64,
        search_samples: 256,
        buffer_size: Some(BLOCK),
        ..Settings::default()
    }
}

/// An event log made of a list.
#[derive(Clone, Default)]
struct Written(Arc<Mutex<Vec<String>>>);

impl LogSink for Written {
    fn append(&mut self, line: &str) {
        self.0.lock().expect("not poisoned").push(line.to_string());
    }
}

impl Written {
    fn lines(&self) -> Vec<String> {
        self.0.lock().expect("not poisoned").clone()
    }

    /// The lines of one kind.
    fn of(&self, word: &str) -> Vec<String> {
        self.lines().into_iter().filter(|line| line.split(' ').nth(2) == Some(word)).collect()
    }
}

/// A clock that has stopped, so that every line is the same every time it is written.
struct Stopped;

impl Clock for Stopped {
    fn stamp(&self) -> String {
        "2026-09-21 23:40:12".to_string()
    }
}

/// A reporter over a run of memory, marking its lines as a run's own exactly as the real one does,
/// with the reader for that memory and the list the log is written to.
fn published() -> (Arc<Reporter>, Reader, Written) {
    let scratch = Scratch::new();
    let publisher = Publisher::map(Box::new(scratch.clone())).expect("a scratch section");
    let reader = Reader::map(Box::new(scratch.opened())).expect("the same one");
    let written = Written::default();
    let log: Box<dyn LogSink> = Box::new(RunLog::marking(Box::new(written.clone())));
    (Arc::new(Reporter::new(Some(publisher), Some(log), Box::new(Stopped))), reader, written)
}

/// One cable: what went into it comes out this many samples later.
struct Cable {
    line: Delay,
    carrying: Vec<i32>,
}

impl Cable {
    fn new(samples: usize) -> Cable {
        Cable { line: Delay::new(1, samples), carrying: vec![0i32; BLOCK as usize] }
    }

    fn carry(&mut self, block: Vec<i32>) {
        let mut going = block;
        self.line.process(&mut going, BLOCK as usize);
        self.carrying = going;
    }
}

/// Where the driver's phase signal has to arrive for its measurement to come to `residual`
/// samples, as a block and a place in it.
///
/// It leaves on the block the driver settles on, held back by what A's own inputs are held back
/// by, which is the one block B's ring costs; what the drivers' figures expect of it is A's
/// reported output latency, B's reported input latency, and that block again.
fn arrives_at(residual: i64) -> (usize, usize) {
    let sent_at = phase::SETTLE_BLOCKS as i64 * BLOCK as i64 + BLOCK as i64;
    let expected = 2 * LATENCY as i64 + BLOCK as i64;
    let position = sent_at + expected + residual;
    ((position / BLOCK as i64) as usize, (position % BLOCK as i64) as usize)
}

/// Run the measurement with this reporter. `phase_residual` is where the phase signal lands on B's
/// measurement channel, or `None` for a cable that carries nothing back. `during` is handed the
/// block number between blocks, which is how a test looks at the record while the run is going.
fn run(
    phased: bool,
    phase_residual: Option<i64>,
    reporter: Arc<Reporter>,
    during: &mut dyn FnMut(usize),
) -> Outcome {
    run_with(phased, phase_residual, reporter, during, &settings())
}

/// [`run`], with the run's settings given, which is how a check differs from a measurement.
fn run_with(
    phased: bool,
    phase_residual: Option<i64>,
    reporter: Arc<Reporter>,
    during: &mut dyn FnMut(usize),
    settings: &Settings,
) -> Outcome {
    let pc = two_interfaces();
    let host: Box<dyn Host> = Box::new(FakeHost { pc: Arc::clone(&pc) });
    let (a, b): (Arc<FakeDevice>, Arc<FakeDevice>) = (pc.device("Device A"), pc.device("Device B"));
    let mut to_a = Cable::new(0);
    let mut to_b = Cable::new(CABLED_LATE);
    let arrival = phase_residual.map(arrives_at);

    let mut pump = |index: usize| {
        let half = index & 1;
        a.set_input(0, half, &to_a.carrying);
        b.set_input(0, half, &to_b.carrying);
        if phased {
            // B's measurement channel is the last one it opened, because the file named a channel
            // outside the two it exposes.
            let mut heard = vec![0i32; BLOCK as usize];
            if let Some((at, offset)) = arrival {
                if at == index {
                    heard[offset] = phase::AMPLITUDE;
                }
            }
            b.set_input(2, half, &heard);
        }
        b.fire(half);
        a.fire(half);
        to_a.carry(a.output(0, half));
        to_b.carry(a.output(1, half));
        during(index);
        true
    };
    measure_reporting(host, config(phased), "a test".to_string(), &rig(), settings, reporter, &mut pump)
}

/// **While a run is going, the record says what it says for a DAW's session**, and a reader can
/// read it: the interfaces, the plan, the buffers open, the audio running and the blocks going by.
#[test]
fn a_run_publishes_a_record_that_can_be_read_while_it_is_going() {
    let (reporter, reader, _written) = published();
    let seen: RefCell<Option<Snapshot>> = RefCell::new(None);
    let outcome = run(true, Some(32), reporter, &mut |index| {
        if index == 40 {
            *seen.borrow_mut() = Some(reader.read().expect("a settled record"));
        }
    });
    assert_eq!(outcome.refusal, None, "{:?}", outcome.refusal);

    let seen = seen.into_inner().expect("the run was looked at part way");
    assert_eq!(seen.driver.open, 1, "the buffers were made");
    assert_eq!(seen.driver.streaming, 1, "and the audio was running");
    assert_eq!(seen.driver.sample_rate, RATE);
    assert_eq!(seen.driver.buffer_size, BLOCK);
    assert!(seen.driver.callbacks > 0, "blocks were going by: {}", seen.driver.callbacks);
    assert_eq!(seen.driver.daw_inputs, 2, "the two channels the run records");
    let devices = seen.devices();
    assert_eq!(devices.len(), 2);
    assert_eq!((devices[0].name.get(), devices[1].name.get()), ("A", "B"));
    assert!(devices.iter().all(|device| device.streaming == 1));
    // And what the phase measurement had come to by then, which is the number nobody could see:
    // measured, and on purpose not applied.
    assert_eq!(devices[0].phase_state, codes::NOT_CONFIGURED);
    assert_eq!(devices[1].phase_state, codes::MEASURED_ONLY);
    assert_eq!(devices[1].phase_measured, 32);
    assert_eq!(devices[1].phase_applied, 0);

    // Once it is over the record says nothing is open, exactly as it does when a DAW lets go.
    let after = reader.read().expect("a settled record");
    assert_eq!((after.driver.open, after.driver.streaming), (0, 0));
}

/// **Afterwards, the log keeps the run's own lines**, and every one of them says it was Gazelle
/// measuring, so nobody reading the file tomorrow takes it for a DAW's session.
#[test]
fn the_log_keeps_the_runs_lines_afterwards_and_each_says_whose_they_are() {
    let (reporter, _reader, written) = published();
    let outcome = run(true, Some(32), reporter, &mut |_| {});
    assert_eq!(outcome.refusal, None, "{:?}", outcome.refusal);

    let started = written.of("session-started");
    assert_eq!(started.len(), 1, "{:?}", written.lines());
    assert_eq!(started[0], format!("2026-09-21 23:40:12 session-started {MARK} 2 in, 2 out at 48000 Hz"));

    let phases = written.of("phase");
    assert_eq!(phases.len(), 1, "one line for the one interface that was measured: {:?}", written.lines());
    assert!(phases[0].contains(&format!("phase {MARK} B was measured at 32 samples")), "{}", phases[0]);
    assert!(phases[0].contains("left there on purpose"), "{}", phases[0]);

    let ended = written.of("session-ended");
    assert_eq!(ended.len(), 1);
    assert!(ended[0].contains(&format!("session-ended {MARK} ran for")), "{}", ended[0]);
    assert!(ended[0].contains("no interface lost a block"), "{}", ended[0]);

    // Every line, not only the ones looked at above, and every one still a line the log's own
    // reader takes apart.
    for line in written.lines() {
        assert!(line.contains(MARK), "{line}");
        assert!(gazelle_audio_aggregate_status::events::parse(&line).is_some(), "{line}");
    }
}

/// A run somebody's cabling refused is published too: the page watching a run has the same
/// question about one that never started.
#[test]
fn a_run_that_is_refused_says_so_in_the_record_and_the_log() {
    let (reporter, reader, written) = published();
    let pc = two_interfaces();
    let host: Box<dyn Host> = Box::new(FakeHost { pc: Arc::clone(&pc) });
    let spread = Rig::new(Direction::Inputs, vec![Pick::new(0, 0), Pick::new(1, 0)], vec![Pick::new(0, 0), Pick::new(1, 0)]);
    let mut pump = |_: usize| panic!("a refused rig never runs a block");
    let outcome = measure_reporting(host, config(false), "a test".to_string(), &spread, &settings(), reporter, &mut pump);
    let refusal = outcome.refusal.expect("the outputs are on two interfaces");
    assert_eq!(reader.read().expect("a settled record").driver.refusal.get(), refusal);
    let said = written.of("refused");
    assert_eq!(said.len(), 1, "{:?}", written.lines());
    assert!(said[0].contains(MARK) && said[0].contains(&refusal), "{}", said[0]);
}

/// **Nowhere to publish is not a reason to fail.** A run whose section could not be made, and one
/// with nothing at all to report to, measure exactly what a published run measures.
#[test]
fn a_run_with_nowhere_to_publish_still_measures_and_answers_normally() {
    let (reporter, _reader, _written) = published();
    let watched = run(true, Some(32), reporter, &mut |_| {});
    assert_eq!(watched.refusal, None, "{:?}", watched.refusal);

    // A section too small to hold a record, which is what failing to make one looks like, and a
    // log that still works.
    let too_small = Publisher::map(Box::new(Scratch::of(64))).ok();
    assert!(too_small.is_none(), "the point of this one is that the mapping failed");
    let written = Written::default();
    let unmapped = Arc::new(Reporter::new(too_small, Some(Box::new(RunLog::marking(Box::new(written.clone())))), Box::new(Stopped)));
    let unwatched = run(true, Some(32), unmapped, &mut |_| {});

    // And nothing at all.
    let silent = run(true, Some(32), Arc::new(Reporter::silent()), &mut |_| {});

    for outcome in [&unwatched, &silent] {
        assert_eq!(outcome.refusal, None, "{:?}", outcome.refusal);
        assert_eq!(outcome.readings, watched.readings, "the same measurement");
        assert_eq!(outcome.trims, watched.trims, "and the same trims");
        assert!(outcome.is_measured() && outcome.was_clean());
        assert!(outcome.phases.is_empty(), "no record to read is no phase, not a phase of nothing");
        // The trim is still paired with the phase it was measured at, because that is read from
        // the audio path and not from the record.
        assert_eq!(outcome.trims[1].phase_reference, Some(PhaseReference { old: Some(OLD_REFERENCE), new: Some(32) }));
    }
    // The log still had somewhere to go, so the run's own lines are still in it.
    assert_eq!(written.of("session-started").len(), 1, "{:?}", written.lines());
    assert!(written.of("phase").is_empty(), "the phase line is read from the record, and there was none");
}

/// **A run measures the phase and does not apply it, and offers the trim paired with it.** The
/// file already has a reference from the last time; this run lands 64 samples earlier than that,
/// and lines nothing up to it, so the lag it measures is the raw one and the new trim and the
/// phase it was measured at are offered together.
#[test]
fn a_run_measures_the_phase_without_applying_it_and_offers_the_trim_paired_with_it() {
    let (reporter, _reader, _written) = published();
    let outcome = run(true, Some(-148), reporter, &mut |_| {});
    assert_eq!(outcome.refusal, None, "{:?}", outcome.refusal);
    assert_eq!(outcome.phases.len(), 2, "one per interface");

    let master = &outcome.phases[0];
    assert_eq!((master.device.as_str(), master.state), ("A", "not_configured"));
    assert_eq!((master.measured_samples, master.applied_samples), (0, 0));

    let measured = &outcome.phases[1];
    assert_eq!((measured.device.as_str(), measured.state), ("B", "measured_only"));
    assert_eq!((measured.measured_samples, measured.applied_samples), (-148, 0));
    assert!(!measured.was_applied() && !measured.was_refused());
    assert!(measured.note.contains("on purpose"), "{}", measured.note);
    assert_eq!(outcome.phases_refused().count(), 0);

    // Nothing was moved: the clicks measure exactly what the same cabling measures with no phase
    // set up at all, whatever the phase came to and whatever the old reference was. That is what
    // makes the lag the raw one, and so the right thing to pair with the phase beside it.
    let (reporter, _reader, _written) = published();
    let unphased = run(false, None, reporter, &mut |_| {});
    assert_eq!(outcome.readings, unphased.readings);
    let (trim, raw) = (&outcome.trims[1], &unphased.trims[1]);
    assert_eq!((trim.old, trim.measured, trim.new), (raw.old, raw.measured, raw.new));
    assert!(trim.not_applied.is_none(), "the trim is offered: {trim:?}");
    assert_eq!(
        trim.phase_reference,
        Some(PhaseReference { old: Some(OLD_REFERENCE), new: Some(-148) }),
        "beside the phase it was measured at, replacing the old pair whole"
    );
    assert_eq!(outcome.trims[0].phase_reference, None, "the interface everything was measured against has none");
    assert_eq!(raw.phase_reference, None, "and an interface with no phase setting has none either");
}

/// **A check is lined up as a DAW's session is**, which is the whole difference from a
/// measurement: the phase is applied from its reference, so the lag it hears is the lag a
/// recording would get, and it offers nothing to write, because a lag heard on top of a correction
/// is a verdict on the trim and not a new one.
#[test]
fn a_check_lines_the_session_up_as_a_daw_would_and_offers_no_trim() {
    let checking = Settings { checking: true, ..settings() };
    let (reporter, _reader, _written) = published();
    let check = run_with(true, Some(-148), reporter, &mut |_| {}, &checking);
    assert_eq!(check.refusal, None, "{:?}", check.refusal);
    assert!(check.checking, "it says it was a check");
    assert!(check.trims.is_empty(), "and offers nothing to write: {:?}", check.trims);

    let phase = &check.phases[1];
    assert_eq!(phase.state, "applied", "{}", phase.note);
    assert_eq!(phase.applied_samples, -148 - OLD_REFERENCE, "exactly the measured phase minus its reference");

    // The same session measured rather than checked moves nothing, so the difference between the
    // two lags is the correction and nothing else.
    let (reporter, _reader, _written) = published();
    let measured = run(true, Some(-148), reporter, &mut |_| {});
    assert!(!measured.checking);
    let moved = (check.readings[1].lag_samples - measured.readings[1].lag_samples).abs();
    assert!(
        (moved - phase.applied_samples.abs() as f64).abs() < 0.5,
        "the check moved B's lag by {moved} samples, not by the {} applied",
        phase.applied_samples
    );
}

#[test]
fn an_interface_the_setup_never_asked_to_measure_says_so() {
    let (reporter, _reader, written) = published();
    let outcome = run(false, None, reporter, &mut |_| {});
    assert_eq!(outcome.refusal, None, "{:?}", outcome.refusal);
    assert_eq!(outcome.phases.len(), 2);
    assert!(outcome.phases.iter().all(|phase| phase.state == "not_configured"), "{:?}", outcome.phases);
    assert!(outcome.phases.iter().all(|phase| phase.measured_samples == 0 && phase.applied_samples == 0));
    assert_eq!(outcome.phases_refused().count(), 0, "nothing asked for is nothing turned down");
    assert!(written.of("phase").is_empty(), "and there is nothing to say about it: {:?}", written.lines());
}

#[test]
fn an_interface_whose_phase_was_refused_says_why_and_that_it_ran_on_the_drivers_figures() {
    let (reporter, reader, written) = published();
    // The cable carries nothing back, so the driver hears nothing inside its window.
    let outcome = run(true, None, reporter, &mut |_| {});
    assert_eq!(outcome.refusal, None, "a refused phase never refuses the run: {:?}", outcome.refusal);

    let refused = &outcome.phases[1];
    assert_eq!(refused.state, "not_heard");
    assert!(refused.was_refused());
    assert_eq!(refused.applied_samples, 0, "a refusal to correct, not a correction of zero");
    assert!(refused.note.contains("B was not lined up"), "{}", refused.note);
    assert!(refused.note.contains("the figures the drivers reported"), "{}", refused.note);
    assert_eq!(outcome.phases_refused().map(|phase| phase.device.as_str()).collect::<Vec<_>>(), vec!["B"]);

    // The log says the same, marked as a run's own, and the run's measurement still stands.
    let said = written.of("phase");
    assert_eq!(said.len(), 1, "{:?}", written.lines());
    assert!(said[0].contains(MARK) && said[0].contains("nothing arrived"), "{}", said[0]);
    assert!(outcome.is_measured(), "{:?}", outcome.trims);
    assert_eq!(reader.read().unwrap().devices()[1].phase_state, codes::NOT_HEARD);

    // Nothing was moved, so the clicks measure exactly what the same cabling measures with no
    // phase set up at all, which is what "ran on the figures the drivers reported" means.
    let (reporter, _reader, _written) = published();
    let unphased = run(false, None, reporter, &mut |_| {});
    assert_eq!(outcome.readings, unphased.readings);
    assert_eq!(outcome.trims[1].new, unphased.trims[1].new);
    // The trim is still offered, and the old reference goes with the old trim: there is no phase
    // to pair the new one with, so its sessions will run on the drivers' figures until a run hears
    // one.
    assert_eq!(outcome.trims[1].phase_reference, Some(PhaseReference { old: Some(OLD_REFERENCE), new: None }));
}

/// A line this build cannot take apart is kept exactly as it was rather than mangled.
#[test]
fn a_run_marks_its_lines_and_leaves_a_line_it_cannot_read_alone() {
    assert_eq!(
        RunLog::marked("2026-09-21 23:40:12 session-ended"),
        format!("2026-09-21 23:40:12 session-ended {MARK}")
    );
    assert_eq!(
        RunLog::marked("2026-09-21 23:40:12 stalled B"),
        format!("2026-09-21 23:40:12 stalled {MARK} B")
    );
    assert_eq!(RunLog::marked("not a line of the log"), "not a line of the log");
}
