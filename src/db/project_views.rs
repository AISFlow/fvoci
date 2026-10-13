//! Project saved views (source `views` table; `task.ts` createView/updateView/
//! purgeView/listViews). A saved view is private to its owner and stores a
//! view query that is validated against the project (statuses, labels,
//! milestones, members and task-collection fields) when saved.

use serde_json::Value;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::collections::{ProjectViewCreateInput, ProjectViewPatchInput};
use crate::db::backend::{Backend, DbTx, FamilyTx, OperationTx};
use crate::db::codec::{Cell, FamilyRow};
use crate::db::collections::{begin_member, scope_access, Actor};
use crate::db::view_query::{
    compile_view_query, prepare_selected_task_view, CompileOptions, RootKind, SqlArgs, ViewScope,
};
use crate::db::workspace::WorkspaceRole;
use crate::projects::ProjectPermission;
use crate::tasks::list_query::{
    parse_view_query_value, view_query_to_json, AssigneeFilter, ViewQuery,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewDbError {
    NotFound,
    InvalidInput,
    VersionConflict,
}

pub type ViewResult<T> = Result<Result<T, ViewDbError>, sqlx::Error>;

#[derive(Debug, Clone)]
pub struct ProjectViewRow {
    pub id: Uuid,
    pub project_id: Uuid,
    pub name: String,
    pub view_type: String,
    pub config: Value,
}

type ViewTuple = (Uuid, Uuid, String, String, Value);

fn view_from(row: ViewTuple) -> ProjectViewRow {
    // Stored configs were canonicalised on write; re-canonicalise so the
    // response is always the parsed shape.
    let config = parse_view_query_value(&row.4)
        .map(|query| view_query_to_json(&query))
        .unwrap_or(row.4);
    ProjectViewRow {
        id: row.0,
        project_id: row.1,
        name: row.2,
        view_type: row.3,
        config,
    }
}

async fn require_project_view(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor: &Actor,
    project_id: Uuid,
    write: bool,
) -> ViewResult<WorkspaceRole> {
    let role = match begin_member(tx, workspace_id, actor, write).await? {
        Ok(role) => role,
        Err(_) => return Ok(Err(ViewDbError::NotFound)),
    };
    let access = scope_access(
        tx,
        workspace_id,
        actor.user_id,
        role,
        Some(project_id),
        write,
    )
    .await?;
    match access {
        Some(access) if access.permission.at_least(ProjectPermission::View) => Ok(Ok(role)),
        _ => Ok(Err(ViewDbError::NotFound)),
    }
}

async fn validate_config(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    project_id: Uuid,
    actor: &Actor,
    query: &ViewQuery,
) -> Result<bool, sqlx::Error> {
    let mut args = SqlArgs::starting_at(1);
    Ok(compile_view_query(
        tx,
        ViewScope {
            workspace_id,
            project_id: Some(project_id),
            collection_id: None,
            kind: RootKind::Task,
        },
        query,
        &CompileOptions {
            actor_user_id: actor.user_id,
            time_zone: "UTC",
            standard_filters: true,
        },
        "t",
        &mut args,
    )
    .await?
    .is_ok())
}

#[derive(Debug, thiserror::Error)]
#[error("project view refused: {0:?}")]
struct ViewRefusal(ViewDbError);

/// Release of a family read: an unknown cleanup withholds the observation and
/// keeps its first cause.
fn view_read_after_rollback<T>(
    result: ViewResult<T>,
    cleanup: Result<(), sqlx::Error>,
) -> ViewResult<T> {
    match cleanup {
        Ok(()) => result,
        Err(cleanup) => {
            let original: Option<Box<dyn std::error::Error + Send + Sync>> = match result {
                Err(driver) => Some(Box::new(driver)),
                Ok(Err(refusal)) => Some(Box::new(ViewRefusal(refusal))),
                Ok(Ok(_)) => None,
            };
            Err(crate::db::backend::rollback_cleanup_unknown(
                original, cleanup,
            ))
        }
    }
}

/// Finish of a family view write: only `Ok(Ok)` commits, and an uncertain
/// finish is not an ACK.
async fn finish_view_write<T>(tx: DbTx, result: ViewResult<T>) -> ViewResult<T> {
    match result {
        Ok(Ok(value)) => {
            tx.commit_with_cleanup()
                .await
                .map_err(|error| sqlx::Error::AnyDriverError(Box::new(error)))?;
            Ok(Ok(value))
        }
        Ok(Err(refusal)) => {
            if let Err(cleanup) = tx.rollback().await {
                return Err(crate::db::backend::rollback_cleanup_unknown(
                    Some(Box::new(ViewRefusal(refusal))),
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

/// [`begin_member`] in a family transaction: a live credential (rechecked for
/// a write), the live workspace, then the actor's membership role.
pub(crate) async fn begin_member_family(
    op: &mut OperationTx<'_, '_>,
    workspace_id: Uuid,
    actor: &Actor,
    write: bool,
) -> Result<Option<WorkspaceRole>, sqlx::Error> {
    op.set_tenant(workspace_id).await?;
    let live = if write {
        op.lock_membership_users(&[actor.user_id]).await?;
        op.recheck_session(actor.user_id, actor.credential_id)
            .await?
    } else {
        op.session_is_live(actor.user_id, actor.credential_id)
            .await?
    };
    if !live || !op.workspace_is_live(workspace_id).await? {
        return Ok(None);
    }
    op.membership_role(workspace_id, actor.user_id, false).await
}

/// [`scope_access`] for a project: View on the live project (archived too).
async fn project_viewable_family(
    op: &mut OperationTx<'_, '_>,
    workspace_id: Uuid,
    user_id: Uuid,
    project_id: Uuid,
) -> Result<bool, sqlx::Error> {
    Ok(op
        .project_permission_by_id(workspace_id, user_id, project_id)
        .await?
        .is_some_and(|permission| permission.at_least(ProjectPermission::View)))
}

/// [`validate_config`] in a family transaction: the standard references must
/// exist in this project (the assignee must be a live workspace member), then
/// the task-collection fields, options and sort as `compile_view_query`.
async fn validate_config_family(
    family: &mut FamilyTx,
    workspace_id: Uuid,
    project_id: Uuid,
    query: &ViewQuery,
) -> Result<bool, sqlx::Error> {
    family.require_tenant(workspace_id)?;
    let filters = &query.filters;
    for (sql, id) in [
        (
            "SELECT EXISTS(SELECT 1 FROM statuses WHERE workspace_id=?1 AND project_id=?2 AND id=?3)",
            filters.status_id,
        ),
        (
            "SELECT EXISTS(SELECT 1 FROM milestones WHERE workspace_id=?1 AND project_id=?2 AND id=?3)",
            filters.milestone_id,
        ),
        (
            "SELECT EXISTS(SELECT 1 FROM labels WHERE workspace_id=?1 AND project_id=?2 AND id=?3)",
            filters.label_id,
        ),
    ] {
        let Some(id) = id else {
            continue;
        };
        let rows = family
            .query(
                sql,
                &[
                    Cell::uuid(workspace_id),
                    Cell::uuid(project_id),
                    Cell::uuid(id),
                ],
            )
            .await?;
        if !rows
            .first()
            .ok_or(sqlx::Error::RowNotFound)?
            .cell(0)?
            .boolean()?
        {
            return Ok(false);
        }
    }
    if let Some(AssigneeFilter::User(user)) = filters.assignee_id {
        let rows = family
            .query(
                "SELECT EXISTS(SELECT 1 FROM memberships m JOIN users u ON u.id=m.user_id
                 WHERE m.workspace_id=?1 AND u.id=?2 AND u.deleted_at IS NULL)",
                &[Cell::uuid(workspace_id), Cell::uuid(user)],
            )
            .await?;
        if !rows
            .first()
            .ok_or(sqlx::Error::RowNotFound)?
            .cell(0)?
            .boolean()?
        {
            return Ok(false);
        }
    }
    Ok(prepare_selected_task_view(
        family,
        ViewScope {
            workspace_id,
            project_id: Some(project_id),
            collection_id: None,
            kind: RootKind::Task,
        },
        query,
    )
    .await?
    .is_ok())
}

fn family_view_from(row: &FamilyRow) -> Result<ProjectViewRow, sqlx::Error> {
    Ok(view_from((
        row.cell(0)?.id()?,
        row.cell(1)?.id()?,
        row.cell(2)?.string()?,
        row.cell(3)?.string()?,
        row.cell(4)?.value()?,
    )))
}

const VIEW_COLUMNS: &str = "id, project_id, name, type, config";

pub async fn list_views(
    pool: &sqlx::PgPool,
    workspace_id: Uuid,
    actor: &Actor,
    project_id: Uuid,
) -> ViewResult<Vec<ProjectViewRow>> {
    let mut tx = pool.begin().await?;
    if let Err(err) = require_project_view(&mut tx, workspace_id, actor, project_id, false).await? {
        tx.rollback().await?;
        return Ok(Err(err));
    }
    let rows: Vec<ViewTuple> = sqlx::query_as(&format!(
        "SELECT {VIEW_COLUMNS} FROM fvoci.views \
         WHERE workspace_id = $1 AND project_id = $2 AND user_id = $3 ORDER BY name, id"
    ))
    .bind(workspace_id)
    .bind(project_id)
    .bind(actor.user_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Ok(rows.into_iter().map(view_from).collect()))
}

/// [`list_views`] for every backend. Names sort by the qualified PostgreSQL
/// collation, then id.
pub async fn list_views_backend(
    backend: &Backend,
    workspace_id: Uuid,
    actor: &Actor,
    project_id: Uuid,
) -> ViewResult<Vec<ProjectViewRow>> {
    if let Backend::Postgres(pool) = backend {
        return list_views(pool, workspace_id, actor, project_id).await;
    }
    let mut tx = backend.begin_read().await?;
    let result = async {
        let mut op = tx.operation();
        if begin_member_family(&mut op, workspace_id, actor, false)
            .await?
            .is_none()
            || !project_viewable_family(&mut op, workspace_id, actor.user_id, project_id).await?
        {
            return Ok(Err(ViewDbError::NotFound));
        }
        let OperationTx::SqliteFamily(family) = op else {
            unreachable!()
        };
        family.require_tenant(workspace_id)?;
        let rows = family
            .query(
                "SELECT id,project_id,name,type,config FROM views
                 WHERE workspace_id=?1 AND project_id=?2 AND user_id=?3",
                &[
                    Cell::uuid(workspace_id),
                    Cell::uuid(project_id),
                    Cell::uuid(actor.user_id),
                ],
            )
            .await?;
        let mut views = rows
            .iter()
            .map(family_view_from)
            .collect::<Result<Vec<_>, sqlx::Error>>()?;
        crate::db::tasks::family_sort_names(&mut views, |row| (&row.name, row.id))?;
        Ok(Ok(views))
    }
    .await;
    let cleanup = tx.rollback().await;
    view_read_after_rollback(result, cleanup)
}

pub async fn create_view(
    pool: &sqlx::PgPool,
    workspace_id: Uuid,
    actor: &Actor,
    project_id: Uuid,
    input: &ProjectViewCreateInput,
) -> ViewResult<ProjectViewRow> {
    let mut tx = pool.begin().await?;
    if let Err(err) = require_project_view(&mut tx, workspace_id, actor, project_id, true).await? {
        tx.rollback().await?;
        return Ok(Err(err));
    }
    if !validate_config(&mut tx, workspace_id, project_id, actor, &input.config).await? {
        tx.rollback().await?;
        return Ok(Err(ViewDbError::InvalidInput));
    }
    let row: ViewTuple = sqlx::query_as(&format!(
        "INSERT INTO fvoci.views (id, workspace_id, project_id, user_id, name, type, config) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING {VIEW_COLUMNS}"
    ))
    .bind(Uuid::now_v7())
    .bind(workspace_id)
    .bind(project_id)
    .bind(actor.user_id)
    .bind(&input.name)
    .bind(&input.view_type)
    .bind(view_query_to_json(&input.config))
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Ok(view_from(row)))
}

/// [`create_view`] for every backend.
pub async fn create_view_backend(
    backend: &Backend,
    workspace_id: Uuid,
    actor: &Actor,
    project_id: Uuid,
    input: &ProjectViewCreateInput,
) -> ViewResult<ProjectViewRow> {
    if let Backend::Postgres(pool) = backend {
        return create_view(pool, workspace_id, actor, project_id, input).await;
    }
    let mut tx = backend.begin_write().await?;
    let result = async {
        let mut op = tx.operation();
        if begin_member_family(&mut op, workspace_id, actor, true)
            .await?
            .is_none()
            || !project_viewable_family(&mut op, workspace_id, actor.user_id, project_id).await?
        {
            return Ok(Err(ViewDbError::NotFound));
        }
        let OperationTx::SqliteFamily(family) = op else {
            unreachable!()
        };
        if !validate_config_family(family, workspace_id, project_id, &input.config).await? {
            return Ok(Err(ViewDbError::InvalidInput));
        }
        let rows = family
            .query(
                "INSERT INTO views(id,workspace_id,project_id,user_id,name,type,config)
                 VALUES(?1,?2,?3,?4,?5,?6,?7) RETURNING id,project_id,name,type,config",
                &[
                    Cell::uuid(Uuid::now_v7()),
                    Cell::uuid(workspace_id),
                    Cell::uuid(project_id),
                    Cell::uuid(actor.user_id),
                    Cell::text(&input.name),
                    Cell::text(&input.view_type),
                    Cell::json(&view_query_to_json(&input.config))?,
                ],
            )
            .await?;
        Ok(Ok(family_view_from(
            rows.first().ok_or(sqlx::Error::RowNotFound)?,
        )?))
    }
    .await;
    finish_view_write(tx, result).await
}

pub async fn update_view(
    pool: &sqlx::PgPool,
    workspace_id: Uuid,
    actor: &Actor,
    view_id: Uuid,
    input: &ProjectViewPatchInput,
) -> ViewResult<()> {
    let mut tx = pool.begin().await?;
    let role = match begin_member(&mut tx, workspace_id, actor, true).await? {
        Ok(role) => role,
        Err(_) => {
            tx.rollback().await?;
            return Ok(Err(ViewDbError::NotFound));
        }
    };
    let project_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT project_id FROM fvoci.views WHERE workspace_id = $1 AND id = $2 AND user_id = $3 FOR UPDATE",
    )
    .bind(workspace_id)
    .bind(view_id)
    .bind(actor.user_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(project_id) = project_id else {
        tx.rollback().await?;
        return Ok(Err(ViewDbError::NotFound));
    };
    let access = scope_access(
        &mut tx,
        workspace_id,
        actor.user_id,
        role,
        Some(project_id),
        true,
    )
    .await?;
    if !access.is_some_and(|access| access.permission.at_least(ProjectPermission::View)) {
        tx.rollback().await?;
        return Ok(Err(ViewDbError::NotFound));
    }
    if let Some(config) = &input.config {
        if !validate_config(&mut tx, workspace_id, project_id, actor, config).await? {
            tx.rollback().await?;
            return Ok(Err(ViewDbError::InvalidInput));
        }
    }
    let updated = sqlx::query(
        r#"
        UPDATE fvoci.views
        SET name = COALESCE($4, name),
            config = COALESCE($5, config),
            updated_at = now()
        WHERE workspace_id = $1 AND id = $2 AND user_id = $3
          AND ($6::jsonb IS NULL OR config = $6::jsonb)
        "#,
    )
    .bind(workspace_id)
    .bind(view_id)
    .bind(actor.user_id)
    .bind(&input.name)
    .bind(input.config.as_ref().map(view_query_to_json))
    .bind(input.expected_config.as_ref().map(view_query_to_json))
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() == 0 {
        tx.rollback().await?;
        return Ok(Err(ViewDbError::VersionConflict));
    }
    tx.commit().await?;
    Ok(Ok(()))
}

/// [`update_view`] for every backend. The expected-config compare runs on the
/// stored JSON value inside the reserved writer, as PostgreSQL's `jsonb` `=`
/// in its `UPDATE`.
pub async fn update_view_backend(
    backend: &Backend,
    workspace_id: Uuid,
    actor: &Actor,
    view_id: Uuid,
    input: &ProjectViewPatchInput,
) -> ViewResult<()> {
    if let Backend::Postgres(pool) = backend {
        return update_view(pool, workspace_id, actor, view_id, input).await;
    }
    let mut tx = backend.begin_write().await?;
    let result = async {
        let mut op = tx.operation();
        if begin_member_family(&mut op, workspace_id, actor, true)
            .await?
            .is_none() {
            return Ok(Err(ViewDbError::NotFound));
        }
        let OperationTx::SqliteFamily(family) = &mut op else {
            unreachable!()
        };
        family.require_tenant(workspace_id)?;
        let rows = family
            .query(
                "SELECT project_id,config FROM views WHERE workspace_id=?1 AND id=?2 AND user_id=?3",
                &[
                    Cell::uuid(workspace_id),
                    Cell::uuid(view_id),
                    Cell::uuid(actor.user_id),
                ],
            )
            .await?;
        let Some(row) = rows.first() else {
            return Ok(Err(ViewDbError::NotFound));
        };
        let (project_id, stored) = (row.cell(0)?.id()?, row.cell(1)?.value()?);
        if !project_viewable_family(&mut op, workspace_id, actor.user_id, project_id).await? {
            return Ok(Err(ViewDbError::NotFound));
        }
        let OperationTx::SqliteFamily(family) = op else {
            unreachable!()
        };
        if let Some(config) = &input.config {
            if !validate_config_family(family, workspace_id, project_id, config).await? {
                return Ok(Err(ViewDbError::InvalidInput));
            }
        }
        if input
            .expected_config
            .as_ref()
            .is_some_and(|expected| view_query_to_json(expected) != stored)
        {
            return Ok(Err(ViewDbError::VersionConflict));
        }
        let config = input
            .config
            .as_ref()
            .map(|config| Cell::json(&view_query_to_json(config)))
            .transpose()?
            .unwrap_or(Cell::Null);
        let updated = family
            .execute(
                "UPDATE views SET name=COALESCE(?4,name),config=COALESCE(?5,config),
                 updated_at=(unixepoch()*1000000+CAST(substr(strftime('%f','now'),4,3) AS INTEGER)*1000)
                 WHERE workspace_id=?1 AND id=?2 AND user_id=?3",
                &[
                    Cell::uuid(workspace_id),
                    Cell::uuid(view_id),
                    Cell::uuid(actor.user_id),
                    Cell::optional_text(input.name.as_deref()),
                    config,
                ],
            )
            .await?;
        if updated == 0 {
            return Ok(Err(ViewDbError::VersionConflict));
        }
        Ok(Ok(()))
    }
    .await;
    finish_view_write(tx, result).await
}

pub async fn delete_view(
    pool: &sqlx::PgPool,
    workspace_id: Uuid,
    actor: &Actor,
    view_id: Uuid,
) -> ViewResult<()> {
    let mut tx = pool.begin().await?;
    if begin_member(&mut tx, workspace_id, actor, true)
        .await?
        .is_err()
    {
        tx.rollback().await?;
        return Ok(Err(ViewDbError::NotFound));
    }
    let deleted =
        sqlx::query("DELETE FROM fvoci.views WHERE workspace_id = $1 AND id = $2 AND user_id = $3")
            .bind(workspace_id)
            .bind(view_id)
            .bind(actor.user_id)
            .execute(&mut *tx)
            .await?;
    if deleted.rows_affected() == 0 {
        tx.rollback().await?;
        return Ok(Err(ViewDbError::NotFound));
    }
    tx.commit().await?;
    Ok(Ok(()))
}

/// [`delete_view`] for every backend: the owner's row, with no project check.
pub async fn delete_view_backend(
    backend: &Backend,
    workspace_id: Uuid,
    actor: &Actor,
    view_id: Uuid,
) -> ViewResult<()> {
    if let Backend::Postgres(pool) = backend {
        return delete_view(pool, workspace_id, actor, view_id).await;
    }
    let mut tx = backend.begin_write().await?;
    let result = async {
        let mut op = tx.operation();
        if begin_member_family(&mut op, workspace_id, actor, true)
            .await?
            .is_none()
        {
            return Ok(Err(ViewDbError::NotFound));
        }
        let OperationTx::SqliteFamily(family) = op else {
            unreachable!()
        };
        family.require_tenant(workspace_id)?;
        let deleted = family
            .execute(
                "DELETE FROM views WHERE workspace_id=?1 AND id=?2 AND user_id=?3",
                &[
                    Cell::uuid(workspace_id),
                    Cell::uuid(view_id),
                    Cell::uuid(actor.user_id),
                ],
            )
            .await?;
        if deleted == 0 {
            return Ok(Err(ViewDbError::NotFound));
        }
        Ok(Ok(()))
    }
    .await;
    finish_view_write(tx, result).await
}
