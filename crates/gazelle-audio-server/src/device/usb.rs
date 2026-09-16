//! The USB backend: the devices as Windows sees them, HID-class.
//!
//! Both models expose a vendor-defined HID interface (usage page `0xffa0`, interface 3), so the
//! OS HID driver owns them and raw USB cannot bind them (`reference/usb-access.md`). This backend
//! therefore speaks HID through `hidapi`.
//!
//! Two facts from hardware session 2, both measured on the attached devices:
//! - **Antelope's Manager Service holds the devices exclusively.** While it runs, even opening a
//!   device with no access rights fails, so enumeration finds nothing. It has to be stopped before
//!   this backend can attach.
//! - **Reports are 320 bytes with report id 0**, and the device may wrap a report in an 8053
//!   receive segment, which [`HostReceiver`] reassembles.
//!
//! Reads are drained when the worker polls; the OS buffers input reports meanwhile, so no reader
//! thread is needed and the [`Device`] trait stays synchronous (decision 0006).

use std::collections::VecDeque;

use gazelle_audio_protocol::wire::WireError;
use gazelle_audio_transport::{Device, HostReceiver, RawPacket, Report};
use hidapi::{DeviceInfo, HidApi, HidDevice, HidError};

use crate::device::hotplug::{Enumerator, Found};

/// Antelope's USB vendor id, 9189.
pub const ANTELOPE_USB_VID: u16 = 0x23e5;

/// The devices' vendor-defined HID usage page; their audio interfaces use other pages.
pub const ANTELOPE_USAGE_PAGE: u16 = 0xffa0;

/// The HID report size both models use, measured on the hardware (session 2).
pub const REPORT_SIZE: usize = 320;

/// Whether a HID interface is one of ours to drive: an Antelope device's control interface.
pub fn is_control_interface(vendor_id: u16, usage_page: u16) -> bool {
    vendor_id == ANTELOPE_USB_VID && usage_page == ANTELOPE_USAGE_PAGE
}

/// The bytes written for a report: HID report id 0, then the report, zero-padded to `size`.
/// A report longer than `size` is refused rather than truncated, since the device would read
/// the tail as another report's bytes.
pub fn write_buffer(report: &Report, size: usize) -> Result<Vec<u8>, WireError> {
    let mut bytes = report.header.to_bytes().to_vec();
    bytes.extend_from_slice(&report.contents);
    if bytes.len() > size {
        return Err(WireError::PayloadTooLarge);
    }
    let mut buf = Vec::with_capacity(size + 1);
    buf.push(0);
    buf.extend_from_slice(&bytes);
    buf.resize(size + 1, 0);
    Ok(buf)
}

/// One attached device, opened through the OS HID stack.
pub struct UsbDevice {
    device: HidDevice,
    vid: u16,
    pid: u16,
    serial: Option<String>,
    receiver: HostReceiver,
    pending: VecDeque<Report>,
    /// False once a read or write has failed. On Windows a HID handle does not come back after
    /// the device is unplugged, so the device is not used again; the rescan opens it afresh.
    connected: bool,
}

impl UsbDevice {
    /// Opens a device listed by [`discover`].
    pub fn open(api: &HidApi, info: &DeviceInfo) -> Result<Self, HidError> {
        let device = info.open_device(api)?;
        device.set_blocking_mode(false)?;
        Ok(UsbDevice {
            device,
            vid: info.vendor_id(),
            pid: info.product_id(),
            serial: info.serial_number().map(str::to_string),
            receiver: HostReceiver::new(),
            pending: VecDeque::new(),
            connected: true,
        })
    }

    /// The device's serial, when the HID stack reports one; the device id uses it.
    pub fn serial(&self) -> Option<&str> {
        self.serial.as_deref()
    }

    /// A read or write failed: the device is unplugged, or as good as. Said once.
    fn lost(&mut self, what: &str, e: HidError) {
        if self.connected {
            tracing::warn!(vid = self.vid, pid = self.pid, "HID {what} failed, treating the device as gone: {e}");
        }
        self.connected = false;
    }

    /// Reads whatever the OS has buffered, without blocking, and reassembles it.
    fn drain(&mut self) {
        let mut buf = [0u8; REPORT_SIZE];
        while self.connected {
            match self.device.read_timeout(&mut buf, 0) {
                Ok(0) => return,
                Ok(len) => {
                    if let Some(report) = self.receiver.push(&buf[..len]) {
                        self.pending.push_back(report);
                    }
                }
                // Before, this warned and returned, and the next poll 5 ms later failed and warned
                // again: an unplugged device logged 200 lines a second and stayed attached.
                Err(e) => self.lost("read", e),
            }
        }
    }
}

impl Device for UsbDevice {
    fn send(&mut self, report: &Report) -> Result<bool, WireError> {
        let buf = write_buffer(report, REPORT_SIZE)?;
        if !self.connected {
            return Ok(false);
        }
        match self.device.write(&buf) {
            Ok(_) => Ok(true),
            Err(e) => {
                self.lost("write", e);
                Ok(false)
            }
        }
    }

    /// Ignored: a real device's reports come from the wire. The worker feeds each segment it sends
    /// back in for the loopback, which must not be mistaken for a reply here.
    fn on_received_data(&mut self, _packet: RawPacket) {}

    fn max_packet_size(&self) -> usize {
        REPORT_SIZE
    }

    fn vid(&self) -> u16 {
        self.vid
    }

    fn pid(&self) -> u16 {
        self.pid
    }

    fn poll_reports(&mut self) -> Vec<Report> {
        self.drain();
        self.pending.drain(..).collect()
    }

    fn is_connected(&self) -> bool {
        self.connected
    }
}

/// Every Antelope control interface the HID stack can open, in enumeration order.
///
/// Returns an empty list when Antelope's Manager Service holds the devices: it opens them
/// exclusively, so they cannot even be enumerated. The caller says so rather than reporting
/// "no devices attached".
pub fn discover(api: &HidApi) -> Vec<&DeviceInfo> {
    api.device_list()
        .filter(|info| is_control_interface(info.vendor_id(), info.usage_page()))
        .collect()
}

/// The HID stack as the hot-plug scanner sees it. Listing is read-only; only [`Enumerator::open`]
/// opens a device.
pub struct HidEnumerator {
    api: HidApi,
}

impl HidEnumerator {
    pub fn new(api: HidApi) -> Self {
        HidEnumerator { api }
    }
}

impl Enumerator for HidEnumerator {
    fn list(&mut self) -> Result<Vec<Found>, String> {
        self.api.refresh_devices().map_err(|e| e.to_string())?;
        Ok(discover(&self.api)
            .into_iter()
            .map(|info| Found {
                vid: info.vendor_id(),
                pid: info.product_id(),
                serial: info.serial_number().map(str::to_string),
                path: info.path().to_string_lossy().into_owned(),
            })
            .collect())
    }

    fn open(&mut self, found: &Found) -> Result<Box<dyn Device + Send>, String> {
        let info = discover(&self.api)
            .into_iter()
            .find(|info| info.path().to_string_lossy() == found.path.as_str())
            .ok_or("no longer listed")?;
        let device = UsbDevice::open(&self.api, info).map_err(|e| e.to_string())?;
        Ok(Box::new(device))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gazelle_audio_protocol::wire::{Header, HEADER_SIZE};

    fn report(cmd: u32, contents: Vec<u8>) -> Report {
        Report { header: Header::new(cmd, contents.len() as u32 + HEADER_SIZE as u32, 0, 0), contents }
    }

    #[test]
    fn a_written_report_carries_report_id_zero_and_is_padded_to_the_report_size() {
        let buf = write_buffer(&report(0x70, vec![1, 2, 3]), REPORT_SIZE).unwrap();
        assert_eq!(buf.len(), REPORT_SIZE + 1, "the HID report id precedes the report");
        assert_eq!(buf[0], 0);
        assert_eq!(u32::from_le_bytes([buf[1], buf[2], buf[3], buf[4]]), 0x70);
        assert_eq!(&buf[17..20], &[1, 2, 3]);
        assert!(buf[20..].iter().all(|&b| b == 0), "the rest is zero padding");
    }

    #[test]
    fn a_report_too_long_for_one_packet_is_refused_not_truncated() {
        let long = report(0x70, vec![9; REPORT_SIZE]);
        assert!(matches!(write_buffer(&long, REPORT_SIZE), Err(WireError::PayloadTooLarge)));
        // Exactly one packet still fits.
        assert!(write_buffer(&report(0x70, vec![9; REPORT_SIZE - HEADER_SIZE]), REPORT_SIZE).is_ok());
    }

    #[test]
    fn only_the_vendor_control_interface_is_ours() {
        assert!(is_control_interface(ANTELOPE_USB_VID, ANTELOPE_USAGE_PAGE));
        assert!(!is_control_interface(ANTELOPE_USB_VID, 0x0001), "the audio interfaces are not ours");
        assert!(!is_control_interface(0x046d, ANTELOPE_USAGE_PAGE), "another vendor's device is not ours");
    }
}
