//! `ready` and `ping`, which say that the server can serve and what it is. They are
//! under `/api/v1` like the rest of the API and need no session.

use axum::extract::State;
use fvoci_model::Problem;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::contract::tag;

use super::{ApiResult, json_response};
use crate::app::{App, Readiness, ServerMetadata};
use crate::error::AppError;

pub(super) fn routes() -> OpenApiRouter<App> {
    OpenApiRouter::new()
        .routes(routes!(handle_ready))
        .routes(routes!(handle_ping))
}

/// Whether the server can serve
///
/// The check that the server is ready: the database reads the table of the sessions,
/// which every signed-in request reads, and this process holds the lock of the server
/// in it; and the file storage takes a small file, gives it back and removes it, each
/// within two seconds. An answer is kept five seconds; a
/// request after that waits for the checks made anew, up to two and a half seconds
/// (a probe's timeout of three seconds is enough), and else gets the answer before.
/// Neither answer says why a check failed: the log does.
#[utoipa::path(
    get,
    path = "/ready",
    operation_id = "ready",
    tag = tag::SYSTEM,
    security(()),
    responses(
        (status = 200, description = "The server is ready: every check is `ok`.", body = Readiness),
        (
            status = 503,
            description = "The server is not ready. `detail` names the checks that are failing: `not ready: database`, `not ready: files`, `not ready: database, files`.",
            body = Problem,
            content_type = "application/problem+json",
        ),
    ),
)]
async fn handle_ready(State(app): State<App>) -> ApiResult {
    let readiness = app.readiness().await;
    match readiness.refusal() {
        None => Ok(json_response(&readiness)),
        Some(refusal) => Err(AppError::service_unavailable(refusal)),
    }
}

/// What the server is
///
/// The version of the server and what it runs on.
#[utoipa::path(
    get,
    path = "/ping",
    operation_id = "ping",
    tag = tag::SYSTEM,
    security(()),
    responses((status = 200, description = "What the server says about itself.", body = ServerMetadata)),
)]
async fn handle_ping(State(app): State<App>) -> ApiResult {
    Ok(json_response(&app.server_metadata().await))
}
