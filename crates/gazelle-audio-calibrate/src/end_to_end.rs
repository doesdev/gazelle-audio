//! The whole measurement, against the aggregate's own devices made of data.
//!
//! Nothing here opens a driver or drives a converter. The aggregate is the real one: the real
//! plan, the real padding, the real rings and delays, the real audio path. What stands in for the
//! hardware is the aggregate crate's fakes, plus a cable made of a delay line, and the delay put
//! into that cable is the number this test expects to get back out.
//!
//! **This is the test that proves the sign convention.** One interface is cabled twenty eight
//! samples late on purpose. The first run measures it. The second run puts the trim the first run
//! asked for into the configuration, changes nothing else, and measures again: the offset is gone.
//! A trim of the wrong sign would have doubled it, which is what happened by hand on 2026-09-21.

use std::sync::Arc;

use gazelle_aggregate::config::{Alignment, Config, DeviceConfig, PhaseConfig};
use gazelle_aggregate::delay::Delay;
use gazelle_aggregate::fake::{FakeDevice, FakeHost, FakePc, Spec, Takes};
use gazelle_aggregate::sub::Host;

use crate::rig::{Direction, Pick, Rig, Settings};
use crate::session::{measure_against, Outcome};

const BLOCK: i32 = 64;
const RATE: f64 = 48_000.0;

/// The interface that is late, in samples. The number the run must give back.
const CABLED_LATE: i32 = 28;

/// Two interfaces, each two channels in and out.
///
/// "A" is first, so it is the master and its audio does not cross a ring. "B"'s does, which costs
/// it a whole buffer, so its driver is given a buffer less reported latency: that leaves the two
/// paths the same length with no trims at all, which is the state a real pair is in before
/// anything is measured.
fn two_interfaces() -> Arc<FakePc> {
    Arc::new(
        FakePc::new()
            .with("Device A", "{AAAAAAAA-0000-0000-0000-000000000001}", r"c:\antelope\a.dll", spec(600 + BLOCK))
            .with("Device B", "{BBBBBBBB-0000-0000-0000-000000000002}", r"c:\antelope\b.dll", spec(600)),
    )
}

fn spec(latency_in: i32) -> Spec {
    Spec {
        min: 8,
        max: 4096,
        preferred: BLOCK,
        rate: RATE,
        rates: vec![44_100.0, 48_000.0, 96_000.0],
        ..Spec::default()
    }
    .with_channels(2, 2)
    .with_latency(latency_in, 700)
}

/// The configuration a person would have, with whatever trims are already in it.
fn config(trims: [i32; 2]) -> Config {
    Config {
        devices: vec![
            DeviceConfig {
                key: Some("Device A".into()),
                name: Some("A".into()),
                input_trim: Some(trims[0]),
                ..DeviceConfig::default()
            },
            DeviceConfig {
                key: Some("Device B".into()),
                name: Some("B".into()),
                input_trim: Some(trims[1]),
                ..DeviceConfig::default()
            },
        ],
        alignment: Alignment::Aligned,
        ..Config::default()
    }
}

/// The cabling of the README, each end named by interface and that interface's own channel: A's two
/// outputs, one into A's first input and one into B's first input.
fn rig() -> Rig {
    Rig::new(Direction::Inputs, vec![Pick::new(0, 0), Pick::new(0, 1)], vec![Pick::new(0, 0), Pick::new(1, 0)])
}

fn settings() -> Settings {
    Settings {
        clicks: 4,
        spacing_seconds: 0.02,
        settle_seconds: 0.01,
        click_samples: 64,
        search_samples: 256,
        buffer_size: Some(BLOCK),
        ..Settings::default()
    }
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

    /// Put one block into the cable and take out whatever reaches the far end.
    fn carry(&mut self, block: Vec<i32>) {
        let mut going = block;
        self.line.process(&mut going, BLOCK as usize);
        self.carrying = going;
    }
}

/// Run the whole measurement with the two interfaces cabled as the README describes, `late`
/// samples of extra cable on the one going to B, and `trims` already in the configuration.
fn measured(trims: [i32; 2], late: usize, plugged: [bool; 2]) -> Outcome {
    let pc = two_interfaces();
    let host: Box<dyn Host> = Box::new(FakeHost { pc: Arc::clone(&pc) });
    let (a, b): (Arc<FakeDevice>, Arc<FakeDevice>) = (pc.device("Device A"), pc.device("Device B"));
    // The cable into A has nothing in it, and the one into B is the fault being measured. Both
    // leave the same output device on the same sample, which is what makes the difference between
    // them the interfaces and nothing else.
    let mut to_a = Cable::new(0);
    let mut to_b = Cable::new(late);

    let mut pump = |index: usize| {
        let half = index & 1;
        // What the cables are carrying arrives at the converters before they call back.
        if plugged[0] {
            a.set_input(0, half, &to_a.carrying);
        }
        if plugged[1] {
            b.set_input(0, half, &to_b.carrying);
        }
        // The interface that is not driving the callback hands its block over first, as it does
        // in the driver's own tests and at the hardware.
        b.fire(half);
        a.fire(half);
        // And what was played goes into the cables.
        to_a.carry(a.output(0, half));
        to_b.carry(a.output(1, half));
        true
    };

    measure_against(host, config(trims), "a test".to_string(), &rig(), &settings(), &mut pump)
}

/// The same two interfaces, with B's **second** input carried along as a witness and cabled to a
/// delay of its own.
///
/// This is the shape of the question the hardware asks: one interface, two of its inputs, one run.
/// Both cables are a split of the same output of A, so both copies of the click still leave on the
/// same sample and the only difference between them is the cable. `witness_late` of `None` is a
/// witness with nothing plugged into it.
fn measured_watching(late: usize, witness_late: Option<usize>) -> Outcome {
    let pc = two_interfaces();
    let host: Box<dyn Host> = Box::new(FakeHost { pc: Arc::clone(&pc) });
    let (a, b): (Arc<FakeDevice>, Arc<FakeDevice>) = (pc.device("Device A"), pc.device("Device B"));
    let mut to_a = Cable::new(0);
    let mut to_b = Cable::new(late);
    let mut to_witness = Cable::new(witness_late.unwrap_or(0));

    let mut pump = |index: usize| {
        let half = index & 1;
        a.set_input(0, half, &to_a.carrying);
        b.set_input(0, half, &to_b.carrying);
        if witness_late.is_some() {
            b.set_input(1, half, &to_witness.carrying);
        }
        b.fire(half);
        a.fire(half);
        to_a.carry(a.output(0, half));
        // A's second output splits: one leg into B's first input, one into B's second.
        let played = a.output(1, half);
        to_b.carry(played.clone());
        to_witness.carry(played);
        true
    };

    // B's first input is the one being measured, and its second is carried along.
    let rig = rig().watching(vec![Pick::new(1, 1)]);
    measure_against(host, config([0, 0]), "a test".to_string(), &rig, &settings(), &mut pump)
}

/// Two inputs of one interface, cabled to different delays, come back as two numbers.
///
/// **This is the test that stands behind the hardware question.** If an interface's inputs could
/// only ever be reported as one number, watching its S/PDIF input beside its analogue input would
/// prove nothing. Here the two are given delays of 28 and 9 samples in the same run, and each is
/// reported as what it was, while the trims stay exactly what the measured channel alone says.
#[test]
fn two_inputs_of_one_interface_are_measured_in_one_run_and_reported_apart() {
    const WITNESS_LATE: usize = 9;
    let outcome = measured_watching(CABLED_LATE as usize, Some(WITNESS_LATE));
    assert_eq!(outcome.refusal, None, "{:?}", outcome.refusal);

    assert_eq!(outcome.readings.len(), 2, "a witness is never an interface");
    let cabled_late = &outcome.readings[1];
    assert!((cabled_late.lag_samples - CABLED_LATE as f64).abs() < 0.5, "B's measured input: {cabled_late:?}");

    assert_eq!(outcome.witnesses.len(), 1);
    let witness = &outcome.witnesses[0];
    assert_eq!(witness.channel, 1, "it carries the channel it was, as B numbers it");
    assert_eq!(witness.device, "B", "and which interface that channel is on");
    assert!(
        (witness.reading.lag_samples - WITNESS_LATE as f64).abs() < 0.5,
        "the witness is its own cable, not its interface's: {:?}",
        witness.reading
    );
    assert_eq!(witness.reading.clicks_found, 4);
    assert!(witness.reading.spread_samples < 0.5);
    assert!(witness.reading.note.contains("input 2 on B"), "{}", witness.reading.note);

    // And nothing about the witness reached the trims.
    let plain = measured([0, 0], CABLED_LATE as usize, [true, true]);
    assert_eq!(outcome.trims, plain.trims, "the trims are what they would have been with no witness at all");
    assert_eq!(outcome.trims[1].measured, CABLED_LATE);
    assert!(outcome.is_measured() && outcome.was_clean());
}

#[test]
fn a_witness_with_nothing_on_it_says_so_and_spoils_nothing() {
    let outcome = measured_watching(CABLED_LATE as usize, None);
    assert_eq!(outcome.refusal, None, "a silent witness is never a refusal");
    let witness = &outcome.witnesses[0];
    assert!(witness.reading.nothing_arrived, "{:?}", witness.reading);
    assert_eq!(witness.reading.clicks_found, 0);
    assert!(witness.reading.note.contains("cable"), "{}", witness.reading.note);
    // The run itself is untouched: the interfaces were measured and the trim is offered.
    assert!((outcome.readings[1].lag_samples - CABLED_LATE as f64).abs() < 0.5);
    assert_eq!(outcome.trims[1].measured, CABLED_LATE);
    assert!(outcome.trims[1].not_applied.is_none());
    assert!(outcome.is_measured(), "nothing arrived on an observation, which is not a measurement going wrong");
    assert!(outcome.was_clean());
}

#[test]
fn a_witness_the_aggregate_has_no_channel_for_is_refused_and_nothing_is_played() {
    let pc = two_interfaces();
    let host: Box<dyn Host> = Box::new(FakeHost { pc: Arc::clone(&pc) });
    let watching_nothing = rig().watching(vec![Pick::new(1, 9)]);
    let mut pump = |_: usize| panic!("a refused rig never runs a block");
    let outcome =
        measure_against(host, config([0, 0]), "a test".to_string(), &watching_nothing, &settings(), &mut pump);
    let refusal = outcome.refusal.expect("B has two inputs");
    assert!(refusal.contains("B has 2 inputs, numbered 1 to 2") && refusal.contains("listen in on"), "{refusal}");
    assert!(!pc.device("Device A").is_started(), "nothing was started");
    assert!(!pc.device("Device B").has_buffers(), "and no buffers were made");
}

#[test]
fn an_interface_cabled_late_is_measured_and_the_trim_that_nulls_it_is_positive() {
    let outcome = measured([0, 0], CABLED_LATE as usize, [true, true]);
    assert_eq!(outcome.refusal, None, "{:?}", outcome.refusal);
    assert_eq!(outcome.rate, RATE);
    assert_eq!(outcome.block, BLOCK);

    let (reference, late) = (&outcome.readings[0], &outcome.readings[1]);
    assert_eq!(reference.device, "A");
    assert_eq!(late.device, "B");
    assert!(reference.lag_samples.abs() < 0.01, "the reference is zero against itself: {reference:?}");
    assert!(
        (late.lag_samples - CABLED_LATE as f64).abs() < 0.5,
        "twenty eight samples of cable is twenty eight samples of lag: {late:?}"
    );
    assert_eq!(late.clicks_found, 4, "every click was found: {late:?}");
    assert!(late.spread_samples < 0.5, "and they all agreed: {late:?}");
    assert_eq!(late.drift, None, "a fixed cable does not drift");

    // The sign rule, end to end: the interface that landed later takes a positive trim.
    let trim = &outcome.trims[1];
    assert_eq!(trim.field, "input_trim");
    assert_eq!((trim.old, trim.measured, trim.new), (0, CABLED_LATE, CABLED_LATE));
    assert!(trim.not_applied.is_none());
    assert!(outcome.trims[0].is_reference, "the reference's trim never moves");
    assert_eq!(outcome.trims[0].new, 0);
    assert!(outcome.is_measured());
}

#[test]
fn putting_that_trim_into_the_file_nulls_the_offset_and_a_second_run_finds_nothing_left() {
    // The whole point of the sign rule. Nothing is changed but the trim the first run asked for.
    let outcome = measured([0, CABLED_LATE], CABLED_LATE as usize, [true, true]);
    assert_eq!(outcome.refusal, None);
    let left_over = &outcome.readings[1];
    assert!(left_over.lag_samples.abs() < 0.5, "the trim cancelled it: {left_over:?}");

    // And the file keeps the trim it already had, because there is nothing left to add to it.
    let trim = &outcome.trims[1];
    assert_eq!((trim.old, trim.measured, trim.new), (CABLED_LATE, 0, CABLED_LATE));
}

#[test]
fn a_trim_of_the_wrong_sign_would_have_doubled_the_error_which_is_what_this_proves() {
    // Written down because it is what happened by hand: minus twenty eight made twenty eight
    // samples into fifty three. This runs the wrong sign on purpose and shows the offset growing.
    let outcome = measured([0, -CABLED_LATE], CABLED_LATE as usize, [true, true]);
    assert_eq!(outcome.refusal, None);
    let worse = outcome.readings[1].lag_samples;
    assert!(worse > 1.5 * CABLED_LATE as f64, "the wrong sign moves it further away, not nearer: {worse}");
}

/// A run whose audio went wrong underneath it. The interface that is not driving the callback
/// hands its blocks over twice as fast as they can be taken, so its ring fills and throws them
/// away, exactly as it does when a PC cannot keep up at the buffer size it was given.
#[test]
fn a_run_the_audio_went_wrong_under_says_so_and_offers_no_trim() {
    let pc = two_interfaces();
    let host: Box<dyn Host> = Box::new(FakeHost { pc: Arc::clone(&pc) });
    let (a, b): (Arc<FakeDevice>, Arc<FakeDevice>) = (pc.device("Device A"), pc.device("Device B"));
    let mut to_a = Cable::new(0);
    let mut to_b = Cable::new(CABLED_LATE as usize);
    let mut pump = |index: usize| {
        let half = index & 1;
        a.set_input(0, half, &to_a.carrying);
        b.set_input(0, half, &to_b.carrying);
        b.fire(half);
        // The block nobody has room for.
        b.fire(half);
        a.fire(half);
        to_a.carry(a.output(0, half));
        to_b.carry(a.output(1, half));
        true
    };
    let outcome = measure_against(host, config([0, 0]), "a test".to_string(), &rig(), &settings(), &mut pump);
    assert_eq!(outcome.refusal, None, "the run itself finished: it is the audio under it that did not");

    let lost = &outcome.readings[1];
    assert!(lost.glitches.lost() > 0, "what the audio did reached the reading: {lost:?}");
    assert!(!lost.is_usable(), "a measurement taken across a lost block is not one");
    assert!(lost.note.contains("the audio was not clean"), "{}", lost.note);
    assert!(outcome.trims[1].not_applied.is_some(), "and no trim is offered from it");
    assert_eq!(outcome.trims[1].new, 0, "what was in the file stays in the file");
    assert!(!outcome.was_clean(), "the run as a whole was not clean");
    assert_eq!(outcome.blocks_lost(), lost.glitches.lost(), "and the reference lost nothing");
    assert!(!outcome.is_measured());

    // A run of the same rig with nobody running ahead is clean, which is what makes the difference
    // above mean something.
    let clean = measured([0, 0], CABLED_LATE as usize, [true, true]);
    assert!(clean.was_clean(), "{:?}", clean.readings);
    assert_eq!(clean.blocks_lost(), 0);
    assert!(clean.readings.iter().all(|reading| reading.glitches.is_clean()));
}

/// A run with a block of silence in it, taken at the block of the run's choosing.
///
/// It leaves longer than the other runs here between starting and its first click, because that is
/// what this is about: the driver forgives the first few blocks of a stream outright, and what has
/// to be shown is a block lost after that and still before the measurement began.
///
/// Up to `slips_at` the interface that is not driving the callback hands its block over in time.
/// At that block the master reaches the ring first and finds nothing, which is the block of
/// silence, and from then on the follower is a block ahead: the arrangement a session settles into
/// and keeps. It is the same thing whether it happens while the interfaces are starting or in the
/// middle of the clicks, and the difference between those two is the whole of this.
fn measured_slipping_at(slips_at: usize) -> Outcome {
    let pc = two_interfaces();
    let host: Box<dyn Host> = Box::new(FakeHost { pc: Arc::clone(&pc) });
    let (a, b): (Arc<FakeDevice>, Arc<FakeDevice>) = (pc.device("Device A"), pc.device("Device B"));
    let mut to_a = Cable::new(0);
    let mut to_b = Cable::new(CABLED_LATE as usize);
    let mut pump = |index: usize| {
        let half = index & 1;
        a.set_input(0, half, &to_a.carrying);
        b.set_input(0, half, &to_b.carrying);
        if index < slips_at {
            b.fire(half);
            a.fire(half);
        } else {
            a.fire(half);
            b.fire(half);
        }
        to_a.carry(a.output(0, half));
        to_b.carry(a.output(1, half));
        true
    };
    let settling = Settings { settle_seconds: 0.05, ..settings() };
    measure_against(host, config([0, 0]), "a test".to_string(), &rig(), &settling, &mut pump)
}

/// **A block of silence taken while the interfaces are starting does not spoil the run.** It
/// happens before the first click, so no click was measured through it, and refusing a perfectly
/// good trim over it is what four of six runs did at the hardware on 2026-09-20.
#[test]
fn a_block_lost_while_the_interfaces_are_starting_is_outside_the_measurement() {
    // Twelve blocks in: past the handful the driver forgives outright, so the block is counted
    // where it happened, and still a long way inside the settling time before the first click.
    let outcome = measured_slipping_at(12);
    assert_eq!(outcome.refusal, None, "{:?}", outcome.refusal);
    assert!(outcome.was_clean(), "nothing was lost while it was measuring: {:?}", outcome.readings);
    assert_eq!(outcome.blocks_lost(), 0);
    assert_eq!(outcome.readings[1].clicks_found, 4, "every click was measured: {:?}", outcome.readings[1]);
    assert!(outcome.readings[1].spread_samples < 0.5, "and they all agreed with each other");
    assert!(outcome.trims[1].not_applied.is_none(), "so the trim it asks for stands");
    assert!(outcome.is_measured());
}

/// And the same block of silence, once the clicks have started, still spoils it.
#[test]
fn a_block_lost_between_the_clicks_still_spoils_the_run() {
    // Half way through the run, which is between the first click and the second.
    let outcome = measured_slipping_at(50);
    assert_eq!(outcome.refusal, None, "the run finished: it is the audio under it that did not");
    assert_eq!(outcome.readings[1].glitches.lost(), 1, "the block reached the reading: {:?}", outcome.readings[1]);
    assert!(!outcome.was_clean());
    assert!(!outcome.readings[1].is_usable(), "a measurement taken across a lost block is not one");
    assert!(outcome.readings[1].note.contains("the audio was not clean"), "{}", outcome.readings[1].note);
    assert!(outcome.trims[1].not_applied.is_some(), "and no trim is offered from it");
    assert_eq!(outcome.trims[1].new, 0, "what was in the file stays in the file");
    assert!(!outcome.is_measured());
    // The reference interface does not cross a ring at all, so it never has this to lose.
    assert!(outcome.readings[0].glitches.is_clean());
}

#[test]
fn a_cable_that_is_not_plugged_in_is_a_cable_and_never_rewrites_a_trim() {
    let outcome = measured([0, 12], CABLED_LATE as usize, [true, false]);
    assert_eq!(outcome.refusal, None);
    let nothing = &outcome.readings[1];
    assert!(nothing.nothing_arrived, "{nothing:?}");
    assert_eq!(nothing.clicks_found, 0);
    assert!(nothing.note.contains("cable"), "{}", nothing.note);
    let trim = &outcome.trims[1];
    assert_eq!(trim.new, 12, "what was in the file is still in the file");
    assert!(trim.not_applied.is_some());
    assert!(!outcome.is_measured());
}

#[test]
fn a_reference_cable_that_is_not_plugged_in_is_a_refusal_rather_than_four_wrong_readings() {
    // Every other interface is measured against the reference, so its cable is the whole run.
    let outcome = measured([0, 0], CABLED_LATE as usize, [false, true]);
    let refusal = outcome.refusal.expect("nothing came back on the reference");
    assert!(refusal.contains("A"), "{refusal}");
    assert!(refusal.contains("check that cable first"), "{refusal}");
    assert!(outcome.readings.is_empty(), "and no interface is given a reading that means nothing");
}

#[test]
fn a_rig_that_spreads_the_outputs_over_both_interfaces_is_refused_and_nothing_is_played() {
    let pc = two_interfaces();
    let host: Box<dyn Host> = Box::new(FakeHost { pc: Arc::clone(&pc) });
    let spread = Rig::new(Direction::Inputs, vec![Pick::new(0, 0), Pick::new(1, 0)], vec![Pick::new(0, 0), Pick::new(1, 0)]);
    let mut pump = |_: usize| panic!("a refused rig never runs a block");
    let outcome = measure_against(host, config([0, 0]), "a test".to_string(), &spread, &settings(), &mut pump);
    let refusal = outcome.refusal.expect("the outputs are on two interfaces");
    assert!(refusal.contains("A") && refusal.contains("B"), "{refusal}");
    assert!(!pc.device("Device A").is_started(), "nothing was started");
    assert!(!pc.device("Device B").has_buffers(), "and no buffers were made");
}

#[test]
fn every_interface_is_let_go_of_when_a_run_finishes() {
    let pc = two_interfaces();
    let host: Box<dyn Host> = Box::new(FakeHost { pc: Arc::clone(&pc) });
    let (a, b) = (pc.device("Device A"), pc.device("Device B"));
    let mut to_a = Cable::new(0);
    let mut to_b = Cable::new(CABLED_LATE as usize);
    let mut pump = |index: usize| {
        let half = index & 1;
        a.set_input(0, half, &to_a.carrying);
        b.set_input(0, half, &to_b.carrying);
        b.fire(half);
        a.fire(half);
        to_a.carry(a.output(0, half));
        to_b.carry(a.output(1, half));
        true
    };
    let outcome = measure_against(host, config([0, 0]), "a test".to_string(), &rig(), &settings(), &mut pump);
    assert_eq!(outcome.refusal, None);
    for key in ["Device A", "Device B"] {
        let device = pc.device(key);
        assert!(!device.is_started(), "{key} was left running");
        assert!(!device.has_buffers(), "{key} was left holding its buffers");
        assert!(device.calls().contains(&"stop".to_string()), "{key} was never stopped");
        assert!(device.calls().contains(&"dispose_buffers".to_string()), "{key} never let its buffers go");
    }
}

/// Stopping is the only way out of a run that is making a noise in the room, so it has to be a
/// real one: the run ends there and then, it says so rather than reporting numbers from half a
/// measurement, and the interfaces are let go exactly as they are on every other path.
#[test]
fn a_run_that_is_stopped_gives_up_at_once_and_lets_the_interfaces_go() {
    let pc = two_interfaces();
    let host: Box<dyn Host> = Box::new(FakeHost { pc: Arc::clone(&pc) });
    let (a, b) = (pc.device("Device A"), pc.device("Device B"));
    let mut to_a = Cable::new(0);
    let mut to_b = Cable::new(CABLED_LATE as usize);
    let mut blocks = 0usize;
    let mut pump = |index: usize| {
        let half = index & 1;
        a.set_input(0, half, &to_a.carrying);
        b.set_input(0, half, &to_b.carrying);
        b.fire(half);
        a.fire(half);
        to_a.carry(a.output(0, half));
        to_b.carry(a.output(1, half));
        blocks += 1;
        blocks < 4
    };
    let outcome = measure_against(host, config([0, 0]), "a test".to_string(), &rig(), &settings(), &mut pump);
    assert_eq!(outcome.refusal.as_deref(), Some(crate::session::STOPPED));
    assert!(outcome.readings.is_empty(), "half a measurement is not a measurement");
    assert!(outcome.trims.is_empty(), "and it implies no trim");
    for key in ["Device A", "Device B"] {
        let device = pc.device(key);
        assert!(!device.is_started(), "{key} was left running");
        assert!(!device.has_buffers(), "{key} was left holding its buffers");
    }
}

// ---------------------------------------------------------------------------------------------
// Naming a channel by interface, against the rig as it really is.
// ---------------------------------------------------------------------------------------------

/// The pair as their drivers really are: the Quadro's has sixteen channels each way and the
/// Studio+'s twenty four. Gazelle's own channel list for the Quadro has fourteen inputs, which is
/// the count a page working the aggregate's numbering out for itself used to go by.
fn quadro_and_studio() -> Arc<FakePc> {
    let spec = |latency_in: i32, channels: i32| {
        Spec { min: 8, max: 4096, preferred: BLOCK, rate: RATE, rates: vec![RATE], ..Spec::default() }
            .with_channels(channels, channels)
            .with_latency(latency_in, 700)
    };
    Arc::new(
        FakePc::new()
            .with("Quadro", "{AAAAAAAA-0000-0000-0000-000000000001}", r"c:\antelope\q.dll", spec(600 + BLOCK, 16))
            .with("Studio+", "{BBBBBBBB-0000-0000-0000-000000000002}", r"c:\antelope\s.dll", spec(600, 24)),
    )
}

/// What the Quadro's channel list in Gazelle has, and so what a count made without the drivers made
/// of it. Studio+'s first input was offered at this number, and the Quadro's fifteenth input is.
const COUNTED_WITHOUT_THE_DRIVERS: usize = 14;

fn studio_setup(tweak: impl FnOnce(&mut DeviceConfig, &mut DeviceConfig)) -> Config {
    let mut quadro = DeviceConfig { key: Some("Quadro".into()), name: Some("Quadro".into()), ..DeviceConfig::default() };
    let mut studio = DeviceConfig { key: Some("Studio+".into()), name: Some("Studio+".into()), ..DeviceConfig::default() };
    tweak(&mut quadro, &mut studio);
    Config { devices: vec![quadro, studio], alignment: Alignment::Aligned, ..Config::default() }
}

/// A run over the real pair's channel counts, with the cable into the Studio+'s first input made
/// `late` samples long, and a decoy: the same click, a different length of cable, on the Quadro's
/// input 15, which is aggregate input 14 and so exactly where a count of fourteen would have put
/// the Studio+'s cable.
fn measured_on_the_real_counts(rig: &Rig, late: usize, decoy_late: usize) -> Outcome {
    let pc = quadro_and_studio();
    let host: Box<dyn Host> = Box::new(FakeHost { pc: Arc::clone(&pc) });
    let (quadro, studio) = (pc.device("Quadro"), pc.device("Studio+"));
    let mut to_quadro = Cable::new(0);
    let mut to_studio = Cable::new(late);
    let mut to_decoy = Cable::new(decoy_late);
    let mut pump = |index: usize| {
        let half = index & 1;
        quadro.set_input(0, half, &to_quadro.carrying);
        quadro.set_input(COUNTED_WITHOUT_THE_DRIVERS, half, &to_decoy.carrying);
        studio.set_input(0, half, &to_studio.carrying);
        studio.fire(half);
        quadro.fire(half);
        to_quadro.carry(quadro.output(0, half));
        let played = quadro.output(1, half);
        to_studio.carry(played.clone());
        to_decoy.carry(played);
        true
    };
    measure_against(host, studio_setup(|_, _| {}), "a test".to_string(), rig, &settings(), &mut pump)
}

/// **The bug the owner hit, and why it cannot happen now.** The Studio+'s cable is named as the
/// Studio+'s first input, and the run finds where that is from the drivers: after all sixteen of
/// the Quadro's inputs, not after the fourteen Gazelle's list has. The Quadro's fifteenth input is
/// carrying a click on a cable of its own, so a run that had recorded the wrong channel would
/// report that cable's length instead of the Studio+'s.
#[test]
fn a_studio_cable_named_by_interface_lands_on_the_studio_whatever_the_quadro_was_counted_as() {
    const DECOY_LATE: usize = 5;
    let outcome = measured_on_the_real_counts(&quadro_studio_rig(), CABLED_LATE as usize, DECOY_LATE);
    assert_eq!(outcome.refusal, None, "{:?}", outcome.refusal);
    let studio = &outcome.readings[1];
    assert_eq!(studio.device, "Studio+");
    assert!(
        (studio.lag_samples - CABLED_LATE as f64).abs() < 0.5,
        "the Studio+'s own cable, not the Quadro's input 15: {studio:?}"
    );
    assert_eq!(outcome.trims[1].measured, CABLED_LATE);
    assert!(outcome.is_measured() && outcome.was_clean());

    // And the Quadro's input 15 is still there to be named, on the Quadro, as the witness it is.
    let watching = quadro_studio_rig().watching(vec![Pick::new(0, COUNTED_WITHOUT_THE_DRIVERS as i32)]);
    let outcome = measured_on_the_real_counts(&watching, CABLED_LATE as usize, DECOY_LATE);
    assert_eq!(outcome.refusal, None, "{:?}", outcome.refusal);
    let witness = &outcome.witnesses[0];
    assert_eq!((witness.device.as_str(), witness.channel), ("Quadro", 14));
    assert!((witness.reading.lag_samples - DECOY_LATE as f64).abs() < 0.5, "{:?}", witness.reading);
    assert!(witness.reading.note.contains("input 15 on Quadro"), "{}", witness.reading.note);
}

/// The Quadro plays out of its first two outputs, one into its own first input and one into the
/// Studio+'s first input.
fn quadro_studio_rig() -> Rig {
    Rig::new(Direction::Inputs, vec![Pick::new(0, 0), Pick::new(0, 1)], vec![Pick::new(0, 0), Pick::new(1, 0)])
}

/// A rig the drivers have to answer for, refused before a buffer is made or a block is played, with
/// what was refused.
fn refused_on_the_real_counts(config: Config, rig: &Rig) -> String {
    let pc = quadro_and_studio();
    let host: Box<dyn Host> = Box::new(FakeHost { pc: Arc::clone(&pc) });
    let mut pump = |_: usize| panic!("a refused rig never runs a block");
    let outcome = measure_against(host, config, "a test".to_string(), rig, &settings(), &mut pump);
    for key in ["Quadro", "Studio+"] {
        assert!(!pc.device(key).is_started(), "{key} was started");
        assert!(!pc.device(key).has_buffers(), "{key} was given buffers");
    }
    outcome.refusal.expect("a refusal")
}

#[test]
fn a_channel_the_interface_has_not_got_is_refused_with_the_count_its_driver_gave() {
    // Input 17 of the Quadro, which the driver says has sixteen.
    let rig = Rig::new(Direction::Inputs, vec![Pick::new(0, 0), Pick::new(0, 1)], vec![Pick::new(0, 16), Pick::new(1, 0)]);
    let refusal = refused_on_the_real_counts(studio_setup(|_, _| {}), &rig);
    assert!(refusal.contains("Quadro has 16 inputs, numbered 1 to 16"), "{refusal}");
    assert!(refusal.contains("no input 17"), "{refusal}");

    // The count is the driver's even when the setup exposes fewer, because the setup is not what
    // decides what the interface has.
    let fewer = studio_setup(|quadro, _| quadro.outputs = Some(vec![0, 1]));
    let past = Rig::new(Direction::Inputs, vec![Pick::new(0, 0), Pick::new(0, 20)], vec![Pick::new(0, 0), Pick::new(1, 0)]);
    let refusal = refused_on_the_real_counts(fewer, &past);
    assert!(refusal.contains("Quadro has 16 outputs, numbered 1 to 16"), "{refusal}");
}

#[test]
fn an_interface_that_is_not_one_of_them_is_refused_and_the_ones_there_are_are_named() {
    let rig = quadro_studio_rig().watching(vec![Pick::new(2, 0)]);
    let refusal = refused_on_the_real_counts(studio_setup(|_, _| {}), &rig);
    assert!(refusal.starts_with("interface 2 is not one of them"), "{refusal}");
    assert!(refusal.contains("0 is Quadro, 1 is Studio+"), "{refusal}");
}

#[test]
fn a_channel_the_setup_keeps_out_of_the_aggregate_is_refused_with_the_ones_it_does_expose() {
    let kept = studio_setup(|_, studio| studio.inputs = Some(vec![0, 1, 2, 3]));
    let rig = Rig::new(Direction::Inputs, vec![Pick::new(0, 0), Pick::new(0, 1)], vec![Pick::new(0, 0), Pick::new(1, 5)]);
    let refusal = refused_on_the_real_counts(kept, &rig);
    assert!(refusal.starts_with("Studio+ input 6 is kept out of the aggregate"), "{refusal}");
    assert!(refusal.contains("(1 to 4)"), "{refusal}");
}

/// **A channel the phase measurement runs over is the driver's**, and the run says that is why it
/// cannot be cabled, rather than calling it missing or letting a cable land on the next channel.
#[test]
fn a_channel_kept_for_the_phase_measurement_is_refused_by_name_and_says_why() {
    let phased = || {
        studio_setup(|_, studio| {
            studio.phase = Some(PhaseConfig { master_output: Some(2), input: Some(2), reference: None });
        })
    };
    let on_input = Rig::new(Direction::Inputs, vec![Pick::new(0, 0), Pick::new(0, 1)], vec![Pick::new(0, 0), Pick::new(1, 2)]);
    let refusal = refused_on_the_real_counts(phased(), &on_input);
    assert!(refusal.starts_with("Studio+ input 3 is where Studio+'s phase measurement arrives"), "{refusal}");
    assert!(refusal.contains("Phase setup"), "{refusal}");

    let on_output = Rig::new(Direction::Inputs, vec![Pick::new(0, 0), Pick::new(0, 2)], vec![Pick::new(0, 0), Pick::new(1, 0)]);
    let refusal = refused_on_the_real_counts(phased(), &on_output);
    assert!(refusal.starts_with("Quadro output 3 carries the signal Studio+'s phase is measured with"), "{refusal}");
}

/// And a channel beside the one the phase keeps is still found where the driver really put it:
/// the Studio+'s fourth input, with its third kept for the phase, is the aggregate's input 18.
#[test]
fn a_channel_beside_the_phase_is_found_where_the_driver_put_it() {
    let pc = quadro_and_studio();
    let host: Box<dyn Host> = Box::new(FakeHost { pc: Arc::clone(&pc) });
    let (quadro, studio) = (pc.device("Quadro"), pc.device("Studio+"));
    let mut to_quadro = Cable::new(0);
    let mut to_studio = Cable::new(CABLED_LATE as usize);
    let mut pump = |index: usize| {
        let half = index & 1;
        quadro.set_input(0, half, &to_quadro.carrying);
        studio.set_input(3, half, &to_studio.carrying);
        studio.fire(half);
        quadro.fire(half);
        to_quadro.carry(quadro.output(0, half));
        to_studio.carry(quadro.output(1, half));
        true
    };
    let config = studio_setup(|_, studio| {
        studio.phase = Some(PhaseConfig { master_output: Some(2), input: Some(2), reference: None });
    });
    let rig = Rig::new(Direction::Inputs, vec![Pick::new(0, 0), Pick::new(0, 1)], vec![Pick::new(0, 0), Pick::new(1, 3)]);
    let settling = Settings { settle_seconds: 0.3, ..settings() };
    let outcome = measure_against(host, config, "a test".to_string(), &rig, &settling, &mut pump);
    assert_eq!(outcome.refusal, None, "{:?}", outcome.refusal);
    assert!((outcome.readings[1].lag_samples - CABLED_LATE as f64).abs() < 0.5, "{:?}", outcome.readings[1]);
}

/// A run at a rate one interface's driver holds on to: the run opens the aggregate the way a DAW
/// does, so the driver is opened again there and the measurement runs at the rate asked for. And a
/// driver that never moves is a refusal, with nothing played.
fn measured_at(rate: f64, takes: Takes) -> (Outcome, Arc<FakeDevice>) {
    let pc = Arc::new(
        FakePc::new()
            .with("Device A", "{AAAAAAAA-0000-0000-0000-000000000001}", r"c:\antelope\a.dll", spec(600 + BLOCK))
            .with("Device B", "{BBBBBBBB-0000-0000-0000-000000000002}", r"c:\antelope\b.dll", spec(600).taking(takes)),
    );
    let host: Box<dyn Host> = Box::new(FakeHost { pc: Arc::clone(&pc) });
    let (a, b) = (pc.device("Device A"), pc.device("Device B"));
    let mut to_a = Cable::new(0);
    let mut to_b = Cable::new(CABLED_LATE as usize);
    let mut pump = |index: usize| {
        let half = index & 1;
        a.set_input(0, half, &to_a.carrying);
        b.set_input(0, half, &to_b.carrying);
        b.fire(half);
        a.fire(half);
        to_a.carry(a.output(0, half));
        to_b.carry(a.output(1, half));
        true
    };
    let at = Settings { rate: Some(rate), ..settings() };
    let outcome = measure_against(host, config([0, 0]), "a test".to_string(), &rig(), &at, &mut pump);
    (outcome, pc.device("Device B"))
}

#[test]
fn a_run_at_a_rate_a_driver_holds_on_to_opens_it_again_and_measures_at_that_rate() {
    let (outcome, b) = measured_at(96_000.0, Takes::WhenReopened);
    assert_eq!(outcome.refusal, None, "{:?}", outcome.refusal);
    assert_eq!(outcome.rate, 96_000.0);
    assert_eq!(b.running_rate(), 96_000.0);
    assert_eq!(b.opens(), 2);
    assert!((outcome.readings[1].lag_samples - CABLED_LATE as f64).abs() < 0.5, "{:?}", outcome.readings[1]);
}

#[test]
fn a_run_at_a_rate_a_driver_never_takes_is_refused_and_nothing_is_played() {
    let (outcome, b) = measured_at(96_000.0, Takes::Never);
    let why = outcome.refusal.expect("B never moved");
    assert!(why.contains("still held 48 kHz after being asked for 96 kHz and reopened twice"), "{why}");
    assert!(outcome.readings.is_empty(), "nothing was measured");
    assert!(!b.is_started());
}
