use chrono::{DateTime, NaiveDate, Utc};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::db::context::{lock_membership_users, recheck_session, session_is_live, set_tenant};
use crate::db::documents::between;
use crate::db::projects::{lock_project, project_permission, visible_project_sql, ProjectDbError};
use crate::db::workspace::WorkspaceRole;
use crate::projects::ProjectPermission;

pub const MILESTONE_NAME_MAX: usize = 200;

#[derive(Debug, Clone)]
pub struct MilestoneRow {
    pub id: Uuid,
    pub project_id: Uuid,
    pub name: String,
    pub due_date: Option<NaiveDate>,
    pub sort_key: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub fn milestone_name_is_valid(name: &str) -> bool {
    let trimmed = name.trim();
    !trimmed.is_empty() && trimmed.chars().count() <= MILESTONE_NAME_MAX
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

fn map_milestone_row(
    id: Uuid,
    project_id: Uuid,
    name: String,
    due_date: Option<NaiveDate>,
    sort_key: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
) -> MilestoneRow {
    MilestoneRow {
        id,
        project_id,
        name,
        due_date,
        sort_key,
        created_at,
        updated_at,
    }
}

type MilestoneSqlRow = (
    Uuid,
    Uuid,
    String,
    Option<NaiveDate>,
    String,
    DateTime<Utc>,
    DateTime<Utc>,
);

async fn list_by_project_tx(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    project_id: Uuid,
) -> Result<Vec<MilestoneRow>, sqlx::Error> {
    let rows = sqlx::query_as::<_, MilestoneSqlRow>(
        r#"
        SELECT id, project_id, name, due_date, sort_key, created_at, updated_at
        FROM fvoci.milestones
        WHERE workspace_id = $1 AND project_id = $2
        ORDER BY sort_key COLLATE "C", id
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(id, project_id, name, due_date, sort_key, created_at, updated_at)| {
                map_milestone_row(
                    id, project_id, name, due_date, sort_key, created_at, updated_at,
                )
            },
        )
        .collect())
}

pub async fn list_project_milestones(
    pool: &PgPool,
    workspace_id: Uuid,
    project_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<Result<Vec<MilestoneRow>, ProjectDbError>, sqlx::Error> {
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
    let rows = list_by_project_tx(&mut tx, workspace_id, project_id).await?;
    tx.commit().await?;
    Ok(Ok(rows))
}

pub async fn create_milestone(
    pool: &PgPool,
    workspace_id: Uuid,
    project_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    name: &str,
    due_date: Option<NaiveDate>,
) -> Result<Result<MilestoneRow, ProjectDbError>, sqlx::Error> {
    if !milestone_name_is_valid(name) {
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
    let siblings = list_by_project_tx(&mut tx, workspace_id, project_id).await?;
    let last = siblings.last().map(|row| row.sort_key.as_str());
    let sort_key = match between(last, None) {
        Ok(key) => key,
        Err(_) => {
            tx.rollback().await?;
            return Ok(Err(ProjectDbError::InvalidAnchor));
        }
    };
    let id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO fvoci.milestones (id, workspace_id, project_id, name, due_date, sort_key)
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(id)
    .bind(workspace_id)
    .bind(project_id)
    .bind(name.trim())
    .bind(due_date)
    .bind(&sort_key)
    .execute(&mut *tx)
    .await?;
    let rows = list_by_project_tx(&mut tx, workspace_id, project_id).await?;
    let Some(created) = rows.into_iter().find(|row| row.id == id) else {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    };
    tx.commit().await?;
    Ok(Ok(created))
}

#[allow(clippy::too_many_arguments)]
pub async fn update_milestone(
    pool: &PgPool,
    workspace_id: Uuid,
    project_id: Uuid,
    milestone_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    name: Option<&str>,
    due_date: Option<Option<NaiveDate>>,
) -> Result<Result<(), ProjectDbError>, sqlx::Error> {
    if name.is_none() && due_date.is_none() {
        return Ok(Err(ProjectDbError::InvalidInput));
    }
    let trimmed = name.map(str::trim);
    if let Some(name) = trimmed {
        if !milestone_name_is_valid(name) {
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
        "SELECT id FROM fvoci.milestones WHERE workspace_id = $1 AND project_id = $2 AND id = $3",
    )
    .bind(workspace_id)
    .bind(project_id)
    .bind(milestone_id)
    .fetch_optional(&mut *tx)
    .await?;
    if exists.is_none() {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::MilestoneNotFound));
    }
    sqlx::query(
        r#"
        UPDATE fvoci.milestones
        SET name = COALESCE($4, name),
            due_date = CASE WHEN $5 THEN $6 ELSE due_date END,
            updated_at = now()
        WHERE workspace_id = $1 AND project_id = $2 AND id = $3
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .bind(milestone_id)
    .bind(trimmed)
    .bind(due_date.is_some())
    .bind(due_date.flatten())
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Ok(()))
}

pub async fn purge_milestone(
    pool: &PgPool,
    workspace_id: Uuid,
    project_id: Uuid,
    milestone_id: Uuid,
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
    let exists: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM fvoci.milestones WHERE workspace_id = $1 AND project_id = $2 AND id = $3",
    )
    .bind(workspace_id)
    .bind(project_id)
    .bind(milestone_id)
    .fetch_optional(&mut *tx)
    .await?;
    if exists.is_none() {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::MilestoneNotFound));
    }
    sqlx::query(
        r#"
        UPDATE fvoci.tasks
        SET milestone_id = NULL, updated_at = now()
        WHERE workspace_id = $1 AND milestone_id = $2
        "#,
    )
    .bind(workspace_id)
    .bind(milestone_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "DELETE FROM fvoci.milestones WHERE workspace_id = $1 AND project_id = $2 AND id = $3",
    )
    .bind(workspace_id)
    .bind(project_id)
    .bind(milestone_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Ok(()))
}

pub async fn project_milestone_exists(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    project_id: Uuid,
    milestone_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let exists: (bool,) = sqlx::query_as(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM fvoci.milestones
            WHERE workspace_id = $1 AND project_id = $2 AND id = $3
        )
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .bind(milestone_id)
    .fetch_one(&mut **tx)
    .await?;
    Ok(exists.0)
}

pub async fn milestone_is_visible(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    milestone_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let Some(role) = membership_role(tx, workspace_id, actor_user_id).await? else {
        return Ok(false);
    };
    let guest = role == WorkspaceRole::Guest;
    let visible = visible_project_sql("p", 3, 4);
    let sql = format!(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM fvoci.milestones m
            INNER JOIN fvoci.projects p
                ON p.workspace_id = m.workspace_id AND p.id = m.project_id
            WHERE m.workspace_id = $1
              AND m.id = $2
              AND p.deleted_at IS NULL
              AND {visible}
        )
        "#
    );
    let exists: (bool,) = sqlx::query_as(&sql)
        .bind(workspace_id)
        .bind(milestone_id)
        .bind(guest)
        .bind(actor_user_id)
        .fetch_one(&mut **tx)
        .await?;
    Ok(exists.0)
}
