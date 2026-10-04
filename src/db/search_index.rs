use chrono::{DateTime, Utc};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::db::backend::{Backend, FamilyTx, OperationTx};
use crate::db::codec::{Cell, FamilyRow};
use crate::db::context::{restore_system, set_system, set_tenant};
use crate::db::workspace::workspace_is_live;
use crate::search::chunk::TextChunk;
use crate::search::embed::embedding_from_json;
use crate::search::meili::SearchSourceKind;
use crate::search::text::to_chosung;

#[cfg(test)]
mod family_source_tests {
    use super::*;

    fn document_cells(workspace: Uuid, document: Uuid) -> Vec<Cell> {
        vec![
            Cell::text("document"),
            Cell::uuid(document),
            Cell::uuid(workspace),
            Cell::Null,
            Cell::uuid(document),
            Cell::Null,
            Cell::Null,
            Cell::Null,
            Cell::Null,
            Cell::text("한글🙂"),
            Cell::text(""),
            Cell::text("ㅎㄱ"),
            Cell::Integer(-1),
            Cell::Null,
        ]
    }

    #[test]
    fn source_cells_preserve_nulls_bytes_and_explicit_search_milliseconds() {
        let workspace = Uuid::now_v7();
        let document = Uuid::now_v7();
        let row = decode_search_cells(&document_cells(workspace, document), workspace).unwrap();
        assert_eq!(row.resource_id, document);
        assert_eq!(row.document_id, Some(document));
        assert_eq!(row.project_id, None);
        assert_eq!(row.chunk_no, None);
        assert_eq!(row.embedding, None);
        assert_eq!(row.body, "");
        assert_eq!(row.title, "한글🙂");
        // PG date_trunc(milliseconds) floors a pre-epoch microsecond too.
        assert_eq!(row.updated_at.timestamp_micros(), -1000);
    }

    #[test]
    fn malformed_or_cross_tenant_source_is_rejected() {
        let workspace = Uuid::now_v7();
        let document = Uuid::now_v7();
        let good = document_cells(workspace, document);
        assert!(decode_search_cells(&good, Uuid::now_v7()).is_err());
        for (column, bad) in [
            (1, Cell::text(document.to_string())),
            (1, Cell::Blob(vec![0; 15])),
            (0, Cell::text("unknown-kind")),
            (8, Cell::Integer(-1)),
            (8, Cell::Integer(i64::from(i32::MAX) + 1)),
            (10, Cell::Null),
            (12, Cell::text("2026-10-04")),
            (13, Cell::text("invalid-json")),
        ] {
            let mut cells = good.clone();
            cells[column] = bad;
            assert!(
                decode_search_cells(&cells, workspace).is_err(),
                "column {column}"
            );
        }
        assert!(decode_search_cells(&good[..13], workspace).is_err());
    }

    #[test]
    fn page_order_and_kind_transition_keep_attachment_null_chunk_boundary() {
        let workspace = Uuid::now_v7();
        let id = Uuid::now_v7();
        let doc = decode_search_cells(&document_cells(workspace, id), workspace).unwrap();
        let mut chunk = doc.clone();
        chunk.kind = SearchSourceKind::Attachment;
        chunk.chunk_no = Some(0);
        let mut fallback = chunk.clone();
        fallback.chunk_no = None;
        let mut rows = vec![chunk, fallback, doc];
        finish_source_page(&mut rows, 3);
        assert_eq!(rows[0].kind, SearchSourceKind::Document);
        assert_eq!(rows[1].chunk_no, None);
        assert_eq!(rows[2].chunk_no, Some(0));
        let cursor = cursor_of(&rows[1]);
        assert_eq!(
            after_pred(SearchSourceKind::Document, Some(&cursor)).0,
            false
        );
        assert_eq!(
            after_pred(SearchSourceKind::Attachment, Some(&cursor)),
            (true, id, -1)
        );
    }
}

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
    let rows = load_sources_tx(&mut tx, workspace_id, kind, id).await?;
    restore_system(&mut tx, &previous).await?;
    tx.commit().await?;
    Ok(rows)
}

async fn load_sources_tx(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    kind: SearchSourceKind,
    id: Uuid,
) -> Result<Vec<SearchIndexRow>, sqlx::Error> {
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
        .fetch_all(&mut **tx)
        .await?;
    let mut rows: Vec<_> = rows.into_iter().filter_map(map_row).collect();
    if kind == SearchSourceKind::Document {
        let ids: Vec<_> = rows.iter().map(|r| r.resource_id).collect();
        let metadata =
            crate::db::zotero::private_search_texts(tx, workspace_id, None, &ids).await?;
        for row in &mut rows {
            row.bibliographic_text = metadata
                .get(&row.resource_id)
                .filter(|v| !v.is_empty())
                .cloned();
        }
    }
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

    let rows = list_sources_tx(&mut tx, workspace_id, after, limit, scope).await?;
    restore_system(&mut tx, &previous).await?;
    tx.commit().await?;
    Ok(rows)
}

async fn list_sources_tx(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    after: Option<&SearchIndexCursor>,
    limit: i64,
    scope: &SourceScope,
) -> Result<Vec<SearchIndexRow>, sqlx::Error> {
    let limit = limit.max(1);
    let mut rows = Vec::new();
    rows.extend(query_documents(tx, workspace_id, after, limit, scope).await?);
    rows.extend(query_tasks(tx, workspace_id, after, limit, scope).await?);
    rows.extend(query_comments(tx, workspace_id, after, limit, scope).await?);
    rows.extend(query_attachments(tx, workspace_id, after, limit, scope).await?);
    finish_source_page(&mut rows, limit);
    Ok(rows)
}

fn finish_source_page(rows: &mut Vec<SearchIndexRow>, limit: i64) {
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
    rows.truncate(limit.max(1) as usize);
}

/// Read current sources on the caller's transaction. The search refresh owns
/// its reservation through enqueue; these operations never begin another writer.
impl OperationTx<'_, '_> {
    pub(crate) async fn load_search_sources(
        &mut self,
        workspace: Uuid,
        kind: SearchSourceKind,
        id: Uuid,
    ) -> Result<Vec<SearchIndexRow>, sqlx::Error> {
        self.set_tenant(workspace).await?;
        let previous = self.set_system().await?;
        let result = async {
            match self {
                Self::Postgres(tx) => {
                    if workspace_is_live(tx, workspace).await? {
                        load_sources_tx(tx, workspace, kind, id).await
                    } else {
                        Ok(Vec::new())
                    }
                }
                Self::SqliteFamily(tx) => {
                    tx.search_source_rows(
                        workspace,
                        kind,
                        Some(id),
                        None,
                        i64::MAX,
                        &SourceScope::default(),
                    )
                    .await
                }
            }
        }
        .await;
        self.restore_system(previous).await?;
        result
    }

    pub(crate) async fn list_search_sources(
        &mut self,
        workspace: Uuid,
        after: Option<&SearchIndexCursor>,
        limit: i64,
        scope: &SourceScope,
    ) -> Result<Vec<SearchIndexRow>, sqlx::Error> {
        self.set_tenant(workspace).await?;
        let previous = self.set_system().await?;
        let result = async {
            match self {
                Self::Postgres(tx) => {
                    if workspace_is_live(tx, workspace).await? {
                        list_sources_tx(tx, workspace, after, limit, scope).await
                    } else {
                        Ok(Vec::new())
                    }
                }
                Self::SqliteFamily(tx) => {
                    let mut rows = Vec::new();
                    for kind in [
                        SearchSourceKind::Document,
                        SearchSourceKind::Task,
                        SearchSourceKind::Comment,
                        SearchSourceKind::Attachment,
                    ] {
                        rows.extend(
                            tx.search_source_rows(workspace, kind, None, after, limit, scope)
                                .await?,
                        );
                    }
                    finish_source_page(&mut rows, limit);
                    Ok(rows)
                }
            }
        }
        .await;
        self.restore_system(previous).await?;
        result
    }
}

pub async fn load_sources_backend(
    backend: &Backend,
    workspace: Uuid,
    kind: SearchSourceKind,
    id: Uuid,
) -> Result<Vec<SearchIndexRow>, sqlx::Error> {
    let mut tx = backend.begin_read().await?;
    let rows = tx
        .operation()
        .load_search_sources(workspace, kind, id)
        .await?;
    // Read-only operations have no command to reconcile after a lost COMMIT.
    tx.rollback().await?;
    Ok(rows)
}

pub async fn list_sources_backend(
    backend: &Backend,
    workspace: Uuid,
    after: Option<&SearchIndexCursor>,
    limit: i64,
    scope: &SourceScope,
) -> Result<Vec<SearchIndexRow>, sqlx::Error> {
    let mut tx = backend.begin_read().await?;
    let rows = tx
        .operation()
        .list_search_sources(workspace, after, limit, scope)
        .await?;
    tx.rollback().await?;
    Ok(rows)
}

// Fixed SQLite-family statements, shared by the native and remote drivers.
// Bindings: workspace, optional exact target, project, document, subtree, task,
// optional page UUID, page chunk, limit. No PostgreSQL SQL is rewritten.
const FAMILY_DOCUMENT_SOURCES: &str = r#"
SELECT 'document', d.id, d.workspace_id, d.project_id, d.id, NULL,
       NULL, NULL, NULL, d.title, d.text, d.chosung, d.updated_at, NULL
FROM documents d
JOIN workspaces w ON w.id=d.workspace_id AND w.deleted_at IS NULL
WHERE d.workspace_id=?1 AND d.deleted_at IS NULL
  AND (?2 IS NULL OR d.id=?2)
  AND (d.project_id IS NULL OR EXISTS (
    SELECT 1 FROM projects p WHERE p.workspace_id=?1 AND p.id=d.project_id AND p.deleted_at IS NULL))
  AND ?6 IS NULL
  AND (?7 IS NULL OR (d.id,0)>(?7,?8))
  AND (?3 IS NOT NULL AND d.project_id=?3
    OR ?4 IS NOT NULL AND ?5=1 AND EXISTS (
      SELECT 1 FROM documents root WHERE root.workspace_id=?1 AND root.id=?4
      AND (d.path=root.path OR substr(d.path,1,length(root.path)+1)=root.path||'.'))
    OR ?4 IS NOT NULL AND ?5=0 AND d.id=?4
    OR ?3 IS NULL AND ?4 IS NULL)
ORDER BY d.id LIMIT ?9
"#;

const FAMILY_TASK_SOURCES: &str = r#"
SELECT 'task', t.id, t.workspace_id, t.project_id, NULL, t.id,
       NULL, NULL, NULL, t.title, t.text, t.chosung, t.updated_at, NULL
FROM tasks t
JOIN workspaces w ON w.id=t.workspace_id AND w.deleted_at IS NULL
WHERE t.workspace_id=?1 AND t.deleted_at IS NULL AND t.archived_at IS NULL
  AND (?2 IS NULL OR t.id=?2)
  AND EXISTS (SELECT 1 FROM projects p WHERE p.workspace_id=?1 AND p.id=t.project_id AND p.deleted_at IS NULL)
  AND ?4 IS NULL AND ?5 IN (0,1)
  AND (?7 IS NULL OR (t.id,0)>(?7,?8))
  AND (?3 IS NULL OR t.project_id=?3) AND (?6 IS NULL OR t.id=?6)
ORDER BY t.id LIMIT ?9
"#;

const FAMILY_COMMENT_SOURCES: &str = r#"
SELECT * FROM (
  SELECT 'comment', c.id AS resource_id, c.workspace_id, d.project_id, c.document_id, NULL,
         c.id, NULL, NULL, d.title, c.body, c.chosung, c.updated_at, NULL
  FROM comments c
  JOIN workspaces w ON w.id=c.workspace_id AND w.deleted_at IS NULL
  JOIN documents d ON d.workspace_id=c.workspace_id AND d.id=c.document_id
  WHERE c.workspace_id=?1 AND c.document_id IS NOT NULL AND d.deleted_at IS NULL
    AND (?2 IS NULL OR c.id=?2)
    AND (d.project_id IS NULL OR EXISTS (
      SELECT 1 FROM projects p WHERE p.workspace_id=?1 AND p.id=d.project_id AND p.deleted_at IS NULL))
    AND ?6 IS NULL AND (?7 IS NULL OR (c.id,0)>(?7,?8))
    AND (?3 IS NOT NULL AND d.project_id=?3
      OR ?4 IS NOT NULL AND ?5=1 AND EXISTS (
        SELECT 1 FROM documents root WHERE root.workspace_id=?1 AND root.id=?4
        AND (d.path=root.path OR substr(d.path,1,length(root.path)+1)=root.path||'.'))
      OR ?4 IS NOT NULL AND ?5=0 AND c.document_id=?4
      OR ?3 IS NULL AND ?4 IS NULL)
  UNION ALL
  SELECT 'comment', c.id, c.workspace_id, t.project_id, NULL, c.task_id,
         c.id, NULL, NULL, t.title, c.body, c.chosung, c.updated_at, NULL
  FROM comments c
  JOIN workspaces w ON w.id=c.workspace_id AND w.deleted_at IS NULL
  JOIN tasks t ON t.workspace_id=c.workspace_id AND t.id=c.task_id
  WHERE c.workspace_id=?1 AND c.task_id IS NOT NULL
    AND t.deleted_at IS NULL AND t.archived_at IS NULL
    AND (?2 IS NULL OR c.id=?2)
    AND EXISTS (SELECT 1 FROM projects p WHERE p.workspace_id=?1 AND p.id=t.project_id AND p.deleted_at IS NULL)
    AND ?4 IS NULL AND ?5 IN (0,1) AND (?7 IS NULL OR (c.id,0)>(?7,?8))
    AND (?3 IS NULL OR t.project_id=?3) AND (?6 IS NULL OR c.task_id=?6)
) u ORDER BY resource_id LIMIT ?9
"#;

const FAMILY_ATTACHMENT_SOURCES: &str = r#"
SELECT * FROM (
  SELECT 'attachment', a.id AS resource_id, a.workspace_id, d.project_id, a.document_id, NULL,
         NULL, a.id, x.chunk_no AS chunk_no, a.name, coalesce(x.text,a.extract_text),
         coalesce(x.chosung,''), a.created_at, x.embedding
  FROM attachments a
  JOIN workspaces w ON w.id=a.workspace_id AND w.deleted_at IS NULL
  JOIN documents d ON d.workspace_id=a.workspace_id AND d.id=a.document_id
  LEFT JOIN attachment_text x ON x.workspace_id=a.workspace_id AND x.attachment_id=a.id
    AND x.status IN ('ok','partial') AND x.text<>''
  WHERE a.workspace_id=?1 AND a.status='stored' AND a.scan_status<>'infected' AND d.deleted_at IS NULL
    AND (?2 IS NULL OR a.id=?2)
    AND (d.project_id IS NULL OR EXISTS (
      SELECT 1 FROM projects p WHERE p.workspace_id=?1 AND p.id=d.project_id AND p.deleted_at IS NULL))
    AND ?6 IS NULL AND (?7 IS NULL OR (a.id,coalesce(x.chunk_no,-1))>(?7,?8))
    AND (?3 IS NOT NULL AND d.project_id=?3
      OR ?4 IS NOT NULL AND ?5=1 AND EXISTS (
        SELECT 1 FROM documents root WHERE root.workspace_id=?1 AND root.id=?4
        AND (d.path=root.path OR substr(d.path,1,length(root.path)+1)=root.path||'.'))
      OR ?4 IS NOT NULL AND ?5=0 AND a.document_id=?4
      OR ?3 IS NULL AND ?4 IS NULL)
  UNION ALL
  SELECT 'attachment', a.id, a.workspace_id, t.project_id, NULL, a.task_id,
         NULL, a.id, x.chunk_no, a.name, coalesce(x.text,a.extract_text),
         coalesce(x.chosung,''), a.created_at, x.embedding
  FROM attachments a
  JOIN workspaces w ON w.id=a.workspace_id AND w.deleted_at IS NULL
  JOIN tasks t ON t.workspace_id=a.workspace_id AND t.id=a.task_id
  LEFT JOIN attachment_text x ON x.workspace_id=a.workspace_id AND x.attachment_id=a.id
    AND x.status IN ('ok','partial') AND x.text<>''
  WHERE a.workspace_id=?1 AND a.status='stored' AND a.scan_status<>'infected'
    AND t.deleted_at IS NULL AND t.archived_at IS NULL
    AND (?2 IS NULL OR a.id=?2)
    AND EXISTS (SELECT 1 FROM projects p WHERE p.workspace_id=?1 AND p.id=t.project_id AND p.deleted_at IS NULL)
    AND ?4 IS NULL AND ?5 IN (0,1) AND (?7 IS NULL OR (a.id,coalesce(x.chunk_no,-1))>(?7,?8))
    AND (?3 IS NULL OR t.project_id=?3) AND (?6 IS NULL OR a.task_id=?6)
) u ORDER BY resource_id,coalesce(chunk_no,-1) LIMIT ?9
"#;

impl FamilyTx {
    async fn search_source_rows(
        &mut self,
        workspace: Uuid,
        kind: SearchSourceKind,
        id: Option<Uuid>,
        after: Option<&SearchIndexCursor>,
        limit: i64,
        scope: &SourceScope,
    ) -> Result<Vec<SearchIndexRow>, sqlx::Error> {
        self.require_tenant(workspace)?;
        self.require_system_context()?;
        let (include, after_id, after_chunk) = after_pred(kind, after);
        if !include {
            return Ok(Vec::new());
        }
        let statement = match kind {
            SearchSourceKind::Document => FAMILY_DOCUMENT_SOURCES,
            SearchSourceKind::Task => FAMILY_TASK_SOURCES,
            SearchSourceKind::Comment => FAMILY_COMMENT_SOURCES,
            SearchSourceKind::Attachment => FAMILY_ATTACHMENT_SOURCES,
        };
        let rows = self
            .query(
                statement,
                &[
                    Cell::uuid(workspace),
                    Cell::optional_uuid(id),
                    Cell::optional_uuid(scope.project_id),
                    Cell::optional_uuid(scope.document_id),
                    Cell::Integer(i64::from(scope.subtree)),
                    Cell::optional_uuid(scope.task_id),
                    Cell::optional_uuid(if after_id.is_nil() {
                        None
                    } else {
                        Some(after_id)
                    }),
                    Cell::Integer(i64::from(after_chunk)),
                    Cell::Integer(limit.max(1)),
                ],
            )
            .await?;
        let mut rows = rows
            .into_iter()
            .map(|row| map_family_search_row(row, workspace))
            .collect::<Result<Vec<_>, _>>()?;
        if kind == SearchSourceKind::Document {
            for row in &mut rows {
                row.bibliographic_text = self
                    .search_bibliographic_text(workspace, row.resource_id)
                    .await?;
            }
        }
        Ok(rows)
    }

    async fn search_bibliographic_text(
        &mut self,
        workspace: Uuid,
        document: Uuid,
    ) -> Result<Option<String>, sqlx::Error> {
        self.require_tenant(workspace)?;
        self.require_system_context()?;
        let rows = self.query(r#"
            SELECT r.bibliography FROM zotero_references r
            JOIN documents d ON d.workspace_id=r.workspace_id AND d.id=r.document_id AND d.deleted_at IS NULL
            JOIN users u ON u.id=r.owner_user_id AND u.personal_workspace_id=r.workspace_id AND u.deleted_at IS NULL
            JOIN workspaces w ON w.id=r.workspace_id AND w.kind='personal' AND w.deleted_at IS NULL
            JOIN memberships m ON m.workspace_id=r.workspace_id AND m.user_id=r.owner_user_id AND m.role='owner'
            WHERE r.workspace_id=?1 AND r.document_id=?2
        "#, &[Cell::uuid(workspace), Cell::uuid(document)]).await?;
        let Some(row) = rows.last() else {
            return Ok(None);
        };
        let text = crate::db::zotero::bibliographic_text(row.cell(0)?.value()?);
        Ok((!text.is_empty()).then_some(text))
    }
}

fn map_family_search_row(row: FamilyRow, workspace: Uuid) -> Result<SearchIndexRow, sqlx::Error> {
    let cells = (0..14)
        .map(|index| row.cell(index))
        .collect::<Result<Vec<_>, _>>()?;
    decode_search_cells(&cells, workspace)
}

fn decode_search_cells(cells: &[Cell], workspace: Uuid) -> Result<SearchIndexRow, sqlx::Error> {
    if cells.len() != 14 {
        return Err(sqlx::Error::Protocol("invalid search source width".into()));
    }
    let kind = parse_kind(&cells[0].string()?)
        .ok_or_else(|| sqlx::Error::Protocol("invalid search source kind".into()))?;
    let actual_workspace = cells[2].id()?;
    if actual_workspace != workspace {
        return Err(sqlx::Error::Protocol(
            "search source tenant mismatch".into(),
        ));
    }
    let chunk_no = cells[8].optional(|c| {
        i32::try_from(c.integer()?)
            .map_err(|_| sqlx::Error::Protocol("search chunk number out of range".into()))
    })?;
    if chunk_no.is_some_and(|n| n < 0) {
        return Err(sqlx::Error::Protocol("negative search chunk number".into()));
    }
    let instant = cells[12].datetime()?;
    let updated_at = DateTime::from_timestamp_millis(instant.timestamp_millis())
        .ok_or_else(|| sqlx::Error::Protocol("search timestamp out of range".into()))?;
    Ok(SearchIndexRow {
        kind,
        resource_id: cells[1].id()?,
        workspace_id: actual_workspace,
        project_id: cells[3].optional(Cell::id)?,
        document_id: cells[4].optional(Cell::id)?,
        task_id: cells[5].optional(Cell::id)?,
        comment_id: cells[6].optional(Cell::id)?,
        attachment_id: cells[7].optional(Cell::id)?,
        chunk_no,
        title: cells[9].string()?,
        body: cells[10].string()?,
        chosung: cells[11].string()?,
        bibliographic_text: None,
        updated_at,
        embedding: embedding_from_json(cells[13].optional(Cell::value)?),
    })
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

pub async fn list_live_workspace_ids_backend(backend: &Backend) -> Result<Vec<Uuid>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return list_live_workspace_ids(pool).await;
    }
    let mut tx = backend.begin_read().await?;
    let mut operation = tx.operation();
    let previous = operation.set_system().await?;
    let result = async {
        match &mut operation {
            OperationTx::SqliteFamily(family) => {
                family.require_system_context()?;
                family
                    .query(
                        "SELECT id FROM workspaces WHERE deleted_at IS NULL ORDER BY id",
                        &[],
                    )
                    .await?
                    .iter()
                    .map(|row| row.cell(0)?.id())
                    .collect()
            }
            OperationTx::Postgres(_) => unreachable!("PostgreSQL handled above"),
        }
    }
    .await;
    operation.restore_system(previous).await?;
    tx.rollback().await?;
    result
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
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
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
