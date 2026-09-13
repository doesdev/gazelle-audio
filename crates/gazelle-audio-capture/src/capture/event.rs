//! The normalised USB event every stage downstream of `decode` consumes.

use serde::{Deserialize, Serialize};

/// Transfer direction, taken from bit 7 of the endpoint address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    /// Device to host.
    In,
    /// Host to device.
    Out,
}

/// USB transfer type. USBPcap and usbmon share the wire values 0..=3.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransferType {
    Isochronous,
    Interrupt,
    Control,
    Bulk,
}

impl TransferType {
    /// Maps the shared wire value; `None` for pseudo-types such as USBPcap IRP info (0xFE).
    pub fn from_wire(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Isochronous),
            1 => Some(Self::Interrupt),
            2 => Some(Self::Control),
            3 => Some(Self::Bulk),
            _ => None,
        }
    }

    /// Inverse of [`TransferType::from_wire`].
    pub fn to_wire(self) -> u8 {
        match self {
            Self::Isochronous => 0,
            Self::Interrupt => 1,
            Self::Control => 2,
            Self::Bulk => 3,
        }
    }
}

/// Whether the record is the host submitting a request or its completion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UrbStage {
    Submit,
    Complete,
}

/// The 8-byte control setup packet (little-endian on the wire).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SetupPacket {
    pub request_type: u8,
    pub request: u8,
    pub value: u16,
    pub index: u16,
    pub length: u16,
}

impl SetupPacket {
    /// Parses the first 8 bytes; `None` when fewer are present.
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        let b: &[u8; 8] = bytes.get(..8)?.try_into().ok()?;
        Some(Self {
            request_type: b[0],
            request: b[1],
            value: u16::from_le_bytes([b[2], b[3]]),
            index: u16::from_le_bytes([b[4], b[5]]),
            length: u16::from_le_bytes([b[6], b[7]]),
        })
    }

    /// Serialises back to the 8 wire bytes.
    pub fn to_bytes(&self) -> [u8; 8] {
        let v = self.value.to_le_bytes();
        let i = self.index.to_le_bytes();
        let l = self.length.to_le_bytes();
        [self.request_type, self.request, v[0], v[1], i[0], i[1], l[0], l[1]]
    }
}

/// One decoded USB record for the target device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsbEvent {
    /// Capture timestamp, nanoseconds since the Unix epoch.
    pub ts_ns: u64,
    /// Zero-based index of the frame in the capture file it was read from.
    pub packet_index: u64,
    pub bus: u16,
    pub device: u16,
    /// Endpoint number without the direction bit.
    pub endpoint: u8,
    pub direction: Direction,
    pub transfer: TransferType,
    pub stage: UrbStage,
    /// IRP id (USBPcap) or URB id (usbmon); pairs a submit with its completion.
    pub urb_id: u64,
    /// Present on control submits that carry a setup stage.
    pub setup: Option<SetupPacket>,
    /// USBD_STATUS (USBPcap) or negative errno (usbmon); 0 is success.
    pub status: i32,
    /// Data bytes the capture tool reported for this record, excluding any setup packet.
    pub data_len: u32,
    /// Data bytes present; shorter than `data_len` when `payload_dropped`.
    pub data: Vec<u8>,
    /// True when some of the `data_len` bytes are absent (dropped or snapped).
    pub payload_dropped: bool,
}
