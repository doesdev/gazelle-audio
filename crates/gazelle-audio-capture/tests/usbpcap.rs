use std::io::Cursor;

use gazelle_audio_capture::capture::decode::usbpcap::encode;
use gazelle_audio_capture::capture::decode::ByteOrder;
use gazelle_audio_capture::capture::event::{Direction, SetupPacket, TransferType, UrbStage, UsbEvent};
use gazelle_audio_capture::capture::import::pcap_frames;
use gazelle_audio_capture::capture::usbpcap::*;
use gazelle_audio_capture::capture::writer::write_pcap;
use gazelle_audio_capture::capture::{CaptureSource, FrameIter, RawFrame};
use gazelle_audio_capture::synth::device::{DeviceModel, SimpleDevice};
use gazelle_audio_capture::synth::frames::device_frames;

/// Format of `print_extcap_interfaces` in USBPcapCMD/cmd.c.
const INTERFACES: &str = "interface {value=\\\\.\\USBPcap1}{display=USBPcap1}\r\ninterface {value=\\\\.\\USBPcap2}{display=USBPcap2}\r\n";

/// Format of `print_extcap_options` + `print_extcap_config` in USBPcapCMD/{cmd,enum}.c.
const CONFIG: &str = "arg {number=0}{call=--snaplen}{display=Snapshot length}{tooltip=Snapshot length}{type=unsigned}{default=65535}
arg {number=4}{call=--inject-descriptors}{display=Inject already connected devices descriptors into capture data}{type=boolflag}{default=true}
value {arg=99}{value=1}{display=[1] Generic USB Hub}{enabled=true}
value {arg=99}{value=5}{display=[5] USB Composite Device}{enabled=true}{parent=1}
value {arg=99}{value=5_1}{display=USB Audio Device}{enabled=false}{parent=5}
";

#[test]
fn parses_hub_interfaces() {
    assert_eq!(
        parse_extcap_interfaces(INTERFACES),
        vec![
            HubInterface { value: r"\\.\USBPcap1".into(), display: "USBPcap1".into() },
            HubInterface { value: r"\\.\USBPcap2".into(), display: "USBPcap2".into() },
        ]
    );
}

#[test]
fn parses_attached_devices_and_skips_interface_nodes() {
    assert_eq!(
        parse_extcap_devices(CONFIG),
        vec![
            AttachedDevice { address: 1, display: "Generic USB Hub".into(), parent: None },
            AttachedDevice { address: 5, display: "USB Composite Device".into(), parent: Some(1) },
        ]
    );
}

#[test]
fn command_lines_are_exact() {
    let cfg = UsbPcapConfig::new(DEFAULT_EXE, r"\\.\USBPcap2");
    assert_eq!(
        capture_args(&cfg),
        ["-d", r"\\.\USBPcap2", "-o", "-", "-A", "--inject-descriptors", "-s", "65535", "-b", "1048576"]
    );
    assert_eq!(extcap_interfaces_args(), ["--extcap-interfaces"]);
    assert_eq!(extcap_config_args(r"\\.\USBPcap2"), ["--extcap-interface", r"\\.\USBPcap2", "--extcap-config"]);
    assert_eq!(dumpcap_args("USBPcap2"), ["-i", "USBPcap2", "-w", "-", "-F", "pcap"]);
}

#[test]
fn elevation_from_whoami_groups() {
    let elevated = "Mandatory Label\\High Mandatory Level       Label            S-1-16-12288\r\n";
    let normal = "Mandatory Label\\Medium Mandatory Level     Label            S-1-16-8192\r\n";
    assert!(elevated_from_whoami_groups(elevated));
    assert!(!elevated_from_whoami_groups(normal));
}

fn hub_pcap() -> Vec<u8> {
    let mut devices: Vec<Box<dyn DeviceModel>> = vec![Box::new(SimpleDevice::new(0x046D, 0xC52B, 2, 3)), Box::new(SimpleDevice::new(0x1234, 0xABCD, 2, 7))];
    let mut frames = device_frames(&mut devices, 1_700_000_000_000_000_000, 1_000_000_000);
    // Injected descriptors have IRP id 0 (USBPcapCMD/descriptors.c); live traffic does not.
    for f in frames.iter_mut() {
        let injected = f.data[22] == 2;
        if injected {
            f.data[2..10].copy_from_slice(&0u64.to_le_bytes());
        } else {
            f.data[2..10].copy_from_slice(&0xFFFF_8000_0000_1000u64.to_le_bytes());
        }
    }
    write_pcap(Vec::new(), 249, ByteOrder::Little, &frames).unwrap()
}

#[test]
fn discovers_the_target_address_from_injected_descriptors() {
    let frames = pcap_frames(Cursor::new(hub_pcap())).unwrap();
    assert_eq!(discover_target(frames, 0x1234, 0xABCD, 4096).unwrap(), Some((2, 7)));
}

#[test]
fn discovery_stops_after_the_injected_block_or_frame_budget() {
    let frames = pcap_frames(Cursor::new(hub_pcap())).unwrap();
    assert_eq!(discover_target(frames, 0xFFFF, 0x0001, 4096).unwrap(), None);
    let frames = pcap_frames(Cursor::new(hub_pcap())).unwrap();
    assert_eq!(discover_target(frames, 0x1234, 0xABCD, 2).unwrap(), None);
}

/// Builds a minimal USBPcap [`RawFrame`] from a [`UsbEvent`], bypassing pcap encoding.
fn frame(index: u64, ev: &UsbEvent) -> RawFrame {
    let data = encode(ev);
    RawFrame { ts_ns: ev.ts_ns, link_type: 249, index, orig_len: data.len() as u32, byte_order: ByteOrder::Little, data }
}

/// Amendment (plan-amendments.md, Task 13): discovery must stop at the first live frame only
/// after at least one descriptor has been learned, not merely after the first frame overall
/// (the old `seen > 0` condition). This constructs two live (non-injected) frames *before* any
/// descriptor has been recorded, followed by the target's descriptor exchange. With `seen > 0`,
/// the second live frame (seen == 1, which is already > 0) wrongly stops discovery before the
/// target's descriptor is ever read, even though no descriptor has been learned yet.
#[test]
fn discovery_does_not_stop_on_a_live_frame_before_any_descriptor_is_learned() {
    // Two live interrupt completions for an unrelated device, no descriptor learned from either.
    let live = |urb_id: u64| UsbEvent {
        ts_ns: 1_700_000_000_000_000_000,
        packet_index: 0,
        bus: 2,
        device: 9,
        endpoint: 1,
        direction: Direction::In,
        transfer: TransferType::Interrupt,
        stage: UrbStage::Complete,
        urb_id,
        setup: None,
        status: 0,
        data_len: 4,
        data: vec![0xA0, 0, 0, 0],
        payload_dropped: false,
    };
    // The target's injected descriptor exchange (IRP id 0), arriving after the live frames.
    let mut req = UsbEvent {
        ts_ns: 1_700_000_000_000_000_001,
        packet_index: 0,
        bus: 2,
        device: 7,
        endpoint: 0,
        direction: Direction::In,
        transfer: TransferType::Control,
        stage: UrbStage::Submit,
        urb_id: 0,
        setup: Some(SetupPacket { request_type: 0x80, request: 6, value: 0x0100, index: 0, length: 18 }),
        status: 0,
        data_len: 0,
        data: Vec::new(),
        payload_dropped: false,
    };
    let mut done = req.clone();
    done.stage = UrbStage::Complete;
    done.setup = None;
    done.ts_ns += 1;
    let descriptor = vec![18, 1, 0x00, 0x02, 0xEF, 0x02, 0x01, 64, 0x34, 0x12, 0xCD, 0xAB, 0x00, 0x01, 1, 2, 3, 1];
    done.data_len = descriptor.len() as u32;
    done.data = descriptor;
    req.urb_id = 0; // injected: descriptors USBPcap injects carry IRP id 0.

    let events = [live(1), live(2), req, done];
    let frames: FrameIter = Box::new(events.iter().enumerate().map(|(i, e)| Ok(frame(i as u64, e))).collect::<Vec<_>>().into_iter());
    assert_eq!(discover_target(frames, 0x1234, 0xABCD, 4096).unwrap(), Some((2, 7)));
}

#[cfg(not(windows))]
#[test]
fn live_capture_is_unsupported_off_windows() {
    let mut source = UsbPcapSource::new(UsbPcapConfig::new(DEFAULT_EXE, r"\\.\USBPcap1"));
    assert_eq!(source.describe(), r"USBPcap \\.\USBPcap1");
    assert!(matches!(source.start(), Err(gazelle_audio_capture::capture::CaptureError::Unsupported(_))));
    assert_eq!(is_elevated(), None);
}
