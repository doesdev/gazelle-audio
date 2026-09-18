//! Check a signed release directory the way an installed copy will check it, before uploading.
//!
//! The checks are the updater's own functions from `gazelle_audio_server::update`, not a second
//! implementation of them: `verify::verify_signature` (ed25519, `verify_strict`),
//! `release::parse_sums` and `verify::sha256_file`. So a directory this passes is one the
//! updater accepts, as far as the files go. On top of what the updater needs, it is strict about
//! the directory itself, because what is in it is exactly what gets uploaded:
//!
//! - every line of `SHA256SUMS` parses, and no name appears twice;
//! - every file in the directory is listed, and every listed file is there, with that digest;
//! - both binaries the updater asks for on this target are there.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use gazelle_audio_server::update::release::{binary_asset_name, parse_sums, to_hex, SIGNATURE_NAME, SUMS_NAME};
use gazelle_audio_server::update::verify::{sha256_file, verify_signature};
use gazelle_audio_server::update::BINARIES;

/// Verify `dir` against `pubkey`, and return the files to upload: every asset, then the sums
/// file and its signature.
pub fn verify(dir: &Path, pubkey: &str, target: &str) -> Result<Vec<PathBuf>, String> {
    let read = |name: &str| std::fs::read(dir.join(name)).map_err(|e| format!("reading {}: {e}", dir.join(name).display()));
    let sums_bytes = read(SUMS_NAME)?;
    let signature = read(SIGNATURE_NAME)?;

    verify_signature(pubkey, &sums_bytes, &signature).map_err(|failure| failure.detail)?;

    let text = String::from_utf8(sums_bytes).map_err(|_| format!("{SUMS_NAME} is not UTF-8"))?;
    let sums = parse_sums(&text);
    let lines = text.lines().filter(|l| !l.trim().is_empty()).count();
    if lines != sums.len() {
        return Err(format!("{SUMS_NAME} has {lines} lines but {} distinct entries parse; a line is malformed or a name is doubled", sums.len()));
    }

    let mut on_disk = BTreeSet::new();
    let entries = std::fs::read_dir(dir).map_err(|e| format!("reading {}: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("reading {}: {e}", dir.display()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == SUMS_NAME || name == SIGNATURE_NAME {
            continue;
        }
        if !entry.path().is_file() {
            return Err(format!("{name} in {} is not a file; the release directory holds files only", dir.display()));
        }
        on_disk.insert(name);
    }
    let listed: BTreeSet<String> = sums.keys().cloned().collect();
    let unlisted: Vec<&String> = on_disk.difference(&listed).collect();
    if !unlisted.is_empty() {
        return Err(format!("{unlisted:?} in {} are not in {SUMS_NAME}; sign again, or take them out", dir.display()));
    }
    let missing: Vec<&String> = listed.difference(&on_disk).collect();
    if !missing.is_empty() {
        return Err(format!("{SUMS_NAME} lists {missing:?}, which are not in {}", dir.display()));
    }

    for (name, expected) in &sums {
        let digest = sha256_file(&dir.join(name)).map_err(|e| format!("reading {name}: {e}"))?;
        if &digest != expected {
            return Err(format!("{name} does not match {SUMS_NAME} (it is {}, the sums say {})", to_hex(&digest), to_hex(expected)));
        }
    }

    for stem in BINARIES {
        let asset = binary_asset_name(stem, target);
        if !sums.contains_key(&asset) {
            return Err(format!("{asset} is not in the release; the updater on {target} asks for it"));
        }
    }

    let mut upload: Vec<PathBuf> = sums.keys().map(|name| dir.join(name)).collect();
    upload.push(dir.join(SUMS_NAME));
    upload.push(dir.join(SIGNATURE_NAME));
    Ok(upload)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;

    const TARGET: &str = "x86_64-pc-windows-msvc";

    struct Dir(PathBuf);

    impl Dir {
        fn new(name: &str) -> Dir {
            let dir = std::env::temp_dir().join(format!("gazelle-xtask-verify-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(dir.join("dist")).unwrap();
            Dir(dir)
        }

        fn dist(&self) -> PathBuf {
            self.0.join("dist")
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn public(seed: u8) -> String {
        to_hex(SigningKey::from_bytes(&[seed; 32]).verifying_key().as_bytes())
    }

    /// A release directory signed by the real `sign`, with seed 3's key.
    fn signed(name: &str) -> Dir {
        let dir = Dir::new(name);
        for stem in BINARIES {
            std::fs::write(dir.dist().join(binary_asset_name(stem, TARGET)), format!("{stem} bytes")).unwrap();
        }
        std::fs::write(dir.dist().join("gazelle-manual.pdf"), b"%PDF manual").unwrap();
        let key = dir.0.join("k.key");
        std::fs::write(&key, to_hex(&[3u8; 32])).unwrap();
        crate::sign(&dir.dist(), Some(key)).unwrap();
        dir
    }

    #[test]
    fn a_directory_sign_wrote_verifies_and_lists_every_file_to_upload() {
        let dir = signed("ok");
        let upload = verify(&dir.dist(), &public(3), TARGET).unwrap();
        let names: Vec<String> = upload.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(
            names,
            [
                "gazelle-audio-server-x86_64-pc-windows-msvc.exe",
                "gazelle-audio-serverw-x86_64-pc-windows-msvc.exe",
                "gazelle-manual.pdf",
                SUMS_NAME,
                SIGNATURE_NAME
            ]
        );
    }

    #[test]
    fn another_public_key_is_refused() {
        let dir = signed("wrong-key");
        assert!(verify(&dir.dist(), &public(4), TARGET).unwrap_err().contains("not made by the release signing key"));
        assert!(verify(&dir.dist(), "not a key", TARGET).unwrap_err().contains("32 hex-encoded bytes"));
    }

    #[test]
    fn one_byte_changed_in_an_asset_is_refused_by_name() {
        let dir = signed("tampered");
        let asset = dir.dist().join("gazelle-manual.pdf");
        let mut bytes = std::fs::read(&asset).unwrap();
        bytes[0] ^= 1;
        std::fs::write(&asset, bytes).unwrap();
        assert!(verify(&dir.dist(), &public(3), TARGET).unwrap_err().contains("gazelle-manual.pdf does not match"));
    }

    #[test]
    fn an_edited_sums_file_or_signature_is_refused() {
        let dir = signed("sums-edited");
        let sums = dir.dist().join(SUMS_NAME);
        let text = std::fs::read_to_string(&sums).unwrap();
        // One hex digit of the first digest changed, as a tampered mirror might serve it.
        let first = if text.starts_with('0') { "1" } else { "0" };
        std::fs::write(&sums, format!("{first}{}", &text[1..])).unwrap();
        assert!(verify(&dir.dist(), &public(3), TARGET).unwrap_err().contains("not made by the release signing key"));

        let dir = signed("sig-short");
        std::fs::write(dir.dist().join(SIGNATURE_NAME), [0u8; 63]).unwrap();
        assert!(verify(&dir.dist(), &public(3), TARGET).unwrap_err().contains("63 bytes"));
    }

    #[test]
    fn a_file_added_or_removed_after_signing_is_refused() {
        let dir = signed("added");
        std::fs::write(dir.dist().join("extra.txt"), b"x").unwrap();
        assert!(verify(&dir.dist(), &public(3), TARGET).unwrap_err().contains("extra.txt"));

        let dir = signed("removed");
        std::fs::remove_file(dir.dist().join("gazelle-manual.pdf")).unwrap();
        assert!(verify(&dir.dist(), &public(3), TARGET).unwrap_err().contains("which are not in"));
    }

    #[test]
    fn a_release_without_the_binaries_for_the_target_is_refused() {
        let dir = signed("other-target");
        let error = verify(&dir.dist(), &public(3), "aarch64-pc-windows-msvc").unwrap_err();
        assert!(error.contains("gazelle-audio-server-aarch64-pc-windows-msvc.exe is not in the release"), "{error}");
    }

    #[test]
    fn a_malformed_or_doubled_sums_line_is_refused_even_when_signed() {
        for extra in ["not a sums line\n", "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  gazelle-manual.pdf\n"] {
            let dir = signed("malformed");
            let sums = dir.dist().join(SUMS_NAME);
            let mut text = std::fs::read_to_string(&sums).unwrap();
            text.push_str(extra);
            std::fs::write(&sums, &text).unwrap();
            // Signed again over the edited sums, so only the strictness can catch it.
            let signature = ed25519_dalek::Signer::sign(&SigningKey::from_bytes(&[3u8; 32]), text.as_bytes()).to_bytes();
            std::fs::write(dir.dist().join(SIGNATURE_NAME), signature).unwrap();
            assert!(verify(&dir.dist(), &public(3), TARGET).unwrap_err().contains("malformed or a name is doubled"), "{extra}");
        }
    }
}
