//! What an organization's administrators change of it: its name and settings, the
//! roles of its members, and its workspaces (made, renamed, put in the trash, brought
//! back, removed for good). And their stepping into a workspace as its administrator,
//! and out again.
//!
//! Every statement here names the organization itself: the handle's
//! ([`Tenant::Org`](crate::Tenant::Org)). What is changed is recorded in the audit log
//! in the same transaction.

use fvoci_model::{
    AuditAction, OrgMember, OrgRole, OrgWorkspace, Organization, OrganizationPatch, Workspace,
    WorkspaceRole,
};

use crate::audit::{self, AuditActor, AuditEntry};
use crate::db::{Id, LockRank, OptId, OrganizationHeld, Row, RowLock, Scope};
use crate::error::{StoreError, Taken};
use crate::trash::{self, Purged, Trashed};
use crate::{Paging, Store, directory, joinings, person_names, users, workspaces};

/// What became of a change of a member's role in the organization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrgRoleChange {
    /// The member has the role now; the organizations's administrator they were no
    /// longer, they are out of the workspaces they had joined as one.
    Done,
    /// The user is no member of the organization.
    NotAMember,
    /// The member is the organization's last administrator, who stays one.
    LastAdmin,
}

/// What became of putting a workspace in the trash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceTrashing {
    /// It is in the trash.
    Done,
    /// The organization has no such workspace in use.
    NotFound,
    /// It is the organization's default workspace, which stays.
    Default,
}

/// What became of an administrator of the organization joining a workspace as its
/// administrator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Joining {
    /// They are its administrator now, as the organization's.
    Joined,
    /// They were its administrator already; nothing changed.
    Already,
    /// The organization has no such workspace in use.
    NotFound,
    /// The user administers the organization no longer.
    NoOrgAdmin,
}

impl Store {
    /// The handle's organization.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if there is no such organization in use.
    pub async fn organization(&self) -> Result<Organization, StoreError> {
        self.with_conn(organization).await
    }

    /// Changes the name of the handle's organization, and whether its boards may be read
    /// by a public link, copied out of it or given to personal API tokens, on behalf of
    /// `actor_id`; what the patch leaves out stays. A change of any of the three is
    /// recorded.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if there is no such organization in use.
    pub async fn update_organization(
        &self,
        patch: &OrganizationPatch,
        actor_id: &str,
    ) -> Result<Organization, StoreError> {
        self.transaction(async |scope| {
            let (before, _) = held_by_admin(scope, actor_id).await?;
            let changes_public_links = patch
                .public_links
                .is_some_and(|on| on != before.public_links);
            let changes_allow_copy_out = patch
                .allow_copy_out
                .is_some_and(|on| on != before.allow_copy_out);
            let changes_allow_api_tokens = patch
                .allow_api_tokens
                .is_some_and(|on| on != before.allow_api_tokens);
            let mut sql = scope.sql("UPDATE organizations SET updated_at = ");
            if !changes_public_links && !changes_allow_copy_out && !changes_allow_api_tokens {
                sql.changing_no_access();
            }
            sql.bind(scope.now_millis());
            if let Some(name) = &patch.name {
                sql.push_bind(", name = ", name);
            }
            if let Some(public_links) = patch.public_links {
                sql.push_bind(", public_links = ", public_links);
            }
            if let Some(allow_copy_out) = patch.allow_copy_out {
                sql.push_bind(", allow_copy_out = ", allow_copy_out);
            }
            if let Some(allow_api_tokens) = patch.allow_api_tokens {
                sql.push_bind(", allow_api_tokens = ", allow_api_tokens);
            }
            sql.push_bind(" WHERE id = ", scope.org()?);
            scope.execute(sql).await?;
            if let Some(on) = patch.public_links
                && on != before.public_links
            {
                let action = if on {
                    AuditAction::PublicLinksEnabled
                } else {
                    AuditAction::PublicLinksDisabled
                };
                let entry = AuditEntry::new("", actor_id, action, &before.id);
                audit::record_entry(scope, &entry).await?;
            }
            if let Some(on) = patch.allow_copy_out
                && on != before.allow_copy_out
            {
                let action = if on {
                    AuditAction::CopyOutEnabled
                } else {
                    AuditAction::CopyOutDisabled
                };
                let entry = AuditEntry::new("", actor_id, action, &before.id);
                audit::record_entry(scope, &entry).await?;
            }
            if let Some(on) = patch.allow_api_tokens
                && on != before.allow_api_tokens
            {
                let action = if on {
                    AuditAction::ApiTokensEnabled
                } else {
                    AuditAction::ApiTokensDisabled
                };
                let entry = AuditEntry::new("", actor_id, action, &before.id);
                audit::record_entry(scope, &entry).await?;
            }
            organization(scope).await
        })
        .await
    }

    /// Records in the handle's organization's audit log that `actor_id` copied
    /// `container_id` out to another organization or personal space (`detail`).
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] if the handle names none, and
    /// [`StoreError::Database`] if the statement fails.
    pub async fn record_container_copied_out(
        &self,
        actor_id: &str,
        container_id: &str,
        detail: &str,
    ) -> Result<(), StoreError> {
        let entry = AuditEntry::new("", actor_id, AuditAction::ContainerCopiedOut, container_id)
            .with_detail(detail);
        self.with_write_conn(async |scope| audit::record_entry(scope, &entry).await)
            .await
    }

    /// One page of the members of the handle's organization, the oldest member first:
    /// those whose account is in use, and those whose account waits to be deleted, who
    /// are members until it is erased.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn org_members(&self, paging: Paging) -> Result<Vec<OrgMember>, StoreError> {
        self.with_conn(async |scope| {
            // The name's columns are the user's alone: no other table has them.
            let mut sql = scope.sql(concat!(
                "SELECT om.user_id, u.username, u.display_name, om.role, ",
                person_names::name_columns!(),
                " FROM organization_members AS om JOIN users AS u ON u.id = om.user_id \
                 WHERE (u.deleted_at IS NULL OR (u.delete_after > 0 AND u.erase_started_at = 0)) \
                 AND om.org_id = ",
            ));
            sql.bind(scope.org()?)
                .push(" ORDER BY om.created_at, om.user_id");
            sql.push_page(paging);
            scope
                .fetch_all(sql)
                .await?
                .iter()
                .map(|row| {
                    Ok(OrgMember {
                        user_id: row.id(0)?,
                        username: row.text(1)?,
                        display_name: row.text(2)?,
                        name: person_names::name_from_row(row, 4)?,
                        role: role(row, 3)?,
                    })
                })
                .collect()
        })
        .await
    }

    /// Gives a member of the handle's organization a role in it, on behalf of
    /// `actor_id`, and records it. An administrator who is made a member is taken out of
    /// every workspace they had joined as the organization's administrator, each of
    /// which records it (`end_joining`). The last administrator stays one.
    ///
    /// Changes of roles, and joinings, are made one at a time in an organization: of
    /// two administrators who make each other members at once, one is refused.
    ///
    /// In the instance's own organization (the oldest, the only one of a single
    /// install) its administrators are the instance's: who is made one administers the
    /// instance too, and who stops stops administering it, unless they are its last
    /// administrator, which refuses the change. In another, a role is its own alone.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if there is no such organization in use.
    pub async fn set_org_role(
        &self,
        user_id: &str,
        role: OrgRole,
        actor_id: &str,
    ) -> Result<OrgRoleChange, StoreError> {
        self.transaction(async |scope| {
            // The instance's row first, as a change of who administers the instance
            // takes it: the two are one role for now (`members::follow_instance_role`).
            let held_instance = users::lock_instance(scope).await?;
            let the_instances = directory::is_the_instances_organization(scope).await?;
            let (org, _) = held_by_admin(scope, actor_id).await?;
            let Some(current) = org_role_of(scope, user_id).await? else {
                return Ok(OrgRoleChange::NotAMember);
            };
            if current == role {
                return Ok(OrgRoleChange::Done);
            }
            if current == OrgRole::Admin && !another_admin(scope, user_id).await? {
                return Ok(OrgRoleChange::LastAdmin);
            }
            // The administrators of the instance's own organization are the
            // instance's for now (`members::follow_instance_role`): who becomes one
            // administers the instance, and who stops does not, or they would make
            // themselves, or anybody, its administrator again past this log. Of
            // another organization, a role is that organization's alone.
            if the_instances && !users::follow_org_role(scope, held_instance, user_id, role).await?
            {
                return Ok(OrgRoleChange::LastAdmin);
            }
            let actor = AuditActor::Person(actor_id.to_owned());
            change_org_role(scope, &org.id, user_id, role, &actor).await?;
            Ok(OrgRoleChange::Done)
        })
        .await
    }

    /// Every workspace of the handle's organization, in use or in the trash, the oldest
    /// first, with what `viewer_id` is in each.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn org_workspaces(&self, viewer_id: &str) -> Result<Vec<OrgWorkspace>, StoreError> {
        self.with_conn(async |scope| {
            let mut sql = scope.sql(
                "SELECT w.id, w.name, w.deleted_at, \
                 CAST(CASE WHEN o.default_workspace_id = w.id THEN 1 ELSE 0 END AS BIGINT), \
                 (SELECT CAST(COUNT(*) AS BIGINT) FROM workspace_members AS m \
                 WHERE m.workspace_id = w.id), me.role, me.via_org_admin \
                 FROM workspaces AS w JOIN organizations AS o ON o.id = w.org_id \
                 LEFT JOIN workspace_members AS me ON me.workspace_id = w.id AND me.user_id = ",
            );
            sql.bind(Id(viewer_id))
                .push_bind(" WHERE w.org_id = ", scope.org()?)
                .push(" ORDER BY w.created_at, w.id");
            scope
                .fetch_all(sql)
                .await?
                .iter()
                .map(|row| {
                    let my_role = row.text(5)?;
                    Ok(OrgWorkspace {
                        id: row.id(0)?,
                        name: row.text(1)?,
                        deleted_at: row.opt_int(2)?,
                        is_default: row.int(3)? == 1,
                        member_count: row.int(4)?,
                        role: if my_role.is_empty() {
                            None
                        } else {
                            Some(role(row, 5)?)
                        },
                        joined_as_org_admin: row.bool(6)?,
                    })
                })
                .collect()
        })
        .await
    }

    /// Stores a new workspace of the handle's organization, made by `actor_id`, and
    /// records it. Nobody is a member of it.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Conflict`] with [`Taken::WorkspaceName`] if a workspace in
    /// use of the organization has its name.
    pub async fn create_named_workspace(
        &self,
        workspace: &Workspace,
        actor_id: &str,
    ) -> Result<(), StoreError> {
        self.transaction(async |scope| {
            held_by_admin(scope, actor_id).await?;
            let org = scope.org()?;
            workspaces::insert_workspace(scope, workspace, org)
                .await
                .map_err(name_taken)?;
            let entry = AuditEntry::new("", actor_id, AuditAction::WorkspaceCreated, &workspace.id);
            audit::record_entry(scope, &entry).await
        })
        .await
    }

    /// Renames a workspace of the handle's organization that is in use, on behalf of
    /// `actor_id`, and records it.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if the organization has no such workspace in use,
    /// and [`StoreError::Conflict`] with [`Taken::WorkspaceName`] if another workspace in
    /// use has the name.
    pub async fn rename_workspace(
        &self,
        workspace_id: &str,
        name: &str,
        actor_id: &str,
    ) -> Result<(), StoreError> {
        self.transaction(async |scope| {
            held_by_admin(scope, actor_id).await?;
            let mut sql = scope.sql("UPDATE workspaces SET name = ");
            sql.changing_no_access()
                .bind(name)
                .push_bind(", modified_by = ", OptId(actor_id))
                .push_bind(", updated_at = ", scope.now_millis())
                .push_bind(" WHERE id = ", Id(workspace_id))
                .push_bind(" AND org_id = ", scope.org()?)
                .push(" AND deleted_at IS NULL");
            if scope.execute(sql).await.map_err(name_taken)? == 0 {
                return Err(StoreError::not_found("workspace"));
            }
            let entry = AuditEntry::new("", actor_id, AuditAction::WorkspaceRenamed, workspace_id);
            audit::record_entry(scope, &entry).await
        })
        .await
    }

    /// Puts a workspace of the handle's organization in the trash, on behalf of
    /// `actor_id`, and records it. Nothing in it changes: it is not there for anybody
    /// until it is brought back, and goes for good when it has been in the trash as
    /// long as a board is kept there ([`Store::purge_workspace`]).
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if a statement fails.
    pub async fn trash_workspace(
        &self,
        workspace_id: &str,
        actor_id: &str,
    ) -> Result<WorkspaceTrashing, StoreError> {
        self.transaction(async |scope| {
            let (org, _) = held_by_admin(scope, actor_id).await?;
            if org.default_workspace_id == workspace_id {
                return Ok(WorkspaceTrashing::Default);
            }
            let mut sql = scope.sql("UPDATE workspaces SET deleted_at = ");
            sql.bind(scope.now_millis())
                .push_bind(", modified_by = ", OptId(actor_id))
                .push_bind(" WHERE id = ", Id(workspace_id))
                .push_bind(" AND org_id = ", scope.org()?)
                .push(" AND deleted_at IS NULL")
                .push_bind(" AND is_system = ", false);
            if scope.execute(sql).await? == 0 {
                return Ok(WorkspaceTrashing::NotFound);
            }
            crate::api_tokens::forget_workspace(scope, workspace_id).await?;
            let entry = AuditEntry::new("", actor_id, AuditAction::WorkspaceDeleted, workspace_id);
            audit::record_entry(scope, &entry).await?;
            Ok(WorkspaceTrashing::Done)
        })
        .await
    }

    /// Brings a workspace of the handle's organization back from the trash, as it was,
    /// on behalf of `actor_id`, and records it.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if no workspace of the organization by the
    /// identifier is in the trash, and [`StoreError::Conflict`] with
    /// [`Taken::WorkspaceName`] if a workspace in use has taken its name meanwhile.
    pub async fn restore_workspace(
        &self,
        workspace_id: &str,
        actor_id: &str,
    ) -> Result<(), StoreError> {
        self.transaction(async |scope| {
            held_by_admin(scope, actor_id).await?;
            // Of a restoration and the removal for good, the one that comes second finds
            // the workspace the first left: the removal holds the row.
            let mut sql = scope.sql("UPDATE workspaces SET deleted_at = NULL");
            sql.push_bind(", modified_by = ", OptId(actor_id))
                .push_bind(", updated_at = ", scope.now_millis())
                .push_bind(" WHERE id = ", Id(workspace_id))
                .push_bind(" AND org_id = ", scope.org()?)
                .push(" AND deleted_at IS NOT NULL");
            if scope.execute(sql).await.map_err(name_taken)? == 0 {
                return Err(StoreError::not_found("workspace in the trash"));
            }
            let entry = AuditEntry::new("", actor_id, AuditAction::WorkspaceRestored, workspace_id);
            audit::record_entry(scope, &entry).await
        })
        .await
    }

    /// Removes for good a workspace of the handle's organization that was put in the
    /// trash before `deleted_before`, and everything in it: each of its boards goes into
    /// the trash as a board that is deleted does, and from there for good with every
    /// other board of it in the trash, as the boards of the trash go
    /// ([`Store::purge_trash`]): their files are doomed. Then its categories, its
    /// members and its audit log go with it. Nothing happens to a workspace that is not
    /// there, in use, or in the trash since `deleted_before` or later.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if a statement fails; nothing is removed then.
    pub async fn purge_workspace(
        &self,
        workspace_id: &str,
        deleted_before: i64,
    ) -> Result<Purged, StoreError> {
        self.transaction(async |scope| {
            // The organization's first, as deleting a board takes it before the board's:
            // with a joining of the workspace that ends meanwhile (`joinings`). Its row
            // alone: an organization no longer in use still has its trash emptied.
            let held_org = hold_organization_row(scope).await?;
            // Held against a restoration under way, which then finds nothing to restore.
            let mut sql = scope.sql("SELECT id FROM workspaces WHERE id = ");
            sql.bind(Id(workspace_id))
                .push_bind(" AND org_id = ", scope.org()?)
                .push(" AND deleted_at IS NOT NULL")
                .push_bind(" AND deleted_at < ", deleted_before);
            sql.push_lock(scope.row_lock(LockRank::Workspace, workspace_id, RowLock::Update));
            if !scope.exists(sql).await? {
                return Ok(Purged::default());
            }
            remove_workspace(scope, held_org, workspace_id).await
        })
        .await
    }

    /// Makes an administrator of the handle's organization an administrator of a
    /// workspace of it in use, as the organization's: one who was a member of it before
    /// keeps what they were, to be it again when they leave (`end_joining`). Records it.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if a statement fails.
    pub async fn join_workspace_as_org_admin(
        &self,
        workspace_id: &str,
        user_id: &str,
    ) -> Result<Joining, StoreError> {
        self.transaction(async |scope| {
            // One at a time with the changes of roles: who is made a member meanwhile
            // does not join after their joinings were taken back.
            lock_organization(scope).await?;
            if org_role_of(scope, user_id).await? != Some(OrgRole::Admin) {
                return Ok(Joining::NoOrgAdmin);
            }
            let mut sql = scope.sql("SELECT id FROM workspaces WHERE id = ");
            sql.bind(Id(workspace_id))
                .push_bind(" AND org_id = ", scope.org()?)
                .push(" AND deleted_at IS NULL")
                .push_bind(" AND is_system = ", false);
            if !scope.exists(sql).await? {
                return Ok(Joining::NotFound);
            }
            let membership = membership(scope, workspace_id, user_id).await?;
            if matches!(membership, Some((WorkspaceRole::Admin, _))) {
                return Ok(Joining::Already);
            }
            let held = joinings::held_roles(scope, workspace_id, user_id).await?;
            if let Some((prior, _)) = membership {
                let mut sql = scope.sql("UPDATE workspace_members SET role = ");
                sql.bind(WorkspaceRole::Admin.as_str())
                    .push_bind(", via_org_admin = ", true)
                    .push_bind(", prior_role = ", prior.as_str())
                    .push_bind(", prior_container_roles = ", held.as_str())
                    .push_bind(" WHERE workspace_id = ", Id(workspace_id))
                    .push_bind(" AND user_id = ", Id(user_id));
                scope.execute(sql).await?;
            } else {
                let mut sql = scope.sql(
                    "INSERT INTO workspace_members (workspace_id, user_id, role, \
                     via_org_admin, prior_container_roles, created_at) VALUES (",
                );
                sql.bind(Id(workspace_id))
                    .push_bind(", ", Id(user_id))
                    .push_bind(", ", WorkspaceRole::Admin.as_str())
                    .push_bind(", ", true)
                    .push_bind(", ", held.as_str())
                    .push_bind(", ", scope.now_millis())
                    .push(")");
                scope.execute(sql).await?;
            }
            let entry = AuditEntry::new(
                workspace_id,
                user_id,
                AuditAction::OrgAdminJoined,
                workspace_id,
            );
            audit::record_entry(scope, &entry).await?;
            Ok(Joining::Joined)
        })
        .await
    }

    /// Takes a user who joined a workspace of the handle's organization as the
    /// organization's administrator out of it, at their own asking (`end_joining`),
    /// and records it. Says whether they had joined it so.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if a statement fails.
    pub async fn leave_workspace_as_org_admin(
        &self,
        workspace_id: &str,
        user_id: &str,
    ) -> Result<bool, StoreError> {
        self.transaction(async |scope| {
            lock_organization(scope).await?;
            let Some((_, Some(joined))) = membership(scope, workspace_id, user_id).await? else {
                return Ok(false);
            };
            end_joining(scope, workspace_id, user_id, joined).await?;
            let entry = AuditEntry::new(
                workspace_id,
                user_id,
                AuditAction::OrgAdminLeft,
                workspace_id,
            );
            audit::record_entry(scope, &entry).await?;
            Ok(true)
        })
        .await
    }
}

/// The handle's organization as it is stored.
async fn organization(scope: &mut Scope<'_>) -> Result<Organization, StoreError> {
    let mut sql = scope.sql(
        "SELECT id, name, public_links, allow_copy_out, default_workspace_id, \
         allow_api_tokens FROM organizations WHERE id = ",
    );
    sql.bind(scope.org()?)
        .push(" AND deleted_at IS NULL AND is_system = false");
    let row = scope
        .fetch_optional(sql)
        .await?
        .ok_or_else(|| StoreError::not_found("organization"))?;
    Ok(Organization {
        id: row.id(0)?,
        name: row.text(1)?,
        public_links: row.bool(2)?,
        allow_copy_out: row.bool(3)?,
        allow_api_tokens: row.bool(5)?,
        default_workspace_id: row.id(4)?,
    })
}

/// Gives a member of the handle's organization (`org_id`) a role in it on behalf of
/// `actor_id`, and records it; who stops administering it is taken out of what they
/// joined as its administrator, each joining recorded (`revoke_joinings`). The one way
/// a role in the organization changes, through its route or the instance's
/// (`members::follow_instance_role`): no role in a workspace changes with it, as an
/// administrator of the organization enters a workspace only by a joining on the record.
pub(crate) async fn change_org_role(
    scope: &mut Scope<'_>,
    org_id: &str,
    user_id: &str,
    role: OrgRole,
    actor: &AuditActor,
) -> Result<(), StoreError> {
    let mut sql = scope.sql("UPDATE organization_members SET role = ");
    sql.bind(role.as_str())
        .push_bind(" WHERE org_id = ", scope.org()?)
        .push_bind(" AND user_id = ", Id(user_id));
    scope.execute(sql).await?;
    let action = match role {
        OrgRole::Admin => AuditAction::OrgAdminGranted,
        OrgRole::Member => AuditAction::OrgAdminRemoved,
    };
    let entry = AuditEntry::by(actor, "", action, org_id).with_subject(user_id);
    audit::record_entry(scope, &entry).await?;
    if role != OrgRole::Admin {
        revoke_joinings(scope, user_id, actor).await?;
    }
    Ok(())
}

/// Removes a workspace of the scope's organization for good, and everything in it, as
/// [`Store::purge_workspace`] says: each of its boards goes into the trash and from
/// there for good with those that were in it, their files doomed, and its categories,
/// its members and its audit log go with it.
pub(crate) async fn remove_workspace<'c>(
    scope: &mut Scope<'c>,
    _held: OrganizationHeld<'c>,
    workspace_id: &str,
) -> Result<Purged, StoreError> {
    let mut sql = scope.sql("SELECT id FROM containers WHERE workspace_id = ");
    sql.bind(Id(workspace_id)).push(" ORDER BY id");
    let containers: Vec<String> = scope.all(sql).await?;
    // Every board, then every block of them, before the first goes: each then holds
    // what it holds.
    for container_id in &containers {
        crate::containers::lock_container_for_deletion(scope, container_id).await?;
    }
    let ids: Vec<&str> = containers.iter().map(String::as_str).collect();
    trash::hold_blocks_of_containers(scope, &ids).await?;
    for container_id in &containers {
        trash::delete_container(scope, container_id, "").await?;
    }
    let mut purged = trash::remove_containers(scope, Trashed::OfWorkspace(workspace_id)).await?;
    purged.records += trash::remove_what_they_left(scope).await?;
    for table in ["categories", "workspace_members"] {
        let mut sql = scope.sql(["DELETE FROM ", table, " WHERE workspace_id = "]);
        sql.bind(Id(workspace_id));
        scope.execute(sql).await?;
    }
    // Its audit log goes with it: the schema lets a record go once its workspace
    // is not there.
    let mut sql = scope.sql("DELETE FROM workspaces WHERE id = ");
    sql.bind(Id(workspace_id));
    scope.execute(sql).await?;
    Ok(purged)
}

/// Holds the handle's organization's row ([`hold_organization_row`]) and reads it:
/// [`StoreError::NotFound`] if it is not in use. A job that must also work on an
/// organization no longer in use, as emptying its trash does, holds the row alone.
pub(crate) async fn lock_organization<'c>(
    scope: &mut Scope<'c>,
) -> Result<(Organization, OrganizationHeld<'c>), StoreError> {
    let held = hold_organization_row(scope).await?;
    Ok((organization(scope).await?, held))
}

/// Holds the handle's organization's row to the end of the transaction, whether it is
/// in use or not: the changes of who administers it, of its workspaces and of the
/// boards they hold, are made one at a time. On SQLite a transaction that writes holds
/// the whole database.
pub(crate) async fn hold_organization_row<'c>(
    scope: &mut Scope<'c>,
) -> Result<OrganizationHeld<'c>, StoreError> {
    let org_key = scope.org_id().unwrap_or_default();
    let mut sql = scope.sql("SELECT id FROM organizations WHERE id = ");
    sql.bind(scope.org()?);
    // Not FOR UPDATE: what refers to the organization (a member, a workspace, a record)
    // is still added meanwhile; only another holder of this lock waits.
    sql.push_lock(scope.row_lock(LockRank::Organization, &org_key, RowLock::NoKeyUpdate));
    scope.fetch_optional(sql).await?;
    Ok(OrganizationHeld::new())
}

/// Holds the handle's organization's row as [`lock_organization`] does, for a change
/// `actor_id` makes as its administrator: one who stopped being one meanwhile changes
/// nothing, as if the organization were not there.
async fn held_by_admin<'c>(
    scope: &mut Scope<'c>,
    actor_id: &str,
) -> Result<(Organization, OrganizationHeld<'c>), StoreError> {
    let (org, held) = lock_organization(scope).await?;
    if org_role_of(scope, actor_id).await? == Some(OrgRole::Admin) {
        Ok((org, held))
    } else {
        Err(StoreError::not_found("organization"))
    }
}

/// Holds the handle's organization's row and tells whether `user_id` administers the
/// workspace: a manager of a board who stopped being one meanwhile gives no role on it.
pub(crate) async fn still_workspace_admin(
    scope: &mut Scope<'_>,
    workspace_id: &str,
    user_id: &str,
) -> Result<bool, StoreError> {
    let _ = lock_organization(scope).await?;
    Ok(matches!(
        membership(scope, workspace_id, user_id).await?,
        Some((WorkspaceRole::Admin, _))
    ))
}

/// What the user is in the handle's organization; `None` for no member.
pub(crate) async fn org_role_of(
    scope: &mut Scope<'_>,
    user_id: &str,
) -> Result<Option<OrgRole>, StoreError> {
    let mut sql = scope.sql("SELECT role FROM organization_members WHERE org_id = ");
    sql.bind(scope.org()?)
        .push_bind(" AND user_id = ", Id(user_id));
    match scope.fetch_optional(sql).await? {
        Some(row) => role(&row, 0).map(Some),
        None => Ok(None),
    }
}

/// Whether the user is the last administrator of the handle's organization whose
/// account is in use. The organization's row is held to the end of the transaction.
pub(crate) async fn is_last_org_admin(
    scope: &mut Scope<'_>,
    user_id: &str,
) -> Result<bool, StoreError> {
    lock_organization(scope).await?;
    Ok(org_role_of(scope, user_id).await? == Some(OrgRole::Admin)
        && !another_admin(scope, user_id).await?)
}

/// Whether the handle's organization has an administrator besides the user whose
/// account is in use.
async fn another_admin(scope: &mut Scope<'_>, user_id: &str) -> Result<bool, StoreError> {
    let mut sql = scope.sql(
        "SELECT 1 FROM organization_members AS om JOIN users AS u ON u.id = om.user_id \
         WHERE u.deleted_at IS NULL AND om.org_id = ",
    );
    sql.bind(scope.org()?)
        .push_bind(" AND om.role = ", OrgRole::Admin.as_str())
        .push_bind(" AND om.user_id <> ", Id(user_id))
        .push(" LIMIT 1");
    scope.exists(sql).await
}

/// How a user who joined a workspace as the organization's administrator was in it
/// before: as what, if a member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Joined {
    prior: Option<WorkspaceRole>,
    /// Their roles on the workspace's boards then (`joinings::held_roles`).
    held: String,
}

/// What the user is in the workspace and, if they joined it as the organization's
/// administrator, how they were in it before.
pub(crate) async fn membership(
    scope: &mut Scope<'_>,
    workspace_id: &str,
    user_id: &str,
) -> Result<Option<(WorkspaceRole, Option<Joined>)>, StoreError> {
    let mut sql = scope.sql(
        "SELECT role, via_org_admin, prior_role, prior_container_roles FROM workspace_members \
         WHERE workspace_id = ",
    );
    sql.bind(Id(workspace_id))
        .push_bind(" AND user_id = ", Id(user_id));
    let Some(row) = scope.fetch_optional(sql).await? else {
        return Ok(None);
    };
    let joined = if row.bool(1)? {
        let prior = row.text(2)?;
        Some(Joined {
            prior: if prior.is_empty() {
                None
            } else {
                Some(role(&row, 2)?)
            },
            held: row.text(3)?,
        })
    } else {
        None
    };
    Ok(Some((role(&row, 0)?, joined)))
}

/// Takes back from the user every joining of a workspace of the handle's organization
/// as the organization's administrator, in use or in the trash, on behalf of
/// `actor`, recording each in its workspace. Returns the workspaces of the joinings, by
/// identifier.
pub(crate) async fn revoke_joinings(
    scope: &mut Scope<'_>,
    user_id: &str,
    actor: &AuditActor,
) -> Result<Vec<String>, StoreError> {
    end_joinings(scope, user_id, |workspace_id| {
        AuditEntry::by(
            actor,
            workspace_id,
            AuditAction::OrgAdminRevoked,
            workspace_id,
        )
        .with_subject(user_id)
    })
    .await
}

/// Ends every joining of a workspace of the handle's organization the user made as its
/// administrator, at their own asking, as they leave the organization: each recorded in
/// its workspace as leaving it (`leave_workspace_as_org_admin`) is. Returns the
/// workspaces, by identifier: one they were no member of before is not theirs any more.
pub(crate) async fn leave_joinings(
    scope: &mut Scope<'_>,
    user_id: &str,
) -> Result<Vec<String>, StoreError> {
    end_joinings(scope, user_id, |workspace_id| {
        AuditEntry::new(
            workspace_id,
            user_id,
            AuditAction::OrgAdminLeft,
            workspace_id,
        )
    })
    .await
}

/// Ends every joining of the user's of a workspace of the handle's organization, in
/// use or in the trash, and records each in its workspace as `record` has it. Returns
/// the workspaces of the joinings, by identifier.
async fn end_joinings(
    scope: &mut Scope<'_>,
    user_id: &str,
    record: impl Fn(&str) -> AuditEntry,
) -> Result<Vec<String>, StoreError> {
    let mut sql = scope.sql(
        "SELECT wm.workspace_id, wm.prior_role, wm.prior_container_roles \
         FROM workspace_members AS wm \
         JOIN workspaces AS w ON w.id = wm.workspace_id WHERE wm.user_id = ",
    );
    sql.bind(Id(user_id))
        .push_bind(" AND wm.via_org_admin = ", true)
        .push_bind(" AND w.org_id = ", scope.org()?)
        .push(" ORDER BY wm.workspace_id");
    let rows = scope.fetch_all(sql).await?;
    let mut ended = Vec::with_capacity(rows.len());
    for row in &rows {
        let workspace_id = row.id(0)?;
        let prior = row.text(1)?;
        let joined = Joined {
            prior: if prior.is_empty() {
                None
            } else {
                Some(role(row, 1)?)
            },
            held: row.text(2)?,
        };
        end_joining(scope, &workspace_id, user_id, joined).await?;
        audit::record_entry(scope, &record(&workspace_id)).await?;
        ended.push(workspace_id);
    }
    Ok(ended)
}

/// Ends a joining of a workspace as the organization's administrator: the user is
/// what they were in it before, or no member if they were none, and has the roles on
/// its boards they had when they joined, and no other (`joinings::give_back_roles`).
async fn end_joining(
    scope: &mut Scope<'_>,
    workspace_id: &str,
    user_id: &str,
    joined: Joined,
) -> Result<(), StoreError> {
    joinings::give_back_roles(scope, workspace_id, user_id, &joined.held).await?;
    if joined.prior.is_none() || joined.prior == Some(WorkspaceRole::Guest) {
        crate::container_team_membership::remove_workspace_teams(scope, workspace_id, user_id)
            .await?;
        crate::api_tokens::forget_workspace_of_user(scope, workspace_id, user_id).await?;
    }
    let mut sql = match joined.prior {
        Some(prior) => {
            let mut sql = scope.sql("UPDATE workspace_members SET role = ");
            sql.bind(prior.as_str())
                .push_bind(", via_org_admin = ", false)
                .push(", prior_role = NULL, prior_container_roles = NULL");
            sql
        }
        None => scope.sql("DELETE FROM workspace_members"),
    };
    sql.push_bind(" WHERE workspace_id = ", Id(workspace_id))
        .push_bind(" AND user_id = ", Id(user_id));
    scope.execute(sql).await?;
    if joined.prior.is_none() {
        crate::favorites::forget_user_in_workspace(scope, workspace_id, user_id).await?;
        crate::unit_progress::forget_user_in_workspace(scope, workspace_id, user_id).await?;
        crate::course_exams::forget_user_in_workspace(scope, workspace_id, user_id).await?;
        crate::unit_reviews::forget_user_in_workspace(scope, workspace_id, user_id).await?;
    } else {
        crate::unit_progress::forget_user_in_workspace_unjoined(scope, workspace_id, user_id)
            .await?;
        crate::course_exams::forget_user_in_workspace_unjoined(scope, workspace_id, user_id)
            .await?;
        crate::unit_reviews::forget_user_in_workspace_unjoined(scope, workspace_id, user_id)
            .await?;
    }
    Ok(())
}

/// Reads the violation of the unique index of the names of the workspaces in use.
pub(crate) fn name_taken(error: StoreError) -> StoreError {
    match error {
        StoreError::Database(error)
            if error
                .as_database_error()
                .is_some_and(sqlx::error::DatabaseError::is_unique_violation) =>
        {
            StoreError::Conflict(Taken::WorkspaceName)
        }
        other => other,
    }
}

/// Reads a role from a column.
fn role<R: std::str::FromStr<Err = fvoci_model::UnknownRole>>(
    row: &Row,
    index: usize,
) -> Result<R, StoreError> {
    row.text(index)?
        .parse()
        .map_err(|error: fvoci_model::UnknownRole| StoreError::Corrupt {
            column: "role",
            reason: error.to_string(),
        })
}
