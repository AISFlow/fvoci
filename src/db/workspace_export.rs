//! Data for GET /api/v1/workspaces/{workspace_id}/export (source workspace-export.ts).
//!
//! Visibility matches notifications: private projects need membership view; wiki
//! documents need document_permission, not listTree ACL bypass.

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::db::context::{recheck_session, set_tenant};
use crate::db::documents::{document_permission, workspace_is_live};
use crate::db::projects::project_permission_by_id;
use crate::db::workspace::{membership_role, WorkspaceRole};
use crate::projects::{effective_permission, ProjectPermission};

pub const EXPORT_TASK_PAGE: i64 = 10_000;
pub const EXPORT_TASK_MAX_PAGES: i32 = 100;
pub const EXPORT_COMMENT_PAGE: i64 = 500;
pub const EXPORT_COMMENT_MAX_PAGES: i32 = 200;
/// Refuse snapshots that would require holding more than this many document bodies at once.
pub const EXPORT_MAX_DOCUMENTS: usize = 20_000;
pub const EXPORT_MAX_ATTACHMENTS: usize = 60_000;
/// Serialized JSON metadata (documents/tasks/comments/attachments listings) per export.
pub const EXPORT_MAX_JSON_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceExportDbError {
    NotFound,
    Forbidden,
    Truncated,
}

#[derive(Debug, Clone)]
pub struct WorkspaceExportMeta {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub excluded_private_project_count: i32,
}

/// Identity and ordering only; bodies load at delivery after a fresh visibility check.
#[derive(Debug, Clone)]
pub struct ExportDocumentRef {
    pub id: Uuid,
    pub parent_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    pub title: String,
    pub status: String,
}

#[derive(Debug, Clone)]
pub struct ExportDocumentBody {
    pub text: String,
    pub content_json: Value,
}

#[derive(Debug, Clone)]
pub struct ExportTask {
    pub id: Uuid,
    pub project_id: Uuid,
    pub title: String,
    pub status_id: Uuid,
    pub task_type: String,
    pub parent_id: Option<Uuid>,
    pub archived_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct ExportCommentRef {
    pub id: Uuid,
    pub document_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct ExportAttachment {
    pub id: Uuid,
    pub document_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub name: String,
    pub mime: String,
    pub size_bytes: Option<i64>,
    pub scan_status: String,
    pub storage_key: String,
}

#[derive(Debug, Clone)]
pub struct WorkspaceExportSnapshot {
    pub workspace: WorkspaceExportMeta,
    pub documents: Vec<ExportDocumentRef>,
    pub tasks: Vec<ExportTask>,
    pub comments: Vec<ExportCommentRef>,
    pub attachments: Vec<ExportAttachment>,
}

async fn require_manage(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<Result<(), WorkspaceExportDbError>, sqlx::Error> {
    if !recheck_session(tx, actor_user_id, session_id).await? {
        return Ok(Err(WorkspaceExportDbError::Forbidden));
    }
    if !workspace_is_live(tx, workspace_id).await? {
        return Ok(Err(WorkspaceExportDbError::NotFound));
    }
    let role = membership_role(tx, workspace_id, actor_user_id).await?;
    if !role
        .map(|r| r.at_least(WorkspaceRole::Admin))
        .unwrap_or(false)
    {
        return Ok(Err(WorkspaceExportDbError::Forbidden));
    }
    Ok(Ok(()))
}

pub async fn recheck_export_authorization(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<Result<(), WorkspaceExportDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let result = require_manage(&mut tx, workspace_id, actor_user_id, session_id).await?;
    tx.commit().await?;
    Ok(result)
}

/// Manage/session/live plus the attachment parent still being visible (revocation mid-pack).
pub async fn recheck_attachment_delivery(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Option<Uuid>,
    task_id: Option<Uuid>,
) -> Result<Result<(), WorkspaceExportDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    match require_manage(&mut tx, workspace_id, actor_user_id, session_id).await? {
        Ok(()) => {}
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    }
    if let Some(document_id) = document_id {
        if !document_permission(&mut tx, workspace_id, actor_user_id, document_id, true)
            .await?
            .at_least(ProjectPermission::View)
        {
            tx.rollback().await?;
            return Ok(Err(WorkspaceExportDbError::Forbidden));
        }
    } else if let Some(task_id) = task_id {
        let project_id: Option<Uuid> = sqlx::query_scalar(
            "SELECT project_id FROM fvoci.tasks WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL",
        )
        .bind(workspace_id)
        .bind(task_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(project_id) = project_id else {
            tx.rollback().await?;
            return Ok(Err(WorkspaceExportDbError::Forbidden));
        };
        let permission = project_permission_by_id(&mut tx, workspace_id, actor_user_id, project_id)
            .await?;
        if !permission
            .map(|p| p.at_least(ProjectPermission::View))
            .unwrap_or(false)
        {
            tx.rollback().await?;
            return Ok(Err(WorkspaceExportDbError::Forbidden));
        }
    } else {
        tx.rollback().await?;
        return Ok(Err(WorkspaceExportDbError::Forbidden));
    }
    tx.commit().await?;
    Ok(Ok(()))
}

pub async fn recheck_document_delivery(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
    project_id: Option<Uuid>,
) -> Result<Result<(), WorkspaceExportDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    match require_manage(&mut tx, workspace_id, actor_user_id, session_id).await? {
        Ok(()) => {}
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    }
    let live: Option<bool> = sqlx::query_scalar(
        "SELECT deleted_at IS NULL FROM fvoci.documents WHERE workspace_id = $1 AND id = $2",
    )
    .bind(workspace_id)
    .bind(document_id)
    .fetch_optional(&mut *tx)
    .await?;
    if !live.unwrap_or(false) {
        tx.rollback().await?;
        return Ok(Err(WorkspaceExportDbError::Forbidden));
    }
    let visible = match project_id {
        Some(pid) => {
            let permission =
                project_permission_by_id(&mut tx, workspace_id, actor_user_id, pid).await?;
            permission
                .map(|p| p.at_least(ProjectPermission::View))
                .unwrap_or(false)
        }
        None => document_permission(&mut tx, workspace_id, actor_user_id, document_id, true)
            .await?
            .at_least(ProjectPermission::View),
    };
    if !visible {
        tx.rollback().await?;
        return Ok(Err(WorkspaceExportDbError::Forbidden));
    }
    tx.commit().await?;
    Ok(Ok(()))
}

pub async fn recheck_task_delivery(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    project_id: Uuid,
) -> Result<Result<(), WorkspaceExportDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    match require_manage(&mut tx, workspace_id, actor_user_id, session_id).await? {
        Ok(()) => {}
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    }
    let permission =
        project_permission_by_id(&mut tx, workspace_id, actor_user_id, project_id).await?;
    if !permission
        .map(|p| p.at_least(ProjectPermission::View))
        .unwrap_or(false)
    {
        tx.rollback().await?;
        return Ok(Err(WorkspaceExportDbError::Forbidden));
    }
    tx.commit().await?;
    Ok(Ok(()))
}

pub async fn recheck_comment_delivery(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Option<Uuid>,
    task_id: Option<Uuid>,
) -> Result<Result<(), WorkspaceExportDbError>, sqlx::Error> {
    if let Some(document_id) = document_id {
        let project_id: Option<Uuid> = sqlx::query_scalar(
            "SELECT project_id FROM fvoci.documents WHERE workspace_id = $1 AND id = $2",
        )
        .bind(workspace_id)
        .bind(document_id)
        .fetch_optional(pool)
        .await?;
        return recheck_document_delivery(
            pool,
            workspace_id,
            actor_user_id,
            session_id,
            document_id,
            project_id,
        )
        .await;
    }
    if let Some(task_id) = task_id {
        let project_id: Option<Uuid> = sqlx::query_scalar(
            "SELECT project_id FROM fvoci.tasks WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL",
        )
        .bind(workspace_id)
        .bind(task_id)
        .fetch_optional(pool)
        .await?;
        let Some(project_id) = project_id else {
            return Ok(Err(WorkspaceExportDbError::Forbidden));
        };
        return recheck_task_delivery(pool, workspace_id, actor_user_id, session_id, project_id)
            .await;
    }
    Ok(Err(WorkspaceExportDbError::Forbidden))
}

pub async fn fetch_document_body(
    pool: &PgPool,
    workspace_id: Uuid,
    document_id: Uuid,
) -> Result<Option<ExportDocumentBody>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let row: Option<(String, Value)> = sqlx::query_as(
        r#"
        SELECT text, content_json
        FROM fvoci.documents
        WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL
        "#,
    )
    .bind(workspace_id)
    .bind(document_id)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(row.map(|(text, content_json)| ExportDocumentBody {
        text,
        content_json,
    }))
}

pub async fn fetch_comment_body(
    pool: &PgPool,
    workspace_id: Uuid,
    comment_id: Uuid,
) -> Result<Option<String>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let body = sqlx::query_scalar(
        "SELECT body FROM fvoci.comments WHERE workspace_id = $1 AND id = $2",
    )
    .bind(workspace_id)
    .bind(comment_id)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(body)
}

pub async fn load_export_snapshot(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<Result<WorkspaceExportSnapshot, WorkspaceExportDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    match require_manage(&mut tx, workspace_id, actor_user_id, session_id).await? {
        Ok(()) => {}
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    }

    let workspace_row: Option<(Uuid, String, String)> = sqlx::query_as(
        "SELECT id, slug, name FROM fvoci.workspaces WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(workspace_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((id, slug, name)) = workspace_row else {
        tx.rollback().await?;
        return Ok(Err(WorkspaceExportDbError::NotFound));
    };

    let project_rows: Vec<(Uuid, String)> = sqlx::query_as(
        r#"
        SELECT id, visibility
        FROM fvoci.projects
        WHERE workspace_id = $1 AND deleted_at IS NULL
        ORDER BY key COLLATE "C"
        "#,
    )
    .bind(workspace_id)
    .fetch_all(&mut *tx)
    .await?;

    let workspace_role = membership_role(&mut tx, workspace_id, actor_user_id)
        .await?
        .unwrap_or(WorkspaceRole::Guest);

    let mut visible_project_ids = std::collections::HashSet::new();
    let mut excluded_private = 0i32;
    for (project_id, visibility) in &project_rows {
        let member_role =
            crate::db::projects::project_member_role(&mut tx, workspace_id, *project_id, actor_user_id)
                .await?;
        let level = effective_permission(workspace_role, visibility, member_role);
        if level.at_least(ProjectPermission::View) {
            visible_project_ids.insert(*project_id);
        } else if visibility == "private" {
            excluded_private += 1;
        }
    }

    let doc_nodes: Vec<(Uuid, Option<Uuid>, Option<Uuid>, String, String)> = sqlx::query_as(
        r#"
        SELECT id, parent_id, project_id, title, status
        FROM fvoci.documents
        WHERE workspace_id = $1 AND deleted_at IS NULL
        ORDER BY sort_key COLLATE "C"
        "#,
    )
    .bind(workspace_id)
    .fetch_all(&mut *tx)
    .await?;

    let mut documents = Vec::new();
    for (doc_id, parent_id, project_id, title, status) in doc_nodes {
        let visible = match project_id {
            Some(pid) => visible_project_ids.contains(&pid),
            None => document_permission(&mut tx, workspace_id, actor_user_id, doc_id, true)
                .await?
                .at_least(ProjectPermission::View),
        };
        if visible {
            documents.push(ExportDocumentRef {
                id: doc_id,
                parent_id,
                project_id,
                title,
                status,
            });
        }
    }

    if documents.len() > EXPORT_MAX_DOCUMENTS {
        tx.rollback().await?;
        return Ok(Err(WorkspaceExportDbError::Truncated));
    }

    let mut tasks = Vec::new();
    for (project_id, _) in &project_rows {
        if !visible_project_ids.contains(project_id) {
            continue;
        }
        let mut after: Option<(DateTime<Utc>, Uuid)> = None;
        let mut pages = 0i32;
        loop {
            if pages >= EXPORT_TASK_MAX_PAGES {
                tx.rollback().await?;
                return Ok(Err(WorkspaceExportDbError::Truncated));
            }
            let page_rows: Vec<(
                Uuid,
                Uuid,
                String,
                Uuid,
                String,
                Option<Uuid>,
                Option<DateTime<Utc>>,
                DateTime<Utc>,
            )> = sqlx::query_as(
                r#"
                SELECT id, project_id, title, status_id, type, parent_id, archived_at, created_at
                FROM fvoci.tasks
                WHERE workspace_id = $1 AND project_id = $2 AND deleted_at IS NULL
                  AND ($3::timestamptz IS NULL OR (created_at, id) > ($3, $4))
                ORDER BY created_at ASC, id ASC
                LIMIT $5
                "#,
            )
            .bind(workspace_id)
            .bind(project_id)
            .bind(after.map(|(at, _)| at))
            .bind(after.map(|(_, id)| id))
            .bind(EXPORT_TASK_PAGE + 1)
            .fetch_all(&mut *tx)
            .await?;
            if page_rows.len() > EXPORT_TASK_PAGE as usize {
                tx.rollback().await?;
                return Ok(Err(WorkspaceExportDbError::Truncated));
            }
            let full = page_rows.len() as i64 == EXPORT_TASK_PAGE;
            for row in page_rows {
                tasks.push(ExportTask {
                    id: row.0,
                    project_id: row.1,
                    title: row.2,
                    status_id: row.3,
                    task_type: row.4,
                    parent_id: row.5,
                    archived_at: row.6,
                    created_at: row.7,
                });
            }
            if !full {
                break;
            }
            let last = tasks.last().expect("full page");
            after = Some((last.created_at, last.id));
            pages += 1;
        }
    }

    let doc_id_set: std::collections::HashSet<Uuid> = documents.iter().map(|d| d.id).collect();
    let task_id_set: std::collections::HashSet<Uuid> = tasks.iter().map(|t| t.id).collect();

    let mut comments = Vec::new();
    let mut after_comment: Option<(DateTime<Utc>, Uuid)> = None;
    let mut comment_pages = 0i32;
    loop {
        if comment_pages >= EXPORT_COMMENT_MAX_PAGES {
            tx.rollback().await?;
            return Ok(Err(WorkspaceExportDbError::Truncated));
        }
        let page_rows: Vec<(Uuid, Option<Uuid>, Option<Uuid>, String, DateTime<Utc>)> =
            sqlx::query_as(
                r#"
                SELECT id, document_id, task_id, body, created_at
                FROM fvoci.comments
                WHERE workspace_id = $1
                  AND ($2::timestamptz IS NULL OR (created_at, id) > ($2, $3))
                ORDER BY created_at ASC, id ASC
                LIMIT $4
                "#,
            )
            .bind(workspace_id)
            .bind(after_comment.map(|(at, _)| at))
            .bind(after_comment.map(|(_, id)| id))
            .bind(EXPORT_COMMENT_PAGE)
            .fetch_all(&mut *tx)
            .await?;
        let full = page_rows.len() as i64 == EXPORT_COMMENT_PAGE;
        let page_cursor = page_rows.last().map(|row| (row.4, row.0));
        for (id, document_id, task_id, _body, created_at) in page_rows {
            let keep = if let Some(document_id) = document_id {
                doc_id_set.contains(&document_id)
            } else if let Some(task_id) = task_id {
                task_id_set.contains(&task_id)
            } else {
                false
            };
            if keep {
                comments.push(ExportCommentRef {
                    id,
                    document_id,
                    task_id,
                    created_at,
                });
            }
        }
        if !full {
            break;
        }
        after_comment = page_cursor;
        comment_pages += 1;
    }

    let doc_ids: Vec<Uuid> = doc_id_set.iter().copied().collect();
    let task_ids: Vec<Uuid> = task_id_set.iter().copied().collect();
    let mut attachments = Vec::new();
    let mut seen = std::collections::HashSet::new();
    if !doc_ids.is_empty() || !task_ids.is_empty() {
        let rows: Vec<(Uuid, Option<Uuid>, Option<Uuid>, String, String, Option<i64>, String, String)> =
            sqlx::query_as(
            r#"
            SELECT id, document_id, task_id, name, mime, size_bytes, scan_status, storage_key
            FROM fvoci.attachments
            WHERE workspace_id = $1 AND status = 'stored'
              AND (
                (document_id IS NOT NULL AND document_id = ANY($2))
                OR (task_id IS NOT NULL AND task_id = ANY($3))
              )
            ORDER BY created_at ASC, id ASC
            "#,
        )
        .bind(workspace_id)
        .bind(&doc_ids)
        .bind(&task_ids)
        .fetch_all(&mut *tx)
        .await?;
        for (id, document_id, task_id, name, mime, size_bytes, scan_status, storage_key) in rows {
            if !seen.insert(id) {
                continue;
            }
            attachments.push(ExportAttachment {
                id,
                document_id,
                task_id,
                name,
                mime,
                size_bytes,
                scan_status,
                storage_key,
            });
        }
    }
    if attachments.len() > EXPORT_MAX_ATTACHMENTS {
        tx.rollback().await?;
        return Ok(Err(WorkspaceExportDbError::Truncated));
    }

    tx.commit().await?;
    Ok(Ok(WorkspaceExportSnapshot {
        workspace: WorkspaceExportMeta {
            id,
            slug,
            name,
            excluded_private_project_count: excluded_private,
        },
        documents,
        tasks,
        comments,
        attachments,
    }))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceJson {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub excluded_private_project_count: i32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentJson<'a> {
    pub id: String,
    pub parent_id: Option<String>,
    pub title: &'a str,
    pub status: &'a str,
    pub text: &'a str,
    pub content_json: &'a Value,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskJson {
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub status_id: String,
    #[serde(rename = "type")]
    pub task_type: String,
    pub parent_id: Option<String>,
    pub archived_at: Option<String>,
    pub created_at: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommentJson<'a> {
    pub id: String,
    pub document_id: Option<String>,
    pub task_id: Option<String>,
    pub body: &'a str,
    pub created_at: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentMetaJson<'a> {
    pub id: String,
    pub name: &'a str,
    pub mime: &'a str,
    pub size_bytes: Option<i64>,
    pub scan_status: &'a str,
}

pub fn js_iso(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
