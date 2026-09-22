//! Device identity and the descriptor clients see.

use serde::{Deserialize, Serialize};

/// A device's stable identifier.
///
/// Prefer the USB serial number; fall back to a topology string. The fallback is not stable
/// across a replug or a hub change, which matters because workspace layouts are keyed by
/// this. So the descriptor carries `identity_stable` to let clients warn.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DeviceId(pub String);

impl DeviceId {
    pub fn from_serial(serial: &str) -> Self {
        DeviceId(format!("serial:{serial}"))
    }

    pub fn from_topology(vid: u16, pid: u16, bus: u8, addr: u8) -> Self {
        DeviceId(format!("usb:{vid:04x}:{pid:04x}:{bus}:{addr}"))
    }

    pub fn loopback(n: usize) -> Self {
        DeviceId(format!("loopback-{n}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for DeviceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// What a client is told about a device.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeviceDescriptor {
    pub id: DeviceId,
    pub vid: u16,
    pub pid: u16,
    /// Device slug, e.g. `zenquadrosc_usb2`. `None` when the model is unknown.
    pub slug: Option<String>,
    /// Human-readable model name. `None` when the model is unknown.
    pub model: Option<String>,
    /// The model's short form, "Quadro" or "Studio+", which the aggregate's channels carry in a DAW
    /// for a device nobody has named. `None` when the model is unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub short_model: Option<String>,
    /// Stable model key (`quadro`, `studio`) for clients to switch on. `None` when unknown.
    pub family: Option<String>,
    /// How many commands this device's registry exposes. `None` when unknown.
    pub command_count: Option<usize>,
    /// False when `id` is derived from bus topology and may change across a replug.
    pub identity_stable: bool,
    /// Which transport backend is serving this device.
    pub backend: String,
    /// Maximum packet size in bytes, bounding segmentation.
    pub max_packet_size: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_distinguishable_by_source() {
        assert_eq!(DeviceId::from_serial("ABC123").as_str(), "serial:ABC123");
        assert_eq!(
            DeviceId::from_topology(9189, 0xa2f9, 1, 7).as_str(),
            "usb:23e5:a2f9:1:7"
        );
        assert_eq!(DeviceId::loopback(0).as_str(), "loopback-0");
    }

    #[test]
    fn ids_roundtrip_through_json() {
        let id = DeviceId::from_serial("X1");
        let s = serde_json::to_string(&id).unwrap();
        assert_eq!(serde_json::from_str::<DeviceId>(&s).unwrap(), id);
    }
}
