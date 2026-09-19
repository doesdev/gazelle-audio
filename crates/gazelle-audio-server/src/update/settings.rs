//! What the user has said about updating, kept beside the workspace in the config directory
//! (`%APPDATA%\gazelle\update.json` on Windows).
//!
//! The defaults are: **check, and fetch what is found**. Waiting to be asked was worse in
//! practice than the megabytes it saved (the user, 2026-09-19): an update meant noticing a tray
//! line, clicking Download, waiting, and then restarting, and every one of those was a place to
//! stop. Nothing is applied by the download: the verified binary sits beside the running one
//! until the app is restarted, which is still only ever asked for.
//!
//! `auto_download: false` in the file is how to go back to being told and deciding.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Which releases count.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    /// Only full releases. The default.
    #[default]
    Stable,
    /// Pre-releases as well.
    Prerelease,
}

impl Channel {
    pub fn as_str(self) -> &'static str {
        match self {
            Channel::Stable => "stable",
            Channel::Prerelease => "prerelease",
        }
    }
}

/// The GitHub repository releases are read from, `owner/name`.
pub const DEFAULT_REPO: &str = "doesdev/gazelle-audio";
/// The GitHub API root. Configurable so a private mirror (and the tests' local stand-in)
/// can be used; a mirror still has to produce a signature the built-in key accepts.
pub const DEFAULT_API_BASE: &str = "https://api.github.com";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Whether to offer updates at all. Off removes the whole updater: no checks, the tray has no
    /// update items (Check for updates included) and the HTTP update routes are not served.
    pub check: bool,
    pub channel: Channel,
    /// How long between background checks. Zero means only on start.
    pub interval_hours: u64,
    /// Whether a found update is fetched and verified without being asked. On by default; the
    /// restart it waits for is still asked for every time.
    pub auto_download: bool,
    pub repo: String,
    pub api_base: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            check: true,
            channel: Channel::Stable,
            interval_hours: 6,
            auto_download: true,
            repo: DEFAULT_REPO.into(),
            api_base: DEFAULT_API_BASE.into(),
        }
    }
}

impl Settings {
    /// Read the settings file. A missing file is the defaults with nothing to say; an unreadable
    /// or malformed one is the defaults **and a warning**, because silently ignoring a file the
    /// user wrote is how a setting stops meaning anything.
    pub fn load(path: &Path) -> (Settings, Option<String>) {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (Settings::default(), None),
            Err(e) => return (Settings::default(), Some(format!("reading {}: {e}", path.display()))),
        };
        match serde_json::from_str(&text) {
            Ok(settings) => (settings, None),
            Err(e) => (Settings::default(), Some(format!("{} is not valid update settings ({e}); using the defaults", path.display()))),
        }
    }

    /// Write the settings, creating the directory if it is missing.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, serde_json::to_vec_pretty(self)?)
    }

    /// How often a background check runs, or `None` when it should only run at start.
    pub fn interval(&self) -> Option<std::time::Duration> {
        (self.interval_hours > 0).then(|| std::time::Duration::from_secs(self.interval_hours * 3600))
    }
}

/// The settings file to use, beside the workspace in the config directory.
pub fn default_settings_path(var: impl Fn(&str) -> Option<String>) -> PathBuf {
    crate::config::config_dir(var).map_or_else(|| PathBuf::from("update.json"), |dir| dir.join("update.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gazelle-update-settings-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("update.json")
    }

    #[test]
    fn the_defaults_check_and_fetch_what_they_find() {
        let s = Settings::default();
        assert!(s.check);
        assert!(s.auto_download, "waiting to be asked left updates unnoticed; only the restart is asked for");
        assert_eq!(s.channel, Channel::Stable);
        assert_eq!(s.interval(), Some(std::time::Duration::from_secs(6 * 3600)));
        assert_eq!(Settings { interval_hours: 0, ..Settings::default() }.interval(), None);
    }

    #[test]
    fn a_missing_file_is_the_defaults_and_says_nothing() {
        let path = temp("missing").parent().unwrap().join("nothing-here.json");
        assert_eq!(Settings::load(&path), (Settings::default(), None));
    }

    #[test]
    fn a_malformed_file_is_the_defaults_and_says_so() {
        let path = temp("malformed");
        std::fs::write(&path, "{not json").unwrap();
        let (settings, warning) = Settings::load(&path);
        assert_eq!(settings, Settings::default());
        assert!(warning.expect("a file the user wrote must not be ignored silently").contains("update settings"));
    }

    #[test]
    fn a_partial_file_keeps_the_defaults_for_what_it_leaves_out() {
        let path = temp("partial");
        std::fs::write(&path, r#"{"channel":"prerelease"}"#).unwrap();
        let (settings, warning) = Settings::load(&path);
        assert_eq!(warning, None);
        assert_eq!(settings.channel, Channel::Prerelease);
        assert!(settings.auto_download, "a key the file does not name is the default");
        assert!(settings.check);
        assert_eq!(settings.repo, DEFAULT_REPO);
    }

    /// A file written before downloading was automatic still loads, and one that asks not to
    /// download is still obeyed.
    #[test]
    fn a_file_from_before_still_loads_and_still_means_what_it_said() {
        let path = temp("old-file");
        // Everything a 1.1.0 install could have written, including the old default.
        std::fs::write(&path, r#"{"check":true,"channel":"stable","interval_hours":6,"auto_download":false,"repo":"doesdev/gazelle-audio","api_base":"https://api.github.com"}"#).unwrap();
        let (settings, warning) = Settings::load(&path);
        assert_eq!(warning, None);
        assert!(!settings.auto_download, "the user asked to be told, not fetched for");
        assert!(settings.check);
        assert_eq!(settings.interval_hours, 6);

        // The same file without that key at all: the new default, and nothing else changed.
        std::fs::write(&path, r#"{"check":true,"channel":"stable","interval_hours":6}"#).unwrap();
        let (settings, warning) = Settings::load(&path);
        assert_eq!(warning, None);
        assert!(settings.auto_download);
        assert_eq!(settings.repo, DEFAULT_REPO);
    }

    #[test]
    fn settings_round_trip_through_the_file() {
        let path = temp("roundtrip");
        let settings = Settings { check: false, channel: Channel::Prerelease, interval_hours: 1, ..Settings::default() };
        settings.save(&path).unwrap();
        assert_eq!(Settings::load(&path), (settings, None));
    }

    #[test]
    fn the_settings_live_in_the_config_directory() {
        let appdata = r"C:\Users\u\AppData\Roaming";
        let env = |k: &str| (k == "APPDATA").then(|| appdata.to_string());
        assert_eq!(default_settings_path(env), PathBuf::from(appdata).join("gazelle").join("update.json"));
        assert_eq!(default_settings_path(|_| None), PathBuf::from("update.json"));
    }

    #[test]
    fn a_channel_names_itself() {
        assert_eq!(Channel::Stable.as_str(), "stable");
        assert_eq!(Channel::Prerelease.as_str(), "prerelease");
        assert_eq!(serde_json::to_string(&Channel::Prerelease).unwrap(), "\"prerelease\"");
    }
}
