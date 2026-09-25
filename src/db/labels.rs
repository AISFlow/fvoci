use chrono::{DateTime, Utc};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::db::context::{lock_membership_users, recheck_session, session_is_live, set_tenant};
use crate::db::projects::{lock_project, project_permission, visible_project_sql, ProjectDbError};
use crate::db::workspace::WorkspaceRole;
use crate::projects::ProjectPermission;

pub const LABEL_NAME_MAX: usize = 100;
pub const LABEL_COLORS: &[&str] = &[
    "gray", "red", "orange", "amber", "green", "teal", "blue", "violet", "pink",
];

#[derive(Debug, Clone)]
pub struct LabelRow {
    pub id: Uuid,
    pub project_id: Uuid,
    pub name: String,
    pub color: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub fn label_name_is_valid(name: &str) -> bool {
    let trimmed = name.trim();
    !trimmed.is_empty() && trimmed.chars().count() <= LABEL_NAME_MAX
}

pub fn label_color_is_valid(color: &str) -> bool {
    LABEL_COLORS.contains(&color)
}

async fn workspace_is_live(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let row: Option<(Option<DateTime<Utc>>,)> =
        sqlx::query_as("SELECT deleted_at FROM fvoci.workspaces WHERE id = $1")
            .bind(workspace_id)
            .fetch_optional(&mut **tx)
            .await?;
    Ok(row.map(|(deleted,)| deleted.is_none()).unwrap_or(false))
}

async fn membership_role(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<Option<WorkspaceRole>, sqlx::Error> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT role FROM fvoci.memberships WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(workspace_id)
    .bind(user_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.and_then(|(role,)| WorkspaceRole::parse(&role)))
}

async fn require_project_view(
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
    let Some(locked) = lock_project(tx, workspace_id, project_id).await? else {
        return Ok(Err(ProjectDbError::NotFound));
    };
    let permission = project_permission(tx, workspace_id, actor_user_id, &locked).await?;
    if !permission.at_least(ProjectPermission::View) {
        return Ok(Err(ProjectDbError::NotFound));
    }
    Ok(Ok(()))
}

async fn require_project_edit(
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

fn map_label_row(
    id: Uuid,
    project_id: Uuid,
    name: String,
    color: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
) -> LabelRow {
    LabelRow {
        id,
        project_id,
        name,
        color,
        created_at,
        updated_at,
    }
}

pub async fn list_project_labels(
    pool: &PgPool,
    workspace_id: Uuid,
    project_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<Result<Vec<LabelRow>, ProjectDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    match require_project_view(&mut tx, workspace_id, actor_user_id, session_id, project_id).await?
    {
        Ok(()) => {}
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    }
    let rows = sqlx::query_as::<_, (Uuid, Uuid, String, String, DateTime<Utc>, DateTime<Utc>)>(
        r#"
        SELECT id, project_id, name, color, created_at, updated_at
        FROM fvoci.labels
        WHERE workspace_id = $1 AND project_id = $2
        ORDER BY name, id
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Ok(rows
        .into_iter()
        .map(|(id, project_id, name, color, created_at, updated_at)| {
            map_label_row(id, project_id, name, color, created_at, updated_at)
        })
        .collect()))
}

pub async fn list_workspace_labels(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<Result<Vec<LabelRow>, ProjectDbError>, sqlx::Error> {
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
    let guest = role == WorkspaceRole::Guest;
    let visible = visible_project_sql("p", 2, 3);
    let sql = format!(
        r#"
        SELECT l.id, l.project_id, l.name, l.color, l.created_at, l.updated_at
        FROM fvoci.labels l
        INNER JOIN fvoci.projects p
            ON p.workspace_id = l.workspace_id AND p.id = l.project_id
        WHERE l.workspace_id = $1
          AND p.deleted_at IS NULL
          AND {visible}
        ORDER BY l.name, l.id
        "#
    );
    let rows =
        sqlx::query_as::<_, (Uuid, Uuid, String, String, DateTime<Utc>, DateTime<Utc>)>(&sql)
            .bind(workspace_id)
            .bind(guest)
            .bind(actor_user_id)
            .fetch_all(&mut *tx)
            .await?;
    tx.commit().await?;
    Ok(Ok(rows
        .into_iter()
        .map(|(id, project_id, name, color, created_at, updated_at)| {
            map_label_row(id, project_id, name, color, created_at, updated_at)
        })
        .collect()))
}

pub async fn create_label(
    pool: &PgPool,
    workspace_id: Uuid,
    project_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    name: &str,
    color: &str,
) -> Result<Result<LabelRow, ProjectDbError>, sqlx::Error> {
    let name = name.trim();
    if !label_name_is_valid(name) || !label_color_is_valid(color) {
        return Ok(Err(ProjectDbError::InvalidInput));
    }
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    match require_project_edit(&mut tx, workspace_id, actor_user_id, session_id, project_id).await?
    {
        Ok(()) => {}
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    }
    let id = Uuid::now_v7();
    let row = sqlx::query_as::<_, (Uuid, Uuid, String, String, DateTime<Utc>, DateTime<Utc>)>(
        r#"
        INSERT INTO fvoci.labels (id, workspace_id, project_id, name, color)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING id, project_id, name, color, created_at, updated_at
        "#,
    )
    .bind(id)
    .bind(workspace_id)
    .bind(project_id)
    .bind(name)
    .bind(color)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Ok(map_label_row(row.0, row.1, row.2, row.3, row.4, row.5)))
}

#[allow(clippy::too_many_arguments)]
pub async fn update_label(
    pool: &PgPool,
    workspace_id: Uuid,
    project_id: Uuid,
    label_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    name: Option<&str>,
    color: Option<&str>,
) -> Result<Result<(), ProjectDbError>, sqlx::Error> {
    if name.is_none() && color.is_none() {
        return Ok(Err(ProjectDbError::InvalidInput));
    }
    let trimmed = name.map(str::trim);
    if let Some(name) = trimmed {
        if !label_name_is_valid(name) {
            return Ok(Err(ProjectDbError::InvalidInput));
        }
    }
    if let Some(color) = color {
        if !label_color_is_valid(color) {
            return Ok(Err(ProjectDbError::InvalidInput));
        }
    }
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    match require_project_edit(&mut tx, workspace_id, actor_user_id, session_id, project_id).await?
    {
        Ok(()) => {}
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    }
    let exists: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM fvoci.labels WHERE workspace_id = $1 AND project_id = $2 AND id = $3",
    )
    .bind(workspace_id)
    .bind(project_id)
    .bind(label_id)
    .fetch_optional(&mut *tx)
    .await?;
    if exists.is_none() {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::LabelNotFound));
    }
    sqlx::query(
        r#"
        UPDATE fvoci.labels
        SET name = COALESCE($4, name),
            color = COALESCE($5, color),
            updated_at = now()
        WHERE workspace_id = $1 AND project_id = $2 AND id = $3
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .bind(label_id)
    .bind(trimmed)
    .bind(color)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Ok(()))
}

pub async fn purge_label(
    pool: &PgPool,
    workspace_id: Uuid,
    project_id: Uuid,
    label_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<Result<(), ProjectDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    match require_project_edit(&mut tx, workspace_id, actor_user_id, session_id, project_id).await?
    {
        Ok(()) => {}
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    }
    let deleted: Option<(Uuid,)> = sqlx::query_as(
        r#"
        DELETE FROM fvoci.labels
        WHERE workspace_id = $1 AND project_id = $2 AND id = $3
        RETURNING id
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .bind(label_id)
    .fetch_optional(&mut *tx)
    .await?;
    if deleted.is_none() {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::LabelNotFound));
    }
    tx.commit().await?;
    Ok(Ok(()))
}

/// Task list `labelId` filter: the label must belong to the listed project (the
/// caller has already been checked for view access to that project).
pub async fn project_label_exists(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    project_id: Uuid,
    label_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let exists: (bool,) = sqlx::query_as(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM fvoci.labels
            WHERE workspace_id = $1 AND project_id = $2 AND id = $3
        )
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .bind(label_id)
    .fetch_one(&mut **tx)
    .await?;
    Ok(exists.0)
}

pub async fn assignee_filter_member_exists(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let exists: (bool,) = sqlx::query_as(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM fvoci.memberships m
            INNER JOIN fvoci.users u ON u.id = m.user_id
            WHERE m.workspace_id = $1
              AND u.id = $2
              AND u.deleted_at IS NULL
        )
        "#,
    )
    .bind(workspace_id)
    .bind(user_id)
    .fetch_one(&mut **tx)
    .await?;
    Ok(exists.0)
}
