//! The aggregate driver a release carries inside Gazelle, and putting it beside Gazelle.
//!
//! A release build holds `gazelle_aggregate.dll` byte for byte (`build/driver.rs` chooses it),
//! so the release stays the files it always was and the update's signature, which covers the
//! executable, covers the driver as well. `--install` writes it into the install folder, and
//! every start writes it beside the running executable when it is missing or differs, which is
//! how an update brings a new driver: the updated Gazelle's first start puts its own copy there.
//! The Aggregate page then finds it beside Gazelle, which is the first place it looks.
//!
//! Registering it is still the page's, and still asks for administrator rights: nothing here
//! registers anything. The registration names the file's path, so replacing the file is enough
//! for the next DAW that opens the driver to get the new one.
//!
//! # A driver a DAW has open
//!
//! Windows will not overwrite or delete a DLL a process has loaded, but it will let it be
//! renamed, and the process keeps running from the renamed file. So when the file is in use the
//! old one is moved aside to `gazelle_aggregate.dll.<n>.old` and the new one written in its
//! place: the running DAW keeps its mapping, and the next one to open the driver gets the new
//! file. The copies moved aside are deleted at a later start, once nothing holds them. The
//! updater does the same with the executable (`update::stage`).
//!
//! Writing the driver never fails an install or a start. When even the rename fails, the reason
//! is logged, printed by `--install`, and shown on the Aggregate page, and the next start tries
//! again.

use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::aggregate::elevate::AGGREGATE_DLL;

/// The driver's bytes when this build carries it, and nothing when it does not. `build.rs`
/// always writes the file, empty when there is no driver to carry, so every build compiles.
static CARRIED: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/gazelle_aggregate.dll"));

/// The driver this build carries, or `None` for a build that carries none: every developer build
/// and every test build, which are not asked to (`GAZELLE_AGGREGATE_DLL`).
pub fn carried() -> Option<&'static [u8]> {
    (!CARRIED.is_empty()).then_some(CARRIED)
}

/// Where the driver goes: beside the executable, in the same folder.
pub fn path_in(dir: &Path) -> PathBuf {
    dir.join(AGGREGATE_DLL)
}

/// Where the new copy is written before it is moved into place, so a half written file is never
/// the one a DAW opens.
fn download_path(dir: &Path) -> PathBuf {
    dir.join(format!("{AGGREGATE_DLL}.download"))
}

/// Whether a file in the install folder is a copy of the driver moved aside by an earlier write,
/// or a download an interrupted write left behind: `gazelle_aggregate.dll.<n>.old`, and
/// `gazelle_aggregate.dll.download`.
pub fn is_leftover(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    let Some(rest) = name.strip_prefix(&format!("{AGGREGATE_DLL}.")) else { return false };
    rest == "download" || rest.strip_suffix(".old").is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// The first free name to move an in-use driver aside to.
fn aside_path(dir: &Path) -> PathBuf {
    (1u32..)
        .map(|n| dir.join(format!("{AGGREGATE_DLL}.{n}.old")))
        .find(|path| !path.exists())
        .expect("a free name among four billion")
}

/// What putting the driver in place did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Placed {
    /// The file there was already this copy, byte for byte.
    AlreadyThere,
    /// There was no driver there, and now there is.
    Written,
    /// A different copy was there and has been replaced.
    Replaced,
    /// A different copy was there and in use, so it was moved aside to `aside` and this one
    /// written in its place. A later start deletes `aside` once nothing holds it.
    MovedAside { aside: PathBuf },
}

/// Put `bytes` at `gazelle_aggregate.dll` in `dir`, unless that is already what is there.
///
/// Written beside the target first and renamed over it, so the file a DAW opens is always a
/// whole one. When the rename cannot replace the file because a DAW has it loaded, that file is
/// renamed aside and the rename tried again. Anything that fails leaves the old file where it
/// was and no download behind, and says why.
pub fn put_in_place(dir: &Path, bytes: &[u8]) -> Result<Placed, String> {
    let target = path_in(dir);
    let existed = target.is_file();
    if existed && std::fs::read(&target).is_ok_and(|there| there == bytes) {
        return Ok(Placed::AlreadyThere);
    }
    let download = download_path(dir);
    if let Err(e) = std::fs::write(&download, bytes) {
        let _ = std::fs::remove_file(&download);
        return Err(format!("writing {}: {e}", download.display()));
    }
    let replace_error = match std::fs::rename(&download, &target) {
        Ok(()) => return Ok(if existed { Placed::Replaced } else { Placed::Written }),
        Err(e) => e,
    };
    if !existed {
        let _ = std::fs::remove_file(&download);
        return Err(format!("putting {} in place: {replace_error}", target.display()));
    }
    // In use, most likely: a DAW has the driver loaded. Renaming is still allowed.
    let aside = aside_path(dir);
    if let Err(e) = std::fs::rename(&target, &aside) {
        let _ = std::fs::remove_file(&download);
        return Err(format!(
            "{} could not be replaced ({replace_error}) or moved aside ({e}); if a DAW has it open, it is replaced at a start after the DAW is closed",
            target.display()
        ));
    }
    match std::fs::rename(&download, &target) {
        Ok(()) => Ok(Placed::MovedAside { aside }),
        Err(e) => {
            // Put the old one back, so the registration still names a file that is there.
            let _ = std::fs::rename(&aside, &target);
            let _ = std::fs::remove_file(&download);
            Err(format!("putting {} in place after moving the old one aside: {e}", target.display()))
        }
    }
}

/// What clearing away the leftovers of earlier writes did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Swept {
    pub removed: Vec<PathBuf>,
    /// Still held (a DAW that has not been closed since has an old copy loaded), and tried
    /// again at the next start.
    pub kept: Vec<PathBuf>,
}

/// Delete every copy an earlier write moved aside, and any download it left, in `dir`. A copy a
/// DAW still has loaded cannot be deleted; it is kept and tried again next time.
pub fn sweep(dir: &Path) -> Swept {
    let mut swept = Swept::default();
    let Ok(entries) = std::fs::read_dir(dir) else { return swept };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !is_leftover(&name) {
            continue;
        }
        let path = entry.path();
        match std::fs::remove_file(&path) {
            Ok(()) => swept.removed.push(path),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(_) => swept.kept.push(path),
        }
    }
    swept.removed.sort();
    swept.kept.sort();
    swept
}

/// Where the driver this build carries stands, for the Aggregate page. Serialised as
/// `{"state": "...", ...}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Status {
    /// This build carries no driver. A release does; a developer build does not.
    NotCarried,
    /// Carried, and the file beside Gazelle is this copy.
    InPlace { dll: String },
    /// Carried, and it could not be put beside Gazelle. The next start tries again.
    Failed { dll: String, message: String },
}

impl Status {
    /// What the status says, in a sentence.
    pub fn message(&self) -> String {
        match self {
            Status::NotCarried => {
                format!("This build of Gazelle does not carry {AGGREGATE_DLL}. A release does, and puts it beside Gazelle.")
            }
            Status::InPlace { dll } => format!("Gazelle put its copy of the driver at {dll}."),
            Status::Failed { dll, message } => format!(
                "Gazelle carries {AGGREGATE_DLL} but could not put it at {dll}: {message}. It tries again the next time Gazelle starts."
            ),
        }
    }
}

/// Put the carried driver beside the executable at `exe`, after clearing away what earlier
/// writes left. What every start does, before the server is up; never fatal.
pub fn at_start(exe: &Path, carried: Option<&[u8]>) -> Status {
    let Some(bytes) = carried else { return Status::NotCarried };
    let Some(dir) = exe.parent().filter(|d| !d.as_os_str().is_empty()) else {
        return Status::Failed { dll: AGGREGATE_DLL.into(), message: format!("{} is in no folder", exe.display()) };
    };
    let swept = sweep(dir);
    for path in &swept.removed {
        tracing::info!("removed {}, an older copy of the aggregate driver", path.display());
    }
    for path in &swept.kept {
        tracing::info!("{} is still in use, so it is kept until a later start", path.display());
    }
    let dll = path_in(dir).display().to_string();
    match put_in_place(dir, bytes) {
        Ok(placed) => {
            match &placed {
                Placed::AlreadyThere => {}
                Placed::Written => tracing::info!("put the aggregate driver at {dll}"),
                Placed::Replaced => tracing::info!("replaced the aggregate driver at {dll} with this version's"),
                Placed::MovedAside { aside } => tracing::info!(
                    "the aggregate driver at {dll} is in use, so it was moved to {} and this version's put in its place; a DAW opening the driver from now on gets the new one",
                    aside.display()
                ),
            }
            Status::InPlace { dll }
        }
        Err(message) => {
            tracing::warn!("the aggregate driver was not put beside Gazelle: {message}. The next start tries again.");
            Status::Failed { dll, message }
        }
    }
}

/// The same for the running executable: what `main` calls.
pub fn at_this_start() -> Status {
    match std::env::current_exe() {
        Ok(exe) => at_start(&exe, carried()),
        Err(e) if carried().is_some() => {
            tracing::warn!("not putting the aggregate driver beside Gazelle: {e}");
            Status::Failed { dll: AGGREGATE_DLL.into(), message: format!("finding Gazelle's own folder: {e}") }
        }
        Err(_) => Status::NotCarried,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dir(PathBuf);

    impl Dir {
        fn new(name: &str) -> Dir {
            let dir = std::env::temp_dir().join(format!("gazelle-bundled-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Dir(dir)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_test_build_carries_no_driver() {
        // Nothing in a test run sets GAZELLE_AGGREGATE_DLL, and a release's own tests are not
        // this one; see tests/runtime.rs for the build that does.
        if std::env::var_os("GAZELLE_AGGREGATE_DLL").is_none() {
            assert_eq!(carried(), None);
        }
    }

    #[test]
    fn only_the_names_an_earlier_write_leaves_count_as_leftovers() {
        for name in ["gazelle_aggregate.dll.1.old", "GAZELLE_AGGREGATE.DLL.12.OLD", "gazelle_aggregate.dll.download"] {
            assert!(is_leftover(name), "{name}");
        }
        for name in ["gazelle_aggregate.dll", "gazelle_aggregate.dll.old", "gazelle_aggregate.dll.x.old", "gazelle-audio-server.exe.old", "notes.txt"] {
            assert!(!is_leftover(name), "{name}");
        }
    }

    #[test]
    fn nothing_there_is_written_the_same_copy_is_left_and_a_different_one_replaced() {
        let dir = Dir::new("place");
        assert_eq!(put_in_place(&dir.0, b"v1"), Ok(Placed::Written));
        assert_eq!(std::fs::read(path_in(&dir.0)).unwrap(), b"v1");
        assert_eq!(put_in_place(&dir.0, b"v1"), Ok(Placed::AlreadyThere));
        assert_eq!(put_in_place(&dir.0, b"v2"), Ok(Placed::Replaced));
        assert_eq!(std::fs::read(path_in(&dir.0)).unwrap(), b"v2");
        assert!(!download_path(&dir.0).exists(), "no download is left behind");
    }

    #[test]
    fn a_failure_leaves_the_old_copy_and_no_download() {
        let dir = Dir::new("fail");
        std::fs::write(path_in(&dir.0), b"v1").unwrap();
        // A folder where the download goes: it cannot be written.
        std::fs::create_dir_all(download_path(&dir.0)).unwrap();
        let error = put_in_place(&dir.0, b"v2").unwrap_err();
        assert!(error.contains("gazelle_aggregate.dll.download"), "{error}");
        assert_eq!(std::fs::read(path_in(&dir.0)).unwrap(), b"v1");
    }

    #[test]
    fn a_start_sweeps_first_then_puts_the_carried_copy_in_place() {
        let dir = Dir::new("start");
        std::fs::write(dir.0.join("gazelle_aggregate.dll.1.old"), b"old").unwrap();
        std::fs::write(dir.0.join("notes.txt"), b"mine").unwrap();
        let exe = dir.0.join("gazelle-audio-server.exe");

        let status = at_start(&exe, Some(b"new"));

        assert_eq!(status, Status::InPlace { dll: path_in(&dir.0).display().to_string() });
        assert_eq!(std::fs::read(path_in(&dir.0)).unwrap(), b"new");
        assert!(!dir.0.join("gazelle_aggregate.dll.1.old").exists());
        assert!(dir.0.join("notes.txt").exists(), "only the driver's own leftovers go");
        assert_eq!(at_start(&exe, None), Status::NotCarried, "a build carrying nothing writes nothing");
    }

    #[test]
    fn every_status_says_something_plain() {
        let statuses = [
            Status::NotCarried,
            Status::InPlace { dll: r"C:\g\gazelle_aggregate.dll".into() },
            Status::Failed { dll: r"C:\g\gazelle_aggregate.dll".into(), message: "access is denied".into() },
        ];
        for status in statuses {
            let message = status.message();
            assert!(!message.chars().any(|c| (0x2013..=0x2014).contains(&(c as u32))), "{message}");
            assert!(message.ends_with('.'), "{message}");
        }
        assert!(Status::Failed { dll: "x".into(), message: "y".into() }.message().contains("tries again"));
    }
}
