//! Frame → event: decode, keep only the target device, drop stream payloads.

use std::collections::{HashMap, HashSet};

use super::decode::{self, DecodeError};
use super::event::{Direction, TransferType, UrbStage, UsbEvent};
use super::RawFrame;

/// Which device's traffic to keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceFilter {
    All,
    Address { bus: u16, device: u16 },
    /// Resolved from device descriptors seen in the stream (USBPcap `--inject-descriptors`).
    Target { vid: u16, pid: u16 },
}

/// Isochronous and bulk payloads are dropped unless the session keeps them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PayloadPolicy {
    pub keep_stream_payloads: bool,
}

impl PayloadPolicy {
    pub fn drops(&self, transfer: TransferType) -> bool {
        !self.keep_stream_payloads && matches!(transfer, TransferType::Isochronous | TransferType::Bulk)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    pub vid: u16,
    pub pid: u16,
    /// The 18-byte device descriptor as captured.
    pub descriptor: Vec<u8>,
}

/// Learns `(bus, address) → device descriptor` from GET_DESCRIPTOR(DEVICE) exchanges.
#[derive(Debug, Default)]
pub struct DeviceMap {
    awaiting: HashSet<(u16, u16)>,
    devices: HashMap<(u16, u16), DeviceInfo>,
}

impl DeviceMap {
    /// True when `ev` is a GET_DESCRIPTOR(DEVICE) request.
    pub fn is_device_descriptor_request(ev: &UsbEvent) -> bool {
        ev.transfer == TransferType::Control
            && ev.stage == UrbStage::Submit
            && ev.setup.is_some_and(|s| s.request_type & 0x80 != 0 && s.request == 6 && s.value >> 8 == 1)
    }

    pub fn observe(&mut self, ev: &UsbEvent) {
        let key = (ev.bus, ev.device);
        if Self::is_device_descriptor_request(ev) {
            self.awaiting.insert(key);
        } else if ev.transfer == TransferType::Control
            && ev.stage == UrbStage::Complete
            && ev.direction == Direction::In
            && self.awaiting.remove(&key)
            && ev.data.len() >= 12
            && ev.data[1] == 1
        {
            let vid = u16::from_le_bytes([ev.data[8], ev.data[9]]);
            let pid = u16::from_le_bytes([ev.data[10], ev.data[11]]);
            self.devices.insert(key, DeviceInfo { vid, pid, descriptor: ev.data.clone() });
        }
    }

    pub fn get(&self, bus: u16, device: u16) -> Option<&DeviceInfo> {
        self.devices.get(&(bus, device))
    }

    /// Address of the first device with this VID/PID.
    pub fn find(&self, vid: u16, pid: u16) -> Option<(u16, u16)> {
        let mut hits: Vec<_> = self.devices.iter().filter(|(_, d)| d.vid == vid && d.pid == pid).map(|(k, _)| *k).collect();
        hits.sort_unstable();
        hits.first().copied()
    }
}

pub struct Pipeline {
    filter: DeviceFilter,
    policy: PayloadPolicy,
    map: DeviceMap,
    /// A descriptor request held until its reply shows whether the device is the target.
    held: HashMap<(u16, u16), (RawFrame, UsbEvent)>,
}

impl Pipeline {
    pub fn new(filter: DeviceFilter, policy: PayloadPolicy) -> Self {
        Self { filter, policy, map: DeviceMap::default(), held: HashMap::new() }
    }

    pub fn device_map(&self) -> &DeviceMap {
        &self.map
    }

    fn accepts(&self, ev: &UsbEvent) -> bool {
        match self.filter {
            DeviceFilter::All => true,
            DeviceFilter::Address { bus, device } => ev.bus == bus && ev.device == device,
            DeviceFilter::Target { vid, pid } => self
                .map
                .get(ev.bus, ev.device)
                .is_some_and(|d| d.vid == vid && d.pid == pid),
        }
    }

    /// Returns the frames to store, each with its event. The frame is truncated to its
    /// headers when the payload policy drops its data; `orig_len` keeps the true size.
    pub fn process(&mut self, mut frame: RawFrame) -> Result<Vec<(RawFrame, UsbEvent)>, DecodeError> {
        let Some(mut ev) = decode::decode(&frame)? else {
            return Ok(Vec::new());
        };
        self.map.observe(&ev);
        let key = (ev.bus, ev.device);
        let targeting = matches!(self.filter, DeviceFilter::Target { .. });
        if targeting && DeviceMap::is_device_descriptor_request(&ev) {
            // Keep it only if the reply proves (or already proved) this is the target.
            if !self.accepts(&ev) {
                self.held.insert(key, (frame, ev));
                return Ok(Vec::new());
            }
        }
        let held = self.held.remove(&key);
        if !self.accepts(&ev) {
            return Ok(Vec::new());
        }
        if self.policy.drops(ev.transfer) && !ev.data.is_empty() {
            frame.data.truncate(decode::header_len(&frame)?);
            ev.data.clear();
            ev.payload_dropped = true;
        }
        let mut out = Vec::with_capacity(2);
        out.extend(held);
        out.push((frame, ev));
        Ok(out)
    }
}
