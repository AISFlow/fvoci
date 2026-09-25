use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, LazyLock};

use argon2::password_hash::rand_core::{OsRng, RngCore};
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use hmac::{Hmac, Mac};
use regex::Regex;
use sha2::Sha256;
use tokio::sync::Semaphore;

type HmacSha256 = Hmac<Sha256>;

const MEMORY_COST: u32 = 65536;
const TIME_COST: u32 = 3;
const ARGON2_CONCURRENCY: usize = 4;

static PASSWORD_HASH_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^\$fvoci-pepper=([a-zA-Z0-9_-]{1,32})(\$argon2id\$v=19\$m=65536,t=3,p=1\$[A-Za-z0-9+/]{42}[AEIMQUYcgkosw048]\$[A-Za-z0-9+/]{42}[AEIMQUYcgkosw048])$",
    )
    .expect("password hash regex")
});
static KEY_ID_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-zA-Z0-9_-]{1,32}$").expect("key id regex"));

static ARGON2_SEM: LazyLock<Arc<Semaphore>> =
    LazyLock::new(|| Arc::new(Semaphore::new(ARGON2_CONCURRENCY)));

const DUMMY_HASH: &str =
    "$argon2id$v=19$m=65536,t=3,p=1$HoET2F3LFcs9mMRSrvzJC2vtvTGCguNTN54dC+hTezE$hUFyPgDPDMXY9wGP3BcOWc0INEgtonfZetdGtmPAqO8";
const DUMMY_KEY: [u8; 32] = [0u8; 32];

#[derive(Clone)]
pub struct Keyring {
    pub active_id: String,
    pub keys: HashMap<String, Vec<u8>>,
}

impl fmt::Debug for Keyring {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Keyring")
            .field("active_id", &self.active_id)
            .field("keys", &format!("<{} keys>", self.keys.len()))
            .finish()
    }
}

impl Keyring {
    pub fn parse(raw: &str, active_id: &str) -> Result<Self, String> {
        if !KEY_ID_RE.is_match(active_id) {
            return Err("invalid active key id".into());
        }
        let parsed: HashMap<String, String> =
            serde_json::from_str(raw).map_err(|_| "invalid keyring json".to_string())?;
        if parsed.is_empty() || parsed.len() > 32 {
            return Err("keyring must have 1-32 keys".into());
        }
        let mut keys = HashMap::new();
        for (id, hex) in parsed {
            if !KEY_ID_RE.is_match(&id) {
                return Err("invalid key id".into());
            }
            if hex.len() != 64 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
                return Err("keys must be 64-char hex".to_string());
            }
            keys.insert(
                id,
                hex::decode(hex).map_err(|_| "invalid hex key".to_string())?,
            );
        }
        if !keys.contains_key(active_id) {
            return Err("active key missing from keyring".into());
        }
        Ok(Self {
            active_id: active_id.to_string(),
            keys,
        })
    }
}

fn sign_hmac(secret: &[u8], password: &str) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(secret).expect("hmac key");
    mac.update(password.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

fn argon2() -> Argon2<'static> {
    let params = Params::new(MEMORY_COST, TIME_COST, 1, None).expect("argon2 params");
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
}

fn generate_salt() -> Result<SaltString, String> {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    let b64 = STANDARD.encode(bytes);
    let trimmed = b64.trim_end_matches('=');
    SaltString::from_b64(trimmed).map_err(|e| e.to_string())
}

pub async fn hash_password(password: &str, ring: &Keyring) -> Result<String, String> {
    let key = ring
        .keys
        .get(&ring.active_id)
        .ok_or_else(|| "active pepper unavailable".to_string())?
        .clone();
    let active_id = ring.active_id.clone();
    let password = password.to_string();
    let permit = ARGON2_SEM
        .clone()
        .acquire_owned()
        .await
        .map_err(|e| e.to_string())?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let input = sign_hmac(&key, &password);
        let salt = generate_salt()?;
        let hash = argon2()
            .hash_password(&input, &salt)
            .map_err(|e| e.to_string())?;
        Ok(format!("$fvoci-pepper={}{}", active_id, hash))
    })
    .await
    .map_err(|e| e.to_string())?
}

pub struct VerifyResult {
    pub ok: bool,
    pub needs_pepper_rotation: bool,
}

pub async fn verify_password(hash: Option<&str>, password: &str, ring: &Keyring) -> VerifyResult {
    let hash = hash.map(str::to_string);
    let password = password.to_string();
    let ring = ring.clone();
    let permit = match ARGON2_SEM.clone().acquire_owned().await {
        Ok(permit) => permit,
        Err(_) => {
            return VerifyResult {
                ok: false,
                needs_pepper_rotation: false,
            };
        }
    };
    match tokio::task::spawn_blocking(move || {
        let _permit = permit;
        verify_password_sync(hash.as_deref(), &password, &ring)
    })
    .await
    {
        Ok(result) => result,
        Err(_) => VerifyResult {
            ok: false,
            needs_pepper_rotation: false,
        },
    }
}

fn verify_password_sync(hash: Option<&str>, password: &str, ring: &Keyring) -> VerifyResult {
    let matched = hash.and_then(|h| PASSWORD_HASH_RE.captures(h));
    let kid = matched.as_ref().and_then(|m| m.get(1)).map(|m| m.as_str());
    let phc = matched.as_ref().and_then(|m| m.get(2)).map(|m| m.as_str());
    let full_match = matched.as_ref().map(|m| m.get(0).unwrap().as_str()) == hash;

    let key = kid.and_then(|id| ring.keys.get(id));
    let secret = key.map(Vec::as_slice).unwrap_or(DUMMY_KEY.as_slice());
    let input = sign_hmac(secret, password);

    let candidate = match (key, phc, full_match) {
        (Some(_), Some(phc), true) => phc,
        _ => DUMMY_HASH,
    };

    let verified = PasswordHash::new(candidate)
        .and_then(|parsed| argon2().verify_password(&input, &parsed))
        .is_ok();

    let ok = key.is_some() && phc.is_some() && full_match && verified;
    VerifyResult {
        ok,
        needs_pepper_rotation: ok && kid != Some(ring.active_id.as_str()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PEPPER: &str =
        r#"{"test":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#;

    #[tokio::test]
    async fn hash_verify_round_trip() {
        let ring = Keyring::parse(PEPPER, "test").unwrap();
        let hash = hash_password("supersecret1", &ring).await.unwrap();
        assert!(PASSWORD_HASH_RE.is_match(&hash));
        let verified = verify_password(Some(&hash), "supersecret1", &ring).await;
        assert!(verified.ok);
        assert!(!verified.needs_pepper_rotation);
    }

    #[tokio::test]
    async fn verify_rejects_wrong_and_missing_password() {
        let ring = Keyring::parse(PEPPER, "test").unwrap();
        let hash = hash_password("supersecret1", &ring).await.unwrap();
        assert!(!verify_password(Some(&hash), "wrong", &ring).await.ok);
        assert!(!verify_password(None, "supersecret1", &ring).await.ok);
    }
}
