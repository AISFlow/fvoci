//! Container memberships: listing, adding, joining, updating, and removing members.

use fvoci_model::{
    AuditAction, Container, ContainerMember, ContainerRole, ContainerVisibility, WorkspaceRole,
};
use fvoci_store::{AddedToContainerNotice, AuditEntry, Joined, MemberChange, Standing};

use super::{App, ContainerAccess};
use crate::collab::AccessScope;
use crate::error::{AppError, OrMissing, Reason};
use crate::permissions::{self, ContainerPermission};

/// The message of the refusal to remove or demote a board's only admin.
const LAST_ADMIN: &str = "cannot leave a board with no admins";

impl App {
    /// Returns the explicit members of a board.
    ///
    /// # Errors
    ///
    /// Returns the store's error if the query fails.
    pub async fn members_for_container(
        &self,
        container_id: &str,
    ) -> Result<Vec<ContainerMember>, AppError> {
        Ok(self.store().get_members_for_container(container_id).await?)
    }

    /// Returns the members of a board who are in its workspace now, as the board's member
    /// list shows them: who left the workspace is listed nowhere, whatever role is left.
    ///
    /// # Errors
    ///
    /// Returns the store's error if the members cannot be read.
    pub async fn present_members_for_container(
        &self,
        container_id: &str,
    ) -> Result<Vec<ContainerMember>, AppError> {
        Ok(self
            .store()
            .get_present_members_for_container(container_id)
            .await?)
    }

    /// Makes a user a member of a board with the roles of `member`, files the board
    /// under the user's default category unless it is a template, and tells the board's
    /// members.
    ///
    /// A user who is a member already stays as they are, and that membership is
    /// returned. `None` is returned if the board does not exist.
    ///
    /// # Errors
    ///
    /// Returns the store's error if the membership cannot be read or written, or an
    /// error if the board cannot be filed under the user's default category; in that
    /// case the membership exists nonetheless.
    pub async fn add_member_to_container(
        &self,
        member: &ContainerMember,
    ) -> Result<Option<ContainerMember>, AppError> {
        self.add_member_to_container_recorded(member, Word::Unasked)
            .await
    }

    /// Makes a user a member of a board as [`App::add_member_to_container`] does, on the
    /// word of who asked, whose permission was checked with `access`: only if they are
    /// still what it found when the membership is written (`Store::add_member_asked`).
    ///
    /// # Errors
    ///
    /// As [`App::add_member_to_container`]; `None` also if who asked is not what they were.
    pub async fn add_member_asked(
        &self,
        member: &ContainerMember,
        user_id: &str,
        access: &ContainerAccess,
    ) -> Result<Option<ContainerMember>, AppError> {
        let word = Word::Asked(standing(user_id, access));
        self.add_member_to_container_recorded(member, word).await
    }

    /// Makes a user a member of a board, on behalf of `actor_id`, who manages the board
    /// without reading it, as [`App::add_member_to_container`] does and, if the membership
    /// is new, records in the organization's audit log in the same step who made whom a
    /// member: themselves, by which they come to read it, or somebody else, who might be
    /// another account of theirs.
    ///
    /// # Errors
    ///
    /// As [`App::add_member_to_container`]; a record that cannot be written is the store's
    /// error, and the membership is not made then.
    pub async fn add_member_as_manager(
        &self,
        member: &ContainerMember,
        actor_id: &str,
    ) -> Result<Option<ContainerMember>, AppError> {
        self.add_member_to_container_recorded(member, Word::Manager(actor_id))
            .await
    }

    /// Adds a member as [`App::add_member_to_container`] does and, if the membership is new
    /// and a manager is given, records the manager doing it in the audit log in the same
    /// step.
    async fn add_member_to_container_recorded(
        &self,
        member: &ContainerMember,
        word: Word<'_>,
    ) -> Result<Option<ContainerMember>, AppError> {
        let Some(container) = self
            .store()
            .get_container(&member.container_id)
            .await
            .found()?
        else {
            return Ok(None);
        };

        // Added unless already there, in one step: of two requests that add the same
        // user at once, the second finds the membership the first made.
        let added = match word {
            Word::Manager(manager_id) => {
                let entry = audit_entry(&container, member, manager_id);
                self.store().add_member_recorded(member, &entry).await
            }
            Word::Asked(asked) => self.store().add_member_asked(member, asked).await,
            Word::Unasked => self.store().add_member(member).await,
        };
        let existing = match added {
            Ok(existing) => existing,
            // The board, or who asked as they were, is not there any more.
            Err(error) if error.is_not_found() && matches!(word, Word::Asked(_)) => {
                return Ok(None);
            }
            Err(error) => return Err(error.into()),
        };
        if let Some(existing) = existing {
            return Ok(Some(existing));
        }

        self.collab()
            .recheck_access(
                self,
                AccessScope::container(&container.id).of_user(&member.user_id),
            )
            .await;
        // The membership is stored: it is announced whatever becomes of the filing.
        self.broadcast_member_change_in_background(container.workspace_id.clone(), member.clone());
        if !container.is_template {
            if let Some(actor_id) = word.actor_id().filter(|actor| *actor != member.user_id)
                && let Err(error) = self
                    .store()
                    .note_added_to_container(AddedToContainerNotice {
                        container_id: &container.id,
                        recipient_id: &member.user_id,
                        actor_id,
                    })
                    .await
            {
                tracing::warn!(
                    container_id = %container.id,
                    recipient_id = %member.user_id,
                    %error,
                    "failed to note added_to_container notification"
                );
            }
            self.add_containers_to_default_category(
                &member.user_id,
                &container.workspace_id,
                std::slice::from_ref(&container.id),
            )
            .await?;
        }
        Ok(Some(member.clone()))
    }

    /// Verifies that `actor_id` is permitted to add members to `container_id` before a
    /// handler decodes the request body (`404` when hidden, `403` otherwise).
    ///
    /// # Errors
    ///
    /// Returns [`AppError`] (`404` or `403`) if `actor_id` cannot add members, or the
    /// store's error if access cannot be read.
    pub async fn require_add_member_permission(
        &self,
        actor_id: &str,
        container_id: &str,
        denial: &str,
    ) -> Result<(), AppError> {
        let access = self
            .container_access(actor_id, container_id)
            .await?
            .ok_or_else(|| AppError::not_found("board"))?;
        MemberGiver::of(&access, denial).map(|_| ())
    }

    /// Adds `target_user_id` to `container_id` on behalf of `actor_id` using the domain
    /// member-granting policy:
    /// - Role managers (`ManageRoles`) must supply `requested_role`.
    /// - Open-container editors who work in the workspace (`ManageProperties` + `Open` +
    ///   non-guest) grant `container.minimum_role` (and asking for `Admin` is 403).
    /// - Workspace guests cannot hold roles above `Editor` (`403 guest_role_limit`).
    /// - Workspace managers who do not read the container are recorded in the audit log.
    ///
    /// # Errors
    ///
    /// Returns [`AppError`] on permission denial, invalid user/role, or store failure.
    pub async fn add_member(
        &self,
        actor_id: &str,
        container_id: &str,
        target_user_id: &str,
        requested_role: Option<ContainerRole>,
        denial: &str,
    ) -> Result<ContainerMember, AppError> {
        let access = self
            .container_access(actor_id, container_id)
            .await?
            .ok_or_else(|| AppError::not_found("board"))?;
        let giver = MemberGiver::of(&access, denial)?;
        if target_user_id.is_empty() {
            return Err(AppError::bad_request("empty userId").with_field("userId"));
        }
        // The target user must exist before a membership is created. A lookup that
        // failed is no answer to whether the user exists.
        match self.user(target_user_id).await {
            Ok(_) => {}
            Err(error) if error.is_not_found() => {
                return Err(AppError::bad_request("no such user").with_field("userId"));
            }
            Err(error) => return Err(error),
        }
        // A board takes the people of its workspace, its guests too. Anybody else is, to
        // who adds members, a user who does not exist: the answer is the same, so that it
        // tells nothing of who has an account elsewhere on the server.
        let Some(target_workspace_role) = self
            .actor(target_user_id, &access.container.workspace_id)
            .await?
            .workspace
        else {
            return Err(AppError::bad_request("no such user").with_field("userId"));
        };
        let role = giver.role_granted(requested_role, access.container.minimum_role, denial)?;
        require_holdable_role(target_workspace_role, role)?;
        let member = ContainerMember::new(container_id, target_user_id, role);
        // A workspace's administrator who could not read the board and gives somebody a
        // role on it is recorded doing so: themselves, by which they come to read it, or
        // somebody else, who might be another account of theirs.
        let added = if access.grant.manages_only() {
            self.add_member_as_manager(&member, actor_id).await?
        } else {
            self.add_member_asked(&member, actor_id, &access).await?
        };
        added.ok_or_else(|| AppError::not_found("board"))
    }

    /// Updates `member_user_id`'s direct role on `container_id` on behalf of `actor_id`,
    /// enforcing role-management permission, workspace membership presence, guest role
    /// limits, last-admin protection, and manager-only audit logging.
    ///
    /// # Errors
    ///
    /// Returns [`AppError`] on permission denial, missing member/role, `last_admin`, or
    /// store failure.
    pub async fn update_member(
        &self,
        actor_id: &str,
        container_id: &str,
        member_user_id: &str,
        role: Option<ContainerRole>,
        denial: &str,
    ) -> Result<ContainerMember, AppError> {
        let asker = self.container_access(actor_id, container_id).await?;
        let grant = asker
            .as_ref()
            .map_or(crate::permissions::Grant::NONE, |access| access.grant);
        if !grant.allows(ContainerPermission::ManageRoles) {
            return Err(if grant.hides(ContainerPermission::ManageRoles) {
                AppError::not_found("board")
            } else {
                AppError::forbidden(denial)
            });
        }
        let target = self
            .container_access(member_user_id, container_id)
            .await?
            .ok_or_else(|| AppError::not_found("board"))?;
        let role = role.ok_or_else(|| AppError::bad_request("missing role").with_field("role"))?;
        // Who is no member of the board is not found, and so is who has left its
        // workspace: the role left there opens nothing, and is not to be changed.
        let (Some(_), Some(workspace_role)) = (target.member, target.actor.workspace) else {
            return Err(AppError::not_found("member"));
        };
        require_holdable_role(workspace_role, role)?;
        let member = ContainerMember::new(container_id, member_user_id, role);
        let updated = if grant.manages_only() {
            self.update_member_as_manager(&member, actor_id).await?
        } else {
            let asker = asker.ok_or_else(|| AppError::not_found("board"))?;
            self.update_member_asked(&member, actor_id, &asker).await?
        };
        updated.ok_or_else(|| AppError::not_found("member"))
    }

    /// Makes a user a member of a board with the board's minimum role, as joining does,
    /// and returns the membership. A member below the minimum role is raised to it; a
    /// member with that role or a higher one stays as they are. The board's members are
    /// told about a membership that is new or changed.
    ///
    /// # Errors
    ///
    /// Returns a not-found error if there is no such board, and an error if the board
    /// cannot be filed under the user's default category; in that case the membership
    /// exists nonetheless.
    pub async fn join_container(
        &self,
        container_id: &str,
        user_id: &str,
    ) -> Result<ContainerMember, AppError> {
        let container = self.container(container_id).await?;
        let joined = self
            .store()
            .join_container(container_id, user_id)
            .await
            .or_missing("board")?;
        match joined {
            Joined::New(member) => {
                self.collab()
                    .recheck_access(self, AccessScope::container(&container.id).of_user(user_id))
                    .await;
                // The membership is stored: it is announced whatever becomes of the
                // filing.
                self.broadcast_member_change_in_background(
                    container.workspace_id.clone(),
                    member.clone(),
                );
                if !container.is_template {
                    self.add_containers_to_default_category(
                        user_id,
                        &container.workspace_id,
                        std::slice::from_ref(&container.id),
                    )
                    .await?;
                }
                Ok(member)
            }
            Joined::Raised(member) => {
                self.collab()
                    .recheck_access(self, AccessScope::container(&container.id).of_user(user_id))
                    .await;
                self.broadcast_member_change_in_background(container.workspace_id, member.clone());
                Ok(member)
            }
            Joined::Already(member) => Ok(member),
        }
    }

    /// Replaces the roles of a member of a board and tells the board's members.
    ///
    /// `None` is returned, and nothing is changed, if the board does not exist or the
    /// user is not a member of it.
    ///
    /// # Errors
    ///
    /// Returns a bad request if the change would leave the board without an admin, or
    /// the store's error.
    pub async fn update_container_member(
        &self,
        member: &ContainerMember,
    ) -> Result<Option<ContainerMember>, AppError> {
        self.update_container_member_recorded(member, Word::Unasked)
            .await
    }

    /// Replaces the roles of a member of a board as [`App::update_container_member`] does,
    /// on the word of who asked, as [`App::add_member_asked`] takes it.
    ///
    /// # Errors
    ///
    /// As [`App::update_container_member`]; `None` also if who asked is not what they were.
    pub async fn update_member_asked(
        &self,
        member: &ContainerMember,
        user_id: &str,
        access: &ContainerAccess,
    ) -> Result<Option<ContainerMember>, AppError> {
        let word = Word::Asked(standing(user_id, access));
        self.update_container_member_recorded(member, word).await
    }

    /// Gives a member of a board roles on it, on behalf of `actor_id`, who manages the
    /// board without reading it, as [`App::update_container_member`] does and records in the
    /// organization's audit log in the same step who gave whom the roles, as
    /// [`App::add_member_as_manager`] does.
    ///
    /// # Errors
    ///
    /// As [`App::update_container_member`]; a record that cannot be written is the store's
    /// error, and the membership is not changed then.
    pub async fn update_member_as_manager(
        &self,
        member: &ContainerMember,
        actor_id: &str,
    ) -> Result<Option<ContainerMember>, AppError> {
        self.update_container_member_recorded(member, Word::Manager(actor_id))
            .await
    }

    async fn update_container_member_recorded(
        &self,
        member: &ContainerMember,
        word: Word<'_>,
    ) -> Result<Option<ContainerMember>, AppError> {
        let Some(container) = self
            .store()
            .get_container(&member.container_id)
            .await
            .found()?
        else {
            return Ok(None);
        };
        let scope = AccessScope::container(&container.id).of_user(&member.user_id);
        self.collab().narrowing_access(self, &scope);
        // A board keeps at least one admin. Whether this change would take the last one
        // away is decided where the change is written, so that two admins who step down
        // at once do not both pass.
        let changed = match word {
            Word::Manager(manager_id) => {
                let entry = audit_entry(&container, member, manager_id);
                self.store().change_member_recorded(member, &entry).await
            }
            Word::Asked(asked) => self.store().change_member_asked(member, asked).await,
            Word::Unasked => self.store().change_member(member).await,
        };
        let change = match changed {
            Ok(change) => change,
            // The board, or who asked as they were, is not there any more.
            Err(error) if error.is_not_found() && matches!(word, Word::Asked(_)) => {
                return Ok(None);
            }
            Err(error) => return Err(error.into()),
        };
        match change {
            MemberChange::Done => {}
            MemberChange::NotAMember => return Ok(None),
            MemberChange::LastAdmin => {
                return Err(AppError::conflict(LAST_ADMIN).with_reason(Reason::LastAdmin));
            }
        }

        self.collab()
            .recheck_access(
                self,
                AccessScope::container(&container.id).of_user(&member.user_id),
            )
            .await;
        self.broadcast_member_change_in_background(container.workspace_id, member.clone());
        Ok(Some(member.clone()))
    }

    pub(super) fn broadcast_member_change_in_background(
        &self,
        workspace_id: String,
        member: ContainerMember,
    ) {
        let app = self.clone();
        self.spawn_background(async move {
            app.ws()
                .broadcast_member_change(&workspace_id, &member.container_id, &member)
                .await;
        });
    }

    /// Removes a user from a board and tells the board's members and the user. Nothing
    /// happens if the board does not exist or the user is not a member of it.
    ///
    /// # Errors
    ///
    /// Returns a bad request if the user is the board's only admin, or the store's
    /// error.
    pub async fn delete_container_member(
        &self,
        container_id: &str,
        user_id: &str,
    ) -> Result<(), AppError> {
        let Some(container) = self.store().get_container(container_id).await.found()? else {
            return Ok(());
        };
        let scope = AccessScope::container(container_id).of_user(user_id);
        self.collab().narrowing_access(self, &scope);
        // A board keeps at least one admin; decided where the membership is removed.
        match self.store().remove_member(container_id, user_id).await? {
            MemberChange::Done => {}
            MemberChange::NotAMember => return Ok(()),
            MemberChange::LastAdmin => {
                return Err(AppError::conflict(LAST_ADMIN).with_reason(Reason::LastAdmin));
            }
        }

        self.collab()
            .recheck_access(self, AccessScope::container(container_id).of_user(user_id))
            .await;
        let app = self.clone();
        let user_id = user_id.to_owned();
        self.spawn_background(async move {
            app.ws()
                .broadcast_member_delete(&container.workspace_id, &container.id, &user_id)
                .await;
        });
        Ok(())
    }
}

/// On whose word a board's membership changes, and what is checked of them where it is
/// written.
#[derive(Clone, Copy)]
enum Word<'a> {
    /// Nobody's that is to be checked again: the server's own (an import).
    Unasked,
    /// A user's whose permission was checked: they must still be what it found.
    Asked(Standing<'a>),
    /// A workspace's administrator's who manages the board without reading it: they must
    /// still administer the workspace, and are recorded.
    Manager(&'a str),
}

impl<'a> Word<'a> {
    fn actor_id(self) -> Option<&'a str> {
        match self {
            Self::Unasked => None,
            Self::Asked(standing) => Some(standing.user_id),
            Self::Manager(manager_id) => Some(manager_id),
        }
    }
}

/// How the caller is permitted to add members to a container.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MemberGiver {
    /// Holds `ManageRoles`: chooses the explicit role.
    RoleManager,
    /// Open-container editor who works in the workspace: grants `minimum_role` only.
    OpenEditor,
}

impl MemberGiver {
    fn of(access: &ContainerAccess, denial: &str) -> Result<Self, AppError> {
        if access.grant.allows(ContainerPermission::ManageRoles) {
            return Ok(Self::RoleManager);
        }
        // A guest's editing adds nobody: who is in the workspace is not a guest's to learn.
        let open_editor = access.grant.allows(ContainerPermission::ManageProperties)
            && access.container.visibility == ContainerVisibility::Open
            && access.actor.works_there();
        if open_editor {
            return Ok(Self::OpenEditor);
        }
        Err(if access.grant.hides(ContainerPermission::ManageRoles) {
            AppError::not_found("board")
        } else {
            AppError::forbidden(denial)
        })
    }

    fn role_granted(
        self,
        requested: Option<ContainerRole>,
        minimum_role: fvoci_model::MinimumRole,
        denial: &str,
    ) -> Result<ContainerRole, AppError> {
        match self {
            Self::RoleManager => {
                requested.ok_or_else(|| AppError::bad_request("missing role").with_field("role"))
            }
            Self::OpenEditor => {
                // Granting the admin role requires role-management permission; otherwise
                // an editor of an open board could promote a second account to admin and
                // take the board over. The web app offers a user who does not manage roles
                // the board's minimum role only: what joining gives.
                if requested == Some(ContainerRole::Admin) {
                    return Err(AppError::forbidden(denial));
                }
                Ok(ContainerRole::from(minimum_role))
            }
        }
    }
}

fn require_holdable_role(
    workspace_role: WorkspaceRole,
    role: ContainerRole,
) -> Result<(), AppError> {
    if permissions::may_hold(workspace_role, role) {
        Ok(())
    } else {
        Err(
            AppError::forbidden("a guest of the workspace is given no role above editor")
                .with_reason(Reason::GuestRoleLimit),
        )
    }
}

/// What `access` found of the user who asks.
fn standing<'a>(user_id: &'a str, access: &'a ContainerAccess) -> Standing<'a> {
    Standing {
        user_id,
        workspace_role: access.actor.workspace,
        member: access.member.as_ref(),
        team_role: access.team_role,
    }
}

/// The record of a manager of the board, who does not read it, giving `member` its
/// roles: themselves, or somebody else.
fn audit_entry(container: &Container, member: &ContainerMember, manager_id: &str) -> AuditEntry {
    if member.user_id == manager_id {
        AuditEntry::new(
            &container.workspace_id,
            manager_id,
            AuditAction::SelfAddedToContainer,
            &container.id,
        )
    } else {
        AuditEntry::new(
            &container.workspace_id,
            manager_id,
            AuditAction::AddedToContainer,
            &container.id,
        )
        .with_subject(&member.user_id)
    }
}
