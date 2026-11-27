//! Secrets: values that are a credential to whoever holds them.
//!
//! Session tokens, the tokens of invitations, the read tokens of shared boards and the names files
//! are stored under are secrets. They do not come from the identifier generator, whose
//! values start with the time and continue a counter within a millisecond; they are 256
//! bits from the operating system's random number generator.

use std::fmt::Write as _;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest as _, Sha256};

/// The number of random bytes in a secret.
const SECRET_BYTES: usize = 32;

/// The length of a secret's text: 32 bytes in unpadded base64url.
pub const SECRET_LENGTH: usize = 43;

/// Returns a new secret: 43 characters of the URL-safe base64 alphabet.
///
/// # Panics
///
/// Panics if the operating system cannot supply random bytes. There is nothing to fall
/// back on that would still be a secret.
pub fn new_secret() -> String {
    let mut bytes = [0_u8; SECRET_BYTES];
    getrandom::fill(&mut bytes)
        .expect("the operating system's random number generator supplies bytes");
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Returns the digest under which a session token is stored and looked up: its SHA-256,
/// in lower-case hexadecimal.
///
/// A token is 256 random bits, so nothing slower than a plain digest is needed: there
/// is no guessing a token from its digest. What the digest buys is that the stored
/// value is no credential. Whoever reads the sessions table, or a backup of it, cannot
/// present what is there as a token.
pub fn token_digest(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        // Writing to a `String` cannot fail.
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_is_43_url_safe_characters() {
        let secret = new_secret();
        assert_eq!(secret.len(), SECRET_LENGTH, "secret {secret}");
        assert!(
            secret
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'),
            "secret {secret}"
        );
    }

    #[test]
    fn a_token_digest_is_the_sha_256_of_the_token_in_hexadecimal() {
        // The test vector of FIPS 180-2 for "abc".
        assert_eq!(
            token_digest("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let token = new_secret();
        assert_eq!(
            token_digest(&token),
            token_digest(&token),
            "it is a function"
        );
        assert_ne!(token_digest(&token), token, "and not the token");
    }

    #[test]
    fn secrets_differ_and_are_no_identifiers() {
        let secrets: std::collections::HashSet<String> = (0..100).map(|_| new_secret()).collect();
        assert_eq!(secrets.len(), 100, "no two secrets are the same");
        assert!(
            secrets.iter().all(|secret| !fvoci_model::is_id(secret)),
            "a secret does not have the form of an identifier"
        );
    }
}
