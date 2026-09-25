use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::db::context::{session_is_live, set_tenant};
use crate::db::documents::{membership_role, wiki_can_edit, workspace_is_live};
use crate::db::identity::{append_event, EventAppend};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevisionDbError {
    NotFound,
    Forbidden,
}

#[derive(Debug, Clone)]
pub struct RevisionMeta {
    pub id: Uuid,
    pub target_kind: String,
    pub target_id: Uuid,
    pub reason: String,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct RevisionDetail {
    pub meta: RevisionMeta,
    pub content_json: Value,
    pub y_snapshot: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct RevisionListPage {
    pub items: Vec<RevisionMeta>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CreateRevisionInput {
    pub y_snapshot: Vec<u8>,
    pub content_json: Value,
    pub text: String,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub struct PersistedCollabSource {
    pub snapshot: Vec<u8>,
    pub tail: Vec<Vec<u8>>,
}

#[derive(Debug, Clone, Copy)]
pub struct RevisionCursor {
    pub created_at: DateTime<Utc>,
    pub id: Uuid,
}

const MANUAL_REASON: &str = "manual";
const TARGET_DOCUMENT: &str = "document";

type RevisionMetaRow = (Uuid, String, Uuid, String, Option<Uuid>, DateTime<Utc>);
type RevisionDetailRow = (
    Uuid,
    String,
    Uuid,
    String,
    Option<Uuid>,
    DateTime<Utc>,
    Value,
    Vec<u8>,
);

pub fn encode_revision_cursor(cursor: RevisionCursor) -> String {
    use base64::Engine;
    let payload = serde_json::json!({
        "ca": cursor.created_at.to_rfc3339(),
        "id": cursor.id.to_string(),
    });
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload.to_string())
}

pub fn decode_revision_cursor(raw: &str) -> Option<RevisionCursor> {
    use base64::Engine;
    if raw.len() > 1024 {
        return None;
    }
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(raw)
        .ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    let object = value.as_object()?;
    if object.keys().any(|key| key != "ca" && key != "id") {
        return None;
    }
    let created_at = DateTime::parse_from_rfc3339(object.get("ca")?.as_str()?)
        .ok()?
        .with_timezone(&Utc);
    let id = Uuid::parse_str(object.get("id")?.as_str()?).ok()?;
    Some(RevisionCursor { created_at, id })
}

async fn authorize_document(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
    write: bool,
) -> Result<Result<(), RevisionDbError>, sqlx::Error> {
    if !session_is_live(&mut *tx, actor_user_id, session_id).await? {
        return Ok(Err(RevisionDbError::Forbidden));
    }
    if !workspace_is_live(&mut *tx, workspace_id).await? {
        return Ok(Err(RevisionDbError::NotFound));
    }
    let role = membership_role(&mut *tx, workspace_id, actor_user_id).await?;
    let _ = write;
    if !wiki_can_edit(role) {
        return Ok(Err(RevisionDbError::Forbidden));
    }
    let row: Option<(Option<Uuid>, Option<DateTime<Utc>>)> = sqlx::query_as(
        r#"
        SELECT project_id, deleted_at
        FROM fvoci.documents
        WHERE workspace_id = $1 AND id = $2
        "#,
    )
    .bind(workspace_id)
    .bind(document_id)
    .fetch_optional(&mut **tx)
    .await?;
    match row {
        Some((project_id, deleted_at)) if project_id.is_none() && deleted_at.is_none() => {
            Ok(Ok(()))
        }
        _ => Ok(Err(RevisionDbError::NotFound)),
    }
}

pub async fn authorize_revision_document(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
    write: bool,
) -> Result<Result<(), RevisionDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let result = authorize_document(
        &mut tx,
        workspace_id,
        actor_user_id,
        session_id,
        document_id,
        write,
    )
    .await?;
    match result {
        Ok(()) => {
            tx.commit().await?;
            Ok(Ok(()))
        }
        Err(err) => {
            tx.rollback().await?;
            Ok(Err(err))
        }
    }
}

async fn collab_state_exists(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    document_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let exists: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM fvoci.document_states
            WHERE workspace_id = $1 AND document_id = $2
        )
        "#,
    )
    .bind(workspace_id)
    .bind(document_id)
    .fetch_one(&mut **tx)
    .await?;
    Ok(exists)
}

pub async fn list_document_revisions(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
    limit: i64,
    before: Option<RevisionCursor>,
) -> Result<Result<RevisionListPage, RevisionDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    match authorize_document(
        &mut tx,
        workspace_id,
        actor_user_id,
        session_id,
        document_id,
        false,
    )
    .await?
    {
        Ok(()) => {}
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    }
    let fetch_limit = limit.saturating_add(1);
    let rows: Vec<RevisionMetaRow> = match before {
        Some(cursor) => {
            sqlx::query_as(
                r#"
                SELECT id, target_kind, target_id, reason, created_by, created_at
                FROM fvoci.revisions
                WHERE workspace_id = $1
                  AND target_kind = $2
                  AND target_id = $3
                  AND (created_at, id) < ($4, $5)
                ORDER BY created_at DESC, id DESC
                LIMIT $6
                "#,
            )
            .bind(workspace_id)
            .bind(TARGET_DOCUMENT)
            .bind(document_id)
            .bind(cursor.created_at)
            .bind(cursor.id)
            .bind(fetch_limit)
            .fetch_all(&mut *tx)
            .await?
        }
        None => {
            sqlx::query_as(
                r#"
                SELECT id, target_kind, target_id, reason, created_by, created_at
                FROM fvoci.revisions
                WHERE workspace_id = $1
                  AND target_kind = $2
                  AND target_id = $3
                ORDER BY created_at DESC, id DESC
                LIMIT $4
                "#,
            )
            .bind(workspace_id)
            .bind(TARGET_DOCUMENT)
            .bind(document_id)
            .bind(fetch_limit)
            .fetch_all(&mut *tx)
            .await?
        }
    };
    tx.commit().await?;
    let mut items: Vec<RevisionMeta> = rows
        .into_iter()
        .map(
            |(id, target_kind, target_id, reason, created_by, created_at)| RevisionMeta {
                id,
                target_kind,
                target_id,
                reason,
                created_by,
                created_at,
            },
        )
        .collect();
    let next_cursor = if items.len() as i64 > limit {
        items.pop();
        items.last().map(|last| {
            encode_revision_cursor(RevisionCursor {
                created_at: last.created_at,
                id: last.id,
            })
        })
    } else {
        None
    };
    Ok(Ok(RevisionListPage { items, next_cursor }))
}

pub async fn get_document_revision(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
    revision_id: Uuid,
) -> Result<Result<RevisionDetail, RevisionDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    match authorize_document(
        &mut tx,
        workspace_id,
        actor_user_id,
        session_id,
        document_id,
        false,
    )
    .await?
    {
        Ok(()) => {}
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    }
    let row: Option<RevisionDetailRow> = sqlx::query_as(
        r#"
        SELECT id, target_kind, target_id, reason, created_by, created_at, content_json, y_snapshot
        FROM fvoci.revisions
        WHERE workspace_id = $1 AND id = $2
        "#,
    )
    .bind(workspace_id)
    .bind(revision_id)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    match row {
        Some((
            id,
            target_kind,
            target_id,
            reason,
            created_by,
            created_at,
            content_json,
            y_snapshot,
        )) if target_kind == TARGET_DOCUMENT && target_id == document_id => {
            Ok(Ok(RevisionDetail {
                meta: RevisionMeta {
                    id,
                    target_kind,
                    target_id,
                    reason,
                    created_by,
                    created_at,
                },
                content_json,
                y_snapshot,
            }))
        }
        _ => Ok(Err(RevisionDbError::NotFound)),
    }
}

pub async fn create_manual_document_revision(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
    input: CreateRevisionInput,
) -> Result<Result<Uuid, RevisionDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    match authorize_document(
        &mut tx,
        workspace_id,
        actor_user_id,
        session_id,
        document_id,
        true,
    )
    .await?
    {
        Ok(()) => {}
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    }
    let recent: Option<(Uuid, Vec<u8>)> = sqlx::query_as(
        r#"
        SELECT id, y_snapshot
        FROM fvoci.revisions
        WHERE workspace_id = $1 AND target_kind = $2 AND target_id = $3
        ORDER BY created_at DESC, id DESC
        LIMIT 1
        "#,
    )
    .bind(workspace_id)
    .bind(TARGET_DOCUMENT)
    .bind(document_id)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some((id, prev_snap)) = recent {
        if prev_snap == input.y_snapshot {
            tx.commit().await?;
            return Ok(Ok(id));
        }
    }
    let id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO fvoci.revisions (
            id, workspace_id, target_kind, target_id, y_snapshot, encoding,
            content_json, text, reason, created_by
        ) VALUES ($1, $2, $3, $4, $5, 1, $6, $7, $8, $9)
        "#,
    )
    .bind(id)
    .bind(workspace_id)
    .bind(TARGET_DOCUMENT)
    .bind(document_id)
    .bind(&input.y_snapshot)
    .bind(&input.content_json)
    .bind(&input.text)
    .bind(if input.reason.is_empty() {
        MANUAL_REASON
    } else {
        input.reason.as_str()
    })
    .bind(actor_user_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Ok(id))
}

pub async fn resolve_document_restore(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
    revision_id: Uuid,
    client_ip: Option<&str>,
) -> Result<Result<Vec<u8>, RevisionDbError>, sqlx::Error> {
    let _ = client_ip;
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    match authorize_document(
        &mut tx,
        workspace_id,
        actor_user_id,
        session_id,
        document_id,
        true,
    )
    .await?
    {
        Ok(()) => {}
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    }
    if !collab_state_exists(&mut tx, workspace_id, document_id).await? {
        tx.rollback().await?;
        return Ok(Err(RevisionDbError::NotFound));
    }
    let row: Option<(String, Uuid, Vec<u8>)> = sqlx::query_as(
        r#"
        SELECT target_kind, target_id, y_snapshot
        FROM fvoci.revisions
        WHERE workspace_id = $1 AND id = $2
        "#,
    )
    .bind(workspace_id)
    .bind(revision_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((target_kind, target_id, y_snapshot)) = row else {
        tx.rollback().await?;
        return Ok(Err(RevisionDbError::NotFound));
    };
    if target_kind != TARGET_DOCUMENT || target_id != document_id {
        tx.rollback().await?;
        return Ok(Err(RevisionDbError::NotFound));
    }
    append_event(
        &mut tx,
        EventAppend {
            id: Uuid::now_v7(),
            workspace_id: Some(workspace_id),
            actor_user_id: Some(actor_user_id),
            verb: "document.updated".into(),
            target_type: Some("document".into()),
            target_id: Some(document_id),
            payload: json!({
                "documentId": document_id,
                "restoreRequested": revision_id,
            }),
        },
    )
    .await?;
    tx.commit().await?;
    Ok(Ok(y_snapshot))
}

pub async fn load_persisted_collab_source(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
) -> Result<Result<PersistedCollabSource, RevisionDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    match authorize_document(
        &mut tx,
        workspace_id,
        actor_user_id,
        session_id,
        document_id,
        true,
    )
    .await?
    {
        Ok(()) => {}
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    }
    let state: Option<(Vec<u8>, i16, i64)> = sqlx::query_as(
        r#"
        SELECT state, encoding, snapshot_cutoff_seq
        FROM fvoci.document_states
        WHERE workspace_id = $1 AND document_id = $2
        "#,
    )
    .bind(workspace_id)
    .bind(document_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((snapshot, encoding, cutoff)) = state else {
        tx.rollback().await?;
        return Ok(Err(RevisionDbError::NotFound));
    };
    if encoding != 1 {
        tx.rollback().await?;
        return Ok(Err(RevisionDbError::NotFound));
    }
    let tail: Vec<(Vec<u8>,)> = sqlx::query_as(
        r#"
        SELECT payload
        FROM fvoci.document_collab_updates
        WHERE workspace_id = $1 AND document_id = $2 AND seq > $3
        ORDER BY seq ASC
        "#,
    )
    .bind(workspace_id)
    .bind(document_id)
    .bind(cutoff)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Ok(PersistedCollabSource {
        snapshot,
        tail: tail.into_iter().map(|(payload,)| payload).collect(),
    }))
}
