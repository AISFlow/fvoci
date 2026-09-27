#![cfg(feature = "db-tests")]
#![allow(dead_code)]

//! Web Push against a real PostgreSQL with the app role: subscription PUT
//! contract, VAPID bootstrap/rotation/backup check, logout disconnect, the
//! `push` outbox fan-out and the sender posting through the real outbound HTTP
//! adapter to a local receiver. The receiver is reached with an explicit test-only allow-list
//! `Outbound`; the product always passes `Outbound::without_allow_list()`.

#[path = "support/project_harness.rs"]
mod project_harness;

use std::ops::AsyncFnMut;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::http::{HeaderMap, StatusCode};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use fvoci_server::auth::password::Keyring;
use fvoci_server::db::context::set_system;
use fvoci_server::integrations::outbound::{Outbound, OutboundPolicy};
use fvoci_server::outbox::{spawn_outbox_dispatcher, OutboxConsumer, OutboxDispatcherSettings};
use fvoci_server::push::{
    ensure_vapid_keys, load_vapid_public_key, push_consumer, rotate_vapid_keys, spawn_push_sender,
    upsert_subscription, PushSenderHandle, PushSenderSettings, PUSH_SUBSCRIPTIONS_PER_USER,
};
use fvoci_server::secret_verify::verify_sealed_secrets;
use project_harness::{
    add_workspace_user, admin_pool, app_pool, json_request, setup_session, TestDb,
};
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;
use web_push_native::jwt_simple::algorithms::{ECDSAP256PublicKeyLike, ES256PublicKey};
use web_push_native::jwt_simple::prelude::{NoCustomClaims, VerificationOptions};
use web_push_native::p256::elliptic_curve::rand_core::OsRng;
use web_push_native::p256::{EncodedPoint, SecretKey};
use web_push_native::Auth;

const ENCRYPTION: &str =
    r#"{"k1":"1111111111111111111111111111111111111111111111111111111111111111"}"#;
const P256DH: &str =
    "BLn9b-VR0ca83knDNZ32dCHGyjJp-1riX9ZTN40MqV8K_LpQmLqxC_DoHvqvFXO_nGdAB4W9dogZb_sM-uV4JbY";
const AUTH: &str = "AAAAAAAAAAAAAAAAAAAAAA";
const ENDPOINT: &str = "https://push.example.com/delivery/tenant-a";
const SUBJECT: &str = "https://fvoci.example";

fn encryption_keys() -> Arc<Keyring> {
    Arc::new(Keyring::parse_named(ENCRYPTION, "k1", "ENCRYPTION_KEYS").expect("keys"))
}

fn body(endpoint: &str, p256dh: &str, auth: &str) -> Value {
    json!({ "endpoint": endpoint, "keys": { "p256dh": p256dh, "auth": auth } })
}

fn path(workspace_id: Uuid) -> String {
    format!("/api/v1/workspaces/{workspace_id}/push-subscriptions")
}

async fn rows_for(admin: &PgPool, user_id: Uuid) -> Vec<(String, String, String)> {
    sqlx::query_as(
        "SELECT endpoint, p256dh, auth FROM fvoci.push_subscriptions WHERE user_id = $1 ORDER BY endpoint",
    )
    .bind(user_id)
    .fetch_all(admin)
    .await
    .expect("rows")
}

async fn count_endpoint(admin: &PgPool, endpoint: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM fvoci.push_subscriptions WHERE endpoint = $1")
        .bind(endpoint)
        .fetch_one(admin)
        .await
        .expect("count")
}

#[tokio::test]
async fn put_subscription_is_session_only_member_scoped_and_capped() {
    let harness = TestDb::bootstrap().await;
    let (app, owner_cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let member = add_workspace_user(&admin, workspace_id, "member", "push-b").await;
    let guest = add_workspace_user(&admin, workspace_id, "guest", "push-guest").await;

    // Another workspace and its member: not a member of `acme`.
    let other_ws = Uuid::now_v7();
    sqlx::query("INSERT INTO fvoci.workspaces (id, slug, name) VALUES ($1, 'other', 'Other')")
        .bind(other_ws)
        .execute(&admin)
        .await
        .expect("other workspace");
    let outsider = add_workspace_user(&admin, other_ws, "owner", "push-outsider").await;

    // No session.
    let (status, _) = json_request(
        app.clone(),
        "PUT",
        &path(workspace_id),
        Some(body(ENDPOINT, P256DH, AUTH)),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // API tokens are refused (session-only route): 404 like other session routes.
    let (status, token) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/api-tokens"),
        Some(json!({"name": "push", "scopes": ["tasks.read"]})),
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{token:?}");
    let pat = token["token"].as_str().unwrap().to_string();
    let response = tower::ServiceExt::oneshot(
        app.clone(),
        axum::http::Request::builder()
            .method("PUT")
            .uri(path(workspace_id))
            .header("origin", "http://localhost")
            .header("authorization", format!("Bearer {pat}"))
            .header("content-type", "application/json")
            .extension(axum::extract::ConnectInfo(project_harness::test_peer()))
            .body(axum::body::Body::from(
                body(ENDPOINT, P256DH, AUTH).to_string(),
            ))
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    // Not a member / unknown workspace: 404 and nothing stored.
    let (status, _) = json_request(
        app.clone(),
        "PUT",
        &path(workspace_id),
        Some(body(ENDPOINT, P256DH, AUTH)),
        Some(&outsider.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = json_request(
        app.clone(),
        "PUT",
        &path(Uuid::now_v7()),
        Some(body(ENDPOINT, P256DH, AUTH)),
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(rows_for(&admin, outsider.user_id).await.is_empty());

    // Invalid bodies: 400.
    for bad in [
        body("http://push.example.com/x", P256DH, AUTH),
        body("https://127.0.0.1/x", P256DH, AUTH),
        body("https://localhost/x", P256DH, AUTH),
        body("https://push.example.com:8443/x", P256DH, AUTH),
        body(ENDPOINT, &P256DH[1..], AUTH),
        body(ENDPOINT, P256DH, "AAAAAAAAAAAAAAAAAAAAAv"),
        json!({ "endpoint": ENDPOINT, "expirationTime": null, "keys": { "p256dh": P256DH, "auth": AUTH } }),
        json!({ "endpoint": ENDPOINT }),
    ] {
        let (status, problem) = json_request(
            app.clone(),
            "PUT",
            &path(workspace_id),
            Some(bad.clone()),
            Some(&owner_cookie),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad} -> {problem:?}");
    }
    assert!(rows_for(&admin, owner_id).await.is_empty());

    // Guest and member may register; padding is stripped.
    let (status, ok) = json_request(
        app.clone(),
        "PUT",
        &path(workspace_id),
        Some(body(ENDPOINT, &format!("{P256DH}="), &format!("{AUTH}=="))),
        Some(&owner_cookie),
    )
    .await;
    assert_eq!((status, ok), (StatusCode::OK, json!({ "ok": true })));
    let (status, _) = json_request(
        app.clone(),
        "PUT",
        &path(workspace_id),
        Some(body("https://push.example.com/guest", P256DH, AUTH)),
        Some(&guest.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Same endpoint for another user: a separate row, no unique violation.
    let auth_b = "BBBBBBBBBBBBBBBBBBBBBA";
    let (status, _) = json_request(
        app.clone(),
        "PUT",
        &path(workspace_id),
        Some(body(ENDPOINT, P256DH, auth_b)),
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(count_endpoint(&admin, ENDPOINT).await, 2);

    // Repeated PUT by the owner refreshes the keys in place.
    let auth_a2 = "CCCCCCCCCCCCCCCCCCCCCA";
    let (status, _) = json_request(
        app.clone(),
        "PUT",
        &path(workspace_id),
        Some(body(ENDPOINT, P256DH, auth_a2)),
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        rows_for(&admin, owner_id).await,
        vec![(
            ENDPOINT.to_string(),
            P256DH.to_string(),
            auth_a2.to_string()
        )]
    );
    assert_eq!(rows_for(&admin, member.user_id).await[0].2, auth_b);

    // Per-user cap: the least recently updated rows go first.
    for n in 0..(PUSH_SUBSCRIPTIONS_PER_USER + 2) {
        let (status, _) = json_request(
            app.clone(),
            "PUT",
            &path(workspace_id),
            Some(body(
                &format!("https://push.example.com/dev/{n:02}"),
                P256DH,
                AUTH,
            )),
            Some(&owner_cookie),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }
    let owned = rows_for(&admin, owner_id).await;
    assert_eq!(owned.len() as i64, PUSH_SUBSCRIPTIONS_PER_USER);
    assert!(owned.iter().all(|row| row.0 != ENDPOINT
        && row.0 != "https://push.example.com/dev/00"
        && row.0 != "https://push.example.com/dev/01"));
    // The cap is per user: the member's row is untouched.
    assert_eq!(rows_for(&admin, member.user_id).await.len(), 1);

    // Removing the membership stops new registrations (checked in the write tx).
    sqlx::query("DELETE FROM fvoci.memberships WHERE workspace_id = $1 AND user_id = $2")
        .bind(workspace_id)
        .bind(member.user_id)
        .execute(&admin)
        .await
        .expect("remove member");
    let (status, _) = json_request(
        app.clone(),
        "PUT",
        &path(workspace_id),
        Some(body("https://push.example.com/after-removal", P256DH, AUTH)),
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(rows_for(&admin, member.user_id).await.len(), 1);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn vapid_bootstrap_rotation_and_secret_check() {
    let harness = TestDb::bootstrap().await;
    let (app, _cookie, owner_id, _workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let pool = app_pool(&harness).await;
    let keys = encryption_keys();

    let instance_key = |app: axum::Router| async move {
        let (status, instance) = json_request(app, "GET", "/api/v1/instance", None, None).await;
        assert_eq!(status, StatusCode::OK, "{instance:?}");
        instance["values"]["webPushPublicKey"].clone()
    };
    assert_eq!(instance_key(app.clone()).await, Value::Null);

    // Fail-open without a keyring: nothing stored, /instance stays null.
    assert!(ensure_vapid_keys(&pool, None).await.is_err());
    assert_eq!(load_vapid_public_key(&pool).await.unwrap(), None);

    // Replicas booting together converge on one keypair (first writer wins).
    let (a, b) = tokio::join!(
        ensure_vapid_keys(&pool, Some(&keys)),
        ensure_vapid_keys(&pool, Some(&keys))
    );
    a.expect("ensure a");
    b.expect("ensure b");
    let public = load_vapid_public_key(&pool)
        .await
        .unwrap()
        .expect("public key");
    assert_eq!(public.len(), 87);
    assert_eq!(URL_SAFE_NO_PAD.decode(&public).unwrap()[0], 4);
    ensure_vapid_keys(&pool, Some(&keys))
        .await
        .expect("idempotent");
    assert_eq!(
        load_vapid_public_key(&pool).await.unwrap().as_deref(),
        Some(public.as_str())
    );
    assert_eq!(instance_key(app.clone()).await, json!(public));
    let sealed: String =
        sqlx::query_scalar("SELECT vapid_private_key FROM fvoci.instance_config WHERE id = 1")
            .fetch_one(&admin)
            .await
            .unwrap();
    assert!(sealed.starts_with("enc:v2:k1:"), "sealed at rest");

    // The app role reads the private key only through the system-context definer.
    let denied = sqlx::query_scalar::<_, Option<String>>(
        "SELECT vapid_private_key FROM fvoci.instance_config WHERE id = 1",
    )
    .fetch_one(&pool)
    .await;
    assert!(denied.is_err(), "column grant must hide the sealed key");
    let denied = sqlx::query_scalar::<_, Option<String>>("SELECT fvoci.app_vapid_private_key()")
        .fetch_one(&pool)
        .await;
    assert!(denied.is_err(), "definer requires system context");
    let denied = sqlx::query("SELECT fvoci.app_set_vapid('x', 'enc:v2:k1:x')")
        .execute(&pool)
        .await;
    assert!(denied.is_err(), "set requires system context");

    // --verify-secrets covers vapid:1.
    let report = verify_sealed_secrets(&pool, Some(&keys)).await.unwrap();
    assert!(report.is_complete(), "{report:?}");
    assert_eq!(report.vapid.checked, 1);
    let other = Keyring::parse_named(
        &format!(r#"{{"k1":"{}"}}"#, "2".repeat(64)),
        "k1",
        "ENCRYPTION_KEYS",
    )
    .unwrap();
    let report = verify_sealed_secrets(&pool, Some(&other)).await.unwrap();
    assert_eq!(report.vapid.invalid, vec![Uuid::nil()]);
    let out = tokio::process::Command::new(env!("CARGO_BIN_EXE_fvoci-migrate"))
        .arg("--verify-secrets")
        .env_clear()
        .env("DATABASE_APP_URL", &harness.app_url)
        .env("ENCRYPTION_KEYS", ENCRYPTION)
        .env("ENCRYPTION_ACTIVE_KEY_ID", "k1")
        .output()
        .await
        .expect("run verify-secrets");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["vapid"]["checked"], 1);

    // Rotation: new pair, all rows wiped, event + audit, one transaction.
    let other_user = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO fvoci.users (id, email, given_name) VALUES ($1, 'rot@example.com', 'Rot')",
    )
    .bind(other_user)
    .execute(&admin)
    .await
    .unwrap();
    let mut tx = pool.begin().await.unwrap();
    set_system(&mut tx).await.unwrap();
    upsert_subscription(&mut tx, owner_id, None, ENDPOINT, P256DH, AUTH)
        .await
        .unwrap();
    upsert_subscription(&mut tx, other_user, None, ENDPOINT, P256DH, AUTH)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // A failed rotation (event insert rejected) changes nothing.
    sqlx::query(
        "CREATE FUNCTION fvoci.test_reject_vapid_event() RETURNS trigger LANGUAGE plpgsql AS $$ \
         BEGIN IF NEW.verb = 'instance.vapid_rotated' THEN RAISE EXCEPTION 'injected'; END IF; RETURN NEW; END $$",
    )
    .execute(&admin)
    .await
    .unwrap();
    sqlx::query("CREATE TRIGGER test_reject_vapid_event BEFORE INSERT ON fvoci.events FOR EACH ROW EXECUTE FUNCTION fvoci.test_reject_vapid_event()")
        .execute(&admin)
        .await
        .unwrap();
    assert!(rotate_vapid_keys(&pool, &keys).await.is_err());
    assert_eq!(
        load_vapid_public_key(&pool).await.unwrap().as_deref(),
        Some(public.as_str())
    );
    assert_eq!(count_endpoint(&admin, ENDPOINT).await, 2);
    sqlx::query("DROP TRIGGER test_reject_vapid_event ON fvoci.events")
        .execute(&admin)
        .await
        .unwrap();

    let rotated = rotate_vapid_keys(&pool, &keys).await.expect("rotate");
    assert_eq!(rotated.revoked_subscriptions, 2);
    assert_ne!(rotated.public_key, public);
    assert_eq!(count_endpoint(&admin, ENDPOINT).await, 0);
    assert_eq!(instance_key(app.clone()).await, json!(rotated.public_key));
    let events: Vec<(Option<Uuid>, Option<Uuid>, String, Value)> = sqlx::query_as(
        "SELECT workspace_id, actor_user_id, channel, payload FROM fvoci.events WHERE verb = 'instance.vapid_rotated'",
    )
    .fetch_all(&admin)
    .await
    .unwrap();
    assert_eq!(
        events,
        vec![(
            None,
            None,
            "system".to_string(),
            json!({ "revokedSubscriptions": 2 })
        )]
    );
    let audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM fvoci.audit_log WHERE verb = 'instance.vapid_rotated' AND actor_user_id IS NULL",
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(audits, 1);

    // The CLI path (app role, server env), public output only.
    let out = tokio::process::Command::new(env!("CARGO_BIN_EXE_fvoci-migrate"))
        .arg("--rotate-vapid")
        .env_clear()
        .env("DATABASE_APP_URL", &harness.app_url)
        .env("ENCRYPTION_KEYS", ENCRYPTION)
        .env("ENCRYPTION_ACTIVE_KEY_ID", "k1")
        .output()
        .await
        .expect("run rotate-vapid");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let printed: Value = serde_json::from_slice(&out.stdout).unwrap();
    let cli_key = printed["publicKey"].as_str().unwrap().to_string();
    assert_eq!(
        printed,
        json!({ "publicKey": cli_key, "revokedSubscriptions": 0 })
    );
    assert_eq!(load_vapid_public_key(&pool).await.unwrap(), Some(cli_key));
    let out = tokio::process::Command::new(env!("CARGO_BIN_EXE_fvoci-migrate"))
        .arg("--rotate-vapid")
        .env_clear()
        .env("DATABASE_APP_URL", &harness.app_url)
        .output()
        .await
        .expect("run rotate-vapid without keys");
    assert!(!out.status.success(), "rotation needs ENCRYPTION_KEYS");

    admin.close().await;
    harness.cleanup().await;
}

// ---------------------------------------------------------------- delivery

#[derive(Clone, Debug)]
struct Received {
    path: String,
    headers: HeaderMap,
    body: Vec<u8>,
}

/// Local push-service stand-in on an ephemeral port. The last path segment
/// picks the answer; `slow*` answers after 1.5 s, `stall*` after 3 s. A
/// request is recorded when it arrives.
async fn start_receiver() -> (String, Arc<Mutex<Vec<Received>>>) {
    let received = Arc::new(Mutex::new(Vec::<Received>::new()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let log = received.clone();
    let location = format!("{base}/redirected");
    let app = axum::Router::new().fallback(
        move |uri: axum::http::Uri, headers: HeaderMap, bytes: axum::body::Bytes| {
            let log = log.clone();
            let location = location.clone();
            async move {
                let path = uri.path().to_string();
                log.lock().unwrap().push(Received {
                    path: path.clone(),
                    headers,
                    body: bytes.to_vec(),
                });
                let last = path.rsplit('/').next().unwrap_or("").to_string();
                if last.starts_with("slow") {
                    tokio::time::sleep(Duration::from_millis(1500)).await;
                } else if last.starts_with("stall") {
                    tokio::time::sleep(Duration::from_secs(3)).await;
                }
                let status = match last.as_str() {
                    "gone" => StatusCode::GONE,
                    "missing" => StatusCode::NOT_FOUND,
                    "error" => StatusCode::INTERNAL_SERVER_ERROR,
                    "stale" => StatusCode::FORBIDDEN,
                    "redirect" => StatusCode::FOUND,
                    _ => StatusCode::CREATED,
                };
                let mut response = axum::response::Response::new(axum::body::Body::empty());
                *response.status_mut() = status;
                if status == StatusCode::FOUND {
                    response
                        .headers_mut()
                        .insert("location", location.parse().unwrap());
                }
                response
            }
        },
    );
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (base, received)
}

fn received_count(received: &Arc<Mutex<Vec<Received>>>, path: &str) -> usize {
    received
        .lock()
        .unwrap()
        .iter()
        .filter(|r| r.path == path)
        .count()
}

struct Device {
    secret: SecretKey,
    auth: [u8; 16],
}

impl Device {
    fn new(seed: u8) -> Self {
        Self {
            secret: SecretKey::random(&mut OsRng),
            auth: [seed; 16],
        }
    }

    fn p256dh(&self) -> String {
        URL_SAFE_NO_PAD.encode(EncodedPoint::from(self.secret.public_key()).as_bytes())
    }

    fn auth(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.auth)
    }

    fn open(&self, body: &[u8]) -> Value {
        let plain = web_push_native::decrypt(
            body.to_vec(),
            &self.secret,
            &Auth::clone_from_slice(&self.auth),
        )
        .expect("decrypt push body");
        serde_json::from_slice(&plain).expect("payload json")
    }
}

/// Stores a subscription directly (the loopback receiver is not an https
/// endpoint the PUT route accepts), optionally bound to a session.
async fn store_bound(
    pool: &PgPool,
    user_id: Uuid,
    session_id: Option<Uuid>,
    endpoint: &str,
    device: &Device,
) {
    let mut tx = pool.begin().await.unwrap();
    set_system(&mut tx).await.unwrap();
    upsert_subscription(
        &mut tx,
        user_id,
        session_id,
        endpoint,
        &device.p256dh(),
        &device.auth(),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
}

async fn store(pool: &PgPool, user_id: Uuid, endpoint: &str, device: &Device) {
    store_bound(pool, user_id, None, endpoint, device).await;
}

fn test_outbound() -> Outbound {
    // Test-only: allow the loopback receiver. Product: without_allow_list().
    Outbound::system(OutboundPolicy::parse_allow_list("127.0.0.1").unwrap())
}

fn sender_settings(batch: i64, request_timeout: Duration) -> PushSenderSettings {
    PushSenderSettings {
        batch,
        request_timeout,
        claim_lease: Duration::from_secs(2),
        poll_interval: Duration::from_millis(50),
        ..PushSenderSettings::default()
    }
}

/// The server's pipeline: outbox dispatcher (push fan-out plus `extra`
/// consumers) and the push sender, or no sender to inspect the ledger.
struct Pipeline {
    dispatcher: fvoci_server::outbox::OutboxDispatcherHandle,
    sender: Option<PushSenderHandle>,
}

impl Pipeline {
    fn start(
        pool: &PgPool,
        sender: Option<(Outbound, Option<Arc<Keyring>>, PushSenderSettings)>,
        extra: Vec<Arc<dyn OutboxConsumer>>,
    ) -> Self {
        let sender = sender.map(|(outbound, keys, settings)| {
            spawn_push_sender(pool.clone(), outbound, keys, SUBJECT.into(), settings)
        });
        let mut consumers = extra;
        consumers.push(push_consumer(sender.as_ref().map(|s| s.wake.clone())));
        let dispatcher = spawn_outbox_dispatcher(
            OutboxDispatcherSettings {
                poll_interval: Duration::from_millis(50),
                ..OutboxDispatcherSettings::default()
            },
            pool.clone(),
            consumers,
        )
        .expect("dispatcher");
        Self { dispatcher, sender }
    }

    async fn stop(self) {
        self.dispatcher.request_shutdown();
        self.dispatcher.join().await.expect("dispatcher join");
        if let Some(sender) = self.sender {
            sender.request_shutdown();
            sender.join().await.expect("sender join");
        }
    }
}

fn start_sender(
    pool: &PgPool,
    keys: Arc<Keyring>,
    settings: PushSenderSettings,
) -> PushSenderHandle {
    spawn_push_sender(
        pool.clone(),
        test_outbound(),
        Some(keys),
        SUBJECT.into(),
        settings,
    )
}

async fn stop_sender(sender: PushSenderHandle) {
    sender.request_shutdown();
    sender.join().await.expect("sender join");
}

async fn wait_until(what: &str, mut check: impl AsyncFnMut() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(40);
    while !check().await {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn wait_processed(admin: &PgPool, consumer: &str, event_id: Uuid) {
    wait_until(&format!("{consumer} to process {event_id}"), async || {
        sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM fvoci.processed_events WHERE consumer = $1 AND event_id = $2)",
        )
        .bind(consumer)
        .bind(event_id)
        .fetch_one(admin)
        .await
        .unwrap()
    })
    .await;
}

async fn ledger_len(admin: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM fvoci.push_deliveries")
        .fetch_one(admin)
        .await
        .unwrap()
}

async fn wait_ledger_empty(admin: &PgPool) {
    wait_until("the push ledger to drain", async || {
        ledger_len(admin).await == 0
    })
    .await;
}

async fn logout(app: &axum::Router, cookie: &str, body: Option<Value>) -> StatusCode {
    json_request(
        app.clone(),
        "POST",
        "/api/v1/auth/logout",
        body,
        Some(cookie),
    )
    .await
    .0
}

/// Another live session for `user_id` (another browser of the same user).
async fn extra_session(admin: &PgPool, user_id: Uuid) -> (String, Uuid) {
    let token = fvoci_server::auth::token::new_token();
    let session_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO fvoci.sessions (id, user_id, token_hash, expires_at) VALUES ($1, $2, $3, now() + interval '1 hour')",
    )
    .bind(session_id)
    .bind(user_id)
    .bind(&token.hash)
    .execute(admin)
    .await
    .expect("insert session");
    (token.token, session_id)
}

/// Owner comments on a workspace document mentioning `mentioned`; returns the
/// `comment.created` event id and the document display id.
async fn comment_event(
    app: &axum::Router,
    admin: &PgPool,
    workspace_id: Uuid,
    owner_cookie: &str,
    mentioned: &[Uuid],
) -> (Uuid, String) {
    let (status, doc) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents"),
        Some(json!({"parentId": null, "title": "푸시 문서"})),
        Some(owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{doc:?}");
    let document_id = doc["id"].as_str().unwrap().to_string();
    let (status, comment) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{document_id}/comments"),
        Some(json!({ "body": "확인 부탁", "mentionedUserIds": mentioned })),
        Some(owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{comment:?}");
    let event_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM fvoci.events WHERE verb = 'comment.created' AND payload->>'commentId' = $1",
    )
    .bind(comment["id"].as_str().unwrap())
    .fetch_one(admin)
    .await
    .expect("comment event");
    let number: i32 = sqlx::query_scalar("SELECT number FROM fvoci.documents WHERE id = $1::uuid")
        .bind(&document_id)
        .fetch_one(admin)
        .await
        .unwrap();
    (event_id, format!("WIKI-{number}"))
}

#[tokio::test]
async fn push_pipeline_sends_through_outbound_and_cleans_gone_endpoints() {
    let harness = TestDb::bootstrap().await;
    let (app, owner_cookie, _owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let pool = app_pool(&harness).await;
    let keys = encryption_keys();
    let member = add_workspace_user(&admin, workspace_id, "member", "push-member").await;
    let quiet = add_workspace_user(&admin, workspace_id, "member", "push-quiet").await;
    let bystander = add_workspace_user(&admin, workspace_id, "member", "push-bystander").await;
    let (base, received) = start_receiver().await;

    // `quiet` keeps the digest but turned in-app (and therefore push) off.
    let (status, _) = json_request(
        app.clone(),
        "PUT",
        &format!("/api/v1/workspaces/{workspace_id}/notification-prefs"),
        Some(json!({ "inApp": false, "mailImmediate": false, "mailDigest": true })),
        Some(&quiet.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let ok = Device::new(1);
    let other = Device::new(2);
    for (user, name, device) in [
        (member.user_id, "ok", &ok),
        (member.user_id, "gone", &other),
        (member.user_id, "missing", &other),
        (member.user_id, "error", &other),
        (member.user_id, "stale", &other),
        (member.user_id, "redirect", &other),
        (quiet.user_id, "quiet", &other),
        // Same gone endpoint held by a user this event does not target.
        (bystander.user_id, "gone", &other),
    ] {
        store(&pool, user, &format!("{base}/push/{name}"), device).await;
    }
    let settings = sender_settings(8, Duration::from_secs(5));

    // 1. No VAPID keypair yet: nothing is queued or sent, the event is marked.
    let (first_event, _) = comment_event(
        &app,
        &admin,
        workspace_id,
        &owner_cookie,
        &[member.user_id, quiet.user_id],
    )
    .await;
    let pipeline = Pipeline::start(
        &pool,
        Some((test_outbound(), Some(keys.clone()), settings.clone())),
        Vec::new(),
    );
    wait_processed(&admin, "push", first_event).await;
    pipeline.stop().await;
    assert_eq!(ledger_len(&admin).await, 0);
    assert!(received.lock().unwrap().is_empty(), "no keys, no sends");

    // 2. With keys: one POST per subscription, independent of a failing mailer.
    ensure_vapid_keys(&pool, Some(&keys)).await.unwrap();
    let public = load_vapid_public_key(&pool).await.unwrap().unwrap();
    let (event_id, display_id) = comment_event(
        &app,
        &admin,
        workspace_id,
        &owner_cookie,
        &[member.user_id, quiet.user_id],
    )
    .await;
    let failing_mail = Arc::new(fvoci_server::mail::Mailer::from_smtp(Some(
        fvoci_server::mail::SmtpConfig {
            host: "127.0.0.1".into(),
            port: 1,
            from: "fvoci@example.com".into(),
        },
    )));
    let pipeline = Pipeline::start(
        &pool,
        Some((test_outbound(), Some(keys.clone()), settings.clone())),
        vec![fvoci_server::mail::mail_consumer(failing_mail)],
    );
    wait_processed(&admin, "push", event_id).await;
    wait_ledger_empty(&admin).await;
    pipeline.stop().await;

    let requests = received.lock().unwrap().clone();
    let mut paths: Vec<&str> = requests.iter().map(|r| r.path.as_str()).collect();
    paths.sort_unstable();
    assert_eq!(
        paths,
        vec![
            "/push/error",
            "/push/gone",
            "/push/missing",
            "/push/ok",
            "/push/redirect",
            "/push/stale"
        ],
        "quiet (inApp off) is skipped and the redirect is not followed"
    );
    let vapid_key = ES256PublicKey::from_bytes(&URL_SAFE_NO_PAD.decode(&public).unwrap()).unwrap();
    for request in &requests {
        let header = |name: &str| {
            request
                .headers
                .get(name)
                .unwrap()
                .to_str()
                .unwrap()
                .to_string()
        };
        assert_eq!(header("ttl"), "86400");
        assert_eq!(header("content-encoding"), "aes128gcm");
        assert_eq!(header("content-type"), "application/octet-stream");
        let authz = header("authorization");
        let (token, k) = authz
            .strip_prefix("vapid t=")
            .and_then(|rest| rest.split_once(", k="))
            .expect("vapid header");
        assert_eq!(k, public);
        let claims = vapid_key
            .verify_token::<NoCustomClaims>(token, Some(VerificationOptions::default()))
            .expect("vapid jwt verifies with the instance key");
        assert_eq!(
            claims.audiences.unwrap().into_string().unwrap(),
            base,
            "aud keeps the port"
        );
        assert_eq!(claims.subject.as_deref(), Some(SUBJECT));
    }
    let delivered = requests.iter().find(|r| r.path == "/push/ok").unwrap();
    assert_eq!(
        ok.open(&delivered.body),
        json!({
            "title": "Owner",
            "body": "문서에 새 댓글이 달렸습니다",
            "url": format!("/w/acme/{display_id}"),
        })
    );

    // 404/410 delete the endpoint for every user; other failures keep the row.
    let remaining: Vec<String> =
        sqlx::query_scalar("SELECT endpoint FROM fvoci.push_subscriptions ORDER BY endpoint")
            .fetch_all(&admin)
            .await
            .unwrap();
    let expected: Vec<String> = ["error", "ok", "quiet", "redirect", "stale"]
        .iter()
        .map(|name| format!("{base}/push/{name}"))
        .collect();
    assert_eq!(remaining, expected);

    // Mail failed and will retry on its own key; push is done and not re-sent.
    let mail_done: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM fvoci.processed_events WHERE consumer = 'mail' AND event_id = $1)",
    )
    .bind(event_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert!(!mail_done, "the mail send failed");
    let mail_failures: i64 =
        sqlx::query_scalar("SELECT count(*) FROM fvoci.outbox_failures WHERE consumer = 'mail'")
            .fetch_one(&admin)
            .await
            .unwrap();
    assert!(
        mail_failures >= 1,
        "the mail consumer is failing on its own key"
    );
    let push_failures: i64 =
        sqlx::query_scalar("SELECT count(*) FROM fvoci.outbox_failures WHERE consumer = 'push'")
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(push_failures, 0);

    // Replay (cursor rebased as `--recover-outbox` does): no second fan-out.
    sqlx::query("UPDATE fvoci.outbox_consumers SET last_xact = '0'::xid8, last_seq = 0 WHERE consumer = 'push'")
        .execute(&admin)
        .await
        .unwrap();
    let before = received.lock().unwrap().len();
    let pipeline = Pipeline::start(
        &pool,
        Some((test_outbound(), Some(keys.clone()), settings)),
        Vec::new(),
    );
    wait_until("the push cursor to catch up", async || {
        sqlx::query_scalar(
            "SELECT (c.last_xact, c.last_seq) >= (e.xact, e.seq) FROM fvoci.outbox_consumers c, fvoci.events e \
             WHERE c.consumer = 'push' AND e.id = $1",
        )
        .bind(event_id)
        .fetch_one(&admin)
        .await
        .unwrap()
    })
    .await;
    pipeline.stop().await;
    assert_eq!(ledger_len(&admin).await, 0);
    assert_eq!(
        received.lock().unwrap().len(),
        before,
        "replayed events are not re-pushed"
    );

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn product_outbound_refuses_loopback_push_endpoints() {
    let harness = TestDb::bootstrap().await;
    let (app, owner_cookie, _owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let pool = app_pool(&harness).await;
    let keys = encryption_keys();
    let member = add_workspace_user(&admin, workspace_id, "member", "push-ssrf").await;
    let (base, received) = start_receiver().await;
    let device = Device::new(3);
    // A row that bypassed the https PUT check (e.g. restored data).
    store(&pool, member.user_id, &format!("{base}/push/ok"), &device).await;
    ensure_vapid_keys(&pool, Some(&keys)).await.unwrap();
    let (event_id, _) =
        comment_event(&app, &admin, workspace_id, &owner_cookie, &[member.user_id]).await;
    let product = Outbound::system(OutboundPolicy::default()).without_allow_list();
    let pipeline = Pipeline::start(
        &pool,
        Some((
            product,
            Some(keys),
            sender_settings(8, Duration::from_secs(5)),
        )),
        Vec::new(),
    );
    wait_processed(&admin, "push", event_id).await;
    wait_ledger_empty(&admin).await;
    pipeline.stop().await;
    assert!(
        received.lock().unwrap().is_empty(),
        "strict policy never reaches loopback"
    );
    assert_eq!(
        count_endpoint(&admin, &format!("{base}/push/ok")).await,
        1,
        "not treated as gone"
    );
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn logout_disconnects_only_this_browser() {
    let harness = TestDb::bootstrap().await;
    let (app, owner_cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let member = add_workspace_user(&admin, workspace_id, "member", "push-shared").await;
    let (second_cookie, _) = extra_session(&admin, owner_id).await;
    let put = |cookie: String, endpoint: &'static str| {
        let app = app.clone();
        async move {
            let (status, _) = json_request(
                app,
                "PUT",
                &path(workspace_id),
                Some(body(endpoint, P256DH, AUTH)),
                Some(&cookie),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
        }
    };
    const THIS: &str = "https://push.example.com/this-browser";
    const OTHER: &str = "https://push.example.com/other-device";
    const REPORTED: &str = "https://push.example.com/reported";
    // Owner: this browser under the first session, another device and the
    // reported endpoint under the second. The member shares two endpoints.
    put(owner_cookie.clone(), THIS).await;
    put(second_cookie.clone(), OTHER).await;
    put(second_cookie.clone(), REPORTED).await;
    put(member.cookie.clone(), THIS).await;
    put(member.cookie.clone(), REPORTED).await;

    assert_eq!(
        logout(
            &app,
            &owner_cookie,
            Some(json!({ "pushEndpoint": REPORTED }))
        )
        .await,
        StatusCode::NO_CONTENT
    );
    let owner_rows: Vec<String> = rows_for(&admin, owner_id)
        .await
        .into_iter()
        .map(|row| row.0)
        .collect();
    assert_eq!(
        owner_rows,
        vec![OTHER.to_string()],
        "session row and reported endpoint go"
    );
    assert_eq!(
        rows_for(&admin, member.user_id).await.len(),
        2,
        "another account's rows stay"
    );

    // A malformed body never blocks logout; the session-bound row still goes.
    let (status, _, _) = project_harness::http_request(
        app.clone(),
        "POST",
        "/api/v1/auth/logout",
        Some(b"{not json".to_vec()),
        Some("application/json"),
        Some(&second_cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(rows_for(&admin, owner_id).await.is_empty());
    assert_eq!(rows_for(&admin, member.user_id).await.len(), 2);

    // Logout without a live session touches nothing.
    assert_eq!(
        logout(&app, "not-a-session", Some(json!({ "pushEndpoint": THIS }))).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(rows_for(&admin, member.user_id).await.len(), 2);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn sender_rechecks_recipients_right_before_sending() {
    let harness = TestDb::bootstrap().await;
    let (app, owner_cookie, _owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let pool = app_pool(&harness).await;
    let keys = encryption_keys();
    ensure_vapid_keys(&pool, Some(&keys)).await.unwrap();
    let logged_out = add_workspace_user(&admin, workspace_id, "member", "push-logout").await;
    let suspended = add_workspace_user(&admin, workspace_id, "member", "push-suspended").await;
    let removed = add_workspace_user(&admin, workspace_id, "member", "push-removed").await;
    let kept = add_workspace_user(&admin, workspace_id, "member", "push-kept").await;
    let (base, received) = start_receiver().await;
    let device = Device::new(4);
    for (user, name) in [
        (&logged_out, "logged-out"),
        (&suspended, "suspended"),
        (&removed, "removed"),
        (&kept, "kept"),
    ] {
        store(&pool, user.user_id, &format!("{base}/push/{name}"), &device).await;
    }
    let (event_id, _) = comment_event(
        &app,
        &admin,
        workspace_id,
        &owner_cookie,
        &[
            logged_out.user_id,
            suspended.user_id,
            removed.user_id,
            kept.user_id,
        ],
    )
    .await;

    // Fan out only: the ledger holds one row per recipient device.
    let pipeline = Pipeline::start(&pool, None, Vec::new());
    wait_processed(&admin, "push", event_id).await;
    pipeline.stop().await;
    assert_eq!(ledger_len(&admin).await, 4);

    // Revocations committed after the fan-out and before the send.
    assert_eq!(
        logout(
            &app,
            &logged_out.cookie,
            Some(json!({ "pushEndpoint": format!("{base}/push/logged-out") }))
        )
        .await,
        StatusCode::NO_CONTENT
    );
    sqlx::query("UPDATE fvoci.users SET suspended_at = now() WHERE id = $1")
        .bind(suspended.user_id)
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query("DELETE FROM fvoci.memberships WHERE workspace_id = $1 AND user_id = $2")
        .bind(workspace_id)
        .bind(removed.user_id)
        .execute(&admin)
        .await
        .unwrap();

    let sender = start_sender(&pool, keys, sender_settings(8, Duration::from_secs(5)));
    wait_ledger_empty(&admin).await;
    stop_sender(sender).await;
    let paths: Vec<String> = received
        .lock()
        .unwrap()
        .iter()
        .map(|r| r.path.clone())
        .collect();
    assert_eq!(paths, vec!["/push/kept".to_string()]);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn logout_waits_for_an_in_flight_send_and_blocks_later_ones() {
    let harness = TestDb::bootstrap().await;
    let (app, owner_cookie, _owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let pool = app_pool(&harness).await;
    let keys = encryption_keys();
    ensure_vapid_keys(&pool, Some(&keys)).await.unwrap();
    let alice = add_workspace_user(&admin, workspace_id, "member", "push-alice").await;
    let bob = add_workspace_user(&admin, workspace_id, "member", "push-bob").await;
    let alice_session = project_harness::session_id_for_user(&admin, alice.user_id).await;
    let bob_session = project_harness::session_id_for_user(&admin, bob.user_id).await;
    let (base, received) = start_receiver().await;
    let device = Device::new(5);
    // Alice: another device first (slow), then this browser (bound to the
    // session that logs out). Bob: only this browser, slow.
    store(
        &pool,
        alice.user_id,
        &format!("{base}/push/slow-alice-other"),
        &device,
    )
    .await;
    store_bound(
        &pool,
        alice.user_id,
        Some(alice_session),
        &format!("{base}/push/alice-this"),
        &device,
    )
    .await;
    store_bound(
        &pool,
        bob.user_id,
        Some(bob_session),
        &format!("{base}/push/slow-bob-this"),
        &device,
    )
    .await;
    let (event_id, _) = comment_event(
        &app,
        &admin,
        workspace_id,
        &owner_cookie,
        &[alice.user_id, bob.user_id],
    )
    .await;
    let pipeline = Pipeline::start(&pool, None, Vec::new());
    wait_processed(&admin, "push", event_id).await;
    pipeline.stop().await;
    assert_eq!(ledger_len(&admin).await, 3);

    // One request at a time, in ledger order (Alice's rows, then Bob's).
    let sender = start_sender(&pool, keys, sender_settings(1, Duration::from_secs(5)));
    wait_until("alice's other device to be in flight", async || {
        received_count(&received, "/push/slow-alice-other") == 1
    })
    .await;
    // This browser's row is not the one in flight: logout returns at once and
    // the queued send for it never starts.
    assert_eq!(
        logout(&app, &alice.cookie, None).await,
        StatusCode::NO_CONTENT
    );

    wait_until("bob's browser to be in flight", async || {
        received_count(&received, "/push/slow-bob-this") == 1
    })
    .await;
    // The row being sent is locked: Bob's logout waits for that attempt.
    let started = std::time::Instant::now();
    assert_eq!(
        logout(&app, &bob.cookie, None).await,
        StatusCode::NO_CONTENT
    );
    assert!(
        started.elapsed() >= Duration::from_millis(700),
        "logout waited for the in-flight send ({:?})",
        started.elapsed()
    );
    wait_ledger_empty(&admin).await;
    stop_sender(sender).await;

    assert_eq!(
        received_count(&received, "/push/alice-this"),
        0,
        "no send after logout"
    );
    assert_eq!(received_count(&received, "/push/slow-alice-other"), 1);
    assert_eq!(
        received_count(&received, "/push/slow-bob-this"),
        1,
        "sent once, not repeated"
    );
    assert_eq!(
        rows_for(&admin, alice.user_id).await.len(),
        1,
        "other device stays"
    );
    assert!(rows_for(&admin, bob.user_id).await.is_empty());

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn stalled_endpoints_delay_but_never_drop_later_recipients() {
    let harness = TestDb::bootstrap().await;
    let (app, owner_cookie, _owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let pool = app_pool(&harness).await;
    let keys = encryption_keys();
    ensure_vapid_keys(&pool, Some(&keys)).await.unwrap();
    // Created first, so its 9 stalled devices come first in the ledger.
    let staller = add_workspace_user(&admin, workspace_id, "member", "push-staller").await;
    let healthy = add_workspace_user(&admin, workspace_id, "member", "push-healthy").await;
    let (base, received) = start_receiver().await;
    let device = Device::new(6);
    for n in 0..9 {
        store(
            &pool,
            staller.user_id,
            &format!("{base}/push/stall-{n}"),
            &device,
        )
        .await;
    }
    store(
        &pool,
        healthy.user_id,
        &format!("{base}/push/healthy"),
        &device,
    )
    .await;
    let (event_id, _) = comment_event(
        &app,
        &admin,
        workspace_id,
        &owner_cookie,
        &[staller.user_id, healthy.user_id],
    )
    .await;
    let pipeline = Pipeline::start(
        &pool,
        Some((
            test_outbound(),
            Some(keys),
            sender_settings(8, Duration::from_secs(1)),
        )),
        Vec::new(),
    );
    wait_processed(&admin, "push", event_id).await;
    wait_ledger_empty(&admin).await;
    pipeline.stop().await;
    assert_eq!(received_count(&received, "/push/healthy"), 1);
    let stalled = received
        .lock()
        .unwrap()
        .iter()
        .filter(|r| r.path.starts_with("/push/stall-"))
        .count();
    assert_eq!(stalled, 9, "every device is attempted once");
    assert_eq!(
        rows_for(&admin, staller.user_id).await.len(),
        9,
        "timeouts keep the row"
    );

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn record_failure_resends_only_that_batch() {
    let harness = TestDb::bootstrap().await;
    let (app, owner_cookie, _owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let pool = app_pool(&harness).await;
    let keys = encryption_keys();
    ensure_vapid_keys(&pool, Some(&keys)).await.unwrap();
    let member = add_workspace_user(&admin, workspace_id, "member", "push-flaky").await;
    let (base, received) = start_receiver().await;
    let device = Device::new(7);
    for name in ["first", "flaky", "last"] {
        store(
            &pool,
            member.user_id,
            &format!("{base}/push/{name}"),
            &device,
        )
        .await;
    }
    // The first attempt to record the `flaky` send fails (the sequence is not
    // rolled back with the failed transaction).
    sqlx::raw_sql(
        r#"
        CREATE SEQUENCE fvoci.test_push_record_fail;
        CREATE FUNCTION fvoci.test_push_record_fail() RETURNS trigger
        LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
        BEGIN
            IF OLD.endpoint LIKE '%/flaky' AND nextval('fvoci.test_push_record_fail') = 1 THEN
                RAISE EXCEPTION 'injected record failure';
            END IF;
            RETURN OLD;
        END $$;
        CREATE TRIGGER test_push_record_fail BEFORE DELETE ON fvoci.push_deliveries
            FOR EACH ROW EXECUTE FUNCTION fvoci.test_push_record_fail();
        "#,
    )
    .execute(&admin)
    .await
    .unwrap();
    let (event_id, _) =
        comment_event(&app, &admin, workspace_id, &owner_cookie, &[member.user_id]).await;
    let pipeline = Pipeline::start(
        &pool,
        Some((
            test_outbound(),
            Some(keys),
            sender_settings(1, Duration::from_secs(5)),
        )),
        Vec::new(),
    );
    wait_processed(&admin, "push", event_id).await;
    wait_ledger_empty(&admin).await;
    pipeline.stop().await;
    assert_eq!(received_count(&received, "/push/first"), 1);
    assert_eq!(
        received_count(&received, "/push/flaky"),
        2,
        "the unrecorded attempt is sent again after the claim lease (at least once)"
    );
    assert_eq!(
        received_count(&received, "/push/last"),
        1,
        "no replay of the whole event"
    );

    admin.close().await;
    harness.cleanup().await;
}
