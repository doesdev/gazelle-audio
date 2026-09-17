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
    /// Mixer layouts the user saved, per device model (decision P56). Additive like `mixers`.
    #[serde(default)]
    pub layouts: Vec<SavedLayout>,
    /// Per-device badge colours, `#rrggbb`, for the strips a surface shows (workspace spec Q15).
    #[serde(default)]
    pub device_colors: BTreeMap<DeviceId, String>,
    /// Cross-device mix surfaces (workspace spec §4). Additive like `mixers`.
    #[serde(default)]
    pub surfaces: Vec<Surface>,
    /// Top-level fields this server does not know (a newer app's), kept as they came and given back
    /// so an export always imports back whole (workspace spec, Q7).
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

/// A mixer layout saved by name, which any device of `family` can start from.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SavedLayout {
    pub id: String,
    pub name: String,
    /// One of [`LAYOUT_FAMILIES`].
    pub family: String,
    #[serde(default)]
    pub mixer: DeviceMixer,
}

pub const LAYOUT_FAMILIES: &[&str] = &["quadro", "studio"];

impl Default for Workspace {
    fn default() -> Self {
        Workspace {
            version: WORKSPACE_VERSION,
            groups: Vec::new(),
            links: Vec::new(),
            aliases: BTreeMap::new(),
            mixers: BTreeMap::new(),
            layouts: Vec::new(),
            device_colors: BTreeMap::new(),
            surfaces: Vec::new(),
            extra: BTreeMap::new(),
        }
    }
}

/// A user-built row of strips drawn from any attached device (workspace spec §4). It holds only
/// what to show: every control sends what the device's own page would, and nothing is shared
/// between devices.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Surface {
    pub id: String,
    pub name: String,
    /// The mix each device's channel strips show, by device (0 until chosen).
    #[serde(default)]
    pub mixes: BTreeMap<DeviceId, u32>,
    /// In display order.
    #[serde(default)]
    pub strips: Vec<SurfaceStrip>,
}

/// One strip on a surface. `kind` says which of the optional parts it has: a `channel` names a
/// mixer channel of the device's layout (and may pin a `mix`), a `master` a mix (the device's
/// selected mix without one), an `input` a hardware input, an `output` an output id, and a `label`
/// only `text`. A strip naming a channel that no longer exists is kept, so removing a channel does
/// not rearrange a surface.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SurfaceStrip {
    /// Unique within its surface.
    pub id: String,
    /// One of [`STRIP_KINDS`].
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<DeviceId>,
    /// A [`MixerChannel`] id, for a `channel` strip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mix: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<InputRef>,
    /// Output id as `set_volume` numbers it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

pub const STRIP_KINDS: &[&str] = &["channel", "master", "input", "output", "label"];

/// A hardware input: one of [`INPUT_KINDS`] and a channel within it, from 0.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InputRef {
    pub kind: String,
    pub channel: u32,
}

pub const INPUT_KINDS: &[&str] = &["preamp", "line", "adat", "spdif"];

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
    /// Present while the mix is summed to mono (decision P57): its channels are panned to centre on
    /// the device, and these are the pans to restore.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mono: Option<MonoMix>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MonoMix {
    /// Mixer input slot → the pan (2..=62) it had, or was moved to, while mono.
    #[serde(default)]
    pub pans: BTreeMap<u32, u32>,
}

/// The device pan range; 32 is centre.
pub const PAN_MIN: u32 = 2;
pub const PAN_MAX: u32 = 62;

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

/// Channels of one kind, on any devices, that change together (decision P51). Links belong to the
/// workspace, not the device: a client sends each change to every member. `channel` in a member is
/// the index within the kind (preamp, line, ADAT or S/PDIF input; mixer channels by input slot).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChannelLink {
    pub id: String,
    /// One of [`LINK_KINDS`].
    pub kind: String,
    /// One of [`LINK_MODES`]: `absolute` members take the same value; `relative` members keep their offsets.
    #[serde(default = "absolute_link")]
    pub mode: String,
    pub members: Vec<ChannelRef>,
}

pub const LINK_KINDS: &[&str] = &["preamp", "line", "adat", "spdif", "mixer"];
pub const LINK_MODES: &[&str] = &["absolute", "relative"];

fn absolute_link() -> String {
    "absolute".into()
}
