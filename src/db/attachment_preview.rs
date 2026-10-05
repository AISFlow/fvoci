//! Preview lease/journal persistence. Named operations borrow the caller's
//! transaction, context and writer reservation. Rendering and storage I/O never
//! run in that transaction. Publication and reclamation serialize on the same
//! journal row (PostgreSQL) or reserved writer (SQLite family).

use serde_json::json;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use super::backend::{Backend, OperationTx};
use super::codec::Cell;

pub const PREVIEW_LEASE_SECS: i32 = 180;

#[derive(Debug, Clone)]
pub struct PreviewClaim {
    pub workspace_id: Uuid,
    pub attachment_id: Uuid,
    pub lease_token: Uuid,
    pub attempt: i16,
}

#[derive(Debug, Clone)]
pub struct PreviewInput {
    pub storage_key: String,
    pub mime: String,
    pub size_bytes: i64,
}

pub async fn claim_preview(pool: &PgPool) -> Result<Option<PreviewClaim>, sqlx::Error> {
    claim_preview_backend(&Backend::Postgres(pool.clone())).await
}

pub async fn claim_preview_backend(backend: &Backend) -> Result<Option<PreviewClaim>, sqlx::Error> {
    let mut tx = backend.begin_write().await?;
    let mut op = tx.operation();
    let previous = op.set_system().await?;
    let claim = op.claim_attachment_preview().await?;
    op.restore_system(previous).await?;
    tx.commit().await.map_err(|e| e.source)?;
    Ok(claim)
}

pub async fn load_preview_input(
    pool: &PgPool,
    claim: &PreviewClaim,
) -> Result<Option<PreviewInput>, sqlx::Error> {
    load_preview_input_backend(&Backend::Postgres(pool.clone()), claim).await
}

pub async fn load_preview_input_backend(
    backend: &Backend,
    claim: &PreviewClaim,
) -> Result<Option<PreviewInput>, sqlx::Error> {
    let mut tx = backend.begin_write().await?;
    let mut op = tx.operation();
    op.set_tenant(claim.workspace_id).await?;
    let input = op.load_attachment_preview(claim).await?;
    tx.commit().await.map_err(|e| e.source)?;
    Ok(input)
}

/// Preserve the public PG wrapper. A stale caller receives RowNotFound before
/// writing storage; the backend consumer handles that refusal as no work left.
pub async fn journal_preview_key(
    pool: &PgPool,
    claim: &PreviewClaim,
    key: &str,
) -> Result<Uuid, sqlx::Error> {
    journal_preview_key_backend(&Backend::Postgres(pool.clone()), claim, key)
        .await?
        .ok_or(sqlx::Error::RowNotFound)
}

pub async fn journal_preview_key_backend(
    backend: &Backend,
    claim: &PreviewClaim,
    key: &str,
) -> Result<Option<Uuid>, sqlx::Error> {
    let mut tx = backend.begin_write().await?;
    let mut op = tx.operation();
    op.set_tenant(claim.workspace_id).await?;
    let id = op.journal_attachment_preview(claim, key).await?;
    tx.commit().await.map_err(|e| e.source)?;
    Ok(id)
}

pub async fn publish_preview(
    pool: &PgPool,
    claim: &PreviewClaim,
    journal_id: Uuid,
    key: &str,
    width: u32,
    height: u32,
    bytes: u64,
) -> Result<bool, sqlx::Error> {
    publish_preview_backend(
        &Backend::Postgres(pool.clone()),
        claim,
        journal_id,
        key,
        width,
        height,
        bytes,
    )
    .await
}

pub async fn publish_preview_backend(
    backend: &Backend,
    claim: &PreviewClaim,
    journal_id: Uuid,
    key: &str,
    width: u32,
    height: u32,
    bytes: u64,
) -> Result<bool, sqlx::Error> {
    publish_preview_backend_with_cancel(backend, claim, journal_id, key, width, height, bytes, None)
        .await
}

/// The consumer supplies its actual cancellation scope. Wait for owned BEGIN
/// cleanup, then fence cancellation before effects and again before commit.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn publish_preview_backend_with_cancel(
    backend: &Backend,
    claim: &PreviewClaim,
    journal_id: Uuid,
    key: &str,
    width: u32,
    height: u32,
    bytes: u64,
    cancel: Option<&tokio_util::sync::CancellationToken>,
) -> Result<bool, sqlx::Error> {
    let mut tx = backend.begin_write().await?;
    if cancel.is_some_and(|token| token.is_cancelled()) {
        tx.rollback().await?;
        return Ok(false);
    }
    let mut op = tx.operation();
    op.set_tenant(claim.workspace_id).await?;
    let published = op
        .publish_attachment_preview(claim, journal_id, key, width, height, bytes)
        .await?;
    if published && !cancel.is_some_and(|token| token.is_cancelled()) {
        tx.commit().await.map_err(|e| e.source)?;
    } else {
        tx.rollback().await?;
        return Ok(false);
    }
    Ok(published)
}

pub async fn fail_preview(pool: &PgPool, claim: &PreviewClaim) -> Result<bool, sqlx::Error> {
    fail_preview_backend(&Backend::Postgres(pool.clone()), claim).await
}
pub async fn release_preview(pool: &PgPool, claim: &PreviewClaim) -> Result<bool, sqlx::Error> {
    release_preview_backend(&Backend::Postgres(pool.clone()), claim).await
}
pub async fn fail_preview_backend(
    backend: &Backend,
    claim: &PreviewClaim,
) -> Result<bool, sqlx::Error> {
    finish_preview_backend(backend, claim, false).await
}
pub async fn release_preview_backend(
    backend: &Backend,
    claim: &PreviewClaim,
) -> Result<bool, sqlx::Error> {
    finish_preview_backend(backend, claim, true).await
}
async fn finish_preview_backend(
    backend: &Backend,
    claim: &PreviewClaim,
    release: bool,
) -> Result<bool, sqlx::Error> {
    let mut tx = backend.begin_write().await?;
    let mut op = tx.operation();
    op.set_tenant(claim.workspace_id).await?;
    let changed = if release {
        op.release_attachment_preview(claim).await?
    } else {
        op.fail_attachment_preview(claim).await?
    };
    tx.commit().await.map_err(|e| e.source)?;
    Ok(changed)
}

impl OperationTx<'_, '_> {
    pub(crate) async fn claim_attachment_preview(
        &mut self,
    ) -> Result<Option<PreviewClaim>, sqlx::Error> {
        match self {
            Self::Postgres(tx) => {
                let row = sqlx::query("SELECT workspace_id, attachment_id, lease_token, attempt FROM fvoci.app_claim_attachment_preview($1)")
                    .bind(PREVIEW_LEASE_SECS).fetch_optional(&mut ***tx).await?;
                row.map(|row| {
                    Ok(PreviewClaim {
                        workspace_id: row.try_get("workspace_id")?,
                        attachment_id: row.try_get("attachment_id")?,
                        lease_token: row.try_get("lease_token")?,
                        attempt: row.try_get("attempt")?,
                    })
                })
                .transpose()
            }
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                tx.require_system_context()?;
                if tx.tenant().is_some() {
                    return Err(sqlx::Error::Protocol(
                        "preview global claim cannot use tenant context".into(),
                    ));
                }
                let now = preview_now(tx).await?;
                // Exact PG030 cap, ordering and strict expired comparison.
                tx.execute("UPDATE attachments SET preview_status='failed',preview_lease_token=NULL,preview_lease_expires_at=NULL WHERE id IN (SELECT id FROM attachments WHERE status='stored' AND preview_status='pending' AND preview_attempts>=3 AND preview_lease_expires_at IS NOT NULL AND preview_lease_expires_at<?1 ORDER BY completed_at,id LIMIT 50)", &[Cell::Integer(now)]).await?;
                let rows = tx.query("SELECT a.workspace_id,a.id,a.preview_attempts FROM attachments a JOIN workspaces w ON w.id=a.workspace_id AND w.deleted_at IS NULL LEFT JOIN documents d ON d.workspace_id=a.workspace_id AND d.id=a.document_id AND d.deleted_at IS NULL LEFT JOIN tasks t ON t.workspace_id=a.workspace_id AND t.id=a.task_id AND t.deleted_at IS NULL WHERE (d.id IS NOT NULL OR t.id IS NOT NULL) AND a.status='stored' AND a.preview_status='pending' AND a.preview_attempts<3 AND (a.preview_lease_expires_at IS NULL OR a.preview_lease_expires_at<?1) ORDER BY a.completed_at,a.id LIMIT 1", &[Cell::Integer(now)]).await?;
                let Some(row) = rows.first() else {
                    return Ok(None);
                };
                let claim = PreviewClaim {
                    workspace_id: row.cell(0)?.id()?,
                    attachment_id: row.cell(1)?.id()?,
                    lease_token: Uuid::now_v7(),
                    attempt: i16::try_from(row.cell(2)?.integer()? + 1)
                        .map_err(|_| sqlx::Error::Protocol("preview attempt overflow".into()))?,
                };
                let changed = tx.execute("UPDATE attachments SET preview_lease_token=?3,preview_lease_expires_at=?4,preview_attempts=preview_attempts+1 WHERE workspace_id=?1 AND id=?2 AND status='stored' AND preview_status='pending' AND preview_attempts<3 AND (preview_lease_expires_at IS NULL OR preview_lease_expires_at<?5)", &[Cell::uuid(claim.workspace_id),Cell::uuid(claim.attachment_id),Cell::uuid(claim.lease_token),Cell::Integer(now+i64::from(PREVIEW_LEASE_SECS)*1_000_000),Cell::Integer(now)]).await?;
                if changed != 1 {
                    return Err(sqlx::Error::Protocol(
                        "reserved preview claim changed unexpectedly".into(),
                    ));
                }
                Ok(Some(claim))
            }
        }
    }

    async fn require_preview_scope(&mut self, workspace: Uuid) -> Result<(), sqlx::Error> {
        match self {
            Self::Postgres(tx) => {
                let (tenant, system, readonly): (Option<String>, Option<String>, String) = sqlx::query_as("SELECT current_setting('app.tenant_id',true),current_setting('app.system_ctx',true),current_setting('transaction_read_only')").fetch_one(&mut ***tx).await?;
                if tenant.as_deref() != Some(workspace.to_string().as_str())
                    || system.as_deref() == Some("on")
                    || readonly != "off"
                {
                    return Err(sqlx::Error::Protocol(
                        "preview operation needs scoped writer context".into(),
                    ));
                }
                Ok(())
            }
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                tx.require_tenant(workspace)?;
                if tx.require_system_context().is_ok() {
                    return Err(sqlx::Error::Protocol(
                        "preview operation cannot broaden to system context".into(),
                    ));
                }
                Ok(())
            }
        }
    }

    /// Current background authority is the selected tenant, current claim and
    /// live workspace/document-or-task. No uploader session is invented.
    /// PG locks workspace then parent before the attachment publication update;
    /// the family caller already reserves its writer before these reads.
    pub(crate) async fn preview_attachment_is_live(
        &mut self,
        claim: &PreviewClaim,
    ) -> Result<bool, sqlx::Error> {
        self.require_preview_scope(claim.workspace_id).await?;
        match self {
            Self::Postgres(tx) => {
                let parent: Option<(Option<Uuid>,Option<Uuid>)> = sqlx::query_as("SELECT document_id,task_id FROM fvoci.attachments WHERE workspace_id=$1 AND id=$2 AND preview_lease_token=$3 AND status='stored' AND preview_status='pending'")
                    .bind(claim.workspace_id).bind(claim.attachment_id).bind(claim.lease_token).fetch_optional(&mut ***tx).await?;
                let Some(parent) = parent else {
                    return Ok(false);
                };
                let workspace: Option<bool> = sqlx::query_scalar(
                    "SELECT deleted_at IS NULL FROM fvoci.workspaces WHERE id=$1 FOR UPDATE",
                )
                .bind(claim.workspace_id)
                .fetch_optional(&mut ***tx)
                .await?;
                if workspace != Some(true) {
                    return Ok(false);
                }
                let live: Option<bool> = match parent {
                    (Some(id),None) => sqlx::query_scalar("SELECT deleted_at IS NULL FROM fvoci.documents WHERE workspace_id=$1 AND id=$2 FOR UPDATE").bind(claim.workspace_id).bind(id).fetch_optional(&mut ***tx).await?,
                    (None,Some(id)) => sqlx::query_scalar("SELECT deleted_at IS NULL FROM fvoci.tasks WHERE workspace_id=$1 AND id=$2 FOR NO KEY UPDATE").bind(claim.workspace_id).bind(id).fetch_optional(&mut ***tx).await?,
                    _ => None,
                };
                Ok(live == Some(true))
            }
            Self::SqliteFamily(tx) => {
                let rows = tx.query("SELECT EXISTS(SELECT 1 FROM attachments a JOIN workspaces w ON w.id=a.workspace_id AND w.deleted_at IS NULL LEFT JOIN documents d ON d.workspace_id=a.workspace_id AND d.id=a.document_id AND d.deleted_at IS NULL LEFT JOIN tasks t ON t.workspace_id=a.workspace_id AND t.id=a.task_id AND t.deleted_at IS NULL WHERE a.workspace_id=?1 AND a.id=?2 AND a.preview_lease_token=?3 AND a.status='stored' AND a.preview_status='pending' AND (d.id IS NOT NULL OR t.id IS NOT NULL))", &claim_cells(claim)).await?;
                rows[0].cell(0)?.boolean()
            }
        }
    }

    pub(crate) async fn load_attachment_preview(
        &mut self,
        claim: &PreviewClaim,
    ) -> Result<Option<PreviewInput>, sqlx::Error> {
        if !self.preview_attachment_is_live(claim).await? {
            return Ok(None);
        }
        match self {
            Self::Postgres(tx) => {
                let row = sqlx::query("SELECT storage_key,mime,size_bytes FROM fvoci.attachments WHERE workspace_id=$1 AND id=$2 AND preview_lease_token=$3 AND status='stored' AND preview_status='pending' AND preview_lease_expires_at>now()")
                    .bind(claim.workspace_id).bind(claim.attachment_id).bind(claim.lease_token).fetch_optional(&mut ***tx).await?;
                row.map(|row| {
                    Ok(PreviewInput {
                        storage_key: row.try_get("storage_key")?,
                        mime: row.try_get("mime")?,
                        size_bytes: row.try_get("size_bytes")?,
                    })
                })
                .transpose()
            }
            Self::SqliteFamily(tx) => {
                let mut args = claim_cells(claim).to_vec();
                args.push(Cell::Integer(preview_now(tx).await?));
                let rows = tx.query("SELECT storage_key,mime,size_bytes FROM attachments WHERE workspace_id=?1 AND id=?2 AND preview_lease_token=?3 AND status='stored' AND preview_status='pending' AND preview_lease_expires_at>?4", &args).await?;
                rows.first()
                    .map(|row| {
                        Ok(PreviewInput {
                            storage_key: row.cell(0)?.string()?,
                            mime: row.cell(1)?.string()?,
                            size_bytes: row.cell(2)?.integer()?,
                        })
                    })
                    .transpose()
            }
        }
    }

    /// Journal only the current, unexpired claim. Preserve PG030's two-lease
    /// grace from now and also bound due_at by the actual claim expiry. A failed
    /// storage write, cancellation or later fence never removes this pointer.
    pub(crate) async fn journal_attachment_preview(
        &mut self,
        claim: &PreviewClaim,
        key: &str,
    ) -> Result<Option<Uuid>, sqlx::Error> {
        if !self.preview_attachment_is_live(claim).await? {
            return Ok(None);
        }
        if key.is_empty() {
            return Err(sqlx::Error::Protocol("empty preview key".into()));
        }
        let id = Uuid::now_v7();
        let changed = match self {
            Self::Postgres(tx) => sqlx::query("INSERT INTO fvoci.attachment_object_cleanups (id,workspace_id,attachment_id,storage_key,due_at) SELECT $4,$1,$2,$5,GREATEST(clock_timestamp()+($6 * interval '2 seconds'),preview_lease_expires_at) FROM fvoci.attachments WHERE workspace_id=$1 AND id=$2 AND preview_lease_token=$3 AND status='stored' AND preview_status='pending' AND preview_lease_expires_at>clock_timestamp()")
                .bind(claim.workspace_id).bind(claim.attachment_id).bind(claim.lease_token).bind(id).bind(key).bind(PREVIEW_LEASE_SECS).execute(&mut ***tx).await?.rows_affected(),
            Self::SqliteFamily(tx) => {
                let now = preview_now(tx).await?;
                let mut args = claim_cells(claim).to_vec();
                args.extend([Cell::uuid(id),Cell::text(key),Cell::Integer(now),Cell::Integer(now+i64::from(PREVIEW_LEASE_SECS)*2_000_000)]);
                tx.execute("INSERT INTO attachment_object_cleanups (id,workspace_id,attachment_id,storage_key,due_at) SELECT ?4,?1,?2,?5,max(?7,preview_lease_expires_at) FROM attachments WHERE workspace_id=?1 AND id=?2 AND preview_lease_token=?3 AND status='stored' AND preview_status='pending' AND preview_lease_expires_at>?6", &args).await?
            }
        };
        Ok((changed == 1).then_some(id))
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn publish_attachment_preview(
        &mut self,
        claim: &PreviewClaim,
        journal_id: Uuid,
        key: &str,
        width: u32,
        height: u32,
        bytes: u64,
    ) -> Result<bool, sqlx::Error> {
        self.require_preview_scope(claim.workspace_id).await?;
        if key.is_empty()
            || width == 0
            || height == 0
            || bytes == 0
            || bytes > 9_007_199_254_740_991
        {
            return Err(sqlx::Error::Protocol("invalid preview metadata".into()));
        }
        // Journal ownership must match the actual claim attachment and key;
        // a different attachment's surviving pointer is never consumable.
        let journal_present = match self {
            Self::Postgres(tx) => sqlx::query_scalar::<_,Uuid>("SELECT id FROM fvoci.attachment_object_cleanups WHERE id=$1 AND workspace_id=$2 AND attachment_id=$3 AND storage_key=$4 FOR UPDATE")
                .bind(journal_id).bind(claim.workspace_id).bind(claim.attachment_id).bind(key).fetch_optional(&mut ***tx).await?.is_some(),
            Self::SqliteFamily(tx) => !tx.query("SELECT id FROM attachment_object_cleanups WHERE id=?1 AND workspace_id=?2 AND attachment_id=?3 AND storage_key=?4", &[Cell::uuid(journal_id),Cell::uuid(claim.workspace_id),Cell::uuid(claim.attachment_id),Cell::text(key)]).await?.is_empty(),
        };
        if !journal_present || !self.preview_attachment_is_live(claim).await? {
            return Ok(false);
        }
        let variant = json!({"key":key,"width":width,"height":height,"bytes":bytes});
        let updated = match self {
            Self::Postgres(tx) => sqlx::query("UPDATE fvoci.attachments SET variants=jsonb_set(variants,'{preview}',$4::jsonb,true),preview_status='ok',preview_lease_token=NULL,preview_lease_expires_at=NULL WHERE workspace_id=$1 AND id=$2 AND preview_lease_token=$3 AND status='stored' AND preview_status='pending' AND preview_lease_expires_at>clock_timestamp() AND NOT (variants ? 'preview')")
                .bind(claim.workspace_id).bind(claim.attachment_id).bind(claim.lease_token).bind(variant).execute(&mut ***tx).await?.rows_affected(),
            Self::SqliteFamily(tx) => {
                let mut args = claim_cells(claim).to_vec();
                args.extend([Cell::json(&variant)?,Cell::Integer(preview_now(tx).await?)]);
                tx.execute("UPDATE attachments SET variants=json_set(variants,'$.preview',json(?4)),preview_status='ok',preview_lease_token=NULL,preview_lease_expires_at=NULL WHERE workspace_id=?1 AND id=?2 AND preview_lease_token=?3 AND status='stored' AND preview_status='pending' AND preview_lease_expires_at>?5 AND json_type(variants,'$.preview') IS NULL", &args).await?
            }
        };
        if updated != 1 {
            return Ok(false);
        }
        let removed = match self {
            Self::Postgres(tx) => {
                sqlx::query("DELETE FROM fvoci.attachment_object_cleanups WHERE id=$1")
                    .bind(journal_id)
                    .execute(&mut ***tx)
                    .await?
                    .rows_affected()
            }
            Self::SqliteFamily(tx) => {
                tx.execute(
                    "DELETE FROM attachment_object_cleanups WHERE id=?1",
                    &[Cell::uuid(journal_id)],
                )
                .await?
            }
        };
        if removed != 1 {
            return Err(sqlx::Error::Protocol(
                "locked preview journal disappeared".into(),
            ));
        }
        Ok(true)
    }

    pub(crate) async fn release_attachment_preview(
        &mut self,
        claim: &PreviewClaim,
    ) -> Result<bool, sqlx::Error> {
        self.finish_attachment_preview(claim, true).await
    }
    pub(crate) async fn fail_attachment_preview(
        &mut self,
        claim: &PreviewClaim,
    ) -> Result<bool, sqlx::Error> {
        self.finish_attachment_preview(claim, false).await
    }
    async fn finish_attachment_preview(
        &mut self,
        claim: &PreviewClaim,
        release: bool,
    ) -> Result<bool, sqlx::Error> {
        if !self.preview_attachment_is_live(claim).await? {
            return Ok(false);
        }
        // Original PG policy allows an expired but unreplaced token to release
        // or fail; completion/load/publication still require live expiry.
        let updated = match self {
            Self::Postgres(tx) => {
                let sql = if release {
                    "UPDATE fvoci.attachments SET preview_lease_token=NULL,preview_lease_expires_at=NULL,preview_attempts=GREATEST(preview_attempts-1,0) WHERE workspace_id=$1 AND id=$2 AND preview_lease_token=$3 AND status='stored' AND preview_status='pending'"
                } else {
                    "UPDATE fvoci.attachments SET preview_status='failed',preview_lease_token=NULL,preview_lease_expires_at=NULL WHERE workspace_id=$1 AND id=$2 AND preview_lease_token=$3 AND status='stored' AND preview_status='pending'"
                };
                sqlx::query(sql)
                    .bind(claim.workspace_id)
                    .bind(claim.attachment_id)
                    .bind(claim.lease_token)
                    .execute(&mut ***tx)
                    .await?
                    .rows_affected()
            }
            Self::SqliteFamily(tx) => {
                let sql = if release {
                    "UPDATE attachments SET preview_lease_token=NULL,preview_lease_expires_at=NULL,preview_attempts=max(preview_attempts-1,0) WHERE workspace_id=?1 AND id=?2 AND preview_lease_token=?3 AND status='stored' AND preview_status='pending'"
                } else {
                    "UPDATE attachments SET preview_status='failed',preview_lease_token=NULL,preview_lease_expires_at=NULL WHERE workspace_id=?1 AND id=?2 AND preview_lease_token=?3 AND status='stored' AND preview_status='pending'"
                };
                tx.execute(sql, &claim_cells(claim)).await?
            }
        };
        Ok(updated == 1)
    }
}

fn claim_cells(claim: &PreviewClaim) -> [Cell; 3] {
    [
        Cell::uuid(claim.workspace_id),
        Cell::uuid(claim.attachment_id),
        Cell::uuid(claim.lease_token),
    ]
}
async fn preview_now(tx: &mut super::backend::FamilyTx) -> Result<i64, sqlx::Error> {
    let rows = tx
        .query(
            "SELECT unixepoch()*1000000+CAST(substr(strftime('%f','now'),4,3) AS INTEGER)*1000",
            &[],
        )
        .await?;
    rows[0].cell(0)?.integer()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use sqlx::SqlitePool;
    use std::path::PathBuf;

    pub(crate) struct Fixture {
        pub root: PathBuf,
        pub path: PathBuf,
        pub pool: SqlitePool,
        pub backend: Backend,
        pub workspace: Uuid,
        pub user: Uuid,
        pub document: Uuid,
    }
    impl Fixture {
        pub async fn new() -> Self {
            let root = std::env::temp_dir().join(format!("fvoci-s31-{}", Uuid::now_v7()));
            std::fs::create_dir_all(&root).unwrap();
            let path = root.join("app.sqlite");
            crate::db::migrate::run_sqlite_migrations(&path)
                .await
                .unwrap();
            let pool = crate::db::pool::connect_sqlite_app(&path, 1).await.unwrap();
            let backend = Backend::Sqlite(pool.clone());
            let gate = crate::db::migrate::assert_sqlite_schema_current(&backend)
                .await
                .unwrap();
            assert_eq!(gate.applied_steps, 5);
            // The accepted full schema is commit254f3f2 + compiled001–003,
            // not a numeric254 object inventory. The maintained gate above
            // compares every canonical object name/type/definition and receipt.
            let (total, user_objects): (i64, i64) =
                sqlx::query_as("SELECT count(*),sum(name NOT GLOB 'sqlite_*') FROM sqlite_schema")
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            println!(
                "S31 prepared schema steps={} sha256={} catalog_total={} user_objects={}",
                gate.applied_steps, gate.schema_sha256, total, user_objects
            );
            let pin: (String,String,i64) = sqlx::query_as("SELECT sqlite_version(),sqlite_source_id(),(SELECT foreign_keys FROM pragma_foreign_keys)").fetch_one(&pool).await.unwrap();
            assert_eq!(
                pin,
                (
                    crate::db::pool::SQLITE_VERSION.into(),
                    crate::db::pool::SQLITE_SOURCE_ID.into(),
                    1
                )
            );
            let f = Self {
                root,
                path,
                pool,
                backend,
                workspace: Uuid::now_v7(),
                user: Uuid::now_v7(),
                document: Uuid::now_v7(),
            };
            sqlx::query(
                "INSERT INTO users(id,email,given_name) VALUES(?1,'s31@example.test','S31')",
            )
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
            sqlx::query("INSERT INTO workspaces(id,slug,name) VALUES(?1,'s31','S31')")
                .bind(f.workspace.as_bytes().as_slice())
                .execute(&f.pool)
                .await
                .unwrap();
            sqlx::query("INSERT INTO memberships(workspace_id,user_id,role) VALUES(?1,?2,'owner')")
                .bind(f.workspace.as_bytes().as_slice())
                .bind(f.user.as_bytes().as_slice())
                .execute(&f.pool)
                .await
                .unwrap();
            sqlx::query("INSERT INTO documents(id,workspace_id,title,path,sort_key,number,status,schema_version,created_by,content_json) VALUES(?1,?2,'S31',?3,'V',1,'published',2,?4,'{\"type\":\"doc\",\"content\":[]}')").bind(f.document.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(f.document.simple().to_string()).bind(f.user.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
            f
        }
        pub async fn attachment(&self, size: i64, mime: &str) -> (Uuid, String) {
            let id = Uuid::now_v7();
            let key = Uuid::now_v7().to_string();
            sqlx::query("INSERT INTO attachments(id,workspace_id,document_id,uploader_id,status,name,mime,size_bytes,reserved_size_bytes,storage_key,image,completed_at,preview_status) VALUES(?1,?2,?3,?4,'stored','s31.png',?5,?6,?6,?7,1,1,'pending')")
                .bind(id.as_bytes().as_slice()).bind(self.workspace.as_bytes().as_slice()).bind(self.document.as_bytes().as_slice()).bind(self.user.as_bytes().as_slice()).bind(mime).bind(size).bind(&key).execute(&self.pool).await.unwrap();
            (id, key)
        }
        pub async fn task_attachment(&self) -> (Uuid, Uuid) {
            let project = Uuid::now_v7();
            let status = Uuid::now_v7();
            let workflow = Uuid::now_v7();
            let task = Uuid::now_v7();
            sqlx::query("INSERT INTO projects(id,workspace_id,key,name,visibility,created_by) VALUES(?1,?2,'S31','S31','workspace',?3)").bind(project.as_bytes().as_slice()).bind(self.workspace.as_bytes().as_slice()).bind(self.user.as_bytes().as_slice()).execute(&self.pool).await.unwrap();
            sqlx::query("INSERT INTO workflows(id,workspace_id,project_id) VALUES(?1,?2,?3)")
                .bind(workflow.as_bytes().as_slice())
                .bind(self.workspace.as_bytes().as_slice())
                .bind(project.as_bytes().as_slice())
                .execute(&self.pool)
                .await
                .unwrap();
            sqlx::query("INSERT INTO statuses(id,workspace_id,project_id,workflow_id,name,category,sort_key) VALUES(?1,?2,?3,?4,'Todo','todo','V')").bind(status.as_bytes().as_slice()).bind(self.workspace.as_bytes().as_slice()).bind(project.as_bytes().as_slice()).bind(workflow.as_bytes().as_slice()).execute(&self.pool).await.unwrap();
            sqlx::query("INSERT INTO tasks(id,workspace_id,project_id,number,title,status_id,content_json,created_by) VALUES(?1,?2,?3,1,'S31',?4,'{}',?5)").bind(task.as_bytes().as_slice()).bind(self.workspace.as_bytes().as_slice()).bind(project.as_bytes().as_slice()).bind(status.as_bytes().as_slice()).bind(self.user.as_bytes().as_slice()).execute(&self.pool).await.unwrap();
            let (id, _) = self.attachment(10, "image/png").await;
            sqlx::query("UPDATE attachments SET document_id=NULL,task_id=?2 WHERE id=?1")
                .bind(id.as_bytes().as_slice())
                .bind(task.as_bytes().as_slice())
                .execute(&self.pool)
                .await
                .unwrap();
            (id, task)
        }
        pub async fn row(&self, id: Uuid) -> (String, String, i64, Option<Vec<u8>>) {
            sqlx::query_as("SELECT preview_status,variants,preview_attempts,preview_lease_token FROM attachments WHERE id=?1").bind(id.as_bytes().as_slice()).fetch_one(&self.pool).await.unwrap()
        }
        pub async fn journals(&self) -> Vec<(Vec<u8>, String, i64)> {
            sqlx::query_as(
                "SELECT id,storage_key,due_at FROM attachment_object_cleanups ORDER BY id",
            )
            .fetch_all(&self.pool)
            .await
            .unwrap()
        }
        pub async fn expire(&self, id: Uuid) {
            sqlx::query("UPDATE attachments SET preview_lease_expires_at=1 WHERE id=?1")
                .bind(id.as_bytes().as_slice())
                .execute(&self.pool)
                .await
                .unwrap();
        }
        pub async fn close(self) {
            self.backend.close().await.unwrap();
            std::fs::remove_dir_all(self.root).unwrap();
        }
    }

    #[tokio::test]
    async fn preview_selected_claim_fences_stale_and_expired_tokens() {
        let f = Fixture::new().await;
        let (id, _) = f.attachment(10, "image/png").await;
        let old = claim_preview_backend(&f.backend).await.unwrap().unwrap();
        let key = Uuid::now_v7().to_string();
        let journal = journal_preview_key_backend(&f.backend, &old, &key)
            .await
            .unwrap()
            .unwrap();
        let due = f.journals().await[0].2;
        let expiry: i64 =
            sqlx::query_scalar("SELECT preview_lease_expires_at FROM attachments WHERE id=?1")
                .bind(id.as_bytes().as_slice())
                .fetch_one(&f.pool)
                .await
                .unwrap();
        assert!(due >= expiry);
        assert!(claim_preview_backend(&f.backend).await.unwrap().is_none());
        f.expire(id).await;
        assert!(load_preview_input_backend(&f.backend, &old)
            .await
            .unwrap()
            .is_none());
        assert!(
            journal_preview_key_backend(&f.backend, &old, &Uuid::now_v7().to_string())
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            !publish_preview_backend(&f.backend, &old, journal, &key, 1, 1, 10)
                .await
                .unwrap()
        );
        // Exact original release policy: expired but not replaced still owns token.
        assert!(release_preview_backend(&f.backend, &old).await.unwrap());
        let current = claim_preview_backend(&f.backend).await.unwrap().unwrap();
        assert_ne!(old.lease_token, current.lease_token);
        assert!(!release_preview_backend(&f.backend, &old).await.unwrap());
        assert!(!fail_preview_backend(&f.backend, &old).await.unwrap());
        assert!(
            !publish_preview_backend(&f.backend, &old, journal, &key, 1, 1, 10)
                .await
                .unwrap()
        );
        assert_eq!(f.journals().await.len(), 1);
        let newkey = Uuid::now_v7().to_string();
        let newjournal = journal_preview_key_backend(&f.backend, &current, &newkey)
            .await
            .unwrap()
            .unwrap();
        assert!(
            !publish_preview_backend(&f.backend, &current, journal, &newkey, 1, 1, 10)
                .await
                .unwrap()
        );
        assert!(
            publish_preview_backend(&f.backend, &current, newjournal, &newkey, 1, 1, 10)
                .await
                .unwrap()
        );
        assert_eq!(f.journals().await.len(), 1);
        assert_eq!(f.row(id).await.0, "ok");
        f.close().await;
    }

    #[tokio::test]
    async fn preview_selected_requires_scope_system_and_writer_before_any_change() {
        let f = Fixture::new().await;
        let (id, _) = f.attachment(10, "image/png").await;
        let mut tx = f.backend.begin_write().await.unwrap();
        assert!(tx.operation().claim_attachment_preview().await.is_err());
        tx.rollback().await.unwrap();
        let claim = claim_preview_backend(&f.backend).await.unwrap().unwrap();
        let before = f.row(id).await;
        let mut tx = f.backend.begin_read().await.unwrap();
        tx.operation().set_tenant(f.workspace).await.unwrap();
        assert!(tx
            .operation()
            .journal_attachment_preview(&claim, "read")
            .await
            .is_err());
        tx.rollback().await.unwrap();
        let mut tx = f.backend.begin_write().await.unwrap();
        tx.operation().set_tenant(Uuid::now_v7()).await.unwrap();
        assert!(tx
            .operation()
            .release_attachment_preview(&claim)
            .await
            .is_err());
        tx.rollback().await.unwrap();
        let mut tx = f.backend.begin_write().await.unwrap();
        tx.operation().set_tenant(f.workspace).await.unwrap();
        tx.operation().set_system().await.unwrap();
        assert!(tx
            .operation()
            .journal_attachment_preview(&claim, "system")
            .await
            .is_err());
        tx.rollback().await.unwrap();
        assert_eq!(before, f.row(id).await);
        assert!(f.journals().await.is_empty());
        let mut wrong = claim.clone();
        wrong.workspace_id = Uuid::now_v7();
        assert!(load_preview_input_backend(&f.backend, &wrong)
            .await
            .unwrap()
            .is_none());
        assert!(journal_preview_key_backend(&f.backend, &wrong, "wrong")
            .await
            .unwrap()
            .is_none());
        assert_eq!(before, f.row(id).await);
        // No existing SQLite missing-object regression is present at this
        // bounded base. Prove the maintained full gate rejects a real missing
        // object, rather than accepting migration markers or a row-only model.
        sqlx::query("DROP INDEX attachments_preview_pending_idx")
            .execute(&f.pool)
            .await
            .unwrap();
        let invalid = crate::db::migrate::assert_sqlite_schema_current(&f.backend)
            .await
            .unwrap_err();
        assert!(
            invalid.to_string().contains("schema definitions differ"),
            "{invalid}"
        );
        f.close().await;
    }

    #[tokio::test]
    async fn preview_selected_parent_revocation_journal_gc_order_and_rollback() {
        let f = Fixture::new().await;
        let (id, _) = f.attachment(10, "image/png").await;
        let claim = claim_preview_backend(&f.backend).await.unwrap().unwrap();
        let key = Uuid::now_v7().to_string();
        let journal = journal_preview_key_backend(&f.backend, &claim, &key)
            .await
            .unwrap()
            .unwrap();
        // GC wins its writer transaction and removes the pointer: no publication.
        sqlx::query("DELETE FROM attachment_object_cleanups WHERE id=?1")
            .bind(journal.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(
            !publish_preview_backend(&f.backend, &claim, journal, &key, 1, 1, 10)
                .await
                .unwrap()
        );
        let journal = journal_preview_key_backend(&f.backend, &claim, &key)
            .await
            .unwrap()
            .unwrap();
        sqlx::query("UPDATE documents SET deleted_at=1 WHERE id=?1")
            .bind(f.document.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let before = f.row(id).await;
        assert!(
            !publish_preview_backend(&f.backend, &claim, journal, &key, 1, 1, 10)
                .await
                .unwrap()
        );
        assert!(!fail_preview_backend(&f.backend, &claim).await.unwrap());
        assert!(!release_preview_backend(&f.backend, &claim).await.unwrap());
        assert_eq!(before, f.row(id).await);
        assert_eq!(f.journals().await.len(), 1);
        sqlx::query("UPDATE documents SET deleted_at=NULL WHERE id=?1")
            .bind(f.document.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let mut tx = f.backend.begin_write().await.unwrap();
        tx.operation().set_tenant(f.workspace).await.unwrap();
        assert!(tx
            .operation()
            .publish_attachment_preview(&claim, journal, &key, 1, 1, 10)
            .await
            .unwrap());
        tx.rollback().await.unwrap();
        assert_eq!(before, f.row(id).await);
        assert_eq!(f.journals().await.len(), 1);
        sqlx::query("UPDATE workspaces SET deleted_at=1 WHERE id=?1")
            .bind(f.workspace.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(
            !publish_preview_backend(&f.backend, &claim, journal, &key, 1, 1, 10)
                .await
                .unwrap()
        );
        assert_eq!(before, f.row(id).await);
        f.close().await;
    }

    #[tokio::test]
    async fn preview_selected_cancel_during_publication_writer_wait_rolls_back() {
        let f = Fixture::new().await;
        let (id, _) = f.attachment(10, "image/png").await;
        let claim = claim_preview_backend(&f.backend).await.unwrap().unwrap();
        let key = Uuid::now_v7().to_string();
        let journal = journal_preview_key_backend(&f.backend, &claim, &key)
            .await
            .unwrap()
            .unwrap();
        let before = f.row(id).await;
        let journals = f.journals().await;
        let holder = crate::db::pool::connect_sqlite_app(&f.path, 1)
            .await
            .unwrap();
        let lock = holder.begin_with("BEGIN IMMEDIATE").await.unwrap();
        let backend = f.backend.clone();
        let cancel = tokio_util::sync::CancellationToken::new();
        let task_cancel = cancel.clone();
        let task = tokio::spawn(async move {
            publish_preview_backend_with_cancel(
                &backend,
                &claim,
                journal,
                &key,
                1,
                1,
                10,
                Some(&task_cancel),
            )
            .await
        });
        // The app connection is owned by BEGIN, waiting for the other writer.
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while f.pool.num_idle() != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(!task.is_finished());
        cancel.cancel();
        lock.rollback().await.unwrap();
        assert!(!task.await.unwrap().unwrap());
        assert_eq!(before, f.row(id).await);
        assert_eq!(journals, f.journals().await);
        holder.close().await;
        f.close().await;
    }

    #[tokio::test]
    async fn preview_selected_exact_three_attempt_exhaustion() {
        let f = Fixture::new().await;
        let (id, _) = f.attachment(10, "image/png").await;
        for attempt in 1..=3 {
            let claim = claim_preview_backend(&f.backend).await.unwrap().unwrap();
            assert_eq!(claim.attempt, attempt);
            f.expire(id).await;
        }
        assert!(claim_preview_backend(&f.backend).await.unwrap().is_none());
        let row = f.row(id).await;
        assert_eq!(row.0, "failed");
        assert_eq!(row.2, 3);
        assert!(row.3.is_none());
        f.close().await;
    }
}

#[cfg(test)]
mod task_tests {
    use super::*;
    #[tokio::test]
    async fn preview_selected_current_task_parent_and_deleted_task_refusal() {
        let f = tests::Fixture::new().await;
        let (id, task) = f.task_attachment().await;
        let claim = claim_preview_backend(&f.backend).await.unwrap().unwrap();
        assert_eq!(claim.attachment_id, id);
        assert!(load_preview_input_backend(&f.backend, &claim)
            .await
            .unwrap()
            .is_some());
        let key = Uuid::now_v7().to_string();
        let journal = journal_preview_key_backend(&f.backend, &claim, &key)
            .await
            .unwrap()
            .unwrap();
        sqlx::query("UPDATE tasks SET deleted_at=1 WHERE id=?1")
            .bind(task.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let before = f.row(id).await;
        assert!(
            !publish_preview_backend(&f.backend, &claim, journal, &key, 1, 1, 10)
                .await
                .unwrap()
        );
        assert!(!release_preview_backend(&f.backend, &claim).await.unwrap());
        assert_eq!(before, f.row(id).await);
        sqlx::query("UPDATE tasks SET deleted_at=NULL WHERE id=?1")
            .bind(task.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(
            publish_preview_backend(&f.backend, &claim, journal, &key, 1, 1, 10)
                .await
                .unwrap()
        );
        f.close().await;
    }
}
