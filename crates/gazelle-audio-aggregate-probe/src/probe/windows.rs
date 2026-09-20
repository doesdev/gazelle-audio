//! The real PC: the registry, COM, and the vendor driver objects.
//!
//! **No Steinberg code.** The driver object's vtable and the structures passed across it are
//! declared in `gazelle-audio-stream-abi`, in Rust, from the interface's public shape. Nothing
//! from the SDK is included, copied or compiled, and this crate builds with the SDK absent. That
//! declaration used to live in this file; it moved so that the aggregate driver, which implements
//! the same interface rather than calling it, works from the same one.
//!
//! **What is never called.** `setClockSource`, `controlPanel` and `future` have their slots in the
//! vtable, because the slots after them must land in the right place, and are never invoked.
//! `setSampleRate` is called only with `--set-rate`. Otherwise this probe reads.
//!
//! **Silence.** The buffer callback zeroes every output buffer it was given and counts. It
//! allocates nothing, locks nothing and prints nothing: the counters are atomics, read after the
//! run. Input buffers are never read and never copied anywhere.

use std::ffi::{c_void, CStr};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use gazelle_audio_stream_abi::raw::{
    BufferInfoRaw, CallbacksRaw, ChannelInfoRaw, ClockSourceRaw, Object, Samples, Time, Vtable, OK,
};
use gazelle_audio_stream_abi::raw::selector;
use gazelle_audio_stream_abi::{registry, sample};
use windows_sys::core::GUID;
use windows_sys::Win32::System::Com::{
    CLSIDFromString, CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};

use super::{AsioEntry, BufferSizes, ClockSource, Description, Host, Position, SubDriver};

/// How many drivers can run at once. Each needs its own set of callback functions, because the
/// interface gives a callback nothing to tell it which driver called.
const MAX_SUBS: usize = 4;
/// How many output buffers one driver's callback will zero.
const MAX_OUT: usize = 8;

// ---------------------------------------------------------------------------------------------
// The callbacks: one set per driver, counting and zeroing only.
// ---------------------------------------------------------------------------------------------

struct Slot {
    taken: AtomicBool,
    count: AtomicU64,
    /// How many of `out` hold a buffer pair.
    out_used: AtomicUsize,
    /// Bytes in one output buffer half.
    out_bytes: AtomicUsize,
    out: [[AtomicPtr<u8>; 2]; MAX_OUT],
}

impl Slot {
    const fn new() -> Slot {
        Slot {
            taken: AtomicBool::new(false),
            count: AtomicU64::new(0),
            out_used: AtomicUsize::new(0),
            out_bytes: AtomicUsize::new(0),
            out: [const { [const { AtomicPtr::new(null_mut()) }; 2] }; MAX_OUT],
        }
    }

    fn clear_buffers(&self) {
        self.out_used.store(0, Ordering::Release);
        self.out_bytes.store(0, Ordering::Release);
        for pair in &self.out {
            for half in pair {
                half.store(null_mut(), Ordering::Release);
            }
        }
    }
}

static SLOTS: [Slot; MAX_SUBS] = [const { Slot::new() }; MAX_SUBS];

/// The one thing that happens on the driver's own thread: zero what we were given, and count.
/// No allocation, no lock, no formatting, no call back into the driver.
///
/// # Safety
/// Called by the driver with a double buffer index it owns. The pointers were handed to us by
/// `createBuffers` and are cleared before `disposeBuffers` returns.
unsafe extern "system" fn buffer_switch<const N: usize>(index: i32, _direct: i32) {
    let slot = &SLOTS[N];
    let half = (index as usize) & 1;
    let bytes = slot.out_bytes.load(Ordering::Acquire);
    let used = slot.out_used.load(Ordering::Acquire).min(MAX_OUT);
    if bytes > 0 {
        for channel in 0..used {
            let buffer = slot.out[channel][half].load(Ordering::Acquire);
            if !buffer.is_null() {
                unsafe { std::ptr::write_bytes(buffer, 0, bytes) };
            }
        }
    }
    slot.count.fetch_add(1, Ordering::Relaxed);
}

/// The driver saying its rate moved. Nothing to do: the report is about callback counts.
unsafe extern "system" fn sample_rate_did_change<const N: usize>(_rate: f64) {}

/// Everything except the engine version is declined: this probe runs for a few seconds and then
/// reports, and answering a reset would mean calling the driver back from its own thread.
unsafe extern "system" fn message<const N: usize>(which: i32, value: i32, _message: *mut c_void, _opt: *mut f64) -> i32 {
    match which {
        selector::SUPPORTED => i32::from(value == selector::SUPPORTED || value == selector::ENGINE_VERSION),
        selector::ENGINE_VERSION => gazelle_audio_stream_abi::raw::ENGINE_VERSION_2,
        _ => 0,
    }
}

/// Offered so the structure is complete. The host declines the time info selector above, so a
/// driver calls `buffer_switch` instead, but a driver that calls this anyway still gets silence.
unsafe extern "system" fn buffer_switch_time_info<const N: usize>(_params: *mut Time, index: i32, direct: i32) -> *mut Time {
    unsafe { buffer_switch::<N>(index, direct) };
    null_mut()
}

const fn callbacks_for<const N: usize>() -> CallbacksRaw {
    CallbacksRaw {
        buffer_switch: buffer_switch::<N>,
        sample_rate_did_change: sample_rate_did_change::<N>,
        message: message::<N>,
        buffer_switch_time_info: buffer_switch_time_info::<N>,
    }
}

/// Kept for the whole life of the process, so a driver that holds the pointer past
/// `disposeBuffers` still points at something valid.
static CALLBACKS: [CallbacksRaw; MAX_SUBS] =
    [callbacks_for::<0>(), callbacks_for::<1>(), callbacks_for::<2>(), callbacks_for::<3>()];

// ---------------------------------------------------------------------------------------------
// The registry.
// ---------------------------------------------------------------------------------------------

/// Every ASIO entry this PC registers, read only.
pub fn entries() -> Result<Vec<AsioEntry>, String> {
    Ok(registry::entries()?
        .into_iter()
        .map(|entry| AsioEntry { key: entry.key, description: entry.description, clsid: entry.clsid, dll: entry.dll })
        .collect())
}

// ---------------------------------------------------------------------------------------------
// The PC.
// ---------------------------------------------------------------------------------------------

/// This PC, with COM started on the calling thread for as long as it lives.
///
/// Both drivers register `ThreadingModel` `Apartment`, so the apartment is a single threaded one
/// and every call into both drivers happens on the thread that made this.
pub struct ThisPc {
    initialised: bool,
    next_slot: std::cell::Cell<usize>,
}

impl ThisPc {
    pub fn new() -> Result<ThisPc, String> {
        let hr = unsafe { CoInitializeEx(null(), COINIT_APARTMENTTHREADED as u32) };
        // S_OK or S_FALSE (already initialised this way) are both fine.
        if hr < 0 {
            return Err(format!("CoInitializeEx answered {hr:#010x}"));
        }
        Ok(ThisPc { initialised: true, next_slot: std::cell::Cell::new(0) })
    }
}

impl Drop for ThisPc {
    fn drop(&mut self) {
        if self.initialised {
            unsafe { CoUninitialize() };
        }
    }
}

fn parse_clsid(text: &str) -> Result<GUID, String> {
    let mut guid = GUID { data1: 0, data2: 0, data3: 0, data4: [0; 8] };
    let hr = unsafe { CLSIDFromString(registry::wide(text).as_ptr(), &mut guid) };
    if hr < 0 {
        Err(format!("{text} is not a class id ({hr:#010x})"))
    } else {
        Ok(guid)
    }
}

impl Host for ThisPc {
    fn entries(&self) -> Result<Vec<AsioEntry>, String> {
        entries()
    }

    fn open(&self, entry: &AsioEntry) -> Result<Box<dyn SubDriver>, String> {
        let slot = self.next_slot.get();
        if slot >= MAX_SUBS {
            return Err(format!("this probe opens at most {MAX_SUBS} drivers at once"));
        }
        let clsid = parse_clsid(&entry.clsid)?;
        let mut ptr: *mut c_void = null_mut();
        // The convention this interface is published under uses the class id as the interface id.
        let hr = unsafe { CoCreateInstance(&clsid, null_mut(), CLSCTX_INPROC_SERVER, &clsid, &mut ptr) };
        if hr < 0 || ptr.is_null() {
            return Err(format!("CoCreateInstance answered {hr:#010x}"));
        }
        if SLOTS[slot].taken.swap(true, Ordering::AcqRel) {
            unsafe { release(ptr.cast()) };
            return Err(format!("callback slot {slot} is still in use"));
        }
        SLOTS[slot].count.store(0, Ordering::Release);
        SLOTS[slot].clear_buffers();
        self.next_slot.set(slot + 1);
        Ok(Box::new(WindowsSub {
            object: ptr.cast(),
            slot,
            initialised: false,
            created: false,
            started: false,
            infos: Vec::new(),
        }))
    }

    fn wait_ms(&self, ms: u64) -> Duration {
        let started = Instant::now();
        std::thread::sleep(Duration::from_millis(ms));
        started.elapsed()
    }
}

unsafe fn release(object: *mut Object) {
    unsafe { ((*(*object).vtable).release)(object) };
}

/// One vendor driver object.
pub struct WindowsSub {
    object: *mut Object,
    slot: usize,
    initialised: bool,
    created: bool,
    started: bool,
    /// Kept alive for as long as the buffers are: the driver was handed this array.
    infos: Vec<BufferInfoRaw>,
}

impl WindowsSub {
    /// The driver's own words for what went wrong, or a plain note when it has none.
    fn error(&mut self, call: &str) -> String {
        let mut text = [0i8; 512];
        unsafe { ((*(*self.object).vtable).get_error_message)(self.object, text.as_mut_ptr()) };
        let said = unsafe { CStr::from_ptr(text.as_ptr()) }.to_string_lossy().trim().to_string();
        if said.is_empty() {
            format!("{call} failed and the driver gave no message")
        } else {
            format!("{call} failed: {said}")
        }
    }

    fn check(&mut self, call: &str, code: i32) -> Result<(), String> {
        if code == OK {
            Ok(())
        } else {
            Err(format!("{} (code {code})", self.error(call)))
        }
    }

    fn vtable(&self) -> *const Vtable {
        unsafe { (*self.object).vtable }
    }
}

impl SubDriver for WindowsSub {
    fn init(&mut self) -> Result<(), String> {
        // The interface takes the host's main window here. This probe has no window, and both
        // drivers accept nothing.
        let ok = unsafe { ((*self.vtable()).init)(self.object, null_mut()) };
        if ok == 0 {
            return Err(self.error("init"));
        }
        self.initialised = true;
        Ok(())
    }

    fn describe(&mut self) -> Result<Description, String> {
        let vtable = self.vtable();
        let mut name = [0i8; 256];
        unsafe { ((*vtable).get_driver_name)(self.object, name.as_mut_ptr()) };
        let name = unsafe { CStr::from_ptr(name.as_ptr()) }.to_string_lossy().trim().to_string();
        let version = unsafe { ((*vtable).get_driver_version)(self.object) };

        let (mut inputs, mut outputs) = (0i32, 0i32);
        let code = unsafe { ((*vtable).get_channels)(self.object, &mut inputs, &mut outputs) };
        self.check("getChannels", code)?;

        let (mut min, mut max, mut preferred, mut granularity) = (0i32, 0i32, 0i32, 0i32);
        let code = unsafe { ((*vtable).get_buffer_size)(self.object, &mut min, &mut max, &mut preferred, &mut granularity) };
        self.check("getBufferSize", code)?;

        let mut rate = 0f64;
        let code = unsafe { ((*vtable).get_sample_rate)(self.object, &mut rate) };
        self.check("getSampleRate", code)?;

        let (mut latency_in, mut latency_out) = (0i32, 0i32);
        let code = unsafe { ((*vtable).get_latencies)(self.object, &mut latency_in, &mut latency_out) };
        self.check("getLatencies", code)?;

        let mut format = |is_input: bool| -> Result<(String, usize), String> {
            let mut info = ChannelInfoRaw { is_input: i32::from(is_input), ..ChannelInfoRaw::default() };
            let code = unsafe { ((*vtable).get_channel_info)(self.object, &mut info) };
            if code != OK {
                return Err(self.error("getChannelInfo"));
            }
            Ok((sample::name(info.sample_type), sample::width(info.sample_type).unwrap_or(0)))
        };
        let (input_format, _) = format(true)?;
        let (output_format, output_bytes) = format(false)?;

        let mut raw: Vec<ClockSourceRaw> = (0..32).map(|_| ClockSourceRaw::default()).collect();
        let mut count = raw.len() as i32;
        let code = unsafe { ((*vtable).get_clock_sources)(self.object, raw.as_mut_ptr(), &mut count) };
        self.check("getClockSources", code)?;
        let clocks = raw
            .iter()
            .take(count.clamp(0, raw.len() as i32) as usize)
            .map(|source| ClockSource {
                index: source.index,
                name: unsafe { CStr::from_ptr(source.name.as_ptr()) }.to_string_lossy().trim().to_string(),
                current: source.is_current != 0,
            })
            .collect();

        Ok(Description {
            name,
            version,
            inputs,
            outputs,
            buffers: BufferSizes { min, max, preferred, granularity },
            rate,
            latency_in,
            latency_out,
            input_format,
            output_format,
            output_bytes,
            clocks,
        })
    }

    fn can_rate(&mut self, hz: f64) -> bool {
        unsafe { ((*self.vtable()).can_sample_rate)(self.object, hz) == OK }
    }

    fn set_rate(&mut self, hz: f64) -> Result<(), String> {
        // The only call in this file that changes anything on the PC, and only with --set-rate.
        let code = unsafe { ((*self.vtable()).set_sample_rate)(self.object, hz) };
        self.check("setSampleRate", code)
    }

    fn create_buffers(&mut self, inputs: i32, outputs: i32, size: i32) -> Result<(), String> {
        let slot = &SLOTS[self.slot];
        if outputs as usize > MAX_OUT {
            return Err(format!("this probe opens at most {MAX_OUT} outputs"));
        }
        self.infos = (0..inputs)
            .map(|channel| BufferInfoRaw { is_input: 1, channel, buffers: [null_mut(); 2] })
            .chain((0..outputs).map(|channel| BufferInfoRaw { is_input: 0, channel, buffers: [null_mut(); 2] }))
            .collect();
        let total = self.infos.len() as i32;
        let code = unsafe {
            ((*self.vtable()).create_buffers)(self.object, self.infos.as_mut_ptr(), total, size, &CALLBACKS[self.slot])
        };
        self.check("createBuffers", code)?;
        self.created = true;

        // Tell the callback what to zero, before anything can call it.
        let mut bytes = 0usize;
        if outputs > 0 {
            let Some(width) = describe_width(self) else {
                return Err("the driver's output sample type is one this probe will not write to".to_string());
            };
            bytes = width * size as usize;
        }
        for (at, info) in self.infos.iter().filter(|info| info.is_input == 0).enumerate() {
            slot.out[at][0].store(info.buffers[0].cast(), Ordering::Release);
            slot.out[at][1].store(info.buffers[1].cast(), Ordering::Release);
        }
        slot.out_bytes.store(bytes, Ordering::Release);
        slot.out_used.store(outputs as usize, Ordering::Release);
        slot.count.store(0, Ordering::Release);
        Ok(())
    }

    fn start(&mut self) -> Result<(), String> {
        let code = unsafe { ((*self.vtable()).start)(self.object) };
        self.check("start", code)?;
        self.started = true;
        Ok(())
    }

    fn stop(&mut self) {
        if self.started {
            unsafe { ((*self.vtable()).stop)(self.object) };
            self.started = false;
        }
    }

    fn dispose_buffers(&mut self) {
        if self.created {
            // The callback stops touching the buffers before the driver frees them.
            SLOTS[self.slot].clear_buffers();
            unsafe { ((*self.vtable()).dispose_buffers)(self.object) };
            self.created = false;
            self.infos.clear();
        }
    }

    fn callbacks(&self) -> u64 {
        SLOTS[self.slot].count.load(Ordering::Relaxed)
    }

    fn position(&mut self) -> Option<Position> {
        // Both halves are 32 bits, high word first. The timestamp is system time in nanoseconds.
        let mut samples = Samples::default();
        let mut stamp = Samples::default();
        let code = unsafe { ((*self.vtable()).get_sample_position)(self.object, &mut samples, &mut stamp) };
        if code != OK {
            return None;
        }
        Some(Position { samples: samples.to_i64(), nanos: stamp.to_i64() })
    }
}

/// The output sample width this driver reported, asked again at buffer time so the callback never
/// zeroes more than one buffer holds.
fn describe_width(sub: &mut WindowsSub) -> Option<usize> {
    let vtable = sub.vtable();
    let mut info = ChannelInfoRaw::default();
    let code = unsafe { ((*vtable).get_channel_info)(sub.object, &mut info) };
    if code != OK {
        return None;
    }
    sample::width(info.sample_type)
}

impl Drop for WindowsSub {
    fn drop(&mut self) {
        self.stop();
        self.dispose_buffers();
        SLOTS[self.slot].clear_buffers();
        SLOTS[self.slot].taken.store(false, Ordering::Release);
        unsafe { release(self.object) };
    }
}
