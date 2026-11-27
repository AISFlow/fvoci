//! `GET /client-config`.

use axum::extract::State;
use fvoci_model::ClientConfig;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::contract::tag;

use super::{ApiResult, json_response};
use crate::app::App;

pub(super) fn routes() -> OpenApiRouter<App> {
    OpenApiRouter::new().routes(routes!(get_client_config))
}

/// What a client must know about the server
///
/// The part of the server's configuration a client needs before anybody is signed in.
#[utoipa::path(
    get,
    path = "/client-config",
    operation_id = "getClientConfig",
    tag = tag::SYSTEM,
    security(()),
    responses((status = 200, description = "The configuration.", body = ClientConfig)),
)]
async fn get_client_config(State(app): State<App>) -> ApiResult {
    Ok(json_response(&app.client_config()))
}
