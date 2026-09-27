//! The recorder end to end, against the aggregate's own devices made of data.
//!
//! Nothing here opens a driver. The aggregate is the real one (its plan, padding, rings, delays and
//! audio path); the vendor drivers are the aggregate crate's fakes, whose callbacks the test fires
//! itself, exactly as the calibration's tests do. Every test that hosts the aggregate takes
//! [`hosting`] first: the callbacks are global, and so is the turn.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use gazelle_aggregate::config::{Alignment, Config, DeviceConfig};
use gazelle_aggregate::fake::{FakeDevice, FakeHost, FakePc, Spec};
use gazelle_aggregate::status::Reporter;
use gazelle_aggregate::sub::Host;
use gazelle_calibrate::Pick;

use crate::capture::Capture;
use crate::host::{ArmRequest, Session, MEASURING};
use crate::recorder::{Environment, Preset, Recorder, CONFIRM_DISARM};
use crate::sim::Timing;
use crate::system::{Clock, Disk, LocalClock, Memory, ThisPcDisk};
use crate::testing::{FreeMemory, MemoryDisk, StoppedClock};
use crate::wav::SampleFormat;
use crate::writer::{Drain, TakeSettings, RF64_AT};

const BLOCK: i32 = 64;
const RATE: f64 = 48_000.0;
const GB: u64 = 1 << 30;

/// One test at a time hosts the aggregate.
fn hosting() -> MutexGuard<'static, ()> {
    static ONE: Mutex<()> = Mutex::new(());
    ONE.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Two interfaces of two channels each. "A" drives the callback; "B"'s audio crosses a ring, which
/// costs it a block, so its driver reports a block less latency and the two paths are the same length.
fn two_interfaces() -> Arc<FakePc> {
    let spec = |latency: i32| {
        Spec { min: 8, max: 4096, preferred: BLOCK, rate: RATE, rates: vec![44_100.0, 48_000.0], ..Spec::default() }.with_channels(2, 2).with_latency(latency, 700)
    };
    Arc::new(
        FakePc::new()
            .with("Device A", "{AAAAAAAA-0000-0000-0000-000000000001}", r"c:\antelope\a.dll", spec(600 + BLOCK))
            .with("Device B", "{BBBBBBBB-0000-0000-0000-000000000002}", r"c:\antelope\b.dll", spec(600)),
    )
}

fn config() -> Config {
    Config {
        devices: vec![
            DeviceConfig { key: Some("Device A".into()), name: Some("A".into()), ..DeviceConfig::default() },
            DeviceConfig { key: Some("Device B".into()), name: Some("B".into()), ..DeviceConfig::default() },
        ],
        alignment: Alignment::Aligned,
        ..Config::default()
    }
}

fn request(picks: Vec<Pick>) -> ArmRequest {
    ArmRequest { picks, percent: 1.0, cap_seconds: Some(5.0), config: config(), source: "a test".into() }
}

/// A counting ramp in the top 24 bits, which every file format keeps whole.
fn ramp(n: u64) -> i32 {
    (((n % (1 << 22)) as i32) + 1) << 8
}

/// The two interfaces' converters: both hear the same ramp on the same sample, which is what the
/// aggregate lines up.
struct Converters {
    a: Arc<FakeDevice>,
    b: Arc<FakeDevice>,
    next: u64,
}

impl Converters {
    fn of(pc: &FakePc) -> Converters {
        Converters { a: pc.device("Device A"), b: pc.device("Device B"), next: 0 }
    }

    fn block(&mut self) {
        let half = ((self.next / BLOCK as u64) & 1) as usize;
        let samples: Vec<i32> = (0..BLOCK as u64).map(|i| ramp(self.next + i)).collect();
        for channel in 0..2 {
            self.a.set_input(channel, half, &samples);
            self.b.set_input(channel, half, &samples);
        }
        // The interface that follows hands its block over first, as at the hardware.
        self.b.fire(half);
        self.a.fire(half);
        self.next += BLOCK as u64;
    }
}

fn settings(channels: Vec<String>, format: SampleFormat) -> TakeSettings {
    TakeSettings {
        folder: PathBuf::from("C:/Takes"),
        pattern: crate::names::DEFAULT_PATTERN.into(),
        preset: "Band".into(),
        format,
        rate: RATE,
        channels,
        originator: "Gazelle test".into(),
        rf64_limit: RF64_AT,
    }
}

/// The 24-bit samples of a finished file.
fn samples(bytes: &[u8]) -> Vec<i32> {
    let data = bytes.windows(4).position(|w| w == b"data").expect("a data chunk") + 8;
    let size = u32::from_le_bytes(bytes[data - 4..data].try_into().unwrap()) as usize;
    bytes[data..data + size].chunks(3).map(|b| i32::from_le_bytes([0, b[0], b[1], b[2]])).collect()
}

/// **The whole path**: through the aggregate, round the ring many times while armed, Record, and on
/// into live audio across more laps. Every file is the ramp, sample by sample, with nothing missing
/// at the press or at any wrap, and the two interfaces' files agree on every sample.
#[test]
fn a_take_through_the_aggregate_is_continuous_across_the_press_and_the_wraps_and_lined_up_across_interfaces() {
    let _one = hosting();
    let pc = two_interfaces();
    let host: Box<dyn Host> = Box::new(FakeHost { pc: Arc::clone(&pc) });
    let mut session = Session::open(host, &request(vec![Pick::new(0, 1), Pick::new(1, 0)]), &FreeMemory(Some(GB)), Arc::new(Reporter::silent())).expect("armed");
    assert_eq!(session.opened.channels.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), vec!["A 2", "B 1"], "named as a DAW names them");
    let capture = session.capture();
    let ring_blocks = capture.capacity_frames() / BLOCK as u64;
    let disk = Arc::new(MemoryDisk::default());
    let names = session.opened.channels.iter().map(|c| c.name.clone()).collect();
    let mut drain = Drain::new(settings(names, SampleFormat::Int24), disk.clone(), Arc::new(StoppedClock), session.dropouts());
    let mut converters = Converters::of(&pc);

    // Armed for two and a half laps of the ring.
    for _ in 0..ring_blocks * 5 / 2 {
        converters.block();
        drain.step(&capture);
    }
    assert!((capture.snapshot().held_frames as f64 / RATE - 5.0).abs() < 0.01, "a full five seconds held");
    capture.want_recording(true);
    // And recording for another lap and a half.
    for _ in 0..ring_blocks * 3 / 2 {
        converters.block();
        drain.step(&capture);
        session.housekeeping();
    }
    capture.want_recording(false);
    converters.block();
    while drain.step(&capture) {}
    session.close();

    let take = drain.state().lock().unwrap().takes[0].clone();
    assert_eq!(take.overruns, 0);
    assert_eq!(take.dropouts, 0, "the aggregate lost nothing either");
    let files: Vec<Vec<i32>> = take.files.iter().map(|f| samples(&disk.read(Path::new(f)))).collect();
    let expected = (capture.preroll_frames() + ring_blocks * 3 / 2 * BLOCK as u64) as usize;
    assert_eq!(files[0].len(), expected, "the pre-roll and everything after the press");
    for file in &files {
        for pair in file.windows(2) {
            let (was, now) = (pair[0] >> 8 << 8, pair[1] >> 8 << 8);
            assert_eq!(now >> 8, (was >> 8) % (1 << 22) + 1, "continuous, sample by sample");
        }
    }
    assert_eq!(files[0], files[1], "the two interfaces are lined up on every sample");
    assert!((take.preroll_seconds - 5.0).abs() < 0.01);
}

#[test]
fn the_recorder_plays_silence_on_every_output_of_every_interface() {
    let _one = hosting();
    let pc = two_interfaces();
    let host: Box<dyn Host> = Box::new(FakeHost { pc: Arc::clone(&pc) });
    let mut session = Session::open(host, &request(vec![Pick::new(0, 0)]), &FreeMemory(Some(GB)), Arc::new(Reporter::silent())).expect("armed");
    let mut converters = Converters::of(&pc);
    for _ in 0..20 {
        converters.block();
    }
    for device in [&converters.a, &converters.b] {
        for channel in 0..2 {
            for half in 0..2 {
                assert!(device.output(channel, half).iter().all(|&s| s == 0), "silence");
            }
        }
    }
    session.close();
    assert!(!converters.a.is_started() && !converters.a.has_buffers(), "and closing lets the driver go");
}

#[test]
fn arming_while_a_measurement_has_the_aggregate_is_refused_and_the_other_way_round() {
    let _one = hosting();
    {
        let _measuring = gazelle_calibrate::session::one_at_a_time();
        let host: Box<dyn Host> = Box::new(FakeHost { pc: two_interfaces() });
        let refused = Session::open(host, &request(vec![Pick::new(0, 0)]), &FreeMemory(Some(GB)), Arc::new(Reporter::silent())).err().unwrap();
        assert_eq!(refused, MEASURING);
    }
    let pc = two_interfaces();
    let host: Box<dyn Host> = Box::new(FakeHost { pc: Arc::clone(&pc) });
    let session = Session::open(host, &request(vec![Pick::new(0, 0)]), &FreeMemory(Some(GB)), Arc::new(Reporter::silent())).expect("armed");
    assert!(gazelle_calibrate::session::try_one_at_a_time().is_none(), "a measurement cannot have the turn while armed");
    drop(session);
    assert!(gazelle_calibrate::session::try_one_at_a_time().is_some(), "and can once disarmed");
}

#[test]
fn a_channel_the_aggregate_has_not_got_is_refused_by_name_and_nothing_is_left_open() {
    let _one = hosting();
    let pc = two_interfaces();
    let open = |picks| {
        let host: Box<dyn Host> = Box::new(FakeHost { pc: Arc::clone(&pc) });
        Session::open(host, &request(picks), &FreeMemory(Some(GB)), Arc::new(Reporter::silent())).err().unwrap()
    };
    assert!(open(vec![Pick::new(4, 0)]).contains("the aggregate has 2"));
    let refused = open(vec![Pick::new(0, 7)]);
    assert!(refused.starts_with("A has 2 inputs in the aggregate, and input 8 is not one of them"), "{refused}");
    assert!(open(vec![]).contains("no channels"));
    assert!(open(vec![Pick::new(0, 0), Pick::new(0, 0)]).contains("chosen twice"));
    assert!(!pc.device("Device A").has_buffers());
}

#[test]
fn too_little_memory_is_refused_before_anything_starts() {
    let _one = hosting();
    let pc = two_interfaces();
    let host: Box<dyn Host> = Box::new(FakeHost { pc: Arc::clone(&pc) });
    let refused = Session::open(host, &request(vec![Pick::new(0, 0), Pick::new(1, 1)]), &FreeMemory(Some(1 << 20)), Arc::new(Reporter::silent())).err().unwrap();
    assert!(refused.contains("only 1 MB of memory is free"), "{refused}");
    assert!(!pc.device("Device A").is_started(), "nothing was started");
    assert!(!pc.device("Device A").has_buffers());
}

#[test]
fn a_float_take_is_the_driver_samples_over_2_to_the_31() {
    let _one = hosting();
    let pc = two_interfaces();
    let host: Box<dyn Host> = Box::new(FakeHost { pc: Arc::clone(&pc) });
    let mut session = Session::open(host, &request(vec![Pick::new(0, 0)]), &FreeMemory(Some(GB)), Arc::new(Reporter::silent())).unwrap();
    let capture = session.capture();
    let disk = Arc::new(MemoryDisk::default());
    let mut drain = Drain::new(settings(vec!["A 1".into()], SampleFormat::Float32), disk.clone(), Arc::new(StoppedClock), session.dropouts());
    let mut converters = Converters::of(&pc);
    capture.want_recording(true);
    for _ in 0..40 {
        converters.block();
        drain.step(&capture);
    }
    capture.want_recording(false);
    converters.block();
    while drain.step(&capture) {}
    session.close();
    let take = drain.state().lock().unwrap().takes[0].clone();
    let bytes = disk.read(Path::new(&take.files[0]));
    let data = bytes.windows(4).position(|w| w == b"data").unwrap() + 8;
    let values: Vec<f32> = bytes[data..].chunks(4).map(|b| f32::from_le_bytes(b.try_into().unwrap())).collect();
    let last = *values.last().unwrap();
    assert!(last > 0.0 && last < 1.0);
    assert_eq!(f64::from(last) * 2_147_483_648.0 % 256.0, 0.0, "exactly a 24-bit value");
}

/// A PC made of the two fake interfaces, for the whole recorder with its threads, writing real
/// files into a folder of the test's own.
struct Fakes {
    pc: Mutex<Option<Arc<FakePc>>>,
}

impl Environment for Fakes {
    fn host(&self) -> Result<Box<dyn Host>, String> {
        let pc = two_interfaces();
        *self.pc.lock().unwrap() = Some(Arc::clone(&pc));
        Ok(Box::new(FakeHost { pc }))
    }
    fn pump(&self, timing: Timing, master: usize) -> Box<dyn FnMut(usize) -> bool + Send> {
        let pc = self.pc.lock().unwrap().clone().unwrap();
        Box::new(crate::sim::pump(crate::env::firing_order(&pc, master), timing))
    }
    fn memory(&self) -> Arc<dyn Memory> {
        Arc::new(FreeMemory(Some(GB)))
    }
    fn disk(&self) -> Arc<dyn Disk> {
        Arc::new(ThisPcDisk)
    }
    fn clock(&self) -> Arc<dyn Clock> {
        Arc::new(LocalClock)
    }
    fn reporter(&self) -> Arc<Reporter> {
        Arc::new(Reporter::silent())
    }
    fn originator(&self) -> String {
        "Gazelle test".into()
    }
}

fn wait_for(what: &str, mut until: impl FnMut() -> bool) {
    let since = Instant::now();
    while !until() {
        assert!(since.elapsed() < Duration::from_secs(20), "waited too long for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn off_armed_recording_armed_off_with_real_threads_and_real_files() {
    let _one = hosting();
    let folder = std::env::temp_dir().join(format!("gazelle-record-states-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&folder);
    let recorder = Recorder::new(Arc::new(Fakes { pc: Mutex::new(None) }));
    let preset = |id: &str| Preset {
        id: id.into(),
        name: format!("Preset {id}"),
        request: request(vec![Pick::new(0, 0), Pick::new(1, 1)]),
        folder: folder.clone(),
        pattern: crate::names::DEFAULT_PATTERN.into(),
        format: SampleFormat::Int24,
    };
    assert_eq!(recorder.status().state, "off");
    assert!(recorder.record().unwrap_err().contains("not armed"));
    recorder.arm(preset("one")).expect("armed");
    let status = recorder.status();
    assert_eq!(status.state, "armed");
    assert_eq!(status.rate, Some(48_000));
    assert_eq!(status.channels.len(), 2);
    assert!(recorder.arm(preset("two")).unwrap_err().contains("disarm it before arming with another preset"), "a preset cannot change under an armed recorder");
    wait_for("some pre-roll", || recorder.status().preroll.is_some_and(|p| p.held_seconds > 0.2));

    recorder.record().expect("recording");
    assert_eq!(recorder.status().state, "recording");
    assert_eq!(recorder.disarm(false).unwrap_err(), CONFIRM_DISARM, "disarming while recording asks first");
    wait_for("the take to open", || recorder.status().take.is_some_and(|t| t.number.is_some() && t.elapsed_seconds > 0.2));
    assert!(recorder.stop());
    assert_eq!(recorder.status().state, "armed", "and still armed");
    wait_for("the take to be written", || !recorder.takes().is_empty());
    let take = recorder.takes()[0].clone();
    assert!(take.preroll_seconds > 0.2, "{take:?}");
    assert!(take.seconds > take.preroll_seconds + 0.2);
    for file in &take.files {
        let bytes = std::fs::read(file).expect("the file is on disk");
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize, bytes.len() - 8, "finished");
    }
    assert!(std::fs::read_to_string(&take.log).unwrap().contains("Preset: Preset one"));

    // Recording again and disarming, confirmed, stops that take and finishes it too.
    recorder.record().unwrap();
    wait_for("the second take to open", || recorder.status().take.is_some_and(|t| t.number == Some(2)));
    assert!(recorder.disarm(true).unwrap());
    assert_eq!(recorder.status().state, "off");
    let takes = recorder.takes();
    assert_eq!(takes.iter().map(|t| t.number).collect::<Vec<_>>(), vec![2, 1], "both kept for the page after the disarm");
    assert!(gazelle_calibrate::session::try_one_at_a_time().is_some(), "the turn is given back");
    let _ = std::fs::remove_dir_all(&folder);
}

#[test]
fn the_ring_is_the_size_arm_worked_out() {
    let capture = Capture::allocate(2, 64, 64 * 100, 64 * 70).unwrap();
    assert_eq!(capture.bytes(), 64 * 100 * 2 * 4);
    assert_eq!(capture.preroll_frames(), 64 * 70);
}
