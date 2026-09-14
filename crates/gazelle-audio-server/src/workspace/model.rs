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
}

impl Default for Workspace {
    fn default() -> Self {
        Workspace {
            version: WORKSPACE_VERSION,
            groups: Vec::new(),
            links: Vec::new(),
            aliases: BTreeMap::new(),
        }
    }
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
