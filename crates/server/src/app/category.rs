//! A user's sidebar categories.

use std::collections::HashSet;

use fvoci_model::{Category, CategoryPatch, CategoryType};

use super::App;
use super::category_containers::category_of_workspace_takes;
use crate::auth::now_millis;
use crate::error::{AppError, OrMissing};

/// What a request misses that names a category the user does not have.
pub(super) const CATEGORY: &str = "category";

impl App {
    /// Returns the category with the given identifier, deleted or not, whoever owns it.
    ///
    /// # Errors
    ///
    /// Returns a not-found error if there is no such category, or the store's error if
    /// it cannot be read.
    pub async fn category(&self, category_id: &str) -> Result<Category, AppError> {
        Ok(self.store().get_category(category_id).await?)
    }

    /// Creates a category, giving it an identifier and timestamps if it has none, and
    /// tells the owner's connection about it.
    ///
    /// # Errors
    ///
    /// Returns a bad request if the category is not valid, or the store's error if it
    /// cannot be written or read back.
    pub async fn create_category(&self, mut category: Category) -> Result<Category, AppError> {
        category.hydrate(now_millis());
        category.validate()?;
        self.store().create_category(&category).await?;
        let created = self.store().get_category(&category.id).await?;
        self.ws().broadcast_category_change(&created).await;
        Ok(created)
    }

    /// Changes the name and the collapsed state of a category to those of `category`,
    /// and tells the owner's connection about it. Nothing else can be changed, and a
    /// system category keeps its name.
    ///
    /// # Errors
    ///
    /// Returns a bad request if `category` is not valid, a not-found error if the user
    /// and workspace `category` names have no such category (also if it is deleted), or the
    /// store's error if it cannot be written or read back.
    pub async fn update_category(&self, mut category: Category) -> Result<Category, AppError> {
        let now = now_millis();
        category.hydrate(now);
        category.validate()?;

        let existing = self
            .user_category(&category.workspace_id, &category.user_id, &category.id)
            .await?;
        if existing.category_type == CategoryType::System {
            category.name = existing.name;
        }
        category.updated_at = now;
        self.store().update_category(&category).await?;

        let updated = self.store().get_category(&category.id).await?;
        self.ws().broadcast_category_change(&updated).await;
        Ok(updated)
    }

    /// Patches the name and/or collapsed state of a user's category in a single
    /// transaction and broadcasts the updated category. System categories ignore name
    /// changes.
    ///
    /// # Errors
    ///
    /// Returns not-found if `category_id` does not belong to `user_id` in `workspace_id`,
    /// bad-request if the patched name is invalid, and the store's error on database failure.
    pub async fn patch_category(
        &self,
        workspace_id: &str,
        user_id: &str,
        category_id: &str,
        patch: &CategoryPatch,
    ) -> Result<Category, AppError> {
        let updated = self
            .store()
            .patch_category(workspace_id, category_id, user_id, patch)
            .await
            .map_err(|error| {
                if error.is_not_found() {
                    AppError::not_found(CATEGORY)
                } else {
                    error.into()
                }
            })?;
        self.ws().broadcast_category_change(&updated).await;
        Ok(updated)
    }

    /// Deletes one of a user's categories in a workspace, after moving its boards to the
    /// user's default category, and tells the owner's connection about it.
    ///
    /// # Errors
    ///
    /// Returns a not-found error if the user has no such category in the workspace (also if
    /// it is deleted already), a permission error if it is a system category, or the
    /// store's error if a read or write fails.
    pub async fn delete_category(
        &self,
        category_id: &str,
        user_id: &str,
        workspace_id: &str,
    ) -> Result<Category, AppError> {
        let existing = self
            .user_category(workspace_id, user_id, category_id)
            .await?;
        if existing.category_type == CategoryType::System {
            return Err(AppError::forbidden("a system category cannot be deleted"));
        }

        self.move_containers_to_default_category(user_id, workspace_id, category_id)
            .await?;
        self.store()
            .delete_category(category_id, user_id, workspace_id)
            .await?;

        // The category is gone from the store. What is sent to the user's connections is
        // the category as it was, marked as deleted now.
        let deleted = Category {
            deleted_at: Some(now_millis()),
            ..existing
        };
        self.ws().broadcast_category_change(&deleted).await;
        Ok(deleted)
    }

    /// Returns one of a user's categories in a workspace.
    ///
    /// # Errors
    ///
    /// Returns a not-found error if there is no such category, or it is another user's or
    /// in another workspace: the categories of a user are nobody else's
    /// business, so for everyone else they do not exist.
    pub(crate) async fn user_category(
        &self,
        workspace_id: &str,
        user_id: &str,
        category_id: &str,
    ) -> Result<Category, AppError> {
        let category = self
            .store()
            .get_category(category_id)
            .await
            .or_missing(CATEGORY)?;
        if category.user_id != user_id || category.workspace_id != workspace_id {
            return Err(AppError::not_found(CATEGORY));
        }
        Ok(category)
    }

    /// Moves the boards of a category to the user's default category, which is created
    /// if the user does not have it yet. A board of another workspace, filed here before the
    /// categories took the boards of their own workspace only, goes to the user's default
    /// category of its own workspace if the user works there, and is filed nowhere if not. A
    /// board that is gone meanwhile is left out.
    async fn move_containers_to_default_category(
        &self,
        user_id: &str,
        workspace_id: &str,
        source_category_id: &str,
    ) -> Result<(), AppError> {
        let category_containers = self.user_category_containers(user_id, workspace_id).await?;
        let Some(source) = category_containers
            .iter()
            .find(|entry| entry.category.id == source_category_id)
        else {
            return Err(AppError::not_found(CATEGORY));
        };
        let Some(default) = category_containers
            .iter()
            .find(|entry| entry.category.category_type == CategoryType::System)
        else {
            return Err(AppError::internal(
                "moveBoardsToDefaultCategory: no default category found",
            ));
        };

        let mut container_ids = Vec::with_capacity(source.container_metadata.len());
        let mut elsewhere = Vec::new();
        for metadata in &source.container_metadata {
            // Of any workspace: a board of another is filed in its own (below), which a
            // handle of this workspace would not find (`App::container`).
            let Some(container) = self
                .store()
                .get_container(&metadata.container_id)
                .await
                .found()?
            else {
                continue;
            };
            if category_of_workspace_takes(&container, workspace_id) {
                container_ids.push(container.id);
            } else {
                elsewhere.push(container);
            }
        }
        self.add_update_user_category_container(
            workspace_id,
            user_id,
            &default.category.id,
            &container_ids,
        )
        .await
        .map_err(|error| error.context("moveBoardsToDefaultCategory"))?;
        for container in elsewhere {
            if self
                .works_in_workspace(user_id, &container.workspace_id)
                .await?
            {
                self.add_containers_to_default_category(
                    user_id,
                    &container.workspace_id,
                    &[container.id],
                )
                .await
                .map_err(|error| error.context("moveBoardsToDefaultCategory"))?;
            }
        }
        Ok(())
    }

    /// Puts a user's categories in a workspace into the given order and tells the user's
    /// connection about it. Returns the order.
    ///
    /// # Errors
    ///
    /// Returns a bad request if `new_order` does not name exactly as many categories as
    /// the user has, or names one the user does not have, and the store's error if a
    /// read or write fails.
    pub async fn reorder_categories(
        &self,
        user_id: &str,
        workspace_id: &str,
        new_order: Vec<String>,
    ) -> Result<Vec<String>, AppError> {
        self.verify_new_categories_match_existing(user_id, workspace_id, &new_order)
            .await?;
        let new_order = self
            .store()
            .reorder_categories(user_id, workspace_id, new_order)
            .await?;
        self.ws()
            .broadcast_category_reorder(workspace_id, user_id, &new_order)
            .await;
        Ok(new_order)
    }

    /// Checks that a new order names every category of the user once and nothing else.
    async fn verify_new_categories_match_existing(
        &self,
        user_id: &str,
        workspace_id: &str,
        new_order: &[String],
    ) -> Result<(), AppError> {
        let existing = self
            .store()
            .get_user_categories(user_id, workspace_id)
            .await?;
        if new_order.len() != existing.len() {
            return Err(AppError::bad_request(format_args!(
                "the new order must list every category once: it has {}, the user has {}",
                new_order.len(),
                existing.len(),
            )));
        }
        let mut seen = HashSet::with_capacity(new_order.len());
        for category_id in new_order {
            if !seen.insert(category_id.as_str()) {
                return Err(AppError::bad_request(format_args!(
                    "the new order lists {category_id} more than once"
                )));
            }
        }
        let existing_ids: HashSet<&str> = existing
            .iter()
            .map(|category| category.id.as_str())
            .collect();
        match new_order
            .iter()
            .find(|category_id| !existing_ids.contains(category_id.as_str()))
        {
            Some(unknown) => Err(AppError::bad_request(format_args!(
                "the new order names {unknown}, which is none of the user's categories"
            ))),
            None => Ok(()),
        }
    }
}
