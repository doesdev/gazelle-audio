//! The workspace: cross-device layout state.
//!
//! Groups, channel links and visibility span devices by definition, so no device can hold
//! them. They live here, keyed by [`DeviceId`], and are persisted server-side so a layout
//! survives a reload and is shared between clients.
//!
//! Profiles (full device state: routing, levels, preamps, phantom) are deliberately **not**
//! here: they are separate JSON documents behind their own storage, so the layout stays small
//! and a device's state is never saved as a side effect of moving a strip.

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
    /// Mixer layouts the user saved, per device model. Additive like `mixers`.
    #[serde(default)]
    pub layouts: Vec<SavedLayout>,
    /// Per-device badge colours, `#rrggbb`, for the strips a surface shows.
    #[serde(default)]
    pub device_colors: BTreeMap<DeviceId, String>,
    /// Cross-device mix surfaces. Additive like `mixers`.
    #[serde(default)]
    pub surfaces: Vec<Surface>,
    /// Digital connections between devices, as the user declares them.
    #[serde(default)]
    pub cables: Vec<Cable>,
    /// Per device, what its Control Room panel shows. A device without an entry shows the client's
    /// default (Monitor, HP1 and HP2). Additive like `mixers`.
    #[serde(default)]
    pub control_room: BTreeMap<DeviceId, ControlRoom>,
    /// The aggregate audio driver's setup, when the user has one. Additive like `mixers`: a
    /// workspace written before it loads with none, and none is written out again, so an older
    /// Gazelle reads a newer workspace unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aggregate: Option<Aggregate>,
    /// The Recording page's presets. Additive like `aggregate`: a workspace written before it loads
    /// with none and writes none, so an older Gazelle reads a newer workspace unchanged, and the
    /// presets travel with a backup like everything else here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recording: Option<Recording>,
    /// Top-level fields this server does not know (a newer app's), kept as they came and given back
    /// so an export always imports back whole.
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
            cables: Vec::new(),
            control_room: BTreeMap::new(),
            aggregate: None,
            recording: None,
            extra: BTreeMap::new(),
        }
    }
}

/// The Recording page's setup: its presets.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Recording {
    #[serde(default)]
    pub presets: Vec<RecordingPreset>,
    /// As [`Workspace::extra`].
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

/// One recording preset: what to record, where, and how.
///
/// The rate and the buffer size are not here: they are the aggregate's, from its setup.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RecordingPreset {
    /// Unique among the presets.
    pub id: String,
    pub name: String,
    /// The channels to record, in the order their files are listed: each an interface by its place
    /// in the aggregate's setup and that interface's own input, both from zero, exactly as the
    /// calibration names a channel. **Never the aggregate's own numbering**, which depends on how
    /// many channels each driver really has.
    #[serde(default)]
    pub channels: Vec<RecordingChannel>,
    /// The folder the files go in. Left out means the default, `Documents\Gazelle Recordings`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    /// How files are named; see [`RECORDING_PATTERN_DEFAULT`]. Left out means that default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    /// One of [`RECORDING_FORMATS`]. Left out means `int24`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    /// How much of the memory free at Arm the pre-roll takes, in percent, 1 to 50. Left out means 10.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preroll_percent: Option<f64>,
    /// The most pre-roll to keep, in seconds, which also makes the buffer smaller. Left out means as
    /// much as the percentage gives.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preroll_max_seconds: Option<f64>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

/// One channel a preset records.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordingChannel {
    /// The interface, by its place in the aggregate's setup, from zero.
    pub device: u32,
    /// That interface's own input, from zero.
    pub channel: u32,
}

/// The sample formats a preset can write: 24-bit integer and 32-bit float.
pub const RECORDING_FORMATS: &[&str] = &["int24", "float32"];
/// How a preset names its files when it does not say.
pub const RECORDING_PATTERN_DEFAULT: &str = "{date} T{take} {channel}";
/// The pre-roll a preset takes when it does not say, in percent of the free memory.
pub const RECORDING_PERCENT_DEFAULT: f64 = 10.0;
/// The longest a pre-roll cap may be, in seconds: an hour.
pub const RECORDING_CAP_MAX: f64 = 3600.0;

/// The aggregate audio driver's setup: which interfaces it opens, in what order, which one drives
/// the callback and how their streams line up.
///
/// This mirrors the driver's own configuration file, because the driver has to work with Gazelle
/// closed and a file is all it reads. Keeping the setup here is what makes it travel with a
/// workspace backup; Gazelle exports it to the file the driver reads whenever it changes
/// (`crate::aggregate::export`). Everything is optional, as it is in the file: a section with no
/// devices means the driver opens every Antelope driver it finds.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Aggregate {
    /// The sub-devices, in the order their channels appear to a DAW.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub devices: Vec<AggregateDevice>,
    /// Which device drives the callback. Gazelle writes the device's registry key (or class id),
    /// which a rename cannot change, and turns it into the name the driver knows it by when the
    /// file is written. An older setup's name for a device is still understood. The first device
    /// when it is not said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callback_master: Option<String>,
    /// One of [`ALIGNMENTS`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alignment: Option<String>,
    /// The rate to put every device at, in Hz. None is "whatever the interfaces are on", which the
    /// exported file turns into the rate they are all running at, when they agree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate: Option<u32>,
    /// The buffer size to offer a DAW as preferred, in samples.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub buffer_size: Option<u32>,
    /// Fields this Gazelle does not know (a newer one's), kept as they came, given back, and
    /// exported to the driver's file, which ignores what it does not know in the same way.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

/// One interface the aggregate opens. It needs a `key` or a `clsid`; everything else is optional.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AggregateDevice {
    /// The name the vendor driver registers itself under, matched without case, whole or as part.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// The vendor driver's class id, which is the sure way to name one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clsid: Option<String>,
    /// What an older setup called this interface. **Not a name anything is called by any more**:
    /// an interface is called by Gazelle's name for its device, so renaming the device is the one
    /// way to rename it and there is no second name to keep in step. It is kept, and read, only so
    /// that a `callback_master` an older Gazelle wrote by this name still finds its device.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Samples to add to what this device's driver says its input latency is. A device that
    /// records late takes a positive trim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_trim: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_trim: Option<i32>,
    /// Which of its inputs to expose, by the device's own numbering from zero; all of them when
    /// it is not said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inputs: Option<Vec<u32>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outputs: Option<Vec<u32>>,
    /// The names the person has **typed** for this device's inputs, by the device's own channel
    /// number from zero, the same numbering [`AggregateDevice::inputs`] uses. Only typed names are
    /// here: the name each channel takes from Gazelle's routing is worked out whenever the file is
    /// written (`crate::aggregate::naming`), so it can never overwrite one of these, and clearing
    /// one brings the automatic name back. A channel that is not named is left out, so an empty map
    /// is the same as saying nothing.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub input_names: BTreeMap<u32, String>,
    /// As [`AggregateDevice::input_names`], for its outputs.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub output_names: BTreeMap<u32, String>,
    /// Where the cable the driver measures this interface's capture phase over runs. Left out
    /// means it is not phase measured, and a session lines it up by the figures its driver
    /// reports. Only an interface that does not drive the callback can have one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<AggregatePhase>,
    /// Input trims measured at one rate and one buffer size each, each with the phase reference
    /// measured beside it. A session uses the one for the rate and buffer size it runs at. Only where
    /// there is none for that setup does it fall back to [`AggregateDevice::input_trim`] and
    /// [`AggregatePhase::reference`], which are a trim whose setup was never written down: every
    /// trim from before trims were kept per setup is one of those, and it is kept as it was.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trims: Vec<AggregateTrim>,
    /// Which Gazelle device this is, when the user has said so, which is how the readiness answer
    /// reads its clock, its rate and its buffer. Gazelle's own, not the driver's: it is left out
    /// of the exported file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<DeviceId>,
    /// What Gazelle last knew about the interface this entry turned out to be. The server's alone:
    /// it is worked out again on every save and whenever the routing changes, a client's copy of it
    /// is never taken, and it is left out of the exported file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub known: Option<AggregateKnown>,
    /// As [`Aggregate::extra`], per device.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

/// What Gazelle last knew about the interface one aggregate entry turned out to be.
///
/// The names the driver is given come from Gazelle's device and Gazelle's routing, and neither can be
/// read while the interface is unplugged or before Gazelle has heard from it after a start. Keeping
/// what was last known here is what keeps those names in the driver's file across a restart, rather
/// than falling back to the vendor driver's until the interface is back. It is a record of facts,
/// never of names: the names are worked out from it, with the person's own names as they are now.
///
/// It records the routing groups the names are worked out from, and that for naming only: a device's
/// state is otherwise never kept in the workspace.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AggregateKnown {
    /// The device it was, whether the person chose it or Gazelle worked it out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<DeviceId>,
    /// Its model, as a family key (`quadro`, `studio`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family: Option<String>,
    /// Its model's name, as the sidebar shows it when the person has not named the device.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The routing groups its channels are named from, by topology id (`COM_REC0`, `MONITOR0`,
    /// `MIXER_IN0`, ...), each as its slots by channel from zero, `[source group position, channel]`,
    /// which is the pair a routing slot is: its USB record group, for what feeds each input, and its
    /// outputs, mix inputs and effect inputs, for where each USB playback channel ends up. A group is here once it
    /// has been read or written through Gazelle.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub routing: BTreeMap<String, Vec<[u8; 2]>>,
    /// The sample rate it was last seen running at, in Hz, as the interface itself reports it (not
    /// what its driver remembers). With no rate chosen in the setup, the aggregate runs at this when
    /// every interface agrees on it (`crate::aggregate::config::rate_in_force`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate: Option<u32>,
}

/// The digital path the driver measures one interface's capture phase over: which output of the
/// interface that drives the callback feeds it, and which of its own inputs the cable arrives on,
/// and the phase measured over it while this interface's input trim was measured.
///
/// Both channels are the interfaces' **own** channel numbers from zero, the numbering
/// [`AggregateDevice::inputs`] and [`AggregateDevice::outputs`] use, so the setting survives a
/// change to which channels are exposed. The driver keeps both of them out of the channel list a
/// DAW sees, so nothing a DAW plays can land on the measurement channel.
///
/// **Three numbers, not one.** The trim ([`AggregateDevice::input_trim`]) is the constant somebody
/// measured once with a click. The reference ([`AggregatePhase::reference`]) is the phase the
/// driver measured in that same session. The phase is what an interface's capture pipeline settles
/// on each time its stream starts, which the driver measures at the start of every session. Each
/// session is lined up by the reference minus the phase, which puts it back in the state the trim
/// was measured in, and then the trim applies.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AggregatePhase {
    /// The callback master's output channel the cable leaves from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub master_output: Option<u32>,
    /// This interface's own input channel the cable arrives on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<u32>,
    /// The phase measured in the session this interface's input trim was measured in, in samples,
    /// and written with it by a calibration run. Left out means there is nothing to line a session
    /// up to yet, so each session runs on the drivers' own figures until the interfaces have been
    /// measured once. It belongs to the trim beside it: a trim typed in by hand has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<i32>,
}

/// One input trim, measured at one rate and one buffer size, with the phase reference measured
/// beside it in the same session. A trim is only true of the setup it was measured at, because the
/// drivers' own latency figures change with the rate and the buffer size.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AggregateTrim {
    /// The rate it was measured at, in Hz.
    pub rate: u32,
    /// The buffer size it was measured at, in samples.
    pub buffer_size: u32,
    /// The trim, with the same meaning and sign as [`AggregateDevice::input_trim`]. Zero is a trim:
    /// it says this setup was measured and needs nothing.
    pub input_trim: i32,
    /// The phase measured in that session, as [`AggregatePhase::reference`] is. Absent when the
    /// interface has no phase setup, or nothing was heard on its cable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<i32>,
}

impl AggregateDevice {
    /// The trim measured at exactly this rate and buffer size, if there is one.
    pub fn trim_at(&self, rate: u32, buffer_size: u32) -> Option<&AggregateTrim> {
        self.trims.iter().find(|trim| trim.rate == rate && trim.buffer_size == buffer_size)
    }

    /// Put a trim measured at its own setup in, replacing the one that was there for that setup.
    /// Kept in order of rate and then buffer size, so the file reads the same way the page lists them.
    pub fn keep_trim(&mut self, trim: AggregateTrim) {
        self.trims.retain(|kept| (kept.rate, kept.buffer_size) != (trim.rate, trim.buffer_size));
        self.trims.push(trim);
        self.trims.sort_by_key(|kept| (kept.rate, kept.buffer_size));
    }
}

/// How the aggregate lines its devices up.
pub const ALIGNMENTS: &[&str] = &["aligned", "lowest_latency"];

/// The largest trim a device may take, either way: one second at the highest rate these
/// interfaces run. Real trims are tens of samples; this only catches a number typed by mistake.
pub const TRIM_MAX: i32 = 192_000;

/// The furthest a phase reference may be from zero, either way. What the hardware measures carries
/// a constant of a few hundred samples; this only catches a number that could never be one, and
/// it is the same bound a trim has.
pub const PHASE_REFERENCE_MAX: i32 = TRIM_MAX;

/// The highest channel index a device may expose. No interface has anything like this many; it
/// only catches a number that is not a channel.
pub const AGGREGATE_CHANNEL_MAX: u32 = 1023;

/// The longest a channel's label may be, in characters. The interface carries 31, and the name a
/// DAW shows is built from it, so a longer one would arrive cut in half.
pub const CHANNEL_NAME_MAX: usize = 31;

/// What one device's Control Room panel shows.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ControlRoom {
    /// Output ids as `set_volume` numbers them. The panel shows them in the device's own order, so
    /// the order here means nothing.
    #[serde(default)]
    pub outputs: Vec<u32>,
}

/// A user-built row of strips drawn from any attached device. It holds only
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
    /// A digital output for a `port` strip: one of [`CABLE_SENDS`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<String>,
    /// A port strip's first channel: 0, or 8 for the Studio+'s second ADAT port.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first: Option<u32>,
}

pub const STRIP_KINDS: &[&str] = &["channel", "master", "input", "output", "port", "label"];

/// A cable the user says joins one device's digital output to another's input. It is a fact about
/// the room, not a setting: declaring one never routes, clocks or links anything, and only lets the
/// app say where a digital input's signal comes from and warn when the two ends disagree.
///
/// The one exception is a cable the person gives over to the aggregate's phase measurement
/// ([`Cable::dedicated`]): the page that does it routes the two ends for it, once, after a confirm,
/// and from then on says when the routing no longer serves it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Cable {
    pub id: String,
    pub from: CableEnd,
    pub to: CableEnd,
    /// How many channels it carries, from each end's `first`.
    pub channels: u32,
    /// Present while the cable is dedicated to the aggregate's phase measurement and the clock.
    /// Additive: a workspace without it loads with none and writes none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dedicated: Option<CableDedication>,
}

/// The two channels a dedicated cable keeps for the phase measurement. The sending device is the
/// aggregate's callback master and the receiving device a follower; the cable's own first channel
/// carries the measurement, so it is not repeated here.
///
/// Both are the devices' own USB channel numbers from zero, the numbering
/// [`AggregatePhase::master_output`] and [`AggregatePhase::input`] use, and while the dedication
/// means anything they are exactly the follower's phase setup.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CableDedication {
    /// The sending device's USB playback channel routed straight to the cable's first channel.
    pub phase_output: u32,
    /// The receiving device's USB record channel that records the cable's first channel.
    pub phase_input: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CableEnd {
    pub device_id: DeviceId,
    /// One of [`CABLE_SENDS`] at the sending end, [`CABLE_RECEIVES`] at the receiving end.
    pub port: String,
    /// The first channel of the port the cable carries.
    #[serde(default)]
    pub first: u32,
}

/// Digital outputs a cable can leave from, and the inputs they feed, in the same order.
pub const CABLE_SENDS: &[&str] = &["SPDIF_OUT", "ADAT_OUT"];
pub const CABLE_RECEIVES: &[&str] = &["SPDIF_IN", "ADAT_IN"];

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
    /// Present while the mix is summed to mono: its channels are panned to centre on
    /// the device, and these are the pans to restore.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mono: Option<MonoMix>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MonoMix {
    /// Mixer input slot → the pan (2..=62) it had, or was moved to, while mono.
    #[serde(default)]
    pub pans: BTreeMap<u32, u32>,
    /// The mix master's level (dB of attenuation, 0..=90) just before mono lowered it, so
    /// ending mono can give back exactly the step mono took. Absent in a workspace written
    /// before mono compensated the level, and left out again when it is absent, so a
    /// workspace still loads in either version: nothing here denies unknown fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub master_level: Option<u32>,
}

/// The device pan range; 32 is centre.
pub const PAN_MIN: u32 = 2;
pub const PAN_MAX: u32 = 62;

/// The mixer level range, in dB of attenuation: 0 is 0 dB and 90 is -90 dB.
pub const LEVEL_MAX: u32 = 90;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MixerGroup {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub collapsed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Present while the group plays a mid and a side microphone decoded to stereo. Additive: a
    /// workspace without it loads with none and writes none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mid_side: Option<MidSide>,
}

/// How the side strips are made: `effect`, the side input through two effect chains holding the
/// same effect, the inverted copy's with its polarity switch on. `preamp` (a second preamp fed by a
/// split of the side microphone with its polarity switched) is a way the client no longer sets up,
/// and so is `effect` with no `left` (one chain for the copy against the dry side). Both are still
/// accepted, because the client writes the whole workspace back, so a decode saved either way must
/// not stop every later save.
pub const MID_SIDE_VIAS: &[&str] = &["preamp", "effect"];

/// A mid and a side microphone played as stereo in the hardware mix: the mid channel centred, the
/// side signal hard left and an inverted copy of it hard right at the same level, so the left is
/// mid plus side and the right is mid minus side. The strips have no polarity switch, so the copy is
/// a channel of its own on a source that inverts (see [`MID_SIDE_VIAS`]), and the side signal goes
/// through a matching one so the two carry the same delay; the side channel itself is muted.
///
/// The client sets it up and takes it down; this is what it needs to notice the decode has been
/// broken and to put back what it changed. The channel ids are not checked against the layout: a
/// channel removed by hand leaves the group to say so.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MidSide {
    /// [`MixerChannel`] ids.
    pub mid: String,
    /// The side microphone's own channel.
    pub side: String,
    /// The channel that carries the side signal through its chain, hard left.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left: Option<String>,
    /// The channel that carries the inverted copy.
    pub inverted: String,
    /// One of [`MID_SIDE_VIAS`].
    pub via: String,
    /// The second preamp, from 0, when `via` is the retired `preamp`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preamp: Option<u32>,
    /// The inverted copy's effect chain, from 0, and the effect in it, when `via` is `effect`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect_type: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect_inst: Option<u32>,
    /// The hard-left side strip's effect chain, from 0, and its instance of the same effect type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left_chain: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left_effect_inst: Option<u32>,
    /// What routing fed the hard-left chain before; none when it was muted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left_chain_input: Option<RouteSource>,
    /// Mixes whose effect return strip for the hard-left chain was muted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub left_returns_muted: Vec<u32>,
    /// Mixes the side channel was muted in while the decode plays there, to unmute again.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub side_muted: Vec<u32>,
    /// The inputs the mid and side channels had when it was set up, to notice them swapped.
    pub mid_source: RouteSource,
    pub side_source: RouteSource,
    /// Per mix it plays in: the pans the mid and side channels had before, to put back.
    #[serde(default)]
    pub pans: BTreeMap<u32, MidSidePans>,
    /// The [`MixerGroup`]s the mid and side channels were in before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mid_group: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub side_group: Option<String>,
    /// Links its own links took channels out of, to make again.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub displaced_links: Vec<ChannelLink>,
    /// The second preamp's polarity switch before, for the retired `preamp` way.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase_invert: Option<bool>,
    /// What routing fed the effect chain before; none when it was muted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain_input: Option<RouteSource>,
    /// Mixes whose effect return strip for the chain was muted, so the copy does not play twice.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub returns_muted: Vec<u32>,
}

/// The pans (2..=62) a mid and a side channel had in one mix.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct MidSidePans {
    pub mid: u32,
    pub side: u32,
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

/// Channels of one kind, on any devices, that change together. Links belong to the
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
