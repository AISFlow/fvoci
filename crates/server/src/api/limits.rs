//! Limits on the size of request bodies.
//!
//! A body over the limit is refused with 413, the limit and the reason
//! `request_too_large`, before any handler runs and without being kept in memory.

use std::time::Duration;

use axum::body::{Body, BodyDataStream, HttpBody as _};
use axum::extract::{Request, State};
use axum::http::header::{CONTENT_LENGTH, EXPECT};
use axum::http::{Extensions, HeaderMap};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use bytes::BytesMut;
use futures_util::StreamExt as _;

use super::session::{claims_session, session_of};
use crate::app::App;
use crate::auth::api_token;
use crate::client_address::ClientAddress;
use crate::error::{AppError, Reason};
use crate::http_credentials::bearer_token;

/// The largest body of a request to a route that needs no session: the login and the
/// registration. Anyone can send these, so they get no more room than they need; the
/// largest measured was 115 bytes.
pub(crate) const UNAUTHENTICATED_BODY_LIMIT: BodyLimit = BodyLimit(64 * 1024);

/// The largest text message a websocket connection may send: the commands are a few
/// hundred bytes of JSON.
pub(crate) const WEBSOCKET_MESSAGE_LIMIT: usize = 64 * 1024;

/// How much of a refused body is read and thrown away before the refusal is sent.
///
/// A client that is still sending its body when the answer comes, and finds the
/// connection closed under it, may never read the answer: in the tests one request in
/// nine over the JSON limit ended in a broken pipe instead of a 413. So the rest of the
/// body is taken off the wire first, without keeping it, up to this much and for at
/// most [`DISCARD_TIMEOUT`]. What is longer or slower is cut off.
const DISCARD_LIMIT: u64 = 16 * 1024 * 1024;

/// How long a refused body is read for before the refusal is sent regardless.
const DISCARD_TIMEOUT: Duration = Duration::from_secs(5);

/// The largest request body, in bytes, that the routes behind [`limit_request_body`]
/// read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BodyLimit(pub(crate) u64);

/// The limit of the routes that take a session, with what it takes to tell whether a
/// request has one.
#[derive(Clone)]
pub(crate) struct SessionBodyLimit {
    pub(crate) app: App,
    pub(crate) limit: BodyLimit,
}

/// [`limit_request_body`] for the routes that take a session: the room beyond that of
/// the routes without one is for requests of a session only.
///
/// A request whose body is, or may be, larger than the small limit is asked for its
/// session before the body is read, or for its personal API token: a token that
/// writes has the room of a session, one that reads only the small limit. A token that
/// is no session's (or no token's) is answered as the route would answer it (401), with
/// nothing of the body kept: a made-up token does not buy the larger allowance.
/// Smaller requests are not looked up here; the route does that.
pub(crate) async fn limit_session_request_body(
    State(SessionBodyLimit { app, limit }): State<SessionBodyLimit>,
    request: Request,
    next: Next,
) -> Response {
    let small = UNAUTHENTICATED_BODY_LIMIT.0;
    let largest = declared_length(&request).or_else(|| request.body().size_hint().upper());
    let claims = claims_session(&app, request.headers());
    let wants_the_room = claims && largest.is_none_or(|length| length > small);
    let room = if wants_the_room {
        match room_of(&app, request.headers(), request.extensions()).await {
            Ok(room) => room,
            Err(error) => {
                let (parts, body) = request.into_parts();
                if !waits_to_send(&parts.headers) {
                    discard(body.into_data_stream(), 0).await;
                }
                return error.into_response();
            }
        }
    } else {
        Room::Granted
    };
    let limit = if claims && room == Room::Granted {
        limit
    } else {
        // A request that claims no session gets the room of the routes that need none:
        // the body is read before anyone looks at who sends it, and "anyone" should not
        // be able to make the server hold more than that.
        BodyLimit(limit.0.min(UNAUTHENTICATED_BODY_LIMIT.0))
    };
    limit_request_body(State(limit), request, next).await
}

/// Whether whoever sends a request has the room of the routes that take a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Room {
    /// A session, or a personal API token that writes.
    Granted,
    /// A token that reads only: no more than a request without a session.
    Small,
}

/// The room of a request that claims a session: of its personal API token if it sends
/// one in its `Authorization` header, else of its session.
///
/// # Errors
///
/// Returns what the route would answer for a credential that stands for nobody.
async fn room_of(
    app: &App,
    headers: &HeaderMap,
    extensions: &Extensions,
) -> Result<Room, AppError> {
    if let Some(token) = bearer_token(headers)
        && api_token::is_api_token(token)
    {
        let address = extensions
            .get::<ClientAddress>()
            .and_then(|ClientAddress(address)| *address);
        let writes = app.token_writes(token, address).await?;
        return Ok(if writes { Room::Granted } else { Room::Small });
    }
    session_of(app, headers, extensions).await?;
    Ok(Room::Granted)
}

/// The length a request declares for its body, if it does.
fn declared_length(request: &Request) -> Option<u64> {
    request
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
}

/// Reads the request body up to the limit and hands the request on with it, or answers
/// 413 with the limit.
///
/// A body that declares more than the limit is refused without any of it being kept.
/// One that declares nothing, or less than it sends, is refused as soon as it passes the
/// limit.
pub(crate) async fn limit_request_body(
    State(BodyLimit(limit)): State<BodyLimit>,
    request: Request,
    next: Next,
) -> Response {
    let declared = declared_length(&request);
    let (parts, body) = request.into_parts();
    let mut stream = body.into_data_stream();
    if declared.is_some_and(|length| length > limit) {
        if !waits_to_send(&parts.headers) {
            discard(stream, 0).await;
        }
        return too_large(limit).into_response();
    }

    let capacity = usize::try_from(declared.unwrap_or(0).min(limit)).unwrap_or(0);
    let mut read = BytesMut::with_capacity(capacity);
    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(error) => {
                tracing::debug!(%error, "a request body could not be read");
                return AppError::bad_request("the request body could not be read").into_response();
            }
        };
        if byte_count(read.len()).saturating_add(byte_count(chunk.len())) > limit {
            drop(read);
            discard(stream, byte_count(chunk.len())).await;
            return too_large(limit).into_response();
        }
        read.extend_from_slice(&chunk);
    }
    next.run(Request::from_parts(parts, Body::from(read.freeze())))
        .await
}

/// The refusal of a body of more than `limit` bytes.
fn too_large(limit: u64) -> AppError {
    AppError::payload_too_large(limit).with_reason(Reason::RequestTooLarge)
}

/// A length in memory as a number of bytes of a body. A `usize` fits everywhere this
/// server runs; where it would not, the length counts as more than any limit.
fn byte_count(length: usize) -> u64 {
    u64::try_from(length).unwrap_or(u64::MAX)
}

/// Whether the client waits for `100 Continue` before it sends the body (RFC 9110
/// §10.1.1): it has sent nothing yet, and reading the body would invite it to, so a
/// refusal is answered at once, without [`discard`].
pub(crate) fn waits_to_send(headers: &HeaderMap) -> bool {
    headers
        .get(EXPECT)
        .is_some_and(|value| value.as_bytes().eq_ignore_ascii_case(b"100-continue"))
}

/// What is left of a refused request, read piece by piece to be thrown away.
pub(crate) trait Leftover {
    /// Reads the next piece and says how many bytes it held, reading not much more than
    /// `allowance` of it; `None` at the end of the request, or where it cannot be read.
    async fn next_piece(&mut self, allowance: u64) -> Option<u64>;
}

impl Leftover for BodyDataStream {
    async fn next_piece(&mut self, _allowance: u64) -> Option<u64> {
        match self.next().await {
            Some(Ok(chunk)) => Some(byte_count(chunk.len())),
            Some(Err(_)) | None => None,
        }
    }
}

/// Reads what is left of a refused request and throws it away, so that the client can
/// finish sending and read the refusal: up to [`DISCARD_LIMIT`] past the `discarded`
/// bytes thrown away already, and for at most [`DISCARD_TIMEOUT`]. What was read and
/// kept before the request passed its limit does not count, so up to the limit plus
/// [`DISCARD_LIMIT`] is read of one request.
pub(crate) async fn discard(mut leftover: impl Leftover, mut discarded: u64) {
    let reading = async {
        while discarded <= DISCARD_LIMIT {
            match leftover.next_piece(DISCARD_LIMIT - discarded).await {
                Some(length) => discarded = discarded.saturating_add(length),
                None => break,
            }
        }
    };
    // Whether the request ended, was too long or too slow makes no difference to the
    // answer.
    let _ = tokio::time::timeout(DISCARD_TIMEOUT, reading).await;
}

#[cfg(test)]
mod tests {
    use axum::Router;
    use axum::body::Bytes;
    use axum::http::StatusCode;
    use axum::middleware;
    use axum::routing::post;
    use tower::ServiceExt as _;

    use super::*;

    /// A route that answers with the length of the body it was given.
    fn limited(limit: u64) -> Router {
        Router::new()
            .route(
                "/",
                post(|body: Bytes| async move { body.len().to_string() }),
            )
            .layer(middleware::from_fn_with_state(
                BodyLimit(limit),
                limit_request_body,
            ))
    }

    async fn answer(router: Router, request: Request) -> (StatusCode, String) {
        let response = router.oneshot(request).await.expect("the router answers");
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("the body is read");
        (status, String::from_utf8_lossy(&body).into_owned())
    }

    #[tokio::test]
    async fn a_body_up_to_the_limit_reaches_the_handler_whole() {
        for length in [0, 1, 15, 16] {
            let request = Request::post("/")
                .body(Body::from(vec![b'x'; length]))
                .expect("a valid request");
            let (status, body) = answer(limited(16), request).await;
            assert_eq!(status, StatusCode::OK, "{length} bytes: {body}");
            assert_eq!(body, length.to_string(), "the handler got every byte");
        }
    }

    #[tokio::test]
    async fn a_body_that_declares_more_than_the_limit_is_refused_without_being_kept() {
        let request = Request::post("/")
            .header(CONTENT_LENGTH, "17")
            .body(Body::from(vec![b'x'; 17]))
            .expect("a valid request");
        let (status, body) = answer(limited(16), request).await;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{body}");
        let error: fvoci_model::Problem = serde_json::from_str(&body).expect("an error body");
        assert_eq!(error.problem_type, "/problems/request-too-large");
        assert_eq!(error.reason.as_deref(), Some("request_too_large"));
        assert_eq!(error.limit, Some(16), "the answer names the limit");
    }

    /// A client that asked first, with `Expect: 100-continue`, is answered without its
    /// body being read: reading would tell it to send what is then thrown away.
    #[tokio::test]
    async fn a_client_that_waits_for_continue_is_refused_without_being_read() {
        let polled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let seen = std::sync::Arc::clone(&polled);
        let chunks = futures_util::stream::poll_fn(move |_| {
            seen.store(true, std::sync::atomic::Ordering::SeqCst);
            std::task::Poll::Ready(None::<Result<Bytes, std::io::Error>>)
        });
        let request = Request::post("/")
            .header(CONTENT_LENGTH, "1000")
            .header(EXPECT, "100-continue")
            .header("Authorization", "Bearer a-token")
            .body(Body::from_stream(chunks))
            .expect("a valid request");
        let (status, body) = answer(limited(16), request).await;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{body}");
        assert!(
            !polled.load(std::sync::atomic::Ordering::SeqCst),
            "the body was not asked for"
        );
    }

    #[tokio::test]
    async fn a_body_longer_than_what_is_thrown_away_is_refused_all_the_same() {
        // Twice the amount that is discarded, in pieces, without a declared length.
        let pieces = usize::try_from(2 * DISCARD_LIMIT / 65_536).expect("a small number");
        let chunks = futures_util::stream::iter(
            (0..pieces).map(|_| Ok::<_, std::io::Error>(Bytes::from(vec![b'x'; 65_536]))),
        );
        let request = Request::post("/")
            .body(Body::from_stream(chunks))
            .expect("a valid request");
        let (status, body) = answer(limited(16), request).await;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{body}");
    }

    #[tokio::test]
    async fn a_body_that_declares_no_length_is_refused_once_it_passes_the_limit() {
        // A stream has no length to declare.
        let chunks = futures_util::stream::iter(
            (0..4).map(|_| Ok::<_, std::io::Error>(Bytes::from(vec![b'x'; 8]))),
        );
        let request = Request::post("/")
            .body(Body::from_stream(chunks))
            .expect("a valid request");
        let (status, body) = answer(limited(16), request).await;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{body}");
        let error: fvoci_model::Problem = serde_json::from_str(&body).expect("an error body");
        assert_eq!(error.limit, Some(16), "the answer names the limit");
    }
}
