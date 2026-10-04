use super::backend::{Backend, OperationTx};
use super::codec::Cell;
use chrono::{DateTime, Utc};
use sqlx::{PgPool, Postgres, Transaction};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::db::context::{restore_system, set_system, set_tenant};
use crate::db::workspace::workspace_is_live;
use crate::search::chunk::TextChunk;
use crate::search::embed::embedding_from_json;
use crate::search::meili::SearchSourceKind;
use crate::search::text::to_chosung;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchIndexCursor {
    pub kind: SearchSourceKind,
    pub id: Uuid,
    pub chunk_no: i32,
}

#[derive(Debug, Clone)]
pub struct SearchIndexRow {
    pub kind: SearchSourceKind,
    pub resource_id: Uuid,
    pub workspace_id: Uuid,
    pub project_id: Option<Uuid>,
    pub document_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub comment_id: Option<Uuid>,
    pub attachment_id: Option<Uuid>,
    pub chunk_no: Option<i32>,
    pub title: String,
    pub body: String,
    /// Owner-private bibliography is never merged into authored recall fields.
    pub bibliographic_text: Option<String>,
    pub chosung: String,
    pub updated_at: DateTime<Utc>,
    /// Attachment chunk vector (`attachment_text.embedding`); `None` otherwise.
    pub embedding: Option<Vec<f32>>,
}

#[derive(Debug, Clone, Default)]
pub struct SourceScope {
    pub project_id: Option<Uuid>,
    pub document_id: Option<Uuid>,
    pub subtree: bool,
    pub task_id: Option<Uuid>,
}

const RELATED_PAGE: i64 = 50;

pub fn related_page_limit() -> i64 {
    RELATED_PAGE
}

fn kind_ord(kind: SearchSourceKind) -> i32 {
    match kind {
        SearchSourceKind::Document => 1,
        SearchSourceKind::Task => 2,
        SearchSourceKind::Comment => 3,
        SearchSourceKind::Attachment => 4,
    }
}

fn parse_kind(raw: &str) -> Option<SearchSourceKind> {
    match raw {
        "document" => Some(SearchSourceKind::Document),
        "task" => Some(SearchSourceKind::Task),
        "comment" => Some(SearchSourceKind::Comment),
        "attachment" => Some(SearchSourceKind::Attachment),
        _ => None,
    }
}

fn map_row(row: sqlx::postgres::PgRow) -> Option<SearchIndexRow> {
    use sqlx::Row;
    let kind = parse_kind(row.get::<String, _>("kind").as_str())?;
    Some(SearchIndexRow {
        kind,
        resource_id: row.get("resource_id"),
        workspace_id: row.get("workspace_id"),
        project_id: row.get("project_id"),
        document_id: row.get("document_id"),
        task_id: row.get("task_id"),
        comment_id: row.get("comment_id"),
        attachment_id: row.get("attachment_id"),
        chunk_no: row.get("chunk_no"),
        title: row.get("title"),
        body: row.get("body"),
        bibliographic_text: None,
        chosung: row.get("chosung"),
        updated_at: row.get("ua"),
        embedding: embedding_from_json(row.get("embedding")),
    })
}

pub async fn load_sources(
    pool: &PgPool,
    workspace_id: Uuid,
    kind: SearchSourceKind,
    id: Uuid,
) -> Result<Vec<SearchIndexRow>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let previous = set_system(&mut tx).await?;
    if !workspace_is_live(&mut tx, workspace_id).await? {
        restore_system(&mut tx, &previous).await?;
        tx.commit().await?;
        return Ok(Vec::new());
    }
    let sql = match kind {
        SearchSourceKind::Document => {
            r#"
            SELECT 'document'::text AS kind, d.id AS resource_id, d.workspace_id,
                   d.project_id, d.id AS document_id, NULL::uuid AS task_id,
                   NULL::uuid AS comment_id, NULL::uuid AS attachment_id, NULL::int AS chunk_no,
                   d.title, d.text AS body, d.chosung,
                   date_trunc('milliseconds', d.updated_at) AS ua, NULL::jsonb AS embedding
            FROM fvoci.documents d
            WHERE d.workspace_id = $1 AND d.id = $2
              AND d.deleted_at IS NULL
              AND (d.project_id IS NULL OR EXISTS (
                    SELECT 1 FROM fvoci.projects p
                    WHERE p.workspace_id = $1 AND p.id = d.project_id AND p.deleted_at IS NULL
              ))
            "#
        }
        SearchSourceKind::Task => {
            r#"
            SELECT 'task'::text AS kind, t.id AS resource_id, t.workspace_id,
                   t.project_id, NULL::uuid AS document_id, t.id AS task_id,
                   NULL::uuid AS comment_id, NULL::uuid AS attachment_id, NULL::int AS chunk_no,
                   t.title, t.text AS body, t.chosung,
                   date_trunc('milliseconds', t.updated_at) AS ua, NULL::jsonb AS embedding
            FROM fvoci.tasks t
            WHERE t.workspace_id = $1 AND t.id = $2
              AND t.deleted_at IS NULL AND t.archived_at IS NULL
              AND EXISTS (
                    SELECT 1 FROM fvoci.projects p
                    WHERE p.workspace_id = $1 AND p.id = t.project_id AND p.deleted_at IS NULL
              )
            "#
        }
        SearchSourceKind::Comment => {
            r#"
            SELECT * FROM (
                SELECT 'comment'::text AS kind, c.id AS resource_id, c.workspace_id,
                       d.project_id, c.document_id, NULL::uuid AS task_id,
                       c.id AS comment_id, NULL::uuid AS attachment_id, NULL::int AS chunk_no,
                       d.title, c.body, c.chosung,
                       date_trunc('milliseconds', c.updated_at) AS ua, NULL::jsonb AS embedding
                FROM fvoci.comments c
                JOIN fvoci.documents d
                  ON d.workspace_id = c.workspace_id AND d.id = c.document_id
                WHERE c.workspace_id = $1 AND c.id = $2
                  AND c.document_id IS NOT NULL AND d.deleted_at IS NULL
                  AND (d.project_id IS NULL OR EXISTS (
                        SELECT 1 FROM fvoci.projects p
                        WHERE p.workspace_id = $1 AND p.id = d.project_id AND p.deleted_at IS NULL
                  ))
                UNION ALL
                SELECT 'comment'::text AS kind, c.id AS resource_id, c.workspace_id,
                       t.project_id, NULL::uuid AS document_id, c.task_id,
                       c.id AS comment_id, NULL::uuid AS attachment_id, NULL::int AS chunk_no,
                       t.title, c.body, c.chosung,
                       date_trunc('milliseconds', c.updated_at) AS ua, NULL::jsonb AS embedding
                FROM fvoci.comments c
                JOIN fvoci.tasks t
                  ON t.workspace_id = c.workspace_id AND t.id = c.task_id
                WHERE c.workspace_id = $1 AND c.id = $2
                  AND c.task_id IS NOT NULL
                  AND t.deleted_at IS NULL AND t.archived_at IS NULL
                  AND EXISTS (
                        SELECT 1 FROM fvoci.projects p
                        WHERE p.workspace_id = $1 AND p.id = t.project_id AND p.deleted_at IS NULL
                  )
            ) u
            "#
        }
        SearchSourceKind::Attachment => {
            r#"
            SELECT * FROM (
                SELECT 'attachment'::text AS kind, a.id AS resource_id, a.workspace_id,
                       d.project_id, a.document_id, NULL::uuid AS task_id,
                       NULL::uuid AS comment_id, a.id AS attachment_id, x.chunk_no,
                       a.name AS title, coalesce(x.text, a.extract_text) AS body,
                       coalesce(x.chosung, '') AS chosung,
                       date_trunc('milliseconds', a.created_at) AS ua, x.embedding
                FROM fvoci.attachments a
                JOIN fvoci.documents d
                  ON d.workspace_id = a.workspace_id AND d.id = a.document_id
                LEFT JOIN fvoci.attachment_text x
                  ON x.workspace_id = a.workspace_id AND x.attachment_id = a.id
                 AND x.status IN ('ok', 'partial') AND x.text <> ''
                WHERE a.workspace_id = $1 AND a.id = $2
                  AND a.status = 'stored' AND a.scan_status <> 'infected'
                  AND d.deleted_at IS NULL
                  AND (d.project_id IS NULL OR EXISTS (
                        SELECT 1 FROM fvoci.projects p
                        WHERE p.workspace_id = $1 AND p.id = d.project_id AND p.deleted_at IS NULL
                  ))
                UNION ALL
                SELECT 'attachment'::text AS kind, a.id AS resource_id, a.workspace_id,
                       t.project_id, NULL::uuid AS document_id, a.task_id,
                       NULL::uuid AS comment_id, a.id AS attachment_id, x.chunk_no,
                       a.name AS title, coalesce(x.text, a.extract_text) AS body,
                       coalesce(x.chosung, '') AS chosung,
                       date_trunc('milliseconds', a.created_at) AS ua, x.embedding
                FROM fvoci.attachments a
                JOIN fvoci.tasks t
                  ON t.workspace_id = a.workspace_id AND t.id = a.task_id
                LEFT JOIN fvoci.attachment_text x
                  ON x.workspace_id = a.workspace_id AND x.attachment_id = a.id
                 AND x.status IN ('ok', 'partial') AND x.text <> ''
                WHERE a.workspace_id = $1 AND a.id = $2
                  AND a.status = 'stored' AND a.scan_status <> 'infected'
                  AND t.deleted_at IS NULL AND t.archived_at IS NULL
                  AND EXISTS (
                        SELECT 1 FROM fvoci.projects p
                        WHERE p.workspace_id = $1 AND p.id = t.project_id AND p.deleted_at IS NULL
                  )
            ) u
            ORDER BY coalesce(chunk_no, -1)
            "#
        }
    };
    let rows = sqlx::query(sql)
        .bind(workspace_id)
        .bind(id)
        .fetch_all(&mut *tx)
        .await?;
    let mut rows: Vec<_> = rows.into_iter().filter_map(map_row).collect();
    if kind == SearchSourceKind::Document {
        let ids: Vec<_> = rows.iter().map(|r| r.resource_id).collect();
        let metadata =
            crate::db::zotero::private_search_texts(&mut tx, workspace_id, None, &ids).await?;
        for row in &mut rows {
            row.bibliographic_text = metadata
                .get(&row.resource_id)
                .filter(|v| !v.is_empty())
                .cloned();
        }
    }
    restore_system(&mut tx, &previous).await?;
    tx.commit().await?;
    Ok(rows)
}

fn after_pred(kind: SearchSourceKind, after: Option<&SearchIndexCursor>) -> (bool, Uuid, i32) {
    let Some(after) = after else {
        return (true, Uuid::nil(), -1);
    };
    let ord = kind_ord(kind);
    let after_ord = kind_ord(after.kind);
    if ord < after_ord {
        return (false, Uuid::nil(), -1);
    }
    if ord > after_ord {
        return (true, Uuid::nil(), -1);
    }
    let after_chunk = if after.kind == SearchSourceKind::Attachment {
        after.chunk_no
    } else {
        0
    };
    (true, after.id, after_chunk)
}

pub async fn list_sources(
    pool: &PgPool,
    workspace_id: Uuid,
    after: Option<&SearchIndexCursor>,
    limit: i64,
    scope: &SourceScope,
) -> Result<Vec<SearchIndexRow>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let previous = set_system(&mut tx).await?;
    if !workspace_is_live(&mut tx, workspace_id).await? {
        restore_system(&mut tx, &previous).await?;
        tx.commit().await?;
        return Ok(Vec::new());
    }

    let limit = limit.max(1);
    let mut rows = Vec::new();
    rows.extend(query_documents(&mut tx, workspace_id, after, limit, scope).await?);
    rows.extend(query_tasks(&mut tx, workspace_id, after, limit, scope).await?);
    rows.extend(query_comments(&mut tx, workspace_id, after, limit, scope).await?);
    rows.extend(query_attachments(&mut tx, workspace_id, after, limit, scope).await?);
    restore_system(&mut tx, &previous).await?;
    tx.commit().await?;

    rows.sort_by(|left, right| {
        kind_ord(left.kind)
            .cmp(&kind_ord(right.kind))
            .then(left.resource_id.cmp(&right.resource_id))
            .then(
                left.chunk_no
                    .unwrap_or(-1)
                    .cmp(&right.chunk_no.unwrap_or(-1)),
            )
    });
    rows.truncate(limit as usize);
    Ok(rows)
}

async fn query_documents(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    after: Option<&SearchIndexCursor>,
    limit: i64,
    scope: &SourceScope,
) -> Result<Vec<SearchIndexRow>, sqlx::Error> {
    if scope.task_id.is_some() {
        return Ok(Vec::new());
    }
    let (include, after_id, after_chunk) = after_pred(SearchSourceKind::Document, after);
    if !include {
        return Ok(Vec::new());
    }
    let rows = sqlx::query(
        r#"
        SELECT 'document'::text AS kind, d.id AS resource_id, d.workspace_id,
               d.project_id, d.id AS document_id, NULL::uuid AS task_id,
               NULL::uuid AS comment_id, NULL::uuid AS attachment_id, NULL::int AS chunk_no,
               d.title, d.text AS body, d.chosung,
               date_trunc('milliseconds', d.updated_at) AS ua, NULL::jsonb AS embedding
        FROM fvoci.documents d
        WHERE d.workspace_id = $1 AND d.deleted_at IS NULL
          AND (d.project_id IS NULL OR EXISTS (
                SELECT 1 FROM fvoci.projects p
                WHERE p.workspace_id = $1 AND p.id = d.project_id AND p.deleted_at IS NULL
          ))
          AND ($5::uuid IS NULL OR (d.id, 0) > ($5::uuid, $6))
          AND (
                $2::uuid IS NOT NULL AND d.project_id = $2
                OR $3::uuid IS NOT NULL AND $4 AND EXISTS (
                    SELECT 1 FROM fvoci.documents AS root
                    WHERE root.workspace_id = $1 AND root.id = $3
                      AND (d.path = root.path OR substr(d.path, 1, length(root.path) + 1) = root.path || '.')
                )
                OR $3::uuid IS NOT NULL AND NOT $4 AND d.id = $3
                OR $2::uuid IS NULL AND $3::uuid IS NULL
          )
        ORDER BY d.id
        LIMIT $7
        "#,
    )
    .bind(workspace_id)
    .bind(scope.project_id)
    .bind(scope.document_id)
    .bind(scope.subtree)
    .bind(if after_id.is_nil() { None } else { Some(after_id) })
    .bind(after_chunk)
    .bind(limit)
    .fetch_all(&mut **tx)
    .await?;
    let mut rows: Vec<_> = rows.into_iter().filter_map(map_row).collect();
    let ids: Vec<_> = rows.iter().map(|r| r.resource_id).collect();
    let metadata = crate::db::zotero::private_search_texts(tx, workspace_id, None, &ids).await?;
    for row in &mut rows {
        row.bibliographic_text = metadata
            .get(&row.resource_id)
            .filter(|v| !v.is_empty())
            .cloned();
    }
    Ok(rows)
}

async fn query_tasks(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    after: Option<&SearchIndexCursor>,
    limit: i64,
    scope: &SourceScope,
) -> Result<Vec<SearchIndexRow>, sqlx::Error> {
    if scope.document_id.is_some() {
        return Ok(Vec::new());
    }
    let (include, after_id, after_chunk) = after_pred(SearchSourceKind::Task, after);
    if !include {
        return Ok(Vec::new());
    }
    let rows = sqlx::query(
        r#"
        SELECT 'task'::text AS kind, t.id AS resource_id, t.workspace_id,
               t.project_id, NULL::uuid AS document_id, t.id AS task_id,
               NULL::uuid AS comment_id, NULL::uuid AS attachment_id, NULL::int AS chunk_no,
               t.title, t.text AS body, t.chosung,
               date_trunc('milliseconds', t.updated_at) AS ua, NULL::jsonb AS embedding
        FROM fvoci.tasks t
        WHERE t.workspace_id = $1 AND t.deleted_at IS NULL AND t.archived_at IS NULL
          AND EXISTS (
                SELECT 1 FROM fvoci.projects p
                WHERE p.workspace_id = $1 AND p.id = t.project_id AND p.deleted_at IS NULL
          )
          AND ($4::uuid IS NULL OR (t.id, 0) > ($4::uuid, $5))
          AND ($2::uuid IS NULL OR t.project_id = $2)
          AND ($3::uuid IS NULL OR t.id = $3)
        ORDER BY t.id
        LIMIT $6
        "#,
    )
    .bind(workspace_id)
    .bind(scope.project_id)
    .bind(scope.task_id)
    .bind(if after_id.is_nil() {
        None
    } else {
        Some(after_id)
    })
    .bind(after_chunk)
    .bind(limit)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows.into_iter().filter_map(map_row).collect())
}

async fn query_comments(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    after: Option<&SearchIndexCursor>,
    limit: i64,
    scope: &SourceScope,
) -> Result<Vec<SearchIndexRow>, sqlx::Error> {
    let (include, after_id, after_chunk) = after_pred(SearchSourceKind::Comment, after);
    if !include {
        return Ok(Vec::new());
    }
    let rows = sqlx::query(
        r#"
        SELECT * FROM (
            SELECT 'comment'::text AS kind, c.id AS resource_id, c.workspace_id,
                   d.project_id, c.document_id, NULL::uuid AS task_id,
                   c.id AS comment_id, NULL::uuid AS attachment_id, NULL::int AS chunk_no,
                   d.title, c.body, c.chosung,
                   date_trunc('milliseconds', c.updated_at) AS ua, NULL::jsonb AS embedding
            FROM fvoci.comments c
            JOIN fvoci.documents d
              ON d.workspace_id = c.workspace_id AND d.id = c.document_id
            WHERE c.workspace_id = $1 AND c.document_id IS NOT NULL AND d.deleted_at IS NULL
              AND (d.project_id IS NULL OR EXISTS (
                    SELECT 1 FROM fvoci.projects p
                    WHERE p.workspace_id = $1 AND p.id = d.project_id AND p.deleted_at IS NULL
              ))
              AND ($6::uuid IS NULL OR (c.id, 0) > ($6::uuid, $7))
              AND ($3::uuid IS NULL)
              AND (
                    $2::uuid IS NOT NULL AND d.project_id = $2
                    OR $4::uuid IS NOT NULL AND $5 AND EXISTS (
                        SELECT 1 FROM fvoci.documents AS root
                        WHERE root.workspace_id = $1 AND root.id = $4
                          AND (d.path = root.path OR substr(d.path, 1, length(root.path) + 1) = root.path || '.')
                    )
                    OR $4::uuid IS NOT NULL AND NOT $5 AND c.document_id = $4
                    OR $2::uuid IS NULL AND $4::uuid IS NULL
              )
            UNION ALL
            SELECT 'comment'::text AS kind, c.id AS resource_id, c.workspace_id,
                   t.project_id, NULL::uuid AS document_id, c.task_id,
                   c.id AS comment_id, NULL::uuid AS attachment_id, NULL::int AS chunk_no,
                   t.title, c.body, c.chosung,
                   date_trunc('milliseconds', c.updated_at) AS ua, NULL::jsonb AS embedding
            FROM fvoci.comments c
            JOIN fvoci.tasks t
              ON t.workspace_id = c.workspace_id AND t.id = c.task_id
            WHERE c.workspace_id = $1 AND c.task_id IS NOT NULL
              AND t.deleted_at IS NULL AND t.archived_at IS NULL
              AND EXISTS (
                    SELECT 1 FROM fvoci.projects p
                    WHERE p.workspace_id = $1 AND p.id = t.project_id AND p.deleted_at IS NULL
              )
              AND ($6::uuid IS NULL OR (c.id, 0) > ($6::uuid, $7))
              AND ($4::uuid IS NULL)
              AND ($2::uuid IS NULL OR t.project_id = $2)
              AND ($3::uuid IS NULL OR c.task_id = $3)
        ) u
        ORDER BY resource_id
        LIMIT $8
        "#,
    )
    .bind(workspace_id)
    .bind(scope.project_id)
    .bind(scope.task_id)
    .bind(scope.document_id)
    .bind(scope.subtree)
    .bind(if after_id.is_nil() { None } else { Some(after_id) })
    .bind(after_chunk)
    .bind(limit)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows.into_iter().filter_map(map_row).collect())
}

async fn query_attachments(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    after: Option<&SearchIndexCursor>,
    limit: i64,
    scope: &SourceScope,
) -> Result<Vec<SearchIndexRow>, sqlx::Error> {
    let (include, after_id, after_chunk) = after_pred(SearchSourceKind::Attachment, after);
    if !include {
        return Ok(Vec::new());
    }
    // Source `attachDocs` / `attachTasks`: a document scope only sees
    // document attachments, a task scope only that task's attachments.
    let rows = sqlx::query(
        r#"
        SELECT * FROM (
            SELECT 'attachment'::text AS kind, a.id AS resource_id, a.workspace_id,
                   d.project_id, a.document_id, NULL::uuid AS task_id,
                   NULL::uuid AS comment_id, a.id AS attachment_id, x.chunk_no,
                   a.name AS title, coalesce(x.text, a.extract_text) AS body,
                   coalesce(x.chosung, '') AS chosung,
                   date_trunc('milliseconds', a.created_at) AS ua, x.embedding
            FROM fvoci.attachments a
            JOIN fvoci.documents d
              ON d.workspace_id = a.workspace_id AND d.id = a.document_id
            LEFT JOIN fvoci.attachment_text x
              ON x.workspace_id = a.workspace_id AND x.attachment_id = a.id
             AND x.status IN ('ok', 'partial') AND x.text <> ''
            WHERE a.workspace_id = $1 AND a.status = 'stored' AND a.scan_status <> 'infected'
              AND d.deleted_at IS NULL
              AND (d.project_id IS NULL OR EXISTS (
                    SELECT 1 FROM fvoci.projects p
                    WHERE p.workspace_id = $1 AND p.id = d.project_id AND p.deleted_at IS NULL
              ))
              AND ($5::uuid IS NULL OR (a.id, coalesce(x.chunk_no, -1)) > ($5::uuid, $6))
              AND $8::uuid IS NULL
              AND (
                    $2::uuid IS NOT NULL AND d.project_id = $2
                    OR $3::uuid IS NOT NULL AND $4 AND EXISTS (
                        SELECT 1 FROM fvoci.documents AS root
                        WHERE root.workspace_id = $1 AND root.id = $3
                          AND (d.path = root.path OR substr(d.path, 1, length(root.path) + 1) = root.path || '.')
                    )
                    OR $3::uuid IS NOT NULL AND NOT $4 AND a.document_id = $3
                    OR $2::uuid IS NULL AND $3::uuid IS NULL
              )
            UNION ALL
            SELECT 'attachment'::text AS kind, a.id AS resource_id, a.workspace_id,
                   t.project_id, NULL::uuid AS document_id, a.task_id,
                   NULL::uuid AS comment_id, a.id AS attachment_id, x.chunk_no,
                   a.name AS title, coalesce(x.text, a.extract_text) AS body,
                   coalesce(x.chosung, '') AS chosung,
                   date_trunc('milliseconds', a.created_at) AS ua, x.embedding
            FROM fvoci.attachments a
            JOIN fvoci.tasks t
              ON t.workspace_id = a.workspace_id AND t.id = a.task_id
            LEFT JOIN fvoci.attachment_text x
              ON x.workspace_id = a.workspace_id AND x.attachment_id = a.id
             AND x.status IN ('ok', 'partial') AND x.text <> ''
            WHERE a.workspace_id = $1 AND a.status = 'stored' AND a.scan_status <> 'infected'
              AND t.deleted_at IS NULL AND t.archived_at IS NULL
              AND EXISTS (
                    SELECT 1 FROM fvoci.projects p
                    WHERE p.workspace_id = $1 AND p.id = t.project_id AND p.deleted_at IS NULL
              )
              AND ($5::uuid IS NULL OR (a.id, coalesce(x.chunk_no, -1)) > ($5::uuid, $6))
              AND $3::uuid IS NULL
              AND ($2::uuid IS NULL OR t.project_id = $2)
              AND ($8::uuid IS NULL OR a.task_id = $8)
        ) u
        ORDER BY resource_id, coalesce(chunk_no, -1)
        LIMIT $7
        "#,
    )
    .bind(workspace_id)
    .bind(scope.project_id)
    .bind(scope.document_id)
    .bind(scope.subtree)
    .bind(if after_id.is_nil() { None } else { Some(after_id) })
    .bind(after_chunk)
    .bind(limit)
    .bind(scope.task_id)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows.into_iter().filter_map(map_row).collect())
}

pub async fn replace_attachment_chunks(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    attachment_id: Uuid,
    status: &str,
    chunks: &[TextChunk],
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM fvoci.attachment_text WHERE workspace_id = $1 AND attachment_id = $2")
        .bind(workspace_id)
        .bind(attachment_id)
        .execute(&mut **tx)
        .await?;
    if status != "ok" && status != "partial" {
        return Ok(());
    }
    for chunk in chunks {
        sqlx::query(
            r#"
            INSERT INTO fvoci.attachment_text (
                workspace_id, attachment_id, chunk_no, start_offset, end_offset, text, chosung, status
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            "#,
        )
        .bind(workspace_id)
        .bind(attachment_id)
        .bind(chunk.chunk_no)
        .bind(chunk.start)
        .bind(chunk.end)
        .bind(&chunk.text)
        .bind(to_chosung(&chunk.text))
        .bind(status)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

pub async fn list_live_workspace_ids(pool: &PgPool) -> Result<Vec<Uuid>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let previous = set_system(&mut tx).await?;
    let ids: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM fvoci.workspaces WHERE deleted_at IS NULL ORDER BY id")
            .fetch_all(&mut *tx)
            .await?;
    restore_system(&mut tx, &previous).await?;
    tx.commit().await?;
    Ok(ids)
}

pub fn cursor_of(row: &SearchIndexRow) -> SearchIndexCursor {
    SearchIndexCursor {
        kind: row.kind,
        id: row.resource_id,
        chunk_no: row.chunk_no.unwrap_or(-1),
    }
}

/// Chunk text still waiting for a vector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingEmbeddingChunk {
    pub chunk_no: i32,
    pub text: String,
}

/// Source `listPendingEmbedding`: text chunks without a vector, in chunk
/// order. FVOCI also embeds `partial` chunks (they are indexed like `ok`) and
/// skips attachments the index would drop (infected, trashed parent). Both
/// parents count: documents and (live, unarchived) tasks, as the index does.
pub async fn list_pending_embedding(
    pool: &PgPool,
    workspace_id: Uuid,
    attachment_id: Uuid,
    limit: i64,
) -> Result<Vec<PendingEmbeddingChunk>, sqlx::Error> {
    list_pending_embedding_backend(
        &Backend::Postgres(pool.clone()),
        workspace_id,
        attachment_id,
        limit,
    )
    .await
}

async fn list_pending_embedding_pg(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    attachment_id: Uuid,
    limit: i64,
) -> Result<Vec<PendingEmbeddingChunk>, sqlx::Error> {
    set_tenant(tx, workspace_id).await?;
    let rows: Vec<(i32, String)> = sqlx::query_as(
        r#"
        SELECT x.chunk_no, x.text
        FROM fvoci.attachment_text x
        JOIN fvoci.attachments a
          ON a.workspace_id = x.workspace_id AND a.id = x.attachment_id
        LEFT JOIN fvoci.documents d
          ON d.workspace_id = a.workspace_id AND d.id = a.document_id
        LEFT JOIN fvoci.tasks t
          ON t.workspace_id = a.workspace_id AND t.id = a.task_id
        WHERE x.workspace_id = $1 AND x.attachment_id = $2
          AND x.embedding IS NULL AND x.text <> '' AND x.status IN ('ok', 'partial')
          AND a.status = 'stored' AND a.scan_status <> 'infected'
          AND (
                (a.document_id IS NOT NULL AND d.deleted_at IS NULL)
             OR (a.task_id IS NOT NULL AND t.deleted_at IS NULL AND t.archived_at IS NULL)
          )
        ORDER BY x.chunk_no
        LIMIT $3
        "#,
    )
    .bind(workspace_id)
    .bind(attachment_id)
    .bind(limit)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(chunk_no, text)| PendingEmbeddingChunk { chunk_no, text })
        .collect())
}

/// Next attachment with pending chunks, skipping `excluded` (backing off).
/// Walks live workspaces one tenant at a time: `attachment_text` is tenant-RLS.
pub async fn next_pending_embedding(
    pool: &PgPool,
    excluded: &[Uuid],
) -> Result<Option<(Uuid, Uuid)>, sqlx::Error> {
    for workspace_id in list_live_workspace_ids(pool).await? {
        let mut tx = pool.begin().await?;
        set_tenant(&mut tx, workspace_id).await?;
        let found = list_pending_attachment_pg(&mut tx, workspace_id, excluded).await?;
        tx.commit().await?;
        if let Some(attachment_id) = found {
            return Ok(Some((workspace_id, attachment_id)));
        }
    }
    Ok(None)
}

/// Source `setEmbeddings`. A vector is stored only while the chunk still has
/// the embedded text and no vector (a re-extract in between replaced the row),
/// and an `attachment.embedded` event in the same transaction makes the
/// search-index consumer copy the vectors into Meili. Returns rows written.
pub async fn store_chunk_embeddings(
    pool: &PgPool,
    workspace_id: Uuid,
    attachment_id: Uuid,
    rows: &[(PendingEmbeddingChunk, Vec<f32>)],
) -> Result<u64, sqlx::Error> {
    store_chunk_embeddings_backend(
        &Backend::Postgres(pool.clone()),
        workspace_id,
        attachment_id,
        rows,
    )
    .await
}

async fn store_chunk_embeddings_pg(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    attachment_id: Uuid,
    rows: &[(PendingEmbeddingChunk, Vec<f32>)],
) -> Result<u64, sqlx::Error> {
    if rows.is_empty() {
        return Ok(0);
    }
    set_tenant(tx, workspace_id).await?;
    let mut written = 0u64;
    for (chunk, vector) in rows {
        written += sqlx::query(
            r#"
            UPDATE fvoci.attachment_text
            SET embedding = $5::jsonb, updated_at = now()
            WHERE workspace_id = $1 AND attachment_id = $2 AND chunk_no = $3
              AND text = $4 AND embedding IS NULL
            "#,
        )
        .bind(workspace_id)
        .bind(attachment_id)
        .bind(chunk.chunk_no)
        .bind(&chunk.text)
        .bind(crate::search::embed::embedding_to_json_text(vector))
        .execute(&mut **tx)
        .await?
        .rows_affected();
    }
    if written > 0 {
        crate::db::identity::append_event(
            tx,
            crate::db::identity::EventAppend {
                id: Uuid::now_v7(),
                workspace_id: Some(workspace_id),
                actor_user_id: None,
                verb: "attachment.embedded".into(),
                target_type: Some("attachment".into()),
                target_id: Some(attachment_id),
                payload: serde_json::json!({
                    "attachmentId": attachment_id.to_string(),
                    "chunks": written,
                }),
            },
        )
        .await?;
    }
    Ok(written)
}

pub async fn list_pending_embedding_backend(
    backend: &Backend,
    workspace_id: Uuid,
    attachment_id: Uuid,
    limit: i64,
) -> Result<Vec<PendingEmbeddingChunk>, sqlx::Error> {
    let mut tx = backend.begin_read().await?;
    tx.operation().set_system().await?;
    tx.operation().set_tenant(workspace_id).await?;
    let chunks = tx
        .operation()
        .list_attachment_pending_embeddings(workspace_id, attachment_id, limit)
        .await?;
    tx.commit().await.map_err(|e| e.source)?;
    Ok(chunks)
}

pub async fn next_pending_embedding_backend(
    backend: &Backend,
    excluded: &[Uuid],
) -> Result<Option<(Uuid, Uuid)>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return next_pending_embedding(pool, excluded).await;
    }
    let mut tx = backend.begin_read().await?;
    tx.operation().set_system().await?;
    let next = tx
        .operation()
        .next_attachment_pending_embedding(excluded)
        .await?;
    tx.commit().await.map_err(|e| e.source)?;
    Ok(next)
}

pub async fn store_chunk_embeddings_backend(
    backend: &Backend,
    workspace_id: Uuid,
    attachment_id: Uuid,
    rows: &[(PendingEmbeddingChunk, Vec<f32>)],
) -> Result<u64, sqlx::Error> {
    // Standalone/legacy PG APIs have no caller cancellation scope.
    store_chunk_embeddings_backend_with_cancel(backend, workspace_id, attachment_id, rows, None)
        .await
}

pub(crate) async fn store_chunk_embeddings_backend_with_cancel(
    backend: &Backend,
    workspace_id: Uuid,
    attachment_id: Uuid,
    rows: &[(PendingEmbeddingChunk, Vec<f32>)],
    cancel: Option<&CancellationToken>,
) -> Result<u64, sqlx::Error> {
    if rows.is_empty() {
        return Ok(0);
    }
    let mut tx = backend.begin_write().await?;
    tx.operation().set_system().await?;
    tx.operation().set_tenant(workspace_id).await?;
    // Await the fully owned acquisition; dropping a cancellation-selected
    // BEGIN future would outsource transaction cleanup to a different owner.
    // Current cancellation is checked after acquisition/context and before
    // any vector/event effects. A cancelled batch never publishes its rows.
    if cancel.is_some_and(CancellationToken::is_cancelled) {
        tx.rollback().await?;
        return Ok(0);
    }
    let written = tx
        .operation()
        .store_attachment_chunk_embeddings(workspace_id, attachment_id, rows)
        .await?;
    if cancel.is_some_and(CancellationToken::is_cancelled) {
        tx.rollback().await?;
        return Ok(0);
    }
    tx.commit().await.map_err(|e| e.source)?;
    Ok(written)
}

impl OperationTx<'_, '_> {
    pub(crate) async fn replace_attachment_chunks(
        &mut self,
        workspace_id: Uuid,
        attachment_id: Uuid,
        status: &str,
        chunks: &[TextChunk],
    ) -> Result<(), sqlx::Error> {
        match self {
            Self::Postgres(tx) => {
                replace_attachment_chunks(tx, workspace_id, attachment_id, status, chunks).await
            }
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                tx.require_system_context()?;
                tx.require_tenant(workspace_id)?;
                tx.execute(
                    "DELETE FROM attachment_text WHERE workspace_id=?1 AND attachment_id=?2",
                    &[Cell::uuid(workspace_id), Cell::uuid(attachment_id)],
                )
                .await?;
                if status != "ok" && status != "partial" {
                    return Ok(());
                }
                for chunk in chunks {
                    tx.execute("INSERT INTO attachment_text (workspace_id,attachment_id,chunk_no,start_offset,end_offset,text,chosung,status) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)", &[Cell::uuid(workspace_id),Cell::uuid(attachment_id),Cell::Integer(i64::from(chunk.chunk_no)),Cell::Integer(i64::from(chunk.start)),Cell::Integer(i64::from(chunk.end)),Cell::text(&chunk.text),Cell::text(to_chosung(&chunk.text)),Cell::text(status)]).await?;
                }
                Ok(())
            }
        }
    }

    pub(crate) async fn list_attachment_pending_embeddings(
        &mut self,
        workspace_id: Uuid,
        attachment_id: Uuid,
        limit: i64,
    ) -> Result<Vec<PendingEmbeddingChunk>, sqlx::Error> {
        if !(1..=crate::search::embed::EMBED_BATCH as i64).contains(&limit) {
            return Err(sqlx::Error::Protocol(
                "embedding batch exceeds bound".into(),
            ));
        }
        match self {
            Self::Postgres(tx) => {
                list_pending_embedding_pg(tx, workspace_id, attachment_id, limit).await
            }
            Self::SqliteFamily(tx) => {
                tx.require_system_context()?;
                tx.require_tenant(workspace_id)?;
                let rows = tx.query("SELECT x.chunk_no,x.text FROM attachment_text x JOIN attachments a ON a.workspace_id=x.workspace_id AND a.id=x.attachment_id JOIN workspaces w ON w.id=a.workspace_id AND w.deleted_at IS NULL LEFT JOIN documents d ON d.workspace_id=a.workspace_id AND d.id=a.document_id LEFT JOIN tasks t ON t.workspace_id=a.workspace_id AND t.id=a.task_id WHERE x.workspace_id=?1 AND x.attachment_id=?2 AND x.embedding IS NULL AND x.text<>'' AND x.status IN ('ok','partial') AND a.status='stored' AND a.scan_status<>'infected' AND ((a.document_id IS NOT NULL AND d.id IS NOT NULL AND d.deleted_at IS NULL) OR (a.task_id IS NOT NULL AND t.id IS NOT NULL AND t.deleted_at IS NULL AND t.archived_at IS NULL)) ORDER BY x.chunk_no LIMIT ?3", &[Cell::uuid(workspace_id),Cell::uuid(attachment_id),Cell::Integer(limit)]).await?;
                rows.iter()
                    .map(|row| {
                        let chunk_no = row.cell(0)?.int32()?;
                        if chunk_no < 0 {
                            return Err(sqlx::Error::Protocol("negative embedding chunk".into()));
                        }
                        Ok(PendingEmbeddingChunk {
                            chunk_no,
                            text: row.cell(1)?.string()?,
                        })
                    })
                    .collect()
            }
        }
    }

    /// A system read, with a bounded candidate list: excluded contains at most
    /// 1000 distinct failed attachments, so 1001 candidates suffice to find the
    /// first eligible one without unbounded memory or dynamic SQL parameters.
    pub(crate) async fn next_attachment_pending_embedding(
        &mut self,
        excluded: &[Uuid],
    ) -> Result<Option<(Uuid, Uuid)>, sqlx::Error> {
        if excluded.len() > 1000 {
            return Err(sqlx::Error::Protocol(
                "embedding exclusions exceed bound".into(),
            ));
        }
        match self {
            Self::Postgres(tx) => {
                // Preserve the existing per-tenant PG RLS traversal and query.
                let workspaces: Vec<Uuid> = sqlx::query_scalar(
                    "SELECT id FROM fvoci.workspaces WHERE deleted_at IS NULL ORDER BY id",
                )
                .fetch_all(&mut ***tx)
                .await?;
                for workspace_id in workspaces {
                    set_tenant(tx, workspace_id).await?;
                    let rows = list_pending_attachment_pg(tx, workspace_id, excluded).await?;
                    if let Some(attachment_id) = rows {
                        return Ok(Some((workspace_id, attachment_id)));
                    }
                }
                Ok(None)
            }
            Self::SqliteFamily(tx) => {
                tx.require_system_context()?;
                let rows = tx.query("SELECT DISTINCT x.workspace_id,x.attachment_id FROM attachment_text x JOIN attachments a ON a.workspace_id=x.workspace_id AND a.id=x.attachment_id JOIN workspaces w ON w.id=a.workspace_id AND w.deleted_at IS NULL LEFT JOIN documents d ON d.workspace_id=a.workspace_id AND d.id=a.document_id LEFT JOIN tasks t ON t.workspace_id=a.workspace_id AND t.id=a.task_id WHERE x.embedding IS NULL AND x.text<>'' AND x.status IN ('ok','partial') AND a.status='stored' AND a.scan_status<>'infected' AND ((a.document_id IS NOT NULL AND d.id IS NOT NULL AND d.deleted_at IS NULL) OR (a.task_id IS NOT NULL AND t.id IS NOT NULL AND t.deleted_at IS NULL AND t.archived_at IS NULL)) ORDER BY x.workspace_id,x.attachment_id LIMIT 1001", &[]).await?;
                for row in rows {
                    let workspace = row.cell(0)?.id()?;
                    let attachment = row.cell(1)?.id()?;
                    if !excluded.contains(&attachment) {
                        return Ok(Some((workspace, attachment)));
                    }
                }
                Ok(None)
            }
        }
    }

    pub(crate) async fn store_attachment_chunk_embeddings(
        &mut self,
        workspace_id: Uuid,
        attachment_id: Uuid,
        rows: &[(PendingEmbeddingChunk, Vec<f32>)],
    ) -> Result<u64, sqlx::Error> {
        if rows.len() > crate::search::embed::EMBED_BATCH {
            return Err(sqlx::Error::Protocol(
                "embedding write exceeds batch bound".into(),
            ));
        }
        match self {
            Self::Postgres(tx) => {
                store_chunk_embeddings_pg(tx, workspace_id, attachment_id, rows).await
            }
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                tx.require_system_context()?;
                tx.require_tenant(workspace_id)?;
                let mut written = 0;
                for (chunk, vector) in rows {
                    if chunk.chunk_no < 0
                        || vector.len() != crate::search::meili::EMBEDDING_DIMENSIONS as usize
                        || vector.iter().any(|v| !v.is_finite())
                    {
                        return Err(sqlx::Error::Protocol("invalid embedding row".into()));
                    }
                    written+=tx.execute("UPDATE attachment_text SET embedding=?5,updated_at=unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000 WHERE workspace_id=?1 AND attachment_id=?2 AND chunk_no=?3 AND text=?4 AND embedding IS NULL", &[Cell::uuid(workspace_id),Cell::uuid(attachment_id),Cell::Integer(i64::from(chunk.chunk_no)),Cell::text(&chunk.text),Cell::text(crate::search::embed::embedding_to_json_text(vector))]).await?;
                }
                if written > 0 {
                    self.append_event(crate::db::identity::EventAppend {id:Uuid::now_v7(),workspace_id:Some(workspace_id),actor_user_id:None,verb:"attachment.embedded".into(),target_type:Some("attachment".into()),target_id:Some(attachment_id),payload:serde_json::json!({"attachmentId":attachment_id.to_string(),"chunks":written})}).await?;
                }
                Ok(written)
            }
        }
    }
}

async fn list_pending_attachment_pg(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    excluded: &[Uuid],
) -> Result<Option<Uuid>, sqlx::Error> {
    let found: Option<(Uuid,)> = sqlx::query_as(
        r#"
            SELECT x.attachment_id
            FROM fvoci.attachment_text x
            JOIN fvoci.attachments a
              ON a.workspace_id = x.workspace_id AND a.id = x.attachment_id
            LEFT JOIN fvoci.documents d
              ON d.workspace_id = a.workspace_id AND d.id = a.document_id
            LEFT JOIN fvoci.tasks t
              ON t.workspace_id = a.workspace_id AND t.id = a.task_id
            WHERE x.workspace_id = $1
              AND x.embedding IS NULL AND x.text <> '' AND x.status IN ('ok', 'partial')
              AND a.status = 'stored' AND a.scan_status <> 'infected'
              AND (
                    (a.document_id IS NOT NULL AND d.deleted_at IS NULL)
                 OR (a.task_id IS NOT NULL AND t.deleted_at IS NULL AND t.archived_at IS NULL)
              )
              AND NOT (x.attachment_id = ANY($2))
            ORDER BY x.attachment_id
            LIMIT 1
            "#,
    )
    .bind(workspace_id)
    .bind(excluded)
    .fetch_optional(&mut **tx)
    .await?;

    Ok(found.map(|(id,)| id))
}

#[cfg(test)]
mod embedding_backend_tests {
    use super::*;
    use crate::db::attachment_extract::backend_tests::Fixture;

    #[tokio::test]
    async fn embedding_named_operations_require_current_tenant_system_and_writer() {
        let f = Fixture::new().await;
        f.extracted("Embedding authority fixture").await;
        let mut tx = f.backend.begin_read().await.unwrap();
        tx.operation().set_tenant(f.workspace).await.unwrap();
        assert!(tx
            .operation()
            .list_attachment_pending_embeddings(f.workspace, f.attachment, 1)
            .await
            .is_err());
        tx.rollback().await.unwrap();
        let mut tx = f.backend.begin_read().await.unwrap();
        tx.operation().set_system().await.unwrap();
        tx.operation().set_tenant(Uuid::now_v7()).await.unwrap();
        assert!(tx
            .operation()
            .list_attachment_pending_embeddings(f.workspace, f.attachment, 1)
            .await
            .is_err());
        tx.rollback().await.unwrap();
        let pending = list_pending_embedding_backend(&f.backend, f.workspace, f.attachment, 1)
            .await
            .unwrap();
        assert_eq!(pending.len(), 1);
        let rows = vec![(pending[0].clone(), vec![1.0; 1536])];
        let mut tx = f.backend.begin_read().await.unwrap();
        tx.operation().set_system().await.unwrap();
        tx.operation().set_tenant(f.workspace).await.unwrap();
        assert!(tx
            .operation()
            .store_attachment_chunk_embeddings(f.workspace, f.attachment, &rows)
            .await
            .is_err());
        tx.rollback().await.unwrap();
        assert!(
            list_pending_embedding_backend(&f.backend, f.workspace, f.attachment, 33)
                .await
                .is_err()
        );
        assert!(
            next_pending_embedding_backend(&f.backend, &vec![Uuid::nil(); 1001])
                .await
                .is_err()
        );
        assert_eq!(
            next_pending_embedding_backend(&f.backend, &[f.attachment])
                .await
                .unwrap(),
            None
        );
        assert!(store_chunk_embeddings_backend(
            &f.backend,
            f.workspace,
            f.attachment,
            &[(pending[0].clone(), vec![f32::NAN; 1536])]
        )
        .await
        .is_err());
        assert!(store_chunk_embeddings_backend(
            &f.backend,
            f.workspace,
            f.attachment,
            &[(pending[0].clone(), vec![1.0; 1])]
        )
        .await
        .is_err());
        assert_eq!(f.event_count("attachment.embedded").await, 0);
        sqlx::query("UPDATE documents SET deleted_at=unixepoch()*1000000")
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(
            list_pending_embedding_backend(&f.backend, f.workspace, f.attachment, 1)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(next_pending_embedding_backend(&f.backend, &[])
            .await
            .unwrap()
            .is_none());
        // The read filter excludes dead parents; late vectors use the same
        // existing text/NULL fence as PG, rather than claiming delivery ACL.
        f.finish().await;
    }
}
