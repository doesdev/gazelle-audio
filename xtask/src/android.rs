//! The Android app, Gazelle Remote, in the release: its version code, and signing its APK.
//!
//! The APK is built unsigned by the release workflow's `android` job, which holds no secret. The
//! `sign` job, in the protected `release` environment, signs it with `sign-apk` into the release
//! directory as `Gazelle-Remote.apk`, **before** `xtask sign`, so `SHA256SUMS` covers it. The
//! updater never asks for it.
//!
//! `sign-apk` runs the Android SDK's own tools, from its newest `build-tools` directory (under
//! `ANDROID_HOME` or `ANDROID_SDK_ROOT`, which GitHub's runners set, or `--build-tools <dir>`):
//!
//! 1. `aapt2 dump badging`: the APK must be this app (`io.github.doesdev.gazelle.remote`), with
//!    the version name the server has and the version code that version gives;
//! 2. `zipalign -p 4`, into a scratch directory outside the release directory;
//! 3. `apksigner sign` with the PKCS12 keystore named by `--keystore`, its passwords read by
//!    apksigner itself from `ANDROID_KEYSTORE_PASSWORD` and `ANDROID_KEY_PASSWORD` (the store's
//!    when that is unset), so no password is ever on a command line;
//! 4. `apksigner verify --print-certs`, whose certificate digests it prints.
//!
//! A keystore inside the release directory is refused, as `sign` refuses a key there: everything
//! in it is uploaded. A failure removes whatever was written.

use std::path::{Path, PathBuf};
use std::process::Command;

use semver::Version;

/// The app's identity: an installed copy accepts an update only with this and the same key.
pub const APPLICATION_ID: &str = "io.github.doesdev.gazelle.remote";
/// The keystore's password, read by apksigner from this variable.
pub const STORE_PASSWORD_ENV: &str = "ANDROID_KEYSTORE_PASSWORD";
/// The key's password, read by apksigner from this variable; the store's when unset.
pub const KEY_PASSWORD_ENV: &str = "ANDROID_KEY_PASSWORD";
/// The key's alias in the keystore, unless `--alias` says otherwise.
pub const DEFAULT_ALIAS: &str = "gazelle-remote";

/// Android's version code for a Gazelle version: major * 10000 + minor * 100 + patch, so every
/// release is higher than the one before. A pre-release has its release's code (Android accepts
/// the same code again as an update), and minor and patch must stay under 100 to keep the order.
pub fn version_code(version: &str) -> Result<u32, String> {
    let parsed = Version::parse(version).map_err(|e| format!("{version:?} is not a semver version: {e}"))?;
    if parsed.minor >= 100 || parsed.patch >= 100 {
        return Err(format!("{version}: the Android version code needs minor and patch under 100"));
    }
    // Google Play's ceiling for a version code is 2100000000; nothing near it is ever reached.
    let code = parsed.major.checked_mul(10_000).map(|m| m + parsed.minor * 100 + parsed.patch).filter(|&c| c <= 2_100_000_000);
    code.map(|c| c as u32).ok_or_else(|| format!("{version}: the major version is too large for an Android version code"))
}

/// What `aapt2 dump badging` says of an APK: its package name, version code and version name.
#[derive(Debug, PartialEq, Eq)]
pub struct Badging {
    pub package: String,
    pub version_code: String,
    pub version_name: String,
}

/// Read the `package:` line of `aapt2 dump badging`:
/// `package: name='io.github...' versionCode='10500' versionName='1.5.0' ...`.
pub fn parse_badging(output: &str) -> Option<Badging> {
    let line = output.lines().find(|l| l.starts_with("package:"))?;
    let field = |name: &str| {
        let start = line.find(&format!(" {name}='"))? + name.len() + 3;
        let end = line[start..].find('\'')?;
        Some(line[start..start + end].to_string())
    };
    Some(Badging { package: field("name")?, version_code: field("versionCode")?, version_name: field("versionName")? })
}

/// Check the badging is this app at `version`.
pub fn check_badging(badging: &Badging, version: &str) -> Result<(), String> {
    if badging.package != APPLICATION_ID {
        return Err(format!("the APK is {}, not {APPLICATION_ID}", badging.package));
    }
    if badging.version_name != version {
        return Err(format!("the APK's version name is {}, but this release is {version}", badging.version_name));
    }
    let code = version_code(version)?;
    if badging.version_code != code.to_string() {
        return Err(format!("the APK's version code is {}, but {version} gives {code}", badging.version_code));
    }
    Ok(())
}

/// The newest `build-tools/<version>` under an Android SDK: numeric versions only, compared as
/// numbers, so `36.0.0` beats `9.0.0` and a `36.1.0-rc1` is passed over.
pub fn newest_build_tools(sdk: &Path) -> Result<PathBuf, String> {
    let dir = sdk.join("build-tools");
    let entries = std::fs::read_dir(&dir).map_err(|e| format!("reading {}: {e}", dir.display()))?;
    let mut best: Option<(Vec<u64>, PathBuf)> = None;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(numbers) = name.split('.').map(|p| p.parse::<u64>().ok()).collect::<Option<Vec<u64>>>() else { continue };
        if best.as_ref().is_none_or(|(b, _)| numbers > *b) {
            best = Some((numbers, entry.path()));
        }
    }
    best.map(|(_, path)| path).ok_or_else(|| format!("{} holds no build-tools version", dir.display()))
}

/// The SDK's build-tools: `--build-tools`, else the newest under `ANDROID_HOME` or `ANDROID_SDK_ROOT`.
fn build_tools(given: Option<PathBuf>) -> Result<PathBuf, String> {
    if let Some(dir) = given {
        return Ok(dir);
    }
    let sdk = ["ANDROID_HOME", "ANDROID_SDK_ROOT"]
        .iter()
        .find_map(|name| std::env::var_os(name).filter(|v| !v.is_empty()))
        .ok_or("no Android SDK: set ANDROID_HOME (GitHub's runners do), or pass --build-tools <dir>")?;
    newest_build_tools(Path::new(&sdk))
}

/// A tool's file name in build-tools: `apksigner.bat`, `zipalign.exe` and `aapt2.exe` on Windows.
fn tool(dir: &Path, name: &str) -> PathBuf {
    if cfg!(windows) {
        dir.join(if name == "apksigner" { format!("{name}.bat") } else { format!("{name}.exe") })
    } else {
        dir.join(name)
    }
}

/// Run `command` and return its standard output, or say what failed.
fn run(command: &mut Command, what: &str) -> Result<String, String> {
    let output = command.output().map_err(|e| format!("running {what}: {e}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    if !output.status.success() {
        return Err(format!("{what} failed ({}): {}{}", output.status, stdout.trim(), String::from_utf8_lossy(&output.stderr).trim()));
    }
    Ok(stdout)
}

pub struct SignApk {
    pub apk: PathBuf,
    pub out: PathBuf,
    pub keystore: PathBuf,
    pub alias: String,
    pub version: String,
    pub build_tools: Option<PathBuf>,
}

/// Check, align, sign and verify the APK into `out`. See the module docs.
pub fn sign_apk(options: &SignApk) -> Result<(), String> {
    let SignApk { apk, out, keystore, alias, version, build_tools: given } = options;
    if !apk.is_file() {
        return Err(format!("{} is missing; build it with gradle :app:assembleRelease", apk.display()));
    }
    if !keystore.is_file() {
        return Err(format!("the keystore {} is missing", keystore.display()));
    }
    if out.exists() {
        return Err(format!("{} already exists; a signed APK is never overwritten", out.display()));
    }
    let release_dir = out.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    if !release_dir.is_dir() {
        return Err(format!("{} is not a directory", release_dir.display()));
    }
    if crate::is_inside(keystore, release_dir) {
        return Err(format!(
            "the keystore {} is inside {}, and everything in there is hashed and uploaded; move it out",
            keystore.display(),
            release_dir.display()
        ));
    }
    if std::env::var_os(STORE_PASSWORD_ENV).is_none_or(|v| v.is_empty()) {
        return Err(format!("{STORE_PASSWORD_ENV} is not set; it must hold the keystore's password"));
    }
    let tools = build_tools(given.clone())?;

    let badging = run(Command::new(tool(&tools, "aapt2")).args(["dump", "badging"]).arg(apk), "aapt2 dump badging")?;
    let found = parse_badging(&badging).ok_or("aapt2 dump badging printed no package line")?;
    check_badging(&found, version)?;

    let scratch = std::env::temp_dir().join(format!("gazelle-sign-apk-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).map_err(|e| format!("creating {}: {e}", scratch.display()))?;
    let aligned = scratch.join("aligned.apk");
    let ks_pass = format!("env:{STORE_PASSWORD_ENV}");
    let key_pass = format!("env:{KEY_PASSWORD_ENV}");
    let result = (|| -> Result<(), String> {
        run(Command::new(tool(&tools, "zipalign")).args(["-f", "-p", "4"]).arg(apk).arg(&aligned), "zipalign")?;
        let mut sign = Command::new(tool(&tools, "apksigner"));
        sign.arg("sign")
            .arg("--ks")
            .arg(keystore)
            .args(["--ks-type", "PKCS12", "--ks-key-alias", alias.as_str()])
            .args(["--ks-pass", ks_pass.as_str(), "--key-pass", key_pass.as_str()])
            .arg("--out")
            .arg(out)
            .arg(&aligned);
        // The key's password is the store's unless it was given one of its own.
        if std::env::var_os(KEY_PASSWORD_ENV).is_none_or(|v| v.is_empty()) {
            if let Some(store) = std::env::var_os(STORE_PASSWORD_ENV) {
                sign.env(KEY_PASSWORD_ENV, store);
            }
        }
        run(&mut sign, "apksigner sign")?;
        let certs = run(Command::new(tool(&tools, "apksigner")).args(["verify", "--verbose", "--print-certs"]).arg(out), "apksigner verify")?;
        println!("signed {} as {} ({}, version {version}, code {})", apk.display(), out.display(), APPLICATION_ID, found.version_code);
        for line in certs.lines().filter(|l| l.starts_with("Verified using") || l.contains("certificate SHA-256 digest")) {
            println!("    {line}");
        }
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&scratch);
    if result.is_err() {
        let _ = std::fs::remove_file(out);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_version_code_orders_every_release() {
        assert_eq!(version_code("1.4.1"), Ok(10401));
        assert_eq!(version_code("1.5.0"), Ok(10500));
        assert_eq!(version_code("2.0.0"), Ok(20000));
        assert_eq!(version_code("0.0.1"), Ok(1));
        // A release candidate has its release's code.
        assert_eq!(version_code("1.5.0-rc.1"), Ok(10500));
        assert!(version_code("1.4.1").unwrap() < version_code("1.4.2").unwrap());
        assert!(version_code("1.99.99").unwrap() < version_code("2.0.0").unwrap());
        assert!(version_code("1.100.0").unwrap_err().contains("under 100"));
        assert!(version_code("1.0.100").unwrap_err().contains("under 100"));
        assert!(version_code("999999.0.0").unwrap_err().contains("too large"));
        assert!(version_code("v1.0.0").is_err());
    }

    #[test]
    fn the_badging_line_is_read_and_checked() {
        let output = "package: name='io.github.doesdev.gazelle.remote' versionCode='10500' versionName='1.5.0' platformBuildVersionName='16'\n\
                      sdkVersion:'26'\n";
        let badging = parse_badging(output).unwrap();
        assert_eq!(
            badging,
            Badging { package: APPLICATION_ID.into(), version_code: "10500".into(), version_name: "1.5.0".into() }
        );
        assert_eq!(check_badging(&badging, "1.5.0"), Ok(()));
        assert!(check_badging(&badging, "1.5.1").unwrap_err().contains("version name is 1.5.0"));

        let wrong_code = Badging { version_code: "1".into(), ..parse_badging(output).unwrap() };
        assert!(check_badging(&wrong_code, "1.5.0").unwrap_err().contains("version code is 1"));
        let other = Badging { package: "com.example".into(), ..parse_badging(output).unwrap() };
        assert!(check_badging(&other, "1.5.0").unwrap_err().contains("com.example"));
        assert_eq!(parse_badging("sdkVersion:'26'\n"), None);
    }

    #[test]
    fn the_newest_numeric_build_tools_is_chosen() {
        let sdk = std::env::temp_dir().join(format!("gazelle-xtask-sdk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&sdk);
        for version in ["9.0.0", "34.0.0", "36.0.0", "35.0.1", "36.1.0-rc1", "debian"] {
            std::fs::create_dir_all(sdk.join("build-tools").join(version)).unwrap();
        }
        let chosen = newest_build_tools(&sdk).unwrap();
        assert_eq!(chosen.file_name().unwrap(), "36.0.0");
        std::fs::remove_dir_all(sdk.join("build-tools")).unwrap();
        std::fs::create_dir_all(sdk.join("build-tools")).unwrap();
        assert!(newest_build_tools(&sdk).unwrap_err().contains("no build-tools"));
        let _ = std::fs::remove_dir_all(&sdk);
    }

    #[test]
    fn a_keystore_inside_the_release_directory_is_refused_before_any_tool_runs() {
        let dir = std::env::temp_dir().join(format!("gazelle-xtask-sign-apk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("release")).unwrap();
        std::fs::write(dir.join("app.apk"), b"PK").unwrap();
        std::fs::write(dir.join("release/store.p12"), b"not a keystore").unwrap();
        let options = SignApk {
            apk: dir.join("app.apk"),
            out: dir.join("release/Gazelle-Remote.apk"),
            keystore: dir.join("release/store.p12"),
            alias: DEFAULT_ALIAS.into(),
            version: "1.5.0".into(),
            // A build-tools directory that does not exist: nothing may get as far as running one.
            build_tools: Some(dir.join("no-build-tools")),
        };
        let error = sign_apk(&options).unwrap_err();
        assert!(error.contains("is inside"), "{error}");
        assert!(!dir.join("release/Gazelle-Remote.apk").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
