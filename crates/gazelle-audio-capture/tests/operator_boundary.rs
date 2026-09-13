//! Spec §5/§9: only the panel can mark done, redo or skip. `OperatorAuthority::grant` is the
//! only way to obtain the proof those commands require; it may be called only by the panel,
//! by the synthetic operator, and by in-crate unit test files (`*_tests.rs`).

use std::fs;
use std::path::{Path, PathBuf};

const ALLOWED: &[&str] = &["src/panel/", "src/synth/", "src/session/authority.rs"];

fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn operator_authority_is_granted_only_by_the_panel_and_synthetic_operator() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut all = Vec::new();
    files(&root.join("src"), &mut all);
    let offenders: Vec<String> = all
        .iter()
        .filter(|p| fs::read_to_string(p).unwrap().contains("OperatorAuthority::grant"))
        .map(|p| p.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/"))
        .filter(|rel| !rel.ends_with("_tests.rs") && !ALLOWED.iter().any(|a| rel.starts_with(a)))
        .collect();
    assert!(offenders.is_empty(), "OperatorAuthority granted outside the operator boundary: {offenders:?}");
}
