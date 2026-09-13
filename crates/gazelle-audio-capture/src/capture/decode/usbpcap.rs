//! Link type 249: USBPcap `USBPCAP_BUFFER_PACKET_HEADER` (packed, little-endian).
//!
//! Offsets: headerLen u16 @0, irpId u64 @2, status i32 @10, function u16 @14, info u8 @16,
//! bus u16 @17, device u16 @19, endpoint u8 @21, transfer u8 @22, dataLength u32 @23.
//! Control transfers append a stage byte @27 (headerLen 28); isochronous transfers append
//! startFrame, numberOfPackets, errorCount and 12 bytes per packet. Data follows headerLen.

use super::DecodeError;
use crate::capture::event::{Direction, SetupPacket, TransferType, UrbStage, UsbEvent};
use crate::capture::RawFrame;

pub const LINKTYPE_USBPCAP: u32 = 249;
/// Size of the base header without transfer-specific extensions.
pub const BASE_HEADER_LEN: usize = 27;
/// `USBPCAP_INFO_PDO_TO_FDO`: set on completions travelling back up the stack.
pub const INFO_PDO_TO_FDO: u8 = 0x01;
pub const CONTROL_STAGE_SETUP: u8 = 0;
pub const CONTROL_STAGE_DATA: u8 = 1;
pub const CONTROL_STAGE_STATUS: u8 = 2;
pub const CONTROL_STAGE_COMPLETE: u8 = 3;

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().expect("4 bytes"))
}

/// Header length declared by the frame, validated against the frame size.
pub fn header_len(data: &[u8]) -> Result<usize, DecodeError> {
    if data.len() < BASE_HEADER_LEN {
        return Err(DecodeError::Truncated { needed: BASE_HEADER_LEN, got: data.len() });
    }
    let len = u16_at(data, 0) as usize;
    if len < BASE_HEADER_LEN || len > data.len() {
        return Err(DecodeError::BadHeaderLen { declared: len, frame: data.len() });
    }
    Ok(len)
}

/// Decodes one frame; `Ok(None)` for pseudo transfer types (IRP info 0xFE, unknown 0xFF).
pub fn decode(frame: &RawFrame) -> Result<Option<UsbEvent>, DecodeError> {
    let b = &frame.data;
    let hlen = header_len(b)?;
    let Some(transfer) = TransferType::from_wire(b[22]) else {
        return Ok(None);
    };
    let endpoint_byte = b[21];
    let declared = u32_at(b, 23);
    let stage = if b[16] & INFO_PDO_TO_FDO != 0 { UrbStage::Complete } else { UrbStage::Submit };
    let mut payload = &b[hlen..];
    let mut data_len = declared;
    let mut setup = None;
    if transfer == TransferType::Control {
        if hlen < BASE_HEADER_LEN + 1 {
            return Err(DecodeError::BadHeaderLen { declared: hlen, frame: b.len() });
        }
        if b[BASE_HEADER_LEN] == CONTROL_STAGE_SETUP {
            setup = SetupPacket::parse(payload);
            if setup.is_none() {
                return Err(DecodeError::Truncated { needed: hlen + 8, got: b.len() });
            }
            payload = &payload[8..];
            data_len = declared.saturating_sub(8);
        }
    }
    let payload_dropped = (payload.len() as u64) < data_len as u64;
    let captured = (payload.len() as u64).min(data_len as u64) as usize;
    Ok(Some(UsbEvent {
        ts_ns: frame.ts_ns,
        packet_index: frame.index,
        bus: u16_at(b, 17),
        device: u16_at(b, 19),
        endpoint: endpoint_byte & 0x7F,
        direction: if endpoint_byte & 0x80 != 0 { Direction::In } else { Direction::Out },
        transfer,
        stage,
        urb_id: u64::from_le_bytes(b[2..10].try_into().expect("8 bytes")),
        setup,
        status: u32_at(b, 10) as i32,
        data_len,
        data: payload[..captured].to_vec(),
        payload_dropped,
    }))
}

/// Encodes an event as a USBPcap frame (the synthetic generator's writer).
/// Isochronous events are written with zero ISO packet descriptors.
pub fn encode(ev: &UsbEvent) -> Vec<u8> {
    let (hlen, extra): (usize, Vec<u8>) = match ev.transfer {
        TransferType::Control => {
            let stage = if ev.setup.is_some() {
                CONTROL_STAGE_SETUP
            } else {
                CONTROL_STAGE_COMPLETE
            };
            (BASE_HEADER_LEN + 1, vec![stage])
        }
        TransferType::Isochronous => (BASE_HEADER_LEN + 12, vec![0; 12]),
        _ => (BASE_HEADER_LEN, Vec::new()),
    };
    let setup_len = if ev.setup.is_some() { 8 } else { 0 };
    let mut out = Vec::with_capacity(hlen + setup_len + ev.data.len());
    out.extend_from_slice(&(hlen as u16).to_le_bytes());
    out.extend_from_slice(&ev.urb_id.to_le_bytes());
    out.extend_from_slice(&(ev.status as u32).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // URB function: not modelled
    out.push(if ev.stage == UrbStage::Complete { INFO_PDO_TO_FDO } else { 0 });
    out.extend_from_slice(&ev.bus.to_le_bytes());
    out.extend_from_slice(&ev.device.to_le_bytes());
    let dir = if ev.direction == Direction::In { 0x80 } else { 0 };
    out.push(ev.endpoint & 0x7F | dir);
    out.push(ev.transfer.to_wire());
    out.extend_from_slice(&(ev.data_len + setup_len as u32).to_le_bytes());
    out.extend_from_slice(&extra);
    if let Some(s) = ev.setup {
        out.extend_from_slice(&s.to_bytes());
    }
    out.extend_from_slice(&ev.data);
    out
}
