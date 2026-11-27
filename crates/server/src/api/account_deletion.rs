//! Deleting the account of the session: what it comes to, asking for it, and taking
//! the asking back while the account waits.

use axum::body::Bytes;
use axum::extract::{Extension, State};
use axum::http::header::SET_COOKIE;
use fvoci_model::{AccountDeletion, AccountDeletionAsked, DeleteAccountRequest, Problem};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::auth::address_of;
use super::contract::tag;
use super::session::AccountSession;
use super::{ApiResult, RequiredSession, decode_object, json_response, no_content};
use crate::app::App;
use crate::client_address::ClientAddress;
use crate::error::AppError;
use crate::http_credentials;

pub(super) fn routes() -> OpenApiRouter<App> {
    OpenApiRouter::new().routes(routes!(
        handle_get_account_deletion,
        handle_request_account_deletion,
        handle_cancel_account_deletion
    ))
}

/// What deleting the account comes to
///
/// What stands in the way of deleting the account of the session, and what goes with
/// it, as things are now. The server keeps an administrator (`lastInstanceAdmin`), and
/// so does every organization that has other people in it (`soleAdminOf`): the account
/// is not deleted while either names it. An organization that has nobody but the
/// account (`organizations`) goes with it, with everything in it: its workspaces,
/// boards and wikis, their files, and its records. How much it holds (`boards`,
/// `wikis`, `files`) is told to a session that opens it, and left out for one of an
/// organization that takes only sessions made through its provider, and for the
/// account's own space where that takes only sessions made with the password
/// (`passwordRequired` of `getMyWorkspaces`). `proof` says what
/// `requestAccountDeletion` asks for.
///
/// Also answered to the session of an account that waits to be deleted, the one
/// operation besides `cancelAccountDeletion` and `logout` that such a session opens:
/// `requestedAt` and `deleteAfter` are there then, and left out otherwise.
#[utoipa::path(
    get,
    path = "/users/me/deletion",
    operation_id = "getAccountDeletion",
    tag = tag::USERS,
    responses((status = 200, description = "What deleting the account comes to.", body = AccountDeletion)),
)]
async fn handle_get_account_deletion(State(app): State<App>, account: AccountSession) -> ApiResult {
    let deletion = app
        .account_deletion(&account.session, account.leaving)
        .await?;
    Ok(json_response(&deletion))
}

/// Ask to delete the account
///
/// Puts the account of the session out of use, to be erased after thirty days
/// (`graceDays` of `getAccountDeletion`): every session of it ends, this one too, and
/// its cookie is cleared. Until it is erased, whoever signs in to the account gets a
/// session that opens nothing (every operation is 401 with the reason
/// `account_deletion_pending`) but `getAccountDeletion`, `cancelAccountDeletion` and
/// `logout`. While it waits, the account stays a member of what it is a member of,
/// shown as one, and what it wrote stays; the links of invitations it made let nobody
/// in, and it may wait past the thirty days until it is erased.
///
/// Erased, the account is gone from every organization, workspace, board and wiki; a
/// board it alone administered passes to an administrator of its workspace; nothing of
/// the person is kept, and what it wrote stays under its identifier, which
/// `getWorkspaceUsersById` answers with no name: the writer is a deleted user. An
/// organization that has nobody else goes with it, but the server's own first one,
/// whose only administrator stays as the only one of an organization with other
/// people does.
///
/// `username` is the account's login name, in any letter case: another is 400 naming
/// `/username`. An account with a password (`proof` is `password`) sends it: a wrong
/// one is 400 with the reason `invalid_credentials`, naming `/password`, and too many
/// are 429, as for `changePassword`. An account without one (`proof` is `sign_in`)
/// asks from a session made through a provider no more than five minutes ago:
/// otherwise 403 with the reason `sign_in_again`.
///
/// An organization that would go with the account and that takes only sessions made
/// through its provider is asked of such a session: any other is 401 with the reason
/// `sso_required` and its `orgId`. Leaving an organization is the account's own, and
/// asked of any session.
///
/// The server's only administrator stays (409, reason `last_admin`), and so does the
/// only administrator of an organization that stays when the account goes: one that
/// has other people in it, or the server's own first organization (409, reason
/// `last_org_admin`, `orgId` naming the first such organization): make somebody else
/// an administrator first.
#[utoipa::path(
    post,
    path = "/users/me/deletion",
    operation_id = "requestAccountDeletion",
    tag = tag::USERS,
    request_body = DeleteAccountRequest,
    responses(
        (
            status = 200,
            description = "The account waits to be erased.",
            body = AccountDeletionAsked,
            headers(("Set-Cookie" = String, description = "Clears the session cookie.")),
        ),
        (
            status = 429,
            description = "The password was wrong too often, or the server has too much password work to do at the moment.",
            body = Problem,
            content_type = "application/problem+json",
            headers(("Retry-After" = u64, description = "How many seconds to wait before trying again.")),
        ),
    ),
)]
async fn handle_request_account_deletion(
    State(app): State<App>,
    RequiredSession(session): RequiredSession,
    client: Option<Extension<ClientAddress>>,
    body: Bytes,
) -> ApiResult {
    let request: DeleteAccountRequest = decode_object(&body)?;
    request.validate()?;
    let asked: AccountDeletionAsked = app
        .request_account_deletion(&session, &request, address_of(client))
        .await?;
    let mut response = json_response(&asked);
    response.headers_mut().insert(
        SET_COOKIE,
        http_credentials::clear_cookie_header(app.authenticator().session_cookie()),
    );
    Ok(response)
}

/// Take back the asking to delete the account
///
/// Puts the account of the session in use again, as it was: for the session of an
/// account that waits to be deleted, which opens everything again from the next
/// request. A session of an account that does not wait is 409, and so is one whose
/// account is being erased already.
#[utoipa::path(
    delete,
    path = "/users/me/deletion",
    operation_id = "cancelAccountDeletion",
    tag = tag::USERS,
    responses((status = 204, description = "The account is in use again.")),
)]
async fn handle_cancel_account_deletion(
    State(app): State<App>,
    account: AccountSession,
) -> ApiResult {
    if account.leaving.is_none() {
        return Err(AppError::conflict(
            "the account does not wait to be deleted",
        ));
    }
    app.cancel_account_deletion(&account.session.user_id)
        .await?;
    Ok(no_content())
}
