pub mod authz;
pub mod cookie;
pub mod guard;
pub mod json_input;
pub mod rate_limit;
pub mod routes;
pub mod state;
pub mod static_assets;

use std::path::PathBuf;

use axum::extract::Request;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use tower_http::trace::TraceLayer;

use crate::collab::transport::collab_entry;
use crate::http::authz::canonicalize_api_token_path;
use crate::http::state::AppState;

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
    let collab = Router::new()
        .route("/collab", get(collab_entry))
        .with_state(state.clone());
    let api = Router::new()
        .merge(routes::setup::router())
        .merge(routes::auth::router())
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
        .merge(routes::revisions::router())
        .merge(routes::attachments::router())
        .merge(routes::comments::router())
        .merge(routes::api_tokens::router())
        .merge(routes::ics::router())
        .merge(collab)
        .layer(middleware::from_fn(canonicalize_bearer_path))
        .with_state(state);

    let app = match static_dir {
        Some(root) => api.merge(static_assets::static_router(root)),
        None => api.fallback(static_assets::unknown_api_fallback),
    };
    app.layer(TraceLayer::new_for_http())
}
