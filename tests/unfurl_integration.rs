#![cfg(feature = "db-tests")]

//! Workspace unfurl HTTP contracts: auth, membership, revocation, SSRF,
//! live `embed.hosts`, and rate limits. Outbound GETs use an injected client
//! (no public internet, no product allow-private backdoor).

#[path = "support/project_harness.rs"]
mod project_harness;

use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use fvoci_server::integrations::outbound::{
    GetClient, GetFuture, Outbound, PinnedGet, Resolve, ResolveFuture,
};
use fvoci_server::integrations::unfurl::UNFURL_USER_AGENT;
use fvoci_server::integrations::Integrations;
use project_harness::{
    add_workspace_user, admin_pool, app_state, setup_session, test_peer, TestDb,
};
use serde_json::{json, Value};
use tokio::sync::Barrier;
use tower::ServiceExt;
use url::Url;
use uuid::Uuid;

struct PublicDns;

impl Resolve for PublicDns {
    fn lookup<'a>(&'a self, _host: &'a str) -> ResolveFuture<'a> {
        Box::pin(async { Ok(vec!["1.1.1.1".parse().unwrap()]) })
    }
}

#[derive(Clone)]
struct Scripted {
    hops: Arc<Mutex<HashMap<String, VecDeque<PinnedGet>>>>,
    fetched: Arc<Mutex<u32>>,
}

impl Scripted {
    fn new(hops: &[(&str, PinnedGet)]) -> Self {
        let mut map = HashMap::new();
        for (url, hop) in hops {
            map.entry((*url).to_string())
                .or_insert_with(VecDeque::new)
                .push_back(hop.clone());
        }
        Self {
            hops: Arc::new(Mutex::new(map)),
            fetched: Arc::new(Mutex::new(0)),
        }
    }
}

impl GetClient for Scripted {
    fn get<'a>(
        &'a self,
        url: &'a Url,
        _pinned: SocketAddr,
        _headers: &'a [(&'a str, String)],
        _timeout: Duration,
    ) -> GetFuture<'a> {
        *self.fetched.lock().unwrap() += 1;
        let next = self
            .hops
            .lock()
            .unwrap()
            .get_mut(url.as_str())
            .and_then(|q| q.pop_front());
        Box::pin(async move {
            Ok(next.unwrap_or(PinnedGet {
                status: 200,
                location: None,
                body: b"<html></html>".to_vec(),
            }))
        })
    }
}

struct Gate {
    started: Arc<Barrier>,
    release: Arc<Barrier>,
}

impl GetClient for Gate {
    fn get<'a>(
        &'a self,
        _url: &'a Url,
        _pinned: SocketAddr,
        _headers: &'a [(&'a str, String)],
        _timeout: Duration,
    ) -> GetFuture<'a> {
        let started = self.started.clone();
        let release = self.release.clone();
        Box::pin(async move {
            started.wait().await;
            release.wait().await;
            Ok(PinnedGet {
                status: 200,
                location: None,
                body: br#"<meta property="og:title" content="late">"#.to_vec(),
            })
        })
    }
}

fn html(body: &str) -> PinnedGet {
    PinnedGet {
        status: 200,
        location: None,
        body: body.as_bytes().to_vec(),
    }
}

fn integrations_with(client: Arc<dyn GetClient>) -> Arc<Integrations> {
    Arc::new(Integrations {
        encryption_keys: None,
        outbound: Outbound::with_get_client(Default::default(), Arc::new(PublicDns), client),
        github: None,
        ai: None,
    })
}

async fn app(harness: &TestDb, client: Arc<dyn GetClient>) -> Router {
    fvoci_server::http::router_with_integrations(
        app_state(&harness.app_url).await,
        None,
        integrations_with(client),
    )
}

async fn call(
    app: &Router,
    method: &str,
    path: &str,
    cookie: Option<&str>,
    bearer: Option<&str>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("origin", "http://localhost");
    if let Some(cookie) = cookie {
        builder = builder.header("cookie", format!("fvoci_session={cookie}"));
    }
    if let Some(bearer) = bearer {
        builder = builder.header("authorization", format!("Bearer {bearer}"));
    }
    let mut request = builder.body(Body::empty()).unwrap();
    request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(test_peer()));
    let response = app.clone().oneshot(request).await.expect("response");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap_or_default();
    let json = serde_json::from_slice(&bytes).unwrap_or(json!({}));
    (status, json)
}

fn unfurl_path(workspace_id: Uuid, url: &str) -> String {
    format!(
        "/api/v1/workspaces/{workspace_id}/unfurl?url={}",
        urlencoding_query(url)
    )
}

fn urlencoding_query(value: &str) -> String {
    let mut out = String::new();
    for b in value.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[tokio::test]
async fn unfurl_route_contracts_auth_ssrf_and_oembed() {
    let harness = TestDb::bootstrap().await;
    let (_, cookie, _, workspace_id) = setup_session(&harness).await;
    let client = Scripted::new(&[(
        "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
        html("<html><title>yt</title></html>"),
    )]);
    let app = app(&harness, Arc::new(client.clone())).await;

    let (status, body) = call(
        &app,
        "GET",
        &unfurl_path(workspace_id, "https://www.youtube.com/watch?v=dQw4w9WgXcQ"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let html = body["html"].as_str().unwrap_or("");
    assert!(html.contains("youtube.com/embed/dQw4w9WgXcQ"), "{body}");
    assert!(html.contains("sandbox="));

    let (status, body) = call(
        &app,
        "GET",
        &unfurl_path(workspace_id, "https://example.com/page"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.get("html").is_none(), "{body}");
    assert!(!body.to_string().to_lowercase().contains("<iframe"));

    for bad in ["javascript:alert(1)", "ftp://files.example/x"] {
        let (status, body) = call(
            &app,
            "GET",
            &unfurl_path(workspace_id, bad),
            Some(&cookie),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
        assert_eq!(body["code"], "invalid_input");
        assert!(!body.to_string().to_lowercase().contains("<iframe"));
    }

    let (status, body) = call(
        &app,
        "GET",
        &unfurl_path(workspace_id, "http://127.0.0.1/secret"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "invalid_input");

    let (status, _) = call(
        &app,
        "GET",
        &unfurl_path(workspace_id, "https://example.com/page"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let admin = admin_pool(&harness).await;
    let outsider = add_workspace_user(&admin, workspace_id, "member", "outsider").await;
    sqlx::query("DELETE FROM fvoci.memberships WHERE user_id = $1")
        .bind(outsider.user_id)
        .execute(&admin)
        .await
        .unwrap();
    let (status, _) = call(
        &app,
        "GET",
        &unfurl_path(workspace_id, "https://example.com/page"),
        Some(&outsider.cookie),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let patch = Request::builder()
        .method("PATCH")
        .uri("/api/v1/admin/instance-settings")
        .header("origin", "http://localhost")
        .header("content-type", "application/json")
        .header("cookie", format!("fvoci_session={cookie}"))
        .body(Body::from(
            json!({"embed":{"hosts":["example.com"]}}).to_string(),
        ))
        .unwrap();
    let mut patch = patch;
    patch
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(test_peer()));
    let patched = app.clone().oneshot(patch).await.expect("patch");
    assert_eq!(patched.status(), StatusCode::OK, "embed hosts patch");

    let (status, body) = call(
        &app,
        "GET",
        &unfurl_path(workspace_id, "https://www.youtube.com/watch?v=dQw4w9WgXcQ"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body.get("html").is_none(),
        "custom hosts dropped youtube: {body}"
    );

    harness.cleanup().await;
}

#[tokio::test]
async fn unfurl_rechecks_membership_after_fetch() {
    let harness = TestDb::bootstrap().await;
    let (_, cookie, user_id, workspace_id) = setup_session(&harness).await;
    let started = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let app = app(
        &harness,
        Arc::new(Gate {
            started: started.clone(),
            release: release.clone(),
        }),
    )
    .await;
    let admin = admin_pool(&harness).await;
    let path = unfurl_path(workspace_id, "https://example.com/slow");
    let mut request = Request::builder()
        .method("GET")
        .uri(&path)
        .header("origin", "http://localhost")
        .header("cookie", format!("fvoci_session={cookie}"))
        .body(Body::empty())
        .unwrap();
    request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(test_peer()));
    let pending = tokio::spawn(async move {
        let response = app.oneshot(request).await.expect("response");
        response.status()
    });
    started.wait().await;
    sqlx::query("DELETE FROM fvoci.memberships WHERE user_id = $1")
        .bind(user_id)
        .execute(&admin)
        .await
        .unwrap();
    release.wait().await;
    let status = pending.await.expect("join");
    assert_eq!(status, StatusCode::NOT_FOUND);
    harness.cleanup().await;
}

#[tokio::test]
async fn unfurl_pat_and_user_rate_limit() {
    let harness = TestDb::bootstrap().await;
    let (_, cookie, _, workspace_id) = setup_session(&harness).await;
    let client = Scripted::new(&[]);
    let app = app(&harness, Arc::new(client)).await;

    let create = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/workspaces/{workspace_id}/api-tokens"))
        .header("origin", "http://localhost")
        .header("content-type", "application/json")
        .header("cookie", format!("fvoci_session={cookie}"))
        .body(Body::from(
            json!({"name":"unfurl","scopes":["documents.read"]}).to_string(),
        ))
        .unwrap();
    let mut create = create;
    create
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(test_peer()));
    let created = app.clone().oneshot(create).await.expect("token");
    assert_eq!(created.status(), StatusCode::CREATED);
    let bytes = axum::body::to_bytes(created.into_body(), usize::MAX)
        .await
        .unwrap();
    let token: Value = serde_json::from_slice(&bytes).unwrap();
    let secret = token["token"].as_str().expect("token secret");

    let (status, body) = call(
        &app,
        "GET",
        &unfurl_path(workspace_id, "https://example.com/pat"),
        None,
        Some(secret),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["kind"], "og");

    let mut limited = None;
    for i in 0..31 {
        let (status, body) = call(
            &app,
            "GET",
            &unfurl_path(workspace_id, &format!("https://example.com/r{i}")),
            Some(&cookie),
            None,
        )
        .await;
        if status == StatusCode::TOO_MANY_REQUESTS {
            limited = Some(body);
            break;
        }
    }
    let body = limited.expect("user rate limit");
    assert_eq!(body["code"], "rate_limit_exceeded");
    let _ = UNFURL_USER_AGENT;
    harness.cleanup().await;
}
