//! The portable self-install, end to end against a **temporary root**.
//!
//! Nothing here touches the real `%LOCALAPPDATA%\Programs\Gazelle`, the real Start Menu or the
//! real `HKCU` keys: the filesystem goes through a [`Layout`] built from a fake environment, and
//! the registry and the login entry go through the same traits the tray already uses, backed by
//! in-memory fakes.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use gazelle_audio_server::install::registry::Registry;
use gazelle_audio_server::install::{self, Context, Layout, Waiting};
use gazelle_audio_server::tray::boot::{RunKey, ENTRY_NAME};

/// A fresh folder under the system temp directory, removed again when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("gazelle-install-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// `(subkey, value name) -> (type, value)`. The type is kept so a test can tell a string from a
/// number: Add/Remove Programs reads `EstimatedSize` as a DWORD and shows nothing for a string.
type Values = BTreeMap<(String, String), (&'static str, String)>;

/// The registry, in memory.
#[derive(Clone, Default)]
struct FakeRegistry {
    values: std::rc::Rc<RefCell<Values>>,
}

impl FakeRegistry {
    fn get(&self, name: &str) -> Option<String> {
        self.values.borrow().get(&(install::UNINSTALL_KEY.to_string(), name.to_string())).map(|(_, v)| v.clone())
    }
    fn kind(&self, name: &str) -> Option<&'static str> {
        self.values.borrow().get(&(install::UNINSTALL_KEY.to_string(), name.to_string())).map(|(k, _)| *k)
    }
    fn is_empty(&self) -> bool {
        self.values.borrow().is_empty()
    }
}

impl Registry for FakeRegistry {
    fn set_string(&self, subkey: &str, name: &str, value: &str) -> io::Result<()> {
        self.values.borrow_mut().insert((subkey.into(), name.into()), ("sz", value.into()));
        Ok(())
    }
    fn set_u32(&self, subkey: &str, name: &str, value: u32) -> io::Result<()> {
        self.values.borrow_mut().insert((subkey.into(), name.into()), ("dword", value.to_string()));
        Ok(())
    }
    fn get_string(&self, subkey: &str, name: &str) -> io::Result<Option<String>> {
        Ok(self.values.borrow().get(&(subkey.into(), name.into())).map(|(_, v)| v.clone()))
    }
    fn delete_tree(&self, subkey: &str) -> io::Result<()> {
        self.values.borrow_mut().retain(|(k, _), _| k != subkey);
        Ok(())
    }
}

#[derive(Clone, Default)]
struct FakeRunKey {
    entries: std::rc::Rc<RefCell<BTreeMap<String, String>>>,
}

impl RunKey for FakeRunKey {
    fn read(&self, name: &str) -> io::Result<Option<String>> {
        Ok(self.entries.borrow().get(name).cloned())
    }
    fn write(&self, name: &str, command: &str) -> io::Result<()> {
        self.entries.borrow_mut().insert(name.into(), command.into());
        Ok(())
    }
    fn remove(&self, name: &str) -> io::Result<()> {
        self.entries.borrow_mut().remove(name);
        Ok(())
    }
}

/// Everything an install needs, pointed at one temporary root.
struct World {
    root: TempDir,
    layout: Layout,
    registry: FakeRegistry,
    run_key: FakeRunKey,
}

/// The environment a Windows user has, with every root inside `root`.
fn fake_env(root: &Path) -> impl Fn(&str) -> Option<String> + '_ {
    move |k| match k {
        "LOCALAPPDATA" => Some(root.join("Local").display().to_string()),
        "APPDATA" => Some(root.join("Roaming").display().to_string()),
        _ => None,
    }
}

impl World {
    fn new(name: &str) -> Self {
        let root = TempDir::new(name);
        let layout = Layout::from_env(fake_env(&root.0)).unwrap();
        Self { root, layout, registry: FakeRegistry::default(), run_key: FakeRunKey::default() }
    }

    /// A portable download: both binaries in a folder of their own, with known contents.
    fn portable(&self, body: &str) -> PathBuf {
        let dir = self.root.join(&format!("downloads-{body}"));
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["gazelle-audio-server.exe", "gazelle-audio-serverw.exe"] {
            std::fs::write(dir.join(name), format!("{name} {body}").repeat(64)).unwrap();
        }
        dir.join("gazelle-audio-server.exe")
    }

    fn context<'a>(&'a self, in_use: &'a dyn Fn(&Path) -> bool) -> Context<'a> {
        Context {
            layout: &self.layout,
            registry: &self.registry,
            run_key: &self.run_key,
            in_use,
            waiting: Waiting::none(),
        }
    }

    fn installed(&self, name: &str) -> PathBuf {
        self.layout.programs.join(name)
    }

    fn shortcut(&self) -> PathBuf {
        self.layout.start_menu.join(install::SHORTCUT_FILE)
    }
}

fn never(_: &Path) -> bool {
    false
}

// --- installing ---------------------------------------------------------------------------

#[test]
fn install_plants_both_binaries_a_shortcut_and_an_add_remove_entry() {
    let w = World::new("plant");
    let source = w.portable("v1");

    let report = install::install(&w.context(&never), &source).unwrap();

    assert_eq!(report.dir, w.layout.programs);
    assert!(!report.upgraded, "nothing was there before");
    for name in ["gazelle-audio-server.exe", "gazelle-audio-serverw.exe"] {
        assert_eq!(
            std::fs::read(w.installed(name)).unwrap(),
            std::fs::read(source.with_file_name(name)).unwrap(),
            "{name} was not copied"
        );
    }
    // The Start Menu shortcut runs the windowless build, so opening Gazelle flashes no console.
    assert_eq!(report.launch, w.installed("gazelle-audio-serverw.exe"));
    assert_eq!(report.shortcut, w.shortcut());
    let lnk = std::fs::read(w.shortcut()).unwrap();
    assert_eq!(&lnk[..4], &0x4Cu32.to_le_bytes(), "a shell link starts with its header size");
    let target = install::shortcut::target_of(&lnk).unwrap();
    assert_eq!(PathBuf::from(target), w.installed("gazelle-audio-serverw.exe"));

    let r = &w.registry;
    assert_eq!(r.get("DisplayName").as_deref(), Some("Gazelle"));
    assert_eq!(r.get("DisplayVersion").as_deref(), Some(env!("CARGO_PKG_VERSION")));
    assert_eq!(r.get("Publisher").as_deref(), Some(install::PUBLISHER));
    assert_eq!(r.get("InstallLocation").map(PathBuf::from), Some(w.layout.programs.clone()));
    assert_eq!(
        r.get("UninstallString").as_deref(),
        Some(format!("\"{}\" --uninstall", w.installed("gazelle-audio-server.exe").display()).as_str())
    );
    assert_eq!(
        r.get("QuietUninstallString").as_deref(),
        Some(format!("\"{}\" --uninstall --yes", w.installed("gazelle-audio-server.exe").display()).as_str())
    );
    assert_eq!(
        r.get("DisplayIcon").as_deref(),
        Some(format!("{},0", w.installed("gazelle-audio-serverw.exe").display()).as_str())
    );
    assert_eq!(r.get("URLInfoAbout").as_deref(), Some(install::ABOUT_URL));
    assert_eq!(r.kind("EstimatedSize"), Some("dword"), "Add/Remove Programs reads a DWORD of kilobytes");
    assert!(r.get("EstimatedSize").unwrap().parse::<u32>().unwrap() >= 1);
    assert_eq!(r.get("NoModify").as_deref(), Some("1"));
    assert_eq!(r.get("NoRepair").as_deref(), Some("1"));
}

#[test]
fn installing_again_upgrades_in_place_and_sweeps_what_an_update_left() {
    let w = World::new("upgrade");
    install::install(&w.context(&never), &w.portable("v1")).unwrap();
    // As the updater leaves things between staging and the next start.
    std::fs::write(w.installed("gazelle-audio-server.exe.old"), b"displaced").unwrap();

    let source = w.portable("v2");
    let report = install::install(&w.context(&never), &source).unwrap();

    assert!(report.upgraded, "an install over an existing one is an upgrade");
    assert_eq!(report.swept, 1, "the .old an update left is cleaned up");
    assert!(!w.installed("gazelle-audio-server.exe.old").exists());
    assert_eq!(
        std::fs::read(w.installed("gazelle-audio-server.exe")).unwrap(),
        std::fs::read(source).unwrap(),
        "the new binary replaced the old one"
    );
}

#[test]
fn install_refuses_to_replace_a_running_copy_and_says_how_to_quit_it() {
    let w = World::new("running");
    install::install(&w.context(&never), &w.portable("v1")).unwrap();
    let running = w.installed("gazelle-audio-serverw.exe");
    let busy = |p: &Path| p == running;

    let source = w.portable("v2");
    let error = install::install(&w.context(&busy), &source).unwrap_err();

    assert!(error.contains("gazelle-audio-serverw.exe"), "{error}");
    assert!(error.to_lowercase().contains("quit"), "the message must say how to stop it: {error}");
    assert_eq!(
        std::fs::read(w.installed("gazelle-audio-server.exe")).unwrap(),
        std::fs::read(w.portable("v1")).unwrap(),
        "a refused install must change nothing"
    );
}

#[test]
fn installing_the_copy_that_is_already_installed_is_refused() {
    let w = World::new("self");
    install::install(&w.context(&never), &w.portable("v1")).unwrap();
    let error = install::install(&w.context(&never), &w.installed("gazelle-audio-server.exe")).unwrap_err();
    assert!(error.contains("already"), "{error}");
}

#[test]
fn install_points_start_on_boot_at_the_installed_windowless_binary() {
    let w = World::new("boot");
    // A login entry made by a portable run, pointing at wherever it was started from.
    let portable = w.portable("v1");
    w.run_key
        .write(ENTRY_NAME, &format!("\"{}\" --backend usb --bind 127.0.0.1:8420", portable.display()))
        .unwrap();

    let report = install::install(&w.context(&never), &portable).unwrap();

    let entry = w.run_key.read(ENTRY_NAME).unwrap().unwrap();
    assert_eq!(
        entry,
        format!("\"{}\" --backend usb --bind 127.0.0.1:8420", w.installed("gazelle-audio-serverw.exe").display()),
        "the entry must run the installed windowless binary, keeping the options it carried"
    );
    assert_eq!(report.boot.as_deref(), Some(entry.as_str()));
}

#[test]
fn reinstalling_leaves_a_login_entry_that_already_runs_the_installed_copy_alone() {
    let w = World::new("boot-same");
    install::install(&w.context(&never), &w.portable("v1")).unwrap();
    let entry = format!("\"{}\" --backend usb --dry-run", w.installed("gazelle-audio-serverw.exe").display());
    w.run_key.write(ENTRY_NAME, &entry).unwrap();

    let report = install::install(&w.context(&never), &w.portable("v2")).unwrap();

    assert_eq!(report.boot, None, "there was nothing to re-point");
    assert_eq!(w.run_key.read(ENTRY_NAME).unwrap().as_deref(), Some(entry.as_str()));
}

#[test]
fn install_does_not_create_a_login_entry_that_was_never_asked_for() {
    let w = World::new("no-boot");
    let report = install::install(&w.context(&never), &w.portable("v1")).unwrap();
    assert_eq!(report.boot, None);
    assert!(w.run_key.read(ENTRY_NAME).unwrap().is_none(), "Start on boot is a setting, not something an install turns on");
}

// --- uninstalling -------------------------------------------------------------------------

#[test]
fn uninstall_removes_what_it_installed_and_keeps_the_config_by_default() {
    let w = World::new("remove");
    install::install(&w.context(&never), &w.portable("v1")).unwrap();
    std::fs::write(w.installed("gazelle-audio-serverw.exe.old"), b"displaced").unwrap();
    // A workspace and a log, as a user would have.
    let config = w.layout.config.clone().unwrap();
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(config.join("workspace.json"), b"{}").unwrap();

    let removed = install::uninstall(&w.context(&never), &w.layout.programs, false).unwrap();

    assert!(!w.installed("gazelle-audio-server.exe").exists());
    assert!(!w.installed("gazelle-audio-serverw.exe").exists());
    assert!(!w.installed("gazelle-audio-serverw.exe.old").exists(), "a .old the updater left goes too");
    assert!(!w.layout.programs.exists(), "an install folder with nothing left in it goes");
    assert!(removed.dir_removed);
    assert!(!w.shortcut().exists());
    assert!(w.registry.is_empty(), "the Add/Remove Programs entry is gone");
    assert!(config.join("workspace.json").exists(), "the config directory is kept unless --purge");
    assert_eq!(removed.kept, vec![config]);
    assert!(removed.purged.is_empty());
}

#[test]
fn uninstall_purge_also_removes_the_config_and_the_logs() {
    let w = World::new("purge");
    install::install(&w.context(&never), &w.portable("v1")).unwrap();
    let (config, logs) = (w.layout.config.clone().unwrap(), w.layout.logs.clone().unwrap());
    for dir in [&config, &logs] {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("something"), b"x").unwrap();
    }

    let removed = install::uninstall(&w.context(&never), &w.layout.programs, true).unwrap();

    assert!(!config.exists(), "--purge removes the workspace, layouts, themes and snapshots");
    assert!(!logs.exists(), "and the logs");
    assert_eq!(removed.purged, vec![config, logs]);
    assert!(removed.kept.is_empty());
}

#[test]
fn uninstall_leaves_anything_it_did_not_put_there() {
    let w = World::new("strangers");
    install::install(&w.context(&never), &w.portable("v1")).unwrap();
    let stranger = w.layout.programs.join("notes.txt");
    std::fs::write(&stranger, b"mine").unwrap();
    // A file that shares the folder's parent but is not in it.
    let outside = w.layout.programs.parent().unwrap().join("someone-elses.exe");
    std::fs::write(&outside, b"not ours").unwrap();

    install::uninstall(&w.context(&never), &w.layout.programs, false).unwrap();

    assert!(stranger.exists(), "only the names the installer wrote are removed");
    assert!(w.layout.programs.exists(), "a folder that still holds something is left alone");
    assert!(outside.exists());
}

#[test]
fn uninstall_refuses_where_nothing_is_installed() {
    let w = World::new("nothing");
    let error = install::uninstall(&w.context(&never), &w.layout.programs, false).unwrap_err();
    assert!(error.contains("no Gazelle"), "{error}");
}

#[test]
fn uninstall_removes_a_login_entry_that_runs_the_installed_copy() {
    let w = World::new("boot-off");
    install::install(&w.context(&never), &w.portable("v1")).unwrap();
    w.run_key.write(ENTRY_NAME, &format!("\"{}\" --backend usb", w.installed("gazelle-audio-serverw.exe").display())).unwrap();

    let removed = install::uninstall(&w.context(&never), &w.layout.programs, false).unwrap();

    assert!(removed.boot);
    assert!(w.run_key.read(ENTRY_NAME).unwrap().is_none());
}

#[test]
fn uninstall_leaves_a_login_entry_that_runs_another_build() {
    let w = World::new("boot-other");
    install::install(&w.context(&never), &w.portable("v1")).unwrap();
    let other = r#""C:\somewhere\else\gazelle-audio-serverw.exe" --backend usb"#;
    w.run_key.write(ENTRY_NAME, other).unwrap();

    let removed = install::uninstall(&w.context(&never), &w.layout.programs, false).unwrap();

    assert!(!removed.boot);
    assert_eq!(w.run_key.read(ENTRY_NAME).unwrap().as_deref(), Some(other), "another install's entry is not ours to remove");
}

#[test]
fn uninstall_refuses_while_the_installed_copy_is_still_running() {
    let w = World::new("busy");
    install::install(&w.context(&never), &w.portable("v1")).unwrap();
    let busy = |_: &Path| true;

    let error = install::uninstall(&w.context(&busy), &w.layout.programs, false).unwrap_err();

    assert!(error.to_lowercase().contains("quit"), "{error}");
    assert!(w.installed("gazelle-audio-server.exe").exists(), "a refused uninstall removes nothing");
    assert!(!w.registry.is_empty(), "and leaves the Add/Remove Programs entry alone");
}

#[test]
fn a_relocated_uninstall_waits_for_the_binary_that_started_it_to_exit() {
    let w = World::new("wait");
    install::install(&w.context(&never), &w.portable("v1")).unwrap();
    // Busy for the first three looks, as the parent process is on its way out.
    let looks = Cell::new(0);
    let busy = |_: &Path| {
        looks.set(looks.get() + 1);
        looks.get() <= 3
    };
    let waiting = Waiting { attempts: 10, delay: Duration::ZERO };

    let mut context = w.context(&busy);
    context.waiting = waiting;
    install::uninstall(&context, &w.layout.programs, false).unwrap();

    assert!(!w.installed("gazelle-audio-server.exe").exists());
}

// --- removing the binary that is running ---------------------------------------------------

#[test]
fn an_uninstall_run_from_the_install_folder_copies_itself_out_first() {
    let dir = Path::new(r"C:\Users\u\AppData\Local\Programs\Gazelle");
    let temp = Path::new(r"C:\Temp");
    assert_eq!(
        install::relocation(&dir.join("gazelle-audio-server.exe"), dir, temp, 42),
        Some(temp.join("gazelle-uninstall-42.exe")),
        "the running image cannot delete itself, so it is copied out and re-run from there"
    );
    assert_eq!(
        install::relocation(Path::new(r"D:\portable\gazelle-audio-server.exe"), dir, temp, 42),
        None,
        "a binary run from anywhere else can remove the install folder as it is"
    );
}

#[test]
fn the_relocated_copy_is_told_what_to_remove_and_asks_nothing() {
    let dir = Path::new(r"C:\Users\u\AppData\Local\Programs\Gazelle");
    assert_eq!(
        install::relocated_arguments(dir, true),
        ["--uninstall", "--uninstall-target", r"C:\Users\u\AppData\Local\Programs\Gazelle", "--purge", "--yes"]
    );
    assert_eq!(
        install::relocated_arguments(dir, false),
        ["--uninstall", "--uninstall-target", r"C:\Users\u\AppData\Local\Programs\Gazelle", "--keep-config", "--yes"]
    );
}

// --- the roots ------------------------------------------------------------------------------

#[test]
fn the_layout_is_read_from_the_windows_environment() {
    let root = Path::new(r"C:\root");
    let layout = Layout::from_env(fake_env(root)).unwrap();
    assert_eq!(layout.programs, root.join("Local").join("Programs").join("Gazelle"));
    assert_eq!(layout.start_menu, root.join("Roaming").join("Microsoft/Windows/Start Menu/Programs".replace('/', "\\")));
    assert_eq!(layout.config, Some(root.join("Roaming").join("gazelle")), "where the workspace already lives");
    assert_eq!(layout.logs, Some(root.join("Local").join("gazelle").join("logs")), "where the tray already logs");
    assert_eq!(layout.uninstall_key, install::UNINSTALL_KEY, "Add/Remove Programs, unless a test says otherwise");
    let scratch = Layout::from_env(|k| match k {
        "GAZELLE_UNINSTALL_KEY" => Some(r"Software\somewhere\else".to_string()),
        other => fake_env(root)(other),
    })
    .unwrap();
    assert_eq!(scratch.uninstall_key, r"Software\somewhere\else");
    assert!(Layout::from_env(|_| None).is_err(), "with no LOCALAPPDATA there is nowhere per-user to install to");
}

// --- the whole thing, through the command line and the real registry ---------------------------

/// `--install` and `--uninstall` as a person runs them, with every root pointed at a temporary
/// folder and the Add/Remove Programs entry pointed — by [`install::UNINSTALL_KEY_VAR`] — at a
/// scratch key of this test's own. So the real `Layout`, the real `CurrentUser` registry code,
/// the real shortcut and the real file copying all run, and the user's installed-programs list,
/// Start Menu and Programs folder are never touched.
#[cfg(windows)]
#[test]
fn the_command_line_installs_and_uninstalls_for_real() {
    use gazelle_audio_server::install::registry::CurrentUser;

    let root = TempDir::new("cli");
    let key = format!(r"Software\gazelle-audio-test-cli-{}", std::process::id());
    let layout = Layout::from_env(|k| match k {
        "GAZELLE_UNINSTALL_KEY" => Some(key.clone()),
        other => fake_env(&root.0)(other),
    })
    .unwrap();
    // However this test ends, the scratch key goes.
    struct Scratch(String);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = CurrentUser.delete_tree(&self.0);
        }
    }
    let _scratch = Scratch(key.clone());

    let gazelle = |args: &[&str]| {
        std::process::Command::new(env!("CARGO_BIN_EXE_gazelle-audio-server"))
            .args(args)
            .env("LOCALAPPDATA", root.0.join("Local"))
            .env("APPDATA", root.0.join("Roaming"))
            .env("GAZELLE_UNINSTALL_KEY", &key)
            // An --install run never reaches the backend, but nothing that starts this binary
            // from a test is allowed to be one flag away from opening a device.
            .env(gazelle_audio_server::no_hardware::VAR, "1")
            .output()
            .unwrap()
    };

    let out = gazelle(&["--install", "--no-start"]);
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(out.status.success(), "{text}{}", String::from_utf8_lossy(&out.stderr));
    assert!(text.contains(&layout.programs.display().to_string()), "it says where it put things: {text}");
    for name in ["gazelle-audio-server.exe", "gazelle-audio-serverw.exe"] {
        assert!(layout.programs.join(name).is_file(), "{name} is not in {}", layout.programs.display());
    }
    assert!(layout.start_menu.join(install::SHORTCUT_FILE).is_file());
    assert_eq!(CurrentUser.get_string(&key, "DisplayName").unwrap().as_deref(), Some("Gazelle"));
    assert_eq!(
        CurrentUser.get_string(&key, "InstallLocation").unwrap().map(PathBuf::from),
        Some(layout.programs.clone()),
        "the real registry write went through"
    );

    // Run from the build directory rather than the installed copy, so this is the plain path and
    // not the relocating one; the installed copy removing itself is checked by hand.
    let out = gazelle(&["--uninstall", "--yes"]);
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(out.status.success(), "{text}{}", String::from_utf8_lossy(&out.stderr));
    assert!(!layout.programs.exists(), "{text}");
    assert!(!layout.start_menu.join(install::SHORTCUT_FILE).exists());
    assert_eq!(CurrentUser.get_string(&key, "DisplayName").unwrap(), None, "the entry is gone from the registry");
}

/// Uninstalling where nothing is installed says so and fails, rather than reporting success.
#[cfg(windows)]
#[test]
fn the_command_line_refuses_to_uninstall_nothing() {
    let root = TempDir::new("cli-nothing");
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_gazelle-audio-server"))
        .args(["--uninstall", "--yes", "--uninstall-target", root.0.to_str().unwrap()])
        .env("LOCALAPPDATA", root.0.join("Local"))
        .env("APPDATA", root.0.join("Roaming"))
        .env(gazelle_audio_server::no_hardware::VAR, "1")
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("no Gazelle is installed"), "{:?}", out);
}

/// The real registry code, on a scratch key: every operation the installer needs.
#[cfg(windows)]
#[test]
fn the_current_user_registry_writes_reads_and_removes() {
    use gazelle_audio_server::install::registry::CurrentUser;
    let key = format!(r"Software\gazelle-audio-test-registry-{}", std::process::id());

    CurrentUser.set_string(&key, "DisplayName", "Gazelle").unwrap();
    CurrentUser.set_u32(&key, "EstimatedSize", 12345).unwrap();
    assert_eq!(CurrentUser.get_string(&key, "DisplayName").unwrap().as_deref(), Some("Gazelle"));
    // A value of another type is an error rather than a quiet `None`: "there is no string there"
    // and "there is something else there" are different answers and worth telling apart.
    assert!(CurrentUser.get_string(&key, "EstimatedSize").is_err(), "a DWORD read as a string");
    assert_eq!(CurrentUser.get_string(&key, "NeverWritten").unwrap(), None);
    assert_eq!(CurrentUser.get_string(r"Software\gazelle-audio-no-such-key", "DisplayName").unwrap(), None);

    CurrentUser.delete_tree(&key).unwrap();
    assert_eq!(CurrentUser.get_string(&key, "DisplayName").unwrap(), None);
    CurrentUser.delete_tree(&key).unwrap();
}

// --- the shortcut, read by the shell itself ---------------------------------------------------

/// The `.lnk` is written as bytes rather than through COM, so the one thing worth proving is
/// that **Windows itself** reads those bytes as a shortcut to the right program. `WScript.Shell`
/// resolves it through the same `IShellLink` Explorer uses, so if this agrees, Explorer does.
#[cfg(windows)]
#[test]
fn the_shell_resolves_the_shortcut_the_installer_wrote() {
    let w = World::new("shell-lnk");
    // A real file to point at, since the shell may check what it resolves to.
    install::install(&w.context(&never), &w.portable("v1")).unwrap();
    let lnk = w.shortcut();

    let script = format!(
        "$s=(New-Object -ComObject WScript.Shell).CreateShortcut('{}'); \
         Write-Output $s.TargetPath; Write-Output $s.WorkingDirectory; Write-Output $s.Description",
        lnk.display()
    );
    let out = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .expect("powershell is how this machine asks the shell what a .lnk points at");
    let text = String::from_utf8_lossy(&out.stdout);
    let read: Vec<&str> = text.lines().map(str::trim).collect();
    // Compared as the places they name, not as spellings: under a temp directory with an 8.3
    // short name (CI's `C:\Users\RUNNER~1`) the shell gives the target back in long form and
    // the working directory as written, so equal strings would be the wrong question.
    let place = |path: PathBuf| std::fs::canonicalize(&path).unwrap_or(path);
    assert_eq!(
        read.first().map(PathBuf::from).map(place),
        Some(place(w.installed("gazelle-audio-serverw.exe"))),
        "the shell read: {text:?} / {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(read.get(1).map(PathBuf::from).map(place), Some(place(w.layout.programs.clone())), "{text:?}");
    assert_eq!(read.get(2).copied(), Some("Gazelle"), "{text:?}");
}

// --- the one thing only a real process can show ---------------------------------------------

/// The refusal above is driven by a fake. This is the real test of the seam behind it: a copy of
/// the server running from a folder, and `image_in_use` saying so. Only the child started here is
/// ever stopped, by PID.
#[cfg(windows)]
#[test]
fn a_running_binary_is_recognised_as_in_use() {
    let dir = TempDir::new("in-use");
    let exe = dir.join("gazelle-audio-server.exe");
    std::fs::copy(env!("CARGO_BIN_EXE_gazelle-audio-server"), &exe).unwrap();
    assert!(!install::image_in_use(&exe), "nothing is running it yet");
    assert!(!install::image_in_use(&dir.join("not-there.exe")), "a file that is not there is not in use");

    let mut child = std::process::Command::new(&exe)
        .args(["--bind", "127.0.0.1:0", "--no-persist", "--no-web-ui", "--no-tray", "--no-update"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let running = install::image_in_use(&exe);
    let _ = child.kill();
    let _ = child.wait();
    assert!(running, "a running image must be recognised, or an install would overwrite it");
}
