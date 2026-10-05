//! Trash retention purge of documents (source `purgeTrashedDocument(s)` in
//! `packages/core/src/document.ts` at `393795261322b916e588043cf94feca999175843`).
//!
//! A document trashed more than `TRASH_RETENTION_DAYS + TRASH_PURGE_MARGIN_DAYS`
//! ago (database clock) is deleted with its attachment rows, revisions and
//! cascading rows (collab state, comments, members, stars, share links, tag
//! assignments, collection items). Differences from the source:
//! - original attachment objects are removed through `ObjectStorage::purge_key`
//!   *before* the DB rows. The purge runs in three steps so no workspace lock
//!   is held during storage I/O: (1) tree lock + document row recheck + collect
//!   keys, commit; (2) delete the objects without any lock, each bounded by
//!   `PURGE_KEY_TIMEOUT` and the sweep deadline; (3) tree lock again, recheck,
//!   delete the rows and append `document.purged` in one transaction. A storage
//!   error or timeout keeps every row for the next sweep; an already missing
//!   object counts as deleted.
//! - deleting the rows also journals each original and preview key through
//!   the migration-030 trigger (`attachment_object_cleanups`), so
//!   `reclaim_attachment_objects` purges the originals again (as already
//!   missing) and is what removes preview objects, which step (1) does not
//!   collect. Storage-first predates that journal and stays; a DB-only purge
//!   would hand every key to the reclaimer's batched drain
//!   (`jobs::OBJECT_CLEANUP_BATCH` rows per upload-GC run).
//! - restore and the trash lists refuse rows past `TRASH_RETENTION_DAYS`
//!   (`trash_expired`), and the purge only selects rows a margin day older, so
//!   between steps (1) and (3), or after a crash with some objects deleted, no
//!   document whose attachments may be gone can be resurrected.
//!
//! Children are purged first (deepest path first); a document that still has
//! any child row (a later-trashed or live child) waits. Documents with an
//! in-flight upload (`uploading`/`assembling`) wait for the stale-upload GC,
//! which owns the upload session lock.

use std::time::Duration;

use serde_json::json;
use sqlx::{PgPool, Postgres, Transaction};
use tokio::time::Instant;
use uuid::Uuid;

use crate::attachments::ObjectStorage;
use crate::db::context::{lock_tree, set_system, set_tenant};
use crate::db::documents::{TRASH_PURGE_MARGIN_DAYS, TRASH_RETENTION_DAYS};
use crate::db::identity::{append_event_channel, EventAppend};
use crate::db::backend::OperationTx;
use crate::db::codec::Cell;

/// Upper bound for one attachment key's storage cleanup (multipart aborts and
/// the object delete); a slow backend fails that document for this sweep.
pub const PURGE_KEY_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrashPurgeOutcome {
    Purged {
        storage_deleted: u32,
    },
    /// Restored, already gone, not yet expired, or waiting on children/uploads.
    Skipped,
    /// A storage key could not be removed (error or `PURGE_KEY_TIMEOUT`);
    /// every row was kept.
    StorageFailed,
    /// The sweep deadline passed during the storage step; every row was kept
    /// and the next sweep continues (objects already removed count as deleted).
    Deferred,
}

fn purge_after_days() -> i32 {
    TRASH_RETENTION_DAYS + TRASH_PURGE_MARGIN_DAYS
}

impl OperationTx<'_, '_> {
    pub(crate) async fn maintenance_live_workspace_ids(
        &mut self,
    ) -> Result<Vec<Uuid>, sqlx::Error> {
        match self {
            Self::Postgres(tx) => sqlx::query_scalar("SELECT id FROM fvoci.workspaces WHERE deleted_at IS NULL ORDER BY created_at ASC, id ASC").fetch_all(&mut ***tx).await,
            Self::SqliteFamily(tx) => {
                tx.require_system_context()?;
                tx.query("SELECT id FROM workspaces WHERE deleted_at IS NULL ORDER BY created_at, id", &[]).await?
                    .iter().map(|row| row.cell(0)?.id()).collect()
            }
        }
    }

    pub(crate) async fn maintenance_expired_documents(
        &mut self,
        workspace: Uuid,
        exclude: &[Uuid],
        limit: i64,
    ) -> Result<Vec<Uuid>, sqlx::Error> {
        match self {
            Self::Postgres(tx) => sqlx::query_scalar("SELECT id FROM fvoci.documents WHERE workspace_id=$1 AND deleted_at IS NOT NULL AND deleted_at <= now()-make_interval(days=>$2) AND NOT (id=ANY($3)) ORDER BY (length(path)-length(replace(path,'.',''))) DESC,deleted_at ASC,id ASC LIMIT $4")
                .bind(workspace).bind(purge_after_days()).bind(exclude).bind(limit).fetch_all(&mut ***tx).await,
            Self::SqliteFamily(tx) => {
                tx.require_tenant(workspace)?;
                let exclude = json!(exclude.iter().map(|id| id.simple().to_string()).collect::<Vec<_>>());
                tx.query("SELECT id FROM documents WHERE workspace_id=?1 AND deleted_at IS NOT NULL AND deleted_at <= (unixepoch()*1000000+CAST(substr(strftime('%f','now'),4,3) AS INTEGER)*1000)-?2 AND NOT EXISTS(SELECT 1 FROM json_each(?3) x WHERE x.value=lower(hex(documents.id))) ORDER BY (length(path)-length(replace(path,'.',''))) DESC,deleted_at,id LIMIT ?4", &[Cell::uuid(workspace), Cell::Integer(i64::from(purge_after_days())*86_400_000_000), Cell::json(&exclude)?, Cell::Integer(limit)])
                    .await?.iter().map(|row| row.cell(0)?.id()).collect()
            }
        }
    }

    pub(crate) async fn maintenance_document_purge_keys(
        &mut self,
        workspace: Uuid,
        document: Uuid,
    ) -> Result<Option<Vec<String>>, sqlx::Error> {
        match self {
            Self::Postgres(tx) => lock_purgeable(tx, workspace, document).await,
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                tx.require_tenant(workspace)?;
                let expired = tx.query("SELECT 1 FROM documents WHERE workspace_id=?1 AND id=?2 AND deleted_at IS NOT NULL AND deleted_at <= (unixepoch()*1000000+CAST(substr(strftime('%f','now'),4,3) AS INTEGER)*1000)-?3 AND NOT EXISTS(SELECT 1 FROM documents c WHERE c.workspace_id=?1 AND c.parent_id=?2)", &[Cell::uuid(workspace), Cell::uuid(document), Cell::Integer(i64::from(purge_after_days())*86_400_000_000)]).await?;
                if expired.is_empty() {
                    return Ok(None);
                }
                let rows = tx.query("SELECT storage_key,status FROM attachments WHERE workspace_id=?1 AND document_id=?2 ORDER BY id", &[Cell::uuid(workspace), Cell::uuid(document)]).await?;
                let mut keys = Vec::with_capacity(rows.len());
                for row in rows {
                    let status = row.cell(1)?.string()?;
                    if status == "uploading" || status == "assembling" {
                        return Ok(None);
                    }
                    keys.push(row.cell(0)?.string()?);
                }
                Ok(Some(keys))
            }
        }
    }

    /// Call only after this same owning writer validated current purgeability.
    /// Exact parent rows become the attachment owner's explicit exclusion set.
    pub(crate) async fn maintenance_document_purge_attachment_ids(
        &mut self,
        workspace: Uuid,
        document: Uuid,
    ) -> Result<Vec<Uuid>, sqlx::Error> {
        let Self::SqliteFamily(tx) = self else {
            return Err(sqlx::Error::Protocol(
                "family doomed-row reader requires its actual family writer".into(),
            ));
        };
        tx.require_writer()?;
        tx.require_tenant(workspace)?;
        tx.query(
            "SELECT id FROM attachments WHERE workspace_id=?1 AND document_id=?2 ORDER BY id",
            &[Cell::uuid(workspace), Cell::uuid(document)],
        )
        .await?
        .iter()
        .map(|row| row.cell(0)?.id())
        .collect()
    }

    pub(crate) async fn maintenance_delete_document(
        &mut self,
        workspace: Uuid,
        document: Uuid,
    ) -> Result<bool, sqlx::Error> {
        let deleted = match self {
            Self::Postgres(tx) => {
                sqlx::query(
                    "DELETE FROM fvoci.attachments WHERE workspace_id=$1 AND document_id=$2",
                )
                .bind(workspace)
                .bind(document)
                .execute(&mut ***tx)
                .await?;
                sqlx::query("DELETE FROM fvoci.revisions WHERE workspace_id=$1 AND target_kind='document' AND target_id=$2").bind(workspace).bind(document).execute(&mut ***tx).await?;
                sqlx::query("DELETE FROM fvoci.documents WHERE workspace_id=$1 AND id=$2 AND deleted_at IS NOT NULL").bind(workspace).bind(document).execute(&mut ***tx).await?.rows_affected()
            }
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                tx.require_tenant(workspace)?;
                let parameters = [Cell::uuid(workspace), Cell::uuid(document)];
                tx.execute(
                    "DELETE FROM attachments WHERE workspace_id=?1 AND document_id=?2",
                    &parameters,
                )
                .await?;
                tx.execute("DELETE FROM revisions WHERE workspace_id=?1 AND target_kind='document' AND target_id=?2", &parameters).await?;
                tx.execute("DELETE FROM documents WHERE workspace_id=?1 AND id=?2 AND deleted_at IS NOT NULL", &parameters).await?
            }
        };
        if deleted != 1 {
            return Err(sqlx::Error::Protocol(
                "documents.purge: current document was not deleted; roll back attachment/history effects".into(),
            ));
        }
        self.append_event_channel(
            EventAppend {
                id: Uuid::now_v7(),
                workspace_id: Some(workspace),
                actor_user_id: None,
                verb: "document.purged".to_string(),
                target_type: Some("document".to_string()),
                target_id: Some(document),
                payload: json!({"documentId":document.to_string()}),
            },
            "system",
        )
        .await?;
        Ok(true)
    }
}

/// Live workspaces, oldest first (tenant scans run per workspace: documents RLS
/// has no system bypass).
pub async fn list_live_workspace_ids(pool: &PgPool) -> Result<Vec<Uuid>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_system(&mut tx).await?;
    let rows: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM fvoci.workspaces WHERE deleted_at IS NULL ORDER BY created_at ASC, id ASC",
    )
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// Purgeable trashed documents of one workspace, deepest first, leaving out
/// `exclude` (ids this sweep already examined), so a run always advances past
/// documents that failed or must wait.
pub async fn list_expired_trashed_documents(
    pool: &PgPool,
    workspace_id: Uuid,
    exclude: &[Uuid],
    limit: i64,
) -> Result<Vec<Uuid>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let rows: Vec<(Uuid,)> = sqlx::query_as(
        r#"
        SELECT id
        FROM fvoci.documents
        WHERE workspace_id = $1 AND deleted_at IS NOT NULL
          AND deleted_at <= now() - make_interval(days => $2)
          AND NOT (id = ANY($3))
        ORDER BY (length(path) - length(replace(path, '.', ''))) DESC, deleted_at ASC, id ASC
        LIMIT $4
        "#,
    )
    .bind(workspace_id)
    .bind(purge_after_days())
    .bind(exclude)
    .bind(limit)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// Under the workspace tree lock: locks the document and its attachment rows
/// and returns the attachment keys when the document is still purgeable
/// (trashed past the purge age, no child row, no in-flight upload).
async fn lock_purgeable(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    document_id: Uuid,
) -> Result<Option<Vec<String>>, sqlx::Error> {
    lock_tree(tx, workspace_id).await?;
    let expired: Option<(bool,)> = sqlx::query_as(
        r#"
        SELECT deleted_at IS NOT NULL AND deleted_at <= now() - make_interval(days => $3)
        FROM fvoci.documents
        WHERE workspace_id = $1 AND id = $2
        FOR UPDATE
        "#,
    )
    .bind(workspace_id)
    .bind(document_id)
    .bind(purge_after_days())
    .fetch_optional(&mut **tx)
    .await?;
    if expired != Some((true,)) {
        return Ok(None);
    }
    let has_child: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM fvoci.documents WHERE workspace_id = $1 AND parent_id = $2 LIMIT 1",
    )
    .bind(workspace_id)
    .bind(document_id)
    .fetch_optional(&mut **tx)
    .await?;
    if has_child.is_some() {
        return Ok(None);
    }
    let attachments: Vec<(String, String)> = sqlx::query_as(
        r#"
        SELECT storage_key, status
        FROM fvoci.attachments
        WHERE workspace_id = $1 AND document_id = $2
        ORDER BY id
        FOR UPDATE
        "#,
    )
    .bind(workspace_id)
    .bind(document_id)
    .fetch_all(&mut **tx)
    .await?;
    if attachments
        .iter()
        .any(|(_, status)| status == "uploading" || status == "assembling")
    {
        return Ok(None);
    }
    Ok(Some(attachments.into_iter().map(|(key, _)| key).collect()))
}

/// Purges one expired trashed document (see the module docs for the steps).
/// `deadline` bounds the storage step; the DB steps are single short
/// transactions.
pub async fn purge_trashed_document(
    pool: &PgPool,
    storage: &ObjectStorage,
    workspace_id: Uuid,
    document_id: Uuid,
    deadline: Instant,
) -> Result<TrashPurgeOutcome, sqlx::Error> {
    // (1) Recheck and collect keys; the tree lock is released on commit.
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let Some(keys) = lock_purgeable(&mut tx, workspace_id, document_id).await? else {
        tx.rollback().await?;
        return Ok(TrashPurgeOutcome::Skipped);
    };
    tx.commit().await?;

    #[cfg(feature = "db-tests")]
    test_hooks::pause_before_storage(document_id).await;

    // (2) Storage without any DB lock. Restore refuses the row (expired), so a
    // failure here only leaves an unrestorable row for the next sweep.
    let mut storage_deleted = 0u32;
    for key in &keys {
        if Instant::now() >= deadline {
            return Ok(TrashPurgeOutcome::Deferred);
        }
        let failure = match tokio::time::timeout(PURGE_KEY_TIMEOUT, storage.purge_key(key)).await {
            Ok(Ok(())) => None,
            Ok(Err(err)) => Some(err.to_string()),
            Err(_) => Some(format!("timed out after {}s", PURGE_KEY_TIMEOUT.as_secs())),
        };
        if let Some(error) = failure {
            tracing::warn!(
                workspace_id = %workspace_id,
                document_id = %document_id,
                error = %error,
                "maintenance.document_purge_storage_failed"
            );
            return Ok(TrashPurgeOutcome::StorageFailed);
        }
        storage_deleted += 1;
    }

    // (3) Recheck under the tree lock and delete the rows. An attachment that
    // appeared after step (1) was not cleaned yet: leave it to the next sweep.
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let Some(current) = lock_purgeable(&mut tx, workspace_id, document_id).await? else {
        tx.rollback().await?;
        return Ok(TrashPurgeOutcome::Skipped);
    };
    if current.iter().any(|key| !keys.contains(key)) {
        tx.rollback().await?;
        return Ok(TrashPurgeOutcome::Skipped);
    }
    sqlx::query("DELETE FROM fvoci.attachments WHERE workspace_id = $1 AND document_id = $2")
        .bind(workspace_id)
        .bind(document_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "DELETE FROM fvoci.revisions WHERE workspace_id = $1 AND target_kind = 'document' AND target_id = $2",
    )
    .bind(workspace_id)
    .bind(document_id)
    .execute(&mut *tx)
    .await?;
    let deleted = sqlx::query(
        "DELETE FROM fvoci.documents WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NOT NULL",
    )
    .bind(workspace_id)
    .bind(document_id)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if deleted != 1 {
        tx.rollback().await?;
        return Ok(TrashPurgeOutcome::Skipped);
    }
    // Source emits the purge from the maintenance worker: channel `system`.
    append_event_channel(
        &mut tx,
        EventAppend {
            id: Uuid::now_v7(),
            workspace_id: Some(workspace_id),
            actor_user_id: None,
            verb: "document.purged".to_string(),
            target_type: Some("document".to_string()),
            target_id: Some(document_id),
            payload: json!({ "documentId": document_id.to_string() }),
        },
        "system",
    )
    .await?;
    tx.commit().await?;
    Ok(TrashPurgeOutcome::Purged { storage_deleted })
}

#[cfg(feature = "db-tests")]
pub mod test_hooks {
    //! Test-only barrier after the recheck commit, before the storage step.
    use std::collections::HashMap;
    use std::sync::{LazyLock, Mutex};

    use tokio::sync::oneshot;
    use uuid::Uuid;

    type Barrier = (oneshot::Sender<()>, oneshot::Receiver<()>);
    static BARRIERS: LazyLock<Mutex<HashMap<Uuid, Barrier>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));

    /// Returns (reached, proceed): the purge signals `reached` before touching
    /// storage and waits for `proceed`.
    pub fn arm_before_storage(document_id: Uuid) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
        let (reached_tx, reached_rx) = oneshot::channel();
        let (proceed_tx, proceed_rx) = oneshot::channel();
        BARRIERS
            .lock()
            .expect("barrier map")
            .insert(document_id, (reached_tx, proceed_rx));
        (reached_rx, proceed_tx)
    }

    pub(crate) async fn pause_before_storage(document_id: Uuid) {
        let barrier = BARRIERS.lock().expect("barrier map").remove(&document_id);
        if let Some((reached, proceed)) = barrier {
            let _ = reached.send(());
            let _ = proceed.await;
        }
    }
}
