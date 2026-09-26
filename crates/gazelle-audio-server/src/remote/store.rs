//! What is kept on disk about phones: `remote.json` in the config directory, beside
//! `update.json` (`%APPDATA%\gazelle\remote.json` on Windows).
//!
//! It holds the setting that lets phones on this network reach Gazelle, and the paired phones,
//! each with the SHA-256 of its token and never the token itself. A missing file is the default:
//! phones not allowed and none paired. A file that cannot be read is the same default **and a
//! warning**, so a damaged file fails closed rather than open.
//!
//! `--no-persist` keeps all of this in memory, as it does the workspace: test servers are started
//! that way, and one must never leave a setting behind that opens the owner's Gazelle to the
//! network the next time it starts.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The file's contents.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RemoteFile {
    /// Whether Gazelle also listens on every network interface, so phones can reach it.
    pub allow_phones: bool,
    pub phones: Vec<StoredPhone>,
}

/// One paired phone.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredPhone {
    pub id: String,
    pub name: String,
    /// The SHA-256 of its token, in hex.
    pub token_sha256: String,
    /// When it was paired, in milliseconds since the Unix epoch.
    pub paired_ms: u64,
    #[serde(default)]
    pub last_seen_ms: Option<u64>,
    /// The address it was last seen from, as the socket reported it.
    #[serde(default)]
    pub last_address: Option<String>,
}

/// Where the file lives, or that it does not.
#[derive(Clone, Debug)]
pub enum Backing {
    File(PathBuf),
    /// `--no-persist`: nothing is read or written.
    Memory,
}

impl Backing {
    /// Read the file. A missing file is the defaults with nothing to say.
    pub fn load(&self) -> (RemoteFile, Option<String>) {
        let Backing::File(path) = self else { return (RemoteFile::default(), None) };
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (RemoteFile::default(), None),
            Err(e) => return (RemoteFile::default(), Some(format!("reading {}: {e}; phones are not allowed until it can be read", path.display()))),
        };
        match serde_json::from_str(&text) {
            Ok(file) => (file, None),
            Err(e) => (
                RemoteFile::default(),
                Some(format!("{} is not valid remote access settings ({e}); phones are not allowed and none are paired until it is fixed or replaced", path.display())),
            ),
        }
    }

    /// Write the file, whole: to a temporary file beside it, then renamed over it, so a crash
    /// part way through leaves the old file rather than half of a new one.
    pub fn save(&self, file: &RemoteFile) -> std::io::Result<()> {
        let Backing::File(path) = self else { return Ok(()) };
        write_atomically(path, &serde_json::to_vec_pretty(file)?)
    }
}

fn write_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, bytes)?;
    std::fs::rename(&temporary, path)
}

/// The file to use, beside the workspace in the config directory.
pub fn default_remote_path(var: impl Fn(&str) -> Option<String>) -> PathBuf {
    crate::config::config_dir(var).map_or_else(|| PathBuf::from("remote.json"), |dir| dir.join("remote.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gazelle-remote-store-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("remote.json")
    }

    fn phone() -> StoredPhone {
        StoredPhone {
            id: "0011223344556677".into(),
            name: "Pixel".into(),
            token_sha256: "ab".repeat(32),
            paired_ms: 1_789_700_000_000,
            last_seen_ms: Some(1_789_700_060_000),
            last_address: Some("192.168.1.20:51000".into()),
        }
    }

    #[test]
    fn the_setting_and_the_phones_persist_through_the_file() {
        let path = temp("roundtrip");
        let backing = Backing::File(path.clone());
        let file = RemoteFile { allow_phones: true, phones: vec![phone()] };
        backing.save(&file).unwrap();
        assert_eq!(backing.load(), (file, None));
        assert!(!path.with_extension("json.tmp").exists(), "the temporary file was renamed into place");
    }

    #[test]
    fn the_file_holds_hashes_never_tokens() {
        let path = temp("hashes");
        Backing::File(path.clone()).save(&RemoteFile { allow_phones: false, phones: vec![phone()] }).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("token_sha256"));
        assert!(!text.contains("\"token\""));
    }

    #[test]
    fn a_missing_file_is_off_with_nothing_paired_and_says_nothing() {
        let path = temp("missing").with_file_name("nothing-here.json");
        assert_eq!(Backing::File(path).load(), (RemoteFile::default(), None));
        assert!(!RemoteFile::default().allow_phones, "phones are off unless someone turns them on");
    }

    #[test]
    fn a_damaged_file_fails_closed_and_says_so() {
        let path = temp("damaged");
        std::fs::write(&path, r#"{"allow_phones": tru"#).unwrap();
        let (file, warning) = Backing::File(path).load();
        assert!(!file.allow_phones);
        assert!(file.phones.is_empty());
        assert!(warning.expect("a damaged file is not ignored silently").contains("not allowed"));
    }

    #[test]
    fn memory_backing_never_touches_the_disk() {
        let backing = Backing::Memory;
        backing.save(&RemoteFile { allow_phones: true, phones: vec![] }).unwrap();
        assert_eq!(backing.load(), (RemoteFile::default(), None));
    }

    #[test]
    fn the_file_lives_beside_update_json() {
        let appdata = r"C:\Users\u\AppData\Roaming";
        let env = |k: &str| (k == "APPDATA").then(|| appdata.to_string());
        assert_eq!(default_remote_path(env), PathBuf::from(appdata).join("gazelle").join("remote.json"));
        assert_eq!(
            default_remote_path(env).parent(),
            crate::update::settings::default_settings_path(env).parent(),
        );
    }
}
