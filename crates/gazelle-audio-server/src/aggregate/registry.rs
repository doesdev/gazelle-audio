//! The audio drivers this PC publishes, read from `HKLM\SOFTWARE\ASIO`.
//!
//! Every driver of this kind puts a key there holding a class id, and the class id names a DLL
//! under `HKLM\SOFTWARE\Classes\CLSID\{...}\InprocServer32`. Reading is all that happens here:
//! nothing in Gazelle writes these keys, and registering the aggregate driver is a separate,
//! elevated act (`crate::aggregate::elevate`).
//!
//! The PC sits behind [`AsioRegistry`] so every rule above it is decided against a registry made
//! of a list, and no test reads a real one.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde::Serialize;

/// Where every driver of this kind registers itself.
pub const ASIO_KEY: &str = r"SOFTWARE\ASIO";

/// The Gazelle Aggregate driver's class id. It was generated once at random and never changes:
/// it is how a DAW, and every saved project that has chosen the driver, finds it again. The
/// driver crate declares the same id and has a test that fails if it is edited; this copy is
/// here so the server does not depend on the driver crate.
pub const AGGREGATE_CLSID: &str = "{F18C80B4-2DE1-43B8-AC84-19DDC22EBFC8}";

/// The registry key the aggregate driver registers itself under, which is what a DAW shows.
pub const AGGREGATE_NAME: &str = "Gazelle Aggregate";

/// One entry as the registry holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawEntry {
    /// The key's own name, which is what a DAW shows in its driver list.
    pub key: String,
    pub description: Option<String>,
    pub clsid: String,
    /// `InprocServer32`'s default value, or why it could not be read.
    pub dll: Result<String, String>,
}

/// One entry as the aggregate answer reports it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AsioEntry {
    pub key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub clsid: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dll: Option<String>,
    /// Why the DLL could not be read, when it could not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dll_error: Option<String>,
    /// Whether the DLL the class id names is a file on this PC now. A driver that is registered
    /// and points at a copy that has been moved or deleted is the case this catches.
    pub dll_present: bool,
    /// True when the aggregate's configuration names this entry.
    pub configured: bool,
    /// True when this is Gazelle's own aggregate driver.
    pub is_aggregate: bool,
}

/// Where the drivers are published, and whether a DLL is still on disk.
pub trait AsioRegistry: Send + Sync {
    /// Every entry this PC registers, in registry order. An empty list is an ordinary answer.
    fn entries(&self) -> Result<Vec<RawEntry>, String>;
    /// Whether the DLL at this path is a file now.
    fn dll_present(&self, dll: &str) -> bool;
}

/// Whether two class id strings name the same class, whatever their case or braces.
pub fn same_clsid(a: &str, b: &str) -> bool {
    fn bare(s: &str) -> String {
        s.trim().trim_start_matches('{').trim_end_matches('}').to_ascii_lowercase()
    }
    bare(a) == bare(b)
}

/// Whether an entry is the one a configured device names: by class id when it gives one, else by
/// its key, matched without case, whole or as a part, as the driver itself matches it.
pub fn entry_matches(entry: &RawEntry, key: Option<&str>, clsid: Option<&str>) -> bool {
    if let Some(clsid) = clsid.filter(|c| !c.trim().is_empty()) {
        return same_clsid(&entry.clsid, clsid);
    }
    match key.map(str::trim).filter(|k| !k.is_empty()) {
        Some(key) => entry.key.to_ascii_lowercase().contains(&key.to_ascii_lowercase()),
        None => false,
    }
}

/// Every entry, marked with whether the configuration names it and whether its DLL is there.
pub fn describe(registry: &dyn AsioRegistry, configured: &BTreeSet<String>) -> Result<Vec<AsioEntry>, String> {
    Ok(registry
        .entries()?
        .into_iter()
        .map(|entry| {
            let (dll, dll_error) = match &entry.dll {
                Ok(dll) => (Some(dll.clone()), None),
                Err(why) => (None, Some(why.clone())),
            };
            AsioEntry {
                dll_present: dll.as_deref().is_some_and(|dll| registry.dll_present(dll)),
                configured: configured.contains(&entry.key),
                is_aggregate: same_clsid(&entry.clsid, AGGREGATE_CLSID),
                key: entry.key,
                description: entry.description,
                clsid: entry.clsid,
                dll,
                dll_error,
            }
        })
        .collect())
}

/// This PC's registry on Windows; elsewhere, one that says there are no drivers.
pub fn for_this_pc() -> Arc<dyn AsioRegistry> {
    #[cfg(windows)]
    {
        Arc::new(windows::ThisPc)
    }
    #[cfg(not(windows))]
    {
        Arc::new(Elsewhere)
    }
}

/// Not Windows: drivers of this kind are a Windows thing, and there are none to list.
#[cfg(not(windows))]
struct Elsewhere;

#[cfg(not(windows))]
impl AsioRegistry for Elsewhere {
    fn entries(&self) -> Result<Vec<RawEntry>, String> {
        Ok(Vec::new())
    }
    fn dll_present(&self, _dll: &str) -> bool {
        false
    }
}

#[cfg(windows)]
pub mod windows {
    //! The real registry. Read only, and 64 bit view: the drivers are 64 bit and so are we.

    use std::ptr::{null, null_mut};

    use windows_sys::Win32::Foundation::{
        ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_NO_MORE_ITEMS, ERROR_PATH_NOT_FOUND, ERROR_SUCCESS,
    };
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegEnumKeyExW, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_64KEY,
    };

    use super::{AsioRegistry, RawEntry, ASIO_KEY};

    pub struct ThisPc;

    impl AsioRegistry for ThisPc {
        fn entries(&self) -> Result<Vec<RawEntry>, String> {
            entries()
        }
        fn dll_present(&self, dll: &str) -> bool {
            std::path::Path::new(dll).is_file()
        }
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn from_wide(chars: &[u16]) -> String {
        let len = chars.iter().position(|&c| c == 0).unwrap_or(chars.len());
        String::from_utf16_lossy(&chars[..len])
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

    fn open(subkey: &str) -> Result<OwnedKey, String> {
        let mut key: HKEY = null_mut();
        let status = unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, wide(subkey).as_ptr(), 0, KEY_READ | KEY_WOW64_64KEY, &mut key) };
        if status == ERROR_SUCCESS {
            Ok(OwnedKey(key))
        } else {
            Err(reg_error(subkey, status))
        }
    }

    /// A string value, or the key's default value when `name` is empty.
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
            let status = unsafe {
                RegEnumKeyExW(key.0, index, name.as_mut_ptr(), &mut len, null_mut(), null_mut(), null_mut(), null_mut())
            };
            if status == ERROR_NO_MORE_ITEMS || status != ERROR_SUCCESS {
                break;
            }
            names.push(from_wide(&name));
            index += 1;
        }
        names
    }

    /// The DLL a class id names, or why it could not be read.
    fn dll_for(clsid: &str) -> Result<String, String> {
        let key = open(&format!(r"SOFTWARE\Classes\CLSID\{clsid}\InprocServer32"))?;
        let dll = value(&key, "")?;
        if dll.is_empty() {
            Err("its InprocServer32 names no DLL".to_string())
        } else {
            Ok(dll)
        }
    }

    /// Every entry this PC registers, in registry order.
    pub fn entries() -> Result<Vec<RawEntry>, String> {
        // No key at all means no driver of this kind is installed, which is an answer, not a fault.
        let Ok(root) = open(ASIO_KEY) else { return Ok(Vec::new()) };
        let mut entries = Vec::new();
        for name in subkeys(&root) {
            let Ok(key) = open(&format!(r"{ASIO_KEY}\{name}")) else { continue };
            let Ok(clsid) = value(&key, "CLSID") else { continue };
            if clsid.is_empty() {
                continue;
            }
            let dll = dll_for(&clsid);
            entries.push(RawEntry { key: name, description: value(&key, "Description").ok().filter(|d| !d.is_empty()), clsid, dll });
        }
        Ok(entries)
    }
}

/// A registry made of a list, for tests.
#[derive(Default)]
pub struct FakeRegistry {
    pub entries: Vec<RawEntry>,
    /// The DLL paths that are files.
    pub present: BTreeSet<String>,
    /// When set, listing fails with this.
    pub failure: Option<String>,
}

impl FakeRegistry {
    /// An entry whose DLL is there.
    pub fn entry(key: &str, clsid: &str, dll: &str) -> RawEntry {
        RawEntry { key: key.into(), description: None, clsid: clsid.into(), dll: Ok(dll.into()) }
    }
}

impl AsioRegistry for FakeRegistry {
    fn entries(&self) -> Result<Vec<RawEntry>, String> {
        match &self.failure {
            Some(why) => Err(why.clone()),
            None => Ok(self.entries.clone()),
        }
    }
    fn dll_present(&self, dll: &str) -> bool {
        self.present.contains(dll)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_class_id_is_the_same_whatever_its_case_or_braces() {
        assert!(same_clsid("{AE4A4452-A316-11E5-A113-080027F6C1F4}", "ae4a4452-a316-11e5-a113-080027f6c1f4"));
        assert!(!same_clsid(AGGREGATE_CLSID, "{AE4A4452-A316-11E5-A113-080027F6C1F4}"));
    }

    #[test]
    fn a_device_names_an_entry_by_class_id_first_and_by_part_of_its_key() {
        let entry = FakeRegistry::entry("Zen Quadro Synergy Core", "{1221}", "q.dll");
        assert!(entry_matches(&entry, None, Some("{1221}")));
        assert!(entry_matches(&entry, Some("quadro"), None), "part of the key, without case");
        assert!(!entry_matches(&entry, Some("quadro"), Some("{9999}")), "a class id given is the one that decides");
        assert!(!entry_matches(&entry, None, None), "a device that names nothing matches nothing");
        assert!(!entry_matches(&entry, Some("  "), Some("  ")), "and neither does one that names blanks");
    }

    #[test]
    fn every_entry_is_described_with_its_dll_and_whether_the_configuration_names_it() {
        let registry = FakeRegistry {
            entries: vec![
                FakeRegistry::entry("Zen Quadro Synergy Core", "{1221}", r"C:\q.dll"),
                RawEntry { key: "Broken".into(), description: Some("d".into()), clsid: "{2}".into(), dll: Err("its InprocServer32 names no DLL".into()) },
                FakeRegistry::entry(AGGREGATE_NAME, AGGREGATE_CLSID, r"C:\gone.dll"),
            ],
            present: [r"C:\q.dll".to_string()].into_iter().collect(),
            failure: None,
        };
        let described = describe(&registry, &["Zen Quadro Synergy Core".to_string()].into_iter().collect()).unwrap();
        assert_eq!(described[0].dll.as_deref(), Some(r"C:\q.dll"));
        assert!(described[0].dll_present && described[0].configured && !described[0].is_aggregate);
        assert_eq!(described[1].dll_error.as_deref(), Some("its InprocServer32 names no DLL"));
        assert!(!described[1].dll_present && !described[1].configured);
        assert!(described[2].is_aggregate, "our own driver is marked");
        assert!(!described[2].dll_present, "registered, pointing at a copy that is not there");
    }
}
