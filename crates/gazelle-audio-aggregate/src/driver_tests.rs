//! The driver, end to end, against devices and a DAW made of data.
//!
//! Nothing here opens a driver, starts a converter or touches a registry. The fakes hand out real
//! memory and the tests fire their callbacks themselves, so the code under test is the same code
//! that will run at the hardware.

use std::sync::Arc;

use crate::aggregate::{Aggregate, Wanted};
use crate::config::{Alignment, Config, DeviceConfig};
use crate::daw;
use crate::fake::{FakeHost, FakePc, Spec, Step};
use crate::sub::Host;
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

    // A small trim moves the device that carries the longest path, so the figure follows it.
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
