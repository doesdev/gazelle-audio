//! Link-layer decoders: everything downstream sees only [`UsbEvent`].

pub mod usbmon;
pub mod usbpcap;

use crate::capture::event::UsbEvent;
use crate::capture::RawFrame;

/// Byte order of multi-byte fields in a frame whose link type is host-ordered (usbmon).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ByteOrder {
    Little,
    Big,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DecodeError {
    #[error("frame truncated: needed {needed} bytes, got {got}")]
    Truncated { needed: usize, got: usize },
    #[error("header length {declared} invalid for a {frame}-byte frame")]
    BadHeaderLen { declared: usize, frame: usize },
    #[error("unsupported link type {0}")]
    UnsupportedLinkType(u32),
}

/// Decodes a frame by its link type. `Ok(None)` means a valid record that is not a USB transfer.
pub fn decode(frame: &RawFrame) -> Result<Option<UsbEvent>, DecodeError> {
    match frame.link_type {
        usbpcap::LINKTYPE_USBPCAP => usbpcap::decode(frame),
        usbmon::LINKTYPE_USB_LINUX_MMAPPED => usbmon::decode(frame),
        other => Err(DecodeError::UnsupportedLinkType(other)),
    }
}

/// Bytes before the transfer data (headers, setup packet excluded). Used to drop payloads.
pub fn header_len(frame: &RawFrame) -> Result<usize, DecodeError> {
    match frame.link_type {
        usbpcap::LINKTYPE_USBPCAP => usbpcap::header_len(&frame.data),
        usbmon::LINKTYPE_USB_LINUX_MMAPPED => usbmon::header_len(&frame.data, frame.byte_order),
        other => Err(DecodeError::UnsupportedLinkType(other)),
    }
}
