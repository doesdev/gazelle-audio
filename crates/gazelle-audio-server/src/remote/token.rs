//! The secrets: phone tokens and pairing codes, and how a token is kept and checked.
//!
//! **Tokens** are 32 bytes (256 bits) from the operating system's CSPRNG, written as 64 hex
//! digits. Only their SHA-256 is kept, in memory and in `remote.json`, so the file is worth
//! nothing to someone who reads it: a hash cannot be sent back as a token. A plain hash rather
//! than a slow password hash is right here because the token is random and long, not chosen by a
//! person, so there is no dictionary to try. Hashes are compared in constant time, and every
//! stored hash is compared on every check, so how long a check takes says nothing about how close
//! a guess was or which phone it nearly matched.
//!
//! **Pairing codes** are 8 characters from a 32 character alphabet (40 bits), shown as
//! `XXXX-XXXX`. The alphabet is Crockford's base 32, which leaves out I, L, O and U, and reading
//! one back folds the look-alikes (`O` to `0`, `I` and `L` to `1`) and ignores case, spaces and
//! dashes, so a code typed from the screen is forgiving. A code lives 5 minutes, is used once,
//! and dies after a handful of wrong tries (`pairing`), so the 2^40 possibilities are far more
//! than a guesser can get through: even at ten thousand guesses a second for the whole 5 minutes,
//! with no limit on wrong tries, the chance of hitting it is about 1 in 370,000.

use subtle::ConstantTimeEq;

/// How many random bytes make a token.
pub const TOKEN_BYTES: usize = 32;

/// The characters a pairing code is made of: Crockford's base 32.
pub const CODE_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// How many characters a pairing code has, not counting the dash.
pub const CODE_LENGTH: usize = 8;

/// Random bytes from the operating system. An error here means the system cannot give any, which
/// is not something to paper over with a weaker source: the caller refuses what it was doing.
pub fn random<const N: usize>() -> Result<[u8; N], String> {
    let mut bytes = [0u8; N];
    getrandom::fill(&mut bytes).map_err(|e| format!("the system's random number source failed: {e}"))?;
    Ok(bytes)
}

/// A new token, as the phone is given it.
pub fn new_token() -> Result<String, String> {
    Ok(crate::update::release::to_hex(&random::<TOKEN_BYTES>()?))
}

/// What is kept of a token: its SHA-256.
pub fn hash(token: &str) -> [u8; 32] {
    crate::update::verify::sha256_bytes(token.as_bytes())
}

/// Two hashes compared without an early exit.
pub fn same(a: &[u8], b: &[u8]) -> bool {
    a.ct_eq(b).into()
}

/// A short id for a paired phone, for the list and the revoke route. Not a secret.
pub fn new_phone_id() -> Result<String, String> {
    Ok(crate::update::release::to_hex(&random::<8>()?))
}

/// A new pairing code, `XXXX-XXXX`.
pub fn new_code() -> Result<String, String> {
    // 256 is a multiple of 32, so taking the low five bits of each byte is uniform.
    let bytes = random::<CODE_LENGTH>()?;
    Ok(format_code(bytes.iter().map(|b| CODE_ALPHABET[usize::from(b & 31)] as char)))
}

fn format_code(chars: impl Iterator<Item = char>) -> String {
    let mut code = String::with_capacity(CODE_LENGTH + 1);
    for (i, c) in chars.enumerate() {
        if i == CODE_LENGTH / 2 {
            code.push('-');
        }
        code.push(c);
    }
    code
}

/// A code as a person or a phone sent it, read back to the form it was shown in, or `None` when
/// it cannot be a code at all.
pub fn normalise_code(input: &str) -> Option<String> {
    let mut chars = Vec::with_capacity(CODE_LENGTH);
    for c in input.chars() {
        let c = match c.to_ascii_uppercase() {
            ' ' | '-' => continue,
            'O' => '0',
            'I' | 'L' => '1',
            c => c,
        };
        if !c.is_ascii() || !CODE_ALPHABET.contains(&(c as u8)) || chars.len() == CODE_LENGTH {
            return None;
        }
        chars.push(c);
    }
    (chars.len() == CODE_LENGTH).then(|| format_code(chars.into_iter()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_is_256_random_bits_in_hex_and_no_two_are_alike() {
        let a = new_token().unwrap();
        let b = new_token().unwrap();
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[test]
    fn only_the_hash_is_compared_and_it_matches_only_itself() {
        let token = new_token().unwrap();
        assert!(same(&hash(&token), &hash(&token)));
        assert!(!same(&hash(&token), &hash(&new_token().unwrap())));
        assert!(!same(&hash(&token), &hash(&token)[..31]), "a shorter slice is not the same");
    }

    #[test]
    fn a_code_is_eight_characters_of_the_alphabet_with_a_dash_in_the_middle() {
        for _ in 0..50 {
            let code = new_code().unwrap();
            assert_eq!(code.len(), 9, "{code}");
            assert_eq!(&code[4..5], "-");
            assert!(code.chars().filter(|&c| c != '-').all(|c| CODE_ALPHABET.contains(&(c as u8))), "{code}");
            assert_eq!(normalise_code(&code).as_deref(), Some(code.as_str()), "a code reads back as itself");
        }
    }

    #[test]
    fn a_code_typed_from_the_screen_is_read_forgivingly() {
        assert_eq!(normalise_code("abcd-ef12").as_deref(), Some("ABCD-EF12"));
        assert_eq!(normalise_code(" ab cd ef 12 ").as_deref(), Some("ABCD-EF12"));
        assert_eq!(normalise_code("ABCDEF12").as_deref(), Some("ABCD-EF12"));
        // The look-alikes the alphabet leaves out.
        assert_eq!(normalise_code("OIL0-0000").as_deref(), Some("0110-0000"));
        assert_eq!(normalise_code("ABCD-EF1"), None, "too short");
        assert_eq!(normalise_code("ABCD-EF123"), None, "too long");
        assert_eq!(normalise_code("ABCD-EF1U"), None, "U is not in the alphabet");
        assert_eq!(normalise_code("ABCD-EF1é"), None);
        assert_eq!(normalise_code(""), None);
    }
}
