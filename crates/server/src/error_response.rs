//! An error as the answer to a request: the [`Problem`] of RFC 9457 as the body, sent as
//! `application/problem+json` with the status of its kind. What an error is and what
//! its body says is `error`'s; here it is given its HTTP form.

use axum::body::Body;
use axum::extract::Request;
use axum::http::header::{CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE, RETRY_AFTER};
use axum::http::{HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use fvoci_model::{PROBLEM_MEDIA_TYPE, Problem};

use crate::error::{AppError, ErrorKind};

/// The response with `problem` as its body, as `application/problem+json`.
fn problem_response(problem: &Problem) -> Response {
    // A problem is a handful of strings and numbers; serializing it cannot fail. If it
    // ever did, the status alone still says what kind of error it is.
    let body = serde_json::to_vec(problem).unwrap_or_default();
    let status = StatusCode::from_u16(problem.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let mut response = (status, Body::from(body)).into_response();
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static(PROBLEM_MEDIA_TYPE));
    response
}

/// Marks an error answered in the format of the protocol it is of, which is left as
/// it is: the JSON-RPC error with which the MCP server answers a body that is no
/// message of it, as that protocol has it answered.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SpokenInItsProtocol;

/// Answers in the API's own format what was refused before a handler ran, by the
/// framework: a parameter of the path that is no text, for one. Such an answer comes
/// with the framework's words as plain text; it becomes the problem of its status,
/// with words of this server, and keeps its other headers. A status the API has no
/// kind for becomes 400 if it is the client's and 500 if not. An error marked
/// [`SpokenInItsProtocol`] is left as it is.
pub(crate) async fn problems_only(request: Request, next: Next) -> Response {
    let response = next.run(request).await;
    let status = response.status();
    let is_error = status.is_client_error() || status.is_server_error();
    let extensions = response.extensions();
    if !is_error
        || extensions.get::<ReportedError>().is_some()
        || extensions.get::<SpokenInItsProtocol>().is_some()
    {
        return response;
    }
    let kind = ErrorKind::of_status(status.as_u16()).unwrap_or(if status.is_client_error() {
        ErrorKind::BadRequest
    } else {
        ErrorKind::Internal
    });
    let message = match kind {
        ErrorKind::BadRequest => "the request is malformed",
        _ => status
            .canonical_reason()
            .unwrap_or("the request was refused"),
    };
    tracing::debug!(
        status = status.as_u16(),
        "an answer that was no problem was made one"
    );
    let (parts, _) = response.into_parts();
    let mut problem = AppError::of_kind(kind, message).into_response();
    for (name, value) in &parts.headers {
        if ![CONTENT_TYPE, CONTENT_LENGTH, CONTENT_ENCODING].contains(name) {
            problem.headers_mut().insert(name, value.clone());
        }
    }
    problem
}

/// The body of an error response, kept beside the response: as it left the handler,
/// so that the request log can add the identifier of the request to it, and then as it
/// was sent, so that the measurement of the requests can count it by its problem.
#[derive(Debug, Clone)]
pub(crate) struct ReportedError(pub(crate) Problem);

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        self.log();
        let body = self.body();
        let mut response = problem_response(&body);
        if let Some(seconds) = body.retry_after {
            response
                .headers_mut()
                .insert(RETRY_AFTER, HeaderValue::from(seconds));
        }
        response.extensions_mut().insert(ReportedError(body));
        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Reason;

    /// Turns the error into its response and returns the status and the body.
    async fn reported(error: AppError) -> (u16, Problem) {
        let response = error.into_response();
        let status = response.status().as_u16();
        assert_eq!(
            response
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("application/problem+json"),
            "the content type"
        );
        assert!(
            response.extensions().get::<ReportedError>().is_some(),
            "the body is kept beside the response for the request log"
        );
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("the body is read");
        (
            status,
            serde_json::from_slice(&body).expect("the body is JSON"),
        )
    }

    #[tokio::test]
    async fn every_kind_has_its_status_type_and_title() {
        let cases = [
            (
                AppError::bad_request("bad field"),
                400,
                "bad-request",
                "bad field",
            ),
            (
                AppError::unauthorized("no session"),
                401,
                "unauthorized",
                "no session",
            ),
            (
                AppError::forbidden("not yours"),
                403,
                "forbidden",
                "not yours",
            ),
            (
                AppError::not_found("board"),
                404,
                "not-found",
                "board not found",
            ),
            (
                AppError::method_not_allowed(),
                405,
                "method-not-allowed",
                "method not allowed",
            ),
            (AppError::conflict("taken"), 409, "conflict", "taken"),
            (
                AppError::precondition_failed("changed"),
                412,
                "precondition-failed",
                "changed",
            ),
            (
                AppError::payload_too_large(64),
                413,
                "payload-too-large",
                "the request body is larger than 64 bytes",
            ),
            (
                AppError::range_not_satisfiable("no such bytes"),
                416,
                "range-not-satisfiable",
                "no such bytes",
            ),
            (
                AppError::rate_limited(std::time::Duration::from_millis(1500)),
                429,
                "rate-limited",
                "too many attempts; try again later",
            ),
            (
                AppError::internal("the disk is on fire at /var/lib"),
                500,
                "internal",
                "internal server error",
            ),
            (
                AppError::service_unavailable("not ready: database"),
                503,
                "service-unavailable",
                "not ready: database",
            ),
        ];
        assert_eq!(cases.len(), ErrorKind::ALL.len(), "a case for every kind");
        for (error, status, name, detail) in cases {
            let kind = error.kind();
            let (got_status, body) = reported(error).await;
            assert_eq!(got_status, status, "the status of {name}");
            assert_eq!(body.status, status, "the body repeats the status");
            assert_eq!(body.problem_type, format!("/problems/{name}"), "the type");
            assert_eq!(body.title, kind.title(), "the title of {name}");
            assert_eq!(body.detail, detail, "the detail of {name}");
            assert_eq!(body.request_id, None, "the request log adds the identifier");
            assert_eq!(body.instance, None, "and the instance");
        }
    }

    #[tokio::test]
    async fn an_error_with_a_reason_is_the_problem_of_that_reason() {
        for &reason in Reason::ALL {
            let forbidden = AppError::forbidden("x").with_reason(reason);
            let conflict = AppError::conflict("y").with_reason(reason);
            let (status, body) = reported(forbidden).await;
            let (other_status, other) = reported(conflict).await;
            assert_eq!((status, other_status), (403, 409));
            let name = reason.as_str().replace('_', "-");
            assert_eq!(body.problem_type, format!("/problems/{name}"));
            assert_eq!(
                (&other.problem_type, &other.title),
                (&body.problem_type, &body.title),
                "the same type and title whatever the status and the detail"
            );
            assert_eq!(body.title, reason.title());
            assert_eq!(body.reason.as_deref(), Some(reason.as_str()));
        }
    }

    #[tokio::test]
    async fn reason_errors_and_limit_are_in_the_body_only_when_set() {
        let (_, plain) = reported(AppError::bad_request("x")).await;
        assert_eq!(
            serde_json::to_value(&plain).expect("the body serializes"),
            serde_json::json!({
                "type": "/problems/bad-request", "title": "Bad request", "status": 400,
                "detail": "x"
            }),
            "nothing else"
        );

        let error = AppError::conflict("no admins")
            .with_field("role")
            .with_reason(Reason::LastAdmin);
        let (_, body) = reported(error).await;
        assert_eq!(body.reason.as_deref(), Some("last_admin"), "the reason");
        assert_eq!(
            serde_json::to_value(&body.errors).expect("the entries serialize"),
            serde_json::json!([
                { "pointer": "/role", "detail": "no admins", "reason": "last_admin" }
            ]),
            "the member, with the detail and the reason of the problem"
        );

        let (_, body) = reported(AppError::payload_too_large(4096)).await;
        assert_eq!(body.limit, Some(4096), "the limit");
    }

    #[tokio::test]
    async fn what_was_refused_is_told_by_where_it_is_in_the_request() {
        let error = AppError::bad_request("no")
            .with_field("a/b")
            .with_pointer("/blocks/0/title")
            .with_parameter("page")
            .with_header("Content-Digest");
        let (_, body) = reported(error).await;
        assert_eq!(
            serde_json::to_value(&body.errors).expect("the entries serialize"),
            serde_json::json!([
                { "pointer": "/a~1b", "detail": "no", "reason": "invalid" },
                { "pointer": "/blocks/0/title", "detail": "no", "reason": "invalid" },
                { "parameter": "page", "detail": "no", "reason": "invalid" },
                { "header": "Content-Digest", "detail": "no", "reason": "invalid" },
            ])
        );
    }

    #[tokio::test]
    async fn a_server_error_does_not_say_what_went_wrong() {
        let error = AppError::internal("connection to 10.0.0.5 refused").context("saving");
        assert_eq!(
            error.to_string(),
            "saving: connection to 10.0.0.5 refused",
            "the log gets the cause"
        );
        let (status, body) = reported(error).await;
        assert_eq!(status, 500);
        assert_eq!(body.detail, "internal server error", "the client does not");
        assert_eq!(body.problem_type, "/problems/internal");
    }

    /// Where an error came up is for the log. What a client is told stays as it is.
    #[tokio::test]
    async fn context_does_not_change_what_a_client_is_told() {
        let error = AppError::bad_request("no admins").context("removing a member");
        assert_eq!(error.kind(), ErrorKind::BadRequest);
        let (status, body) = reported(error).await;
        assert_eq!(status, 400);
        assert_eq!(
            body.detail, "no admins",
            "the app layer's words are not sent"
        );

        let error = AppError::internal("disk full").context("writing a file");
        assert!(
            error.to_string().contains("writing a file"),
            "a server error carries it for the log: {error}"
        );
    }
}
