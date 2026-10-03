//! Durable person-wide stopwatch. Reuses the existing membership/credential
//! writer fence and task/project lock order. No process-local run state.
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::{DateTime, NaiveDate, Utc};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::api::task_timer::{
    LegacyReleaseBody, LegacyReleaseOutput, OwnerTimerState, StudyPlanTaskBody,
    StudyPlanTaskOutput, TaskEstimate, TaskEstimateCommandBody, TaskTimerState, TimeCorrectionBody,
    TimeRecord, TimeRecordKind, TimerCleanupBody, TimerCommandBody, TimerCommandOutput,
    TimerDayTotal, TimerHistory, TimerHistoryQuery, TimerManualBody, TimerRecordOutput,
    TimerRunOutput, TimerSummary, TimerSummaryQuery,
};
use crate::db::context::{
    begin_read, lock_membership_users, recheck_session, session_is_live, set_self_user, set_tenant,
};
use crate::db::projects::{load_live_project, project_permission, ProjectDbError};
use crate::db::task_origins::{
    create_document_task_tx, DocumentTaskOutcome, DocumentTaskRequest, TaskOriginDbError,
};
use crate::db::tasks::{
    record_task_event_and_audit, require_task_write_access, CreateTaskInput, TaskChangeRecord,
};
use crate::db::workspace::workspace_is_live;
use crate::projects::ProjectPermission;
use crate::task_timer::{transition, TimerOperation, TimerStatus};

#[derive(Debug)]
pub enum TimerDbError {
    Project(ProjectDbError),
    Origin(TaskOriginDbError),
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
    can_control: bool,
) -> Result<TaskTimerState, sqlx::Error> {
    let active = unfinished(tx, actor).await?;
    let busy_elsewhere = active
        .as_ref()
        .is_some_and(|r| r.workspace_id != workspace || r.task_id != task);
    let run = active.filter(|r| r.workspace_id == workspace && r.task_id == task);
    // Personal effective closed intervals include canonical fractions/cleanup
    // once and exclude duplicate projections and other actors' history.
    let actual: i64 = sqlx::query_scalar(&format!("WITH records AS ({RECORDS_SQL}) SELECT COALESCE(sum(EXTRACT(EPOCH FROM(date_trunc('milliseconds',ended_at)-date_trunc('milliseconds',started_at)))*1000),0)::bigint FROM records WHERE ended_at IS NOT NULL"))
        .bind(workspace).bind(task).bind(actor).fetch_one(&mut **tx).await?;
    Ok(TaskTimerState {
        server_now: clock(tx).await?,
        run,
        busy_elsewhere,
        legacy_open: legacy_open(tx, actor).await?,
        can_control,
        actual_milliseconds: actual,
        estimate: estimate_in(tx, workspace, task).await?,
    })
}

async fn estimate_in(
    tx: &mut Transaction<'_, Postgres>,
    workspace: Uuid,
    task: Uuid,
) -> Result<TaskEstimate, sqlx::Error> {
    let (value, unit, updated_at): (Option<String>, Option<String>, DateTime<Utc>) =
        sqlx::query_as("SELECT estimate::text, estimate_unit, updated_at FROM fvoci.tasks WHERE workspace_id=$1 AND id=$2 AND deleted_at IS NULL")
            .bind(workspace).bind(task).fetch_one(&mut **tx).await?;
    Ok(TaskEstimate {
        value,
        unit,
        updated_at,
    })
}

/// Uses the same actor/credential/project/task writer fence as ordinary task
/// metadata. Authorized replay returns the committed snapshot without applying
/// the estimate again, including after a subsequent metadata edit.
pub async fn set_estimate(
    pool: &PgPool,
    workspace: Uuid,
    task: Uuid,
    actor: Uuid,
    session: Uuid,
    body: &TaskEstimateCommandBody,
) -> DbResult<TaskEstimate> {
    if body.expected_actor_id != actor {
        return Ok(Err(TimerDbError::Conflict("timer_context_changed")));
    }
    if body.minutes.is_some_and(|minutes| minutes < 0)
        || !note_reason_valid(&None, &body.reason)
        || body
            .expected
            .unit
            .as_deref()
            .is_some_and(|unit| unit != "minutes")
    {
        return Ok(Err(TimerDbError::InvalidInput));
    }
    let mut tx = pool.begin().await?;
    if let Err(err) = write_allowed(&mut tx, workspace, task, actor, session).await? {
        return Ok(Err(err));
    }
    let digest = hash("estimate.minutes", Some(workspace), Some(task), body)?;
    match replay(&mut tx, actor, body.request_id, &digest).await? {
        Err(err) => return Ok(Err(err)),
        Ok(Some(value)) => {
            return Ok(Ok(serde_json::from_value(value)
                .map_err(|err| sqlx::Error::Protocol(err.to_string()))?))
        }
        Ok(None) => {}
    }
    if body.expected_session_id != session {
        return Ok(Err(TimerDbError::Conflict("timer_context_changed")));
    }
    let before = estimate_in(&mut tx, workspace, task).await?;
    if before != body.expected {
        return Ok(Err(TimerDbError::Conflict("estimate_changed")));
    }
    let output = persist_estimate(
        &mut tx,
        workspace,
        task,
        actor,
        body.request_id,
        before,
        body.minutes,
        body.reason.trim(),
    )
    .await?;
    let value =
        serde_json::to_value(&output).map_err(|err| sqlx::Error::Protocol(err.to_string()))?;
    receipt(&mut tx, actor, body.request_id, &digest, None, &value).await?;
    tx.commit().await?;
    Ok(Ok(output))
}

/// Shared only by the two real estimate consumers, inside their already
/// authorized/locked transaction. This does not acquire a different fence.
#[allow(clippy::too_many_arguments)] // Explicit locator, raw CAS baseline and correction reason.
async fn persist_estimate(
    tx: &mut Transaction<'_, Postgres>,
    workspace: Uuid,
    task: Uuid,
    actor: Uuid,
    request: Uuid,
    before: TaskEstimate,
    minutes: Option<i32>,
    reason: &str,
) -> Result<TaskEstimate, sqlx::Error> {
    sqlx::query("UPDATE fvoci.tasks SET estimate=$3::integer::numeric, estimate_unit=CASE WHEN $3::integer IS NULL THEN NULL ELSE 'minutes' END, updated_at=clock_timestamp() WHERE workspace_id=$1 AND id=$2")
        .bind(workspace).bind(task).bind(minutes).execute(&mut **tx).await?;
    let output = estimate_in(tx, workspace, task).await?;
    let value =
        serde_json::to_value(&output).map_err(|err| sqlx::Error::Protocol(err.to_string()))?;
    audit(
        tx,
        actor,
        request,
        Some(workspace),
        Some(task),
        None,
        "task.estimate.minutes",
        serde_json::to_value(before).map_err(|err| sqlx::Error::Protocol(err.to_string()))?,
        value.clone(),
        reason,
    )
    .await?;
    record_task_event_and_audit(
        tx,
        TaskChangeRecord {
            workspace_id: workspace,
            actor_user_id: actor,
            verb: "task.updated",
            target_type: "task",
            target_id: task,
            payload: json!({"taskId":task,"estimate":output.value,"estimateUnit":output.unit}),
            client_ip: None,
        },
    )
    .await?;
    Ok(output)
}

// Keep the authorized actor/session/locator and ordinary normalized task/hash
// explicit at this thin adapter to the existing document-task writer.
#[allow(clippy::too_many_arguments)]
pub async fn create_plan_task(
    pool: &PgPool,
    workspace: Uuid,
    document: Uuid,
    actor: Uuid,
    session: Uuid,
    body: &StudyPlanTaskBody,
    task: CreateTaskInput<'_>,
    request_hash: &str,
) -> DbResult<StudyPlanTaskOutput> {
    if body.expected_actor_id != actor {
        return Ok(Err(TimerDbError::Conflict("timer_context_changed")));
    }
    if body.minutes.is_some_and(|minutes| minutes < 0) {
        return Ok(Err(TimerDbError::InvalidInput));
    }
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace).await?;
    set_self_user(&mut tx, actor).await?;
    let outcome = match create_document_task_tx(
        &mut tx,
        workspace,
        actor,
        session,
        DocumentTaskRequest {
            document_id: document,
            project_id: body.project_id,
            request_id: body.request_id,
            self_assign: body.self_assign,
            anchor: body.anchor.as_deref(),
            request_hash,
            task,
        },
        None,
        "web",
    )
    .await?
    {
        Ok(outcome) => outcome,
        Err(err) => return Ok(Err(TimerDbError::Origin(err))),
    };
    if let DocumentTaskOutcome::Created(task_id) = &outcome {
        if body.expected_session_id != session {
            // Existing origin creator owns the locks/auth/dedupe. A new stale
            // captured command must roll back task, numbering and origin too.
            tx.rollback().await?;
            return Ok(Err(TimerDbError::Conflict("timer_context_changed")));
        }
        if let Some(minutes) = body.minutes {
            let before = estimate_in(&mut tx, workspace, *task_id).await?;
            persist_estimate(
                &mut tx,
                workspace,
                *task_id,
                actor,
                body.request_id,
                before,
                Some(minutes),
                "plan_explicit_minutes",
            )
            .await?;
        }
    }
    let task_id = outcome.task_id();
    // The existing origin transaction checks current task/source/target ACL
    // before replay. Return its current ordinary locator within that fence;
    // the browser does not need an uncaptured metadata read after success.
    let (number, project_key): (i32, String) = sqlx::query_as("SELECT t.number,p.key FROM fvoci.tasks t JOIN fvoci.projects p ON p.workspace_id=t.workspace_id AND p.id=t.project_id WHERE t.workspace_id=$1 AND t.id=$2 AND t.deleted_at IS NULL")
        .bind(workspace).bind(task_id).fetch_one(&mut *tx).await?;
    let output = StudyPlanTaskOutput {
        task_id,
        number,
        project_key,
    };
    tx.commit().await?;
    Ok(Ok(output))
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
    let can_control = match crate::db::task_ops::time_entry_capability_in(
        &mut tx, workspace, task, actor, session,
    )
    .await?
    {
        Ok(value) => value,
        Err(err) => return Ok(Err(TimerDbError::Project(err))),
    };
    let state = state_in(&mut tx, workspace, task, actor, can_control).await?;
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
        legacy_open_ids: sqlx::query_scalar("SELECT time_entry_id FROM fvoci.task_timer_legacy_open WHERE user_id=$1 ORDER BY time_entry_id")
            .bind(actor).fetch_all(&mut *tx).await?,
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
    let mut semantic =
        serde_json::to_value(body).map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
    // A receipt belongs to the actor, not the transport credential. A fresh
    // live session may replay a success, but cannot apply an old new command.
    if let Some(object) = semantic.as_object_mut() {
        object.remove("expectedSessionId");
    }
    let value = serde_json::to_vec(&(kind, workspace, task, semantic))
        .map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
    Ok(format!("{:x}", Sha256::digest(value)))
}

/// Receipts are read only after current credential and target permission are
/// checked. A deleted run retires its key rather than creating a replacement.
/// A receipt a native restore imported (052 `restored_from_archive`) is
/// history only: after the hash check it is retired before its stored result
/// is looked at, so it never replays as a live success.
async fn replay(
    tx: &mut Transaction<'_, Postgres>,
    actor: Uuid,
    request: Uuid,
    digest: &str,
) -> DbResult<Option<Value>> {
    let row: Option<(String, bool, Option<Uuid>, Value)> = sqlx::query_as("SELECT request_hash, restored_from_archive IS NOT NULL, run_id, result FROM fvoci.task_timer_commands WHERE user_id = $1 AND request_id = $2")
        .bind(actor).bind(request).fetch_optional(&mut **tx).await?;
    match row {
        Some((stored, _, _, _)) if stored != digest => {
            Ok(Err(TimerDbError::Conflict("request_mismatch")))
        }
        Some((_, true, _, _)) => Ok(Err(TimerDbError::Conflict("timer_retired"))),
        Some((_, _, None, value)) if value.get("runId").is_some() => {
            Ok(Err(TimerDbError::Conflict("timer_retired")))
        }
        Some((_, _, _, value)) => Ok(Ok(Some(value))),
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

// Keep the immutable audit row's identity, nullable locators and before/after
// values explicit, in the same order as its SQL columns and binds.
#[allow(clippy::too_many_arguments)]
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
) -> Result<(bool, Option<Uuid>), sqlx::Error> {
    let row: Option<(Uuid, DateTime<Utc>)> = sqlx::query_as("SELECT id, started_at FROM fvoci.task_timer_segments WHERE run_id = $1 AND ended_at IS NULL FOR UPDATE")
        .bind(run.id).fetch_optional(&mut **tx).await?;
    let Some((id, start)) = row else {
        return Ok((false, None));
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
    Ok((at < start, Some(id)))
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
    if body.expected_actor_id != actor {
        return Ok(Err(TimerDbError::Conflict("timer_context_changed")));
    }
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
    if body.expected_session_id != session {
        return Ok(Err(TimerDbError::Conflict("timer_context_changed")));
    }
    let active = unfinished(&mut tx, actor).await?;
    let at = clock(&mut tx).await?;
    let mut regression = false;
    let mut closed_segment = None;
    let mut closed_note = body.note.clone();
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
        closed_note = run.note.clone();
        if run.status == TimerStatus::Running {
            (regression, closed_segment) = close_segment(&mut tx, &run, actor, at, true).await?;
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
    let mut audit_value = value.clone();
    audit_value["recordId"] = json!(closed_segment);
    audit_value["kind"] = json!("segment");
    audit_value["note"] = json!(closed_note);
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
        audit_value,
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
    if body.expected_actor_id != actor {
        return Ok(Err(TimerDbError::Conflict("timer_context_changed")));
    }
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
    if body.expected_session_id != session {
        return Ok(Err(TimerDbError::Conflict("timer_context_changed")));
    }
    let Some(run) = unfinished(&mut tx, actor).await? else {
        return Ok(Err(TimerDbError::Conflict("timer_version")));
    };
    if run.id != body.run_id || run.version != body.expected_version {
        return Ok(Err(TimerDbError::Conflict("timer_version")));
    }
    let at = clock(&mut tx).await?;
    let (regression, closed_segment) = close_segment(&mut tx, &run, actor, at, false).await?;
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
        json!({"runId":run.id,"version":version,"status":"stopped","recordId":closed_segment,"kind":"segment","note":run.note}),
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

// Effective personal ranges overlay append-only corrections. Original server
// anchors, projections and closed manual rows stay durable and unmodified.
// Each correction is actor-owned and requires current target Edit permission.
const RECORDS_SQL: &str = r#"
WITH scoped_segments AS MATERIALIZED (
 SELECT s.* FROM fvoci.task_timer_segments s
 WHERE s.workspace_id=$1 AND s.task_id=$2 AND s.user_id=$3
), segment_notes AS MATERIALIZED (
 SELECT DISTINCT ON (a.after_value->>'recordId')
        a.after_value->>'recordId' AS record_id,a.after_value
 FROM fvoci.task_timer_audit a
 JOIN scoped_segments s ON a.after_value->>'recordId'=s.id::text
 WHERE a.user_id=$3 AND a.verb<>'time.correct'
 ORDER BY a.after_value->>'recordId',a.id DESC
), corrections AS MATERIALIZED (
 SELECT DISTINCT ON (a.after_value->>'kind',a.after_value->>'recordId')
        a.after_value->>'kind' AS kind,a.after_value->>'recordId' AS record_id,a.after_value
 FROM fvoci.task_timer_audit a
 WHERE a.user_id=$3 AND a.workspace_id=$1 AND a.task_id=$2 AND a.verb='time.correct'
 ORDER BY a.after_value->>'kind',a.after_value->>'recordId',
          (a.after_value->>'revision')::bigint DESC,a.id DESC
)
SELECT b.id,b.kind,
 COALESCE((c.after_value->>'startedAt')::timestamptz,b.started_at) AS started_at,
 COALESCE((c.after_value->>'endedAt')::timestamptz,b.ended_at) AS ended_at,
 b.run_id,CASE WHEN c.after_value IS NOT NULL THEN c.after_value->>'note' ELSE b.note END AS note,
 COALESCE((c.after_value->>'revision')::bigint,0) AS revision,b.reserved_legacy
FROM (
 SELECT s.id,'segment'::text AS kind,s.started_at,s.ended_at,s.run_id,
 CASE WHEN a.after_value IS NOT NULL THEN a.after_value->>'note' ELSE COALESCE(e.note,r.note) END AS note,
 false AS reserved_legacy
 FROM scoped_segments s
 JOIN fvoci.task_timer_runs r ON r.id=s.run_id
 LEFT JOIN fvoci.time_entries e ON e.workspace_id=s.workspace_id AND e.id=s.time_entry_id
 LEFT JOIN segment_notes a ON a.record_id=s.id::text
 UNION ALL
 SELECT e.id,'manual'::text,e.started_at,e.ended_at,NULL::uuid,e.note,
 EXISTS(SELECT 1 FROM fvoci.task_timer_legacy_open l WHERE l.time_entry_id=e.id AND l.user_id=$3)
 FROM fvoci.time_entries e
 WHERE e.workspace_id=$1 AND e.task_id=$2 AND e.user_id=$3
 AND NOT EXISTS(SELECT 1 FROM fvoci.task_timer_segments s WHERE s.time_entry_id=e.id)
) b
LEFT JOIN corrections c ON c.record_id=b.id::text AND c.kind=b.kind
"#;

/// Called only after the existing task-read fence, on that same read snapshot.
/// Decorates the actor's already-loaded rows; other authors retain raw history.
/// One bounded batch covers manual rows and canonical segment projections.
pub async fn apply_self_corrections(
    tx: &mut Transaction<'_, Postgres>,
    workspace: Uuid,
    task: Uuid,
    actor: Uuid,
    rows: &mut [crate::db::task_ops::TimeEntryRow],
) -> Result<(), sqlx::Error> {
    let ids: Vec<Uuid> = rows
        .iter()
        .filter(|row| row.workspace_id == workspace && row.task_id == task && row.user_id == actor)
        .map(|row| row.id)
        .collect();
    if ids.is_empty() {
        return Ok(());
    }
    set_self_user(tx, actor).await?;
    type Correction = (Uuid, DateTime<Utc>, DateTime<Utc>, Option<String>);
    let corrections: Vec<Correction> = sqlx::query_as(
        r#"
        SELECT e.id,(c.after_value->>'startedAt')::timestamptz,
               (c.after_value->>'endedAt')::timestamptz,c.after_value->>'note'
        FROM fvoci.time_entries e
        LEFT JOIN fvoci.task_timer_segments s
          ON s.time_entry_id=e.id AND s.workspace_id=$1 AND s.task_id=$2 AND s.user_id=$3
        JOIN LATERAL (
          SELECT after_value FROM fvoci.task_timer_audit
          WHERE user_id=$3 AND workspace_id=$1 AND task_id=$2 AND verb='time.correct'
            AND after_value->>'recordId'=COALESCE(s.id,e.id)::text
            AND after_value->>'kind'=CASE WHEN s.id IS NULL THEN 'manual' ELSE 'segment' END
          ORDER BY (after_value->>'revision')::bigint DESC,id DESC LIMIT 1
        ) c ON true
        WHERE e.workspace_id=$1 AND e.task_id=$2 AND e.user_id=$3 AND e.id=ANY($4)
        "#,
    )
    .bind(workspace)
    .bind(task)
    .bind(actor)
    .bind(&ids)
    .fetch_all(&mut **tx)
    .await?;
    let corrections: std::collections::HashMap<_, _> = corrections
        .into_iter()
        .map(|(id, start, end, note)| (id, (start, end, note)))
        .collect();
    for row in rows {
        if row.workspace_id != workspace || row.task_id != task || row.user_id != actor {
            continue;
        }
        if let Some((start, end, note)) = corrections.get(&row.id) {
            // Existing DTO uses whole seconds. Canonical/history totals retain
            // exact milliseconds, including corrections shorter than a second.
            let seconds = (end.signed_duration_since(*start).num_milliseconds()).div_euclid(1000);
            let seconds = i32::try_from(seconds)
                .map_err(|_| sqlx::Error::Protocol("corrected duration out of range".into()))?;
            row.started_at = *start;
            row.ended_at = Some(*end);
            row.duration_seconds = Some(seconds);
            row.note = note.clone();
        }
    }
    Ok(())
}

type RecordTuple = (
    Uuid,
    String,
    DateTime<Utc>,
    Option<DateTime<Utc>>,
    Option<Uuid>,
    Option<String>,
    i64,
    bool,
);
fn record(row: RecordTuple) -> Result<TimeRecord, sqlx::Error> {
    let kind = match row.1.as_str() {
        "manual" => TimeRecordKind::Manual,
        "segment" => TimeRecordKind::Segment,
        _ => return Err(sqlx::Error::Protocol("invalid time record kind".into())),
    };
    Ok(TimeRecord {
        id: row.0,
        kind,
        started_at: row.2,
        ended_at: row.3,
        run_id: row.4,
        note: row.5,
        revision: row.6,
        reserved_legacy: row.7,
    })
}
async fn time_zone(tx: &mut Transaction<'_, Postgres>, actor: Uuid) -> Result<String, sqlx::Error> {
    sqlx::query_scalar("SELECT CASE WHEN EXISTS(SELECT 1 FROM pg_timezone_names p WHERE p.name=u.timezone) THEN u.timezone ELSE 'Asia/Seoul' END FROM fvoci.users u WHERE u.id=$1")
  .bind(actor).fetch_one(&mut **tx).await
}
async fn read_allowed(
    tx: &mut Transaction<'_, Postgres>,
    workspace: Uuid,
    task: Uuid,
    actor: Uuid,
    session: Uuid,
) -> DbResult<()> {
    set_tenant(tx, workspace).await?;
    set_self_user(tx, actor).await?;
    if !session_is_live(tx, actor, session).await? {
        return Ok(Err(TimerDbError::Project(ProjectDbError::Forbidden)));
    }
    if !can_view(tx, workspace, task, actor).await? {
        return Ok(Err(TimerDbError::Project(ProjectDbError::NotFound)));
    }
    Ok(Ok(()))
}
fn range_valid(from: NaiveDate, to: NaiveDate) -> bool {
    to >= from && (to - from).num_days() < 32
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct HistoryCursor {
    version: u8,
    session: Uuid,
    fingerprint: String,
    actor: Uuid,
    workspace: Uuid,
    task: Uuid,
    from: NaiveDate,
    to: NaiveDate,
    zone: String,
    started_at: DateTime<Utc>,
    id: Uuid,
    kind: TimeRecordKind,
}

#[cfg(feature = "db-tests")]
fn history_record_page_sql() -> String {
    format!("WITH records AS ({RECORDS_SQL}) SELECT * FROM records WHERE started_at < (($5::date+1)::timestamp AT TIME ZONE $6) AND (ended_at > ($4::date::timestamp AT TIME ZONE $6) OR ended_at IS NULL OR (kind='segment' AND ended_at=started_at AND started_at >= ($4::date::timestamp AT TIME ZONE $6))) AND ($7::timestamptz IS NULL OR (started_at,id,kind)<($7,$8::uuid,$9::text)) ORDER BY started_at DESC,id DESC,kind DESC LIMIT 101")
}

// Fingerprint and page share the complete effective local-date snapshot.
// This costs O(n) for the eligible records; it is not a writer revision counter.
fn history_sql() -> String {
    format!(
        r#"WITH records AS MATERIALIZED (
{RECORDS_SQL}
), eligible AS MATERIALIZED (
 SELECT * FROM records
 WHERE started_at < (($5::date+1)::timestamp AT TIME ZONE $6)
   AND (ended_at > ($4::date::timestamp AT TIME ZONE $6)
        OR ended_at IS NULL
        OR (kind='segment' AND ended_at=started_at
            AND started_at >= ($4::date::timestamp AT TIME ZONE $6)))
), witness AS (
 SELECT encode(sha256(convert_to('task-history-v1:' || COALESCE(
   string_agg(encode(sha256(convert_to(jsonb_build_array(
       kind,id,EXTRACT(EPOCH FROM started_at),EXTRACT(EPOCH FROM ended_at),
       run_id,note,revision,reserved_legacy
   )::text,'UTF8')),'hex'),'' ORDER BY id,kind),''),'UTF8')),'hex') AS fingerprint
 FROM eligible
), page AS (
 SELECT * FROM eligible
 WHERE $7::timestamptz IS NULL OR (started_at,id,kind)<($7,$8::uuid,$9::text)
 ORDER BY started_at DESC,id DESC,kind DESC LIMIT 101
)
SELECT witness.fingerprint,page.id,page.kind,page.started_at,page.ended_at,
       page.run_id,page.note,page.revision,page.reserved_legacy
FROM witness LEFT JOIN page ON true
ORDER BY page.started_at DESC NULLS LAST,page.id DESC NULLS LAST,page.kind DESC NULLS LAST"#
    )
}

/// Exact adopted history statement for restricted-role plan measurements.
#[cfg(feature = "db-tests")]
pub fn history_snapshot_measurement_sql() -> String {
    history_sql()
}

/// Preserve the earlier page/four-count experiment for its existing fixtures.
/// Production uses the effective-record fingerprint, not these counts.
#[cfg(feature = "db-tests")]
pub fn history_measurement_sql(with_witness: bool) -> String {
    let current = history_record_page_sql();
    if !with_witness {
        return current;
    }
    format!(
        r#"WITH page AS MATERIALIZED ({current}), witness AS (
 SELECT ARRAY[
  (SELECT count(*) FROM fvoci.task_timer_audit WHERE user_id=$3 AND workspace_id=$1 AND task_id=$2),
  (SELECT count(*) FROM fvoci.time_entries WHERE user_id=$3 AND workspace_id=$1 AND task_id=$2),
  (SELECT count(*) FROM fvoci.task_timer_segments WHERE user_id=$3 AND workspace_id=$1 AND task_id=$2 AND ended_at IS NOT NULL),
  (SELECT count(*) FROM fvoci.task_timer_legacy_open WHERE user_id=$3 AND workspace_id=$1 AND task_id=$2)
 ]::bigint[] AS version)
 SELECT witness.version,COALESCE((SELECT jsonb_agg(to_jsonb(p) ORDER BY p.started_at DESC,p.id DESC,p.kind DESC) FROM page p),'[]'::jsonb) AS records FROM witness"#
    )
}

pub async fn history(
    pool: &PgPool,
    workspace: Uuid,
    task: Uuid,
    actor: Uuid,
    session: Uuid,
    query: &TimerHistoryQuery,
) -> DbResult<TimerHistory> {
    if !range_valid(query.from, query.to) {
        return Ok(Err(TimerDbError::InvalidInput));
    }
    let mut tx = begin_read(pool).await?;
    if let Err(e) = read_allowed(&mut tx, workspace, task, actor, session).await? {
        return Ok(Err(e));
    }
    let zone = time_zone(&mut tx, actor).await?;
    let cursor: Option<HistoryCursor> = match &query.cursor {
        None => None,
        Some(value) if value.len() > 4096 => return Ok(Err(TimerDbError::InvalidInput)),
        Some(value) => match URL_SAFE_NO_PAD
            .decode(value)
            .ok()
            .and_then(|b| serde_json::from_slice::<HistoryCursor>(&b).ok())
        {
            Some(c)
                if c.version == 1
                    && c.fingerprint.len() == 64
                    && c.fingerprint
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
                    && c.actor == actor
                    && c.workspace == workspace
                    && c.task == task
                    && c.from == query.from
                    && c.to == query.to
                    && c.zone == zone =>
            {
                if c.session != session {
                    return Ok(Err(TimerDbError::Conflict("timer_context_changed")));
                }
                Some(c)
            }
            _ => return Ok(Err(TimerDbError::InvalidInput)),
        },
    };
    let sql = history_sql();
    type HistoryRow = (
        String,
        Option<Uuid>,
        Option<String>,
        Option<DateTime<Utc>>,
        Option<DateTime<Utc>>,
        Option<Uuid>,
        Option<String>,
        Option<i64>,
        Option<bool>,
    );
    let rows: Vec<HistoryRow> = sqlx::query_as(&sql)
        .bind(workspace)
        .bind(task)
        .bind(actor)
        .bind(query.from)
        .bind(query.to)
        .bind(&zone)
        .bind(cursor.as_ref().map(|c| c.started_at))
        .bind(cursor.as_ref().map(|c| c.id))
        .bind(cursor.as_ref().map(|c| match c.kind {
            TimeRecordKind::Manual => "manual",
            TimeRecordKind::Segment => "segment",
        }))
        .fetch_all(&mut *tx)
        .await?;
    let fingerprint = rows
        .first()
        .map(|r| r.0.clone())
        .ok_or_else(|| sqlx::Error::Protocol("missing history snapshot header".into()))?;
    if cursor
        .as_ref()
        .is_some_and(|c| c.fingerprint != fingerprint)
    {
        return Ok(Err(TimerDbError::Conflict("timer_history_changed")));
    }
    let mut records = Vec::with_capacity(rows.len());
    for (_, id, kind, start, end, run, note, revision, reserved) in rows {
        // LEFT JOIN retains the witness even when the page has become empty.
        let Some(id) = id else {
            continue;
        };
        let missing = || sqlx::Error::Protocol("incomplete history page record".into());
        records.push((
            id,
            kind.ok_or_else(missing)?,
            start.ok_or_else(missing)?,
            end,
            run,
            note,
            revision.ok_or_else(missing)?,
            reserved.ok_or_else(missing)?,
        ));
    }
    let more = records.len() > 100;
    let items = records
        .into_iter()
        .take(100)
        .map(record)
        .collect::<Result<Vec<_>, _>>()?;
    let next_cursor = if more {
        let last = items.last().expect("101 rows have a last item");
        Some(
            URL_SAFE_NO_PAD.encode(
                serde_json::to_vec(&HistoryCursor {
                    version: 1,
                    session,
                    fingerprint,
                    actor,
                    workspace,
                    task,
                    from: query.from,
                    to: query.to,
                    zone,
                    started_at: last.started_at,
                    id: last.id,
                    kind: last.kind,
                })
                .map_err(|e| sqlx::Error::Protocol(e.to_string()))?,
            ),
        )
    } else {
        None
    };
    let result = TimerHistory {
        server_now: clock(&mut tx).await?,
        items,
        next_cursor,
    };
    tx.commit().await?;
    Ok(Ok(result))
}

pub async fn summary(
    pool: &PgPool,
    workspace: Uuid,
    task: Uuid,
    actor: Uuid,
    session: Uuid,
    query: &TimerSummaryQuery,
) -> DbResult<TimerSummary> {
    if !range_valid(query.from, query.to) {
        return Ok(Err(TimerDbError::InvalidInput));
    }
    let mut tx = begin_read(pool).await?;
    if let Err(e) = read_allowed(&mut tx, workspace, task, actor, session).await? {
        return Ok(Err(e));
    }
    let zone = time_zone(&mut tx, actor).await?;
    let at = clock(&mut tx).await?;
    let sql = format!(
        r#"WITH records AS ({RECORDS_SQL}), intervals AS (
 SELECT date_trunc('milliseconds',started_at) AS started_at,date_trunc('milliseconds',COALESCE(ended_at,GREATEST(started_at,$7::timestamptz))) AS ended_at
 FROM records WHERE ended_at IS NOT NULL OR kind='segment'
 ), days AS (SELECT d::date AS day,d::timestamp AT TIME ZONE $6 AS a,(d::date+1)::timestamp AT TIME ZONE $6 AS b FROM generate_series($4::date::timestamp,$5::date::timestamp,interval '1 day') d)
 SELECT day,COALESCE(sum(GREATEST(0,EXTRACT(EPOCH FROM(LEAST(i.ended_at,b)-GREATEST(i.started_at,a)))*1000)) FILTER (WHERE i.started_at IS NOT NULL),0)::bigint
 FROM days LEFT JOIN intervals i ON i.started_at<b AND i.ended_at>a GROUP BY day ORDER BY day"#
    );
    let rows: Vec<(NaiveDate, i64)> = sqlx::query_as(&sql)
        .bind(workspace)
        .bind(task)
        .bind(actor)
        .bind(query.from)
        .bind(query.to)
        .bind(&zone)
        .bind(at)
        .fetch_all(&mut *tx)
        .await?;
    let flags:(bool,bool)=sqlx::query_as(&format!("WITH records AS ({RECORDS_SQL}) SELECT EXISTS(SELECT 1 FROM fvoci.task_timer_runs WHERE workspace_id=$1 AND task_id=$2 AND user_id=$3 AND status<>'stopped'),EXISTS(SELECT 1 FROM records WHERE kind='manual' AND ended_at IS NULL)")).bind(workspace).bind(task).bind(actor).fetch_one(&mut *tx).await?;
    let days = rows
        .into_iter()
        .map(|(date, milliseconds)| TimerDayTotal { date, milliseconds })
        .collect::<Vec<_>>();
    let result = TimerSummary {
        server_now: at,
        time_zone: zone,
        total_milliseconds: days.iter().map(|d| d.milliseconds).sum(),
        days,
        unfinished: flags.0,
        unresolved_manual: flags.1,
    };
    tx.commit().await?;
    Ok(Ok(result))
}

fn note_reason_valid(note: &Option<String>, reason: &str) -> bool {
    note.as_ref()
        .is_none_or(|n| n.encode_utf16().count() <= 2000)
        && !reason.trim().is_empty()
        && reason.encode_utf16().count() <= 2000
}
fn kind_text(kind: TimeRecordKind) -> &'static str {
    match kind {
        TimeRecordKind::Manual => "manual",
        TimeRecordKind::Segment => "segment",
    }
}
fn record_audit(value: &TimeRecord) -> Value {
    json!({"recordId":value.id,"kind":value.kind,"startedAt":value.started_at,"endedAt":value.ended_at,"note":value.note,"revision":value.revision})
}
// Only new submitted ranges adopt millisecond precision. Expected CAS ranges
// and existing034 anchors are never normalized. Hash/replay uses the original
// typed request, so even aliases of one effective range remain different intents.
fn effective_range(
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
    let start = DateTime::from_timestamp_millis(start.timestamp_millis())?;
    let end = DateTime::from_timestamp_millis(end.timestamp_millis())?;
    (end > start).then_some((start, end))
}

fn submitted_range_audit(record: &TimeRecord, start: DateTime<Utc>, end: DateTime<Utc>) -> Value {
    let mut value = record_audit(record);
    value["submittedStartedAt"] = json!(start);
    value["submittedEndedAt"] = json!(end);
    value
}

async fn hint(
    tx: &mut Transaction<'_, Postgres>,
    workspace: Uuid,
    task: Uuid,
    actor: Uuid,
) -> Result<(), sqlx::Error> {
    record_task_event_and_audit(
        tx,
        TaskChangeRecord {
            workspace_id: workspace,
            actor_user_id: actor,
            verb: "task.timer.changed",
            target_type: "task",
            target_id: task,
            payload: json!({"taskId":task,"hint":"time"}),
            client_ip: None,
        },
    )
    .await
}
async fn write_allowed(
    tx: &mut Transaction<'_, Postgres>,
    workspace: Uuid,
    task: Uuid,
    actor: Uuid,
    session: Uuid,
) -> DbResult<()> {
    set_tenant(tx, workspace).await?;
    set_self_user(tx, actor).await?;
    Ok(
        require_task_write_access(tx, workspace, actor, session, task, false)
            .await?
            .map(|_| ())
            .map_err(TimerDbError::Project),
    )
}

pub async fn create_manual(
    pool: &PgPool,
    workspace: Uuid,
    task: Uuid,
    actor: Uuid,
    session: Uuid,
    body: &TimerManualBody,
) -> DbResult<TimerRecordOutput> {
    if body.expected_actor_id != actor {
        return Ok(Err(TimerDbError::Conflict("timer_context_changed")));
    }
    if !note_reason_valid(&body.note, &body.reason) {
        return Ok(Err(TimerDbError::InvalidInput));
    }
    let mut tx = pool.begin().await?;
    if let Err(e) = write_allowed(&mut tx, workspace, task, actor, session).await? {
        return Ok(Err(e));
    }
    let digest = hash("manual", Some(workspace), Some(task), body)?;
    match replay(&mut tx, actor, body.request_id, &digest).await? {
        Err(e) => return Ok(Err(e)),
        Ok(Some(value)) => {
            return Ok(Ok(
                serde_json::from_value(value).map_err(|e| sqlx::Error::Protocol(e.to_string()))?
            ))
        }
        Ok(None) => {}
    }
    if body.expected_session_id != session {
        return Ok(Err(TimerDbError::Conflict("timer_context_changed")));
    }
    let submitted = body;
    let Some((start, end)) = effective_range(submitted.started_at, submitted.ended_at) else {
        return Ok(Err(TimerDbError::InvalidInput));
    };
    let mut effective = submitted.clone();
    effective.started_at = start;
    effective.ended_at = end;
    let body = &effective;
    let Some(seconds) =
        crate::db::task_ops::time_entry_duration_seconds(body.started_at, body.ended_at)
    else {
        return Ok(Err(TimerDbError::InvalidInput));
    };
    let at = clock(&mut tx).await?;
    if body.ended_at > at {
        return Ok(Err(TimerDbError::InvalidInput));
    }
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO fvoci.time_entries(id,workspace_id,task_id,user_id,started_at,ended_at,duration_seconds,note) VALUES($1,$2,$3,$4,$5,$6,$7,$8)").bind(id).bind(workspace).bind(task).bind(actor).bind(body.started_at).bind(body.ended_at).bind(seconds).bind(&body.note).execute(&mut *tx).await?;
    let record = TimeRecord {
        id,
        kind: TimeRecordKind::Manual,
        started_at: body.started_at,
        ended_at: Some(body.ended_at),
        run_id: None,
        note: body.note.clone(),
        revision: 0,
        reserved_legacy: false,
    };
    audit(
        &mut tx,
        actor,
        body.request_id,
        Some(workspace),
        Some(task),
        Some(id),
        "time.manual",
        Value::Null,
        submitted_range_audit(&record, submitted.started_at, submitted.ended_at),
        body.reason.trim(),
    )
    .await?;
    let output = TimerRecordOutput {
        server_now: at,
        record,
    };
    let value = serde_json::to_value(&output).map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
    hint(&mut tx, workspace, task, actor).await?;
    receipt(&mut tx, actor, body.request_id, &digest, None, &value).await?;
    tx.commit().await?;
    Ok(Ok(output))
}

pub async fn correct(
    pool: &PgPool,
    workspace: Uuid,
    task: Uuid,
    id: Uuid,
    actor: Uuid,
    session: Uuid,
    body: &TimeCorrectionBody,
) -> DbResult<TimerRecordOutput> {
    if body.expected_actor_id != actor {
        return Ok(Err(TimerDbError::Conflict("timer_context_changed")));
    }
    if !note_reason_valid(&body.note, &body.reason)
        || body.expected_revision < 0
        || body.expected_revision == i64::MAX
    {
        return Ok(Err(TimerDbError::InvalidInput));
    }
    let mut tx = pool.begin().await?;
    if let Err(e) = write_allowed(&mut tx, workspace, task, actor, session).await? {
        return Ok(Err(e));
    }
    let mut semantic =
        serde_json::to_value(body).map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
    semantic["recordId"] = json!(id);
    let digest = hash("correct", Some(workspace), Some(task), &semantic)?;
    match replay(&mut tx, actor, body.request_id, &digest).await? {
        Err(e) => return Ok(Err(e)),
        Ok(Some(value)) => {
            return Ok(Ok(
                serde_json::from_value(value).map_err(|e| sqlx::Error::Protocol(e.to_string()))?
            ))
        }
        Ok(None) => {}
    }
    if body.expected_session_id != session {
        return Ok(Err(TimerDbError::Conflict("timer_context_changed")));
    }
    let submitted = body;
    let Some((start, end)) = effective_range(submitted.started_at, submitted.ended_at) else {
        return Ok(Err(TimerDbError::InvalidInput));
    };
    let mut effective = submitted.clone();
    effective.started_at = start;
    effective.ended_at = end;
    let body = &effective;
    if i32::try_from((end - start).num_milliseconds().div_euclid(1000)).is_err() {
        return Ok(Err(TimerDbError::InvalidInput));
    }
    // Same writer prefix serializes own commands, then a concrete record row is
    // locked. No update to another actor's rows or hidden task is possible.
    let exists:Option<Uuid>=match body.kind {
  TimeRecordKind::Manual=>sqlx::query_scalar("SELECT id FROM fvoci.time_entries WHERE workspace_id=$1 AND task_id=$2 AND user_id=$3 AND id=$4 AND NOT EXISTS(SELECT 1 FROM fvoci.task_timer_segments s WHERE s.time_entry_id=$4) FOR UPDATE").bind(workspace).bind(task).bind(actor).bind(id).fetch_optional(&mut *tx).await?,
  TimeRecordKind::Segment=>sqlx::query_scalar("SELECT id FROM fvoci.task_timer_segments WHERE workspace_id=$1 AND task_id=$2 AND user_id=$3 AND id=$4 FOR UPDATE").bind(workspace).bind(task).bind(actor).bind(id).fetch_optional(&mut *tx).await?,
 };
    if exists.is_none() {
        return Ok(Err(TimerDbError::Project(ProjectDbError::NotFound)));
    }
    let sql =
        format!("WITH records AS ({RECORDS_SQL}) SELECT * FROM records WHERE id=$4 AND kind=$5");
    let row: RecordTuple = sqlx::query_as(&sql)
        .bind(workspace)
        .bind(task)
        .bind(actor)
        .bind(id)
        .bind(kind_text(body.kind))
        .fetch_one(&mut *tx)
        .await?;
    let before = record(row)?;
    if before.started_at != body.expected_started_at
        || before.ended_at != body.expected_ended_at
        || before.note != body.expected_note
        || before.revision != body.expected_revision
    {
        return Ok(Err(TimerDbError::Conflict("time_record_version")));
    }
    // An active server interval must be paused/stopped before editing its range.
    if before.kind == TimeRecordKind::Segment && before.ended_at.is_none() {
        return Ok(Err(TimerDbError::Conflict("timer_interval_running")));
    }
    let at = clock(&mut tx).await?;
    if body.ended_at > at {
        return Ok(Err(TimerDbError::InvalidInput));
    }
    if before.kind == TimeRecordKind::Manual && before.ended_at.is_none() {
        // Closing a legacy row is an explicit user-supplied end, never clock-now or
        // an inferred duration. Keep its original anchor; a correction overlays
        // the requested effective range and stores the old open range in audit.
        let Some(seconds) =
            crate::db::task_ops::time_entry_duration_seconds(before.started_at, body.ended_at)
        else {
            return Ok(Err(TimerDbError::InvalidInput));
        };
        sqlx::query("UPDATE fvoci.time_entries SET ended_at=$5,duration_seconds=$6 WHERE workspace_id=$1 AND task_id=$2 AND user_id=$3 AND id=$4").bind(workspace).bind(task).bind(actor).bind(id).bind(body.ended_at).bind(seconds).execute(&mut *tx).await?;
    }
    let corrected = TimeRecord {
        id,
        kind: before.kind,
        started_at: body.started_at,
        ended_at: Some(body.ended_at),
        run_id: before.run_id,
        note: body.note.clone(),
        revision: before.revision + 1,
        reserved_legacy: false,
    };
    audit(
        &mut tx,
        actor,
        body.request_id,
        Some(workspace),
        Some(task),
        (before.kind == TimeRecordKind::Manual).then_some(id),
        "time.correct",
        record_audit(&before),
        submitted_range_audit(&corrected, submitted.started_at, submitted.ended_at),
        body.reason.trim(),
    )
    .await?;
    let output = TimerRecordOutput {
        server_now: at,
        record: corrected,
    };
    let value = serde_json::to_value(&output).map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
    hint(&mut tx, workspace, task, actor).await?;
    receipt(
        &mut tx,
        actor,
        body.request_id,
        &digest,
        before.run_id,
        &value,
    )
    .await?;
    tx.commit().await?;
    Ok(Ok(output))
}

pub async fn release_legacy(
    pool: &PgPool,
    actor: Uuid,
    session: Uuid,
    body: &LegacyReleaseBody,
) -> DbResult<LegacyReleaseOutput> {
    if body.expected_actor_id != actor {
        return Ok(Err(TimerDbError::Conflict("timer_context_changed")));
    }
    let mut tx = pool.begin().await?;
    set_self_user(&mut tx, actor).await?;
    lock_membership_users(&mut tx, &[actor]).await?;
    if !recheck_session(&mut tx, actor, session).await? {
        return Ok(Err(TimerDbError::Project(ProjectDbError::Forbidden)));
    }
    let digest = hash("legacy-release", None, None, body)?;
    match replay(&mut tx, actor, body.request_id, &digest).await? {
        Err(e) => return Ok(Err(e)),
        Ok(Some(value)) => {
            return Ok(Ok(
                serde_json::from_value(value).map_err(|e| sqlx::Error::Protocol(e.to_string()))?
            ))
        }
        Ok(None) => {}
    }
    if body.expected_session_id != session {
        return Ok(Err(TimerDbError::Conflict("timer_context_changed")));
    }
    let removed = sqlx::query(
        "DELETE FROM fvoci.task_timer_legacy_open WHERE user_id=$1 AND time_entry_id=$2",
    )
    .bind(actor)
    .bind(body.time_entry_id)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if removed != 1 {
        return Ok(Err(TimerDbError::Conflict("timer_legacy_changed")));
    }
    let result = LegacyReleaseOutput {
        server_now: clock(&mut tx).await?,
        time_entry_id: body.time_entry_id,
        released: true,
    };
    let value = serde_json::to_value(&result).map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
    audit(
        &mut tx,
        actor,
        body.request_id,
        None,
        None,
        Some(body.time_entry_id),
        "legacy.release",
        json!({"reserved":true}),
        value.clone(),
        "explicit_release_original_range_unresolved",
    )
    .await?;
    receipt(&mut tx, actor, body.request_id, &digest, None, &value).await?;
    tx.commit().await?;
    Ok(Ok(result))
}
