//! No em dash (U+2014) and no en dash (U+2013) anywhere in the product's source.
//!
//! The user asked for none, anywhere: not in what a person sees (the tray, the window, the web
//! UI, notices, log lines, CLI help) and, so that this check can be total rather than a judgement
//! about which strings are visible, not in comments either. An escape or entity that renders as
//! one counts too. Each one found is reported by file and line, so a failure says exactly what to
//! rewrite.
//!
//! The roots are everything that is built into, or served by, the app: the web app and the client
//! package, every crate's sources and build script, the release helper, the themes, and the
//! schemas the server embeds. Tests, `.agent/` and `refs/` notes are not product. Generated files
//! (the effect catalogues and mic emulations from `refs/tools/scripts/`) sit inside these roots,
//! so a generator that emitted a dash again would fail here too; each generator also refuses to
//! write one.
//!
//! The minus sign (U+2212, as in "-inf dB" written properly) is a mathematical sign, not a dash,
//! and is allowed.

use std::path::{Path, PathBuf};

/// The dashes themselves, and each way the product's languages can spell one without writing it:
/// a JavaScript or JSON escape (what `json.dumps` writes), a Rust escape, an HTML entity. Matched
/// against the line lower-cased.
const NEEDLES: &[&str] = &[
    "\u{2013}", "\u{2014}",
    r"–", r"—", r"\u{2013}", r"\u{2014}",
    "&ndash;", "&mdash;", "&#8211;", "&#8212;", "&#x2013;", "&#x2014;",
];

/// Directories under the repository root whose whole trees are product.
const TREES: &[&str] = &["web/apps/web/src", "web/apps/web/themes", "web/themes", "web/packages/client/src", "xtask/src"];

/// Single files under the repository root that are product.
const FILES: &[&str] = &["web/apps/web/index.html", "xtask/Cargo.toml"];

/// Never descended into, wherever they appear.
const SKIP_DIRS: &[&str] = &["node_modules", "dist", "target"];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("xtask sits in the repository root").to_path_buf()
}

/// Every root: the fixed ones, plus each crate's `src`, `build`, `build.rs` and manifest, plus
/// the schema files the server embeds. Crates are listed from disk so a new one is covered
/// without anyone remembering to add it here.
fn roots(root: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = TREES.iter().chain(FILES).map(|r| root.join(r)).collect();
    let crates = std::fs::read_dir(root.join("crates")).expect("the crates directory is readable");
    for entry in crates {
        let dir = entry.expect("a crate directory entry").path();
        for part in ["src", "build", "build.rs", "Cargo.toml"] {
            let path = dir.join(part);
            if path.exists() {
                out.push(path);
            }
        }
    }
    let schemas = std::fs::read_dir(root.join("refs/schemas")).expect("the schemas directory is readable");
    for entry in schemas {
        let path = entry.expect("a schema directory entry").path();
        if path.extension().is_some_and(|e| e == "json") {
            out.push(path);
        }
    }
    out
}

fn files_under(path: &Path, out: &mut Vec<PathBuf>) {
    if path.is_file() {
        out.push(path.to_path_buf());
        return;
    }
    let entries = std::fs::read_dir(path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    for entry in entries {
        let child = entry.expect("a directory entry").path();
        let skipped = child.file_name().and_then(|n| n.to_str()).is_some_and(|n| SKIP_DIRS.contains(&n));
        if !skipped {
            files_under(&child, out);
        }
    }
}

/// `(line, text)` for every line of `text` holding an en or em dash, or a spelling of one, lines
/// counted from 1.
fn dashes(text: &str) -> Vec<(usize, &str)> {
    text.lines()
        .enumerate()
        .filter(|(_, line)| {
            let line = line.to_lowercase();
            NEEDLES.iter().any(|needle| line.contains(needle))
        })
        .map(|(i, line)| (i + 1, line.trim()))
        .collect()
}

#[test]
fn the_product_source_has_no_en_or_em_dashes() {
    let root = repo_root();
    let mut files = Vec::new();
    for r in roots(&root) {
        assert!(r.exists(), "a product root is missing: {}", r.display());
        files_under(&r, &mut files);
    }
    // A guard that looked at nothing would pass for the wrong reason.
    assert!(files.len() > 100, "only {} files found under the product roots", files.len());

    let mut found = Vec::new();
    for file in &files {
        let bytes = std::fs::read(file).unwrap_or_else(|e| panic!("cannot read {}: {e}", file.display()));
        // Binary files (the icon, fonts) cannot hold a dash as text; skip what is not UTF-8.
        let Ok(text) = std::str::from_utf8(&bytes) else { continue };
        let shown = file.strip_prefix(&root).unwrap_or(file).display().to_string().replace('\\', "/");
        for (line, content) in dashes(text) {
            found.push(format!("{shown}:{line}: {content}"));
        }
    }
    assert!(
        found.is_empty(),
        "{} line(s) in the product hold an en dash (U+2013) or em dash (U+2014); rewrite each one:\n{}",
        found.len(),
        found.join("\n")
    );
}

#[test]
fn the_check_finds_both_dashes_and_names_the_line() {
    let text = "first\nan en \u{2013} dash\nplain - hyphen\nan em \u{2014} dash\nminus \u{2212}1";
    assert_eq!(dashes(text), vec![(2, "an en \u{2013} dash"), (4, "an em \u{2014} dash")]);
}

#[test]
fn the_check_finds_a_dash_spelled_as_an_escape_or_an_entity() {
    for spelled in [r"—", r"–", r"\u{2014}", "&mdash;", "&ndash;", "&#8211;", "&#X2014;"] {
        let text = format!("a\nb {spelled} c");
        let expected = format!("b {spelled} c");
        assert_eq!(dashes(&text), vec![(2, expected.as_str())], "{spelled}");
    }
    // Neighbours that are not en or em dashes: the minus sign, a hyphen, a double hyphen.
    assert!(dashes(r"− &minus; ‐ - --").is_empty());
}

#[test]
fn the_roots_include_every_crate_and_the_embedded_schemas() {
    let root = repo_root();
    let roots = roots(&root);
    for expected in [
        "crates/gazelle-audio-server/src",
        "crates/gazelle-audio-server/build",
        "crates/gazelle-audio-server/build.rs",
        "crates/gazelle-audio-capture/src",
        "crates/gazelle-audio-protocol/src",
        "crates/gazelle-audio-transport/src",
        "refs/schemas/quadro_commands.json",
        "web/apps/web/src",
        "web/packages/client/src",
    ] {
        assert!(roots.contains(&root.join(expected)), "{expected} is not checked");
    }
}
