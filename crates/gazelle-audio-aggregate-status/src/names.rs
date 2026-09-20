//! The names and paths the two sides have to agree on, worked out in one place so that they
//! cannot disagree.
//!
//! Every name carries the format version, so a driver and a Gazelle built against different
//! versions of this crate do not meet at all rather than meeting and misreading each other. The
//! namespace is `Local\`, which is per logon session: the driver runs inside the DAW and Gazelle
//! runs as the same person, and nothing here should be reachable from another account.

use std::path::{Path, PathBuf};

use crate::record::FORMAT_VERSION;

/// The folder Gazelle keeps its settings in, under `%APPDATA%`.
pub const FOLDER: &str = "gazelle";

/// The file the driver reads its plan from. Named here because the watcher reloads it and Gazelle
/// writes it, so neither should be spelling it out for itself.
pub const CONFIG_FILE: &str = "aggregate.json";

/// The durable event log.
pub const EVENT_LOG_FILE: &str = "aggregate-events.log";

/// The shared section the driver publishes its live state in.
pub fn section_name() -> String {
    format!(r"Local\gazelle-aggregate-status-{FORMAT_VERSION}")
}

/// The event Gazelle signals to say "the configuration has changed, read it again".
pub fn reload_event_name() -> String {
    format!(r"Local\gazelle-aggregate-reload-{FORMAT_VERSION}")
}

/// `%APPDATA%\gazelle`, or nothing at all on a machine with no such variable.
pub fn folder() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    Some(Path::new(&appdata).join(FOLDER))
}

/// `%APPDATA%\gazelle\aggregate.json`.
pub fn config_path() -> Option<PathBuf> {
    Some(folder()?.join(CONFIG_FILE))
}

/// `%APPDATA%\gazelle\aggregate-events.log`.
pub fn event_log_path() -> Option<PathBuf> {
    Some(folder()?.join(EVENT_LOG_FILE))
}

/// A name as Windows wants one: UTF-16 with a terminator.
pub fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shared_name_carries_the_format_version() {
        // Two builds that disagree about the record must not find each other's section at all.
        assert!(section_name().ends_with(&FORMAT_VERSION.to_string()));
        assert!(reload_event_name().ends_with(&FORMAT_VERSION.to_string()));
        assert_ne!(section_name(), reload_event_name());
    }

    #[test]
    fn the_section_is_private_to_this_logon() {
        assert!(section_name().starts_with(r"Local\"), "{}", section_name());
        assert!(reload_event_name().starts_with(r"Local\"), "{}", reload_event_name());
    }

    #[test]
    fn the_files_sit_beside_the_configuration_the_driver_already_reads() {
        let path = event_log_path().expect("this machine has an APPDATA");
        let config = config_path().expect("this machine has an APPDATA");
        assert_eq!(path.parent(), config.parent(), "one folder, not two");
        assert_eq!(path.file_name().unwrap(), EVENT_LOG_FILE);
        assert_eq!(config.file_name().unwrap(), CONFIG_FILE);
    }

    #[test]
    fn a_wide_name_is_terminated() {
        assert_eq!(wide("ab"), vec![97, 98, 0]);
    }
}
