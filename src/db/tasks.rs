use chrono::{DateTime, NaiveDate, Utc};
use serde_json::{json, Value};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::db::context::{lock_membership_users, recheck_session, session_is_live, set_tenant};
use crate::db::documents::empty_document_json;
use crate::db::identity::{append_audit, append_event, AuditAppend, EventAppend};
use crate::db::projects::{lock_project, project_permission, ProjectDbError};
use crate::projects::ProjectPermission;
use crate::db::workspace::WorkspaceRole;

#[derive(Debug, Clone)]
pub struct TaskMetaRow {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub project_id: Uuid,
    pub number: i32,
    pub title: String,
    pub task_type: String,
    pub priority: String,
    pub status_id: Uuid,
    pub start_date: Option<NaiveDate>,
    pub due_date: Option<NaiveDate>,
    pub due_at: Option<DateTime<Utc>>,
    pub estimate: Option<f64>,
    pub parent_id: Option<Uuid>,
    pub milestone_id: Option<Uuid>,
    pub recurrence: Option<Value>,
    pub archived_at: Option<DateTime<Utc>>,
    pub created_by: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct TaskDetailRow {
    pub meta: TaskMetaRow,
    pub content_json: Value,
    pub can_edit: bool,
}

pub struct CreateTaskInput<'a> {
    pub title: &'a str,
    pub task_type: &'a str,
    pub priority: &'a str,
    pub status_id: Option<Uuid>,
    pub start_date: Option<NaiveDate>,
    pub due_date: Option<NaiveDate>,
    pub parent_id: Option<Uuid>,
    pub milestone_id: Option<Uuid>,
    pub recurrence: Option<Value>,
}

struct TaskChangeRecord<'a> {
    workspace_id: Uuid,
    actor_user_id: Uuid,
    verb: &'a str,
    target_type: &'a str,
    target_id: Uuid,
    payload: Value,
    client_ip: Option<&'a str>,
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

async fn record_task_event_and_audit(
    tx: &mut Transaction<'_, Postgres>,
    change: TaskChangeRecord<'_>,
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

async fn default_backlog_status(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    project_id: Uuid,
) -> Result<Option<Uuid>, sqlx::Error> {
    let row: Option<(Uuid,)> = sqlx::query_as(
        r#"
        SELECT id FROM fvoci.statuses
        WHERE workspace_id = $1 AND project_id = $2 AND category = 'backlog'
        ORDER BY sort_key COLLATE "C"
        LIMIT 1
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .fetch_optional(&mut **tx)
    .await?;
    if row.is_some() {
        return Ok(row.map(|(id,)| id));
    }
    let fallback: Option<(Uuid,)> = sqlx::query_as(
        r#"
        SELECT id FROM fvoci.statuses
        WHERE workspace_id = $1 AND project_id = $2
        ORDER BY sort_key COLLATE "C"
        LIMIT 1
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(fallback.map(|(id,)| id))
}

fn row_to_meta(
    workspace_id: Uuid,
    row: (
        Uuid,
        Uuid,
        i32,
        String,
        String,
        String,
        Uuid,
        Option<NaiveDate>,
        Option<NaiveDate>,
        Option<DateTime<Utc>>,
        Option<Uuid>,
        Option<Uuid>,
        Option<DateTime<Utc>>,
        Uuid,
        DateTime<Utc>,
        DateTime<Utc>,
    ),
) -> TaskMetaRow {
    TaskMetaRow {
        id: row.0,
        workspace_id,
        project_id: row.1,
        number: row.2,
        title: row.3,
        task_type: row.4,
        priority: row.5,
        status_id: row.6,
        start_date: row.7,
        due_date: row.8,
        due_at: row.9,
        estimate: None,
        parent_id: row.10,
        milestone_id: row.11,
        recurrence: None,
        archived_at: row.12,
        created_by: row.13,
        created_at: row.14,
        updated_at: row.15,
    }
}

type TaskRowTuple = (
    Uuid,
    Uuid,
    i32,
    String,
    String,
    String,
    Uuid,
    Option<NaiveDate>,
    Option<NaiveDate>,
    Option<DateTime<Utc>>,
    Option<Uuid>,
    Option<Uuid>,
    Option<DateTime<Utc>>,
    Uuid,
    DateTime<Utc>,
    DateTime<Utc>,
);

pub async fn create_task(
    pool: &PgPool,
    workspace_id: Uuid,
    project_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    input: CreateTaskInput<'_>,
    client_ip: Option<&str>,
) -> Result<Result<TaskMetaRow, ProjectDbError>, sqlx::Error> {
    let task_id = Uuid::now_v7();
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
    let locked = lock_project(&mut tx, workspace_id, project_id).await?;
    let Some(locked) = locked else {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    };
    if locked.status == "archived" {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::Archived));
    }
    let permission = project_permission(&mut tx, workspace_id, actor_user_id, &locked).await?;
    if !permission.at_least(ProjectPermission::Edit) {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }

    if input.task_type == "subtask" && input.parent_id.is_none() {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::Conflict));
    }

    if let Some(parent_id) = input.parent_id {
        let parent: Option<(Uuid, Option<DateTime<Utc>>)> = sqlx::query_as(
            r#"
            SELECT project_id, deleted_at
            FROM fvoci.tasks
            WHERE workspace_id = $1 AND id = $2
            "#,
        )
        .bind(workspace_id)
        .bind(parent_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((parent_project, deleted)) = parent else {
            tx.rollback().await?;
            return Ok(Err(ProjectDbError::NotFound));
        };
        if deleted.is_some() || parent_project != project_id {
            tx.rollback().await?;
            return Ok(Err(ProjectDbError::NotFound));
        }
    }

    let status_id = if let Some(status_id) = input.status_id {
        let valid: Option<(Uuid,)> = sqlx::query_as(
            r#"
            SELECT id FROM fvoci.statuses
            WHERE workspace_id = $1 AND project_id = $2 AND id = $3
            "#,
        )
        .bind(workspace_id)
        .bind(project_id)
        .bind(status_id)
        .fetch_optional(&mut *tx)
        .await?;
        if valid.is_none() {
            tx.rollback().await?;
            return Ok(Err(ProjectDbError::NotFound));
        }
        status_id
    } else {
        let Some(status_id) = default_backlog_status(&mut tx, workspace_id, project_id).await?
        else {
            tx.rollback().await?;
            return Ok(Err(ProjectDbError::NotFound));
        };
        status_id
    };

    let number: (i32,) = sqlx::query_as(
        r#"
        UPDATE fvoci.projects
        SET next_number = next_number + 1, updated_at = now()
        WHERE workspace_id = $1 AND id = $2
        RETURNING next_number - 1
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .fetch_one(&mut *tx)
    .await?;

    let row = sqlx::query_as::<_, TaskRowTuple>(
        r#"
        INSERT INTO fvoci.tasks (
            id, workspace_id, project_id, number, title, type, priority, status_id,
            start_date, due_date, parent_id, milestone_id, recurrence, content_json, created_by
        ) VALUES (
            $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15
        )
        RETURNING id, project_id, number, title, type AS task_type, priority, status_id,
                  start_date, due_date, due_at, parent_id, milestone_id, archived_at,
                  created_by, created_at, updated_at
        "#,
    )
    .bind(task_id)
    .bind(workspace_id)
    .bind(project_id)
    .bind(number.0)
    .bind(input.title.trim())
    .bind(input.task_type)
    .bind(input.priority)
    .bind(status_id)
    .bind(input.start_date)
    .bind(input.due_date)
    .bind(input.parent_id)
    .bind(input.milestone_id)
    .bind(input.recurrence)
    .bind(empty_document_json())
    .bind(actor_user_id)
    .fetch_one(&mut *tx)
    .await?;

    record_task_event_and_audit(
        &mut tx,
        TaskChangeRecord {
            workspace_id,
            actor_user_id,
            verb: "task.created",
            target_type: "task",
            target_id: task_id,
            payload: json!({
                "taskId": task_id.to_string(),
                "projectId": project_id.to_string(),
                "title": input.title.trim(),
            }),
            client_ip,
        },
    )
    .await?;

    tx.commit().await?;
    Ok(Ok(row_to_meta(workspace_id, row)))
}

pub async fn get_task(
    pool: &PgPool,
    workspace_id: Uuid,
    task_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<Result<TaskDetailRow, ProjectDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !session_is_live(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::Forbidden));
    }
    let row = sqlx::query_as::<_, TaskRowTuple>(
        r#"
        SELECT t.id, t.project_id, t.number, t.title, t.type AS task_type, t.priority, t.status_id,
               t.start_date, t.due_date, t.due_at, t.parent_id, t.milestone_id, t.archived_at,
               t.created_by, t.created_at, t.updated_at
        FROM fvoci.tasks t
        WHERE t.workspace_id = $1 AND t.id = $2 AND t.deleted_at IS NULL
        "#,
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(tuple) = row else {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    };
    let content_json: Value = sqlx::query_scalar(
        "SELECT content_json FROM fvoci.tasks WHERE workspace_id = $1 AND id = $2",
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_one(&mut *tx)
    .await?;
    let project_id = tuple.1;
    let locked = lock_project(&mut tx, workspace_id, project_id).await?;
    let Some(locked) = locked else {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    };
    let permission = project_permission(&mut tx, workspace_id, actor_user_id, &locked).await?;
    if !permission.at_least(ProjectPermission::View) {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }
    tx.commit().await?;
    Ok(Ok(TaskDetailRow {
        meta: row_to_meta(workspace_id, tuple),
        content_json,
        can_edit: permission.at_least(ProjectPermission::Edit),
    }))
}

pub async fn list_project_tasks(
    pool: &PgPool,
    workspace_id: Uuid,
    project_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<Result<Vec<TaskMetaRow>, ProjectDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !session_is_live(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::Forbidden));
    }
    let locked = lock_project(&mut tx, workspace_id, project_id).await?;
    let Some(locked) = locked else {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    };
    if !project_permission(&mut tx, workspace_id, actor_user_id, &locked)
        .await?
        .at_least(ProjectPermission::View)
    {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }
    let rows = sqlx::query_as::<_, TaskRowTuple>(
        r#"
        SELECT id, project_id, number, title, type AS task_type, priority, status_id, start_date,
               due_date, due_at, parent_id, milestone_id, archived_at, created_by, created_at,
               updated_at
        FROM fvoci.tasks
        WHERE workspace_id = $1 AND project_id = $2 AND deleted_at IS NULL
        ORDER BY number ASC
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Ok(
        rows.into_iter()
            .map(|row| row_to_meta(workspace_id, row))
            .collect(),
    ))
}
