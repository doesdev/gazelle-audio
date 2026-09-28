//! What this PC does about recording when nobody is looking: `recording.json` in the config
//! directory, beside `remote.json` and `update.json` (`%APPDATA%\gazelle\recording.json` on
//! Windows).
//!
//! **Why here and not in the workspace.** The presets are the workspace's, because a preset is part
//! of a setup and travels with a backup. These two are about this computer: whether it holds the
//! audio drivers from the moment it starts, and what it shows when it does. A workspace carried to
//! another machine, or restored from a backup, must not start holding that machine's drivers or
//! filling its screen. They are also not a phone's to change, and the workspace is.
//!
//! A missing file is the defaults: auto-arm off, starting in the ordinary window. An unreadable one
//! is the same **and a warning**, so a damaged file never leaves Gazelle holding the drivers.
//! `--no-persist` keeps all of it in memory, as it does the workspace and the phones: a test server
//! never reads the owner's file, so it never auto-arms.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// The file's contents.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StudioSettings {
    /// Arm whenever Gazelle starts, and again when the interfaces come back. Off by default.
    pub auto_arm: bool,
    /// The preset auto-arm arms with. Kept when auto-arm is turned off, so turning it on again from
    /// the tray needs nothing chosen.
    pub auto_arm_preset: Option<String>,
    /// Open straight into the full-screen recording hub when Gazelle starts, login starts included.
    pub start_in_hub: bool,
}

impl StudioSettings {
    /// The preset auto-arm should arm with now, when it is on and has one.
    pub fn auto_arm_with(&self) -> Option<&str> {
        self.auto_arm_preset.as_deref().filter(|id| self.auto_arm && !id.trim().is_empty())
    }
}

/// Where the settings live, or that they do not.
#[derive(Clone, Debug)]
pub enum Backing {
    File(PathBuf),
    /// `--no-persist`: nothing is read or written.
    Memory,
}

impl Backing {
    /// Read the file. A missing file is the defaults with nothing to say.
    pub fn load(&self) -> (StudioSettings, Option<String>) {
        self.load_as("recording settings", "auto-arm is off")
    }

    /// Read the file as `T`. A missing file is the defaults with nothing to say; an unreadable one is
    /// the defaults and a warning, which ends with `meanwhile`.
    pub fn load_as<T: DeserializeOwned + Default>(&self, what: &str, meanwhile: &str) -> (T, Option<String>) {
        let Backing::File(path) = self else { return (T::default(), None) };
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (T::default(), None),
            Err(e) => return (T::default(), Some(format!("reading {}: {e}; {meanwhile} until it can be read", path.display()))),
        };
        match serde_json::from_str(&text) {
            Ok(settings) => (settings, None),
            Err(e) => (T::default(), Some(format!("{} is not valid {what} ({e}); {meanwhile} until it is fixed or replaced", path.display()))),
        }
    }

    /// Write the file whole: to a temporary file beside it, then renamed over it.
    pub fn save(&self, settings: &StudioSettings) -> std::io::Result<()> {
        self.save_as(settings)
    }

    /// Write any settings whole, as [`Backing::save`] does.
    pub fn save_as<T: Serialize>(&self, settings: &T) -> std::io::Result<()> {
        let Backing::File(path) = self else { return Ok(()) };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let temporary = path.with_extension("json.tmp");
        std::fs::write(&temporary, serde_json::to_vec_pretty(settings)?)?;
        std::fs::rename(&temporary, path)
    }
}

/// The file to use, beside the workspace in the config directory.
pub fn default_studio_path(var: impl Fn(&str) -> Option<String>) -> PathBuf {
    crate::config::config_dir(var).map_or_else(|| PathBuf::from("recording.json"), |dir| dir.join("recording.json"))
}

/// The settings, shared by the tray, the routes, the recorder's auto-arm and the windows.
pub struct Studio {
    backing: Backing,
    settings: Mutex<StudioSettings>,
}

impl Studio {
    /// Read them from `backing`, with a warning to log when the file could not be used.
    pub fn new(backing: Backing) -> (Studio, Option<String>) {
        let (settings, warning) = backing.load();
        (Studio { backing, settings: Mutex::new(settings) }, warning)
    }

    /// In memory only, from these values: for `--no-persist` and the tests.
    pub fn in_memory(settings: StudioSettings) -> Studio {
        Studio { backing: Backing::Memory, settings: Mutex::new(settings) }
    }

    pub fn get(&self) -> StudioSettings {
        self.settings.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// Change them and write them back. Nothing changes when the file cannot be written.
    pub fn update(&self, change: impl FnOnce(&mut StudioSettings)) -> Result<StudioSettings, String> {
        let mut settings = self.settings.lock().map_err(|_| "the recording settings are unavailable".to_string())?;
        let mut next = settings.clone();
        change(&mut next);
        if next.auto_arm_preset.as_deref().is_some_and(|id| id.trim().is_empty()) {
            next.auto_arm_preset = None;
        }
        self.backing.save(&next).map_err(|e| format!("saving the recording settings: {e}"))?;
        *settings = next.clone();
        Ok(next)
    }

    /// Where they are kept, for the log.
    pub fn path(&self) -> Option<&Path> {
        match &self.backing {
            Backing::File(path) => Some(path),
            Backing::Memory => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gazelle-studio-settings-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("recording.json")
    }

    #[test]
    fn the_defaults_hold_nothing_and_start_in_the_window() {
        let settings = StudioSettings::default();
        assert!(!settings.auto_arm, "holding the drivers is never the default");
        assert!(!settings.start_in_hub);
        assert_eq!(settings.auto_arm_with(), None);
    }

    #[test]
    fn auto_arm_needs_both_the_switch_and_a_preset() {
        let with = |on: bool, preset: Option<&str>| StudioSettings { auto_arm: on, auto_arm_preset: preset.map(str::to_string), ..StudioSettings::default() };
        assert_eq!(with(true, Some("band")).auto_arm_with(), Some("band"));
        assert_eq!(with(false, Some("band")).auto_arm_with(), None, "off keeps the preset for next time, and arms nothing");
        assert_eq!(with(true, None).auto_arm_with(), None);
        assert_eq!(with(true, Some("  ")).auto_arm_with(), None);
    }

    #[test]
    fn the_settings_persist_through_the_file_and_a_damaged_one_fails_off() {
        let path = temp("roundtrip");
        let (studio, warning) = Studio::new(Backing::File(path.clone()));
        assert_eq!((studio.get(), warning), (StudioSettings::default(), None), "a missing file is the defaults, said nothing about");
        studio.update(|s| {
            s.auto_arm = true;
            s.auto_arm_preset = Some("band".into());
            s.start_in_hub = true;
        })
        .unwrap();
        assert!(!path.with_extension("json.tmp").exists(), "renamed into place");
        let (again, _) = Studio::new(Backing::File(path.clone()));
        assert_eq!(again.get().auto_arm_with(), Some("band"));
        assert!(again.get().start_in_hub);

        std::fs::write(&path, r#"{"auto_arm": tru"#).unwrap();
        let (damaged, warning) = Studio::new(Backing::File(path.clone()));
        assert!(!damaged.get().auto_arm, "a damaged file never leaves Gazelle holding the drivers");
        assert!(warning.expect("and says so").contains("auto-arm is off"));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn memory_never_touches_the_disk_and_the_file_lives_beside_remote_json() {
        let studio = Studio::in_memory(StudioSettings::default());
        studio.update(|s| s.auto_arm = true).unwrap();
        assert!(studio.get().auto_arm);
        assert_eq!(studio.path(), None);
        let appdata = r"C:\Users\u\AppData\Roaming";
        let env = |k: &str| (k == "APPDATA").then(|| appdata.to_string());
        assert_eq!(default_studio_path(env), PathBuf::from(appdata).join("gazelle").join("recording.json"));
        assert_eq!(default_studio_path(env).parent(), crate::remote::store::default_remote_path(env).parent());
    }
}
