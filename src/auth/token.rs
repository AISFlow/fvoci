use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rand::RngCore;
use sha2::{Digest, Sha256};

pub const SESSION_TTL_SECS: i64 = 30 * 24 * 60 * 60;

pub struct SessionToken {
    pub token: String,
    pub hash: String,
}

pub fn hash_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    hex::encode(digest)
}

pub fn token_hashes_eq(left: &str, right: &str) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in left.as_bytes().iter().zip(right.as_bytes()) {
        diff |= a ^ b;
    }
    diff == 0
}

pub fn new_token() -> SessionToken {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    let token = URL_SAFE_NO_PAD.encode(bytes);
    let hash = hash_token(&token);
    SessionToken { token, hash }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_token_matches_sha256_hex() {
        assert_eq!(
            hash_token("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn new_token_is_43_char_base64url_with_256_bits() {
        let first = new_token();
        let second = new_token();
        assert_eq!(first.token.len(), 43);
        assert_ne!(first.token, second.token);
        assert_eq!(first.hash.len(), 64);
    }

    #[test]
    fn token_hashes_eq_is_length_checked() {
        assert!(token_hashes_eq("aa", "aa"));
        assert!(!token_hashes_eq("aa", "ab"));
        assert!(!token_hashes_eq("aa", "aaa"));
    }
}
