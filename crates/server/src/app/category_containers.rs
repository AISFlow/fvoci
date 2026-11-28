//! Which boards a user has filed under which sidebar category.

use std::collections::{HashMap, HashSet};

use fvoci_model::PublicContainers;
use fvoci_model::{
    CATEGORY_CONTAINERS_SORT_ORDER_GAP, Category, CategoryContainerMetadata, CategoryContainers,
    CategoryType, Container, ContainerCategoryWebsocketData, SidebarVisibility,
};

use super::App;
use super::category::CATEGORY;
use crate::error::{AppError, OrMissing};

/// The name of the category every user has, which collects the boards filed nowhere else.
pub const DEFAULT_CATEGORY_CONTAINERS: &str = fvoci_model::DEFAULT_CATEGORY_NAME;

/// The default category of a user who has `existing` categories in the workspace: it
/// goes after them.
fn default_category(user_id: &str, workspace_id: &str, existing: usize) -> Category {
    Category {
        name: DEFAULT_CATEGORY_CONTAINERS.to_owned(),
        user_id: user_id.to_owned(),
        workspace_id: workspace_id.to_owned(),
        collapsed: false,
        category_type: CategoryType::System,
        sort_order: i64::try_from(existing)
            .unwrap_or(i64::MAX)
            .saturating_mul(CATEGORY_CONTAINERS_SORT_ORDER_GAP),
        ..Category::default()
    }
}

fn has_default_category(category_containers: &[CategoryContainers]) -> bool {
    category_containers
        .iter()
        .any(|entry| entry.category.category_type == CategoryType::System)
}

impl App {
    /// Returns a user's categories in a workspace with the boards in each, creating the
    /// default category first if the user does not have it yet.
    ///
    /// # Errors
    ///
    /// Returns the store's error if the categories cannot be read or the default
    /// category cannot be created.
    pub async fn user_category_containers(
        &self,
        user_id: &str,
        workspace_id: &str,
    ) -> Result<Vec<CategoryContainers>, AppError> {
        let category_containers = self.read_category_containers(user_id, workspace_id).await?;
        if has_default_category(&category_containers) {
            return Ok(category_containers);
        }
        self.ensure_default_category(user_id, workspace_id).await
    }

    /// A user's categories in a workspace with the boards in each, as they are.
    async fn read_category_containers(
        &self,
        user_id: &str,
        workspace_id: &str,
    ) -> Result<Vec<CategoryContainers>, AppError> {
        Ok(self
            .store()
            .get_user_category_containers(user_id, workspace_id)
            .await?)
    }

    /// Makes the default category of a user in a workspace if they have none, and
    /// returns their categories with it. One request makes it at a time, so that two at
    /// once from a new user do not make two.
    async fn ensure_default_category(
        &self,
        user_id: &str,
        workspace_id: &str,
    ) -> Result<Vec<CategoryContainers>, AppError> {
        let _turn = self.default_category_turn().await;
        let category_containers = self.read_category_containers(user_id, workspace_id).await?;
        if has_default_category(&category_containers) {
            return Ok(category_containers);
        }
        self.create_containers_category(user_id, workspace_id, &category_containers)
            .await?;
        // Read again: making the category moved the others, and the list is in the
        // order of the categories as they are now.
        self.read_category_containers(user_id, workspace_id).await
    }

    /// The sidebar of a guest of the workspace: their categories as
    /// [`App::user_category_containers`] has them, with the boards they are on now and no
    /// other. A board they filed while they worked in the workspace, or before they
    /// left it and came back as a guest, is not theirs to know of any more.
    ///
    /// # Errors
    ///
    /// Returns the store's error if the categories or the boards cannot be read.
    pub async fn category_containers_of_guest(
        &self,
        user_id: &str,
        workspace_id: &str,
    ) -> Result<Vec<CategoryContainers>, AppError> {
        let mut category_containers = self.user_category_containers(user_id, workspace_id).await?;
        let on: HashSet<String> = self
            .readable_workspace_containers(user_id, workspace_id, fvoci_model::WorkspaceRole::Guest)
            .await?
            .into_iter()
            .map(|container| container.id)
            .collect();
        for category in &mut category_containers {
            category
                .container_metadata
                .retain(|entry| on.contains(&entry.container_id));
        }
        Ok(category_containers)
    }

    /// Creates the default category and files under it every board the user is an
    /// explicit member of that is in no category yet.
    async fn create_containers_category(
        &self,
        user_id: &str,
        workspace_id: &str,
        existing: &[CategoryContainers],
    ) -> Result<CategoryContainers, AppError> {
        let category = default_category(user_id, workspace_id, existing.len());
        let created = self.create_category(category).await.map_err(|error| {
            error.context("createBoardsCategory default category creation failed")
        })?;

        // Read boards before memberships so a board created concurrently in between does
        // not appear in the board list without a corresponding membership.
        // Boards and wikis alike, whichever the request is about that makes the category:
        // it is made once, and files what the user is on.
        let containers = self
            .store()
            .get_containers_for_user_and_workspace(
                user_id,
                workspace_id,
                PublicContainers::Exclude,
                self.allowed_kinds(),
            )
            .await
            .map_err(|error| {
                AppError::from(error)
                    .context("createBoardsCategory error fetching user's workspace's boards")
            })?;

        let memberships: HashMap<String, bool> = self
            .store()
            .get_members_for_user(user_id)
            .await
            .map_err(|error| {
                AppError::from(error)
                    .context("createBoardsCategory error fetching user's board memberships")
            })?
            .into_iter()
            .map(|member| {
                (
                    member.container_id,
                    member.synthetic && member.inherited_role.is_none(),
                )
            })
            .collect();
        let mut container_ids = Vec::new();
        for container in containers {
            let Some(synthetic) = memberships.get(&container.id) else {
                return Err(AppError::internal(
                    "createBoardsCategory: board membership not found for user's board",
                ));
            };
            // A board the user can merely see shows in the sidebar only once opened.
            if *synthetic {
                continue;
            }
            let filed = existing
                .iter()
                .flat_map(|entry| &entry.container_metadata)
                .any(|metadata| metadata.container_id == container.id);
            if !filed {
                container_ids.push(container.id);
            }
        }

        let container_metadata = container_ids
            .iter()
            .map(|container_id| CategoryContainerMetadata {
                container_id: container_id.clone(),
                hidden: false,
            })
            .collect();
        if !container_ids.is_empty() {
            self.file_containers(workspace_id, user_id, &created.id, &container_ids)
                .await
                .map_err(|error| {
                    error.context(format_args!(
                        "createBoardsCategory failed to add category-less board to the \
                         default category, defaultCategoryID: {}, error",
                        created.id
                    ))
                })?;
            let category_containers = self.read_category_containers(user_id, workspace_id).await?;
            self.tell_category_containers(workspace_id, user_id, &created.id, &category_containers)
                .await?;
        }
        Ok(CategoryContainers {
            category: created,
            container_metadata,
        })
    }

    /// Files boards under one of a user's categories, moving them out of any other, and
    /// tells the user's connection the category's new content.
    ///
    /// Each board must be one of the workspace or a built-in template; whether the user may
    /// see it is the caller's to check.
    ///
    /// # Errors
    ///
    /// Returns a not-found error if the category is not one of the user's categories in
    /// the workspace, or a board is not there or is one of another workspace; or the store's error
    /// if the boards cannot be filed. Nothing is written in any of these cases.
    pub async fn add_update_user_category_container(
        &self,
        workspace_id: &str,
        user_id: &str,
        category_id: &str,
        container_ids: &[String],
    ) -> Result<(), AppError> {
        if container_ids.is_empty() {
            return Ok(());
        }
        self.file_containers(workspace_id, user_id, category_id, container_ids)
            .await?;
        let category_containers = self.user_category_containers(user_id, workspace_id).await?;
        self.tell_category_containers(workspace_id, user_id, category_id, &category_containers)
            .await
    }

    /// Files boards under one of a user's categories, moving them out of any other,
    /// once the category is the user's and every board one of the workspace.
    async fn file_containers(
        &self,
        workspace_id: &str,
        user_id: &str,
        category_id: &str,
        container_ids: &[String],
    ) -> Result<(), AppError> {
        // Verify category ownership before writing so a user cannot file boards into
        // another user's category.
        self.user_category(workspace_id, user_id, category_id)
            .await?;
        for container_id in container_ids {
            self.require_container_of_workspace(workspace_id, container_id)
                .await?;
        }
        self.store()
            .add_update_category_container(user_id, category_id, container_ids)
            .await?;
        Ok(())
    }

    /// Tells the user's connection the boards of one of their categories, as
    /// `category_containers` has them.
    async fn tell_category_containers(
        &self,
        workspace_id: &str,
        user_id: &str,
        category_id: &str,
        category_containers: &[CategoryContainers],
    ) -> Result<(), AppError> {
        let Some(updated) = category_containers
            .iter()
            .find(|entry| entry.category.id == category_id)
        else {
            return Err(AppError::not_found(CATEGORY));
        };
        let payload: Vec<ContainerCategoryWebsocketData> = updated
            .container_metadata
            .iter()
            .map(|metadata| ContainerCategoryWebsocketData {
                container_id: metadata.container_id.clone(),
                category_id: category_id.to_owned(),
                hidden: metadata.hidden,
            })
            .collect();
        self.ws()
            .broadcast_category_container_change(workspace_id, user_id, &payload)
            .await;
        Ok(())
    }

    /// Puts the boards of one of a user's categories into the given order and tells the
    /// user's connection about it. Returns the order.
    ///
    /// # Errors
    ///
    /// Returns a not-found error if the category is not one of the user's categories in
    /// the workspace, a bad request if `new_order` does not name exactly as many boards as
    /// the category has, or names one that is not in it, and the store's error if a
    /// read or write fails.
    pub async fn reorder_category_containers(
        &self,
        user_id: &str,
        workspace_id: &str,
        category_id: &str,
        new_order: Vec<String>,
    ) -> Result<Vec<String>, AppError> {
        self.verify_new_category_containers_match_existing(
            user_id,
            workspace_id,
            category_id,
            &new_order,
        )
        .await?;
        let new_order = self
            .store()
            .reorder_category_containers(category_id, new_order)
            .await?;
        self.ws()
            .broadcast_category_containers_reorder(workspace_id, user_id, category_id, &new_order)
            .await;
        Ok(new_order)
    }

    /// Checks that a new order names every board of the category once and nothing else.
    async fn verify_new_category_containers_match_existing(
        &self,
        user_id: &str,
        workspace_id: &str,
        category_id: &str,
        new_order: &[String],
    ) -> Result<(), AppError> {
        let category_containers = self.user_category_containers(user_id, workspace_id).await?;
        let Some(target) = category_containers
            .iter()
            .find(|entry| entry.category.id == category_id)
        else {
            return Err(AppError::not_found(CATEGORY));
        };
        if target.container_metadata.len() != new_order.len() {
            return Err(AppError::bad_request(format_args!(
                "the new order must list every board of the category once: it has {}, the \
                 category has {}",
                new_order.len(),
                target.container_metadata.len(),
            )));
        }
        let mut seen = HashSet::with_capacity(new_order.len());
        for container_id in new_order {
            if !seen.insert(container_id.as_str()) {
                return Err(AppError::bad_request(format_args!(
                    "the new order lists {container_id} more than once"
                )));
            }
        }
        let existing_ids: HashSet<&str> = target
            .container_metadata
            .iter()
            .map(|metadata| metadata.container_id.as_str())
            .collect();
        match new_order
            .iter()
            .find(|container_id| !existing_ids.contains(container_id.as_str()))
        {
            Some(unknown) => Err(AppError::bad_request(format_args!(
                "the new order names {unknown}, which is no board of the category"
            ))),
            None => Ok(()),
        }
    }

    /// Moves a board into `category_id` (optionally positioned via `index` and setting
    /// `hidden`) in a single database transaction and broadcasts the updated category
    /// entries and order.
    ///
    /// # Errors
    ///
    /// Returns not-found if `category_id` or `container_id` is absent in `workspace_id`,
    /// and the store's error on database failure.
    pub async fn put_category_container(
        &self,
        workspace_id: &str,
        user_id: &str,
        category_id: &str,
        container_id: &str,
        update: &fvoci_model::CategoryContainerUpdate,
    ) -> Result<(), AppError> {
        self.user_category(workspace_id, user_id, category_id)
            .await?;
        self.require_container_of_workspace(workspace_id, container_id)
            .await?;
        self.store()
            .put_category_container(user_id, workspace_id, category_id, container_id, update)
            .await
            .map_err(|error| {
                if error.is_not_found() {
                    AppError::not_found(CATEGORY)
                } else {
                    error.into()
                }
            })?;
        let category_containers = self.user_category_containers(user_id, workspace_id).await?;
        if let Some(updated) = category_containers
            .iter()
            .find(|entry| entry.category.id == category_id)
        {
            let payload: Vec<ContainerCategoryWebsocketData> = updated
                .container_metadata
                .iter()
                .map(|metadata| ContainerCategoryWebsocketData {
                    container_id: metadata.container_id.clone(),
                    category_id: category_id.to_owned(),
                    hidden: metadata.hidden,
                })
                .collect();
            let order: Vec<String> = updated
                .container_metadata
                .iter()
                .map(|metadata| metadata.container_id.clone())
                .collect();
            self.ws()
                .broadcast_category_container_change(workspace_id, user_id, &payload)
                .await;
            self.ws()
                .broadcast_category_containers_reorder(workspace_id, user_id, category_id, &order)
                .await;
        }
        Ok(())
    }

    /// Hides a board in one of a user's categories, or shows it again, and tells the
    /// user's connection about it.
    ///
    /// The category must be the user's and the board one of its workspace or a built-in
    /// template (both checked here), and the route has checked that the user sees the
    /// board. If the user does not have the board in the category, nothing changes in the
    /// store, and the user's connection is still notified.
    ///
    /// # Errors
    ///
    /// Returns a not-found error if the category is not one of the user's categories in
    /// the workspace, or the board is not there or is one of another workspace; and the store's
    /// error if the write fails.
    pub async fn set_sidebar_visibility(
        &self,
        workspace_id: &str,
        user_id: &str,
        category_id: &str,
        container_id: &str,
        visibility: SidebarVisibility,
    ) -> Result<(), AppError> {
        self.user_category(workspace_id, user_id, category_id)
            .await?;
        self.require_container_of_workspace(workspace_id, container_id)
            .await?;
        self.store()
            .set_container_visibility(user_id, category_id, container_id, visibility)
            .await
            .map_err(|error| {
                AppError::from(error)
                    .context("SetBoardVisibility: failed to update board visibility")
            })?;
        let change = ContainerCategoryWebsocketData {
            container_id: container_id.to_owned(),
            category_id: category_id.to_owned(),
            hidden: visibility == SidebarVisibility::Hidden,
        };
        self.ws()
            .broadcast_category_container_change(workspace_id, user_id, &[change])
            .await;
        Ok(())
    }

    /// Fails with the not-found error of a board that is not there unless the board is
    /// one a category of the workspace takes: one of the workspace, or a built-in template, which
    /// every workspace's categories take. A user who works in two workspaces of an organization
    /// sees the boards of both; a category holds those of its own.
    ///
    /// # Errors
    ///
    /// Returns a not-found error for a board that is not there or is another workspace's, and
    /// the store's error if the board cannot be read.
    pub(crate) async fn require_container_of_workspace(
        &self,
        workspace_id: &str,
        container_id: &str,
    ) -> Result<(), AppError> {
        // The workspace named, which may be another than the handle's (a board filed
        // back in its own): compared here, not by `App::container`.
        let container = self
            .store()
            .get_container(container_id)
            .await
            .or_missing("board")?;
        if category_of_workspace_takes(&container, workspace_id) {
            Ok(())
        } else {
            Err(AppError::not_found("board"))
        }
    }
}

/// Whether a category of the workspace takes the board: one of the workspace, or a built-in
/// template.
pub(super) fn category_of_workspace_takes(container: &Container, workspace_id: &str) -> bool {
    container.workspace_id == workspace_id || container.is_builtin
}
