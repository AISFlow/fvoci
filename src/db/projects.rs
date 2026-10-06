#![allow(clippy::too_many_arguments)]

use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::db::backend::{Backend, DbTx, FamilyTx, OperationTx};
use crate::db::codec::{Cell, FamilyRow};
use crate::db::context::{
    begin_read, lock_membership_users, lock_tree, recheck_session, session_is_live, set_tenant,
};
use crate::db::documents::{empty_document_json, to_path_label, DOCUMENT_SCHEMA_VERSION};
use crate::db::group_grants::{
    group_project_grant_exists_sql, group_project_grant_roles_select_sql,
};
use crate::db::identity::{append_audit, append_event, AuditAppend, EventAppend};
use crate::db::workspace::{
    membership_role, membership_role_for_update, workspace_is_live, WorkspaceRole,
};
use crate::projects::{
    effective_permission, optional_text_to_db, ProjectMemberRole, ProjectPermission,
};

const PRIVATE_LEAD_SQLSTATE: &str = "23514";

#[derive(Debug)]
pub enum ProjectDbError {
    NotFound,
    Forbidden,
    Conflict,
    LastLead,
    GuestLead,
    LeadNotMember,
    Archived,
    InvalidCursor,
    VersionConflict,
    TaskArchived,
    InvalidAnchor,
    StatusNotInWorkflow,
    WipLimitExceeded,
    InvalidMoveAnchors,
    WorkflowHasNoStatuses,
    AssigneeIsNotAMember,
    LabelNotFound,
    MilestoneNotFound,
    DependencyNotFound,
    DependencyCycle,
    DependencyContradiction,
    TaskCannotBlockItself,
    InvalidInput,
    OpenTimeEntryExists,
    StatusHasTasks,
    WorkflowStatusLimit,
}

#[derive(Debug, Clone)]
pub struct ProjectRow {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub key: String,
    pub name: String,
    pub description: Option<String>,
    pub icon: Option<String>,
    pub visibility: String,
    pub root_document_id: Option<Uuid>,
    pub status: String,
    pub created_by: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct ProjectListItem {
    pub project: ProjectRow,
    pub document_count: i64,
    pub task_count: i64,
    pub open_task_count: i64,
    pub can_edit: bool,
    pub can_manage: bool,
}

#[derive(Debug, Clone)]
pub struct ProjectMemberRow {
    pub user_id: Uuid,
    pub email: String,
    pub given_name: String,
    pub family_name: Option<String>,
    pub role: ProjectMemberRole,
}

#[derive(Debug, Clone)]
pub struct WorkflowStatusRow {
    pub id: Uuid,
    pub name: String,
    pub category: String,
    pub sort_key: String,
    pub wip_limit: Option<i32>,
}

#[derive(Debug, Clone)]
pub struct WorkflowRow {
    pub id: Uuid,
    pub project_id: Uuid,
    pub statuses: Vec<WorkflowStatusRow>,
}

pub struct CreateProjectInput<'a> {
    pub key: &'a str,
    pub name: &'a str,
    pub visibility: &'a str,
    pub description: Option<&'a str>,
    pub icon: Option<&'a str>,
    pub lead_user_id: Option<Uuid>,
}

pub struct CloneProjectInput<'a> {
    pub key: &'a str,
    pub name: &'a str,
    pub visibility: Option<&'a str>,
    pub description: Option<Option<&'a str>>,
    pub icon: Option<Option<&'a str>>,
    pub lead_user_id: Option<Uuid>,
}

pub struct UpdateProjectInput<'a> {
    pub name: Option<&'a str>,
    pub visibility: Option<&'a str>,
    pub description: Option<Option<&'a str>>,
    pub icon: Option<Option<&'a str>>,
    pub lead_user_id: Option<Uuid>,
}

struct ProjectChangeRecord<'a> {
    workspace_id: Uuid,
    actor_user_id: Uuid,
    verb: &'a str,
    target_type: &'a str,
    target_id: Uuid,
    payload: Value,
    client_ip: Option<&'a str>,
}

/// A live project row: [`lock_project`] returns it under a row lock,
/// [`load_live_project`] without one.
pub(crate) struct LiveProject {
    pub id: Uuid,
    pub key: String,
    pub name: String,
    pub description: Option<String>,
    pub icon: Option<String>,
    pub visibility: String,
    pub root_document_id: Option<Uuid>,
    pub status: String,
    pub created_by: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub(crate) async fn project_member_role(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    project_id: Uuid,
    user_id: Uuid,
) -> Result<Option<ProjectMemberRole>, sqlx::Error> {
    OperationTx::Postgres(tx)
        .project_member_role(workspace_id, project_id, user_id)
        .await
}

impl OperationTx<'_, '_> {
    /// Current direct and group grants, evaluated in the caller's tenant
    /// transaction. The caller retains credential/membership and commit ownership.
    pub(crate) async fn project_member_role(
        &mut self,
        workspace: Uuid,
        project: Uuid,
        user: Uuid,
    ) -> Result<Option<ProjectMemberRole>, sqlx::Error> {
        let roles = match self {
            Self::Postgres(tx) => sqlx::query_as::<_, (String,)>(&format!(
                r#"SELECT role FROM fvoci.project_members
                   WHERE workspace_id=$1 AND project_id=$2 AND user_id=$3
                   UNION ALL {}"#,
                group_project_grant_roles_select_sql(1, 2, 3)
            ))
            .bind(workspace)
            .bind(project)
            .bind(user)
            .fetch_all(&mut ***tx)
            .await?
            .into_iter()
            .map(|(role,)| role)
            .collect::<Vec<_>>(),
            Self::SqliteFamily(tx) => {
                tx.require_tenant(workspace)?;
                tx.query(
                    "SELECT role FROM project_members
                     WHERE workspace_id=?1 AND project_id=?2 AND user_id=?3
                     UNION ALL
                     SELECT pm.role FROM project_members pm
                     INNER JOIN group_members gm
                       ON gm.workspace_id=pm.workspace_id AND gm.group_id=pm.group_id
                     WHERE pm.workspace_id=?1 AND pm.project_id=?2
                       AND gm.user_id=?3 AND pm.group_id IS NOT NULL",
                    &[Cell::uuid(workspace), Cell::uuid(project), Cell::uuid(user)],
                )
                .await?
                .iter()
                .map(|row| row.cell(0)?.string())
                .collect::<Result<Vec<_>, sqlx::Error>>()?
            }
        };
        Ok(roles
            .iter()
            .filter_map(|role| ProjectMemberRole::parse(role))
            .max_by_key(|role| role.permission()))
    }

    /// Effective permission on a live project in this snapshot. As in the
    /// PostgreSQL entrypoint, consumers must first validate current credentials,
    /// live workspace and workspace membership in this same transaction.
    pub(crate) async fn project_permission_by_id(
        &mut self,
        workspace: Uuid,
        actor: Uuid,
        project: Uuid,
    ) -> Result<Option<ProjectPermission>, sqlx::Error> {
        let visibility: Option<String> = match self {
            Self::Postgres(tx) => sqlx::query_scalar(
                "SELECT visibility FROM fvoci.projects WHERE workspace_id=$1 AND id=$2 AND deleted_at IS NULL"
            ).bind(workspace).bind(project).fetch_optional(&mut ***tx).await?,
            Self::SqliteFamily(tx) => {
                tx.require_tenant(workspace)?;
                tx.query(
                    "SELECT visibility FROM projects WHERE workspace_id=?1 AND id=?2 AND deleted_at IS NULL",
                    &[Cell::uuid(workspace), Cell::uuid(project)],
                ).await?.first().map(|row| row.cell(0)?.string()).transpose()?
            }
        };
        let Some(visibility) = visibility else {
            return Ok(None);
        };
        let workspace_role = self
            .membership_role(workspace, actor, false)
            .await?
            .unwrap_or(WorkspaceRole::Guest);
        let member_role = self.project_member_role(workspace, project, actor).await?;
        Ok(Some(effective_permission(
            workspace_role,
            &visibility,
            member_role,
        )))
    }
}

async fn direct_project_member_role(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    project_id: Uuid,
    user_id: Uuid,
) -> Result<Option<ProjectMemberRole>, sqlx::Error> {
    let row: Option<(String,)> = sqlx::query_as(
        r#"
        SELECT role FROM fvoci.project_members
        WHERE workspace_id = $1 AND project_id = $2 AND user_id = $3
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .bind(user_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.and_then(|(role,)| ProjectMemberRole::parse(&role)))
}

/// Visibility predicate matching `project_permission`: workspace-visible to
/// non-guests, otherwise a direct user row or a group grant for the actor.
pub(crate) fn visible_project_sql(
    project_alias: &str,
    guest_param: u32,
    actor_param: u32,
) -> String {
    visible_project_predicate(project_alias, &format!("${guest_param}"), actor_param)
}

/// `visible_project_sql` with the actor's guest flag inlined as a SQL literal
/// (a server-computed boolean, never request text), for queries that have no
/// boolean bind for it, such as the task list, whose trailing binds are a text
/// list. The caller still binds the actor's user id (a `uuid`) at
/// `actor_param`.
pub(crate) fn visible_project_sql_for_guest(
    project_alias: &str,
    guest: bool,
    actor_param: u32,
) -> String {
    visible_project_predicate(
        project_alias,
        if guest { "true" } else { "false" },
        actor_param,
    )
}

fn visible_project_predicate(project_alias: &str, guest_sql: &str, actor_param: u32) -> String {
    format!(
        "(
            ({project_alias}.visibility = 'workspace' AND {guest_sql} = false)
            OR EXISTS (
                SELECT 1 FROM fvoci.project_members pm
                WHERE pm.workspace_id = {project_alias}.workspace_id
                  AND pm.project_id = {project_alias}.id
                  AND pm.user_id = ${actor_param}
            )
            OR {group_exists}
        )",
        group_exists = group_project_grant_exists_sql(project_alias, actor_param),
    )
}

async fn count_project_leads(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    project_id: Uuid,
) -> Result<i64, sqlx::Error> {
    let row: (i64,) = sqlx::query_as(
        r#"
        SELECT count(*) FROM fvoci.project_members
        WHERE workspace_id = $1 AND project_id = $2 AND role = 'lead'
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .fetch_one(&mut **tx)
    .await?;
    Ok(row.0)
}

pub(crate) async fn count_project_leads_except(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    project_id: Uuid,
    except_user_id: Option<Uuid>,
    except_group_id: Option<Uuid>,
) -> Result<i64, sqlx::Error> {
    let row: (i64,) = sqlx::query_as(
        r#"
        SELECT count(*) FROM fvoci.project_members
        WHERE workspace_id = $1 AND project_id = $2 AND role = 'lead'
          AND ($3::uuid IS NULL OR user_id IS DISTINCT FROM $3)
          AND ($4::uuid IS NULL OR group_id IS DISTINCT FROM $4)
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .bind(except_user_id)
    .bind(except_group_id)
    .fetch_one(&mut **tx)
    .await?;
    Ok(row.0)
}

async fn count_project_leads_excluding(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    project_id: Uuid,
    exclude_user_id: Uuid,
) -> Result<i64, sqlx::Error> {
    count_project_leads_except(tx, workspace_id, project_id, Some(exclude_user_id), None).await
}

/// Columns of one live project row, shared by [`lock_project`] and
/// [`load_live_project`].
macro_rules! live_project_select {
    () => {
        r#"
        SELECT id, key, name, description, icon, visibility, root_document_id, status,
               created_by, created_at, updated_at
        FROM fvoci.projects
        WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL
        "#
    };
}

type LiveProjectColumns = (
    Uuid,
    String,
    String,
    Option<String>,
    Option<String>,
    String,
    Option<Uuid>,
    String,
    Uuid,
    DateTime<Utc>,
    DateTime<Utc>,
);

async fn fetch_live_project(
    tx: &mut Transaction<'_, Postgres>,
    sql: &'static str,
    workspace_id: Uuid,
    project_id: Uuid,
) -> Result<Option<LiveProject>, sqlx::Error> {
    let row = sqlx::query_as::<_, LiveProjectColumns>(sql)
        .bind(workspace_id)
        .bind(project_id)
        .fetch_optional(&mut **tx)
        .await?;
    Ok(row.map(
        |(
            id,
            key,
            name,
            description,
            icon,
            visibility,
            root_document_id,
            status,
            created_by,
            created_at,
            updated_at,
        )| LiveProject {
            id,
            key,
            name,
            description,
            icon,
            visibility,
            root_document_id,
            status,
            created_by,
            created_at,
            updated_at,
        },
    ))
}

/// The live project row under `FOR NO KEY UPDATE`, for writers: visibility,
/// archive, trash and member changes serialize with the caller's write.
pub(crate) async fn lock_project(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    project_id: Uuid,
) -> Result<Option<LiveProject>, sqlx::Error> {
    const SQL: &str = concat!(live_project_select!(), "FOR NO KEY UPDATE");
    fetch_live_project(tx, SQL, workspace_id, project_id).await
}

/// The live project row without a row lock: the lock-free twin of
/// [`lock_project`] for checks that only read.
pub(crate) async fn load_live_project(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    project_id: Uuid,
) -> Result<Option<LiveProject>, sqlx::Error> {
    const SQL: &str = live_project_select!();
    fetch_live_project(tx, SQL, workspace_id, project_id).await
}

pub(crate) async fn project_permission(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    project: &LiveProject,
) -> Result<ProjectPermission, sqlx::Error> {
    let workspace_role = membership_role(tx, workspace_id, actor_user_id)
        .await?
        .unwrap_or(WorkspaceRole::Guest);
    let member_role = project_member_role(tx, workspace_id, project.id, actor_user_id).await?;
    Ok(effective_permission(
        workspace_role,
        &project.visibility,
        member_role,
    ))
}

/// Effective permission on a live project by id, without taking the project
/// lock (read-only display checks, e.g. an activity entry's parent title).
pub(crate) async fn project_permission_by_id(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    project_id: Uuid,
) -> Result<Option<ProjectPermission>, sqlx::Error> {
    OperationTx::Postgres(tx)
        .project_permission_by_id(workspace_id, actor_user_id, project_id)
        .await
}

/// Read check for rows owned by a project (labels, milestones): a live
/// credential ([`session_is_live`]), a live workspace and at least View on the
/// live project. Takes no row lock; callers run it in a [`begin_read`]
/// transaction so the check and the rows they return share one snapshot.
/// `Forbidden` for a dead credential, `NotFound` for everything else.
/// (Unrelated to the private `project_views::require_project_view`.)
pub(crate) async fn require_project_view(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    project_id: Uuid,
) -> Result<Result<(), ProjectDbError>, sqlx::Error> {
    if !session_is_live(tx, actor_user_id, session_id).await? {
        return Ok(Err(ProjectDbError::Forbidden));
    }
    if !workspace_is_live(tx, workspace_id).await? {
        return Ok(Err(ProjectDbError::NotFound));
    }
    let Some(project) = load_live_project(tx, workspace_id, project_id).await? else {
        return Ok(Err(ProjectDbError::NotFound));
    };
    let permission = project_permission(tx, workspace_id, actor_user_id, &project).await?;
    if !permission.at_least(ProjectPermission::View) {
        return Ok(Err(ProjectDbError::NotFound));
    }
    Ok(Ok(()))
}

/// Write check for rows owned by a project (labels, milestones): the actor's
/// membership advisory lock, [`recheck_session`] under row locks, a live
/// workspace, then the project row under [`lock_project`], so a revocation,
/// archive or visibility change cannot commit between this check and the
/// caller's write. `Forbidden` for a dead credential, `Archived` for an
/// archived project, `NotFound` for a gone workspace or project or less than
/// Edit.
pub(crate) async fn require_project_edit(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    project_id: Uuid,
) -> Result<Result<(), ProjectDbError>, sqlx::Error> {
    lock_membership_users(tx, &[actor_user_id]).await?;
    if !recheck_session(tx, actor_user_id, session_id).await? {
        return Ok(Err(ProjectDbError::Forbidden));
    }
    if !workspace_is_live(tx, workspace_id).await? {
        return Ok(Err(ProjectDbError::NotFound));
    }
    let Some(locked) = lock_project(tx, workspace_id, project_id).await? else {
        return Ok(Err(ProjectDbError::NotFound));
    };
    if locked.status == "archived" {
        return Ok(Err(ProjectDbError::Archived));
    }
    let permission = project_permission(tx, workspace_id, actor_user_id, &locked).await?;
    if !permission.at_least(ProjectPermission::Edit) {
        return Ok(Err(ProjectDbError::NotFound));
    }
    Ok(Ok(()))
}

/// Effective permission on a live project under a `FOR SHARE` row lock, plus
/// whether the project is archived. Collab writers on sibling documents share
/// the lock; project mutations that take `FOR NO KEY UPDATE` (visibility,
/// archive, trash, member/group grants) wait until this transaction ends, so a
/// revocation cannot interleave between this check and the caller's write.
pub(crate) async fn share_lock_project_permission(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    project_id: Uuid,
) -> Result<Option<(ProjectPermission, bool)>, sqlx::Error> {
    OperationTx::Postgres(tx)
        .share_lock_project_permission(workspace_id, actor_user_id, project_id)
        .await
}

impl OperationTx<'_, '_> {
    pub(crate) async fn share_lock_project_permission(
        &mut self,
        workspace: Uuid,
        actor: Uuid,
        project: Uuid,
    ) -> Result<Option<(ProjectPermission, bool)>, sqlx::Error> {
        let row: Option<(String, String)> = match self {
            Self::Postgres(tx) => sqlx::query_as(
                "SELECT visibility,status FROM fvoci.projects WHERE workspace_id=$1 AND id=$2 AND deleted_at IS NULL FOR SHARE"
            ).bind(workspace).bind(project).fetch_optional(&mut ***tx).await?,
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                tx.require_tenant(workspace)?;
                tx.query(
                    "SELECT visibility,status FROM projects WHERE workspace_id=?1 AND id=?2 AND deleted_at IS NULL",
                    &[Cell::uuid(workspace),Cell::uuid(project)],
                ).await?.first().map(|row| Ok::<_, sqlx::Error>((row.cell(0)?.string()?,row.cell(1)?.string()?)))
                    .transpose()?
            }
        };
        let Some((visibility, status)) = row else {
            return Ok(None);
        };
        let workspace_role = self
            .membership_role(workspace, actor, false)
            .await?
            .unwrap_or(WorkspaceRole::Guest);
        let member_role = self.project_member_role(workspace, project, actor).await?;
        Ok(Some((
            effective_permission(workspace_role, &visibility, member_role),
            status == "archived",
        )))
    }
}

async fn record_project_event_and_audit(
    tx: &mut Transaction<'_, Postgres>,
    change: ProjectChangeRecord<'_>,
) -> Result<(), sqlx::Error> {
    append_event(
        tx,
        EventAppend {
            id: Uuid::now_v7(),
            workspace_id: Some(change.workspace_id),
            actor_user_id: Some(change.actor_user_id),
            verb: change.verb.to_string(),
            target_type: Some(change.target_type.to_string()),
            target_id: Some(change.target_id),
            payload: change.payload.clone(),
        },
    )
    .await?;
    append_audit(
        tx,
        AuditAppend {
            id: Uuid::now_v7(),
            workspace_id: Some(change.workspace_id),
            actor_user_id: Some(change.actor_user_id),
            verb: change.verb.to_string(),
            target_type: Some(change.target_type.to_string()),
            target_id: Some(change.target_id),
            payload: change.payload,
            ip: change.client_ip.map(str::to_string),
        },
    )
    .await?;
    Ok(())
}

pub(crate) fn is_private_lead_violation(err: &sqlx::Error) -> bool {
    err.as_database_error()
        .and_then(|db| db.code())
        .is_some_and(|code| code.as_ref() == PRIVATE_LEAD_SQLSTATE)
}

pub(crate) async fn seed_workflow(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    project_id: Uuid,
) -> Result<Uuid, sqlx::Error> {
    let workflow_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO fvoci.workflows (id, workspace_id, project_id)
        VALUES ($1, $2, $3)
        "#,
    )
    .bind(workflow_id)
    .bind(workspace_id)
    .bind(project_id)
    .execute(&mut **tx)
    .await?;

    use crate::settings::messages::Message;
    // Names come from the instance overrides at seed time; statuses already
    // created (or copied by a project clone) keep their names.
    let messages = crate::settings::messages::load(&mut **tx).await?;
    let seeds: [(Message, &str, &str); 6] = [
        (Message::SeedStatusBacklog, "backlog", "V"),
        (Message::SeedStatusTodo, "todo", "W"),
        (Message::SeedStatusInProgress, "in_progress", "X"),
        (Message::SeedStatusReview, "in_progress", "Y"),
        (Message::SeedStatusDone, "done", "Z"),
        (Message::SeedStatusCanceled, "canceled", "a"),
    ];
    for (message, category, sort_key) in seeds {
        let name = messages.field(message, |name| {
            crate::db::workflow_statuses::status_name_is_valid(name.trim())
        });
        sqlx::query(
            r#"
            INSERT INTO fvoci.statuses (id, workspace_id, project_id, workflow_id, name, category, sort_key)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(workspace_id)
        .bind(project_id)
        .bind(workflow_id)
        .bind(name)
        .bind(category)
        .bind(sort_key)
        .execute(&mut **tx)
        .await?;
    }
    Ok(workflow_id)
}

impl OperationTx<'_, '_> {
    pub(crate) async fn workspace_removal_blocked_by_private_leads(
        &mut self,
        workspace: Uuid,
        target: Uuid,
    ) -> Result<bool, sqlx::Error> {
        match self {
            Self::Postgres(tx) => {
                workspace_removal_blocked_by_private_leads(tx, workspace, target).await
            }
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                tx.require_tenant(workspace)?;
                // The maintained policy counts lead grant rows, including
                // NULL-user group grants, not the group's current user count.
                let rows = tx.query(
                    "SELECT EXISTS(SELECT 1 FROM projects p JOIN project_members mine ON mine.workspace_id=p.workspace_id AND mine.project_id=p.id WHERE p.workspace_id=?1 AND p.deleted_at IS NULL AND p.visibility='private' AND mine.user_id=?2 AND mine.role='lead' AND NOT EXISTS(SELECT 1 FROM project_members other WHERE other.workspace_id=p.workspace_id AND other.project_id=p.id AND other.role='lead' AND (other.user_id IS NULL OR other.user_id<>?2)))",
                    &[Cell::uuid(workspace), Cell::uuid(target)],
                ).await?;
                rows.first()
                    .ok_or(sqlx::Error::RowNotFound)?
                    .cell(0)?
                    .boolean()
            }
        }
    }
}

#[cfg(all(test, feature = "db-tests"))]
mod selected_member_removal_lead_tests {
    use super::*;
    use crate::db::workspace::selected_member_removal_tests::{fixture, project, remove, snapshot};

    #[tokio::test]
    async fn sqlite_workspace_removal_private_archived_direct_group_and_writer_scope() {
        let (f, credential, target, _) = fixture().await;
        let project = project(&f, target, "private").await;
        let before = snapshot(&f).await;
        assert!(matches!(
            remove(&f, credential, target).await.unwrap(),
            Err(crate::db::workspace::WorkspaceDbError::LastProjectLead)
        ));
        assert_eq!(snapshot(&f).await, before);
        sqlx::query("UPDATE projects SET status='archived' WHERE id=?1")
            .bind(project.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let before = snapshot(&f).await;
        assert!(matches!(
            remove(&f, credential, target).await.unwrap(),
            Err(crate::db::workspace::WorkspaceDbError::LastProjectLead)
        ));
        assert_eq!(snapshot(&f).await, before);
        let mut read = f.backend.begin_read().await.unwrap();
        read.operation().set_tenant(f.workspace).await.unwrap();
        assert!(read
            .operation()
            .workspace_removal_blocked_by_private_leads(f.workspace, target)
            .await
            .is_err());
        read.rollback().await.unwrap();
        let mut tx = f.backend.begin_write().await.unwrap();
        {
            let mut op = tx.operation();
            op.set_tenant(f.workspace).await.unwrap();
            assert!(op
                .workspace_removal_blocked_by_private_leads(Uuid::now_v7(), target)
                .await
                .is_err());
            assert!(op
                .workspace_removal_blocked_by_private_leads(f.workspace, target)
                .await
                .unwrap());
        }
        tx.rollback().await.unwrap();
        sqlx::query("UPDATE projects SET deleted_at=1 WHERE id=?1")
            .bind(project.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let mut tx = f.backend.begin_write().await.unwrap();
        tx.operation().set_tenant(f.workspace).await.unwrap();
        assert!(!tx
            .operation()
            .workspace_removal_blocked_by_private_leads(f.workspace, target)
            .await
            .unwrap());
        tx.rollback().await.unwrap();
        sqlx::query("UPDATE projects SET deleted_at=NULL WHERE id=?1")
            .bind(project.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let group = Uuid::now_v7();
        let grant = Uuid::now_v7();
        sqlx::query("INSERT INTO groups(id,workspace_id,name) VALUES(?1,?2,'Lead grant retained')")
            .bind(group.as_bytes().as_slice())
            .bind(f.workspace.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        // A NULL-user group lead counts as another grant even with no group users.
        sqlx::query("INSERT INTO project_members(id,workspace_id,project_id,group_id,role) VALUES(?1,?2,?3,?4,'lead')").bind(grant.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(project.as_bytes().as_slice()).bind(group.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        remove(&f, credential, target).await.unwrap().unwrap();
        let kept: (Option<Vec<u8>>, Vec<u8>, String) =
            sqlx::query_as("SELECT user_id,group_id,role FROM project_members WHERE id=?1")
                .bind(grant.as_bytes().as_slice())
                .fetch_one(&f.pool)
                .await
                .unwrap();
        assert_eq!(kept, (None, group.as_bytes().to_vec(), "lead".into()));
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM project_members WHERE project_id=?1 AND user_id=?2"
            )
            .bind(project.as_bytes().as_slice())
            .bind(target.as_bytes().as_slice())
            .fetch_one(&f.pool)
            .await
            .unwrap(),
            0
        );
        crate::db::workspace::selected_personal_workspace_tests::foreign_keys(&f).await;
        f.close().await;
    }
}

pub(crate) async fn workspace_removal_blocked_by_private_leads(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    target_user_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let projects = sqlx::query_as::<_, (Uuid, String)>(
        r#"
        SELECT p.id, p.visibility
        FROM fvoci.projects p
        INNER JOIN fvoci.project_members pm
            ON pm.workspace_id = p.workspace_id AND pm.project_id = p.id
        WHERE p.workspace_id = $1
          AND pm.user_id = $2
          AND pm.role = 'lead'
          AND p.deleted_at IS NULL
        ORDER BY p.id
        FOR NO KEY UPDATE OF p
        "#,
    )
    .bind(workspace_id)
    .bind(target_user_id)
    .fetch_all(&mut **tx)
    .await?;
    for (project_id, visibility) in projects {
        if visibility == "private"
            && count_project_leads_excluding(tx, workspace_id, project_id, target_user_id).await?
                == 0
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Selected project creation uses one current writer for authority, root,
/// workflow, events and audit. Existing PostgreSQL callers keep their wrapper.
pub async fn create_project_backend(
    backend: &Backend,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    input: CreateProjectInput<'_>,
    client_ip: Option<&str>,
) -> Result<Result<ProjectRow, ProjectDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return create_project(
            pool,
            workspace_id,
            actor_user_id,
            session_id,
            input,
            client_ip,
        )
        .await;
    }
    let mut tx = backend.begin_write().await?;
    let result = create_project_operation(
        &mut tx.operation(),
        workspace_id,
        actor_user_id,
        session_id,
        input,
        client_ip,
    )
    .await;
    finish_project_create(tx, result).await
}

/// Borrow the caller's current transaction; creation never owns BEGIN/finish.
/// The caller keeps tenant/current authority and commits all composed effects.
pub(crate) async fn create_project_operation(
    op: &mut OperationTx<'_, '_>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    input: CreateProjectInput<'_>,
    client_ip: Option<&str>,
) -> Result<Result<ProjectRow, ProjectDbError>, sqlx::Error> {
    if let OperationTx::Postgres(tx) = op {
        return create_project_tx(
            tx,
            workspace_id,
            actor_user_id,
            session_id,
            input,
            client_ip,
        )
        .await;
    }
    op.set_tenant(workspace_id).await?;
    let mut users = vec![actor_user_id];
    if let Some(lead) = input.lead_user_id {
        if lead != actor_user_id {
            users.push(lead);
        }
    }
    users.sort_unstable();
    op.lock_membership_users(&users).await?;
    if !op.recheck_session(actor_user_id, session_id).await? {
        return Ok(Err(ProjectDbError::Forbidden));
    }
    if !op.workspace_is_live(workspace_id).await? {
        return Ok(Err(ProjectDbError::NotFound));
    }
    if !op
        .membership_role(workspace_id, actor_user_id, true)
        .await?
        .is_some_and(|role| role.at_least(WorkspaceRole::Member))
    {
        return Ok(Err(ProjectDbError::NotFound));
    }
    op.lock_tree(workspace_id).await?;
    let project = Uuid::now_v7();
    let root = Uuid::now_v7();
    let OperationTx::SqliteFamily(family) = &mut *op else {
        unreachable!()
    };
    family.require_writer()?;
    family.require_tenant(workspace_id)?;
    let inserted = family.query(
        "INSERT INTO projects(id,workspace_id,key,name,description,icon,visibility,status,next_number,created_by)
         VALUES(?1,?2,?3,?4,?5,?6,?7,'active',1,?8)
         ON CONFLICT(workspace_id,key) DO NOTHING RETURNING id",
        &[Cell::uuid(project),Cell::uuid(workspace_id),Cell::text(input.key),Cell::text(input.name.trim()),
          Cell::optional_text(optional_text_to_db(input.description).as_deref()),
          Cell::optional_text(optional_text_to_db(input.icon).as_deref()),
          Cell::text(input.visibility),Cell::uuid(actor_user_id)],
    ).await?;
    if inserted.is_empty() {
        return Ok(Err(ProjectDbError::Conflict));
    }
    family.execute(
        "INSERT INTO project_members(id,workspace_id,project_id,user_id,role) VALUES(?1,?2,?3,?4,'lead')",
        &[Cell::uuid(Uuid::now_v7()),Cell::uuid(workspace_id),Cell::uuid(project),Cell::uuid(actor_user_id)],
    ).await?;
    if let Some(lead) = input.lead_user_id.filter(|lead| *lead != actor_user_id) {
        // Retain the PG ordering: duplicate key precedes a refused lead.
        if !op
            .membership_role(workspace_id, lead, false)
            .await?
            .is_some_and(|role| role.at_least(WorkspaceRole::Member))
        {
            return Ok(Err(ProjectDbError::NotFound));
        }
        let OperationTx::SqliteFamily(family) = &mut *op else {
            unreachable!()
        };
        family.execute(
            "INSERT INTO project_members(id,workspace_id,project_id,user_id,role) VALUES(?1,?2,?3,?4,'lead')
             ON CONFLICT(workspace_id,project_id,user_id) DO UPDATE SET role='lead',updated_at=(unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000)",
            &[Cell::uuid(Uuid::now_v7()),Cell::uuid(workspace_id),Cell::uuid(project),Cell::uuid(lead)],
        ).await?;
        family.execute(
            "UPDATE project_members SET role='member',updated_at=(unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000)
             WHERE workspace_id=?1 AND project_id=?2 AND user_id=?3",
            &[Cell::uuid(workspace_id),Cell::uuid(project),Cell::uuid(actor_user_id)],
        ).await?;
    }
    let OperationTx::SqliteFamily(family) = &mut *op else {
        unreachable!()
    };
    let numbers = family.query(
        "UPDATE projects SET next_number=next_number+1,updated_at=(unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000)
         WHERE workspace_id=?1 AND id=?2 RETURNING next_number-1",
        &[Cell::uuid(workspace_id),Cell::uuid(project)],
    ).await?;
    let number = numbers
        .first()
        .ok_or(sqlx::Error::RowNotFound)?
        .cell(0)?
        .int32()?;
    family.execute(
        "INSERT INTO documents(id,workspace_id,title,path,parent_id,sort_key,project_id,number,status,schema_version,content_json,created_by)
         VALUES(?1,?2,?3,?4,NULL,'V',?5,?6,'published',?7,?8,?9)",
        &[Cell::uuid(root),Cell::uuid(workspace_id),Cell::text(input.name.trim()),Cell::text(to_path_label(root)),
          Cell::uuid(project),Cell::Integer(i64::from(number)),Cell::Integer(i64::from(DOCUMENT_SCHEMA_VERSION)),
          Cell::json(&empty_document_json())?,Cell::uuid(actor_user_id)],
    ).await?;
    family.execute(
        "UPDATE projects SET root_document_id=?3,updated_at=(unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000)
         WHERE workspace_id=?1 AND id=?2",
        &[Cell::uuid(workspace_id),Cell::uuid(project),Cell::uuid(root)],
    ).await?;
    seed_project_workflow_family(family, workspace_id, project).await?;
    let payload = json!({"projectId":project.to_string(),"key":input.key,"name":input.name.trim(),
                         "visibility":input.visibility,"rootDocumentId":root.to_string()});
    op.append_event(EventAppend {
        id: Uuid::now_v7(),
        workspace_id: Some(workspace_id),
        actor_user_id: Some(actor_user_id),
        verb: "project.created".into(),
        target_type: Some("project".into()),
        target_id: Some(project),
        payload: payload.clone(),
    })
    .await?;
    op.append_audit(AuditAppend {
        id: Uuid::now_v7(),
        workspace_id: Some(workspace_id),
        actor_user_id: Some(actor_user_id),
        verb: "project.created".into(),
        target_type: Some("project".into()),
        target_id: Some(project),
        payload,
        ip: client_ip.map(str::to_string),
    })
    .await?;
    let OperationTx::SqliteFamily(family) = &mut *op else {
        unreachable!()
    };
    let rows = family.query(
        "SELECT id,key,name,description,icon,visibility,root_document_id,status,created_by,created_at,updated_at
         FROM projects WHERE workspace_id=?1 AND id=?2",
        &[Cell::uuid(workspace_id),Cell::uuid(project)],
    ).await?;
    Ok(Ok(project_created_family_row(
        rows.first().ok_or(sqlx::Error::RowNotFound)?,
        workspace_id,
    )?))
}

async fn seed_project_workflow_family(
    family: &mut FamilyTx,
    workspace: Uuid,
    project: Uuid,
) -> Result<(), sqlx::Error> {
    use crate::settings::messages::{Message, Messages};
    family.require_writer()?;
    family.require_tenant(workspace)?;
    let workflow = Uuid::now_v7();
    family
        .execute(
            "INSERT INTO workflows(id,workspace_id,project_id) VALUES(?1,?2,?3)",
            &[
                Cell::uuid(workflow),
                Cell::uuid(workspace),
                Cell::uuid(project),
            ],
        )
        .await?;
    // Read the current override through the same writer; reuse its canonical
    // validation and field fallback, without another settings lease/transaction.
    let rows = family
        .query("SELECT value FROM instance_settings WHERE key='i18n'", &[])
        .await?;
    let raw = rows.first().map(|row| row.cell(0)?.value()).transpose()?;
    let messages = Messages::from_row(raw.as_ref());
    for (message, category, sort_key) in [
        (Message::SeedStatusBacklog, "backlog", "V"),
        (Message::SeedStatusTodo, "todo", "W"),
        (Message::SeedStatusInProgress, "in_progress", "X"),
        (Message::SeedStatusReview, "in_progress", "Y"),
        (Message::SeedStatusDone, "done", "Z"),
        (Message::SeedStatusCanceled, "canceled", "a"),
    ] {
        let name = messages.field(message, |name| {
            crate::db::workflow_statuses::status_name_is_valid(name.trim())
        });
        family.execute("INSERT INTO statuses(id,workspace_id,project_id,workflow_id,name,category,sort_key) VALUES(?1,?2,?3,?4,?5,?6,?7)",
            &[Cell::uuid(Uuid::now_v7()),Cell::uuid(workspace),Cell::uuid(project),Cell::uuid(workflow),
              Cell::text(name),Cell::text(category),Cell::text(sort_key)]).await?;
    }
    Ok(())
}

fn project_created_family_row(
    row: &FamilyRow,
    workspace_id: Uuid,
) -> Result<ProjectRow, sqlx::Error> {
    Ok(ProjectRow {
        id: row.cell(0)?.id()?,
        workspace_id,
        key: row.cell(1)?.string()?,
        name: row.cell(2)?.string()?,
        description: row.cell(3)?.optional(Cell::string)?,
        icon: row.cell(4)?.optional(Cell::string)?,
        visibility: row.cell(5)?.string()?,
        root_document_id: row.cell(6)?.optional(Cell::id)?,
        status: row.cell(7)?.string()?,
        created_by: row.cell(8)?.id()?,
        created_at: row.cell(9)?.datetime()?,
        updated_at: row.cell(10)?.datetime()?,
    })
}

#[derive(Debug, thiserror::Error)]
#[error("project creation refused: {0:?}")]
struct ProjectCreateRefusal(ProjectDbError);

async fn finish_project_create(
    tx: DbTx,
    result: Result<Result<ProjectRow, ProjectDbError>, sqlx::Error>,
) -> Result<Result<ProjectRow, ProjectDbError>, sqlx::Error> {
    match result {
        Ok(Ok(project)) => {
            tx.commit_with_cleanup()
                .await
                .map_err(|error| sqlx::Error::AnyDriverError(Box::new(error)))?;
            Ok(Ok(project))
        }
        Ok(Err(refusal)) => {
            if let Err(cleanup) = tx.rollback().await {
                return Err(crate::db::backend::rollback_cleanup_unknown(
                    Some(Box::new(ProjectCreateRefusal(refusal))),
                    cleanup,
                ));
            }
            Ok(Err(refusal))
        }
        Err(original) => {
            if let Err(cleanup) = tx.rollback().await {
                return Err(crate::db::backend::rollback_cleanup_unknown(
                    Some(Box::new(original)),
                    cleanup,
                ));
            }
            Err(original)
        }
    }
}

pub async fn create_project(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    input: CreateProjectInput<'_>,
    client_ip: Option<&str>,
) -> Result<Result<ProjectRow, ProjectDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let result = create_project_tx(
        &mut tx,
        workspace_id,
        actor_user_id,
        session_id,
        input,
        client_ip,
    )
    .await?;
    if result.is_ok() {
        tx.commit().await?;
    } else {
        tx.rollback().await?;
    }
    Ok(result)
}

/// Existing project/workflow/root creation in the caller's tenant transaction.
/// The caller commits only Ok and rolls back domain failures.
pub(crate) async fn create_project_tx(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    input: CreateProjectInput<'_>,
    client_ip: Option<&str>,
) -> Result<Result<ProjectRow, ProjectDbError>, sqlx::Error> {
    let project_id = Uuid::now_v7();
    let root_document_id = Uuid::now_v7();
    let mut lock_users = vec![actor_user_id];
    if let Some(lead) = input.lead_user_id {
        if lead != actor_user_id {
            lock_users.push(lead);
        }
    }

    lock_membership_users(tx, &lock_users).await?;
    if !recheck_session(tx, actor_user_id, session_id).await? {
        return Ok(Err(ProjectDbError::Forbidden));
    }
    if !workspace_is_live(tx, workspace_id).await? {
        return Ok(Err(ProjectDbError::NotFound));
    }
    let actor_role = membership_role_for_update(tx, workspace_id, actor_user_id).await?;
    if !actor_role
        .map(|r| r.at_least(WorkspaceRole::Member))
        .unwrap_or(false)
    {
        return Ok(Err(ProjectDbError::NotFound));
    }
    lock_tree(tx, workspace_id).await?;

    let inserted = sqlx::query_as::<_, (Uuid,)>(
        r#"
        INSERT INTO fvoci.projects (
            id, workspace_id, key, name, description, icon, visibility, status,
            next_number, created_by
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, 'active', 1, $8)
        RETURNING id
        "#,
    )
    .bind(project_id)
    .bind(workspace_id)
    .bind(input.key)
    .bind(input.name.trim())
    .bind(optional_text_to_db(input.description))
    .bind(optional_text_to_db(input.icon))
    .bind(input.visibility)
    .bind(actor_user_id)
    .fetch_optional(&mut **tx)
    .await;

    if let Err(err) = inserted {
        if let Some(db_err) = err.as_database_error() {
            if db_err.constraint() == Some("projects_workspace_id_key_unique") {
                return Ok(Err(ProjectDbError::Conflict));
            }
        }
        return Err(err);
    }
    inserted?;

    sqlx::query(
        r#"
        INSERT INTO fvoci.project_members (id, workspace_id, project_id, user_id, role)
        VALUES ($1, $2, $3, $4, 'lead')
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(workspace_id)
    .bind(project_id)
    .bind(actor_user_id)
    .execute(&mut **tx)
    .await?;

    if let Some(lead_user_id) = input.lead_user_id {
        if lead_user_id != actor_user_id {
            let lead_role = membership_role(tx, workspace_id, lead_user_id).await?;
            let lead_role = match lead_role {
                Some(r) if r.at_least(WorkspaceRole::Member) => r,
                _ => {
                    return Ok(Err(ProjectDbError::NotFound));
                }
            };
            let _ = lead_role;
            sqlx::query(
                r#"
                INSERT INTO fvoci.project_members (id, workspace_id, project_id, user_id, role)
                VALUES ($1, $2, $3, $4, 'lead')
                ON CONFLICT (workspace_id, project_id, user_id) DO UPDATE SET role = 'lead', updated_at = now()
                "#,
            )
            .bind(Uuid::now_v7())
            .bind(workspace_id)
            .bind(project_id)
            .bind(lead_user_id)
            .execute(&mut **tx)
            .await?;
            sqlx::query(
                r#"
                UPDATE fvoci.project_members
                SET role = 'member', updated_at = now()
                WHERE workspace_id = $1 AND project_id = $2 AND user_id = $3
                "#,
            )
            .bind(workspace_id)
            .bind(project_id)
            .bind(actor_user_id)
            .execute(&mut **tx)
            .await?;
        }
    }

    let doc_number: (i32,) = sqlx::query_as(
        r#"
        UPDATE fvoci.projects
        SET next_number = next_number + 1, updated_at = now()
        WHERE workspace_id = $1 AND id = $2
        RETURNING next_number - 1
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .fetch_one(&mut **tx)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO fvoci.documents (
            id, workspace_id, title, path, parent_id, sort_key, project_id, number, status,
            schema_version, content_json, created_by
        ) VALUES ($1, $2, $3, $4, NULL, 'V', $5, $6, 'published', $7, $8, $9)
        "#,
    )
    .bind(root_document_id)
    .bind(workspace_id)
    .bind(input.name.trim())
    .bind(to_path_label(root_document_id))
    .bind(project_id)
    .bind(doc_number.0)
    .bind(DOCUMENT_SCHEMA_VERSION)
    .bind(empty_document_json())
    .bind(actor_user_id)
    .execute(&mut **tx)
    .await?;

    sqlx::query(
        r#"
        UPDATE fvoci.projects
        SET root_document_id = $3, updated_at = now()
        WHERE workspace_id = $1 AND id = $2
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .bind(root_document_id)
    .execute(&mut **tx)
    .await?;

    seed_workflow(tx, workspace_id, project_id).await?;

    record_project_event_and_audit(
        tx,
        ProjectChangeRecord {
            workspace_id,
            actor_user_id,
            verb: "project.created",
            target_type: "project",
            target_id: project_id,
            payload: json!({
                "projectId": project_id.to_string(),
                "key": input.key,
                "name": input.name.trim(),
                "visibility": input.visibility,
                "rootDocumentId": root_document_id.to_string(),
            }),
            client_ip,
        },
    )
    .await?;

    let row = sqlx::query_as::<
        _,
        (
            Uuid,
            String,
            String,
            Option<String>,
            Option<String>,
            String,
            Option<Uuid>,
            String,
            Uuid,
            DateTime<Utc>,
            DateTime<Utc>,
        ),
    >(
        r#"
        SELECT id, key, name, description, icon, visibility, root_document_id, status,
               created_by, created_at, updated_at
        FROM fvoci.projects
        WHERE workspace_id = $1 AND id = $2
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .fetch_one(&mut **tx)
    .await?;

    Ok(Ok(ProjectRow {
        id: row.0,
        workspace_id,
        key: row.1,
        name: row.2,
        description: row.3,
        icon: row.4,
        visibility: row.5,
        root_document_id: row.6,
        status: row.7,
        created_by: row.8,
        created_at: row.9,
        updated_at: row.10,
    }))
}

pub async fn clone_project(
    pool: &PgPool,
    workspace_id: Uuid,
    source_project_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    input: CloneProjectInput<'_>,
    client_ip: Option<&str>,
) -> Result<Result<ProjectRow, ProjectDbError>, sqlx::Error> {
    let dest_project_id = Uuid::now_v7();
    let root_document_id = Uuid::now_v7();
    let mut lock_users = vec![actor_user_id];
    if let Some(lead) = input.lead_user_id {
        if lead != actor_user_id {
            lock_users.push(lead);
        }
    }

    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    lock_membership_users(&mut tx, &lock_users).await?;
    if !recheck_session(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::Forbidden));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }
    let actor_role = membership_role_for_update(&mut tx, workspace_id, actor_user_id).await?;
    if !actor_role
        .map(|r| r.at_least(WorkspaceRole::Member))
        .unwrap_or(false)
    {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }

    lock_tree(&mut tx, workspace_id).await?;

    let source = lock_project(&mut tx, workspace_id, source_project_id).await?;
    let Some(source) = source else {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    };
    if !project_permission(&mut tx, workspace_id, actor_user_id, &source)
        .await?
        .at_least(ProjectPermission::Manage)
    {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }

    let visibility = input.visibility.unwrap_or(&source.visibility);
    let description = match input.description {
        Some(value) => optional_text_to_db(value),
        None => source.description.clone(),
    };
    let icon = match input.icon {
        Some(value) => optional_text_to_db(value),
        None => source.icon.clone(),
    };

    let inserted = sqlx::query_as::<_, (Uuid,)>(
        r#"
        INSERT INTO fvoci.projects (
            id, workspace_id, key, name, description, icon, visibility, status,
            next_number, created_by
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, 'active', 1, $8)
        RETURNING id
        "#,
    )
    .bind(dest_project_id)
    .bind(workspace_id)
    .bind(input.key)
    .bind(input.name.trim())
    .bind(&description)
    .bind(&icon)
    .bind(visibility)
    .bind(actor_user_id)
    .fetch_optional(&mut *tx)
    .await;

    if let Err(err) = inserted {
        if let Some(db_err) = err.as_database_error() {
            if db_err.constraint() == Some("projects_workspace_id_key_unique") {
                tx.rollback().await?;
                return Ok(Err(ProjectDbError::Conflict));
            }
        }
        return Err(err);
    }
    inserted?;

    sqlx::query(
        r#"
        INSERT INTO fvoci.project_members (id, workspace_id, project_id, user_id, role)
        VALUES ($1, $2, $3, $4, 'lead')
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(workspace_id)
    .bind(dest_project_id)
    .bind(actor_user_id)
    .execute(&mut *tx)
    .await?;

    if let Some(lead_user_id) = input.lead_user_id {
        if lead_user_id != actor_user_id {
            let lead_role = membership_role(&mut tx, workspace_id, lead_user_id).await?;
            if !lead_role
                .map(|r| r.at_least(WorkspaceRole::Member))
                .unwrap_or(false)
            {
                tx.rollback().await?;
                return Ok(Err(ProjectDbError::NotFound));
            }
            sqlx::query(
                r#"
                INSERT INTO fvoci.project_members (id, workspace_id, project_id, user_id, role)
                VALUES ($1, $2, $3, $4, 'lead')
                ON CONFLICT (workspace_id, project_id, user_id) DO UPDATE SET role = 'lead', updated_at = now()
                "#,
            )
            .bind(Uuid::now_v7())
            .bind(workspace_id)
            .bind(dest_project_id)
            .bind(lead_user_id)
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                r#"
                UPDATE fvoci.project_members
                SET role = 'member', updated_at = now()
                WHERE workspace_id = $1 AND project_id = $2 AND user_id = $3
                "#,
            )
            .bind(workspace_id)
            .bind(dest_project_id)
            .bind(actor_user_id)
            .execute(&mut *tx)
            .await?;
        }
    }

    let doc_number: (i32,) = sqlx::query_as(
        r#"
        UPDATE fvoci.projects
        SET next_number = next_number + 1, updated_at = now()
        WHERE workspace_id = $1 AND id = $2
        RETURNING next_number - 1
        "#,
    )
    .bind(workspace_id)
    .bind(dest_project_id)
    .fetch_one(&mut *tx)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO fvoci.documents (
            id, workspace_id, title, path, parent_id, sort_key, project_id, number, status,
            schema_version, content_json, created_by
        ) VALUES ($1, $2, $3, $4, NULL, 'V', $5, $6, 'published', $7, $8, $9)
        "#,
    )
    .bind(root_document_id)
    .bind(workspace_id)
    .bind(input.name.trim())
    .bind(to_path_label(root_document_id))
    .bind(dest_project_id)
    .bind(doc_number.0)
    .bind(DOCUMENT_SCHEMA_VERSION)
    .bind(empty_document_json())
    .bind(actor_user_id)
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        r#"
        UPDATE fvoci.projects
        SET root_document_id = $3, updated_at = now()
        WHERE workspace_id = $1 AND id = $2
        "#,
    )
    .bind(workspace_id)
    .bind(dest_project_id)
    .bind(root_document_id)
    .execute(&mut *tx)
    .await?;

    if !crate::db::project_clone::copy_project_configuration(
        &mut tx,
        workspace_id,
        source_project_id,
        dest_project_id,
        actor_user_id,
    )
    .await?
    {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::InvalidInput));
    }

    record_project_event_and_audit(
        &mut tx,
        ProjectChangeRecord {
            workspace_id,
            actor_user_id,
            verb: "project.created",
            target_type: "project",
            target_id: dest_project_id,
            payload: json!({
                "projectId": dest_project_id.to_string(),
                "key": input.key,
                "name": input.name.trim(),
                "visibility": visibility,
                "rootDocumentId": root_document_id.to_string(),
                "sourceProjectId": source_project_id.to_string(),
            }),
            client_ip,
        },
    )
    .await?;

    let row = sqlx::query_as::<
        _,
        (
            Uuid,
            String,
            String,
            Option<String>,
            Option<String>,
            String,
            Option<Uuid>,
            String,
            Uuid,
            DateTime<Utc>,
            DateTime<Utc>,
        ),
    >(
        r#"
        SELECT id, key, name, description, icon, visibility, root_document_id, status,
               created_by, created_at, updated_at
        FROM fvoci.projects
        WHERE workspace_id = $1 AND id = $2
        "#,
    )
    .bind(workspace_id)
    .bind(dest_project_id)
    .fetch_one(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(Ok(ProjectRow {
        id: row.0,
        workspace_id,
        key: row.1,
        name: row.2,
        description: row.3,
        icon: row.4,
        visibility: row.5,
        root_document_id: row.6,
        status: row.7,
        created_by: row.8,
        created_at: row.9,
        updated_at: row.10,
    }))
}

/// Active selected-project list. Authority, grants and grouped counts share
/// one read snapshot; the original PostgreSQL/RLS entrypoint is unchanged.
pub async fn list_projects_backend(
    backend: &Backend,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<Result<Vec<ProjectListItem>, ProjectDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return list_projects(pool, workspace_id, actor_user_id, session_id).await;
    }
    let mut tx = backend.begin_read().await?;
    let result = async {
        let mut op = tx.operation();
        op.set_tenant(workspace_id).await?;
        if !op.session_is_live(actor_user_id, session_id).await? {
            return Ok(Err(ProjectDbError::Forbidden));
        }
        if !op.workspace_is_live(workspace_id).await? {
            return Ok(Err(ProjectDbError::NotFound));
        }
        let Some(role) = op.membership_role(workspace_id, actor_user_id, false).await? else {
            return Ok(Err(ProjectDbError::NotFound));
        };
        let OperationTx::SqliteFamily(family) = op else { unreachable!() };
        family.require_tenant(workspace_id)?;
        // Fetch current grants once, not one permission query per project.
        let grants = family.query(
            "SELECT project_id,role FROM project_members
             WHERE workspace_id=?1 AND user_id=?2
             UNION ALL
             SELECT pm.project_id,pm.role FROM project_members pm
             JOIN group_members gm ON gm.workspace_id=pm.workspace_id AND gm.group_id=pm.group_id
             WHERE pm.workspace_id=?1 AND gm.user_id=?2 AND pm.group_id IS NOT NULL",
            &[Cell::uuid(workspace_id),Cell::uuid(actor_user_id)],
        ).await?;
        let mut roles = std::collections::HashMap::<Uuid,ProjectMemberRole>::new();
        for grant in grants {
            if let Some(granted) = ProjectMemberRole::parse(&grant.cell(1)?.string()?) {
                roles.entry(grant.cell(0)?.id()?)
                    .and_modify(|current| {
                        if granted.permission() > current.permission() { *current = granted; }
                    })
                    .or_insert(granted);
            }
        }
        let rows = family.query(
            "SELECT p.id,p.key,p.name,p.description,p.icon,p.visibility,p.root_document_id,
                    p.status,p.created_by,p.created_at,p.updated_at,
                    COALESCE(dc.n,0),COALESCE(tc.n,0),COALESCE(tc.open_n,0)
             FROM projects p
             LEFT JOIN (
                 SELECT workspace_id,project_id,count(*) AS n FROM documents
                 WHERE workspace_id=?1 AND project_id IS NOT NULL AND deleted_at IS NULL
                 GROUP BY workspace_id,project_id
             ) dc ON dc.workspace_id=p.workspace_id AND dc.project_id=p.id
             LEFT JOIN (
                 SELECT t.workspace_id,t.project_id,
                     count(*) FILTER (WHERE t.deleted_at IS NULL AND t.archived_at IS NULL) AS n,
                     count(*) FILTER (WHERE t.deleted_at IS NULL AND t.archived_at IS NULL
                                      AND s.category NOT IN ('done','canceled')) AS open_n
                 FROM tasks t JOIN statuses s ON s.workspace_id=t.workspace_id
                     AND s.project_id=t.project_id AND s.id=t.status_id
                 WHERE t.workspace_id=?1 GROUP BY t.workspace_id,t.project_id
             ) tc ON tc.workspace_id=p.workspace_id AND tc.project_id=p.id
             WHERE p.workspace_id=?1 AND p.deleted_at IS NULL
               AND ((p.visibility='workspace' AND ?2=0)
                    OR EXISTS(SELECT 1 FROM project_members pm WHERE pm.workspace_id=p.workspace_id
                              AND pm.project_id=p.id AND pm.user_id=?3)
                    OR EXISTS(SELECT 1 FROM project_members pm JOIN group_members gm
                              ON gm.workspace_id=pm.workspace_id AND gm.group_id=pm.group_id
                              WHERE pm.workspace_id=p.workspace_id AND pm.project_id=p.id AND gm.user_id=?3))
             ORDER BY p.key COLLATE BINARY",
            &[Cell::uuid(workspace_id),Cell::Integer(i64::from(u8::from(role==WorkspaceRole::Guest))),Cell::uuid(actor_user_id)],
        ).await?;
        let mut items = Vec::with_capacity(rows.len());
        for row in rows {
            let project = project_created_family_row(&row, workspace_id)?;
            let permission = effective_permission(role, &project.visibility, roles.get(&project.id).copied());
            items.push(ProjectListItem {
                project,
                document_count: row.cell(11)?.integer()?,
                task_count: row.cell(12)?.integer()?,
                open_task_count: row.cell(13)?.integer()?,
                can_edit: permission.at_least(ProjectPermission::Edit),
                can_manage: permission.at_least(ProjectPermission::Manage),
            });
        }
        Ok(Ok(items))
    }.await;
    let cleanup = tx.rollback().await;
    project_read_after_rollback(result, cleanup)
}

#[derive(Debug, thiserror::Error)]
#[error("project read refused: {0:?}")]
struct ProjectReadRefusal(ProjectDbError);

// The two actual selected readers return different typed rows. Withhold either
// observation if original-stream cleanup is unknown, retaining its first cause.
fn project_read_after_rollback<T>(
    result: Result<Result<T, ProjectDbError>, sqlx::Error>,
    cleanup: Result<(), sqlx::Error>,
) -> Result<Result<T, ProjectDbError>, sqlx::Error> {
    match cleanup {
        Ok(()) => result,
        Err(cleanup) => {
            let original: Option<Box<dyn std::error::Error + Send + Sync>> = match result {
                Err(driver) => Some(Box::new(driver)),
                Ok(Err(refusal)) => Some(Box::new(ProjectReadRefusal(refusal))),
                Ok(Ok(_)) => None,
            };
            Err(crate::db::backend::rollback_cleanup_unknown(
                original, cleanup,
            ))
        }
    }
}

pub async fn list_projects(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<Result<Vec<ProjectListItem>, ProjectDbError>, sqlx::Error> {
    let mut tx = begin_read(pool).await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !session_is_live(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::Forbidden));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }
    let workspace_role = membership_role(&mut tx, workspace_id, actor_user_id).await?;
    let workspace_role = match workspace_role {
        Some(r) if r.at_least(WorkspaceRole::Guest) => r,
        _ => {
            tx.rollback().await?;
            return Ok(Err(ProjectDbError::NotFound));
        }
    };

    let guest = workspace_role == WorkspaceRole::Guest;
    let visible = visible_project_sql("p", 2, 3);
    let list_sql = format!(
        r#"
        SELECT p.id, p.key, p.name, p.description, p.icon, p.visibility, p.root_document_id,
               p.status, p.created_by, p.created_at, p.updated_at,
               COALESCE(dc.document_count, 0)::bigint AS document_count
        FROM fvoci.projects p
        LEFT JOIN (
            SELECT workspace_id, project_id, count(*)::bigint AS document_count
            FROM fvoci.documents
            WHERE workspace_id = $1 AND project_id IS NOT NULL AND deleted_at IS NULL
            GROUP BY workspace_id, project_id
        ) dc ON dc.workspace_id = p.workspace_id AND dc.project_id = p.id
        WHERE p.workspace_id = $1
          AND p.deleted_at IS NULL
          AND {visible}
        ORDER BY p.key COLLATE "C"
        "#
    );
    let rows = sqlx::query_as::<
        _,
        (
            Uuid,
            String,
            String,
            Option<String>,
            Option<String>,
            String,
            Option<Uuid>,
            String,
            Uuid,
            DateTime<Utc>,
            DateTime<Utc>,
            i64,
        ),
    >(&list_sql)
    .bind(workspace_id)
    .bind(guest)
    .bind(actor_user_id)
    .fetch_all(&mut *tx)
    .await?;

    let mut items = Vec::new();
    for row in rows {
        let project_id = row.0;
        let counts: (i64, i64) = sqlx::query_as(
            r#"
            SELECT
                count(*) FILTER (WHERE t.deleted_at IS NULL AND t.archived_at IS NULL),
                count(*) FILTER (
                    WHERE t.deleted_at IS NULL
                      AND t.archived_at IS NULL
                      AND s.category NOT IN ('done', 'canceled')
                )
            FROM fvoci.tasks t
            INNER JOIN fvoci.statuses s
                ON s.workspace_id = t.workspace_id
               AND s.project_id = t.project_id
               AND s.id = t.status_id
            WHERE t.workspace_id = $1 AND t.project_id = $2
            "#,
        )
        .bind(workspace_id)
        .bind(project_id)
        .fetch_one(&mut *tx)
        .await?;

        let project = LiveProject {
            id: project_id,
            key: row.1.clone(),
            name: row.2.clone(),
            description: row.3.clone(),
            icon: row.4.clone(),
            visibility: row.5.clone(),
            root_document_id: row.6,
            status: row.7.clone(),
            created_by: row.8,
            created_at: row.9,
            updated_at: row.10,
        };
        let permission = project_permission(&mut tx, workspace_id, actor_user_id, &project).await?;
        let can_edit = permission.at_least(ProjectPermission::Edit);
        let can_manage = permission.at_least(ProjectPermission::Manage);

        items.push(ProjectListItem {
            project: ProjectRow {
                id: project_id,
                workspace_id,
                key: row.1,
                name: row.2,
                description: row.3,
                icon: row.4,
                visibility: row.5,
                root_document_id: row.6,
                status: row.7,
                created_by: row.8,
                created_at: row.9,
                updated_at: row.10,
            },
            document_count: row.11,
            task_count: counts.0,
            open_task_count: counts.1,
            can_edit,
            can_manage,
        });
    }
    tx.commit().await?;
    Ok(Ok(items))
}

pub async fn get_project(
    pool: &PgPool,
    workspace_id: Uuid,
    project_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<Result<ProjectRow, ProjectDbError>, sqlx::Error> {
    let mut tx = begin_read(pool).await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !session_is_live(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::Forbidden));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }
    let project = load_live_project(&mut tx, workspace_id, project_id).await?;
    let Some(project) = project else {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    };
    if project_permission(&mut tx, workspace_id, actor_user_id, &project)
        .await?
        .at_least(ProjectPermission::View)
    {
        tx.commit().await?;
        Ok(Ok(ProjectRow {
            id: project.id,
            workspace_id,
            key: project.key,
            name: project.name,
            description: project.description,
            icon: project.icon,
            visibility: project.visibility,
            root_document_id: project.root_document_id,
            status: project.status,
            created_by: project.created_by,
            created_at: project.created_at,
            updated_at: project.updated_at,
        }))
    } else {
        tx.rollback().await?;
        Ok(Err(ProjectDbError::NotFound))
    }
}

pub async fn update_project(
    pool: &PgPool,
    workspace_id: Uuid,
    project_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    input: UpdateProjectInput<'_>,
    client_ip: Option<&str>,
) -> Result<Result<ProjectRow, ProjectDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let mut lock_users = vec![actor_user_id];
    if let Some(lead) = input.lead_user_id {
        lock_users.push(lead);
    }
    lock_membership_users(&mut tx, &lock_users).await?;
    if !recheck_session(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::Forbidden));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }
    let locked = lock_project(&mut tx, workspace_id, project_id).await?;
    let Some(locked) = locked else {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    };
    if locked.status == "archived" {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::Archived));
    }
    let pre_permission = project_permission(&mut tx, workspace_id, actor_user_id, &locked).await?;
    if !pre_permission.at_least(ProjectPermission::Manage) {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }

    let mut name = locked.name.clone();
    let mut visibility = locked.visibility.clone();
    let mut description = locked.description.clone();
    let mut icon = locked.icon.clone();

    if let Some(next_name) = input.name {
        name = next_name.trim().to_string();
    }
    if let Some(next_visibility) = input.visibility {
        visibility = next_visibility.to_string();
    }
    if let Some(next_description) = input.description {
        description = optional_text_to_db(next_description);
    }
    if let Some(next_icon) = input.icon {
        icon = optional_text_to_db(next_icon);
    }

    if visibility == "private"
        && locked.visibility == "workspace"
        && count_project_leads(&mut tx, workspace_id, project_id).await? == 0
    {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::LastLead));
    }

    if let Some(lead_user_id) = input.lead_user_id {
        let lead_ws_role = membership_role(&mut tx, workspace_id, lead_user_id).await?;
        if !lead_ws_role
            .map(|r| r.at_least(WorkspaceRole::Member))
            .unwrap_or(false)
        {
            tx.rollback().await?;
            return Ok(Err(ProjectDbError::GuestLead));
        }
        let member =
            direct_project_member_role(&mut tx, workspace_id, project_id, lead_user_id).await?;
        if member.is_none() {
            tx.rollback().await?;
            return Ok(Err(ProjectDbError::LeadNotMember));
        }
        let promoted = sqlx::query(
            r#"
            UPDATE fvoci.project_members
            SET role = 'lead', updated_at = now()
            WHERE workspace_id = $1 AND project_id = $2 AND user_id = $3
            "#,
        )
        .bind(workspace_id)
        .bind(project_id)
        .bind(lead_user_id)
        .execute(&mut *tx)
        .await?;
        if promoted.rows_affected() == 0 {
            tx.rollback().await?;
            return Ok(Err(ProjectDbError::LeadNotMember));
        }
        sqlx::query(
            r#"
            UPDATE fvoci.project_members
            SET role = 'member', updated_at = now()
            WHERE workspace_id = $1 AND project_id = $2 AND user_id <> $3 AND role = 'lead'
            "#,
        )
        .bind(workspace_id)
        .bind(project_id)
        .bind(lead_user_id)
        .execute(&mut *tx)
        .await?;
    }

    let updated = sqlx::query_as::<
        _,
        (
            Uuid,
            String,
            String,
            Option<String>,
            Option<String>,
            String,
            Option<Uuid>,
            String,
            Uuid,
            DateTime<Utc>,
            DateTime<Utc>,
        ),
    >(
        r#"
        UPDATE fvoci.projects
        SET name = $3, visibility = $4, description = $5, icon = $6, updated_at = now()
        WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL
        RETURNING id, key, name, description, icon, visibility, root_document_id, status,
                  created_by, created_at, updated_at
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .bind(&name)
    .bind(&visibility)
    .bind(&description)
    .bind(&icon)
    .fetch_one(&mut *tx)
    .await?;

    if visibility == "private" && count_project_leads(&mut tx, workspace_id, project_id).await? == 0
    {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::LastLead));
    }

    record_project_event_and_audit(
        &mut tx,
        ProjectChangeRecord {
            workspace_id,
            actor_user_id,
            verb: "project.updated",
            target_type: "project",
            target_id: project_id,
            payload: json!({
                "projectId": project_id.to_string(),
                "name": updated.2,
                "visibility": updated.5,
            }),
            client_ip,
        },
    )
    .await?;

    match tx.commit().await {
        Ok(()) => Ok(Ok(ProjectRow {
            id: updated.0,
            workspace_id,
            key: updated.1,
            name: updated.2,
            description: updated.3,
            icon: updated.4,
            visibility: updated.5,
            root_document_id: updated.6,
            status: updated.7,
            created_by: updated.8,
            created_at: updated.9,
            updated_at: updated.10,
        })),
        Err(err) if is_private_lead_violation(&err) => Ok(Err(ProjectDbError::LastLead)),
        Err(err) => Err(err),
    }
}

pub async fn list_project_members(
    pool: &PgPool,
    workspace_id: Uuid,
    project_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<Result<Vec<ProjectMemberRow>, ProjectDbError>, sqlx::Error> {
    let mut tx = begin_read(pool).await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !session_is_live(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::Forbidden));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }
    let project = load_live_project(&mut tx, workspace_id, project_id).await?;
    let Some(project) = project else {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    };
    if !project_permission(&mut tx, workspace_id, actor_user_id, &project)
        .await?
        .at_least(ProjectPermission::View)
    {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }
    let rows = sqlx::query_as::<_, (Uuid, String, String, Option<String>, String)>(
        r#"
        SELECT u.id, u.email, u.given_name, u.family_name, pm.role
        FROM fvoci.project_members pm
        INNER JOIN fvoci.users u ON u.id = pm.user_id
        WHERE pm.workspace_id = $1 AND pm.project_id = $2
          AND pm.user_id IS NOT NULL
          AND u.deleted_at IS NULL
        ORDER BY u.email COLLATE "C"
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Ok(rows
        .into_iter()
        .filter_map(|(user_id, email, given_name, family_name, role)| {
            ProjectMemberRole::parse(&role).map(|role| ProjectMemberRow {
                user_id,
                email,
                given_name,
                family_name,
                role,
            })
        })
        .collect()))
}

pub async fn add_project_member(
    pool: &PgPool,
    workspace_id: Uuid,
    project_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    target_user_id: Uuid,
    role: ProjectMemberRole,
    client_ip: Option<&str>,
) -> Result<Result<(), ProjectDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    lock_membership_users(&mut tx, &[actor_user_id, target_user_id]).await?;
    if !recheck_session(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::Forbidden));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }
    let locked = lock_project(&mut tx, workspace_id, project_id).await?;
    let Some(locked) = locked else {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    };
    if !project_permission(&mut tx, workspace_id, actor_user_id, &locked)
        .await?
        .at_least(ProjectPermission::Manage)
    {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }
    if role == ProjectMemberRole::Lead {
        let target_ws_role = membership_role(&mut tx, workspace_id, target_user_id).await?;
        if !target_ws_role
            .map(|r| r.at_least(WorkspaceRole::Member))
            .unwrap_or(false)
        {
            tx.rollback().await?;
            return Ok(Err(ProjectDbError::GuestLead));
        }
    }
    let inserted = sqlx::query(
        r#"
        INSERT INTO fvoci.project_members (id, workspace_id, project_id, user_id, role)
        VALUES ($1, $2, $3, $4, $5)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(workspace_id)
    .bind(project_id)
    .bind(target_user_id)
    .bind(role.as_str())
    .execute(&mut *tx)
    .await;
    if let Err(err) = inserted {
        if err
            .as_database_error()
            .and_then(|db| db.constraint())
            .is_some()
        {
            tx.rollback().await?;
            return Ok(Err(ProjectDbError::Conflict));
        }
        return Err(err);
    }

    record_project_event_and_audit(
        &mut tx,
        ProjectChangeRecord {
            workspace_id,
            actor_user_id,
            verb: "project_member.added",
            target_type: "project_member",
            target_id: target_user_id,
            payload: json!({
                "projectId": project_id.to_string(),
                "userId": target_user_id.to_string(),
                "role": role.as_str(),
            }),
            client_ip,
        },
    )
    .await?;

    tx.commit().await?;
    Ok(Ok(()))
}

pub async fn update_project_member_role(
    pool: &PgPool,
    workspace_id: Uuid,
    project_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    target_user_id: Uuid,
    role: ProjectMemberRole,
    client_ip: Option<&str>,
) -> Result<Result<(), ProjectDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    lock_membership_users(&mut tx, &[actor_user_id, target_user_id]).await?;
    if !recheck_session(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::Forbidden));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }
    let locked = lock_project(&mut tx, workspace_id, project_id).await?;
    let Some(locked) = locked else {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    };
    if !project_permission(&mut tx, workspace_id, actor_user_id, &locked)
        .await?
        .at_least(ProjectPermission::Manage)
    {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }
    let current =
        direct_project_member_role(&mut tx, workspace_id, project_id, target_user_id).await?;
    let Some(current) = current else {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    };
    if role == ProjectMemberRole::Lead {
        let target_ws_role = membership_role(&mut tx, workspace_id, target_user_id).await?;
        if !target_ws_role
            .map(|r| r.at_least(WorkspaceRole::Member))
            .unwrap_or(false)
        {
            tx.rollback().await?;
            return Ok(Err(ProjectDbError::GuestLead));
        }
    }
    if current == ProjectMemberRole::Lead
        && role != ProjectMemberRole::Lead
        && locked.visibility == "private"
        && count_project_leads_excluding(&mut tx, workspace_id, project_id, target_user_id).await?
            == 0
    {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::LastLead));
    }
    let updated = sqlx::query(
        r#"
        UPDATE fvoci.project_members
        SET role = $4, updated_at = now()
        WHERE workspace_id = $1 AND project_id = $2 AND user_id = $3
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .bind(target_user_id)
    .bind(role.as_str())
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() == 0 {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }

    record_project_event_and_audit(
        &mut tx,
        ProjectChangeRecord {
            workspace_id,
            actor_user_id,
            verb: "project_member.role_changed",
            target_type: "project_member",
            target_id: target_user_id,
            payload: json!({
                "projectId": project_id.to_string(),
                "userId": target_user_id.to_string(),
                "fromRole": current.as_str(),
                "role": role.as_str(),
            }),
            client_ip,
        },
    )
    .await?;

    let commit = tx.commit().await;
    if let Err(err) = commit {
        if is_private_lead_violation(&err) {
            return Ok(Err(ProjectDbError::LastLead));
        }
        return Err(err);
    }
    Ok(Ok(()))
}

pub async fn remove_project_member(
    pool: &PgPool,
    workspace_id: Uuid,
    project_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    target_user_id: Uuid,
    client_ip: Option<&str>,
) -> Result<Result<(), ProjectDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    lock_membership_users(&mut tx, &[actor_user_id, target_user_id]).await?;
    if !recheck_session(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::Forbidden));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }
    let locked = lock_project(&mut tx, workspace_id, project_id).await?;
    let Some(locked) = locked else {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    };
    if !project_permission(&mut tx, workspace_id, actor_user_id, &locked)
        .await?
        .at_least(ProjectPermission::Manage)
    {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }
    let current =
        direct_project_member_role(&mut tx, workspace_id, project_id, target_user_id).await?;
    let Some(current) = current else {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    };
    if current == ProjectMemberRole::Lead
        && locked.visibility == "private"
        && count_project_leads_excluding(&mut tx, workspace_id, project_id, target_user_id).await?
            == 0
    {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::LastLead));
    }
    let deleted = sqlx::query(
        r#"
        DELETE FROM fvoci.project_members
        WHERE workspace_id = $1 AND project_id = $2 AND user_id = $3
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .bind(target_user_id)
    .execute(&mut *tx)
    .await?;
    if deleted.rows_affected() == 0 {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }

    record_project_event_and_audit(
        &mut tx,
        ProjectChangeRecord {
            workspace_id,
            actor_user_id,
            verb: "project_member.removed",
            target_type: "project_member",
            target_id: target_user_id,
            payload: json!({
                "projectId": project_id.to_string(),
                "userId": target_user_id.to_string(),
                "role": current.as_str(),
            }),
            client_ip,
        },
    )
    .await?;

    let commit = tx.commit().await;
    if let Err(err) = commit {
        if is_private_lead_violation(&err) {
            return Ok(Err(ProjectDbError::LastLead));
        }
        return Err(err);
    }
    Ok(Ok(()))
}

pub async fn get_project_workflow_backend(
    backend: &Backend,
    workspace_id: Uuid,
    project_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<Result<WorkflowRow, ProjectDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return get_project_workflow(pool, workspace_id, project_id, actor_user_id, session_id)
            .await;
    }
    let mut tx = backend.begin_read().await?;
    let result = async {
        let mut op = tx.operation();
        op.set_tenant(workspace_id).await?;
        if !op.session_is_live(actor_user_id, session_id).await? {
            return Ok(Err(ProjectDbError::Forbidden));
        }
        if !op.workspace_is_live(workspace_id).await?
            || op
                .membership_role(workspace_id, actor_user_id, false)
                .await?
                .is_none()
        {
            return Ok(Err(ProjectDbError::NotFound));
        }
        if !op
            .project_permission_by_id(workspace_id, actor_user_id, project_id)
            .await?
            .is_some_and(|permission| permission.at_least(ProjectPermission::View))
        {
            return Ok(Err(ProjectDbError::NotFound));
        }
        let OperationTx::SqliteFamily(family) = op else {
            unreachable!()
        };
        family.require_tenant(workspace_id)?;
        let rows = family
            .query(
                "SELECT id FROM workflows WHERE workspace_id=?1 AND project_id=?2 LIMIT 1",
                &[Cell::uuid(workspace_id), Cell::uuid(project_id)],
            )
            .await?;
        let Some(row) = rows.first() else {
            return Ok(Err(ProjectDbError::NotFound));
        };
        let id = row.cell(0)?.id()?;
        let rows = family
            .query(
                "SELECT id,name,category,sort_key,wip_limit FROM statuses
            WHERE workspace_id=?1 AND project_id=?2 ORDER BY sort_key COLLATE BINARY",
                &[Cell::uuid(workspace_id), Cell::uuid(project_id)],
            )
            .await?;
        let statuses = rows
            .iter()
            .map(|row| {
                Ok(WorkflowStatusRow {
                    id: row.cell(0)?.id()?,
                    name: row.cell(1)?.string()?,
                    category: row.cell(2)?.string()?,
                    sort_key: row.cell(3)?.string()?,
                    wip_limit: row.cell(4)?.optional(Cell::int32)?,
                })
            })
            .collect::<Result<Vec<_>, sqlx::Error>>()?;
        Ok(Ok(WorkflowRow {
            id,
            project_id,
            statuses,
        }))
    }
    .await;
    let cleanup = tx.rollback().await;
    project_read_after_rollback(result, cleanup)
}

pub async fn get_project_workflow(
    pool: &PgPool,
    workspace_id: Uuid,
    project_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<Result<WorkflowRow, ProjectDbError>, sqlx::Error> {
    let mut tx = begin_read(pool).await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !session_is_live(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::Forbidden));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }
    let project = load_live_project(&mut tx, workspace_id, project_id).await?;
    let Some(project) = project else {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    };
    if !project_permission(&mut tx, workspace_id, actor_user_id, &project)
        .await?
        .at_least(ProjectPermission::View)
    {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }
    let workflow: Option<(Uuid,)> = sqlx::query_as(
        r#"
        SELECT id FROM fvoci.workflows
        WHERE workspace_id = $1 AND project_id = $2
        LIMIT 1
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((workflow_id,)) = workflow else {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    };
    let statuses = sqlx::query_as::<_, (Uuid, String, String, String, Option<i32>)>(
        r#"
        SELECT id, name, category, sort_key, wip_limit
        FROM fvoci.statuses
        WHERE workspace_id = $1 AND project_id = $2
        ORDER BY sort_key COLLATE "C"
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Ok(WorkflowRow {
        id: workflow_id,
        project_id,
        statuses: statuses
            .into_iter()
            .map(
                |(id, name, category, sort_key, wip_limit)| WorkflowStatusRow {
                    id,
                    name,
                    category,
                    sort_key,
                    wip_limit,
                },
            )
            .collect(),
    }))
}

fn row_from_locked(workspace_id: Uuid, locked: LiveProject) -> ProjectRow {
    ProjectRow {
        id: locked.id,
        workspace_id,
        key: locked.key,
        name: locked.name,
        description: locked.description,
        icon: locked.icon,
        visibility: locked.visibility,
        root_document_id: locked.root_document_id,
        status: locked.status,
        created_by: locked.created_by,
        created_at: locked.created_at,
        updated_at: locked.updated_at,
    }
}

async fn begin_project_manage(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    project_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    tree_lock: bool,
) -> Result<Result<LiveProject, ProjectDbError>, sqlx::Error> {
    set_tenant(tx, workspace_id).await?;
    lock_membership_users(tx, &[actor_user_id]).await?;
    if !recheck_session(tx, actor_user_id, session_id).await? {
        return Ok(Err(ProjectDbError::Forbidden));
    }
    if !workspace_is_live(tx, workspace_id).await? {
        return Ok(Err(ProjectDbError::NotFound));
    }
    if tree_lock {
        lock_tree(tx, workspace_id).await?;
    }
    let Some(locked) = lock_project(tx, workspace_id, project_id).await? else {
        return Ok(Err(ProjectDbError::NotFound));
    };
    let permission = project_permission(tx, workspace_id, actor_user_id, &locked).await?;
    if !permission.at_least(ProjectPermission::Manage) {
        return Ok(Err(ProjectDbError::NotFound));
    }
    Ok(Ok(locked))
}

/// Source `purgeProject` (DELETE project): a soft delete. Every live document in
/// the project's tree is trashed with the project's own `deleted_at` stamp so
/// `restore_project` can bring back exactly those rows.
pub async fn trash_project(
    pool: &PgPool,
    workspace_id: Uuid,
    project_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    client_ip: Option<&str>,
) -> Result<Result<(), ProjectDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let locked = match begin_project_manage(
        &mut tx,
        workspace_id,
        project_id,
        actor_user_id,
        session_id,
        true,
    )
    .await?
    {
        Ok(locked) => locked,
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    };
    let (stamp,): (DateTime<Utc>,) = sqlx::query_as(
        r#"
        UPDATE fvoci.projects
        SET deleted_at = now(), updated_at = now()
        WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL
        RETURNING deleted_at
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .fetch_one(&mut *tx)
    .await?;
    if let Some(root_id) = locked.root_document_id {
        let subtree = crate::db::documents::subtree_ids(&mut tx, workspace_id, root_id).await?;
        crate::db::documents::lock_document_rows(&mut tx, workspace_id, &subtree).await?;
        let trashed: Vec<(Uuid,)> = sqlx::query_as(
            r#"
            UPDATE fvoci.documents
            SET deleted_at = $3, updated_at = now()
            WHERE workspace_id = $1 AND id = ANY($2) AND deleted_at IS NULL
            RETURNING id
            "#,
        )
        .bind(workspace_id)
        .bind(&subtree)
        .bind(stamp)
        .fetch_all(&mut *tx)
        .await?;
        for (id,) in trashed {
            record_project_event_and_audit(
                &mut tx,
                ProjectChangeRecord {
                    workspace_id,
                    actor_user_id,
                    verb: "document.trashed",
                    target_type: "document",
                    target_id: id,
                    payload: json!({
                        "documentId": id.to_string(),
                        "projectId": project_id.to_string(),
                    }),
                    client_ip,
                },
            )
            .await?;
        }
    }
    record_project_event_and_audit(
        &mut tx,
        ProjectChangeRecord {
            workspace_id,
            actor_user_id,
            verb: "project.deleted",
            target_type: "project",
            target_id: project_id,
            payload: json!({ "projectId": project_id.to_string() }),
            client_ip,
        },
    )
    .await?;
    tx.commit().await?;
    Ok(Ok(()))
}

/// Source `restoreProject`: workspace admins only. Restores documents trashed
/// together with the project (same stamp); documents trashed on their own stay
/// in the trash. A project past the trash retention is gone (its documents may
/// already be purged).
pub async fn restore_project(
    pool: &PgPool,
    workspace_id: Uuid,
    project_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    client_ip: Option<&str>,
) -> Result<Result<ProjectRow, ProjectDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    lock_membership_users(&mut tx, &[actor_user_id]).await?;
    if !recheck_session(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::Forbidden));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }
    let role = membership_role_for_update(&mut tx, workspace_id, actor_user_id).await?;
    if !role.is_some_and(|role| role.at_least(WorkspaceRole::Admin)) {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }
    lock_tree(&mut tx, workspace_id).await?;
    let row: Option<(Option<Uuid>, DateTime<Utc>)> = sqlx::query_as(
        r#"
        SELECT root_document_id, deleted_at
        FROM fvoci.projects
        WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NOT NULL
        FOR NO KEY UPDATE
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((root_document_id, stamp)) = row else {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    };
    if crate::db::documents::trash_expired(&mut tx, stamp).await? {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }
    sqlx::query(
        "UPDATE fvoci.projects SET deleted_at = NULL, updated_at = now() WHERE workspace_id = $1 AND id = $2",
    )
    .bind(workspace_id)
    .bind(project_id)
    .execute(&mut *tx)
    .await?;
    if let Some(root_id) = root_document_id {
        let subtree = crate::db::documents::subtree_ids(&mut tx, workspace_id, root_id).await?;
        crate::db::documents::lock_document_rows(&mut tx, workspace_id, &subtree).await?;
        let restored: Vec<(Uuid,)> = sqlx::query_as(
            r#"
            UPDATE fvoci.documents
            SET deleted_at = NULL, updated_at = now()
            WHERE workspace_id = $1 AND id = ANY($2) AND deleted_at = $3
            RETURNING id
            "#,
        )
        .bind(workspace_id)
        .bind(&subtree)
        .bind(stamp)
        .fetch_all(&mut *tx)
        .await?;
        for (id,) in restored {
            record_project_event_and_audit(
                &mut tx,
                ProjectChangeRecord {
                    workspace_id,
                    actor_user_id,
                    verb: "document.restored",
                    target_type: "document",
                    target_id: id,
                    payload: json!({
                        "documentId": id.to_string(),
                        "projectId": project_id.to_string(),
                    }),
                    client_ip,
                },
            )
            .await?;
        }
    }
    record_project_event_and_audit(
        &mut tx,
        ProjectChangeRecord {
            workspace_id,
            actor_user_id,
            verb: "project.restored",
            target_type: "project",
            target_id: project_id,
            payload: json!({ "projectId": project_id.to_string() }),
            client_ip,
        },
    )
    .await?;
    let Some(locked) = lock_project(&mut tx, workspace_id, project_id).await? else {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    };
    tx.commit().await?;
    Ok(Ok(row_from_locked(workspace_id, locked)))
}

/// Source `archiveProject` / `unarchiveProject`: project status only. Archived
/// projects are read-only everywhere writes check `status = 'archived'`.
pub async fn set_project_archived(
    pool: &PgPool,
    workspace_id: Uuid,
    project_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    archived: bool,
    client_ip: Option<&str>,
) -> Result<Result<(), ProjectDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    if let Err(err) = begin_project_manage(
        &mut tx,
        workspace_id,
        project_id,
        actor_user_id,
        session_id,
        false,
    )
    .await?
    {
        tx.rollback().await?;
        return Ok(Err(err));
    }
    let status = if archived { "archived" } else { "active" };
    sqlx::query(
        "UPDATE fvoci.projects SET status = $3, updated_at = now() WHERE workspace_id = $1 AND id = $2",
    )
    .bind(workspace_id)
    .bind(project_id)
    .bind(status)
    .execute(&mut *tx)
    .await?;
    record_project_event_and_audit(
        &mut tx,
        ProjectChangeRecord {
            workspace_id,
            actor_user_id,
            verb: if archived {
                "project.archived"
            } else {
                "project.unarchived"
            },
            target_type: "project",
            target_id: project_id,
            payload: json!({ "projectId": project_id.to_string() }),
            client_ip,
        },
    )
    .await?;
    tx.commit().await?;
    Ok(Ok(()))
}

/// Source `listDeletedProjects` (`GET projects?deleted=true`): workspace admins
/// only; rows past the trash retention are not restorable and are left out.
pub async fn list_deleted_projects(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<Result<Vec<ProjectRow>, ProjectDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !session_is_live(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::Forbidden));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }
    let role = membership_role(&mut tx, workspace_id, actor_user_id).await?;
    if !role.is_some_and(|role| role.at_least(WorkspaceRole::Admin)) {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }
    type Row = (
        Uuid,
        String,
        String,
        Option<String>,
        Option<String>,
        String,
        Option<Uuid>,
        String,
        Uuid,
        DateTime<Utc>,
        DateTime<Utc>,
    );
    let rows: Vec<Row> = sqlx::query_as(
        r#"
        SELECT id, key, name, description, icon, visibility, root_document_id, status,
               created_by, created_at, updated_at
        FROM fvoci.projects
        WHERE workspace_id = $1 AND deleted_at IS NOT NULL
          AND deleted_at > now() - make_interval(days => $2)
        ORDER BY deleted_at DESC, id DESC
        "#,
    )
    .bind(workspace_id)
    .bind(crate::db::documents::TRASH_RETENTION_DAYS)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Ok(rows
        .into_iter()
        .map(
            |(
                id,
                key,
                name,
                description,
                icon,
                visibility,
                root_document_id,
                status,
                created_by,
                created_at,
                updated_at,
            )| ProjectRow {
                id,
                workspace_id,
                key,
                name,
                description,
                icon,
                visibility,
                root_document_id,
                status,
                created_by,
                created_at,
                updated_at,
            },
        )
        .collect()))
}

#[cfg(test)]
mod selected_project_create_tests {
    use super::*;
    use crate::db::attachment_preview::tests::Fixture;

    /// Exact columns read back for the created root document (unchanged query).
    type RootDocumentRow = (
        String,
        String,
        Option<Vec<u8>>,
        String,
        Vec<u8>,
        i64,
        String,
        i64,
        String,
        Vec<u8>,
    );

    async fn credential(f: &Fixture) -> Uuid {
        let id = Uuid::now_v7();
        let mut tx = f.backend.begin_write().await.unwrap();
        tx.operation()
            .create_session(
                id,
                f.user,
                "project-create-test",
                DateTime::from_timestamp_micros(Utc::now().timestamp_micros() + 86_400_000_000)
                    .unwrap(),
            )
            .await
            .unwrap();
        tx.commit().await.unwrap();
        id
    }
    async fn create(
        f: &Fixture,
        session: Uuid,
        key: &str,
        lead: Option<Uuid>,
    ) -> Result<Result<ProjectRow, ProjectDbError>, sqlx::Error> {
        create_project_backend(
            &f.backend,
            f.workspace,
            f.user,
            session,
            CreateProjectInput {
                key,
                name: "  실제 프로젝트 中 😀  ",
                visibility: "private",
                description: Some("  description  "),
                icon: Some("  "),
                lead_user_id: lead,
            },
            Some("127.0.0.1"),
        )
        .await
    }
    async fn counts(f: &Fixture) -> (i64, i64, i64, i64, i64, i64, i64) {
        sqlx::query_as("SELECT (SELECT count(*) FROM projects),(SELECT count(*) FROM project_members),(SELECT count(*) FROM documents),(SELECT count(*) FROM workflows),(SELECT count(*) FROM statuses),(SELECT count(*) FROM events),(SELECT count(*) FROM audit_log)")
            .fetch_one(&f.pool).await.unwrap()
    }
    async fn member(f: &Fixture, role: &str) -> Uuid {
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO users(id,email,given_name) VALUES(?1,?2,'Lead')")
            .bind(id.as_bytes().as_slice())
            .bind(format!("{id}@example.test"))
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO memberships(workspace_id,user_id,role) VALUES(?1,?2,?3)")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(id.as_bytes().as_slice())
            .bind(role)
            .execute(&f.pool)
            .await
            .unwrap();
        id
    }

    #[tokio::test]
    async fn wiki_aux_project_create_literal_root_workflow_i18n_lead_event_and_audit() {
        let f = Fixture::new().await;
        let session = credential(&f).await;
        let lead = member(&f, "member").await;
        let long = "가".repeat(101);
        sqlx::query("INSERT INTO instance_settings(key,value) VALUES('i18n',?1)")
            .bind(
                json!({"overrides":{"seed.status.todo":long,"seed.status.done":"Shipped 中 😀"}})
                    .to_string(),
            )
            .execute(&f.pool)
            .await
            .unwrap();
        let project = create(&f, session, "LITERAL", Some(lead))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(project.workspace_id, f.workspace);
        assert_eq!(project.created_by, f.user);
        assert_eq!(project.key, "LITERAL");
        assert_eq!(project.name, "실제 프로젝트 中 😀");
        assert_eq!(project.visibility, "private");
        assert_eq!(project.description.as_deref(), Some("description"));
        assert_eq!(project.icon, None);
        assert_eq!(project.status, "active");
        let root = project.root_document_id.unwrap();
        let row: RootDocumentRow = sqlx::query_as("SELECT title,path,parent_id,sort_key,project_id,number,status,schema_version,content_json,created_by FROM documents WHERE workspace_id=?1 AND id=?2")
            .bind(f.workspace.as_bytes().as_slice()).bind(root.as_bytes().as_slice()).fetch_one(&f.pool).await.unwrap();
        assert_eq!(row.0, project.name);
        assert_eq!(row.1, to_path_label(root));
        assert_eq!(row.2, None);
        assert_eq!(row.3, "V");
        assert_eq!(row.4, project.id.as_bytes());
        assert_eq!(row.5, 1);
        assert_eq!(row.6, "published");
        assert_eq!(row.7, i64::from(DOCUMENT_SCHEMA_VERSION));
        assert_eq!(
            serde_json::from_str::<Value>(&row.8).unwrap(),
            empty_document_json()
        );
        assert_eq!(row.9, f.user.as_bytes());
        let next: (i64,) = sqlx::query_as("SELECT next_number FROM projects WHERE id=?1")
            .bind(project.id.as_bytes().as_slice())
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(next.0, 2);
        let roles: Vec<(Vec<u8>, String)> = sqlx::query_as(
            "SELECT user_id,role FROM project_members WHERE project_id=?1 ORDER BY user_id",
        )
        .bind(project.id.as_bytes().as_slice())
        .fetch_all(&f.pool)
        .await
        .unwrap();
        assert_eq!(roles.len(), 2);
        assert!(roles.contains(&(f.user.as_bytes().to_vec(), "member".into())));
        assert!(roles.contains(&(lead.as_bytes().to_vec(), "lead".into())));
        let mut read = f.backend.begin_read().await.unwrap();
        let mut op = read.operation();
        op.set_tenant(f.workspace).await.unwrap();
        assert_eq!(
            op.project_permission_by_id(f.workspace, f.user, project.id)
                .await
                .unwrap(),
            Some(ProjectPermission::Edit)
        );
        assert_eq!(
            op.project_permission_by_id(f.workspace, lead, project.id)
                .await
                .unwrap(),
            Some(ProjectPermission::Manage)
        );
        read.commit().await.unwrap();
        let statuses:Vec<(String,String,String)> = sqlx::query_as("SELECT s.name,s.category,s.sort_key FROM statuses s JOIN workflows w ON w.workspace_id=s.workspace_id AND w.id=s.workflow_id WHERE s.workspace_id=?1 AND s.project_id=?2 AND w.project_id=s.project_id ORDER BY s.sort_key")
            .bind(f.workspace.as_bytes().as_slice()).bind(project.id.as_bytes().as_slice()).fetch_all(&f.pool).await.unwrap();
        use crate::settings::messages::Message;
        assert_eq!(
            statuses,
            vec![
                (
                    Message::SeedStatusBacklog.default_text().into(),
                    "backlog".into(),
                    "V".into()
                ),
                (
                    Message::SeedStatusTodo.default_text().into(),
                    "todo".into(),
                    "W".into()
                ),
                (
                    Message::SeedStatusInProgress.default_text().into(),
                    "in_progress".into(),
                    "X".into()
                ),
                (
                    Message::SeedStatusReview.default_text().into(),
                    "in_progress".into(),
                    "Y".into()
                ),
                ("Shipped 中 😀".into(), "done".into(), "Z".into()),
                (
                    Message::SeedStatusCanceled.default_text().into(),
                    "canceled".into(),
                    "a".into()
                ),
            ]
        );
        let expected = json!({"projectId":project.id.to_string(),"key":"LITERAL","name":project.name,"visibility":"private","rootDocumentId":root.to_string()});
        let event:(Vec<u8>,Vec<u8>,String,String,String) = sqlx::query_as("SELECT workspace_id,actor_user_id,verb,channel,payload FROM events WHERE target_type='project' AND target_id=?1")
            .bind(project.id.as_bytes().as_slice()).fetch_one(&f.pool).await.unwrap();
        assert_eq!(event.0, f.workspace.as_bytes());
        assert_eq!(event.1, f.user.as_bytes());
        assert_eq!(event.2, "project.created");
        assert_eq!(event.3, "web");
        assert_eq!(serde_json::from_str::<Value>(&event.4).unwrap(), expected);
        let audit:(Vec<u8>,Vec<u8>,String,String,String) = sqlx::query_as("SELECT workspace_id,actor_user_id,verb,payload,ip FROM audit_log WHERE target_type='project' AND target_id=?1")
            .bind(project.id.as_bytes().as_slice()).fetch_one(&f.pool).await.unwrap();
        assert_eq!(audit.0, f.workspace.as_bytes());
        assert_eq!(audit.1, f.user.as_bytes());
        assert_eq!(audit.2, "project.created");
        assert_eq!(serde_json::from_str::<Value>(&audit.3).unwrap(), expected);
        assert_eq!(audit.4, "127.0.0.1");
        // An invalid stored override map uses the existing whole-row fallback.
        sqlx::query("UPDATE instance_settings SET value='{\"overrides\":{\"unknown\":\"bad\"}}' WHERE key='i18n'")
            .execute(&f.pool).await.unwrap();
        let fallback = create(&f, session, "FALLBACK", None)
            .await
            .unwrap()
            .unwrap();
        let done: (String,) =
            sqlx::query_as("SELECT name FROM statuses WHERE project_id=?1 AND sort_key='Z'")
                .bind(fallback.id.as_bytes().as_slice())
                .fetch_one(&f.pool)
                .await
                .unwrap();
        assert_eq!(done.0, Message::SeedStatusDone.default_text());
        f.close().await;
    }

    #[tokio::test]
    async fn wiki_aux_project_create_current_actor_lead_tenant_denials_and_conflict_order() {
        let f = Fixture::new().await;
        let session = credential(&f).await;
        let before = counts(&f).await;
        let guest = member(&f, "guest").await;
        assert!(matches!(
            create_project_backend(
                &f.backend,
                f.workspace,
                guest,
                session,
                CreateProjectInput {
                    key: "WRONGACTOR",
                    name: "No",
                    visibility: "private",
                    description: None,
                    icon: None,
                    lead_user_id: None
                },
                None
            )
            .await
            .unwrap(),
            Err(ProjectDbError::Forbidden)
        ));
        assert_eq!(counts(&f).await, before);
        for lead in [guest, Uuid::now_v7()] {
            assert!(matches!(
                create(&f, session, "LEADDENY", Some(lead)).await.unwrap(),
                Err(ProjectDbError::NotFound)
            ));
            assert_eq!(counts(&f).await, before);
        }
        assert!(matches!(
            create(&f, Uuid::now_v7(), "DEAD", None).await.unwrap(),
            Err(ProjectDbError::Forbidden)
        ));
        assert_eq!(counts(&f).await, before);
        for (set, restore) in [
            (
                "UPDATE sessions SET revoked_at=1 WHERE id=?1",
                "UPDATE sessions SET revoked_at=NULL WHERE id=?1",
            ),
            (
                "UPDATE users SET suspended_at=1 WHERE id=?1",
                "UPDATE users SET suspended_at=NULL WHERE id=?1",
            ),
        ] {
            let id = if set.contains("sessions") {
                session
            } else {
                f.user
            };
            sqlx::query(set)
                .bind(id.as_bytes().as_slice())
                .execute(&f.pool)
                .await
                .unwrap();
            assert!(matches!(
                create(&f, session, "CURRENT", None).await.unwrap(),
                Err(ProjectDbError::Forbidden)
            ));
            assert_eq!(counts(&f).await, before);
            sqlx::query(restore)
                .bind(id.as_bytes().as_slice())
                .execute(&f.pool)
                .await
                .unwrap();
        }
        sqlx::query("UPDATE memberships SET role='guest' WHERE workspace_id=?1 AND user_id=?2")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            create(&f, session, "GUEST", None).await.unwrap(),
            Err(ProjectDbError::NotFound)
        ));
        assert_eq!(counts(&f).await, before);
        sqlx::query("DELETE FROM memberships WHERE workspace_id=?1 AND user_id=?2")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            create(&f, session, "REMOVED", None).await.unwrap(),
            Err(ProjectDbError::NotFound)
        ));
        assert_eq!(counts(&f).await, before);
        sqlx::query("INSERT INTO memberships(workspace_id,user_id,role) VALUES(?1,?2,'owner')")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE workspaces SET deleted_at=1 WHERE id=?1")
            .bind(f.workspace.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            create(&f, session, "DELETED", None).await.unwrap(),
            Err(ProjectDbError::NotFound)
        ));
        assert_eq!(counts(&f).await, before);
        sqlx::query("UPDATE workspaces SET deleted_at=NULL WHERE id=?1")
            .bind(f.workspace.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let other = Uuid::now_v7();
        sqlx::query("INSERT INTO workspaces(id,name,slug) VALUES(?1,'Other',?2)")
            .bind(other.as_bytes().as_slice())
            .bind(other.simple().to_string())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            create_project_backend(
                &f.backend,
                other,
                f.user,
                session,
                CreateProjectInput {
                    key: "TENANT",
                    name: "Other",
                    visibility: "workspace",
                    description: None,
                    icon: None,
                    lead_user_id: None
                },
                None
            )
            .await
            .unwrap(),
            Err(ProjectDbError::NotFound)
        ));
        assert_eq!(counts(&f).await, before);
        let good = create(&f, session, "HEALTHY", None).await.unwrap().unwrap();
        let after = counts(&f).await;
        assert!(matches!(
            create(&f, session, "HEALTHY", Some(guest)).await.unwrap(),
            Err(ProjectDbError::Conflict)
        ));
        assert_eq!(counts(&f).await, after);
        assert_eq!(good.created_by, f.user);
        f.close().await;
    }

    #[tokio::test]
    async fn wiki_aux_project_create_event_audit_failures_rollback_all_rows_then_healthy_retry() {
        let f = Fixture::new().await;
        let session = credential(&f).await;
        for (table, trigger) in [
            ("events", "reject_project_event"),
            ("audit_log", "reject_project_audit"),
        ] {
            let before = counts(&f).await;
            sqlx::query(&format!("CREATE TRIGGER {trigger} BEFORE INSERT ON {table} WHEN NEW.verb='project.created' BEGIN SELECT RAISE(ABORT,'project publication failure'); END;"))
                .execute(&f.pool).await.unwrap();
            assert!(create(&f, session, "ROLLBACK", None).await.is_err());
            assert_eq!(counts(&f).await, before);
            let missing: (i64,) =
                sqlx::query_as("SELECT count(*) FROM projects WHERE key='ROLLBACK'")
                    .fetch_one(&f.pool)
                    .await
                    .unwrap();
            assert_eq!(missing.0, 0);
            sqlx::query(&format!("DROP TRIGGER {trigger}"))
                .execute(&f.pool)
                .await
                .unwrap();
            let key = if table == "events" {
                "RETRYEVENT"
            } else {
                "RETRYAUDIT"
            };
            let healthy = create(&f, session, key, None).await.unwrap().unwrap();
            assert!(healthy.root_document_id.is_some());
            assert_eq!(
                counts(&f).await,
                (
                    before.0 + 1,
                    before.1 + 1,
                    before.2 + 1,
                    before.3 + 1,
                    before.4 + 6,
                    before.5 + 1,
                    before.6 + 1
                )
            );
        }
        f.close().await;
    }

    #[tokio::test]
    async fn wiki_aux_project_create_concurrent_single_winner_and_waiting_writer_revocation() {
        let mut f = Fixture::new().await;
        let session = credential(&f).await;
        f.pool.close().await;
        f.pool = crate::db::pool::connect_sqlite_app(&f.path, 3)
            .await
            .unwrap();
        f.backend = Backend::Sqlite(f.pool.clone());
        let before = counts(&f).await;
        let (left, right) = tokio::join!(
            create(&f, session, "RACE", None),
            create(&f, session, "RACE", None)
        );
        let results = [left.unwrap(), right.unwrap()];
        assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|r| matches!(r, Err(ProjectDbError::Conflict)))
                .count(),
            1
        );
        assert_eq!(
            counts(&f).await,
            (
                before.0 + 1,
                before.1 + 1,
                before.2 + 1,
                before.3 + 1,
                before.4 + 6,
                before.5 + 1,
                before.6 + 1
            )
        );
        let after = counts(&f).await;
        let mut revoker = f.backend.begin_write().await.unwrap();
        let OperationTx::SqliteFamily(family) = revoker.operation() else {
            unreachable!()
        };
        family
            .execute(
                "UPDATE sessions SET revoked_at=1 WHERE id=?1",
                &[Cell::uuid(session)],
            )
            .await
            .unwrap();
        let backend = f.backend.clone();
        let workspace = f.workspace;
        let actor = f.user;
        let (started, wait) = tokio::sync::oneshot::channel();
        let pending = tokio::spawn(async move {
            started.send(()).unwrap();
            create_project_backend(
                &backend,
                workspace,
                actor,
                session,
                CreateProjectInput {
                    key: "WAITING",
                    name: "Waiting",
                    visibility: "private",
                    description: None,
                    icon: None,
                    lead_user_id: None,
                },
                None,
            )
            .await
        });
        wait.await.unwrap();
        revoker.commit().await.unwrap();
        assert!(matches!(
            pending.await.unwrap().unwrap(),
            Err(ProjectDbError::Forbidden)
        ));
        assert_eq!(counts(&f).await, after);
        sqlx::query("UPDATE sessions SET revoked_at=NULL WHERE id=?1")
            .bind(session.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(create(&f, session, "WAITING", None).await.unwrap().is_ok());
        f.close().await;
    }
}

#[cfg(test)]
mod selected_project_read_tests {
    use super::*;
    use crate::db::attachment_preview::tests::Fixture;

    async fn credential(f: &Fixture) -> Uuid {
        let id = Uuid::now_v7();
        let mut tx = f.backend.begin_write().await.unwrap();
        tx.operation()
            .create_session(
                id,
                f.user,
                "selected-project-read",
                crate::db::identity::stored_now()
                    + chrono::Duration::seconds(crate::auth::token::SESSION_TTL_SECS),
            )
            .await
            .unwrap();
        tx.commit().await.unwrap();
        id
    }
    async fn create(f: &Fixture, credential: Uuid, key: &str, visibility: &str) -> ProjectRow {
        create_project_backend(
            &f.backend,
            f.workspace,
            f.user,
            credential,
            CreateProjectInput {
                key,
                name: key,
                visibility,
                description: Some("read fixture"),
                icon: None,
                lead_user_id: None,
            },
            None,
        )
        .await
        .unwrap()
        .unwrap()
    }
    async fn list(f: &Fixture, credential: Uuid) -> Vec<ProjectListItem> {
        list_projects_backend(&f.backend, f.workspace, f.user, credential)
            .await
            .unwrap()
            .unwrap()
    }
    async fn workflow(f: &Fixture, credential: Uuid, project: Uuid) -> WorkflowRow {
        get_project_workflow_backend(&f.backend, f.workspace, project, f.user, credential)
            .await
            .unwrap()
            .unwrap()
    }

    #[tokio::test]
    async fn sqlite_project_reads_literal_counts_workflow_and_binary_order() {
        let f = Fixture::new().await;
        let credential = credential(&f).await;
        let private = create(&f, credential, "ZZ", "private").await;
        let public = create(&f, credential, "AA", "workspace").await;
        let flow = workflow(&f, credential, public.id).await;
        assert_eq!(flow.project_id, public.id);
        let stored: Vec<u8> =
            sqlx::query_scalar("SELECT id FROM workflows WHERE workspace_id=?1 AND project_id=?2")
                .bind(f.workspace.as_bytes().as_slice())
                .bind(public.id.as_bytes().as_slice())
                .fetch_one(&f.pool)
                .await
                .unwrap();
        assert_eq!(stored, flow.id.as_bytes());
        assert_eq!(
            flow.statuses
                .iter()
                .map(|s| (s.category.as_str(), s.sort_key.as_str()))
                .collect::<Vec<_>>(),
            [
                ("backlog", "V"),
                ("todo", "W"),
                ("in_progress", "X"),
                ("in_progress", "Y"),
                ("done", "Z"),
                ("canceled", "a")
            ]
        );
        assert!(flow
            .statuses
            .iter()
            .all(|s| !s.id.is_nil() && !s.name.is_empty() && s.wip_limit.is_none()));
        sqlx::query("UPDATE statuses SET wip_limit=3 WHERE id=?1")
            .bind(flow.statuses[2].id.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(
            workflow(&f, credential, public.id).await.statuses[2].wip_limit,
            Some(3)
        );
        // Real baseline FK constraints remain enabled; rows below are count
        // fixtures, not a replacement for the existing document/task creators.
        for (index, (status, deleted)) in [
            ("draft", None),
            ("published", None),
            ("archived", None),
            ("published", Some(1_i64)),
        ]
        .into_iter()
        .enumerate()
        {
            let id = Uuid::now_v7();
            sqlx::query("INSERT INTO documents(id,workspace_id,project_id,title,path,sort_key,number,status,schema_version,created_by,content_json,deleted_at) VALUES(?1,?2,?3,'Count',?4,'V',?5,?6,?7,?8,'{\"type\":\"doc\",\"content\":[]}',?9)")
                .bind(id.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(public.id.as_bytes().as_slice())
                .bind(to_path_label(id)).bind(i64::try_from(index).unwrap()+2).bind(status).bind(DOCUMENT_SCHEMA_VERSION)
                .bind(f.user.as_bytes().as_slice()).bind(deleted).execute(&f.pool).await.unwrap();
        }
        for (index, (status_index, archived, deleted)) in [
            (0, None, None),
            (4, None, None),
            (5, None, None),
            (1, Some(1_i64), None),
            (1, None, Some(1_i64)),
        ]
        .into_iter()
        .enumerate()
        {
            sqlx::query("INSERT INTO tasks(id,workspace_id,project_id,number,title,status_id,content_json,created_by,archived_at,deleted_at) VALUES(?1,?2,?3,?4,'Count',?5,'{}',?6,?7,?8)")
                .bind(Uuid::now_v7().as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(public.id.as_bytes().as_slice())
                .bind(i64::try_from(index).unwrap()+1).bind(flow.statuses[status_index].id.as_bytes().as_slice())
                .bind(f.user.as_bytes().as_slice()).bind(archived).bind(deleted).execute(&f.pool).await.unwrap();
        }
        sqlx::query("UPDATE projects SET status='archived' WHERE id=?1")
            .bind(public.id.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let items = list(&f, credential).await;
        assert_eq!(
            items.iter().map(|i| i.project.id).collect::<Vec<_>>(),
            [public.id, private.id]
        );
        assert_eq!(items[0].project.status, "archived");
        assert_eq!(items[0].project.root_document_id, public.root_document_id);
        assert_eq!(
            (
                items[0].document_count,
                items[0].task_count,
                items[0].open_task_count
            ),
            (4, 3, 1)
        );
        assert_eq!(
            (
                items[1].document_count,
                items[1].task_count,
                items[1].open_task_count
            ),
            (1, 0, 0)
        );
        assert!(items.iter().all(|i| i.can_edit && i.can_manage));
        assert_eq!(
            sqlx::query_scalar::<_, i64>("PRAGMA foreign_keys")
                .fetch_one(&f.pool)
                .await
                .unwrap(),
            1
        );
        sqlx::query("UPDATE projects SET deleted_at=1 WHERE id=?1")
            .bind(public.id.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(list(&f, credential).await.len(), 1);
        assert!(matches!(
            get_project_workflow_backend(&f.backend, f.workspace, public.id, f.user, credential)
                .await
                .unwrap(),
            Err(ProjectDbError::NotFound)
        ));
        f.close().await;
    }

    #[tokio::test]
    async fn sqlite_project_reads_private_group_guest_and_current_authority() {
        let f = Fixture::new().await;
        let credential = credential(&f).await;
        let public = create(&f, credential, "AA", "workspace").await;
        let private = create(&f, credential, "ZZ", "private").await;
        // No workspace-owner bypass of private projects, even if the actor was
        // their creator. Fixture revocation leaves the product read unchanged.
        sqlx::query("DELETE FROM project_members WHERE user_id=?1")
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(
            list(&f, credential)
                .await
                .iter()
                .map(|i| i.project.id)
                .collect::<Vec<_>>(),
            [public.id]
        );
        assert!(matches!(
            get_project_workflow_backend(&f.backend, f.workspace, private.id, f.user, credential)
                .await
                .unwrap(),
            Err(ProjectDbError::NotFound)
        ));
        let group = Uuid::now_v7();
        sqlx::query("INSERT INTO groups(id,workspace_id,name) VALUES(?1,?2,'Read viewers')")
            .bind(group.as_bytes().as_slice())
            .bind(f.workspace.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO group_members(workspace_id,group_id,user_id) VALUES(?1,?2,?3)")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(group.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO project_members(id,workspace_id,project_id,group_id,role) VALUES(?1,?2,?3,?4,'viewer')").bind(Uuid::now_v7().as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(private.id.as_bytes().as_slice()).bind(group.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        let items = list(&f, credential).await;
        assert_eq!(items.len(), 2);
        assert!(!items[1].can_edit && !items[1].can_manage);
        assert_eq!(
            workflow(&f, credential, private.id).await.project_id,
            private.id
        );
        sqlx::query("INSERT INTO project_members(id,workspace_id,project_id,user_id,role) VALUES(?1,?2,?3,?4,'member')").bind(Uuid::now_v7().as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(private.id.as_bytes().as_slice()).bind(f.user.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        assert!(list(&f, credential).await[1].can_edit);
        assert!(!list(&f, credential).await[1].can_manage);
        sqlx::query("DELETE FROM project_members WHERE user_id=?1")
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE memberships SET role='guest' WHERE user_id=?1")
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(
            list(&f, credential)
                .await
                .iter()
                .map(|i| i.project.id)
                .collect::<Vec<_>>(),
            [private.id]
        );
        sqlx::query("DELETE FROM group_members WHERE user_id=?1")
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(list(&f, credential).await.is_empty());
        assert!(matches!(
            get_project_workflow_backend(&f.backend, f.workspace, private.id, f.user, credential)
                .await
                .unwrap(),
            Err(ProjectDbError::NotFound)
        ));
        assert!(matches!(
            list_projects_backend(&f.backend, Uuid::now_v7(), f.user, credential)
                .await
                .unwrap(),
            Err(ProjectDbError::NotFound)
        ));
        sqlx::query("UPDATE sessions SET revoked_at=1 WHERE id=?1")
            .bind(credential.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            list_projects_backend(&f.backend, f.workspace, f.user, credential)
                .await
                .unwrap(),
            Err(ProjectDbError::Forbidden)
        ));
        assert!(matches!(
            get_project_workflow_backend(&f.backend, f.workspace, public.id, f.user, credential)
                .await
                .unwrap(),
            Err(ProjectDbError::Forbidden)
        ));
        sqlx::query("UPDATE sessions SET revoked_at=NULL,expires_at=1 WHERE id=?1")
            .bind(credential.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            list_projects_backend(&f.backend, f.workspace, f.user, credential)
                .await
                .unwrap(),
            Err(ProjectDbError::Forbidden)
        ));
        f.close().await;
    }

    #[tokio::test]
    async fn sqlite_project_reads_driver_failure_rolls_back_and_healthy_retry() {
        let f = Fixture::new().await;
        let credential = credential(&f).await;
        let project = create(&f, credential, "AA", "workspace").await;
        sqlx::query("ALTER TABLE projects RENAME COLUMN key TO broken_key")
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(
            list_projects_backend(&f.backend, f.workspace, f.user, credential)
                .await
                .is_err()
        );
        sqlx::query("ALTER TABLE projects RENAME COLUMN broken_key TO key")
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(list(&f, credential).await[0].project.id, project.id);
        sqlx::query("ALTER TABLE statuses RENAME COLUMN category TO broken_category")
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(get_project_workflow_backend(
            &f.backend,
            f.workspace,
            project.id,
            f.user,
            credential
        )
        .await
        .is_err());
        sqlx::query("ALTER TABLE statuses RENAME COLUMN broken_category TO category")
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(workflow(&f, credential, project.id).await.statuses.len(), 6);
        sqlx::query("UPDATE users SET suspended_at=1 WHERE id=?1")
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            list_projects_backend(&f.backend, f.workspace, f.user, credential)
                .await
                .unwrap(),
            Err(ProjectDbError::Forbidden)
        ));
        sqlx::query("UPDATE users SET suspended_at=NULL WHERE id=?1")
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE workspaces SET deleted_at=1 WHERE id=?1")
            .bind(f.workspace.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            get_project_workflow_backend(&f.backend, f.workspace, project.id, f.user, credential)
                .await
                .unwrap(),
            Err(ProjectDbError::NotFound)
        ));
        sqlx::query("UPDATE workspaces SET deleted_at=NULL WHERE id=?1")
            .bind(f.workspace.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM memberships WHERE workspace_id=?1 AND user_id=?2")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            list_projects_backend(&f.backend, f.workspace, f.user, credential)
                .await
                .unwrap(),
            Err(ProjectDbError::NotFound)
        ));
        f.close().await;
    }

    #[test]
    fn project_read_cleanup_failure_withholds_rows_and_retains_typed_causes() {
        // Pure returned-error propagation only. This does not simulate or prove
        // the settlement of an actual remote provider transaction.
        let error = project_read_after_rollback::<Vec<ProjectListItem>>(
            Ok(Ok(vec![])),
            Err(sqlx::Error::Protocol("cleanup".into())),
        )
        .unwrap_err();
        let sqlx::Error::AnyDriverError(source) = error else {
            panic!("missing cleanup envelope")
        };
        let unknown = source
            .downcast_ref::<crate::db::backend::RollbackCleanupUnknown>()
            .unwrap();
        assert!(unknown.original.is_none());
        assert!(matches!(&unknown.cleanup,sqlx::Error::Protocol(message) if message=="cleanup"));
        let error = project_read_after_rollback::<WorkflowRow>(
            Ok(Err(ProjectDbError::NotFound)),
            Err(sqlx::Error::Protocol("cleanup".into())),
        )
        .unwrap_err();
        let sqlx::Error::AnyDriverError(source) = error else {
            panic!("missing cleanup envelope")
        };
        assert!(matches!(
            source
                .downcast_ref::<crate::db::backend::RollbackCleanupUnknown>()
                .unwrap()
                .original
                .as_ref()
                .unwrap()
                .downcast_ref::<ProjectReadRefusal>(),
            Some(ProjectReadRefusal(ProjectDbError::NotFound))
        ));
        let error = project_read_after_rollback::<WorkflowRow>(
            Err(sqlx::Error::Protocol("original query".into())),
            Err(sqlx::Error::Protocol("cleanup".into())),
        )
        .unwrap_err();
        let sqlx::Error::AnyDriverError(source) = error else {
            panic!("missing cleanup envelope")
        };
        assert!(
            matches!(source.downcast_ref::<crate::db::backend::RollbackCleanupUnknown>().unwrap().original.as_ref().unwrap().downcast_ref::<sqlx::Error>(),Some(sqlx::Error::Protocol(message)) if message=="original query")
        );
    }
}
