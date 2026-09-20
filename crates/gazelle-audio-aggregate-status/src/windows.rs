//! The real shared section and the real event, on Windows.
//!
//! This is the only file in the crate that cannot be exercised without Windows, so it is kept as
//! thin as it can be: it makes a mapping and an event and decides nothing. Everything that decides
//! anything is above it and runs against [`crate::map::Scratch`].
//!
//! Nothing here is ever a reason to fail. A driver that cannot make its section carries on exactly
//! as it would with Gazelle closed, which is the state it has to work in anyway.

use std::ptr::null;

use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, MapViewOfFile, OpenFileMappingW, UnmapViewOfFile, FILE_MAP_ALL_ACCESS, PAGE_READWRITE,
};
use windows_sys::Win32::System::Threading::{CreateEventW, OpenEventW, SetEvent, WaitForSingleObject, EVENT_MODIFY_STATE};

use crate::map::{Mapping, Notifier, Wake, Waiter};
use crate::names;
use crate::record::RECORD_BYTES;

/// The right to wait on a handle. Written out here because it lives in a part of the API this
/// crate does not otherwise use.
const SYNCHRONIZE: u32 = 0x0010_0000;

/// A named shared section, mapped into this process.
pub struct Section {
    handle: HANDLE,
    view: *mut u8,
    bytes: usize,
    created: bool,
}

impl Section {
    /// Make the section, or take the one that is already there. The driver calls this.
    pub fn create() -> Result<Section, String> {
        Section::open_or_make(true)
    }

    /// Open a section somebody else made. Gazelle calls this, and gets a plain refusal when the
    /// driver is not running.
    pub fn open() -> Result<Section, String> {
        Section::open_or_make(false)
    }

    fn open_or_make(make: bool) -> Result<Section, String> {
        let name = names::wide(&names::section_name());
        let bytes = RECORD_BYTES;
        let (handle, created) = if make {
            // Safety: a named mapping backed by the page file, of a size this process chose.
            let handle =
                unsafe { CreateFileMappingW(INVALID_HANDLE_VALUE, null(), PAGE_READWRITE, 0, bytes as u32, name.as_ptr()) };
            if handle.is_null() {
                return Err(format!("the status section could not be made ({})", last_error()));
            }
            let existed = last_error() == ERROR_ALREADY_EXISTS;
            (handle, !existed)
        } else {
            let handle = unsafe { OpenFileMappingW(FILE_MAP_ALL_ACCESS, 0, name.as_ptr()) };
            if handle.is_null() {
                return Err("the aggregate driver is not publishing a status section".to_string());
            }
            (handle, false)
        };
        let view = unsafe { MapViewOfFile(handle, FILE_MAP_ALL_ACCESS, 0, 0, bytes) };
        if view.Value.is_null() {
            unsafe { CloseHandle(handle) };
            return Err(format!("the status section could not be mapped ({})", last_error()));
        }
        Ok(Section { handle, view: view.Value.cast(), bytes, created })
    }
}

impl Drop for Section {
    fn drop(&mut self) {
        // Safety: both were made here and neither has been released.
        unsafe {
            UnmapViewOfFile(windows_sys::Win32::System::Memory::MEMORY_MAPPED_VIEW_ADDRESS { Value: self.view.cast() });
            CloseHandle(self.handle);
        }
    }
}

// The view is the same address for the life of the mapping, so sharing it between threads is
// sharing a constant; what the bytes behind it mean is the seqlock's business, not the handle's.
unsafe impl Send for Section {}
unsafe impl Sync for Section {}

// A mapped view is the same address for the life of the mapping, and a page from the page file is
// aligned far past anything the record asks for.
unsafe impl Mapping for Section {
    fn base(&self) -> *mut u8 {
        self.view
    }

    fn bytes(&self) -> usize {
        self.bytes
    }

    fn created(&self) -> bool {
        self.created
    }
}

/// The named event Gazelle signals and the driver's watcher waits on.
pub struct ReloadEvent {
    handle: HANDLE,
}

// The handle is used from several threads, which is what it is for.
unsafe impl Send for ReloadEvent {}
unsafe impl Sync for ReloadEvent {}

impl ReloadEvent {
    /// Make the event, or take the one that is there. Automatic reset, so one signal wakes one
    /// wait and the flag does not stay set behind it.
    pub fn create() -> Result<ReloadEvent, String> {
        let name = names::wide(&names::reload_event_name());
        // Safety: a named automatic reset event, initially clear.
        let handle = unsafe { CreateEventW(null(), 0, 0, name.as_ptr()) };
        if handle.is_null() {
            return Err(format!("the reload event could not be made ({})", last_error()));
        }
        Ok(ReloadEvent { handle })
    }

    /// Open the event the driver made. Gazelle calls this.
    pub fn open() -> Result<ReloadEvent, String> {
        let name = names::wide(&names::reload_event_name());
        let handle = unsafe { OpenEventW(EVENT_MODIFY_STATE | SYNCHRONIZE, 0, name.as_ptr()) };
        if handle.is_null() {
            return Err("the aggregate driver is not listening for changes".to_string());
        }
        Ok(ReloadEvent { handle })
    }
}

impl Drop for ReloadEvent {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.handle) };
    }
}

impl Notifier for ReloadEvent {
    fn signal(&self) {
        unsafe { SetEvent(self.handle) };
    }
}

impl Waiter for ReloadEvent {
    fn wait(&self, millis: u32) -> Wake {
        // Safety: the handle is ours until this object is dropped, and nothing drops it while a
        // wait is in flight: the thread that waits is joined first.
        match unsafe { WaitForSingleObject(self.handle, millis) } {
            WAIT_OBJECT_0 => Wake::Signalled,
            WAIT_TIMEOUT => Wake::TimedOut,
            _ => Wake::Closed,
        }
    }
}

fn last_error() -> u32 {
    unsafe { windows_sys::Win32::Foundation::GetLastError() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::publish::Publisher;
    use crate::read::Reader;

    // These make a real named section and a real named event, which costs a page of memory and a
    // handle and touches no device, no file and no registry.

    #[test]
    fn a_real_section_carries_the_record_between_two_ends_of_it() {
        let Ok(section) = Section::create() else { return };
        if !section.created() {
            // Something else on this machine already has the section open, which on a developer's
            // PC means a driver is loaded in a DAW. Leave it alone.
            return;
        }
        let publisher = Publisher::map(Box::new(section)).expect("our own section");
        let Ok(opened) = Section::open() else { panic!("the section we just made is there") };
        let reader = Reader::map(Box::new(opened)).expect("the header is ours");
        publisher.update(|area| {
            area.device_count = 2;
            area.buffer_size = 512;
        });
        let seen = reader.read().expect("a settled record");
        assert_eq!(seen.driver.device_count, 2);
        assert_eq!(seen.driver.buffer_size, 512);
        // And the other way: Gazelle asks, the driver sees it.
        let asked = reader.request(42);
        assert_eq!(publisher.generation(), asked);
    }

    #[test]
    fn a_real_event_wakes_a_wait_and_times_out_when_nobody_signals() {
        let Ok(event) = ReloadEvent::create() else { return };
        assert_eq!(event.wait(0), Wake::TimedOut, "nothing has happened yet");
        event.signal();
        assert_eq!(event.wait(0), Wake::Signalled);
        assert_eq!(event.wait(0), Wake::TimedOut, "one signal, one wake");
    }
}
