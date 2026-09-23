pub mod cookie;
pub mod guard;
pub mod json_input;
pub mod rate_limit;
pub mod routes;
pub mod state;

use axum::Router;
use tower_http::trace::TraceLayer;

use crate::http::state::AppState;

pub fn router(state: AppState) -> Router {
    Router::new()
        .merge(routes::setup::router())
        .merge(routes::auth::router())
        .with_state(state)
        .layer(TraceLayer::new_for_http())
}
