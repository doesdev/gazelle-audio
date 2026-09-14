//! Wall clock for marks. On Windows `SystemTime::now` is `GetSystemTimePreciseAsFileTime`,
//! and USBPcap timestamps with `KeQuerySystemTimePrecise`: the same system clock.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub trait Clock: Send + Sync {
    /// Nanoseconds since the Unix epoch.
    fn now_ns(&self) -> u64;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ns(&self) -> u64 {
        SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64)
    }
}

/// A clock tests move by hand.
#[derive(Debug, Default)]
pub struct ManualClock(AtomicU64);

impl ManualClock {
    pub fn new(start_ns: u64) -> Self {
        Self(AtomicU64::new(start_ns))
    }

    pub fn set(&self, ns: u64) {
        self.0.store(ns, Ordering::SeqCst);
    }

    pub fn advance(&self, ns: u64) {
        self.0.fetch_add(ns, Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now_ns(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}
