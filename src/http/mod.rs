pub mod authz;
pub mod cookie;
pub mod guard;
pub mod json_input;
pub mod probes;
pub mod rate_limit;
pub mod request_trace;
pub mod routes;
pub mod security_headers;
pub mod spa_head;
pub mod state;
pub mod static_assets;

use std::path::PathBuf;

use axum::extract::Request;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;

use axum::extract::State;
use axum_extra::extract::CookieJar;

use crate::auth::token::hash_token;
use crate::collab::transport::collab_entry;
use crate::error::{AppError, ProblemCode, API_PREFIX, SESSION_COOKIE};
use crate::http::authz::canonicalize_api_token_path;
use crate::http::state::AppState;

/// API paths a signed-in user without the latest required consents may still
/// use (source `CONSENT_ALLOWLIST`, `http-runtime.ts`; its health/ready/metrics
/// entries are the `probes` routes, merged outside this gate).
const CONSENT_ALLOWLIST: &[&str] = &[
    "/setup",
    "/branding",
    "/legal",
    "/auth/consents",
    "/auth/logout",
    "/auth/withdraw",
    "/auth/cancel-withdraw",
    "/me/export",
];

fn consent_exempt(path: &str) -> bool {
    let Some(rest) = path.strip_prefix(API_PREFIX) else {
        return false;
    };
    CONSENT_ALLOWLIST.iter().any(|allowed| {
        rest == *allowed
            || rest
                .strip_prefix(allowed)
                .is_some_and(|r| r.starts_with('/'))
    })
}

/// Source 428 gate: a request carrying a live session cookie whose user has
/// not consented to the latest version of every required legal document is
/// refused with `consent_required` before any route runs (collab included).
/// API-token requests are not gated, like the source. A failing check fails
/// closed with 500 rather than letting the request through.
async fn consent_gate(
    State(state): State<AppState>,
    jar: CookieJar,
    req: Request,
    next: Next,
) -> Response {
    let Some(cookie) = jar.get(SESSION_COOKIE).map(|c| c.value().to_string()) else {
        return next.run(req).await;
    };
    if consent_exempt(req.uri().path()) {
        return next.run(req).await;
    }
    match crate::db::legal::session_consent_pending_backend(
        &state.auth.db.pool,
        &hash_token(&cookie),
    )
    .await
    {
        Ok(false) => next.run(req).await,
        Ok(true) => AppError::from_code(ProblemCode::ConsentRequired).into_response(),
        Err(err) => {
            tracing::error!("consent gate: {}", err);
            AppError::internal().into_response()
        }
    }
}

/// A request with a `Bearer` Authorization header whose path breaks the
/// Bearer path rule ([`canonicalize_api_token_path`]) gets 404 before its
/// handler runs. As an api-router layer it runs after route matching.
async fn canonicalize_bearer_path(req: Request, next: Next) -> Response {
    let has_bearer = req
        .headers()
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .map(|value| value.trim().starts_with("Bearer "))
        .unwrap_or(false);
    if has_bearer {
        if let Err(err) = canonicalize_api_token_path(req.uri().path()) {
            return err.into_response();
        }
    }
    next.run(req).await
}

pub fn router(state: AppState, static_dir: Option<PathBuf>) -> Router {
    router_with_integrations(
        state,
        static_dir,
        std::sync::Arc::new(crate::integrations::Integrations::disabled()),
    )
}

/// `router` with integration settings (webhook keys, GitHub App, AI).
pub fn router_with_integrations(
    state: AppState,
    static_dir: Option<PathBuf>,
    integrations: std::sync::Arc<crate::integrations::Integrations>,
) -> Router {
    let identity = std::sync::Arc::new(crate::identity::Identity::disabled(&state.public_origin));
    router_with_settings(state, static_dir, integrations, identity)
}

/// `router` with MFA / OIDC settings (`ENCRYPTION_KEYS`, `OIDC_*`).
pub fn router_with_identity(
    state: AppState,
    static_dir: Option<PathBuf>,
    identity: std::sync::Arc<crate::identity::Identity>,
) -> Router {
    router_with_settings(
        state,
        static_dir,
        std::sync::Arc::new(crate::integrations::Integrations::disabled()),
        identity,
    )
}

/// Integration and identity settings; `/metrics` denies every peer.
pub fn router_with_settings(
    state: AppState,
    static_dir: Option<PathBuf>,
    integrations: std::sync::Arc<crate::integrations::Integrations>,
    identity: std::sync::Arc<crate::identity::Identity>,
) -> Router {
    router_with_observability(
        state,
        static_dir,
        integrations,
        identity,
        std::sync::Arc::new(probes::Observability::new(Default::default())),
    )
}

/// The full router: integration, identity and probe/metrics settings.
pub fn router_with_observability(
    state: AppState,
    static_dir: Option<PathBuf>,
    integrations: std::sync::Arc<crate::integrations::Integrations>,
    identity: std::sync::Arc<crate::identity::Identity>,
    observability: std::sync::Arc<probes::Observability>,
) -> Router {
    let public_origin = state.public_origin.clone();
    let storage_origin = state.storage.presign_origin();
    let share_state = state.clone();
    let collab = Router::new()
        .route("/collab", get(collab_entry))
        .with_state(state.clone());
    let api = Router::new()
        .merge(routes::setup::router())
        .merge(routes::auth::router())
        .merge(routes::mfa::router(identity.clone()))
        .merge(routes::oidc::router(identity.clone()))
        .merge(routes::account::router())
        .merge(routes::workspaces::router())
        .merge(routes::personal_input::router())
        .merge(routes::personal_transfer::router())
        .merge(routes::invitations::router())
        .merge(routes::projects::router())
        .merge(routes::project_documents::router())
        .merge(routes::groups::router())
        .merge(routes::lookup::router())
        .merge(routes::notifications::router())
        .merge(routes::push::router())
        .merge(routes::search::router())
        .merge(routes::unfurl::router(integrations.clone()))
        .merge(routes::tasks::router())
        .merge(routes::task_ops::router())
        .merge(routes::task_timer::router())
        .merge(routes::documents::router())
        .merge(routes::document_body::router())
        .merge(routes::import::router())
        .merge(routes::native_archive::router())
        .merge(routes::revisions::router())
        .merge(routes::task_body::router())
        .merge(routes::attachments::router())
        .merge(routes::comments::router())
        .merge(routes::api_tokens::router())
        .merge(routes::api_docs::router())
        .merge(routes::ics::router())
        .merge(routes::zotero::router(integrations.clone()))
        .merge(routes::integrations::router(integrations))
        .merge(routes::stars::router())
        .merge(routes::share::router())
        .merge(routes::admin::router())
        .merge(routes::legal::router())
        .merge(routes::collections::router())
        .merge(routes::document_tags::router())
        .merge(routes::templates::router())
        .merge(routes::project_views::router())
        .merge(routes::streams::router())
        .merge(routes::task_layout::router())
        .merge(collab)
        // Layer order, outermost first: request_trace, record_http and the
        // security headers (added below, around every route), then, on the
        // routes above only, canonicalize_bearer_path and consent_gate. A
        // layer wraps only the routes already added, so the probes merged
        // next, the static assets and the fallback get neither api layer.
        .layer(middleware::from_fn_with_state(state.clone(), consent_gate))
        .layer(middleware::from_fn(canonicalize_bearer_path))
        .with_state(state.clone())
        .merge(probes::router(state, observability.clone()));

    let security = std::sync::Arc::new(security_headers::SecurityHeaders::new(
        &public_origin,
        static_dir.as_deref(),
        storage_origin.as_deref(),
    ));
    let app = match static_dir {
        Some(root) => api.merge(static_assets::static_router_with_share_head(
            root,
            share_state,
        )),
        None => api.fallback(static_assets::unknown_api_fallback),
    };
    app.layer(middleware::map_response(move |mut response: Response| {
        let security = security.clone();
        async move {
            security.apply(&mut response);
            response
        }
    }))
    .layer(middleware::from_fn_with_state(
        observability,
        probes::record_http,
    ))
    .layer(request_trace::layer())
}

#[cfg(test)]
mod consent_tests {
    use super::consent_exempt;

    #[test]
    fn allowlist_matches_whole_segments_only() {
        assert!(consent_exempt("/api/v1/auth/consents"));
        assert!(consent_exempt("/api/v1/auth/consents/pending"));
        assert!(consent_exempt("/api/v1/legal/terms/versions"));
        assert!(!consent_exempt("/api/v1/legalese"));
        assert!(!consent_exempt("/api/v1/auth/me"));
        assert!(!consent_exempt("/api/v1/instance"));
        assert!(!consent_exempt("/collab"));
    }
}

#[cfg(test)]
mod trace_tests {
    use std::net::SocketAddr;
    use std::sync::Arc;
    use std::time::Duration;

    use axum::body::Body;
    use axum::extract::ConnectInfo;
    use axum::http::Request;
    use tower::ServiceExt;

    use super::request_trace::{capture, UNMATCHED_ROUTE};
    use super::router;
    use super::state::AppState;

    const SHARE_TOKEN: &str = "synthShareTok6d1e";
    const ICS_TOKEN: &str = "synthIcsTok2a90";
    const OIDC_CODE: &str = "synthOidcCode4b7c";
    const OIDC_STATE: &str = "synthOidcState93f1";
    const SPA_TOKEN: &str = "synthSpaShareTok5e08";
    const ASSET_SEGMENT: &str = "synthAssetSeg0f3a";
    const UNKNOWN_SEGMENT: &str = "synthUnknownSeg71cd";
    const HEADER_SECRET: &str = "synthBasicSecret8c25";

    /// Unreachable database: DB-backed handlers fail fast with 500.
    fn app_state(storage_root: std::path::PathBuf) -> AppState {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(Duration::from_millis(200))
            .connect_lazy("postgres://fvoci:fvoci@127.0.0.1:1/none")
            .expect("lazy pool");
        AppState {
            realtime_mode: crate::config::RealtimeMode::On,
            native_engine: None,
            auth: Arc::new(crate::auth::AuthService {
                db: crate::db::Db::new(pool),
                password_keys: crate::auth::password::Keyring::parse(
                    r#"{"test":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#,
                    "test",
                )
                .expect("pepper"),
            }),
            branding_name: "FVOCI".to_string(),
            public_origin: "http://localhost".to_string(),
            cookie_secure: false,
            rate_limiter: super::rate_limit::RateLimiter::new(),
            storage: crate::attachments::LocalStorage::new(storage_root).into(),
            upload: crate::attachments::UploadLimits {
                part_size_bytes: crate::config::DEFAULT_UPLOAD_PART_SIZE_BYTES,
                max_file_size_bytes: crate::config::DEFAULT_UPLOAD_MAX_FILE_SIZE_BYTES,
                create_rate_per_5min: crate::config::DEFAULT_UPLOAD_CREATE_RATE_PER_5MIN,
                part_put_slots: crate::attachments::PartPutSlots::new(
                    crate::config::DEFAULT_UPLOAD_MAX_CONCURRENT_PARTS,
                ),
            },
            collab: None,
            meili: None,
            search_embedder: None,
            markdown: None,
            import_wake: None,
            import_extractor_available: false,
            preview_extract: None,
            quota: Default::default(),
            streams: AppState::fresh_streams(),
            mailer: Arc::new(crate::mail::Mailer::disabled()),
        }
    }

    /// The production router (API routes, static/SPA fallback) at DEBUG:
    /// share/ICS tokens, OIDC code/state and unmatched paths stay out of
    /// the log while route templates, status, latency and failures remain.
    #[tokio::test]
    async fn production_router_trace_omits_request_secrets() {
        let root = std::env::temp_dir().join(format!("fvoci-trace-test-{}", uuid::Uuid::now_v7()));
        let static_dir = root.join("static");
        std::fs::create_dir_all(&static_dir).expect("static dir");
        std::fs::write(static_dir.join("index.html"), "<html></html>").expect("index");
        let app = router(app_state(root.join("storage")), Some(static_dir));

        let (captured, guard) = capture::logs();
        let uris = [
            format!("/api/v1/share/{SHARE_TOKEN}"),
            format!("/api/v1/ics/{ICS_TOKEN}"),
            format!("/api/v1/auth/oidc/google/callback?code={OIDC_CODE}&state={OIDC_STATE}"),
            format!("/s/{SPA_TOKEN}?code={OIDC_CODE}"),
            format!("/assets/{ASSET_SEGMENT}.js?state={OIDC_STATE}"),
            format!("/api/v1/{UNKNOWN_SEGMENT}?code={OIDC_CODE}"),
        ];
        for uri in &uris {
            let mut request = Request::builder()
                .uri(uri.as_str())
                .header("authorization", format!("Basic {HEADER_SECRET}"))
                .body(Body::empty())
                .unwrap();
            request
                .extensions_mut()
                .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 40000))));
            let response = app.clone().oneshot(request).await.unwrap();
            axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
        }
        drop(guard);
        let _ = std::fs::remove_dir_all(&root);

        let log = captured.text();
        for secret in [
            SHARE_TOKEN,
            ICS_TOKEN,
            OIDC_CODE,
            OIDC_STATE,
            SPA_TOKEN,
            ASSET_SEGMENT,
            UNKNOWN_SEGMENT,
            HEADER_SECRET,
            "uri=",
        ] {
            assert!(
                !log.contains(secret),
                "{secret:?} leaked into trace:\n{log}"
            );
        }
        for route in [
            "route=/api/v1/share/{token}",
            "route=/api/v1/ics/{token}",
            "route=/api/v1/auth/oidc/{provider}/callback",
        ] {
            assert!(log.contains(route), "{route} missing:\n{log}");
        }
        let unmatched = format!("route={UNMATCHED_ROUTE}");
        assert!(log.contains(&unmatched), "{log}");
        assert_eq!(
            log.matches("finished processing request").count(),
            uris.len(),
            "{log}"
        );
        for field in ["method=GET", "status=", "latency="] {
            assert!(log.contains(field), "{field} missing:\n{log}");
        }
        assert!(log.contains("response failed"), "{log}");
    }
}
