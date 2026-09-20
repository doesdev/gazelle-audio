//! A DAW made of data, for the tests.
//!
//! The interface gives a host's callbacks nothing to say which driver called them, so a host's
//! callbacks have to be plain functions over something global. That is true of a real DAW too.
//! Everything here is therefore one session at a time, and a test takes [`session`] first.

use std::ffi::c_void;
use std::sync::{Mutex, MutexGuard};

use gazelle_audio_stream_abi::raw::{selector, CallbacksRaw, Samples, Time, ENGINE_VERSION_2};
use gazelle_audio_stream_abi::sample;

/// One block, as the DAW saw or made it: one run of samples per channel.
pub type Block = Vec<Vec<i32>>;

#[derive(Default)]
pub struct Session {
    pub block: usize,
    inputs: Vec<[*mut c_void; 2]>,
    outputs: Vec<[*mut c_void; 2]>,
    /// Every block of input the DAW was given, in order.
    pub heard: Vec<Block>,
    /// Every block of output the DAW wrote, in order.
    pub played: Vec<Block>,
    /// What the DAW writes next: this value on every output, then one more.
    pub next: i32,
    /// True when the DAW writes nothing at all, which a driver must play as silence.
    pub silent: bool,
    pub calls: u64,
    pub messages: Vec<(i32, i32)>,
    /// Whether this DAW asks for the callback that carries the time.
    pub wants_time: bool,
    /// The sample position and system time the driver reported with each block.
    pub times: Vec<(i64, i64)>,
    pub rate_changes: Vec<f64>,
    /// What the DAW answers a reset request with.
    pub accepts_reset: bool,
}

/// Pointers into the driver's own buffers. One session at a time, and only the thread holding the
/// session lock touches them.
unsafe impl Send for Session {}

static SESSION: Mutex<Session> = Mutex::new(Session {
    block: 0,
    inputs: Vec::new(),
    outputs: Vec::new(),
    heard: Vec::new(),
    played: Vec::new(),
    next: 0,
    silent: false,
    calls: 0,
    messages: Vec::new(),
    wants_time: false,
    times: Vec::new(),
    rate_changes: Vec::new(),
    accepts_reset: true,
});

/// Whose turn it is. The callbacks reach the session itself, so this is a separate lock: a test
/// holds this one for its whole run and the callbacks take the other one for a moment at a time.
static ORDER: Mutex<()> = Mutex::new(());

/// One test at a time, because the callbacks are global. A test that panicked leaves a lock
/// poisoned, which says nothing about the next test, so both are taken anyway.
pub fn session() -> MutexGuard<'static, ()> {
    let order = ORDER.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    *SESSION.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) =
        Session { accepts_reset: true, ..Session::default() };
    order
}

/// Look at, or change, what the DAW is doing. Held only for as long as the closure runs, so a
/// callback can take it in between.
pub fn with<R>(what: impl FnOnce(&mut Session) -> R) -> R {
    let mut session = SESSION.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    what(&mut session)
}

impl Session {
    /// Tell the DAW where its buffers are, once the driver has made them.
    pub fn buffers(&mut self, block: usize, inputs: Vec<[*mut c_void; 2]>, outputs: Vec<[*mut c_void; 2]>) {
        self.block = block;
        self.inputs = inputs;
        self.outputs = outputs;
    }

    /// The last block of input the DAW was given.
    pub fn last_heard(&self) -> Option<&Block> {
        self.heard.last()
    }
}

/// The four functions the driver is handed.
pub fn callbacks() -> CallbacksRaw {
    CallbacksRaw {
        buffer_switch,
        sample_rate_did_change,
        message,
        buffer_switch_time_info,
    }
}

/// What the DAW does with a block: read every input, write a value to every output.
fn work(half: usize) {
    let mut session = SESSION.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let block = session.block;
    let mut heard = Vec::new();
    for channel in 0..session.inputs.len() {
        let pointer = session.inputs[channel][half & 1] as *const i32;
        let run = unsafe { std::slice::from_raw_parts(pointer, block) };
        heard.push(run.to_vec());
    }
    session.heard.push(heard);

    let value = session.next;
    session.next += 1;
    let mut played = Vec::new();
    for channel in 0..session.outputs.len() {
        let pointer = session.outputs[channel][half & 1] as *mut i32;
        let run = unsafe { std::slice::from_raw_parts_mut(pointer, block) };
        if !session.silent {
            // A different value per channel as well as per block, so a test can tell a channel
            // that went to the wrong device from one that did not.
            for (index, at) in run.iter_mut().enumerate() {
                *at = value * 1000 + channel as i32 * 10 + index as i32;
            }
        }
        played.push(run.to_vec());
    }
    session.played.push(played);
    session.calls += 1;
}

unsafe extern "system" fn buffer_switch(index: i32, _direct: i32) {
    work((index as usize) & 1);
}

unsafe extern "system" fn sample_rate_did_change(hz: f64) {
    SESSION.lock().unwrap_or_else(|p| p.into_inner()).rate_changes.push(hz);
}

unsafe extern "system" fn message(which: i32, value: i32, _message: *mut c_void, _opt: *mut f64) -> i32 {
    let mut session = SESSION.lock().unwrap_or_else(|p| p.into_inner());
    session.messages.push((which, value));
    match which {
        selector::SUPPORTED => i32::from(matches!(
            value,
            selector::SUPPORTED
                | selector::ENGINE_VERSION
                | selector::RESET_REQUEST
                | selector::LATENCIES_CHANGED
                | selector::BUFFER_SIZE_CHANGE
                | selector::RESYNC_REQUEST
                | selector::OVERLOAD
        )) | i32::from(value == selector::SUPPORTS_TIME_INFO && session.wants_time),
        selector::ENGINE_VERSION => ENGINE_VERSION_2,
        selector::SUPPORTS_TIME_INFO => i32::from(session.wants_time),
        selector::RESET_REQUEST => i32::from(session.accepts_reset),
        _ => 1,
    }
}

unsafe extern "system" fn buffer_switch_time_info(time: *mut Time, index: i32, _direct: i32) -> *mut Time {
    if !time.is_null() {
        let carried = unsafe { &*time };
        let position = Samples::to_i64(carried.info.sample_position);
        let nanos = Samples::to_i64(carried.info.system_time);
        SESSION.lock().unwrap_or_else(|p| p.into_inner()).times.push((position, nanos));
    }
    work((index as usize) & 1);
    std::ptr::null_mut()
}

/// What a device's input buffer holds, for a test that fills one by hand.
pub fn tone(block: usize, from: i32) -> Vec<i32> {
    (0..block as i32).map(|index| from + index).collect()
}

/// The width of one sample of this type, for a test that lays out a buffer itself.
pub fn width(code: i32) -> usize {
    sample::width(code).unwrap_or(4)
}
