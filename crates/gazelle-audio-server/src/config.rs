//! Where the server keeps its state on disk.

use std::path::PathBuf;

/// The workspace file to use when `--workspace` is not given.
///
/// `var` looks up an environment variable; it is a parameter so the precedence can be
/// tested without mutating the process environment.
pub fn default_workspace_path(var: impl Fn(&str) -> Option<String>) -> PathBuf {
    if let Some(dir) = var("GAZELLE_CONFIG_DIR") {
        return PathBuf::from(dir).join("workspace.json");
    }
    if let Some(dir) = var("XDG_CONFIG_HOME") {
        return PathBuf::from(dir).join("gazelle").join("workspace.json");
    }
    if let Some(home) = var("HOME") {
        return PathBuf::from(home).join(".config").join("gazelle").join("workspace.json");
    }
    PathBuf::from("workspace.json")
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
}
