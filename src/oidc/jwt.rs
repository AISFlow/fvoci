//! id_token validation (OIDC Core 3.1.3.7): compact JWS signed with RS256 or
//! ES256 by a key from the provider's JWKS; issuer, audience, authorized
//! party, expiry, issue time and nonce checked. `none`, HMAC and any other
//! algorithm are refused.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use ring::signature::{self, UnparsedPublicKey};
use serde::Deserialize;
use serde_json::Value;

/// Clock skew tolerated for `exp` / `iat`.
pub const CLOCK_SKEW_SECS: i64 = 60;
/// An id_token issued further in the future than this is refused.
const MAX_FUTURE_IAT_SECS: i64 = 300;
const MAX_TOKEN_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, Deserialize)]
pub struct Jwk {
    pub kty: String,
    #[serde(default)]
    pub kid: Option<String>,
    #[serde(default, rename = "use")]
    pub use_: Option<String>,
    #[serde(default)]
    pub alg: Option<String>,
    #[serde(default)]
    pub n: Option<String>,
    #[serde(default)]
    pub e: Option<String>,
    #[serde(default)]
    pub crv: Option<String>,
    #[serde(default)]
    pub x: Option<String>,
    #[serde(default)]
    pub y: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JwkSet {
    pub keys: Vec<Jwk>,
}

#[derive(Debug, Deserialize)]
struct Header {
    alg: String,
    #[serde(default)]
    kid: Option<String>,
    #[serde(default)]
    crit: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Alg {
    Rs256,
    Es256,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum JwtError {
    #[error("malformed token")]
    Malformed,
    #[error("unsupported algorithm")]
    Algorithm,
    #[error("no matching key")]
    UnknownKey,
    #[error("bad signature")]
    Signature,
    #[error("claim {0} rejected")]
    Claim(&'static str),
}

/// The pieces of a compact JWS before signature verification.
pub struct Parsed {
    pub alg: Alg,
    pub kid: Option<String>,
    signing_input: Vec<u8>,
    signature: Vec<u8>,
    pub claims: Value,
}

fn b64(part: &str) -> Result<Vec<u8>, JwtError> {
    URL_SAFE_NO_PAD
        .decode(part.as_bytes())
        .map_err(|_| JwtError::Malformed)
}

pub fn parse(token: &str) -> Result<Parsed, JwtError> {
    if token.len() > MAX_TOKEN_BYTES {
        return Err(JwtError::Malformed);
    }
    let mut parts = token.split('.');
    let (Some(h), Some(p), Some(s), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(JwtError::Malformed);
    };
    let header: Header = serde_json::from_slice(&b64(h)?).map_err(|_| JwtError::Malformed)?;
    if header.crit.is_some() {
        return Err(JwtError::Malformed);
    }
    let alg = match header.alg.as_str() {
        "RS256" => Alg::Rs256,
        "ES256" => Alg::Es256,
        _ => return Err(JwtError::Algorithm),
    };
    let claims: Value = serde_json::from_slice(&b64(p)?).map_err(|_| JwtError::Malformed)?;
    if !claims.is_object() {
        return Err(JwtError::Malformed);
    }
    Ok(Parsed {
        alg,
        kid: header.kid,
        signing_input: format!("{h}.{p}").into_bytes(),
        signature: b64(s)?,
        claims,
    })
}

fn key_matches(jwk: &Jwk, alg: Alg, kid: Option<&str>) -> bool {
    if jwk.use_.as_deref().is_some_and(|u| u != "sig") {
        return false;
    }
    let (kty, alg_name) = match alg {
        Alg::Rs256 => ("RSA", "RS256"),
        Alg::Es256 => ("EC", "ES256"),
    };
    if jwk.kty != kty || jwk.alg.as_deref().is_some_and(|a| a != alg_name) {
        return false;
    }
    if alg == Alg::Es256 && jwk.crv.as_deref() != Some("P-256") {
        return false;
    }
    match kid {
        Some(kid) => jwk.kid.as_deref() == Some(kid),
        None => true,
    }
}

/// Candidate keys for the token; empty means the set must be refreshed.
pub fn candidates<'a>(set: &'a JwkSet, parsed: &Parsed) -> Vec<&'a Jwk> {
    set.keys
        .iter()
        .filter(|k| key_matches(k, parsed.alg, parsed.kid.as_deref()))
        .collect()
}

fn verify_with(jwk: &Jwk, parsed: &Parsed) -> bool {
    match parsed.alg {
        Alg::Rs256 => {
            let (Some(n), Some(e)) = (jwk.n.as_deref(), jwk.e.as_deref()) else {
                return false;
            };
            let (Ok(n), Ok(e)) = (b64(n), b64(e)) else {
                return false;
            };
            let key = signature::RsaPublicKeyComponents { n: &n, e: &e };
            key.verify(
                &signature::RSA_PKCS1_2048_8192_SHA256,
                &parsed.signing_input,
                &parsed.signature,
            )
            .is_ok()
        }
        Alg::Es256 => {
            let (Some(x), Some(y)) = (jwk.x.as_deref(), jwk.y.as_deref()) else {
                return false;
            };
            let (Ok(x), Ok(y)) = (b64(x), b64(y)) else {
                return false;
            };
            if x.len() != 32 || y.len() != 32 {
                return false;
            }
            let mut point = Vec::with_capacity(65);
            point.push(0x04);
            point.extend_from_slice(&x);
            point.extend_from_slice(&y);
            UnparsedPublicKey::new(&signature::ECDSA_P256_SHA256_FIXED, point)
                .verify(&parsed.signing_input, &parsed.signature)
                .is_ok()
        }
    }
}

pub fn verify_signature(set: &JwkSet, parsed: &Parsed) -> Result<(), JwtError> {
    let keys = candidates(set, parsed);
    if keys.is_empty() {
        return Err(JwtError::UnknownKey);
    }
    if keys.iter().any(|k| verify_with(k, parsed)) {
        Ok(())
    } else {
        Err(JwtError::Signature)
    }
}

pub struct Expected<'a> {
    /// Accepted `iss` values (Microsoft multi-tenant substitutes `tid`).
    pub issuer: IssuerRule<'a>,
    pub client_id: &'a str,
    pub nonce: &'a str,
    pub now_secs: i64,
}

pub enum IssuerRule<'a> {
    Exact(&'a str),
    /// Discovery issuer containing `{tenantid}`, filled with the `tid` claim.
    TenantTemplate(&'a str),
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn num(claims: &Value, name: &'static str) -> Result<i64, JwtError> {
    claims
        .get(name)
        .and_then(Value::as_f64)
        .filter(|v| v.is_finite())
        .map(|v| v as i64)
        .ok_or(JwtError::Claim(name))
}

/// Claims checks after the signature verified.
pub fn check_claims(claims: &Value, expected: &Expected<'_>) -> Result<(), JwtError> {
    let iss = claims
        .get("iss")
        .and_then(Value::as_str)
        .ok_or(JwtError::Claim("iss"))?;
    let issuer_ok = match expected.issuer {
        IssuerRule::Exact(issuer) => iss == issuer,
        IssuerRule::TenantTemplate(template) => claims
            .get("tid")
            .and_then(Value::as_str)
            .filter(|tid| {
                !tid.is_empty() && tid.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            })
            .is_some_and(|tid| iss == template.replace("{tenantid}", tid)),
    };
    if !issuer_ok {
        return Err(JwtError::Claim("iss"));
    }
    let audiences: Vec<&str> = match claims.get("aud") {
        Some(Value::String(aud)) => vec![aud.as_str()],
        Some(Value::Array(items)) => items.iter().filter_map(Value::as_str).collect(),
        _ => return Err(JwtError::Claim("aud")),
    };
    if !audiences.contains(&expected.client_id) {
        return Err(JwtError::Claim("aud"));
    }
    let azp = claims.get("azp").and_then(Value::as_str);
    if (audiences.len() > 1 || azp.is_some()) && azp != Some(expected.client_id) {
        return Err(JwtError::Claim("azp"));
    }
    let exp = num(claims, "exp")?;
    if exp + CLOCK_SKEW_SECS <= expected.now_secs {
        return Err(JwtError::Claim("exp"));
    }
    let iat = num(claims, "iat")?;
    if iat > expected.now_secs + MAX_FUTURE_IAT_SECS {
        return Err(JwtError::Claim("iat"));
    }
    if let Ok(nbf) = num(claims, "nbf") {
        if nbf > expected.now_secs + CLOCK_SKEW_SECS {
            return Err(JwtError::Claim("nbf"));
        }
    }
    let nonce = claims
        .get("nonce")
        .and_then(Value::as_str)
        .ok_or(JwtError::Claim("nonce"))?;
    if !ct_eq(nonce.as_bytes(), expected.nonce.as_bytes()) {
        return Err(JwtError::Claim("nonce"));
    }
    match claims.get("sub").and_then(Value::as_str) {
        Some(sub) if !sub.is_empty() && sub.len() <= 255 => Ok(()),
        _ => Err(JwtError::Claim("sub")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::rand::SystemRandom;
    use ring::signature::{EcdsaKeyPair, KeyPair, ECDSA_P256_SHA256_FIXED_SIGNING};
    use serde_json::json;

    struct Signer {
        pair: EcdsaKeyPair,
        kid: String,
    }

    impl Signer {
        fn new(kid: &str) -> Self {
            let rng = SystemRandom::new();
            let pkcs8 =
                EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
            let pair =
                EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &rng)
                    .unwrap();
            Self {
                pair,
                kid: kid.into(),
            }
        }

        fn jwk(&self) -> Jwk {
            let point = self.pair.public_key().as_ref();
            Jwk {
                kty: "EC".into(),
                kid: Some(self.kid.clone()),
                use_: Some("sig".into()),
                alg: Some("ES256".into()),
                n: None,
                e: None,
                crv: Some("P-256".into()),
                x: Some(URL_SAFE_NO_PAD.encode(&point[1..33])),
                y: Some(URL_SAFE_NO_PAD.encode(&point[33..65])),
            }
        }

        fn sign(&self, header: Value, claims: Value) -> String {
            let h = URL_SAFE_NO_PAD.encode(header.to_string());
            let p = URL_SAFE_NO_PAD.encode(claims.to_string());
            let input = format!("{h}.{p}");
            let sig = self
                .pair
                .sign(&SystemRandom::new(), input.as_bytes())
                .unwrap();
            format!("{input}.{}", URL_SAFE_NO_PAD.encode(sig.as_ref()))
        }
    }

    fn claims() -> Value {
        json!({
            "iss": "https://idp.test", "aud": "client", "sub": "user-1",
            "exp": 2_000_000_100i64, "iat": 2_000_000_000i64, "nonce": "n-1",
        })
    }

    fn expected() -> Expected<'static> {
        Expected {
            issuer: IssuerRule::Exact("https://idp.test"),
            client_id: "client",
            nonce: "n-1",
            now_secs: 2_000_000_050,
        }
    }

    #[test]
    fn es256_round_trip_and_tamper() {
        let signer = Signer::new("k1");
        let set = JwkSet {
            keys: vec![signer.jwk()],
        };
        let token = signer.sign(json!({"alg":"ES256","kid":"k1"}), claims());
        let parsed = parse(&token).unwrap();
        verify_signature(&set, &parsed).unwrap();
        check_claims(&parsed.claims, &expected()).unwrap();

        // Payload swapped under the same signature.
        let mut forged = claims();
        forged["sub"] = json!("admin");
        let other = signer.sign(json!({"alg":"ES256","kid":"k1"}), forged);
        let mut parts: Vec<&str> = token.split('.').collect();
        let other_parts: Vec<&str> = other.split('.').collect();
        parts[1] = other_parts[1];
        let parsed = parse(&parts.join(".")).unwrap();
        assert_eq!(verify_signature(&set, &parsed), Err(JwtError::Signature));

        // Another key with the same kid.
        let impostor = Signer::new("k1");
        let parsed = parse(&impostor.sign(json!({"alg":"ES256","kid":"k1"}), claims())).unwrap();
        assert_eq!(verify_signature(&set, &parsed), Err(JwtError::Signature));
        // Unknown kid asks for a refresh.
        let parsed = parse(&signer.sign(json!({"alg":"ES256","kid":"k9"}), claims())).unwrap();
        assert_eq!(verify_signature(&set, &parsed), Err(JwtError::UnknownKey));
    }

    #[test]
    fn refuses_none_hmac_and_crit() {
        let body = URL_SAFE_NO_PAD.encode(claims().to_string());
        for alg in ["none", "HS256", "RS512", "PS256"] {
            let header = URL_SAFE_NO_PAD.encode(json!({ "alg": alg }).to_string());
            assert_eq!(
                parse(&format!("{header}.{body}.")).err(),
                Some(JwtError::Algorithm),
                "{alg}"
            );
        }
        let header = URL_SAFE_NO_PAD.encode(json!({"alg":"ES256","crit":["x"]}).to_string());
        assert!(parse(&format!("{header}.{body}.AA")).is_err());
        assert!(parse("a.b").is_err());
        assert!(parse("a.b.c.d").is_err());
    }

    #[test]
    fn claim_rules() {
        let base = claims();
        let e = expected();
        let mut c = base.clone();
        c["iss"] = json!("https://evil.test");
        assert_eq!(check_claims(&c, &e), Err(JwtError::Claim("iss")));
        let mut c = base.clone();
        c["aud"] = json!("other");
        assert_eq!(check_claims(&c, &e), Err(JwtError::Claim("aud")));
        let mut c = base.clone();
        c["aud"] = json!(["client", "other"]);
        assert_eq!(check_claims(&c, &e), Err(JwtError::Claim("azp")));
        c["azp"] = json!("client");
        assert!(check_claims(&c, &e).is_ok());
        let mut c = base.clone();
        c["exp"] = json!(2_000_000_050i64 - CLOCK_SKEW_SECS);
        assert_eq!(check_claims(&c, &e), Err(JwtError::Claim("exp")));
        let mut c = base.clone();
        c["iat"] = json!(2_000_000_050i64 + 301);
        assert_eq!(check_claims(&c, &e), Err(JwtError::Claim("iat")));
        let mut c = base.clone();
        c["nonce"] = json!("n-2");
        assert_eq!(check_claims(&c, &e), Err(JwtError::Claim("nonce")));
        let mut c = base.clone();
        c.as_object_mut().unwrap().remove("nonce");
        assert_eq!(check_claims(&c, &e), Err(JwtError::Claim("nonce")));
        let mut c = base.clone();
        c["sub"] = json!("");
        assert_eq!(check_claims(&c, &e), Err(JwtError::Claim("sub")));

        let template = Expected {
            issuer: IssuerRule::TenantTemplate("https://login.microsoftonline.com/{tenantid}/v2.0"),
            ..expected()
        };
        let mut c = base.clone();
        c["iss"] = json!("https://login.microsoftonline.com/abc-123/v2.0");
        c["tid"] = json!("abc-123");
        assert!(check_claims(&c, &template).is_ok());
        c["tid"] = json!("other");
        assert_eq!(check_claims(&c, &template), Err(JwtError::Claim("iss")));
    }
}
