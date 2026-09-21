//! Driving the aggregate, as a DAW does, and keeping what came back.
//!
//! **Why through the aggregate and not around it.** Opening the vendor drivers separately and
//! comparing their sample counters compares two things that were never meant to be compared, and
//! it measures the wrong quantity: what a person wants corrected is what is left after the
//! aggregate has done all of its lining up. So this opens the aggregate itself, with the
//! configuration the person is actually using, and reads the aggregate's own input buffers.
//! Whatever offset is in there **is** the error, in the coordinates a trim is written in.
//!
//! **The audio path here obeys the same rules as the driver's.** The callback allocates nothing,
//! locks nothing, logs nothing and makes no system call: it copies into an arena that was
//! allocated before the stream started, and stops when the arena is full.
//!
//! **One run at a time.** The interface gives a host's callbacks nothing to say which driver called
//! them, so they have to be plain functions over something global, exactly as they are in a DAW.
//!
//! **A run publishes what a session publishes.** The aggregate is opened with the driver's own
//! reporter, so while a run is going the shared record says what it always says (the plan in force,
//! the interfaces, the buffers, the counters, the phase each interface settled on) and the event
//! log keeps the same lines afterwards. Every one of those lines is marked as Gazelle's own, so
//! that somebody reading the file tomorrow can tell a measurement from a night's recording. Being
//! unable to publish is never a reason to fail a run: a reporter with nowhere to write says
//! nothing, and the measurement is exactly the same measurement.

use std::ffi::c_void;
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use gazelle_aggregate::aggregate::{Aggregate, Wanted};
use gazelle_aggregate::config::Config;
use gazelle_aggregate::phase;
use gazelle_aggregate::status::{LogSink, Noticing, Reporter};
use gazelle_aggregate::sub::Host;
use gazelle_audio_aggregate_status::events;
use gazelle_audio_aggregate_status::record::phase as phase_codes;
use gazelle_audio_stream_abi::raw::{selector, CallbacksRaw, Time, ENGINE_VERSION_2};
use serde::Serialize;
use std::cell::UnsafeCell;

use crate::click;
use crate::measure::{self, Glitches, Reading};
use crate::rig::{Direction, Rig, Settings};
use crate::trim::{self, PhaseAtRun, TrimChange};

/// An extra input channel the run recorded and reported, which took no part in any trim.
///
/// A witness has no device of its own to be named by, because an interface may have several of
/// them, so it carries the channel number it was and the name of the interface that channel
/// belongs to. It is measured exactly as every reading is: against the reference channel, across
/// all of the clicks.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Witness {
    /// The aggregate input channel this was, counted as a DAW counts them.
    pub channel: i32,
    /// The interface that channel belongs to, as `aggregate.json` names it.
    pub device: String,
    /// What was heard on it, measured the same way everything else in the run was.
    pub reading: Reading,
}

/// **What a run's phase measurement came to on one interface**, exactly as the driver reported it.
///
/// The driver measures where each interface's capture pipeline actually started, at the beginning
/// of every session, and lines each session up to the phase its trim was measured at. A calibration
/// run is a session, so it has one of these too; it measures and applies nothing, and reading this
/// is the only way to tell a phase that was measured from one that was refused and from an
/// interface the setup never asked to measure. Those are three very different reasons for a number
/// to be where it is.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PhaseHeard {
    /// The interface, as `aggregate.json` names it.
    pub device: String,
    /// The one word the driver's own record gives this state: `not_configured` when the setup asks
    /// for no measurement on this interface, `measuring` when a run ended before one settled,
    /// `measured_only` once it has, because a run measures the phase and moves nothing for it, and
    /// `not_heard` when nothing came back. A DAW's session can also be `applied`, `no_reference`,
    /// `off_the_grid` or `too_far`, and a run passes on whatever the record says.
    pub state: &'static str,
    /// What the measurement came to, in samples, exactly. Positive means this interface's capture
    /// arrived later than the reported figures said it would. This is the number that becomes the
    /// trim's phase reference.
    pub measured_samples: i32,
    /// What was added to this interface's input path because of it, in samples. Always zero in a
    /// run, which moves nothing, and zero in any session unless the state is `applied`.
    pub applied_samples: i32,
    /// The sentence the driver's own event log keeps about it, in the driver's own words.
    pub note: String,
    /// The driver's code for the state. Left out of what is sent on, because the word above is the
    /// same thing in a form a person can read.
    #[serde(skip)]
    pub code: u32,
}

impl PhaseHeard {
    /// One interface's phase, as the driver wrote it into the shared record.
    pub fn from_record(device: &str, code: u32, measured: i32, applied: i32) -> PhaseHeard {
        PhaseHeard {
            device: device.to_string(),
            state: phase_codes::name(code),
            measured_samples: measured,
            applied_samples: applied,
            note: phase::detail(device, code, measured, applied),
            code,
        }
    }

    /// Whether this interface was lined up by a measurement, which a run never does.
    pub fn was_applied(&self) -> bool {
        phase_codes::is_applied(self.code)
    }

    /// Whether a measurement was asked for on this interface and turned down, which is the case
    /// worth putting in front of a person: the interfaces ran on the drivers' own figures.
    pub fn was_refused(&self) -> bool {
        phase_codes::is_refused(self.code)
    }
}

/// Everything one run found, or the one sentence saying why there was no run.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Outcome {
    pub direction: Direction,
    /// The rate the run actually happened at.
    pub rate: f64,
    /// The buffer size the run actually happened at.
    pub block: i32,
    /// How many clicks the run was told to play.
    pub clicks: u32,
    /// One per interface, in the order they appear in `aggregate.json`.
    pub readings: Vec<Reading>,
    /// The extra channels the run listened in on, in the order they were asked for. Kept apart
    /// from the readings so that nothing downstream can mistake an observation for an interface.
    pub witnesses: Vec<Witness>,
    /// One per interface: what the file said, what was measured, and what to write.
    pub trims: Vec<TrimChange>,
    /// One per interface: what this run's own phase measurement came to. Empty when the run had
    /// nowhere to publish, because the driver reports a phase through the shared record and a run
    /// that could not make one has nothing to read.
    pub phases: Vec<PhaseHeard>,
    /// True when this was a check rather than a measurement: lined up as a DAW's session is, so its
    /// lags say whether the trims hold, and it offers no trims of its own.
    pub checking: bool,
    /// Why nothing was measured. `None` means the run happened.
    pub refusal: Option<String>,
}

impl Outcome {
    /// A run that never started, and the sentence that says why.
    pub fn refused(direction: Direction, why: impl Into<String>) -> Outcome {
        Outcome {
            direction,
            rate: 0.0,
            block: 0,
            clicks: 0,
            readings: Vec::new(),
            witnesses: Vec::new(),
            trims: Vec::new(),
            phases: Vec::new(),
            checking: false,
            refusal: Some(why.into()),
        }
    }

    /// The interfaces whose phase measurement was asked for and turned down, which is the thing a
    /// person reading a result has to be told: those interfaces were lined up from the figures
    /// their drivers report, and not from a measurement.
    pub fn phases_refused(&self) -> impl Iterator<Item = &PhaseHeard> {
        self.phases.iter().filter(|phase| phase.was_refused())
    }

    /// Whether this run produced trims worth writing into the file.
    pub fn is_measured(&self) -> bool {
        self.refusal.is_none() && self.trims.iter().any(|trim| trim.not_applied.is_none())
    }

    /// Whether the audio underneath the measurement was clean. A run that lost a block on any
    /// interface between the first click and the last was measuring through a fault, and a person
    /// deciding whether to believe the numbers wants to be told that before they read them.
    ///
    /// **Only what happened while it was measuring counts.** The settling time before the first
    /// click is a stream starting, and an interface can take a block of silence there while the
    /// callbacks find each other. Nothing is being measured yet, so nothing that happens there
    /// makes a measurement untrue.
    ///
    /// Witnesses are not counted here. A witness carries the counters of the interface its channel
    /// is on, which is an interface that already has a reading, so counting both would say the same
    /// lost blocks twice.
    pub fn was_clean(&self) -> bool {
        self.refusal.is_none() && self.readings.iter().all(|reading| reading.glitches.is_clean())
    }

    /// What every interface lost while the measurement was going, added up.
    pub fn blocks_lost(&self) -> u64 {
        self.readings.iter().map(|reading| reading.glitches.lost()).sum()
    }
}

/// The environment variable every test and script sets. This crate plays audio out of real
/// converters, so it refuses outright when it is set rather than looking for a safe subset.
pub const NO_HARDWARE: &str = "GAZELLE_NO_HARDWARE";

/// What to say, and stop for, when this run is forbidden hardware. `None` means carry on.
pub fn no_hardware_refusal(value: Option<&str>) -> Option<String> {
    let forbidden = !matches!(value.map(str::trim), None | Some("") | Some("0") | Some("false"));
    forbidden.then(|| {
        format!(
            "{NO_HARDWARE} is set, and this measurement drives real converters: it plays a click out of an \
             interface and records it back. Unset {NO_HARDWARE} to run it deliberately."
        )
    })
}

/// What to say when the aggregate driver is already open in another process.
pub fn in_use_refusal(open: bool, streaming: bool) -> Option<String> {
    (open || streaming).then(|| {
        "the Gazelle Aggregate driver is already open somewhere else, and these interfaces can only be opened once: \
         close the DAW or whatever else is using them, and start this again"
            .to_string()
    })
}

// ---------------------------------------------------------------------------------------------
// What a run says about itself while it is going, and leaves behind afterwards.
// ---------------------------------------------------------------------------------------------

/// What every line a run writes says before it says anything else.
///
/// A run is a session, and it writes a session's lines: the interfaces opened, the audio started
/// and stopped, what each one lost, what its phase came to. Somebody reading the file the next
/// morning has to be able to tell those apart from a night's recording, because "the interfaces
/// were out by 32 samples" means one thing when Gazelle went looking and another when a DAW was
/// halfway through a take.
pub const MARK: &str = "Gazelle's own measurement:";

/// The event log a run writes: every line the driver's own machinery writes goes through here and
/// comes out marked as Gazelle's own.
///
/// It is done here, at the one place every line passes, rather than at each of the places a line
/// is written, because there is no version of this that somebody remembers to do every time.
pub struct RunLog {
    keeping: Box<dyn LogSink>,
}

impl RunLog {
    /// Mark everything written to this log as a run's own.
    pub fn marking(keeping: Box<dyn LogSink>) -> RunLog {
        RunLog { keeping }
    }

    /// One line, marked. A line this build cannot take apart is kept exactly as it was: an
    /// unmarked line is worth more than a mangled one.
    pub fn marked(line: &str) -> String {
        match events::parse(line) {
            Some(parsed) => events::line(&parsed.at, parsed.event, &format!("{MARK} {}", parsed.detail)),
            None => line.to_string(),
        }
    }
}

impl LogSink for RunLog {
    fn append(&mut self, line: &str) {
        self.keeping.append(&RunLog::marked(line));
    }
}

/// Where a run publishes what it is doing and writes down what happened: the same shared section
/// and the same log file the driver uses, because a run watched from the Aggregate page has to
/// look like what that page already knows how to read.
///
/// **Not being able to do either is not a failure**, exactly as it is not one for the driver. A
/// reporter with no section and no log says nothing, and the measurement is unchanged: nothing
/// below asks it whether it worked.
///
/// **A section somebody else made is left alone.** It only exists while something holds it, and
/// the something is the driver loaded in a DAW that has let go of its buffers, which the check for
/// a driver in use cannot see. That record has one writer and it is not us: writing a run into it
/// would put two processes' sessions into one record. The run then goes ahead unpublished, and its
/// own lines still reach the log, which is append only and marked.
#[cfg(windows)]
pub fn run_reporter() -> Arc<Reporter> {
    use gazelle_aggregate::status::{FileLog, LocalClock};
    use gazelle_audio_aggregate_status::map::Mapping;
    use gazelle_audio_aggregate_status::windows::Section;
    use gazelle_audio_aggregate_status::Publisher;

    let publisher = Section::create()
        .ok()
        .filter(|section| section.created())
        .and_then(|section| Publisher::map(Box::new(section)).ok());
    let log: Option<Box<dyn LogSink>> =
        FileLog::open().map(|log| Box::new(RunLog::marking(Box::new(log))) as Box<dyn LogSink>);
    Arc::new(Reporter::new(publisher, log, Box::new(LocalClock)))
}

/// What each interface's phase came to, read out of the record the run has just been writing.
///
/// Nothing at all when there was nowhere to publish: the driver reports a phase through the shared
/// record, so a run without one has nothing to read rather than a set of interfaces that were not
/// measured, and saying the second when the first is true would be a lie about the hardware.
fn phases_now(reporter: &Reporter, names: &[String]) -> Vec<PhaseHeard> {
    let Some(area) = reporter.snapshot() else { return Vec::new() };
    let count = (area.device_count as usize).min(area.devices.len()).min(names.len());
    (0..count)
        .map(|index| {
            let device = &area.devices[index];
            PhaseHeard::from_record(&names[index], device.phase_state, device.phase_measured, device.phase_applied)
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// The arena, and the callbacks that fill it.
// ---------------------------------------------------------------------------------------------

/// Where the run's audio lives: allocated before the stream starts, written only by the callback,
/// and read only after the stream has stopped.
pub struct Arena {
    block: usize,
    /// The DAW buffer pointers for the channels being recorded, in device order.
    inputs: Vec<[*mut c_void; 2]>,
    /// The DAW buffer pointers for the channels the click leaves on, in device order.
    outputs: Vec<[*mut c_void; 2]>,
    /// The click, already at the level asked for and in the samples the driver carries.
    click: Vec<i32>,
    /// Where each click starts, in samples from the first block of the run.
    emits: Vec<usize>,
    /// How many samples one channel holds.
    samples: usize,
    /// `inputs.len()` runs of `samples`, one after another.
    captured: UnsafeCell<Box<[i32]>>,
    /// How many samples have been captured. The callback's only shared state.
    written: AtomicUsize,
}

/// The arena is touched by one callback thread at a time, and by the thread that made it only
/// before that thread starts the stream and after it has stopped it.
unsafe impl Sync for Arena {}
unsafe impl Send for Arena {}

impl Arena {
    /// Room for a whole run, allocated at once.
    pub fn new(block: usize, channels: usize, samples: usize, click: Vec<i32>, emits: Vec<usize>) -> Arena {
        Arena {
            block,
            inputs: Vec::new(),
            outputs: Vec::new(),
            click,
            emits,
            samples,
            captured: UnsafeCell::new(vec![0i32; channels * samples].into_boxed_slice()),
            written: AtomicUsize::new(0),
        }
    }

    /// Where the aggregate's buffers are, once it has made them. Before the stream starts.
    pub fn buffers(&mut self, inputs: Vec<[*mut c_void; 2]>, outputs: Vec<[*mut c_void; 2]>) {
        self.inputs = inputs;
        self.outputs = outputs;
    }

    pub fn is_full(&self) -> bool {
        self.written.load(Ordering::Acquire) + self.block > self.samples
    }

    /// How many samples of the run were captured.
    pub fn captured_samples(&self) -> usize {
        self.written.load(Ordering::Acquire)
    }

    /// One channel's capture, as the arithmetic wants it. Only after the stream has stopped.
    pub fn channel(&self, index: usize) -> Vec<f64> {
        let captured = unsafe { &*self.captured.get() };
        let at = index * self.samples;
        let scale = 1.0 / i32::MAX as f64;
        captured[at..at + self.captured_samples().min(self.samples)].iter().map(|&v| v as f64 * scale).collect()
    }

    /// Where each click was played, which is where the search for it begins.
    pub fn emits(&self) -> &[usize] {
        &self.emits
    }

    /// One block: record every input, and play whatever part of a click belongs in this block.
    ///
    /// Nothing here allocates, locks or logs.
    fn block_happened(&self, half: usize) {
        let at = self.written.load(Ordering::Relaxed);
        if at + self.block > self.samples {
            // Full. The aggregate clears every output buffer before this is called, so writing
            // nothing from here plays silence.
            return;
        }
        // Safety: the callback thread is the only one that touches the capture while the stream is
        // running, and the thread that made the arena does not read it until the stream has
        // stopped. Each run below is this channel's alone.
        let captured = unsafe { &mut *self.captured.get() };
        for (index, pair) in self.inputs.iter().enumerate() {
            let from = pair[half & 1] as *const i32;
            if from.is_null() {
                continue;
            }
            // Safety: the aggregate made this buffer and it is `block` samples of `i32`, which is
            // the one sample type the aggregate presents.
            let run = unsafe { std::slice::from_raw_parts(from, self.block) };
            let into = index * self.samples + at;
            captured[into..into + self.block].copy_from_slice(run);
        }

        for &emit in &self.emits {
            let start = emit.max(at);
            let end = (emit + self.click.len()).min(at + self.block);
            if start >= end {
                continue;
            }
            for pair in &self.outputs {
                let into = pair[half & 1] as *mut i32;
                if into.is_null() {
                    continue;
                }
                // Safety: as above, for a buffer the aggregate handed us to fill.
                let run = unsafe { std::slice::from_raw_parts_mut(into, self.block) };
                run[start - at..end - at].copy_from_slice(&self.click[start - emit..end - emit]);
            }
        }

        self.written.store(at + self.block, Ordering::Release);
    }
}

/// The run in flight, for the callbacks to find. Null except between `start` and `stop`.
static ARENA: AtomicPtr<Arena> = AtomicPtr::new(std::ptr::null_mut());

/// Whose turn it is. The callbacks are global, so there is one run at a time, and a run that
/// panicked says nothing about the next one.
static ORDER: Mutex<()> = Mutex::new(());

/// Take the one run there can be. Held for the whole of a measurement.
pub fn one_at_a_time() -> MutexGuard<'static, ()> {
    ORDER.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

unsafe extern "system" fn buffer_switch(index: i32, _direct: i32) {
    let arena = ARENA.load(Ordering::Acquire);
    if arena.is_null() {
        return;
    }
    // Safety: the pointer is published before the stream is started and cleared after it has been
    // stopped, so the arena is alive for as long as this can run.
    unsafe { (*arena).block_happened((index as usize) & 1) };
}

unsafe extern "system" fn sample_rate_did_change(_hz: f64) {}

unsafe extern "system" fn message(which: i32, value: i32, _message: *mut c_void, _opt: *mut f64) -> i32 {
    match which {
        selector::SUPPORTED => i32::from(matches!(
            value,
            selector::SUPPORTED | selector::ENGINE_VERSION | selector::RESET_REQUEST | selector::LATENCIES_CHANGED
        )),
        selector::ENGINE_VERSION => ENGINE_VERSION_2,
        // The plain callback is all this host wants: it counts its own blocks.
        selector::SUPPORTS_TIME_INFO | selector::SUPPORTS_TIME_CODE => 0,
        // A reset in the middle of a measurement would silently change what is being measured, so
        // this host does not take one. The run is short enough to simply do again.
        selector::RESET_REQUEST => 0,
        _ => 1,
    }
}

unsafe extern "system" fn buffer_switch_time_info(_time: *mut Time, index: i32, _direct: i32) -> *mut Time {
    unsafe { buffer_switch(index, 0) };
    std::ptr::null_mut()
}

/// The four functions the aggregate is handed, which is what makes this a host.
pub fn callbacks() -> CallbacksRaw {
    CallbacksRaw { buffer_switch, sample_rate_did_change, message, buffer_switch_time_info }
}

// ---------------------------------------------------------------------------------------------
// The run.
// ---------------------------------------------------------------------------------------------

/// What makes the blocks happen. At the hardware the devices' own threads do, and this only
/// waits; against devices made of data the test fires them itself.
///
/// It answers whether to carry on. Answering false gives up on the run there and then, which is
/// what a person pressing stop is: the click is loud, it is in the room, and the way to end it
/// cannot be to wait for the run to finish. The devices are let go on that path exactly as they
/// are on every other.
pub type Pump<'a> = &'a mut dyn FnMut(usize) -> bool;

/// Wait for the devices to do the work, which is all a real run has to do between blocks.
pub fn wait_for_the_devices(_block: usize) -> bool {
    std::thread::sleep(Duration::from_millis(1));
    true
}

/// What a run that was given up on says, in the words the person who stopped it would use.
pub const STOPPED: &str = "the measurement was stopped part way, so nothing was measured";

/// This thread's place in COM, held for as long as a run needs one and given back after.
///
/// A thread that was already in an apartment keeps the one it had: leaving somebody else's
/// apartment would be a rudeness with consequences, and a run works perfectly well inside it.
#[cfg(windows)]
struct Apartment {
    /// True only when this entry is the one that put the thread in, and so the one to take it out.
    ours: bool,
}

#[cfg(windows)]
impl Apartment {
    fn enter() -> Apartment {
        use windows_sys::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
        // Safety: a plain call with no pointer of ours, on the thread that is about to open a
        // driver. RPC_E_CHANGED_MODE means the thread is already in the other kind of apartment,
        // which is still an apartment, so the run goes ahead and nothing is undone afterwards.
        let hr = unsafe { CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32) };
        Apartment { ours: hr >= 0 }
    }
}

#[cfg(windows)]
impl Drop for Apartment {
    fn drop(&mut self) {
        if self.ours {
            // Safety: undoes exactly the one call above, on the same thread, and only that one.
            unsafe { windows_sys::Win32::System::Com::CoUninitialize() };
        }
    }
}

/// **The one entry that opens anything.** Everything it refuses, it refuses before a device is
/// opened or a sample is played.
#[cfg(windows)]
pub fn measure(rig: &Rig, settings: &Settings) -> Outcome {
    measure_with(rig, settings, &mut wait_for_the_devices)
}

/// [`measure`], with the caller's own wait between blocks, which is how a program around this one
/// follows a run and stops it. Every refusal is still made before a device is opened.
#[cfg(windows)]
pub fn measure_with(rig: &Rig, settings: &Settings, pump: Pump<'_>) -> Outcome {
    // A vendor driver is a COM object, and the thread that asks for one has to have said so. In
    // the driver this never comes up, because the thread is a DAW's and a DAW has already done it;
    // here the thread is ours, and without this every driver refuses with CO_E_NOTINITIALIZED
    // (0x800401F0), which is what the first run at the hardware met (2026-09-20). Apartment
    // threaded, because that is what these drivers register themselves as.
    let _com = Apartment::enter();
    if let Some(why) = no_hardware_refusal(std::env::var(NO_HARDWARE).ok().as_deref()) {
        return Outcome::refused(rig.direction, why);
    }
    if let Some(why) = driver_in_use() {
        return Outcome::refused(rig.direction, why);
    }
    let Some(path) = gazelle_aggregate::config::config_path() else {
        return Outcome::refused(rig.direction, "there is no APPDATA folder to read aggregate.json from");
    };
    let config = match Config::read(&path) {
        Ok(config) => config,
        Err(why) => return Outcome::refused(rig.direction, why),
    };
    let host: Box<dyn Host> = Box::new(gazelle_aggregate::windows_host::ThisPc);
    measure_reporting(host, config, path.display().to_string(), rig, settings, run_reporter(), pump)
}

/// Whether the driver already has these interfaces, read from the record it publishes. A record
/// that is not there at all is a driver that is not running, which is the ordinary case.
#[cfg(windows)]
pub fn driver_in_use() -> Option<String> {
    use gazelle_audio_aggregate_status::windows::Section;
    use gazelle_audio_aggregate_status::Reader;

    let section = Section::open().ok()?;
    let reader = Reader::map(Box::new(section)).ok()?;
    let snapshot = reader.read().ok()?;
    in_use_refusal(snapshot.driver.open != 0, snapshot.driver.streaming != 0)
}

/// The measurement, against whatever PC it is handed, publishing nothing.
///
/// This is the seam the tests use, with sub-devices made of data. [`measure`] is the only thing
/// that hands it a real one.
pub fn measure_against(
    host: Box<dyn Host>,
    config: Config,
    source: String,
    rig: &Rig,
    settings: &Settings,
    pump: Pump<'_>,
) -> Outcome {
    measure_reporting(host, config, source, rig, settings, Arc::new(Reporter::silent()), pump)
}

/// [`measure_against`], publishing what it is doing as it goes.
///
/// The reporter is the driver's own: the aggregate underneath writes the plan, the buffers, the
/// session and every refusal into it without being asked, and what this adds is the half the
/// driver's watcher thread does, which is turning what the audio path noticed into lines of the
/// event log. A run can then be watched from the Aggregate page exactly as a DAW's session is.
pub fn measure_reporting(
    host: Box<dyn Host>,
    config: Config,
    source: String,
    rig: &Rig,
    settings: &Settings,
    reporter: Arc<Reporter>,
    pump: Pump<'_>,
) -> Outcome {
    let _order = one_at_a_time();
    let direction = rig.direction;
    // A refusal of our own is published in the same breath as it is answered, because the page
    // showing a run has the same question about a run that never started as about one that did.
    let refuse = |why: String| {
        reporter.refused(0, &why);
        Outcome::refused(direction, why)
    };
    if let Some(why) = rig.refusal() {
        return refuse(why);
    }
    if let Some(why) = settings.refusal() {
        return refuse(why);
    }

    // What the file already says, kept before the configuration is handed over, because that is
    // what this run's measurement is added to.
    let old: Vec<i32> = (0..rig.devices())
        .map(|index| {
            let device = config.devices.get(index);
            match direction {
                Direction::Inputs => device.and_then(|d| d.input_trim).unwrap_or(0),
                Direction::Outputs => device.and_then(|d| d.output_trim).unwrap_or(0),
            }
        })
        .collect();
    // And the phase each of those trims was measured at, which the new trims replace in the same
    // breath.
    let old_references: Vec<Option<i32>> = (0..rig.devices())
        .map(|index| config.devices.get(index).and_then(|d| d.phase).and_then(|phase| phase.reference))
        .collect();

    // The driver's own object, reporting the way the driver reports: everything below this line
    // that the aggregate itself refuses is written into the record and the log by the aggregate,
    // in the aggregate's own words, without anything here having to remember to say so.
    let mut aggregate = Aggregate::reporting(host, Arc::clone(&reporter));
    // Measure every phase and move nothing for it. The lag this run hears becomes a trim and the
    // phase heard beside it becomes that trim's reference, so both have to be the raw figures of
    // this one session: lining the session up first would measure the trim on top of a correction
    // made from the old reference, and the pair written down would describe no real session.
    //
    // A check is the other way round on purpose: it is lined up exactly as a DAW's session would
    // be, so what it hears is what a recording would get.
    if !settings.checking {
        aggregate.measure_trims();
    }
    if let Err(why) = aggregate.init(config, source) {
        return Outcome::refused(direction, why);
    }
    if let Some(hz) = settings.rate {
        if let Err(why) = aggregate.set_rate(hz) {
            return Outcome::refused(direction, why);
        }
    }

    let Some(plan) = aggregate.plan().cloned() else {
        return refuse("the aggregate opened without a plan, which should not be possible".to_string());
    };
    let names: Vec<String> = plan.devices.iter().map(|device| device.name.clone()).collect();
    if names.len() < 2 {
        return refuse(format!(
            "the aggregate has {} interface in it, and a lag is the difference between two of them: add the other \
             interface to aggregate.json and measure again",
            names.len()
        ));
    }
    let on_input = |channel: i32| plan.inputs.get(usize::try_from(channel).ok()?).map(|reference| reference.device);
    let on_output = |channel: i32| plan.outputs.get(usize::try_from(channel).ok()?).map(|reference| reference.device);
    if let Some(why) = rig.refusal_against(on_input, on_output, &names) {
        return refuse(why);
    }

    let rate = aggregate.rate();
    if !(rate.is_finite() && rate > 0.0) {
        return refuse("these interfaces did not say what rate they are running at".to_string());
    }
    let block = settings.buffer_size.unwrap_or(plan.preferred);

    // Everything the run needs, allocated before a single device is started.
    let samples = click::run_length(settings.clicks, settings.settle_seconds, settings.spacing_seconds, rate);
    let emits = click::schedule(settings.clicks, settings.settle_seconds, settings.spacing_seconds, rate);
    // Every interface's own channel, and then the witnesses, in one arena: a witness is recorded
    // exactly as a measured channel is, and only what is done with it afterwards differs.
    let witness_devices = rig.witness_devices(on_input);
    let mut arena = Box::new(Arena::new(
        block.max(0) as usize,
        rig.devices() + rig.witnesses.len(),
        samples,
        click::as_samples(settings.click_samples, settings.level()),
        emits,
    ));

    let wanted: Vec<Wanted> = rig
        .inputs
        .iter()
        .chain(rig.witnesses.iter())
        .map(|&channel| Wanted { is_input: true, channel })
        .chain(rig.outputs.iter().map(|&channel| Wanted { is_input: false, channel }))
        .collect();
    let pairs = match aggregate.create_buffers(&wanted, block, callbacks()) {
        Ok(pairs) => pairs,
        Err(why) => return Outcome::refused(direction, why),
    };
    let (inputs, outputs) = pairs.split_at(rig.inputs.len() + rig.witnesses.len());
    arena.buffers(inputs.to_vec(), outputs.to_vec());

    // From here on the drivers are open and something has to let them go, whatever happens.
    // What the aggregate's rings have lost so far, which is nothing on buffers this run made
    // itself, but is taken rather than assumed so that what is reported is always the difference
    // this run is answerable for. It is read again when the settling time is over, and it is that
    // second reading a measurement is judged against: see `counting_from`.
    let before = lost_so_far(&aggregate);
    let mut counting_from: Option<Vec<Glitches>> = None;
    // The block the first click is played in. The run has to have captured everything before it
    // for the counters to be read, and the poll below can only be late, never early.
    let first_click = arena.emits().first().copied().unwrap_or(0);
    let one_block = block.max(0) as usize;
    // What the audio path notices and only counts: an interface that stopped calling back, the
    // first block lost, a phase measurement settling. In the driver a thread of its own turns
    // those into lines of the log; a run has a thread of its own already, and this is it, between
    // the blocks and never on a callback.
    let mut noticing = Noticing::new();
    ARENA.store(arena.as_mut() as *mut Arena, Ordering::Release);
    let started = aggregate.start();
    let ran = match started {
        Ok(()) => {
            let limit = Duration::from_secs_f64((samples as f64 / rate) * 3.0 + 5.0);
            let since = Instant::now();
            let mut index = 0usize;
            let mut stopped = false;
            while !arena.is_full() && since.elapsed() < limit {
                // **A measurement counts only what happened while it was measuring.** A stream
                // starting is not a silent run of audio: the interfaces' callbacks find each other
                // in the first blocks of it, and an interface can take a block of silence doing so
                // before a click has been played. Reading the counters here, with the settling time
                // over and the first click about to go out, is what keeps that out of the answer,
                // while a block lost between the first click and the last still spoils the run it
                // was actually in.
                if counting_from.is_none() && arena.captured_samples() + one_block >= first_click {
                    counting_from = Some(lost_so_far(&aggregate));
                }
                noticing.look(&reporter);
                if !pump(index) {
                    stopped = true;
                    break;
                }
                index += 1;
            }
            if stopped {
                Err(STOPPED.to_string())
            } else {
                Ok(())
            }
        }
        Err(why) => Err(why),
    };
    // The last look before the audio stops, so that a measurement which settled in the final
    // blocks still gets its line, and then what every interface's phase came to, read out of the
    // record while the session that measured it is still the session in the record.
    noticing.look(&reporter);
    let phases = phases_now(&reporter, &names);
    let at_run = phases_at_run(&aggregate);
    aggregate.stop();
    // Read before the buffers go, because the rings that hold the counts go with them. A run that
    // never reached its first click is judged from where it started, which is the whole of it: it
    // is a refusal either way, and a count taken from nowhere would be worse than one taken early.
    let glitches = since(counting_from.as_deref().unwrap_or(&before), &lost_so_far(&aggregate));
    aggregate.dispose_buffers();
    ARENA.store(std::ptr::null_mut(), Ordering::Release);
    drop(aggregate);

    if let Err(why) = ran {
        // A run the aggregate itself refused has already been written down, in its own words. A
        // run somebody stopped is ours to say.
        return if why == STOPPED { refuse(why) } else { Outcome::refused(direction, why) };
    }
    if arena.captured_samples() == 0 {
        return refuse(
            "the interfaces never called back, so nothing was recorded at all: they opened and then did nothing, \
             which usually means another program still has them"
                .to_string(),
        );
    }

    let mut outcome = read_arena(&arena, rig, settings, &names, &witness_devices, &old, rate, block, &glitches);
    drop(arena);
    trim::pair_with_phases(&mut outcome.trims, &old_references, &at_run);
    outcome.phases = phases;
    if settings.checking {
        // Heard on top of the correction, so it says whether the trim holds and nothing about
        // what a new one should be.
        outcome.trims.clear();
        outcome.checking = true;
    }
    if let Some(why) = &outcome.refusal {
        reporter.refused(0, why);
    }
    outcome
}

/// What each interface's phase came to, read from the audio path itself rather than from the
/// record, because the trims are paired with it and a trim is measured whether or not there was
/// anywhere to publish. In device order.
fn phases_at_run(aggregate: &Aggregate) -> Vec<PhaseAtRun> {
    let Some(stream) = aggregate.stream() else { return Vec::new() };
    stream
        .devices
        .iter()
        .map(|device| match device.phase_state.load(Ordering::Acquire) {
            phase_codes::NOT_CONFIGURED => PhaseAtRun::NotMeasured,
            phase_codes::MEASURED_ONLY => PhaseAtRun::Heard(device.phase_measured.load(Ordering::Acquire)),
            _ => PhaseAtRun::NotHeard,
        })
        .collect()
}

/// What each interface's rings have lost since the buffers were made, in device order.
fn lost_so_far(aggregate: &Aggregate) -> Vec<Glitches> {
    aggregate
        .stream()
        .map(|stream| {
            stream
                .glitches()
                .into_iter()
                .map(|glitch| Glitches { dropped: glitch.dropped, starved: glitch.starved })
                .collect()
        })
        .unwrap_or_default()
}

/// What happened between two readings of those counters, which is what this run is answerable for.
fn since(before: &[Glitches], after: &[Glitches]) -> Vec<Glitches> {
    after
        .iter()
        .enumerate()
        .map(|(index, now)| {
            let was = before.get(index).copied().unwrap_or_default();
            Glitches {
                dropped: now.dropped.saturating_sub(was.dropped),
                starved: now.starved.saturating_sub(was.starved),
            }
        })
        .collect()
}

/// What the run captured, turned into readings and trims. No hardware, and no state: this is the
/// same arithmetic the measurement tests run against signals made of data.
#[allow(clippy::too_many_arguments)]
fn read_arena(
    arena: &Arena,
    rig: &Rig,
    settings: &Settings,
    names: &[String],
    witness_devices: &[usize],
    old: &[i32],
    rate: f64,
    block: i32,
    glitches: &[Glitches],
) -> Outcome {
    let click = click::shape(settings.click_samples, settings.level());
    let reference = arena.channel(rig.reference);
    // A search that reaches past the next click would find the next click, so it stops short of it.
    let horizon = ((settings.spacing_seconds * rate) as usize / 2).max(1);
    let emits = arena.emits().to_vec();

    // Nothing on the reference is not the same as nothing on a channel: every other interface is
    // measured against it, so its cable is the whole run, and saying "nothing arrived" about all
    // of them would send somebody checking cables that are fine.
    let of_reference = measure::lags_of_channel(&reference, &reference, &click, &emits, horizon, settings.search_samples);
    if of_reference.is_empty() {
        let name = names.get(rig.reference).map(String::as_str).unwrap_or("the reference interface");
        return Outcome::refused(
            rig.direction,
            format!(
                "the click never came back on {name}, which is the interface everything else is measured against, so \
                 there is nothing to measure against: check that cable first, and that input channel {} is the one \
                 it is plugged into",
                rig.inputs.get(rig.reference).copied().unwrap_or_default()
            ),
        );
    }

    let mut readings = Vec::new();
    for index in 0..rig.devices() {
        let name = names.get(index).cloned().unwrap_or_else(|| format!("interface {index}"));
        let lags = if index == rig.reference {
            // The reference against itself is zero, and saying so keeps every list the same
            // length and every index the device's own.
            of_reference.clone()
        } else {
            let channel = arena.channel(index);
            measure::lags_of_channel(&reference, &channel, &click, &emits, horizon, settings.search_samples)
        };
        let lost = glitches.get(index).copied().unwrap_or_default();
        readings.push(measure::summarise(&name, &lags, emits.len(), rate, block, lost));
    }

    // The witnesses, after the readings and never among them. Each one is measured by the same
    // call the readings are, and then it is simply reported: nothing here can refuse a run, throw
    // a reading out or move a trim, because a witness is something heard and not something
    // measured against.
    let mut witnesses = Vec::new();
    for (index, &channel) in rig.witnesses.iter().enumerate() {
        let on = witness_devices.get(index).copied().unwrap_or(rig.reference);
        let device = names.get(on).cloned().unwrap_or_else(|| format!("interface {on}"));
        let capture = arena.channel(rig.devices() + index);
        let lags = measure::lags_of_channel(&reference, &capture, &click, &emits, horizon, settings.search_samples);
        // A witness has no name of its own, so it is called what it is: a channel, and whose.
        let called = format!("input channel {channel} on {device}");
        let lost = glitches.get(on).copied().unwrap_or_default();
        let reading = measure::summarise(&called, &lags, emits.len(), rate, block, lost);
        witnesses.push(Witness { channel, device, reading });
    }

    let trims = trim::implied_for_all(&readings, rig.direction, old, rig.reference);
    Outcome {
        direction: rig.direction,
        rate,
        block,
        clicks: settings.clicks,
        readings,
        witnesses,
        trims,
        // What the interfaces' phase came to is read from the record, not from the audio this
        // captured, so it is put in by the run itself.
        phases: Vec::new(),
        // Set by the run once it knows, since only the run knows whether it was a check.
        checking: false,
        refusal: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_variable_that_keeps_a_test_off_the_hardware_refuses_this_outright() {
        let refusal = no_hardware_refusal(Some("1")).expect("set means refuse");
        assert!(refusal.contains(NO_HARDWARE), "{refusal}");
        assert!(refusal.contains("real converters"), "it says what it would have done: {refusal}");
        assert_eq!(no_hardware_refusal(None), None);
        assert_eq!(no_hardware_refusal(Some("0")), None);
        assert_eq!(no_hardware_refusal(Some("")), None);
        assert_eq!(no_hardware_refusal(Some("false")), None);
    }

    #[test]
    fn a_driver_that_already_has_the_interfaces_is_a_refusal_that_says_what_to_close() {
        let refusal = in_use_refusal(true, false).expect("open is enough");
        assert!(refusal.contains("already open"), "{refusal}");
        assert!(refusal.contains("close the DAW"), "{refusal}");
        assert!(in_use_refusal(false, true).is_some(), "streaming counts too");
        assert_eq!(in_use_refusal(false, false), None);
    }

    #[test]
    fn a_refusal_carries_no_readings_and_no_trims() {
        let outcome = Outcome::refused(Direction::Inputs, "no");
        assert!(outcome.readings.is_empty() && outcome.trims.is_empty());
        assert!(!outcome.is_measured());
        assert_eq!(outcome.refusal.as_deref(), Some("no"));
    }

    #[test]
    fn a_cabling_mistake_is_refused_before_a_single_device_is_opened() {
        // No host is handed over at all, so a refusal that reached the aggregate would panic here.
        let rig = Rig::new(Direction::Inputs, vec![0], vec![0]);
        assert!(rig.refusal().is_some());
        let settings = Settings { clicks: 1, ..Settings::default() };
        assert!(settings.refusal().is_some());
    }
}
