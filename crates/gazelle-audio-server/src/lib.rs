//! Multi-device HTTP/WebSocket control server for Antelope Audio interfaces.
//!
//! One server manages **every connected device at once** and exposes the in-scope command
//! surface over HTTP and WebSocket, so a client can present a unified view rather than one
//! panel per device. The stock software binds one control server per device, which is why
//! its panels make you switch between them.
//!
//! # Safety posture
//!
//! The default transport backend is the hardware-free loopback, and binding is loopback-only
//! unless explicitly overridden. Driving real hardware is a deliberate act. See
//! `.agent/decisions/0012-loopback-default-and-dry-run.md`.

pub mod config;
pub mod device;
pub mod error;
pub mod http;
pub mod registry_set;
pub mod value;
#[cfg(feature = "web-ui")]
pub mod web;
pub mod workspace;
pub mod ws;

use std::sync::Arc;

use crate::device::manager::DeviceManager;
use crate::workspace::store::WorkspaceStore;

/// Everything the HTTP and WebSocket layers share.
#[derive(Clone)]
pub struct AppState {
    pub devices: Arc<DeviceManager>,
    pub store: Arc<dyn WorkspaceStore>,
    /// When set, every command is non-mutating regardless of per-request options.
    pub force_dry_run: bool,
    pub backend: String,
    /// Where user theme JSON files for the web UI live; `None` lists no user themes.
    pub themes_dir: Option<std::path::PathBuf>,
}

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
