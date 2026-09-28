//! **The recorder's states**: Off, Armed, Counting in and Recording, the metronome beside them, and
//! the threads behind them.
//!
//! ```text
//!          arm                 record                       (count-in over)
//!   Off  ------->  Armed  ---------------> [Counting in] ----------------> Recording
//!    ^   <-------         <---------------------------------------------
//!         disarm                               stop
//!    ^                                                                    |
//!    +------------------------ disarm (asks first) -----------------------+
//! ```
//!
//! - **Arm** holds the shared session ([`crate::engine`]), opening the aggregate if the metronome has
//!   not already, reserves the pre-roll, hangs the tap on the callback, and starts the *writer*
//!   thread. It answers once all of that has happened, or with the sentence that says why it has not,
//!   and then nothing is left held.
//! - **Record** and **Stop** are one flag each way, read by the callback at its next block
//!   ([`crate::capture`]). They answer at once; the take begins, or ends, a block later.
//! - **Record with a count-in** starts the click if it is not running, and the callback starts the
//!   take itself on the downbeat after the count-in, exactly as a Record pressed on that sample would:
//!   reaching back into the pre-roll, which holds the count-in. The take's clock counts from the press,
//!   and every file carries a cue on the downbeat ([`crate::writer`]). Stop during the count-in cancels
//!   it and no take is started.
//! - **Disarm** stops any take, takes the tap off the callback, lets the writer finish, frees the
//!   memory and lets go of the session, which closes unless the metronome is still playing.
//!   Disarming while recording is a stop and a disarm; the page asks first, and so does this
//!   ([`Recorder::disarm`] with `confirmed`).
//! - **A preset cannot change under an armed recorder.** Arm takes a copy of it, and arming again
//!   with another preset is refused until it is disarmed.
//!
//! # The metronome
//!
//! Started by hand it plays until it is stopped by hand, whatever the recorder does. Started by a
//! count-in, or by Record with *follows Record* on, it is the take's: Stop and Disarm stop it too.
//! Either way it holds the session while it plays, so it can run with nothing armed.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use gazelle_aggregate::config::Config;
use gazelle_aggregate::status::{Glitches, Reporter};
use gazelle_aggregate::sub::Host;
use gazelle_calibrate::Pick;
use serde::Serialize;

use crate::capture::Capture;
use crate::engine::{Engine, Lease, User};
use crate::host::{self, ArmRequest, Armed, Channel};
use crate::metronome::{Params, Then, COUNT_IN_MAX};
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
    /// Seconds since Record took effect, or, after a count-in, since Record was pressed.
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
    /// `off`, `arming`, `armed`, `counting_in`, `recording` or `disarming`.
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

/// Who started the click, which decides what stops it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartedBy {
    /// A person: only a person stops it.
    Hand,
    /// Record with a count-in: Stop and Disarm stop it.
    CountIn,
    /// Record, with the click following it: Stop and Disarm stop it.
    Follow,
    /// A preview of one bar: it stops itself.
    Preview,
}

impl StartedBy {
    pub fn words(self) -> &'static str {
        match self {
            StartedBy::Hand => "hand",
            StartedBy::CountIn => "count_in",
            StartedBy::Follow => "follow",
            StartedBy::Preview => "preview",
        }
    }

    fn is_the_takes(self) -> bool {
        matches!(self, StartedBy::CountIn | StartedBy::Follow)
    }
}

/// A count-in, as the pages show it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct CountInLive {
    pub bars: u32,
    /// The bar of the count-in playing now, from one; zero while it waits for the next downbeat.
    pub bar: u32,
}

/// The metronome, as the pages show it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MetronomeStatus {
    pub running: bool,
    /// The aggregate is open, by the recorder or the metronome.
    pub open: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_by: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate: Option<u32>,
    /// The beat last played, from one, in its bar, from one.
    pub beat: u32,
    pub bar: u64,
    pub beats_per_bar: u32,
    /// How long a beat is now.
    pub beat_seconds: f64,
    /// How long ago the last beat was played, as the session's samples count it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since_beat_seconds: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count_in: Option<CountInLive>,
    /// The outputs it plays to, named as the aggregate names them, while the aggregate is open.
    pub outputs: Vec<Channel>,
    /// Why some of the chosen outputs are not played to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outputs_problem: Option<String>,
}

/// An armed recorder's threads and what they share.
struct Running {
    preset: Preset,
    capture: Arc<Capture>,
    lease: Lease,
    channels: Vec<Channel>,
    sizing: Sizing,
    writer_state: Arc<Mutex<WriterState>>,
    dropouts_at_arm: Vec<Glitches>,
    stop_writer: Arc<AtomicBool>,
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

/// The metronome's side of things, off the audio thread.
#[derive(Default)]
struct Click {
    /// It holds the session.
    held: bool,
    started_by: Option<StartedBy>,
    count_in_bars: u32,
    follow: bool,
    /// A count-in for a take is under way: where the generator was when it was asked for.
    counting_from: Option<u64>,
}

/// Why a disarm was not done: it would stop a take, and was not confirmed.
pub const CONFIRM_DISARM: &str = "a take is being recorded: disarming stops it, so confirm to disarm";
/// Why the metronome will not start.
pub const NO_OUTPUTS: &str = "the metronome has no outputs to play to: choose them in the Metronome section of the Recording page";

/// The recorder and the metronome: one per server.
pub struct Recorder {
    env: Arc<dyn Environment>,
    engine: Arc<Engine>,
    phase: Mutex<Phase>,
    click: Mutex<Click>,
    /// Takes from earlier sessions, newest first: the page lists them after a disarm too.
    history: Mutex<Vec<TakeRecord>>,
    problem: Mutex<Option<String>>,
    /// The preset last armed with, which the page offers first.
    last_preset: Mutex<Option<String>>,
}

/// How long the writer waits when there is nothing to write.
const WRITER_IDLE: Duration = Duration::from_millis(5);
/// How long a stopped click is given to ring out before the session under it closes.
const RING_OUT: Duration = Duration::from_millis(300);

impl Recorder {
    pub fn new(env: Arc<dyn Environment>) -> Recorder {
        let engine = Arc::new(Engine::new(Arc::clone(&env)));
        Recorder { env, engine, phase: Mutex::new(Phase::Off), click: Mutex::new(Click::default()), history: Mutex::new(Vec::new()), problem: Mutex::new(None), last_preset: Mutex::new(None) }
    }

    fn phase(&self) -> std::sync::MutexGuard<'_, Phase> {
        self.phase.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn click(&self) -> std::sync::MutexGuard<'_, Click> {
        self.click.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The shared session.
    pub fn engine(&self) -> &Arc<Engine> {
        &self.engine
    }

    /// Whether the recorder holds the aggregate, or is about to: armed, recording, or on the way.
    pub fn is_active(&self) -> bool {
        !matches!(*self.phase(), Phase::Off)
    }

    /// Whether anything holds the aggregate: the recorder, or the metronome.
    pub fn holds_the_aggregate(&self) -> bool {
        self.is_active() || self.engine.is_open() || self.click().held
    }

    /// The state in one word, as [`Status::state`] says it, without reading anything else: `off`,
    /// `arming`, `armed`, `counting_in`, `recording` or `disarming`.
    pub fn state(&self) -> &'static str {
        match &*self.phase() {
            Phase::Off => "off",
            Phase::Arming => "arming",
            Phase::Armed(running) if running.capture.wants_recording() => "recording",
            Phase::Armed(running) if self.counting(running) => "counting_in",
            Phase::Armed(_) => "armed",
            Phase::Disarming => "disarming",
        }
    }

    /// Whether a count-in for a take is under way. It ends when the take starts, when it is
    /// cancelled, or when the click that carried it stopped.
    fn counting(&self, running: &Running) -> bool {
        let mut click = self.click();
        let Some(from) = click.counting_from else { return false };
        let beat = running.lease.shared.generator.beat();
        let asked_and_not_seen = beat.position <= from + 2 * running.lease.opened.block as u64;
        if running.capture.wants_recording() || !(beat.counting_to.is_some() || asked_and_not_seen) {
            click.counting_from = None;
            return false;
        }
        true
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

    /// **Arm**: hold the session, reserve the pre-roll and start copying into it. Answers once it
    /// has, or with the sentence that says why not.
    pub fn arm(&self, preset: Preset) -> Result<(), String> {
        {
            let mut phase = self.phase();
            match &*phase {
                Phase::Off => *phase = Phase::Arming,
                Phase::Armed(running) if running.preset.id == preset.id => return Ok(()),
                Phase::Armed(running) => return Err(format!("Gazelle is armed with {}: disarm it before arming with another preset", running.preset.name)),
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
        let lease = self.engine.acquire(User::Recorder, &preset.request.config, &preset.request.source)?;
        let armed = match Armed::prepare(&lease.opened, &preset.request, &*self.env.memory()) {
            Ok(armed) => armed,
            Err(why) => {
                self.engine.release(User::Recorder);
                return Err(why);
            }
        };
        let capture = Arc::clone(armed.tap.capture());
        let settings = TakeSettings {
            folder: preset.folder.clone(),
            pattern: preset.pattern.clone(),
            preset: preset.name.clone(),
            format: preset.format,
            rate: lease.opened.rate,
            channels: armed.channels.iter().map(|channel| channel.name.clone()).collect(),
            originator: self.env.originator(),
            rf64_limit: RF64_AT,
        };
        let mut drain = Drain::new(settings, self.env.disk(), self.env.clock(), Arc::clone(&lease.dropouts));
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
                self.engine.release(User::Recorder);
                return Err(format!("the recorder's writer could not start: {why}"));
            }
        };
        if let Err(why) = lease.shared.attach(Arc::clone(&armed.tap)) {
            stop_writer.store(true, Ordering::Release);
            let _ = writer.join();
            self.engine.release(User::Recorder);
            return Err(why);
        }
        let dropouts_at_arm = lease.dropouts.lock().map(|d| d.clone()).unwrap_or_default();
        let count = armed.channels.len();
        Ok(Running {
            preset,
            capture,
            lease,
            channels: armed.channels,
            sizing: armed.sizing,
            writer_state,
            dropouts_at_arm,
            stop_writer,
            writer: Some(writer),
            peaks: vec![None; count],
            peaks_read: Instant::now(),
        })
    }

    /// **Record**: a take starts at the callback's next block, reaching back into the pre-roll; with
    /// a count-in, on the downbeat after it.
    pub fn record(&self) -> Result<(), String> {
        let phase = self.phase();
        let Phase::Armed(running) = &*phase else { return Err("Gazelle is not armed: arm it first".into()) };
        if running.capture.wants_recording() || self.counting(running) {
            return Ok(());
        }
        let per_second = running.lease.opened.rate * running.channels.len() as f64 * f64::from(running.preset.format.bytes());
        writer::room_to_record(self.env.disk().free_bytes(&running.preset.folder), per_second)?;
        let (bars, follow) = {
            let click = self.click();
            (click.count_in_bars, click.follow)
        };
        let config = running.preset.request.config.clone();
        let generator = &running.lease.shared.generator;
        if bars > 0 {
            self.hold_click(&config, &running.preset.request.source, StartedBy::CountIn)?;
            let mut click = self.click();
            click.counting_from = Some(generator.beat().position);
            generator.count_in(bars, Then::Record, false);
            return Ok(());
        }
        running.capture.want_recording(true);
        if follow && !generator.beat().running {
            // The take has started; a click that cannot play is said, and does not stop it.
            if let Err(why) = self.hold_click(&config, &running.preset.request.source, StartedBy::Follow) {
                if let Ok(mut problem) = self.problem.lock() {
                    *problem = Some(format!("The take is recording, and the metronome did not start: {why}."));
                }
                return Ok(());
            }
            generator.start();
        }
        Ok(())
    }

    /// **Stop**: the take ends at the callback's next block, and the recorder stays armed. A count-in
    /// under way is cancelled and no take starts. A click the take started stops with it.
    pub fn stop(&self) -> bool {
        let phase = self.phase();
        let Phase::Armed(running) = &*phase else { return false };
        let counting = self.counting(running);
        let was = running.capture.wants_recording();
        running.lease.shared.generator.cancel_count_in();
        self.click().counting_from = None;
        running.capture.want_recording(false);
        drop(phase);
        self.stop_the_takes_click();
        was || counting
    }

    /// **Disarm**: stop any take, let go of the session and free the memory. A take being recorded
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
        running.lease.shared.generator.cancel_count_in();
        self.click().counting_from = None;
        running.capture.want_recording(false);
        self.stop_the_takes_click();
        // Off the callback, then the take is ended where the audio ended, as a stopped stream ends it.
        running.lease.shared.detach();
        running.capture.close_after_stream_stopped();
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
        self.engine.release(User::Recorder);
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
        let counting = match &*phase {
            Phase::Armed(running) => self.counting(running),
            _ => false,
        };
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
        let rate = running.lease.opened.rate;
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
        let now = running.lease.dropouts.lock().map(|d| d.clone()).unwrap_or_default();
        let lost = gazelle_aggregate::status::lost_since(&running.dropouts_at_arm, &now);
        Status {
            state: if recording {
                "recording"
            } else if counting {
                "counting_in"
            } else {
                "armed"
            },
            preset: Some(PresetHeld { id: running.preset.id.clone(), name: running.preset.name.clone(), folder: running.preset.folder.display().to_string(), format: running.preset.format.words() }),
            rate: Some(rate.round() as u32),
            buffer_size: Some(running.lease.opened.block),
            channels: running.channels.iter().zip(running.peaks.iter()).map(|(channel, peak)| ChannelLevel { channel: channel.clone(), peak_dbfs: *peak }).collect(),
            preroll: Some(Preroll { sizing: running.sizing, held_seconds: snapshot.held_frames as f64 / rate }),
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

    // ---------------------------------------------------------------------------------------------
    // The metronome.
    // ---------------------------------------------------------------------------------------------

    /// The metronome's choices: what it plays, where, its count-in and whether it follows Record.
    /// The outputs are refused while the aggregate is open with other ones.
    pub fn set_metronome(&self, params: Params, outputs: Vec<Pick>, count_in_bars: u32, follow: bool) -> Result<(), String> {
        if let Some(why) = params.problem() {
            return Err(why);
        }
        if count_in_bars > COUNT_IN_MAX {
            return Err(format!("a count-in is 0 to {COUNT_IN_MAX} bars, not {count_in_bars}"));
        }
        self.engine.set_outputs(outputs)?;
        self.engine.set_params(params);
        let mut click = self.click();
        click.count_in_bars = count_in_bars;
        click.follow = follow;
        Ok(())
    }

    /// Hold the session for the click, opening it with `config` if nothing has it open, and say who
    /// started it. The click is not started here.
    fn hold_click(&self, config: &Config, source: &str, by: StartedBy) -> Result<Lease, String> {
        // Not under the lock: opening the drivers takes a while, and the pages read the click meanwhile.
        let held = self.click().held;
        let lease = if held {
            self.engine.lease().ok_or("the interfaces closed under the metronome")?
        } else {
            let lease = self.engine.acquire(User::Metronome, config, source)?;
            if lease.opened.outputs.is_empty() {
                self.engine.release(User::Metronome);
                return Err(lease.opened.outputs_problem.clone().unwrap_or_else(|| NO_OUTPUTS.into()));
            }
            lease
        };
        let mut click = self.click();
        click.held = true;
        let running = lease.shared.generator.wants_running();
        // A click a person started stays theirs; anything else is taken over by what starts it now.
        if !(running && click.started_by == Some(StartedBy::Hand)) {
            click.started_by = Some(by);
        }
        Ok(lease)
    }

    /// **Start the click** by hand, opening the aggregate with `config` if nothing has it open.
    /// Blocks while the drivers open: call it off the runtime.
    pub fn metronome_start(&self, config: &Config, source: &str) -> Result<(), String> {
        if self.engine.outputs().is_empty() {
            return Err(NO_OUTPUTS.into());
        }
        let lease = self.hold_click(config, source, StartedBy::Hand)?;
        self.click().started_by = Some(StartedBy::Hand);
        // A preview under way becomes the click itself.
        lease.shared.generator.cancel_count_in();
        lease.shared.generator.start();
        Ok(())
    }

    /// **Stop the click** by hand. A count-in under way is cancelled with it. Once nothing else
    /// holds the aggregate, the last click rings out and the aggregate closes.
    pub fn metronome_stop(&self) -> bool {
        let lease = self.engine.lease();
        let was = lease.as_ref().is_some_and(|lease| lease.shared.generator.wants_running() || lease.shared.generator.beat().running);
        if let Some(lease) = &lease {
            lease.shared.generator.cancel_count_in();
            lease.shared.generator.stop();
        }
        self.click().counting_from = None;
        self.let_go_of_click(lease);
        was
    }

    /// A click a take started stops with it.
    fn stop_the_takes_click(&self) {
        let takes = self.click().started_by.is_some_and(StartedBy::is_the_takes);
        if !takes {
            return;
        }
        let lease = self.engine.lease();
        if let Some(lease) = &lease {
            lease.shared.generator.stop();
        }
        self.let_go_of_click(lease);
    }

    fn let_go_of_click(&self, lease: Option<Lease>) {
        let held = {
            let mut click = self.click();
            click.started_by = None;
            std::mem::take(&mut click.held)
        };
        if !held {
            return;
        }
        // Closing the aggregate under a click still sounding would cut it off: let it ring out.
        if !self.is_active() {
            if let Some(lease) = lease {
                let since = Instant::now();
                while lease.shared.generator.beat().sounding && since.elapsed() < RING_OUT {
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        }
        self.engine.release(User::Metronome);
    }

    /// **Preview**: one bar, quietly, then stop. Only while the recorder already has the aggregate
    /// open and the click is not running: a preview never opens the interfaces.
    pub fn metronome_preview(&self) -> Result<(), String> {
        if !matches!(&*self.phase(), Phase::Armed(_)) {
            return Err("a preview plays only while Gazelle is armed, so it never opens the interfaces by itself: arm first, or start the metronome".into());
        }
        let lease = self.engine.lease().ok_or("the interfaces are not open")?;
        if lease.opened.outputs.is_empty() {
            return Err(lease.opened.outputs_problem.clone().unwrap_or_else(|| NO_OUTPUTS.into()));
        }
        let generator = &lease.shared.generator;
        if generator.wants_running() || generator.beat().running {
            return Err("the metronome is already playing".into());
        }
        self.click().started_by = Some(StartedBy::Preview);
        generator.count_in(1, Then::Stop, true);
        Ok(())
    }

    /// Where the click is.
    pub fn metronome_status(&self) -> MetronomeStatus {
        let started_by = self.click().started_by;
        let Some(lease) = self.engine.lease() else {
            return MetronomeStatus {
                running: false,
                open: false,
                started_by: None,
                rate: None,
                beat: 0,
                bar: 0,
                beats_per_bar: self.engine.params().numerator,
                beat_seconds: 0.0,
                since_beat_seconds: None,
                count_in: None,
                outputs: Vec::new(),
                outputs_problem: None,
            };
        };
        let rate = lease.opened.rate;
        let beat = lease.shared.generator.beat();
        let running = lease.shared.generator.wants_running();
        let count_in = beat.counting_to.map(|(target, bars)| {
            let first = target.saturating_sub(u64::from(bars));
            let bar = if beat.bar < first || beat.beat_at.is_none() { 0 } else { (beat.bar - first + 1).min(u64::from(bars)) as u32 };
            CountInLive { bars, bar }
        });
        MetronomeStatus {
            running,
            open: true,
            started_by: if running { started_by.map(StartedBy::words) } else { None },
            rate: Some(rate.round() as u32),
            beat: beat.beat + 1,
            bar: beat.bar + 1,
            beats_per_bar: beat.beats_per_bar,
            beat_seconds: beat.beat_samples / rate,
            since_beat_seconds: beat.beat_at.filter(|_| running).map(|at| beat.position.saturating_sub(at) as f64 / rate),
            count_in,
            outputs: lease.opened.outputs.clone(),
            outputs_problem: lease.opened.outputs_problem.clone(),
        }
    }

    /// A preview that has stopped itself leaves nothing to hold; a click whose session has gone
    /// holds nothing either. Called now and then, off the audio thread.
    pub fn tidy(&self) {
        let preview_done = {
            let click = self.click();
            click.started_by == Some(StartedBy::Preview) && self.engine.lease().is_none_or(|lease| !lease.shared.generator.wants_running())
        };
        if preview_done {
            self.click().started_by = None;
        }
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        let _ = self.disarm(true);
        self.metronome_stop();
    }
}

/// A magnitude out of 2^31 in dBFS, or nothing for silence.
fn dbfs(peak: u32) -> Option<f64> {
    (peak > 0).then(|| 20.0 * (f64::from(peak) / 2_147_483_648.0).log10())
}

/// At the hardware the devices' own threads call back, and the host thread only waits.
pub fn wait_for_the_devices() -> Box<dyn FnMut(usize) -> bool + Send> {
    crate::engine::wait_for_the_devices()
}
