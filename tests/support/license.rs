//! Ephemeral public trust injected into integration app states only.
use std::sync::Arc;

use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine,
};
use fvoci_server::license::Entitlements;
use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair};
use serde_json::json;
use sha2::{Digest, Sha256};

pub fn signed_license() -> Arc<Entitlements> {
    signed_license_with_limits(json!({}))
}

pub fn signed_license_with_limits(limits: serde_json::Value) -> Arc<Entitlements> {
    let rng = SystemRandom::new();
    let pair =
        Ed25519KeyPair::from_pkcs8(Ed25519KeyPair::generate_pkcs8(&rng).unwrap().as_ref()).unwrap();
    let mut der = hex::decode("302a300506032b6570032100").unwrap();
    der.extend_from_slice(pair.public_key().as_ref());
    let manifest = json!({"version":1,"keys":[{
        "kid":"integration","alg":"ed25519",
        "spki":format!("-----BEGIN PUBLIC KEY-----\n{}\n-----END PUBLIC KEY-----\n", STANDARD.encode(&der)),
        "spkiSha256":hex::encode(Sha256::digest(&der))
    }]}).to_string();
    let header = URL_SAFE_NO_PAD.encode(json!({"alg":"ed25519","kid":"integration"}).to_string());
    let payload = URL_SAFE_NO_PAD.encode(
        json!({
            "v":1,"licensee":"integration","plan":"selfhost-pro",
            "features":["audit","branding","workspaceSso"],
            "limits":limits,
            "iat":"2026-01-01T00:00:00Z","nbf":"2026-01-01T00:00:00Z",
            "exp":"2030-01-01T00:00:00Z"
        })
        .to_string(),
    );
    let signed = format!("{header}.{payload}");
    let token = format!(
        "FVOCI2-{signed}.{}",
        URL_SAFE_NO_PAD.encode(pair.sign(signed.as_bytes()).as_ref())
    );
    Arc::new(fvoci_server::license::load(Some(&token), &manifest))
}

pub fn absent_license() -> Arc<Entitlements> {
    Arc::new(fvoci_server::license::load(
        None,
        r#"{"version":1,"keys":[]}"#,
    ))
}
