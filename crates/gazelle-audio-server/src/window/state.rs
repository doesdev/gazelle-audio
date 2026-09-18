//! Where the window was last time: its size, its position and whether it was maximised.
//!
//! Kept in the config directory beside the workspace and the themes (`%APPDATA%\gazelle`, P82), in
//! its own small file: it is the window's, not the workspace's, and a workspace carried to another
//! machine should not carry a position on a monitor that machine does not have.
//!
//! Nothing here creates a window, so all of it is tested without a desktop.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// The window's size on a first run: wide enough for the mixer's strips with the sidebar open,
/// and short enough to fit a 900-pixel-tall laptop screen with its taskbar.
pub const DEFAULT_WIDTH: u32 = 1280;
pub const DEFAULT_HEIGHT: u32 = 820;

/// The smallest the window may be. Below this the mixer's strips start dropping off the right of
/// the page rather than reflowing, and the transport bar wraps.
pub const MIN_WIDTH: u32 = 900;
pub const MIN_HEIGHT: u32 = 600;

/// Beyond this a remembered size is not a size, it is a corrupt file or a monitor that has gone.
const MAX_DIMENSION: u32 = 16_384;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowState {
    pub width: u32,
    pub height: u32,
    /// The top-left corner, when there is one to restore. Absent on a first run, so the system
    /// places the window itself, which is better than guessing at a monitor layout.
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub maximised: bool,
}

impl Default for WindowState {
    fn default() -> Self {
        WindowState { width: DEFAULT_WIDTH, height: DEFAULT_HEIGHT, x: None, y: None, maximised: false }
    }
}

impl WindowState {
    /// The state as it may be used: a size no smaller than the window's minimum and no larger than
    /// any real screen, whatever the file said.
    #[must_use]
    pub fn sanitised(self) -> Self {
        WindowState {
            width: self.width.clamp(MIN_WIDTH, MAX_DIMENSION),
            height: self.height.clamp(MIN_HEIGHT, MAX_DIMENSION),
            ..self
        }
    }

    /// Read the remembered state, or the default when there is nothing readable to read.
    ///
    /// A missing file is the first run; an unreadable or malformed one is not worth failing a
    /// launch over, and writing the window's own position back fixes it.
    pub fn load(path: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        match serde_json::from_str::<WindowState>(&text) {
            Ok(state) => state.sanitised(),
            Err(e) => {
                tracing::warn!("ignoring {}: {e}", path.display());
                Self::default()
            }
        }
    }

    /// Write it back, creating the config directory if this is the first thing to go in it.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("gazelle-window-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("window.json")
    }

    #[test]
    fn a_saved_window_comes_back_as_it_was() {
        let path = temp("roundtrip");
        let state = WindowState { width: 1600, height: 900, x: Some(-12), y: Some(40), maximised: true };
        state.save(&path).expect("the config directory is created on the way");
        assert_eq!(WindowState::load(&path), state);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_first_run_and_an_unreadable_file_both_give_the_default() {
        let path = temp("missing");
        assert_eq!(WindowState::load(&path), WindowState::default());
        assert_eq!(WindowState::default().x, None, "the system places the first window");

        let path = temp("garbage");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{not json").unwrap();
        assert_eq!(WindowState::load(&path), WindowState::default());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// A file that says the window was 40 pixels wide would open one nobody can use.
    #[test]
    fn a_remembered_size_is_kept_within_what_a_window_may_be() {
        let path = temp("clamped");
        WindowState { width: 40, height: 10, x: Some(0), y: Some(0), maximised: false }.save(&path).unwrap();
        let loaded = WindowState::load(&path);
        assert_eq!((loaded.width, loaded.height), (MIN_WIDTH, MIN_HEIGHT));
        assert_eq!(loaded.x, Some(0), "only the size is corrected");

        let huge = WindowState { width: 999_999, height: 999_999, ..WindowState::default() }.sanitised();
        assert_eq!((huge.width, huge.height), (MAX_DIMENSION, MAX_DIMENSION));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// An older file, or one written by hand, need not carry every field.
    #[test]
    fn missing_fields_fall_back_to_the_default() {
        let path = temp("partial");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{"maximised": true}"#).unwrap();
        let loaded = WindowState::load(&path);
        assert_eq!(loaded, WindowState { maximised: true, ..WindowState::default() });
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
