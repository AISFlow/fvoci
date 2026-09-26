//! Task operations beyond metadata edits (source `packages/core/src/task.ts`,
//! `time-entry.ts`, `reference.ts` at `393795261322b916e588043cf94feca999175843`):
//! time entries, clone, backlinks, workspace lookup for the flat `/tasks/:id`
//! routes, purge and parent candidates. Every write rechecks the actor's
//! credential, workspace and project access inside its transaction in the
//! task lock order: membership advisory lock → credential row → workspace →
//! project row → task row.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::db::context::{lock_membership_users, recheck_session, session_is_live, set_tenant};
use crate::db::documents::document_permission;
use crate::db::projects::{
    lock_project, project_permission, project_permission_by_id, LockedProject, ProjectDbError,
};
use crate::db::task_activity::record_task_activity;
use crate::db::tasks::{
    copy_task_assignees_and_labels, insert_task_in_locked_project, list_task_assignee_ids,
    record_task_event_and_audit, require_task_write_access, uuid_strings, workspace_is_live,
    CreateTaskInput, TaskChangeRecord, TaskMetaRow,
};
use crate::display_id::{format_display_id, parse_display_id};
use crate::projects::ProjectPermission;
use crate::tasks::activity::ActivitySnapshot;

/// A task the actor can currently view, with its locked project.
struct ViewableTask {
    project: LockedProject,
    archived_at: Option<DateTime<Utc>>,
    permission: ProjectPermission,
}

/// Read access in the `get_task` order: live credential, live workspace, live
/// task, project row lock, view permission. Hidden and missing are both 404.
async fn viewable_task(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    task_id: Uuid,
) -> Result<Result<ViewableTask, ProjectDbError>, sqlx::Error> {
    if !session_is_live(tx, actor_user_id, session_id).await? {
        return Ok(Err(ProjectDbError::Forbidden));
    }
    if !workspace_is_live(tx, workspace_id).await? {
        return Ok(Err(ProjectDbError::NotFound));
    }
    let row: Option<(Uuid, Option<DateTime<Utc>>)> = sqlx::query_as(
        r#"
        SELECT project_id, archived_at
        FROM fvoci.tasks
        WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL
        "#,
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some((project_id, archived_at)) = row else {
        return Ok(Err(ProjectDbError::NotFound));
    };
    let Some(project) = lock_project(tx, workspace_id, project_id).await? else {
        return Ok(Err(ProjectDbError::NotFound));
    };
    let permission = project_permission(tx, workspace_id, actor_user_id, &project).await?;
    if !permission.at_least(ProjectPermission::View) {
        return Ok(Err(ProjectDbError::NotFound));
    }
    Ok(Ok(ViewableTask {
        project,
        archived_at,
        permission,
    }))
}

// ---------------------------------------------------------------------------
// Time entries
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct TimeEntryRow {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub task_id: Uuid,
    pub user_id: Uuid,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub duration_seconds: Option<i32>,
    pub note: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TimeEntryList {
    pub items: Vec<TimeEntryRow>,
    /// Source `canCreate`: edit permission on a writable project and an
    /// unarchived task.
    pub can_create: bool,
}

pub struct CreateTimeEntryInput {
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub note: Option<String>,
}

/// Source `durationSecondsFromRange`: whole seconds, rounded down. `None` for a
/// non-positive range, which the source rejects as `invalid_input`.
pub fn time_entry_duration_seconds(
    started_at: DateTime<Utc>,
    ended_at: DateTime<Utc>,
) -> Option<i32> {
    let millis = (ended_at - started_at).num_milliseconds();
    let seconds = millis.div_euclid(1000);
    if seconds <= 0 {
        return None;
    }
    i32::try_from(seconds).ok()
}

type TimeEntryTuple = (
    Uuid,
    Uuid,
    Uuid,
    DateTime<Utc>,
    Option<DateTime<Utc>>,
    Option<i32>,
    Option<String>,
);

fn time_entry_row(workspace_id: Uuid, row: TimeEntryTuple) -> TimeEntryRow {
    let (id, task_id, user_id, started_at, ended_at, duration_seconds, note) = row;
    TimeEntryRow {
        id,
        workspace_id,
        task_id,
        user_id,
        started_at,
        ended_at,
        duration_seconds,
        note,
    }
}

pub async fn list_time_entries(
    pool: &PgPool,
    workspace_id: Uuid,
    task_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<Result<TimeEntryList, ProjectDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let task =
        match viewable_task(&mut tx, workspace_id, actor_user_id, session_id, task_id).await? {
            Ok(task) => task,
            Err(err) => {
                tx.rollback().await?;
                return Ok(Err(err));
            }
        };
    let rows: Vec<TimeEntryTuple> = sqlx::query_as(
        r#"
        SELECT id, task_id, user_id, started_at, ended_at, duration_seconds, note
        FROM fvoci.time_entries
        WHERE workspace_id = $1 AND task_id = $2
        ORDER BY started_at DESC, id
        "#,
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    let can_create = task.permission.at_least(ProjectPermission::Edit)
        && task.project.status != "archived"
        && task.archived_at.is_none();
    Ok(Ok(TimeEntryList {
        items: rows
            .into_iter()
            .map(|row| time_entry_row(workspace_id, row))
            .collect(),
        can_create,
    }))
}

pub async fn create_time_entry(
    pool: &PgPool,
    workspace_id: Uuid,
    task_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    input: CreateTimeEntryInput,
) -> Result<Result<TimeEntryRow, ProjectDbError>, sqlx::Error> {
    let duration_seconds = match input.ended_at {
        None => None,
        Some(ended_at) => match time_entry_duration_seconds(input.started_at, ended_at) {
            Some(seconds) => Some(seconds),
            None => return Ok(Err(ProjectDbError::InvalidInput)),
        },
    };
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if let Err(err) = require_task_write_access(
        &mut tx,
        workspace_id,
        actor_user_id,
        session_id,
        task_id,
        false,
    )
    .await?
    {
        tx.rollback().await?;
        return Ok(Err(err));
    }
    // The one-open-entry-per-actor index is partial, so an open entry that
    // already exists leaves this statement without a row instead of aborting.
    let row: Option<TimeEntryTuple> = sqlx::query_as(
        r#"
        INSERT INTO fvoci.time_entries (
            id, workspace_id, task_id, user_id, started_at, ended_at, duration_seconds, note
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        ON CONFLICT (workspace_id, user_id) WHERE ended_at IS NULL DO NOTHING
        RETURNING id, task_id, user_id, started_at, ended_at, duration_seconds, note
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(workspace_id)
    .bind(task_id)
    .bind(actor_user_id)
    .bind(input.started_at)
    .bind(input.ended_at)
    .bind(duration_seconds)
    .bind(input.note)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::OpenTimeEntryExists));
    };
    tx.commit().await?;
    Ok(Ok(time_entry_row(workspace_id, row)))
}

// ---------------------------------------------------------------------------
// Clone
// ---------------------------------------------------------------------------

/// Source `task.duplicate.suffix` (ko): "{{title}} 복사", or the plain title
/// when the labelled one would exceed the title limit.
pub fn clone_title(source_title: &str) -> String {
    let labeled = format!("{source_title} 복사");
    if crate::tasks::title_is_valid(&labeled) {
        labeled
    } else {
        source_title.to_string()
    }
}

pub struct ClonedTask {
    pub meta: TaskMetaRow,
    pub display_id: String,
}

/// Source `cloneTask`: a copy in the source task's project with the same type,
/// parent, status, priority, dates, milestone, assignees and labels (no body,
/// estimate, due time or recurrence). Creating the copy needs the same right as
/// creating a task there — edit on a writable project — and the source task
/// must be writable. Unlike the source's three transactions, the checks, the
/// insert and the assignee/label copy commit together.
pub async fn clone_task(
    pool: &PgPool,
    workspace_id: Uuid,
    task_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    client_ip: Option<&str>,
    channel: &str,
) -> Result<Result<ClonedTask, ProjectDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let source = match require_task_write_access(
        &mut tx,
        workspace_id,
        actor_user_id,
        session_id,
        task_id,
        false,
    )
    .await?
    {
        Ok(task) => task.record,
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    };
    let project_key: String =
        sqlx::query_scalar("SELECT key FROM fvoci.projects WHERE workspace_id = $1 AND id = $2")
            .bind(workspace_id)
            .bind(source.project_id)
            .fetch_one(&mut *tx)
            .await?;
    let title = clone_title(&source.title);
    let input = CreateTaskInput {
        title: &title,
        task_type: &source.task_type,
        priority: &source.priority,
        status_id: Some(source.status_id),
        start_date: source.start_date,
        due_date: source.due_date,
        parent_id: source.parent_id,
        milestone_id: source.milestone_id,
        recurrence: None,
    };
    let copy = match insert_task_in_locked_project(
        &mut tx,
        workspace_id,
        source.project_id,
        actor_user_id,
        &input,
        client_ip,
        channel,
    )
    .await?
    {
        Ok(copy) => copy,
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    };
    copy_task_assignees_and_labels(&mut tx, workspace_id, source.id, copy.id).await?;
    tx.commit().await?;
    let display_id = format_display_id(&project_key, copy.number);
    Ok(Ok(ClonedTask {
        meta: copy,
        display_id,
    }))
}

// ---------------------------------------------------------------------------
// Backlinks
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BacklinkSource {
    Document,
    Task,
}

impl BacklinkSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Document => "document",
            Self::Task => "task",
        }
    }
}

#[derive(Debug, Clone)]
pub struct TaskBacklink {
    pub kind: BacklinkSource,
    pub id: Uuid,
    pub title: String,
    pub display_id: Option<String>,
}

/// JSONPath form of source `extractInternalRefs` for one task target: a
/// `mention` (`attrs.id`) or `embed` (`attrs.ref`) whose `attrs.entity` is
/// `task`. Only the canonical UUID text of the target enters the path.
fn task_ref_jsonpath(task_id: Uuid) -> String {
    let id = task_id.hyphenated().to_string();
    format!(
        r#"$.** ? ((@.type == "mention" && @.attrs.entity == "task" && @.attrs.id like_regex "^{id}$" flag "i") || (@.type == "embed" && @.attrs.entity == "task" && @.attrs.ref like_regex "^{id}$" flag "i"))"#
    )
}

/// Source `listTaskBacklinks`: live documents and tasks whose current body
/// references the task, limited to items the actor can view (wiki documents
/// by document permission, project items by project view permission). The
/// references are read from the stored bodies instead of a reference table.
pub async fn list_task_backlinks(
    pool: &PgPool,
    workspace_id: Uuid,
    task_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<Result<Vec<TaskBacklink>, ProjectDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if let Err(err) =
        viewable_task(&mut tx, workspace_id, actor_user_id, session_id, task_id).await?
    {
        tx.rollback().await?;
        return Ok(Err(err));
    }
    let path = task_ref_jsonpath(task_id);
    let documents = sqlx::query_as::<_, (Uuid, String, Option<Uuid>, i32, Option<String>)>(
        r#"
        SELECT d.id, d.title, d.project_id, d.number, p.key
        FROM fvoci.documents d
        LEFT JOIN fvoci.projects p
          ON p.workspace_id = d.workspace_id AND p.id = d.project_id AND p.deleted_at IS NULL
        WHERE d.workspace_id = $1
          AND d.deleted_at IS NULL
          AND jsonb_path_exists(d.content_json, $2::jsonpath)
        "#,
    )
    .bind(workspace_id)
    .bind(&path)
    .fetch_all(&mut *tx)
    .await?;
    let tasks = sqlx::query_as::<_, (Uuid, String, Uuid, i32, Option<String>)>(
        r#"
        SELECT t.id, t.title, t.project_id, t.number, p.key
        FROM fvoci.tasks t
        LEFT JOIN fvoci.projects p
          ON p.workspace_id = t.workspace_id AND p.id = t.project_id AND p.deleted_at IS NULL
        WHERE t.workspace_id = $1
          AND t.id <> $2
          AND t.deleted_at IS NULL
          AND jsonb_path_exists(t.content_json, $3::jsonpath)
        "#,
    )
    .bind(workspace_id)
    .bind(task_id)
    .bind(&path)
    .fetch_all(&mut *tx)
    .await?;

    let mut project_views: HashMap<Uuid, bool> = HashMap::new();
    let mut items = Vec::new();
    for (id, title, project_id, number, key) in documents {
        let visible = match project_id {
            None => document_permission(&mut tx, workspace_id, actor_user_id, id, true)
                .await?
                .at_least(ProjectPermission::View),
            Some(project_id) => {
                project_visible(
                    &mut tx,
                    &mut project_views,
                    workspace_id,
                    actor_user_id,
                    project_id,
                )
                .await?
            }
        };
        if !visible {
            continue;
        }
        let display_id = match (project_id, key) {
            (None, _) => Some(format_display_id("WIKI", number)),
            (Some(_), Some(key)) => Some(format_display_id(&key, number)),
            (Some(_), None) => None,
        };
        items.push(TaskBacklink {
            kind: BacklinkSource::Document,
            id,
            title,
            display_id,
        });
    }
    for (id, title, project_id, number, key) in tasks {
        if !project_visible(
            &mut tx,
            &mut project_views,
            workspace_id,
            actor_user_id,
            project_id,
        )
        .await?
        {
            continue;
        }
        items.push(TaskBacklink {
            kind: BacklinkSource::Task,
            id,
            title,
            display_id: key.map(|key| format_display_id(&key, number)),
        });
    }
    tx.commit().await?;
    // Source sorts by the reference row id; the referencing item id stands in.
    items.sort_by_key(|item| item.id);
    Ok(Ok(items))
}

async fn project_visible(
    tx: &mut Transaction<'_, Postgres>,
    cache: &mut HashMap<Uuid, bool>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    project_id: Uuid,
) -> Result<bool, sqlx::Error> {
    if let Some(visible) = cache.get(&project_id) {
        return Ok(*visible);
    }
    let visible = project_permission_by_id(tx, workspace_id, actor_user_id, project_id)
        .await?
        .is_some_and(|permission| permission.at_least(ProjectPermission::View));
    cache.insert(project_id, visible);
    Ok(visible)
}

// ---------------------------------------------------------------------------
// Flat `/tasks/:taskId` lookup
// ---------------------------------------------------------------------------

/// Source `workspaceIdForTask`: the actor's workspace that holds the task
/// (trashed included; each route applies its own access checks). Only the
/// workspaces the actor is a member of are searched, each under its own tenant.
pub async fn locate_task(
    pool: &PgPool,
    actor_user_id: Uuid,
    task_id: Uuid,
) -> Result<Option<Uuid>, sqlx::Error> {
    let mut workspaces: Vec<Uuid> =
        crate::db::workspace::list_workspaces_for_user(pool, actor_user_id)
            .await?
            .into_iter()
            .map(|workspace| workspace.id)
            .collect();
    workspaces.sort();
    for workspace_id in workspaces {
        let mut tx = pool.begin().await?;
        set_tenant(&mut tx, workspace_id).await?;
        let found: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM fvoci.tasks WHERE workspace_id = $1 AND id = $2)",
        )
        .bind(workspace_id)
        .bind(task_id)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        if found {
            return Ok(Some(workspace_id));
        }
    }
    Ok(None)
}

// ---------------------------------------------------------------------------
// Purge
// ---------------------------------------------------------------------------

/// Source `purgeTask` (session only): permanently deletes a live or trashed
/// task the actor may edit (writable project, unarchived task). Attachment rows
/// are deleted in the same transaction, so their object keys enter the cleanup
/// journal (`attachment_object_cleanups`); children are detached (a subtask
/// becomes a task) with their own activity and event. Comments, activity,
/// dependencies, assignees, labels, stars, time entries and collection items
/// cascade. Returns the task's project.
pub async fn purge_task(
    pool: &PgPool,
    workspace_id: Uuid,
    task_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    client_ip: Option<&str>,
) -> Result<Result<Uuid, ProjectDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    match purge_task_in_tx(
        &mut tx,
        workspace_id,
        task_id,
        actor_user_id,
        session_id,
        client_ip,
    )
    .await?
    {
        Ok(project_id) => {
            tx.commit().await?;
            Ok(Ok(project_id))
        }
        Err(err) => {
            tx.rollback().await?;
            Ok(Err(err))
        }
    }
}

async fn purge_task_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    task_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    client_ip: Option<&str>,
) -> Result<Result<Uuid, ProjectDbError>, sqlx::Error> {
    lock_membership_users(tx, &[actor_user_id]).await?;
    if !recheck_session(tx, actor_user_id, session_id).await? {
        return Ok(Err(ProjectDbError::Forbidden));
    }
    if !workspace_is_live(tx, workspace_id).await? {
        return Ok(Err(ProjectDbError::NotFound));
    }
    let project_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT project_id FROM fvoci.tasks WHERE workspace_id = $1 AND id = $2",
    )
    .bind(workspace_id)
    .bind(task_id)
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
        .at_least(ProjectPermission::Edit)
    {
        return Ok(Err(ProjectDbError::NotFound));
    }
    if project.status == "archived" {
        return Ok(Err(ProjectDbError::Archived));
    }
    let task: Option<(Uuid, i32, Option<DateTime<Utc>>)> = sqlx::query_as(
        r#"
        SELECT project_id, number, archived_at
        FROM fvoci.tasks
        WHERE workspace_id = $1 AND id = $2
        FOR UPDATE
        "#,
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_optional(&mut **tx)
    .await?;
    // A move to another project between the unlocked read and the row lock
    // would leave the checked project stale.
    let Some((locked_project_id, number, archived_at)) = task else {
        return Ok(Err(ProjectDbError::NotFound));
    };
    if locked_project_id != project_id {
        return Ok(Err(ProjectDbError::NotFound));
    }
    if archived_at.is_some() {
        return Ok(Err(ProjectDbError::TaskArchived));
    }
    let assignee_ids = list_task_assignee_ids(tx, workspace_id, task_id).await?;
    sqlx::query("DELETE FROM fvoci.attachments WHERE workspace_id = $1 AND task_id = $2")
        .bind(workspace_id)
        .bind(task_id)
        .execute(&mut **tx)
        .await?;
    detach_children(tx, workspace_id, task_id, actor_user_id, client_ip).await?;
    let deleted = sqlx::query("DELETE FROM fvoci.tasks WHERE workspace_id = $1 AND id = $2")
        .bind(workspace_id)
        .bind(task_id)
        .execute(&mut **tx)
        .await?
        .rows_affected();
    if deleted != 1 {
        return Ok(Err(ProjectDbError::NotFound));
    }
    record_task_event_and_audit(
        tx,
        TaskChangeRecord {
            workspace_id,
            actor_user_id,
            verb: "task.deleted",
            target_type: "task",
            target_id: task_id,
            payload: json!({
                "taskId": task_id.to_string(),
                "projectId": project_id.to_string(),
                "number": number,
                "assigneeIds": uuid_strings(&assignee_ids),
            }),
            client_ip,
        },
    )
    .await?;
    Ok(Ok(project_id))
}

/// Source `detachChildrenForTaskRemoval`: every child (live or trashed) loses
/// its parent; a subtask becomes a task.
async fn detach_children(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    parent_id: Uuid,
    actor_user_id: Uuid,
    client_ip: Option<&str>,
) -> Result<(), sqlx::Error> {
    let children: Vec<(Uuid, String)> = sqlx::query_as(
        r#"
        SELECT id, type
        FROM fvoci.tasks
        WHERE workspace_id = $1 AND parent_id = $2
        ORDER BY id
        FOR UPDATE
        "#,
    )
    .bind(workspace_id)
    .bind(parent_id)
    .fetch_all(&mut **tx)
    .await?;
    if children.is_empty() {
        return Ok(());
    }
    sqlx::query(
        r#"
        UPDATE fvoci.tasks
        SET parent_id = NULL,
            type = CASE WHEN type = 'subtask' THEN 'task' ELSE type END,
            updated_at = now()
        WHERE workspace_id = $1 AND parent_id = $2
        "#,
    )
    .bind(workspace_id)
    .bind(parent_id)
    .execute(&mut **tx)
    .await?;
    for (child_id, child_type) in children {
        let next_type = if child_type == "subtask" {
            "task"
        } else {
            child_type.as_str()
        };
        let mut before = ActivitySnapshot::new();
        before.insert("type".into(), Value::String(child_type.clone()));
        before.insert(
            "parentId".into(),
            json!({ "id": parent_id.to_string(), "label": Value::Null }),
        );
        let mut after = ActivitySnapshot::new();
        after.insert("type".into(), Value::String(next_type.to_string()));
        after.insert("parentId".into(), Value::Null);
        record_task_activity(
            tx,
            workspace_id,
            child_id,
            actor_user_id,
            "web",
            Some(&before),
            &after,
        )
        .await?;
        record_task_event_and_audit(
            tx,
            TaskChangeRecord {
                workspace_id,
                actor_user_id,
                verb: "task.updated",
                target_type: "task",
                target_id: child_id,
                payload: json!({
                    "taskId": child_id.to_string(),
                    "parentId": Value::Null,
                    "type": next_type,
                }),
                client_ip,
            },
        )
        .await?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Parent candidates
// ---------------------------------------------------------------------------

pub const TASK_PARENT_PAGE_MAX: i64 = 20;

#[derive(Debug, Clone)]
pub struct TaskParentQuery {
    /// Trimmed search text (title substring or a display id).
    pub q: String,
    pub child_type: String,
    pub exclude_task_id: Option<Uuid>,
    pub cursor: Option<String>,
    pub limit: i64,
}

#[derive(Debug, Clone)]
pub struct TaskParentCandidate {
    pub id: Uuid,
    pub title: String,
    pub display_id: String,
    pub task_type: String,
}

#[derive(Debug, Clone)]
pub struct TaskParentPage {
    pub items: Vec<TaskParentCandidate>,
    pub next_cursor: Option<String>,
}

/// Source `taskParentCursor` scope: sha256 of the JSON array
/// `[workspaceId, projectId, q, childType, excludeTaskId ?? null]`.
fn parent_cursor_scope(workspace_id: Uuid, project_id: Uuid, query: &TaskParentQuery) -> String {
    let scope = json!([
        workspace_id.to_string(),
        project_id.to_string(),
        query.q,
        query.child_type,
        query.exclude_task_id.map(|id| id.to_string()),
    ]);
    Sha256::digest(scope.to_string().as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn encode_parent_cursor(updated_at: DateTime<Utc>, id: Uuid, scope: &str) -> String {
    use base64::Engine;
    let payload = json!({
        "ua": updated_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        "id": id.to_string(),
        "f": scope,
    });
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload.to_string())
}

/// `Err(())` for a malformed cursor or one issued for another search.
fn decode_parent_cursor(raw: &str, scope: &str) -> Result<(DateTime<Utc>, Uuid), ()> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(raw.trim_end_matches('='))
        .map_err(|_| ())?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| ())?;
    let object = value.as_object().ok_or(())?;
    if object.len() != 3 {
        return Err(());
    }
    let ua = object
        .get("ua")
        .and_then(Value::as_str)
        .and_then(crate::tasks::parse_iso_datetime)
        .ok_or(())?;
    let id = object
        .get("id")
        .and_then(Value::as_str)
        .and_then(|id| Uuid::parse_str(id).ok())
        .ok_or(())?;
    let f = object.get("f").and_then(Value::as_str).ok_or(())?;
    if f != scope {
        return Err(());
    }
    Ok((ua, id))
}

/// Source `listTaskParents`: candidates for the parent of a `child_type` task
/// in a writable project the actor can view. A subtask's parent is a
/// task/bug/story, any other non-epic type's parent is an epic, and an epic
/// has none. A display id is an exact lookup in this project; otherwise the
/// title substring search pages by `(updated_at, id)` descending.
pub async fn list_task_parents(
    pool: &PgPool,
    workspace_id: Uuid,
    project_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    query: &TaskParentQuery,
) -> Result<Result<TaskParentPage, ProjectDbError>, sqlx::Error> {
    let scope = parent_cursor_scope(workspace_id, project_id, query);
    let after = match query.cursor.as_deref() {
        None => None,
        Some(raw) => match decode_parent_cursor(raw, &scope) {
            Ok(after) => Some(after),
            Err(()) => return Ok(Err(ProjectDbError::InvalidCursor)),
        },
    };
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
    let Some(project) = lock_project(&mut tx, workspace_id, project_id).await? else {
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
    if project.status == "archived" {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::Archived));
    }
    let types: &[&str] = match query.child_type.as_str() {
        "subtask" => &["task", "bug", "story"],
        "epic" => &[],
        _ => &["epic"],
    };
    let display = parse_display_id(&query.q);
    let empty = TaskParentPage {
        items: Vec::new(),
        next_cursor: None,
    };
    if types.is_empty()
        || display
            .as_ref()
            .is_some_and(|display| display.prefix != project.key.to_uppercase())
    {
        tx.commit().await?;
        return Ok(Ok(empty));
    }
    let types: Vec<String> = types.iter().map(|t| (*t).to_string()).collect();
    if let Some(display) = display {
        let found: Option<(Uuid, String, i32, String)> = sqlx::query_as(
            r#"
            SELECT id, title, number, type
            FROM fvoci.tasks
            WHERE workspace_id = $1 AND project_id = $2 AND number = $3
              AND deleted_at IS NULL AND archived_at IS NULL
              AND type = ANY($4)
              AND ($5::uuid IS NULL OR id <> $5)
            "#,
        )
        .bind(workspace_id)
        .bind(project_id)
        .bind(display.number)
        .bind(&types)
        .bind(query.exclude_task_id)
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        return Ok(Ok(TaskParentPage {
            items: found
                .into_iter()
                .map(|(id, title, number, task_type)| TaskParentCandidate {
                    id,
                    title,
                    display_id: format_display_id(&project.key, number),
                    task_type,
                })
                .collect(),
            next_cursor: None,
        }));
    }
    let rows: Vec<(Uuid, String, i32, String, DateTime<Utc>)> = sqlx::query_as(
        r#"
        SELECT t.id, t.title, t.number, t.type,
               date_trunc('milliseconds', t.updated_at) AS ua
        FROM fvoci.tasks t
        WHERE t.workspace_id = $1
          AND t.project_id = $2
          AND t.deleted_at IS NULL AND t.archived_at IS NULL
          AND t.type = ANY($3)
          AND ($4 = '' OR strpos(lower(t.title), lower($4)) > 0)
          AND ($5::uuid IS NULL OR t.id <> $5)
          AND ($6::timestamptz IS NULL
               OR (date_trunc('milliseconds', t.updated_at), t.id) < ($6::timestamptz, $7::uuid))
        ORDER BY ua DESC, t.id DESC
        LIMIT $8
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .bind(&types)
    .bind(&query.q)
    .bind(query.exclude_task_id)
    .bind(after.map(|(ua, _)| ua))
    .bind(after.map(|(_, id)| id))
    .bind(query.limit + 1)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    let has_more = rows.len() as i64 > query.limit;
    let page: Vec<_> = rows.into_iter().take(query.limit as usize).collect();
    let next_cursor = if has_more {
        page.last()
            .map(|(id, _, _, _, ua)| encode_parent_cursor(*ua, *id, &scope))
    } else {
        None
    };
    Ok(Ok(TaskParentPage {
        items: page
            .into_iter()
            .map(|(id, title, number, task_type, _)| TaskParentCandidate {
                id,
                title,
                display_id: format_display_id(&project.key, number),
                task_type,
            })
            .collect(),
        next_cursor,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_rounds_down_and_rejects_sub_second_ranges() {
        let start = DateTime::parse_from_rfc3339("2026-01-01T00:00:00.000Z")
            .unwrap()
            .with_timezone(&Utc);
        let plus = |ms| start + chrono::Duration::milliseconds(ms);
        assert_eq!(time_entry_duration_seconds(start, plus(1_999)), Some(1));
        assert_eq!(time_entry_duration_seconds(start, plus(60_000)), Some(60));
        assert_eq!(time_entry_duration_seconds(start, plus(999)), None);
        assert_eq!(time_entry_duration_seconds(start, plus(-5_000)), None);
    }

    #[test]
    fn clone_title_keeps_the_title_limit() {
        assert_eq!(clone_title("Fix login"), "Fix login 복사");
        let long = "a".repeat(498);
        assert_eq!(clone_title(&long), long);
    }

    #[test]
    fn parent_cursor_is_bound_to_its_search() {
        let ws = Uuid::now_v7();
        let project = Uuid::now_v7();
        let query = TaskParentQuery {
            q: "fix".into(),
            child_type: "task".into(),
            exclude_task_id: None,
            cursor: None,
            limit: 20,
        };
        let scope = parent_cursor_scope(ws, project, &query);
        let at = DateTime::parse_from_rfc3339("2026-01-01T00:00:00.123Z")
            .unwrap()
            .with_timezone(&Utc);
        let id = Uuid::now_v7();
        let cursor = encode_parent_cursor(at, id, &scope);
        assert_eq!(decode_parent_cursor(&cursor, &scope), Ok((at, id)));
        let other = parent_cursor_scope(
            ws,
            project,
            &TaskParentQuery {
                q: "other".into(),
                ..query
            },
        );
        assert!(decode_parent_cursor(&cursor, &other).is_err());
        assert!(decode_parent_cursor("not-base64!", &scope).is_err());
    }

    #[test]
    fn task_jsonpath_only_embeds_canonical_uuid() {
        let id = Uuid::parse_str("0190c6b8-8f3e-7a1b-9c2d-3e4f5a6b7c8d").unwrap();
        let path = task_ref_jsonpath(id);
        assert!(path.contains("^0190c6b8-8f3e-7a1b-9c2d-3e4f5a6b7c8d$"));
        assert!(path.contains(r#"@.attrs.entity == "task""#));
    }
}
