//! The two checks an update must pass, and the key the second one is made against.
//!
//! The hash says the file arrived whole and unchanged; only the signature says the release
//! source is the one we trust. Neither alone is enough, so both are run and a failure of either
//! is fatal to the download; `update/mod.rs` deletes the file and reports.

use std::io::Read;
use std::path::Path;

use ed25519_dalek::{Signature, VerifyingKey};
use sha2::{Digest, Sha256};

use super::{Failure, Summary};

/// The release signing key, hex-encoded, baked in at build time:
/// `GAZELLE_UPDATE_PUBKEY=<64 hex digits> cargo build --release`.
///
/// A build without one cannot verify anything, so it refuses to download rather than applying
/// an unverified file. `refs/tools/scripts/README.md` says how to make the pair.
pub fn built_in_public_key() -> Option<&'static str> {
    option_env!("GAZELLE_UPDATE_PUBKEY").filter(|k| !k.is_empty())
}

/// SHA-256 of a file, read in chunks so a large binary is never held in memory.
pub fn sha256_file(path: &Path) -> std::io::Result<[u8; 32]> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize().into())
}

pub fn sha256_bytes(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

/// Check a detached ed25519 signature over `message` against a hex public key.
///
/// `verify_strict` rather than `verify`: it rejects small-order and non-canonical keys and
/// signatures, so one signature cannot be made to verify under two keys.
///
/// Every way this can go wrong is one thing to the person looking (the download could not be
/// verified), so they all carry [`Summary::Unverified`], and which of them it was stays in the
/// detail, where whoever is diagnosing it can read it.
pub fn verify_signature(public_key_hex: &str, message: &[u8], signature: &[u8]) -> Result<(), Failure> {
    let key_bytes = super::release::from_hex(public_key_hex.trim())
        .ok_or_else(|| Summary::Unverified.with("the built-in release signing key is not 32 hex-encoded bytes"))?;
    let key = VerifyingKey::from_bytes(&key_bytes)
        .map_err(|e| Summary::Unverified.with(format!("the built-in release signing key is not a valid ed25519 key: {e}")))?;
    let signature: [u8; 64] = signature.try_into().map_err(|_| {
        Summary::Unverified.with(format!("the signature is {} bytes, not the 64 an ed25519 signature is", signature.len()))
    })?;
    key.verify_strict(message, &Signature::from_bytes(&signature))
        .map_err(|_| Summary::Unverified.with("the signature over SHA256SUMS was not made by the release signing key"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::update::release::to_hex;
    use ed25519_dalek::{Signer, SigningKey};

    fn key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    #[test]
    fn a_files_digest_matches_the_same_bytes_hashed_in_one_go() {
        let dir = std::env::temp_dir().join(format!("gazelle-update-verify-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("payload.bin");
        // Bigger than one read buffer, so the chunked loop is what is being tested.
        let bytes: Vec<u8> = (0..200_000u32).map(|i| i as u8).collect();
        std::fs::write(&path, &bytes).unwrap();
        assert_eq!(sha256_file(&path).unwrap(), sha256_bytes(&bytes));

        std::fs::write(&path, b"").unwrap();
        assert_eq!(to_hex(&sha256_file(&path).unwrap()), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_signature_by_the_built_in_key_verifies() {
        let signing = key(7);
        let public = to_hex(signing.verifying_key().as_bytes());
        let message = b"deadbeef  gazelle-audio-server-x86_64-pc-windows-msvc.exe\n";
        let signature = signing.sign(message).to_bytes();
        assert_eq!(verify_signature(&public, message, &signature), Ok(()));
    }

    #[test]
    fn a_signature_by_another_key_does_not() {
        let message = b"SHA256SUMS";
        let signature = key(9).sign(message).to_bytes();
        let public = to_hex(key(7).verifying_key().as_bytes());
        let failure = verify_signature(&public, message, &signature).unwrap_err();
        assert_eq!(failure.summary, Summary::Unverified, "one thing to the person looking: it could not be verified");
        assert!(failure.detail.contains("not made by the release signing key"), "{}", failure.detail);
    }

    #[test]
    fn a_signature_over_other_bytes_does_not() {
        let signing = key(7);
        let public = to_hex(signing.verifying_key().as_bytes());
        let signature = signing.sign(b"the sums as released").to_bytes();
        assert!(verify_signature(&public, b"the sums as edited", &signature).is_err());
    }

    #[test]
    fn a_signature_of_the_wrong_length_is_named_as_such() {
        let public = to_hex(key(7).verifying_key().as_bytes());
        let failure = verify_signature(&public, b"x", &[0u8; 63]).unwrap_err();
        assert_eq!(failure.summary, Summary::Unverified);
        assert!(failure.detail.contains("63 bytes"), "{}", failure.detail);
        assert!(verify_signature(&public, b"x", &[]).is_err());
    }

    #[test]
    fn a_key_that_is_not_a_key_is_named_as_such() {
        let signature = key(7).sign(b"x").to_bytes();
        for key in ["", "nonsense"] {
            let failure = verify_signature(key, b"x", &signature).unwrap_err();
            assert_eq!(failure.summary, Summary::Unverified);
            assert!(failure.detail.contains("32 hex-encoded bytes"), "{}", failure.detail);
        }
    }

    /// A build with no key set must not pretend it has one.
    #[test]
    fn a_build_without_a_key_has_none() {
        assert_eq!(built_in_public_key(), option_env!("GAZELLE_UPDATE_PUBKEY").filter(|k| !k.is_empty()));
    }
}
