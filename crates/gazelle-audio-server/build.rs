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
//! Add/Remove Programs entry and the window, and the application manifest, which says both run
//! as whoever started them rather than asking for elevation. See `build/resource.rs`.
//!
//! And it chooses the aggregate driver a release carries: the DLL `GAZELLE_AGGREGATE_DLL` names,
//! or nothing when it is unset, which is every build but a release. A build asked to carry one
//! that it cannot use stops here with the reason. See `build/driver.rs`.

#[path = "build/driver.rs"]
mod driver;
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
    embed_the_driver(&manifest);
}

/// Put the aggregate driver's bytes, or no bytes, where `aggregate::bundled` includes them.
///
/// A file is always written, so the library compiles the same way either way and an empty one
/// means "this build carries no driver". Asked for a driver it cannot use, the build fails: the
/// panic's message is the reason, and cargo prints it.
fn embed_the_driver(manifest: &std::path::Path) {
    println!("cargo:rerun-if-env-changed={}", driver::VAR);
    let value = std::env::var_os(driver::VAR);
    // The workspace root, two folders up from this package.
    let root = manifest.ancestors().nth(2).unwrap_or(manifest).to_path_buf();
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join(driver::EMBEDDED_NAME);
    let bytes = match driver::choose(value.as_deref(), &root, &os, &arch) {
        Ok(Some((path, bytes))) => {
            println!("cargo:rerun-if-changed={}", path.display());
            bytes
        }
        Ok(None) => Vec::new(),
        Err(why) => panic!("the aggregate driver cannot be carried: {why}"),
    };
    std::fs::write(&out, bytes).unwrap_or_else(|e| panic!("writing {}: {e}", out.display()));
}

/// Put the icon and the application manifest in every binary this package builds: both of them.
///
/// `rustc-link-arg-bins` reaches the two `bin` targets and nothing else, which is what is
/// wanted: a test executable has no use for an icon. A failure here is a warning rather than a
/// broken build (the app runs perfectly well with the default executable icon), and a release
/// that quietly lost either resource is caught by `tests/icon.rs` instead.
fn embed_the_icon(manifest: &std::path::Path) {
    let ico = manifest.join("assets/gazelle.ico");
    let app_manifest = manifest.join("assets/gazelle.manifest");
    println!("cargo:rerun-if-changed={}", ico.display());
    println!("cargo:rerun-if-changed={}", app_manifest.display());
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let object = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("gazelle-icon.o");
    match resource::write_object(&ico, &app_manifest, &object, &arch) {
        Ok(()) => println!("cargo:rustc-link-arg-bins={}", object.display()),
        Err(e) => println!("cargo:warning=the icon and the manifest were not embedded: {e}"),
    }
}
