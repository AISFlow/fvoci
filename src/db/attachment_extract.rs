use super::backend::{Backend, OperationTx};
use super::codec::{Cell, FamilyRow};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

use document_extract_client::limits::{Limits, MAX_INPUT_BYTES, MAX_WARNING_ENTRIES};

use crate::db::identity::{append_event, EventAppend};
use crate::db::search_index::replace_attachment_chunks;
use crate::search::chunk::chunk_plain_text;

pub const EXTRACT_LEASE_SECS: u64 = 300;
pub const EXTRACT_MAX_ATTEMPTS: i16 = 2;
pub const EXTRACT_RETRY_BACKOFF_MS: u64 = 1_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractClaim {
    pub workspace_id: Uuid,
    pub attachment_id: Uuid,
    pub lease_token: Uuid,
    pub attempt: i16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractInput {
    pub storage_key: String,
    pub name: String,
    pub mime: String,
    pub size_bytes: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinishExtract {
    pub status: String,
    pub text: String,
    pub warnings: Vec<String>,
    pub rhwp_rev: Option<String>,
}

pub async fn claim_extract(pool: &PgPool) -> Result<Option<ExtractClaim>, sqlx::Error> {
    claim_extract_backend(&Backend::Postgres(pool.clone())).await
}

async fn claim_extract_pg(
    tx: &mut Transaction<'_, Postgres>,
) -> Result<Option<ExtractClaim>, sqlx::Error> {
    let row = sqlx::query(
        r#"
        SELECT workspace_id, attachment_id, lease_token, attempt
        FROM fvoci.app_claim_attachment_extract()
        "#,
    )
    .fetch_optional(&mut **tx)
    .await?;

    row.map(|row| {
        Ok(ExtractClaim {
            workspace_id: row.try_get("workspace_id")?,
            attachment_id: row.try_get("attachment_id")?,
            lease_token: row.try_get("lease_token")?,
            attempt: row.try_get("attempt")?,
        })
    })
    .transpose()
}

pub async fn load_extract_input(
    pool: &PgPool,
    claim: &ExtractClaim,
) -> Result<Option<ExtractInput>, sqlx::Error> {
    load_extract_input_backend(&Backend::Postgres(pool.clone()), claim).await
}

async fn load_extract_input_pg(
    tx: &mut Transaction<'_, Postgres>,
    claim: &ExtractClaim,
) -> Result<Option<ExtractInput>, sqlx::Error> {
    crate::db::context::set_tenant(tx, claim.workspace_id).await?;
    let row = sqlx::query(
        r#"
        SELECT storage_key, name, mime, size_bytes
        FROM fvoci.attachments
        WHERE workspace_id = $1
          AND id = $2
          AND extract_lease_token = $3
          AND status = 'stored'
          AND extract_status = 'pending'
        "#,
    )
    .bind(claim.workspace_id)
    .bind(claim.attachment_id)
    .bind(claim.lease_token)
    .fetch_optional(&mut **tx)
    .await?;

    row.map(|row| {
        Ok(ExtractInput {
            storage_key: row.try_get("storage_key")?,
            name: row.try_get("name")?,
            mime: row.try_get("mime")?,
            size_bytes: row.try_get("size_bytes")?,
        })
    })
    .transpose()
}

pub async fn finish_extract(
    pool: &PgPool,
    claim: &ExtractClaim,
    finish: &FinishExtract,
) -> Result<bool, sqlx::Error> {
    finish_extract_backend(&Backend::Postgres(pool.clone()), claim, finish).await
}

async fn finish_extract_pg(
    tx: &mut Transaction<'_, Postgres>,
    claim: &ExtractClaim,
    finish: &FinishExtract,
) -> Result<bool, sqlx::Error> {
    let warnings = bounded_warnings_json(&finish.warnings);
    crate::db::context::set_tenant(tx, claim.workspace_id).await?;

    let parent: Option<(Option<Uuid>, Option<Uuid>)> = sqlx::query_as(
        r#"
        SELECT document_id, task_id
        FROM fvoci.attachments
        WHERE workspace_id = $1
          AND id = $2
          AND extract_lease_token = $3
          AND status = 'stored'
          AND extract_status = 'pending'
        "#,
    )
    .bind(claim.workspace_id)
    .bind(claim.attachment_id)
    .bind(claim.lease_token)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(parent) = parent else {
        return Ok(false);
    };

    let workspace_live: Option<(bool,)> =
        sqlx::query_as("SELECT deleted_at IS NULL FROM fvoci.workspaces WHERE id = $1 FOR UPDATE")
            .bind(claim.workspace_id)
            .fetch_optional(&mut **tx)
            .await?;
    if !workspace_live.map(|(live,)| live).unwrap_or(false) {
        return Ok(false);
    }

    let parent_live = match parent {
        (Some(document_id), _) => sqlx::query_scalar::<_, bool>(
            r#"
            SELECT deleted_at IS NULL
            FROM fvoci.documents
            WHERE workspace_id = $1 AND id = $2
            FOR UPDATE
            "#,
        )
        .bind(claim.workspace_id)
        .bind(document_id)
        .fetch_optional(&mut **tx)
        .await?
        .unwrap_or(false),
        (None, Some(task_id)) => sqlx::query_scalar::<_, bool>(
            r#"
            SELECT deleted_at IS NULL
            FROM fvoci.tasks
            WHERE workspace_id = $1 AND id = $2
            FOR NO KEY UPDATE
            "#,
        )
        .bind(claim.workspace_id)
        .bind(task_id)
        .fetch_optional(&mut **tx)
        .await?
        .unwrap_or(false),
        (None, None) => false,
    };
    if !parent_live {
        return Ok(false);
    }

    let attachment_locked: Option<(Option<Uuid>, Option<Uuid>)> = sqlx::query_as(
        r#"
        SELECT document_id, task_id
        FROM fvoci.attachments
        WHERE workspace_id = $1
          AND id = $2
          AND extract_lease_token = $3
          AND status = 'stored'
          AND extract_status = 'pending'
        FOR UPDATE
        "#,
    )
    .bind(claim.workspace_id)
    .bind(claim.attachment_id)
    .bind(claim.lease_token)
    .fetch_optional(&mut **tx)
    .await?;
    if attachment_locked != Some(parent) {
        return Ok(false);
    }

    let updated = sqlx::query(
        r#"
        UPDATE fvoci.attachments
        SET extract_status = $4,
            extract_text = $5,
            extract_warnings = $6,
            extract_rhwp_rev = $7,
            extract_lease_token = NULL,
            extract_lease_expires_at = NULL
        WHERE workspace_id = $1
          AND id = $2
          AND extract_lease_token = $3
          AND status = 'stored'
          AND extract_status = 'pending'
        "#,
    )
    .bind(claim.workspace_id)
    .bind(claim.attachment_id)
    .bind(claim.lease_token)
    .bind(&finish.status)
    .bind(&finish.text)
    .bind(warnings)
    .bind(&finish.rhwp_rev)
    .execute(&mut **tx)
    .await?;

    let chunks = if finish.status == "ok" || finish.status == "partial" {
        chunk_plain_text(&finish.text)
    } else {
        Vec::new()
    };
    replace_attachment_chunks(
        tx,
        claim.workspace_id,
        claim.attachment_id,
        &finish.status,
        &chunks,
    )
    .await?;
    append_event(
        tx,
        EventAppend {
            id: Uuid::now_v7(),
            workspace_id: Some(claim.workspace_id),
            actor_user_id: None,
            verb: "attachment.extracted".into(),
            target_type: Some("attachment".into()),
            target_id: Some(claim.attachment_id),
            payload: json!({
                "attachmentId": claim.attachment_id.to_string(),
                "status": finish.status,
            }),
        },
    )
    .await?;

    Ok(updated.rows_affected() > 0)
}

pub async fn release_extract(pool: &PgPool, claim: &ExtractClaim) -> Result<bool, sqlx::Error> {
    release_extract_backend(&Backend::Postgres(pool.clone()), claim).await
}

async fn release_extract_pg(
    tx: &mut Transaction<'_, Postgres>,
    claim: &ExtractClaim,
) -> Result<bool, sqlx::Error> {
    crate::db::context::set_tenant(tx, claim.workspace_id).await?;
    let updated = sqlx::query(
        r#"
        UPDATE fvoci.attachments
        SET extract_lease_token = NULL,
            extract_lease_expires_at = NULL,
            extract_attempts = GREATEST(extract_attempts - 1, 0)
        WHERE workspace_id = $1
          AND id = $2
          AND extract_lease_token = $3
          AND status = 'stored'
          AND extract_status = 'pending'
        "#,
    )
    .bind(claim.workspace_id)
    .bind(claim.attachment_id)
    .bind(claim.lease_token)
    .execute(&mut **tx)
    .await?;
    Ok(updated.rows_affected() > 0)
}

pub fn default_extract_limits() -> Limits {
    Limits::default()
}

pub fn oversize_resource_limit(size_bytes: i64) -> FinishExtract {
    FinishExtract {
        status: "resource_limit".into(),
        text: String::new(),
        warnings: vec![format!(
            "size_bytes {} exceeds {}-byte extract input limit",
            size_bytes, MAX_INPUT_BYTES
        )],
        rhwp_rev: None,
    }
}

fn bounded_warnings_json(warnings: &[String]) -> Value {
    let capped = warnings
        .iter()
        .take(MAX_WARNING_ENTRIES)
        .collect::<Vec<_>>();
    json!(capped)
}

#[derive(Debug, Clone)]
pub struct ExtractRowState {
    pub extract_status: String,
    pub extract_text: String,
    pub extract_attempts: i16,
    pub lease_token: Option<Uuid>,
    pub lease_expires_at: Option<DateTime<Utc>>,
    pub warnings: Value,
    pub rhwp_rev: Option<String>,
}

pub async fn fetch_extract_state(
    pool: &PgPool,
    workspace_id: Uuid,
    attachment_id: Uuid,
) -> Result<Option<ExtractRowState>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    crate::db::context::set_tenant(&mut tx, workspace_id).await?;
    let row = sqlx::query(
        r#"
        SELECT extract_status,
               extract_text,
               extract_attempts,
               extract_lease_token,
               extract_lease_expires_at,
               extract_warnings,
               extract_rhwp_rev
        FROM fvoci.attachments
        WHERE workspace_id = $1 AND id = $2
        "#,
    )
    .bind(workspace_id)
    .bind(attachment_id)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;

    Ok(row.map(|row| ExtractRowState {
        extract_status: row.get("extract_status"),
        extract_text: row.get("extract_text"),
        extract_attempts: row.get("extract_attempts"),
        lease_token: row.get("extract_lease_token"),
        lease_expires_at: row.get("extract_lease_expires_at"),
        warnings: row.get("extract_warnings"),
        rhwp_rev: row.get("extract_rhwp_rev"),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warnings_are_bounded() {
        let warnings = (0..40).map(|i| i.to_string()).collect::<Vec<_>>();
        let json = bounded_warnings_json(&warnings);
        assert_eq!(json.as_array().unwrap().len(), MAX_WARNING_ENTRIES);
    }
}

/// Background worker authority: system context can claim across tenants, while
/// all subsequent work is bound to the claim's tenant. No interactive-user
/// permission is inferred from a queued attachment or its uploader.
pub async fn claim_extract_backend(backend: &Backend) -> Result<Option<ExtractClaim>, sqlx::Error> {
    let mut tx = backend.begin_write().await?;
    tx.operation().set_system().await?;
    let claim = tx.operation().claim_attachment_extract().await?;
    tx.commit().await.map_err(|e| e.source)?;
    Ok(claim)
}

pub async fn load_extract_input_backend(
    backend: &Backend,
    claim: &ExtractClaim,
) -> Result<Option<ExtractInput>, sqlx::Error> {
    let mut tx = backend.begin_read().await?;
    tx.operation().set_system().await?;
    tx.operation().set_tenant(claim.workspace_id).await?;
    let input = tx.operation().load_attachment_extract(claim).await?;
    tx.commit().await.map_err(|e| e.source)?;
    Ok(input)
}

pub async fn finish_extract_backend(
    backend: &Backend,
    claim: &ExtractClaim,
    finish: &FinishExtract,
) -> Result<bool, sqlx::Error> {
    let mut tx = backend.begin_write().await?;
    tx.operation().set_system().await?;
    tx.operation().set_tenant(claim.workspace_id).await?;
    let applied = tx
        .operation()
        .finish_attachment_extract(claim, finish)
        .await?;
    if applied {
        tx.commit().await.map_err(|e| e.source)?;
    } else {
        tx.rollback().await?;
    }
    Ok(applied)
}

pub async fn release_extract_backend(
    backend: &Backend,
    claim: &ExtractClaim,
) -> Result<bool, sqlx::Error> {
    let mut tx = backend.begin_write().await?;
    tx.operation().set_system().await?;
    tx.operation().set_tenant(claim.workspace_id).await?;
    let released = tx.operation().release_attachment_extract(claim).await?;
    tx.commit().await.map_err(|e| e.source)?;
    Ok(released)
}

fn checked_extract_attempt(cell: &Cell) -> Result<i16, sqlx::Error> {
    let attempt = cell.integer()?;
    if !(1..=i64::from(EXTRACT_MAX_ATTEMPTS)).contains(&attempt) {
        return Err(sqlx::Error::Protocol(
            "invalid extract claim attempt".into(),
        ));
    }
    Ok(attempt as i16)
}

fn decode_extract_input(row: &FamilyRow) -> Result<ExtractInput, sqlx::Error> {
    let size_bytes = row.cell(3)?.integer()?;
    if !(0..=9_007_199_254_740_991).contains(&size_bytes) {
        return Err(sqlx::Error::Protocol(
            "invalid stored attachment size".into(),
        ));
    }
    Ok(ExtractInput {
        storage_key: row.cell(0)?.string()?,
        name: row.cell(1)?.string()?,
        mime: row.cell(2)?.string()?,
        size_bytes,
    })
}

impl OperationTx<'_, '_> {
    pub(crate) async fn claim_attachment_extract(
        &mut self,
    ) -> Result<Option<ExtractClaim>, sqlx::Error> {
        match self {
            Self::Postgres(tx) => claim_extract_pg(tx).await,
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                tx.require_system_context()?;
                // One authoritative clock sample under the same writer as all
                // housekeeping and the claim, matching PG transaction now().
                let clock = tx.query("SELECT unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000", &[]).await?;
                let now = clock.first().ok_or(sqlx::Error::RowNotFound)?.cell(0)?;
                now.datetime()?;
                let expiry = now
                    .integer()?
                    .checked_add(EXTRACT_LEASE_SECS as i64 * 1_000_000)
                    .ok_or_else(|| {
                        sqlx::Error::Protocol("extract lease timestamp overflow".into())
                    })?;
                Cell::Integer(expiry).datetime()?;
                tx.execute("UPDATE attachments SET extract_status='worker_failure',extract_text='',extract_warnings='[]',extract_rhwp_rev=NULL,extract_lease_token=NULL,extract_lease_expires_at=NULL WHERE (workspace_id,id) IN (SELECT workspace_id,id FROM attachments WHERE status='stored' AND extract_status='pending' AND extract_attempts>=2 AND extract_lease_expires_at IS NOT NULL AND extract_lease_expires_at<?1 ORDER BY completed_at,id LIMIT 50)", std::slice::from_ref(&now)).await?;
                tx.execute("UPDATE attachments SET extract_status='skipped',extract_text='',extract_warnings='[]',extract_rhwp_rev=NULL,extract_lease_token=NULL,extract_lease_expires_at=NULL WHERE (workspace_id,id) IN (SELECT a.workspace_id,a.id FROM attachments a LEFT JOIN documents d ON d.workspace_id=a.workspace_id AND d.id=a.document_id AND d.deleted_at IS NULL LEFT JOIN tasks t ON t.workspace_id=a.workspace_id AND t.id=a.task_id AND t.deleted_at IS NULL LEFT JOIN workspaces w ON w.id=a.workspace_id AND w.deleted_at IS NULL WHERE a.status='stored' AND a.extract_status='pending' AND ((d.id IS NULL AND t.id IS NULL) OR w.id IS NULL) AND (a.extract_lease_expires_at IS NULL OR a.extract_lease_expires_at<?1) ORDER BY a.completed_at,a.id LIMIT 50)", std::slice::from_ref(&now)).await?;
                let rows = tx.query("SELECT a.workspace_id,a.id,a.extract_attempts+1 FROM attachments a LEFT JOIN documents d ON d.workspace_id=a.workspace_id AND d.id=a.document_id AND d.deleted_at IS NULL LEFT JOIN tasks t ON t.workspace_id=a.workspace_id AND t.id=a.task_id AND t.deleted_at IS NULL JOIN workspaces w ON w.id=a.workspace_id AND w.deleted_at IS NULL WHERE (d.id IS NOT NULL OR t.id IS NOT NULL) AND a.status='stored' AND a.extract_status='pending' AND a.extract_attempts<2 AND (a.extract_lease_expires_at IS NULL OR a.extract_lease_expires_at<?1) ORDER BY a.completed_at,a.id LIMIT 1", &[now]).await?;
                let Some(row) = rows.first() else {
                    return Ok(None);
                };
                let claim = ExtractClaim {
                    workspace_id: row.cell(0)?.id()?,
                    attachment_id: row.cell(1)?.id()?,
                    lease_token: Uuid::now_v7(),
                    attempt: checked_extract_attempt(&row.cell(2)?)?,
                };
                let changed = tx.execute("UPDATE attachments SET extract_lease_token=?3,extract_lease_expires_at=?4,extract_attempts=extract_attempts+1 WHERE workspace_id=?1 AND id=?2", &[Cell::uuid(claim.workspace_id),Cell::uuid(claim.attachment_id),Cell::uuid(claim.lease_token),Cell::Integer(expiry)]).await?;
                if changed != 1 {
                    return Err(sqlx::Error::RowNotFound);
                }
                Ok(Some(claim))
            }
        }
    }

    pub(crate) async fn load_attachment_extract(
        &mut self,
        claim: &ExtractClaim,
    ) -> Result<Option<ExtractInput>, sqlx::Error> {
        checked_extract_attempt(&Cell::Integer(i64::from(claim.attempt)))?;
        match self {
            Self::Postgres(tx) => load_extract_input_pg(tx, claim).await,
            Self::SqliteFamily(tx) => {
                tx.require_system_context()?;
                tx.require_tenant(claim.workspace_id)?;
                let rows = tx.query("SELECT storage_key,name,mime,size_bytes FROM attachments WHERE workspace_id=?1 AND id=?2 AND extract_lease_token=?3 AND status='stored' AND extract_status='pending'", &[Cell::uuid(claim.workspace_id),Cell::uuid(claim.attachment_id),Cell::uuid(claim.lease_token)]).await?;
                rows.first().map(decode_extract_input).transpose()
            }
        }
    }

    pub(crate) async fn finish_attachment_extract(
        &mut self,
        claim: &ExtractClaim,
        finish: &FinishExtract,
    ) -> Result<bool, sqlx::Error> {
        checked_extract_attempt(&Cell::Integer(i64::from(claim.attempt)))?;
        match self {
            Self::Postgres(tx) => finish_extract_pg(tx, claim, finish).await,
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                tx.require_system_context()?;
                tx.require_tenant(claim.workspace_id)?;
                // The writer prevents parent deletion or lease replacement
                // between this current-row check and finish/chunks/event.
                let rows = tx.query("SELECT a.document_id,a.task_id FROM attachments a JOIN workspaces w ON w.id=a.workspace_id AND w.deleted_at IS NULL LEFT JOIN documents d ON d.workspace_id=a.workspace_id AND d.id=a.document_id AND d.deleted_at IS NULL LEFT JOIN tasks t ON t.workspace_id=a.workspace_id AND t.id=a.task_id AND t.deleted_at IS NULL WHERE a.workspace_id=?1 AND a.id=?2 AND a.extract_lease_token=?3 AND a.status='stored' AND a.extract_status='pending' AND (d.id IS NOT NULL OR t.id IS NOT NULL)", &[Cell::uuid(claim.workspace_id),Cell::uuid(claim.attachment_id),Cell::uuid(claim.lease_token)]).await?;
                let Some(row) = rows.first() else {
                    return Ok(false);
                };
                let document = row.cell(0)?.optional(Cell::id)?;
                let task = row.cell(1)?.optional(Cell::id)?;
                if document.is_some() == task.is_some() {
                    return Err(sqlx::Error::Protocol("invalid extract parent".into()));
                }
                let changed = tx.execute("UPDATE attachments SET extract_status=?4,extract_text=?5,extract_warnings=?6,extract_rhwp_rev=?7,extract_lease_token=NULL,extract_lease_expires_at=NULL WHERE workspace_id=?1 AND id=?2 AND extract_lease_token=?3 AND status='stored' AND extract_status='pending'", &[Cell::uuid(claim.workspace_id),Cell::uuid(claim.attachment_id),Cell::uuid(claim.lease_token),Cell::text(&finish.status),Cell::text(&finish.text),Cell::json(&bounded_warnings_json(&finish.warnings))?,Cell::optional_text(finish.rhwp_rev.as_deref())]).await?;
                if changed != 1 {
                    return Err(sqlx::Error::RowNotFound);
                }
                let chunks = if finish.status == "ok" || finish.status == "partial" {
                    chunk_plain_text(&finish.text)
                } else {
                    Vec::new()
                };
                self.replace_attachment_chunks(
                    claim.workspace_id,
                    claim.attachment_id,
                    &finish.status,
                    &chunks,
                )
                .await?;
                self.append_event(EventAppend {id:Uuid::now_v7(),workspace_id:Some(claim.workspace_id),actor_user_id:None,verb:"attachment.extracted".into(),target_type:Some("attachment".into()),target_id:Some(claim.attachment_id),payload:json!({"attachmentId":claim.attachment_id.to_string(),"status":finish.status})}).await?;
                Ok(true)
            }
        }
    }

    pub(crate) async fn release_attachment_extract(
        &mut self,
        claim: &ExtractClaim,
    ) -> Result<bool, sqlx::Error> {
        checked_extract_attempt(&Cell::Integer(i64::from(claim.attempt)))?;
        match self {
            Self::Postgres(tx) => release_extract_pg(tx, claim).await,
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                tx.require_system_context()?;
                tx.require_tenant(claim.workspace_id)?;
                let changed = tx.execute("UPDATE attachments SET extract_lease_token=NULL,extract_lease_expires_at=NULL,extract_attempts=max(extract_attempts-1,0) WHERE workspace_id=?1 AND id=?2 AND extract_lease_token=?3 AND status='stored' AND extract_status='pending'", &[Cell::uuid(claim.workspace_id),Cell::uuid(claim.attachment_id),Cell::uuid(claim.lease_token)]).await?;
                Ok(changed > 0)
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod backend_tests {
    use super::*;
    use sqlx::SqlitePool;
    use std::path::PathBuf;

    pub(crate) struct Fixture {
        pub(crate) directory: PathBuf,
        pub(crate) path: PathBuf,
        pub(crate) pool: SqlitePool,
        pub(crate) backend: Backend,
        pub(crate) workspace: Uuid,
        pub(crate) document: Uuid,
        pub(crate) attachment: Uuid,
        pub(crate) storage_key: String,
    }
    impl Fixture {
        pub(crate) async fn new() -> Self {
            let directory =
                std::env::temp_dir().join(format!("fvoci-w2-extract-{}", Uuid::now_v7()));
            std::fs::create_dir(&directory).unwrap();
            let path = directory.join("extract.sqlite");
            let prepare = crate::db::pool::connect_sqlite_prepare(&path)
                .await
                .unwrap();
            let mut tx = prepare.pool.begin_with("BEGIN IMMEDIATE").await.unwrap();
            for ddl in [
                include_str!("../../migrations/sqlite/001_current_schema.sql"),
                include_str!("../../migrations/sqlite/002_wiki_create_commands.sql"),
                include_str!("../../migrations/sqlite/003_collab_room_fences.sql"),
            ] {
                sqlx::raw_sql(ddl).execute(&mut *tx).await.unwrap();
            }
            tx.commit().await.unwrap();
            prepare.close_confirmed().await.unwrap();
            let pool = crate::db::pool::connect_sqlite_app(&path, 1).await.unwrap();
            assert_eq!(
                sqlx::query_scalar::<_, String>("SELECT sqlite_version()")
                    .fetch_one(&pool)
                    .await
                    .unwrap(),
                crate::db::pool::SQLITE_VERSION
            );
            assert_eq!(
                sqlx::query_scalar::<_, i64>("PRAGMA foreign_keys")
                    .fetch_one(&pool)
                    .await
                    .unwrap(),
                1
            );
            let (user, workspace, document, attachment) = (
                Uuid::now_v7(),
                Uuid::now_v7(),
                Uuid::now_v7(),
                Uuid::now_v7(),
            );
            sqlx::query("INSERT INTO users (id,email,given_name,password_hash) VALUES (?1,'extract@example.invalid','Extract fixture','synthetic')").bind(user.as_bytes().as_slice()).execute(&pool).await.unwrap();
            sqlx::query("INSERT INTO workspaces (id,slug,name) VALUES (?1,'extract-test','Extract fixture')").bind(workspace.as_bytes().as_slice()).execute(&pool).await.unwrap();
            sqlx::query("INSERT INTO documents (id,workspace_id,title,path,sort_key,number,status,schema_version,created_by,content_json) VALUES (?1,?2,'Extract',?3,'a',1,'published',1,?4,'{}')").bind(document.as_bytes().as_slice()).bind(workspace.as_bytes().as_slice()).bind(document.simple().to_string()).bind(user.as_bytes().as_slice()).execute(&pool).await.unwrap();
            sqlx::query("INSERT INTO attachments (id,workspace_id,document_id,uploader_id,status,name,mime,size_bytes,reserved_size_bytes,storage_key,completed_at,extract_status) VALUES (?1,?2,?3,?4,'stored','fixture.txt','text/plain',1,1,?5,unixepoch()*1000000,'pending')").bind(attachment.as_bytes().as_slice()).bind(workspace.as_bytes().as_slice()).bind(document.as_bytes().as_slice()).bind(user.as_bytes().as_slice()).bind(attachment.to_string()).execute(&pool).await.unwrap();
            Self {
                directory,
                path,
                pool: pool.clone(),
                backend: Backend::Sqlite(pool),
                workspace,
                document,
                attachment,
                storage_key: attachment.to_string(),
            }
        }
        pub(crate) async fn claim(&self) -> ExtractClaim {
            claim_extract_backend(&self.backend).await.unwrap().unwrap()
        }
        pub(crate) async fn expire(&self) {
            sqlx::query("UPDATE attachments SET extract_lease_expires_at=unixepoch()*1000000-1000000 WHERE id=?1").bind(self.attachment.as_bytes().as_slice()).execute(&self.pool).await.unwrap();
        }
        pub(crate) async fn extracted(&self, text: &str) {
            let c = self.claim().await;
            assert!(finish_extract_backend(
                &self.backend,
                &c,
                &FinishExtract {
                    status: "ok".into(),
                    text: text.into(),
                    warnings: vec![],
                    rhwp_rev: None
                }
            )
            .await
            .unwrap());
        }
        pub(crate) async fn event_count(&self, verb: &str) -> i64 {
            sqlx::query_scalar("SELECT count(*) FROM events WHERE verb=?1")
                .bind(verb)
                .fetch_one(&self.pool)
                .await
                .unwrap()
        }
        pub(crate) async fn finish(self) {
            self.pool.close().await;
            self.backend.close().await.unwrap();
            std::fs::remove_dir_all(self.directory).unwrap();
        }
    }
    fn result() -> FinishExtract {
        FinishExtract {
            status: "partial".into(),
            text: "안녕 world\nSecond paragraph".into(),
            warnings: (0..40).map(|i| i.to_string()).collect(),
            rhwp_rev: Some("fixture-revision".into()),
        }
    }

    #[tokio::test]
    async fn claim_finish_chunks_event_are_durable_on_a_fresh_connection() {
        let f = Fixture::new().await;
        let claim = f.claim().await;
        assert_eq!(claim.attempt, 1);
        assert!(claim_extract_backend(&f.backend).await.unwrap().is_none());
        let input = load_extract_input_backend(&f.backend, &claim)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(input.storage_key, f.storage_key);
        assert_eq!(input.size_bytes, 1);
        let finish = result();
        assert!(finish_extract_backend(&f.backend, &claim, &finish)
            .await
            .unwrap());
        assert!(!finish_extract_backend(&f.backend, &claim, &finish)
            .await
            .unwrap());
        let fresh = crate::db::pool::connect_sqlite_app(&f.path, 1)
            .await
            .unwrap();
        let (status,text,warnings,rev,token):(String,String,String,Option<String>,Option<Vec<u8>>)=sqlx::query_as("SELECT extract_status,extract_text,extract_warnings,extract_rhwp_rev,extract_lease_token FROM attachments").fetch_one(&fresh).await.unwrap();
        assert_eq!(status, "partial");
        assert_eq!(text, finish.text);
        assert_eq!(rev, finish.rhwp_rev);
        assert!(token.is_none());
        assert_eq!(
            serde_json::from_str::<Value>(&warnings)
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            MAX_WARNING_ENTRIES
        );
        let chunks = chunk_plain_text(&finish.text);
        let rows:Vec<(i32,i32,i32,String,String)>=sqlx::query_as("SELECT chunk_no,start_offset,end_offset,text,chosung FROM attachment_text ORDER BY chunk_no").fetch_all(&fresh).await.unwrap();
        assert_eq!(rows.len(), chunks.len());
        assert!(!rows.is_empty());
        for (r, c) in rows.iter().zip(chunks) {
            assert_eq!(
                r,
                &(
                    c.chunk_no,
                    c.start,
                    c.end,
                    c.text.clone(),
                    crate::search::text::to_chosung(&c.text)
                )
            );
        }
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM events WHERE verb='attachment.extracted'"
            )
            .fetch_one(&fresh)
            .await
            .unwrap(),
            1
        );
        fresh.close().await;
        f.finish().await;
    }

    #[tokio::test]
    async fn cross_pool_reclaim_fences_stale_finish_and_release() {
        let f = Fixture::new().await;
        let old = f.claim().await;
        f.expire().await;
        let other = crate::db::pool::connect_sqlite_app(&f.path, 1)
            .await
            .unwrap();
        let backend = Backend::Sqlite(other.clone());
        let current = claim_extract_backend(&backend).await.unwrap().unwrap();
        assert_eq!(current.attempt, 2);
        assert_ne!(old.lease_token, current.lease_token);
        assert!(load_extract_input_backend(&f.backend, &old)
            .await
            .unwrap()
            .is_none());
        assert!(!release_extract_backend(&f.backend, &old).await.unwrap());
        assert!(!finish_extract_backend(&f.backend, &old, &result())
            .await
            .unwrap());
        assert_eq!(f.event_count("attachment.extracted").await, 0);
        assert!(finish_extract_backend(&backend, &current, &result())
            .await
            .unwrap());
        assert_eq!(f.event_count("attachment.extracted").await, 1);
        backend.close().await.unwrap();
        other.close().await;
        f.finish().await;
    }

    #[tokio::test]
    async fn current_parent_tenant_authority_and_retry_exhaustion() {
        for denial in ["tenant", "parent", "workspace"] {
            let f = Fixture::new().await;
            let mut c = f.claim().await;
            match denial {
                "tenant" => c.workspace_id = Uuid::now_v7(),
                "parent" => {
                    sqlx::query("UPDATE documents SET deleted_at=unixepoch()*1000000 WHERE id=?1")
                        .bind(f.document.as_bytes().as_slice())
                        .execute(&f.pool)
                        .await
                        .unwrap();
                }
                _ => {
                    sqlx::query("UPDATE workspaces SET deleted_at=unixepoch()*1000000")
                        .execute(&f.pool)
                        .await
                        .unwrap();
                }
            }
            assert!(!finish_extract_backend(&f.backend, &c, &result())
                .await
                .unwrap());
            assert_eq!(f.event_count("attachment.extracted").await, 0);
            if denial != "tenant" {
                f.expire().await;
                assert!(claim_extract_backend(&f.backend).await.unwrap().is_none());
                assert_eq!(
                    sqlx::query_scalar::<_, String>("SELECT extract_status FROM attachments")
                        .fetch_one(&f.pool)
                        .await
                        .unwrap(),
                    "skipped"
                );
            }
            f.finish().await;
        }
        let f = Fixture::new().await;
        let c = f.claim().await;
        let mut tx = f.backend.begin_write().await.unwrap();
        tx.operation().set_tenant(f.workspace).await.unwrap();
        assert!(tx
            .operation()
            .finish_attachment_extract(&c, &result())
            .await
            .is_err());
        tx.rollback().await.unwrap();
        let mut tx = f.backend.begin_read().await.unwrap();
        tx.operation().set_system().await.unwrap();
        assert!(tx.operation().claim_attachment_extract().await.is_err());
        tx.rollback().await.unwrap();
        let mut tx = f.backend.begin_write().await.unwrap();
        tx.operation().set_system().await.unwrap();
        tx.operation().set_tenant(Uuid::now_v7()).await.unwrap();
        assert!(tx.operation().release_attachment_extract(&c).await.is_err());
        tx.rollback().await.unwrap();
        f.expire().await;
        let second = f.claim().await;
        assert_eq!(second.attempt, 2);
        f.expire().await;
        assert!(claim_extract_backend(&f.backend).await.unwrap().is_none());
        let (status, attempts, token): (String, i16, Option<Vec<u8>>) = sqlx::query_as(
            "SELECT extract_status,extract_attempts,extract_lease_token FROM attachments",
        )
        .fetch_one(&f.pool)
        .await
        .unwrap();
        assert_eq!(status, "worker_failure");
        assert_eq!(attempts, EXTRACT_MAX_ATTEMPTS);
        assert!(token.is_none());
        f.finish().await;
    }

    #[tokio::test]
    async fn finish_sql_failure_and_abort_rollback_before_pool_reuse() {
        let f = Fixture::new().await;
        let claim = f.claim().await;
        sqlx::raw_sql("CREATE TRIGGER fail_extract_event BEFORE INSERT ON events WHEN NEW.verb='attachment.extracted' BEGIN SELECT RAISE(ABORT,'fixture event failure'); END;").execute(&f.pool).await.unwrap();
        assert!(finish_extract_backend(&f.backend, &claim, &result())
            .await
            .is_err());
        let status: String = sqlx::query_scalar("SELECT extract_status FROM attachments")
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(status, "pending");
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM attachment_text")
                .fetch_one(&f.pool)
                .await
                .unwrap(),
            0
        );
        assert_eq!(f.event_count("attachment.extracted").await, 0);
        sqlx::raw_sql("DROP TRIGGER fail_extract_event")
            .execute(&f.pool)
            .await
            .unwrap();
        let mut tx = f.backend.begin_write().await.unwrap();
        tx.operation().set_system().await.unwrap();
        tx.operation().set_tenant(f.workspace).await.unwrap();
        assert!(tx
            .operation()
            .finish_attachment_extract(&claim, &result())
            .await
            .unwrap());
        drop(tx); // Simulate task abort after all operations but before commit.
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT extract_status FROM attachments")
                .fetch_one(&f.pool)
                .await
                .unwrap(),
            "pending"
        );
        assert_eq!(f.event_count("attachment.extracted").await, 0);
        assert!(finish_extract_backend(&f.backend, &claim, &result())
            .await
            .unwrap());
        f.finish().await;
    }

    #[tokio::test]
    async fn two_pools_claim_one_attachment_once_and_keep_foreign_keys() {
        let f = Fixture::new().await;
        let pool = crate::db::pool::connect_sqlite_app(&f.path, 1)
            .await
            .unwrap();
        let other = Backend::Sqlite(pool.clone());
        let (one, two) = tokio::join!(
            claim_extract_backend(&f.backend),
            claim_extract_backend(&other)
        );
        let one = one.unwrap();
        let two = two.unwrap();
        assert_ne!(one.is_some(), two.is_some());
        let claim = one.or(two).unwrap();
        assert_eq!(claim.attempt, 1);
        assert!(sqlx::query("UPDATE attachments SET document_id=?1")
            .bind(Uuid::now_v7().as_bytes().as_slice())
            .execute(&pool)
            .await
            .is_err());
        assert!(finish_extract_backend(&other, &claim, &result())
            .await
            .unwrap());
        assert_eq!(f.event_count("attachment.extracted").await, 1);
        other.close().await.unwrap();
        pool.close().await;
        f.finish().await;
    }

    #[test]
    fn checked_claim_cells_and_native_dates_reject_malformed_values() {
        assert!(checked_extract_attempt(&Cell::Integer(0)).is_err());
        assert!(checked_extract_attempt(&Cell::Integer(3)).is_err());
        assert!(checked_extract_attempt(&Cell::Integer(i64::MAX)).is_err());
        assert!(checked_extract_attempt(&Cell::text("1")).is_err());
        assert!(Cell::Blob(vec![0; 15]).id().is_err());
        assert!(Cell::Integer(i64::MAX).datetime().is_err());
        assert!(Cell::Null.datetime().is_err());
    }
}
