//! Chooses the directory the `web-ui` feature embeds, and records what this binary was built
//! for and against.
//!
//! The Rust build never runs pnpm. If the web UI has been built, its `dist/` is embedded;
//! otherwise a one-page notice is, so `cargo build` and `cargo test` work without Node.
//!
//! The updater needs two things only the build knows: the **target triple**, so it can ask a
//! release for the asset built for this platform, and whether `GAZELLE_UPDATE_PUBKEY` was set,
//! so a change of signing key rebuilds rather than being cached.

use std::path::PathBuf;

const NOT_BUILT: &str = "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\
<title>Gazelle</title></head>\n<body><h1>Web UI not built</h1>\
<p>Run <code>pnpm -C web build</code>, then rebuild the server.</p></body></html>\n";

fn main() {
    println!("cargo:rerun-if-env-changed=GAZELLE_UPDATE_PUBKEY");
    // Nothing in `std` exposes the target triple to the crate; a build script is given it.
    println!("cargo:rustc-env=GAZELLE_TARGET={}", std::env::var("TARGET").unwrap_or_else(|_| "unknown".into()));

    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let dist = manifest.join("../../web/apps/web/dist");
    // A missing path counts as changed, so this also re-runs until the UI is first built.
    println!("cargo:rerun-if-changed={}", dist.display());

    let folder = if dist.join("index.html").is_file() {
        dist.canonicalize().unwrap()
    } else {
        let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("web-not-built");
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join("index.html"), NOT_BUILT).unwrap();
        out
    };
    println!("cargo:rustc-env=GAZELLE_WEB_DIST={}", folder.display());
}
