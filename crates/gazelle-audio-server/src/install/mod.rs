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
//! anything machine-wide. See `specs/2026-09-18-shipping-portable.md`.

pub mod registry;
pub mod shortcut;

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::tray::boot::{self, RunKey, ENTRY_NAME};
use crate::update;

/// What the app is called in the Start Menu, in Add/Remove Programs, and as the install folder.
pub const APP_NAME: &str = "Gazelle";
/// Who Add/Remove Programs says published it. Not a legal entity and not a signing identity —
/// nothing here is signed (the shipping spec defers that) — just the name of the project.
pub const PUBLISHER: &str = "Gazelle";
/// Where Add/Remove Programs' "publisher's website" link goes.
pub const ABOUT_URL: &str = "https://github.com/doesdev/gazelle-audio";
/// The per-user Add/Remove Programs entry. Under `HKCU`, which is what makes this need no UAC.
pub const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Gazelle";
/// The Start Menu file name.
pub const SHORTCUT_FILE: &str = "Gazelle.lnk";
/// The console build, which is always the one an install is driven from.
pub const CONSOLE_BINARY: &str = "gazelle-audio-server";

/// Where an install puts things, read from the environment so a test can point it at a temporary
/// root.
///
/// `programs` and `start_menu` are the installer's own; `config` and `logs` are the app's data,
/// named here only so `--purge` can offer to remove them and so an ordinary uninstall can say
/// plainly what it is leaving behind. They are exactly the folders P82 and P78 already chose.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    /// `%LOCALAPPDATA%\Programs\Gazelle`.
    pub programs: PathBuf,
    /// `%APPDATA%\Microsoft\Windows\Start Menu\Programs`.
    pub start_menu: PathBuf,
    /// `%APPDATA%\gazelle` — workspace, layouts, themes, snapshots (P82).
    pub config: Option<PathBuf>,
    /// `%LOCALAPPDATA%\gazelle\logs` (P78).
    pub logs: Option<PathBuf>,
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
    let dir = &ctx.layout.programs;
    let source = absolute(source);
    if is_inside(&source, dir) {
        return Err(format!(
            "this is already the installed copy, in {}. There is nothing to install; run it, or --uninstall it.",
            dir.display()
        ));
    }

    let sources: Vec<PathBuf> = update::siblings(&source).into_iter().filter(|p| p.is_file()).collect();
    if sources.is_empty() {
        return Err(format!("{} is not a Gazelle binary to install", source.display()));
    }

    let console = dir.join(file_name_of(&source, CONSOLE_BINARY));
    let upgraded = update::siblings(&console).iter().any(|p| p.exists());

    // Checked for every destination before the first byte is written, so a refusal leaves the
    // installed copy exactly as it was rather than half replaced.
    for from in &sources {
        let to = dir.join(from.file_name().unwrap_or_default());
        if to.exists() && ctx.still_in_use(&to) {
            return Err(format!(
                "{} is running. Quit Gazelle first — right-click its tray icon and choose Quit — then run --install again.",
                to.display()
            ));
        }
    }

    std::fs::create_dir_all(dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    let mut copied = Vec::new();
    let mut bytes = 0u64;
    for from in &sources {
        let to = dir.join(from.file_name().unwrap_or_default());
        bytes += std::fs::copy(from, &to).map_err(|e| format!("copying {} to {}: {e}", from.display(), to.display()))?;
        copied.push(to);
    }

    // A `.exe.old` the in-app updater left between staging and a restart is clutter in a folder
    // we have just rewritten; the updater's own sweep is the same call.
    let swept = update::clean_up_after(&console);

    let launch = boot::boot_program(&console, |p| p.is_file());
    let size_kb = (bytes / 1024).max(1) as u32;
    let shortcut = write_shortcut(ctx, &launch, dir)?;
    write_uninstall_entry(ctx, dir, &console, &launch, size_kb)?;
    let boot = repoint_boot(ctx.run_key, &launch).map_err(|e| format!("re-pointing Start on boot: {e}"))?;

    Ok(Installed { dir: dir.clone(), copied, launch, shortcut, upgraded, swept, boot, size_kb })
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
    let r = ctx.registry;
    let set = |name: &str, value: &str| {
        r.set_string(UNINSTALL_KEY, name, value).map_err(|e| format!("writing HKCU\\{UNINSTALL_KEY}\\{name}: {e}"))
    };
    set("DisplayName", APP_NAME)?;
    set("DisplayVersion", crate::VERSION)?;
    set("Publisher", PUBLISHER)?;
    set("InstallLocation", &dir.display().to_string())?;
    // The uninstall is the console build: it prints what it removed and asks about the config
    // directory. The quiet form answers that question with the default (keep) and says nothing.
    set("UninstallString", &format!("\"{}\" --uninstall", console.display()))?;
    set("QuietUninstallString", &format!("\"{}\" --uninstall --yes", console.display()))?;
    // There is no icon resource in the binary yet (the tray's icon is drawn at run time), so
    // this is the executable's default icon. Naming it anyway means the entry has an icon the
    // moment the binary gains one, with no second release to change the registry.
    set("DisplayIcon", &format!("{},0", launch.display()))?;
    set("URLInfoAbout", ABOUT_URL)?;
    let set_u32 = |name: &str, value: u32| {
        r.set_u32(UNINSTALL_KEY, name, value).map_err(|e| format!("writing HKCU\\{UNINSTALL_KEY}\\{name}: {e}"))
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
/// Start on boot is a setting the person turned on (P78), and after an install the copy they
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

/// Remove the shortcut, the registry entry and the installed files — and nothing else.
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
                "{} is running. Quit Gazelle first — right-click its tray icon and choose Quit — then uninstall again.",
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

    ctx.registry.delete_tree(UNINSTALL_KEY).map_err(|e| format!("removing HKCU\\{UNINSTALL_KEY}: {e}"))?;

    let mut files = Vec::new();
    for path in ours {
        if remove_if_there(&path).map_err(|e| format!("removing {}: {e}", path.display()))? {
            files.push(path);
        }
    }
    // `remove_dir`, never `remove_dir_all`: a folder still holding something the person put
    // there stays, with what they put there still in it.
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

    Ok(Removed { dir: dir.to_path_buf(), files, shortcut, boot, dir_removed, purged, kept })
}

/// Where an install is, according to the Add/Remove Programs entry it wrote, falling back to
/// where this layout would have put it.
pub fn installed_dir(ctx: &Context) -> PathBuf {
    match ctx.registry.get_string(UNINSTALL_KEY, "InstallLocation") {
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
/// Windows to clean up, since it cannot delete itself either — the one thing this leaves behind.
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

// --- the real seams ---------------------------------------------------------------------------

/// Whether a binary at `path` is being run right now.
///
/// Windows locks a running image against writing, so asking for write access is the question:
/// a sharing violation means a process has it open as an image. Nothing else counts — a
/// read-only file, or a file that is not there, is not "running" — because this answer is what
/// refuses an install, and a wrong yes would be an app that cannot be updated.
pub fn image_in_use(path: &Path) -> bool {
    match std::fs::OpenOptions::new().write(true).open(path) {
        Ok(_) => false,
        // 32 is ERROR_SHARING_VIOLATION, 33 ERROR_LOCK_VIOLATION.
        Err(e) => matches!(e.raw_os_error(), Some(32) | Some(33)),
    }
}

// --- small shared pieces -----------------------------------------------------------------------

fn console_name() -> String {
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
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let text = path.to_string_lossy().into_owned();
    let text = text.strip_prefix(r"\\?\").unwrap_or(&text);
    text.split(['\\', '/']).filter(|c| !c.is_empty()).map(str::to_lowercase).collect()
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
