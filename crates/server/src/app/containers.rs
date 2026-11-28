//! Boards: reading, creating, patching, deleting, restoring, and sidebar placement.

use fvoci_model::PublicContainers;
use fvoci_model::{
    CategoryType, Container, ContainerCategoryWebsocketData, ContainerKind, ContainerMember,
    ContainerPatch, ContainerVisibility, new_id,
};
use fvoci_store::DeletedContainer;

use super::{App, Former};
use crate::collab::{AccessScope, Deletion};
use crate::error::{AppError, OrMissing};
use crate::metrics::CourseSource;
use crate::permissions::ContainerScope;

/// Whether a patch of a board changes who may do what on it: its visibility (open to the
/// workspace or by invitation) or the role of who joins it.
pub(super) fn changes_access(patch: &ContainerPatch) -> bool {
    patch.visibility.is_some() || patch.minimum_role.is_some()
}

/// Whether the user who creates a board becomes a member of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CreatorMembership {
    /// The creator becomes an admin and editor of the board.
    Admin,
    /// The board starts without members.
    None,
}

impl App {
    /// Returns a board: for a handle of one workspace ([`App::in_workspace`]), only a
    /// board of that workspace or a built-in template.
    ///
    /// # Errors
    ///
    /// Returns a not-found error if there is no such board, or the store's error.
    pub async fn container(&self, container_id: &str) -> Result<Container, AppError> {
        let container = self
            .store()
            .get_container(container_id)
            .await
            .or_missing("board")?;
        // A board of another workspace than the handle's is not there.
        if self.reaches_container(&container) {
            Ok(container)
        } else {
            Err(AppError::not_found("board"))
        }
    }

    /// Returns the boards of a workspace that a user is a member of and, if asked for, the
    /// workspace's open boards: of the kinds the handle reaches ([`App::reaching`]).
    /// Templates are left out.
    ///
    /// # Errors
    ///
    /// Returns the store's error if the query fails.
    pub async fn containers_for_user_and_workspace(
        &self,
        user_id: &str,
        workspace_id: &str,
        public_containers: PublicContainers,
    ) -> Result<Vec<Container>, AppError> {
        Ok(self
            .store()
            .get_containers_for_user_and_workspace(
                user_id,
                workspace_id,
                public_containers,
                self.kinds(),
            )
            .await?)
    }

    /// Returns the boards of a workspace that a user is a member of, of the kinds the
    /// handle reaches: [`App::containers_for_user_and_workspace`] without the open ones.
    ///
    /// # Errors
    ///
    /// Returns the store's error if the query fails.
    pub async fn joined_containers(
        &self,
        user_id: &str,
        workspace_id: &str,
    ) -> Result<Vec<Container>, AppError> {
        self.containers_for_user_and_workspace(user_id, workspace_id, PublicContainers::Exclude)
            .await
    }

    /// Creates a board without members and files it under the default category of
    /// `user_id`, unless it is a template. The board gets a new identifier.
    ///
    /// # Errors
    ///
    /// Returns an error if the board already has an identifier, if it cannot be stored,
    /// or if it cannot be filed under the user's default category; in the last case the
    /// board exists nonetheless.
    pub async fn create_container(
        &self,
        container: Container,
        user_id: &str,
    ) -> Result<Container, AppError> {
        self.create_container_as(container, user_id, CreatorMembership::None)
            .await
    }

    /// Creates a board as [`App::create_container`] does and makes `user_id` an admin and
    /// editor of it.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`App::create_container`].
    pub async fn create_container_with_admin(
        &self,
        container: Container,
        user_id: &str,
    ) -> Result<Container, AppError> {
        self.create_container_as(container, user_id, CreatorMembership::Admin)
            .await
    }

    async fn create_container_as(
        &self,
        mut container: Container,
        user_id: &str,
        membership: CreatorMembership,
    ) -> Result<Container, AppError> {
        refuse_named_id(&container)?;
        self.require_makes(&container)?;
        container.id = new_id();

        let creator = match membership {
            CreatorMembership::Admin => Some(
                self.store()
                    .insert_container_with_admin(&mut container, user_id)
                    .await?,
            ),
            CreatorMembership::None => {
                self.store()
                    .insert_container(&mut container, user_id)
                    .await?;
                None
            }
        };

        let app = self.clone();
        let created = container.clone();
        self.spawn_background(async move {
            app.ws()
                .broadcast_container_change(&created.workspace_id, &created)
                .await;
            if let Some(creator) = creator {
                app.ws()
                    .broadcast_member_change(&created.workspace_id, &created.id, &creator)
                    .await;
            }
        });

        if !container.is_template {
            self.add_containers_to_default_category(
                user_id,
                &container.workspace_id,
                std::slice::from_ref(&container.id),
            )
            .await?;
            if container.kind == ContainerKind::Course {
                self.metrics().course_created(CourseSource::Blank);
            }
        }
        Ok(container)
    }

    /// Files boards under a user's default category in a workspace, creating that category
    /// if the user does not have it yet.
    pub(super) async fn add_containers_to_default_category(
        &self,
        user_id: &str,
        workspace_id: &str,
        container_ids: &[String],
    ) -> Result<(), AppError> {
        let categories = self.user_category_containers(user_id, workspace_id).await?;
        let Some(default) = categories
            .iter()
            .find(|entry| entry.category.category_type == CategoryType::System)
        else {
            return Err(AppError::internal(format_args!(
                "no default category found for user userID: {user_id}"
            )));
        };
        self.add_update_user_category_container(
            workspace_id,
            user_id,
            &default.category.id,
            container_ids,
        )
        .await
    }

    /// Applies a patch to a board on behalf of a user and tells the board's members.
    ///
    /// # Errors
    ///
    /// Returns a not-found error if there is no such board, or the store's error.
    pub async fn patch_container(
        &self,
        patch: &ContainerPatch,
        container_id: &str,
        user_id: &str,
    ) -> Result<Container, AppError> {
        let mut is_template = false;
        let mut scope_before = None;
        if patch.visibility.is_some() {
            let container = self
                .store()
                .get_container(container_id)
                .await
                .or_missing("board")?;
            is_template = container.is_template;
            scope_before = Some(ContainerScope::of(&container));
        }

        if changes_access(patch) {
            self.collab()
                .narrowing_access(self, &AccessScope::container(container_id));
        }
        let updated = self
            .store()
            .patch_container(container_id, patch, user_id)
            .await?;
        // Who read an open board, wiki, or course without joining it reads it no more
        // once it is private: they are told it is gone for them, as when it is deleted.
        let closed = scope_before.filter(|before| {
            *before == ContainerScope::OpenToRead
                && ContainerScope::of(&updated) != ContainerScope::OpenToRead
        });
        if changes_access(patch) {
            self.collab()
                .recheck_access(self, AccessScope::container(container_id))
                .await;
        }

        let app = self.clone();
        let container = updated.clone();
        let new_visibility = patch.visibility.filter(|_| is_template);
        self.spawn_background(async move {
            app.ws()
                .broadcast_container_change(&container.workspace_id, &container)
                .await;
            if let Some(visibility) = new_visibility {
                app.broadcast_workspace_users(&container, visibility).await;
            }
            if let Some(scope) = closed {
                app.broadcast_closed(&container, scope).await;
            }
        });

        Ok(updated)
    }

    /// Puts the card properties of a board into the order of their identifiers in
    /// `order`, which names each of them once, tells the board's listeners with the
    /// board's change as for any change of it, and returns the order.
    ///
    /// # Errors
    ///
    /// Returns a not-found error if there is no such board, a bad request if `order`
    /// does not name each property once, and the store's error.
    pub async fn reorder_card_properties(
        &self,
        container_id: &str,
        order: &[String],
        user_id: &str,
    ) -> Result<Vec<String>, AppError> {
        let container = match self
            .store()
            .reorder_card_properties(container_id, order, user_id)
            .await
        {
            Ok(container) => container,
            Err(error) if error.is_not_found() => return Err(AppError::not_found("board")),
            Err(error) => return Err(error.into()),
        };
        let app = self.clone();
        self.spawn_background(async move {
            app.ws()
                .broadcast_container_change(&container.workspace_id, &container)
                .await;
        });
        Ok(order.to_vec())
    }

    /// Tells who read the board as it was, `scope`, and no longer does that it is gone
    /// for them: the message of its deletion, to who reads it no more alone. Nothing is
    /// told if it is open again by now: a later change made it so, and told so.
    async fn broadcast_closed(&self, container: &Container, scope: ContainerScope) {
        let now = match self.store().get_container(&container.id).await {
            Ok(now) => Some(ContainerScope::of(&now)),
            Err(error) if error.is_not_found() => None,
            Err(error) => {
                tracing::error!(%error, "Unable to read the board made private");
                return;
            }
        };
        if now == Some(scope) {
            return;
        }
        let members = match self.effective_container_members(&container.id).await {
            Ok(members) => members,
            Err(error) => {
                tracing::error!(%error, "Unable to get the board members");
                return;
            }
        };
        self.ws()
            .broadcast_closed_to_others(&container.workspace_id, &container.id, &members, scope)
            .await;
    }

    /// Tells who works in the workspace and is no member of a template that they gained or
    /// lost access to it, after the template changed between open and private.
    async fn broadcast_workspace_users(
        &self,
        container: &Container,
        visibility: ContainerVisibility,
    ) {
        // A team's people keep their role across the change, as direct members do.
        let members = match self.effective_container_members(&container.id).await {
            Ok(members) => members,
            Err(error) => {
                tracing::error!(%error, "Unable to get the board members");
                Vec::new()
            }
        };
        // Who works in the board's workspace, whom an open template is for: nobody else
        // gains or loses it, and nobody else is named.
        let users = match self
            .store()
            .get_workspace_worker_ids(&container.workspace_id)
            .await
        {
            Ok(users) => users,
            Err(error) => {
                tracing::error!(%error, "Unable to get the workspace users");
                Vec::new()
            }
        };
        let others = users
            .iter()
            .filter(|user_id| !members.iter().any(|member| member.user_id == **user_id));
        for user_id in others {
            match visibility {
                ContainerVisibility::Private => {
                    self.ws()
                        .broadcast_member_delete(&container.workspace_id, &container.id, user_id)
                        .await;
                }
                ContainerVisibility::Open => {
                    let viewer = ContainerMember {
                        user_id: user_id.clone(),
                        container_id: container.id.clone(),
                        role: fvoci_model::ContainerRole::Viewer,
                        synthetic: true,
                        inherited_role: None,
                    };
                    self.ws()
                        .broadcast_member_change(&container.workspace_id, &container.id, &viewer)
                        .await;
                }
            }
        }
    }

    /// Deletes a board with its blocks and tells the board's members. Deleting a board
    /// that does not exist succeeds.
    ///
    /// # Errors
    ///
    /// Returns the store's error if the board cannot be read or deleted.
    pub async fn delete_container(
        &self,
        container_id: &str,
        user_id: &str,
    ) -> Result<(), AppError> {
        let Some(container) = self.store().get_container(container_id).await.found()? else {
            return Ok(());
        };
        // The memberships go with the board; who is told is who was a member.
        let members = self
            .store()
            .effective_container_members(container_id)
            .await?;
        let (removed, by) = (container_id.to_owned(), user_id.to_owned());
        self.collab()
            .remove(
                self,
                Deletion::Containers(vec![removed.clone()]),
                async move |app| Ok(app.store().delete_container(&removed, &by).await?),
            )
            .await?;
        self.collab()
            .recheck_access(self, AccessScope::container(container_id))
            .await;

        let former = Former {
            members: &members,
            scope: ContainerScope::of(&container),
        };
        self.ws()
            .broadcast_container_delete(&container.workspace_id, &container.id, former)
            .await;
        // The board's sharing went with it; so does whoever followed it by its link.
        self.ws().end_readers_of_container(&container.id);
        Ok(())
    }

    /// Returns a board that is in the trash with the memberships it had, or `None` if
    /// no board of that identifier is there.
    ///
    /// # Errors
    ///
    /// Returns the store's error if the trash cannot be read.
    pub async fn deleted_container(
        &self,
        container_id: &str,
    ) -> Result<Option<DeletedContainer>, AppError> {
        self.store()
            .get_deleted_container(container_id)
            .await
            .found()
    }

    /// Restores a board from the trash, with its blocks, its file records, the
    /// memberships and places in the sidebar of the users who still exist and the roles
    /// of its surviving teams, rechecks its card rooms, tells the board's members and
    /// each user it is filed for, and returns the board.
    ///
    /// # Errors
    ///
    /// Returns a not-found error if the board is not in the trash, and the store's
    /// error if it cannot be restored.
    pub async fn undelete_container(
        &self,
        container_id: &str,
        modified_by: &str,
    ) -> Result<Container, AppError> {
        let restored = {
            let _structure = self.collab().changing_structure(self, container_id).await;
            self.store()
                .undelete_container(container_id, modified_by)
                .await
        };
        let container = restored.or_missing("board")?;
        // Its people and its surviving teams have their roles on it again. Its rooms
        // were emptied when it was deleted: this is for an entry under way, which
        // reads the roles again.
        self.collab()
            .recheck_access(self, AccessScope::container(&container.id))
            .await;

        let app = self.clone();
        let restored = container.clone();
        self.spawn_background(async move {
            app.ws()
                .broadcast_container_change(&restored.workspace_id, &restored)
                .await;
            app.tell_where_restored_container_is(&restored).await;
        });
        Ok(container)
    }

    /// Tells each user who has a restored board filed where it is: the board's entry, and
    /// the order of the boards of that category, which moved to make room for it. Read
    /// after the restore; a failed read is logged, the restore stands.
    async fn tell_where_restored_container_is(&self, container: &Container) {
        let places = match self.store().get_container_places(&container.id).await {
            Ok(places) => places,
            Err(error)
                if {
                    tracing::error!(
                        %error,
                        container_id = container.id,
                        "the places of a restored board could not be read"
                    );
                    true
                } =>
            {
                return;
            }
            Err(_) => unreachable!(),
        };
        for place in places {
            let filed = ContainerCategoryWebsocketData {
                container_id: container.id.clone(),
                category_id: place.category_id.clone(),
                hidden: place.hidden,
            };
            self.ws()
                .broadcast_category_container_change(
                    &container.workspace_id,
                    &place.user_id,
                    std::slice::from_ref(&filed),
                )
                .await;
            let categories = self
                .store()
                .get_user_category_containers(&place.user_id, &container.workspace_id)
                .await;
            let order: Option<Vec<String>> = categories.ok().and_then(|categories| {
                categories
                    .into_iter()
                    .find(|entry| entry.category.id == place.category_id)
                    .map(|entry| {
                        entry
                            .container_metadata
                            .into_iter()
                            .map(|metadata| metadata.container_id)
                            .collect()
                    })
            });
            if let Some(order) = order {
                self.ws()
                    .broadcast_category_containers_reorder(
                        &container.workspace_id,
                        &place.user_id,
                        &place.category_id,
                        &order,
                    )
                    .await;
            }
        }
    }
}

/// Refuses a new board that names its own identifier: the server gives it one, however
/// the board is made.
pub(super) fn refuse_named_id(container: &Container) -> Result<(), AppError> {
    if container.id.is_empty() {
        Ok(())
    } else {
        Err(AppError::bad_request("new board cannot have an ID").with_field("id"))
    }
}
