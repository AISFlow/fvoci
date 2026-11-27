//! Login, logout, registration and password change.

use axum::body::Bytes;
use axum::extract::{Extension, State};
use axum::http::header::{SET_COOKIE, USER_AGENT};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse as _, Response};
use fvoci_model::{
    ChangePasswordRequest, ConfirmPasswordRequest, LoginRequest, Problem, RegisterRequest,
    SetPasswordRequest, User,
};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::contract::params::AcceptLanguage;
use super::contract::tag;
use super::signup_cookie::{marker_of, with_marker};

use super::session::AccountSession;
use super::{ApiResult, RequiredSession, accept_language, created, decode_object, no_content};
use crate::app::App;
use crate::client_address::ClientAddress;
use crate::error::AppError;
use crate::http_credentials;

/// The longest `User-Agent` kept with a session, in bytes. Browsers send a hundred or
/// two; anything longer is cut, it is a label and nothing is decided by it.
const USER_AGENT_MAX_BYTES: usize = 512;

/// What the client says it is, cut to [`USER_AGENT_MAX_BYTES`] at a character boundary.
pub(super) fn user_agent(headers: &HeaderMap) -> String {
    let Some(value) = headers
        .get(USER_AGENT)
        .and_then(|value| value.to_str().ok())
    else {
        return String::new();
    };
    let mut end = value.len().min(USER_AGENT_MAX_BYTES);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

/// The routes that need a session, or none of their own doing.
pub(super) fn routes() -> OpenApiRouter<App> {
    OpenApiRouter::new()
        .routes(routes!(handle_logout))
        .routes(routes!(handle_change_password, handle_set_password))
        .routes(routes!(handle_confirm_password))
}

/// The routes anyone can send a body to: they come before there is a session.
pub(super) fn unauthenticated_routes() -> OpenApiRouter<App> {
    OpenApiRouter::new()
        .routes(routes!(handle_login))
        .routes(routes!(handle_register))
}

/// Sign in
///
/// Starts a session for the user with the given name, or, if no user has that name,
/// with the given email address. The session comes in the `Set-Cookie` header and
/// nowhere else.
///
/// Wrong credentials are 401 with the reason `invalid_credentials`, whatever was wrong
/// about them, a missing password too. Attempts are counted at the name and at the
/// email address the request gives, each for itself, and by where they come from; too
/// many are 429. What is counted does not depend on whether there is such an account.
///
/// On a server that asks for the email address of a registration to be confirmed
/// (`emailVerification` of `getLoginOptions`), the right password of a registration
/// that is not confirmed yet is 403 with the reason `email_unverified`, and opens no
/// session; a wrong one is the 401 of no account.
#[utoipa::path(
    post,
    path = "/login",
    operation_id = "login",
    tag = tag::AUTH,
    request_body = LoginRequest,
    security(()),
    responses(
        (
            status = 204,
            description = "The user is signed in.",
            headers(("Set-Cookie" = String, description = "The session cookie.")),
        ),
        (
            status = 429,
            description = "Too many attempts. `retryAfter`, and the `Retry-After` header, say how long to wait; there is no `reason`.",
            body = Problem,
            content_type = "application/problem+json",
            headers(("Retry-After" = u64, description = "How many seconds to wait before trying again.")),
        ),
    ),
)]
async fn handle_login(
    State(app): State<App>,
    client: Option<Extension<ClientAddress>>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult {
    let login: LoginRequest = decode_object(&body)?;
    let token = app
        .login(
            &login.username,
            &login.email,
            &login.password,
            &user_agent(&headers),
            address_of(client),
        )
        .await?;
    // The token goes out in the cookie only: a script of the page never sees it. A
    // client that is no browser reads it from the `Set-Cookie` header.
    Ok(with_session_cookie(&app, &token, no_content()))
}

/// The answer to a request after which the session goes on under a new token: no
/// content, and the token in the cookie in place of the old one.
fn renewed_session(app: &App, token: &str) -> Response {
    with_session_cookie(app, token, no_content())
}

/// Adds the session cookie with the token to a response.
pub(super) fn with_session_cookie(app: &App, token: &str, mut response: Response) -> Response {
    let cookie = http_credentials::set_cookie_header(app.authenticator().session_cookie(), token);
    response.headers_mut().insert(SET_COOKIE, cookie);
    response
}

/// Sign out
///
/// Ends the session of the request and clears its cookie. The session of an account
/// that waits to be deleted is ended too.
#[utoipa::path(
    post,
    path = "/logout",
    operation_id = "logout",
    tag = tag::AUTH,
    responses((
        status = 204,
        description = "The session is over.",
        headers(("Set-Cookie" = String, description = "Clears the session cookie.")),
    )),
)]
async fn handle_logout(State(app): State<App>, account: AccountSession) -> ApiResult {
    app.logout(&account.session.id).await?;
    let mut response = no_content();
    let headers = response.headers_mut();
    headers.insert(
        SET_COOKIE,
        http_credentials::clear_cookie_header(app.authenticator().session_cookie()),
    );
    headers.insert(
        axum::http::HeaderName::from_static("clear-site-data"),
        axum::http::HeaderValue::from_static("\"cache\""),
    );
    Ok(response)
}

/// The address of the client, as the request log worked it out.
pub(super) fn address_of(client: Option<Extension<ClientAddress>>) -> Option<std::net::IpAddr> {
    client.and_then(|Extension(ClientAddress(address))| address)
}

/// Register a user
///
/// Makes an account, with its own space (see "The user's own space"), and the
/// organization it starts if it names one: `organizationName` names the organization
/// and its first workspace, and the account administers both. Without
/// `organizationName` (left out, or blank) the account starts with its own space
/// alone, in no organization, and may start one later (`createOrganization`).
///
/// The first user of a server starts the server's own organization and administers
/// the server too: until there is one (`organizationRequired` of `getLoginOptions`), a
/// registration without `organizationName` is 400, and `errors` names it with the
/// reason `required`. On a server that takes one organization (`org_creation` is
/// `first_user_only`) only the first user registers; everybody after is
/// refused with 403 and the reason `invitation_required`, whatever else the request
/// says, and gets in by the link of an invitation (`acceptInvitation`). A name or an
/// email address that is taken is 409 with the reason `username_taken` or
/// `email_taken`. Registering does not sign in.
///
/// On a server that asks for the email address to be confirmed (`emailVerification` of
/// `getLoginOptions`), registering makes no account: it answers 202 with no body, and
/// mails a link to the address, in the language of `language` (English or Korean), that
/// makes the account, and the organization if it names one, and signs in
/// (`completeEmailVerification`).
/// Until then there is nothing of it but its name and address, which are taken as an
/// account's are, for seven days; a registration that is not confirmed by then is
/// removed, and its name and address are free again. The 202 sets a cookie that tells
/// the browser the registration was made from (`fvoci_signup`, `__Host-fvoci_signup`
/// over HTTPS; `HttpOnly`, for the seven days): no session, it reaches nothing, and
/// shows `completeEmailVerification` that the browser is the registrant's. A
/// registration from a browser that has the cookie of one that waits still replaces
/// that one: whoever mistyped an address registers again at once, under the same user
/// name, and the link mailed for the first confirms nothing any more. Without the
/// cookie the name and the address of a registration that waits are taken.
///
/// The name and the email address must not be blank, the email address must look like
/// one, the display name (`displayName`, may be left out) is checked as `patchMe` checks it,
/// the password must have at least 8 characters and at most 1024 bytes, and the organization
/// name, where there is one, is read as a display name is; otherwise the
/// request is 400, and `errors` names every member that is refused, each with
/// its `reason`: `required`, `invalid_format`, `too_short`, `too_long` or
/// `invalid_characters`.
#[utoipa::path(
    post,
    path = "/register",
    operation_id = "register",
    tag = tag::AUTH,
    params(AcceptLanguage),
    request_body = RegisterRequest,
    security(()),
    responses(
        (status = 201, description = "The user that was made.", body = User),
        (
            status = 202,
            description = "The registration waits for its email address to be confirmed by the link mailed to it.",
            headers(("Set-Cookie" = String, description = "The cookie that tells the browser the registration was made from; no session.")),
        ),
        (
            status = 429,
            description = "Too many registrations from where this one comes from, or the server has too much password work to do at the moment.",
            body = Problem,
            content_type = "application/problem+json",
            headers(("Retry-After" = u64, description = "How many seconds to wait before trying again.")),
        ),
    ),
)]
async fn handle_register(
    State(app): State<App>,
    client: Option<Extension<ClientAddress>>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult {
    let register: RegisterRequest = decode_object(&body)?;
    // Asked first, so that a request that is refused whatever it says costs no
    // password hash; whether a registration really is the first is decided where the
    // user is stored.
    if !app.takes_registrations().await? {
        return Err(AppError::invitation_required());
    }
    // Until the server has its first user a registration names the organization it
    // starts: told with whatever else is wrong, and asked again where it is stored.
    let registration = if app.has_first_user().await? {
        register.check()?
    } else {
        register.check_as_first()?
    };
    if app.verifies_email() {
        let browser = marker_of(&app, &headers);
        let marker = app
            .sign_up_to_confirm(&registration, browser.as_deref(), address_of(client))
            .await?;
        return Ok(with_marker(
            &app,
            &marker,
            StatusCode::ACCEPTED.into_response(),
        ));
    }
    let user = app
        .sign_up_with_language(&registration, address_of(client), accept_language(&headers))
        .await?;
    Ok(created(&user))
}

/// Change the password
///
/// Changes the password of the user of the session, who proves to know the old one.
/// The user's other sessions end, and this one goes on under a new cookie.
///
/// A wrong old password is 400 (not 401: the session is good) with the reason
/// `invalid_credentials`, and `errors` names `oldPassword`. Wrong ones are counted with the
/// failed logins to the account by its name, and too many are 429, also for a login.
/// The new password must have at least 8 characters and at most 1024 bytes; `errors`
/// names every member that is refused. A password
/// that was changed by somebody else while this request was under way is 409.
#[utoipa::path(
    post,
    path = "/users/me/password",
    operation_id = "changePassword",
    tag = tag::AUTH,
    request_body = ChangePasswordRequest,
    responses(
        (
            status = 204,
            description = "The password is changed.",
            headers(("Set-Cookie" = String, description = "The session cookie, with its new value.")),
        ),
        (
            status = 429,
            description = "The old password was wrong too often, or the server has too much password work to do at the moment.",
            body = Problem,
            content_type = "application/problem+json",
            headers(("Retry-After" = u64, description = "How many seconds to wait before trying again.")),
        ),
    ),
)]
async fn handle_change_password(
    State(app): State<App>,
    RequiredSession(session): RequiredSession,
    client: Option<Extension<ClientAddress>>,
    body: Bytes,
) -> ApiResult {
    let request: ChangePasswordRequest = decode_object(&body)?;
    request.validate()?;
    let address = address_of(client);

    let token = app
        .change_password(
            &session,
            &request.old_password,
            &request.new_password,
            address,
        )
        .await?;
    Ok(renewed_session(&app, &token))
}

/// Set the first password
///
/// Gives the account of the session its first password: for an account that has none
/// (`hasPassword` of `getMe`), which was made by a sign-in through an organization's
/// provider. No password is asked for, as there is none: what vouches is a session
/// made through a provider in the last five minutes. Any other session is 403 with
/// the reason `sign_in_again`: the account signs in through the provider again
/// (`startOidcLogin`) and sends the request from the session that comes of it.
/// From then on the account signs in with it (`login`) by its user name, and reaches
/// its own space so, also once it is no longer in the organization whose provider made
/// it; an organization that takes only sessions made through its provider goes on
/// asking for those (`sso_required`). The account's other sessions end, and this one
/// goes on under a new cookie, as after `changePassword`.
///
/// From then on the account's own space takes only sessions that have shown the
/// password (`password_sign_in_required`, see "The user's own space"): this session,
/// which was made through a provider and set the password without being asked for
/// one, opens it no more until it gives the password (`confirmPassword`). Its
/// websocket connections stay open and hear no more of the space; those of the
/// account's other sessions are closed.
///
/// Once only: an account that has a password is 409, and changes it with
/// `changePassword`. The password must have at least 8 characters and at most 1024
/// bytes; otherwise the request is 400 and `errors` names `password`.
#[utoipa::path(
    put,
    path = "/users/me/password",
    operation_id = "setPassword",
    tag = tag::AUTH,
    request_body = SetPasswordRequest,
    responses(
        (
            status = 204,
            description = "The account has a password.",
            headers(("Set-Cookie" = String, description = "The session cookie, with its new value.")),
        ),
        (
            status = 429,
            description = "The server has too much password work to do at the moment.",
            body = Problem,
            content_type = "application/problem+json",
            headers(("Retry-After" = u64, description = "How many seconds to wait before trying again.")),
        ),
    ),
)]
async fn handle_set_password(
    State(app): State<App>,
    RequiredSession(session): RequiredSession,
    body: Bytes,
) -> ApiResult {
    let request: SetPasswordRequest = decode_object(&body)?;
    request.validate()?;
    let token = app.set_password(&session, &request.password).await?;
    Ok(with_session_cookie(&app, &token, no_content()))
}

/// Confirm the password in this session
///
/// Takes the session as one that has shown the account's password: for a session made
/// through an organization's provider, to which the account's own space is closed once
/// the account has a password (`password_sign_in_required`, see "The user's own
/// space"). From then until the session ends it opens the space as a session made
/// with the password does, `getMyWorkspaces` lists the space with `passwordRequired`
/// false, and it goes on opening what it opened of the organizations: nothing else
/// changes of what it opens, and an organization that takes only sessions made
/// through another provider stays closed to it (`sso_required`). A session made with
/// the password is answered the same and opens what it opened.
///
/// The session goes on under a new cookie, as after `changePassword`: it opens more
/// than it did, and the value that opened less is no longer accepted. Its websocket
/// connections stay open, and are the confirmed session's from then on: what the
/// space refused them is asked again on the same connection.
///
/// A wrong password is 400 (not 401: the session is good) with the reason
/// `invalid_credentials`, and `errors` names `password`. Wrong ones are counted with
/// the failed logins to the account by its name, and too many are 429, also for a
/// login: the limits are those of `login`. An account that has no password
/// (`hasPassword` of `getMe`) is 409: its space takes any session, and there is
/// nothing to confirm. A password that was changed while this request was under way
/// is 409 too.
#[utoipa::path(
    post,
    path = "/users/me/password/confirm",
    operation_id = "confirmPassword",
    tag = tag::AUTH,
    request_body = ConfirmPasswordRequest,
    responses(
        (
            status = 204,
            description = "The session has shown the password.",
            headers(("Set-Cookie" = String, description = "The session cookie, with its new value.")),
        ),
        (
            status = 429,
            description = "The password was wrong too often, or the server has too much password work to do at the moment.",
            body = Problem,
            content_type = "application/problem+json",
            headers(("Retry-After" = u64, description = "How many seconds to wait before trying again.")),
        ),
    ),
)]
async fn handle_confirm_password(
    State(app): State<App>,
    RequiredSession(session): RequiredSession,
    client: Option<Extension<ClientAddress>>,
    body: Bytes,
) -> ApiResult {
    let request: ConfirmPasswordRequest = decode_object(&body)?;
    let token = app
        .confirm_session_password(&session, &request.password, address_of(client))
        .await?;
    Ok(renewed_session(&app, &token))
}
