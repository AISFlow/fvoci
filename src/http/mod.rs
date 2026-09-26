pub mod authz;
pub mod cookie;
pub mod guard;
pub mod json_input;
pub mod rate_limit;
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
use tower_http::trace::TraceLayer;

use axum::extract::State;
use axum_extra::extract::CookieJar;

use crate::auth::token::hash_token;
use crate::collab::transport::collab_entry;
use crate::error::{AppError, ProblemCode, API_PREFIX, SESSION_COOKIE};
use crate::http::authz::canonicalize_api_token_path;
use crate::http::state::AppState;

/// API paths a signed-in user without the latest required consents may still
/// use (source `CONSENT_ALLOWLIST`, `http-runtime.ts`; its health/ready/metrics
/// entries have no counterpart on this server's API router).
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
    match crate::db::legal::session_consent_pending(&state.auth.db.pool, &hash_token(&cookie)).await
    {
        Ok(false) => next.run(req).await,
        Ok(true) => AppError::from_code(ProblemCode::ConsentRequired).into_response(),
        Err(err) => {
            tracing::error!("consent gate: {}", err);
            AppError::internal().into_response()
        }
    }
}

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

/// The full router: integration and identity settings.
pub fn router_with_settings(
    state: AppState,
    static_dir: Option<PathBuf>,
    integrations: std::sync::Arc<crate::integrations::Integrations>,
    identity: std::sync::Arc<crate::identity::Identity>,
) -> Router {
    let public_origin = state.public_origin.clone();
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
        .merge(routes::invitations::router())
        .merge(routes::projects::router())
        .merge(routes::project_documents::router())
        .merge(routes::groups::router())
        .merge(routes::lookup::router())
        .merge(routes::notifications::router())
        .merge(routes::search::router())
        .merge(routes::tasks::router())
        .merge(routes::documents::router())
        .merge(routes::document_body::router())
        .merge(routes::import::router())
        .merge(routes::revisions::router())
        .merge(routes::attachments::router())
        .merge(routes::comments::router())
        .merge(routes::api_tokens::router())
        .merge(routes::ics::router())
        .merge(routes::integrations::router(integrations))
        .merge(routes::stars::router())
        .merge(routes::share::router())
        .merge(routes::admin::router())
        .merge(routes::legal::router())
        .merge(routes::collections::router())
        .merge(routes::document_tags::router())
        .merge(routes::project_views::router())
        .merge(collab)
        .layer(middleware::from_fn_with_state(state.clone(), consent_gate))
        .layer(middleware::from_fn(canonicalize_bearer_path))
        .with_state(state);

    let security = std::sync::Arc::new(security_headers::SecurityHeaders::new(
        &public_origin,
        static_dir.as_deref(),
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
    .layer(TraceLayer::new_for_http())
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
