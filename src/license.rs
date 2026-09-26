//! Offline FVOCI2 entitlement verification. The released trust manifest is
//! intentionally empty until the issuer supplies public verification keys.
use std::collections::{BTreeMap, BTreeSet};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::{DateTime, Duration, Utc};
use ring::signature::{UnparsedPublicKey, ECDSA_P256_SHA256_ASN1, ED25519};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use spki::der::{Decode, Document};
use spki::{ObjectIdentifier, SubjectPublicKeyInfoRef};

const TRUST_MANIFEST: &str = include_str!("license-trust.json");
const GRACE_HOURS: i64 = 72;
const MAX_SAFE_INTEGER: u64 = (1_u64 << 53) - 1;
const FEATURES: [&str; 4] = ["audit", "branding", "cloudSplit", "workspaceSso"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Limit {
    Unlimited,
    Value(u64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub seats: Limit,
    pub storage_bytes: Limit,
    pub upload_bytes: Limit,
    pub ai_credits: Limit,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            seats: Limit::Value(10),
            storage_bytes: Limit::Unlimited,
            upload_bytes: Limit::Unlimited,
            ai_credits: Limit::Unlimited,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    Absent,
    Invalid,
    Expired,
}

#[derive(Debug, Clone)]
pub struct Claims {
    pub kid: String,
    pub fingerprint: String,
    features: BTreeSet<String>,
    limits: Limits,
    nbf: DateTime<Utc>,
    exp: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub enum Inspection<'a> {
    Enabled(&'a Claims),
    Disabled(Reason),
}

#[derive(Debug, Clone)]
pub struct Entitlements {
    claims: Option<Claims>,
    reason: Reason,
}

impl Entitlements {
    pub fn inspect(&self, now: DateTime<Utc>) -> Inspection<'_> {
        match &self.claims {
            Some(claims)
                if now >= claims.nbf - Duration::hours(GRACE_HOURS)
                    && now <= claims.exp + Duration::hours(GRACE_HOURS) =>
            {
                Inspection::Enabled(claims)
            }
            Some(_) => Inspection::Disabled(Reason::Expired),
            None => Inspection::Disabled(self.reason),
        }
    }

    pub fn has_feature_at(&self, feature: &str, now: DateTime<Utc>) -> bool {
        FEATURES.contains(&feature)
            && matches!(self.inspect(now), Inspection::Enabled(c) if c.features.contains(feature))
    }

    pub fn has_feature(&self, feature: &str) -> bool {
        self.has_feature_at(feature, Utc::now())
    }

    pub fn enabled_features(&self) -> Vec<&'static str> {
        FEATURES
            .into_iter()
            .filter(|feature| self.has_feature(feature))
            .collect()
    }

    pub fn limits_at(&self, now: DateTime<Utc>) -> Limits {
        match self.inspect(now) {
            Inspection::Enabled(c) => c.limits,
            Inspection::Disabled(_) => Limits::default(),
        }
    }

    pub fn limits(&self) -> Limits {
        self.limits_at(Utc::now())
    }
}

/// The only production trust path: the compiled release manifest.
pub fn from_env() -> Entitlements {
    let token = std::env::var("FVOCI_LICENSE_KEY").ok();
    let result = load(token.as_deref(), TRUST_MANIFEST);
    match result.inspect(Utc::now()) {
        Inspection::Disabled(Reason::Invalid) => tracing::warn!("ee.license_invalid"),
        Inspection::Disabled(Reason::Expired) => {
            if let Some(claims) = &result.claims {
                tracing::warn!(kid = %claims.kid, fingerprint = %claims.fingerprint, "ee.license_expired");
            }
        }
        _ => {}
    }
    result
}

pub fn absent() -> Entitlements {
    Entitlements {
        claims: None,
        reason: Reason::Absent,
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TrustManifest {
    version: u8,
    keys: Vec<TrustKey>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TrustKey {
    kid: String,
    alg: String,
    spki: String,
    #[serde(rename = "spkiSha256")]
    spki_sha256: String,
}

#[derive(Debug)]
struct PreparedKey {
    alg: String,
    bytes: Vec<u8>,
}

fn prepare_keys(manifest: &str) -> Option<BTreeMap<String, PreparedKey>> {
    let manifest: TrustManifest = serde_json::from_str(manifest).ok()?;
    if manifest.version != 1 {
        return None;
    }
    let mut out = BTreeMap::new();
    for key in manifest.keys {
        if !valid_kid(&key.kid) || !matches!(key.alg.as_str(), "ed25519" | "es256") {
            return None;
        }
        let (label, document) = Document::from_pem(&key.spki).ok()?;
        if label != "PUBLIC KEY"
            || hex::encode(Sha256::digest(document.as_bytes())) != key.spki_sha256
        {
            return None;
        }
        let info = SubjectPublicKeyInfoRef::from_der(document.as_bytes()).ok()?;
        let bytes = info.subject_public_key.as_bytes()?.to_vec();
        let oid = info.algorithm.oid;
        let ed = ObjectIdentifier::new_unwrap("1.3.101.112");
        let ec = ObjectIdentifier::new_unwrap("1.2.840.10045.2.1");
        let p256 = ObjectIdentifier::new_unwrap("1.2.840.10045.3.1.7");
        let valid = match key.alg.as_str() {
            "ed25519" => oid == ed && info.algorithm.parameters.is_none() && bytes.len() == 32,
            "es256" => {
                oid == ec
                    && info
                        .algorithm
                        .parameters
                        .and_then(|p| p.decode_as::<ObjectIdentifier>().ok())
                        == Some(p256)
                    && bytes.len() == 65
                    && bytes[0] == 4
            }
            _ => false,
        };
        if !valid
            || out
                .insert(
                    key.kid,
                    PreparedKey {
                        alg: key.alg,
                        bytes,
                    },
                )
                .is_some()
        {
            return None;
        }
    }
    Some(out)
}

fn valid_kid(kid: &str) -> bool {
    let mut bytes = kid.bytes();
    matches!(bytes.next(), Some(b) if b.is_ascii_alphanumeric())
        && kid.len() <= 64
        && bytes.all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn valid_feature(feature: &str) -> bool {
    let mut bytes = feature.bytes();
    matches!(bytes.next(), Some(b) if b.is_ascii_alphabetic())
        && feature.len() <= 64
        && bytes.all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn parse_limit(value: &Value) -> Option<Limit> {
    if value == "unlimited" {
        Some(Limit::Unlimited)
    } else {
        value
            .as_u64()
            .filter(|n| *n <= MAX_SAFE_INTEGER)
            .map(Limit::Value)
    }
}

fn parse_claims(payload: &[u8], kid: String, fingerprint: String) -> Option<Claims> {
    let v: Value = serde_json::from_slice(payload).ok()?;
    let obj = v.as_object()?;
    if obj.get("v")?.as_u64()? != 1
        || obj.get("licensee")?.as_str()?.trim().is_empty()
        || !matches!(
            obj.get("plan")?.as_str()?,
            "selfhost-ce"
                | "selfhost-pro"
                | "selfhost-max"
                | "hosted-free"
                | "hosted-pro"
                | "hosted-max"
        )
    {
        return None;
    }
    let mut features = BTreeSet::new();
    for feature in obj.get("features")?.as_array()? {
        let feature = feature.as_str()?;
        if !valid_feature(feature) || !features.insert(feature.to_owned()) {
            return None;
        }
    }
    let mut limits = Limits::default();
    if let Some(value) = obj.get("limits") {
        for (key, value) in value.as_object()? {
            let parsed = parse_limit(value)?;
            match key.as_str() {
                "seats" => limits.seats = parsed,
                "storageBytes" => limits.storage_bytes = parsed,
                "uploadBytes" => limits.upload_bytes = parsed,
                "aiCredits" => limits.ai_credits = parsed,
                _ => return None,
            }
        }
    }
    let parse_time = |key: &str| {
        DateTime::parse_from_rfc3339(obj.get(key)?.as_str()?)
            .ok()
            .map(|dt| dt.with_timezone(&Utc))
    };
    let iat = parse_time("iat")?;
    let nbf = parse_time("nbf")?;
    let exp = parse_time("exp")?;
    if iat > exp || nbf > exp {
        return None;
    }
    Some(Claims {
        kid,
        fingerprint,
        features,
        limits,
        nbf,
        exp,
    })
}

/// Test callers may inject ephemeral public keys. Production calls this with
/// the compiled empty manifest; no environment variable can replace trust.
pub fn load(token: Option<&str>, manifest: &str) -> Entitlements {
    let disabled = |reason| Entitlements {
        claims: None,
        reason,
    };
    let Some(token) = token.map(str::trim).filter(|s| !s.is_empty()) else {
        return disabled(Reason::Absent);
    };
    let Some(keys) = prepare_keys(manifest) else {
        return disabled(Reason::Invalid);
    };
    if token.len() > 4096 || !token.starts_with("FVOCI2-") {
        return disabled(Reason::Invalid);
    }
    let parts: Vec<_> = token["FVOCI2-".len()..].split('.').collect();
    if parts.len() != 3 {
        return disabled(Reason::Invalid);
    }
    let decode = |s: &str| URL_SAFE_NO_PAD.decode(s).ok().filter(|b| b.len() <= 2048);
    let (Some(header), Some(payload), Some(signature)) = (
        decode(parts[0]),
        decode(parts[1]),
        URL_SAFE_NO_PAD.decode(parts[2]).ok(),
    ) else {
        return disabled(Reason::Invalid);
    };
    let Ok(header): Result<Value, _> = serde_json::from_slice(&header) else {
        return disabled(Reason::Invalid);
    };
    let (Some(alg), Some(kid)) = (
        header.get("alg").and_then(Value::as_str),
        header.get("kid").and_then(Value::as_str),
    ) else {
        return disabled(Reason::Invalid);
    };
    let Some(key) = keys.get(kid).filter(|key| key.alg == alg && valid_kid(kid)) else {
        return disabled(Reason::Invalid);
    };
    let algorithm: &'static dyn ring::signature::VerificationAlgorithm = match alg {
        "ed25519" if signature.len() == 64 => &ED25519,
        "es256" if signature.len() != 64 => &ECDSA_P256_SHA256_ASN1,
        _ => return disabled(Reason::Invalid),
    };
    let signed = format!("{}.{}", parts[0], parts[1]);
    if UnparsedPublicKey::new(algorithm, &key.bytes)
        .verify(signed.as_bytes(), &signature)
        .is_err()
    {
        return disabled(Reason::Invalid);
    }
    let fingerprint = hex::encode(Sha256::digest(parts[1].as_bytes()))[..12].to_owned();
    match parse_claims(&payload, kid.to_owned(), fingerprint) {
        Some(claims) => Entitlements {
            claims: Some(claims),
            reason: Reason::Invalid,
        },
        None => disabled(Reason::Invalid),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::STANDARD;
    use ring::rand::SystemRandom;
    use ring::signature::{EcdsaKeyPair, Ed25519KeyPair, KeyPair, ECDSA_P256_SHA256_ASN1_SIGNING};
    use serde_json::json;

    fn manifest(kid: &str, alg: &str, der: &[u8]) -> String {
        let encoded = STANDARD.encode(der);
        let body = encoded
            .as_bytes()
            .chunks(64)
            .map(|chunk| std::str::from_utf8(chunk).unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        let pem = format!(
            "-----BEGIN PUBLIC KEY-----\n{}\n-----END PUBLIC KEY-----\n",
            body
        );
        json!({"version":1,"keys":[{
            "kid":kid,"alg":alg,"spki":pem,"spkiSha256":hex::encode(Sha256::digest(der))
        }]})
        .to_string()
    }

    fn signed_token<F>(alg: &str, payload: Value, sign: F) -> String
    where
        F: FnOnce(&[u8]) -> Vec<u8>,
    {
        let header = URL_SAFE_NO_PAD.encode(json!({"alg":alg,"kid":"test-1"}).to_string());
        let payload = URL_SAFE_NO_PAD.encode(payload.to_string());
        let signed = format!("{header}.{payload}");
        format!(
            "FVOCI2-{signed}.{}",
            URL_SAFE_NO_PAD.encode(sign(signed.as_bytes()))
        )
    }

    fn payload() -> Value {
        json!({
            "v":1,"licensee":"example","plan":"selfhost-pro",
            "features":["branding","workspaceSso","future.feature"],
            "limits":{"seats":25,"storageBytes":1000,"uploadBytes":100,"aiCredits":"unlimited"},
            "iat":"2026-01-01T00:00:00Z","nbf":"2026-01-01T00:00:00Z",
            "exp":"2027-01-01T00:00:00Z"
        })
    }

    #[test]
    fn ed25519_verifies_only_with_injected_key_and_live_claims() {
        let rng = SystemRandom::new();
        let pair =
            Ed25519KeyPair::from_pkcs8(Ed25519KeyPair::generate_pkcs8(&rng).unwrap().as_ref())
                .unwrap();
        let mut der = hex::decode("302a300506032b6570032100").unwrap();
        der.extend_from_slice(pair.public_key().as_ref());
        let trust = manifest("test-1", "ed25519", &der);
        let token = signed_token("ed25519", payload(), |bytes| {
            pair.sign(bytes).as_ref().to_vec()
        });
        let now = DateTime::parse_from_rfc3339("2026-06-01T00:00:00Z")
            .unwrap()
            .to_utc();
        let loaded = load(Some(&token), &trust);
        assert!(loaded.has_feature_at("branding", now));
        assert!(!loaded.has_feature_at("future.feature", now));
        assert_eq!(loaded.limits_at(now).seats, Limit::Value(25));
        assert_eq!(loaded.limits_at(now).storage_bytes, Limit::Value(1000));
        assert!(matches!(
            load(Some(&token), TRUST_MANIFEST).inspect(now),
            Inspection::Disabled(Reason::Invalid)
        ));
        let tampered = token.replacen("FVOCI2-", "FVOCI1-", 1);
        assert!(matches!(
            load(Some(&tampered), &trust).inspect(now),
            Inspection::Disabled(Reason::Invalid)
        ));
        let mut parts = token.split('.').collect::<Vec<_>>();
        parts[1] = "e30";
        let altered_payload = parts.join(".");
        assert!(matches!(
            load(Some(&altered_payload), &trust).inspect(now),
            Inspection::Disabled(Reason::Invalid)
        ));
        assert!(matches!(
            loaded.inspect(now + Duration::days(600)),
            Inspection::Disabled(Reason::Expired)
        ));
        assert_eq!(
            loaded.limits_at(now + Duration::days(600)),
            Limits::default()
        );
        assert!(matches!(
            load(None, &trust).inspect(now),
            Inspection::Disabled(Reason::Absent)
        ));

        let mut bad = payload();
        bad["features"] = json!(["audit", "audit"]);
        let duplicate = signed_token("ed25519", bad, |bytes| pair.sign(bytes).as_ref().to_vec());
        assert!(matches!(
            load(Some(&duplicate), &trust).inspect(now),
            Inspection::Disabled(Reason::Invalid)
        ));
        let mut bad = payload();
        bad["limits"]["seats"] = json!(9007199254740992_u64);
        let over_limit = signed_token("ed25519", bad, |bytes| pair.sign(bytes).as_ref().to_vec());
        assert!(matches!(
            load(Some(&over_limit), &trust).inspect(now),
            Inspection::Disabled(Reason::Invalid)
        ));
        let mut future = payload();
        future["nbf"] = json!("2026-06-05T00:00:00Z");
        let future_token = signed_token("ed25519", future, |bytes| {
            pair.sign(bytes).as_ref().to_vec()
        });
        assert!(matches!(
            load(Some(&future_token), &trust).inspect(now),
            Inspection::Disabled(Reason::Expired)
        ));
        assert!(
            load(Some(&future_token), &trust).has_feature_at("branding", now + Duration::days(2))
        );
    }

    #[test]
    fn es256_accepts_der_signature_and_rejects_wrong_trust() {
        let rng = SystemRandom::new();
        let pair = EcdsaKeyPair::from_pkcs8(
            &ECDSA_P256_SHA256_ASN1_SIGNING,
            EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng)
                .unwrap()
                .as_ref(),
            &rng,
        )
        .unwrap();
        let mut der = hex::decode("3059301306072a8648ce3d020106082a8648ce3d030107034200").unwrap();
        der.extend_from_slice(pair.public_key().as_ref());
        let trust = manifest("test-1", "es256", &der);
        let info = SubjectPublicKeyInfoRef::from_der(&der).expect("test DER");
        assert_eq!(info.subject_public_key.as_bytes().unwrap().len(), 65);
        assert_eq!(
            info.algorithm.oid,
            ObjectIdentifier::new_unwrap("1.2.840.10045.2.1")
        );
        assert_eq!(
            info.algorithm
                .parameters
                .and_then(|p| p.decode_as::<ObjectIdentifier>().ok()),
            Some(ObjectIdentifier::new_unwrap("1.2.840.10045.3.1.7"))
        );
        assert!(prepare_keys(&trust).is_some(), "test SPKI must parse");
        let token = signed_token("es256", payload(), |bytes| {
            pair.sign(&rng, bytes).unwrap().as_ref().to_vec()
        });
        let now = DateTime::parse_from_rfc3339("2026-06-01T00:00:00Z")
            .unwrap()
            .to_utc();
        assert!(load(Some(&token), &trust).has_feature_at("workspaceSso", now));
        let wrong_alg = trust.replace("\"es256\"", "\"ed25519\"");
        assert!(matches!(
            load(Some(&token), &wrong_alg).inspect(now),
            Inspection::Disabled(Reason::Invalid)
        ));
        let revoked = json!({"version":1,"keys":[]}).to_string();
        assert!(matches!(
            load(Some(&token), &revoked).inspect(now),
            Inspection::Disabled(Reason::Invalid)
        ));
    }
}
