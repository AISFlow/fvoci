//! Signing in through an organization's `OpenID Connect` provider: what the login page
//! may know beforehand, the start, and the return from the provider.
//!
//! The start is a request of the login page, behind the API's defence against forgery:
//! it answers where the browser goes. The return is an address the provider sends the
//! browser to: it answers with a redirect (303), to the web app, or to the login page
//! with the name of what went wrong in its `oidc_error` parameter. The browser that
//! starts a sign-in gets a cookie of its own for it, and only the browser with that
//! cookie finishes it.

use axum::body::Bytes;
use axum::extract::{Extension, Path, Query, State};
use axum::http::header::{LOCATION, SET_COOKIE};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use fvoci_model::{
    LoginOptions, OidcStart, OidcStartRequest, Problem, RefusalReason, Refusals, shown_name,
};

use super::auth::{address_of, user_agent};
use super::contract::params::{
    AcceptLanguage, CallbackOrg, LoginOrg, OidcCode, OidcError, OidcIssuer, OidcState, StartOrg,
};
use super::contract::tag;
use super::query::QueryParams;
use super::{ApiResult, accept_language, decode_json, json_object, json_response};
use crate::app::{
    App, BackTo, Callback, FinishedLogin, HELD_SIGN_IN_LIFETIME_MILLIS, LOGIN_FLOW_LIFETIME_MILLIS,
    LoginFailure, StartedLogin,
};
use crate::auth::token::{Credential, Https};
use crate::client_address::ClientAddress;
use crate::error::AppError;
use crate::http_credentials::{self, cookie_value};

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
struct LoginOptionsQuery {
    org: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
struct OidcStartQuery {
    org: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct OidcCallbackQuery {
    state: String,
    code: String,
    error: Option<String>,
    iss: Option<String>,
}

/// The cookie that ties a sign-in to the browser that started it, where the server is
/// reached over HTTPS: the `__Host-` prefix keeps other hosts of the domain from
/// planting one.
const FLOW_COOKIE_SECURE: &str = "__Host-fvoci_oidc";
/// The same cookie where the server is reached over plain HTTP.
const FLOW_COOKIE_PLAIN: &str = "fvoci_oidc";
/// The cookie of the sign-in held for a browser (`oidc_held.rs`), over HTTPS.
const HELD_COOKIE_SECURE: &str = "__Host-fvoci_oidc_held";
/// The same cookie where the server is reached over plain HTTP.
const HELD_COOKIE_PLAIN: &str = "fvoci_oidc_held";

/// The routes anyone may call: they come before there is a session.
pub(super) fn unauthenticated_routes() -> OpenApiRouter<App> {
    OpenApiRouter::new()
        .routes(routes!(handle_login_options))
        .routes(routes!(handle_start))
        .routes(routes!(handle_callback))
        .merge(super::oidc_held::routes())
}

/// What the login page shows
///
/// The organization the login page is for, and whether people sign in through its
/// provider: the one `org` names, or without it the only organization of a server that
/// holds one. Its identifier, its name and whether it has a provider that is on, and
/// nothing else. An identifier that names no organization in use answers as none
/// (`organization` is `null`), and so does a server that takes in several
/// organizations when none is named.
///
/// `registrationOpen` says whether `register` takes somebody without an invitation:
/// always on a server where everybody starts an organization of their own, and until the first
/// user registered on a server that holds one organization. `emailVerification` says
/// whether a registration waits for the link mailed to its address
/// (`completeEmailVerification`) before there is an account.
#[utoipa::path(
    get,
    path = "/login-options",
    operation_id = "getLoginOptions",
    tag = tag::AUTH,
    params(LoginOrg),
    security(()),
    responses((status = 200, description = "What the login page shows.", body = LoginOptions)),
)]
async fn handle_login_options(
    State(app): State<App>,
    QueryParams(query): QueryParams<LoginOptionsQuery>,
) -> ApiResult {
    let options = app.login_options(query.org.as_deref()).await?;
    Ok(json_response(&options))
}

/// Start signing in through a provider
///
/// For the login page of the server, as a request of its own, not an address a browser
/// is sent to: like every request that changes something it needs the header
/// `X-Requested-With: XMLHttpRequest` and no foreign `Origin`, which a page of another
/// site cannot send, so that no such page can sign a browser in to an account of its
/// choosing. Starts a sign-in for the organization `org` and answers where the page
/// sends the browser (the provider), with a cookie that ties the sign-in to this
/// browser, and ten minutes to come back. 409 with the reason `oidc_unavailable` if
/// the organization has no provider that is on, or it cannot be reached; 429 when
/// this address made too many sign-ins of late.
///
/// With `linkPassword` in the body the sign-in links: it needs the session of the
/// browser's cookie (401 without; a bearer token does not do, as the browser comes back
/// with its cookie), whose account's password that is, and when the browser comes back, who the provider
/// vouches for is tied to that account in place of signing in (`getMyIdentities`);
/// after that the provider signs in to the account. No account is made and none is
/// joined to another: an identity that is another account's is refused
/// (`oidc_identity_taken`), and so is one whose email address, as the provider has
/// checked it, is not the account's own (`oidc_email_mismatch`): the password ties no
/// other person of the provider to the account. It ends only in the browser that started it and still has
/// the session it was started from (`oidc_state_invalid` otherwise). A wrong password
/// is 400 with the reason `invalid_credentials`, naming `/linkPassword`, counted as a
/// wrong one of `changePassword` is; an account without a password is 409 with the
/// reason `password_not_set`. The session need not open the organization. A body with
/// another member, or a `linkPassword` that is no text, is 400 naming it: what was
/// meant to link does not sign in instead.
#[utoipa::path(
    post,
    path = "/auth/oidc/start",
    operation_id = "startOidcLogin",
    tag = tag::AUTH,
    params(StartOrg),
    request_body(
        content = Option<OidcStartRequest>,
        description = "May be left out, to sign in.",
    ),
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
            description = "No provider that is on, or it cannot be reached.",
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
async fn handle_start(
    State(app): State<App>,
    client: Option<Extension<ClientAddress>>,
    headers: HeaderMap,
    QueryParams(query): QueryParams<OidcStartQuery>,
    body: Bytes,
) -> ApiResult {
    let org_id = query.org;
    if org_id.is_empty() {
        return Err(AppError::bad_request("the query parameter org is required"));
    }
    let address = address_of(client);
    let started = match link_password(&body)? {
        None => app.start_oidc_login(&org_id, address).await?,
        Some(password) => {
            // Linking is the account's own doing, with a session of it, whatever
            // organization that session opens: the session of the browser's cookie,
            // which is the one the browser comes back with.
            let token = match http_credentials::session_credential(
                app.authenticator().session_cookie(),
                &headers,
            ) {
                Some(Credential::Cookie(token)) => token,
                Some(Credential::Bearer(_)) | None => {
                    return Err(AppError::unauthorized(
                        "linking needs the session of the browser's cookie",
                    ));
                }
            };
            let session = app.authenticator().find_session(&token).await?;
            app.start_oidc_link(&org_id, &session, &password, address)
                .await?
        }
    };
    Ok(started_response(&app, started))
}

/// The answer to a start of a sign-in: where the browser goes, and the cookie that ties
/// the sign-in to it.
pub(super) fn started_response(app: &App, started: StartedLogin) -> Response {
    let mut response = json_response(&OidcStart {
        authorization_url: started.authorization_url,
    });
    let cookie = flow_cookie(
        app,
        &started.browser_secret,
        LOGIN_FLOW_LIFETIME_MILLIS / 1000,
    );
    response.headers_mut().append(SET_COOKIE, cookie);
    response
}

/// Come back from a provider
///
/// Where a provider sends the browser back to: each organization has an address of its
/// own, its provider's `redirectUri`, and a sign-in started for one organization does
/// not finish at another's address, nor with an `iss` (RFC 9207) other than the
/// provider's, before any code is exchanged. The sign-in is taken by its `state`, once, and only from the
/// browser that started it; another browser takes nothing. The code is
/// exchanged for an ID token, whose signature, issuer, audience, time and nonce are
/// checked. The person is known by the issuer and the subject: an account of theirs
/// signs in, and somebody who has none gets one, if the organization lets newcomers in
/// and no account has their email address. A registration that waits for that address
/// to be confirmed (`register`, `acceptInvitation`) stands in nobody's way. Where the
/// provider vouches for the address, one made by a link of this organization is
/// removed with the sign-in, new account or known: the use it took of the link is
/// given back, and the link mailed for it confirms nothing any more. A registration
/// of its own, or by a link of another organization, stays, and so does any at a
/// trial of pending settings and at a sign-in that carries over. The session comes in the `Set-Cookie`
/// header, and the browser goes to the web app (303). A sign-in that was started to
/// link sets no session: the browser goes to the web app with `oidc_linked=1` in the
/// query. A trial of the provider's pending settings (`startOidcTrial`) that ends well
/// makes them the provider's and sets the session, made through them, and the browser
/// goes to the organization's page (`/org/{orgId}`) with `oidc_pending=applied`; one
/// that fails changes nothing, and goes there with `oidc_pending_error` naming the
/// reason as `oidc_error` does (among them `oidc_trial_account` and
/// `oidc_trial_stale`).
///
/// Otherwise the browser goes to the login page of the organization (`/login?org=…`),
/// with the reason of what went wrong in `oidc_error`, by the name of its problem
/// (`oidc_state_invalid` as `oidc-state-invalid`): `oidc_state_invalid`, `oidc_denied`,
/// `oidc_token_refused`, `oidc_token_invalid`, `oidc_email_missing`,
/// `oidc_email_unverified`, `oidc_email_taken`, `oidc_join_closed`,
/// `oidc_identity_taken`, `oidc_email_mismatch`, `oidc_carry_over_refused` or
/// `oidc_unavailable`.
///
/// While the organization keeps the provider it moved away from (`former` of its
/// provider), a sign-in that vouches for somebody who has no account makes none: what
/// the provider said is held for this browser for ten minutes, under a cookie of its
/// own, and the browser goes to the login page with `oidc_held=1`, which asks whether
/// the person had an account: `startOidcCarryOver` if they had, `finishHeldOidcLogin`
/// if not. A sign-in through the former provider, which `startOidcCarryOver` starts,
/// comes back here too: it sets a session only by carrying the held sign-in over.
/// Where nothing is carried over it goes to the login page with
/// `oidc_carry_over_refused`, and where the sign-in itself failed (refused by the
/// former provider, its token not taken, not this browser's) with that reason, as
/// any sign-in does. An account that comes into the organization by the sign-in, as a
/// member, is given its welcome board unless it has one (`onboard` says more): in the
/// language of its settings, or else in the one `Accept-Language` prefers.
#[utoipa::path(
    get,
    path = "/auth/oidc/callback/{orgId}",
    operation_id = "finishOidcLogin",
    tag = tag::AUTH,
    params(CallbackOrg, OidcState, OidcCode, OidcError, OidcIssuer, AcceptLanguage),
    security(()),
    responses((
        status = 303,
        description = "To the web app, signed in or (with `oidc_linked=1`) linked; to the login page, with `oidc_error` for a sign-in that failed or `oidc_held=1` for one that is held; or to the organization's page after a trial.",
        headers(
            ("Location" = String, description = "Where the browser goes."),
            ("Set-Cookie" = String, description = "The session cookie, unless the sign-in linked, is held or failed; the cookie of the held sign-in, set where one is held and cleared where one was carried over or left behind; and the cookie of the sign-in cleared."),
        ),
    )),
)]
async fn handle_callback(
    State(app): State<App>,
    Path(org_id): Path<String>,
    headers: HeaderMap,
    Query(query): Query<OidcCallbackQuery>,
) -> ApiResult {
    let callback = Callback {
        state: query.state,
        code: query.code,
        error: query.error,
        browser_secret: cookie_value(&headers, flow_cookie_name(&app)).unwrap_or_default(),
        org_id,
        issuer: query.iss,
        // The session a browser comes back with is the one of its cookie.
        session_token: match http_credentials::session_credential(
            app.authenticator().session_cookie(),
            &headers,
        ) {
            Some(Credential::Cookie(token)) => Some(token),
            Some(Credential::Bearer(_)) | None => None,
        },
        held_secret: cookie_value(&headers, held_cookie_name(&app)),
        accept_language: accept_language(&headers).map(str::to_owned),
    };
    let came_with_held = callback.held_secret.is_some();
    let returned = app
        .finish_oidc_login(&callback, &user_agent(&headers))
        .await;
    let org_id = returned.org_id.as_deref();
    let signed_in = |location: &str, token: &str| {
        let mut response = see_other(location);
        let cookie =
            http_credentials::set_cookie_header(app.authenticator().session_cookie(), token);
        response.headers_mut().append(SET_COOKIE, cookie);
        response
    };
    let mut response = match (returned.result, org_id) {
        (Ok(FinishedLogin::SignedIn { token } | FinishedLogin::Applied { token }), None)
        | (Ok(FinishedLogin::SignedIn { token }), _) => signed_in(&app_root(&app), &token),
        // The session there goes on; the web app is told that the link is made.
        (Ok(FinishedLogin::Linked), _) => see_other(&format!("{}?oidc_linked=1", app_root(&app))),
        (Ok(FinishedLogin::Applied { token }), Some(org_id)) => signed_in(
            &format!("{}?oidc_pending=applied", org_page(&app, org_id)),
            &token,
        ),
        (Err(failure), Some(org_id)) if returned.back_to == BackTo::Organization => {
            see_other(&format!(
                "{}?oidc_pending_error={}",
                org_page(&app, org_id),
                failure.name()
            ))
        }
        // Held: the login page asks whether the person had an account, and the
        // browser keeps the cookie of what is held for it.
        (Ok(FinishedLogin::Held { secret }), org_id) => {
            let mut response = see_other(&held_page(&app, org_id));
            let seconds = HELD_SIGN_IN_LIFETIME_MILLIS / 1000;
            let cookie = browser_cookie(&app, held_cookie_name(&app), &secret, seconds);
            response.headers_mut().append(SET_COOKIE, cookie);
            return Ok(with_flow_cookie_cleared(&app, response));
        }
        (Err(failure), _) => to_login_page(&app, org_id, failure),
    };
    // A sign-in held for this browser before is over with this one: carried over, or
    // left behind by a sign-in that went another way.
    if came_with_held {
        let cleared = browser_cookie(&app, held_cookie_name(&app), "", 0);
        response.headers_mut().append(SET_COOKIE, cleared);
    }
    Ok(with_flow_cookie_cleared(&app, response))
}

/// `response` with the cookie of the sign-in cleared: the sign-in is over, whatever
/// came of it.
fn with_flow_cookie_cleared(app: &App, mut response: Response) -> Response {
    response
        .headers_mut()
        .append(SET_COOKIE, flow_cookie(app, "", 0));
    response
}

/// The login page of the organization `org_id`, where it is an identifier, as it asks
/// whoever a sign-in is held for whether they had an account.
fn held_page(app: &App, org_id: Option<&str>) -> String {
    let mut location = format!("{}login?", app_root(app));
    if let Some(org_id) = org_id.filter(|org_id| fvoci_model::is_id(org_id)) {
        location.push_str("org=");
        location.push_str(org_id);
        location.push('&');
    }
    location.push_str("oidc_held=1");
    location
}

/// The password of a request to start a sign-in that links; `None` for a body that is
/// left out or has no members, which starts a sign-in.
///
/// # Errors
///
/// Returns 400 naming the member for a `linkPassword` that is no text (`null` too) and
/// for any other member: a request that was meant to link must not sign in instead,
/// into whatever account the provider vouches for.
fn link_password(body: &Bytes) -> Result<Option<String>, AppError> {
    if body.is_empty() {
        return Ok(None);
    }
    let mut password = None;
    let mut refused = Refusals::new();
    for (name, value) in json_object(decode_json(body)?)? {
        match (name.as_str(), value) {
            ("linkPassword", serde_json::Value::String(text)) => password = Some(text),
            ("linkPassword", _) => refused.refuse(
                "linkPassword",
                RefusalReason::Invalid,
                "linkPassword must be a string",
            ),
            (other, _) => refused.refuse(
                other,
                RefusalReason::Unknown,
                format!("there is no member {:?}", shown_name(other)),
            ),
        }
    }
    refused.into_result()?;
    Ok(password)
}

/// A redirect to `location`, which the server made: always a valid header value.
fn see_other(location: &str) -> Response {
    let mut response = StatusCode::SEE_OTHER.into_response();
    if let Ok(value) = HeaderValue::from_str(location) {
        response.headers_mut().insert(LOCATION, value);
    } else {
        tracing::error!("a redirect to an address that is no header value");
        *response.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
    }
    response
}

/// The address of the web app, from the server's own address.
fn app_root(app: &App) -> String {
    app.settings().server_url().at("/")
}

/// The page of the organization `org_id` in the web app, where it is an identifier.
fn org_page(app: &App, org_id: &str) -> String {
    if fvoci_model::is_id(org_id) {
        format!("{}org/{org_id}", app_root(app))
    } else {
        app_root(app)
    }
}

/// A redirect to the login page of the organization `org_id`, if one is known, with the
/// name of what went wrong.
fn to_login_page(app: &App, org_id: Option<&str>, failure: LoginFailure) -> Response {
    let mut location = format!("{}login?", app_root(app));
    // The identifier goes back only if it is one: nothing of the request is repeated.
    if let Some(org_id) = org_id.filter(|org_id| fvoci_model::is_id(org_id)) {
        location.push_str("org=");
        location.push_str(org_id);
        location.push('&');
    }
    location.push_str("oidc_error=");
    location.push_str(&failure.name());
    see_other(&location)
}

fn flow_cookie_name(app: &App) -> &'static str {
    if app.authenticator().origins().https() == Https::Yes {
        FLOW_COOKIE_SECURE
    } else {
        FLOW_COOKIE_PLAIN
    }
}

/// The name of the cookie of the sign-in held for a browser, as [`flow_cookie_name`]
/// names the one of a sign-in under way.
pub(super) fn held_cookie_name(app: &App) -> &'static str {
    if app.authenticator().origins().https() == Https::Yes {
        HELD_COOKIE_SECURE
    } else {
        HELD_COOKIE_PLAIN
    }
}

/// The cookie of a sign-in.
fn flow_cookie(app: &App, secret: &str, max_age_seconds: i64) -> HeaderValue {
    browser_cookie(app, flow_cookie_name(app), secret, max_age_seconds)
}

/// A cookie that ties a sign-in, or a registration that waits, to this browser: not
/// readable by scripts, sent along with the browser's return from the provider (a
/// top-level navigation, which `SameSite=Lax` lets through), for the whole host as the
/// `__Host-` prefix wants.
pub(super) fn browser_cookie(
    app: &App,
    name: &str,
    secret: &str,
    max_age_seconds: i64,
) -> HeaderValue {
    let secure = if app.authenticator().origins().https() == Https::Yes {
        "; Secure"
    } else {
        ""
    };
    let cookie = format!(
        "{name}={secret}; Max-Age={max_age_seconds}; Path=/; HttpOnly; SameSite=Lax{secure}"
    );
    // A secret is base64url text and the rest is fixed: always a valid header value.
    HeaderValue::from_str(&cookie).unwrap_or_else(|_| HeaderValue::from_static(""))
}
