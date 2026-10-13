//! Workflow status management and the workspace status list (source
//! `packages/core/src/workflow.ts` at `393795261322b916e588043cf94feca999175843`).
//!
//! Writes need manage permission on the workflow's project and a writable
//! (unarchived) project, rechecked inside the transaction after the project
//! row lock. The project lock also orders them against task writes, which
//! take it before touching a status, so a status cannot be deleted while a
//! concurrent create or move puts a task into it.

use crate::db::backend::{Backend, DbTx, FamilyTx, OperationTx};
use crate::db::codec::{Cell, FamilyRow};
use crate::db::tasks::{family_project_access, finish_task_operation};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::db::context::{lock_membership_users, recheck_session, session_is_live, set_tenant};
use crate::db::documents::between;
use crate::db::projects::{lock_project, project_permission, visible_project_sql, ProjectDbError};
use crate::db::workspace::{membership_role, workspace_is_live, WorkspaceRole};
use crate::projects::ProjectPermission;

/// Source `MAX_WORKFLOW_STATUSES`.
pub const MAX_WORKFLOW_STATUSES: i64 = 200;

pub const STATUS_CATEGORIES: &[&str] = &["backlog", "todo", "in_progress", "done", "canceled"];

pub fn status_category_is_valid(value: &str) -> bool {
    STATUS_CATEGORIES.contains(&value)
}

/// Source `z.string().trim().min(1).max(100)` (UTF-16 code units).
pub fn status_name_is_valid(trimmed: &str) -> bool {
    let len = trimmed.encode_utf16().count();
    (1..=100).contains(&len)
}

#[derive(Debug, Clone)]
pub struct StatusRow {
    pub id: Uuid,
    pub workflow_id: Uuid,
    pub project_id: Uuid,
    pub name: String,
    pub category: String,
    pub sort_key: String,
    pub wip_limit: Option<i32>,
}

type StatusTuple = (Uuid, Uuid, Uuid, String, String, String, Option<i32>);

fn status_row(
    (id, workflow_id, project_id, name, category, sort_key, wip_limit): StatusTuple,
) -> StatusRow {
    StatusRow {
        id,
        workflow_id,
        project_id,
        name,
        category,
        sort_key,
        wip_limit,
    }
}

const STATUS_COLUMNS: &str = "id, workflow_id, project_id, name, category, sort_key, wip_limit";

/// Source `listWorkspaceStatuses`: statuses of every live project the actor
/// can view, for any workspace member (guests see only granted projects).
pub async fn list_workspace_statuses(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<Result<Vec<StatusRow>, ProjectDbError>, sqlx::Error> {
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
    let Some(role) = membership_role(&mut tx, workspace_id, actor_user_id).await? else {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    };
    let visible = visible_project_sql("p", 2, 3);
    let sql = format!(
        r#"
        SELECT s.id, s.workflow_id, s.project_id, s.name, s.category, s.sort_key, s.wip_limit
        FROM fvoci.statuses s
        INNER JOIN fvoci.projects p
            ON p.workspace_id = s.workspace_id AND p.id = s.project_id
        WHERE s.workspace_id = $1
          AND p.deleted_at IS NULL
          AND {visible}
        ORDER BY s.sort_key COLLATE "C", s.id
        "#
    );
    let rows: Vec<StatusTuple> = sqlx::query_as(&sql)
        .bind(workspace_id)
        .bind(role == WorkspaceRole::Guest)
        .bind(actor_user_id)
        .fetch_all(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Ok(rows.into_iter().map(status_row).collect()))
}

pub async fn list_workspace_statuses_backend(
    backend: &Backend,
    workspace: Uuid,
    actor: Uuid,
    credential: Uuid,
) -> Result<Result<Vec<StatusRow>, ProjectDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return list_workspace_statuses(pool, workspace, actor, credential).await;
    }
    let mut tx = backend.begin_read().await?;
    let result=async {
        let mut op = tx.operation();
        op.set_tenant(workspace).await?;
        if !op.session_is_live(actor, credential).await? {
            return Ok(Err(ProjectDbError::Forbidden));
        }
        if !op.workspace_is_live(workspace).await?
            || op.membership_role(workspace, actor, false).await?.is_none()
        {
            return Ok(Err(ProjectDbError::NotFound));
        }

        let OperationTx::SqliteFamily(family) = &mut op else {
            unreachable!()
        };
        let rows=family.query("SELECT id,workflow_id,project_id,name,category,sort_key,wip_limit FROM statuses WHERE workspace_id=?1 ORDER BY sort_key COLLATE BINARY,id",&[Cell::uuid(workspace)]).await?;

        let mut items = Vec::new();
        for row in rows {
            let status = family_status_row(&row)?;
            if op
                .project_permission_by_id(workspace, actor, status.project_id)
                .await?
                .is_some_and(|p| p.at_least(ProjectPermission::View))
            {
                items.push(status);
            }
        }
        Ok(Ok(items))
    }
.await;

    finish_task_operation(tx, result).await
}
fn family_status_row(r: &FamilyRow) -> Result<StatusRow, sqlx::Error> {
    Ok(status_row((
        r.cell(0)?.id()?,
        r.cell(1)?.id()?,
        r.cell(2)?.id()?,
        r.cell(3)?.string()?,
        r.cell(4)?.string()?,
        r.cell(5)?.string()?,
        r.cell(6)?.optional(Cell::int32)?,
    )))
}
async fn family_workflow_manage(
    tx: &mut FamilyTx,
    workspace: Uuid,
    workflow: Uuid,
    actor: Uuid,
    credential: Uuid,
) -> Result<Result<Uuid, ProjectDbError>, sqlx::Error> {
    let mut op = OperationTx::SqliteFamily(&mut *tx);
    op.set_tenant(workspace).await?;
    if !op.recheck_session(actor, credential).await? {
        return Ok(Err(ProjectDbError::Forbidden));
    }
    if !op.workspace_is_live(workspace).await? {
        return Ok(Err(ProjectDbError::NotFound));
    }
    let rows = tx
        .query(
            "SELECT project_id FROM workflows WHERE workspace_id=?1 AND id=?2",
            &[Cell::uuid(workspace), Cell::uuid(workflow)],
        )
        .await?;
    let Some(row) = rows.first() else {
        return Ok(Err(ProjectDbError::NotFound));
    };
    let project = row.cell(0)?.id()?;
    // Manage is checked before archive, matching the PostgreSQL error precedence.
    let mut op = OperationTx::SqliteFamily(&mut *tx);
    if !op
        .project_permission_by_id(workspace, actor, project)
        .await?
        .is_some_and(|p| p.at_least(ProjectPermission::Manage))
    {
        return Ok(Err(ProjectDbError::NotFound));
    }
    Ok(family_project_access(
        &mut op,
        workspace,
        project,
        actor,
        credential,
        ProjectPermission::Manage,
        true,
    )
    .await?
    .map(|()| project))
}
async fn family_statuses(
    tx: &mut FamilyTx,
    workspace: Uuid,
    workflow: Uuid,
) -> Result<Vec<StatusRow>, sqlx::Error> {
    tx.query("SELECT id,workflow_id,project_id,name,category,sort_key,wip_limit FROM statuses WHERE workspace_id=?1 AND workflow_id=?2 ORDER BY sort_key COLLATE BINARY,id",&[Cell::uuid(workspace),Cell::uuid(workflow)]).await?.iter().map(family_status_row).collect()
}

/// Source `requireWorkflowManage`: returns the workflow's project after
/// locking it and the workflow row. Missing, hidden and view/edit-only are all
/// 404; an archived project is `project_archived`.
async fn require_workflow_manage(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    workflow_id: Uuid,
) -> Result<Result<Uuid, ProjectDbError>, sqlx::Error> {
    lock_membership_users(tx, &[actor_user_id]).await?;
    if !recheck_session(tx, actor_user_id, session_id).await? {
        return Ok(Err(ProjectDbError::Forbidden));
    }
    if !workspace_is_live(tx, workspace_id).await? {
        return Ok(Err(ProjectDbError::NotFound));
    }
    let project_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT project_id FROM fvoci.workflows WHERE workspace_id = $1 AND id = $2",
    )
    .bind(workspace_id)
    .bind(workflow_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(project_id) = project_id else {
        return Ok(Err(ProjectDbError::NotFound));
    };
    let Some(project) = lock_project(tx, workspace_id, project_id).await? else {
        return Ok(Err(ProjectDbError::NotFound));
    };
    if !project_permission(tx, workspace_id, actor_user_id, &project)
        .await?
        .at_least(ProjectPermission::Manage)
    {
        return Ok(Err(ProjectDbError::NotFound));
    }
    if project.status == "archived" {
        return Ok(Err(ProjectDbError::Archived));
    }
    // Every status creation counts under this row lock, so two concurrent
    // creations cannot both take the last slot.
    sqlx::query("SELECT id FROM fvoci.workflows WHERE workspace_id = $1 AND id = $2 FOR UPDATE")
        .bind(workspace_id)
        .bind(workflow_id)
        .execute(&mut **tx)
        .await?;
    Ok(Ok(project_id))
}

async fn list_workflow_statuses(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    workflow_id: Uuid,
) -> Result<Vec<StatusRow>, sqlx::Error> {
    let sql = format!(
        r#"
        SELECT {STATUS_COLUMNS}
        FROM fvoci.statuses
        WHERE workspace_id = $1 AND workflow_id = $2
        ORDER BY sort_key COLLATE "C", id
        "#
    );
    let rows: Vec<StatusTuple> = sqlx::query_as(&sql)
        .bind(workspace_id)
        .bind(workflow_id)
        .fetch_all(&mut **tx)
        .await?;
    Ok(rows.into_iter().map(status_row).collect())
}

async fn fetch_status(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    status_id: Uuid,
) -> Result<Option<StatusRow>, sqlx::Error> {
    let sql =
        format!("SELECT {STATUS_COLUMNS} FROM fvoci.statuses WHERE workspace_id = $1 AND id = $2");
    let row: Option<StatusTuple> = sqlx::query_as(&sql)
        .bind(workspace_id)
        .bind(status_id)
        .fetch_optional(&mut **tx)
        .await?;
    Ok(row.map(status_row))
}

pub struct CreateStatusInput {
    pub name: String,
    pub category: String,
    pub wip_limit: Option<i32>,
}

/// Source `createStatus`: appended after the last status of the workflow.
pub async fn create_status(
    pool: &PgPool,
    workspace_id: Uuid,
    workflow_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    input: CreateStatusInput,
) -> Result<Result<StatusRow, ProjectDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let project_id = match require_workflow_manage(
        &mut tx,
        workspace_id,
        actor_user_id,
        session_id,
        workflow_id,
    )
    .await?
    {
        Ok(project_id) => project_id,
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    };
    let siblings = list_workflow_statuses(&mut tx, workspace_id, workflow_id).await?;
    if siblings.len() as i64 >= MAX_WORKFLOW_STATUSES {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::WorkflowStatusLimit));
    }
    let sort_key = match between(siblings.last().map(|s| s.sort_key.as_str()), None) {
        Ok(key) => key,
        Err(err) => {
            tracing::error!("status sort_key allocation failed: {err}");
            tx.rollback().await?;
            return Ok(Err(ProjectDbError::Conflict));
        }
    };
    let sql = format!(
        r#"
        INSERT INTO fvoci.statuses (
            id, workspace_id, project_id, workflow_id, name, category, sort_key, wip_limit
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        RETURNING {STATUS_COLUMNS}
        "#
    );
    let row: StatusTuple = sqlx::query_as(&sql)
        .bind(Uuid::now_v7())
        .bind(workspace_id)
        .bind(project_id)
        .bind(workflow_id)
        .bind(&input.name)
        .bind(&input.category)
        .bind(&sort_key)
        .bind(input.wip_limit)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Ok(status_row(row)))
}

pub async fn create_status_backend(
    backend: &Backend,
    workspace: Uuid,
    workflow: Uuid,
    actor: Uuid,
    credential: Uuid,
    input: CreateStatusInput,
) -> Result<Result<StatusRow, ProjectDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return create_status(pool, workspace, workflow, actor, credential, input).await;
    }
    let mut tx = backend.begin_write().await?;
    let result=async {
        let DbTx::SqliteFamily(family) = &mut tx else {
            unreachable!()
        };
        let project =
            match family_workflow_manage(family, workspace, workflow, actor, credential).await? {
                Ok(p) => p,
                Err(e) => return Ok(Err(e)),
            };
        let siblings = family_statuses(family, workspace, workflow).await?;

        if siblings.len() as i64 >= MAX_WORKFLOW_STATUSES {
            return Ok(Err(ProjectDbError::WorkflowStatusLimit));
        }
        let Ok(key) = between(siblings.last().map(|s| s.sort_key.as_str()), None) else {
            return Ok(Err(ProjectDbError::Conflict));
        };

        let rows=family.query("INSERT INTO statuses(id,workspace_id,project_id,workflow_id,name,category,sort_key,wip_limit) VALUES(?1,?2,?3,?4,?5,?6,?7,?8) RETURNING id,workflow_id,project_id,name,category,sort_key,wip_limit",&[Cell::uuid(Uuid::now_v7()),Cell::uuid(workspace),Cell::uuid(project),Cell::uuid(workflow),Cell::text(input.name),Cell::text(input.category),Cell::text(key),input.wip_limit.map(|v|Cell::Integer(i64::from(v))).unwrap_or(Cell::Null)]).await?;
        Ok(Ok(family_status_row(
            rows.first().ok_or(sqlx::Error::RowNotFound)?,
        )?))
    }
.await;

    finish_task_operation(tx, result).await
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusAnchor {
    Before(Uuid),
    After(Uuid),
}

#[derive(Default)]
pub struct UpdateStatusInput {
    pub name: Option<String>,
    pub category: Option<String>,
    /// `Some(None)` clears the limit.
    pub wip_limit: Option<Option<i32>>,
    pub anchor: Option<StatusAnchor>,
}

/// Source `resolveAnchorSortKey` over the other statuses of the workflow.
fn anchor_sort_key(
    others: &[StatusRow],
    anchor: StatusAnchor,
) -> Result<Result<String, ProjectDbError>, crate::db::documents::FractionalError> {
    let (target, before) = match anchor {
        StatusAnchor::Before(id) => (id, true),
        StatusAnchor::After(id) => (id, false),
    };
    let Some(index) = others.iter().position(|s| s.id == target) else {
        return Ok(Err(ProjectDbError::InvalidAnchor));
    };
    let key = |i: usize| others.get(i).map(|s| s.sort_key.as_str());
    let (low, high) = if before {
        (index.checked_sub(1).and_then(key), key(index))
    } else {
        (key(index), key(index + 1))
    };
    Ok(Ok(between(low, high)?))
}

/// Source `updateStatus` + `reorderStatus`, applied together in one
/// transaction (the source runs them as two).
pub async fn update_status(
    pool: &PgPool,
    workspace_id: Uuid,
    workflow_id: Uuid,
    status_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    input: UpdateStatusInput,
) -> Result<Result<StatusRow, ProjectDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if let Err(err) = require_workflow_manage(
        &mut tx,
        workspace_id,
        actor_user_id,
        session_id,
        workflow_id,
    )
    .await?
    {
        tx.rollback().await?;
        return Ok(Err(err));
    }
    let Some(status) = fetch_status(&mut tx, workspace_id, status_id).await? else {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    };
    if status.workflow_id != workflow_id {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }
    let sort_key = match input.anchor {
        None => None,
        Some(anchor) => {
            let others: Vec<StatusRow> = list_workflow_statuses(&mut tx, workspace_id, workflow_id)
                .await?
                .into_iter()
                .filter(|s| s.id != status_id)
                .collect();
            match anchor_sort_key(&others, anchor) {
                Ok(Ok(key)) => Some(key),
                Ok(Err(err)) => {
                    tx.rollback().await?;
                    return Ok(Err(err));
                }
                Err(err) => {
                    tracing::error!("status sort_key allocation failed: {err}");
                    tx.rollback().await?;
                    return Ok(Err(ProjectDbError::Conflict));
                }
            }
        }
    };
    let sql = format!(
        r#"
        UPDATE fvoci.statuses
        SET name = COALESCE($3, name),
            category = COALESCE($4, category),
            wip_limit = CASE WHEN $5 THEN $6 ELSE wip_limit END,
            sort_key = COALESCE($7, sort_key),
            updated_at = now()
        WHERE workspace_id = $1 AND id = $2
        RETURNING {STATUS_COLUMNS}
        "#
    );
    let row: StatusTuple = sqlx::query_as(&sql)
        .bind(workspace_id)
        .bind(status_id)
        .bind(input.name)
        .bind(input.category)
        .bind(input.wip_limit.is_some())
        .bind(input.wip_limit.flatten())
        .bind(sort_key)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Ok(status_row(row)))
}

#[allow(clippy::too_many_arguments)]
pub async fn update_status_backend(
    backend: &Backend,
    workspace: Uuid,
    workflow: Uuid,
    status: Uuid,
    actor: Uuid,
    credential: Uuid,
    input: UpdateStatusInput,
) -> Result<Result<StatusRow, ProjectDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return update_status(pool, workspace, workflow, status, actor, credential, input).await;
    }
    let mut tx = backend.begin_write().await?;
    let result=async {
        let DbTx::SqliteFamily(family) = &mut tx else {
            unreachable!()
        };
        if let Err(e) = family_workflow_manage(family, workspace, workflow, actor, credential).await? {
            return Ok(Err(e));
        }
        let siblings = family_statuses(family, workspace, workflow).await?;
        if !siblings.iter().any(|s| s.id == status) {
            return Ok(Err(ProjectDbError::NotFound));
        }

        let key = if let Some(anchor) = input.anchor {
            let others = siblings
                .into_iter()
                .filter(|s| s.id != status)
                .collect::<Vec<_>>();
            match anchor_sort_key(&others, anchor) {
                Ok(Ok(key)) => Some(key),
                Ok(Err(e)) => return Ok(Err(e)),
                Err(_) => return Ok(Err(ProjectDbError::Conflict)),
            }
        } else {
            None
        };

        let rows=family.query("UPDATE statuses SET name=COALESCE(?3,name),category=COALESCE(?4,category),wip_limit=CASE WHEN ?5 THEN ?6 ELSE wip_limit END,sort_key=COALESCE(?7,sort_key),updated_at=unixepoch()*1000000+CAST(substr(strftime('%f','now'),4,3) AS INTEGER)*1000 WHERE workspace_id=?1 AND id=?2 RETURNING id,workflow_id,project_id,name,category,sort_key,wip_limit",&[Cell::uuid(workspace),Cell::uuid(status),Cell::optional_text(input.name.as_deref()),Cell::optional_text(input.category.as_deref()),Cell::Integer(i64::from(input.wip_limit.is_some())),input.wip_limit.flatten().map(|v|Cell::Integer(i64::from(v))).unwrap_or(Cell::Null),Cell::optional_text(key.as_deref())]).await?;
        Ok(Ok(family_status_row(
            rows.first().ok_or(sqlx::Error::RowNotFound)?,
        )?))
    }
.await;

    finish_task_operation(tx, result).await
}

/// Source `purgeStatus`: refused with `status_has_tasks` while any task row
/// (trashed included) still uses the status.
pub async fn delete_status(
    pool: &PgPool,
    workspace_id: Uuid,
    workflow_id: Uuid,
    status_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<Result<(), ProjectDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if let Err(err) = require_workflow_manage(
        &mut tx,
        workspace_id,
        actor_user_id,
        session_id,
        workflow_id,
    )
    .await?
    {
        tx.rollback().await?;
        return Ok(Err(err));
    }
    let Some(status) = fetch_status(&mut tx, workspace_id, status_id).await? else {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    };
    if status.workflow_id != workflow_id {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }
    let occupied: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM fvoci.tasks WHERE workspace_id = $1 AND status_id = $2)",
    )
    .bind(workspace_id)
    .bind(status_id)
    .fetch_one(&mut *tx)
    .await?;
    if occupied {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::StatusHasTasks));
    }
    sqlx::query("DELETE FROM fvoci.statuses WHERE workspace_id = $1 AND id = $2")
        .bind(workspace_id)
        .bind(status_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Ok(()))
}

pub async fn delete_status_backend(
    backend: &Backend,
    workspace: Uuid,
    workflow: Uuid,
    status: Uuid,
    actor: Uuid,
    credential: Uuid,
) -> Result<Result<(), ProjectDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return delete_status(pool, workspace, workflow, status, actor, credential).await;
    }
    let mut tx = backend.begin_write().await?;
    let result = async {
        let DbTx::SqliteFamily(family) = &mut tx else {
            unreachable!()
        };
        if let Err(e) =
            family_workflow_manage(family, workspace, workflow, actor, credential).await?
        {
            return Ok(Err(e));
        }
        if !family_statuses(family, workspace, workflow)
            .await?
            .iter()
            .any(|s| s.id == status)
        {
            return Ok(Err(ProjectDbError::NotFound));
        }
        if !family
            .query(
                "SELECT id FROM tasks WHERE workspace_id=?1 AND status_id=?2 LIMIT 1",
                &[Cell::uuid(workspace), Cell::uuid(status)],
            )
            .await?
            .is_empty()
        {
            return Ok(Err(ProjectDbError::StatusHasTasks));
        }
        family
            .execute(
                "DELETE FROM statuses WHERE workspace_id=?1 AND id=?2",
                &[Cell::uuid(workspace), Cell::uuid(status)],
            )
            .await?;
        Ok(Ok(()))
    }
    .await;
    finish_task_operation(tx, result).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(id: Uuid, sort_key: &str) -> StatusRow {
        StatusRow {
            id,
            workflow_id: Uuid::nil(),
            project_id: Uuid::nil(),
            name: String::new(),
            category: "todo".into(),
            sort_key: sort_key.into(),
            wip_limit: None,
        }
    }

    #[test]
    fn anchor_places_between_neighbours() {
        let (a, b, c) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
        let others = vec![status(a, "V"), status(b, "W"), status(c, "X")];
        let before_b = anchor_sort_key(&others, StatusAnchor::Before(b))
            .unwrap()
            .unwrap();
        assert!(before_b.as_str() > "V" && before_b.as_str() < "W");
        let after_c = anchor_sort_key(&others, StatusAnchor::After(c))
            .unwrap()
            .unwrap();
        assert!(after_c.as_str() > "X");
        let before_a = anchor_sort_key(&others, StatusAnchor::Before(a))
            .unwrap()
            .unwrap();
        assert!(before_a.as_str() < "V");
        assert!(matches!(
            anchor_sort_key(&others, StatusAnchor::After(Uuid::now_v7())).unwrap(),
            Err(ProjectDbError::InvalidAnchor)
        ));
    }

    #[test]
    fn names_and_categories_follow_the_source_limits() {
        assert!(status_name_is_valid("Review"));
        assert!(!status_name_is_valid(""));
        assert!(status_name_is_valid(&"가".repeat(100)));
        assert!(!status_name_is_valid(&"가".repeat(101)));
        assert!(status_category_is_valid("in_progress"));
        assert!(!status_category_is_valid("doing"));
    }
}
