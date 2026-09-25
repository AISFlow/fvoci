use std::collections::{HashMap, HashSet};

use chrono::{DateTime, NaiveDate, Utc};
use serde_json::{json, Value};
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

use crate::db::context::{
    lock_key_from_uuid, lock_membership_users, recheck_session, session_is_live, set_tenant,
};
use crate::db::documents::{between, empty_document_json, DOCUMENT_SCHEMA_VERSION};
use crate::db::identity::{append_audit, append_event, AuditAppend, EventAppend};
use crate::db::labels::{assignee_filter_member_exists, label_is_visible};
use crate::db::milestones::{milestone_is_visible, project_milestone_exists};
use crate::db::projects::{lock_project, project_permission, ProjectDbError};
use crate::projects::ProjectPermission;
use crate::tasks::dependency::{
    finish_date, required_dates_present, schedule_ends, violates_inequality, DependencyType,
    ScheduleEnds,
};
use crate::tasks::list_query::{
    cursor_key_for_row, effective_sort_entries, encode_cursor, filter_fingerprint,
    sort_value_token, AssigneeFilter, ParsedTaskListQuery, SortDirection, SortField,
    TaskListCursor, ViewSort,
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
    pub estimate: Option<String>,
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
    pub assignee_ids: Vec<Uuid>,
    pub label_ids: Vec<Uuid>,
    pub dependencies: Vec<TaskDependencyEdge>,
}

#[derive(Debug, Clone)]
pub struct TaskDependencyEdge {
    pub blocker_id: Uuid,
    pub blocked_id: Uuid,
    pub dependency_type: String,
    pub lag_days: i32,
}

pub struct TaskListItemRow {
    pub meta: TaskMetaRow,
    pub assignee_ids: Vec<Uuid>,
    pub label_ids: Vec<Uuid>,
}

pub struct TaskListPage {
    pub items: Vec<TaskListItemRow>,
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

const MAX_TASK_REFS: usize = 50;

fn unique_ids(ids: &[Uuid]) -> Vec<Uuid> {
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        if seen.insert(*id) {
            out.push(*id);
        }
    }
    out
}

fn uuid_strings(ids: &[Uuid]) -> Vec<String> {
    ids.iter().map(ToString::to_string).collect()
}

async fn list_task_assignee_ids(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    task_id: Uuid,
) -> Result<Vec<Uuid>, sqlx::Error> {
    let rows: Vec<(Uuid,)> = sqlx::query_as(
        r#"
        SELECT user_id
        FROM fvoci.task_assignees
        WHERE workspace_id = $1 AND task_id = $2
        ORDER BY user_id
        "#,
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

async fn list_task_label_ids(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    task_id: Uuid,
) -> Result<Vec<Uuid>, sqlx::Error> {
    let rows: Vec<(Uuid,)> = sqlx::query_as(
        r#"
        SELECT label_id
        FROM fvoci.task_labels
        WHERE workspace_id = $1 AND task_id = $2
        ORDER BY label_id
        "#,
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

async fn load_task_refs(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    task_ids: &[Uuid],
) -> Result<(HashMap<Uuid, Vec<Uuid>>, HashMap<Uuid, Vec<Uuid>>), sqlx::Error> {
    let mut assignees: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    let mut labels: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    if task_ids.is_empty() {
        return Ok((assignees, labels));
    }
    let assignee_rows: Vec<(Uuid, Uuid)> = sqlx::query_as(
        r#"
        SELECT task_id, user_id
        FROM fvoci.task_assignees
        WHERE workspace_id = $1 AND task_id = ANY($2)
        ORDER BY task_id, user_id
        "#,
    )
    .bind(workspace_id)
    .bind(task_ids)
    .fetch_all(&mut **tx)
    .await?;
    for (task_id, user_id) in assignee_rows {
        assignees.entry(task_id).or_default().push(user_id);
    }
    let label_rows: Vec<(Uuid, Uuid)> = sqlx::query_as(
        r#"
        SELECT task_id, label_id
        FROM fvoci.task_labels
        WHERE workspace_id = $1 AND task_id = ANY($2)
        ORDER BY task_id, label_id
        "#,
    )
    .bind(workspace_id)
    .bind(task_ids)
    .fetch_all(&mut **tx)
    .await?;
    for (task_id, label_id) in label_rows {
        labels.entry(task_id).or_default().push(label_id);
    }
    Ok((assignees, labels))
}

async fn membership_exists(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let exists: (bool,) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM fvoci.memberships WHERE workspace_id = $1 AND user_id = $2)",
    )
    .bind(workspace_id)
    .bind(user_id)
    .fetch_one(&mut **tx)
    .await?;
    Ok(exists.0)
}

async fn copy_task_assignees_and_labels(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    source_task_id: Uuid,
    next_task_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO fvoci.task_assignees (workspace_id, task_id, user_id)
        SELECT workspace_id, $3, user_id
        FROM fvoci.task_assignees
        WHERE workspace_id = $1 AND task_id = $2
        "#,
    )
    .bind(workspace_id)
    .bind(source_task_id)
    .bind(next_task_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO fvoci.task_labels (workspace_id, task_id, label_id)
        SELECT workspace_id, $3, label_id
        FROM fvoci.task_labels
        WHERE workspace_id = $1 AND task_id = $2
        "#,
    )
    .bind(workspace_id)
    .bind(source_task_id)
    .bind(next_task_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn replace_task_assignees(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    task_id: Uuid,
    assignee_ids: &[Uuid],
    client_ip: Option<&str>,
) -> Result<Result<(), ProjectDbError>, sqlx::Error> {
    if assignee_ids.len() > MAX_TASK_REFS {
        return Ok(Err(ProjectDbError::InvalidInput));
    }
    let unique = unique_ids(assignee_ids);
    // Caller (patch_task_meta) already holds the assignees' membership locks.
    if !unique.is_empty() {
        for user_id in &unique {
            if !membership_exists(tx, workspace_id, *user_id).await? {
                return Ok(Err(ProjectDbError::AssigneeIsNotAMember));
            }
        }
    }
    let current = list_task_assignee_ids(tx, workspace_id, task_id).await?;
    let changed = current.len() != unique.len() || unique.iter().any(|id| !current.contains(id));
    for user_id in &unique {
        if !current.contains(user_id) {
            sqlx::query(
                r#"
                INSERT INTO fvoci.task_assignees (workspace_id, task_id, user_id)
                VALUES ($1, $2, $3)
                ON CONFLICT (task_id, user_id) DO NOTHING
                "#,
            )
            .bind(workspace_id)
            .bind(task_id)
            .bind(user_id)
            .execute(&mut **tx)
            .await?;
        }
    }
    for user_id in &current {
        if !unique.contains(user_id) {
            sqlx::query(
                r#"
                DELETE FROM fvoci.task_assignees
                WHERE workspace_id = $1 AND task_id = $2 AND user_id = $3
                "#,
            )
            .bind(workspace_id)
            .bind(task_id)
            .bind(user_id)
            .execute(&mut **tx)
            .await?;
        }
    }
    if changed {
        let added: Vec<Uuid> = unique
            .iter()
            .copied()
            .filter(|id| !current.contains(id))
            .collect();
        record_task_event_and_audit(
            tx,
            TaskChangeRecord {
                workspace_id,
                actor_user_id,
                verb: "task.updated",
                target_type: "task",
                target_id: task_id,
                payload: json!({
                    "taskId": task_id.to_string(),
                    "assigneeIds": uuid_strings(&unique),
                    "addedAssigneeIds": uuid_strings(&added),
                }),
                client_ip,
            },
        )
        .await?;
    }
    Ok(Ok(()))
}

async fn replace_task_labels(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    project_id: Uuid,
    task_id: Uuid,
    label_ids: &[Uuid],
    client_ip: Option<&str>,
) -> Result<Result<(), ProjectDbError>, sqlx::Error> {
    if label_ids.len() > MAX_TASK_REFS {
        return Ok(Err(ProjectDbError::InvalidInput));
    }
    let unique = unique_ids(label_ids);
    if !unique.is_empty() {
        let known: Vec<(Uuid,)> = sqlx::query_as(
            r#"
            SELECT id
            FROM fvoci.labels
            WHERE workspace_id = $1 AND project_id = $2 AND id = ANY($3)
            "#,
        )
        .bind(workspace_id)
        .bind(project_id)
        .bind(&unique)
        .fetch_all(&mut **tx)
        .await?;
        if known.len() != unique.len() {
            return Ok(Err(ProjectDbError::LabelNotFound));
        }
    }
    let current = list_task_label_ids(tx, workspace_id, task_id).await?;
    let changed = current.len() != unique.len() || unique.iter().any(|id| !current.contains(id));
    for label_id in &unique {
        if !current.contains(label_id) {
            sqlx::query(
                r#"
                INSERT INTO fvoci.task_labels (workspace_id, task_id, label_id)
                VALUES ($1, $2, $3)
                ON CONFLICT (task_id, label_id) DO NOTHING
                "#,
            )
            .bind(workspace_id)
            .bind(task_id)
            .bind(label_id)
            .execute(&mut **tx)
            .await?;
        }
    }
    for label_id in &current {
        if !unique.contains(label_id) {
            sqlx::query(
                r#"
                DELETE FROM fvoci.task_labels
                WHERE workspace_id = $1 AND task_id = $2 AND label_id = $3
                "#,
            )
            .bind(workspace_id)
            .bind(task_id)
            .bind(label_id)
            .execute(&mut **tx)
            .await?;
        }
    }
    if changed {
        record_task_event_and_audit(
            tx,
            TaskChangeRecord {
                workspace_id,
                actor_user_id,
                verb: "task.updated",
                target_type: "task",
                target_id: task_id,
                payload: json!({
                    "taskId": task_id.to_string(),
                    "labelIds": uuid_strings(&unique),
                }),
                client_ip,
            },
        )
        .await?;
    }
    Ok(Ok(()))
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

const TASK_STATUS_LOCK_NAMESPACE: i32 = 1_907_002;

fn violates_task_hierarchy(child_type: &str, parent_type: &str) -> bool {
    if child_type == "subtask" {
        !matches!(parent_type, "task" | "bug" | "story")
    } else if child_type == "epic" {
        true
    } else {
        parent_type != "epic"
    }
}

fn allowed_child_types(task_type: &str) -> &'static [&'static str] {
    match task_type {
        "epic" => &["task", "bug", "story"],
        "subtask" => &[],
        _ => &["subtask"],
    }
}

fn days_in_month(year: i32, month: u32) -> Option<u32> {
    use chrono::Datelike;
    let (next_year, next_month) = if month == 12 {
        (year.checked_add(1)?, 1)
    } else {
        (year, month + 1)
    };
    NaiveDate::from_ymd_opt(next_year, next_month, 1)?
        .pred_opt()
        .map(|d| d.day())
}

/// Matches source `shiftDate` monthly rollover (e.g. 2026-01-31 → 2026-03-03).
fn shift_recurrence_date_monthly(date: NaiveDate) -> Option<NaiveDate> {
    use chrono::Datelike;
    let (new_year, new_month) = if date.month() == 12 {
        (date.year().checked_add(1)?, 1)
    } else {
        (date.year(), date.month() + 1)
    };
    let days_in_target = days_in_month(new_year, new_month)?;
    if date.day() <= days_in_target {
        return NaiveDate::from_ymd_opt(new_year, new_month, date.day());
    }
    let (overflow_year, overflow_month) = if new_month == 12 {
        (new_year.checked_add(1)?, 1)
    } else {
        (new_year, new_month + 1)
    };
    NaiveDate::from_ymd_opt(overflow_year, overflow_month, date.day() - days_in_target)
}

/// Returns None when the shifted date leaves the four-digit-year range the API
/// accepts (the source fails the request in that case too).
fn shift_recurrence_date(date: NaiveDate, kind: &str) -> Option<NaiveDate> {
    use chrono::Datelike;
    let shifted = match kind {
        "daily" => date.checked_add_signed(chrono::Duration::days(1)),
        "weekly" => date.checked_add_signed(chrono::Duration::days(7)),
        "monthly" => shift_recurrence_date_monthly(date),
        _ => Some(date),
    }?;
    (shifted.year() <= 9999).then_some(shifted)
}

fn parse_recurrence_kind(value: &Value) -> Option<&str> {
    value
        .as_object()
        .and_then(|obj| obj.get("kind"))
        .and_then(Value::as_str)
        .filter(|kind| matches!(*kind, "daily" | "weekly" | "monthly"))
}

async fn has_children_outside_types(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    task_id: Uuid,
    allowed: &[&str],
) -> Result<bool, sqlx::Error> {
    let row: (bool,) = if allowed.is_empty() {
        sqlx::query_as(
            r#"
            SELECT EXISTS(
                SELECT 1 FROM fvoci.tasks
                WHERE workspace_id = $1 AND parent_id = $2
            )
            "#,
        )
        .bind(workspace_id)
        .bind(task_id)
        .fetch_one(&mut **tx)
        .await?
    } else {
        sqlx::query_as(
            r#"
            SELECT EXISTS(
                SELECT 1 FROM fvoci.tasks
                WHERE workspace_id = $1 AND parent_id = $2
                  AND type <> ALL($3::text[])
            )
            "#,
        )
        .bind(workspace_id)
        .bind(task_id)
        .bind(allowed)
        .fetch_one(&mut **tx)
        .await?
    };
    Ok(row.0)
}

async fn assert_task_hierarchy(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    project_id: Uuid,
    task_id: Uuid,
    next_type: &str,
    next_parent: Option<Uuid>,
    parent_changed: bool,
) -> Result<Result<(), ProjectDbError>, sqlx::Error> {
    if let Some(parent_id) = next_parent {
        if parent_id == task_id {
            return Ok(Err(ProjectDbError::Conflict));
        }
        type ParentRow = (Uuid, Option<DateTime<Utc>>, Option<DateTime<Utc>>, String);
        let parent: Option<ParentRow> = sqlx::query_as(
            r#"
                SELECT project_id, deleted_at, archived_at, type
                FROM fvoci.tasks
                WHERE workspace_id = $1 AND id = $2
                "#,
        )
        .bind(workspace_id)
        .bind(parent_id)
        .fetch_optional(&mut **tx)
        .await?;
        let Some((parent_project, deleted, archived, parent_type)) = parent else {
            return Ok(Err(ProjectDbError::NotFound));
        };
        if deleted.is_some() || parent_project != project_id {
            return Ok(Err(ProjectDbError::NotFound));
        }
        if parent_changed && archived.is_some() {
            return Ok(Err(ProjectDbError::TaskArchived));
        }
        if violates_task_hierarchy(next_type, &parent_type) {
            return Ok(Err(ProjectDbError::Conflict));
        }
    } else if next_type == "subtask" {
        return Ok(Err(ProjectDbError::Conflict));
    }
    if has_children_outside_types(tx, workspace_id, task_id, allowed_child_types(next_type)).await?
    {
        return Ok(Err(ProjectDbError::Conflict));
    }
    Ok(Ok(()))
}

async fn lock_status_for_transition(
    tx: &mut Transaction<'_, Postgres>,
    status_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT pg_advisory_xact_lock($1, $2)")
        .bind(TASK_STATUS_LOCK_NAMESPACE)
        .bind(lock_key_from_uuid(status_id))
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn count_active_tasks_in_status(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    status_id: Uuid,
) -> Result<i64, sqlx::Error> {
    let row: (i64,) = sqlx::query_as(
        r#"
        SELECT count(*)
        FROM fvoci.tasks
        WHERE workspace_id = $1
          AND status_id = $2
          AND deleted_at IS NULL
          AND archived_at IS NULL
        "#,
    )
    .bind(workspace_id)
    .bind(status_id)
    .fetch_one(&mut **tx)
    .await?;
    Ok(row.0)
}

struct StatusInfo {
    category: String,
    wip_limit: Option<i32>,
}

async fn load_status_info(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    project_id: Uuid,
    status_id: Uuid,
) -> Result<Option<StatusInfo>, sqlx::Error> {
    let row: Option<(String, Option<i32>)> = sqlx::query_as(
        r#"
        SELECT category, wip_limit
        FROM fvoci.statuses
        WHERE workspace_id = $1 AND project_id = $2 AND id = $3
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .bind(status_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.map(|(category, wip_limit)| StatusInfo {
        category,
        wip_limit,
    }))
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
    estimate: Option<String>,
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

    if let Some(milestone_id) = input.milestone_id {
        if !project_milestone_exists(&mut tx, workspace_id, project_id, milestone_id).await? {
            tx.rollback().await?;
            return Ok(Err(ProjectDbError::MilestoneNotFound));
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
            return Ok(Err(ProjectDbError::StatusNotInWorkflow));
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

    let last_sort: Option<(String,)> = sqlx::query_as(
        r#"
        SELECT sort_key
        FROM fvoci.tasks
        WHERE workspace_id = $1
          AND project_id = $2
          AND status_id = $3
          AND deleted_at IS NULL
        ORDER BY sort_key COLLATE "C" DESC
        LIMIT 1
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .bind(status_id)
    .fetch_optional(&mut *tx)
    .await?;
    let sort_key = match between(last_sort.as_ref().map(|(key,)| key.as_str()), None) {
        Ok(key) => key,
        Err(err) => {
            tracing::error!("task sort_key allocation failed: {err}");
            tx.rollback().await?;
            return Ok(Err(ProjectDbError::Conflict));
        }
    };

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
                  start_date, due_date, due_at, estimate::text AS estimate, parent_id,
                  milestone_id, sort_key, schema_version, version, archived_at, created_by,
                  created_at, updated_at
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
               t.start_date, t.due_date, t.due_at, t.estimate::text AS estimate,
               t.parent_id, t.milestone_id, t.sort_key, t.schema_version, t.version,
               t.archived_at, t.created_by, t.created_at, t.updated_at
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
    let assignee_ids = list_task_assignee_ids(&mut tx, workspace_id, task_id).await?;
    let label_ids = list_task_label_ids(&mut tx, workspace_id, task_id).await?;
    let dependencies = list_task_dependency_edges(&mut tx, workspace_id, task_id).await?;

    tx.commit().await?;
    Ok(Ok(TaskDetailRow {
        meta,
        content_json,
        can_edit: permission.at_least(ProjectPermission::Edit),
        parent,
        children,
        child_progress,
        assignee_ids,
        label_ids,
        dependencies,
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
    Option<DateTime<Utc>>,
    String,
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
    if let Some(label_id) = query.view.filters.label_id {
        if !label_is_visible(&mut tx, workspace_id, actor_user_id, label_id).await? {
            tx.rollback().await?;
            return Ok(Err(ProjectDbError::InvalidInput));
        }
    }
    if let Some(milestone_id) = query.view.filters.milestone_id {
        if !milestone_is_visible(&mut tx, workspace_id, actor_user_id, milestone_id).await? {
            tx.rollback().await?;
            return Ok(Err(ProjectDbError::InvalidInput));
        }
    }
    if let Some(AssigneeFilter::User(user_id)) = query.view.filters.assignee_id {
        if !assignee_filter_member_exists(&mut tx, workspace_id, user_id).await? {
            tx.rollback().await?;
            return Ok(Err(ProjectDbError::InvalidInput));
        }
    }

    let (base_conditions, base_binds) = task_list_filter_conditions(query, actor_user_id);
    let mut conditions = base_conditions.clone();
    let mut binds = base_binds.clone();
    let sort = effective_sort_entries(&query.view.sort);
    if let Some(cursor) = &query.cursor {
        let anchor: Option<TaskListCursorAnchor> = sqlx::query_as(
            r#"
            SELECT t.created_at, t.updated_at, t.id, t.number, t.title, t.sort_key, t.priority, t.status_id,
                   t.due_date, t.due_at,
                   (
                       SELECT st.sort_key
                       FROM fvoci.statuses st
                       WHERE st.workspace_id = t.workspace_id
                         AND st.project_id = t.project_id
                         AND st.id = t.status_id
                   ) AS status_sort_key
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
            _status_id,
            due_date,
            due_at,
            status_sort_key,
        )) = anchor
        else {
            tx.rollback().await?;
            return Ok(Err(ProjectDbError::InvalidCursor));
        };
        let key = cursor_key_for_row(
            &sort,
            id,
            created_at,
            updated_at,
            number,
            &title,
            &sort_key,
            &priority,
            &status_sort_key,
            due_date,
            due_at,
        );
        if key != cursor.key {
            tx.rollback().await?;
            return Ok(Err(ProjectDbError::InvalidCursor));
        }
        let bind_start = binds.len() + 3;
        conditions.push(cursor_clause(&sort, bind_start));
        for entry in &sort {
            binds.push(cursor_bind_value(
                entry.field,
                created_at,
                updated_at,
                number,
                &title,
                &sort_key,
                &priority,
                &status_sort_key,
                due_date,
                due_at,
            ));
        }
        binds.push(id.to_string());
    }

    let list_where_sql = conditions.join(" AND ");
    let count_where_sql = base_conditions.join(" AND ");
    let order_sql = order_clause(&sort);
    let limit = query.limit + 1;
    let list_sql = format!(
        r#"
        SELECT t.id, t.project_id, t.number, t.title, t.type AS task_type, t.priority, t.status_id,
               t.start_date, t.due_date, t.due_at, t.estimate::text AS estimate,
               t.parent_id, t.milestone_id, t.sort_key, t.schema_version, t.version,
               t.archived_at, t.created_by, t.created_at, t.updated_at, t.recurrence,
               (
                   SELECT st.sort_key
                   FROM fvoci.statuses st
                   WHERE st.workspace_id = t.workspace_id
                     AND st.project_id = t.project_id
                     AND st.id = t.status_id
               ) AS status_sort_key
        FROM fvoci.tasks t
        WHERE {list_where_sql}
        ORDER BY {order_sql}
        LIMIT {limit}
        "#
    );
    let mut list_query = sqlx::query(&list_sql).bind(workspace_id).bind(project_id);
    for value in &binds {
        list_query = list_query.bind(value);
    }
    let rows = list_query.fetch_all(&mut *tx).await?;

    let count_sql = format!(
        r#"
        SELECT t.status_id, count(*)
        FROM fvoci.tasks t
        WHERE {count_where_sql}
        GROUP BY t.status_id
        "#
    );
    let mut count_query = sqlx::query_as::<_, (Uuid, i64)>(&count_sql)
        .bind(workspace_id)
        .bind(project_id);
    for value in &base_binds {
        count_query = count_query.bind(value);
    }
    let status_counts = count_query.fetch_all(&mut *tx).await?;

    let mut items = Vec::new();
    let mut item_ids = Vec::new();
    for row in rows.iter().take(query.limit as usize) {
        let record = map_task_row(row)?;
        item_ids.push(record.id);
        let recurrence = row.try_get::<Option<Value>, _>("recurrence").ok().flatten();
        items.push(row_to_meta(workspace_id, record, recurrence));
    }
    let (assignee_map, label_map) = load_task_refs(&mut tx, workspace_id, &item_ids).await?;
    let items = items
        .into_iter()
        .map(|meta| {
            let id = meta.id;
            TaskListItemRow {
                assignee_ids: assignee_map.get(&id).cloned().unwrap_or_default(),
                label_ids: label_map.get(&id).cloned().unwrap_or_default(),
                meta,
            }
        })
        .collect();
    let next_cursor = if rows.len() as i32 > query.limit {
        let last = &rows[(query.limit - 1) as usize];
        let record = map_task_row(last)?;
        let created_at: DateTime<Utc> = last.try_get("created_at")?;
        let updated_at: DateTime<Utc> = last.try_get("updated_at")?;
        let due_date: Option<NaiveDate> = last.try_get("due_date")?;
        let due_at: Option<DateTime<Utc>> = last.try_get("due_at")?;
        let status_sort_key: String = last.try_get("status_sort_key")?;
        let key = cursor_key_for_row(
            &sort,
            record.id,
            created_at,
            updated_at,
            record.number,
            &record.title,
            &record.sort_key,
            &record.priority,
            &status_sort_key,
            due_date,
            due_at,
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

fn task_list_filter_conditions(
    query: &ParsedTaskListQuery,
    actor_user_id: Uuid,
) -> (Vec<String>, Vec<String>) {
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
        binds.push(format!("%{}%", escape_ilike_pattern(title)));
        conditions.push(format!("t.title ILIKE ${idx} ESCAPE '\\'"));
    }
    if let Some(label_id) = query.view.filters.label_id {
        let idx = binds.len() + 3;
        binds.push(label_id.to_string());
        conditions.push(format!(
            "EXISTS (
                SELECT 1 FROM fvoci.task_labels l
                WHERE l.workspace_id = t.workspace_id
                  AND l.task_id = t.id
                  AND l.label_id = ${idx}::uuid
            )"
        ));
    }
    if let Some(milestone_id) = query.view.filters.milestone_id {
        let idx = binds.len() + 3;
        binds.push(milestone_id.to_string());
        conditions.push(format!("t.milestone_id = ${idx}::uuid"));
    }
    if let Some(assignee) = &query.view.filters.assignee_id {
        let assignee_id = match assignee {
            AssigneeFilter::Me => actor_user_id,
            AssigneeFilter::User(id) => *id,
        };
        let idx = binds.len() + 3;
        binds.push(assignee_id.to_string());
        conditions.push(format!(
            "EXISTS (
                SELECT 1 FROM fvoci.task_assignees a
                WHERE a.workspace_id = t.workspace_id
                  AND a.task_id = t.id
                  AND a.user_id = ${idx}::uuid
            )"
        ));
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
    let as_of_idx = binds.len() + 3;
    binds.push(query.as_of.to_rfc3339());
    conditions.push(format!("t.created_at <= ${as_of_idx}::timestamptz"));
    (conditions, binds)
}

fn escape_ilike_pattern(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        if matches!(ch, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// Due-date sort uses UTC; the source uses the request time zone.
fn sort_expression_sql(field: SortField) -> &'static str {
    match field {
        SortField::Priority => {
            "CASE t.priority WHEN 'none' THEN 0 WHEN 'low' THEN 1 WHEN 'medium' THEN 2 WHEN 'high' THEN 3 WHEN 'urgent' THEN 4 END"
        }
        SortField::Due => "COALESCE(t.due_date, (t.due_at AT TIME ZONE 'UTC')::date)",
        SortField::Updated => "t.updated_at",
        SortField::Created => "t.created_at",
        SortField::Rank => r#"t.sort_key COLLATE "C""#,
        SortField::Title => r#"t.title COLLATE "C""#,
        SortField::Status => {
            "(SELECT st.sort_key FROM fvoci.statuses st WHERE st.workspace_id = t.workspace_id AND st.project_id = t.project_id AND st.id = t.status_id)"
        }
        SortField::Number => "t.number",
    }
}

fn sort_anchor_ref(field: SortField, bind_index: usize) -> String {
    match field {
        SortField::Created | SortField::Updated => format!("${bind_index}::timestamptz"),
        SortField::Number | SortField::Priority => format!("${bind_index}::int"),
        SortField::Due => format!("NULLIF(${bind_index}, 'null')::date"),
        SortField::Title | SortField::Rank | SortField::Status => format!("${bind_index}"),
    }
}

fn order_clause(sort: &[ViewSort]) -> String {
    let mut parts = Vec::new();
    for entry in sort {
        let column = sort_expression_sql(entry.field);
        let dir = if entry.direction == SortDirection::Asc {
            "ASC"
        } else {
            "DESC"
        };
        parts.push(format!("{column} {dir} NULLS LAST"));
    }
    parts.push("t.id ASC".to_string());
    parts.join(", ")
}

fn cursor_clause(sort: &[ViewSort], bind_start: usize) -> String {
    let id_bind = bind_start + sort.len();
    let mut branches = Vec::with_capacity(sort.len() + 1);
    for (index, entry) in sort.iter().enumerate() {
        let mut parts = Vec::with_capacity(index + 1);
        for (prior_index, prior) in sort[..index].iter().enumerate() {
            parts.push(sort_equality_sql(prior.field, bind_start + prior_index));
        }
        parts.push(sort_strict_after_sql(
            entry.field,
            bind_start + index,
            entry.direction,
        ));
        branches.push(format!("({})", parts.join(" AND ")));
    }
    let mut equal_parts: Vec<String> = sort
        .iter()
        .enumerate()
        .map(|(index, entry)| sort_equality_sql(entry.field, bind_start + index))
        .collect();
    equal_parts.push(format!("t.id > ${id_bind}::uuid"));
    branches.push(format!("({})", equal_parts.join(" AND ")));
    format!("({})", branches.join(" OR "))
}

fn sort_equality_sql(field: SortField, bind_index: usize) -> String {
    let expr = sort_expression_sql(field);
    let anchor = sort_anchor_ref(field, bind_index);
    format!("{expr} IS NOT DISTINCT FROM {anchor}")
}

fn sort_strict_after_sql(field: SortField, bind_index: usize, direction: SortDirection) -> String {
    let expr = sort_expression_sql(field);
    let anchor = sort_anchor_ref(field, bind_index);
    let op = if direction == SortDirection::Asc {
        ">"
    } else {
        "<"
    };
    format!("({expr} IS NULL AND {anchor} IS NOT NULL OR {expr} {op} {anchor})")
}

#[allow(clippy::too_many_arguments)]
fn cursor_bind_value(
    field: SortField,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    number: i32,
    title: &str,
    sort_key: &str,
    priority: &str,
    status_sort_key: &str,
    due_date: Option<NaiveDate>,
    due_at: Option<DateTime<Utc>>,
) -> String {
    sort_value_token(
        field,
        created_at,
        updated_at,
        number,
        title,
        sort_key,
        priority,
        status_sort_key,
        due_date,
        due_at,
    )
}

#[derive(Debug, Clone)]
struct TaskWriteRow {
    record: TaskRowRecord,
    recurrence: Option<Value>,
}

async fn task_project_id_for_write(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    task_id: Uuid,
) -> Result<Option<Uuid>, sqlx::Error> {
    let row: Option<(Uuid,)> = sqlx::query_as(
        r#"
        SELECT project_id
        FROM fvoci.tasks
        WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL
        "#,
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.map(|(project_id,)| project_id))
}

async fn load_task_for_write_locked(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    task_id: Uuid,
) -> Result<Option<TaskWriteRow>, sqlx::Error> {
    let row = sqlx::query(
        r#"
        SELECT id, project_id, number, title, type AS task_type, priority, status_id,
               start_date, due_date, due_at, estimate::text AS estimate, parent_id,
               milestone_id, sort_key, schema_version, version, archived_at, created_by, created_at,
               updated_at, recurrence
        FROM fvoci.tasks
        WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL
        FOR NO KEY UPDATE
        "#,
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let record = map_task_row(&row)?;
    let recurrence = row.try_get::<Option<Value>, _>("recurrence").ok().flatten();
    Ok(Some(TaskWriteRow { record, recurrence }))
}

async fn require_task_write_access(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    task_id: Uuid,
    allow_archived: bool,
) -> Result<Result<TaskWriteRow, ProjectDbError>, sqlx::Error> {
    lock_membership_users(tx, &[actor_user_id]).await?;
    if !recheck_session(tx, actor_user_id, session_id).await? {
        return Ok(Err(ProjectDbError::Forbidden));
    }
    if !workspace_is_live(tx, workspace_id).await? {
        return Ok(Err(ProjectDbError::NotFound));
    }
    let Some(project_id) = task_project_id_for_write(tx, workspace_id, task_id).await? else {
        return Ok(Err(ProjectDbError::NotFound));
    };
    let locked = lock_project(tx, workspace_id, project_id).await?;
    let Some(locked) = locked else {
        return Ok(Err(ProjectDbError::NotFound));
    };
    if locked.status == "archived" {
        return Ok(Err(ProjectDbError::Archived));
    }
    let permission = project_permission(tx, workspace_id, actor_user_id, &locked).await?;
    if !permission.at_least(ProjectPermission::Edit) {
        return Ok(Err(ProjectDbError::NotFound));
    }
    let Some(task) = load_task_for_write_locked(tx, workspace_id, task_id).await? else {
        return Ok(Err(ProjectDbError::NotFound));
    };
    if !allow_archived && task.record.archived_at.is_some() {
        return Ok(Err(ProjectDbError::TaskArchived));
    }
    Ok(Ok(task))
}

fn resolve_anchor_sort_key(
    ordered: &[(Uuid, String)],
    before_id: Option<Uuid>,
    after_id: Option<Uuid>,
) -> Result<String, ProjectDbError> {
    if before_id.is_some() && after_id.is_some() {
        return Err(ProjectDbError::InvalidMoveAnchors);
    }
    if let Some(before_id) = before_id {
        let index = ordered.iter().position(|(id, _)| *id == before_id);
        let Some(index) = index else {
            return Err(ProjectDbError::InvalidAnchor);
        };
        let left = index
            .checked_sub(1)
            .and_then(|i| ordered.get(i))
            .map(|(_, key)| key.as_str());
        let right = ordered.get(index).map(|(_, key)| key.as_str());
        return between(left, right).map_err(|_| ProjectDbError::InvalidAnchor);
    }
    if let Some(after_id) = after_id {
        let index = ordered.iter().position(|(id, _)| *id == after_id);
        let Some(index) = index else {
            return Err(ProjectDbError::InvalidAnchor);
        };
        let left = ordered.get(index).map(|(_, key)| key.as_str());
        let right = ordered.get(index + 1).map(|(_, key)| key.as_str());
        return between(left, right).map_err(|_| ProjectDbError::InvalidAnchor);
    }
    let last = ordered.last().map(|(_, key)| key.as_str());
    between(last, None).map_err(|_| ProjectDbError::InvalidAnchor)
}

async fn list_sorted_tasks_in_status(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    status_id: Uuid,
    exclude_id: Option<Uuid>,
) -> Result<Vec<(Uuid, String)>, sqlx::Error> {
    let rows = sqlx::query_as::<_, (Uuid, String)>(
        r#"
        SELECT id, sort_key
        FROM fvoci.tasks
        WHERE workspace_id = $1
          AND status_id = $2
          AND deleted_at IS NULL
          AND archived_at IS NULL
        ORDER BY sort_key COLLATE "C", id
        "#,
    )
    .bind(workspace_id)
    .bind(status_id)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows
        .into_iter()
        .filter(|(id, _)| exclude_id.map(|skip| skip != *id).unwrap_or(true))
        .collect())
}

#[derive(Debug, Clone, Copy)]
struct TransitionResult {
    status_changed: bool,
}

#[derive(Debug, Clone)]
struct RecurrenceSpawnFields {
    task_type: String,
    parent_id: Option<Uuid>,
}

async fn spawn_recurring_next_task(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    project_id: Uuid,
    actor_user_id: Uuid,
    task: &TaskWriteRow,
    recurrence_kind: &str,
    spawn_fields: Option<&RecurrenceSpawnFields>,
) -> Result<Result<(), ProjectDbError>, sqlx::Error> {
    let next_id = Uuid::now_v7();
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
    .fetch_one(&mut **tx)
    .await?;
    let target_status = match default_backlog_status(tx, workspace_id, project_id).await? {
        Some(status_id) => status_id,
        None => return Ok(Err(ProjectDbError::WorkflowHasNoStatuses)),
    };
    let siblings = list_sorted_tasks_in_status(tx, workspace_id, target_status, None).await?;
    let sort_key = match resolve_anchor_sort_key(&siblings, None, None) {
        Ok(key) => key,
        Err(err) => return Ok(Err(err)),
    };
    let task_type = spawn_fields
        .map(|fields| fields.task_type.as_str())
        .unwrap_or(&task.record.task_type);
    let parent_id = spawn_fields
        .map(|fields| fields.parent_id)
        .unwrap_or(task.record.parent_id);
    let shift = |date: Option<NaiveDate>| match date {
        None => Ok(None),
        Some(date) => shift_recurrence_date(date, recurrence_kind)
            .map(Some)
            .ok_or_else(|| sqlx::Error::Protocol("recurrence date out of range".into())),
    };
    let next_start = shift(task.record.start_date)?;
    let next_due = shift(task.record.due_date)?;
    let recurrence = json!({ "kind": recurrence_kind });
    sqlx::query(
        r#"
        INSERT INTO fvoci.tasks (
            id, workspace_id, project_id, number, title, type, priority, status_id,
            start_date, due_date, parent_id, milestone_id, recurrence, sort_key,
            schema_version, content_json, created_by
        ) VALUES (
            $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17
        )
        "#,
    )
    .bind(next_id)
    .bind(workspace_id)
    .bind(project_id)
    .bind(number.0)
    .bind(&task.record.title)
    .bind(task_type)
    .bind(&task.record.priority)
    .bind(target_status)
    .bind(next_start)
    .bind(next_due)
    .bind(parent_id)
    .bind(task.record.milestone_id)
    .bind(recurrence)
    .bind(sort_key)
    .bind(DOCUMENT_SCHEMA_VERSION)
    .bind(empty_document_json())
    .bind(actor_user_id)
    .execute(&mut **tx)
    .await?;
    copy_task_assignees_and_labels(tx, workspace_id, task.record.id, next_id).await?;
    record_task_event_and_audit(
        tx,
        TaskChangeRecord {
            workspace_id,
            actor_user_id,
            verb: "task.created",
            target_type: "task",
            target_id: next_id,
            payload: json!({
                "taskId": next_id.to_string(),
                "projectId": project_id.to_string(),
                "number": number.0,
                "statusId": target_status.to_string(),
                "title": task.record.title,
                "recurrenceOf": task.record.id.to_string(),
            }),
            client_ip: None,
        },
    )
    .await?;
    Ok(Ok(()))
}

#[allow(clippy::too_many_arguments)]
async fn transition_task_status(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    project_id: Uuid,
    actor_user_id: Uuid,
    task: &TaskWriteRow,
    from_status_id: Uuid,
    to_status_id: Uuid,
    before_id: Option<Uuid>,
    after_id: Option<Uuid>,
    spawn_fields: Option<&RecurrenceSpawnFields>,
) -> Result<Result<TransitionResult, ProjectDbError>, sqlx::Error> {
    if from_status_id == to_status_id && before_id.is_none() && after_id.is_none() {
        return Ok(Ok(TransitionResult {
            status_changed: false,
        }));
    }
    let to_status = load_status_info(tx, workspace_id, project_id, to_status_id).await?;
    let Some(to_status) = to_status else {
        return Ok(Err(ProjectDbError::StatusNotInWorkflow));
    };
    let from_status = load_status_info(tx, workspace_id, project_id, from_status_id).await?;
    let changing_column = from_status_id != to_status_id;
    if changing_column && to_status.wip_limit.is_some() {
        lock_status_for_transition(tx, to_status_id).await?;
        let occupied = count_active_tasks_in_status(tx, workspace_id, to_status_id).await?;
        let limit = to_status.wip_limit.unwrap_or(0) as i64;
        if occupied >= limit {
            return Ok(Err(ProjectDbError::WipLimitExceeded));
        }
    }
    let siblings =
        list_sorted_tasks_in_status(tx, workspace_id, to_status_id, Some(task.record.id)).await?;
    let sort_key = match resolve_anchor_sort_key(&siblings, before_id, after_id) {
        Ok(key) => key,
        Err(err) => return Ok(Err(err)),
    };
    let result = sqlx::query(
        r#"
        UPDATE fvoci.tasks
        SET status_id = $4, sort_key = $5, updated_at = now()
        WHERE workspace_id = $1 AND id = $2 AND status_id = $3 AND deleted_at IS NULL
        "#,
    )
    .bind(workspace_id)
    .bind(task.record.id)
    .bind(from_status_id)
    .bind(to_status_id)
    .bind(&sort_key)
    .execute(&mut **tx)
    .await?;
    if result.rows_affected() == 0 {
        return Ok(Err(ProjectDbError::VersionConflict));
    }
    if changing_column
        && to_status.category == "done"
        && from_status.as_ref().map(|s| s.category.as_str()) != Some("done")
    {
        if let Some(kind) = task.recurrence.as_ref().and_then(parse_recurrence_kind) {
            sqlx::query(
                r#"
                UPDATE fvoci.tasks
                SET recurrence = NULL, updated_at = now()
                WHERE workspace_id = $1 AND id = $2
                "#,
            )
            .bind(workspace_id)
            .bind(task.record.id)
            .execute(&mut **tx)
            .await?;
            match spawn_recurring_next_task(
                tx,
                workspace_id,
                project_id,
                actor_user_id,
                task,
                kind,
                spawn_fields,
            )
            .await?
            {
                Ok(()) => {}
                Err(err) => return Ok(Err(err)),
            }
        }
    }
    Ok(Ok(TransitionResult {
        status_changed: changing_column,
    }))
}

fn dates_conflict(
    expected: &crate::tasks::patch::ExpectedDatesInput,
    start_date: Option<NaiveDate>,
    due_date: Option<NaiveDate>,
    due_at: Option<DateTime<Utc>>,
) -> bool {
    expected.start_date != start_date || expected.due_date != due_date || expected.due_at != due_at
}

fn patch_only_unarchives(input: &crate::tasks::patch::PatchTaskMetaInput) -> bool {
    input.archived == Some(false)
        && input.task_type.is_none()
        && input.title.is_none()
        && input.priority.is_none()
        && input.status_id.is_none()
        && input.start_date == crate::tasks::patch::FieldUpdate::Unchanged
        && input.due_date == crate::tasks::patch::FieldUpdate::Unchanged
        && input.due_at == crate::tasks::patch::FieldUpdate::Unchanged
        && input.estimate == crate::tasks::patch::FieldUpdate::Unchanged
        && input.parent_id == crate::tasks::patch::FieldUpdate::Unchanged
        && input.milestone_id == crate::tasks::patch::FieldUpdate::Unchanged
        && input.recurrence == crate::tasks::patch::FieldUpdate::Unchanged
        && input.expected_dates.is_none()
        && input.assignee_ids.is_none()
        && input.label_ids.is_none()
}

pub async fn patch_task_meta(
    pool: &PgPool,
    workspace_id: Uuid,
    task_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    input: crate::tasks::patch::PatchTaskMetaInput,
    client_ip: Option<&str>,
) -> Result<Result<TaskMetaRow, ProjectDbError>, sqlx::Error> {
    let restores_archived = patch_only_unarchives(&input);
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    // Membership locks come before the project row lock on every write path; take
    // the assignees' together with the actor's (sorted) before anything else.
    if let Some(assignee_ids) = input
        .assignee_ids
        .as_deref()
        .filter(|ids| !ids.is_empty() && ids.len() <= MAX_TASK_REFS)
    {
        let mut lock_ids = unique_ids(assignee_ids);
        lock_ids.push(actor_user_id);
        lock_membership_users(&mut tx, &lock_ids).await?;
    }
    let task = match require_task_write_access(
        &mut tx,
        workspace_id,
        actor_user_id,
        session_id,
        task_id,
        restores_archived,
    )
    .await?
    {
        Ok(task) => task,
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    };
    if let Some(expected) = &input.expected_dates {
        if dates_conflict(
            expected,
            task.record.start_date,
            task.record.due_date,
            task.record.due_at,
        ) {
            tx.rollback().await?;
            return Ok(Err(ProjectDbError::VersionConflict));
        }
    }

    let next_type = input.task_type.as_deref().unwrap_or(&task.record.task_type);
    let next_parent = match input.parent_id {
        crate::tasks::patch::FieldUpdate::Set(parent_id) => Some(parent_id),
        crate::tasks::patch::FieldUpdate::Clear => None,
        crate::tasks::patch::FieldUpdate::Unchanged => task.record.parent_id,
    };
    let parent_changed = input.parent_id != crate::tasks::patch::FieldUpdate::Unchanged;
    if input.task_type.is_some() || parent_changed {
        match assert_task_hierarchy(
            &mut tx,
            workspace_id,
            task.record.project_id,
            task_id,
            next_type,
            next_parent,
            parent_changed,
        )
        .await?
        {
            Ok(()) => {}
            Err(err) => {
                tx.rollback().await?;
                return Ok(Err(err));
            }
        }
    }

    let recurrence_spawn_fields = RecurrenceSpawnFields {
        task_type: next_type.to_string(),
        parent_id: next_parent,
    };

    if input.milestone_id != crate::tasks::patch::FieldUpdate::Unchanged {
        if let crate::tasks::patch::FieldUpdate::Set(milestone_id) = input.milestone_id {
            if !project_milestone_exists(
                &mut tx,
                workspace_id,
                task.record.project_id,
                milestone_id,
            )
            .await?
            {
                tx.rollback().await?;
                return Ok(Err(ProjectDbError::MilestoneNotFound));
            }
        }
    }

    if input.start_date != crate::tasks::patch::FieldUpdate::Unchanged
        || input.due_date != crate::tasks::patch::FieldUpdate::Unchanged
        || input.due_at != crate::tasks::patch::FieldUpdate::Unchanged
    {
        match assert_dependency_dates_ok(&mut tx, workspace_id, &task.record, &input).await? {
            Ok(()) => {}
            Err(err) => {
                tx.rollback().await?;
                return Ok(Err(err));
            }
        }
    }

    let mut status_changed = false;
    let from_status_id = task.record.status_id;
    if let Some(status_id) = input.status_id {
        if status_id != task.record.status_id {
            let result = transition_task_status(
                &mut tx,
                workspace_id,
                task.record.project_id,
                actor_user_id,
                &task,
                task.record.status_id,
                status_id,
                None,
                None,
                Some(&recurrence_spawn_fields),
            )
            .await?;
            match result {
                Ok(transition) => status_changed = transition.status_changed,
                Err(err) => {
                    tx.rollback().await?;
                    return Ok(Err(err));
                }
            }
        }
    }

    let mut sets = Vec::<String>::new();
    let mut bind_idx = 3u32;
    let mut bind_title: Option<String> = None;
    let mut bind_type: Option<String> = None;
    let mut bind_priority: Option<String> = None;
    let mut bind_start_date: Option<Option<NaiveDate>> = None;
    let mut bind_due_date: Option<Option<NaiveDate>> = None;
    let mut bind_due_at: Option<Option<DateTime<Utc>>> = None;
    let mut bind_estimate: Option<Option<String>> = None;
    let mut bind_parent_id: Option<Option<Uuid>> = None;
    let mut bind_milestone_id: Option<Option<Uuid>> = None;
    let mut bind_recurrence: Option<Option<Value>> = None;
    let mut bind_archived_at: Option<Option<DateTime<Utc>>> = None;

    let mut payload = serde_json::Map::new();
    payload.insert("taskId".to_string(), json!(task_id.to_string()));

    if let Some(title) = &input.title {
        sets.push(format!("title = ${bind_idx}"));
        bind_idx += 1;
        bind_title = Some(title.clone());
        payload.insert("title".to_string(), json!(title));
    }
    if let Some(task_type) = &input.task_type {
        sets.push(format!("type = ${bind_idx}"));
        bind_idx += 1;
        bind_type = Some(task_type.clone());
        payload.insert("type".to_string(), json!(task_type));
    }
    if let Some(priority) = &input.priority {
        sets.push(format!("priority = ${bind_idx}"));
        bind_idx += 1;
        bind_priority = Some(priority.clone());
        payload.insert("priority".to_string(), json!(priority));
    }
    if input.start_date != crate::tasks::patch::FieldUpdate::Unchanged {
        sets.push(format!("start_date = ${bind_idx}"));
        bind_idx += 1;
        bind_start_date = Some(match input.start_date {
            crate::tasks::patch::FieldUpdate::Clear => None,
            crate::tasks::patch::FieldUpdate::Set(value) => Some(value),
            crate::tasks::patch::FieldUpdate::Unchanged => unreachable!(),
        });
        payload.insert(
            "startDate".to_string(),
            json!(bind_start_date.as_ref().unwrap()),
        );
    }
    if input.due_date != crate::tasks::patch::FieldUpdate::Unchanged {
        sets.push(format!("due_date = ${bind_idx}"));
        bind_idx += 1;
        bind_due_date = Some(match input.due_date {
            crate::tasks::patch::FieldUpdate::Clear => None,
            crate::tasks::patch::FieldUpdate::Set(value) => Some(value),
            crate::tasks::patch::FieldUpdate::Unchanged => unreachable!(),
        });
        payload.insert(
            "dueDate".to_string(),
            json!(bind_due_date.as_ref().unwrap()),
        );
    }
    if input.due_at != crate::tasks::patch::FieldUpdate::Unchanged {
        sets.push(format!("due_at = ${bind_idx}"));
        bind_idx += 1;
        bind_due_at = Some(match input.due_at {
            crate::tasks::patch::FieldUpdate::Clear => None,
            crate::tasks::patch::FieldUpdate::Set(value) => Some(value),
            crate::tasks::patch::FieldUpdate::Unchanged => unreachable!(),
        });
        payload.insert("dueAt".to_string(), json!(bind_due_at.as_ref().unwrap()));
    }
    if input.estimate != crate::tasks::patch::FieldUpdate::Unchanged {
        sets.push(format!("estimate = ${bind_idx}::numeric"));
        bind_idx += 1;
        bind_estimate = Some(match &input.estimate {
            crate::tasks::patch::FieldUpdate::Clear => None,
            crate::tasks::patch::FieldUpdate::Set(value) => Some(value.clone()),
            crate::tasks::patch::FieldUpdate::Unchanged => unreachable!(),
        });
        payload.insert(
            "estimate".to_string(),
            json!(bind_estimate.as_ref().unwrap()),
        );
    }
    if input.parent_id != crate::tasks::patch::FieldUpdate::Unchanged {
        sets.push(format!("parent_id = ${bind_idx}"));
        bind_idx += 1;
        bind_parent_id = Some(match input.parent_id {
            crate::tasks::patch::FieldUpdate::Clear => None,
            crate::tasks::patch::FieldUpdate::Set(value) => Some(value),
            crate::tasks::patch::FieldUpdate::Unchanged => unreachable!(),
        });
        payload.insert(
            "parentId".to_string(),
            json!(bind_parent_id.as_ref().unwrap().map(|id| id.to_string())),
        );
    }
    if input.milestone_id != crate::tasks::patch::FieldUpdate::Unchanged {
        sets.push(format!("milestone_id = ${bind_idx}"));
        bind_idx += 1;
        bind_milestone_id = Some(match input.milestone_id {
            crate::tasks::patch::FieldUpdate::Clear => None,
            crate::tasks::patch::FieldUpdate::Set(value) => Some(value),
            crate::tasks::patch::FieldUpdate::Unchanged => unreachable!(),
        });
        payload.insert(
            "milestoneId".to_string(),
            json!(bind_milestone_id.as_ref().unwrap().map(|id| id.to_string())),
        );
    }
    if input.recurrence != crate::tasks::patch::FieldUpdate::Unchanged {
        sets.push(format!("recurrence = ${bind_idx}"));
        bind_idx += 1;
        bind_recurrence = Some(match &input.recurrence {
            crate::tasks::patch::FieldUpdate::Clear => None,
            crate::tasks::patch::FieldUpdate::Set(value) => Some(value.clone()),
            crate::tasks::patch::FieldUpdate::Unchanged => unreachable!(),
        });
        payload.insert(
            "recurrence".to_string(),
            json!(bind_recurrence.as_ref().unwrap()),
        );
    }
    if let Some(archived) = input.archived {
        sets.push(format!("archived_at = ${bind_idx}"));
        bind_archived_at = Some(if archived { Some(Utc::now()) } else { None });
        payload.insert("archived".to_string(), json!(archived));
    }

    let meta_changed = !sets.is_empty();
    let row = if meta_changed {
        sets.push("updated_at = now()".to_string());
        let sql = format!(
            r#"
            UPDATE fvoci.tasks
            SET {}
            WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL
            RETURNING id, project_id, number, title, type AS task_type, priority, status_id,
                      start_date, due_date, due_at, estimate::text AS estimate,
                      parent_id, milestone_id, sort_key, schema_version, version, archived_at,
                      created_by, created_at, updated_at
            "#,
            sets.join(", ")
        );
        let mut query = sqlx::query(&sql).bind(workspace_id).bind(task_id);
        if let Some(title) = bind_title {
            query = query.bind(title);
        }
        if let Some(task_type) = bind_type {
            query = query.bind(task_type);
        }
        if let Some(priority) = bind_priority {
            query = query.bind(priority);
        }
        if let Some(start_date) = bind_start_date {
            query = query.bind(start_date);
        }
        if let Some(due_date) = bind_due_date {
            query = query.bind(due_date);
        }
        if let Some(due_at) = bind_due_at {
            query = query.bind(due_at);
        }
        if let Some(estimate) = bind_estimate {
            query = query.bind(estimate);
        }
        if let Some(parent_id) = bind_parent_id {
            query = query.bind(parent_id);
        }
        if let Some(milestone_id) = bind_milestone_id {
            query = query.bind(milestone_id);
        }
        if let Some(recurrence) = bind_recurrence {
            query = query.bind(recurrence);
        }
        if let Some(archived_at) = bind_archived_at {
            query = query.bind(archived_at);
        }
        let row = query.fetch_optional(&mut *tx).await?;
        let Some(row) = row else {
            tx.rollback().await?;
            return Ok(Err(ProjectDbError::NotFound));
        };
        map_task_row(&row)?
    } else if status_changed || input.assignee_ids.is_some() || input.label_ids.is_some() {
        let row = sqlx::query(
            r#"
            SELECT id, project_id, number, title, type AS task_type, priority, status_id,
                   start_date, due_date, due_at, estimate::text AS estimate, parent_id,
                   milestone_id, sort_key, schema_version, version, archived_at, created_by,
                   created_at, updated_at
            FROM fvoci.tasks
            WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL
            "#,
        )
        .bind(workspace_id)
        .bind(task_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(row) = row else {
            tx.rollback().await?;
            return Ok(Err(ProjectDbError::NotFound));
        };
        map_task_row(&row)?
    } else {
        tx.rollback().await?;
        return Ok(Ok(row_to_meta(workspace_id, task.record, task.recurrence)));
    };

    let recurrence = load_task_recurrence(&mut tx, workspace_id, task_id).await?;
    if status_changed {
        record_task_event_and_audit(
            &mut tx,
            TaskChangeRecord {
                workspace_id,
                actor_user_id,
                verb: "task.updated",
                target_type: "task",
                target_id: task_id,
                payload: json!({
                    "taskId": task_id.to_string(),
                    "from": from_status_id.to_string(),
                    "to": row.status_id.to_string(),
                }),
                client_ip,
            },
        )
        .await?;
    }
    if meta_changed {
        record_task_event_and_audit(
            &mut tx,
            TaskChangeRecord {
                workspace_id,
                actor_user_id,
                verb: "task.updated",
                target_type: "task",
                target_id: task_id,
                payload: Value::Object(payload),
                client_ip,
            },
        )
        .await?;
    }
    if let Some(assignee_ids) = &input.assignee_ids {
        match replace_task_assignees(
            &mut tx,
            workspace_id,
            actor_user_id,
            task_id,
            assignee_ids,
            client_ip,
        )
        .await?
        {
            Ok(()) => {}
            Err(err) => {
                tx.rollback().await?;
                return Ok(Err(err));
            }
        }
    }
    if let Some(label_ids) = &input.label_ids {
        match replace_task_labels(
            &mut tx,
            workspace_id,
            actor_user_id,
            row.project_id,
            task_id,
            label_ids,
            client_ip,
        )
        .await?
        {
            Ok(()) => {}
            Err(err) => {
                tx.rollback().await?;
                return Ok(Err(err));
            }
        }
    }

    tx.commit().await?;
    Ok(Ok(row_to_meta(workspace_id, row, recurrence)))
}

pub async fn move_task(
    pool: &PgPool,
    workspace_id: Uuid,
    task_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    input: crate::tasks::patch::MoveTaskInput,
    client_ip: Option<&str>,
) -> Result<Result<TaskMetaRow, ProjectDbError>, sqlx::Error> {
    if input.before_id.is_some() && input.after_id.is_some() {
        return Ok(Err(ProjectDbError::InvalidMoveAnchors));
    }
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let task = match require_task_write_access(
        &mut tx,
        workspace_id,
        actor_user_id,
        session_id,
        task_id,
        false,
    )
    .await?
    {
        Ok(task) => task,
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    };
    if let Some(expected) = input.expected_status_id {
        if expected != task.record.status_id {
            tx.rollback().await?;
            return Ok(Err(ProjectDbError::VersionConflict));
        }
    }
    let from_status_id = task.record.status_id;
    let result = transition_task_status(
        &mut tx,
        workspace_id,
        task.record.project_id,
        actor_user_id,
        &task,
        task.record.status_id,
        input.status_id,
        input.before_id,
        input.after_id,
        None,
    )
    .await?;
    let transition = match result {
        Ok(value) => value,
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    };
    if transition.status_changed {
        record_task_event_and_audit(
            &mut tx,
            TaskChangeRecord {
                workspace_id,
                actor_user_id,
                verb: "task.updated",
                target_type: "task",
                target_id: task_id,
                payload: json!({
                    "taskId": task_id.to_string(),
                    "from": from_status_id.to_string(),
                    "to": input.status_id.to_string(),
                }),
                client_ip,
            },
        )
        .await?;
    }
    let row = sqlx::query(
        r#"
        SELECT id, project_id, number, title, type AS task_type, priority, status_id,
               start_date, due_date, due_at, estimate::text AS estimate,
               parent_id, milestone_id, sort_key, schema_version, version, archived_at,
               created_by, created_at, updated_at
        FROM fvoci.tasks
        WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL
        "#,
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    };
    let row = map_task_row(&row)?;
    let recurrence = load_task_recurrence(&mut tx, workspace_id, task_id).await?;
    tx.commit().await?;
    Ok(Ok(row_to_meta(workspace_id, row, recurrence)))
}

pub async fn trash_task(
    pool: &PgPool,
    workspace_id: Uuid,
    task_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    client_ip: Option<&str>,
) -> Result<Result<(), ProjectDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let task = match require_task_write_access(
        &mut tx,
        workspace_id,
        actor_user_id,
        session_id,
        task_id,
        false,
    )
    .await?
    {
        Ok(task) => task,
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    };
    let updated = sqlx::query(
        r#"
        UPDATE fvoci.tasks
        SET deleted_at = now(), updated_at = now()
        WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL
        "#,
    )
    .bind(workspace_id)
    .bind(task_id)
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() == 0 {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }
    let assignee_ids = list_task_assignee_ids(&mut tx, workspace_id, task_id).await?;
    record_task_event_and_audit(
        &mut tx,
        TaskChangeRecord {
            workspace_id,
            actor_user_id,
            verb: "task.deleted",
            target_type: "task",
            target_id: task_id,
            payload: json!({
                "taskId": task_id.to_string(),
                "projectId": task.record.project_id.to_string(),
                "number": task.record.number,
                "assigneeIds": uuid_strings(&assignee_ids),
            }),
            client_ip,
        },
    )
    .await?;
    tx.commit().await?;
    Ok(Ok(()))
}

pub async fn restore_task(
    pool: &PgPool,
    workspace_id: Uuid,
    task_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    client_ip: Option<&str>,
) -> Result<Result<(), ProjectDbError>, sqlx::Error> {
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
    let row: Option<(Uuid,)> = sqlx::query_as(
        r#"
        SELECT project_id
        FROM fvoci.tasks
        WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NOT NULL
        "#,
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((project_id,)) = row else {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    };
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
    let updated = sqlx::query(
        r#"
        UPDATE fvoci.tasks
        SET deleted_at = NULL, updated_at = now()
        WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NOT NULL
        "#,
    )
    .bind(workspace_id)
    .bind(task_id)
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() == 0 {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }
    record_task_event_and_audit(
        &mut tx,
        TaskChangeRecord {
            workspace_id,
            actor_user_id,
            verb: "task.restored",
            target_type: "task",
            target_id: task_id,
            payload: json!({ "taskId": task_id.to_string() }),
            client_ip,
        },
    )
    .await?;
    tx.commit().await?;
    Ok(Ok(()))
}

async fn lock_tasks_sorted(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    ids: &[Uuid],
) -> Result<(), sqlx::Error> {
    let mut sorted = ids.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    for id in sorted {
        sqlx::query(
            r#"
            SELECT id FROM fvoci.tasks
            WHERE workspace_id = $1 AND id = $2
            FOR NO KEY UPDATE
            "#,
        )
        .bind(workspace_id)
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?;
    }
    Ok(())
}

type DependencySqlRow = (Uuid, Uuid, String, i32);

fn map_dependency_row(
    blocker_id: Uuid,
    blocked_id: Uuid,
    dependency_type: String,
    lag_days: i32,
) -> TaskDependencyEdge {
    TaskDependencyEdge {
        blocker_id,
        blocked_id,
        dependency_type,
        lag_days,
    }
}

async fn list_task_dependency_edges(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    task_id: Uuid,
) -> Result<Vec<TaskDependencyEdge>, sqlx::Error> {
    let rows = sqlx::query_as::<_, DependencySqlRow>(
        r#"
        SELECT blocker_id, blocked_id, type, lag_days
        FROM fvoci.task_dependencies
        WHERE workspace_id = $1 AND (blocker_id = $2 OR blocked_id = $2)
        ORDER BY blocker_id, blocked_id
        "#,
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(blocker_id, blocked_id, dependency_type, lag_days)| {
            map_dependency_row(blocker_id, blocked_id, dependency_type, lag_days)
        })
        .collect())
}

async fn inspect_addition_creates_cycle(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    project_id: Uuid,
    blocker_id: Uuid,
    blocked_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let creates: (bool,) = sqlx::query_as(
        r#"
        WITH RECURSIVE reachable(id) AS (
            VALUES ($4::uuid)
            UNION
            SELECT d.blocked_id
            FROM fvoci.task_dependencies d
            INNER JOIN reachable r ON r.id = d.blocker_id
            INNER JOIN fvoci.tasks t
                ON t.id = d.blocker_id AND t.workspace_id = d.workspace_id
            WHERE d.workspace_id = $1 AND t.project_id = $2
        )
        SELECT EXISTS(SELECT 1 FROM reachable WHERE id = $3)
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .bind(blocker_id)
    .bind(blocked_id)
    .fetch_one(&mut **tx)
    .await?;
    Ok(creates.0)
}

async fn load_task_schedule(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    task_id: Uuid,
) -> Result<Option<(Uuid, Option<DateTime<Utc>>, ScheduleEnds)>, sqlx::Error> {
    type ScheduleSqlRow = (
        Uuid,
        Option<DateTime<Utc>>,
        Option<NaiveDate>,
        Option<NaiveDate>,
        Option<DateTime<Utc>>,
    );
    let row: Option<ScheduleSqlRow> = sqlx::query_as(
        r#"
        SELECT project_id, archived_at, start_date, due_date, due_at
        FROM fvoci.tasks
        WHERE workspace_id = $1 AND id = $2
        "#,
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(
        row.map(|(project_id, archived_at, start_date, due_date, due_at)| {
            (
                project_id,
                archived_at,
                schedule_ends(start_date, due_date, due_at),
            )
        }),
    )
}

fn merged_schedule_ends(
    record: &TaskRowRecord,
    input: &crate::tasks::patch::PatchTaskMetaInput,
) -> ScheduleEnds {
    let start_date = match input.start_date {
        crate::tasks::patch::FieldUpdate::Set(value) => Some(value),
        crate::tasks::patch::FieldUpdate::Clear => None,
        crate::tasks::patch::FieldUpdate::Unchanged => record.start_date,
    };
    let due_date = match input.due_date {
        crate::tasks::patch::FieldUpdate::Set(value) => Some(value),
        crate::tasks::patch::FieldUpdate::Clear => None,
        crate::tasks::patch::FieldUpdate::Unchanged => record.due_date,
    };
    let due_at = match input.due_at {
        crate::tasks::patch::FieldUpdate::Set(value) => Some(value),
        crate::tasks::patch::FieldUpdate::Clear => None,
        crate::tasks::patch::FieldUpdate::Unchanged => record.due_at,
    };
    ScheduleEnds {
        start_date,
        due_date: finish_date(due_date, due_at),
    }
}

async fn assert_dependency_dates_ok(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    record: &TaskRowRecord,
    input: &crate::tasks::patch::PatchTaskMetaInput,
) -> Result<Result<(), ProjectDbError>, sqlx::Error> {
    let due_after = match input.due_date {
        crate::tasks::patch::FieldUpdate::Set(value) => Some(value),
        crate::tasks::patch::FieldUpdate::Clear => None,
        crate::tasks::patch::FieldUpdate::Unchanged => record.due_date,
    };
    if input.start_date == crate::tasks::patch::FieldUpdate::Unchanged
        && input.due_date == crate::tasks::patch::FieldUpdate::Unchanged
        && due_after.is_some()
    {
        return Ok(Ok(()));
    }
    let holidays = HashSet::new();
    let merged = merged_schedule_ends(record, input);
    let edges = list_task_dependency_edges(tx, workspace_id, record.id).await?;
    for edge in edges {
        let other_id = if edge.blocker_id == record.id {
            edge.blocked_id
        } else {
            edge.blocker_id
        };
        let Some((_, _, other_ends)) = load_task_schedule(tx, workspace_id, other_id).await? else {
            continue;
        };
        let Some(dep_type) = DependencyType::parse(&edge.dependency_type) else {
            continue;
        };
        let blocker_dates = if edge.blocker_id == record.id {
            merged
        } else {
            other_ends
        };
        let blocked_dates = if edge.blocked_id == record.id {
            merged
        } else {
            other_ends
        };
        if required_dates_present(dep_type, blocker_dates, blocked_dates)
            && violates_inequality(
                dep_type,
                edge.lag_days,
                blocker_dates,
                blocked_dates,
                &holidays,
            )
        {
            return Ok(Err(ProjectDbError::DependencyContradiction));
        }
    }
    Ok(Ok(()))
}

pub async fn list_project_dependencies(
    pool: &PgPool,
    workspace_id: Uuid,
    project_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<Result<Vec<TaskDependencyEdge>, ProjectDbError>, sqlx::Error> {
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
    let Some(locked) = lock_project(&mut tx, workspace_id, project_id).await? else {
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
    let rows = sqlx::query_as::<_, DependencySqlRow>(
        r#"
        SELECT d.blocker_id, d.blocked_id, d.type, d.lag_days
        FROM fvoci.task_dependencies d
        INNER JOIN fvoci.tasks t
            ON t.workspace_id = d.workspace_id AND t.id = d.blocker_id
        WHERE d.workspace_id = $1 AND t.project_id = $2
        ORDER BY d.blocker_id, d.blocked_id
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Ok(rows
        .into_iter()
        .map(|(blocker_id, blocked_id, dependency_type, lag_days)| {
            map_dependency_row(blocker_id, blocked_id, dependency_type, lag_days)
        })
        .collect()))
}

#[allow(clippy::too_many_arguments)]
pub async fn add_task_dependency(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    blocker_id: Uuid,
    blocked_id: Uuid,
    requested_type: Option<DependencyType>,
    requested_lag: Option<i32>,
) -> Result<Result<(), ProjectDbError>, sqlx::Error> {
    if blocker_id == blocked_id {
        return Ok(Err(ProjectDbError::TaskCannotBlockItself));
    }
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
    let blocker_project: Option<(Uuid,)> =
        sqlx::query_as("SELECT project_id FROM fvoci.tasks WHERE workspace_id = $1 AND id = $2")
            .bind(workspace_id)
            .bind(blocker_id)
            .fetch_optional(&mut *tx)
            .await?;
    let Some((project_id,)) = blocker_project else {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    };
    let Some(locked) = lock_project(&mut tx, workspace_id, project_id).await? else {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    };
    if locked.status == "archived" {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::Archived));
    }
    if !project_permission(&mut tx, workspace_id, actor_user_id, &locked)
        .await?
        .at_least(ProjectPermission::Edit)
    {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }
    lock_tasks_sorted(&mut tx, workspace_id, &[blocker_id, blocked_id]).await?;
    let Some((blocker_project_id, blocker_archived, blocker_ends)) =
        load_task_schedule(&mut tx, workspace_id, blocker_id).await?
    else {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    };
    let Some((blocked_project_id, blocked_archived, blocked_ends)) =
        load_task_schedule(&mut tx, workspace_id, blocked_id).await?
    else {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    };
    if blocker_project_id != blocked_project_id {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }
    if blocker_archived.is_some() || blocked_archived.is_some() {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::TaskArchived));
    }
    if inspect_addition_creates_cycle(
        &mut tx,
        workspace_id,
        blocker_project_id,
        blocker_id,
        blocked_id,
    )
    .await?
    {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::DependencyCycle));
    }
    let existing: Option<(String, i32)> = sqlx::query_as(
        r#"
        SELECT type, lag_days FROM fvoci.task_dependencies
        WHERE workspace_id = $1 AND blocker_id = $2 AND blocked_id = $3
        "#,
    )
    .bind(workspace_id)
    .bind(blocker_id)
    .bind(blocked_id)
    .fetch_optional(&mut *tx)
    .await?;
    let dep_type = requested_type
        .or_else(|| {
            existing
                .as_ref()
                .and_then(|(value, _)| DependencyType::parse(value))
        })
        .unwrap_or(DependencyType::Fs);
    let lag_days = requested_lag.or(existing.map(|(_, lag)| lag)).unwrap_or(0);
    let holidays = HashSet::new();
    if required_dates_present(dep_type, blocker_ends, blocked_ends)
        && violates_inequality(dep_type, lag_days, blocker_ends, blocked_ends, &holidays)
    {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::DependencyContradiction));
    }
    sqlx::query(
        r#"
        INSERT INTO fvoci.task_dependencies (workspace_id, blocker_id, blocked_id, type, lag_days)
        VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT (blocker_id, blocked_id) DO UPDATE
        SET type = EXCLUDED.type, lag_days = EXCLUDED.lag_days
        "#,
    )
    .bind(workspace_id)
    .bind(blocker_id)
    .bind(blocked_id)
    .bind(dep_type.as_str())
    .bind(lag_days)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Ok(()))
}

pub async fn remove_task_dependency(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    blocker_id: Uuid,
    blocked_id: Uuid,
) -> Result<Result<(), ProjectDbError>, sqlx::Error> {
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
    let blocker_project: Option<(Uuid,)> =
        sqlx::query_as("SELECT project_id FROM fvoci.tasks WHERE workspace_id = $1 AND id = $2")
            .bind(workspace_id)
            .bind(blocker_id)
            .fetch_optional(&mut *tx)
            .await?;
    let Some((project_id,)) = blocker_project else {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    };
    let Some(locked) = lock_project(&mut tx, workspace_id, project_id).await? else {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    };
    if locked.status == "archived" {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::Archived));
    }
    if !project_permission(&mut tx, workspace_id, actor_user_id, &locked)
        .await?
        .at_least(ProjectPermission::Edit)
    {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }
    lock_tasks_sorted(&mut tx, workspace_id, &[blocker_id, blocked_id]).await?;
    let Some((blocker_project_id, blocker_archived, _)) =
        load_task_schedule(&mut tx, workspace_id, blocker_id).await?
    else {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    };
    let Some((blocked_project_id, blocked_archived, _)) =
        load_task_schedule(&mut tx, workspace_id, blocked_id).await?
    else {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::DependencyNotFound));
    };
    if blocker_project_id != blocked_project_id {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::DependencyNotFound));
    }
    if blocker_archived.is_some() || blocked_archived.is_some() {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::TaskArchived));
    }
    let deleted = sqlx::query(
        r#"
        DELETE FROM fvoci.task_dependencies
        WHERE workspace_id = $1 AND blocker_id = $2 AND blocked_id = $3
        "#,
    )
    .bind(workspace_id)
    .bind(blocker_id)
    .bind(blocked_id)
    .execute(&mut *tx)
    .await?;
    if deleted.rows_affected() == 0 {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::DependencyNotFound));
    }
    tx.commit().await?;
    Ok(Ok(()))
}
