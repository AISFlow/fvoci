//! Extractors that resolve the session of a request, and the renewal of the session
//! cookie.

use std::sync::{Arc, Mutex};

use axum::extract::{FromRequestParts, Request, State};
use axum::http::header::SET_COOKIE;
use axum::http::request::Parts;
use axum::http::{Extensions, HeaderMap};
use axum::middleware::Next;
use axum::response::Response;
use fvoci_model::{Leaving, Session};

use super::delegated::Admits;
use crate::app::{App, Caller};
use crate::auth::api_token;
use crate::auth::token::Credential;
use crate::client_address::ClientAddress;
use crate::error::{AppError, ErrorKind, Reason};
use crate::http_credentials::{self, bearer_token};

/// The session of a request to a route that needs one. Extraction fails with 401 if the
/// request carries no valid session token.
#[derive(Debug)]
pub(crate) struct RequiredSession(pub(crate) Session);

/// The session of a request to a route that also serves whoever has none: the
/// acceptance of an invitation, which makes an account for them. `None` if the request
/// carries no valid session token.
#[derive(Debug)]
pub(crate) struct OptionalSession(pub(crate) Option<Session>);

impl FromRequestParts<App> for OptionalSession {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, app: &App) -> Result<Self, Self::Rejection> {
        optional_session_of(app, &parts.headers, &parts.extensions)
            .await
            .map(Self)
    }
}

/// The session of a request, as [`session_of`] finds it, or `None` if the request
/// carries no valid session token.
///
/// # Errors
///
/// Returns the 401 of a session whose account waits to be deleted (reason
/// `account_deletion_pending`), and the store's error if the session cannot be looked
/// up.
pub(super) async fn optional_session_of(
    app: &App,
    headers: &HeaderMap,
    extensions: &Extensions,
) -> Result<Option<Session>, AppError> {
    match session_of(app, headers, extensions).await {
        Ok(session) => Ok(Some(session)),
        // The session of an account that waits to be deleted is not "no session": it
        // opens nothing but the way back, here as everywhere.
        Err(error) if error.reason() == Some(Reason::AccountDeletionPending) => Err(error),
        // No session is an answer the route can work with. A store that failed is not
        // "no session": answered as one, it would make clients throw a valid session
        // away.
        Err(error) if error.kind() == ErrorKind::Unauthorized => Ok(None),
        Err(error) => Err(error),
    }
}

impl FromRequestParts<App> for RequiredSession {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, app: &App) -> Result<Self, Self::Rejection> {
        session_of(app, &parts.headers, &parts.extensions)
            .await
            .map(Self)
    }
}

/// The session of a request to a route that also serves an account that waits to be
/// deleted: what deleting comes to, taking the asking back, and signing out. `leaving`
/// says until when the account waits; `None` for an account in use. Extraction fails
/// with 401 if the request carries no token of either.
#[derive(Debug)]
pub(crate) struct AccountSession {
    pub(crate) session: Session,
    pub(crate) leaving: Option<Leaving>,
}

impl FromRequestParts<App> for AccountSession {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, app: &App) -> Result<Self, Self::Rejection> {
        match session_of(app, &parts.headers, &parts.extensions).await {
            Ok(session) => Ok(Self {
                session,
                leaving: None,
            }),
            Err(error) if error.reason() == Some(Reason::AccountDeletionPending) => {
                let credential = http_credentials::session_credential(
                    app.authenticator().session_cookie(),
                    &parts.headers,
                );
                let found = match &credential {
                    Some(credential) => {
                        app.authenticator()
                            .leaving_session(credential.token())
                            .await?
                    }
                    None => None,
                };
                // Taken back, or erased, between the two lookups: the next request says.
                let (session, leaving) = found.ok_or(error)?;
                Ok(Self {
                    session,
                    leaving: Some(leaving),
                })
            }
            Err(error) => Err(error),
        }
    }
}

/// Whether a request says it comes with a session, valid or not.
pub(crate) fn claims_session(app: &App, headers: &HeaderMap) -> bool {
    http_credentials::session_credential(app.authenticator().session_cookie(), headers).is_some()
}

/// The session of a request, from the token in its `Authorization` header or in its
/// session cookie.
///
/// If this lookup is the one that writes down the use of a session that came in a
/// cookie, the cookie is due for renewal: that is noted in the request's
/// [`CookieRenewal`], where [`renew_session_cookie`] finds it.
///
/// # Errors
///
/// Returns a 401 if the request carries no token or one that is no session's, and the
/// store's error if the session cannot be looked up.
pub(crate) async fn session_of(
    app: &App,
    headers: &HeaderMap,
    extensions: &Extensions,
) -> Result<Session, AppError> {
    let credential =
        http_credentials::session_credential(app.authenticator().session_cookie(), headers);
    let Some(credential) = credential else {
        return Err(AppError::unauthorized("no session token"));
    };
    let authenticated = app.authenticator().get_session(credential.token()).await?;
    if authenticated.use_recorded
        && let Credential::Cookie(token) = credential
        && let Some(renewal) = extensions.get::<CookieRenewal>()
    {
        renewal.note(token);
    }
    Ok(authenticated.session)
}

/// Who sends a request to a route that takes what `admits` says: its session, or, where
/// it takes one, the personal API token in its `Authorization` header. A token in the
/// session cookie is no token: a page cannot send one by itself.
///
/// # Errors
///
/// Returns the errors of [`session_of`] (403 with the reason `token_forbidden_endpoint`
/// for a token where `admits` takes none); for a token, those of
/// `App::caller_by_token` and 403 with the reason `token_scope_insufficient` for one
/// that reads only where the route writes.
pub(crate) async fn caller_of(
    app: &App,
    headers: &HeaderMap,
    extensions: &Extensions,
    admits: Admits,
) -> Result<Caller, AppError> {
    if let Admits::Tokens(needed) = admits
        && let Some(token) = bearer_token(headers)
        && api_token::is_api_token(token)
    {
        let address = extensions
            .get::<ClientAddress>()
            .and_then(|ClientAddress(address)| *address);
        let caller = app.caller_by_token(token, address).await?;
        caller.require_scope(needed)?;
        return Ok(caller);
    }
    session_of(app, headers, extensions).await.map(Caller::from)
}

/// Where a request's handlers note that the session cookie is due for renewal: the
/// token to send back, with a fresh lifetime.
#[derive(Debug, Clone, Default)]
pub(crate) struct CookieRenewal(Arc<Mutex<Option<String>>>);

impl CookieRenewal {
    fn note(&self, token: String) {
        *self
            .0
            .lock()
            .expect("the renewal note is never held across a panic") = Some(token);
    }

    fn take(&self) -> Option<String> {
        self.0
            .lock()
            .expect("the renewal note is never held across a panic")
            .take()
    }
}

/// Sends the session cookie again, with its full lifetime, on the response to the
/// request that recorded the session's use: a browser forgets the cookie when the
/// session has been idle for as long as the server would let it, and not before.
///
/// A response that sets the cookie itself (login, logout, a changed password) is left
/// as it is.
pub(crate) async fn renew_session_cookie(
    State(app): State<App>,
    mut request: Request,
    next: Next,
) -> Response {
    let renewal = CookieRenewal::default();
    request.extensions_mut().insert(renewal.clone());
    let mut response = next.run(request).await;
    if let Some(token) = renewal.take()
        && !response.headers().contains_key(SET_COOKIE)
    {
        let cookie =
            http_credentials::set_cookie_header(app.authenticator().session_cookie(), &token);
        response.headers_mut().insert(SET_COOKIE, cookie);
    }
    response
}
