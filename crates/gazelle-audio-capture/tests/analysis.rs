//! Analysis on synthetic sessions: attribution recovers exactly the planted fields.

use gazelle_audio_capture::analysis::attribute::{attribute_commands, attribute_readback, MaskedByte};
use gazelle_audio_capture::analysis::channel::{ChannelKey, Channels};
use gazelle_audio_capture::analysis::noise::{noise_model, ByteClass, Checksum};
use gazelle_audio_capture::analysis::segment::{segments, Segment};
use gazelle_audio_capture::capture::event::{Direction, TransferType};
use gazelle_audio_capture::capture::pipeline::PayloadPolicy;
use gazelle_audio_capture::session::marks::Mark;
use gazelle_audio_capture::session::model::{Parameter, ParameterDomain, ParameterKind, ProbePlan};
use gazelle_audio_capture::session::plan::StepKind;
use gazelle_audio_capture::session::step::StepTiming;
use gazelle_audio_capture::session::timeline::{probe_timelines, ProbeTimeline};
use gazelle_audio_capture::synth::device::*;
use gazelle_audio_capture::synth::session::{generate_session, ScriptedOperator, SynthResult, SynthSpec};

const VID: u16 = 0x1234;
const PID: u16 = 0xABCD;
const LEVELS: &[&str] = &["0 dB", "-6 dB", "-12 dB"];

fn parameters() -> Vec<Parameter> {
    vec![
        Parameter { id: "monitor_level".into(), label: "Monitor level".into(), kind: ParameterKind::Discrete, domain: ParameterDomain { values: LEVELS.iter().map(|s| s.to_string()).collect(), unit: Some("dB".into()) }, location: String::new() },
        Parameter { id: "mute".into(), label: "Mute".into(), kind: ParameterKind::Toggle, domain: ParameterDomain { values: vec!["off".into(), "on".into()], unit: None }, location: String::new() },
    ]
}

fn simple_target() -> SimpleDevice {
    SimpleDevice::new(VID, PID, 1, 5).with_parameter("monitor_level", LEVELS).with_parameter("mute", &["off", "on"])
}

fn rich_target() -> RichDevice {
    RichDevice::new(VID, PID, 1, 5).with_parameter("monitor_level", LEVELS).with_parameter("mute", &["off", "on"])
}

fn generate(dir: &std::path::Path, operator: ScriptedOperator, target: Box<dyn DeviceModel>) -> SynthResult {
    let spec = SynthSpec {
        parameters: parameters(),
        plan: ProbePlan { parameter: "monitor_level".into(), value_a: "0 dB".into(), value_b: vec!["-6 dB".into(), "-12 dB".into()], sweep: vec![], repeats: 3, control_parameter: "mute".into() },
        seed: 7,
        timing: StepTiming::default(),
        start_ns: 1_700_000_000_000_000_000,
        operator,
        link_type: 249,
        policy: PayloadPolicy::default(),
    };
    let devices: Vec<Box<dyn DeviceModel>> = vec![target, Box::new(SimpleDevice::new(0x046D, 0xC52B, 1, 3))];
    generate_session(&dir.join("session"), spec, devices).unwrap()
}

struct Analysed {
    result: SynthResult,
    marks: Vec<Mark>,
}

impl Analysed {
    fn new(dir: &std::path::Path, operator: ScriptedOperator, target: Box<dyn DeviceModel>) -> Self {
        let result = generate(dir, operator, target);
        let marks = result.store.marks().unwrap();
        Self { result, marks }
    }

    fn timeline(&self) -> ProbeTimeline {
        probe_timelines(&self.marks).remove(0)
    }

    fn segments<'a>(&'a self, timeline: &ProbeTimeline) -> Vec<Segment<'a>> {
        segments(timeline, &self.result.events)
    }

    fn channels(&self) -> Channels {
        Channels::detect(&self.result.events)
    }
}

fn simple_monitor_channel() -> ChannelKey {
    let planted = simple_target().planted("monitor_level").unwrap();
    assert_eq!(planted.request, SIMPLE_REQUEST);
    ChannelKey { endpoint: 0, direction: Direction::Out, transfer: TransferType::Control, request: Some(planted.request), index: Some(planted.index), discriminator: None }
}

fn rich_channel(endpoint: u8, direction: Direction, discriminator: u8) -> ChannelKey {
    ChannelKey { endpoint, direction, transfer: TransferType::Interrupt, request: None, index: None, discriminator: Some(discriminator) }
}

#[test]
fn command_attribution_recovers_the_planted_control_field() {
    let dir = tempfile::tempdir().unwrap();
    let a = Analysed::new(dir.path(), ScriptedOperator::default(), Box::new(simple_target()));
    let timeline = a.timeline();
    let segs = a.segments(&timeline);
    assert_eq!(segs.len(), timeline.steps.len(), "every step closed");

    let attribution = attribute_commands(&segs, &a.channels(), "monitor_level");
    // SimpleDevice carries the value in wValue (message byte 0) and in data byte 1 (message
    // byte 2 + 1); wValue's high byte and data byte 0 (the parameter index) never vary.
    let planted = simple_target().planted("monitor_level").unwrap();
    let expected = vec![(simple_monitor_channel(), 0), (simple_monitor_channel(), 2 + planted.data_byte)];
    assert_eq!(attribution.fields.iter().map(|f| (f.channel, f.byte)).collect::<Vec<_>>(), expected);
    assert!(attribution.shared.is_empty());

    let device = simple_target();
    let set_steps = segs.iter().filter(|s| s.kind == StepKind::Set).count();
    for field in &attribution.fields {
        assert_eq!(field.values.len(), LEVELS.len());
        for (value, raw) in &field.values {
            assert_eq!(Some(*raw), device.raw_value("monitor_level", value), "{value}");
        }
        // Raw values 0, 1, 2 differ only in bits 0 and 1.
        assert_eq!(field.bits, (0, 1));
        assert_eq!(field.template, "?? 00 01 ??");
        assert_eq!(field.evidence.len(), set_steps);
    }

    // The control parameter is only ever changed by Control steps, so nothing is attributed to it.
    assert_eq!(attribute_commands(&segs, &a.channels(), "mute"), Default::default());
}

#[test]
fn attribution_survives_redo_and_a_skipped_step() {
    let dir = tempfile::tempdir().unwrap();
    let operator = ScriptedOperator { redo_once: vec![3], skip: vec![(7, "control stuck".into())], ..ScriptedOperator::default() };
    let a = Analysed::new(dir.path(), operator, Box::new(simple_target()));
    let timeline = a.timeline();
    assert_eq!(timeline.redos.get(&3), Some(&1));
    let segs = a.segments(&timeline);
    assert_eq!(segs.len(), timeline.steps.len() - 1, "the skipped step has no window");
    let attribution = attribute_commands(&segs, &a.channels(), "monitor_level");
    assert_eq!(attribution.fields.iter().map(|f| f.byte).collect::<Vec<_>>(), vec![0, 3]);
}

#[test]
fn spillover_from_the_control_parameter_is_reported_shared_not_attributed() {
    let dir = tempfile::tempdir().unwrap();
    let target = rich_target().with_spillover("mute", "monitor_level");
    let a = Analysed::new(dir.path(), ScriptedOperator::default(), Box::new(target));
    let timeline = a.timeline();
    let segs = a.segments(&timeline);
    let channels = a.channels();
    let noise = noise_model(&segs, &channels);
    let position = rich_target().position("monitor_level").unwrap();

    let commands = attribute_commands(&segs, &channels, "monitor_level");
    assert!(commands.fields.is_empty(), "{:?}", commands.fields);
    let set = rich_channel(RICH_COMMAND_ENDPOINT, Direction::Out, RICH_SET);
    assert_eq!(commands.shared.iter().map(|f| (f.channel, f.byte)).collect::<Vec<_>>(), vec![(set, RICH_VALUE)]);
    assert_eq!(commands.shared[0].bits, (0, 1), "bits come from the Set steps, not the spilled value");

    // The spilled value persists into the following Idle step; that is the device's new state,
    // not an Idle change, so readback is reported shared rather than rejected.
    let readback = attribute_readback(&segs, &channels, &noise, "monitor_level");
    assert!(readback.fields.is_empty(), "{:?}", readback.fields);
    let status = rich_channel(RICH_STATUS_ENDPOINT, Direction::In, RICH_STATUS);
    assert_eq!(
        readback.shared.iter().map(|f| (f.channel, f.byte)).collect::<Vec<_>>(),
        vec![(status, RICH_READBACK_BASE + position), (status, RICH_PEAK_BASE + position)]
    );

    assert_eq!(attribute_commands(&segs, &channels, "mute").fields, vec![]);
}

#[test]
fn discriminators_split_the_rich_device_endpoints() {
    let dir = tempfile::tempdir().unwrap();
    let a = Analysed::new(dir.path(), ScriptedOperator::default(), Box::new(rich_target()));
    let channels = a.channels();
    assert!(channels.is_discriminated(RICH_COMMAND_ENDPOINT, Direction::Out, TransferType::Interrupt));
    assert!(channels.is_discriminated(RICH_STATUS_ENDPOINT, Direction::In, TransferType::Interrupt));
    assert!(!channels.is_discriminated(0, Direction::In, TransferType::Control), "control transfers are keyed by setup, not byte 0");
}

#[test]
fn noise_model_classifies_the_status_report() {
    let dir = tempfile::tempdir().unwrap();
    let a = Analysed::new(dir.path(), ScriptedOperator::default(), Box::new(rich_target()));
    let timeline = a.timeline();
    let segs = a.segments(&timeline);
    let noise = noise_model(&segs, &a.channels());

    let status = noise.channel(&rich_channel(RICH_STATUS_ENDPOINT, Direction::In, RICH_STATUS)).expect("status reports while idle");
    assert!(status.messages >= 100, "{} idle status reports", status.messages);
    assert_eq!(status.classes.len(), RICH_REPORT_LEN);
    assert_eq!(status.classes[0], ByteClass::Constant);
    assert_eq!(status.classes[RICH_COUNTER], ByteClass::Counter { step: 1 });
    assert_eq!(status.classes[RICH_METER], ByteClass::Noisy);
    assert_eq!(status.classes[RICH_METER + 1], ByteClass::Constant);
    assert_eq!(status.classes[RICH_READBACK_BASE], ByteClass::Constant, "readback does not change while idle");
    assert_eq!(status.classes[RICH_CHECKSUM], ByteClass::Derived { checksum: Checksum::Sum8 });

    let meters = noise.channel(&rich_channel(RICH_STATUS_ENDPOINT, Direction::In, RICH_METERS)).unwrap();
    assert_eq!(meters.classes[0], ByteClass::Constant);
    assert!(meters.classes[1..].iter().all(|c| *c == ByteClass::Noisy), "{:?}", meters.classes);

    assert!(noise.channel(&rich_channel(RICH_COMMAND_ENDPOINT, Direction::Out, RICH_SET)).is_none(), "no commands while idle");
}

#[test]
fn rich_device_command_and_readback_are_attributed_with_noise_masked() {
    let dir = tempfile::tempdir().unwrap();
    let a = Analysed::new(dir.path(), ScriptedOperator::default(), Box::new(rich_target()));
    let timeline = a.timeline();
    let segs = a.segments(&timeline);
    let channels = a.channels();
    let device = rich_target();
    let position = device.position("monitor_level").unwrap();

    // Commands: only the Set message's value byte. The sequence number never repeats per value,
    // the Commit message carries no value, the Focus message only appears on No-op steps, and
    // the Control step's Set for `mute` has a different id byte, so it does not match the
    // template and cannot make the field look shared.
    let commands = attribute_commands(&segs, &channels, "monitor_level");
    let set = rich_channel(RICH_COMMAND_ENDPOINT, Direction::Out, RICH_SET);
    assert_eq!(commands.fields.iter().map(|f| (f.channel, f.byte)).collect::<Vec<_>>(), vec![(set, RICH_VALUE)]);
    assert!(commands.shared.is_empty(), "{:?}", commands.shared);
    let field = &commands.fields[0];
    assert!(field.template.starts_with(&format!("70 ?? {:02x} ?? 00", position + 1)), "{}", field.template);
    for (value, raw) in &field.values {
        assert_eq!(Some(*raw), device.raw_value("monitor_level", value));
    }

    // Readback: the parameter's status byte; counter, meter and checksum are masked, not attributed.
    let noise = noise_model(&segs, &channels);
    let readback = attribute_readback(&segs, &channels, &noise, "monitor_level");
    let status = rich_channel(RICH_STATUS_ENDPOINT, Direction::In, RICH_STATUS);
    // Both the readback byte and the peak meter settle with the value; only the meter dips and
    // recovers within a step.
    assert_eq!(
        readback.fields.iter().map(|f| (f.channel, f.byte, f.oscillates)).collect::<Vec<_>>(),
        vec![(status, RICH_READBACK_BASE + position, false), (status, RICH_PEAK_BASE + position, true)]
    );
    assert!(readback.shared.is_empty());
    assert!(commands.fields.iter().all(|f| !f.oscillates), "one Set per change never revisits a value");
    for masked in [
        MaskedByte { channel: status, byte: RICH_COUNTER, class: ByteClass::Counter { step: 1 } },
        MaskedByte { channel: status, byte: RICH_METER, class: ByteClass::Noisy },
        MaskedByte { channel: status, byte: RICH_CHECKSUM, class: ByteClass::Derived { checksum: Checksum::Sum8 } },
    ] {
        assert!(readback.masked.contains(&masked), "{masked:?} in {:?}", readback.masked);
    }
    for (value, raw) in &readback.fields[0].values {
        assert_eq!(Some(*raw), device.raw_value("monitor_level", value));
    }
}
