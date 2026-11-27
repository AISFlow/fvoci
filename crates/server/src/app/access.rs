//! What is known of a user and a board: the facts [`permissions::grant`] is asked with.
//!
//! This and `Store::resolve_workspace` (by `App::actor`, and `Store::org_role_of_user` by
//! `App::org_actor` for the organization alone) are where the facts of a permission are
//! read.
//! Every check of a route that names a board asks [`App::container_access`], and decides by
//! the [`Grant`] it gives; nothing else reads a membership to decide what a user may do.

use fvoci_model::{Container, ContainerMember, ContainerRole, Upload, UploadPart, UploadPurpose};
use fvoci_store::DeletedContainer;

use super::App;
use crate::error::AppError;
use crate::permissions::{self, Actor, ContainerFacts, ContainerScope, Grant};

/// A board, and what one user is and may do there.
#[derive(Debug, Clone)]
pub struct ContainerAccess {
    /// The board as it is stored.
    pub container: Container,
    /// What the user is in the board's workspace and organization.
    pub actor: Actor,
    /// The user's membership of the board as stored; `None` for who is no member.
    pub member: Option<ContainerMember>,
    /// The inherited role observed with the direct membership.
    pub team_role: Option<ContainerRole>,
    /// What the user may do on the board.
    pub grant: Grant,
}

impl ContainerAccess {
    /// Whether the user holds an enrolled membership on this container (either a direct
    /// membership or an inherited group role as a workspace worker).
    #[must_use]
    pub fn has_membership(&self) -> bool {
        let direct = self.member.is_some();
        let inherited = self.actor.works_there() && self.team_role.is_some();
        self.actor.workspace.is_some() && (direct || inherited)
    }
}

/// A board in the trash, and what one user may do with it: what the role they had
/// when it was deleted would let them do, as what they are in its workspace now.
#[derive(Debug, Clone)]
pub struct DeletedContainerAccess {
    /// The board as the trash keeps it.
    pub deleted: DeletedContainer,
    /// What the user may do with the board.
    pub grant: Grant,
}

impl App {
    /// The board and what the user may do on it; `None` if there is no such board, and
    /// for a handle of one workspace ([`App::in_workspace`]) if the board is of another:
    /// this is where a request's workspace and the workspace of what it names are
    /// compared. Built-in templates (`is_builtin`) carry an empty `workspace_id` and
    /// resolve the caller's role against `self.workspace` (granting nothing when the
    /// handle has no workspace or the caller is a guest). A user without an identifier
    /// may do nothing.
    ///
    /// The same fact reads are made whether the board is there or not, so that a
    /// board that is not there for the user takes as long to refuse as one that does
    /// not exist.
    ///
    /// # Errors
    ///
    /// Returns the store's error if a fact cannot be read: that is no "no", which would
    /// tell every member that their board does not exist.
    pub async fn container_access(
        &self,
        user_id: &str,
        container_id: &str,
    ) -> Result<Option<ContainerAccess>, AppError> {
        let facts = self
            .store()
            .container_access_facts(container_id, user_id)
            .await
            .map_err(|error| AppError::from(error).context("reading a board to check access"))?;
        let container = facts
            .container
            .filter(|container| self.reaches_container(container));
        let member = facts.member;
        let team_role = facts.team_role;
        let workspace_id = match container.as_ref() {
            Some(container) if container.is_builtin => self.workspace.as_deref().unwrap_or(""),
            Some(container) => &container.workspace_id,
            None => self.workspace.as_deref().unwrap_or(""),
        };
        let actor = self.actor(user_id, workspace_id).await?;
        let Some(container) = container else {
            return Ok(None);
        };
        let grant = if user_id.is_empty() {
            Grant::NONE
        } else {
            permissions::grant(
                actor,
                ContainerFacts {
                    team_role,
                    scope: ContainerScope::of(&container),
                    role: member.as_ref().map(|m| m.role),
                },
            )
        };
        Ok(Some(ContainerAccess {
            container,
            actor,
            member,
            team_role,
            grant,
        }))
    }

    /// Refuses to make a board of a kind this handle does not reach
    /// ([`App::reaching`]): every way a board is made of what a request says asks here
    /// first, so that an operation on boards makes no wiki, whatever its body says. A
    /// copy is of the kind of what it copies, and what comes back from the trash of
    /// the kind it went as; the handle reached both before.
    ///
    /// # Errors
    ///
    /// Returns 400 naming `kind`.
    pub(super) fn require_makes(&self, container: &Container) -> Result<(), AppError> {
        if self.kinds.holds(container.kind) {
            return Ok(());
        }
        Err(AppError::bad_request(format_args!(
            "this operation makes no {}",
            container.kind.as_str()
        ))
        .with_field("kind"))
    }

    /// The user's upload that is still going and the parts it has received, as
    /// [`App::upload`] gives them, if it is of this handle's workspace: the workspace an
    /// archive is imported into, or the one of the board a file is for, in the trash too.
    /// An upload for a board that is gone altogether is of no workspace, and its owner
    /// ends it under any. For a handle of every workspace, any of the user's uploads.
    ///
    /// # Errors
    ///
    /// Returns a not-found error for an upload of another workspace and whatever
    /// [`App::upload`] returns.
    pub async fn workspace_upload(
        &self,
        upload_id: &str,
        user_id: &str,
    ) -> Result<(Upload, Vec<UploadPart>), AppError> {
        let (upload, parts) = self.upload(upload_id, user_id).await?;
        if self.workspace.is_none() {
            return Ok((upload, parts));
        }
        let reached = match upload.purpose {
            UploadPurpose::Import => self.reaches_workspace(&upload.workspace_id),
            UploadPurpose::File => match self.store().get_container(&upload.container_id).await {
                Ok(container) => self.reaches_container(&container),
                Err(error) if error.is_not_found() => self
                    .deleted_container(&upload.container_id)
                    .await?
                    .is_none_or(|deleted| self.reaches_container(&deleted.container)),
                Err(error) => return Err(error.into()),
            },
        };
        if reached {
            Ok((upload, parts))
        } else {
            Err(AppError::not_found("upload"))
        }
    }

    /// What the user may do on the board; nothing if there is no such board.
    ///
    /// # Errors
    ///
    /// Returns the store's error if a fact cannot be read.
    pub async fn container_grant(
        &self,
        user_id: &str,
        container_id: &str,
    ) -> Result<Grant, AppError> {
        Ok(self
            .container_access(user_id, container_id)
            .await?
            .map_or(Grant::NONE, |access| access.grant))
    }

    /// A board in the trash and what the user may do with it; `None` if no board of
    /// that identifier is there, or for a handle of one workspace if it is of another.
    /// The role is the one the trash kept of the user, and counts as what the user is in
    /// the board's workspace now: who has left the
    /// workspace since brings nothing back. A board in the trash is open to nobody
    /// (`ContainerScope::in_the_trash`).
    ///
    /// # Errors
    ///
    /// Returns the store's error if a fact cannot be read.
    pub async fn deleted_container_access(
        &self,
        user_id: &str,
        container_id: &str,
    ) -> Result<Option<DeletedContainerAccess>, AppError> {
        let Some(deleted) = self
            .deleted_container(container_id)
            .await?
            .filter(|deleted| self.reaches_container(&deleted.container))
        else {
            return Ok(None);
        };
        let actor = self.actor(user_id, &deleted.container.workspace_id).await?;
        let role = deleted
            .members
            .iter()
            .find(|member| member.user_id == user_id)
            .map(|member| member.role);
        let grant = if user_id.is_empty() {
            Grant::NONE
        } else {
            permissions::grant(
                actor,
                ContainerFacts {
                    team_role: None,
                    scope: ContainerScope::of(&deleted.container).in_the_trash(),
                    role,
                },
            )
        };
        Ok(Some(DeletedContainerAccess { deleted, grant }))
    }

    /// What the user is in the organization alone, of no workspace of it, as
    /// `Store::org_role_of_user` answers: nothing for who is no member of it, and for an
    /// organization that is not in use or is the system's.
    ///
    /// # Errors
    ///
    /// Returns the store's error if the membership cannot be read.
    pub async fn org_actor(&self, user_id: &str, org_id: &str) -> Result<Actor, AppError> {
        Ok(Actor {
            org: self.store().org_role_of_user(user_id, org_id).await?,
            workspace: None,
        })
    }

    /// What the user is in the workspace and its organization, as
    /// `Store::resolve_workspace` answers: nothing for who is no member of it, and for
    /// a workspace that is not there or not in use.
    ///
    /// # Errors
    ///
    /// Returns the store's error if the membership cannot be read.
    pub async fn actor(&self, user_id: &str, workspace_id: &str) -> Result<Actor, AppError> {
        if !workspace_id.is_empty()
            && let Some((known_user, known_ws, actor)) = self.known_actor.as_deref()
            && known_user == user_id
            && known_ws == workspace_id
        {
            return Ok(*actor);
        }
        let access = self
            .store()
            .resolve_workspace(user_id, workspace_id)
            .await?;
        Ok(access.map_or(Actor::default(), |access| Actor {
            org: access.org_role,
            workspace: Some(access.workspace_role),
        }))
    }
}

/// The board as who only manages it sees it ([`Grant::manages_only`]): its name, kind
/// and times, without its description and properties, which are its content.
#[must_use]
pub fn as_managed(mut container: Container) -> Container {
    container.description.clear();
    container.properties.clear();
    container.card_properties.clear();
    container
}
