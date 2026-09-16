//! Where the server keeps its state on disk.

use std::path::PathBuf;

/// The server's configuration directory as the environment names it, or `None` when it names
/// none (the server then uses the working directory).
///
/// `var` looks up an environment variable; it is a parameter so the precedence can be
/// tested without mutating the process environment.
pub fn config_dir(var: impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    // A set-but-empty variable is treated as unset, per the XDG Base Directory spec.
    let var = |name: &str| var(name).filter(|v| !v.is_empty());
    if let Some(dir) = var("GAZELLE_CONFIG_DIR") {
        return Some(PathBuf::from(dir));
    }
    if let Some(dir) = var("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(dir).join("gazelle"));
    }
    // Windows gives every process `APPDATA`, while `HOME` exists only inside shells like Git Bash:
    // without this a login entry or an Explorer launch had no config folder, and wrote to a working
    // directory it could not write (the user, 2026-09-16).
    if let Some(dir) = var("APPDATA") {
        return Some(PathBuf::from(dir).join("gazelle"));
    }
    var("HOME").map(|home| PathBuf::from(home).join(".config").join("gazelle"))
}

/// The workspace file to use when `--workspace` is not given.
pub fn default_workspace_path(var: impl Fn(&str) -> Option<String>) -> PathBuf {
    config_dir(var).map_or_else(|| PathBuf::from("workspace.json"), |dir| dir.join("workspace.json"))
}

/// The directory of user theme files for the web UI when `--themes-dir` is not given.
pub fn default_themes_dir(var: impl Fn(&str) -> Option<String>) -> PathBuf {
    config_dir(var).map_or_else(|| PathBuf::from("themes"), |dir| dir.join("themes"))
}

/// The directory a server's log file goes in when `--log-dir` is not given, or `None` when the
/// environment names nowhere for it (the server then writes no log file).
///
/// Logs are state, not configuration, so they do not follow `GAZELLE_CONFIG_DIR`: they go where
/// each platform keeps such things, in a `gazelle` folder as the configuration does —
/// `$XDG_STATE_HOME/gazelle/logs`, else `%LOCALAPPDATA%\gazelle\logs` (always set on Windows,
/// including for a login entry, which has no `HOME`), else `$HOME/.local/state/gazelle/logs`.
/// Never the working directory: a server started at login runs in a directory of the system's
/// choosing.
pub fn default_log_dir(var: impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let var = |name: &str| var(name).filter(|v| !v.is_empty());
    let base = var("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| var("LOCALAPPDATA").map(PathBuf::from))
        .or_else(|| var("HOME").map(|home| PathBuf::from(home).join(".local").join("state")))?;
    Some(base.join("gazelle").join("logs"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
    }

    #[test]
    fn explicit_config_dir_wins() {
        let p = default_workspace_path(env(&[
            ("GAZELLE_CONFIG_DIR", "/srv/gazelle"),
            ("XDG_CONFIG_HOME", "/xdg"),
            ("HOME", "/home/u"),
        ]));
        assert_eq!(p, PathBuf::from("/srv/gazelle/workspace.json"));
    }

    #[test]
    fn xdg_config_home_uses_a_gazelle_folder() {
        let p = default_workspace_path(env(&[("XDG_CONFIG_HOME", "/xdg"), ("HOME", "/home/u")]));
        assert_eq!(p, PathBuf::from("/xdg/gazelle/workspace.json"));
    }

    #[test]
    fn home_is_the_last_named_fallback() {
        let p = default_workspace_path(env(&[("HOME", "/home/u")]));
        assert_eq!(p, PathBuf::from("/home/u/.config/gazelle/workspace.json"));
        assert_eq!(default_workspace_path(env(&[])), PathBuf::from("workspace.json"));
    }

    /// Windows sets `APPDATA` for every process, and only Git Bash sets `HOME`, so preferring it gives
    /// a server started from a terminal, the tray, Explorer or a login entry the same workspace. A
    /// login entry starts in a folder Windows picks (usually System32) and cannot fall back to that.
    #[test]
    fn appdata_is_the_windows_config_folder_ahead_of_home() {
        let appdata = r"C:\Users\u\AppData\Roaming";
        let p = default_workspace_path(env(&[("APPDATA", appdata), ("HOME", "/home/u")]));
        assert_eq!(p, PathBuf::from(appdata).join("gazelle").join("workspace.json"));
        assert_eq!(default_themes_dir(env(&[("APPDATA", appdata)])), PathBuf::from(appdata).join("gazelle").join("themes"));
        // The explicit and XDG overrides still win, and an empty APPDATA names nothing.
        assert_eq!(default_workspace_path(env(&[("GAZELLE_CONFIG_DIR", "/srv/gazelle"), ("APPDATA", appdata)])), PathBuf::from("/srv/gazelle/workspace.json"));
        assert_eq!(default_workspace_path(env(&[("XDG_CONFIG_HOME", "/xdg"), ("APPDATA", appdata)])), PathBuf::from("/xdg/gazelle/workspace.json"));
        assert_eq!(default_workspace_path(env(&[("APPDATA", ""), ("HOME", "/home/u")])), PathBuf::from("/home/u/.config/gazelle/workspace.json"));
    }

    #[test]
    fn empty_gazelle_config_dir_is_treated_as_unset() {
        let p = default_workspace_path(env(&[
            ("GAZELLE_CONFIG_DIR", ""),
            ("XDG_CONFIG_HOME", "/xdg"),
            ("HOME", "/home/u"),
        ]));
        assert_eq!(p, PathBuf::from("/xdg/gazelle/workspace.json"));
    }

    #[test]
    fn empty_xdg_config_home_is_treated_as_unset() {
        let p = default_workspace_path(env(&[("XDG_CONFIG_HOME", ""), ("HOME", "/home/u")]));
        assert_eq!(p, PathBuf::from("/home/u/.config/gazelle/workspace.json"));
    }

    #[test]
    fn themes_live_beside_the_workspace_in_the_config_dir() {
        assert_eq!(default_themes_dir(env(&[("GAZELLE_CONFIG_DIR", "/srv/gazelle")])), PathBuf::from("/srv/gazelle/themes"));
        assert_eq!(default_themes_dir(env(&[("XDG_CONFIG_HOME", "/xdg")])), PathBuf::from("/xdg/gazelle/themes"));
        assert_eq!(default_themes_dir(env(&[("HOME", "/home/u")])), PathBuf::from("/home/u/.config/gazelle/themes"));
        assert_eq!(default_themes_dir(env(&[])), PathBuf::from("themes"));
        assert_eq!(config_dir(env(&[])), None);
    }

    #[test]
    fn logs_go_in_the_platform_state_folder_never_the_working_directory() {
        let all = [("XDG_STATE_HOME", "/state"), ("LOCALAPPDATA", r"C:\Users\u\AppData\Local"), ("HOME", "/home/u")];
        assert_eq!(default_log_dir(env(&all)), Some(PathBuf::from("/state/gazelle/logs")));
        assert_eq!(default_log_dir(env(&all[1..])), Some(PathBuf::from(r"C:\Users\u\AppData\Local").join("gazelle").join("logs")));
        assert_eq!(default_log_dir(env(&all[2..])), Some(PathBuf::from("/home/u/.local/state/gazelle/logs")));
        assert_eq!(default_log_dir(env(&[])), None);
    }

    #[test]
    fn empty_variables_do_not_name_a_log_folder_and_the_config_dir_is_not_one() {
        let p = default_log_dir(env(&[("XDG_STATE_HOME", ""), ("LOCALAPPDATA", ""), ("HOME", "/home/u")]));
        assert_eq!(p, Some(PathBuf::from("/home/u/.local/state/gazelle/logs")));
        assert_eq!(default_log_dir(env(&[("GAZELLE_CONFIG_DIR", "/srv/gazelle")])), None);
    }
}
