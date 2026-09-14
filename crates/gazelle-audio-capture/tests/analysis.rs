//! Analysis on synthetic sessions: attribution recovers exactly the planted fields.

use gazelle_audio_capture::analysis::attribute::attribute_commands;
use gazelle_audio_capture::analysis::channel::ChannelKey;
use gazelle_audio_capture::analysis::segment::segments;
use gazelle_audio_capture::capture::event::{Direction, TransferType};
use gazelle_audio_capture::capture::pipeline::PayloadPolicy;
use gazelle_audio_capture::session::model::{Parameter, ParameterDomain, ParameterKind, ProbePlan};
use gazelle_audio_capture::session::plan::StepKind;
use gazelle_audio_capture::session::step::StepTiming;
use gazelle_audio_capture::session::timeline::probe_timelines;
use gazelle_audio_capture::synth::device::{DeviceModel, SimpleDevice, SIMPLE_REQUEST};
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

fn target() -> SimpleDevice {
    SimpleDevice::new(VID, PID, 1, 5).with_parameter("monitor_level", LEVELS).with_parameter("mute", &["off", "on"])
}

fn generate(dir: &std::path::Path, operator: ScriptedOperator) -> SynthResult {
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
    let devices: Vec<Box<dyn DeviceModel>> = vec![Box::new(target()), Box::new(SimpleDevice::new(0x046D, 0xC52B, 1, 3))];
    generate_session(&dir.join("session"), spec, devices).unwrap()
}

fn monitor_channel() -> ChannelKey {
    let planted = target().planted("monitor_level").unwrap();
    assert_eq!(planted.request, SIMPLE_REQUEST);
    ChannelKey { endpoint: 0, direction: Direction::Out, transfer: TransferType::Control, request: Some(planted.request), index: Some(planted.index) }
}

#[test]
fn command_attribution_recovers_the_planted_field() {
    let dir = tempfile::tempdir().unwrap();
    let result = generate(dir.path(), ScriptedOperator::default());
    let marks = result.store.marks().unwrap();
    let timeline = &probe_timelines(&marks)[0];
    let segs = segments(timeline, &result.events);
    assert_eq!(segs.len(), timeline.steps.len(), "every step closed");

    let attribution = attribute_commands(&segs, "monitor_level");
    // SimpleDevice carries the value in wValue (message byte 0) and in data byte 1 (message
    // byte 2 + 1); wValue's high byte and data byte 0 (the parameter index) never vary.
    let planted = target().planted("monitor_level").unwrap();
    let expected = vec![(monitor_channel(), 0), (monitor_channel(), 2 + planted.data_byte)];
    assert_eq!(attribution.fields.iter().map(|f| (f.channel, f.byte)).collect::<Vec<_>>(), expected);
    assert!(attribution.shared.is_empty());

    let device = target();
    for field in &attribution.fields {
        assert_eq!(field.values.len(), LEVELS.len());
        for (value, raw) in &field.values {
            assert_eq!(Some(*raw), device.raw_value("monitor_level", value), "{value}");
        }
        // Raw values 0, 1, 2 differ only in bits 0 and 1.
        assert_eq!(field.bits, (0, 1));
        let set_steps = segs.iter().filter(|s| s.kind == StepKind::Set).count();
        assert_eq!(field.evidence.len(), set_steps);
    }

    // The control parameter is only ever changed by Control steps, so nothing is attributed to it.
    assert_eq!(attribute_commands(&segs, "mute"), Default::default());
}

#[test]
fn attribution_survives_redo_and_a_skipped_step() {
    let dir = tempfile::tempdir().unwrap();
    let operator = ScriptedOperator { redo_once: vec![3], skip: vec![(7, "control stuck".into())], ..ScriptedOperator::default() };
    let result = generate(dir.path(), operator);
    let marks = result.store.marks().unwrap();
    let timeline = &probe_timelines(&marks)[0];
    assert_eq!(timeline.redos.get(&3), Some(&1));
    let segs = segments(timeline, &result.events);
    assert_eq!(segs.len(), timeline.steps.len() - 1, "the skipped step has no window");
    let attribution = attribute_commands(&segs, "monitor_level");
    assert_eq!(attribution.fields.iter().map(|f| f.byte).collect::<Vec<_>>(), vec![0, 3]);
}
