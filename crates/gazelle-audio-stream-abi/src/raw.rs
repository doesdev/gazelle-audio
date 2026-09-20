//! The interface across the boundary: the structures, the vtable, and the numbers.
//!
//! Every type here is `#[repr(C)]` and every function pointer is `extern "system"`, because both
//! sides of this boundary are C++ objects compiled by MSVC.

use std::ffi::c_void;
use std::os::raw::c_char;

/// A class id, laid out as Windows lays one out. Declared here so that the structures in this
//  module are the same on every target and this crate's tests run anywhere.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Guid {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

impl Guid {
    /// The same class id in the braced, upper case form the registry writes.
    pub fn to_registry_string(self) -> String {
        let d4 = self.data4;
        format!(
            "{{{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}",
            self.data1, self.data2, self.data3, d4[0], d4[1], d4[2], d4[3], d4[4], d4[5], d4[6], d4[7]
        )
    }
}

#[cfg(windows)]
impl From<Guid> for windows_sys::core::GUID {
    fn from(value: Guid) -> Self {
        windows_sys::core::GUID { data1: value.data1, data2: value.data2, data3: value.data3, data4: value.data4 }
    }
}

#[cfg(windows)]
impl From<windows_sys::core::GUID> for Guid {
    fn from(value: windows_sys::core::GUID) -> Self {
        Guid { data1: value.data1, data2: value.data2, data3: value.data3, data4: value.data4 }
    }
}

/// `IUnknown`'s own interface id, which every COM object must answer to.
pub const IID_IUNKNOWN: Guid = Guid { data1: 0, data2: 0, data3: 0, data4: [0xC0, 0, 0, 0, 0, 0, 0, 0x46] };
/// `IClassFactory`'s interface id, which is what `DllGetClassObject` is usually asked for.
pub const IID_ICLASSFACTORY: Guid = Guid { data1: 1, data2: 0, data3: 0, data4: [0xC0, 0, 0, 0, 0, 0, 0, 0x46] };

/// One entry from `getClockSources`.
#[repr(C)]
#[derive(Default)]
pub struct ClockSourceRaw {
    pub index: i32,
    pub associated_channel: i32,
    pub associated_group: i32,
    pub is_current: i32,
    pub name: [c_char; 32],
}

/// What `getChannelInfo` is asked and what it answers: the caller fills `channel` and `is_input`.
#[repr(C)]
#[derive(Default)]
pub struct ChannelInfoRaw {
    pub channel: i32,
    pub is_input: i32,
    pub is_active: i32,
    pub channel_group: i32,
    pub sample_type: i32,
    pub name: [c_char; 32],
}

/// One channel asked for at `createBuffers`. The caller fills `is_input` and `channel`; the driver
/// fills `buffers` with the two halves of that channel's double buffer.
#[repr(C)]
pub struct BufferInfoRaw {
    pub is_input: i32,
    pub channel: i32,
    pub buffers: [*mut c_void; 2],
}

/// A 64 bit count, as this interface carries one on a platform whose compiler may have none: two
/// 32 bit words, high word first.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Samples {
    pub hi: u32,
    pub lo: u32,
}

impl Samples {
    pub fn from_i64(value: i64) -> Samples {
        let value = value as u64;
        Samples { hi: (value >> 32) as u32, lo: value as u32 }
    }

    pub fn to_i64(self) -> i64 {
        (((self.hi as u64) << 32) | self.lo as u64) as i64
    }
}

/// What the driver knows about the stream when it calls back, when the host asked for it.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct TimeInfo {
    pub speed: f64,
    pub system_time: Samples,
    pub sample_position: Samples,
    pub sample_rate: f64,
    pub flags: u32,
    pub reserved: [c_char; 12],
}

/// The time code half of the same structure. Nothing in this workspace fills it in.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TimeCode {
    pub speed: f64,
    pub samples: Samples,
    pub flags: u32,
    pub future: [c_char; 64],
}

impl Default for TimeCode {
    fn default() -> Self {
        TimeCode { speed: 0.0, samples: Samples::default(), flags: 0, future: [0; 64] }
    }
}

/// What `bufferSwitchTimeInfo` is handed.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Time {
    pub reserved: [i32; 4],
    pub info: TimeInfo,
    pub code: TimeCode,
}

/// The flags in [`TimeInfo::flags`] that anything here sets.
pub mod time_flags {
    pub const SYSTEM_TIME_VALID: u32 = 1;
    pub const SAMPLE_POSITION_VALID: u32 = 1 << 1;
    pub const SAMPLE_RATE_VALID: u32 = 1 << 2;
    pub const SPEED_VALID: u32 = 1 << 3;
    pub const SAMPLE_RATE_CHANGED: u32 = 1 << 4;
}

/// The four functions a host hands a driver at `createBuffers`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct CallbacksRaw {
    pub buffer_switch: unsafe extern "system" fn(i32, i32),
    pub sample_rate_did_change: unsafe extern "system" fn(f64),
    pub message: unsafe extern "system" fn(i32, i32, *mut c_void, *mut f64) -> i32,
    pub buffer_switch_time_info: unsafe extern "system" fn(*mut Time, i32, i32) -> *mut Time,
}

/// `IUnknown`'s three entries, then the interface's twenty one, in order.
#[repr(C)]
pub struct Vtable {
    pub query_interface: unsafe extern "system" fn(*mut Object, *const Guid, *mut *mut c_void) -> i32,
    pub add_ref: unsafe extern "system" fn(*mut Object) -> u32,
    pub release: unsafe extern "system" fn(*mut Object) -> u32,
    pub init: unsafe extern "system" fn(*mut Object, *mut c_void) -> i32,
    pub get_driver_name: unsafe extern "system" fn(*mut Object, *mut c_char),
    pub get_driver_version: unsafe extern "system" fn(*mut Object) -> i32,
    pub get_error_message: unsafe extern "system" fn(*mut Object, *mut c_char),
    pub start: unsafe extern "system" fn(*mut Object) -> i32,
    pub stop: unsafe extern "system" fn(*mut Object) -> i32,
    pub get_channels: unsafe extern "system" fn(*mut Object, *mut i32, *mut i32) -> i32,
    pub get_latencies: unsafe extern "system" fn(*mut Object, *mut i32, *mut i32) -> i32,
    pub get_buffer_size: unsafe extern "system" fn(*mut Object, *mut i32, *mut i32, *mut i32, *mut i32) -> i32,
    pub can_sample_rate: unsafe extern "system" fn(*mut Object, f64) -> i32,
    pub get_sample_rate: unsafe extern "system" fn(*mut Object, *mut f64) -> i32,
    pub set_sample_rate: unsafe extern "system" fn(*mut Object, f64) -> i32,
    pub get_clock_sources: unsafe extern "system" fn(*mut Object, *mut ClockSourceRaw, *mut i32) -> i32,
    pub set_clock_source: unsafe extern "system" fn(*mut Object, i32) -> i32,
    pub get_sample_position: unsafe extern "system" fn(*mut Object, *mut Samples, *mut Samples) -> i32,
    pub get_channel_info: unsafe extern "system" fn(*mut Object, *mut ChannelInfoRaw) -> i32,
    pub create_buffers: unsafe extern "system" fn(*mut Object, *mut BufferInfoRaw, i32, i32, *const CallbacksRaw) -> i32,
    pub dispose_buffers: unsafe extern "system" fn(*mut Object) -> i32,
    pub control_panel: unsafe extern "system" fn(*mut Object) -> i32,
    pub future: unsafe extern "system" fn(*mut Object, i32, *mut c_void) -> i32,
    pub output_ready: unsafe extern "system" fn(*mut Object) -> i32,
}

/// A driver object: a pointer to its vtable, and whatever the driver keeps after it.
#[repr(C)]
pub struct Object {
    pub vtable: *const Vtable,
}

/// The vtable of `IClassFactory`, which is what `DllGetClassObject` hands back.
#[repr(C)]
pub struct ClassFactoryVtable {
    pub query_interface: unsafe extern "system" fn(*mut ClassFactory, *const Guid, *mut *mut c_void) -> i32,
    pub add_ref: unsafe extern "system" fn(*mut ClassFactory) -> u32,
    pub release: unsafe extern "system" fn(*mut ClassFactory) -> u32,
    pub create_instance: unsafe extern "system" fn(*mut ClassFactory, *mut c_void, *const Guid, *mut *mut c_void) -> i32,
    pub lock_server: unsafe extern "system" fn(*mut ClassFactory, i32) -> i32,
}

#[repr(C)]
pub struct ClassFactory {
    pub vtable: *const ClassFactoryVtable,
}

/// The call answered yes. Everything else is a refusal whose text comes from `getErrorMessage`.
pub const OK: i32 = 0;
/// `init` answers one of these two rather than an error code.
pub const TRUE: i32 = 1;
pub const FALSE: i32 = 0;

/// The interface's own refusals, in its own numbering.
pub mod error {
    /// There is no driver, or no hardware behind it.
    pub const NOT_PRESENT: i32 = -1000;
    pub const HW_MALFUNCTION: i32 = -999;
    /// The call was made with something the driver will not take.
    pub const INVALID_PARAMETER: i32 = -998;
    /// The call came at a moment the driver is not in.
    pub const BAD_MODE: i32 = -997;
    /// Something changed under the driver and the host should reset it.
    pub const NO_CLOCK: i32 = -995;
    pub const NO_MEMORY: i32 = -994;
}

/// The COM results this workspace hands back.
pub mod hresult {
    pub const S_OK: i32 = 0;
    pub const S_FALSE: i32 = 1;
    pub const E_NOINTERFACE: i32 = -2147467262; // 0x80004002
    pub const E_OUTOFMEMORY: i32 = -2147024882; // 0x8007000E
    pub const E_INVALIDARG: i32 = -2147024809; // 0x80070057
    pub const CLASS_E_CLASSNOTAVAILABLE: i32 = -2147221231; // 0x80040111
    pub const CLASS_E_NOAGGREGATION: i32 = -2147221232; // 0x80040110
    pub const SELFREG_E_CLASS: i32 = -2147220992; // 0x80040201
}

/// The selectors a driver and a host pass each other through the message callback.
pub mod selector {
    /// "Do you handle selector `value`?"
    pub const SUPPORTED: i32 = 1;
    /// "Which version of the host engine is this?"
    pub const ENGINE_VERSION: i32 = 2;
    /// "Stop me, dispose my buffers and start me again."
    pub const RESET_REQUEST: i32 = 3;
    /// "My preferred buffer size has changed."
    pub const BUFFER_SIZE_CHANGE: i32 = 4;
    /// "I lost sync; resynchronise."
    pub const RESYNC_REQUEST: i32 = 5;
    /// "My latencies have changed."
    pub const LATENCIES_CHANGED: i32 = 6;
    /// "Do you take time info in the buffer callback?"
    pub const SUPPORTS_TIME_INFO: i32 = 7;
    /// "Do you take time code?"
    pub const SUPPORTS_TIME_CODE: i32 = 8;
    /// "I could not keep up."
    pub const OVERLOAD: i32 = 15;
}

/// The engine version a host reports when it takes the messages above.
pub const ENGINE_VERSION_2: i32 = 2;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_class_id_prints_the_way_the_registry_holds_it() {
        let guid = Guid {
            data1: 0xF18C_80B4,
            data2: 0x2DE1,
            data3: 0x43B8,
            data4: [0xAC, 0x84, 0x19, 0xDD, 0xC2, 0x2E, 0xBF, 0xC8],
        };
        assert_eq!(guid.to_registry_string(), "{F18C80B4-2DE1-43B8-AC84-19DDC22EBFC8}");
    }

    #[test]
    fn a_sample_count_survives_the_two_halves_it_is_carried_in() {
        for value in [0i64, 1, 512, 0xFFFF_FFFF, 0x1_0000_0000, 96_000 * 3600 * 24] {
            assert_eq!(Samples::from_i64(value).to_i64(), value, "{value}");
        }
    }

    #[test]
    fn the_structures_are_the_size_the_interface_says() {
        // A wrong size here would put every later field, and every later vtable slot, in the wrong
        // place, which is the one mistake a hand written declaration can make.
        assert_eq!(std::mem::size_of::<Samples>(), 8);
        assert_eq!(std::mem::size_of::<ClockSourceRaw>(), 4 * 4 + 32);
        assert_eq!(std::mem::size_of::<ChannelInfoRaw>(), 5 * 4 + 32);
        assert_eq!(std::mem::size_of::<TimeInfo>(), 8 + 8 + 8 + 8 + 4 + 12);
        assert_eq!(std::mem::size_of::<TimeCode>(), 8 + 8 + 4 + 64 + 4);
        assert_eq!(std::mem::size_of::<Time>(), 16 + std::mem::size_of::<TimeInfo>() + std::mem::size_of::<TimeCode>());
        assert_eq!(std::mem::size_of::<Vtable>(), 24 * std::mem::size_of::<usize>());
    }
}
