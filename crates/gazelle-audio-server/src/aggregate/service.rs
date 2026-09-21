//! Putting the aggregate's answer together: what the registry says, what each device says about
//! itself now, what the driver says about itself, and what all of that comes to.
//!
//! Reading the audio driver loads a DLL and calls into it, so [`AggregateService::answer`] is
//! blocking and the route runs it off the runtime, the way the per device driver route does.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

use gazelle_audio_protocol::payload::Value;

use crate::aggregate::config::{device_name, master_of};
use crate::aggregate::elevate::{command_for, dll_candidates, DllSearch, Elevator};
use crate::aggregate::readiness::{self, Reason};
use crate::aggregate::registry::{self, entry_matches, AsioRegistry, AGGREGATE_CLSID, AGGREGATE_NAME};
use crate::aggregate::status::{AggregateEvent, StatusLink, StatusReading};
use crate::aggregate::usb::UsbTopology;
use crate::aggregate::{AggregateAnswer, ChannelNames, ClockReading, DeviceReport, DriverSummary, MatchedBy, Registration, STATUS_REPORT};
use crate::device::descriptor::{DeviceDescriptor, DeviceId};
use crate::device::manager::DeviceManager;
use crate::driver::{now_ms, DriverAnswer, DriverService, Reading};
use crate::workspace::model::{Aggregate, AggregateDevice, Workspace};
use crate::workspace::topology;

/// How many entries of the driver's event log an answer carries.
pub const EVENTS: usize = 50;

/// Everything the aggregate routes need, each part behind its own trait.
pub struct AggregateService {
    pub devices: Arc<DeviceManager>,
    pub driver: Arc<DriverService>,
    pub registry: Arc<dyn AsioRegistry>,
    pub topology: Arc<dyn UsbTopology>,
    pub link: Arc<dyn StatusLink>,
    pub elevator: Arc<dyn Elevator>,
    /// Where the driver's own configuration file is written.
    pub export_path: PathBuf,
    /// Where the driver's DLL is looked for, in order.
    pub dll_candidates: Vec<PathBuf>,
}

impl AggregateService {
    /// The service for this PC: the real registry, device tree, link and prompt.
    pub fn for_this_pc(devices: Arc<DeviceManager>, driver: Arc<DriverService>, export_path: PathBuf, exe_dir: Option<PathBuf>, appdata: Option<PathBuf>) -> Arc<Self> {
        Arc::new(AggregateService {
            devices,
            driver,
            registry: registry::for_this_pc(),
            topology: crate::aggregate::usb::for_this_pc(),
            link: crate::aggregate::status::for_this_pc(),
            elevator: crate::aggregate::elevate::for_this_pc(),
            export_path,
            dll_candidates: dll_candidates(exe_dir.as_deref(), appdata.as_deref(), None),
        })
    }

    /// Where the DLL is now, or every place that was looked in.
    pub fn find_dll(&self) -> DllSearch {
        crate::aggregate::elevate::find_dll(&self.dll_candidates, &|path| path.is_file())
    }

    /// The whole answer. Blocking: it may load a driver's DLL and call into it.
    pub fn answer(&self, workspace: &Workspace) -> AggregateAnswer {
        let config = workspace.aggregate.clone();
        let (entries, drivers_error) = match self.registry.entries() {
            Ok(entries) => (entries, None),
            Err(why) => (Vec::new(), Some(format!("The audio drivers on this PC could not be listed: {why}."))),
        };
        let attached = self.devices.descriptors();
        let devices = config.as_ref().map(|config| self.reports(config, &entries, &attached)).unwrap_or_default();
        let named: BTreeSet<String> = devices.iter().filter_map(|d| d.entry_key.clone()).collect();
        let registration = self.registration(&entries);
        let configured = config.as_ref().is_some_and(|config| !config.devices.is_empty());
        let reasons: Vec<Reason> = readiness::reasons(configured, registration.registered, registration.dll_present, &devices, workspace);
        let (events, events_error) = match self.link.events(EVENTS) {
            Ok(events) => (events, None),
            Err(why) => (Vec::new(), Some(why)),
        };
        AggregateAnswer {
            read_at_ms: now_ms(),
            configured,
            config,
            export_path: self.export_path.display().to_string(),
            drivers: registry::describe(self.registry.as_ref(), &named).unwrap_or_default(),
            drivers_error,
            registration,
            ready: readiness::ready(&reasons),
            reasons,
            devices,
            status: self.link.status(),
            events,
            events_error,
        }
    }

    /// The live event log on its own, for a page that polls it.
    pub fn events(&self, limit: usize) -> Result<Vec<AggregateEvent>, String> {
        self.link.events(limit)
    }

    pub fn status(&self) -> StatusReading {
        self.link.status()
    }

    fn registration(&self, entries: &[registry::RawEntry]) -> Registration {
        let entry = entries.iter().find(|entry| registry::same_clsid(&entry.clsid, AGGREGATE_CLSID));
        let registered_dll = entry.and_then(|entry| entry.dll.as_ref().ok()).cloned();
        let dll_present = registered_dll.as_deref().is_some_and(|dll| self.registry.dll_present(dll));
        let search = self.find_dll();
        let commands = match &search {
            DllSearch::Found { dll } => {
                let dll = PathBuf::from(dll);
                (Some(command_for(&dll, false)), Some(command_for(&dll, true)))
            }
            DllSearch::Missing { .. } => (None, None),
        };
        let message = match (entry.is_some(), dll_present, &registered_dll) {
            (false, _, _) => format!("{AGGREGATE_NAME} is not registered on this PC. A DAW will not list it until it is."),
            (true, true, Some(dll)) => format!("{AGGREGATE_NAME} is registered, pointing at {dll}."),
            (true, false, Some(dll)) => {
                format!("{AGGREGATE_NAME} is registered, and {dll} is not there any more. Register it again from where the file is now.")
            }
            (true, _, None) => format!("{AGGREGATE_NAME} is registered, and its registration names no DLL."),
        };
        Registration {
            registered: entry.is_some(),
            clsid: AGGREGATE_CLSID.to_string(),
            name: AGGREGATE_NAME.to_string(),
            dll: registered_dll,
            dll_present,
            message,
            register_command: commands.0,
            unregister_command: commands.1,
            dll_search: search,
        }
    }

    /// One report per configured device.
    fn reports(&self, config: &Aggregate, entries: &[registry::RawEntry], attached: &[DeviceDescriptor]) -> Vec<DeviceReport> {
        let master = master_of(config).map(device_name);
        config
            .devices
            .iter()
            .map(|device| {
                let name = device_name(device);
                let entry = entries.iter().find(|entry| entry_matches(entry, device.key.as_deref(), device.clsid.as_deref()));
                let found = match_device(device, entries, attached);
                let descriptor = found.descriptor;
                let mut report = DeviceReport {
                    is_master: master.as_deref() == Some(name.as_str()),
                    name,
                    key: device.key.clone(),
                    clsid: device.clsid.clone(),
                    registered: entry.is_some(),
                    entry_key: entry.map(|entry| entry.key.clone()),
                    device_id: descriptor.as_ref().map(|d| d.id.clone()).or_else(|| device.device_id.clone()),
                    attached: descriptor.is_some(),
                    matched_by: found.matched_by,
                    match_note: found.note,
                    channels: descriptor.as_ref().map(channel_names).unwrap_or_default(),
                    family: descriptor.as_ref().and_then(|d| d.family.clone()),
                    clock: None,
                    driver: DriverSummary::default(),
                    controller: None,
                    controller_error: None,
                    phase_configured: device.phase.is_some(),
                };
                if let Some(descriptor) = descriptor {
                    report.clock = self.clock(&descriptor);
                    report.driver = self.driver_summary(&descriptor);
                    match self.topology.controller_of(descriptor.vid, descriptor.pid, serial_of(&descriptor.id)) {
                        Ok(controller) => report.controller = Some(controller),
                        Err(why) => report.controller_error = Some(why),
                    }
                }
                report
            })
            .collect()
    }

    fn clock(&self, descriptor: &DeviceDescriptor) -> Option<ClockReading> {
        let family = descriptor.family.as_deref()?;
        let fields = self.devices.cyclic(&descriptor.id, STATUS_REPORT)?;
        let byte = |name: &str| number(&fields, name).unwrap_or_default();
        let source_index = u32::try_from(byte("sync_source")).unwrap_or_default();
        // The lock bit is named differently in the two models' reports.
        let locked = byte(if family == "quadro" { "locked" } else { "locked_wc" }) == 1;
        Some(ClockReading {
            source: topology::clock_sources(family).and_then(|sources| sources.get(source_index as usize)).map(|s| (*s).to_string()),
            source_index,
            locked,
            hz: u32::try_from((byte("sync_freq_hi") << 16) | (byte("sync_freq_mid") << 8) | byte("sync_freq_low")).unwrap_or_default(),
            rate_index: u32::try_from(byte("base_index")).unwrap_or_default(),
        })
    }

    fn driver_summary(&self, descriptor: &DeviceDescriptor) -> DriverSummary {
        let Some(serial) = serial_of(&descriptor.id) else {
            return DriverSummary { message: Some("This device reports no serial, which is how its driver is found.".into()), ..DriverSummary::default() };
        };
        match self.driver.read(descriptor.id.as_str(), serial, false).answer {
            DriverAnswer::Read(settings) => match settings.asio {
                Reading::Read { value } => DriverSummary {
                    sample_rate: Some(value.sample_rate),
                    buffer_size: Some(value.buffer_size),
                    safe_mode: Some(value.safe_mode),
                    asio_clients: Some(value.asio_clients),
                    message: None,
                },
                Reading::Unread { message } => DriverSummary { message: Some(message), ..DriverSummary::default() },
            },
            DriverAnswer::NoDriver { message } | DriverAnswer::NotFound { message } | DriverAnswer::Failed { message } => {
                DriverSummary { message: Some(message), ..DriverSummary::default() }
            }
        }
    }
}

/// Which connected interface a configured device is, and how that was arrived at.
pub struct DeviceMatch {
    /// The interface itself, when Gazelle can say which one it is.
    pub descriptor: Option<DeviceDescriptor>,
    pub matched_by: MatchedBy,
    /// Only when there is no match: why not, and what would settle it. It is written for the
    /// person, because it is what both the report and a refused write say.
    pub note: Option<String>,
}

/// **The one rule for which connected interface a configured device is.** Every answer and every
/// write goes through it, so a route that changes a device's driver finds exactly the device the
/// report on the page was about.
///
/// The setup's own `device_id` first, when it names one. Failing that, the model its driver's
/// registry entry says it is, when exactly one of that model is connected: the person never had to
/// say which Quadro is which if only one is plugged in, and refusing to act on what Gazelle plainly
/// knows was the bug this rule closes. Two of one model, or an entry whose words name no model, is
/// where guessing would put a reading or a write against the wrong interface, so nothing is matched
/// and the note says what would settle it.
pub fn match_device(device: &AggregateDevice, entries: &[registry::RawEntry], attached: &[DeviceDescriptor]) -> DeviceMatch {
    let name = device_name(device);
    let matched = |descriptor: DeviceDescriptor, matched_by: MatchedBy| DeviceMatch { descriptor: Some(descriptor), matched_by, note: None };
    let unmatched = |note: String| DeviceMatch { descriptor: None, matched_by: MatchedBy::None, note: Some(note) };

    if let Some(id) = &device.device_id {
        return match attached.iter().find(|d| &d.id == id) {
            Some(descriptor) => matched(descriptor.clone(), MatchedBy::Chosen),
            None => unmatched(format!(
                "{name} is set to {id}, which is not connected to Gazelle now. Plug it back in, or choose one of the connected interfaces on its card."
            )),
        };
    }
    let entry = entries.iter().find(|entry| entry_matches(entry, device.key.as_deref(), device.clsid.as_deref()));
    let Some(family) = entry.and_then(family_of) else {
        return unmatched(format!(
            "{name}'s driver entry does not say which model it is, so Gazelle cannot work out which connected interface it is. Choose it on its card."
        ));
    };
    let model = family_words(family);
    match only_of_family(attached, family) {
        Some(descriptor) => matched(descriptor, MatchedBy::WorkedOut),
        None if of_family(attached, family).count() > 1 => {
            unmatched(format!("More than one {model} is connected, so Gazelle cannot tell which of them {name} is. Choose it on its card."))
        }
        None => unmatched(format!("{name} is a {model}, and no {model} is connected to Gazelle now. Plug it in, or choose one of the connected interfaces on its card.")),
    }
}

/// What to call a model in a message, as the rest of the app names it.
fn family_words(family: &str) -> &str {
    match family {
        "quadro" => "Zen Quadro Synergy Core",
        "studio" => "Zen Studio+",
        other => other,
    }
}

/// Every input the Inputs page shows for a model, as `(what it calls them, topology type)`, in the
/// order that page shows them. A type the model does not have has no channels and drops out.
const INPUT_KINDS: &[(&str, &str)] = &[("Preamp", "PREAMP"), ("Line", "LINE_IN"), ("ADAT", "ADAT_IN"), ("S/PDIF", "SPDIF_IN")];

/// Every output the Outputs page shows, in `set_volume`'s id order. Each is a stereo pair, and a
/// model has the first [`topology::output_ids`] of them.
const OUTPUT_NAMES: &[&str] = &["Monitor", "HP1", "HP2", "Line out", "Reamp"];

/// What Gazelle calls one device's channels, taken from the model's own topology so the Inputs and
/// Outputs pages and this answer cannot drift apart. A hint for the page, not the audio driver's
/// channel order: see [`ChannelNames`].
fn channel_names(descriptor: &DeviceDescriptor) -> ChannelNames {
    let Some(family) = descriptor.family.as_deref() else { return ChannelNames::default() };
    let mut inputs = Vec::new();
    for (words, kind) in INPUT_KINDS {
        let count = topology::input_channels(family, kind).unwrap_or_default();
        inputs.extend((1..=count).map(|at| format!("{words} {at}")));
    }
    let outputs = OUTPUT_NAMES
        .iter()
        .take(topology::output_ids(family).unwrap_or_default() as usize)
        .flat_map(|words| ["L", "R"].map(|side| format!("{words} {side}")))
        .collect();
    ChannelNames { inputs, outputs, source: "gazelle" }
}

/// The serial in a `serial:<n>` device id.
pub fn serial_of(id: &DeviceId) -> Option<&str> {
    id.as_str().strip_prefix("serial:")
}

/// The one attached device of a model, or nothing when there is not exactly one.
///
/// Two of one model with nothing saying which is which: guessing would put a clock reading
/// against the wrong interface, which is worse than saying nothing. The page asks the person,
/// and the answer is kept as the device's `device_id`.
pub fn only_of_family(attached: &[DeviceDescriptor], family: &str) -> Option<DeviceDescriptor> {
    let mut found = of_family(attached, family);
    let first = found.next()?;
    found.next().is_none().then(|| first.clone())
}

/// Every attached device of a model. A loopback is left out: it has no audio driver to read.
fn of_family<'a>(attached: &'a [DeviceDescriptor], family: &'a str) -> impl Iterator<Item = &'a DeviceDescriptor> {
    attached.iter().filter(move |d| d.family.as_deref() == Some(family) && d.backend == "usb")
}

/// Which model a registry entry is, by what its key, description and DLL say. Matching the words
/// rather than a list of class ids is what makes a third interface work without a code change.
fn family_of(entry: &registry::RawEntry) -> Option<&'static str> {
    let text = format!(
        "{} {} {}",
        entry.key,
        entry.description.clone().unwrap_or_default(),
        entry.dll.clone().unwrap_or_default()
    )
    .to_ascii_lowercase();
    if text.contains("quadro") {
        Some("quadro")
    } else if text.contains("studio") {
        Some("studio")
    } else {
        None
    }
}

/// A report field as a number, whichever way it was decoded.
fn number(fields: &HashMap<String, Value>, name: &str) -> Option<i64> {
    match fields.get(name)? {
        Value::U64(n) => i64::try_from(*n).ok(),
        Value::I64(n) => Some(*n),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aggregate::registry::FakeRegistry;

    fn entry(key: &str, dll: &str) -> registry::RawEntry {
        FakeRegistry::entry(key, "{1}", dll)
    }

    #[test]
    fn a_registry_entry_says_which_model_it_is_by_its_words_not_by_a_list_of_class_ids() {
        assert_eq!(family_of(&entry("Zen Quadro Synergy Core", "q.dll")), Some("quadro"));
        assert_eq!(family_of(&entry("ZenStudioTB ASIO Driver", "s.dll")), Some("studio"));
        assert_eq!(family_of(&entry("Some Driver", r"C:\Program Files\Antelope\Zen Studio\api.dll")), Some("studio"));
        assert_eq!(family_of(&entry("Focusrite USB", "f.dll")), None);
    }

    fn descriptor(id: &str, family: &str, backend: &str) -> DeviceDescriptor {
        DeviceDescriptor {
            id: DeviceId(id.into()),
            vid: 0x23e5,
            pid: 0xa2f9,
            slug: None,
            model: None,
            family: Some(family.into()),
            command_count: None,
            identity_stable: true,
            backend: backend.into(),
            max_packet_size: 64,
        }
    }

    #[test]
    fn a_model_is_guessed_only_when_there_is_exactly_one_of_it() {
        let one = [descriptor("serial:Q", "quadro", "usb"), descriptor("serial:S", "studio", "usb")];
        assert_eq!(only_of_family(&one, "quadro").unwrap().id, DeviceId::from_serial("Q"));
        assert!(only_of_family(&one, "octo").is_none(), "a model that is not here");
        let two = [descriptor("serial:Q", "quadro", "usb"), descriptor("serial:Q2", "quadro", "usb")];
        assert!(only_of_family(&two, "quadro").is_none(), "two of one model: the person says which, not us");
        let loopbacks = [descriptor("loopback-0", "quadro", "loopback")];
        assert!(only_of_family(&loopbacks, "quadro").is_none(), "a loopback has no audio driver to read");
    }

    /// One entry of a setup, naming a driver by key and possibly one of Gazelle's devices.
    fn configured(name: &str, key: &str, id: Option<&str>) -> AggregateDevice {
        AggregateDevice { key: Some(key.into()), name: Some(name.into()), device_id: id.map(DeviceId::from_serial), ..AggregateDevice::default() }
    }

    fn one_of_each() -> Vec<DeviceDescriptor> {
        vec![descriptor("serial:Q", "quadro", "usb"), descriptor("serial:S", "studio", "usb")]
    }

    fn both_entries() -> Vec<registry::RawEntry> {
        vec![entry("Zen Quadro Synergy Core", r"C:\q.dll"), entry("ZenStudioTB ASIO Driver", r"C:\s.dll")]
    }

    #[test]
    fn the_interface_a_setup_names_is_the_one_it_is_matched_to_and_an_absent_one_says_so() {
        let found = match_device(&configured("Quadro", "Zen Quadro Synergy Core", Some("Q")), &both_entries(), &one_of_each());
        assert_eq!(found.matched_by, MatchedBy::Chosen);
        assert_eq!(found.descriptor.unwrap().id, DeviceId::from_serial("Q"));
        assert_eq!(found.note, None);

        let away = match_device(&configured("Quadro", "Zen Quadro Synergy Core", Some("elsewhere")), &both_entries(), &one_of_each());
        assert_eq!(away.matched_by, MatchedBy::None);
        assert!(away.descriptor.is_none());
        assert!(away.note.unwrap().contains("is not connected to Gazelle"), "the one it names is away, and Gazelle does not then guess another");
    }

    /// The bug this rule closes: a setup that never said which interface it is, with one of that
    /// model plugged in, read fine on the page and was refused every write.
    #[test]
    fn a_setup_that_names_no_interface_is_matched_by_the_model_its_driver_entry_gives() {
        let found = match_device(&configured("Quadro", "Zen Quadro Synergy Core", None), &both_entries(), &one_of_each());
        assert_eq!(found.matched_by, MatchedBy::WorkedOut);
        assert_eq!(found.descriptor.unwrap().id, DeviceId::from_serial("Q"));
        assert_eq!(found.note, None);
    }

    #[test]
    fn two_of_one_model_and_an_entry_naming_no_model_are_left_unmatched_and_say_what_would_settle_it() {
        let two = [descriptor("serial:Q", "quadro", "usb"), descriptor("serial:Q2", "quadro", "usb")];
        let crowded = match_device(&configured("Quadro", "Zen Quadro Synergy Core", None), &both_entries(), &two);
        assert_eq!(crowded.matched_by, MatchedBy::None);
        assert!(crowded.note.unwrap().contains("More than one Zen Quadro Synergy Core is connected"));

        let unknown_model = match_device(&configured("Something", "Focusrite USB", None), &[entry("Focusrite USB", r"C:\f.dll")], &one_of_each());
        assert_eq!(unknown_model.matched_by, MatchedBy::None);
        assert!(unknown_model.note.unwrap().contains("does not say which model it is"));

        let none_here = match_device(&configured("Quadro", "Zen Quadro Synergy Core", None), &both_entries(), &[descriptor("serial:S", "studio", "usb")]);
        assert!(none_here.note.unwrap().contains("no Zen Quadro Synergy Core is connected"));

        let no_entry = match_device(&configured("Quadro", "Zen Quadro Synergy Core", None), &[], &one_of_each());
        assert_eq!(no_entry.matched_by, MatchedBy::None, "no driver entry at all says no model either");
    }

    #[test]
    fn a_matched_device_carries_the_names_its_own_pages_give_its_channels_and_an_unmatched_one_none() {
        let names = channel_names(&descriptor("serial:Q", "quadro", "usb"));
        assert_eq!(names.source, "gazelle");
        assert_eq!(names.inputs[0], "Preamp 1");
        assert_eq!(names.inputs[3], "Preamp 4");
        assert_eq!(names.inputs[4], "ADAT 1", "the Quadro has no line inputs, so they are not in the list");
        assert_eq!(names.inputs.len(), 14, "four preamps, eight ADAT and a S/PDIF pair");
        assert_eq!(names.outputs, ["Monitor L", "Monitor R", "HP1 L", "HP1 R", "HP2 L", "HP2 R", "Line out L", "Line out R"]);

        let studio = channel_names(&descriptor("serial:S", "studio", "usb"));
        assert_eq!(studio.inputs[12], "Line 1", "the Studio+ has line inputs after its twelve preamps");
        assert_eq!(studio.inputs.len(), 38);
        assert_eq!(studio.outputs.len(), 10, "and a reamp output of its own");

        let unknown = channel_names(&DeviceDescriptor { family: None, ..descriptor("loopback-0", "quadro", "loopback") });
        assert_eq!(ChannelNames::default(), unknown, "a device of no known model is named nothing at all");
        assert_eq!(unknown.source, "none");
    }

    #[test]
    fn a_serial_id_gives_its_serial_and_nothing_else_does() {
        assert_eq!(serial_of(&DeviceId::from_serial("1000000000001")), Some("1000000000001"));
        assert_eq!(serial_of(&DeviceId::loopback(0)), None);
    }

    #[test]
    fn a_report_field_reads_as_a_number_however_it_was_decoded() {
        let fields: HashMap<String, Value> = [("a".to_string(), Value::U64(5)), ("b".to_string(), Value::I64(-1)), ("c".to_string(), Value::Bytes(vec![1]))]
            .into_iter()
            .collect();
        assert_eq!(number(&fields, "a"), Some(5));
        assert_eq!(number(&fields, "b"), Some(-1));
        assert_eq!(number(&fields, "c"), None, "a field that is not a number reads as nothing, not as zero");
        assert_eq!(number(&fields, "missing"), None);
    }
}
