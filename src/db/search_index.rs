use chrono::{DateTime, Utc};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::db::context::{restore_system, set_system, set_tenant};
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

async fn workspace_live(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let live: Option<(bool,)> =
        sqlx::query_as("SELECT deleted_at IS NULL FROM fvoci.workspaces WHERE id = $1")
            .bind(workspace_id)
            .fetch_optional(&mut **tx)
            .await?;
    Ok(live.map(|(v,)| v).unwrap_or(false))
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
    if !workspace_live(&mut tx, workspace_id).await? {
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
                   t.title, ''::text AS body, ''::text AS chosung,
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
            ORDER BY coalesce(x.chunk_no, -1)
            "#
        }
    };
    let rows = sqlx::query(sql)
        .bind(workspace_id)
        .bind(id)
        .fetch_all(&mut *tx)
        .await?;
    restore_system(&mut tx, &previous).await?;
    tx.commit().await?;
    Ok(rows.into_iter().filter_map(map_row).collect())
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
    if !workspace_live(&mut tx, workspace_id).await? {
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
    Ok(rows.into_iter().filter_map(map_row).collect())
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
               t.title, ''::text AS body, ''::text AS chosung,
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
    if scope.task_id.is_some() {
        return Ok(Vec::new());
    }
    let (include, after_id, after_chunk) = after_pred(SearchSourceKind::Attachment, after);
    if !include {
        return Ok(Vec::new());
    }
    let rows = sqlx::query(
        r#"
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
        ORDER BY a.id, coalesce(x.chunk_no, -1)
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
/// skips attachments the index would drop (infected, trashed parent).
pub async fn list_pending_embedding(
    pool: &PgPool,
    workspace_id: Uuid,
    attachment_id: Uuid,
    limit: i64,
) -> Result<Vec<PendingEmbeddingChunk>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let rows: Vec<(i32, String)> = sqlx::query_as(
        r#"
        SELECT x.chunk_no, x.text
        FROM fvoci.attachment_text x
        JOIN fvoci.attachments a
          ON a.workspace_id = x.workspace_id AND a.id = x.attachment_id
        JOIN fvoci.documents d
          ON d.workspace_id = a.workspace_id AND d.id = a.document_id
        WHERE x.workspace_id = $1 AND x.attachment_id = $2
          AND x.embedding IS NULL AND x.text <> '' AND x.status IN ('ok', 'partial')
          AND a.status = 'stored' AND a.scan_status <> 'infected'
          AND d.deleted_at IS NULL
        ORDER BY x.chunk_no
        LIMIT $3
        "#,
    )
    .bind(workspace_id)
    .bind(attachment_id)
    .bind(limit)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
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
        let found: Option<(Uuid,)> = sqlx::query_as(
            r#"
            SELECT x.attachment_id
            FROM fvoci.attachment_text x
            JOIN fvoci.attachments a
              ON a.workspace_id = x.workspace_id AND a.id = x.attachment_id
            JOIN fvoci.documents d
              ON d.workspace_id = a.workspace_id AND d.id = a.document_id
            WHERE x.workspace_id = $1
              AND x.embedding IS NULL AND x.text <> '' AND x.status IN ('ok', 'partial')
              AND a.status = 'stored' AND a.scan_status <> 'infected'
              AND d.deleted_at IS NULL
              AND NOT (x.attachment_id = ANY($2))
            ORDER BY x.attachment_id
            LIMIT 1
            "#,
        )
        .bind(workspace_id)
        .bind(excluded)
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        if let Some((attachment_id,)) = found {
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
    if rows.is_empty() {
        return Ok(0);
    }
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
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
        .execute(&mut *tx)
        .await?
        .rows_affected();
    }
    if written > 0 {
        crate::db::identity::append_event(
            &mut tx,
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
    tx.commit().await?;
    Ok(written)
}
