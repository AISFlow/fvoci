//! The routes of an organization, under `/org/{orgId}`, for its administrators: the
//! organization's provider of sign-ins, and its pending settings.
//!
//! A route here takes [`OrgAdmin`]: the session's user as an administrator of the
//! organization, with the application reaching the rows of that organization. For
//! anybody else the organization is not there (404), whether it exists or not.

use axum::body::Bytes;
use axum::extract::Extension;
use axum::extract::rejection::RawPathParamsRejection;
use axum::extract::{FromRequestParts, RawPathParams};
use axum::http::request::Parts;
use fvoci_model::{
    OidcFormerProviderEnd, OidcProvider, OidcProviderSettings, OidcStart, Problem, Session,
};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::auth::address_of;
use super::contract::params::OrgId;
use super::contract::tag;
use super::oidc::started_response;
use super::session::session_of;
use super::{ApiResult, decode_object, json_response, no_content};
use crate::app::App;
use crate::client_address::ClientAddress;
use crate::error::AppError;

/// The path parameter that names the organization of a route.
const ORG_ID: &str = "orgId";

pub(super) fn routes() -> OpenApiRouter<App> {
    OpenApiRouter::new()
        .routes(routes!(
            handle_get_oidc_provider,
            handle_put_oidc_provider,
            handle_delete_oidc_provider
        ))
        .routes(routes!(handle_delete_oidc_pending))
        .routes(routes!(handle_delete_oidc_former))
        .routes(routes!(handle_start_oidc_trial))
}

/// The organization a request names in its path, for an administrator of it.
///
/// Extraction fails with 401 without a session, and with 404 unless the session's user
/// administers the organization: a member who is no administrator, somebody of another
/// organization, and an organization that does not exist are answered alike. An
/// administrator whose session does not open the organization, which takes only
/// sessions made through its provider, is told so (401, reason `sso_required`): asked
/// where the organization is entered (`App::enter_org_as_admin`), of every request.
#[derive(Debug)]
pub(crate) struct OrgAdmin {
    /// The application, reaching the rows of the organization.
    pub(crate) app: App,
    /// The session of the request.
    pub(crate) session: Session,
    /// The organization the path names.
    pub(crate) org_id: String,
}

impl FromRequestParts<App> for OrgAdmin {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, app: &App) -> Result<Self, Self::Rejection> {
        let session = session_of(app, &parts.headers, &parts.extensions).await?;
        let params = match RawPathParams::from_request_parts(parts, app).await {
            Ok(params) => params,
            Err(RawPathParamsRejection::InvalidUtf8InPathParam(_)) => {
                return Err(org_not_found());
            }
            Err(_) => return Err(AppError::internal("the route has no path parameters")),
        };
        let org_id = params
            .iter()
            .find(|(name, _)| *name == ORG_ID)
            .map(|(_, value)| value.to_owned())
            .ok_or_else(|| AppError::internal("the route names no organization"))?;
        let app = app
            .enter_org_as_admin(&session, &org_id)
            .await?
            .ok_or_else(org_not_found)?;
        Ok(Self {
            app,
            session,
            org_id,
        })
    }
}

fn org_not_found() -> AppError {
    AppError::not_found("organization")
}

/// The provider of an organization
///
/// The `OpenID Connect` provider people of the organization sign in through, without
/// its client secret: `hasClientSecret` says whether there is one. 404 if the
/// organization has none. `canRequireSso` says whether the requesting session was
/// made through this enabled provider as it is stored now; it is false after its
/// issuer, client, secret or `enabled` changes, until somebody signs in through it again.
#[utoipa::path(
    get,
    path = "/org/{orgId}/oidc",
    operation_id = "getOidcProvider",
    tag = tag::AUTH,
    params(OrgId),
    responses((status = 200, description = "The provider.", body = OidcProvider)),
)]
async fn handle_get_oidc_provider(org: OrgAdmin) -> ApiResult {
    let provider = org
        .app
        .oidc_provider(&org.session)
        .await?
        .ok_or_else(|| AppError::not_found("oidc provider"))?;
    Ok(json_response(&provider))
}

/// Set the provider of an organization
///
/// Sets the organization's `OpenID Connect` provider, in place of the one it has. The
/// discovery document of an issuer that is not the organization's already is read
/// first: it must name the issuer exactly and have
/// `https` endpoints, or the request is 400 with the reason `oidc_issuer_unverified`
/// and `errors` naming `/issuer`. The server reaches only `https` addresses, follows no
/// redirect, and reaches only public addresses unless its configuration lets it
/// (`oidc_allow_private_addresses`).
///
/// `clientSecret` left out keeps the stored secret while the issuer and the client stay
/// the same; with another issuer or client it is required when a secret is stored (400,
/// `required` at `/clientSecret`), as the secret is the old client's. The empty string
/// stores none. The secret is stored sealed with the server's key and never answered:
/// without the key the request is 409 with the reason `secret_key_required`. `autoJoin`
/// and `enabled` left out stay as they are, also for another issuer or client, and are
/// on when the organization has no provider.
///
/// While the organization takes no password and the request keeps it so, another
/// issuer, client or secret is not taken at once: it is saved aside as the provider's
/// `pending` settings (in place of pending ones there are), and people go on signing
/// in through the provider as it is. A sign-in through the pending settings
/// (`startOidcTrial`) that ends well makes them the provider's; `deleteOidcPending`
/// throws them away. The secret of the pending settings is never answered either. What
/// else the request changes (`autoJoin`) is taken at once; `enabled` false is refused
/// (`sso_enforced`). While the organization takes a password a change is taken at
/// once, and a change of what signing in takes throws pending settings away.
///
/// `ssoRequired` true makes the organization take no password: from the next request
/// on, what is the organization's (the routes under `/w/{workspaceId}` and
/// `/org/{orgId}`, and `setUserAdmin` where the organization is the instance's own, the
/// oldest) answers 401 with the reason `sso_required` to every session that was not
/// made through this provider, `getMyMemberships` leaves its boards out for one, and
/// the websocket follows none of its workspaces for one. The sessions stay, and open
/// what is of other organizations. It is switched on by a
/// session that was itself made through this provider, switched on, as it is stored
/// (`canRequireSso`), by a request that does not change its issuer, client, secret or
/// `enabled`, or the request is 409 with the reason `sso_unproven`: who asks has then
/// shown that it lets them in. A `clientSecret` sent as it is stored is no change.
/// While it is on, the provider is not switched off (`enabled` false) and not removed:
/// 409 with the reason `sso_enforced`, unless the same request switches `ssoRequired`
/// off. Left out it stays as it is,
/// and is off when the organization has no provider. A provider changed by another request
/// since this one read it is 409, to be read and sent again. The answer's `redirectUri`
/// is the address to register with the provider, exactly: the organization's own,
/// `…/api/v1/auth/oidc/callback/{orgId}`, the same before the provider is set and after
/// it is changed.
#[utoipa::path(
    put,
    path = "/org/{orgId}/oidc",
    operation_id = "putOidcProvider",
    tag = tag::AUTH,
    params(OrgId),
    request_body = OidcProviderSettings,
    responses((status = 200, description = "The provider as stored.", body = OidcProvider)),
)]
async fn handle_put_oidc_provider(org: OrgAdmin, body: Bytes) -> ApiResult {
    let settings: OidcProviderSettings = decode_object(&body)?;
    let provider = org
        .app
        .put_oidc_provider(&org.org_id, &org.session, &settings)
        .await?;
    tracing::debug!(user_id = %org.session.user_id, "the provider was set by an administrator");
    Ok(json_response(&provider))
}

/// Remove the provider of an organization
///
/// Removes the organization's provider and the sign-ins under way through it, and
/// ends the sessions made through a provider of the organization under its issuer. An
/// organization without one answers the same.
/// While the organization takes no password (`ssoRequired`) the provider stays: 409
/// with the reason `sso_enforced`.
#[utoipa::path(
    delete,
    path = "/org/{orgId}/oidc",
    operation_id = "deleteOidcProvider",
    tag = tag::AUTH,
    params(OrgId),
    responses((status = 204, description = "The organization has no provider.")),
)]
async fn handle_delete_oidc_provider(org: OrgAdmin) -> ApiResult {
    org.app.delete_oidc_provider(&org.session.user_id).await?;
    Ok(no_content())
}

/// Throw the pending settings of an organization's provider away
///
/// Throws away the settings saved aside while the organization takes no password
/// (`pending` of the provider): the provider stays as it is. An organization without
/// any answers the same. Recorded in the organization's audit log.
#[utoipa::path(
    delete,
    path = "/org/{orgId}/oidc/pending",
    operation_id = "deleteOidcPending",
    tag = tag::AUTH,
    params(OrgId),
    responses((status = 204, description = "The provider has no pending settings.")),
)]
async fn handle_delete_oidc_pending(org: OrgAdmin) -> ApiResult {
    org.app.discard_oidc_pending(&org.session.user_id).await?;
    Ok(no_content())
}

/// End the keeping of an organization's former provider
///
/// When the provider's issuer changes, the provider before is kept as the `former`
/// one for `oidc_former_provider_days` (30 by default): nobody signs in through it,
/// but somebody the new provider vouches for who has no account yet may sign in
/// through it once to carry their account over. This ends that before its time: from
/// then on nobody carries an account over from it. The body names the former
/// provider as it was read (`createdAt`): if the organization moved again since, and
/// another one is kept, the request is 409, to be read and sent again. An organization
/// that keeps none answers 204 as well. Recorded in the organization's audit log
/// (`oidc_former_provider_ended`), as its keeping (`oidc_former_provider_kept`) and
/// its running out (`oidc_former_provider_lapsed`, the server's doing) are.
#[utoipa::path(
    delete,
    path = "/org/{orgId}/oidc/former",
    operation_id = "deleteOidcFormerProvider",
    tag = tag::AUTH,
    params(OrgId),
    request_body = OidcFormerProviderEnd,
    responses(
        (status = 204, description = "The organization keeps no former provider."),
        (
            status = 409,
            description = "Another former provider is kept than the one read.",
            body = Problem,
            content_type = "application/problem+json",
        ),
    ),
)]
async fn handle_delete_oidc_former(org: OrgAdmin, body: Bytes) -> ApiResult {
    let read: OidcFormerProviderEnd = decode_object(&body)?;
    org.app
        .end_oidc_former_provider(&org.session.user_id, read.created_at)
        .await?;
    Ok(no_content())
}

/// Try the pending settings of an organization's provider
///
/// Starts a sign-in through the pending settings (`pending` of the provider), as
/// `startOidcLogin` starts one through the provider: it answers where the page sends
/// the browser, with the cookie of the sign-in. When the browser comes back
/// (`finishOidcLogin`) and the sign-in ends well, in the account of the administrator
/// who started it, the pending settings become the provider's, at a new revision that the
/// new session proves (`canRequireSso`); another issuer ends the sessions of the one
/// before, and keeps the provider before as the `former` one. A sign-in that fails
/// changes nothing, and no trial makes an account.
///
/// Whose account it ends in: the identity the pending settings vouch for must be tied
/// to the account of the administrator who started the trial. One that is another
/// account's, or that administrator administering the organization no more, is
/// refused with `oidc_trial_account`. One that is tied to no account, as an
/// administrator's is at an issuer the organization moves to, is tied to their own
/// account by the trial, in the transaction that applies the settings and recorded
/// (`oidc_identity_bound`), only if the trial was started from a session made through
/// the organization's provider as it is stored, the browser comes back with that
/// session, and it comes back no more than five minutes after that session was made
/// (`bindsIdentityUntil` of the pending settings): who only holds a session cannot
/// sign in through the provider again, and ties nothing. Otherwise it is refused with
/// `oidc_trial_stale`: sign in through the provider again (`startOidcLogin`) and
/// finish the trial within five minutes. Pending
/// settings of the same issuer tie nothing.
///
/// 409 with the reason `oidc_unavailable` if there are no pending settings, the
/// provider is off, or their issuer cannot be reached; 429 when this address made too
/// many sign-ins of late.
#[utoipa::path(
    post,
    path = "/org/{orgId}/oidc/pending/sign-in",
    operation_id = "startOidcTrial",
    tag = tag::AUTH,
    params(OrgId),
    responses(
        (
            status = 200,
            description = "Started: where the browser goes.",
            body = OidcStart,
            headers(("Set-Cookie" = String, description = "The cookie of the sign-in.")),
        ),
        (
            status = 409,
            description = "No pending settings, or they cannot be reached.",
            body = Problem,
            content_type = "application/problem+json",
        ),
        (
            status = 429,
            description = "Too many sign-ins from this address. `retryAfter`, and the `Retry-After` header, say how long to wait.",
            body = Problem,
            content_type = "application/problem+json",
            headers(("Retry-After" = u64, description = "How many seconds to wait before trying again.")),
        ),
    ),
)]
async fn handle_start_oidc_trial(
    org: OrgAdmin,
    client: Option<Extension<ClientAddress>>,
) -> ApiResult {
    let started = org
        .app
        .start_oidc_trial(&org.org_id, &org.session, address_of(client))
        .await?;
    Ok(started_response(&org.app, started))
}
