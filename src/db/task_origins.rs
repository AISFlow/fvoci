//! Tasks created from a document block (source `packages/core/src/task-origin.ts`,
//! `apps/server/src/domains/documents/task-origins.ts`).
//!
//! `POST documents/{id}/tasks` creates the task and its `task_origins` row in one
//! transaction. Requests for one document are serialised by a transaction
//! advisory lock on the document id instead of the source's document row lock:
//! the task insert locks the target project, and project document mutations lock
//! project → document, so a document row lock taken first could deadlock with
//! them. The unique `(workspace_id, document_id, request_id)` key still decides
//! replays.

use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::db::context::{lock_key_from_uuid, recheck_session, session_is_live, set_tenant};
use crate::db::documents::{document_permission, lock_membership_users, workspace_is_live};
use crate::db::projects::{project_permission_by_id, ProjectDbError};
use crate::db::tasks::{create_task_tx, CreateTaskInput};
use crate::projects::ProjectPermission;

/// Serialises origin creation per source document (see module docs).
pub const TASK_ORIGIN_LOCK_NAMESPACE: i32 = 1_907_021;
pub const TASK_ORIGIN_ANCHOR_MAX_CHARS: usize = 200;

#[derive(Debug)]
pub enum TaskOriginDbError {
    NotFound,
    Forbidden,
    /// Same `requestId` with a different request (409 `document_version_mismatch`).
    RequestMismatch,
    /// Task creation refused (archived project, invalid status, …).
    Task(ProjectDbError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocumentTaskOutcome {
    Created(Uuid),
    Replayed(Uuid),
}

impl DocumentTaskOutcome {
    pub fn task_id(&self) -> Uuid {
        match self {
            Self::Created(id) | Self::Replayed(id) => *id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskOriginItem {
    pub task_id: Uuid,
    pub document_id: Uuid,
    pub task_display_id: String,
    pub document_display_id: String,
    pub task_title: String,
    pub document_title: String,
    pub anchor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskOriginPage {
    pub items: Vec<TaskOriginItem>,
    pub next_cursor: Option<Uuid>,
}

/// Source request hash: sha256 over the user, target project, anchor and the
/// parsed task input.
pub fn origin_request_hash(
    user_id: Uuid,
    project_id: Uuid,
    anchor: Option<&str>,
    task: &serde_json::Value,
) -> String {
    let canonical = serde_json::json!({
        "userId": user_id,
        "projectId": project_id,
        "anchor": anchor,
        "task": task,
    });
    Sha256::digest(canonical.to_string().as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// View permission on a live wiki or project document (a trashed document or
/// project yields `None`).
async fn document_view_permission(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    document_id: Uuid,
) -> Result<ProjectPermission, sqlx::Error> {
    let row: Option<(Option<Uuid>,)> = sqlx::query_as(
        "SELECT project_id FROM fvoci.documents WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL",
    )
    .bind(workspace_id)
    .bind(document_id)
    .fetch_optional(&mut **tx)
    .await?;
    match row {
        None => Ok(ProjectPermission::None),
        Some((Some(project_id),)) => {
            Ok(
                project_permission_by_id(tx, workspace_id, actor_user_id, project_id)
                    .await?
                    .unwrap_or(ProjectPermission::None),
            )
        }
        Some((None,)) => {
            document_permission(tx, workspace_id, actor_user_id, document_id, true).await
        }
    }
}

/// View permission on a live task in a live project.
async fn task_view_permission(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    task_id: Uuid,
) -> Result<ProjectPermission, sqlx::Error> {
    let project_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT project_id FROM fvoci.tasks WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL",
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(project_id) = project_id else {
        return Ok(ProjectPermission::None);
    };
    Ok(
        project_permission_by_id(tx, workspace_id, actor_user_id, project_id)
            .await?
            .unwrap_or(ProjectPermission::None),
    )
}

pub struct DocumentTaskRequest<'a> {
    pub document_id: Uuid,
    pub project_id: Uuid,
    pub request_id: Uuid,
    pub anchor: Option<&'a str>,
    pub request_hash: &'a str,
    pub task: CreateTaskInput<'a>,
}

/// Source `createDocumentTask`: `view` on the document, `edit` on the target
/// project; a replay of the same `requestId` returns the first task.
pub async fn create_document_task(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    request: DocumentTaskRequest<'_>,
    client_ip: Option<&str>,
    channel: &str,
) -> Result<Result<DocumentTaskOutcome, TaskOriginDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let result = create_document_task_tx(
        &mut tx,
        workspace_id,
        actor_user_id,
        session_id,
        request,
        client_ip,
        channel,
    )
    .await?;
    match result {
        Ok(outcome) => {
            tx.commit().await?;
            Ok(Ok(outcome))
        }
        Err(err) => {
            tx.rollback().await?;
            Ok(Err(err))
        }
    }
}

async fn create_document_task_tx(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    request: DocumentTaskRequest<'_>,
    client_ip: Option<&str>,
    channel: &str,
) -> Result<Result<DocumentTaskOutcome, TaskOriginDbError>, sqlx::Error> {
    lock_membership_users(tx, &[actor_user_id]).await?;
    if !recheck_session(tx, actor_user_id, session_id).await? {
        return Ok(Err(TaskOriginDbError::Forbidden));
    }
    if !workspace_is_live(tx, workspace_id).await? {
        return Ok(Err(TaskOriginDbError::NotFound));
    }
    sqlx::query("SELECT pg_advisory_xact_lock($1, $2)")
        .bind(TASK_ORIGIN_LOCK_NAMESPACE)
        .bind(lock_key_from_uuid(request.document_id))
        .execute(&mut **tx)
        .await?;
    let document_permission =
        document_view_permission(tx, workspace_id, actor_user_id, request.document_id).await?;
    if !document_permission.at_least(ProjectPermission::View) {
        return Ok(Err(TaskOriginDbError::NotFound));
    }
    let existing: Option<(Uuid, String)> = sqlx::query_as(
        r#"
        SELECT task_id, request_hash
        FROM fvoci.task_origins
        WHERE workspace_id = $1 AND document_id = $2 AND request_id = $3
        "#,
    )
    .bind(workspace_id)
    .bind(request.document_id)
    .bind(request.request_id)
    .fetch_optional(&mut **tx)
    .await?;
    if let Some((task_id, hash)) = existing {
        if hash != request.request_hash {
            return Ok(Err(TaskOriginDbError::RequestMismatch));
        }
        let permission = task_view_permission(tx, workspace_id, actor_user_id, task_id).await?;
        if !permission.at_least(ProjectPermission::View) {
            return Ok(Err(TaskOriginDbError::NotFound));
        }
        return Ok(Ok(DocumentTaskOutcome::Replayed(task_id)));
    }
    let created = create_task_tx(
        tx,
        workspace_id,
        request.project_id,
        actor_user_id,
        session_id,
        request.task,
        client_ip,
        channel,
    )
    .await?;
    let task = match created {
        Ok(task) => task,
        Err(err) => return Ok(Err(TaskOriginDbError::Task(err))),
    };
    sqlx::query(
        r#"
        INSERT INTO fvoci.task_origins (
            workspace_id, task_id, document_id, request_id, request_hash, anchor
        ) VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(workspace_id)
    .bind(task.id)
    .bind(request.document_id)
    .bind(request.request_id)
    .bind(request.request_hash)
    .bind(request.anchor)
    .execute(&mut **tx)
    .await?;
    Ok(Ok(DocumentTaskOutcome::Created(task.id)))
}

type OriginRow = (
    Uuid,
    Uuid,
    Option<String>,
    i32,
    String,
    Option<String>,
    i32,
    String,
    Option<String>,
);

/// Source `listTaskOrigin` (`GET tasks/{id}/origin`): the task must be visible;
/// an origin whose document the caller cannot view is omitted (200, empty).
pub async fn get_task_origin(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    task_id: Uuid,
    after: Option<Uuid>,
    limit: i64,
) -> Result<Result<TaskOriginPage, TaskOriginDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !session_is_live(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(TaskOriginDbError::Forbidden));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(TaskOriginDbError::NotFound));
    }
    let permission = task_view_permission(&mut tx, workspace_id, actor_user_id, task_id).await?;
    if !permission.at_least(ProjectPermission::View) {
        tx.rollback().await?;
        return Ok(Err(TaskOriginDbError::NotFound));
    }
    let rows: Vec<OriginRow> = sqlx::query_as(
        r#"
        SELECT o.task_id, o.document_id, tp.key, t.number, t.title,
               dp.key, d.number, d.title, o.anchor
        FROM fvoci.task_origins o
        JOIN fvoci.tasks t
          ON t.workspace_id = o.workspace_id AND t.id = o.task_id AND t.deleted_at IS NULL
        JOIN fvoci.projects tp
          ON tp.workspace_id = t.workspace_id AND tp.id = t.project_id
        JOIN fvoci.documents d
          ON d.workspace_id = o.workspace_id AND d.id = o.document_id AND d.deleted_at IS NULL
        LEFT JOIN fvoci.projects dp
          ON dp.workspace_id = d.workspace_id AND dp.id = d.project_id
        WHERE o.workspace_id = $1
          AND o.task_id = $2
          AND ($3::uuid IS NULL OR o.task_id > $3)
        ORDER BY o.task_id
        LIMIT $4
        "#,
    )
    .bind(workspace_id)
    .bind(task_id)
    .bind(after)
    .bind(limit.saturating_add(1))
    .fetch_all(&mut *tx)
    .await?;
    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        let (
            origin_task_id,
            document_id,
            task_project_key,
            task_number,
            task_title,
            document_project_key,
            document_number,
            document_title,
            anchor,
        ) = row;
        let visible = document_view_permission(&mut tx, workspace_id, actor_user_id, document_id)
            .await?
            .at_least(ProjectPermission::View);
        if !visible {
            continue;
        }
        items.push(TaskOriginItem {
            task_id: origin_task_id,
            document_id,
            task_display_id: format!(
                "{}-{task_number}",
                task_project_key.as_deref().unwrap_or_default()
            ),
            document_display_id: format!(
                "{}-{document_number}",
                document_project_key.as_deref().unwrap_or("WIKI")
            ),
            task_title,
            document_title,
            anchor,
        });
    }
    tx.commit().await?;
    let next_cursor = if items.len() as i64 > limit {
        items.truncate(limit as usize);
        items.last().map(|item| item.task_id)
    } else {
        None
    };
    Ok(Ok(TaskOriginPage { items, next_cursor }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_hash_depends_on_every_part() {
        let user = Uuid::nil();
        let project = Uuid::from_u128(1);
        let task = serde_json::json!({"title": "a"});
        let base = origin_request_hash(user, project, Some("b1"), &task);
        assert_eq!(base.len(), 64);
        assert_eq!(base, origin_request_hash(user, project, Some("b1"), &task));
        assert_ne!(base, origin_request_hash(user, project, None, &task));
        assert_ne!(
            base,
            origin_request_hash(user, Uuid::from_u128(2), Some("b1"), &task)
        );
        assert_ne!(
            base,
            origin_request_hash(
                user,
                project,
                Some("b1"),
                &serde_json::json!({"title": "b"})
            )
        );
    }
}
