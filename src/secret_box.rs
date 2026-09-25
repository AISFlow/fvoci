//! Secrets sealed at rest (source `secret-box.ts`).
//!
//! AES-256-GCM with the `ENCRYPTION_KEYS` keyring. Layout
//! `enc:v2:<kid>:<base64url(iv || tag || ciphertext)>`: 12-byte nonce, 16-byte
//! tag, AAD = JSON `[header, context]`. The context binds a value to its row
//! (`webhook:<workspace>:<id>`), so a sealed secret copied to another row does
//! not open. There is no v1 or plaintext fallback.

use std::sync::LazyLock;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::RngCore;
use regex::Regex;
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM, NONCE_LEN};

use crate::auth::password::Keyring;

const TAG_LEN: usize = 16;

static SEALED_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^enc:v2:([a-zA-Z0-9_-]{1,32}):([A-Za-z0-9_-]+)$").expect("sealed secret regex")
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SecretBoxError {
    #[error("secret box key unavailable")]
    KeyUnavailable,
    #[error("sealed secret is malformed or does not open")]
    Invalid,
}

fn header(kid: &str) -> String {
    format!("enc:v2:{kid}:")
}

fn aad(kid: &str, context: &str) -> Vec<u8> {
    serde_json::to_vec(&[header(kid), context.to_string()]).expect("aad json")
}

fn key_for(ring: &Keyring, kid: &str) -> Result<LessSafeKey, SecretBoxError> {
    let material = ring.keys.get(kid).ok_or(SecretBoxError::KeyUnavailable)?;
    let unbound =
        UnboundKey::new(&AES_256_GCM, material).map_err(|_| SecretBoxError::KeyUnavailable)?;
    Ok(LessSafeKey::new(unbound))
}

pub fn seal(ring: &Keyring, plaintext: &str, context: &str) -> Result<String, SecretBoxError> {
    let kid = ring.active_id.as_str();
    let key = key_for(ring, kid)?;
    let mut iv = [0u8; NONCE_LEN];
    rand::rng().fill_bytes(&mut iv);
    let mut body = plaintext.as_bytes().to_vec();
    let tag = key
        .seal_in_place_separate_tag(
            Nonce::assume_unique_for_key(iv),
            Aad::from(aad(kid, context)),
            &mut body,
        )
        .map_err(|_| SecretBoxError::Invalid)?;
    let mut packed = Vec::with_capacity(NONCE_LEN + TAG_LEN + body.len());
    packed.extend_from_slice(&iv);
    packed.extend_from_slice(tag.as_ref());
    packed.extend_from_slice(&body);
    Ok(format!("{}{}", header(kid), URL_SAFE_NO_PAD.encode(packed)))
}

pub fn open(ring: &Keyring, stored: &str, context: &str) -> Result<String, SecretBoxError> {
    let caps = SEALED_RE.captures(stored).ok_or(SecretBoxError::Invalid)?;
    let kid = &caps[1];
    let raw = URL_SAFE_NO_PAD
        .decode(&caps[2])
        .map_err(|_| SecretBoxError::Invalid)?;
    if raw.len() < NONCE_LEN + TAG_LEN || URL_SAFE_NO_PAD.encode(&raw) != caps[2] {
        return Err(SecretBoxError::Invalid);
    }
    let key = key_for(ring, kid)?;
    let iv: [u8; NONCE_LEN] = raw[..NONCE_LEN].try_into().expect("nonce length");
    let tag = &raw[NONCE_LEN..NONCE_LEN + TAG_LEN];
    let mut in_out = raw[NONCE_LEN + TAG_LEN..].to_vec();
    in_out.extend_from_slice(tag);
    let opened = key
        .open_in_place(
            Nonce::assume_unique_for_key(iv),
            Aad::from(aad(kid, context)),
            &mut in_out,
        )
        .map_err(|_| SecretBoxError::Invalid)?;
    String::from_utf8(opened.to_vec()).map_err(|_| SecretBoxError::Invalid)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ring(active: &str) -> Keyring {
        Keyring::parse(
            &format!(r#"{{"a":"{}","b":"{}"}}"#, "11".repeat(32), "22".repeat(32)),
            active,
        )
        .expect("keyring")
    }

    #[test]
    fn seal_open_round_trip_and_rotation() {
        let sealed = seal(&ring("a"), "s3cr3t", "webhook:w:1").expect("seal");
        assert!(sealed.starts_with("enc:v2:a:"));
        assert!(!sealed.contains("s3cr3t"));
        // Rotating the active key keeps old values readable.
        assert_eq!(
            open(&ring("b"), &sealed, "webhook:w:1").expect("open"),
            "s3cr3t"
        );
    }

    #[test]
    fn open_rejects_other_context_tamper_and_unknown_key() {
        let sealed = seal(&ring("a"), "s3cr3t", "webhook:w:1").expect("seal");
        assert_eq!(
            open(&ring("a"), &sealed, "webhook:w:2"),
            Err(SecretBoxError::Invalid)
        );
        let mut tampered = sealed.clone();
        let last = tampered.pop().expect("char");
        tampered.push(if last == 'A' { 'B' } else { 'A' });
        assert!(open(&ring("a"), &tampered, "webhook:w:1").is_err());
        let only_b =
            Keyring::parse(&format!(r#"{{"b":"{}"}}"#, "22".repeat(32)), "b").expect("keyring");
        assert_eq!(
            open(&only_b, &sealed, "webhook:w:1"),
            Err(SecretBoxError::KeyUnavailable)
        );
        assert_eq!(
            open(&ring("a"), "plaintext", "webhook:w:1"),
            Err(SecretBoxError::Invalid)
        );
    }

    #[test]
    fn opens_a_value_sealed_by_the_source_layout() {
        // Built by hand with the source layout: iv || tag || ciphertext and the
        // JSON AAD, so the format (not just this module) is what is checked.
        let keyring = ring("a");
        let key = key_for(&keyring, "a").expect("key");
        let iv = [7u8; NONCE_LEN];
        let mut body = b"hello".to_vec();
        let tag = key
            .seal_in_place_separate_tag(
                Nonce::assume_unique_for_key(iv),
                Aad::from(br#"["enc:v2:a:","ctx"]"#.to_vec()),
                &mut body,
            )
            .expect("seal");
        let mut packed = iv.to_vec();
        packed.extend_from_slice(tag.as_ref());
        packed.extend_from_slice(&body);
        let stored = format!("enc:v2:a:{}", URL_SAFE_NO_PAD.encode(packed));
        assert_eq!(open(&keyring, &stored, "ctx").expect("open"), "hello");
    }
}
