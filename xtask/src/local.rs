//! `xtask install-local`: build this checkout the way a release is built, and install it here.
//!
//! The by-hand version of this was one long line that built the web app and the server and ran
//! `--install --start`, and it had drifted from what a release is: it built no Gazelle Aggregate
//! driver, so the installed copy carried none; it baked in no update key, so the installed copy's
//! updater went quiet; and it needed Gazelle quit by hand first, because `--install` will not
//! replace a copy that is running. This is that line, with those three put right, in one place.
//!
//! In order:
//!
//! 1. the web app, `pnpm install --frozen-lockfile` then `pnpm build` (skip with `--skip-web`);
//! 2. the aggregate driver, on its own, **before** the server, because the server's build script
//!    reads the driver while it compiles and one `cargo build` naming both would not promise that
//!    order;
//! 3. a copy of that driver set aside in `target/install-local`, because building the server
//!    builds the driver's crate again as a dependency, with the server's features, and rewrites the
//!    file in `target/release` after the server has read it; the copy is what is embedded and what
//!    the last step checks against;
//! 4. the server, `--release --features window`, with `GAZELLE_AGGREGATE_DLL` naming that copy and
//!    `GAZELLE_UPDATE_PUBKEY` set when a key can be had (below);
//! 5. any Gazelle running from the install folder is stopped, **by its process id**, found by its
//!    executable's exact path, never by name, and waited out until Windows lets go of the file;
//! 6. the console build's `--install --start`;
//! 7. a check that the driver the installed copy wrote beside itself is the one just built.
//!
//! **The update key** is the public half of the release signing key, so there is nothing secret
//! about it. It comes from `--pubkey`, else `GAZELLE_UPDATE_PUBKEY`, else the repository variable
//! the release workflow reads, through `gh variable get`. With none of them the build still goes
//! ahead, and says plainly that the installed copy will not update itself until a release is
//! installed over it.
//!
//! **The one launch that is meant to reach the hardware.** Everything cargo starts in this
//! repository runs with `GAZELLE_NO_HARDWARE=1` (see `.cargo/config.toml`), this helper included,
//! and a child inherits it. So the installed Gazelle this starts would inherit it too, refuse its
//! own interfaces and exit. Starting the real app on purpose is the whole point of this command,
//! so that one launch, and only that one, has the variable taken away.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use gazelle_audio_server::install;
use gazelle_audio_server::no_hardware;
use gazelle_audio_server::update::BINARIES;

use crate::smoke::DRIVER;

/// The variable the server's build script embeds the driver from.
pub const DRIVER_VAR: &str = "GAZELLE_AGGREGATE_DLL";
/// The variable the server's build script bakes the update key from.
pub const PUBKEY_VAR: &str = "GAZELLE_UPDATE_PUBKEY";
/// How long to wait for a stopped Gazelle to let go of its executable.
const LET_GO: Duration = Duration::from_secs(15);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Options {
    /// Leave the web app as it is: for a change that is Rust only.
    pub skip_web: bool,
    /// Say what would happen, and do none of it.
    pub dry_run: bool,
    /// The update key to bake in, when given on the command line.
    pub pubkey: Option<String>,
}

/// One thing this command does, in order. A plan is data so it can be printed for `--dry-run`
/// and checked by the tests without building anything.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// Run a program in the repository root, with these variables set.
    Run { label: String, program: String, args: Vec<String>, env: Vec<(String, String)> },
    /// Copy a built file aside, where no later build writes.
    Stage { from: PathBuf, to: PathBuf },
    /// Stop whatever is running from the install folder.
    StopRunning { install_dir: PathBuf },
    /// Run the built console binary's `--install --start`, off the hardware backstop.
    Install { exe: PathBuf },
    /// Check the driver the installed copy wrote beside itself.
    CheckDriver { built: PathBuf, installed: PathBuf },
}

impl Step {
    /// What a person reads for this step, in `--dry-run` and as it happens.
    pub fn describe(&self) -> String {
        match self {
            Step::Run { label, program, args, env } => {
                let vars: String = env.iter().map(|(k, v)| format!("{k}={v} ")).collect();
                format!("{label}: {vars}{program} {}", args.join(" "))
            }
            Step::Stage { from, to } => format!("set the driver aside: {} to {}", from.display(), to.display()),
            Step::StopRunning { install_dir } => format!("stop any Gazelle running from {}, by process id", install_dir.display()),
            Step::Install { exe } => format!("install: {} --install --start (with {} taken away)", exe.display(), no_hardware::VAR),
            Step::CheckDriver { installed, .. } => format!("check {} is the driver just built", installed.display()),
        }
    }
}

/// The steps, in order. `root` is the repository, `install_dir` where Gazelle installs to,
/// `cargo` the cargo to build with, and `pubkey` the update key if one was found.
pub fn plan(root: &Path, install_dir: &Path, cargo: &str, pubkey: Option<&str>, options: &Options) -> Vec<Step> {
    let release = root.join("target").join("release");
    let driver = release.join(DRIVER);
    let staged = root.join("target").join("install-local").join(DRIVER);
    let run = |label: &str, program: &str, args: &[&str], env: Vec<(String, String)>| Step::Run {
        label: label.to_string(),
        program: program.to_string(),
        args: args.iter().map(|a| a.to_string()).collect(),
        env,
    };
    let mut steps = Vec::new();
    if !options.skip_web {
        // Through the shell on Windows: `corepack` is a `.cmd` there, which only a shell finds on
        // the path.
        let (shell, first): (&str, &[&'static str]) = if cfg!(windows) { ("cmd", &["/C", "corepack"]) } else { ("corepack", &[]) };
        let with = |rest: &[&'static str]| first.iter().chain(rest).copied().collect::<Vec<&'static str>>();
        steps.push(run("web dependencies", shell, &with(&["pnpm", "-C", "web", "install", "--frozen-lockfile"]), Vec::new()));
        steps.push(run("web app", shell, &with(&["pnpm", "-C", "web", "build"]), Vec::new()));
    }
    steps.push(run("aggregate driver", cargo, &["build", "--release", "-p", "gazelle-audio-aggregate"], Vec::new()));
    steps.push(Step::Stage { from: driver, to: staged.clone() });
    let mut env = vec![(DRIVER_VAR.to_string(), staged.display().to_string())];
    if let Some(key) = pubkey {
        env.push((PUBKEY_VAR.to_string(), key.to_string()));
    }
    steps.push(run("server", cargo, &["build", "--release", "-p", "gazelle-audio-server", "--features", "window"], env));
    steps.push(Step::StopRunning { install_dir: install_dir.to_path_buf() });
    steps.push(Step::Install { exe: release.join(format!("{}{}", BINARIES[0], std::env::consts::EXE_SUFFIX)) });
    steps.push(Step::CheckDriver { built: staged, installed: install_dir.join(DRIVER) });
    steps
}

/// The update key to bake in: the one given, else the environment's, else the repository
/// variable. A value that is not 64 hex digits is refused rather than baked into a binary that
/// could then never verify anything.
pub fn choose_pubkey(given: Option<&str>, from_env: Option<&str>, from_repository: impl FnOnce() -> Option<String>) -> Result<Option<String>, String> {
    let found = given
        .map(str::to_string)
        .or_else(|| from_env.map(str::to_string))
        .filter(|k| !k.trim().is_empty())
        .or_else(from_repository);
    match found.map(|k| k.trim().to_ascii_lowercase()) {
        Some(key) if key.len() == 64 && key.bytes().all(|b| b.is_ascii_hexdigit()) => Ok(Some(key)),
        Some(key) => Err(format!("the update key {key:?} is not 64 hex digits; it is the public half the release is signed with")),
        None => Ok(None),
    }
}

/// The processes whose executable sits in `dir`, from lines of `<pid>\t<path>`. Anything that is
/// not such a line is skipped, and so is a path outside `dir`, so a Gazelle run from anywhere
/// else, a build in `target` included, is never touched.
pub fn running_from(lines: &str, dir: &Path) -> Vec<(u32, PathBuf)> {
    lines
        .lines()
        .filter_map(|line| {
            let (pid, path) = line.trim().split_once('\t')?;
            let path = PathBuf::from(path.trim());
            let pid = pid.trim().parse().ok()?;
            install::is_inside(&path, dir).then_some((pid, path))
        })
        .collect()
}

/// The command that installs and starts the built Gazelle. The hardware backstop is taken away
/// for this one command, and only this one, because reaching the interfaces is its purpose.
pub fn install_command(exe: &Path) -> Command {
    let mut command = Command::new(exe);
    command.args(["--install", "--start"]).env_remove(no_hardware::VAR);
    command
}

/// The whole command.
pub fn install_local(options: &Options) -> Result<(), String> {
    if !cfg!(windows) {
        return Err("install-local installs Gazelle for this Windows user; there is nothing to install to here".into());
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().ok_or("xtask sits in the repository root")?.to_path_buf();
    let layout = install::Layout::from_env(|name| std::env::var(name).ok())?;
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let pubkey = choose_pubkey(options.pubkey.as_deref(), std::env::var(PUBKEY_VAR).ok().as_deref(), repository_pubkey)?;
    let steps = plan(&root, &layout.programs, &cargo, pubkey.as_deref(), options);

    if pubkey.is_none() {
        println!(
            "note: no update key was found (--pubkey, {PUBKEY_VAR}, or `gh variable get {PUBKEY_VAR}`), so the installed \
             copy will not update itself until a release is installed over it"
        );
    }
    if options.dry_run {
        for (at, step) in steps.iter().enumerate() {
            println!("{}. {}", at + 1, step.describe());
        }
        return Ok(());
    }
    for step in &steps {
        println!("==> {}", step.describe());
        perform(step, &root)?;
    }
    println!("installed and started; the Aggregate page will offer the driver in {}", layout.programs.display());
    Ok(())
}

fn perform(step: &Step, root: &Path) -> Result<(), String> {
    match step {
        Step::Run { label, program, args, env } => {
            let status = Command::new(program)
                .args(args)
                .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
                .current_dir(root)
                .status()
                .map_err(|e| format!("{label}: could not run {program}: {e}"))?;
            if status.success() {
                Ok(())
            } else {
                Err(format!("{label} failed ({status}); nothing was installed"))
            }
        }
        Step::Stage { from, to } => {
            if let Some(dir) = to.parent() {
                std::fs::create_dir_all(dir).map_err(|e| format!("making {}: {e}", dir.display()))?;
            }
            std::fs::copy(from, to).map(|_| ()).map_err(|e| format!("copying {} to {}: {e}", from.display(), to.display()))
        }
        Step::StopRunning { install_dir } => stop_running(install_dir),
        Step::Install { exe } => {
            if !exe.is_file() {
                return Err(format!("{} is missing; the server build should have made it", exe.display()));
            }
            let status = install_command(exe).current_dir(root).status().map_err(|e| format!("running {}: {e}", exe.display()))?;
            if status.success() {
                Ok(())
            } else {
                Err(format!("{} --install --start failed ({status})", exe.display()))
            }
        }
        Step::CheckDriver { built, installed } => {
            let (built_bytes, installed_bytes) = (std::fs::read(built), std::fs::read(installed));
            match (built_bytes, installed_bytes) {
                (Ok(a), Ok(b)) if a == b => {
                    println!("the installed driver is the one just built ({} bytes)", a.len());
                    Ok(())
                }
                (Ok(_), Ok(_)) => Err(format!(
                    "{} is not the driver just built. A DAW may still have the old one open: close it, and the next start of \
                     Gazelle puts the new one in place",
                    installed.display()
                )),
                (_, Err(e)) => Err(format!("{} was not written ({e}); the Aggregate page says why", installed.display())),
                (Err(e), _) => Err(format!("reading {}: {e}", built.display())),
            }
        }
    }
}

/// Stop every Gazelle running from `dir`, by process id, and wait until Windows has let go of the
/// executables, since `--install` refuses to replace one that is still in use.
fn stop_running(dir: &Path) -> Result<(), String> {
    let listed = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Get-CimInstance Win32_Process | Where-Object { $_.ExecutablePath } | \
             ForEach-Object { \"$($_.ProcessId)`t$($_.ExecutablePath)\" }",
        ])
        .output()
        .map_err(|e| format!("listing processes: {e}"))?;
    let running = running_from(&String::from_utf8_lossy(&listed.stdout), dir);
    if running.is_empty() {
        println!("    nothing running from there");
        return Ok(());
    }
    for (pid, path) in &running {
        println!("    stopping {} (process {pid})", path.display());
        let stopped = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/F"])
            .output()
            .map_err(|e| format!("stopping process {pid}: {e}"))?;
        if !stopped.status.success() {
            return Err(format!(
                "process {pid} ({}) would not stop: {}",
                path.display(),
                String::from_utf8_lossy(&stopped.stderr).trim()
            ));
        }
    }
    let since = Instant::now();
    let held = |path: &PathBuf| install::image_in_use(path);
    while running.iter().any(|(_, path)| held(path)) {
        if since.elapsed() > LET_GO {
            return Err(format!("Windows is still holding Gazelle's files after {} seconds; try again", LET_GO.as_secs()));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Ok(())
}

/// The update key the release workflow reads, through `gh`. Nothing found is not a failure.
fn repository_pubkey() -> Option<String> {
    let run = Command::new("gh").args(["variable", "get", PUBKEY_VAR]).output().ok()?;
    run.status.success().then(|| String::from_utf8_lossy(&run.stdout).trim().to_string()).filter(|k| !k.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "6ed1f34fb363edf80bf0b0279cad7548671a9e2e9f1f5456d4ece59fe956f03f";

    fn root() -> PathBuf {
        PathBuf::from(r"C:\repo")
    }

    fn installed() -> PathBuf {
        PathBuf::from(r"C:\Users\u\AppData\Local\Programs\Gazelle")
    }

    fn labels(steps: &[Step]) -> Vec<String> {
        steps
            .iter()
            .map(|step| match step {
                Step::Run { label, .. } => label.clone(),
                Step::Stage { .. } => "stage".into(),
                Step::StopRunning { .. } => "stop".into(),
                Step::Install { .. } => "install".into(),
                Step::CheckDriver { .. } => "check".into(),
            })
            .collect()
    }

    #[test]
    fn the_driver_is_built_on_its_own_and_set_aside_before_the_server_which_then_carries_it() {
        let steps = plan(&root(), &installed(), "cargo", Some(KEY), &Options::default());
        assert_eq!(labels(&steps), ["web dependencies", "web app", "aggregate driver", "stage", "server", "stop", "install", "check"]);
        let staged = root().join("target").join("install-local").join(DRIVER);
        let Step::Stage { from, to } = &steps[3] else { panic!("the driver is set aside") };
        assert_eq!((from, to), (&root().join("target").join("release").join(DRIVER), &staged));
        let Step::Run { args, env, .. } = &steps[4] else { panic!("the server is a build") };
        assert!(args.windows(2).any(|w| w == ["--features", "window"]), "the shipped build: {args:?}");
        assert!(env.contains(&(DRIVER_VAR.to_string(), staged.display().to_string())), "it embeds the copy the server build cannot rewrite: {env:?}");
        assert!(env.contains(&(PUBKEY_VAR.to_string(), KEY.to_string())), "and the update key: {env:?}");
    }

    #[test]
    fn a_rust_only_change_can_leave_the_web_app_alone_and_no_key_means_none_is_baked_in() {
        let steps = plan(&root(), &installed(), "cargo", None, &Options { skip_web: true, ..Options::default() });
        assert_eq!(labels(&steps), ["aggregate driver", "stage", "server", "stop", "install", "check"]);
        let Step::Run { env, .. } = &steps[2] else { panic!("the server is a build") };
        assert!(env.iter().all(|(k, _)| k != PUBKEY_VAR), "no key is invented: {env:?}");
    }

    #[test]
    fn the_install_is_the_console_build_and_the_check_looks_in_the_install_folder() {
        let steps = plan(&root(), &installed(), "cargo", None, &Options::default());
        let Step::Install { exe } = &steps[6] else { panic!("install") };
        assert_eq!(exe.file_stem().and_then(|s| s.to_str()), Some("gazelle-audio-server"), "the console one prints what it did");
        let Step::CheckDriver { built, installed: at } = &steps[7] else { panic!("check") };
        assert_eq!(at, &installed().join(DRIVER));
        assert_eq!(built, &root().join("target").join("install-local").join(DRIVER), "checked against what was embedded");
    }

    /// Everything cargo starts runs with the backstop set, this helper included, and a child
    /// inherits it; the installed Gazelle would then refuse its own interfaces. This launch is the
    /// one that is meant to reach them, so it, and only it, has the variable taken away.
    #[test]
    fn the_one_launch_meant_to_reach_the_hardware_has_the_backstop_taken_away() {
        let command = install_command(Path::new(r"C:\repo\target\release\gazelle-audio-server.exe"));
        let args: Vec<_> = command.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(args, ["--install", "--start"]);
        let removed = command.get_envs().any(|(k, v)| k == no_hardware::VAR && v.is_none());
        assert!(removed, "{} is removed for this one launch", no_hardware::VAR);
        for step in plan(&root(), &installed(), "cargo", None, &Options::default()) {
            if let Step::Run { env, label, .. } = step {
                assert!(env.iter().all(|(k, _)| k != no_hardware::VAR), "no build step touches the backstop: {label}");
            }
        }
    }

    #[test]
    fn only_a_gazelle_running_from_the_install_folder_is_ever_stopped() {
        let listed = format!(
            "1200\t{}\\gazelle-audio-serverw.exe\n\
             1300\tC:\\repo\\target\\release\\gazelle-audio-server.exe\n\
             1400\tC:\\Windows\\explorer.exe\n\
             not a line at all\n\
             abc\tC:\\Users\\u\\AppData\\Local\\Programs\\Gazelle\\x.exe\n\
             1500\tC:\\Users\\u\\AppData\\Local\\Programs\\Gazelle-other\\gazelle-audio-server.exe\n",
            installed().display()
        );
        let found = running_from(&listed, &installed());
        assert_eq!(found, [(1200, installed().join("gazelle-audio-serverw.exe"))], "a build in target and a lookalike folder are left alone");
    }

    #[test]
    fn the_update_key_comes_from_the_flag_then_the_environment_then_the_repository() {
        let never = || -> Option<String> { panic!("the repository is only asked when nothing nearer answered") };
        assert_eq!(choose_pubkey(Some(KEY), Some("ignored"), never).unwrap().as_deref(), Some(KEY));
        assert_eq!(choose_pubkey(None, Some(KEY), never).unwrap().as_deref(), Some(KEY));
        assert_eq!(choose_pubkey(None, None, || Some(KEY.to_uppercase())).unwrap().as_deref(), Some(KEY), "any case, stored lower");
        assert_eq!(choose_pubkey(None, Some("  "), || None).unwrap(), None, "a blank variable is no key");
        assert_eq!(choose_pubkey(None, None, || None).unwrap(), None);
        assert!(choose_pubkey(Some("abc"), None, || None).unwrap_err().contains("64 hex digits"));
    }

    #[test]
    fn a_dry_run_says_every_step_in_words() {
        let steps = plan(&root(), &installed(), "cargo", Some(KEY), &Options { dry_run: true, ..Options::default() });
        let said: Vec<String> = steps.iter().map(Step::describe).collect();
        assert!(said[3].starts_with("set the driver aside"), "{}", said[3]);
        assert!(said[4].starts_with("server: GAZELLE_AGGREGATE_DLL="), "{}", said[4]);
        assert!(said[5].contains("by process id"), "{}", said[5]);
        assert!(said[6].contains(no_hardware::VAR), "it says what the install does differently: {}", said[6]);
    }
}
