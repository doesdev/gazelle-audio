//! The aggregate audio driver, as Gazelle configures and diagnoses it.
//!
//! The driver itself is a separate thing entirely: it lives in a DAW's process, reads one file
//! and never talks to this server to do its job. What is here is the other half, which is
//! everything a person needs around it. The setup lives in the workspace and is exported to the
//! driver's file whenever it changes (`config`, `export`). One route answers whether the PC is
//! ready to run it and, when it is not, says why in words and with a code the page can act on
//! (`readiness`). The live picture comes out of shared memory and a small log the driver writes
//! (`status`). And the fixes a diagnosis implies are routes of their own (`crate::http::aggregate`).
//!
//! Everything a PC does sits behind a trait: the driver registry (`registry`), the device tree
//! that says which USB host controller an interface is on (`usb`), the driver's own record
//! (`status`) and the elevated helper that registers the DLL (`elevate`). The DLL itself comes
//! with a release, inside the executable, and is written out beside it (`bundled`). No test reads a real
//! registry, maps a section or elevates anything.

pub mod bundled;
pub mod calibrate;
pub mod config;
pub mod elevate;
pub mod export;
pub mod follow;
pub mod naming;
pub mod readiness;
pub mod registry;
pub mod service;
pub mod status;
pub mod usb;

use serde::Serialize;

use crate::device::descriptor::DeviceId;

/// The status report both models push, whose fields carry the live clock.
pub const STATUS_REPORT: u32 = 0x73;

/// The sample rates `set_samp_rate` takes, in its index order. The web client carries the same
/// list; a rate that is not here cannot be asked for by index.
pub const RATES: &[u32] = &[32000, 44100, 48000, 88200, 96000, 176400, 192000];

/// The index `set_samp_rate` takes for a rate in Hz.
pub fn rate_index(hz: u32) -> Option<u32> {
    RATES.iter().position(|&rate| rate == hz).map(|at| at as u32)
}

/// Where the rate the aggregate runs at comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RateFrom {
    /// The setup names a rate.
    Setup,
    /// The setup leaves it to the interfaces, and every one of them is running at this rate.
    Interfaces,
}

/// The rate the aggregate puts every interface at when it opens, and why.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct RateInForce {
    pub hz: u32,
    pub from: RateFrom,
}

/// A rate in words, as the page writes one: "96 kHz", "44.1 kHz".
pub fn khz(hz: u32) -> String {
    let text = format!("{:.3}", f64::from(hz) / 1000.0);
    format!("{} kHz", text.trim_end_matches('0').trim_end_matches('.'))
}

/// What a device says about its clock **now**, while it is streaming, which is the only reading
/// that means anything: a device set to internal silently becomes USB clocked once a DAW opens it
/// (Antelope support article 42000096748, and phase 0 measured it).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ClockReading {
    /// `set_sync_source`'s index.
    pub source_index: u32,
    /// The source's name on this model, when the model is known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub locked: bool,
    /// The rate the device says it is running at, in Hz, from its own three frequency bytes.
    pub hz: u32,
    /// `set_samp_rate`'s index, as the device reports it.
    pub rate_index: u32,
}

impl ClockReading {
    /// The rate the interface is running at: the rate its own report names, else the frequency it
    /// measures. This is the interface, not its driver, which can remember another rate.
    pub fn running_rate(&self) -> Option<u32> {
        RATES.get(self.rate_index as usize).copied().or_else(|| (self.hz > 0).then_some(self.hz))
    }
}

/// What the audio driver says about one device, as much of it as the aggregate cares about.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct DriverSummary {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub buffer_size: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub safe_mode: Option<bool>,
    /// How many programs hold this driver's interface. One DAW recording counted as four
    /// (2026-09-18), so this is "something is using it", not a number of programs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asio_clients: Option<u32>,
    /// Why there is less here than there could be.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// How Gazelle worked out which connected interface a configured device is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchedBy {
    /// The setup names a device of Gazelle's, and that device is connected now.
    Chosen,
    /// The setup names none, and the driver's registry entry says which model it is, of which
    /// exactly one is connected.
    WorkedOut,
    /// Neither, so nothing about this interface can be read or changed from here.
    #[default]
    None,
}

/// One interface's channels in the aggregate, which are its USB audio channels: aggregate input
/// *k* is its USB record channel *k*, and aggregate output *k* its USB playback channel *k*. The
/// counts are those groups' own, from the model's topology, so they are exact and known without a
/// DAW (`crate::aggregate::naming`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct UsbChannels {
    /// How many inputs the aggregate has from this interface.
    pub inputs: u32,
    /// How many outputs.
    pub outputs: u32,
    /// What Gazelle calls the group its inputs are: "USB A REC" on the Quadro, "USB REC" on the Studio+.
    pub input_group: String,
    /// What Gazelle calls the group its outputs are: "USB 1 PLAY", "USB PLAY".
    pub output_group: String,
}

/// One configured device, with everything known about it now.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DeviceReport {
    /// Its place in the setup, from zero, which is how a page finds its card whatever it is called.
    pub index: usize,
    /// Gazelle's name for the device: the person's own name for it, else its model.
    pub name: String,
    /// What the driver is given for it, and so what the driver's own record, its log and a
    /// measurement call it: the person's own name for the device, else its model's short form.
    pub daw_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clsid: Option<String>,
    /// True when a driver on this PC registers it.
    pub registered: bool,
    /// The registry key that matched, when one did.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entry_key: Option<String>,
    /// Which device of Gazelle's this is, when the configuration says.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_id: Option<DeviceId>,
    /// True when that device is attached to Gazelle now, which is what makes the live readings
    /// below possible.
    pub attached: bool,
    /// How `device_id` above was arrived at, so the page can say whether the person chose this
    /// interface or Gazelle worked it out.
    pub matched_by: MatchedBy,
    /// Only when nothing was matched: why not, and what would settle it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub match_note: Option<String>,
    /// Its channels in the aggregate, when its model is known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channels: Option<UsbChannels>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub family: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clock: Option<ClockReading>,
    pub driver: DriverSummary,
    /// The USB host controller it is on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub controller: Option<usb::UsbController>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub controller_error: Option<String>,
    /// True when it is the device that drives the callback.
    pub is_master: bool,
    /// True when the setup says where the cable the driver measures this interface's capture
    /// phase over runs. An interface without it is lined up by the figures its driver reports,
    /// which is what every session did before the measurement existed.
    pub phase_configured: bool,
}

/// Whether Gazelle's own driver is registered, and what its class id points at.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Registration {
    pub registered: bool,
    pub clsid: String,
    pub name: String,
    /// The DLL the class id names, when it is registered.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dll: Option<String>,
    /// Whether that DLL is a file now. Registered and false is "registered, pointing at a copy
    /// that is not there any more".
    pub dll_present: bool,
    pub message: String,
    /// The command a person could run themselves, when a DLL to register was found.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub register_command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unregister_command: Option<String>,
    /// Where the DLL was looked for, and whether one was found.
    pub dll_search: elevate::DllSearch,
    /// Whether this build carries the driver, and whether it could put its copy beside Gazelle.
    pub bundled: bundled::Status,
}

/// The whole answer `GET /api/v1/aggregate` gives.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AggregateAnswer {
    /// When this was put together, in milliseconds since the Unix epoch.
    pub read_at_ms: u64,
    /// True when the workspace has an aggregate section naming at least one device.
    pub configured: bool,
    /// The section as the workspace holds it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<crate::workspace::model::Aggregate>,
    /// Where the driver's own file is written.
    pub export_path: String,
    /// Every audio driver this PC publishes.
    pub drivers: Vec<registry::AsioEntry>,
    /// Why the drivers could not be listed, when they could not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drivers_error: Option<String>,
    pub registration: Registration,
    pub devices: Vec<DeviceReport>,
    /// The rate the aggregate puts every interface at when it opens, and where that comes from, or
    /// nothing when none is in force: the setup names none and the interfaces are not all running
    /// at one rate Gazelle can read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_in_force: Option<RateInForce>,
    pub ready: bool,
    pub reasons: Vec<readiness::Reason>,
    pub status: status::StatusReading,
    pub events: Vec<status::AggregateEvent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub events_error: Option<String>,
}
