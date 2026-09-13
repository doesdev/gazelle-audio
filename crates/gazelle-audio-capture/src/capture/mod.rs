//! Capture backends and the frame → event pipeline.

pub mod decode;
pub mod event;

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
