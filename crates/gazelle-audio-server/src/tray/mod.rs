//! The server's tray icon: open the UI, see what the server is doing, rescan devices, start it on
//! boot, open its log folder, quit.
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

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use crate::device::descriptor::DeviceDescriptor;
use crate::device::manager::DeviceManager;
use boot::BootArgs;

/// Antelope's own background service. It opens the devices exclusively while it runs, which is
/// the commonest reason nothing attaches. The tray only reads its status; the user decides
/// whether it runs.
pub const ANTELOPE_SERVICE: &str = "Antelope-Manager-Service";

/// The icon's hover title: the app's name and nothing else. Where it is listening is the menu's
/// first status line, so the title does not repeat it.
pub const TITLE: &str = "Gazelle";

/// Everything the tray needs from the server.
pub struct Context {
    /// The updater, when there is one. `None` leaves the menu without an update section.
    pub update: Option<std::sync::Arc<crate::update::Updater>>,
    /// Stop the server and start the staged binary, once the server has stopped.
    pub restart: Option<Box<dyn Fn()>>,
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
    /// Brings the desktop window to the front. `None` on a server without one, and Open then
    /// opens a browser as it always did.
    pub show_window: Option<Box<dyn Fn()>>,
    /// Asks for a device scan now. `None` where devices cannot come and go (the loopback), and
    /// the menu then has no Rescan item.
    pub rescan: Option<Box<dyn Fn()>>,
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

/// The current user's login entries, `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.
///
/// The tray uses this to read and flip Start on boot; the installer uses it to re-point an entry
/// at the installed binary. Both go through [`boot::RunKey`], so every rule about it is tested
/// against an in-memory fake and no test writes the real key.
#[cfg(windows)]
pub fn user_run_key() -> Box<dyn boot::RunKey> {
    Box::new(windows::UserRunKey)
}

#[cfg(not(windows))]
pub fn user_run_key() -> Box<dyn boot::RunKey> {
    Box::new(boot::NoRunKey)
}

/// Whether Antelope's Manager Service is running now. The one thing outside the tray that wants
/// to know is [`crate::notice`]; anything that cannot be read counts as not running.
///
/// There is no such service off Windows, so nothing there can be holding the devices this way.
#[cfg(windows)]
pub fn antelope_service_running() -> bool {
    windows::service_running(ANTELOPE_SERVICE)
}

#[cfg(not(windows))]
pub fn antelope_service_running() -> bool {
    false
}

/// A menu command.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Open,
    /// The web UI in the system browser. Only offered where Open means the desktop window.
    OpenInBrowser,
    StartOnBoot,
    Quit,
    OpenLogFolder,
    Rescan,
    CheckUpdates,
    DownloadUpdate,
    /// Stop the server and start the staged binary. The only place the app restarts itself.
    RestartToUpdate,
}

impl Command {
    /// The menu item id. Zero is what a dismissed menu returns, so ids start at 1.
    pub fn id(self) -> usize {
        match self {
            Command::Open => 1,
            Command::StartOnBoot => 2,
            Command::Quit => 3,
            Command::OpenLogFolder => 4,
            Command::Rescan => 5,
            Command::CheckUpdates => 6,
            Command::DownloadUpdate => 7,
            Command::RestartToUpdate => 8,
            Command::OpenInBrowser => 9,
        }
    }

    pub fn from_id(id: usize) -> Option<Command> {
        ALL.into_iter().find(|c| c.id() == id)
    }
}

/// Every menu command, for the id round trip and for the Windows module's dispatch.
pub const ALL: [Command; 9] = [
    Command::Open,
    Command::StartOnBoot,
    Command::Quit,
    Command::OpenLogFolder,
    Command::Rescan,
    Command::CheckUpdates,
    Command::DownloadUpdate,
    Command::RestartToUpdate,
    Command::OpenInBrowser,
];

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
    /// Whether devices can come and go, so a rescan means something (the USB backend).
    pub can_rescan: bool,
    /// What the updater has to say, or `None` when there is no updater: a non-loopback bind,
    /// where update control is deliberately not offered.
    pub update: Option<UpdateMenu>,

    /// Whether this server has a desktop window. Open then shows it, and a second item opens a
    /// browser; without one Open is the browser, as it always was.
    pub has_window: bool,
}

/// The updater as the menu shows it, read when the menu opens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateMenu {
    /// The one line of state, from `update::State::line`.
    pub line: String,
    /// A version found but not yet fetched, so there is something to download.
    pub available: Option<String>,
    /// A version verified and in place, so there is something to restart into.
    pub staged: Option<String>,
}

/// Where to point a browser. A wildcard bind listens on every interface, but a browser cannot
/// open `0.0.0.0`, so it gets the loopback address of the same family.
pub fn ui_url(address: SocketAddr) -> String {
    format!("http://{}/", crate::handover::reachable(address))
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
    ];
    if status.has_window {
        items.push(Item::Action {
            command: Command::OpenInBrowser,
            label: "Open in browser".into(),
            enabled: status.web_ui,
            checked: None,
            default: false,
        });
    }
    items.extend([
        Item::Separator,
        Item::Info(format!("Listening on {}", ui_url(status.address))),
        Item::Info(format!("Backend: {}", status.backend)),
        Item::Info(format!("Dry run: {}", if status.dry_run { "on" } else { "off" })),
    ]);
    if status.devices.is_empty() {
        items.push(Item::Info("No devices attached".into()));
    }
    items.extend(status.devices.iter().map(|d| Item::Info(format!("Device: {d}"))));
    if status.antelope_service_running {
        items.push(Item::Info("Warning: Antelope’s service is holding the devices".into()));
    }
    if status.can_rescan {
        items.push(Item::Action {
            command: Command::Rescan,
            label: "Rescan devices".into(),
            enabled: true,
            checked: None,
            default: false,
        });
    }
    if let Some(update) = &status.update {
        items.push(Item::Separator);
        items.push(Item::Info(update.line.clone()));
        items.push(Item::Action {
            command: Command::CheckUpdates,
            label: "Check for updates".into(),
            // Once something is staged there is nothing left to find until the restart.
            enabled: update.staged.is_none(),
            checked: None,
            default: false,
        });
        if let Some(version) = &update.available {
            items.push(Item::Action {
                command: Command::DownloadUpdate,
                label: format!("Download update {version}"),
                enabled: true,
                checked: None,
                default: false,
            });
        }
        if let Some(version) = &update.staged {
            items.push(Item::Action {
                command: Command::RestartToUpdate,
                label: format!("Restart to update to {version}"),
                enabled: true,
                checked: None,
                default: false,
            });
        }
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
            can_rescan: false,
            update: None,
            has_window: false,
        }
    }

    fn with_update(update: UpdateMenu) -> Status {
        Status { update: Some(update), ..status() }
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
        assert_eq!(infos(&menu(&s)).last(), Some(&"Warning: Antelope’s service is holding the devices"));
    }

    #[test]
    fn start_on_boot_is_checked_as_the_entry_is() {
        for on in [false, true] {
            let items = menu(&Status { start_on_boot: on, ..status() });
            assert!(matches!(action(&items, Command::StartOnBoot), Item::Action { checked: Some(c), enabled: true, .. } if *c == on));
        }
    }

    #[test]
    fn rescan_follows_the_device_lines_where_devices_can_come_and_go() {
        let items = menu(&Status { backend: "usb".into(), can_rescan: true, antelope_service_running: true, ..status() });
        let commands: Vec<Command> =
            items.iter().filter_map(|i| if let Item::Action { command, .. } = i { Some(*command) } else { None }).collect();
        assert_eq!(commands, [Command::Open, Command::Rescan, Command::StartOnBoot, Command::OpenLogFolder, Command::Quit]);
        assert_eq!(
            action(&items, Command::Rescan),
            &Item::Action { command: Command::Rescan, label: "Rescan devices".into(), enabled: true, checked: None, default: false }
        );
        let at = items.iter().position(|i| i == action(&items, Command::Rescan)).unwrap();
        assert_eq!(items[at - 1], Item::Info("Warning: Antelope’s service is holding the devices".into()));
        assert_eq!(items[at + 1], Item::Separator);

        // The loopback's devices never change, so there is nothing to offer.
        let items = menu(&status());
        assert!(!items.iter().any(|i| matches!(i, Item::Action { command: Command::Rescan, .. })));
    }

    /// With a desktop window, Open shows it and a second item still opens a browser: the app is
    /// served over HTTP either way, and a phone or a second machine is the point of that.
    #[test]
    fn a_window_adds_open_in_browser_beside_open() {
        let items = menu(&Status { has_window: true, ..status() });
        let commands: Vec<Command> =
            items.iter().filter_map(|i| if let Item::Action { command, .. } = i { Some(*command) } else { None }).collect();
        assert_eq!(commands, [Command::Open, Command::OpenInBrowser, Command::StartOnBoot, Command::OpenLogFolder, Command::Quit]);
        assert_eq!(
            action(&items, Command::OpenInBrowser),
            &Item::Action { command: Command::OpenInBrowser, label: "Open in browser".into(), enabled: true, checked: None, default: false }
        );
        assert!(matches!(action(&items, Command::Open), Item::Action { default: true, label, .. } if label == "Open Gazelle"));

        // Without a window there is only one way to open it, and the menu does not grow an item
        // that would do the same thing twice.
        assert!(!menu(&status()).iter().any(|i| matches!(i, Item::Action { command: Command::OpenInBrowser, .. })));

        // Nothing to open at all: both are dead, as Open already was.
        let items = menu(&Status { has_window: true, web_ui: false, ..status() });
        assert!(matches!(action(&items, Command::OpenInBrowser), Item::Action { enabled: false, .. }));
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
    fn without_an_updater_the_menu_says_nothing_about_updates() {
        let items = menu(&status());
        assert!(!items.iter().any(|i| matches!(i, Item::Action { command, .. }
            if matches!(command, Command::CheckUpdates | Command::DownloadUpdate | Command::RestartToUpdate))));
        assert!(!infos(&items).iter().any(|l| l.contains("date") || l.contains("Update")));
    }

    #[test]
    fn a_checked_updater_offers_a_check_and_says_where_it_got_to() {
        let items = menu(&with_update(UpdateMenu {
            line: "Updates: this is the newest version".into(),
            available: None,
            staged: None,
        }));
        assert_eq!(infos(&items).last(), Some(&"Updates: this is the newest version"));
        assert_eq!(
            action(&items, Command::CheckUpdates),
            &Item::Action { command: Command::CheckUpdates, label: "Check for updates".into(), enabled: true, checked: None, default: false }
        );
        assert!(!items.iter().any(|i| matches!(i, Item::Action { command: Command::DownloadUpdate, .. })));
        assert!(!items.iter().any(|i| matches!(i, Item::Action { command: Command::RestartToUpdate, .. })));
    }

    /// Nothing is fetched unasked, so a found update is offered as a thing to download.
    #[test]
    fn a_found_update_is_offered_as_a_download() {
        let items = menu(&with_update(UpdateMenu {
            line: "Update available: 0.2.0".into(),
            available: Some("0.2.0".into()),
            staged: None,
        }));
        assert_eq!(
            action(&items, Command::DownloadUpdate),
            &Item::Action { command: Command::DownloadUpdate, label: "Download update 0.2.0".into(), enabled: true, checked: None, default: false }
        );
        assert!(matches!(action(&items, Command::CheckUpdates), Item::Action { enabled: true, .. }));
        assert!(!items.iter().any(|i| matches!(i, Item::Action { command: Command::RestartToUpdate, .. })));
    }

    /// A staged update is the only thing the app will restart itself for, and only when asked.
    #[test]
    fn a_staged_update_offers_a_restart_and_stops_offering_a_check() {
        let items = menu(&with_update(UpdateMenu {
            line: "Update 0.2.0 is ready (restart to use it)".into(),
            available: None,
            staged: Some("0.2.0".into()),
        }));
        assert_eq!(
            action(&items, Command::RestartToUpdate),
            &Item::Action { command: Command::RestartToUpdate, label: "Restart to update to 0.2.0".into(), enabled: true, checked: None, default: false }
        );
        assert!(matches!(action(&items, Command::CheckUpdates), Item::Action { enabled: false, .. }));
        assert_eq!(infos(&items).last(), Some(&"Update 0.2.0 is ready (restart to use it)"));
    }

    #[test]
    fn the_update_section_sits_between_the_devices_and_start_on_boot() {
        let items = menu(&with_update(UpdateMenu { line: "Updates: not checked yet".into(), available: Some("0.2.0".into()), staged: None }));
        let commands: Vec<Command> =
            items.iter().filter_map(|i| if let Item::Action { command, .. } = i { Some(*command) } else { None }).collect();
        assert_eq!(
            commands,
            [Command::Open, Command::CheckUpdates, Command::DownloadUpdate, Command::StartOnBoot, Command::OpenLogFolder, Command::Quit]
        );
        assert_eq!(items.last(), Some(action(&items, Command::Quit)));
        // Opening the UI stays the bold default; nothing about updates takes a click of the icon.
        assert!(!items.iter().any(|i| matches!(i, Item::Action { default: true, command, .. } if *command != Command::Open)));
    }

    /// A tray menu is one narrow column, read at a glance. Nothing in it may run long, and
    /// nothing may leak a path, a URL beyond the one address the user needs, or an error's own
    /// text (P-entry `update-messages`).
    #[test]
    fn no_menu_line_runs_long() {
        let states = [
            crate::update::State::Unknown,
            crate::update::State::Checking,
            crate::update::State::UpToDate,
            crate::update::State::Available { version: "0.2.0".into(), page: "https://example.invalid/r".into() },
            crate::update::State::Downloading { version: "0.2.0".into() },
            crate::update::State::Staged { version: "0.2.0".into() },
        ];
        let mut menus: Vec<Status> = vec![
            status(),
            Status { backend: "usb".into(), dry_run: true, devices: vec![], can_rescan: true, antelope_service_running: true, ..status() },
            Status { web_ui: false, log_file: false, has_window: true, ..status() },
        ];
        for state in states {
            menus.push(with_update(UpdateMenu { line: state.line(), available: Some("0.2.0".into()), staged: Some("0.2.0".into()) }));
        }
        for summary in crate::update::SUMMARIES {
            let state = crate::update::State::Failed {
                message: summary.as_str().into(),
                detail: "https://example.invalid/releases: status 503".into(),
            };
            menus.push(with_update(UpdateMenu { line: state.line(), available: None, staged: None }));
        }
        for status in &menus {
            for item in menu(status) {
                let line = match &item {
                    Item::Action { label, .. } => label.clone(),
                    Item::Info(line) => line.clone(),
                    Item::Separator => continue,
                };
                let length = line.chars().count();
                assert!(length <= crate::update::LINE_LIMIT, "{length} characters is too long for a menu line: {line:?}");
                assert!(!line.contains("://") || line.starts_with("Listening on "), "only the address the user needs is a URL: {line:?}");
            }
        }
    }

    #[test]
    fn command_ids_round_trip_and_zero_is_no_command() {
        for c in ALL {
            assert_eq!(Command::from_id(c.id()), Some(c));
        }
        assert_eq!(Command::from_id(0), None);
    }
}
