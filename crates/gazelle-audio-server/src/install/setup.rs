//! Setup mode: installing Gazelle by double-clicking one file.
//!
//! A release carries `Gazelle-Setup.exe`, which is the windowless build under a name a person
//! recognises. Double-clicked, it asks one question in a small dialog and installs itself with the
//! same code as `--install`, then starts the installed copy. No terminal, no zip.
//!
//! **When a run is in setup mode** ([`mode`]), only ever with no arguments:
//!
//! - the file's own name has `setup` in it ([`Mode::Named`]): the release's setup file. It always
//!   asks, whatever is or is not installed;
//! - or it is the windowless build started from outside the install folder while nothing is
//!   installed ([`Mode::FirstRun`]): someone double-clicked it in an unzipped release. It offers
//!   a third answer, to run from where it is without installing, and a person who chooses that is
//!   never asked again ([`remember_run_here`]).
//!
//! Any argument at all means an ordinary run: a login entry, a shortcut with options, the tests
//! and the release smoke test never see a dialog.
//!
//! **Everything that decides** is here, over the [`Host`] trait: what is installed, whether it
//! is running, the dialog, the install, starting the installed copy and remembering the answer.
//! So every branch is tested with a fake host, and the real one ([`from_windowless`]) is a thin
//! layer over the installer, the registry and the Win32 dialog in `dialog.rs`.

use std::path::{Path, PathBuf};

use semver::Version;
use serde::{Deserialize, Serialize};

/// Why this run is in setup mode.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    /// The file is called something with `setup` in it. It always asks.
    Named,
    /// The windowless build, started with no arguments from outside the install folder while
    /// nothing is installed. It may also run without installing, and remembers that.
    FirstRun,
}

/// Whether a file name marks the setup file: `setup` anywhere in its stem, in any case, so
/// `Gazelle-Setup.exe` and a browser's `Gazelle-Setup (1).exe` both count. The folder it is in
/// does not.
pub fn is_setup_name(exe: &Path) -> bool {
    exe.file_stem().is_some_and(|stem| stem.to_string_lossy().to_lowercase().contains("setup"))
}

/// The facts [`mode`] decides from.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Start {
    /// Started with no arguments at all.
    pub no_arguments: bool,
    /// [`is_setup_name`] of the running file.
    pub named: bool,
    /// The running file is inside the install folder: it is the installed copy.
    pub inside_install: bool,
    /// Something is installed.
    pub installed: bool,
    /// The person chose to run without installing, earlier ([`remember_run_here`]).
    pub declined: bool,
}

/// Whether this run is in setup mode, and why. `None` is an ordinary run.
pub fn mode(start: &Start) -> Option<Mode> {
    if !start.no_arguments {
        return None;
    }
    if start.named {
        return Some(Mode::Named);
    }
    if start.inside_install || start.installed || start.declined {
        return None;
    }
    Some(Mode::FirstRun)
}

/// What is installed now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Found {
    Nothing,
    /// An install, and its version when that could be read.
    Installed(Option<Version>),
}

/// One button in a setup dialog.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Choice {
    Install,
    Replace,
    InstallAgain,
    RunHere,
    Open,
    TryAgain,
    Cancel,
    Close,
}

impl Choice {
    /// The button's words.
    pub fn label(self) -> &'static str {
        match self {
            Choice::Install => "Install",
            Choice::Replace => "Replace",
            Choice::InstallAgain => "Install again",
            Choice::RunHere => "Run without installing",
            Choice::Open => "Open Gazelle",
            Choice::TryAgain => "Try again",
            Choice::Cancel => "Cancel",
            Choice::Close => "Close",
        }
    }
}

/// What kind of message a dialog is, which decides its icon.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Tone {
    Question,
    Warning,
    Error,
}

/// One dialog: its words and its buttons, the first of which is the default. Closing the dialog
/// without choosing is its last button, which is always [`Choice::Cancel`] or [`Choice::Close`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Question {
    pub text: String,
    pub choices: Vec<Choice>,
    pub tone: Tone,
}

impl Question {
    fn new(text: impl Into<String>, choices: &[Choice], tone: Tone) -> Question {
        Question { text: text.into(), choices: choices.to_vec(), tone }
    }

    /// What closing the dialog (Esc, or its close box) means.
    pub fn dismissed(&self) -> Choice {
        *self.choices.last().unwrap_or(&Choice::Cancel)
    }
}

/// The dialogs' title.
pub const TITLE: &str = "Gazelle";

/// The question when nothing is installed.
pub const INSTALL: &str = "Install Gazelle for your account? It goes in your user folder, needs no administrator rights, \
and adds Gazelle to the Start Menu.";

/// Added to [`INSTALL`] when running without installing is on offer.
pub const OR_RUN_HERE: &str = "You can also run it from here without installing. If you do, Gazelle will not ask again.";

/// Said when the installed copy is running and would have to be replaced.
pub const RUNNING: &str = "Gazelle is running, so it cannot be replaced yet. Quit it first: right-click the Gazelle icon \
in the notification area, next to the clock, and choose Quit. Then choose Try again.";

/// The first dialog, for what is installed and why this is setup mode.
pub fn first_question(mode: Mode, found: &Found, this: &Version) -> Question {
    match found {
        Found::Nothing => match mode {
            Mode::Named => Question::new(INSTALL, &[Choice::Install, Choice::Cancel], Tone::Question),
            Mode::FirstRun => Question::new(
                format!("{INSTALL}\n\n{OR_RUN_HERE}"),
                &[Choice::Install, Choice::RunHere, Choice::Cancel],
                Tone::Question,
            ),
        },
        Found::Installed(None) => Question::new(
            format!("Gazelle is already installed. Replace it with Gazelle {this}? Your settings and layouts are kept."),
            &[Choice::Replace, Choice::Cancel],
            Tone::Question,
        ),
        Found::Installed(Some(installed)) if installed < this => Question::new(
            format!("Gazelle {installed} is installed. Replace it with Gazelle {this}? Your settings and layouts are kept."),
            &[Choice::Replace, Choice::Cancel],
            Tone::Question,
        ),
        Found::Installed(Some(installed)) if installed == this => Question::new(
            format!("Gazelle {this} is already installed."),
            &[Choice::Open, Choice::InstallAgain, Choice::Cancel],
            Tone::Question,
        ),
        Found::Installed(Some(installed)) => Question::new(
            format!(
                "A newer Gazelle is already installed: version {installed}. This file is version {this}, \
                 so it will not replace it."
            ),
            &[Choice::Open, Choice::Cancel],
            Tone::Warning,
        ),
    }
}

/// The installed copy is running.
pub fn running_question() -> Question {
    Question::new(RUNNING, &[Choice::TryAgain, Choice::Cancel], Tone::Warning)
}

/// Something went wrong; `what` says what, `detail` is the installer's own message.
pub fn failed(what: &str, detail: &str) -> Question {
    Question::new(format!("{what}\n\n{detail}"), &[Choice::Close], Tone::Error)
}

pub const NOT_INSTALLED: &str = "Gazelle could not be installed.";
pub const INSTALLED_NOT_STARTED: &str = "Gazelle was installed, but it could not be started.";
pub const NOT_STARTED: &str = "Gazelle could not be started.";

/// Everything setup mode reads and does, so the decisions can be tested without a real install.
pub trait Host {
    /// What is installed, read afresh each time.
    fn found(&mut self) -> Found;
    /// Whether the installed copy is running, so it cannot be replaced.
    fn running(&mut self) -> bool;
    /// Show a dialog and wait for the answer.
    fn ask(&mut self, question: &Question) -> Choice;
    /// Install this file, as `--install` does, without asking anything.
    fn install(&mut self) -> Result<(), String>;
    /// Start the installed copy, or bring its window to the front if it is running.
    fn open(&mut self) -> Result<(), String>;
    /// Remember that this person runs Gazelle without installing it.
    fn remember_run_here(&mut self) -> Result<(), String>;
}

/// What the process does after setup mode.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Next {
    /// Stop: setup is done, or was cancelled.
    Exit,
    /// Carry on as the ordinary app, from where it is.
    RunHere,
}

/// Setup mode from start to finish.
pub fn run(host: &mut dyn Host, mode: Mode, this: &Version) -> Next {
    let found = host.found();
    let question = first_question(mode, &found, this);
    match host.ask(&question) {
        Choice::Install | Choice::Replace | Choice::InstallAgain => install_and_open(host),
        Choice::Open => {
            if let Err(e) = host.open() {
                host.ask(&failed(NOT_STARTED, &e));
            }
            Next::Exit
        }
        // Not being asked again is a convenience: a failure to remember it is no reason not to run.
        Choice::RunHere => {
            let _ = host.remember_run_here();
            Next::RunHere
        }
        Choice::TryAgain | Choice::Cancel | Choice::Close => Next::Exit,
    }
}

/// Wait for the installed copy to be quit, install over it, and start the result.
fn install_and_open(host: &mut dyn Host) -> Next {
    while host.running() {
        if host.ask(&running_question()) != Choice::TryAgain {
            return Next::Exit;
        }
    }
    if let Err(e) = host.install() {
        host.ask(&failed(NOT_INSTALLED, &e));
        return Next::Exit;
    }
    if let Err(e) = host.open() {
        host.ask(&failed(INSTALLED_NOT_STARTED, &e));
    }
    Next::Exit
}

// --- the remembered answer ----------------------------------------------------------------------

/// The file the "run without installing" answer is kept in, beside `update.json` in the config
/// directory. `None` when the environment names no config directory.
pub fn remembered_path(var: impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    crate::config::config_dir(var).map(|dir| dir.join("setup.json"))
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Remembered {
    /// The person chose to run without installing, so the windowless build does not offer to
    /// install again.
    run_without_installing: bool,
}

/// Whether the person chose, earlier, to run without installing. A missing or unreadable file is
/// "no": the worst that does is ask once more.
pub fn declined_before(path: &Path) -> bool {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Remembered>(&bytes).ok())
        .is_some_and(|r| r.run_without_installing)
}

/// Remember that this person runs without installing.
pub fn remember_run_here(path: &Path) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    }
    let bytes = serde_json::to_vec_pretty(&Remembered { run_without_installing: true }).map_err(|e| e.to_string())?;
    std::fs::write(path, bytes).map_err(|e| format!("writing {}: {e}", path.display()))
}

// --- the real host ------------------------------------------------------------------------------

/// Setup mode for the windowless build, when this run is one: `None` for an ordinary run, which
/// is every run with arguments, and every run anywhere but Windows.
#[cfg(windows)]
pub fn from_windowless() -> Option<Next> {
    use super::{image_in_use, installed_dir, is_inside, registry, Context, Layout, Waiting};

    if std::env::args_os().len() > 1 {
        return None;
    }
    let exe = std::env::current_exe().ok()?;
    let var = |k: &str| std::env::var(k).ok();
    let layout = Layout::from_env(var).ok()?;
    let registry = registry::CurrentUser;
    let run_key = crate::tray::user_run_key();
    let in_use: &dyn Fn(&Path) -> bool = &image_in_use;
    let ctx = Context { layout: &layout, registry: &registry, run_key: run_key.as_ref(), in_use, waiting: Waiting::none() };

    let dir = installed_dir(&ctx);
    let remembered = remembered_path(var);
    let start = Start {
        no_arguments: true,
        named: is_setup_name(&exe),
        inside_install: is_inside(&exe, &dir) || is_inside(&exe, &layout.programs),
        installed: !real::installed_files(&dir).is_empty(),
        declined: remembered.as_deref().is_some_and(declined_before),
    };
    let mode = mode(&start)?;
    let this = Version::parse(crate::VERSION).ok()?;
    let mut host = real::RealHost { ctx, exe, remembered };
    Some(run(&mut host, mode, &this))
}

/// Anywhere but Windows there is no installer, so there is no setup mode.
#[cfg(not(windows))]
pub fn from_windowless() -> Option<Next> {
    None
}

#[cfg(windows)]
mod real {
    use super::*;
    use crate::install::{console_name, install_windowless, installed_dir, start_installed, Context};
    use crate::tray::boot;
    use crate::update;

    pub struct RealHost<'a> {
        pub ctx: Context<'a>,
        pub exe: PathBuf,
        pub remembered: Option<PathBuf>,
    }

    /// The Gazelle binaries in an install folder.
    pub fn installed_files(dir: &Path) -> Vec<PathBuf> {
        update::siblings(&dir.join(console_name())).into_iter().filter(|p| p.is_file()).collect()
    }

    impl RealHost<'_> {
        fn dir(&self) -> PathBuf {
            installed_dir(&self.ctx)
        }

        /// The binary the Start Menu shortcut runs: the windowless build when it is there.
        fn launch(&self) -> PathBuf {
            boot::boot_program(&self.dir().join(console_name()), |p| p.is_file())
        }
    }

    impl Host for RealHost<'_> {
        fn found(&mut self) -> Found {
            if installed_files(&self.dir()).is_empty() {
                return Found::Nothing;
            }
            // The installed binary knows its own version; the Add/Remove Programs entry says what
            // was installed, which an in-app update since then has made out of date.
            let version = version_of(&self.launch()).or_else(|| {
                self.ctx
                    .registry
                    .get_string(&self.ctx.layout.uninstall_key, "DisplayVersion")
                    .ok()
                    .flatten()
                    .and_then(|v| Version::parse(v.trim()).ok())
            });
            Found::Installed(version)
        }

        fn running(&mut self) -> bool {
            installed_files(&self.dir()).iter().any(|p| (self.ctx.in_use)(p))
        }

        fn ask(&mut self, question: &Question) -> Choice {
            crate::install::dialog::ask(question)
        }

        fn install(&mut self) -> Result<(), String> {
            install_windowless(&self.ctx, &self.exe).map(|_| ())
        }

        fn open(&mut self) -> Result<(), String> {
            start_installed(&self.launch())
        }

        fn remember_run_here(&mut self) -> Result<(), String> {
            match &self.remembered {
                Some(path) => super::remember_run_here(path),
                None => Err("there is no settings folder to remember that in".into()),
            }
        }
    }

    /// What `<exe> --version` says, or `None` if it says nothing readable within a few seconds.
    /// It is our own installed binary, and `--version` does nothing but print.
    fn version_of(exe: &Path) -> Option<Version> {
        use std::os::windows::process::CommandExt;
        use std::process::{Command, Stdio};
        use std::time::{Duration, Instant};
        // CREATE_NO_WINDOW: a console build must not flash a console window over the dialog.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut child = Command::new(exe)
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .ok()?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
                // Our own child, by its handle.
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
            }
        }
        let mut out = String::new();
        std::io::Read::read_to_string(child.stdout.as_mut()?, &mut out).ok()?;
        parse_version_line(&out)
    }
}

/// The version in what `--version` prints: `gazelle-audio-server 1.0.0`.
pub fn parse_version_line(text: &str) -> Option<Version> {
    text.lines().next()?.split_whitespace().last().and_then(|v| Version::parse(v).ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    fn v(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    /// A scripted host: what is found, whether it is running (one answer per look, the last one
    /// repeating), the answers to give, and what each action returns. It records every dialog and
    /// every action, in order.
    struct Fake {
        found: Found,
        running: VecDeque<bool>,
        answers: VecDeque<Choice>,
        install: Result<(), String>,
        open: Result<(), String>,
        remember: Result<(), String>,
        asked: Vec<Question>,
        did: Vec<&'static str>,
    }

    impl Fake {
        fn new(found: Found, answers: &[Choice]) -> Fake {
            Fake {
                found,
                running: VecDeque::from([false]),
                answers: answers.iter().copied().collect(),
                install: Ok(()),
                open: Ok(()),
                remember: Ok(()),
                asked: Vec::new(),
                did: Vec::new(),
            }
        }
        fn running(mut self, looks: &[bool]) -> Fake {
            self.running = looks.iter().copied().collect();
            self
        }
        fn texts(&self) -> Vec<&str> {
            self.asked.iter().map(|q| q.text.as_str()).collect()
        }
    }

    impl Host for Fake {
        fn found(&mut self) -> Found {
            self.found.clone()
        }
        fn running(&mut self) -> bool {
            self.did.push("look");
            if self.running.len() > 1 {
                self.running.pop_front().unwrap()
            } else {
                self.running[0]
            }
        }
        fn ask(&mut self, question: &Question) -> Choice {
            self.asked.push(question.clone());
            let answer = self.answers.pop_front().expect("a dialog nobody scripted an answer for");
            assert!(question.choices.contains(&answer), "{answer:?} is not a button on {question:?}");
            answer
        }
        fn install(&mut self) -> Result<(), String> {
            self.did.push("install");
            self.install.clone()
        }
        fn open(&mut self) -> Result<(), String> {
            self.did.push("open");
            self.open.clone()
        }
        fn remember_run_here(&mut self) -> Result<(), String> {
            self.did.push("remember");
            self.remember.clone()
        }
    }

    const THIS: &str = "1.1.0";

    fn go(host: &mut Fake, mode: Mode) -> Next {
        run(host, mode, &v(THIS))
    }

    // --- when it is setup mode -------------------------------------------------------------

    #[test]
    fn the_setup_file_is_known_by_setup_in_its_own_name_in_any_case() {
        assert!(is_setup_name(Path::new(r"C:\Users\u\Downloads\Gazelle-Setup.exe")));
        assert!(is_setup_name(Path::new(r"C:\Users\u\Downloads\Gazelle-Setup (1).exe")));
        assert!(is_setup_name(Path::new("SETUP.EXE")));
        assert!(!is_setup_name(Path::new(r"C:\Users\u\Downloads\gazelle-audio-serverw.exe")));
        assert!(!is_setup_name(Path::new(r"C:\setup\gazelle-audio-serverw.exe")), "the folder's name does not count");
        assert!(!is_setup_name(Path::new(r"C:\apps\setup.d\gazelle.exe")), "nor does an extension");
    }

    #[test]
    fn any_argument_at_all_is_an_ordinary_run() {
        let start = Start { no_arguments: false, named: true, ..Start::default() };
        assert_eq!(mode(&start), None, "a login entry or the smoke test must never see a dialog");
        assert_eq!(mode(&Start { no_arguments: false, ..Start::default() }), None);
    }

    #[test]
    fn a_setup_named_file_always_asks() {
        for (inside_install, installed, declined) in
            [(false, false, false), (true, true, true), (false, true, false), (false, false, true)]
        {
            let start = Start { no_arguments: true, named: true, inside_install, installed, declined };
            assert_eq!(mode(&start), Some(Mode::Named), "{start:?}");
        }
    }

    #[test]
    fn the_windowless_build_asks_only_on_a_first_run_outside_an_install() {
        let first = Start { no_arguments: true, ..Start::default() };
        assert_eq!(mode(&first), Some(Mode::FirstRun));
        assert_eq!(mode(&Start { inside_install: true, ..first }), None, "the installed copy, from the Start Menu");
        assert_eq!(mode(&Start { installed: true, ..first }), None, "a portable copy beside an install just runs");
        assert_eq!(mode(&Start { declined: true, ..first }), None, "the person said to run without installing");
    }

    // --- nothing installed -----------------------------------------------------------------

    #[test]
    fn nothing_installed_asks_to_install_and_installing_starts_the_installed_copy() {
        let mut host = Fake::new(Found::Nothing, &[Choice::Install]);
        assert_eq!(go(&mut host, Mode::Named), Next::Exit);
        assert_eq!(host.asked[0].text, INSTALL);
        assert_eq!(host.asked[0].choices, [Choice::Install, Choice::Cancel]);
        assert_eq!(host.did, ["look", "install", "open"]);
        assert_eq!(host.asked.len(), 1, "a successful install says nothing more: the window opening is the answer");
    }

    #[test]
    fn cancel_does_nothing_at_all() {
        let mut host = Fake::new(Found::Nothing, &[Choice::Cancel]);
        assert_eq!(go(&mut host, Mode::Named), Next::Exit);
        assert!(host.did.is_empty());
    }

    #[test]
    fn a_first_run_also_offers_to_run_without_installing_and_remembers_it() {
        let mut host = Fake::new(Found::Nothing, &[Choice::RunHere]);
        assert_eq!(go(&mut host, Mode::FirstRun), Next::RunHere);
        assert_eq!(host.asked[0].choices, [Choice::Install, Choice::RunHere, Choice::Cancel]);
        assert!(host.asked[0].text.starts_with(INSTALL) && host.asked[0].text.ends_with(OR_RUN_HERE));
        assert_eq!(host.did, ["remember"]);
    }

    #[test]
    fn running_without_installing_still_runs_when_the_answer_cannot_be_remembered() {
        let mut host = Fake::new(Found::Nothing, &[Choice::RunHere]);
        host.remember = Err("read-only".into());
        assert_eq!(go(&mut host, Mode::FirstRun), Next::RunHere);
        assert_eq!(host.asked.len(), 1, "nothing more to say: the app opens");
    }

    #[test]
    fn a_first_run_cancelled_is_not_remembered_and_asks_again_next_time() {
        let mut host = Fake::new(Found::Nothing, &[Choice::Cancel]);
        assert_eq!(go(&mut host, Mode::FirstRun), Next::Exit);
        assert!(host.did.is_empty());
    }

    #[test]
    fn a_first_run_can_install_too() {
        let mut host = Fake::new(Found::Nothing, &[Choice::Install]);
        assert_eq!(go(&mut host, Mode::FirstRun), Next::Exit);
        assert_eq!(host.did, ["look", "install", "open"]);
    }

    // --- something installed ---------------------------------------------------------------

    #[test]
    fn an_older_install_is_offered_for_replacing_naming_both_versions() {
        let mut host = Fake::new(Found::Installed(Some(v("1.0.0"))), &[Choice::Replace]);
        assert_eq!(go(&mut host, Mode::Named), Next::Exit);
        assert_eq!(
            host.asked[0].text,
            "Gazelle 1.0.0 is installed. Replace it with Gazelle 1.1.0? Your settings and layouts are kept."
        );
        assert_eq!(host.asked[0].choices, [Choice::Replace, Choice::Cancel]);
        assert_eq!(host.did, ["look", "install", "open"]);
    }

    #[test]
    fn an_install_of_unknown_version_is_offered_for_replacing() {
        let mut host = Fake::new(Found::Installed(None), &[Choice::Cancel]);
        go(&mut host, Mode::Named);
        assert_eq!(host.asked[0].choices, [Choice::Replace, Choice::Cancel]);
        assert!(host.asked[0].text.contains("already installed") && host.asked[0].text.contains("1.1.0"));
        assert!(host.did.is_empty());
    }

    #[test]
    fn the_same_version_offers_to_open_it_or_install_it_again() {
        let mut host = Fake::new(Found::Installed(Some(v(THIS))), &[Choice::Open]);
        assert_eq!(go(&mut host, Mode::Named), Next::Exit);
        assert_eq!(host.asked[0].text, "Gazelle 1.1.0 is already installed.");
        assert_eq!(host.asked[0].choices, [Choice::Open, Choice::InstallAgain, Choice::Cancel]);
        assert_eq!(host.did, ["open"], "opening installs nothing");

        let mut host = Fake::new(Found::Installed(Some(v(THIS))), &[Choice::InstallAgain]);
        go(&mut host, Mode::Named);
        assert_eq!(host.did, ["look", "install", "open"]);
    }

    #[test]
    fn a_newer_install_is_never_replaced_only_opened() {
        let mut host = Fake::new(Found::Installed(Some(v("1.2.0"))), &[Choice::Open]);
        assert_eq!(go(&mut host, Mode::Named), Next::Exit);
        let question = &host.asked[0];
        assert_eq!(
            question.text,
            "A newer Gazelle is already installed: version 1.2.0. This file is version 1.1.0, so it will not replace it."
        );
        assert_eq!(question.choices, [Choice::Open, Choice::Cancel], "there is no button that goes back a version");
        assert_eq!(host.did, ["open"]);

        // A pre-release of this version is older than it, and a patch newer is newer.
        assert_eq!(first_question(Mode::Named, &Found::Installed(Some(v("1.1.0-rc.1"))), &v(THIS)).choices[0], Choice::Replace);
        assert_eq!(first_question(Mode::Named, &Found::Installed(Some(v("1.1.1"))), &v(THIS)).choices[0], Choice::Open);
    }

    #[test]
    fn opening_that_fails_says_so() {
        let mut host = Fake::new(Found::Installed(Some(v("2.0.0"))), &[Choice::Open, Choice::Close]);
        host.open = Err("the file is gone".into());
        go(&mut host, Mode::Named);
        assert_eq!(host.texts()[1], "Gazelle could not be started.\n\nthe file is gone");
        assert_eq!(host.asked[1].tone, Tone::Error);
    }

    // --- running, and failing ----------------------------------------------------------------

    #[test]
    fn a_running_copy_is_quit_by_the_person_then_replaced() {
        let mut host = Fake::new(Found::Installed(Some(v("1.0.0"))), &[Choice::Replace, Choice::TryAgain, Choice::TryAgain])
            .running(&[true, true, false]);
        assert_eq!(go(&mut host, Mode::Named), Next::Exit);
        assert_eq!(host.texts()[1..], [RUNNING, RUNNING], "asked until it is quit");
        assert_eq!(host.asked[1].choices, [Choice::TryAgain, Choice::Cancel]);
        assert_eq!(host.did, ["look", "look", "look", "install", "open"]);
    }

    #[test]
    fn a_running_copy_and_cancel_changes_nothing() {
        let mut host =
            Fake::new(Found::Installed(Some(v("1.0.0"))), &[Choice::Replace, Choice::Cancel]).running(&[true]);
        assert_eq!(go(&mut host, Mode::Named), Next::Exit);
        assert_eq!(host.did, ["look"], "nothing installed over a running copy");
    }

    #[test]
    fn an_install_that_fails_shows_the_reason_and_starts_nothing() {
        let mut host = Fake::new(Found::Nothing, &[Choice::Install, Choice::Close]);
        host.install = Err("the disk is full".into());
        assert_eq!(go(&mut host, Mode::Named), Next::Exit);
        assert_eq!(host.texts()[1], "Gazelle could not be installed.\n\nthe disk is full");
        assert_eq!(host.asked[1].choices, [Choice::Close]);
        assert_eq!(host.did, ["look", "install"]);
    }

    #[test]
    fn an_install_that_cannot_start_the_installed_copy_says_it_is_installed() {
        let mut host = Fake::new(Found::Nothing, &[Choice::Install, Choice::Close]);
        host.open = Err("blocked".into());
        go(&mut host, Mode::Named);
        assert_eq!(host.texts()[1], "Gazelle was installed, but it could not be started.\n\nblocked");
    }

    // --- the words -------------------------------------------------------------------------

    #[test]
    fn every_dialog_is_in_plain_words_with_no_dashes_and_a_way_out() {
        let this = v(THIS);
        let mut questions = vec![running_question(), failed(NOT_INSTALLED, "x"), failed(INSTALLED_NOT_STARTED, "x")];
        for mode in [Mode::Named, Mode::FirstRun] {
            for found in [Found::Nothing, Found::Installed(None), Found::Installed(Some(v("1.0.0"))), Found::Installed(Some(v(THIS))), Found::Installed(Some(v("9.0.0")))] {
                questions.push(first_question(mode, &found, &this));
            }
        }
        for question in &questions {
            let words = format!("{} {}", question.text, question.choices.iter().map(|c| c.label()).collect::<Vec<_>>().join(" "));
            // U+2013 and U+2014, by number, so this line does not hold what it forbids.
            assert!(!words.chars().any(|c| (0x2013..=0x2014).contains(&(c as u32))), "no en or em dash: {words}");
            assert!(!words.contains("--"), "no command-line talk: {words}");
            assert!(matches!(question.dismissed(), Choice::Cancel | Choice::Close), "{question:?}");
        }
    }

    #[test]
    fn the_version_is_read_from_what_version_prints() {
        assert_eq!(parse_version_line("gazelle-audio-server 1.0.0\n"), Some(v("1.0.0")));
        assert_eq!(parse_version_line("gazelle-audio-server 1.1.0-rc.1\r\n"), Some(v("1.1.0-rc.1")));
        assert_eq!(parse_version_line(""), None);
        assert_eq!(parse_version_line("error: unexpected argument"), None);
    }

    // --- remembering -----------------------------------------------------------------------

    #[test]
    fn run_without_installing_is_remembered_beside_the_update_settings() {
        let dir = std::env::temp_dir().join(format!("gazelle-setup-remember-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let appdata = dir.display().to_string();
        let path = remembered_path(|k| (k == "APPDATA").then(|| appdata.clone())).unwrap();
        assert_eq!(path, dir.join("gazelle").join("setup.json"));
        assert_eq!(path.parent(), crate::update::settings::default_settings_path(|k| (k == "APPDATA").then(|| appdata.clone())).parent());

        assert!(!declined_before(&path), "nothing remembered yet");
        remember_run_here(&path).unwrap();
        assert!(declined_before(&path));
        std::fs::write(&path, b"{not json").unwrap();
        assert!(!declined_before(&path), "an unreadable file means asking again, not never asking");
        std::fs::write(&path, br#"{"run_without_installing":false}"#).unwrap();
        assert!(!declined_before(&path));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(remembered_path(|_| None), None);
    }
}
