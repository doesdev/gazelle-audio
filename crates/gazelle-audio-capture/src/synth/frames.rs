//! Raw frames from simulated devices without a probe: capture-source input for controller and
//! panel tests and for `serve --source demo`.

use crate::capture::decode::{usbpcap, ByteOrder};
use crate::capture::RawFrame;

use super::device::DeviceModel;

/// Enumeration at `start_ns`, then every device's periodic traffic for `duration_ns`, as
/// USBPcap frames in timestamp order.
pub fn device_frames(devices: &mut [Box<dyn DeviceModel>], start_ns: u64, duration_ns: u64) -> Vec<RawFrame> {
    let mut events = Vec::new();
    for d in devices.iter_mut() {
        d.enumerate(start_ns, &mut events);
        let mut t = start_ns + d.tick_ns();
        while t < start_ns + duration_ns {
            d.tick(t, &mut events);
            t += d.tick_ns();
        }
    }
    events.sort_by_key(|e| e.ts_ns);
    events
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let data = usbpcap::encode(e);
            RawFrame {
                ts_ns: e.ts_ns,
                link_type: usbpcap::LINKTYPE_USBPCAP,
                index: i as u64,
                orig_len: data.len() as u32,
                byte_order: ByteOrder::Little,
                data,
            }
        })
        .collect()
}
