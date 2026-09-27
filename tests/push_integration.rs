#![cfg(feature = "db-tests")]
#![allow(dead_code)]

//! Web Push against a real PostgreSQL with the app role: subscription PUT
//! contract, VAPID bootstrap/rotation/backup check, and the `push` outbox
//! consumer sending through the real outbound HTTP adapter to a local
//! receiver. The receiver is reached with an explicit test-only allow-list
//! `Outbound`; the product always passes `Outbound::without_allow_list()`.

#[path = "support/project_harness.rs"]
mod project_harness;

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
    ensure_vapid_keys, load_vapid_public_key, push_consumer, rotate_vapid_keys,
    upsert_subscription, PUSH_SUBSCRIPTIONS_PER_USER,
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
    upsert_subscription(&mut tx, owner_id, ENDPOINT, P256DH, AUTH)
        .await
        .unwrap();
    upsert_subscription(&mut tx, other_user, ENDPOINT, P256DH, AUTH)
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

/// Local push-service stand-in on an ephemeral port: the status comes from
/// the last path segment.
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
                let status = match path.rsplit('/').next().unwrap_or("") {
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

async fn store(pool: &PgPool, user_id: Uuid, endpoint: &str, device: &Device) {
    let mut tx = pool.begin().await.unwrap();
    set_system(&mut tx).await.unwrap();
    upsert_subscription(&mut tx, user_id, endpoint, &device.p256dh(), &device.auth())
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

fn test_outbound() -> Outbound {
    // Test-only: allow the loopback receiver. Product: without_allow_list().
    Outbound::system(OutboundPolicy::parse_allow_list("127.0.0.1").unwrap())
}

fn dispatcher(
    pool: &PgPool,
    consumers: Vec<Arc<dyn OutboxConsumer>>,
) -> fvoci_server::outbox::OutboxDispatcherHandle {
    spawn_outbox_dispatcher(
        OutboxDispatcherSettings {
            poll_interval: Duration::from_millis(50),
            ..OutboxDispatcherSettings::default()
        },
        pool.clone(),
        consumers,
    )
    .expect("dispatcher")
}

async fn stop(handle: fvoci_server::outbox::OutboxDispatcherHandle) {
    handle.request_shutdown();
    handle.join().await.expect("dispatcher join");
}

async fn wait_processed(admin: &PgPool, consumer: &str, event_id: Uuid) {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let done: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM fvoci.processed_events WHERE consumer = $1 AND event_id = $2)",
        )
        .bind(consumer)
        .bind(event_id)
        .fetch_one(admin)
        .await
        .unwrap();
        if done {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "{consumer} never processed {event_id}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
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
async fn push_consumer_sends_through_outbound_and_cleans_gone_endpoints() {
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

    // 1. No VAPID keypair yet: nothing is sent, the event is still marked.
    let (first_event, _) = comment_event(
        &app,
        &admin,
        workspace_id,
        &owner_cookie,
        &[member.user_id, quiet.user_id],
    )
    .await;
    let handle = dispatcher(
        &pool,
        vec![push_consumer(
            test_outbound(),
            Some(keys.clone()),
            SUBJECT.into(),
        )],
    );
    wait_processed(&admin, "push", first_event).await;
    stop(handle).await;
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
    let handle = dispatcher(
        &pool,
        vec![
            fvoci_server::mail::mail_consumer(failing_mail),
            push_consumer(test_outbound(), Some(keys.clone()), SUBJECT.into()),
        ],
    );
    wait_processed(&admin, "push", event_id).await;
    stop(handle).await;

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

    // Replay (cursor rebased as `--recover-outbox` does): no second push.
    sqlx::query("UPDATE fvoci.outbox_consumers SET last_xact = '0'::xid8, last_seq = 0 WHERE consumer = 'push'")
        .execute(&admin)
        .await
        .unwrap();
    let before = received.lock().unwrap().len();
    let handle = dispatcher(
        &pool,
        vec![push_consumer(
            test_outbound(),
            Some(keys.clone()),
            SUBJECT.into(),
        )],
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let (xact_done,): (bool,) = sqlx::query_as(
            "SELECT (c.last_xact, c.last_seq) >= (e.xact, e.seq) FROM fvoci.outbox_consumers c, fvoci.events e \
             WHERE c.consumer = 'push' AND e.id = $1",
        )
        .bind(event_id)
        .fetch_one(&admin)
        .await
        .unwrap();
        if xact_done {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "push cursor never caught up"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    stop(handle).await;
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
    let handle = dispatcher(
        &pool,
        vec![push_consumer(product, Some(keys), SUBJECT.into())],
    );
    wait_processed(&admin, "push", event_id).await;
    stop(handle).await;
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
