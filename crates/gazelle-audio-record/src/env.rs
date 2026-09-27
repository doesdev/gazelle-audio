//! The two PCs the recorder runs on: this one, with its drivers, and the loopback's, made of data.

use std::sync::{Arc, Mutex};

use gazelle_aggregate::fake::{FakeDevice, FakeHost, FakePc};
use gazelle_aggregate::status::Reporter;
use gazelle_aggregate::sub::Host;

use crate::recorder::Environment;
use crate::sim::{self, Timing};
use crate::system::{Clock, Disk, LocalClock, Memory, ThisPcDisk, ThisPcMemory};

/// This PC: its drivers, its memory, its disks, its clock, and the driver's own log.
pub struct ThisPc {
    pub originator: String,
}

impl Environment for ThisPc {
    #[cfg(windows)]
    fn host(&self) -> Result<Box<dyn Host>, String> {
        use gazelle_calibrate::session::{driver_in_use, no_hardware_refusal, NO_HARDWARE};
        // The same refusals a measurement makes, before a driver is opened: a test must never reach
        // one, and a DAW that has the interfaces keeps them.
        if no_hardware_refusal(std::env::var(NO_HARDWARE).ok().as_deref()).is_some() {
            return Err(format!("{NO_HARDWARE} is set, and recording opens the real interfaces: set {NO_HARDWARE}=0 to record deliberately"));
        }
        if let Some(why) = driver_in_use() {
            return Err(why);
        }
        Ok(Box::new(gazelle_aggregate::windows_host::ThisPc))
    }

    #[cfg(not(windows))]
    fn host(&self) -> Result<Box<dyn Host>, String> {
        Err("recording from the interfaces needs Windows, which this is not running on".into())
    }

    fn pump(&self, _timing: Timing, _master: usize) -> Box<dyn FnMut(usize) -> bool + Send> {
        crate::recorder::wait_for_the_devices()
    }

    fn memory(&self) -> Arc<dyn Memory> {
        Arc::new(ThisPcMemory)
    }

    fn disk(&self) -> Arc<dyn Disk> {
        Arc::new(ThisPcDisk)
    }

    fn clock(&self) -> Arc<dyn Clock> {
        Arc::new(LocalClock)
    }

    #[cfg(windows)]
    fn reporter(&self) -> Arc<Reporter> {
        gazelle_calibrate::session::reporter_marked(crate::host::MARK)
    }

    #[cfg(not(windows))]
    fn reporter(&self) -> Arc<Reporter> {
        Arc::new(Reporter::silent())
    }

    fn originator(&self) -> String {
        self.originator.clone()
    }
}

/// The loopback's PC: both interfaces made of data (`crate::sim`), a test signal on every input, this
/// PC's memory, disks and clock, and a session that publishes nothing, since there is no driver.
pub struct Loopback {
    pub originator: String,
    pc: Mutex<Option<Arc<FakePc>>>,
}

impl Loopback {
    pub fn new(originator: String) -> Loopback {
        Loopback { originator, pc: Mutex::new(None) }
    }
}

impl Environment for Loopback {
    fn host(&self) -> Result<Box<dyn Host>, String> {
        let pc = sim::simulated_pc();
        if let Ok(mut current) = self.pc.lock() {
            *current = Some(Arc::clone(&pc));
        }
        Ok(Box::new(FakeHost { pc }))
    }

    fn pump(&self, timing: Timing, master: usize) -> Box<dyn FnMut(usize) -> bool + Send> {
        let pc = self.pc.lock().ok().and_then(|pc| pc.clone()).unwrap_or_else(sim::simulated_pc);
        Box::new(sim::pump(firing_order(&pc, master), timing))
    }

    fn memory(&self) -> Arc<dyn Memory> {
        Arc::new(ThisPcMemory)
    }

    fn disk(&self) -> Arc<dyn Disk> {
        Arc::new(ThisPcDisk)
    }

    fn clock(&self) -> Arc<dyn Clock> {
        Arc::new(LocalClock)
    }

    fn reporter(&self) -> Arc<Reporter> {
        Arc::new(Reporter::silent())
    }

    fn originator(&self) -> String {
        self.originator.clone()
    }
}

/// The devices in the aggregate, the ones that follow first and the one driving the callback last.
pub fn firing_order(pc: &FakePc, master: usize) -> Vec<Arc<FakeDevice>> {
    let mut open: Vec<Arc<FakeDevice>> = FakeDevice::every_device(pc).into_iter().filter(|device| device.stream_index().is_some()).collect();
    open.sort_by_key(|device| device.stream_index() == Some(master));
    open
}
