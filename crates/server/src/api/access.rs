//! Checks that many handlers share: who may do what on a board or in a workspace.
//!
//! Every check of a board asks `App::reach` once and decides by the `Reached` it gives:
//! these are the checks of the routes that ask one question of a board, and the routes
//! that ask more keep the `Reached` and ask it.

use fvoci_model::Workspace;

use crate::app::App;
pub(crate) use crate::app::{BlockAction, refusal, require_granted};
use crate::error::{AppError, Reason};
use crate::permissions::ContainerPermission;

/// Checks that the user is an administrator of the instance.
///
/// Who is one is no secret, and neither is that these routes exist: anybody else is
/// refused (403, reason `admin_required`). A route under a workspace checks first that the
/// workspace exists, as every workspace route does, so there the refusal is of a real workspace.
pub(crate) async fn require_admin(app: &App, user_id: &str) -> Result<(), AppError> {
    if app.user(user_id).await?.is_admin {
        Ok(())
    } else {
        Err(
            AppError::forbidden("only an administrator of the instance may do this")
                .with_reason(Reason::AdminRequired),
        )
    }
}

/// Checks that the user has the permission on the board that the request names in its
/// path (`Reached::require`).
///
/// What a user without the permission is told depends on what they may know. Who sees
/// the board (`Grant::sees`) is refused (403, `denial`); for anybody else the board does
/// not exist (404), whether it does or not.
pub(crate) async fn require_container_permission(
    app: &App,
    user_id: &str,
    container_id: &str,
    permission: ContainerPermission,
    denial: &str,
) -> Result<(), AppError> {
    app.reach(user_id, container_id)
        .await?
        .require(permission, denial)?;
    Ok(())
}

/// Checks that the user has the permission on a board that the request does not name
/// itself but reaches through `what`, such as a block or a card of the board
/// (`Reached::require_through`).
///
/// A user who may not read the board's contents is told that `what` does not exist
/// (404): the identifier in the request says nothing about a board, so not even an open
/// board is given away. A reader without the permission is refused (403, `denial`).
pub(crate) async fn require_permission_through(
    app: &App,
    user_id: &str,
    container_id: &str,
    permission: ContainerPermission,
    denial: &str,
    what: &str,
) -> Result<(), AppError> {
    app.reach(user_id, container_id)
        .await?
        .require_through(permission, denial, what)?;
    Ok(())
}

/// Checks that the user sees the board at all (`Grant::sees`). For anybody else it does
/// not exist (404).
pub(crate) async fn require_container_visible(
    app: &App,
    user_id: &str,
    container_id: &str,
) -> Result<(), AppError> {
    app.reach(user_id, container_id).await?.require_visible()
}

/// The workspace, or a 404 unless the user works in it (`App::works_in_workspace`): a workspace
/// the user is no member of, and a workspace of another organization, answer as one that
/// does not exist.
///
/// For the copy of a board, whose query names the workspace it goes to besides the one of
/// the path: it asks the same fact the routes under a workspace ask (`api/workspace.rs`,
/// `InWorkspace::require_no_guest`).
pub(crate) async fn require_workspace_access(
    app: &App,
    user_id: &str,
    workspace_id: &str,
) -> Result<Workspace, AppError> {
    if !app.works_in_workspace(user_id, workspace_id).await? {
        return Err(AppError::not_found("workspace"));
    }
    app.workspace(workspace_id).await
}
