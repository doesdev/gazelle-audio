//! Owns every connected device and routes work to the right worker.

use antelope_transport::{Device, LoopbackDevice};
use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};
use tokio::sync::broadcast;

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

        let join = std::thread::Builder::new()
            .name(format!("antelope-device-{id}"))
            .spawn(move || worker::run(ctx, rx))
            .expect("spawn device worker");

        let handle = DeviceHandle::new(id.to_string(), tx);
        self.entries.write().unwrap().insert(
            id.clone(),
            Entry { descriptor: descriptor.clone(), handle, join: Some(join) },
        );
        let _ = self.events.send(ServerEvent::DeviceAdded(descriptor.clone()));
        descriptor
    }

    /// Attach `count` loopback devices for a given model. Used as the safe default backend
    /// and by the test suite.
    pub fn attach_loopbacks(self: &Arc<Self>, pids: &[u16], max_packet_size: usize) {
        for (n, pid) in pids.iter().enumerate() {
            // `emulating` answers like a device (cmd + 1, same ext2), so the full
            // request/response path is exercised rather than only framing.
            let dev = LoopbackDevice::emulating(ANTELOPE_USB_VID, *pid, max_packet_size);
            self.attach(DeviceId::loopback(n), Box::new(dev), "loopback", true);
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
