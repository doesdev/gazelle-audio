//! Simulated devices. Sub-project 2 adds models with counters, checksums, meters,
//! multi-message commands and spillover by implementing [`DeviceModel`].

use crate::capture::event::{Direction, SetupPacket, TransferType, UrbStage, UsbEvent};

/// A device on the simulated bus. Events are emitted with `packet_index` 0; the generator
/// assigns indices when it stores them.
pub trait DeviceModel: Send {
    fn vid_pid(&self) -> (u16, u16);
    fn address(&self) -> (u16, u16);
    /// Interval between `tick` calls.
    fn tick_ns(&self) -> u64;
    /// The descriptor exchange USBPcap injects at capture start.
    fn enumerate(&mut self, t_ns: u64, out: &mut Vec<UsbEvent>);
    /// Periodic traffic.
    fn tick(&mut self, t_ns: u64, out: &mut Vec<UsbEvent>);
    /// The operator set `parameter` to `value` in the vendor UI.
    fn change(&mut self, t_ns: u64, parameter: &str, value: &str, out: &mut Vec<UsbEvent>);
    /// The operator touched `parameter` and left it unchanged.
    fn touch(&mut self, t_ns: u64, parameter: &str, out: &mut Vec<UsbEvent>);
}

/// Where [`SimpleDevice`] encodes a parameter: a vendor control OUT request with
/// `bRequest` = `request`, `wIndex` = `index`, `wValue` and data byte 1 = value position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlantedField {
    pub request: u8,
    pub index: u16,
    pub data_byte: usize,
}

pub const SIMPLE_REQUEST: u8 = 0x01;
pub const SIMPLE_STATUS_ENDPOINT: u8 = 1;

/// Periodic 4-byte interrupt status report plus one vendor control write per change.
pub struct SimpleDevice {
    vid: u16,
    pid: u16,
    bus: u16,
    device: u16,
    period_ns: u64,
    parameters: Vec<(String, Vec<String>)>,
    urb: u64,
}

impl SimpleDevice {
    pub fn new(vid: u16, pid: u16, bus: u16, device: u16) -> Self {
        Self { vid, pid, bus, device, period_ns: 100_000_000, parameters: Vec::new(), urb: 0 }
    }

    /// Declares a parameter the device understands and the UI values in wire order.
    pub fn with_parameter(mut self, id: &str, values: &[&str]) -> Self {
        self.parameters.push((id.to_string(), values.iter().map(|v| v.to_string()).collect()));
        self
    }

    pub fn planted(&self, parameter: &str) -> Option<PlantedField> {
        let i = self.parameters.iter().position(|(id, _)| id == parameter)?;
        Some(PlantedField { request: SIMPLE_REQUEST, index: i as u16 + 1, data_byte: 1 })
    }

    /// Raw value the device sends for a UI value (its position; 0xFF when unknown).
    pub fn raw_value(&self, parameter: &str, value: &str) -> Option<u8> {
        let (_, values) = self.parameters.iter().find(|(id, _)| id == parameter)?;
        Some(values.iter().position(|v| v == value).map_or(0xFF, |p| p as u8))
    }

    fn event(&mut self, t_ns: u64, endpoint: u8, direction: Direction, transfer: TransferType, stage: UrbStage) -> UsbEvent {
        if stage == UrbStage::Submit {
            self.urb += 1;
        }
        UsbEvent {
            ts_ns: t_ns,
            packet_index: 0,
            bus: self.bus,
            device: self.device,
            endpoint,
            direction,
            transfer,
            stage,
            urb_id: self.urb,
            setup: None,
            status: 0,
            data_len: 0,
            data: Vec::new(),
            payload_dropped: false,
        }
    }
}

fn with_data(mut ev: UsbEvent, data: Vec<u8>) -> UsbEvent {
    ev.data_len = data.len() as u32;
    ev.data = data;
    ev
}

impl DeviceModel for SimpleDevice {
    fn vid_pid(&self) -> (u16, u16) {
        (self.vid, self.pid)
    }

    fn address(&self) -> (u16, u16) {
        (self.bus, self.device)
    }

    fn tick_ns(&self) -> u64 {
        self.period_ns
    }

    fn enumerate(&mut self, t_ns: u64, out: &mut Vec<UsbEvent>) {
        let mut req = self.event(t_ns, 0, Direction::In, TransferType::Control, UrbStage::Submit);
        req.setup = Some(SetupPacket { request_type: 0x80, request: 6, value: 0x0100, index: 0, length: 18 });
        out.push(req);
        let v = self.vid.to_le_bytes();
        let p = self.pid.to_le_bytes();
        let descriptor = vec![18, 1, 0x00, 0x02, 0xEF, 0x02, 0x01, 64, v[0], v[1], p[0], p[1], 0x00, 0x01, 1, 2, 3, 1];
        let done = self.event(t_ns, 0, Direction::In, TransferType::Control, UrbStage::Complete);
        out.push(with_data(done, descriptor));
    }

    fn tick(&mut self, t_ns: u64, out: &mut Vec<UsbEvent>) {
        let ev = self.event(t_ns, SIMPLE_STATUS_ENDPOINT, Direction::In, TransferType::Interrupt, UrbStage::Complete);
        out.push(with_data(ev, vec![0xA0, 0x00, 0x00, 0x00]));
    }

    fn change(&mut self, t_ns: u64, parameter: &str, value: &str, out: &mut Vec<UsbEvent>) {
        let (Some(field), Some(raw)) = (self.planted(parameter), self.raw_value(parameter, value)) else {
            return;
        };
        let mut req = self.event(t_ns, 0, Direction::Out, TransferType::Control, UrbStage::Submit);
        req.setup = Some(SetupPacket { request_type: 0x40, request: field.request, value: raw as u16, index: field.index, length: 2 });
        out.push(with_data(req, vec![field.index as u8, raw]));
        out.push(self.event(t_ns + 1_000_000, 0, Direction::Out, TransferType::Control, UrbStage::Complete));
    }

    fn touch(&mut self, _t_ns: u64, _parameter: &str, _out: &mut Vec<UsbEvent>) {}
}
