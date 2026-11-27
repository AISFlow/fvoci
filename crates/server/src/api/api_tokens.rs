//! The personal API tokens of the user of the session, and those that reach an
//! organization, for its administrators. A person manages them, by a session: a token
//! is answered 403 here as on every route that is the account's.

use axum::Extension;
use axum::body::Bytes;
use axum::extract::{Path, State};
use fvoci_model::{ApiToken, IssuedApiToken, MemberApiToken, NewApiToken};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::auth::address_of;
use super::contract::params::{ApiTokenId, OrgId};
use super::contract::tag;
use super::org::OrgAdmin;
use super::{ApiResult, RequiredSession, created, decode_object, json_response, no_content};
use crate::app::App;
use crate::client_address::ClientAddress;

pub(super) fn routes() -> OpenApiRouter<App> {
    OpenApiRouter::new()
        .routes(routes!(handle_create_api_token, handle_get_api_tokens))
        .routes(routes!(handle_revoke_api_token))
        .routes(routes!(handle_get_org_api_tokens))
        .routes(routes!(handle_revoke_org_api_token))
}

/// Make a personal API token
///
/// Makes a token for an agent to act as the user with, and answers it once: it is not
/// stored, only its digest, and is not shown again. It reaches the workspaces of
/// `workspaceIds`, each one the user works in (no guest's) and the session opens, and
/// the user's own space if `allowPersonalSpace` (with the account's `password`, if it
/// has one), never a diary. `scope` says whether it reads, or reads and writes the
/// content of what it reaches. A workspace of an organization that takes only sessions
/// made through its provider is given only if the organization lets it (403,
/// `api_tokens_disallowed`), and the token lasts at most 90 days then; otherwise as
/// `expiresInDays` asks, 90 unless it asks, at most 365. An account holds at most 25
/// tokens that still open something (400, `too_many_api_tokens`). A session that has
/// ended makes none (401), nor one made through a provider that has been removed,
/// switched off or changed since (409, `provider_changed`: the session stays signed in,
/// and one made through the provider again makes the token).
#[utoipa::path(
    post,
    path = "/users/me/api-tokens",
    operation_id = "createApiToken",
    tag = tag::API_TOKENS,
    request_body = NewApiToken,
    responses((status = 201, description = "The token, told this once.", body = IssuedApiToken)),
)]
async fn handle_create_api_token(
    State(app): State<App>,
    RequiredSession(session): RequiredSession,
    client: Option<Extension<ClientAddress>>,
    body: Bytes,
) -> ApiResult {
    let new: NewApiToken = decode_object(&body)?;
    let issued = app
        .create_api_token(&session, new, address_of(client))
        .await?;
    Ok(created(&issued))
}

/// The personal API tokens of the user
///
/// The tokens of the user of the session that are not taken back, the newest first,
/// each with the workspaces it reaches; those past their end too, until they are taken
/// back. Never a token itself.
#[utoipa::path(
    get,
    path = "/users/me/api-tokens",
    operation_id = "getApiTokens",
    tag = tag::API_TOKENS,
    responses((status = 200, description = "The tokens.", body = Vec<ApiToken>)),
)]
async fn handle_get_api_tokens(
    State(app): State<App>,
    RequiredSession(session): RequiredSession,
) -> ApiResult {
    Ok(json_response(
        &app.api_tokens_of_user(&session.user_id).await?,
    ))
}

/// Take back a personal API token
///
/// From now on the token opens nothing. 404 for one that is not the user's, or was
/// taken back.
#[utoipa::path(
    delete,
    path = "/users/me/api-tokens/{tokenId}",
    operation_id = "revokeApiToken",
    tag = tag::API_TOKENS,
    params(ApiTokenId),
    responses((status = 204, description = "The token opens nothing.")),
)]
async fn handle_revoke_api_token(
    State(app): State<App>,
    RequiredSession(session): RequiredSession,
    Path(token_id): Path<String>,
) -> ApiResult {
    app.revoke_api_token(&session.user_id, &token_id).await?;
    Ok(no_content())
}

/// The personal API tokens that reach an organization
///
/// The tokens of its members that reach a workspace of it and open anything now, the
/// newest first, each with those of its workspaces it reaches. Nothing is told of what
/// a token opens elsewhere, or where it was used from.
#[utoipa::path(
    get,
    path = "/org/{orgId}/api-tokens",
    operation_id = "getOrgApiTokens",
    tag = tag::ORGANIZATION,
    params(OrgId),
    responses((status = 200, description = "The tokens.", body = Vec<MemberApiToken>)),
)]
async fn handle_get_org_api_tokens(org: OrgAdmin) -> ApiResult {
    Ok(json_response(&org.app.api_tokens_of_organization().await?))
}

/// Take a personal API token out of an organization
///
/// From now on the token reaches no workspace of this organization; the audit log says
/// who took it out. What it reaches elsewhere stays its owner's to end, and a token
/// left reaching nothing (no other workspace, not its owner's own space) is taken back.
/// 404 for one that reaches no workspace of it, or opens nothing.
#[utoipa::path(
    delete,
    path = "/org/{orgId}/api-tokens/{tokenId}",
    operation_id = "revokeOrgApiToken",
    tag = tag::ORGANIZATION,
    params(OrgId, ApiTokenId),
    responses((status = 204, description = "The token reaches the organization no more.")),
)]
async fn handle_revoke_org_api_token(
    org: OrgAdmin,
    Path((_, token_id)): Path<(String, String)>,
) -> ApiResult {
    org.app
        .revoke_api_token_of_organization(&org.session.user_id, &token_id)
        .await?;
    Ok(no_content())
}
