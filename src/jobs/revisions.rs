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
    pub scheduled_phase_complete: bool,
    pub gc_phase_complete: bool,
}

impl RevisionMaintenanceResume {
    pub fn sweep_complete(&self) -> bool {
        self.scheduled_phase_complete && self.gc_phase_complete
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

    if params.settings.snapshot_interval_hours > 0 && !resume.scheduled_phase_complete {
        if let Some(engine) = params.engine.as_ref() {
            let cutoff = snapshot_cutoff(Utc::now(), params.settings.snapshot_interval_hours);
            let sweep = run_scheduled_snapshots(pool, engine, cutoff, resume, cancel).await?;
            stats.snapshots_attempted = sweep.stats.snapshots_attempted;
            stats.snapshots_created = sweep.stats.snapshots_created;
            stats.snapshots_deduped = sweep.stats.snapshots_deduped;
            stats.snapshots_skipped = sweep.stats.snapshots_skipped;
            stats.snapshots_failed = sweep.stats.snapshots_failed;
            next_resume = sweep.resume;
        } else {
            next_resume.scheduled_phase_complete = true;
        }
    } else if params.settings.snapshot_interval_hours == 0 {
        next_resume.scheduled_phase_complete = true;
    }

    if !cancel.is_cancelled() && !next_resume.gc_phase_complete {
        let (deleted, gc_resume, gc_done) = run_automatic_revision_gc(
            pool,
            params.settings.keep,
            next_resume.gc_workspace_after,
            cancel,
        )
        .await?;
        stats.revisions_deleted = deleted;
        next_resume.gc_workspace_after = gc_resume;
        next_resume.gc_phase_complete = gc_done;
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
    scheduled_phase_complete: bool,
    gc_phase_complete: bool,
) -> RevisionMaintenanceResume {
    RevisionMaintenanceResume {
        workspace_id,
        target,
        gc_workspace_after,
        scheduled_phase_complete,
        gc_phase_complete,
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
    let gc_phase_complete = resume.gc_phase_complete;
    let mut workspace_after = resume.workspace_id;
    let mut target_cursor = resume.target;
    let mut attempts_remaining = SCHEDULED_REVISION_TARGET_BATCH;
    let mut examined_remaining = SCHEDULED_REVISION_EXAMINE_BATCH;
    let mut workspaces_remaining = WORKSPACE_SCAN_BATCH as usize;

    while examined_remaining > 0
        && attempts_remaining > 0
        && workspaces_remaining > 0
        && !cancel.is_cancelled()
    {
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
                resume: snapshot_resume(None, None, gc_hold, true, gc_phase_complete),
            });
        }
        let workspace_count = workspaces.len();
        for workspace_id in workspaces {
            if cancel.is_cancelled()
                || attempts_remaining == 0
                || examined_remaining == 0
                || workspaces_remaining == 0
            {
                break;
            }
            workspaces_remaining -= 1;
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
        if attempts_remaining == 0 || examined_remaining == 0 || workspaces_remaining == 0 {
            break;
        }
        if workspace_count < WORKSPACE_SCAN_BATCH as usize {
            let complete = !cancel.is_cancelled();
            return Ok(ScheduledSweep {
                stats,
                resume: snapshot_resume(
                    if complete { None } else { workspace_after },
                    if complete { None } else { target_cursor },
                    gc_hold,
                    complete,
                    gc_phase_complete,
                ),
            });
        }
    }

    let complete = false;
    Ok(ScheduledSweep {
        stats,
        resume: snapshot_resume(
            workspace_after,
            target_cursor,
            gc_hold,
            complete,
            gc_phase_complete,
        ),
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

/// Bounded automatic revision GC for one maintenance batch. Returns deleted row
/// count, resume cursor (`None` when the workspace scan finished), and whether the
/// GC phase completed for this hourly sweep.
pub async fn run_automatic_revision_gc(
    pool: &PgPool,
    keep: u32,
    resume_after: Option<Uuid>,
    cancel: &CancellationToken,
) -> Result<(u32, Option<Uuid>, bool), sqlx::Error> {
    let mut deleted = 0u32;
    let mut delete_rounds = REVISION_GC_ROUNDS;
    let mut workspace_after = resume_after;
    let mut resume_workspace = resume_after;

    while delete_rounds > 0 && !cancel.is_cancelled() {
        if let Some(workspace_id) = resume_workspace {
            let (n, exhausted) =
                gc_workspace_rounds(pool, workspace_id, keep, &mut delete_rounds, cancel).await?;
            deleted += n;
            workspace_after = Some(workspace_id);
            if exhausted {
                resume_workspace = None;
                continue;
            }
            if delete_rounds == 0 {
                return Ok((deleted, Some(workspace_id), false));
            }
            resume_workspace = None;
            continue;
        }

        let workspaces =
            list_live_workspace_ids_batch(pool, workspace_after, false, WORKSPACE_SCAN_BATCH)
                .await?;
        if workspaces.is_empty() {
            return Ok((deleted, None, true));
        }

        let workspace_count = workspaces.len();
        for workspace_id in workspaces {
            if cancel.is_cancelled() || delete_rounds == 0 {
                return Ok((deleted, workspace_after, false));
            }
            workspace_after = Some(workspace_id);
            let (n, exhausted) =
                gc_workspace_rounds(pool, workspace_id, keep, &mut delete_rounds, cancel).await?;
            deleted += n;
            if !exhausted {
                return Ok((deleted, Some(workspace_id), false));
            }
        }

        if workspace_count < WORKSPACE_SCAN_BATCH as usize {
            return Ok((deleted, None, true));
        }
    }

    if cancel.is_cancelled() {
        return Ok((deleted, workspace_after, false));
    }
    Ok((deleted, workspace_after, false))
}

/// Delete rounds for one workspace until it is caught up or the batch budget is spent.
/// Returns whether this workspace is fully compacted for the current `keep`.
async fn gc_workspace_rounds(
    pool: &PgPool,
    workspace_id: Uuid,
    keep: u32,
    delete_rounds: &mut u32,
    cancel: &CancellationToken,
) -> Result<(u32, bool), sqlx::Error> {
    let mut deleted = 0u32;
    while *delete_rounds > 0 && !cancel.is_cancelled() {
        let n = gc_automatic_revisions_batch(pool, workspace_id, keep, REVISION_GC_DELETE_BATCH)
            .await?;
        deleted += n;
        *delete_rounds -= 1;
        if n < REVISION_GC_DELETE_BATCH as u32 {
            return Ok((deleted, true));
        }
    }
    Ok((deleted, false))
}

/// Actual selected-family batch. The named consumer retains each mutation
/// writer across native work; borrowed producers never start another BEGIN.
pub(crate) async fn run_revision_maintenance_batch_family(
    backend: &crate::db::backend::Backend,
    params: &RevisionMaintenanceParams,
    resume: RevisionMaintenanceResume,
    proof: &super::FamilyMaintenanceProof,
    policy: super::FamilyMaintenanceLeasePolicy,
    cancel: &CancellationToken,
) -> Result<(RevisionMaintenanceStats, RevisionMaintenanceResume), super::MaintenanceConsumerError>
{
    let consumer = FamilyRevisionConsumer {
        backend,
        proof,
        policy,
        cancel,
    };
    let mut stats = RevisionMaintenanceStats::default();
    let mut next_resume = resume;
    if params.settings.snapshot_interval_hours > 0 && !resume.scheduled_phase_complete {
        if let Some(engine) = params.engine.as_ref() {
            let cutoff = snapshot_cutoff(
                super::family_maintenance_now(),
                params.settings.snapshot_interval_hours,
            );
            let sweep = consumer.scheduled(engine, cutoff, resume).await?;
            stats = sweep.stats;
            next_resume = sweep.resume;
        } else {
            next_resume.scheduled_phase_complete = true;
        }
    } else if params.settings.snapshot_interval_hours == 0 {
        next_resume.scheduled_phase_complete = true;
    }
    if !cancel.is_cancelled() && !next_resume.gc_phase_complete {
        let (deleted, after, complete) = consumer
            .gc(params.settings.keep, next_resume.gc_workspace_after)
            .await?;
        stats.revisions_deleted = deleted;
        next_resume.gc_workspace_after = after;
        next_resume.gc_phase_complete = complete;
    }
    Ok((stats, next_resume))
}

struct FamilyRevisionConsumer<'a> {
    backend: &'a crate::db::backend::Backend,
    proof: &'a super::FamilyMaintenanceProof,
    policy: super::FamilyMaintenanceLeasePolicy,
    cancel: &'a CancellationToken,
}

impl FamilyRevisionConsumer<'_> {
    async fn writer(
        &self,
        workspace: Option<Uuid>,
    ) -> Result<crate::db::backend::DbTx, super::MaintenanceConsumerError> {
        let mut tx = self.backend.begin_write().await?;
        let result = async {
            super::renew_maintenance_writer(
                &mut tx,
                self.proof,
                super::MaintenanceJobKey::Revisions,
                self.policy,
                self.cancel,
            )
            .await?;
            if let Some(workspace) = workspace {
                tx.operation().set_tenant(workspace).await?;
            }
            Ok::<_, super::MaintenanceConsumerError>(())
        }
        .await;
        match result {
            Ok(()) => Ok(tx),
            Err(error) => Err(super::rollback_maintenance_writer(tx, error).await),
        }
    }

    async fn finish(
        &self,
        tx: crate::db::backend::DbTx,
    ) -> Result<(), super::MaintenanceConsumerError> {
        super::commit_maintenance_writer(
            tx,
            self.proof,
            super::MaintenanceJobKey::Revisions,
            self.cancel,
        )
        .await
    }

    async fn workspaces(
        &self,
        after: Option<Uuid>,
        inclusive: bool,
    ) -> Result<Vec<Uuid>, super::MaintenanceConsumerError> {
        let mut tx = self.writer(None).await?;
        let result = async {
            let previous = tx.operation().set_system().await?;
            let ids = tx
                .operation()
                .list_revision_live_workspace_ids(after, inclusive, WORKSPACE_SCAN_BATCH)
                .await?;
            tx.operation().restore_system(previous).await?;
            Ok::<_, super::MaintenanceConsumerError>(ids)
        }
        .await;
        let ids = match result {
            Ok(ids) => ids,
            Err(error) => return Err(super::rollback_maintenance_writer(tx, error).await),
        };
        self.finish(tx).await?;
        Ok(ids)
    }

    async fn candidates(
        &self,
        workspace: Uuid,
        after: Option<ScheduledRevisionCursor>,
    ) -> Result<Vec<ScheduledRevisionCandidate>, super::MaintenanceConsumerError> {
        let mut tx = self.writer(Some(workspace)).await?;
        let result = tx
            .operation()
            .list_scheduled_revision_candidates(
                workspace,
                after,
                SCHEDULED_REVISION_TARGET_BATCH as i64,
            )
            .await;
        let rows = match result {
            Ok(rows) => rows,
            Err(error) => return Err(super::rollback_maintenance_writer(tx, error.into()).await),
        };
        self.finish(tx).await?;
        Ok(rows)
    }

    async fn scheduled(
        &self,
        engine: &RevisionMaintenanceEngine,
        cutoff: DateTime<Utc>,
        resume: RevisionMaintenanceResume,
    ) -> Result<ScheduledSweep, super::MaintenanceConsumerError> {
        let mut stats = RevisionMaintenanceStats::default();
        let gc_hold = resume.gc_workspace_after;
        let gc_phase_complete = resume.gc_phase_complete;
        let mut workspace_after = resume.workspace_id;
        let mut target_cursor = resume.target;
        let mut attempts_remaining = SCHEDULED_REVISION_TARGET_BATCH;
        let mut examined_remaining = SCHEDULED_REVISION_EXAMINE_BATCH;
        let mut workspaces_remaining = WORKSPACE_SCAN_BATCH as usize;
        while examined_remaining > 0
            && attempts_remaining > 0
            && workspaces_remaining > 0
            && !self.cancel.is_cancelled()
        {
            let workspaces = self
                .workspaces(workspace_after, target_cursor.is_some())
                .await?;
            if workspaces.is_empty() {
                return Ok(ScheduledSweep {
                    stats,
                    resume: snapshot_resume(None, None, gc_hold, true, gc_phase_complete),
                });
            }
            let workspace_count = workspaces.len();
            for workspace_id in workspaces {
                if self.cancel.is_cancelled()
                    || attempts_remaining == 0
                    || examined_remaining == 0
                    || workspaces_remaining == 0
                {
                    break;
                }
                workspaces_remaining -= 1;
                let mut cursor = if workspace_after == Some(workspace_id) {
                    target_cursor
                } else {
                    None
                };
                while examined_remaining > 0
                    && attempts_remaining > 0
                    && !self.cancel.is_cancelled()
                {
                    let candidates = self.candidates(workspace_id, cursor).await?;
                    if candidates.is_empty() {
                        workspace_after = Some(workspace_id);
                        target_cursor = None;
                        break;
                    }
                    let page_len = candidates.len();
                    let mut advanced = false;
                    for candidate in candidates {
                        if self.cancel.is_cancelled()
                            || attempts_remaining == 0
                            || examined_remaining == 0
                        {
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
                        match self.snapshot(engine, candidate).await {
                            Ok(ScheduledOutcome::Created) => stats.snapshots_created += 1,
                            Ok(ScheduledOutcome::Deduped) => stats.snapshots_deduped += 1,
                            Ok(ScheduledOutcome::Skipped) => stats.snapshots_skipped += 1,
                            Ok(ScheduledOutcome::Failed) => stats.snapshots_failed += 1,
                            Ok(ScheduledOutcome::Cancelled) => break,
                            Err(super::MaintenanceConsumerError::RevisionRefused(_)) => {
                                stats.snapshots_skipped += 1;
                            }
                            Err(error) if error.stops_on_backend(self.backend) => {
                                return Err(error)
                            }
                            Err(error) => {
                                stats.snapshots_failed += 1;
                                tracing::warn!(%error,"maintenance.scheduled_revision_target_failed");
                            }
                        }
                    }
                    if !advanced || page_len < SCHEDULED_REVISION_TARGET_BATCH {
                        break;
                    }
                }
            }
            if attempts_remaining == 0 || examined_remaining == 0 || workspaces_remaining == 0 {
                break;
            }
            if workspace_count < WORKSPACE_SCAN_BATCH as usize {
                let complete = !self.cancel.is_cancelled();
                return Ok(ScheduledSweep {
                    stats,
                    resume: snapshot_resume(
                        if complete { None } else { workspace_after },
                        if complete { None } else { target_cursor },
                        gc_hold,
                        complete,
                        gc_phase_complete,
                    ),
                });
            }
        }
        Ok(ScheduledSweep {
            stats,
            resume: snapshot_resume(
                workspace_after,
                target_cursor,
                gc_hold,
                false,
                gc_phase_complete,
            ),
        })
    }

    async fn snapshot(
        &self,
        engine: &RevisionMaintenanceEngine,
        candidate: ScheduledRevisionCandidate,
    ) -> Result<ScheduledOutcome, super::MaintenanceConsumerError> {
        let mut tx = self.writer(Some(candidate.workspace_id)).await?;
        let result = async {
            let source = match tx
                .operation()
                .load_scheduled_revision_source(
                    candidate.workspace_id,
                    candidate.target,
                    candidate.writer_generation,
                )
                .await?
            {
                Ok(source) => source,
                Err(_) => return Ok(ScheduledOutcome::Skipped),
            };
            let captured = tokio::task::spawn_blocking({
                let snapshot = source.durable.snapshot.clone();
                let tail = source.durable.tail.clone();
                let bin = engine.engine_bin.clone();
                let limits = engine.limits;
                move || capture_revision_offline(bin, limits, snapshot, tail)
            })
            .await;
            #[cfg(all(test, feature = "db-tests"))]
            family_revision_native_tests::after_native(
                self.proof,
                family_revision_native_tests::Phase::Capture,
            )
            .await;
            // Recheck even a helper failure, and never renew an expired owner.
            if self.cancel.is_cancelled() {
                return Err(super::MaintenanceConsumerError::Cancelled);
            }
            if !tx
                .operation()
                .check_family_maintenance_claim(self.proof, super::MaintenanceJobKey::Revisions)
                .await?
            {
                return Err(super::MaintenanceConsumerError::OwnershipLost);
            }
            if tx
                .operation()
                .check_scheduled_revision_source(&source)
                .await?
                .is_err()
            {
                return Ok(ScheduledOutcome::Skipped);
            }
            let captured = match captured {
                Ok(Ok(value)) => value,
                _ => return Ok(ScheduledOutcome::Failed),
            };
            let mut head_retries = 0u32;
            loop {
                if self.cancel.is_cancelled() {
                    return Err(super::MaintenanceConsumerError::Cancelled);
                }
                let latest = tx
                    .operation()
                    .latest_scheduled_revision_head(candidate.workspace_id, candidate.target)
                    .await?;
                let head_fence = SystemRevisionHead::from_latest(latest.clone());
                if let Some((_, previous)) = &latest {
                    let equal = tokio::task::spawn_blocking({
                        let left = previous.clone();
                        let right = captured.y_snapshot.clone();
                        let bin = engine.engine_bin.clone();
                        let limits = engine.limits;
                        move || revision_snapshots_equal_offline(bin, limits, &left, &right)
                    })
                    .await;
                    #[cfg(all(test, feature = "db-tests"))]
                    family_revision_native_tests::after_native(
                        self.proof,
                        family_revision_native_tests::Phase::Comparison,
                    )
                    .await;
                    if self.cancel.is_cancelled() {
                        return Err(super::MaintenanceConsumerError::Cancelled);
                    }
                    if !tx
                        .operation()
                        .check_family_maintenance_claim(
                            self.proof,
                            super::MaintenanceJobKey::Revisions,
                        )
                        .await?
                    {
                        return Err(super::MaintenanceConsumerError::OwnershipLost);
                    }
                    if tx
                        .operation()
                        .check_scheduled_revision_source(&source)
                        .await?
                        .is_err()
                    {
                        return Ok(ScheduledOutcome::Skipped);
                    }
                    match equal {
                        Ok(Ok(true)) => return Ok(ScheduledOutcome::Deduped),
                        Ok(Ok(false)) => {}
                        _ => return Ok(ScheduledOutcome::Failed),
                    }
                }
                let text = match prepare_revision_text(&captured.content_json) {
                    Ok(text) => text,
                    Err(_) => return Ok(ScheduledOutcome::Failed),
                };
                let input = CreateRevisionInput {
                    y_snapshot: captured.y_snapshot.clone(),
                    content_json: captured.content_json.clone(),
                    text,
                    reason: SCHEDULED_REASON.into(),
                };
                // Original expected generation/tail/cutoff/head come from the
                // opaque actual writer source; no cached actor proof substitute.
                super::renew_maintenance_writer(
                    &mut tx,
                    self.proof,
                    super::MaintenanceJobKey::Revisions,
                    self.policy,
                    self.cancel,
                )
                .await?;
                match tx
                    .operation()
                    .create_scheduled_revision(&source, &input, &head_fence)
                    .await?
                {
                    Ok(_) => {
                        if tx
                            .operation()
                            .check_scheduled_revision_source(&source)
                            .await?
                            .is_err()
                        {
                            return Err(super::MaintenanceConsumerError::OwnershipLost);
                        }
                        return Ok(ScheduledOutcome::Created);
                    }
                    Err(RevisionDbError::StaleRevisionHead)
                        if head_retries + 1 < SYSTEM_REVISION_HEAD_RETRIES =>
                    {
                        head_retries += 1;
                    }
                    Err(error) => {
                        // The borrowed producer may have inserted before its
                        // final source refusal. Roll back this whole unit;
                        // never COMMIT a partial publication as a skip.
                        if self.cancel.is_cancelled() {
                            return Err(super::MaintenanceConsumerError::Cancelled);
                        }
                        if !tx
                            .operation()
                            .check_family_maintenance_claim(
                                self.proof,
                                super::MaintenanceJobKey::Revisions,
                            )
                            .await?
                        {
                            return Err(super::MaintenanceConsumerError::OwnershipLost);
                        }
                        return Err(super::MaintenanceConsumerError::RevisionRefused(error));
                    }
                }
            }
        }
        .await;
        let outcome = match result {
            Ok(outcome) => outcome,
            Err(error) => return Err(super::rollback_maintenance_writer(tx, error).await),
        };
        self.finish(tx).await?;
        Ok(outcome)
    }

    async fn gc_round(
        &self,
        workspace: Uuid,
        keep: u32,
    ) -> Result<u32, super::MaintenanceConsumerError> {
        let mut tx = self.writer(Some(workspace)).await?;
        let result = tx
            .operation()
            .gc_revision_automatic_rows(workspace, keep, REVISION_GC_DELETE_BATCH)
            .await;
        let deleted = match result {
            Ok(deleted) => deleted,
            Err(error) => return Err(super::rollback_maintenance_writer(tx, error.into()).await),
        };
        self.finish(tx).await?;
        Ok(deleted)
    }

    async fn gc_workspace(
        &self,
        workspace: Uuid,
        keep: u32,
        rounds: &mut u32,
    ) -> Result<(u32, bool), super::MaintenanceConsumerError> {
        let mut deleted = 0;
        while *rounds > 0 && !self.cancel.is_cancelled() {
            let n = self.gc_round(workspace, keep).await?;
            deleted += n;
            *rounds -= 1;
            if n < REVISION_GC_DELETE_BATCH as u32 {
                return Ok((deleted, true));
            }
        }
        Ok((deleted, false))
    }

    async fn gc(
        &self,
        keep: u32,
        resume_after: Option<Uuid>,
    ) -> Result<(u32, Option<Uuid>, bool), super::MaintenanceConsumerError> {
        let mut deleted = 0;
        let mut rounds = REVISION_GC_ROUNDS;
        let mut workspace_after = resume_after;
        let mut resume_workspace = resume_after;
        while rounds > 0 && !self.cancel.is_cancelled() {
            if let Some(workspace) = resume_workspace {
                let (n, exhausted) = self.gc_workspace(workspace, keep, &mut rounds).await?;
                deleted += n;
                workspace_after = Some(workspace);
                if exhausted {
                    resume_workspace = None;
                    continue;
                }
                if rounds == 0 {
                    return Ok((deleted, Some(workspace), false));
                }
                resume_workspace = None;
                continue;
            }
            let workspaces = self.workspaces(workspace_after, false).await?;
            if workspaces.is_empty() {
                return Ok((deleted, None, true));
            }
            let workspace_count = workspaces.len();
            for workspace in workspaces {
                if self.cancel.is_cancelled() || rounds == 0 {
                    return Ok((deleted, workspace_after, false));
                }
                workspace_after = Some(workspace);
                let (n, exhausted) = self.gc_workspace(workspace, keep, &mut rounds).await?;
                deleted += n;
                if !exhausted {
                    return Ok((deleted, Some(workspace), false));
                }
            }
            if workspace_count < WORKSPACE_SCAN_BATCH as usize {
                return Ok((deleted, None, true));
            }
        }
        Ok((deleted, workspace_after, false))
    }
}

// The maintained native batch owns registration and an explicitly selected
// freshly built helper. Default fast library tests never launch this resource.
#[cfg(all(test, feature = "db-tests"))]
mod family_revision_native_tests {
    use super::*;
    use crate::db::backend::Backend;
    use crate::db::revisions::RevisionTarget;
    use crate::jobs::family_maintenance_fixture::{acquired, policy, Fixture};
    use crate::jobs::{FamilyMaintenanceProof, MaintenanceConsumerError, MaintenanceJobKey};
    use std::sync::{LazyLock, Mutex};
    use std::time::Duration;

    #[derive(Clone, Copy, PartialEq, Eq)]
    pub(super) enum Phase {
        Capture,
        Comparison,
    }
    type Barrier = (
        tokio::sync::oneshot::Sender<()>,
        tokio::sync::oneshot::Receiver<()>,
    );
    static BARRIERS: LazyLock<Mutex<Vec<(FamilyMaintenanceProof, Phase, Barrier)>>> =
        LazyLock::new(|| Mutex::new(Vec::new()));

    fn arm(
        proof: &FamilyMaintenanceProof,
        phase: Phase,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Sender<()>,
    ) {
        let (reached, rx) = tokio::sync::oneshot::channel();
        let (proceed, go) = tokio::sync::oneshot::channel();
        let mut barriers = BARRIERS.lock().unwrap();
        assert!(!barriers.iter().any(|(p, s, _)| p == proof && *s == phase));
        barriers.push((proof.clone(), phase, (reached, go)));
        (rx, proceed)
    }
    pub(super) async fn after_native(proof: &FamilyMaintenanceProof, phase: Phase) {
        let barrier = {
            let mut barriers = BARRIERS.lock().unwrap();
            barriers
                .iter()
                .position(|(p, s, _)| p == proof && *s == phase)
                .map(|position| barriers.remove(position).2)
        };
        if let Some((reached, go)) = barrier {
            reached.send(()).unwrap();
            go.await.unwrap();
        }
    }

    fn engine() -> RevisionMaintenanceEngine {
        let selected = std::env::var("FVOCI_COLLAB_ENGINE")
            .expect("explicit root-allocated FVOCI_COLLAB_ENGINE required; no fallback or skip");
        assert!(
            !selected.trim().is_empty(),
            "native prerequisite cannot be empty"
        );
        let engine_bin = PathBuf::from(selected.trim());
        let metadata = std::fs::metadata(&engine_bin).expect("allocated helper must exist");
        assert!(
            metadata.is_file() && metadata.len() > 0,
            "allocated helper must be a nonempty file"
        );
        RevisionMaintenanceEngine {
            engine_bin,
            limits: Limits::for_tests(),
        }
    }
    fn params() -> RevisionMaintenanceParams {
        RevisionMaintenanceParams {
            settings: RevisionSettings {
                session_snapshot_enabled: false,
                keep: 200,
                snapshot_interval_hours: 24,
            },
            engine: Some(engine()),
        }
    }
    fn fixture_bytes(name: &str) -> Vec<u8> {
        std::fs::read(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("crates/collab-engine/fixtures")
                .join(name),
        )
        .unwrap()
    }
    async fn seed_document(f: &Fixture) -> ScheduledRevisionCandidate {
        let document = f.document(f.workspace).await;
        let anchor = crate::jobs::family_maintenance_now() - chrono::Duration::hours(30);
        let updated = anchor + chrono::Duration::hours(1);
        sqlx::query("INSERT INTO document_states(workspace_id,document_id,state,created_at,updated_at) VALUES(?1,?2,?3,?4,?5)")
            .bind(f.workspace.as_bytes().as_slice()).bind(document.as_bytes().as_slice())
            .bind(fixture_bytes("structured.v1")).bind(anchor.timestamp_micros()).bind(updated.timestamp_micros())
            .execute(&f.pool).await.unwrap();
        ScheduledRevisionCandidate {
            workspace_id: f.workspace,
            target: RevisionTarget::Document(document),
            writer_generation: 0,
            state_updated_at: updated,
            anchor_at: anchor,
        }
    }
    async fn seed_task(f: &Fixture) -> Uuid {
        let project = Uuid::now_v7();
        let workflow = Uuid::now_v7();
        let status = Uuid::now_v7();
        let task = Uuid::now_v7();
        sqlx::query("INSERT INTO projects(id,workspace_id,key,name,visibility,created_by) VALUES(?1,?2,'MAINT','maintenance','workspace',?3)")
            .bind(project.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(f.user.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        sqlx::query("INSERT INTO workflows(id,workspace_id,project_id) VALUES(?1,?2,?3)")
            .bind(workflow.as_bytes().as_slice())
            .bind(f.workspace.as_bytes().as_slice())
            .bind(project.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO statuses(id,workspace_id,project_id,workflow_id,name,category,sort_key) VALUES(?1,?2,?3,?4,'todo','todo','V')")
            .bind(status.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(project.as_bytes().as_slice()).bind(workflow.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        sqlx::query("INSERT INTO tasks(id,workspace_id,project_id,number,title,status_id,content_json,created_by) VALUES(?1,?2,?3,1,'maintenance',?4,'{}',?5)")
            .bind(task.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(project.as_bytes().as_slice()).bind(status.as_bytes().as_slice()).bind(f.user.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        let anchor = crate::jobs::family_maintenance_now() - chrono::Duration::hours(30);
        sqlx::query("INSERT INTO task_states(workspace_id,task_id,state,created_at,updated_at) VALUES(?1,?2,?3,?4,?5)")
            .bind(f.workspace.as_bytes().as_slice()).bind(task.as_bytes().as_slice()).bind(fixture_bytes("structured.v1"))
            .bind(anchor.timestamp_micros()).bind((anchor+chrono::Duration::hours(1)).timestamp_micros()).execute(&f.pool).await.unwrap();
        task
    }
    async fn count(f: &Fixture) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM revisions")
            .fetch_one(&f.pool)
            .await
            .unwrap()
    }
    fn consumer<'a>(
        backend: &'a Backend,
        proof: &'a FamilyMaintenanceProof,
        cancel: &'a CancellationToken,
    ) -> FamilyRevisionConsumer<'a> {
        FamilyRevisionConsumer {
            backend,
            proof,
            policy: policy(),
            cancel,
        }
    }

    #[tokio::test]
    async fn selected_revision_native_capture_history_dedupe_tail_and_bounded_resume() {
        let f = Fixture::new().await;
        let params = params();
        let mut first = None;
        for _ in 0..20 {
            let candidate = seed_document(&f).await;
            first.get_or_insert(candidate);
        }
        let task = seed_task(&f).await;
        let request =
            crate::jobs::claim::FamilyMaintenanceClaimRequest::new(MaintenanceJobKey::Revisions);
        let owner = acquired(&request, &f.backend).await;
        let cancel = CancellationToken::new();
        let (stats, resume) = run_revision_maintenance_batch_family(
            &f.backend,
            &params,
            RevisionMaintenanceResume::default(),
            owner.proof(),
            policy(),
            &cancel,
        )
        .await
        .unwrap();
        assert_eq!(stats.snapshots_attempted, 16);
        assert_eq!(stats.snapshots_created, 16);
        assert!(!resume.scheduled_phase_complete);
        assert!(resume.target.is_some());
        assert_eq!(count(&f).await, 16);
        let (stats, resume) = run_revision_maintenance_batch_family(
            &f.backend,
            &params,
            resume,
            owner.proof(),
            policy(),
            &cancel,
        )
        .await
        .unwrap();
        assert_eq!(stats.snapshots_created, 5);
        assert!(resume.sweep_complete());
        assert_eq!(count(&f).await, 21);
        type ScheduledHistoryRow = (String, String, String, Option<Vec<u8>>, i64);
        let rows: Vec<ScheduledHistoryRow> = sqlx::query_as(
            "SELECT reason,content_json,text,created_by,encoding FROM revisions ORDER BY id",
        )
        .fetch_all(&f.pool)
        .await
        .unwrap();
        let expected: serde_json::Value = serde_json::from_str(include_str!(
            "../../crates/collab-engine/fixtures/expectations.json"
        ))
        .unwrap();
        for (reason, content, text, actor, encoding) in rows {
            assert_eq!(reason, "scheduled");
            assert_eq!(encoding, 1);
            assert!(actor.is_none());
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&content).unwrap(),
                expected["structured"]["prosemirror_json"]
            );
            for value in ["안녕 본문", "한글셀", "🚀"] {
                assert!(text.contains(value), "{text}");
            }
        }
        let task_rows: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM revisions WHERE target_kind='task' AND target_id=?1",
        )
        .bind(task.as_bytes().as_slice())
        .fetch_one(&f.pool)
        .await
        .unwrap();
        assert_eq!(task_rows, 1);
        let first = first.unwrap();
        let document = first.target.id();
        assert!(matches!(
            consumer(&f.backend, owner.proof(), &cancel)
                .snapshot(params.engine.as_ref().unwrap(), first)
                .await
                .unwrap(),
            ScheduledOutcome::Deduped
        ));
        assert_eq!(
            count(&f).await,
            21,
            "equal native capture cannot publish a duplicate history row"
        );
        sqlx::query("INSERT INTO document_collab_updates(workspace_id,document_id,seq,op_id,payload) VALUES(?1,?2,1,?3,?4)")
            .bind(f.workspace.as_bytes().as_slice()).bind(document.as_bytes().as_slice()).bind(Uuid::now_v7().as_bytes().as_slice()).bind(fixture_bytes("followup_edit.v1")).execute(&f.pool).await.unwrap();
        sqlx::query(
            "UPDATE document_states SET tail_seq=1 WHERE workspace_id=?1 AND document_id=?2",
        )
        .bind(f.workspace.as_bytes().as_slice())
        .bind(document.as_bytes().as_slice())
        .execute(&f.pool)
        .await
        .unwrap();
        let candidate = ScheduledRevisionCandidate {
            workspace_id: f.workspace,
            target: RevisionTarget::Document(document),
            writer_generation: 0,
            state_updated_at: crate::jobs::family_maintenance_now(),
            anchor_at: crate::jobs::family_maintenance_now() - chrono::Duration::hours(30),
        };
        assert!(matches!(
            consumer(&f.backend, owner.proof(), &cancel)
                .snapshot(params.engine.as_ref().unwrap(), candidate)
                .await
                .unwrap(),
            ScheduledOutcome::Created
        ));
        assert_eq!(
            count(&f).await,
            22,
            "different current durable tail must publish"
        );
        let text:String=sqlx::query_scalar("SELECT text FROM revisions WHERE target_id=?1 ORDER BY created_at DESC,id DESC LIMIT 1")
            .bind(document.as_bytes().as_slice()).fetch_one(&f.pool).await.unwrap();
        assert!(text.contains("후속편집한글✨"), "{text}");
        owner.release().await.unwrap();
        f.finish().await;
    }

    #[tokio::test]
    async fn selected_revision_native_late_cancel_capture_and_equal_noop_no_success() {
        let engine = engine();
        for phase in [Phase::Capture, Phase::Comparison] {
            let f = Fixture::new().await;
            let candidate = seed_document(&f).await;
            let request = crate::jobs::claim::FamilyMaintenanceClaimRequest::new(
                MaintenanceJobKey::Revisions,
            );
            let owner = acquired(&request, &f.backend).await;
            let cancel = CancellationToken::new();
            if phase == Phase::Comparison {
                assert!(matches!(
                    consumer(&f.backend, owner.proof(), &cancel)
                        .snapshot(&engine, candidate.clone())
                        .await
                        .unwrap(),
                    ScheduledOutcome::Created
                ));
            }
            let before = count(&f).await;
            let (reached, go) = arm(owner.proof(), phase);
            let backend = f.backend.clone();
            let proof = owner.proof().clone();
            let child = cancel.clone();
            let selected = engine.clone();
            let mut job = tokio::spawn(async move {
                consumer(&backend, &proof, &child)
                    .snapshot(&selected, candidate)
                    .await
                    .map(|outcome| matches!(outcome, ScheduledOutcome::Created))
            });
            crate::jobs::maintenance_test_hooks::wait_reached(reached, &mut job).await;
            cancel.cancel();
            go.send(()).unwrap();
            assert!(matches!(
                job.await.unwrap(),
                Err(MaintenanceConsumerError::Cancelled)
            ));
            assert_eq!(
                count(&f).await,
                before,
                "cancel after awaited native work cannot count/publish a success"
            );
            owner.release().await.unwrap();
            let healthy_request = crate::jobs::claim::FamilyMaintenanceClaimRequest::new(
                MaintenanceJobKey::Revisions,
            );
            let healthy = acquired(&healthy_request, &f.backend).await;
            assert_eq!(
                healthy.proof().generation(),
                2,
                "healthy acquisition never resets generation"
            );
            let fresh = CancellationToken::new();
            let candidate = {
                let mut writer = consumer(&f.backend, healthy.proof(), &fresh)
                    .candidates(f.workspace, None)
                    .await
                    .unwrap();
                writer.remove(0)
            };
            let outcome = consumer(&f.backend, healthy.proof(), &fresh)
                .snapshot(&engine, candidate)
                .await
                .unwrap();
            assert!(if phase == Phase::Capture {
                matches!(outcome, ScheduledOutcome::Created)
            } else {
                matches!(outcome, ScheduledOutcome::Deduped)
            });
            healthy.release().await.unwrap();
            f.finish().await;
        }
    }

    #[tokio::test]
    async fn selected_revision_wrong_expired_and_real_clock_expiry_after_native_no_effect() {
        let engine = engine();
        let f = Fixture::new().await;
        let candidate = seed_document(&f).await;
        let wrong_request =
            crate::jobs::claim::FamilyMaintenanceClaimRequest::new(MaintenanceJobKey::Daily);
        let wrong = acquired(&wrong_request, &f.backend).await;
        let cancel = CancellationToken::new();
        assert!(matches!(
            consumer(&f.backend, wrong.proof(), &cancel)
                .snapshot(&engine, candidate.clone())
                .await,
            Err(MaintenanceConsumerError::OwnershipLost)
        ));
        assert_eq!(count(&f).await, 0);
        wrong.release().await.unwrap();
        let request =
            crate::jobs::claim::FamilyMaintenanceClaimRequest::new(MaintenanceJobKey::Revisions);
        let short = crate::jobs::FamilyMaintenanceLeasePolicy::new(
            Duration::from_secs(2),
            Duration::from_secs(1),
        )
        .unwrap();
        let owner =
            match crate::jobs::GlobalJobClaim::try_claim(&f.backend, &request, short, &cancel)
                .await
                .unwrap()
            {
                crate::jobs::GlobalClaimAcquisition::Acquired(
                    crate::jobs::GlobalJobClaim::Family(owner),
                ) => owner,
                _ => panic!("actual short live claim required"),
            };
        let (reached, go) = arm(owner.proof(), Phase::Capture);
        let backend = f.backend.clone();
        let proof = owner.proof().clone();
        let selected = engine.clone();
        let target = candidate.clone();
        let mut job = tokio::spawn(async move {
            let cancel = CancellationToken::new();
            FamilyRevisionConsumer {
                backend: &backend,
                proof: &proof,
                policy: short,
                cancel: &cancel,
            }
            .snapshot(&selected, target)
            .await
            .map(|outcome| matches!(outcome, ScheduledOutcome::Created))
        });
        crate::jobs::maintenance_test_hooks::wait_reached(reached, &mut job).await;
        // Actual DB clock passes the short lease while this same writer waits;
        // no second writer fabricates an owner loss or provider acknowledgement.
        tokio::time::sleep(Duration::from_secs(3)).await;
        go.send(()).unwrap();
        assert!(matches!(
            job.await.unwrap(),
            Err(MaintenanceConsumerError::OwnershipLost)
        ));
        assert_eq!(count(&f).await, 0);
        assert!(
            matches!(
                consumer(&f.backend, owner.proof(), &cancel)
                    .snapshot(&engine, candidate.clone())
                    .await,
                Err(MaintenanceConsumerError::OwnershipLost)
            ),
            "expired proof cannot be revived by renewal"
        );
        assert_eq!(count(&f).await, 0);
        let recovered = acquired(&request, &f.backend).await;
        assert_eq!(recovered.proof().generation(), 2);
        assert!(
            matches!(
                consumer(&f.backend, owner.proof(), &cancel)
                    .snapshot(&engine, candidate.clone())
                    .await,
                Err(MaintenanceConsumerError::OwnershipLost)
            ),
            "old generation cannot borrow the new live owner's writer proof"
        );
        assert_eq!(count(&f).await, 0);
        assert!(matches!(
            consumer(&f.backend, recovered.proof(), &cancel)
                .snapshot(&engine, candidate)
                .await
                .unwrap(),
            ScheduledOutcome::Created
        ));
        assert!(matches!(
            owner.release().await.unwrap(),
            crate::jobs::FamilyLeaseAction::Lost
        ));
        recovered.release().await.unwrap();
        f.finish().await;
    }

    #[tokio::test]
    async fn selected_revision_post_insert_source_refusal_and_real_fk_commit_roll_back_history() {
        let engine = engine();
        let f = Fixture::new().await;
        let candidate = seed_document(&f).await;
        let request =
            crate::jobs::claim::FamilyMaintenanceClaimRequest::new(MaintenanceJobKey::Revisions);
        let owner = acquired(&request, &f.backend).await;
        let cancel = CancellationToken::new();
        let mut stale = candidate.clone();
        stale.writer_generation = 1;
        assert!(matches!(
            consumer(&f.backend, owner.proof(), &cancel)
                .snapshot(&engine, stale)
                .await
                .unwrap(),
            ScheduledOutcome::Skipped
        ));
        assert_eq!(count(&f).await, 0);
        sqlx::query("CREATE TRIGGER maintenance_revision_source_change AFTER INSERT ON revisions WHEN NEW.target_kind='document' BEGIN UPDATE document_states SET writer_generation=writer_generation+1 WHERE workspace_id=NEW.workspace_id AND document_id=NEW.target_id; END")
            .execute(&f.pool).await.unwrap();
        let error = consumer(&f.backend, owner.proof(), &cancel)
            .snapshot(&engine, candidate.clone())
            .await
            .err()
            .unwrap();
        assert!(matches!(
            error,
            MaintenanceConsumerError::RevisionRefused(RevisionDbError::NotFound)
        ));
        assert_eq!(
            count(&f).await,
            0,
            "post-insert refusal must roll back rather than commit as skipped"
        );
        let generation: i64 = sqlx::query_scalar(
            "SELECT writer_generation FROM document_states WHERE document_id=?1",
        )
        .bind(candidate.target.id().as_bytes().as_slice())
        .fetch_one(&f.pool)
        .await
        .unwrap();
        assert_eq!(generation, 0);
        sqlx::query("DROP TRIGGER maintenance_revision_source_change")
            .execute(&f.pool)
            .await
            .unwrap();
        crate::jobs::maintenance_test_hooks::arm_commit_fault(owner.proof());
        let error = consumer(&f.backend, owner.proof(), &cancel)
            .snapshot(&engine, candidate.clone())
            .await
            .err()
            .unwrap();
        let MaintenanceConsumerError::CommitUnknown(receipt) = &error else {
            panic!("real FK must fail at COMMIT: {error}")
        };
        assert!(
            matches!(&receipt.source.source,sqlx::Error::Database(source) if source.code().as_deref()==Some("787"))
        );
        assert_eq!(
            count(&f).await,
            0,
            "unknown commit cannot count history success"
        );
        let phantom: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM memberships WHERE user_id NOT IN (SELECT id FROM users)",
        )
        .fetch_one(&f.pool)
        .await
        .unwrap();
        assert_eq!(phantom, 0, "failed real FK transaction rolled back locally");
        assert!(matches!(
            consumer(&f.backend, owner.proof(), &cancel)
                .snapshot(&engine, candidate)
                .await
                .unwrap(),
            ScheduledOutcome::Created
        ));
        assert_eq!(count(&f).await, 1);
        crate::db::migrate::assert_sqlite_schema_current(&f.backend)
            .await
            .unwrap();
        owner.release().await.unwrap();
        f.finish().await;
    }

    #[tokio::test]
    async fn selected_revision_gc_real_commit_failure_manual_history_caps_and_healthy_progress() {
        let _engine = engine();
        let f = Fixture::new().await;
        let candidate = seed_document(&f).await;
        sqlx::query("WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<5003) INSERT INTO revisions(id,workspace_id,target_kind,target_id,y_snapshot,content_json,text,reason,created_at) SELECT randomblob(16),?1,'document',?2,?3,'{}','automatic',CASE WHEN i%2=0 THEN 'session' ELSE 'scheduled' END,i FROM n")
            .bind(f.workspace.as_bytes().as_slice()).bind(candidate.target.id().as_bytes().as_slice()).bind(fixture_bytes("revision_snapshot.bin")).execute(&f.pool).await.unwrap();
        let other_workspace = f.other_workspace().await;
        let other_document = f.document(other_workspace).await;
        let outside = Uuid::now_v7();
        sqlx::query("INSERT INTO revisions(id,workspace_id,target_kind,target_id,y_snapshot,content_json,text,reason,created_at) VALUES(?1,?2,'document',?3,?4,'{}','outside immutable','session',-1)")
            .bind(outside.as_bytes().as_slice()).bind(other_workspace.as_bytes().as_slice()).bind(other_document.as_bytes().as_slice())
            .bind(fixture_bytes("revision_snapshot.bin")).execute(&f.pool).await.unwrap();
        let manual = Uuid::now_v7();
        sqlx::query("INSERT INTO revisions(id,workspace_id,target_kind,target_id,y_snapshot,content_json,text,reason,created_by,created_at) VALUES(?1,?2,'document',?3,?4,'{}','manual','manual',?5,0)")
            .bind(manual.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(candidate.target.id().as_bytes().as_slice()).bind(fixture_bytes("revision_snapshot.bin")).bind(f.user.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        let request =
            crate::jobs::claim::FamilyMaintenanceClaimRequest::new(MaintenanceJobKey::Revisions);
        let owner = acquired(&request, &f.backend).await;
        let cancel = CancellationToken::new();
        crate::jobs::maintenance_test_hooks::arm_commit_fault(owner.proof());
        let error = consumer(&f.backend, owner.proof(), &cancel)
            .gc_round(f.workspace, 2)
            .await
            .err()
            .unwrap();
        assert!(matches!(error, MaintenanceConsumerError::CommitUnknown(_)));
        assert_eq!(count(&f).await, 5005);
        assert_eq!(
            consumer(&f.backend, owner.proof(), &cancel)
                .gc_round(f.workspace, 2)
                .await
                .unwrap(),
            5000
        );
        assert_eq!(count(&f).await, 5);
        assert_eq!(
            consumer(&f.backend, owner.proof(), &cancel)
                .gc_round(f.workspace, 2)
                .await
                .unwrap(),
            1
        );
        assert_eq!(count(&f).await, 4);
        let remaining: Vec<(String, i64)> = sqlx::query_as(
            "SELECT reason,created_at FROM revisions WHERE workspace_id=?1 ORDER BY created_at",
        )
        .bind(f.workspace.as_bytes().as_slice())
        .fetch_all(&f.pool)
        .await
        .unwrap();
        let outside_row: (String, i64) =
            sqlx::query_as("SELECT text,created_at FROM revisions WHERE workspace_id=?1 AND id=?2")
                .bind(other_workspace.as_bytes().as_slice())
                .bind(outside.as_bytes().as_slice())
                .fetch_one(&f.pool)
                .await
                .unwrap();
        assert_eq!(outside_row, ("outside immutable".into(), -1));
        assert_eq!(
            remaining,
            vec![
                ("manual".into(), 0),
                ("session".into(), 5002),
                ("scheduled".into(), 5003)
            ]
        );
        assert_eq!(
            consumer(&f.backend, owner.proof(), &cancel)
                .gc_round(f.workspace, 2)
                .await
                .unwrap(),
            0
        );
        owner.release().await.unwrap();
        f.finish().await;
    }
    #[tokio::test]
    async fn selected_revision_batch_real_target_commit_failure_returns_no_resume_then_healthy_retry(
    ) {
        let f = Fixture::new().await;
        let candidate = seed_document(&f).await;
        let params = params();
        let request =
            crate::jobs::claim::FamilyMaintenanceClaimRequest::new(MaintenanceJobKey::Revisions);
        let owner = acquired(&request, &f.backend).await;
        let (reached, go) = arm(owner.proof(), Phase::Capture);
        let backend = f.backend.clone();
        let proof = owner.proof().clone();
        let selected = params.clone();
        let mut job = tokio::spawn(async move {
            run_revision_maintenance_batch_family(
                &backend,
                &selected,
                RevisionMaintenanceResume::default(),
                &proof,
                policy(),
                &CancellationToken::new(),
            )
            .await
        });
        crate::jobs::maintenance_test_hooks::wait_reached(reached, &mut job).await;
        // Scanner/page finishes have already settled. Arm the genuine deferred
        // FK on the actual native target's final writer commit, not an oracle.
        crate::jobs::maintenance_test_hooks::arm_commit_fault(owner.proof());
        go.send(()).unwrap();
        let result = job.await.unwrap();
        assert!(matches!(&result,Err(MaintenanceConsumerError::CommitUnknown(_))),
            "failed target finish must not return successful counts or an advanced resume: {result:?}");
        assert_eq!(count(&f).await, 0);
        let generation: i64 = sqlx::query_scalar(
            "SELECT writer_generation FROM document_states WHERE document_id=?1",
        )
        .bind(candidate.target.id().as_bytes().as_slice())
        .fetch_one(&f.pool)
        .await
        .unwrap();
        assert_eq!(generation, 0);
        let (stats, resume) = run_revision_maintenance_batch_family(
            &f.backend,
            &params,
            RevisionMaintenanceResume::default(),
            owner.proof(),
            policy(),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(stats.snapshots_created, 1);
        assert_eq!(stats.snapshots_attempted, 1);
        assert!(resume.sweep_complete());
        assert_eq!(count(&f).await, 1);
        owner.release().await.unwrap();
        f.finish().await;
    }
}
