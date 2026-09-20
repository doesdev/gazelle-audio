//! One sub-device, and the PC it was found on, behind traits.
//!
//! Every rule in this crate is decided against these two traits, so the whole driver runs against
//! fakes and no test opens a driver, starts a converter or touches a registry. The real ones live
//! in `windows_host`.

use std::sync::Arc;

use crate::stream::Stream;
use gazelle_audio_stream_abi::Entry;

/// Everything the aggregate reads out of a sub-device after `init`.
#[derive(Clone, Debug, PartialEq)]
pub struct Description {
    /// What the driver calls itself.
    pub name: String,
    pub version: i32,
    pub inputs: i32,
    pub outputs: i32,
    pub min: i32,
    pub max: i32,
    pub preferred: i32,
    pub granularity: i32,
    pub rate: f64,
    pub latency_in: i32,
    pub latency_out: i32,
    /// The sample type of its input and output channels.
    pub input_type: i32,
    pub output_type: i32,
    /// Whether it takes `outputReady`.
    pub output_ready: bool,
}

/// Where a device's audio actually is: two halves per channel, owned by the device, filled in when
/// its buffers were made.
///
/// These are raw pointers into another driver's memory. They are valid from the moment its buffers
/// are made until they are disposed of, and nothing in this crate keeps one past that.
pub struct DeviceBuffers {
    pub inputs: Vec<[*mut u8; 2]>,
    pub outputs: Vec<[*mut u8; 2]>,
    /// Samples in one half, which is the block every device runs at.
    pub block: usize,
}

impl DeviceBuffers {
    pub fn empty() -> DeviceBuffers {
        DeviceBuffers { inputs: Vec::new(), outputs: Vec::new(), block: 0 }
    }
}

/// One vendor driver. Dropping it releases whatever it holds.
pub trait SubDriver: Send {
    fn init(&mut self) -> Result<(), String>;
    fn describe(&mut self) -> Result<Description, String>;
    /// Whether the device will run at this rate. Asks, and changes nothing.
    fn can_rate(&mut self, hz: f64) -> bool;
    fn set_rate(&mut self, hz: f64) -> Result<(), String>;
    /// Open exactly these channels, by the device's own numbering, at `block` samples.
    fn create_buffers(&mut self, inputs: &[i32], outputs: &[i32], block: i32) -> Result<DeviceBuffers, String>;
    /// Hand the device the stream its callbacks belong to. Called after every device's buffers are
    /// made and before any of them is started, which is the only moment at which a callback cannot
    /// already be in flight.
    fn attach(&mut self, stream: Arc<Stream>, device: usize);
    /// Let go of the stream. Called after every device is stopped.
    fn detach(&mut self);
    fn start(&mut self) -> Result<(), String>;
    fn stop(&mut self);
    fn dispose_buffers(&mut self);
}

/// The PC: which drivers it has, and opening one.
pub trait Host {
    /// Every driver of this kind registered on the PC, in registry order.
    fn entries(&self) -> Result<Vec<Entry>, String>;
    /// Create the driver object for one entry. `slot` is which set of static callbacks it gets,
    /// because the interface gives a callback nothing to say who called it.
    fn open(&self, entry: &Entry, slot: usize) -> Result<Box<dyn SubDriver>, String>;
}
