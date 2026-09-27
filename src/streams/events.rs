use serde_json::Value;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::db::context::set_tenant;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventCursor {
    pub xact: String,
    pub seq: i64,
}

impl Default for EventCursor {
    fn default() -> Self {
        Self {
            xact: "0".to_string(),
            seq: 0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct StreamEventRow {
    pub xact: String,
    pub seq: i64,
    pub verb: String,
    pub payload: Value,
}

const TASK_VERBS: &[&str] = &["task.created", "task.updated", "task.deleted"];

const ACCESS_VERBS: &[&str] = &[
    "workspace_member.removed",
    "workspace_member.role_changed",
    "admin.user_suspended_set",
    "user.withdrawn",
    "user.withdraw_cancelled",
];

pub async fn initial_cursor(
    pool: &PgPool,
    workspace_id: Uuid,
) -> Result<EventCursor, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let row: Option<(String, i64)> = sqlx::query_as(
        r#"
        SELECT e.xact::text, e.seq
        FROM fvoci.events AS e
        WHERE e.workspace_id = $1
          AND e.xact < pg_catalog.pg_snapshot_xmin(pg_catalog.pg_current_snapshot())
        ORDER BY e.xact DESC, e.seq DESC
        LIMIT 1
        "#,
    )
    .bind(workspace_id)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(match row {
        Some((xact, seq)) => EventCursor { xact, seq },
        None => EventCursor::default(),
    })
}

pub async fn poll_task_events(
    pool: &PgPool,
    workspace_id: Uuid,
    project_id: Uuid,
    cursor: &EventCursor,
    limit: i32,
) -> Result<Vec<StreamEventRow>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let rows = query_events(
        &mut tx,
        workspace_id,
        cursor,
        limit,
        TASK_VERBS,
        Some(project_id),
        None,
    )
    .await?;
    tx.commit().await?;
    Ok(rows)
}

pub async fn poll_access_events(
    pool: &PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
    cursor: &EventCursor,
    limit: i32,
) -> Result<Vec<StreamEventRow>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let rows = query_events(
        &mut tx,
        workspace_id,
        cursor,
        limit,
        ACCESS_VERBS,
        None,
        Some(user_id),
    )
    .await?;
    tx.commit().await?;
    Ok(rows)
}

async fn query_events(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    cursor: &EventCursor,
    limit: i32,
    verbs: &[&str],
    project_id: Option<Uuid>,
    access_user_id: Option<Uuid>,
) -> Result<Vec<StreamEventRow>, sqlx::Error> {
    let limit = limit.clamp(1, 100);
    let rows = if let Some(project_id) = project_id {
        sqlx::query_as::<_, (String, i64, String, Value)>(
            r#"
            SELECT e.xact::text, e.seq, e.verb, e.payload
            FROM fvoci.events AS e
            WHERE e.workspace_id = $1
              AND (e.xact, e.seq) > ($2::xid8, $3)
              AND e.xact < pg_catalog.pg_snapshot_xmin(pg_catalog.pg_current_snapshot())
              AND e.verb = ANY($4::text[])
              AND e.payload->>'projectId' = $5
            ORDER BY e.xact, e.seq
            LIMIT $6
            "#,
        )
        .bind(workspace_id)
        .bind(&cursor.xact)
        .bind(cursor.seq)
        .bind(verbs)
        .bind(project_id.to_string())
        .bind(limit)
        .fetch_all(&mut **tx)
        .await?
    } else {
        let user_id = access_user_id.expect("access poll needs user");
        sqlx::query_as::<_, (String, i64, String, Value)>(
            r#"
            SELECT e.xact::text, e.seq, e.verb, e.payload
            FROM fvoci.events AS e
            WHERE e.workspace_id = $1
              AND (e.xact, e.seq) > ($2::xid8, $3)
              AND e.xact < pg_catalog.pg_snapshot_xmin(pg_catalog.pg_current_snapshot())
              AND e.verb = ANY($4::text[])
              AND (
                (e.verb IN ('workspace_member.removed', 'workspace_member.role_changed')
                 AND e.target_id = $5)
                OR (e.verb = 'admin.user_suspended_set' AND e.target_id = $5)
                OR (e.verb IN ('user.withdrawn', 'user.withdraw_cancelled')
                    AND e.actor_user_id = $5)
              )
            ORDER BY e.xact, e.seq
            LIMIT $6
            "#,
        )
        .bind(workspace_id)
        .bind(&cursor.xact)
        .bind(cursor.seq)
        .bind(verbs)
        .bind(user_id)
        .bind(limit)
        .fetch_all(&mut **tx)
        .await?
    };

    Ok(rows
        .into_iter()
        .map(|(xact, seq, verb, payload)| StreamEventRow {
            xact,
            seq,
            verb,
            payload,
        })
        .collect())
}

pub fn access_event_targets_user(row: &StreamEventRow, user_id: Uuid) -> bool {
    match row.verb.as_str() {
        "workspace_member.removed" | "workspace_member.role_changed" => {
            row.payload
                .get("userId")
                .and_then(|v| v.as_str())
                .is_some_and(|id| id == user_id.to_string())
        }
        "admin.user_suspended_set" => row
            .payload
            .get("targetId")
            .and_then(|v| v.as_str())
            .is_some_and(|id| id == user_id.to_string()),
        "user.withdrawn" | "user.withdraw_cancelled" => true,
        _ => false,
    }
}
