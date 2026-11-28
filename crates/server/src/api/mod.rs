//! The HTTP API, all of it under `/api/v1`.
//!
//! One module per area. Each module exposes a `routes()` function, and [`router`] puts
//! them together.

mod access;
mod account_deletion;
mod admin;
mod api_tokens;
mod archive;
mod attachments;
mod auth;
mod board_ops;
mod body;
mod body_versions;
mod card_activity;
mod card_comments;
mod card_markdown;
mod cards;
mod categories;
mod comments;
mod config;
mod container_teams;
mod containers;
mod content_search;
pub mod contract;
mod course_events;
mod course_schedules;
mod courses;
pub(crate) mod delegated;
mod diaries;
mod diary_versions;
mod email_change;
mod email_verification;
mod favorites;
mod file_leases;
mod file_versions;
mod files;
mod invitation_mail;
mod invitations;
mod items;
mod limits;
mod lists;
mod mcp;
mod members;
mod notifications;
mod oidc;
mod oidc_held;
mod onboarding;
mod org;
mod org_admin;
mod organizations;
mod overview;
mod page_comments;
mod page_versions;
mod password_reset;
mod public_links;
mod query;
mod reactions;
mod recovery;
mod references;
mod search;
mod session;
mod shared;
mod signup_cookie;
mod subscriptions;
mod system;
mod teams;
mod templates;
mod time_entries;
mod today;
mod trash;
mod unit_comments;
mod unit_progress;
mod unit_reviews;
mod unit_versions;
mod uploads;
mod users;
mod views;
mod wikis;
mod workspace;
mod workspace_members;

use axum::Json;
use axum::Router;
use axum::extract::{DefaultBodyLimit, MatchedPath, Request, State};
use axum::http::header::CACHE_CONTROL;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use utoipa_axum::router::OpenApiRouter;

pub(crate) use access::{
    BlockAction, refusal, require_admin, require_container_permission, require_container_visible,
    require_granted, require_permission_through, require_workspace_access,
};
pub(crate) use body::{
    BLOCK_FIELDS, decode_json, decode_object, decode_object_with, decode_with_document, json_object,
};
pub(crate) use delegated::{Delegated, Read, Write};
pub(crate) use limits::WEBSOCKET_MESSAGE_LIMIT;
pub(crate) use session::{OptionalSession, RequiredSession};

use limits::{
    BodyLimit, SessionBodyLimit, UNAUTHENTICATED_BODY_LIMIT, limit_request_body,
    limit_session_request_body,
};

use crate::app::App;
use crate::error::{AppError, Reason};

/// The header that shows a request was sent by a script of the server's own pages, and
/// its value. A browser does not let another site's page set it, which is one half of
/// the API's protection against cross-site request forgery; see
/// [`guard_against_forgery`].
const HEADER_REQUESTED_WITH: &str = "X-Requested-With";
const HEADER_REQUESTED_WITH_XML: &str = "XMLHttpRequest";

/// The result of an API handler.
pub(crate) type ApiResult = Result<Response, AppError>;

/// Where the API is: the prefix of every path of it.
pub const PREFIX: &str = "/api/v1";

/// How many users `getWorkspaceUsersById` is asked for at once at most (the contract's
/// `maxItems` of `UserIds` says the same).
pub const MAX_USER_IDS: usize = 200;

/// The routes under [`PREFIX`], with what the contract says of them, apart by the limit
/// on the bodies of their requests.
struct Routes {
    /// The routes that take JSON from a session.
    json: OpenApiRouter<App>,
    /// The routes anyone can send a body to: they come before there is a session.
    unauthenticated: OpenApiRouter<App>,
    /// The routes that take files.
    uploads: OpenApiRouter<App>,
    /// The routes of a shared board, read with its read token: they look at no session.
    shared: OpenApiRouter<App>,
}

/// Every route under [`PREFIX`]. The router and the contract are both made from this.
fn routes() -> Routes {
    let json = OpenApiRouter::new()
        .merge(account_deletion::routes())
        .merge(api_tokens::routes())
        .merge(archive::routes())
        .merge(auth::routes())
        .merge(containers::routes())
        .merge(wikis::routes())
        .merge(page_comments::routes())
        .merge(page_versions::routes())
        .merge(diaries::routes())
        .merge(diary_versions::routes())
        .merge(courses::routes())
        .merge(course_events::routes())
        .merge(course_schedules::routes())
        .merge(unit_comments::routes())
        .merge(unit_progress::routes())
        .merge(unit_reviews::routes())
        .merge(unit_versions::routes())
        .merge(board_ops::routes())
        .merge(views::routes())
        .merge(attachments::routes())
        .merge(card_activity::routes())
        .merge(card_comments::routes())
        .merge(card_markdown::routes())
        .merge(lists::routes())
        .merge(mcp::routes())
        .merge(cards::routes())
        .merge(today::routes())
        .merge(trash::routes())
        .merge(categories::routes())
        .merge(favorites::routes())
        .merge(config::routes())
        .merge(files::routes())
        .merge(file_versions::routes())
        .merge(file_leases::routes())
        .merge(invitations::routes())
        .merge(invitation_mail::routes())
        .merge(members::routes())
        .merge(notifications::routes())
        .merge(workspace_members::routes())
        .merge(teams::routes())
        .merge(container_teams::routes())
        .merge(onboarding::routes())
        .merge(org::routes())
        .merge(org_admin::routes())
        .merge(organizations::routes())
        .merge(overview::routes())
        .merge(references::routes())
        .merge(search::routes())
        .merge(content_search::routes())
        .merge(public_links::routes())
        .merge(reactions::routes())
        .merge(subscriptions::routes())
        .merge(templates::routes())
        .merge(time_entries::routes())
        .merge(uploads::routes())
        .merge(users::routes())
        .merge(workspace::routes());
    let uploads = OpenApiRouter::new()
        .merge(files::upload_routes())
        .merge(uploads::part_routes());
    // What asks whether the server is there needs no session either.
    let unauthenticated = OpenApiRouter::new()
        .merge(auth::unauthenticated_routes())
        .merge(password_reset::routes())
        .merge(email_verification::routes())
        .merge(email_change::routes())
        .merge(invitations::unauthenticated_routes())
        .merge(oidc::unauthenticated_routes())
        .merge(system::routes());
    Routes {
        json,
        unauthenticated,
        uploads,
        shared: shared::routes(),
    }
}

/// The routes under `/api/v1`.
///
/// Every request that changes something is checked for cross-site request forgery, and
/// responses are marked as not cacheable unless a handler set its own policy. Request bodies are limited: to
/// `max_request_bytes` where a route takes JSON and the request has a session, to less
/// where no session is needed, none is claimed, or the claimed one does not exist. The
/// routes that take files are outside these limits.
pub fn router(app: &App) -> Router<App> {
    let session_limit = SessionBodyLimit {
        app: app.clone(),
        limit: BodyLimit(app.config().max_request_bytes),
    };
    let routes = routes();
    let json = Router::from(routes.json).layer(middleware::from_fn_with_state(
        session_limit,
        limit_session_request_body,
    ));
    let unauthenticated = Router::from(routes.unauthenticated).layer(
        middleware::from_fn_with_state(UNAUTHENTICATED_BODY_LIMIT, limit_request_body),
    );
    // Not behind the limits above. A file upload is limited to one part
    // (`upload_part_bytes`) as it is read, and a part of an upload to its size. An
    // archive comes as an upload in parts; its import is a JSON request.
    let uploads = Router::from(routes.uploads);
    // A shared board is read as anybody with its link reads it: a session that comes
    // along is not looked at, not even to be renewed.
    let shared = Router::from(routes.shared).layer(middleware::from_fn_with_state(
        UNAUTHENTICATED_BODY_LIMIT,
        limit_request_body,
    ));

    Router::new()
        .merge(json)
        .merge(unauthenticated)
        .merge(uploads)
        .route_layer(middleware::from_fn_with_state(
            app.clone(),
            session::renew_session_cookie,
        ))
        .merge(shared)
        // The limits above are the ones that count, and they answer in the API's own
        // shape; the framework's limit of 2 MB on what an extractor reads would cut in
        // below them with an answer of its own.
        .layer(DefaultBodyLimit::disable())
        .route_layer(middleware::from_fn_with_state(
            app.clone(),
            guard_against_forgery,
        ))
        .route_layer(middleware::from_fn(mark_not_cacheable))
}

/// The administration routes, with their full paths (`/api/v1/admin/...`).
///
/// They are to be served on the local Unix socket only, and refuse every request that
/// arrives another way. Whatever else is asked on that socket gets a 404.
pub fn admin_router() -> Router<App> {
    // The one body this socket takes is a password; it gets the small limit, and the
    // refusal in the API's own format.
    admin::routes()
        .layer(middleware::from_fn_with_state(
            UNAUTHENTICATED_BODY_LIMIT,
            limit_request_body,
        ))
        .layer(DefaultBodyLimit::disable())
}

/// Refuses requests that another site's page may have made a browser send.
///
/// A browser sends the session cookie along by itself, so a request that comes with it
/// must show that the server's own pages sent it. Two things show that, and a request
/// that changes something needs both:
///
/// * If it has an `Origin` header, the origin is the server's own (or one of
///   `allowed_origins`). Browsers set the header on such requests and scripts cannot
///   change it.
/// * It has the header `X-Requested-With: XMLHttpRequest`, which a page of another site
///   cannot add without the server's leave, and no leave is given. A request with a
///   token in its `Authorization` header needs none: no browser adds that header by
///   itself.
///
/// `GET`, `HEAD` and `OPTIONS` requests change nothing and are not checked: an image or
/// a link can point at the API. A request without any session is checked like one with the cookie:
/// a forged login would sign the browser in to somebody else's account.
///
/// The MCP server ([`mcp`]) needs no header: it reads no cookie, so a browser sends it
/// nothing a page of another site could use, and a client that sends no token is told
/// so (401) as the transport has it. Its `Origin` is checked all the same.
pub(crate) async fn guard_against_forgery(
    State(app): State<App>,
    request: Request,
    next: Next,
) -> Response {
    if is_safe(request.method()) {
        return next.run(request).await;
    }
    let headers = request.headers();
    if crate::http_credentials::foreign_origin(app.authenticator().origins(), headers).is_some() {
        tracing::debug!("a request from another origin was refused");
        return foreign_origin().into_response();
    }
    let reads_no_cookie = request
        .extensions()
        .get::<MatchedPath>()
        .is_some_and(|path| path.as_str().ends_with(mcp::PATH));
    let sent_on_purpose = reads_no_cookie
        || crate::http_credentials::bearer_token(headers).is_some()
        || headers
            .get(HEADER_REQUESTED_WITH)
            .and_then(|value| value.to_str().ok())
            == Some(HEADER_REQUESTED_WITH_XML);
    if sent_on_purpose {
        next.run(request).await
    } else {
        tracing::debug!("a request without the X-Requested-With header was refused");
        AppError::forbidden("the X-Requested-With: XMLHttpRequest header is required")
            .with_reason(Reason::Csrf)
            .into_response()
    }
}

/// Whether a request of this method changes nothing.
fn is_safe(method: &Method) -> bool {
    matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

/// The refusal of a request, or of a websocket handshake, that a page of another origin
/// sent.
pub(crate) fn foreign_origin() -> AppError {
    AppError::forbidden("requests from this origin are not accepted")
        .with_reason(Reason::ForeignOrigin)
}

/// Runs the rest of the request as a task of its own, so that it finishes even if the
/// client disconnects.
///
/// The framework drops a handler when its connection closes. Spawning the handler ensures
/// multi-step operations (such as storing a board and filing it under a category, or
/// storing blocks and broadcasting to listeners) run to completion.
pub(crate) async fn run_to_completion(
    State(app): State<App>,
    request: Request,
    next: Next,
) -> Response {
    match app.spawn_tracked(next.run(request)).await {
        Ok(response) => response,
        // Let the panic layer see a panic of the handler as if it had happened here.
        Err(error) if error.is_panic() => std::panic::resume_unwind(error.into_panic()),
        Err(_) => AppError::internal("the request was cancelled").into_response(),
    }
}

/// API responses depend on who asks and change at any time, so no cache may keep them.
async fn mark_not_cacheable(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .entry(CACHE_CONTROL)
        .or_insert(HeaderValue::from_static("no-store"));
    response
}

/// A JSON response with status 200.
pub(crate) fn json_response(value: &impl Serialize) -> Response {
    Json(value).into_response()
}

/// What an operation that made something new answers with: status 201 and what it
/// made, as JSON.
pub(crate) fn created(value: &impl Serialize) -> Response {
    (StatusCode::CREATED, Json(value)).into_response()
}

/// What an operation without a result answers with: status 204 and no body.
pub(crate) fn no_content() -> Response {
    StatusCode::NO_CONTENT.into_response()
}

/// The request's `Accept-Language`, if it has one that is text.
pub(crate) fn accept_language(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(axum::http::header::ACCEPT_LANGUAGE)
        .and_then(|value| value.to_str().ok())
}

/// The value of a query flag that is set.
pub(crate) const TRUE: &str = "true";
