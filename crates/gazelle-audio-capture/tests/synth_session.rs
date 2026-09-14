//! End to end without hardware: generate a session, import its capture, rebuild the timeline,
//! and check that the planted commands land in the right step windows.

use std::path::PathBuf;

use gazelle_audio_capture::capture::event::{Direction, TransferType, UsbEvent};
use gazelle_audio_capture::capture::import::ImportSource;
use gazelle_audio_capture::capture::pipeline::{DeviceFilter, PayloadPolicy, Pipeline};
use gazelle_audio_capture::capture::CaptureSource;
use gazelle_audio_capture::session::model::{Parameter, ParameterDomain, ParameterKind, ProbePlan};
use gazelle_audio_capture::session::plan::StepKind;
use gazelle_audio_capture::session::step::{RunStatus, StepTiming};
use gazelle_audio_capture::session::timeline::{events_in, probe_timelines};
use gazelle_audio_capture::synth::device::{DeviceModel, SimpleDevice};
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

fn devices() -> Vec<Box<dyn DeviceModel>> {
    vec![
        Box::new(SimpleDevice::new(VID, PID, 1, 5).with_parameter("monitor_level", LEVELS).with_parameter("mute", &["off", "on"])),
        Box::new(SimpleDevice::new(0x046D, 0xC52B, 1, 3)),
    ]
}

fn spec(link_type: u32, operator: ScriptedOperator) -> SynthSpec {
    SynthSpec {
        parameters: parameters(),
        plan: ProbePlan { parameter: "monitor_level".into(), value_a: "0 dB".into(), value_b: vec!["-6 dB".into()], sweep: vec!["-12 dB".into()], repeats: 2, control_parameter: "mute".into() },
        seed: 11,
        timing: StepTiming::default(),
        start_ns: 1_700_000_000_000_000_000,
        operator,
        link_type,
        policy: PayloadPolicy::default(),
    }
}

fn generate(dir: &std::path::Path, link_type: u32, operator: ScriptedOperator) -> SynthResult {
    generate_session(&dir.join("session"), spec(link_type, operator), devices()).unwrap()
}

fn import(path: &std::path::Path) -> Vec<UsbEvent> {
    let mut pipeline = Pipeline::new(DeviceFilter::All, PayloadPolicy::default());
    ImportSource::new(path)
        .start()
        .unwrap()
        .frames
        .flat_map(|f| pipeline.process(f.unwrap()).unwrap())
        .map(|(_, e)| e)
        .collect()
}

fn is_command(e: &UsbEvent) -> bool {
    e.transfer == TransferType::Control && e.direction == Direction::Out && e.setup.is_some_and(|s| s.request_type == 0x40)
}

#[test]
fn generated_session_round_trips_and_commands_land_in_action_windows() {
    let dir = tempfile::tempdir().unwrap();
    let operator = ScriptedOperator {
        redo_once: vec![3],
        skip: vec![(5, "control stuck".into())],
        actual_values: vec![(4, "-0.5 dB".into())],
        ..ScriptedOperator::default()
    };
    let result = generate(dir.path(), 249, operator);
    let events = import(&result.capture);
    assert_eq!(events, result.events, "capture decodes to the generated events");
    assert!(events.iter().all(|e| e.device == 5), "neighbour device filtered at the source");
    assert!(events.windows(2).all(|w| w[0].packet_index + 1 == w[1].packet_index));

    let store = &result.store;
    assert_eq!(store.parameters().unwrap(), parameters());
    assert!(store.info().unwrap().device_descriptor_hex.unwrap().starts_with("1201"));
    let timelines = probe_timelines(&store.marks().unwrap());
    let tl = &timelines[0];
    assert_eq!((tl.probe.as_str(), tl.outcome), ("p1", Some(RunStatus::Completed)));
    assert_eq!(tl.steps.len(), 2 * 7 + 2);
    assert_eq!(tl.skipped, vec![(5, "control stuck".to_string())]);
    assert_eq!(tl.redos.get(&3), Some(&1));
    assert_eq!(tl.windows.len(), tl.steps.len() - 1);
    assert_eq!(tl.windows.iter().find(|w| w.step == 4).unwrap().actual_value.as_deref(), Some("-0.5 dB"));
    assert!(tl.windows.iter().all(|w| !w.clock_suspect));

    let device = SimpleDevice::new(VID, PID, 1, 5).with_parameter("monitor_level", LEVELS).with_parameter("mute", &["off", "on"]);
    for w in &tl.windows {
        let spec = &tl.steps[w.step];
        let commands: Vec<&UsbEvent> = events_in(&events, w.action()).iter().filter(|e| is_command(e)).collect();
        match spec.kind {
            StepKind::Set => {
                let raw = device.raw_value("monitor_level", spec.to.as_deref().unwrap()).unwrap();
                assert_eq!(commands.len(), 1, "step {} ({:?})", w.step, spec.kind);
                assert_eq!(commands[0].setup.unwrap().value, raw as u16);
                assert_eq!(commands[0].data, vec![1, raw]);
            }
            StepKind::Control => assert_eq!(commands[0].setup.unwrap().index, 2),
            StepKind::Idle | StepKind::NoOp => assert!(commands.is_empty(), "step {}", w.step),
        }
        assert!(events_in(&events, w.settle()).iter().all(|e| !is_command(e)), "settle of step {}", w.step);
    }
}

#[test]
fn usbmon_link_type_produces_the_same_events() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let pcap = generate(a.path(), 249, ScriptedOperator::default());
    let mon = generate(b.path(), 220, ScriptedOperator::default());
    assert_eq!(import(&mon.capture), mon.events);
    assert_eq!(pcap.events, mon.events);
}

#[test]
fn generation_is_deterministic() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let x = generate(a.path(), 249, ScriptedOperator::default());
    let y = generate(b.path(), 249, ScriptedOperator::default());
    assert_eq!(std::fs::read(&x.capture).unwrap(), std::fs::read(&y.capture).unwrap());
    assert_eq!(
        std::fs::read(x.store.root().join("marks.jsonl")).unwrap(),
        std::fs::read(y.store.root().join("marks.jsonl")).unwrap()
    );
}

/// Committed fixtures (spec §10) must match the generator. Regenerate with
/// `GAZELLE_REGEN_FIXTURES=1 cargo test -p gazelle-audio-capture --test synth_session`.
#[test]
fn committed_fixtures_match_the_generator() {
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    for (link_type, name) in [(249, "synth-usbpcap.pcapng"), (220, "synth-usbmon.pcapng")] {
        let dir = tempfile::tempdir().unwrap();
        let generated = std::fs::read(generate(dir.path(), link_type, ScriptedOperator::default()).capture).unwrap();
        let path = fixtures.join(name);
        if std::env::var_os("GAZELLE_REGEN_FIXTURES").is_some() {
            std::fs::create_dir_all(&fixtures).unwrap();
            std::fs::write(&path, &generated).unwrap();
        }
        assert_eq!(std::fs::read(&path).unwrap(), generated, "{name} is stale");
    }
}
