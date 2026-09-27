//! **The recorder's states**: Off, Armed and Recording, and the threads behind them.
//!
//! ```text
//!          arm                 record
//!   Off  ------->  Armed  ---------------> Recording
//!    ^   <-------         <---------------
//!         disarm                stop
//!    ^                                      |
//!    +-------- disarm (asks first) ---------+
//! ```
//!
//! - **Arm** opens the aggregate on a thread of its own (the *host* thread, which holds the turn and
//!   the drivers' apartment for as long as it is armed), reserves the pre-roll, starts streaming into
//!   it, and starts the *writer* thread. It answers once all of that has happened, or with the
//!   sentence that says why it has not, and then nothing is left open.
//! - **Record** and **Stop** are one flag each way, read by the callback at its next block
//!   ([`crate::capture`]). They answer at once; the take begins, or ends, a block later.
//! - **Disarm** stops the stream, lets the writer finish any take from the ring, lets go of every
//!   driver and frees the memory. Disarming while recording is a stop and a disarm; the page asks
//!   first, and so does this ([`Recorder::disarm`] with `confirmed`).
//! - **A preset cannot change under an armed recorder.** Arm takes a copy of it, and arming again
//!   with another preset is refused until it is disarmed.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use gazelle_aggregate::status::{Glitches, Reporter};
use gazelle_aggregate::sub::Host;
use serde::Serialize;

use crate::capture::Capture;
use crate::host::{self, ArmRequest, Channel, Session};
use crate::sim::Timing;
use crate::sizing::Sizing;
use crate::system::{Clock, Disk, Memory};
use crate::wav::SampleFormat;
use crate::writer::{self, Drain, TakeRecord, TakeSettings, WriterState, RF64_AT};

/// Everything the recorder needs from the PC, behind one trait: the real one opens this PC's
/// drivers, the loopback's is made of data, and a test's is whatever it says.
pub trait Environment: Send + Sync {
    /// The PC to open the aggregate on, or why not: no hardware allowed, a DAW already has it.
    fn host(&self) -> Result<Box<dyn Host>, String>;
    /// What happens between blocks on the host thread: at the hardware, a short wait; with fakes,
    /// the blocks themselves. Answering false ends the session.
    fn pump(&self, timing: Timing, master: usize) -> Box<dyn FnMut(usize) -> bool + Send>;
    fn memory(&self) -> Arc<dyn Memory>;
    fn disk(&self) -> Arc<dyn Disk>;
    fn clock(&self) -> Arc<dyn Clock>;
    /// Where the session is published and logged.
    fn reporter(&self) -> Arc<Reporter>;
    /// What made the files: "Gazelle 1.5.0".
    fn originator(&self) -> String;
}

/// A preset as Arm takes it, resolved: every path whole, every number checked.
#[derive(Clone, Debug)]
pub struct Preset {
    pub id: String,
    pub name: String,
    pub request: ArmRequest,
    pub folder: PathBuf,
    pub pattern: String,
    pub format: SampleFormat,
}

/// The preset an armed recorder is holding, as the page shows it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PresetHeld {
    pub id: String,
    pub name: String,
    pub folder: String,
    pub format: &'static str,
}

/// One recorded channel and how loud it is.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ChannelLevel {
    #[serde(flatten)]
    pub channel: Channel,
    /// The loudest sample since the last reading, in dBFS, or nothing for silence.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peak_dbfs: Option<f64>,
}

/// The pre-roll, as the page shows it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Preroll {
    #[serde(flatten)]
    pub sizing: Sizing,
    /// How much is held now, in seconds: filling up to `preroll_seconds` after Arm and after each
    /// take, and, while recording, what the take started with.
    pub held_seconds: f64,
}

/// The take being recorded.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TakeLive {
    /// Seconds since Record took effect.
    pub elapsed_seconds: f64,
    /// Seconds of pre-roll the take started with.
    pub preroll_seconds: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub number: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    pub files: Vec<String>,
}

/// What one interface lost since Arm.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Dropouts {
    pub device: String,
    pub dropped: u64,
    pub starved: u64,
}

/// Everything the page shows, at one moment.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Status {
    /// `off`, `arming`, `armed`, `recording` or `disarming`.
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preset: Option<PresetHeld>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub buffer_size: Option<usize>,
    pub channels: Vec<ChannelLevel>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preroll: Option<Preroll>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub take: Option<TakeLive>,
    /// Blocks lost since Arm because the writer fell behind.
    pub overruns: u64,
    /// What the aggregate lost since Arm, every interface together, and each on its own.
    pub dropouts: u64,
    pub dropouts_by_device: Vec<Dropouts>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disk_free_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disk_seconds_left: Option<f64>,
    pub disk_low: bool,
    /// A driver asked to be reset while armed, which the recorder does not do under a take.
    pub reset_asked: bool,
    /// The last thing that went wrong, in a sentence.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
}

impl Status {
    fn off(problem: Option<String>) -> Status {
        Status {
            state: "off",
            preset: None,
            rate: None,
            buffer_size: None,
            channels: Vec::new(),
            preroll: None,
            take: None,
            overruns: 0,
            dropouts: 0,
            dropouts_by_device: Vec::new(),
            disk_free_bytes: None,
            disk_seconds_left: None,
            disk_low: false,
            reset_asked: false,
            problem,
        }
    }
}

/// An armed recorder's threads and what they share.
struct Running {
    preset: Preset,
    capture: Arc<Capture>,
    opened: host::Opened,
    writer_state: Arc<Mutex<WriterState>>,
    dropouts: Arc<Mutex<Vec<Glitches>>>,
    dropouts_at_arm: Vec<Glitches>,
    stop_host: Arc<AtomicBool>,
    stop_writer: Arc<AtomicBool>,
    host: Option<JoinHandle<()>>,
    writer: Option<JoinHandle<()>>,
    peaks: Vec<Option<f64>>,
    peaks_read: Instant,
}

enum Phase {
    Off,
    Arming,
    Armed(Box<Running>),
    Disarming,
}

/// Why a disarm was not done: it would stop a take, and was not confirmed.
pub const CONFIRM_DISARM: &str = "a take is being recorded: disarming stops it, so confirm to disarm";

/// The recorder: one per server.
pub struct Recorder {
    env: Arc<dyn Environment>,
    phase: Mutex<Phase>,
    /// Takes from earlier sessions, newest first: the page lists them after a disarm too.
    history: Mutex<Vec<TakeRecord>>,
    problem: Mutex<Option<String>>,
    /// The preset last armed with, which the page offers first.
    last_preset: Mutex<Option<String>>,
}

/// How long Arm waits for the drivers to open.
const ARM_TIMEOUT: Duration = Duration::from_secs(30);
/// How often the host thread reads the counters and the log.
const HOUSEKEEPING: Duration = Duration::from_millis(50);
/// How long the writer waits when there is nothing to write.
const WRITER_IDLE: Duration = Duration::from_millis(5);

impl Recorder {
    pub fn new(env: Arc<dyn Environment>) -> Recorder {
        Recorder { env, phase: Mutex::new(Phase::Off), history: Mutex::new(Vec::new()), problem: Mutex::new(None), last_preset: Mutex::new(None) }
    }

    fn phase(&self) -> std::sync::MutexGuard<'_, Phase> {
        self.phase.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Whether the recorder holds the aggregate, or is about to: armed, recording, or on the way.
    pub fn is_active(&self) -> bool {
        !matches!(*self.phase(), Phase::Off)
    }

    /// The state in one word, as [`Status::state`] says it, without reading anything else: `off`,
    /// `arming`, `armed`, `recording` or `disarming`.
    pub fn state(&self) -> &'static str {
        match &*self.phase() {
            Phase::Off => "off",
            Phase::Arming => "arming",
            Phase::Armed(running) if running.capture.wants_recording() => "recording",
            Phase::Armed(_) => "armed",
            Phase::Disarming => "disarming",
        }
    }

    /// The preset an armed recorder holds.
    pub fn armed_preset(&self) -> Option<String> {
        match &*self.phase() {
            Phase::Armed(running) => Some(running.preset.id.clone()),
            _ => None,
        }
    }

    pub fn last_preset(&self) -> Option<String> {
        self.last_preset.lock().ok().and_then(|p| p.clone())
    }

    /// **Arm**: open the aggregate, reserve the pre-roll and start streaming into it. Answers once
    /// it has, or with the sentence that says why not.
    pub fn arm(&self, preset: Preset) -> Result<(), String> {
        {
            let mut phase = self.phase();
            match &*phase {
                Phase::Off => *phase = Phase::Arming,
                Phase::Armed(running) if running.preset.id == preset.id => return Ok(()),
                Phase::Armed(running) => {
                    return Err(format!("Gazelle is armed with {}: disarm it before arming with another preset", running.preset.name))
                }
                _ => return Err("Gazelle is arming or disarming already: wait a moment".into()),
            }
        }
        if let Ok(mut problem) = self.problem.lock() {
            *problem = None;
        }
        match self.start(preset.clone()) {
            Ok(running) => {
                *self.phase() = Phase::Armed(Box::new(running));
                if let Ok(mut last) = self.last_preset.lock() {
                    *last = Some(preset.id);
                }
                Ok(())
            }
            Err(why) => {
                *self.phase() = Phase::Off;
                Err(why)
            }
        }
    }

    fn start(&self, preset: Preset) -> Result<Running, String> {
        let env = Arc::clone(&self.env);
        let request = preset.request.clone();
        let stop_host = Arc::new(AtomicBool::new(false));
        let (reply, answer) = mpsc::channel::<Result<(host::Opened, Arc<Capture>, Arc<Mutex<Vec<Glitches>>>), String>>();
        let stopping = Arc::clone(&stop_host);
        let host = std::thread::Builder::new()
            .name("gazelle-record-host".into())
            .spawn(move || host_thread(env, request, reply, stopping))
            .map_err(|why| format!("the recorder could not start: {why}"))?;
        let (opened, capture, dropouts) = match answer.recv_timeout(ARM_TIMEOUT) {
            Ok(Ok(armed)) => armed,
            Ok(Err(why)) => {
                let _ = host.join();
                return Err(why);
            }
            Err(_) => {
                stop_host.store(true, Ordering::Release);
                return Err("the interfaces did not open within 30 seconds, so Gazelle gave up: another program may be holding them".into());
            }
        };
        let settings = TakeSettings {
            folder: preset.folder.clone(),
            pattern: preset.pattern.clone(),
            preset: preset.name.clone(),
            format: preset.format,
            rate: opened.rate,
            channels: opened.channels.iter().map(|channel| channel.name.clone()).collect(),
            originator: self.env.originator(),
            rf64_limit: RF64_AT,
        };
        let mut drain = Drain::new(settings, self.env.disk(), self.env.clock(), Arc::clone(&dropouts));
        let writer_state = drain.state();
        let stop_writer = Arc::new(AtomicBool::new(false));
        let (ring, leave) = (Arc::clone(&capture), Arc::clone(&stop_writer));
        let writer = std::thread::Builder::new().name("gazelle-record-writer".into()).spawn(move || loop {
            let busy = drain.step(&ring);
            if leave.load(Ordering::Acquire) && ring.take().is_none() && !drain.is_writing() {
                break;
            }
            if !busy {
                std::thread::sleep(WRITER_IDLE);
            }
        });
        let writer = match writer {
            Ok(writer) => writer,
            Err(why) => {
                stop_host.store(true, Ordering::Release);
                let _ = host.join();
                return Err(format!("the recorder's writer could not start: {why}"));
            }
        };
        let dropouts_at_arm = dropouts.lock().map(|d| d.clone()).unwrap_or_default();
        let channels = opened.channels.len();
        Ok(Running {
            preset,
            capture,
            opened,
            writer_state,
            dropouts,
            dropouts_at_arm,
            stop_host,
            stop_writer,
            host: Some(host),
            writer: Some(writer),
            peaks: vec![None; channels],
            peaks_read: Instant::now(),
        })
    }

    /// **Record**: a take starts at the callback's next block, reaching back into the pre-roll.
    pub fn record(&self) -> Result<(), String> {
        let phase = self.phase();
        let Phase::Armed(running) = &*phase else { return Err("Gazelle is not armed: arm it first".into()) };
        if running.capture.wants_recording() {
            return Ok(());
        }
        let per_second = running.opened.rate * running.opened.channels.len() as f64 * f64::from(running.preset.format.bytes());
        writer::room_to_record(self.env.disk().free_bytes(&running.preset.folder), per_second)?;
        running.capture.want_recording(true);
        Ok(())
    }

    /// **Stop**: the take ends at the callback's next block, and the recorder stays armed.
    pub fn stop(&self) -> bool {
        match &*self.phase() {
            Phase::Armed(running) => {
                let was = running.capture.wants_recording();
                running.capture.want_recording(false);
                was
            }
            _ => false,
        }
    }

    /// **Disarm**: stop any take, let go of the drivers and free the memory. A take being recorded
    /// is only stopped this way when `confirmed`.
    pub fn disarm(&self, confirmed: bool) -> Result<bool, String> {
        let running = {
            let mut phase = self.phase();
            match &*phase {
                Phase::Armed(running) if running.capture.wants_recording() && !confirmed => return Err(CONFIRM_DISARM.into()),
                Phase::Armed(_) => {}
                _ => return Ok(false),
            }
            match std::mem::replace(&mut *phase, Phase::Disarming) {
                Phase::Armed(running) => running,
                _ => unreachable!("checked above"),
            }
        };
        let mut running = running;
        running.capture.want_recording(false);
        running.stop_host.store(true, Ordering::Release);
        if let Some(host) = running.host.take() {
            let _ = host.join();
        }
        running.stop_writer.store(true, Ordering::Release);
        if let Some(writer) = running.writer.take() {
            let _ = writer.join();
        }
        let state = running.writer_state.lock().map(|s| s.clone()).unwrap_or_default();
        if let Ok(mut history) = self.history.lock() {
            let mut all = state.takes.clone();
            all.extend(history.drain(..));
            all.truncate(writer::RECENT_TAKES);
            *history = all;
        }
        if let (Some(problem), Ok(mut kept)) = (state.problem, self.problem.lock()) {
            *kept = Some(problem);
        }
        drop(running);
        *self.phase() = Phase::Off;
        Ok(true)
    }

    /// The takes recorded since Gazelle started, newest first.
    pub fn takes(&self) -> Vec<TakeRecord> {
        let mut takes = match &*self.phase() {
            Phase::Armed(running) => running.writer_state.lock().map(|s| s.takes.clone()).unwrap_or_default(),
            _ => Vec::new(),
        };
        if let Ok(history) = self.history.lock() {
            takes.extend(history.iter().cloned());
        }
        takes.truncate(writer::RECENT_TAKES);
        takes
    }

    /// Where things stand. Levels are the loudest sample since the last reading at least a tenth of a
    /// second ago, so more than one reader does not steal them from each other.
    pub fn status(&self) -> Status {
        let problem = self.problem.lock().ok().and_then(|p| p.clone());
        let mut phase = self.phase();
        let running = match &mut *phase {
            Phase::Off => return Status::off(problem),
            Phase::Arming => return Status { state: "arming", ..Status::off(None) },
            Phase::Disarming => return Status { state: "disarming", ..Status::off(None) },
            Phase::Armed(running) => running,
        };
        if running.peaks_read.elapsed() >= Duration::from_millis(100) {
            running.peaks_read = Instant::now();
            running.peaks = running.capture.take_peaks().into_iter().map(dbfs).collect();
        }
        let snapshot = running.capture.snapshot();
        let rate = running.opened.rate;
        let writer = running.writer_state.lock().map(|s| s.clone()).unwrap_or_default();
        let recording = running.capture.wants_recording();
        let take = recording.then(|| {
            let (elapsed, preroll) = match (snapshot.recording, snapshot.window) {
                (true, Some(window)) => (snapshot.written.saturating_sub(window.pressed_at) as f64 / rate, (window.pressed_at - window.start) as f64 / rate),
                _ => (0.0, snapshot.held_frames as f64 / rate),
            };
            TakeLive {
                elapsed_seconds: elapsed,
                preroll_seconds: preroll,
                number: writer.take.as_ref().map(|t| t.number),
                folder: writer.take.as_ref().map(|t| t.folder.clone()),
                files: writer.take.as_ref().map(|t| t.files.clone()).unwrap_or_default(),
            }
        });
        let now = running.dropouts.lock().map(|d| d.clone()).unwrap_or_default();
        let lost = gazelle_aggregate::status::lost_since(&running.dropouts_at_arm, &now);
        Status {
            state: if recording { "recording" } else { "armed" },
            preset: Some(PresetHeld {
                id: running.preset.id.clone(),
                name: running.preset.name.clone(),
                folder: running.preset.folder.display().to_string(),
                format: running.preset.format.words(),
            }),
            rate: Some(rate.round() as u32),
            buffer_size: Some(running.opened.block),
            channels: running
                .opened
                .channels
                .iter()
                .zip(running.peaks.iter())
                .map(|(channel, peak)| ChannelLevel { channel: channel.clone(), peak_dbfs: *peak })
                .collect(),
            preroll: Some(Preroll { sizing: running.opened.sizing, held_seconds: snapshot.held_frames as f64 / rate }),
            take,
            overruns: snapshot.overruns,
            dropouts: lost.iter().map(Glitches::lost).sum(),
            dropouts_by_device: lost.iter().map(|g| Dropouts { device: g.device.clone(), dropped: g.dropped, starved: g.starved }).collect(),
            disk_free_bytes: writer.disk_free_bytes,
            disk_seconds_left: writer.disk_seconds_left,
            disk_low: writer.disk_low,
            reset_asked: host::reset_asked(),
            problem: writer.problem.or(problem),
        }
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        let _ = self.disarm(true);
    }
}

/// A magnitude out of 2^31 in dBFS, or nothing for silence.
fn dbfs(peak: u32) -> Option<f64> {
    (peak > 0).then(|| 20.0 * (f64::from(peak) / 2_147_483_648.0).log10())
}

type Armed = (host::Opened, Arc<Capture>, Arc<Mutex<Vec<Glitches>>>);

/// The host thread: opens the session, answers Arm, pumps until disarmed, and closes.
fn host_thread(env: Arc<dyn Environment>, request: ArmRequest, reply: mpsc::Sender<Result<Armed, String>>, stop: Arc<AtomicBool>) {
    // A vendor driver is a COM object, and this thread is ours, so it says so first, as a
    // measurement's thread does.
    #[cfg(windows)]
    let _com = gazelle_calibrate::session::Apartment::enter();
    let host = match env.host() {
        Ok(host) => host,
        Err(why) => {
            let _ = reply.send(Err(why));
            return;
        }
    };
    let mut session = match Session::open(host, &request, &*env.memory(), env.reporter()) {
        Ok(session) => session,
        Err(why) => {
            let _ = reply.send(Err(why));
            return;
        }
    };
    let mut pump = env.pump(Timing { rate: session.opened.rate, block: session.opened.block }, session.opened.master);
    if reply.send(Ok((session.opened.clone(), session.capture(), session.dropouts()))).is_err() {
        // Arm gave up waiting: nobody wants this session.
        return;
    }
    let mut index = 0usize;
    let mut looked = Instant::now();
    while !stop.load(Ordering::Acquire) {
        if !pump(index) {
            break;
        }
        index = index.wrapping_add(1);
        if looked.elapsed() >= HOUSEKEEPING {
            looked = Instant::now();
            session.housekeeping();
        }
    }
    session.close();
}

/// At the hardware the devices' own threads call back, and the host thread only waits.
pub fn wait_for_the_devices() -> Box<dyn FnMut(usize) -> bool + Send> {
    Box::new(|_| {
        std::thread::sleep(Duration::from_millis(2));
        true
    })
}
