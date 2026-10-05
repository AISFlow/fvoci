//! `fvoci.import_jobs` persistence (source `repos.importJobs`).
//!
//! Every write runs in a transaction with tenant context (or, for the
//! cross-tenant claim and orphan sweep only, the system context the table
//! policy admits) and reports whether exactly the intended row changed.
//! Worker writes are fenced by `status = 'running' AND lease_token = $token AND
//! lease_until > now()`: once the sweep (or another claim) takes the row, a
//! late worker's writes match nothing and it stops.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

use crate::db::context::{
    lock_membership_users, recheck_session, restore_system, set_system, set_tenant,
};
use crate::db::workspace::{membership_role, WorkspaceRole};

/// Source `IMPORT_LEASE_MS` (15 minutes).
pub const IMPORT_LEASE_SECS: i64 = 15 * 60;
/// Decoded upload cap (source import body policy is 64 MiB).
pub const IMPORT_HTTP_MAX_BYTES: usize = 64 * 1024 * 1024;
/// A claim may run a job at most twice: the first run and one recovery after
/// a crash (source: BullMQ `maxStalledCount` 1). Later expiries are swept.
pub const IMPORT_MAX_ATTEMPTS: i16 = 2;
/// A run released after a transient failure (no collab seed slot) is
/// claimable again this long after the release.
pub const IMPORT_RETRY_BACKOFF_SECS: i64 = 30;
/// Source `IMPORT_SWEEP_MAX`.
pub const IMPORT_SWEEP_MAX: usize = 100;
/// A request-driven (markdown-zip) row still `pending` this long after its
/// last update belongs to a request that was cancelled or a process that
/// died. A live run never refreshes `updated_at`, so this is set far above
/// any request instead of at [`IMPORT_LEASE_SECS`].
pub const SYNC_IMPORT_STALE_SECS: i64 = 24 * 60 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportSource {
    NativeArchive,
    MarkdownZip,
    OfficeFile,
    NotionZip,
}

impl ImportSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NativeArchive => "native-archive",
            Self::MarkdownZip => "markdown-zip",
            Self::OfficeFile => "office-file",
            Self::NotionZip => "notion-zip",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "native-archive" => Some(Self::NativeArchive),
            "markdown-zip" => Some(Self::MarkdownZip),
            "office-file" => Some(Self::OfficeFile),
            "notion-zip" => Some(Self::NotionZip),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportStatus {
    Pending,
    Running,
    Completed,
    Failed,
}

impl ImportStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "running" => Some(Self::Running),
            "completed" => Some(Self::Completed),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ImportJobRefs {
    pub document_ids: Vec<Uuid>,
    pub task_ids: Vec<Uuid>,
    pub stored_keys: Vec<String>,
}

impl ImportJobRefs {
    pub fn is_empty(&self) -> bool {
        self.document_ids.is_empty() && self.task_ids.is_empty() && self.stored_keys.is_empty()
    }
}

#[derive(Debug, Clone)]
pub struct ImportJobRow {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub source: ImportSource,
    pub status: ImportStatus,
    pub created_refs: ImportJobRefs,
}

#[derive(Debug)]
pub enum ImportDbError {
    NotFound,
    Forbidden,
}

/// Durable input of an async job.
pub struct NewAsyncImport<'a> {
    pub file_name: Option<&'a str>,
    pub project_id: Option<Uuid>,
    pub payload: &'a [u8],
}

/// A leased async job. `lease_token` fences every later write.
#[derive(Debug, Clone)]
pub struct ImportClaim {
    pub workspace_id: Uuid,
    pub job_id: Uuid,
    pub lease_token: Uuid,
    pub attempt: i16,
    pub created_by: Uuid,
    pub session_id: Uuid,
    pub source: ImportSource,
    pub file_name: Option<String>,
    pub project_id: Option<Uuid>,
    /// Rows a previous, dead run of this job created (source `startRun`).
    pub prior_refs: ImportJobRefs,
}

#[derive(Debug, Clone)]
pub struct ExpiredImport {
    pub workspace_id: Uuid,
    pub job_id: Uuid,
    pub created_by: Uuid,
    pub created_refs: ImportJobRefs,
}

fn parse_refs(value: Value) -> ImportJobRefs {
    serde_json::from_value(value).unwrap_or_default()
}

/// Live session plus workspace admin, under the membership lock.
pub async fn require_import_admin(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<Result<(), ImportDbError>, sqlx::Error> {
    lock_membership_users(tx, &[actor_user_id]).await?;
    if !recheck_session(tx, actor_user_id, session_id).await? {
        return Ok(Err(ImportDbError::Forbidden));
    }
    let role = membership_role(tx, workspace_id, actor_user_id).await?;
    if !role.is_some_and(|r| r.at_least(WorkspaceRole::Admin)) {
        return Ok(Err(ImportDbError::Forbidden));
    }
    Ok(Ok(()))
}

#[allow(clippy::too_many_arguments)]
async fn insert_job(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Option<Uuid>,
    source: ImportSource,
    status: ImportStatus,
    input: Option<&NewAsyncImport<'_>>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO fvoci.import_jobs (
            id, workspace_id, created_by, session_id, source, status,
            file_name, project_id, payload
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        "#,
    )
    .bind(id)
    .bind(workspace_id)
    .bind(actor_user_id)
    .bind(session_id)
    .bind(source.as_str())
    .bind(status.as_str())
    .bind(input.and_then(|i| i.file_name))
    .bind(input.and_then(|i| i.project_id))
    .bind(input.map(|i| i.payload))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Source `importMarkdownZip` job row: `pending`, driven by the request.
pub async fn create_sync_import_job(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<Result<ImportJobRow, ImportDbError>, sqlx::Error> {
    let id = Uuid::now_v7();
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if let Err(err) = require_import_admin(&mut tx, workspace_id, actor_user_id, session_id).await?
    {
        tx.rollback().await?;
        return Ok(Err(err));
    }
    insert_job(
        &mut tx,
        id,
        workspace_id,
        actor_user_id,
        None,
        ImportSource::MarkdownZip,
        ImportStatus::Pending,
        None,
    )
    .await?;
    tx.commit().await?;
    Ok(Ok(ImportJobRow {
        id,
        workspace_id,
        source: ImportSource::MarkdownZip,
        status: ImportStatus::Pending,
        created_refs: ImportJobRefs::default(),
    }))
}

/// Source `startAsyncImport` job row: `running` with no lease (queued) and the
/// upload stored on the row in the same transaction.
pub async fn create_async_import_job(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    source: ImportSource,
    input: NewAsyncImport<'_>,
) -> Result<Result<ImportJobRow, ImportDbError>, sqlx::Error> {
    let id = Uuid::now_v7();
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if let Err(err) = require_import_admin(&mut tx, workspace_id, actor_user_id, session_id).await?
    {
        tx.rollback().await?;
        return Ok(Err(err));
    }
    insert_job(
        &mut tx,
        id,
        workspace_id,
        actor_user_id,
        Some(session_id),
        source,
        ImportStatus::Running,
        Some(&input),
    )
    .await?;
    tx.commit().await?;
    Ok(Ok(ImportJobRow {
        id,
        workspace_id,
        source,
        status: ImportStatus::Running,
        created_refs: ImportJobRefs::default(),
    }))
}

/// Terminal transition of a request-driven (unleased) job. Source
/// `updateStatus`: only from `pending`/`running`. Returns whether the row moved.
pub async fn finish_sync_import_job(
    pool: &PgPool,
    workspace_id: Uuid,
    job_id: Uuid,
    status: ImportStatus,
) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let result = sqlx::query(
        r#"
        UPDATE fvoci.import_jobs
        SET status = $3, payload = NULL, updated_at = now()
        WHERE workspace_id = $1
          AND id = $2
          AND status IN ('pending', 'running')
          AND lease_token IS NULL
        "#,
    )
    .bind(workspace_id)
    .bind(job_id)
    .bind(status.as_str())
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(result.rows_affected() == 1)
}

pub async fn get_import_job(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    job_id: Uuid,
) -> Result<Result<ImportJobRow, ImportDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if let Err(err) = require_import_admin(&mut tx, workspace_id, actor_user_id, session_id).await?
    {
        tx.rollback().await?;
        return Ok(Err(err));
    }
    let row: Option<(String, String, Value)> = sqlx::query_as(
        r#"
        SELECT source, status, created_refs
        FROM fvoci.import_jobs
        WHERE workspace_id = $1 AND id = $2
        "#,
    )
    .bind(workspace_id)
    .bind(job_id)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    let Some((source, status, refs)) = row else {
        return Ok(Err(ImportDbError::NotFound));
    };
    let (Some(source), Some(status)) = (ImportSource::parse(&source), ImportStatus::parse(&status))
    else {
        return Ok(Err(ImportDbError::NotFound));
    };
    Ok(Ok(ImportJobRow {
        id: job_id,
        workspace_id,
        source,
        status,
        created_refs: parse_refs(refs),
    }))
}

/// Claims the oldest queued job, or one whose previous run died (expired
/// lease) or was released for retry (after [`IMPORT_RETRY_BACKOFF_SECS`])
/// and still has an attempt left. Cross-tenant, so it runs in the
/// system context; `SKIP LOCKED` lets replicas claim different jobs.
pub async fn claim_next_import_job(pool: &PgPool) -> Result<Option<ImportClaim>, sqlx::Error> {
    let lease_token = Uuid::now_v7();
    let mut tx = pool.begin().await?;
    let previous = set_system(&mut tx).await?;
    let row = sqlx::query(
        r#"
        WITH picked AS (
            SELECT workspace_id, id
            FROM fvoci.import_jobs
            WHERE status = 'running'
              AND source <> 'markdown-zip'
              AND payload IS NOT NULL
              AND session_id IS NOT NULL
              AND attempts < $1
              AND (lease_until IS NULL OR lease_until < now())
              AND (attempts = 0
                   OR lease_until IS NOT NULL
                   OR updated_at < now() - make_interval(secs => $4))
            ORDER BY created_at, id
            LIMIT 1
            FOR UPDATE SKIP LOCKED
        )
        UPDATE fvoci.import_jobs AS j
        SET lease_token = $2,
            lease_until = now() + make_interval(secs => $3),
            attempts = j.attempts + 1,
            updated_at = now()
        FROM picked
        WHERE j.workspace_id = picked.workspace_id AND j.id = picked.id
        RETURNING j.workspace_id, j.id, j.attempts, j.created_by, j.session_id, j.source,
                  j.file_name, j.project_id, j.created_refs
        "#,
    )
    .bind(IMPORT_MAX_ATTEMPTS)
    .bind(lease_token)
    .bind(IMPORT_LEASE_SECS as f64)
    .bind(IMPORT_RETRY_BACKOFF_SECS as f64)
    .fetch_optional(&mut *tx)
    .await?;
    restore_system(&mut tx, &previous).await?;
    tx.commit().await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let source: String = row.get("source");
    let Some(source) = ImportSource::parse(&source) else {
        return Err(sqlx::Error::Protocol(format!(
            "import_jobs.claim: unexpected source {source}"
        )));
    };
    Ok(Some(ImportClaim {
        workspace_id: row.get("workspace_id"),
        job_id: row.get("id"),
        lease_token,
        attempt: row.get("attempts"),
        created_by: row.get("created_by"),
        session_id: row.get("session_id"),
        source,
        file_name: row.get("file_name"),
        project_id: row.get("project_id"),
        prior_refs: parse_refs(row.get("created_refs")),
    }))
}

const OWNED_BY_RUNNER: &str = "status = 'running' AND lease_token = $3 AND lease_until > now()";

/// Upload bytes of a claimed job; `None` when the fence is lost.
pub async fn load_import_payload(
    pool: &PgPool,
    claim: &ImportClaim,
) -> Result<Option<Vec<u8>>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, claim.workspace_id).await?;
    let row: Option<(Option<Vec<u8>>,)> = sqlx::query_as(&format!(
        "SELECT payload FROM fvoci.import_jobs WHERE workspace_id = $1 AND id = $2 AND {OWNED_BY_RUNNER}"
    ))
    .bind(claim.workspace_id)
    .bind(claim.job_id)
    .bind(claim.lease_token)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(row.and_then(|(payload,)| payload))
}

/// Source `resetRefs`: after the prior run's rows were compensated.
pub async fn reset_import_refs(pool: &PgPool, claim: &ImportClaim) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, claim.workspace_id).await?;
    let result = sqlx::query(&format!(
        r#"
        UPDATE fvoci.import_jobs
        SET created_refs = '{{"documentIds":[],"taskIds":[],"storedKeys":[]}}'::jsonb,
            updated_at = now()
        WHERE workspace_id = $1 AND id = $2 AND {OWNED_BY_RUNNER}
        "#
    ))
    .bind(claim.workspace_id)
    .bind(claim.job_id)
    .bind(claim.lease_token)
    .execute(&mut *tx)
    .await?;
    if result.rows_affected() != 1 {
        tx.rollback().await?;
        return Ok(false);
    }
    discard_deferred_events(&mut tx, claim.workspace_id, claim.job_id).await?;
    tx.commit().await?;
    Ok(true)
}

/// Drops events a run parked (the rows they describe were or will be
/// compensated, so they must never reach the outbox).
async fn discard_deferred_events(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    job_id: Uuid,
) -> Result<u64, sqlx::Error> {
    let result = sqlx::query(
        "DELETE FROM fvoci.import_deferred_events WHERE workspace_id = $1 AND import_job_id = $2",
    )
    .bind(workspace_id)
    .bind(job_id)
    .execute(&mut **tx)
    .await?;
    Ok(result.rows_affected())
}

/// Source `commitImport`: parked events go to fvoci.events in their original
/// order, in the transaction that makes the job `completed`.
async fn publish_deferred_events(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    job_id: Uuid,
) -> Result<u64, sqlx::Error> {
    let published = sqlx::query(
        r#"
        INSERT INTO fvoci.events (
            id, workspace_id, actor_user_id, verb, target_type, target_id, payload,
            channel, created_at
        )
        SELECT id, workspace_id, actor_user_id, verb, target_type, target_id, payload,
               channel, created_at
        FROM fvoci.import_deferred_events
        WHERE workspace_id = $1 AND import_job_id = $2
        ORDER BY seq
        "#,
    )
    .bind(workspace_id)
    .bind(job_id)
    .execute(&mut **tx)
    .await?;
    discard_deferred_events(tx, workspace_id, job_id).await?;
    Ok(published.rows_affected())
}

/// Extends the lease without new refs (e.g. after a long parse).
pub async fn extend_import_lease(pool: &PgPool, claim: &ImportClaim) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, claim.workspace_id).await?;
    let result = sqlx::query(&format!(
        r#"
        UPDATE fvoci.import_jobs
        SET lease_until = now() + make_interval(secs => $4), updated_at = now()
        WHERE workspace_id = $1 AND id = $2 AND {OWNED_BY_RUNNER}
        "#
    ))
    .bind(claim.workspace_id)
    .bind(claim.job_id)
    .bind(claim.lease_token)
    .bind(IMPORT_LEASE_SECS as f64)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(result.rows_affected() == 1)
}

/// Which `created_refs` list a fenced write appends to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportRefKind {
    Document,
    Task,
    StoredKey,
}

impl ImportRefKind {
    fn key(self) -> &'static str {
        match self {
            Self::Document => "documentIds",
            Self::Task => "taskIds",
            Self::StoredKey => "storedKeys",
        }
    }
}

/// Source `saveProgress` for one created document, inside the transaction
/// that created it: the ref is durable exactly when the document is.
pub async fn append_import_document_ref(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    job_id: Uuid,
    lease_token: Uuid,
    document_id: Uuid,
) -> Result<bool, sqlx::Error> {
    append_import_ref(
        tx,
        workspace_id,
        job_id,
        lease_token,
        ImportRefKind::Document,
        &document_id.to_string(),
    )
    .await
}

/// Source `saveProgress` for any created row or object key, inside the
/// transaction that created it; also renews the lease. `false` = the fence is
/// lost and the caller must roll back.
pub async fn append_import_ref(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    job_id: Uuid,
    lease_token: Uuid,
    kind: ImportRefKind,
    value: &str,
) -> Result<bool, sqlx::Error> {
    let key = kind.key();
    let result = sqlx::query(&format!(
        r#"
        UPDATE fvoci.import_jobs
        SET created_refs = jsonb_set(
                created_refs,
                '{{{key}}}',
                (created_refs -> '{key}') || jsonb_build_array($4::text)
            ),
            lease_until = now() + make_interval(secs => $5),
            updated_at = now()
        WHERE workspace_id = $1 AND id = $2 AND {OWNED_BY_RUNNER}
        "#
    ))
    .bind(workspace_id)
    .bind(job_id)
    .bind(lease_token)
    .bind(value)
    .bind(IMPORT_LEASE_SECS as f64)
    .execute(&mut **tx)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// Fence check inside a caller's transaction (renews the lease). `false` =
/// the lease is no longer this run's.
pub async fn hold_import_fence(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    fence: crate::db::documents::ImportFence,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(&format!(
        r#"
        UPDATE fvoci.import_jobs
        SET lease_until = now() + make_interval(secs => $4), updated_at = now()
        WHERE workspace_id = $1 AND id = $2 AND {OWNED_BY_RUNNER}
        "#
    ))
    .bind(workspace_id)
    .bind(fence.job_id)
    .bind(fence.lease_token)
    .bind(IMPORT_LEASE_SECS as f64)
    .execute(&mut **tx)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// Terminal transition by the lease holder; clears payload and lease.
pub async fn finish_import_job(
    pool: &PgPool,
    claim: &ImportClaim,
    status: ImportStatus,
) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, claim.workspace_id).await?;
    let finished = finish_import_job_in_tx(&mut tx, claim, status).await?;
    if finished {
        tx.commit().await?;
    } else {
        tx.rollback().await?;
    }
    Ok(finished)
}

/// The native graph caller owns this transaction: graph, result, cleanup
/// handoff, events and terminal state must commit together.
pub(crate) async fn finish_import_job_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    claim: &ImportClaim,
    status: ImportStatus,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(&format!(
        r#"
        UPDATE fvoci.import_jobs
        SET status = $4, payload = NULL, lease_token = NULL, lease_until = NULL,
            updated_at = now()
        WHERE workspace_id = $1 AND id = $2 AND {OWNED_BY_RUNNER}
        "#
    ))
    .bind(claim.workspace_id)
    .bind(claim.job_id)
    .bind(claim.lease_token)
    .bind(status.as_str())
    .execute(&mut **tx)
    .await?;
    if result.rows_affected() != 1 {
        // Fence lost: the sweep owns the row and its parked events.
        return Ok(false);
    }
    if status == ImportStatus::Completed {
        publish_deferred_events(tx, claim.workspace_id, claim.job_id).await?;
    } else {
        discard_deferred_events(tx, claim.workspace_id, claim.job_id).await?;
    }
    Ok(true)
}

/// Gives a claimed run back to the queue after a transient failure. Only
/// successful compensation permits clearing refs and parked events; otherwise
/// the next claim must retry cleanup before creating anything. The lease is
/// dropped (fencing this runner out) and the row is
/// claimable again [`IMPORT_RETRY_BACKOFF_SECS`] after the release (from
/// `updated_at`). The spent attempt stays counted, so
/// [`IMPORT_MAX_ATTEMPTS`] still bounds the retries.
pub async fn release_import_job_for_retry(
    pool: &PgPool,
    claim: &ImportClaim,
    clear_refs: bool,
) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, claim.workspace_id).await?;
    let result = sqlx::query(&format!(
        r#"
        UPDATE fvoci.import_jobs
        SET created_refs = CASE WHEN $4 THEN '{{"documentIds":[],"taskIds":[],"storedKeys":[]}}'::jsonb
                                ELSE created_refs END,
            lease_token = NULL,
            lease_until = NULL,
            updated_at = now()
        WHERE workspace_id = $1 AND id = $2 AND {OWNED_BY_RUNNER}
        "#
    ))
    .bind(claim.workspace_id)
    .bind(claim.job_id)
    .bind(claim.lease_token)
    .bind(clear_refs)
    .execute(&mut *tx)
    .await?;
    if result.rows_affected() != 1 {
        tx.rollback().await?;
        return Ok(false);
    }
    if clear_refs {
        discard_deferred_events(&mut tx, claim.workspace_id, claim.job_id).await?;
    }
    tx.commit().await?;
    Ok(true)
}

/// Source `claimExpired`: atomically fails one running job whose lease
/// expired and returns what it created. A second sweep cannot take it again.
pub async fn claim_expired_import_job(pool: &PgPool) -> Result<Option<ExpiredImport>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let previous = set_system(&mut tx).await?;
    let row: Option<(Uuid, Uuid, Uuid, Value)> = sqlx::query_as(
        r#"
        UPDATE fvoci.import_jobs AS j
        SET status = 'failed', payload = NULL, lease_token = NULL, lease_until = NULL,
            updated_at = now()
        FROM (
            SELECT workspace_id, id
            FROM fvoci.import_jobs
            WHERE status = 'running' AND lease_until < now()
            ORDER BY lease_until
            LIMIT 1
            FOR UPDATE SKIP LOCKED
        ) AS expired
        WHERE j.workspace_id = expired.workspace_id AND j.id = expired.id
        RETURNING j.workspace_id, j.id, j.created_by, j.created_refs
        "#,
    )
    .fetch_optional(&mut *tx)
    .await?;
    if let Some((workspace_id, job_id, _, _)) = &row {
        discard_deferred_events(&mut tx, *workspace_id, *job_id).await?;
    }
    restore_system(&mut tx, &previous).await?;
    tx.commit().await?;
    Ok(
        row.map(|(workspace_id, job_id, created_by, refs)| ExpiredImport {
            workspace_id,
            job_id,
            created_by,
            created_refs: parse_refs(refs),
        }),
    )
}

/// Fails request-driven markdown-zip rows left `pending` for longer than
/// [`SYNC_IMPORT_STALE_SECS`] (the handler future was dropped on a client
/// disconnect or the shutdown deadline, or the process died before the
/// terminal update). Documents the run created stay: this path never
/// compensates. Cross-tenant, so it runs in the system context; at most
/// [`IMPORT_SWEEP_MAX`] rows per call. Returns how many rows it failed.
pub async fn fail_stale_sync_import_jobs(pool: &PgPool) -> Result<u64, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let previous = set_system(&mut tx).await?;
    let result = sqlx::query(
        r#"
        UPDATE fvoci.import_jobs AS j
        SET status = 'failed', payload = NULL, updated_at = now()
        FROM (
            SELECT workspace_id, id
            FROM fvoci.import_jobs
            WHERE source = 'markdown-zip'
              AND status = 'pending'
              AND lease_token IS NULL
              AND updated_at < now() - make_interval(secs => $1)
            ORDER BY updated_at, id
            LIMIT $2
            FOR UPDATE SKIP LOCKED
        ) AS stale
        WHERE j.workspace_id = stale.workspace_id AND j.id = stale.id
        "#,
    )
    .bind(SYNC_IMPORT_STALE_SECS as f64)
    .bind(IMPORT_SWEEP_MAX as i64)
    .execute(&mut *tx)
    .await?;
    restore_system(&mut tx, &previous).await?;
    tx.commit().await?;
    Ok(result.rows_affected())
}

/// Compensation of one imported document (source `compensateImport`):
/// removes attachment rows (returning their storage keys for the caller to
/// delete), hard-deletes the document and records `document.purged` so the
/// search index drops it. `Ok(None)` = already gone.
pub async fn purge_imported_document(
    pool: &PgPool,
    workspace_id: Uuid,
    document_id: Uuid,
) -> Result<Option<Vec<String>>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    crate::db::context::lock_tree(&mut tx, workspace_id).await?;
    let exists: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM fvoci.documents WHERE workspace_id = $1 AND id = $2 FOR UPDATE",
    )
    .bind(workspace_id)
    .bind(document_id)
    .fetch_optional(&mut *tx)
    .await?;
    if exists.is_none() {
        tx.rollback().await?;
        return Ok(None);
    }
    let keys: Vec<(String,)> = sqlx::query_as(
        "DELETE FROM fvoci.attachments WHERE workspace_id = $1 AND document_id = $2 RETURNING storage_key",
    )
    .bind(workspace_id)
    .bind(document_id)
    .fetch_all(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM fvoci.documents WHERE workspace_id = $1 AND id = $2")
        .bind(workspace_id)
        .bind(document_id)
        .execute(&mut *tx)
        .await?;
    crate::db::identity::append_event(
        &mut tx,
        crate::db::identity::EventAppend {
            id: Uuid::now_v7(),
            workspace_id: Some(workspace_id),
            actor_user_id: None,
            verb: "document.purged".to_string(),
            target_type: Some("document".to_string()),
            target_id: Some(document_id),
            payload: json!({ "documentId": document_id.to_string() }),
        },
    )
    .await?;
    tx.commit().await?;
    Ok(Some(keys.into_iter().map(|(key,)| key).collect()))
}

/// Compensation of one imported task (source `compensateImport` →
/// `detachChildrenForTaskRemoval` + `tasks.remove`): children another user
/// attached meanwhile are detached (a subtask becomes a task), attachment
/// rows are removed (their keys returned for the caller to delete) and the
/// task is hard-deleted; comments, dependencies, assignees, labels, stars and
/// activity cascade. `Ok(None)` = already gone.
pub async fn purge_imported_task(
    pool: &PgPool,
    workspace_id: Uuid,
    task_id: Uuid,
) -> Result<Option<Vec<String>>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let exists: Option<(Uuid,)> = sqlx::query_as(
        "SELECT project_id FROM fvoci.tasks WHERE workspace_id = $1 AND id = $2 FOR UPDATE",
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_optional(&mut *tx)
    .await?;
    if exists.is_none() {
        tx.rollback().await?;
        return Ok(None);
    }
    let detached: Vec<(Uuid, String)> = sqlx::query_as(
        r#"
        UPDATE fvoci.tasks
        SET parent_id = NULL,
            type = CASE WHEN type = 'subtask' THEN 'task' ELSE type END,
            updated_at = now()
        WHERE workspace_id = $1 AND parent_id = $2
        RETURNING id, type
        "#,
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_all(&mut *tx)
    .await?;
    for (child_id, child_type) in detached {
        crate::db::identity::append_event(
            &mut tx,
            crate::db::identity::EventAppend {
                id: Uuid::now_v7(),
                workspace_id: Some(workspace_id),
                actor_user_id: None,
                verb: "task.updated".to_string(),
                target_type: Some("task".to_string()),
                target_id: Some(child_id),
                payload: json!({
                    "taskId": child_id.to_string(),
                    "parentId": null,
                    "type": child_type,
                }),
            },
        )
        .await?;
    }
    let keys: Vec<(String,)> = sqlx::query_as(
        "DELETE FROM fvoci.attachments WHERE workspace_id = $1 AND task_id = $2 RETURNING storage_key",
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_all(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM fvoci.tasks WHERE workspace_id = $1 AND id = $2")
        .bind(workspace_id)
        .bind(task_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Some(keys.into_iter().map(|(key,)| key).collect()))
}

// Selected-backend import lifecycle. PG wrappers above keep their public API;
// SQLite-family writers own the current row and use the checked cell codec.
use crate::db::backend::{Backend, FamilyTx, OperationTx};
use crate::db::codec::{Cell, FamilyRow};

#[derive(Debug, thiserror::Error)]
#[error("import request refused: {0:?}")]
struct ImportRollbackRefusal(ImportDbError);

fn import_protocol(message: &str) -> sqlx::Error {
    sqlx::Error::Protocol(format!("import_jobs: {message}"))
}
fn checked_refs(value: Value) -> Result<ImportJobRefs, sqlx::Error> {
    serde_json::from_value(value).map_err(|e| sqlx::Error::Decode(Box::new(e)))
}
fn job_from_family(row: &FamilyRow) -> Result<ImportJobRow, sqlx::Error> {
    Ok(ImportJobRow {
        id: row.cell(0)?.id()?,
        workspace_id: row.cell(1)?.id()?,
        source: ImportSource::parse(&row.cell(2)?.string()?)
            .ok_or_else(|| import_protocol("invalid source"))?,
        status: ImportStatus::parse(&row.cell(3)?.string()?)
            .ok_or_else(|| import_protocol("invalid status"))?,
        created_refs: checked_refs(row.cell(4)?.value()?)?,
    })
}
async fn import_now(tx: &mut FamilyTx) -> Result<i64, sqlx::Error> {
    tx.require_writer()?;
    let rows = tx
        .query(
            "SELECT unixepoch()*1000000+CAST(substr(strftime('%f','now'),4,3) AS INTEGER)*1000",
            &[],
        )
        .await?;
    rows.first()
        .ok_or(sqlx::Error::RowNotFound)?
        .cell(0)?
        .integer()
}
fn claim_args(claim: &ImportClaim, now: i64) -> Vec<Cell> {
    vec![
        Cell::uuid(claim.workspace_id),
        Cell::uuid(claim.job_id),
        Cell::uuid(claim.lease_token),
        Cell::Integer(now),
    ]
}
fn unknown_commit(unknown: crate::db::backend::CommitUnknown) -> sqlx::Error {
    // No fresh writer/observer or compensation after uncertain remote commit.
    sqlx::Error::AnyDriverError(Box::new(unknown))
}

impl OperationTx<'_, '_> {
    pub(crate) async fn require_import_admin(
        &mut self,
        workspace: Uuid,
        actor: Uuid,
        credential: Uuid,
    ) -> Result<Result<(), ImportDbError>, sqlx::Error> {
        if let Self::Postgres(tx) = self {
            return require_import_admin(tx, workspace, actor, credential).await;
        }
        self.lock_membership_users(&[actor]).await?;
        if !self.recheck_session(actor, credential).await?
            || !self.workspace_is_live(workspace).await?
            || !self
                .membership_role(workspace, actor, true)
                .await?
                .is_some_and(|r| r.at_least(WorkspaceRole::Admin))
        {
            return Ok(Err(ImportDbError::Forbidden));
        }
        Ok(Ok(()))
    }

    /// The producer uses this inside the same writer as the created resource.
    pub(crate) async fn append_import_ref(
        &mut self,
        workspace: Uuid,
        fence: crate::db::documents::ImportFence,
        kind: ImportRefKind,
        value: &str,
    ) -> Result<bool, sqlx::Error> {
        match self {
            Self::Postgres(tx) => {
                append_import_ref(tx, workspace, fence.job_id, fence.lease_token, kind, value).await
            }
            Self::SqliteFamily(tx) => {
                tx.require_tenant(workspace)?;
                let now = import_now(tx).await?;
                let path = format!("$.{}[#]", kind.key());
                Ok(tx.execute("UPDATE import_jobs SET created_refs=json_insert(created_refs,?5,?6),lease_until=?7,updated_at=?4 WHERE workspace_id=?1 AND id=?2 AND status='running' AND lease_token=?3 AND lease_until>?4", &[
                    Cell::uuid(workspace),Cell::uuid(fence.job_id),Cell::uuid(fence.lease_token),Cell::Integer(now),Cell::text(path),Cell::text(value),Cell::Integer(now+IMPORT_LEASE_SECS*1_000_000)
                ]).await? == 1)
            }
        }
    }

    /// Full current leased identity in this same writer. This grants job
    /// ownership only: the producer separately requires current admin/session
    /// and current target authority before publication.
    pub(crate) async fn hold_import_claim(
        &mut self,
        claim: &ImportClaim,
    ) -> Result<bool, sqlx::Error> {
        let found=match self {
            Self::Postgres(tx)=>sqlx::query_scalar::<_,Uuid>(
                "SELECT id FROM fvoci.import_jobs WHERE workspace_id=$1 AND id=$2 AND status='running' AND lease_token=$3 AND lease_until>now() AND created_by=$4 AND session_id=$5 AND source=$6 AND project_id IS NOT DISTINCT FROM $7 AND attempts=$8 FOR UPDATE"
            ).bind(claim.workspace_id).bind(claim.job_id).bind(claim.lease_token).bind(claim.created_by).bind(claim.session_id).bind(claim.source.as_str()).bind(claim.project_id).bind(claim.attempt).fetch_optional(&mut ***tx).await?.is_some(),
            Self::SqliteFamily(family)=>{
                family.require_writer()?;family.require_tenant(claim.workspace_id)?;
                let now=import_now(family).await?;
                !family.query("SELECT id FROM import_jobs WHERE workspace_id=?1 AND id=?2 AND status='running' AND lease_token=?3 AND lease_until>?4 AND created_by=?5 AND session_id=?6 AND source=?7 AND project_id IS ?8 AND attempts=?9", &[
                    Cell::uuid(claim.workspace_id),Cell::uuid(claim.job_id),Cell::uuid(claim.lease_token),Cell::Integer(now),Cell::uuid(claim.created_by),Cell::uuid(claim.session_id),Cell::text(claim.source.as_str()),Cell::optional_uuid(claim.project_id),Cell::Integer(i64::from(claim.attempt))
                ]).await?.is_empty()
            }
        };
        if !found {
            return Ok(false);
        }
        self.hold_import_fence(
            claim.workspace_id,
            crate::db::documents::ImportFence {
                job_id: claim.job_id,
                lease_token: claim.lease_token,
            },
        )
        .await
    }

    /// Exact durable ref membership under the full current claim. No caller
    /// supplied key/id becomes authority by merely sharing a workspace.
    pub(crate) async fn import_claim_contains_ref(
        &mut self,
        claim: &ImportClaim,
        kind: ImportRefKind,
        value: &str,
    ) -> Result<bool, sqlx::Error> {
        if !self.hold_import_claim(claim).await? {
            return Ok(false);
        }
        let refs = match self {
            Self::Postgres(tx) => checked_refs(
                sqlx::query_scalar(
                    "SELECT created_refs FROM fvoci.import_jobs WHERE workspace_id=$1 AND id=$2",
                )
                .bind(claim.workspace_id)
                .bind(claim.job_id)
                .fetch_one(&mut ***tx)
                .await?,
            )?,
            Self::SqliteFamily(family) => {
                family.require_tenant(claim.workspace_id)?;
                let rows = family
                    .query(
                        "SELECT created_refs FROM import_jobs WHERE workspace_id=?1 AND id=?2",
                        &[Cell::uuid(claim.workspace_id), Cell::uuid(claim.job_id)],
                    )
                    .await?;
                checked_refs(
                    rows.first()
                        .ok_or(sqlx::Error::RowNotFound)?
                        .cell(0)?
                        .value()?,
                )?
            }
        };
        Ok(match kind {
            ImportRefKind::Document => Uuid::parse_str(value)
                .ok()
                .is_some_and(|id| refs.document_ids.contains(&id)),
            ImportRefKind::Task => Uuid::parse_str(value)
                .ok()
                .is_some_and(|id| refs.task_ids.contains(&id)),
            ImportRefKind::StoredKey => refs.stored_keys.iter().any(|key| key == value),
        })
    }

    /// Recheck the current job owner after conversion/I/O, in the publication writer.
    pub(crate) async fn hold_import_fence(
        &mut self,
        workspace: Uuid,
        fence: crate::db::documents::ImportFence,
    ) -> Result<bool, sqlx::Error> {
        match self {
            Self::Postgres(tx) => hold_import_fence(tx, workspace, fence).await,
            Self::SqliteFamily(tx) => {
                tx.require_tenant(workspace)?;
                let now = import_now(tx).await?;
                Ok(tx.execute("UPDATE import_jobs SET lease_until=?5,updated_at=?4 WHERE workspace_id=?1 AND id=?2 AND status='running' AND lease_token=?3 AND lease_until>?4", &[
                    Cell::uuid(workspace),Cell::uuid(fence.job_id),Cell::uuid(fence.lease_token),Cell::Integer(now),Cell::Integer(now+IMPORT_LEASE_SECS*1_000_000)
                ]).await? == 1)
            }
        }
    }
}

pub async fn create_sync_import_job_backend(
    backend: &Backend,
    workspace: Uuid,
    actor: Uuid,
    credential: Uuid,
) -> Result<Result<ImportJobRow, ImportDbError>, sqlx::Error> {
    create_import_job_backend(
        backend,
        workspace,
        actor,
        credential,
        ImportSource::MarkdownZip,
        None,
    )
    .await
}

pub async fn create_async_import_job_backend(
    backend: &Backend,
    workspace: Uuid,
    actor: Uuid,
    credential: Uuid,
    source: ImportSource,
    input: NewAsyncImport<'_>,
) -> Result<Result<ImportJobRow, ImportDbError>, sqlx::Error> {
    create_import_job_backend(backend, workspace, actor, credential, source, Some(input)).await
}

async fn create_import_job_backend(
    backend: &Backend,
    workspace: Uuid,
    actor: Uuid,
    credential: Uuid,
    source: ImportSource,
    input: Option<NewAsyncImport<'_>>,
) -> Result<Result<ImportJobRow, ImportDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return match input {
            Some(input) => {
                create_async_import_job(pool, workspace, actor, credential, source, input).await
            }
            None => create_sync_import_job(pool, workspace, actor, credential).await,
        };
    }
    let id = Uuid::now_v7();
    let mut tx = backend.begin_write().await?;
    let mut op = tx.operation();
    op.set_tenant(workspace).await?;
    if let Err(error) = op
        .require_import_admin(workspace, actor, credential)
        .await?
    {
        if let Err(cleanup) = tx.rollback().await {
            return Err(crate::db::backend::rollback_cleanup_unknown(
                Some(Box::new(ImportRollbackRefusal(error))),
                cleanup,
            ));
        }
        return Ok(Err(error));
    }
    let status = if input.is_some() {
        ImportStatus::Running
    } else {
        ImportStatus::Pending
    };
    if let OperationTx::SqliteFamily(family) = &mut op {
        let now = import_now(family).await?;
        family.execute("INSERT INTO import_jobs(id,workspace_id,created_by,session_id,source,status,file_name,project_id,payload,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?10)", &[
            Cell::uuid(id),Cell::uuid(workspace),Cell::uuid(actor),Cell::optional_uuid(input.as_ref().map(|_|credential)),Cell::text(source.as_str()),Cell::text(status.as_str()),Cell::optional_text(input.as_ref().and_then(|v|v.file_name)),Cell::optional_uuid(input.as_ref().and_then(|v|v.project_id)),input.as_ref().map(|v|Cell::Blob(v.payload.to_vec())).unwrap_or(Cell::Null),Cell::Integer(now)
        ]).await?;
    }
    tx.commit().await.map_err(unknown_commit)?;
    Ok(Ok(ImportJobRow {
        id,
        workspace_id: workspace,
        source,
        status,
        created_refs: ImportJobRefs::default(),
    }))
}

pub async fn get_import_job_backend(
    backend: &Backend,
    workspace: Uuid,
    actor: Uuid,
    credential: Uuid,
    job: Uuid,
) -> Result<Result<ImportJobRow, ImportDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return get_import_job(pool, workspace, actor, credential, job).await;
    }
    let mut tx = backend.begin_write().await?;
    let mut op = tx.operation();
    op.set_tenant(workspace).await?;
    if let Err(error) = op
        .require_import_admin(workspace, actor, credential)
        .await?
    {
        if let Err(cleanup) = tx.rollback().await {
            return Err(crate::db::backend::rollback_cleanup_unknown(
                Some(Box::new(ImportRollbackRefusal(error))),
                cleanup,
            ));
        }
        return Ok(Err(error));
    }
    let rows = match &mut op {
        OperationTx::SqliteFamily(family) => family.query("SELECT id,workspace_id,source,status,created_refs FROM import_jobs WHERE workspace_id=?1 AND id=?2", &[Cell::uuid(workspace),Cell::uuid(job)]).await?,
        OperationTx::Postgres(_) => unreachable!(),
    };
    let row = rows.first().map(job_from_family).transpose()?;
    tx.rollback()
        .await
        .map_err(|cleanup| crate::db::backend::rollback_cleanup_unknown(None, cleanup))?;
    Ok(row.ok_or(ImportDbError::NotFound))
}

pub async fn claim_next_import_job_backend(
    backend: &Backend,
) -> Result<Option<ImportClaim>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return claim_next_import_job(pool).await;
    }
    let mut tx = backend.begin_write().await?;
    let mut op = tx.operation();
    let previous = op.set_system().await?;
    let row = match &mut op {
        OperationTx::SqliteFamily(family) => {
            family.require_system_context()?;
            let now = import_now(family).await?;
            let rows = family.query("SELECT workspace_id,id,attempts,created_by,session_id,source,file_name,project_id,created_refs FROM import_jobs WHERE status='running' AND source<>'markdown-zip' AND payload IS NOT NULL AND session_id IS NOT NULL AND attempts<?1 AND (lease_until IS NULL OR lease_until<?2) AND (attempts=0 OR lease_until IS NOT NULL OR updated_at<?3) ORDER BY created_at,id LIMIT 1", &[Cell::Integer(i64::from(IMPORT_MAX_ATTEMPTS)),Cell::Integer(now),Cell::Integer(now-IMPORT_RETRY_BACKOFF_SECS*1_000_000)]).await?;
            if let Some(row) = rows.first() {
                let token = Uuid::now_v7();
                let attempt = i16::try_from(row.cell(2)?.integer()?)
                    .map_err(|_| import_protocol("attempt out of range"))?
                    + 1;
                let claim = ImportClaim {
                    workspace_id: row.cell(0)?.id()?,
                    job_id: row.cell(1)?.id()?,
                    lease_token: token,
                    attempt,
                    created_by: row.cell(3)?.id()?,
                    session_id: row.cell(4)?.id()?,
                    source: ImportSource::parse(&row.cell(5)?.string()?)
                        .ok_or_else(|| import_protocol("invalid claim source"))?,
                    file_name: row.cell(6)?.optional(Cell::string)?,
                    project_id: row.cell(7)?.optional(Cell::id)?,
                    prior_refs: checked_refs(row.cell(8)?.value()?)?,
                };
                let changed = family.execute("UPDATE import_jobs SET lease_token=?3,lease_until=?4,attempts=?5,updated_at=?6 WHERE workspace_id=?1 AND id=?2", &[Cell::uuid(claim.workspace_id),Cell::uuid(claim.job_id),Cell::uuid(token),Cell::Integer(now+IMPORT_LEASE_SECS*1_000_000),Cell::Integer(i64::from(attempt)),Cell::Integer(now)]).await?;
                if changed != 1 {
                    return Err(import_protocol("claim row disappeared under writer"));
                }
                Some(claim)
            } else {
                None
            }
        }
        OperationTx::Postgres(_) => unreachable!(),
    };
    op.restore_system(previous).await?;
    if row.is_some() {
        tx.commit().await.map_err(unknown_commit)?;
    } else {
        tx.rollback()
            .await
            .map_err(|cleanup| crate::db::backend::rollback_cleanup_unknown(None, cleanup))?;
    }
    Ok(row)
}

pub async fn load_import_payload_backend(
    backend: &Backend,
    claim: &ImportClaim,
) -> Result<Option<Vec<u8>>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return load_import_payload(pool, claim).await;
    }
    let mut tx = backend.begin_write().await?;
    let mut op = tx.operation();
    op.set_tenant(claim.workspace_id).await?;
    let result = match &mut op {
        OperationTx::SqliteFamily(family) => {
            let now = import_now(family).await?;
            let rows=family.query("SELECT payload FROM import_jobs WHERE workspace_id=?1 AND id=?2 AND status='running' AND lease_token=?3 AND lease_until>?4", &claim_args(claim,now)).await?;
            rows.first()
                .map(|r| r.cell(0)?.optional(Cell::bytes))
                .transpose()?
                .flatten()
        }
        OperationTx::Postgres(_) => unreachable!(),
    };
    tx.rollback()
        .await
        .map_err(|cleanup| crate::db::backend::rollback_cleanup_unknown(None, cleanup))?;
    Ok(result)
}

pub async fn extend_import_lease_backend(
    backend: &Backend,
    claim: &ImportClaim,
) -> Result<bool, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return extend_import_lease(pool, claim).await;
    }
    let mut tx = backend.begin_write().await?;
    let mut op = tx.operation();
    op.set_tenant(claim.workspace_id).await?;
    let result = op.hold_import_claim(claim).await?;
    if result {
        tx.commit().await.map_err(unknown_commit)?;
    } else {
        tx.rollback()
            .await
            .map_err(|cleanup| crate::db::backend::rollback_cleanup_unknown(None, cleanup))?;
    }
    Ok(result)
}

async fn discard_family_import_events(
    family: &mut FamilyTx,
    workspace: Uuid,
    job: Uuid,
) -> Result<(), sqlx::Error> {
    family.require_writer()?;
    family.require_tenant(workspace)?;
    family
        .execute(
            "DELETE FROM import_deferred_events WHERE workspace_id=?1 AND import_job_id=?2",
            &[Cell::uuid(workspace), Cell::uuid(job)],
        )
        .await?;
    Ok(())
}

impl OperationTx<'_, '_> {
    pub(crate) async fn finish_import_job(
        &mut self,
        claim: &ImportClaim,
        status: ImportStatus,
    ) -> Result<bool, sqlx::Error> {
        if !matches!(status, ImportStatus::Completed | ImportStatus::Failed) {
            return Err(import_protocol("finish requires terminal status"));
        }
        if status == ImportStatus::Completed
            && matches!(self, Self::SqliteFamily(_))
            && self
                .require_import_admin(claim.workspace_id, claim.created_by, claim.session_id)
                .await?
                .is_err()
        {
            return Ok(false);
        }
        if matches!(self, Self::SqliteFamily(_)) && !self.hold_import_claim(claim).await? {
            return Ok(false);
        }
        match self {
            Self::Postgres(tx) => finish_import_job_in_tx(tx, claim, status).await,
            Self::SqliteFamily(family) => {
                family.require_tenant(claim.workspace_id)?;
                let now = import_now(family).await?;
                let mut args = claim_args(claim, now);
                args.push(Cell::text(status.as_str()));
                let moved=family.execute("UPDATE import_jobs SET status=?5,payload=NULL,lease_token=NULL,lease_until=NULL,updated_at=?4 WHERE workspace_id=?1 AND id=?2 AND status='running' AND lease_token=?3 AND lease_until>?4", &args).await?==1;
                if !moved {
                    return Ok(false);
                }
                if status == ImportStatus::Completed {
                    // Preserve original event order while using the current global
                    // sequence allocator, in this same terminal writer.
                    let rows=family.query("SELECT id,actor_user_id,verb,target_type,target_id,payload,channel,created_at FROM import_deferred_events WHERE workspace_id=?1 AND import_job_id=?2 ORDER BY seq,id", &[Cell::uuid(claim.workspace_id),Cell::uuid(claim.job_id)]).await?;
                    for row in rows {
                        let event = crate::db::identity::EventAppend {
                            id: row.cell(0)?.id()?,
                            workspace_id: Some(claim.workspace_id),
                            actor_user_id: row.cell(1)?.optional(Cell::id)?,
                            verb: row.cell(2)?.string()?,
                            target_type: row.cell(3)?.optional(Cell::string)?,
                            target_id: row.cell(4)?.optional(Cell::id)?,
                            payload: row.cell(5)?.value()?,
                        };
                        let channel = row.cell(6)?.string()?;
                        let created = row.cell(7)?.integer()?;
                        OperationTx::SqliteFamily(family)
                            .append_event_channel(event, &channel)
                            .await?;
                        family
                            .execute(
                                "UPDATE events SET created_at=?2 WHERE id=?1",
                                &[row.cell(0)?, Cell::Integer(created)],
                            )
                            .await?;
                    }
                }
                discard_family_import_events(family, claim.workspace_id, claim.job_id).await?;
                Ok(true)
            }
        }
    }
}

pub async fn finish_import_job_backend(
    backend: &Backend,
    claim: &ImportClaim,
    status: ImportStatus,
) -> Result<bool, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return finish_import_job(pool, claim, status).await;
    }
    let mut tx = backend.begin_write().await?;
    let mut op = tx.operation();
    op.set_tenant(claim.workspace_id).await?;
    let moved = op.finish_import_job(claim, status).await?;
    if moved {
        tx.commit().await.map_err(unknown_commit)?;
    } else {
        tx.rollback()
            .await
            .map_err(|cleanup| crate::db::backend::rollback_cleanup_unknown(None, cleanup))?;
    }
    Ok(moved)
}

pub async fn reset_import_refs_backend(
    backend: &Backend,
    claim: &ImportClaim,
) -> Result<bool, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return reset_import_refs(pool, claim).await;
    }
    release_or_reset_family(backend, claim, true, false).await
}
pub async fn release_import_job_for_retry_backend(
    backend: &Backend,
    claim: &ImportClaim,
    clear_refs: bool,
) -> Result<bool, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return release_import_job_for_retry(pool, claim, clear_refs).await;
    }
    release_or_reset_family(backend, claim, clear_refs, true).await
}
async fn release_or_reset_family(
    backend: &Backend,
    claim: &ImportClaim,
    clear_refs: bool,
    release: bool,
) -> Result<bool, sqlx::Error> {
    let mut tx = backend.begin_write().await?;
    let mut op = tx.operation();
    op.set_tenant(claim.workspace_id).await?;
    let moved = match &mut op {
        OperationTx::SqliteFamily(family) => {
            let now = import_now(family).await?;
            let mut args = claim_args(claim, now);
            args.extend([
                Cell::Integer(i64::from(clear_refs)),
                Cell::Integer(i64::from(release)),
            ]);
            let moved=family.execute("UPDATE import_jobs SET created_refs=CASE WHEN ?5=1 THEN '{\"documentIds\":[],\"taskIds\":[],\"storedKeys\":[]}' ELSE created_refs END,lease_token=CASE WHEN ?6=1 THEN NULL ELSE lease_token END,lease_until=CASE WHEN ?6=1 THEN NULL ELSE lease_until END,updated_at=?4 WHERE workspace_id=?1 AND id=?2 AND status='running' AND lease_token=?3 AND lease_until>?4", &args).await?==1;
            if moved && clear_refs {
                discard_family_import_events(family, claim.workspace_id, claim.job_id).await?;
            }
            moved
        }
        OperationTx::Postgres(_) => unreachable!(),
    };
    if moved {
        tx.commit().await.map_err(unknown_commit)?;
    } else {
        tx.rollback()
            .await
            .map_err(|cleanup| crate::db::backend::rollback_cleanup_unknown(None, cleanup))?;
    }
    Ok(moved)
}

pub async fn finish_sync_import_job_backend(
    backend: &Backend,
    workspace: Uuid,
    job: Uuid,
    actor: Uuid,
    credential: Uuid,
    status: ImportStatus,
) -> Result<bool, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return finish_sync_import_job(pool, workspace, job, status).await;
    }
    if !matches!(status, ImportStatus::Completed | ImportStatus::Failed) {
        return Err(import_protocol("finish requires terminal status"));
    }
    let mut tx = backend.begin_write().await?;
    let mut op = tx.operation();
    op.set_tenant(workspace).await?;
    if !op.hold_sync_import_job(workspace, job, actor).await?
        || (status == ImportStatus::Completed
            && op
                .require_import_admin(workspace, actor, credential)
                .await?
                .is_err())
    {
        tx.rollback()
            .await
            .map_err(|cleanup| crate::db::backend::rollback_cleanup_unknown(None, cleanup))?;
        return Ok(false);
    }
    let moved = match &mut op {
        OperationTx::SqliteFamily(family) => {
            let now = import_now(family).await?;
            family.execute("UPDATE import_jobs SET status=?3,payload=NULL,updated_at=?4 WHERE workspace_id=?1 AND id=?2 AND status IN ('pending','running') AND lease_token IS NULL", &[Cell::uuid(workspace),Cell::uuid(job),Cell::text(status.as_str()),Cell::Integer(now)]).await?==1
        }
        OperationTx::Postgres(_) => unreachable!(),
    };
    if moved {
        tx.commit().await.map_err(unknown_commit)?;
    } else {
        tx.rollback()
            .await
            .map_err(|cleanup| crate::db::backend::rollback_cleanup_unknown(None, cleanup))?;
    }
    Ok(moved)
}

pub async fn claim_expired_import_job_backend(
    backend: &Backend,
) -> Result<Option<ExpiredImport>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return claim_expired_import_job(pool).await;
    }
    let mut tx = backend.begin_write().await?;
    let mut op = tx.operation();
    let previous = op.set_system().await?;
    let expired = match &mut op {
        OperationTx::SqliteFamily(family) => {
            family.require_system_context()?;
            let now = import_now(family).await?;
            let rows=family.query("SELECT workspace_id,id,created_by,created_refs FROM import_jobs WHERE status='running' AND lease_until<?1 ORDER BY lease_until,id LIMIT 1", &[Cell::Integer(now)]).await?;
            if let Some(row) = rows.first() {
                let job = ExpiredImport {
                    workspace_id: row.cell(0)?.id()?,
                    job_id: row.cell(1)?.id()?,
                    created_by: row.cell(2)?.id()?,
                    created_refs: checked_refs(row.cell(3)?.value()?)?,
                };
                family.execute("UPDATE import_jobs SET status='failed',payload=NULL,lease_token=NULL,lease_until=NULL,updated_at=?3 WHERE workspace_id=?1 AND id=?2", &[Cell::uuid(job.workspace_id),Cell::uuid(job.job_id),Cell::Integer(now)]).await?;
                // System claim owns this row; restore the prior tenant after
                // discarding its parked effects within this same transaction.
                family.set_tenant(job.workspace_id)?;
                discard_family_import_events(family, job.workspace_id, job.job_id).await?;
                Some(job)
            } else {
                None
            }
        }
        OperationTx::Postgres(_) => unreachable!(),
    };
    op.restore_system(previous).await?;
    if expired.is_some() {
        tx.commit().await.map_err(unknown_commit)?;
    } else {
        tx.rollback()
            .await
            .map_err(|cleanup| crate::db::backend::rollback_cleanup_unknown(None, cleanup))?;
    }
    Ok(expired)
}

pub async fn fail_stale_sync_import_jobs_backend(backend: &Backend) -> Result<u64, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return fail_stale_sync_import_jobs(pool).await;
    }
    let mut tx = backend.begin_write().await?;
    let mut op = tx.operation();
    let previous = op.set_system().await?;
    let moved = match &mut op {
        OperationTx::SqliteFamily(family) => {
            family.require_system_context()?;
            let now = import_now(family).await?;
            family.execute("UPDATE import_jobs SET status='failed',payload=NULL,updated_at=?1 WHERE id IN (SELECT id FROM import_jobs WHERE source='markdown-zip' AND status='pending' AND lease_token IS NULL AND updated_at<?2 ORDER BY updated_at,id LIMIT ?3)", &[Cell::Integer(now),Cell::Integer(now-SYNC_IMPORT_STALE_SECS*1_000_000),Cell::Integer(IMPORT_SWEEP_MAX as i64)]).await?
        }
        OperationTx::Postgres(_) => unreachable!(),
    };
    op.restore_system(previous).await?;
    if moved > 0 {
        tx.commit().await.map_err(unknown_commit)?;
    } else {
        tx.rollback()
            .await
            .map_err(|cleanup| crate::db::backend::rollback_cleanup_unknown(None, cleanup))?;
    }
    Ok(moved)
}

/// Cleanup authority comes from this job's durable refs, not caller-supplied IDs.
/// A live owner must still match its lease; a sweep owns a terminal failed row.
pub(crate) enum ImportCleanupOwner<'a> {
    Runner(&'a ImportClaim),
    Expired(&'a ExpiredImport),
}
impl ImportCleanupOwner<'_> {
    fn identity(&self) -> (Uuid, Uuid) {
        match self {
            Self::Runner(c) => (c.workspace_id, c.job_id),
            Self::Expired(c) => (c.workspace_id, c.job_id),
        }
    }
}
async fn cleanup_refs(
    family: &mut FamilyTx,
    owner: &ImportCleanupOwner<'_>,
) -> Result<Option<ImportJobRefs>, sqlx::Error> {
    let (workspace, job) = owner.identity();
    family.require_tenant(workspace)?;
    let now = import_now(family).await?;
    let rows = match owner {
        ImportCleanupOwner::Runner(claim) => family.query("SELECT created_refs FROM import_jobs WHERE workspace_id=?1 AND id=?2 AND status='running' AND lease_token=?3 AND lease_until>?4", &claim_args(claim,now)).await?,
        ImportCleanupOwner::Expired(_) => family.query("SELECT created_refs FROM import_jobs WHERE workspace_id=?1 AND id=?2 AND source<>'markdown-zip' AND status='failed' AND lease_token IS NULL", &[Cell::uuid(workspace),Cell::uuid(job)]).await?,
    };
    rows.first()
        .map(|r| checked_refs(r.cell(0)?.value()?))
        .transpose()
}

#[cfg(test)]
tokio::task_local! {
    // Propagation control only: injection follows a real acknowledged local
    // rollback. This is not an actual provider rollback-response loss oracle.
    pub(crate) static IMPORT_ROLLBACK_AFTER_ACK_CONTROL: bool;
    static IMPORT_CLEANUP_AFTER_PURGE: std::sync::Arc<(tokio::sync::Notify,tokio::sync::Notify)>;
}

/// Real selected-family compensation under one current writer. Keeping refs
/// until every physical purge and commit succeeds makes partial cleanup retryable.
pub(crate) async fn compensate_family_import(
    backend: &Backend,
    storage: &crate::attachments::ObjectStorage,
    owner: ImportCleanupOwner<'_>,
    refs: &ImportJobRefs,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<crate::import_job::CompensateOutcome, sqlx::Error> {
    if matches!(backend, Backend::Postgres(_)) {
        return Err(import_protocol(
            "family compensation requires selected family",
        ));
    }
    let (workspace, _) = owner.identity();
    let mut tx = backend.begin_write().await?;
    tx.operation().set_tenant(workspace).await?;
    let result = {
        let mut op = tx.operation();
        let OperationTx::SqliteFamily(family) = &mut op else {
            unreachable!()
        };
        compensate_family_import_tx(family, storage, owner, refs, cancel, None).await
    };
    match result {
        Ok(outcome) => {
            tx.commit().await.map_err(unknown_commit)?;
            Ok(outcome)
        }
        Err(error) => {
            let cleanup = tx.rollback().await;
            #[cfg(test)]
            let cleanup = cleanup.and_then(|()| {
                if IMPORT_ROLLBACK_AFTER_ACK_CONTROL
                    .try_with(|enabled| *enabled)
                    .unwrap_or(false)
                {
                    Err(sqlx::Error::Io(std::io::Error::new(
                        std::io::ErrorKind::ConnectionAborted,
                        "after-real-rollback propagation control",
                    )))
                } else {
                    Ok(())
                }
            });
            if let Err(cleanup) = cleanup {
                return Err(crate::db::backend::rollback_cleanup_unknown(
                    Some(Box::new(error)),
                    cleanup,
                ));
            }
            Err(error)
        }
    }
}

/// Borrowed Daily authority from the existing selected maintenance owner.
#[derive(Clone, Copy)]
pub(crate) struct ImportMaintenanceContext<'a> {
    pub proof: &'a crate::db::maintenance_claim::FamilyMaintenanceProof,
    pub policy: crate::db::maintenance_claim::FamilyMaintenanceLeasePolicy,
}

async fn renew_import_maintenance(
    family: &mut FamilyTx,
    context: ImportMaintenanceContext<'_>,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<(), sqlx::Error> {
    if cancel.is_cancelled() {
        return Err(import_protocol("import maintenance cancelled"));
    }
    if OperationTx::SqliteFamily(family)
        .renew_family_maintenance_claim(
            context.proof,
            crate::db::maintenance_claim::MaintenanceJobKey::Daily,
            context.policy,
        )
        .await?
        .is_none()
    {
        return Err(import_protocol("current Daily maintenance owner lost"));
    }
    Ok(())
}

async fn compensate_family_import_tx(
    family: &mut FamilyTx,
    storage: &crate::attachments::ObjectStorage,
    owner: ImportCleanupOwner<'_>,
    refs: &ImportJobRefs,
    cancel: &tokio_util::sync::CancellationToken,
    maintenance: Option<ImportMaintenanceContext<'_>>,
) -> Result<crate::import_job::CompensateOutcome, sqlx::Error> {
    let (workspace, _) = owner.identity();
    if let Some(context) = maintenance {
        renew_import_maintenance(family, context, cancel).await?;
    }
    let durable = cleanup_refs(family, &owner)
        .await?
        .ok_or_else(|| import_protocol("cleanup owner fenced"))?;
    if !refs
        .document_ids
        .iter()
        .all(|id| durable.document_ids.contains(id))
        || !refs.task_ids.iter().all(|id| durable.task_ids.contains(id))
        || !refs
            .stored_keys
            .iter()
            .all(|key| durable.stored_keys.contains(key))
    {
        return Err(import_protocol("cleanup refs do not belong to current job"));
    }
    if maintenance.is_some() && refs != &durable {
        return Err(import_protocol("maintenance refs changed"));
    }
    let mut outcome = crate::import_job::CompensateOutcome::default();
    let mut keys = std::collections::BTreeSet::from_iter(refs.stored_keys.iter().cloned());
    for task in refs.task_ids.iter().rev() {
        if cancel.is_cancelled() {
            return Err(import_protocol("cleanup cancelled"));
        }
        if let Some(context) = maintenance {
            renew_import_maintenance(family, context, cancel).await?;
        }
        let found = family
            .query(
                "SELECT id FROM tasks WHERE workspace_id=?1 AND id=?2",
                &[Cell::uuid(workspace), Cell::uuid(*task)],
            )
            .await?;
        if found.is_empty() {
            outcome.skipped += 1;
            continue;
        }
        let now = import_now(family).await?;
        let detached=family.query("UPDATE tasks SET parent_id=NULL,type=CASE WHEN type='subtask' THEN 'task' ELSE type END,updated_at=?3 WHERE workspace_id=?1 AND parent_id=?2 RETURNING id,type", &[Cell::uuid(workspace),Cell::uuid(*task),Cell::Integer(now)]).await?;
        for child in detached {
            OperationTx::SqliteFamily(family).append_event(crate::db::identity::EventAppend{id:Uuid::now_v7(),workspace_id:Some(workspace),actor_user_id:None,verb:"task.updated".into(),target_type:Some("task".into()),target_id:Some(child.cell(0)?.id()?),payload:json!({"taskId":child.cell(0)?.id()?.to_string(),"parentId":null,"type":child.cell(1)?.string()?})}).await?;
        }
        let attachments=family.query("DELETE FROM attachments WHERE workspace_id=?1 AND task_id=?2 RETURNING storage_key", &[Cell::uuid(workspace),Cell::uuid(*task)]).await?;
        for row in attachments {
            keys.insert(row.cell(0)?.string()?);
        }
        family
            .execute(
                "DELETE FROM tasks WHERE workspace_id=?1 AND id=?2",
                &[Cell::uuid(workspace), Cell::uuid(*task)],
            )
            .await?;
    }
    for document in refs.document_ids.iter().rev() {
        if cancel.is_cancelled() {
            return Err(import_protocol("cleanup cancelled"));
        }
        if let Some(context) = maintenance {
            renew_import_maintenance(family, context, cancel).await?;
        }
        let found = family
            .query(
                "SELECT id FROM documents WHERE workspace_id=?1 AND id=?2",
                &[Cell::uuid(workspace), Cell::uuid(*document)],
            )
            .await?;
        if found.is_empty() {
            outcome.skipped += 1;
            continue;
        }
        let attachments=family.query("DELETE FROM attachments WHERE workspace_id=?1 AND document_id=?2 RETURNING storage_key", &[Cell::uuid(workspace),Cell::uuid(*document)]).await?;
        for row in attachments {
            keys.insert(row.cell(0)?.string()?);
        }
        family
            .execute(
                "DELETE FROM documents WHERE workspace_id=?1 AND id=?2",
                &[Cell::uuid(workspace), Cell::uuid(*document)],
            )
            .await?;
        OperationTx::SqliteFamily(family)
            .append_event(crate::db::identity::EventAppend {
                id: Uuid::now_v7(),
                workspace_id: Some(workspace),
                actor_user_id: None,
                verb: "document.purged".into(),
                target_type: Some("document".into()),
                target_id: Some(*document),
                payload: json!({"documentId":document.to_string()}),
            })
            .await?;
    }
    for key in keys {
        if cancel.is_cancelled() || cleanup_refs(family, &owner).await?.is_none() {
            return Err(import_protocol("cleanup cancelled or fenced"));
        }
        // Check every current original/preview reference, including other
        // tenants; UUID key ownership must not permit purging a shared key.
        let previous_system = family.replace_system_context(true);
        family.require_system_context()?;
        let used=family.query("SELECT EXISTS(SELECT 1 FROM attachments WHERE storage_key=?1 OR json_extract(variants,'$.preview.key')=?1)", &[Cell::text(&key)]).await?;
        family.replace_system_context(previous_system);
        if used
            .first()
            .ok_or(sqlx::Error::RowNotFound)?
            .cell(0)?
            .boolean()?
        {
            return Err(import_protocol("cleanup key still referenced"));
        }
        if let Some(context) = maintenance {
            renew_import_maintenance(family, context, cancel).await?;
        }
        storage
            .purge_key(&key)
            .await
            .map_err(|e| sqlx::Error::Io(std::io::Error::other(e)))?;
        #[cfg(test)]
        if let Ok(pause) = IMPORT_CLEANUP_AFTER_PURGE.try_with(std::sync::Arc::clone) {
            pause.0.notify_one();
            pause.1.notified().await;
        }
        if cancel.is_cancelled() || cleanup_refs(family, &owner).await?.is_none() {
            return Err(import_protocol("cleanup cancelled or fenced after purge"));
        }
        if let Some(context) = maintenance {
            renew_import_maintenance(family, context, cancel).await?;
        }
        if storage
            .head(&key)
            .await
            .map_err(|e| sqlx::Error::Io(std::io::Error::other(e)))?
            .is_some()
        {
            return Err(import_protocol("cleanup payload survived purge"));
        }
    }
    if cancel.is_cancelled() || cleanup_refs(family, &owner).await?.is_none() {
        return Err(import_protocol("cleanup cancelled or fenced before commit"));
    }
    if let Some(context) = maintenance {
        renew_import_maintenance(family, context, cancel).await?;
        let (workspace, job) = owner.identity();
        let changed=family.execute("UPDATE import_jobs SET created_refs=?3 WHERE workspace_id=?1 AND id=?2 AND source<>'markdown-zip' AND status='failed' AND lease_token IS NULL AND json(created_refs)=json(?4)", &[
                Cell::uuid(workspace),Cell::uuid(job),Cell::json(&json!(ImportJobRefs::default()))?,Cell::json(&json!(durable))?
            ]).await?;
        if changed != 1 {
            return Err(import_protocol("failed cleanup refs changed before clear"));
        }
        renew_import_maintenance(family, context, cancel).await?;
    }
    // Runner reset/release owns ref clearing. Maintenance clears matching
    // FAILED refs only after physical absence, in this same actual writer.
    Ok(outcome)
}

/// Bounded observational candidate list. Every mutation below independently
/// borrows and checks the current Daily proof in its own actual writer.
pub(crate) async fn import_cleanup_candidates_backend(
    backend: &Backend,
    context: ImportMaintenanceContext<'_>,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<Vec<ExpiredImport>, sqlx::Error> {
    let mut tx = backend.begin_write().await?;
    let mut op = tx.operation();
    let previous = op.set_system().await?;
    let result=async {
        let OperationTx::SqliteFamily(family)=&mut op else{return Err(import_protocol("family maintenance candidates require selected family"));};
        renew_import_maintenance(family,context,cancel).await?;
        family.require_system_context()?;
        let now=import_now(family).await?;
        let rows=family.query("SELECT workspace_id,id,created_by,created_refs FROM import_jobs WHERE source<>'markdown-zip' AND ((status='running' AND lease_until<?1) OR (status='failed' AND lease_token IS NULL AND (json_array_length(created_refs,'$.documentIds')>0 OR json_array_length(created_refs,'$.taskIds')>0 OR json_array_length(created_refs,'$.storedKeys')>0))) ORDER BY coalesce(lease_until,updated_at),id LIMIT ?2", &[Cell::Integer(now),Cell::Integer(IMPORT_SWEEP_MAX as i64)]).await?;
        rows.iter().map(|row|Ok(ExpiredImport{workspace_id:row.cell(0)?.id()?,job_id:row.cell(1)?.id()?,created_by:row.cell(2)?.id()?,created_refs:checked_refs(row.cell(3)?.value()?)?})).collect::<Result<Vec<_>,sqlx::Error>>()
    }.await;
    op.restore_system(previous).await?;
    if let Err(cleanup) = tx.rollback().await {
        return Err(crate::db::backend::rollback_cleanup_unknown(
            result
                .err()
                .map(|error| Box::new(error) as Box<dyn std::error::Error + Send + Sync>),
            cleanup,
        ));
    }
    result
}

/// Prep terminal failure before external cleanup. This phase never parses,
/// increments attempts or resurrects a FAILED job. Unknown commit stops caller.
pub(crate) async fn prepare_import_cleanup_backend(
    backend: &Backend,
    job: &ExpiredImport,
    context: ImportMaintenanceContext<'_>,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<bool, sqlx::Error> {
    let mut tx = backend.begin_write().await?;
    tx.operation().set_tenant(job.workspace_id).await?;
    let result=async {
        let mut op=tx.operation();
        let OperationTx::SqliteFamily(family)=&mut op else{return Err(import_protocol("family cleanup prep requires selected family"));};
        renew_import_maintenance(family,context,cancel).await?;
        let now=import_now(family).await?;
        let rows=family.query("SELECT created_by,created_refs,status FROM import_jobs WHERE workspace_id=?1 AND id=?2 AND source<>'markdown-zip' AND ((status='running' AND lease_until<?3) OR (status='failed' AND lease_token IS NULL))", &[Cell::uuid(job.workspace_id),Cell::uuid(job.job_id),Cell::Integer(now)]).await?;
        let Some(row)=rows.first() else{return Ok(false);};
        if row.cell(0)?.id()?!=job.created_by || checked_refs(row.cell(1)?.value()?)?!=job.created_refs{return Ok(false);}
        if row.cell(2)?.string()?=="running"{
            family.execute("UPDATE import_jobs SET status='failed',payload=NULL,lease_until=NULL,lease_token=NULL,updated_at=?3 WHERE workspace_id=?1 AND id=?2", &[Cell::uuid(job.workspace_id),Cell::uuid(job.job_id),Cell::Integer(now)]).await?;
            discard_family_import_events(family,job.workspace_id,job.job_id).await?;
        }
        renew_import_maintenance(family,context,cancel).await?;
        Ok(true)
    }.await;
    match result {
        Ok(true) => {
            tx.commit().await.map_err(unknown_commit)?;
            Ok(true)
        }
        Ok(false) => {
            tx.rollback()
                .await
                .map_err(|cleanup| crate::db::backend::rollback_cleanup_unknown(None, cleanup))?;
            Ok(false)
        }
        Err(error) => {
            if let Err(cleanup) = tx.rollback().await {
                return Err(crate::db::backend::rollback_cleanup_unknown(
                    Some(Box::new(error)),
                    cleanup,
                ));
            }
            Err(error)
        }
    }
}

pub(crate) async fn cleanup_failed_import_backend(
    backend: &Backend,
    storage: &crate::attachments::ObjectStorage,
    job: &ExpiredImport,
    context: ImportMaintenanceContext<'_>,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<crate::import_job::CompensateOutcome, sqlx::Error> {
    let mut tx = backend.begin_write().await?;
    tx.operation().set_tenant(job.workspace_id).await?;
    let result = {
        let mut op = tx.operation();
        let OperationTx::SqliteFamily(family) = &mut op else {
            return Err(import_protocol("failed cleanup requires selected family"));
        };
        compensate_family_import_tx(
            family,
            storage,
            ImportCleanupOwner::Expired(job),
            &job.created_refs,
            cancel,
            Some(context),
        )
        .await
    };
    match result {
        Ok(outcome) => {
            tx.commit().await.map_err(unknown_commit)?;
            Ok(outcome)
        }
        Err(error) => {
            if let Err(cleanup) = tx.rollback().await {
                return Err(crate::db::backend::rollback_cleanup_unknown(
                    Some(Box::new(error)),
                    cleanup,
                ));
            }
            Err(error)
        }
    }
}

pub(crate) async fn fail_stale_sync_imports_with_maintenance_backend(
    backend: &Backend,
    context: ImportMaintenanceContext<'_>,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<u64, sqlx::Error> {
    let mut tx = backend.begin_write().await?;
    let mut op = tx.operation();
    let previous = op.set_system().await?;
    let result=async {
        let OperationTx::SqliteFamily(family)=&mut op else{return Err(import_protocol("sync sweep requires selected family"));};
        renew_import_maintenance(family,context,cancel).await?;
        family.require_system_context()?;
        let now=import_now(family).await?;
        let moved=family.execute("UPDATE import_jobs SET status='failed',payload=NULL,updated_at=?1 WHERE id IN (SELECT id FROM import_jobs WHERE source='markdown-zip' AND status='pending' AND lease_token IS NULL AND updated_at<?2 ORDER BY updated_at,id LIMIT ?3)", &[Cell::Integer(now),Cell::Integer(now-SYNC_IMPORT_STALE_SECS*1_000_000),Cell::Integer(IMPORT_SWEEP_MAX as i64)]).await?;
        renew_import_maintenance(family,context,cancel).await?;
        Ok(moved)
    }.await;
    op.restore_system(previous).await?;
    match result {
        Ok(moved) => {
            tx.commit().await.map_err(unknown_commit)?;
            Ok(moved)
        }
        Err(error) => {
            if let Err(cleanup) = tx.rollback().await {
                return Err(crate::db::backend::rollback_cleanup_unknown(
                    Some(Box::new(error)),
                    cleanup,
                ));
            }
            Err(error)
        }
    }
}

impl OperationTx<'_, '_> {
    /// Selected producer parks only a live Notion job's events. Office imports
    /// retain their existing immediate event behavior; non-import callers do
    /// not use this method.
    pub(crate) async fn park_import_event(
        &mut self,
        workspace: Uuid,
        fence: crate::db::documents::ImportFence,
        event: crate::db::identity::EventAppend,
        channel: &str,
    ) -> Result<bool, sqlx::Error> {
        if event.workspace_id != Some(workspace) {
            return Err(import_protocol("event tenant mismatch"));
        }
        match self {
            Self::Postgres(tx) => {
                if !hold_import_fence(tx, workspace, fence).await? {
                    return Ok(false);
                }
                crate::db::identity::append_event_channel(tx, event, channel).await?;
            }
            Self::SqliteFamily(family) => {
                family.require_tenant(workspace)?;
                let now = import_now(family).await?;
                let rows=family.query("SELECT source,created_by FROM import_jobs WHERE workspace_id=?1 AND id=?2 AND status='running' AND lease_token=?3 AND lease_until>?4", &[Cell::uuid(workspace),Cell::uuid(fence.job_id),Cell::uuid(fence.lease_token),Cell::Integer(now)]).await?;
                let Some(row) = rows.first() else {
                    return Ok(false);
                };
                let creator = row.cell(1)?.id()?;
                if event.actor_user_id.is_some_and(|actor| actor != creator) {
                    return Ok(false);
                }
                if row.cell(0)?.string()? == ImportSource::NotionZip.as_str() {
                    family.execute("INSERT INTO import_deferred_events(workspace_id,import_job_id,id,seq,actor_user_id,verb,target_type,target_id,payload,channel,created_at) VALUES(?1,?2,?3,(SELECT coalesce(max(seq),0)+1 FROM import_deferred_events WHERE workspace_id=?1 AND import_job_id=?2),?4,?5,?6,?7,?8,?9,?10)", &[Cell::uuid(workspace),Cell::uuid(fence.job_id),Cell::uuid(event.id),Cell::optional_uuid(event.actor_user_id),Cell::text(event.verb),Cell::optional_text(event.target_type.as_deref()),Cell::optional_uuid(event.target_id),Cell::json(&event.payload)?,Cell::text(channel),Cell::Integer(now)]).await?;
                } else {
                    OperationTx::SqliteFamily(family)
                        .append_event_channel(event, channel)
                        .await?;
                }
            }
        }
        Ok(true)
    }

    /// Request-driven sync job proof. The producer separately rechecks current
    /// admin/session in this same writer. Never renew a request's stale cutoff.
    pub(crate) async fn hold_sync_import_job(
        &mut self,
        workspace: Uuid,
        job: Uuid,
        actor: Uuid,
    ) -> Result<bool, sqlx::Error> {
        match self {
            Self::Postgres(tx) => {
                let found:Option<(Uuid,)>=sqlx::query_as("SELECT id FROM fvoci.import_jobs WHERE workspace_id=$1 AND id=$2 AND created_by=$3 AND source='markdown-zip' AND status='pending' AND lease_token IS NULL FOR UPDATE").bind(workspace).bind(job).bind(actor).fetch_optional(&mut ***tx).await?;
                Ok(found.is_some())
            }
            Self::SqliteFamily(family) => {
                family.require_writer()?;
                family.require_tenant(workspace)?;
                let rows=family.query("SELECT id FROM import_jobs WHERE workspace_id=?1 AND id=?2 AND created_by=?3 AND source='markdown-zip' AND status='pending' AND lease_token IS NULL", &[Cell::uuid(workspace),Cell::uuid(job),Cell::uuid(actor)]).await?;
                Ok(!rows.is_empty())
            }
        }
    }
    /// Borrowed current sync proof for the canonical native publisher. The
    /// caller still checks current admin/session; no lease or writer is made.
    pub(crate) async fn sync_import_contains_document_ref(
        &mut self,
        workspace: Uuid,
        job: Uuid,
        actor: Uuid,
        document: Uuid,
    ) -> Result<bool, sqlx::Error> {
        if !self.hold_sync_import_job(workspace, job, actor).await? {
            return Ok(false);
        }
        let refs = match self {
            Self::Postgres(tx) => checked_refs(
                sqlx::query_scalar(
                    "SELECT created_refs FROM fvoci.import_jobs WHERE workspace_id=$1 AND id=$2",
                )
                .bind(workspace)
                .bind(job)
                .fetch_one(&mut ***tx)
                .await?,
            )?,
            Self::SqliteFamily(family) => {
                let rows = family
                    .query(
                        "SELECT created_refs FROM import_jobs WHERE workspace_id=?1 AND id=?2",
                        &[Cell::uuid(workspace), Cell::uuid(job)],
                    )
                    .await?;
                checked_refs(
                    rows.first()
                        .ok_or(sqlx::Error::RowNotFound)?
                        .cell(0)?
                        .value()?,
                )?
            }
        };
        Ok(refs.document_ids.contains(&document))
    }

    pub(crate) async fn append_sync_import_document_ref(
        &mut self,
        workspace: Uuid,
        job: Uuid,
        actor: Uuid,
        document: Uuid,
    ) -> Result<bool, sqlx::Error> {
        if !self.hold_sync_import_job(workspace, job, actor).await? {
            return Ok(false);
        }
        match self {
            Self::Postgres(tx) => {
                sqlx::query("UPDATE fvoci.import_jobs SET created_refs=jsonb_set(created_refs,'{documentIds}',(created_refs->'documentIds')||jsonb_build_array($3::text)) WHERE workspace_id=$1 AND id=$2").bind(workspace).bind(job).bind(document.to_string()).execute(&mut ***tx).await?;
            }
            Self::SqliteFamily(family) => {
                family.execute("UPDATE import_jobs SET created_refs=json_insert(created_refs,'$.documentIds[#]',?3) WHERE workspace_id=?1 AND id=?2", &[Cell::uuid(workspace),Cell::uuid(job),Cell::text(document.to_string())]).await?;
            }
        }
        Ok(true)
    }
}

#[cfg(test)]
mod selected_import_tests {
    use super::*;
    use crate::db::attachment_preview::tests::Fixture;
    use tokio_util::sync::CancellationToken;

    async fn session(f: &Fixture) -> Uuid {
        let id = Uuid::now_v7();
        let token = crate::auth::token::new_token();
        let mut tx = f.backend.begin_write().await.unwrap();
        tx.operation()
            .create_session(
                id,
                f.user,
                &token.hash,
                chrono::DateTime::from_timestamp_micros(
                    chrono::Utc::now().timestamp_micros() + 86_400_000_000,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        tx.commit().await.unwrap();
        id
    }
    async fn queued(f: &Fixture, credential: Uuid, source: ImportSource) -> ImportJobRow {
        create_async_import_job_backend(
            &f.backend,
            f.workspace,
            f.user,
            credential,
            source,
            NewAsyncImport {
                file_name: Some("actual.txt"),
                project_id: None,
                payload: b"literal selected import bytes",
            },
        )
        .await
        .unwrap()
        .unwrap()
    }
    async fn claim(f: &Fixture) -> ImportClaim {
        claim_next_import_job_backend(&f.backend)
            .await
            .unwrap()
            .unwrap()
    }
    async fn refs(f: &Fixture, c: &ImportClaim, document: Uuid, key: Option<&str>) {
        let mut tx = f.backend.begin_write().await.unwrap();
        let mut op = tx.operation();
        op.set_tenant(f.workspace).await.unwrap();
        let fence = crate::db::documents::ImportFence {
            job_id: c.job_id,
            lease_token: c.lease_token,
        };
        assert!(op
            .append_import_ref(
                f.workspace,
                fence,
                ImportRefKind::Document,
                &document.to_string()
            )
            .await
            .unwrap());
        if let Some(key) = key {
            assert!(op
                .append_import_ref(f.workspace, fence, ImportRefKind::StoredKey, key)
                .await
                .unwrap());
        }
        tx.commit().await.unwrap();
    }
    fn event(f: &Fixture, verb: &str) -> crate::db::identity::EventAppend {
        crate::db::identity::EventAppend {
            id: Uuid::now_v7(),
            workspace_id: Some(f.workspace),
            actor_user_id: Some(f.user),
            verb: verb.into(),
            target_type: Some("document".into()),
            target_id: Some(f.document),
            payload: json!({"literal":"한글 import receipt"}),
        }
    }
    async fn parked(f: &Fixture, c: &ImportClaim) {
        let mut tx = f.backend.begin_write().await.unwrap();
        let mut op = tx.operation();
        op.set_tenant(f.workspace).await.unwrap();
        let fence = crate::db::documents::ImportFence {
            job_id: c.job_id,
            lease_token: c.lease_token,
        };
        assert!(op
            .park_import_event(f.workspace, fence, event(f, "import.first"), "web")
            .await
            .unwrap());
        assert!(op
            .park_import_event(f.workspace, fence, event(f, "import.second"), "web")
            .await
            .unwrap());
        tx.commit().await.unwrap();
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM events")
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn import_selected_cancel_after_actual_purge_keeps_pointer_and_healthy_retry() {
        let f = Fixture::new().await;
        let credential = session(&f).await;
        queued(&f, credential, ImportSource::NotionZip).await;
        let c = claim(&f).await;
        let (_, key) = f.attachment(23, "text/plain").await;
        let storage = crate::attachments::ObjectStorage::local(f.root.join("import-storage"));
        storage
            .put_bytes(&key, b"actually purged import bytes".to_vec())
            .await
            .unwrap();
        refs(&f, &c, f.document, Some(&key)).await;
        let durable = get_import_job_backend(&f.backend, f.workspace, f.user, credential, c.job_id)
            .await
            .unwrap()
            .unwrap()
            .created_refs;
        let pause = std::sync::Arc::new((tokio::sync::Notify::new(), tokio::sync::Notify::new()));
        let cancel = CancellationToken::new();
        let (backend, store, owner, created, token) = (
            f.backend.clone(),
            storage.clone(),
            c.clone(),
            durable.clone(),
            cancel.clone(),
        );
        let task = tokio::spawn(IMPORT_CLEANUP_AFTER_PURGE.scope(pause.clone(), async move {
            compensate_family_import(
                &backend,
                &store,
                ImportCleanupOwner::Runner(&owner),
                &created,
                &token,
            )
            .await
        }));
        pause.0.notified().await;
        assert!(
            storage.head(&key).await.unwrap().is_none(),
            "the real purge must have settled before cancellation"
        );
        cancel.cancel();
        pause.1.notify_one();
        assert!(task.await.unwrap().is_err());
        let counts:(i64,i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM documents WHERE id=?1),(SELECT count(*) FROM attachments WHERE storage_key=?2),(SELECT count(*) FROM events WHERE verb='document.purged')").bind(f.document.as_bytes().as_slice()).bind(&key).fetch_one(&f.pool).await.unwrap();
        assert_eq!(counts, (1, 1, 0));
        assert_eq!(
            get_import_job_backend(&f.backend, f.workspace, f.user, credential, c.job_id)
                .await
                .unwrap()
                .unwrap()
                .created_refs,
            durable
        );
        assert_eq!(
            compensate_family_import(
                &f.backend,
                &storage,
                ImportCleanupOwner::Runner(&c),
                &durable,
                &CancellationToken::new()
            )
            .await
            .unwrap()
            .failed,
            0
        );
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM documents WHERE id=?1")
            .bind(f.document.as_bytes().as_slice())
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
        assert!(storage.head(&key).await.unwrap().is_none());
        assert!(reset_import_refs_backend(&f.backend, &c).await.unwrap());
        f.close().await;
    }

    #[tokio::test]
    async fn import_selected_sync_owner_and_late_revocation_fence_terminal() {
        let f = Fixture::new().await;
        let credential = session(&f).await;
        let job = create_sync_import_job_backend(&f.backend, f.workspace, f.user, credential)
            .await
            .unwrap()
            .unwrap();
        assert!(!finish_sync_import_job_backend(
            &f.backend,
            f.workspace,
            job.id,
            Uuid::now_v7(),
            credential,
            ImportStatus::Completed
        )
        .await
        .unwrap());
        let mut tx = f.backend.begin_write().await.unwrap();
        let mut op = tx.operation();
        op.set_tenant(f.workspace).await.unwrap();
        assert!(op
            .append_sync_import_document_ref(f.workspace, job.id, f.user, f.document)
            .await
            .unwrap());
        tx.commit().await.unwrap();
        sqlx::query("UPDATE sessions SET revoked_at=1 WHERE id=?1")
            .bind(credential.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(!finish_sync_import_job_backend(
            &f.backend,
            f.workspace,
            job.id,
            f.user,
            credential,
            ImportStatus::Completed
        )
        .await
        .unwrap());
        let actual: String = sqlx::query_scalar("SELECT status FROM import_jobs WHERE id=?1")
            .bind(job.id.as_bytes().as_slice())
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(actual, "pending");
        // Failure may settle this exact request's row after revocation; sync
        // import's already-created documents remain, as in the PG contract.
        assert!(finish_sync_import_job_backend(
            &f.backend,
            f.workspace,
            job.id,
            f.user,
            credential,
            ImportStatus::Failed
        )
        .await
        .unwrap());
        let fresh = session(&f).await;
        let observed = get_import_job_backend(&f.backend, f.workspace, f.user, fresh, job.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(observed.status, ImportStatus::Failed);
        assert_eq!(observed.created_refs.document_ids, vec![f.document]);
        let retry = create_sync_import_job_backend(&f.backend, f.workspace, f.user, fresh)
            .await
            .unwrap()
            .unwrap();
        assert!(finish_sync_import_job_backend(
            &f.backend,
            f.workspace,
            retry.id,
            f.user,
            fresh,
            ImportStatus::Completed
        )
        .await
        .unwrap());
        f.close().await;
    }

    #[tokio::test]
    async fn import_selected_claim_order_backoff_two_attempts_and_old_fence() {
        let f = Fixture::new().await;
        let credential = session(&f).await;
        let first = queued(&f, credential, ImportSource::OfficeFile).await;
        let second = queued(&f, credential, ImportSource::OfficeFile).await;
        sqlx::query("UPDATE import_jobs SET created_at=CASE WHEN id=?1 THEN 1 ELSE 2 END")
            .bind(first.id.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let old = claim(&f).await;
        assert_eq!(old.job_id, first.id);
        assert_eq!(old.attempt, 1);
        assert_eq!(
            load_import_payload_backend(&f.backend, &old)
                .await
                .unwrap()
                .unwrap(),
            b"literal selected import bytes"
        );
        let other = claim(&f).await;
        assert_eq!(other.job_id, second.id);
        assert!(claim_next_import_job_backend(&f.backend)
            .await
            .unwrap()
            .is_none());
        assert!(
            release_import_job_for_retry_backend(&f.backend, &old, false)
                .await
                .unwrap()
        );
        assert!(claim_next_import_job_backend(&f.backend)
            .await
            .unwrap()
            .is_none());
        sqlx::query("UPDATE import_jobs SET updated_at=1 WHERE id=?1")
            .bind(first.id.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let current = claim(&f).await;
        assert_eq!(current.job_id, first.id);
        assert_eq!(current.attempt, 2);
        assert_ne!(current.lease_token, old.lease_token);
        assert!(!extend_import_lease_backend(&f.backend, &old).await.unwrap());
        assert!(load_import_payload_backend(&f.backend, &old)
            .await
            .unwrap()
            .is_none());
        let mut wrong = current.clone();
        wrong.workspace_id = Uuid::now_v7();
        assert!(load_import_payload_backend(&f.backend, &wrong)
            .await
            .unwrap()
            .is_none());
        assert!(!reset_import_refs_backend(&f.backend, &wrong).await.unwrap());
        sqlx::query("UPDATE import_jobs SET lease_until=1 WHERE id=?1")
            .bind(first.id.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(claim_next_import_job_backend(&f.backend)
            .await
            .unwrap()
            .is_none());
        let expired = claim_expired_import_job_backend(&f.backend)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(expired.job_id, first.id);
        assert!(
            !finish_import_job_backend(&f.backend, &current, ImportStatus::Completed)
                .await
                .unwrap()
        );
        f.close().await;
    }

    #[tokio::test]
    async fn import_selected_current_session_role_and_tenant_no_row() {
        let f = Fixture::new().await;
        let credential = session(&f).await;
        let job = queued(&f, credential, ImportSource::OfficeFile).await;
        assert!(matches!(
            get_import_job_backend(&f.backend, Uuid::now_v7(), f.user, credential, job.id)
                .await
                .unwrap(),
            Err(ImportDbError::Forbidden)
        ));
        sqlx::query("UPDATE sessions SET revoked_at=1 WHERE id=?1")
            .bind(credential.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            create_sync_import_job_backend(&f.backend, f.workspace, f.user, credential)
                .await
                .unwrap(),
            Err(ImportDbError::Forbidden)
        ));
        assert!(matches!(
            get_import_job_backend(&f.backend, f.workspace, f.user, credential, job.id)
                .await
                .unwrap(),
            Err(ImportDbError::Forbidden)
        ));
        let fresh = session(&f).await;
        sqlx::query("UPDATE memberships SET role='member' WHERE workspace_id=?1 AND user_id=?2")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            create_sync_import_job_backend(&f.backend, f.workspace, f.user, fresh)
                .await
                .unwrap(),
            Err(ImportDbError::Forbidden)
        ));
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM import_jobs")
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
        f.close().await;
    }

    #[tokio::test]
    async fn import_selected_refs_deferred_events_atomic_terminal_fresh_client() {
        let f = Fixture::new().await;
        let credential = session(&f).await;
        let job = queued(&f, credential, ImportSource::NotionZip).await;
        let c = claim(&f).await;
        refs(&f, &c, f.document, None).await;
        parked(&f, &c).await;
        assert!(
            finish_import_job_backend(&f.backend, &c, ImportStatus::Completed)
                .await
                .unwrap()
        );
        assert!(
            !finish_import_job_backend(&f.backend, &c, ImportStatus::Completed)
                .await
                .unwrap()
        );
        let fresh = crate::db::pool::connect_sqlite_app(&f.path, 1)
            .await
            .unwrap();
        let fk: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(&fresh)
            .await
            .unwrap();
        assert_eq!(fk, 1);
        let new_credential = session(&f).await;
        let observed = get_import_job_backend(
            &Backend::Sqlite(fresh.clone()),
            f.workspace,
            f.user,
            new_credential,
            job.id,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(observed.status, ImportStatus::Completed);
        assert_eq!(observed.created_refs.document_ids, vec![f.document]);
        let events: Vec<(String, String)> =
            sqlx::query_as("SELECT verb,payload FROM events ORDER BY seq")
                .fetch_all(&fresh)
                .await
                .unwrap();
        assert_eq!(
            events.iter().map(|e| e.0.as_str()).collect::<Vec<_>>(),
            vec!["import.first", "import.second"]
        );
        for (_, payload) in events {
            assert_eq!(
                serde_json::from_str::<Value>(&payload).unwrap(),
                json!({"literal":"한글 import receipt"})
            );
        }
        let clear:(Option<Vec<u8>>,Option<Vec<u8>>,Option<i64>,i64)=sqlx::query_as("SELECT payload,lease_token,lease_until,(SELECT count(*) FROM import_deferred_events) FROM import_jobs WHERE id=?1").bind(job.id.as_bytes().as_slice()).fetch_one(&fresh).await.unwrap();
        assert_eq!(clear, (None, None, None, 0));
        fresh.close().await;
        f.close().await;
    }

    #[tokio::test]
    async fn import_selected_late_revoke_denies_terminal_events_then_explicit_retry() {
        let f = Fixture::new().await;
        let credential = session(&f).await;
        queued(&f, credential, ImportSource::NotionZip).await;
        let c = claim(&f).await;
        parked(&f, &c).await;
        sqlx::query("UPDATE sessions SET revoked_at=1 WHERE id=?1")
            .bind(credential.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(
            !finish_import_job_backend(&f.backend, &c, ImportStatus::Completed)
                .await
                .unwrap()
        );
        let counts: (i64, i64) = sqlx::query_as(
            "SELECT (SELECT count(*) FROM events),(SELECT count(*) FROM import_deferred_events)",
        )
        .fetch_one(&f.pool)
        .await
        .unwrap();
        assert_eq!(counts, (0, 2));
        sqlx::query("UPDATE sessions SET revoked_at=NULL WHERE id=?1")
            .bind(credential.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(
            finish_import_job_backend(&f.backend, &c, ImportStatus::Completed)
                .await
                .unwrap()
        );
        f.close().await;
    }

    #[tokio::test]
    async fn import_selected_actual_deferred_fk_unknown_rolls_back_job_and_events() {
        let f = Fixture::new().await;
        let credential = session(&f).await;
        queued(&f, credential, ImportSource::NotionZip).await;
        let c = claim(&f).await;
        parked(&f, &c).await;
        let mut tx = f.backend.begin_write().await.unwrap();
        let mut op = tx.operation();
        op.set_tenant(f.workspace).await.unwrap();
        assert!(op
            .finish_import_job(&c, ImportStatus::Completed)
            .await
            .unwrap());
        let OperationTx::SqliteFamily(family) = &mut op else {
            unreachable!()
        };
        family
            .execute("PRAGMA defer_foreign_keys=ON", &[])
            .await
            .unwrap();
        family
            .execute(
                "UPDATE import_jobs SET created_by=?2 WHERE id=?1",
                &[Cell::uuid(c.job_id), Cell::uuid(Uuid::now_v7())],
            )
            .await
            .unwrap();
        let unknown = tx.commit().await.unwrap_err();
        assert!(unknown.source.as_database_error().is_some());
        let error = unknown_commit(unknown);
        assert!(
            matches!(&error,sqlx::Error::AnyDriverError(source) if source.is::<crate::db::backend::CommitUnknown>())
        );
        let row = get_import_job_backend(&f.backend, f.workspace, f.user, credential, c.job_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.status, ImportStatus::Running);
        let counts: (i64, i64) = sqlx::query_as(
            "SELECT (SELECT count(*) FROM events),(SELECT count(*) FROM import_deferred_events)",
        )
        .fetch_one(&f.pool)
        .await
        .unwrap();
        assert_eq!(counts, (0, 2));
        assert!(
            finish_import_job_backend(&f.backend, &c, ImportStatus::Completed)
                .await
                .unwrap()
        );
        f.close().await;
    }

    #[tokio::test]
    async fn import_selected_compensation_fk_partial_failure_preserves_literal_then_retry() {
        let f = Fixture::new().await;
        let credential = session(&f).await;
        queued(&f, credential, ImportSource::NotionZip).await;
        let c = claim(&f).await;
        let (_attachment, key) = f.attachment(31, "text/plain").await;
        let storage = crate::attachments::ObjectStorage::local(f.root.join("import-storage"));
        let literal = "physical compensation body 한글".as_bytes().to_vec();
        storage.put_bytes(&key, literal.clone()).await.unwrap();
        refs(&f, &c, f.document, Some(&key)).await;
        let child = Uuid::now_v7();
        sqlx::query("INSERT INTO documents(id,workspace_id,title,path,parent_id,sort_key,number,created_by) VALUES(?1,?2,'foreign child',?3,?4,'W',2,?5)").bind(child.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(child.simple().to_string()).bind(f.document.as_bytes().as_slice()).bind(f.user.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        let durable = get_import_job_backend(&f.backend, f.workspace, f.user, credential, c.job_id)
            .await
            .unwrap()
            .unwrap()
            .created_refs;
        assert!(compensate_family_import(
            &f.backend,
            &storage,
            ImportCleanupOwner::Runner(&c),
            &durable,
            &CancellationToken::new()
        )
        .await
        .is_err());
        assert_eq!(
            std::fs::read(
                f.root
                    .join("import-storage/objects")
                    .join(&key)
                    .join("payload")
            )
            .unwrap(),
            literal
        );
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM attachments WHERE storage_key=?1")
                .bind(&key)
                .fetch_one(&f.pool)
                .await
                .unwrap();
        assert_eq!(count, 1);
        sqlx::query("DELETE FROM documents WHERE id=?1")
            .bind(child.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let cancelled = CancellationToken::new();
        cancelled.cancel();
        assert!(compensate_family_import(
            &f.backend,
            &storage,
            ImportCleanupOwner::Runner(&c),
            &durable,
            &cancelled
        )
        .await
        .is_err());
        assert_eq!(
            std::fs::read(
                f.root
                    .join("import-storage/objects")
                    .join(&key)
                    .join("payload")
            )
            .unwrap(),
            literal
        );
        let outcome = compensate_family_import(
            &f.backend,
            &storage,
            ImportCleanupOwner::Runner(&c),
            &durable,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(outcome.failed, 0);
        assert!(storage.head(&key).await.unwrap().is_none());
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM documents WHERE id=?1")
            .bind(f.document.as_bytes().as_slice())
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
        assert!(reset_import_refs_backend(&f.backend, &c).await.unwrap());
        assert!(
            get_import_job_backend(&f.backend, f.workspace, f.user, credential, c.job_id)
                .await
                .unwrap()
                .unwrap()
                .created_refs
                .is_empty()
        );
        f.close().await;
    }

    #[tokio::test]
    async fn import_selected_cleanup_wrong_proof_shared_key_and_expired_sweep() {
        let f = Fixture::new().await;
        let credential = session(&f).await;
        queued(&f, credential, ImportSource::OfficeFile).await;
        let c = claim(&f).await;
        let (_, key) = f.attachment(8, "text/plain").await;
        let storage = crate::attachments::ObjectStorage::local(f.root.join("import-storage"));
        storage
            .put_bytes(&key, b"retained shared bytes".to_vec())
            .await
            .unwrap();
        let forged = ImportJobRefs {
            document_ids: vec![f.document],
            task_ids: vec![],
            stored_keys: vec![key.clone()],
        };
        assert!(compensate_family_import(
            &f.backend,
            &storage,
            ImportCleanupOwner::Runner(&c),
            &forged,
            &CancellationToken::new()
        )
        .await
        .is_err());
        assert!(storage.head(&key).await.unwrap().is_some());
        let mut tx = f.backend.begin_write().await.unwrap();
        let mut op = tx.operation();
        op.set_tenant(f.workspace).await.unwrap();
        assert!(op
            .append_import_ref(
                f.workspace,
                crate::db::documents::ImportFence {
                    job_id: c.job_id,
                    lease_token: c.lease_token
                },
                ImportRefKind::StoredKey,
                &key
            )
            .await
            .unwrap());
        tx.commit().await.unwrap();
        let only_key = ImportJobRefs {
            stored_keys: vec![key.clone()],
            ..Default::default()
        };
        assert!(compensate_family_import(
            &f.backend,
            &storage,
            ImportCleanupOwner::Runner(&c),
            &only_key,
            &CancellationToken::new()
        )
        .await
        .is_err());
        assert!(storage.head(&key).await.unwrap().is_some());
        let sync = create_sync_import_job_backend(&f.backend, f.workspace, f.user, credential)
            .await
            .unwrap()
            .unwrap();
        sqlx::query("UPDATE import_jobs SET updated_at=1 WHERE id=?1")
            .bind(sync.id.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(
            fail_stale_sync_import_jobs_backend(&f.backend)
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            fail_stale_sync_import_jobs_backend(&f.backend)
                .await
                .unwrap(),
            0
        );
        sqlx::query("UPDATE import_jobs SET lease_until=1 WHERE id=?1")
            .bind(c.job_id.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let expired = claim_expired_import_job_backend(&f.backend)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(expired.created_refs.stored_keys, vec![key.clone()]);
        assert!(!extend_import_lease_backend(&f.backend, &c).await.unwrap());
        assert!(compensate_family_import(
            &f.backend,
            &storage,
            ImportCleanupOwner::Expired(&expired),
            &only_key,
            &CancellationToken::new()
        )
        .await
        .is_err());
        assert!(storage.head(&key).await.unwrap().is_some());
        f.close().await;
    }
    async fn daily(f: &Fixture) -> crate::db::maintenance_claim::FamilyMaintenanceClaim {
        use crate::db::maintenance_claim::{
            FamilyClaimAcquisition, FamilyMaintenanceClaimRequest, FamilyMaintenanceLeasePolicy,
            MaintenanceJobKey,
        };
        let policy = FamilyMaintenanceLeasePolicy::new(
            std::time::Duration::from_secs(60),
            std::time::Duration::from_secs(10),
        )
        .unwrap();
        match FamilyMaintenanceClaimRequest::new(MaintenanceJobKey::Daily)
            .try_acquire(&f.backend, policy, &CancellationToken::new())
            .await
            .unwrap()
        {
            FamilyClaimAcquisition::Acquired(claim) => claim,
            _ => panic!("actual Daily claim required"),
        }
    }
    async fn failed_with_refs(f: &Fixture, key: &str) -> ImportClaim {
        let credential = session(f).await;
        queued(f, credential, ImportSource::NotionZip).await;
        let c = claim(f).await;
        refs(f, &c, f.document, Some(key)).await;
        assert!(
            finish_import_job_backend(&f.backend, &c, ImportStatus::Failed)
                .await
                .unwrap()
        );
        c
    }
    #[tokio::test]
    async fn import_selected_sync_document_ref_same_writer_current_owner_and_decode_errors() {
        let f = Fixture::new().await;
        let credential = session(&f).await;
        let job = create_sync_import_job_backend(&f.backend, f.workspace, f.user, credential)
            .await
            .unwrap()
            .unwrap();
        let mut tx = f.backend.begin_write().await.unwrap();
        let mut op = tx.operation();
        op.set_tenant(f.workspace).await.unwrap();
        assert!(!op
            .sync_import_contains_document_ref(f.workspace, job.id, f.user, f.document)
            .await
            .unwrap());
        assert!(op
            .append_sync_import_document_ref(f.workspace, job.id, f.user, f.document)
            .await
            .unwrap());
        assert!(op
            .sync_import_contains_document_ref(f.workspace, job.id, f.user, f.document)
            .await
            .unwrap());
        assert!(!op
            .sync_import_contains_document_ref(f.workspace, job.id, f.user, Uuid::now_v7())
            .await
            .unwrap());
        assert!(!op
            .sync_import_contains_document_ref(f.workspace, job.id, Uuid::now_v7(), f.document)
            .await
            .unwrap());
        assert!(!op
            .sync_import_contains_document_ref(f.workspace, Uuid::now_v7(), f.user, f.document)
            .await
            .unwrap());
        assert!(matches!(
            op.sync_import_contains_document_ref(Uuid::now_v7(), job.id, f.user, f.document)
                .await,
            Err(sqlx::Error::Protocol(_))
        ));
        let OperationTx::SqliteFamily(family) = &mut op else {
            unreachable!()
        };
        family
            .execute(
                "UPDATE import_jobs SET created_refs=?2 WHERE id=?1",
                &[
                    Cell::uuid(job.id),
                    Cell::text(r#"{"documentIds":["not-a-uuid"],"taskIds":[],"storedKeys":[]}"#),
                ],
            )
            .await
            .unwrap();
        assert!(matches!(
            op.sync_import_contains_document_ref(f.workspace, job.id, f.user, f.document)
                .await,
            Err(sqlx::Error::Decode(_))
        ));
        tx.rollback().await.unwrap();
        let mut tx = f.backend.begin_write().await.unwrap();
        tx.operation().set_tenant(f.workspace).await.unwrap();
        assert!(!tx
            .operation()
            .sync_import_contains_document_ref(f.workspace, job.id, f.user, f.document)
            .await
            .unwrap());
        assert!(tx
            .operation()
            .append_sync_import_document_ref(f.workspace, job.id, f.user, f.document)
            .await
            .unwrap());
        assert!(tx
            .operation()
            .sync_import_contains_document_ref(f.workspace, job.id, f.user, f.document)
            .await
            .unwrap());
        tx.commit().await.unwrap();
        let mut read = f.backend.begin_read().await.unwrap();
        read.operation().set_tenant(f.workspace).await.unwrap();
        assert!(matches!(
            read.operation()
                .sync_import_contains_document_ref(f.workspace, job.id, f.user, f.document)
                .await,
            Err(sqlx::Error::Protocol(_))
        ));
        read.rollback().await.unwrap();
        assert!(finish_sync_import_job_backend(
            &f.backend,
            f.workspace,
            job.id,
            f.user,
            credential,
            ImportStatus::Failed
        )
        .await
        .unwrap());
        let mut tx = f.backend.begin_write().await.unwrap();
        tx.operation().set_tenant(f.workspace).await.unwrap();
        assert!(!tx
            .operation()
            .sync_import_contains_document_ref(f.workspace, job.id, f.user, f.document)
            .await
            .unwrap());
        tx.rollback().await.unwrap();
        let after = get_import_job_backend(&f.backend, f.workspace, f.user, credential, job.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(after.created_refs.document_ids, vec![f.document]);
        let events: i64 = sqlx::query_scalar("SELECT count(*) FROM events")
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(events, 0);
        f.close().await;
    }

    #[tokio::test]
    async fn import_selected_daily_retains_actual_failed_sync_document_and_refs() {
        for stale in [false, true] {
            let f = Fixture::new().await;
            let literal = b"actual failed sync retained bytes";
            let (_, key) = f.attachment(literal.len() as i64, "text/plain").await;
            let storage =
                crate::attachments::ObjectStorage::local(f.root.join("actual-sync-retention"));
            storage.put_bytes(&key, literal.to_vec()).await.unwrap();
            let credential = session(&f).await;
            let job = create_sync_import_job_backend(&f.backend, f.workspace, f.user, credential)
                .await
                .unwrap()
                .unwrap();
            let mut tx = f.backend.begin_write().await.unwrap();
            tx.operation().set_tenant(f.workspace).await.unwrap();
            assert!(tx
                .operation()
                .append_sync_import_document_ref(f.workspace, job.id, f.user, f.document)
                .await
                .unwrap());
            tx.commit().await.unwrap();
            let content = r#"{"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"retained synchronous body"}]}]}"#;
            sqlx::query("UPDATE documents SET content_json=?2,text=?3 WHERE id=?1")
                .bind(f.document.as_bytes().as_slice())
                .bind(content)
                .bind("retained synchronous body")
                .execute(&f.pool)
                .await
                .unwrap();
            let native_state = b"retained synchronous native state";
            sqlx::query(
                "INSERT INTO document_states(workspace_id,document_id,state) VALUES(?1,?2,?3)",
            )
            .bind(f.workspace.as_bytes().as_slice())
            .bind(f.document.as_bytes().as_slice())
            .bind(native_state.as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
            let revision = Uuid::now_v7();
            sqlx::query("INSERT INTO revisions(id,workspace_id,target_kind,target_id,y_snapshot,content_json,text,reason,created_by) VALUES(?1,?2,'document',?3,?4,?5,?6,'manual',?7)")
                .bind(revision.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(f.document.as_bytes().as_slice()).bind(native_state.as_slice()).bind(content).bind("retained synchronous body").bind(f.user.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
            if stale {
                sqlx::query("UPDATE import_jobs SET updated_at=1 WHERE id=?1")
                    .bind(job.id.as_bytes().as_slice())
                    .execute(&f.pool)
                    .await
                    .unwrap();
            } else {
                assert!(finish_sync_import_job_backend(
                    &f.backend,
                    f.workspace,
                    job.id,
                    f.user,
                    credential,
                    ImportStatus::Failed
                )
                .await
                .unwrap());
            }
            let before =
                get_import_job_backend(&f.backend, f.workspace, f.user, credential, job.id)
                    .await
                    .unwrap()
                    .unwrap();
            assert_eq!(before.source, ImportSource::MarkdownZip);
            assert_eq!(
                before.status,
                if stale {
                    ImportStatus::Pending
                } else {
                    ImportStatus::Failed
                }
            );
            assert_eq!(before.created_refs.document_ids, vec![f.document]);
            // A genuine separate failed async obligation in this SAME sweep.
            // It owns a different document/key, so retention cannot be passed
            // by disabling cleanup or by counting unrelated retained refs.
            let async_doc = Uuid::now_v7();
            sqlx::query("INSERT INTO documents(id,workspace_id,title,path,sort_key,number,status,schema_version,created_by,content_json) VALUES(?1,?2,'async partial',?3,'W',2,'published',2,?4,'{\"type\":\"doc\",\"content\":[]}')")
                .bind(async_doc.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(async_doc.simple().to_string()).bind(f.user.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
            let async_bytes = b"eligible async cleanup bytes";
            let (async_attachment, async_key) =
                f.attachment(async_bytes.len() as i64, "text/plain").await;
            sqlx::query("UPDATE attachments SET document_id=?2 WHERE id=?1")
                .bind(async_attachment.as_bytes().as_slice())
                .bind(async_doc.as_bytes().as_slice())
                .execute(&f.pool)
                .await
                .unwrap();
            storage
                .put_bytes(&async_key, async_bytes.to_vec())
                .await
                .unwrap();
            queued(&f, credential, ImportSource::NotionZip).await;
            let async_claim = claim(&f).await;
            refs(&f, &async_claim, async_doc, Some(&async_key)).await;
            assert!(
                finish_import_job_backend(&f.backend, &async_claim, ImportStatus::Failed)
                    .await
                    .unwrap()
            );
            let daily = daily(&f).await;
            assert_eq!(
                crate::import_job::sweep_orphan_imports_with_maintenance_claim_backend(
                    &f.backend,
                    &storage,
                    &CancellationToken::new(),
                    daily.proof(),
                    daily.policy()
                )
                .await
                .unwrap(),
                if stale { 2 } else { 1 }
            );
            let after = get_import_job_backend(&f.backend, f.workspace, f.user, credential, job.id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(after.status, ImportStatus::Failed);
            assert_eq!(after.created_refs, before.created_refs);
            assert_eq!(
                std::fs::read(
                    f.root
                        .join("actual-sync-retention/objects")
                        .join(&key)
                        .join("payload")
                )
                .unwrap(),
                literal
            );
            let retained: (String, String, Vec<u8>) = sqlx::query_as("SELECT d.content_json,d.text,s.state FROM documents d JOIN document_states s ON s.workspace_id=d.workspace_id AND s.document_id=d.id WHERE d.id=?1")
                .bind(f.document.as_bytes().as_slice()).fetch_one(&f.pool).await.unwrap();
            assert_eq!(
                retained,
                (
                    content.into(),
                    "retained synchronous body".into(),
                    native_state.to_vec()
                )
            );
            let history: (Vec<u8>, String, String) =
                sqlx::query_as("SELECT y_snapshot,content_json,text FROM revisions WHERE id=?1")
                    .bind(revision.as_bytes().as_slice())
                    .fetch_one(&f.pool)
                    .await
                    .unwrap();
            assert_eq!(
                history,
                (
                    native_state.to_vec(),
                    content.into(),
                    "retained synchronous body".into()
                )
            );
            let retained_attachment: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM attachments WHERE document_id=?1 AND storage_key=?2",
            )
            .bind(f.document.as_bytes().as_slice())
            .bind(&key)
            .fetch_one(&f.pool)
            .await
            .unwrap();
            assert_eq!(retained_attachment, 1);
            let purged: (i64, i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM documents WHERE id=?1),(SELECT count(*) FROM attachments WHERE id=?2),(SELECT count(*) FROM events WHERE target_id=?3 AND verb='document.purged')")
                .bind(async_doc.as_bytes().as_slice()).bind(async_attachment.as_bytes().as_slice()).bind(f.document.as_bytes().as_slice()).fetch_one(&f.pool).await.unwrap();
            assert_eq!(purged, (0, 0, 0));
            assert!(storage.head(&async_key).await.unwrap().is_none());
            let async_status = get_import_job_backend(
                &f.backend,
                f.workspace,
                f.user,
                credential,
                async_claim.job_id,
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(async_status.status, ImportStatus::Failed);
            assert!(async_status.created_refs.is_empty());
            let async_purged: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM events WHERE target_id=?1 AND verb='document.purged'",
            )
            .bind(async_doc.as_bytes().as_slice())
            .fetch_one(&f.pool)
            .await
            .unwrap();
            assert_eq!(async_purged, 1);
            daily.release().await.unwrap();
            f.close().await;
        }
    }
    #[tokio::test]
    async fn import_selected_daily_retains_failed_markdown_zip_and_rechecks_source() {
        let f = Fixture::new().await;
        let literal = b"retained synchronous import attachment";
        let (_, key) = f.attachment(literal.len() as i64, "text/plain").await;
        let storage = crate::attachments::ObjectStorage::local(f.root.join("sync-retention"));
        storage.put_bytes(&key, literal.to_vec()).await.unwrap();
        let c = failed_with_refs(&f, &key).await;
        let daily = daily(&f).await;
        let context = ImportMaintenanceContext {
            proof: daily.proof(),
            policy: daily.policy(),
        };
        let candidates =
            import_cleanup_candidates_backend(&f.backend, context, &CancellationToken::new())
                .await
                .unwrap();
        assert_eq!(candidates.len(), 1);
        // Real catalog change after candidate observation: mutation must check
        // the source again before row deletion or any physical storage purge.
        sqlx::query("UPDATE import_jobs SET source='markdown-zip' WHERE id=?1")
            .bind(c.job_id.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let seed = b"retained document state bytes";
        sqlx::query("INSERT INTO document_states(workspace_id,document_id,state) VALUES(?1,?2,?3) ON CONFLICT(workspace_id,document_id) DO UPDATE SET state=excluded.state")
            .bind(f.workspace.as_bytes().as_slice()).bind(f.document.as_bytes().as_slice()).bind(seed.as_slice()).execute(&f.pool).await.unwrap();
        let revision = Uuid::now_v7();
        sqlx::query("INSERT INTO revisions(id,workspace_id,target_kind,target_id,y_snapshot,content_json,text,reason,created_by) VALUES(?1,?2,'document',?3,?4,?5,?6,'manual',?7)")
            .bind(revision.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(f.document.as_bytes().as_slice()).bind(seed.as_slice())
            .bind(r#"{"type":"doc","content":[]}"#).bind("retained synchronous history").bind(f.user.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        let before: (String, String, i64, i64) = sqlx::query_as("SELECT status,created_refs,attempts,(SELECT count(*) FROM events) FROM import_jobs WHERE id=?1")
            .bind(c.job_id.as_bytes().as_slice()).fetch_one(&f.pool).await.unwrap();
        assert!(!prepare_import_cleanup_backend(
            &f.backend,
            &candidates[0],
            context,
            &CancellationToken::new()
        )
        .await
        .unwrap());
        assert!(cleanup_failed_import_backend(
            &f.backend,
            &storage,
            &candidates[0],
            context,
            &CancellationToken::new()
        )
        .await
        .is_err());
        assert!(
            import_cleanup_candidates_backend(&f.backend, context, &CancellationToken::new())
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            crate::import_job::sweep_orphan_imports_with_maintenance_claim_backend(
                &f.backend,
                &storage,
                &CancellationToken::new(),
                daily.proof(),
                daily.policy()
            )
            .await
            .unwrap(),
            0
        );
        let after: (String, String, i64, i64) = sqlx::query_as("SELECT status,created_refs,attempts,(SELECT count(*) FROM events) FROM import_jobs WHERE id=?1")
            .bind(c.job_id.as_bytes().as_slice()).fetch_one(&f.pool).await.unwrap();
        assert_eq!(before, after);
        assert_eq!(
            std::fs::read(
                f.root
                    .join("sync-retention/objects")
                    .join(&key)
                    .join("payload")
            )
            .unwrap(),
            literal
        );
        let state: Vec<u8> = sqlx::query_scalar(
            "SELECT state FROM document_states WHERE workspace_id=?1 AND document_id=?2",
        )
        .bind(f.workspace.as_bytes().as_slice())
        .bind(f.document.as_bytes().as_slice())
        .fetch_one(&f.pool)
        .await
        .unwrap();
        assert_eq!(state, seed);
        let history: (Vec<u8>, String, String) =
            sqlx::query_as("SELECT y_snapshot,content_json,text FROM revisions WHERE id=?1")
                .bind(revision.as_bytes().as_slice())
                .fetch_one(&f.pool)
                .await
                .unwrap();
        assert_eq!(
            history,
            (
                seed.to_vec(),
                r#"{"type":"doc","content":[]}"#.into(),
                "retained synchronous history".into()
            )
        );
        let retained: i64 = sqlx::query_scalar("SELECT count(*) FROM attachments WHERE workspace_id=?1 AND document_id=?2 AND storage_key=?3")
            .bind(f.workspace.as_bytes().as_slice()).bind(f.document.as_bytes().as_slice()).bind(&key).fetch_one(&f.pool).await.unwrap();
        assert_eq!(retained, 1);
        // Eligible async jobs remain cleanable; the original cleanup tests also
        // retain their real I/O failure, cancellation and healthy retry oracles.
        sqlx::query("UPDATE import_jobs SET source='notion-zip' WHERE id=?1")
            .bind(c.job_id.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(
            crate::import_job::sweep_orphan_imports_with_maintenance_claim_backend(
                &f.backend,
                &storage,
                &CancellationToken::new(),
                daily.proof(),
                daily.policy()
            )
            .await
            .unwrap(),
            1
        );
        assert!(storage.head(&key).await.unwrap().is_none());
        let remaining: i64 = sqlx::query_scalar("SELECT count(*) FROM documents WHERE id=?1")
            .bind(f.document.as_bytes().as_slice())
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(remaining, 0);
        daily.release().await.unwrap();
        f.close().await;
    }
    #[tokio::test]
    async fn import_selected_failed_cleanup_actual_io_failure_then_automatic_healthy_sweep() {
        let f = Fixture::new().await;
        let (_, key) = f.attachment(31, "text/plain").await;
        let root = f.root.join("failed-import-storage");
        let storage = crate::attachments::ObjectStorage::local(root.clone());
        let literal = b"failed cleanup source literal bytes";
        storage.put_bytes(&key, literal.to_vec()).await.unwrap();
        let c = failed_with_refs(&f, &key).await;
        let daily = daily(&f).await;
        // Real filesystem abort failure: an existing multipart path is a file,
        // so remove_dir_all fails before touching the still-readable object.
        let bad_part = root.join("tmp").join(&key);
        std::fs::create_dir_all(bad_part.parent().unwrap()).unwrap();
        std::fs::write(&bad_part, b"physical multipart obstruction").unwrap();
        assert_eq!(
            crate::import_job::sweep_orphan_imports_with_maintenance_claim_backend(
                &f.backend,
                &storage,
                &CancellationToken::new(),
                daily.proof(),
                daily.policy()
            )
            .await
            .unwrap(),
            1
        );
        assert_eq!(
            std::fs::read(root.join("objects").join(&key).join("payload")).unwrap(),
            literal
        );
        let state:(String,i64,i64,i64)=sqlx::query_as("SELECT status,attempts,(SELECT count(*) FROM documents WHERE id=?2),json_array_length(created_refs,'$.storedKeys') FROM import_jobs WHERE id=?1").bind(c.job_id.as_bytes().as_slice()).bind(f.document.as_bytes().as_slice()).fetch_one(&f.pool).await.unwrap();
        assert_eq!(state, ("failed".into(), 1, 1, 1));
        std::fs::remove_file(&bad_part).unwrap();
        assert_eq!(
            crate::import_job::sweep_orphan_imports_with_maintenance_claim_backend(
                &f.backend,
                &storage,
                &CancellationToken::new(),
                daily.proof(),
                daily.policy()
            )
            .await
            .unwrap(),
            1
        );
        assert!(storage.head(&key).await.unwrap().is_none());
        let state:(String,i64,i64,String,Option<Vec<u8>>)=sqlx::query_as("SELECT status,attempts,(SELECT count(*) FROM documents WHERE id=?2),created_refs,payload FROM import_jobs WHERE id=?1").bind(c.job_id.as_bytes().as_slice()).bind(f.document.as_bytes().as_slice()).fetch_one(&f.pool).await.unwrap();
        assert_eq!(
            (&state.0, state.1, state.2, state.4.as_ref()),
            (&"failed".to_owned(), 1, 0, None)
        );
        assert!(serde_json::from_str::<ImportJobRefs>(&state.3)
            .unwrap()
            .is_empty());
        assert_eq!(
            crate::import_job::sweep_orphan_imports_with_maintenance_claim_backend(
                &f.backend,
                &storage,
                &CancellationToken::new(),
                daily.proof(),
                daily.policy()
            )
            .await
            .unwrap(),
            0
        );
        daily.release().await.unwrap();
        f.close().await;
    }
    #[tokio::test]
    async fn import_selected_failed_cleanup_current_refs_old_daily_and_cancel_protect_replacement()
    {
        let f = Fixture::new().await;
        let (_, key) = f.attachment(17, "text/plain").await;
        let storage = crate::attachments::ObjectStorage::local(f.root.join("failed-ref-storage"));
        storage
            .put_bytes(&key, b"retained replacement".to_vec())
            .await
            .unwrap();
        let c = failed_with_refs(&f, &key).await;
        let daily = daily(&f).await;
        let context = ImportMaintenanceContext {
            proof: daily.proof(),
            policy: daily.policy(),
        };
        let jobs =
            import_cleanup_candidates_backend(&f.backend, context, &CancellationToken::new())
                .await
                .unwrap();
        assert_eq!(jobs.len(), 1);
        let mut changed = jobs[0].created_refs.clone();
        changed.stored_keys.push(Uuid::now_v7().to_string());
        sqlx::query("UPDATE import_jobs SET created_refs=?2 WHERE id=?1")
            .bind(c.job_id.as_bytes().as_slice())
            .bind(serde_json::to_string(&changed).unwrap())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(!prepare_import_cleanup_backend(
            &f.backend,
            &jobs[0],
            context,
            &CancellationToken::new()
        )
        .await
        .unwrap());
        assert!(cleanup_failed_import_backend(
            &f.backend,
            &storage,
            &jobs[0],
            context,
            &CancellationToken::new()
        )
        .await
        .is_err());
        assert!(storage.head(&key).await.unwrap().is_some());
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert_eq!(
            crate::import_job::sweep_orphan_imports_with_maintenance_claim_backend(
                &f.backend,
                &storage,
                &cancel,
                daily.proof(),
                daily.policy()
            )
            .await
            .unwrap(),
            0
        );
        let mut tx = f.backend.begin_write().await.unwrap();
        let previous = tx.operation().set_system().await.unwrap();
        // Actual current claim revocation in the catalog, no fake token/proof.
        let mut op = tx.operation();
        let OperationTx::SqliteFamily(family) = &mut op else {
            unreachable!()
        };
        family.execute("UPDATE maintenance_job_claims SET owner_token=NULL,expires_at=NULL WHERE job_key=1", &[]).await.unwrap();
        op.restore_system(previous).await.unwrap();
        tx.commit().await.unwrap();
        assert!(
            crate::import_job::sweep_orphan_imports_with_maintenance_claim_backend(
                &f.backend,
                &storage,
                &CancellationToken::new(),
                daily.proof(),
                daily.policy()
            )
            .await
            .is_err()
        );
        assert!(storage.head(&key).await.unwrap().is_some());
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM documents WHERE id=?1")
            .bind(f.document.as_bytes().as_slice())
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
        f.close().await;
    }
    #[tokio::test]
    async fn import_selected_failed_cleanup_late_cancel_after_real_purge_keeps_refs_for_automatic_retry(
    ) {
        let f = Fixture::new().await;
        let (_, key) = f.attachment(19, "text/plain").await;
        let storage =
            crate::attachments::ObjectStorage::local(f.root.join("failed-cancel-storage"));
        storage
            .put_bytes(&key, b"purge actually happened".to_vec())
            .await
            .unwrap();
        let c = failed_with_refs(&f, &key).await;
        let daily = daily(&f).await;
        let pause = std::sync::Arc::new((tokio::sync::Notify::new(), tokio::sync::Notify::new()));
        let cancel = CancellationToken::new();
        let sweep = IMPORT_CLEANUP_AFTER_PURGE.scope(
            pause.clone(),
            crate::import_job::sweep_orphan_imports_with_maintenance_claim_backend(
                &f.backend,
                &storage,
                &cancel,
                daily.proof(),
                daily.policy(),
            ),
        );
        let control = async {
            pause.0.notified().await;
            cancel.cancel();
            pause.1.notify_one();
        };
        let (result, ()) = tokio::join!(sweep, control);
        assert_eq!(result.unwrap(), 1);
        assert!(storage.head(&key).await.unwrap().is_none());
        let state:(i64,String)=sqlx::query_as("SELECT (SELECT count(*) FROM documents WHERE id=?2),created_refs FROM import_jobs WHERE id=?1").bind(c.job_id.as_bytes().as_slice()).bind(f.document.as_bytes().as_slice()).fetch_one(&f.pool).await.unwrap();
        assert_eq!(state.0, 1);
        assert!(!serde_json::from_str::<ImportJobRefs>(&state.1)
            .unwrap()
            .is_empty());
        assert_eq!(
            crate::import_job::sweep_orphan_imports_with_maintenance_claim_backend(
                &f.backend,
                &storage,
                &CancellationToken::new(),
                daily.proof(),
                daily.policy()
            )
            .await
            .unwrap(),
            1
        );
        assert_eq!(
            crate::import_job::sweep_orphan_imports_with_maintenance_claim_backend(
                &f.backend,
                &storage,
                &CancellationToken::new(),
                daily.proof(),
                daily.policy()
            )
            .await
            .unwrap(),
            0
        );
        daily.release().await.unwrap();
        f.close().await;
    }
    #[tokio::test]
    async fn import_selected_full_claim_identity_and_exact_ref_membership_same_writer() {
        let f = Fixture::new().await;
        let credential = session(&f).await;
        queued(&f, credential, ImportSource::NotionZip).await;
        let c = claim(&f).await;
        let key = Uuid::now_v7().to_string();
        refs(&f, &c, f.document, Some(&key)).await;
        let mut tx = f.backend.begin_write().await.unwrap();
        let mut op = tx.operation();
        op.set_tenant(f.workspace).await.unwrap();
        assert!(op.hold_import_claim(&c).await.unwrap());
        assert!(op
            .import_claim_contains_ref(&c, ImportRefKind::Document, &f.document.to_string())
            .await
            .unwrap());
        assert!(op
            .import_claim_contains_ref(&c, ImportRefKind::StoredKey, &key)
            .await
            .unwrap());
        assert!(!op
            .import_claim_contains_ref(&c, ImportRefKind::Task, &f.document.to_string())
            .await
            .unwrap());
        assert!(!op
            .import_claim_contains_ref(&c, ImportRefKind::Document, "invalid uuid")
            .await
            .unwrap());
        assert!(!op
            .import_claim_contains_ref(&c, ImportRefKind::StoredKey, &Uuid::now_v7().to_string())
            .await
            .unwrap());
        let mut changed = Vec::new();
        let mut wrong = c.clone();
        wrong.created_by = Uuid::now_v7();
        changed.push(wrong);
        let mut wrong = c.clone();
        wrong.session_id = Uuid::now_v7();
        changed.push(wrong);
        let mut wrong = c.clone();
        wrong.source = ImportSource::OfficeFile;
        changed.push(wrong);
        let mut wrong = c.clone();
        wrong.project_id = Some(Uuid::now_v7());
        changed.push(wrong);
        let mut wrong = c.clone();
        wrong.attempt += 1;
        changed.push(wrong);
        let mut wrong = c.clone();
        wrong.lease_token = Uuid::now_v7();
        changed.push(wrong);
        let mut wrong = c.clone();
        wrong.job_id = Uuid::now_v7();
        changed.push(wrong);
        for wrong in changed {
            assert!(!op.hold_import_claim(&wrong).await.unwrap());
            assert!(!op
                .import_claim_contains_ref(&wrong, ImportRefKind::StoredKey, &key)
                .await
                .unwrap());
        }
        let fence = crate::db::documents::ImportFence {
            job_id: c.job_id,
            lease_token: c.lease_token,
        };
        let mut forged = event(&f, "forged.actor");
        forged.actor_user_id = Some(Uuid::now_v7());
        assert!(!op
            .park_import_event(f.workspace, fence, forged, "web")
            .await
            .unwrap());
        tx.rollback().await.unwrap();
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM import_deferred_events")
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
        f.close().await;
    }
    #[tokio::test]
    async fn import_selected_cleanup_cross_tenant_canonical_preview_retains_bytes_then_healthy_sweep(
    ) {
        let f = Fixture::new().await;
        let preview_bytes = b"literal retained other tenant preview";
        let original_bytes = b"literal unrelated original payload";
        let (_, key) = f.attachment(preview_bytes.len() as i64, "text/plain").await;
        let (other_attachment, other_key) = f
            .attachment(original_bytes.len() as i64, "image/webp")
            .await;
        let other_workspace = Uuid::now_v7();
        let other_document = Uuid::now_v7();
        sqlx::query("INSERT INTO workspaces(id,slug,name) VALUES(?1,'import-preview-other','Other preview workspace')").bind(other_workspace.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        sqlx::query("INSERT INTO documents(id,workspace_id,title,path,sort_key,number,status,schema_version,created_by,content_json) VALUES(?1,?2,'Other live preview',?3,'V',1,'published',?4,?5,?6)").bind(other_document.as_bytes().as_slice()).bind(other_workspace.as_bytes().as_slice()).bind(other_document.simple().to_string()).bind(crate::db::documents::DOCUMENT_SCHEMA_VERSION).bind(f.user.as_bytes().as_slice()).bind(serde_json::to_string(&crate::db::documents::empty_document_json()).unwrap()).execute(&f.pool).await.unwrap();
        // Literal maintained variant shape from attachment_preview publication:
        // key, width, height, bytes. There is no preview.storageKey field.
        let preview =
            json!({"preview":{"key":key,"width":1,"height":1,"bytes":preview_bytes.len()}});
        sqlx::query("UPDATE attachments SET workspace_id=?2,document_id=?3,variants=?4,preview_status='ok' WHERE id=?1").bind(other_attachment.as_bytes().as_slice()).bind(other_workspace.as_bytes().as_slice()).bind(other_document.as_bytes().as_slice()).bind(serde_json::to_string(&preview).unwrap()).execute(&f.pool).await.unwrap();
        let root = f.root.join("cross-tenant-import-storage");
        let storage = crate::attachments::ObjectStorage::local(root.clone());
        storage
            .put_bytes(&key, preview_bytes.to_vec())
            .await
            .unwrap();
        storage
            .put_bytes(&other_key, original_bytes.to_vec())
            .await
            .unwrap();
        let c = failed_with_refs(&f, &key).await;
        let daily = daily(&f).await;
        let context = ImportMaintenanceContext {
            proof: daily.proof(),
            policy: daily.policy(),
        };
        let jobs =
            import_cleanup_candidates_backend(&f.backend, context, &CancellationToken::new())
                .await
                .unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].job_id, c.job_id);
        assert!(cleanup_failed_import_backend(
            &f.backend,
            &storage,
            &jobs[0],
            context,
            &CancellationToken::new()
        )
        .await
        .is_err());
        assert_eq!(
            std::fs::read(root.join("objects").join(&key).join("payload")).unwrap(),
            preview_bytes
        );
        assert_eq!(
            std::fs::read(root.join("objects").join(&other_key).join("payload")).unwrap(),
            original_bytes
        );
        let state:(i64,String)=sqlx::query_as("SELECT (SELECT count(*) FROM documents WHERE id=?2),created_refs FROM import_jobs WHERE id=?1").bind(c.job_id.as_bytes().as_slice()).bind(f.document.as_bytes().as_slice()).fetch_one(&f.pool).await.unwrap();
        assert_eq!(state.0, 1);
        assert_eq!(
            serde_json::from_str::<ImportJobRefs>(&state.1).unwrap(),
            jobs[0].created_refs
        );
        let current: (Vec<u8>, String) =
            sqlx::query_as("SELECT workspace_id,variants FROM attachments WHERE id=?1")
                .bind(other_attachment.as_bytes().as_slice())
                .fetch_one(&f.pool)
                .await
                .unwrap();
        assert_eq!(current.0, other_workspace.as_bytes());
        assert_eq!(serde_json::from_str::<Value>(&current.1).unwrap(), preview);
        // Releasing only the unrelated fixture preview pointer makes the key
        // unreferenced. The real automatic sweep now cleans the failed job.
        sqlx::query("UPDATE attachments SET variants=json_remove(variants,'$.preview'),preview_status='skipped' WHERE id=?1").bind(other_attachment.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        assert_eq!(
            crate::import_job::sweep_orphan_imports_with_maintenance_claim_backend(
                &f.backend,
                &storage,
                &CancellationToken::new(),
                daily.proof(),
                daily.policy()
            )
            .await
            .unwrap(),
            1
        );
        assert!(storage.head(&key).await.unwrap().is_none());
        assert_eq!(
            std::fs::read(root.join("objects").join(&other_key).join("payload")).unwrap(),
            original_bytes
        );
        let state:(String,i64,i64,String)=sqlx::query_as("SELECT status,attempts,(SELECT count(*) FROM documents WHERE id=?2),created_refs FROM import_jobs WHERE id=?1").bind(c.job_id.as_bytes().as_slice()).bind(f.document.as_bytes().as_slice()).fetch_one(&f.pool).await.unwrap();
        assert_eq!((state.0, state.1, state.2), ("failed".into(), 1, 0));
        assert!(serde_json::from_str::<ImportJobRefs>(&state.3)
            .unwrap()
            .is_empty());
        let count:i64=sqlx::query_scalar("SELECT count(*) FROM attachments WHERE workspace_id=?1 AND id=?2 AND storage_key=?3 AND status='stored'").bind(other_workspace.as_bytes().as_slice()).bind(other_attachment.as_bytes().as_slice()).bind(&other_key).fetch_one(&f.pool).await.unwrap();
        assert_eq!(count, 1);
        assert_eq!(
            crate::import_job::sweep_orphan_imports_with_maintenance_claim_backend(
                &f.backend,
                &storage,
                &CancellationToken::new(),
                daily.proof(),
                daily.policy()
            )
            .await
            .unwrap(),
            0
        );
        daily.release().await.unwrap();
        f.close().await;
    }
}
