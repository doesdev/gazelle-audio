//! Live capture through `USBPcapCMD.exe` (Windows). Command lines, output parsing and target
//! discovery are platform-neutral and unit-tested; only process spawning is `cfg(windows)`.
//!
//! Flags used, from USBPcapCMD's `print_help` and extcap handlers: `-d <device>`,
//! `-o -` (stdout), `-A` (all devices on the root hub), `--inject-descriptors`, `-s <snaplen>`,
//! `-b <bufferlen>`, `--extcap-interfaces`, `--extcap-interface <device> --extcap-config`.
//! Its output is classic pcap, link type 249.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::decode;
use super::pipeline::DeviceMap;
use super::{CaptureError, CaptureSource, CaptureStream, FrameIter, RawFrame};

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

/// Maps a live child's exit to a final tool error, or `None` if nothing should be reported.
/// A requested stop, a still-running process (`code: None`, couldn't be reaped), or a clean
/// exit (`code: Some(0)`) all report nothing; any other exit is a tool failure worth surfacing.
#[cfg_attr(not(windows), allow(dead_code, reason = "only called from cfg(windows) platform::start; exercised directly by usbpcap_tests.rs on every platform"))]
fn exit_tool_error(stopped: bool, code: Option<i32>) -> Option<CaptureError> {
    if stopped {
        return None;
    }
    match code {
        Some(0) | None => None,
        Some(code) => Some(CaptureError::Tool(format!("USBPcapCMD exited with status {code}"))),
    }
}

/// Wraps a live source's frame iterator so that:
/// - an error surfacing after `stop()` was requested (typically a truncated pcap record from
///   killing the process mid-write) ends the stream cleanly instead of propagating as an error;
/// - once the stream ends on its own, `reap` (the only platform-specific part — `Child::wait` in
///   the live backend) is called exactly once, and a non-zero, unrequested exit is reported as
///   one final `Err` before the stream truly ends.
#[cfg_attr(not(windows), allow(dead_code, reason = "only constructed by cfg(windows) platform::start; exercised directly by usbpcap_tests.rs on every platform"))]
struct Supervised<F> {
    inner: FrameIter,
    stopped: Arc<AtomicBool>,
    reap: F,
    reaped: bool,
}

impl<F: FnMut() -> Option<i32> + Send> Iterator for Supervised<F> {
    type Item = Result<RawFrame, CaptureError>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.inner.next() {
            Some(Err(_)) if self.stopped.load(Ordering::Relaxed) => None,
            Some(item) => Some(item),
            None if self.reaped => None,
            None => {
                self.reaped = true;
                let stopped = self.stopped.load(Ordering::Relaxed);
                exit_tool_error(stopped, (self.reap)()).map(Err)
            }
        }
    }
}

/// Tries hubs in order via `probe`, which reports whether the target device was found on that
/// hub. A hub that fails to start or errors during discovery is skipped and its error
/// remembered, so one bad hub does not abort the search. Returns the first hub whose probe
/// finds the target; if none do, returns the last such error, or `Ok(None)` if none occurred.
#[cfg_attr(not(windows), allow(dead_code, reason = "only called from cfg(windows) platform::find_hub; exercised directly by usbpcap_tests.rs on every platform"))]
fn select_hub(hubs: Vec<HubInterface>, mut probe: impl FnMut(&HubInterface) -> Result<bool, CaptureError>) -> Result<Option<String>, CaptureError> {
    let mut last_err = None;
    for hub in hubs {
        match probe(&hub) {
            Ok(true) => return Ok(Some(hub.value)),
            Ok(false) => {}
            Err(e) => last_err = Some(e),
        }
    }
    last_err.map_or(Ok(None), Err)
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
    use std::os::windows::process::CommandExt;
    use std::path::Path;
    use std::process::{Command, Stdio};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use super::*;
    use crate::capture::import::pcap_frames;
    use crate::capture::StopHandle;

    /// How long hub discovery listens to one hub.
    const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(3);

    /// winbase.h `CREATE_NEW_PROCESS_GROUP`. USBPcapCMD would otherwise share our console, so a
    /// console Ctrl-C reaches it directly (it may die before `stop()` sets `stopped`, so the
    /// capture thread reads that as an unrequested exit — `CaptureError::Tool` — racing
    /// `writer.finish()`). Running it in its own process group makes our own Ctrl-C handler
    /// (`main.rs`'s `serve`, via `stop()`) the only path that stops it.
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

    pub fn start(cfg: &UsbPcapConfig) -> Result<CaptureStream, CaptureError> {
        let mut child = Command::new(&cfg.exe)
            .args(capture_args(cfg))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .creation_flags(CREATE_NEW_PROCESS_GROUP)
            .spawn()?;
        let stdout = child.stdout.take().ok_or_else(|| CaptureError::Tool("USBPcapCMD has no stdout".into()))?;
        let child = Arc::new(Mutex::new(child));
        let stopped = Arc::new(AtomicBool::new(false));
        let for_stop = Arc::clone(&child);
        let stopped_for_stop = Arc::clone(&stopped);
        let stop = StopHandle::new(move || {
            // Set before killing: a truncated record the kill causes then reads as a clean end
            // (Supervised checks this flag), and the end-of-stream reap below skips reporting an
            // exit the operator asked for.
            stopped_for_stop.store(true, Ordering::Relaxed);
            let mut c = for_stop.lock().unwrap_or_else(|p| p.into_inner());
            let _ = c.kill();
            let _ = c.wait();
        });
        match pcap_frames(stdout) {
            Ok(frames) => {
                let for_reap = Arc::clone(&child);
                let reap = move || for_reap.lock().unwrap_or_else(|p| p.into_inner()).wait().ok().and_then(|s| s.code());
                let supervised: FrameIter = Box::new(Supervised { inner: frames, stopped: Arc::clone(&stopped), reap, reaped: false });
                Ok(CaptureStream { frames: supervised, stop })
            }
            Err(e) => {
                // The header read failed, most likely because USBPcapCMD exited before writing
                // any output (bad device path, no permission, ...). Reap it directly here rather
                // than through `stop` (which would mark this a requested stop): kill defensively
                // first, in case it's still running for some other reason, then prefer its exit
                // status over the raw pcap-parse error, which would otherwise read as a
                // confusing "capture file: ..." message with no sign the tool itself failed.
                let mut c = child.lock().unwrap_or_else(|p| p.into_inner());
                let _ = c.kill();
                let code = c.wait().ok().and_then(|s| s.code());
                drop(c);
                Err(exit_tool_error(false, code).unwrap_or(e))
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
        let hubs = parse_extcap_interfaces(&run_text(exe, &extcap_interfaces_args())?);
        select_hub(hubs, |hub| probe_hub(exe, hub, vid, pid))
    }

    /// Starts a capture on `hub` and watches it for up to [`DISCOVERY_TIMEOUT`], reporting
    /// whether the target's descriptor appeared. A start or discovery failure is returned to the
    /// caller, which decides (in [`select_hub`]) whether to keep trying other hubs.
    fn probe_hub(exe: &Path, hub: &HubInterface, vid: u16, pid: u16) -> Result<bool, CaptureError> {
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
        Ok(found?.is_some())
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

#[cfg(test)]
#[path = "usbpcap_tests.rs"]
mod tests;
