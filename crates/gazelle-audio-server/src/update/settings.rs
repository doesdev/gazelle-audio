//! What the user has said about updating, kept beside the workspace in the config directory
//! (`%APPDATA%\gazelle\update.json` on Windows — P82).
//!
//! The defaults are the quiet ones: **check, never download by itself**. A check costs one
//! request and tells the user something they can act on; a download writes megabytes beside a
//! running binary and swaps the executable under them, so it waits to be asked.

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
/// The GitHub API root. Configurable so a private mirror — and the tests' local stand-in —
/// can be used; a mirror still has to produce a signature the built-in key accepts.
pub const DEFAULT_API_BASE: &str = "https://api.github.com";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Whether to look for updates at all. Off means the updater does nothing unasked; the tray
    /// and the HTTP endpoint can still check on request.
    pub check: bool,
    pub channel: Channel,
    /// How long between background checks. Zero means only on start.
    pub interval_hours: u64,
    /// Whether a found update is fetched without being asked. Off by default: downloading
    /// replaces the executable on disk, which is the user's call.
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
            auto_download: false,
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
    fn the_defaults_check_but_never_download_by_themselves() {
        let s = Settings::default();
        assert!(s.check);
        assert!(!s.auto_download, "a download replaces the executable; it waits to be asked");
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
        std::fs::write(&path, r#"{"channel":"prerelease","auto_download":true}"#).unwrap();
        let (settings, warning) = Settings::load(&path);
        assert_eq!(warning, None);
        assert_eq!(settings.channel, Channel::Prerelease);
        assert!(settings.auto_download);
        assert!(settings.check);
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
