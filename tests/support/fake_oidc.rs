//! A local OpenID provider for tests: discovery, JWKS, the authorization
//! code + PKCE token endpoint and a Naver-style OAuth2 profile API, bound to
//! 127.0.0.1:0. Signing keys are generated per instance (RSA via the openssl
//! CLI, ES256 via ring); nothing talks to a real provider.

#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::extract::{Form, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use ring::rand::SystemRandom;
use ring::signature::{EcdsaKeyPair, KeyPair, RsaKeyPair, ECDSA_P256_SHA256_FIXED_SIGNING};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

pub enum SigningKey {
    Rsa(RsaKeyPair),
    Ec(EcdsaKeyPair),
}

pub struct Key {
    pub kid: String,
    pub key: SigningKey,
}

impl Key {
    /// RSA-2048 PKCS#8 from `openssl genpkey`, generated now.
    pub fn rsa(kid: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("fvoci-fake-oidc-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).expect("key dir");
        let path = dir.join("key.der");
        let status = std::process::Command::new("openssl")
            .args([
                "genpkey",
                "-algorithm",
                "RSA",
                "-pkeyopt",
                "rsa_keygen_bits:2048",
                "-outform",
                "DER",
                "-out",
            ])
            .arg(&path)
            .stderr(std::process::Stdio::null())
            .status()
            .expect("openssl is required for the fake OIDC provider");
        assert!(status.success(), "openssl genpkey failed");
        let der = std::fs::read(&path).expect("key der");
        let _ = std::fs::remove_dir_all(&dir);
        Self {
            kid: kid.into(),
            // OpenSSL versions differ: PKCS#8 or traditional PKCS#1 DER.
            key: SigningKey::Rsa(
                RsaKeyPair::from_pkcs8(&der)
                    .or_else(|_| RsaKeyPair::from_der(&der))
                    .expect("rsa key der"),
            ),
        }
    }

    pub fn ec(kid: &str) -> Self {
        let rng = SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
        Self {
            kid: kid.into(),
            key: SigningKey::Ec(
                EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &rng)
                    .unwrap(),
            ),
        }
    }

    pub fn jwk(&self) -> Value {
        match &self.key {
            SigningKey::Rsa(pair) => {
                let parts = ring::rsa::PublicKeyComponents::<Vec<u8>>::from(pair.public());
                json!({
                    "kty": "RSA", "kid": self.kid, "use": "sig", "alg": "RS256",
                    "n": URL_SAFE_NO_PAD.encode(parts.n),
                    "e": URL_SAFE_NO_PAD.encode(parts.e),
                })
            }
            SigningKey::Ec(pair) => {
                let point = pair.public_key().as_ref();
                json!({
                    "kty": "EC", "kid": self.kid, "use": "sig", "alg": "ES256", "crv": "P-256",
                    "x": URL_SAFE_NO_PAD.encode(&point[1..33]),
                    "y": URL_SAFE_NO_PAD.encode(&point[33..65]),
                })
            }
        }
    }

    pub fn sign(&self, claims: &Value) -> String {
        let alg = match self.key {
            SigningKey::Rsa(_) => "RS256",
            SigningKey::Ec(_) => "ES256",
        };
        let header =
            URL_SAFE_NO_PAD.encode(json!({"alg": alg, "kid": self.kid, "typ": "JWT"}).to_string());
        let payload = URL_SAFE_NO_PAD.encode(claims.to_string());
        let input = format!("{header}.{payload}");
        let rng = SystemRandom::new();
        let sig = match &self.key {
            SigningKey::Rsa(pair) => {
                let mut sig = vec![0u8; pair.public().modulus_len()];
                pair.sign(
                    &ring::signature::RSA_PKCS1_SHA256,
                    &rng,
                    input.as_bytes(),
                    &mut sig,
                )
                .unwrap();
                sig
            }
            SigningKey::Ec(pair) => pair.sign(&rng, input.as_bytes()).unwrap().as_ref().to_vec(),
        };
        format!("{input}.{}", URL_SAFE_NO_PAD.encode(sig))
    }
}

/// What the next token response does wrong, if anything.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Misbehave {
    None,
    WrongAudience,
    WrongIssuer,
    WrongNonce,
    Expired,
    AlgNone,
    ForeignKey,
    NoIdToken,
    /// Sign with a key the JWKS does not list yet (rotation).
    UnpublishedKey,
}

#[derive(Clone, Debug)]
pub struct Profile {
    pub sub: String,
    pub email: Option<String>,
    pub email_verified: bool,
    pub name: Option<String>,
}

impl Profile {
    pub fn new(sub: &str, email: &str, verified: bool) -> Self {
        Self {
            sub: sub.into(),
            email: Some(email.into()),
            email_verified: verified,
            name: Some("외부 사용자".into()),
        }
    }
}

struct PendingCode {
    client_id: String,
    redirect_uri: String,
    nonce: Option<String>,
    challenge: Option<String>,
    profile: Profile,
}

pub struct Inner {
    pub issuer: String,
    pub discovery_issuer: Option<String>,
    pub client_id: String,
    pub client_secret: String,
    pub keys: Vec<Key>,
    pub unpublished: Option<Key>,
    pub foreign: Key,
    pub misbehave: Misbehave,
    codes: HashMap<String, PendingCode>,
    naver_tokens: HashMap<String, Profile>,
    pub post_auth_only: bool,
    pub oversized_discovery: bool,
    pub redirect_discovery_to: Option<String>,
}

#[derive(Clone)]
pub struct FakeOidc {
    pub base: String,
    pub inner: Arc<Mutex<Inner>>,
    pub discovery_hits: Arc<AtomicUsize>,
    pub jwks_hits: Arc<AtomicUsize>,
    pub token_hits: Arc<AtomicUsize>,
    handle: Arc<JoinHandle<()>>,
}

impl FakeOidc {
    pub async fn start(client_id: &str, client_secret: &str, key: Key) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fake oidc");
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let inner = Arc::new(Mutex::new(Inner {
            issuer: base.clone(),
            discovery_issuer: None,
            client_id: client_id.into(),
            client_secret: client_secret.into(),
            keys: vec![key],
            unpublished: None,
            foreign: Key::ec("foreign"),
            misbehave: Misbehave::None,
            codes: HashMap::new(),
            naver_tokens: HashMap::new(),
            post_auth_only: false,
            oversized_discovery: false,
            redirect_discovery_to: None,
        }));
        let fake = Self {
            base,
            inner,
            discovery_hits: Arc::new(AtomicUsize::new(0)),
            jwks_hits: Arc::new(AtomicUsize::new(0)),
            token_hits: Arc::new(AtomicUsize::new(0)),
            handle: Arc::new(tokio::spawn(async {})),
        };
        let app = Router::new()
            .route("/.well-known/openid-configuration", get(discovery))
            .route("/jwks", get(jwks))
            .route("/token", post(token))
            .route("/oauth2.0/token", post(naver_token))
            .route("/v1/nid/me", get(naver_me))
            .with_state(fake.clone());
        let handle = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Self {
            handle: Arc::new(handle),
            ..fake
        }
    }

    pub fn set(&self, f: impl FnOnce(&mut Inner)) {
        f(&mut self.inner.lock().unwrap());
    }

    /// Plays the provider's authorization endpoint: checks the request the
    /// server built and returns the callback query (`code`, `state`).
    pub fn authorize(&self, authorization_url: &str, profile: Profile) -> String {
        let url = url::Url::parse(authorization_url).expect("authorization url");
        let params: HashMap<String, String> = url.query_pairs().into_owned().collect();
        let mut inner = self.inner.lock().unwrap();
        assert_eq!(params.get("client_id"), Some(&inner.client_id));
        assert_eq!(
            params.get("response_type").map(String::as_str),
            Some("code")
        );
        let naver = url.path() == "/oauth2.0/authorize";
        if !naver {
            assert_eq!(
                params.get("code_challenge_method").map(String::as_str),
                Some("S256")
            );
            assert!(params
                .get("scope")
                .is_some_and(|s| s.split(' ').any(|p| p == "openid")));
        }
        let code = format!("code-{}", uuid::Uuid::now_v7().simple());
        inner.codes.insert(
            code.clone(),
            PendingCode {
                client_id: params["client_id"].clone(),
                redirect_uri: params["redirect_uri"].clone(),
                nonce: params.get("nonce").cloned(),
                challenge: params.get("code_challenge").cloned(),
                profile,
            },
        );
        let state = params.get("state").cloned().unwrap_or_default();
        url::form_urlencoded::Serializer::new(String::new())
            .append_pair("code", &code)
            .append_pair("state", &state)
            .finish()
    }
}

async fn discovery(State(fake): State<FakeOidc>) -> Response {
    fake.discovery_hits.fetch_add(1, Ordering::SeqCst);
    let inner = fake.inner.lock().unwrap();
    if let Some(to) = &inner.redirect_discovery_to {
        return (StatusCode::FOUND, [("location", to.clone())]).into_response();
    }
    if inner.oversized_discovery {
        return (StatusCode::OK, "x".repeat(1024 * 1024)).into_response();
    }
    let issuer = inner
        .discovery_issuer
        .clone()
        .unwrap_or(inner.issuer.clone());
    let mut doc = json!({
        "issuer": issuer,
        "authorization_endpoint": format!("{}/authorize", fake.base),
        "token_endpoint": format!("{}/token", fake.base),
        "jwks_uri": format!("{}/jwks", fake.base),
        "response_types_supported": ["code"],
        "id_token_signing_alg_values_supported": ["RS256", "ES256"],
    });
    if inner.post_auth_only {
        doc["token_endpoint_auth_methods_supported"] = json!(["client_secret_post"]);
    }
    Json(doc).into_response()
}

async fn jwks(State(fake): State<FakeOidc>) -> Json<Value> {
    fake.jwks_hits.fetch_add(1, Ordering::SeqCst);
    let inner = fake.inner.lock().unwrap();
    Json(json!({ "keys": inner.keys.iter().map(Key::jwk).collect::<Vec<_>>() }))
}

fn basic_credentials(headers: &HeaderMap) -> Option<(String, String)> {
    let raw = headers
        .get("authorization")?
        .to_str()
        .ok()?
        .strip_prefix("Basic ")?;
    let decoded = String::from_utf8(STANDARD.decode(raw).ok()?).ok()?;
    let (user, pass) = decoded.split_once(':')?;
    let dec = |v: &str| {
        url::form_urlencoded::parse(format!("v={v}").as_bytes())
            .next()
            .map(|(_, v)| v.into_owned())
            .unwrap_or_default()
    };
    Some((dec(user), dec(pass)))
}

async fn token(
    State(fake): State<FakeOidc>,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    fake.token_hits.fetch_add(1, Ordering::SeqCst);
    let mut inner = fake.inner.lock().unwrap();
    let post = form
        .get("client_id")
        .cloned()
        .zip(form.get("client_secret").cloned());
    // Like most providers, accept either method unless configured post-only.
    let credentials = if inner.post_auth_only {
        post
    } else {
        basic_credentials(&headers).or(post)
    };
    if credentials != Some((inner.client_id.clone(), inner.client_secret.clone())) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "invalid_client"})),
        )
            .into_response();
    }
    if form.get("grant_type").map(String::as_str) != Some("authorization_code") {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "unsupported_grant_type"})),
        )
            .into_response();
    }
    let Some(pending) = form.get("code").and_then(|c| inner.codes.remove(c)) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "invalid_grant"})),
        )
            .into_response();
    };
    let verifier = form.get("code_verifier").cloned().unwrap_or_default();
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    if pending.challenge.as_deref() != Some(challenge.as_str())
        || form.get("redirect_uri") != Some(&pending.redirect_uri)
        || pending.client_id != inner.client_id
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "invalid_grant"})),
        )
            .into_response();
    }
    let now = chrono::Utc::now().timestamp();
    let mut claims = json!({
        "iss": inner.issuer,
        "aud": inner.client_id,
        "sub": pending.profile.sub,
        "iat": now,
        "exp": now + 300,
        "nonce": pending.nonce,
        "email_verified": pending.profile.email_verified,
    });
    if let Some(email) = &pending.profile.email {
        claims["email"] = json!(email);
    }
    if let Some(name) = &pending.profile.name {
        claims["name"] = json!(name);
    }
    let misbehave = std::mem::replace(&mut inner.misbehave, Misbehave::None);
    match misbehave {
        Misbehave::WrongAudience => claims["aud"] = json!("someone-else"),
        Misbehave::WrongIssuer => claims["iss"] = json!("https://evil.example"),
        Misbehave::WrongNonce => claims["nonce"] = json!("other-nonce"),
        Misbehave::Expired => claims["exp"] = json!(now - 3600),
        _ => {}
    }
    let id_token = match misbehave {
        Misbehave::AlgNone => {
            let h = URL_SAFE_NO_PAD.encode(json!({"alg": "none"}).to_string());
            let p = URL_SAFE_NO_PAD.encode(claims.to_string());
            Some(format!("{h}.{p}."))
        }
        Misbehave::ForeignKey => {
            let mut forged = inner.foreign.sign(&claims);
            // Claim the published kid while signing with another key.
            let kid = inner.keys[0].kid.clone();
            let header = URL_SAFE_NO_PAD.encode(json!({"alg": "ES256", "kid": kid}).to_string());
            let rest = forged.split_once('.').unwrap().1.to_string();
            forged = format!("{header}.{rest}");
            Some(forged)
        }
        Misbehave::UnpublishedKey => {
            let key = inner.unpublished.take().expect("unpublished key");
            let token = key.sign(&claims);
            inner.keys.push(key);
            Some(token)
        }
        Misbehave::NoIdToken => None,
        _ => Some(inner.keys[0].sign(&claims)),
    };
    let mut body =
        json!({"access_token": "fake-access", "token_type": "Bearer", "expires_in": 300});
    if let Some(id_token) = id_token {
        body["id_token"] = json!(id_token);
    }
    Json(body).into_response()
}

async fn naver_token(
    State(fake): State<FakeOidc>,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    fake.token_hits.fetch_add(1, Ordering::SeqCst);
    let mut inner = fake.inner.lock().unwrap();
    if form.get("client_id") != Some(&inner.client_id)
        || form.get("client_secret") != Some(&inner.client_secret)
    {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "invalid_client"})),
        )
            .into_response();
    }
    let Some(pending) = form.get("code").and_then(|c| inner.codes.remove(c)) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "invalid_grant"})),
        )
            .into_response();
    };
    let access = format!("naver-{}", uuid::Uuid::now_v7().simple());
    inner.naver_tokens.insert(access.clone(), pending.profile);
    Json(json!({"access_token": access, "token_type": "bearer"})).into_response()
}

async fn naver_me(State(fake): State<FakeOidc>, headers: HeaderMap) -> Response {
    let inner = fake.inner.lock().unwrap();
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default();
    let Some(profile) = inner.naver_tokens.get(token) else {
        return (StatusCode::UNAUTHORIZED, Json(json!({"resultcode": "024"}))).into_response();
    };
    Json(json!({
        "resultcode": "00",
        "message": "success",
        "response": { "id": profile.sub, "email": profile.email, "name": profile.name },
    }))
    .into_response()
}
