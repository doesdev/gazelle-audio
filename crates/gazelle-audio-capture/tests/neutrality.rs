//! The crate stays vendor-neutral and independent of the other Gazelle crates.

use std::fs;
use std::path::{Path, PathBuf};

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn rust_and_asset_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("read src dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            rust_and_asset_files(&path, out);
        } else {
            out.push(path);
        }
    }
}

#[test]
fn manifest_has_no_gazelle_audio_dependency() {
    // Stricter than a simple "starts with" check on dependency lines: any
    // occurrence of "gazelle-audio-" in the manifest other than the crate's
    // own `name = "gazelle-audio-capture"` line fails. This also catches
    // `[dependencies.gazelle-audio-*]` table headers, `package = "gazelle-audio-*"`
    // renames, and occurrences under [dev-dependencies]/[build-dependencies].
    let manifest = fs::read_to_string(crate_dir().join("Cargo.toml")).expect("read Cargo.toml");
    let offending: Vec<&str> = manifest
        .lines()
        .filter(|l| l.contains("gazelle-audio-") && l.trim() != "name = \"gazelle-audio-capture\"")
        .collect();
    assert!(offending.is_empty(), "gazelle-audio-* dependencies found: {offending:?}");
}

#[test]
fn sources_contain_no_vendor_name() {
    // Built by concatenation so this test file does not trip its own check if moved into src/.
    let needle = ["ante", "lope"].concat();
    let mut files = Vec::new();
    rust_and_asset_files(&crate_dir().join("src"), &mut files);
    assert!(!files.is_empty());
    let offending: Vec<String> = files
        .iter()
        .filter(|p| {
            let bytes = fs::read(p).expect("read source file");
            String::from_utf8_lossy(&bytes).to_lowercase().contains(&needle)
        })
        .map(|p| p.display().to_string())
        .collect();
    assert!(offending.is_empty(), "vendor name found in: {offending:?}");
}
