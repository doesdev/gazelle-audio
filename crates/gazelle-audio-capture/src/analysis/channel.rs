//! Traffic channels and the messages carried on them.

use std::collections::HashMap;

use serde::Serialize;

use crate::capture::event::{Direction, TransferType, UrbStage, UsbEvent};

/// Most distinct values byte 0 may take on an interrupt or bulk endpoint for it to be treated
/// as a message-type discriminator.
pub const MAX_DISCRIMINATORS: usize = 8;
/// Share of the endpoint's messages every discriminator value must cover.
pub const MIN_DISCRIMINATOR_SHARE: f64 = 0.01;

/// Where a message travels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct ChannelKey {
    pub endpoint: u8,
    pub direction: Direction,
    pub transfer: TransferType,
    /// Control transfers: `bRequest`.
    pub request: Option<u8>,
    /// Control transfers: `wIndex`.
    pub index: Option<u16>,
    /// Interrupt and bulk transfers: byte 0, when it was detected to partition the endpoint.
    pub discriminator: Option<u8>,
}

impl ChannelKey {
    /// A total order for stable output.
    pub fn sort_key(&self) -> (u8, u8, u8, u16, u32, u16) {
        let direction = u8::from(self.direction == Direction::Out);
        let request = self.request.map_or(0, |r| r as u16 + 1);
        let index = self.index.map_or(0, |i| i as u32 + 1);
        let discriminator = self.discriminator.map_or(0, |d| d as u16 + 1);
        (self.endpoint, direction, self.transfer.to_wire(), request, index, discriminator)
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

/// The message an event carries, without discriminators. Host→device data travels in submits
/// and device→host data in completions; isochronous traffic and records whose payload was
/// dropped carry none.
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
    let channel = ChannelKey { endpoint: event.endpoint, direction: event.direction, transfer: event.transfer, request, index, discriminator: None };
    Some(Message { channel, bytes, event })
}

type Endpoint = (u8, Direction, TransferType);

/// Which interrupt and bulk endpoints are split by a leading discriminator byte, detected over
/// a whole capture: byte 0 takes at most [`MAX_DISCRIMINATORS`] values, each covering at least
/// [`MIN_DISCRIMINATOR_SHARE`] of the endpoint's messages.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Channels {
    discriminated: Vec<Endpoint>,
}

impl Channels {
    pub fn detect(events: &[UsbEvent]) -> Self {
        let mut counts: HashMap<Endpoint, HashMap<u8, usize>> = HashMap::new();
        for m in events.iter().filter_map(message) {
            if matches!(m.channel.transfer, TransferType::Interrupt | TransferType::Bulk) {
                let endpoint = (m.channel.endpoint, m.channel.direction, m.channel.transfer);
                *counts.entry(endpoint).or_default().entry(m.bytes[0]).or_default() += 1;
            }
        }
        let mut discriminated: Vec<Endpoint> = counts
            .into_iter()
            .filter(|(_, by_value)| {
                let total: usize = by_value.values().sum();
                by_value.len() <= MAX_DISCRIMINATORS && by_value.values().all(|&n| n as f64 >= MIN_DISCRIMINATOR_SHARE * total as f64)
            })
            .map(|(endpoint, _)| endpoint)
            .collect();
        discriminated.sort_by_key(|(endpoint, direction, transfer)| (*endpoint, u8::from(*direction == Direction::Out), transfer.to_wire()));
        Self { discriminated }
    }

    pub fn is_discriminated(&self, endpoint: u8, direction: Direction, transfer: TransferType) -> bool {
        self.discriminated.contains(&(endpoint, direction, transfer))
    }

    /// The message an event carries, keyed by discriminator where one was detected.
    pub fn message<'a>(&self, event: &'a UsbEvent) -> Option<Message<'a>> {
        let mut m = message(event)?;
        if self.is_discriminated(m.channel.endpoint, m.channel.direction, m.channel.transfer) {
            m.channel.discriminator = Some(m.bytes[0]);
        }
        Some(m)
    }
}
