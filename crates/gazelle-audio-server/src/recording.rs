//! The Recording page's server half: presets from the workspace turned into what the recorder arms
//! with, the one recorder, and its live state for the WebSocket.
//!
//! The recording itself is `gazelle-audio-record`. What is here is everything around it:
//!
//! - **Presets live in the workspace** (`recording.presets`), so they travel with a backup, and are
//!   edited through it like everything else there. A preset names its channels by interface and that
//!   interface's own input, and is resolved against the aggregate's setup only at Arm.
//! - **One owner of the aggregate.** Arm is refused while a measurement is running on the Aggregate
//!   page, and a measurement is refused while the recorder is armed (`crate::http::aggregate`). Under
//!   both sits the calibration's own turn, which the recorder holds from Arm to Disarm.
//! - **A preset cannot change under an armed recorder.** A workspace save that changes the preset
//!   it is armed with is refused until it is disarmed.
//! - **Phones may arm, record, stop and disarm**: that is the point of a remote. Editing presets
//!   stays on the computer, because a preset names a folder on it: a phone's workspace save that
//!   changes the presets is refused with `not_local`, and a phone never sees a folder picker.
//! - **The loopback records its test tones into the temporary folder**, whatever a preset says, so
//!   trying the page or running the tests never puts a file among real takes.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gazelle_aggregate::config::Config;
use gazelle_calibrate::Pick;
use gazelle_record::host::ArmRequest;
use gazelle_record::names::pattern_problem;
use gazelle_record::recorder::Environment;
use gazelle_record::sizing::{MIN_PREROLL_SECONDS, PERCENT_MAX, PERCENT_MIN};
use gazelle_record::{Preset, Recorder, SampleFormat};
use serde_json::{json, Value};
use tokio::sync::watch;

use crate::aggregate::calibrate::{sentence, Calibration};
use crate::aggregate::config::export_document;
use crate::aggregate::naming::{device_of, family_words};
use crate::config::{default_recordings_dir, loopback_recordings_dir};
use crate::device::manager::DeviceManager;
use crate::workspace::model::{Aggregate, AggregateDevice, AggregateKnown, Recording, RecordingPreset, Workspace, RECORDING_CAP_MAX, RECORDING_FORMATS, RECORDING_PATTERN_DEFAULT, RECORDING_PERCENT_DEFAULT};
use crate::workspace::store::WorkspaceStore;

/// How often the live state is sent while armed: five times a second, which is what a meter and a
/// clock need and no more.
pub const LIVE_EVERY: Duration = Duration::from_millis(200);

/// A refusal, with the code the page switches on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    pub code: &'static str,
    pub message: String,
}

impl Refusal {
    fn new(code: &'static str, why: impl AsRef<str>) -> Refusal {
        Refusal { code, message: sentence(why.as_ref()) }
    }
}

/// The recorder, and what it needs to arm from a preset.
pub struct RecordingService {
    recorder: Recorder,
    calibration: Arc<Calibration>,
    store: Arc<dyn WorkspaceStore>,
    devices: Arc<DeviceManager>,
    loopback: bool,
    live: watch::Sender<Value>,
}

impl RecordingService {
    /// The service for the backend the server runs: this PC's drivers, or the loopback's fakes.
    pub fn for_backend(loopback: bool, calibration: Arc<Calibration>, store: Arc<dyn WorkspaceStore>, devices: Arc<DeviceManager>) -> Arc<RecordingService> {
        let originator = format!("Gazelle {}", crate::VERSION);
        let env: Arc<dyn Environment> = if loopback {
            Arc::new(gazelle_record::env::Loopback::new(originator))
        } else {
            Arc::new(gazelle_record::env::ThisPc { originator })
        };
        RecordingService::with_environment(env, loopback, calibration, store, devices)
    }

    /// The service over any PC, which is how a test puts one made of data behind it.
    pub fn with_environment(env: Arc<dyn Environment>, loopback: bool, calibration: Arc<Calibration>, store: Arc<dyn WorkspaceStore>, devices: Arc<DeviceManager>) -> Arc<RecordingService> {
        let (live, _) = watch::channel(Value::Null);
        let service = Arc::new(RecordingService { recorder: Recorder::new(env), calibration, store, devices, loopback, live });
        service.publish();
        service
    }

    /// Whether the recorder holds the aggregate, or is about to.
    pub fn is_active(&self) -> bool {
        self.recorder.is_active()
    }

    /// Everything the page shows: the recorder's state, the preset it offers first, and where files
    /// go when a preset says nowhere.
    pub fn answer(&self) -> Value {
        let mut answer = serde_json::to_value(self.recorder.status()).unwrap_or(Value::Null);
        if let Some(object) = answer.as_object_mut() {
            object.insert("last_preset".into(), json!(self.recorder.last_preset()));
            object.insert("default_folder".into(), json!(self.default_folder().display().to_string()));
            object.insert("loopback".into(), json!(self.loopback));
        }
        answer
    }

    fn default_folder(&self) -> PathBuf {
        if self.loopback {
            loopback_recordings_dir()
        } else {
            default_recordings_dir(|k| std::env::var(k).ok())
        }
    }

    /// The takes recorded since the server started, newest first.
    pub fn takes(&self) -> Value {
        json!({ "takes": self.recorder.takes() })
    }

    /// Follow the live state. The value is the whole answer, as `GET /api/v1/recording` gives it.
    pub fn subscribe(&self) -> watch::Receiver<Value> {
        self.live.subscribe()
    }

    /// Send the live state on, if it has changed.
    pub fn publish(&self) {
        let answer = self.answer();
        self.live.send_if_modified(|current| {
            if *current == answer {
                return false;
            }
            *current = answer;
            true
        });
    }

    /// Keep the live state moving while armed, for as long as the server runs.
    pub async fn follow(self: Arc<Self>) {
        let mut every = tokio::time::interval(LIVE_EVERY);
        every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            every.tick().await;
            if self.recorder.is_active() || !self.live.borrow().get("state").is_some_and(|s| s == "off") {
                self.publish();
            }
        }
    }

    /// **Arm** with a preset from the workspace. Blocks while the drivers open: call it off the
    /// runtime.
    pub fn arm(&self, preset_id: &str) -> Result<(), Refusal> {
        if self.calibration.is_running() {
            return Err(Refusal::new("measuring", gazelle_record::host::MEASURING));
        }
        let workspace = self.store.load().map_err(|e| Refusal::new("arm_refused", e.to_string()))?;
        let preset = self.resolve(&workspace, preset_id)?;
        let armed = self.recorder.arm(preset);
        self.publish();
        armed.map_err(|why| Refusal::new(if why == gazelle_record::host::MEASURING { "measuring" } else { "arm_refused" }, why))
    }

    pub fn record(&self) -> Result<(), Refusal> {
        let started = self.recorder.record();
        self.publish();
        started.map_err(|why| Refusal::new(if why.contains("not armed") { "not_armed" } else { "record_refused" }, why))
    }

    pub fn stop(&self) -> bool {
        let stopped = self.recorder.stop();
        self.publish();
        stopped
    }

    /// Blocks while the take is finished and the drivers let go: call it off the runtime.
    pub fn disarm(&self, confirmed: bool) -> Result<bool, Refusal> {
        let disarmed = self.recorder.disarm(confirmed);
        self.publish();
        disarmed.map_err(|why| Refusal::new("confirm_disarm", why))
    }

    /// A preset from the workspace, as the recorder arms with it.
    pub fn resolve(&self, workspace: &Workspace, preset_id: &str) -> Result<Preset, Refusal> {
        let preset = workspace
            .recording
            .as_ref()
            .and_then(|recording| recording.presets.iter().find(|preset| preset.id == preset_id))
            .ok_or_else(|| Refusal::new("no_preset", format!("there is no preset {preset_id:?}: choose one on the Recording page")))?;
        check_preset(preset).map_err(|why| Refusal::new("arm_refused", format!("{}: {why}", preset.name)))?;
        let config = self.config_for(workspace).map_err(|why| Refusal::new("arm_refused", why))?;
        let folder = match (&preset.folder, self.loopback) {
            (_, true) => loopback_recordings_dir(),
            (Some(folder), false) if !folder.trim().is_empty() => PathBuf::from(folder.trim()),
            _ => self.default_folder(),
        };
        Ok(Preset {
            id: preset.id.clone(),
            name: preset.name.clone(),
            request: ArmRequest {
                picks: preset.channels.iter().map(|c| Pick::new(c.device as i32, c.channel as i32)).collect(),
                percent: preset.preroll_percent.unwrap_or(RECORDING_PERCENT_DEFAULT),
                cap_seconds: preset.preroll_max_seconds,
                config,
                source: if self.loopback { "the loopback's interfaces".into() } else { "Gazelle's workspace".into() },
            },
            folder,
            pattern: preset.pattern.clone().filter(|p| !p.trim().is_empty()).unwrap_or_else(|| RECORDING_PATTERN_DEFAULT.into()),
            format: if preset.format.as_deref() == Some("float32") { SampleFormat::Float32 } else { SampleFormat::Int24 },
        })
    }

    /// The aggregate's setup exactly as the driver's file says it, from the workspace. The
    /// loopback's interfaces are the fakes, found by model.
    fn config_for(&self, workspace: &Workspace) -> Result<Config, String> {
        let mut section = workspace.aggregate.clone().unwrap_or_default();
        if self.loopback {
            section = self.loopback_section(section);
        }
        let document = export_document(&section, workspace);
        Config::parse(&document.to_string()).map_err(|why| format!("the aggregate's setup is not one the driver would take: {why}"))
    }

    fn loopback_section(&self, mut section: Aggregate) -> Aggregate {
        let attached = self.devices.descriptors();
        let family_of = |device: &AggregateDevice| {
            device_of(device)
                .and_then(|id| attached.iter().find(|d| &d.id == id))
                .and_then(|d| d.family.clone())
                .or_else(|| device.known.as_ref().and_then(|k| k.family.clone()))
                // Last, the vendor driver's own name for it, which says which model it is.
                .or_else(|| device.key.as_deref().map(|key| if key.to_lowercase().contains("studio") { "studio" } else { "quadro" }.to_string()))
        };
        if section.devices.is_empty() {
            section.devices = attached
                .iter()
                .filter(|d| d.family.is_some())
                .map(|d| AggregateDevice {
                    device_id: Some(d.id.clone()),
                    known: Some(AggregateKnown { device_id: Some(d.id.clone()), family: d.family.clone(), model: d.family.as_deref().and_then(family_words).map(str::to_string), ..AggregateKnown::default() }),
                    ..AggregateDevice::default()
                })
                .collect();
        }
        for device in &mut section.devices {
            let family = family_of(device);
            device.key = Some(match family.as_deref() {
                Some("studio") => gazelle_record::sim::STUDIO_KEY.into(),
                _ => gazelle_record::sim::QUADRO_KEY.into(),
            });
            device.clsid = None;
            if device.known.is_none() {
                device.known = Some(AggregateKnown { device_id: device_of(device).cloned(), family: family.clone(), model: family.as_deref().and_then(family_words).map(str::to_string), ..AggregateKnown::default() });
            }
        }
        section.callback_master = None;
        section
    }

    /// Whether a workspace save may change the presets: a phone may not, and nobody may change the
    /// preset the recorder is armed with.
    pub fn check_change(&self, before: &Workspace, after: &Workspace, phone: bool) -> Result<(), Refusal> {
        let old = before.recording.clone().unwrap_or_default();
        let new = after.recording.clone().unwrap_or_default();
        if phone && old != new {
            return Err(Refusal { code: "not_local", message: "Recording presets are changed on the computer, not from a phone.".into() });
        }
        if let Some(armed) = self.recorder.armed_preset() {
            let find = |recording: &Recording| recording.presets.iter().find(|p| p.id == armed).cloned();
            if find(&old) != find(&new) {
                return Err(Refusal {
                    code: "recording_armed",
                    message: "Gazelle is armed with that preset, so it cannot be changed or removed now. Disarm first.".into(),
                });
            }
        }
        Ok(())
    }
}

/// Whether one preset could be armed with, as far as can be told without the drivers.
pub fn check_preset(preset: &RecordingPreset) -> Result<(), String> {
    if preset.channels.is_empty() {
        return Err("it records no channels yet".into());
    }
    for (at, channel) in preset.channels.iter().enumerate() {
        if preset.channels[..at].contains(channel) {
            return Err(format!("input {} of interface {} is chosen twice", channel.channel + 1, channel.device + 1));
        }
    }
    if let Some(pattern) = preset.pattern.as_deref().filter(|p| !p.trim().is_empty()) {
        if let Some(why) = pattern_problem(pattern) {
            return Err(why);
        }
    }
    if let Some(format) = &preset.format {
        if !RECORDING_FORMATS.contains(&format.as_str()) {
            return Err(format!("the format is one of {}, not {format:?}", RECORDING_FORMATS.join(", ")));
        }
    }
    if let Some(percent) = preset.preroll_percent {
        if !(PERCENT_MIN..=PERCENT_MAX).contains(&percent) {
            return Err(format!("the pre-roll takes between {PERCENT_MIN} and {PERCENT_MAX} percent of the free memory, not {percent}"));
        }
    }
    if let Some(cap) = preset.preroll_max_seconds {
        if !(MIN_PREROLL_SECONDS..=RECORDING_CAP_MAX).contains(&cap) {
            return Err(format!("the pre-roll limit is between {MIN_PREROLL_SECONDS} and {RECORDING_CAP_MAX} seconds, not {cap}"));
        }
    }
    if let Some(folder) = preset.folder.as_deref().map(str::trim).filter(|f| !f.is_empty()) {
        if !std::path::Path::new(folder).is_absolute() {
            return Err(format!("the folder has to be a whole path, such as D:\\Recordings, not {folder:?}"));
        }
    }
    Ok(())
}

/// The workspace's presets, checked as a save checks them: every preset has an id of its own and a
/// name. What a preset holds is checked at Arm, so one being built can be saved half done.
pub fn check_presets(recording: &Recording) -> Result<(), String> {
    for (at, preset) in recording.presets.iter().enumerate() {
        if preset.id.trim().is_empty() {
            return Err("every preset needs an id".into());
        }
        if recording.presets[..at].iter().any(|p| p.id == preset.id) {
            return Err(format!("the preset id {:?} is used twice", preset.id));
        }
        if preset.name.trim().is_empty() {
            return Err(format!("preset {:?} needs a name", preset.id));
        }
        if let Some(format) = &preset.format {
            if !RECORDING_FORMATS.contains(&format.as_str()) {
                return Err(format!("preset {:?}: the format is one of {}, not {format:?}", preset.name, RECORDING_FORMATS.join(", ")));
            }
        }
        if let Some(percent) = preset.preroll_percent {
            if !(PERCENT_MIN..=PERCENT_MAX).contains(&percent) {
                return Err(format!("preset {:?}: the pre-roll takes between {PERCENT_MIN} and {PERCENT_MAX} percent", preset.name));
            }
        }
    }
    Ok(())
}

/// One test at a time hosts the aggregate: its callbacks, and the turn, are the process's.
#[cfg(test)]
pub(crate) fn hosting() -> std::sync::MutexGuard<'static, ()> {
    static ONE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    ONE.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::descriptor::DeviceId;
    use crate::registry_set::RegistrySet;
    use crate::workspace::model::RecordingChannel;
    use crate::workspace::store::MemoryStore;

    fn service() -> (Arc<RecordingService>, Arc<MemoryStore>) {
        let devices = DeviceManager::new(RegistrySet::builtin().unwrap());
        devices.attach_loopbacks(&[crate::registry_set::PID_QUADRO, crate::registry_set::PID_STUDIO], 64);
        let store = Arc::new(MemoryStore::default());
        let service = RecordingService::for_backend(true, Arc::new(Calibration::this_pc()), store.clone(), devices);
        (service, store)
    }

    fn preset(id: &str) -> RecordingPreset {
        RecordingPreset {
            id: id.into(),
            name: format!("Preset {id}"),
            channels: vec![RecordingChannel { device: 0, channel: 0 }, RecordingChannel { device: 1, channel: 2 }],
            ..RecordingPreset::default()
        }
    }

    fn with_presets(presets: Vec<RecordingPreset>) -> Workspace {
        Workspace { recording: Some(Recording { presets, ..Recording::default() }), ..Workspace::default() }
    }

    #[test]
    fn a_preset_resolves_against_the_loopbacks_interfaces_and_writes_into_the_temporary_folder() {
        let (service, _) = service();
        let mut named = with_presets(vec![RecordingPreset { folder: Some(r"D:\Takes".into()), format: Some("float32".into()), ..preset("p") }]);
        named.aliases.insert(DeviceId::loopback(0), "Desk".into());
        let resolved = service.resolve(&named, "p").expect("resolved");
        assert_eq!(resolved.folder, loopback_recordings_dir(), "never a real folder from the loopback");
        assert_eq!(resolved.format, SampleFormat::Float32);
        assert_eq!(resolved.pattern, RECORDING_PATTERN_DEFAULT);
        assert_eq!(resolved.request.percent, RECORDING_PERCENT_DEFAULT);
        assert_eq!(resolved.request.picks, vec![Pick::new(0, 0), Pick::new(1, 2)]);
        let names: Vec<Option<String>> = resolved.request.config.devices.iter().map(|d| d.name.clone()).collect();
        assert_eq!(names, vec![Some("Desk".into()), Some("Studio+".into())], "named as Gazelle names them");
        assert_eq!(resolved.request.config.devices[1].key.as_deref(), Some(gazelle_record::sim::STUDIO_KEY));
        assert_eq!(service.resolve(&named, "nope").unwrap_err().code, "no_preset");
    }

    #[test]
    fn a_preset_that_could_not_record_says_why() {
        assert!(check_preset(&RecordingPreset { channels: vec![], ..preset("p") }).unwrap_err().contains("no channels"));
        let twice = RecordingPreset { channels: vec![RecordingChannel { device: 0, channel: 1 }; 2], ..preset("p") };
        assert!(check_preset(&twice).unwrap_err().contains("chosen twice"));
        assert!(check_preset(&RecordingPreset { pattern: Some("{take}".into()), ..preset("p") }).unwrap_err().contains("{channel}"));
        assert!(check_preset(&RecordingPreset { preroll_percent: Some(80.0), ..preset("p") }).is_err());
        assert!(check_preset(&RecordingPreset { preroll_max_seconds: Some(2.0), ..preset("p") }).is_err());
        assert!(check_preset(&RecordingPreset { folder: Some("Takes".into()), ..preset("p") }).unwrap_err().contains("whole path"));
        assert!(check_presets(&Recording { presets: vec![preset("a"), preset("a")], ..Recording::default() }).unwrap_err().contains("used twice"));
    }

    #[test]
    fn a_phone_may_not_change_the_presets_and_nobody_may_change_the_armed_one() {
        let _one = hosting();
        let (service, store) = service();
        let before = with_presets(vec![preset("p"), preset("q")]);
        let mut after = before.clone();
        after.recording.as_mut().unwrap().presets[1].name = "Renamed".into();
        assert_eq!(service.check_change(&before, &after, true).unwrap_err().code, "not_local");
        assert!(service.check_change(&before, &after, false).is_ok());
        assert!(service.check_change(&before, &before, true).is_ok(), "a phone's save that leaves them alone is fine");

        store.save(&before).unwrap();
        service.arm("p").expect("armed on the loopback");
        assert!(service.check_change(&before, &after, false).is_ok(), "another preset may change");
        let mut touched = before.clone();
        touched.recording.as_mut().unwrap().presets[0].preroll_percent = Some(20.0);
        assert_eq!(service.check_change(&before, &touched, false).unwrap_err().code, "recording_armed");
        let mut removed = before.clone();
        removed.recording.as_mut().unwrap().presets.remove(0);
        assert_eq!(service.check_change(&before, &removed, false).unwrap_err().code, "recording_armed");
        assert!(service.disarm(false).unwrap());
    }
}
