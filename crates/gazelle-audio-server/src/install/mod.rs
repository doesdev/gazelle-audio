//! The portable self-install: `--install` and `--uninstall`.
//!
//! The app ships as a binary you can run from anywhere. `--install` is that binary planting
//! itself: it copies itself (and its windowless sibling, when that is beside it) into
//! `%LOCALAPPDATA%\Programs\Gazelle`, writes a Start Menu shortcut and registers an Add/Remove
//! Programs entry under `HKCU`. Everything is per-user, so nothing here asks for elevation and
//! nothing here can touch another account or the machine. `--uninstall` reverses exactly those
//! three things and nothing else.
//!
//! **Every path and every registry key goes through a seam.** The filesystem roots come from
//! [`Layout::from_env`], the registry from [`registry::Registry`], the login entry from the
//! [`RunKey`] the tray already had, and "is that binary running?" from a closure. So the whole
//! install → upgrade → uninstall story is tested against a temporary root, and no test writes to
//! the real Programs folder, the real Start Menu or the real `HKCU`.
//!
//! What it deliberately does not do: an MSI, a WiX or NSIS build step, elevation, a service, or
//! anything machine-wide: the app is portable, and installing is copying it into place.
//!
//! The release's `Gazelle-Setup.exe` is the same install without a terminal: the windowless
//! build, which on a double-click asks in a small dialog and calls [`install_windowless`]
//! (`setup`).
//!
//! **The aggregate driver** a release carries inside the executable is written into the install
//! folder too (`aggregate::bundled`), and a failure to write it never fails the install: every
//! start tries again. Registering it is the Aggregate page's, with Windows' administrator prompt.
//! An uninstall that finds the driver registered from the install folder says so and offers that
//! same prompt to remove the registration ([`offer_to_unregister`]); a quiet one never prompts.
//! While the driver is still registered its file is kept, so the registration never points at
//! nothing and the command that removes it still works.

#[cfg(windows)]
mod dialog;
pub mod registry;
pub mod setup;
pub mod shortcut;

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::aggregate::bundled;
use crate::aggregate::elevate::{arguments_for, command_for, Elevator, REGISTRAR};
use crate::aggregate::registry::{same_clsid, AsioRegistry, AGGREGATE_CLSID, AGGREGATE_NAME};
use crate::tray::boot::{self, RunKey, ENTRY_NAME};
use crate::update;

/// What the app is called in the Start Menu, in Add/Remove Programs, and as the install folder.
pub const APP_NAME: &str = "Gazelle";
/// Who Add/Remove Programs says published it. Not a legal entity and not a signing identity
/// (nothing here is signed; the shipping spec defers that), just the name of the project.
pub const PUBLISHER: &str = "Gazelle";
/// Where Add/Remove Programs' "publisher's website" link goes.
pub const ABOUT_URL: &str = "https://github.com/doesdev/gazelle-audio";
/// The per-user Add/Remove Programs entry. Under `HKCU`, which is what makes this need no UAC.
pub const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Gazelle";
/// Points the Add/Remove Programs entry somewhere else under `HKCU`. **For testing the installer
/// end to end**: it lets a test drive the real `--install` and `--uninstall`, through the real
/// registry code, without going anywhere near the list of the user's installed programs. Unset,
/// which it is for anybody not running the tests, the entry is [`UNINSTALL_KEY`].
pub const UNINSTALL_KEY_VAR: &str = "GAZELLE_UNINSTALL_KEY";
/// Points the login entry an install re-points and an uninstall removes somewhere else under
/// `HKCU`, for the same reason as [`UNINSTALL_KEY_VAR`]: a test driving the real `--install` and
/// `--uninstall` must never re-point, and then remove, the person's own Start on boot entry. Unset,
/// it is the real login entry, which the tray's Start on boot writes.
pub const RUN_KEY_VAR: &str = "GAZELLE_RUN_KEY";
/// The Start Menu file name.
pub const SHORTCUT_FILE: &str = "Gazelle.lnk";
/// The console build: the one Add/Remove Programs uninstalls with, when it is installed.
pub const CONSOLE_BINARY: &str = "gazelle-audio-server";
/// The windowless build: what the shortcut runs, and what the setup file is a copy of.
pub const WINDOWLESS_BINARY: &str = "gazelle-audio-serverw";

/// Where an install puts things, read from the environment so a test can point it at a temporary
/// root.
///
/// `programs` and `start_menu` are the installer's own; `config` and `logs` are the app's data,
/// named here only so `--purge` can offer to remove them and so an ordinary uninstall can say
/// plainly what it is leaving behind. They are exactly the folders the workspace and the tray's logs already use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    /// `%LOCALAPPDATA%\Programs\Gazelle`.
    pub programs: PathBuf,
    /// `%APPDATA%\Microsoft\Windows\Start Menu\Programs`.
    pub start_menu: PathBuf,
    /// `%APPDATA%\gazelle`: workspace, layouts, themes, snapshots.
    pub config: Option<PathBuf>,
    /// `%LOCALAPPDATA%\gazelle\logs`.
    pub logs: Option<PathBuf>,
    /// The Add/Remove Programs key, [`UNINSTALL_KEY`] unless [`UNINSTALL_KEY_VAR`] names another.
    pub uninstall_key: String,
}

impl Layout {
    /// `var` looks up an environment variable; it is a parameter so every path here is tested
    /// without mutating the process environment, and so the tests install into a temp folder.
    pub fn from_env(var: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let get = |name: &str| var(name).filter(|v| !v.is_empty());
        let local = get("LOCALAPPDATA")
            .ok_or("LOCALAPPDATA names nowhere to install to; this is a Windows install")?;
        let roaming = get("APPDATA").ok_or("APPDATA names no Start Menu to write a shortcut to")?;
        Ok(Self {
            programs: PathBuf::from(&local).join("Programs").join(APP_NAME),
            start_menu: PathBuf::from(&roaming).join("Microsoft").join("Windows").join("Start Menu").join("Programs"),
            config: crate::config::config_dir(|k| var(k)),
            logs: crate::config::default_log_dir(|k| var(k)),
            uninstall_key: get(UNINSTALL_KEY_VAR).unwrap_or_else(|| UNINSTALL_KEY.to_string()),
        })
    }
}

/// How long to keep looking at a binary that is still running before giving up on it.
///
/// One look, and no wait, is the ordinary answer: a person who left the app running is told to
/// quit it. The one case that waits is the relocated uninstall ([`relocation`]), where the
/// binary being removed is the parent process this one was started from and is on its way out.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Waiting {
    pub attempts: u32,
    pub delay: Duration,
}

impl Waiting {
    /// Look once and refuse.
    pub fn none() -> Self {
        Self { attempts: 1, delay: Duration::ZERO }
    }
    /// Wait about ten seconds, which is far longer than a process takes to exit.
    pub fn for_the_parent_to_exit() -> Self {
        Self { attempts: 100, delay: Duration::from_millis(100) }
    }
}

/// Everything an install or an uninstall acts on, all of it replaceable in a test.
pub struct Context<'a> {
    pub layout: &'a Layout,
    pub registry: &'a dyn registry::Registry,
    pub run_key: &'a dyn RunKey,
    /// Whether a binary at this path is running, so nothing ever replaces or deletes a live
    /// image. [`image_in_use`] is the real one.
    pub in_use: &'a dyn Fn(&Path) -> bool,
    pub waiting: Waiting,
    /// The aggregate driver to write into the install folder: the one the running binary
    /// carries ([`bundled::carried`]), or `None` for a build that carries none.
    pub driver: Option<&'a [u8]>,
    /// Where audio drivers are registered, read to see whether the aggregate driver is
    /// registered from the install folder. Never written.
    pub asio: &'a dyn AsioRegistry,
    /// Windows' administrator prompt, the one the Aggregate page registers the driver with,
    /// for an uninstall that offers to remove the registration.
    pub elevator: &'a dyn Elevator,
}

impl Context<'_> {
    /// Wait for `path` to stop being in use, within [`Waiting`]. `true` when it is still in use.
    fn still_in_use(&self, path: &Path) -> bool {
        for attempt in 0..self.waiting.attempts.max(1) {
            if !(self.in_use)(path) {
                return false;
            }
            if attempt + 1 < self.waiting.attempts {
                std::thread::sleep(self.waiting.delay);
            }
        }
        true
    }
}

/// What an install did, so the caller can print it.
#[derive(Clone, Debug)]
pub struct Installed {
    pub dir: PathBuf,
    /// Every binary now in place.
    pub copied: Vec<PathBuf>,
    /// The binary the shortcut and a "start it now" run: the windowless build when there is one.
    pub launch: PathBuf,
    pub shortcut: PathBuf,
    /// Whether something was already installed here.
    pub upgraded: bool,
    /// How many `.exe.old` files an earlier in-app update had left.
    pub swept: usize,
    /// The login entry as it now reads, when there was one and it had to be re-pointed.
    pub boot: Option<String>,
    pub size_kb: u32,
    /// What writing the aggregate driver did, or why it could not; `None` when this build
    /// carries no driver. An error here is not a failed install.
    pub driver: Option<Result<bundled::Placed, String>>,
}

/// What an uninstall removed, and what it left.
#[derive(Clone, Debug)]
pub struct Removed {
    pub dir: PathBuf,
    pub files: Vec<PathBuf>,
    pub shortcut: bool,
    pub boot: bool,
    pub dir_removed: bool,
    /// Data directories removed by `--purge`.
    pub purged: Vec<PathBuf>,
    /// Data directories deliberately left behind.
    pub kept: Vec<PathBuf>,
    /// What happened to the aggregate driver's file.
    pub driver: DriverRemoval,
    /// Older copies of the driver that could not be deleted, because a DAW still has one open.
    pub left: Vec<PathBuf>,
}

/// What an uninstall did with the aggregate driver's file in the install folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DriverRemoval {
    /// There was none.
    NotThere,
    Removed,
    /// Kept, because the driver is still registered from it: deleting it would leave DAWs a
    /// driver they list and cannot open, and the command that removes the registration needs it.
    KeptRegistered { dll: PathBuf },
    /// Kept, because whether it is registered could not be read.
    KeptUnknown { dll: PathBuf, why: String },
    /// It could not be deleted, most likely because a DAW has it open.
    InUse { dll: PathBuf, error: String },
}

/// Copy this binary into place, write the shortcut and register the Add/Remove Programs entry.
///
/// `source` is the running binary. Its windowless sibling comes too when it sits beside it, so
/// Start on boot and the Start Menu both have a build that opens no console window.
///
/// An install over an existing one is an upgrade in place, under the updater's rule: **never
/// replace a running image.** A running copy is refused with a message saying how to stop it,
/// and nothing is changed.
pub fn install(ctx: &Context, source: &Path) -> Result<Installed, String> {
    let source = absolute(source);
    let sources: Vec<(PathBuf, std::ffi::OsString)> = update::siblings(&source)
        .into_iter()
        .filter(|p| p.is_file())
        .map(|p| {
            let name = p.file_name().unwrap_or_default().to_os_string();
            (p, name)
        })
        .collect();
    plant(ctx, &source, sources)
}

/// Install the running **windowless** build, whatever its file is called.
///
/// This is what the setup file does: the release's `Gazelle-Setup.exe` is the windowless build
/// under a name a person recognises, downloaded on its own. It goes in as
/// `gazelle-audio-serverw.exe`, the name the shortcut, the login entry and the updater all use,
/// and it is the only file copied. A windowless build still called by its own name is installed
/// exactly as [`install`] would, sibling and all.
pub fn install_windowless(ctx: &Context, source: &Path) -> Result<Installed, String> {
    let source = absolute(source);
    let lower = |p: &Path| p.file_name().map(|n| n.to_string_lossy().to_lowercase());
    let own_name = update::siblings(&source).iter().any(|p| lower(p) == lower(&source));
    if own_name {
        return install(ctx, &source);
    }
    let name = file_name_of(&source, WINDOWLESS_BINARY).into_os_string();
    let sources = if source.is_file() { vec![(source.clone(), name)] } else { Vec::new() };
    plant(ctx, &source, sources)
}

/// Copy each `(from, installed name)` into the install folder, then write the shortcut and the
/// Add/Remove Programs entry. `source` is the running binary, which must not be the installed one.
fn plant(ctx: &Context, source: &Path, sources: Vec<(PathBuf, std::ffi::OsString)>) -> Result<Installed, String> {
    let dir = &ctx.layout.programs;
    if is_inside(source, dir) {
        return Err(format!(
            "this is already the installed copy, in {}. There is nothing to install; run it, or --uninstall it.",
            dir.display()
        ));
    }
    if sources.is_empty() {
        return Err(format!("{} is not a Gazelle binary to install", source.display()));
    }

    let console = dir.join(file_name_of(source, CONSOLE_BINARY));
    let upgraded = update::siblings(&console).iter().any(|p| p.exists());

    // Checked for every destination before the first byte is written, so a refusal leaves the
    // installed copy exactly as it was rather than half replaced.
    for (_, name) in &sources {
        let to = dir.join(name);
        if to.exists() && ctx.still_in_use(&to) {
            return Err(format!(
                "{} is running. Quit Gazelle first (right-click its tray icon and choose Quit), then run --install again.",
                to.display()
            ));
        }
    }

    std::fs::create_dir_all(dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    let mut copied = Vec::new();
    let mut bytes = 0u64;
    for (from, name) in &sources {
        let to = dir.join(name);
        bytes += std::fs::copy(from, &to).map_err(|e| format!("copying {} to {}: {e}", from.display(), to.display()))?;
        copied.push(to);
    }

    // A `.exe.old` the in-app updater left between staging and a restart is clutter in a folder
    // we have just rewritten; the updater's own sweep is the same call.
    let swept = update::clean_up_after(&console);

    // After the binaries, and never a reason to fail: the install is whole without it, the next
    // start writes it again, and a DAW holding the old copy cannot be allowed to block an upgrade.
    let driver = ctx.driver.map(|carried| {
        bundled::sweep(dir);
        bundled::put_in_place(dir, carried)
    });
    if matches!(driver, Some(Ok(_))) {
        bytes += ctx.driver.map_or(0, |carried| carried.len() as u64);
    }

    let launch = boot::boot_program(&console, |p| p.is_file());
    let size_kb = (bytes / 1024).max(1) as u32;
    let shortcut = write_shortcut(ctx, &launch, dir)?;
    write_uninstall_entry(ctx, dir, &console, &launch, size_kb)?;
    let boot = repoint_boot(ctx.run_key, &launch).map_err(|e| format!("re-pointing Start on boot: {e}"))?;

    Ok(Installed { dir: dir.clone(), copied, launch, shortcut, upgraded, swept, boot, size_kb, driver })
}

fn write_shortcut(ctx: &Context, launch: &Path, dir: &Path) -> Result<PathBuf, String> {
    let menu = &ctx.layout.start_menu;
    std::fs::create_dir_all(menu).map_err(|e| format!("creating {}: {e}", menu.display()))?;
    let path = menu.join(SHORTCUT_FILE);
    // No arguments: the shortcut starts the app as it is configured, and the settings live in
    // the config directory rather than in a command line nobody can see.
    // The shell's own item ID list for the target, which is what makes the link launchable.
    let id_list = shortcut::target_id_list(launch);
    let bytes = shortcut::shell_link(launch, "", dir, APP_NAME, launch, 0, id_list.as_deref());
    std::fs::write(&path, bytes).map_err(|e| format!("writing {}: {e}", path.display()))?;
    Ok(path)
}

/// The Add/Remove Programs entry, exactly as Windows reads it.
fn write_uninstall_entry(ctx: &Context, dir: &Path, console: &Path, launch: &Path, size_kb: u32) -> Result<(), String> {
    let (r, key) = (ctx.registry, ctx.layout.uninstall_key.as_str());
    let set =
        |name: &str, value: &str| r.set_string(key, name, value).map_err(|e| format!("writing HKCU\\{key}\\{name}: {e}"));
    set("DisplayName", APP_NAME)?;
    set("DisplayVersion", crate::VERSION)?;
    set("Publisher", PUBLISHER)?;
    set("InstallLocation", &dir.display().to_string())?;
    // The uninstall is the console build: it prints what it removed and asks about the config
    // directory. The quiet form answers that question with the default (keep) and says nothing.
    // An install from the setup file has no console build, and the windowless one does the same
    // uninstall, taking the default answer because it has no terminal to ask in.
    let uninstaller = if console.is_file() { console } else { launch };
    set("UninstallString", &format!("\"{}\" --uninstall", uninstaller.display()))?;
    set("QuietUninstallString", &format!("\"{}\" --uninstall --yes", uninstaller.display()))?;
    // There is no icon resource in the binary yet (the tray's icon is drawn at run time), so
    // this is the executable's default icon. Naming it anyway means the entry has an icon the
    // moment the binary gains one, with no second release to change the registry.
    set("DisplayIcon", &format!("{},0", launch.display()))?;
    set("URLInfoAbout", ABOUT_URL)?;
    let set_u32 = |name: &str, value: u32| {
        r.set_u32(key, name, value).map_err(|e| format!("writing HKCU\\{key}\\{name}: {e}"))
    };
    // Kilobytes, which is the unit Add/Remove Programs shows.
    set_u32("EstimatedSize", size_kb)?;
    // There is nothing to modify and nothing to repair: the two buttons are hidden rather than
    // offered and then failing.
    set_u32("NoModify", 1)?;
    set_u32("NoRepair", 1)?;
    Ok(())
}

/// Point an existing login entry at the installed windowless binary, keeping its options.
///
/// Start on boot is a setting the person turned on, and after an install the copy they
/// turned it on for may be a download folder that is about to be deleted. So an entry that
/// exists is re-pointed, with everything after the program name carried over untouched. An entry
/// that does not exist is **not** created: installing an app is not asking it to start at login.
/// An entry already pointing at the installed binary is left exactly as it is.
pub fn repoint_boot(run_key: &dyn RunKey, launch: &Path) -> io::Result<Option<String>> {
    let Some(entry) = run_key.read(ENTRY_NAME)? else { return Ok(None) };
    let Some(program) = boot::program_of(&entry) else { return Ok(None) };
    if same_path(program, launch) {
        return Ok(None);
    }
    let command = format!("\"{}\"{}", launch.display(), boot::arguments_of(&entry));
    run_key.write(ENTRY_NAME, &command)?;
    Ok(Some(command))
}

/// Remove the shortcut, the registry entry and the installed files, and nothing else.
///
/// Only the file names an install wrote are deleted, and only from `dir`; anything else in that
/// folder is left, and the folder itself only goes when nothing is left in it. The config and
/// log directories are kept unless `purge`.
pub fn uninstall(ctx: &Context, dir: &Path, purge: bool) -> Result<Removed, String> {
    let console = dir.join(console_name());
    let installed = update::siblings(&console);
    let ours: Vec<PathBuf> = installed.iter().flat_map(|p| [p.clone(), update::stage::old_path(p)]).collect();
    if !ours.iter().any(|p| p.exists()) {
        return Err(format!("no Gazelle is installed in {}", dir.display()));
    }

    // Before anything is removed, as the install does.
    for path in &installed {
        if path.exists() && ctx.still_in_use(path) {
            return Err(format!(
                "{} is running. Quit Gazelle first (right-click its tray icon and choose Quit), then uninstall again.",
                path.display()
            ));
        }
    }

    // A login entry that runs the copy being removed would be a broken entry after this.
    // One pointing anywhere else belongs to another install and is not ours to touch.
    let boot = match ctx.run_key.read(ENTRY_NAME) {
        Ok(Some(entry)) => match boot::program_of(&entry).map(|p| is_inside(Path::new(p), dir)) {
            Some(true) => ctx.run_key.remove(ENTRY_NAME).is_ok(),
            _ => false,
        },
        _ => false,
    };

    let shortcut_path = ctx.layout.start_menu.join(SHORTCUT_FILE);
    let shortcut = remove_if_there(&shortcut_path).map_err(|e| format!("removing {}: {e}", shortcut_path.display()))?;

    let key = ctx.layout.uninstall_key.as_str();
    ctx.registry.delete_tree(key).map_err(|e| format!("removing HKCU\\{key}: {e}"))?;

    let mut files = Vec::new();
    for path in ours {
        if remove_if_there(&path).map_err(|e| format!("removing {}: {e}", path.display()))? {
            files.push(path);
        }
    }
    // `remove_dir`, never `remove_dir_all`: a folder still holding something the person put
    // there stays, with what they put there still in it.
    let (driver, left) = remove_driver(ctx, dir, &mut files);
    let dir_removed = std::fs::remove_dir(dir).is_ok();

    let data: Vec<PathBuf> = [ctx.layout.config.clone(), ctx.layout.logs.clone()].into_iter().flatten().collect();
    let (mut purged, mut kept) = (Vec::new(), Vec::new());
    for path in data {
        if !path.exists() {
            continue;
        }
        if purge {
            std::fs::remove_dir_all(&path).map_err(|e| format!("removing {}: {e}", path.display()))?;
            purged.push(path);
        } else {
            kept.push(path);
        }
    }

    Ok(Removed { dir: dir.to_path_buf(), files, shortcut, boot, dir_removed, purged, kept, driver, left })
}

/// Remove the aggregate driver from `dir`, unless it is still registered from there, and every
/// older copy that nothing holds. Nothing here fails the uninstall: a file a DAW has open is
/// left and named, since the rest of Gazelle is gone either way.
fn remove_driver(ctx: &Context, dir: &Path, files: &mut Vec<PathBuf>) -> (DriverRemoval, Vec<PathBuf>) {
    let swept = bundled::sweep(dir);
    files.extend(swept.removed);
    let dll = bundled::path_in(dir);
    if !dll.exists() {
        return (DriverRemoval::NotThere, swept.kept);
    }
    let removal = match driver_registered_in(ctx.asio, dir) {
        Ok(Some(_)) => DriverRemoval::KeptRegistered { dll },
        Err(why) => DriverRemoval::KeptUnknown { dll, why },
        Ok(None) => match std::fs::remove_file(&dll) {
            Ok(()) => {
                files.push(dll);
                DriverRemoval::Removed
            }
            Err(e) => DriverRemoval::InUse { dll, error: e.to_string() },
        },
    };
    (removal, swept.kept)
}

// --- the aggregate driver's registration ---------------------------------------------------------

/// The DLL the aggregate driver's registration names, when that is inside `dir`. `None` when it
/// is not registered, or is registered from somewhere else: a copy somebody built or put there
/// themselves is not the uninstall's to touch.
pub fn driver_registered_in(asio: &dyn AsioRegistry, dir: &Path) -> Result<Option<PathBuf>, String> {
    let entries = asio.entries()?;
    Ok(entries
        .into_iter()
        .find(|entry| same_clsid(&entry.clsid, AGGREGATE_CLSID))
        .and_then(|entry| entry.dll.ok())
        .map(PathBuf::from)
        .filter(|dll| is_inside(dll, dir)))
}

/// What an uninstall did about the aggregate driver's registration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unregistering {
    /// It is not registered from the install folder, so there was nothing to offer.
    NothingToOffer,
    /// A quiet uninstall: it was left registered, and nobody was asked.
    Left { dll: PathBuf },
    /// The person chose to keep it registered.
    Declined { dll: PathBuf },
    /// The elevated registrar removed the registration.
    Unregistered { dll: PathBuf },
    /// The person said yes and it did not happen: the prompt was declined, or the registrar
    /// reported a failure. `message` says which.
    DidNotWork { dll: PathBuf, message: String },
    /// It is registered from the install folder and the file is not there, so the registrar could
    /// not unregister it and nothing was offered.
    FileGone { dll: PathBuf },
}

/// The question an uninstall asks when the aggregate driver is registered from its folder.
pub fn unregister_question(dll: &Path) -> String {
    format!(
        "{AGGREGATE_NAME}, the audio driver your DAWs open, is registered from Gazelle's folder ({}). \
         Remove its registration too? Windows will ask for administrator rights. If you keep it, DAWs can still \
         open the driver, and its file stays in that folder.",
        dll.display()
    )
}

/// How to remove the registration later, by hand, from an administrator terminal.
fn how_to_unregister(dll: &Path) -> String {
    let dir = dll.parent().map(|d| d.display().to_string()).unwrap_or_default();
    format!("To remove it later, open a terminal as administrator and run {}, then delete {dir}.", command_for(dll, true))
}

impl Unregistering {
    /// What to tell the person, a sentence or two a line. Empty when there is nothing to say.
    pub fn report(&self) -> Vec<String> {
        match self {
            Unregistering::NothingToOffer => Vec::new(),
            Unregistering::Left { dll } => vec![
                format!("{AGGREGATE_NAME} is still registered, from {}, so that file was kept.", dll.display()),
                how_to_unregister(dll),
            ],
            Unregistering::Declined { dll } => vec![
                format!("{AGGREGATE_NAME} is still registered, as you chose, from {}, so that file was kept.", dll.display()),
                how_to_unregister(dll),
            ],
            Unregistering::Unregistered { .. } => vec![format!("Removed the registration of {AGGREGATE_NAME}.")],
            Unregistering::DidNotWork { dll, message } => vec![
                format!("{AGGREGATE_NAME} is still registered: {message} Its file, {}, was kept.", dll.display()),
                how_to_unregister(dll),
            ],
            Unregistering::FileGone { dll } => vec![format!(
                "{AGGREGATE_NAME} is still registered, pointing at {}, which is not there any more, so a DAW that lists it \
                 cannot open it. To remove the registration, put a copy of the driver back at that path and run {} in a \
                 terminal opened as administrator.",
                dll.display(),
                command_for(dll, true)
            )],
        }
    }

    /// Whether the person should be told about this even without a terminal: it left something
    /// behind that only they can remove.
    pub fn needs_telling(&self) -> bool {
        matches!(self, Unregistering::Declined { .. } | Unregistering::DidNotWork { .. } | Unregistering::FileGone { .. })
    }
}

/// When the aggregate driver is registered from `dir`, offer to remove the registration with
/// Windows' administrator prompt, before anything is deleted: the registrar needs the file.
///
/// `quiet` (`--yes`) never asks and never prompts: the registration is left, and the answer says
/// how to remove it. `ask` puts the question to the person and says whether they agreed. Only a
/// yes reaches the elevator.
pub fn offer_to_unregister(ctx: &Context, dir: &Path, quiet: bool, ask: &mut dyn FnMut(&str) -> bool) -> Unregistering {
    let dll = match driver_registered_in(ctx.asio, dir) {
        Ok(Some(dll)) => dll,
        // Unreadable is not a reason to prompt; the uninstall keeps the file and says so.
        Ok(None) | Err(_) => return Unregistering::NothingToOffer,
    };
    if !ctx.asio.dll_present(&dll.display().to_string()) {
        return Unregistering::FileGone { dll };
    }
    if quiet {
        return Unregistering::Left { dll };
    }
    if !ask(&unregister_question(&dll)) {
        return Unregistering::Declined { dll };
    }
    match ctx.elevator.run(REGISTRAR, &arguments_for(&dll, true)) {
        Ok(run) if run.started && run.exit_code == Some(0) => Unregistering::Unregistered { dll },
        Ok(run) => Unregistering::DidNotWork { dll, message: run.message },
        Err(message) => Unregistering::DidNotWork { dll, message: format!("{message}.") },
    }
}

/// Where an install is, according to the Add/Remove Programs entry it wrote, falling back to
/// where this layout would have put it.
pub fn installed_dir(ctx: &Context) -> PathBuf {
    match ctx.registry.get_string(&ctx.layout.uninstall_key, "InstallLocation") {
        Ok(Some(dir)) if !dir.is_empty() => PathBuf::from(dir),
        _ => ctx.layout.programs.clone(),
    }
}

// --- removing the binary that is running ------------------------------------------------------

/// Where the running binary must copy itself to before it can remove the folder it is in.
///
/// Windows will not delete a running image. Two ways out: schedule the last step (a detached
/// `cmd /c` that waits and deletes, which leaves a shell process racing this one and a window
/// flashing), or copy out and re-exec. **Copy out and re-exec** is what this does: the second
/// copy is an ordinary run of the same program with [`relocated_arguments`], it does the whole
/// uninstall itself rather than half of it, and its only wait is for the parent's image to be
/// released ([`Waiting::for_the_parent_to_exit`]). The copy is left in the temp folder for
/// Windows to clean up, since it cannot delete itself either: the one thing this leaves behind.
///
/// `None` when the binary is not inside the folder being removed, which is the portable case: it
/// can remove the install folder as it stands.
pub fn relocation(exe: &Path, dir: &Path, temp: &Path, tag: u32) -> Option<PathBuf> {
    is_inside(exe, dir).then(|| temp.join(format!("gazelle-uninstall-{tag}.exe")))
}

/// What the relocated copy is run with: the folder to remove, the config decision already made,
/// and nothing to ask, because there is no one at a terminal by then.
pub fn relocated_arguments(dir: &Path, purge: bool) -> Vec<String> {
    vec![
        "--uninstall".into(),
        "--uninstall-target".into(),
        dir.display().to_string(),
        if purge { "--purge".into() } else { "--keep-config".into() },
        "--yes".into(),
    ]
}

// --- what the command line asks for -------------------------------------------------------------

/// The `--install` / `--uninstall` request, as the CLI spells it.
///
/// `start` and `purge` are three-valued on purpose: `None` is "ask, if there is anyone to ask",
/// which is the difference between a person running this in a terminal and Add/Remove Programs
/// running it with nowhere to put a question.
#[derive(Clone, Debug, Default)]
pub struct Options {
    pub install: bool,
    pub uninstall: bool,
    /// Start the installed copy afterwards.
    pub start: Option<bool>,
    /// Remove the config and log directories as well.
    pub purge: Option<bool>,
    /// Take the default answer to everything and ask nothing.
    pub yes: bool,
    /// The folder a relocated uninstall was told to remove; also the mark that says this process
    /// **is** the relocated copy ([`relocation`]).
    pub target: Option<PathBuf>,
}

/// Do what the command line asked, printing what happened. The one entry point `main` calls.
pub fn run(options: &Options) -> Result<(), String> {
    let layout = Layout::from_env(|k| std::env::var(k).ok())?;
    let registry = real_registry()?;
    let run_key = run_key_for(std::env::var(RUN_KEY_VAR).ok().filter(|k| !k.is_empty()));
    let in_use: &dyn Fn(&Path) -> bool = &image_in_use;
    let waiting = if options.target.is_some() { Waiting::for_the_parent_to_exit() } else { Waiting::none() };
    let asio = crate::aggregate::registry::for_this_pc();
    let elevator = crate::aggregate::elevate::for_this_pc();
    let ctx = Context {
        layout: &layout,
        registry: registry.as_ref(),
        run_key: run_key.as_ref(),
        in_use,
        waiting,
        driver: bundled::carried(),
        asio: asio.as_ref(),
        elevator: elevator.as_ref(),
    };

    if options.install {
        let source = std::env::current_exe().map_err(|e| format!("finding this binary: {e}"))?;
        return do_install(&ctx, &source, options);
    }
    do_uninstall(&ctx, options)
}

/// The login entry, or a stand-in for it under another `HKCU` key when [`RUN_KEY_VAR`] names one.
fn run_key_for(scratch: Option<String>) -> Box<dyn RunKey> {
    match scratch {
        #[cfg(windows)]
        Some(key) => Box::new(ScratchRunKey(key)),
        _ => crate::tray::user_run_key(),
    }
}

/// A login entry kept as a value under a key of its own, for a test. Removing the entry removes
/// that key, which holds nothing else.
#[cfg(windows)]
struct ScratchRunKey(String);

#[cfg(windows)]
impl RunKey for ScratchRunKey {
    fn read(&self, name: &str) -> io::Result<Option<String>> {
        registry::Registry::get_string(&registry::CurrentUser, &self.0, name)
    }
    fn write(&self, name: &str, command: &str) -> io::Result<()> {
        registry::Registry::set_string(&registry::CurrentUser, &self.0, name, command)
    }
    fn remove(&self, _name: &str) -> io::Result<()> {
        registry::Registry::delete_tree(&registry::CurrentUser, &self.0)
    }
}

#[cfg(windows)]
fn real_registry() -> Result<Box<dyn registry::Registry>, String> {
    Ok(Box::new(registry::CurrentUser))
}

#[cfg(not(windows))]
fn real_registry() -> Result<Box<dyn registry::Registry>, String> {
    Err("--install and --uninstall are Windows-only so far; elsewhere, copy the binary where you want it".into())
}

fn do_install(ctx: &Context, source: &Path, options: &Options) -> Result<(), String> {
    let done = install(ctx, source)?;
    println!("{} {} to {}", if done.upgraded { "Upgraded" } else { "Installed" }, crate::VERSION, done.dir.display());
    for path in &done.copied {
        println!("  {}", path.display());
    }
    if done.swept > 0 {
        println!("  swept {} file(s) a previous update had left", done.swept);
    }
    println!("  Start Menu: {}", done.shortcut.display());
    println!("  Add/Remove Programs: HKCU\\{}", ctx.layout.uninstall_key);
    match &done.boot {
        Some(command) => println!("  Start on boot now runs {command}"),
        None => println!("  Start on boot: unchanged"),
    }
    let dll = bundled::path_in(&done.dir);
    match &done.driver {
        None => {}
        Some(Ok(bundled::Placed::MovedAside { aside })) => println!(
            "  {} (the old copy is in use, so it was moved to {} and is removed at a later start)",
            dll.display(),
            aside.display()
        ),
        Some(Ok(_)) => println!("  {}", dll.display()),
        Some(Err(why)) => println!("  The aggregate driver was not put in place: {why}. Gazelle tries again each time it starts."),
    }
    println!("To remove it: \"{}\" --uninstall", done.dir.join(console_name()).display());

    let start = match options.start {
        Some(start) => start,
        None if options.yes => false,
        None => ask(&format!("Start {APP_NAME} now?"), true),
    };
    if start {
        start_installed(&done.launch)?;
    }
    Ok(())
}

fn do_uninstall(ctx: &Context, options: &Options) -> Result<(), String> {
    let dir = options.target.clone().unwrap_or_else(|| installed_dir(ctx));
    let data: Vec<PathBuf> =
        [ctx.layout.config.clone(), ctx.layout.logs.clone()].into_iter().flatten().filter(|p| p.exists()).collect();
    // The question is asked here, in the process that still has a terminal, and the answer is
    // carried to the relocated copy rather than asked again where nobody could see it.
    let purge = match options.purge {
        Some(purge) => purge,
        None if options.yes || data.is_empty() => false,
        None => {
            println!("Your settings, layouts, themes, snapshots and logs are in:");
            for path in &data {
                println!("  {}", path.display());
            }
            ask("Remove them too? (they are kept by default)", false)
        }
    };

    // Asked here too, before the copy below, which has nobody to ask: the registrar needs the
    // driver's file, and the relocated copy deletes it only once it is no longer registered.
    if options.target.is_none() {
        let terminal = std::io::IsTerminal::is_terminal(&std::io::stdin());
        let unregistering = offer_to_unregister(ctx, &dir, options.yes, &mut |question| ask_to_unregister(question, terminal));
        let report = unregistering.report();
        for line in &report {
            println!("{line}");
        }
        if !terminal && !options.yes && unregistering.needs_telling() {
            tell_without_a_terminal(&report.join("\n\n"));
        }
    }

    // A binary cannot delete itself. When this one is inside the folder being removed it copies
    // itself out, hands the whole job to the copy and exits so its image is released.
    if options.target.is_none() {
        let exe = std::env::current_exe().map_err(|e| format!("finding this binary: {e}"))?;
        if let Some(copy) = relocation(&exe, &dir, &std::env::temp_dir(), std::process::id()) {
            std::fs::copy(&exe, &copy).map_err(|e| format!("copying this binary to {}: {e}", copy.display()))?;
            std::process::Command::new(&copy)
                .args(relocated_arguments(&dir, purge))
                .spawn()
                .map_err(|e| format!("starting {}: {e}", copy.display()))?;
            println!("Removing {} from {}.", APP_NAME, dir.display());
            return Ok(());
        }
    }

    let removed = uninstall(ctx, &dir, purge)?;
    println!("Removed {} from {}", APP_NAME, removed.dir.display());
    for path in &removed.files {
        println!("  {}", path.display());
    }
    if removed.shortcut {
        println!("  {}", ctx.layout.start_menu.join(SHORTCUT_FILE).display());
    }
    println!("  HKCU\\{}", ctx.layout.uninstall_key);
    if removed.boot {
        println!("  the Start on boot login entry");
    }
    match &removed.driver {
        DriverRemoval::NotThere | DriverRemoval::Removed => {}
        DriverRemoval::KeptRegistered { dll } => println!("Kept {}: {AGGREGATE_NAME} is still registered from it.", dll.display()),
        DriverRemoval::KeptUnknown { dll, why } => {
            println!("Kept {}: whether {AGGREGATE_NAME} is registered from it could not be read ({why}).", dll.display())
        }
        DriverRemoval::InUse { dll, error } => println!(
            "Kept {}: it could not be deleted ({error}), most likely because a DAW has it open. Delete it once the DAW is closed.",
            dll.display()
        ),
    }
    for path in &removed.left {
        println!("Kept {}: an older copy of the driver that a DAW still has open. Delete it once the DAW is closed.", path.display());
    }
    let driver_stays = !matches!(removed.driver, DriverRemoval::NotThere | DriverRemoval::Removed) || !removed.left.is_empty();
    if !removed.dir_removed {
        if driver_stays {
            println!("  {} was left, with the driver in it", removed.dir.display());
        } else {
            println!("  {} was left: it still holds files this did not put there", removed.dir.display());
        }
    }
    for path in &removed.purged {
        println!("  {} (--purge)", path.display());
    }
    for path in &removed.kept {
        println!("Kept {}. Remove it by hand, or uninstall with --purge.", path.display());
    }
    Ok(())
}

/// Start the copy that has just been installed, honouring the single-instance handover:
/// a Gazelle already listening on the default address is brought to the front rather than a
/// second one started behind it.
pub(crate) fn start_installed(launch: &Path) -> Result<(), String> {
    let address = std::net::SocketAddr::from(([127, 0, 0, 1], crate::config::DEFAULT_PORT));
    match crate::handover::hand_over(address) {
        Ok(crate::handover::Outcome::Shown) => {
            println!("{APP_NAME} was already running on {address}; brought its window to the front.");
            return Ok(());
        }
        Ok(crate::handover::Outcome::Headless(reason)) => {
            println!("{APP_NAME} was already running on {address}, and {reason}.");
            return Ok(());
        }
        // Nothing of ours is listening, so there is nothing to hand over to: start one.
        Err(_) => {}
    }
    let child = update::relaunch_command(launch, [])
        .spawn()
        .map_err(|e| format!("starting {}: {e}", launch.display()))?;
    println!("Started {} as process {}.", launch.display(), child.id());
    Ok(())
}

/// The question about the aggregate driver's registration: in the terminal when there is one,
/// and otherwise in a dialog, since an uninstall from Settings, Apps of an install made with the
/// setup file runs the windowless build, which has no terminal, and whose person is right there.
/// `--yes` never gets this far.
fn ask_to_unregister(question: &str, terminal: bool) -> bool {
    if terminal {
        return ask(question, true);
    }
    #[cfg(windows)]
    {
        use setup::{Choice, Question, Tone};
        let question = Question { text: question.to_string(), choices: vec![Choice::Unregister, Choice::KeepRegistered], tone: Tone::Question };
        dialog::ask(&question) == Choice::Unregister
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Say something that matters to a person with no terminal to read it in.
fn tell_without_a_terminal(text: &str) {
    #[cfg(windows)]
    {
        use setup::{Choice, Question, Tone};
        dialog::ask(&Question { text: text.to_string(), choices: vec![Choice::Close], tone: Tone::Warning });
    }
    #[cfg(not(windows))]
    {
        let _ = text;
    }
}

/// A yes/no question, when there is someone at a terminal to answer it. Started from Add/Remove
/// Programs, from Explorer or from a script there is not, and the default stands.
fn ask(question: &str, default_yes: bool) -> bool {
    use std::io::{IsTerminal, Write};
    if !std::io::stdin().is_terminal() {
        return default_yes;
    }
    print!("{question} [{}] ", if default_yes { "Y/n" } else { "y/N" });
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return default_yes;
    }
    match line.trim().to_lowercase().as_str() {
        "y" | "yes" => true,
        "n" | "no" => false,
        _ => default_yes,
    }
}

// --- the real seams ---------------------------------------------------------------------------

/// Whether a binary at `path` is being run right now.
///
/// Windows locks a running image against writing, so asking for write access is the question:
/// a sharing violation means a process has it open as an image. Nothing else counts (a
/// read-only file, or a file that is not there, is not "running") because this answer is what
/// refuses an install, and a wrong yes would be an app that cannot be updated.
pub fn image_in_use(path: &Path) -> bool {
    match std::fs::OpenOptions::new().write(true).open(path) {
        Ok(_) => false,
        // 32 is ERROR_SHARING_VIOLATION, 33 ERROR_LOCK_VIOLATION.
        Err(e) => matches!(e.raw_os_error(), Some(32) | Some(33)),
    }
}

// --- small shared pieces -----------------------------------------------------------------------

pub(crate) fn console_name() -> String {
    if cfg!(windows) {
        format!("{CONSOLE_BINARY}.exe")
    } else {
        CONSOLE_BINARY.to_string()
    }
}

/// `CONSOLE_BINARY` with the extension `source` has, so an install driven by
/// `gazelle-audio-serverw.exe` still names the console build correctly.
fn file_name_of(source: &Path, stem: &str) -> PathBuf {
    match source.extension() {
        Some(extension) => PathBuf::from(stem).with_extension(extension),
        None => PathBuf::from(stem),
    }
}

fn absolute(path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    std::env::current_dir().map(|cwd| cwd.join(path)).unwrap_or_else(|_| path.to_path_buf())
}

fn remove_if_there(path: &Path) -> io::Result<bool> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

/// A path as a comparable list of components: resolved where the file exists, lower-cased
/// because Windows paths compare without case, and with the `\\?\` a canonical Windows path
/// carries taken off so a resolved path and a plain one still line up.
fn components(path: &Path) -> Vec<String> {
    let path = resolved(path);
    let text = path.to_string_lossy().into_owned();
    let text = text.strip_prefix(r"\\?\").unwrap_or(&text);
    text.split(['\\', '/']).filter(|c| !c.is_empty()).map(str::to_lowercase).collect()
}

/// A path in the one spelling Windows settles on, so two spellings of one place compare equal.
///
/// Windows still hands out short 8.3 names (`C:\Users\RUNNER~1` on a CI runner, `JOHNSM~1` for a
/// person called John Smith), and canonicalising turns them into the long form. But canonicalising
/// fails on a path that does not exist, and the one place this matters most is a registration
/// whose file has gone: the folder would resolve to its long name while the file under it kept its
/// short one, and the two would read as different places. So the deepest part that does exist is
/// resolved, and the rest is put back on the end as it was written.
fn resolved(path: &Path) -> PathBuf {
    if let Ok(whole) = path.canonicalize() {
        return whole;
    }
    let mut missing = Vec::new();
    let mut existing = path;
    loop {
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                missing.push(name.to_os_string());
                existing = parent;
                if let Ok(mut found) = existing.canonicalize() {
                    for name in missing.iter().rev() {
                        found.push(name);
                    }
                    return found;
                }
            }
            // Nothing of it exists, not even the drive: compared as written.
            _ => return path.to_path_buf(),
        }
    }
}

/// Whether `path` sits inside `dir`, at any depth.
pub fn is_inside(path: &Path, dir: &Path) -> bool {
    let (path, dir) = (components(path), components(dir));
    path.len() > dir.len() && path[..dir.len()] == dir[..]
}

fn same_path(a: &str, b: &Path) -> bool {
    components(Path::new(a)) == components(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_inside_a_folder_is_recognised_whatever_the_case_or_the_slashes() {
        let dir = Path::new(r"C:\Users\u\AppData\Local\Programs\Gazelle");
        assert!(is_inside(Path::new(r"C:\Users\U\AppData\Local\Programs\GAZELLE\gazelle-audio-server.exe"), dir));
        assert!(is_inside(Path::new("C:/Users/u/AppData/Local/Programs/Gazelle/sub/x.exe"), dir));
        assert!(!is_inside(dir, dir), "a folder is not inside itself");
        assert!(!is_inside(Path::new(r"C:\Users\u\AppData\Local\Programs\Gazelle-other\x.exe"), dir), "a longer name is not the same folder");
        assert!(!is_inside(Path::new(r"D:\portable\gazelle-audio-server.exe"), dir));
    }

    /// The same folder, spelled with its short 8.3 name and with its long one, is one folder, even
    /// for a file inside it that is not there. That last part is the whole point: an uninstall
    /// asking whether a registration points into Gazelle's folder is asking about a driver file
    /// that may already have gone, and CI's `C:\Users\RUNNER~1` found that it read as elsewhere.
    #[cfg(windows)]
    #[test]
    fn a_short_name_and_a_long_name_are_one_folder_even_for_a_file_that_has_gone() {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        use windows_sys::Win32::Storage::FileSystem::GetShortPathNameW;

        let long = std::env::temp_dir().join(format!("gazelle resolved check {}", std::process::id()));
        std::fs::create_dir_all(&long).unwrap();
        // The short name is Windows' to give, once, when the folder is made; it is asked for here
        // and the folder is kept, since making it again could give it another.
        let wide: Vec<u16> = long.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut buffer = vec![0u16; 1024];
        // Safety: `wide` is a terminated string this function owns, and the buffer's length is
        // the one passed; the answer is the number of characters written, zero on failure.
        let written = unsafe { GetShortPathNameW(wide.as_ptr(), buffer.as_mut_ptr(), buffer.len() as u32) } as usize;
        assert!(written > 0 && written < buffer.len(), "Windows gave no short name for {}", long.display());
        let short = PathBuf::from(std::ffi::OsString::from_wide(&buffer[..written]));
        if short == long {
            // This volume hands out no short names, so there is nothing to tell apart here.
            let _ = std::fs::remove_dir_all(&long);
            return;
        }
        let gone = short.join("Gazelle").join("gazelle_aggregate.dll");
        std::fs::create_dir_all(long.join("Gazelle")).unwrap();
        assert!(!gone.exists(), "the file is meant to be missing");
        assert!(is_inside(&gone, &long.join("Gazelle")), "{} is inside {}", gone.display(), long.display());
        assert!(is_inside(&long.join("Gazelle").join("x.dll"), &short.join("Gazelle")), "and the other way round");
        let _ = std::fs::remove_dir_all(&long);
    }

    #[test]
    fn the_console_binary_is_named_after_the_binary_the_install_was_run_from() {
        assert_eq!(file_name_of(Path::new(r"D:\p\gazelle-audio-serverw.exe"), CONSOLE_BINARY), PathBuf::from("gazelle-audio-server.exe"));
        assert_eq!(file_name_of(Path::new("/opt/gazelle-audio-serverw"), CONSOLE_BINARY), PathBuf::from("gazelle-audio-server"));
    }

    #[test]
    fn nothing_that_is_not_there_is_in_use() {
        assert!(!image_in_use(Path::new("no-such-binary-anywhere.exe")));
    }

    #[test]
    fn waiting_looks_once_by_default_and_many_times_for_a_parent() {
        assert_eq!(Waiting::none().attempts, 1);
        assert_eq!(Waiting::none().delay, Duration::ZERO);
        assert!(Waiting::for_the_parent_to_exit().attempts > 1);
        assert!(Waiting::for_the_parent_to_exit().delay > Duration::ZERO);
    }
}
