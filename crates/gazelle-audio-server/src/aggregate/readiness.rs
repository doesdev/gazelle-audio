//! Whether this PC can run the aggregate, and if not, why, in words a person can act on.
//!
//! Every reason carries a code, so the page can offer the right fix, and where there is a fix
//! Gazelle already knows how to make, the reason names the route, the device and the value to
//! send. Nothing here reads anything: it is given what was read and decides, so every rule is
//! tested against devices made of data.
//!
//! The rules come from what phase 0 found at the hardware, and two of them would not have been
//! guessed from the code: two interfaces on one USB host controller cannot both stream, and a
//! device's clock source has to be read **while it streams**, because a device left on internal
//! silently becomes USB clocked the moment a DAW opens it.

use serde::Serialize;

use crate::aggregate::config::rate_in_force;
use crate::aggregate::{khz, rate_index, DeviceReport, RateFrom, RateInForce};
use crate::device::descriptor::DeviceId;
use crate::workspace::model::{Cable, Workspace};
use crate::workspace::topology;

/// Why the aggregate is not ready. A code per reason, so a page can offer the right fix without
/// reading the message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasonCode {
    /// The workspace has no aggregate setup, or it names no devices.
    NotConfigured,
    /// A device the setup names is not a driver registered on this PC.
    DeviceMissing,
    /// Gazelle's own aggregate driver is not registered, so no DAW can choose it.
    NotRegistered,
    /// It is registered and the DLL its class id names is not there any more.
    DllMissing,
    /// A configured device names one of Gazelle's devices, and that device is not attached now, so
    /// its live state cannot be read.
    DeviceNotAttached,
    /// A configured device names none of Gazelle's, and which connected interface it is could not
    /// be worked out, so nothing about it can be read or changed from here.
    DeviceNotMatched,
    /// Its audio driver could not be read.
    DriverUnreadable,
    /// Two or more devices are on one USB host controller, which cannot carry both.
    OneUsbController,
    /// A device's host controller could not be found.
    ControllerUnknown,
    /// The devices are not all at one sample rate.
    RatesDiffer,
    /// An interface's driver remembers a rate other than the one the interface runs at, which it
    /// puts the interface back to when a DAW opens it.
    DriverRateDiffers,
    /// Their driver buffer sizes are not all the same.
    BuffersDiffer,
    /// No digital cable is declared between two of the devices, so nothing says they share a clock.
    NoCable,
    /// A device's clock source, as it reports it now, is not the input its cable arrives on. This
    /// is the trap: the device is USB clocked and will drift.
    ClockNotCabled,
    /// A device says it is not locked to its clock.
    NotLocked,
    /// A cable already runs into this interface and nothing says which channels it is on, so the
    /// driver cannot measure where its capture actually started and will line it up by the figures
    /// its driver reports instead.
    PhaseNotMeasured,
}

/// How much a reason matters. A warning is something to know; `ready` is false only when there is
/// at least one blocking reason.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Blocking,
    Warning,
}

/// A request the page can make to put one reason right.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Fix {
    /// What it does: `match_buffers`, `set_clock_source`, `set_sample_rate`, `register`, or
    /// `set_setup_rate`, which the page makes itself, by putting the rate into the setup it keeps in
    /// the workspace: that is where the setup is edited, and the page's copy of it stays the truth.
    pub kind: &'static str,
    pub method: &'static str,
    /// The route, relative to `/api/v1/`.
    pub route: String,
    pub body: serde_json::Value,
    /// What a button would say.
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Reason {
    pub code: ReasonCode,
    pub severity: Severity,
    pub message: String,
    /// The configured device this is about, by Gazelle's name for it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    /// That device's place in the setup, from zero, which is how a page finds its card.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_id: Option<DeviceId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<Fix>,
}

impl Reason {
    fn new(code: ReasonCode, severity: Severity, message: impl Into<String>) -> Reason {
        Reason { code, severity, message: message.into(), device: None, device_index: None, device_id: None, fix: None }
    }

    fn about(mut self, device: &DeviceReport) -> Reason {
        self.device = Some(device.name.clone());
        self.device_index = Some(device.index);
        self.device_id.clone_from(&device.device_id);
        self
    }

    fn with(mut self, fix: Fix) -> Reason {
        self.fix = Some(fix);
        self
    }
}

/// Whether there is anything blocking.
pub fn ready(reasons: &[Reason]) -> bool {
    !reasons.iter().any(|reason| reason.severity == Severity::Blocking)
}

/// Every reason the aggregate is not ready, in the order a person would want to read them: what
/// is missing first, then the machine, then the clock.
pub fn reasons(configured: bool, registered: bool, dll_present: bool, devices: &[DeviceReport], workspace: &Workspace) -> Vec<Reason> {
    let mut reasons = Vec::new();
    if !configured {
        reasons.push(Reason::new(
            ReasonCode::NotConfigured,
            Severity::Blocking,
            "No interfaces have been chosen for the aggregate yet. Add the ones it should open, in the order their channels should appear.",
        ));
        return reasons;
    }
    if !registered {
        reasons.push(
            Reason::new(
                ReasonCode::NotRegistered,
                Severity::Blocking,
                "Gazelle Aggregate is not registered on this PC, so no DAW can choose it. Registering needs administrator rights.",
            )
            .with(register_fix()),
        );
    } else if !dll_present {
        reasons.push(
            Reason::new(
                ReasonCode::DllMissing,
                Severity::Blocking,
                "Gazelle Aggregate is registered, and the copy its registration points at is not there any more. Register it again from where the file is now.",
            )
            .with(register_fix()),
        );
    }
    reasons.extend(per_device(devices));
    reasons.extend(controllers(devices));
    reasons.extend(rates(devices));
    reasons.extend(driver_rates(devices, workspace.aggregate.as_ref().and_then(rate_in_force)));
    reasons.extend(buffers(devices));
    reasons.extend(clocks(devices, &workspace.cables));
    reasons.extend(phases(devices, &workspace.cables));
    reasons
}

/// Interfaces the driver could measure the capture phase of, and has not been told how to.
///
/// Two interfaces record a fixed number of samples apart within a session, and that number jumps
/// by a whole multiple of 32 samples between sessions: each interface's capture pipeline settles on
/// a different phase when its stream starts. The driver can measure that over the digital cable
/// that already locks them together, and line every session up to the phase that was measured when
/// the interfaces were last measured. It needs to be told which channels the cable is on, and until
/// it is, every session lines up by the figures the drivers report.
///
/// **This is not the trim.** The trim is the constant somebody measured once with a cable and a
/// click, and setting one does not answer this.
fn phases(devices: &[DeviceReport], cables: &[Cable]) -> Vec<Reason> {
    let mut reasons = Vec::new();
    for device in devices.iter().filter(|d| !d.is_master && !d.phase_configured) {
        let Some(id) = &device.device_id else { continue };
        let arriving = cables.iter().find(|cable| {
            &cable.to.device_id == id && devices.iter().any(|other| other.device_id.as_ref() == Some(&cable.from.device_id))
        });
        let Some(cable) = arriving else { continue };
        let from = devices
            .iter()
            .find(|other| other.device_id.as_ref() == Some(&cable.from.device_id))
            .map(|other| other.name.clone())
            .unwrap_or_default();
        reasons.push(
            Reason::new(
                ReasonCode::PhaseNotMeasured,
                Severity::Warning,
                format!(
                    "{} has a {} cable from {} and has not been set up for phase measurement, so every session lines it up by the figures its driver reports. Each interface's capture starts a whole multiple of 32 samples away from the last session's, so the two land a different distance apart each time. Say which channels that cable is on and measure the interfaces once, and the driver measures it at the start of every session and lines each session up to where it was when they were measured.",
                    device.name,
                    port_words(&cable.to.port),
                    from
                ),
            )
            .about(device),
        );
    }
    reasons
}

fn register_fix() -> Fix {
    Fix {
        kind: "register",
        method: "POST",
        route: "aggregate/register".into(),
        body: serde_json::json!({}),
        label: "Register the driver".into(),
    }
}

/// Reasons that are about one device on its own.
fn per_device(devices: &[DeviceReport]) -> Vec<Reason> {
    let mut reasons = Vec::new();
    for device in devices {
        if !device.registered {
            reasons.push(
                Reason::new(
                    ReasonCode::DeviceMissing,
                    Severity::Blocking,
                    format!(
                        "{} is not one of the audio drivers on this PC, so the aggregate would refuse to open. Plug the interface in and install its driver, or take it out of the list.",
                        device.name
                    ),
                )
                .about(device),
            );
            continue;
        }
        // Two ways an entry has no live device behind it, and they are different things to be
        // told. One names an interface of Gazelle's that is not plugged in now; the other names
        // none, and which of the connected ones it is could not be worked out. Only one of them
        // can be true of an entry, so only one is ever said.
        if !device.attached && device.device_id.is_some() {
            reasons.push(
                Reason::new(
                    ReasonCode::DeviceNotAttached,
                    Severity::Warning,
                    format!(
                        "{} is not connected to Gazelle, so its clock, its rate and its buffer cannot be checked. Its driver is installed, so the aggregate may still open it.",
                        device.name
                    ),
                )
                .about(device),
            );
            continue;
        }
        if !device.attached {
            // Why it could not be worked out is on the device's own report, as `match_note`, so
            // the card can say it in place rather than the message saying it twice.
            reasons.push(
                Reason::new(
                    ReasonCode::DeviceNotMatched,
                    Severity::Warning,
                    format!(
                        "Gazelle cannot tell which connected interface {} is, so its clock, its rate and its driver settings cannot be checked or changed from here. Choosing it on its card settles it.",
                        device.name
                    ),
                )
                .about(device),
            );
            continue;
        }
        if let Some(message) = &device.driver.message {
            reasons.push(
                Reason::new(
                    ReasonCode::DriverUnreadable,
                    Severity::Warning,
                    format!("{}'s audio driver could not be read, so its buffer size could not be checked. {message}", device.name),
                )
                .about(device),
            );
        }
    }
    reasons
}

/// Two interfaces on one host controller cannot both stream at these channel counts: phase 0 met
/// exactly this, as Windows saying "not enough USB resources" at the second driver.
fn controllers(devices: &[DeviceReport]) -> Vec<Reason> {
    let mut reasons = Vec::new();
    for device in devices.iter().filter(|d| d.registered) {
        if let Some(why) = &device.controller_error {
            reasons.push(
                Reason::new(
                    ReasonCode::ControllerUnknown,
                    Severity::Warning,
                    format!("Which USB controller {} is on could not be found, so Gazelle cannot say whether it shares one. {why}.", device.name),
                )
                .about(device),
            );
        }
    }
    // Every pair that shares one, reported once against the later of the two.
    for (at, device) in devices.iter().enumerate() {
        let Some(controller) = &device.controller else { continue };
        let Some(first) = devices[..at].iter().find(|other| other.controller.as_ref().is_some_and(|c| super::usb::same_controller(c, controller)))
        else {
            continue;
        };
        reasons.push(
            Reason::new(
                ReasonCode::OneUsbController,
                Severity::Blocking,
                format!(
                    "{} and {} are both on the {} USB controller, and one controller cannot carry both interfaces at these channel counts. Move one of them to a port on a different controller.",
                    first.name, device.name, controller.description
                ),
            )
            .about(device),
        );
    }
    reasons
}

/// The rate every device should be at: the first device's, which is the master unless the setup
/// says otherwise, and `is_master` says which that is.
fn wanted<'a, T: PartialEq>(devices: &'a [DeviceReport], of: impl Fn(&'a DeviceReport) -> Option<T>) -> Option<T> {
    devices.iter().find(|d| d.is_master).and_then(&of).or_else(|| devices.iter().find_map(of))
}

/// The rate an interface is running at: what the interface itself reports, and where it has not
/// reported, what its driver says.
fn running(device: &DeviceReport) -> Option<u32> {
    device.clock.as_ref().and_then(|clock| clock.running_rate()).or(device.driver.sample_rate)
}

/// Interfaces not all running at one rate. The interface's own report is what is compared, not the
/// rate its driver remembers: the driver's is a reason of its own ([`driver_rates`]).
fn rates(devices: &[DeviceReport]) -> Vec<Reason> {
    let Some(wanted_rate) = wanted(devices, running) else { return Vec::new() };
    devices
        .iter()
        .filter(|d| running(d).is_some_and(|rate| rate != wanted_rate))
        .map(|device| {
            let at = running(device).unwrap_or_default();
            let reason = Reason::new(
                ReasonCode::RatesDiffer,
                Severity::Blocking,
                format!(
                    "{} is at {at} Hz and the aggregate would run at {wanted_rate} Hz. Every interface has to be at one rate, because the aggregate will not resample.",
                    device.name
                ),
            )
            .about(device);
            match (device.device_id.clone(), rate_index(wanted_rate)) {
                (Some(id), Some(index)) => reason.with(Fix {
                    kind: "set_sample_rate",
                    method: "POST",
                    route: format!("devices/{id}/command/set_samp_rate"),
                    body: serde_json::json!({ "srate_idx": index }),
                    label: format!("Put {} at {wanted_rate} Hz", device.name),
                }),
                _ => reason,
            }
        })
        .collect()
}

/// An interface whose driver remembers another rate than the one the interface runs at.
///
/// The Quadro's driver was found remembering 44.1 kHz while the interface ran at 96 kHz, and it puts
/// the interface back to its own rate when a DAW opens it, with every interface clocked from it
/// following. What that does depends on the rate in force:
///
/// - none: nothing tells the drivers otherwise, so opening the aggregate moves the interface. That
///   stops it, and the fix puts the rate the interface runs at into the setup.
/// - the rate the interface runs at: the aggregate puts every interface there when it opens, so it
///   stays where it is, and it is only worth knowing.
/// - another rate, from the setup: opening moves every interface there anyway, which is what the
///   setup asks for, so the driver's own rate changes nothing and is not said.
///
/// The fix is the setup's rate rather than the driver's own: the aggregate setting the rate when it
/// opens is the supported way to set an interface driver's rate, and Gazelle has no other.
fn driver_rates(devices: &[DeviceReport], in_force: Option<RateInForce>) -> Vec<Reason> {
    devices
        .iter()
        .filter_map(|device| {
            let remembers = device.driver.sample_rate?;
            let runs = device.clock.as_ref().and_then(|clock| clock.running_rate())?;
            if remembers == runs {
                return None;
            }
            let says = format!("{}'s driver says {} while the interface runs at {}", device.name, khz(remembers), khz(runs));
            match in_force {
                Some(force) if force.hz == runs => {
                    let why = match force.from {
                        RateFrom::Setup => "the setup asks for it",
                        RateFrom::Interfaces => "it is the rate the interfaces are on",
                    };
                    Some(
                        Reason::new(
                            ReasonCode::DriverRateDiffers,
                            Severity::Warning,
                            format!("{says}. The aggregate puts it at {} when it opens, because {why}, so nothing moves.", khz(runs)),
                        )
                        .about(device),
                    )
                }
                Some(_) => None,
                None => Some(
                    Reason::new(
                        ReasonCode::DriverRateDiffers,
                        Severity::Blocking,
                        format!("{says}: opening the aggregate would move the interface to {}.", khz(remembers)),
                    )
                    .about(device)
                    .with(Fix {
                        kind: "set_setup_rate",
                        method: "PUT",
                        route: "workspace".into(),
                        body: serde_json::json!({ "rate": runs }),
                        label: format!("Put the aggregate at {}", khz(runs)),
                    }),
                ),
            }
        })
        .collect()
}

fn buffers(devices: &[DeviceReport]) -> Vec<Reason> {
    let Some(wanted_size) = wanted(devices, |d| d.driver.buffer_size) else { return Vec::new() };
    devices
        .iter()
        .filter(|d| d.driver.buffer_size.is_some_and(|size| size != wanted_size))
        .map(|device| {
            let at = device.driver.buffer_size.unwrap_or_default();
            Reason::new(
                ReasonCode::BuffersDiffer,
                Severity::Blocking,
                format!(
                    "{}'s driver buffer is {at} samples and the aggregate would run at {wanted_size}. Every interface has to be on the same buffer size.",
                    device.name
                ),
            )
            .about(device)
            .with(Fix {
                kind: "match_buffers",
                method: "POST",
                route: "aggregate/match-buffers".into(),
                body: serde_json::json!({ "buffer_size": wanted_size }),
                label: format!("Put every interface on {wanted_size} samples"),
            })
        })
        .collect()
}

/// The clock: every device but the master has to be following a cable from one of the others, and
/// it has to say so itself while it is streaming.
fn clocks(devices: &[DeviceReport], cables: &[Cable]) -> Vec<Reason> {
    let mut reasons = Vec::new();
    for device in devices.iter().filter(|d| d.attached && !d.is_master) {
        let Some(id) = &device.device_id else { continue };
        // A cable from another configured device into this one.
        let arriving = cables.iter().find(|cable| {
            &cable.to.device_id == id && devices.iter().any(|other| other.device_id.as_ref() == Some(&cable.from.device_id))
        });
        let Some(cable) = arriving else {
            reasons.push(
                Reason::new(
                    ReasonCode::NoCable,
                    Severity::Blocking,
                    format!(
                        "No digital cable is declared into {}, so nothing says it shares a clock with the others. Run one from a device the aggregate opens, and declare it, or the two will drift apart.",
                        device.name
                    ),
                )
                .about(device),
            );
            continue;
        };
        let Some(clock) = &device.clock else { continue };
        let family = device.family.as_deref().unwrap_or_default();
        if !topology::follows_port(family, &cable.to.port, clock.source_index) {
            let named = clock.source.clone().unwrap_or_else(|| format!("source {}", clock.source_index));
            let reason = Reason::new(
                ReasonCode::ClockNotCabled,
                Severity::Blocking,
                format!(
                    "{} says its clock is {named}, not the {} its cable arrives on. A device that was not put on its cable's input beforehand follows USB instead once a DAW opens it, and the two interfaces then drift apart.",
                    device.name,
                    port_words(&cable.to.port)
                ),
            )
            .about(device);
            reasons.push(match topology::clock_source_for_port(family, &cable.to.port) {
                Some((index, name)) => reason.with(Fix {
                    kind: "set_clock_source",
                    method: "POST",
                    route: format!("devices/{id}/command/set_sync_source"),
                    body: serde_json::json!({ "src_index": index }),
                    label: format!("Put {} on {name}", device.name),
                }),
                None => reason,
            });
        }
    }
    for device in devices.iter().filter(|d| d.attached && !d.is_master) {
        if device.clock.as_ref().is_some_and(|clock| !clock.locked) {
            reasons.push(
                Reason::new(
                    ReasonCode::NotLocked,
                    Severity::Blocking,
                    format!("{} says it is not locked to its clock. Check the cable, and that the device it follows is running.", device.name),
                )
                .about(device),
            );
        }
    }
    reasons
}

fn port_words(port: &str) -> &'static str {
    if port.starts_with("ADAT") {
        "ADAT"
    } else {
        "S/PDIF"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aggregate::usb::UsbController;
    use crate::aggregate::{ClockReading, DriverSummary};
    use crate::workspace::model::CableEnd;

    fn quadro() -> DeviceReport {
        DeviceReport {
            index: 0,
            name: "Quadro".into(),
            daw_name: "Quadro".into(),
            key: Some("Zen Quadro Synergy Core".into()),
            clsid: None,
            registered: true,
            entry_key: Some("Zen Quadro Synergy Core".into()),
            device_id: Some(DeviceId::from_serial("Q")),
            attached: true,
            matched_by: crate::aggregate::MatchedBy::Chosen,
            match_note: None,
            channels: None,
            family: Some("quadro".into()),
            clock: Some(ClockReading { source_index: 5, source: Some("USB".into()), locked: true, hz: 96000, rate_index: 4 }),
            driver: DriverSummary { sample_rate: Some(96000), buffer_size: Some(512), safe_mode: Some(true), asio_clients: Some(0), message: None },
            controller: Some(UsbController { instance_id: r"PCI\A".into(), description: "one".into() }),
            controller_error: None,
            is_master: true,
            phase_configured: true,
        }
    }

    fn studio() -> DeviceReport {
        DeviceReport {
            index: 1,
            name: "Studio+".into(),
            key: Some("ZenStudioTB".into()),
            device_id: Some(DeviceId::from_serial("S")),
            family: Some("studio".into()),
            clock: Some(ClockReading { source_index: 5, source: Some("S/PDIF".into()), locked: true, hz: 96000, rate_index: 4 }),
            controller: Some(UsbController { instance_id: r"PCI\B".into(), description: "another".into() }),
            is_master: false,
            ..quadro()
        }
    }

    /// The Quadro's S/PDIF out into the Studio+'s S/PDIF in, which is the setup phase 0 measured.
    fn cabled() -> Workspace {
        Workspace {
            cables: vec![Cable {
                id: "c1".into(),
                from: CableEnd { device_id: DeviceId::from_serial("Q"), port: "SPDIF_OUT".into(), first: 0 },
                to: CableEnd { device_id: DeviceId::from_serial("S"), port: "SPDIF_IN".into(), first: 0 },
                channels: 2,
            }],
            ..Workspace::default()
        }
    }

    fn codes(reasons: &[Reason]) -> Vec<ReasonCode> {
        reasons.iter().map(|r| r.code).collect()
    }

    #[test]
    fn the_setup_phase_0_measured_is_ready() {
        let reasons = reasons(true, true, true, &[quadro(), studio()], &cabled());
        assert!(reasons.is_empty(), "{reasons:?}");
        assert!(ready(&reasons));
    }

    /// An interface the driver could measure, and has not been told how to. It is something to
    /// know rather than something that stops a session: without it the interfaces still record,
    /// they just land a different distance apart each session.
    #[test]
    fn an_interface_that_could_be_phase_measured_and_is_not_set_up_for_it_is_said_so() {
        let not_set_up = DeviceReport { phase_configured: false, ..studio() };
        let reasons = reasons(true, true, true, &[quadro(), not_set_up], &cabled());
        assert_eq!(codes(&reasons), [ReasonCode::PhaseNotMeasured]);
        assert_eq!(reasons[0].device.as_deref(), Some("Studio+"));
        assert_eq!(reasons[0].device_id, Some(DeviceId::from_serial("S")));
        assert!(reasons[0].message.contains("S/PDIF cable from Quadro"), "{}", reasons[0].message);
        assert!(reasons[0].message.contains("multiple of 32 samples"), "{}", reasons[0].message);
        assert!(!reasons[0].message.to_ascii_lowercase().contains("trim"), "a phase is not a trim: {}", reasons[0].message);
        assert!(reasons[0].fix.is_none(), "the page writes the workspace itself");
        assert!(ready(&reasons), "it is something to know, not something that stops the driver opening");
    }

    #[test]
    fn an_interface_with_no_cable_into_it_is_not_asked_to_be_phase_measured_as_well() {
        // The measurement runs over the cable that locks the two together. With no cable declared
        // there is a blocking reason already, and saying this as well would be saying it twice.
        let not_set_up = DeviceReport { phase_configured: false, ..studio() };
        let uncabled = reasons(true, true, true, &[quadro(), not_set_up], &Workspace::default());
        assert_eq!(codes(&uncabled), [ReasonCode::NoCable]);
        // And the interface that drives the callback is never asked: it is what the others are
        // measured against.
        let master_only = DeviceReport { phase_configured: false, ..quadro() };
        let about_master = reasons(true, true, true, &[master_only, studio()], &cabled());
        assert!(about_master.is_empty(), "{about_master:?}");
    }

    #[test]
    fn nothing_configured_is_the_only_thing_said() {
        let reasons = reasons(false, false, false, &[], &Workspace::default());
        assert_eq!(codes(&reasons), [ReasonCode::NotConfigured]);
        assert!(!ready(&reasons));
    }

    #[test]
    fn an_unregistered_driver_and_one_pointing_at_a_copy_that_is_gone_are_told_apart() {
        let unregistered = reasons(true, false, false, &[quadro(), studio()], &cabled());
        assert_eq!(codes(&unregistered), [ReasonCode::NotRegistered]);
        assert_eq!(unregistered[0].fix.as_ref().unwrap().route, "aggregate/register");
        let stale = reasons(true, true, false, &[quadro(), studio()], &cabled());
        assert_eq!(codes(&stale), [ReasonCode::DllMissing]);
        assert!(stale[0].message.contains("not there any more"));
    }

    #[test]
    fn a_device_the_pc_does_not_have_is_named_and_nothing_else_is_said_about_it() {
        let missing = DeviceReport { registered: false, entry_key: None, attached: false, ..studio() };
        let reasons = reasons(true, true, true, &[quadro(), missing], &cabled());
        assert_eq!(codes(&reasons), [ReasonCode::DeviceMissing]);
        assert_eq!(reasons[0].device.as_deref(), Some("Studio+"));
        assert_eq!(reasons[0].device_id, Some(DeviceId::from_serial("S")));
    }

    #[test]
    fn a_device_gazelle_is_not_connected_to_is_a_warning_and_still_ready() {
        let away = DeviceReport { attached: false, ..studio() };
        let reasons = reasons(true, true, true, &[quadro(), away], &cabled());
        assert_eq!(codes(&reasons), [ReasonCode::DeviceNotAttached]);
        assert!(ready(&reasons), "its driver is there, so the aggregate may still open it");
    }

    /// A setup entry that names none of Gazelle's devices, with nothing else settling which it is.
    #[test]
    fn a_device_gazelle_cannot_tell_apart_is_said_so_once_and_not_also_called_disconnected() {
        let unmatched = DeviceReport {
            device_id: None,
            attached: false,
            matched_by: crate::aggregate::MatchedBy::None,
            match_note: Some("More than one Zen Studio+ is connected, so Gazelle cannot tell which of them Studio+ is. Choose it on its card.".into()),
            ..studio()
        };
        let reasons = reasons(true, true, true, &[quadro(), unmatched], &cabled());
        assert_eq!(codes(&reasons), [ReasonCode::DeviceNotMatched], "one reason, not this and not connected as well");
        assert_eq!(reasons[0].device.as_deref(), Some("Studio+"));
        assert_eq!(reasons[0].device_id, None);
        assert!(reasons[0].message.contains("cannot be checked or changed from here"));
        assert!(reasons[0].message.ends_with("Choosing it on its card settles it."));
        assert!(reasons[0].fix.is_none(), "the page writes the workspace itself");
        assert!(ready(&reasons), "it is something to know, not something that stops the driver opening");
    }

    #[test]
    fn a_driver_that_could_not_be_read_is_a_warning_that_says_what_went_wrong() {
        let unread = DeviceReport {
            driver: DriverSummary { message: Some("No driver API on this PC lists this device.".into()), ..DriverSummary::default() },
            ..studio()
        };
        let reasons = reasons(true, true, true, &[quadro(), unread], &cabled());
        assert_eq!(codes(&reasons), [ReasonCode::DriverUnreadable]);
        assert!(reasons[0].message.contains("No driver API on this PC lists this device."));
        assert!(ready(&reasons));
    }

    #[test]
    fn two_interfaces_on_one_usb_controller_are_named_together() {
        let together = DeviceReport { controller: quadro().controller, ..studio() };
        let reasons = reasons(true, true, true, &[quadro(), together], &cabled());
        assert_eq!(codes(&reasons), [ReasonCode::OneUsbController]);
        assert!(reasons[0].message.contains("Quadro and Studio+ are both on the one USB controller"), "{}", reasons[0].message);
        assert!(!ready(&reasons));
    }

    #[test]
    fn a_controller_that_could_not_be_found_is_a_warning_naming_the_device() {
        let lost = DeviceReport { controller: None, controller_error: Some("no device matching USB\\VID_23E5&PID_A100 is attached".into()), ..studio() };
        let reasons = reasons(true, true, true, &[quadro(), lost], &cabled());
        assert_eq!(codes(&reasons), [ReasonCode::ControllerUnknown]);
        assert!(ready(&reasons));
    }

    #[test]
    fn a_rate_that_differs_names_the_device_and_the_rate_to_send() {
        // The interface itself is at 44.1 kHz, and so is its driver.
        let slow = DeviceReport {
            clock: Some(ClockReading { source_index: 5, source: Some("S/PDIF".into()), locked: true, hz: 44100, rate_index: 1 }),
            driver: DriverSummary { sample_rate: Some(44100), ..studio().driver },
            ..studio()
        };
        let reasons = reasons(true, true, true, &[quadro(), slow], &cabled());
        assert_eq!(codes(&reasons), [ReasonCode::RatesDiffer]);
        let fix = reasons[0].fix.as_ref().expect("a rate has a route already");
        assert_eq!(fix.route, "devices/serial:S/command/set_samp_rate");
        assert_eq!(fix.body, serde_json::json!({ "srate_idx": 4 }), "96000 is index 4");
        assert!(reasons[0].message.contains("will not resample"));
    }

    /// The owner's other PC: the Quadro's driver remembers 44.1 kHz while the Quadro runs at 96 kHz.
    /// With nothing in force, opening the aggregate would move it, and the fix is the setup's rate.
    #[test]
    fn a_driver_remembering_another_rate_stops_it_when_nothing_would_put_the_interface_back() {
        let remembers = DeviceReport { driver: DriverSummary { sample_rate: Some(44100), ..quadro().driver }, ..quadro() };
        let reasons = reasons(true, true, true, &[remembers.clone(), studio()], &cabled());
        assert_eq!(codes(&reasons), [ReasonCode::DriverRateDiffers], "the interfaces are both at 96 kHz, so their rates do not differ");
        assert_eq!(reasons[0].severity, Severity::Blocking);
        assert_eq!(reasons[0].message, "Quadro's driver says 44.1 kHz while the interface runs at 96 kHz: opening the aggregate would move the interface to 44.1 kHz.");
        let fix = reasons[0].fix.as_ref().expect("the setup's rate puts it right");
        assert_eq!((fix.kind, fix.method, fix.route.as_str()), ("set_setup_rate", "PUT", "workspace"));
        assert_eq!(fix.body, serde_json::json!({ "rate": 96000 }));
        assert_eq!(fix.label, "Put the aggregate at 96 kHz");
    }

    #[test]
    fn a_driver_remembering_another_rate_is_only_worth_knowing_when_the_aggregate_puts_the_interface_back() {
        let remembers = DeviceReport { driver: DriverSummary { sample_rate: Some(44100), ..quadro().driver }, ..quadro() };
        let mut workspace = cabled();
        let seen = |rate| crate::workspace::model::AggregateKnown { rate: Some(rate), ..Default::default() };
        workspace.aggregate = Some(crate::workspace::model::Aggregate {
            devices: vec![
                crate::workspace::model::AggregateDevice { key: Some("Q".into()), known: Some(seen(96000)), ..Default::default() },
                crate::workspace::model::AggregateDevice { key: Some("S".into()), known: Some(seen(96000)), ..Default::default() },
            ],
            ..Default::default()
        });
        let following = reasons(true, true, true, &[remembers.clone(), studio()], &workspace);
        assert_eq!(codes(&following), [ReasonCode::DriverRateDiffers]);
        assert_eq!(following[0].severity, Severity::Warning);
        assert!(following[0].message.ends_with("The aggregate puts it at 96 kHz when it opens, because it is the rate the interfaces are on, so nothing moves."), "{}", following[0].message);
        assert!(following[0].fix.is_none());
        assert!(ready(&following));

        // A setup that asks for the rate the interface is on says so.
        workspace.aggregate.as_mut().unwrap().rate = Some(96000);
        assert!(reasons(true, true, true, &[remembers.clone(), studio()], &workspace)[0].message.contains("because the setup asks for it"));
        // A setup that asks for another rate moves every interface anyway: the driver's own rate changes nothing.
        workspace.aggregate.as_mut().unwrap().rate = Some(48000);
        assert!(codes(&reasons(true, true, true, &[remembers, studio()], &workspace)).is_empty());
    }

    #[test]
    fn a_buffer_that_differs_asks_for_every_device_to_be_matched_to_the_masters() {
        let big = DeviceReport { driver: DriverSummary { buffer_size: Some(1024), ..studio().driver }, ..studio() };
        let reasons = reasons(true, true, true, &[quadro(), big], &cabled());
        assert_eq!(codes(&reasons), [ReasonCode::BuffersDiffer]);
        let fix = reasons[0].fix.as_ref().unwrap();
        assert_eq!(fix.route, "aggregate/match-buffers");
        assert_eq!(fix.body, serde_json::json!({ "buffer_size": 512 }), "the master's size, not the other one's");
    }

    /// The master is what everything is compared against, wherever it is in the list.
    #[test]
    fn the_master_sets_the_rate_and_the_buffer_even_when_it_is_second() {
        let master_second = vec![DeviceReport { is_master: false, driver: DriverSummary { buffer_size: Some(1024), ..quadro().driver }, ..quadro() }, DeviceReport { is_master: true, ..studio() }];
        let reasons = reasons(true, true, true, &master_second, &cabled());
        let buffers = reasons.iter().find(|r| r.code == ReasonCode::BuffersDiffer).expect("{reasons:?}");
        assert_eq!(buffers.device.as_deref(), Some("Quadro"), "the one that is not the master is the one to change");
        assert_eq!(buffers.fix.as_ref().unwrap().body, serde_json::json!({ "buffer_size": 512 }));
        // The cable now runs the wrong way for this master, and that is a reason of its own.
        assert!(reasons.iter().any(|r| r.code == ReasonCode::NoCable && r.device.as_deref() == Some("Quadro")));
    }

    #[test]
    fn a_device_with_no_cable_into_it_is_told_so() {
        let reasons = reasons(true, true, true, &[quadro(), studio()], &Workspace::default());
        assert_eq!(codes(&reasons), [ReasonCode::NoCable]);
        assert!(reasons[0].message.contains("drift apart"));
        assert_eq!(reasons[0].device.as_deref(), Some("Studio+"));
    }

    /// The trap phase 0 found: a device on USB while a cable feeds it, which looks fine and is not.
    #[test]
    fn a_device_clocked_to_usb_rather_than_its_cable_is_the_reason_with_the_fix() {
        let usb_clocked = DeviceReport {
            clock: Some(ClockReading { source_index: 6, source: Some("USB".into()), locked: true, hz: 96000, rate_index: 4 }),
            ..studio()
        };
        let reasons = reasons(true, true, true, &[quadro(), usb_clocked], &cabled());
        assert_eq!(codes(&reasons), [ReasonCode::ClockNotCabled]);
        assert!(reasons[0].message.contains("its clock is USB, not the S/PDIF its cable arrives on"), "{}", reasons[0].message);
        let fix = reasons[0].fix.as_ref().unwrap();
        assert_eq!(fix.route, "devices/serial:S/command/set_sync_source");
        assert_eq!(fix.body, serde_json::json!({ "src_index": 5 }), "the Studio+'s S/PDIF");
        assert_eq!(fix.label, "Put Studio+ on S/PDIF");
    }

    #[test]
    fn a_device_that_is_not_locked_is_a_reason_of_its_own() {
        let adrift = DeviceReport {
            clock: Some(ClockReading { source_index: 5, source: Some("S/PDIF".into()), locked: false, hz: 0, rate_index: 4 }),
            ..studio()
        };
        let reasons = reasons(true, true, true, &[quadro(), adrift], &cabled());
        assert_eq!(codes(&reasons), [ReasonCode::NotLocked]);
        assert!(!ready(&reasons));
    }

    /// The master is clocked however it likes: it is what the others follow.
    #[test]
    fn the_masters_own_clock_is_never_a_reason() {
        let mut master = quadro();
        master.clock = Some(ClockReading { source_index: 5, source: Some("USB".into()), locked: false, hz: 96000, rate_index: 4 });
        let reasons = reasons(true, true, true, &[master, studio()], &cabled());
        assert!(reasons.is_empty(), "{reasons:?}");
    }

    /// A cable from a device the aggregate does not open says nothing about sharing a clock.
    #[test]
    fn a_cable_from_a_device_outside_the_aggregate_does_not_count() {
        let mut workspace = cabled();
        workspace.cables[0].from.device_id = DeviceId::from_serial("somebody else");
        let reasons = reasons(true, true, true, &[quadro(), studio()], &workspace);
        assert_eq!(codes(&reasons), [ReasonCode::NoCable]);
    }
}
