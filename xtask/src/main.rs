//! Release chores, so cutting a release is not guesswork.
//!
//! ```text
//! cargo run -p xtask -- keygen --out <outside-the-repo>/gazelle-release.key
//! cargo run -p xtask -- sign --dir dist [--key <file>]
//! ```
//!
//! `keygen` makes an ed25519 pair, writes the **private** half to a file outside the repository
//! and prints the public half. `sign` hashes every file in a directory into `SHA256SUMS`, signs
//! that file, and prints what to upload.
//!
//! # The key
//!
//! The private key is 64 hex digits in a file. It never goes in the repository, and this tool
//! refuses to write one inside a git working tree. It reaches `sign` by `--key <file>`, or by
//! `GAZELLE_RELEASE_KEY` naming that file — never as a value on the command line, where it would
//! land in shell history and process listings.
//!
//! The public half is compiled into the server:
//!
//! ```text
//! GAZELLE_UPDATE_PUBKEY=<64 hex digits> cargo build --release -p gazelle-audio-server
//! ```
//!
//! A binary built without it will not download an update, because it could not check one.

use std::path::{Path, PathBuf};

use ed25519_dalek::{Signer, SigningKey};
use sha2::{Digest, Sha256};

const SUMS: &str = "SHA256SUMS";
const SIGNATURE: &str = "SHA256SUMS.sig";
const KEY_ENV: &str = "GAZELLE_RELEASE_KEY";

const USAGE: &str = "\
gazelle release helper

    xtask keygen --out <file>        make a signing key pair; prints the public half
    xtask sign --dir <dir> [--key <file>]
                                     write SHA256SUMS over <dir>, sign it, say what to upload

The private key is read from --key, or from the file named by GAZELLE_RELEASE_KEY.
";

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("keygen") => keygen(&flag(&args, "--out").ok_or("keygen needs --out <file>")?),
        Some("sign") => sign(&flag(&args, "--dir").ok_or("sign needs --dir <dir>")?, flag(&args, "--key")),
        _ => {
            print!("{USAGE}");
            Ok(())
        }
    }
}

/// The value after `name`, if it is there.
fn flag(args: &[String], name: &str) -> Option<PathBuf> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(PathBuf::from)
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
    println!("private key: {} — back it up; losing it means no more updates for installed copies", out.display());
    println!();
    println!("public key, to build the server with:");
    println!("    GAZELLE_UPDATE_PUBKEY={} cargo build --release -p gazelle-audio-server", to_hex(key.verifying_key().as_bytes()));
    Ok(())
}

fn sign(dir: &Path, key_file: Option<PathBuf>) -> Result<(), String> {
    let key = read_key(key_file)?;
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
        assets.push((name, Sha256::digest(&bytes).into()));
    }
    assets.sort();
    Ok(assets)
}

/// `SHA256SUMS` as `sha256sum` writes it: digest, two spaces, name.
fn sums_file(assets: &[(String, [u8; 32])]) -> String {
    assets.iter().map(|(name, digest)| format!("{}  {name}\n", to_hex(digest))).collect()
}

fn read_key(from: Option<PathBuf>) -> Result<SigningKey, String> {
    let path = from
        .or_else(|| std::env::var_os(KEY_ENV).map(PathBuf::from))
        .ok_or(format!("no signing key: pass --key <file> or set {KEY_ENV} to the file holding it"))?;
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
    for (i, pair) in bytes.chunks_exact(2).enumerate() {
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
    fn hex_round_trips() {
        let bytes: [u8; 32] = std::array::from_fn(|i| (i * 7) as u8);
        assert_eq!(from_hex(&to_hex(&bytes)), Some(bytes));
        assert_eq!(from_hex("00"), None);
    }
}
