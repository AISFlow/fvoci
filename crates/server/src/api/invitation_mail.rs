//! Emailing an existing member or guest invitation without changing the link.

use axum::body::Bytes;
use fvoci_model::InvitationMail;
use utoipa_axum::{router::OpenApiRouter, routes};

use super::contract::{params::PeopleWorkspaceId, tag};
use super::workspace::InShared;
use super::{ApiResult, decode_object, no_content};
use crate::app::App;

pub(super) fn routes() -> OpenApiRouter<App> {
    OpenApiRouter::new().routes(routes!(send))
}

/// Email an invitation
///
/// Queues a separate message for each unique email address with an existing link.
/// The token must name an invitation of this workspace that is neither revoked nor
/// ended (a member link must let somebody in still), and the sender must still be
/// allowed to make that kind of invitation. The same checks are repeated before each
/// delivery. A member link is sent as it is, its expiry and uses unchanged.
/// A guest link lets one person in once, so it is not sent: each recipient gets a
/// one-use guest link of their own, to the same board or wiki in the same role and ending
/// with it, made by the sender as their message leaves, naming the named link as its
/// `parentId` and the address as its `recipient` (and revoked if that message is
/// refused for good or given up). The named link is left as it is; messages go out
/// whether or not it was used, and revoking it revokes its children nobody used.
/// Invalid recipients reject the whole request before sending.
/// Each message is in the recipient's language when the mail is written in it, else the
/// sender's setting, else `language`, else English. A server that sends no mail answers 404.
/// A request names at most `invitation_mail_recipients` addresses, repeats counted
/// (400, `too_many_recipients`). Its messages are queued all or none: 429
/// (`invitation_mail_limit`, with `Retry-After`) when the workspace would pass
/// `invitation_mail_per_hour` messages within an hour or its organization its own
/// hourly limit (`invitation_mail_per_org_hour`), and 503 (`mail_queue_full`) when the
/// server's invitation queue cannot hold them all, or, on a server of several
/// organizations, the organization's share of it (half of what the others leave, at
/// least one request). A refused request sends nothing.
#[utoipa::path(
    post, path = "/w/{workspaceId}/invitations/mail", operation_id = "emailInvitation",
    tag = tag::WORKSPACES, params(PeopleWorkspaceId), request_body = InvitationMail,
    responses((status = 204, description = "Delivery queued; the existing invitation is unchanged.")),
)]
async fn send(InShared(ws): InShared, body: Bytes) -> ApiResult {
    let ws = ws.require_people()?;
    let request = decode_object(&body)?;
    ws.app
        .email_invitation(&ws.workspace_id, &ws.caller.user_id, request)
        .await?;
    Ok(no_content())
}
