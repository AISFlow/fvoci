//! Values the server keeps in the database and nobody who reads the database is to
//! read: sealed with the server's `secret_key` (AES-256-GCM).
//!
//! A sealed value is `v1.` and, in unpadded base64url, a random 96-bit nonce followed
//! by the ciphertext and its tag. What it is sealed for (the purpose and whose it is)
//! is authenticated with it and not stored: a value moved to another row, or another
//! organization's, does not open there.

use aws_lc_rs::aead::{AES_256_GCM, Aad, LessSafeKey, NONCE_LEN, Nonce, UnboundKey};
use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use thiserror::Error;

/// The length of the key, in bytes.
pub const KEY_BYTES: usize = 32;

/// What a sealed value begins with: the form it is in.
const VERSION: &str = "v1.";

/// The server's key for what it seals. Never written out: its `Debug` says nothing of
/// it.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretKey([u8; KEY_BYTES]);

impl std::fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretKey(..)")
    }
}

/// The text of a key is not 32 bytes in base64.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("the key must be {KEY_BYTES} bytes in base64")]
pub struct ParseSecretKeyError;

/// A sealed value did not open: it is not one, was changed, or was sealed with another
/// key or for something else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("the sealed value does not open with this key")]
pub struct OpenError;

impl SecretKey {
    /// Reads a key from its text: 32 bytes in standard base64, with or without padding,
    /// as `openssl rand -base64 32` writes them.
    ///
    /// # Errors
    ///
    /// Returns an error, which does not repeat the text, if it is no such key.
    pub fn from_base64(text: &str) -> Result<Self, ParseSecretKeyError> {
        let bytes = STANDARD
            .decode(text.trim())
            .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(text.trim()))
            .map_err(|_| ParseSecretKeyError)?;
        let key: [u8; KEY_BYTES] = bytes.try_into().map_err(|_| ParseSecretKeyError)?;
        Ok(Self(key))
    }

    fn cipher(&self) -> LessSafeKey {
        LessSafeKey::new(
            UnboundKey::new(&AES_256_GCM, &self.0).expect("a key of 32 bytes is an AES-256 key"),
        )
    }

    /// Seals `plain` for `context`, which opening it must name again.
    ///
    /// # Panics
    ///
    /// Panics if the operating system supplies no random bytes for the nonce, or the
    /// value is too long for AES-GCM (64 GiB).
    pub fn seal(&self, plain: &str, context: &[u8]) -> String {
        let mut nonce = [0_u8; NONCE_LEN];
        getrandom::fill(&mut nonce).expect("the operating system supplies random bytes");
        let mut sealed = plain.as_bytes().to_vec();
        self.cipher()
            .seal_in_place_append_tag(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(context),
                &mut sealed,
            )
            .expect("a value of this size is sealed");
        let mut bytes = nonce.to_vec();
        bytes.append(&mut sealed);
        format!("{VERSION}{}", URL_SAFE_NO_PAD.encode(bytes))
    }

    /// Opens a value sealed for `context`.
    ///
    /// # Errors
    ///
    /// Returns [`OpenError`] if it does not open.
    pub fn open(&self, sealed: &str, context: &[u8]) -> Result<String, OpenError> {
        let bytes = sealed
            .strip_prefix(VERSION)
            .and_then(|text| URL_SAFE_NO_PAD.decode(text).ok())
            .ok_or(OpenError)?;
        if bytes.len() < NONCE_LEN {
            return Err(OpenError);
        }
        let (nonce, sealed) = bytes.split_at(NONCE_LEN);
        let nonce = Nonce::try_assume_unique_for_key(nonce).map_err(|_| OpenError)?;
        let mut in_out = sealed.to_vec();
        let plain = self
            .cipher()
            .open_in_place(nonce, Aad::from(context), &mut in_out)
            .map_err(|_| OpenError)?;
        String::from_utf8(plain.to_vec()).map_err(|_| OpenError)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(byte: u8) -> SecretKey {
        SecretKey::from_base64(&STANDARD.encode([byte; KEY_BYTES])).expect("a key")
    }

    #[test]
    fn a_sealed_value_opens_with_its_key_and_context_only() {
        let sealed = key(1).seal("the client secret", b"org-a");
        assert!(!sealed.contains("client"), "{sealed}");
        assert_eq!(
            key(1).open(&sealed, b"org-a").as_deref(),
            Ok("the client secret")
        );
        assert_eq!(
            key(2).open(&sealed, b"org-a"),
            Err(OpenError),
            "another key"
        );
        assert_eq!(
            key(1).open(&sealed, b"org-b"),
            Err(OpenError),
            "another context"
        );
        let mut changed = sealed.into_bytes();
        let last = changed.len() - 1;
        changed[last] = if changed[last] == b'A' { b'B' } else { b'A' };
        let changed = String::from_utf8(changed).expect("text");
        assert_eq!(key(1).open(&changed, b"org-a"), Err(OpenError), "changed");
        assert_eq!(key(1).open("v1.", b"org-a"), Err(OpenError), "too short");
        assert_eq!(key(1).open("", b"org-a"), Err(OpenError), "nothing");
    }

    #[test]
    fn the_same_value_seals_differently_each_time() {
        assert_ne!(key(1).seal("x", b"c"), key(1).seal("x", b"c"));
    }

    #[test]
    fn a_key_is_32_bytes_of_base64_and_says_nothing_of_itself() {
        let text = STANDARD.encode([7_u8; KEY_BYTES]);
        let parsed = SecretKey::from_base64(&text).expect("a key");
        assert_eq!(format!("{parsed:?}"), "SecretKey(..)");
        for bad in ["", "abc", &STANDARD.encode([7_u8; 16]), "!!!!"] {
            let error = SecretKey::from_base64(bad).expect_err(bad);
            assert!(
                bad.is_empty() || !error.to_string().contains(bad),
                "{error}"
            );
        }
    }
}
