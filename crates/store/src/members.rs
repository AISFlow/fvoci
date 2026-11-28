//! Who belongs to an organization and who is in a workspace.
//!
//! Every statement here names the organization itself: the handle's
//! ([`Tenant::Org`](crate::Tenant::Org)).

use fvoci_model::{
    ContainerKinds, ContainerRole, ContainerVisibility, InstanceRole, OrgRole, WorkspaceRole,
};

use crate::audit::AuditActor;
use crate::db::{Id, LockRank, RowLock, Scope, Sql};
use crate::error::StoreError;
use crate::{Store, container_kinds, org_admin};

/// Opens the condition that the board `b` is in a workspace in use the membership `wm`
/// is of: the caller narrows the memberships and closes the parenthesis. A workspace in
/// the trash has no members to anybody.
const IN_USE_MEMBERSHIPS: &str = " AND b.workspace_id IN (SELECT wm.workspace_id \
     FROM workspace_members AS wm JOIN workspaces AS ws ON ws.id = wm.workspace_id \
     WHERE ws.deleted_at IS NULL";

/// Opens the condition on a board `b` that it is an open board of `kinds` in a workspace
/// the user works in (a member who is no guest: `WorkspaceAccess::works_there`), "or":
/// the caller appends the other way to the board and closes the parenthesis. An open
/// board is no secret to who works in its workspace, and to nobody else.
pub(crate) fn push_open_containers_at_work<'sql>(
    sql: &'sql mut Sql,
    user_id: &str,
    kinds: ContainerKinds,
) -> &'sql mut Sql {
    sql.push_bind(" AND ((b.visibility = ", ContainerVisibility::Open.as_str());
    container_kinds::push_kinds(sql, "b.kind", kinds);
    sql.push(IN_USE_MEMBERSHIPS)
        .push_bind(" AND wm.user_id = ", Id(user_id))
        .push_bind(" AND wm.role <> ", WorkspaceRole::Guest.as_str())
        .push(")) OR ")
}

/// Pushes the condition that the membership `bm` is the user's and that the user is
/// still a member of the board's workspace `b`, a guest too: a role left on a board after
/// leaving its workspace finds nothing.
pub(crate) fn push_own_membership_in_workspace<'sql>(
    sql: &'sql mut Sql,
    user_id: &str,
) -> &'sql mut Sql {
    sql.push_bind("bm.user_id = ", Id(user_id))
        .push(IN_USE_MEMBERSHIPS)
        .push_bind(" AND wm.user_id = ", Id(user_id))
        .push(")")
}

impl Store {
    /// Makes a user a member of the handle's organization.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] if the handle names no organization, and
    /// [`StoreError::Database`] if the statement fails, as it does for a user who is a
    /// member already or does not exist.
    pub async fn add_organization_member(
        &self,
        user_id: &str,
        role: OrgRole,
    ) -> Result<(), StoreError> {
        self.with_write_conn(async |scope| add_organization_member(scope, user_id, role).await)
            .await
    }

    /// The identifiers of who works in a workspace of the handle's organization: its
    /// members who are no guests, by identifier.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn get_workspace_worker_ids(
        &self,
        workspace_id: &str,
    ) -> Result<Vec<String>, StoreError> {
        self.with_conn(async |scope| {
            let mut sql = scope.sql("SELECT user_id FROM workspace_members WHERE workspace_id = ");
            sql.bind(Id(workspace_id))
                .push_bind(" AND role <> ", WorkspaceRole::Guest.as_str())
                .push(" ORDER BY user_id");
            scope.all(sql).await
        })
        .await
    }

    /// Makes a user a member of a workspace of the handle's organization.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] if the handle names no organization,
    /// [`StoreError::NotFound`] if the organization has no such workspace, and
    /// [`StoreError::Database`] if the statement fails, as it does for a user who is a
    /// member already or does not exist.
    #[cfg(any(test, feature = "test-util"))]
    pub async fn add_workspace_member(
        &self,
        workspace_id: &str,
        user_id: &str,
        role: WorkspaceRole,
    ) -> Result<(), StoreError> {
        self.with_write_conn(async |scope| {
            let mut sql = scope.sql(
                "INSERT INTO workspace_members (workspace_id, user_id, role, created_at) SELECT id, ",
            );
            sql.bind(Id(user_id))
                .push_bind(", ", role.as_str())
                .push_bind(", ", scope.now_millis())
                .push_bind(" FROM workspaces WHERE id = ", Id(workspace_id))
                .push_bind(" AND org_id = ", scope.org()?)
                .push(" AND deleted_at IS NULL");
            if scope.execute(sql).await? == 0 {
                return Err(StoreError::not_found("workspace"));
            }
            Ok(())
        })
        .await
    }
}

async fn add_organization_member(
    scope: &mut Scope<'_>,
    user_id: &str,
    role: OrgRole,
) -> Result<(), StoreError> {
    let mut sql =
        scope.sql("INSERT INTO organization_members (org_id, user_id, role, created_at) VALUES (");
    sql.bind(scope.org()?)
        .push_bind(", ", Id(user_id))
        .push_bind(", ", role.as_str())
        .push_bind(", ", scope.now_millis())
        .push(")");
    scope.execute(sql).await?;
    Ok(())
}

/// The role in an organization that goes with being, or not being, an administrator
/// of the instance.
const fn org_role_of_instance(role: InstanceRole) -> OrgRole {
    match role {
        InstanceRole::Administrator => OrgRole::Admin,
        InstanceRole::User => OrgRole::Member,
    }
}

/// Gives a member of the organization the role in it that goes with being, or no
/// longer being, an administrator of the instance, as the organization's route does
/// (`org_admin::change_org_role`): recorded, and who stops being one is taken out of
/// what they joined as its administrator, by `actor`. No role in a
/// workspace changes. Somebody who is no member stays none.
///
/// Instance administrators stay coupled to the oldest organization by product policy.
pub(crate) async fn follow_instance_role(
    scope: &mut Scope<'_>,
    user_id: &str,
    instance_role: InstanceRole,
    actor: &AuditActor,
) -> Result<(), StoreError> {
    let org_role = org_role_of_instance(instance_role);
    let (org, _) = org_admin::lock_organization(scope).await?;
    match org_admin::org_role_of(scope, user_id).await? {
        Some(before) if before != org_role => {
            org_admin::change_org_role(scope, &org.id, user_id, org_role, actor).await?;
        }
        // Already of that role; one who is no administrator has joined nothing to keep.
        _ if org_role != OrgRole::Admin => {
            org_admin::revoke_joinings(scope, user_id, actor).await?;
        }
        _ => {}
    }
    Ok(())
}

/// Whether no other member of the board than the user is an administrator.
pub(crate) async fn is_only_admin(
    scope: &mut Scope<'_>,
    container_id: &str,
    user_id: &str,
) -> Result<bool, StoreError> {
    Ok(!has_acting_admin(scope, container_id, Some(user_id)).await?)
}

/// Whether the board has no administrator who can act as one.
pub(crate) async fn has_no_admin(
    scope: &mut Scope<'_>,
    container_id: &str,
) -> Result<bool, StoreError> {
    Ok(!has_acting_admin(scope, container_id, None).await?)
}

/// Whether the user works in the workspace: a member of it who is no guest.
pub(crate) async fn works_in(
    scope: &mut Scope<'_>,
    workspace_id: &str,
    user_id: &str,
) -> Result<bool, StoreError> {
    let mut sql = scope.sql("SELECT 1 FROM workspace_members WHERE workspace_id = ");
    sql.bind(Id(workspace_id))
        .push_bind(" AND user_id = ", Id(user_id))
        .push_bind(" AND role <> ", WorkspaceRole::Guest.as_str());
    scope.exists(sql).await
}

/// Holds the membership of a board's creator in its workspace before they are made its
/// admin, from here to the end of the transaction, and refuses one who does not work
/// there. The server let them make the board before its transaction: who takes them out
/// of the workspace, or makes them a guest of it, at the same time waits for the board
/// and hands it over, or is waited for, and the board is not stored. Asked after the
/// boards are written, as who changes a membership holds the boards before it.
///
/// # Errors
///
/// Returns [`StoreError::NotFound`] for the workspace if the creator is no member of
/// it, or a guest of it.
pub(crate) async fn hold_creator(
    scope: &mut Scope<'_>,
    workspace_id: &str,
    user_id: &str,
) -> Result<(), StoreError> {
    let mut sql = scope.sql("SELECT role FROM workspace_members WHERE workspace_id = ");
    sql.bind(Id(workspace_id))
        .push_bind(" AND user_id = ", Id(user_id));
    sql.push_lock(scope.row_lock(
        LockRank::WorkspaceMembers,
        &format!("{workspace_id}:{user_id}"),
        RowLock::Share,
    ));
    match scope.fetch_optional(sql).await? {
        Some(row) if row.text(0)? != WorkspaceRole::Guest.as_str() => Ok(()),
        _ => Err(StoreError::not_found("workspace")),
    }
}

/// Whether a member of the board, other than `except` if one is named, is an admin of
/// it who can act as one.
async fn has_acting_admin(
    scope: &mut Scope<'_>,
    container_id: &str,
    except: Option<&str>,
) -> Result<bool, StoreError> {
    // An admin counts who can act as one: a member of the board's workspace who is no
    // guest. A guest is no more than an editor, and who has left opens nothing.
    let mut sql = scope.sql(
        "SELECT 1 FROM container_members AS bm JOIN containers AS b ON b.id = bm.container_id \
         JOIN workspace_members AS wm \
         ON wm.workspace_id = b.workspace_id AND wm.user_id = bm.user_id WHERE bm.container_id = ",
    );
    sql.bind(Id(container_id))
        .push_bind(" AND bm.role = ", ContainerRole::Admin.as_str())
        .push_bind(" AND wm.role <> ", WorkspaceRole::Guest.as_str());
    if let Some(user_id) = except {
        sql.push_bind(" AND bm.user_id <> ", Id(user_id));
    }
    sql.push(" LIMIT 1");
    sql.push_lock(scope.row_lock(
        LockRank::WorkspaceMembers,
        container_id,
        RowLock::Share.of("wm"),
    ));
    scope.exists(sql).await
}
