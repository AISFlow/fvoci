//! Workspace event log (source `listWorkspaceEvents`, packages/core/src/events.ts).
//!
//! Reads `fvoci.events` in relay order `(xact, seq)` under the workspace tenant
//! context, for workspace owners/admins only. Rows tied to a project the actor
//! cannot view (source `eventVisibleTo`) are skipped in SQL so that `LIMIT` and
//! the next cursor count visible rows only.

use base64::Engine;
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

use crate::db::context::{session_is_live, set_tenant};
use crate::db::projects::visible_project_sql_for_guest;
use crate::db::workspace::{membership_role, workspace_kind_read, WorkspaceDbError, WorkspaceRole};

/// Source `eventCursor` payload `{ xact, seq }`: decimal strings of at most 32 chars.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventCursor {
    pub xact: u64,
    pub seq: i64,
}

#[derive(Debug, Clone)]
pub struct WorkspaceEventRow {
    pub id: Uuid,
    pub verb: String,
    pub workspace_id: Option<Uuid>,
    pub actor_user_id: Option<Uuid>,
    pub target_type: Option<String>,
    pub target_id: Option<Uuid>,
    pub payload: Value,
    pub channel: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug)]
pub struct WorkspaceEventPage {
    pub items: Vec<WorkspaceEventRow>,
    pub next_cursor: Option<EventCursor>,
}

/// base64url(JSON) like the source `makeCursorCodec`.
pub fn encode_event_cursor(cursor: EventCursor) -> String {
    let payload = json!({ "xact": cursor.xact.to_string(), "seq": cursor.seq.to_string() });
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload.to_string())
}

/// Strict decode: exactly `xact` and `seq`, each a digit string of at most 32
/// chars that fits `xid8` / `bigint` (the source would fail the SQL cast).
pub fn decode_event_cursor(raw: &str) -> Option<EventCursor> {
    if raw.len() > 1024 {
        return None;
    }
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(raw.trim_end_matches('='))
        .ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    let object = value.as_object()?;
    if object.len() != 2 {
        return None;
    }
    let digits = |key: &str| -> Option<&str> {
        let text = object.get(key)?.as_str()?;
        (!text.is_empty() && text.len() <= 32 && text.bytes().all(|b| b.is_ascii_digit()))
            .then_some(text)
    };
    Some(EventCursor {
        xact: digits("xact")?.parse().ok()?,
        seq: digits("seq")?.parse().ok()?,
    })
}

type EventScan = (
    Uuid,
    String,
    i64,
    String,
    Option<Uuid>,
    Option<Uuid>,
    Option<String>,
    Option<Uuid>,
    Value,
    String,
    DateTime<Utc>,
);

const UUID_TEXT_RE: &str =
    "^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$";

fn list_sql() -> String {
    // Project resolution follows `eventVisibleTo`: payload projectId, then
    // documentId, then taskId, then the task/document/attachment/comment target.
    // A reference that resolves to no project (wiki document, missing row) is
    // visible as in the source; a non-UUID reference fails closed.
    format!(
        r#"
        SELECT e.id, e.xact::text, e.seq, e.verb, e.workspace_id, e.actor_user_id,
               e.target_type, e.target_id, e.payload, e.channel, e.created_at
        FROM fvoci.events AS e
        CROSS JOIN LATERAL (
            SELECT
                CASE
                    WHEN jsonb_typeof(e.payload->'projectId') = 'string' THEN 'project'
                    WHEN jsonb_typeof(e.payload->'documentId') = 'string' THEN 'document'
                    WHEN jsonb_typeof(e.payload->'taskId') = 'string' THEN 'task'
                    WHEN e.target_id IS NOT NULL
                         AND e.target_type IN ('task', 'document', 'attachment', 'comment')
                        THEN e.target_type
                END AS kind,
                CASE
                    WHEN jsonb_typeof(e.payload->'projectId') = 'string' THEN e.payload->>'projectId'
                    WHEN jsonb_typeof(e.payload->'documentId') = 'string' THEN e.payload->>'documentId'
                    WHEN jsonb_typeof(e.payload->'taskId') = 'string' THEN e.payload->>'taskId'
                    ELSE e.target_id::text
                END AS raw_id
        ) AS r
        CROSS JOIN LATERAL (
            SELECT CASE WHEN r.raw_id ~ '{UUID_TEXT_RE}' THEN r.raw_id::uuid END AS ref_id
        ) AS v
        CROSS JOIN LATERAL (
            SELECT CASE r.kind
                WHEN 'project' THEN v.ref_id
                WHEN 'document' THEN (
                    SELECT d.project_id FROM fvoci.documents AS d
                    WHERE d.workspace_id = e.workspace_id AND d.id = v.ref_id
                )
                WHEN 'task' THEN (
                    SELECT t.project_id FROM fvoci.tasks AS t
                    WHERE t.workspace_id = e.workspace_id AND t.id = v.ref_id
                )
                WHEN 'attachment' THEN (
                    SELECT COALESCE(ad.project_id, at.project_id)
                    FROM fvoci.attachments AS a
                    LEFT JOIN fvoci.documents AS ad
                      ON ad.workspace_id = a.workspace_id AND ad.id = a.document_id
                    LEFT JOIN fvoci.tasks AS at
                      ON at.workspace_id = a.workspace_id AND at.id = a.task_id
                    WHERE a.workspace_id = e.workspace_id AND a.id = v.ref_id
                )
                WHEN 'comment' THEN (
                    SELECT COALESCE(cd.project_id, ct.project_id)
                    FROM fvoci.comments AS c
                    LEFT JOIN fvoci.documents AS cd
                      ON cd.workspace_id = c.workspace_id AND cd.id = c.document_id
                    LEFT JOIN fvoci.tasks AS ct
                      ON ct.workspace_id = c.workspace_id AND ct.id = c.task_id
                    WHERE c.workspace_id = e.workspace_id AND c.id = v.ref_id
                )
            END AS project_id
        ) AS pr
        WHERE e.workspace_id = $1
          AND (e.xact, e.seq) > ($3::xid8, $4::bigint)
          AND e.xact < pg_catalog.pg_snapshot_xmin(pg_catalog.pg_current_snapshot())
          AND (
            r.kind IS NULL
            OR (
              v.ref_id IS NOT NULL
              AND (
                pr.project_id IS NULL
                OR EXISTS (
                    SELECT 1 FROM fvoci.projects AS p
                    WHERE p.workspace_id = $1
                      AND p.id = pr.project_id
                      AND p.deleted_at IS NULL
                      AND {visible}
                )
              )
            )
          )
        ORDER BY e.xact, e.seq
        LIMIT $5
        "#,
        visible = visible_project_sql_for_guest("p", false, 2),
    )
}

/// Oldest first; `limit` is 1..=100 (validated by the route). Only committed
/// transactions below the snapshot xmin are read, so a later page cannot gain
/// rows behind an already returned cursor.
pub async fn list_workspace_events(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    cursor: Option<EventCursor>,
    limit: i64,
) -> Result<Result<WorkspaceEventPage, WorkspaceDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !session_is_live(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::Forbidden));
    }
    let role = membership_role(&mut tx, workspace_id, actor_user_id).await?;
    if !role.is_some_and(|r| r.at_least(WorkspaceRole::Admin)) {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::Forbidden));
    }
    if workspace_kind_read(&mut tx, workspace_id).await?.is_none() {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::NotFound));
    }
    let after = cursor.unwrap_or(EventCursor { xact: 0, seq: 0 });
    let rows: Vec<EventScan> = sqlx::query_as(&list_sql())
        .bind(workspace_id)
        .bind(actor_user_id)
        .bind(after.xact.to_string())
        .bind(after.seq)
        .bind(limit + 1)
        .fetch_all(&mut *tx)
        .await?;
    tx.commit().await?;
    let has_more = rows.len() as i64 > limit;
    let mut items = Vec::with_capacity(rows.len().min(limit as usize));
    let mut last = None;
    for (id, xact, seq, verb, ws, actor, target_type, target_id, payload, channel, created_at) in
        rows.into_iter().take(limit as usize)
    {
        let xact: u64 = xact
            .parse()
            .map_err(|_| sqlx::Error::Decode("events.xact is not a decimal xid8".into()))?;
        last = Some(EventCursor { xact, seq });
        items.push(WorkspaceEventRow {
            id,
            verb,
            workspace_id: ws,
            actor_user_id: actor,
            target_type,
            target_id,
            payload,
            channel,
            created_at,
        });
    }
    Ok(Ok(WorkspaceEventPage {
        items,
        next_cursor: if has_more { last } else { None },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_round_trips_and_rejects_foreign_shapes() {
        let cursor = EventCursor {
            xact: 7_500_000_000,
            seq: 42,
        };
        assert_eq!(
            decode_event_cursor(&encode_event_cursor(cursor)),
            Some(cursor)
        );
        let enc = |v: Value| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v.to_string());
        assert!(decode_event_cursor(&enc(json!({"xact": "1", "seq": "2", "x": 1}))).is_none());
        assert!(decode_event_cursor(&enc(json!({"xact": 1, "seq": "2"}))).is_none());
        assert!(decode_event_cursor(&enc(json!({"xact": "-1", "seq": "2"}))).is_none());
        assert!(decode_event_cursor(&enc(json!({"xact": "", "seq": "2"}))).is_none());
        assert!(
            decode_event_cursor(&enc(json!({"xact": "1", "seq": "99999999999999999999"})))
                .is_none()
        );
        assert!(decode_event_cursor("not base64 !").is_none());
        assert!(decode_event_cursor(&"A".repeat(1025)).is_none());
    }
}
