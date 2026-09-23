pub mod cookie;
pub mod guard;
pub mod json_input;
pub mod rate_limit;
pub mod routes;
pub mod state;
pub mod static_assets;

use std::path::PathBuf;

use axum::Router;
use tower_http::trace::TraceLayer;

use crate::http::state::AppState;

pub fn router(state: AppState, static_dir: Option<PathBuf>) -> Router {
    let api = Router::new()
        .merge(routes::setup::router())
        .merge(routes::auth::router())
        .merge(routes::workspaces::router())
        .with_state(state);

    let app = match static_dir {
        Some(root) => api.merge(static_assets::static_router(root)),
        None => api.fallback(static_assets::unknown_api_fallback),
    };
    app.layer(TraceLayer::new_for_http())
}
