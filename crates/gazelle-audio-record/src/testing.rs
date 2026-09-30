//! A PC made of data for the tests: a disk of byte vectors that can fill up, and a stopped clock.

use std::collections::HashMap;
use std::io::{self, Cursor, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::system::{Civil, Clock, Disk, FileOut, Memory};

/// A disk made of data: files are byte vectors, free space is what the test says, and a write
/// can be made to fail after so many bytes.
#[derive(Default)]
pub struct MemoryDisk {
    pub files: Mutex<HashMap<PathBuf, Arc<Mutex<Vec<u8>>>>>,
    pub free: Mutex<Option<u64>>,
    pub fail_after: Mutex<Option<u64>>,
}

struct MemoryFile {
    bytes: Arc<Mutex<Vec<u8>>>,
    at: u64,
    budget: Option<u64>,
}

impl Write for MemoryFile {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if let Some(budget) = self.budget.as_mut() {
            if *budget < data.len() as u64 {
                return Err(io::Error::new(io::ErrorKind::StorageFull, "there is not enough space on the disk"));
            }
            *budget -= data.len() as u64;
        }
        let mut bytes = self.bytes.lock().unwrap();
        let mut cursor = Cursor::new(&mut *bytes);
        cursor.set_position(self.at);
        cursor.write_all(data)?;
        self.at = cursor.position();
        Ok(data.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Seek for MemoryFile {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let len = self.bytes.lock().unwrap().len() as u64;
        self.at = match to {
            SeekFrom::Start(at) => at,
            SeekFrom::End(by) => (len as i64 + by) as u64,
            SeekFrom::Current(by) => (self.at as i64 + by) as u64,
        };
        Ok(self.at)
    }
}

impl Disk for MemoryDisk {
    fn create_dir_all(&self, _dir: &Path) -> io::Result<()> {
        Ok(())
    }
    fn exists(&self, path: &Path) -> bool {
        self.files.lock().unwrap().contains_key(path)
    }
    fn create_new(&self, path: &Path) -> io::Result<Box<dyn FileOut>> {
        let mut files = self.files.lock().unwrap();
        if files.contains_key(path) {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, "already there"));
        }
        let bytes = Arc::new(Mutex::new(Vec::new()));
        files.insert(path.to_path_buf(), Arc::clone(&bytes));
        let budget = *self.fail_after.lock().unwrap();
        Ok(Box::new(MemoryFile { bytes, at: 0, budget }))
    }
    fn free_bytes(&self, _dir: &Path) -> Option<u64> {
        *self.free.lock().unwrap()
    }
    fn read_file(&self, path: &Path) -> io::Result<Vec<u8>> {
        self.files.lock().unwrap().get(path).map(|b| b.lock().unwrap().clone()).ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "not there"))
    }
    fn write_new(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        let mut files = self.files.lock().unwrap();
        if files.contains_key(path) {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, "already there"));
        }
        files.insert(path.to_path_buf(), Arc::new(Mutex::new(bytes.to_vec())));
        Ok(())
    }
}

impl MemoryDisk {
    pub fn read(&self, path: &Path) -> Vec<u8> {
        self.files.lock().unwrap().get(path).map(|b| b.lock().unwrap().clone()).unwrap_or_default()
    }
}

/// A clock stopped at 2026-09-27 14:03:21 UTC.
pub struct StoppedClock;

impl Clock for StoppedClock {
    fn now(&self) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(1_790_517_801)
    }
    fn civil(&self, at: SystemTime) -> Civil {
        Civil::utc(at)
    }
}


/// A machine with this much memory free.
pub struct FreeMemory(pub Option<u64>);

impl Memory for FreeMemory {
    fn available(&self) -> Option<u64> {
        self.0
    }
}
