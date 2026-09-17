//! Putting a verified binary in place, which on Windows means moving the running one aside.
//!
//! Windows will not let an executable be overwritten or deleted while a process is running from
//! it, but it **will** let it be renamed: the open image keeps running from the renamed file.
//! So staging is two renames — the running binary to `<name>.old`, the verified download to
//! `<name>` — and the swap is finished before anyone restarts. The next successful start
//! deletes the `.old`, by which time nothing holds it.
//!
//! The process is never restarted from here. The user restarts, from the tray or by hand.

use std::io;
use std::path::{Path, PathBuf};

/// Where the running binary is moved to. The suffix is appended, not substituted, so
/// `gazelle-audio-server.exe` becomes `gazelle-audio-server.exe.old` and the two never collide
/// with a real asset name.
pub fn old_path(target: &Path) -> PathBuf {
    let mut name = target.as_os_str().to_os_string();
    name.push(".old");
    PathBuf::from(name)
}

/// Move `verified` into `target`, keeping the file that was there as `<target>.old`.
///
/// If the second rename fails the first is undone, so a failed staging leaves the binary that
/// was running exactly where it was. The caller owns `verified` and deletes it on an error.
pub fn stage(verified: &Path, target: &Path) -> io::Result<()> {
    let old = old_path(target);
    // A `.old` from an earlier update that could not be cleaned up (the file was still in use)
    // would make the rename below fail on some filesystems. Removing it first is allowed to
    // fail: it only matters if the rename then fails too, and that error is the one reported.
    let _ = std::fs::remove_file(&old);
    let displaced = match std::fs::rename(target, &old) {
        Ok(()) => true,
        // Nothing to displace: the target does not exist yet.
        Err(e) if e.kind() == io::ErrorKind::NotFound => false,
        Err(e) => return Err(e),
    };
    match std::fs::rename(verified, target) {
        Ok(()) => Ok(()),
        Err(e) => {
            if displaced {
                let _ = std::fs::rename(&old, target);
            }
            Err(e)
        }
    }
}

/// Delete the binary a previous update displaced, if one is there and nothing holds it.
///
/// Returns whether a file was removed. Failure is not an error worth stopping a start for — the
/// file is inert, and the next start tries again — so it is reported to the caller to log.
pub fn clean_old(target: &Path) -> Result<bool, io::Error> {
    let old = old_path(target);
    match std::fs::remove_file(&old) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dir(PathBuf);

    impl Dir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("gazelle-update-stage-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
        fn join(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn the_displaced_binary_keeps_its_whole_name_plus_old() {
        assert_eq!(old_path(Path::new(r"C:\apps\gazelle-audio-server.exe")), PathBuf::from(r"C:\apps\gazelle-audio-server.exe.old"));
        assert_eq!(old_path(Path::new("/usr/local/bin/gazelle-audio-server")), PathBuf::from("/usr/local/bin/gazelle-audio-server.old"));
    }

    #[test]
    fn staging_puts_the_new_binary_in_place_and_keeps_the_old_one() {
        let dir = Dir::new("swap");
        let (target, verified) = (dir.join("app.exe"), dir.join("app.exe.download"));
        std::fs::write(&target, b"running").unwrap();
        std::fs::write(&verified, b"new").unwrap();

        stage(&verified, &target).unwrap();

        assert_eq!(std::fs::read(&target).unwrap(), b"new");
        assert_eq!(std::fs::read(old_path(&target)).unwrap(), b"running");
        assert!(!verified.exists(), "the download is moved, not copied");
    }

    #[test]
    fn staging_twice_replaces_the_kept_copy_rather_than_failing() {
        let dir = Dir::new("twice");
        let target = dir.join("app.exe");
        std::fs::write(&target, b"v1").unwrap();
        for (body, kept) in [(b"v2", b"v1"), (b"v3", b"v2")] {
            let verified = dir.join("download");
            std::fs::write(&verified, body).unwrap();
            stage(&verified, &target).unwrap();
            assert_eq!(std::fs::read(&target).unwrap(), body);
            assert_eq!(std::fs::read(old_path(&target)).unwrap(), kept);
        }
    }

    #[test]
    fn a_failed_second_rename_puts_the_running_binary_back() {
        let dir = Dir::new("rollback");
        let target = dir.join("app.exe");
        std::fs::write(&target, b"running").unwrap();
        // Nothing where the verified download should be: the rename into place cannot work.
        let verified = dir.join("no-such-download");

        assert!(stage(&verified, &target).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"running", "a failed staging must leave the running binary in place");
    }

    #[test]
    fn staging_where_nothing_is_yet_simply_puts_the_file_there() {
        let dir = Dir::new("fresh");
        let (target, verified) = (dir.join("app.exe"), dir.join("download"));
        std::fs::write(&verified, b"new").unwrap();
        stage(&verified, &target).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"new");
        assert!(!old_path(&target).exists());
    }

    #[test]
    fn the_next_start_removes_the_displaced_binary_and_says_whether_there_was_one() {
        let dir = Dir::new("clean");
        let target = dir.join("app.exe");
        assert!(!clean_old(&target).unwrap(), "nothing to clean is not a failure");
        std::fs::write(old_path(&target), b"previous").unwrap();
        assert!(clean_old(&target).unwrap(), "a displaced binary is reported as removed");
        assert!(!old_path(&target).exists());
        assert!(!clean_old(&target).unwrap());
    }
}
