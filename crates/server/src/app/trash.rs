//! What is in the trash, as lists to choose what to bring back from.

use fvoci_model::{DeletedItem, WorkspaceRole};

use super::App;
use super::list_page::ListPage;
use crate::error::AppError;

impl App {
    /// Returns one page of the cards of a board that are in the trash, the last deleted
    /// first.
    ///
    /// # Errors
    ///
    /// Returns the store's error if the trash cannot be read.
    pub async fn deleted_cards(
        &self,
        container_id: &str,
        page: ListPage,
    ) -> Result<Vec<DeletedItem>, AppError> {
        let paging = Some(page.paging());
        let deleted = self.store().get_deleted_cards(container_id, paging).await?;
        Ok(deleted
            .into_iter()
            .map(|deleted| DeletedItem {
                id: deleted.block.id,
                title: deleted.block.title,
                deleted_at: deleted.deleted_at,
                deleted_by: deleted.deleted_by,
                kind: None,
            })
            .collect())
    }

    /// Returns one page of the boards of a workspace that are in the trash and that the user
    /// was an admin of when they were deleted, or, for an administrator of the workspace
    /// (`role`), every one: whom `restoreContainer` lets bring them back. Of the kinds the
    /// handle reaches ([`App::reaching`]). The last deleted first.
    ///
    /// # Errors
    ///
    /// Returns the store's error if the trash cannot be read.
    pub async fn deleted_containers(
        &self,
        workspace_id: &str,
        user_id: &str,
        role: WorkspaceRole,
        page: ListPage,
    ) -> Result<Vec<DeletedItem>, AppError> {
        let paging = Some(page.paging());
        let deleted = self
            .store()
            .get_deleted_containers(
                workspace_id,
                (role != WorkspaceRole::Admin).then_some(user_id),
                paging,
                self.kinds(),
            )
            .await?;
        Ok(deleted
            .into_iter()
            .map(|deleted| DeletedItem {
                id: deleted.container.id,
                title: deleted.container.title,
                deleted_at: deleted.deleted_at,
                deleted_by: deleted.deleted_by,
                kind: Some(deleted.container.kind),
            })
            .collect())
    }
}
