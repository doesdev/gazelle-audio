//! Devices that come and go while the server runs.
//!
//! The USB backend lists Antelope control interfaces every [`RESCAN_INTERVAL`], attaches the ones
//! it has not got, and detaches the ones no longer listed. A device unplugged mid-run is usually
//! noticed sooner by its own worker, whose next read fails ([`Device::is_connected`]); the manager
//! then detaches it, and a later scan attaches it again once it is back.
//!
//! Listing and opening sit behind [`Enumerator`], so every rule here is tested with a fake bus;
//! only `usb::HidEnumerator` touches the HID stack.
//!
//! A device that is listed but will not open (another program holds it) is tried again on every
//! scan and reported once. What each scan changed comes back as [`Change`]s, which the caller
//! logs, so the log says what happened once rather than every two seconds.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use gazelle_audio_transport::Device;

use crate::device::descriptor::{DeviceDescriptor, DeviceId};
use crate::device::manager::DeviceManager;

/// How often the USB backend looks for devices.
///
/// Two seconds: a device plugged in, or freed by stopping Antelope's service, shows up about as
/// soon as a person looks for it (the devices themselves take a few seconds to start after being
/// plugged in), while a listing, which opens each HID interface on the system briefly with no
/// access rights, costs milliseconds, well under 1% of a core at this rate. An unplug is noticed
/// sooner by the device's worker, within one 5 ms poll.
pub const RESCAN_INTERVAL: Duration = Duration::from_secs(2);

/// The backend name devices attached here carry.
const BACKEND: &str = "usb";

/// A control interface as listed, before it is opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub vid: u16,
    pub pid: u16,
    pub serial: Option<String>,
    /// The OS's path for the interface; what identifies a device that reports no serial.
    pub path: String,
}

/// Lists and opens devices. The HID stack in production, a fake bus in tests.
pub trait Enumerator: Send {
    /// Every control interface present now.
    fn list(&mut self) -> Result<Vec<Found>, String>;
    /// Open one interface from the latest listing.
    fn open(&mut self, found: &Found) -> Result<Box<dyn Device + Send>, String>;
}

/// Something a scan changed, to be said once.
#[derive(Clone, Debug)]
pub enum Change {
    Attached(DeviceDescriptor),
    /// No longer listed: unplugged.
    Detached(DeviceId),
    /// Listed but would not open. Tried again every scan; reported again only after it has
    /// opened or stopped being listed.
    OpenFailed { id: DeviceId, error: String },
    /// Nothing listed, at the first scan or after the last device went.
    NoneFound,
    /// The listing itself failed; nothing is attached or detached until it works again.
    ListFailed(String),
    ListRecovered,
}

impl Change {
    pub fn log(&self) {
        match self {
            Change::Attached(d) => tracing::info!(
                "attached {} ({:04x}:{:04x}) as {}",
                d.model.as_deref().unwrap_or("an unknown model"),
                d.vid,
                d.pid,
                d.id
            ),
            Change::Detached(id) => tracing::info!("detached {id}: no longer connected"),
            Change::OpenFailed { id, error } => tracing::warn!(
                "{id} is connected but could not be opened ({error}); trying again every {}s. \
                 If Antelope Manager Service is running, it holds the device.",
                RESCAN_INTERVAL.as_secs()
            ),
            Change::NoneFound => tracing::warn!(
                "no Antelope device found; looking again every {}s. If one is attached, Antelope's own \
                 software is probably holding it: the Antelope Manager Service opens the devices \
                 exclusively, so nothing else can even list them. Stopping that service lets them attach.",
                RESCAN_INTERVAL.as_secs()
            ),
            Change::ListFailed(error) => tracing::warn!("listing HID devices failed: {error}"),
            Change::ListRecovered => tracing::info!("listing HID devices works again"),
        }
    }
}

/// The scanning rules, one scan at a time.
pub struct Scanner<E> {
    enumerator: E,
    /// What this scanner attached and has not seen go.
    attached: BTreeSet<DeviceId>,
    /// Listed devices whose last open failed, already reported.
    failing: BTreeSet<DeviceId>,
    /// Ids given to devices without a serial, by path, kept for the server's life.
    unserialed: BTreeMap<String, DeviceId>,
    /// Whether the last listing found anything; `None` before the first.
    found_any: Option<bool>,
    list_failed: bool,
}

impl<E: Enumerator> Scanner<E> {
    pub fn new(enumerator: E) -> Self {
        Scanner {
            enumerator,
            attached: BTreeSet::new(),
            failing: BTreeSet::new(),
            unserialed: BTreeMap::new(),
            found_any: None,
            list_failed: false,
        }
    }

    /// List once, and attach and detach to match.
    pub fn scan(&mut self, devices: &Arc<DeviceManager>) -> Vec<Change> {
        let mut changes = Vec::new();
        let listed = match self.enumerator.list() {
            Ok(listed) => {
                if std::mem::take(&mut self.list_failed) {
                    changes.push(Change::ListRecovered);
                }
                listed
            }
            Err(error) => {
                // Detaching everything on a bad listing would drop devices that are working.
                if !std::mem::replace(&mut self.list_failed, true) {
                    changes.push(Change::ListFailed(error));
                }
                return changes;
            }
        };

        let mut present: BTreeMap<DeviceId, (Found, bool)> = BTreeMap::new();
        for found in listed {
            let (id, stable) = self.identify(&found);
            present.entry(id).or_insert((found, stable));
        }

        // A device whose worker gave up on it is already detached; forgetting it here lets it be
        // opened afresh below if it is still listed.
        self.attached.retain(|id| devices.descriptor(id).is_ok());
        let gone: Vec<DeviceId> = self.attached.iter().filter(|id| !present.contains_key(*id)).cloned().collect();
        for id in gone {
            self.attached.remove(&id);
            if devices.detach(&id).is_ok() {
                changes.push(Change::Detached(id));
            }
        }
        self.failing.retain(|id| present.contains_key(id));

        for (id, (found, stable)) in &present {
            if self.attached.contains(id) {
                continue;
            }
            match self.enumerator.open(found) {
                Ok(device) => {
                    self.failing.remove(id);
                    self.attached.insert(id.clone());
                    changes.push(Change::Attached(devices.attach(id.clone(), device, BACKEND, *stable)));
                }
                Err(error) => {
                    if self.failing.insert(id.clone()) {
                        changes.push(Change::OpenFailed { id: id.clone(), error });
                    }
                }
            }
        }

        let found_any = !present.is_empty();
        if !found_any && self.found_any != Some(false) {
            changes.push(Change::NoneFound);
        }
        self.found_any = Some(found_any);
        changes
    }

    /// A device's id, and whether it survives a replug. A serial keeps it stable; without one the
    /// id follows the interface's path, numbered in the order paths were first seen.
    fn identify(&mut self, found: &Found) -> (DeviceId, bool) {
        match found.serial.as_deref().filter(|s| !s.is_empty()) {
            Some(serial) => (DeviceId::from_serial(serial), true),
            None => {
                let n = self.unserialed.len();
                let id = self
                    .unserialed
                    .entry(found.path.clone())
                    .or_insert_with(|| DeviceId::from_topology(found.vid, found.pid, 0, n as u8));
                (id.clone(), false)
            }
        }
    }
}

enum Signal {
    Rescan,
    Stop,
}

/// Scans on a thread of its own, every interval and whenever asked. Dropping it stops the thread
/// and waits for a scan in progress; attached devices stay attached.
pub struct HotPlug {
    signals: mpsc::Sender<Signal>,
    thread: Option<JoinHandle<()>>,
}

/// Asks for a scan now, from anywhere (the tray's Rescan devices).
#[derive(Clone)]
pub struct Rescan(mpsc::Sender<Signal>);

impl Rescan {
    pub fn now(&self) {
        // A stopped scanner has nothing to do.
        let _ = self.0.send(Signal::Rescan);
    }
}

impl HotPlug {
    pub fn spawn<E: Enumerator + 'static>(scanner: Scanner<E>, devices: Arc<DeviceManager>, interval: Duration) -> HotPlug {
        let mut scanner = scanner;
        let (signals, rx) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("gazelle-hotplug".into())
            .spawn(move || loop {
                match rx.recv_timeout(interval) {
                    Ok(Signal::Stop) | Err(RecvTimeoutError::Disconnected) => return,
                    Ok(Signal::Rescan) | Err(RecvTimeoutError::Timeout) => {}
                }
                // Several asks queued up are answered by one scan.
                while let Ok(signal) = rx.try_recv() {
                    if let Signal::Stop = signal {
                        return;
                    }
                }
                for change in scanner.scan(&devices) {
                    change.log();
                }
            })
            .expect("spawn hot-plug scanner");
        HotPlug { signals, thread: Some(thread) }
    }

    pub fn rescan(&self) -> Rescan {
        Rescan(self.signals.clone())
    }

    /// Stop scanning, waiting for a scan in progress.
    pub fn stop(self) {}
}

impl Drop for HotPlug {
    fn drop(&mut self) {
        let _ = self.signals.send(Signal::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::manager::tests::{next_lifecycle, Unpluggable};
    use crate::device::manager::ServerEvent;
    use crate::registry_set::{RegistrySet, PID_QUADRO, PID_STUDIO};
    use gazelle_audio_transport::LoopbackDevice;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;
    use std::time::Instant;

    const VID: u16 = 0x23e5;

    #[derive(Default)]
    struct BusState {
        listed: Vec<Found>,
        /// Paths that refuse to open, as while another program holds them.
        held: BTreeSet<String>,
        list_error: Option<String>,
        lists: usize,
        opens: usize,
        /// When set, opened devices can be unplugged through the flag, and each open's flag is kept.
        unpluggable: bool,
        plugs: Vec<Arc<AtomicBool>>,
    }

    /// A fake bus: what is listed, what opens, and counts of both.
    #[derive(Clone, Default)]
    struct Bus(Arc<Mutex<BusState>>);

    impl Bus {
        fn state(&self) -> std::sync::MutexGuard<'_, BusState> {
            self.0.lock().unwrap()
        }
        fn plug(&self, pid: u16, serial: Option<&str>, path: &str) {
            self.state().listed.push(Found { vid: VID, pid, serial: serial.map(str::to_string), path: path.into() });
        }
        fn unplug(&self, path: &str) {
            self.state().listed.retain(|f| f.path != path);
        }
    }

    impl Enumerator for Bus {
        fn list(&mut self) -> Result<Vec<Found>, String> {
            let mut state = self.state();
            state.lists += 1;
            match &state.list_error {
                Some(e) => Err(e.clone()),
                None => Ok(state.listed.clone()),
            }
        }

        fn open(&mut self, found: &Found) -> Result<Box<dyn Device + Send>, String> {
            let mut state = self.state();
            state.opens += 1;
            if state.held.contains(&found.path) {
                return Err("access denied".into());
            }
            if state.unpluggable {
                let (device, plug) = Unpluggable::new(found.pid, true);
                state.plugs.push(plug);
                return Ok(Box::new(device));
            }
            Ok(Box::new(LoopbackDevice::emulating(found.vid, found.pid, 64)))
        }
    }

    fn manager() -> Arc<DeviceManager> {
        DeviceManager::new(RegistrySet::builtin().expect("registries"))
    }

    fn attached(changes: &[Change]) -> Vec<String> {
        changes.iter().filter_map(|c| if let Change::Attached(d) = c { Some(d.id.to_string()) } else { None }).collect()
    }

    /// A change as a short word and id, for comparing whole scans.
    fn summary(changes: &[Change]) -> Vec<String> {
        changes
            .iter()
            .map(|c| match c {
                Change::Attached(d) => format!("attached {}", d.id),
                Change::Detached(id) => format!("detached {id}"),
                Change::OpenFailed { id, .. } => format!("open failed {id}"),
                Change::NoneFound => "none found".into(),
                Change::ListFailed(_) => "list failed".into(),
                Change::ListRecovered => "list recovered".into(),
            })
            .collect()
    }

    fn ids(devices: &DeviceManager) -> Vec<String> {
        devices.descriptors().iter().map(|d| d.id.to_string()).collect()
    }

    #[test]
    fn a_device_that_appears_is_attached_by_serial_and_clients_are_told() {
        let (bus, devices) = (Bus::default(), manager());
        let mut events = devices.subscribe();
        let mut scanner = Scanner::new(bus.clone());
        bus.plug(PID_QUADRO, Some("1000000000001"), "quadro-path");

        assert_eq!(summary(&scanner.scan(&devices)), ["attached serial:1000000000001"]);
        let d = devices.descriptor(&DeviceId::from_serial("1000000000001")).expect("attached");
        assert_eq!((d.backend.as_str(), d.identity_stable, d.family.as_deref()), ("usb", true, Some("quadro")));
        assert!(matches!(next_lifecycle(&mut events, Duration::from_secs(1)), Some(ServerEvent::DeviceAdded(a)) if a.id == d.id));

        assert!(scanner.scan(&devices).is_empty(), "nothing changed");
        assert_eq!(bus.state().opens, 1, "an attached device is not opened again");
        devices.shutdown_all();
    }

    #[test]
    fn a_device_no_longer_listed_is_detached_and_clients_are_told() {
        let (bus, devices) = (Bus::default(), manager());
        let mut scanner = Scanner::new(bus.clone());
        bus.plug(PID_QUADRO, Some("Q1"), "quadro-path");
        bus.plug(PID_STUDIO, Some("S1"), "studio-path");
        assert_eq!(attached(&scanner.scan(&devices)), ["serial:Q1", "serial:S1"]);
        let mut events = devices.subscribe();

        bus.unplug("quadro-path");
        assert_eq!(summary(&scanner.scan(&devices)), ["detached serial:Q1"]);
        assert_eq!(ids(&devices), ["serial:S1"], "the other device is untouched");
        assert!(matches!(next_lifecycle(&mut events, Duration::from_secs(1)), Some(ServerEvent::DeviceRemoved(id)) if id.as_str() == "serial:Q1"));
        assert!(scanner.scan(&devices).is_empty(), "said once");
        devices.shutdown_all();
    }

    #[test]
    fn a_device_that_will_not_open_is_tried_every_scan_and_reported_once() {
        let (bus, devices) = (Bus::default(), manager());
        let mut scanner = Scanner::new(bus.clone());
        bus.plug(PID_STUDIO, Some("S1"), "studio-path");
        bus.state().held.insert("studio-path".into());

        let first = scanner.scan(&devices);
        assert_eq!(summary(&first), ["open failed serial:S1"]);
        assert!(matches!(&first[0], Change::OpenFailed { error, .. } if error == "access denied"));
        assert!(scanner.scan(&devices).is_empty(), "not reported again");
        assert!(scanner.scan(&devices).is_empty());
        assert_eq!(bus.state().opens, 3, "but tried on every scan");
        assert!(devices.is_empty());

        bus.state().held.clear();
        assert_eq!(summary(&scanner.scan(&devices)), ["attached serial:S1"]);

        // Failing again later, after it had opened and gone, is news again.
        bus.unplug("studio-path");
        scanner.scan(&devices);
        bus.plug(PID_STUDIO, Some("S1"), "studio-path");
        bus.state().held.insert("studio-path".into());
        assert_eq!(summary(&scanner.scan(&devices)), ["open failed serial:S1"]);

        // So is failing after being unplugged while it failed.
        bus.unplug("studio-path");
        assert_eq!(summary(&scanner.scan(&devices)), ["none found"]);
        bus.plug(PID_STUDIO, Some("S1"), "studio-path");
        assert_eq!(summary(&scanner.scan(&devices)), ["open failed serial:S1"]);
        devices.shutdown_all();
    }

    #[test]
    fn a_serial_seen_again_after_removal_keeps_its_id() {
        let (bus, devices) = (Bus::default(), manager());
        let mut scanner = Scanner::new(bus.clone());
        bus.plug(PID_QUADRO, Some("1000000000001"), "port-1");
        scanner.scan(&devices);
        bus.unplug("port-1");
        scanner.scan(&devices);
        assert!(devices.is_empty());

        // Back in another port: another path, the same device.
        bus.plug(PID_QUADRO, Some("1000000000001"), "port-2");
        assert_eq!(summary(&scanner.scan(&devices)), ["attached serial:1000000000001"]);
        devices.shutdown_all();
    }

    #[test]
    fn a_device_without_a_serial_is_named_by_its_path_for_as_long_as_the_server_runs() {
        let (bus, devices) = (Bus::default(), manager());
        let mut scanner = Scanner::new(bus.clone());
        bus.plug(PID_QUADRO, None, "port-1");
        bus.plug(PID_QUADRO, Some(""), "port-2");
        let changes = scanner.scan(&devices);
        assert_eq!(attached(&changes), ["usb:23e5:a2f9:0:0", "usb:23e5:a2f9:0:1"], "an empty serial is no serial");
        assert!(devices.descriptors().iter().all(|d| !d.identity_stable));

        bus.unplug("port-1");
        scanner.scan(&devices);
        bus.plug(PID_QUADRO, None, "port-1");
        assert_eq!(attached(&scanner.scan(&devices)), ["usb:23e5:a2f9:0:0"]);
        devices.shutdown_all();
    }

    #[test]
    fn finding_nothing_is_said_at_the_start_and_again_each_time_the_last_device_goes() {
        let (bus, devices) = (Bus::default(), manager());
        let mut scanner = Scanner::new(bus.clone());
        assert_eq!(summary(&scanner.scan(&devices)), ["none found"]);
        assert!(scanner.scan(&devices).is_empty());

        bus.plug(PID_QUADRO, Some("Q1"), "q");
        assert_eq!(summary(&scanner.scan(&devices)), ["attached serial:Q1"]);
        bus.unplug("q");
        assert_eq!(summary(&scanner.scan(&devices)), ["detached serial:Q1", "none found"]);
        assert!(scanner.scan(&devices).is_empty());
    }

    #[test]
    fn a_failed_listing_is_said_once_and_detaches_nothing() {
        let (bus, devices) = (Bus::default(), manager());
        let mut scanner = Scanner::new(bus.clone());
        bus.plug(PID_QUADRO, Some("Q1"), "q");
        scanner.scan(&devices);

        bus.state().list_error = Some("the HID stack is unwell".into());
        assert_eq!(summary(&scanner.scan(&devices)), ["list failed"]);
        assert!(scanner.scan(&devices).is_empty());
        assert_eq!(ids(&devices), ["serial:Q1"]);

        bus.state().list_error = None;
        assert_eq!(summary(&scanner.scan(&devices)), ["list recovered"]);
        devices.shutdown_all();
    }

    #[test]
    fn a_device_its_worker_gave_up_on_is_opened_afresh_while_still_listed() {
        let (bus, devices) = (Bus::default(), manager());
        bus.state().unpluggable = true;
        let mut scanner = Scanner::new(bus.clone());
        bus.plug(PID_QUADRO, Some("Q1"), "q");
        bus.state().held.insert("q".into());
        assert_eq!(summary(&scanner.scan(&devices)), ["open failed serial:Q1"]);
        bus.state().held.clear();
        assert_eq!(summary(&scanner.scan(&devices)), ["attached serial:Q1"]);

        // The handle went bad (a glitch, or a replug faster than a scan): the worker drops it.
        let plug = bus.state().plugs[0].clone();
        plug.store(true, Ordering::SeqCst);
        let deadline = Instant::now() + Duration::from_secs(2);
        while !devices.is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(devices.is_empty(), "the worker detached it");

        // Still listed, so it is opened again; a failure then is news, though one was said before.
        bus.state().held.insert("q".into());
        assert_eq!(summary(&scanner.scan(&devices)), ["open failed serial:Q1"]);
        bus.state().held.clear();
        assert_eq!(summary(&scanner.scan(&devices)), ["attached serial:Q1"]);
        assert_eq!(bus.state().opens, 4);
        devices.shutdown_all();
    }

    fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !done() {
            assert!(Instant::now() < deadline, "timed out waiting until {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn the_thread_scans_at_once_when_asked() {
        let (bus, devices) = (Bus::default(), manager());
        let hotplug = HotPlug::spawn(Scanner::new(bus.clone()), devices.clone(), Duration::from_secs(3600));
        bus.plug(PID_QUADRO, Some("Q1"), "q");
        hotplug.rescan().now();
        wait_until("the device is attached", || !devices.is_empty());
        hotplug.stop();
        devices.shutdown_all();
    }

    #[test]
    fn the_thread_scans_every_interval_until_stopped() {
        let (bus, devices) = (Bus::default(), manager());
        let hotplug = HotPlug::spawn(Scanner::new(bus.clone()), devices.clone(), Duration::from_millis(10));
        let rescan = hotplug.rescan();
        wait_until("three scans have run", || bus.state().lists >= 3);
        bus.plug(PID_STUDIO, Some("S1"), "s");
        wait_until("the device is attached", || !devices.is_empty());

        hotplug.stop();
        let lists = bus.state().lists;
        rescan.now();
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(bus.state().lists, lists, "no scan after stopping, even when asked");
        devices.shutdown_all();
    }
}
