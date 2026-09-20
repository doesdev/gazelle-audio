//! Where drivers of this kind publish themselves on a PC, and how to read that.
//!
//! Every driver puts a key under `HKLM\SOFTWARE\ASIO` holding a class id, and the class id names
//! a DLL under `HKLM\SOFTWARE\Classes\CLSID\{...}\InprocServer32`. Reading is all that happens
//! here; the aggregate driver's own registration, which writes, keeps its writes in its own crate
//! behind a trait so they can be tested without a registry.

use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{
    ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_NO_MORE_ITEMS, ERROR_PATH_NOT_FOUND, ERROR_SUCCESS,
};
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegEnumKeyExW, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_64KEY,
};

/// Where every driver of this kind registers itself.
pub const ASIO_KEY: &str = r"SOFTWARE\ASIO";

pub use crate::entry::Entry;

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn from_wide(chars: &[u16]) -> String {
    let len = chars.iter().position(|&c| c == 0).unwrap_or(chars.len());
    String::from_utf16_lossy(&chars[..len])
}

pub fn reg_error(what: &str, status: u32) -> String {
    match status {
        ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND => format!("{what} is not there"),
        other => format!("{what}: {}", std::io::Error::from_raw_os_error(other as i32)),
    }
}

/// A key that closes itself.
pub struct OwnedKey(pub HKEY);

impl Drop for OwnedKey {
    fn drop(&mut self) {
        unsafe { RegCloseKey(self.0) };
    }
}

/// # Safety
/// `root` must be a live key, or one of the predefined roots.
pub unsafe fn open_key(root: HKEY, subkey: &str) -> Result<OwnedKey, String> {
    let mut key: HKEY = null_mut();
    let status = unsafe { RegOpenKeyExW(root, wide(subkey).as_ptr(), 0, KEY_READ | KEY_WOW64_64KEY, &mut key) };
    if status == ERROR_SUCCESS {
        Ok(OwnedKey(key))
    } else {
        Err(reg_error(subkey, status))
    }
}

/// A string value, or the key's default value when `name` is empty.
pub fn value(key: &OwnedKey, name: &str) -> Result<String, String> {
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

pub fn subkeys(key: &OwnedKey) -> Vec<String> {
    let mut names = Vec::new();
    let mut index = 0u32;
    loop {
        let mut name = [0u16; 512];
        let mut len = name.len() as u32;
        let status =
            unsafe { RegEnumKeyExW(key.0, index, name.as_mut_ptr(), &mut len, null_mut(), null_mut(), null_mut(), null_mut()) };
        if status == ERROR_NO_MORE_ITEMS || status != ERROR_SUCCESS {
            break;
        }
        names.push(from_wide(&name));
        index += 1;
    }
    names
}

/// The DLL a class id names, or why it could not be read.
pub fn dll_for(clsid: &str) -> Result<String, String> {
    let subkey = format!(r"SOFTWARE\Classes\CLSID\{clsid}\InprocServer32");
    let key = unsafe { open_key(HKEY_LOCAL_MACHINE, &subkey) }?;
    let dll = value(&key, "")?;
    if dll.is_empty() {
        Err("its InprocServer32 names no DLL".to_string())
    } else {
        Ok(dll)
    }
}

/// Every entry this PC registers, in registry order. Read only.
pub fn entries() -> Result<Vec<Entry>, String> {
    let root = match unsafe { open_key(HKEY_LOCAL_MACHINE, ASIO_KEY) } {
        Ok(key) => key,
        Err(_) => return Ok(Vec::new()),
    };
    let mut entries = Vec::new();
    for name in subkeys(&root) {
        let key = match unsafe { open_key(HKEY_LOCAL_MACHINE, &format!(r"{ASIO_KEY}\{name}")) } {
            Ok(key) => key,
            Err(_) => continue,
        };
        let Ok(clsid) = value(&key, "CLSID") else { continue };
        if clsid.is_empty() {
            continue;
        }
        let dll = dll_for(&clsid);
        entries.push(Entry { key: name, description: value(&key, "Description").ok().filter(|d| !d.is_empty()), clsid, dll });
    }
    Ok(entries)
}
