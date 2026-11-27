//! The administration API: setting a user's password without knowing the old one, which
//! also ends every session of that user, confirming the address of a registration whose
//! mail does not arrive, making a user an administrator of the instance, releasing an
//! organization from taking no password, comparing the file storage with the records,
//! and making the search index again.
//!
//! It is served on the local Unix socket only, never on the TCP listener, so that being
//! able to open the socket file is the credential. No session cookie or cross-site
//! request forgery header is required on this socket.

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, Request, State};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use serde::Deserialize;

use super::{ApiResult, decode_json, json_response, no_content};
use crate::Peer;
use crate::app::{App, Unrecorded};
use crate::auth::now_millis;
use crate::error::AppError;

pub(super) fn routes() -> Router<App> {
    Router::new()
        .route(
            &format!("{}/admin/users/{{username}}/password-reset", super::PREFIX),
            post(handle_password_reset_link),
        )
        .route(
            &format!("{}/admin/users/{{username}}/password", super::PREFIX),
            post(handle_admin_set_password),
        )
        .route(
            &format!("{}/admin/users/{{username}}/admin", super::PREFIX),
            post(handle_admin_make_admin),
        )
        .route(
            &format!(
                "{}/admin/users/{{username}}/email-verification",
                super::PREFIX
            ),
            post(handle_verify_email),
        )
        .route(
            &format!("{}/admin/users/{{username}}/erase", super::PREFIX),
            post(handle_admin_erase_account),
        )
        .route(
            &format!("{}/admin/files/reconcile", super::PREFIX),
            post(handle_reconcile_files),
        )
        .route(
            &format!("{}/admin/orgs/{{orgId}}/sso/release", super::PREFIX),
            post(handle_release_sso),
        )
        .route(
            &format!("{}/admin/search/reindex", super::PREFIX),
            post(handle_reindex_search),
        )
        .route_layer(middleware::from_fn(require_unix_connection))
        // The socket speaks the API's language for what it does not have as well.
        .fallback(|| async { AppError::not_found("route") })
        .method_not_allowed_fallback(|| async { AppError::method_not_allowed() })
}

/// Makes a single-use link for an operator to deliver without mail.
async fn handle_password_reset_link(
    State(app): State<App>,
    Path(username): Path<String>,
) -> ApiResult {
    let url = app.password_reset_link(&username).await?;
    Ok(json_response(&serde_json::json!({"url": url})))
}

/// The body of a request to set a password.
#[derive(Deserialize)]
struct AdminSetPasswordData {
    /// Optional: absent and `null` are no password, which is refused as an empty one.
    #[serde(default)]
    password: Option<String>,
}

/// Refuses requests that did not arrive through a Unix socket. The routes are not mounted
/// anywhere else; this middleware keeps them closed as defense in depth.
async fn require_unix_connection(request: Request, next: Next) -> Response {
    if Peer::of(&request) == Some(Peer::Unix) {
        next.run(request).await
    } else {
        // For anyone who does not come through the socket these routes do not exist.
        AppError::not_found("route").into_response()
    }
}

/// Sets a user's password and ends their sessions; recorded as the operator's in each
/// organization the user is a member of, and as every command is in the log.
async fn handle_admin_set_password(
    State(app): State<App>,
    Path(username): Path<String>,
    body: Bytes,
) -> ApiResult {
    let user_id = app.operator_user_id(&username).await;
    let result = async {
        // A body of `null` names no password, and is refused below as an empty one.
        let data: Option<AdminSetPasswordData> = decode_json(&body)?;
        let password = data.and_then(|data| data.password).unwrap_or_default();
        fvoci_model::check_new_password(&password)?;
        app.reset_user_password(&username, &password).await?;
        Ok(no_content())
    }
    .await;
    record_operator(Command::ResetPassword(user_id.as_deref()), result)
}

/// Confirms the address of the registration with the user name, as its mailed link
/// would, without signing anybody in: for a person whose mail does not arrive. 204, or
/// 404 if no registration that waits has the name. Recorded as the operator's in the
/// organization a registration by a link for members joins, and as every command is in
/// the log.
async fn handle_verify_email(State(app): State<App>, Path(username): Path<String>) -> ApiResult {
    let (user_id, result) = match app.confirm_signup_by_operator(&username).await {
        Ok(user_id) => (Some(user_id), Ok(no_content())),
        Err(error) => (None, Err(error)),
    };
    record_operator(Command::VerifyEmail(user_id.as_deref()), result)
}

/// Makes a user an administrator of the instance: the way back in when no
/// administrator can sign in any more.
async fn handle_admin_make_admin(
    State(app): State<App>,
    Path(username): Path<String>,
) -> ApiResult {
    let (user_id, result) = match app.make_admin_by_username(&username).await {
        Ok(user) => (Some(user.id), Ok(no_content())),
        Err(error) => (app.operator_user_id(&username).await, Err(error)),
    };
    record_operator(Command::MakeAdmin(user_id.as_deref()), result)
}

/// Erases a user's account at once, whether its person asked for that or not: it goes
/// from every organization, workspace and board, nothing of the person is kept, and
/// what it wrote stays under an identifier that names nobody; an organization that has
/// nobody else goes with it. 204; 404 if no account has the name; 409 for the
/// instance's only administrator (reason `last_admin`) and for the only administrator
/// of an organization that has other people, or of the instance's own first
/// organization (reason `last_org_admin`, with `orgId`).
/// As every command it is in the log, by the account's identifier.
async fn handle_admin_erase_account(
    State(app): State<App>,
    Path(username): Path<String>,
) -> ApiResult {
    let (user_id, erased) = app.erase_account_as_operator(&username).await;
    record_operator(
        Command::EraseAccount(user_id.as_deref()),
        erased.map(|()| no_content()),
    )
}

/// Releases an organization from taking no password (`ssoRequired` of its provider):
/// the way back in when nobody gets in through the provider, which an administrator
/// of the organization could then not switch off either. The provider stays as it is
/// otherwise, and the release is recorded. 204 also for an organization that takes a
/// password already; 404 for an identifier that names no organization in use.
async fn handle_release_sso(State(app): State<App>, Path(org_id): Path<String>) -> ApiResult {
    let result = app
        .release_sso_as_operator(&org_id)
        .await
        .map(|()| no_content());
    let org_id = fvoci_model::is_id(&org_id).then_some(org_id.as_str());
    record_operator(Command::ReleaseSso(org_id), result)
}

/// The body of a request to compare the file storage with the records; none for a
/// report alone.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReconcileFilesData {
    /// Whether to doom the objects nothing names at places of the server's making that
    /// are older than a day, for the housekeeping to remove. Absent, false.
    #[serde(default)]
    remove_unrecorded: bool,
}

/// Compares the file storage with the records of every organization: the objects
/// nothing names, at places of the server's making and elsewhere, and the places
/// records name where the storage holds nothing. Without a body, or with
/// `removeUnrecorded` false, it changes nothing. With it true, the objects nothing
/// names at places of the server's making that are older than a day are doomed: the
/// housekeeping removes them at its next run, asking again whether something holds
/// them. The others, and a record without its object, are reported and never changed.
/// The storage (`file_storage_path`, or the bucket under `file_storage_s3.key_prefix`) is taken
/// to be this database's alone: another server's objects there are named by nothing
/// here.
async fn handle_reconcile_files(State(app): State<App>, body: Bytes) -> ApiResult {
    let result = async {
        let data: Option<ReconcileFilesData> = if body.is_empty() {
            None
        } else {
            decode_json(&body)?
        };
        let unrecorded = if data.is_some_and(|data| data.remove_unrecorded) {
            Unrecorded::Doom
        } else {
            Unrecorded::Report
        };
        let report = app
            .housekeeping()
            .reconcile_files(unrecorded, now_millis())
            .await?;
        Ok(json_response(&report))
    }
    .await;
    record_operator(Command::ReconcileFiles, result)
}

/// Makes the index of the search server again and lists every board and wiki of every
/// organization to be sent to it: for an index that is thought to be wrong. Answers
/// how many were listed (`containers`); they are sent by the server's upkeep in
/// the seconds or minutes after, and until then a search finds what was sent so far.
/// 404 (`search_not_configured`) on a server without a search server, 503
/// (`search_unavailable`) if the search server cannot hold the index or does not take
/// the requests.
async fn handle_reindex_search(State(app): State<App>) -> ApiResult {
    let result = crate::search::reindex(&app)
        .await
        .map(|containers| json_response(&serde_json::json!({"containers": containers})));
    record_operator(Command::ReindexSearch, result)
}

/// A command of the operator, with what it was about, as its line in the log names
/// them.
#[derive(Debug, Clone, Copy)]
enum Command<'a> {
    /// Setting a user's password: the user, by identifier where the name was one's.
    ResetPassword(Option<&'a str>),
    /// Making a user an administrator of the instance: the user, as above.
    MakeAdmin(Option<&'a str>),
    /// Confirming the address of a registration: the account it became, if it did.
    VerifyEmail(Option<&'a str>),
    /// Erasing a user's account: the user, as above.
    EraseAccount(Option<&'a str>),
    /// Releasing an organization from taking no password: the organization, by
    /// identifier where the path held one.
    ReleaseSso(Option<&'a str>),
    /// Comparing the file storage with the records: the whole server.
    ReconcileFiles,
    /// Making the search index again: the whole server.
    ReindexSearch,
}

impl<'a> Command<'a> {
    /// The command's name in the log.
    const fn name(self) -> &'static str {
        match self {
            Self::ResetPassword(_) => "reset_password",
            Self::MakeAdmin(_) => "make_admin",
            Self::VerifyEmail(_) => "verify_email",
            Self::EraseAccount(_) => "erase_account",
            Self::ReleaseSso(_) => "release_sso",
            Self::ReconcileFiles => "reconcile_files",
            Self::ReindexSearch => "reindex_search",
        }
    }

    /// The user it was about, and the organization; each empty for none.
    fn target(self) -> (&'a str, &'a str) {
        match self {
            Self::ResetPassword(user_id)
            | Self::MakeAdmin(user_id)
            | Self::VerifyEmail(user_id)
            | Self::EraseAccount(user_id) => (user_id.unwrap_or_default(), ""),
            Self::ReleaseSso(org_id) => ("", org_id.unwrap_or_default()),
            Self::ReconcileFiles | Self::ReindexSearch => ("", ""),
        }
    }
}

/// Writes one line of the log for each command the socket carried out, failures of
/// validation and hashing included: the command, its target, and its outcome. Never
/// what was sent with it. The changes an organization holds are in its records too.
fn record_operator(command: Command<'_>, result: ApiResult) -> ApiResult {
    let status = match &result {
        Ok(response) => response.status().as_u16(),
        Err(error) => error.status(),
    };
    let (user_id, org_id) = command.target();
    tracing::info!(
        target: "audit",
        actor = "operator",
        command = command.name(),
        user_id,
        org_id,
        result = if result.is_ok() { "succeeded" } else { "failed" },
        status,
    );
    result
}

#[cfg(test)]
mod tests {
    use axum::extract::ConnectInfo;

    use crate::connection::Connection;
    use std::net::SocketAddr;

    use axum::body::Body;
    use axum::http::StatusCode;
    use tower::ServiceExt as _;

    use super::*;

    fn guarded() -> Router {
        Router::new()
            .route("/guarded", post(|| async { "reached" }))
            .route_layer(middleware::from_fn(require_unix_connection))
    }

    async fn status_for(peer: Option<Peer>) -> (StatusCode, String) {
        let mut request = Request::post("/guarded")
            .body(Body::empty())
            .expect("a valid request");
        if let Some(peer) = peer {
            request.extensions_mut().insert(ConnectInfo(Connection {
                peer,
                breaker: None,
            }));
        }
        let response = guarded()
            .oneshot(request)
            .await
            .expect("the router does not fail");
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("the body is read");
        (status, String::from_utf8_lossy(&body).into_owned())
    }

    /// Only a Unix socket connection passes the admin-route guard.
    #[tokio::test]
    async fn admin_required_lets_only_unix_connections_through() {
        let (status, body) = status_for(Some(Peer::Unix)).await;
        assert_eq!(status, StatusCode::OK, "a Unix socket connection: {body}");
        assert_eq!(body, "reached");

        let tcp: SocketAddr = "127.0.0.1:4000".parse().expect("a socket address");
        for peer in [Some(Peer::Tcp(tcp)), None] {
            let (status, body) = status_for(peer).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{peer:?}: {body}");
            assert_eq!(
                body,
                r#"{"type":"/problems/not-found","title":"Not found","status":404,"detail":"route not found"}"#,
                "{peer:?}"
            );
        }
    }
}
