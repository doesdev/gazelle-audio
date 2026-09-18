//! Collect the release directory: exactly the files the release carries, under exactly the names
//! the updater asks for (`.agent/specs/2026-09-18-shipping-portable.md`, "The release layout").
//!
//! | File | From |
//! |---|---|
//! | `gazelle-audio-server-<target>.exe` | `<bin-dir>/gazelle-audio-server.exe` |
//! | `gazelle-audio-serverw-<target>.exe` | `<bin-dir>/gazelle-audio-serverw.exe` |
//! | `gazelle-audio-<target>.zip` | both of those, under their plain names, for a person |
//! | `gazelle-manual.pdf`, `gazelle-cheat-sheet.pdf` | `<docs>/`, from `pnpm -C web docs:pdf` |
//!
//! The asset names come from the updater's own `binary_asset_name`, and `<target>` defaults to
//! the triple the server's `build.rs` recorded for this helper's build, which is the host's, as
//! the release build's is. `sign` hashes everything in the directory, so this writes only into a
//! new or empty one and checks at the end that it holds those files and nothing else: a key, a
//! stray build output or a leftover from an earlier run can never be signed and uploaded.
//!
//! The zip is written by PowerShell's `Compress-Archive`, as the by-hand checklist always made it,
//! rather than by a zip crate: the release is built on Windows, where it is always there, and the
//! helper stays free of new dependencies (`reference/rust-crates.md`, "Toolchain floor").

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use gazelle_audio_server::update::release::binary_asset_name;
use gazelle_audio_server::update::BINARIES;

pub const MANUAL: &str = "gazelle-manual.pdf";
pub const CHEAT_SHEET: &str = "gazelle-cheat-sheet.pdf";

pub fn zip_name(target: &str) -> String {
    format!("gazelle-audio-{target}.zip")
}

/// A binary's file name as cargo writes it for `target`.
pub(crate) fn built_name(stem: &str, target: &str) -> String {
    if target.contains("windows") {
        format!("{stem}.exe")
    } else {
        stem.to_string()
    }
}

/// Every file name the release directory holds for `target`, and nothing else.
pub fn expected(target: &str) -> BTreeSet<String> {
    let mut names: BTreeSet<String> = BINARIES.iter().map(|stem| binary_asset_name(stem, target)).collect();
    names.insert(zip_name(target));
    names.insert(MANUAL.to_string());
    names.insert(CHEAT_SHEET.to_string());
    names
}

/// Write the release directory `out` and return its files, sorted.
pub fn dist(out: &Path, bin_dir: &Path, docs: &Path, target: &str) -> Result<Vec<PathBuf>, String> {
    let binaries: Vec<(PathBuf, String)> =
        BINARIES.iter().map(|stem| (bin_dir.join(built_name(stem, target)), binary_asset_name(stem, target))).collect();
    let pdfs: Vec<(PathBuf, String)> = [MANUAL, CHEAT_SHEET].iter().map(|name| (docs.join(name), name.to_string())).collect();

    // Everything must be there before anything is written, so a failure leaves no half release.
    for (source, _) in &binaries {
        if !source.is_file() {
            return Err(format!(
                "{} is missing; build the release first: cargo build --release -p gazelle-audio-server --features window",
                source.display()
            ));
        }
    }
    for (source, _) in &pdfs {
        if !source.is_file() {
            return Err(format!("{} is missing; build the docs first: pnpm -C web docs:pdf", source.display()));
        }
    }

    if out.exists() {
        let mut entries = std::fs::read_dir(out).map_err(|e| format!("reading {}: {e}", out.display()))?;
        if entries.next().is_some() {
            return Err(format!(
                "{} is not empty; everything in the release directory is signed and uploaded, so dist writes only into a new or empty one",
                out.display()
            ));
        }
    } else {
        std::fs::create_dir_all(out).map_err(|e| format!("creating {}: {e}", out.display()))?;
    }

    for (source, name) in binaries.iter().chain(&pdfs) {
        std::fs::copy(source, out.join(name)).map_err(|e| format!("copying {} to {}: {e}", source.display(), out.join(name).display()))?;
    }
    let sources: Vec<PathBuf> = binaries.iter().map(|(source, _)| source.clone()).collect();
    zip(&sources, &out.join(zip_name(target)))?;

    let mut found = BTreeSet::new();
    for entry in std::fs::read_dir(out).map_err(|e| format!("reading {}: {e}", out.display()))? {
        let entry = entry.map_err(|e| format!("reading {}: {e}", out.display()))?;
        found.insert(entry.file_name().to_string_lossy().into_owned());
    }
    let wanted = expected(target);
    if found != wanted {
        return Err(format!("{} holds {found:?}, not exactly {wanted:?}", out.display()));
    }
    Ok(wanted.iter().map(|name| out.join(name)).collect())
}

/// Zip `files` under their own names into `out`, with PowerShell's `Compress-Archive`. The paths
/// travel in environment variables, so no quoting of a path ever reaches a command line.
pub fn zip(files: &[PathBuf], out: &Path) -> Result<(), String> {
    let shell = if cfg!(windows) { "powershell.exe" } else { "pwsh" };
    let script = "$ErrorActionPreference = 'Stop'; $ProgressPreference = 'SilentlyContinue'; \
                  Compress-Archive -LiteralPath ($env:GAZELLE_ZIP_FILES -split [char]10) \
                  -DestinationPath $env:GAZELLE_ZIP_OUT -CompressionLevel Optimal";
    let list = files.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join("\n");
    let run = Command::new(shell)
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .env("GAZELLE_ZIP_FILES", list)
        .env("GAZELLE_ZIP_OUT", out)
        .output()
        .map_err(|e| format!("running {shell} to write the zip: {e}"))?;
    if !run.status.success() || !out.is_file() {
        return Err(format!(
            "Compress-Archive did not write {} ({}): {}",
            out.display(),
            run.status,
            String::from_utf8_lossy(&run.stderr).trim()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TARGET: &str = "x86_64-pc-windows-msvc";

    struct Dir(PathBuf);

    impl Dir {
        fn new(name: &str) -> Dir {
            let dir = std::env::temp_dir().join(format!("gazelle-xtask-dist-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            for sub in ["bin", "docs"] {
                std::fs::create_dir_all(dir.join(sub)).unwrap();
            }
            std::fs::write(dir.join("bin/gazelle-audio-server.exe"), b"console build").unwrap();
            std::fs::write(dir.join("bin/gazelle-audio-serverw.exe"), b"windowless build").unwrap();
            // What else a real target/release holds, none of which belongs in a release.
            std::fs::write(dir.join("bin/xtask.exe"), b"not shipped").unwrap();
            std::fs::write(dir.join("bin/gazelle_audio_server.pdb"), b"not shipped").unwrap();
            std::fs::write(dir.join("docs").join(MANUAL), b"%PDF manual").unwrap();
            std::fs::write(dir.join("docs").join(CHEAT_SHEET), b"%PDF cheat sheet").unwrap();
            Dir(dir)
        }

        fn run(&self) -> Result<Vec<PathBuf>, String> {
            dist(&self.0.join("out"), &self.0.join("bin"), &self.0.join("docs"), TARGET)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn names(paths: &[PathBuf]) -> Vec<String> {
        paths.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn the_names_are_the_ones_the_spec_and_the_updater_use() {
        assert_eq!(
            expected(TARGET).into_iter().collect::<Vec<_>>(),
            [
                "gazelle-audio-server-x86_64-pc-windows-msvc.exe",
                "gazelle-audio-serverw-x86_64-pc-windows-msvc.exe",
                "gazelle-audio-x86_64-pc-windows-msvc.zip",
                "gazelle-cheat-sheet.pdf",
                "gazelle-manual.pdf",
            ]
        );
        // The updater asks for these two by name; dist must write the same.
        for stem in BINARIES {
            assert!(expected(TARGET).contains(&gazelle_audio_server::update::release::binary_asset_name(stem, TARGET)));
        }
    }

    #[cfg(windows)]
    #[test]
    fn dist_writes_exactly_the_release_and_the_zip_holds_both_binaries_by_their_plain_names() {
        let dir = Dir::new("ok");
        let written = dir.run().unwrap();
        assert_eq!(names(&written), expected(TARGET).into_iter().collect::<Vec<_>>());
        let out = dir.0.join("out");
        assert_eq!(std::fs::read(out.join("gazelle-audio-server-x86_64-pc-windows-msvc.exe")).unwrap(), b"console build");
        assert_eq!(std::fs::read(out.join("gazelle-audio-serverw-x86_64-pc-windows-msvc.exe")).unwrap(), b"windowless build");
        assert_eq!(std::fs::read(out.join(MANUAL)).unwrap(), b"%PDF manual");

        let unzipped = dir.0.join("unzipped");
        let expand = Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", "Expand-Archive -LiteralPath $env:Z -DestinationPath $env:D"])
            .env("Z", out.join(zip_name(TARGET)))
            .env("D", &unzipped)
            .status()
            .unwrap();
        assert!(expand.success());
        let mut inside: Vec<String> =
            std::fs::read_dir(&unzipped).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        inside.sort();
        assert_eq!(inside, ["gazelle-audio-server.exe", "gazelle-audio-serverw.exe"]);
        assert_eq!(std::fs::read(unzipped.join("gazelle-audio-serverw.exe")).unwrap(), b"windowless build");
    }

    #[test]
    fn a_directory_that_already_holds_anything_is_refused_untouched() {
        let dir = Dir::new("not-empty");
        let out = dir.0.join("out");
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join("gazelle-release.key"), b"anything").unwrap();
        assert!(dir.run().unwrap_err().contains("is not empty"));
        assert_eq!(std::fs::read(out.join("gazelle-release.key")).unwrap(), b"anything");
        assert_eq!(std::fs::read_dir(&out).unwrap().count(), 1, "nothing was added");
    }

    #[test]
    fn a_missing_binary_or_pdf_is_named_with_how_to_make_it_and_nothing_is_written() {
        let dir = Dir::new("missing-pdf");
        std::fs::remove_file(dir.0.join("docs").join(CHEAT_SHEET)).unwrap();
        let error = dir.run().unwrap_err();
        assert!(error.contains(CHEAT_SHEET) && error.contains("docs:pdf"), "{error}");
        assert!(!dir.0.join("out").exists(), "no half release is left behind");

        let dir = Dir::new("missing-binary");
        std::fs::remove_file(dir.0.join("bin/gazelle-audio-serverw.exe")).unwrap();
        let error = dir.run().unwrap_err();
        assert!(error.contains("gazelle-audio-serverw.exe") && error.contains("cargo build --release"), "{error}");
        assert!(!dir.0.join("out").exists());
    }
}
