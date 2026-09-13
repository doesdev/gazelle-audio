//! Capture backends and the frame → event pipeline.

pub mod decode;
pub mod event;
pub mod import;
pub mod pipeline;
pub mod rate;
pub mod writer;

use decode::ByteOrder;

/// One link-layer frame as read from a capture source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawFrame {
    /// Nanoseconds since the Unix epoch.
    pub ts_ns: u64,
    /// pcap `LINKTYPE_*` value (249 USBPcap, 220 usbmon).
    pub link_type: u32,
    /// Zero-based index within the source (file position or live sequence).
    pub index: u64,
    /// Length on the wire before any truncation.
    pub orig_len: u32,
    /// Byte order of host-ordered link headers (usbmon); USBPcap is always little-endian.
    pub byte_order: ByteOrder,
    pub data: Vec<u8>,
}

#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("i/o: {0}")]
    Io(#[from] std::io::Error),
    #[error("capture file: {0}")]
    Format(String),
    #[error("decode: {0}")]
    Decode(#[from] decode::DecodeError),
    #[error("unsupported on this platform: {0}")]
    Unsupported(String),
    #[error("capture tool: {0}")]
    Tool(String),
}

/// Frames in capture order; ends when the source is exhausted or stopped.
pub type FrameIter = Box<dyn Iterator<Item = Result<RawFrame, CaptureError>> + Send>;

/// Stops a running source from another thread; the frame iterator then ends.
pub struct StopHandle(Option<Box<dyn FnOnce() + Send>>);

impl StopHandle {
    pub fn new(stop: impl FnOnce() + Send + 'static) -> Self {
        Self(Some(Box::new(stop)))
    }

    /// A handle for sources that end on their own (files).
    pub fn noop() -> Self {
        Self(None)
    }

    pub fn stop(mut self) {
        if let Some(f) = self.0.take() {
            f();
        }
    }
}

/// A started capture: its frames and the means to stop it.
pub struct CaptureStream {
    pub frames: FrameIter,
    pub stop: StopHandle,
}

/// A backend producing raw link-layer frames. Implemented by the USBPcap live backend and by
/// file import; a Linux usbmon live backend would implement it too.
pub trait CaptureSource: Send {
    /// Human-readable description for status displays and logs.
    fn describe(&self) -> String;
    /// Starts capturing. Called once per probe.
    fn start(&mut self) -> Result<CaptureStream, CaptureError>;
}
