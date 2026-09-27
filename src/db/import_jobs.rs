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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportSource {
    MarkdownZip,
    OfficeFile,
    NotionZip,
}

impl ImportSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MarkdownZip => "markdown-zip",
            Self::OfficeFile => "office-file",
            Self::NotionZip => "notion-zip",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
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
    .execute(&mut *tx)
    .await?;
    if result.rows_affected() != 1 {
        // Fence lost: the sweep owns the row and its parked events.
        tx.rollback().await?;
        return Ok(false);
    }
    if status == ImportStatus::Completed {
        publish_deferred_events(&mut tx, claim.workspace_id, claim.job_id).await?;
    } else {
        discard_deferred_events(&mut tx, claim.workspace_id, claim.job_id).await?;
    }
    tx.commit().await?;
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
