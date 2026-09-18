//! Release chores, so cutting a release is not guesswork.
//!
//! ```text
//! cargo run -p xtask -- keygen --out <outside-the-repo>/gazelle-release.key
//! cargo run -p xtask -- check-version 1.0.0
//! cargo run -p xtask -- release-notes 1.0.0
//! cargo run -p xtask -- smoke --pubkey <64 hex digits>
//! cargo run -p xtask -- dist --out dist
//! cargo run -p xtask -- pubkey --key <file> --expect <64 hex digits>
//! cargo run -p xtask -- sign --dir dist [--key <file>]
//! cargo run -p xtask -- verify --dir dist --pubkey <64 hex digits>
//! ```
//!
//! `keygen` makes an ed25519 pair, writes the **private** half to a file outside the repository
//! and prints the public half. `sign` hashes every file in a directory into `SHA256SUMS`, signs
//! that file, and prints what to upload. The rest are the steps of the release checklist
//! (`.agent/specs/2026-09-18-shipping-portable.md`) that are more than one command, so that the
//! release workflow (`.github/workflows/release.yml`) and a person cutting a release by hand run
//! the same code, and that code is under test.
//!
//! # The key
//!
//! The private key is 64 hex digits in a file. It never goes in the repository, and this tool
//! refuses to write one inside a git working tree. It reaches `sign` and `pubkey` by `--key
//! <file>`, or by `GAZELLE_RELEASE_KEY` naming that file, never as a value on the command line,
//! where it would land in shell history and process listings. `sign` refuses a key file that
//! sits inside the directory it signs, and any file there that looks like a key, because
//! everything in that directory is hashed and uploaded.
//!
//! The public half is compiled into the server:
//!
//! ```text
//! GAZELLE_UPDATE_PUBKEY=<64 hex digits> cargo build --release -p gazelle-audio-server
//! ```
//!
//! A binary built without it will not download an update, because it could not check one.

mod dist;
mod notes;
mod smoke;
mod verify;

use std::path::{Path, PathBuf};

use ed25519_dalek::{Signer, SigningKey};
use sha2::{Digest, Sha256};

const SUMS: &str = "SHA256SUMS";
const SIGNATURE: &str = "SHA256SUMS.sig";
const KEY_ENV: &str = "GAZELLE_RELEASE_KEY";

const USAGE: &str = "\
gazelle release helper

    xtask keygen --out <file>        make a signing key pair; prints the public half
    xtask check-version [<tag>]      the tag must be the server's version, bare (1.0.0, no v);
                                     prints version=, tag= and prerelease= lines
    xtask release-notes <version> [--changelog <file>]
                                     print that version's CHANGELOG.md section; fails if it is
                                     missing or empty
    xtask smoke [--bin-dir <dir>] [--version <v>] [--pubkey <hex>] [--target <triple>]
                                     start both built binaries on the loopback backend and check
                                     what they answer: version, can_verify, the web UI
    xtask dist --out <dir> [--bin-dir <dir>] [--docs <dir>] [--target <triple>]
                                     collect the release directory under the names the updater
                                     asks for, with the zip and the PDFs
    xtask pubkey [--key <file>] [--expect <hex>]
                                     print the public half of the signing key; fails if it is
                                     not the expected one
    xtask sign --dir <dir> [--key <file>]
                                     write SHA256SUMS over <dir>, sign it, say what to upload
    xtask verify --dir <dir> --pubkey <hex> [--target <triple>]
                                     check <dir> as the updater will; prints the files to upload

The private key is read from --key, or from the file named by GAZELLE_RELEASE_KEY.
Defaults: --bin-dir target/release, --docs docs/dist, --changelog CHANGELOG.md (all relative to
the current directory), --version the server's own, --target the triple this helper was built for.
";

fn main() {
    if let Err(e) = run(std::env::args().skip(1).collect()) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run(args: Vec<String>) -> Result<(), String> {
    let target = || value(&args, "--target").unwrap_or_else(|| gazelle_audio_server::update::TARGET.to_string());
    match args.first().map(String::as_str) {
        None | Some("help" | "--help" | "-h") => {
            print!("{USAGE}");
            Ok(())
        }
        Some("keygen") => {
            only(&args, &["--out"])?;
            keygen(&flag(&args, "--out").ok_or("keygen needs --out <file>")?)
        }
        Some("sign") => {
            only(&args, &["--dir", "--key"])?;
            sign(&flag(&args, "--dir").ok_or("sign needs --dir <dir>")?, flag(&args, "--key"))
        }
        Some("pubkey") => {
            only(&args, &["--key", "--expect"])?;
            println!("{}", pubkey(flag(&args, "--key"), value(&args, "--expect").as_deref())?);
            Ok(())
        }
        Some("check-version") => {
            only(&args, &[])?;
            let report = notes::check_version(positional(&args), gazelle_audio_server::VERSION)?;
            print!("{}", report.lines());
            Ok(())
        }
        Some("release-notes") => {
            only(&args, &["--changelog"])?;
            let version = positional(&args).ok_or("release-notes needs a version, e.g. 1.0.0")?;
            let file = flag(&args, "--changelog").unwrap_or_else(|| PathBuf::from("CHANGELOG.md"));
            let text = std::fs::read_to_string(&file).map_err(|e| format!("reading {}: {e}", file.display()))?;
            print!("{}", notes::section(&text, version).map_err(|e| format!("{}: {e}", file.display()))?);
            Ok(())
        }
        Some("dist") => {
            only(&args, &["--out", "--bin-dir", "--docs", "--target"])?;
            let out = flag(&args, "--out").ok_or("dist needs --out <dir>")?;
            let bin_dir = flag(&args, "--bin-dir").unwrap_or_else(|| PathBuf::from("target/release"));
            let docs = flag(&args, "--docs").unwrap_or_else(|| PathBuf::from("docs/dist"));
            for path in dist::dist(&out, &bin_dir, &docs, &target())? {
                println!("{}", path.display());
            }
            Ok(())
        }
        Some("verify") => {
            only(&args, &["--dir", "--pubkey", "--target"])?;
            let dir = flag(&args, "--dir").ok_or("verify needs --dir <dir>")?;
            let pubkey = value(&args, "--pubkey").ok_or("verify needs --pubkey <64 hex digits>")?;
            // The list on stdout is exactly what to upload, so a workflow can hand it straight on.
            for path in verify::verify(&dir, &pubkey, &target())? {
                println!("{}", path.display());
            }
            Ok(())
        }
        Some("smoke") => {
            only(&args, &["--bin-dir", "--version", "--pubkey", "--target"])?;
            let bin_dir = flag(&args, "--bin-dir").unwrap_or_else(|| PathBuf::from("target/release"));
            let version = value(&args, "--version").unwrap_or_else(|| gazelle_audio_server::VERSION.to_string());
            smoke::smoke(&bin_dir, &version, value(&args, "--pubkey").as_deref(), &target())
        }
        // A typo in a workflow must fail the step, not print the usage and pass.
        Some(other) => Err(format!("unknown command {other:?}; run xtask help for the list")),
    }
}

/// The value after `name`, if it is there.
fn flag(args: &[String], name: &str) -> Option<PathBuf> {
    value(args, name).map(PathBuf::from)
}

fn value(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

/// The argument after the command, when it is not a flag: `check-version 1.0.0`.
fn positional(args: &[String]) -> Option<&str> {
    args.get(1).map(String::as_str).filter(|a| !a.starts_with("--"))
}

/// Refuse any flag the command does not take, so a misspelt one is an error rather than a
/// default quietly used in its place.
fn only(args: &[String], allowed: &[&str]) -> Result<(), String> {
    match args.iter().skip(1).find(|a| a.starts_with("--") && !allowed.contains(&a.as_str())) {
        Some(unknown) => Err(format!("{} does not take {unknown}", args[0])),
        None => Ok(()),
    }
}

fn keygen(out: &Path) -> Result<(), String> {
    if out.exists() {
        return Err(format!("{} already exists; a signing key is never overwritten", out.display()));
    }
    if in_a_repository(out) {
        return Err(format!("{} is inside a git working tree; keep the signing key outside the repository", out.display()));
    }
    let mut seed = [0u8; 32];
    getrandom::fill(&mut seed).map_err(|e| format!("no randomness available: {e}"))?;
    let key = SigningKey::from_bytes(&seed);
    if let Some(dir) = out.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    }
    std::fs::write(out, format!("{}\n", to_hex(&seed))).map_err(|e| format!("writing {}: {e}", out.display()))?;
    restrict(out);
    println!("private key: {} (back it up; losing it means no more updates for installed copies)", out.display());
    println!();
    println!("public key, to build the server with:");
    println!("    GAZELLE_UPDATE_PUBKEY={} cargo build --release -p gazelle-audio-server", to_hex(key.verifying_key().as_bytes()));
    Ok(())
}

fn sign(dir: &Path, key_file: Option<PathBuf>) -> Result<(), String> {
    let path = key_path(key_file)?;
    if is_inside(&path, dir) {
        return Err(format!(
            "the signing key {} is inside {}, and everything in there is hashed and uploaded; move the key out",
            path.display(),
            dir.display()
        ));
    }
    let key = read_key(Some(path))?;
    let assets = assets(dir)?;
    if assets.is_empty() {
        return Err(format!("{} holds no files to sign", dir.display()));
    }
    let sums = sums_file(&assets);
    let signature = key.sign(sums.as_bytes()).to_bytes();
    std::fs::write(dir.join(SUMS), &sums).map_err(|e| format!("writing {SUMS}: {e}"))?;
    std::fs::write(dir.join(SIGNATURE), signature).map_err(|e| format!("writing {SIGNATURE}: {e}"))?;

    println!("signed with the key whose public half is {}", to_hex(key.verifying_key().as_bytes()));
    println!("(the server only accepts this release if it was built with GAZELLE_UPDATE_PUBKEY set to that)");
    println!();
    println!("upload all of these to the release, and nothing else:");
    for (name, _) in &assets {
        println!("    {}", dir.join(name).display());
    }
    println!("    {}", dir.join(SUMS).display());
    println!("    {}", dir.join(SIGNATURE).display());
    Ok(())
}

/// Every file in the directory that is not itself an output of this tool, by name and digest,
/// sorted so the sums file is the same whatever order the directory is read in.
fn assets(dir: &Path) -> Result<Vec<(String, [u8; 32])>, String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("reading {}: {e}", dir.display()))?;
    let mut assets = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| format!("reading {}: {e}", dir.display()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == SUMS || name == SIGNATURE || !entry.path().is_file() {
            continue;
        }
        let bytes = std::fs::read(entry.path()).map_err(|e| format!("reading {name}: {e}"))?;
        if looks_like_a_key(&bytes) {
            return Err(format!(
                "{name} in {} holds 64 hex digits and nothing else, which is what a signing key file looks like; \
                 nothing like that is ever uploaded, so it was not signed",
                dir.display()
            ));
        }
        assets.push((name, Sha256::digest(&bytes).into()));
    }
    assets.sort();
    Ok(assets)
}

/// `SHA256SUMS` as `sha256sum` writes it: digest, two spaces, name.
fn sums_file(assets: &[(String, [u8; 32])]) -> String {
    assets.iter().map(|(name, digest)| format!("{}  {name}\n", to_hex(digest))).collect()
}

/// A private key file as `keygen` writes it: 64 hex digits, and at most whitespace around them.
fn looks_like_a_key(bytes: &[u8]) -> bool {
    std::str::from_utf8(bytes).ok().and_then(|text| from_hex(text.trim())).is_some()
}

/// Whether `path` is somewhere under `dir`, comparing the paths as the file system resolves them.
fn is_inside(path: &Path, dir: &Path) -> bool {
    match (path.canonicalize(), dir.canonicalize()) {
        (Ok(path), Ok(dir)) => path.starts_with(dir),
        _ => false,
    }
}

/// The public half of the signing key, checked against the one the binaries are built with.
///
/// The release workflow runs this with `--expect` set to the `GAZELLE_UPDATE_PUBKEY` it built
/// with before it signs anything: a mismatch would publish binaries that can never verify an
/// update signed by this key, and nothing downstream would notice until the next release.
fn pubkey(key_file: Option<PathBuf>, expect: Option<&str>) -> Result<String, String> {
    let public = to_hex(read_key(key_file)?.verifying_key().as_bytes());
    match expect.map(|e| e.trim().to_ascii_lowercase()) {
        None => Ok(public),
        Some(expected) if expected.is_empty() => {
            Err("the expected public key is empty (is the GAZELLE_UPDATE_PUBKEY variable set?)".to_string())
        }
        Some(expected) if expected == public => Ok(public),
        Some(expected) => Err(format!(
            "the signing key's public half is {public}, but the binaries are built with {expected}; \
             they could never verify an update signed by this key. Set GAZELLE_UPDATE_PUBKEY to the key's public half"
        )),
    }
}

/// The key file: `--key`, or the file `GAZELLE_RELEASE_KEY` names.
fn key_path(from: Option<PathBuf>) -> Result<PathBuf, String> {
    from.or_else(|| std::env::var_os(KEY_ENV).filter(|v| !v.is_empty()).map(PathBuf::from))
        .ok_or(format!("no signing key: pass --key <file> or set {KEY_ENV} to the file holding it"))
}

fn read_key(from: Option<PathBuf>) -> Result<SigningKey, String> {
    let path = key_path(from)?;
    let text = std::fs::read_to_string(&path).map_err(|e| format!("reading the signing key {}: {e}", path.display()))?;
    let bytes = from_hex(text.trim()).ok_or_else(|| format!("{} is not 32 hex-encoded bytes", path.display()))?;
    Ok(SigningKey::from_bytes(&bytes))
}

/// Whether the path would land inside a git working tree, which a private key never should.
fn in_a_repository(path: &Path) -> bool {
    let start = path.parent().filter(|d| !d.as_os_str().is_empty()).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    let mut dir = start.canonicalize().unwrap_or(start);
    loop {
        if dir.join(".git").exists() {
            return true;
        }
        match dir.parent() {
            Some(parent) => dir = parent.to_path_buf(),
            None => return false,
        }
    }
}

/// Take the key file's permissions down to its owner where the platform has any.
#[cfg(unix)]
fn restrict(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict(_path: &Path) {}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn from_hex(hex: &str) -> Option<[u8; 32]> {
    let bytes = hex.as_bytes();
    if bytes.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, pair) in bytes.as_chunks::<2>().0.iter().enumerate() {
        out[i] = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gazelle_audio_server::update::release::parse_sums;
    use gazelle_audio_server::update::verify::verify_signature;

    struct Dir(PathBuf);

    impl Dir {
        fn new(name: &str) -> Dir {
            let dir = std::env::temp_dir().join(format!("gazelle-xtask-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(dir.join("dist")).unwrap();
            Dir(dir)
        }

        /// The release directory. The key lives outside it: everything in here gets signed.
        fn dist(&self) -> PathBuf {
            self.0.join("dist")
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn key_file(dir: &Dir, seed: u8) -> PathBuf {
        let path = dir.0.join("signing.key");
        std::fs::write(&path, format!("{}\n", to_hex(&[seed; 32]))).unwrap();
        path
    }

    /// The whole point: what this writes is what the app accepts.
    #[test]
    fn what_is_signed_here_is_what_the_server_verifies() {
        let dir = Dir::new("roundtrip");
        std::fs::write(dir.dist().join("gazelle-audio-server-x86_64-pc-windows-msvc.exe"), b"the console build").unwrap();
        std::fs::write(dir.dist().join("gazelle-audio-serverw-x86_64-pc-windows-msvc.exe"), b"the windowless build").unwrap();
        let key = key_file(&dir, 11);

        sign(&dir.dist(), Some(key)).unwrap();

        let sums = std::fs::read(dir.dist().join(SUMS)).unwrap();
        let signature = std::fs::read(dir.dist().join(SIGNATURE)).unwrap();
        let public = to_hex(SigningKey::from_bytes(&[11u8; 32]).verifying_key().as_bytes());
        assert_eq!(verify_signature(&public, &sums, &signature), Ok(()));

        let parsed = parse_sums(&String::from_utf8(sums).unwrap());
        assert_eq!(parsed.len(), 2);
        assert_eq!(
            parsed["gazelle-audio-server-x86_64-pc-windows-msvc.exe"],
            <[u8; 32]>::from(Sha256::digest(b"the console build"))
        );
    }

    #[test]
    fn signing_again_over_the_same_directory_writes_the_same_sums() {
        let dir = Dir::new("idempotent");
        std::fs::write(dir.dist().join("b.exe"), b"b").unwrap();
        std::fs::write(dir.dist().join("a.exe"), b"a").unwrap();
        let key = key_file(&dir, 5);

        sign(&dir.dist(), Some(key.clone())).unwrap();
        let first = std::fs::read(dir.dist().join(SUMS)).unwrap();
        sign(&dir.dist(), Some(key)).unwrap();

        assert_eq!(std::fs::read(dir.dist().join(SUMS)).unwrap(), first, "the sums file must not depend on directory order");
        // Its own outputs are never listed in it, however many times it runs.
        let text = String::from_utf8(first).unwrap();
        assert_eq!(text.lines().map(|l| l.split_at(66).1).collect::<Vec<_>>(), ["a.exe", "b.exe"]);
        assert!(!text.contains(SUMS));
    }

    #[test]
    fn the_key_comes_from_a_file_named_by_a_flag_or_the_environment_and_never_from_a_value() {
        let dir = Dir::new("key");
        let path = key_file(&dir, 3);
        assert_eq!(read_key(Some(path.clone())).unwrap().to_bytes(), [3u8; 32]);

        let missing = read_key(None);
        // With no flag and (in this process) no variable set, it says how to give it one.
        if std::env::var_os(KEY_ENV).is_none() {
            assert!(missing.unwrap_err().contains(KEY_ENV));
        }

        std::fs::write(&path, "not a key").unwrap();
        assert!(read_key(Some(path)).unwrap_err().contains("32 hex-encoded bytes"));
    }

    #[test]
    fn signing_an_empty_or_missing_directory_says_so_rather_than_signing_nothing() {
        let dir = Dir::new("empty");
        let key = key_file(&dir, 1);
        assert!(sign(&dir.dist(), Some(key.clone())).unwrap_err().contains("no files to sign"));
        assert!(sign(&dir.0.join("nowhere"), Some(key)).unwrap_err().contains("reading"));
    }

    #[test]
    fn a_key_is_never_written_inside_the_repository_or_over_an_existing_one() {
        let dir = Dir::new("keygen");
        let inside = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("do-not-write-me.key");
        assert!(keygen(&inside).unwrap_err().contains("git working tree"));
        assert!(!inside.exists());

        let path = dir.0.join("new.key");
        keygen(&path).unwrap();
        let first = std::fs::read_to_string(&path).unwrap();
        assert_eq!(from_hex(first.trim()).map(|k| k.len()), Some(32));
        assert!(keygen(&path).unwrap_err().contains("already exists"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), first, "a second keygen must not overwrite the first");
    }

    #[test]
    fn a_key_inside_the_directory_being_signed_is_refused_and_nothing_is_written() {
        let dir = Dir::new("key-inside");
        std::fs::write(dir.dist().join("a.exe"), b"a").unwrap();
        let inside = dir.dist().join("signing.key");
        std::fs::write(&inside, format!("{}\n", to_hex(&[4u8; 32]))).unwrap();

        assert!(sign(&dir.dist(), Some(inside)).unwrap_err().contains("is inside"));
        assert!(!dir.dist().join(SUMS).exists(), "nothing is signed when the key is in the directory");
    }

    #[test]
    fn a_file_that_looks_like_a_key_is_never_signed_whatever_it_is_called() {
        let dir = Dir::new("key-lookalike");
        std::fs::write(dir.dist().join("a.exe"), b"a").unwrap();
        std::fs::write(dir.dist().join("notes.txt"), format!("  {}\r\n", to_hex(&[9u8; 32]).to_uppercase())).unwrap();
        let key = key_file(&dir, 4);

        assert!(sign(&dir.dist(), Some(key.clone())).unwrap_err().contains("notes.txt"));
        assert!(!dir.dist().join(SUMS).exists());

        // Hex that is not exactly a key's length is an ordinary file.
        std::fs::write(dir.dist().join("notes.txt"), to_hex(&[9u8; 31])).unwrap();
        sign(&dir.dist(), Some(key)).unwrap();
    }

    #[test]
    fn pubkey_prints_the_public_half_and_fails_hard_on_any_other_expected_key() {
        let dir = Dir::new("pubkey");
        let key = key_file(&dir, 21);
        let public = to_hex(SigningKey::from_bytes(&[21u8; 32]).verifying_key().as_bytes());

        assert_eq!(pubkey(Some(key.clone()), None).unwrap(), public);
        // As a variable might hold it: upper case, a trailing newline.
        assert_eq!(pubkey(Some(key.clone()), Some(&format!("{}\n", public.to_uppercase()))).unwrap(), public);

        let other = to_hex(SigningKey::from_bytes(&[22u8; 32]).verifying_key().as_bytes());
        let mismatch = pubkey(Some(key.clone()), Some(&other)).unwrap_err();
        assert!(mismatch.contains(&public) && mismatch.contains(&other), "{mismatch}");
        assert!(!mismatch.contains(&to_hex(&[21u8; 32])), "the private half is never printed");

        assert!(pubkey(Some(key), Some("")).unwrap_err().contains("empty"));
    }

    #[test]
    fn an_unknown_command_or_flag_is_an_error_not_the_usage() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(run(args(&["sing", "--dir", "x"])).unwrap_err().contains("unknown command"));
        assert!(run(args(&["verify", "--dir", "x", "--pubkey", "y", "--taget", "z"])).unwrap_err().contains("--taget"));
        assert!(run(args(&[])).is_ok());
        assert!(run(args(&["help"])).is_ok());
    }

    #[test]
    fn hex_round_trips() {
        let bytes: [u8; 32] = std::array::from_fn(|i| (i * 7) as u8);
        assert_eq!(from_hex(&to_hex(&bytes)), Some(bytes));
        assert_eq!(from_hex("00"), None);
    }
}
