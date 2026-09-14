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
}
