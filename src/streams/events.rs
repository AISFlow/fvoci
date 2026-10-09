use serde_json::Value;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::db::context::{session_is_live, set_tenant};
use crate::db::projects::project_permission_by_id;
use crate::db::workspace::membership_role;
use crate::projects::ProjectPermission;

/// Position in the event log, in `(xact, seq)` order. `xact` is an xid8 in
/// text form.
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

/// Events after a cursor, and where the next poll starts.
#[derive(Debug, Clone)]
pub struct EventPage {
    pub rows: Vec<StreamEventRow>,
    /// The last row of a full page. Otherwise `(horizon, 0)`: past every
    /// settled event, matching or not, so the next poll does not rescan them.
    pub next: EventCursor,
}

/// Every transaction with an xid below this horizon had ended when it was
/// read, so a later statement sees all of their committed events. Read it
/// before the events query, never after: under READ COMMITTED a later
/// horizon could pass rows that the query did not see. `seq` starts at 1,
/// so `(horizon, 0)` sorts before every event of the horizon transaction.
const HORIZON_SQL: &str =
    "SELECT pg_catalog.pg_snapshot_xmin(pg_catalog.pg_current_snapshot())::text";

async fn settled_horizon(tx: &mut Transaction<'_, Postgres>) -> Result<String, sqlx::Error> {
    sqlx::query_scalar(HORIZON_SQL).fetch_one(&mut **tx).await
}

const TASK_VERBS: &[&str] = &["task.created", "task.updated", "task.deleted"];

const COMMENT_ACTIVITY_VERBS: &[&str] = &["comment.created"];

/// Membership changes aimed at one user (`target_id` = user). The access
/// stream closes on them so the client refetches its workspace list.
/// Suspension, withdrawal and credential revocation are instance-level (no
/// workspace_id) and reach the stream through the per-tick credential check.
const MEMBER_ACCESS_VERBS: &[&str] = &["workspace_member.removed", "workspace_member.role_changed"];

/// Workspace-wide access changes, for every member.
const WORKSPACE_ACCESS_VERBS: &[&str] = &["workspace.deleted"];

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

/// Access-stream cursor only: PG transaction ordering and family committed
/// sequence are distinct; neither is exposed as a client/transport credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BackendAccessCursor {
    Postgres(EventCursor),
    SqliteFamily { seq: i64 },
}

pub(crate) async fn workspace_stream_access_backend(
    backend: &crate::db::backend::Backend,
    workspace_id: Uuid,
    user_id: Uuid,
    credential_id: Uuid,
) -> Result<StreamAccess, sqlx::Error> {
    if let crate::db::backend::Backend::Postgres(pool) = backend {
        return workspace_stream_access(pool, workspace_id, user_id, credential_id).await;
    }
    let mut tx = backend.begin_read().await?;
    let result = async {
        let mut op = tx.operation();
        op.set_tenant(workspace_id).await?;
        family_workspace_access(&mut op, workspace_id, user_id, credential_id).await
    }
    .await;
    let cleanup = tx.rollback().await;
    access_read_after_rollback(result, cleanup)
}

async fn family_workspace_access(
    op: &mut crate::db::backend::OperationTx<'_, '_>,
    workspace_id: Uuid,
    user_id: Uuid,
    credential_id: Uuid,
) -> Result<StreamAccess, sqlx::Error> {
    if !op.session_is_live(user_id, credential_id).await? {
        return Ok(StreamAccess::CredentialDead);
    }
    if !op.workspace_is_live(workspace_id).await?
        || op
            .membership_role(workspace_id, user_id, false)
            .await?
            .is_none()
    {
        return Ok(StreamAccess::Denied);
    }
    Ok(StreamAccess::Allowed)
}

pub(crate) async fn initial_access_cursor_backend(
    backend: &crate::db::backend::Backend,
) -> Result<BackendAccessCursor, sqlx::Error> {
    if let crate::db::backend::Backend::Postgres(pool) = backend {
        return initial_cursor(pool)
            .await
            .map(BackendAccessCursor::Postgres);
    }
    let mut tx = backend.begin_read().await?;
    let result = async {
        let crate::db::backend::DbTransaction::SqliteFamily(family) = &mut tx else {
            unreachable!()
        };
        Ok(BackendAccessCursor::SqliteFamily {
            seq: family_access_horizon(family).await?,
        })
    }
    .await;
    let cleanup = tx.rollback().await;
    access_read_after_rollback(result, cleanup)
}

/// Same snapshot as authority and event query. The counter survives event
/// retention, and a serialized writer rolls its allocation back with its event.
async fn family_access_horizon(
    family: &mut crate::db::backend::FamilyTx,
) -> Result<i64, sqlx::Error> {
    let rows = family
        .query("SELECT last_seq FROM event_sequence WHERE id=1", &[])
        .await?;
    let seq = rows
        .first()
        .ok_or(sqlx::Error::RowNotFound)?
        .cell(0)?
        .integer()?;
    if seq < 0 {
        return Err(sqlx::Error::Protocol("invalid access event horizon".into()));
    }
    Ok(seq)
}

pub(crate) async fn poll_access_events_backend(
    backend: &crate::db::backend::Backend,
    workspace_id: Uuid,
    user_id: Uuid,
    credential_id: Uuid,
    cursor: &BackendAccessCursor,
) -> Result<Option<BackendAccessCursor>, sqlx::Error> {
    use crate::db::backend::{Backend, OperationTx};
    use crate::db::codec::Cell;
    match (backend, cursor) {
        (Backend::Postgres(pool), BackendAccessCursor::Postgres(cursor)) => {
            return poll_access_events(pool, workspace_id, user_id, credential_id, cursor)
                .await
                .map(|next| next.map(BackendAccessCursor::Postgres));
        }
        (
            Backend::Sqlite(_) | Backend::LibsqlRemote(_),
            BackendAccessCursor::SqliteFamily { seq },
        ) if *seq >= 0 => {}
        _ => {
            return Err(sqlx::Error::Protocol(
                "access cursor belongs to a different backend or is invalid".into(),
            ))
        }
    }
    let BackendAccessCursor::SqliteFamily { seq } = cursor else {
        unreachable!()
    };
    let mut tx = backend.begin_read().await?;
    let result = async {
        let mut op = tx.operation();
        op.set_tenant(workspace_id).await?;
        if family_workspace_access(&mut op,workspace_id,user_id,credential_id).await? != StreamAccess::Allowed {
            return Ok(None);
        }
        let OperationTx::SqliteFamily(family) = &mut op else { unreachable!() };
        let horizon = family_access_horizon(family).await?;
        if *seq > horizon {
            return Err(sqlx::Error::Protocol("access cursor is ahead of committed horizon".into()));
        }
        let rows = family.query(
            "SELECT EXISTS(SELECT 1 FROM events WHERE workspace_id=?1 AND seq>?2 AND seq<=?3
              AND ((verb IN ('workspace_member.removed','workspace_member.role_changed') AND target_id=?4)
                   OR verb='workspace.deleted'))",
            &[Cell::uuid(workspace_id),Cell::Integer(*seq),Cell::Integer(horizon),Cell::uuid(user_id)],
        ).await?;
        let changed = rows.first().ok_or(sqlx::Error::RowNotFound)?.cell(0)?.boolean()?;
        Ok((!changed).then_some(BackendAccessCursor::SqliteFamily {seq:horizon}))
    }.await;
    let cleanup = tx.rollback().await;
    access_read_after_rollback(result, cleanup)
}

// Narrow access-reader finish, not a new backend cleanup mechanism. Only an
// actual awaited rollback permits returning observations from this snapshot.
fn access_read_after_rollback<T>(
    result: Result<T, sqlx::Error>,
    cleanup: Result<(), sqlx::Error>,
) -> Result<T, sqlx::Error> {
    match cleanup {
        Ok(()) => result,
        Err(cleanup) => Err(crate::db::backend::rollback_cleanup_unknown(
            result
                .err()
                .map(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>),
            cleanup,
        )),
    }
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

/// Where a new stream starts: past every settled event. Events of
/// transactions still open now sort after it and are delivered once settled;
/// the client's resync on `open` covers everything before it.
pub async fn initial_cursor(pool: &PgPool) -> Result<EventCursor, sqlx::Error> {
    let horizon: String = sqlx::query_scalar(HORIZON_SQL).fetch_one(pool).await?;
    Ok(EventCursor {
        xact: horizon,
        seq: 0,
    })
}

/// One task stream tick in one transaction: the credential check, then up
/// to `limit` (clamped to 1..=100) project events after `cursor`. `None` once
/// the credential is dead.
pub async fn poll_task_events(
    pool: &PgPool,
    workspace_id: Uuid,
    project_id: Uuid,
    user_id: Uuid,
    session_id: Uuid,
    cursor: &EventCursor,
    limit: i32,
) -> Result<Option<EventPage>, sqlx::Error> {
    let limit = limit.clamp(1, 100);
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !session_is_live(&mut tx, user_id, session_id).await? {
        tx.commit().await?;
        return Ok(None);
    }
    let horizon = settled_horizon(&mut tx).await?;
    let rows =
        query_task_project_events(&mut tx, workspace_id, project_id, cursor, &horizon, limit)
            .await?;
    tx.commit().await?;
    let next = match rows.last() {
        Some(last) if rows.len() >= limit as usize => EventCursor {
            xact: last.xact.clone(),
            seq: last.seq,
        },
        _ => EventCursor {
            xact: horizon,
            seq: 0,
        },
    };
    Ok(Some(EventPage { rows, next }))
}

/// One access stream tick in one transaction: the credential and
/// membership checks, then the access events after `cursor`. `None` ends the
/// stream (credential dead, no longer a member, or an access event for this
/// user); otherwise the cursor for the next tick, `(horizon, 0)`.
pub async fn poll_access_events(
    pool: &PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
    session_id: Uuid,
    cursor: &EventCursor,
) -> Result<Option<EventCursor>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if workspace_access_in(&mut tx, workspace_id, user_id, session_id).await?
        != StreamAccess::Allowed
    {
        tx.commit().await?;
        return Ok(None);
    }
    let horizon = settled_horizon(&mut tx).await?;
    let changed = access_event_after(&mut tx, workspace_id, user_id, cursor, &horizon).await?;
    tx.commit().await?;
    Ok((!changed).then_some(EventCursor {
        xact: horizon,
        seq: 0,
    }))
}

/// Events of one workspace tick, each with the project it belongs to, and
/// where the next poll starts (same paging rule as [`EventPage`]).
#[derive(Debug, Clone)]
pub struct WorkspaceEventPage {
    /// Only rows of projects the actor can View, with that project.
    pub rows: Vec<(Uuid, StreamEventRow)>,
    pub next: EventCursor,
}

/// One workspace task stream tick in one transaction: the credential and
/// membership checks, then up to `limit` (clamped to 1..=100) task and comment
/// events after `cursor` across the workspace, kept only for projects the
/// actor can View in THIS transaction (nothing is cached from admission).
/// `None` ends the stream: the credential is dead or the membership is gone.
pub async fn poll_workspace_task_events(
    pool: &PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
    session_id: Uuid,
    cursor: &EventCursor,
    limit: i32,
) -> Result<Option<WorkspaceEventPage>, sqlx::Error> {
    let limit = limit.clamp(1, 100);
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if workspace_access_in(&mut tx, workspace_id, user_id, session_id).await?
        != StreamAccess::Allowed
    {
        tx.commit().await?;
        return Ok(None);
    }
    let horizon = settled_horizon(&mut tx).await?;
    let rows = query_task_workspace_events(&mut tx, workspace_id, cursor, &horizon, limit).await?;
    let mut visible = std::collections::HashMap::new();
    let mut kept = Vec::new();
    for (project_id, row) in &rows {
        // A comment whose task is gone has no project: like the per-project
        // query, nothing is hinted.
        let Some(project_id) = project_id else {
            continue;
        };
        let allowed = match visible.get(project_id) {
            Some(allowed) => *allowed,
            None => {
                let allowed = project_permission_by_id(&mut tx, workspace_id, user_id, *project_id)
                    .await?
                    .is_some_and(|permission| permission.at_least(ProjectPermission::View));
                visible.insert(*project_id, allowed);
                allowed
            }
        };
        if allowed {
            kept.push((*project_id, row.clone()));
        }
    }
    tx.commit().await?;
    // Paging follows every fetched row, kept or not, so hidden or orphaned
    // rows are not rescanned and a full page continues from its last row.
    let next = match rows.last() {
        Some((_, last)) if rows.len() >= limit as usize => EventCursor {
            xact: last.xact.clone(),
            seq: last.seq,
        },
        _ => EventCursor {
            xact: horizon,
            seq: 0,
        },
    };
    Ok(Some(WorkspaceEventPage { rows: kept, next }))
}

/// Task and comment events of every project in the workspace, each with
/// its project resolved like the per-project query (task events by payload
/// projectId, comment events by their task's current project; `None` when
/// that task is gone). Every fetched row is returned so the caller pages on
/// the raw page. The CASE keeps the uuid cast to comment events, as the
/// per-project EXISTS does.
async fn query_task_workspace_events(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    cursor: &EventCursor,
    horizon: &str,
    limit: i32,
) -> Result<Vec<(Option<Uuid>, StreamEventRow)>, sqlx::Error> {
    let rows = sqlx::query_as::<_, (String, i64, String, Value, Option<String>)>(
        r#"
        SELECT e.xact::text, e.seq, e.verb, e.payload,
               CASE
                 WHEN e.verb = ANY($4::text[]) THEN e.payload->>'projectId'
                 ELSE (
                   SELECT t.project_id::text
                   FROM fvoci.tasks AS t
                   WHERE t.id = (e.payload->>'taskId')::uuid
                     AND t.workspace_id = $1
                 )
               END
        FROM fvoci.events AS e
        WHERE e.workspace_id = $1
          AND (e.xact, e.seq) > ($2::xid8, $3)
          AND e.xact < $7::xid8
          AND (
            (e.verb = ANY($4::text[]) AND e.payload->>'projectId' IS NOT NULL)
            OR (e.verb = ANY($5::text[]) AND e.payload->>'taskId' IS NOT NULL)
          )
        ORDER BY e.xact, e.seq
        LIMIT $6
        "#,
    )
    .bind(workspace_id)
    .bind(&cursor.xact)
    .bind(cursor.seq)
    .bind(TASK_VERBS)
    .bind(COMMENT_ACTIVITY_VERBS)
    .bind(limit)
    .bind(horizon)
    .fetch_all(&mut **tx)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(xact, seq, verb, payload, project)| {
            (
                project.and_then(|id| Uuid::parse_str(&id).ok()),
                StreamEventRow {
                    xact,
                    seq,
                    verb,
                    payload,
                },
            )
        })
        .collect())
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
    horizon: &str,
    limit: i32,
) -> Result<Vec<StreamEventRow>, sqlx::Error> {
    let rows = sqlx::query_as::<_, (String, i64, String, Value)>(
        r#"
        SELECT e.xact::text, e.seq, e.verb, e.payload
        FROM fvoci.events AS e
        WHERE e.workspace_id = $1
          AND (e.xact, e.seq) > ($2::xid8, $3)
          AND e.xact < $9::xid8
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
    .bind(horizon)
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

async fn access_event_after(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    user_id: Uuid,
    cursor: &EventCursor,
    horizon: &str,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM fvoci.events AS e
            WHERE e.workspace_id = $1
              AND (e.xact, e.seq) > ($2::xid8, $3)
              AND e.xact < $7::xid8
              AND (
                (e.verb = ANY($4::text[]) AND e.target_id = $5)
                OR e.verb = ANY($6::text[])
              )
        )
        "#,
    )
    .bind(workspace_id)
    .bind(&cursor.xact)
    .bind(cursor.seq)
    .bind(MEMBER_ACCESS_VERBS)
    .bind(user_id)
    .bind(WORKSPACE_ACCESS_VERBS)
    .bind(horizon)
    .fetch_one(&mut **tx)
    .await
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

#[cfg(all(test, feature = "db-tests"))]
mod selected_access_read_tests {
    use super::*;
    use crate::db::backend::Backend;
    use crate::db::notifications::family_runtime_fixture::Fixture;

    async fn append(f: &Fixture, verb: &str, target: Uuid, workspace: Uuid, commit: bool) {
        let mut tx = f.backend.begin_write().await.unwrap();
        tx.operation().set_tenant(workspace).await.unwrap();
        tx.operation()
            .append_event(crate::db::identity::EventAppend {
                id: Uuid::now_v7(),
                workspace_id: Some(workspace),
                actor_user_id: Some(f.actor),
                verb: verb.into(),
                target_type: Some("workspace_member".into()),
                target_id: Some(target),
                payload: serde_json::json!({}),
            })
            .await
            .unwrap();
        if commit {
            tx.commit().await.unwrap()
        } else {
            tx.rollback().await.unwrap()
        }
    }
    async fn tick(f: &Fixture, cursor: &BackendAccessCursor) -> Option<BackendAccessCursor> {
        poll_access_events_backend(&f.backend, f.workspace, f.user, f.credential, cursor)
            .await
            .unwrap()
    }
    async fn close(f: Fixture) {
        f.backend.close().await.unwrap();
        std::fs::remove_dir_all(f.dir).unwrap();
    }

    #[tokio::test]
    async fn sqlite_access_role_change_targets_and_wrong_workspace() {
        let f = Fixture::new().await;
        let mut cursor = initial_access_cursor_backend(&f.backend).await.unwrap();
        append(
            &f,
            "workspace_member.role_changed",
            f.actor,
            f.workspace,
            true,
        )
        .await;
        cursor = tick(&f, &cursor)
            .await
            .expect("bystander role change must not close");
        append(
            &f,
            "workspace_member.role_changed",
            f.user,
            f.other_workspace,
            true,
        )
        .await;
        cursor = tick(&f, &cursor)
            .await
            .expect("other tenant event must not close");
        append(
            &f,
            "workspace_member.role_changed",
            f.user,
            f.workspace,
            true,
        )
        .await;
        assert!(
            tick(&f, &cursor).await.is_none(),
            "targeted role change closes even with membership still live"
        );
        assert_eq!(
            workspace_stream_access_backend(&f.backend, f.workspace, f.user, f.credential)
                .await
                .unwrap(),
            StreamAccess::Allowed
        );
        close(f).await;
    }

    #[tokio::test]
    async fn sqlite_access_counter_retention_rollback_and_inflight_writer() {
        let f = Fixture::new().await;
        append(&f, "unrelated", f.user, f.workspace, true).await;
        let cursor = initial_access_cursor_backend(&f.backend).await.unwrap();
        assert_eq!(cursor, BackendAccessCursor::SqliteFamily { seq: 1 });
        sqlx::query("DELETE FROM events")
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(
            initial_access_cursor_backend(&f.backend).await.unwrap(),
            cursor,
            "counter survives empty retained log"
        );
        append(
            &f,
            "workspace_member.role_changed",
            f.user,
            f.workspace,
            false,
        )
        .await;
        assert_eq!(
            tick(&f, &cursor).await,
            Some(cursor.clone()),
            "rolled-back event and allocation stay invisible"
        );
        let fresh_pool = crate::db::pool::connect_sqlite_app(&f.dir.join("test.sqlite"), 1)
            .await
            .unwrap();
        let fresh = Backend::Sqlite(fresh_pool);
        let mut writer = f.backend.begin_write().await.unwrap();
        writer.operation().set_tenant(f.workspace).await.unwrap();
        writer
            .operation()
            .append_event(crate::db::identity::EventAppend {
                id: Uuid::now_v7(),
                workspace_id: Some(f.workspace),
                actor_user_id: Some(f.actor),
                verb: "workspace_member.role_changed".into(),
                target_type: Some("workspace_member".into()),
                target_id: Some(f.user),
                payload: serde_json::json!({}),
            })
            .await
            .unwrap();
        let during = initial_access_cursor_backend(&fresh).await.unwrap();
        assert_eq!(during, cursor, "reader cannot jump past uncommitted writer");
        writer.commit().await.unwrap();
        assert!(
            poll_access_events_backend(&fresh, f.workspace, f.user, f.credential, &during)
                .await
                .unwrap()
                .is_none(),
            "event committed after cursor is observed"
        );
        fresh.close().await.unwrap();
        close(f).await;
    }

    #[tokio::test]
    async fn sqlite_access_current_credential_membership_workspace_and_cursor_denials() {
        let f = Fixture::new().await;
        let cursor = initial_access_cursor_backend(&f.backend).await.unwrap();
        assert_eq!(
            workspace_stream_access_backend(&f.backend, f.other_workspace, f.user, f.credential)
                .await
                .unwrap(),
            StreamAccess::Denied
        );
        for sql in [
            "UPDATE users SET suspended_at=1 WHERE id=?1",
            "UPDATE users SET deleted_at=1 WHERE id=?1",
        ] {
            sqlx::query(sql)
                .bind(f.user.as_bytes().as_slice())
                .execute(&f.pool)
                .await
                .unwrap();
            assert_eq!(
                workspace_stream_access_backend(&f.backend, f.workspace, f.user, f.credential)
                    .await
                    .unwrap(),
                StreamAccess::CredentialDead
            );
            assert!(tick(&f, &cursor).await.is_none());
            sqlx::query("UPDATE users SET suspended_at=NULL,deleted_at=NULL WHERE id=?1")
                .bind(f.user.as_bytes().as_slice())
                .execute(&f.pool)
                .await
                .unwrap();
        }
        sqlx::query("UPDATE sessions SET revoked_at=1 WHERE id=?1")
            .bind(f.credential.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(tick(&f, &cursor).await.is_none());
        sqlx::query("UPDATE sessions SET revoked_at=NULL,expires_at=1 WHERE id=?1")
            .bind(f.credential.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(tick(&f, &cursor).await.is_none());
        sqlx::query("UPDATE sessions SET expires_at=?2 WHERE id=?1")
            .bind(f.credential.as_bytes().as_slice())
            .bind(chrono::Utc::now().timestamp_micros() + 3_600_000_000i64)
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(poll_access_events_backend(
            &f.backend,
            f.workspace,
            f.user,
            f.credential,
            &BackendAccessCursor::Postgres(EventCursor::default())
        )
        .await
        .is_err());
        assert!(poll_access_events_backend(
            &f.backend,
            f.workspace,
            f.user,
            f.credential,
            &BackendAccessCursor::SqliteFamily { seq: -1 }
        )
        .await
        .is_err());
        assert!(poll_access_events_backend(
            &f.backend,
            f.workspace,
            f.user,
            f.credential,
            &BackendAccessCursor::SqliteFamily { seq: 1 }
        )
        .await
        .is_err());
        sqlx::query("UPDATE workspaces SET deleted_at=1 WHERE id=?1")
            .bind(f.workspace.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(tick(&f, &cursor).await.is_none());
        sqlx::query("UPDATE workspaces SET deleted_at=NULL WHERE id=?1")
            .bind(f.workspace.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM memberships WHERE workspace_id=?1 AND user_id=?2")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(tick(&f, &cursor).await.is_none());
        close(f).await;
    }

    #[test]
    fn access_cleanup_uncertainty_withholds_observation_and_retains_driver_cause() {
        let error = access_read_after_rollback(
            Ok(StreamAccess::Allowed),
            Err(sqlx::Error::Protocol(
                "synthetic returned cleanup failure".into(),
            )),
        )
        .unwrap_err();
        assert!(crate::db::backend::is_rollback_cleanup_unknown(&error));
        let error = access_read_after_rollback::<StreamAccess>(
            Err(sqlx::Error::Protocol("original read fault".into())),
            Err(sqlx::Error::Protocol(
                "synthetic returned cleanup failure".into(),
            )),
        )
        .unwrap_err();
        let sqlx::Error::AnyDriverError(source) = error else {
            panic!("typed cleanup required")
        };
        let receipt = source
            .downcast_ref::<crate::db::backend::RollbackCleanupUnknown>()
            .unwrap();
        assert!(
            matches!(receipt.original.as_ref().unwrap().downcast_ref::<sqlx::Error>(),Some(sqlx::Error::Protocol(message)) if message=="original read fault")
        );
        // Pure propagation seam only: not an actual remote rollback/settlement.
    }
}
