use serde_json::Value;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::db::context::{session_is_live, set_tenant};
use crate::db::projects::project_permission_by_id;
use crate::db::workspace::membership_role;
use crate::projects::ProjectPermission;

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

const COMMENT_ACTIVITY_VERBS: &[&str] = &["comment.created"];

const ACCESS_VERBS: &[&str] = &[
    "workspace_member.removed",
    "workspace_member.role_changed",
    "admin.user_suspended_set",
    "user.withdrawn",
    "user.withdraw_cancelled",
];

/// Result of one stream access check. Every check runs in one transaction
/// under the workspace tenant (api_tokens has RLS) and takes no row lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamAccess {
    Allowed,
    /// The session or API token is revoked or expired, or its user is
    /// suspended or deleted.
    CredentialDead,
    /// The credential is live but no longer grants the project or workspace.
    Denied,
}

/// Project stream access: a live credential with at least View on a live
/// project. Used for admission and for every delivered item.
pub async fn project_stream_access(
    pool: &PgPool,
    workspace_id: Uuid,
    project_id: Uuid,
    user_id: Uuid,
    session_id: Uuid,
) -> Result<StreamAccess, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let access = if !session_is_live(&mut tx, user_id, session_id).await? {
        StreamAccess::CredentialDead
    } else if project_permission_by_id(&mut tx, workspace_id, user_id, project_id)
        .await?
        .is_some_and(|permission| permission.at_least(ProjectPermission::View))
    {
        StreamAccess::Allowed
    } else {
        StreamAccess::Denied
    };
    tx.commit().await?;
    Ok(access)
}

/// Workspace stream access: a live credential of a current member.
pub async fn workspace_stream_access(
    pool: &PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
    session_id: Uuid,
) -> Result<StreamAccess, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let access = workspace_access_in(&mut tx, workspace_id, user_id, session_id).await?;
    tx.commit().await?;
    Ok(access)
}

async fn workspace_access_in(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    user_id: Uuid,
    session_id: Uuid,
) -> Result<StreamAccess, sqlx::Error> {
    if !session_is_live(tx, user_id, session_id).await? {
        return Ok(StreamAccess::CredentialDead);
    }
    Ok(match membership_role(tx, workspace_id, user_id).await? {
        Some(_) => StreamAccess::Allowed,
        None => StreamAccess::Denied,
    })
}

pub async fn initial_cursor(pool: &PgPool, workspace_id: Uuid) -> Result<EventCursor, sqlx::Error> {
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

/// One task stream tick in one transaction: the credential check, then the
/// project's events after `cursor`. `None` once the credential is dead.
pub async fn poll_task_events(
    pool: &PgPool,
    workspace_id: Uuid,
    project_id: Uuid,
    user_id: Uuid,
    session_id: Uuid,
    cursor: &EventCursor,
    limit: i32,
) -> Result<Option<Vec<StreamEventRow>>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !session_is_live(&mut tx, user_id, session_id).await? {
        tx.commit().await?;
        return Ok(None);
    }
    let rows = query_task_project_events(&mut tx, workspace_id, project_id, cursor, limit).await?;
    tx.commit().await?;
    Ok(Some(rows))
}

/// One access stream tick in one transaction: the credential check, then
/// the access events after `cursor`. `None` once the credential is dead.
pub async fn poll_access_events(
    pool: &PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
    session_id: Uuid,
    cursor: &EventCursor,
    limit: i32,
) -> Result<Option<Vec<StreamEventRow>>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !session_is_live(&mut tx, user_id, session_id).await? {
        tx.commit().await?;
        return Ok(None);
    }
    let rows = query_access_events(&mut tx, workspace_id, user_id, cursor, limit).await?;
    tx.commit().await?;
    Ok(Some(rows))
}

/// Map a polled row to the wire `event: task` hint (`verb`, `taskId`), if any.
pub fn task_stream_wire_hint(row: &StreamEventRow) -> Option<(String, String)> {
    let task_id = row
        .payload
        .get("taskId")
        .and_then(|v| v.as_str())
        .filter(|id| !id.is_empty())?;
    let wire_verb = match row.verb.as_str() {
        "task.created" | "task.updated" | "task.deleted" => row.verb.clone(),
        v if COMMENT_ACTIVITY_VERBS.contains(&v) => "task.activity".to_string(),
        _ => return None,
    };
    Some((wire_verb, task_id.to_string()))
}

async fn query_task_project_events(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    project_id: Uuid,
    cursor: &EventCursor,
    limit: i32,
) -> Result<Vec<StreamEventRow>, sqlx::Error> {
    let limit = limit.clamp(1, 100);
    let rows = sqlx::query_as::<_, (String, i64, String, Value)>(
        r#"
        SELECT e.xact::text, e.seq, e.verb, e.payload
        FROM fvoci.events AS e
        WHERE e.workspace_id = $1
          AND (e.xact, e.seq) > ($2::xid8, $3)
          AND e.xact < pg_catalog.pg_snapshot_xmin(pg_catalog.pg_current_snapshot())
          AND (
            (
              e.verb = ANY($4::text[])
              AND e.payload->>'projectId' = $5
            )
            OR (
              e.verb = ANY($6::text[])
              AND e.payload->>'taskId' IS NOT NULL
              AND EXISTS (
                SELECT 1
                FROM fvoci.tasks AS t
                WHERE t.id = (e.payload->>'taskId')::uuid
                  AND t.project_id = $7::uuid
                  AND t.workspace_id = $1
              )
            )
          )
        ORDER BY e.xact, e.seq
        LIMIT $8
        "#,
    )
    .bind(workspace_id)
    .bind(&cursor.xact)
    .bind(cursor.seq)
    .bind(TASK_VERBS)
    .bind(project_id.to_string())
    .bind(COMMENT_ACTIVITY_VERBS)
    .bind(project_id)
    .bind(limit)
    .fetch_all(&mut **tx)
    .await?;

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

async fn query_access_events(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    user_id: Uuid,
    cursor: &EventCursor,
    limit: i32,
) -> Result<Vec<StreamEventRow>, sqlx::Error> {
    let limit = limit.clamp(1, 100);
    let rows = sqlx::query_as::<_, (String, i64, String, Value)>(
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
    .bind(ACCESS_VERBS)
    .bind(user_id)
    .bind(limit)
    .fetch_all(&mut **tx)
    .await?;

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

pub fn access_event_targets_user(row: &StreamEventRow, _user_id: Uuid) -> bool {
    ACCESS_VERBS.contains(&row.verb.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn task_stream_wire_hint_maps_comment_to_activity() {
        let row = StreamEventRow {
            xact: "1".into(),
            seq: 1,
            verb: "comment.created".into(),
            payload: json!({"taskId": "550e8400-e29b-41d4-a716-446655440000"}),
        };
        let (verb, task_id) = task_stream_wire_hint(&row).expect("hint");
        assert_eq!(verb, "task.activity");
        assert_eq!(task_id, "550e8400-e29b-41d4-a716-446655440000");
    }

    #[test]
    fn task_stream_wire_hint_passes_task_verbs() {
        let row = StreamEventRow {
            xact: "1".into(),
            seq: 2,
            verb: "task.updated".into(),
            payload: json!({
                "taskId": "550e8400-e29b-41d4-a716-446655440000",
                "projectId": "660e8400-e29b-41d4-a716-446655440001"
            }),
        };
        let (verb, _) = task_stream_wire_hint(&row).expect("hint");
        assert_eq!(verb, "task.updated");
    }
}
