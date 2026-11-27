//! Who is in a workspace: its members, what each is in it, and taking one out.
//!
//! The caller has asked whether the user may manage the members
//! (`permissions::WorkspacePermission::ManageMembers`); what a change of role makes of
//! a member is `permissions::may_become`.

use fvoci_model::{ContainerMember, ContainerRole, WorkspaceMember, WorkspaceRole, is_id};
use fvoci_store::MembershipChange;

use super::App;
use crate::collab::AccessScope;
use crate::error::{AppError, Reason};
use crate::permissions;

/// How often a change of role is tried against a role that others change meanwhile.
const ATTEMPTS: usize = 3;

const CHANGED_MEANWHILE: &str = "what the member is in the workspace was changed meanwhile";

/// What a change is refused with that would leave a workspace without an administrator.
const LAST_ADMIN: &str = "the workspace must keep an administrator";

fn no_such_member() -> AppError {
    AppError::not_found("member")
}

impl App {
    /// The members of the workspace, its guests too, with their names: the oldest
    /// membership first.
    ///
    /// # Errors
    ///
    /// Returns the store's error if they cannot be read.
    pub async fn workspace_members(
        &self,
        workspace_id: &str,
    ) -> Result<Vec<WorkspaceMember>, AppError> {
        Ok(self.store().get_workspace_members(workspace_id).await?)
    }

    /// Makes a member of the workspace what `role` says, on behalf of `by`, and returns
    /// the member. Who becomes a guest stops being an admin of the workspace's boards,
    /// and a board that would be left without one passes to `by`.
    ///
    /// # Errors
    ///
    /// Returns a 404 for somebody who is no member of the workspace, a 400 for a change
    /// that is not made so (a guest to a member, an administrator to a guest), and a
    /// 409 (reason `last_admin`) if the workspace would be left without an
    /// administrator.
    pub async fn change_workspace_role(
        &self,
        workspace_id: &str,
        user_id: &str,
        role: WorkspaceRole,
        by: &str,
    ) -> Result<WorkspaceMember, AppError> {
        let mut member = self.workspace_member(workspace_id, user_id).await?;
        // What the change makes of the member is decided of what they are when it is
        // stored: somebody else may have changed it since it was read.
        for _ in 0..ATTEMPTS {
            if member.role == role {
                return Ok(member);
            }
            if !permissions::may_become(member.role, role) {
                let how = match member.role {
                    WorkspaceRole::Guest => "a guest becomes a member by an invitation for members",
                    WorkspaceRole::Admin | WorkspaceRole::Member => {
                        "an administrator is made a member before a guest"
                    }
                };
                return Err(AppError::bad_request(how).with_field("role"));
            }
            if matches!(
                (member.role, role),
                (
                    WorkspaceRole::Admin,
                    WorkspaceRole::Member | WorkspaceRole::Guest
                ) | (WorkspaceRole::Member, WorkspaceRole::Guest)
            ) {
                self.collab()
                    .narrowing_access(self, &AccessScope::workspace(workspace_id).of_user(user_id));
            }
            let change = self
                .store()
                .change_workspace_role(workspace_id, user_id, (member.role, role), by)
                .await?;
            if let MembershipChange::RoleIs(current) = change {
                member.role = current;
                continue;
            }
            let taken_over = self.settle(workspace_id, by, change)?;
            self.ws()
                .workspace_role_changed_to(workspace_id, user_id, role);
            self.recheck_member(workspace_id, user_id, by, &taken_over)
                .await;
            tracing::info!(
                workspace_id,
                user_id,
                by,
                role = role.as_str(),
                "what a member is in a workspace was changed"
            );
            return self.workspace_member(workspace_id, user_id).await;
        }
        // Changed by somebody else every time it was tried.
        Err(AppError::conflict(CHANGED_MEANWHILE))
    }

    /// Takes a member out of the workspace, on behalf of `by`: its workspace is not
    /// there for them from then on, and their roles on its boards go with them. What
    /// they wrote stays, under their name. A board that would be left without an admin
    /// passes to `by`.
    ///
    /// # Errors
    ///
    /// Returns a 404 for somebody who is no member of the workspace, a 403 for `by`
    /// themselves (somebody else takes an administrator out), and a 409 (reason
    /// `last_admin`) if the workspace would be left without an administrator.
    pub async fn remove_workspace_member(
        &self,
        workspace_id: &str,
        user_id: &str,
        by: &str,
    ) -> Result<(), AppError> {
        self.workspace_member(workspace_id, user_id).await?;
        if user_id == by {
            return Err(AppError::forbidden(
                "another administrator takes an administrator out of the workspace",
            ));
        }
        self.collab()
            .narrowing_access(self, &AccessScope::workspace(workspace_id).of_user(user_id));
        let change = self
            .store()
            .remove_workspace_member(workspace_id, user_id, by)
            .await?;
        let taken_over = self.settle(workspace_id, by, change)?;
        self.ws().workspace_role_changed(workspace_id, user_id);
        self.recheck_member(workspace_id, user_id, by, &taken_over)
            .await;
        tracing::info!(
            workspace_id,
            user_id,
            by,
            "a member was taken out of a workspace"
        );
        Ok(())
    }

    /// The rooms of the workspace ask again about the member whose place in it changed,
    /// and those of the boards that passed to `by` (their only admin was the member)
    /// about `by`, who may write there now.
    async fn recheck_member(
        &self,
        workspace_id: &str,
        user_id: &str,
        by: &str,
        taken_over: &[String],
    ) {
        self.collab()
            .recheck_access(self, AccessScope::workspace(workspace_id).of_user(user_id))
            .await;
        for container_id in taken_over {
            self.collab()
                .recheck_access(self, AccessScope::container(container_id).of_user(by))
                .await;
        }
    }

    /// One member of the workspace; 404 for somebody who is none.
    async fn workspace_member(
        &self,
        workspace_id: &str,
        user_id: &str,
    ) -> Result<WorkspaceMember, AppError> {
        if !is_id(user_id) {
            return Err(no_such_member());
        }
        self.store()
            .get_workspace_member(workspace_id, user_id)
            .await?
            .ok_or_else(no_such_member)
    }

    /// What the store made of a change, as the client is told it; the boards that
    /// passed to `by` are announced to who is on them.
    fn settle(
        &self,
        workspace_id: &str,
        by: &str,
        change: MembershipChange,
    ) -> Result<Vec<String>, AppError> {
        match change {
            MembershipChange::Done { taken_over } => {
                for container_id in taken_over.iter().cloned() {
                    tracing::info!(
                        workspace_id,
                        container_id,
                        by,
                        "a board passed to who manages the workspace"
                    );
                    let admin = ContainerMember::new(container_id, by, ContainerRole::Admin);
                    self.broadcast_member_change_in_background(workspace_id.to_owned(), admin);
                }
                Ok(taken_over)
            }
            MembershipChange::NotAMember => Err(no_such_member()),
            MembershipChange::RoleIs(_) | MembershipChange::Unsettled => {
                Err(AppError::conflict(CHANGED_MEANWHILE))
            }
            MembershipChange::LastAdmin => {
                Err(AppError::conflict(LAST_ADMIN).with_reason(Reason::LastAdmin))
            }
        }
    }
}
