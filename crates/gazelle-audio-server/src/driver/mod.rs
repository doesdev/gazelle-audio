//! The audio driver's own settings on this PC: buffer size, latency and Safe Mode.
//!
//! These belong to the USB audio driver (Thesycon's TUSBAudio, branded for Antelope), not to the
//! device, so they never cross the HID protocol the rest of the server speaks. Each driver installs
//! a user-mode API DLL beside its control panel; this module finds it, loads it, and calls the read
//! functions named in `.agent/reference/driver-api.md` and **one setter**,
//! `SetASIOBufferPreferredSize`, which sets the preferred buffer and Safe Mode together
//! ([`write_device`]). Nothing here loads firmware, starts, enables or sets anything else, and the
//! Windows side resolves nothing outside [`READ_EXPORTS`] and [`WRITE_EXPORTS`].
//!
//! The DLL sits behind [`DriverApi`] and the PC (where DLLs are found, how they are loaded, the
//! registry) behind [`DriverHost`], so every rule here is tested against fakes and no test touches
//! a real driver. A driver device is tied to a Gazelle device by **serial**: the driver's
//! `GetDeviceProperties` serial is the `<n>` in Gazelle's `serial:<n>`.

pub mod asio;
#[cfg(windows)]
pub mod windows;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;

pub use asio::AsioInstance;

/// Every export the Windows side may resolve. Anything else in the DLL (its `Set*`, `Load*`,
/// `Start*`, `Dfu*`, `Enable*` and `Disable*` functions among them) is never looked up, so it
/// cannot be called by mistake.
pub const READ_EXPORTS: &[&str] = &[
    "TUSBAUDIO_GetApiVersion",
    "TUSBAUDIO_CheckApiVersion",
    "TUSBAUDIO_EnumerateDevices",
    "TUSBAUDIO_GetDeviceCount",
    "TUSBAUDIO_OpenDeviceByIndex",
    "TUSBAUDIO_CloseDevice",
    "TUSBAUDIO_GetDriverInfo",
    "TUSBAUDIO_GetDeviceProperties",
    "TUSBAUDIO_GetCurrentSampleRate",
    "TUSBAUDIO_GetASIOInstanceCount",
    "TUSBAUDIO_GetASIOInstanceInfo",
    "TUSBAUDIO_StatusCodeStringA",
];

/// The one write the Windows side may resolve: the preferred ASIO buffer and Safe Mode, in one
/// call (probed on both drivers with the user, 2026-09-18). A test keeps this list to that name.
pub const WRITE_EXPORTS: &[&str] = &["TUSBAUDIO_SetASIOBufferPreferredSize"];

/// The API version asked of `CheckApiVersion`: the oldest seen (the Studio+'s), whose calls and
/// structures are the ones read here. A newer DLL that says it still serves this is read.
pub const REQUIRED_API: (u32, u32) = (5, 7);

/// API versions this code has been checked against, live, on both drivers.
pub const KNOWN_APIS: &[(u32, u32)] = &[(5, 7), (5, 12)];

/// No PC has more of one driver's devices attached than this; a larger count is a driver that
/// answered something other than a count.
const MAX_DEVICES: u32 = 64;

/// How long an answer is served from the cache before the driver is asked again.
pub const CACHE_FOR: Duration = Duration::from_secs(5);

/// An opened driver device, as the DLL hands it out.
pub type Handle = usize;

/// Why one call into the DLL did not answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallError {
    /// The DLL does not export this function.
    Missing(&'static str),
    /// The function answered with a status other than OK.
    Status { call: &'static str, code: u32, text: String },
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CallError::Missing(name) => write!(f, "its API has no {name}"),
            CallError::Status { call, code, text } if text.is_empty() => write!(f, "{call} answered status {code:#x}"),
            CallError::Status { call, code, text } => write!(f, "{call} answered {text} (status {code:#x})"),
        }
    }
}

/// `TUSBAUDIO_GetDriverInfo`'s six numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DriverInfo {
    pub api_major: u32,
    pub api_minor: u32,
    pub driver_major: u32,
    pub driver_minor: u32,
    pub driver_sub: u32,
    pub flags: u32,
}

/// The part of `TUSBAUDIO_GetDeviceProperties` this module uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceProperties {
    pub vid: u32,
    pub pid: u32,
    pub serial: String,
    pub product: String,
}

/// One loaded driver API DLL: the read functions and the one setter, nothing else.
pub trait DriverApi: Send + Sync {
    /// `GetApiVersion`: major in the high 16 bits.
    fn api_version(&self) -> Result<u32, CallError>;
    fn check_api_version(&self, major: u32, minor: u32) -> Result<bool, CallError>;
    fn driver_info(&self) -> Result<DriverInfo, CallError>;
    fn enumerate_devices(&self) -> Result<(), CallError>;
    fn device_count(&self) -> Result<u32, CallError>;
    fn open_device(&self, index: u32) -> Result<Handle, CallError>;
    fn close_device(&self, handle: Handle);
    fn device_properties(&self, handle: Handle) -> Result<DeviceProperties, CallError>;
    fn current_sample_rate(&self, handle: Handle) -> Result<u32, CallError>;
    /// `None` when the DLL does not export `GetASIOInstanceCount` (the Studio+'s 5.7 does not).
    fn asio_instance_count(&self) -> Option<Result<u32, CallError>>;
    /// The raw structure, at least [`asio::INFO_LEN`] bytes.
    fn asio_instance_info(&self, index: u32) -> Result<Vec<u8>, CallError>;
    /// `SetASIOBufferPreferredSize(asioInstance, referenceSampleRate, preferredSize, options)`: the
    /// only write. `options` replaces the driver's options; bit 0x10000 is Safe Mode.
    fn set_asio_buffer_preferred_size(&self, asio_instance: u32, reference_sample_rate: u32, preferred_size: u32, options: u32) -> Result<(), CallError>;
}

/// The PC: where the driver DLLs are, loading one, and the registry the driver keeps settings in.
pub trait DriverHost: Send + Sync {
    /// Every driver API DLL installed. An empty list is an ordinary answer.
    fn find_api_dlls(&self) -> Result<Vec<PathBuf>, String>;
    fn load(&self, dll: &Path) -> Result<Arc<dyn DriverApi>, String>;
    /// A `REG_DWORD` under `HKEY_LOCAL_MACHINE`; `None` when the key or value is not there.
    fn registry_u32(&self, subkey: &str, name: &str) -> Result<Option<u32>, String>;
}

/// A value that was read, or why not. Each setting is read on its own, so one that fails does not
/// hide the others.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Reading<T> {
    Read { value: T },
    Unread { message: String },
}

impl<T> Reading<T> {
    fn unread(message: impl Into<String>) -> Self {
        Reading::Unread { message: message.into() }
    }
}

/// What the driver says about one device.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DriverSettings {
    /// The API DLL that answered.
    pub dll: String,
    /// The driver's Windows service, whose registry key holds Safe Mode.
    pub service: String,
    /// The API DLL's version, `major.minor`.
    pub api_version: String,
    /// Whether that version is one this code was checked against.
    pub api_known: bool,
    /// The driver's own version, `major.minor.sub`.
    pub driver_version: Reading<String>,
    /// The rate the driver is running the device at, in Hz.
    pub sample_rate: Reading<u32>,
    /// How many ASIO instances the driver has, when its DLL can say.
    pub asio_instances: Option<u32>,
    /// Which instance was read.
    pub asio_instance: u32,
    pub asio: Reading<AsioInstance>,
    pub safe_mode: Reading<bool>,
}

/// The whole answer for one device.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum DriverAnswer {
    /// The device is not served by a driver on this PC at all: the loopback.
    NoDriver { message: String },
    /// No driver was found, or none lists this device.
    NotFound { message: String },
    /// A driver was there and could not be read.
    Failed { message: String },
    Read(DriverSettings),
}

/// An answer with when it was read.
#[derive(Clone, Debug, Serialize)]
pub struct DriverReport {
    pub device_id: String,
    /// When the driver was asked, in milliseconds since the Unix epoch.
    pub read_at_ms: u64,
    /// True when this came from the cache rather than a fresh read.
    pub cached: bool,
    #[serde(flatten)]
    pub answer: DriverAnswer,
}

/// The service name from the DLL's file name: `ZenStudioTBapi_x64.dll` is `ZenStudioTB`.
pub fn service_of(dll: &Path) -> Option<String> {
    const SUFFIX: &str = "api_x64.dll";
    let name = dll.file_name()?.to_str()?;
    let cut = name.len().checked_sub(SUFFIX.len())?;
    (cut > 0 && name.is_char_boundary(cut) && name[cut..].eq_ignore_ascii_case(SUFFIX)).then(|| name[..cut].to_string())
}

fn same_serial(driver: &str, gazelle: &str) -> bool {
    let driver = driver.trim();
    !driver.is_empty() && driver.eq_ignore_ascii_case(gazelle.trim())
}

fn file_name(dll: &Path) -> String {
    dll.file_name().map_or_else(|| dll.display().to_string(), |n| n.to_string_lossy().into_owned())
}

/// Closes a device handle however the read that opened it ends.
struct Opened<'a> {
    api: &'a dyn DriverApi,
    handle: Handle,
}

impl Drop for Opened<'_> {
    fn drop(&mut self) {
        self.api.close_device(self.handle);
    }
}

/// Read the driver settings of the device whose serial is `serial`, from whichever driver on this
/// PC lists it. Never panics on anything a driver answers; every failure is a message.
pub fn read_device(host: &dyn DriverHost, serial: &str) -> DriverAnswer {
    let dlls = match host.find_api_dlls() {
        Err(e) => return DriverAnswer::Failed { message: format!("Could not look for an audio driver: {e}.") },
        Ok(dlls) if dlls.is_empty() => {
            return DriverAnswer::NotFound { message: "No driver API found on this PC.".into() }
        }
        Ok(dlls) => dlls,
    };
    let mut problems = Vec::new();
    for dll in &dlls {
        match read_from(host, dll, serial) {
            Ok(Some(settings)) => return DriverAnswer::Read(settings),
            Ok(None) => {}
            Err(problem) => problems.push(format!("{}: {problem}", file_name(dll))),
        }
    }
    if problems.is_empty() {
        DriverAnswer::NotFound { message: "No driver API on this PC lists this device.".into() }
    } else {
        DriverAnswer::Failed {
            message: format!("No driver that could be read lists this device. {}.", problems.join("; ")),
        }
    }
}

/// One DLL: `Ok(None)` when it does not list the device.
fn read_from(host: &dyn DriverHost, dll: &Path, serial: &str) -> Result<Option<DriverSettings>, String> {
    let api = host.load(dll)?;
    let api = api.as_ref();
    let version = api.api_version().map_err(|e| e.to_string())?;
    let (major, minor) = (version >> 16, version & 0xffff);
    if !api.check_api_version(REQUIRED_API.0, REQUIRED_API.1).map_err(|e| e.to_string())? {
        return Err(format!("its API ({major}.{minor}) is not one Gazelle can read"));
    }
    api.enumerate_devices().map_err(|e| e.to_string())?;
    let count = api.device_count().map_err(|e| e.to_string())?;
    if count > MAX_DEVICES {
        return Err(format!("it reports {count} devices, which is not a count"));
    }
    for index in 0..count {
        let opened = Opened { api, handle: api.open_device(index).map_err(|e| e.to_string())? };
        let properties = api.device_properties(opened.handle).map_err(|e| e.to_string())?;
        if !same_serial(&properties.serial, serial) {
            continue;
        }
        let sample_rate = match api.current_sample_rate(opened.handle) {
            Ok(rate) if asio::plausible_rate(rate) => Reading::Read { value: rate },
            Ok(rate) => Reading::unread(format!("The driver's sample rate ({rate}) is not a plausible rate.")),
            Err(e) => Reading::unread(format!("Could not be read: {e}.")),
        };
        drop(opened);

        let driver_version = match api.driver_info() {
            Ok(info) => Reading::Read { value: format!("{}.{}.{}", info.driver_major, info.driver_minor, info.driver_sub) },
            Err(e) => Reading::unread(format!("Could not be read: {e}.")),
        };
        let asio_instance = 0;
        let (asio_instances, asio) = match api.asio_instance_count() {
            None => (None, read_asio(api, asio_instance)),
            Some(Ok(0)) => (Some(0), Reading::unread("The driver has no ASIO instance.")),
            Some(Ok(n)) => (Some(n), read_asio(api, asio_instance)),
            Some(Err(e)) => (None, Reading::unread(format!("Could not be read: {e}."))),
        };
        let service = service_of(dll);
        // The driver's own flag when the structure was read; the registry value only when it was not.
        let safe_mode = match (&asio, &service) {
            (Reading::Read { value }, _) => Reading::Read { value: value.safe_mode },
            (_, Some(service)) => safe_mode(host, service, asio_instance),
            (_, None) => Reading::unread("Could not tell which driver service this is, so its Safe Mode setting was not found."),
        };
        return Ok(Some(DriverSettings {
            dll: dll.display().to_string(),
            service: service.unwrap_or_default(),
            api_version: format!("{major}.{minor}"),
            api_known: KNOWN_APIS.contains(&(major, minor)),
            driver_version,
            sample_rate,
            asio_instances,
            asio_instance,
            asio,
            safe_mode,
        }));
    }
    Ok(None)
}

fn read_asio(api: &dyn DriverApi, index: u32) -> Reading<AsioInstance> {
    match api.asio_instance_info(index) {
        Ok(bytes) => match asio::parse(&bytes) {
            Ok(instance) => Reading::Read { value: instance },
            Err(why) => Reading::unread(format!("Could not be read: {why}.")),
        },
        Err(e) => Reading::unread(format!("Could not be read: {e}.")),
    }
}

/// `AsioSafeMode`, per instance where the driver keeps it so (the Quadro's), else flat (the
/// Studio+'s).
fn safe_mode(host: &dyn DriverHost, service: &str, instance: u32) -> Reading<bool> {
    let base = format!(r"SYSTEM\CurrentControlSet\Services\{service}\ParametersDriver\Settings");
    for key in [format!(r"{base}\AsioInstance{instance}"), base.clone()] {
        match host.registry_u32(&key, "AsioSafeMode") {
            Ok(Some(value)) => return Reading::Read { value: value != 0 },
            Ok(None) => {}
            Err(e) => return Reading::unread(format!("Could not be read: {e}.")),
        }
    }
    Reading::unread("The driver keeps no Safe Mode setting in the registry.")
}

/// Reads on demand and keeps each answer for [`CACHE_FOR`], one read at a time.
pub struct DriverService {
    host: Arc<dyn DriverHost>,
    keep_for: Duration,
    cache: Mutex<HashMap<String, (Instant, u64, DriverAnswer)>>,
    /// Held across a read, so two requests do not call into a DLL at once.
    reading: Mutex<()>,
}

impl DriverService {
    pub fn new(host: Arc<dyn DriverHost>, keep_for: Duration) -> Self {
        DriverService { host, keep_for, cache: Mutex::new(HashMap::new()), reading: Mutex::new(()) }
    }

    /// The real PC's drivers on Windows; elsewhere, a host that says there are none to read.
    pub fn for_this_pc() -> Arc<Self> {
        #[cfg(windows)]
        let host: Arc<dyn DriverHost> = Arc::new(windows::ThisPc::default());
        #[cfg(not(windows))]
        let host: Arc<dyn DriverHost> = Arc::new(Elsewhere);
        Arc::new(DriverService::new(host, CACHE_FOR))
    }

    /// The device's settings. Blocking: it may load a DLL and call into it, so call it off the
    /// async runtime. `refresh` skips the cache.
    pub fn read(&self, device_id: &str, serial: &str, refresh: bool) -> DriverReport {
        let _one_at_a_time = self.reading.lock().unwrap_or_else(|e| e.into_inner());
        if !refresh {
            let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            if let Some((at, read_at_ms, answer)) = cache.get(serial) {
                if at.elapsed() < self.keep_for {
                    return DriverReport { device_id: device_id.into(), read_at_ms: *read_at_ms, cached: true, answer: answer.clone() };
                }
            }
        }
        let answer = read_device(self.host.as_ref(), serial);
        let read_at_ms = now_ms();
        self.cache.lock().unwrap_or_else(|e| e.into_inner()).insert(serial.to_string(), (Instant::now(), read_at_ms, answer.clone()));
        DriverReport { device_id: device_id.into(), read_at_ms, cached: false, answer }
    }
}

impl DriverService {
    /// Change the device's buffer size and/or Safe Mode, then read back. Blocking, as `read` is,
    /// and under the same one-at-a-time lock. The read-back replaces the cached answer.
    pub fn write(&self, device_id: &str, serial: &str, change: &DriverChange, dry_run: bool) -> Result<DriverWriteReport, WriteRefusal> {
        let _one_at_a_time = self.reading.lock().unwrap_or_else(|e| e.into_inner());
        let write = write_device(self.host.as_ref(), serial, change, dry_run)?;
        let read_at_ms = now_ms();
        self.cache.lock().unwrap_or_else(|e| e.into_inner()).insert(serial.to_string(), (Instant::now(), read_at_ms, write.read_back.clone()));
        Ok(DriverWriteReport {
            device_id: device_id.into(),
            outcome: write.outcome,
            message: write.message,
            call: write.call,
            read_back: DriverReport { device_id: device_id.into(), read_at_ms, cached: false, answer: write.read_back },
        })
    }
}

/// What a request asks to change. Anything left out is sent as the driver has it now.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DriverChange {
    /// The preferred buffer, in samples: one of the sizes the driver offers.
    #[serde(default)]
    pub buffer_size: Option<u32>,
    #[serde(default)]
    pub safe_mode: Option<bool>,
    /// Change it even while a program (a DAW) is using the driver's ASIO interface, whose audio
    /// then restarts.
    #[serde(default)]
    pub force: bool,
}

/// The one call, with the vendor's own names for its arguments.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct SetterCall {
    pub asio_instance: u32,
    pub reference_sample_rate: u32,
    pub preferred_size: u32,
    pub options: u32,
}

/// Why nothing was sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RefusalCode {
    /// No driver lists the device, or the device has none (the loopback).
    Unavailable,
    /// The driver's current settings could not be read, so the call cannot be built from them.
    Unreadable,
    NothingToChange,
    /// A buffer size the driver does not offer.
    NotOffered,
    /// A program is using the driver's ASIO interface and the request did not say `force`.
    AsioInUse,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct WriteRefusal {
    pub code: RefusalCode,
    pub message: String,
}

impl WriteRefusal {
    fn new(code: RefusalCode, message: impl Into<String>) -> Self {
        WriteRefusal { code, message: message.into() }
    }
}

/// What happened once a call was built.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WriteOutcome {
    /// Sent, and the driver now reports what was sent.
    Applied,
    /// Sent, answered OK, and the driver reports something else.
    Mismatch,
    /// Sent, and the driver could not be read back.
    Unconfirmed,
    /// Sent, and the driver answered a status other than OK.
    Failed,
    /// The driver already has what was asked for, so nothing was sent.
    Unchanged,
    /// Built and not sent: the server or the request is a dry run.
    DryRun,
}

/// A write's result: what was sent (if anything) and what the driver reports now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DriverWrite {
    pub outcome: WriteOutcome,
    pub message: String,
    pub call: Option<SetterCall>,
    pub read_back: DriverAnswer,
}

/// [`DriverWrite`] as the route answers it.
#[derive(Clone, Debug, Serialize)]
pub struct DriverWriteReport {
    pub device_id: String,
    pub outcome: WriteOutcome,
    pub message: String,
    pub call: Option<SetterCall>,
    pub read_back: DriverReport,
}

/// The call for a change, built from the driver's current settings: `None` when the driver already
/// has what is asked for. Refuses before anything is sent.
pub fn plan(settings: &DriverSettings, change: &DriverChange) -> Result<Option<SetterCall>, WriteRefusal> {
    if change.buffer_size.is_none() && change.safe_mode.is_none() {
        return Err(WriteRefusal::new(RefusalCode::NothingToChange, "Nothing to change: give a buffer size, Safe Mode or both."));
    }
    let asio = match &settings.asio {
        Reading::Read { value } => value,
        Reading::Unread { message } => {
            return Err(WriteRefusal::new(RefusalCode::Unreadable, format!("The driver's buffer settings could not be read, so nothing was sent. {message}")))
        }
    };
    if !asio::plausible_rate(asio.reference_rate) {
        return Err(WriteRefusal::new(
            RefusalCode::Unreadable,
            format!("The rate the driver's buffer applies at ({}) is not a plausible rate, so nothing was sent.", asio.reference_rate),
        ));
    }
    let preferred_size = change.buffer_size.unwrap_or(asio.buffer_size);
    if !asio.buffer_sizes.contains(&preferred_size) {
        let offered: Vec<String> = asio.buffer_sizes.iter().map(u32::to_string).collect();
        return Err(WriteRefusal::new(
            RefusalCode::NotOffered,
            format!("The driver does not offer a buffer of {preferred_size} samples; it offers {}.", offered.join(", ")),
        ));
    }
    let safe_mode = change.safe_mode.unwrap_or(asio.safe_mode);
    if preferred_size == asio.buffer_size && safe_mode == asio.safe_mode {
        return Ok(None);
    }
    if asio.asio_clients > 0 && !change.force {
        let who = if asio.asio_clients == 1 { "1 program is".to_string() } else { format!("{} programs are", asio.asio_clients) };
        return Err(WriteRefusal::new(
            RefusalCode::AsioInUse,
            format!("{who} using the driver's ASIO interface (a DAW, most likely), and changing the buffer or Safe Mode restarts its audio. Nothing was sent; ask again with force to change it anyway."),
        ));
    }
    Ok(Some(SetterCall { asio_instance: settings.asio_instance, reference_sample_rate: asio.reference_rate, preferred_size, options: asio::options_for(safe_mode) }))
}

/// Change a device's buffer size and/or Safe Mode: read, build the call from what the driver has
/// now, call the one setter, read back. A refusal means nothing was sent.
pub fn write_device(host: &dyn DriverHost, serial: &str, change: &DriverChange, dry_run: bool) -> Result<DriverWrite, WriteRefusal> {
    let settings = match read_device(host, serial) {
        DriverAnswer::Read(settings) => settings,
        DriverAnswer::NoDriver { message } | DriverAnswer::NotFound { message } => return Err(WriteRefusal::new(RefusalCode::Unavailable, message)),
        DriverAnswer::Failed { message } => return Err(WriteRefusal::new(RefusalCode::Unreadable, message)),
    };
    let Some(call) = plan(&settings, change)? else {
        return Ok(DriverWrite {
            outcome: WriteOutcome::Unchanged,
            message: "The driver already has these settings, so nothing was sent.".into(),
            call: None,
            read_back: DriverAnswer::Read(settings),
        });
    };
    if dry_run {
        return Ok(DriverWrite { outcome: WriteOutcome::DryRun, message: "A dry run: the call was built and not sent.".into(), call: Some(call), read_back: DriverAnswer::Read(settings) });
    }
    let api = host.load(Path::new(&settings.dll)).map_err(|e| WriteRefusal::new(RefusalCode::Unreadable, format!("The driver's API could not be loaded: {e}.")))?;
    let sent = api.set_asio_buffer_preferred_size(call.asio_instance, call.reference_sample_rate, call.preferred_size, call.options);
    let read_back = read_device(host, serial);
    let (outcome, message) = match (&sent, &read_back) {
        (Err(e), _) => (WriteOutcome::Failed, format!("The driver refused the change: {e}. What it reports now is shown.")),
        (Ok(()), DriverAnswer::Read(DriverSettings { asio: Reading::Read { value }, .. })) => compare(&call, value),
        (Ok(()), DriverAnswer::Read(DriverSettings { asio: Reading::Unread { message }, .. })) => {
            (WriteOutcome::Unconfirmed, format!("The driver took the change, and its settings could not be read back: {message}"))
        }
        (Ok(()), DriverAnswer::NoDriver { message } | DriverAnswer::NotFound { message } | DriverAnswer::Failed { message }) => {
            (WriteOutcome::Unconfirmed, format!("The driver took the change, and could not be read back: {message}"))
        }
    };
    Ok(DriverWrite { outcome, message, call: Some(call), read_back })
}

fn on_off(on: bool) -> &'static str {
    if on {
        "on"
    } else {
        "off"
    }
}

/// Whether what the driver reports is what was sent, said plainly when it is not.
fn compare(call: &SetterCall, now: &AsioInstance) -> (WriteOutcome, String) {
    let safe_mode = call.options & asio::SAFE_MODE_FLAG != 0;
    let mut differences = Vec::new();
    if now.buffer_size != call.preferred_size {
        differences.push(format!("a buffer of {} samples was sent and the driver reports {}", call.preferred_size, now.buffer_size));
    }
    if now.safe_mode != safe_mode {
        differences.push(format!("Safe Mode {} was sent and the driver reports it {}", on_off(safe_mode), on_off(now.safe_mode)));
    }
    if differences.is_empty() {
        (WriteOutcome::Applied, format!("The driver now reports a buffer of {} samples with Safe Mode {}.", now.buffer_size, on_off(now.safe_mode)))
    } else {
        (WriteOutcome::Mismatch, format!("The driver did not take the change as sent: {}.", differences.join("; ")))
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// Not Windows: the driver and its API are Windows-only here.
#[cfg(not(windows))]
struct Elsewhere;

#[cfg(not(windows))]
impl DriverHost for Elsewhere {
    fn find_api_dlls(&self) -> Result<Vec<PathBuf>, String> {
        Ok(Vec::new())
    }
    fn load(&self, _dll: &Path) -> Result<Arc<dyn DriverApi>, String> {
        Err("driver APIs are only read on Windows".into())
    }
    fn registry_u32(&self, _subkey: &str, _name: &str) -> Result<Option<u32>, String> {
        Ok(None)
    }
}

#[cfg(test)]
pub(crate) mod fake;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod write_tests;
