#![cfg(feature = "db-tests")]
#![allow(dead_code)]

//! TOTP MFA and OIDC sign-in over HTTP with the real non-superuser app role.
//! The TOTP clock is pinned; OIDC runs against a local fake provider bound to
//! 127.0.0.1:0 whose signing keys are generated per test.

#[path = "support/project_harness.rs"]
mod project_harness;

use std::net::SocketAddr;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use fvoci_server::attachments::{LocalStorage, ObjectStorage};
use fvoci_server::auth::password::{hash_password, Keyring};
use fvoci_server::auth::token::{hash_token, new_token};
use fvoci_server::auth::totp;
use fvoci_server::auth::AuthService;
use fvoci_server::db::{pool, Db};
use fvoci_server::http::rate_limit::RateLimiter;
use fvoci_server::http::{router_with_identity, state::AppState};
use fvoci_server::identity::Identity;
use fvoci_server::mail::Mailer;
use fvoci_server::oidc::OidcSettings;
use project_harness::{admin_pool, TestDb};
use serde_json::{json, Value};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

const PEPPER: &str =
    r#"{"test":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#;
const ENCRYPTION: &str =
    r#"{"k1":"1111111111111111111111111111111111111111111111111111111111111111"}"#;
const OWNER_EMAIL: &str = "owner@example.com";
const OWNER_PASSWORD: &str = "supersecret1";
const PASSWORD: &str = "membersecret1";
/// 2030-03-17T17:46:40Z, a step boundary: pinned so codes are deterministic.
const T0_MS: i64 = 1_900_000_000_000 - (1_900_000_000_000 % 30_000);

fn peer(n: u8) -> SocketAddr {
    SocketAddr::from(([203, 0, 113, n], 42424))
}

fn keyring() -> Keyring {
    Keyring::parse(PEPPER, "test").expect("pepper")
}

fn encryption_keys() -> Arc<Keyring> {
    Arc::new(Keyring::parse_named(ENCRYPTION, "k1", "ENCRYPTION_KEYS").expect("keys"))
}

// ---------------------------------------------------------------------------
// Harness

pub struct Harness {
    pub db: TestDb,
    pub app: axum::Router,
    pub admin: PgPool,
    pub app_pool: PgPool,
    pub clock: Arc<AtomicI64>,
    pub owner_cookie: String,
    pub owner_id: Uuid,
    pub workspace_id: Uuid,
    storage_root: std::path::PathBuf,
}

pub struct Options {
    pub encryption: bool,
    pub oidc: OidcSettings,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            encryption: true,
            oidc: OidcSettings {
                public_origin: "http://localhost".into(),
                ..OidcSettings::default()
            },
        }
    }
}

impl Harness {
    pub async fn start() -> Self {
        Self::start_with(Options::default()).await
    }

    pub async fn start_with(options: Options) -> Self {
        let db = TestDb::bootstrap().await;
        let app_pool = pool::connect_app(&db.app_url).await.expect("app pool");
        let storage_root =
            std::env::temp_dir().join(format!("fvoci-identity-test-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&storage_root).expect("storage root");
        let storage = ObjectStorage::from(LocalStorage::new(storage_root.clone()));
        let state = AppState {
            auth: Arc::new(AuthService {
                db: Db::new(app_pool.clone()),
                password_keys: keyring(),
            }),
            branding_name: "FVOCI".to_string(),
            public_origin: "http://localhost".to_string(),
            cookie_secure: false,
            rate_limiter: RateLimiter::new(),
            storage,
            upload: fvoci_server::attachments::UploadLimits {
                part_size_bytes: fvoci_server::config::DEFAULT_UPLOAD_PART_SIZE_BYTES,
                max_file_size_bytes: fvoci_server::config::DEFAULT_UPLOAD_MAX_FILE_SIZE_BYTES,
                create_rate_per_5min: fvoci_server::config::DEFAULT_UPLOAD_CREATE_RATE_PER_5MIN,
                part_put_slots: fvoci_server::attachments::PartPutSlots::new(
                    fvoci_server::config::DEFAULT_UPLOAD_MAX_CONCURRENT_PARTS,
                ),
            },
            collab: None,
            meili: None,
            mailer: Arc::new(Mailer::from_smtp(None)),
            document_convert: None,
            import_wake: None,
            import_extractor_available: false,
        };
        let clock = Arc::new(AtomicI64::new(T0_MS));
        let clock_read = clock.clone();
        let identity = Identity {
            encryption_keys: options.encryption.then(encryption_keys),
            totp_issuer: "localhost".into(),
            oidc: options.oidc,
            clock: Arc::new(move || clock_read.load(Ordering::SeqCst)),
        };
        let app = router_with_identity(state, None, Arc::new(identity));
        let admin = admin_pool(&db).await;
        let res = call(
            &app,
            "POST",
            "/api/v1/setup",
            Some(json!({
                "email": OWNER_EMAIL,
                "password": OWNER_PASSWORD,
                "givenName": "Owner",
                "workspaceSlug": "acme",
                "workspaceName": "Acme"
            })),
            None,
            peer(1),
        )
        .await;
        assert_eq!(res.status, StatusCode::CREATED, "{:?}", res.json);
        let owner_cookie = res.cookie().expect("owner cookie");
        let owner_id: Uuid = sqlx::query_scalar("SELECT id FROM fvoci.users WHERE email = $1")
            .bind(OWNER_EMAIL)
            .fetch_one(&admin)
            .await
            .unwrap();
        let workspace_id: Uuid =
            sqlx::query_scalar("SELECT id FROM fvoci.workspaces WHERE slug = 'acme'")
                .fetch_one(&admin)
                .await
                .unwrap();
        Self {
            db,
            app,
            admin,
            app_pool,
            clock,
            owner_cookie,
            owner_id,
            workspace_id,
            storage_root,
        }
    }

    pub async fn finish(self) {
        self.admin.close().await;
        self.app_pool.close().await;
        let _ = std::fs::remove_dir_all(&self.storage_root);
        self.db.cleanup().await;
    }

    pub fn advance_steps(&self, steps: i64) {
        self.clock.fetch_add(steps * 30_000, Ordering::SeqCst);
    }

    pub fn now_ms(&self) -> i64 {
        self.clock.load(Ordering::SeqCst)
    }

    pub async fn insert_user(&self, email: &str, password: Option<&str>) -> Uuid {
        let user_id = Uuid::now_v7();
        let hash = match password {
            Some(password) => Some(hash_password(password, &keyring()).await.unwrap()),
            None => None,
        };
        sqlx::query(
            "INSERT INTO fvoci.users (id, email, given_name, password_hash) VALUES ($1, $2, '사용자', $3)",
        )
        .bind(user_id)
        .bind(email)
        .bind(hash)
        .execute(&self.admin)
        .await
        .expect("insert user");
        user_id
    }

    pub async fn add_membership(&self, user_id: Uuid, role: &str) {
        sqlx::query(
            "INSERT INTO fvoci.memberships (workspace_id, user_id, role) VALUES ($1, $2, $3)",
        )
        .bind(self.workspace_id)
        .bind(user_id)
        .bind(role)
        .execute(&self.admin)
        .await
        .expect("insert membership");
    }

    pub async fn login(&self, email: &str, password: &str, from: SocketAddr) -> Response {
        call(
            &self.app,
            "POST",
            "/api/v1/auth/login",
            Some(json!({"email": email, "password": password})),
            None,
            from,
        )
        .await
    }

    pub async fn member(&self, label: &str) -> (Uuid, String, String) {
        let email = format!("{label}@example.com");
        let user_id = self.insert_user(&email, Some(PASSWORD)).await;
        self.add_membership(user_id, "member").await;
        let res = self.login(&email, PASSWORD, peer(2)).await;
        assert_eq!(res.status, StatusCode::OK, "{:?}", res.json);
        (user_id, email, res.cookie().expect("cookie"))
    }

    /// Setup + enable through the API; returns (secret bytes, recovery codes).
    pub async fn enable_mfa(&self, cookie: &str, password: Option<&str>) -> (Vec<u8>, Vec<String>) {
        let res = call(
            &self.app,
            "POST",
            "/api/v1/auth/mfa/setup",
            Some(json!({ "currentPassword": password })),
            Some(cookie),
            peer(3),
        )
        .await;
        assert_eq!(res.status, StatusCode::OK, "setup: {:?}", res.json);
        let secret = decode_base32(res.json["secret"].as_str().unwrap());
        let codes: Vec<String> = res.json["recoveryCodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c.as_str().unwrap().to_string())
            .collect();
        let code = totp::totp_code(&secret, totp::totp_step(self.now_ms()));
        let res = call(
            &self.app,
            "POST",
            "/api/v1/auth/mfa/enable",
            Some(json!({ "code": code })),
            Some(cookie),
            peer(3),
        )
        .await;
        assert_eq!(res.status, StatusCode::OK, "enable: {:?}", res.json);
        (secret, codes)
    }

    pub async fn verify(&self, mfa_token: &str, code: &str, from: SocketAddr) -> Response {
        call(
            &self.app,
            "POST",
            "/api/v1/auth/mfa/verify",
            Some(json!({ "mfaToken": mfa_token, "code": code })),
            None,
            from,
        )
        .await
    }

    pub fn code_now(&self, secret: &[u8]) -> String {
        totp::totp_code(secret, totp::totp_step(self.now_ms()))
    }

    pub async fn count(&self, sql: &str, id: Uuid) -> i64 {
        sqlx::query_scalar(sql)
            .bind(id)
            .fetch_one(&self.admin)
            .await
            .unwrap()
    }

    pub async fn login_methods(&self, user_id: Uuid) -> Vec<String> {
        sqlx::query_scalar(
            "SELECT payload->>'method' FROM fvoci.events WHERE verb = 'auth.login' AND actor_user_id = $1 ORDER BY seq",
        )
        .bind(user_id)
        .fetch_all(&self.admin)
        .await
        .unwrap()
    }
}

pub fn decode_base32(input: &str) -> Vec<u8> {
    let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut out = Vec::new();
    let mut bits = 0u32;
    let mut value = 0u32;
    for c in input.bytes() {
        let v = alphabet.iter().position(|&a| a == c).expect("base32") as u32;
        value = (value << 5) | v;
        bits += 5;
        if bits >= 8 {
            out.push(((value >> (bits - 8)) & 0xff) as u8);
            bits -= 8;
        }
    }
    out
}

pub struct Response {
    pub status: StatusCode,
    pub json: Value,
    pub headers: HeaderMap,
}

impl Response {
    pub fn cookie(&self) -> Option<String> {
        self.cookie_named("fvoci_session")
    }

    pub fn cookie_named(&self, name: &str) -> Option<String> {
        let prefix = format!("{name}=");
        self.headers
            .get_all("set-cookie")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .find_map(|v| {
                let value = v.split(';').next()?.strip_prefix(prefix.as_str())?;
                (!value.is_empty()).then(|| value.to_string())
            })
    }

    pub fn code(&self) -> &str {
        self.json["code"].as_str().unwrap_or("")
    }

    pub fn location(&self) -> String {
        self.headers
            .get("location")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string()
    }
}

pub async fn call_with(
    app: &axum::Router,
    method: &str,
    path: &str,
    body: Option<Value>,
    cookies: &[(&str, &str)],
    from: SocketAddr,
    extra: &[(&str, &str)],
) -> Response {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("origin", "http://localhost");
    if !cookies.is_empty() {
        let header = cookies
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("; ");
        builder = builder.header("cookie", header);
    }
    for (name, value) in extra {
        builder = builder.header(*name, *value);
    }
    let mut request = match body {
        Some(body) => builder
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    };
    request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(from));
    let response = app.clone().oneshot(request).await.expect("response");
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    Response {
        status,
        json,
        headers,
    }
}

pub async fn call(
    app: &axum::Router,
    method: &str,
    path: &str,
    body: Option<Value>,
    cookie: Option<&str>,
    from: SocketAddr,
) -> Response {
    let cookies: Vec<(&str, &str)> = cookie.map(|c| ("fvoci_session", c)).into_iter().collect();
    call_with(app, method, path, body, &cookies, from, &[]).await
}

// ---------------------------------------------------------------------------
// MFA

#[tokio::test]
async fn mfa_setup_enable_status_and_secret_at_rest() {
    let h = Harness::start().await;
    let (user_id, _email, cookie) = h.member("kim").await;

    let status = call(
        &h.app,
        "GET",
        "/api/v1/auth/mfa",
        None,
        Some(&cookie),
        peer(3),
    )
    .await;
    assert_eq!(status.status, StatusCode::OK);
    assert_eq!(
        status.json,
        json!({"enabled": false, "recoveryCodesLeft": 0})
    );

    let wrong = call(
        &h.app,
        "POST",
        "/api/v1/auth/mfa/setup",
        Some(json!({ "currentPassword": "not-it" })),
        Some(&cookie),
        peer(3),
    )
    .await;
    assert_eq!(wrong.status, StatusCode::BAD_REQUEST);
    assert_eq!(wrong.code(), "mfa_password_invalid");
    let missing = call(
        &h.app,
        "POST",
        "/api/v1/auth/mfa/setup",
        Some(json!({ "currentPassword": null })),
        Some(&cookie),
        peer(3),
    )
    .await;
    assert_eq!(missing.code(), "mfa_password_invalid");

    let setup = call(
        &h.app,
        "POST",
        "/api/v1/auth/mfa/setup",
        Some(json!({ "currentPassword": PASSWORD })),
        Some(&cookie),
        peer(3),
    )
    .await;
    assert_eq!(setup.status, StatusCode::OK, "{:?}", setup.json);
    let secret_b32 = setup.json["secret"].as_str().unwrap().to_string();
    assert_eq!(secret_b32.len(), 32);
    let uri = setup.json["otpauthUri"].as_str().unwrap();
    assert!(uri.starts_with("otpauth://totp/localhost%3Akim%40example.com?secret="));
    assert!(uri.contains("&issuer=localhost&algorithm=SHA1&digits=6&period=30"));
    let codes = setup.json["recoveryCodes"].as_array().unwrap();
    assert_eq!(codes.len(), 10);
    assert!(codes
        .iter()
        .all(|c| c.as_str().unwrap().len() == 14 && c.as_str().unwrap().matches('-').count() == 2));

    // Secret sealed; recovery codes only as sha256.
    let (stored, hashes): (String, Vec<String>) = sqlx::query_as(
        "SELECT totp_secret, recovery_hashes FROM fvoci.user_mfa WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_one(&h.admin)
    .await
    .unwrap();
    let secret = decode_base32(&secret_b32);
    assert!(stored.starts_with("enc:v2:k1:"));
    assert!(!stored.contains(&secret_b32) && !stored.contains(&hex::encode(&secret)));
    let first_plain = totp::normalize_recovery_code(codes[0].as_str().unwrap());
    assert!(hashes.contains(&hash_token(&first_plain)));
    assert!(!hashes.iter().any(|h| h.contains(&first_plain)));

    // Not enabled yet: login still issues a session directly.
    let res = h.login("kim@example.com", PASSWORD, peer(4)).await;
    assert!(res.cookie().is_some());

    let bad = call(
        &h.app,
        "POST",
        "/api/v1/auth/mfa/enable",
        Some(json!({ "code": "000000" })),
        Some(&cookie),
        peer(3),
    )
    .await;
    if h.code_now(&secret) != "000000" {
        assert_eq!(bad.status, StatusCode::BAD_REQUEST);
        assert_eq!(bad.code(), "mfa_code_invalid");
    }
    let short = call(
        &h.app,
        "POST",
        "/api/v1/auth/mfa/enable",
        Some(json!({ "code": "123" })),
        Some(&cookie),
        peer(3),
    )
    .await;
    assert_eq!(short.status, StatusCode::BAD_REQUEST);
    assert_eq!(short.code(), "invalid_input");

    let ok = call(
        &h.app,
        "POST",
        "/api/v1/auth/mfa/enable",
        Some(json!({ "code": h.code_now(&secret) })),
        Some(&cookie),
        peer(3),
    )
    .await;
    assert_eq!(ok.status, StatusCode::OK, "{:?}", ok.json);
    assert_eq!(ok.json, json!({"ok": true}));
    let again = call(
        &h.app,
        "POST",
        "/api/v1/auth/mfa/enable",
        Some(json!({ "code": h.code_now(&secret) })),
        Some(&cookie),
        peer(3),
    )
    .await;
    assert_eq!(again.code(), "mfa_not_setup");

    let status = call(
        &h.app,
        "GET",
        "/api/v1/auth/mfa",
        None,
        Some(&cookie),
        peer(3),
    )
    .await;
    assert_eq!(
        status.json,
        json!({"enabled": true, "recoveryCodesLeft": 10})
    );
    let resetup = call(
        &h.app,
        "POST",
        "/api/v1/auth/mfa/setup",
        Some(json!({ "currentPassword": PASSWORD })),
        Some(&cookie),
        peer(3),
    )
    .await;
    assert_eq!(resetup.status, StatusCode::CONFLICT);
    assert_eq!(resetup.code(), "mfa_already_enabled");

    // Enabling keeps the current session (source user decision).
    let me = call(
        &h.app,
        "GET",
        "/api/v1/auth/me",
        None,
        Some(&cookie),
        peer(3),
    )
    .await;
    assert_eq!(me.status, StatusCode::OK);

    assert_eq!(
        h.count(
            "SELECT count(*) FROM fvoci.events WHERE verb = 'auth.mfa_enabled' AND target_id = $1",
            user_id
        )
        .await,
        1
    );
    assert_eq!(
        h.count(
            "SELECT count(*) FROM fvoci.audit_log WHERE verb = 'auth.mfa_enabled' AND target_id = $1 AND ip = '203.0.113.3'",
            user_id
        )
        .await,
        1
    );
    h.finish().await;
}

#[tokio::test]
async fn mfa_login_challenge_replay_and_recovery() {
    let h = Harness::start().await;
    let (user_id, email, cookie) = h.member("lee").await;
    let (secret, recovery) = h.enable_mfa(&cookie, Some(PASSWORD)).await;

    // Wrong password: same 401 as without MFA.
    let wrong = h.login(&email, "nope-nope", peer(4)).await;
    assert_eq!(wrong.status, StatusCode::UNAUTHORIZED);
    assert_eq!(wrong.code(), "invalid_email_or_password");

    let res = h.login(&email, PASSWORD, peer(4)).await;
    assert_eq!(res.status, StatusCode::OK);
    assert!(
        res.cookie().is_none(),
        "no session before the second factor"
    );
    assert_eq!(res.json["userId"], Value::Null);
    let mfa_token = res.json["mfaToken"].as_str().unwrap().to_string();
    // Stored hashed only.
    assert_eq!(
        h.count(
            "SELECT count(*) FROM fvoci.mfa_challenges WHERE user_id = $1",
            user_id
        )
        .await,
        1
    );
    let stored_hash: String =
        sqlx::query_scalar("SELECT token_hash FROM fvoci.mfa_challenges WHERE user_id = $1")
            .bind(user_id)
            .fetch_one(&h.admin)
            .await
            .unwrap();
    assert_eq!(stored_hash, hash_token(&mfa_token));

    // The step claimed by enable cannot be replayed.
    let replay = h.verify(&mfa_token, &h.code_now(&secret), peer(4)).await;
    assert_eq!(replay.status, StatusCode::UNAUTHORIZED);
    assert_eq!(replay.code(), "mfa_invalid");
    let garbage = h.verify("not-a-token", &h.code_now(&secret), peer(4)).await;
    assert_eq!(garbage.code(), "mfa_invalid");

    h.advance_steps(1);
    let code = h.code_now(&secret);
    let ok = h.verify(&mfa_token, &code, peer(4)).await;
    assert_eq!(ok.status, StatusCode::OK, "{:?}", ok.json);
    assert_eq!(ok.json, json!({"userId": user_id.to_string()}));
    let session = ok.cookie().expect("session after verify");
    let me = call(
        &h.app,
        "GET",
        "/api/v1/auth/me",
        None,
        Some(&session),
        peer(4),
    )
    .await;
    assert_eq!(me.status, StatusCode::OK);

    // Challenge is single use, and the same step is spent for a new challenge.
    let reuse = h.verify(&mfa_token, &code, peer(4)).await;
    assert_eq!(reuse.code(), "mfa_invalid");
    let second = h.login(&email, PASSWORD, peer(4)).await;
    let second_token = second.json["mfaToken"].as_str().unwrap().to_string();
    let replay = h.verify(&second_token, &code, peer(4)).await;
    assert_eq!(replay.code(), "mfa_invalid");

    // Recovery code (display form, any case) works once.
    let shown = recovery[0].to_uppercase();
    let ok = h.verify(&second_token, &shown, peer(4)).await;
    assert_eq!(ok.status, StatusCode::OK, "{:?}", ok.json);
    let third = h.login(&email, PASSWORD, peer(4)).await;
    let third_token = third.json["mfaToken"].as_str().unwrap().to_string();
    let reuse = h.verify(&third_token, &recovery[0], peer(4)).await;
    assert_eq!(reuse.code(), "mfa_invalid");
    let status = call(
        &h.app,
        "GET",
        "/api/v1/auth/mfa",
        None,
        Some(&cookie),
        peer(3),
    )
    .await;
    assert_eq!(status.json["recoveryCodesLeft"], 9);

    assert_eq!(
        h.login_methods(user_id).await,
        vec!["password", "totp", "recovery"]
    );
    h.finish().await;
}

#[tokio::test]
async fn mfa_verify_is_limited_per_account_across_tokens() {
    let h = Harness::start().await;
    let (_user_id, email, cookie) = h.member("park").await;
    let (secret, _) = h.enable_mfa(&cookie, Some(PASSWORD)).await;
    h.advance_steps(1);
    let good = h.code_now(&secret);
    let wrong = if good == "111111" { "222222" } else { "111111" };
    // Five failures, each from a fresh challenge and a different IP.
    for n in 0..5u8 {
        let res = h.login(&email, PASSWORD, peer(20 + n)).await;
        let token = res.json["mfaToken"].as_str().unwrap().to_string();
        let res = h.verify(&token, wrong, peer(20 + n)).await;
        assert_eq!(res.status, StatusCode::UNAUTHORIZED);
    }
    let res = h.login(&email, PASSWORD, peer(30)).await;
    let token = res.json["mfaToken"].as_str().unwrap().to_string();
    let limited = h.verify(&token, &good, peer(30)).await;
    assert_eq!(limited.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(limited.code(), "rate_limit_exceeded");
    assert!(limited.headers.get("retry-after").is_some());
    h.finish().await;
}

#[tokio::test]
async fn mfa_challenge_dies_with_credential_change_suspension_and_expiry() {
    let h = Harness::start().await;
    let (user_id, email, cookie) = h.member("choi").await;
    let (secret, _) = h.enable_mfa(&cookie, Some(PASSWORD)).await;
    h.advance_steps(1);

    // Credential replacement bumps auth_generation.
    let res = h.login(&email, PASSWORD, peer(4)).await;
    let token = res.json["mfaToken"].as_str().unwrap().to_string();
    sqlx::query("UPDATE fvoci.users SET auth_generation = auth_generation + 1 WHERE id = $1")
        .bind(user_id)
        .execute(&h.admin)
        .await
        .unwrap();
    let res = h.verify(&token, &h.code_now(&secret), peer(4)).await;
    assert_eq!(res.code(), "mfa_invalid");

    // Suspension after the first factor.
    h.advance_steps(1);
    let res = h.login(&email, PASSWORD, peer(4)).await;
    let token = res.json["mfaToken"].as_str().unwrap().to_string();
    sqlx::query("UPDATE fvoci.users SET suspended_at = now() WHERE id = $1")
        .bind(user_id)
        .execute(&h.admin)
        .await
        .unwrap();
    let res = h.verify(&token, &h.code_now(&secret), peer(4)).await;
    assert_eq!(res.code(), "mfa_invalid");
    sqlx::query("UPDATE fvoci.users SET suspended_at = NULL WHERE id = $1")
        .bind(user_id)
        .execute(&h.admin)
        .await
        .unwrap();

    // Expired challenge.
    let res = h.login(&email, PASSWORD, peer(4)).await;
    let token = res.json["mfaToken"].as_str().unwrap().to_string();
    sqlx::query("UPDATE fvoci.mfa_challenges SET expires_at = now() - interval '1 second' WHERE token_hash = $1")
        .bind(hash_token(&token))
        .execute(&h.admin)
        .await
        .unwrap();
    let res = h.verify(&token, &h.code_now(&secret), peer(4)).await;
    assert_eq!(res.code(), "mfa_invalid");
    // The GC removes it.
    let removed = fvoci_server::jobs::run_magic_token_gc(
        &h.app_pool,
        chrono::Utc::now(),
        &tokio_util::sync::CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(removed >= 1);
    let left: i64 =
        sqlx::query_scalar("SELECT count(*) FROM fvoci.mfa_challenges WHERE token_hash = $1")
            .bind(hash_token(&token))
            .fetch_one(&h.admin)
            .await
            .unwrap();
    assert_eq!(left, 0);
    h.finish().await;
}

#[tokio::test]
async fn mfa_gate_covers_magic_link_and_invitation_accept() {
    let h = Harness::start().await;
    let (user_id, email, cookie) = h.member("jung").await;
    let (secret, _) = h.enable_mfa(&cookie, Some(PASSWORD)).await;
    h.advance_steps(1);

    // Magic link.
    let issued = new_token();
    let user = fvoci_server::db::account::login_link_user(&h.app_pool, &email)
        .await
        .unwrap()
        .unwrap();
    fvoci_server::db::account::issue_login_token(
        &h.app_pool,
        &user,
        &issued.hash,
        chrono::Utc::now() + chrono::Duration::minutes(15),
    )
    .await
    .unwrap();
    let res = call(
        &h.app,
        "POST",
        "/api/v1/auth/magic-link/consume",
        Some(json!({ "token": issued.token })),
        None,
        peer(5),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK, "{:?}", res.json);
    assert!(res.cookie().is_none());
    assert_eq!(res.json["userId"], Value::Null);
    let token = res.json["mfaToken"].as_str().unwrap().to_string();
    let ok = h.verify(&token, &h.code_now(&secret), peer(5)).await;
    assert_eq!(ok.status, StatusCode::OK);
    assert!(ok.cookie().is_some());

    // Invitation accept by an existing MFA account.
    let other = h.db_workspace("beta", "Beta").await;
    let invite = call(
        &h.app,
        "POST",
        &format!("/api/v1/workspaces/{other}/invitations"),
        Some(json!({ "email": email, "role": "member" })),
        Some(&h.owner_cookie),
        peer(6),
    )
    .await;
    assert_eq!(invite.status, StatusCode::CREATED, "{:?}", invite.json);
    let url = invite.json["acceptUrl"].as_str().unwrap();
    let invite_token = url.rsplit('/').next().unwrap();
    let res = call(
        &h.app,
        "POST",
        &format!("/api/v1/invitations/{invite_token}/accept"),
        Some(json!({ "password": PASSWORD })),
        None,
        peer(6),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK, "{:?}", res.json);
    assert!(res.cookie().is_none());
    let token = res.json["mfaToken"].as_str().unwrap().to_string();
    h.advance_steps(1);
    let ok = h.verify(&token, &h.code_now(&secret), peer(6)).await;
    assert_eq!(ok.status, StatusCode::OK);
    // Membership was granted by the first factor, as in the source.
    assert_eq!(
        h.count(
            "SELECT count(*) FROM fvoci.memberships WHERE user_id = $1",
            user_id
        )
        .await,
        2
    );
    assert_eq!(
        h.login_methods(user_id).await,
        vec!["password", "totp", "totp"]
    );
    h.finish().await;
}

impl Harness {
    /// A second team workspace owned by the instance owner.
    async fn db_workspace(&self, slug: &str, name: &str) -> Uuid {
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO fvoci.workspaces (id, slug, name) VALUES ($1, $2, $3)")
            .bind(id)
            .bind(slug)
            .bind(name)
            .execute(&self.admin)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO fvoci.memberships (workspace_id, user_id, role) VALUES ($1, $2, 'owner')",
        )
        .bind(id)
        .bind(self.owner_id)
        .execute(&self.admin)
        .await
        .unwrap();
        id
    }
}

#[tokio::test]
async fn mfa_disable_reauth_and_passwordless_accounts() {
    let h = Harness::start().await;
    let (user_id, email, cookie) = h.member("han").await;
    let (_secret, _) = h.enable_mfa(&cookie, Some(PASSWORD)).await;

    let neither = call(
        &h.app,
        "POST",
        "/api/v1/auth/mfa/disable",
        Some(json!({ "currentPassword": null, "code": null })),
        Some(&cookie),
        peer(3),
    )
    .await;
    assert_eq!(neither.status, StatusCode::BAD_REQUEST);
    assert_eq!(neither.code(), "invalid_input");
    let wrong = call(
        &h.app,
        "POST",
        "/api/v1/auth/mfa/disable",
        Some(json!({ "currentPassword": "wrong-one", "code": null })),
        Some(&cookie),
        peer(3),
    )
    .await;
    assert_eq!(wrong.code(), "mfa_confirm_invalid");
    let ok = call(
        &h.app,
        "POST",
        "/api/v1/auth/mfa/disable",
        Some(json!({ "currentPassword": PASSWORD, "code": null })),
        Some(&cookie),
        peer(3),
    )
    .await;
    assert_eq!(ok.status, StatusCode::OK, "{:?}", ok.json);
    let again = call(
        &h.app,
        "POST",
        "/api/v1/auth/mfa/disable",
        Some(json!({ "currentPassword": PASSWORD, "code": null })),
        Some(&cookie),
        peer(3),
    )
    .await;
    assert_eq!(again.code(), "mfa_not_enabled");
    assert_eq!(
        h.count(
            "SELECT count(*) FROM fvoci.audit_log WHERE verb = 'auth.mfa_disabled' AND target_id = $1",
            user_id
        )
        .await,
        1
    );
    let res = h.login(&email, PASSWORD, peer(4)).await;
    assert!(res.cookie().is_some(), "session again without MFA");

    // Password-less account (OIDC / magic link): setup needs a fresh session,
    // disable needs a current code.
    let nopw = h.insert_user("nopw@example.com", None).await;
    h.add_membership(nopw, "member").await;
    let fresh = new_token();
    sqlx::query(
        "INSERT INTO fvoci.sessions (id, user_id, token_hash, expires_at, created_at) VALUES ($1, $2, $3, now() + interval '1 day', now() - interval '11 minutes')",
    )
    .bind(Uuid::now_v7())
    .bind(nopw)
    .bind(&fresh.hash)
    .execute(&h.admin)
    .await
    .unwrap();
    let stale = call(
        &h.app,
        "POST",
        "/api/v1/auth/mfa/setup",
        Some(json!({ "currentPassword": null })),
        Some(&fresh.token),
        peer(7),
    )
    .await;
    assert_eq!(stale.status, StatusCode::UNAUTHORIZED);
    assert_eq!(stale.code(), "mfa_reauth_required");
    sqlx::query("UPDATE fvoci.sessions SET created_at = now() WHERE token_hash = $1")
        .bind(&fresh.hash)
        .execute(&h.admin)
        .await
        .unwrap();
    let (secret, _) = h.enable_mfa(&fresh.token, None).await;
    let no_code = call(
        &h.app,
        "POST",
        "/api/v1/auth/mfa/disable",
        Some(json!({ "currentPassword": "anything", "code": null })),
        Some(&fresh.token),
        peer(7),
    )
    .await;
    assert_eq!(no_code.code(), "mfa_confirm_invalid");
    // The enable step is spent; the next step disables.
    let replay = call(
        &h.app,
        "POST",
        "/api/v1/auth/mfa/disable",
        Some(json!({ "currentPassword": null, "code": h.code_now(&secret) })),
        Some(&fresh.token),
        peer(7),
    )
    .await;
    assert_eq!(replay.code(), "mfa_confirm_invalid");
    h.advance_steps(1);
    let ok = call(
        &h.app,
        "POST",
        "/api/v1/auth/mfa/disable",
        Some(json!({ "currentPassword": null, "code": h.code_now(&secret) })),
        Some(&fresh.token),
        peer(7),
    )
    .await;
    assert_eq!(ok.status, StatusCode::OK, "{:?}", ok.json);
    h.finish().await;
}

#[tokio::test]
async fn mfa_concurrent_verifies_issue_one_session() {
    let h = Harness::start().await;
    let (user_id, email, cookie) = h.member("oh").await;
    let (_secret, recovery) = h.enable_mfa(&cookie, Some(PASSWORD)).await;
    let res = h.login(&email, PASSWORD, peer(4)).await;
    let token = res.json["mfaToken"].as_str().unwrap().to_string();
    let before = h
        .count(
            "SELECT count(*) FROM fvoci.sessions WHERE user_id = $1",
            user_id,
        )
        .await;
    let (a, b) = tokio::join!(
        h.verify(&token, &recovery[1], peer(40)),
        h.verify(&token, &recovery[2], peer(41))
    );
    let oks = [a.status, b.status]
        .iter()
        .filter(|s| **s == StatusCode::OK)
        .count();
    assert_eq!(oks, 1, "{:?} {:?}", a.json, b.json);
    let after = h
        .count(
            "SELECT count(*) FROM fvoci.sessions WHERE user_id = $1",
            user_id,
        )
        .await;
    assert_eq!(after, before + 1);
    // The losing request did not spend its recovery code.
    let status = call(
        &h.app,
        "GET",
        "/api/v1/auth/mfa",
        None,
        Some(&cookie),
        peer(3),
    )
    .await;
    assert_eq!(status.json["recoveryCodesLeft"], 9);
    h.finish().await;
}

#[tokio::test]
async fn mfa_rows_are_owner_scoped_for_the_app_role() {
    let h = Harness::start().await;
    let (_user_id, _email, cookie) = h.member("yoon").await;
    h.enable_mfa(&cookie, Some(PASSWORD)).await;
    let res = h.login("yoon@example.com", PASSWORD, peer(4)).await;
    assert!(res.json["mfaToken"].is_string());

    // Without the owner context the app role sees no MFA rows.
    let visible: i64 = sqlx::query_scalar("SELECT count(*) FROM fvoci.user_mfa")
        .fetch_one(&h.app_pool)
        .await
        .unwrap();
    assert_eq!(visible, 0);
    let forced: bool = sqlx::query_scalar(
        "SELECT relforcerowsecurity FROM pg_class WHERE oid = 'fvoci.user_mfa'::regclass",
    )
    .fetch_one(&h.admin)
    .await
    .unwrap();
    assert!(forced);
    // Challenges and flow state are not readable at all.
    for table in ["fvoci.mfa_challenges", "fvoci.oidc_states"] {
        let err = sqlx::query(&format!("SELECT count(*) FROM {table}"))
            .fetch_one(&h.app_pool)
            .await
            .expect_err("app role must not read");
        assert!(err.to_string().contains("permission denied"), "{err}");
    }
    h.finish().await;
}

#[tokio::test]
async fn mfa_setup_without_encryption_keys_is_unavailable() {
    let h = Harness::start_with(Options {
        encryption: false,
        ..Options::default()
    })
    .await;
    let (_user_id, _email, cookie) = h.member("seo").await;
    let res = call(
        &h.app,
        "POST",
        "/api/v1/auth/mfa/setup",
        Some(json!({ "currentPassword": PASSWORD })),
        Some(&cookie),
        peer(3),
    )
    .await;
    assert_eq!(res.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(res.code(), "encryption_unavailable");
    // API tokens never reach the session-only MFA routes.
    let res = call_with(
        &h.app,
        "GET",
        "/api/v1/auth/mfa",
        None,
        &[],
        peer(3),
        &[("authorization", "Bearer fvoci_pat_unknown")],
    )
    .await;
    assert_eq!(res.status, StatusCode::UNAUTHORIZED);
    h.finish().await;
}
