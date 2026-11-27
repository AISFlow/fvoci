//! The request log and the containment of handler panics.

use std::net::SocketAddr;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Instant;

use axum::body::Body;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::header::{CONTENT_ENCODING, CONTENT_LENGTH};
use axum::http::{HeaderMap, HeaderValue};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use futures_util::FutureExt as _;
use uuid::Uuid;

use crate::client_address::{self, ClientAddress};
use crate::config::TrustedProxies;
use crate::connection::Connection;
use crate::error::{AppError, PROBLEMS_PATH};
use crate::error_response::ReportedError;

/// The response header that carries the identifier of the request.
const X_REQUEST_ID: &str = "x-request-id";

/// Where a connection comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Peer {
    /// A TCP connection from this address.
    Tcp(SocketAddr),
    /// A connection through the administration socket.
    Unix,
}

impl Peer {
    /// Where the connection of a request comes from, if the server said
    /// ([`Connection`]).
    pub(crate) fn of(request: &Request) -> Option<Self> {
        request
            .extensions()
            .get::<ConnectInfo<Connection>>()
            .map(|ConnectInfo(connection)| connection.peer)
    }
}

/// What the request log knows of how the server is served.
#[derive(Debug, Default)]
pub(crate) struct Served {
    /// The reverse proxies whose word about the client's address is taken.
    pub(crate) proxies: TrustedProxies,
    /// The path everything is served under; empty at the root of the host.
    pub(crate) base_path: String,
}

/// Returns the client of a request: its address, also behind the reverse proxies the
/// configuration trusts, and how the log shows it.
fn client_of(
    proxies: &TrustedProxies,
    headers: &HeaderMap,
    peer: Option<Peer>,
) -> (ClientAddress, String) {
    match peer {
        Some(Peer::Tcp(peer)) => {
            let address = client_address::client_of(proxies, peer.ip(), headers);
            (ClientAddress(Some(address)), address.to_string())
        }
        Some(Peer::Unix) => (ClientAddress(None), "unix socket".to_owned()),
        None => (ClientAddress(None), "unknown".to_owned()),
    }
}

/// Logs every request: method, path, status, duration, client address and the
/// identifier it gave the request.
///
/// The identifier is a random one of this server's making. It goes into the log line,
/// into the `x-request-id` response header and into the body of an error answer (its
/// `requestId`, and its `instance` as a URN), so that what a user reports can be found
/// in the log. The `type` of an error answer gets the path the server is served under
/// before it, so that it is the address of the page about it. An identifier the client sends is not
/// taken over: it would let a client write into the log.
///
/// The query string is left out because it can carry the read token of a shared board,
/// and no header is logged, so neither are the session token and the cookies.
pub(crate) async fn log_request(
    State(served): State<Arc<Served>>,
    mut request: Request,
    next: Next,
) -> Response {
    let started = Instant::now();
    let request_id = Uuid::now_v7().to_string();
    let method = request.method().clone();
    let path = logged_path(request.uri().path());
    let peer = Peer::of(&request);
    let (address, client) = client_of(&served.proxies, request.headers(), peer);
    // For the handlers that count by where a request comes from: the same address the
    // log shows.
    request.extensions_mut().insert(address);

    let response = identify(next.run(request).await, &request_id, &served.base_path);

    // Whole microseconds, shown as milliseconds.
    let duration_ms = (started.elapsed().as_secs_f64() * 1_000_000.0).round() / 1000.0;
    tracing::info!(
        %method,
        path,
        status = response.status().as_u16(),
        duration_ms,
        client,
        request_id,
        "request"
    );
    response
}

/// Puts the identifier of the request on its response: into the header, and into the
/// body if the response is an error of this server's own format, whose `type` is made
/// the address of its page under `base_path`.
fn identify(mut response: Response, request_id: &str, base_path: &str) -> Response {
    if let Some(ReportedError(mut body)) = response.extensions_mut().remove::<ReportedError>() {
        if body.problem_type.starts_with(PROBLEMS_PATH) {
            body.problem_type.insert_str(0, base_path);
        }
        body.instance = Some(format!("urn:uuid:{request_id}"));
        body.request_id = Some(request_id.to_owned());
        // An error body is a handful of strings; serializing it cannot fail.
        if let Ok(bytes) = serde_json::to_vec(&body) {
            let (mut parts, _) = response.into_parts();
            parts.headers.remove(CONTENT_LENGTH);
            // The body is written anew, as it is: an encoding of the one before is gone.
            parts.headers.remove(CONTENT_ENCODING);
            response = Response::from_parts(parts, Body::from(bytes));
        }
        // Kept, as sent, for the measurement of the requests, which counts problems.
        response.extensions_mut().insert(ReportedError(body));
    }
    if let Ok(value) = HeaderValue::from_str(request_id) {
        response.headers_mut().insert(X_REQUEST_ID, value);
    }
    response
}

/// The part of an address that names the page of an invitation, under whatever path
/// the server is served at. What follows it is the token of the invitation.
const INVITATION_PAGE: &[u8] = b"invite";

/// The path of a request as the log has it: the address of the page of an invitation
/// carries the invitation's token, a credential, which the log leaves out. No other
/// address of the server has that part.
///
/// The page is told as the app tells it, by the part of the path as it reads once its
/// escapes are undone (`/%69nvite/…` is the page as `/invite/…` is), in any letter
/// case: an address that is taken for the page once too often loses what follows it
/// in the log, and no more.
fn logged_path(path: &str) -> String {
    let mut start = 0;
    for part in path.split('/') {
        let end = start + part.len();
        if end < path.len() && unescaped(part).eq_ignore_ascii_case(INVITATION_PAGE) {
            let page = path.get(..end).unwrap_or(path);
            return format!("{page}/<token>");
        }
        start = end + 1;
    }
    path.to_owned()
}

/// A part of a path with its percent escapes undone; what is no escape stays as it is.
fn unescaped(part: &str) -> Vec<u8> {
    let bytes = part.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while let Some(&byte) = bytes.get(at) {
        let escaped = (byte == b'%')
            .then(|| part.get(at + 1..at + 3))
            .flatten()
            .and_then(|digits| u8::from_str_radix(digits, 16).ok());
        if let Some(escaped) = escaped {
            out.push(escaped);
            at += 3;
        } else {
            out.push(byte);
            at += 1;
        }
    }
    out
}

/// Answers with the standard 500 when a handler panics, instead of dropping the
/// connection.
pub(crate) async fn contain_panics(request: Request, next: Next) -> Response {
    let path = logged_path(request.uri().path());
    match AssertUnwindSafe(next.run(request)).catch_unwind().await {
        Ok(response) => response,
        Err(payload) => {
            let message = payload
                .downcast_ref::<&str>()
                .copied()
                .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
                .unwrap_or("a panic without a message");
            tracing::error!(panic = message, path, "http handler panic");
            AppError::internal("http handler panic").into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::sync::{Arc, Mutex};

    use axum::Router;
    use axum::body::Body;
    use axum::http::StatusCode;
    use axum::middleware;
    use axum::routing::get;
    use tower::ServiceExt as _;

    use super::*;

    const X_FORWARDED_FOR: &str = "x-forwarded-for";

    /// The peer the connections of these tests come from.
    fn socket_peer() -> Peer {
        Peer::Tcp("192.0.2.1:5000".parse().expect("a socket address"))
    }

    async fn boom() -> &'static str {
        panic!("boom");
    }

    fn forwarded(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            X_FORWARDED_FOR,
            value.parse().expect("a valid header value"),
        );
        headers
    }

    fn proxies(ranges: &[&str]) -> TrustedProxies {
        let ranges: Vec<String> = ranges.iter().map(|range| (*range).to_owned()).collect();
        TrustedProxies::parse(&ranges).expect("valid ranges")
    }

    #[test]
    fn the_token_of_an_invitation_is_not_in_a_logged_path() {
        for (path, logged) in [
            ("/invite/secret-token", "/invite/<token>"),
            ("/base/invite/secret-token", "/base/invite/<token>"),
            ("/invite/secret/more", "/invite/<token>"),
            ("/invite/", "/invite/<token>"),
            ("/invite", "/invite"),
            // As the app reads the address: with its escapes undone.
            ("/%69nvite/secret-token", "/%69nvite/<token>"),
            ("/base/inv%69te/secret-token", "/base/inv%69te/<token>"),
            ("/INVITE/secret-token", "/INVITE/<token>"),
            ("/invite//secret-token", "/invite/<token>"),
            ("/%invite/not-a-token", "/%invite/not-a-token"),
            ("/inv%2Fite/not-a-token", "/inv%2Fite/not-a-token"),
            ("/invited/people", "/invited/people"),
            ("/api/v1/invitations/accept", "/api/v1/invitations/accept"),
            ("/w/1/boards/2", "/w/1/boards/2"),
        ] {
            assert_eq!(logged_path(path), logged, "{path}");
        }
    }

    #[test]
    fn client_is_the_socket_peer_unless_the_peer_is_a_trusted_proxy() {
        let headers = forwarded("203.0.113.7");
        let nobody = proxies(&[]);
        let peer_address = "192.0.2.1".parse().expect("an address");
        assert_eq!(
            client_of(&nobody, &headers, Some(socket_peer())),
            (ClientAddress(Some(peer_address)), "192.0.2.1".to_owned()),
            "the header is not believed unless it comes from a trusted proxy"
        );
        assert_eq!(
            client_of(&nobody, &headers, Some(Peer::Unix)),
            (ClientAddress(None), "unix socket".to_owned())
        );
        assert_eq!(
            client_of(&nobody, &headers, None),
            (ClientAddress(None), "unknown".to_owned())
        );

        let trusting = proxies(&["192.0.2.0/24"]);
        let (address, shown) = client_of(
            &trusting,
            &forwarded("1.2.3.4, 2001:db8::7"),
            Some(socket_peer()),
        );
        assert_eq!(
            shown, "2001:db8::7",
            "what the proxy added, not the left end"
        );
        assert_eq!(
            address,
            ClientAddress(Some("2001:db8::7".parse().expect("an address")))
        );
        for junk in ["", "unknown", "203.0.113.7:port", "evil\tclient=10.0.0.1"] {
            assert_eq!(
                client_of(&trusting, &forwarded(junk), Some(socket_peer())).1,
                "192.0.2.1",
                "{junk:?} is not an address and is not copied into the log"
            );
        }
        // An address with a port is the address: what is logged is what was read, not
        // the text that was sent.
        assert_eq!(
            client_of(
                &trusting,
                &forwarded("203.0.113.7:1234"),
                Some(socket_peer())
            )
            .1,
            "203.0.113.7"
        );
    }

    /// A log output that tests can read back.
    #[derive(Clone, Default)]
    struct Captured(Arc<Mutex<Vec<u8>>>);

    impl io::Write for Captured {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0
                .lock()
                .expect("lock poisoned")
                .extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Captured {
        fn text(&self) -> String {
            String::from_utf8_lossy(&self.0.lock().expect("lock poisoned")).into_owned()
        }
    }

    /// Sends one request through both layers and returns the status, the body and
    /// what was logged.
    async fn logged(proxies: TrustedProxies, request: Request) -> (StatusCode, String, String) {
        let captured = Captured::default();
        let writer = captured.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);

        let router = Router::new()
            .route("/api/v1/boards/{board_id}", get(|| async { "fine" }))
            .route("/api/v1/boom", get(boom))
            .layer(middleware::from_fn(contain_panics))
            .layer(middleware::from_fn_with_state(
                Arc::new(Served {
                    proxies,
                    base_path: "/base".to_owned(),
                }),
                log_request,
            ));
        let response = router
            .oneshot(request)
            .await
            .expect("the router does not fail");
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("the body is read");
        (
            status,
            String::from_utf8_lossy(&body).into_owned(),
            captured.text(),
        )
    }

    #[tokio::test]
    async fn request_log_has_no_query_string_and_no_credentials() {
        let mut request =
            Request::get("/api/v1/boards/b1?readToken=SECRET-READ&other=SECRET-OTHER")
                .header("Authorization", "Bearer SECRET-BEARER")
                .header("Cookie", "fvoci_session=SECRET-COOKIE")
                .header(X_FORWARDED_FOR, "203.0.113.7")
                .body(Body::empty())
                .expect("a valid request");
        request.extensions_mut().insert(ConnectInfo(Connection {
            peer: socket_peer(),
            breaker: None,
        }));

        let (status, body, log) = logged(proxies(&[]), request).await;

        assert_eq!(status, StatusCode::OK, "{body}");
        for expected in [
            "request",
            "method=GET",
            "path=\"/api/v1/boards/b1\"",
            "status=200",
            "duration_ms=",
            "client=\"192.0.2.1\"",
        ] {
            assert!(log.contains(expected), "{expected:?} is logged: {log}");
        }
        for secret in ["SECRET", "readToken", "203.0.113.7"] {
            assert!(!log.contains(secret), "{secret:?} is not logged: {log}");
        }
    }

    #[tokio::test]
    async fn request_log_names_the_forwarded_client_behind_trusted_proxies() {
        let mut request = Request::get("/api/v1/boards/b1")
            .header(X_FORWARDED_FOR, "203.0.113.7, 10.0.0.1")
            .body(Body::empty())
            .expect("a valid request");
        request.extensions_mut().insert(ConnectInfo(Connection {
            peer: socket_peer(),
            breaker: None,
        }));

        let (_, _, log) = logged(proxies(&["192.0.2.1", "10.0.0.0/8"]), request).await;

        assert!(log.contains("client=\"203.0.113.7\""), "{log}");
    }

    /// A handler panic becomes the standard 500 response, and the request is still logged.
    #[tokio::test]
    async fn handler_panic_becomes_the_internal_server_error_response() {
        let request = Request::get("/api/v1/boom?readToken=SECRET-READ")
            .body(Body::empty())
            .expect("a valid request");

        let (status, body, log) = logged(proxies(&[]), request).await;

        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
        let error: fvoci_model::Problem =
            serde_json::from_str(&body).expect("the standard error body");
        assert_eq!(
            error.problem_type, "/base/problems/internal",
            "the type is under the path the server is served at: {body}"
        );
        assert_eq!(error.status, 500, "{body}");
        assert_eq!(error.detail, "internal server error", "{body}");
        let request_id = error.request_id.expect("the identifier of the request");
        assert_eq!(request_id.len(), 36, "a random identifier: {request_id}");
        assert_eq!(
            error.instance,
            Some(format!("urn:uuid:{request_id}")),
            "the instance is the request: {body}"
        );
        assert!(
            log.contains(&format!("request_id=\"{request_id}\"")),
            "the log line of the request carries the identifier of the answer: {log}"
        );
        assert!(log.contains("http handler panic"), "{log}");
        assert!(log.contains("panic=\"boom\""), "{log}");
        assert!(log.contains("status=500"), "{log}");
        assert!(!log.contains("SECRET"), "the query is not logged: {log}");
    }
}
