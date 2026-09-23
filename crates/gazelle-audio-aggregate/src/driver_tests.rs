//! The driver, end to end, against devices and a DAW made of data.
//!
//! Nothing here opens a driver, starts a converter or touches a registry. The fakes hand out real
//! memory and the tests fire their callbacks themselves, so the code under test is the same code
//! that will run at the hardware.

use std::sync::Arc;

use crate::aggregate::{Aggregate, Wanted};
use crate::config::{Alignment, Config, DeviceConfig, PhaseConfig};
use crate::daw;
use crate::fake::{FakeHost, FakePc, Spec, Step, Takes};
use crate::phase;
use gazelle_audio_aggregate_status::record::phase as phase_state;
use crate::status::fake::{unmapped, watched, Written};
use crate::sub::Host;
use crate::watch::{DriverWatch, Reload};
use gazelle_audio_aggregate_status::Reader;
use std::sync::Mutex;
use gazelle_audio_stream_abi::raw::selector;
use gazelle_audio_stream_abi::sample;

const BLOCK: i32 = 4;

/// Two devices, two channels each way, the same latency, so nothing is padded unless a test asks
/// for it. "A" is first and is the master unless the configuration says otherwise.
fn two_devices() -> Arc<FakePc> {
    Arc::new(
        FakePc::new()
            .with_stranger("Realtek ASIO", "{22222222-2222-2222-2222-222222222222}", r"c:\realtek\rt.dll")
            .with("Device A", "{AAAAAAAA-0000-0000-0000-000000000001}", r"c:\antelope\a.dll", spec(2, 2, 600, 700))
            .with("Device B", "{BBBBBBBB-0000-0000-0000-000000000002}", r"c:\antelope\b.dll", spec(2, 2, 600, 700)),
    )
}

fn spec(inputs: i32, outputs: i32, latency_in: i32, latency_out: i32) -> Spec {
    Spec { min: 2, max: 64, preferred: BLOCK, ..Spec::default() }
        .with_channels(inputs, outputs)
        .with_latency(latency_in, latency_out)
}

/// A configuration naming both devices, with "A" first.
fn both(alignment: Alignment) -> Config {
    Config {
        devices: vec![
            DeviceConfig { key: Some("Device A".into()), name: Some("A".into()), ..DeviceConfig::default() },
            DeviceConfig { key: Some("Device B".into()), name: Some("B".into()), ..DeviceConfig::default() },
        ],
        alignment,
        ..Config::default()
    }
}

fn open(pc: &Arc<FakePc>, config: Config) -> Aggregate {
    let host: Box<dyn Host> = Box::new(FakeHost { pc: Arc::clone(pc) });
    let mut aggregate = Aggregate::new(host);
    aggregate.init(config, "a test".to_string()).expect("two ordinary devices open");
    aggregate
}

/// Every channel of the aggregate, in order, which is what a DAW usually asks for.
fn everything(aggregate: &Aggregate) -> Vec<Wanted> {
    let (inputs, outputs) = aggregate.channels();
    (0..inputs)
        .map(|channel| Wanted { is_input: true, channel })
        .chain((0..outputs).map(|channel| Wanted { is_input: false, channel }))
        .collect()
}

/// Open the aggregate, make every buffer, and tell the DAW where they are.
fn running(pc: &Arc<FakePc>, config: Config) -> Aggregate {
    let mut aggregate = open(pc, config);
    let wanted = everything(&aggregate);
    let pairs = aggregate.create_buffers(&wanted, BLOCK, daw::callbacks()).expect("the buffers are made");
    let inputs: Vec<_> = wanted.iter().zip(&pairs).filter(|(w, _)| w.is_input).map(|(_, p)| *p).collect();
    let outputs: Vec<_> = wanted.iter().zip(&pairs).filter(|(w, _)| !w.is_input).map(|(_, p)| *p).collect();
    daw::with(|session| session.buffers(BLOCK as usize, inputs, outputs));
    aggregate.start().expect("both devices start");
    aggregate
}

// ---------------------------------------------------------------------------------------------
// One device, made of several.
// ---------------------------------------------------------------------------------------------

#[test]
fn several_devices_are_presented_as_one_with_every_channel_named() {
    let _order = daw::session();
    let pc = two_devices();
    let aggregate = open(&pc, both(Alignment::Aligned));
    assert_eq!(aggregate.channels(), (4, 4));
    let names: Vec<String> = (0..4).map(|c| aggregate.channel_info(true, c).expect("a channel").name).collect();
    assert_eq!(names, vec!["A 1", "A 2", "B 1", "B 2"]);
    assert_eq!(aggregate.channel_info(false, 3).unwrap().name, "B 2");
    assert!(aggregate.channel_info(true, 4).is_none(), "there is no fifth input");
    // One type for the whole aggregate, whatever the devices carry underneath.
    assert_eq!(aggregate.channel_info(true, 0).unwrap().sample_type, sample::INT32_LSB);
}

#[test]
fn a_device_the_file_does_not_name_is_never_opened() {
    let _order = daw::session();
    let pc = two_devices();
    let only_b = Config {
        devices: vec![DeviceConfig { key: Some("Device B".into()), name: Some("B".into()), ..DeviceConfig::default() }],
        ..Config::default()
    };
    let aggregate = open(&pc, only_b);
    assert_eq!(aggregate.channels(), (2, 2));
    assert_eq!(aggregate.channel_info(true, 0).unwrap().name, "B 1");
    assert!(pc.device("Device A").calls().is_empty(), "the other device was never touched");
}

#[test]
fn a_configured_device_that_is_not_on_this_pc_is_a_refusal_that_names_it() {
    let _order = daw::session();
    let pc = two_devices();
    let missing = Config {
        devices: vec![DeviceConfig { key: Some("Device C".into()), ..DeviceConfig::default() }],
        ..Config::default()
    };
    let host: Box<dyn Host> = Box::new(FakeHost { pc: Arc::clone(&pc) });
    let mut aggregate = Aggregate::new(host);
    let error = aggregate.init(missing, "a test".into()).expect_err("there is no Device C");
    assert!(error.contains("Device C"), "{error}");
    assert_eq!(aggregate.last_error(), error, "and the DAW can read it back");
}

// ---------------------------------------------------------------------------------------------
// The audio path.
// ---------------------------------------------------------------------------------------------

/// Put a block of audio on one device's input.
fn feed(pc: &Arc<FakePc>, key: &str, channel: usize, half: usize, from: i32) {
    pc.device(key).set_input(channel, half, &daw::tone(BLOCK as usize, from));
}

#[test]
fn audio_goes_from_every_device_to_the_daw_and_back_out_again() {
    let _order = daw::session();
    let pc = two_devices();
    let mut aggregate = running(&pc, both(Alignment::LowestLatency));
    let (a, b) = (pc.device("Device A"), pc.device("Device B"));

    feed(&pc, "Device A", 0, 0, 100);
    feed(&pc, "Device A", 1, 0, 200);
    feed(&pc, "Device B", 0, 0, 300);
    feed(&pc, "Device B", 1, 0, 400);

    // The device that is not driving the callback hands its block over first.
    b.fire(0);
    a.fire(0);

    let heard = daw::with(|session| session.heard.clone());
    assert_eq!(heard.len(), 1, "one block of the master is one block to the DAW");
    assert_eq!(heard[0][0], daw::tone(BLOCK as usize, 100), "the master's first input");
    assert_eq!(heard[0][1], daw::tone(BLOCK as usize, 200));
    assert_eq!(heard[0][2], daw::tone(BLOCK as usize, 300), "the other device's input, over the ring");
    assert_eq!(heard[0][3], daw::tone(BLOCK as usize, 400));

    let played = daw::with(|session| session.played.clone());
    // The master's outputs are written on the same callback.
    assert_eq!(a.output(0, 0), played[0][0]);
    assert_eq!(a.output(1, 0), played[0][1]);
    // The other device gets them on its next callback.
    assert_eq!(b.output(0, 0), vec![0; BLOCK as usize], "nothing yet, and silence rather than noise");
    b.fire(1);
    assert_eq!(b.output(0, 1), played[0][2], "and the right channels, to the right device");
    assert_eq!(b.output(1, 1), played[0][3]);

    aggregate.dispose_buffers();
}

#[test]
fn the_daw_is_given_only_the_channels_it_asked_for_and_in_its_own_order() {
    let _order = daw::session();
    let pc = two_devices();
    let mut aggregate = open(&pc, both(Alignment::LowestLatency));
    // A DAW that wants the second device's second input, then the first device's first.
    let wanted = vec![
        Wanted { is_input: true, channel: 3 },
        Wanted { is_input: true, channel: 0 },
        Wanted { is_input: false, channel: 2 },
    ];
    let pairs = aggregate.create_buffers(&wanted, BLOCK, daw::callbacks()).expect("a partial set is fine");
    daw::with(|session| session.buffers(BLOCK as usize, vec![pairs[0], pairs[1]], vec![pairs[2]]));
    aggregate.start().expect("started");

    feed(&pc, "Device A", 0, 0, 100);
    feed(&pc, "Device B", 1, 0, 400);
    pc.device("Device B").fire(0);
    pc.device("Device A").fire(0);

    let heard = daw::with(|session| session.heard.clone());
    assert_eq!(heard[0][0], daw::tone(BLOCK as usize, 400), "the DAW's first buffer is what it asked for first");
    assert_eq!(heard[0][1], daw::tone(BLOCK as usize, 100));

    // The one output it asked for goes to the right device and channel; the others stay silent.
    let played = daw::with(|session| session.played.clone());
    pc.device("Device B").fire(1);
    assert_eq!(pc.device("Device B").output(0, 1), played[0][0]);
    assert_eq!(pc.device("Device B").output(1, 1), vec![0; BLOCK as usize], "a channel nobody asked for is silent");
    assert_eq!(pc.device("Device A").output(0, 0), vec![0; BLOCK as usize]);
    aggregate.dispose_buffers();
}

#[test]
fn no_output_buffer_is_ever_handed_back_without_being_written() {
    let _order = daw::session();
    let pc = two_devices();
    let mut aggregate = running(&pc, both(Alignment::LowestLatency));
    let (a, b) = (pc.device("Device A"), pc.device("Device B"));

    // Every case: before anything has been played, while running, and while the ring is empty.
    for half in [0usize, 1, 0, 1] {
        a.poison_outputs();
        b.poison_outputs();
        b.fire(half);
        a.fire(half);
        assert!(!a.any_output_unwritten(half), "the master left a buffer untouched");
        assert!(!b.any_output_unwritten(half), "the follower left a buffer untouched");
    }

    // And when the DAW itself writes nothing at all, what comes out is silence.
    daw::with(|session| session.silent = true);
    a.poison_outputs();
    b.fire(0);
    a.fire(0);
    assert_eq!(a.output(0, 0), vec![0; BLOCK as usize]);
    aggregate.dispose_buffers();
}

#[test]
fn a_device_that_never_hands_anything_over_is_heard_as_silence() {
    let _order = daw::session();
    let pc = two_devices();
    let mut aggregate = running(&pc, both(Alignment::LowestLatency));
    feed(&pc, "Device A", 0, 0, 100);
    feed(&pc, "Device B", 0, 0, 300);

    // The master runs alone: nothing has been put in the ring.
    pc.device("Device A").fire(0);
    let heard = daw::with(|session| session.heard.clone());
    assert_eq!(heard[0][0], daw::tone(BLOCK as usize, 100), "the master is still heard");
    assert_eq!(heard[0][2], vec![0; BLOCK as usize], "and the other device as silence, not as noise");
    aggregate.dispose_buffers();
}

#[test]
fn a_device_that_runs_ahead_drops_blocks_rather_than_growing_without_end() {
    let _order = daw::session();
    let pc = two_devices();
    let mut aggregate = running(&pc, both(Alignment::LowestLatency));
    let b = pc.device("Device B");
    // Four slots carry three blocks; the fifth has nowhere to go.
    for _ in 0..5 {
        b.fire(0);
    }
    let stream = aggregate.stream().expect("the stream exists");
    let ring = stream.devices[1].in_ring.as_ref().expect("a follower has a ring");
    assert_eq!(ring.waiting(), 3);
    assert_eq!(ring.dropped(), 2, "what could not be carried was dropped, and counted");
    aggregate.dispose_buffers();
}

/// **What the start of a session looks like, and what it must not be logged as.**
///
/// Measured against both interfaces on 2026-09-20: the follower took a block of silence once or
/// twice in the instant a session started, whatever the session went on to do, and never once it
/// was running. It is the master's callback and the follower's finding where they sit inside each
/// other's block: while the master arrives at the ring first there is nothing in it for it, and the
/// silence it takes is what puts the follower a block ahead, where it stays. A driver that called
/// that a lost block would report one on every clean session.
#[test]
fn a_session_that_starts_with_the_follower_behind_has_lost_nothing() {
    let _order = daw::session();
    let pc = two_devices();
    let mut aggregate = running(&pc, both(Alignment::LowestLatency));
    let (a, b) = (pc.device("Device A"), pc.device("Device B"));
    feed(&pc, "Device B", 0, 0, 300);

    // The arrangement with no room in it: the follower hands its block over only just in time.
    for _ in 0..3 {
        b.fire(0);
        a.fire(0);
    }
    // And the master slips in front of it, which is the block of silence and the whole of this.
    a.fire(0);
    let heard = daw::with(|session| session.heard.clone());
    assert_eq!(heard.last().unwrap()[2], vec![0; BLOCK as usize], "silence, because there was nothing there yet");
    // From here the follower is a block ahead, which is where a session stays.
    for _ in 0..12 {
        b.fire(0);
        a.fire(0);
    }

    let glitches = aggregate.stream().expect("the stream exists").glitches();
    assert_eq!(glitches[1].starved, 0, "a session finding its footing has not lost a block");
    assert_eq!(glitches[1].dropped, 0);
    assert_eq!(glitches[0].starved + glitches[0].dropped, 0, "and the master never loses anything at all");
    aggregate.dispose_buffers();
}

/// The same silence once the session is running, which is a different thing entirely: the follower
/// had a whole block of room and still did not fill it in time.
#[test]
fn a_block_missed_once_the_session_is_running_is_counted_reported_and_heard_as_silence() {
    let _order = daw::session();
    let pc = two_devices();
    let (mut aggregate, _reader, written) = reporting(&pc, both(Alignment::LowestLatency));
    go(&mut aggregate);
    let (a, b) = (pc.device("Device A"), pc.device("Device B"));
    feed(&pc, "Device B", 0, 0, 300);

    // Long enough that the two of them are settled, and every block of it comes through.
    for _ in 0..16 {
        b.fire(0);
        a.fire(0);
    }
    assert_eq!(aggregate.stream().expect("streaming").glitches()[1].starved, 0);
    let heard = daw::with(|session| session.heard.clone());
    assert_eq!(heard.last().unwrap()[2], daw::tone(BLOCK as usize, 300), "the follower is being heard");

    // And then one of its blocks is not ready.
    a.fire(0);
    let heard = daw::with(|session| session.heard.clone());
    assert_eq!(heard.last().unwrap()[2], vec![0; BLOCK as usize], "what a lost block sounds like");
    assert_eq!(aggregate.stream().expect("streaming").glitches()[1].starved, 1, "and it is counted");

    aggregate.dispose_buffers();
    let ended = written.of("session-ended");
    assert_eq!(ended.len(), 1, "{:?}", written.lines());
    assert!(ended[0].contains("A lost nothing"), "{}", ended[0]);
    assert!(ended[0].contains("B dropped no blocks and missed 1 block"), "{}", ended[0]);
}

#[test]
fn a_device_that_stops_calling_back_is_muted_and_the_rest_keeps_running() {
    let _order = daw::session();
    let pc = two_devices();
    let config = Config { stall_after_buffers: 3, recover_after_buffers: 2, ..both(Alignment::LowestLatency) };
    let mut aggregate = running(&pc, config);
    let (a, b) = (pc.device("Device A"), pc.device("Device B"));

    // One good block, so the follower has been seen alive.
    feed(&pc, "Device B", 0, 0, 300);
    b.fire(0);
    a.fire(0);
    assert!(!aggregate.stream().unwrap().is_stalled(1));

    // Then it goes away. The master keeps going, and the DAW keeps being called.
    for _ in 0..4 {
        a.fire(0);
    }
    let stream = aggregate.stream().expect("the stream exists");
    assert!(stream.is_stalled(1), "a device that misses enough buffers is declared gone");
    assert_eq!(daw::with(|session| session.calls), 5, "the aggregate did not stop");
    let heard = daw::with(|session| session.heard.clone());
    assert_eq!(heard.last().unwrap()[2], vec![0; BLOCK as usize], "its inputs read as silence");

    // While it is gone, anything it does play is silence, not what it was left with.
    b.poison_outputs();
    b.fire(1);
    assert_eq!(b.output(0, 1), vec![0; BLOCK as usize]);
    assert!(!b.any_output_unwritten(1));

    // And it comes back.
    for _ in 0..3 {
        b.fire(0);
        a.fire(0);
    }
    assert!(!aggregate.stream().unwrap().is_stalled(1), "a device that calls back again is trusted again");
    aggregate.dispose_buffers();
}

#[test]
fn the_master_is_whichever_device_the_file_says() {
    let _order = daw::session();
    let pc = two_devices();
    let config = Config { callback_master: Some("B".into()), ..both(Alignment::LowestLatency) };
    let mut aggregate = running(&pc, config);
    assert_eq!(aggregate.stream().unwrap().master, 1);

    // Now it is "A" that hands its block over, and "B" that calls the DAW.
    feed(&pc, "Device A", 0, 0, 100);
    feed(&pc, "Device B", 0, 0, 300);
    pc.device("Device A").fire(0);
    pc.device("Device B").fire(0);
    let heard = daw::with(|session| session.heard.clone());
    assert_eq!(heard.len(), 1);
    assert_eq!(heard[0][0], daw::tone(BLOCK as usize, 100));
    assert_eq!(heard[0][2], daw::tone(BLOCK as usize, 300));
    aggregate.dispose_buffers();
}

#[test]
fn lining_the_devices_up_holds_the_nearer_one_back_by_the_difference() {
    let _order = daw::session();
    // One device is 8 samples closer on its inputs than the other, and the block is 4, so the
    // master is held back by 8 + 4 = 12 samples: three blocks.
    let pc = Arc::new(
        FakePc::new()
            .with("Device A", "{AAAAAAAA-0000-0000-0000-000000000001}", r"c:\antelope\a.dll", spec(1, 1, 600, 700))
            .with("Device B", "{BBBBBBBB-0000-0000-0000-000000000002}", r"c:\antelope\b.dll", spec(1, 1, 608, 700)),
    );
    let mut aggregate = running(&pc, both(Alignment::Aligned));
    assert_eq!(aggregate.latencies(), Some((608 + BLOCK, 700 + BLOCK)));

    let (a, b) = (pc.device("Device A"), pc.device("Device B"));
    for round in 0..4i32 {
        let half = (round as usize) & 1;
        a.set_input(0, half, &daw::tone(BLOCK as usize, 100 + round * 10));
        b.set_input(0, half, &daw::tone(BLOCK as usize, 300 + round * 10));
        b.fire(half);
        a.fire(half);
    }
    let heard = daw::with(|session| session.heard.clone());
    // The device that is furthest away is heard as it arrives; the near one is held back.
    assert_eq!(heard[0][1], daw::tone(BLOCK as usize, 300), "the far device is not delayed");
    assert_eq!(heard[0][0], vec![0; BLOCK as usize], "the near device is held back");
    assert_eq!(heard[1][0], vec![0; BLOCK as usize]);
    assert_eq!(heard[2][0], vec![0; BLOCK as usize]);
    assert_eq!(heard[3][0], daw::tone(BLOCK as usize, 100), "and arrives three blocks later, lined up");
    assert_eq!(heard[3][1], daw::tone(BLOCK as usize, 330));
    aggregate.dispose_buffers();
}

#[test]
fn the_lowest_latency_setting_holds_nothing_back() {
    let _order = daw::session();
    let pc = Arc::new(
        FakePc::new()
            .with("Device A", "{AAAAAAAA-0000-0000-0000-000000000001}", r"c:\antelope\a.dll", spec(1, 1, 600, 700))
            .with("Device B", "{BBBBBBBB-0000-0000-0000-000000000002}", r"c:\antelope\b.dll", spec(1, 1, 608, 700)),
    );
    let mut aggregate = running(&pc, both(Alignment::LowestLatency));
    let (a, b) = (pc.device("Device A"), pc.device("Device B"));
    a.set_input(0, 0, &daw::tone(BLOCK as usize, 100));
    b.set_input(0, 0, &daw::tone(BLOCK as usize, 300));
    b.fire(0);
    a.fire(0);
    let heard = daw::with(|session| session.heard.clone());
    assert_eq!(heard[0][0], daw::tone(BLOCK as usize, 100), "the master's own path is direct");
    assert_eq!(heard[0][1], daw::tone(BLOCK as usize, 300));
    aggregate.dispose_buffers();
}

#[test]
fn a_device_that_carries_a_different_sample_type_is_converted_on_the_way_past() {
    let _order = daw::session();
    let pc = Arc::new(
        FakePc::new()
            .with("Device A", "{AAAAAAAA-0000-0000-0000-000000000001}", r"c:\antelope\a.dll", spec(1, 1, 600, 700))
            .with(
                "Device B",
                "{BBBBBBBB-0000-0000-0000-000000000002}",
                r"c:\antelope\b.dll",
                spec(1, 1, 600, 700).of_type(sample::INT16_LSB, sample::INT16_LSB),
            ),
    );
    let mut aggregate = running(&pc, both(Alignment::LowestLatency));
    let (a, b) = (pc.device("Device A"), pc.device("Device B"));

    // Sixteen bit samples only carry the top half of a thirty two bit one, so a test value must
    // have nothing below that, which is what the fake writes and reads.
    let sent: Vec<i32> = (0..BLOCK).map(|index| (index + 1) << 16).collect();
    b.set_input(0, 0, &sent);
    b.fire(0);
    a.fire(0);
    let heard = daw::with(|session| session.heard.clone());
    assert_eq!(heard[0][1], sent, "what the narrow device heard arrives at full scale");

    let played = daw::with(|session| session.played.clone());
    b.fire(1);
    let back = b.output(0, 1);
    // What the DAW wrote comes back out of the narrow device with its low bits dropped, and
    // nothing else changed.
    let expected: Vec<i32> = played[0][1].iter().map(|value| value & !0xFFFF).collect();
    assert_eq!(back, expected);
    aggregate.dispose_buffers();
}

// ---------------------------------------------------------------------------------------------
// Rates, buffer sizes and refusals.
// ---------------------------------------------------------------------------------------------

/// Three devices that were not on one rate, and the last refuses to move: each of the others goes
/// back to the rate it was on itself, not to the first one's.
#[test]
fn a_refused_rate_puts_every_device_that_moved_back_on_its_own_rate() {
    let _order = daw::session();
    let pc = Arc::new(
        FakePc::new()
            .with("Device A", "{AAAAAAAA-0000-0000-0000-000000000001}", r"c:\antelope\a.dll", spec(2, 2, 600, 700))
            .with("Device B", "{BBBBBBBB-0000-0000-0000-000000000002}", r"c:\antelope\b.dll", spec(2, 2, 600, 700))
            .with("Device C", "{CCCCCCCC-0000-0000-0000-000000000003}", r"c:\antelope\c.dll", spec(2, 2, 600, 700)),
    );
    pc.device("Device B").spec.lock().unwrap().rate = 88_200.0;
    let three = Config {
        devices: ["A", "B", "C"].iter().map(|name| DeviceConfig { key: Some(format!("Device {name}")), name: Some((*name).into()), ..DeviceConfig::default() }).collect(),
        ..Config::default()
    };
    let mut aggregate = open(&pc, three);
    pc.device("Device C").spec.lock().unwrap().fails_at = Some((Step::SetRate, "no".into()));
    aggregate.set_rate(48_000.0).expect_err("the third refused");
    assert_eq!(pc.device("Device A").spec.lock().unwrap().rate, 96_000.0, "A is back on its own rate");
    assert_eq!(pc.device("Device B").spec.lock().unwrap().rate, 88_200.0, "and B on its own, not on A's");
}

/// The same at `init`, where the aggregate has no rate of its own yet: a rate in the file that the
/// second device refuses leaves the first where it was, rather than at the rate nobody got to.
#[test]
fn a_rate_in_the_file_the_second_device_refuses_puts_the_first_back_on_its_own_rate() {
    let _order = daw::session();
    let pc = two_devices();
    pc.device("Device A").spec.lock().unwrap().rate = 44_100.0;
    pc.device("Device B").spec.lock().unwrap().fails_at = Some((Step::SetRate, "no".into()));
    let host: Box<dyn Host> = Box::new(FakeHost { pc: Arc::clone(&pc) });
    let mut aggregate = Aggregate::new(host);
    let error = aggregate.init(Config { rate: Some(96_000.0), ..both(Alignment::Aligned) }, "a test".into()).expect_err("B refused");
    assert!(error.contains("refused 96000"), "{error}");
    assert_eq!(pc.device("Device A").spec.lock().unwrap().rate, 44_100.0, "A is put back where it was, which nothing did before");
    assert!(pc.device("Device A").calls().iter().any(|call| call == "set_rate 44100"), "{:?}", pc.device("Device A").calls());
}

#[test]
fn a_rate_is_set_on_every_device_or_on_none() {
    let _order = daw::session();
    let pc = two_devices();
    pc.device("Device B").spec.lock().unwrap().rates = vec![44_100.0];
    let mut aggregate = open(&pc, both(Alignment::Aligned));
    assert!(aggregate.can_rate(44_100.0));
    assert!(!aggregate.can_rate(96_000.0), "one device that will not is the whole answer");

    let error = aggregate.set_rate(48_000.0).expect_err("the aggregate refuses as a whole");
    assert!(error.contains("B"), "{error}");
    assert!(
        !pc.device("Device A").calls().iter().any(|call| call.starts_with("set_rate")),
        "nothing was moved, so the devices are still together"
    );
    aggregate.set_rate(44_100.0).expect("a rate they both take");
    assert_eq!(aggregate.rate(), 44_100.0);
}

#[test]
fn a_device_that_refuses_the_buffer_size_is_named_and_nothing_is_left_open() {
    let _order = daw::session();
    let pc = two_devices();
    pc.device("Device B").spec.lock().unwrap().fails_at = Some((Step::CreateBuffers, "not enough USB resources".into()));
    let mut aggregate = open(&pc, both(Alignment::Aligned));
    let wanted = everything(&aggregate);
    let error = aggregate.create_buffers(&wanted, BLOCK, daw::callbacks()).expect_err("B will not take it");
    assert!(error.contains("B") && error.contains("USB"), "{error}");
    assert!(
        pc.device("Device A").calls().iter().any(|call| call == "dispose_buffers"),
        "the device that did make buffers was cleaned up"
    );
}

#[test]
fn a_buffer_size_no_device_will_take_is_refused_before_anything_is_opened() {
    let _order = daw::session();
    let pc = two_devices();
    let mut aggregate = open(&pc, both(Alignment::Aligned));
    let wanted = everything(&aggregate);
    let error = aggregate.create_buffers(&wanted, 4096, daw::callbacks()).expect_err("far too big");
    assert!(error.contains("4096"), "{error}");
    assert!(!pc.device("Device A").has_buffers());
}

#[test]
fn a_device_that_will_not_start_stops_the_ones_that_did() {
    let _order = daw::session();
    let pc = two_devices();
    pc.device("Device A").spec.lock().unwrap().fails_at = Some((Step::Start, "the interface is asleep".into()));
    let mut aggregate = open(&pc, both(Alignment::Aligned));
    let wanted = everything(&aggregate);
    aggregate.create_buffers(&wanted, BLOCK, daw::callbacks()).expect("the buffers are made");
    let error = aggregate.start().expect_err("A will not start");
    assert!(error.contains("A") && error.contains("asleep"), "{error}");
    assert!(!pc.device("Device B").is_started(), "the one that did start was stopped again");
    assert!(!aggregate.stream().unwrap().is_running());
}

#[test]
fn output_ready_is_offered_only_when_every_device_offers_it() {
    let _order = daw::session();
    let pc = two_devices();
    assert!(open(&pc, both(Alignment::Aligned)).output_ready());
    pc.device("Device B").spec.lock().unwrap().output_ready = false;
    assert!(!open(&pc, both(Alignment::Aligned)).output_ready());
}

#[test]
fn the_buffer_sizes_offered_are_the_ones_every_device_can_take() {
    let _order = daw::session();
    let pc = two_devices();
    pc.device("Device A").spec.lock().unwrap().min = 8;
    pc.device("Device B").spec.lock().unwrap().max = 32;
    let aggregate = open(&pc, both(Alignment::Aligned));
    let (min, max, preferred, granularity) = aggregate.buffer_sizes().expect("they overlap");
    assert_eq!((min, max), (8, 32));
    assert_eq!(preferred, BLOCK.max(min));
    assert_eq!(granularity, -1);
}

// ---------------------------------------------------------------------------------------------
// Messages, time and position.
// ---------------------------------------------------------------------------------------------

#[test]
fn a_reset_request_from_any_device_reaches_the_daw() {
    let _order = daw::session();
    let pc = two_devices();
    let mut aggregate = running(&pc, both(Alignment::LowestLatency));
    let answer = pc.device("Device B").send(selector::RESET_REQUEST, 0);
    assert_eq!(answer, 1, "the DAW took it");
    let messages = daw::with(|session| session.messages.clone());
    assert!(messages.contains(&(selector::RESET_REQUEST, 0)), "{messages:?}");

    // A latency change from the other device goes up too.
    pc.device("Device A").send(selector::LATENCIES_CHANGED, 0);
    let messages = daw::with(|session| session.messages.clone());
    assert!(messages.contains(&(selector::LATENCIES_CHANGED, 0)));
    aggregate.dispose_buffers();
}

#[test]
fn the_daw_can_have_the_time_with_each_block_and_it_comes_from_the_master() {
    let _order = daw::session();
    daw::with(|session| session.wants_time = true);
    let pc = two_devices();
    let mut aggregate = running(&pc, both(Alignment::LowestLatency));
    for round in 0..3 {
        pc.device("Device B").fire(round & 1);
        pc.device("Device A").fire(round & 1);
    }
    let times = daw::with(|session| session.times.clone());
    assert_eq!(times.len(), 3, "the time came with every block");
    let positions: Vec<i64> = times.iter().map(|(position, _)| *position).collect();
    assert_eq!(positions, vec![0, BLOCK as i64, BLOCK as i64 * 2], "one block of the master is one block of position");
    // The time carried with a block is the machine's own reference clock, the same one the vendor
    // drivers report (their readings run to days of uptime), NOT time since this driver started. A
    // DAW compares it with its own clock: Cubase metered input happily from a driver whose clock
    // began at zero and then recorded nothing at all (the user, 2026-09-21).
    let machine = crate::now_nanos();
    for (_, nanos) in &times {
        assert!(*nanos > 0, "a system time comes with every block");
        assert!((machine - *nanos).abs() < 60_000_000_000, "and it is this machine's clock: {nanos} against {machine}");
    }
    assert_eq!(aggregate.position().0, BLOCK as i64 * 3);
    aggregate.dispose_buffers();
}

#[test]
fn nothing_is_played_and_nobody_is_called_before_the_daw_starts() {
    let _order = daw::session();
    let pc = two_devices();
    let mut aggregate = open(&pc, both(Alignment::LowestLatency));
    let wanted = everything(&aggregate);
    let pairs = aggregate.create_buffers(&wanted, BLOCK, daw::callbacks()).expect("the buffers are made");
    let inputs: Vec<_> = wanted.iter().zip(&pairs).filter(|(w, _)| w.is_input).map(|(_, p)| *p).collect();
    let outputs: Vec<_> = wanted.iter().zip(&pairs).filter(|(w, _)| !w.is_input).map(|(_, p)| *p).collect();
    daw::with(|session| session.buffers(BLOCK as usize, inputs, outputs));

    // A device that calls back early, which a real one can do between createBuffers and start.
    pc.device("Device A").poison_outputs();
    pc.device("Device A").fire(0);
    pc.device("Device B").fire(0);
    assert_eq!(daw::with(|session| session.calls), 0, "the DAW was not called");
    assert_eq!(pc.device("Device A").output(0, 0), vec![0; BLOCK as usize], "and what came out was silence");
    assert!(!pc.device("Device A").any_output_unwritten(0));
    aggregate.dispose_buffers();
}

#[test]
fn the_device_that_drives_the_callback_starts_last_and_stops_first() {
    let _order = daw::session();
    // Whichever device drives the callback, and wherever it is in the list.
    for (master, follower) in [("A", "B"), ("B", "A")] {
        let pc = two_devices();
        let config = Config { callback_master: Some(master.into()), ..both(Alignment::LowestLatency) };
        let mut aggregate = running(&pc, config);
        aggregate.dispose_buffers();
        let journal = pc.journal();
        let at = |what: &str| {
            journal.iter().position(|call| call == what).unwrap_or_else(|| panic!("{what} never happened: {journal:?}"))
        };
        let master = format!("Device {master}");
        let follower = format!("Device {follower}");
        // Starting the master first would have it calling the DAW, and asking the other device for
        // audio, before that device was running.
        assert!(at(&format!("{follower}: start")) < at(&format!("{master}: start")), "{journal:?}");
        // And stopping it last would leave it calling the DAW while the other was going away.
        assert!(at(&format!("{master}: stop")) < at(&format!("{follower}: stop")), "{journal:?}");
        assert!(at(&format!("{master}: stop")) < at(&format!("{master}: dispose_buffers")), "{journal:?}");
    }
}

#[test]
fn stopping_and_disposing_leaves_every_device_stopped_and_empty() {
    let _order = daw::session();
    let pc = two_devices();
    let mut aggregate = running(&pc, both(Alignment::LowestLatency));
    assert!(pc.device("Device A").is_started() && pc.device("Device B").is_started());
    aggregate.dispose_buffers();
    assert!(!pc.device("Device A").is_started());
    assert!(!pc.device("Device B").is_started());
    assert!(!pc.device("Device A").has_buffers());
    // The master is stopped first, so that it cannot call the DAW while the others are going away.
    let calls = pc.device("Device A").calls();
    let stopped = calls.iter().position(|call| call == "stop").expect("it was stopped");
    let disposed = calls.iter().position(|call| call == "dispose_buffers").expect("and disposed");
    assert!(stopped < disposed, "{calls:?}");
}

// ---------------------------------------------------------------------------------------------
// With no configuration file at all.
// ---------------------------------------------------------------------------------------------

#[test]
fn with_no_file_every_antelope_driver_is_opened_in_registry_order() {
    let _order = daw::session();
    let pc = Arc::new(FakePc::this_pc());
    let aggregate = open(&pc, Config::default());
    assert_eq!(aggregate.channels(), (16 + 24, 16 + 24), "both interfaces, and neither stranger");
    assert_eq!(aggregate.channel_info(true, 0).unwrap().name, "Zen Quadro Synergy Core 1");
    assert_eq!(aggregate.channel_info(true, 16).unwrap().name, "ZenStudioTB ASIO Driver 1");
    assert_eq!(aggregate.plan().unwrap().master, 0, "the first one found drives the callback");
}

#[test]
fn a_pc_with_nothing_of_ours_on_it_says_so_plainly() {
    let _order = daw::session();
    let pc = Arc::new(FakePc::new().with_stranger("Realtek ASIO", "{22222222-2222-2222-2222-222222222222}", r"c:\rt.dll"));
    let host: Box<dyn Host> = Box::new(FakeHost { pc });
    let mut aggregate = Aggregate::new(host);
    let error = aggregate.init(Config::default(), "a test".into()).expect_err("there is nothing to aggregate");
    assert!(error.contains("Antelope"), "{error}");
}

#[test]
fn a_driver_that_will_not_share_a_process_is_reported_by_name() {
    let _order = daw::session();
    let pc = Arc::new(FakePc::this_pc().refusing("{AE4A4452-A316-11E5-A113-080027F6C1F4}"));
    let host: Box<dyn Host> = Box::new(FakeHost { pc });
    let mut aggregate = Aggregate::new(host);
    let error = aggregate.init(Config::default(), "a test".into()).expect_err("the second driver refused");
    assert!(error.contains("ZenStudioTB"), "{error}");
    assert!(error.contains("share a process"), "{error}");
}

#[test]
fn a_trim_in_the_file_moves_a_device_and_the_latency_the_daw_is_told() {
    // Measured at the devices on 2026-09-21: the same source recorded into both landed about 28
    // samples later on one of them than on the other, and stayed there when the two microphones
    // were swapped, so the difference is the device's, not the microphone's. A trim in the file
    // nulls it without touching any code.
    let _order = daw::session();
    let pc = Arc::new(
        FakePc::new()
            .with("Device A", "{AAAAAAAA-0000-0000-0000-000000000001}", r"c:\antelope\a.dll", spec(1, 1, 600, 700))
            .with("Device B", "{BBBBBBBB-0000-0000-0000-000000000002}", r"c:\antelope\b.dll", spec(1, 1, 600, 700)),
    );
    let mut plain = running(&pc, both(Alignment::Aligned));
    let (was_in, was_out) = plain.latencies().expect("a plan exists");
    plain.dispose_buffers();

    // The device that records late admits to less latency than it has: a positive trim puts that
    // back, the device becomes the longest path, and every other device is held back to meet it.
    let mut config = both(Alignment::Aligned);
    config.devices[1].input_trim = Some(9);
    let mut late = running(&pc, config);
    assert_eq!(late.latencies().expect("a plan exists").0, was_in + 9, "the figure follows the device that records late");
    late.dispose_buffers();

    // A small negative trim shortens the longest path, so the figure follows it down.
    let mut config = both(Alignment::Aligned);
    config.devices[1].input_trim = Some(-2);
    let mut small = running(&pc, config);
    let (now_in, now_out) = small.latencies().expect("a plan exists");
    assert_eq!(now_in, was_in - 2, "the trimmed device is still the slowest path, so the figure follows it");
    assert_eq!(now_out, was_out, "and its outputs are untouched");
    small.dispose_buffers();

    // A trim bigger than the block a buffered device costs brings it in front of the master, and
    // then the master carries the longest path and the figure stops following the trim.
    let mut config = both(Alignment::Aligned);
    config.devices[1].input_trim = Some(-28);
    let mut big = running(&pc, config);
    assert_eq!(big.latencies().expect("a plan exists").0, 600, "the master's own path, which nothing trimmed");
    big.dispose_buffers();
}

// ---------------------------------------------------------------------------------------------
// What the driver tells the outside world, and what it does when it is told something.
// ---------------------------------------------------------------------------------------------

/// A driver that publishes, with the reader for its record and the log it writes.
fn reporting(pc: &Arc<FakePc>, config: Config) -> (Aggregate, Reader, Written) {
    let (reporter, reader, written) = watched();
    let host: Box<dyn Host> = Box::new(FakeHost { pc: Arc::clone(pc) });
    let mut aggregate = Aggregate::reporting(host, reporter);
    aggregate.init(config, r"C:\Users\someone\AppData\Roaming\gazelle\aggregate.json".to_string()).expect("both open");
    (aggregate, reader, written)
}

/// Make every buffer and start, as `running` does, on an aggregate that is already open.
fn go(aggregate: &mut Aggregate) {
    let wanted = everything(aggregate);
    let pairs = aggregate.create_buffers(&wanted, BLOCK, daw::callbacks()).expect("the buffers are made");
    let inputs: Vec<_> = wanted.iter().zip(&pairs).filter(|(w, _)| w.is_input).map(|(_, p)| *p).collect();
    let outputs: Vec<_> = wanted.iter().zip(&pairs).filter(|(w, _)| !w.is_input).map(|(_, p)| *p).collect();
    daw::with(|session| session.buffers(BLOCK as usize, inputs, outputs));
    aggregate.start().expect("both devices start");
}

#[test]
fn the_record_says_what_plan_is_in_force_as_soon_as_the_driver_is_open() {
    let _order = daw::session();
    let pc = two_devices();
    let (_aggregate, reader, written) = reporting(&pc, both(Alignment::Aligned));
    let seen = reader.read().expect("a settled record");
    assert_eq!(seen.driver.device_count, 2);
    assert_eq!(seen.driver.master, 0);
    assert_eq!(seen.driver.sample_rate, 96_000.0);
    assert!(seen.driver.config_source.get().ends_with("aggregate.json"), "{:?}", seen.driver.config_source);
    let names: Vec<&str> = seen.devices().iter().map(|device| device.name.get()).collect();
    assert_eq!(names, vec!["A", "B"]);
    assert_eq!(seen.devices()[0].is_master, 1);
    assert_eq!(seen.devices()[1].is_master, 0);
    assert_eq!(seen.devices()[0].inputs, 2);
    assert_eq!(seen.driver.open, 0, "no DAW has asked for buffers yet");
    assert_eq!(seen.driver.streaming, 0);
    assert!(written.lines().is_empty(), "opening the driver is not an event worth keeping");
}

#[test]
fn the_record_follows_a_session_from_the_buffers_being_made_to_them_being_let_go() {
    let _order = daw::session();
    let pc = two_devices();
    let (mut aggregate, reader, written) = reporting(&pc, both(Alignment::Aligned));
    go(&mut aggregate);
    let seen = reader.read().unwrap();
    assert_eq!(seen.driver.open, 1);
    assert_eq!(seen.driver.streaming, 1);
    assert_eq!(seen.driver.daw_inputs, 4);
    assert_eq!(seen.driver.daw_outputs, 4);
    assert_eq!(seen.driver.buffer_size, BLOCK);
    assert_eq!(written.of("session-started").len(), 1, "{:?}", written.lines());

    aggregate.dispose_buffers();
    let seen = reader.read().unwrap();
    assert_eq!(seen.driver.open, 0);
    assert_eq!(seen.driver.streaming, 0);
    assert_eq!(written.of("session-ended").len(), 1);
}

#[test]
fn every_block_moves_the_counters_and_two_locked_devices_show_no_gap_at_all() {
    let _order = daw::session();
    let pc = two_devices();
    let (mut aggregate, reader, _written) = reporting(&pc, both(Alignment::Aligned));
    go(&mut aggregate);
    let (a, b) = (pc.device("Device A"), pc.device("Device B"));
    // A locked pair: each device calls back once per block, in step, which is what phase 0
    // measured at the hardware.
    for half in 0..6 {
        b.fire(half & 1);
        a.fire(half & 1);
    }
    let seen = reader.read().expect("a settled record");
    assert_eq!(seen.driver.callbacks, 6, "the master drove six blocks");
    assert_eq!(seen.driver.position, 6 * BLOCK as u64);
    assert!(seen.driver.last_block_nanos > 0, "and said when the last one was, on the machine's clock");
    assert_eq!(seen.devices()[0].gap, 0, "the master is its own reference");
    assert_eq!(seen.devices()[1].gap, 0, "a locked device holds the same count");
    assert_eq!(seen.devices()[1].callbacks, 6);
    assert!(seen.devices().iter().all(|device| device.streaming == 1));
}

#[test]
fn a_device_that_falls_behind_shows_the_gap_in_samples() {
    let _order = daw::session();
    let pc = two_devices();
    let (mut aggregate, reader, _written) = reporting(&pc, both(Alignment::Aligned));
    go(&mut aggregate);
    let (a, b) = (pc.device("Device A"), pc.device("Device B"));
    // The follower misses two of the master's blocks, which is two blocks of samples behind.
    for half in 0..6 {
        if half >= 2 {
            b.fire(half & 1);
        }
        a.fire(half & 1);
    }
    let seen = reader.read().unwrap();
    assert_eq!(seen.devices()[1].gap, -2 * BLOCK as i64, "behind the master by two blocks of samples");

    // And it can be in front, which is what a device that started early looks like.
    for half in 0..4 {
        b.fire(half & 1);
    }
    a.fire(0);
    let seen = reader.read().unwrap();
    assert_eq!(seen.devices()[1].gap, BLOCK as i64);
}

#[test]
fn a_device_that_stops_is_stalled_in_the_record_and_one_line_in_the_log_when_the_watcher_looks() {
    let _order = daw::session();
    let pc = two_devices();
    let (reporter, reader, written) = watched();
    let host: Box<dyn Host> = Box::new(FakeHost { pc: Arc::clone(&pc) });
    let driver = Arc::new(Mutex::new(Aggregate::reporting(host, Arc::clone(&reporter))));
    driver.lock().unwrap().init(both(Alignment::Aligned), "a test".to_string()).expect("both open");
    go(&mut driver.lock().unwrap());
    let watcher = DriverWatch::new(&driver, Arc::clone(&reporter));

    let (a, b) = (pc.device("Device A"), pc.device("Device B"));
    for half in 0..2 {
        b.fire(half & 1);
        a.fire(half & 1);
    }
    watcher.look_around();
    assert!(written.of("stalled").is_empty(), "nothing has gone anywhere");

    // The follower stops dead. The master notices after `stall_after_buffers` of its own blocks.
    for half in 0..6 {
        a.fire(half & 1);
    }
    assert_eq!(reader.read().unwrap().devices()[1].stalled, 1, "the audio path set the flag");
    watcher.look_around();
    watcher.look_around();
    assert_eq!(written.of("stalled").len(), 1, "once, not once per look: {:?}", written.lines());
    assert!(written.of("stalled")[0].ends_with("stalled B"));

    // And it comes back.
    for half in 0..4 {
        b.fire(half & 1);
        a.fire(half & 1);
    }
    assert_eq!(reader.read().unwrap().devices()[1].stalled, 0);
    watcher.look_around();
    watcher.look_around();
    assert_eq!(written.of("recovered").len(), 1);
    assert!(written.of("recovered")[0].ends_with("recovered B"));

    driver.lock().unwrap().dispose_buffers();
}

#[test]
fn a_session_that_lost_nothing_says_so_when_it_ends() {
    let _order = daw::session();
    let pc = two_devices();
    let (mut aggregate, _reader, written) = reporting(&pc, both(Alignment::Aligned));
    go(&mut aggregate);
    let (a, b) = (pc.device("Device A"), pc.device("Device B"));
    for half in 0..6 {
        b.fire(half & 1);
        a.fire(half & 1);
    }
    aggregate.dispose_buffers();
    let ended = written.of("session-ended");
    assert_eq!(ended.len(), 1, "{:?}", written.lines());
    assert!(ended[0].contains("ran for"), "{}", ended[0]);
    assert!(ended[0].contains("no interface lost a block"), "{}", ended[0]);
    assert!(written.of("glitched").is_empty(), "nothing was lost, so nothing was said about it");
}

#[test]
fn a_session_that_dropped_blocks_says_which_interface_and_how_many_when_it_ends() {
    let _order = daw::session();
    let pc = two_devices();
    let (mut aggregate, _reader, written) = reporting(&pc, both(Alignment::Aligned));
    go(&mut aggregate);
    let b = pc.device("Device B");
    // The follower runs away from the master: four slots carry three blocks, and the two after
    // them have nowhere to go.
    for _ in 0..5 {
        b.fire(0);
    }
    aggregate.dispose_buffers();
    let ended = written.of("session-ended");
    assert_eq!(ended.len(), 1, "{:?}", written.lines());
    assert!(ended[0].contains("A lost nothing"), "{}", ended[0]);
    assert!(ended[0].contains("B dropped 2 blocks and missed no blocks"), "{}", ended[0]);
    assert!(written.of("glitched").len() <= 1, "the first one only, and nobody was watching here");
    // The counters themselves are gone with the buffers, and this line is what is left of them.
    assert!(aggregate.stream().is_none());
}

#[test]
fn the_first_block_a_session_loses_is_one_line_and_the_rest_of_them_are_none() {
    let _order = daw::session();
    let pc = two_devices();
    let (reporter, _reader, written) = watched();
    let host: Box<dyn Host> = Box::new(FakeHost { pc: Arc::clone(&pc) });
    let driver = Arc::new(Mutex::new(Aggregate::reporting(host, Arc::clone(&reporter))));
    driver.lock().unwrap().init(both(Alignment::Aligned), "a test".to_string()).expect("both open");
    go(&mut driver.lock().unwrap());
    let watcher = DriverWatch::new(&driver, Arc::clone(&reporter));

    let (a, b) = (pc.device("Device A"), pc.device("Device B"));
    for half in 0..2 {
        b.fire(half & 1);
        a.fire(half & 1);
    }
    watcher.look_around();
    assert!(written.of("glitched").is_empty(), "a session in step has lost nothing");

    // The follower runs ahead until its ring has nowhere to put a block, and the master's next
    // callback is what writes the count into the record for the watcher to find.
    for _ in 0..6 {
        b.fire(0);
    }
    a.fire(0);
    watcher.look_around();
    watcher.look_around();
    let said = written.of("glitched");
    assert_eq!(said.len(), 1, "once, not once per look and not once per block: {:?}", written.lines());
    assert!(said[0].contains("B dropped a block"), "{}", said[0]);
    assert!(said[0].contains("the first this session has lost"), "{}", said[0]);

    driver.lock().unwrap().dispose_buffers();
}

#[test]
fn a_refusal_reaches_the_daw_the_record_and_the_log_in_the_same_words() {
    let _order = daw::session();
    let pc = two_devices();
    let (mut aggregate, reader, written) = reporting(&pc, both(Alignment::Aligned));
    let why = aggregate.set_rate(22_050.0).expect_err("neither device will run there");
    assert_eq!(aggregate.last_error(), why, "the DAW reads it back");
    assert_eq!(reader.read().unwrap().driver.refusal.get(), why, "and so does anything watching");
    assert_eq!(written.of("refused").len(), 1);
    assert!(written.of("refused")[0].contains("22050"), "{:?}", written.lines());
}

#[test]
fn a_driver_that_could_not_make_a_section_works_exactly_as_it_did_before() {
    let _order = daw::session();
    let pc = two_devices();
    let (reporter, written) = unmapped();
    assert!(!reporter.is_publishing(), "there is nowhere to publish");
    let host: Box<dyn Host> = Box::new(FakeHost { pc: Arc::clone(&pc) });
    let mut aggregate = Aggregate::reporting(host, reporter);
    aggregate.init(both(Alignment::Aligned), "a test".to_string()).expect("the devices open anyway");
    assert_eq!(aggregate.channels(), (4, 4));
    go(&mut aggregate);

    // The whole audio path still runs, and the DAW still hears what the devices heard.
    let (a, b) = (pc.device("Device A"), pc.device("Device B"));
    let tone = daw::tone(BLOCK as usize, 100);
    for half in 0..4 {
        a.set_input(0, half & 1, &tone);
        b.fire(half & 1);
        a.fire(half & 1);
    }
    let heard = daw::with(|session| session.last_heard().cloned()).expect("the DAW was called");
    assert_eq!(heard[0], tone, "the master's input reached the DAW with nowhere to report it");
    // And the durable half still works, because the two are not the same thing.
    assert_eq!(written.of("session-started").len(), 1);
    aggregate.dispose_buffers();
}

#[test]
fn the_names_a_person_gave_their_channels_are_what_the_daw_is_told() {
    let _order = daw::session();
    let pc = two_devices();
    let mut config = both(Alignment::Aligned);
    config.devices[0].input_names = [(0, "Vocal mic".to_string())].into_iter().collect();
    config.devices[1].output_names = [(1, "Main R".to_string())].into_iter().collect();
    let mut aggregate = open(&pc, config);
    assert_eq!(aggregate.channel_info(true, 0).unwrap().name, "Vocal mic (A 1)");
    assert_eq!(aggregate.channel_info(true, 1).unwrap().name, "A 2", "the channel beside it is untouched");
    assert_eq!(aggregate.channel_info(false, 3).unwrap().name, "Main R (B 2)");

    // The plan is made again when the DAW asks for buffers, from what the drivers report, which
    // carries none of this. The names have to survive that.
    let wanted = everything(&aggregate);
    aggregate.create_buffers(&wanted, BLOCK, daw::callbacks()).expect("the buffers are made");
    assert_eq!(aggregate.channel_info(true, 0).unwrap().name, "Vocal mic (A 1)");
    assert_eq!(aggregate.channel_info(false, 3).unwrap().name, "Main R (B 2)");
    aggregate.dispose_buffers();
}

#[test]
fn renaming_a_channel_while_the_driver_is_loaded_is_an_ordinary_change_of_configuration() {
    let _order = daw::session();
    let pc = two_devices();
    let (mut aggregate, _reader, written) = reporting(&pc, both(Alignment::Aligned));
    assert_eq!(aggregate.channel_info(true, 0).unwrap().name, "A 1");

    let mut renamed = both(Alignment::Aligned);
    renamed.devices[0].input_names = [(0, "Vocal mic".to_string())].into_iter().collect();
    aggregate.reconfigure(renamed, "a newer file".to_string(), 2).expect("a name is a plan like any other");
    assert_eq!(aggregate.channel_info(true, 0).unwrap().name, "Vocal mic (A 1)");
    assert!(written.of("refused").is_empty(), "{:?}", written.lines());
}

#[test]
fn a_new_configuration_with_nothing_streaming_is_taken_up_there_and_then() {
    let _order = daw::session();
    let pc = two_devices();
    let (mut aggregate, reader, written) = reporting(&pc, both(Alignment::Aligned));
    assert_eq!(aggregate.channels(), (4, 4));

    let only_b = Config {
        devices: vec![DeviceConfig { key: Some("Device B".into()), name: Some("B".into()), ..DeviceConfig::default() }],
        ..Config::default()
    };
    aggregate.reconfigure(only_b, "a newer file".to_string(), 3).expect("one of the two devices is a fine plan");
    assert_eq!(aggregate.channels(), (2, 2));
    assert_eq!(aggregate.channel_info(true, 0).unwrap().name, "B 1");
    let seen = reader.read().unwrap();
    assert_eq!(seen.driver.device_count, 1);
    assert_eq!(seen.devices()[0].name.get(), "B");
    assert_eq!(seen.driver.generation_in_force, 3, "and Gazelle can see its request landed");
    assert_eq!(seen.driver.config_source.get(), "a newer file");
    assert!(written.of("refused").is_empty(), "{:?}", written.lines());
    // The device that is no longer in the plan was let go of: nothing was asked of it again, and
    // it holds no buffers.
    assert_eq!(pc.device("Device A").calls(), vec!["init", "describe"]);
    assert!(!pc.device("Device A").has_buffers());
}

#[test]
fn a_new_configuration_that_names_a_device_this_pc_does_not_have_is_refused_before_anything_is_touched() {
    let _order = daw::session();
    let pc = two_devices();
    let (aggregate, _reader, _written) = reporting(&pc, both(Alignment::Aligned));
    let missing = Config {
        devices: vec![DeviceConfig { key: Some("Device C".into()), ..DeviceConfig::default() }],
        ..Config::default()
    };
    let why = aggregate.check(&missing).expect_err("there is no Device C on this PC");
    assert!(why.contains("Device C"), "{why}");
    // Checking opens nothing, so the driver is exactly as it was.
    assert_eq!(aggregate.channels(), (4, 4));
    assert!(aggregate.check(&both(Alignment::Aligned)).is_ok());
}

#[test]
fn a_configuration_that_will_not_open_puts_back_the_one_that_did() {
    let _order = daw::session();
    let pc = Arc::new(
        FakePc::new()
            .with("Device A", "{AAAAAAAA-0000-0000-0000-000000000001}", r"c:\antelope\a.dll", spec(2, 2, 600, 700))
            .with(
                "Device B",
                "{BBBBBBBB-0000-0000-0000-000000000002}",
                r"c:\antelope\b.dll",
                Spec { min: 2, max: 64, preferred: BLOCK, ..Spec::default() }
                    .with_channels(2, 2)
                    .failing(Step::Init, "this driver will not start up twice"),
            ),
    );
    let only_a = Config {
        devices: vec![DeviceConfig { key: Some("Device A".into()), name: Some("A".into()), ..DeviceConfig::default() }],
        ..Config::default()
    };
    let (mut aggregate, reader, written) = reporting(&pc, only_a);
    assert_eq!(aggregate.channels(), (2, 2));

    let why = aggregate
        .reconfigure(both(Alignment::Aligned), "a newer file".to_string(), 5)
        .expect_err("Device B will not start up");
    assert!(why.contains("Device B") || why.contains('B'), "{why}");
    // What was working is working again, and the DAW would be told why the change did not happen.
    assert_eq!(aggregate.channels(), (2, 2), "the plan that opened is in force again");
    assert_eq!(aggregate.channel_info(true, 0).unwrap().name, "A 1");
    let seen = reader.read().unwrap();
    assert_eq!(seen.driver.device_count, 1);
    assert_eq!(seen.devices()[0].name.get(), "A");
    assert_eq!(seen.driver.refused_generation, 5);
    assert!(!written.of("refused").is_empty());
}

#[test]
fn a_configuration_that_arrives_while_a_daw_is_streaming_is_taken_up_when_it_comes_back() {
    let _order = daw::session();
    let pc = two_devices();
    let (mut aggregate, reader, _written) = reporting(&pc, both(Alignment::Aligned));
    go(&mut aggregate);
    assert!(aggregate.has_buffers(), "a DAW is holding a plan");

    let only_b = Config {
        devices: vec![DeviceConfig { key: Some("Device B".into()), name: Some("B".into()), ..DeviceConfig::default() }],
        ..Config::default()
    };
    aggregate.queue(only_b, "a newer file".to_string(), 8);
    // Nothing has moved under the DAW.
    assert_eq!(aggregate.channels(), (4, 4));
    assert_eq!(reader.read().unwrap().driver.device_count, 2);

    // The DAW puts everything down and picks it up again, which is what a reset means.
    aggregate.dispose_buffers();
    let wanted: Vec<Wanted> = (0..2)
        .map(|channel| Wanted { is_input: true, channel })
        .chain((0..2).map(|channel| Wanted { is_input: false, channel }))
        .collect();
    aggregate.create_buffers(&wanted, BLOCK, daw::callbacks()).expect("the new plan makes buffers");
    assert_eq!(aggregate.channels(), (2, 2), "the queued plan is in force now");
    let seen = reader.read().unwrap();
    assert_eq!(seen.driver.device_count, 1);
    assert_eq!(seen.driver.generation_in_force, 8);
    aggregate.dispose_buffers();
}

#[test]
fn asking_the_host_to_reset_sends_exactly_one_message_and_only_while_a_daw_is_streaming() {
    let _order = daw::session();
    let pc = two_devices();
    let (reporter, _reader, written) = watched();
    let host: Box<dyn Host> = Box::new(FakeHost { pc: Arc::clone(&pc) });
    let driver = Arc::new(Mutex::new(Aggregate::reporting(host, Arc::clone(&reporter))));
    driver.lock().unwrap().init(both(Alignment::Aligned), "a test".to_string()).expect("both open");
    let watcher = DriverWatch::new(&driver, Arc::clone(&reporter));

    // Nothing is streaming: there is no host to ask, and nothing is sent.
    assert!(!watcher.is_streaming());
    watcher.ask_host_to_reset();
    assert!(daw::with(|session| session.messages.clone()).is_empty(), "no DAW has the driver open");

    go(&mut driver.lock().unwrap());
    daw::with(|session| session.messages.clear());
    assert!(watcher.is_streaming());
    watcher.ask_host_to_reset();
    let asked: Vec<(i32, i32)> =
        daw::with(|session| session.messages.iter().copied().filter(|(what, _)| *what == selector::RESET_REQUEST).collect());
    assert_eq!(asked, vec![(selector::RESET_REQUEST, 0)], "one reset request, and nothing else");
    assert_eq!(written.of("reset-asked").len(), 1);

    driver.lock().unwrap().dispose_buffers();
}

#[test]
fn the_watcher_lets_go_of_a_driver_that_has_been_released() {
    let _order = daw::session();
    let pc = two_devices();
    let (reporter, _reader, _written) = watched();
    let host: Box<dyn Host> = Box::new(FakeHost { pc: Arc::clone(&pc) });
    let driver = Arc::new(Mutex::new(Aggregate::reporting(host, Arc::clone(&reporter))));
    let watcher = DriverWatch::new(&driver, Arc::clone(&reporter));
    assert!(watcher.is_alive());
    drop(driver);
    assert!(!watcher.is_alive(), "the watcher never holds a driver open");
    // And everything it can be asked to do answers without a driver rather than panicking.
    assert!(watcher.check(&both(Alignment::Aligned)).is_ok());
    assert!(!watcher.is_streaming());
    assert!(watcher.adopt(both(Alignment::Aligned), "a test".to_string(), 1).is_err());
    watcher.ask_host_to_reset();
    watcher.look_around();
}

// ---------------------------------------------------------------------------------------------
// The phase each interface's capture settles on, measured at the start of a session.
// ---------------------------------------------------------------------------------------------

/// A device with no latency of its own, so that what a test works out is the measurement's own
/// arithmetic and nothing else, and a rate that makes the listening window a handful of blocks.
fn quiet(inputs: i32, outputs: i32) -> Spec {
    Spec { min: 2, max: 64, preferred: BLOCK, rate: QUIET_RATE, rates: vec![QUIET_RATE], ..Spec::default() }
        .with_channels(inputs, outputs)
        .with_latency(QUIET_LATENCY, QUIET_LATENCY)
}

/// What these devices report each way, which is what a measurement is residual to.
const QUIET_LATENCY: i32 = 100;

/// A rate that makes the block a sensible fraction of a second, so the window the driver listens
/// for is tens of blocks rather than thousands.
const QUIET_RATE: f64 = 2_000.0;

/// Two devices, three channels each way, with nothing to line up until something is measured.
fn quiet_pc() -> Arc<FakePc> {
    Arc::new(
        FakePc::new()
            .with("Device A", "{AAAAAAAA-0000-0000-0000-000000000001}", r"c:\antelope\a.dll", quiet(3, 3))
            .with("Device B", "{BBBBBBBB-0000-0000-0000-000000000002}", r"c:\antelope\b.dll", quiet(3, 3)),
    )
}

/// The same configuration as `both`, with a cable declared from the master's output to the
/// follower's input, which is what its phase is measured over. Its reference is zero: its trim was
/// measured in a session that landed exactly where the drivers' figures put it, so what a session
/// measures is also what it is lined up by, and the arithmetic below is easy to follow.
fn cabled(master_output: i32, input: i32) -> Config {
    let mut config = both(Alignment::Aligned);
    config.devices[1].phase = Some(PhaseConfig { master_output: Some(master_output), input: Some(input), reference: Some(0) });
    config
}

/// The same cable from the master's third output into the follower's third input, with the
/// reference given, or none.
fn cabled_to(reference: Option<i32>) -> Config {
    let mut config = cabled(2, 2);
    config.devices[1].phase = Some(PhaseConfig { master_output: Some(2), input: Some(2), reference });
    config
}

/// Where the burst has to arrive for the measurement to come to `residual` samples.
///
/// The signal leaves on the block the driver settles on, held back by whatever the master's own
/// outputs are held back by, which is one block here; what the drivers' figures expect of it is the
/// master's reported output latency, the follower's reported input latency and the one block the
/// ring costs.
fn arrives_at(residual: i64) -> (u64, usize) {
    let sent_at = phase::SETTLE_BLOCKS as i64 * BLOCK as i64 + BLOCK as i64;
    let expected = QUIET_LATENCY as i64 * 2 + BLOCK as i64;
    let position = sent_at + expected + residual;
    ((position / BLOCK as i64) as u64, (position % BLOCK as i64) as usize)
}

/// Run a session, putting the measurement signal on the follower's measurement channel at the
/// block and the offset a measurement of `residual` samples would land at. `None` cables nothing up.
fn run_measuring(pc: &Arc<FakePc>, residual: Option<i64>, blocks: u64) {
    run_measuring_from(pc, residual, 0, blocks)
}

/// The same, from a block other than the first, for a session that has already driven some.
fn run_measuring_from(pc: &Arc<FakePc>, residual: Option<i64>, first: u64, blocks: u64) {
    let (a, b) = (pc.device("Device A"), pc.device("Device B"));
    let arrival = residual.map(arrives_at);
    for block in first..blocks {
        let half = (block as usize) & 1;
        let mut heard = vec![0i32; BLOCK as usize];
        if let Some((at, offset)) = arrival {
            if at == block {
                heard[offset] = phase::AMPLITUDE;
            }
        }
        // The follower's measurement channel is the last one it opened, because the file named a
        // channel outside the ones the DAW is given.
        b.set_input(2, half, &heard);
        b.fire(half);
        a.fire(half);
    }
}

/// A watcher over a driver of this PC, for the tests that read the log rather than the record.
fn watching(pc: &Arc<FakePc>, reporter: Arc<crate::status::Reporter>) -> DriverWatch {
    let host: Box<dyn Host> = Box::new(FakeHost { pc: Arc::clone(pc) });
    DriverWatch::new(&Arc::new(Mutex::new(Aggregate::new(host))), reporter)
}

#[test]
fn the_channels_a_phase_is_measured_over_are_never_the_daws() {
    let _order = daw::session();
    let pc = quiet_pc();
    let aggregate = open(&pc, cabled(2, 2));
    // Three channels each way on each device, less the master's measurement output and the
    // follower's measurement input, so nothing a DAW plays can land on either of them.
    assert_eq!(aggregate.channels(), (3 + 2, 2 + 3));
    let inputs: Vec<String> = (0..5).map(|c| aggregate.channel_info(true, c).expect("a channel").name).collect();
    assert_eq!(inputs, vec!["A 1", "A 2", "A 3", "B 1", "B 2"], "the follower's third input is the driver's own");
    let outputs: Vec<String> = (0..5).map(|c| aggregate.channel_info(false, c).expect("a channel").name).collect();
    assert_eq!(outputs, vec!["A 1", "A 2", "B 1", "B 2", "B 3"], "and the master's third output is as well");
}

#[test]
fn the_measurement_signal_goes_out_on_the_drivers_own_channel_and_nowhere_else() {
    let _order = daw::session();
    let pc = quiet_pc();
    let mut aggregate = running(&pc, cabled(2, 2));
    let (a, b) = (pc.device("Device A"), pc.device("Device B"));
    let (mut sent, mut sent_out) = (Vec::new(), Vec::new());
    for block in 0..8u64 {
        let half = (block as usize) & 1;
        b.fire(half);
        a.fire(half);
        sent.push(a.output(2, half));
        sent_out.push(a.output(0, half));
    }
    // One burst, once, on the channel the file named, and nothing at all on the ones the DAW has.
    let bursts: Vec<usize> = sent.iter().enumerate().filter(|(_, run)| run.iter().any(|&s| s != 0)).map(|(at, _)| at).collect();
    assert_eq!(bursts.len(), 1, "one signal a session, not one a block: {bursts:?}");
    assert_eq!(sent[bursts[0]][0], phase::AMPLITUDE);
    // An aligned session holds the master's own outputs back by a block, so what a device played
    // on one callback is what the DAW wrote on the one before it.
    let played = daw::with(|session| session.played.clone());
    for block in 1..8usize {
        assert_eq!(sent_out[block], played[block - 1][0], "what the DAW wrote is what came out of its own channels");
    }
    aggregate.dispose_buffers();
}

#[test]
fn a_follower_cabled_with_a_known_phase_is_lined_up_by_what_was_measured() {
    let _order = daw::session();
    let pc = quiet_pc();
    let (mut aggregate, reader, written) = reporting(&pc, cabled(2, 2));
    let reporter = Arc::clone(&aggregate.reporter);
    go(&mut aggregate);

    // Nothing has been measured yet, and the record says so rather than saying nothing.
    let seen = reader.read().expect("a settled record");
    assert_eq!(seen.devices()[1].phase_state, phase_state::MEASURING);
    assert_eq!(seen.devices()[0].phase_state, phase_state::NOT_CONFIGURED, "the master is not measured");

    // The interface's capture landed 32 samples later than its driver's figures put it, which is
    // one of the whole multiples of 32 the hardware moves by.
    run_measuring(&pc, Some(32), 80);
    let seen = reader.read().unwrap();
    let follower = seen.devices()[1];
    assert_eq!(follower.phase_state, phase_state::APPLIED);
    assert_eq!(follower.phase_measured, 32);
    assert_eq!(follower.phase_applied, 32);
    // An interface that records late means the others are held back to meet it, which is what the
    // plan already does with the figures the drivers report.
    assert_eq!(follower.pad_in, 0, "the late interface is where it is");
    assert_eq!(seen.devices()[0].pad_in, BLOCK + 32, "and the other one waits for it");
    assert_eq!(seen.driver.input_latency, QUIET_LATENCY + BLOCK + 32, "the figure in force follows what was measured");

    // One line of the log, which is the only thing that survives the session.
    let watcher = watching(&pc, reporter);
    watcher.look_around();
    watcher.look_around();
    let said = written.of("phase");
    assert_eq!(said.len(), 1, "once, not once per look: {:?}", written.lines());
    assert!(said[0].contains("B was measured at 32 samples"), "{}", said[0]);
    assert!(said[0].contains("brought 32 samples earlier"), "{}", said[0]);
    aggregate.dispose_buffers();
}

#[test]
fn a_follower_whose_cable_is_out_is_left_exactly_where_the_drivers_figures_put_it() {
    let _order = daw::session();
    let pc = quiet_pc();
    let (mut aggregate, reader, written) = reporting(&pc, cabled(2, 2));
    let reporter = Arc::clone(&aggregate.reporter);
    go(&mut aggregate);
    let was = reader.read().unwrap().driver.input_latency;

    // Nothing at all arrives on the measurement channel, for longer than the driver listens.
    run_measuring(&pc, None, 140);
    let seen = reader.read().unwrap();
    assert_eq!(seen.devices()[1].phase_state, phase_state::NOT_HEARD);
    assert_eq!(seen.devices()[1].phase_applied, 0, "a refusal to correct, not a correction of zero");
    assert_eq!(seen.driver.input_latency, was, "the session runs on the figures the drivers reported");
    assert_eq!(seen.devices()[0].pad_in, BLOCK, "and nothing was moved");

    let watcher = watching(&pc, reporter);
    watcher.look_around();
    let said = written.of("phase");
    assert_eq!(said.len(), 1, "{:?}", written.lines());
    assert!(said[0].contains("nothing arrived on its measurement channel"), "{}", said[0]);
    assert!(said[0].contains("Check the cable"), "{}", said[0]);
    aggregate.dispose_buffers();
}

#[test]
fn a_change_from_the_reference_that_is_not_near_a_whole_number_of_steps_moves_nothing_and_says_why() {
    let _order = daw::session();
    let pc = quiet_pc();
    let (mut aggregate, reader, written) = reporting(&pc, cabled(2, 2));
    let reporter = Arc::clone(&aggregate.reporter);
    go(&mut aggregate);
    let was = reader.read().unwrap().driver.input_latency;

    // Something arrives 20 samples from the reference, which is nowhere near a whole number of 32
    // sample steps, so it is a measurement of something else.
    run_measuring(&pc, Some(20), 80);
    let seen = reader.read().unwrap();
    assert_eq!(seen.devices()[1].phase_state, phase_state::OFF_THE_GRID);
    assert_eq!(seen.devices()[1].phase_measured, 20, "what was measured is still said");
    assert_eq!(seen.devices()[1].phase_applied, 0);
    assert_eq!(seen.driver.input_latency, was);

    let watcher = watching(&pc, reporter);
    watcher.look_around();
    let said = written.of("phase");
    assert!(said[0].contains("20 samples"), "{}", said[0]);
    assert!(said[0].contains("32 sample steps"), "{}", said[0]);
    aggregate.dispose_buffers();
}

#[test]
fn a_follower_with_no_phase_configuration_behaves_exactly_as_it_did_before() {
    let _order = daw::session();
    let pc = quiet_pc();
    let (mut aggregate, reader, written) = reporting(&pc, both(Alignment::Aligned));
    assert_eq!(aggregate.channels(), (6, 6), "every channel of both devices reaches the DAW");
    go(&mut aggregate);
    let was = reader.read().unwrap().driver.input_latency;

    // Even with something on the channel a measurement would have used, nothing is measured.
    run_measuring(&pc, Some(32), 80);
    let seen = reader.read().unwrap();
    assert!(seen.devices().iter().all(|device| device.phase_state == phase_state::NOT_CONFIGURED));
    assert!(seen.devices().iter().all(|device| device.phase_applied == 0 && device.phase_measured == 0));
    assert_eq!(seen.driver.input_latency, was);
    assert_eq!(seen.devices()[0].pad_in, BLOCK, "held back by exactly what the drivers' figures say");
    assert!(written.of("phase").is_empty(), "and there is nothing to say about it: {:?}", written.lines());
    aggregate.dispose_buffers();
}

#[test]
fn three_interfaces_are_each_measured_against_the_master_on_their_own_cable() {
    let _order = daw::session();
    let pc = Arc::new(
        FakePc::new()
            .with("Device A", "{AAAAAAAA-0000-0000-0000-000000000001}", r"c:\antelope\a.dll", quiet(3, 3))
            .with("Device B", "{BBBBBBBB-0000-0000-0000-000000000002}", r"c:\antelope\b.dll", quiet(3, 3))
            .with("Device C", "{CCCCCCCC-0000-0000-0000-000000000003}", r"c:\antelope\c.dll", quiet(3, 3)),
    );
    let mut config = cabled(2, 2);
    config.devices.push(DeviceConfig {
        key: Some("Device C".into()),
        name: Some("C".into()),
        phase: Some(PhaseConfig { master_output: Some(1), input: Some(2), reference: Some(0) }),
        ..DeviceConfig::default()
    });
    let (mut aggregate, reader, _written) = reporting(&pc, config);
    // Two of the master's outputs are the driver's own now, and one input of each follower.
    assert_eq!(aggregate.channels(), (3 + 2 + 2, 1 + 3 + 3));
    go(&mut aggregate);

    // Each interface's capture settled somewhere of its own, and each is measured on its own
    // cable: one landed 32 samples late, the other 64 early.
    let (a, b, c) = (pc.device("Device A"), pc.device("Device B"), pc.device("Device C"));
    let (late, early) = (arrives_at(32), arrives_at(-64));
    for block in 0..80u64 {
        let half = (block as usize) & 1;
        for (device, (at, offset)) in [(&b, late), (&c, early)] {
            let mut heard = vec![0i32; BLOCK as usize];
            if at == block {
                heard[offset] = phase::AMPLITUDE;
            }
            device.set_input(2, half, &heard);
        }
        b.fire(half);
        c.fire(half);
        a.fire(half);
    }
    let seen = reader.read().unwrap();
    assert_eq!(seen.devices()[1].phase_applied, 32, "one interface was late");
    assert_eq!(seen.devices()[2].phase_applied, -64, "and the other was early, which is its own answer");
    // The late one sets the length of everything, and the early one is held back most.
    assert_eq!(seen.driver.input_latency, QUIET_LATENCY + BLOCK + 32);
    assert_eq!(seen.devices()[1].pad_in, 0);
    assert_eq!(seen.devices()[2].pad_in, 32 + 64, "the early interface waits for the late one");
    assert_eq!(seen.devices()[0].pad_in, BLOCK + 32);
    aggregate.dispose_buffers();
}

#[test]
fn a_session_started_again_is_measured_again_rather_than_keeping_the_last_ones_answer() {
    let _order = daw::session();
    let pc = quiet_pc();
    let (mut aggregate, reader, _written) = reporting(&pc, cabled(2, 2));
    go(&mut aggregate);
    run_measuring(&pc, Some(32), 80);
    assert_eq!(reader.read().unwrap().devices()[1].phase_applied, 32);

    // A DAW that stops the audio and starts it again without letting the buffers go is measured
    // again, because the phase is a different number every session.
    aggregate.stop();
    aggregate.start().expect("the same buffers, running again");
    // The first block of the new session is what starts the measurement again, because nothing
    // outside the audio path may touch what the audio path is using.
    run_measuring_from(&pc, None, 0, 1);
    let seen = reader.read().unwrap();
    assert_eq!(seen.devices()[1].phase_state, phase_state::MEASURING, "the last session's answer is not kept");
    assert_eq!(seen.devices()[1].phase_applied, 0);
    assert_eq!(seen.devices()[0].pad_in, BLOCK, "and the padding is back to what the drivers' figures say");

    run_measuring_from(&pc, Some(-32), 1, 80);
    let seen = reader.read().unwrap();
    assert_eq!(seen.devices()[1].phase_applied, -32, "and this session's answer is its own");
    // An interface that is early is delayed. It crosses a ring, which costs a block, and it
    // measured 32 samples in front of that, so what it waits out is the rest.
    assert_eq!(seen.devices()[1].pad_in, 32 - BLOCK);
    assert_eq!(seen.devices()[0].pad_in, 0, "and the master is not held back for it at all");
    assert_eq!(seen.driver.input_latency, QUIET_LATENCY, "the master's own path, which nothing measured");
    aggregate.dispose_buffers();
}

// ---------------------------------------------------------------------------------------------
// Lining every session up to the one its trim was measured in.
// ---------------------------------------------------------------------------------------------

/// A session with a reference of its own, or none, and the phase it lands in.
fn run_against(reference: Option<i32>, residual: Option<i64>) -> (Aggregate, Reader, Written) {
    let pc = quiet_pc();
    let (mut aggregate, reader, written) = reporting(&pc, cabled_to(reference));
    go(&mut aggregate);
    run_measuring(&pc, residual, 140);
    (aggregate, reader, written)
}

/// The log's line about the phase, read by a watcher the way the driver's own thread reads it.
fn phase_line(aggregate: &Aggregate, written: &Written) -> String {
    let pc = quiet_pc();
    let watcher = watching(&pc, Arc::clone(&aggregate.reporter));
    watcher.look_around();
    let said = written.of("phase");
    assert_eq!(said.len(), 1, "{:?}", written.lines());
    said[0].clone()
}

/// **Sessions that land in different states are all lined up to the reference**, by exactly the
/// reference minus what each measured, including the one sample of wobble the hardware showed.
#[test]
fn sessions_that_land_in_different_states_are_all_lined_up_to_the_reference() {
    let _order = daw::session();
    // The reference is 40: the state the trim was measured in. The sessions below land 64, 63 and
    // 160 samples earlier than that, and one 32 later, which is what the hardware did.
    for (measured, pad_follower, pad_master) in [(40, 0, 0), (-24, 64, 0), (-23, 63, 0), (-120, 160, 0), (72, 0, 32)] {
        let (mut aggregate, reader, written) = run_against(Some(40), Some(measured));
        let seen = reader.read().unwrap();
        let follower = seen.devices()[1];
        assert_eq!(follower.phase_state, phase_state::APPLIED, "{measured}");
        assert_eq!(follower.phase_measured, measured as i32);
        assert_eq!(follower.phase_applied, measured as i32 - 40, "what was measured, less the reference, exactly");
        // The follower crosses a ring, which costs a block and is already in the plan; what the
        // measurement adds is the reference minus what it measured, on whichever side it falls.
        assert_eq!(follower.pad_in, (pad_follower - BLOCK).max(0), "{measured}");
        assert_eq!(seen.devices()[0].pad_in, (BLOCK - pad_follower).max(0) + pad_master, "{measured}");
        let said = phase_line(&aggregate, &written);
        assert!(said.contains(&format!("B was measured at {measured} samples")), "{said}");
        if measured != 40 {
            assert!(said.contains("against 40 when its trim was measured"), "{said}");
        } else {
            assert!(said.contains("nothing was moved"), "{said}");
        }
        aggregate.dispose_buffers();
    }
}

/// The change of 63 samples the hardware showed, which a rule that rounded to 32 would have made
/// 64 and a rule that asked for a multiple of 32 would have refused.
#[test]
fn a_change_of_63_samples_is_lined_up_by_63_and_not_by_64() {
    let _order = daw::session();
    let (mut aggregate, reader, written) = run_against(Some(0), Some(-63));
    let follower = reader.read().unwrap().devices()[1];
    assert_eq!(follower.phase_state, phase_state::APPLIED);
    assert_eq!(follower.phase_applied, -63);
    assert_eq!(follower.pad_in, 63 - BLOCK, "held back by 63, less the block its ring already costs");
    assert!(phase_line(&aggregate, &written).contains("held back by 63 samples"));
    aggregate.dispose_buffers();
}

/// **No reference, nothing applied**, and the log says the interface needs measuring once.
#[test]
fn with_no_reference_a_session_is_measured_and_left_where_the_drivers_figures_put_it() {
    let _order = daw::session();
    let (mut aggregate, reader, written) = run_against(None, Some(-148));
    let seen = reader.read().unwrap();
    assert_eq!(seen.devices()[1].phase_state, phase_state::NO_REFERENCE);
    assert_eq!(seen.devices()[1].phase_measured, -148, "what it measured is still said");
    assert_eq!(seen.devices()[1].phase_applied, 0);
    assert_eq!(seen.devices()[0].pad_in, BLOCK, "held back by exactly what the drivers' figures say");
    assert_eq!(seen.driver.input_latency, QUIET_LATENCY + BLOCK);
    let said = phase_line(&aggregate, &written);
    assert!(said.contains("B was measured at -148 samples"), "{said}");
    assert!(said.contains("there is no phase from the session its trim was measured in"), "{said}");
    assert!(said.contains("Measure the interfaces once"), "{said}");
    assert!(said.contains("the figures the drivers reported"), "{said}");
    aggregate.dispose_buffers();
}

/// A correction bigger than the room the delays keep for one is refused, and nothing moves.
#[test]
fn a_correction_past_the_room_the_delays_have_is_refused_and_moves_nothing() {
    let _order = daw::session();
    let (mut aggregate, reader, written) = run_against(Some(-(phase::LIMIT + 88)), Some(0));
    let seen = reader.read().unwrap();
    assert_eq!(seen.devices()[1].phase_state, phase_state::TOO_FAR);
    assert_eq!(seen.devices()[1].phase_applied, 0);
    assert_eq!(seen.devices()[0].pad_in, BLOCK);
    assert_eq!(seen.driver.input_latency, QUIET_LATENCY + BLOCK);
    let said = phase_line(&aggregate, &written);
    assert!(said.contains("further from the phase its trim was measured at"), "{said}");
    aggregate.dispose_buffers();
}

/// **A calibration run measures the phase and moves nothing**, so the click lag it measures is
/// the raw one, and the phase is there to be paired with the trim that lag becomes.
#[test]
fn a_session_measuring_trims_measures_the_phase_and_moves_nothing_for_it() {
    let _order = daw::session();
    let pc = quiet_pc();
    // A reference is there to be used, and it is not: this session is where the next one comes from.
    let (mut aggregate, reader, written) = reporting(&pc, cabled_to(Some(0)));
    aggregate.measure_trims();
    go(&mut aggregate);
    let was = reader.read().unwrap().driver.input_latency;
    run_measuring(&pc, Some(-148), 140);
    let seen = reader.read().unwrap();
    assert_eq!(seen.devices()[1].phase_state, phase_state::MEASURED_ONLY);
    assert_eq!(seen.devices()[1].phase_measured, -148);
    assert_eq!(seen.devices()[1].phase_applied, 0);
    assert_eq!(seen.devices()[0].pad_in, BLOCK, "nothing was moved");
    assert_eq!(seen.driver.input_latency, was);
    let said = phase_line(&aggregate, &written);
    assert!(said.contains("B was measured at -148 samples") && said.contains("on purpose"), "{said}");
    aggregate.dispose_buffers();
}

/// What the interfaces report for the table below: enough that the earliest session the hardware
/// showed still hears its signal after it was sent, and little enough that the latest one still
/// hears it inside the window.
const TABLE_LATENCY: i32 = 200;

/// The part of the click lag that is not the phase, in whole samples: the hardware's was 144.4
/// in every run, and a fake that works in whole samples drops the fraction.
const NOT_THE_PHASE: i64 = 144;

/// **The hardware's own sessions, reproduced.** A follower whose capture phase moves between
/// sessions exactly as the real one's did, measured in every session against the reference from
/// the run whose trim was measured, ends every session with the same click lag. With no reference
/// the same sessions scatter exactly as the hardware's did.
///
/// The fake's click lag is the phase plus 144, as the hardware's was. The trim is what the first
/// run's click lag came to with nothing moved and no trim in force, which is what a calibration run
/// in that session offers, and the reference is the phase measured beside it, -84.
#[test]
fn the_hardwares_sessions_all_end_with_the_same_click_lag_once_lined_up_to_the_reference() {
    let _order = daw::session();
    const REFERENCE: i32 = -84;
    // Runs 1 to 6 of the table, and the three before them.
    let phases: [i64; 9] = [-84, -148, -148, -147, -307, -307, -244, -307, -307];

    let session = |phase: i64, reference: Option<i32>, trim: i32| -> (i64, u32, i32) {
        let spec = || {
            Spec { min: 2, max: 64, preferred: BLOCK, rate: QUIET_RATE, rates: vec![QUIET_RATE], ..Spec::default() }
                .with_channels(3, 3)
                .with_latency(TABLE_LATENCY, TABLE_LATENCY)
        };
        let pc = Arc::new(
            FakePc::new()
                .with("Device A", "{AAAAAAAA-0000-0000-0000-000000000001}", r"c:\antelope\a.dll", spec())
                .with("Device B", "{BBBBBBBB-0000-0000-0000-000000000002}", r"c:\antelope\b.dll", spec()),
        );
        let mut config = cabled_to(reference);
        config.devices[1].input_trim = Some(trim);
        let (mut aggregate, reader, _written) = reporting(&pc, config);
        daw::with(|daw| daw.heard.clear());
        go(&mut aggregate);

        // The signal leaves on the block the driver settles on, after the master's output delay,
        // which is one block; what the drivers' figures expect of it is both reported latencies and
        // the block the ring costs, and the trim is no part of it.
        let sent_at = (phase::SETTLE_BLOCKS as i64 + 1) * BLOCK as i64;
        let burst = sent_at + 2 * TABLE_LATENCY as i64 + BLOCK as i64 + phase;
        // One click, well after the measurement has settled, into both interfaces: the follower's
        // copy lands by the part of the lag that is not the phase, and then by the phase.
        let click_a = 800i64;
        let click_b = click_a + NOT_THE_PHASE + phase;
        let (a, b) = (pc.device("Device A"), pc.device("Device B"));
        let at = |position: i64, block: i64| -> Vec<i32> {
            let mut run = vec![0i32; BLOCK as usize];
            if position >= block * BLOCK as i64 && position < (block + 1) * BLOCK as i64 {
                run[(position - block * BLOCK as i64) as usize] = phase::AMPLITUDE;
            }
            run
        };
        for block in 0..400i64 {
            let half = (block as usize) & 1;
            a.set_input(0, half, &at(click_a, block));
            b.set_input(0, half, &at(click_b, block));
            b.set_input(2, half, &at(burst, block));
            b.fire(half);
            a.fire(half);
        }
        let heard = daw::with(|daw| daw.heard.clone());
        let arrived = |channel: usize| -> i64 {
            let flat: Vec<i32> = heard.iter().flat_map(|block| block[channel].iter().copied()).collect();
            flat.iter().position(|&sample| sample != 0).expect("the click reached the DAW") as i64
        };
        // The DAW's channels are A's three inputs and then B's first two: the third is the driver's.
        let lag = arrived(3) - arrived(0);
        let follower = reader.read().unwrap().devices()[1];
        aggregate.dispose_buffers();
        (lag, follower.phase_state, follower.phase_measured)
    };

    // The first run, measuring its trim: nothing is lined up and no trim is in force, so the lag
    // it hears is the whole of it, and the phase beside it is the reference.
    let (trim, state, measured) = session(REFERENCE as i64, None, 0);
    assert_eq!((state, measured), (phase_state::NO_REFERENCE, REFERENCE));
    let trim = trim as i32;

    for phase in phases {
        let (lag, state, measured) = session(phase, Some(REFERENCE), trim);
        assert_eq!(state, phase_state::APPLIED, "a phase of {phase}");
        assert_eq!(measured as i64, phase, "the measurement tracks the phase to the sample");
        assert_eq!(lag, 0, "a phase of {phase}: every session ends where the trim was measured, and the trim then cancels it");
    }

    // The same sessions with nothing to line them up to scatter exactly as the table did, by how
    // far each phase moved from the reference: the trim only holds in the state it was measured in.
    for phase in phases {
        let (lag, state, _) = session(phase, None, trim);
        assert_eq!(state, phase_state::NO_REFERENCE);
        assert_eq!(lag, phase - REFERENCE as i64, "a phase of {phase}");
    }
}

// ---------------------------------------------------------------------------------------------
// Drivers that say yes to a rate and do not move.
//
// The Antelope drivers take a rate the first time they are asked, which the hardware showed on
// 2026-09-22. These are the drivers that do not, and what the aggregate does about each: read the
// rate back, ask again, close the driver and open it again, and refuse honestly when nothing works.
// ---------------------------------------------------------------------------------------------

/// Two devices on 44.1 kHz, "B" with a driver that takes a rate the way `takes` says.
fn at_44k(takes: Takes) -> Arc<FakePc> {
    let pc = Arc::new(
        FakePc::new()
            .with("Device A", "{AAAAAAAA-0000-0000-0000-000000000001}", r"c:\antelope\a.dll", spec(2, 2, 600, 700))
            .with("Device B", "{BBBBBBBB-0000-0000-0000-000000000002}", r"c:\antelope\b.dll", spec(2, 2, 600, 700).taking(takes)),
    );
    for key in ["Device A", "Device B"] {
        pc.device(key).spec.lock().unwrap().rate = 44_100.0;
    }
    pc
}

/// Both devices, with 96 kHz in the file.
fn at_96k() -> Config {
    Config { rate: Some(96_000.0), ..both(Alignment::Aligned) }
}

/// A driver that takes a rate the first time is asked once, read once, and says nothing.
#[test]
fn a_driver_that_takes_the_rate_at_once_is_not_opened_again_and_nothing_is_written() {
    let _order = daw::session();
    let pc = at_44k(Takes::AtOnce);
    let (mut aggregate, _reader, written) = reporting(&pc, at_96k());
    go(&mut aggregate);
    for key in ["Device A", "Device B"] {
        assert_eq!(pc.device(key).running_rate(), 96_000.0, "{key}");
        assert_eq!(pc.device(key).opens(), 1, "{key} was opened once");
        assert_eq!(pc.device(key).calls().iter().filter(|call| call.starts_with("set_rate")).count(), 1, "{key} was asked once");
    }
    assert!(written.of("rate").is_empty(), "{:?}", written.lines());
    aggregate.dispose_buffers();
}

/// (a) It says yes and keeps the old rate until it is opened again.
#[test]
fn a_driver_that_holds_its_rate_until_it_is_reopened_is_reopened_and_takes_it() {
    let _order = daw::session();
    let pc = at_44k(Takes::WhenReopened);
    let (mut aggregate, _reader, written) = reporting(&pc, at_96k());
    assert_eq!(pc.device("Device B").opens(), 2, "opened once more");
    assert_eq!(pc.device("Device A").opens(), 1, "and only B");
    assert_eq!(pc.device("Device B").running_rate(), 96_000.0);
    assert_eq!(
        written.of("rate"),
        ["2026-09-20 21:14:07 rate B's driver held 44.1 kHz after being asked for 96 kHz; reopened it and it took 96 kHz"]
    );
    go(&mut aggregate);
    assert!(aggregate.is_started());
    assert_eq!(written.of("rate").len(), 1, "one line for one step: {:?}", written.lines());
    aggregate.dispose_buffers();
}

/// (b) It says yes and moves only once its buffers are made. Opening it again does not help, so
/// the aggregate says it is still waiting, and looks again after the buffers.
#[test]
fn a_driver_that_takes_the_rate_with_its_buffers_is_held_to_it_there() {
    let _order = daw::session();
    let pc = at_44k(Takes::WithBuffers);
    let (mut aggregate, _reader, written) = reporting(&pc, at_96k());
    assert_eq!(pc.device("Device B").opens(), 3, "opened again twice, and no further");
    assert_eq!(
        written.of("rate"),
        ["2026-09-20 21:14:07 rate B's driver held 44.1 kHz after being asked for 96 kHz, and still held 44.1 kHz after two reopens; the aggregate asks again once its buffers are made"]
    );
    go(&mut aggregate);
    assert!(aggregate.is_started());
    assert_eq!(pc.device("Device B").running_rate(), 96_000.0);
    assert_eq!(pc.device("Device B").opens(), 3, "the buffers settled it without another reopen");
    assert_eq!(
        written.of("rate")[1],
        "2026-09-20 21:14:07 rate B's driver held 44.1 kHz until its buffers were made, and then took 96 kHz"
    );
    aggregate.dispose_buffers();
}

/// (c) It says yes, reports the new rate, and asks to be reset the moment it has anyone to ask,
/// which is once its buffers are made. The aggregate is still opening, so the request is its own to
/// answer: the driver is opened again, its buffers made again, and nothing reaches the DAW.
#[test]
fn a_driver_that_asks_to_be_reset_after_a_rate_is_reopened_before_anything_starts() {
    let _order = daw::session();
    let pc = at_44k(Takes::AfterReset);
    let (mut aggregate, _reader, written) = reporting(&pc, at_96k());
    assert!(written.of("rate").is_empty(), "it said it took the rate: {:?}", written.lines());
    assert_eq!(pc.device("Device B").running_rate(), 44_100.0, "and it has not, yet");
    go(&mut aggregate);
    assert!(aggregate.is_started());
    assert_eq!(pc.device("Device B").opens(), 2);
    assert_eq!(pc.device("Device B").running_rate(), 96_000.0);
    assert!(pc.device("Device B").has_buffers(), "its buffers were made again");
    assert_eq!(
        written.of("rate"),
        ["2026-09-20 21:14:07 rate B's driver asked to be reset after being asked for 96 kHz once its buffers were made; reopened it and it took 96 kHz"]
    );
    let messages = daw::with(|session| session.messages.clone());
    assert!(!messages.contains(&(selector::RESET_REQUEST, 0)), "the DAW was not asked: {messages:?}");

    // The audio runs through the buffers made the second time.
    feed(&pc, "Device B", 0, 0, 500);
    pc.device("Device B").fire(0);
    pc.device("Device A").fire(0);
    assert_eq!(daw::with(|session| session.calls), 1);
    aggregate.dispose_buffers();
}

/// (d) It says yes and never moves. It is opened again the most times allowed, where it was asked
/// and again after its buffers, and then the aggregate refuses to open rather than run at two rates.
#[test]
fn a_driver_that_never_takes_the_rate_is_refused_after_the_reopens_and_nothing_is_left_open() {
    let _order = daw::session();
    let pc = at_44k(Takes::Never);
    let (mut aggregate, reader, written) = reporting(&pc, at_96k());
    let wanted = everything(&aggregate);
    let why = aggregate.create_buffers(&wanted, BLOCK, daw::callbacks()).expect_err("B never moved");
    assert_eq!(why, "B's driver still held 44.1 kHz after being asked for 96 kHz and reopened twice, so the aggregate did not open");
    assert_eq!(pc.device("Device B").opens(), 5, "twice where it was asked and twice after its buffers, and no more");
    assert_eq!(written.of("refused"), [format!("2026-09-20 21:14:07 refused {why}")]);
    assert_eq!(reader.read().unwrap().driver.refusal.get(), why);
    for key in ["Device A", "Device B"] {
        assert!(!pc.device(key).has_buffers(), "{key} was left holding its buffers");
        assert!(!pc.device(key).is_started());
    }
    assert!(!aggregate.has_buffers());
    assert!(aggregate.start().is_err(), "and nothing starts");
}

/// A DAW moving the rate, rather than the file: the same holding, through `setSampleRate`.
#[test]
fn a_rate_a_daw_asks_for_is_held_the_same_way() {
    let _order = daw::session();
    let pc = at_44k(Takes::WhenReopened);
    let (mut aggregate, _reader, written) = reporting(&pc, both(Alignment::Aligned));
    aggregate.set_rate(48_000.0).expect("B moves once reopened");
    assert_eq!(pc.device("Device B").running_rate(), 48_000.0);
    assert_eq!(pc.device("Device B").opens(), 2);
    assert_eq!(aggregate.rate(), 48_000.0);
    assert_eq!(written.of("rate").len(), 1, "{:?}", written.lines());
    go(&mut aggregate);
    aggregate.dispose_buffers();
}

/// A rate asked for while a DAW has the buffers made: nothing can be closed under them, so a driver
/// that did not move is the DAW's to reset, and the aggregate opens it again when the DAW comes back
/// for its buffers.
#[test]
fn a_driver_that_holds_a_rate_asked_for_with_the_buffers_made_is_reset_through_the_daw() {
    let _order = daw::session();
    let pc = at_44k(Takes::WhenReopened);
    let (mut aggregate, _reader, written) = reporting(&pc, both(Alignment::Aligned));
    let wanted = everything(&aggregate);
    aggregate.create_buffers(&wanted, BLOCK, daw::callbacks()).expect("the buffers are made");
    aggregate.set_rate(96_000.0).expect("said yes");
    assert_eq!(pc.device("Device B").opens(), 1, "nothing was closed under the buffers");
    let messages = daw::with(|session| session.messages.clone());
    assert!(messages.contains(&(selector::RESET_REQUEST, 0)), "the DAW was asked to reset: {messages:?}");

    // The DAW does as it was asked.
    aggregate.dispose_buffers();
    aggregate.create_buffers(&wanted, BLOCK, daw::callbacks()).expect("and B is opened again there");
    assert_eq!(pc.device("Device B").opens(), 2);
    assert_eq!(pc.device("Device B").running_rate(), 96_000.0);
    let lines = written.of("rate");
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(lines[1].ends_with("B's driver held 44.1 kHz after being asked for 96 kHz once its buffers were made; reopened it and it took 96 kHz"), "{lines:?}");
    aggregate.dispose_buffers();
}

/// A driver saying its rate moved before anything starts is asked again, and one that goes back to
/// the rate it was asked for needs nothing more.
#[test]
fn a_driver_that_says_its_rate_moved_before_the_start_is_asked_again() {
    let _order = daw::session();
    let pc = at_44k(Takes::AtOnce);
    let (mut aggregate, _reader, written) = reporting(&pc, at_96k());
    // Something moved it back, and it said so, with no DAW yet to tell.
    pc.device("Device A").spec.lock().unwrap().rate = 44_100.0;
    pc.device("Device A").say_rate_moved(44_100.0);
    go(&mut aggregate);
    assert_eq!(pc.device("Device A").running_rate(), 96_000.0);
    assert_eq!(pc.device("Device A").opens(), 1, "asking again was enough");
    assert_eq!(
        written.of("rate"),
        ["2026-09-20 21:14:07 rate A's driver was at 44.1 kHz once its buffers were made; asked it again and it took 96 kHz"]
    );
    aggregate.dispose_buffers();
}

/// While the audio runs, a driver's reset request is the DAW's to act on, as it is for any host
/// driver: it goes up, and nothing is closed from inside a callback.
#[test]
fn a_reset_request_while_running_goes_to_the_daw_and_nothing_is_reopened() {
    let _order = daw::session();
    let pc = at_44k(Takes::AtOnce);
    let (mut aggregate, _reader, written) = reporting(&pc, at_96k());
    go(&mut aggregate);
    pc.device("Device B").ask_for_reset();
    let messages = daw::with(|session| session.messages.clone());
    assert!(messages.contains(&(selector::RESET_REQUEST, 0)), "{messages:?}");
    assert_eq!(pc.device("Device B").opens(), 1);
    assert!(aggregate.is_started());
    assert!(written.of("rate").is_empty());
    aggregate.dispose_buffers();
}

/// Three devices not on one rate, a stubborn one in the middle, and the last refusing: each goes
/// back to its own rate, including the stubborn one, which needs opening again to go back as it
/// did to move.
#[test]
fn a_refused_rate_puts_a_stubborn_driver_back_on_its_own_rate_too() {
    let _order = daw::session();
    let pc = Arc::new(
        FakePc::new()
            .with("Device A", "{AAAAAAAA-0000-0000-0000-000000000001}", r"c:\antelope\a.dll", spec(2, 2, 600, 700))
            .with("Device B", "{BBBBBBBB-0000-0000-0000-000000000002}", r"c:\antelope\b.dll", spec(2, 2, 600, 700).taking(Takes::WhenReopened))
            .with("Device C", "{CCCCCCCC-0000-0000-0000-000000000003}", r"c:\antelope\c.dll", spec(2, 2, 600, 700)),
    );
    pc.device("Device B").spec.lock().unwrap().rate = 88_200.0;
    let three = Config {
        devices: ["A", "B", "C"].iter().map(|name| DeviceConfig { key: Some(format!("Device {name}")), name: Some((*name).into()), ..DeviceConfig::default() }).collect(),
        ..Config::default()
    };
    let mut aggregate = open(&pc, three);
    pc.device("Device C").spec.lock().unwrap().fails_at = Some((Step::SetRate, "no".into()));
    aggregate.set_rate(48_000.0).expect_err("the third refused");
    assert_eq!(pc.device("Device A").running_rate(), 96_000.0, "A is back on its own rate");
    assert_eq!(pc.device("Device B").running_rate(), 88_200.0, "and B on its own, having been opened again for it");
    assert_eq!(pc.device("Device B").opens(), 3, "once to move, once to move back");
    assert!(aggregate.is_initialised(), "every device is still open");
}

/// A stubborn driver that will not open again at all: the aggregate refuses, puts the one that
/// moved back where it was, and lets everything go rather than answer for a device it has lost.
#[test]
fn a_stubborn_driver_that_will_not_open_again_is_a_refusal_and_the_rest_go_back() {
    let _order = daw::session();
    let pc = at_44k(Takes::WhenReopened);
    pc.device("Device A").spec.lock().unwrap().rate = 88_200.0;
    let mut aggregate = open(&pc, both(Alignment::Aligned));
    pc.device("Device B").spec.lock().unwrap().fails_at = Some((Step::Init, "not now".into()));
    let why = aggregate.set_rate(48_000.0).expect_err("B could not be opened again");
    assert_eq!(why, "B refused to start up again: not now");
    assert_eq!(pc.device("Device A").running_rate(), 88_200.0, "A is back on its own rate");
    assert!(!aggregate.is_initialised(), "nothing is left pretending B is there");
}
