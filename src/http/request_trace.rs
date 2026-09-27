//! Request tracing without the raw request target.
//!
//! tower-http's `DefaultMakeSpan` records `uri = %request.uri()` (path and
//! query) on every request span. Paths and queries here carry credentials:
//! public share tokens, ICS feed tokens and OIDC `code`/`state`. The span
//! built below records only the method, the HTTP version and axum's route
//! template (`MatchedPath`, e.g. `/api/v1/share/{token}`), never the concrete
//! path, query or headers. Requests without a matched route (static files,
//! SPA fallback, unknown paths) get a constant.

use axum::extract::MatchedPath;
use axum::http::{Method, Request};
use tower_http::classify::{ServerErrorsAsFailures, SharedClassifier};
use tower_http::trace::{MakeSpan, TraceLayer};
use tracing::Span;

/// Route label for requests no router path matched.
pub const UNMATCHED_ROUTE: &str = "<unmatched>";

/// `MakeSpan` recording only method, route template and version.
#[derive(Clone, Copy, Debug, Default)]
pub struct SafeMakeSpan;

impl<B> MakeSpan<B> for SafeMakeSpan {
    fn make_span(&mut self, request: &Request<B>) -> Span {
        // The matched path is a template from the route table, so it is
        // bounded and never holds request data. `Router::layer` wraps each
        // route after axum inserts it; outside a route it is absent.
        let route = request
            .extensions()
            .get::<MatchedPath>()
            .map_or(UNMATCHED_ROUTE, MatchedPath::as_str);
        // Same target as tower-http's default span so existing
        // `RUST_LOG=tower_http=debug` filters keep the request context.
        tracing::debug_span!(
            target: "tower_http::trace::make_span",
            "request",
            method = %method_label(request.method()),
            route = %route,
            version = ?request.version(),
        )
    }
}

/// Standard methods by name; extension methods are client-chosen tokens.
fn method_label(method: &Method) -> &'static str {
    match *method {
        Method::GET => "GET",
        Method::HEAD => "HEAD",
        Method::POST => "POST",
        Method::PUT => "PUT",
        Method::DELETE => "DELETE",
        Method::PATCH => "PATCH",
        Method::OPTIONS => "OPTIONS",
        Method::CONNECT => "CONNECT",
        Method::TRACE => "TRACE",
        _ => "OTHER",
    }
}

/// `TraceLayer::new_for_http` with the redacting span; request, response
/// (status, latency) and failure events are tower-http's defaults.
pub fn layer() -> TraceLayer<SharedClassifier<ServerErrorsAsFailures>, SafeMakeSpan> {
    TraceLayer::new_for_http().make_span_with(SafeMakeSpan)
}

/// DEBUG-level log capture for trace redaction tests.
#[cfg(test)]
pub(crate) mod capture {
    use std::io;
    use std::sync::{Arc, Mutex};

    use tracing::subscriber::DefaultGuard;
    use tracing_subscriber::fmt::format::FmtSpan;

    #[derive(Clone, Default)]
    pub(crate) struct Captured(Arc<Mutex<Vec<u8>>>);

    impl Captured {
        pub(crate) fn text(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
    }

    impl io::Write for Captured {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// Thread-local fmt subscriber at TRACE that also prints span creation
    /// and close with their fields; use from a current-thread runtime.
    pub(crate) fn logs() -> (Captured, DefaultGuard) {
        let captured = Captured::default();
        let writer = captured.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .with_span_events(FmtSpan::NEW | FmtSpan::CLOSE)
            .with_writer(move || writer.clone())
            .finish();
        (captured, tracing::subscriber::set_default(subscriber))
    }
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Method, Request, StatusCode};
    use axum::middleware::{self, Next};
    use axum::response::Response;
    use axum::routing::get;
    use axum::Router;
    use tower::ServiceExt;

    use super::{capture, layer, method_label, UNMATCHED_ROUTE};

    const SHARE_TOKEN: &str = "synthShareTok3n9f2a";
    const ICS_TOKEN: &str = "synthIcsTok3n41c7";
    const OIDC_CODE: &str = "synthOidcCode8e10";
    const OIDC_STATE: &str = "synthOidcState5b3d";
    const SPA_TOKEN: &str = "synthSpaShareTok77e4";
    const UNKNOWN_SEGMENT: &str = "synthUnknownSeg0c9b";
    const BEARER: &str = "synthBearerSecret2d6f";
    const COOKIE: &str = "synthCookieSecret19aa";

    async fn passthrough(req: axum::extract::Request, next: Next) -> Response {
        next.run(req).await
    }

    /// Mirrors the production shape: templated API routes behind a
    /// middleware, merged with a router whose only entry is a fallback
    /// service (static files / SPA shell), traced via `Router::layer`.
    fn app() -> Router {
        let api = Router::new()
            .route("/api/v1/share/{token}", get(|| async { "meta" }))
            .route(
                "/api/v1/ics/{token}",
                get(|| async { StatusCode::INTERNAL_SERVER_ERROR }),
            )
            .route(
                "/api/v1/auth/oidc/{provider}/callback",
                get(|| async { StatusCode::SEE_OTHER }),
            )
            .layer(middleware::from_fn(passthrough));
        let static_files =
            Router::new().fallback_service(tower::service_fn(|_req: Request<Body>| async {
                Ok::<_, std::convert::Infallible>(Response::new(Body::from("<html>")))
            }));
        api.merge(static_files).layer(layer())
    }

    #[tokio::test]
    async fn request_span_omits_path_query_and_headers() {
        let (captured, guard) = capture::logs();
        let requests = [
            (format!("/api/v1/share/{SHARE_TOKEN}"), StatusCode::OK),
            (
                format!("/api/v1/ics/{ICS_TOKEN}"),
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
            (
                format!("/api/v1/auth/oidc/google/callback?code={OIDC_CODE}&state={OIDC_STATE}"),
                StatusCode::SEE_OTHER,
            ),
            (format!("/s/{SPA_TOKEN}?code={OIDC_CODE}"), StatusCode::OK),
            (
                format!("/api/v1/{UNKNOWN_SEGMENT}?state={OIDC_STATE}"),
                StatusCode::OK,
            ),
        ];
        for (uri, status) in &requests {
            let response = app()
                .oneshot(
                    Request::builder()
                        .uri(uri.as_str())
                        .header("authorization", format!("Bearer {BEARER}"))
                        .header("cookie", format!("fvoci_session={COOKIE}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), *status, "{uri}");
            // Drain the body so the span closes before the log is read.
            axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
        }
        drop(guard);

        let log = captured.text();
        for secret in [
            SHARE_TOKEN,
            ICS_TOKEN,
            OIDC_CODE,
            OIDC_STATE,
            SPA_TOKEN,
            UNKNOWN_SEGMENT,
            BEARER,
            COOKIE,
            "google",
            "uri=",
            "headers=",
        ] {
            assert!(
                !log.contains(secret),
                "{secret:?} leaked into trace:\n{log}"
            );
        }

        // Route templates and the unmatched constant identify requests.
        for route in [
            "route=/api/v1/share/{token}",
            "route=/api/v1/ics/{token}",
            "route=/api/v1/auth/oidc/{provider}/callback",
        ] {
            assert!(log.contains(route), "{route} missing:\n{log}");
        }
        let unmatched = format!("route={UNMATCHED_ROUTE}");
        assert!(log.matches(unmatched.as_str()).count() >= 2, "{log}");

        // tower-http's request/response/failure events are kept.
        assert_eq!(
            log.matches("started processing request").count(),
            5,
            "{log}"
        );
        assert_eq!(
            log.matches("finished processing request").count(),
            5,
            "{log}"
        );
        for field in [
            "method=GET",
            "version=HTTP/1.1",
            "status=200",
            "status=303",
            "status=500",
            "latency=",
            "classification=Status code: 500",
        ] {
            assert!(log.contains(field), "{field} missing:\n{log}");
        }
        assert_eq!(log.matches("response failed").count(), 1, "{log}");
    }

    #[test]
    fn extension_methods_are_not_recorded_verbatim() {
        let method = Method::from_bytes(b"SYNTHSECRETVERB").unwrap();
        assert_eq!(method_label(&method), "OTHER");
        assert_eq!(method_label(&Method::PATCH), "PATCH");
    }
}
