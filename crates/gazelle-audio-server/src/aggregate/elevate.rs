//! Registering the aggregate driver, which is the one thing Gazelle does that asks for
//! administrator rights.
//!
//! Registration writes under `HKEY_LOCAL_MACHINE`, so it cannot be done by the server itself:
//! Gazelle installs per user and runs as the person. The offer is therefore a separate program,
//! started with the verb that makes Windows put up its own prompt. If the person would rather do
//! it themselves, the answer always carries the exact command to type, and it is the same command.
//!
//! Nothing here runs in a test: [`Elevator`] is the seam, and the fake counts what it was asked
//! for and elevates nothing.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::Serialize;

/// The program that registers a driver DLL, and the switch that undoes it.
pub const REGISTRAR: &str = "regsvr32.exe";

/// The DLL's file name, as the driver crate builds it.
pub const AGGREGATE_DLL: &str = "gazelle_aggregate.dll";

/// What running the elevated program came to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ElevatedRun {
    /// True when Windows started the program at all: false means the prompt was declined.
    pub started: bool,
    /// The program's exit code, when it was waited for and gave one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<u32>,
    pub message: String,
}

/// Running a program elevated. One implementation prompts; the other counts.
pub trait Elevator: Send + Sync {
    /// Run `program` with `arguments`, asking Windows for administrator rights. Blocking while
    /// the prompt is up, so callers run it off the runtime.
    fn run(&self, program: &str, arguments: &str) -> Result<ElevatedRun, String>;
}

/// The command a person could run themselves, quoted as a shell needs it.
pub fn command_for(dll: &Path, unregister: bool) -> String {
    let switch = if unregister { "/u " } else { "" };
    format!("regsvr32 {switch}\"{}\"", dll.display())
}

/// The arguments the elevated registrar is given: silent, so it puts up no box of its own behind
/// the page that asked for it.
pub fn arguments_for(dll: &Path, unregister: bool) -> String {
    let switch = if unregister { "/u /s " } else { "/s " };
    format!("{switch}\"{}\"", dll.display())
}

/// Where the driver's DLL is looked for, in the order it is looked for: beside the server, in the
/// build output a developer would have just made, and in Gazelle's own folder.
pub fn dll_candidates(exe_dir: Option<&Path>, appdata: Option<&Path>, repo: Option<&Path>) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(dir) = exe_dir {
        candidates.push(dir.join(AGGREGATE_DLL));
    }
    if let Some(dir) = appdata {
        candidates.push(dir.join("gazelle").join(AGGREGATE_DLL));
    }
    if let Some(dir) = repo {
        candidates.push(dir.join("target").join("release").join(AGGREGATE_DLL));
        candidates.push(dir.join("target").join("debug").join(AGGREGATE_DLL));
    }
    candidates
}

/// Where the DLL was found, or every place that was looked in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum DllSearch {
    Found { dll: String },
    Missing { message: String, looked_in: Vec<String> },
}

/// The first candidate that is a file, or a refusal naming every place that was tried.
pub fn find_dll(candidates: &[PathBuf], is_file: &dyn Fn(&Path) -> bool) -> DllSearch {
    match candidates.iter().find(|path| is_file(path)) {
        Some(dll) => DllSearch::Found { dll: dll.display().to_string() },
        None => DllSearch::Missing {
            message: format!(
                "{AGGREGATE_DLL} was not found. Build it with `cargo build -p gazelle-audio-aggregate --release` and put it beside Gazelle, or copy it into Gazelle's own folder."
            ),
            looked_in: candidates.iter().map(|p| p.display().to_string()).collect(),
        },
    }
}

/// The real prompt, on Windows; elsewhere, an elevator that refuses.
pub fn for_this_pc() -> Arc<dyn Elevator> {
    #[cfg(windows)]
    {
        Arc::new(windows::Uac)
    }
    #[cfg(not(windows))]
    {
        Arc::new(Elsewhere)
    }
}

#[cfg(not(windows))]
struct Elsewhere;

#[cfg(not(windows))]
impl Elevator for Elsewhere {
    fn run(&self, _program: &str, _arguments: &str) -> Result<ElevatedRun, String> {
        Err("the aggregate driver is registered on Windows only".into())
    }
}

#[cfg(windows)]
pub mod windows {
    //! `ShellExecuteExW` with the `runas` verb, which is what makes Windows prompt. The prompt is
    //! the person's decision and the only place it can be made; nothing here elevates by itself,
    //! and a declined prompt is an ordinary answer rather than a failure.

    use std::ptr::null;

    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_CANCELLED, HANDLE, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
    use windows_sys::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;

    use super::{ElevatedRun, Elevator};

    /// How long the program is waited for once the prompt has been answered. Registering a DLL is
    /// instant; this is only so a person who leaves the prompt on screen does not hold a worker.
    const WAIT_MS: u32 = 120_000;

    pub struct Uac;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    impl Elevator for Uac {
        fn run(&self, program: &str, arguments: &str) -> Result<ElevatedRun, String> {
            let verb = wide("runas");
            let file = wide(program);
            let parameters = wide(arguments);
            let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
            info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
            info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC;
            info.lpVerb = verb.as_ptr();
            info.lpFile = file.as_ptr();
            info.lpParameters = parameters.as_ptr();
            info.lpDirectory = null();
            info.nShow = SW_HIDE;
            if unsafe { ShellExecuteExW(&mut info) } == 0 {
                let error = std::io::Error::last_os_error();
                let declined = error.raw_os_error() == Some(ERROR_CANCELLED as i32);
                return Ok(ElevatedRun {
                    started: false,
                    exit_code: None,
                    message: if declined {
                        "Windows asked for administrator rights and the prompt was declined, so nothing was changed.".into()
                    } else {
                        format!("Windows would not start {program} as an administrator: {error}.")
                    },
                });
            }
            let process: HANDLE = info.hProcess;
            if process.is_null() {
                return Ok(ElevatedRun { started: true, exit_code: None, message: format!("{program} was started as an administrator.") });
            }
            let waited = unsafe { WaitForSingleObject(process, WAIT_MS) };
            let mut code = 0u32;
            let read = waited == WAIT_OBJECT_0 && unsafe { GetExitCodeProcess(process, &mut code) } != 0;
            unsafe { CloseHandle(process) };
            Ok(match read {
                false => ElevatedRun {
                    started: true,
                    exit_code: None,
                    message: format!("{program} was started as an administrator and had not finished when Gazelle stopped waiting."),
                },
                true if code == 0 => ElevatedRun { started: true, exit_code: Some(0), message: format!("{program} finished and reported success.") },
                true => ElevatedRun { started: true, exit_code: Some(code), message: format!("{program} finished and reported code {code}.") },
            })
        }
    }
}

/// An elevator that runs nothing and remembers what it was asked for.
pub struct FakeElevator {
    pub calls: Mutex<Vec<(String, String)>>,
    pub answer: Result<ElevatedRun, String>,
}

impl Default for FakeElevator {
    fn default() -> Self {
        FakeElevator {
            calls: Mutex::new(Vec::new()),
            answer: Ok(ElevatedRun { started: true, exit_code: Some(0), message: "regsvr32.exe finished and reported success.".into() }),
        }
    }
}

impl FakeElevator {
    pub fn declining() -> Self {
        FakeElevator {
            answer: Ok(ElevatedRun {
                started: false,
                exit_code: None,
                message: "Windows asked for administrator rights and the prompt was declined, so nothing was changed.".into(),
            }),
            ..FakeElevator::default()
        }
    }

    pub fn calls(&self) -> Vec<(String, String)> {
        self.calls.lock().unwrap().clone()
    }
}

impl Elevator for FakeElevator {
    fn run(&self, program: &str, arguments: &str) -> Result<ElevatedRun, String> {
        self.calls.lock().unwrap().push((program.to_string(), arguments.to_string()));
        self.answer.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_shown_and_the_one_run_register_the_same_file() {
        let dll = PathBuf::from(r"C:\Program Files\Gazelle\gazelle_aggregate.dll");
        assert_eq!(command_for(&dll, false), r#"regsvr32 "C:\Program Files\Gazelle\gazelle_aggregate.dll""#);
        assert_eq!(command_for(&dll, true), r#"regsvr32 /u "C:\Program Files\Gazelle\gazelle_aggregate.dll""#);
        assert_eq!(arguments_for(&dll, false), r#"/s "C:\Program Files\Gazelle\gazelle_aggregate.dll""#);
        assert_eq!(arguments_for(&dll, true), r#"/u /s "C:\Program Files\Gazelle\gazelle_aggregate.dll""#);
    }

    #[test]
    fn the_dll_is_looked_for_beside_gazelle_first_and_every_place_is_named_when_it_is_not_found() {
        let candidates = dll_candidates(Some(Path::new(r"C:\app")), Some(Path::new(r"C:\appdata")), Some(Path::new(r"C:\repo")));
        assert_eq!(candidates[0], PathBuf::from(r"C:\app\gazelle_aggregate.dll"));
        assert_eq!(candidates[1], PathBuf::from(r"C:\appdata\gazelle\gazelle_aggregate.dll"));
        assert_eq!(candidates.len(), 4, "and the two build outputs");

        let found = find_dll(&candidates, &|path| path.ends_with(r"gazelle\gazelle_aggregate.dll"));
        assert_eq!(found, DllSearch::Found { dll: r"C:\appdata\gazelle\gazelle_aggregate.dll".into() });

        let DllSearch::Missing { message, looked_in } = find_dll(&candidates, &|_| false) else {
            panic!("nothing is a file, so nothing is found");
        };
        assert!(message.contains("cargo build -p gazelle-audio-aggregate"), "{message}");
        assert_eq!(looked_in.len(), 4, "every place that was tried is named: {looked_in:?}");
    }

    #[test]
    fn the_fake_elevator_runs_nothing_and_remembers_what_it_was_asked() {
        let elevator = FakeElevator::default();
        elevator.run(REGISTRAR, r#"/s "C:\a.dll""#).unwrap();
        assert_eq!(elevator.calls(), vec![(REGISTRAR.to_string(), r#"/s "C:\a.dll""#.to_string())]);
        assert!(!FakeElevator::declining().answer.unwrap().started);
    }
}
