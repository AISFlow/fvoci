use chrono::{DateTime, Utc};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;

use crate::db::context::set_system;

pub const TOKEN_GC_BATCH: i64 = 5_000;

pub async fn run_ics_token_gc(
    pool: &PgPool,
    now: DateTime<Utc>,
    cancel: &CancellationToken,
) -> Result<u32, sqlx::Error> {
    if cancel.is_cancelled() {
        return Ok(0);
    }
    let mut tx = pool.begin().await?;
    set_system(&mut tx).await?;
    let deleted = sqlx::query(
        r#"
        WITH doomed AS (
            SELECT id
            FROM fvoci.ics_tokens
            WHERE expires_at IS NOT NULL
              AND expires_at <= $1
            ORDER BY expires_at ASC, id ASC
            LIMIT $2
            FOR UPDATE SKIP LOCKED
        )
        DELETE FROM fvoci.ics_tokens AS t
        USING doomed
        WHERE t.id = doomed.id
        "#,
    )
    .bind(now)
    .bind(TOKEN_GC_BATCH)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    tx.commit().await?;
    Ok(deleted as u32)
}

pub async fn run_magic_token_gc(
    pool: &PgPool,
    now: DateTime<Utc>,
    cancel: &CancellationToken,
) -> Result<u32, sqlx::Error> {
    if cancel.is_cancelled() {
        return Ok(0);
    }
    let mut tx = pool.begin().await?;
    set_system(&mut tx).await?;
    let deleted: i32 = sqlx::query_scalar("SELECT fvoci.app_magic_purge_expired($1, $2)")
        .bind(now)
        .bind(TOKEN_GC_BATCH as i32)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(deleted.max(0) as u32)
}
