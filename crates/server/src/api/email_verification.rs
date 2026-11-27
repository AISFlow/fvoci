//! Confirming the email address of a registration without a session; the links'
//! tokens travel only in request bodies.

use axum::body::Bytes;
use axum::extract::{Extension, State};
use axum::http::HeaderMap;
use fvoci_model::ContainerKind;
use serde::{Deserialize, Serialize};
use utoipa_axum::{router::OpenApiRouter, routes};

use super::auth::{address_of, user_agent, with_session_cookie};
use super::contract::tag;
use super::recovery::RecoveryResponse;
use super::signup_cookie::{marker_of, without_marker};
use super::{ApiResult, decode_object, json_response};
use crate::app::{App, Registrant};
use crate::client_address::ClientAddress;

/// Where a confirmed registration's account was let in.
#[derive(Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
struct EmailVerified {
    /// The workspace: the first of the organization it started, the one its link
    /// leads into, or the one of its own space if it started with that alone.
    workspace_id: String,
    /// For a guest's link: the board or the wiki it was let onto.
    #[serde(skip_serializing_if = "Option::is_none")]
    container_id: Option<String>,
    /// For a guest's link: which of the two it is, so that they are sent to its own
    /// address.
    #[serde(skip_serializing_if = "Option::is_none")]
    container_kind: Option<ContainerKind>,
    /// Whether the registration started its organization, rather than joining one by
    /// the link of an invitation.
    organization_started: bool,
}

#[derive(Deserialize, utoipa::ToSchema)]
struct CompleteEmailVerification {
    /// The secret from the confirmation link's fragment.
    #[serde(default)]
    token: String,
    /// The password the registration was made with: needed where the link is opened in
    /// another browser than the one the registration was made from, and not read in
    /// that one.
    #[serde(default)]
    password: String,
}

impl std::fmt::Debug for CompleteEmailVerification {
    /// Leaves out the token and the password.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompleteEmailVerification")
            .finish_non_exhaustive()
    }
}

pub(super) fn routes() -> OpenApiRouter<App> {
    OpenApiRouter::new()
        .routes(routes!(complete))
        .routes(routes!(request))
}

#[derive(Deserialize, utoipa::ToSchema)]
struct RequestEmailVerification {
    /// The user name the registration was made with; or leave it empty and give the
    /// address.
    #[serde(default)]
    username: String,
    /// The address the registration was made with, used when no registration has the
    /// given name.
    #[serde(default)]
    email: String,
    /// The requester's language tag (`ko`, `en-GB`), used when the registration's own
    /// screen named no language the mail is written in (English, Korean); English if
    /// neither does.
    #[serde(default)]
    language: String,
}

/// Mail a new confirmation link
///
/// Answers 204 with an empty body whether or not a registration that is not confirmed
/// yet has the user name (or else the address), and whatever became of the message;
/// lookup and delivery run in the background. A new link ends the one before it. A
/// registration's address is sent at most `mail_reset_per_account_hour` messages an
/// hour, its first link counted: more requests are answered the same, send nothing
/// and lock nothing. Requests are limited by where they come from, with the requests
/// for a reset link (429). A request that names neither is 400. A server that sends no
/// mail answers 404; one that sends mail answers also after `emailVerification` was
/// turned off, for the registrations made while it was on.
#[utoipa::path(
    post, path = "/email-verification/request", operation_id = "requestEmailVerification",
    tag = tag::AUTH, security(()), request_body = RequestEmailVerification,
    responses((status = 204, description = "If a registration waits, its address will receive a new link.")),
)]
async fn request(
    State(app): State<App>,
    client: Option<Extension<ClientAddress>>,
    body: Bytes,
) -> ApiResult {
    let request: RequestEmailVerification = decode_object(&body)?;
    let work = app.prepare_signup_mail(
        &request.username,
        &request.email,
        &request.language,
        address_of(client),
    )?;
    Ok(RecoveryResponse::releasing(app, work))
}

/// Confirm the email address of a registration
///
/// Consumes the token of the link mailed to a registration's address, within a day of
/// the mail and seven days of the registration: makes the account, and the
/// organization and first workspace it registered with if it named one, and signs it in. The session
/// comes in the `Set-Cookie` header, as from `login`, and the answer says where the
/// account was let in. A link is used once, and a newer mail of the same registration
/// ends the links before it. A missing, ended, used or replaced token answers 400,
/// naming `token` in `errors`.
///
/// Only who registered finishes a registration, so that nobody is signed in to an
/// account whose password somebody else chose. In the browser the registration was
/// made from, which the cookie of `register` or `acceptInvitation` shows, the token is
/// enough. Anywhere else a token that would confirm is answered 403 with the reason
/// `password_required`, and the request is sent again with `password`, the password the
/// registration was made with: a wrong one is the 401 of `login` (`invalid_credentials`),
/// counted with the failed logins to the registration's user name, and too many are
/// 429. A request that is refused so leaves the link as it was. Nothing is asked of a
/// token that confirms nothing, so the answers tell no more of whether a registration
/// waits than the token does.
///
/// On a server that takes one organization and has its first user by now,
/// the registration is removed and the answer is 403 with the reason
/// `invitation_required`. A registration made by the link of an invitation
/// (`acceptInvitation`) is let in as the acceptance would have let it in, and the answer
/// names the board of a guest's link (`containerId`); if the link lets nobody in any more
/// (revoked, or who made it may no longer invite), the registration is removed and the
/// answer is 404 with the reason `invitation_unavailable`. Requests are counted by where they come from, as
/// registrations are, and too many are 429.
///
/// An account that a link for members lets in so is given its welcome board in the
/// workspace, unless it has one (`onboard` says more): written in the `language` of
/// the acceptance if that was English or Korean, or else in English.
#[utoipa::path(
    post, path = "/email-verification/complete", operation_id = "completeEmailVerification",
    tag = tag::AUTH, security(()), request_body = CompleteEmailVerification,
    responses(
        (
            status = 200,
            description = "The account was made and signed in, and was let in here.",
            body = EmailVerified,
            headers(("Set-Cookie" = String, description = "The session cookie.")),
        ),
        (
            status = 429,
            description = "Too many requests from where this one comes from, or too many wrong passwords at the registration's user name.",
            body = fvoci_model::Problem,
            content_type = "application/problem+json",
            headers(("Retry-After" = u64, description = "How many seconds to wait before trying again.")),
        ),
    ),
)]
async fn complete(
    State(app): State<App>,
    client: Option<Extension<ClientAddress>>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult {
    let request: CompleteEmailVerification = decode_object(&body)?;
    let marker = marker_of(&app, &headers);
    let registrant = Registrant {
        marker: marker.as_deref(),
        password: &request.password,
    };
    let signed_in = app
        .confirm_signup(
            &request.token,
            registrant,
            &user_agent(&headers),
            address_of(client),
        )
        .await?;
    let organization_started = signed_in.started_organization();
    let answer = EmailVerified {
        workspace_id: signed_in.confirmed.workspace_id,
        container_id: signed_in.confirmed.container_id,
        container_kind: signed_in.container_kind,
        organization_started,
    };
    let response = with_session_cookie(&app, &signed_in.session_token, json_response(&answer));
    Ok(if signed_in.made_here {
        without_marker(&app, response)
    } else {
        response
    })
}
