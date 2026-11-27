//! Measuring the HTTP requests on the API's listener: how many are being answered, how
//! long each took, by method, route and status, and which problems they were refused
//! with. The metrics themselves are [`RequestMetrics`]; here is the middleware that
//! counts into them.

use std::sync::{Arc, OnceLock};
use std::time::Instant;

use axum::extract::{MatchedPath, Request, State};
use axum::http::Method;
use axum::middleware::Next;
use axum::response::Response;

use crate::error::PROBLEMS_PATH;
use crate::error_response::ReportedError;
use crate::metrics::{ABANDONED, Metrics, OTHER_METHOD, OTHER_PROBLEM, RequestMetrics, UNMATCHED};

/// Where the template of the route a request is routed to is put, once routing has
/// found it: given to the request by [`measure_requests`] and filled by [`note_route`],
/// so that it is there also when the answer never comes back up, after a panic or when
/// the client went away.
#[derive(Debug, Clone, Default)]
struct RouteSlot(Arc<OnceLock<MatchedPath>>);

/// Puts the template of the route that is answering into the request's [`RouteSlot`].
/// A layer of the routes only: requests that match no route do not pass here, and are
/// measured as [`UNMATCHED`].
pub(crate) async fn note_route(request: Request, next: Next) -> Response {
    if let (Some(path), Some(RouteSlot(slot))) = (
        request.extensions().get::<MatchedPath>(),
        request.extensions().get::<RouteSlot>(),
    ) {
        let _ = slot.set(path.clone());
    }
    next.run(request).await
}

/// What [`measure_requests`] needs: the metrics, and the base path the routes are
/// nested under, which is left out of the route label.
#[derive(Debug, Clone)]
pub(crate) struct Measured {
    pub(crate) metrics: Metrics,
    pub(crate) base_path: Arc<str>,
}

/// Counts the request as being answered until its answer's head is ready, and then its
/// duration by method, route template and status. The outermost layer of the API's
/// listener, so that what the inner layers answer, a contained panic among them, is
/// measured too. A request whose client goes away before the answer is measured with
/// the status `abandoned`, at the time it was given up. An answer that is a problem is
/// counted by the problem's name.
pub(crate) async fn measure_requests(
    State(measured): State<Measured>,
    mut request: Request,
    next: Next,
) -> Response {
    let Some(requests) = measured.metrics.requests() else {
        return next.run(request).await;
    };
    let route = RouteSlot::default();
    request.extensions_mut().insert(route.clone());
    let measuring = Measuring::start(
        requests,
        method_label(request.method()),
        route,
        &measured.base_path,
    );
    let response = next.run(request).await;
    measuring.finish(response.status().as_str());
    if let Some(ReportedError(problem)) = response.extensions().get::<ReportedError>() {
        requests.problem(problem_label(&problem.problem_type));
    }
    response
}

/// A request being measured: counted as being answered while it lives, and measured
/// as [`ABANDONED`] if it is dropped before it is finished.
struct Measuring<'a> {
    requests: &'a RequestMetrics,
    method: &'static str,
    route: RouteSlot,
    base_path: &'a str,
    started: Instant,
    finished: bool,
}

impl<'a> Measuring<'a> {
    fn start(
        requests: &'a RequestMetrics,
        method: &'static str,
        route: RouteSlot,
        base_path: &'a str,
    ) -> Self {
        requests.started();
        Self {
            requests,
            method,
            route,
            base_path,
            started: Instant::now(),
            finished: false,
        }
    }

    /// Measures the request with the status of its answer.
    fn finish(mut self, status: &str) {
        self.observe(status);
        self.finished = true;
    }

    fn observe(&self, status: &str) {
        let RouteSlot(slot) = &self.route;
        let route = slot
            .get()
            .map_or(UNMATCHED, |path| route_label(path.as_str(), self.base_path));
        self.requests
            .took(self.method, route, status, self.started.elapsed());
    }
}

impl Drop for Measuring<'_> {
    fn drop(&mut self) {
        self.requests.ended();
        if !self.finished {
            self.observe(ABANDONED);
        }
    }
}

/// The method as a label: one of the methods with routes, or [`OTHER_METHOD`], so that
/// a client cannot make a label of its own.
fn method_label(method: &Method) -> &'static str {
    match *method {
        Method::GET => "GET",
        Method::HEAD => "HEAD",
        Method::POST => "POST",
        Method::PUT => "PUT",
        Method::PATCH => "PATCH",
        Method::DELETE => "DELETE",
        Method::OPTIONS => "OPTIONS",
        _ => OTHER_METHOD,
    }
}

/// The names of the server's problems, those its pages describe (`problems::all`): a
/// set fixed in the code.
fn problem_names() -> &'static [String] {
    static NAMES: OnceLock<Vec<String>> = OnceLock::new();
    NAMES.get_or_init(|| {
        crate::problems::all()
            .into_iter()
            .map(|problem| problem.name)
            .collect()
    })
}

/// Starts the count of every problem at zero, if the metrics are kept.
pub(crate) fn know_problems(metrics: &Metrics) {
    if let Some(requests) = metrics.requests() {
        let names: Vec<&'static str> = problem_names().iter().map(String::as_str).collect();
        requests.know_problems(&names);
    }
}

/// The problem as a label: the name of the server's problem the `type` ends in, after
/// [`PROBLEMS_PATH`] and whatever base path is before it, or [`OTHER_PROBLEM`].
fn problem_label(problem_type: &str) -> &'static str {
    let name = problem_type
        .rsplit_once('/')
        .filter(|(pages, _)| pages.ends_with(PROBLEMS_PATH))
        .map(|(_, name)| name);
    name.and_then(|name| problem_names().iter().find(|known| *known == name))
        .map_or(OTHER_PROBLEM, String::as_str)
}

/// The route template as a label: as the router has it, without the base path.
fn route_label<'a>(template: &'a str, base_path: &str) -> &'a str {
    match template.strip_prefix(base_path) {
        Some("") => "/",
        Some(rest) if rest.starts_with('/') => rest,
        _ => template,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use axum::Router;
    use axum::body::Body;
    use axum::http::StatusCode;
    use axum::middleware;
    use axum::routing::get;
    use tower::ServiceExt as _;

    use super::*;
    use crate::error::{AppError, ErrorKind, Reason};

    /// A router with a route that panics and one that takes a minute, measured as the
    /// server's router is.
    fn measured_router(metrics: &Metrics) -> Router {
        async fn panics() -> &'static str {
            panic!("the handler panics on purpose")
        }
        async fn takes_a_minute() -> &'static str {
            tokio::time::sleep(Duration::from_secs(60)).await;
            "late"
        }
        Router::new()
            .route("/boards/{boardId}/panic", get(panics))
            .route("/slow", get(takes_a_minute))
            .route_layer(middleware::from_fn(note_route))
            .layer(middleware::from_fn(crate::request_log::contain_panics))
            .layer(middleware::from_fn_with_state(
                Measured {
                    metrics: metrics.clone(),
                    base_path: Arc::from(""),
                },
                measure_requests,
            ))
    }

    fn rendered(metrics: &Metrics) -> String {
        let text = metrics
            .render()
            .expect("the metrics are kept")
            .expect("the metrics encode");
        String::from_utf8(text).expect("the text format is UTF-8")
    }

    fn get_request(path: &str) -> Request {
        Request::get(path).body(Body::empty()).expect("a request")
    }

    #[tokio::test]
    async fn a_request_whose_handler_panics_keeps_its_route() {
        // The panic layer logs. tracing caches whether a log line is wanted once for
        // every thread: logged here with no subscriber, it could be cached as never
        // wanted while the request log's tests, on other threads, read theirs back.
        let subscriber = tracing_subscriber::fmt()
            .with_writer(std::io::sink)
            .finish();
        let _logging = tracing::subscriber::set_default(subscriber);
        let metrics = Metrics::kept().expect("the metrics register");
        let response = measured_router(&metrics)
            .oneshot(get_request("/boards/b1/panic"))
            .await
            .expect("an answer");
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let text = rendered(&metrics);
        assert!(
            text.contains(
                "fvoci_http_request_duration_seconds_count{method=\"GET\",\
                 route=\"/boards/{boardId}/panic\",status=\"500\"} 1"
            ),
            "{text}"
        );
        assert!(
            text.contains("fvoci_http_problems_total{problem=\"internal\"} 1"),
            "the contained panic is the problem it was answered with: {text}"
        );
    }

    #[tokio::test]
    async fn a_request_the_client_gives_up_is_measured_as_abandoned() {
        let metrics = Metrics::kept().expect("the metrics register");
        let answer = tokio::time::timeout(
            Duration::from_millis(50),
            measured_router(&metrics).oneshot(get_request("/slow")),
        )
        .await;
        assert!(answer.is_err(), "the request is given up");
        let text = rendered(&metrics);
        assert!(
            text.contains(
                "fvoci_http_request_duration_seconds_count{method=\"GET\",\
                 route=\"/slow\",status=\"abandoned\"} 1"
            ),
            "{text}"
        );
        assert!(
            text.contains("\nfvoci_http_requests_in_flight 0\n"),
            "{text}"
        );
    }

    #[test]
    fn a_method_without_routes_is_one_label() {
        assert_eq!(method_label(&Method::PATCH), "PATCH");
        for name in ["TRACE", "CONNECT", "PROPFIND", "X-ANYTHING"] {
            let method = Method::from_bytes(name.as_bytes()).expect("a method");
            assert_eq!(method_label(&method), OTHER_METHOD, "{name}");
        }
    }

    #[test]
    fn a_problem_is_one_of_the_servers_or_other() {
        for kind in ErrorKind::ALL {
            let name = kind.name();
            assert_eq!(problem_label(&format!("/problems/{name}")), name);
        }
        for &reason in Reason::ALL {
            let name = reason.name();
            assert_eq!(problem_label(&format!("/problems/{name}")), name);
            assert_eq!(
                problem_label(&format!("/boards/problems/{name}")),
                name,
                "under a base path"
            );
        }
        for problem_type in [
            "",
            "/problems/",
            "/problems/no-such-problem",
            "/problems/not-found/x",
            "/elsewhere/not-found",
            "not-found",
            "about:blank",
            "/problems/foreign_origin",
        ] {
            assert_eq!(
                problem_label(problem_type),
                OTHER_PROBLEM,
                "{problem_type:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_problem_is_counted_by_its_name_as_it_was_sent() {
        let metrics = Metrics::kept().expect("the metrics register");
        let router = Router::new()
            .route(
                "/taken",
                get(|| async { AppError::conflict("taken").with_reason(Reason::UsernameTaken) }),
            )
            .layer(middleware::from_fn_with_state(
                Arc::new(crate::request_log::Served {
                    proxies: crate::config::TrustedProxies::default(),
                    base_path: "/boards".to_owned(),
                }),
                crate::request_log::log_request,
            ))
            .layer(middleware::from_fn_with_state(
                Measured {
                    metrics: metrics.clone(),
                    base_path: Arc::from("/boards"),
                },
                measure_requests,
            ));
        for _ in 0..2 {
            let response = router
                .clone()
                .oneshot(get_request("/taken"))
                .await
                .expect("an answer");
            assert_eq!(response.status(), StatusCode::CONFLICT);
        }
        let text = rendered(&metrics);
        assert!(
            text.contains("fvoci_http_problems_total{problem=\"username-taken\"} 2"),
            "{text}"
        );
    }

    #[test]
    fn the_base_path_is_left_out_of_the_route() {
        assert_eq!(
            route_label("/api/v1/boards/{boardId}", ""),
            "/api/v1/boards/{boardId}"
        );
        assert_eq!(
            route_label("/boards/api/v1/ping", "/boards"),
            "/api/v1/ping"
        );
        assert_eq!(route_label("/boards/", "/boards"), "/");
        assert_eq!(route_label("/boards", "/boards"), "/");
    }
}
