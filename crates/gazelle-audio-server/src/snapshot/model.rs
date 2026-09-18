//! The snapshot document.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value as Json};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::device::descriptor::DeviceId;
use crate::snapshot::time::{now_millis, now_rfc3339};
use crate::workspace::model::Workspace;

/// Schema version of the snapshot document. Versioned from the first one written, so a later
/// format can migrate rather than misread, and so an older server refuses a newer file outright
/// instead of dropping the half of it that it does not understand.
pub const SNAPSHOT_VERSION: u32 = 1;

/// The sections a device snapshot is grouped into, in the order the UI shows them.
pub const SECTIONS: &[&str] = &["inputs", "mixer", "routing", "outputs", "clock", "settings"];

/// A named, dated record of the workspace and of every attached device's state.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub version: u32,
    pub id: String,
    pub name: String,
    /// RFC 3339, UTC.
    pub created: String,
    #[serde(default)]
    pub note: String,
    /// The workspace exactly as the server held it, including fields this server does not model.
    pub workspace: Workspace,
    #[serde(default)]
    pub devices: BTreeMap<DeviceId, DeviceSnapshot>,
    /// Top-level fields a newer app wrote, kept as they came so a snapshot exported from a newer
    /// server and imported back is still whole (workspace spec Q7, as the workspace does).
    #[serde(flatten)]
    pub extra: BTreeMap<String, Json>,
}

/// One device's state at the moment of capture, as decoded named values rather than raw bytes.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeviceSnapshot {
    pub family: String,
    pub model: String,
    /// RFC 3339, UTC: when this device was read, which is not quite when the snapshot was made.
    pub read_at: String,
    /// The preset slot the device says is current. **Recorded, never recalled**: nobody has yet
    /// established what a preset recall changes (spec §2.4), so this is a note for the user.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_preset: Option<i64>,
    /// Section name (one of [`SECTIONS`]) to its values.
    #[serde(default)]
    pub sections: BTreeMap<String, Json>,
    /// What could not be read, rather than a value invented for it (P96, spec §2.2).
    #[serde(default)]
    pub unreadable: Vec<Unreadable>,
}

/// A value the device would not give up, and why.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unreadable {
    /// Where the value would have gone, as a diff path: `inputs.preamps`, `routing.SPDIF_OUT0`.
    pub path: String,
    pub reason: String,
}

impl Snapshot {
    /// A fresh id: milliseconds since the epoch, and a counter so two captures in the same
    /// millisecond cannot collide. Only `[a-z0-9-]`, which [`crate::snapshot::store`] requires.
    pub fn new_id() -> String {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        format!("snap-{:x}-{:04x}", now_millis(), NEXT.fetch_add(1, Ordering::Relaxed) & 0xffff)
    }

    pub fn new(name: String, note: String, workspace: Workspace) -> Self {
        Snapshot {
            version: SNAPSHOT_VERSION,
            id: Self::new_id(),
            name,
            created: now_rfc3339(),
            note,
            workspace,
            devices: BTreeMap::new(),
            extra: BTreeMap::new(),
        }
    }

    /// What the list shows: when it was taken, from which devices, and how much of each device
    /// could not be read. Not the values, which are tens of kilobytes each.
    pub fn summary(&self) -> Json {
        let devices: Vec<Json> = self
            .devices
            .iter()
            .map(|(id, device)| {
                json!({
                    "device_id": id,
                    "family": device.family,
                    "model": device.model,
                    "read_at": device.read_at,
                    "current_preset": device.current_preset,
                    "sections": device.sections.keys().collect::<Vec<_>>(),
                    "unreadable": device.unreadable.len(),
                })
            })
            .collect();
        json!({
            "version": self.version,
            "id": self.id,
            "name": self.name,
            "created": self.created,
            "note": self.note,
            "devices": devices,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_snapshot_carries_the_schema_version_and_a_unique_id() {
        let a = Snapshot::new("Drum tracking".into(), String::new(), Workspace::default());
        let b = Snapshot::new("Drum tracking".into(), String::new(), Workspace::default());
        assert_eq!(a.version, SNAPSHOT_VERSION);
        assert_ne!(a.id, b.id, "two captures in the same millisecond are still two snapshots");
        assert!(a.id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'), "{}", a.id);
    }

    #[test]
    fn the_summary_counts_what_could_not_be_read_without_carrying_the_values() {
        let mut snapshot = Snapshot::new("Live".into(), "before the gig".into(), Workspace::default());
        snapshot.devices.insert(
            DeviceId::loopback(0),
            DeviceSnapshot {
                family: "quadro".into(),
                model: "Zen Quadro".into(),
                read_at: "2026-09-17T20:15:00Z".into(),
                current_preset: Some(2),
                sections: BTreeMap::from([("clock".to_string(), json!({"sync_source": 5}))]),
                unreadable: vec![Unreadable { path: "inputs.emulations".into(), reason: "refused".into() }],
            },
        );
        let summary = snapshot.summary();
        assert_eq!(summary["devices"][0]["model"], "Zen Quadro");
        assert_eq!(summary["devices"][0]["current_preset"], 2);
        assert_eq!(summary["devices"][0]["unreadable"], 1);
        assert_eq!(summary["devices"][0]["sections"], json!(["clock"]));
        assert!(summary.to_string().find("sync_source").is_none(), "the summary is not the snapshot: {summary}");
    }

    #[test]
    fn unknown_top_level_fields_survive_a_round_trip() {
        let text = r#"{"version":1,"id":"snap-1","name":"n","created":"2026-01-01T00:00:00Z",
            "workspace":{"version":1},"devices":{},"taken_by":"a newer app"}"#;
        let snapshot: Snapshot = serde_json::from_str(text).expect("parses");
        assert_eq!(snapshot.extra["taken_by"], "a newer app");
        let back = serde_json::to_value(&snapshot).expect("serialises");
        assert_eq!(back["taken_by"], "a newer app");
    }
}
