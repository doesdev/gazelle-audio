//! Owns every connected device and routes work to the right worker.

use gazelle_audio_protocol::field::Field;
use gazelle_audio_protocol::registry::Registry;
use gazelle_audio_transport::{Device, LoopbackDevice};
use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::sync::broadcast;

use crate::device::cyclic_loopback::CyclicLoopback;
use crate::device::mixer_loopback::MixerLoopback;
use crate::device::read_loopback::ReadLoopback;
use crate::device::routing_loopback::RoutingLoopback;
use crate::device::descriptor::{DeviceDescriptor, DeviceId};
use crate::device::handle::DeviceHandle;
use crate::device::worker::{self, DeviceEvent, WorkerContext};
use crate::error::ServerError;
use crate::registry_set::RegistrySet;

/// How many events may queue for a slow WebSocket client before it is told it lagged.
const EVENT_BUFFER: usize = 256;

/// Vendor id shared by Antelope USB devices: 9189 (`0x23E5`).
///
/// Source: `ANTELOPE_USB_VENDOR_ID = 9189` in the decompiled `antelope_dev_base.py`.
/// The project notes long carried this as "9189 (0x23F9)", but 9189 is `0x23E5`; `0x23F9`
/// is 9209 and is the Quadro *product* id. Verified 2026-09-12.
pub const ANTELOPE_USB_VID: u16 = 9189;

/// Vendor id for Antelope Thunderbolt devices: 7499 (`0x1D4B`).
///
/// Also mis-annotated as `0x1D57` in the earlier notes; 7499 is `0x1D4B`.
pub const ANTELOPE_TB_VID: u16 = 7499;

/// Wrap an emulating loopback in the layers that make it answer like a device: reads, routing,
/// mixer and links. The loopback backend and the tests build their devices with this.
pub fn loopback_stack(dev: Box<dyn Device + Send>, registry: Option<&Registry>) -> Box<dyn Device + Send> {
    MixerLoopback::wrap(RoutingLoopback::wrap(ReadLoopback::wrap(dev, registry), registry), registry)
}

struct Entry {
    descriptor: DeviceDescriptor,
    handle: DeviceHandle,
    join: Option<std::thread::JoinHandle<()>>,
}

/// The set of devices the server is managing.
pub struct DeviceManager {
    entries: RwLock<BTreeMap<DeviceId, Entry>>,
    registries: RegistrySet,
    events: broadcast::Sender<ServerEvent>,
}

/// Anything a WebSocket client may be told about.
#[derive(Debug, Clone)]
pub enum ServerEvent {
    Device(DeviceEvent),
    DeviceAdded(DeviceDescriptor),
    DeviceRemoved(DeviceId),
}

impl DeviceManager {
    pub fn new(registries: RegistrySet) -> Arc<Self> {
        let (events, _) = broadcast::channel(EVENT_BUFFER);
        Arc::new(DeviceManager {
            entries: RwLock::new(BTreeMap::new()),
            registries,
            events,
        })
    }

    /// Subscribe to the event stream. Each subscriber gets its own buffer.
    pub fn subscribe(&self) -> broadcast::Receiver<ServerEvent> {
        self.events.subscribe()
    }

    /// Attach a device, spawning its worker thread.
    pub fn attach(
        self: &Arc<Self>,
        id: DeviceId,
        device: Box<dyn Device + Send>,
        backend: &str,
        identity_stable: bool,
    ) -> DeviceDescriptor {
        let (vid, pid, mps) = (device.vid(), device.pid(), device.max_packet_size());
        let model = self.registries.for_pid(pid);

        let descriptor = DeviceDescriptor {
            id: id.clone(),
            vid,
            pid,
            slug: model.map(|m| m.slug.to_string()),
            model: model.map(|m| m.model.to_string()),
            family: model.map(|m| m.family.to_string()),
            command_count: model.map(|m| m.registry.len()),
            identity_stable,
            backend: backend.to_string(),
            max_packet_size: mps,
        };

        let (tx, rx) = std::sync::mpsc::channel();
        let events = self.events.clone();
        let ctx = WorkerContext {
            device_id: id.clone(),
            device,
            registry: model.map(|m| m.registry.clone()),
            events: Box::new(move |e| {
                // No subscribers is the normal case when nothing is watching; ignore.
                let _ = events.send(ServerEvent::Device(e));
            }),
        };

        // A worker whose device goes away removes its own entry. The lock is held until the entry
        // is in and added has been announced, so a device gone at once is still removed after it.
        let mut entries = self.entries.write().unwrap();
        let manager = Arc::downgrade(self);
        let worker_id = id.clone();
        let join = std::thread::Builder::new()
            .name(format!("gazelle-device-{id}"))
            .spawn(move || {
                if worker::run(ctx, rx) == worker::Exit::DeviceGone {
                    if let Some(manager) = manager.upgrade() {
                        manager.remove_gone(&worker_id);
                    }
                }
            })
            .expect("spawn device worker");

        let handle = DeviceHandle::new(id.to_string(), tx);
        entries.insert(id.clone(), Entry { descriptor: descriptor.clone(), handle, join: Some(join) });
        let _ = self.events.send(ServerEvent::DeviceAdded(descriptor.clone()));
        descriptor
    }

    /// Called by a worker whose device went away: forget the device and tell clients.
    ///
    /// No join, since this runs on that worker's own thread, which ends straight after. If the
    /// device was detached meanwhile, `detach` removed the entry and announced it, and waits for
    /// this thread, so an entry found here is always this worker's own.
    fn remove_gone(&self, id: &DeviceId) {
        if self.entries.write().unwrap().remove(id).is_none() {
            return;
        }
        tracing::warn!("{id} stopped responding (unplugged?) and was detached");
        let _ = self.events.send(ServerEvent::DeviceRemoved(id.clone()));
    }

    /// Attach `count` loopback devices for a given model. Used as the safe default backend
    /// and by the test suite.
    pub fn attach_loopbacks(self: &Arc<Self>, pids: &[u16], max_packet_size: usize) {
        for (n, pid) in pids.iter().enumerate() {
            // `emulating` answers like a device (cmd + 1, same ext2), so the full
            // request/response path is exercised rather than only framing.
            let dev = LoopbackDevice::emulating(ANTELOPE_USB_VID, *pid, max_packet_size);
            let registry = self.registries.for_pid(*pid).map(|m| m.registry.as_ref());
            self.attach(DeviceId::loopback(n), loopback_stack(Box::new(dev), registry), "loopback", true);
        }
    }

    /// Attach emulating loopbacks that also push every cyclic report their model declares,
    /// once per `interval` (`--loopback-cyclic-ms`). Each report is at least as long as its
    /// layout, which the decoder accepts, as the existing cyclic tests rely on.
    pub fn attach_cyclic_loopbacks(self: &Arc<Self>, pids: &[u16], max_packet_size: usize, interval: Duration) {
        for (n, pid) in pids.iter().enumerate() {
            let reports: Vec<(u32, usize)> = self
                .registries
                .for_pid(*pid)
                .map(|model| {
                    let mut ids: Vec<u32> = model.registry.cyclic_ids().copied().collect();
                    ids.sort_unstable();
                    ids.into_iter()
                        .filter_map(|id| model.registry.cyclic(id).map(|layout| (id, layout.fields.iter().map(Field::size).sum())))
                        .collect()
                })
                .unwrap_or_default();
            let dev = CyclicLoopback::new(LoopbackDevice::emulating(ANTELOPE_USB_VID, *pid, max_packet_size), reports, interval);
            let registry = self.registries.for_pid(*pid).map(|m| m.registry.as_ref());
            self.attach(DeviceId::loopback(n), loopback_stack(Box::new(dev), registry), "loopback", true);
        }
    }

    pub fn descriptors(&self) -> Vec<DeviceDescriptor> {
        self.entries
            .read()
            .unwrap()
            .values()
            .map(|e| e.descriptor.clone())
            .collect()
    }

    pub fn descriptor(&self, id: &DeviceId) -> Result<DeviceDescriptor, ServerError> {
        self.entries
            .read()
            .unwrap()
            .get(id)
            .map(|e| e.descriptor.clone())
            .ok_or_else(|| ServerError::UnknownDevice(id.to_string()))
    }

    pub fn handle(&self, id: &DeviceId) -> Result<DeviceHandle, ServerError> {
        self.entries
            .read()
            .unwrap()
            .get(id)
            .map(|e| e.handle.clone())
            .ok_or_else(|| ServerError::UnknownDevice(id.to_string()))
    }

    pub fn registries(&self) -> &RegistrySet {
        &self.registries
    }

    pub fn len(&self) -> usize {
        self.entries.read().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Detach a device and join its worker thread.
    pub fn detach(&self, id: &DeviceId) -> Result<(), ServerError> {
        let entry = self
            .entries
            .write()
            .unwrap()
            .remove(id)
            .ok_or_else(|| ServerError::UnknownDevice(id.to_string()))?;
        entry.handle.shutdown();
        if let Some(join) = entry.join {
            let _ = join.join();
        }
        let _ = self.events.send(ServerEvent::DeviceRemoved(id.clone()));
        Ok(())
    }

    /// Stop every worker. Called on shutdown.
    pub fn shutdown_all(&self) {
        let ids: Vec<DeviceId> = self.entries.read().unwrap().keys().cloned().collect();
        for id in ids {
            let _ = self.detach(&id);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::registry_set::PID_QUADRO;
    use gazelle_audio_protocol::payload::PayloadValues;
    use gazelle_audio_protocol::wire::WireError;
    use gazelle_audio_transport::{RawPacket, Report};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Instant;

    /// An answering loopback that can be unplugged: once `unplugged` is set, its next write fails
    /// (or its next poll, with `on_poll`) and from then on it reports itself gone, as the USB
    /// device does.
    pub(crate) struct Unpluggable {
        inner: LoopbackDevice,
        unplugged: Arc<AtomicBool>,
        connected: bool,
        on_poll: bool,
        /// Pulled out just as a write lands: the write succeeds, no reply comes, the next poll fails.
        after_write: bool,
    }

    impl Unpluggable {
        pub(crate) fn new(pid: u16, on_poll: bool) -> (Self, Arc<AtomicBool>) {
            let unplugged = Arc::new(AtomicBool::new(false));
            let device = Unpluggable {
                inner: LoopbackDevice::emulating(ANTELOPE_USB_VID, pid, 64),
                unplugged: unplugged.clone(),
                connected: true,
                on_poll,
                after_write: false,
            };
            (device, unplugged)
        }
    }

    impl Device for Unpluggable {
        fn send(&mut self, report: &Report) -> Result<bool, WireError> {
            if self.unplugged.load(Ordering::SeqCst) {
                self.connected = false;
            }
            if !self.connected {
                return Ok(false);
            }
            let sent = self.inner.send(report);
            if self.after_write {
                self.unplugged.store(true, Ordering::SeqCst);
            }
            sent
        }
        fn on_received_data(&mut self, packet: RawPacket) {
            if !self.unplugged.load(Ordering::SeqCst) {
                self.inner.on_received_data(packet)
            }
        }
        fn max_packet_size(&self) -> usize {
            self.inner.max_packet_size()
        }
        fn vid(&self) -> u16 {
            self.inner.vid()
        }
        fn pid(&self) -> u16 {
            self.inner.pid()
        }
        fn poll_reports(&mut self) -> Vec<Report> {
            if self.on_poll && self.unplugged.load(Ordering::SeqCst) {
                self.connected = false;
            }
            self.inner.poll_reports()
        }
        fn is_connected(&self) -> bool {
            self.connected
        }
    }

    fn manager() -> Arc<DeviceManager> {
        DeviceManager::new(RegistrySet::builtin().expect("registries"))
    }

    /// The next lifecycle event, skipping device traffic, within `within`.
    pub(crate) fn next_lifecycle(events: &mut broadcast::Receiver<ServerEvent>, within: Duration) -> Option<ServerEvent> {
        let deadline = Instant::now() + within;
        while Instant::now() < deadline {
            match events.try_recv() {
                Ok(ServerEvent::Device(_)) => {}
                Ok(event) => return Some(event),
                Err(_) => std::thread::sleep(Duration::from_millis(5)),
            }
        }
        None
    }

    #[test]
    fn a_device_that_goes_away_while_idle_is_detached_and_clients_are_told() {
        let devices = manager();
        let mut events = devices.subscribe();
        let (device, unplugged) = Unpluggable::new(PID_QUADRO, true);
        let id = DeviceId::from_serial("1000000000001");
        devices.attach(id.clone(), Box::new(device), "usb", true);
        assert!(matches!(next_lifecycle(&mut events, Duration::from_secs(1)), Some(ServerEvent::DeviceAdded(d)) if d.id == id));

        unplugged.store(true, Ordering::SeqCst);
        match next_lifecycle(&mut events, Duration::from_secs(2)) {
            Some(ServerEvent::DeviceRemoved(gone)) => assert_eq!(gone, id),
            other => panic!("expected the device to be removed, got {other:?}"),
        }
        assert!(devices.is_empty(), "the device is no longer listed");
        assert!(matches!(devices.handle(&id), Err(ServerError::UnknownDevice(_))));
        assert!(next_lifecycle(&mut events, Duration::from_millis(100)).is_none(), "removed once");
    }

    #[tokio::test]
    async fn a_request_to_a_device_that_went_away_fails_as_gone_at_once_not_as_a_timeout() {
        let devices = manager();
        let mut events = devices.subscribe();
        // Only a write shows it gone, so the request is what finds out.
        let (device, unplugged) = Unpluggable::new(PID_QUADRO, false);
        let id = DeviceId::from_serial("1000000000001");
        devices.attach(id.clone(), Box::new(device), "usb", true);
        let handle = devices.handle(&id).unwrap();
        handle.request("get_adats_links", PayloadValues::default(), None, false).await.expect("answered while plugged in");

        unplugged.store(true, Ordering::SeqCst);
        let started = Instant::now();
        let result = handle.request("get_adats_links", PayloadValues::default(), None, false).await;
        assert!(matches!(result, Err(ServerError::DeviceGone(_))), "got {result:?}");
        assert!(started.elapsed() < Duration::from_secs(1), "not left to the 3 s timeout: {:?}", started.elapsed());

        let removed = tokio::task::spawn_blocking(move || {
            std::iter::from_fn(|| next_lifecycle(&mut events, Duration::from_secs(2)))
                .find(|e| matches!(e, ServerEvent::DeviceRemoved(_)))
        })
        .await
        .unwrap();
        assert!(matches!(removed, Some(ServerEvent::DeviceRemoved(gone)) if gone == id));
        assert!(devices.is_empty());
    }

    #[tokio::test]
    async fn a_set_written_to_a_device_that_went_away_is_not_reported_done() {
        let devices = manager();
        let (device, unplugged) = Unpluggable::new(PID_QUADRO, false);
        let id = DeviceId::from_serial("1000000000001");
        devices.attach(id.clone(), Box::new(device), "usb", true);
        let handle = devices.handle(&id).unwrap();
        let values = || crate::value::json_to_payload_values(&serde_json::json!({"level": 64})).unwrap();
        handle.request("set_mixer", values(), None, false).await.expect("done while plugged in");

        // A set waits for no reply, so the write is all there is to go on.
        unplugged.store(true, Ordering::SeqCst);
        let result = handle.request("set_mixer", values(), None, false).await;
        assert!(matches!(result, Err(ServerError::DeviceGone(_))), "got {result:?}");
    }

    #[tokio::test]
    async fn a_device_pulled_out_while_a_reply_is_awaited_fails_the_request_at_once() {
        let devices = manager();
        let (mut device, _) = Unpluggable::new(PID_QUADRO, true);
        device.after_write = true;
        let id = DeviceId::from_serial("1000000000001");
        devices.attach(id.clone(), Box::new(device), "usb", true);

        let started = Instant::now();
        let result = devices.handle(&id).unwrap().request("get_adats_links", PayloadValues::default(), None, false).await;
        assert!(matches!(result, Err(ServerError::DeviceGone(_))), "got {result:?}");
        assert!(started.elapsed() < Duration::from_secs(1), "not left to the 3 s timeout: {:?}", started.elapsed());
    }

    #[test]
    fn detaching_a_device_tells_clients_once() {
        let devices = manager();
        let mut events = devices.subscribe();
        let (device, _) = Unpluggable::new(PID_QUADRO, true);
        let id = DeviceId::from_serial("1");
        devices.attach(id.clone(), Box::new(device), "usb", true);
        devices.detach(&id).unwrap();
        assert!(matches!(next_lifecycle(&mut events, Duration::from_secs(1)), Some(ServerEvent::DeviceAdded(_))));
        assert!(matches!(next_lifecycle(&mut events, Duration::from_secs(1)), Some(ServerEvent::DeviceRemoved(gone)) if gone == id));
        assert!(next_lifecycle(&mut events, Duration::from_millis(100)).is_none());
    }
}
