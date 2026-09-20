//! The real PC: the ASIO registry, COM, and the vendor driver objects.
//!
//! **No Steinberg code.** The driver object is a COM object whose vtable, after `IUnknown`'s
//! three entries, holds twenty one calls in a fixed order. That order and the layout of the four
//! structures passed across it are described below and declared in Rust here, from the public
//! shape of the interface. Nothing from the ASIO SDK is included, copied or compiled, and this
//! crate builds with the SDK absent.
//!
//! **Types.** In the 64 bit Windows build of that interface every `long` is 32 bits (`i32`), a
//! sample rate is an IEEE 754 double passed by value, and a sample position is a pair of 32 bit
//! words, high first. `extern "system"` on x86_64 Windows is the same convention a C++ virtual
//! call uses, with the object pointer first.
//!
//! **What is never called.** `setSampleRate`, `setClockSource`, `controlPanel` and `future` have
//! their slots in the vtable, because the slots after them must land in the right place, and are
//! never invoked. This probe reads.
//!
//! **Silence.** The buffer callback zeroes every output buffer it was given and counts. It
//! allocates nothing, locks nothing and prints nothing: the counters are atomics, read after the
//! run. Input buffers are never read and never copied anywhere.

use std::ffi::{c_void, CStr};
use std::os::raw::c_char;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use windows_sys::core::GUID;
use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_NO_MORE_ITEMS, ERROR_PATH_NOT_FOUND, ERROR_SUCCESS};
use windows_sys::Win32::System::Com::{
    CLSIDFromString, CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegEnumKeyExW, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_64KEY,
};

use super::{AsioEntry, BufferSizes, ClockSource, Description, Host, Position, SubDriver};

/// Where every ASIO driver on a PC registers itself.
const ASIO_KEY: &str = r"SOFTWARE\ASIO";

/// How many drivers can run at once. Each needs its own set of callback functions, because the
/// interface gives a callback nothing to tell it which driver called.
const MAX_SUBS: usize = 4;
/// How many output buffers one driver's callback will zero.
const MAX_OUT: usize = 8;

// ---------------------------------------------------------------------------------------------
// The interface, declared here rather than included from anywhere.
// ---------------------------------------------------------------------------------------------

#[repr(C)]
struct ClockSourceRaw {
    index: i32,
    associated_channel: i32,
    associated_group: i32,
    is_current: i32,
    name: [c_char; 32],
}

#[repr(C)]
struct ChannelInfoRaw {
    channel: i32,
    is_input: i32,
    is_active: i32,
    channel_group: i32,
    sample_type: i32,
    name: [c_char; 32],
}

#[repr(C)]
struct BufferInfoRaw {
    is_input: i32,
    channel: i32,
    /// Filled in by the driver: the two halves of the double buffer.
    buffers: [*mut c_void; 2],
}

/// A sample position, as the 64 bit build of the interface carries it: high word first.
#[repr(C)]
struct SamplesRaw {
    hi: u32,
    lo: u32,
}

#[repr(C)]
struct CallbacksRaw {
    buffer_switch: unsafe extern "system" fn(i32, i32),
    sample_rate_did_change: unsafe extern "system" fn(f64),
    message: unsafe extern "system" fn(i32, i32, *mut c_void, *mut f64) -> i32,
    buffer_switch_time_info: unsafe extern "system" fn(*mut c_void, i32, i32) -> *mut c_void,
}

/// `IUnknown`'s three entries, then the interface's twenty one, in order.
#[repr(C)]
struct Vtable {
    query_interface: unsafe extern "system" fn(*mut Object, *const GUID, *mut *mut c_void) -> i32,
    add_ref: unsafe extern "system" fn(*mut Object) -> u32,
    release: unsafe extern "system" fn(*mut Object) -> u32,
    init: unsafe extern "system" fn(*mut Object, *mut c_void) -> i32,
    get_driver_name: unsafe extern "system" fn(*mut Object, *mut c_char),
    get_driver_version: unsafe extern "system" fn(*mut Object) -> i32,
    get_error_message: unsafe extern "system" fn(*mut Object, *mut c_char),
    start: unsafe extern "system" fn(*mut Object) -> i32,
    stop: unsafe extern "system" fn(*mut Object) -> i32,
    get_channels: unsafe extern "system" fn(*mut Object, *mut i32, *mut i32) -> i32,
    get_latencies: unsafe extern "system" fn(*mut Object, *mut i32, *mut i32) -> i32,
    get_buffer_size: unsafe extern "system" fn(*mut Object, *mut i32, *mut i32, *mut i32, *mut i32) -> i32,
    can_sample_rate: unsafe extern "system" fn(*mut Object, f64) -> i32,
    get_sample_rate: unsafe extern "system" fn(*mut Object, *mut f64) -> i32,
    /// Never called.
    set_sample_rate: unsafe extern "system" fn(*mut Object, f64) -> i32,
    get_clock_sources: unsafe extern "system" fn(*mut Object, *mut ClockSourceRaw, *mut i32) -> i32,
    /// Never called.
    set_clock_source: unsafe extern "system" fn(*mut Object, i32) -> i32,
    get_sample_position: unsafe extern "system" fn(*mut Object, *mut SamplesRaw, *mut SamplesRaw) -> i32,
    get_channel_info: unsafe extern "system" fn(*mut Object, *mut ChannelInfoRaw) -> i32,
    create_buffers: unsafe extern "system" fn(*mut Object, *mut BufferInfoRaw, i32, i32, *const CallbacksRaw) -> i32,
    dispose_buffers: unsafe extern "system" fn(*mut Object) -> i32,
    /// Never called: it opens the vendor's own window.
    control_panel: unsafe extern "system" fn(*mut Object) -> i32,
    /// Never called.
    future: unsafe extern "system" fn(*mut Object, i32, *mut c_void) -> i32,
    output_ready: unsafe extern "system" fn(*mut Object) -> i32,
}

#[repr(C)]
struct Object {
    vtable: *const Vtable,
}

/// The call answered yes. Everything else is an error whose text comes from `getErrorMessage`.
const OK: i32 = 0;

/// The sample types this probe knows how to keep silent, with their width in bytes. A driver that
/// answers anything else is reported and not opened, rather than written to blindly.
const SAMPLE_TYPES: &[(i32, &str, usize)] = &[
    (0, "Int16 big endian", 2),
    (1, "Int24 big endian", 3),
    (2, "Int32 big endian", 4),
    (3, "Float32 big endian", 4),
    (4, "Float64 big endian", 8),
    (8, "Int32 big endian, 16 bits used", 4),
    (9, "Int32 big endian, 18 bits used", 4),
    (10, "Int32 big endian, 20 bits used", 4),
    (11, "Int32 big endian, 24 bits used", 4),
    (16, "Int16 little endian", 2),
    (17, "Int24 little endian", 3),
    (18, "Int32 little endian", 4),
    (19, "Float32 little endian", 4),
    (20, "Float64 little endian", 8),
    (24, "Int32 little endian, 16 bits used", 4),
    (25, "Int32 little endian, 18 bits used", 4),
    (26, "Int32 little endian, 20 bits used", 4),
    (27, "Int32 little endian, 24 bits used", 4),
];

fn sample_type(code: i32) -> Option<(&'static str, usize)> {
    SAMPLE_TYPES.iter().find(|(c, _, _)| *c == code).map(|(_, name, bytes)| (*name, *bytes))
}

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

/// The driver saying its rate moved. Nothing to do: the probe never sets a rate, and the report
/// is about callback counts.
unsafe extern "system" fn sample_rate_did_change<const N: usize>(_rate: f64) {}

/// Selector 1 asks whether a selector is supported; selector 2 asks the host's engine version.
/// Everything else, including a reset request, is declined: this probe runs for a few seconds and
/// then reports, and answering a reset would mean calling the driver back from its own thread.
unsafe extern "system" fn message<const N: usize>(selector: i32, value: i32, _message: *mut c_void, _opt: *mut f64) -> i32 {
    const SUPPORTED: i32 = 1;
    const ENGINE_VERSION: i32 = 2;
    match selector {
        SUPPORTED => i32::from(value == SUPPORTED || value == ENGINE_VERSION),
        ENGINE_VERSION => 2,
        _ => 0,
    }
}

/// Offered so the structure is complete. The host declines the time info selector above, so a
/// driver calls `buffer_switch` instead, but a driver that calls this anyway still gets silence.
unsafe extern "system" fn buffer_switch_time_info<const N: usize>(_params: *mut c_void, index: i32, direct: i32) -> *mut c_void {
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

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn from_wide(chars: &[u16]) -> String {
    let len = chars.iter().position(|&c| c == 0).unwrap_or(chars.len());
    String::from_utf16_lossy(&chars[..len])
}

fn open_key(root: HKEY, subkey: &str) -> Result<OwnedKey, String> {
    let mut key: HKEY = null_mut();
    let status = unsafe { RegOpenKeyExW(root, wide(subkey).as_ptr(), 0, KEY_READ | KEY_WOW64_64KEY, &mut key) };
    if status == ERROR_SUCCESS {
        Ok(OwnedKey(key))
    } else {
        Err(reg_error(subkey, status))
    }
}

fn reg_error(what: &str, status: u32) -> String {
    match status {
        ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND => format!("{what} is not there"),
        other => format!("{what}: {}", std::io::Error::from_raw_os_error(other as i32)),
    }
}

struct OwnedKey(HKEY);

impl Drop for OwnedKey {
    fn drop(&mut self) {
        unsafe { RegCloseKey(self.0) };
    }
}

/// A string value, or the default value when `name` is empty.
fn value(key: &OwnedKey, name: &str) -> Result<String, String> {
    let wide_name = wide(name);
    // A null name asks for the key's default value, which is where a class id keeps its DLL.
    let name = if name.is_empty() { null() } else { wide_name.as_ptr() };
    let mut size = 0u32;
    let status = unsafe { RegQueryValueExW(key.0, name, null(), null_mut(), null_mut(), &mut size) };
    if status != ERROR_SUCCESS && status != ERROR_MORE_DATA {
        return Err(reg_error("the value", status));
    }
    let mut buffer = vec![0u16; (size as usize / 2) + 2];
    let mut size = (buffer.len() * 2) as u32;
    let status = unsafe { RegQueryValueExW(key.0, name, null(), null_mut(), buffer.as_mut_ptr().cast(), &mut size) };
    if status != ERROR_SUCCESS {
        return Err(reg_error("the value", status));
    }
    Ok(from_wide(&buffer).trim().to_string())
}

fn subkeys(key: &OwnedKey) -> Vec<String> {
    let mut names = Vec::new();
    let mut index = 0u32;
    loop {
        let mut name = [0u16; 512];
        let mut len = name.len() as u32;
        let status =
            unsafe { RegEnumKeyExW(key.0, index, name.as_mut_ptr(), &mut len, null_mut(), null_mut(), null_mut(), null_mut()) };
        if status == ERROR_NO_MORE_ITEMS {
            break;
        }
        if status != ERROR_SUCCESS {
            break;
        }
        names.push(from_wide(&name));
        index += 1;
    }
    names
}

fn dll_for(clsid: &str) -> Result<String, String> {
    let subkey = format!(r"SOFTWARE\Classes\CLSID\{clsid}\InprocServer32");
    let key = open_key(HKEY_LOCAL_MACHINE, &subkey)?;
    let dll = value(&key, "")?;
    if dll.is_empty() {
        Err("its InprocServer32 names no DLL".to_string())
    } else {
        Ok(dll)
    }
}

/// Every ASIO entry this PC registers, read only.
pub fn entries() -> Result<Vec<AsioEntry>, String> {
    let root = match open_key(HKEY_LOCAL_MACHINE, ASIO_KEY) {
        Ok(key) => key,
        Err(_) => return Ok(Vec::new()),
    };
    let mut entries = Vec::new();
    for name in subkeys(&root) {
        let key = match open_key(HKEY_LOCAL_MACHINE, &format!(r"{ASIO_KEY}\{name}")) {
            Ok(key) => key,
            Err(_) => continue,
        };
        let Ok(clsid) = value(&key, "CLSID") else { continue };
        if clsid.is_empty() {
            continue;
        }
        let dll = dll_for(&clsid);
        entries.push(AsioEntry { key: name, description: value(&key, "Description").ok().filter(|d| !d.is_empty()), clsid, dll });
    }
    Ok(entries)
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
    let hr = unsafe { CLSIDFromString(wide(text).as_ptr(), &mut guid) };
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
}

impl SubDriver for WindowsSub {
    fn init(&mut self) -> Result<(), String> {
        // The interface takes the host's main window here. This probe has no window, and both
        // drivers accept nothing.
        let ok = unsafe { ((*(*self.object).vtable).init)(self.object, null_mut()) };
        if ok == 0 {
            return Err(self.error("init"));
        }
        self.initialised = true;
        Ok(())
    }

    fn describe(&mut self) -> Result<Description, String> {
        let vtable = unsafe { (*self.object).vtable };
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
            let mut info = ChannelInfoRaw {
                channel: 0,
                is_input: i32::from(is_input),
                is_active: 0,
                channel_group: 0,
                sample_type: 0,
                name: [0; 32],
            };
            let code = unsafe { ((*vtable).get_channel_info)(self.object, &mut info) };
            if code != OK {
                return Err(self.error("getChannelInfo"));
            }
            match sample_type(info.sample_type) {
                Some((name, bytes)) => Ok((format!("{name} (type {})", info.sample_type), bytes)),
                None => Ok((format!("sample type {}, which this probe does not know", info.sample_type), 0)),
            }
        };
        let (input_format, _) = format(true)?;
        let (output_format, output_bytes) = format(false)?;

        let mut raw: Vec<ClockSourceRaw> = (0..32)
            .map(|_| ClockSourceRaw { index: 0, associated_channel: 0, associated_group: 0, is_current: 0, name: [0; 32] })
            .collect();
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
        unsafe { ((*(*self.object).vtable).can_sample_rate)(self.object, hz) == OK }
    }

    fn set_rate(&mut self, hz: f64) -> Result<(), String> {
        // The only call in this file that changes anything on the PC, and only with --set-rate.
        let code = unsafe { ((*(*self.object).vtable).set_sample_rate)(self.object, hz) };
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
            ((*(*self.object).vtable).create_buffers)(self.object, self.infos.as_mut_ptr(), total, size, &CALLBACKS[self.slot])
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
        let code = unsafe { ((*(*self.object).vtable).start)(self.object) };
        self.check("start", code)?;
        self.started = true;
        Ok(())
    }

    fn stop(&mut self) {
        if self.started {
            unsafe { ((*(*self.object).vtable).stop)(self.object) };
            self.started = false;
        }
    }

    fn dispose_buffers(&mut self) {
        if self.created {
            // The callback stops touching the buffers before the driver frees them.
            SLOTS[self.slot].clear_buffers();
            unsafe { ((*(*self.object).vtable).dispose_buffers)(self.object) };
            self.created = false;
            self.infos.clear();
        }
    }

    fn callbacks(&self) -> u64 {
        SLOTS[self.slot].count.load(Ordering::Relaxed)
    }

    fn position(&mut self) -> Option<Position> {
        // Both halves are 32 bits, high word first, as the interface carries a 64 bit count on a
        // platform whose compiler may have none. The timestamp is system time in nanoseconds.
        let mut samples = SamplesRaw { hi: 0, lo: 0 };
        let mut stamp = SamplesRaw { hi: 0, lo: 0 };
        let code = unsafe { ((*(*self.object).vtable).get_sample_position)(self.object, &mut samples, &mut stamp) };
        if code != OK {
            return None;
        }
        let join = |v: &SamplesRaw| ((v.hi as u64) << 32 | v.lo as u64) as i64;
        Some(Position { samples: join(&samples), nanos: join(&stamp) })
    }
}

/// The output sample width this driver reported, asked again at buffer time so the callback never
/// zeroes more than one buffer holds.
fn describe_width(sub: &mut WindowsSub) -> Option<usize> {
    let vtable = unsafe { (*sub.object).vtable };
    let mut info =
        ChannelInfoRaw { channel: 0, is_input: 0, is_active: 0, channel_group: 0, sample_type: 0, name: [0; 32] };
    let code = unsafe { ((*vtable).get_channel_info)(sub.object, &mut info) };
    if code != OK {
        return None;
    }
    sample_type(info.sample_type).map(|(_, bytes)| bytes)
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
