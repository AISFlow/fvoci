//! The workspaces of a user, and the wall of the routes that name a workspace.
//!
//! A route under `/w/{workspaceId}` takes [`InWorkspace`]: the session's user as a member of the
//! workspace, with the application reaching the rows of the workspace's organization
//! and, of those, what is of this workspace (`App::in_workspace`): a board of another
//! workspace, and what is on it, is not there (404) as for a board that does not exist.
//! For anybody else the workspace is not there (404), whether it exists or not.

use axum::body::Bytes;
use axum::extract::rejection::RawPathParamsRejection;
use axum::extract::{FromRequestParts, Path, RawPathParams, State};
use axum::http::request::Parts;
use fvoci_model::{
    AuditRecord, Container, ContainerKinds, OrgKind, OrgRole, User, UserWorkspace, Workspace,
    WorkspaceRole,
};
use serde::Deserialize;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::contract::bodies::UserIds;
use super::contract::params::{
    MemberWorkspaceId, Page, PerPage, UserId, UserSearch, WorkerWorkspaceId, WorkspaceId,
};
use super::contract::tag;
use super::delegated::{Admits, NotAWall, Wall, sessions_only};
use super::query::{PagingQuery, QueryParams};
use super::session::caller_of;
use super::{ApiResult, MAX_USER_IDS, RequiredSession, decode_json, json_response};
use super::{Delegated, Read};

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
struct WorkspaceUsersQuery {
    search: String,
}
use crate::app::{App, Caller, MarkdownReader};
use crate::error::AppError;
use crate::permissions::{self, Actor};

/// The path parameter that names the workspace of a route.
const WORKSPACE_ID: &str = "workspaceId";

pub(super) fn routes() -> OpenApiRouter<App> {
    OpenApiRouter::new()
        .routes(routes!(handle_get_my_workspaces))
        .routes(routes!(handle_get_workspace_boards))
        .routes(routes!(handle_get_workspace))
        .routes(routes!(
            handle_get_workspace_users,
            handle_get_workspace_users_by_id
        ))
        .routes(routes!(handle_get_workspace_user))
        .routes(routes!(handle_get_workspace_audit))
}

/// The workspace a request names in its path, for a member of it, reaching its boards
/// and no wiki: an operation on wikis takes [`InWiki`], one on both kinds [`InContainer`].
///
/// Extraction fails with 401 without a session, and with 404 unless the session's user
/// is a member of the workspace: one that is another's and one that does not exist
/// answer alike. A member whose session does not open the workspace's organization,
/// which takes only sessions made through its provider, is told so (401, reason
/// `sso_required`): that is asked where the workspace is entered
/// (`App::enter_workspace`), of every request, before anything else is asked of the
/// member. The application it hands the handler is behind the wall between
/// organizations, reaching the rows of the workspace's organization and no other, and
/// finds a board of the organization's other workspaces as one that is not there
/// (`App::container_access`); a handler that lists asks for what is in this one by
/// `workspace_id`.
#[derive(Debug)]
pub(crate) struct InWorkspace {
    /// The application, reaching the rows of the workspace's organization.
    pub(crate) app: App,
    /// Who sends the request: by its session, or, on a route that takes one
    /// ([`super::Delegated`]), by a personal API token.
    pub(crate) caller: Caller,
    /// The workspace the path names.
    pub(crate) workspace_id: String,
    /// What the user is in the workspace.
    pub(crate) role: WorkspaceRole,
    /// What the user is in the workspace's organization, if a member of it.
    org_role: Option<OrgRole>,
    /// What the workspace's organization is.
    pub(crate) kind: OrgKind,
}

impl Wall for InWorkspace {
    async fn enter(parts: &mut Parts, app: &App, admits: Admits) -> Result<Self, AppError> {
        Self::extract(parts, app, admits).await
    }
}

impl InWorkspace {
    /// The caller as the reader of a body in Markdown in this workspace: the people it
    /// names are named as the caller sees them here.
    pub(crate) fn markdown_reader(&self) -> MarkdownReader<'_> {
        MarkdownReader {
            user_id: &self.caller.user_id,
            workspace_id: &self.workspace_id,
            role: self.role,
        }
    }

    /// The workspace of the path for the caller ([`caller_of`]).
    async fn extract(parts: &mut Parts, app: &App, admits: Admits) -> Result<Self, AppError> {
        let caller = caller_of(app, &parts.headers, &parts.extensions, admits).await?;
        let params = match RawPathParams::from_request_parts(parts, app).await {
            Ok(params) => params,
            // A workspace that is no text is none the user could be in (404); another
            // parameter that is no text (a board's, a card's) is a malformed request (400).
            Err(RawPathParamsRejection::InvalidUtf8InPathParam(error)) => {
                return Err(
                    if error.to_string().ends_with(&format!("`{WORKSPACE_ID}`")) {
                        workspace_not_found()
                    } else {
                        malformed_path()
                    },
                );
            }
            Err(_) => return Err(AppError::internal("the route has no path parameters")),
        };
        let workspace_id = params
            .iter()
            .find(|(name, _)| *name == WORKSPACE_ID)
            .map(|(_, value)| value.to_owned())
            .ok_or_else(|| AppError::internal("the route names no workspace"))?;
        Self::enter_as(app, caller, workspace_id).await
    }
}

impl InWorkspace {
    /// The workspace for the caller, as the extractor finds it: for a route that looks
    /// at something of the request (its body, its query) before it asks whether the
    /// workspace is there.
    ///
    /// # Errors
    ///
    /// Returns 404 unless the user is a member of the workspace (and, for a token, one
    /// it reaches), and the errors of `App::enter_workspace_as`.
    pub(crate) async fn enter_as(
        app: &App,
        caller: Caller,
        workspace_id: String,
    ) -> Result<Self, AppError> {
        let entered = app
            .enter_workspace_as(caller.asker(), &workspace_id)
            .await?
            .ok_or_else(workspace_not_found)?;
        let actor = Actor {
            org: entered.org_role,
            workspace: Some(entered.role),
        };
        Ok(Self {
            // What a route under a workspace is about is a board's, unless its extractor
            // says otherwise (`InWiki`, `InContainer`).
            app: entered
                .app
                .in_workspace_with_actor(&workspace_id, &caller.user_id, actor)
                .reaching(ContainerKinds::BOARDS),
            caller,
            workspace_id,
            role: entered.role,
            org_role: entered.org_role,
            kind: entered.kind,
        })
    }

    /// The same workspace, reaching boards of `kinds`.
    pub(crate) fn reaching(mut self, kinds: ContainerKinds) -> Self {
        self.app = self.app.reaching(kinds);
        self
    }

    /// What the user is in the workspace and its organization: the facts of what they
    /// may do there (`permissions`).
    pub(crate) fn actor(&self) -> Actor {
        Actor {
            org: self.org_role,
            workspace: Some(self.role),
        }
    }

    /// Refuses a guest of this workspace who asks to make a board.
    ///
    /// # Errors
    ///
    /// Returns 403 in `denial` for a guest; the workspace is there for them.
    pub(crate) fn require_container_maker(&self, denial: &str) -> Result<(), AppError> {
        if self.role == WorkspaceRole::Guest {
            return Err(AppError::forbidden(denial));
        }
        Ok(())
    }

    /// Puts the path's workspace into a board a request body makes: a body that names
    /// another is refused (400, naming `field`), one that names none or this one takes
    /// it.
    ///
    /// # Errors
    ///
    /// Returns 400 for a board that names another workspace.
    pub(crate) fn place_container(
        &self,
        container: &mut Container,
        field: &str,
    ) -> Result<(), AppError> {
        if !container.workspace_id.is_empty() && container.workspace_id != self.workspace_id {
            return Err(
                AppError::bad_request("the board is made in the workspace of the path")
                    .with_field(field),
            );
        }
        container.workspace_id.clone_from(&self.workspace_id);
        Ok(())
    }

    /// The workspace for one who works in it, a member who is no guest, in whatever
    /// organization it is: what the routes take that were made under `/w/` and never
    /// read their workspace anywhere else. A guest is told what anybody else is.
    ///
    /// # Errors
    ///
    /// Returns 404 for a guest.
    pub(crate) fn require_no_guest(self) -> Result<Self, AppError> {
        if self.role == WorkspaceRole::Guest {
            return Err(workspace_not_found());
        }
        Ok(self)
    }

    /// The workspace for one who works in it, where there are people to work with: the
    /// routes of invitations, of guests and of the groups people are put in. A user's
    /// own space has none of them, and its workspace is not there for these routes,
    /// for its owner as for anybody else.
    ///
    /// # Errors
    ///
    /// Returns 404 for the workspace of a user's own space, and for a guest.
    pub(crate) fn require_people(self) -> Result<Self, AppError> {
        if self.kind == OrgKind::Personal {
            return Err(workspace_not_found());
        }
        self.require_no_guest()
    }
}

/// Declares a wall a personal API token may be let through: a [`Wall`], which
/// [`super::Delegated`] wraps, entering the workspace as [`InWorkspace`] does and reaching
/// the containers of `$kinds` alone. On its own it takes a session alone
/// (`sessions_only!`).
macro_rules! delegable_wall {
    ($(#[$doc:meta])* $name:ident => $kinds:ident) => {
        $(#[$doc])*
        #[derive(Debug)]
        pub(crate) struct $name(pub(crate) InWorkspace);

        impl Wall for $name {
            async fn enter(parts: &mut Parts, app: &App, admits: Admits) -> Result<Self, AppError> {
                let ws = InWorkspace::enter(parts, app, admits).await?;
                Ok(Self(ws.reaching(ContainerKinds::$kinds)))
            }
        }

        sessions_only!($name);

        impl From<$name> for InWorkspace {
            fn from(wall: $name) -> Self {
                wall.0
            }
        }
    };
}

/// Declares a wall that takes a session alone, entering the workspace as
/// [`InWorkspace`] does and reaching the containers of `$kinds` alone; with `only`, a
/// workspace of an organization of another kind is not there. It is no [`Wall`], so
/// that no `Delegated` of it compiles: no token gets through it.
macro_rules! session_wall {
    ($(#[$doc:meta])* $name:ident => $kinds:ident $(, only $org:path)?) => {
        $(#[$doc])*
        #[derive(Debug)]
        pub(crate) struct $name(pub(crate) InWorkspace);

        impl FromRequestParts<App> for $name {
            type Rejection = AppError;

            async fn from_request_parts(
                parts: &mut Parts,
                app: &App,
            ) -> Result<Self, Self::Rejection> {
                let ws = InWorkspace::from_request_parts(parts, app).await?;
                $(if ws.kind != $org {
                    return Err(workspace_not_found());
                })?
                Ok(Self(ws.reaching(ContainerKinds::$kinds)))
            }
        }

        impl From<$name> for InWorkspace {
            fn from(wall: $name) -> Self {
                wall.0
            }
        }
    };
}

sessions_only!(InWorkspace);

delegable_wall!(
    /// A workspace as an operation on a wiki and its pages enters it: [`InWorkspace`]
    /// that reaches wikis and no board. A board named where a wiki is meant is not there.
    InWiki => WIKIS
);

session_wall!(
    /// A workspace as an operation on a diary and its entries enters it: [`InWorkspace`]
    /// that reaches diaries and nothing else, and only the workspace of the user's own
    /// space, which alone has diaries. Any other workspace is not there to it.
    InDiary => DIARIES, only OrgKind::Personal
);

delegable_wall!(
    /// A workspace as an operation on a course and its chapters and units enters it:
    /// [`InWorkspace`] that reaches courses and nothing else.
    InCourse => COURSES
);

session_wall!(
    /// A workspace as an operation enters it that is about the people of a shared
    /// container (members, joining, leaving, groups, invitations): [`InWorkspace`] that
    /// reaches boards, wikis and courses. A diary, which is its owner's alone, is not
    /// there to it.
    InShared => SHARED
);

delegable_wall!(
    /// A workspace as an operation enters it that reaches publicly linkable or
    /// referenceable containers: boards, wikis and courses. Diaries are never there to it.
    InLinked => LINKED
);

session_wall!(
    /// A workspace as an operation enters it that duplicates a container whole: boards,
    /// wikis and courses. Diaries are not there to it.
    InCopied => COPIED
);

// No `Wall` of these: no token gets through a wall that a diary is behind, or that
// works on the people of a container or copies it whole, however their declarations
// are changed.
const _: () = <InDiary as NotAWall<_>>::CHECKED;
const _: () = <InShared as NotAWall<_>>::CHECKED;
const _: () = <InCopied as NotAWall<_>>::CHECKED;

delegable_wall!(
    /// A workspace as an operation enters it that is the same for every container kind
    /// (the container itself, its files, its place in the sidebar and in the trash, what
    /// is searched and exported): [`InWorkspace`] that reaches every kind.
    InContainer => EVERY
);

fn workspace_not_found() -> AppError {
    AppError::not_found("workspace")
}

/// What the framework answers for a parameter of the path that is no text.
fn malformed_path() -> AppError {
    AppError::bad_request("the request is malformed")
}

/// The workspaces of the user
///
/// The workspaces the user of the session is a member of, in every organization, with
/// what the user is in each. By organization, the oldest first, and in each the oldest
/// workspace first; the one workspace of the user's own space (`kind` `personal`, see
/// "The user's own space") comes last, whenever it was made, and a client puts it where
/// it shows it. Every account has one, so the list is never empty. A workspace of an
/// organization that takes only sessions made
/// through its provider is listed for any session, with `ssoRequired` true for one that
/// was not made so: what is under it answers 401 with the reason `sso_required` until
/// the user signs in through the provider of `orgId`. The user's own space asks for no
/// provider: its `ssoRequired` is false. Once its account has a password it takes only
/// sessions that have shown that, and is listed with `passwordRequired` true for a
/// session made through a provider that has not: what is under it answers 401 with the
/// reason `password_sign_in_required` until the session gives the password
/// (`confirmPassword`), or the user signs in with it (`login`).
#[utoipa::path(
    get,
    path = "/users/me/workspaces",
    operation_id = "getMyWorkspaces",
    tag = tag::WORKSPACES,
    responses((status = 200, description = "The workspaces.", body = Vec<UserWorkspace>)),
)]
async fn handle_get_my_workspaces(
    State(app): State<App>,
    RequiredSession(session): RequiredSession,
) -> ApiResult {
    let workspaces = app.workspaces_of_user(&session).await?;
    Ok(json_response(&workspaces))
}

/// The boards of a workspace
///
/// The boards of the workspace that the user is a member of, and the workspace's open
/// boards; a guest of the workspace gets only the former, an administrator every board
/// of the workspace (what is on a private board they are no member of stays 404 to
/// them). Templates are not among them. Oldest first. 404 unless the user is a member of
/// the workspace.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/boards",
    operation_id = "getWorkspaceBoards",
    tag = tag::CONTAINERS,
    params(MemberWorkspaceId),
    responses((status = 200, description = "The boards.", body = Vec<Container>)),
)]
async fn handle_get_workspace_boards(Delegated(ws, ..): Delegated<Read, InWorkspace>) -> ApiResult {
    let boards = ws
        .app
        .workspace_containers(&ws.caller.user_id, &ws.workspace_id, ws.role)
        .await?;
    Ok(json_response(&boards))
}

/// A workspace
///
/// Its name, and whether the admins of its boards may invite guests to them.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}",
    operation_id = "getWorkspace",
    tag = tag::WORKSPACES,
    params(WorkspaceId),
    responses((status = 200, description = "The workspace.", body = Workspace)),
)]
async fn handle_get_workspace(Delegated(ws, ..): Delegated<Read, InWorkspace>) -> ApiResult {
    let ws = ws.require_no_guest()?;
    let workspace = ws.app.workspace(&ws.workspace_id).await?;
    Ok(json_response(&workspace))
}

/// The users of a workspace
///
/// Up to ten of the users the user sees in the workspace: whoever works there sees its
/// members and who is on a board of it with them, a guest only who is on a board of it
/// with them; and the user. With `search` those whose login name, display name or a
/// part of whose name (either way round, and the name they wrote) has that text in it,
/// whatever the case of the letters, without it the ten oldest accounts. Oldest account
/// first.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/users",
    operation_id = "getWorkspaceUsers",
    tag = tag::WORKSPACES,
    params(MemberWorkspaceId, UserSearch),
    responses((status = 200, description = "The users.", body = Vec<User>)),
)]
async fn handle_get_workspace_users(
    Delegated(ws, ..): Delegated<Read, InWorkspace>,
    QueryParams(query): QueryParams<WorkspaceUsersQuery>,
) -> ApiResult {
    Ok(json_response(
        &ws.app
            .search_users_in_workspace(&ws.caller.user_id, &ws.workspace_id, ws.role, &query.search)
            .await?,
    ))
}

/// Users of a workspace by their identifiers
///
/// The users with the identifiers of the body that the user sees in the workspace, each
/// once; the body names at least one and at most 200: an empty list, or a longer one,
/// is 400. An identifier of
/// somebody the user does not see finds nothing, as one of nobody: the answer does not
/// tell them apart. The user sees whom `getWorkspaceUsers` finds, and who created or
/// last changed a card, block or comment of a board of the workspace they read, also
/// after leaving the workspace. Oldest account first, not in the order of the request:
/// by `createdAt`, then by identifier.
///
/// An account that was deleted is answered to whoever asks by its identifier, seen
/// before or not, as what is left of it: the identifier, an empty `username`, `email`
/// and `displayName`, every part of the name empty, no title nor credentials,
/// `nameShown` `name`, `createdAt` and `updatedAt` both zero, and `deletedAt` absent: no
/// time of the account is answered, neither when it went nor when it was asked to (the
/// identifier is kept as it was made). The empty `username` is what tells it, not a
/// time: a page shows a deleted user where it wrote or is mentioned.
/// `deletedAt` is present for a writer whose account waits to be deleted, who is
/// answered as they are, with their name.
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/users",
    operation_id = "getWorkspaceUsersById",
    tag = tag::WORKSPACES,
    params(MemberWorkspaceId),
    request_body = UserIds,
    responses((status = 200, description = "The users.", body = Vec<User>)),
)]
async fn handle_get_workspace_users_by_id(
    Delegated(ws, ..): Delegated<Read, InWorkspace>,
    body: Bytes,
) -> ApiResult {
    let user_ids: Vec<String> = decode_json(&body)?;
    if user_ids.is_empty() {
        return Err(AppError::bad_request("User IDs are empty"));
    }
    if user_ids.len() > MAX_USER_IDS {
        return Err(AppError::bad_request(format_args!(
            "at most {MAX_USER_IDS} user IDs are asked for at once"
        )));
    }
    let users = ws
        .app
        .users_in_workspace(&ws.caller.user_id, &ws.workspace_id, ws.role, &user_ids)
        .await?;
    Ok(json_response(&users))
}

/// A user of a workspace
///
/// A user the user sees in the workspace, as `getWorkspaceUsersById` finds them; anybody
/// else is 404, as one who does not exist.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/users/{userId}",
    operation_id = "getWorkspaceUser",
    tag = tag::WORKSPACES,
    params(MemberWorkspaceId, UserId),
    responses((status = 200, description = "The user.", body = User)),
)]
async fn handle_get_workspace_user(
    Delegated(ws, ..): Delegated<Read, InWorkspace>,
    Path((_, user_id)): Path<(String, String)>,
) -> ApiResult {
    let mut user = ws
        .app
        .users_in_workspace(
            &ws.caller.user_id,
            &ws.workspace_id,
            ws.role,
            std::slice::from_ref(&user_id),
        )
        .await?
        .pop()
        .ok_or_else(|| AppError::not_found("user"))?;
    if user_id == ws.caller.user_id {
        crate::app::sanitize(&mut user);
    } else {
        ws.app.sanitize_profile(&mut user);
    }
    Ok(json_response(&user))
}

/// The audit log of a workspace
///
/// One page of what was done in the workspace that its members are to know of, the
/// newest first: an administrator of the organization joining it as its administrator,
/// leaving it, or being taken out of it when they stopped administering the
/// organization; and an administrator of the workspace who could not read a board
/// giving somebody (themselves or another) a role on it. A board's title is there where
/// the reader sees the board. For whoever works in the workspace (a member who is no
/// guest), of any organization. The records are never changed or removed but with the workspace, when it is
/// removed for good.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/audit",
    operation_id = "getWorkspaceAudit",
    tag = tag::WORKSPACES,
    params(WorkerWorkspaceId, Page, PerPage),
    responses((status = 200, description = "The records of the page.", body = Vec<AuditRecord>)),
)]
async fn handle_get_workspace_audit(
    ws: InWorkspace,
    QueryParams(query): QueryParams<PagingQuery>,
) -> ApiResult {
    if !permissions::reads_the_audit_log(Some(ws.role)) {
        return Err(workspace_not_found());
    }
    let paging = query.into_list_page()?;
    Ok(json_response(
        &ws.app
            .workspace_audit(&ws.caller.user_id, &ws.workspace_id, paging)
            .await?,
    ))
}
