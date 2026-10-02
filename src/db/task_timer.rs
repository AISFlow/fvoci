//! Durable person-wide stopwatch. Reuses the existing membership/credential
//! writer fence and task/project lock order. No process-local run state.
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::api::task_timer::{
    OwnerTimerState, TaskTimerState, TimerCleanupBody, TimerCommandBody, TimerCommandOutput,
    TimerRunOutput,
};
use crate::db::context::{
    begin_read, lock_membership_users, recheck_session, session_is_live, set_self_user, set_tenant,
};
use crate::db::projects::{load_live_project, project_permission, ProjectDbError};
use crate::db::tasks::{record_task_event_and_audit, require_task_write_access, TaskChangeRecord};
use crate::db::workspace::workspace_is_live;
use crate::projects::ProjectPermission;
use crate::task_timer::{transition, TimerOperation, TimerStatus};

#[derive(Debug)]
pub enum TimerDbError {
    Project(ProjectDbError),
    Conflict(&'static str),
    InvalidInput,
}

type DbResult<T> = Result<Result<T, TimerDbError>, sqlx::Error>;
type RunTuple = (
    Uuid,
    Uuid,
    Uuid,
    String,
    i32,
    DateTime<Utc>,
    Option<String>,
    Option<DateTime<Utc>>,
    i64,
);

fn decode_run(row: RunTuple) -> Result<TimerRunOutput, sqlx::Error> {
    let (
        id,
        workspace_id,
        task_id,
        status,
        version,
        started_at,
        note,
        running_since,
        elapsed_milliseconds,
    ) = row;
    let status = match status.as_str() {
        "running" => TimerStatus::Running,
        "paused" => TimerStatus::Paused,
        "stopped" => TimerStatus::Stopped,
        _ => return Err(sqlx::Error::Protocol("invalid stored timer status".into())),
    };
    Ok(TimerRunOutput {
        id,
        workspace_id,
        task_id,
        status,
        version,
        started_at,
        running_since,
        elapsed_milliseconds,
        note,
    })
}

async fn clock(tx: &mut Transaction<'_, Postgres>) -> Result<DateTime<Utc>, sqlx::Error> {
    sqlx::query_scalar("SELECT date_trunc('milliseconds', clock_timestamp())")
        .fetch_one(&mut **tx)
        .await
}

async fn unfinished(
    tx: &mut Transaction<'_, Postgres>,
    actor: Uuid,
) -> Result<Option<TimerRunOutput>, sqlx::Error> {
    let row: Option<RunTuple> = sqlx::query_as(
        r#"
        SELECT r.id, r.workspace_id, r.task_id, r.status, r.version, r.started_at, r.note,
            min(s.started_at) FILTER (WHERE s.ended_at IS NULL),
            COALESCE(sum(EXTRACT(EPOCH FROM (s.ended_at - s.started_at)) * 1000)
                FILTER (WHERE s.ended_at IS NOT NULL), 0)::bigint
        FROM fvoci.task_timer_runs r
        LEFT JOIN fvoci.task_timer_segments s ON s.run_id = r.id
        WHERE r.user_id = $1 AND r.status <> 'stopped'
        GROUP BY r.id
    "#,
    )
    .bind(actor)
    .fetch_optional(&mut **tx)
    .await?;
    row.map(decode_run).transpose()
}

pub(crate) async fn legacy_open(
    tx: &mut Transaction<'_, Postgres>,
    actor: Uuid,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM fvoci.task_timer_legacy_open WHERE user_id = $1)",
    )
    .bind(actor)
    .fetch_one(&mut **tx)
    .await
}

/// Used by the existing manual/open entry writer after require_task_write_access.
/// Caller already holds the person's membership and credential rows.
pub(crate) async fn person_has_unfinished(
    tx: &mut Transaction<'_, Postgres>,
    actor: Uuid,
) -> Result<bool, sqlx::Error> {
    set_self_user(tx, actor).await?;
    Ok(unfinished(tx, actor).await?.is_some() || legacy_open(tx, actor).await?)
}

async fn can_view(
    tx: &mut Transaction<'_, Postgres>,
    workspace: Uuid,
    task: Uuid,
    actor: Uuid,
) -> Result<bool, sqlx::Error> {
    set_tenant(tx, workspace).await?;
    if !workspace_is_live(tx, workspace).await? {
        return Ok(false);
    }
    let project: Option<Uuid> = sqlx::query_scalar("SELECT project_id FROM fvoci.tasks WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL")
        .bind(workspace).bind(task).fetch_optional(&mut **tx).await?;
    let Some(project) = project else {
        return Ok(false);
    };
    let Some(project) = load_live_project(tx, workspace, project).await? else {
        return Ok(false);
    };
    Ok(project_permission(tx, workspace, actor, &project)
        .await?
        .at_least(ProjectPermission::View))
}

async fn state_in(
    tx: &mut Transaction<'_, Postgres>,
    workspace: Uuid,
    task: Uuid,
    actor: Uuid,
) -> Result<TaskTimerState, sqlx::Error> {
    let active = unfinished(tx, actor).await?;
    let busy_elsewhere = active
        .as_ref()
        .is_some_and(|r| r.workspace_id != workspace || r.task_id != task);
    let run = active.filter(|r| r.workspace_id == workspace && r.task_id == task);
    // Existing time-entry history remains the shared task total. Own timer
    // fractions/cleanup intervals are added once, never double-count projections.
    let actual: i64 = sqlx::query_scalar(r#"
        SELECT COALESCE((SELECT sum(duration_seconds::bigint * 1000) FROM fvoci.time_entries
            WHERE workspace_id = $1 AND task_id = $2),0)::bigint
        + COALESCE((SELECT sum(EXTRACT(EPOCH FROM (s.ended_at - s.started_at))*1000
            - CASE WHEN s.time_entry_id IS NOT NULL THEN FLOOR(EXTRACT(EPOCH FROM (s.ended_at - s.started_at))) * 1000 ELSE 0 END)
            FROM fvoci.task_timer_segments s WHERE s.workspace_id = $1 AND s.task_id = $2 AND s.user_id = $3 AND s.ended_at IS NOT NULL),0)::bigint
    "#).bind(workspace).bind(task).bind(actor).fetch_one(&mut **tx).await?;
    Ok(TaskTimerState {
        server_now: clock(tx).await?,
        run,
        busy_elsewhere,
        legacy_open: legacy_open(tx, actor).await?,
        actual_milliseconds: actual,
    })
}

pub async fn task_state(
    pool: &PgPool,
    workspace: Uuid,
    task: Uuid,
    actor: Uuid,
    session: Uuid,
) -> DbResult<TaskTimerState> {
    let mut tx = begin_read(pool).await?;
    set_tenant(&mut tx, workspace).await?;
    set_self_user(&mut tx, actor).await?;
    if !session_is_live(&mut tx, actor, session).await? {
        return Ok(Err(TimerDbError::Project(ProjectDbError::Forbidden)));
    }
    if !can_view(&mut tx, workspace, task, actor).await? {
        return Ok(Err(TimerDbError::Project(ProjectDbError::NotFound)));
    }
    let state = state_in(&mut tx, workspace, task, actor).await?;
    tx.commit().await?;
    Ok(Ok(state))
}

pub async fn owner_state(pool: &PgPool, actor: Uuid, session: Uuid) -> DbResult<OwnerTimerState> {
    let mut tx = begin_read(pool).await?;
    set_self_user(&mut tx, actor).await?;
    if !session_is_live(&mut tx, actor, session).await? {
        return Ok(Err(TimerDbError::Project(ProjectDbError::Forbidden)));
    }
    let run = unfinished(&mut tx, actor).await?;
    let (run_id, version, status) = run
        .as_ref()
        .map(|r| (Some(r.id), Some(r.version), Some(r.status)))
        .unwrap_or_default();
    let visible_run = match run {
        Some(run) if can_view(&mut tx, run.workspace_id, run.task_id, actor).await? => Some(run),
        _ => None,
    };
    let result = OwnerTimerState {
        server_now: clock(&mut tx).await?,
        run_id,
        version,
        status,
        visible_run,
        legacy_open: legacy_open(&mut tx, actor).await?,
    };
    tx.commit().await?;
    Ok(Ok(result))
}

fn hash<T: serde::Serialize>(
    kind: &str,
    workspace: Option<Uuid>,
    task: Option<Uuid>,
    body: &T,
) -> Result<String, sqlx::Error> {
    let value = serde_json::to_vec(&(kind, workspace, task, body))
        .map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
    Ok(format!("{:x}", Sha256::digest(value)))
}

/// Receipts are read only after current credential and target permission are
/// checked. A deleted run retires its key rather than creating a replacement.
async fn replay(
    tx: &mut Transaction<'_, Postgres>,
    actor: Uuid,
    request: Uuid,
    digest: &str,
) -> DbResult<Option<Value>> {
    let row: Option<(String, Option<Uuid>, Value)> = sqlx::query_as("SELECT request_hash, run_id, result FROM fvoci.task_timer_commands WHERE user_id = $1 AND request_id = $2")
        .bind(actor).bind(request).fetch_optional(&mut **tx).await?;
    match row {
        Some((stored, _, _)) if stored != digest => {
            Ok(Err(TimerDbError::Conflict("request_mismatch")))
        }
        Some((_, None, value)) if value.get("runId").is_some() => {
            Ok(Err(TimerDbError::Conflict("timer_retired")))
        }
        Some((_, _, value)) => Ok(Ok(Some(value))),
        None => Ok(Ok(None)),
    }
}

async fn receipt(
    tx: &mut Transaction<'_, Postgres>,
    actor: Uuid,
    request: Uuid,
    digest: &str,
    run: Option<Uuid>,
    result: &Value,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO fvoci.task_timer_commands (user_id, request_id, request_hash, run_id, result) VALUES ($1,$2,$3,$4,$5)")
        .bind(actor).bind(request).bind(digest).bind(run).bind(result).execute(&mut **tx).await?;
    Ok(())
}

async fn audit(
    tx: &mut Transaction<'_, Postgres>,
    actor: Uuid,
    request: Uuid,
    workspace: Option<Uuid>,
    task: Option<Uuid>,
    entry: Option<Uuid>,
    verb: &str,
    before: Value,
    after: Value,
    reason: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO fvoci.task_timer_audit (id,user_id,request_id,workspace_id,task_id,time_entry_id,verb,before_value,after_value,reason) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)")
        .bind(Uuid::now_v7()).bind(actor).bind(request).bind(workspace).bind(task).bind(entry).bind(verb).bind(before).bind(after).bind(reason).execute(&mut **tx).await?;
    Ok(())
}

async fn open_segment(
    tx: &mut Transaction<'_, Postgres>,
    run: &TimerRunOutput,
    actor: Uuid,
    at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO fvoci.task_timer_segments (id,run_id,user_id,workspace_id,task_id,started_at) VALUES ($1,$2,$3,$4,$5,$6)")
        .bind(Uuid::now_v7()).bind(run.id).bind(actor).bind(run.workspace_id).bind(run.task_id).bind(at).execute(&mut **tx).await?;
    Ok(())
}

/// Own cleanup never reads/writes a hidden task or existing tenant history.
/// The exact interval remains in private durable segments for later ACL-aware
/// queries. Normal pause/stop also project whole-second rows into 034 history.
async fn close_segment(
    tx: &mut Transaction<'_, Postgres>,
    run: &TimerRunOutput,
    actor: Uuid,
    at: DateTime<Utc>,
    project: bool,
) -> Result<bool, sqlx::Error> {
    let row: Option<(Uuid, DateTime<Utc>)> = sqlx::query_as("SELECT id, started_at FROM fvoci.task_timer_segments WHERE run_id = $1 AND ended_at IS NULL FOR UPDATE")
        .bind(run.id).fetch_optional(&mut **tx).await?;
    let Some((id, start)) = row else {
        return Ok(false);
    };
    let end = at.max(start);
    let seconds = (end - start).num_seconds();
    let entry_id = if project && seconds > 0 {
        let entry = Uuid::now_v7();
        sqlx::query("INSERT INTO fvoci.time_entries (id,workspace_id,task_id,user_id,started_at,ended_at,duration_seconds,note) VALUES ($1,$2,$3,$4,$5,$6,$7,$8)")
            .bind(entry).bind(run.workspace_id).bind(run.task_id).bind(actor).bind(start).bind(end).bind(seconds).bind(&run.note).execute(&mut **tx).await?;
        Some(entry)
    } else {
        None
    };
    sqlx::query(
        "UPDATE fvoci.task_timer_segments SET ended_at = $2, time_entry_id = $3 WHERE id = $1",
    )
    .bind(id)
    .bind(end)
    .bind(entry_id)
    .execute(&mut **tx)
    .await?;
    Ok(at < start)
}

pub fn command_valid(body: &TimerCommandBody) -> bool {
    body.note
        .as_ref()
        .is_none_or(|n| n.encode_utf16().count() <= 2000)
        && match body.operation {
            TimerOperation::Start => body.expected_version == 0 && body.run_id.is_none(),
            _ => {
                body.expected_version > 0
                    && body.expected_version < i32::MAX
                    && body.run_id.is_some()
            }
        }
}

pub async fn command(
    pool: &PgPool,
    workspace: Uuid,
    task: Uuid,
    actor: Uuid,
    session: Uuid,
    body: &TimerCommandBody,
) -> DbResult<TimerCommandOutput> {
    if !command_valid(body) {
        return Ok(Err(TimerDbError::InvalidInput));
    }
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace).await?;
    set_self_user(&mut tx, actor).await?;
    if let Err(err) =
        require_task_write_access(&mut tx, workspace, actor, session, task, false).await?
    {
        return Ok(Err(TimerDbError::Project(err)));
    }
    let digest = hash("timer", Some(workspace), Some(task), body)?;
    match replay(&mut tx, actor, body.request_id, &digest).await? {
        Err(err) => return Ok(Err(err)),
        Ok(Some(value)) => {
            return Ok(Ok(
                serde_json::from_value(value).map_err(|e| sqlx::Error::Protocol(e.to_string()))?
            ))
        }
        Ok(None) => {}
    }
    let active = unfinished(&mut tx, actor).await?;
    let at = clock(&mut tx).await?;
    let mut regression = false;
    let (id, status, version) = if body.operation == TimerOperation::Start {
        if active.is_some() || legacy_open(&mut tx, actor).await? {
            return Ok(Err(TimerDbError::Conflict("timer_busy")));
        }
        let run = TimerRunOutput {
            id: Uuid::now_v7(),
            workspace_id: workspace,
            task_id: task,
            status: TimerStatus::Running,
            version: 1,
            started_at: at,
            running_since: Some(at),
            elapsed_milliseconds: 0,
            note: body.note.clone(),
        };
        sqlx::query("INSERT INTO fvoci.task_timer_runs (id,user_id,workspace_id,task_id,status,version,started_at,note) VALUES ($1,$2,$3,$4,'running',1,$5,$6)")
            .bind(run.id).bind(actor).bind(workspace).bind(task).bind(at).bind(&run.note).execute(&mut *tx).await?;
        open_segment(&mut tx, &run, actor, at).await?;
        (run.id, run.status, run.version)
    } else {
        let Some(mut run) = active else {
            return Ok(Err(TimerDbError::Conflict("timer_version")));
        };
        if Some(run.id) != body.run_id
            || run.workspace_id != workspace
            || run.task_id != task
            || run.version != body.expected_version
        {
            return Ok(Err(TimerDbError::Conflict("timer_version")));
        }
        let Some(status) = transition(run.status, body.operation) else {
            return Ok(Err(TimerDbError::Conflict("timer_transition")));
        };
        if body.note.is_some() {
            run.note = body.note.clone();
        }
        if run.status == TimerStatus::Running {
            regression = close_segment(&mut tx, &run, actor, at, true).await?;
        }
        if body.operation == TimerOperation::Resume {
            open_segment(&mut tx, &run, actor, at).await?;
        }
        let version = run.version + 1;
        sqlx::query("UPDATE fvoci.task_timer_runs SET status=$2,version=$3,note=$4,stopped_at=$5 WHERE id=$1")
            .bind(run.id).bind(status.as_str()).bind(version).bind(&run.note).bind((status == TimerStatus::Stopped).then_some(at.max(run.started_at))).execute(&mut *tx).await?;
        (run.id, status, version)
    };
    let output = TimerCommandOutput {
        run_id: id,
        version,
        status,
        server_now: at,
    };
    let value = serde_json::to_value(&output).map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
    let before = json!({"runId":body.run_id,"expectedVersion":body.expected_version});
    let reason = if regression {
        "server_clock_regression_clamped"
    } else {
        "user_command"
    };
    audit(
        &mut tx,
        actor,
        body.request_id,
        Some(workspace),
        Some(task),
        None,
        status.as_str(),
        before,
        value.clone(),
        reason,
    )
    .await?;
    record_task_event_and_audit(
        &mut tx,
        TaskChangeRecord {
            workspace_id: workspace,
            actor_user_id: actor,
            verb: "task.timer.changed",
            target_type: "task",
            target_id: task,
            payload: json!({"runId":id,"version":version,"status":status}),
            client_ip: None,
        },
    )
    .await?;
    receipt(&mut tx, actor, body.request_id, &digest, Some(id), &value).await?;
    tx.commit().await?;
    Ok(Ok(output))
}

pub async fn cleanup(
    pool: &PgPool,
    actor: Uuid,
    session: Uuid,
    body: &TimerCleanupBody,
) -> DbResult<TimerCommandOutput> {
    if body.expected_version <= 0 || body.expected_version == i32::MAX {
        return Ok(Err(TimerDbError::InvalidInput));
    }
    let mut tx = pool.begin().await?;
    set_self_user(&mut tx, actor).await?;
    lock_membership_users(&mut tx, &[actor]).await?;
    if !recheck_session(&mut tx, actor, session).await? {
        return Ok(Err(TimerDbError::Project(ProjectDbError::Forbidden)));
    }
    let digest = hash("cleanup", None, None, body)?;
    match replay(&mut tx, actor, body.request_id, &digest).await? {
        Err(err) => return Ok(Err(err)),
        Ok(Some(value)) => {
            return Ok(Ok(
                serde_json::from_value(value).map_err(|e| sqlx::Error::Protocol(e.to_string()))?
            ))
        }
        Ok(None) => {}
    }
    let Some(run) = unfinished(&mut tx, actor).await? else {
        return Ok(Err(TimerDbError::Conflict("timer_version")));
    };
    if run.id != body.run_id || run.version != body.expected_version {
        return Ok(Err(TimerDbError::Conflict("timer_version")));
    }
    let at = clock(&mut tx).await?;
    let regression = close_segment(&mut tx, &run, actor, at, false).await?;
    let version = run.version + 1;
    sqlx::query(
        "UPDATE fvoci.task_timer_runs SET status='stopped',version=$2,stopped_at=$3 WHERE id=$1",
    )
    .bind(run.id)
    .bind(version)
    .bind(at.max(run.started_at))
    .execute(&mut *tx)
    .await?;
    let output = TimerCommandOutput {
        run_id: run.id,
        version,
        status: TimerStatus::Stopped,
        server_now: at,
    };
    let value = serde_json::to_value(&output).map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
    audit(
        &mut tx,
        actor,
        body.request_id,
        None,
        None,
        None,
        "cleanup",
        json!({"version":run.version}),
        value.clone(),
        if regression {
            "self_cleanup_clock_regression_clamped"
        } else {
            "explicit_self_cleanup"
        },
    )
    .await?;
    receipt(
        &mut tx,
        actor,
        body.request_id,
        &digest,
        Some(run.id),
        &value,
    )
    .await?;
    tx.commit().await?;
    Ok(Ok(output))
}
