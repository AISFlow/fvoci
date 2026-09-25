//! Trash retention purge of documents (source `purgeTrashedDocument(s)` in
//! `packages/core/src/document.ts` at `393795261322b916e588043cf94feca999175843`).
//!
//! A document trashed more than `TRASH_RETENTION_DAYS` ago is deleted with its
//! attachment rows, revisions and cascading rows (collab state, comments,
//! members, stars, share links). Differences from the source, both for storage
//! crash-safety without a cleanup journal table:
//! - attachment objects are removed through `ObjectStorage::purge_key` *before*
//!   the DB rows, inside the purge transaction that holds the workspace tree
//!   lock and the document row, so restore/move cannot interleave. A storage
//!   error rolls back and keeps every row for the next sweep; an already missing
//!   object counts as deleted.
//! - restore refuses rows past the retention (`trash_retention_cutoff`), so a
//!   crash after some objects were deleted cannot resurrect a document whose
//!   attachments are gone.
//!
//! Children are purged first (deepest path first); a document that still has
//! any child row (a later-trashed or live child) waits. Documents with an
//! in-flight upload (`uploading`/`assembling`) wait for the stale-upload GC,
//! which owns the upload session lock.

use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use crate::attachments::ObjectStorage;
use crate::db::context::{lock_tree, set_system, set_tenant};
use crate::db::identity::{append_event, EventAppend};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrashPurgeOutcome {
    Purged {
        storage_deleted: u32,
    },
    /// Restored, already gone, not yet expired, or waiting on children/uploads.
    Skipped,
    /// A storage key could not be removed; every row was kept.
    StorageFailed,
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

/// Expired trashed documents of one workspace, deepest first.
pub async fn list_expired_trashed_documents(
    pool: &PgPool,
    workspace_id: Uuid,
    cutoff: DateTime<Utc>,
    limit: i64,
) -> Result<Vec<Uuid>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let rows: Vec<(Uuid,)> = sqlx::query_as(
        r#"
        SELECT id
        FROM fvoci.documents
        WHERE workspace_id = $1 AND deleted_at IS NOT NULL AND deleted_at <= $2
        ORDER BY (length(path) - length(replace(path, '.', ''))) DESC, deleted_at ASC, id ASC
        LIMIT $3
        "#,
    )
    .bind(workspace_id)
    .bind(cutoff)
    .bind(limit)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// Purges one expired trashed document. Rechecks everything under the tree lock.
pub async fn purge_trashed_document(
    pool: &PgPool,
    storage: &ObjectStorage,
    workspace_id: Uuid,
    document_id: Uuid,
    cutoff: DateTime<Utc>,
) -> Result<TrashPurgeOutcome, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    lock_tree(&mut tx, workspace_id).await?;
    let row: Option<(Option<DateTime<Utc>>,)> = sqlx::query_as(
        "SELECT deleted_at FROM fvoci.documents WHERE workspace_id = $1 AND id = $2 FOR UPDATE",
    )
    .bind(workspace_id)
    .bind(document_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((Some(deleted_at),)) = row else {
        tx.rollback().await?;
        return Ok(TrashPurgeOutcome::Skipped);
    };
    if deleted_at > cutoff {
        tx.rollback().await?;
        return Ok(TrashPurgeOutcome::Skipped);
    }
    let has_child: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM fvoci.documents WHERE workspace_id = $1 AND parent_id = $2 LIMIT 1",
    )
    .bind(workspace_id)
    .bind(document_id)
    .fetch_optional(&mut *tx)
    .await?;
    if has_child.is_some() {
        tx.rollback().await?;
        return Ok(TrashPurgeOutcome::Skipped);
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
    .fetch_all(&mut *tx)
    .await?;
    if attachments
        .iter()
        .any(|(_, status)| status == "uploading" || status == "assembling")
    {
        tx.rollback().await?;
        return Ok(TrashPurgeOutcome::Skipped);
    }

    #[cfg(feature = "db-tests")]
    test_hooks::pause_before_storage(document_id).await;

    let mut storage_deleted = 0u32;
    for (key, _) in &attachments {
        if let Err(err) = storage.purge_key(key).await {
            tracing::warn!(
                workspace_id = %workspace_id,
                document_id = %document_id,
                error = %err,
                "maintenance.document_purge_storage_failed"
            );
            tx.rollback().await?;
            return Ok(TrashPurgeOutcome::StorageFailed);
        }
        storage_deleted += 1;
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
    sqlx::query(
        "DELETE FROM fvoci.documents WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NOT NULL",
    )
    .bind(workspace_id)
    .bind(document_id)
    .execute(&mut *tx)
    .await?;
    append_event(
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
    )
    .await?;
    tx.commit().await?;
    Ok(TrashPurgeOutcome::Purged { storage_deleted })
}

#[cfg(feature = "db-tests")]
pub mod test_hooks {
    //! Test-only barrier between the DB recheck and the storage phase.
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
