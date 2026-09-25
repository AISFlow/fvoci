#![cfg(feature = "db-tests")]
#![allow(dead_code)]

//! Account lifecycle over HTTP with the real non-superuser app role:
//! withdraw / cancel / final anonymization (maintenance job), password and
//! email change, magic-link login, providers/identities, user export,
//! dashboard and locate.

#[path = "support/project_harness.rs"]
mod project_harness;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use chrono::{DateTime, Utc};
use fvoci_server::attachments::{LocalStorage, ObjectStorage};
use fvoci_server::auth::password::{hash_password, Keyring};
use fvoci_server::auth::token::hash_token;
use fvoci_server::auth::AuthService;
use fvoci_server::db::{pool, Db};
use fvoci_server::http::rate_limit::RateLimiter;
use fvoci_server::http::{router, state::AppState};
use fvoci_server::jobs::{run_daily_sweep, run_withdrawn_anonymize};
use fvoci_server::mail::{Mailer, SmtpConfig};
use project_harness::{admin_pool, wait_for_blocked_by_holder, TestDb};
use serde_json::{json, Value};
use sqlx::PgPool;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;
use uuid::Uuid;

const PEPPER: &str =
    r#"{"test":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#;
const OWNER_EMAIL: &str = "owner@example.com";
const OWNER_PASSWORD: &str = "supersecret1";
const PASSWORD: &str = "membersecret1";

fn peer(n: u8) -> SocketAddr {
    SocketAddr::from(([203, 0, 113, n], 42424))
}

fn keyring() -> Keyring {
    Keyring::parse(PEPPER, "test").expect("pepper")
}

// ---------------------------------------------------------------------------
// SMTP sink

#[derive(Clone)]
struct CapturedMail {
    to: String,
    data: String,
}

impl CapturedMail {
    fn text(&self) -> String {
        let parsed = mailparse::parse_mail(self.data.as_bytes()).expect("parse captured mail");
        let subject = parsed
            .headers
            .iter()
            .find(|h| h.get_key().eq_ignore_ascii_case("subject"))
            .map(|h| h.get_value())
            .unwrap_or_default();
        let body = parsed.get_body().expect("decode captured body");
        format!("Subject: {subject}\n\n{body}")
    }
}

struct SmtpSink {
    port: u16,
    mails: Arc<Mutex<Vec<CapturedMail>>>,
    handle: JoinHandle<()>,
}

impl SmtpSink {
    async fn spawn() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind smtp");
        let port = listener.local_addr().expect("addr").port();
        let mails = Arc::new(Mutex::new(Vec::new()));
        let captured = mails.clone();
        let handle = tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = listener.accept().await else {
                    break;
                };
                let captured = captured.clone();
                tokio::spawn(async move {
                    let _ = serve_smtp(socket, captured).await;
                });
            }
        });
        Self {
            port,
            mails,
            handle,
        }
    }

    fn snapshot(&self) -> Vec<CapturedMail> {
        self.mails.lock().expect("mails").clone()
    }

    async fn wait_for(&self, predicate: impl Fn(&CapturedMail) -> bool) -> CapturedMail {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(mail) = self.snapshot().into_iter().find(&predicate) {
                return mail;
            }
            assert!(
                Instant::now() < deadline,
                "smtp sink did not capture expected mail"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

impl Drop for SmtpSink {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

async fn serve_smtp(
    socket: tokio::net::TcpStream,
    captured: Arc<Mutex<Vec<CapturedMail>>>,
) -> Result<(), std::io::Error> {
    let (reader, mut writer) = socket.into_split();
    let mut reader = BufReader::new(reader);
    writer.write_all(b"220 fvoci-test\r\n").await?;
    let mut rcpt = String::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).await? == 0 {
            break;
        }
        let command = line.trim_end_matches(['\r', '\n']);
        let upper = command.to_ascii_uppercase();
        if upper.starts_with("RCPT TO:") {
            rcpt = command
                .split(':')
                .nth(1)
                .unwrap_or("")
                .trim()
                .trim_matches(|c| c == '<' || c == '>')
                .to_string();
            writer.write_all(b"250 ok\r\n").await?;
        } else if upper == "DATA" {
            writer.write_all(b"354 go\r\n").await?;
            let mut body = String::new();
            loop {
                let mut data_line = String::new();
                reader.read_line(&mut data_line).await?;
                if data_line == ".\r\n" || data_line == ".\n" {
                    break;
                }
                body.push_str(&data_line);
            }
            captured.lock().expect("mails").push(CapturedMail {
                to: rcpt.clone(),
                data: body,
            });
            writer.write_all(b"250 ok\r\n").await?;
        } else if upper == "QUIT" {
            writer.write_all(b"221 bye\r\n").await?;
            break;
        } else {
            writer.write_all(b"250 ok\r\n").await?;
        }
    }
    Ok(())
}

fn token_after(text: &str, marker: &str) -> String {
    text.split(marker)
        .nth(1)
        .and_then(|rest| rest.split(|c: char| c.is_whitespace()).next())
        .expect("token in mail")
        .to_string()
}

// ---------------------------------------------------------------------------
// App harness

struct Harness {
    db: TestDb,
    app: axum::Router,
    admin: PgPool,
    app_pool: PgPool,
    storage: ObjectStorage,
    storage_root: PathBuf,
    mailer: Arc<Mailer>,
    sink: SmtpSink,
    owner_cookie: String,
    owner_id: Uuid,
    workspace_id: Uuid,
}

impl Harness {
    async fn start() -> Self {
        let db = TestDb::bootstrap().await;
        let sink = SmtpSink::spawn().await;
        let mailer = Arc::new(Mailer::from_smtp(Some(SmtpConfig {
            host: "127.0.0.1".into(),
            port: sink.port,
            from: "noreply@example.com".into(),
        })));
        let app_pool = pool::connect_app(&db.app_url).await.expect("app pool");
        let storage_root =
            std::env::temp_dir().join(format!("fvoci-account-test-{}", Uuid::now_v7()));
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
            storage: storage.clone(),
            upload: fvoci_server::attachments::UploadLimits {
                part_size_bytes: fvoci_server::config::DEFAULT_UPLOAD_PART_SIZE_BYTES,
                max_file_size_bytes: fvoci_server::config::DEFAULT_UPLOAD_MAX_FILE_SIZE_BYTES,
                create_rate_per_5min: fvoci_server::config::DEFAULT_UPLOAD_CREATE_RATE_PER_5MIN,
            },
            collab: None,
            meili: None,
            mailer: mailer.clone(),
        };
        let app = router(state, None);
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
            storage,
            storage_root,
            mailer,
            sink,
            owner_cookie,
            owner_id,
            workspace_id,
        }
    }

    async fn finish(self) {
        self.admin.close().await;
        self.app_pool.close().await;
        let _ = std::fs::remove_dir_all(&self.storage_root);
        self.db.cleanup().await;
    }

    /// A member of `acme` with a password, signed in through the real login route.
    async fn member(&self, label: &str, role: &str) -> (Uuid, String, String) {
        let email = format!("{label}@example.com");
        let user_id = self.insert_user(&email, label, Some(PASSWORD)).await;
        self.add_membership(self.workspace_id, user_id, role).await;
        let cookie = self.login(&email, PASSWORD, peer(2)).await;
        (user_id, email, cookie)
    }

    async fn insert_user(&self, email: &str, given: &str, password: Option<&str>) -> Uuid {
        let user_id = Uuid::now_v7();
        let hash = match password {
            Some(password) => Some(hash_password(password, &keyring()).await.unwrap()),
            None => None,
        };
        sqlx::query(
            "INSERT INTO fvoci.users (id, email, given_name, family_name, password_hash) VALUES ($1, $2, $3, '김', $4)",
        )
        .bind(user_id)
        .bind(email)
        .bind(given)
        .bind(hash)
        .execute(&self.admin)
        .await
        .expect("insert user");
        user_id
    }

    async fn add_membership(&self, workspace_id: Uuid, user_id: Uuid, role: &str) {
        sqlx::query(
            "INSERT INTO fvoci.memberships (workspace_id, user_id, role) VALUES ($1, $2, $3)",
        )
        .bind(workspace_id)
        .bind(user_id)
        .bind(role)
        .execute(&self.admin)
        .await
        .expect("insert membership");
    }

    async fn login(&self, email: &str, password: &str, from: SocketAddr) -> String {
        let res = call(
            &self.app,
            "POST",
            "/api/v1/auth/login",
            Some(json!({"email": email, "password": password})),
            None,
            from,
        )
        .await;
        assert_eq!(res.status, StatusCode::OK, "login {email}: {:?}", res.json);
        res.cookie().expect("session cookie")
    }

    async fn wiki_document(&self) -> Uuid {
        let document_id = Uuid::now_v7();
        sqlx::query(
            r#"
            INSERT INTO fvoci.documents (
                id, workspace_id, title, path, parent_id, sort_key, project_id, number, status,
                schema_version, content_json, created_by
            ) VALUES (
                $1, $2, '문서', $3, NULL, 'V', NULL, 900, 'published', 2,
                '{"type":"doc","content":[{"type":"paragraph"}]}'::jsonb, $4
            )
            "#,
        )
        .bind(document_id)
        .bind(self.workspace_id)
        .bind(document_id.simple().to_string())
        .bind(self.owner_id)
        .execute(&self.admin)
        .await
        .expect("insert wiki document");
        document_id
    }

    async fn me_status(&self, cookie: &str) -> StatusCode {
        call(
            &self.app,
            "GET",
            "/api/v1/auth/me",
            None,
            Some(cookie),
            peer(9),
        )
        .await
        .status
    }

    async fn count(&self, sql: &str, id: Uuid) -> i64 {
        sqlx::query_scalar(sql)
            .bind(id)
            .fetch_one(&self.admin)
            .await
            .unwrap()
    }

    async fn live_sessions(&self, user_id: Uuid) -> i64 {
        self.count(
            "SELECT count(*) FROM fvoci.sessions WHERE user_id = $1 AND revoked_at IS NULL AND expires_at > now()",
            user_id,
        )
        .await
    }

    async fn withdraw(&self, cookie: &str, body: Value) -> Response {
        call(
            &self.app,
            "POST",
            "/api/v1/auth/withdraw",
            Some(body),
            Some(cookie),
            peer(3),
        )
        .await
    }
}

struct Response {
    status: StatusCode,
    json: Value,
    headers: HeaderMap,
    bytes: Vec<u8>,
}

impl Response {
    fn cookie(&self) -> Option<String> {
        self.headers
            .get_all("set-cookie")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .find_map(|v| {
                let value = v.split(';').next()?.strip_prefix("fvoci_session=")?;
                (!value.is_empty()).then(|| value.to_string())
            })
    }

    fn clears_cookie(&self) -> bool {
        self.headers
            .get_all("set-cookie")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .any(|v| v.starts_with("fvoci_session=;") && v.contains("Max-Age=0"))
    }

    fn code(&self) -> &str {
        self.json["code"].as_str().unwrap_or("")
    }
}

async fn call_with(
    app: &axum::Router,
    method: &str,
    path: &str,
    body: Option<Value>,
    cookie: Option<&str>,
    from: SocketAddr,
    extra: &[(&str, &str)],
) -> Response {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("origin", "http://localhost");
    if let Some(cookie) = cookie {
        builder = builder.header("cookie", format!("fvoci_session={cookie}"));
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
        .expect("body")
        .to_vec();
    let json = serde_json::from_slice(&bytes).unwrap_or(json!({}));
    Response {
        status,
        json,
        headers,
        bytes,
    }
}

async fn call(
    app: &axum::Router,
    method: &str,
    path: &str,
    body: Option<Value>,
    cookie: Option<&str>,
    from: SocketAddr,
) -> Response {
    call_with(app, method, path, body, cookie, from, &[]).await
}

async fn wait_for_row_lock_waiters(admin: &PgPool, expected: i64) {
    wait_for_lock_waiters(admin, "%fvoci.users%FOR UPDATE%", expected).await
}

async fn wait_for_lock_waiters(admin: &PgPool, query_like: &str, expected: i64) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let waiting: i64 = sqlx::query_scalar(
            r#"
            SELECT count(*) FROM pg_stat_activity
            WHERE datname = current_database()
              AND wait_event_type = 'Lock'
              AND state = 'active'
              AND query ILIKE $1
            "#,
        )
        .bind(query_like)
        .fetch_one(admin)
        .await
        .unwrap();
        if waiting >= expected {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "expected {expected} lock waiters on {query_like}, saw {waiting}"
        );
        tokio::task::yield_now().await;
    }
}

// ---------------------------------------------------------------------------
// Withdraw and cancel

#[tokio::test]
async fn withdraw_revokes_credentials_blocks_login_and_cancel_restores() {
    let h = Harness::start().await;
    // Admin (not owner): may create workspace API tokens and still withdraw.
    let (user_id, email, cookie) = h.member("member", "admin").await;
    let second_cookie = h.login(&email, PASSWORD, peer(4)).await;
    let ws = h.workspace_id;

    let res = call(
        &h.app,
        "POST",
        "/api/v1/me/api-tokens",
        Some(json!({"workspaceId": ws, "name": "cli", "scopes": ["documents.read"]})),
        Some(&cookie),
        peer(2),
    )
    .await;
    assert_eq!(res.status, StatusCode::CREATED, "{:?}", res.json);
    let secret = res.json["token"].as_str().unwrap().to_string();
    let res = call(
        &h.app,
        "POST",
        &format!("/api/v1/workspaces/{ws}/ics-token"),
        None,
        Some(&cookie),
        peer(2),
    )
    .await;
    assert_eq!(res.status, StatusCode::CREATED, "{:?}", res.json);
    let res = call(
        &h.app,
        "POST",
        "/api/v1/me/personal-workspace",
        None,
        Some(&cookie),
        peer(2),
    )
    .await;
    assert!(res.status.is_success(), "{:?}", res.json);
    sqlx::query(
        r#"
        INSERT INTO fvoci.invitations (id, workspace_id, email, role, token_hash, invited_by, expires_at)
        VALUES ($1, $2, 'pending@example.com', 'member', $3, $4, now() + interval '1 day')
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(ws)
    .bind(hash_token("pending-invite"))
    .bind(user_id)
    .execute(&h.admin)
    .await
    .unwrap();

    // An API token cannot withdraw (session-only route).
    let bearer = format!("Bearer {secret}");
    let res = call_with(
        &h.app,
        "POST",
        "/api/v1/auth/withdraw",
        Some(json!({"currentPassword": PASSWORD, "emailLocalPart": null})),
        None,
        peer(3),
        &[("authorization", &bearer)],
    )
    .await;
    assert_eq!(res.status, StatusCode::NOT_FOUND);
    // Anonymous: 401 before the body is looked at.
    let res = call(
        &h.app,
        "POST",
        "/api/v1/auth/withdraw",
        Some(json!({})),
        None,
        peer(3),
    )
    .await;
    assert_eq!(res.status, StatusCode::UNAUTHORIZED);

    for body in [
        json!({"currentPassword": "wrong-password", "emailLocalPart": null}),
        json!({"currentPassword": null, "emailLocalPart": "member"}),
        json!({"currentPassword": null, "emailLocalPart": null}),
    ] {
        let res = h.withdraw(&cookie, body).await;
        assert_eq!(res.status, StatusCode::BAD_REQUEST, "{:?}", res.json);
        assert_eq!(res.code(), "confirm_invalid");
    }
    assert_eq!(
        h.count(
            "SELECT count(*) FROM fvoci.users WHERE id = $1 AND deleted_at IS NULL",
            user_id
        )
        .await,
        1
    );

    let before = Utc::now();
    let res = h
        .withdraw(
            &cookie,
            json!({"currentPassword": PASSWORD, "emailLocalPart": null}),
        )
        .await;
    assert_eq!(res.status, StatusCode::OK, "{:?}", res.json);
    assert!(res.clears_cookie());
    assert_eq!(res.json["ok"], true);
    assert_eq!(res.json["mailSent"], true);
    let cancel_token = res.json["cancelToken"].as_str().unwrap().to_string();
    let erase_at: DateTime<Utc> = res.json["eraseAt"].as_str().unwrap().parse().unwrap();
    let grace = erase_at - before;
    assert!(grace >= chrono::Duration::days(14) - chrono::Duration::seconds(1));
    assert!(grace <= chrono::Duration::days(14) + chrono::Duration::seconds(30));

    let mail = h
        .sink
        .wait_for(|m| m.to == email && m.text().contains("/cancel-withdraw#token="))
        .await;
    assert!(mail.text().contains("Subject: FVOCI 탈퇴 취소"));
    assert_eq!(
        token_after(&mail.text(), "/cancel-withdraw#token="),
        cancel_token
    );

    // Hash at rest only; the app role cannot read it.
    let stored: String =
        sqlx::query_scalar("SELECT withdraw_cancel_token_hash FROM fvoci.users WHERE id = $1")
            .bind(user_id)
            .fetch_one(&h.admin)
            .await
            .unwrap();
    assert_eq!(stored, hash_token(&cancel_token));
    let denied = sqlx::query("SELECT withdraw_cancel_token_hash FROM fvoci.users LIMIT 1")
        .fetch_all(&h.app_pool)
        .await
        .unwrap_err();
    assert!(denied.to_string().contains("permission denied"), "{denied}");

    // Every credential is gone; login refuses the withdrawn account.
    assert_eq!(h.me_status(&cookie).await, StatusCode::UNAUTHORIZED);
    assert_eq!(h.me_status(&second_cookie).await, StatusCode::UNAUTHORIZED);
    assert_eq!(h.live_sessions(user_id).await, 0);
    let res = call_with(
        &h.app,
        "GET",
        "/api/v1/me/api-tokens",
        None,
        None,
        peer(3),
        &[("authorization", &bearer)],
    )
    .await;
    assert_eq!(res.status, StatusCode::UNAUTHORIZED);
    for table in ["api_tokens", "ics_tokens"] {
        let sql = format!("SELECT count(*) FROM fvoci.{table} WHERE user_id = $1");
        assert_eq!(h.count(&sql, user_id).await, 0, "{table}");
    }
    assert_eq!(
        h.count(
            "SELECT count(*) FROM fvoci.invitations WHERE invited_by = $1",
            user_id
        )
        .await,
        0
    );
    // Source keeps team memberships until anonymization.
    assert_eq!(
        h.count(
            "SELECT count(*) FROM fvoci.memberships WHERE user_id = $1",
            user_id
        )
        .await,
        2
    );
    let res = call(
        &h.app,
        "POST",
        "/api/v1/auth/login",
        Some(json!({"email": email, "password": PASSWORD})),
        None,
        peer(5),
    )
    .await;
    assert_eq!(res.status, StatusCode::UNAUTHORIZED);
    assert_eq!(res.code(), "invalid_email_or_password");
    for verb in ["user.withdrawn"] {
        let events: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM fvoci.events WHERE verb = $1 AND target_id = $2",
        )
        .bind(verb)
        .bind(user_id)
        .fetch_one(&h.admin)
        .await
        .unwrap();
        let audits: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM fvoci.audit_log WHERE verb = $1 AND target_id = $2",
        )
        .bind(verb)
        .bind(user_id)
        .fetch_one(&h.admin)
        .await
        .unwrap();
        assert_eq!((events, audits), (1, 1), "{verb}");
    }

    // Cancel: wrong token 404, right token restores once.
    let res = call(
        &h.app,
        "POST",
        "/api/v1/auth/cancel-withdraw",
        Some(json!({"token": "not-the-token"})),
        None,
        peer(6),
    )
    .await;
    assert_eq!(res.status, StatusCode::NOT_FOUND);
    let res = call(
        &h.app,
        "POST",
        "/api/v1/auth/cancel-withdraw",
        Some(json!({"token": cancel_token})),
        None,
        peer(6),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK, "{:?}", res.json);
    assert_eq!(res.json, json!({"ok": true}));
    let res = call(
        &h.app,
        "POST",
        "/api/v1/auth/cancel-withdraw",
        Some(json!({"token": cancel_token})),
        None,
        peer(6),
    )
    .await;
    assert_eq!(res.status, StatusCode::NOT_FOUND);
    let restored: (Option<DateTime<Utc>>, Option<String>) = sqlx::query_as(
        "SELECT deleted_at, withdraw_cancel_token_hash FROM fvoci.users WHERE id = $1",
    )
    .bind(user_id)
    .fetch_one(&h.admin)
    .await
    .unwrap();
    assert_eq!(restored, (None, None));
    // Revoked sessions stay revoked; the account signs in again.
    assert_eq!(h.me_status(&cookie).await, StatusCode::UNAUTHORIZED);
    let fresh = h.login(&email, PASSWORD, peer(7)).await;
    assert_eq!(h.me_status(&fresh).await, StatusCode::OK);
    assert_eq!(
        h.count(
            "SELECT count(*) FROM fvoci.api_tokens WHERE user_id = $1",
            user_id
        )
        .await,
        0
    );
    assert_eq!(
        h.count(
            "SELECT count(*) FROM fvoci.events WHERE verb = 'user.withdraw_cancelled' AND target_id = $1",
            user_id
        )
        .await,
        1
    );
    h.finish().await;
}

#[tokio::test]
async fn withdraw_refuses_team_owner_and_last_instance_admin() {
    let h = Harness::start().await;
    let res = h
        .withdraw(
            &h.owner_cookie,
            json!({"currentPassword": OWNER_PASSWORD, "emailLocalPart": null}),
        )
        .await;
    assert_eq!(res.status, StatusCode::CONFLICT, "{:?}", res.json);
    assert_eq!(res.code(), "owner_transfer_required");
    assert!(!res.clears_cookie());
    assert_eq!(h.me_status(&h.owner_cookie).await, StatusCode::OK);

    // A second instance admin without team ownership; the owner stops being admin.
    let (admin_id, _, admin_cookie) = h.member("second-admin", "member").await;
    sqlx::query("UPDATE fvoci.users SET is_instance_admin = (id = $1)")
        .bind(admin_id)
        .execute(&h.admin)
        .await
        .unwrap();
    let res = h
        .withdraw(
            &admin_cookie,
            json!({"currentPassword": PASSWORD, "emailLocalPart": null}),
        )
        .await;
    assert_eq!(res.status, StatusCode::CONFLICT, "{:?}", res.json);
    assert_eq!(res.code(), "last_instance_admin");
    assert_eq!(h.me_status(&admin_cookie).await, StatusCode::OK);

    // A personal-workspace owner is not a team owner.
    sqlx::query("UPDATE fvoci.users SET is_instance_admin = true WHERE id = $1")
        .bind(h.owner_id)
        .execute(&h.admin)
        .await
        .unwrap();
    let res = call(
        &h.app,
        "POST",
        "/api/v1/me/personal-workspace",
        None,
        Some(&admin_cookie),
        peer(2),
    )
    .await;
    assert!(res.status.is_success());
    let res = h
        .withdraw(
            &admin_cookie,
            json!({"currentPassword": PASSWORD, "emailLocalPart": null}),
        )
        .await;
    assert_eq!(res.status, StatusCode::OK, "{:?}", res.json);
    h.finish().await;
}

#[tokio::test]
async fn cancel_after_grace_period_is_not_found() {
    let h = Harness::start().await;
    let (user_id, _, cookie) = h.member("late", "member").await;
    let res = h
        .withdraw(
            &cookie,
            json!({"currentPassword": PASSWORD, "emailLocalPart": null}),
        )
        .await;
    assert_eq!(res.status, StatusCode::OK);
    let cancel_token = res.json["cancelToken"].as_str().unwrap().to_string();
    sqlx::query("UPDATE fvoci.users SET deleted_at = now() - interval '14 days' WHERE id = $1")
        .bind(user_id)
        .execute(&h.admin)
        .await
        .unwrap();
    let res = call(
        &h.app,
        "POST",
        "/api/v1/auth/cancel-withdraw",
        Some(json!({"token": cancel_token})),
        None,
        peer(6),
    )
    .await;
    assert_eq!(res.status, StatusCode::NOT_FOUND);
    assert_eq!(
        h.count(
            "SELECT count(*) FROM fvoci.users WHERE id = $1 AND deleted_at IS NOT NULL",
            user_id
        )
        .await,
        1
    );
    h.finish().await;
}

/// Login's password check passes before withdraw commits; the session insert
/// must still observe the withdrawal under the users row lock.
#[tokio::test]
async fn withdraw_racing_login_leaves_no_live_session() {
    let h = Harness::start().await;
    let (user_id, email, cookie) = h.member("racer", "member").await;

    let mut blocker = h.admin.begin().await.unwrap();
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    sqlx::query("SELECT id FROM fvoci.users WHERE id = $1 FOR UPDATE")
        .bind(user_id)
        .execute(&mut *blocker)
        .await
        .unwrap();

    let app = h.app.clone();
    let login_email = email.clone();
    let login = tokio::spawn(async move {
        call(
            &app,
            "POST",
            "/api/v1/auth/login",
            Some(json!({"email": login_email, "password": PASSWORD})),
            None,
            peer(8),
        )
        .await
    });
    wait_for_blocked_by_holder(&h.admin, blocker_pid, Some("%fvoci.users%FOR UPDATE%"), 1).await;
    let app = h.app.clone();
    let withdraw_cookie = cookie.clone();
    let withdraw = tokio::spawn(async move {
        call(
            &app,
            "POST",
            "/api/v1/auth/withdraw",
            Some(json!({"currentPassword": PASSWORD, "emailLocalPart": null})),
            Some(&withdraw_cookie),
            peer(3),
        )
        .await
    });
    // The second waiter queues behind the first on the row's tuple lock, so
    // count waiters on the users row rather than direct victims of the blocker.
    wait_for_row_lock_waiters(&h.admin, 2).await;
    blocker.commit().await.unwrap();

    let login = login.await.unwrap();
    let withdraw = withdraw.await.unwrap();
    assert_eq!(withdraw.status, StatusCode::OK, "{:?}", withdraw.json);
    match login.status {
        StatusCode::OK => {
            let late = login.cookie().expect("cookie");
            assert_eq!(h.me_status(&late).await, StatusCode::UNAUTHORIZED);
        }
        StatusCode::UNAUTHORIZED => {}
        other => panic!("unexpected login status {other}"),
    }
    assert_eq!(h.live_sessions(user_id).await, 0);

    // Unsynchronized burst: whatever interleaving, nothing stays live.
    let res = call(
        &h.app,
        "POST",
        "/api/v1/auth/cancel-withdraw",
        Some(json!({"token": withdraw.json["cancelToken"]})),
        None,
        peer(6),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK);
    let cookie = h.login(&email, PASSWORD, peer(10)).await;
    let mut tasks = Vec::new();
    for n in 0..6u8 {
        let app = h.app.clone();
        let email = email.clone();
        tasks.push(tokio::spawn(async move {
            call(
                &app,
                "POST",
                "/api/v1/auth/login",
                Some(json!({"email": email, "password": PASSWORD})),
                None,
                peer(20 + n),
            )
            .await
            .status
        }));
    }
    let res = h
        .withdraw(
            &cookie,
            json!({"currentPassword": PASSWORD, "emailLocalPart": null}),
        )
        .await;
    assert_eq!(res.status, StatusCode::OK);
    for task in tasks {
        let status = task.await.unwrap();
        assert!(
            status == StatusCode::OK || status == StatusCode::UNAUTHORIZED,
            "{status}"
        );
    }
    assert_eq!(h.live_sessions(user_id).await, 0);
    h.finish().await;
}

// ---------------------------------------------------------------------------
// Final erasure job

#[tokio::test]
async fn maintenance_sweep_anonymizes_withdrawn_users_after_grace_period() {
    let h = Harness::start().await;
    let (due_id, due_email, due_cookie) = h.member("due", "member").await;
    let (recent_id, _, recent_cookie) = h.member("recent", "member").await;
    let res = call(
        &h.app,
        "POST",
        "/api/v1/me/personal-workspace",
        None,
        Some(&due_cookie),
        peer(2),
    )
    .await;
    assert!(res.status.is_success(), "{:?}", res.json);
    let personal_id: Uuid =
        sqlx::query_scalar("SELECT personal_workspace_id FROM fvoci.users WHERE id = $1")
            .bind(due_id)
            .fetch_one(&h.admin)
            .await
            .unwrap();

    // Data the erasure must scrub.
    let document_id = h.wiki_document().await;
    let attachment_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO fvoci.attachments (
            id, workspace_id, document_id, uploader_id, status, name, reserved_size_bytes,
            size_bytes, storage_key, completed_at
        ) VALUES ($1, $2, $3, $4, 'stored', '개인 이력서.pdf', 4, 4, $5, now())
        "#,
    )
    .bind(attachment_id)
    .bind(h.workspace_id)
    .bind(document_id)
    .bind(due_id)
    .bind(Uuid::now_v7().to_string())
    .execute(&h.admin)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO fvoci.notifications (workspace_id, user_id, event_id, verb) VALUES ($1, $2, $3, 'comment.created')",
    )
    .bind(h.workspace_id)
    .bind(due_id)
    .bind(Uuid::now_v7())
    .execute(&h.admin)
    .await
    .unwrap();

    let res = h
        .withdraw(
            &due_cookie,
            json!({"currentPassword": PASSWORD, "emailLocalPart": null}),
        )
        .await;
    assert_eq!(res.status, StatusCode::OK);
    let due_cancel = res.json["cancelToken"].as_str().unwrap().to_string();
    let res = h
        .withdraw(
            &recent_cookie,
            json!({"currentPassword": PASSWORD, "emailLocalPart": null}),
        )
        .await;
    assert_eq!(res.status, StatusCode::OK);
    // One second before the deadline must not be erased; past it must be.
    sqlx::query(
        "UPDATE fvoci.users SET deleted_at = now() - interval '14 days' - interval '1 minute' WHERE id = $1",
    )
    .bind(due_id)
    .execute(&h.admin)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE fvoci.users SET deleted_at = now() - interval '14 days' + interval '1 minute' WHERE id = $1",
    )
    .bind(recent_id)
    .execute(&h.admin)
    .await
    .unwrap();

    let cancel = CancellationToken::new();
    let stats = run_daily_sweep(&h.app_pool, &h.storage, &h.mailer, &cancel)
        .await
        .unwrap()
        .expect("claimed daily sweep");
    assert_eq!(stats.withdrawn_anonymized, 1);
    assert!(stats.workspace.purged >= 1, "{stats:?}");

    type Erased = (
        String,
        Option<String>,
        String,
        Option<DateTime<Utc>>,
        Option<String>,
        Option<String>,
    );
    let erased: Erased = sqlx::query_as(
        r#"
        SELECT given_name, family_name, email, anonymized_at, password_hash, withdraw_cancel_token_hash
        FROM fvoci.users WHERE id = $1
        "#,
    )
    .bind(due_id)
    .fetch_one(&h.admin)
    .await
    .unwrap();
    assert_eq!(erased.0, "탈퇴한 사용자");
    assert_eq!(erased.1, None);
    assert!(
        regex::Regex::new(r"^withdrawn-[0-9a-f]{12}@withdrawn\.invalid$")
            .unwrap()
            .is_match(&erased.2),
        "{}",
        erased.2
    );
    assert_ne!(erased.2, due_email);
    assert!(erased.3.is_some());
    assert_eq!((erased.4, erased.5), (None, None));
    assert_eq!(
        h.count(
            "SELECT count(*) FROM fvoci.sessions WHERE user_id = $1",
            due_id
        )
        .await,
        0
    );
    assert_eq!(
        h.count(
            "SELECT count(*) FROM fvoci.notifications WHERE user_id = $1",
            due_id
        )
        .await,
        0
    );
    let name: String = sqlx::query_scalar("SELECT name FROM fvoci.attachments WHERE id = $1")
        .bind(attachment_id)
        .fetch_one(&h.admin)
        .await
        .unwrap();
    assert_eq!(name, "deleted");
    // Personal workspace marked deleted and purged by the same sweep.
    assert_eq!(
        h.count(
            "SELECT count(*) FROM fvoci.workspaces WHERE id = $1",
            personal_id
        )
        .await,
        0
    );
    for (verb, channel) in [("user.anonymized", "system")] {
        let row: (Option<Uuid>, String) = sqlx::query_as(
            "SELECT actor_user_id, channel FROM fvoci.events WHERE verb = $1 AND target_id = $2",
        )
        .bind(verb)
        .bind(due_id)
        .fetch_one(&h.admin)
        .await
        .unwrap();
        assert_eq!(row, (None, channel.to_string()));
    }
    assert_eq!(
        h.count(
            "SELECT count(*) FROM fvoci.events WHERE verb = 'workspace.deleted' AND workspace_id = $1",
            personal_id
        )
        .await,
        1
    );
    // Not yet due: untouched and still cancellable.
    assert_eq!(
        h.count(
            "SELECT count(*) FROM fvoci.users WHERE id = $1 AND anonymized_at IS NULL AND deleted_at IS NOT NULL",
            recent_id
        )
        .await,
        1
    );
    // Idempotent; the erased user's cancel token is dead.
    assert_eq!(
        run_withdrawn_anonymize(&h.app_pool, Utc::now(), &cancel)
            .await
            .unwrap(),
        0
    );
    let res = call(
        &h.app,
        "POST",
        "/api/v1/auth/cancel-withdraw",
        Some(json!({"token": due_cancel})),
        None,
        peer(6),
    )
    .await;
    assert_eq!(res.status, StatusCode::NOT_FOUND);
    // A cancelled sweep does nothing.
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    sqlx::query("UPDATE fvoci.users SET deleted_at = now() - interval '15 days' WHERE id = $1")
        .bind(recent_id)
        .execute(&h.admin)
        .await
        .unwrap();
    assert_eq!(
        run_withdrawn_anonymize(&h.app_pool, Utc::now(), &cancelled)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        run_withdrawn_anonymize(&h.app_pool, Utc::now(), &cancel)
            .await
            .unwrap(),
        1
    );
    h.finish().await;
}

/// A cancel that takes the users row first wins; the sweep's recheck under the
/// same lock then finds nothing to erase.
#[tokio::test]
async fn anonymize_rechecks_after_concurrent_cancel() {
    let h = Harness::start().await;
    let (user_id, _, cookie) = h.member("flip", "member").await;
    let res = h
        .withdraw(
            &cookie,
            json!({"currentPassword": PASSWORD, "emailLocalPart": null}),
        )
        .await;
    let token = res.json["cancelToken"].as_str().unwrap().to_string();
    // Inside the grace period by one minute for cancel; the job is told a
    // later "now" so the candidate snapshot includes the user.
    sqlx::query(
        "UPDATE fvoci.users SET deleted_at = now() - interval '14 days' + interval '1 minute' WHERE id = $1",
    )
    .bind(user_id)
    .execute(&h.admin)
    .await
    .unwrap();
    let mut blocker = h.admin.begin().await.unwrap();
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    sqlx::query("SELECT id FROM fvoci.users WHERE id = $1 FOR UPDATE")
        .bind(user_id)
        .execute(&mut *blocker)
        .await
        .unwrap();
    let app = h.app.clone();
    let cancel_task = tokio::spawn(async move {
        call(
            &app,
            "POST",
            "/api/v1/auth/cancel-withdraw",
            Some(json!({"token": token})),
            None,
            peer(6),
        )
        .await
        .status
    });
    wait_for_blocked_by_holder(&h.admin, blocker_pid, Some("%fvoci.users%FOR UPDATE%"), 1).await;
    let pool = h.app_pool.clone();
    let sweep = tokio::spawn(async move {
        run_withdrawn_anonymize(
            &pool,
            Utc::now() + chrono::Duration::minutes(2),
            &CancellationToken::new(),
        )
        .await
        .unwrap()
    });
    // The sweep queues behind the cancel on the account advisory locks.
    wait_for_lock_waiters(&h.admin, "%pg_advisory_xact_lock%", 1).await;
    blocker.commit().await.unwrap();
    assert_eq!(cancel_task.await.unwrap(), StatusCode::OK);
    assert_eq!(sweep.await.unwrap(), 0);
    assert_eq!(
        h.count(
            "SELECT count(*) FROM fvoci.users WHERE id = $1 AND deleted_at IS NULL AND anonymized_at IS NULL",
            user_id
        )
        .await,
        1
    );
    h.finish().await;
}

// ---------------------------------------------------------------------------
// Password change

#[tokio::test]
async fn password_change_keeps_current_session_and_revokes_others() {
    let h = Harness::start().await;
    let (user_id, email, cookie) = h.member("pw", "member").await;
    let other = h.login(&email, PASSWORD, peer(4)).await;
    let path = "/api/v1/auth/password";

    let res = call(
        &h.app,
        "PATCH",
        path,
        Some(json!({"currentPassword": "wrong-password", "newPassword": "brandnew-secret"})),
        Some(&cookie),
        peer(2),
    )
    .await;
    assert_eq!(res.status, StatusCode::BAD_REQUEST);
    assert_eq!(res.code(), "password_invalid");
    let res = call(
        &h.app,
        "PATCH",
        path,
        Some(json!({"currentPassword": null, "newPassword": "brandnew-secret"})),
        Some(&cookie),
        peer(2),
    )
    .await;
    assert_eq!(res.code(), "password_invalid");
    let res = call(
        &h.app,
        "PATCH",
        path,
        Some(json!({"currentPassword": PASSWORD, "newPassword": "short"})),
        Some(&cookie),
        peer(2),
    )
    .await;
    assert_eq!(res.code(), "password_invalid");
    let res = call(
        &h.app,
        "PATCH",
        path,
        Some(json!({"currentPassword": PASSWORD, "newPassword": "brandnew-secret"})),
        None,
        peer(2),
    )
    .await;
    assert_eq!(res.status, StatusCode::UNAUTHORIZED);

    let generation: i32 =
        sqlx::query_scalar("SELECT auth_generation FROM fvoci.users WHERE id = $1")
            .bind(user_id)
            .fetch_one(&h.admin)
            .await
            .unwrap();
    // Two concurrent changes from one session with the same current password:
    // exactly one commits (the other sees a replaced hash under the row lock).
    let mut tasks = Vec::new();
    for new_password in ["brandnew-secret", "otherpass-secret"] {
        let app = h.app.clone();
        let cookie = cookie.clone();
        tasks.push(tokio::spawn(async move {
            let res = call(
                &app,
                "PATCH",
                "/api/v1/auth/password",
                Some(json!({"currentPassword": PASSWORD, "newPassword": new_password})),
                Some(&cookie),
                peer(2),
            )
            .await;
            (new_password, res.status, res.json)
        }));
    }
    let mut winners = Vec::new();
    for task in tasks {
        let (password, status, body) = task.await.unwrap();
        match status {
            StatusCode::OK => winners.push(password),
            StatusCode::BAD_REQUEST => assert_eq!(body["code"], "password_invalid"),
            other => panic!("unexpected {other}"),
        }
    }
    assert_eq!(winners.len(), 1, "{winners:?}");
    let winner = winners[0];

    assert_eq!(h.me_status(&cookie).await, StatusCode::OK);
    assert_eq!(h.me_status(&other).await, StatusCode::UNAUTHORIZED);
    let res = call(
        &h.app,
        "POST",
        "/api/v1/auth/login",
        Some(json!({"email": email, "password": PASSWORD})),
        None,
        peer(5),
    )
    .await;
    assert_eq!(res.status, StatusCode::UNAUTHORIZED);
    h.login(&email, winner, peer(5)).await;
    let bumped: i32 = sqlx::query_scalar("SELECT auth_generation FROM fvoci.users WHERE id = $1")
        .bind(user_id)
        .fetch_one(&h.admin)
        .await
        .unwrap();
    assert_eq!(bumped, generation + 1);
    assert_eq!(
        h.count(
            "SELECT count(*) FROM fvoci.audit_log WHERE verb = 'auth.password_changed' AND target_id = $1",
            user_id
        )
        .await,
        1
    );
    h.finish().await;
}

// ---------------------------------------------------------------------------
// Email change

async fn request_email_change(h: &Harness, cookie: &str, new_email: &str, from: u8) -> Duration {
    let started = Instant::now();
    let res = call(
        &h.app,
        "PATCH",
        "/api/v1/auth/email",
        Some(json!({"newEmail": new_email})),
        Some(cookie),
        peer(from),
    )
    .await;
    assert_eq!(res.status, StatusCode::ACCEPTED, "{:?}", res.json);
    assert_eq!(res.json, json!({"ok": true}));
    started.elapsed()
}

#[tokio::test]
async fn email_change_is_single_use_expires_and_hides_taken_addresses() {
    let h = Harness::start().await;
    let (user_id, email, cookie) = h.member("mail", "member").await;

    // Taken address (the owner's): same 202, no token, no mail.
    let taken = request_email_change(&h, &cookie, "OWNER@example.com", 30).await;
    assert!(taken >= Duration::from_millis(100));
    assert_eq!(
        h.count(
            "SELECT count(*) FROM fvoci.magic_tokens WHERE user_id = $1",
            user_id
        )
        .await,
        0
    );

    let elapsed = request_email_change(&h, &cookie, "New.Address@Example.com", 31).await;
    assert!(elapsed >= Duration::from_millis(100));
    let confirm = h
        .sink
        .wait_for(|m| {
            m.to == "new.address@example.com" && m.text().contains("/confirm-email?token=")
        })
        .await;
    assert!(confirm.text().contains("Subject: FVOCI 이메일 변경 확인"));
    let notice = h
        .sink
        .wait_for(|m| m.to == email && m.text().contains("변경이 요청되었습니다"))
        .await;
    assert!(notice
        .text()
        .contains("Subject: FVOCI 이메일 변경 요청 알림"));
    let token = token_after(&confirm.text(), "/confirm-email?token=");
    let stored: (String, String) = sqlx::query_as(
        "SELECT token_hash, new_email FROM fvoci.magic_tokens WHERE user_id = $1 AND kind = 'email_change'",
    )
    .bind(user_id)
    .fetch_one(&h.admin)
    .await
    .unwrap();
    assert_eq!(
        stored,
        (hash_token(&token), "new.address@example.com".into())
    );

    // Double confirm: exactly one succeeds.
    let mut tasks = Vec::new();
    for n in 0..2u8 {
        let app = h.app.clone();
        let token = token.clone();
        tasks.push(tokio::spawn(async move {
            call(
                &app,
                "POST",
                "/api/v1/auth/email/confirm",
                Some(json!({"token": token})),
                None,
                peer(40 + n),
            )
            .await
        }));
    }
    let mut ok = 0;
    for task in tasks {
        let res = task.await.unwrap();
        match res.status {
            StatusCode::OK => ok += 1,
            StatusCode::BAD_REQUEST => assert_eq!(res.code(), "magic_invalid"),
            other => panic!("unexpected {other}"),
        }
    }
    assert_eq!(ok, 1);
    let me = call(
        &h.app,
        "GET",
        "/api/v1/auth/me",
        None,
        Some(&cookie),
        peer(9),
    )
    .await;
    assert_eq!(me.json["email"], "new.address@example.com");
    assert!(me.json["emailVerifiedAt"].is_string());
    assert_eq!(
        h.count(
            "SELECT count(*) FROM fvoci.events WHERE verb = 'user.email_changed' AND target_id = $1",
            user_id
        )
        .await,
        1
    );
    h.sink
        .wait_for(|m| m.to == email && m.text().contains("Subject: FVOCI 이메일 변경 완료 알림"))
        .await;
    h.login("new.address@example.com", PASSWORD, peer(11)).await;

    // Expired token.
    request_email_change(&h, &cookie, "expired@example.com", 32).await;
    let expired = h.sink.wait_for(|m| m.to == "expired@example.com").await;
    sqlx::query("UPDATE fvoci.magic_tokens SET expires_at = now() - interval '1 second' WHERE new_email = 'expired@example.com'")
        .execute(&h.admin)
        .await
        .unwrap();
    let res = call(
        &h.app,
        "POST",
        "/api/v1/auth/email/confirm",
        Some(json!({"token": token_after(&expired.text(), "/confirm-email?token=")})),
        None,
        peer(42),
    )
    .await;
    assert_eq!(res.code(), "magic_invalid");

    // A password change (credential generation bump) invalidates a pending change.
    request_email_change(&h, &cookie, "stale@example.com", 33).await;
    let stale = h.sink.wait_for(|m| m.to == "stale@example.com").await;
    let res = call(
        &h.app,
        "PATCH",
        "/api/v1/auth/password",
        Some(json!({"currentPassword": PASSWORD, "newPassword": "rotated-secret1"})),
        Some(&cookie),
        peer(2),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK);
    let res = call(
        &h.app,
        "POST",
        "/api/v1/auth/email/confirm",
        Some(json!({"token": token_after(&stale.text(), "/confirm-email?token=")})),
        None,
        peer(43),
    )
    .await;
    assert_eq!(res.code(), "magic_invalid");
    let current: String = sqlx::query_scalar("SELECT email FROM fvoci.users WHERE id = $1")
        .bind(user_id)
        .fetch_one(&h.admin)
        .await
        .unwrap();
    assert_eq!(current, "new.address@example.com");
    // Anonymous request: 401.
    let res = call(
        &h.app,
        "PATCH",
        "/api/v1/auth/email",
        Some(json!({"newEmail": "x@example.com"})),
        None,
        peer(34),
    )
    .await;
    assert_eq!(res.status, StatusCode::UNAUTHORIZED);
    h.finish().await;
}

// ---------------------------------------------------------------------------
// Magic link

#[tokio::test]
async fn magic_link_login_is_single_use_without_enumeration() {
    let h = Harness::start().await;
    let passwordless = h.insert_user("nopass@example.com", "무암호", None).await;
    h.add_membership(h.workspace_id, passwordless, "member")
        .await;

    let started = Instant::now();
    let res = call(
        &h.app,
        "POST",
        "/api/v1/auth/magic-link",
        Some(json!({"email": "ghost@example.com"})),
        None,
        peer(50),
    )
    .await;
    assert_eq!(res.status, StatusCode::ACCEPTED);
    assert!(started.elapsed() >= Duration::from_millis(100));
    let started = Instant::now();
    let res = call(
        &h.app,
        "POST",
        "/api/v1/auth/magic-link",
        Some(json!({"email": "NoPass@Example.com"})),
        None,
        peer(51),
    )
    .await;
    assert_eq!(res.status, StatusCode::ACCEPTED);
    assert_eq!(res.json, json!({"ok": true}));
    assert!(started.elapsed() >= Duration::from_millis(100));
    assert!(res.cookie().is_none());
    let mail = h
        .sink
        .wait_for(|m| m.to == "nopass@example.com" && m.text().contains("/magic-link?token="))
        .await;
    assert!(mail.text().contains("Subject: FVOCI 로그인 링크"));
    let token = token_after(&mail.text(), "/magic-link?token=");
    assert_eq!(
        h.count(
            "SELECT count(*) FROM fvoci.magic_tokens WHERE kind = 'login' AND user_id = $1",
            passwordless
        )
        .await,
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM fvoci.magic_tokens WHERE kind = 'login'"
        )
        .fetch_one(&h.admin)
        .await
        .unwrap(),
        1
    );

    // A login token is not an email-change token (and is consumed by the attempt).
    let res = call(
        &h.app,
        "POST",
        "/api/v1/auth/email/confirm",
        Some(json!({"token": token})),
        None,
        peer(52),
    )
    .await;
    assert_eq!(res.code(), "magic_invalid");
    let res = call(
        &h.app,
        "POST",
        "/api/v1/auth/magic-link/consume",
        Some(json!({"token": token})),
        None,
        peer(52),
    )
    .await;
    assert_eq!(res.code(), "magic_invalid");

    let res = call(
        &h.app,
        "POST",
        "/api/v1/auth/magic-link",
        Some(json!({"email": "nopass@example.com"})),
        None,
        peer(51),
    )
    .await;
    assert_eq!(res.status, StatusCode::ACCEPTED);
    let deadline = Instant::now() + Duration::from_secs(5);
    let token = loop {
        let mails: Vec<CapturedMail> = h
            .sink
            .snapshot()
            .into_iter()
            .filter(|m| m.to == "nopass@example.com")
            .collect();
        if mails.len() >= 2 {
            break token_after(&mails[1].text(), "/magic-link?token=");
        }
        assert!(Instant::now() < deadline, "second magic mail");
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    let res = call(
        &h.app,
        "POST",
        "/api/v1/auth/magic-link/consume",
        Some(json!({"token": token})),
        None,
        peer(53),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK, "{:?}", res.json);
    assert_eq!(res.json["userId"], passwordless.to_string());
    let cookie = res.cookie().expect("magic session");
    let me = call(
        &h.app,
        "GET",
        "/api/v1/auth/me",
        None,
        Some(&cookie),
        peer(9),
    )
    .await;
    assert_eq!(me.json["hasPassword"], false);
    assert!(me.json["emailVerifiedAt"].is_string());
    let res = call(
        &h.app,
        "POST",
        "/api/v1/auth/magic-link/consume",
        Some(json!({"token": token})),
        None,
        peer(53),
    )
    .await;
    assert_eq!(res.code(), "magic_invalid");
    let method: String = sqlx::query_scalar(
        "SELECT payload->>'method' FROM fvoci.events WHERE verb = 'auth.login' AND actor_user_id = $1",
    )
    .bind(passwordless)
    .fetch_one(&h.admin)
    .await
    .unwrap();
    assert_eq!(method, "magic");

    // Password-less withdraw confirms with the email local part (case-folded).
    let res = h
        .withdraw(
            &cookie,
            json!({"currentPassword": "anything", "emailLocalPart": null}),
        )
        .await;
    assert_eq!(res.code(), "confirm_invalid");
    let res = h
        .withdraw(
            &cookie,
            json!({"currentPassword": null, "emailLocalPart": "nopass@example.com"}),
        )
        .await;
    assert_eq!(res.code(), "confirm_invalid");
    let res = h
        .withdraw(
            &cookie,
            json!({"currentPassword": null, "emailLocalPart": "NOPASS"}),
        )
        .await;
    assert_eq!(res.status, StatusCode::OK, "{:?}", res.json);

    // A withdrawn account gets no link, and a link issued before withdrawal is dead.
    let res = call(
        &h.app,
        "POST",
        "/api/v1/auth/magic-link",
        Some(json!({"email": "nopass@example.com"})),
        None,
        peer(54),
    )
    .await;
    assert_eq!(res.status, StatusCode::ACCEPTED);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM fvoci.magic_tokens WHERE kind = 'login'"
        )
        .fetch_one(&h.admin)
        .await
        .unwrap(),
        0
    );

    // Per ip:email limit (10 per window).
    let mut last = StatusCode::OK;
    for _ in 0..11 {
        last = call(
            &h.app,
            "POST",
            "/api/v1/auth/magic-link",
            Some(json!({"email": "ghost@example.com"})),
            None,
            peer(55),
        )
        .await
        .status;
    }
    assert_eq!(last, StatusCode::TOO_MANY_REQUESTS);
    h.finish().await;
}

#[tokio::test]
async fn providers_and_identities_report_password_only() {
    let h = Harness::start().await;
    let res = call(
        &h.app,
        "GET",
        "/api/v1/auth/providers",
        None,
        None,
        peer(60),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK);
    assert_eq!(
        res.json,
        json!({"providers": [], "magicLink": true, "workspaceSso": false})
    );
    let res = call(
        &h.app,
        "GET",
        "/api/v1/auth/identities",
        None,
        None,
        peer(60),
    )
    .await;
    assert_eq!(res.status, StatusCode::UNAUTHORIZED);
    let res = call(
        &h.app,
        "GET",
        "/api/v1/auth/identities",
        None,
        Some(&h.owner_cookie),
        peer(60),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK);
    assert_eq!(res.json, json!({"items": []}));
    h.finish().await;
}

// ---------------------------------------------------------------------------
// Export

struct ZipEntry {
    name: String,
    data: Vec<u8>,
}

/// Central-directory reader for the stored archives the export writes.
fn read_zip(bytes: &[u8]) -> Vec<ZipEntry> {
    let u16_at = |at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]) as usize;
    let u32_at = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
    let eocd = bytes.len() - 22;
    assert_eq!(u32_at(eocd), 0x0605_4b50, "eocd");
    let count = u16_at(eocd + 10);
    let mut at = u32_at(eocd + 16);
    let mut entries = Vec::new();
    for _ in 0..count {
        assert_eq!(u32_at(at), 0x0201_4b50, "central header");
        let crc = u32_at(at + 16) as u32;
        let size = u32_at(at + 24);
        let name_len = u16_at(at + 28);
        let local = u32_at(at + 42);
        let name = String::from_utf8(bytes[at + 46..at + 46 + name_len].to_vec()).unwrap();
        assert_eq!(u32_at(local), 0x0403_4b50, "local header");
        let data_at = local + 30 + u16_at(local + 26) + u16_at(local + 28);
        let data = bytes[data_at..data_at + size].to_vec();
        assert_eq!(fvoci_server::export_zip::crc32(&data), crc, "crc {name}");
        // Data descriptor follows the payload.
        assert_eq!(u32_at(data_at + size), 0x0807_4b50, "descriptor {name}");
        assert_eq!(u32_at(data_at + size + 4) as u32, crc);
        entries.push(ZipEntry { name, data });
        at += 46 + name_len + u16_at(at + 30) + u16_at(at + 32);
    }
    entries
}

#[tokio::test]
async fn export_streams_profile_comments_and_attachments() {
    let h = Harness::start().await;
    let (user_id, email, cookie) = h.member("exporter", "member").await;
    let other_ws = Uuid::now_v7();
    sqlx::query("INSERT INTO fvoci.workspaces (id, slug, name) VALUES ($1, 'other', 'Other')")
        .bind(other_ws)
        .execute(&h.admin)
        .await
        .unwrap();
    let document_id = h.wiki_document().await;
    let mut comment_ids = Vec::new();
    for (n, body) in ["첫 댓글 😀", "두 번째 \"인용\"\n줄바꿈"]
        .iter()
        .enumerate()
    {
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO fvoci.comments (id, workspace_id, document_id, created_by, body, created_at) VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(id)
        .bind(h.workspace_id)
        .bind(document_id)
        .bind(user_id)
        .bind(body)
        .bind(
            DateTime::parse_from_rfc3339(&format!("2026-09-2{n}T01:02:03.456789Z"))
                .unwrap()
                .with_timezone(&Utc),
        )
        .execute(&h.admin)
        .await
        .unwrap();
        comment_ids.push(id);
    }
    // Someone else's comment is not exported.
    sqlx::query(
        "INSERT INTO fvoci.comments (id, workspace_id, document_id, created_by, body) VALUES ($1, $2, $3, $4, 'owner')",
    )
    .bind(Uuid::now_v7())
    .bind(h.workspace_id)
    .bind(document_id)
    .bind(h.owner_id)
    .execute(&h.admin)
    .await
    .unwrap();

    async fn attachment(
        h: &Harness,
        uploader: Uuid,
        name: &str,
        scan: &str,
        bytes: Option<&[u8]>,
        document_id: Uuid,
    ) -> Uuid {
        let id = Uuid::now_v7();
        let key = Uuid::now_v7().to_string();
        if let Some(bytes) = bytes {
            let dir = h.storage_root.join("objects").join(&key);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("payload"), bytes).unwrap();
        }
        sqlx::query(
            r#"
            INSERT INTO fvoci.attachments (
                id, workspace_id, document_id, uploader_id, status, name, mime, reserved_size_bytes,
                size_bytes, storage_key, scan_status, completed_at
            ) VALUES ($1, $2, $3, $4, 'stored', $5, 'application/pdf', $6, $6, $7, $8, now())
            "#,
        )
        .bind(id)
        .bind(h.workspace_id)
        .bind(document_id)
        .bind(uploader)
        .bind(name)
        .bind(bytes.map(|b| b.len() as i64).unwrap_or(9))
        .bind(key)
        .bind(scan)
        .execute(&h.admin)
        .await
        .unwrap();
        id
    }
    let payload = vec![7u8; 200_000];
    let kept = attachment(
        &h,
        user_id,
        "../보고서.pdf",
        "clean",
        Some(&payload),
        document_id,
    )
    .await;
    let infected = attachment(
        &h,
        user_id,
        "bad.pdf",
        "infected",
        Some(b"virus"),
        document_id,
    )
    .await;
    let missing = attachment(&h, user_id, "gone.pdf", "clean", None, document_id).await;
    attachment(
        &h,
        h.owner_id,
        "owner.pdf",
        "clean",
        Some(b"x"),
        document_id,
    )
    .await;

    let res = call(
        &h.app,
        "GET",
        "/api/v1/me/export",
        None,
        Some(&cookie),
        peer(70),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK);
    assert_eq!(res.headers["content-type"], "application/zip");
    assert_eq!(
        res.headers["content-disposition"],
        "attachment; filename=\"fvoci-export.zip\""
    );
    let entries = read_zip(&res.bytes);
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "profile.json",
            "comments.json",
            "attachments.json",
            &format!("attachments/{kept}-보고서.pdf"),
        ]
    );
    let profile = String::from_utf8(entries[0].data.clone()).unwrap();
    assert_eq!(
        profile,
        format!(
            "{{\n  \"id\": \"{user_id}\",\n  \"email\": \"{email}\",\n  \"givenName\": \"exporter\",\n  \"familyName\": \"김\",\n  \"locale\": \"ko\",\n  \"timezone\": \"Asia/Seoul\",\n  \"weekStartsOn\": 1\n}}\n"
        )
    );
    let comments_text = String::from_utf8(entries[1].data.clone()).unwrap();
    assert!(
        comments_text.starts_with("[\n  {\n    \"id\": "),
        "{comments_text}"
    );
    assert!(comments_text.ends_with("\n  }\n]\n"));
    let comments: Value = serde_json::from_str(&comments_text).unwrap();
    assert_eq!(comments.as_array().unwrap().len(), 2);
    assert_eq!(comments[0]["id"], comment_ids[0].to_string());
    assert_eq!(comments[0]["body"], "첫 댓글 😀");
    assert_eq!(comments[0]["createdAt"], "2026-09-20T01:02:03.456Z");
    assert_eq!(comments[1]["taskId"], Value::Null);
    let attachments: Value = serde_json::from_slice(&entries[2].data).unwrap();
    let listed: Vec<String> = attachments
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        listed,
        vec![kept.to_string(), infected.to_string(), missing.to_string()]
    );
    assert_eq!(attachments[0]["sizeBytes"], 200_000);
    assert_eq!(attachments[1]["scanStatus"], "infected");
    assert_eq!(entries[3].data, payload);

    // Anonymous, API token and rate limit (5 per 15 minutes).
    let res = call(&h.app, "GET", "/api/v1/me/export", None, None, peer(70)).await;
    assert_eq!(res.status, StatusCode::UNAUTHORIZED);
    let mut last = StatusCode::OK;
    for _ in 0..5 {
        last = call(
            &h.app,
            "GET",
            "/api/v1/me/export",
            None,
            Some(&cookie),
            peer(70),
        )
        .await
        .status;
    }
    assert_eq!(last, StatusCode::TOO_MANY_REQUESTS);
    // Empty history: "[]\n" arrays.
    let (_, _, fresh) = h.member("empty", "member").await;
    let res = call(
        &h.app,
        "GET",
        "/api/v1/me/export",
        None,
        Some(&fresh),
        peer(71),
    )
    .await;
    let entries = read_zip(&res.bytes);
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[1].data, b"[]\n");
    assert_eq!(entries[2].data, b"[]\n");
    h.finish().await;
}

// ---------------------------------------------------------------------------
// Dashboard and locate

#[tokio::test]
async fn dashboard_and_locate_follow_current_visibility() {
    let h = Harness::start().await;
    let ws = h.workspace_id;
    let (member_id, _, member_cookie) = h.member("dash", "member").await;
    let public =
        project_harness::create_project(h.app.clone(), &h.owner_cookie, ws, "PUB", "workspace")
            .await;
    let private =
        project_harness::create_project(h.app.clone(), &h.owner_cookie, ws, "PRV", "private").await;
    let public_id = public["id"].as_str().unwrap().to_string();
    let private_id = private["id"].as_str().unwrap().to_string();

    let mut tasks = Vec::new();
    for (project, title, due) in [
        (&public_id, "늦은 일", Some("2026-10-02")),
        (&public_id, "급한 일", Some("2026-10-01")),
        (&public_id, "기한 없음", None),
        (&private_id, "비공개 일", Some("2026-09-30")),
    ] {
        let mut body = json!({"title": title});
        if let Some(due) = due {
            body["dueDate"] = json!(due);
        }
        let res = call(
            &h.app,
            "POST",
            &format!("/api/v1/workspaces/{ws}/projects/{project}/tasks"),
            Some(body),
            Some(&h.owner_cookie),
            peer(1),
        )
        .await;
        assert_eq!(res.status, StatusCode::CREATED, "{:?}", res.json);
        let task_id = res.json["id"].as_str().unwrap().to_string();
        let res = call(
            &h.app,
            "PATCH",
            &format!("/api/v1/workspaces/{ws}/tasks/{task_id}"),
            Some(json!({"assigneeIds": [member_id, h.owner_id]})),
            Some(&h.owner_cookie),
            peer(1),
        )
        .await;
        // The private project rejects a non-member assignee; assign only the owner there.
        if res.status != StatusCode::OK {
            let res = call(
                &h.app,
                "PATCH",
                &format!("/api/v1/workspaces/{ws}/tasks/{task_id}"),
                Some(json!({"assigneeIds": [h.owner_id]})),
                Some(&h.owner_cookie),
                peer(1),
            )
            .await;
            assert_eq!(res.status, StatusCode::OK, "{:?}", res.json);
        }
        tasks.push(task_id);
    }

    let res = call(
        &h.app,
        "GET",
        "/api/v1/me/dashboard",
        None,
        Some(&member_cookie),
        peer(80),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK, "{:?}", res.json);
    let assigned: Vec<&str> = res.json["assigned"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["title"].as_str().unwrap())
        .collect();
    assert_eq!(assigned, vec!["급한 일", "늦은 일", "기한 없음"]);
    let projects: Vec<&str> = res.json["projects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["key"].as_str().unwrap())
        .collect();
    assert_eq!(projects, vec!["PUB"]);
    assert!(res.json["statuses"]
        .as_array()
        .unwrap()
        .iter()
        .all(|s| s["projectId"] == public_id.as_str()));
    let recent = res.json["recent"].as_array().unwrap();
    assert!(!recent.is_empty());
    assert!(recent.iter().all(|r| r["title"] != "비공개 일"));
    let member_ids: Vec<&str> = res.json["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["userId"].as_str().unwrap())
        .collect();
    assert!(member_ids.contains(&member_id.to_string().as_str()));
    let workspaces = res.json["workspaces"].as_array().unwrap();
    assert_eq!(workspaces.len(), 1);
    assert_eq!(workspaces[0]["assignedCount"], 3);
    assert_eq!(workspaces[0]["unreadCount"], 0);

    // Owner sees the private project and its task first by due date.
    let res = call(
        &h.app,
        "GET",
        "/api/v1/me/dashboard",
        None,
        Some(&h.owner_cookie),
        peer(80),
    )
    .await;
    assert_eq!(res.json["assigned"][0]["title"], "비공개 일");

    // lastVisited moves a workspace first; strict query.
    let res = call(
        &h.app,
        "POST",
        "/api/v1/me/personal-workspace",
        None,
        Some(&member_cookie),
        peer(2),
    )
    .await;
    assert!(res.status.is_success());
    let personal: Uuid =
        sqlx::query_scalar("SELECT personal_workspace_id FROM fvoci.users WHERE id = $1")
            .bind(member_id)
            .fetch_one(&h.admin)
            .await
            .unwrap();
    let res = call(
        &h.app,
        "GET",
        &format!("/api/v1/me/dashboard?lastVisited={personal}"),
        None,
        Some(&member_cookie),
        peer(80),
    )
    .await;
    assert_eq!(res.json["workspaces"][0]["id"], personal.to_string());
    for bad in ["?lastVisited=nope", "?other=1"] {
        let res = call(
            &h.app,
            "GET",
            &format!("/api/v1/me/dashboard{bad}"),
            None,
            Some(&member_cookie),
            peer(80),
        )
        .await;
        assert_eq!(res.status, StatusCode::BAD_REQUEST, "{bad}");
    }
    let res = call(&h.app, "GET", "/api/v1/me/dashboard", None, None, peer(80)).await;
    assert_eq!(res.status, StatusCode::UNAUTHORIZED);

    // Locate.
    let locate = |cookie: String, kind: &'static str, id: String| {
        let app = h.app.clone();
        async move {
            call(
                &app,
                "GET",
                &format!("/api/v1/me/locate?type={kind}&id={id}"),
                None,
                Some(&cookie),
                peer(81),
            )
            .await
        }
    };
    let res = locate(member_cookie.clone(), "task", tasks[0].clone()).await;
    assert_eq!(res.status, StatusCode::OK);
    assert_eq!(res.json, json!({"workspaceId": ws.to_string()}));
    let res = locate(member_cookie.clone(), "task", tasks[3].clone()).await;
    assert_eq!(res.status, StatusCode::NOT_FOUND);
    let res = locate(h.owner_cookie.clone(), "task", tasks[3].clone()).await;
    assert_eq!(res.status, StatusCode::OK);
    let res = locate(member_cookie.clone(), "task", Uuid::now_v7().to_string()).await;
    assert_eq!(res.status, StatusCode::NOT_FOUND);
    let res = locate(member_cookie.clone(), "project", tasks[0].clone()).await;
    assert_eq!(res.status, StatusCode::BAD_REQUEST);
    let root = public["rootDocumentId"].as_str().map(str::to_string);
    if let Some(root) = root {
        let res = locate(member_cookie.clone(), "document", root).await;
        assert_eq!(res.status, StatusCode::OK, "{:?}", res.json);
    }
    // Membership removal is observed on the next request.
    sqlx::query("DELETE FROM fvoci.memberships WHERE workspace_id = $1 AND user_id = $2")
        .bind(ws)
        .bind(member_id)
        .execute(&h.admin)
        .await
        .unwrap();
    let res = locate(member_cookie.clone(), "task", tasks[0].clone()).await;
    assert_eq!(res.status, StatusCode::NOT_FOUND);
    let res = call(
        &h.app,
        "GET",
        "/api/v1/me/dashboard",
        None,
        Some(&member_cookie),
        peer(80),
    )
    .await;
    assert_eq!(res.json["assigned"], json!([]));
    h.finish().await;
}
