//! The cookie of the browser a registration was made from, while the registration waits
//! for its address to be confirmed: a secret that is no session and reaches nothing.
//! It shows the browser to be the registrant's, where the link of the mail confirms the
//! registration at once (`completeEmailVerification`), and lets a second registration
//! from the browser replace the first.

use axum::http::header::SET_COOKIE;
use axum::http::{HeaderMap, HeaderValue};
use axum::response::Response;

use super::oidc::browser_cookie;
use crate::app::{App, SignupMarker};
use crate::auth::token::Https;
use crate::http_credentials::cookie_value;

/// The name of the cookie where the server is reached over HTTPS: the `__Host-` prefix
/// holds it to exactly this host, as the session cookie's.
const SECURE_NAME: &str = "__Host-fvoci_signup";
/// The name of the cookie where the server is reached over plain HTTP, as in
/// development.
const PLAIN_NAME: &str = "fvoci_signup";

fn name(app: &App) -> &'static str {
    if app.authenticator().origins().https() == Https::Yes {
        SECURE_NAME
    } else {
        PLAIN_NAME
    }
}

/// The secret a request's browser keeps of a registration made from it, if it has one.
pub(super) fn marker_of(app: &App, headers: &HeaderMap) -> Option<String> {
    cookie_value(headers, name(app)).filter(|secret| !secret.is_empty())
}

/// Gives the browser the marker of the registration it just made, for as long as the
/// registration waits: not readable by scripts, sent along with requests from the same
/// site only, `Secure` over HTTPS.
pub(super) fn with_marker(app: &App, marker: &SignupMarker, response: Response) -> Response {
    let cookie = browser_cookie(
        app,
        name(app),
        marker.secret(),
        SignupMarker::MAX_AGE_SECONDS,
    );
    appended(response, cookie)
}

/// Makes the browser forget the marker of a registration that is confirmed.
pub(super) fn without_marker(app: &App, response: Response) -> Response {
    appended(response, browser_cookie(app, name(app), "", 0))
}

/// Beside whatever cookie the response sets already.
fn appended(mut response: Response, cookie: HeaderValue) -> Response {
    response.headers_mut().append(SET_COOKIE, cookie);
    response
}
