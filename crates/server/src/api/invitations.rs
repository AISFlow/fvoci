//! Invitations: the links that let somebody into a workspace.
//!
//! The routes under a workspace make, list and revoke its links. The routes that look
//! at one and accept one name no workspace: whoever comes with a link knows its token
//! and nothing else, and where the token leads is the server's to find out. A token
//! is sent to the API in a body, never in an address, and is answered once, by the
//! request that makes its link.

use axum::body::Bytes;
use axum::extract::{Extension, Path, State};
use axum::http::{Extensions, HeaderMap, StatusCode};
use axum::response::IntoResponse as _;
use fvoci_model::{
    AcceptInvitation, AcceptedInvitation, ContainerRole, GuestInvitationsChange, Invitation,
    InvitationPreview, InvitationToken, NewInvitation, Workspace, WorkspaceRole,
};
use serde::Deserialize;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::auth::address_of;
use super::contract::params::{AcceptLanguage, InvitationId, PeopleWorkspaceId};
use super::contract::tag;
use super::session::optional_session_of;
use super::signup_cookie::{marker_of, with_marker};
use super::workspace::{InShared, InWorkspace};
use super::{
    ApiResult, OptionalSession, accept_language, created, decode_object, json_response, no_content,
};
use crate::app::{Accepting, Admission, App, Inviting};
use crate::client_address::ClientAddress;
use crate::error::AppError;
use crate::permissions::{self, ContainerPermission, WorkspacePermission};

/// Where the account to make is in the request that accepts a link: a JSON Pointer.
const ACCOUNT: &str = "/account";

/// The routes of a workspace's links, for who works in it.
pub(super) fn routes() -> OpenApiRouter<App> {
    OpenApiRouter::new()
        .routes(routes!(handle_create_invitation, handle_get_invitations))
        .routes(routes!(handle_revoke_invitation))
        .routes(routes!(handle_set_guest_invitations))
}

/// The routes of whoever comes with a link: they may have no session yet.
pub(super) fn unauthenticated_routes() -> OpenApiRouter<App> {
    OpenApiRouter::new()
        .routes(routes!(handle_preview_invitation))
        .routes(routes!(handle_accept_invitation))
}

/// Make an invitation
///
/// Makes a link that lets whoever accepts it into the workspace, and answers it with
/// its `token`: the one time the token is told, as the server keeps its digest only. A
/// link ends seven days after it was made, or when it is revoked, and lets nobody in
/// from the moment who made it could no longer make it.
///
/// With the `role` `member` the link makes members of the workspace, as often as it is
/// used, and names no board. Only the workspace's administrators make one: anybody
/// else who works there is refused (403). Whoever accepts it becomes a member of the
/// workspace's organization too.
///
/// With the `role` `guest` the link lets one person, once, onto the board or the wiki
/// `containerId` of the workspace, in the role `containerRole` (`viewer`, `commenter` or
/// `editor`; an editor if left out): a guest of the workspace, who sees that board or
/// wiki and nothing else of it.
/// Who adds members to the board makes one (its admins, and the workspace's
/// administrators), while the workspace lets guests be invited
/// (`setGuestInvitations`); anybody else who sees the board is refused (403), and a
/// board the user does not see, or of another workspace, is not there (404). A
/// template takes no guests (400).
///
/// A guest of the workspace is told that it is not there (404).
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/invitations",
    operation_id = "createInvitation",
    tag = tag::WORKSPACES,
    params(PeopleWorkspaceId),
    request_body = NewInvitation,
    responses((status = 201, description = "The invitation, with its token.", body = Invitation)),
)]
async fn handle_create_invitation(InShared(ws): InShared, body: Bytes) -> ApiResult {
    let ws = ws.require_people()?;
    let request: NewInvitation = decode_object(&body)?;
    let inviting = match request.role {
        WorkspaceRole::Member => {
            for (given, field) in [
                (request.container_id.is_some(), "containerId"),
                (request.container_role.is_some(), "containerRole"),
            ] {
                if given {
                    return Err(
                        AppError::bad_request("an invitation for members names no board")
                            .with_field(field),
                    );
                }
            }
            if !permissions::allows_in_workspace(ws.actor(), WorkspacePermission::InviteMembers) {
                return Err(AppError::forbidden(
                    "only the administrators of the workspace invite its members",
                ));
            }
            Inviting::Members
        }
        WorkspaceRole::Guest => guest_of(&ws, &request).await?,
        WorkspaceRole::Admin => {
            return Err(
                AppError::bad_request("an invitation makes a member or a guest").with_field("role"),
            );
        }
    };
    let invitation = ws
        .app
        .invite(&ws.workspace_id, &ws.caller.user_id, inviting)
        .await?;
    Ok(created(&invitation))
}

/// The board and the role a request invites a guest to, once the user may invite one
/// there.
async fn guest_of<'r>(
    ws: &InWorkspace,
    request: &'r NewInvitation,
) -> Result<Inviting<'r>, AppError> {
    let Some(container_id) = request.container_id.as_deref() else {
        return Err(
            AppError::bad_request("an invitation for a guest names the board")
                .with_field("containerId"),
        );
    };
    let role = request.container_role.unwrap_or(ContainerRole::Editor);
    if !permissions::may_hold(WorkspaceRole::Guest, role) {
        return Err(
            AppError::bad_request("a guest is an editor of a board at most")
                .with_field("containerRole"),
        );
    }
    // A board the user does not see, and one of another workspace, is not there.
    let (access, invites) = ws
        .app
        .guest_inviter(&ws.caller.user_id, &ws.workspace_id, container_id)
        .await?
        .filter(|(access, _)| access.grant.sees())
        .ok_or_else(|| AppError::not_found("board"))?;
    if access.container.is_template {
        return Err(
            AppError::bad_request("a guest is invited to a board, not to a template")
                .with_field("containerId"),
        );
    }
    if !access.grant.allows(ContainerPermission::ManageRoles) {
        return Err(AppError::forbidden(
            "only who adds the members of the board invites guests to it",
        ));
    }
    if !invites {
        return Err(AppError::forbidden(
            "guests are not invited to the boards of this workspace",
        ));
    }
    Ok(Inviting::Guest { container_id, role })
}

/// The invitations of a workspace
///
/// The links into the workspace that are not ended, not used as often as they may be
/// and not revoked, and a used one while a guest link made like it for one recipient of
/// its mail (`parentId`) is still out, unused. One of them may let nobody in all the same, while who made it may
/// not invite as they did: it is listed, so that it can be revoked. Its
/// administrators get all of them, anybody else
/// who works there the links they made themselves. No link comes with its token, which
/// was told when it was made. The newest first.
///
/// A guest of the workspace is told that it is not there (404).
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/invitations",
    operation_id = "getInvitations",
    tag = tag::WORKSPACES,
    params(PeopleWorkspaceId),
    responses((status = 200, description = "The invitations.", body = Vec<Invitation>)),
)]
async fn handle_get_invitations(ws: InWorkspace) -> ApiResult {
    let ws = ws.require_people()?;
    let invitations = ws
        .app
        .invitations(&ws.workspace_id, &ws.caller.user_id, ws.actor())
        .await?;
    Ok(json_response(&invitations))
}

/// The invitation a request names in its path.
#[derive(Debug, Deserialize)]
struct InvitationPlace {
    #[serde(rename = "invitationId")]
    invitation_id: String,
}

/// Revoke an invitation
///
/// The link lets nobody in from now on, nor do the guest links made like it for the
/// recipients of its mail that nobody used yet. The workspace's administrators revoke
/// any link into it, anybody else who works there the links they made themselves. A
/// link that is not there, that is revoked or ended already, or that is not the user's
/// to revoke is 404, the same for each; a used link that has not ended is revoked.
///
/// A guest of the workspace is told that it is not there (404).
#[utoipa::path(
    delete,
    path = "/w/{workspaceId}/invitations/{invitationId}",
    operation_id = "revokeInvitation",
    tag = tag::WORKSPACES,
    params(PeopleWorkspaceId, InvitationId),
    responses((status = 204, description = "The invitation is revoked.")),
)]
async fn handle_revoke_invitation(
    ws: InWorkspace,
    Path(place): Path<InvitationPlace>,
) -> ApiResult {
    let ws = ws.require_people()?;
    ws.app
        .revoke_invitation(
            &ws.workspace_id,
            &place.invitation_id,
            &ws.caller.user_id,
            ws.actor(),
        )
        .await?;
    Ok(no_content())
}

/// Let guests be invited, or not
///
/// Turns on or off whether guests are invited to the boards of the workspace, and
/// answers the workspace. Turning it off revokes the links that invite guests, which
/// do not come back when it is turned on again, and while it is off none is made.
/// Who is a guest already stays one. Only the workspace's administrators: anybody else who
/// works there is refused (403).
///
/// A guest of the workspace is told that it is not there (404).
#[utoipa::path(
    put,
    path = "/w/{workspaceId}/guest-invitations",
    operation_id = "setGuestInvitations",
    tag = tag::WORKSPACES,
    params(PeopleWorkspaceId),
    request_body = GuestInvitationsChange,
    responses((status = 200, description = "The workspace.", body = Workspace)),
)]
async fn handle_set_guest_invitations(ws: InWorkspace, body: Bytes) -> ApiResult {
    let ws = ws.require_people()?;
    let change: GuestInvitationsChange = decode_object(&body)?;
    if !permissions::allows_in_workspace(ws.actor(), WorkspacePermission::SwitchGuestInvitations) {
        return Err(AppError::forbidden(
            "only the administrators of the workspace say whether guests are invited",
        ));
    }
    let workspace = ws
        .app
        .set_guest_invitations(&ws.workspace_id, change, &ws.caller.user_id)
        .await?;
    Ok(json_response(&workspace))
}

/// Look at an invitation
///
/// What the link of `token` shows to whoever holds it, before they accept it and
/// whether they are signed in or not: the name of the workspace it leads into and the
/// name of who made it (the name they are shown by, or their login name), and for a
/// guest's link the title of the board (`containerTitle`).
///
/// A token that lets nobody in is 404 with the reason `invitation_unavailable`, as
/// `acceptInvitation` answers it and whatever is wrong with it. A link for one guest
/// that only the registration of this browser uses up (the cookie of `acceptInvitation`'s
/// 202) is shown to it still, to register by again, unless it is signed in. Requests are counted
/// by where they come from, and too many are 429.
#[utoipa::path(
    post,
    path = "/invitations/preview",
    operation_id = "previewInvitation",
    tag = tag::WORKSPACES,
    request_body = InvitationToken,
    security(()),
    responses(
        (status = 200, description = "What the link shows.", body = InvitationPreview),
        (
            status = 429,
            description = "Too many requests from where this one comes from.",
            body = fvoci_model::Problem,
            content_type = "application/problem+json",
            headers(("Retry-After" = u64, description = "How many seconds to wait before trying again.")),
        ),
    ),
)]
async fn handle_preview_invitation(
    State(app): State<App>,
    client: Option<Extension<ClientAddress>>,
    extensions: Extensions,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult {
    let request: InvitationToken = decode_object(&body)?;
    // Who is signed in accepts as their account, which replaces no registration. Only
    // a browser with a registration's marker is asked for its session.
    let browser = match marker_of(&app, &headers).filter(|_| app.verifies_email()) {
        Some(marker)
            if optional_session_of(&app, &headers, &extensions)
                .await?
                .is_none() =>
        {
            Some(marker)
        }
        _ => None,
    };
    let preview = app
        .preview_invitation(&request.token, browser.as_deref(), address_of(client))
        .await?;
    Ok(json_response(&preview))
}

/// Accept an invitation
///
/// Lets somebody into the workspace the link of `token` leads into, and answers where
/// that is. A request with a session lets in the user of the session, and names no
/// `account` (400 if it does). A request without one makes the `account` it names and
/// lets that in: the account's members are checked as `register` checks them (400,
/// with `errors`, which name them under `/account`), and a name or an email address
/// that is taken is 409 with the reason `username_taken` or `email_taken`. Accepting
/// does not sign in.
///
/// A member's link makes the account a member of the workspace and of its
/// organization; who is a guest of the workspace becomes a member, and who works there
/// already stays what they are. A guest's link puts the account on its board, in the
/// link's role or the higher one the account has there: who is no member of the
/// workspace becomes a guest of it (and no member of its organization), and who is one
/// stays what they are in it. The answer names the board then (`containerId`). The account
/// is made, the membership stored and the use of the link counted together or not at
/// all, and an acceptance that changes nothing uses nothing of the link.
///
/// On a server that asks for the email address of a registration to be confirmed
/// (`emailVerification` of `getLoginOptions`), a new `account` waits for its address,
/// unless the link was made for one recipient of a mailed guest link at that address,
/// which the mail proves: the answer is 202 with no body, nobody is let in, and the
/// link mailed to the address (`completeEmailVerification`) makes the account and lets
/// it in, as this acceptance would have, and signs it in. The 202 sets the cookie of
/// the browser the registration was made from, as `register` does. The registration counts as a
/// use of the link from the start, so a link for one guest lets nobody else in
/// meanwhile, and it holds its name and address as an account would. Not confirmed in
/// seven days, it is removed and gives the use back. A new `account` from the browser
/// of a registration that waits still (by this link, another, or `register`) replaces
/// that registration: its name and address are free for the account, and the use it
/// took of a link is given back, so that a link for one guest takes the same person
/// again with the address put right. `language` is the language of the
/// mail (English or Korean).
///
/// A session that the link's organization does not take (it takes no password,
/// `ssoRequired`, and the session was not made through its provider) is 401 with the
/// reason `sso_required` and its `orgId`, and lets nobody in.
///
/// A token that lets nobody in is 404 with the reason `invitation_unavailable`,
/// whatever is wrong with it: there is no such link, it ended, it was used as often as
/// it may be, it was revoked, or who made it may no longer invite. Requests are counted
/// by where they come from, and too many are 429.
///
/// An account that a link for members lets in is given its welcome board in the
/// workspace, a private board of its own, unless it has one (`onboard` says more):
/// written in the language of the account's settings, or else in `language` for an
/// account made here, if that is English or Korean, or else in the one
/// `Accept-Language` prefers. An account whose address is confirmed first is given
/// it then (`completeEmailVerification`).
#[utoipa::path(
    post,
    path = "/invitations/accept",
    operation_id = "acceptInvitation",
    tag = tag::WORKSPACES,
    params(AcceptLanguage),
    request_body = AcceptInvitation,
    security((), ("sessionCookie" = []), ("bearerToken" = [])),
    responses(
        (status = 200, description = "Where the account was let in.", body = AcceptedInvitation),
        (
            status = 202,
            description = "A new account waits for its email address to be confirmed by the link mailed to it.",
            headers(("Set-Cookie" = String, description = "The cookie that tells the browser the registration was made from, as from `register`; no session.")),
        ),
        (
            status = 429,
            description = "Too many requests from where this one comes from, or the server has too much password work to do at the moment.",
            body = fvoci_model::Problem,
            content_type = "application/problem+json",
            headers(("Retry-After" = u64, description = "How many seconds to wait before trying again.")),
        ),
    ),
)]
async fn handle_accept_invitation(
    State(app): State<App>,
    OptionalSession(session): OptionalSession,
    client: Option<Extension<ClientAddress>>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult {
    let request: AcceptInvitation = decode_object(&body)?;
    let account = match (&session, request.account) {
        (Some(_), Some(_)) => {
            return Err(AppError::bad_request(
                "a request with a session lets in its user, and names no account",
            )
            .with_field("account"));
        }
        (_, account) => account
            .map(fvoci_model::NewAccount::check)
            .transpose()
            .map_err(|refused| AppError::from(refused).under(ACCOUNT))?,
    };
    let browser = marker_of(&app, &headers);
    let accepting = match (&session, &account) {
        // A request with a session names no account: refused above.
        (Some(session), _) => Accepting::SignedIn(session.into()),
        (None, Some(account)) => Accepting::NewAccount {
            account,
            language: &request.language,
            browser: browser.as_deref(),
        },
        (None, None) => {
            return Err(AppError::unauthorized(
                "accepting an invitation takes a session or an account to make",
            ));
        }
    };
    // What is refused of the body is of the account: a name or an address that is taken.
    let admission = app
        .accept_invitation(
            &request.token,
            accepting,
            address_of(client),
            accept_language(&headers),
        )
        .await
        .map_err(|error| error.under(ACCOUNT))?;
    Ok(match admission {
        Admission::LetIn(accepted) => json_response(&accepted),
        Admission::Waiting(marker) => {
            with_marker(&app, &marker, StatusCode::ACCEPTED.into_response())
        }
    })
}
