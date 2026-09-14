//! Live capture through `USBPcapCMD.exe` (Windows). Command lines, output parsing and target
//! discovery are platform-neutral and unit-tested; only process spawning is `cfg(windows)`.
//!
//! Flags used, from USBPcapCMD's `print_help` and extcap handlers: `-d <device>`,
//! `-o -` (stdout), `-A` (all devices on the root hub), `--inject-descriptors`, `-s <snaplen>`,
//! `-b <bufferlen>`, `--extcap-interfaces`, `--extcap-interface <device> --extcap-config`.
//! Its output is classic pcap, link type 249.

use std::path::{Path, PathBuf};

use super::decode;
use super::pipeline::DeviceMap;
use super::{CaptureError, CaptureSource, CaptureStream, FrameIter};

pub const DEFAULT_EXE: &str = r"C:\Program Files\USBPcap\USBPcapCMD.exe";
pub const DEFAULT_SNAPLEN: u32 = 65_535;
pub const DEFAULT_BUFFERLEN: u32 = 1_048_576;
/// SID of the High Mandatory Level group, present in `whoami /groups` when elevated.
pub const HIGH_MANDATORY_LEVEL_SID: &str = "S-1-16-12288";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HubInterface {
    /// Control device path, e.g. `\\.\USBPcap1`.
    pub value: String,
    pub display: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachedDevice {
    pub address: u16,
    pub display: String,
    pub parent: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsbPcapConfig {
    pub exe: PathBuf,
    pub hub: String,
    pub snaplen: u32,
    pub bufferlen: u32,
}

impl UsbPcapConfig {
    pub fn new(exe: impl Into<PathBuf>, hub: impl Into<String>) -> Self {
        Self { exe: exe.into(), hub: hub.into(), snaplen: DEFAULT_SNAPLEN, bufferlen: DEFAULT_BUFFERLEN }
    }
}

/// `{key=value}` pairs of one extcap line.
fn extcap_fields(line: &str) -> Vec<(&str, &str)> {
    line.split('{')
        .skip(1)
        .filter_map(|part| part.strip_suffix('}').or_else(|| part.split_once('}').map(|(a, _)| a)))
        .filter_map(|kv| kv.split_once('='))
        .collect()
}

fn field<'a>(fields: &[(&'a str, &'a str)], key: &str) -> Option<&'a str> {
    fields.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
}

/// Parses `USBPcapCMD.exe --extcap-interfaces` (`interface {value=…}{display=…}` lines).
pub fn parse_extcap_interfaces(stdout: &str) -> Vec<HubInterface> {
    stdout
        .lines()
        .filter(|l| l.starts_with("interface "))
        .filter_map(|l| {
            let f = extcap_fields(l);
            Some(HubInterface { value: field(&f, "value")?.to_string(), display: field(&f, "display")?.to_string() })
        })
        .collect()
}

/// Parses the device list of `--extcap-interface <hub> --extcap-config`. Lines whose value is
/// `address_node` describe interfaces of a device and are skipped. The display text is the
/// Windows device description; it does not carry VID/PID.
pub fn parse_extcap_devices(stdout: &str) -> Vec<AttachedDevice> {
    stdout
        .lines()
        .filter(|l| l.starts_with("value "))
        .filter_map(|l| {
            let f = extcap_fields(l);
            let address = field(&f, "value")?.parse().ok()?;
            let display = field(&f, "display")?;
            let display = display.split_once("] ").map_or(display, |(_, rest)| rest).to_string();
            Some(AttachedDevice { address, display, parent: field(&f, "parent").and_then(|p| p.parse().ok()) })
        })
        .collect()
}

pub fn extcap_interfaces_args() -> Vec<String> {
    vec!["--extcap-interfaces".into()]
}

pub fn extcap_config_args(hub: &str) -> Vec<String> {
    vec!["--extcap-interface".into(), hub.into(), "--extcap-config".into()]
}

/// Whole root hub to stdout with descriptors injected; the target is filtered in `Pipeline`.
pub fn capture_args(cfg: &UsbPcapConfig) -> Vec<String> {
    vec![
        "-d".into(),
        cfg.hub.clone(),
        "-o".into(),
        "-".into(),
        "-A".into(),
        "--inject-descriptors".into(),
        "-s".into(),
        cfg.snaplen.to_string(),
        "-b".into(),
        cfg.bufferlen.to_string(),
    ]
}

/// Fallback through Wireshark's `dumpcap` on the USBPcap extcap interface (e.g. `USBPcap1`),
/// classic pcap on stdout. Whether extcap defaults (`--inject-descriptors`) apply this way is
/// verified by the Windows checklist, not assumed.
pub fn dumpcap_args(interface_display: &str) -> Vec<String> {
    vec!["-i".into(), interface_display.into(), "-w".into(), "-".into(), "-F".into(), "pcap".into()]
}

pub fn elevated_from_whoami_groups(stdout: &str) -> bool {
    stdout.contains(HIGH_MANDATORY_LEVEL_SID)
}

/// Reads frames until the target's device descriptor appears. Injected descriptors carry
/// IRP id 0; the first live frame (non-zero IRP id) seen *after at least one descriptor has
/// been learned* ends the injected block. Gives up after `max_frames`.
pub fn discover_target(frames: FrameIter, vid: u16, pid: u16, max_frames: usize) -> Result<Option<(u16, u16)>, CaptureError> {
    let mut map = DeviceMap::default();
    let mut descriptor_seen = false;
    for (seen, frame) in frames.enumerate() {
        if seen >= max_frames {
            break;
        }
        let Ok(Some(ev)) = decode::decode(&frame?) else {
            continue;
        };
        map.observe(&ev);
        if let Some(address) = map.find(vid, pid) {
            return Ok(Some(address));
        }
        if map.get(ev.bus, ev.device).is_some() {
            descriptor_seen = true;
        }
        if ev.urb_id != 0 && descriptor_seen {
            break;
        }
    }
    Ok(None)
}

pub struct UsbPcapSource {
    cfg: UsbPcapConfig,
}

impl UsbPcapSource {
    pub fn new(cfg: UsbPcapConfig) -> Self {
        Self { cfg }
    }
}

impl CaptureSource for UsbPcapSource {
    fn describe(&self) -> String {
        format!("USBPcap {}", self.cfg.hub)
    }

    fn start(&mut self) -> Result<CaptureStream, CaptureError> {
        platform::start(&self.cfg)
    }
}

/// `Some(elevated)` on Windows; `None` elsewhere.
pub fn is_elevated() -> Option<bool> {
    platform::is_elevated()
}

/// Root hubs as reported by USBPcapCMD.
pub fn list_hubs(exe: &Path) -> Result<Vec<(HubInterface, Vec<AttachedDevice>)>, CaptureError> {
    platform::list_hubs(exe)
}

/// The control device of the root hub the target is attached to.
pub fn find_hub(exe: &Path, vid: u16, pid: u16) -> Result<Option<String>, CaptureError> {
    platform::find_hub(exe, vid, pid)
}

#[cfg(windows)]
mod platform {
    use std::path::Path;
    use std::process::{Command, Stdio};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use super::*;
    use crate::capture::import::pcap_frames;
    use crate::capture::StopHandle;

    /// How long hub discovery listens to one hub.
    const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(3);

    pub fn start(cfg: &UsbPcapConfig) -> Result<CaptureStream, CaptureError> {
        let mut child = Command::new(&cfg.exe)
            .args(capture_args(cfg))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;
        let stdout = child.stdout.take().ok_or_else(|| CaptureError::Tool("USBPcapCMD has no stdout".into()))?;
        let child = Arc::new(Mutex::new(child));
        let for_stop = Arc::clone(&child);
        let stop = StopHandle::new(move || {
            let mut c = for_stop.lock().unwrap_or_else(|p| p.into_inner());
            let _ = c.kill();
            let _ = c.wait();
        });
        match pcap_frames(stdout) {
            Ok(frames) => Ok(CaptureStream { frames, stop }),
            Err(e) => {
                stop.stop();
                Err(e)
            }
        }
    }

    fn run_text(exe: &Path, args: &[String]) -> Result<String, CaptureError> {
        let out = Command::new(exe).args(args).stdin(Stdio::null()).output()?;
        if !out.status.success() {
            return Err(CaptureError::Tool(String::from_utf8_lossy(&out.stderr).into_owned()));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    pub fn is_elevated() -> Option<bool> {
        let out = Command::new("whoami").arg("/groups").output().ok()?;
        Some(elevated_from_whoami_groups(&String::from_utf8_lossy(&out.stdout)))
    }

    pub fn list_hubs(exe: &Path) -> Result<Vec<(HubInterface, Vec<AttachedDevice>)>, CaptureError> {
        let hubs = parse_extcap_interfaces(&run_text(exe, &extcap_interfaces_args())?);
        hubs.into_iter()
            .map(|hub| {
                let devices = parse_extcap_devices(&run_text(exe, &extcap_config_args(&hub.value))?);
                Ok((hub, devices))
            })
            .collect()
    }

    pub fn find_hub(exe: &Path, vid: u16, pid: u16) -> Result<Option<String>, CaptureError> {
        for hub in parse_extcap_interfaces(&run_text(exe, &extcap_interfaces_args())?) {
            let stream = start(&UsbPcapConfig::new(exe, hub.value.clone()))?;
            let stop = Arc::new(Mutex::new(Some(stream.stop)));
            let watchdog_stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                std::thread::sleep(DISCOVERY_TIMEOUT);
                if let Some(s) = watchdog_stop.lock().unwrap_or_else(|p| p.into_inner()).take() {
                    s.stop();
                }
            });
            let found = discover_target(stream.frames, vid, pid, 4096);
            if let Some(s) = stop.lock().unwrap_or_else(|p| p.into_inner()).take() {
                s.stop();
            }
            if found?.is_some() {
                return Ok(Some(hub.value));
            }
        }
        Ok(None)
    }
}

#[cfg(not(windows))]
mod platform {
    use std::path::Path;

    use super::*;

    const WHY: &str = "USBPcap live capture requires Windows; use import or synthetic sessions";

    pub fn start(_cfg: &UsbPcapConfig) -> Result<CaptureStream, CaptureError> {
        Err(CaptureError::Unsupported(WHY.into()))
    }

    pub fn is_elevated() -> Option<bool> {
        None
    }

    pub fn list_hubs(_exe: &Path) -> Result<Vec<(HubInterface, Vec<AttachedDevice>)>, CaptureError> {
        Err(CaptureError::Unsupported(WHY.into()))
    }

    pub fn find_hub(_exe: &Path, _vid: u16, _pid: u16) -> Result<Option<String>, CaptureError> {
        Err(CaptureError::Unsupported(WHY.into()))
    }
}
