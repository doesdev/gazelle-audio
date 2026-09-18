//! Chooses the directory the `web-ui` feature embeds, and records what this binary was built
//! for and against.
//!
//! The Rust build never runs pnpm. If the web UI has been built, its `dist/` is embedded;
//! otherwise a one-page notice is, so `cargo build` and `cargo test` work without Node.
//!
//! The updater needs two things only the build knows: the **target triple**, so it can ask a
//! release for the asset built for this platform, and whether `GAZELLE_UPDATE_PUBKEY` was set,
//! so a change of signing key rebuilds rather than being cached.
//!
//! On Windows it also hands the linker a COFF object carrying `assets/gazelle.ico`, so both
//! binaries show the app's own icon in Explorer, the taskbar, the Start Menu shortcut, the
//! Add/Remove Programs entry and the window. See `build/resource.rs`.

#[path = "build/resource.rs"]
mod resource;

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

    embed_the_icon(&manifest);
}

/// Put the icon resource in every binary this package builds: both of them.
///
/// `rustc-link-arg-bins` reaches the two `bin` targets and nothing else, which is what is
/// wanted: a test executable has no use for an icon. A failure here is a warning rather than a
/// broken build (the app runs perfectly well with the default executable icon), and a release
/// that quietly lost it is caught by `tests/icon.rs` instead.
fn embed_the_icon(manifest: &std::path::Path) {
    let ico = manifest.join("assets/gazelle.ico");
    println!("cargo:rerun-if-changed={}", ico.display());
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let object = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("gazelle-icon.o");
    match resource::write_object(&ico, &object, &arch) {
        Ok(()) => println!("cargo:rustc-link-arg-bins={}", object.display()),
        Err(e) => println!("cargo:warning=the icon was not embedded: {e}"),
    }
}
