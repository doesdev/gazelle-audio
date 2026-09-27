//! What the recorder asks of the PC it runs on: how much memory is free, how much room a disk has,
//! creating files that are never already there, and what the local time is.
//!
//! Each is a trait, so every rule in this crate is tested with a PC made of data: a disk that fills
//! up after so many bytes, a machine with 100 MB free, a clock stopped at 23:59:59.

use std::io::{self, Seek, Write};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// How much physical memory is free now, in bytes.
pub trait Memory: Send + Sync {
    /// `None` when the PC will not say, which refuses Arm: sizing the pre-roll from nothing would be
    /// a guess about the one resource it is sized from.
    fn available(&self) -> Option<u64>;
}

/// This PC's memory, from `GlobalMemoryStatusEx`: `ullAvailPhys`, the physical memory that can be
/// had without taking any from what programs are using now.
pub struct ThisPcMemory;

impl Memory for ThisPcMemory {
    #[cfg(windows)]
    fn available(&self) -> Option<u64> {
        use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
        // Safety: a plain structure the call fills in, with its length set as the call requires.
        let mut status: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
        status.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        let ok = unsafe { GlobalMemoryStatusEx(&mut status) };
        (ok != 0).then_some(status.ullAvailPhys)
    }

    #[cfg(not(windows))]
    fn available(&self) -> Option<u64> {
        None
    }
}

/// A file being written: anything that writes and seeks.
pub trait FileOut: Write + Seek + Send {}
impl<T: Write + Seek + Send> FileOut for T {}

/// Where a take's files go.
pub trait Disk: Send + Sync {
    fn create_dir_all(&self, dir: &Path) -> io::Result<()>;
    fn exists(&self, path: &Path) -> bool;
    /// A new file, **never one that is already there**: a name that is taken is an error, not a
    /// file overwritten.
    fn create_new(&self, path: &Path) -> io::Result<Box<dyn FileOut>>;
    /// Bytes free on the disk `dir` is on, for this user. `None` when it cannot be read.
    fn free_bytes(&self, dir: &Path) -> Option<u64>;
}

/// How much a file is buffered before it reaches the disk. A second of 24-bit audio at 96 kHz is
/// 288 kB, so this is a few writes a second per file.
const FILE_BUFFER: usize = 1 << 20;

/// This PC's disks.
pub struct ThisPcDisk;

impl Disk for ThisPcDisk {
    fn create_dir_all(&self, dir: &Path) -> io::Result<()> {
        std::fs::create_dir_all(dir)
    }

    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn create_new(&self, path: &Path) -> io::Result<Box<dyn FileOut>> {
        let file = std::fs::OpenOptions::new().write(true).create_new(true).open(path)?;
        Ok(Box::new(io::BufWriter::with_capacity(FILE_BUFFER, file)))
    }

    #[cfg(windows)]
    fn free_bytes(&self, dir: &Path) -> Option<u64> {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
        // The folder may not be there yet: ask about the nearest one that is.
        let mut at = Some(dir);
        while let Some(path) = at {
            if path.exists() {
                break;
            }
            at = path.parent();
        }
        let wide: Vec<u16> = at?.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
        let mut free: u64 = 0;
        // Safety: a NUL-terminated path and three out parameters, the two we do not want left null.
        let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, std::ptr::null_mut(), std::ptr::null_mut()) };
        (ok != 0).then_some(free)
    }

    #[cfg(not(windows))]
    fn free_bytes(&self, _dir: &Path) -> Option<u64> {
        None
    }
}

/// A moment in the local calendar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Civil {
    pub year: u32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub nanos: u32,
}

impl Civil {
    /// `yyyy-mm-dd`, as `bext` and a file name write a date.
    pub fn date(&self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }

    /// `hh:mm:ss`, as `bext` writes a time.
    pub fn time(&self) -> String {
        format!("{:02}:{:02}:{:02}", self.hour, self.minute, self.second)
    }

    /// Seconds since this day's midnight, with the fraction.
    pub fn since_midnight(&self) -> f64 {
        f64::from(self.hour * 3600 + self.minute * 60 + self.second) + f64::from(self.nanos) / 1e9
    }

    /// The same moment in UTC, from a Unix time: what a PC with no time zone to ask says.
    pub fn utc(at: SystemTime) -> Civil {
        let since = at.duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO);
        let seconds = since.as_secs();
        let days = (seconds / 86_400) as i64;
        let of_day = (seconds % 86_400) as u32;
        let (year, month, day) = civil_from_days(days);
        Civil { year, month, day, hour: of_day / 3600, minute: of_day % 3600 / 60, second: of_day % 60, nanos: since.subsec_nanos() }
    }
}

/// The calendar date of a count of days since 1970-01-01 (Howard Hinnant's `civil_from_days`).
fn civil_from_days(days: i64) -> (u32, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as u32, m as u32, d as u32)
}

/// What time it is, and what that is in the local calendar.
pub trait Clock: Send + Sync {
    fn now(&self) -> SystemTime;
    fn civil(&self, at: SystemTime) -> Civil;
}

/// This PC's clock and time zone.
pub struct LocalClock;

impl Clock for LocalClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }

    #[cfg(windows)]
    fn civil(&self, at: SystemTime) -> Civil {
        use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
        use windows_sys::Win32::Storage::FileSystem::FileTimeToLocalFileTime;
        use windows_sys::Win32::System::Time::FileTimeToSystemTime;
        // FILETIME counts 100 ns steps since 1601-01-01.
        const FROM_1601: u64 = 11_644_473_600;
        let since = at.duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO);
        let ticks = (since.as_secs() + FROM_1601) * 10_000_000 + u64::from(since.subsec_nanos() / 100);
        let utc = FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 };
        let mut local = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        // Safety: plain structures in and out.
        let mut civil: SYSTEMTIME = unsafe { std::mem::zeroed() };
        let ok = unsafe { FileTimeToLocalFileTime(&utc, &mut local) != 0 && FileTimeToSystemTime(&local, &mut civil) != 0 };
        if !ok {
            return Civil::utc(at);
        }
        Civil {
            year: u32::from(civil.wYear),
            month: u32::from(civil.wMonth),
            day: u32::from(civil.wDay),
            hour: u32::from(civil.wHour),
            minute: u32::from(civil.wMinute),
            second: u32::from(civil.wSecond),
            nanos: since.subsec_nanos(),
        }
    }

    #[cfg(not(windows))]
    fn civil(&self, at: SystemTime) -> Civil {
        Civil::utc(at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_unix_time_is_the_calendar_date_it_is() {
        // 2026-09-27 14:03:21.5 UTC.
        let at = UNIX_EPOCH + Duration::from_millis(1_790_517_801_500);
        let civil = Civil::utc(at);
        assert_eq!((civil.year, civil.month, civil.day), (2026, 9, 27));
        assert_eq!((civil.hour, civil.minute, civil.second), (14, 3, 21));
        assert_eq!(civil.date(), "2026-09-27");
        assert_eq!(civil.time(), "14:03:21");
        assert_eq!(civil.since_midnight(), 50_601.5);
        assert_eq!(Civil::utc(UNIX_EPOCH).date(), "1970-01-01");
        // A leap day.
        assert_eq!(Civil::utc(UNIX_EPOCH + Duration::from_secs(1_709_164_800)).date(), "2024-02-29");
    }

    #[test]
    fn the_local_clock_gives_a_real_date() {
        let civil = LocalClock.civil(SystemTime::now());
        assert!(civil.year >= 2024 && (1..=12).contains(&civil.month) && (1..=31).contains(&civil.day), "{civil:?}");
        assert!(civil.hour < 24 && civil.minute < 60 && civil.second < 61);
    }

    #[test]
    fn this_pc_says_how_much_memory_is_free_and_how_much_room_a_disk_has() {
        if cfg!(windows) {
            assert!(ThisPcMemory.available().is_some_and(|free| free > 0));
            let missing = std::env::temp_dir().join("gazelle-record-not-made-yet").join("deeper");
            assert!(ThisPcDisk.free_bytes(&missing).is_some_and(|free| free > 0), "a folder not made yet is asked about through its nearest parent");
        }
    }

    #[test]
    fn a_new_file_is_never_one_that_was_already_there() {
        let path = std::env::temp_dir().join(format!("gazelle-record-new-{}.wav", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let mut first = ThisPcDisk.create_new(&path).expect("a new name");
        first.write_all(b"kept").unwrap();
        first.flush().unwrap();
        drop(first);
        let second = ThisPcDisk.create_new(&path);
        assert_eq!(second.err().map(|e| e.kind()), Some(io::ErrorKind::AlreadyExists));
        assert_eq!(std::fs::read(&path).unwrap(), b"kept", "and what was there is untouched");
        let _ = std::fs::remove_file(&path);
    }
}
