//! Public link management of a board, wiki or course.

use axum::body::Bytes;
use axum::extract::Path;
use fvoci_model::{PublicLink, PublicLinkChange, RefusalReason, Refusals};
use serde_json::Value;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::contract::bodies::PublicLinkRequest;
use super::contract::params::{LinkedContainerId, MemberWorkspaceId};
use super::contract::tag;
use super::workspace::InLinked;

use super::{ApiResult, json_response};
use super::{decode_json, json_object, require_container_permission};
use crate::app::App;
use crate::error::{AppError, Reason};
use crate::permissions::ContainerPermission;

pub(super) fn routes() -> OpenApiRouter<App> {
    OpenApiRouter::new().routes(routes!(handle_post_public_link, handle_get_public_link))
}

/// Checks that the user may manage the container's public link: its admins may. The
/// container must still be there.
async fn require_share_permission(
    app: &App,
    user_id: &str,
    container_id: &str,
) -> Result<(), AppError> {
    require_container_permission(
        app,
        user_id,
        container_id,
        ContainerPermission::Share,
        "access denied to sharing the board",
    )
    .await?;
    app.container(container_id).await?;
    Ok(())
}

/// Whether a board, wiki or course has a public link
///
/// Whether the container has an active public link, and with which read token. A
/// container that never had a public link is disabled. For the container's admins.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/containers/{containerId}/public-link",
    operation_id = "getPublicLink",
    tag = tag::PUBLIC_LINKS,
    params(MemberWorkspaceId, LinkedContainerId),
    responses((status = 200, description = "The public link state.", body = PublicLink)),
)]
async fn handle_get_public_link(
    InLinked(ws): InLinked,
    Path((_, container_id)): Path<(String, String)>,
) -> ApiResult {
    require_share_permission(&ws.app, &ws.caller.user_id, &container_id).await?;
    let link = ws.app.public_link(&container_id).await?;
    tracing::debug!(container_id, enabled = link.enabled, "GET public-link");
    Ok(json_response(&link))
}

/// Reads the body of `POST /containers/{containerId}/public-link`: `enabled`, a boolean,
/// and optionally `regenerate`, a boolean. A `token` is refused, `null` too: the server
/// makes it.
fn public_link_change(body: &Bytes) -> Result<PublicLinkChange, AppError> {
    let mut members = json_object(decode_json(body)?)?;
    // Every member that is refused, not the first alone.
    let mut refused = Refusals::new();
    if members.contains_key("token") {
        refused.refuse(
            "token",
            RefusalReason::Invalid,
            "the server makes the read token; a request cannot set it",
        );
    }
    members.retain(|_, member| !member.is_null());
    let mut flag = |name: &'static str, absent: Option<bool>| {
        let (read, reason) = match members.get(name) {
            Some(Value::Bool(flag)) => (Some(*flag), RefusalReason::Invalid),
            None | Some(Value::Null) => (absent, RefusalReason::Required),
            Some(_) => (None, RefusalReason::Invalid),
        };
        if read.is_none() {
            refused.refuse(name, reason, format!("{name} must be true or false"));
        }
        // What a refused member is read as is never used: the request is refused.
        read.unwrap_or_default()
    };
    let enabled = flag("enabled", None);
    let regenerate = flag("regenerate", Some(false));
    refused.into_result()?;
    Ok(match (enabled, regenerate) {
        (false, _) => PublicLinkChange::Off,
        (true, false) => PublicLinkChange::On,
        (true, true) => PublicLinkChange::OnWithNewToken,
    })
}

/// Enable, regenerate or disable a container's public link
///
/// Enables the container's public link, gives it a new read token, or disables it. The
/// server makes the token; a request that names one is 400. Where the server is
/// configured without public links, every request is refused with 403 and the reason
/// `sharing_disabled`, also one that turns a public link off. While the organization has
/// its public links off (`patchOrganization`), enabling a public link is refused with
/// 403 and the reason `public_links_off`; disabling it is not. For the container's
/// admins.
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/containers/{containerId}/public-link",
    operation_id = "setPublicLink",
    tag = tag::PUBLIC_LINKS,
    params(MemberWorkspaceId, LinkedContainerId),
    request_body = PublicLinkRequest,
    responses((
        status = 200,
        description = "The public link state as it is now, with the token while the public link is enabled.",
        body = PublicLink,
    )),
)]
async fn handle_post_public_link(
    InLinked(ws): InLinked,
    Path((_, container_id)): Path<(String, String)>,
    body: Bytes,
) -> ApiResult {
    require_share_permission(&ws.app, &ws.caller.user_id, &container_id).await?;

    if !ws.app.config().allow_public_links {
        tracing::warn!(
            container_id,
            user_id = %ws.caller.user_id,
            "Attempt to turn on public link for board via API failed, public links off in configuration."
        );
        return Err(
            AppError::forbidden("public sharing is turned off on this server")
                .with_reason(Reason::SharingDisabled),
        );
    }

    let change = public_link_change(&body)?;
    let link = ws
        .app
        .set_public_link(&container_id, change, &ws.caller.user_id)
        .await?;
    tracing::debug!(container_id, enabled = link.enabled, "POST public-link");
    Ok(json_response(&link))
}
