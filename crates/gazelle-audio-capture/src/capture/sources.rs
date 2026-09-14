//! Choosing and building the capture source for a probe, shared by `gazelle-capture serve` and
//! the `start_probe` operation.

use std::path::PathBuf;

use super::import::{ImportSource, MemorySource};
use super::usbpcap::{self, UsbPcapConfig, UsbPcapSource, DEFAULT_EXE};
use super::{CaptureError, CaptureSource};
use crate::session::clock::{Clock, SystemClock};
use crate::synth::device::{DeviceModel, SimpleDevice};
use crate::synth::frames::device_frames;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SourceKind {
    /// Live capture through USBPcapCMD (Windows).
    #[default]
    Usbpcap,
    /// Replay a `.pcap`/`.pcapng` file.
    Import,
    /// A minute of synthetic traffic for the target and one neighbour.
    Demo,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceSettings {
    pub kind: SourceKind,
    /// Capture file for [`SourceKind::Import`].
    pub file: Option<PathBuf>,
    /// USBPcap control device, e.g. `\\.\USBPcap1`; found by VID/PID when `None`.
    pub hub: Option<String>,
    pub usbpcap_exe: PathBuf,
}

impl Default for SourceSettings {
    fn default() -> Self {
        Self { kind: SourceKind::default(), file: None, hub: None, usbpcap_exe: PathBuf::from(DEFAULT_EXE) }
    }
}

/// Builds the source for a target. USBPcap hub discovery can take a few seconds per hub.
pub fn build_source(settings: &SourceSettings, vid: u16, pid: u16) -> Result<Box<dyn CaptureSource>, CaptureError> {
    Ok(match settings.kind {
        SourceKind::Usbpcap => {
            let hub = match &settings.hub {
                Some(h) => h.clone(),
                None => usbpcap::find_hub(&settings.usbpcap_exe, vid, pid)?
                    .ok_or_else(|| CaptureError::Tool(format!("no USBPcap root hub has a device {vid:04x}:{pid:04x}")))?,
            };
            Box::new(UsbPcapSource::new(UsbPcapConfig::new(&settings.usbpcap_exe, hub)))
        }
        SourceKind::Import => {
            let file = settings.file.clone().ok_or_else(|| CaptureError::Tool("--source import needs --file".into()))?;
            Box::new(ImportSource::new(file))
        }
        SourceKind::Demo => {
            let mut devices: Vec<Box<dyn DeviceModel>> = vec![Box::new(SimpleDevice::new(vid, pid, 1, 5)), Box::new(SimpleDevice::new(0x046D, 0xC52B, 1, 3))];
            Box::new(MemorySource::new("demo", device_frames(&mut devices, SystemClock.now_ns(), 60_000_000_000)))
        }
    })
}
