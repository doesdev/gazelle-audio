//! Gazelle control server.
//!
//! **The backend is `usb` unless told otherwise** (decision `0018`): talking to the interfaces is
//! what the app is for, attaching is read-only, and every write is still its own deliberate act:
//! `--dry-run` is off by default but always available. `--backend loopback` is the hardware-free
//! emulator, which every test suite names explicitly, and a server started with
//! `GAZELLE_NO_HARDWARE` set refuses `usb` outright (`no_hardware`).
//!
//! The listener still binds to localhost unless told otherwise. Phones on the network are a
//! setting of their own, and every request from another machine needs a paired phone's token
//! (`remote`).

use clap::{Parser, ValueEnum};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use gazelle_audio_server::aggregate::bundled;
use gazelle_audio_server::aggregate::export::ExportingStore;
use gazelle_audio_server::aggregate::service::AggregateService;
use gazelle_audio_server::config::{default_aggregate_path, default_log_dir, default_snapshots_dir, default_themes_dir, default_workspace_path};
use gazelle_audio_server::device::hotplug::{self, HotPlug, Scanner};
use gazelle_audio_server::device::manager::DeviceManager;
use gazelle_audio_server::device::usb;
use gazelle_audio_server::registry_set::RegistrySet;
use gazelle_audio_server::remote::store::{default_remote_path, Backing};
use gazelle_audio_server::remote::{self, guard, Exposure, Remote};
use gazelle_audio_server::snapshot::store::{JsonDirStore, MemorySnapshotStore, SnapshotStore};
use gazelle_audio_server::workspace::store::{JsonFileStore, MemoryStore, WorkspaceStore};
use gazelle_audio_server::tray::{self, boot::BootArgs};
use gazelle_audio_server::update::{self, settings::Settings, Updater};
use gazelle_audio_server::recording::RecordingService;
use gazelle_audio_server::studio::{settings::default_studio_path, settings::Backing as StudioBacking, Studio};
use gazelle_audio_server::window::Viewports;
use gazelle_audio_server::{http, logging, AppState};
use tokio::sync::Notify;

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum Backend {
    /// Hardware-free emulator: for trying the UI without an interface, for reproducing a bug, and
    /// for every test suite. Asked for by name.
    Loopback,
    /// Real USB devices, over the OS HID stack. The default: driving the interfaces is what the
    /// app is for (decision `0018`).
    Usb,
}

impl Backend {
    /// The name `--backend` spells, which is also what `/api/v1/health` and the tray report.
    fn name(self) -> String {
        format!("{self:?}").to_lowercase()
    }
}

#[derive(Parser, Debug)]
#[command(name = "gazelle-audio-server", version, about = "Gazelle control server for Antelope Audio interfaces")]
struct Args {
    /// Address to bind. Defaults to localhost; override only deliberately. A network address
    /// makes Gazelle reachable from other machines whatever the phones setting says, and every
    /// one of them then needs a paired phone's token.
    #[arg(long, default_value = gazelle_audio_server::config::DEFAULT_BIND)]
    bind: SocketAddr,

    /// Transport backend. `usb`, the default, drives the attached interfaces; `loopback` is the
    /// hardware-free emulator, for trying the UI without an interface and for the test suites.
    /// Setting GAZELLE_NO_HARDWARE makes a server refuse `usb`, which is how the harnesses make
    /// sure they never open a real device.
    #[arg(long, value_enum, default_value_t = Backend::Usb)]
    backend: Backend,

    /// Never write to a device: report the bytes each command would send.
    #[arg(long)]
    dry_run: bool,

    /// Allow the recall route to apply a snapshot to a device. Off by default, and off is not the
    /// whole guard: the request must ask as well, and applying is not built yet. It waits for a
    /// session at the hardware to confirm how recall must behave.
    #[arg(long)]
    enable_recall: bool,

    /// Where workspace state is stored. Defaults to a platform config path.
    #[arg(long)]
    workspace: Option<PathBuf>,

    /// Where snapshots are stored, one JSON file each. Defaults to a "snapshots" folder in the
    /// config folder, which stays there even when `--workspace` puts the workspace elsewhere.
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

    /// Start in the tray with the window hidden. The tray's Open, or launching Gazelle again,
    /// shows it. Start on boot runs Gazelle this way. The recording widget still comes back if it
    /// was open, and "Start in the recording hub" still opens the hub: a studio PC starting at
    /// login is what those are for.
    #[arg(long)]
    hidden: bool,

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
    /// terminal asks, and one started from Explorer or a script starts it (`--yes` does not).
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
    // Before the listener, the devices, or anything else: a run forbidden hardware that asked for
    // it anyway stops here, having opened nothing. `--backend` defaults to `usb`,
    // so this is what stands between a harness that forgot the flag and the user's own interfaces.
    if let Some(refusal) = gazelle_audio_server::no_hardware::refusal(
        &args.backend.name(),
        std::env::var(gazelle_audio_server::no_hardware::VAR).ok().as_deref(),
    ) {
        return Err(refusal.into());
    }
    // A binary an earlier update displaced is nothing but clutter now.
    update::clean_up_after_previous_update();
    // The aggregate driver this build carries goes beside it, when it is not there already: the
    // first start after an update is how a new driver arrives. Never fatal; the page says why.
    let carried = bundled::at_this_start();

    let runtime = tokio::runtime::Runtime::new()?;
    // The port is taken before anything else is opened, so a second launch that is about to hand
    // over never takes a USB handle or a workspace file on its way out.
    let listener = match runtime.block_on(tokio::net::TcpListener::bind(args.bind)) {
        Ok(listener) => listener,
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => return second_instance(args.bind, args.hidden, &e),
        Err(e) => return Err(Box::new(e)),
    };
    let updater = updater(args);
    let address = listener.local_addr()?;
    // Bound to one network address, Gazelle also listens on loopback at the same port: otherwise
    // nothing on this machine, the window included, could reach it without a token, and nothing
    // could start the pairing that gives one (`remote`).
    let local_listener = match Exposure::of(address) {
        Exposure::Address(_) => {
            let local = SocketAddr::from(([127, 0, 0, 1], address.port()));
            match runtime.block_on(tokio::net::TcpListener::bind(local)) {
                Ok(listener) => Some(listener),
                Err(e) => {
                    tracing::warn!("also listening on {local} for this machine failed: {e}");
                    None
                }
            }
        }
        _ => None,
    };
    // Where this machine's own window and tray open the app.
    let local_address = local_listener.as_ref().and_then(|l| l.local_addr().ok()).unwrap_or(address);
    let remote = remote(args, address);

    // Quit in the tray and Ctrl-C both end the server the same way, and a restart is a quit that
    // is followed by a relaunch: built here so the tray item and the HTTP route share the one
    // path (`update::Restart`), which is also the only thing that sets the flag read below.
    let quit = Arc::new(Notify::new());
    let restart = updater.clone().map(|updater| {
        let quit = quit.clone();
        Arc::new(update::Restart::new(updater, Box::new(move || quit.notify_one())))
    });

    // Auto-arm and starting in the hub, before the windows, which start in the hub when it says so.
    let studio = studio(args);

    // Before the devices are attached, so the window is on screen while that happens rather than
    // after it; its first request waits in the listener's backlog until the server answers.
    let desktop = open_window(args, local_address, studio.get().start_in_hub);
    let window = desktop.show.clone();
    let showing = desktop.showing;
    let (app, devices, hotplug, recording) = runtime.block_on(prepare(args, address, window.clone(), desktop.viewports.clone(), studio, restart.clone(), carried, remote.clone()))?;
    // The phone listener serves the same app, gate and all, and starts now if phones are allowed.
    remote.serve_phones(app.clone(), runtime.handle().clone());

    // The tray's message loop needs the thread that created the icon, so it takes this one and
    // the server runs on the runtime's workers.
    let tray = if args.no_tray {
        None
    } else {
        let mut context = tray_context(args, local_address, &devices, &quit, log_dir, window.clone());
        context.phones = Some(phones_toggle(&remote));
        context.recording = Some(recording_hooks(&recording, desktop.viewports.clone()));
        context.end_session = Some(end_session(&recording));
        context.update = updater.clone();
        context.restart = restart.clone().map(|restart| Box::new(move || restart.request()) as Box<dyn Fn() -> Result<String, String>>);
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
        let result = serve(listener, local_listener, app, devices, hotplug, quit, remote, recording).await;
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
    // The port is given up and the USB handles are closed; only now is it safe to start the
    // binary the update put in place.
    if restart.is_some_and(|restart| restart.asked()) {
        // The new process keeps the window as it is now: on screen, or closed to the tray.
        update::relaunch(showing.map(|showing| showing()));
    }
    Ok(())
}

/// The updater, when this server should have one.
///
/// Off with `--no-update` or when the settings say not to check. Wherever Gazelle listens, an update
/// check and a download belong to the machine running it: the HTTP routes answer only this
/// machine (`remote::guard`), and the tray is on this machine by nature.
fn updater(args: &Args) -> Option<Arc<Updater>> {
    let path = update::settings::default_settings_path(|k| std::env::var(k).ok());
    let (settings, warning) = Settings::load(&path);
    if let Some(warning) = warning {
        tracing::warn!("{warning}");
    }
    if !update::is_offered(args.no_update, &settings) {
        tracing::info!("no update checks (--no-update, or the settings in {})", path.display());
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
#[allow(clippy::too_many_arguments)]
async fn prepare(
    args: &Args,
    address: SocketAddr,
    show_window: Option<gazelle_audio_server::ShowWindow>,
    viewports: Option<Arc<dyn Viewports>>,
    studio: Arc<Studio>,
    restart: Option<Arc<update::Restart>>,
    carried: bundled::Status,
    remote: Arc<Remote>,
) -> Result<(axum::Router, Arc<DeviceManager>, Option<HotPlug>, Arc<RecordingService>), Box<dyn std::error::Error>> {
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

    // The audio driver's own settings (buffer size and Safe Mode, read and changed); a loopback
    // device is answered without a DLL, and `--dry-run` builds a change without sending it.
    let driver = gazelle_audio_server::driver::DriverService::for_this_pc();

    // The aggregate audio driver. Its setup lives in the workspace and is exported to the file
    // the driver reads whenever it changes, so the store is wrapped rather than replaced and
    // every other route is untouched. `--no-persist` keeps its file out of the way too.
    let aggregate_path = default_aggregate_path(|k| std::env::var(k).ok());
    let aggregate = AggregateService::for_this_pc(
        devices.clone(),
        driver.clone(),
        aggregate_path.clone(),
        std::env::current_exe().ok().and_then(|exe| exe.parent().map(std::path::Path::to_path_buf)),
        std::env::var("APPDATA").ok().map(std::path::PathBuf::from),
        carried,
    );
    let store: Arc<dyn WorkspaceStore> = if args.no_persist {
        store
    } else {
        // The names the driver is given are Gazelle's and follow the devices and their routing, so
        // the store sees the devices, and a follower brings the names up to date when the routing
        // changes or a device comes or goes, whether or not any page is open.
        let exporting = Arc::new(ExportingStore::new(store, &aggregate_path, aggregate.link.clone()).with_live(aggregate.clone()));
        match exporting.sync() {
            Ok(Some(exported)) => tracing::info!("wrote the aggregate's setup to {}", exported.path.display()),
            Ok(None) => {}
            Err(why) => tracing::warn!("the aggregate's setup could not be exported: {why}"),
        }
        tokio::spawn(gazelle_audio_server::aggregate::follow::follow(exporting.clone(), devices.clone(), !args.dry_run));
        exporting
    };

    let state = AppState {
        devices: devices.clone(),
        store: store.clone(),
        snapshots,
        force_dry_run: args.dry_run,
        enable_recall: args.enable_recall,
        backend: args.backend.name(),
        themes_dir: Some(args.themes_dir.clone().unwrap_or_else(|| default_themes_dir(|k| std::env::var(k).ok()))),
        show_window,
    };

    // One calibration and one recorder, which share the aggregate and each refuse while the other
    // has it. The loopback records from the aggregate's own fakes, never a driver.
    let calibration = Arc::new(gazelle_audio_server::aggregate::calibrate::Calibration::this_pc());
    // The recording settings come with it: auto-arm looks once a second, and does nothing while it
    // is off, which it is by default and always under --no-persist.
    let recording = RecordingService::for_backend_with(args.backend == Backend::Loopback, calibration.clone(), store.clone(), devices.clone(), studio);
    // The metronome's settings, beside the recording settings; --no-persist keeps them in memory.
    if !args.no_persist {
        let path = gazelle_audio_server::studio::metronome::default_metronome_path(|k| std::env::var(k).ok());
        if let Some(warning) = recording.load_metronome(StudioBacking::File(path)) {
            tracing::warn!("{warning}");
        }
    }
    tokio::spawn(recording.clone().follow());
    tokio::spawn(recording.clone().follow_auto_arm());

    let app = http::router(state)
        .merge(http::driver::routes(devices.clone(), driver, args.dry_run))
        .merge(http::aggregate::routes_sharing(aggregate, store, args.dry_run, calibration))
        .merge(http::recording::routes(recording.clone()))
        .merge(http::metronome::routes(recording.clone()))
        .merge(http::studio::routes(recording.clone(), viewports))
        .merge(http::remote::routes(remote.clone()));
    // Every route sees the recorder: the WebSocket sends its live state, the workspace keeps the
    // armed preset as it is, and a measurement is refused while it is armed.
    let app = app.layer(axum::Extension(recording.clone()));
    let app = match restart {
        Some(restart) => app.merge(http::update::routes(restart)),
        None => app,
    };
    #[cfg(feature = "web-ui")]
    let app = if args.no_web_ui { app } else { gazelle_audio_server::web::with_ui(app) };
    // Last, so it stands in front of everything above, the web app's files included.
    let app = guard::protect(app, remote);

    // The bound address, not `args.bind`: with port 0 this log line is how another process (the
    // web client's integration tests) finds the server.
    tracing::info!(
        "listening on http://{} (backend={} devices={} dry_run={})",
        address,
        args.backend.name(),
        devices.len(),
        args.dry_run
    );
    // A USB run that attached nothing is the one case that looks like a working app with nothing
    // in it. The scanner's warning is general; these name the reason (Antelope's service holding
    // the devices, or simply nothing plugged in) and the UI shows the same text (`notice`).
    for notice in gazelle_audio_server::notice::current(
        &args.backend.name(),
        devices.len(),
        gazelle_audio_server::tray::antelope_service_running(),
    ) {
        tracing::warn!("{}", notice.message);
    }
    if !args.bind.ip().is_loopback() {
        tracing::warn!(
            "bound to a non-loopback address ({}); other machines can reach Gazelle, and each needs a paired phone's token",
            args.bind.ip()
        );
    }

    Ok((app, devices, hotplug, recording))
}

/// The port is already taken. If a Gazelle holds it, hand it the window and go quietly; the
/// person double-clicked the app again, and what they want is the window they already have
/// (`handover`). Anything else on the port is the error it always was.
///
/// A `--hidden` start (a login) asks for no window, so it leaves the running one alone and goes.
fn second_instance(bind: SocketAddr, hidden: bool, error: &std::io::Error) -> Result<(), Box<dyn std::error::Error>> {
    if hidden {
        tracing::info!("{bind} is in use ({error}); a --hidden start leaves whatever holds it alone");
        return Ok(());
    }
    // Bound to one network address, the running copy also listens on loopback, and only a request
    // from this machine may raise its window.
    let target = match Exposure::of(bind) {
        Exposure::Address(_) => SocketAddr::from(([127, 0, 0, 1], bind.port())),
        _ => bind,
    };
    match gazelle_audio_server::handover::hand_over(target) {
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
///
/// `local` is the loopback listener a bind to one network address brings with it; it stops with
/// the main one. The phone listener stops first of all, so its port is free before a restart
/// into an update starts the next copy. Then the recorder is disarmed, which finishes any take's
/// files and lets go of the audio drivers, before the devices are closed and the process ends.
#[allow(clippy::too_many_arguments)]
async fn serve(
    listener: tokio::net::TcpListener,
    local: Option<tokio::net::TcpListener>,
    app: axum::Router,
    devices: Arc<DeviceManager>,
    hotplug: Option<HotPlug>,
    quit: Arc<Notify>,
    remote: Arc<Remote>,
    recording: Arc<RecordingService>,
) -> std::io::Result<()> {
    let (stop_local, local_stopped) = tokio::sync::oneshot::channel::<()>();
    if let Some(local) = local {
        let app = app.clone();
        tokio::spawn(async move {
            let shutdown = async move {
                let _ = local_stopped.await;
            };
            if let Err(e) = http::serve_with_shutdown(local, app, shutdown).await {
                tracing::warn!("the loopback listener stopped: {e}");
            }
        });
    }
    let shutdown = async move {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = quit.notified() => {}
        }
        gazelle_audio_server::window::note_shutdown();
        remote.stop();
        let _ = stop_local.send(());
        // A take being recorded is finished, not cut: its files closed with their sizes, its log
        // complete. Disarming waits for the writer, so it is not done on a runtime worker.
        if tokio::task::spawn_blocking(move || recording.shutdown()).await.is_err() {
            tracing::warn!("the recorder stopped part way through disarming at shutdown");
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

/// Remote access: the phones setting and the paired phones, from `remote.json` beside
/// `update.json`, or in memory only under `--no-persist`, so a test server never leaves the
/// owner's Gazelle open to the network.
fn remote(args: &Args, address: SocketAddr) -> Arc<Remote> {
    let backing = if args.no_persist {
        Backing::Memory
    } else {
        Backing::File(default_remote_path(|k| std::env::var(k).ok()))
    };
    let (remote, warning) = Remote::new(remote::Options::for_this_pc(backing, args.bind, address.port()));
    if let Some(warning) = warning {
        tracing::warn!("{warning}");
    }
    remote
}

/// The recording settings: auto-arm and starting in the hub, from `recording.json` beside
/// `remote.json`, or in memory only under `--no-persist`, so a test server never reads the owner's
/// file and never auto-arms.
fn studio(args: &Args) -> Arc<Studio> {
    let backing = if args.no_persist {
        StudioBacking::Memory
    } else {
        StudioBacking::File(default_studio_path(|k| std::env::var(k).ok()))
    };
    let (studio, warning) = Studio::new(backing);
    if let Some(warning) = warning {
        tracing::warn!("{warning}");
    }
    let settings = studio.get();
    if let Some(preset) = settings.auto_arm_with() {
        tracing::info!("auto-arm is on, with preset {preset:?}: Gazelle arms as soon as the interfaces are there, and holds the audio drivers while armed");
    }
    Arc::new(studio)
}

/// The tray's recording items: the widget and the hub when there are windows, auto-arm and
/// starting in the hub.
fn recording_hooks(recording: &Arc<RecordingService>, viewports: Option<Arc<dyn Viewports>>) -> tray::RecordingHooks {
    let (read, arm, hub_setting) = (recording.clone(), recording.clone(), recording.clone());
    let (seen, toggle, open) = (viewports.clone(), viewports.clone(), viewports);
    tray::RecordingHooks {
        menu: Box::new(move || {
            let settings = read.studio().get();
            let name = settings.auto_arm_preset.as_deref().map(|id| read.preset_name(id).unwrap_or_else(|| id.to_string()));
            tray::RecordingMenu { widget: seen.as_ref().map(|v| v.state().widget), auto_arm: settings.auto_arm, auto_arm_preset: name, start_in_hub: settings.start_in_hub }
        }),
        toggle_widget: Box::new(move || {
            if let Some(viewports) = &toggle {
                viewports.set_widget(!viewports.state().widget);
            }
        }),
        open_hub: Box::new(move || {
            if let Some(viewports) = &open {
                viewports.set_hub(true);
            }
        }),
        toggle_auto_arm: Box::new(move || arm.change_settings(|s| s.auto_arm = !s.auto_arm).map(|s| s.auto_arm)),
        toggle_start_in_hub: Box::new(move || hub_setting.change_settings(|s| s.start_in_hub = !s.start_in_hub).map(|s| s.start_in_hub)),
    }
}

/// What Windows ending the session waits for: the recorder finishing any take. The same
/// shutdown a Quit runs, from the tray's thread, which Windows is waiting on.
fn end_session(recording: &Arc<RecordingService>) -> tray::EndSession {
    let (busy, finish) = (recording.clone(), recording.clone());
    tray::EndSession { busy: Box::new(move || busy.is_active()), finish: Box::new(move || finish.shutdown()) }
}

/// The tray's Allow phones item: whether it is on, and a way to flip it.
fn phones_toggle(remote: &Arc<Remote>) -> tray::PhonesToggle {
    let (read, flip) = (remote.clone(), remote.clone());
    tray::PhonesToggle {
        fixed: remote.fixed_by_bind().is_some(),
        on: Box::new(move || read.allow_phones() || read.fixed_by_bind().is_some()),
        toggle: Box::new(move || {
            let on = !flip.allow_phones();
            flip.set_allow_phones(on).map(|()| on)
        }),
    }
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
    let backend = args.backend.name();
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
        phones: None,
        recording: None,
        end_session: None,
    }
}

/// Opens the desktop window, when this build has one and this run wants one.
///
/// **A `--no-tray` run never has one.** That is the headless shape (a service, a test harness,
/// the web suite's own server), and a window appearing in the middle of one would be a surprise.
/// The desktop run is the tray and the window together. Nor is there one without the web UI to
/// put in it.
///
/// A window that cannot be created is a warning, never a reason to fail: the UI is still served,
/// and the tray's Open falls back to the browser.
///
/// `--hidden` makes it but leaves it closed to the tray. Alongside the way to show it comes a way
/// to ask whether it is showing, which a restart into an update carries over, and the recording
/// widget and hub, which the routes and the tray open and close.
///
/// **Start in the hub wins over `--hidden` for the hub**, not for the app's window: a login start
/// with the setting on opens the full-screen hub, since a studio PC ready at login is the point of
/// the setting, and the app's window stays in the tray as `--hidden` asks. The widget comes back if
/// it was open, whatever `--hidden` says, for the same reason.
#[cfg(feature = "window")]
fn open_window(args: &Args, address: SocketAddr, hub: bool) -> Desktop {
    if args.no_window || args.no_tray || args.no_web_ui {
        return Desktop::none();
    }
    let path = |kind| gazelle_audio_server::window::state::state_path(kind, |k| std::env::var(k).ok());
    use gazelle_audio_server::window::Kind;
    let options = gazelle_audio_server::window::Options {
        url: tray::ui_url(address),
        main_state: path(Kind::Main),
        widget_state: path(Kind::Widget),
        hub_state: path(Kind::Hub),
        visible: !args.hidden,
        hub,
    };
    match gazelle_audio_server::window::open(options) {
        Ok(window) => {
            let (asked, viewports) = (window.clone(), window.clone());
            Desktop {
                show: Some(Arc::new(move || window.show()) as gazelle_audio_server::ShowWindow),
                showing: Some(Box::new(move || asked.is_showing()) as Showing),
                viewports: Some(viewports as Arc<dyn Viewports>),
            }
        }
        Err(e) => {
            tracing::warn!("no window, serving the UI over HTTP only: {e}");
            Desktop::none()
        }
    }
}

/// Whether the desktop window is on screen.
type Showing = Box<dyn Fn() -> bool>;

/// The desktop windows, as far as the rest of the server reaches them. All `None` without windows.
struct Desktop {
    show: Option<gazelle_audio_server::ShowWindow>,
    showing: Option<Showing>,
    viewports: Option<Arc<dyn Viewports>>,
}

impl Desktop {
    fn none() -> Desktop {
        Desktop { show: None, showing: None, viewports: None }
    }
}

/// Without the `window` feature there is no window to open, and `--no-window` is accepted and
/// already true.
#[cfg(not(feature = "window"))]
fn open_window(_args: &Args, _address: SocketAddr, _hub: bool) -> Desktop {
    Desktop::none()
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
