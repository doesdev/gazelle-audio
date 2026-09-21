//! Choosing the aggregate driver a release build carries inside the server.
//!
//! A release ships Gazelle Aggregate (`gazelle_aggregate.dll`) inside the server executable, and
//! the server writes it out beside itself on `--install` and at every start (`aggregate::bundled`).
//! Carrying it inside keeps the release to the files it already has, and the update's signature
//! already covers the executable, so it covers the driver too, with no second signature.
//!
//! The build is told which file to carry by [`VAR`], and only the release sets it. An ordinary
//! `cargo build` and every test leave it unset and carry nothing, so neither has to build the
//! driver first. **A build that is asked to carry it and cannot is a failed build**, never a
//! release that quietly ships without it: a missing file, one that is not a DLL, and a DLL for
//! another machine are all refused here, before anything is compiled.
//!
//! `build.rs` uses this; the library includes the same file under `cfg(test)` so that what the
//! build decides is under test, as it does for `resource.rs`.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// The environment variable naming the driver to carry. A relative path is taken from the
/// workspace root, which is where the release workflow and `docs/releasing.md` run from.
pub const VAR: &str = "GAZELLE_AGGREGATE_DLL";

/// The file in `OUT_DIR` the library includes: the driver's bytes, or nothing at all.
pub const EMBEDDED_NAME: &str = "gazelle_aggregate.dll";

/// Where the file `value` names is, taking a relative path from `root`.
pub fn resolve(value: &OsStr, root: &Path) -> PathBuf {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        root.join(path)
    }
}

/// What to carry: nothing when [`VAR`] is unset or empty, the named file's path and bytes when
/// it is a DLL for this build's machine, and a sentence saying what is wrong otherwise.
///
/// `os` and `arch` are the target's (`CARGO_CFG_TARGET_OS` and `CARGO_CFG_TARGET_ARCH`), since a
/// driver built for another machine would be written out and then refused by every DAW.
pub fn choose(value: Option<&OsStr>, root: &Path, os: &str, arch: &str) -> Result<Option<(PathBuf, Vec<u8>)>, String> {
    let Some(value) = value.filter(|v| !v.is_empty()) else { return Ok(None) };
    let path = resolve(value, root);
    if os != "windows" {
        return Err(format!("{VAR} names {}, but only a Windows build carries the aggregate driver", path.display()));
    }
    let bytes = std::fs::read(&path).map_err(|e| {
        format!(
            "{VAR} names {}, which could not be read ({e}). Build the driver first: cargo build --release -p gazelle-audio-aggregate",
            path.display()
        )
    })?;
    check_dll(&bytes, arch).map_err(|why| format!("{VAR} names {}, which {why}", path.display()))?;
    Ok(Some((path, bytes)))
}

/// The PE machine type a DLL for `arch` has.
fn machine_for(arch: &str) -> Option<u16> {
    match arch {
        "x86_64" => Some(0x8664),
        "aarch64" => Some(0xAA64),
        "x86" => Some(0x014C),
        _ => None,
    }
}

/// Whether `bytes` is a DLL for `arch`, or what it is instead, worded to follow "which".
pub fn check_dll(bytes: &[u8], arch: &str) -> Result<(), String> {
    let u16_at = |at: usize| bytes.get(at..at + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
    let u32_at = |at: usize| bytes.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    if bytes.get(..2) != Some(b"MZ".as_slice()) {
        return Err("is not a Windows program or DLL".into());
    }
    let pe = u32_at(0x3c).map(|at| at as usize).ok_or("is too short to be a DLL")?;
    if bytes.get(pe..pe + 4) != Some(b"PE\0\0".as_slice()) {
        return Err("is not a Windows program or DLL".into());
    }
    let machine = u16_at(pe + 4).ok_or("is too short to be a DLL")?;
    let characteristics = u16_at(pe + 22).ok_or("is too short to be a DLL")?;
    // IMAGE_FILE_DLL: an executable has the same header without it.
    if characteristics & 0x2000 == 0 {
        return Err("is a program, not a DLL".into());
    }
    let wanted = machine_for(arch).ok_or_else(|| format!("cannot be checked against this build's machine ({arch})"))?;
    if machine != wanted {
        return Err(format!("is built for machine type {machine:#06x}, and this build is {arch} ({wanted:#06x})"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The smallest header `check_dll` reads: "MZ", the offset of the PE header, then the PE
    /// signature, the machine and the characteristics.
    fn header(machine: u16, characteristics: u16) -> Vec<u8> {
        let mut bytes = vec![0u8; 0x40 + 24];
        bytes[..2].copy_from_slice(b"MZ");
        bytes[0x3c..0x40].copy_from_slice(&0x40u32.to_le_bytes());
        bytes[0x40..0x44].copy_from_slice(b"PE\0\0");
        bytes[0x44..0x46].copy_from_slice(&machine.to_le_bytes());
        bytes[0x40 + 22..0x40 + 24].copy_from_slice(&characteristics.to_le_bytes());
        bytes
    }

    struct Dir(PathBuf);

    impl Dir {
        fn new(name: &str) -> Dir {
            let dir = std::env::temp_dir().join(format!("gazelle-build-driver-{}-{name}", std::process::id()));
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
    fn a_build_not_asked_to_carry_the_driver_carries_nothing() {
        assert_eq!(choose(None, Path::new("/nowhere"), "windows", "x86_64"), Ok(None));
        assert_eq!(choose(Some(OsStr::new("")), Path::new("/nowhere"), "windows", "x86_64"), Ok(None), "set but empty is unset");
    }

    #[test]
    fn a_build_asked_to_carry_a_driver_that_is_not_there_fails_and_says_how_to_build_it() {
        let dir = Dir::new("missing");
        let error = choose(Some(OsStr::new("target/release/gazelle_aggregate.dll")), &dir.0, "windows", "x86_64").unwrap_err();
        assert!(error.contains(VAR), "{error}");
        assert!(error.contains(&dir.0.join("target/release/gazelle_aggregate.dll").display().to_string()), "the path is taken from the root: {error}");
        assert!(error.contains("cargo build --release -p gazelle-audio-aggregate"), "{error}");
    }

    #[test]
    fn only_a_dll_for_this_machine_is_carried() {
        let dir = Dir::new("kinds");
        let write = |name: &str, bytes: &[u8]| {
            std::fs::write(dir.0.join(name), bytes).unwrap();
            dir.0.join(name)
        };
        let good = write("good.dll", &header(0x8664, 0x2022));
        let (path, bytes) = choose(Some(good.as_os_str()), Path::new("/ignored"), "windows", "x86_64").unwrap().unwrap();
        assert_eq!(path, good, "an absolute path is used as it is");
        assert_eq!(bytes, header(0x8664, 0x2022));

        let cases: [(&str, Vec<u8>, &str); 4] = [
            ("text.dll", b"not a DLL at all".to_vec(), "not a Windows program"),
            ("program.exe", header(0x8664, 0x0022), "a program, not a DLL"),
            ("arm.dll", header(0xAA64, 0x2022), "machine type 0xaa64"),
            ("short.dll", b"MZ".to_vec(), "too short"),
        ];
        for (name, bytes, expected) in cases {
            let path = write(name, &bytes);
            let error = choose(Some(path.as_os_str()), Path::new("/ignored"), "windows", "x86_64").unwrap_err();
            assert!(error.contains(expected), "{name}: {error}");
        }
        assert!(choose(Some(good.as_os_str()), Path::new("/ignored"), "linux", "x86_64").unwrap_err().contains("only a Windows build"));
    }
}
