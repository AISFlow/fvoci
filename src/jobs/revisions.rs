use chrono::{DateTime, Utc};
use collab_engine::limits::Limits;
use sqlx::PgPool;
use std::path::PathBuf;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::collab::revision::{
    capture_revision_offline, prepare_revision_text, revision_snapshots_equal_offline,
};
use crate::config::RevisionSettings;
use crate::db::revisions::{
    create_system_revision, gc_automatic_revisions_batch, latest_revision_y_snapshot,
    list_live_workspace_ids_batch, list_scheduled_revision_candidates_for_workspace,
    load_durable_collab_for_system, scheduled_revision_cursor, CreateRevisionInput,
    RevisionDbError, ScheduledRevisionCandidate, ScheduledRevisionCursor, SystemRevisionHead,
    SCHEDULED_REASON, SYSTEM_REVISION_HEAD_RETRIES,
};

use super::claim::{JobClaim, JOB_KEY_REVISIONS};

pub const SCHEDULED_REVISION_TARGET_BATCH: usize = 16;
/// Max collab targets examined (including predicate skips) per maintenance batch.
pub const SCHEDULED_REVISION_EXAMINE_BATCH: usize = 64;
pub const REVISION_GC_DELETE_BATCH: i32 = 5_000;
pub const REVISION_GC_ROUNDS: u32 = 30;
pub const WORKSPACE_SCAN_BATCH: i64 = 8;

#[derive(Debug, Clone)]
pub struct RevisionMaintenanceEngine {
    pub engine_bin: PathBuf,
    pub limits: Limits,
}

#[derive(Debug, Clone)]
pub struct RevisionMaintenanceParams {
    pub settings: RevisionSettings,
    pub engine: Option<RevisionMaintenanceEngine>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RevisionMaintenanceStats {
    pub snapshots_attempted: u32,
    pub snapshots_created: u32,
    pub snapshots_deduped: u32,
    pub snapshots_skipped: u32,
    pub snapshots_failed: u32,
    pub revisions_deleted: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RevisionMaintenanceResume {
    pub workspace_id: Option<Uuid>,
    pub target: Option<ScheduledRevisionCursor>,
    pub gc_workspace_after: Option<Uuid>,
}

impl RevisionMaintenanceResume {
    pub fn sweep_complete(&self) -> bool {
        self.workspace_id.is_none() && self.target.is_none() && self.gc_workspace_after.is_none()
    }
}

/// Claim the revision maintenance lock and run one bounded batch (scheduled
/// snapshots + automatic retention). `None` if another process holds the lock.
pub async fn run_revision_maintenance_sweep(
    pool: &PgPool,
    params: &RevisionMaintenanceParams,
    resume: RevisionMaintenanceResume,
    cancel: &CancellationToken,
) -> Result<Option<(RevisionMaintenanceStats, RevisionMaintenanceResume)>, sqlx::Error> {
    let Some(claim) = JobClaim::try_claim(pool, JOB_KEY_REVISIONS).await? else {
        return Ok(None);
    };
    let result = run_revision_maintenance_batch(pool, params, resume, cancel).await;
    claim.release().await;
    result.map(Some)
}

pub async fn run_revision_maintenance_batch(
    pool: &PgPool,
    params: &RevisionMaintenanceParams,
    resume: RevisionMaintenanceResume,
    cancel: &CancellationToken,
) -> Result<(RevisionMaintenanceStats, RevisionMaintenanceResume), sqlx::Error> {
    let mut stats = RevisionMaintenanceStats::default();
    let mut next_resume = resume;

    if params.settings.snapshot_interval_hours > 0 {
        if let Some(engine) = params.engine.as_ref() {
            let cutoff = snapshot_cutoff(Utc::now(), params.settings.snapshot_interval_hours);
            let sweep = run_scheduled_snapshots(pool, engine, cutoff, resume, cancel).await?;
            stats.snapshots_attempted = sweep.stats.snapshots_attempted;
            stats.snapshots_created = sweep.stats.snapshots_created;
            stats.snapshots_deduped = sweep.stats.snapshots_deduped;
            stats.snapshots_skipped = sweep.stats.snapshots_skipped;
            stats.snapshots_failed = sweep.stats.snapshots_failed;
            next_resume = sweep.resume;
        }
    }

    if !cancel.is_cancelled() {
        let (deleted, gc_resume) = run_automatic_revision_gc(
            pool,
            params.settings.keep,
            next_resume.gc_workspace_after,
            cancel,
        )
        .await?;
        stats.revisions_deleted = deleted;
        next_resume.gc_workspace_after = gc_resume;
    }

    Ok((stats, next_resume))
}

fn snapshot_cutoff(now: DateTime<Utc>, interval_hours: u32) -> DateTime<Utc> {
    let hours = interval_hours as i64;
    now.checked_sub_signed(chrono::Duration::hours(hours))
        .unwrap_or(DateTime::<Utc>::MIN_UTC)
}

struct ScheduledSweep {
    stats: RevisionMaintenanceStats,
    resume: RevisionMaintenanceResume,
}

fn snapshot_resume(
    workspace_id: Option<Uuid>,
    target: Option<ScheduledRevisionCursor>,
    gc_workspace_after: Option<Uuid>,
) -> RevisionMaintenanceResume {
    RevisionMaintenanceResume {
        workspace_id,
        target,
        gc_workspace_after,
    }
}

async fn run_scheduled_snapshots(
    pool: &PgPool,
    engine: &RevisionMaintenanceEngine,
    cutoff: DateTime<Utc>,
    resume: RevisionMaintenanceResume,
    cancel: &CancellationToken,
) -> Result<ScheduledSweep, sqlx::Error> {
    let mut stats = RevisionMaintenanceStats::default();
    let gc_hold = resume.gc_workspace_after;
    let mut workspace_after = resume.workspace_id;
    let mut target_cursor = resume.target;
    let mut attempts_remaining = SCHEDULED_REVISION_TARGET_BATCH;
    let mut examined_remaining = SCHEDULED_REVISION_EXAMINE_BATCH;

    while examined_remaining > 0 && attempts_remaining > 0 && !cancel.is_cancelled() {
        let inclusive_workspace = target_cursor.is_some();
        let workspaces = list_live_workspace_ids_batch(
            pool,
            workspace_after,
            inclusive_workspace,
            WORKSPACE_SCAN_BATCH,
        )
        .await?;
        if workspaces.is_empty() {
            return Ok(ScheduledSweep {
                stats,
                resume: snapshot_resume(None, None, gc_hold),
            });
        }
        let workspace_count = workspaces.len();
        for workspace_id in workspaces {
            if cancel.is_cancelled() || attempts_remaining == 0 || examined_remaining == 0 {
                break;
            }
            let mut cursor = if workspace_after == Some(workspace_id) {
                target_cursor
            } else {
                None
            };
            while examined_remaining > 0 && attempts_remaining > 0 && !cancel.is_cancelled() {
                let candidates = list_scheduled_revision_candidates_for_workspace(
                    pool,
                    workspace_id,
                    cursor,
                    SCHEDULED_REVISION_TARGET_BATCH as i64,
                )
                .await?;
                if candidates.is_empty() {
                    workspace_after = Some(workspace_id);
                    target_cursor = None;
                    break;
                }
                let page_len = candidates.len();
                let mut advanced = false;
                for candidate in candidates {
                    if cancel.is_cancelled() || attempts_remaining == 0 || examined_remaining == 0 {
                        break;
                    }
                    examined_remaining -= 1;
                    advanced = true;
                    workspace_after = Some(workspace_id);
                    let next = scheduled_revision_cursor(&candidate);
                    target_cursor = Some(next);
                    cursor = Some(next);
                    if candidate.anchor_at >= cutoff
                        || candidate.state_updated_at <= candidate.anchor_at
                    {
                        stats.snapshots_skipped += 1;
                        continue;
                    }
                    attempts_remaining -= 1;
                    stats.snapshots_attempted += 1;
                    match try_scheduled_snapshot(pool, engine, candidate, cancel).await {
                        ScheduledOutcome::Created => stats.snapshots_created += 1,
                        ScheduledOutcome::Deduped => stats.snapshots_deduped += 1,
                        ScheduledOutcome::Skipped => stats.snapshots_skipped += 1,
                        ScheduledOutcome::Failed => stats.snapshots_failed += 1,
                        ScheduledOutcome::Cancelled => break,
                    }
                }
                if !advanced {
                    break;
                }
                if page_len < SCHEDULED_REVISION_TARGET_BATCH {
                    break;
                }
            }
        }
        if attempts_remaining == 0 || examined_remaining == 0 {
            break;
        }
        if workspace_count < WORKSPACE_SCAN_BATCH as usize {
            return Ok(ScheduledSweep {
                stats,
                resume: snapshot_resume(None, None, gc_hold),
            });
        }
    }

    Ok(ScheduledSweep {
        stats,
        resume: snapshot_resume(workspace_after, target_cursor, gc_hold),
    })
}

enum ScheduledOutcome {
    Created,
    Deduped,
    Skipped,
    Failed,
    Cancelled,
}

async fn try_scheduled_snapshot(
    pool: &PgPool,
    engine: &RevisionMaintenanceEngine,
    candidate: ScheduledRevisionCandidate,
    cancel: &CancellationToken,
) -> ScheduledOutcome {
    let workspace_id = candidate.workspace_id;
    let target = candidate.target;
    let writer_generation = candidate.writer_generation;
    let engine_bin = engine.engine_bin.clone();
    let limits = engine.limits;

    let mut captured = None;
    let mut head_retries = 0u32;

    loop {
        if cancel.is_cancelled() {
            return ScheduledOutcome::Cancelled;
        }
        if captured.is_none() {
            let durable = match load_durable_collab_for_system(pool, workspace_id, target).await {
                Ok(Ok(durable)) => durable,
                Ok(Err(_)) => return ScheduledOutcome::Skipped,
                Err(_) => return ScheduledOutcome::Failed,
            };
            let snap = match tokio::task::spawn_blocking({
                let snapshot = durable.snapshot.clone();
                let tail = durable.tail.clone();
                let engine_bin = engine_bin.clone();
                move || capture_revision_offline(engine_bin, limits, snapshot, tail)
            })
            .await
            {
                Ok(Ok(captured)) => captured,
                _ => return ScheduledOutcome::Failed,
            };
            captured = Some(snap);
        }
        let captured = captured.as_ref().expect("captured");

        let latest = match latest_revision_y_snapshot(pool, workspace_id, target).await {
            Ok(row) => row,
            Err(_) => return ScheduledOutcome::Failed,
        };
        let head_fence = SystemRevisionHead::from_latest(latest.clone());

        if let Some((_, prev_snap)) = &latest {
            let equal = match tokio::task::spawn_blocking({
                let left = prev_snap.clone();
                let right = captured.y_snapshot.clone();
                let engine_bin = engine_bin.clone();
                move || revision_snapshots_equal_offline(engine_bin, limits, &left, &right)
            })
            .await
            {
                Ok(Ok(true)) => true,
                Ok(Ok(false)) => false,
                _ => return ScheduledOutcome::Failed,
            };
            if equal {
                return ScheduledOutcome::Deduped;
            }
        }

        let text = match prepare_revision_text(&captured.content_json) {
            Ok(text) => text,
            Err(_) => return ScheduledOutcome::Failed,
        };
        let input = CreateRevisionInput {
            y_snapshot: captured.y_snapshot.clone(),
            content_json: captured.content_json.clone(),
            text,
            reason: SCHEDULED_REASON.to_string(),
        };

        match create_system_revision(
            pool,
            workspace_id,
            target,
            input,
            writer_generation,
            head_fence,
        )
        .await
        {
            Ok(Ok(_)) => return ScheduledOutcome::Created,
            Ok(Err(RevisionDbError::StaleRevisionHead))
                if head_retries + 1 < SYSTEM_REVISION_HEAD_RETRIES =>
            {
                head_retries += 1;
                continue;
            }
            Ok(Err(_)) => return ScheduledOutcome::Skipped,
            Err(_) => return ScheduledOutcome::Failed,
        }
    }
}

pub async fn run_automatic_revision_gc(
    pool: &PgPool,
    keep: u32,
    resume_after: Option<Uuid>,
    cancel: &CancellationToken,
) -> Result<(u32, Option<Uuid>), sqlx::Error> {
    let mut deleted = 0u32;
    let mut workspace_after = resume_after;
    for _ in 0..REVISION_GC_ROUNDS {
        if cancel.is_cancelled() {
            break;
        }
        let workspaces =
            list_live_workspace_ids_batch(pool, workspace_after, false, WORKSPACE_SCAN_BATCH)
                .await?;
        if workspaces.is_empty() {
            return Ok((deleted, None));
        }
        let workspace_count = workspaces.len();
        for workspace_id in workspaces {
            if cancel.is_cancelled() {
                break;
            }
            workspace_after = Some(workspace_id);
            for _ in 0..REVISION_GC_ROUNDS {
                if cancel.is_cancelled() {
                    break;
                }
                let n = gc_automatic_revisions_batch(
                    pool,
                    workspace_id,
                    keep,
                    REVISION_GC_DELETE_BATCH,
                )
                .await?;
                deleted += n;
                if n < REVISION_GC_DELETE_BATCH as u32 {
                    break;
                }
            }
        }
        if workspace_count < WORKSPACE_SCAN_BATCH as usize {
            return Ok((deleted, None));
        }
    }
    Ok((deleted, workspace_after))
}
