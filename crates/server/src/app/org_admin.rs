//! What an organization's administrators do to it: its name and public links, the
//! roles of its members, its workspaces and their trash, and stepping into a workspace
//! as its administrator. And the audit logs that record it, for whom they are kept.
//!
//! The application here is the one the route of the organization entered
//! (`App::enter_org_as_admin`), reaching the organization's rows.

use std::collections::HashMap;

use fvoci_model::{
    AuditAction, AuditRecord, AuthParamError, ContainerKinds, Named, OrgMember, OrgRole,
    OrgWorkspace, Organization, OrganizationPatch, Workspace, is_id, new_id, organization_name,
    required_name,
};
use fvoci_store::{
    AuditPlace, Joining, MemberRemoval, OrgRoleChange, StoreError, Taken, WorkspaceTrashing,
};

use super::App;
use super::list_page::ListPage;
use crate::collab::AccessScope;
use crate::error::{AppError, Reason};

/// The name of a workspace as it is stored: trimmed, non-empty, within the name length
/// ceiling, and free of control, invisible, and bidirectional override characters.
fn workspace_name(name: &str) -> Result<String, AppError> {
    required_name(name, Named::Workspace).map_err(AppError::from)
}

fn name_taken() -> AppError {
    AppError::conflict("another workspace of the organization has the name")
        .with_reason(Reason::WorkspaceNameTaken)
        .with_field("name")
}

fn workspace_not_found() -> AppError {
    AppError::not_found("workspace")
}

/// A workspace named by text that is no identifier is not there, as one nobody has.
fn known_workspace(workspace_id: &str) -> Result<(), AppError> {
    if is_id(workspace_id) {
        Ok(())
    } else {
        Err(workspace_not_found())
    }
}

impl App {
    /// The organization of the application.
    ///
    /// # Errors
    ///
    /// Returns a 404 if it is not there, and the store's error if it cannot be read.
    pub async fn organization(&self) -> Result<Organization, AppError> {
        match self.store().organization().await {
            Ok(organization) => Ok(organization),
            Err(error) if error.is_not_found() => Err(AppError::not_found("organization")),
            Err(error) => Err(error.into()),
        }
    }

    /// Changes the organization's name or whether its boards may be read by a public
    /// link, on behalf of `actor_id`. Turned off, the links of its boards close at once,
    /// for whoever follows a board through one too.
    ///
    /// # Errors
    ///
    /// Returns a 400 for a name that violates the organization name character or length
    /// rules, and the store's error.
    pub async fn update_organization(
        &self,
        patch: &OrganizationPatch,
        actor_id: &str,
    ) -> Result<Organization, AppError> {
        let mut patch = patch.clone();
        if let Some(name) = &mut patch.name {
            let trimmed = name.trim();
            // Empty stays allowed, as the contract says; anything else is named as on sign-up.
            *name = if trimmed.is_empty() {
                String::new()
            } else {
                organization_name(trimmed).map_err(|error| {
                    AppError::from(AuthParamError::new(
                        "name",
                        error.reason(),
                        error.to_string(),
                    ))
                })?
            };
        }
        if patch.public_links == Some(false) {
            self.collab()
                .narrowing_access(self, &AccessScope::organization());
        }
        let organization = match self.store().update_organization(&patch, actor_id).await {
            Ok(organization) => organization,
            Err(error) if error.is_not_found() => {
                return Err(AppError::not_found("organization"));
            }
            Err(error) => return Err(error.into()),
        };
        if !organization.public_links {
            self.end_shared_readers(None).await?;
        }
        Ok(organization)
    }

    /// Ends what connections follow of a board of the organization, or of one of its
    /// workspaces, by its read token, and tells them: its links open nothing now.
    pub(super) async fn end_shared_readers(
        &self,
        workspace_id: Option<&str>,
    ) -> Result<(), AppError> {
        for container_id in self.store().public_link_container_ids(workspace_id).await? {
            self.ws().end_readers_of_container(&container_id);
        }
        Ok(())
    }

    /// One page of the organization's members, the oldest member first.
    ///
    /// # Errors
    ///
    /// Returns the store's error if they cannot be read.
    pub async fn org_members(&self, asked: ListPage) -> Result<Vec<OrgMember>, AppError> {
        let paging = asked.paging();
        Ok(self.store().org_members(paging).await?)
    }

    /// Gives a member of the organization a role in it, on behalf of `actor_id`. Who is
    /// its administrator no more is taken out of the workspaces they joined as one.
    ///
    /// # Errors
    ///
    /// Returns a 404 if the user is no member, a 409 with the reason `last_org_admin` if
    /// they are its last administrator and would stop being one, and the store's error.
    pub async fn set_org_role(
        &self,
        user_id: &str,
        role: OrgRole,
        actor_id: &str,
    ) -> Result<(), AppError> {
        if !is_id(user_id) {
            return Err(AppError::not_found("member"));
        }
        if role == OrgRole::Member {
            self.collab()
                .narrowing_access(self, &AccessScope::organization().of_user(user_id));
        }
        match self.store().set_org_role(user_id, role, actor_id).await {
            Ok(OrgRoleChange::Done) => {
                if role == OrgRole::Member {
                    // What they joined is theirs no more, and what happens there neither.
                    self.ws().follow_only_own_workspaces(user_id).await;
                }
                self.collab()
                    .recheck_access(self, AccessScope::organization().of_user(user_id))
                    .await;
                Ok(())
            }
            Ok(OrgRoleChange::NotAMember) => Err(AppError::not_found("member")),
            Ok(OrgRoleChange::LastAdmin) => Err(AppError::conflict(
                "the organization must keep an administrator",
            )
            .with_reason(Reason::LastOrgAdmin)),
            Err(error) if error.is_not_found() => Err(AppError::not_found("organization")),
            Err(error) => Err(error.into()),
        }
    }

    /// Takes a member out of the organization on behalf of `actor_id`
    /// ([`fvoci_store::Store::remove_org_member`]), and tells their open connections
    /// and rooms (`App::after_leaving`). Does not require `user_id` to have a password.
    ///
    /// # Errors
    ///
    /// Returns a 404 (`organization`) if `actor_id` no longer administers the
    /// organization, a 404 (`member`) if `user_id` is no member, a 409 with the reason
    /// `cannot_remove_self`, `last_instance_admin`, `sole_org_admin`, or
    /// `alone_in_organization` for what stands in the way, and the store's error.
    pub async fn remove_org_member(
        &self,
        org_id: &str,
        user_id: &str,
        actor_id: &str,
    ) -> Result<(), AppError> {
        if !is_id(user_id) {
            return Err(AppError::not_found("member"));
        }
        self.collab()
            .narrowing_access(self, &AccessScope::organization().of_user(user_id));
        match self.store().remove_org_member(user_id, actor_id).await? {
            MemberRemoval::Removed(left) => {
                tracing::info!(
                    org_id,
                    user_id,
                    actor_id,
                    "a member was taken out of an organization"
                );
                self.after_leaving(org_id, user_id, &left).await;
                Ok(())
            }
            MemberRemoval::NotAnAdmin => Err(AppError::not_found("organization")),
            MemberRemoval::NotAMember => Err(AppError::not_found("member")),
            MemberRemoval::CannotRemoveSelf => Err(AppError::conflict(
                "an administrator cannot remove themselves: use leaveOrganization instead",
            )
            .with_reason(Reason::CannotRemoveSelf)),
            MemberRemoval::Refused(refused) => Err(super::organizations::removal_refusal(refused)),
        }
    }

    /// Every workspace of the organization, in use and in the trash, with what the
    /// administrator `viewer_id` is in each. Nothing of what is in them.
    ///
    /// # Errors
    ///
    /// Returns the store's error if they cannot be read.
    pub async fn org_workspaces(&self, viewer_id: &str) -> Result<Vec<OrgWorkspace>, AppError> {
        Ok(self.store().org_workspaces(viewer_id).await?)
    }

    /// The workspace of the organization as its administrators list it.
    async fn org_workspace(
        &self,
        viewer_id: &str,
        workspace_id: &str,
    ) -> Result<OrgWorkspace, AppError> {
        self.org_workspaces(viewer_id)
            .await?
            .into_iter()
            .find(|workspace| workspace.id == workspace_id)
            .ok_or_else(workspace_not_found)
    }

    /// Makes a workspace of the organization, on behalf of `actor_id`. Nobody is a
    /// member of it, its maker neither: an administrator of the organization joins it
    /// to work in it.
    ///
    /// # Errors
    ///
    /// Returns a 400 for a name that is no name, a 409 with the reason
    /// `workspace_name_taken` if another workspace in use has it, and the store's error.
    pub async fn create_org_workspace(
        &self,
        name: &str,
        actor_id: &str,
    ) -> Result<OrgWorkspace, AppError> {
        let workspace = Workspace {
            id: new_id(),
            title: workspace_name(name)?,
            modified_by: actor_id.to_owned(),
            ..Workspace::default()
        };
        match self
            .store()
            .create_named_workspace(&workspace, actor_id)
            .await
        {
            Ok(()) => self.org_workspace(actor_id, &workspace.id).await,
            Err(StoreError::Conflict(Taken::WorkspaceName)) => Err(name_taken()),
            Err(error) => Err(error.into()),
        }
    }

    /// Renames a workspace of the organization that is in use, on behalf of `actor_id`.
    ///
    /// # Errors
    ///
    /// As [`App::create_org_workspace`], and a 404 if there is no such workspace in use.
    pub async fn rename_org_workspace(
        &self,
        workspace_id: &str,
        name: &str,
        actor_id: &str,
    ) -> Result<OrgWorkspace, AppError> {
        known_workspace(workspace_id)?;
        let name = workspace_name(name)?;
        match self
            .store()
            .rename_workspace(workspace_id, &name, actor_id)
            .await
        {
            Ok(()) => self.org_workspace(actor_id, workspace_id).await,
            Err(StoreError::Conflict(Taken::WorkspaceName)) => Err(name_taken()),
            Err(error) if error.is_not_found() => Err(workspace_not_found()),
            Err(error) => Err(error.into()),
        }
    }

    /// Puts a workspace of the organization in the trash, on behalf of `actor_id`: it is
    /// there for nobody, its links open nothing, and it goes for good when it has been
    /// in the trash as long as a board is kept there.
    ///
    /// # Errors
    ///
    /// Returns a 404 if there is no such workspace in use, a 409 with the reason
    /// `default_workspace` for the organization's default workspace, and the store's
    /// error.
    pub async fn trash_org_workspace(
        &self,
        workspace_id: &str,
        actor_id: &str,
    ) -> Result<(), AppError> {
        known_workspace(workspace_id)?;
        self.collab()
            .narrowing_access(self, &AccessScope::workspace(workspace_id));
        match self.store().trash_workspace(workspace_id, actor_id).await {
            Ok(WorkspaceTrashing::Done) => {
                self.ws().stop_following_workspace(workspace_id);
                // The rooms ask again even if its links cannot be read now.
                let readers = self.end_shared_readers(Some(workspace_id)).await;
                self.collab()
                    .recheck_access(self, AccessScope::workspace(workspace_id))
                    .await;
                readers?;
                Ok(())
            }
            Ok(WorkspaceTrashing::NotFound) => Err(workspace_not_found()),
            Ok(WorkspaceTrashing::Default) => Err(AppError::conflict(
                "the organization's default workspace cannot be deleted",
            )
            .with_reason(Reason::DefaultWorkspace)),
            Err(error) if error.is_not_found() => Err(AppError::not_found("organization")),
            Err(error) => Err(error.into()),
        }
    }

    /// Brings a workspace of the organization back from the trash as it was, on behalf
    /// of `actor_id`.
    ///
    /// # Errors
    ///
    /// Returns a 404 if no such workspace is in the trash, a 409 with the reason
    /// `workspace_name_taken` if a workspace in use took its name meanwhile, and the
    /// store's error.
    pub async fn restore_org_workspace(
        &self,
        workspace_id: &str,
        actor_id: &str,
    ) -> Result<OrgWorkspace, AppError> {
        known_workspace(workspace_id)?;
        match self.store().restore_workspace(workspace_id, actor_id).await {
            Ok(()) => self.org_workspace(actor_id, workspace_id).await,
            Err(StoreError::Conflict(Taken::WorkspaceName)) => Err(name_taken()),
            Err(error) if error.is_not_found() => Err(workspace_not_found()),
            Err(error) => Err(error.into()),
        }
    }

    /// Makes the administrator `user_id` of the organization an administrator of one of
    /// its workspaces in use, as the organization's, which the workspace's members see
    /// recorded. Nothing changes for one who is its administrator already.
    ///
    /// # Errors
    ///
    /// Returns a 404 if there is no such workspace in use, or the user administers the
    /// organization no more, and the store's error.
    pub async fn join_org_workspace(
        &self,
        workspace_id: &str,
        user_id: &str,
    ) -> Result<OrgWorkspace, AppError> {
        known_workspace(workspace_id)?;
        match self
            .store()
            .join_workspace_as_org_admin(workspace_id, user_id)
            .await?
        {
            Joining::Joined => {
                self.ws().workspace_role_changed(workspace_id, user_id);
                self.collab()
                    .recheck_access(self, AccessScope::workspace(workspace_id).of_user(user_id))
                    .await;
                self.org_workspace(user_id, workspace_id).await
            }
            Joining::Already => {
                self.collab()
                    .recheck_access(self, AccessScope::workspace(workspace_id).of_user(user_id))
                    .await;
                self.org_workspace(user_id, workspace_id).await
            }
            Joining::NotFound => Err(workspace_not_found()),
            Joining::NoOrgAdmin => Err(AppError::not_found("organization")),
        }
    }

    /// Takes the administrator `user_id` of the organization out of a workspace they
    /// joined as one: they are what they were in it before, or no member, which the
    /// workspace's members see recorded. Nothing changes for one who did not join it so.
    ///
    /// # Errors
    ///
    /// Returns a 404 if the organization has no such workspace, and the store's error.
    pub async fn leave_org_workspace(
        &self,
        workspace_id: &str,
        user_id: &str,
    ) -> Result<OrgWorkspace, AppError> {
        known_workspace(workspace_id)?;
        self.collab()
            .narrowing_access(self, &AccessScope::workspace(workspace_id).of_user(user_id));
        if self
            .store()
            .leave_workspace_as_org_admin(workspace_id, user_id)
            .await?
        {
            self.ws().follow_only_own_workspaces(user_id).await;
            self.collab()
                .recheck_access(self, AccessScope::workspace(workspace_id).of_user(user_id))
                .await;
        }
        self.org_workspace(user_id, workspace_id).await
    }

    /// One page of the records of the organization itself, the newest first.
    ///
    /// # Errors
    ///
    /// Returns the store's error if they cannot be read.
    pub async fn org_audit(&self, asked: ListPage) -> Result<Vec<AuditRecord>, AppError> {
        let paging = asked.paging();
        Ok(self
            .store()
            .audit_records(AuditPlace::Organization, paging)
            .await?)
    }

    /// One page of the records of a workspace, the newest first, as the member
    /// `reader_id` reads them: the title of a board is told where the reader sees the
    /// board, and of no other.
    ///
    /// # Errors
    ///
    /// Returns the store's error if they cannot be read.
    pub async fn workspace_audit(
        &self,
        reader_id: &str,
        workspace_id: &str,
        asked: ListPage,
    ) -> Result<Vec<AuditRecord>, AppError> {
        let paging = asked.paging();
        let mut records = self
            .store()
            .audit_records(AuditPlace::Workspace(workspace_id), paging)
            .await?;
        let mut titles: HashMap<String, Option<String>> = HashMap::new();
        // A user is added to a board or to a wiki alike.
        let every_kind = self.clone().reaching(ContainerKinds::EVERY);
        for record in &mut records {
            if !matches!(
                record.action,
                AuditAction::SelfAddedToContainer | AuditAction::AddedToContainer
            ) {
                continue;
            }
            if !titles.contains_key(&record.target_id) {
                let title = every_kind
                    .container_access(reader_id, &record.target_id)
                    .await?
                    .filter(|access| {
                        access.grant.sees() && access.container.workspace_id == workspace_id
                    })
                    .map(|access| access.container.title);
                titles.insert(record.target_id.clone(), title);
            }
            record.target_name = titles.get(&record.target_id).cloned().flatten();
        }
        Ok(records)
    }

    /// Whether the boards of the application's organization may be read by a public
    /// link.
    ///
    /// # Errors
    ///
    /// Returns the store's error if the organization cannot be read.
    pub async fn public_links_on(&self) -> Result<bool, AppError> {
        Ok(self.organization().await?.public_links)
    }
}
