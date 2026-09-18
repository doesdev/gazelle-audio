//! Start on boot: a per-user login entry that runs this server again.
//!
//! On Windows that is a value under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, a user
//! preference rather than a security setting. The key sits behind [`RunKey`] so everything here
//! is tested against an in-memory fake; no test touches the real key.

use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// The name of the login entry.
pub const ENTRY_NAME: &str = "Gazelle";

/// What `--backend` does without being asked (decision `0018`), and so what a boot entry can
/// leave unsaid.
pub const DEFAULT_BACKEND: &str = "usb";

/// Where login entries live: read, write and remove one command line by name.
pub trait RunKey {
    fn read(&self, name: &str) -> io::Result<Option<String>>;
    fn write(&self, name: &str, command: &str) -> io::Result<()>;
    fn remove(&self, name: &str) -> io::Result<()>;
}

/// No login entries at all: what a platform with no such concept offers, so the rest of the
/// code has one shape rather than two.
pub struct NoRunKey;

impl RunKey for NoRunKey {
    fn read(&self, _name: &str) -> io::Result<Option<String>> {
        Ok(None)
    }
    fn write(&self, _name: &str, _command: &str) -> io::Result<()> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "there are no login entries on this platform"))
    }
    fn remove(&self, _name: &str) -> io::Result<()> {
        Ok(())
    }
}

/// The arguments a boot entry carries over from the running server.
///
/// Carried: what decides what the server serves and how safely (`--bind`, `--backend` when it is
/// not the default,
/// `--dry-run`, `--workspace`, `--themes-dir`, `--no-web-ui`, and for the loopback backend its
/// models and cyclic interval) and where it keeps its record, `--log-dir`. Not carried: `--no-persist`, which is for throwaway runs (a server
/// that starts at every login and forgets the user's layouts at every logoff is not what anyone
/// is asking for), and `--no-tray`, which cannot be set on a server that has a tray to click.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BootArgs {
    pub bind: SocketAddr,
    /// `loopback` or `usb`, as `--backend` spells it.
    pub backend: String,
    pub dry_run: bool,
    pub workspace: Option<PathBuf>,
    pub themes_dir: Option<PathBuf>,
    pub log_dir: Option<PathBuf>,
    pub loopback_models: Vec<String>,
    pub loopback_cyclic_ms: Option<u64>,
    pub no_web_ui: bool,
}

impl BootArgs {
    /// The same arguments with relative paths resolved against `cwd`. A login entry starts in a
    /// directory of the system's choosing, so a path relative to where the user ran the server
    /// would silently point somewhere else.
    pub fn absolute(self, cwd: &Path) -> Self {
        let resolve = |p: Option<PathBuf>| p.map(|p| if p.is_absolute() { p } else { cwd.join(p) });
        Self {
            workspace: resolve(self.workspace),
            themes_dir: resolve(self.themes_dir),
            log_dir: resolve(self.log_dir),
            ..self
        }
    }

    /// The argument list, in a fixed order.
    pub fn arguments(&self) -> Vec<String> {
        let mut args = Vec::new();
        // `usb` is the default (decision `0018`), so the ordinary desktop entry names no backend
        // at all; only a run that is deliberately on the emulator has to say so.
        if self.backend != DEFAULT_BACKEND {
            args.extend(["--backend".to_string(), self.backend.clone()]);
        }
        args.extend(["--bind".to_string(), self.bind.to_string()]);
        if self.dry_run {
            args.push("--dry-run".into());
        }
        if let Some(path) = &self.workspace {
            args.extend(["--workspace".into(), path.display().to_string()]);
        }
        if let Some(path) = &self.themes_dir {
            args.extend(["--themes-dir".into(), path.display().to_string()]);
        }
        if let Some(path) = &self.log_dir {
            args.extend(["--log-dir".into(), path.display().to_string()]);
        }
        if self.no_web_ui {
            args.push("--no-web-ui".into());
        }
        // The loopback options mean nothing to the USB backend, so they are left out there.
        if self.backend == "loopback" {
            args.extend(["--loopback-models".into(), self.loopback_models.join(",")]);
            if let Some(ms) = self.loopback_cyclic_ms {
                args.extend(["--loopback-cyclic-ms".into(), ms.to_string()]);
            }
        }
        args
    }
}

/// One argument quoted as `CommandLineToArgvW` and the C runtime read it back: quotes only when
/// needed, backslashes doubled only where they precede a quote.
pub fn quote_arg(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '\n', '\x0b', '"']) {
        return arg.to_string();
    }
    let mut out = String::from("\"");
    let mut backslashes = 0;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                out.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            _ => {
                out.extend(std::iter::repeat_n('\\', backslashes));
                out.push(c);
                backslashes = 0;
            }
        }
    }
    out.extend(std::iter::repeat_n('\\', backslashes * 2));
    out.push('"');
    out
}

/// The full command line: the program always quoted, as Windows advises for login entries (an
/// unquoted path with spaces is resolved piece by piece), then each argument as needed.
pub fn command_line(exe: &Path, args: &[String]) -> String {
    let mut line = format!("\"{}\"", exe.display());
    for arg in args {
        line.push(' ');
        line.push_str(&quote_arg(arg));
    }
    line
}

/// The program a command line runs: the quoted first token, or everything up to the first space.
pub fn program_of(command: &str) -> Option<&str> {
    let command = command.trim_start();
    let program = match command.strip_prefix('"') {
        Some(rest) => &rest[..rest.find('"')?],
        None => command.split([' ', '\t']).next().unwrap_or(""),
    };
    (!program.is_empty()).then_some(program)
}

/// Everything after the program in a command line, leading space and all, so it can be put back
/// behind another program's path unchanged. An install re-points a login entry at the installed
/// binary and must not reinterpret the options the person's own entry carries.
pub fn arguments_of(command: &str) -> &str {
    let trimmed = command.trim_start();
    match trimmed.strip_prefix('"') {
        Some(rest) => match rest.find('"') {
            Some(at) => &rest[at + 1..],
            None => "",
        },
        None => match trimmed.find([' ', '\t']) {
            Some(at) => &trimmed[at..],
            None => "",
        },
    }
}

/// The program a login entry runs: the windowless build beside `exe` when there is one, so a
/// server started at login opens no console window; else `exe` itself.
///
/// The windowless build is the same server with a `w` after the name, as `pythonw` and `javaw`
/// are: `gazelle-audio-server.exe` → `gazelle-audio-serverw.exe`. `exists` is a parameter so the
/// rule is tested without files.
pub fn boot_program(exe: &Path, exists: impl Fn(&Path) -> bool) -> PathBuf {
    let Some(stem) = exe.file_stem().and_then(|s| s.to_str()) else { return exe.to_path_buf() };
    if stem.ends_with(WINDOWLESS_SUFFIX) {
        return exe.to_path_buf();
    }
    let mut name = format!("{stem}{WINDOWLESS_SUFFIX}");
    if let Some(ext) = exe.extension().and_then(|e| e.to_str()) {
        name = format!("{name}.{ext}");
    }
    let windowless = exe.with_file_name(name);
    if exists(&windowless) {
        windowless
    } else {
        exe.to_path_buf()
    }
}

/// What the windowless build adds to the server's name.
pub const WINDOWLESS_SUFFIX: &str = "w";

/// The start-on-boot setting for one binary with one argument list.
pub struct StartOnBoot {
    key: Box<dyn RunKey>,
    exe: PathBuf,
    command: String,
}

impl StartOnBoot {
    pub fn new(key: Box<dyn RunKey>, exe: PathBuf, args: &BootArgs) -> Self {
        let command = command_line(&exe, &args.arguments());
        Self { key, exe, command }
    }

    /// The command line an enabled entry holds.
    pub fn command(&self) -> &str {
        &self.command
    }

    /// Whether the entry exists and runs this binary. Its arguments are not compared: an entry
    /// made from a server started with other options is still this server starting on boot.
    /// An entry for another binary (another build, another checkout) reads as off, so turning it
    /// on points it here. A key that cannot be read also reads as off.
    pub fn is_enabled(&self) -> bool {
        let Ok(Some(entry)) = self.key.read(ENTRY_NAME) else { return false };
        // Windows paths compare without case.
        program_of(&entry).is_some_and(|p| p.to_lowercase() == self.exe.display().to_string().to_lowercase())
    }

    pub fn set(&self, on: bool) -> io::Result<()> {
        if on {
            self.key.write(ENTRY_NAME, &self.command)
        } else {
            self.key.remove(ENTRY_NAME)
        }
    }

    /// Flip the setting as the menu shows it, and return the new state.
    pub fn toggle(&self) -> io::Result<bool> {
        let on = !self.is_enabled();
        self.set(on)?;
        Ok(on)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;

    /// The login entries, in memory. Shared through `Rc` so a test can look after handing it over.
    #[derive(Clone, Default)]
    struct FakeRunKey {
        entries: Rc<RefCell<HashMap<String, String>>>,
        unreadable: bool,
    }

    impl RunKey for FakeRunKey {
        fn read(&self, name: &str) -> io::Result<Option<String>> {
            if self.unreadable {
                return Err(io::Error::new(io::ErrorKind::PermissionDenied, "unreadable"));
            }
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

    fn args() -> BootArgs {
        BootArgs {
            bind: "127.0.0.1:8420".parse().unwrap(),
            backend: "usb".into(),
            dry_run: false,
            workspace: None,
            themes_dir: None,
            log_dir: None,
            loopback_models: vec!["quadro".into(), "studio".into()],
            loopback_cyclic_ms: None,
            no_web_ui: false,
        }
    }

    const EXE: &str = r"C:\Program Files\Gazelle\gazelle-audio-server.exe";

    /// The ordinary desktop run is `usb`, which is the default, so the entry a person reads in
    /// Task Manager or the registry says nothing about backends at all (decision `0018`).
    #[test]
    fn the_default_backend_is_not_spelled_out_and_the_other_one_is() {
        assert_eq!(args().arguments(), ["--bind", "127.0.0.1:8420"]);
        let loopback = BootArgs { backend: "loopback".into(), ..args() };
        assert_eq!(loopback.arguments()[..4], ["--backend", "loopback", "--bind", "127.0.0.1:8420"]);
    }

    #[test]
    fn options_that_change_what_is_served_are_carried_when_set() {
        let a = BootArgs {
            dry_run: true,
            workspace: Some(PathBuf::from("/w/workspace.json")),
            themes_dir: Some(PathBuf::from("/w/themes")),
            log_dir: Some(PathBuf::from("/w/logs")),
            no_web_ui: true,
            ..args()
        };
        assert_eq!(
            a.arguments(),
            [
                "--bind", "127.0.0.1:8420", "--dry-run", "--workspace", "/w/workspace.json", "--themes-dir",
                "/w/themes", "--log-dir", "/w/logs", "--no-web-ui",
            ]
        );
    }

    #[test]
    fn loopback_options_are_carried_only_for_the_loopback_backend() {
        let usb = BootArgs { loopback_cyclic_ms: Some(50), ..args() };
        assert!(!usb.arguments().iter().any(|a| a.starts_with("--loopback")));
        let loopback = BootArgs { backend: "loopback".into(), ..usb };
        assert_eq!(
            loopback.arguments()[4..],
            ["--loopback-models", "quadro,studio", "--loopback-cyclic-ms", "50"]
        );
    }

    #[test]
    fn relative_paths_are_resolved_against_the_working_directory() {
        let cwd = std::env::current_dir().unwrap();
        let absolute = cwd.join("elsewhere").join("themes");
        let a = BootArgs {
            workspace: Some("state/workspace.json".into()),
            themes_dir: Some(absolute.clone()),
            log_dir: Some("logs".into()),
            ..args()
        }
        .absolute(&cwd);
        assert_eq!(a.workspace, Some(cwd.join("state/workspace.json")));
        assert_eq!(a.log_dir, Some(cwd.join("logs")));
        assert_eq!(a.themes_dir, Some(absolute), "an absolute path is left alone");
        assert_eq!(BootArgs { ..args() }.absolute(&cwd), args(), "no paths, nothing to resolve");
    }

    #[test]
    fn arguments_are_quoted_only_when_they_need_it() {
        assert_eq!(quote_arg("--dry-run"), "--dry-run");
        assert_eq!(quote_arg(r"C:\a\b"), r"C:\a\b", "backslashes alone are literal");
        assert_eq!(quote_arg(""), "\"\"");
        assert_eq!(quote_arg(r"C:\My Themes"), r#""C:\My Themes""#);
        assert_eq!(quote_arg(r"C:\My Themes\"), r#""C:\My Themes\\""#, "a trailing backslash must not escape the closing quote");
        assert_eq!(quote_arg(r#"say "hi""#), r#""say \"hi\"""#);
        assert_eq!(quote_arg(r#"a\"b"#), r#""a\\\"b""#, "a backslash before a quote is doubled, then the quote escaped");
    }

    #[test]
    fn the_command_line_quotes_the_program_and_each_argument_as_needed() {
        let line = command_line(Path::new(EXE), &["--themes-dir".into(), r"C:\My Themes".into()]);
        assert_eq!(line, r#""C:\Program Files\Gazelle\gazelle-audio-server.exe" --themes-dir "C:\My Themes""#);
    }

    #[test]
    fn the_program_is_read_back_quoted_or_not() {
        assert_eq!(program_of(r#""C:\Program Files\g.exe" --bind x"#), Some(r"C:\Program Files\g.exe"));
        assert_eq!(program_of(r"C:\tools\g.exe --bind x"), Some(r"C:\tools\g.exe"));
        assert_eq!(program_of(r"C:\tools\g.exe"), Some(r"C:\tools\g.exe"));
        assert_eq!(program_of(r#""C:\unterminated"#), None);
        assert_eq!(program_of(""), None);
    }

    #[test]
    fn what_follows_the_program_is_carried_over_untouched() {
        assert_eq!(arguments_of(r#""C:\Program Files\g.exe" --bind 127.0.0.1:8420 --dry-run"#), " --bind 127.0.0.1:8420 --dry-run");
        assert_eq!(arguments_of(r"C:\tools\g.exe --backend usb"), " --backend usb");
        assert_eq!(arguments_of(r#""C:\tools\g.exe""#), "", "a program on its own carries nothing");
        assert_eq!(arguments_of(r"C:\tools\g.exe"), "");
        assert_eq!(arguments_of(r#""C:\unterminated --bind x"#), "", "an unterminated quote names no arguments either");
        assert_eq!(arguments_of(""), "");
    }

    #[test]
    fn a_login_entry_runs_the_windowless_build_when_it_is_there() {
        let windowless = r"C:\Program Files\Gazelle\gazelle-audio-serverw.exe";
        let only = |present: &'static str| move |p: &Path| p == Path::new(present);
        assert_eq!(boot_program(Path::new(EXE), only(windowless)), PathBuf::from(windowless));
        assert_eq!(boot_program(Path::new(EXE), |_| false), PathBuf::from(EXE), "no windowless build: the console one");
        assert_eq!(boot_program(Path::new(windowless), |_| true), PathBuf::from(windowless), "already windowless, no second w");
        assert_eq!(boot_program(Path::new("/opt/gazelle/gazelle-audio-server"), |_| true), PathBuf::from("/opt/gazelle/gazelle-audio-serverw"));
    }

    #[test]
    fn turning_it_on_registers_this_binary_with_the_carried_arguments() {
        let key = FakeRunKey::default();
        let boot = StartOnBoot::new(Box::new(key.clone()), EXE.into(), &args());
        assert!(!boot.is_enabled());
        boot.set(true).unwrap();
        assert_eq!(
            key.entries.borrow().get(ENTRY_NAME).map(String::as_str),
            Some(r#""C:\Program Files\Gazelle\gazelle-audio-server.exe" --bind 127.0.0.1:8420"#)
        );
        assert!(boot.is_enabled());
        boot.set(false).unwrap();
        assert!(key.entries.borrow().is_empty());
        assert!(!boot.is_enabled());
    }

    #[test]
    fn the_check_follows_the_binary_not_the_arguments() {
        let key = FakeRunKey::default();
        let boot = StartOnBoot::new(Box::new(key.clone()), EXE.into(), &args());
        let set = |command: &str| key.entries.borrow_mut().insert(ENTRY_NAME.into(), command.into());

        set(r#""C:\Program Files\Gazelle\gazelle-audio-server.exe" --backend loopback --bind 127.0.0.1:9000"#);
        assert!(boot.is_enabled(), "other arguments, same binary");
        set(r#""c:\program files\gazelle\GAZELLE-AUDIO-SERVER.EXE""#);
        assert!(boot.is_enabled(), "Windows paths compare without case");
        set(r#""C:\other\gazelle-audio-server.exe" --backend usb --bind 127.0.0.1:8420"#);
        assert!(!boot.is_enabled(), "another build's entry is not this one");
        key.entries.borrow_mut().insert("Something else".into(), EXE.into());
        key.entries.borrow_mut().remove(ENTRY_NAME);
        assert!(!boot.is_enabled(), "only the Gazelle entry counts");
    }

    #[test]
    fn toggling_flips_the_shown_state_and_takes_over_a_stale_entry() {
        let key = FakeRunKey::default();
        let boot = StartOnBoot::new(Box::new(key.clone()), EXE.into(), &args());
        key.entries.borrow_mut().insert(ENTRY_NAME.into(), r#""C:\old\gazelle-audio-server.exe""#.into());

        assert!(boot.toggle().unwrap(), "shown off, so the click turns it on");
        assert_eq!(key.entries.borrow().get(ENTRY_NAME).map(String::as_str), Some(boot.command()));
        assert!(!boot.toggle().unwrap());
        assert!(key.entries.borrow().get(ENTRY_NAME).is_none());
    }

    #[test]
    fn an_unreadable_key_reads_as_off() {
        let key = FakeRunKey { unreadable: true, ..FakeRunKey::default() };
        key.entries.borrow_mut().insert(ENTRY_NAME.into(), format!("\"{EXE}\""));
        assert!(!StartOnBoot::new(Box::new(key), EXE.into(), &args()).is_enabled());
    }
}
