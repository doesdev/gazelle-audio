//! The real PC underneath the driver: COM, the vendor driver objects, and the registry.
//!
//! This is the half that cannot be tested without hardware, so it is kept as thin as it can be:
//! it turns the interface's calls into the traits in [`crate::sub`] and does no deciding of its
//! own. Everything that decides anything is above it, and is tested against fakes.

use std::ffi::{c_void, CStr};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, Ordering};
use std::sync::Arc;

use gazelle_audio_stream_abi::raw::{
    selector, BufferInfoRaw, CallbacksRaw, ChannelInfoRaw, Object, Time, Vtable, ENGINE_VERSION_2, OK,
};
use gazelle_audio_stream_abi::{registry, Entry};
use windows_sys::core::GUID;
use windows_sys::Win32::System::Com::{CLSIDFromString, CoCreateInstance, CLSCTX_INPROC_SERVER};

use crate::aggregate::MAX_DEVICES;
use crate::stream::Stream;
use crate::sub::{Description, DeviceBuffers, Host, Requests, SubDriver};

/// The stream each device's callbacks belong to. Set when its buffers are made, cleared after it
/// is stopped, and never null while a callback can be in flight: the driver that owns the slot
/// holds a counted reference for exactly that time.
static STREAMS: [AtomicPtr<Stream>; MAX_DEVICES] = [const { AtomicPtr::new(null_mut()) }; MAX_DEVICES];

/// A reset a device asked for before its buffers belonged to a stream, kept for the aggregate to
/// act on: at that point there is no DAW to pass it to, and the aggregate is still opening.
static RESET_ASKED: [AtomicBool; MAX_DEVICES] = [const { AtomicBool::new(false) }; MAX_DEVICES];

/// A rate a device said it moved to at the same point, as the bits of an `f64`. Zero is none.
static RATE_SAID: [AtomicU64; MAX_DEVICES] = [const { AtomicU64::new(0) }; MAX_DEVICES];

/// One device has audio for us. This is all that happens on a vendor driver's own thread.
///
/// # Safety
/// The pointer was published by `attach` before the device was started and is cleared only after
/// it has been stopped, so it is a live `Arc<Stream>` for as long as this can run.
unsafe extern "system" fn buffer_switch<const N: usize>(index: i32, _direct: i32) {
    let stream = STREAMS[N].load(Ordering::Acquire);
    if stream.is_null() {
        return;
    }
    unsafe { (*stream).device_callback(N, (index as usize) & 1) };
}

/// A device saying its rate moved: on its way to the DAW once there is one, and kept for the
/// aggregate while it is still opening. Nothing here does more than store a number, because this
/// can be called on the device's own thread.
unsafe extern "system" fn sample_rate_did_change<const N: usize>(rate: f64) {
    let stream = STREAMS[N].load(Ordering::Acquire);
    if stream.is_null() {
        RATE_SAID[N].store(rate.to_bits(), Ordering::Release);
    } else {
        unsafe { (*stream).forward_rate(rate) };
    }
}

/// A device's message. The ones the aggregate cannot answer for itself, which is all of the ones
/// that matter, go up to the DAW.
unsafe extern "system" fn message<const N: usize>(which: i32, value: i32, _message: *mut c_void, _opt: *mut f64) -> i32 {
    match which {
        selector::SUPPORTED => i32::from(matches!(
            value,
            selector::SUPPORTED
                | selector::ENGINE_VERSION
                | selector::RESET_REQUEST
                | selector::RESYNC_REQUEST
                | selector::LATENCIES_CHANGED
                | selector::BUFFER_SIZE_CHANGE
                | selector::OVERLOAD
        )),
        selector::ENGINE_VERSION => ENGINE_VERSION_2,
        // The aggregate wants the plain callback from its devices: it makes the time itself, from
        // the device that drives the callback.
        selector::SUPPORTS_TIME_INFO | selector::SUPPORTS_TIME_CODE => 0,
        other => {
            let stream = STREAMS[N].load(Ordering::Acquire);
            if !stream.is_null() {
                // Once the buffers belong to a stream the DAW is the host, and a reset is its to
                // do: the aggregate never closes a driver from inside that driver's own callback.
                unsafe { (*stream).forward(other, value) }
            } else if other == selector::RESET_REQUEST {
                // Still opening: the aggregate reopens this driver itself before it lets it start.
                RESET_ASKED[N].store(true, Ordering::Release);
                1
            } else {
                0
            }
        }
    }
}

/// Offered so the set is complete. The host above declines the time selector, so a device calls
/// `buffer_switch`; one that calls this anyway is handled the same way.
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

/// One set per device, kept for the life of the process so that a driver holding the pointer past
/// `disposeBuffers` still points at something valid.
static CALLBACKS: [CallbacksRaw; MAX_DEVICES] = [
    callbacks_for::<0>(),
    callbacks_for::<1>(),
    callbacks_for::<2>(),
    callbacks_for::<3>(),
    callbacks_for::<4>(),
    callbacks_for::<5>(),
    callbacks_for::<6>(),
    callbacks_for::<7>(),
];

/// This PC.
pub struct ThisPc;

impl Host for ThisPc {
    fn entries(&self) -> Result<Vec<Entry>, String> {
        registry::entries()
    }

    fn open(&self, entry: &Entry, slot: usize) -> Result<Box<dyn SubDriver>, String> {
        if slot >= MAX_DEVICES {
            return Err(format!("this driver holds at most {MAX_DEVICES} devices"));
        }
        let mut clsid = GUID { data1: 0, data2: 0, data3: 0, data4: [0; 8] };
        let hr = unsafe { CLSIDFromString(registry::wide(&entry.clsid).as_ptr(), &mut clsid) };
        if hr < 0 {
            return Err(format!("{} is not a class id ({hr:#010x})", entry.clsid));
        }
        let mut ptr: *mut c_void = null_mut();
        // The convention this interface is published under uses the class id as the interface id.
        let hr = unsafe { CoCreateInstance(&clsid, null_mut(), CLSCTX_INPROC_SERVER, &clsid, &mut ptr) };
        if hr < 0 || ptr.is_null() {
            return Err(format!("CoCreateInstance answered {hr:#010x}"));
        }
        // Whatever the slot's last driver asked for is not this one's to answer.
        RESET_ASKED[slot].store(false, Ordering::Release);
        RATE_SAID[slot].store(0, Ordering::Release);
        Ok(Box::new(VendorDriver {
            object: ptr.cast(),
            slot,
            created: false,
            started: false,
            infos: Vec::new(),
            stream: None,
        }))
    }
}

/// One vendor driver object.
pub struct VendorDriver {
    object: *mut Object,
    slot: usize,
    created: bool,
    started: bool,
    /// Kept alive for as long as the buffers are: the driver was handed this array.
    infos: Vec<BufferInfoRaw>,
    /// Kept so the pointer published for the callbacks stays alive.
    stream: Option<Arc<Stream>>,
}

// The object is only ever called from the thread that made it; the callbacks reach the stream
// through the static above, not through this.
unsafe impl Send for VendorDriver {}

impl VendorDriver {
    fn vtable(&self) -> *const Vtable {
        unsafe { (*self.object).vtable }
    }

    fn error(&mut self, call: &str) -> String {
        let mut text = [0i8; 512];
        unsafe { ((*self.vtable()).get_error_message)(self.object, text.as_mut_ptr()) };
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

impl SubDriver for VendorDriver {
    fn init(&mut self) -> Result<(), String> {
        // The interface takes the host's main window here. This driver has no window, and both
        // drivers on this PC accept nothing.
        let ok = unsafe { ((*self.vtable()).init)(self.object, null_mut()) };
        if ok == 0 {
            return Err(self.error("init"));
        }
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

        let mut kind = |is_input: bool| -> Result<i32, String> {
            let mut info = ChannelInfoRaw { is_input: i32::from(is_input), ..ChannelInfoRaw::default() };
            let code = unsafe { ((*vtable).get_channel_info)(self.object, &mut info) };
            if code != OK {
                return Err(self.error("getChannelInfo"));
            }
            Ok(info.sample_type)
        };
        let input_type = kind(true)?;
        let output_type = kind(false)?;

        let output_ready = unsafe { ((*vtable).output_ready)(self.object) } == OK;

        Ok(Description {
            name,
            version,
            inputs,
            outputs,
            min,
            max,
            preferred,
            granularity,
            rate,
            latency_in,
            latency_out,
            input_type,
            output_type,
            output_ready,
        })
    }

    fn can_rate(&mut self, hz: f64) -> bool {
        unsafe { ((*self.vtable()).can_sample_rate)(self.object, hz) == OK }
    }

    fn set_rate(&mut self, hz: f64) -> Result<(), String> {
        let code = unsafe { ((*self.vtable()).set_sample_rate)(self.object, hz) };
        self.check("setSampleRate", code)
    }

    fn read_rate(&mut self) -> Result<f64, String> {
        let mut rate = 0f64;
        let code = unsafe { ((*self.vtable()).get_sample_rate)(self.object, &mut rate) };
        self.check("getSampleRate", code)?;
        Ok(rate)
    }

    fn take_requests(&mut self) -> Requests {
        let said = RATE_SAID[self.slot].swap(0, Ordering::AcqRel);
        Requests {
            reset: RESET_ASKED[self.slot].swap(false, Ordering::AcqRel),
            rate: (said != 0).then(|| f64::from_bits(said)),
        }
    }

    fn create_buffers(&mut self, inputs: &[i32], outputs: &[i32], block: i32) -> Result<DeviceBuffers, String> {
        self.infos = inputs
            .iter()
            .map(|&channel| BufferInfoRaw { is_input: 1, channel, buffers: [null_mut(); 2] })
            .chain(outputs.iter().map(|&channel| BufferInfoRaw { is_input: 0, channel, buffers: [null_mut(); 2] }))
            .collect();
        let total = self.infos.len() as i32;
        let code = unsafe {
            ((*self.vtable()).create_buffers)(self.object, self.infos.as_mut_ptr(), total, block, &CALLBACKS[self.slot])
        };
        self.check("createBuffers", code)?;
        self.created = true;
        let split = inputs.len();
        Ok(DeviceBuffers {
            inputs: self.infos[..split].iter().map(|info| [info.buffers[0].cast(), info.buffers[1].cast()]).collect(),
            outputs: self.infos[split..].iter().map(|info| [info.buffers[0].cast(), info.buffers[1].cast()]).collect(),
            block: block.max(0) as usize,
        })
    }

    fn attach(&mut self, stream: Arc<Stream>, _device: usize) {
        // Publish the pointer while holding the reference that keeps it alive.
        let pointer = Arc::as_ptr(&stream) as *mut Stream;
        self.stream = Some(stream);
        STREAMS[self.slot].store(pointer, Ordering::Release);
    }

    fn detach(&mut self) {
        STREAMS[self.slot].store(null_mut(), Ordering::Release);
        self.stream = None;
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
            unsafe { ((*self.vtable()).dispose_buffers)(self.object) };
            self.created = false;
            self.infos.clear();
        }
    }
}

impl Drop for VendorDriver {
    fn drop(&mut self) {
        self.stop();
        self.dispose_buffers();
        self.detach();
        unsafe { ((*self.vtable()).release)(self.object) };
    }
}

/// The real registry, writing under `HKEY_LOCAL_MACHINE`, which is why registration needs
/// administrator rights.
pub struct MachineRegistry;

impl crate::registration::RegistryWriter for MachineRegistry {
    fn create_key(&mut self, path: &str) -> Result<(), String> {
        use windows_sys::Win32::System::Registry::{
            RegCloseKey, RegCreateKeyExW, HKEY, HKEY_LOCAL_MACHINE, KEY_WOW64_64KEY, KEY_WRITE, REG_OPTION_NON_VOLATILE,
        };
        let mut key: HKEY = null_mut();
        let status = unsafe {
            RegCreateKeyExW(
                HKEY_LOCAL_MACHINE,
                registry::wide(path).as_ptr(),
                0,
                null(),
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE | KEY_WOW64_64KEY,
                null(),
                &mut key,
                null_mut(),
            )
        };
        if status != windows_sys::Win32::Foundation::ERROR_SUCCESS {
            return Err(registry::reg_error(path, status));
        }
        unsafe { RegCloseKey(key) };
        Ok(())
    }

    fn set_value(&mut self, path: &str, name: &str, value: &str) -> Result<(), String> {
        use windows_sys::Win32::System::Registry::{
            RegCloseKey, RegOpenKeyExW, RegSetValueExW, HKEY, HKEY_LOCAL_MACHINE, KEY_WOW64_64KEY, KEY_WRITE, REG_SZ,
        };
        let mut key: HKEY = null_mut();
        let status =
            unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, registry::wide(path).as_ptr(), 0, KEY_WRITE | KEY_WOW64_64KEY, &mut key) };
        if status != windows_sys::Win32::Foundation::ERROR_SUCCESS {
            return Err(registry::reg_error(path, status));
        }
        let wide_value = registry::wide(value);
        let bytes = wide_value.len() * 2;
        let wide_name = registry::wide(name);
        let name_ptr = if name.is_empty() { null() } else { wide_name.as_ptr() };
        let status =
            unsafe { RegSetValueExW(key, name_ptr, 0, REG_SZ, wide_value.as_ptr().cast(), bytes as u32) };
        unsafe { RegCloseKey(key) };
        if status != windows_sys::Win32::Foundation::ERROR_SUCCESS {
            return Err(registry::reg_error(&format!("{path} value {name}"), status));
        }
        Ok(())
    }

    fn delete_tree(&mut self, path: &str) -> Result<(), String> {
        use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, ERROR_SUCCESS};
        use windows_sys::Win32::System::Registry::{RegDeleteTreeW, HKEY_LOCAL_MACHINE};
        let status = unsafe { RegDeleteTreeW(HKEY_LOCAL_MACHINE, registry::wide(path).as_ptr()) };
        match status {
            ERROR_SUCCESS | ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND => Ok(()),
            other => Err(registry::reg_error(path, other)),
        }
    }
}
