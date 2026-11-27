//! Sidebar categories and the boards filed under them.

use axum::body::Bytes;
use axum::extract::Path;
use fvoci_model::{
    Category, CategoryContainerUpdate, CategoryContainers, CategoryError, CategoryPatch,
    CategoryType, RefusalReason, SidebarVisibility, WorkspaceRole,
};
use serde_json::Value;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::contract::bodies::{CategoryUpdate, NewCategory};
use super::contract::params::{CategoryId, ContainerId, MemberWorkspaceId, WorkspaceId};
use super::contract::tag;

use super::workspace::{InContainer, InWorkspace};
use super::{
    ApiResult, decode_json, decode_object, json_object, json_response, no_content,
    require_container_visible,
};
use crate::app::App;
use crate::auth::now_millis;
use crate::error::AppError;

pub(super) fn routes() -> OpenApiRouter<App> {
    OpenApiRouter::new()
        .routes(routes!(
            handle_create_category,
            handle_get_user_category_containers
        ))
        .routes(routes!(handle_reorder_categories))
        .routes(routes!(
            handle_update_category,
            handle_patch_category,
            handle_delete_category
        ))
        .routes(routes!(handle_reorder_category_containers))
        .routes(routes!(
            handle_update_category_container,
            handle_put_category_container
        ))
        .routes(routes!(handle_hide_container))
        .routes(routes!(handle_unhide_container))
}

/// The path parameters of the routes that address a board in a category: the workspace, the
/// category and the board.
type ContainerPlace = (String, String, String);

/// A category as a request body carries it.
struct CategoryRequest {
    category: Category,
    /// Whether the body's `type`, if it has one, is one a category can have. Reported
    /// with the category's other refusals, see [`CategoryRequest::into_category`].
    type_is_valid: bool,
}

impl CategoryRequest {
    /// Decodes a category: a member that is missing leaves the default. A `type` that
    /// is no type of a category, blank included, is refused by
    /// [`CategoryRequest::into_category`]. A body that is no JSON object, `null`
    /// included, or whose members have the wrong types (`null` among them) is a bad
    /// request.
    fn decode(body: &[u8]) -> Result<Self, AppError> {
        let mut members = json_object(decode_json(body)?)?;
        let type_is_valid = match members.get("type") {
            Some(Value::String(text)) => is_category_type(text),
            _ => true,
        };
        if !type_is_valid {
            members.remove("type");
        }
        let category =
            serde_json::from_value(Value::Object(members)).map_err(AppError::bad_request)?;
        Ok(Self {
            category,
            type_is_valid,
        })
    }

    /// The category, if its type is valid. If not, the bad request names every member
    /// that is refused: the type, and whichever of the identifier, the name, the user
    /// and the workspace is blank.
    fn into_category(self) -> Result<Category, AppError> {
        let mut category = self.category;
        if self.type_is_valid {
            return Ok(category);
        }
        category.hydrate(now_millis());
        let mut refused = category
            .validate()
            .err()
            .map(CategoryError::into_refusals)
            .unwrap_or_default();
        refused.refuse(
            "type",
            RefusalReason::Invalid,
            format!(
                "type must be {:?} or {:?}",
                CategoryType::System.as_str(),
                CategoryType::Custom.as_str(),
            ),
        );
        Err(refused.into())
    }
}

fn is_category_type(text: &str) -> bool {
    [CategoryType::System, CategoryType::Custom]
        .iter()
        .any(|category_type| category_type.as_str() == text)
}

/// Decodes the list of identifiers of a reorder request.
fn decode_order(body: &[u8]) -> Result<Vec<String>, AppError> {
    decode_json(body)
}

/// Make a category
///
/// Makes a category of the user's sidebar in the workspace. The server makes its
/// identifier, and a request that names one is 400. A `userId` that is not the user of
/// the session is 403. The category is stored with its `sortOrder` raised by 10, and
/// every other category of the user in the workspace moves by 10 as well, so that it keeps
/// its place among them.
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/categories",
    operation_id = "createCategory",
    tag = tag::SIDEBAR,
    params(WorkspaceId),
    request_body = NewCategory,
    responses((status = 201, description = "The category that was made.", body = Category)),
)]
async fn handle_create_category(InContainer(ws): InContainer, body: Bytes) -> ApiResult {
    let ws = ws.require_no_guest()?;
    // The workspace of the path comes first (the extractor): what is asked of a
    // workspace that is not there is not looked at.
    let InWorkspace {
        app,
        caller,
        workspace_id,
        ..
    } = ws;
    let request = CategoryRequest::decode(&body)?;
    let category = &request.category;
    if category.user_id != caller.user_id {
        return Err(
            AppError::forbidden("a user makes categories for themselves only").with_field("userId"),
        );
    }
    // The server makes the identifier. One chosen by the client would have to be
    // checked against everybody's categories, and the answer to that check would say
    // whether somebody has a category of that identifier.
    if !category.id.is_empty() {
        return Err(AppError::bad_request("a new category cannot have an ID").with_field("id"));
    }
    if category.workspace_id != workspace_id {
        return Err(AppError::bad_request("workspaceId mismatch").with_field("workspaceId"));
    }

    let created = app.create_category(request.into_category()?).await?;
    Ok(super::created(&created))
}

/// Change a category
///
/// Changes the name of one of the user's categories and whether it is collapsed.
/// Nothing else changes, and a system category keeps its name. A `userId` that is not
/// the user of the session is 400.
#[utoipa::path(
    put,
    path = "/w/{workspaceId}/categories/{categoryId}",
    operation_id = "updateCategory",
    tag = tag::SIDEBAR,
    params(WorkspaceId, CategoryId),
    request_body = CategoryUpdate,
    responses((status = 200, description = "The category as it is now.", body = Category)),
)]
async fn handle_update_category(
    InContainer(ws): InContainer,
    Path((_, category_id)): Path<(String, String)>,
    body: Bytes,
) -> ApiResult {
    let ws = ws.require_no_guest()?;
    let InWorkspace {
        app,
        caller,
        workspace_id,
        ..
    } = ws;
    // Before the body is looked at: another user's category does not exist for this one.
    app.user_category(&workspace_id, &caller.user_id, &category_id)
        .await?;
    let request = CategoryRequest::decode(&body)?;
    let category = &request.category;
    if category.id != category_id {
        return Err(
            AppError::bad_request("the category of the body is not the one of the path")
                .with_field("id"),
        );
    }
    if category.user_id != caller.user_id {
        return Err(
            AppError::bad_request("the user of the body is not the session's").with_field("userId"),
        );
    }
    if category.workspace_id != workspace_id {
        return Err(AppError::bad_request("workspaceId mismatch").with_field("workspaceId"));
    }

    let updated = app.update_category(request.into_category()?).await?;
    Ok(json_response(&updated))
}

/// Partially update a category
///
/// Updates the display `name` and/or `collapsed` state of one of the user's categories
/// in a single transaction. System categories ignore name changes.
#[utoipa::path(
    patch,
    path = "/w/{workspaceId}/categories/{categoryId}",
    operation_id = "patchCategory",
    tag = tag::SIDEBAR,
    params(WorkspaceId, CategoryId),
    request_body = CategoryPatch,
    responses((status = 200, description = "The updated category.", body = Category)),
)]
async fn handle_patch_category(
    InContainer(ws): InContainer,
    Path((_, category_id)): Path<(String, String)>,
    body: Bytes,
) -> ApiResult {
    let ws = ws.require_no_guest()?;
    let patch: CategoryPatch = decode_object(&body)?;
    let updated = ws
        .app
        .patch_category(&ws.workspace_id, &ws.caller.user_id, &category_id, &patch)
        .await?;
    Ok(json_response(&updated))
}

/// Delete a category
///
/// Deletes one of the user's categories; its boards go to the user's default
/// category. A system category cannot be deleted. A deleted category does not come
/// back.
#[utoipa::path(
    delete,
    path = "/w/{workspaceId}/categories/{categoryId}",
    operation_id = "deleteCategory",
    tag = tag::SIDEBAR,
    params(WorkspaceId, CategoryId),
    responses((status = 204, description = "The category is deleted.")),
)]
async fn handle_delete_category(
    InContainer(ws): InContainer,
    Path((_, category_id)): Path<(String, String)>,
) -> ApiResult {
    let ws = ws.require_no_guest()?;
    ws.app
        .delete_category(&category_id, &ws.caller.user_id, &ws.workspace_id)
        .await?;
    Ok(no_content())
}

/// The user's sidebar
///
/// The user's categories in the workspace, each with the boards filed under it. A user who
/// has none yet gets the default category made. A guest of the workspace has theirs
/// too, with the boards they are on and no other, whatever they filed before; the
/// routes that change the sidebar are for who works in the workspace.
///
/// The categories come by `sortOrder`, lowest first, and among those with the same one
/// the oldest first. The boards of a category come in the order the user gave them,
/// and by identifier among those with the same place.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/categories",
    operation_id = "getCategories",
    tag = tag::SIDEBAR,
    params(MemberWorkspaceId),
    responses((
        status = 200,
        description = "The categories with their boards.",
        body = Vec<CategoryContainers>,
    )),
)]
async fn handle_get_user_category_containers(InContainer(ws): InContainer) -> ApiResult {
    // A guest too: their sidebar has the boards they are on, filed as they have filed
    // them. The rest of the sidebar's routes are for who works there.
    let (user_id, workspace_id) = (&ws.caller.user_id, &ws.workspace_id);
    let category_containers = if ws.role == WorkspaceRole::Guest {
        ws.app
            .category_containers_of_guest(user_id, workspace_id)
            .await?
    } else {
        ws.app
            .user_category_containers(user_id, workspace_id)
            .await?
    };
    Ok(json_response(&category_containers))
}

/// File a board under a category
///
/// Puts a board the user may see under one of the user's categories; it leaves the
/// category it was under. A category takes the boards of its own workspace and the
/// built-in templates: a board of another workspace answers as one that is not there.
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/categories/{categoryId}/containers/{containerId}",
    operation_id = "setContainerCategory",
    tag = tag::SIDEBAR,
    params(WorkspaceId, CategoryId, ContainerId),
    responses((status = 204, description = "The board is under the category.")),
)]
async fn handle_update_category_container(
    InContainer(ws): InContainer,
    Path((_, category_id, container_id)): Path<ContainerPlace>,
) -> ApiResult {
    let ws = ws.require_no_guest()?;
    let InWorkspace {
        app,
        caller,
        workspace_id,
        ..
    } = ws;
    app.user_category(&workspace_id, &caller.user_id, &category_id)
        .await?;
    require_container_visible(&app, &caller.user_id, &container_id).await?;
    app.add_update_user_category_container(
        &workspace_id,
        &caller.user_id,
        &category_id,
        &[container_id],
    )
    .await?;
    Ok(no_content())
}

/// Move and/or hide a container in a category atomically
///
/// Moves the container into `categoryId` at optional 0-based `index` and/or sets its
/// `hidden` flag in one database transaction.
#[utoipa::path(
    put,
    path = "/w/{workspaceId}/categories/{categoryId}/containers/{containerId}",
    operation_id = "putCategoryContainer",
    tag = tag::SIDEBAR,
    params(WorkspaceId, CategoryId, ContainerId),
    request_body = CategoryContainerUpdate,
    responses((status = 204, description = "The container placement and visibility are updated.")),
)]
async fn handle_put_category_container(
    InContainer(ws): InContainer,
    Path((_, category_id, container_id)): Path<ContainerPlace>,
    body: Bytes,
) -> ApiResult {
    let ws = ws.require_no_guest()?;
    let InWorkspace {
        app,
        caller,
        workspace_id,
        ..
    } = ws;
    app.user_category(&workspace_id, &caller.user_id, &category_id)
        .await?;
    require_container_visible(&app, &caller.user_id, &container_id).await?;
    let update: CategoryContainerUpdate = decode_object(&body)?;
    app.put_category_container(
        &workspace_id,
        &caller.user_id,
        &category_id,
        &container_id,
        &update,
    )
    .await?;
    Ok(no_content())
}

/// Put the categories in an order
///
/// The body lists the identifiers of all of the user's categories in the workspace, in the
/// order they are to have. A list that is not as long as the user has categories, or
/// that names one that is not the user's, is refused.
#[utoipa::path(
    put,
    path = "/w/{workspaceId}/category-order",
    operation_id = "reorderCategories",
    tag = tag::SIDEBAR,
    params(WorkspaceId),
    request_body = Vec<String>,
    responses((
        status = 200,
        description = "The identifiers of the categories in their new order.",
        body = Vec<String>,
    )),
)]
async fn handle_reorder_categories(InContainer(ws): InContainer, body: Bytes) -> ApiResult {
    let ws = ws.require_no_guest()?;
    let new_order = decode_order(&body)?;
    let order = ws
        .app
        .reorder_categories(&ws.caller.user_id, &ws.workspace_id, new_order)
        .await?;
    Ok(json_response(&order))
}

/// Put the boards of a category in an order
///
/// The body lists the identifiers of all boards of the category, in the order they
/// are to have. A list that is not as long as the category has boards, or that names
/// one that is not in the category, is refused.
#[utoipa::path(
    put,
    path = "/w/{workspaceId}/categories/{categoryId}/container-order",
    operation_id = "reorderCategoryContainers",
    tag = tag::SIDEBAR,
    params(WorkspaceId, CategoryId),
    request_body = Vec<String>,
    responses((
        status = 200,
        description = "The identifiers of the boards in their new order.",
        body = Vec<String>,
    )),
)]
async fn handle_reorder_category_containers(
    InContainer(ws): InContainer,
    Path((_, category_id)): Path<(String, String)>,
    body: Bytes,
) -> ApiResult {
    let ws = ws.require_no_guest()?;
    let InWorkspace {
        app,
        caller,
        workspace_id,
        ..
    } = ws;
    app.user_category(&workspace_id, &caller.user_id, &category_id)
        .await?;
    let new_order = decode_order(&body)?;
    let order = app
        .reorder_category_containers(&caller.user_id, &workspace_id, &category_id, new_order)
        .await?;
    Ok(json_response(&order))
}

/// Hide a board in the sidebar
///
/// Hides a board of one of the user's categories from the user's sidebar. The board
/// stays where it is filed, and nothing changes for anybody else. For a board that is
/// not filed under the category nothing changes, and the answer is 204 all the same; a
/// board of another workspace than the category's answers as one that is not there.
#[utoipa::path(
    put,
    path = "/w/{workspaceId}/categories/{categoryId}/containers/{containerId}/hide",
    operation_id = "hideContainer",
    tag = tag::SIDEBAR,
    params(WorkspaceId, CategoryId, ContainerId),
    responses((status = 204, description = "The board is hidden.")),
)]
async fn handle_hide_container(
    InContainer(ws): InContainer,
    Path(place): Path<ContainerPlace>,
) -> ApiResult {
    let ws = ws.require_no_guest()?;
    set_sidebar_visibility(&ws, &place, SidebarVisibility::Hidden).await
}

/// Show a hidden board again
///
/// Ends the hiding of a board of one of the user's categories. For a board that is not
/// filed under the category nothing changes, and the answer is 204 all the same; a
/// board of another workspace than the category's answers as one that is not there.
#[utoipa::path(
    put,
    path = "/w/{workspaceId}/categories/{categoryId}/containers/{containerId}/unhide",
    operation_id = "unhideContainer",
    tag = tag::SIDEBAR,
    params(WorkspaceId, CategoryId, ContainerId),
    responses((status = 204, description = "The board is shown.")),
)]
async fn handle_unhide_container(
    InContainer(ws): InContainer,
    Path(place): Path<ContainerPlace>,
) -> ApiResult {
    let ws = ws.require_no_guest()?;
    set_sidebar_visibility(&ws, &place, SidebarVisibility::Visible).await
}

async fn set_sidebar_visibility(
    ws: &InWorkspace,
    (_, category_id, container_id): &ContainerPlace,
    visibility: SidebarVisibility,
) -> ApiResult {
    let (app, user_id, workspace_id) = (&ws.app, ws.caller.user_id.as_str(), &ws.workspace_id);
    app.user_category(workspace_id, user_id, category_id)
        .await?;
    require_container_visible(app, user_id, container_id).await?;
    app.set_sidebar_visibility(workspace_id, user_id, category_id, container_id, visibility)
        .await?;
    Ok(no_content())
}
