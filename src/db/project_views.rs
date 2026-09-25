//! Project saved views (source `views` table; `task.ts` createView/updateView/
//! purgeView/listViews). A saved view is private to its owner and stores a
//! view query that is validated against the project (statuses, labels,
//! milestones, members and task-collection fields) when saved.

use serde_json::Value;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::collections::{ProjectViewCreateInput, ProjectViewPatchInput};
use crate::db::collections::{begin_member, scope_access, Actor};
use crate::db::view_query::{compile_view_query, CompileOptions, RootKind, SqlArgs, ViewScope};
use crate::db::workspace::WorkspaceRole;
use crate::projects::ProjectPermission;
use crate::tasks::list_query::{parse_view_query_value, view_query_to_json, ViewQuery};

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
