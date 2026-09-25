use sqlx::PgPool;
use tokio_util::sync::CancellationToken;

use crate::db::context::set_system;

pub const NOTIFICATION_READ_RETENTION_DAYS: i32 = 90;
pub const NOTIFICATION_ARCHIVED_RETENTION_DAYS: i32 = 90;
pub const PROCESSED_GC_WINDOW_DAYS: i32 = 30;
pub const GC_DELETE_BATCH: i32 = 5_000;
pub const GC_DELETE_ROUNDS: u32 = 30;

const PROCESSED_GC_CONSUMERS: &[&str] = &[
    "notifications",
    "mail",
    "search-index",
    "webhooks",
    "github",
];

/// Source sweep.ts notification + processed_events GC. events, audit_log, and
/// collab receipts are not purged in the source daily sweep (and the app role
/// cannot DELETE them).
pub async fn run_notification_gc(
    pool: &PgPool,
    cancel: &CancellationToken,
) -> Result<(u32, u32), sqlx::Error> {
    let read =
        gc_notification_kind(pool, cancel, "notifications", NotificationGcKind::Read).await?;
    let archived = gc_notification_kind(
        pool,
        cancel,
        "notifications-archived",
        NotificationGcKind::Archived,
    )
    .await?;
    Ok((read, archived))
}

enum NotificationGcKind {
    Read,
    Archived,
}

pub async fn run_processed_gc(
    pool: &PgPool,
    cancel: &CancellationToken,
) -> Result<u32, sqlx::Error> {
    let mut deleted = 0u32;
    for consumer in PROCESSED_GC_CONSUMERS {
        if cancel.is_cancelled() {
            break;
        }
        deleted += gc_processed_consumer(pool, consumer, cancel).await?;
    }
    Ok(deleted)
}

async fn gc_processed_consumer(
    pool: &PgPool,
    consumer: &str,
    cancel: &CancellationToken,
) -> Result<u32, sqlx::Error> {
    let mut deleted = 0u32;
    for _ in 0..GC_DELETE_ROUNDS {
        if cancel.is_cancelled() {
            break;
        }
        let mut tx = pool.begin().await?;
        set_system(&mut tx).await?;
        let n: i32 = sqlx::query_scalar("SELECT fvoci.app_outbox_gc_processed($1, $2, $3)")
            .bind(consumer)
            .bind(PROCESSED_GC_WINDOW_DAYS)
            .bind(GC_DELETE_BATCH)
            .fetch_one(&mut *tx)
            .await?;
        tx.commit().await?;
        let n = n.max(0) as u32;
        deleted += n;
        if n < GC_DELETE_BATCH as u32 {
            break;
        }
    }
    Ok(deleted)
}

async fn gc_notification_kind(
    pool: &PgPool,
    cancel: &CancellationToken,
    kind: &'static str,
    which: NotificationGcKind,
) -> Result<u32, sqlx::Error> {
    let mut deleted = 0u32;
    let mut rounds = 0u32;
    for _ in 0..GC_DELETE_ROUNDS {
        if cancel.is_cancelled() {
            break;
        }
        let n = match which {
            NotificationGcKind::Read => purge_read_batch(pool).await?,
            NotificationGcKind::Archived => purge_archived_batch(pool).await?,
        };
        rounds += 1;
        deleted += n;
        if n < GC_DELETE_BATCH as u32 {
            return Ok(deleted);
        }
    }
    tracing::warn!(kind, deleted, rounds, "cleanup.gc_round_capped");
    Ok(deleted)
}

async fn purge_read_batch(pool: &PgPool) -> Result<u32, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_system(&mut tx).await?;
    let deleted = sqlx::query(
        r#"
        WITH doomed AS (
            SELECT ctid
            FROM fvoci.notifications
            WHERE read_at IS NOT NULL
              AND read_at < now() - ($1::text || ' days')::interval
            LIMIT $2
            FOR UPDATE SKIP LOCKED
        )
        DELETE FROM fvoci.notifications AS n
        USING doomed
        WHERE n.ctid = doomed.ctid
        "#,
    )
    .bind(NOTIFICATION_READ_RETENTION_DAYS.to_string())
    .bind(GC_DELETE_BATCH)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    tx.commit().await?;
    Ok(deleted as u32)
}

async fn purge_archived_batch(pool: &PgPool) -> Result<u32, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_system(&mut tx).await?;
    let deleted = sqlx::query(
        r#"
        WITH doomed AS (
            SELECT ctid
            FROM fvoci.notifications
            WHERE archived_at IS NOT NULL
              AND read_at IS NULL
              AND archived_at < now() - ($1::text || ' days')::interval
            LIMIT $2
            FOR UPDATE SKIP LOCKED
        )
        DELETE FROM fvoci.notifications AS n
        USING doomed
        WHERE n.ctid = doomed.ctid
        "#,
    )
    .bind(NOTIFICATION_ARCHIVED_RETENTION_DAYS.to_string())
    .bind(GC_DELETE_BATCH)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    tx.commit().await?;
    Ok(deleted as u32)
}

/// Source `gcWebhookDeliveries` (90 days) and the GitHub delivery-id dedupe
/// rows (30 days), in bounded batches.
pub async fn run_integration_gc(
    pool: &PgPool,
    cancel: &CancellationToken,
) -> Result<(u32, u32), sqlx::Error> {
    use crate::db::integrations::{
        purge_github_deliveries, purge_settled_deliveries, GITHUB_DELIVERY_RETENTION_DAYS,
        INTEGRATION_GC_BATCH, WEBHOOK_DELIVERY_RETENTION_DAYS,
    };
    let mut webhook = 0u32;
    let mut github = 0u32;
    for _ in 0..GC_DELETE_ROUNDS {
        if cancel.is_cancelled() {
            break;
        }
        let n = purge_settled_deliveries(pool, WEBHOOK_DELIVERY_RETENTION_DAYS).await?;
        webhook += n as u32;
        if (n as i64) < INTEGRATION_GC_BATCH {
            break;
        }
    }
    for _ in 0..GC_DELETE_ROUNDS {
        if cancel.is_cancelled() {
            break;
        }
        let n = purge_github_deliveries(pool, GITHUB_DELIVERY_RETENTION_DAYS).await?;
        github += n as u32;
        if (n as i64) < INTEGRATION_GC_BATCH {
            break;
        }
    }
    Ok((webhook, github))
}
