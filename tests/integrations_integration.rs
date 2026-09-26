#![cfg(feature = "db-tests")]
#![allow(dead_code)]

//! Webhooks, GitHub App and document AI routes against a real PostgreSQL with
//! the unprivileged app role. Webhook receivers and the fake GitHub API are
//! local HTTP servers bound to 127.0.0.1:0; nothing leaves the host.

#[path = "support/project_harness.rs"]
mod project_harness;

use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::{Body, Bytes};
use axum::extract::{Path as AxPath, State as AxState};
use axum::http::{HeaderMap, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get, patch, post};
use axum::{Json, Router};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use fvoci_server::auth::password::Keyring;
use fvoci_server::integrations::github::{github_sync_consumer, parse_private_key, GithubConfig};
use fvoci_server::integrations::outbound::{
    Outbound, OutboundPolicy, Resolve, ResolveFuture, SystemResolver,
};
use fvoci_server::integrations::webhooks::{
    spawn_webhook_sender, webhooks_consumer, WebhookDeliverySettings, WebhookSenderHandle,
};
use fvoci_server::integrations::{ai::AiConfig, Integrations};
use fvoci_server::outbox::{
    spawn_outbox_dispatcher, OutboxConsumer, OutboxDispatcherHandle, OutboxDispatcherSettings,
};
use hmac::{Hmac, Mac};
use project_harness::{
    add_workspace_user, admin_pool, app_state, create_project, setup_session, test_peer, TestDb,
};
use serde_json::{json, Value};
use sha2::Sha256;
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

const ENC_KEYS: &str =
    r#"{"k1":"0101010101010101010101010101010101010101010101010101010101010101"}"#;
const GITHUB_KEY: &str = include_str!("fixtures/github-app-test-key.pem");
const GITHUB_SECRET: &str = "gh-webhook-secret";

fn keys() -> Arc<Keyring> {
    Arc::new(Keyring::parse(ENC_KEYS, "k1").expect("keys"))
}

// ---------------------------------------------------------------------------
// Local webhook receiver

#[derive(Clone, Debug)]
enum Reply {
    Status(u16),
    Redirect(String),
    Slow(Duration),
    Endless,
}

#[derive(Clone, Debug)]
struct Received {
    path: String,
    headers: HeaderMap,
    body: Bytes,
}

/// Replies are scripted per path because deliveries are sent concurrently.
#[derive(Clone, Default)]
struct Receiver {
    replies: Arc<Mutex<HashMap<String, VecDeque<Reply>>>>,
    received: Arc<Mutex<Vec<Received>>>,
}

impl Receiver {
    fn script(&self, path: &str, replies: &[Reply]) {
        self.replies
            .lock()
            .unwrap()
            .entry(path.to_string())
            .or_default()
            .extend(replies.iter().cloned());
    }

    fn count(&self) -> usize {
        self.received.lock().unwrap().len()
    }

    fn all(&self) -> Vec<Received> {
        self.received.lock().unwrap().clone()
    }
}

async fn receive(AxState(receiver): AxState<Receiver>, request: Request<Body>) -> Response {
    let path = request.uri().path().to_string();
    let headers = request.headers().clone();
    let body = axum::body::to_bytes(request.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap_or_default();
    receiver.received.lock().unwrap().push(Received {
        path: path.clone(),
        headers,
        body,
    });
    let reply = receiver
        .replies
        .lock()
        .unwrap()
        .get_mut(&path)
        .and_then(VecDeque::pop_front);
    match reply.unwrap_or(Reply::Status(204)) {
        Reply::Status(code) => StatusCode::from_u16(code).unwrap().into_response(),
        Reply::Redirect(location) => (StatusCode::FOUND, [("location", location)]).into_response(),
        Reply::Slow(delay) => {
            tokio::time::sleep(delay).await;
            StatusCode::OK.into_response()
        }
        Reply::Endless => {
            let stream = futures_util::stream::unfold((), |()| async {
                Some((
                    Ok::<_, std::io::Error>(Bytes::from(vec![b'x'; 64 * 1024])),
                    (),
                ))
            });
            (StatusCode::OK, Body::from_stream(stream)).into_response()
        }
    }
}

async fn start_receiver() -> (Receiver, SocketAddr) {
    let receiver = Receiver::default();
    let app = Router::new()
        .route("/{*path}", any(receive))
        .with_state(receiver.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (receiver, addr)
}

/// DNS answers for test host names; every other name uses the system resolver.
struct TableResolver(HashMap<String, Vec<IpAddr>>);

impl Resolve for TableResolver {
    fn lookup<'a>(&'a self, host: &'a str) -> ResolveFuture<'a> {
        match self.0.get(host) {
            Some(answers) => {
                let answers = answers.clone();
                Box::pin(async move { Ok(answers) })
            }
            None => SystemResolver.lookup(host),
        }
    }
}

fn outbound(allow: &str, table: &[(&str, &[&str])]) -> Outbound {
    let map = table
        .iter()
        .map(|(host, ips)| {
            (
                host.to_string(),
                ips.iter().map(|ip| ip.parse().unwrap()).collect(),
            )
        })
        .collect();
    Outbound::new(
        OutboundPolicy::parse_allow_list(allow).expect("allow list"),
        Arc::new(TableResolver(map)),
    )
}

fn integrations(outbound: Outbound) -> Arc<Integrations> {
    Arc::new(Integrations {
        encryption_keys: Some(keys()),
        outbound,
        github: None,
        ai: None,
    })
}

async fn app(harness: &TestDb, integrations: Arc<Integrations>) -> Router {
    fvoci_server::http::router_with_integrations(
        app_state(&harness.app_url).await,
        None,
        integrations,
    )
}

async fn call(
    app: &Router,
    method: &str,
    path: &str,
    body: Option<Value>,
    cookie: Option<&str>,
) -> (StatusCode, Value, HeaderMap) {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("origin", "http://localhost");
    if let Some(cookie) = cookie {
        builder = builder.header("cookie", format!("fvoci_session={cookie}"));
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
        .insert(axum::extract::ConnectInfo(test_peer()));
    let response = app.clone().oneshot(request).await.expect("response");
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap_or_default();
    let json = serde_json::from_slice(&bytes).unwrap_or(json!({}));
    (status, json, headers)
}

async fn raw_post(
    app: &Router,
    path: &str,
    body: &[u8],
    headers: &[(&str, &str)],
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method("POST").uri(path);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let mut request = builder.body(Body::from(body.to_vec())).unwrap();
    request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(test_peer()));
    let response = app.clone().oneshot(request).await.expect("response");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap_or_default();
    (status, serde_json::from_slice(&bytes).unwrap_or(json!({})))
}

async fn wait_until<F, Fut>(what: &str, mut check: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if check().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("timed out waiting for {what}");
}

fn dispatcher_settings() -> OutboxDispatcherSettings {
    OutboxDispatcherSettings {
        poll_interval: Duration::from_millis(50),
        ..OutboxDispatcherSettings::default()
    }
}

fn delivery_settings(timeout: Duration) -> WebhookDeliverySettings {
    WebhookDeliverySettings {
        request_timeout: timeout,
        poll_interval: Duration::from_millis(50),
        ..WebhookDeliverySettings::default()
    }
}

/// The product wiring: the outbox dispatcher fans out (`webhooks` consumer,
/// plus `extra` consumers) and an independent sender task sends due rows.
struct Delivery {
    dispatcher: OutboxDispatcherHandle,
    sender: WebhookSenderHandle,
}

impl Delivery {
    fn start(
        pool: &PgPool,
        outbound: Outbound,
        timeout: Duration,
        extra: Vec<Arc<dyn OutboxConsumer>>,
    ) -> Self {
        let mut consumers = vec![webhooks_consumer()];
        consumers.extend(extra);
        Self {
            dispatcher: spawn_outbox_dispatcher(dispatcher_settings(), pool.clone(), consumers)
                .expect("dispatcher"),
            sender: spawn_webhook_sender(
                pool.clone(),
                outbound,
                Some(keys()),
                delivery_settings(timeout),
            ),
        }
    }

    async fn stop(self) {
        self.sender.request_shutdown();
        self.dispatcher.request_shutdown();
        self.sender.join().await.unwrap();
        self.dispatcher.join().await.unwrap();
    }
}

fn hmac_hex(secret: &str, body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(body);
    hex::encode(mac.finalize().into_bytes())
}

async fn create_hook(
    app: &Router,
    cookie: &str,
    workspace_id: Uuid,
    url: &str,
    events: &[&str],
) -> (Uuid, String) {
    let (status, body, _) = call(
        app,
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/webhooks"),
        Some(json!({ "url": url, "events": events })),
        Some(cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    (
        Uuid::parse_str(body["id"].as_str().unwrap()).unwrap(),
        body["secret"].as_str().unwrap().to_string(),
    )
}

/// Records a workspace event the way product writes do (system context).
async fn insert_event(
    admin: &PgPool,
    workspace_id: Uuid,
    verb: &str,
    target: Option<(&str, Uuid)>,
    payload: Value,
) -> Uuid {
    let id = Uuid::now_v7();
    let mut tx = admin.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.system_ctx', 'on', true)")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        r#"
        INSERT INTO fvoci.events (id, workspace_id, verb, target_type, target_id, payload)
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(id)
    .bind(workspace_id)
    .bind(verb)
    .bind(target.map(|t| t.0))
    .bind(target.map(|t| t.1))
    .bind(payload)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    id
}

#[derive(Debug, Clone, PartialEq)]
struct DeliveryRow {
    status: String,
    attempt: i32,
    http_status: Option<i32>,
    next_in_secs: Option<f64>,
}

async fn deliveries(admin: &PgPool, webhook_id: Uuid) -> Vec<DeliveryRow> {
    let rows: Vec<(String, i32, Option<i32>, Option<f64>)> = sqlx::query_as(
        r#"
        SELECT status, attempt, http_status,
               EXTRACT(EPOCH FROM (next_attempt_at - now()))::float8
        FROM fvoci.webhook_deliveries WHERE webhook_id = $1 ORDER BY created_at, id
        "#,
    )
    .bind(webhook_id)
    .fetch_all(admin)
    .await
    .unwrap();
    rows.into_iter()
        .map(|(status, attempt, http_status, next_in_secs)| DeliveryRow {
            status,
            attempt,
            http_status,
            next_in_secs,
        })
        .collect()
}

async fn make_due(admin: &PgPool, webhook_id: Uuid) {
    sqlx::query(
        "UPDATE fvoci.webhook_deliveries SET next_attempt_at = now() WHERE webhook_id = $1 AND status = 'pending'",
    )
    .bind(webhook_id)
    .execute(admin)
    .await
    .unwrap();
}

// ---------------------------------------------------------------------------
// Webhook CRUD

#[tokio::test]
async fn webhook_crud_is_admin_only_and_shows_the_secret_once() {
    let harness = TestDb::bootstrap().await;
    let (_, owner_cookie, _owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let app = app(&harness, integrations(outbound("127.0.0.1", &[]))).await;
    let base = format!("/api/v1/workspaces/{workspace_id}/webhooks");

    let (id, secret) = create_hook(
        &app,
        &owner_cookie,
        workspace_id,
        "http://127.0.0.1:9/hook?token=abc",
        &["task.created", "comment.created"],
    )
    .await;
    assert_eq!(secret.len(), 43, "256-bit base64url secret");

    // Sealed at rest, bound to the row; never plaintext.
    let stored: String = sqlx::query_scalar("SELECT secret FROM fvoci.webhooks WHERE id = $1")
        .bind(id)
        .fetch_one(&admin)
        .await
        .unwrap();
    assert!(stored.starts_with("enc:v2:k1:"), "{stored}");
    assert!(!stored.contains(&secret));
    assert_eq!(
        fvoci_server::secret_box::open(&keys(), &stored, &format!("webhook:{workspace_id}:{id}"))
            .unwrap(),
        secret
    );

    let (status, list, _) = call(&app, "GET", &base, None, Some(&owner_cookie)).await;
    assert_eq!(status, StatusCode::OK);
    let items = list["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], id.to_string());
    assert_eq!(items[0]["url"], "http://127.0.0.1:9/hook?token=abc");
    assert_eq!(
        items[0]["events"],
        json!(["task.created", "comment.created"])
    );
    assert!(
        items[0].get("secret").is_none(),
        "secret is shown only once"
    );

    // Members, guests and outsiders see 404 for every verb.
    let member = add_workspace_user(&admin, workspace_id, "member", "member").await;
    for (method, path, body) in [
        ("GET", base.clone(), None),
        (
            "POST",
            base.clone(),
            Some(json!({"url": "https://example.com/h", "events": ["task.created"]})),
        ),
        ("DELETE", format!("{base}/{id}"), None),
    ] {
        let (status, body, _) = call(&app, method, &path, body, Some(&member.cookie)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {path}: {body}");
    }
    let (status, _, _) = call(&app, "GET", &base, None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Input rules (source contract + SSRF rules at creation).
    for (url, events) in [
        ("http://localhost/h", json!(["task.created"])),
        ("http://169.254.169.254/latest", json!(["task.created"])),
        ("http://[::1]/h", json!(["task.created"])),
        ("http://[::ffff:127.0.0.1]/h", json!(["task.created"])),
        ("http://10.0.0.5/h", json!(["task.created"])),
        ("http://example.com:8080/h", json!(["task.created"])),
        ("http://user:pw@example.com/h", json!(["task.created"])),
        ("ftp://example.com/h", json!(["task.created"])),
        ("http://metadata.google.internal/", json!(["task.created"])),
        ("https://example.com/h", json!([])),
        ("https://example.com/h", json!(["task.exploded"])),
        ("https://example.com/h", json!("task.created")),
    ] {
        let (status, body, _) = call(
            &app,
            "POST",
            &base,
            Some(json!({ "url": url, "events": events })),
            Some(&owner_cookie),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{url} {events}: {body}");
        assert_eq!(body["code"], "invalid_input");
    }
    let (status, _, _) = call(
        &app,
        "POST",
        &base,
        Some(json!({"url": "https://example.com/h", "events": ["task.created"], "extra": 1})),
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Without ENCRYPTION_KEYS a secret cannot be sealed: fail closed.
    let no_keys = Arc::new(Integrations {
        encryption_keys: None,
        ..(*integrations(outbound("", &[]))).clone()
    });
    let app_no_keys = self::app(&harness, no_keys).await;
    let (status, body, _) = call(
        &app_no_keys,
        "POST",
        &base,
        Some(json!({"url": "https://example.com/h", "events": ["task.created"]})),
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert_eq!(body["code"], "integration_unavailable");

    let (status, _, _) = call(
        &app,
        "DELETE",
        &format!("{base}/{id}"),
        None,
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = call(
        &app,
        "DELETE",
        &format!("{base}/{id}"),
        None,
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Audit rows carry ids only: no URL (it had a token) and no secret.
    let audits: Vec<(String, Value)> = sqlx::query_as(
        "SELECT verb, payload FROM fvoci.audit_log WHERE verb LIKE 'webhook.%' ORDER BY created_at",
    )
    .fetch_all(&admin)
    .await
    .unwrap();
    assert_eq!(
        audits.iter().map(|a| a.0.as_str()).collect::<Vec<_>>(),
        ["webhook.created", "webhook.deleted"]
    );
    for (_, payload) in &audits {
        let text = payload.to_string();
        assert!(
            !text.contains("token=abc") && !text.contains(&secret),
            "{text}"
        );
    }

    admin.close().await;
    harness.cleanup().await;
}

// ---------------------------------------------------------------------------
// Delivery through the outbox dispatcher

#[tokio::test]
async fn webhook_delivers_a_signed_source_payload_through_the_outbox() {
    let harness = TestDb::bootstrap().await;
    let (_, cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let (receiver, addr) = start_receiver().await;
    let outbound = outbound("127.0.0.1", &[]);
    let app = app(&harness, integrations(outbound.clone())).await;
    let (hook_id, secret) = create_hook(
        &app,
        &cookie,
        workspace_id,
        &format!("http://{addr}/hook"),
        &["project.created"],
    )
    .await;

    let pool = project_harness::app_pool(&harness).await;
    let dispatcher = Delivery::start(&pool, outbound, Duration::from_secs(10), Vec::new());

    let project = create_project(app.clone(), &cookie, workspace_id, "HOOK", "workspace").await;
    wait_until("webhook POST", || {
        let receiver = receiver.clone();
        async move { receiver.count() == 1 }
    })
    .await;
    let got = receiver.all().remove(0);
    assert_eq!(got.path, "/hook");
    assert_eq!(got.headers["content-type"], "application/json");
    assert_eq!(got.headers["user-agent"], "FVOCI-Webhook/1");
    assert_eq!(
        got.headers["x-fvoci-signature"].to_str().unwrap(),
        format!("sha256={}", hmac_hex(&secret, &got.body))
    );
    let payload: Value = serde_json::from_slice(&got.body).unwrap();
    assert_eq!(payload["verb"], "project.created");
    assert_eq!(payload["workspaceId"], workspace_id.to_string());
    assert_eq!(payload["actorUserId"], owner_id.to_string());
    assert_eq!(payload["targetType"], "project");
    assert_eq!(payload["targetId"], project["id"]);
    assert_eq!(payload["payload"]["projectId"], project["id"]);
    assert_eq!(payload["channel"], "web");
    let created_at = payload["createdAt"].as_str().unwrap();
    assert!(
        created_at.len() == 24 && created_at.ends_with('Z'),
        "JS toISOString form: {created_at}"
    );
    let keys: Vec<&str> = payload
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys.len(),
        9,
        "source serializeWebhookPayload has exactly nine fields: {keys:?}"
    );

    wait_until("delivered row", || {
        let admin = admin.clone();
        async move {
            deliveries(&admin, hook_id).await
                == vec![DeliveryRow {
                    status: "delivered".into(),
                    attempt: 1,
                    http_status: Some(204),
                    next_in_secs: None,
                }]
        }
    })
    .await;
    // Unsubscribed verbs produce no delivery.
    insert_event(&admin, workspace_id, "comment.created", None, json!({})).await;
    let processed_marker =
        insert_event(&admin, workspace_id, "project.updated", None, json!({})).await;
    wait_until("webhooks consumer passed the marker", || {
        let admin = admin.clone();
        async move {
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS (SELECT 1 FROM fvoci.processed_events WHERE consumer = 'webhooks' AND event_id = $1)",
            )
            .bind(processed_marker)
            .fetch_one(&admin)
            .await
            .unwrap()
        }
    })
    .await;
    assert_eq!(deliveries(&admin, hook_id).await.len(), 1);
    assert_eq!(receiver.count(), 1);

    dispatcher.stop().await;
    pool.close().await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn webhook_retries_with_source_backoff_and_stops_on_client_errors() {
    let harness = TestDb::bootstrap().await;
    let (_, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let (receiver, addr) = start_receiver().await;
    let outbound = outbound("127.0.0.1", &[]);
    let app = app(&harness, integrations(outbound.clone())).await;
    let (retry_hook, _) = create_hook(
        &app,
        &cookie,
        workspace_id,
        &format!("http://{addr}/retry"),
        &["project.updated"],
    )
    .await;
    let pool = project_harness::app_pool(&harness).await;
    let dispatcher = Delivery::start(&pool, outbound, Duration::from_millis(400), Vec::new());

    // 500 → retry in 60 s; 503 → 300 s; timeout → 900 s; 502 → 900 s;
    // fifth failure (500) → failed. Each retry is made due by moving
    // next_attempt_at, the same column the dispatcher polls.
    receiver.script(
        "/retry",
        &[
            Reply::Status(500),
            Reply::Status(503),
            Reply::Slow(Duration::from_secs(3)),
            Reply::Status(502),
            Reply::Status(500),
        ],
    );
    insert_event(&admin, workspace_id, "project.updated", None, json!({})).await;
    let expected = [
        (1, Some(500), Some(60.0)),
        (2, Some(503), Some(300.0)),
        (3, None, Some(900.0)),
        (4, Some(502), Some(900.0)),
    ];
    for (attempt, http_status, next) in expected {
        wait_until(&format!("attempt {attempt}"), || {
            let admin = admin.clone();
            async move {
                deliveries(&admin, retry_hook)
                    .await
                    .first()
                    .is_some_and(|row| row.attempt == attempt)
            }
        })
        .await;
        let row = deliveries(&admin, retry_hook).await.remove(0);
        assert_eq!(row.status, "pending", "attempt {attempt}");
        assert_eq!(row.http_status, http_status, "attempt {attempt}");
        let next_in = row.next_in_secs.expect("next attempt");
        let want = next.unwrap();
        assert!(
            next_in > want - 10.0 && next_in <= want,
            "attempt {attempt}: next in {next_in}s, want ≈{want}s"
        );
        make_due(&admin, retry_hook).await;
    }
    wait_until("fifth attempt", || {
        let admin = admin.clone();
        async move {
            deliveries(&admin, retry_hook)
                .await
                .first()
                .is_some_and(|row| row.attempt == 5)
        }
    })
    .await;
    assert_eq!(
        deliveries(&admin, retry_hook).await,
        vec![DeliveryRow {
            status: "failed".into(),
            attempt: 5,
            http_status: Some(500),
            next_in_secs: None,
        }]
    );
    assert_eq!(receiver.count(), 5);

    // 4xx is terminal on the first attempt; 2xx after a 5xx is delivered.
    let (gone_hook, _) = create_hook(
        &app,
        &cookie,
        workspace_id,
        &format!("http://{addr}/gone"),
        &["project.archived"],
    )
    .await;
    receiver.script("/gone", &[Reply::Status(410)]);
    insert_event(&admin, workspace_id, "project.archived", None, json!({})).await;
    wait_until("410 recorded", || {
        let admin = admin.clone();
        async move {
            !deliveries(&admin, gone_hook).await.is_empty()
                && deliveries(&admin, gone_hook).await[0].attempt == 1
        }
    })
    .await;
    assert_eq!(
        deliveries(&admin, gone_hook).await,
        vec![DeliveryRow {
            status: "failed".into(),
            attempt: 1,
            http_status: Some(410),
            next_in_secs: None,
        }]
    );

    // A receiver that is down (connection refused) is retried.
    let closed = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap()
    };
    let (down_hook, _) = create_hook(
        &app,
        &cookie,
        workspace_id,
        &format!("http://{closed}/down"),
        &["project.unarchived"],
    )
    .await;
    insert_event(&admin, workspace_id, "project.unarchived", None, json!({})).await;
    wait_until("refused recorded", || {
        let admin = admin.clone();
        async move {
            deliveries(&admin, down_hook)
                .await
                .first()
                .is_some_and(|r| r.attempt == 1)
        }
    })
    .await;
    let row = deliveries(&admin, down_hook).await.remove(0);
    assert_eq!((row.status.as_str(), row.http_status), ("pending", None));

    dispatcher.stop().await;
    pool.close().await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn webhook_refuses_redirects_private_answers_and_reads_bounded_responses() {
    let harness = TestDb::bootstrap().await;
    let (_, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let (receiver, addr) = start_receiver().await;
    let port = addr.port();
    // Host names are listed (so a non-default port is accepted) but only the
    // exact IP 127.0.0.1 is an allowed private answer.
    let outbound = outbound(
        "127.0.0.1,hook.test,rebind.test,meta.test,mixed.test,v6.test",
        &[
            ("hook.test", &["127.0.0.1"]),
            ("rebind.test", &["127.0.0.2"]),
            ("meta.test", &["169.254.169.254"]),
            ("mixed.test", &["93.184.216.34", "10.0.0.1"]),
            ("v6.test", &["::1"]),
        ],
    );
    let app = app(&harness, integrations(outbound.clone())).await;
    let mut hooks = HashMap::new();
    for (name, url) in [
        ("pinned", format!("http://hook.test:{port}/pinned")),
        ("rebind", format!("http://rebind.test:{port}/rebind")),
        ("meta", format!("http://meta.test:{port}/latest/meta-data")),
        ("mixed", format!("http://mixed.test:{port}/mixed")),
        ("v6", format!("http://v6.test:{port}/v6")),
        ("endless", format!("http://127.0.0.1:{port}/endless")),
    ] {
        let (id, _) = create_hook(&app, &cookie, workspace_id, &url, &["task.deleted"]).await;
        hooks.insert(name, id);
    }
    let pool = project_harness::app_pool(&harness).await;
    let dispatcher = Delivery::start(&pool, outbound, Duration::from_secs(5), Vec::new());
    receiver.script("/endless", &[Reply::Endless]);
    let started = Instant::now();
    insert_event(&admin, workspace_id, "task.deleted", None, json!({})).await;
    wait_until("every hook settled", || {
        let admin = admin.clone();
        let hooks = hooks.clone();
        async move {
            for id in hooks.values() {
                let rows = deliveries(&admin, *id).await;
                if rows.first().is_none_or(|r| r.attempt == 0) {
                    return false;
                }
            }
            true
        }
    })
    .await;
    let row = |name: &'static str| {
        let admin = admin.clone();
        let id = hooks[name];
        async move { deliveries(&admin, id).await.remove(0) }
    };
    // Connected to the checked address with the original Host header.
    assert_eq!(row("pinned").await.status, "delivered");
    let pinned = receiver
        .all()
        .into_iter()
        .find(|r| r.path == "/pinned")
        .expect("pinned request");
    assert_eq!(pinned.headers["host"], format!("hook.test:{port}"));
    // Private DNS answers are refused before any connection: terminal, and
    // the receiver never saw them.
    for name in ["rebind", "meta", "mixed", "v6"] {
        let r = row(name).await;
        assert_eq!(
            (r.status.as_str(), r.attempt, r.http_status),
            ("failed", 1, None),
            "{name}"
        );
    }
    let paths: Vec<String> = receiver.all().into_iter().map(|r| r.path).collect();
    for refused in ["/rebind", "/latest/meta-data", "/mixed", "/v6"] {
        assert!(
            !paths.iter().any(|p| p == refused),
            "{refused} reached: {paths:?}"
        );
    }
    // Endless body: the read stops at the cap and the delivery completes
    // well inside the 5 s request budget.
    assert_eq!(row("endless").await.status, "delivered");
    assert!(started.elapsed() < Duration::from_secs(5));

    // Redirect: refused (not followed) and terminal; the Location target is
    // never requested.
    let (redirect_receiver, redirect_addr) = start_receiver().await;
    let stolen = format!("http://{redirect_addr}/stolen");
    redirect_receiver.script("/redirect", &[Reply::Redirect(stolen)]);
    let (redirect_hook, _) = create_hook(
        &app,
        &cookie,
        workspace_id,
        &format!("http://{redirect_addr}/redirect"),
        &["comment.deleted"],
    )
    .await;
    insert_event(&admin, workspace_id, "comment.deleted", None, json!({})).await;
    wait_until("redirect recorded", || {
        let admin = admin.clone();
        async move {
            deliveries(&admin, redirect_hook)
                .await
                .first()
                .is_some_and(|r| r.attempt == 1)
        }
    })
    .await;
    let r = deliveries(&admin, redirect_hook).await.remove(0);
    assert_eq!((r.status.as_str(), r.http_status), ("failed", None));
    let paths: Vec<String> = redirect_receiver
        .all()
        .into_iter()
        .map(|r| r.path)
        .collect();
    assert_eq!(paths, ["/redirect"], "Location was not followed");

    dispatcher.stop().await;
    pool.close().await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn webhook_fan_out_rechecks_creator_role_and_event_visibility() {
    let harness = TestDb::bootstrap().await;
    let (_, owner_cookie, _owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let (receiver, addr) = start_receiver().await;
    let outbound = outbound("127.0.0.1", &[]);
    let app = app(&harness, integrations(outbound.clone())).await;
    let second_admin = add_workspace_user(&admin, workspace_id, "admin", "hookadmin").await;
    let (hook_id, _) = create_hook(
        &app,
        &second_admin.cookie,
        workspace_id,
        &format!("http://{addr}/vis"),
        &["task.updated"],
    )
    .await;

    // A private project the hook creator is not a member of: its events are
    // not delivered (workspace admins do not see private projects).
    let private = create_project(
        app.clone(),
        &owner_cookie,
        workspace_id,
        "SECRET",
        "private",
    )
    .await;
    let private_id = Uuid::parse_str(private["id"].as_str().unwrap()).unwrap();
    let open = create_project(
        app.clone(),
        &owner_cookie,
        workspace_id,
        "OPEN",
        "workspace",
    )
    .await;
    let open_id = Uuid::parse_str(open["id"].as_str().unwrap()).unwrap();
    insert_event(
        &admin,
        workspace_id,
        "task.updated",
        None,
        json!({ "projectId": private_id.to_string(), "title": "hidden" }),
    )
    .await;
    insert_event(
        &admin,
        workspace_id,
        "task.updated",
        None,
        json!({ "projectId": open_id.to_string(), "title": "visible" }),
    )
    .await;

    let pool = project_harness::app_pool(&harness).await;
    let dispatcher = Delivery::start(&pool, outbound, Duration::from_secs(5), Vec::new());
    wait_until("visible delivery", || {
        let receiver = receiver.clone();
        async move { receiver.count() == 1 }
    })
    .await;
    let body: Value = serde_json::from_slice(&receiver.all()[0].body).unwrap();
    assert_eq!(body["payload"]["title"], "visible");
    assert_eq!(deliveries(&admin, hook_id).await.len(), 1);

    // Pending retry, then the creator is demoted: the send-time recheck fails
    // the row without contacting the receiver.
    receiver.script("/vis", &[Reply::Status(500)]);
    insert_event(
        &admin,
        workspace_id,
        "task.updated",
        None,
        json!({ "projectId": open_id.to_string(), "title": "second" }),
    )
    .await;
    wait_until("second attempt pending", || {
        let admin = admin.clone();
        async move {
            let rows = deliveries(&admin, hook_id).await;
            rows.len() == 2 && rows[1].attempt == 1
        }
    })
    .await;
    sqlx::query(
        "UPDATE fvoci.memberships SET role = 'member' WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(workspace_id)
    .bind(second_admin.user_id)
    .execute(&admin)
    .await
    .unwrap();
    make_due(&admin, hook_id).await;
    wait_until("demoted creator row failed", || {
        let admin = admin.clone();
        async move { deliveries(&admin, hook_id).await[1].status == "failed" }
    })
    .await;
    assert_eq!(deliveries(&admin, hook_id).await[1].attempt, 2);
    assert_eq!(receiver.count(), 2, "no send after demotion");

    // New events are no longer fanned out to the demoted creator's hook.
    let marker = insert_event(
        &admin,
        workspace_id,
        "task.updated",
        None,
        json!({ "projectId": open_id.to_string(), "title": "third" }),
    )
    .await;
    wait_until("marker processed", || {
        let admin = admin.clone();
        async move {
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS (SELECT 1 FROM fvoci.processed_events WHERE consumer = 'webhooks' AND event_id = $1)",
            )
            .bind(marker)
            .fetch_one(&admin)
            .await
            .unwrap()
        }
    })
    .await;
    assert_eq!(deliveries(&admin, hook_id).await.len(), 2);

    // Removing the hook cascades its ledger.
    let (status, _, _) = call(
        &app,
        "DELETE",
        &format!("/api/v1/workspaces/{workspace_id}/webhooks/{hook_id}"),
        None,
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(deliveries(&admin, hook_id).await.is_empty());

    dispatcher.stop().await;
    pool.close().await;
    admin.close().await;
    harness.cleanup().await;
}

// ---------------------------------------------------------------------------
// GitHub (local fake API; github.com is never contacted)

/// Scripted fake of the GitHub REST API. Token and PATCH replies pop from
/// per-installation / per-issue queues (default 201 / 200); `GET
/// /app/installations/{id}` answers 200 only for ids in `installations`.
#[derive(Clone, Default)]
struct FakeGithub {
    calls: Arc<Mutex<Vec<(String, String, Value)>>>,
    public_key: Arc<Vec<u8>>,
    installations: Arc<Mutex<std::collections::HashSet<String>>>,
    token_replies: Arc<Mutex<HashMap<String, VecDeque<u16>>>>,
    patch_replies: Arc<Mutex<HashMap<String, VecDeque<u16>>>>,
}

impl FakeGithub {
    fn script_token(&self, installation: &str, statuses: &[u16]) {
        self.token_replies
            .lock()
            .unwrap()
            .entry(installation.to_string())
            .or_default()
            .extend(statuses);
    }

    fn script_patch(&self, path: &str, statuses: &[u16]) {
        self.patch_replies
            .lock()
            .unwrap()
            .entry(path.to_string())
            .or_default()
            .extend(statuses);
    }

    fn calls(&self) -> Vec<(String, String, Value)> {
        self.calls.lock().unwrap().clone()
    }

    fn count(&self, method: &str, path: &str) -> usize {
        self.calls()
            .iter()
            .filter(|c| c.0 == method && c.1 == path)
            .count()
    }
}

fn verify_jwt(fake: &FakeGithub, headers: &HeaderMap) -> bool {
    let Some(token) = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
    else {
        return false;
    };
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return false;
    }
    let Ok(signature) = URL_SAFE_NO_PAD.decode(parts[2]) else {
        return false;
    };
    let key = ring::signature::UnparsedPublicKey::new(
        &ring::signature::RSA_PKCS1_2048_8192_SHA256,
        fake.public_key.as_slice(),
    );
    if key
        .verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &signature)
        .is_err()
    {
        return false;
    }
    let claims: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[1]).unwrap()).unwrap();
    claims["iss"] == "123" && claims["exp"].as_i64().unwrap() > chrono::Utc::now().timestamp()
}

async fn fake_app(AxState(fake): AxState<FakeGithub>, headers: HeaderMap) -> Response {
    fake.calls
        .lock()
        .unwrap()
        .push(("GET".into(), "/app".into(), json!({})));
    if !verify_jwt(&fake, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    Json(json!({ "slug": "fvoci-test" })).into_response()
}

async fn fake_installation(
    AxState(fake): AxState<FakeGithub>,
    AxPath(id): AxPath<String>,
    headers: HeaderMap,
) -> Response {
    fake.calls
        .lock()
        .unwrap()
        .push(("GET".into(), format!("/app/installations/{id}"), json!({})));
    if !verify_jwt(&fake, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if fake.installations.lock().unwrap().contains(&id) {
        Json(json!({ "id": id.parse::<u64>().unwrap() })).into_response()
    } else {
        StatusCode::NOT_FOUND.into_response()
    }
}

async fn fake_token(
    AxState(fake): AxState<FakeGithub>,
    AxPath(id): AxPath<String>,
    headers: HeaderMap,
) -> Response {
    fake.calls.lock().unwrap().push((
        "POST".into(),
        format!("/app/installations/{id}/access_tokens"),
        json!({}),
    ));
    if !verify_jwt(&fake, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let scripted = fake
        .token_replies
        .lock()
        .unwrap()
        .get_mut(&id)
        .and_then(VecDeque::pop_front);
    match scripted.unwrap_or(201) {
        201 => (StatusCode::CREATED, Json(json!({ "token": "ghs_fake" }))).into_response(),
        other => StatusCode::from_u16(other).unwrap().into_response(),
    }
}

async fn fake_patch(
    AxState(fake): AxState<FakeGithub>,
    AxPath((owner, repo, number)): AxPath<(String, String, String)>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let path = format!("/repos/{owner}/{repo}/issues/{number}");
    fake.calls
        .lock()
        .unwrap()
        .push(("PATCH".into(), path.clone(), body));
    if headers.get("authorization").and_then(|v| v.to_str().ok()) != Some("Bearer ghs_fake") {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let scripted = fake
        .patch_replies
        .lock()
        .unwrap()
        .get_mut(&path)
        .and_then(VecDeque::pop_front);
    StatusCode::from_u16(scripted.unwrap_or(200))
        .unwrap()
        .into_response()
}

async fn start_fake_github() -> (FakeGithub, SocketAddr) {
    let key = parse_private_key(GITHUB_KEY).unwrap();
    use ring::signature::KeyPair;
    let fake = FakeGithub {
        public_key: Arc::new(key.public_key().as_ref().to_vec()),
        ..FakeGithub::default()
    };
    fake.installations
        .lock()
        .unwrap()
        .extend(["42".to_string(), "43".to_string()]);
    let app = Router::new()
        .route("/app", get(fake_app))
        .route("/app/installations/{id}", get(fake_installation))
        .route("/app/installations/{id}/access_tokens", post(fake_token))
        .route("/repos/{owner}/{repo}/issues/{number}", patch(fake_patch))
        .with_state(fake.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (fake, addr)
}

const STATE_KEY: [u8; 32] = [9u8; 32];

fn github_config(addr: SocketAddr) -> GithubConfig {
    GithubConfig::new(
        "123",
        GITHUB_KEY,
        GITHUB_SECRET,
        &format!("http://{addr}"),
        STATE_KEY,
    )
    .unwrap()
}

fn github_integrations(addr: SocketAddr) -> Arc<Integrations> {
    Arc::new(Integrations {
        github: Some(github_config(addr)),
        ..(*integrations(outbound("", &[]))).clone()
    })
}

async fn second_workspace(app: &Router, cookie: &str, slug: &str) -> Uuid {
    let (status, body, _) = call(
        app,
        "POST",
        "/api/v1/workspaces",
        Some(json!({ "name": slug, "slug": slug })),
        Some(cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    Uuid::parse_str(body["id"].as_str().unwrap()).unwrap()
}

/// `POST …/github/install` and the `state` of the returned github.com URL.
async fn start_install(app: &Router, cookie: &str, workspace_id: Uuid) -> String {
    let (status, body, _) = call(
        app,
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/github/install"),
        None,
        Some(cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let url = url::Url::parse(body["url"].as_str().unwrap()).unwrap();
    assert_eq!(url.host_str(), Some("github.com"));
    assert_eq!(url.path(), "/apps/fvoci-test/installations/new");
    url.query_pairs()
        .find(|(k, _)| k == "state")
        .map(|(_, v)| v.to_string())
        .unwrap()
}

async fn callback(
    app: &Router,
    state: &str,
    installation_id: &str,
    cookie: Option<&str>,
) -> (StatusCode, HeaderMap) {
    let (status, _, headers) = call(
        app,
        "GET",
        &format!(
            "/api/v1/github/callback?state={}&installation_id={installation_id}",
            urlencode(state)
        ),
        None,
        cookie,
    )
    .await;
    (status, headers)
}

#[tokio::test]
async fn github_install_callback_get_and_remove() {
    let harness = TestDb::bootstrap().await;
    let (_, cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let (fake, fake_addr) = start_fake_github().await;
    let app = app(&harness, github_integrations(fake_addr)).await;
    let base = format!("/api/v1/workspaces/{workspace_id}/github");

    let (status, body, _) = call(&app, "GET", &base, None, Some(&cookie)).await;
    assert_eq!(
        (status, body),
        (StatusCode::OK, json!({ "installationId": null }))
    );

    let state = start_install(&app, &cookie, workspace_id).await;
    assert_eq!(fake.calls()[0].1, "/app", "slug via the fake API");
    // The nonce is stored hashed with the initiating user and session.
    let (user_id, session_id): (Uuid, Uuid) = sqlx::query_as(
        "SELECT user_id, session_id FROM fvoci.github_install_states WHERE workspace_id = $1",
    )
    .bind(workspace_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(user_id, owner_id);
    assert_eq!(
        session_id,
        project_harness::session_id_for_user(&admin, owner_id).await
    );

    // Member cannot start or read; bad/forged state and ids are 400.
    let member = add_workspace_user(&admin, workspace_id, "member", "m").await;
    for (method, path) in [
        ("GET", base.clone()),
        ("POST", format!("{base}/install")),
        ("DELETE", base.clone()),
    ] {
        let (status, _, _) = call(&app, method, &path, None, Some(&member.cookie)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {path}");
    }
    // Same webhook secret, another state key: forged.
    let forged = GithubConfig::new(
        "123",
        GITHUB_KEY,
        GITHUB_SECRET,
        "http://127.0.0.1:9",
        [1u8; 32],
    )
    .unwrap()
    .sign_install_state(workspace_id, "guess", chrono::Utc::now().timestamp_millis());
    // Authentic MAC, but a nonce that was never issued.
    let unissued = github_config(fake_addr).sign_install_state(
        workspace_id,
        "never-issued",
        chrono::Utc::now().timestamp_millis(),
    );
    for query in [
        format!("state={}&installation_id=42", urlencode(&forged)),
        format!("state={}&installation_id=42", urlencode(&unissued)),
        format!("state={}&installation_id=4x2", urlencode(&state)),
        format!("state={}", urlencode(&state)),
        "installation_id=42".to_string(),
    ] {
        let (status, _, _) = call(
            &app,
            "GET",
            &format!("/api/v1/github/callback?{query}"),
            None,
            Some(&cookie),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{query}");
    }

    // The callback needs the session that started the install: none is 401,
    // another admin of the same workspace is refused.
    assert_eq!(
        callback(&app, &state, "42", None).await.0,
        StatusCode::UNAUTHORIZED
    );
    let other_admin = add_workspace_user(&admin, workspace_id, "admin", "a2").await;
    assert_eq!(
        callback(&app, &state, "42", Some(&other_admin.cookie))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    // An installation GitHub does not know for this app is refused before
    // the nonce is spent.
    assert_eq!(
        callback(&app, &state, "999", Some(&cookie)).await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(fake.count("GET", "/app/installations/999"), 1);

    let checks = fake.count("GET", "/app/installations/42");
    let (status, headers) = callback(&app, &state, "42", Some(&cookie)).await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(headers["location"], "http://localhost/");
    assert_eq!(fake.count("GET", "/app/installations/42"), checks + 1);
    let (_, body, _) = call(&app, "GET", &base, None, Some(&cookie)).await;
    assert_eq!(body, json!({ "installationId": "42" }));
    let audit_actor: Option<Uuid> = sqlx::query_scalar(
        "SELECT actor_user_id FROM fvoci.audit_log WHERE verb = 'github.installed' AND workspace_id = $1",
    )
    .bind(workspace_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(audit_actor, Some(owner_id));
    // Single use: the same state again is refused.
    assert_eq!(
        callback(&app, &state, "42", Some(&cookie)).await.0,
        StatusCode::BAD_REQUEST
    );

    // A new round trip cannot silently replace the link with another
    // installation; completing it with the same one is idempotent.
    let again = start_install(&app, &cookie, workspace_id).await;
    assert_eq!(
        callback(&app, &again, "43", Some(&cookie)).await.0,
        StatusCode::BAD_REQUEST
    );
    let (_, body, _) = call(&app, "GET", &base, None, Some(&cookie)).await;
    assert_eq!(body, json!({ "installationId": "42" }));
    let again = start_install(&app, &cookie, workspace_id).await;
    assert_eq!(
        callback(&app, &again, "42", Some(&cookie)).await.0,
        StatusCode::FOUND
    );

    // Another workspace cannot claim an installation that is linked here.
    let other_ws = second_workspace(&app, &cookie, "gh-other").await;
    let other_state = start_install(&app, &cookie, other_ws).await;
    assert_eq!(
        callback(&app, &other_state, "42", Some(&cookie)).await.0,
        StatusCode::BAD_REQUEST
    );
    // A state minted for one workspace does not complete another's install
    // (the workspace is inside the MAC; the nonce row is per workspace).
    let (_, body, _) = call(
        &app,
        "GET",
        &format!("/api/v1/workspaces/{other_ws}/github"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(body, json!({ "installationId": null }));

    let (status, _, _) = call(&app, "DELETE", &base, None, Some(&cookie)).await;
    assert_eq!(status, StatusCode::OK);
    let (_, body, _) = call(&app, "GET", &base, None, Some(&cookie)).await;
    assert_eq!(body, json!({ "installationId": null }));
    let (status, _, _) = call(&app, "DELETE", &base, None, Some(&cookie)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // An expired, never-completed round trip is removed by maintenance.
    let stale = start_install(&app, &cookie, workspace_id).await;
    sqlx::query("UPDATE fvoci.github_install_states SET expires_at = now() - interval '1 second'")
        .execute(&admin)
        .await
        .unwrap();
    assert_eq!(
        callback(&app, &stale, "43", Some(&cookie)).await.0,
        StatusCode::BAD_REQUEST
    );
    // The daily sweep (app role) also drops GitHub delivery ids past 30 days.
    let old_delivery = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO fvoci.github_deliveries (delivery_id, processed_at) VALUES ($1, now() - interval '31 days'), ($2, now())",
    )
    .bind(old_delivery)
    .bind(Uuid::now_v7())
    .execute(&admin)
    .await
    .unwrap();
    let pool = project_harness::app_pool(&harness).await;
    let sweep_state = app_state(&harness.app_url).await;
    let stats = fvoci_server::jobs::run_daily_sweep(
        &pool,
        &sweep_state.storage,
        &sweep_state.mailer,
        &tokio_util::sync::CancellationToken::new(),
    )
    .await
    .unwrap()
    .expect("sweep lock");
    let states_left: i64 = sqlx::query_scalar("SELECT count(*) FROM fvoci.github_install_states")
        .fetch_one(&admin)
        .await
        .unwrap();
    let deliveries_left: i64 = sqlx::query_scalar("SELECT count(*) FROM fvoci.github_deliveries")
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_eq!((states_left, deliveries_left), (0, 1));
    assert!(stats.github_deliveries >= 2, "{stats:?}");

    // Without the app configured, install is 400 (source) and the public
    // endpoints refuse input.
    let plain = self::app(&harness, integrations(outbound("", &[]))).await;
    let (status, _, _) = call(
        &plain,
        "POST",
        &format!("{base}/install"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = raw_post(&plain, "/api/v1/github/webhook", b"{}", &[]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    pool.close().await;
    admin.close().await;
    harness.cleanup().await;
}

fn urlencode(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

async fn github_hook(app: &Router, event: &str, delivery: &str, payload: &Value) -> StatusCode {
    let body = payload.to_string();
    let signature = format!("sha256={}", hmac_hex(GITHUB_SECRET, body.as_bytes()));
    raw_post(
        app,
        "/api/v1/github/webhook",
        body.as_bytes(),
        &[
            ("content-type", "application/json"),
            ("x-github-event", event),
            ("x-github-delivery", delivery),
            ("x-hub-signature-256", &signature),
        ],
    )
    .await
    .0
}

async fn workflow_statuses(
    app: &Router,
    cookie: &str,
    workspace_id: Uuid,
    project_id: &str,
) -> Vec<Value> {
    let (status, body, _) = call(
        app,
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/workflow"),
        None,
        Some(cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["statuses"].as_array().unwrap().clone()
}

fn status_of(statuses: &[Value], category: &str) -> String {
    statuses.iter().find(|s| s["category"] == category).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn github_webhook_verifies_signature_dedupes_and_syncs_issue_tasks() {
    let harness = TestDb::bootstrap().await;
    let (_, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let (_fake, fake_addr) = start_fake_github().await;
    let app = app(&harness, github_integrations(fake_addr)).await;
    let project = create_project(app.clone(), &cookie, workspace_id, "GH", "workspace").await;
    let project_id = project["id"].as_str().unwrap().to_string();
    let statuses = workflow_statuses(&app, &cookie, workspace_id, &project_id).await;
    let backlog = status_of(&statuses, "backlog");
    let done = status_of(&statuses, "done");
    sqlx::query("INSERT INTO fvoci.github_installations (id, workspace_id, installation_id) VALUES ($1, $2, '42')")
        .bind(Uuid::now_v7())
        .bind(workspace_id)
        .execute(&admin)
        .await
        .unwrap();

    let issue = |action: &str, title: &str, state: &str| {
        json!({
            "action": action,
            "installation": { "id": 42 },
            "repository": { "full_name": "octo/repo" },
            "issue": { "number": 7, "title": title, "state": state },
        })
    };
    // Signature: missing, wrong, and over another body are all 401.
    let opened = issue("opened", "버그 🐛 수정", "open");
    let (status, body) = raw_post(
        &app,
        "/api/v1/github/webhook",
        opened.to_string().as_bytes(),
        &[("x-github-event", "issues")],
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    let wrong = format!("sha256={}", hmac_hex("nope", opened.to_string().as_bytes()));
    let (status, _) = raw_post(
        &app,
        "/api/v1/github/webhook",
        opened.to_string().as_bytes(),
        &[
            ("x-github-event", "issues"),
            ("x-hub-signature-256", &wrong),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let too_big = vec![b' '; 1024 * 1024 + 1];
    let (status, _) = raw_post(&app, "/api/v1/github/webhook", &too_big, &[]).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        github_hook(
            &app,
            "ping",
            &Uuid::now_v7().to_string(),
            &json!({"zen": "hi"})
        )
        .await,
        StatusCode::OK
    );

    // opened → linked task in the first active project, recorded as webhook.
    let delivery = Uuid::now_v7().to_string();
    assert_eq!(
        github_hook(&app, "issues", &delivery, &opened).await,
        StatusCode::OK
    );
    let task: (Uuid, String, Uuid) = sqlx::query_as(
        r#"
        SELECT t.id, t.title, t.status_id FROM fvoci.tasks t
        JOIN fvoci.github_issue_links l ON l.task_id = t.id
        WHERE l.repo = 'octo/repo' AND l.issue_number = 7
        "#,
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(task.1, "버그 🐛 수정");
    assert_eq!(task.2.to_string(), backlog);
    let channel: String = sqlx::query_scalar(
        "SELECT channel FROM fvoci.events WHERE verb = 'task.created' AND target_id = $1",
    )
    .bind(task.0)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(channel, "webhook");

    // Same delivery id again: acknowledged, not applied twice.
    assert_eq!(
        github_hook(&app, "issues", &delivery, &opened).await,
        StatusCode::OK
    );
    let tasks: i64 = sqlx::query_scalar("SELECT count(*) FROM fvoci.tasks")
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_eq!(tasks, 1);

    // closed → done; reopened → backlog; edited → title.
    assert_eq!(
        github_hook(
            &app,
            "issues",
            &Uuid::now_v7().to_string(),
            &issue("closed", "버그 🐛 수정", "closed")
        )
        .await,
        StatusCode::OK
    );
    let status_id: Uuid = sqlx::query_scalar("SELECT status_id FROM fvoci.tasks WHERE id = $1")
        .bind(task.0)
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_eq!(status_id.to_string(), done);
    assert_eq!(
        github_hook(
            &app,
            "issues",
            &Uuid::now_v7().to_string(),
            &issue("reopened", "버그 🐛 수정", "open")
        )
        .await,
        StatusCode::OK
    );
    let status_id: Uuid = sqlx::query_scalar("SELECT status_id FROM fvoci.tasks WHERE id = $1")
        .bind(task.0)
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_eq!(status_id.to_string(), backlog);
    assert_eq!(
        github_hook(
            &app,
            "issues",
            &Uuid::now_v7().to_string(),
            &issue("edited", "새 제목", "open")
        )
        .await,
        StatusCode::OK
    );
    let title: String = sqlx::query_scalar("SELECT title FROM fvoci.tasks WHERE id = $1")
        .bind(task.0)
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_eq!(title, "새 제목");
    let activity: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM fvoci.task_activity WHERE task_id = $1 AND channel = 'webhook'",
    )
    .bind(task.0)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(activity, 4, "created + closed + reopened + edited");

    // Unknown installation is ignored; installation deleted removes the link.
    let mut other = issue("opened", "x", "open");
    other["installation"]["id"] = json!(999);
    assert_eq!(
        github_hook(&app, "issues", &Uuid::now_v7().to_string(), &other).await,
        StatusCode::OK
    );
    assert_eq!(
        github_hook(
            &app,
            "installation",
            &Uuid::now_v7().to_string(),
            &json!({"action": "deleted", "installation": {"id": 42}})
        )
        .await,
        StatusCode::OK
    );
    let installs: i64 = sqlx::query_scalar("SELECT count(*) FROM fvoci.github_installations")
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_eq!(installs, 0);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn github_issue_links_and_status_sync_use_the_fake_api() {
    let harness = TestDb::bootstrap().await;
    let (_, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let (fake, fake_addr) = start_fake_github().await;
    let app = app(&harness, github_integrations(fake_addr)).await;
    let project = create_project(app.clone(), &cookie, workspace_id, "SYNC", "workspace").await;
    let project_id = project["id"].as_str().unwrap().to_string();
    let statuses = workflow_statuses(&app, &cookie, workspace_id, &project_id).await;
    let done = status_of(&statuses, "done");
    let (status, task, _) = call(
        &app,
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
        Some(json!({ "title": "linked" })),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    let task_id = task["id"].as_str().unwrap().to_string();
    sqlx::query("INSERT INTO fvoci.github_installations (id, workspace_id, installation_id) VALUES ($1, $2, '42')")
        .bind(Uuid::now_v7())
        .bind(workspace_id)
        .execute(&admin)
        .await
        .unwrap();

    let links = format!("/api/v1/workspaces/{workspace_id}/github/issue-links");
    let (status, body, _) = call(
        &app,
        "POST",
        &links,
        Some(json!({"taskId": task_id, "repo": "octo/repo", "issueNumber": 7})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["repo"], "octo/repo");
    assert_eq!(body["issueNumber"], 7);
    for bad in [
        json!({"taskId": task_id, "repo": "octo/repo", "issueNumber": 8}),
        json!({"taskId": task_id, "repo": "octo", "issueNumber": 9}),
        json!({"taskId": task_id, "repo": "octo/repo", "issueNumber": 0}),
        json!({"taskId": task_id, "repo": "octo/repo", "issueNumber": 5_000_000_000_i64}),
    ] {
        let (status, _, _) = call(&app, "POST", &links, Some(bad.clone()), Some(&cookie)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
    }
    let (status, _, _) = call(
        &app,
        "POST",
        &links,
        Some(json!({"taskId": Uuid::now_v7(), "repo": "octo/repo", "issueNumber": 11})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let guest = add_workspace_user(&admin, workspace_id, "guest", "g").await;
    let (status, _, _) = call(
        &app,
        "POST",
        &links,
        Some(json!({"taskId": task_id, "repo": "o/r", "issueNumber": 12})),
        Some(&guest.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "guest has no project edit");

    let pool = project_harness::app_pool(&harness).await;
    let dispatcher = spawn_outbox_dispatcher(
        dispatcher_settings(),
        pool.clone(),
        vec![github_sync_consumer(Some(github_config(fake_addr)))],
    )
    .expect("dispatcher");
    let (status, body, _) = call(
        &app,
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/move"),
        Some(json!({ "statusId": done })),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    wait_until("issue PATCH", || {
        let fake = fake.clone();
        async move { fake.calls.lock().unwrap().iter().any(|c| c.0 == "PATCH") }
    })
    .await;
    let calls = fake.calls.lock().unwrap().clone();
    assert_eq!(
        calls
            .iter()
            .map(|c| (c.0.as_str(), c.1.as_str()))
            .collect::<Vec<_>>(),
        [
            ("POST", "/app/installations/42/access_tokens"),
            ("PATCH", "/repos/octo/repo/issues/7"),
        ]
    );
    assert_eq!(calls[1].2, json!({ "state": "closed" }));

    // An inbound (channel webhook) status change is not echoed back.
    let reopened = json!({
        "action": "reopened",
        "installation": { "id": 42 },
        "repository": { "full_name": "octo/repo" },
        "issue": { "number": 7, "title": "linked", "state": "open" },
    });
    assert_eq!(
        github_hook(&app, "issues", &Uuid::now_v7().to_string(), &reopened).await,
        StatusCode::OK
    );
    let marker = {
        let mut tx = admin.begin().await.unwrap();
        sqlx::query("SELECT set_config('app.system_ctx', 'on', true)")
            .execute(&mut *tx)
            .await
            .unwrap();
        let id: Uuid = sqlx::query_scalar(
            "SELECT id FROM fvoci.events WHERE channel = 'webhook' ORDER BY seq DESC LIMIT 1",
        )
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
        id
    };
    wait_until("github consumer passed the inbound event", || {
        let admin = admin.clone();
        async move {
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS (SELECT 1 FROM fvoci.processed_events WHERE consumer = 'github' AND event_id = $1)",
            )
            .bind(marker)
            .fetch_one(&admin)
            .await
            .unwrap()
        }
    })
    .await;
    assert_eq!(
        fake.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.0 == "PATCH")
            .count(),
        1
    );

    dispatcher.request_shutdown();
    dispatcher.join().await.unwrap();
    pool.close().await;
    admin.close().await;
    harness.cleanup().await;
}

// ---------------------------------------------------------------------------
// Document AI (local heuristics; no external model)

#[tokio::test]
async fn ai_routes_are_member_gated_and_use_the_document_markdown() {
    let harness = TestDb::bootstrap().await;
    let (_, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let state = app_state(&harness.app_url).await;
    let enabled = Arc::new(Integrations {
        ai: Some(AiConfig::new("ai-secret")),
        ..(*integrations(outbound("", &[]))).clone()
    });
    let app = fvoci_server::http::router_with_integrations(state, None, enabled);
    let disabled = self::app(&harness, integrations(outbound("", &[]))).await;

    let mut docs = Vec::new();
    for title in ["계획", "다른 문서"] {
        let (status, doc, _) = call(
            &app,
            "POST",
            &format!("/api/v1/workspaces/{workspace_id}/documents"),
            Some(json!({ "parentId": null, "title": title })),
            Some(&cookie),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{doc}");
        docs.push(doc["id"].as_str().unwrap().to_string());
    }
    let content = json!({"type": "doc", "content": [
        {"type": "heading", "attrs": {"level": 1}, "content": [{"type": "text", "text": "첫째 할 일"}]},
        {"type": "paragraph", "content": [{"type": "text", "text": "본문 🙂"}]},
        {"type": "heading", "attrs": {"level": 2}, "content": [{"type": "text", "text": "둘째 할 일"}]}
    ]});
    sqlx::query("UPDATE fvoci.documents SET content_json = $2 WHERE id = $1")
        .bind(Uuid::parse_str(&docs[0]).unwrap())
        .bind(&content)
        .execute(&admin)
        .await
        .unwrap();
    let path = |action: &str| format!("/api/v1/workspaces/{workspace_id}/ai/{action}");
    let body = json!({ "documentId": docs[0] });

    // Off: members get 503, outsiders 404 first.
    let (status, problem, _) = call(
        &disabled,
        "POST",
        &path("summarize"),
        Some(body.clone()),
        Some(&cookie),
    )
    .await;
    assert_eq!(
        (status, problem["code"].as_str()),
        (StatusCode::SERVICE_UNAVAILABLE, Some("ai_unavailable"))
    );
    let (status, _, _) = call(&app, "POST", &path("summarize"), Some(body.clone()), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    // A user with a live session but no membership: 404, even with AI on.
    let outsider = add_workspace_user(&admin, workspace_id, "member", "outsider").await;
    sqlx::query("DELETE FROM fvoci.memberships WHERE user_id = $1")
        .bind(outsider.user_id)
        .execute(&admin)
        .await
        .unwrap();
    for app in [&app, &disabled] {
        let (status, _, _) = call(
            app,
            "POST",
            &path("summarize"),
            Some(body.clone()),
            Some(&outsider.cookie),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    let (status, out, _) = call(
        &app,
        "POST",
        &path("summarize"),
        Some(body.clone()),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let summary = out["summary"].as_str().unwrap();
    assert!(
        summary.contains("첫째 할 일") && summary.contains("본문 🙂"),
        "{summary}"
    );
    assert!(
        !summary.contains("계획"),
        "title is not part of the body markdown"
    );

    let (status, out, _) = call(
        &app,
        "POST",
        &path("generate-tasks"),
        Some(body.clone()),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(out["titles"], json!(["첫째 할 일", "둘째 할 일"]));

    let (status, out, _) = call(
        &app,
        "POST",
        &path("suggest-links"),
        Some(body.clone()),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(out["documentIds"], json!([docs[1]]));

    let (status, _, _) = call(
        &app,
        "POST",
        &path("summarize"),
        Some(json!({"documentId": Uuid::now_v7()})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    admin.close().await;
    harness.cleanup().await;
}

// ---------------------------------------------------------------------------
// Round 2: GitHub sync finality, isolation and concurrency

/// Reads through RLS with the system context, as product workers do.
async fn system_scalar_uuid(admin: &PgPool, sql: &str, bind: Uuid) -> Option<Uuid> {
    let mut tx = admin.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.system_ctx', 'on', true)")
        .execute(&mut *tx)
        .await
        .unwrap();
    let value = sqlx::query_scalar(sql)
        .bind(bind)
        .fetch_optional(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    value
}

async fn last_task_event(admin: &PgPool, task_id: &str) -> Uuid {
    system_scalar_uuid(
        admin,
        "SELECT id FROM fvoci.events WHERE target_id = $1 AND verb = 'task.updated' ORDER BY xact DESC, seq DESC LIMIT 1",
        Uuid::parse_str(task_id).unwrap(),
    )
    .await
    .expect("task.updated event")
}

async fn processed(admin: &PgPool, consumer: &str, event_id: Uuid) -> bool {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM fvoci.processed_events WHERE consumer = $1 AND event_id = $2)",
    )
    .bind(consumer)
    .bind(event_id)
    .fetch_one(admin)
    .await
    .unwrap()
}

async fn github_failures(admin: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM fvoci.outbox_failures WHERE consumer = 'github'")
        .fetch_one(admin)
        .await
        .unwrap()
}

async fn install_row(admin: &PgPool, workspace_id: Uuid, installation_id: &str) {
    sqlx::query(
        "INSERT INTO fvoci.github_installations (id, workspace_id, installation_id) VALUES ($1, $2, $3)",
    )
    .bind(Uuid::now_v7())
    .bind(workspace_id)
    .bind(installation_id)
    .execute(admin)
    .await
    .unwrap();
}

/// A project with one task linked to `repo#number`; returns the task id and
/// the project's workflow statuses.
async fn linked_task(
    app: &Router,
    cookie: &str,
    workspace_id: Uuid,
    key: &str,
    repo: &str,
    number: i64,
) -> (String, Vec<Value>) {
    let project = create_project(app.clone(), cookie, workspace_id, key, "workspace").await;
    let project_id = project["id"].as_str().unwrap().to_string();
    let statuses = workflow_statuses(app, cookie, workspace_id, &project_id).await;
    let (status, task, _) = call(
        app,
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
        Some(json!({ "title": format!("{key} task") })),
        Some(cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    let task_id = task["id"].as_str().unwrap().to_string();
    let (status, body, _) = call(
        app,
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/github/issue-links"),
        Some(json!({ "taskId": task_id, "repo": repo, "issueNumber": number })),
        Some(cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    (task_id, statuses)
}

async fn move_task(app: &Router, cookie: &str, workspace_id: Uuid, task_id: &str, status: &str) {
    let (code, body, _) = call(
        app,
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/move"),
        Some(json!({ "statusId": status })),
        Some(cookie),
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{body}");
}

fn github_dispatcher(
    pool: &PgPool,
    config: Option<GithubConfig>,
    failure_backoff: Duration,
) -> OutboxDispatcherHandle {
    spawn_outbox_dispatcher(
        OutboxDispatcherSettings {
            failure_backoff,
            ..dispatcher_settings()
        },
        pool.clone(),
        vec![github_sync_consumer(config)],
    )
    .expect("dispatcher")
}

#[tokio::test]
async fn github_token_client_errors_are_final_and_do_not_hold_other_workspaces() {
    let harness = TestDb::bootstrap().await;
    let (_, cookie, _, ws_a) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let (fake, fake_addr) = start_fake_github().await;
    let app = app(&harness, github_integrations(fake_addr)).await;
    let ws_b = second_workspace(&app, &cookie, "gh-b").await;
    // A: the app was removed on GitHub but the `installation` webhook never
    // arrived, so the row stays and every token request answers 404.
    install_row(&admin, ws_a, "404001").await;
    fake.script_token("404001", &[404, 404, 404, 404, 404, 404]);
    install_row(&admin, ws_b, "42").await;
    let (task_a, statuses_a) = linked_task(&app, &cookie, ws_a, "GHA", "octo/a", 1).await;
    let (task_b, statuses_b) = linked_task(&app, &cookie, ws_b, "GHB", "octo/b", 2).await;

    // A retried event would wait 30 s before its next attempt and hold the
    // shared cursor that long; B must be PATCHed well before that.
    let pool = project_harness::app_pool(&harness).await;
    let dispatcher = github_dispatcher(
        &pool,
        Some(github_config(fake_addr)),
        Duration::from_secs(30),
    );
    move_task(
        &app,
        &cookie,
        ws_a,
        &task_a,
        &status_of(&statuses_a, "done"),
    )
    .await;
    move_task(
        &app,
        &cookie,
        ws_b,
        &task_b,
        &status_of(&statuses_b, "done"),
    )
    .await;
    let started = Instant::now();
    wait_until("B PATCHed", || {
        let fake = fake.clone();
        async move { fake.count("PATCH", "/repos/octo/b/issues/2") == 1 }
    })
    .await;
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "B waited {:?} behind A",
        started.elapsed()
    );
    assert_eq!(
        fake.count("POST", "/app/installations/404001/access_tokens"),
        1,
        "a 4xx token answer is not retried"
    );
    assert_eq!(fake.count("PATCH", "/repos/octo/a/issues/1"), 0);
    assert!(processed(&admin, "github", last_task_event(&admin, &task_a).await).await);
    assert_eq!(github_failures(&admin).await, 0);
    dispatcher.request_shutdown();
    dispatcher.join().await.unwrap();

    // 5xx and transport errors stay retryable: token 503, then PATCH 500,
    // then success. A canceled task closes the issue (source parity).
    let dispatcher = github_dispatcher(
        &pool,
        Some(github_config(fake_addr)),
        Duration::from_millis(100),
    );
    let tokens_before = fake.count("POST", "/app/installations/42/access_tokens");
    fake.script_token("42", &[503]);
    fake.script_patch("/repos/octo/b/issues/2", &[500]);
    move_task(
        &app,
        &cookie,
        ws_b,
        &task_b,
        &status_of(&statuses_b, "canceled"),
    )
    .await;
    let event = last_task_event(&admin, &task_b).await;
    wait_until("retried PATCH delivered", || {
        let admin = admin.clone();
        async move { processed(&admin, "github", event).await }
    })
    .await;
    assert_eq!(
        fake.count("POST", "/app/installations/42/access_tokens") - tokens_before,
        3,
        "token 503, token ok + PATCH 500, token ok + PATCH ok"
    );
    let patches: Vec<Value> = fake
        .calls()
        .into_iter()
        .filter(|c| c.0 == "PATCH" && c.1 == "/repos/octo/b/issues/2")
        .map(|c| c.2)
        .collect();
    assert_eq!(
        patches,
        vec![
            json!({"state": "closed"}),
            json!({"state": "closed"}),
            json!({"state": "closed"})
        ]
    );
    wait_until("failure row cleared", || {
        let admin = admin.clone();
        async move { github_failures(&admin).await == 0 }
    })
    .await;

    // PATCH 4xx is final: one request, no failure row.
    fake.script_patch("/repos/octo/b/issues/2", &[404]);
    move_task(
        &app,
        &cookie,
        ws_b,
        &task_b,
        &status_of(&statuses_b, "backlog"),
    )
    .await;
    let event = last_task_event(&admin, &task_b).await;
    wait_until("4xx PATCH settled", || {
        let admin = admin.clone();
        async move { processed(&admin, "github", event).await }
    })
    .await;
    assert_eq!(fake.count("PATCH", "/repos/octo/b/issues/2"), 4);
    assert_eq!(github_failures(&admin).await, 0);

    dispatcher.request_shutdown();
    dispatcher.join().await.unwrap();
    pool.close().await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn github_sync_does_not_replay_changes_made_while_unconfigured() {
    let harness = TestDb::bootstrap().await;
    let (_, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let (fake, fake_addr) = start_fake_github().await;
    let app = app(&harness, github_integrations(fake_addr)).await;
    install_row(&admin, workspace_id, "42").await;
    let (task_id, statuses) = linked_task(&app, &cookie, workspace_id, "OFF", "octo/off", 3).await;
    let pool = project_harness::app_pool(&harness).await;

    // App not configured on this server: the cursor still moves.
    let dispatcher = github_dispatcher(&pool, None, Duration::from_millis(100));
    move_task(
        &app,
        &cookie,
        workspace_id,
        &task_id,
        &status_of(&statuses, "done"),
    )
    .await;
    let skipped = last_task_event(&admin, &task_id).await;
    let skipped_seq: i64 = sqlx::query_scalar("SELECT seq FROM fvoci.events WHERE id = $1")
        .bind(skipped)
        .fetch_one(&admin)
        .await
        .unwrap();
    wait_until("cursor passed the event", || {
        let admin = admin.clone();
        async move {
            sqlx::query_scalar::<_, bool>(
                r#"
                SELECT (c.last_xact, c.last_seq) >= (e.xact, e.seq)
                FROM fvoci.outbox_consumers c, fvoci.events e
                WHERE c.consumer = 'github' AND e.id = $1
                "#,
            )
            .bind(skipped)
            .fetch_one(&admin)
            .await
            .unwrap()
        }
    })
    .await;
    assert!(skipped_seq > 0);
    dispatcher.request_shutdown();
    dispatcher.join().await.unwrap();

    // Configured later: only changes from now on are pushed.
    let dispatcher = github_dispatcher(
        &pool,
        Some(github_config(fake_addr)),
        Duration::from_millis(100),
    );
    move_task(
        &app,
        &cookie,
        workspace_id,
        &task_id,
        &status_of(&statuses, "backlog"),
    )
    .await;
    let event = last_task_event(&admin, &task_id).await;
    wait_until("new change pushed", || {
        let admin = admin.clone();
        async move { processed(&admin, "github", event).await }
    })
    .await;
    let patches: Vec<Value> = fake
        .calls()
        .into_iter()
        .filter(|c| c.0 == "PATCH")
        .map(|c| c.2)
        .collect();
    assert_eq!(patches, vec![json!({"state": "open"})]);

    dispatcher.request_shutdown();
    dispatcher.join().await.unwrap();
    pool.close().await;
    admin.close().await;
    harness.cleanup().await;
}

fn issue_payload(action: &str, installation: u64, number: i64, title: &str, state: &str) -> Value {
    json!({
        "action": action,
        "installation": { "id": installation },
        "repository": { "full_name": "octo/race" },
        "issue": { "number": number, "title": title, "state": state },
    })
}

async fn links_for(admin: &PgPool, number: i32) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM fvoci.github_issue_links WHERE repo = 'octo/race' AND issue_number = $1",
    )
    .bind(number)
    .fetch_one(admin)
    .await
    .unwrap()
}

#[tokio::test]
async fn github_concurrent_and_redelivered_webhooks_apply_once() {
    let harness = TestDb::bootstrap().await;
    let (_, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let (_fake, fake_addr) = start_fake_github().await;
    let app = app(&harness, github_integrations(fake_addr)).await;
    create_project(app.clone(), &cookie, workspace_id, "RACE", "workspace").await;
    install_row(&admin, workspace_id, "42").await;

    // The same delivery sent twice at once: applied once.
    let opened = issue_payload("opened", 42, 1, "once", "open");
    let delivery = Uuid::now_v7().to_string();
    let (a, b) = tokio::join!(
        github_hook(&app, "issues", &delivery, &opened),
        github_hook(&app, "issues", &delivery, &opened)
    );
    assert_eq!((a, b), (StatusCode::OK, StatusCode::OK));
    assert_eq!(links_for(&admin, 1).await, 1);

    // `opened` and `edited` for the same unlinked issue at once: the second
    // waits for the first one's link instead of failing on the unique index.
    for number in 2..8 {
        let (first_id, second_id) = (Uuid::now_v7().to_string(), Uuid::now_v7().to_string());
        let first = issue_payload("opened", 42, number, "first", "open");
        let second = issue_payload("edited", 42, number, "second", "open");
        let (a, b) = tokio::join!(
            github_hook(&app, "issues", &first_id, &first),
            github_hook(&app, "issues", &second_id, &second)
        );
        assert_eq!((a, b), (StatusCode::OK, StatusCode::OK), "issue {number}");
        assert_eq!(links_for(&admin, number as i32).await, 1, "issue {number}");
    }

    // A delivery whose apply fails is not marked: GitHub's redelivery of the
    // same id is applied.
    project_harness::install_insert_fail_trigger(&admin, "tasks", "fail_task_insert").await;
    let failing = issue_payload("opened", 42, 20, "redelivered", "open");
    let delivery = Uuid::now_v7().to_string();
    assert_eq!(
        github_hook(&app, "issues", &delivery, &failing).await,
        StatusCode::INTERNAL_SERVER_ERROR
    );
    assert_eq!(links_for(&admin, 20).await, 0);
    project_harness::drop_insert_fail_trigger(&admin, "tasks", "fail_task_insert").await;
    assert_eq!(
        github_hook(&app, "issues", &delivery, &failing).await,
        StatusCode::OK
    );
    assert_eq!(links_for(&admin, 20).await, 1);
    assert_eq!(
        github_hook(&app, "issues", &delivery, &failing).await,
        StatusCode::OK
    );
    assert_eq!(links_for(&admin, 20).await, 1);

    // `installation.suspend` removes the link; later events are ignored.
    assert_eq!(
        github_hook(
            &app,
            "installation",
            &Uuid::now_v7().to_string(),
            &json!({"action": "suspend", "installation": {"id": 42}})
        )
        .await,
        StatusCode::OK
    );
    let installs: i64 = sqlx::query_scalar("SELECT count(*) FROM fvoci.github_installations")
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_eq!(installs, 0);
    assert_eq!(
        github_hook(
            &app,
            "issues",
            &Uuid::now_v7().to_string(),
            &issue_payload("opened", 42, 30, "after suspend", "open")
        )
        .await,
        StatusCode::OK
    );
    assert_eq!(links_for(&admin, 30).await, 0);

    admin.close().await;
    harness.cleanup().await;
}

// ---------------------------------------------------------------------------
// Round 2: webhook sender independence

#[tokio::test]
async fn slow_webhook_receiver_does_not_hold_the_notification_consumer() {
    let harness = TestDb::bootstrap().await;
    let (_, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let (receiver, addr) = start_receiver().await;
    let outbound = outbound("127.0.0.1", &[]);
    let app = app(&harness, integrations(outbound.clone())).await;
    let (hook_id, _) = create_hook(
        &app,
        &cookie,
        workspace_id,
        &format!("http://{addr}/tarpit"),
        &["project.created"],
    )
    .await;
    // The tarpit answers after 60 s; the request budget is 10 s (source).
    receiver.script("/tarpit", &[Reply::Slow(Duration::from_secs(60))]);
    let member = add_workspace_user(&admin, workspace_id, "member", "n").await;

    let pool = project_harness::app_pool(&harness).await;
    let delivery = Delivery::start(
        &pool,
        outbound,
        Duration::from_secs(10),
        vec![fvoci_server::notifications::notifications_consumer()],
    );
    let project = create_project(app.clone(), &cookie, workspace_id, "SLOW", "workspace").await;
    wait_until("tarpit request in flight", || {
        let receiver = receiver.clone();
        async move { receiver.count() == 1 }
    })
    .await;

    // While that request is held, a notification is produced promptly.
    let project_id = project["id"].as_str().unwrap();
    let (status, task, _) = call(
        &app,
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
        Some(json!({ "title": "notify me" })),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    let task_id = task["id"].as_str().unwrap();
    let (status, body, _) = call(
        &app,
        "PATCH",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}"),
        Some(json!({ "assigneeIds": [member.user_id] })),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let started = Instant::now();
    wait_until("member notification", || {
        let admin = admin.clone();
        let user = member.user_id;
        async move {
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM fvoci.notifications WHERE user_id = $1",
            )
            .bind(user)
            .fetch_one(&admin)
            .await
            .unwrap()
                > 0
        }
    })
    .await;
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "notification waited {:?} behind the webhook send",
        started.elapsed()
    );
    // The webhook attempt is still in flight (claimed, not yet recorded).
    let rows = deliveries(&admin, hook_id).await;
    assert_eq!(rows.len(), 1);
    assert_eq!((rows[0].status.as_str(), rows[0].attempt), ("pending", 0));

    delivery.stop().await;
    pool.close().await;
    admin.close().await;
    harness.cleanup().await;
}

// ---------------------------------------------------------------------------
// Round 2: RLS catalog and cross-tenant access with the app role

const INTEGRATION_TABLES: [&str; 6] = [
    "webhooks",
    "webhook_deliveries",
    "github_installations",
    "github_install_states",
    "github_issue_links",
    "github_deliveries",
];

#[tokio::test]
async fn integration_tables_force_rls_and_isolate_tenants() {
    let harness = TestDb::bootstrap().await;
    let (_, cookie, owner_id, ws_a) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let (_fake, fake_addr) = start_fake_github().await;
    let app = app(&harness, github_integrations(fake_addr)).await;
    let ws_b = second_workspace(&app, &cookie, "rls-b").await;

    for table in INTEGRATION_TABLES {
        let (enabled, forced): (bool, bool) = sqlx::query_as(
            r#"
            SELECT c.relrowsecurity, c.relforcerowsecurity
            FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
            WHERE n.nspname = 'fvoci' AND c.relname = $1
            "#,
        )
        .bind(table)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert!(enabled && forced, "{table} must FORCE RLS");
        let policies: Vec<(String, String)> = sqlx::query_as(
            "SELECT policyname::text, qual FROM pg_policies WHERE schemaname = 'fvoci' AND tablename = $1",
        )
        .bind(table)
        .fetch_all(&admin)
        .await
        .unwrap();
        assert_eq!(policies.len(), 1, "{table}: {policies:?}");
        let qual = &policies[0].1;
        match table {
            "github_deliveries" => assert!(
                !qual.contains("app_tenant_id") && qual.contains("app_system_ctx_on"),
                "{table}: {qual}"
            ),
            "github_issue_links" => assert!(
                qual.contains("app_tenant_id") && !qual.contains("app_system_ctx_on"),
                "{table}: {qual}"
            ),
            _ => assert!(
                qual.contains("app_tenant_id") && qual.contains("app_system_ctx_on"),
                "{table}: {qual}"
            ),
        }
    }

    // One row of each tenant-scoped table in workspace A.
    let (hook_id, _) = create_hook(
        &app,
        &cookie,
        ws_a,
        "https://example.com/rls",
        &["task.created"],
    )
    .await;
    let event = insert_event(&admin, ws_a, "task.created", None, json!({})).await;
    sqlx::query(
        "INSERT INTO fvoci.webhook_deliveries (id, workspace_id, webhook_id, event_id, status, next_attempt_at) VALUES ($1, $2, $3, $4, 'pending', now())",
    )
    .bind(Uuid::now_v7())
    .bind(ws_a)
    .bind(hook_id)
    .bind(event)
    .execute(&admin)
    .await
    .unwrap();
    install_row(&admin, ws_a, "42").await;
    start_install(&app, &cookie, ws_a).await;
    linked_task(&app, &cookie, ws_a, "RLS", "octo/rls", 1).await;
    sqlx::query("INSERT INTO fvoci.github_deliveries (delivery_id) VALUES ($1)")
        .bind(Uuid::now_v7())
        .execute(&admin)
        .await
        .unwrap();

    let pool = project_harness::app_pool(&harness).await;
    let is_superuser: bool = sqlx::query_scalar(
        "SELECT rolsuper OR rolbypassrls FROM pg_roles WHERE rolname = current_user",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        !is_superuser,
        "checks must run as the unprivileged app role"
    );
    let count = |table: &'static str, tenant: Option<Uuid>, system: bool| {
        let pool = pool.clone();
        async move {
            let mut tx = pool.begin().await.unwrap();
            if let Some(tenant) = tenant {
                fvoci_server::db::context::set_tenant(&mut tx, tenant)
                    .await
                    .unwrap();
            }
            if system {
                fvoci_server::db::context::set_system(&mut tx)
                    .await
                    .unwrap();
            }
            let n: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM fvoci.{table}"))
                .fetch_one(&mut *tx)
                .await
                .unwrap();
            tx.rollback().await.unwrap();
            n
        }
    };
    for table in INTEGRATION_TABLES {
        let own = count(table, Some(ws_a), false).await;
        let other = count(table, Some(ws_b), false).await;
        let none = count(table, None, false).await;
        let system = count(table, None, true).await;
        match table {
            "github_deliveries" => assert_eq!((own, other, none, system), (0, 0, 0, 1), "{table}"),
            "github_issue_links" => assert_eq!((own, other, none, system), (1, 0, 0, 0), "{table}"),
            _ => assert_eq!((own, other, none, system), (1, 0, 0, 1), "{table}"),
        }
    }

    // Writes naming another tenant's workspace are refused by the policy.
    let mut tx = pool.begin().await.unwrap();
    fvoci_server::db::context::set_tenant(&mut tx, ws_b)
        .await
        .unwrap();
    let err = sqlx::query(
        "INSERT INTO fvoci.github_installations (id, workspace_id, installation_id) VALUES ($1, $2, '77')",
    )
    .bind(Uuid::now_v7())
    .bind(ws_a)
    .execute(&mut *tx)
    .await
    .expect_err("cross-tenant insert");
    assert!(err.to_string().contains("row-level security"), "{err}");
    tx.rollback().await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    fvoci_server::db::context::set_tenant(&mut tx, ws_b)
        .await
        .unwrap();
    let err = sqlx::query(
        "INSERT INTO fvoci.github_install_states (nonce_hash, workspace_id, user_id, session_id, expires_at) VALUES ($1, $2, $3, $4, now())",
    )
    .bind("a".repeat(64))
    .bind(ws_a)
    .bind(owner_id)
    .bind(Uuid::now_v7())
    .execute(&mut *tx)
    .await
    .expect_err("cross-tenant insert");
    assert!(err.to_string().contains("row-level security"), "{err}");
    tx.rollback().await.unwrap();
    // Tenant B cannot update or delete tenant A's rows (they are invisible).
    let mut tx = pool.begin().await.unwrap();
    fvoci_server::db::context::set_tenant(&mut tx, ws_b)
        .await
        .unwrap();
    for table in ["webhooks", "github_installations", "github_issue_links"] {
        let n = sqlx::query(&format!("DELETE FROM fvoci.{table}"))
            .execute(&mut *tx)
            .await
            .unwrap()
            .rows_affected();
        assert_eq!(n, 0, "{table}");
    }
    tx.rollback().await.unwrap();

    pool.close().await;
    admin.close().await;
    harness.cleanup().await;
}

// ---------------------------------------------------------------------------
// Round 2: AI document permission and rate limit

#[tokio::test]
async fn ai_routes_follow_document_permission_and_rate_limit() {
    let harness = TestDb::bootstrap().await;
    let (_, cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let state = app_state(&harness.app_url).await;
    let enabled = Arc::new(Integrations {
        ai: Some(AiConfig::new("ai-secret")),
        ..(*integrations(outbound("", &[]))).clone()
    });
    let app = fvoci_server::http::router_with_integrations(state, None, enabled);
    let path = |action: &str| format!("/api/v1/workspaces/{workspace_id}/ai/{action}");

    let (status, wiki, _) = call(
        &app,
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents"),
        Some(json!({ "parentId": null, "title": "위키" })),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{wiki}");
    let wiki_id = wiki["id"].as_str().unwrap().to_string();
    // A private project with one document; the owner is its only member.
    let project = create_project(app.clone(), &cookie, workspace_id, "PRV", "private").await;
    let project_id = Uuid::parse_str(project["id"].as_str().unwrap()).unwrap();
    let project_doc = Uuid::now_v7();
    project_harness::insert_project_document(
        &admin,
        workspace_id,
        project_id,
        project_doc,
        owner_id,
        999,
    )
    .await;

    // Project documents resolve through the project (source requirePermission).
    let (status, out, _) = call(
        &app,
        "POST",
        &path("summarize"),
        Some(json!({ "documentId": project_doc })),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let (status, out, _) = call(
        &app,
        "POST",
        &path("suggest-links"),
        Some(json!({ "documentId": wiki_id })),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    // Every live document of the project (its home page and ours), and
    // nothing else: the wiki page itself is excluded.
    let project_docs: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM fvoci.documents WHERE project_id = $1 AND deleted_at IS NULL",
    )
    .bind(project_id)
    .fetch_all(&admin)
    .await
    .unwrap();
    assert!(project_docs.contains(&project_doc));
    let mut suggested: Vec<String> = out["documentIds"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    suggested.sort();
    let mut expected: Vec<String> = project_docs.iter().map(Uuid::to_string).collect();
    expected.sort();
    assert_eq!(suggested, expected);

    // A member outside the private project neither reads nor sees it.
    let member = add_workspace_user(&admin, workspace_id, "member", "ai-m").await;
    let (status, _, _) = call(
        &app,
        "POST",
        &path("generate-tasks"),
        Some(json!({ "documentId": project_doc })),
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, out, _) = call(
        &app,
        "POST",
        &path("suggest-links"),
        Some(json!({ "documentId": wiki_id })),
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(out["documentIds"], json!([]));
    let (status, _, _) = call(
        &app,
        "POST",
        &path("suggest-links"),
        Some(json!({ "documentId": project_doc })),
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // A guest has no wiki permission: the wiki document is hidden too.
    let guest = add_workspace_user(&admin, workspace_id, "guest", "ai-g").await;
    for action in ["summarize", "generate-tasks", "suggest-links"] {
        let (status, _, _) = call(
            &app,
            "POST",
            &path(action),
            Some(json!({ "documentId": wiki_id })),
            Some(&guest.cookie),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{action}");
    }

    // 10 requests per 5 minutes per user (source), counted before the
    // document lookup; the 11th is 429 with Retry-After.
    let limited = add_workspace_user(&admin, workspace_id, "member", "ai-rl").await;
    for i in 0..10 {
        let (status, _, _) = call(
            &app,
            "POST",
            &path("summarize"),
            Some(json!({ "documentId": wiki_id })),
            Some(&limited.cookie),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "request {i}");
    }
    let (status, _, headers) = call(
        &app,
        "POST",
        &path("summarize"),
        Some(json!({ "documentId": wiki_id })),
        Some(&limited.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert!(headers.contains_key("retry-after"));
    // Another user is not affected.
    let (status, _, _) = call(
        &app,
        "POST",
        &path("summarize"),
        Some(json!({ "documentId": wiki_id })),
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    admin.close().await;
    harness.cleanup().await;
}
