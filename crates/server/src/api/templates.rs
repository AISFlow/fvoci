//! Listing the template boards of a workspace.

use axum::http::HeaderMap;
use fvoci_model::{Container, ContainerKinds};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::contract::params::{AcceptLanguage, WorkspaceId};
use super::contract::tag;

use super::workspace::InWorkspace;
use super::{ApiResult, accept_language, json_response};
use crate::app::{App, as_managed};

pub(super) fn routes() -> OpenApiRouter<App> {
    OpenApiRouter::new().routes(routes!(handle_get_templates))
}

/// The templates of a workspace
///
/// The workspace's template boards the user may see, then the built-in templates, which
/// every workspace is offered. Each of the two oldest first.
///
/// The built-in templates are written in several languages (`templateLanguage` among a
/// board's `properties`), and those of one are listed: the language of the user's
/// settings; where that is left to the device, or is one the templates are not written
/// in, the one `Accept-Language` prefers; failing both, English.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/templates",
    operation_id = "getTemplates",
    tag = tag::CONTAINERS,
    params(WorkspaceId, AcceptLanguage),
    responses((status = 200, description = "The templates.", body = Vec<Container>)),
)]
async fn handle_get_templates(ws: InWorkspace, headers: HeaderMap) -> ApiResult {
    let ws = ws.require_no_guest()?;
    let app = &ws
        .app
        .clone()
        .reaching(ContainerKinds::BOARDS.union(ContainerKinds::COURSES));
    let (user_id, workspace_id) = (ws.caller.user_id.as_str(), &ws.workspace_id);
    let containers = app
        .template_boards(workspace_id, user_id, ws.role, accept_language(&headers))
        .await?;
    let mut results: Vec<Container> = Vec::with_capacity(containers.len());
    for container in containers {
        // Whole to who reads it; to the workspace's administrators who only manage it,
        // its name, as getContainer gives it (`as_managed`).
        let grant = app.container_grant(user_id, &container.id).await?;
        if grant.reads() {
            results.push(container);
        } else if grant.manages_only() {
            results.push(as_managed(container));
        }
    }
    tracing::debug!(
        workspace_id,
        containers_count = results.len(),
        "GetTemplates"
    );
    Ok(json_response(&results))
}
