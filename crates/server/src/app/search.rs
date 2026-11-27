//! Searching boards by title or by the name of a card property.

use fvoci_model::{Container, ContainerSearchField, PublicContainers};

use super::App;
use crate::error::AppError;

impl App {
    /// Returns the boards whose `search_field` matches `term`, among the boards the user
    /// is a member of and, with [`PublicContainers::Include`], the open boards. The search
    /// spans all workspaces and leaves out templates; it finds the kinds the handle
    /// reaches ([`App::reaching`]).
    ///
    /// # Errors
    ///
    /// Returns the store's error if the query fails.
    pub async fn search_containers_for_user(
        &self,
        term: &str,
        search_field: ContainerSearchField,
        user_id: &str,
        public_containers: PublicContainers,
    ) -> Result<Vec<Container>, AppError> {
        Ok(self
            .store()
            .search_containers_for_user(
                term,
                search_field,
                user_id,
                public_containers,
                self.kinds(),
            )
            .await?)
    }

    /// Returns the boards of a workspace whose title matches `term`, among the workspace's open
    /// boards and the private ones the user is a member of. Templates are left out.
    ///
    /// # Errors
    ///
    /// Returns the store's error if the query fails.
    pub async fn search_containers_for_user_in_workspace(
        &self,
        workspace_id: &str,
        term: &str,
        user_id: &str,
    ) -> Result<Vec<Container>, AppError> {
        Ok(self
            .store()
            .search_containers_for_user_in_workspace(workspace_id, term, user_id, self.kinds())
            .await?)
    }
}
