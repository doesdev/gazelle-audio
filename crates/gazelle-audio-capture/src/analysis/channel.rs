//! Traffic channels and the messages carried on them (spec §8 step 2).

use serde::Serialize;

use crate::capture::event::{Direction, TransferType, UrbStage, UsbEvent};

/// Where a message travels. Control transfers are further split by `bRequest` and `wIndex`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct ChannelKey {
    pub endpoint: u8,
    pub direction: Direction,
    pub transfer: TransferType,
    pub request: Option<u8>,
    pub index: Option<u16>,
}

impl ChannelKey {
    /// A total order for stable output.
    pub fn sort_key(&self) -> (u8, u8, u8, u16, u32) {
        let direction = match self.direction {
            Direction::In => 0,
            Direction::Out => 1,
        };
        let request = self.request.map_or(0, |r| r as u16 + 1);
        let index = self.index.map_or(0, |i| i as u32 + 1);
        (self.endpoint, direction, self.transfer.to_wire(), request, index)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message<'a> {
    pub channel: ChannelKey,
    /// Control transfers: `wValue` as two little-endian bytes, then the data stage. Other
    /// transfers: the data.
    pub bytes: Vec<u8>,
    pub event: &'a UsbEvent,
}

/// The message an event carries, if any. Host→device data travels in submits and device→host
/// data in completions; isochronous traffic and records whose payload was dropped carry none.
pub fn message(event: &UsbEvent) -> Option<Message<'_>> {
    let carries = match event.direction {
        Direction::Out => event.stage == UrbStage::Submit,
        Direction::In => event.stage == UrbStage::Complete,
    };
    if !carries || event.transfer == TransferType::Isochronous || event.payload_dropped {
        return None;
    }
    let (request, index, mut bytes) = match event.transfer {
        TransferType::Control => {
            let setup = event.setup?;
            (Some(setup.request), Some(setup.index), setup.value.to_le_bytes().to_vec())
        }
        _ => (None, None, Vec::new()),
    };
    bytes.extend_from_slice(&event.data);
    if bytes.is_empty() {
        return None;
    }
    let channel = ChannelKey { endpoint: event.endpoint, direction: event.direction, transfer: event.transfer, request, index };
    Some(Message { channel, bytes, event })
}
