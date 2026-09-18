//! The real PC: finding each driver's API DLL, loading it, and calling its read functions.
//!
//! Plain `windows-sys`, as the tray and the installer use it, so reading the driver adds no crate.
//!
//! **Finding.** Every entry under `HKLM\...\Uninstall` (both registry views) that names an
//! `InstallLocation` is looked in, one folder deep, for a `*api_x64.dll` whose version resource
//! describes it as "TUSBAudio API DLL". Nothing is assumed about vendor or folder names, and the
//! DLL is never looked for on the search path: it is not in System32, and a DLL of that name
//! anywhere else is not the driver's.
//!
//! **Loading.** `LoadLibraryExW` with an absolute path and `LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR |
//! LOAD_LIBRARY_SEARCH_DEFAULT_DIRS`, so the DLL's own dependencies resolve from its folder and
//! System32 and never from the current directory. Each DLL is loaded once and **never freed**:
//! the API may keep threads or state of its own, and unloading a library under them is the one
//! way a read could bring the server down. Only [`READ_EXPORTS`] and the one setter in
//! [`WRITE_EXPORTS`] are ever resolved.
//!
//! **Calling.** Every structure the DLL fills is given a buffer several times the size the
//! reference measured, zeroed, so a newer driver that writes more than we expect writes into our
//! slack rather than past it. What is read out of it is then checked (`asio::parse`).

use std::collections::HashMap;
use std::ffi::{c_char, c_void, CStr, OsStr};
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::sync::{Arc, Mutex};

use windows_sys::Win32::Foundation::{
    GetLastError, ERROR_FILE_NOT_FOUND, ERROR_NO_MORE_ITEMS, ERROR_PATH_NOT_FOUND, ERROR_SUCCESS, HMODULE,
};
use windows_sys::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
use windows_sys::Win32::System::LibraryLoader::{
    GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_DEFAULT_DIRS, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR,
};
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW, HKEY, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY,
    KEY_WOW64_64KEY, RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
};

use super::{asio, CallError, DeviceProperties, DriverApi, DriverHost, DriverInfo, Handle, READ_EXPORTS, WRITE_EXPORTS};

const UNINSTALL: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall";
const DESCRIPTION: &str = "TUSBAudio API DLL";
const SUFFIX: &str = "api_x64.dll";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn wide_path(path: &Path) -> Vec<u16> {
    OsStr::new(path).encode_wide().chain(std::iter::once(0)).collect()
}

fn from_wide(chars: &[u16]) -> String {
    let len = chars.iter().position(|&c| c == 0).unwrap_or(chars.len());
    String::from_utf16_lossy(&chars[..len])
}

/// This PC's drivers, each DLL loaded at most once for the life of the server.
#[derive(Default)]
pub struct ThisPc {
    loaded: Mutex<HashMap<PathBuf, Arc<Library>>>,
}

impl DriverHost for ThisPc {
    fn find_api_dlls(&self) -> Result<Vec<PathBuf>, String> {
        let mut found: Vec<PathBuf> = Vec::new();
        for location in install_locations()? {
            for dll in candidates(&location) {
                let seen = found.iter().any(|f| f.as_os_str().eq_ignore_ascii_case(dll.as_os_str()));
                if !seen && description(&dll).is_some_and(|d| d.trim() == DESCRIPTION) {
                    found.push(dll);
                }
            }
        }
        found.sort();
        Ok(found)
    }

    fn load(&self, dll: &Path) -> Result<Arc<dyn DriverApi>, String> {
        let mut loaded = self.loaded.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(library) = loaded.get(dll) {
            return Ok(library.clone());
        }
        let library = Arc::new(Library::load(dll)?);
        loaded.insert(dll.to_path_buf(), library.clone());
        Ok(library)
    }

    fn registry_u32(&self, subkey: &str, name: &str) -> Result<Option<u32>, String> {
        let mut value = 0u32;
        let mut size = 4u32;
        let status = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                wide(subkey).as_ptr(),
                wide(name).as_ptr(),
                RRF_RT_REG_DWORD,
                null_mut(),
                std::ptr::addr_of_mut!(value).cast(),
                &mut size,
            )
        };
        match status {
            ERROR_SUCCESS => Ok(Some(value)),
            ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND => Ok(None),
            e => Err(std::io::Error::from_raw_os_error(e as i32).to_string()),
        }
    }
}

/// Every `InstallLocation` named under the uninstall key, in both registry views.
fn install_locations() -> Result<Vec<PathBuf>, String> {
    let mut locations = Vec::new();
    let mut opened = false;
    for view in [KEY_WOW64_64KEY, KEY_WOW64_32KEY] {
        let mut key: HKEY = null_mut();
        if unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, wide(UNINSTALL).as_ptr(), 0, KEY_READ | view, &mut key) } != ERROR_SUCCESS {
            continue;
        }
        opened = true;
        // Bounded, so a registry that keeps answering something odd cannot keep us here.
        for index in 0..100_000u32 {
            let mut name = [0u16; 256];
            let mut len = name.len() as u32;
            let status = unsafe {
                RegEnumKeyExW(key, index, name.as_mut_ptr(), &mut len, null(), null_mut(), null_mut(), null_mut())
            };
            if status == ERROR_NO_MORE_ITEMS {
                break;
            }
            if status != ERROR_SUCCESS {
                continue;
            }
            let entry = from_wide(&name[..len as usize]);
            if let Some(location) = string_value(key, &entry, "InstallLocation") {
                let location = location.trim().trim_matches('"');
                if !location.is_empty() {
                    locations.push(PathBuf::from(location));
                }
            }
        }
        unsafe { RegCloseKey(key) };
    }
    if opened {
        Ok(locations)
    } else {
        Err("the list of installed programs could not be opened".into())
    }
}

fn string_value(key: HKEY, subkey: &str, name: &str) -> Option<String> {
    let (subkey, name) = (wide(subkey), wide(name));
    let mut bytes = 0u32;
    let query = |data: *mut u16, bytes: &mut u32| unsafe {
        RegGetValueW(key, subkey.as_ptr(), name.as_ptr(), RRF_RT_REG_SZ, null_mut(), data.cast(), bytes)
    };
    if query(null_mut(), &mut bytes) != ERROR_SUCCESS || bytes > 64 * 1024 {
        return None;
    }
    let mut buf = vec![0u16; (bytes as usize).div_ceil(2) + 1];
    let mut size = (buf.len() * 2) as u32;
    (query(buf.as_mut_ptr(), &mut size) == ERROR_SUCCESS).then(|| from_wide(&buf))
}

/// `*api_x64.dll` in a folder and the folders directly inside it.
fn candidates(location: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(location) else { return out };
    for entry in entries.flatten() {
        let path = entry.path();
        match entry.file_type() {
            Ok(t) if t.is_dir() => {
                if let Ok(inner) = std::fs::read_dir(&path) {
                    out.extend(inner.flatten().map(|e| e.path()).filter(|p| is_candidate(p)));
                }
            }
            Ok(t) if t.is_file() && is_candidate(&path) => out.push(path),
            _ => {}
        }
    }
    out
}

fn is_candidate(path: &Path) -> bool {
    path.file_name().and_then(OsStr::to_str).is_some_and(|name| {
        let name = name.to_ascii_lowercase();
        name.len() > SUFFIX.len() && name.ends_with(SUFFIX)
    })
}

/// The file's version-resource `FileDescription`, in whichever language it carries.
fn description(path: &Path) -> Option<String> {
    let file = wide_path(path);
    let size = unsafe { GetFileVersionInfoSizeW(file.as_ptr(), null_mut()) };
    if size == 0 || size > 1 << 20 {
        return None;
    }
    let mut block = vec![0u8; size as usize];
    if unsafe { GetFileVersionInfoW(file.as_ptr(), 0, size, block.as_mut_ptr().cast()) } == 0 {
        return None;
    }
    let query = |sub: &str| -> Option<&[u16]> {
        let mut ptr: *mut c_void = null_mut();
        let mut len = 0u32;
        let ok = unsafe { VerQueryValueW(block.as_ptr().cast(), wide(sub).as_ptr(), &mut ptr, &mut len) };
        (ok != 0 && !ptr.is_null() && len > 0).then(|| unsafe { std::slice::from_raw_parts(ptr as *const u16, len as usize) })
    };
    let mut languages: Vec<(u16, u16)> = query(r"\VarFileInfo\Translation")
        .map(|words| words.as_chunks::<2>().0.iter().map(|&[lang, page]| (lang, page)).collect())
        .unwrap_or_default();
    languages.extend([(0x0409, 0x04b0), (0x0409, 0x04e4)]);
    languages.iter().find_map(|(lang, page)| {
        query(&format!(r"\StringFileInfo\{lang:04x}{page:04x}\FileDescription")).map(from_wide)
    })
}

/// One loaded API DLL and the read exports it has.
pub struct Library {
    #[allow(dead_code)]
    module: HMODULE,
    exports: HashMap<&'static str, unsafe extern "system" fn() -> isize>,
}

// SAFETY: the module handle is only used to resolve exports at load; the exports are plain
// functions the driver documents as callable from any thread, and every call here is made under
// `DriverService`'s one-at-a-time lock anyway.
unsafe impl Send for Library {}
unsafe impl Sync for Library {}

type Status = u32;

impl Library {
    fn load(dll: &Path) -> Result<Library, String> {
        if !dll.is_absolute() {
            return Err("its path is not absolute, so it was not loaded".into());
        }
        let module = unsafe {
            LoadLibraryExW(wide_path(dll).as_ptr(), null_mut(), LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS)
        };
        if module.is_null() {
            let error = std::io::Error::from_raw_os_error(unsafe { GetLastError() } as i32);
            return Err(format!("it could not be loaded: {error}"));
        }
        let mut exports = HashMap::new();
        for &name in READ_EXPORTS.iter().chain(WRITE_EXPORTS) {
            let symbol = std::ffi::CString::new(name).expect("export names have no NUL");
            if let Some(function) = unsafe { GetProcAddress(module, symbol.as_ptr().cast()) } {
                exports.insert(name, function);
            }
        }
        Ok(Library { module, exports })
    }

    fn export(&self, name: &'static str) -> Result<unsafe extern "system" fn() -> isize, CallError> {
        self.exports.get(name).copied().ok_or(CallError::Missing(name))
    }

    /// The driver's own words for a status, when its DLL can give them.
    fn status_text(&self, status: Status) -> String {
        let Ok(function) = self.export("TUSBAUDIO_StatusCodeStringA") else { return String::new() };
        let function: unsafe extern "system" fn(Status) -> *const c_char = unsafe { std::mem::transmute(function) };
        let text = unsafe { function(status) };
        if text.is_null() {
            return String::new();
        }
        unsafe { CStr::from_ptr(text) }.to_string_lossy().chars().take(120).collect()
    }

    fn check(&self, call: &'static str, status: Status) -> Result<(), CallError> {
        if status == 0 {
            Ok(())
        } else {
            Err(CallError::Status { call, code: status, text: self.status_text(status) })
        }
    }
}

// The signatures, from the public header and the reference (x64: "system" is the one convention).
type NoArgsU32 = unsafe extern "system" fn() -> u32;
type CheckApi = unsafe extern "system" fn(u32, u32) -> i32;
type FillOut = unsafe extern "system" fn(*mut c_void) -> Status;
type OpenByIndex = unsafe extern "system" fn(u32, *mut *mut c_void) -> Status;
type Close = unsafe extern "system" fn(*mut c_void) -> Status;
type DeviceFill = unsafe extern "system" fn(*mut c_void, *mut c_void) -> Status;
type IndexFill = unsafe extern "system" fn(u32, *mut c_void) -> Status;
/// `SetASIOBufferPreferredSize(asioInstance, referenceSampleRate, preferredSize, options)`.
type SetBuffer = unsafe extern "system" fn(u32, u32, u32, u32) -> Status;

/// Several times any structure the reference measured, in `u32`s so it is aligned for them.
const SLACK_WORDS: usize = 1024;

impl DriverApi for Library {
    fn api_version(&self) -> Result<u32, CallError> {
        let f: NoArgsU32 = unsafe { std::mem::transmute(self.export("TUSBAUDIO_GetApiVersion")?) };
        Ok(unsafe { f() })
    }

    fn check_api_version(&self, major: u32, minor: u32) -> Result<bool, CallError> {
        let f: CheckApi = unsafe { std::mem::transmute(self.export("TUSBAUDIO_CheckApiVersion")?) };
        Ok(unsafe { f(major, minor) } != 0)
    }

    fn driver_info(&self) -> Result<DriverInfo, CallError> {
        let f: FillOut = unsafe { std::mem::transmute(self.export("TUSBAUDIO_GetDriverInfo")?) };
        let mut words = vec![0u32; SLACK_WORDS];
        self.check("TUSBAUDIO_GetDriverInfo", unsafe { f(words.as_mut_ptr().cast()) })?;
        Ok(DriverInfo {
            api_major: words[0],
            api_minor: words[1],
            driver_major: words[2],
            driver_minor: words[3],
            driver_sub: words[4],
            flags: words[5],
        })
    }

    fn enumerate_devices(&self) -> Result<(), CallError> {
        let f: unsafe extern "system" fn() -> Status = unsafe { std::mem::transmute(self.export("TUSBAUDIO_EnumerateDevices")?) };
        self.check("TUSBAUDIO_EnumerateDevices", unsafe { f() })
    }

    fn device_count(&self) -> Result<u32, CallError> {
        let f: NoArgsU32 = unsafe { std::mem::transmute(self.export("TUSBAUDIO_GetDeviceCount")?) };
        Ok(unsafe { f() })
    }

    fn open_device(&self, index: u32) -> Result<Handle, CallError> {
        let f: OpenByIndex = unsafe { std::mem::transmute(self.export("TUSBAUDIO_OpenDeviceByIndex")?) };
        let mut handle: *mut c_void = null_mut();
        self.check("TUSBAUDIO_OpenDeviceByIndex", unsafe { f(index, &mut handle) })?;
        Ok(handle as Handle)
    }

    fn close_device(&self, handle: Handle) {
        if let Ok(f) = self.export("TUSBAUDIO_CloseDevice") {
            let f: Close = unsafe { std::mem::transmute(f) };
            unsafe { f(handle as *mut c_void) };
        }
    }

    fn device_properties(&self, handle: Handle) -> Result<DeviceProperties, CallError> {
        let f: DeviceFill = unsafe { std::mem::transmute(self.export("TUSBAUDIO_GetDeviceProperties")?) };
        let mut words = vec![0u32; SLACK_WORDS];
        self.check("TUSBAUDIO_GetDeviceProperties", unsafe { f(handle as *mut c_void, words.as_mut_ptr().cast()) })?;
        // vid, pid, rev, then three WCHAR[128]: serial, manufacturer, product.
        let chars: Vec<u16> = words[3..].iter().flat_map(|w| [*w as u16, (*w >> 16) as u16]).collect();
        Ok(DeviceProperties {
            vid: words[0],
            pid: words[1],
            serial: from_wide(&chars[..128]),
            product: from_wide(&chars[256..384]),
        })
    }

    fn current_sample_rate(&self, handle: Handle) -> Result<u32, CallError> {
        let f: DeviceFill = unsafe { std::mem::transmute(self.export("TUSBAUDIO_GetCurrentSampleRate")?) };
        let mut words = vec![0u32; SLACK_WORDS];
        self.check("TUSBAUDIO_GetCurrentSampleRate", unsafe { f(handle as *mut c_void, words.as_mut_ptr().cast()) })?;
        Ok(words[0])
    }

    fn asio_instance_count(&self) -> Option<Result<u32, CallError>> {
        let f: FillOut = unsafe { std::mem::transmute(self.export("TUSBAUDIO_GetASIOInstanceCount").ok()?) };
        let mut words = vec![0u32; SLACK_WORDS];
        Some(self.check("TUSBAUDIO_GetASIOInstanceCount", unsafe { f(words.as_mut_ptr().cast()) }).map(|()| words[0]))
    }

    fn asio_instance_info(&self, index: u32) -> Result<Vec<u8>, CallError> {
        let f: IndexFill = unsafe { std::mem::transmute(self.export("TUSBAUDIO_GetASIOInstanceInfo")?) };
        let mut words = vec![0u32; SLACK_WORDS];
        self.check("TUSBAUDIO_GetASIOInstanceInfo", unsafe { f(index, words.as_mut_ptr().cast()) })?;
        Ok(words.iter().flat_map(|w| w.to_le_bytes()).take(asio::INFO_LEN).collect())
    }

    fn set_asio_buffer_preferred_size(&self, asio_instance: u32, reference_sample_rate: u32, preferred_size: u32, options: u32) -> Result<(), CallError> {
        let f: SetBuffer = unsafe { std::mem::transmute(self.export("TUSBAUDIO_SetASIOBufferPreferredSize")?) };
        self.check("TUSBAUDIO_SetASIOBufferPreferredSize", unsafe { f(asio_instance, reference_sample_rate, preferred_size, options) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_api_dlls_are_candidates() {
        assert!(is_candidate(Path::new(r"C:\d\x64\Zen_Quadro_Synergy_Coreapi_x64.dll")));
        assert!(is_candidate(Path::new(r"C:\d\W10_x64\ZENSTUDIOTBAPI_X64.DLL")));
        assert!(!is_candidate(Path::new(r"C:\d\x64\api_x64.dll")));
        assert!(!is_candidate(Path::new(r"C:\d\x86\ZenStudioTBapi.dll")));
        assert!(!is_candidate(Path::new(r"C:\d\x64\ZenStudioTBCpl.exe")));
    }

    #[test]
    fn a_folder_that_is_not_there_has_no_candidates() {
        assert!(candidates(Path::new(r"C:\no\such\folder\for\gazelle")).is_empty());
    }

    #[test]
    fn a_dll_that_is_not_there_does_not_load_and_says_so() {
        let error = Library::load(Path::new(r"C:\no\such\folder\for\gazelle\xapi_x64.dll")).err().unwrap();
        assert!(error.starts_with("it could not be loaded: "), "{error}");
        let error = Library::load(Path::new(r"relative\xapi_x64.dll")).err().unwrap();
        assert!(error.contains("not absolute"), "{error}");
    }

    #[test]
    fn a_file_with_no_version_resource_has_no_description() {
        let dir = std::env::temp_dir().join(format!("gazelle-driver-desc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("fakeapi_x64.dll");
        std::fs::write(&fake, b"not a DLL").unwrap();
        assert_eq!(description(&fake), None);
        assert_eq!(candidates(&dir), vec![fake]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_system_dlls_description_is_read() {
        let kernel32 = PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into())).join(r"System32\kernel32.dll");
        let found = description(&kernel32).expect("kernel32 has a version resource");
        assert!(found.contains("API"), "{found}");
    }

    /// The one live check: this PC's real drivers, through the same read path the route uses. Read
    /// only, and never run by `cargo test` on its own:
    /// `cargo test -p gazelle-audio-server --lib driver::windows::tests::live -- --ignored --nocapture`
    #[test]
    #[ignore = "reads this PC's real audio drivers; run it by name"]
    fn live_read_of_this_pcs_drivers() {
        let pc = ThisPc::default();
        let dlls = pc.find_api_dlls().expect("looking for drivers");
        println!("driver API DLLs found: {dlls:#?}");
        for dll in &dlls {
            let api = pc.load(dll).expect("loading");
            let version = api.api_version().expect("GetApiVersion");
            println!("{}: API {}.{}", dll.display(), version >> 16, version & 0xffff);
            api.enumerate_devices().expect("EnumerateDevices");
            let count = api.device_count().expect("GetDeviceCount");
            for index in 0..count {
                let handle = api.open_device(index).expect("OpenDeviceByIndex");
                let properties = api.device_properties(handle);
                api.close_device(handle);
                let properties = properties.expect("GetDeviceProperties");
                println!("  device {index}: {properties:?}");
                let answer = super::super::read_device(&pc, &properties.serial);
                println!("  read: {}", serde_json::to_string_pretty(&answer).unwrap());
            }
        }
    }

    #[test]
    fn a_missing_registry_value_is_none_not_an_error() {
        let pc = ThisPc::default();
        assert_eq!(pc.registry_u32(r"SYSTEM\CurrentControlSet\Services\NoSuchGazelleService", "AsioSafeMode"), Ok(None));
    }
}
