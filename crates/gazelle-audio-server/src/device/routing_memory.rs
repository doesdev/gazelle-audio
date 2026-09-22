//! What Gazelle has seen of each device's routing, from the commands it sent itself.
//!
//! Every routing change reaches a device through this server, whichever client made it, so the
//! server can know a group's routing without asking the device over and over: it remembers each
//! `set_routing` it sent and each `get_routing` answer it had, and says when something changed.
//! Nothing here asks a device anything. A group nobody has read or written since the device was
//! attached is simply not known, and whoever needs it reads it once.
//!
//! The aggregate is what needs it: its input channels are named after what the routing sends to
//! each USB record channel, and those names have to follow the routing whether or not any page is
//! open (`crate::aggregate::export`).

use std::collections::BTreeMap;
use std::sync::RwLock;

use gazelle_audio_protocol::payload::Value;
use gazelle_audio_protocol::wire::HEADER_SIZE;
use tokio::sync::watch;

use crate::device::descriptor::DeviceId;
use crate::device::worker::CommandOutcome;

/// One routing slot as the wire carries it: `[source group position, channel]`.
pub type Slot = [u8; 2];

/// Slots in one `set_routing`, in both families.
const WRITTEN_SLOTS: usize = 32;

/// Each device's routing groups as last seen, by wire position.
pub struct RoutingMemory {
    groups: RwLock<BTreeMap<DeviceId, BTreeMap<u32, Vec<Slot>>>>,
    /// Bumped whenever a group's slots are not what they were, or a device's are forgotten. A
    /// watch rather than a queue, so a burst of changes is one wake and nothing is ever missed.
    changed: watch::Sender<u64>,
}

impl Default for RoutingMemory {
    fn default() -> Self {
        RoutingMemory { groups: RwLock::new(BTreeMap::new()), changed: watch::channel(0).0 }
    }
}

impl RoutingMemory {
    /// A group's slots as last seen, or nothing when it has not been read or written since the
    /// device was attached.
    pub fn slots(&self, device: &DeviceId, group: u32) -> Option<Vec<Slot>> {
        self.groups.read().ok()?.get(device)?.get(&group).cloned()
    }

    /// Something to wait on for the next change.
    pub fn changes(&self) -> watch::Receiver<u64> {
        self.changed.subscribe()
    }

    /// Take note of a command that reached a device. Anything but a routing read or write is
    /// passed over, and so is one whose bytes or answer do not read as one.
    pub fn observe(&self, device: &DeviceId, command: &str, ext3: Option<u32>, outcome: &CommandOutcome) {
        if outcome.dry_run {
            return;
        }
        let seen = match command {
            "set_routing" => written(&outcome.sent),
            "get_routing" => outcome.response.as_ref().and_then(|fields| read(ext3, fields)),
            _ => None,
        };
        if let Some((group, slots)) = seen {
            self.put(device, group, slots);
        }
    }

    /// Put a group's slots down as seen, saying so when they are not what they were.
    pub fn put(&self, device: &DeviceId, group: u32, slots: Vec<Slot>) {
        let Ok(mut groups) = self.groups.write() else { return };
        let known = groups.entry(device.clone()).or_default();
        if known.get(&group) == Some(&slots) {
            return;
        }
        known.insert(group, slots);
        drop(groups);
        self.changed.send_modify(|generation| *generation += 1);
    }

    /// A device went away: what was seen of it may not be true when it comes back.
    pub fn forget(&self, device: &DeviceId) {
        let removed = self.groups.write().map(|mut groups| groups.remove(device).is_some()).unwrap_or(false);
        if removed {
            self.changed.send_modify(|generation| *generation += 1);
        }
    }
}

/// The group and slots a `set_routing` carried: after the header, the payload's own two bytes, then
/// `bank_idx` and 32 pairs, which is the layout the routing loopback reads its echo by.
fn written(sent: &[u8]) -> Option<(u32, Vec<Slot>)> {
    let payload = sent.get(HEADER_SIZE..)?;
    let group = *payload.get(2)?;
    let pairs = payload.get(3..3 + 2 * WRITTEN_SLOTS)?;
    Some((u32::from(group), (0..WRITTEN_SLOTS).map(|at| [pairs[2 * at], pairs[2 * at + 1]]).collect()))
}

/// The group and slots a `get_routing` answer carried. The group is the one the answer says, else
/// the one that was asked for.
fn read(ext3: Option<u32>, fields: &std::collections::HashMap<String, Value>) -> Option<(u32, Vec<Slot>)> {
    let group = match fields.get("bank_idx") {
        Some(Value::U64(n)) => u32::try_from(*n).ok(),
        _ => ext3,
    }?;
    let Some(Value::List(entries)) = fields.get("bank_configs") else { return None };
    let byte = |entry: &std::collections::HashMap<String, Value>, name: &str| match entry.get(name) {
        Some(Value::U64(n)) => u8::try_from(*n).ok(),
        _ => None,
    };
    let slots = entries
        .iter()
        .map(|entry| match entry {
            Value::Struct(entry) => Some([byte(entry, "in_periph_id")?, byte(entry, "in_chann")?]),
            _ => None,
        })
        .collect::<Option<Vec<Slot>>>()?;
    Some((group, slots))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn outcome(sent: Vec<u8>, response: Option<HashMap<String, Value>>) -> CommandOutcome {
        CommandOutcome { sent, response, response_error: None, dry_run: false }
    }

    /// A `set_routing` as it goes on the wire: a header, the payload's own two bytes, the group and
    /// 32 pairs.
    fn set_routing(group: u8, pairs: &[Slot]) -> Vec<u8> {
        let mut bytes = vec![0u8; HEADER_SIZE];
        bytes.extend([19, 65, group]);
        for at in 0..WRITTEN_SLOTS {
            bytes.extend(pairs.get(at).copied().unwrap_or([10, 0]));
        }
        bytes
    }

    fn get_routing(group: u64, pairs: &[Slot]) -> HashMap<String, Value> {
        let entries = pairs
            .iter()
            .map(|[source, channel]| Value::Struct([("in_periph_id".to_string(), Value::U64(u64::from(*source))), ("in_chann".to_string(), Value::U64(u64::from(*channel)))].into_iter().collect()))
            .collect();
        [("bank_idx".to_string(), Value::U64(group)), ("bank_configs".to_string(), Value::List(entries))].into_iter().collect()
    }

    #[test]
    fn a_routing_write_that_went_is_remembered_and_said_once() {
        let memory = RoutingMemory::default();
        let mut changes = memory.changes();
        let device = DeviceId::from_serial("Q");
        memory.observe(&device, "set_routing", None, &outcome(set_routing(4, &[[0, 0], [5, 2]]), None));
        let slots = memory.slots(&device, 4).expect("the group that was written");
        assert_eq!(slots.len(), 32);
        assert_eq!(&slots[..3], &[[0, 0], [5, 2], [10, 0]]);
        assert!(changes.has_changed().unwrap(), "a change is said");
        changes.borrow_and_update();
        // The same routing again is no change at all.
        memory.observe(&device, "set_routing", None, &outcome(set_routing(4, &[[0, 0], [5, 2]]), None));
        assert!(!changes.has_changed().unwrap());
        assert_eq!(memory.slots(&device, 5), None, "a group nobody touched is not known");
    }

    #[test]
    fn a_routing_read_is_remembered_by_the_group_it_answers_for() {
        let memory = RoutingMemory::default();
        let device = DeviceId::from_serial("S");
        memory.observe(&device, "get_routing", Some(6), &outcome(Vec::new(), Some(get_routing(6, &[[0, 0], [0, 1]]))));
        assert_eq!(memory.slots(&device, 6), Some(vec![[0, 0], [0, 1]]));
    }

    #[test]
    fn a_dry_run_another_command_and_bytes_that_are_not_a_write_are_passed_over() {
        let memory = RoutingMemory::default();
        let device = DeviceId::from_serial("Q");
        memory.observe(&device, "set_routing", None, &CommandOutcome { dry_run: true, ..outcome(set_routing(4, &[[1, 1]]), None) });
        memory.observe(&device, "set_mixer", None, &outcome(set_routing(4, &[[1, 1]]), None));
        memory.observe(&device, "set_routing", None, &outcome(vec![0; 20], None));
        memory.observe(&device, "get_routing", Some(4), &outcome(Vec::new(), None));
        assert_eq!(memory.slots(&device, 4), None);
    }

    #[test]
    fn a_device_that_went_away_is_forgotten() {
        let memory = RoutingMemory::default();
        let device = DeviceId::from_serial("Q");
        memory.put(&device, 4, vec![[0, 0]]);
        let mut changes = memory.changes();
        memory.forget(&device);
        assert_eq!(memory.slots(&device, 4), None);
        assert!(changes.has_changed().unwrap());
        changes.borrow_and_update();
        memory.forget(&device);
        assert!(!changes.has_changed().unwrap(), "forgetting nothing says nothing");
    }
}
