//! The metrics listener: the metrics served at [`PATH`] on a listener of their own,
//! never on the API's.
//!
//! A scrape must send the bearer token of `metrics_token` if it is set, and needs
//! nothing if it is empty; the listener is meant for an address only the monitoring
//! can reach either way (`docs/configuration.md`, "키: 지표").

use anyhow::Context as _;
use axum::Router;
use axum::extract::{Request, State};
use axum::http::header::{AUTHORIZATION, CONTENT_TYPE, WWW_AUTHENTICATE};
use axum::http::{HeaderMap, HeaderValue};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;

use crate::app::App;
use crate::config::{MetricsAddress, MetricsHost, MetricsToken};
use crate::error::AppError;
use crate::metrics;

/// The path the metrics are served at, on the metrics listener.
const PATH: &str = "/metrics";

/// The router of the metrics listener: [`PATH`] and nothing else of the server. No
/// route of the API, the websocket or the web app is on it. What it refuses it refuses
/// as the API does, with a problem (RFC 9457), and so it serves the pages such an
/// answer points at; without the identifier of a request, as nothing is logged here.
pub(crate) fn router(app: App) -> Router {
    let token = app.config().metrics_scrape_token();
    Router::new()
        .route(PATH, get(answer_scrape))
        .route_layer(middleware::from_fn_with_state(token, require_token))
        .fallback(|| async { AppError::not_found("route") })
        .method_not_allowed_fallback(|| async { AppError::method_not_allowed() })
        .with_state(app)
        .merge(crate::problems::routes())
}

/// Lets a scrape through if it sends `token` as its bearer token, or if there is none
/// to send; else answers 401, before anything is read for it. The token is compared in
/// constant time and written nowhere.
async fn require_token(
    State(token): State<Option<MetricsToken>>,
    request: Request,
    next: Next,
) -> Response {
    let Some(token) = &token else {
        return next.run(request).await;
    };
    let challenge = match credentials(request.headers()) {
        Credentials::Bearer(presented) if token.admits(presented) => {
            return next.run(request).await;
        }
        // A token was sent, and is not this one (RFC 6750, 3.1).
        Credentials::Bearer(_) => "Bearer realm=\"metrics\", error=\"invalid_token\"",
        // The header repeated: the request is malformed (RFC 6750, 3.1). 401 rather than
        // the 400 it suggests, so that every refusal of this listener is one status.
        Credentials::Several => "Bearer realm=\"metrics\", error=\"invalid_request\"",
        Credentials::None => "Bearer realm=\"metrics\"",
    };
    let mut refused = AppError::unauthorized("a scrape needs the bearer token").into_response();
    refused
        .headers_mut()
        .insert(WWW_AUTHENTICATE, HeaderValue::from_static(challenge));
    refused
}

/// What the `Authorization` headers of a scrape send. No `Debug` but in tests: it holds
/// the token presented, which may be the right one.
#[derive(Clone, Copy)]
#[cfg_attr(test, derive(Debug, PartialEq, Eq))]
enum Credentials<'h> {
    /// No bearer token: no header, or one of another scheme, with no space after the
    /// scheme, or that is no text.
    None,
    /// One header with a bearer token (RFC 6750, 2.1): after the scheme, which is read
    /// without regard to case, and the spaces.
    Bearer(&'h str),
    /// More than one header. None of them is taken: which one would be is the order
    /// of the headers, which a proxy may change.
    Several,
}

/// What the `Authorization` headers of `headers` send.
fn credentials(headers: &HeaderMap) -> Credentials<'_> {
    let mut values = headers.get_all(AUTHORIZATION).iter();
    let Some(value) = values.next() else {
        return Credentials::None;
    };
    if values.next().is_some() {
        return Credentials::Several;
    }
    let bearer = value.to_str().ok().and_then(|value| {
        let (scheme, token) = value.split_once(' ')?;
        scheme
            .eq_ignore_ascii_case("bearer")
            .then(|| token.trim_start_matches(' '))
    });
    bearer.map_or(Credentials::None, Credentials::Bearer)
}

/// Answers a scrape: reads what is measured by asking, then writes all.
async fn answer_scrape(axum::extract::State(app): axum::extract::State<App>) -> Response {
    // Reads of the database side by side make a large future: on the heap.
    Box::pin(async { tokio::join!(app.readiness(), app.read_storage_metrics()) }).await;
    match app.metrics().render() {
        Some(Ok(text)) => (
            [(CONTENT_TYPE, HeaderValue::from_static(metrics::TEXT_FORMAT))],
            text,
        )
            .into_response(),
        // The cause is logged where the error becomes its answer, and not sent.
        Some(Err(error)) => {
            AppError::from(error.context("unable to answer a scrape")).into_response()
        }
        None => AppError::not_found("route").into_response(),
    }
}

/// Opens the metrics listener at `address`. On a server that listens on the loopback
/// only, a listener that ends up on every interface (a name that resolves to
/// `0.0.0.0`) is refused.
///
/// # Errors
///
/// Returns an error if the host cannot be resolved, or the address cannot be listened
/// on: taken, or not this machine's; or if it is every interface and `reach` is the
/// loopback only.
pub(crate) async fn bind(
    address: &MetricsAddress,
    reach: Reach,
) -> anyhow::Result<tokio::net::TcpListener> {
    let listener = match &address.host {
        MetricsHost::Ip(ip) => tokio::net::TcpListener::bind((*ip, address.port)).await,
        MetricsHost::Name(name) => {
            tokio::net::TcpListener::bind((name.as_str(), address.port)).await
        }
    }
    .with_context(|| format!("listening for metrics scrapes on {address} (metrics_address)"))?;
    let bound = listener
        .local_addr()
        .context("the metrics listener's address")?;
    if reach == Reach::LoopbackOnly && bound.ip().to_canonical().is_unspecified() {
        anyhow::bail!(
            "metrics_address {address} is every interface, and listen_loopback_only is set: \
             give an address that is not every interface, such as 127.0.0.1:9092"
        );
    }
    Ok(listener)
}

/// Whether the server listens on the loopback only (`listen_loopback_only`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reach {
    /// On the loopback only: `listen_loopback_only` is on.
    LoopbackOnly,
    /// Wherever its addresses say.
    Any,
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::sync::{Arc, Mutex};

    use axum::body::Body;
    use axum::http::StatusCode;
    use tower::ServiceExt as _;

    use super::*;

    const TOKEN: &str = "the-token-of-the-scrapes";

    /// What a subscriber of the log writes.
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

    #[tokio::test]
    async fn a_listener_on_every_interface_is_refused_on_a_local_only_server() {
        let every = MetricsAddress {
            host: MetricsHost::Ip(std::net::Ipv4Addr::UNSPECIFIED.into()),
            port: 0,
        };
        let error = bind(&every, Reach::LoopbackOnly)
            .await
            .expect_err("every interface");
        let error = format!("{error:#}");
        assert!(error.contains("listen_loopback_only"), "{error}");
        let listener = bind(&every, Reach::Any).await.expect("every interface");
        assert!(listener.local_addr().expect("bound").ip().is_unspecified());
        let loopback = MetricsAddress {
            host: MetricsHost::Name("localhost".to_owned()),
            port: 0,
        };
        let listener = bind(&loopback, Reach::LoopbackOnly)
            .await
            .expect("the loopback");
        assert!(listener.local_addr().expect("bound").ip().is_loopback());
    }

    /// The status of a scrape that sends an `Authorization` header with each value of
    /// `authorization`, and the log of it at every level.
    async fn scraped(token: Option<&str>, authorization: &[&str]) -> (StatusCode, String) {
        let captured = Captured::default();
        let writer = captured.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_max_level(tracing::Level::TRACE)
            .with_writer(move || writer.clone())
            .finish();
        let _logging = tracing::subscriber::set_default(subscriber);
        let config = crate::config::Configuration {
            metrics_token: token.unwrap_or_default().to_owned(),
            ..crate::config::Configuration::default()
        };
        let router = Router::new()
            .route(PATH, get(|| async { "the metrics" }))
            .route_layer(middleware::from_fn_with_state(
                config.metrics_scrape_token(),
                require_token,
            ));
        let mut request = Request::get(PATH);
        for value in authorization {
            request = request.header(AUTHORIZATION, *value);
        }
        let response = router
            .oneshot(request.body(Body::empty()).expect("a request"))
            .await
            .expect("an answer");
        let status = response.status();
        if status == StatusCode::UNAUTHORIZED {
            let challenge = response
                .headers()
                .get(WWW_AUTHENTICATE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default();
            assert!(
                challenge.starts_with("Bearer realm=\"metrics\""),
                "{challenge}"
            );
        }
        let log = String::from_utf8_lossy(&captured.0.lock().expect("lock poisoned")).into_owned();
        (status, log)
    }

    #[tokio::test]
    async fn with_a_token_only_a_scrape_that_sends_it_is_answered() {
        let right = format!("Bearer {TOKEN}");
        for (authorization, status) in [
            (None, StatusCode::UNAUTHORIZED),
            (Some("Bearer"), StatusCode::UNAUTHORIZED),
            (Some("Bearer "), StatusCode::UNAUTHORIZED),
            (Some("Bearer the-token"), StatusCode::UNAUTHORIZED),
            (
                Some(&format!("Basic {TOKEN}")[..]),
                StatusCode::UNAUTHORIZED,
            ),
            (Some(TOKEN), StatusCode::UNAUTHORIZED),
            (Some(&right[..]), StatusCode::OK),
            (Some(&format!("bearer  {TOKEN}")[..]), StatusCode::OK),
        ] {
            let (got, log) = scraped(Some(TOKEN), authorization.as_slice()).await;
            assert_eq!(got, status, "{authorization:?}");
            assert!(!log.contains(TOKEN), "the token is not logged: {log}");
            assert!(!log.contains("the-token"), "nor a guess: {log}");
        }
    }

    /// The headers with each of `values`.
    fn headers(values: &[&[u8]]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for value in values {
            headers.append(
                AUTHORIZATION,
                HeaderValue::from_bytes(value).expect("a value"),
            );
        }
        headers
    }

    #[test]
    fn more_than_one_authorization_header_sends_no_token() {
        let right = format!("Bearer {TOKEN}");
        let right = right.as_bytes();
        for values in [
            [right, b"Bearer wrong"],
            [b"Bearer wrong", right],
            [right, right],
            [b"", right],
        ] {
            assert_eq!(
                credentials(&headers(&values)),
                Credentials::Several,
                "{values:?}"
            );
        }
    }

    #[test]
    fn one_authorization_header_sends_its_bearer_token_if_it_has_one() {
        let right = format!("Bearer {TOKEN}");
        let joined = format!("Bearer {TOKEN}, Bearer wrong");
        for (value, sent) in [
            (&b""[..], Credentials::None),
            (b"Basic abc", Credentials::None),
            (b"Bearer", Credentials::None),
            (b"Bearer \xff", Credentials::None),
            (right.as_bytes(), Credentials::Bearer(TOKEN)),
            // One header that names two: one token, which is not the right one.
            (
                joined.as_bytes(),
                Credentials::Bearer(&joined["Bearer ".len()..]),
            ),
        ] {
            assert_eq!(credentials(&headers(&[value])), sent, "{value:?}");
        }
        assert_eq!(credentials(&HeaderMap::new()), Credentials::None);
    }

    #[tokio::test]
    async fn without_a_token_every_scrape_is_answered() {
        let several = ["Bearer anything", "Bearer else"];
        for authorization in [&[][..], &["Bearer anything"], &several] {
            let (status, _) = scraped(None, authorization).await;
            assert_eq!(status, StatusCode::OK, "{authorization:?}");
        }
    }
}
