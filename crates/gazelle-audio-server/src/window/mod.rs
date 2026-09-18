//! The desktop window: the same web UI the server already serves, in a window of its own.
//!
//! Nothing here is a second implementation of the app. The window is a system webview
//! (`WebView2` on Windows) pointed at `http://127.0.0.1:<port>`, so a phone or a second machine
//! sees exactly what the desktop does. That is the reason the shipping spec chose a window over
//! a native shell (`specs/2026-09-18-shipping-portable.md`).
//!
//! It is behind the `window` Cargo feature, off by default: the headless server, and every test
//! harness that runs it with `--no-tray`, is unchanged by this module's existence. Only
//! [`state`] is compiled without the feature, since where the window was is worth remembering
//! whether or not this build can open one.

pub mod state;

#[cfg(feature = "window")]
mod desktop;

#[cfg(feature = "window")]
pub use desktop::{open, Window};

pub use state::WindowState;

/// The window's title, in the title bar and the taskbar.
pub const TITLE: &str = "Gazelle";
