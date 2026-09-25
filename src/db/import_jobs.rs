use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::db::context::{lock_membership_users, recheck_session, set_tenant};
use crate::db::workspace::{membership_role, WorkspaceRole};

pub const IMPORT_LEASE_SECS: i64 = 15 * 60;
pub const IMPORT_HTTP_MAX_BYTES: usize = 64 * 1024 * 1024;

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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ImportJobRefs {
    pub document_ids: Vec<Uuid>,
    pub task_ids: Vec<Uuid>,
    pub stored_keys: Vec<String>,
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
    InvalidInput,
}

pub async fn require_import_admin(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<(), ImportDbError> {
    lock_membership_users(tx, &[actor_user_id])
        .await
        .map_err(|_| ImportDbError::Forbidden)?;
    if !recheck_session(tx, actor_user_id, session_id)
        .await
        .map_err(|_| ImportDbError::Forbidden)?
    {
        return Err(ImportDbError::Forbidden);
    }
    let role = membership_role(tx, workspace_id, actor_user_id)
        .await
        .map_err(|_| ImportDbError::Forbidden)?;
    if !role.is_some_and(|r| r.at_least(WorkspaceRole::Admin)) {
        return Err(ImportDbError::Forbidden);
    }
    Ok(())
}

pub async fn create_import_job(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    source: ImportSource,
    status: ImportStatus,
) -> Result<Result<ImportJobRow, ImportDbError>, sqlx::Error> {
    let id = Uuid::now_v7();
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if let Err(err) = require_import_admin(&mut tx, workspace_id, actor_user_id, session_id).await {
        tx.rollback().await?;
        return Ok(Err(err));
    }
    sqlx::query(
        r#"
        INSERT INTO fvoci.import_jobs (
            id, workspace_id, created_by, source, status
        ) VALUES ($1, $2, $3, $4, $5)
        "#,
    )
    .bind(id)
    .bind(workspace_id)
    .bind(actor_user_id)
    .bind(source.as_str())
    .bind(status.as_str())
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Ok(ImportJobRow {
        id,
        workspace_id,
        source,
        status,
        created_refs: ImportJobRefs::default(),
    }))
}

pub async fn update_import_job_status(
    pool: &PgPool,
    workspace_id: Uuid,
    job_id: Uuid,
    status: ImportStatus,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        r#"
        UPDATE fvoci.import_jobs
        SET status = $3, updated_at = now()
        WHERE workspace_id = $1 AND id = $2
        "#,
    )
    .bind(workspace_id)
    .bind(job_id)
    .bind(status.as_str())
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn save_import_progress(
    pool: &PgPool,
    workspace_id: Uuid,
    job_id: Uuid,
    refs: &ImportJobRefs,
) -> Result<bool, sqlx::Error> {
    let lease_until = Utc::now() + chrono::Duration::seconds(IMPORT_LEASE_SECS);
    let refs_json = json!({
        "documentIds": refs.document_ids,
        "taskIds": refs.task_ids,
        "storedKeys": refs.stored_keys,
    });
    let result = sqlx::query(
        r#"
        UPDATE fvoci.import_jobs
        SET created_refs = $3::jsonb,
            lease_until = $4,
            updated_at = now()
        WHERE workspace_id = $1 AND id = $2 AND status = 'running'
        "#,
    )
    .bind(workspace_id)
    .bind(job_id)
    .bind(refs_json)
    .bind(lease_until)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
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
    if let Err(err) = require_import_admin(&mut tx, workspace_id, actor_user_id, session_id).await {
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
    let source = match ImportSource::parse(&source) {
        Some(v) => v,
        None => return Ok(Err(ImportDbError::NotFound)),
    };
    let status = match ImportStatus::parse(&status) {
        Some(v) => v,
        None => return Ok(Err(ImportDbError::NotFound)),
    };
    let created_refs: ImportJobRefs = serde_json::from_value(refs).unwrap_or_default();
    Ok(Ok(ImportJobRow {
        id: job_id,
        workspace_id,
        source,
        status,
        created_refs,
    }))
}

pub async fn claim_import_job(
    pool: &PgPool,
    workspace_id: Uuid,
    job_id: Uuid,
) -> Result<Option<ImportJobRow>, sqlx::Error> {
    let row: Option<(Uuid, String, String, Value)> = sqlx::query_as(
        r#"
        UPDATE fvoci.import_jobs
        SET status = 'running',
            lease_until = now() + make_interval(secs => $3),
            updated_at = now()
        WHERE workspace_id = $1
          AND id = $2
          AND status IN ('pending', 'running')
        RETURNING created_by, source, status, created_refs
        "#,
    )
    .bind(workspace_id)
    .bind(job_id)
    .bind(IMPORT_LEASE_SECS as f64)
    .fetch_optional(pool)
    .await?;
    let Some((_created_by, source, status, refs)) = row else {
        return Ok(None);
    };
    Ok(Some(ImportJobRow {
        id: job_id,
        workspace_id,
        source: ImportSource::parse(&source).unwrap_or(ImportSource::OfficeFile),
        status: ImportStatus::parse(&status).unwrap_or(ImportStatus::Running),
        created_refs: serde_json::from_value(refs).unwrap_or_default(),
    }))
}
