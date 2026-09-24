use chrono::{DateTime, NaiveDate, Utc};
use serde_json::{json, Value};
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

use crate::db::context::{lock_membership_users, recheck_session, session_is_live, set_tenant};
use crate::db::documents::{empty_document_json, DOCUMENT_SCHEMA_VERSION};
use crate::db::identity::{append_audit, append_event, AuditAppend, EventAppend};
use crate::db::projects::{lock_project, project_permission, ProjectDbError};
use crate::projects::ProjectPermission;
use crate::tasks::list_query::{
    cursor_key_for_row, encode_cursor, filter_fingerprint, ParsedTaskListQuery, SortDirection,
    SortField, TaskListCursor, ViewSort,
};

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
    pub sort_key: String,
    pub schema_version: i32,
    pub version: i32,
    pub archived_at: Option<DateTime<Utc>>,
    pub created_by: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct TaskParentRow {
    pub id: Uuid,
    pub title: String,
    pub task_type: String,
    pub number: i32,
}

#[derive(Debug, Clone)]
pub struct TaskChildRow {
    pub id: Uuid,
    pub number: i32,
    pub title: String,
    pub task_type: String,
    pub status_id: Uuid,
}

#[derive(Debug, Clone, Copy)]
pub struct TaskChildProgress {
    pub done: i64,
    pub total: i64,
}

#[derive(Debug, Clone)]
pub struct TaskDetailRow {
    pub meta: TaskMetaRow,
    pub content_json: Value,
    pub can_edit: bool,
    pub parent: Option<TaskParentRow>,
    pub children: Vec<TaskChildRow>,
    pub child_progress: Option<TaskChildProgress>,
}

pub struct TaskListPage {
    pub items: Vec<TaskMetaRow>,
    pub status_counts: Vec<(Uuid, i64)>,
    pub next_cursor: Option<String>,
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

fn violates_task_hierarchy(child_type: &str, parent_type: &str) -> bool {
    if child_type == "subtask" {
        !matches!(parent_type, "task" | "bug" | "story")
    } else if child_type == "epic" {
        true
    } else {
        parent_type != "epic"
    }
}

#[derive(Debug, Clone)]
struct TaskRowRecord {
    id: Uuid,
    project_id: Uuid,
    number: i32,
    title: String,
    task_type: String,
    priority: String,
    status_id: Uuid,
    start_date: Option<NaiveDate>,
    due_date: Option<NaiveDate>,
    due_at: Option<DateTime<Utc>>,
    estimate: Option<f64>,
    parent_id: Option<Uuid>,
    milestone_id: Option<Uuid>,
    sort_key: String,
    schema_version: i32,
    version: i32,
    archived_at: Option<DateTime<Utc>>,
    created_by: Uuid,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

fn map_task_row(row: &sqlx::postgres::PgRow) -> Result<TaskRowRecord, sqlx::Error> {
    Ok(TaskRowRecord {
        id: row.try_get("id")?,
        project_id: row.try_get("project_id")?,
        number: row.try_get("number")?,
        title: row.try_get("title")?,
        task_type: row.try_get("task_type")?,
        priority: row.try_get("priority")?,
        status_id: row.try_get("status_id")?,
        start_date: row.try_get("start_date")?,
        due_date: row.try_get("due_date")?,
        due_at: row.try_get("due_at")?,
        estimate: row.try_get("estimate")?,
        parent_id: row.try_get("parent_id")?,
        milestone_id: row.try_get("milestone_id")?,
        sort_key: row.try_get("sort_key")?,
        schema_version: row.try_get("schema_version")?,
        version: row.try_get("version")?,
        archived_at: row.try_get("archived_at")?,
        created_by: row.try_get("created_by")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn row_to_meta(workspace_id: Uuid, row: TaskRowRecord, recurrence: Option<Value>) -> TaskMetaRow {
    TaskMetaRow {
        id: row.id,
        workspace_id,
        project_id: row.project_id,
        number: row.number,
        title: row.title,
        task_type: row.task_type,
        priority: row.priority,
        status_id: row.status_id,
        start_date: row.start_date,
        due_date: row.due_date,
        due_at: row.due_at,
        estimate: row.estimate,
        parent_id: row.parent_id,
        milestone_id: row.milestone_id,
        sort_key: row.sort_key,
        schema_version: row.schema_version,
        version: row.version,
        recurrence,
        archived_at: row.archived_at,
        created_by: row.created_by,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

async fn load_task_recurrence(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    task_id: Uuid,
) -> Result<Option<Value>, sqlx::Error> {
    sqlx::query_scalar("SELECT recurrence FROM fvoci.tasks WHERE workspace_id = $1 AND id = $2")
        .bind(workspace_id)
        .bind(task_id)
        .fetch_one(&mut **tx)
        .await
}

async fn load_task_parent(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    project_id: Uuid,
    parent_id: Uuid,
) -> Result<Option<TaskParentRow>, sqlx::Error> {
    let row: Option<(Uuid, String, String, i32)> = sqlx::query_as(
        r#"
        SELECT id, title, type, number
        FROM fvoci.tasks
        WHERE workspace_id = $1 AND id = $2 AND project_id = $3 AND deleted_at IS NULL
        "#,
    )
    .bind(workspace_id)
    .bind(parent_id)
    .bind(project_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.map(|(id, title, task_type, number)| TaskParentRow {
        id,
        title,
        task_type,
        number,
    }))
}

async fn load_task_children(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    parent_id: Uuid,
) -> Result<Vec<TaskChildRow>, sqlx::Error> {
    let rows = sqlx::query_as::<_, (Uuid, i32, String, String, Uuid)>(
        r#"
        SELECT id, number, title, type, status_id
        FROM fvoci.tasks
        WHERE workspace_id = $1 AND parent_id = $2
          AND deleted_at IS NULL AND archived_at IS NULL
        ORDER BY created_at DESC, id DESC
        "#,
    )
    .bind(workspace_id)
    .bind(parent_id)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, number, title, task_type, status_id)| TaskChildRow {
            id,
            number,
            title,
            task_type,
            status_id,
        })
        .collect())
}

async fn load_task_child_progress(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    parent_id: Uuid,
) -> Result<TaskChildProgress, sqlx::Error> {
    let row: (i64, i64) = sqlx::query_as(
        r#"
        SELECT
            count(*) FILTER (WHERE s.category = 'done')::bigint,
            count(*)::bigint
        FROM fvoci.tasks t
        INNER JOIN fvoci.statuses s
          ON s.workspace_id = t.workspace_id AND s.id = t.status_id
        WHERE t.workspace_id = $1 AND t.parent_id = $2
          AND t.deleted_at IS NULL AND t.archived_at IS NULL
          AND s.category <> 'canceled'
        "#,
    )
    .bind(workspace_id)
    .bind(parent_id)
    .fetch_one(&mut **tx)
    .await?;
    Ok(TaskChildProgress {
        done: row.0,
        total: row.1,
    })
}

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
        let parent: Option<(Uuid, Option<DateTime<Utc>>, String)> = sqlx::query_as(
            r#"
            SELECT project_id, deleted_at, type
            FROM fvoci.tasks
            WHERE workspace_id = $1 AND id = $2
            "#,
        )
        .bind(workspace_id)
        .bind(parent_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((parent_project, deleted, parent_type)) = parent else {
            tx.rollback().await?;
            return Ok(Err(ProjectDbError::NotFound));
        };
        if deleted.is_some() || parent_project != project_id {
            tx.rollback().await?;
            return Ok(Err(ProjectDbError::NotFound));
        }
        if violates_task_hierarchy(input.task_type, &parent_type) {
            tx.rollback().await?;
            return Ok(Err(ProjectDbError::Conflict));
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

    let (sort_key,): (String,) = sqlx::query_as(
        r#"
        SELECT sort_key FROM fvoci.statuses
        WHERE workspace_id = $1 AND project_id = $2 AND id = $3
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .bind(status_id)
    .fetch_one(&mut *tx)
    .await?;

    let row = map_task_row(
        &sqlx::query(
            r#"
        INSERT INTO fvoci.tasks (
            id, workspace_id, project_id, number, title, type, priority, status_id,
            start_date, due_date, parent_id, milestone_id, recurrence, sort_key,
            schema_version, content_json, created_by
        ) VALUES (
            $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17
        )
        RETURNING id, project_id, number, title, type AS task_type, priority, status_id,
                  start_date, due_date, due_at, estimate, parent_id, milestone_id, sort_key,
                  schema_version, version, archived_at, created_by, created_at, updated_at
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
        .bind(input.recurrence.clone())
        .bind(&sort_key)
        .bind(DOCUMENT_SCHEMA_VERSION)
        .bind(empty_document_json())
        .bind(actor_user_id)
        .fetch_one(&mut *tx)
        .await?,
    )?;

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
    Ok(Ok(row_to_meta(workspace_id, row, input.recurrence.clone())))
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
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }
    let Some(raw) = sqlx::query(
        r#"
        SELECT t.id, t.project_id, t.number, t.title, t.type AS task_type, t.priority, t.status_id,
               t.start_date, t.due_date, t.due_at, t.estimate, t.parent_id, t.milestone_id,
               t.sort_key, t.schema_version, t.version, t.archived_at, t.created_by, t.created_at,
               t.updated_at
        FROM fvoci.tasks t
        WHERE t.workspace_id = $1 AND t.id = $2 AND t.deleted_at IS NULL
        "#,
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_optional(&mut *tx)
    .await?
    else {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    };
    let task_row = map_task_row(&raw)?;
    let content_json: Value = sqlx::query_scalar(
        "SELECT content_json FROM fvoci.tasks WHERE workspace_id = $1 AND id = $2",
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_one(&mut *tx)
    .await?;
    let project_id = task_row.project_id;
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

    let meta = row_to_meta(
        workspace_id,
        task_row,
        load_task_recurrence(&mut tx, workspace_id, task_id).await?,
    );
    let parent = if let Some(parent_id) = meta.parent_id {
        load_task_parent(&mut tx, workspace_id, project_id, parent_id).await?
    } else {
        None
    };
    let children = load_task_children(&mut tx, workspace_id, task_id).await?;
    let child_progress = if meta.task_type == "subtask" {
        None
    } else {
        Some(load_task_child_progress(&mut tx, workspace_id, task_id).await?)
    };

    tx.commit().await?;
    Ok(Ok(TaskDetailRow {
        meta,
        content_json,
        can_edit: permission.at_least(ProjectPermission::Edit),
        parent,
        children,
        child_progress,
    }))
}

type TaskListCursorAnchor = (
    DateTime<Utc>,
    DateTime<Utc>,
    Uuid,
    i32,
    String,
    String,
    String,
    Uuid,
    Option<NaiveDate>,
);

pub async fn list_project_tasks(
    pool: &PgPool,
    workspace_id: Uuid,
    project_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    query: &ParsedTaskListQuery,
) -> Result<Result<TaskListPage, ProjectDbError>, sqlx::Error> {
    let fingerprint = filter_fingerprint(workspace_id, project_id, query);
    if let Some(cursor) = &query.cursor {
        if cursor.f != fingerprint {
            return Ok(Err(ProjectDbError::InvalidCursor));
        }
    }
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

    let mut binds: Vec<String> = Vec::new();
    let mut conditions = vec![
        "t.workspace_id = $1".to_string(),
        "t.project_id = $2".to_string(),
        "t.deleted_at IS NULL".to_string(),
    ];
    if query.archived {
        conditions.push("t.archived_at IS NOT NULL".to_string());
    } else {
        conditions.push("t.archived_at IS NULL".to_string());
    }
    if let Some(task_type) = &query.view.filters.task_type {
        let idx = binds.len() + 3;
        binds.push(task_type.clone());
        conditions.push(format!("t.type = ${idx}"));
    }
    if let Some(status_id) = query.view.filters.status_id {
        let idx = binds.len() + 3;
        binds.push(status_id.to_string());
        conditions.push(format!("t.status_id = ${idx}::uuid"));
    }
    if let Some(priority) = &query.view.filters.priority {
        let idx = binds.len() + 3;
        binds.push(priority.clone());
        conditions.push(format!("t.priority = ${idx}"));
    }
    if query.view.filters.open_only {
        conditions.push(
            "EXISTS (
                SELECT 1 FROM fvoci.statuses s_open
                WHERE s_open.workspace_id = t.workspace_id
                  AND s_open.project_id = t.project_id
                  AND s_open.id = t.status_id
                  AND s_open.category NOT IN ('done', 'canceled')
            )"
            .to_string(),
        );
    }
    if let Some(title) = &query.view.filters.title {
        let idx = binds.len() + 3;
        binds.push(format!("%{title}%"));
        conditions.push(format!("t.title ILIKE ${idx}"));
    }
    if let (Some(from), Some(to)) = (query.from, query.to) {
        let from_idx = binds.len() + 3;
        binds.push(from.to_string());
        let to_idx = binds.len() + 3;
        binds.push(to.to_string());
        conditions.push(format!(
            "LEAST(t.start_date, COALESCE(t.due_date, (t.due_at AT TIME ZONE 'UTC')::date)) <= ${to_idx}::date"
        ));
        conditions.push(format!(
            "GREATEST(t.start_date, COALESCE(t.due_date, (t.due_at AT TIME ZONE 'UTC')::date)) >= ${from_idx}::date"
        ));
    }

    let sort = effective_sort(&query.view.sort);
    if let Some(cursor) = &query.cursor {
        let anchor: Option<TaskListCursorAnchor> = sqlx::query_as(
                r#"
                SELECT t.created_at, t.updated_at, t.id, t.number, t.title, t.sort_key, t.priority, t.status_id, t.due_date
                FROM fvoci.tasks t
                WHERE t.workspace_id = $1 AND t.project_id = $2 AND t.id = $3 AND t.deleted_at IS NULL
                "#,
            )
            .bind(workspace_id)
            .bind(project_id)
            .bind(cursor.id)
            .fetch_optional(&mut *tx)
            .await?;
        let Some((
            created_at,
            updated_at,
            id,
            number,
            title,
            sort_key,
            priority,
            status_id,
            due_date,
        )) = anchor
        else {
            tx.rollback().await?;
            return Ok(Err(ProjectDbError::InvalidCursor));
        };
        let key = cursor_key_for_row(
            &sort, created_at, updated_at, number, &title, &sort_key, &priority, status_id,
            due_date,
        );
        if key != cursor.key {
            tx.rollback().await?;
            return Ok(Err(ProjectDbError::InvalidCursor));
        }
        let bind_start = binds.len() + 3;
        match sort[0].field {
            SortField::Created => {
                conditions.push(cursor_clause(&sort, bind_start));
                binds.push(created_at.to_rfc3339());
                binds.push(id.to_string());
            }
            SortField::Updated => {
                conditions.push(cursor_clause(&sort, bind_start));
                binds.push(updated_at.to_rfc3339());
                binds.push(id.to_string());
            }
            SortField::Number => {
                conditions.push(cursor_clause(&sort, bind_start));
                binds.push(number.to_string());
                binds.push(id.to_string());
            }
            SortField::Title => {
                conditions.push(cursor_clause(&sort, bind_start));
                binds.push(title);
                binds.push(id.to_string());
            }
            SortField::Rank => {
                conditions.push(cursor_clause(&sort, bind_start));
                binds.push(sort_key);
                binds.push(id.to_string());
            }
            SortField::Priority => {
                conditions.push(cursor_clause(&sort, bind_start));
                binds.push(priority);
                binds.push(id.to_string());
            }
            SortField::Status => {
                conditions.push(cursor_clause(&sort, bind_start));
                binds.push(status_id.to_string());
                binds.push(id.to_string());
            }
            SortField::Due => {
                conditions.push(cursor_clause(&sort, bind_start));
                binds.push(
                    due_date
                        .map(|date| date.to_string())
                        .unwrap_or_else(|| "null".to_string()),
                );
                binds.push(id.to_string());
            }
        }
    }

    let where_sql = conditions.join(" AND ");
    let order_sql = order_clause(&sort);
    let limit = query.limit + 1;
    let list_sql = format!(
        r#"
        SELECT t.id, t.project_id, t.number, t.title, t.type AS task_type, t.priority, t.status_id,
               t.start_date, t.due_date, t.due_at, t.estimate, t.parent_id, t.milestone_id,
               t.sort_key, t.schema_version, t.version, t.archived_at, t.created_by, t.created_at,
               t.updated_at, t.recurrence
        FROM fvoci.tasks t
        WHERE {where_sql}
        ORDER BY {order_sql}
        LIMIT {limit}
        "#
    );
    let mut list_query = sqlx::query(&list_sql).bind(workspace_id).bind(project_id);
    for value in &binds {
        list_query = list_query.bind(value);
    }
    let rows = list_query.fetch_all(&mut *tx).await?;

    let status_counts = sqlx::query_as::<_, (Uuid, i64)>(
        r#"
        SELECT t.status_id, count(*)
        FROM fvoci.tasks t
        WHERE t.workspace_id = $1 AND t.project_id = $2 AND t.deleted_at IS NULL
          AND (($3::bool AND t.archived_at IS NOT NULL) OR (NOT $3::bool AND t.archived_at IS NULL))
        GROUP BY t.status_id
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .bind(query.archived)
    .fetch_all(&mut *tx)
    .await?;

    let mut items = Vec::new();
    for row in rows.iter().take(query.limit as usize) {
        let record = map_task_row(row)?;
        let recurrence = row.try_get::<Option<Value>, _>("recurrence").ok().flatten();
        items.push(row_to_meta(workspace_id, record, recurrence));
    }
    let next_cursor = if rows.len() as i32 > query.limit {
        let last = &rows[(query.limit - 1) as usize];
        let record = map_task_row(last)?;
        let created_at: DateTime<Utc> = last.try_get("created_at")?;
        let updated_at: DateTime<Utc> = last.try_get("updated_at")?;
        let due_date: Option<NaiveDate> = last.try_get("due_date")?;
        let key = cursor_key_for_row(
            &sort,
            created_at,
            updated_at,
            record.number,
            &record.title,
            &record.sort_key,
            &record.priority,
            record.status_id,
            due_date,
        );
        Some(encode_cursor(&TaskListCursor {
            id: record.id,
            key,
            f: fingerprint,
            as_of: query.as_of,
        }))
    } else {
        None
    };

    tx.commit().await?;
    Ok(Ok(TaskListPage {
        items,
        status_counts,
        next_cursor,
    }))
}

fn effective_sort(sort: &[ViewSort]) -> Vec<ViewSort> {
    if sort.is_empty() {
        vec![ViewSort {
            field: SortField::Created,
            direction: SortDirection::Desc,
        }]
    } else {
        sort.to_vec()
    }
}

fn order_clause(sort: &[ViewSort]) -> String {
    let mut parts = Vec::new();
    for entry in sort {
        let column = match entry.field {
            SortField::Priority => "t.priority",
            SortField::Due => "COALESCE(t.due_date, (t.due_at AT TIME ZONE 'UTC')::date)",
            SortField::Updated => "t.updated_at",
            SortField::Created => "t.created_at",
            SortField::Rank => "t.sort_key COLLATE \"C\"",
            SortField::Title => "t.title COLLATE \"C\"",
            SortField::Status => "t.status_id",
            SortField::Number => "t.number",
        };
        let dir = if entry.direction == SortDirection::Asc {
            "ASC"
        } else {
            "DESC"
        };
        parts.push(format!("{column} {dir} NULLS LAST"));
    }
    parts.push("t.id DESC".to_string());
    parts.join(", ")
}

fn cursor_clause(sort: &[ViewSort], bind_index: usize) -> String {
    let primary = &sort[0];
    let op = if primary.direction == SortDirection::Asc {
        ">"
    } else {
        "<"
    };
    match primary.field {
        SortField::Created | SortField::Updated => {
            let column = if primary.field == SortField::Created {
                "t.created_at"
            } else {
                "t.updated_at"
            };
            format!(
                "(({column}, t.id) {op} (${bind_index}::timestamptz, ${}::uuid))",
                bind_index + 1
            )
        }
        SortField::Number => {
            format!("((t.number, t.id) {op} (${bind_index}::int, ${}::uuid))", bind_index + 1)
        }
        SortField::Title => format!(
            "((t.title COLLATE \"C\", t.id) {op} (${bind_index}, ${}::uuid))",
            bind_index + 1
        ),
        SortField::Rank => format!(
            "((t.sort_key COLLATE \"C\", t.id) {op} (${bind_index}, ${}::uuid))",
            bind_index + 1
        ),
        SortField::Priority => format!(
            "((t.priority, t.id) {op} (${bind_index}, ${}::uuid))",
            bind_index + 1
        ),
        SortField::Status => format!(
            "((t.status_id, t.id) {op} (${bind_index}::uuid, ${}::uuid))",
            bind_index + 1
        ),
        SortField::Due => format!(
            "((COALESCE(t.due_date, (t.due_at AT TIME ZONE 'UTC')::date), t.id) {op} (${bind_index}, ${}::uuid))",
            bind_index + 1
        ),
    }
}
