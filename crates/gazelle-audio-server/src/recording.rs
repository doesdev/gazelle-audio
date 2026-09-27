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
//! - **Auto-arm** (`crate::studio::auto_arm`) is driven from here, once a second: it arms at start,
//!   disarms when the interfaces go away and arms again when they come back. A disarm through
//!   [`RecordingService::disarm`] is a person's and pauses it; an arm through
//!   [`RecordingService::arm`] is a person's and resumes it.
//! - **A normal shutdown finishes the take** ([`RecordingService::shutdown`]): Quit, Ctrl-C, and the
//!   restart into an update all disarm before the process ends, so every file of a take is closed
//!   with its sizes and its log is complete.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

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
use crate::studio::auto_arm::{self, AutoArm, Inputs, Phase, Step};
use crate::studio::{Studio, StudioSettings};
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

/// Whether the interfaces the aggregate needs are here, asked once a tick.
pub type Presence = Arc<dyn Fn() -> bool + Send + Sync>;

/// How often auto-arm looks.
pub const AUTO_ARM_EVERY: Duration = Duration::from_secs(1);

/// Auto-arm's memory, and what the pages are told about it.
struct Auto {
    rules: AutoArm,
    /// The preset's name, for the pages, read from the workspace each tick.
    preset_name: Option<String>,
}

/// The recorder, and what it needs to arm from a preset.
pub struct RecordingService {
    recorder: Recorder,
    calibration: Arc<Calibration>,
    store: Arc<dyn WorkspaceStore>,
    devices: Arc<DeviceManager>,
    loopback: bool,
    live: watch::Sender<Value>,
    studio: Arc<Studio>,
    auto: Mutex<Auto>,
    presence: Mutex<Option<Presence>>,
    /// Set by [`RecordingService::shutdown`]: nothing arms from then on.
    stopping: std::sync::atomic::AtomicBool,
}

impl RecordingService {
    /// The service for the backend the server runs: this PC's drivers, or the loopback's fakes,
    /// with auto-arm off and nothing kept on disk.
    pub fn for_backend(loopback: bool, calibration: Arc<Calibration>, store: Arc<dyn WorkspaceStore>, devices: Arc<DeviceManager>) -> Arc<RecordingService> {
        RecordingService::for_backend_with(loopback, calibration, store, devices, Arc::new(Studio::in_memory(StudioSettings::default())))
    }

    /// As [`RecordingService::for_backend`], following these recording settings.
    pub fn for_backend_with(loopback: bool, calibration: Arc<Calibration>, store: Arc<dyn WorkspaceStore>, devices: Arc<DeviceManager>, studio: Arc<Studio>) -> Arc<RecordingService> {
        let originator = format!("Gazelle {}", crate::VERSION);
        let env: Arc<dyn Environment> = if loopback {
            Arc::new(gazelle_record::env::Loopback::new(originator))
        } else {
            Arc::new(gazelle_record::env::ThisPc { originator })
        };
        RecordingService::with_parts(env, loopback, calibration, store, devices, studio)
    }

    /// The service over any PC, which is how a test puts one made of data behind it.
    pub fn with_environment(env: Arc<dyn Environment>, loopback: bool, calibration: Arc<Calibration>, store: Arc<dyn WorkspaceStore>, devices: Arc<DeviceManager>) -> Arc<RecordingService> {
        RecordingService::with_parts(env, loopback, calibration, store, devices, Arc::new(Studio::in_memory(StudioSettings::default())))
    }

    fn with_parts(env: Arc<dyn Environment>, loopback: bool, calibration: Arc<Calibration>, store: Arc<dyn WorkspaceStore>, devices: Arc<DeviceManager>, studio: Arc<Studio>) -> Arc<RecordingService> {
        let (live, _) = watch::channel(Value::Null);
        let service = Arc::new(RecordingService {
            recorder: Recorder::new(env),
            calibration,
            store,
            devices,
            loopback,
            live,
            studio,
            auto: Mutex::new(Auto { rules: AutoArm::new(), preset_name: None }),
            presence: Mutex::new(None),
            stopping: std::sync::atomic::AtomicBool::new(false),
        });
        service.publish();
        service
    }

    /// The recording settings this service follows.
    pub fn studio(&self) -> &Arc<Studio> {
        &self.studio
    }

    /// Ask this instead of the attached devices whether the interfaces are here: for the tests,
    /// which make them come and go.
    pub fn set_presence(&self, presence: Presence) {
        if let Ok(mut current) = self.presence.lock() {
            *current = Some(presence);
        }
    }

    fn auto(&self) -> std::sync::MutexGuard<'_, Auto> {
        self.auto.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
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
            object.insert("auto_arm".into(), self.auto_arm_answer());
        }
        answer
    }

    /// Auto-arm as the pages show it: whether it is on, with which preset, and where it has got to.
    /// The next try is a moment in time rather than a countdown, so the live state does not change
    /// every second while it waits.
    fn auto_arm_answer(&self) -> Value {
        let settings = self.studio.get();
        let auto = self.auto();
        let status = auto.rules.status(Instant::now());
        let retry_at_ms = status.retry_in.and_then(|wait| (SystemTime::now() + wait).duration_since(SystemTime::UNIX_EPOCH).ok()).map(|d| (d.as_millis() / 1000) * 1000);
        json!({
            "on": settings.auto_arm_with().is_some(),
            "preset": settings.auto_arm_preset,
            "preset_name": auto.preset_name,
            "phase": if settings.auto_arm_with().is_some() { status.phase } else { Phase::Off },
            "reason": status.reason,
            "failures": status.failures,
            "retry_at_ms": retry_at_ms,
            "lost": status.lost,
        })
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

    /// **Arm** with a preset from the workspace, as a person asks: this resumes a paused auto-arm.
    /// Blocks while the drivers open: call it off the runtime.
    pub fn arm(&self, preset_id: &str) -> Result<(), Refusal> {
        let armed = self.arm_with(preset_id);
        if armed.is_ok() {
            let mut auto = self.auto();
            if auto.rules.is_paused() {
                tracing::info!("auto-arm resumed: armed by hand");
            }
            auto.rules.resume();
            auto.rules.armed();
        }
        armed
    }

    fn arm_with(&self, preset_id: &str) -> Result<(), Refusal> {
        if self.stopping.load(std::sync::atomic::Ordering::Acquire) {
            return Err(Refusal::new("arm_refused", "Gazelle is stopping"));
        }
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

    /// **Disarm**, as a person asks: this pauses auto-arm until Gazelle next starts or they arm by
    /// hand. Blocks while the take is finished and the drivers let go: call it off the runtime.
    pub fn disarm(&self, confirmed: bool) -> Result<bool, Refusal> {
        let disarmed = self.recorder.disarm(confirmed);
        if disarmed == Ok(true) && self.studio.get().auto_arm_with().is_some() {
            self.auto().rules.disarmed_by_hand();
            tracing::info!("auto-arm paused: disarmed by hand; it arms again when Gazelle next starts, or when you arm");
        }
        self.publish();
        disarmed.map_err(|why| Refusal::new("confirm_disarm", why))
    }

    /// Before the process ends: stop any take, finish its files and let go of the drivers. Quit,
    /// Ctrl-C and the restart into an update all come through here. Blocks: call it off the
    /// runtime.
    pub fn shutdown(&self) {
        // Auto-arm must not arm again behind the disarm, and nobody else may either.
        self.stopping.store(true, std::sync::atomic::Ordering::Release);
        // An arm under way is let finish, so it is disarmed rather than left holding the drivers.
        let since = Instant::now();
        while self.recorder.state() == "arming" && since.elapsed() < Duration::from_secs(35) {
            std::thread::sleep(Duration::from_millis(50));
        }
        let state = self.recorder.state();
        if state == "off" {
            return;
        }
        tracing::info!("shutting down while {state}: finishing any take and letting go of the audio drivers");
        match self.recorder.disarm(true) {
            Ok(_) => tracing::info!("the recorder is disarmed, and every file it was writing is finished"),
            Err(why) => tracing::warn!("the recorder did not disarm at shutdown: {why}"),
        }
        self.publish();
    }

    /// Change the recording settings. Turning auto-arm on, or giving it another preset, starts it
    /// afresh: it arms at its next tick.
    pub fn change_settings(&self, change: impl FnOnce(&mut StudioSettings)) -> Result<StudioSettings, String> {
        let before = self.studio.get();
        let after = self.studio.update(change)?;
        if before.auto_arm_with() != after.auto_arm_with() {
            match after.auto_arm_with() {
                Some(preset) => tracing::info!("auto-arm on, with preset {preset:?}"),
                None => tracing::info!("auto-arm off"),
            }
            self.auto().rules.resume();
        }
        if before.start_in_hub != after.start_in_hub {
            tracing::info!("start in the recording hub: {}", if after.start_in_hub { "on" } else { "off" });
        }
        self.publish();
        Ok(after)
    }

    /// The name of the workspace's preset `id`, if it has one.
    pub fn preset_name(&self, id: &str) -> Option<String> {
        self.store.load().ok()?.recording?.presets.into_iter().find(|p| p.id == id).map(|p| p.name)
    }

    /// Whether the workspace has the preset `id`, or the refusal that says it has not.
    pub fn preset_exists(&self, id: &str) -> Result<(), Refusal> {
        let workspace = self.store.load().map_err(|e| Refusal::new("storage_error", e.to_string()))?;
        let found = workspace.recording.as_ref().is_some_and(|recording| recording.presets.iter().any(|p| p.id == id));
        if found {
            Ok(())
        } else {
            Err(Refusal::new("no_preset", format!("there is no preset {id:?}: choose one on the Recording page")))
        }
    }

    /// Whether the interfaces the aggregate names are all attached; with none named, whether any
    /// interface is.
    fn interfaces_present(&self, workspace: Option<&Workspace>) -> bool {
        if let Some(presence) = self.presence.lock().ok().and_then(|p| p.clone()) {
            return presence();
        }
        let attached = self.devices.descriptors();
        let devices: Vec<&AggregateDevice> = workspace.and_then(|w| w.aggregate.as_ref()).map(|a| a.devices.iter().filter(|d| device_of(d).is_some()).collect()).unwrap_or_default();
        if devices.is_empty() {
            return attached.iter().any(|d| d.family.is_some());
        }
        if devices.iter().filter_map(|d| device_of(d)).all(|id| attached.iter().any(|d| &d.id == id)) {
            return true;
        }
        // An interface with no serial number can come back under another id. It counts as back when
        // as many interfaces of each model are attached as the aggregate names.
        let wanted: Vec<Option<String>> = devices.iter().map(|d| d.known.as_ref().and_then(|k| k.family.clone())).collect();
        if wanted.iter().any(Option::is_none) {
            return false;
        }
        let here: Vec<Option<String>> = attached.iter().filter(|d| d.family.is_some()).map(|d| d.family.clone()).collect();
        wanted.iter().all(|family| here.iter().filter(|h| *h == family).count() >= wanted.iter().filter(|w| *w == family).count())
    }

    /// One look by auto-arm: arm, disarm or wait, and say what happened. Blocks while it arms or
    /// disarms: call it off the runtime.
    pub fn auto_arm_tick(&self, now: Instant) {
        if self.stopping.load(std::sync::atomic::Ordering::Acquire) {
            return;
        }
        let settings = self.studio.get();
        let preset = settings.auto_arm_with().map(str::to_string);
        let workspace = if preset.is_some() { self.store.load().ok() } else { None };
        let preset_name = preset.as_ref().and_then(|id| workspace.as_ref()?.recording.as_ref()?.presets.iter().find(|p| &p.id == id).map(|p| p.name.clone()));
        let inputs = Inputs {
            preset: preset.as_deref(),
            recorder: auto_arm::Recorder::from_state(self.recorder.state()),
            measuring: self.calibration.is_running(),
            present: preset.is_none() || self.interfaces_present(workspace.as_ref()),
        };
        let step = {
            let mut auto = self.auto();
            auto.preset_name = preset_name.clone();
            auto.rules.decide(now, inputs)
        };
        let name = preset_name.or(preset).unwrap_or_default();
        match step {
            Step::Wait => {}
            Step::Arm(id) => {
                tracing::info!("auto-arm: arming with {name}");
                match self.arm_with(&id) {
                    Ok(()) => {
                        tracing::info!("auto-arm: armed with {name}");
                        self.auto().rules.armed();
                    }
                    Err(refusal) => {
                        let measuring = refusal.code == "measuring";
                        let wait = self.auto().rules.refused(now, measuring, &refusal.message);
                        if measuring {
                            tracing::info!("auto-arm: a measurement has the interfaces; looking again in {} s", wait.as_secs());
                        } else {
                            tracing::warn!("auto-arm: {name} was not armed: {} Trying again in {} s.", refusal.message, wait.as_secs());
                        }
                    }
                }
            }
            Step::Disarm => {
                tracing::warn!("auto-arm: the interfaces went away while armed; disarming, which finishes any take, and arming again when they are back");
                match self.recorder.disarm(true) {
                    Ok(_) => self.auto().rules.disarmed_for_loss(),
                    Err(why) => tracing::warn!("auto-arm: the recorder did not disarm: {why}"),
                }
            }
        }
        self.publish();
    }

    /// Auto-arm, once a second, for as long as the server runs. Nothing happens while it is off.
    pub async fn follow_auto_arm(self: Arc<Self>) {
        let mut every = tokio::time::interval(AUTO_ARM_EVERY);
        every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            every.tick().await;
            let service = Arc::clone(&self);
            // Arming opens drivers and disarming waits for the files: never on a runtime worker.
            if tokio::task::spawn_blocking(move || service.auto_arm_tick(Instant::now())).await.is_err() {
                tracing::warn!("auto-arm stopped part way through a look; it looks again in a second");
            }
        }
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

    /// A service on the loopback's recorder with auto-arm on for `preset`, and interfaces that come
    /// and go when the test says.
    fn auto_armed(preset: &str) -> (Arc<RecordingService>, Arc<std::sync::atomic::AtomicBool>) {
        let devices = DeviceManager::new(RegistrySet::builtin().unwrap());
        devices.attach_loopbacks(&[crate::registry_set::PID_QUADRO, crate::registry_set::PID_STUDIO], 64);
        let store = Arc::new(MemoryStore::default());
        store.save(&with_presets(vec![preset_with_room("band")])).unwrap();
        let studio = Arc::new(Studio::in_memory(StudioSettings { auto_arm: true, auto_arm_preset: Some(preset.into()), start_in_hub: false }));
        let service = RecordingService::for_backend_with(true, Arc::new(Calibration::this_pc()), store, devices, studio);
        let here = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let seen = here.clone();
        service.set_presence(Arc::new(move || seen.load(std::sync::atomic::Ordering::SeqCst)));
        (service, here)
    }

    /// A preset whose pre-roll is kept small, so arming in a test reserves little memory.
    fn preset_with_room(id: &str) -> RecordingPreset {
        RecordingPreset { preroll_max_seconds: Some(5.0), ..preset(id) }
    }

    fn phase(service: &RecordingService) -> String {
        service.answer()["auto_arm"]["phase"].as_str().unwrap_or_default().to_string()
    }

    #[test]
    fn auto_arm_arms_at_start_disarms_when_the_interfaces_go_and_arms_again_when_they_come_back() {
        let _one = hosting();
        let (service, here) = auto_armed("band");
        let start = Instant::now();
        let second = |n: u64| start + Duration::from_secs(n);
        assert_eq!(service.answer()["auto_arm"]["on"], true);
        service.auto_arm_tick(second(0));
        assert_eq!(service.recorder.state(), "armed", "armed as Gazelle starts");
        assert_eq!(phase(&service), "armed");
        assert_eq!(service.answer()["auto_arm"]["preset_name"], "Preset band");
        service.record().unwrap();
        std::thread::sleep(Duration::from_millis(400));

        here.store(false, std::sync::atomic::Ordering::SeqCst);
        service.auto_arm_tick(second(1));
        assert_eq!(service.recorder.state(), "recording", "a moment's absence does not cut a take");
        service.auto_arm_tick(second(7));
        assert_eq!(service.recorder.state(), "off", "gone for longer: disarmed, the take finished");
        assert_eq!(phase(&service), "waiting_for_interfaces");
        assert_eq!(service.answer()["auto_arm"]["lost"], true);
        let take = service.recorder.takes().into_iter().next().expect("the take was finished and listed");
        service.auto_arm_tick(second(8));
        assert_eq!(service.recorder.state(), "off", "not while they are away");

        here.store(true, std::sync::atomic::Ordering::SeqCst);
        service.auto_arm_tick(second(9));
        assert_eq!(service.recorder.state(), "armed", "back, and armed again");
        assert!(service.disarm(false).unwrap());
        for file in take.files.iter().chain(std::iter::once(&take.log)) {
            let _ = std::fs::remove_file(file);
        }
    }

    #[test]
    fn a_hand_disarm_is_not_undone_and_a_hand_arm_resumes_auto_arm() {
        let _one = hosting();
        let (service, _) = auto_armed("band");
        let start = Instant::now();
        service.auto_arm_tick(start);
        assert_eq!(service.recorder.state(), "armed");
        assert!(service.disarm(false).unwrap(), "the person disarms, for a DAW");
        for n in [1, 5, 60, 600] {
            service.auto_arm_tick(start + Duration::from_secs(n));
            assert_eq!(service.recorder.state(), "off", "auto-arm never takes the drivers back behind them");
        }
        assert_eq!(phase(&service), "paused");
        service.arm("band").unwrap();
        assert_eq!(phase(&service), "armed", "arming by hand resumes it");
        assert!(!service.auto().rules.is_paused());
        assert!(service.disarm(false).unwrap());
    }

    #[test]
    fn an_auto_arm_that_is_refused_backs_off_and_says_why() {
        let _one = hosting();
        let (service, _) = auto_armed("gone");
        let start = Instant::now();
        service.auto_arm_tick(start);
        assert_eq!(service.recorder.state(), "off");
        let answer = service.answer();
        assert_eq!(answer["auto_arm"]["phase"], "backing_off");
        assert!(answer["auto_arm"]["reason"].as_str().unwrap().contains("no preset"), "{answer}");
        assert_eq!(answer["auto_arm"]["failures"], 1);
        assert!(answer["auto_arm"]["retry_at_ms"].as_u64().is_some());
        // Nothing more is tried until the wait is over, however often it looks.
        for tenth in 1..50 {
            service.auto_arm_tick(start + Duration::from_millis(tenth * 100));
        }
        assert_eq!(service.answer()["auto_arm"]["failures"], 1);
        service.auto_arm_tick(start + Duration::from_secs(5));
        assert_eq!(service.answer()["auto_arm"]["failures"], 2, "and then once, with a longer wait after it");
        service.auto_arm_tick(start + Duration::from_secs(6));
        assert_eq!(service.answer()["auto_arm"]["failures"], 2);

        // Pointing it at a preset that is there starts afresh, and arms.
        service.change_settings(|s| s.auto_arm_preset = Some("band".into())).unwrap();
        service.auto_arm_tick(start + Duration::from_secs(7));
        assert_eq!(service.recorder.state(), "armed");
        assert!(service.disarm(false).unwrap());
    }

    #[test]
    fn auto_arm_waits_for_a_measurement_and_arms_once_it_is_over() {
        let _one = hosting();
        let (service, _) = auto_armed("band");
        let start = Instant::now();
        // What a measurement holds while it runs: the one turn at the aggregate.
        let turn = gazelle_calibrate::session::try_one_at_a_time().expect("nobody else has the aggregate");
        service.auto_arm_tick(start);
        assert_eq!(service.recorder.state(), "off");
        assert_eq!(phase(&service), "waiting_for_measurement");
        assert_eq!(service.answer()["auto_arm"]["failures"], 0, "a measurement is not a failure");
        drop(turn);
        service.auto_arm_tick(start + Duration::from_secs(1));
        assert_eq!(service.recorder.state(), "off", "it looks again after a short wait, not at once");
        service.auto_arm_tick(start + auto_arm::MEASURING_WAIT);
        assert_eq!(service.recorder.state(), "armed");
        assert!(service.disarm(false).unwrap());
    }

    /// Auto-arm's "the interfaces are here": the ones the aggregate names, by id, or, for an
    /// interface that came back under another id, as many of each model as it names.
    #[test]
    fn the_interfaces_are_here_when_the_aggregate_s_are_attached() {
        let (service, _) = service();
        let known = |id: &str, family: &str| AggregateDevice {
            device_id: Some(DeviceId(id.into())),
            known: Some(AggregateKnown { device_id: Some(DeviceId(id.into())), family: Some(family.into()), ..AggregateKnown::default() }),
            ..AggregateDevice::default()
        };
        let naming = |devices: Vec<AggregateDevice>| Workspace { aggregate: Some(Aggregate { devices, ..Aggregate::default() }), ..Workspace::default() };
        assert!(service.interfaces_present(None), "with none named, any interface will do");
        assert!(service.interfaces_present(Some(&naming(vec![known("loopback-0", "quadro"), known("loopback-1", "studio")]))));
        assert!(service.interfaces_present(Some(&naming(vec![known("serial:ELSEWHERE", "quadro"), known("loopback-1", "studio")]))), "back under another id");
        assert!(!service.interfaces_present(Some(&naming(vec![known("serial:A", "quadro"), known("serial:B", "quadro")]))), "one Quadro is not two");
        let unknown = AggregateDevice { device_id: Some(DeviceId("serial:C".into())), ..AggregateDevice::default() };
        assert!(!service.interfaces_present(Some(&naming(vec![unknown]))), "gone, and nothing says what it was");
    }

    #[test]
    fn auto_arm_off_never_arms_and_a_shutdown_finishes_the_take() {
        let _one = hosting();
        let (service, _) = auto_armed("band");
        service.change_settings(|s| s.auto_arm = false).unwrap();
        service.auto_arm_tick(Instant::now());
        assert_eq!(service.recorder.state(), "off");
        assert_eq!(service.answer()["auto_arm"], json!({"on": false, "preset": "band", "preset_name": null, "phase": "off", "reason": null, "failures": 0, "retry_at_ms": null, "lost": false}));

        service.arm("band").unwrap();
        service.record().unwrap();
        std::thread::sleep(Duration::from_millis(300));
        service.shutdown();
        assert_eq!(service.recorder.state(), "off");
        let take = service.recorder.takes().into_iter().next().expect("the take was finished at shutdown");
        assert!(take.seconds > 0.0);
        for file in take.files.iter().chain(std::iter::once(&take.log)) {
            let bytes = std::fs::read(file).expect("every file is there");
            if file.ends_with(".wav") {
                // The sizes were written at the end, so the file says how much audio it holds.
                let riff = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
                assert_eq!(riff + 8, bytes.len(), "{file} was finished");
            }
            let _ = std::fs::remove_file(file);
        }
    }
}
