//! Who is in a workspace: its members as its administrators list them, a change of what
//! one of them is there, and the removal of one.
//!
//! A change and a removal are one transaction each, under a lock of the workspace, so
//! that of two who would take the last administrator away at once one is refused. Who
//! leaves a workspace, and who becomes a guest of it, stops being an admin of its
//! boards; a board that would be left without an admin by that passes to who made the
//! change. The locks are taken in one order, as every write of a board's members takes
//! them: the organization (as every change of who administers what has it,
//! `org_admin::hold_organization_row`), the workspace, the boards, those of the trash,
//! the membership of the workspace. Every statement names the organization itself: the
//! handle's.
//!
//! The boards are held before the membership, and so they are listed before it is
//! held: a board the user made, or was put on, while the locks were waited for is not
//! among them. With everything held the list is read again, and if it has grown the
//! transaction is given up and made anew, in the same order, with the board in it.

use std::collections::BTreeSet;

use fvoci_model::{ContainerMember, ContainerRole, WorkspaceMember, WorkspaceRole};

use crate::Store;
use crate::audit::{self, AuditEntry};
use crate::containers::{delete_member, find_member, lock_container, save_member};
use crate::db::{Dialect, Id, LockRank, Row, Scope};
use crate::error::StoreError;
use crate::trash::{self, TrashedRoles};
use crate::{members, org_admin, person_names};

/// How often a change is made anew because the user came onto another board while its
/// locks were waited for.
const ATTEMPTS: usize = 4;

/// What became of a change to who is in a workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MembershipChange {
    /// It is done. The boards that would have been left without an admin by it, of
    /// which who did it is the admin now.
    Done {
        /// The identifiers of those boards.
        taken_over: Vec<String>,
    },
    /// The user is no member of the workspace, or there is no such workspace in use.
    NotAMember,
    /// The user is something else in the workspace than the change was asked from:
    /// this, by now. Nothing was changed.
    RoleIs(WorkspaceRole),
    /// It would leave the workspace without an administrator.
    LastAdmin,
    /// The user came onto another board of the workspace every time the change was
    /// tried: nothing was changed.
    Unsettled,
}

impl Store {
    /// The members of a workspace in use of the handle's organization, its guests too,
    /// with their names: the oldest membership first, and then by identifier.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] if the handle names none, and
    /// [`StoreError::Database`] if the query fails.
    pub async fn get_workspace_members(
        &self,
        workspace_id: &str,
    ) -> Result<Vec<WorkspaceMember>, StoreError> {
        self.with_conn(async |scope| {
            let mut sql = scope.sql(SELECT_MEMBERS);
            sql.bind(Id(workspace_id))
                .push_bind(" AND t.org_id = ", scope.org()?)
                .push(" AND t.deleted_at IS NULL")
                .push(" ORDER BY wm.created_at, wm.user_id");
            scope
                .fetch_all(sql)
                .await?
                .iter()
                .map(member_from_row)
                .collect()
        })
        .await
    }

    /// One member of a workspace in use of the handle's organization, with their names;
    /// `None` for somebody who is none.
    ///
    /// # Errors
    ///
    /// As [`Store::get_workspace_members`].
    pub async fn get_workspace_member(
        &self,
        workspace_id: &str,
        user_id: &str,
    ) -> Result<Option<WorkspaceMember>, StoreError> {
        self.with_conn(async |scope| {
            let mut sql = scope.sql(SELECT_MEMBERS);
            sql.bind(Id(workspace_id))
                .push_bind(" AND t.org_id = ", scope.org()?)
                .push(" AND t.deleted_at IS NULL")
                .push_bind(" AND wm.user_id = ", Id(user_id));
            scope
                .fetch_optional(sql)
                .await?
                .as_ref()
                .map(member_from_row)
                .transpose()
        })
        .await
    }

    /// Makes a member of a workspace who is `from` there what `to` says, on behalf of
    /// `by`, unless that would leave the workspace without an administrator.
    ///
    /// Who becomes a guest is an editor, and no more, of the boards of the workspace
    /// they were an admin of, those in the trash too; a board they were the only admin
    /// of passes to `by`, who is its admin from then on, and that is recorded for each
    /// board `by` had no role on before.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] if the handle names none, and
    /// [`StoreError::Database`] if a statement fails. Nothing is stored then.
    pub async fn change_workspace_role(
        &self,
        workspace_id: &str,
        user_id: &str,
        (from, to): (WorkspaceRole, WorkspaceRole),
        by: &str,
    ) -> Result<MembershipChange, StoreError> {
        for _ in 0..ATTEMPTS {
            let attempt = self
                .transaction(async |scope| {
                    change_role(scope, workspace_id, user_id, (from, to), by).await
                })
                .await?;
            if let Some(change) = attempt {
                return Ok(change);
            }
        }
        Ok(MembershipChange::Unsettled)
    }

    /// Removes a member from a workspace, on behalf of `by`, unless that would leave
    /// the workspace without an administrator.
    ///
    /// The roles the user has on the boards of the workspace go with the membership,
    /// with what they subscribed to there, and those its boards in the trash would
    /// come back with too: who comes back later starts anew. A board they were the
    /// only admin of passes to `by`, as [`Store::change_workspace_role`] says. What
    /// they wrote stays, under their name. They stay a member of the organization.
    ///
    /// # Errors
    ///
    /// As [`Store::change_workspace_role`].
    pub async fn remove_workspace_member(
        &self,
        workspace_id: &str,
        user_id: &str,
        by: &str,
    ) -> Result<MembershipChange, StoreError> {
        for _ in 0..ATTEMPTS {
            let attempt = self
                .transaction(async |scope| remove(scope, workspace_id, user_id, by).await)
                .await?;
            if let Some(change) = attempt {
                return Ok(change);
            }
        }
        Ok(MembershipChange::Unsettled)
    }
}

/// One try at [`Store::change_workspace_role`], in its transaction: `None` if the user
/// came onto another board while the locks were waited for, and nothing was written.
async fn change_role(
    scope: &mut Scope<'_>,
    workspace_id: &str,
    user_id: &str,
    (from, to): (WorkspaceRole, WorkspaceRole),
    by: &str,
) -> Result<Option<MembershipChange>, StoreError> {
    org_admin::hold_organization_row(scope).await?;
    if !hold_workspace(scope, workspace_id).await? {
        return Ok(Some(MembershipChange::NotAMember));
    }
    // The boards before the membership, as every write of a board's members has them;
    // those in the trash too, as a restore has them.
    let containers = if to == WorkspaceRole::Guest {
        let containers = hold_containers_of(scope, workspace_id, user_id).await?;
        let _ = trash::hold_deleted_containers(scope, workspace_id).await?;
        containers
    } else {
        Vec::new()
    };
    match held_role(scope, workspace_id, user_id).await? {
        None => return Ok(Some(MembershipChange::NotAMember)),
        Some(current) if current != from => {
            return Ok(Some(MembershipChange::RoleIs(current)));
        }
        Some(_) => {}
    }
    if to == WorkspaceRole::Guest
        && !holds_every_container_of(scope, workspace_id, user_id, &containers).await?
    {
        return Ok(None);
    }
    if from == WorkspaceRole::Admin
        && to != WorkspaceRole::Admin
        && is_only_administrator(scope, workspace_id, user_id).await?
    {
        return Ok(Some(MembershipChange::LastAdmin));
    }
    let mut taken_over = Vec::new();
    if to == WorkspaceRole::Guest {
        crate::container_team_membership::remove_workspace_teams(scope, workspace_id, user_id)
            .await?;
        crate::api_tokens::forget_workspace_of_user(scope, workspace_id, user_id).await?;
        taken_over = hand_over(scope, workspace_id, &containers, user_id, by).await?;
        let mut sql = scope.sql("UPDATE container_members SET role = ");
        sql.bind(ContainerRole::Editor.as_str())
            .push_bind(" WHERE user_id = ", Id(user_id))
            .push_bind(" AND role = ", ContainerRole::Admin.as_str())
            .push_bind(
                " AND container_id IN (SELECT id FROM containers WHERE workspace_id = ",
                Id(workspace_id),
            )
            .push(")");
        scope.execute(sql).await?;
        trash::change_member_of_deleted_containers(
            scope,
            workspace_id,
            user_id,
            TrashedRoles::NoMoreThanEditor,
        )
        .await?;
    }
    let mut sql = scope.sql("UPDATE workspace_members SET role = ");
    sql.bind(to.as_str())
        .push_bind(" WHERE workspace_id = ", Id(workspace_id))
        .push_bind(" AND user_id = ", Id(user_id));
    scope.execute(sql).await?;
    if to == WorkspaceRole::Guest {
        crate::unit_progress::forget_user_in_workspace_unjoined(scope, workspace_id, user_id)
            .await?;
        crate::course_exams::forget_user_in_workspace_unjoined(scope, workspace_id, user_id)
            .await?;
        crate::unit_reviews::forget_user_in_workspace_unjoined(scope, workspace_id, user_id)
            .await?;
    }
    Ok(Some(MembershipChange::Done { taken_over }))
}

/// One try at [`Store::remove_workspace_member`], in its transaction: `None` if the
/// user came onto another board while the locks were waited for, and nothing was
/// written.
async fn remove(
    scope: &mut Scope<'_>,
    workspace_id: &str,
    user_id: &str,
    by: &str,
) -> Result<Option<MembershipChange>, StoreError> {
    org_admin::hold_organization_row(scope).await?;
    if !hold_workspace(scope, workspace_id).await? {
        return Ok(Some(MembershipChange::NotAMember));
    }
    let containers = hold_containers_of(scope, workspace_id, user_id).await?;
    let _ = trash::hold_deleted_containers(scope, workspace_id).await?;
    let Some(current) = held_role(scope, workspace_id, user_id).await? else {
        return Ok(Some(MembershipChange::NotAMember));
    };
    if !holds_every_container_of(scope, workspace_id, user_id, &containers).await? {
        return Ok(None);
    }
    if current == WorkspaceRole::Admin
        && is_only_administrator(scope, workspace_id, user_id).await?
    {
        return Ok(Some(MembershipChange::LastAdmin));
    }
    let taken_over = hand_over(scope, workspace_id, &containers, user_id, by).await?;
    for container_id in &containers {
        delete_member(scope, container_id, user_id).await?;
    }
    // A role stored on a board that was not held: one stored since the boards were
    // last listed.
    let mut sql = scope.sql("DELETE FROM container_members WHERE user_id = ");
    sql.bind(Id(user_id))
        .push_bind(
            " AND container_id IN (SELECT id FROM containers WHERE workspace_id = ",
            Id(workspace_id),
        )
        .push(")");
    scope.execute(sql).await?;
    trash::change_member_of_deleted_containers(scope, workspace_id, user_id, TrashedRoles::Forget)
        .await?;
    trash::forget_watcher_of_workspace(scope, workspace_id, user_id).await?;
    crate::unit_progress::forget_user_in_workspace(scope, workspace_id, user_id).await?;
    crate::course_exams::forget_user_in_workspace(scope, workspace_id, user_id).await?;
    crate::unit_reviews::forget_user_in_workspace(scope, workspace_id, user_id).await?;
    crate::container_team_membership::remove_workspace_teams(scope, workspace_id, user_id).await?;
    crate::api_tokens::forget_workspace_of_user(scope, workspace_id, user_id).await?;
    let mut sql = scope.sql("DELETE FROM workspace_members WHERE workspace_id = ");
    sql.bind(Id(workspace_id))
        .push_bind(" AND user_id = ", Id(user_id));
    scope.execute(sql).await?;
    // A workspace guest is never in `organization_members`: once removed from their last
    // workspace in this organization, they can no longer leave the organization, so wipe
    // their time entries in it (including any active timer) to prevent orphans. Hold the
    // user's row first to serialize with concurrent `start_time_entry` / `create_time_entry`.
    crate::users::hold_user_for_cleanup(scope, user_id).await?;
    let org = scope.org()?;
    let mut sql = scope.sql("DELETE FROM time_entries WHERE user_id = ");
    sql.bind(Id(user_id))
        .push_bind(" AND org_id = ", org.clone())
        .push(" AND NOT EXISTS (SELECT 1 FROM organization_members WHERE user_id = ")
        .bind(Id(user_id))
        .push_bind(" AND org_id = ", org.clone())
        .push(
            ") AND NOT EXISTS (SELECT 1 FROM workspace_members AS wm \
               JOIN workspaces AS w ON w.id = wm.workspace_id WHERE wm.user_id = ",
        )
        .bind(Id(user_id))
        .push_bind(" AND w.org_id = ", org)
        .push(&scope.same_organization(&[("wm", "w")]))
        .push(")");
    scope.execute(sql).await?;
    Ok(Some(MembershipChange::Done { taken_over }))
}

/// The members of a workspace `t` of an organization; the workspace's identifier and
/// the conditions on `t` follow. The name's columns, unqualified, are the user's alone.
const SELECT_MEMBERS: &str = concat!(
    "SELECT wm.user_id, u.username, u.display_name, wm.role, wm.created_at, wm.via_org_admin, ",
    person_names::name_columns!(),
    " FROM workspace_members AS wm \
     JOIN workspaces AS t ON t.id = wm.workspace_id \
     JOIN users AS u ON u.id = wm.user_id WHERE wm.workspace_id = "
);

fn member_from_row(row: &Row) -> Result<WorkspaceMember, StoreError> {
    Ok(WorkspaceMember {
        user_id: row.id(0)?,
        username: row.text(1)?,
        display_name: row.text(2)?,
        name: person_names::name_from_row(row, 6)?,
        role: role_from(&row.text(3)?)?,
        created_at: row.int(4)?,
        joined_as_org_admin: row.bool(5)?,
    })
}

fn role_from(text: &str) -> Result<WorkspaceRole, StoreError> {
    text.parse()
        .map_err(|error: fvoci_model::UnknownRole| StoreError::Corrupt {
            column: "role",
            reason: error.to_string(),
        })
}

/// Whether a workspace in use of the scope's organization has the identifier: read and
/// not held, for a write that refuses one of another organization, or gone, as not
/// there before it writes anything.
pub(crate) async fn is_workspace_here(
    scope: &mut Scope<'_>,
    workspace_id: &str,
) -> Result<bool, StoreError> {
    let mut sql = scope.sql("SELECT id FROM workspaces WHERE id = ");
    sql.bind(Id(workspace_id))
        .push_bind(" AND org_id = ", scope.org()?)
        .push(" AND deleted_at IS NULL");
    Ok(scope.fetch_optional(sql).await?.is_some())
}

/// Holds a workspace in use of the scope's organization against other changes of who
/// is in it; says whether there is such a workspace. The lock leaves alone whoever
/// stores a row that refers to the workspace (a board, a membership, a record of the
/// audit log), as the lock of a board does for the board.
pub(crate) async fn hold_workspace(
    scope: &mut Scope<'_>,
    workspace_id: &str,
) -> Result<bool, StoreError> {
    scope.record_lock(LockRank::Workspace, workspace_id);
    let mut sql = scope.sql("SELECT id FROM workspaces WHERE id = ");
    sql.bind(Id(workspace_id))
        .push_bind(" AND org_id = ", scope.org()?)
        .push(" AND deleted_at IS NULL");
    if scope.dialect() == Dialect::Postgres {
        sql.push(" FOR NO KEY UPDATE");
    }
    Ok(scope.fetch_optional(sql).await?.is_some())
}

/// Holds every board of the workspace the user has a role on, in the order of their
/// identifiers, and returns those that are there: one that went to the trash while
/// this waited is none of them.
pub(crate) async fn hold_containers_of(
    scope: &mut Scope<'_>,
    workspace_id: &str,
    user_id: &str,
) -> Result<Vec<String>, StoreError> {
    let mut sql = scope.sql(
        "SELECT bm.container_id FROM container_members AS bm JOIN containers AS b ON b.id = bm.container_id \
         WHERE b.workspace_id = ",
    );
    sql.bind(Id(workspace_id))
        .push_bind(" AND bm.user_id = ", Id(user_id));
    let mut found = BTreeSet::new();
    for row in scope.fetch_all(sql).await? {
        found.insert(row.id(0)?);
    }
    let mut held = Vec::with_capacity(found.len());
    for container_id in found {
        if lock_container(scope, &container_id).await? {
            held.push(container_id);
        }
    }
    Ok(held)
}

/// Whether every board of the workspace the user has a role on is among `held`: read
/// once their membership is held too, it tells of a board they made, or were put on,
/// while the locks were waited for.
async fn holds_every_container_of(
    scope: &mut Scope<'_>,
    workspace_id: &str,
    user_id: &str,
    held: &[String],
) -> Result<bool, StoreError> {
    let mut sql = scope.sql(
        "SELECT bm.container_id FROM container_members AS bm JOIN containers AS b ON b.id = bm.container_id \
         WHERE b.workspace_id = ",
    );
    sql.bind(Id(workspace_id))
        .push_bind(" AND bm.user_id = ", Id(user_id));
    for row in scope.fetch_all(sql).await? {
        if !held.contains(&row.id(0)?) {
            return Ok(false);
        }
    }
    Ok(true)
}

/// What the user is in the workspace, held against whoever counts them among the
/// admins of a board until this transaction ends; `None` for who is no member.
pub(crate) async fn held_role(
    scope: &mut Scope<'_>,
    workspace_id: &str,
    user_id: &str,
) -> Result<Option<WorkspaceRole>, StoreError> {
    scope.record_lock(
        LockRank::WorkspaceMembers,
        &format!("{workspace_id}:{user_id}"),
    );
    let mut sql = scope.sql("SELECT role FROM workspace_members WHERE workspace_id = ");
    sql.bind(Id(workspace_id))
        .push_bind(" AND user_id = ", Id(user_id));
    if scope.dialect() == Dialect::Postgres {
        sql.push(" FOR UPDATE");
    }
    match scope.fetch_optional(sql).await? {
        Some(row) => role_from(&row.text(0)?).map(Some),
        None => Ok(None),
    }
}

/// Whether no other member of the workspace than the user administers it.
pub(crate) async fn is_only_administrator(
    scope: &mut Scope<'_>,
    workspace_id: &str,
    user_id: &str,
) -> Result<bool, StoreError> {
    let mut sql = scope.sql("SELECT 1 FROM workspace_members WHERE workspace_id = ");
    sql.bind(Id(workspace_id))
        .push_bind(" AND user_id <> ", Id(user_id))
        .push_bind(" AND role = ", WorkspaceRole::Admin.as_str())
        .push(" LIMIT 1");
    Ok(scope.fetch_optional(sql).await?.is_none())
}

/// Makes `by` the admin of each of `containers`, which are held, that `user_id` is the only
/// admin of, before `user_id` stops being one, and returns those boards. A board `by`
/// had no role on is recorded as taken.
async fn hand_over(
    scope: &mut Scope<'_>,
    workspace_id: &str,
    containers: &[String],
    user_id: &str,
    by: &str,
) -> Result<Vec<String>, StoreError> {
    let mut taken_over = Vec::new();
    for container_id in containers {
        let administers = find_member(scope, container_id, user_id)
            .await?
            .is_some_and(|member| member.role == ContainerRole::Admin);
        if !administers || !members::is_only_admin(scope, container_id, user_id).await? {
            continue;
        }
        let had_a_role = find_member(scope, container_id, by).await?.is_some();
        let admin = ContainerMember::new(container_id, by, ContainerRole::Admin);
        save_member(scope, &admin).await?;
        if !had_a_role {
            let entry = AuditEntry::new(
                workspace_id,
                by,
                fvoci_model::AuditAction::SelfAddedToContainer,
                container_id,
            );
            audit::record_entry(scope, &entry).await?;
        }
        taken_over.push(container_id.clone());
    }
    Ok(taken_over)
}
