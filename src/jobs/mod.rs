//! In-process maintenance scheduler.
//!
//! One concept: named jobs, session advisory claim (`pg_try_advisory_lock`),
//! bounded batches, cancel-aware drain. There is no extra daemon. Source
//! BullMQ `daily-sweep` (`0 4 * * *`) is the same work under one cluster lock;
//! here each replica ticks and only the claimant runs.
//!
//! Source does not purge `events`, `audit_log`, or collab receipts. Those
//! tables stay append-only (app role cannot DELETE them).
//!
//! Abandoned-upload cleanup is a second job in the same loop with its own
//! cadence and claim key.

mod claim;
mod retention;
mod tokens;
mod uploads;
mod workspace;

use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Utc;
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::attachments::ObjectStorage;
use crate::mail::Mailer;

pub use claim::{
    JobClaim, JOB_KEY_DAILY, JOB_KEY_DIGEST, JOB_KEY_ICS, JOB_KEY_MAGIC, JOB_KEY_NOTIFICATIONS,
    JOB_KEY_PROCESSED, JOB_KEY_UPLOADS, JOB_KEY_WORKSPACE, JOB_LOCK_NAMESPACE,
};
pub use retention::{
    run_notification_gc, run_processed_gc, GC_DELETE_BATCH, GC_DELETE_ROUNDS,
    NOTIFICATION_ARCHIVED_RETENTION_DAYS, NOTIFICATION_READ_RETENTION_DAYS,
    PROCESSED_GC_WINDOW_DAYS,
};
pub use tokens::{run_ics_token_gc, run_magic_token_gc, TOKEN_GC_BATCH};
pub use uploads::{run_stale_upload_gc, StaleUploadGcStats, UPLOAD_GC_BATCH};
pub use workspace::{
    run_workspace_purge, WorkspacePurgeStats, WORKSPACE_PURGE_AFTER_DAYS, WORKSPACE_PURGE_BATCH,
};

const DEFAULT_TICK: Duration = Duration::from_secs(60);
const DEFAULT_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
const DEFAULT_UPLOAD_GC_INTERVAL: Duration = Duration::from_secs(10 * 60);
const DEFAULT_UPLOAD_INCOMPLETE_TTL: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Clone)]
pub struct MaintenanceSettings {
    pub tick: Duration,
    /// Cadence of the daily sweep.
    pub interval: Duration,
    /// Cadence of the abandoned-upload cleanup.
    pub upload_gc_interval: Duration,
    /// Incomplete uploads older than this are removed
    /// (`UPLOAD_INCOMPLETE_TTL_HOURS`, validated in `Config`).
    pub upload_incomplete_ttl: Duration,
}

impl Default for MaintenanceSettings {
    fn default() -> Self {
        Self {
            tick: DEFAULT_TICK,
            interval: DEFAULT_INTERVAL,
            upload_gc_interval: DEFAULT_UPLOAD_GC_INTERVAL,
            upload_incomplete_ttl: DEFAULT_UPLOAD_INCOMPLETE_TTL,
        }
    }
}

impl MaintenanceSettings {
    pub fn from_env(upload_incomplete_ttl: Duration) -> Self {
        Self {
            upload_gc_interval: Duration::from_secs(parse_positive_u64(
                "FVOCI_UPLOAD_GC_INTERVAL_SECS",
                std::env::var("FVOCI_UPLOAD_GC_INTERVAL_SECS")
                    .ok()
                    .as_deref(),
                DEFAULT_UPLOAD_GC_INTERVAL.as_secs(),
            )),
            upload_incomplete_ttl,
            tick: Duration::from_secs(parse_positive_u64(
                "FVOCI_MAINTENANCE_TICK_SECS",
                std::env::var("FVOCI_MAINTENANCE_TICK_SECS").ok().as_deref(),
                DEFAULT_TICK.as_secs(),
            )),
            interval: Duration::from_secs(parse_positive_u64(
                "FVOCI_MAINTENANCE_INTERVAL_SECS",
                std::env::var("FVOCI_MAINTENANCE_INTERVAL_SECS")
                    .ok()
                    .as_deref(),
                DEFAULT_INTERVAL.as_secs(),
            )),
        }
    }
}

fn parse_positive_u64(name: &str, raw: Option<&str>, default: u64) -> u64 {
    let Some(raw) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
        return default;
    };
    match raw.parse::<u64>() {
        Ok(value) if value > 0 => value,
        _ => {
            warn!(name, raw, "invalid maintenance duration; using default");
            default
        }
    }
}

pub struct MaintenanceHandle {
    cancel: CancellationToken,
    join: tokio::task::JoinHandle<()>,
}

impl MaintenanceHandle {
    pub fn request_shutdown(&self) {
        self.cancel.cancel();
    }

    pub async fn join(self) -> Result<(), String> {
        self.join
            .await
            .map_err(|err| format!("maintenance scheduler join failed: {err}"))?;
        Ok(())
    }
}

pub fn spawn_maintenance(
    settings: MaintenanceSettings,
    pool: PgPool,
    storage: ObjectStorage,
    mailer: Arc<Mailer>,
) -> MaintenanceHandle {
    let cancel = CancellationToken::new();
    let child = cancel.clone();
    let join = tokio::spawn(run_maintenance_loop(settings, pool, storage, mailer, child));
    MaintenanceHandle { cancel, join }
}

async fn run_maintenance_loop(
    settings: MaintenanceSettings,
    pool: PgPool,
    storage: ObjectStorage,
    mailer: Arc<Mailer>,
    cancel: CancellationToken,
) {
    let mut last_daily: Option<Instant> = None;
    let mut last_upload_gc: Option<Instant> = None;
    let mut ticker = tokio::time::interval(settings.tick);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = ticker.tick() => {
                if is_due(last_upload_gc, settings.upload_gc_interval) {
                    match run_stale_upload_sweep(
                        &pool,
                        &storage,
                        settings.upload_incomplete_ttl,
                        &cancel,
                    )
                    .await
                    {
                        Ok(Some(_)) => last_upload_gc = Some(Instant::now()),
                        Ok(None) => {}
                        Err(err) => warn!(error = %err, "maintenance.upload_gc_failed"),
                    }
                }
                if cancel.is_cancelled() {
                    return;
                }
                if is_due(last_daily, settings.interval) {
                    match run_daily_sweep(&pool, &storage, &mailer, &cancel).await {
                        Ok(Some(_)) => last_daily = Some(Instant::now()),
                        Ok(None) => {}
                        Err(err) => warn!(error = %err, "maintenance.daily_sweep_failed"),
                    }
                }
            }
        }
    }
}

fn is_due(last: Option<Instant>, interval: Duration) -> bool {
    last.is_none_or(|started| started.elapsed() >= interval)
}

/// Claim the upload cleanup lock and remove one bounded batch of incomplete
/// uploads older than `ttl`. `None` means another process holds the lock.
pub async fn run_stale_upload_sweep(
    pool: &PgPool,
    storage: &ObjectStorage,
    ttl: Duration,
    cancel: &CancellationToken,
) -> Result<Option<StaleUploadGcStats>, sqlx::Error> {
    let Some(claim) = JobClaim::try_claim(pool, JOB_KEY_UPLOADS).await? else {
        return Ok(None);
    };
    let ttl = chrono::Duration::from_std(ttl).unwrap_or(chrono::Duration::MAX);
    let cutoff = Utc::now()
        .checked_sub_signed(ttl)
        .unwrap_or(chrono::DateTime::<Utc>::MIN_UTC);
    let result = run_stale_upload_gc(pool, storage, cutoff, cancel).await;
    claim.release().await;
    let stats = result?;
    if stats.claimed > 0 {
        info!(
            claimed = stats.claimed,
            purged = stats.purged,
            failed = stats.failed,
            "maintenance.upload_gc"
        );
    }
    Ok(Some(stats))
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct DailySweepStats {
    pub workspace: WorkspacePurgeStats,
    pub ics_deleted: u32,
    pub magic_deleted: u32,
    pub notifications_read: u32,
    pub notifications_archived: u32,
    pub processed: u32,
    pub digests_sent: u32,
    pub imports_swept: u32,
}

/// Claim the daily sweep lock, run every job, then release. `None` means
/// another process holds the lock.
pub async fn run_daily_sweep(
    pool: &PgPool,
    storage: &ObjectStorage,
    mailer: &Mailer,
    cancel: &CancellationToken,
) -> Result<Option<DailySweepStats>, sqlx::Error> {
    let Some(claim) = JobClaim::try_claim(pool, JOB_KEY_DAILY).await? else {
        return Ok(None);
    };
    let result = run_daily_jobs(pool, storage, mailer, cancel).await;
    claim.release().await;
    result.map(Some)
}

async fn run_daily_jobs(
    pool: &PgPool,
    storage: &ObjectStorage,
    mailer: &Mailer,
    cancel: &CancellationToken,
) -> Result<DailySweepStats, sqlx::Error> {
    let now = Utc::now();
    let mut stats = DailySweepStats::default();

    if !cancel.is_cancelled() {
        match run_workspace_purge(pool, storage, now, cancel).await {
            Ok(workspace) => {
                info!(
                    purged = workspace.purged,
                    storage_deleted = workspace.storage_deleted,
                    skipped = workspace.skipped,
                    "maintenance.workspace_purge"
                );
                stats.workspace = workspace;
            }
            Err(err) => warn!(error = %err, "maintenance.workspace_purge_failed"),
        }
    }

    if !cancel.is_cancelled() {
        match run_ics_token_gc(pool, now, cancel).await {
            Ok(deleted) => {
                info!(deleted, "maintenance.ics_token_gc");
                stats.ics_deleted = deleted;
            }
            Err(err) => warn!(error = %err, "maintenance.ics_token_gc_failed"),
        }
    }

    if !cancel.is_cancelled() {
        match run_magic_token_gc(pool, now, cancel).await {
            Ok(deleted) => {
                info!(deleted, "maintenance.magic_token_gc");
                stats.magic_deleted = deleted;
            }
            Err(err) => warn!(error = %err, "maintenance.magic_token_gc_failed"),
        }
    }

    if !cancel.is_cancelled() {
        match run_notification_gc(pool, cancel).await {
            Ok((read, archived)) => {
                info!(read, archived, "maintenance.notification_gc");
                stats.notifications_read = read;
                stats.notifications_archived = archived;
            }
            Err(err) => warn!(error = %err, "maintenance.notification_gc_failed"),
        }
    }

    if !cancel.is_cancelled() {
        match run_processed_gc(pool, cancel).await {
            Ok(deleted) => {
                info!(deleted, "maintenance.processed_gc");
                stats.processed = deleted;
            }
            Err(err) => warn!(error = %err, "maintenance.processed_gc_failed"),
        }
    }

    // Source `sweepOrphanImports` runs in the same daily sweep, with storage
    // available because compensation may delete attachment objects.
    if !cancel.is_cancelled() {
        match crate::import_job::sweep_orphan_imports(pool, storage, cancel).await {
            Ok(swept) => {
                info!(swept, "maintenance.import_sweep");
                stats.imports_swept = swept;
            }
            Err(err) => warn!(error = %err, "maintenance.import_sweep_failed"),
        }
    }

    if !cancel.is_cancelled() {
        match crate::mail::send_due_digests(pool, mailer, now, cancel).await {
            Ok(sent) => {
                info!(sent, "maintenance.digest");
                stats.digests_sent = sent;
            }
            Err(err) => warn!(error = %err, "maintenance.digest_failed"),
        }
    }

    Ok(stats)
}
