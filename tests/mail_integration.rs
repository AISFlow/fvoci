#![cfg(feature = "db-tests")]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use fvoci_server::auth::password::Keyring;
use fvoci_server::auth::token::hash_token;
use fvoci_server::auth::AuthService;
use fvoci_server::db::outbox::{is_processed, mark_processed, read_events};
use fvoci_server::db::{migrate, pool, Db};
use fvoci_server::http::rate_limit::RateLimiter;
use fvoci_server::http::{router, state::AppState};
use fvoci_server::mail::{Mailer, SmtpConfig};
use rand::RngCore;
use serde_json::{json, Value};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tower::ServiceExt;
use uuid::Uuid;

const PEPPER: &str =
    r#"{"test":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#;

fn test_peer() -> std::net::SocketAddr {
    std::net::SocketAddr::from(([203, 0, 113, 10], 42424))
}

struct CapturedMail {
    to: String,
    data: String,
}

impl CapturedMail {
    /// The message as a mail client shows it: decoded subject and plain body.
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
                    let _ = serve_smtp(socket, captured, false).await;
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
        self.mails
            .lock()
            .expect("mails")
            .iter()
            .map(|mail| CapturedMail {
                to: mail.to.clone(),
                data: mail.data.clone(),
            })
            .collect()
    }

    async fn wait_for(&self, predicate: impl Fn(&CapturedMail) -> bool) -> CapturedMail {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(mail) = self.snapshot().into_iter().find(&predicate) {
                return mail;
            }
            if std::time::Instant::now() >= deadline {
                panic!("smtp sink did not capture expected mail");
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

impl Drop for SmtpSink {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

struct RejectingSmtp {
    port: u16,
    handle: JoinHandle<()>,
}

impl RejectingSmtp {
    async fn spawn() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind smtp");
        let port = listener.local_addr().expect("addr").port();
        let handle = tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let _ = serve_smtp(socket, Arc::new(Mutex::new(Vec::new())), true).await;
                });
            }
        });
        Self { port, handle }
    }
}

impl Drop for RejectingSmtp {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

async fn serve_smtp(
    socket: tokio::net::TcpStream,
    captured: Arc<Mutex<Vec<CapturedMail>>>,
    reject: bool,
) -> Result<(), std::io::Error> {
    let (reader, mut writer) = socket.into_split();
    let mut reader = BufReader::new(reader);
    writer.write_all(b"220 fvoci-test\r\n").await?;
    let mut rcpt = String::new();
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            break;
        }
        let command = line.trim_end_matches(['\r', '\n']);
        let upper = command.to_ascii_uppercase();
        if upper.starts_with("EHLO") || upper.starts_with("HELO") {
            writer.write_all(b"250 fvoci\r\n").await?;
        } else if upper.starts_with("MAIL FROM:") {
            if reject {
                writer.write_all(b"550 no\r\n").await?;
                continue;
            }
            writer.write_all(b"250 ok\r\n").await?;
        } else if upper.starts_with("RCPT TO:") {
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

struct TestDb {
    admin_url: String,
    app_url: String,
    db_name: String,
    role_name: String,
}

impl TestDb {
    async fn bootstrap() -> Self {
        let admin_base = std::env::var("TEST_DATABASE_URL")
            .or_else(|_| std::env::var("FVOCI_TEST_DATABASE_URL"))
            .expect("TEST_DATABASE_URL missing");

        let db_name = format!("fvoci_mail_{}", Uuid::now_v7().simple());
        let role_name = format!("fvoci_app_{}", db_name.replace('-', "_"));
        let mut password_bytes = [0u8; 24];
        rand::rng().fill_bytes(&mut password_bytes);
        let role_password = hex::encode(password_bytes);
        let server_url = server_db_url(&admin_base);

        let admin_pool = PgPoolOptions::new()
            .max_connections(2)
            .connect(&server_url)
            .await
            .expect("connect admin");
        sqlx::query(&format!("CREATE DATABASE \"{db_name}\""))
            .execute(&admin_pool)
            .await
            .expect("create database");
        admin_pool.close().await;

        let admin_url = join_db_url(&server_url, &db_name);
        migrate::run_migrations(&admin_url).await.expect("migrate");

        let migration_pool = PgPoolOptions::new()
            .max_connections(2)
            .connect(&admin_url)
            .await
            .expect("connect migration db");
        sqlx::query(&format!(
            "CREATE ROLE \"{role_name}\" LOGIN PASSWORD '{role_password}' NOSUPERUSER NOBYPASSRLS"
        ))
        .execute(&migration_pool)
        .await
        .expect("create role");
        fvoci_server::db::migrate::apply_app_role_grants(&migration_pool, &role_name)
            .await
            .expect("grant");
        migration_pool.close().await;

        let mut app = url::Url::parse(&admin_url).expect("database url");
        app.set_username(&role_name).ok();
        app.set_password(Some(&role_password)).ok();

        Self {
            admin_url,
            app_url: app.to_string(),
            db_name,
            role_name,
        }
    }

    async fn cleanup(self) {
        let server_url = server_db_url(&self.admin_url);
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(&server_url)
            .await
            .ok();
        if let Some(pool) = pool {
            let _ = sqlx::query(&format!(
                "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = '{}'",
                self.db_name
            ))
            .execute(&pool)
            .await;
            let _ = sqlx::query(&format!("DROP DATABASE IF EXISTS \"{}\"", self.db_name))
                .execute(&pool)
                .await;
            let _ = sqlx::query(&format!("DROP ROLE IF EXISTS \"{}\"", self.role_name))
                .execute(&pool)
                .await;
            pool.close().await;
        }
    }

    async fn admin(&self) -> PgPool {
        PgPoolOptions::new()
            .max_connections(2)
            .connect(&self.admin_url)
            .await
            .unwrap()
    }
}

fn server_db_url(url: &str) -> String {
    let parsed = url::Url::parse(url).expect("database url");
    let mut server = parsed;
    server.set_path("");
    server.set_query(None);
    server.to_string().trim_end_matches('/').to_string()
}

fn join_db_url(server_url: &str, db_name: &str) -> String {
    let mut parsed = url::Url::parse(server_url).expect("server url");
    parsed.set_path(&format!("/{db_name}"));
    parsed.to_string()
}

fn mailer_for(port: u16) -> Arc<Mailer> {
    Arc::new(Mailer::from_smtp(Some(SmtpConfig {
        host: "127.0.0.1".into(),
        port,
        from: "noreply@example.com".into(),
    })))
}

async fn app_state(app_url: &str, mailer: Arc<Mailer>) -> AppState {
    let pool = pool::connect_app(app_url).await.expect("app pool");
    let storage_root = std::env::temp_dir().join(format!("fvoci-mail-test-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&storage_root).expect("storage root");
    AppState {
        auth: Arc::new(AuthService {
            db: Db::new(pool),
            password_keys: Keyring::parse(PEPPER, "test").expect("pepper"),
        }),
        branding_name: "FVOCI".to_string(),
        public_origin: "http://localhost".to_string(),
        cookie_secure: false,
        rate_limiter: RateLimiter::new(),
        storage: fvoci_server::attachments::ObjectStorage::from(
            fvoci_server::attachments::LocalStorage::new(storage_root),
        ),
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
        mailer,
        markdown: Some(
            fvoci_server::documents::markdown_helper::MarkdownHelper::new(env!(
                "CARGO_BIN_EXE_fvoci-server"
            )),
        ),
        import_wake: None,
        import_extractor_available: false,
        preview_extract: None,
        quota: Default::default(),
        streams: fvoci_server::http::state::AppState::fresh_streams(),
    }
}

async fn json_request(
    app: axum::Router,
    method: &str,
    path: &str,
    body: Option<Value>,
    cookie: Option<&str>,
    extra_headers: &[(&str, &str)],
    peer: Option<std::net::SocketAddr>,
) -> (StatusCode, Value, Option<String>, HeaderMap) {
    let peer = peer.unwrap_or_else(test_peer);
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(cookie) = cookie {
        builder = builder.header("cookie", format!("fvoci_session={cookie}"));
    }
    for (name, value) in extra_headers {
        builder = builder.header(*name, *value);
    }
    let mut request = if let Some(body) = body {
        builder
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    } else {
        builder.body(Body::empty()).unwrap()
    };
    request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(peer));
    let response = app.oneshot(request).await.expect("response");
    let status = response.status();
    let headers = response.headers().clone();
    let set_cookie = response
        .headers()
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap_or_default();
    let json = if bytes.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&bytes).unwrap_or(json!({}))
    };
    (status, json, set_cookie, headers)
}

fn extract_session_cookie(set_cookie: &str) -> String {
    set_cookie
        .split(';')
        .next()
        .unwrap_or("")
        .split('=')
        .nth(1)
        .unwrap_or("")
        .to_string()
}

async fn setup_session(harness: &TestDb, mailer: Arc<Mailer>) -> (axum::Router, String, Uuid) {
    let app = router(app_state(&harness.app_url, mailer).await, None);
    let (_, _, cookie_hdr, _) = json_request(
        app.clone(),
        "POST",
        "/api/v1/setup",
        Some(json!({
            "email": "admin@example.com",
            "password": "supersecret1",
            "givenName": "Admin",
            "workspaceSlug": "acme",
            "workspaceName": "Acme"
        })),
        None,
        &[],
        None,
    )
    .await;
    let cookie = extract_session_cookie(cookie_hdr.as_ref().unwrap());
    let admin = harness.admin().await;
    let user_id: (Uuid,) = sqlx::query_as("SELECT id FROM fvoci.users LIMIT 1")
        .fetch_one(&admin)
        .await
        .unwrap();
    admin.close().await;
    (app, cookie, user_id.0)
}

fn extract_token(data: &str) -> String {
    data.split("token=")
        .nth(1)
        .and_then(|rest| {
            rest.split(|c: char| c.is_whitespace() || c == '\r' || c == '\n')
                .next()
        })
        .expect("token in mail")
        .to_string()
}

#[tokio::test]
async fn setup_reports_mail_enabled_from_smtp_config() {
    let harness = TestDb::bootstrap().await;
    let sink = SmtpSink::spawn().await;
    let app = router(
        app_state(&harness.app_url, mailer_for(sink.port)).await,
        None,
    );
    let (status, body, _, _) =
        json_request(app, "GET", "/api/v1/setup", None, None, &[], None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["mailEnabled"], true);
    let disabled = router(
        app_state(&harness.app_url, Arc::new(Mailer::disabled())).await,
        None,
    );
    let (status, body, _, _) =
        json_request(disabled, "GET", "/api/v1/setup", None, None, &[], None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["mailEnabled"], false);
    harness.cleanup().await;
}

#[tokio::test]
async fn invitation_sends_mail_and_smtp_failure_sets_mail_delayed() {
    let harness = TestDb::bootstrap().await;
    let sink = SmtpSink::spawn().await;
    let (app, cookie, _) = setup_session(&harness, mailer_for(sink.port)).await;
    let admin = harness.admin().await;
    let ws: Uuid = sqlx::query_scalar("SELECT id FROM fvoci.workspaces WHERE slug = 'acme'")
        .fetch_one(&admin)
        .await
        .unwrap();
    admin.close().await;
    let (status, body, _, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{ws}/invitations"),
        Some(json!({"email": "invitee@example.com", "role": "member"})),
        Some(&cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert!(body["mailDelayed"].is_null());
    let accept_url = body["acceptUrl"].as_str().unwrap();
    let mail = sink.wait_for(|mail| mail.to == "invitee@example.com").await;
    assert!(mail.text().contains("Subject: 워크스페이스 초대"));
    assert!(mail.text().contains(accept_url));

    let reject = RejectingSmtp::spawn().await;
    let app = router(
        app_state(&harness.app_url, mailer_for(reject.port)).await,
        None,
    );
    let (status, body, _, _) = json_request(
        app,
        "POST",
        &format!("/api/v1/workspaces/{ws}/invitations"),
        Some(json!({"email": "delayed@example.com", "role": "member"})),
        Some(&cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["mailDelayed"], true);
    assert!(body["acceptUrl"].as_str().unwrap().contains("/invite/"));
    harness.cleanup().await;
}

#[tokio::test]
async fn password_reset_hides_enumeration_hashes_token_revokes_sessions() {
    let harness = TestDb::bootstrap().await;
    let sink = SmtpSink::spawn().await;
    let (app, cookie, user_id) = setup_session(&harness, mailer_for(sink.port)).await;

    let unknown_peer = std::net::SocketAddr::from(([198, 51, 100, 1], 9));
    let started = std::time::Instant::now();
    let (status, body, set_cookie, _) = json_request(
        app.clone(),
        "POST",
        "/api/v1/auth/password-reset",
        Some(json!({"email": "missing@example.com"})),
        None,
        &[],
        Some(unknown_peer),
    )
    .await;
    let unknown_elapsed = started.elapsed();
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(body, json!({"ok": true}));
    assert!(set_cookie.is_none());

    let known_peer = std::net::SocketAddr::from(([198, 51, 100, 2], 9));
    let started = std::time::Instant::now();
    let (status, body, set_cookie, _) = json_request(
        app.clone(),
        "POST",
        "/api/v1/auth/password-reset",
        Some(json!({"email": "Admin@example.com"})),
        None,
        &[("authorization", "Bearer pat-should-be-ignored")],
        Some(known_peer),
    )
    .await;
    let known_elapsed = started.elapsed();
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(body, json!({"ok": true}));
    assert!(set_cookie.is_none());
    assert!(unknown_elapsed >= Duration::from_millis(100));
    assert!(known_elapsed >= Duration::from_millis(100));

    let mail = sink
        .wait_for(|mail| mail.text().contains("/reset-password?token="))
        .await;
    assert_eq!(mail.to, "admin@example.com");
    assert!(mail.text().contains("Subject: FVOCI 비밀번호 재설정"));
    let parsed = mailparse::parse_mail(mail.data.as_bytes()).expect("parse reset mail");
    for header in ["Message-ID", "Date"] {
        assert!(
            parsed
                .headers
                .iter()
                .any(|h| h.get_key().eq_ignore_ascii_case(header)),
            "reset mail carries {header}"
        );
    }
    let token = extract_token(&mail.text());
    assert!(!mail.text().contains(&hash_token(&token)));

    let admin = harness.admin().await;
    let stored: String = sqlx::query_scalar("SELECT token_hash FROM fvoci.magic_tokens")
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_eq!(stored, hash_token(&token));
    assert_ne!(stored, token);
    admin.close().await;

    let (me_status, _, _, _) = json_request(
        app.clone(),
        "GET",
        "/api/v1/auth/me",
        None,
        Some(&cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(me_status, StatusCode::OK);

    let (status, body, set_cookie, _) = json_request(
        app.clone(),
        "POST",
        "/api/v1/auth/password-reset/confirm",
        Some(json!({"token": token, "newPassword": "newsecret12"})),
        None,
        &[],
        Some(std::net::SocketAddr::from(([198, 51, 100, 3], 9))),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({"ok": true}));
    assert!(set_cookie
        .as_deref()
        .map(|value| !value.contains("fvoci_session="))
        .unwrap_or(true));

    let (me_status, _, _, _) = json_request(
        app.clone(),
        "GET",
        "/api/v1/auth/me",
        None,
        Some(&cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(me_status, StatusCode::UNAUTHORIZED);

    let (status, _, _, _) = json_request(
        app.clone(),
        "POST",
        "/api/v1/auth/login",
        Some(json!({"email": "admin@example.com", "password": "supersecret1"})),
        None,
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _, new_cookie, _) = json_request(
        app.clone(),
        "POST",
        "/api/v1/auth/login",
        Some(json!({"email": "admin@example.com", "password": "newsecret12"})),
        None,
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(new_cookie.is_some());

    let (status, body, _, _) = json_request(
        app.clone(),
        "POST",
        "/api/v1/auth/password-reset/confirm",
        Some(json!({"token": token, "newPassword": "anotherpass1"})),
        None,
        &[],
        Some(std::net::SocketAddr::from(([198, 51, 100, 4], 9))),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "magic_invalid");

    let admin = harness.admin().await;
    let verb: String = sqlx::query_scalar(
        "SELECT verb FROM fvoci.events WHERE verb = 'auth.password_reset' AND actor_user_id = $1",
    )
    .bind(user_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(verb, "auth.password_reset");
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn password_reset_confirm_shares_magic_ip_limit() {
    let harness = TestDb::bootstrap().await;
    let app = router(
        app_state(&harness.app_url, Arc::new(Mailer::disabled())).await,
        None,
    );
    let peer = std::net::SocketAddr::from(([203, 0, 113, 99], 9));
    for i in 0..30 {
        let (status, _, _, _) = json_request(
            app.clone(),
            "POST",
            "/api/v1/auth/password-reset/confirm",
            Some(json!({"token": format!("forged-{i}"), "newPassword": "longenoughpw1"})),
            None,
            &[],
            Some(peer),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "attempt {i}");
    }
    let (status, body, _, headers) = json_request(
        app,
        "POST",
        "/api/v1/auth/password-reset/confirm",
        Some(json!({"token": "forged-overflow", "newPassword": "longenoughpw1"})),
        None,
        &[],
        Some(peer),
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(body["code"], "rate_limit_exceeded");
    assert!(headers.get("retry-after").is_some());
    harness.cleanup().await;
}

#[tokio::test]
async fn mail_consumer_is_at_least_once_and_skips_after_processed_events() {
    let harness = TestDb::bootstrap().await;
    let sink = SmtpSink::spawn().await;
    let mailer = mailer_for(sink.port);
    let (app, _cookie, user_id) = setup_session(&harness, mailer.clone()).await;
    drop(app);

    let admin = harness.admin().await;
    sqlx::query("SELECT set_config('app.system_ctx', 'on', true)")
        .execute(&admin)
        .await
        .unwrap();
    let event_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO fvoci.events (id, actor_user_id, verb, payload, channel)
        VALUES ($1, $2, 'identity.linked', '{"provider":"oidc"}'::jsonb, 'web')
        "#,
    )
    .bind(event_id)
    .bind(user_id)
    .execute(&admin)
    .await
    .unwrap();
    admin.close().await;

    let app_pool = pool::connect_app(&harness.app_url).await.unwrap();
    fvoci_server::db::outbox::ensure_consumer(&app_pool, "mail")
        .await
        .unwrap();
    // The relay reads settled events only (xact < cluster-wide snapshot xmin);
    // another test database's open transaction can delay that briefly.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    let events = loop {
        let events = read_events(&app_pool, "mail", 100).await.unwrap();
        if events.iter().any(|event| event.id == event_id) {
            break events;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "mail consumer can read identity.linked"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    };
    let event = events
        .iter()
        .find(|event| event.id == event_id)
        .expect("mail consumer can read identity.linked");
    let consumer = fvoci_server::mail::mail_consumer(mailer);
    consumer
        .deliver(&app_pool, Uuid::now_v7(), event)
        .await
        .expect("first deliver");
    sink.wait_for(|mail| mail.text().contains("소셜 로그인"))
        .await;
    assert_eq!(sink.snapshot().len(), 1);

    consumer
        .deliver(&app_pool, Uuid::now_v7(), event)
        .await
        .expect("replay before mark");
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if sink.snapshot().len() >= 2 {
            break;
        }
        if std::time::Instant::now() >= deadline {
            panic!("replay before processed_events did not send a second mail");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    mark_processed(&app_pool, "mail", event.id).await.unwrap();
    assert!(is_processed(&app_pool, "mail", event.id).await.unwrap());
    app_pool.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn app_role_cannot_select_magic_tokens() {
    let harness = TestDb::bootstrap().await;
    let app = pool::connect_app(&harness.app_url).await.unwrap();
    let denied = sqlx::query_scalar::<_, String>("SELECT token_hash FROM fvoci.magic_tokens")
        .fetch_optional(&app)
        .await;
    assert!(denied.is_err());
    app.close().await;
    harness.cleanup().await;
}

fn header_values(mail: &CapturedMail, name: &str) -> Vec<String> {
    let parsed = mailparse::parse_mail(mail.data.as_bytes()).expect("parse captured mail");
    parsed
        .headers
        .iter()
        .filter(|h| h.get_key().eq_ignore_ascii_case(name))
        .map(|h| h.get_value())
        .collect()
}

async fn deliver_identity_linked(harness: &TestDb, mailer: Arc<Mailer>, user_id: Uuid) {
    let admin = harness.admin().await;
    let event_id = Uuid::now_v7();
    let mut tx = admin.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.system_ctx', 'on', true)")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        r#"
        INSERT INTO fvoci.events (id, actor_user_id, verb, payload, channel)
        VALUES ($1, $2, 'identity.linked', '{"provider":"oidc"}'::jsonb, 'web')
        "#,
    )
    .bind(event_id)
    .bind(user_id)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    admin.close().await;

    let app_pool = pool::connect_app(&harness.app_url).await.unwrap();
    fvoci_server::db::outbox::ensure_consumer(&app_pool, "mail")
        .await
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    let event = loop {
        let events = read_events(&app_pool, "mail", 100).await.unwrap();
        if let Some(event) = events.into_iter().find(|event| event.id == event_id) {
            break event;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "mail consumer can read identity.linked"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    fvoci_server::mail::mail_consumer(mailer)
        .deliver(&app_pool, Uuid::now_v7(), &event)
        .await
        .expect("deliver identity mail");
    app_pool.close().await;
}

/// Source `settings-overrides.test.ts`: an admin PATCH of `i18n.overrides`
/// changes the next mail without a restart; absent keys keep the Korean
/// default, an invalid PATCH changes nothing and a reset restores defaults.
#[tokio::test]
async fn i18n_overrides_reach_the_next_mail_live_and_reset() {
    let harness = TestDb::bootstrap().await;
    let sink = SmtpSink::spawn().await;
    let mailer = mailer_for(sink.port);
    let (app, cookie, user_id) = setup_session(&harness, mailer.clone()).await;
    let admin = harness.admin().await;
    let ws: Uuid = sqlx::query_scalar("SELECT id FROM fvoci.workspaces WHERE slug = 'acme'")
        .fetch_one(&admin)
        .await
        .unwrap();
    admin.close().await;

    let patch = |body: Value| {
        let app = app.clone();
        let cookie = cookie.clone();
        async move {
            json_request(
                app,
                "PATCH",
                "/api/v1/admin/instance-settings",
                Some(body),
                Some(&cookie),
                &[],
                None,
            )
            .await
        }
    };
    let invite = |email: &'static str| {
        let app = app.clone();
        let cookie = cookie.clone();
        async move {
            let (status, body, _, _) = json_request(
                app,
                "POST",
                &format!("/api/v1/workspaces/{ws}/invitations"),
                Some(json!({"email": email, "role": "member"})),
                Some(&cookie),
                &[],
                None,
            )
            .await;
            assert_eq!(status, StatusCode::CREATED, "{body}");
            assert!(body["mailDelayed"].is_null());
            body["acceptUrl"].as_str().unwrap().to_string()
        }
    };
    let reset_password = |octet: u8| {
        let app = app.clone();
        async move {
            let (status, _, _, _) = json_request(
                app,
                "POST",
                "/api/v1/auth/password-reset",
                Some(json!({"email": "admin@example.com"})),
                None,
                &[],
                Some(std::net::SocketAddr::from(([198, 51, 100, octet], 9))),
            )
            .await;
            assert_eq!(status, StatusCode::ACCEPTED);
        }
    };

    let overrides = json!({
        "mail.invite.subject": "Team invite\r\nBcc: intruder@example.com",
        "mail.magic.reset.subject": "Reset your password",
        "mail.magic.link.text": "Open {{url}} within {{minutes}} minutes. {{title}}",
        "mail.identity.linked.subject": "Sign-in linked",
        "mail.identity.linked.text": "Provider {{provider}} was linked."
    });
    // `{{title}}` is not a variable of this key: the PATCH is rejected.
    let (status, body, _, _) = patch(json!({"i18n": {"overrides": overrides}})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let mut overrides = overrides;
    overrides["mail.magic.link.text"] = json!("Open {{url}} within {{minutes}} minutes.");
    let (status, body, _, _) = patch(json!({"i18n": {"overrides": overrides}})).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Invitation: override subject, header-safe (no injected Bcc header or
    // extra recipient), body unchanged.
    let accept_url = invite("first@example.com").await;
    let mail = sink.wait_for(|m| m.to == "first@example.com").await;
    assert_eq!(
        header_values(&mail, "subject"),
        ["Team invite  Bcc: intruder@example.com"]
    );
    assert!(header_values(&mail, "bcc").is_empty());
    assert!(mail.text().contains(&accept_url));

    // Password reset: override subject and link text with both variables.
    reset_password(20).await;
    let mail = sink
        .wait_for(|m| m.text().contains("/reset-password?token="))
        .await;
    assert!(mail.text().contains("Subject: Reset your password"));
    assert!(mail
        .text()
        .contains("Open http://localhost/reset-password?token="));
    assert!(mail.text().contains(" within 15 minutes."));
    assert!(!mail.text().contains("분 안에"));

    // Outbox identity mail (read at delivery, in the consumer transaction).
    deliver_identity_linked(&harness, mailer.clone(), user_id).await;
    let mail = sink
        .wait_for(|m| m.text().contains("Provider oidc was linked."))
        .await;
    assert!(mail.text().contains("Subject: Sign-in linked"));

    // Invalid PATCHes leave the stored map and the next mail unchanged.
    for bad in [
        json!({"i18n": {"overrides": {"mail.invite.subject": "<b>x</b>"}}}),
        json!({"i18n": {"overrides": {"mail.magic.link.text": "no url"}}}),
        json!({"i18n": {"overrides": {"mail.comment.subject": "not overridable"}}}),
        json!({"i18n": {"overrides": {"mail.invite.subject": "   "}}}),
    ] {
        let (status, body, _, _) = patch(bad.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}: {body}");
    }
    invite("second@example.com").await;
    let mail = sink.wait_for(|m| m.to == "second@example.com").await;
    assert!(mail.text().contains("Subject: Team invite  Bcc:"));

    // The anonymous projection never carries the map.
    let (status, public, _, _) = json_request(
        app.clone(),
        "GET",
        "/api/v1/instance",
        None,
        None,
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(public["values"].get("i18n").is_none());
    assert!(!public.to_string().contains("Team invite"));

    // Live update: the next send uses the new copy, no restart.
    let (status, _, _, _) = patch(json!({"i18n": {"overrides": {
        "mail.invite.subject": "Invitation v2"
    }}}))
    .await;
    assert_eq!(status, StatusCode::OK);
    invite("third@example.com").await;
    let mail = sink.wait_for(|m| m.to == "third@example.com").await;
    assert!(mail.text().contains("Subject: Invitation v2"));
    // Keys dropped from the map are back to the default.
    reset_password(21).await;
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let resets = loop {
        let resets: Vec<CapturedMail> = sink
            .snapshot()
            .into_iter()
            .filter(|m| m.text().contains("/reset-password?token="))
            .collect();
        if resets.len() >= 2 {
            break resets;
        }
        assert!(std::time::Instant::now() < deadline, "second reset mail");
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert!(resets[1].text().contains("Subject: FVOCI 비밀번호 재설정"));
    assert!(resets[1].text().contains("15분 안에 사용할 수 있습니다."));

    // Reset to the absent default.
    let (status, body, _, _) = patch(json!({"i18n": null})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let admin = harness.admin().await;
    let rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM fvoci.instance_settings WHERE key = 'i18n'")
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(rows, 0);
    admin.close().await;
    invite("fourth@example.com").await;
    let mail = sink.wait_for(|m| m.to == "fourth@example.com").await;
    assert!(mail.text().contains("Subject: 워크스페이스 초대"));
    harness.cleanup().await;
}

/// How the scripted SMTP server answers `RCPT TO` for one address.
#[derive(Clone)]
struct RcptRule {
    reply: &'static str,
    /// `None`: every time; `Some(n)`: the next `n` times, then `250`.
    times: Option<u32>,
}

/// SMTP sink with a per-session delay and per-recipient `RCPT` replies.
struct ScriptedSmtp {
    port: u16,
    mails: Arc<Mutex<Vec<CapturedMail>>>,
    handle: JoinHandle<()>,
}

impl ScriptedSmtp {
    async fn spawn(session_delay: Duration, rules: Vec<(String, RcptRule)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind smtp");
        let port = listener.local_addr().expect("addr").port();
        let mails = Arc::new(Mutex::new(Vec::new()));
        let rules = Arc::new(Mutex::new(
            rules
                .into_iter()
                .collect::<std::collections::HashMap<String, RcptRule>>(),
        ));
        let captured = mails.clone();
        let handle = tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = listener.accept().await else {
                    break;
                };
                let captured = captured.clone();
                let rules = rules.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(session_delay).await;
                    let _ = serve_scripted_smtp(socket, captured, rules).await;
                });
            }
        });
        Self {
            port,
            mails,
            handle,
        }
    }

    fn count_to(&self, address: &str) -> usize {
        self.mails
            .lock()
            .expect("mails")
            .iter()
            .filter(|mail| mail.to == address)
            .count()
    }

    fn count_text(&self, needle: &str) -> usize {
        self.mails
            .lock()
            .expect("mails")
            .iter()
            .filter(|mail| mail.text().contains(needle))
            .count()
    }
}

impl Drop for ScriptedSmtp {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

async fn serve_scripted_smtp(
    socket: tokio::net::TcpStream,
    captured: Arc<Mutex<Vec<CapturedMail>>>,
    rules: Arc<Mutex<std::collections::HashMap<String, RcptRule>>>,
) -> Result<(), std::io::Error> {
    let (reader, mut writer) = socket.into_split();
    let mut reader = BufReader::new(reader);
    writer.write_all(b"220 fvoci-test\r\n").await?;
    let mut rcpt = String::new();
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            break;
        }
        let command = line.trim_end_matches(['\r', '\n']);
        let upper = command.to_ascii_uppercase();
        if upper.starts_with("EHLO") || upper.starts_with("HELO") {
            writer.write_all(b"250 fvoci\r\n").await?;
        } else if upper.starts_with("RCPT TO:") {
            let address = command
                .split(':')
                .nth(1)
                .unwrap_or("")
                .trim()
                .trim_matches(|c| c == '<' || c == '>')
                .to_string();
            let reply = {
                let mut rules = rules.lock().expect("rules");
                match rules.get_mut(&address) {
                    Some(rule) => match rule.times {
                        None => Some(rule.reply),
                        Some(0) => None,
                        Some(ref mut left) => {
                            *left -= 1;
                            Some(rule.reply)
                        }
                    },
                    None => None,
                }
            };
            match reply {
                Some(reply) => {
                    writer.write_all(format!("{reply}\r\n").as_bytes()).await?;
                }
                None => {
                    rcpt = address;
                    writer.write_all(b"250 ok\r\n").await?;
                }
            }
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

/// Move the `mail` cursor past every event recorded so far, so a test's
/// dispatcher only sees the events the test adds next.
async fn start_mail_cursor_at_latest_event(admin: &PgPool) {
    sqlx::query(
        r#"
        INSERT INTO fvoci.outbox_consumers (consumer, last_xact, last_seq)
        SELECT 'mail', e.xact, e.seq
        FROM fvoci.events AS e
        ORDER BY e.xact DESC, e.seq DESC
        LIMIT 1
        ON CONFLICT (consumer) DO UPDATE
        SET last_xact = EXCLUDED.last_xact, last_seq = EXCLUDED.last_seq
        "#,
    )
    .execute(admin)
    .await
    .expect("mail cursor");
}

fn run_mail_dispatcher(
    app_pool: PgPool,
    mailer: Arc<Mailer>,
    lease_ttl: Duration,
) -> fvoci_server::outbox::OutboxDispatcherHandle {
    fvoci_server::outbox::spawn_outbox_dispatcher(
        fvoci_server::outbox::OutboxDispatcherSettings {
            poll_interval: Duration::from_millis(20),
            lease_ttl,
            batch_limit: 100,
            failure_backoff: Duration::from_millis(50),
        },
        app_pool,
        vec![fvoci_server::mail::mail_consumer(mailer)],
    )
    .expect("dispatcher")
}

/// Wait until every event is processed by `mail` or dead-lettered, or the
/// deadline passes (the caller's assertions then report what happened).
async fn wait_mail_settled(app_pool: &PgPool, ids: &[Uuid], deadline: Duration) {
    let until = std::time::Instant::now() + deadline;
    loop {
        let mut settled = true;
        for id in ids {
            let processed = is_processed(app_pool, "mail", *id).await.unwrap_or(false);
            let dead = fvoci_server::db::outbox::fetch_failure_state(app_pool, "mail", *id)
                .await
                .ok()
                .flatten()
                .is_some_and(|row| row.dead_at.is_some());
            if !processed && !dead {
                settled = false;
                break;
            }
        }
        if settled || std::time::Instant::now() >= until {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn mail_failure_rows(admin: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM fvoci.outbox_failures WHERE consumer = 'mail'")
        .fetch_one(admin)
        .await
        .expect("mail failure rows")
}

/// A comment on a project document that mentions three members. Returns the
/// comment.created event id and the three recipient addresses in the order
/// the mail consumer sends them (recipient user id order).
async fn comment_event_for_three_recipients(
    harness: &TestDb,
    owner_id: Uuid,
) -> (Uuid, Vec<String>) {
    let admin = harness.admin().await;
    let workspace_id: Uuid =
        sqlx::query_scalar("SELECT id FROM fvoci.workspaces WHERE slug = 'acme'")
            .fetch_one(&admin)
            .await
            .expect("workspace");
    let mut recipients: Vec<(Uuid, String)> = Vec::new();
    for label in ["rcpt-a", "rcpt-b", "rcpt-c"] {
        let user_id = Uuid::now_v7();
        let email = format!("{label}@example.com");
        sqlx::query("INSERT INTO fvoci.users (id, email, given_name) VALUES ($1, $2, $3)")
            .bind(user_id)
            .bind(&email)
            .bind(label)
            .execute(&admin)
            .await
            .expect("insert user");
        sqlx::query(
            "INSERT INTO fvoci.memberships (workspace_id, user_id, role) VALUES ($1, $2, 'member')",
        )
        .bind(workspace_id)
        .bind(user_id)
        .execute(&admin)
        .await
        .expect("insert membership");
        recipients.push((user_id, email));
    }
    recipients.sort();
    let project_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO fvoci.projects (
            id, workspace_id, key, name, visibility, status, next_number, created_by
        ) VALUES ($1, $2, 'MAIL', 'MAIL', 'workspace', 'active', 1, $3)
        "#,
    )
    .bind(project_id)
    .bind(workspace_id)
    .bind(owner_id)
    .execute(&admin)
    .await
    .expect("insert project");
    let document_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO fvoci.documents (
            id, workspace_id, title, path, parent_id, sort_key, project_id, number, status,
            schema_version, content_json, created_by
        ) VALUES (
            $1, $2, 'Mail doc', $3, NULL, 'V', $4, 1, 'published', 2,
            '{"type":"doc","content":[{"type":"paragraph"}]}'::jsonb, $5
        )
        "#,
    )
    .bind(document_id)
    .bind(workspace_id)
    .bind(document_id.simple().to_string())
    .bind(project_id)
    .bind(owner_id)
    .execute(&admin)
    .await
    .expect("insert document");
    let comment_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO fvoci.comments (id, workspace_id, document_id, created_by, body) VALUES ($1, $2, $3, $4, 'three-recipient comment')",
    )
    .bind(comment_id)
    .bind(workspace_id)
    .bind(document_id)
    .bind(owner_id)
    .execute(&admin)
    .await
    .expect("insert comment");
    start_mail_cursor_at_latest_event(&admin).await;

    let event_id = Uuid::now_v7();
    let mentioned: Vec<String> = recipients.iter().map(|(id, _)| id.to_string()).collect();
    let mut tx = admin.begin().await.expect("event tx");
    sqlx::query("SELECT set_config('app.system_ctx', 'on', true)")
        .execute(&mut *tx)
        .await
        .expect("system ctx");
    sqlx::query(
        r#"
        INSERT INTO fvoci.events (
            id, workspace_id, actor_user_id, verb, target_type, target_id, payload, channel
        ) VALUES ($1, $2, $3, 'comment.created', 'comment', $4, $5, 'web')
        "#,
    )
    .bind(event_id)
    .bind(workspace_id)
    .bind(owner_id)
    .bind(comment_id)
    .bind(json!({
        "commentId": comment_id,
        "documentId": document_id,
        "mentionedUserIds": mentioned,
    }))
    .execute(&mut *tx)
    .await
    .expect("insert comment event");
    tx.commit().await.expect("commit event");
    admin.close().await;
    (
        event_id,
        recipients.into_iter().map(|(_, email)| email).collect(),
    )
}

/// SMTP sessions slow enough that several mail events do not fit in one
/// lease: each mail is still sent exactly once and nothing is dead-lettered.
#[tokio::test]
async fn slow_smtp_sends_each_mail_event_once() {
    let harness = TestDb::bootstrap().await;
    let sink = ScriptedSmtp::spawn(Duration::from_millis(700), Vec::new()).await;
    let mailer = mailer_for(sink.port);
    let (app, _cookie, user_id) = setup_session(&harness, mailer.clone()).await;
    drop(app);
    let admin = harness.admin().await;
    start_mail_cursor_at_latest_event(&admin).await;
    let providers = ["slow-p0", "slow-p1", "slow-p2", "slow-p3"];
    let mut ids = Vec::new();
    let mut tx = admin.begin().await.expect("event tx");
    sqlx::query("SELECT set_config('app.system_ctx', 'on', true)")
        .execute(&mut *tx)
        .await
        .expect("system ctx");
    for provider in providers {
        let event_id = Uuid::now_v7();
        sqlx::query(
            r#"
            INSERT INTO fvoci.events (id, actor_user_id, verb, payload, channel)
            VALUES ($1, $2, 'identity.linked', $3, 'web')
            "#,
        )
        .bind(event_id)
        .bind(user_id)
        .bind(json!({ "provider": provider }))
        .execute(&mut *tx)
        .await
        .expect("identity event");
        ids.push(event_id);
    }
    tx.commit().await.expect("commit events");

    let app_pool = pool::connect_app(&harness.app_url).await.unwrap();
    let dispatcher = run_mail_dispatcher(app_pool.clone(), mailer, Duration::from_secs(2));
    wait_mail_settled(&app_pool, &ids, Duration::from_secs(25)).await;
    dispatcher.request_shutdown();
    dispatcher.join().await.expect("join");

    let counts: Vec<usize> = providers.iter().map(|p| sink.count_text(p)).collect();
    assert_eq!(counts, vec![1; providers.len()], "mails per event");
    assert_eq!(sink.count_to("admin@example.com"), providers.len());
    for id in &ids {
        assert!(is_processed(&app_pool, "mail", *id).await.unwrap());
    }
    assert_eq!(mail_failure_rows(&admin).await, 0);
    admin.close().await;
    app_pool.close().await;
    harness.cleanup().await;
}

/// A permanent 550 for one of three recipients is final for that recipient:
/// the other two get one mail each and the event is processed, not retried.
#[tokio::test]
async fn permanent_rejection_of_one_recipient_does_not_resend_to_the_others() {
    let harness = TestDb::bootstrap().await;
    let sink0 = SmtpSink::spawn().await;
    let (app, _cookie, owner_id) = setup_session(&harness, mailer_for(sink0.port)).await;
    drop(app);
    let (event_id, to) = comment_event_for_three_recipients(&harness, owner_id).await;
    let sink = ScriptedSmtp::spawn(
        Duration::ZERO,
        vec![(
            to[1].clone(),
            RcptRule {
                reply: "550 5.1.1 no such user",
                times: None,
            },
        )],
    )
    .await;
    let app_pool = pool::connect_app(&harness.app_url).await.unwrap();
    let dispatcher = run_mail_dispatcher(
        app_pool.clone(),
        mailer_for(sink.port),
        Duration::from_secs(5),
    );
    wait_mail_settled(&app_pool, &[event_id], Duration::from_secs(15)).await;
    dispatcher.request_shutdown();
    dispatcher.join().await.expect("join");

    let counts: Vec<usize> = to.iter().map(|address| sink.count_to(address)).collect();
    assert_eq!(counts, vec![1, 0, 1], "mails per recipient");
    assert_eq!(sink.count_text("three-recipient comment"), 2);
    assert!(is_processed(&app_pool, "mail", event_id).await.unwrap());
    let admin = harness.admin().await;
    assert_eq!(mail_failure_rows(&admin).await, 0);
    admin.close().await;
    app_pool.close().await;
    harness.cleanup().await;
}

/// A transient 451 for the second of three recipients retries the event, but
/// the retry does not send the first recipient's mail again.
#[tokio::test]
async fn transient_rejection_retry_does_not_resend_to_accepted_recipients() {
    let harness = TestDb::bootstrap().await;
    let sink0 = SmtpSink::spawn().await;
    let (app, _cookie, owner_id) = setup_session(&harness, mailer_for(sink0.port)).await;
    drop(app);
    let (event_id, to) = comment_event_for_three_recipients(&harness, owner_id).await;
    let sink = ScriptedSmtp::spawn(
        Duration::ZERO,
        vec![(
            to[1].clone(),
            RcptRule {
                reply: "451 4.7.1 try again later",
                times: Some(1),
            },
        )],
    )
    .await;
    let app_pool = pool::connect_app(&harness.app_url).await.unwrap();
    let dispatcher = run_mail_dispatcher(
        app_pool.clone(),
        mailer_for(sink.port),
        Duration::from_secs(5),
    );
    wait_mail_settled(&app_pool, &[event_id], Duration::from_secs(15)).await;
    dispatcher.request_shutdown();
    dispatcher.join().await.expect("join");

    let counts: Vec<usize> = to.iter().map(|address| sink.count_to(address)).collect();
    assert_eq!(counts, vec![1, 1, 1], "mails per recipient");
    assert_eq!(sink.count_text("three-recipient comment"), 3);
    assert!(is_processed(&app_pool, "mail", event_id).await.unwrap());
    let admin = harness.admin().await;
    assert_eq!(mail_failure_rows(&admin).await, 0);
    admin.close().await;
    app_pool.close().await;
    harness.cleanup().await;
}

/// When every recipient is rejected (a relay-wide refusal looks like this),
/// the event still fails and dead-letters where an operator can see it.
#[tokio::test]
async fn every_recipient_rejected_still_dead_letters_the_event() {
    let harness = TestDb::bootstrap().await;
    let sink0 = SmtpSink::spawn().await;
    let (app, _cookie, owner_id) = setup_session(&harness, mailer_for(sink0.port)).await;
    drop(app);
    let (event_id, to) = comment_event_for_three_recipients(&harness, owner_id).await;
    let rules = to
        .iter()
        .map(|address| {
            (
                address.clone(),
                RcptRule {
                    reply: "550 5.7.1 relaying denied",
                    times: None,
                },
            )
        })
        .collect();
    let sink = ScriptedSmtp::spawn(Duration::ZERO, rules).await;
    let app_pool = pool::connect_app(&harness.app_url).await.unwrap();
    let dispatcher = run_mail_dispatcher(
        app_pool.clone(),
        mailer_for(sink.port),
        Duration::from_secs(5),
    );
    wait_mail_settled(&app_pool, &[event_id], Duration::from_secs(15)).await;
    dispatcher.request_shutdown();
    dispatcher.join().await.expect("join");

    assert!(!is_processed(&app_pool, "mail", event_id).await.unwrap());
    let dead = fvoci_server::db::outbox::fetch_failure_state(&app_pool, "mail", event_id)
        .await
        .unwrap()
        .expect("failure row");
    assert!(dead.dead_at.is_some(), "{dead:?}");
    for address in &to {
        assert_eq!(sink.count_to(address), 0);
    }
    app_pool.close().await;
    harness.cleanup().await;
}
