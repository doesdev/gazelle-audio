//! Gazelle control server.
//!
//! Safe by default: the transport backend is the hardware-free loopback and the listener
//! binds to localhost unless told otherwise. Driving real hardware is a deliberate act.

use clap::{Parser, ValueEnum};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use gazelle_audio_server::config::{default_log_dir, default_snapshots_dir, default_themes_dir, default_workspace_path};
use gazelle_audio_server::device::hotplug::{self, HotPlug, Scanner};
use gazelle_audio_server::device::manager::DeviceManager;
use gazelle_audio_server::device::usb;
use gazelle_audio_server::registry_set::RegistrySet;
use gazelle_audio_server::snapshot::store::{JsonDirStore, MemorySnapshotStore, SnapshotStore};
use gazelle_audio_server::workspace::store::{JsonFileStore, MemoryStore, WorkspaceStore};
use gazelle_audio_server::tray::{self, boot::BootArgs};
use gazelle_audio_server::update::{self, settings::Settings, Updater};
use gazelle_audio_server::{http, logging, AppState};
use tokio::sync::Notify;

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum Backend {
    /// Hardware-free emulator. The default.
    Loopback,
    /// Real USB devices, over the OS HID stack.
    Usb,
}

#[derive(Parser, Debug)]
#[command(name = "gazelle-audio-server", version, about = "Gazelle control server for Antelope Audio interfaces")]
struct Args {
    /// Address to bind. Defaults to localhost; override only deliberately.
    #[arg(long, default_value = gazelle_audio_server::config::DEFAULT_BIND)]
    bind: SocketAddr,

    /// Transport backend.
    #[arg(long, value_enum, default_value_t = Backend::Loopback)]
    backend: Backend,

    /// Never write to a device: report the bytes each command would send.
    #[arg(long)]
    dry_run: bool,

    /// Allow the recall route to apply a snapshot to a device. Off by default, and off is not the
    /// whole guard: the request must ask as well, and applying is not built yet — it waits for the
    /// hardware session in the workspace spec's §6.
    #[arg(long)]
    enable_recall: bool,

    /// Where workspace state is stored. Defaults to a platform config path.
    #[arg(long)]
    workspace: Option<PathBuf>,

    /// Where snapshots are stored, one JSON file each. Defaults to a "snapshots" folder beside
    /// the workspace file.
    #[arg(long)]
    snapshots_dir: Option<PathBuf>,

    /// Keep workspace state and snapshots in memory only.
    #[arg(long)]
    no_persist: bool,

    /// Which loopback devices to create, by model.
    #[arg(long, value_delimiter = ',', default_values_t = ["quadro".to_string(), "studio".to_string()])]
    loopback_models: Vec<String>,

    /// Directory of user theme JSON files for the web UI. Defaults to `themes` in the config
    /// directory, beside the workspace file.
    #[arg(long)]
    themes_dir: Option<PathBuf>,

    /// Make each loopback device also push its cyclic reports every MS milliseconds, with a
    /// moving test pattern, so clients see state and meter traffic without hardware.
    #[arg(long, value_name = "MS")]
    loopback_cyclic_ms: Option<u64>,

    /// Serve only the API, not the embedded web UI.
    #[arg(long)]
    no_web_ui: bool,

    /// Run without the tray icon, as test harnesses and services do. A server that cannot
    /// create one (no desktop session) also runs without it, and says so.
    #[arg(long)]
    no_tray: bool,

    /// Never look for, or offer, an update. The update settings in the config directory
    /// (`update.json`) decide the rest: the channel, and whether checks happen unasked.
    #[arg(long)]
    no_update: bool,

    /// Run without the desktop window, serving the UI over HTTP only. A build without the
    /// `window` feature, and any `--no-tray` run, has no window either way.
    #[arg(long)]
    no_window: bool,

    /// Also log to a size-capped file in DIR. Tray runs do by default, in the platform's state
    /// folder (on Windows `%LOCALAPPDATA%\gazelle\logs`); `--no-tray` runs only when given this.
    #[arg(long, value_name = "DIR")]
    log_dir: Option<PathBuf>,

    /// Copy this binary into `%LOCALAPPDATA%\Programs\Gazelle`, add a Start Menu shortcut and an
    /// Add/Remove Programs entry, and stop. Per-user: no administrator, no MSI. Run again over an
    /// existing install to upgrade it in place. Does not start the server.
    #[arg(long, conflicts_with = "uninstall")]
    install: bool,

    /// With `--install`: start the installed copy afterwards. Without either flag, a run in a
    /// terminal asks and one started from Explorer does not.
    #[arg(long, requires = "install")]
    start: bool,

    /// With `--install`: do not start the installed copy, and do not ask.
    #[arg(long, requires = "install", conflicts_with = "start")]
    no_start: bool,

    /// Remove the installed copy, its shortcut and its Add/Remove Programs entry, and stop. Your
    /// settings and layouts are kept unless `--purge`.
    #[arg(long)]
    uninstall: bool,

    /// With `--uninstall`: also remove the configuration (`%APPDATA%\gazelle`) and the logs
    /// (`%LOCALAPPDATA%\gazelle`). Nothing outside those and the install folder is ever touched.
    #[arg(long, requires = "uninstall")]
    purge: bool,

    /// With `--uninstall`: keep them, without asking.
    #[arg(long, requires = "uninstall", conflicts_with = "purge")]
    keep_config: bool,

    /// Take the default answer to every question and ask nothing. What Add/Remove Programs'
    /// quiet uninstall runs.
    #[arg(long)]
    yes: bool,

    /// Set by an uninstall on itself, never by hand: the folder the relocated copy is to remove.
    #[arg(long, value_name = "DIR", hide = true, requires = "uninstall")]
    uninstall_target: Option<PathBuf>,
}

impl Args {
    /// The install request this command line makes, or `None` for an ordinary server run.
    fn install_options(&self) -> Option<gazelle_audio_server::install::Options> {
        if !self.install && !self.uninstall {
            return None;
        }
        Some(gazelle_audio_server::install::Options {
            install: self.install,
            uninstall: self.uninstall,
            start: three(self.start, self.no_start),
            purge: three(self.purge, self.keep_config),
            yes: self.yes,
            target: self.uninstall_target.clone(),
        })
    }
}

/// Two flags that mean yes and no, and neither meaning "ask".
fn three(yes: bool, no: bool) -> Option<bool> {
    match (yes, no) {
        (true, _) => Some(true),
        (_, true) => Some(false),
        _ => None,
    }
}

/// Public so the windowless build (`bin/gazelle-audio-serverw.rs`) runs this same server.
pub fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    // Planting the app, or taking it away, is all this run does: no listener, no devices, no
    // tray, no log file. It prints to whoever asked and stops.
    if let Some(options) = args.install_options() {
        return gazelle_audio_server::install::run(&options).map_err(Into::into);
    }
    let log_dir = logging::init(logging::file_log_dir(!args.no_tray, args.log_dir.clone(), || {
        default_log_dir(|k| std::env::var(k).ok())
    }));
    // The error is printed to the console on return anyway; a log file needs it too, since a
    // server with no terminal (the likeliest reason: its port is taken) leaves nothing else.
    run(&args, log_dir.clone()).inspect_err(|e| {
        if log_dir.is_some() {
            tracing::error!("the server stopped: {e}");
        }
    })
}

fn run(args: &Args, log_dir: Option<PathBuf>) -> Result<(), Box<dyn std::error::Error>> {
    // Before anything else: a binary an earlier update displaced is nothing but clutter now.
    update::clean_up_after_previous_update();

    let runtime = tokio::runtime::Runtime::new()?;
    // The port is taken before anything else is opened, so a second launch that is about to hand
    // over never takes a USB handle or a workspace file on its way out.
    let listener = match runtime.block_on(tokio::net::TcpListener::bind(args.bind)) {
        Ok(listener) => listener,
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => return second_instance(args.bind, &e),
        Err(e) => return Err(Box::new(e)),
    };
    let updater = updater(args);
    let address = listener.local_addr()?;
    // Set when the tray asks to restart into a staged update; acted on once the server is down.
    let restart = Arc::new(std::sync::atomic::AtomicBool::new(false));

    // Before the devices are attached, so the window is on screen while that happens rather than
    // after it; its first request waits in the listener's backlog until the server answers.
    let window = open_window(args, address);
    let (app, devices, hotplug) = runtime.block_on(prepare(args, address, window.clone(), updater.clone()))?;

    // Quit in the tray and Ctrl-C both end the server the same way.
    let quit = Arc::new(Notify::new());

    // The tray's message loop needs the thread that created the icon, so it takes this one and
    // the server runs on the runtime's workers.
    let tray = if args.no_tray {
        None
    } else {
        let mut context = tray_context(args, address, &devices, &quit, log_dir, window.clone());
        context.update = updater.clone();
        let asked = restart.clone();
        context.restart = Some(Box::new(move || asked.store(true, std::sync::atomic::Ordering::SeqCst)));
        context.rescan = hotplug.as_ref().map(|hotplug| {
            let rescan = hotplug.rescan();
            Box::new(move || rescan.now()) as Box<dyn Fn()>
        });
        match tray::start(context) {
            Ok(tray) => {
                tracing::info!("tray icon added (--no-tray to run without one)");
                Some(tray)
            }
            Err(e) => {
                tracing::warn!("no tray icon, running headless: {e}");
                None
            }
        }
    };

    if let Some(updater) = updater {
        update::spawn_background_checks(updater);
    }

    let closer = tray.as_ref().map(tray::Tray::closer);
    let server = runtime.spawn(async move {
        let result = serve(listener, app, devices, hotplug, quit).await;
        // Stopped by Ctrl-C or an error rather than the tray: take the icon down too.
        if let Some(closer) = closer {
            closer.close();
        }
        result
    });
    if let Some(tray) = tray {
        tray.run();
    }
    runtime.block_on(server)??;
    if restart.load(std::sync::atomic::Ordering::SeqCst) {
        update::relaunch();
    }
    Ok(())
}

/// The updater, when this server should have one.
///
/// Off with `--no-update` or when the settings say not to check, and **only on a loopback
/// bind**: an update check and a download belong to the machine running the server, not to
/// whoever can reach it over the network, so a server bound anywhere else offers neither the
/// tray items nor the HTTP routes.
fn updater(args: &Args) -> Option<Arc<Updater>> {
    let path = update::settings::default_settings_path(|k| std::env::var(k).ok());
    let (settings, warning) = Settings::load(&path);
    if let Some(warning) = warning {
        tracing::warn!("{warning}");
    }
    if !update::is_offered(args.bind.ip(), args.no_update, &settings) {
        tracing::info!("no update checks (--no-update, the settings in {}, or a non-loopback bind)", path.display());
        return None;
    }
    match Updater::for_this_build(settings) {
        Ok(updater) => Some(Arc::new(updater)),
        Err(e) => {
            tracing::warn!("no update checks: {e}");
            None
        }
    }
}

/// Everything behind the bound listener: devices attached, workspace store chosen, routes built.
/// For the USB backend, also the scanner that keeps attaching and detaching devices from then on.
///
/// `address` is what the listener really bound, which with port 0 is not what was asked for.
async fn prepare(
    args: &Args,
    address: SocketAddr,
    show_window: Option<gazelle_audio_server::ShowWindow>,
    updater: Option<Arc<Updater>>,
) -> Result<(axum::Router, Arc<DeviceManager>, Option<HotPlug>), Box<dyn std::error::Error>> {
    let registries = RegistrySet::builtin().map_err(|e| format!("loading registries: {e}"))?;

    let pids: Vec<u16> = args
        .loopback_models
        .iter()
        .filter_map(|m| {
            let pid = registries.models().find(|(_, r)| r.family == m.as_str()).map(|(pid, _)| *pid);
            if pid.is_none() {
                tracing::warn!("unknown loopback model '{m}', skipping");
            }
            pid
        })
        .collect();

    let devices = DeviceManager::new(registries);
    let hotplug = if args.backend == Backend::Usb {
        Some(attach_usb_devices(&devices)?)
    } else {
        match args.loopback_cyclic_ms {
            Some(ms) => devices.attach_cyclic_loopbacks(&pids, 64, std::time::Duration::from_millis(ms.max(1))),
            None => devices.attach_loopbacks(&pids, 64),
        }
        None
    };

    let store: Arc<dyn WorkspaceStore> = if args.no_persist {
        Arc::new(MemoryStore::default())
    } else {
        let path = args
            .workspace
            .clone()
            .unwrap_or_else(|| default_workspace_path(|k| std::env::var(k).ok()));
        tracing::info!("workspace: {}", path.display());
        Arc::new(JsonFileStore::new(path))
    };

    let snapshots: Arc<dyn SnapshotStore> = if args.no_persist {
        Arc::new(MemorySnapshotStore::default())
    } else {
        let dir = args
            .snapshots_dir
            .clone()
            .unwrap_or_else(|| default_snapshots_dir(|k| std::env::var(k).ok()));
        tracing::info!("snapshots: {}", dir.display());
        Arc::new(JsonDirStore::new(dir))
    };

    let state = AppState {
        devices: devices.clone(),
        store,
        snapshots,
        force_dry_run: args.dry_run,
        enable_recall: args.enable_recall,
        backend: format!("{:?}", args.backend).to_lowercase(),
        themes_dir: Some(args.themes_dir.clone().unwrap_or_else(|| default_themes_dir(|k| std::env::var(k).ok()))),
        show_window,
    };

    let app = http::router(state);
    let app = match updater {
        Some(updater) => app.merge(http::update::routes(updater)),
        None => app,
    };
    #[cfg(feature = "web-ui")]
    let app = if args.no_web_ui { app } else { gazelle_audio_server::web::with_ui(app) };

    // The bound address, not `args.bind`: with port 0 this log line is how another process (the
    // web client's integration tests) finds the server.
    tracing::info!(
        "listening on http://{} — backend={:?} devices={} dry_run={}",
        address,
        args.backend,
        devices.len(),
        args.dry_run
    );
    // A USB run that attached nothing while Antelope's service holds the devices is the one case
    // that looks like a working app with nothing plugged in. The scanner's warning is general; this
    // one names the reason, and the UI shows the same text (`notice`).
    for notice in gazelle_audio_server::notice::current(
        &format!("{:?}", args.backend).to_lowercase(),
        devices.len(),
        gazelle_audio_server::tray::antelope_service_running(),
    ) {
        tracing::warn!("{}", notice.message);
    }
    if !args.bind.ip().is_loopback() {
        tracing::warn!(
            "bound to a non-loopback address ({}); this exposes device control to the network",
            args.bind.ip()
        );
    }

    Ok((app, devices, hotplug))
}

/// The port is already taken. If a Gazelle holds it, hand it the window and go quietly; the
/// person double-clicked the app again, and what they want is the window they already have
/// (`handover`). Anything else on the port is the error it always was.
fn second_instance(bind: SocketAddr, error: &std::io::Error) -> Result<(), Box<dyn std::error::Error>> {
    match gazelle_audio_server::handover::hand_over(bind) {
        Ok(gazelle_audio_server::handover::Outcome::Shown) => {
            tracing::info!("Gazelle is already running on {bind}; brought its window to the front");
            Ok(())
        }
        Ok(gazelle_audio_server::handover::Outcome::Headless(reason)) => {
            tracing::info!("Gazelle is already running on {bind}, and {reason}");
            Ok(())
        }
        Err(why) => Err(format!("{bind} is in use ({error}), and what is listening there is not Gazelle: {why}").into()),
    }
}

/// Serve until Ctrl-C or Quit, then stop the device workers.
async fn serve(
    listener: tokio::net::TcpListener,
    app: axum::Router,
    devices: Arc<DeviceManager>,
    hotplug: Option<HotPlug>,
    quit: Arc<Notify>,
) -> std::io::Result<()> {
    let shutdown = async move {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = quit.notified() => {}
        }
        tracing::info!("shutting down, stopping device workers");
        // Scanning stops first, so nothing is attached behind the shutdown.
        if let Some(hotplug) = hotplug {
            hotplug.stop();
        }
        devices.shutdown_all();
    };
    http::serve_with_shutdown(listener, app, shutdown).await
}

/// What the tray is told about this server, including the arguments a boot entry repeats.
fn tray_context(
    args: &Args,
    address: SocketAddr,
    devices: &Arc<DeviceManager>,
    quit: &Arc<Notify>,
    log_dir: Option<PathBuf>,
    show_window: Option<gazelle_audio_server::ShowWindow>,
) -> tray::Context {
    let backend = format!("{:?}", args.backend).to_lowercase();
    let boot_args = BootArgs {
        bind: args.bind,
        backend: backend.clone(),
        dry_run: args.dry_run,
        workspace: args.workspace.clone(),
        themes_dir: args.themes_dir.clone(),
        log_dir: args.log_dir.clone(),
        loopback_models: args.loopback_models.clone(),
        loopback_cyclic_ms: args.loopback_cyclic_ms,
        no_web_ui: args.no_web_ui,
    };
    let boot_args = match std::env::current_dir() {
        Ok(cwd) => boot_args.absolute(&cwd),
        Err(_) => boot_args,
    };
    let quit = quit.clone();
    tray::Context {
        address,
        backend,
        dry_run: args.dry_run,
        web_ui: cfg!(feature = "web-ui") && !args.no_web_ui,
        devices: devices.clone(),
        exe: std::env::current_exe().unwrap_or_else(|_| PathBuf::from("gazelle-audio-server")),
        boot_args,
        log_dir,
        quit: Box::new(move || quit.notify_one()),
        show_window: show_window.map(|show| Box::new(move || show()) as Box<dyn Fn()>),
        rescan: None,
        update: None,
        restart: None,
    }
}

/// Opens the desktop window, when this build has one and this run wants one.
///
/// **A `--no-tray` run never has one.** That is the headless shape — a service, a test harness,
/// the web suite's own server — and a window appearing in the middle of one would be a surprise.
/// The desktop run is the tray and the window together. Nor is there one without the web UI to
/// put in it.
///
/// A window that cannot be created is a warning, never a reason to fail: the UI is still served,
/// and the tray's Open falls back to the browser.
#[cfg(feature = "window")]
fn open_window(args: &Args, address: SocketAddr) -> Option<gazelle_audio_server::ShowWindow> {
    if args.no_window || args.no_tray || args.no_web_ui {
        return None;
    }
    let path = gazelle_audio_server::config::default_window_state_path(|k| std::env::var(k).ok());
    match gazelle_audio_server::window::open(tray::ui_url(address), path) {
        Ok(window) => Some(Arc::new(move || window.show()) as gazelle_audio_server::ShowWindow),
        Err(e) => {
            tracing::warn!("no window, serving the UI over HTTP only: {e}");
            None
        }
    }
}

/// Without the `window` feature there is no window to open, and `--no-window` is accepted and
/// already true.
#[cfg(not(feature = "window"))]
fn open_window(_args: &Args, _address: SocketAddr) -> Option<gazelle_audio_server::ShowWindow> {
    None
}

/// Attaches every Antelope control interface the HID stack can open now, then keeps scanning.
///
/// Finding none, or none that opens, no longer stops the server: Antelope's Manager Service holds
/// the devices exclusively while it runs (hardware session 2), and they attach within a scan of it
/// being stopped. The scanner's log says so. Only a HID stack that cannot start at all is fatal.
fn attach_usb_devices(devices: &Arc<DeviceManager>) -> Result<HotPlug, Box<dyn std::error::Error>> {
    let api = hidapi::HidApi::new().map_err(|e| format!("opening the HID stack: {e}"))?;
    let mut scanner = Scanner::new(usb::HidEnumerator::new(api));
    for change in scanner.scan(devices) {
        change.log();
    }
    Ok(HotPlug::spawn(scanner, devices.clone(), hotplug::RESCAN_INTERVAL))
}
