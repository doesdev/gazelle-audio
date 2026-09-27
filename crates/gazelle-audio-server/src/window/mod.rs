//! The desktop windows: the same web UI the server already serves, in windows of their own.
//!
//! Nothing here is a second implementation of the app. Each window is a system webview (`WebView2`
//! on Windows) pointed at `http://127.0.0.1:<port>`, so a phone or a second machine sees exactly what
//! the desktop does. That is the reason for windows over a native shell.
//!
//! There are three, all on one event loop on a thread of their own ([`state::Kind`]):
//!
//! - **the app**, over the whole web UI;
//! - **the recording widget**, a small frameless window that stays on top of everything, over the
//!   `#/widget` route: the recorder's state, time and buttons, wherever the person leaves it;
//! - **the recording hub**, a full-screen window over the `#/hub` route, to be read from across the
//!   room.
//!
//! It is behind the `window` Cargo feature, off by default: the headless server, and every test
//! harness that runs it with `--no-tray`, is unchanged by this module's existence. Only [`state`],
//! [`place`] and the [`Viewports`] seam are compiled without the feature, since where a window was is
//! worth remembering whether or not this build can open one, and the routes that open the widget and
//! the hub answer either way.

pub mod place;
pub mod state;

#[cfg(feature = "window")]
mod desktop;

#[cfg(feature = "window")]
pub use desktop::{open, Options, Window};

pub use state::{Kind, WindowState};

/// The window's title, in the title bar and the taskbar.
pub const TITLE: &str = "Gazelle";

static SHUTTING_DOWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Say that the server is stopping. A window closed by Windows from now on (`taskkill` asks every
/// window to close, and so does signing out) is closing because Gazelle is, and the widget's being
/// open is not forgotten for it.
pub fn note_shutdown() {
    SHUTTING_DOWN.store(true, std::sync::atomic::Ordering::Release);
}

/// Whether [`note_shutdown`] has been called.
pub fn shutting_down() -> bool {
    SHUTTING_DOWN.load(std::sync::atomic::Ordering::Acquire)
}

/// Whether the recording widget and hub are open, as the routes and the tray report it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct ViewportsState {
    pub widget: bool,
    pub hub: bool,
    /// The hub fills its monitor, rather than being an ordinary window.
    pub hub_full_screen: bool,
}

/// The recording widget and hub, from any thread: the routes, the tray and the start-up.
pub trait Viewports: Send + Sync {
    fn state(&self) -> ViewportsState;
    /// Open or close the widget.
    fn set_widget(&self, open: bool);
    /// Open the hub, full screen on the monitor it was last on, or close it.
    fn set_hub(&self, open: bool);
    /// Put the open hub into full screen, or out of it.
    fn set_hub_full_screen(&self, full: bool);
}
