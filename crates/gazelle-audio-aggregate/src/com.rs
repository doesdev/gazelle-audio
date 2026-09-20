//! The COM object a DAW opens, and the four things the DLL exports.
//!
//! This file is the shell: it turns the interface's calls into calls on [`Aggregate`] and turns
//! refusals back into the interface's own codes. Nothing here decides anything, which is why the
//! part of it that cannot be reached without a DAW is so small. The integration test drives every
//! one of these entry points in process, with fake devices underneath.

use std::ffi::c_void;
use std::os::raw::c_char;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};
use std::sync::Mutex;

use gazelle_audio_stream_abi::raw::{
    error, hresult, BufferInfoRaw, CallbacksRaw, ChannelInfoRaw, ClassFactory, ClassFactoryVtable, ClockSourceRaw, Guid,
    Object, Samples, Vtable, FALSE, IID_ICLASSFACTORY, IID_IUNKNOWN, OK, TRUE,
};

use crate::aggregate::{Aggregate, Wanted};
use crate::config::{self, Config};
use crate::sub::Host;
use crate::{CLASS_ID, DRIVER_NAME, DRIVER_VERSION};

/// How many objects of ours are alive. COM may unload the DLL only when this is zero.
static ALIVE: AtomicU32 = AtomicU32::new(0);

/// The module this code was loaded as, kept so registration can say where the DLL is.
static MODULE: AtomicIsize = AtomicIsize::new(0);

/// What a test puts in place of the PC. This is the only seam in the crate that exists for tests,
/// and nothing in the shipped path sets it.
pub struct Bench {
    pub host: Box<dyn Host>,
    pub config: Config,
    pub source: String,
}

type BenchMaker = Box<dyn Fn() -> Bench + Send + Sync>;

static BENCH: Mutex<Option<BenchMaker>> = Mutex::new(None);

/// Put a PC made of fakes behind the next object that is created. For tests only.
pub fn install_bench(maker: impl Fn() -> Bench + Send + Sync + 'static) {
    *BENCH.lock().expect("not poisoned") = Some(Box::new(maker));
}

/// Take the fakes away again.
pub fn clear_bench() {
    *BENCH.lock().expect("not poisoned") = None;
}

/// The driver object itself. The vtable pointer is first, because that is what a COM object is.
#[repr(C)]
pub struct Driver {
    object: Object,
    refs: AtomicU32,
    /// Everything outside the audio path. A DAW makes these calls from one thread; the lock is
    /// there so that a DAW which does not is still safe, and it is never taken on a callback.
    inner: Mutex<Aggregate>,
}

impl Driver {
    fn new() -> *mut Driver {
        let bench = BENCH.lock().expect("not poisoned").as_ref().map(|maker| maker());
        let aggregate = match bench {
            Some(bench) => {
                let mut aggregate = Aggregate::new(bench.host);
                aggregate.config_source = bench.source.clone();
                aggregate.pending = Some((bench.config, bench.source));
                aggregate
            }
            None => Aggregate::new(Box::new(crate::windows_host::ThisPc)),
        };
        ALIVE.fetch_add(1, Ordering::AcqRel);
        Box::into_raw(Box::new(Driver { object: Object { vtable: &VTABLE }, refs: AtomicU32::new(1), inner: Mutex::new(aggregate) }))
    }
}

/// Turn a pointer the DAW gave us back into our object.
///
/// # Safety
/// Only ever called with a pointer this file handed out.
unsafe fn driver<'a>(object: *mut Object) -> Option<&'a Driver> {
    if object.is_null() {
        None
    } else {
        Some(unsafe { &*(object as *mut Driver) })
    }
}

/// Copy a name into the fixed buffer the interface passes, always terminated.
///
/// # Safety
/// `into` must have room for `limit` bytes, which is what the interface promises.
unsafe fn write_c_string(into: *mut c_char, limit: usize, text: &str) {
    if into.is_null() || limit == 0 {
        return;
    }
    let bytes = text.as_bytes();
    let room = limit - 1;
    let take = bytes.len().min(room);
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr().cast::<c_char>(), into, take);
        *into.add(take) = 0;
    }
}

// ---------------------------------------------------------------------------------------------
// IUnknown.
// ---------------------------------------------------------------------------------------------

unsafe extern "system" fn query_interface(object: *mut Object, iid: *const Guid, out: *mut *mut c_void) -> i32 {
    if out.is_null() {
        return hresult::E_INVALIDARG;
    }
    unsafe { *out = null_mut() };
    let wanted = if iid.is_null() { return hresult::E_INVALIDARG } else { unsafe { *iid } };
    // The convention this interface is published under uses the class id as the interface id.
    if wanted == IID_IUNKNOWN || wanted == CLASS_ID {
        unsafe { add_ref(object) };
        unsafe { *out = object.cast() };
        hresult::S_OK
    } else {
        hresult::E_NOINTERFACE
    }
}

unsafe extern "system" fn add_ref(object: *mut Object) -> u32 {
    match unsafe { driver(object) } {
        Some(driver) => driver.refs.fetch_add(1, Ordering::AcqRel) + 1,
        None => 0,
    }
}

unsafe extern "system" fn release(object: *mut Object) -> u32 {
    let Some(driver) = (unsafe { driver(object) }) else { return 0 };
    let left = driver.refs.fetch_sub(1, Ordering::AcqRel) - 1;
    if left == 0 {
        // Everything is stopped and disposed of by the aggregate's own drop.
        drop(unsafe { Box::from_raw(object as *mut Driver) });
        ALIVE.fetch_sub(1, Ordering::AcqRel);
    }
    left
}

// ---------------------------------------------------------------------------------------------
// The driver.
// ---------------------------------------------------------------------------------------------

unsafe extern "system" fn init(object: *mut Object, _window: *mut c_void) -> i32 {
    let Some(driver) = (unsafe { driver(object) }) else { return FALSE };
    let mut aggregate = driver.inner.lock().expect("not poisoned");
    let (config, source) = match aggregate.pending.take() {
        Some(ready) => ready,
        None => match config::config_path() {
            Some(path) => match Config::read(&path) {
                Ok(config) => (config, path.display().to_string()),
                Err(why) => {
                    aggregate.refuse(why);
                    return FALSE;
                }
            },
            None => (Config::default(), "the defaults, because APPDATA is not set".to_string()),
        },
    };
    match aggregate.init(config, source) {
        Ok(()) => TRUE,
        Err(_) => FALSE,
    }
}

unsafe extern "system" fn get_driver_name(_object: *mut Object, into: *mut c_char) {
    // The interface carries 32 bytes here.
    unsafe { write_c_string(into, 32, DRIVER_NAME) };
}

unsafe extern "system" fn get_driver_version(_object: *mut Object) -> i32 {
    DRIVER_VERSION
}

unsafe extern "system" fn get_error_message(object: *mut Object, into: *mut c_char) {
    let text = match unsafe { driver(object) } {
        Some(driver) => driver.inner.lock().expect("not poisoned").last_error().to_string(),
        None => String::new(),
    };
    // The interface carries 124 bytes here, and a DAW puts them in front of a person, so a long
    // refusal is cut rather than lost.
    unsafe { write_c_string(into, 124, &text) };
}

unsafe extern "system" fn start(object: *mut Object) -> i32 {
    let Some(driver) = (unsafe { driver(object) }) else { return error::NOT_PRESENT };
    match driver.inner.lock().expect("not poisoned").start() {
        Ok(()) => OK,
        Err(_) => error::HW_MALFUNCTION,
    }
}

unsafe extern "system" fn stop(object: *mut Object) -> i32 {
    let Some(driver) = (unsafe { driver(object) }) else { return error::NOT_PRESENT };
    driver.inner.lock().expect("not poisoned").stop();
    OK
}

unsafe extern "system" fn get_channels(object: *mut Object, inputs: *mut i32, outputs: *mut i32) -> i32 {
    let Some(driver) = (unsafe { driver(object) }) else { return error::NOT_PRESENT };
    let aggregate = driver.inner.lock().expect("not poisoned");
    if !aggregate.is_initialised() {
        return error::NOT_PRESENT;
    }
    let (ins, outs) = aggregate.channels();
    unsafe {
        if !inputs.is_null() {
            *inputs = ins;
        }
        if !outputs.is_null() {
            *outputs = outs;
        }
    }
    OK
}

unsafe extern "system" fn get_latencies(object: *mut Object, input: *mut i32, output: *mut i32) -> i32 {
    let Some(driver) = (unsafe { driver(object) }) else { return error::NOT_PRESENT };
    let aggregate = driver.inner.lock().expect("not poisoned");
    let Some((ins, outs)) = aggregate.latencies() else { return error::NOT_PRESENT };
    unsafe {
        if !input.is_null() {
            *input = ins;
        }
        if !output.is_null() {
            *output = outs;
        }
    }
    OK
}

unsafe extern "system" fn get_buffer_size(
    object: *mut Object,
    min: *mut i32,
    max: *mut i32,
    preferred: *mut i32,
    granularity: *mut i32,
) -> i32 {
    let Some(driver) = (unsafe { driver(object) }) else { return error::NOT_PRESENT };
    let aggregate = driver.inner.lock().expect("not poisoned");
    let Some((lowest, highest, best, step)) = aggregate.buffer_sizes() else { return error::NOT_PRESENT };
    unsafe {
        for (out, value) in [(min, lowest), (max, highest), (preferred, best), (granularity, step)] {
            if !out.is_null() {
                *out = value;
            }
        }
    }
    OK
}

unsafe extern "system" fn can_sample_rate(object: *mut Object, hz: f64) -> i32 {
    let Some(driver) = (unsafe { driver(object) }) else { return error::NOT_PRESENT };
    if driver.inner.lock().expect("not poisoned").can_rate(hz) {
        OK
    } else {
        error::NO_CLOCK
    }
}

unsafe extern "system" fn get_sample_rate(object: *mut Object, into: *mut f64) -> i32 {
    let Some(driver) = (unsafe { driver(object) }) else { return error::NOT_PRESENT };
    let rate = driver.inner.lock().expect("not poisoned").rate();
    if into.is_null() {
        return error::INVALID_PARAMETER;
    }
    unsafe { *into = rate };
    OK
}

unsafe extern "system" fn set_sample_rate(object: *mut Object, hz: f64) -> i32 {
    let Some(driver) = (unsafe { driver(object) }) else { return error::NOT_PRESENT };
    match driver.inner.lock().expect("not poisoned").set_rate(hz) {
        Ok(()) => OK,
        Err(_) => error::NO_CLOCK,
    }
}

/// The aggregate has one clock source of its own: whatever each device is set to. Which device
/// follows which is a matter for the interfaces themselves and their cables, and a driver that
/// changed it from here would be changing the hardware behind the person's back.
unsafe extern "system" fn get_clock_sources(_object: *mut Object, into: *mut ClockSourceRaw, count: *mut i32) -> i32 {
    if into.is_null() || count.is_null() {
        return error::INVALID_PARAMETER;
    }
    if unsafe { *count } < 1 {
        return error::INVALID_PARAMETER;
    }
    let mut source = ClockSourceRaw { index: 0, associated_channel: -1, associated_group: -1, is_current: 1, name: [0; 32] };
    unsafe { write_c_string(source.name.as_mut_ptr(), 32, "Set on the devices") };
    unsafe {
        *into = source;
        *count = 1;
    }
    OK
}

unsafe extern "system" fn set_clock_source(_object: *mut Object, index: i32) -> i32 {
    if index == 0 {
        OK
    } else {
        error::INVALID_PARAMETER
    }
}

unsafe extern "system" fn get_sample_position(object: *mut Object, samples: *mut Samples, stamp: *mut Samples) -> i32 {
    let Some(driver) = (unsafe { driver(object) }) else { return error::NOT_PRESENT };
    let (position, nanos) = driver.inner.lock().expect("not poisoned").position();
    unsafe {
        if !samples.is_null() {
            *samples = Samples::from_i64(position);
        }
        if !stamp.is_null() {
            *stamp = Samples::from_i64(nanos);
        }
    }
    OK
}

unsafe extern "system" fn get_channel_info(object: *mut Object, info: *mut ChannelInfoRaw) -> i32 {
    let Some(driver) = (unsafe { driver(object) }) else { return error::NOT_PRESENT };
    if info.is_null() {
        return error::INVALID_PARAMETER;
    }
    let asked = unsafe { &mut *info };
    let aggregate = driver.inner.lock().expect("not poisoned");
    let Some(channel) = aggregate.channel_info(asked.is_input != 0, asked.channel) else {
        return error::INVALID_PARAMETER;
    };
    asked.is_active = i32::from(channel.active);
    asked.channel_group = channel.group;
    asked.sample_type = channel.sample_type;
    unsafe { write_c_string(asked.name.as_mut_ptr(), 32, &channel.name) };
    OK
}

unsafe extern "system" fn create_buffers(
    object: *mut Object,
    infos: *mut BufferInfoRaw,
    count: i32,
    block: i32,
    callbacks: *const CallbacksRaw,
) -> i32 {
    let Some(driver) = (unsafe { driver(object) }) else { return error::NOT_PRESENT };
    if infos.is_null() || callbacks.is_null() || count < 0 {
        return error::INVALID_PARAMETER;
    }
    let asked = unsafe { std::slice::from_raw_parts_mut(infos, count as usize) };
    let wanted: Vec<Wanted> =
        asked.iter().map(|info| Wanted { is_input: info.is_input != 0, channel: info.channel }).collect();
    let host = unsafe { *callbacks };
    let made = driver.inner.lock().expect("not poisoned").create_buffers(&wanted, block, host);
    match made {
        Ok(pairs) => {
            for (info, pair) in asked.iter_mut().zip(pairs) {
                info.buffers = pair;
            }
            OK
        }
        Err(_) => error::NO_MEMORY,
    }
}

unsafe extern "system" fn dispose_buffers(object: *mut Object) -> i32 {
    let Some(driver) = (unsafe { driver(object) }) else { return error::NOT_PRESENT };
    driver.inner.lock().expect("not poisoned").dispose_buffers();
    OK
}

/// There is no window to open. What this driver does is in its configuration file.
unsafe extern "system" fn control_panel(_object: *mut Object) -> i32 {
    error::NOT_PRESENT
}

unsafe extern "system" fn future(_object: *mut Object, _selector: i32, _opt: *mut c_void) -> i32 {
    error::NOT_PRESENT
}

/// Offered only when every device offers it. It is not passed on to them: calling a vendor driver
/// from the callback thread is the one thing this driver will not do.
unsafe extern "system" fn output_ready(object: *mut Object) -> i32 {
    let Some(driver) = (unsafe { driver(object) }) else { return error::NOT_PRESENT };
    if driver.inner.lock().expect("not poisoned").output_ready() {
        OK
    } else {
        error::NOT_PRESENT
    }
}

/// The driver's vtable, in the one order the interface has.
static VTABLE: Vtable = Vtable {
    query_interface,
    add_ref,
    release,
    init,
    get_driver_name,
    get_driver_version,
    get_error_message,
    start,
    stop,
    get_channels,
    get_latencies,
    get_buffer_size,
    can_sample_rate,
    get_sample_rate,
    set_sample_rate,
    get_clock_sources,
    set_clock_source,
    get_sample_position,
    get_channel_info,
    create_buffers,
    dispose_buffers,
    control_panel,
    future,
    output_ready,
};

// ---------------------------------------------------------------------------------------------
// The class factory, and the DLL's exports.
// ---------------------------------------------------------------------------------------------

unsafe extern "system" fn factory_query_interface(factory: *mut ClassFactory, iid: *const Guid, out: *mut *mut c_void) -> i32 {
    if out.is_null() || iid.is_null() {
        return hresult::E_INVALIDARG;
    }
    let wanted = unsafe { *iid };
    if wanted == IID_IUNKNOWN || wanted == IID_ICLASSFACTORY {
        unsafe { *out = factory.cast() };
        hresult::S_OK
    } else {
        unsafe { *out = null_mut() };
        hresult::E_NOINTERFACE
    }
}

/// The factory is a single static object, so counting references on it changes nothing.
unsafe extern "system" fn factory_add_ref(_factory: *mut ClassFactory) -> u32 {
    1
}

unsafe extern "system" fn factory_release(_factory: *mut ClassFactory) -> u32 {
    1
}

unsafe extern "system" fn factory_create_instance(
    _factory: *mut ClassFactory,
    outer: *mut c_void,
    iid: *const Guid,
    out: *mut *mut c_void,
) -> i32 {
    if out.is_null() {
        return hresult::E_INVALIDARG;
    }
    unsafe { *out = null_mut() };
    if !outer.is_null() {
        // This object cannot be part of another one.
        return hresult::CLASS_E_NOAGGREGATION;
    }
    let driver = Driver::new();
    if driver.is_null() {
        return hresult::E_OUTOFMEMORY;
    }
    let object = driver as *mut Object;
    let asked = unsafe { query_interface(object, iid, out) };
    // The object was made with one reference; whoever asked now holds their own.
    unsafe { release(object) };
    asked
}

unsafe extern "system" fn factory_lock_server(_factory: *mut ClassFactory, _lock: i32) -> i32 {
    hresult::S_OK
}

static FACTORY_VTABLE: ClassFactoryVtable = ClassFactoryVtable {
    query_interface: factory_query_interface,
    add_ref: factory_add_ref,
    release: factory_release,
    create_instance: factory_create_instance,
    lock_server: factory_lock_server,
};

/// One factory for the whole DLL. It holds nothing but its vtable, so sharing it between threads
/// is sharing a constant.
struct StaticFactory(ClassFactory);
unsafe impl Sync for StaticFactory {}

static FACTORY: StaticFactory = StaticFactory(ClassFactory { vtable: &FACTORY_VTABLE });

/// COM asking for the thing that makes our objects.
///
/// # Safety
/// Called by COM with pointers it owns.
#[no_mangle]
pub unsafe extern "system" fn DllGetClassObject(clsid: *const Guid, iid: *const Guid, out: *mut *mut c_void) -> i32 {
    if out.is_null() {
        return hresult::E_INVALIDARG;
    }
    unsafe { *out = null_mut() };
    if clsid.is_null() || unsafe { *clsid } != CLASS_ID {
        return hresult::CLASS_E_CLASSNOTAVAILABLE;
    }
    let factory = std::ptr::addr_of!(FACTORY.0) as *mut ClassFactory;
    unsafe { factory_query_interface(factory, iid, out) }
}

/// Whether COM may unload this DLL.
///
/// # Safety
/// Called by COM.
#[no_mangle]
pub unsafe extern "system" fn DllCanUnloadNow() -> i32 {
    if ALIVE.load(Ordering::Acquire) == 0 {
        hresult::S_OK
    } else {
        hresult::S_FALSE
    }
}

/// Where this DLL is on disk, which registration has to write down.
fn module_path() -> Result<String, String> {
    use windows_sys::Win32::System::LibraryLoader::GetModuleFileNameW;
    let module = MODULE.load(Ordering::Acquire);
    let mut buffer = [0u16; 32768];
    let written = unsafe { GetModuleFileNameW(module as _, buffer.as_mut_ptr(), buffer.len() as u32) };
    if written == 0 {
        return Err("this DLL could not say where it is".to_string());
    }
    Ok(String::from_utf16_lossy(&buffer[..written as usize]))
}

/// `regsvr32 gazelle_aggregate.dll`, from an elevated prompt.
///
/// # Safety
/// Called by `regsvr32`.
#[no_mangle]
pub unsafe extern "system" fn DllRegisterServer() -> i32 {
    let Ok(path) = module_path() else { return hresult::SELFREG_E_CLASS };
    match crate::registration::register(&mut crate::windows_host::MachineRegistry, &path) {
        Ok(()) => hresult::S_OK,
        Err(_) => hresult::SELFREG_E_CLASS,
    }
}

/// `regsvr32 /u gazelle_aggregate.dll`, from an elevated prompt.
///
/// # Safety
/// Called by `regsvr32`.
#[no_mangle]
pub unsafe extern "system" fn DllUnregisterServer() -> i32 {
    match crate::registration::unregister(&mut crate::windows_host::MachineRegistry) {
        Ok(()) => hresult::S_OK,
        Err(_) => hresult::SELFREG_E_CLASS,
    }
}

/// Windows telling the DLL it has been loaded. The only thing kept is which module this is, so
/// that registration can say where the file lives.
///
/// # Safety
/// Called by Windows.
#[no_mangle]
pub unsafe extern "system" fn DllMain(module: isize, reason: u32, _reserved: *mut c_void) -> i32 {
    const PROCESS_ATTACH: u32 = 1;
    if reason == PROCESS_ATTACH {
        MODULE.store(module, Ordering::Release);
    }
    TRUE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_vtable_is_the_right_shape() {
        // Twenty four function pointers, three of them IUnknown's, in one order that must not move.
        assert_eq!(std::mem::size_of::<Vtable>(), 24 * std::mem::size_of::<usize>());
        assert_eq!(std::mem::size_of::<ClassFactoryVtable>(), 5 * std::mem::size_of::<usize>());
    }

    #[test]
    fn a_class_id_that_is_not_ours_is_refused() {
        let other = Guid { data1: 1, data2: 2, data3: 3, data4: [0; 8] };
        let mut out: *mut c_void = std::ptr::null_mut();
        let answer = unsafe { DllGetClassObject(&other, &IID_ICLASSFACTORY, &mut out) };
        assert_eq!(answer, hresult::CLASS_E_CLASSNOTAVAILABLE);
        assert!(out.is_null());
    }

    #[test]
    fn names_are_cut_to_fit_and_always_terminated() {
        let mut buffer = [0i8; 8];
        unsafe { write_c_string(buffer.as_mut_ptr(), buffer.len(), "a rather long name") };
        let text = unsafe { std::ffi::CStr::from_ptr(buffer.as_ptr()) }.to_string_lossy().to_string();
        assert_eq!(text, "a rathe");
    }
}
