//! The recorder and the metronome end to end, against the aggregate's own devices made of data.
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
use crate::host::{ArmRequest, Armed, OpenRequest, Session, MEASURING};
use crate::metronome::{Params, Then};
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

fn open(pc: &Arc<FakePc>, outputs: Vec<Pick>, params: Params) -> Result<Session, String> {
    let host: Box<dyn Host> = Box::new(FakeHost { pc: Arc::clone(pc) });
    Session::open(host, &OpenRequest { config: config(), source: "a test".into(), outputs, params }, Arc::new(Reporter::silent()))
}

/// Arm an open session: the preset's channels, sized and reserved, the tap on the callback.
fn arm(session: &Session, picks: Vec<Pick>, memory: u64) -> Result<Armed, String> {
    let armed = Armed::prepare(&session.opened, &request(picks), &FreeMemory(Some(memory)))?;
    session.shared().attach(Arc::clone(&armed.tap))?;
    Ok(armed)
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
    TakeSettings { folder: PathBuf::from("C:/Takes"), pattern: crate::names::DEFAULT_PATTERN.into(), preset: "Band".into(), format, rate: RATE, channels, originator: "Gazelle test".into(), rf64_limit: RF64_AT, cubase_seed: Default::default() }
}

/// The 24-bit samples of a finished file.
fn samples(bytes: &[u8]) -> Vec<i32> {
    let data = bytes.windows(4).position(|w| w == b"data").expect("a data chunk") + 8;
    let size = u32::from_le_bytes(bytes[data - 4..data].try_into().unwrap()) as usize;
    bytes[data..data + size].chunks(3).map(|b| i32::from_le_bytes([0, b[0], b[1], b[2]])).collect()
}

/// A file's cue point, in samples into its audio, if it has one.
fn cue(bytes: &[u8]) -> Option<u32> {
    let at = bytes.windows(4).rposition(|w| w == b"cue ")? + 8;
    Some(u32::from_le_bytes(bytes[at + 24..at + 28].try_into().unwrap()))
}

/// **The whole path**: through the aggregate, round the ring many times while armed, Record, and on
/// into live audio across more laps. Every file is the ramp, sample by sample, with nothing missing
/// at the press or at any wrap, and the two interfaces' files agree on every sample.
#[test]
fn a_take_through_the_aggregate_is_continuous_across_the_press_and_the_wraps_and_lined_up_across_interfaces() {
    let _one = hosting();
    let pc = two_interfaces();
    let mut session = open(&pc, vec![], Params::default()).expect("open");
    let armed = arm(&session, vec![Pick::new(0, 1), Pick::new(1, 0)], GB).expect("armed");
    assert_eq!(armed.channels.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), vec!["A 2", "B 1"], "named as a DAW names them");
    let capture = Arc::clone(armed.tap.capture());
    let ring_blocks = capture.capacity_frames() / BLOCK as u64;
    let disk = Arc::new(MemoryDisk::default());
    let names = armed.channels.iter().map(|c| c.name.clone()).collect();
    let mut drain = Drain::new(settings(names, SampleFormat::Int24), disk.clone(), Arc::new(StoppedClock), session.dropouts());
    let mut converters = Converters::of(&pc);

    for _ in 0..ring_blocks * 5 / 2 {
        converters.block();
        drain.step(&capture);
    }
    assert!((capture.snapshot().held_frames as f64 / RATE - 5.0).abs() < 0.01, "a full five seconds held");
    capture.want_recording(true);
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
    assert_eq!(take.downbeat_seconds, None, "no count-in, no cue");
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
fn with_no_metronome_outputs_every_output_of_every_interface_is_silence() {
    let _one = hosting();
    let pc = two_interfaces();
    let mut session = open(&pc, vec![], Params::default()).expect("open");
    let _armed = arm(&session, vec![Pick::new(0, 0)], GB).expect("armed");
    session.shared().generator.start();
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
fn the_metronome_plays_to_the_outputs_picked_and_every_other_output_stays_silent() {
    let _one = hosting();
    let pc = two_interfaces();
    // B's second output: a pair would be two picks, and each gets the same click.
    let mut session = open(&pc, vec![Pick::new(1, 1), Pick::new(0, 0)], Params::default()).expect("open");
    assert_eq!(session.opened.outputs.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), vec!["B 2", "A 1"], "named as the aggregate names them");
    assert_eq!(session.opened.outputs_problem, None);
    let mut converters = Converters::of(&pc);
    let mut heard = [[Vec::new(), Vec::new()], [Vec::new(), Vec::new()]];
    session.shared().generator.start();
    for n in 0..40 {
        converters.block();
        let half = (n & 1) as usize;
        for (d, device) in [&converters.a, &converters.b].into_iter().enumerate() {
            for (channel, into) in heard[d].iter_mut().enumerate() {
                into.extend(device.output(channel, half));
            }
        }
    }
    session.close();
    let peak = |s: &[i32]| s.iter().map(|v| v.unsigned_abs()).max().unwrap_or(0);
    assert!(heard[0][1].iter().all(|&s| s == 0), "A 2 is not picked: silence");
    assert!(heard[1][0].iter().all(|&s| s == 0), "B 1 is not picked: silence");
    assert!(peak(&heard[0][0]) > 0 && peak(&heard[1][1]) > 0, "both picks play the click");
    let ceiling = 10f64.powf(-6.0 / 20.0) * f64::from(i32::MAX);
    assert!(f64::from(peak(&heard[0][0])) <= ceiling);
    let expected = 10f64.powf(-18.0 / 20.0) * f64::from(i32::MAX);
    assert!((f64::from(peak(&heard[0][0])) - expected).abs() < expected * 0.001, "at -18 dBFS by default");
}

#[test]
fn an_output_the_aggregate_has_not_got_is_left_out_and_said_so_without_stopping_the_session() {
    let _one = hosting();
    let pc = two_interfaces();
    let session = open(&pc, vec![Pick::new(0, 7), Pick::new(1, 0)], Params::default()).expect("the recorder can still arm");
    assert_eq!(session.opened.outputs.len(), 1);
    assert!(session.opened.outputs_problem.as_deref().unwrap().contains("A has no output 8"), "{:?}", session.opened.outputs_problem);
}

#[test]
fn opening_while_a_measurement_has_the_aggregate_is_refused_and_the_other_way_round() {
    let _one = hosting();
    {
        let _measuring = gazelle_calibrate::session::one_at_a_time();
        let refused = open(&two_interfaces(), vec![], Params::default()).err().unwrap();
        assert_eq!(refused, MEASURING);
    }
    let session = open(&two_interfaces(), vec![], Params::default()).expect("open");
    assert!(gazelle_calibrate::session::try_one_at_a_time().is_none(), "a measurement cannot have the turn while open");
    drop(session);
    assert!(gazelle_calibrate::session::try_one_at_a_time().is_some(), "and can once closed");
}

#[test]
fn a_channel_the_aggregate_has_not_got_is_refused_by_name() {
    let _one = hosting();
    let pc = two_interfaces();
    let session = open(&pc, vec![], Params::default()).expect("open");
    let refuse = |picks| Armed::prepare(&session.opened, &request(picks), &FreeMemory(Some(GB))).err().unwrap();
    assert!(refuse(vec![Pick::new(4, 0)]).contains("the aggregate has 2"));
    let refused = refuse(vec![Pick::new(0, 7)]);
    assert!(refused.starts_with("A has 2 inputs in the aggregate, and input 8 is not one of them"), "{refused}");
    assert!(refuse(vec![]).contains("no channels"));
    assert!(refuse(vec![Pick::new(0, 0), Pick::new(0, 0)]).contains("chosen twice"));
    let tight = Armed::prepare(&session.opened, &request(vec![Pick::new(0, 0), Pick::new(1, 1)]), &FreeMemory(Some(1 << 20))).err().unwrap();
    assert!(tight.contains("only 1 MB of memory is free"), "{tight}");
}

#[test]
fn a_float_take_is_the_driver_samples_over_2_to_the_31() {
    let _one = hosting();
    let pc = two_interfaces();
    let mut session = open(&pc, vec![], Params::default()).unwrap();
    let armed = arm(&session, vec![Pick::new(0, 0)], GB).unwrap();
    let capture = Arc::clone(armed.tap.capture());
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

/// A cable from A's first output into A's second input: what the metronome plays comes back into
/// the take, a fixed number of samples later.
struct Cable {
    converters: Converters,
    carried: Vec<i32>,
}

impl Cable {
    fn block(&mut self) {
        let half = ((self.converters.next / BLOCK as u64) & 1) as usize;
        self.converters.a.set_input(1, half, &self.carried);
        self.converters.b.fire(half);
        self.converters.a.fire(half);
        self.carried = self.converters.a.output(0, half);
        self.converters.next += BLOCK as u64;
    }
}

/// The first sample at or after `from` that is not silence.
fn first_sound(file: &[i32], from: usize) -> usize {
    from + file[from..].iter().position(|&s| s != 0).expect("a click after it")
}

/// **Record with a count-in, sample for sample.** The take starts on the downbeat after the count-in
/// and reaches back into the pre-roll, so the count-in is in it; its clock counts from the press; and
/// the cue in every file is exactly one bar after the press, where the click on the downbeat is,
/// through the same path as the click on the press.
#[test]
fn a_take_after_a_count_in_starts_on_the_downbeat_holds_the_count_in_and_marks_the_downbeat_to_the_sample() {
    let _one = hosting();
    let pc = two_interfaces();
    // 150 BPM in 3/4: a bar is 57 600 samples, which is not a whole number of 64-sample blocks.
    let params = Params { tempo_tenths: 1_500, numerator: 3, ..Params::default() };
    let mut session = open(&pc, vec![Pick::new(0, 0)], params).expect("open");
    let armed = arm(&session, vec![Pick::new(0, 1)], GB).expect("armed");
    let capture = Arc::clone(armed.tap.capture());
    let disk = Arc::new(MemoryDisk::default());
    let mut drain = Drain::new(settings(vec!["A 2".into()], SampleFormat::Int24), disk.clone(), Arc::new(StoppedClock), session.dropouts());
    let mut cable = Cable { converters: Converters::of(&pc), carried: vec![0; BLOCK as usize] };
    for _ in 0..1_003 {
        cable.block();
        drain.step(&capture);
    }
    let shared = session.shared();
    let generator = &shared.generator;
    generator.count_in(1, Then::Record, false);
    for _ in 0..1_000 {
        cable.block();
        drain.step(&capture);
    }
    assert!(capture.wants_recording(), "the take started on its own, on the downbeat");
    capture.want_recording(false);
    cable.block();
    while drain.step(&capture) {}
    session.close();

    let take = drain.state().lock().unwrap().takes[0].clone();
    let bytes = disk.read(Path::new(&take.files[0]));
    let file = samples(&bytes);
    let pressed = (take.preroll_seconds * RATE).round() as usize;
    let downbeat = cue(&bytes).expect("a cue on the downbeat") as usize;
    assert_eq!(downbeat - pressed, 57_600, "exactly one bar of 3/4 at 150 after the press");
    assert_eq!((take.downbeat_seconds.unwrap() * RATE).round() as usize, downbeat);
    assert!(pressed > 0, "the take reaches back before the press, into the pre-roll");
    // The click on the press and the click on the downbeat come back through the same path.
    let latency = first_sound(&file, pressed) - pressed;
    assert_eq!(first_sound(&file, downbeat) - downbeat, latency, "the cue is on the downbeat's click, to the sample");
    assert!(file[..pressed].iter().all(|&s| s == 0), "nothing before the press: the click was not playing");
    let log = String::from_utf8(disk.read(Path::new(&take.log))).unwrap();
    assert!(log.contains(&format!("the downbeat after it is sample {downbeat} of the take")), "{log}");
}

#[test]
fn stop_during_a_count_in_cancels_it_and_starts_no_take() {
    let _one = hosting();
    let pc = two_interfaces();
    let mut session = open(&pc, vec![Pick::new(0, 0)], Params::default()).expect("open");
    let armed = arm(&session, vec![Pick::new(0, 1)], GB).expect("armed");
    let capture = Arc::clone(armed.tap.capture());
    let mut cable = Cable { converters: Converters::of(&pc), carried: vec![0; BLOCK as usize] };
    let shared = session.shared();
    let generator = &shared.generator;
    generator.count_in(2, Then::Record, false);
    for _ in 0..500 {
        cable.block();
    }
    generator.cancel_count_in();
    capture.want_recording(false);
    for _ in 0..4_000 {
        cable.block();
    }
    assert!(capture.take().is_none() && !capture.wants_recording(), "no take");
    assert!(generator.beat().running, "the click it started is left for the recorder to stop");
    session.close();
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

fn recorder_in(folder: &Path) -> (Recorder, impl Fn(&str) -> Preset) {
    let recorder = Recorder::new(Arc::new(Fakes { pc: Mutex::new(None) }));
    let folder = folder.to_path_buf();
    let preset = move |id: &str| Preset {
        id: id.into(),
        name: format!("Preset {id}"),
        request: request(vec![Pick::new(0, 0), Pick::new(1, 1)]),
        folder: folder.clone(),
        pattern: crate::names::DEFAULT_PATTERN.into(),
        format: SampleFormat::Int24,
    };
    (recorder, preset)
}

fn temp(name: &str) -> PathBuf {
    let folder = std::env::temp_dir().join(format!("gazelle-record-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&folder);
    folder
}

#[test]
fn off_armed_recording_armed_off_with_real_threads_and_real_files() {
    let _one = hosting();
    let folder = temp("states");
    let (recorder, preset) = recorder_in(&folder);
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
    assert_eq!(take.cubase, None, "no seed, no archive");

    // A seed set while armed counts from the next take.
    let seed = folder.join("seed.xml");
    std::fs::write(&seed, include_str!("../testdata/cubase-seed.xml")).unwrap();
    recorder.set_cubase_seed(Some(seed));
    recorder.record().unwrap();
    wait_for("the second take to open", || recorder.status().take.is_some_and(|t| t.number == Some(2)));
    assert!(recorder.disarm(true).unwrap());
    assert_eq!(recorder.status().state, "off");
    let takes = recorder.takes();
    assert_eq!(takes.iter().map(|t| t.number).collect::<Vec<_>>(), vec![2, 1], "both kept for the page after the disarm");
    let archive = takes[0].cubase.clone().expect("the second take has its archive");
    assert!(archive.ends_with("T002 Cubase.xml"), "{archive}");
    let xml = std::fs::read_to_string(&archive).unwrap();
    assert_eq!(crate::cubase::check_archive(&xml).unwrap().listed, 2, "a track for each of its two files");
    for file in &takes[0].files {
        let name = Path::new(file).file_name().unwrap().to_str().unwrap();
        assert!(xml.contains(name), "{name}");
    }
    assert!(gazelle_calibrate::session::try_one_at_a_time().is_some(), "the turn is given back");
    let _ = std::fs::remove_dir_all(&folder);
}

fn metronome(recorder: &Recorder, params: Params, bars: u32, follow: bool) {
    recorder.set_metronome(params, vec![Pick::new(0, 0), Pick::new(0, 1)], bars, follow).expect("the metronome's choices");
}

/// **One session, two users**: whichever starts first opens it, the other joins it, stopping one
/// leaves the other running, and it closes only when neither needs it.
#[test]
fn the_recorder_and_the_metronome_share_one_session_and_it_closes_when_neither_needs_it() {
    let _one = hosting();
    let folder = temp("shared");
    let (recorder, preset) = recorder_in(&folder);
    metronome(&recorder, Params::default(), 0, false);
    let engine = Arc::clone(recorder.engine());

    // The metronome alone.
    recorder.metronome_start(&config(), "a test").expect("started with nothing armed");
    assert!(engine.is_open() && recorder.holds_the_aggregate());
    assert!(gazelle_calibrate::session::try_one_at_a_time().is_none(), "it holds the turn, as arming does");
    wait_for("the click", || recorder.metronome_status().running && recorder.metronome_status().since_beat_seconds.is_some());
    assert_eq!(recorder.metronome_status().outputs.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), vec!["A 1", "A 2"]);
    assert!(recorder.metronome_stop());
    assert!(!engine.is_open(), "stopped with nothing armed: closed");
    assert!(gazelle_calibrate::session::try_one_at_a_time().is_some());

    // The metronome first, then Arm joins its session.
    recorder.metronome_start(&config(), "a test").unwrap();
    recorder.arm(preset("one")).expect("armed, joining the metronome's session");
    assert_eq!(engine.sessions_opened(), 2, "one session each time, not a second one for Arm");
    assert!(recorder.metronome_status().running, "arming did not interrupt the click");
    assert!(recorder.metronome_stop());
    assert!(engine.is_open(), "the recorder still has it");
    assert_eq!(recorder.status().state, "armed");
    assert!(recorder.disarm(false).unwrap());
    assert!(!engine.is_open(), "and now neither has");

    // Arm first, then the metronome uses the armed session; disarm, and the click plays on.
    recorder.arm(preset("one")).unwrap();
    recorder.metronome_start(&config(), "a test").unwrap();
    assert_eq!(engine.sessions_opened(), 3);
    assert!(recorder.disarm(false).unwrap());
    assert!(engine.is_open() && recorder.metronome_status().running, "the click keeps the interfaces");
    assert!(recorder.metronome_stop());
    assert!(!engine.is_open());

    // The outputs cannot change while the session is open.
    recorder.arm(preset("one")).unwrap();
    assert!(recorder.set_metronome(Params::default(), vec![Pick::new(1, 0)], 0, false).unwrap_err().contains("fixed while the interfaces are open"));
    recorder.disarm(false).unwrap();
    metronome(&recorder, Params::default(), 0, false);

    // A measurement has the aggregate: the metronome is refused, as Arm is.
    {
        let _measuring = gazelle_calibrate::session::one_at_a_time();
        assert_eq!(recorder.metronome_start(&config(), "a test").unwrap_err(), MEASURING);
        assert!(recorder.arm(preset("one")).unwrap_err().contains("measurement"));
        assert!(!engine.is_open());
    }
    let _ = std::fs::remove_dir_all(&folder);
}

#[test]
fn a_metronome_with_no_outputs_does_not_start_and_says_where_to_choose_them() {
    let _one = hosting();
    let (recorder, _) = recorder_in(&temp("no-outputs"));
    assert!(recorder.metronome_start(&config(), "a test").unwrap_err().contains("no outputs"));
    recorder.set_metronome(Params::default(), vec![Pick::new(0, 9)], 0, false).unwrap();
    assert!(recorder.metronome_start(&config(), "a test").unwrap_err().contains("A has no output 10"));
    assert!(!recorder.engine().is_open(), "and nothing is left open");
}

#[test]
fn record_with_a_count_in_counts_then_records_and_stop_stops_the_click_it_started() {
    let _one = hosting();
    let folder = temp("count-in");
    let (recorder, preset) = recorder_in(&folder);
    // 400 BPM in 2/4: a bar is 0.3 s.
    metronome(&recorder, Params { tempo_tenths: 4_000, numerator: 2, ..Params::default() }, 2, false);
    recorder.arm(preset("one")).unwrap();
    recorder.record().unwrap();
    assert_eq!(recorder.state(), "counting_in");
    let status = recorder.metronome_status();
    assert_eq!(status.started_by, Some("count_in"));
    wait_for("the take to start on the downbeat", || recorder.state() == "recording");
    wait_for("some of the take", || recorder.status().take.is_some_and(|t| t.elapsed_seconds > 0.8));
    assert!(recorder.stop());
    assert!(!recorder.metronome_status().running, "the click the count-in started stops with the take");
    wait_for("the take to be written", || !recorder.takes().is_empty());
    let take = recorder.takes()[0].clone();
    let downbeat = take.downbeat_seconds.expect("a cue on the downbeat");
    assert!((downbeat - take.preroll_seconds - 0.6).abs() < 1e-9, "two bars after the press: {downbeat} against {}", take.preroll_seconds);

    // Stop during a count-in: no take, and the click goes.
    recorder.record().unwrap();
    assert_eq!(recorder.state(), "counting_in");
    assert!(recorder.stop());
    assert_eq!(recorder.state(), "armed");
    std::thread::sleep(Duration::from_millis(900));
    assert_eq!(recorder.takes().len(), 1, "no take was started");
    assert!(!recorder.metronome_status().running);

    // A click started by hand is left alone by Stop and by Disarm.
    metronome(&recorder, Params { tempo_tenths: 4_000, numerator: 2, ..Params::default() }, 1, false);
    recorder.metronome_start(&config(), "a test").unwrap();
    recorder.record().unwrap();
    wait_for("the take", || recorder.state() == "recording");
    assert_eq!(recorder.metronome_status().started_by, Some("hand"));
    recorder.stop();
    assert!(recorder.metronome_status().running, "started by hand, stopped by hand");
    recorder.disarm(true).unwrap();
    assert!(recorder.metronome_status().running);
    recorder.metronome_stop();
    assert!(!recorder.engine().is_open());
    let _ = std::fs::remove_dir_all(&folder);
}

#[test]
fn a_click_that_follows_record_runs_with_the_take_and_stops_with_it() {
    let _one = hosting();
    let folder = temp("follow");
    let (recorder, preset) = recorder_in(&folder);
    metronome(&recorder, Params::default(), 0, true);
    recorder.arm(preset("one")).unwrap();
    assert!(!recorder.metronome_status().running);
    recorder.record().unwrap();
    assert_eq!(recorder.state(), "recording", "no count-in: at once");
    let status = recorder.metronome_status();
    assert!(status.running);
    assert_eq!(status.started_by, Some("follow"));
    recorder.stop();
    assert!(!recorder.metronome_status().running);
    assert!(recorder.metronome_preview().is_ok(), "a preview, while armed");
    assert_eq!(recorder.metronome_status().started_by, Some("preview"));
    recorder.disarm(true).unwrap();
    assert!(recorder.metronome_preview().unwrap_err().contains("only while Gazelle is armed"), "a preview never opens the interfaces");
    let _ = std::fs::remove_dir_all(&folder);
}

#[test]
fn the_ring_is_the_size_arm_worked_out() {
    let capture = Capture::allocate(2, 64, 64 * 100, 64 * 70).unwrap();
    assert_eq!(capture.bytes(), 64 * 100 * 2 * 4);
    assert_eq!(capture.preroll_frames(), 64 * 70);
}
