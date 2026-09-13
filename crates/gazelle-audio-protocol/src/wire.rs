//! The 16-byte wire header shared by every Antelope Synergy Core report.
//!
//! Recovered from `refs/decompiled/manager/antelope_dev_reports.py` (`Request`,
//! `IncomingReport`) and `refs/decompiled/manager/antelope_dev_base.py`
//! (`_get_cmd_id`, `_get_pkt_len`, `_get_ext2`, `_get_ext3`).
//!
//! Layout (all little-endian `u32`):
//! ```text
//! cmd   : u32  report/command id
//! seq   : u32  payload length for requests; CRC32 of payload for cyclic reports
//! ext2  : u32  command type / bank index / per-field selector
//! ext3  : u32  per-field selector / offset
//! ```

use std::fmt;

/// Size of the fixed wire header, in bytes.
pub const HEADER_SIZE: usize = 16;

/// The header is four little-endian `u32` words.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(C)]
pub struct Header {
    pub cmd: u32,
    pub seq: u32,
    pub ext2: u32,
    pub ext3: u32,
}

impl Header {
    pub fn new(cmd: u32, seq: u32, ext2: u32, ext3: u32) -> Self {
        Header { cmd, seq, ext2, ext3 }
    }

    /// Serialize the header to 16 little-endian bytes.
    pub fn to_bytes(&self) -> [u8; HEADER_SIZE] {
        let mut out = [0u8; HEADER_SIZE];
        out[0..4].copy_from_slice(&self.cmd.to_le_bytes());
        out[4..8].copy_from_slice(&self.seq.to_le_bytes());
        out[8..12].copy_from_slice(&self.ext2.to_le_bytes());
        out[12..16].copy_from_slice(&self.ext3.to_le_bytes());
        out
    }

    /// Parse a header from the first 16 bytes of a buffer.
    pub fn from_bytes(b: &[u8]) -> Result<Self, WireError> {
        if b.len() < HEADER_SIZE {
            return Err(WireError::TruncatedHeader);
        }
        Ok(Header {
            cmd: u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            seq: u32::from_le_bytes([b[4], b[5], b[6], b[7]]),
            ext2: u32::from_le_bytes([b[8], b[9], b[10], b[11]]),
            ext3: u32::from_le_bytes([b[12], b[13], b[14], b[15]]),
        })
    }
}

impl fmt::Display for Header {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "[0x{:X}, seq=0x{:X}, ext2=0x{:X}, ext3=0x{:X}]",
            self.cmd, self.seq, self.ext2, self.ext3
        )
    }
}

/// Errors produced while parsing or serializing wire data.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WireError {
    /// A buffer was shorter than the 16-byte header.
    TruncatedHeader,
    /// A buffer was shorter than the payload length declared in `seq`.
    TruncatedPayload,
    /// A payload length exceeded the maximum representable size.
    PayloadTooLarge,
    /// A field value did not fit in its declared bit width.
    FieldOverflow,
}

impl fmt::Display for WireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use WireError::*;
        let msg = match self {
            TruncatedHeader => "buffer shorter than 16-byte header",
            TruncatedPayload => "buffer shorter than declared payload length",
            PayloadTooLarge => "payload length exceeds maximum",
            FieldOverflow => "field value does not fit its declared bit width",
        };
        f.write_str(msg)
    }
}

impl std::error::Error for WireError {}
