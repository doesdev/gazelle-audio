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
//! (`status`) and the elevated helper that registers the DLL (`elevate`). No test reads a real
//! registry, maps a section or elevates anything.

pub mod calibrate;
pub mod config;
pub mod elevate;
pub mod export;
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

/// The names Gazelle shows for a device's channels, as the Inputs and Outputs pages name them.
///
/// **A hint for the page, not ground truth about the audio driver.** These are Gazelle's own
/// names, in the order its own pages show them; the vendor driver publishes its channels in
/// whatever order it likes, and the two need not line up. They are here so a page offering the
/// person a label for each channel can start from a name they already recognise, not so anything
/// can be matched up by position.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ChannelNames {
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    /// `gazelle` when they came from the matched device, `none` when there is no matched device.
    pub source: &'static str,
}

impl Default for ChannelNames {
    fn default() -> Self {
        ChannelNames { inputs: Vec::new(), outputs: Vec::new(), source: "none" }
    }
}

/// One configured device, with everything known about it now.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DeviceReport {
    /// What the configuration calls it, which is what its channels are named after.
    pub name: String,
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
    /// What Gazelle calls this device's channels, for a page offering to label them.
    pub channels: ChannelNames,
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
    pub ready: bool,
    pub reasons: Vec<readiness::Reason>,
    pub status: status::StatusReading,
    pub events: Vec<status::AggregateEvent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub events_error: Option<String>,
}
