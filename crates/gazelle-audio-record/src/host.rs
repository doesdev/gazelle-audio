//! **Hosting the aggregate**, as a calibration run does, for as long as the recorder is armed.
//!
//! This is the calibration's machinery, not a copy of it (`gazelle_calibrate::session`): the same
//! driver object opened with the configuration Gazelle keeps, the same reporter so the Aggregate page
//! and the driver's log see a session, marked as the recorder's own, the same COM apartment on the
//! thread that opens the drivers, and **the same turn**. There is one owner of the aggregate in
//! Gazelle: a measurement holds the turn while it runs and the recorder holds it from Arm to Disarm,
//! so neither can start while the other has the interfaces.
//!
//! What differs is what the callback does with a block. A run fills an arena and stops; the recorder
//! copies each input it was asked for into the capture ring ([`crate::capture`]) for as long as it is
//! armed, and asks for **no outputs at all**: the aggregate clears every output of every interface
//! on every block before anything is handed out, so a host that plays nothing plays silence.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use gazelle_aggregate::aggregate::{Aggregate, Wanted};
use gazelle_aggregate::config::Config;
use gazelle_aggregate::status::{Glitches, Noticing, Reporter};
use gazelle_aggregate::sub::Host;
use gazelle_audio_stream_abi::raw::{selector, CallbacksRaw, Time, ENGINE_VERSION_2};
use gazelle_calibrate::{Layout, Pick};
use serde::Serialize;

use crate::capture::Capture;
use crate::sizing::{self, Shape, Sizing};
use crate::system::Memory;

/// What every line the recorder's sessions write to the driver's log starts with, so a person
/// reading it can tell Gazelle recording from a DAW, and from a measurement.
pub const MARK: &str = "Gazelle's own recording:";

/// What Arm asks the aggregate for.
#[derive(Clone, Debug)]
pub struct ArmRequest {
    /// The channels to record, by interface and that interface's own number, never the aggregate's.
    pub picks: Vec<Pick>,
    pub percent: f64,
    pub cap_seconds: Option<f64>,
    /// The aggregate's setup, exactly as the driver would read it.
    pub config: Config,
    /// Where it came from, for anything that has to say so.
    pub source: String,
}

/// One recorded channel.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Channel {
    /// Its name in the aggregate, which is what a DAW shows and what its file is called.
    pub name: String,
    /// The interface it is on, by the aggregate's name for it.
    pub device: String,
    pub device_index: usize,
    /// Its number on that interface, from zero.
    pub channel: i32,
}

/// What an armed session turned out to be.
#[derive(Clone, Debug)]
pub struct Opened {
    pub rate: f64,
    pub block: usize,
    pub channels: Vec<Channel>,
    pub sizing: Sizing,
    /// The interface driving the callback, by its place in the setup.
    pub master: usize,
}

/// What the callback reads: the aggregate's input buffers for the channels being recorded, and the
/// ring they go into.
struct Tap {
    inputs: Vec<[*mut c_void; 2]>,
    block: usize,
    capture: Arc<Capture>,
}

impl Tap {
    /// One block. Nothing here allocates, locks or logs.
    fn block_happened(&self, half: usize) {
        self.capture.push_with(|channel, run| {
            let from = self.inputs.get(channel).map_or(std::ptr::null_mut(), |pair| pair[half & 1]) as *const i32;
            if from.is_null() {
                run.fill(0);
                return;
            }
            // Safety: the aggregate made this buffer, `block` samples of `i32`, the one sample type
            // it presents, and it is alive from createBuffers to disposeBuffers.
            run.copy_from_slice(unsafe { std::slice::from_raw_parts(from, self.block) });
        });
    }
}

/// The session in flight, for the callbacks to find. Null except between start and stop.
static TAP: AtomicPtr<Tap> = AtomicPtr::new(std::ptr::null_mut());
/// A driver asked to be reset while armed. The recorder cannot reset under a take, so it says so and
/// leaves it to the person: disarm and arm again.
static RESET_ASKED: AtomicBool = AtomicBool::new(false);

unsafe extern "system" fn buffer_switch(index: i32, _direct: i32) {
    let tap = TAP.load(Ordering::Acquire);
    if tap.is_null() {
        return;
    }
    // Safety: published before the stream starts and cleared after it has stopped, so the tap is
    // alive for as long as this can run.
    unsafe { (*tap).block_happened((index as usize) & 1) };
}

unsafe extern "system" fn sample_rate_did_change(_hz: f64) {
    RESET_ASKED.store(true, Ordering::Release);
}

unsafe extern "system" fn message(which: i32, value: i32, _message: *mut c_void, _opt: *mut f64) -> i32 {
    match which {
        selector::SUPPORTED => i32::from(matches!(
            value,
            selector::SUPPORTED | selector::ENGINE_VERSION | selector::RESET_REQUEST | selector::LATENCIES_CHANGED
        )),
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

/// Whether a driver has asked to be reset since Arm.
pub fn reset_asked() -> bool {
    RESET_ASKED.load(Ordering::Acquire)
}

/// What to say when a measurement has the aggregate.
pub const MEASURING: &str = "a measurement on the Aggregate page is using the interfaces: wait for it to finish, or stop it, and arm again";

/// **An armed session**: the aggregate open, streaming into the ring, holding the turn.
///
/// Not `Send`: it holds the turn, and it lives and dies on the thread that opened it, which is the
/// thread that is in the drivers' apartment.
pub struct Session {
    aggregate: Aggregate,
    capture: Arc<Capture>,
    tap: Option<Box<Tap>>,
    reporter: Arc<Reporter>,
    noticing: Noticing,
    dropouts: Arc<Mutex<Vec<Glitches>>>,
    pub opened: Opened,
    closed: bool,
    _turn: MutexGuard<'static, ()>,
}

impl Session {
    /// Open the aggregate, size and reserve the pre-roll, and start streaming into it. Everything it
    /// refuses, it refuses in a sentence, and nothing is left open when it does.
    pub fn open(host: Box<dyn Host>, request: &ArmRequest, memory: &dyn Memory, reporter: Arc<Reporter>) -> Result<Session, String> {
        if request.picks.is_empty() {
            return Err("the preset records no channels: choose at least one".into());
        }
        for (at, pick) in request.picks.iter().enumerate() {
            if request.picks[..at].contains(pick) {
                return Err(format!("input {} of interface {} is chosen twice", pick.channel + 1, pick.device + 1));
            }
        }
        let turn = gazelle_calibrate::session::try_one_at_a_time().ok_or_else(|| MEASURING.to_string())?;
        RESET_ASKED.store(false, Ordering::Release);

        let mut aggregate = Aggregate::reporting(host, Arc::clone(&reporter));
        aggregate.init(request.config.clone(), request.source.clone())?;
        let plan = aggregate.plan().cloned().ok_or("the aggregate opened without a plan, which should not be possible")?;
        let layout = Layout::of(&plan, aggregate.descriptions());
        let mut wanted = Vec::new();
        let mut channels = Vec::new();
        for pick in &request.picks {
            let index = layout
                .inputs
                .iter()
                .position(|&(device, channel)| device as i32 == pick.device && channel == pick.channel)
                .ok_or_else(|| unplaced(&layout, *pick))?;
            let device = pick.device as usize;
            channels.push(Channel {
                name: plan.input_names.get(index).cloned().unwrap_or_else(|| format!("Input {}", index + 1)),
                device: layout.interfaces[device].name.clone(),
                device_index: device,
                channel: pick.channel,
            });
            wanted.push(Wanted { is_input: true, channel: index as i32 });
        }

        let rate = aggregate.rate();
        if !(rate.is_finite() && rate > 0.0) {
            return Err("these interfaces did not say what rate they are running at".into());
        }
        let block = plan.preferred.max(1) as usize;
        let sizing = sizing::size(memory.available(), request.percent, request.cap_seconds, Shape { rate, channels: channels.len(), block })?;
        let capture = Arc::new(Capture::allocate(channels.len(), block, sizing.capacity_frames, sizing.preroll_frames)?);

        let pairs = aggregate.create_buffers(&wanted, block as i32, callbacks())?;
        let tap = Box::new(Tap { inputs: pairs, block, capture: Arc::clone(&capture) });
        TAP.store(&*tap as *const Tap as *mut Tap, Ordering::Release);
        if let Err(why) = aggregate.start() {
            TAP.store(std::ptr::null_mut(), Ordering::Release);
            aggregate.dispose_buffers();
            return Err(why);
        }
        let master = aggregate.stream().map_or(0, |stream| stream.master);
        let dropouts = Arc::new(Mutex::new(aggregate.stream().map(|s| s.glitches()).unwrap_or_default()));
        Ok(Session {
            aggregate,
            capture,
            tap: Some(tap),
            reporter,
            noticing: Noticing::new(),
            dropouts,
            opened: Opened { rate, block, channels, sizing, master },
            closed: false,
            _turn: turn,
        })
    }

    pub fn capture(&self) -> Arc<Capture> {
        Arc::clone(&self.capture)
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

    /// Stop the stream and let go of every driver. Any take is ended where the audio ended; the
    /// writer finishes it from the ring, which outlives the session.
    pub fn close(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        self.noticing.look(&self.reporter);
        self.housekeeping();
        self.aggregate.stop();
        self.capture.close_after_stream_stopped();
        TAP.store(std::ptr::null_mut(), Ordering::Release);
        self.aggregate.dispose_buffers();
        self.tap = None;
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.close();
    }
}

/// Why a pick is not an input of this aggregate, in words.
fn unplaced(layout: &Layout, pick: Pick) -> String {
    let Some(interface) = usize::try_from(pick.device).ok().and_then(|at| layout.interfaces.get(at)) else {
        return format!(
            "the preset records from interface {}, and the aggregate has {}: choose its channels again",
            pick.device + 1,
            layout.interfaces.len()
        );
    };
    if interface.phase_input == Some(pick.channel) {
        return format!(
            "input {} of {} is kept for the phase measurement, so the aggregate does not offer it: choose another",
            pick.channel + 1,
            interface.name
        );
    }
    format!(
        "{} has {} inputs in the aggregate, and input {} is not one of them: choose the preset's channels again",
        interface.name,
        layout.inputs.iter().filter(|(device, _)| *device as i32 == pick.device).count(),
        pick.channel + 1
    )
}
