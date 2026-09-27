//! Where each window was last time: its size, its position, and what else it needs to come back
//! as it was.
//!
//! There are three windows ([`Kind`]): the app itself, the recording widget and the recording hub.
//! Each is remembered in a small file of its own in the config directory beside the workspace and
//! the themes (`%APPDATA%\gazelle`): it is the window's, not the workspace's, and a workspace carried
//! to another machine should not carry a position on a monitor that machine does not have. The
//! main window's file is `window.json`, as it always was.
//!
//! Nothing here creates a window, so all of it is tested without a desktop.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The main window's size on a first run: wide enough for the mixer's strips with the sidebar open,
/// and short enough to fit a 900-pixel-tall laptop screen with its taskbar.
pub const DEFAULT_WIDTH: u32 = 1280;
pub const DEFAULT_HEIGHT: u32 = 820;

/// The smallest the main window may be. Below this the mixer's strips start dropping off the right
/// of the page rather than reflowing, and the transport bar wraps.
pub const MIN_WIDTH: u32 = 900;
pub const MIN_HEIGHT: u32 = 600;

/// The recording widget's size on a first run, in logical pixels: the state and the time on one
/// line, the preset and the pre-roll on the next, the buttons, and a warning line.
pub const WIDGET_WIDTH: u32 = 320;
pub const WIDGET_HEIGHT: u32 = 140;
/// The smallest it may be made: below this the buttons no longer fit on their line.
pub const WIDGET_MIN_WIDTH: u32 = 260;
pub const WIDGET_MIN_HEIGHT: u32 = 120;

/// The recording hub's size when it leaves full screen.
pub const HUB_WIDTH: u32 = 1280;
pub const HUB_HEIGHT: u32 = 800;
pub const HUB_MIN_WIDTH: u32 = 640;
pub const HUB_MIN_HEIGHT: u32 = 400;

/// Beyond this a remembered size is not a size, it is a corrupt file or a monitor that has gone.
const MAX_DIMENSION: u32 = 16_384;

/// Which window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    /// The app, over the whole web UI.
    Main,
    /// The small, frameless, always-on-top recording widget (`#/widget`).
    Widget,
    /// The full-screen recording hub (`#/hub`).
    Hub,
}

impl Kind {
    /// The size a window of this kind starts at.
    pub fn default_size(self) -> (u32, u32) {
        match self {
            Kind::Main => (DEFAULT_WIDTH, DEFAULT_HEIGHT),
            Kind::Widget => (WIDGET_WIDTH, WIDGET_HEIGHT),
            Kind::Hub => (HUB_WIDTH, HUB_HEIGHT),
        }
    }

    /// The smallest a window of this kind may be.
    pub fn min_size(self) -> (u32, u32) {
        match self {
            Kind::Main => (MIN_WIDTH, MIN_HEIGHT),
            Kind::Widget => (WIDGET_MIN_WIDTH, WIDGET_MIN_HEIGHT),
            Kind::Hub => (HUB_MIN_WIDTH, HUB_MIN_HEIGHT),
        }
    }

    /// The file it is remembered in, in the config directory.
    pub fn file_name(self) -> &'static str {
        match self {
            Kind::Main => "window.json",
            Kind::Widget => "recording-widget.json",
            Kind::Hub => "recording-hub.json",
        }
    }

    /// The route of the web app it shows, after the server's address.
    pub fn route(self) -> &'static str {
        match self {
            Kind::Main => "",
            Kind::Widget => "#/widget",
            Kind::Hub => "#/hub",
        }
    }

    /// Its title, in the taskbar and Alt-Tab.
    pub fn title(self) -> &'static str {
        match self {
            Kind::Main => super::TITLE,
            Kind::Widget => "Gazelle recording",
            Kind::Hub => "Gazelle recording hub",
        }
    }
}

/// Where the window of `kind` is remembered.
pub fn state_path(kind: Kind, var: impl Fn(&str) -> Option<String>) -> PathBuf {
    crate::config::config_dir(var).map_or_else(|| PathBuf::from(kind.file_name()), |dir| dir.join(kind.file_name()))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowState {
    /// The inner size, in logical pixels, so it means the same at any scale.
    pub width: u32,
    pub height: u32,
    /// The top-left corner, in physical pixels, when there is one to restore. Absent on a first run,
    /// so the system places the window itself, which is better than guessing at a monitor layout.
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub maximised: bool,
    /// Whether it was open when Gazelle last stopped: the widget comes back if it was.
    #[serde(skip_serializing_if = "is_false")]
    pub open: bool,
    /// The monitor it was on, by the name Windows gives it: the hub opens there again.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub monitor: Option<String>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

impl Default for WindowState {
    fn default() -> Self {
        WindowState::first_run(Kind::Main)
    }
}

impl WindowState {
    /// What a window of `kind` starts as, with nothing remembered.
    pub fn first_run(kind: Kind) -> Self {
        let (width, height) = kind.default_size();
        WindowState { width, height, x: None, y: None, maximised: false, open: false, monitor: None }
    }

    /// The state as it may be used for a window of `kind`: a size no smaller than its minimum and no
    /// larger than any real screen, whatever the file said.
    #[must_use]
    pub fn sanitised(self, kind: Kind) -> Self {
        let (min_width, min_height) = kind.min_size();
        WindowState {
            width: self.width.clamp(min_width, MAX_DIMENSION),
            height: self.height.clamp(min_height, MAX_DIMENSION),
            ..self
        }
    }

    /// Read the remembered state of a window of `kind`, or its first-run state when there is nothing
    /// readable to read.
    ///
    /// A missing file is the first run; an unreadable or malformed one is not worth failing a
    /// launch over, and writing the window's own position back fixes it.
    pub fn load(path: &Path, kind: Kind) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::first_run(kind);
        };
        let mut value: serde_json::Value = match serde_json::from_str(&text) {
            Ok(value) => value,
            Err(e) => {
                tracing::warn!("ignoring {}: {e}", path.display());
                return Self::first_run(kind);
            }
        };
        // A field the file leaves out is this kind's own first-run value, not the main window's.
        if let (Some(object), serde_json::Value::Object(first)) = (value.as_object_mut(), serde_json::to_value(Self::first_run(kind)).unwrap_or_default()) {
            for (key, default) in first {
                object.entry(key).or_insert(default);
            }
        }
        match serde_json::from_value::<WindowState>(value) {
            Ok(state) => state.sanitised(kind),
            Err(e) => {
                tracing::warn!("ignoring {}: {e}", path.display());
                Self::first_run(kind)
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
        let state = WindowState { width: 1600, height: 900, x: Some(-12), y: Some(40), maximised: true, ..WindowState::default() };
        state.save(&path).expect("the config directory is created on the way");
        assert_eq!(WindowState::load(&path, Kind::Main), state);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_first_run_and_an_unreadable_file_both_give_the_default() {
        let path = temp("missing");
        assert_eq!(WindowState::load(&path, Kind::Main), WindowState::default());
        assert_eq!(WindowState::default().x, None, "the system places the first window");

        let path = temp("garbage");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{not json").unwrap();
        assert_eq!(WindowState::load(&path, Kind::Widget), WindowState::first_run(Kind::Widget));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// A file that says the window was 40 pixels wide would open one nobody can use.
    #[test]
    fn a_remembered_size_is_kept_within_what_a_window_may_be() {
        let path = temp("clamped");
        WindowState { width: 40, height: 10, x: Some(0), y: Some(0), ..WindowState::default() }.save(&path).unwrap();
        let loaded = WindowState::load(&path, Kind::Main);
        assert_eq!((loaded.width, loaded.height), (MIN_WIDTH, MIN_HEIGHT));
        assert_eq!(loaded.x, Some(0), "only the size is corrected");
        let widget = WindowState::load(&path, Kind::Widget);
        assert_eq!((widget.width, widget.height), (WIDGET_MIN_WIDTH, WIDGET_MIN_HEIGHT), "each kind has its own smallest");

        let huge = WindowState { width: 999_999, height: 999_999, ..WindowState::default() }.sanitised(Kind::Main);
        assert_eq!((huge.width, huge.height), (MAX_DIMENSION, MAX_DIMENSION));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// An older file, or one written by hand, need not carry every field; what it leaves out is the
    /// first-run value of the window it is for.
    #[test]
    fn missing_fields_fall_back_to_the_kinds_own_default() {
        let path = temp("partial");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{"maximised": true}"#).unwrap();
        assert_eq!(WindowState::load(&path, Kind::Main), WindowState { maximised: true, ..WindowState::default() });
        std::fs::write(&path, r#"{"open": true, "x": 10}"#).unwrap();
        let widget = WindowState::load(&path, Kind::Widget);
        assert_eq!((widget.width, widget.height, widget.open, widget.x), (WIDGET_WIDTH, WIDGET_HEIGHT, true, Some(10)));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// The widget's open state and the hub's monitor go in their files; the main window's file is
    /// what it always was.
    #[test]
    fn open_and_the_monitor_are_written_only_when_they_say_something() {
        let main = serde_json::to_value(WindowState::default()).unwrap();
        assert!(main.get("open").is_none() && main.get("monitor").is_none(), "{main}");
        let path = temp("widget-open");
        let widget = WindowState { open: true, x: Some(1500), y: Some(20), ..WindowState::first_run(Kind::Widget) };
        widget.save(&path).unwrap();
        assert_eq!(WindowState::load(&path, Kind::Widget), widget);
        let hub = WindowState { monitor: Some(r"\\.\DISPLAY2".into()), x: Some(1920), y: Some(0), ..WindowState::first_run(Kind::Hub) };
        hub.save(&path).unwrap();
        assert_eq!(WindowState::load(&path, Kind::Hub), hub);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn each_window_has_a_file_of_its_own_beside_the_workspace() {
        let appdata = r"C:\Users\u\AppData\Roaming";
        let env = |k: &str| (k == "APPDATA").then(|| appdata.to_string());
        let dir = PathBuf::from(appdata).join("gazelle");
        assert_eq!(state_path(Kind::Main, env), dir.join("window.json"));
        assert_eq!(state_path(Kind::Main, env), crate::config::default_window_state_path(env), "the main window's file has not moved");
        assert_eq!(state_path(Kind::Widget, env), dir.join("recording-widget.json"));
        assert_eq!(state_path(Kind::Hub, env), dir.join("recording-hub.json"));
        assert_eq!(Kind::Widget.route(), "#/widget");
        assert_eq!(Kind::Hub.route(), "#/hub");
    }
}
