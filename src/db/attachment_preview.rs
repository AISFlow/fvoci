//! Preview job persistence (source `thumbnailAttempts` + `attachments.variants`).
//!
//! Ordering that keeps storage and rows consistent:
//! 1. claim a lease on a `pending` stored attachment;
//! 2. journal the fresh preview key in `attachment_object_cleanups`, due only
//!    after the lease can no longer publish ([`journal_preview_key`]);
//! 3. write the object;
//! 4. publish `variants.preview` and delete that journal row in one
//!    transaction, only while the lease is still ours ([`publish_preview`]).
//!
//! A crash or lost lease after 2 leaves the key journaled, and the object
//! reclaim job deletes it; a deleted attachment journals its published key
//! through the delete trigger.

use serde_json::json;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::db::context::set_tenant;

/// Lease for one claim; covers the child watchdog, storage I/O and publish.
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
    let row = sqlx::query(
        "SELECT workspace_id, attachment_id, lease_token, attempt FROM fvoci.app_claim_attachment_preview($1)",
    )
    .bind(PREVIEW_LEASE_SECS)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|row| PreviewClaim {
        workspace_id: row.get("workspace_id"),
        attachment_id: row.get("attachment_id"),
        lease_token: row.get("lease_token"),
        attempt: row.get("attempt"),
    }))
}

pub async fn load_preview_input(
    pool: &PgPool,
    claim: &PreviewClaim,
) -> Result<Option<PreviewInput>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, claim.workspace_id).await?;
    let row = sqlx::query(
        r#"
        SELECT storage_key, mime, size_bytes
        FROM fvoci.attachments
        WHERE workspace_id = $1 AND id = $2 AND preview_lease_token = $3
          AND status = 'stored' AND preview_status = 'pending'
          AND preview_lease_expires_at > now()
        "#,
    )
    .bind(claim.workspace_id)
    .bind(claim.attachment_id)
    .bind(claim.lease_token)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(row.map(|row| PreviewInput {
        storage_key: row.get("storage_key"),
        mime: row.get("mime"),
        size_bytes: row.get("size_bytes"),
    }))
}

/// Journals `key` before the object exists. It becomes due twice a lease
/// from now, after any publish under this lease must have happened.
pub async fn journal_preview_key(
    pool: &PgPool,
    claim: &PreviewClaim,
    key: &str,
) -> Result<Uuid, sqlx::Error> {
    let id = Uuid::now_v7();
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, claim.workspace_id).await?;
    sqlx::query(
        r#"
        INSERT INTO fvoci.attachment_object_cleanups (id, workspace_id, attachment_id, storage_key, due_at)
        VALUES ($1, $2, $3, $4, clock_timestamp() + ($5 * interval '2 seconds'))
        "#,
    )
    .bind(id)
    .bind(claim.workspace_id)
    .bind(claim.attachment_id)
    .bind(key)
    .bind(PREVIEW_LEASE_SECS)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(id)
}

/// Publishes the preview when the lease is still held and live. Returns
/// `false` when it was lost (deleted row, expired lease): the journaled key
/// then stays for the reclaim job.
pub async fn publish_preview(
    pool: &PgPool,
    claim: &PreviewClaim,
    journal_id: Uuid,
    key: &str,
    width: u32,
    height: u32,
    bytes: u64,
) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, claim.workspace_id).await?;
    // Hold the journal row first: once reclaim has taken it (and purged the
    // object) the preview must not be published, whatever the timing.
    let journal: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM fvoci.attachment_object_cleanups WHERE id = $1 FOR UPDATE")
            .bind(journal_id)
            .fetch_optional(&mut *tx)
            .await?;
    if journal.is_none() {
        tx.rollback().await?;
        return Ok(false);
    }
    let updated = sqlx::query(
        r#"
        UPDATE fvoci.attachments
        SET variants = jsonb_set(variants, '{preview}', $4::jsonb, true),
            preview_status = 'ok',
            preview_lease_token = NULL,
            preview_lease_expires_at = NULL
        WHERE workspace_id = $1 AND id = $2 AND preview_lease_token = $3
          AND status = 'stored' AND preview_status = 'pending'
          AND preview_lease_expires_at > clock_timestamp()
          AND NOT (variants ? 'preview')
        "#,
    )
    .bind(claim.workspace_id)
    .bind(claim.attachment_id)
    .bind(claim.lease_token)
    .bind(json!({"key": key, "width": width, "height": height, "bytes": bytes}))
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() == 0 {
        tx.rollback().await?;
        return Ok(false);
    }
    let removed = sqlx::query("DELETE FROM fvoci.attachment_object_cleanups WHERE id = $1")
        .bind(journal_id)
        .execute(&mut *tx)
        .await?;
    if removed.rows_affected() != 1 {
        tx.rollback().await?;
        return Ok(false);
    }
    tx.commit().await?;
    Ok(true)
}

/// Source `UnrecoverableError`: the input can never produce a preview.
pub async fn fail_preview(pool: &PgPool, claim: &PreviewClaim) -> Result<bool, sqlx::Error> {
    finish_without_preview(pool, claim, "failed").await
}

/// Gives the lease back without spending an attempt (shutdown).
pub async fn release_preview(pool: &PgPool, claim: &PreviewClaim) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, claim.workspace_id).await?;
    let updated = sqlx::query(
        r#"
        UPDATE fvoci.attachments
        SET preview_lease_token = NULL,
            preview_lease_expires_at = NULL,
            preview_attempts = GREATEST(preview_attempts - 1, 0)
        WHERE workspace_id = $1 AND id = $2 AND preview_lease_token = $3
          AND status = 'stored' AND preview_status = 'pending'
        "#,
    )
    .bind(claim.workspace_id)
    .bind(claim.attachment_id)
    .bind(claim.lease_token)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(updated.rows_affected() == 1)
}

async fn finish_without_preview(
    pool: &PgPool,
    claim: &PreviewClaim,
    status: &str,
) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, claim.workspace_id).await?;
    let updated = sqlx::query(
        r#"
        UPDATE fvoci.attachments
        SET preview_status = $4,
            preview_lease_token = NULL,
            preview_lease_expires_at = NULL
        WHERE workspace_id = $1 AND id = $2 AND preview_lease_token = $3
          AND status = 'stored' AND preview_status = 'pending'
        "#,
    )
    .bind(claim.workspace_id)
    .bind(claim.attachment_id)
    .bind(claim.lease_token)
    .bind(status)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(updated.rows_affected() == 1)
}
