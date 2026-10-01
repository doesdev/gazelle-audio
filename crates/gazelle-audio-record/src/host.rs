//! **Hosting the aggregate**, as a calibration run does, for as long as the recorder is armed or the
//! metronome is running.
//!
//! This is the calibration's machinery, not a copy of it (`gazelle_calibrate::session`): the same
//! driver object opened with the configuration Gazelle keeps, the same reporter so the Aggregate page
//! and the driver's log see a session, marked as Gazelle's own, the same COM apartment on the thread
//! that opens the drivers, and **the same turn**. There is one owner of the aggregate in Gazelle: a
//! measurement holds the turn while it runs and a session holds it from the moment it opens to the
//! moment it closes, so neither can start while the other has the interfaces.
//!
//! # One session, two users
//!
//! The recorder and the metronome share one session ([`crate::engine`]), so it is opened with what
//! either could want, once, and never has to be made again while it is open:
//!
//! - **every input the aggregate offers.** Arming picks the preset's channels out of them and starts
//!   copying those into the capture ring ([`Tap`]); a session the metronome opened can be armed
//!   without being stopped, and disarming leaves the metronome playing.
//! - **the metronome's outputs** and no others: the ones the person picked, by interface and that
//!   interface's own channel. The metronome's generator ([`crate::metronome::Generator`]) fills them
//!   every block, with the click or with silence, and the aggregate clears every other output of
//!   every interface on every block before anything is handed out, so nothing else is ever played.
//!   Outputs that cannot be placed are left out and said so ([`Opened::outputs_problem`]); they never
//!   stop the recorder arming.
//! - **the phase cable, when the setup has one** ([`Opened::phase_path`]): while the recorder is armed
//!   the aggregate keeps a check running over it ([`crate::alignment`]), and the callback has nothing
//!   to do for that beyond what the aggregate's own audio path does.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use gazelle_aggregate::aggregate::{Aggregate, BufferPair, Wanted};
use gazelle_aggregate::config::{Alignment, Config};
use gazelle_aggregate::phase::Watch;
use gazelle_aggregate::plan::Plan;
use gazelle_aggregate::status::{Glitches, Noticing, Reporter};
use gazelle_aggregate::stream::Stream;
use gazelle_aggregate::sub::Host;
use gazelle_audio_stream_abi::raw::{selector, CallbacksRaw, Time, ENGINE_VERSION_2};
use gazelle_calibrate::{Layout, Pick};
use serde::Serialize;

use crate::capture::Capture;
use crate::metronome::{Generator, Params, Then};
use crate::sizing::{self, Shape, Sizing};
use crate::system::Memory;

/// What every line Gazelle's own sessions write to the driver's log starts with, so a person
/// reading it can tell Gazelle recording from a DAW, and from a measurement.
pub const MARK: &str = "Gazelle's own recording:";

/// What a session is opened with.
#[derive(Clone, Debug)]
pub struct OpenRequest {
    /// The aggregate's setup, exactly as the driver would read it.
    pub config: Config,
    /// Where it came from, for anything that has to say so.
    pub source: String,
    /// The metronome's outputs, by interface and that interface's own channel.
    pub outputs: Vec<Pick>,
    /// What the metronome plays, until it is told otherwise.
    pub params: Params,
}

/// What Arm asks of an open session.
#[derive(Clone, Debug)]
pub struct ArmRequest {
    /// The channels to record, by interface and that interface's own number, never the aggregate's.
    pub picks: Vec<Pick>,
    pub percent: f64,
    pub cap_seconds: Option<f64>,
    /// The aggregate's setup, for a session Arm has to open itself.
    pub config: Config,
    /// Where it came from, for anything that has to say so.
    pub source: String,
}

/// One channel of the aggregate, input or output.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Channel {
    /// Its name in the aggregate, which is what a DAW shows and what a recorded file is called.
    pub name: String,
    /// The interface it is on, by the aggregate's name for it.
    pub device: String,
    pub device_index: usize,
    /// Its number on that interface, from zero.
    pub channel: i32,
}

/// What an open session turned out to be.
#[derive(Clone, Debug)]
pub struct Opened {
    pub rate: f64,
    pub block: usize,
    /// The interface driving the callback, by its place in the setup.
    pub master: usize,
    pub layout: Layout,
    /// Every input the aggregate offers, in its order.
    pub inputs: Vec<Channel>,
    /// The metronome's outputs that could be placed, in the order they were picked.
    pub outputs: Vec<Channel>,
    /// Why some picked outputs were left out, when they were.
    pub outputs_problem: Option<String>,
    /// The cable dedicated to the phase measurement, which the alignment is checked over while the
    /// recorder is armed, or why there is nothing to check it over.
    pub phase_path: Result<PhasePath, String>,
}

/// The cable a follower's phase is measured over, as the setup has it.
#[derive(Clone, Debug, PartialEq)]
pub struct PhasePath {
    /// The interface whose input the cable arrives on, by its place in the setup.
    pub device: usize,
    /// Its name in the aggregate.
    pub name: String,
    /// Where the cable leaves among the master's opened outputs, and where it arrives among this
    /// interface's opened inputs.
    pub master_slot: usize,
    pub input_slot: usize,
    /// The phase measured when its trim was measured, if one has been.
    pub reference: Option<i32>,
}

/// No cable to check the alignment over.
pub const NO_PHASE_PATH: &str = "no phase path is set up, so there is no cable to send a check down. Set one up on the Aggregate page, under Phase path";
/// The aggregate does not line the interfaces up at all.
pub const NOT_ALIGNED: &str = "the aggregate is set to the lowest latency, which does not line the interfaces up, so there is no alignment to check";

impl PhasePath {
    /// The first follower the setup gives a phase cable, or why the alignment cannot be checked.
    pub fn of(plan: &Plan) -> Result<PhasePath, String> {
        let (device, found) = plan.devices.iter().enumerate().find_map(|(at, device)| device.phase.map(|phase| (at, (device, phase)))).ok_or_else(|| NO_PHASE_PATH.to_string())?;
        if plan.alignment != Alignment::Aligned {
            return Err(NOT_ALIGNED.into());
        }
        let (plan_device, phase) = found;
        Ok(PhasePath { device, name: plan_device.name.clone(), master_slot: phase.master_slot, input_slot: phase.input_slot, reference: phase.reference })
    }
}

/// **What the recorder hangs on the callback while armed**: which of the session's inputs to copy,
/// and the ring they go into.
pub struct Tap {
    /// For each recorded channel, its place in [`Shared`]'s inputs.
    map: Vec<usize>,
    capture: Arc<Capture>,
    /// The session position of the ring's position zero, written by the callback on every block:
    /// what turns a check's place in the session into its place in a take. [`UNSET`] until the
    /// first block.
    origin: Arc<AtomicU64>,
}

/// An origin no block has written yet.
pub const UNSET: u64 = u64::MAX;

impl Tap {
    pub fn capture(&self) -> &Arc<Capture> {
        &self.capture
    }

    /// Where the ring's position zero is in the session, shared with whatever reads it off the
    /// audio thread.
    pub fn origin(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.origin)
    }
}

/// An armed recorder's share of a session: the tap, and what it records.
pub struct Armed {
    pub tap: Arc<Tap>,
    pub channels: Vec<Channel>,
    pub sizing: Sizing,
}

impl Armed {
    /// Resolve the preset's channels against an open session, size the pre-roll and reserve it.
    /// Everything it refuses, it refuses in a sentence.
    pub fn prepare(opened: &Opened, request: &ArmRequest, memory: &dyn Memory) -> Result<Armed, String> {
        if request.picks.is_empty() {
            return Err("the preset records no channels: choose at least one".into());
        }
        for (at, pick) in request.picks.iter().enumerate() {
            if request.picks[..at].contains(pick) {
                return Err(format!("input {} of interface {} is chosen twice", pick.channel + 1, pick.device + 1));
            }
        }
        let mut map = Vec::new();
        let mut channels = Vec::new();
        for pick in &request.picks {
            let index = opened
                .layout
                .inputs
                .iter()
                .position(|&(device, channel)| device as i32 == pick.device && channel == pick.channel)
                .ok_or_else(|| unplaced(&opened.layout, *pick))?;
            map.push(index);
            channels.push(opened.inputs[index].clone());
        }
        let shape = Shape { rate: opened.rate, channels: channels.len(), block: opened.block };
        let sizing = sizing::size(memory.available(), request.percent, request.cap_seconds, shape)?;
        let capture = Arc::new(Capture::allocate(channels.len(), opened.block, sizing.capacity_frames, sizing.preroll_frames)?);
        Ok(Armed { tap: Arc::new(Tap { map, capture, origin: Arc::new(AtomicU64::new(UNSET)) }), channels, sizing })
    }
}

/// **What the callback reads**: the session's buffers, the recorder's tap when it is armed, and the
/// metronome's generator.
pub struct Shared {
    inputs: Vec<BufferPair>,
    outputs: Vec<BufferPair>,
    block: usize,
    /// Null while nothing is armed. Made from `Arc::into_raw`.
    tap: AtomicPtr<Tap>,
    /// The callback is looking at the tap.
    in_tap: AtomicBool,
    pub generator: Generator,
    /// The aggregate's audio path, for hanging the alignment check's watch on it. Nothing here
    /// calls into it from the callback.
    stream: Option<Arc<Stream>>,
}

// The buffer pointers are the aggregate's, alive for as long as the session is open, and written
// only by the callback; the tap and the generator keep their own rules.
unsafe impl Send for Shared {}
unsafe impl Sync for Shared {}

impl Shared {
    /// Hang an armed recorder's tap on the callback. It starts at the next block.
    pub fn attach(&self, tap: Arc<Tap>) -> Result<(), String> {
        let raw = Arc::into_raw(tap) as *mut Tap;
        if self.tap.compare_exchange(std::ptr::null_mut(), raw, Ordering::SeqCst, Ordering::SeqCst).is_err() {
            // Safety: made by `into_raw` just above and published nowhere.
            drop(unsafe { Arc::from_raw(raw) });
            return Err("the recorder is armed already".into());
        }
        Ok(())
    }

    /// Take the tap off the callback, and answer once no callback can be using it any more. From
    /// then on the capture is the caller's alone, as it would be once the stream had stopped.
    pub fn detach(&self) {
        let raw = self.tap.swap(std::ptr::null_mut(), Ordering::SeqCst);
        if raw.is_null() {
            return;
        }
        // A callback that got the tap before the swap says so until it is done with it.
        while self.in_tap.load(Ordering::SeqCst) {
            std::thread::yield_now();
        }
        // Safety: made by `into_raw` in `attach`, and nothing can reach it now.
        drop(unsafe { Arc::from_raw(raw) });
    }

    pub fn is_armed(&self) -> bool {
        !self.tap.load(Ordering::Acquire).is_null()
    }

    /// Start the alignment check over the phase cable: the aggregate sends its burst down it from
    /// its next block on, and copies what arrives into the watch.
    pub fn watch(&self, watch: Arc<Watch>) -> Result<(), String> {
        self.stream.as_ref().ok_or("the aggregate has no audio path open")?.watch(watch)
    }

    /// Stop it. Answers once the aggregate's callback cannot be using the watch any more.
    pub fn unwatch(&self) {
        if let Some(stream) = &self.stream {
            stream.unwatch();
        }
    }

    /// One block. Nothing here allocates, locks or logs.
    fn block_happened(&self, half: usize) {
        // Safety: this is the callback, the generator's one caller, and the output buffers are the
        // aggregate's own for this session.
        let events = unsafe { self.generator.fill_outputs(&self.outputs, half, self.block) };
        self.in_tap.store(true, Ordering::SeqCst);
        let tap = self.tap.load(Ordering::SeqCst);
        if !tap.is_null() {
            // Safety: `detach` waits for `in_tap` to fall before it lets the tap go.
            let tap = unsafe { &*tap };
            // The ring counts from Arm and the session from its start, and the difference is fixed.
            tap.origin.store(events.block_start.wrapping_sub(tap.capture.written()), Ordering::Release);
            if let Some(counted) = events.counted.filter(|counted| counted.then == Then::Record) {
                // The ring counts from Arm and the generator from the session's start: the same
                // block is `written` in one and `block_start` in the other.
                let written = tap.capture.written();
                let pressed = written.saturating_sub(events.block_start - counted.pressed);
                let downbeat = written + (counted.downbeat - events.block_start);
                tap.capture.start_take_counted(pressed, downbeat);
            }
            tap.capture.push_with(|channel, run| {
                let from = tap.map.get(channel).and_then(|&index| self.inputs.get(index)).map_or(std::ptr::null_mut(), |pair| pair[half & 1]) as *const i32;
                if from.is_null() {
                    run.fill(0);
                    return;
                }
                // Safety: the aggregate made this buffer, `block` samples of `i32`, the one sample type
                // it presents, and it is alive from createBuffers to disposeBuffers.
                run.copy_from_slice(unsafe { std::slice::from_raw_parts(from, self.block) });
            });
        }
        self.in_tap.store(false, Ordering::Release);
    }
}

impl Drop for Shared {
    fn drop(&mut self) {
        self.detach();
    }
}

/// The session in flight, for the callbacks to find. Null except between start and stop. Made from
/// an `Arc<Shared>` the session keeps.
static SHARED: AtomicPtr<Shared> = AtomicPtr::new(std::ptr::null_mut());
/// A driver asked to be reset while open. Gazelle cannot reset under a take, so it says so and
/// leaves it to the person: disarm and arm again.
static RESET_ASKED: AtomicBool = AtomicBool::new(false);

unsafe extern "system" fn buffer_switch(index: i32, _direct: i32) {
    let shared = SHARED.load(Ordering::Acquire);
    if shared.is_null() {
        return;
    }
    // Safety: published before the stream starts and cleared after it has stopped, so it is alive
    // for as long as this can run.
    unsafe { (*shared).block_happened((index as usize) & 1) };
}

unsafe extern "system" fn sample_rate_did_change(_hz: f64) {
    RESET_ASKED.store(true, Ordering::Release);
}

unsafe extern "system" fn message(which: i32, value: i32, _message: *mut c_void, _opt: *mut f64) -> i32 {
    match which {
        selector::SUPPORTED => i32::from(matches!(value, selector::SUPPORTED | selector::ENGINE_VERSION | selector::RESET_REQUEST | selector::LATENCIES_CHANGED)),
        selector::ENGINE_VERSION => ENGINE_VERSION_2,
        selector::SUPPORTS_TIME_INFO | selector::SUPPORTS_TIME_CODE => 0,
        // Resetting would end the take mid-note. It is noticed, and the page says so.
        selector::RESET_REQUEST => {
            RESET_ASKED.store(true, Ordering::Release);
            0
        }
        _ => 1,
    }
}

unsafe extern "system" fn buffer_switch_time_info(_time: *mut Time, index: i32, _direct: i32) -> *mut Time {
    unsafe { buffer_switch(index, 0) };
    std::ptr::null_mut()
}

fn callbacks() -> CallbacksRaw {
    CallbacksRaw { buffer_switch, sample_rate_did_change, message, buffer_switch_time_info }
}

/// Whether a driver has asked to be reset since the session opened.
pub fn reset_asked() -> bool {
    RESET_ASKED.load(Ordering::Acquire)
}

/// What to say when a measurement has the aggregate.
pub const MEASURING: &str = "a measurement on the Aggregate page is using the interfaces: wait for it to finish, or stop it, and try again";

/// **An open session**: the aggregate open and streaming, holding the turn.
///
/// Not `Send`: it holds the turn, and it lives and dies on the thread that opened it, which is the
/// thread that is in the drivers' apartment.
pub struct Session {
    aggregate: Aggregate,
    shared: Arc<Shared>,
    reporter: Arc<Reporter>,
    noticing: Noticing,
    dropouts: Arc<Mutex<Vec<Glitches>>>,
    pub opened: Opened,
    closed: bool,
    _turn: MutexGuard<'static, ()>,
}

impl Session {
    /// Open the aggregate with every input and the metronome's outputs, and start streaming.
    /// Everything it refuses, it refuses in a sentence, and nothing is left open when it does.
    pub fn open(host: Box<dyn Host>, request: &OpenRequest, reporter: Arc<Reporter>) -> Result<Session, String> {
        let turn = gazelle_calibrate::session::try_one_at_a_time().ok_or_else(|| MEASURING.to_string())?;
        RESET_ASKED.store(false, Ordering::Release);

        let mut aggregate = Aggregate::reporting(host, Arc::clone(&reporter));
        aggregate.init(request.config.clone(), request.source.clone())?;
        let plan = aggregate.plan().cloned().ok_or("the aggregate opened without a plan, which should not be possible")?;
        let layout = Layout::of(&plan, aggregate.descriptions());
        let phase_path = PhasePath::of(&plan);
        let named = |names: &[String], index: usize, fallback: &str, device: usize, channel: i32| Channel {
            name: names.get(index).cloned().unwrap_or_else(|| format!("{fallback} {}", index + 1)),
            device: layout.interfaces.get(device).map_or_else(String::new, |i| i.name.clone()),
            device_index: device,
            channel,
        };
        let inputs: Vec<Channel> = layout.inputs.iter().enumerate().map(|(index, &(device, channel))| named(&plan.input_names, index, "Input", device, channel)).collect();

        let mut outputs = Vec::new();
        let mut output_indices = Vec::new();
        let mut left_out = Vec::new();
        for (at, pick) in request.outputs.iter().enumerate() {
            if request.outputs[..at].contains(pick) {
                continue;
            }
            match layout.outputs.iter().position(|&(device, channel)| device as i32 == pick.device && channel == pick.channel) {
                Some(index) => {
                    output_indices.push(index);
                    outputs.push(named(&plan.output_names, index, "Output", index_device(&layout, index), pick.channel));
                }
                None => left_out.push(unplaced_output(&layout, *pick)),
            }
        }
        let outputs_problem = (!left_out.is_empty()).then(|| left_out.join("; "));

        let rate = aggregate.rate();
        if !(rate.is_finite() && rate > 0.0) {
            return Err("these interfaces did not say what rate they are running at".into());
        }
        let block = plan.preferred.max(1) as usize;
        let generator = Generator::new(rate, block, request.params);
        let wanted: Vec<Wanted> = (0..inputs.len())
            .map(|channel| Wanted { is_input: true, channel: channel as i32 })
            .chain(output_indices.iter().map(|&channel| Wanted { is_input: false, channel: channel as i32 }))
            .collect();
        let pairs = aggregate.create_buffers(&wanted, block as i32, callbacks())?;
        let (input_pairs, output_pairs) = pairs.split_at(inputs.len());
        let shared = Arc::new(Shared {
            inputs: input_pairs.to_vec(),
            outputs: output_pairs.to_vec(),
            block,
            tap: AtomicPtr::new(std::ptr::null_mut()),
            in_tap: AtomicBool::new(false),
            generator,
            stream: aggregate.stream().cloned(),
        });
        SHARED.store(Arc::as_ptr(&shared) as *mut Shared, Ordering::Release);
        if let Err(why) = aggregate.start() {
            SHARED.store(std::ptr::null_mut(), Ordering::Release);
            aggregate.dispose_buffers();
            return Err(why);
        }
        let master = aggregate.stream().map_or(0, |stream| stream.master);
        let dropouts = Arc::new(Mutex::new(aggregate.stream().map(|s| s.glitches()).unwrap_or_default()));
        Ok(Session {
            aggregate,
            shared,
            reporter,
            noticing: Noticing::new(),
            dropouts,
            opened: Opened { rate, block, master, layout, inputs, outputs, outputs_problem, phase_path },
            closed: false,
            _turn: turn,
        })
    }

    pub fn shared(&self) -> Arc<Shared> {
        Arc::clone(&self.shared)
    }

    /// What the start of the session measured on the phase cable, for the tests.
    #[cfg(test)]
    pub(crate) fn phase_measured(&self) -> Option<i32> {
        let path = self.opened.phase_path.as_ref().ok()?;
        Some(self.aggregate.stream()?.devices.get(path.device)?.phase_measured.load(Ordering::Acquire))
    }

    /// The aggregate's own counters of what each interface lost, kept up to date by
    /// [`Session::housekeeping`], for the writer and the page.
    pub fn dropouts(&self) -> Arc<Mutex<Vec<Glitches>>> {
        Arc::clone(&self.dropouts)
    }

    /// Between blocks, never on a callback: turn what the audio path noticed into lines of the event
    /// log, and read what the interfaces have lost.
    pub fn housekeeping(&mut self) {
        self.noticing.look(&self.reporter);
        if let Some(stream) = self.aggregate.stream() {
            if let Ok(mut dropouts) = self.dropouts.lock() {
                *dropouts = stream.glitches();
            }
        }
    }

    /// Stop the stream and let go of every driver. A tap still on the callback has its take ended
    /// where the audio ended; the writer finishes it from the ring, which outlives the session.
    pub fn close(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        self.housekeeping();
        self.shared.unwatch();
        self.aggregate.stop();
        SHARED.store(std::ptr::null_mut(), Ordering::Release);
        let tap = self.shared.tap.load(Ordering::SeqCst);
        if !tap.is_null() {
            // Safety: the stream has stopped, so no callback holds it; the recorder's own Arc keeps it.
            unsafe { (*tap).capture.close_after_stream_stopped() };
        }
        self.shared.detach();
        self.aggregate.dispose_buffers();
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.close();
    }
}

/// The interface an aggregate output is on.
fn index_device(layout: &Layout, index: usize) -> usize {
    layout.outputs.get(index).map_or(0, |&(device, _)| device)
}

/// Why a pick is not an input of this aggregate, in words.
fn unplaced(layout: &Layout, pick: Pick) -> String {
    let Some(interface) = usize::try_from(pick.device).ok().and_then(|at| layout.interfaces.get(at)) else {
        return format!("the preset records from interface {}, and the aggregate has {}: choose its channels again", pick.device + 1, layout.interfaces.len());
    };
    if interface.phase_input == Some(pick.channel) {
        return format!("input {} of {} is kept for the phase measurement, so the aggregate does not offer it: choose another", pick.channel + 1, interface.name);
    }
    format!(
        "{} has {} inputs in the aggregate, and input {} is not one of them: choose the preset's channels again",
        interface.name,
        layout.inputs.iter().filter(|(device, _)| *device as i32 == pick.device).count(),
        pick.channel + 1
    )
}

/// Why a metronome output is not an output of this aggregate, in words.
fn unplaced_output(layout: &Layout, pick: Pick) -> String {
    let Some(interface) = usize::try_from(pick.device).ok().and_then(|at| layout.interfaces.get(at)) else {
        return format!("the metronome plays to interface {}, and the aggregate has {}", pick.device + 1, layout.interfaces.len());
    };
    if interface.phase_outputs.iter().any(|(channel, _)| *channel == pick.channel) {
        return format!("output {} of {} is kept for the phase measurement, so the metronome cannot play to it", pick.channel + 1, interface.name);
    }
    format!("{} has no output {} in the aggregate", interface.name, pick.channel + 1)
}
