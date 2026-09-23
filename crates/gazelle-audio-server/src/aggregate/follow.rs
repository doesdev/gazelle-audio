//! Keeping the names in the aggregate's file in step with the devices while Gazelle runs.
//!
//! The names the driver is given follow Gazelle's routing (`crate::aggregate::naming`), and a
//! routing change is not a workspace save, so something has to notice one. The server sends every
//! routing change itself, whichever client asked for it, and remembers it
//! (`crate::device::routing_memory`); this waits on that, and on devices coming and going, and asks
//! the exporting store to look again. It does not depend on any page being open, and it never polls
//! a device. It also follows each device's own status reports for the rate it runs at, which the
//! file carries when the setup leaves the rate to the interfaces, waking only when that rate
//! changes. The only thing it ever asks a device is a routing group the names come from (its USB record
//! group, its outputs and its mix inputs), each once, when an interface of the aggregate is there
//! and Gazelle has not seen that group since it was attached.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use gazelle_audio_protocol::payload::PayloadValues;
use tokio::sync::broadcast::error::RecvError;

use crate::aggregate::export::ExportingStore;
use crate::aggregate::naming::{device_of, naming_groups};
use crate::device::descriptor::DeviceId;
use crate::aggregate::STATUS_REPORT;
use crate::device::manager::{DeviceManager, ServerEvent};
use crate::device::worker::DeviceEvent;
use crate::workspace::model::Workspace;
use crate::workspace::store::WorkspaceStore;

/// The routing groups worth reading now: for every interface of the aggregate that is connected and
/// of a known model, each group its names come from that Gazelle has not seen, as `(device, wire
/// position)`.
pub fn wanted_reads(workspace: &Workspace, devices: &DeviceManager) -> Vec<(DeviceId, u32)> {
    let Some(config) = &workspace.aggregate else { return Vec::new() };
    let attached = devices.descriptors();
    config
        .devices
        .iter()
        .filter_map(|device| {
            let id = device_of(device)?;
            let descriptor = attached.iter().find(|d| &d.id == id)?;
            Some((id.clone(), naming_groups(descriptor.family.as_deref()?)))
        })
        .flat_map(|(id, groups)| {
            groups.into_iter().filter(|(_, position)| devices.routing().slots(&id, *position).is_none()).map(|(_, position)| (id.clone(), position)).collect::<Vec<_>>()
        })
        .collect()
}

/// Follow the devices for as long as the server runs. `reads` is false under `--dry-run`, where
/// nothing at all is sent to a device, and the names then follow only what was seen.
pub async fn follow(store: Arc<ExportingStore>, devices: Arc<DeviceManager>, reads: bool) {
    let mut changes = devices.routing().changes();
    let mut events = devices.subscribe();
    // A group asked for once is not asked for again until its device has been away and come back,
    // so a device that will not answer is not asked over and over.
    let mut asked: BTreeSet<(DeviceId, u32)> = BTreeSet::new();
    // The rate each device last reported, so that its own status reports, which come many times a
    // second, wake this only when the rate in them is not what it was.
    let mut rates: BTreeMap<DeviceId, u64> = BTreeMap::new();
    loop {
        let refreshing = store.clone();
        match tokio::task::spawn_blocking(move || refreshing.refresh()).await {
            Ok(Err(why)) => tracing::warn!("the aggregate's names could not be brought up to date: {why}"),
            Err(why) => tracing::warn!("bringing the aggregate's names up to date stopped part way: {why}"),
            Ok(Ok(_)) => {}
        }
        if reads {
            if let Ok(workspace) = store.load() {
                for (id, group) in wanted_reads(&workspace, &devices) {
                    if !asked.insert((id.clone(), group)) {
                        continue;
                    }
                    let Ok(handle) = devices.handle(&id) else { continue };
                    // What it answers is remembered on the way back, which wakes this loop again.
                    if let Err(why) = handle.request("get_routing", PayloadValues::default(), Some(group), false).await {
                        tracing::info!("{id} did not say how routing group {group} is routed, so the aggregate names those channels by their USB channel: {why}");
                    }
                }
            }
        }
        // Wait for something that could change a name: a routing change, a device coming or
        // going, or a save. The meters' reports are passed over without waking anything.
        loop {
            tokio::select! {
                changed = changes.changed() => {
                    if changed.is_err() {
                        return;
                    }
                    break;
                }
                () = store.saved().notified() => break,
                event = events.recv() => match event {
                    Ok(ServerEvent::DeviceAdded(_)) => break,
                    Ok(ServerEvent::DeviceRemoved(id)) => {
                        asked.retain(|(device, _)| *device != id);
                        rates.remove(&id);
                        break;
                    }
                    // A device's own report of the rate it runs at, which the file follows when the
                    // setup leaves the rate to the interfaces.
                    Ok(ServerEvent::Device(DeviceEvent::Cyclic { device_id, report_id, fields })) if report_id == STATUS_REPORT => {
                        let Some(rate) = rate_index_of(&fields) else { continue };
                        if rates.insert(device_id, rate) == Some(rate) {
                            continue;
                        }
                        break;
                    }
                    Ok(ServerEvent::Device(_)) => continue,
                    // Something may have been missed while the meters filled the queue: look again.
                    Err(RecvError::Lagged(_)) => break,
                    Err(RecvError::Closed) => return,
                },
            }
        }
    }
}

/// The rate index a status report carries, however it was decoded.
fn rate_index_of(fields: &std::collections::HashMap<String, gazelle_audio_protocol::payload::Value>) -> Option<u64> {
    match fields.get("base_index")? {
        gazelle_audio_protocol::payload::Value::U64(n) => Some(*n),
        gazelle_audio_protocol::payload::Value::I64(n) => u64::try_from(*n).ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aggregate::naming::usb_groups;
    use crate::device::manager::loopback_stack;
    use crate::registry_set::{RegistrySet, PID_QUADRO};
    use crate::workspace::model::{Aggregate, AggregateDevice};
    use gazelle_audio_transport::LoopbackDevice;

    fn attached_quadro() -> Arc<DeviceManager> {
        let devices = DeviceManager::new(RegistrySet::builtin().unwrap());
        let registries = RegistrySet::builtin().unwrap();
        let registry = registries.for_pid(PID_QUADRO).map(|m| m.registry.as_ref());
        devices.attach(DeviceId::from_serial("Q"), loopback_stack(Box::new(LoopbackDevice::emulating(0x23e5, PID_QUADRO, 64)), registry), "loopback", true);
        devices
    }

    fn setup(ids: &[&str]) -> Workspace {
        Workspace {
            aggregate: Some(Aggregate {
                devices: ids.iter().map(|id| AggregateDevice { key: Some(format!("key {id}")), device_id: Some(DeviceId::from_serial(id)), ..AggregateDevice::default() }).collect(),
                ..Aggregate::default()
            }),
            ..Workspace::default()
        }
    }

    #[test]
    fn a_status_report_gives_its_rate_index_however_it_was_decoded() {
        use gazelle_audio_protocol::payload::Value;
        let report = |value: Value| [("base_index".to_string(), value)].into_iter().collect::<std::collections::HashMap<_, _>>();
        assert_eq!(rate_index_of(&report(Value::U64(4))), Some(4));
        assert_eq!(rate_index_of(&report(Value::I64(1))), Some(1));
        assert_eq!(rate_index_of(&report(Value::Bytes(vec![4]))), None);
        assert_eq!(rate_index_of(&std::collections::HashMap::new()), None);
    }

    /// Reading the group once is what the server does, and after it nothing more is wanted: the
    /// answer came back through the device worker and was remembered on the way.
    #[tokio::test]
    async fn an_interface_whose_record_routing_is_not_known_is_read_once_and_then_known() {
        let devices = attached_quadro();
        let workspace = setup(&["Q", "elsewhere"]);
        let position = usb_groups("quadro").unwrap().record_position;
        let wanted = wanted_reads(&workspace, &devices);
        assert_eq!(wanted.len(), 10, "its record group, five outputs and four mix inputs: {wanted:?}");
        assert!(wanted.iter().all(|(id, _)| id == &DeviceId::from_serial("Q")), "only the connected one");

        let handle = devices.handle(&DeviceId::from_serial("Q")).unwrap();
        for (_, group) in &wanted {
            handle.request("get_routing", PayloadValues::default(), Some(*group), false).await.expect("the loopback answers");
        }
        let slots = devices.routing().slots(&DeviceId::from_serial("Q"), position).expect("remembered on the way back");
        assert_eq!(&slots[..4], &[[0, 0], [0, 1], [0, 2], [0, 3]], "the loopback's USB A REC takes the four preamps");
        assert!(wanted_reads(&workspace, &devices).is_empty(), "and nothing more is wanted");
    }

    #[tokio::test]
    async fn a_routing_write_through_the_server_is_remembered_without_asking_the_device() {
        let devices = attached_quadro();
        let position = usb_groups("quadro").unwrap().record_position;
        let handle = devices.handle(&DeviceId::from_serial("Q")).unwrap();
        let mut pairs = vec![5u8, 2];
        pairs.extend(std::iter::repeat_n([10u8, 0], 31).flatten());
        let values = PayloadValues::default().with_scalar("bank_idx", u64::from(position)).with_bytes("bank_configs", pairs);
        handle.request("set_routing", values, None, false).await.expect("the loopback takes it");
        assert_eq!(devices.routing().slots(&DeviceId::from_serial("Q"), position).unwrap()[0], [5, 2]);
        // A dry run reaches nothing, so nothing is remembered from one.
        let dry = PayloadValues::default().with_scalar("bank_idx", u64::from(position)).with_bytes("bank_configs", vec![0; 64]);
        handle.request("set_routing", dry, None, true).await.unwrap();
        assert_eq!(devices.routing().slots(&DeviceId::from_serial("Q"), position).unwrap()[0], [5, 2]);
    }
}
