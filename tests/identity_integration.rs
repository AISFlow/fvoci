#![cfg(feature = "db-tests")]
#![allow(dead_code)]

//! TOTP MFA and OIDC sign-in over HTTP with the real non-superuser app role.
//! The TOTP clock is pinned; OIDC runs against a local fake provider bound to
//! 127.0.0.1:0 whose signing keys are generated per test.

#[path = "support/project_harness.rs"]
mod project_harness;

#[path = "support/fake_oidc.rs"]
mod fake_oidc;

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
    state: AppState,
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
        let _ = tracing_subscriber::fmt()
            .with_test_writer()
            .with_env_filter("warn")
            .try_init();
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
            search_embedder: None,
            mailer: Arc::new(Mailer::from_smtp(None)),
            quota: Default::default(),
            markdown: Some(
                fvoci_server::documents::markdown_helper::MarkdownHelper::new(env!(
                    "CARGO_BIN_EXE_fvoci-server"
                )),
            ),
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
        let app = router_with_identity(state.clone(), None, Arc::new(identity));
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
            state,
        }
    }

    /// A second server over the same database and pools with other OIDC
    /// settings (an operator reconfiguring a provider).
    pub fn app_with_oidc(&self, oidc: OidcSettings) -> axum::Router {
        let clock_read = self.clock.clone();
        let identity = Identity {
            encryption_keys: Some(encryption_keys()),
            totp_issuer: "localhost".into(),
            oidc,
            clock: Arc::new(move || clock_read.load(Ordering::SeqCst)),
        };
        router_with_identity(self.state.clone(), None, Arc::new(identity))
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
    let (user_id, email, cookie) = h.member("park").await;
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
    // The count lives in the database (not the process-local limiter), so it
    // holds across restarts and replicas.
    let stored: i32 =
        sqlx::query_scalar("SELECT verify_count FROM fvoci.user_mfa WHERE user_id = $1")
            .bind(user_id)
            .fetch_one(&h.admin)
            .await
            .unwrap();
    assert_eq!(stored, 6);
    // Once the window has passed, the account can verify again.
    sqlx::query(
        "UPDATE fvoci.user_mfa SET verify_window_start = now() - interval '6 minutes' WHERE user_id = $1",
    )
    .bind(user_id)
    .execute(&h.admin)
    .await
    .unwrap();
    let res = h.login(&email, PASSWORD, peer(31)).await;
    let token = res.json["mfaToken"].as_str().unwrap().to_string();
    h.advance_steps(1);
    let good = h.code_now(&secret);
    let ok = h.verify(&token, &good, peer(31)).await;
    assert_eq!(ok.status, StatusCode::OK, "{}", ok.json);
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

// ---------------------------------------------------------------------------
// OIDC (fake provider on 127.0.0.1:0)

use fake_oidc::{FakeOidc, Key, Misbehave, Profile};
use fvoci_server::oidc::{ProviderKey, ResolvedProvider};

const CLIENT_ID: &str = "fvoci-client";
const CLIENT_SECRET: &str = "s3cret-value";

fn oidc_settings(providers: Vec<ResolvedProvider>, insecure: bool) -> OidcSettings {
    OidcSettings {
        providers,
        allow_insecure_loopback: insecure,
        public_origin: "http://localhost".into(),
        cache: Default::default(),
    }
}

fn provider(key: ProviderKey, issuer: &str, secret: &str) -> ResolvedProvider {
    OidcSettings::provider(key, CLIENT_ID, secret, Some(issuer)).expect("provider")
}

async fn oidc_harness(fake: &FakeOidc, keys: &[ProviderKey]) -> Harness {
    let providers = keys
        .iter()
        .map(|k| provider(*k, &fake.base, CLIENT_SECRET))
        .collect();
    Harness::start_with(Options {
        encryption: true,
        oidc: oidc_settings(providers, true),
    })
    .await
}

struct Started {
    location: String,
    state_cookie: String,
}

impl Harness {
    async fn oidc_start(&self, path: &str, cookie: Option<&str>, from: SocketAddr) -> Response {
        oidc_start_on(&self.app, path, cookie, from).await
    }

    async fn begin(&self, path: &str, cookie: Option<&str>, from: SocketAddr) -> Started {
        begin_on(&self.app, path, cookie, from).await
    }

    async fn callback(
        &self,
        provider: &str,
        query: &str,
        state_cookie: Option<&str>,
        session: Option<&str>,
        from: SocketAddr,
    ) -> Response {
        callback_on(&self.app, provider, query, state_cookie, session, from).await
    }

    /// start → provider authorize → callback, all for `profile`.
    async fn oidc_round(
        &self,
        fake: &FakeOidc,
        start_path: &str,
        provider: &str,
        profile: Profile,
        session: Option<&str>,
        from: SocketAddr,
    ) -> Response {
        oidc_round_on(
            &self.app, fake, start_path, provider, profile, session, from,
        )
        .await
    }
}

async fn oidc_start_on(
    app: &axum::Router,
    path: &str,
    cookie: Option<&str>,
    from: SocketAddr,
) -> Response {
    let method = if path.contains("/link") {
        "POST"
    } else {
        "GET"
    };
    call(app, method, path, None, cookie, from).await
}

async fn begin_on(
    app: &axum::Router,
    path: &str,
    cookie: Option<&str>,
    from: SocketAddr,
) -> Started {
    let res = oidc_start_on(app, path, cookie, from).await;
    assert!(
        res.status == StatusCode::FOUND || res.status == StatusCode::SEE_OTHER,
        "{path}: {} {:?}",
        res.status,
        res.json
    );
    let set = res
        .headers
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with("fvoci_oidc_state="))
        .expect("state cookie")
        .to_string();
    assert!(
        set.contains("HttpOnly") && set.contains("SameSite=Lax") && set.contains("Max-Age=600")
    );
    Started {
        location: res.location(),
        state_cookie: res.cookie_named("fvoci_oidc_state").unwrap(),
    }
}

async fn callback_on(
    app: &axum::Router,
    provider: &str,
    query: &str,
    state_cookie: Option<&str>,
    session: Option<&str>,
    from: SocketAddr,
) -> Response {
    let mut cookies = Vec::new();
    if let Some(c) = state_cookie {
        cookies.push(("fvoci_oidc_state", c));
    }
    if let Some(c) = session {
        cookies.push(("fvoci_session", c));
    }
    call_with(
        app,
        "GET",
        &format!("/api/v1/auth/oidc/{provider}/callback?{query}"),
        None,
        &cookies,
        from,
        &[],
    )
    .await
}

/// start → provider authorize → callback, all for `profile`.
async fn oidc_round_on(
    app: &axum::Router,
    fake: &FakeOidc,
    start_path: &str,
    provider: &str,
    profile: Profile,
    session: Option<&str>,
    from: SocketAddr,
) -> Response {
    let started = begin_on(app, start_path, session, from).await;
    let query = fake.authorize(&started.location, profile);
    callback_on(
        app,
        provider,
        &query,
        Some(&started.state_cookie),
        session,
        from,
    )
    .await
}

#[tokio::test]
async fn oidc_login_link_unlink_and_rules() {
    let fake = FakeOidc::start(CLIENT_ID, CLIENT_SECRET, Key::rsa("rsa-1")).await;
    let h = oidc_harness(&fake, &[ProviderKey::Generic]).await;
    let providers = call(
        &h.app,
        "GET",
        "/api/v1/auth/providers",
        None,
        None,
        peer(50),
    )
    .await;
    assert_eq!(
        providers.json,
        json!({"providers": [{"provider": "generic", "label": "SSO"}], "magicLink": false, "workspaceSso": false})
    );

    // The authorization request carries PKCE S256, nonce and the exact redirect URI.
    let started = h
        .begin("/api/v1/auth/oidc/generic/start", None, peer(50))
        .await;
    let url = url::Url::parse(&started.location).unwrap();
    assert_eq!(url.path(), "/authorize");
    let params: std::collections::HashMap<String, String> =
        url.query_pairs().into_owned().collect();
    assert_eq!(
        params["redirect_uri"],
        "http://localhost/api/v1/auth/oidc/generic/callback"
    );
    assert_eq!(params["scope"], "openid email profile");
    assert_eq!(params["code_challenge"].len(), 43);
    assert!(params["nonce"].len() >= 43 && params["state"].len() >= 43);
    // State is stored sealed, keyed by its hash.
    let (stored_hash, payload): (String, String) =
        sqlx::query_as("SELECT state_hash, payload FROM fvoci.oidc_states")
            .fetch_one(&h.admin)
            .await
            .unwrap();
    assert_eq!(stored_hash, hash_token(&params["state"]));
    assert!(payload.starts_with("enc:v2:k1:") && !payload.contains(&params["nonce"]));

    // Unknown identity: not linked, no session.
    let query = fake.authorize(
        &started.location,
        Profile::new("ext-1", "kim@example.com", true),
    );
    let res = h
        .callback(
            "generic",
            &query,
            Some(&started.state_cookie),
            None,
            peer(50),
        )
        .await;
    assert_eq!(res.status, StatusCode::FOUND);
    assert_eq!(
        res.location(),
        "http://localhost/login?error=oidc_not_linked"
    );
    assert!(res.cookie().is_none());
    assert!(res
        .headers
        .get_all("set-cookie")
        .iter()
        .any(|v| v.to_str().unwrap().starts_with("fvoci_oidc_state=;")));

    // Link from account settings.
    let (user_id, email, cookie) = h.member("kim").await;
    let res = h
        .oidc_round(
            &fake,
            "/api/v1/auth/oidc/generic/link",
            "generic",
            Profile::new("ext-1", "Kim@Example.com", true),
            Some(&cookie),
            peer(51),
        )
        .await;
    assert_eq!(res.location(), "http://localhost/settings/account?linked=1");
    let ids = call(
        &h.app,
        "GET",
        "/api/v1/auth/identities",
        None,
        Some(&cookie),
        peer(51),
    )
    .await;
    assert_eq!(ids.json["items"][0]["provider"], "generic");
    assert_eq!(ids.json["items"][0]["email"], "kim@example.com");
    assert_eq!(
        h.count(
            "SELECT count(*) FROM fvoci.events WHERE verb = 'identity.linked' AND actor_user_id = $1",
            user_id
        )
        .await,
        1
    );

    // Sign in with it.
    let res = h
        .oidc_round(
            &fake,
            "/api/v1/auth/oidc/generic/start",
            "generic",
            Profile::new("ext-1", "whatever@example.com", false),
            None,
            peer(52),
        )
        .await;
    assert_eq!(res.location(), "http://localhost/");
    let session = res.cookie().expect("session");
    let me = call(
        &h.app,
        "GET",
        "/api/v1/auth/me",
        None,
        Some(&session),
        peer(52),
    )
    .await;
    assert_eq!(me.json["email"], email);
    assert_eq!(
        h.login_methods(user_id).await.last().unwrap(),
        "oidc:generic"
    );

    // Same provider again, or the same subject for someone else: refused.
    let res = h
        .oidc_round(
            &fake,
            "/api/v1/auth/oidc/generic/link",
            "generic",
            Profile::new("ext-2", "kim@example.com", true),
            Some(&cookie),
            peer(53),
        )
        .await;
    assert_eq!(
        res.location(),
        "http://localhost/settings/account?error=oidc_already_linked"
    );
    let (_other_id, _other_email, other_cookie) = h.member("lee").await;
    let res = h
        .oidc_round(
            &fake,
            "/api/v1/auth/oidc/generic/link",
            "generic",
            Profile::new("ext-1", "lee@example.com", true),
            Some(&other_cookie),
            peer(53),
        )
        .await;
    assert_eq!(
        res.location(),
        "http://localhost/settings/account?error=oidc_already_linked"
    );

    // Link needs a session; the state is bound to the linking user.
    let res = h
        .oidc_start("/api/v1/auth/oidc/generic/link", None, peer(54))
        .await;
    assert_eq!(res.status, StatusCode::UNAUTHORIZED);
    let started = h
        .begin(
            "/api/v1/auth/oidc/generic/link",
            Some(&other_cookie),
            peer(54),
        )
        .await;
    let query = fake.authorize(
        &started.location,
        Profile::new("ext-9", "x@example.com", true),
    );
    let res = h
        .callback(
            "generic",
            &query,
            Some(&started.state_cookie),
            Some(&cookie),
            peer(54),
        )
        .await;
    assert_eq!(
        res.location(),
        "http://localhost/settings/account?error=oidc_state_mismatch"
    );

    // Unlink: the password remains, so it is allowed.
    let res = call(
        &h.app,
        "POST",
        "/api/v1/auth/oidc/generic/unlink",
        None,
        Some(&cookie),
        peer(55),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK, "{:?}", res.json);
    let res = call(
        &h.app,
        "POST",
        "/api/v1/auth/oidc/generic/unlink",
        None,
        Some(&cookie),
        peer(55),
    )
    .await;
    assert_eq!(res.status, StatusCode::NOT_FOUND);
    assert_eq!(res.code(), "identity_link_not_found");
    let res = call(
        &h.app,
        "POST",
        "/api/v1/auth/oidc/github/unlink",
        None,
        Some(&cookie),
        peer(55),
    )
    .await;
    assert_eq!(res.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        h.count(
            "SELECT count(*) FROM fvoci.events WHERE verb = 'identity.unlinked' AND actor_user_id = $1",
            user_id
        )
        .await,
        1
    );

    // A password-less account cannot drop its last sign-in method.
    let nopw = h.insert_user("nopw@example.com", None).await;
    h.add_membership(nopw, "member").await;
    sqlx::query("INSERT INTO fvoci.identity_links (id, user_id, provider, provider_user_id, email) VALUES ($1, $2, 'generic', 'ext-nopw', NULL)")
        .bind(Uuid::now_v7())
        .bind(nopw)
        .execute(&h.admin)
        .await
        .unwrap();
    let res = h
        .oidc_round(
            &fake,
            "/api/v1/auth/oidc/generic/start",
            "generic",
            Profile::new("ext-nopw", "nopw@example.com", true),
            None,
            peer(56),
        )
        .await;
    let nopw_cookie = res.cookie().expect("session");
    let res = call(
        &h.app,
        "POST",
        "/api/v1/auth/oidc/generic/unlink",
        None,
        Some(&nopw_cookie),
        peer(56),
    )
    .await;
    assert_eq!(res.status, StatusCode::CONFLICT);
    assert_eq!(res.code(), "oidc_last_method");

    // Suspended accounts get no session through OIDC either.
    sqlx::query("UPDATE fvoci.users SET suspended_at = now() WHERE id = $1")
        .bind(nopw)
        .execute(&h.admin)
        .await
        .unwrap();
    let res = h
        .oidc_round(
            &fake,
            "/api/v1/auth/oidc/generic/start",
            "generic",
            Profile::new("ext-nopw", "nopw@example.com", true),
            None,
            peer(57),
        )
        .await;
    assert_eq!(
        res.location(),
        "http://localhost/login?error=oidc_provider_error"
    );
    assert!(res.cookie().is_none());

    // Identity links are owner-scoped for the app role.
    let visible: i64 = sqlx::query_scalar("SELECT count(*) FROM fvoci.identity_links")
        .fetch_one(&h.app_pool)
        .await
        .unwrap();
    assert_eq!(visible, 0);
    h.finish().await;
}

#[tokio::test]
async fn oidc_state_is_single_use_and_bound_to_the_browser() {
    let fake = FakeOidc::start(CLIENT_ID, CLIENT_SECRET, Key::ec("ec-1")).await;
    let h = oidc_harness(&fake, &[ProviderKey::Generic, ProviderKey::Google]).await;
    let (_user_id, _email, cookie) = h.member("park").await;
    h.oidc_round(
        &fake,
        "/api/v1/auth/oidc/generic/link",
        "generic",
        Profile::new("ext-park", "park@example.com", true),
        Some(&cookie),
        peer(60),
    )
    .await;
    let profile = || Profile::new("ext-park", "park@example.com", true);
    let mismatch = "http://localhost/login?error=oidc_state_mismatch";

    // No state cookie (another browser).
    let started = h
        .begin("/api/v1/auth/oidc/generic/start", None, peer(61))
        .await;
    let query = fake.authorize(&started.location, profile());
    let res = h.callback("generic", &query, None, None, peer(61)).await;
    assert_eq!(res.location(), mismatch);
    // The cookie of a different flow.
    let other = h
        .begin("/api/v1/auth/oidc/generic/start", None, peer(61))
        .await;
    let res = h
        .callback("generic", &query, Some(&other.state_cookie), None, peer(61))
        .await;
    assert_eq!(res.location(), mismatch);
    // A tampered MAC.
    let mut forged = started.state_cookie.clone();
    let last = forged.pop().unwrap();
    forged.push(if last == 'a' { 'b' } else { 'a' });
    let res = h
        .callback("generic", &query, Some(&forged), None, peer(61))
        .await;
    assert_eq!(res.location(), mismatch);
    // The right cookie still works once, then the state is gone.
    let res = h
        .callback(
            "generic",
            &query,
            Some(&started.state_cookie),
            None,
            peer(61),
        )
        .await;
    assert_eq!(res.location(), "http://localhost/", "{:?}", res.headers);
    let res = h
        .callback(
            "generic",
            &query,
            Some(&started.state_cookie),
            None,
            peer(61),
        )
        .await;
    assert_eq!(res.location(), mismatch);

    // Callback on another provider's path.
    let started = h
        .begin("/api/v1/auth/oidc/generic/start", None, peer(62))
        .await;
    let query = fake.authorize(&started.location, profile());
    let res = h
        .callback(
            "google",
            &query,
            Some(&started.state_cookie),
            None,
            peer(62),
        )
        .await;
    assert_eq!(res.location(), mismatch);

    // Provider error parameter.
    let started = h
        .begin("/api/v1/auth/oidc/generic/start", None, peer(62))
        .await;
    let state = url::Url::parse(&started.location)
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == "state")
        .unwrap()
        .1
        .into_owned();
    let res = h
        .callback(
            "generic",
            &format!("error=access_denied&state={state}"),
            Some(&started.state_cookie),
            None,
            peer(62),
        )
        .await;
    assert_eq!(
        res.location(),
        "http://localhost/login?error=oidc_provider_error"
    );

    // Expired state.
    let started = h
        .begin("/api/v1/auth/oidc/generic/start", None, peer(63))
        .await;
    let query = fake.authorize(&started.location, profile());
    sqlx::query("UPDATE fvoci.oidc_states SET expires_at = now() - interval '1 second'")
        .execute(&h.admin)
        .await
        .unwrap();
    let res = h
        .callback(
            "generic",
            &query,
            Some(&started.state_cookie),
            None,
            peer(63),
        )
        .await;
    assert_eq!(res.location(), mismatch);

    // Unknown provider key, unconfigured provider, bad query.
    let res = call(
        &h.app,
        "GET",
        "/api/v1/auth/oidc/github/start",
        None,
        None,
        peer(64),
    )
    .await;
    assert_eq!(res.status, StatusCode::BAD_REQUEST);
    let res = call(
        &h.app,
        "GET",
        "/api/v1/auth/oidc/kakao/start",
        None,
        None,
        peer(64),
    )
    .await;
    assert_eq!(res.status, StatusCode::NOT_FOUND);
    assert_eq!(res.code(), "provider_not_configured");
    let res = call(
        &h.app,
        "GET",
        "/api/v1/auth/oidc/generic/start?bogus=1",
        None,
        None,
        peer(64),
    )
    .await;
    assert_eq!(res.status, StatusCode::BAD_REQUEST);
    let res = call(
        &h.app,
        "GET",
        "/api/v1/auth/oidc/generic/start?invitation=x&consents=nope",
        None,
        None,
        peer(64),
    )
    .await;
    assert_eq!(res.code(), "invalid_consents_query");

    // Expired rows are collected.
    fvoci_server::jobs::run_magic_token_gc(
        &h.app_pool,
        chrono::Utc::now(),
        &tokio_util::sync::CancellationToken::new(),
    )
    .await
    .unwrap();
    let expired: i64 =
        sqlx::query_scalar("SELECT count(*) FROM fvoci.oidc_states WHERE expires_at <= now()")
            .fetch_one(&h.admin)
            .await
            .unwrap();
    assert_eq!(expired, 0);
    h.finish().await;
}

#[tokio::test]
async fn oidc_id_token_validation_failures_issue_nothing() {
    let fake = FakeOidc::start(CLIENT_ID, CLIENT_SECRET, Key::rsa("rsa-1")).await;
    let h = oidc_harness(&fake, &[ProviderKey::Generic]).await;
    let (user_id, _email, cookie) = h.member("choi").await;
    h.oidc_round(
        &fake,
        "/api/v1/auth/oidc/generic/link",
        "generic",
        Profile::new("ext-choi", "choi@example.com", true),
        Some(&cookie),
        peer(70),
    )
    .await;
    let before = h
        .count(
            "SELECT count(*) FROM fvoci.sessions WHERE user_id = $1",
            user_id,
        )
        .await;
    for (n, misbehave) in [
        Misbehave::WrongAudience,
        Misbehave::WrongIssuer,
        Misbehave::WrongNonce,
        Misbehave::Expired,
        Misbehave::AlgNone,
        Misbehave::ForeignKey,
        Misbehave::NoIdToken,
        Misbehave::HmacWithJwks,
        Misbehave::HmacWithClientSecret,
        Misbehave::UnknownKid,
        Misbehave::ExpiredBeyondSkew,
        Misbehave::IssuedInFuture,
        Misbehave::ExtraAudience,
        Misbehave::Oversize,
    ]
    .into_iter()
    .enumerate()
    {
        fake.set(|i| i.misbehave = misbehave.clone());
        let res = h
            .oidc_round(
                &fake,
                "/api/v1/auth/oidc/generic/start",
                "generic",
                Profile::new("ext-choi", "choi@example.com", true),
                None,
                peer(71 + n as u8),
            )
            .await;
        assert_eq!(
            res.location(),
            "http://localhost/login?error=oidc_provider_error",
            "{misbehave:?}"
        );
        assert!(res.cookie().is_none(), "{misbehave:?}");
    }
    assert_eq!(
        h.count(
            "SELECT count(*) FROM fvoci.sessions WHERE user_id = $1",
            user_id
        )
        .await,
        before
    );

    // Discovery and JWKS were fetched once and cached across all rounds.
    assert_eq!(
        fake.discovery_hits
            .load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    // ForeignKey named the published kid with the other key type, which no
    // cached key matches: exactly one forced refresh. UnknownKid right after
    // is inside the refresh window and does not refetch.
    assert_eq!(fake.jwks_hits.load(std::sync::atomic::Ordering::SeqCst), 2);

    h.finish().await;
}

#[tokio::test]
async fn oidc_jwks_rotation_refreshes_once() {
    let fake = FakeOidc::start(CLIENT_ID, CLIENT_SECRET, Key::ec("ec-1")).await;
    let h = oidc_harness(&fake, &[ProviderKey::Generic]).await;
    let (_user_id, _email, cookie) = h.member("rot").await;
    h.oidc_round(
        &fake,
        "/api/v1/auth/oidc/generic/link",
        "generic",
        Profile::new("ext-rot", "rot@example.com", true),
        Some(&cookie),
        peer(90),
    )
    .await;
    let login = |from| {
        h.oidc_round(
            &fake,
            "/api/v1/auth/oidc/generic/start",
            "generic",
            Profile::new("ext-rot", "rot@example.com", true),
            None,
            from,
        )
    };
    assert_eq!(login(peer(91)).await.location(), "http://localhost/");
    assert_eq!(fake.jwks_hits.load(std::sync::atomic::Ordering::SeqCst), 1);
    fake.set(|i| {
        i.unpublished = Some(Key::ec("ec-2"));
        i.misbehave = Misbehave::UnpublishedKey;
    });
    assert_eq!(login(peer(92)).await.location(), "http://localhost/");
    assert_eq!(fake.jwks_hits.load(std::sync::atomic::Ordering::SeqCst), 2);
    // A second unknown key right away does not refetch again (bounded).
    fake.set(|i| {
        i.unpublished = Some(Key::ec("ec-3"));
        i.misbehave = Misbehave::UnpublishedKey;
    });
    assert_eq!(
        login(peer(93)).await.location(),
        "http://localhost/login?error=oidc_provider_error"
    );
    assert_eq!(fake.jwks_hits.load(std::sync::atomic::Ordering::SeqCst), 2);
    h.finish().await;
}

#[tokio::test]
async fn oidc_rotation_removing_the_old_key_and_clock_skew() {
    let fake = FakeOidc::start(CLIENT_ID, CLIENT_SECRET, Key::ec("ec-1")).await;
    let h = oidc_harness(&fake, &[ProviderKey::Generic]).await;
    let (_user_id, _email, cookie) = h.member("skew").await;
    h.oidc_round(
        &fake,
        "/api/v1/auth/oidc/generic/link",
        "generic",
        Profile::new("ext-skew", "skew@example.com", true),
        Some(&cookie),
        peer(110),
    )
    .await;
    let login = |from| {
        h.oidc_round(
            &fake,
            "/api/v1/auth/oidc/generic/start",
            "generic",
            Profile::new("ext-skew", "skew@example.com", true),
            None,
            from,
        )
    };
    // exp 30 s ago is inside the tolerated skew.
    fake.set(|i| i.misbehave = Misbehave::ExpiredWithinSkew);
    assert_eq!(login(peer(111)).await.location(), "http://localhost/");
    assert_eq!(fake.jwks_hits.load(std::sync::atomic::Ordering::SeqCst), 1);

    // The provider replaces ec-1 with ec-2: the cached set misses the new
    // kid, one forced refresh picks it up.
    fake.set(|i| {
        let old = std::mem::replace(&mut i.keys, vec![Key::ec("ec-2")]);
        i.retired = old.into_iter().next();
    });
    assert_eq!(login(peer(112)).await.location(), "http://localhost/");
    assert_eq!(fake.jwks_hits.load(std::sync::atomic::Ordering::SeqCst), 2);
    // A token under the removed key is refused, without another fetch.
    fake.set(|i| i.misbehave = Misbehave::RetiredKey);
    let res = login(peer(113)).await;
    assert_eq!(
        res.location(),
        "http://localhost/login?error=oidc_provider_error"
    );
    assert!(res.cookie().is_none());
    assert_eq!(fake.jwks_hits.load(std::sync::atomic::Ordering::SeqCst), 2);
    h.finish().await;
}

#[tokio::test]
async fn oidc_redirecting_jwks_and_private_issuers_are_refused() {
    let fake = FakeOidc::start(CLIENT_ID, CLIENT_SECRET, Key::ec("ec-1")).await;
    let target = FakeOidc::start(CLIENT_ID, CLIENT_SECRET, Key::ec("ec-1")).await;
    let h = oidc_harness(&fake, &[ProviderKey::Generic]).await;
    let (user_id, _email, cookie) = h.member("redir").await;
    // Redirect to a JWKS that would verify the token: never followed.
    fake.set(|i| i.redirect_jwks_to = Some(format!("{}/jwks", target.base)));
    target.set(|i| i.keys = vec![Key::ec("ec-1")]);
    let res = h
        .oidc_round(
            &fake,
            "/api/v1/auth/oidc/generic/link",
            "generic",
            Profile::new("ext-redir", "redir@example.com", true),
            Some(&cookie),
            peer(120),
        )
        .await;
    assert!(
        res.location().contains("error=oidc_provider_error"),
        "{}",
        res.location()
    );
    assert_eq!(fake.jwks_hits.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(
        target.jwks_hits.load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    assert_eq!(
        h.count(
            "SELECT count(*) FROM fvoci.identity_links WHERE user_id = $1",
            user_id
        )
        .await,
        0
    );
    h.finish().await;

    // Instance providers on private or link-local addresses are refused
    // before any request, with and without the loopback development mode.
    for (issuer, insecure) in [
        ("https://10.0.0.5", false),
        ("https://192.168.1.10", true),
        ("http://169.254.169.254", true),
        ("https://[fd00::1]", false),
    ] {
        let h = Harness::start_with(Options {
            encryption: true,
            oidc: oidc_settings(
                vec![provider(ProviderKey::Generic, issuer, CLIENT_SECRET)],
                insecure,
            ),
        })
        .await;
        let started = std::time::Instant::now();
        let res = call(
            &h.app,
            "GET",
            "/api/v1/auth/oidc/generic/start",
            None,
            None,
            peer(121),
        )
        .await;
        assert_eq!(res.status, StatusCode::INTERNAL_SERVER_ERROR, "{issuer}");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "{issuer}"
        );
        h.finish().await;
    }
}

#[tokio::test]
async fn oidc_outbound_fetches_are_ssrf_guarded() {
    let fake = FakeOidc::start(CLIENT_ID, CLIENT_SECRET, Key::ec("ec-1")).await;
    // Production policy: plain http to loopback is refused before any fetch.
    let strict = Harness::start_with(Options {
        encryption: true,
        oidc: oidc_settings(
            vec![provider(ProviderKey::Generic, &fake.base, CLIENT_SECRET)],
            false,
        ),
    })
    .await;
    let res = call(
        &strict.app,
        "GET",
        "/api/v1/auth/oidc/generic/start",
        None,
        None,
        peer(100),
    )
    .await;
    assert_eq!(res.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(
        fake.discovery_hits
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    strict.finish().await;

    let h = oidc_harness(&fake, &[ProviderKey::Generic]).await;
    // Discovery that names another issuer, redirects, or is oversized.
    fake.set(|i| i.discovery_issuer = Some("http://127.0.0.1:1".into()));
    let res = call(
        &h.app,
        "GET",
        "/api/v1/auth/oidc/generic/start",
        None,
        None,
        peer(101),
    )
    .await;
    assert_eq!(res.status, StatusCode::INTERNAL_SERVER_ERROR);
    fake.set(|i| {
        i.discovery_issuer = None;
        i.redirect_discovery_to =
            Some("http://127.0.0.1:1/.well-known/openid-configuration".into());
    });
    let res = call(
        &h.app,
        "GET",
        "/api/v1/auth/oidc/generic/start",
        None,
        None,
        peer(101),
    )
    .await;
    assert_eq!(res.status, StatusCode::INTERNAL_SERVER_ERROR);
    fake.set(|i| {
        i.redirect_discovery_to = None;
        i.oversized_discovery = true;
    });
    let res = call(
        &h.app,
        "GET",
        "/api/v1/auth/oidc/generic/start",
        None,
        None,
        peer(101),
    )
    .await;
    assert_eq!(res.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM fvoci.oidc_states")
            .fetch_one(&h.admin)
            .await
            .unwrap(),
        0
    );

    // A workspace admin pointing SSO at a private address gets nothing fetched.
    let res = call(
        &h.app,
        "PUT",
        &format!("/api/v1/workspaces/{}/oidc", h.workspace_id),
        Some(json!({"issuer": "http://169.254.169.254/", "clientId": "c", "clientSecret": "s"})),
        Some(&h.owner_cookie),
        peer(102),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK, "{:?}", res.json);
    let res = call(
        &h.app,
        "GET",
        "/api/v1/auth/sso?slug=acme",
        None,
        None,
        peer(102),
    )
    .await;
    assert_eq!(res.status, StatusCode::INTERNAL_SERVER_ERROR);
    h.finish().await;
}

#[tokio::test]
async fn workspace_oidc_config_is_admin_only_and_sealed() {
    let fake = FakeOidc::start(CLIENT_ID, CLIENT_SECRET, Key::ec("ec-1")).await;
    let h = oidc_harness(&fake, &[]).await;
    let path = format!("/api/v1/workspaces/{}/oidc", h.workspace_id);
    let res = call(&h.app, "GET", &path, None, Some(&h.owner_cookie), peer(110)).await;
    assert_eq!(
        res.json,
        json!({"issuer": null, "clientId": null, "label": null})
    );

    let bad = call(
        &h.app,
        "PUT",
        &path,
        Some(json!({"issuer": "ftp://idp", "clientId": "c", "clientSecret": "s"})),
        Some(&h.owner_cookie),
        peer(110),
    )
    .await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);
    let extra = call(
        &h.app,
        "PUT",
        &path,
        Some(json!({"issuer": &fake.base, "clientId": "c", "clientSecret": "s", "x": 1})),
        Some(&h.owner_cookie),
        peer(110),
    )
    .await;
    assert_eq!(extra.status, StatusCode::BAD_REQUEST);
    let put = call(
        &h.app,
        "PUT",
        &path,
        Some(json!({"issuer": format!("{}/", fake.base), "clientId": " fvoci-client ", "clientSecret": CLIENT_SECRET, "label": "  "})),
        Some(&h.owner_cookie),
        peer(110),
    )
    .await;
    assert_eq!(put.status, StatusCode::OK, "{:?}", put.json);
    assert_eq!(
        put.json,
        json!({"issuer": fake.base, "clientId": "fvoci-client", "label": "SSO"})
    );
    let stored: String = sqlx::query_scalar("SELECT client_secret FROM fvoci.workspace_oidc")
        .fetch_one(&h.admin)
        .await
        .unwrap();
    assert!(stored.starts_with("enc:v2:k1:") && !stored.contains(CLIENT_SECRET));
    let res = call(&h.app, "GET", &path, None, Some(&h.owner_cookie), peer(110)).await;
    assert_eq!(res.json["clientId"], "fvoci-client");
    assert!(res.json.get("clientSecret").is_none());
    let providers = call(
        &h.app,
        "GET",
        "/api/v1/auth/providers",
        None,
        None,
        peer(110),
    )
    .await;
    assert_eq!(providers.json["workspaceSso"], true);

    // Members cannot manage; outsiders do not see the workspace.
    let (_m, _e, member_cookie) = h.member("mem").await;
    let res = call(&h.app, "GET", &path, None, Some(&member_cookie), peer(111)).await;
    assert_eq!(res.status, StatusCode::FORBIDDEN);
    h.insert_user("out@example.com", Some(PASSWORD)).await;
    let out = h.login("out@example.com", PASSWORD, peer(111)).await;
    let out_cookie = out.cookie().unwrap();
    let res = call(&h.app, "DELETE", &path, None, Some(&out_cookie), peer(111)).await;
    assert_eq!(res.status, StatusCode::NOT_FOUND);
    // The app role cannot read another tenant's row.
    let visible: i64 = sqlx::query_scalar("SELECT count(*) FROM fvoci.workspace_oidc")
        .fetch_one(&h.app_pool)
        .await
        .unwrap();
    assert_eq!(visible, 0);

    let res = call(
        &h.app,
        "DELETE",
        &path,
        None,
        Some(&h.owner_cookie),
        peer(112),
    )
    .await;
    assert_eq!(res.json, json!({"ok": true}));
    let res = call(
        &h.app,
        "DELETE",
        &path,
        None,
        Some(&h.owner_cookie),
        peer(112),
    )
    .await;
    assert_eq!(res.status, StatusCode::NOT_FOUND);
    let res = call(
        &h.app,
        "GET",
        "/api/v1/auth/sso?slug=acme",
        None,
        None,
        peer(112),
    )
    .await;
    assert_eq!(res.status, StatusCode::NOT_FOUND);
    assert_eq!(res.code(), "provider_not_configured");
    h.finish().await;
}

#[tokio::test]
async fn workspace_sso_login_and_jit_join() {
    let fake = FakeOidc::start(CLIENT_ID, CLIENT_SECRET, Key::rsa("rsa-1")).await;
    let h = oidc_harness(&fake, &[]).await;
    let res = call(
        &h.app,
        "PUT",
        &format!("/api/v1/workspaces/{}/oidc", h.workspace_id),
        Some(json!({"issuer": &fake.base, "clientId": CLIENT_ID, "clientSecret": CLIENT_SECRET, "label": "사내 SSO"})),
        Some(&h.owner_cookie),
        peer(120),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK);
    let sso = "/api/v1/auth/sso?slug=acme";

    // No auto-join domains yet: unknown identities are not linked.
    let res = h
        .oidc_round(
            &fake,
            sso,
            "generic",
            Profile::new("corp-1", "new@corp.example", true),
            None,
            peer(121),
        )
        .await;
    assert_eq!(
        res.location(),
        "http://localhost/login?error=oidc_not_linked"
    );

    sqlx::query(
        "UPDATE fvoci.workspaces SET auto_join_domains = ARRAY['Corp.Example'] WHERE id = $1",
    )
    .bind(h.workspace_id)
    .execute(&h.admin)
    .await
    .unwrap();
    // Unverified email or another domain: still not linked.
    let res = h
        .oidc_round(
            &fake,
            sso,
            "generic",
            Profile::new("corp-1", "new@corp.example", false),
            None,
            peer(122),
        )
        .await;
    assert_eq!(
        res.location(),
        "http://localhost/login?error=oidc_not_linked"
    );
    let res = h
        .oidc_round(
            &fake,
            sso,
            "generic",
            Profile::new("corp-1", "new@other.example", true),
            None,
            peer(122),
        )
        .await;
    assert_eq!(
        res.location(),
        "http://localhost/login?error=oidc_not_linked"
    );
    // An existing account's email is never taken over.
    h.insert_user("taken@corp.example", Some(PASSWORD)).await;
    let res = h
        .oidc_round(
            &fake,
            sso,
            "generic",
            Profile::new("corp-2", "taken@corp.example", true),
            None,
            peer(123),
        )
        .await;
    assert_eq!(
        res.location(),
        "http://localhost/login?error=oidc_not_linked"
    );

    // Verified email in an allowed domain: password-less member, linked.
    let res = h
        .oidc_round(
            &fake,
            sso,
            "generic",
            Profile::new("corp-1", "New@Corp.Example", true),
            None,
            peer(124),
        )
        .await;
    assert_eq!(res.location(), "http://localhost/", "{:?}", res.headers);
    let session = res.cookie().expect("session");
    let (user_id, has_password, name): (Uuid, bool, String) = sqlx::query_as(
        "SELECT id, password_hash IS NOT NULL, given_name FROM fvoci.users WHERE email = 'new@corp.example'",
    )
    .fetch_one(&h.admin)
    .await
    .unwrap();
    assert!(!has_password);
    assert_eq!(name, "외부 사용자");
    let (role,): (String,) = sqlx::query_as(
        "SELECT role FROM fvoci.memberships WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(h.workspace_id)
    .bind(user_id)
    .fetch_one(&h.admin)
    .await
    .unwrap();
    assert_eq!(role, "member");
    let (subject,): (String,) =
        sqlx::query_as("SELECT provider_user_id FROM fvoci.identity_links WHERE user_id = $1")
            .bind(user_id)
            .fetch_one(&h.admin)
            .await
            .unwrap();
    assert_eq!(subject, format!("{}:corp-1", h.workspace_id));
    let me = call(
        &h.app,
        "GET",
        "/api/v1/auth/me",
        None,
        Some(&session),
        peer(124),
    )
    .await;
    assert_eq!(me.json["hasPassword"], false);

    // The next SSO login uses the link; no second account.
    let res = h
        .oidc_round(
            &fake,
            sso,
            "generic",
            Profile::new("corp-1", "new@corp.example", true),
            None,
            peer(125),
        )
        .await;
    assert_eq!(res.location(), "http://localhost/");
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM fvoci.users WHERE email LIKE '%@corp.example'"
        )
        .fetch_one(&h.admin)
        .await
        .unwrap(),
        2
    );
    // The same subject through the instance-level generic provider is a
    // different identity (workspace-scoped subject).
    assert_eq!(
        h.login_methods(user_id).await,
        vec!["oidc:generic", "oidc:generic"]
    );

    // Unknown slug.
    let res = call(
        &h.app,
        "GET",
        "/api/v1/auth/sso?slug=nope",
        None,
        None,
        peer(126),
    )
    .await;
    assert_eq!(res.code(), "provider_not_configured");
    h.finish().await;
}

#[tokio::test]
async fn oidc_invitation_accept_creates_or_requires_the_linked_account() {
    let fake = FakeOidc::start(CLIENT_ID, CLIENT_SECRET, Key::rsa("rsa-1")).await;
    let h = oidc_harness(&fake, &[ProviderKey::Google]).await;
    let invite = |email: &'static str| {
        let app = h.app.clone();
        let cookie = h.owner_cookie.clone();
        let ws = h.workspace_id;
        async move {
            let res = call(
                &app,
                "POST",
                &format!("/api/v1/workspaces/{ws}/invitations"),
                Some(json!({ "email": email, "role": "member" })),
                Some(&cookie),
                peer(130),
            )
            .await;
            assert_eq!(res.status, StatusCode::CREATED, "{:?}", res.json);
            res.json["acceptUrl"]
                .as_str()
                .unwrap()
                .rsplit('/')
                .next()
                .unwrap()
                .to_string()
        }
    };

    // New account through Google.
    let token = invite("guest1@example.com").await;
    let res = h
        .oidc_round(
            &fake,
            &format!("/api/v1/auth/oidc/google/start?invitation={token}&consents=%5B%5D"),
            "google",
            Profile::new("g-1", "someone@gmail.test", true),
            None,
            peer(131),
        )
        .await;
    assert_eq!(res.location(), "http://localhost/", "{:?}", res.headers);
    let (user_id, has_password): (Uuid, bool) = sqlx::query_as(
        "SELECT id, password_hash IS NOT NULL FROM fvoci.users WHERE email = 'guest1@example.com'",
    )
    .fetch_one(&h.admin)
    .await
    .unwrap();
    assert!(!has_password);
    assert_eq!(
        h.count(
            "SELECT count(*) FROM fvoci.memberships WHERE user_id = $1",
            user_id
        )
        .await,
        1
    );
    assert_eq!(
        h.count(
            "SELECT count(*) FROM fvoci.identity_links WHERE user_id = $1 AND provider = 'google' AND provider_user_id = 'g-1' AND email = 'someone@gmail.test'",
            user_id
        )
        .await,
        1
    );
    assert_eq!(h.login_methods(user_id).await, vec!["oidc:google"]);
    // The invitation is spent.
    let res = h
        .oidc_round(
            &fake,
            &format!("/api/v1/auth/oidc/google/start?invitation={token}"),
            "google",
            Profile::new("g-1", "someone@gmail.test", true),
            None,
            peer(132),
        )
        .await;
    assert_eq!(
        res.location(),
        "http://localhost/login?error=oidc_invitation_invalid"
    );

    // Existing account without that identity: refused (no takeover by email).
    let (_kim, kim_email, _c) = h.member("kim").await;
    let token = invite(Box::leak(kim_email.clone().into_boxed_str())).await;
    let res = h
        .oidc_round(
            &fake,
            &format!("/api/v1/auth/oidc/google/start?invitation={token}"),
            "google",
            Profile::new("g-kim", &kim_email, true),
            None,
            peer(133),
        )
        .await;
    assert_eq!(
        res.location(),
        "http://localhost/login?error=oidc_invitation_invalid"
    );

    // An identity already linked to someone cannot open a new account.
    let token = invite("guest2@example.com").await;
    let res = h
        .oidc_round(
            &fake,
            &format!("/api/v1/auth/oidc/google/start?invitation={token}"),
            "google",
            Profile::new("g-1", "someone@gmail.test", true),
            None,
            peer(134),
        )
        .await;
    assert_eq!(
        res.location(),
        "http://localhost/login?error=oidc_already_linked"
    );
    h.finish().await;
}

#[tokio::test]
async fn oidc_sign_in_still_requires_mfa() {
    let fake = FakeOidc::start(CLIENT_ID, CLIENT_SECRET, Key::ec("ec-1")).await;
    let h = oidc_harness(&fake, &[ProviderKey::Generic]).await;
    let (user_id, _email, cookie) = h.member("mfa").await;
    let (secret, _) = h.enable_mfa(&cookie, Some(PASSWORD)).await;
    h.oidc_round(
        &fake,
        "/api/v1/auth/oidc/generic/link",
        "generic",
        Profile::new("ext-mfa", "mfa@example.com", true),
        Some(&cookie),
        peer(140),
    )
    .await;
    let res = h
        .oidc_round(
            &fake,
            "/api/v1/auth/oidc/generic/start",
            "generic",
            Profile::new("ext-mfa", "mfa@example.com", true),
            None,
            peer(141),
        )
        .await;
    assert!(res.cookie().is_none());
    let location = res.location();
    let token = location
        .strip_prefix("http://localhost/login#mfa=")
        .expect("mfa fragment")
        .to_string();
    h.advance_steps(1);
    let ok = h.verify(&token, &h.code_now(&secret), peer(141)).await;
    assert_eq!(ok.status, StatusCode::OK);
    assert_eq!(h.login_methods(user_id).await.last().unwrap(), "totp");
    h.finish().await;
}

#[tokio::test]
async fn naver_oauth2_and_post_only_client_auth() {
    let fake = FakeOidc::start(CLIENT_ID, CLIENT_SECRET, Key::ec("ec-1")).await;
    let h = oidc_harness(&fake, &[ProviderKey::Naver, ProviderKey::Kakao]).await;
    let (user_id, _email, cookie) = h.member("naver").await;
    let res = h
        .oidc_round(
            &fake,
            "/api/v1/auth/oidc/naver/link",
            "naver",
            Profile::new("nv-1", "naver@naver.test", true),
            Some(&cookie),
            peer(150),
        )
        .await;
    assert_eq!(res.location(), "http://localhost/settings/account?linked=1");
    let res = h
        .oidc_round(
            &fake,
            "/api/v1/auth/oidc/naver/start",
            "naver",
            Profile::new("nv-1", "naver@naver.test", true),
            None,
            peer(151),
        )
        .await;
    assert_eq!(res.location(), "http://localhost/");
    assert_eq!(h.login_methods(user_id).await.last().unwrap(), "oidc:naver");

    // Kakao-style provider that only accepts client_secret_post.
    fake.set(|i| i.post_auth_only = true);
    let res = h
        .oidc_round(
            &fake,
            "/api/v1/auth/oidc/kakao/link",
            "kakao",
            Profile::new("kk-1", "kakao@kakao.test", true),
            Some(&cookie),
            peer(152),
        )
        .await;
    assert_eq!(
        res.location(),
        "http://localhost/settings/account?linked=1",
        "{:?}",
        res.headers
    );
    h.finish().await;
}

// ---------------------------------------------------------------------------
// Identity link issuer boundary (035) and link save vs session revocation.

async fn link_row(h: &Harness, provider: &str, subject: &str) -> Option<(Uuid, Option<String>)> {
    sqlx::query_as(
        "SELECT user_id, issuer FROM fvoci.identity_links WHERE provider = $1 AND provider_user_id = $2",
    )
    .bind(provider)
    .bind(subject)
    .fetch_optional(&h.admin)
    .await
    .unwrap()
}

#[tokio::test]
async fn oidc_same_sub_from_another_global_issuer_is_not_the_linked_account() {
    let fake_a = FakeOidc::start(CLIENT_ID, CLIENT_SECRET, Key::rsa("rsa-a")).await;
    let fake_b = FakeOidc::start(CLIENT_ID, CLIENT_SECRET, Key::rsa("rsa-b")).await;
    assert_ne!(fake_a.base, fake_b.base);
    let h = oidc_harness(&fake_a, &[ProviderKey::Generic]).await;
    let (kim, _email, kim_cookie) = h.member("kim").await;
    let res = h
        .oidc_round(
            &fake_a,
            "/api/v1/auth/oidc/generic/link",
            "generic",
            Profile::new("same-sub", "kim@example.com", true),
            Some(&kim_cookie),
            peer(60),
        )
        .await;
    assert_eq!(res.location(), "http://localhost/settings/account?linked=1");
    // New links record the verified issuer.
    assert_eq!(
        link_row(&h, "generic", "same-sub").await,
        Some((kim, Some(fake_a.base.clone())))
    );

    // The operator points the instance-level generic provider at another
    // IdP, which happens to use the same `sub` for someone else.
    let app_b = h.app_with_oidc(oidc_settings(
        vec![provider(ProviderKey::Generic, &fake_b.base, CLIENT_SECRET)],
        true,
    ));
    let res = oidc_round_on(
        &app_b,
        &fake_b,
        "/api/v1/auth/oidc/generic/start",
        "generic",
        Profile::new("same-sub", "stranger@example.com", true),
        None,
        peer(61),
    )
    .await;
    assert_eq!(
        res.location(),
        "http://localhost/login?error=oidc_not_linked"
    );
    assert!(res.cookie().is_none());
    // Nor can another account take the subject over (the unique key holds).
    let (_lee, _lee_email, lee_cookie) = h.member("lee").await;
    let res = oidc_round_on(
        &app_b,
        &fake_b,
        "/api/v1/auth/oidc/generic/link",
        "generic",
        Profile::new("same-sub", "lee@example.com", true),
        Some(&lee_cookie),
        peer(62),
    )
    .await;
    assert_eq!(
        res.location(),
        "http://localhost/settings/account?error=oidc_already_linked"
    );
    assert_eq!(
        link_row(&h, "generic", "same-sub").await,
        Some((kim, Some(fake_a.base.clone())))
    );

    // Through the original issuer the link still signs kim in.
    let res = h
        .oidc_round(
            &fake_a,
            "/api/v1/auth/oidc/generic/start",
            "generic",
            Profile::new("same-sub", "kim@example.com", true),
            None,
            peer(63),
        )
        .await;
    assert_eq!(res.location(), "http://localhost/");
    assert!(res.cookie().is_some());
    assert_eq!(h.login_methods(kim).await, vec!["password", "oidc:generic"]);
    h.finish().await;
}

#[tokio::test]
async fn workspace_sso_issuer_change_does_not_remap_existing_links() {
    let fake_a = FakeOidc::start(CLIENT_ID, CLIENT_SECRET, Key::rsa("rsa-a")).await;
    let fake_b = FakeOidc::start(CLIENT_ID, CLIENT_SECRET, Key::rsa("rsa-b")).await;
    let h = oidc_harness(&fake_a, &[]).await;
    let put = |issuer: String| {
        let app = h.app.clone();
        let cookie = h.owner_cookie.clone();
        let path = format!("/api/v1/workspaces/{}/oidc", h.workspace_id);
        async move {
            let res = call(
                &app,
                "PUT",
                &path,
                Some(json!({"issuer": issuer, "clientId": CLIENT_ID, "clientSecret": CLIENT_SECRET, "label": "사내 SSO"})),
                Some(&cookie),
                peer(130),
            )
            .await;
            assert_eq!(res.status, StatusCode::OK, "{:?}", res.json);
        }
    };
    put(fake_a.base.clone()).await;
    sqlx::query(
        "UPDATE fvoci.workspaces SET auto_join_domains = ARRAY['corp.example'] WHERE id = $1",
    )
    .bind(h.workspace_id)
    .execute(&h.admin)
    .await
    .unwrap();
    let sso = "/api/v1/auth/sso?slug=acme";
    let subject = format!("{}:corp-1", h.workspace_id);

    // JIT through IdP A creates the account and a link on issuer A.
    let res = h
        .oidc_round(
            &fake_a,
            sso,
            "generic",
            Profile::new("corp-1", "first@corp.example", true),
            None,
            peer(131),
        )
        .await;
    assert_eq!(res.location(), "http://localhost/");
    let first: Uuid =
        sqlx::query_scalar("SELECT id FROM fvoci.users WHERE email = 'first@corp.example'")
            .fetch_one(&h.admin)
            .await
            .unwrap();
    assert_eq!(
        link_row(&h, "generic", &subject).await,
        Some((first, Some(fake_a.base.clone())))
    );

    // The admin switches the workspace to IdP B; links are not touched.
    put(fake_b.base.clone()).await;
    assert_eq!(
        link_row(&h, "generic", &subject).await,
        Some((first, Some(fake_a.base.clone())))
    );
    // B's `corp-1` is someone else: not signed in as the old account, and not
    // JIT-joined either (the subject stays taken), whatever email B reports.
    for (email, from) in [
        ("first@corp.example", peer(132)),
        ("second@corp.example", peer(133)),
    ] {
        let res = h
            .oidc_round(
                &fake_b,
                sso,
                "generic",
                Profile::new("corp-1", email, true),
                None,
                from,
            )
            .await;
        assert_eq!(
            res.location(),
            "http://localhost/login?error=oidc_not_linked",
            "{email}"
        );
        assert!(res.cookie().is_none());
    }
    assert_eq!(h.login_methods(first).await, vec!["oidc:generic"]);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM fvoci.users WHERE email LIKE '%@corp.example'"
        )
        .fetch_one(&h.admin)
        .await
        .unwrap(),
        1
    );
    // A new B subject still joins normally.
    let res = h
        .oidc_round(
            &fake_b,
            sso,
            "generic",
            Profile::new("corp-2", "second@corp.example", true),
            None,
            peer(134),
        )
        .await;
    assert_eq!(res.location(), "http://localhost/");
    let second_subject = format!("{}:corp-2", h.workspace_id);
    assert_eq!(
        link_row(&h, "generic", &second_subject)
            .await
            .and_then(|(_, issuer)| issuer),
        Some(fake_b.base.clone())
    );

    // Switching back to A restores the original mapping.
    put(fake_a.base.clone()).await;
    let res = h
        .oidc_round(
            &fake_a,
            sso,
            "generic",
            Profile::new("corp-1", "first@corp.example", true),
            None,
            peer(135),
        )
        .await;
    assert_eq!(res.location(), "http://localhost/");
    assert_eq!(
        h.login_methods(first).await,
        vec!["oidc:generic", "oidc:generic"]
    );
    h.finish().await;
}

#[tokio::test]
async fn pre_issuer_links_sign_in_once_and_are_pinned_to_that_issuer() {
    let fake_a = FakeOidc::start(CLIENT_ID, CLIENT_SECRET, Key::rsa("rsa-a")).await;
    let fake_b = FakeOidc::start(CLIENT_ID, CLIENT_SECRET, Key::rsa("rsa-b")).await;
    let h = oidc_harness(&fake_a, &[ProviderKey::Generic]).await;
    let (user_id, _email, _cookie) = h.member("legacy").await;
    // A link written before 035: no issuer.
    let link_id = Uuid::now_v7();
    sqlx::query("INSERT INTO fvoci.identity_links (id, user_id, provider, provider_user_id, email) VALUES ($1, $2, 'generic', 'legacy-sub', NULL)")
        .bind(link_id)
        .bind(user_id)
        .execute(&h.admin)
        .await
        .unwrap();
    assert_eq!(
        link_row(&h, "generic", "legacy-sub").await,
        Some((user_id, None))
    );
    let res = h
        .oidc_round(
            &fake_a,
            "/api/v1/auth/oidc/generic/start",
            "generic",
            Profile::new("legacy-sub", "legacy@example.com", true),
            None,
            peer(70),
        )
        .await;
    assert_eq!(res.location(), "http://localhost/");
    assert!(res.cookie().is_some());
    // The successful sign-in recorded the issuer that verified it.
    assert_eq!(
        link_row(&h, "generic", "legacy-sub").await,
        Some((user_id, Some(fake_a.base.clone())))
    );
    // From now on another issuer's same `sub` is refused.
    let app_b = h.app_with_oidc(oidc_settings(
        vec![provider(ProviderKey::Generic, &fake_b.base, CLIENT_SECRET)],
        true,
    ));
    let res = oidc_round_on(
        &app_b,
        &fake_b,
        "/api/v1/auth/oidc/generic/start",
        "generic",
        Profile::new("legacy-sub", "legacy@example.com", true),
        None,
        peer(71),
    )
    .await;
    assert_eq!(
        res.location(),
        "http://localhost/login?error=oidc_not_linked"
    );
    assert_eq!(
        h.login_methods(user_id).await,
        vec!["password", "oidc:generic"]
    );

    // The app role cannot rewrite links; the backfill function is write-once.
    let mut tx = h.app_pool.begin().await.unwrap();
    fvoci_server::db::context::set_system(&mut tx)
        .await
        .unwrap();
    let err =
        sqlx::query("UPDATE fvoci.identity_links SET issuer = 'https://evil.test' WHERE id = $1")
            .bind(link_id)
            .execute(&mut *tx)
            .await
            .expect_err("app role has no UPDATE on identity_links");
    assert_eq!(
        err.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("42501")
    );
    tx.rollback().await.unwrap();
    let mut tx = h.app_pool.begin().await.unwrap();
    fvoci_server::db::context::set_system(&mut tx)
        .await
        .unwrap();
    let overwritten: bool = sqlx::query_scalar(
        "SELECT fvoci.app_identity_link_backfill_issuer($1, 'https://evil.test')",
    )
    .bind(link_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert!(!overwritten);
    let same: bool = sqlx::query_scalar("SELECT fvoci.app_identity_link_backfill_issuer($1, $2)")
        .bind(link_id)
        .bind(&fake_a.base)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert!(same);
    tx.commit().await.unwrap();
    assert_eq!(
        link_row(&h, "generic", "legacy-sub").await,
        Some((user_id, Some(fake_a.base.clone())))
    );
    // Outside the system context (and not the owner) RLS hides the row, so
    // even a NULL-issuer link cannot be claimed.
    let other = Uuid::now_v7();
    sqlx::query("INSERT INTO fvoci.identity_links (id, user_id, provider, provider_user_id) VALUES ($1, $2, 'google', 'legacy-g')")
        .bind(other)
        .bind(user_id)
        .execute(&h.admin)
        .await
        .unwrap();
    let claimed: bool = sqlx::query_scalar(
        "SELECT fvoci.app_identity_link_backfill_issuer($1, 'https://evil.test')",
    )
    .bind(other)
    .fetch_one(&h.app_pool)
    .await
    .unwrap();
    assert!(!claimed);
    assert_eq!(
        link_row(&h, "google", "legacy-g").await,
        Some((user_id, None))
    );
    h.finish().await;
}

const TENANT_A: &str = "9188040d-6c67-4c5b-b112-36a304b66dad";
const TENANT_B: &str = "72f988bf-86f1-41af-91ab-2d7cd011db47";

/// Makes `fake` a Microsoft `common` endpoint: the discovery issuer is the
/// `{tenantid}` template and id_tokens come from `tenant`.
fn ms_tenant(fake: &FakeOidc, tenant: &str) -> String {
    let issuer = format!("{}/{tenant}/v2.0", fake.base);
    let template = ms_template(fake);
    let tenant = tenant.to_string();
    let token_issuer = issuer.clone();
    fake.set(move |i| {
        i.discovery_issuer = Some(template);
        i.issuer = token_issuer;
        i.tid = Some(tenant);
    });
    issuer
}

fn ms_template(fake: &FakeOidc) -> String {
    format!("{}/{{tenantid}}/v2.0", fake.base)
}

async fn user_count(h: &Harness) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM fvoci.users")
        .fetch_one(&h.admin)
        .await
        .unwrap()
}

#[tokio::test]
async fn microsoft_links_pin_the_tenant_issuer_and_refuse_other_tenants() {
    let fake = FakeOidc::start(CLIENT_ID, CLIENT_SECRET, Key::rsa("rsa-ms")).await;
    let h = oidc_harness(&fake, &[ProviderKey::Microsoft]).await;
    // Microsoft's common JWKS binds every key to the issuer template.
    let template = ms_template(&fake);
    fake.set(|i| i.key_issuer = Some(template.clone()));
    let issuer_a = ms_tenant(&fake, TENANT_A);
    let (kim, kim_email, kim_cookie) = h.member("kim").await;
    let res = h
        .oidc_round(
            &fake,
            "/api/v1/auth/oidc/microsoft/link",
            "microsoft",
            Profile::new("ms-sub", &kim_email, true),
            Some(&kim_cookie),
            peer(80),
        )
        .await;
    assert_eq!(res.location(), "http://localhost/settings/account?linked=1");
    // The verified tenant issuer is stored, not the discovery template.
    assert_eq!(
        link_row(&h, "microsoft", "ms-sub").await,
        Some((kim, Some(issuer_a.clone())))
    );

    // Tenant B issues a token with the same `sub` (and even kim's email).
    ms_tenant(&fake, TENANT_B);
    let sessions = "SELECT count(*) FROM fvoci.sessions WHERE user_id = $1";
    let before = h.count(sessions, kim).await;
    let users = user_count(&h).await;
    let res = h
        .oidc_round(
            &fake,
            "/api/v1/auth/oidc/microsoft/start",
            "microsoft",
            Profile::new("ms-sub", &kim_email, true),
            None,
            peer(81),
        )
        .await;
    assert_eq!(
        res.location(),
        "http://localhost/login?error=oidc_not_linked"
    );
    assert!(res.cookie().is_none());
    assert_eq!(h.count(sessions, kim).await, before);
    assert_eq!(user_count(&h).await, users, "no account is created");
    // Nor can another account link tenant B's `sub`.
    let (_lee, lee_email, lee_cookie) = h.member("lee").await;
    let res = h
        .oidc_round(
            &fake,
            "/api/v1/auth/oidc/microsoft/link",
            "microsoft",
            Profile::new("ms-sub", &lee_email, true),
            Some(&lee_cookie),
            peer(82),
        )
        .await;
    assert_eq!(
        res.location(),
        "http://localhost/settings/account?error=oidc_already_linked"
    );
    assert_eq!(
        link_row(&h, "microsoft", "ms-sub").await,
        Some((kim, Some(issuer_a.clone())))
    );

    // Tenant A still signs kim in.
    ms_tenant(&fake, TENANT_A);
    let res = h
        .oidc_round(
            &fake,
            "/api/v1/auth/oidc/microsoft/start",
            "microsoft",
            Profile::new("ms-sub", &kim_email, true),
            None,
            peer(83),
        )
        .await;
    assert_eq!(res.location(), "http://localhost/");
    assert!(res.cookie().is_some());
    assert_eq!(h.count(sessions, kim).await, before + 1);

    // A key bound to tenant A does not verify tenant B's token, although the
    // signature is valid.
    fake.set(|i| i.key_issuer = Some(issuer_a.clone()));
    let app = h.app_with_oidc(oidc_settings(
        vec![provider(ProviderKey::Microsoft, &fake.base, CLIENT_SECRET)],
        true,
    ));
    ms_tenant(&fake, TENANT_B);
    let res = oidc_round_on(
        &app,
        &fake,
        "/api/v1/auth/oidc/microsoft/start",
        "microsoft",
        Profile::new("ms-sub", &kim_email, true),
        None,
        peer(84),
    )
    .await;
    assert_eq!(
        res.location(),
        "http://localhost/login?error=oidc_provider_error"
    );
    ms_tenant(&fake, TENANT_A);
    let res = oidc_round_on(
        &app,
        &fake,
        "/api/v1/auth/oidc/microsoft/start",
        "microsoft",
        Profile::new("ms-sub", &kim_email, true),
        None,
        peer(85),
    )
    .await;
    assert_eq!(res.location(), "http://localhost/");
    // A non-GUID tid is refused even with a consistent issuer.
    fake.set(|i| i.key_issuer = None);
    let bad = format!("{}/abc-123/v2.0", fake.base);
    fake.set(move |i| {
        i.issuer = bad;
        i.tid = Some("abc-123".into());
    });
    let res = oidc_round_on(
        &app,
        &fake,
        "/api/v1/auth/oidc/microsoft/start",
        "microsoft",
        Profile::new("ms-sub", &kim_email, true),
        None,
        peer(86),
    )
    .await;
    assert_eq!(
        res.location(),
        "http://localhost/login?error=oidc_provider_error"
    );
    assert_eq!(
        link_row(&h, "microsoft", "ms-sub").await,
        Some((kim, Some(issuer_a)))
    );
    h.finish().await;
}

#[tokio::test]
async fn microsoft_template_links_are_repinned_to_the_first_verified_tenant() {
    let fake = FakeOidc::start(CLIENT_ID, CLIENT_SECRET, Key::rsa("rsa-ms")).await;
    let h = oidc_harness(&fake, &[ProviderKey::Microsoft]).await;
    let template = ms_template(&fake);
    let (user_id, email, _cookie) = h.member("legacy").await;
    // A link written before 036 stored the discovery template.
    let link_id = Uuid::now_v7();
    sqlx::query("INSERT INTO fvoci.identity_links (id, user_id, provider, provider_user_id, email, issuer) VALUES ($1, $2, 'microsoft', 'ms-legacy', NULL, $3)")
        .bind(link_id)
        .bind(user_id)
        .bind(&template)
        .execute(&h.admin)
        .await
        .unwrap();
    let issuer_a = ms_tenant(&fake, TENANT_A);
    let res = h
        .oidc_round(
            &fake,
            "/api/v1/auth/oidc/microsoft/start",
            "microsoft",
            Profile::new("ms-legacy", &email, true),
            None,
            peer(90),
        )
        .await;
    assert_eq!(res.location(), "http://localhost/");
    assert!(res.cookie().is_some());
    assert_eq!(
        link_row(&h, "microsoft", "ms-legacy").await,
        Some((user_id, Some(issuer_a.clone())))
    );
    // Pinned: tenant B's same `sub` is refused.
    let issuer_b = ms_tenant(&fake, TENANT_B);
    let sessions = "SELECT count(*) FROM fvoci.sessions WHERE user_id = $1";
    let before = h.count(sessions, user_id).await;
    let res = h
        .oidc_round(
            &fake,
            "/api/v1/auth/oidc/microsoft/start",
            "microsoft",
            Profile::new("ms-legacy", &email, true),
            None,
            peer(91),
        )
        .await;
    assert_eq!(
        res.location(),
        "http://localhost/login?error=oidc_not_linked"
    );
    assert_eq!(h.count(sessions, user_id).await, before);
    assert_eq!(
        link_row(&h, "microsoft", "ms-legacy").await,
        Some((user_id, Some(issuer_a.clone())))
    );

    // The function never rewrites anything but the exact template, and only
    // to a GUID tenant instance of it.
    let repin = |id: Uuid, template: String, issuer: String| {
        let pool = h.app_pool.clone();
        async move {
            let mut tx = pool.begin().await.unwrap();
            fvoci_server::db::context::set_system(&mut tx)
                .await
                .unwrap();
            let done: bool =
                sqlx::query_scalar("SELECT fvoci.app_identity_link_repin_template($1, $2, $3)")
                    .bind(id)
                    .bind(template)
                    .bind(issuer)
                    .fetch_one(&mut *tx)
                    .await
                    .unwrap();
            tx.commit().await.unwrap();
            done
        }
    };
    // A stored real issuer: the template does not match it.
    assert!(!repin(link_id, template.clone(), issuer_b.clone()).await);
    assert_eq!(
        link_row(&h, "microsoft", "ms-legacy").await,
        Some((user_id, Some(issuer_a.clone())))
    );
    let other = Uuid::now_v7();
    sqlx::query("INSERT INTO fvoci.identity_links (id, user_id, provider, provider_user_id, issuer) VALUES ($1, $2, 'google', 'tmpl-g', $3)")
        .bind(other)
        .bind(user_id)
        .bind(&template)
        .execute(&h.admin)
        .await
        .unwrap();
    for issuer in [
        template.clone(),
        "https://evil.test".to_string(),
        format!("{}/abc-123/v2.0", fake.base),
        format!("{}/{TENANT_B}/v2.0/x", fake.base),
        format!("https://evil.test/{TENANT_B}/v2.0"),
    ] {
        assert!(
            !repin(other, template.clone(), issuer.clone()).await,
            "{issuer}"
        );
    }
    // A template argument that is not the stored value.
    assert!(
        !repin(
            other,
            format!("{}/{{tenantid}}/v1", fake.base),
            issuer_b.clone()
        )
        .await
    );
    assert_eq!(
        link_row(&h, "google", "tmpl-g").await,
        Some((user_id, Some(template.clone())))
    );
    // Outside the system context (and not the owner) RLS hides the row.
    let claimed: bool =
        sqlx::query_scalar("SELECT fvoci.app_identity_link_repin_template($1, $2, $3)")
            .bind(other)
            .bind(&template)
            .bind(&issuer_b)
            .fetch_one(&h.app_pool)
            .await
            .unwrap();
    assert!(!claimed);
    assert_eq!(
        link_row(&h, "google", "tmpl-g").await,
        Some((user_id, Some(template.clone())))
    );
    // The app role still cannot UPDATE identity_links directly.
    let mut tx = h.app_pool.begin().await.unwrap();
    fvoci_server::db::context::set_system(&mut tx)
        .await
        .unwrap();
    let err = sqlx::query("UPDATE fvoci.identity_links SET issuer = $2 WHERE id = $1")
        .bind(other)
        .bind(&issuer_b)
        .execute(&mut *tx)
        .await
        .expect_err("app role has no UPDATE on identity_links");
    assert_eq!(
        err.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("42501")
    );
    tx.rollback().await.unwrap();
    // The exact template to a tenant instance of it is the one rewrite.
    assert!(repin(other, template.clone(), issuer_b.clone()).await);
    assert_eq!(
        link_row(&h, "google", "tmpl-g").await,
        Some((user_id, Some(issuer_b)))
    );
    h.finish().await;
}

/// Waits (bounded) until a backend of this test database is blocked on a
/// row lock: the callback's link transaction queued behind `lock_sign_in`.
async fn wait_for_lock_waiter(h: &Harness) {
    for _ in 0..500 {
        let waiting: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_stat_activity WHERE datname = current_database() AND wait_event_type = 'Lock' AND query LIKE '%FOR UPDATE%'",
        )
        .fetch_one(&h.admin)
        .await
        .unwrap();
        if waiting > 0 {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("the link transaction never queued behind the sign-in lock");
}

#[tokio::test]
async fn oidc_link_is_not_saved_for_a_session_revoked_during_the_round_trip() {
    let fake = FakeOidc::start(CLIENT_ID, CLIENT_SECRET, Key::rsa("rsa-1")).await;
    let h = oidc_harness(&fake, &[ProviderKey::Generic]).await;
    let (user_id, _email, cookie) = h.member("revoked").await;
    let links = |h: &Harness| {
        let admin = h.admin.clone();
        async move {
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM fvoci.identity_links WHERE user_id = $1",
            )
            .bind(user_id)
            .fetch_one(&admin)
            .await
            .unwrap()
        }
    };

    // In flight: the callback has read the live session and finished the
    // provider exchange; the logout commits while the link waits for the
    // sign-in lock. The link must see the revocation.
    let started = h
        .begin("/api/v1/auth/oidc/generic/link", Some(&cookie), peer(80))
        .await;
    let query = fake.authorize(
        &started.location,
        Profile::new("inflight-sub", "revoked@example.com", true),
    );
    let mut barrier = h.admin.begin().await.unwrap();
    sqlx::query("SELECT id FROM fvoci.users WHERE id = $1 FOR UPDATE")
        .bind(user_id)
        .execute(&mut *barrier)
        .await
        .unwrap();
    let app = h.app.clone();
    let (state_cookie, session_cookie) = (started.state_cookie.clone(), cookie.clone());
    let callback = tokio::spawn(async move {
        callback_on(
            &app,
            "generic",
            &query,
            Some(&state_cookie),
            Some(&session_cookie),
            peer(80),
        )
        .await
    });
    wait_for_lock_waiter(&h).await;
    assert!(!callback.is_finished());
    sqlx::query(
        "UPDATE fvoci.sessions SET revoked_at = now() WHERE user_id = $1 AND revoked_at IS NULL",
    )
    .bind(user_id)
    .execute(&mut *barrier)
    .await
    .unwrap();
    barrier.commit().await.unwrap();
    let res = callback.await.unwrap();
    assert_eq!(res.status, StatusCode::UNAUTHORIZED, "{:?}", res.json);
    assert_eq!(res.code(), "authentication_required");
    assert!(res
        .headers
        .get_all("set-cookie")
        .iter()
        .any(|v| v.to_str().unwrap().starts_with("fvoci_oidc_state=;")));
    assert_eq!(links(&h).await, 0);
    assert_eq!(
        h.count(
            "SELECT count(*) FROM fvoci.events WHERE verb = 'identity.linked' AND actor_user_id = $1",
            user_id
        )
        .await,
        0
    );

    // Revoked before the callback arrives, with the original cookie string:
    // the callback has no live session, so nothing is linked either.
    let res = h.login("revoked@example.com", PASSWORD, peer(81)).await;
    let cookie = res.cookie().expect("cookie");
    let started = h
        .begin("/api/v1/auth/oidc/generic/link", Some(&cookie), peer(81))
        .await;
    let query = fake.authorize(
        &started.location,
        Profile::new("inflight-sub", "revoked@example.com", true),
    );
    let res = call(
        &h.app,
        "POST",
        "/api/v1/auth/logout",
        None,
        Some(&cookie),
        peer(81),
    )
    .await;
    assert!(res.status.is_success(), "{:?}", res.json);
    let res = h
        .callback(
            "generic",
            &query,
            Some(&started.state_cookie),
            Some(&cookie),
            peer(81),
        )
        .await;
    assert_eq!(
        res.location(),
        "http://localhost/settings/account?error=oidc_state_mismatch"
    );
    assert_eq!(links(&h).await, 0);

    // A live session still links.
    let res = h.login("revoked@example.com", PASSWORD, peer(82)).await;
    let cookie = res.cookie().expect("cookie");
    let res = h
        .oidc_round(
            &fake,
            "/api/v1/auth/oidc/generic/link",
            "generic",
            Profile::new("inflight-sub", "revoked@example.com", true),
            Some(&cookie),
            peer(82),
        )
        .await;
    assert_eq!(res.location(), "http://localhost/settings/account?linked=1");
    assert_eq!(links(&h).await, 1);
    h.finish().await;
}

// ---------------------------------------------------------------------------
// Post-restore decrypt probe (`fvoci-migrate --verify-secrets`).

async fn run_verify_secrets(h: &Harness, keys: Option<(&str, &str)>) -> (bool, Value, String) {
    let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_fvoci-migrate"));
    cmd.arg("--verify-secrets")
        .env_clear()
        .env("DATABASE_APP_URL", &h.db.app_url);
    if let Some((ring, active)) = keys {
        cmd.env("ENCRYPTION_KEYS", ring)
            .env("ENCRYPTION_ACTIVE_KEY_ID", active);
    }
    let out = cmd.output().await.expect("run fvoci-migrate");
    let stdout = String::from_utf8(out.stdout).unwrap();
    let stderr = String::from_utf8(out.stderr).unwrap();
    let report = serde_json::from_str(stdout.trim()).unwrap_or(Value::Null);
    (out.status.success(), report, format!("{stdout}{stderr}"))
}

#[tokio::test]
async fn verify_secrets_opens_every_sealed_value_and_fails_on_a_bad_one() {
    use fvoci_server::secret_verify::verify_sealed_secrets;
    let h = Harness::start().await;
    // One sealed value of each kind, written through the product paths where
    // they exist: MFA setup, workspace SSO PUT; the webhook row is sealed the
    // way the integrations route seals it.
    let (user_id, _email, cookie) = h.member("sealed").await;
    h.enable_mfa(&cookie, Some(PASSWORD)).await;
    let res = call(
        &h.app,
        "PUT",
        &format!("/api/v1/workspaces/{}/oidc", h.workspace_id),
        Some(json!({"issuer": "https://idp.example", "clientId": CLIENT_ID, "clientSecret": CLIENT_SECRET, "label": "SSO"})),
        Some(&h.owner_cookie),
        peer(90),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK, "{:?}", res.json);
    let webhook_id = Uuid::now_v7();
    let webhook_secret = fvoci_server::secret_box::seal(
        &encryption_keys(),
        "whsec-value",
        &fvoci_server::integrations::webhooks::webhook_secret_context(h.workspace_id, webhook_id),
    )
    .unwrap();
    sqlx::query("INSERT INTO fvoci.webhooks (id, workspace_id, url, secret, events, created_by) VALUES ($1, $2, 'https://hooks.example/x', $3, ARRAY['document.created'], $4)")
        .bind(webhook_id)
        .bind(h.workspace_id)
        .bind(&webhook_secret)
        .bind(h.owner_id)
        .execute(&h.admin)
        .await
        .unwrap();

    let keys = encryption_keys();
    let report = verify_sealed_secrets(&h.app_pool, Some(&keys))
        .await
        .unwrap();
    assert!(report.is_complete(), "{report:?}");
    assert_eq!(
        (
            report.user_mfa.checked,
            report.workspace_oidc.checked,
            report.webhooks.checked
        ),
        (1, 1, 1)
    );
    assert_eq!(report.key_ids_in_use.get("k1"), Some(&3));

    // Rotated superset keyring (new active key, old one kept): still opens.
    let rotated = format!(r#"{{"k1":"{}","k2":"{}"}}"#, "1".repeat(64), "2".repeat(64));
    let ring = Keyring::parse_named(&rotated, "k2", "ENCRYPTION_KEYS").unwrap();
    assert!(verify_sealed_secrets(&h.app_pool, Some(&ring))
        .await
        .unwrap()
        .is_complete());
    // The backed-up key id is missing: every value is key-unavailable.
    let missing = Keyring::parse_named(
        &format!(r#"{{"k2":"{}"}}"#, "2".repeat(64)),
        "k2",
        "ENCRYPTION_KEYS",
    )
    .unwrap();
    let report = verify_sealed_secrets(&h.app_pool, Some(&missing))
        .await
        .unwrap();
    assert_eq!(report.failed(), 3);
    assert_eq!(report.user_mfa.key_unavailable, vec![user_id]);
    assert_eq!(report.workspace_oidc.key_unavailable, vec![h.workspace_id]);
    assert_eq!(report.webhooks.key_unavailable, vec![webhook_id]);
    // Same key id, another key (mis-keyed restore env): every value invalid.
    let mis_keyed = Keyring::parse_named(
        &format!(r#"{{"k1":"{}"}}"#, "3".repeat(64)),
        "k1",
        "ENCRYPTION_KEYS",
    )
    .unwrap();
    let report = verify_sealed_secrets(&h.app_pool, Some(&mis_keyed))
        .await
        .unwrap();
    assert_eq!(report.failed(), 3);
    assert_eq!(report.webhooks.invalid, vec![webhook_id]);
    // No keyring at all while sealed values exist.
    let report = verify_sealed_secrets(&h.app_pool, None).await.unwrap();
    assert!(!report.keyring_configured);
    assert_eq!(report.failed(), 3);

    // The binary: good keyring passes and prints counts, never the values.
    let (ok, printed, output) = run_verify_secrets(&h, Some((ENCRYPTION, "k1"))).await;
    assert!(ok, "{output}");
    assert_eq!(printed["userMfa"]["checked"], 1);
    assert_eq!(printed["workspaceOidc"]["checked"], 1);
    assert_eq!(printed["webhooks"]["checked"], 1);
    assert_eq!(printed["keyIdsInUse"], json!({"k1": 3}));
    for secret in [
        "whsec-value",
        CLIENT_SECRET,
        webhook_secret.as_str(),
        "1111111111111111",
    ] {
        assert!(!output.contains(secret), "printed a secret");
    }
    // A mis-keyed environment fails nonzero.
    let wrong = format!(r#"{{"k1":"{}"}}"#, "3".repeat(64));
    let (ok, printed, output) = run_verify_secrets(&h, Some((&wrong, "k1"))).await;
    assert!(!ok);
    assert_eq!(printed["webhooks"]["invalid"], json!([webhook_id]));
    assert!(
        output.contains("3 sealed secret(s) do not open"),
        "{output}"
    );
    // Unset ENCRYPTION_KEYS with sealed rows fails too.
    let (ok, _printed, _output) = run_verify_secrets(&h, None).await;
    assert!(!ok);

    // One corrupted ciphertext (a value moved to another row does not open
    // under that row's context): only that row fails.
    let sso_sealed: String = sqlx::query_scalar(
        "SELECT client_secret FROM fvoci.workspace_oidc WHERE workspace_id = $1",
    )
    .bind(h.workspace_id)
    .fetch_one(&h.admin)
    .await
    .unwrap();
    sqlx::query("UPDATE fvoci.user_mfa SET totp_secret = $1 WHERE user_id = $2")
        .bind(&sso_sealed)
        .bind(user_id)
        .execute(&h.admin)
        .await
        .unwrap();
    let (ok, printed, output) = run_verify_secrets(&h, Some((ENCRYPTION, "k1"))).await;
    assert!(!ok, "{output}");
    assert_eq!(printed["userMfa"]["invalid"], json!([user_id]));
    assert_eq!(printed["workspaceOidc"]["invalid"], json!([]));
    assert_eq!(printed["webhooks"]["invalid"], json!([]));
    assert!(
        output.contains("1 sealed secret(s) do not open"),
        "{output}"
    );
    h.finish().await;
}
