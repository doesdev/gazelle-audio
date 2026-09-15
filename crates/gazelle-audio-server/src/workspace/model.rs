//! The workspace: cross-device layout state.
//!
//! Groups, channel links and visibility span devices by definition, so no device can hold
//! them. They live here, keyed by [`DeviceId`], and are persisted server-side so a layout
//! survives a reload and is shared between clients.
//!
//! Profiles (full device state: routing, levels, preamps, phantom) are deliberately **not**
//! here — see `.agent/decisions/0011-json-profiles-behind-storage-trait.md`.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::device::descriptor::DeviceId;

/// Schema version, so a future format change can migrate rather than misread.
pub const WORKSPACE_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Workspace {
    pub version: u32,
    /// Top-level groups; groups nest via `children`.
    #[serde(default)]
    pub groups: Vec<Group>,
    /// Stereo/multi links between channels, possibly spanning devices.
    #[serde(default)]
    pub links: Vec<ChannelLink>,
    /// Per-device user-assigned names.
    #[serde(default)]
    pub aliases: BTreeMap<DeviceId, String>,
    /// Per-device mixer layouts the user builds (plan 2026-09-16). Additive, so version 1
    /// documents without it load with none.
    #[serde(default)]
    pub mixers: BTreeMap<DeviceId, DeviceMixer>,
}

impl Default for Workspace {
    fn default() -> Self {
        Workspace {
            version: WORKSPACE_VERSION,
            groups: Vec::new(),
            links: Vec::new(),
            aliases: BTreeMap::new(),
            mixers: BTreeMap::new(),
        }
    }
}

/// Mixers per device and mixer input slots per mix, in both supported families.
pub const MIXER_COUNT: u32 = 4;
pub const MIXER_SLOTS: u32 = 32;

/// A device's mixer as the user laid it out. Routing and levels stay on the device; this holds
/// what the device cannot: which channels exist, their order, names and groups, and mix names.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DeviceMixer {
    /// Indexed by device mixer.
    #[serde(default)]
    pub mixes: Vec<MixConfig>,
    #[serde(default)]
    pub groups: Vec<MixerGroup>,
    /// In display order.
    #[serde(default)]
    pub channels: Vec<MixerChannel>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MixConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MixerGroup {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub collapsed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

/// One user channel. It occupies one mixer input slot in every mix: routed to its source in its
/// main mix and send mixes, muted in the others.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MixerChannel {
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// A [`MixerGroup`] id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Mixer input slot, `0..MIXER_SLOTS`.
    pub slot: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<RouteSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub main_mix: Option<u32>,
    #[serde(default)]
    pub sends: Vec<u32>,
}

/// A routing source: a group's position in the device's input group list, and a channel in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteSource {
    pub group: u32,
    pub channel: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Group {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub collapsed: bool,
    #[serde(default)]
    pub hidden: bool,
    /// `#rrggbb` when the user colour-codes the group; omitted when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Channels in this group, each naming its device.
    #[serde(default)]
    pub members: Vec<ChannelRef>,
    /// Nested sub-groups.
    #[serde(default)]
    pub children: Vec<Group>,
}

/// One channel on one device.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelRef {
    pub device_id: DeviceId,
    pub channel: u32,
}

/// A link between two or more channels, which may span devices.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChannelLink {
    pub id: String,
    pub members: Vec<ChannelRef>,
}
