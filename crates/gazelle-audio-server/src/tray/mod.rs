//! The server's tray icon: open the UI, see what the server is doing, start it on boot, open its
//! log folder, quit.
//!
//! What the menu holds is decided here, as plain data, so it is tested without a desktop. The
//! Windows module only draws it. The menu is rebuilt each time it opens, so the status lines
//! (devices attached, whether Antelope's service is running, the boot entry) are read then rather
//! than kept up to date in the background.
//!
//! Other platforms have no tray yet; [`start`] says so and the server runs headless.

pub mod boot;
#[cfg(windows)]
mod windows;

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;

use crate::device::descriptor::DeviceDescriptor;
use crate::device::manager::DeviceManager;
use boot::BootArgs;

/// Antelope's own background service. It opens the devices exclusively while it runs, which is
/// the commonest reason nothing attaches. The tray only reads its status; the user decides
/// whether it runs.
pub const ANTELOPE_SERVICE: &str = "Antelope-Manager-Service";

/// Everything the tray needs from the server.
pub struct Context {
    /// The address the listener really bound.
    pub address: SocketAddr,
    pub backend: String,
    pub dry_run: bool,
    /// Whether the web UI is served at `/`; without it there is nothing to open.
    pub web_ui: bool,
    pub devices: Arc<DeviceManager>,
    /// This binary, for the boot entry.
    pub exe: PathBuf,
    pub boot_args: BootArgs,
    /// The folder the log file is written to, when there is one.
    pub log_dir: Option<PathBuf>,
    /// Asks the server to stop. The tray removes itself straight after.
    pub quit: Box<dyn Fn()>,
}

/// The tray, created but not yet running. [`Tray::run`] pumps its messages on this thread until
/// Quit is picked or a [`Closer`] closes it.
#[cfg(windows)]
pub use self::windows::{Closer, Tray};

/// Create the tray icon on this thread. An error means no tray (no desktop session, or the shell
/// refused the icon); the caller carries on headless.
#[cfg(windows)]
pub fn start(context: Context) -> Result<Tray, String> {
    windows::start(context)
}

#[cfg(not(windows))]
pub enum Tray {}

#[cfg(not(windows))]
#[derive(Clone)]
pub enum Closer {}

#[cfg(not(windows))]
impl Tray {
    pub fn closer(&self) -> Closer {
        match *self {}
    }
    pub fn run(self) {
        match self {}
    }
}

#[cfg(not(windows))]
impl Closer {
    pub fn close(&self) {
        match *self {}
    }
}

#[cfg(not(windows))]
pub fn start(_context: Context) -> Result<Tray, String> {
    Err("the tray icon is only built for Windows so far".into())
}

/// A menu command.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Open,
    StartOnBoot,
    Quit,
    OpenLogFolder,
}

impl Command {
    /// The menu item id. Zero is what a dismissed menu returns, so ids start at 1.
    pub fn id(self) -> usize {
        match self {
            Command::Open => 1,
            Command::StartOnBoot => 2,
            Command::Quit => 3,
            Command::OpenLogFolder => 4,
        }
    }

    pub fn from_id(id: usize) -> Option<Command> {
        [Command::Open, Command::StartOnBoot, Command::Quit, Command::OpenLogFolder].into_iter().find(|c| c.id() == id)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    /// Something to click. `checked` is `Some` for a checkable item. `default` is drawn bold and
    /// is what clicking the icon itself does.
    Action { command: Command, label: String, enabled: bool, checked: Option<bool>, default: bool },
    /// A line to read, not click.
    Info(String),
    Separator,
}

/// The server's state as the menu shows it, read when the menu opens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    pub address: SocketAddr,
    pub backend: String,
    pub dry_run: bool,
    pub web_ui: bool,
    /// One label per attached device, in the manager's order.
    pub devices: Vec<String>,
    pub antelope_service_running: bool,
    pub start_on_boot: bool,
    /// Whether a log file is being written, so there is a folder to open.
    pub log_file: bool,
}

/// Where to point a browser. A wildcard bind listens on every interface, but a browser cannot
/// open `0.0.0.0`, so it gets the loopback address of the same family.
pub fn ui_url(address: SocketAddr) -> String {
    let ip = match address.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(ip) if ip.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
        ip => ip,
    };
    format!("http://{}/", SocketAddr::new(ip, address.port()))
}

/// A device as the menu names it: its model, or its USB ids when the model is unknown.
pub fn device_label(descriptor: &DeviceDescriptor) -> String {
    match &descriptor.model {
        Some(model) => model.clone(),
        None => format!("Unknown device {:04x}:{:04x}", descriptor.vid, descriptor.pid),
    }
}

/// The menu, top to bottom.
pub fn menu(status: &Status) -> Vec<Item> {
    let mut items = vec![
        Item::Action {
            command: Command::Open,
            label: if status.web_ui { "Open Gazelle".into() } else { "Open Gazelle (web UI not served)".into() },
            enabled: status.web_ui,
            checked: None,
            default: status.web_ui,
        },
        Item::Separator,
        Item::Info(format!("Listening on {}", ui_url(status.address))),
        Item::Info(format!("Backend: {}", status.backend)),
        Item::Info(format!("Dry run: {}", if status.dry_run { "on" } else { "off" })),
    ];
    if status.devices.is_empty() {
        items.push(Item::Info("No devices attached".into()));
    }
    items.extend(status.devices.iter().map(|d| Item::Info(format!("Device: {d}"))));
    if status.antelope_service_running {
        items.push(Item::Info("Warning: Antelope Manager Service is running and holds the devices".into()));
    }
    items.extend([
        Item::Separator,
        Item::Action {
            command: Command::StartOnBoot,
            label: "Start on boot".into(),
            enabled: true,
            checked: Some(status.start_on_boot),
            default: false,
        },
        Item::Action {
            command: Command::OpenLogFolder,
            label: if status.log_file { "Open log folder".into() } else { "Open log folder (no log file)".into() },
            enabled: status.log_file,
            checked: None,
            default: false,
        },
        Item::Separator,
        Item::Action { command: Command::Quit, label: "Quit".into(), enabled: true, checked: None, default: false },
    ]);
    items
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::descriptor::DeviceId;

    fn status() -> Status {
        Status {
            address: "127.0.0.1:8420".parse().unwrap(),
            backend: "loopback".into(),
            dry_run: false,
            web_ui: true,
            devices: vec!["Zen Quadro".into(), "Zen Studio+".into()],
            antelope_service_running: false,
            start_on_boot: false,
            log_file: true,
        }
    }

    fn infos(items: &[Item]) -> Vec<&str> {
        items.iter().filter_map(|i| if let Item::Info(s) = i { Some(s.as_str()) } else { None }).collect()
    }

    fn action(items: &[Item], command: Command) -> &Item {
        items.iter().find(|i| matches!(i, Item::Action { command: c, .. } if *c == command)).unwrap()
    }

    #[test]
    fn the_browser_opens_a_loopback_address_for_a_wildcard_bind() {
        assert_eq!(ui_url("127.0.0.1:8420".parse().unwrap()), "http://127.0.0.1:8420/");
        assert_eq!(ui_url("192.168.1.5:80".parse().unwrap()), "http://192.168.1.5:80/");
        assert_eq!(ui_url("0.0.0.0:8420".parse().unwrap()), "http://127.0.0.1:8420/");
        assert_eq!(ui_url("[::]:8420".parse().unwrap()), "http://[::1]:8420/");
        assert_eq!(ui_url("[::1]:9".parse().unwrap()), "http://[::1]:9/");
    }

    #[test]
    fn a_device_is_named_by_model_or_by_its_ids() {
        let mut d = DeviceDescriptor {
            id: DeviceId::loopback(0),
            vid: 0x23e5,
            pid: 0xa2f9,
            slug: None,
            model: Some("Zen Quadro".into()),
            family: None,
            command_count: None,
            identity_stable: true,
            backend: "loopback".into(),
            max_packet_size: 64,
        };
        assert_eq!(device_label(&d), "Zen Quadro");
        d.model = None;
        assert_eq!(device_label(&d), "Unknown device 23e5:a2f9");
    }

    #[test]
    fn the_menu_opens_then_reports_then_offers_boot_the_log_and_quit() {
        let items = menu(&status());
        assert_eq!(
            items.first(),
            Some(&Item::Action { command: Command::Open, label: "Open Gazelle".into(), enabled: true, checked: None, default: true })
        );
        assert_eq!(
            infos(&items),
            ["Listening on http://127.0.0.1:8420/", "Backend: loopback", "Dry run: off", "Device: Zen Quadro", "Device: Zen Studio+"]
        );
        let commands: Vec<Command> =
            items.iter().filter_map(|i| if let Item::Action { command, .. } = i { Some(*command) } else { None }).collect();
        assert_eq!(commands, [Command::Open, Command::StartOnBoot, Command::OpenLogFolder, Command::Quit]);
        assert_eq!(items.last(), Some(action(&items, Command::Quit)));
        assert!(!items.iter().any(|i| matches!(i, Item::Action { default: true, command, .. } if *command != Command::Open)));
    }

    #[test]
    fn status_lines_follow_the_server() {
        let s = Status { backend: "usb".into(), dry_run: true, devices: vec![], ..status() };
        assert_eq!(infos(&menu(&s))[1..], ["Backend: usb", "Dry run: on", "No devices attached"]);
    }

    #[test]
    fn a_running_antelope_service_is_warned_about() {
        assert!(!infos(&menu(&status())).iter().any(|l| l.contains("Antelope")));
        let s = Status { antelope_service_running: true, ..status() };
        assert_eq!(infos(&menu(&s)).last(), Some(&"Warning: Antelope Manager Service is running and holds the devices"));
    }

    #[test]
    fn start_on_boot_is_checked_as_the_entry_is() {
        for on in [false, true] {
            let items = menu(&Status { start_on_boot: on, ..status() });
            assert!(matches!(action(&items, Command::StartOnBoot), Item::Action { checked: Some(c), enabled: true, .. } if *c == on));
        }
    }

    #[test]
    fn without_the_web_ui_there_is_nothing_to_open() {
        let items = menu(&Status { web_ui: false, ..status() });
        assert!(matches!(action(&items, Command::Open), Item::Action { enabled: false, default: false, .. }));
    }

    #[test]
    fn the_log_folder_opens_only_while_a_log_file_is_written() {
        let items = menu(&status());
        assert_eq!(
            action(&items, Command::OpenLogFolder),
            &Item::Action { command: Command::OpenLogFolder, label: "Open log folder".into(), enabled: true, checked: None, default: false }
        );
        let items = menu(&Status { log_file: false, ..status() });
        assert!(matches!(action(&items, Command::OpenLogFolder), Item::Action { enabled: false, label, .. } if label == "Open log folder (no log file)"));
    }

    #[test]
    fn command_ids_round_trip_and_zero_is_no_command() {
        for c in [Command::Open, Command::StartOnBoot, Command::Quit, Command::OpenLogFolder] {
            assert_eq!(Command::from_id(c.id()), Some(c));
        }
        assert_eq!(Command::from_id(0), None);
    }
}
