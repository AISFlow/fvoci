//! Users: the signed-in user, other users' profiles, settings, and client state.

use axum::body::Bytes;
use axum::extract::{Extension, Path, State};
use fvoci_model::InstanceRole;
use fvoci_model::{
    ClientStatePatch, ContainerMember, LinkedIdentity, Me, PersonNameChange, Problem,
    UnlinkRequest, User, UserOrganization, UserSettings, UserSettingsPatch,
};
use serde::Deserialize;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::contract::bodies::{AccountChange, AdminChange, SettingsChange};
use super::contract::params::UserId;
use super::contract::tag;

use super::auth::address_of;
use super::{
    ApiResult, RequiredSession, decode_json, decode_object, json_object, json_response, no_content,
    require_admin,
};
use crate::app::App;
use crate::client_address::ClientAddress;
use crate::error::AppError;

pub(super) fn routes() -> OpenApiRouter<App> {
    OpenApiRouter::new()
        .routes(routes!(handle_get_me, handle_patch_me))
        .routes(routes!(handle_get_my_memberships))
        .routes(routes!(handle_get_my_organizations))
        .routes(routes!(handle_get_client_state, handle_patch_client_state))
        .routes(routes!(
            handle_get_user_settings,
            handle_patch_user_settings
        ))
        .routes(routes!(handle_get_my_identities, handle_unlink_identity))
        .routes(routes!(handle_set_user_admin))
}

/// What a request misses that names a user who is not there.
const USER: &str = "user";

/// Says which kind of thing was not found, for a lookup of a user.
fn user_not_found(error: AppError) -> AppError {
    if error.is_not_found() {
        AppError::not_found(USER)
    } else {
        error
    }
}

/// The user of the session
///
/// The user as anybody who sees them is answered them, and what only they are told of
/// their account: its email address (`email`), the one it waits to have confirmed
/// (`pendingEmail`, see `requestEmailChange`), and whether it has a password
/// (`hasPassword`). An account made by a sign-in through an organization's provider
/// has none, and is reached through that provider only, its own space too, until its
/// person sets one (`setPassword`).
#[utoipa::path(
    get,
    path = "/users/me",
    operation_id = "getMe",
    tag = tag::USERS,
    responses((status = 200, description = "The user.", body = Me)),
)]
async fn handle_get_me(
    State(app): State<App>,
    RequiredSession(session): RequiredSession,
) -> ApiResult {
    let me = app.me(&session.user_id).await.map_err(user_not_found)?;
    Ok(json_response(&me))
}

/// Change the user's account
///
/// Changes what the body names of the user of the session's name: its parts
/// (`firstName`, `lastName`, `latinFirstName`, `latinLastName`), the name they wrote to
/// be shown by (`customName`), which of them they are shown by (`nameShown`), the title
/// and the credentials. White space around a part is dropped, and empty or `null` is
/// none. A part has at most 64 UTF-16 code units, without control characters and
/// without characters that show nothing, break a line or turn the text around (U+00AD,
/// U+034F, U+061C, U+115F, U+1160, U+180E, U+200B–U+200F, U+2028–U+202E, U+2060–U+206F,
/// U+2800, U+3164, U+FEFF, U+FFA0, U+FFF9–U+FFFB). `title` is a
/// code of its list or `null`; `credentials` at most three codes of their list, each
/// once, in the order they are written, or `null` for none. `displayName` follows: the
/// server makes it from these (`contracts/name-vectors.json`). A member that is no such
/// thing, or a value that breaks these rules, is refused (400 naming it) and changes
/// nothing. Answers the user as `getMe` does.
#[utoipa::path(
    patch,
    path = "/users/me",
    operation_id = "patchMe",
    tag = tag::USERS,
    request_body = AccountChange,
    responses((status = 200, description = "The user as they are now.", body = User)),
)]
async fn handle_patch_me(
    State(app): State<App>,
    RequiredSession(session): RequiredSession,
    body: Bytes,
) -> ApiResult {
    let members = json_object(decode_json(&body)?)?;
    let change = PersonNameChange::from_members(&members)?;
    let mut user = if change.is_empty() {
        app.user(&session.user_id).await?
    } else {
        app.change_person_name(&session.user_id, &change).await?
    };
    crate::app::sanitize(&mut user);
    Ok(json_response(&user))
}

/// The boards the user is a member of
///
/// The user's memberships, with the roles of each, of the boards of the workspaces the
/// user is a member of, in every organization the session opens: one that takes only
/// sessions made through its provider leaves its boards out of another session's list,
/// and the user's own space, once the account has a password, out of the list of a
/// session made through a provider.
/// A board the user has a role on only through a team of its workspace is listed too
/// (`synthetic`, with the team's role in `inheritedRole`); a guest is in no team. A
/// wiki the user is on is listed as a board is, under its identifier as `containerId`. By
/// the identifier of the board.
#[utoipa::path(
    get,
    path = "/users/me/memberships",
    operation_id = "getMyMemberships",
    tag = tag::USERS,
    responses((status = 200, description = "The memberships.", body = Vec<ContainerMember>)),
)]
async fn handle_get_my_memberships(
    State(app): State<App>,
    RequiredSession(session): RequiredSession,
) -> ApiResult {
    let members = app.container_memberships(&session).await?;
    Ok(json_response(&members))
}

/// The organizations the user is a member of
///
/// The organizations that people work in together of which the user of the session is a
/// member, with their name and the user's role in each. Includes organizations in which
/// the user is in no workspace, and leaves out the user's own space and organizations
/// where the user is only a guest of a workspace. Asked of any session: an organization
/// that takes only sessions made through its provider is still listed so the user can
/// see it in settings and leave it (`leaveOrganization`). Oldest first.
#[utoipa::path(
    get,
    path = "/users/me/organizations",
    operation_id = "getMyOrganizations",
    tag = tag::USERS,
    responses((status = 200, description = "The organizations.", body = Vec<UserOrganization>)),
)]
async fn handle_get_my_organizations(
    State(app): State<App>,
    RequiredSession(session): RequiredSession,
) -> ApiResult {
    let organizations = app.organizations_of_user(&session.user_id).await?;
    Ok(json_response(&organizations))
}

/// The identities linked to the account
///
/// Who the user of the session is at the providers they linked to the account, or
/// first signed in through: an organization's provider with that issuer signs in to
/// this account. Linked by `startOidcLogin` with `linkPassword`. Oldest first, each
/// saying whether this session was made through a provider of its issuer.
#[utoipa::path(
    get,
    path = "/users/me/identities",
    operation_id = "getMyIdentities",
    tag = tag::AUTH,
    responses((status = 200, description = "The identities.", body = Vec<LinkedIdentity>)),
)]
async fn handle_get_my_identities(
    State(app): State<App>,
    RequiredSession(session): RequiredSession,
) -> ApiResult {
    Ok(json_response(&app.identities(&session).await?))
}

/// Remove an identity from the account
///
/// Removes the identity of `issuer` and `subject` from the account of the session: the
/// providers of that issuer no longer sign in to it. Asks for the account's password,
/// as `changePassword` does: a wrong one is 400 with the reason `invalid_credentials`,
/// naming `/password`, and too many are 429. An identity that is not the account's is
/// 404. An account without a password keeps its last identity, which is all that lets
/// its owner in: 409 with the reason `last_sign_in`; and has no password to confirm
/// removing another with: 409 with the reason `password_not_set`. The account's
/// sessions made through a provider of that issuer end, and their connections close:
/// this session too when it was one (`madeThisSession` of the identity).
#[utoipa::path(
    delete,
    path = "/users/me/identities",
    operation_id = "unlinkIdentity",
    tag = tag::AUTH,
    request_body = UnlinkRequest,
    responses(
        (status = 204, description = "The identity is removed."),
        (
            status = 429,
            description = "The password was wrong too often, or the server has too much password work to do at the moment.",
            body = Problem,
            content_type = "application/problem+json",
            headers(("Retry-After" = u64, description = "How many seconds to wait before trying again.")),
        ),
    ),
)]
async fn handle_unlink_identity(
    State(app): State<App>,
    RequiredSession(session): RequiredSession,
    client: Option<Extension<ClientAddress>>,
    body: Bytes,
) -> ApiResult {
    let request: UnlinkRequest = decode_object(&body)?;
    request.validate()?;
    app.unlink_identity(&session, &request, address_of(client))
        .await?;
    Ok(no_content())
}

/// A user
///
/// The body of `PUT /users/{userId}/admin`.
#[derive(Deserialize)]
struct AdminFlag {
    #[serde(rename = "isAdmin")]
    is_admin: Option<bool>,
}

/// Make a user an administrator, or end that
///
/// Makes a user an administrator of the instance, or ends that. For administrators;
/// anybody else is refused with 403 and the reason `admin_required`. The last
/// administrator cannot stop being one, which is 409 with the reason `last_admin`:
/// handing the role over is two requests, first making the other one an administrator.
/// A body without `isAdmin` is 400.
#[utoipa::path(
    put,
    path = "/users/{userId}/admin",
    operation_id = "setUserAdmin",
    tag = tag::USERS,
    params(UserId),
    request_body = AdminChange,
    responses((status = 200, description = "The user as they are now.", body = User)),
)]
async fn handle_set_user_admin(
    State(app): State<App>,
    RequiredSession(session): RequiredSession,
    Path(user_id): Path<String>,
    body: Bytes,
) -> ApiResult {
    // Who administers the instance administers its own organization too.
    app.require_session_opens_instance(&session).await?;
    require_admin(&app, &session.user_id).await?;
    let flag: AdminFlag = decode_object(&body)?;
    let role = match flag.is_admin {
        Some(true) => InstanceRole::Administrator,
        Some(false) => InstanceRole::User,
        None => {
            return Err(
                AppError::bad_request("isAdmin must be true or false").with_field("isAdmin")
            );
        }
    };

    let mut user = app.set_user_admin(&user_id, role, &session.user_id).await?;
    if user.id == session.user_id {
        crate::app::sanitize(&mut user);
    } else {
        app.sanitize_profile(&mut user);
    }
    Ok(json_response(&user))
}

/// Change the client's state
///
/// Sets and removes entries of the state the client keeps with the account. The
/// server does not know what the entries mean; it bounds their number, their names and
/// their size.
#[utoipa::path(
    patch,
    path = "/users/me/client-state",
    operation_id = "patchClientState",
    tag = tag::USERS,
    request_body = ClientStatePatch,
    responses((
        status = 200,
        description = "All of the state as it is now: the value of every entry by its name.",
        body = BTreeMap<String, String>,
    )),
)]
async fn handle_patch_client_state(
    State(app): State<App>,
    RequiredSession(session): RequiredSession,
    body: Bytes,
) -> ApiResult {
    let patch: ClientStatePatch = decode_object(&body)?;
    patch.validate()?;
    let entries = app.patch_client_state(&session.user_id, &patch).await?;
    Ok(json_response(&entries))
}

/// The user's settings
///
/// What the user has set about how things are shown to them. Every setting is
/// answered; what was never set has its default.
#[utoipa::path(
    get,
    path = "/users/me/settings",
    operation_id = "getUserSettings",
    tag = tag::USERS,
    responses((status = 200, description = "The settings.", body = UserSettings)),
)]
async fn handle_get_user_settings(
    State(app): State<App>,
    RequiredSession(session): RequiredSession,
) -> ApiResult {
    Ok(json_response(&app.user_settings(&session.user_id).await?))
}

/// Change the user's settings
///
/// Changes the settings the body names. A member that is `null` puts its setting back
/// to the default. A member that is no setting, or a value its setting cannot have, is
/// refused and changes nothing.
#[utoipa::path(
    patch,
    path = "/users/me/settings",
    operation_id = "patchUserSettings",
    tag = tag::USERS,
    request_body = SettingsChange,
    responses((status = 200, description = "All settings as they are now.", body = UserSettings)),
)]
async fn handle_patch_user_settings(
    State(app): State<App>,
    RequiredSession(session): RequiredSession,
    body: Bytes,
) -> ApiResult {
    let members = json_object(decode_json(&body)?)?;
    let patch = UserSettingsPatch::from_members(&members)?;
    Ok(json_response(
        &app.patch_user_settings(&session.user_id, &patch).await?,
    ))
}

/// The client's state
///
/// The state the client keeps with the account.
#[utoipa::path(
    get,
    path = "/users/me/client-state",
    operation_id = "getClientState",
    tag = tag::USERS,
    responses((
        status = 200,
        description = "The value of every entry by its name.",
        body = BTreeMap<String, String>,
    )),
)]
async fn handle_get_client_state(
    State(app): State<App>,
    RequiredSession(session): RequiredSession,
) -> ApiResult {
    let entries = app.client_state(&session.user_id).await?;
    Ok(json_response(&entries))
}
