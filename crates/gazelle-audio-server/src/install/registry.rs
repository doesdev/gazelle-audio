//! The registry, behind a trait, as [`crate::tray::boot::RunKey`] already does for the login
//! entry.
//!
//! Only `HKEY_CURRENT_USER` is ever touched: an Add/Remove Programs entry under
//! `HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall` is a per-user install, which is
//! the whole point of not needing UAC. Nothing here can reach `HKLM`.
//!
//! The trait exists so every rule about *which* values are written is tested against an
//! in-memory fake and no test writes a real key.

use std::io;

/// A registry hive the installer may write to: string and number values under a subkey, and
/// removing the subkey outright.
pub trait Registry {
    fn set_string(&self, subkey: &str, name: &str, value: &str) -> io::Result<()>;
    fn set_u32(&self, subkey: &str, name: &str, value: u32) -> io::Result<()>;
    fn get_string(&self, subkey: &str, name: &str) -> io::Result<Option<String>>;
    /// Remove the subkey and everything under it. Removing one that is not there is not an error.
    fn delete_tree(&self, subkey: &str) -> io::Result<()>;
}

/// `HKEY_CURRENT_USER` itself.
#[cfg(windows)]
pub struct CurrentUser;

#[cfg(windows)]
mod win {
    use super::*;
    use std::ptr::null_mut;
    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{
        RegDeleteTreeW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_DWORD, REG_SZ, RRF_RT_REG_SZ,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn done(code: u32) -> io::Result<()> {
        match code {
            ERROR_SUCCESS => Ok(()),
            e => Err(io::Error::from_raw_os_error(e as i32)),
        }
    }

    impl Registry for CurrentUser {
        fn set_string(&self, subkey: &str, name: &str, value: &str) -> io::Result<()> {
            let data = wide(value);
            done(unsafe {
                RegSetKeyValueW(
                    HKEY_CURRENT_USER,
                    wide(subkey).as_ptr(),
                    wide(name).as_ptr(),
                    REG_SZ,
                    data.as_ptr().cast(),
                    (data.len() * 2) as u32,
                )
            })
        }

        fn set_u32(&self, subkey: &str, name: &str, value: u32) -> io::Result<()> {
            done(unsafe {
                RegSetKeyValueW(
                    HKEY_CURRENT_USER,
                    wide(subkey).as_ptr(),
                    wide(name).as_ptr(),
                    REG_DWORD,
                    std::ptr::addr_of!(value).cast(),
                    4,
                )
            })
        }

        fn get_string(&self, subkey: &str, name: &str) -> io::Result<Option<String>> {
            let (subkey, name) = (wide(subkey), wide(name));
            let mut bytes = 0u32;
            let query = |data: *mut u16, bytes: &mut u32| unsafe {
                RegGetValueW(HKEY_CURRENT_USER, subkey.as_ptr(), name.as_ptr(), RRF_RT_REG_SZ, null_mut(), data.cast(), bytes)
            };
            match query(null_mut(), &mut bytes) {
                ERROR_SUCCESS => {}
                ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND => return Ok(None),
                e => return Err(io::Error::from_raw_os_error(e as i32)),
            }
            let mut buf = vec![0u16; (bytes as usize).div_ceil(2)];
            match query(buf.as_mut_ptr(), &mut bytes) {
                ERROR_SUCCESS => {}
                ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND => return Ok(None),
                e => return Err(io::Error::from_raw_os_error(e as i32)),
            }
            let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            Ok(Some(String::from_utf16_lossy(&buf[..len])))
        }

        fn delete_tree(&self, subkey: &str) -> io::Result<()> {
            match unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, wide(subkey).as_ptr()) } {
                ERROR_SUCCESS | ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND => Ok(()),
                e => Err(io::Error::from_raw_os_error(e as i32)),
            }
        }
    }
}
