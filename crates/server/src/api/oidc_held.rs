//! The two ends of a sign-in that is held while an organization moves to another
//! provider: carrying it over to the account the person had, through the former
//! provider, or ending it in a new account. Both are requests of the login page,
//! behind the API's defence against forgery, from the browser whose cookie holds the
//! sign-in; the return from the former provider is the callback of every sign-in
//! (`oidc.rs`).

use axum::extract::{Extension, State};
use axum::http::header::SET_COOKIE;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse as _;
use serde::Deserialize;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use fvoci_model::{OidcStart, Problem};

use super::auth::{address_of, user_agent};
use super::contract::params::{AcceptLanguage, StartOrg};
use super::contract::tag;
use super::oidc::{browser_cookie, held_cookie_name, started_response};
use super::query::QueryParams;
use super::{ApiResult, accept_language, no_content};
use crate::app::App;
use crate::client_address::ClientAddress;
use crate::error::AppError;
use crate::http_credentials::{self, cookie_value};

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
struct OidcHeldQuery {
    org: String,
}

/// The routes of a held sign-in: they come before there is a session.
pub(super) fn routes() -> OpenApiRouter<App> {
    OpenApiRouter::new()
        .routes(routes!(handle_start_carry_over))
        .routes(routes!(handle_finish_held))
}

/// The organization the request names, and the secret of the browser's cookie of the
/// sign-in held for it; empty if the browser has none.
fn held_of(headers: &HeaderMap, app: &App, org_id: &str) -> Result<(String, String), AppError> {
    if org_id.is_empty() {
        return Err(AppError::bad_request("the query parameter org is required"));
    }
    let secret = cookie_value(headers, held_cookie_name(app)).unwrap_or_default();
    Ok((org_id.to_owned(), secret))
}

/// Carry an account over through the former provider
///
/// For the login page, after a sign-in through the provider of the organization `org`
/// was held for this browser (`finishOidcLogin` sent it to the login page with
/// `oidc_held=1`): for somebody who had an account before the organization moved to
/// this provider. Starts a sign-in through the organization's former provider and
/// answers where the page sends the browser, with the cookie of that sign-in. When
/// the browser comes back (`finishOidcLogin`), the former provider must vouch for an
/// identity tied to an account in use that has a place in the organization (a member,
/// or a guest of one of its workspaces in use, who joins nothing by it): the held
/// identity is then tied to that account, which is recorded in the organization's
/// audit log (`oidc_identity_carried_over`), and a session made through the
/// organization's provider is set. Both identities are shown by signing in, so no
/// email address is compared. The former provider signs nobody in otherwise: no
/// session is made through it.
///
/// 409 with the reason `oidc_carry_over_refused` if no sign-in of the organization is
/// held for this browser (none, ended after ten minutes, taken already, or held for
/// another browser), the organization keeps no former provider any more, or `org`
/// names no organization that is signed in to; the same
/// reason comes back on the login page (`oidc_error`) when the return carried nothing
/// over. 409 with `oidc_unavailable` if the former provider cannot be reached; 429
/// when this address made too many sign-ins of late.
#[utoipa::path(
    post,
    path = "/auth/oidc/held/carry-over",
    operation_id = "startOidcCarryOver",
    tag = tag::AUTH,
    params(StartOrg),
    security(()),
    responses(
        (
            status = 200,
            description = "Started: where the browser goes.",
            body = OidcStart,
            headers(("Set-Cookie" = String, description = "The cookie of the sign-in.")),
        ),
        (
            status = 400,
            description = "No organization named (`org`).",
            body = Problem,
            content_type = "application/problem+json",
        ),
        (
            status = 409,
            description = "Nothing to carry over, or the former provider cannot be reached.",
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
async fn handle_start_carry_over(
    State(app): State<App>,
    client: Option<Extension<ClientAddress>>,
    headers: HeaderMap,
    QueryParams(query): QueryParams<OidcHeldQuery>,
) -> ApiResult {
    let (org_id, secret) = held_of(&headers, &app, &query.org)?;
    let started = app
        .start_oidc_carry_over(&org_id, &secret, address_of(client))
        .await?;
    Ok(started_response(&app, started))
}

/// End a held sign-in in a new account
///
/// For the login page, after a sign-in through the provider of the organization `org`
/// was held for this browser: for somebody who had no account. The held sign-in ends
/// as a first sign-in through the provider does: an account is made for who the
/// provider vouched for, a member of the organization, if the organization lets
/// newcomers in and no account has their email address; the session comes in the
/// `Set-Cookie` header. An identity that was tied to an account meanwhile (carried
/// over from another browser) signs in to that account, as any sign-in would. The
/// held sign-in is taken by this, whatever comes of it, and its cookie cleared.
///
/// 409 with the reason `oidc_carry_over_refused` if no sign-in of the organization is
/// held for this browser, or the provider changed since; 409 with the reason a first
/// sign-in is refused with otherwise (`oidc_join_closed`, `oidc_email_missing`,
/// `oidc_email_unverified`, `oidc_email_taken`, `oidc_unavailable`); 429 when this
/// address made too many sign-ins of late.
///
/// An account that comes into the organization by this, as a member, is given its
/// welcome board unless it has one (`onboard` says more): in the language of its
/// settings, or else in the one `Accept-Language` prefers.
#[utoipa::path(
    post,
    path = "/auth/oidc/held/sign-in",
    operation_id = "finishHeldOidcLogin",
    tag = tag::AUTH,
    params(StartOrg, AcceptLanguage),
    security(()),
    responses(
        (
            status = 204,
            description = "Signed in: to a new account, or to the account the held identity was tied to meanwhile.",
            headers(("Set-Cookie" = String, description = "The session cookie, and the cookie of the held sign-in cleared.")),
        ),
        (
            status = 400,
            description = "No organization named (`org`).",
            body = Problem,
            content_type = "application/problem+json",
        ),
        (
            status = 409,
            description = "Nothing is held, or no account is made for the person.",
            body = Problem,
            content_type = "application/problem+json",
            headers(("Set-Cookie" = String, description = "The cookie of the held sign-in cleared.")),
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
async fn handle_finish_held(
    State(app): State<App>,
    client: Option<Extension<ClientAddress>>,
    headers: HeaderMap,
    QueryParams(query): QueryParams<OidcHeldQuery>,
) -> ApiResult {
    let (org_id, secret) = held_of(&headers, &app, &query.org)?;
    let ended = app
        .finish_held_oidc_login(
            &org_id,
            &secret,
            address_of(client),
            &user_agent(&headers),
            accept_language(&headers),
        )
        .await;
    let mut response = match ended {
        Ok(token) => {
            let mut response = no_content();
            let session =
                http_credentials::set_cookie_header(app.authenticator().session_cookie(), &token);
            response.headers_mut().append(SET_COOKIE, session);
            response
        }
        // Nothing was held, or what was held is taken and made no account: either
        // way the browser's cookie holds nothing any more.
        Err(error) if error.status() == StatusCode::CONFLICT.as_u16() => error.into_response(),
        // Not asked yet (too many requests, a failure of the server's): what is held
        // stays, and so does its cookie.
        Err(error) => return Err(error),
    };
    let cleared = browser_cookie(&app, held_cookie_name(&app), "", 0);
    response.headers_mut().append(SET_COOKIE, cleared);
    Ok(response)
}
