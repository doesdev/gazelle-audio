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

fn device_descriptor(vid: u16, pid: u16) -> Vec<u8> {
    let v = vid.to_le_bytes();
    let p = pid.to_le_bytes();
    vec![18, 1, 0x00, 0x02, 0xEF, 0x02, 0x01, 64, v[0], v[1], p[0], p[1], 0x00, 0x01, 1, 2, 3, 1]
}

pub const RICH_COMMAND_ENDPOINT: u8 = 1;
pub const RICH_STATUS_ENDPOINT: u8 = 2;
/// Leading bytes of [`RichDevice`] messages.
pub const RICH_SET: u8 = 0x70;
pub const RICH_COMMIT: u8 = 0x71;
pub const RICH_FOCUS: u8 = 0x72;
pub const RICH_STATUS: u8 = 0x73;
pub const RICH_METERS: u8 = 0x83;
pub const RICH_COMMAND_LEN: usize = 32;
pub const RICH_REPORT_LEN: usize = 16;
/// Command layout: `[0]` kind, `[1]` sequence number, `[2]` parameter id (declaration order + 1),
/// `[3]` raw value.
pub const RICH_SEQ: usize = 1;
pub const RICH_ID: usize = 2;
pub const RICH_VALUE: usize = 3;
/// Status report layout: `[0]` [`RICH_STATUS`], `[1]` counter, `[2]` meter, `[3]` reserved,
/// `[4 + i]` raw value of parameter `i`, `[15]` sum of bytes 0..15 modulo 256.
pub const RICH_COUNTER: usize = 1;
pub const RICH_METER: usize = 2;
pub const RICH_READBACK_BASE: usize = 4;
/// `[8 + i]`: a peak meter for parameter `i`. It rests at a level set by the parameter's raw
/// value and dips and recovers for [`RICH_DIP_REPORTS`] status reports after each change, like
/// the Studio+ `peaks_preamp` bytes.
pub const RICH_PEAK_BASE: usize = 8;
pub const RICH_DIP_REPORTS: u8 = 6;
pub const RICH_CHECKSUM: usize = 15;

/// A device shaped like the audio interfaces captured live on 2026-09-14: interrupt OUT commands on
/// one endpoint told apart by their leading byte and a parameter id — a Set then a Commit per
/// change, each with a sequence number, and a Focus message when a control is touched — and
/// interrupt IN reports alternating between a status report (counter, meter, readback bytes,
/// checksum) and a meters report of pure noise.
pub struct RichDevice {
    vid: u16,
    pid: u16,
    bus: u16,
    device: u16,
    period_ns: u64,
    parameters: Vec<(String, Vec<String>)>,
    raw: Vec<u8>,
    counter: u8,
    seq: u8,
    noise: u32,
    status_next: bool,
    urb: u64,
    /// `(changed, affected)`: changing `changed` also rewrites `affected`.
    spillover: Vec<(String, String)>,
    /// Status reports left in each parameter's peak-meter dip.
    dip: Vec<u8>,
    /// Changes so far, counting from 1.
    changes: usize,
    /// Changes (by count) whose Set command carries a corrupted value on the wire.
    glitches: Vec<usize>,
}

/// Bits a command glitch flips in the value sent on the wire.
pub const RICH_GLITCH_BITS: u8 = 0x40;

/// Bit a spillover flips in the affected parameter's raw value.
pub const RICH_SPILL_BIT: u8 = 0x80;

impl RichDevice {
    pub fn new(vid: u16, pid: u16, bus: u16, device: u16) -> Self {
        Self {
            vid,
            pid,
            bus,
            device,
            period_ns: 20_000_000,
            parameters: Vec::new(),
            raw: Vec::new(),
            counter: 0,
            seq: 0,
            noise: 0x9E37_79B9,
            status_next: true,
            urb: 0,
            spillover: Vec::new(),
            dip: Vec::new(),
            changes: 0,
            glitches: Vec::new(),
        }
    }

    /// The `nth` change (counting every parameter, from 1) sends its Set command with
    /// [`RICH_GLITCH_BITS`] flipped in the value, while the device itself takes the right value
    /// (its readback stays correct): one bad message among good ones.
    pub fn with_command_glitch(mut self, nth: usize) -> Self {
        self.glitches.push(nth);
        self
    }

    /// Deliberate spillover: every change of `changed` also flips [`RICH_SPILL_BIT`] of
    /// `affected`'s raw value, sending a Set for it and updating its readback byte.
    pub fn with_spillover(mut self, changed: &str, affected: &str) -> Self {
        self.spillover.push((changed.to_string(), affected.to_string()));
        self
    }

    /// Declares a parameter the device understands and the UI values in wire order. Its
    /// readback byte starts at raw 0.
    pub fn with_parameter(mut self, id: &str, values: &[&str]) -> Self {
        self.parameters.push((id.to_string(), values.iter().map(|v| v.to_string()).collect()));
        self.raw.push(0);
        self.dip.push(0);
        self
    }

    /// Raw value the device sends for a UI value (its position; 0xFF when unknown).
    pub fn raw_value(&self, parameter: &str, value: &str) -> Option<u8> {
        let (_, values) = self.parameters.iter().find(|(id, _)| id == parameter)?;
        Some(values.iter().position(|v| v == value).map_or(0xFF, |p| p as u8))
    }

    /// Declaration order of `parameter`: its command id is this + 1 and its readback byte is
    /// [`RICH_READBACK_BASE`] + this.
    pub fn position(&self, parameter: &str) -> Option<usize> {
        self.parameters.iter().position(|(id, _)| id == parameter)
    }

    fn next_noise(&mut self) -> u8 {
        let mut x = self.noise;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.noise = x;
        (x >> 24) as u8
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

    fn command(&mut self, kind: u8, position: usize, raw: u8) -> Vec<u8> {
        let mut bytes = vec![0; RICH_COMMAND_LEN];
        bytes[0] = kind;
        bytes[RICH_SEQ] = self.seq;
        bytes[RICH_ID] = position as u8 + 1;
        bytes[RICH_VALUE] = raw;
        self.seq = self.seq.wrapping_add(1);
        bytes
    }

    fn send(&mut self, t_ns: u64, bytes: Vec<u8>, out: &mut Vec<UsbEvent>) {
        let submit = self.event(t_ns, RICH_COMMAND_ENDPOINT, Direction::Out, TransferType::Interrupt, UrbStage::Submit);
        out.push(with_data(submit, bytes));
        out.push(self.event(t_ns + 1_000_000, RICH_COMMAND_ENDPOINT, Direction::Out, TransferType::Interrupt, UrbStage::Complete));
    }
}

impl DeviceModel for RichDevice {
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
        let done = self.event(t_ns, 0, Direction::In, TransferType::Control, UrbStage::Complete);
        out.push(with_data(done, device_descriptor(self.vid, self.pid)));
    }

    fn tick(&mut self, t_ns: u64, out: &mut Vec<UsbEvent>) {
        let mut bytes = vec![0; RICH_REPORT_LEN];
        if self.status_next {
            bytes[0] = RICH_STATUS;
            bytes[RICH_COUNTER] = self.counter;
            self.counter = self.counter.wrapping_add(1);
            bytes[RICH_METER] = self.next_noise();
            for (slot, raw) in bytes[RICH_READBACK_BASE..RICH_PEAK_BASE].iter_mut().zip(&self.raw) {
                *slot = *raw;
            }
            for (i, slot) in bytes[RICH_PEAK_BASE..RICH_CHECKSUM].iter_mut().enumerate().take(self.raw.len()) {
                let rest = 96u8.wrapping_sub(self.raw[i].wrapping_mul(3));
                *slot = match self.dip[i] {
                    0 => rest,
                    d if d % 2 == 0 => rest.wrapping_sub(10),
                    _ => rest.wrapping_sub(4),
                };
                self.dip[i] = self.dip[i].saturating_sub(1);
            }
            bytes[RICH_CHECKSUM] = bytes[..RICH_CHECKSUM].iter().fold(0u8, |sum, b| sum.wrapping_add(*b));
        } else {
            bytes[0] = RICH_METERS;
            for slot in &mut bytes[1..] {
                *slot = self.next_noise();
            }
        }
        self.status_next = !self.status_next;
        let ev = self.event(t_ns, RICH_STATUS_ENDPOINT, Direction::In, TransferType::Interrupt, UrbStage::Complete);
        out.push(with_data(ev, bytes));
    }

    fn change(&mut self, t_ns: u64, parameter: &str, value: &str, out: &mut Vec<UsbEvent>) {
        let (Some(position), Some(raw)) = (self.position(parameter), self.raw_value(parameter, value)) else {
            return;
        };
        self.raw[position] = raw;
        self.dip[position] = RICH_DIP_REPORTS;
        self.changes += 1;
        let wire = if self.glitches.contains(&self.changes) { raw ^ RICH_GLITCH_BITS } else { raw };
        let set = self.command(RICH_SET, position, wire);
        self.send(t_ns, set, out);
        let commit = self.command(RICH_COMMIT, position, 0);
        self.send(t_ns + 2_000_000, commit, out);
        let affected: Vec<usize> = self.spillover.iter().filter(|(changed, _)| changed == parameter).filter_map(|(_, affected)| self.position(affected)).collect();
        for (k, other) in affected.into_iter().enumerate() {
            self.raw[other] ^= RICH_SPILL_BIT;
            self.dip[other] = RICH_DIP_REPORTS;
            let spill = self.command(RICH_SET, other, self.raw[other]);
            self.send(t_ns + 4_000_000 + k as u64 * 2_000_000, spill, out);
        }
    }

    fn touch(&mut self, t_ns: u64, parameter: &str, out: &mut Vec<UsbEvent>) {
        if let Some(position) = self.position(parameter) {
            let focus = self.command(RICH_FOCUS, position, 0);
            self.send(t_ns, focus, out);
        }
    }
}
