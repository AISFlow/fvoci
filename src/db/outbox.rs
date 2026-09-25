use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{PgPool, Row};
use uuid::Uuid;

pub const OUTBOX_LEASE_SECS: i64 = 30;
pub const OUTBOX_DEFAULT_BATCH: i32 = 100;
pub const OUTBOX_MAX_ATTEMPTS: i32 = 5;
pub const OUTBOX_FAILURE_BACKOFF_SECS: i32 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxEvent {
    pub snapshot_xmin: String,
    pub id: Uuid,
    pub seq: i64,
    pub xact: String,
    pub workspace_id: Option<Uuid>,
    pub actor_user_id: Option<Uuid>,
    pub verb: String,
    pub target_type: Option<String>,
    pub target_id: Option<Uuid>,
    pub payload: Value,
    pub channel: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxRetry {
    pub event_id: Uuid,
    pub attempts: i32,
    pub last_error: String,
}

pub async fn ensure_consumer(pool: &PgPool, consumer: &str) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT fvoci.app_outbox_ensure_consumer($1)")
        .bind(consumer)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn lease_consumer(
    pool: &PgPool,
    consumer: &str,
    owner: Uuid,
    ttl_secs: i64,
) -> Result<bool, sqlx::Error> {
    let leased = sqlx::query_scalar("SELECT fvoci.app_outbox_lease($1, $2, $3::integer)")
        .bind(consumer)
        .bind(owner)
        .bind(ttl_secs.clamp(1, 3600) as i32)
        .fetch_one(pool)
        .await?;
    Ok(leased)
}

pub async fn release_consumer(
    pool: &PgPool,
    consumer: &str,
    owner: Uuid,
) -> Result<bool, sqlx::Error> {
    let released = sqlx::query_scalar("SELECT fvoci.app_outbox_release($1, $2)")
        .bind(consumer)
        .bind(owner)
        .fetch_one(pool)
        .await?;
    Ok(released)
}

pub async fn read_events(
    pool: &PgPool,
    consumer: &str,
    limit: i32,
) -> Result<Vec<OutboxEvent>, sqlx::Error> {
    let rows = sqlx::query(
        r#"
        SELECT
            snapshot_xmin::text,
            event_id,
            seq,
            xact::text,
            workspace_id,
            actor_user_id,
            verb,
            target_type,
            target_id,
            payload,
            channel,
            created_at
        FROM fvoci.app_outbox_read($1, $2)
        "#,
    )
    .bind(consumer)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| OutboxEvent {
            snapshot_xmin: row.get("snapshot_xmin"),
            id: row.get("event_id"),
            seq: row.get("seq"),
            xact: row.get("xact"),
            workspace_id: row.get("workspace_id"),
            actor_user_id: row.get("actor_user_id"),
            verb: row.get("verb"),
            target_type: row.get("target_type"),
            target_id: row.get("target_id"),
            payload: row.get("payload"),
            channel: row.get("channel"),
            created_at: row.get("created_at"),
        })
        .collect())
}

pub async fn advance_cursor(
    pool: &PgPool,
    consumer: &str,
    owner: Uuid,
    xact: &str,
    seq: i64,
) -> Result<bool, sqlx::Error> {
    let advanced = sqlx::query_scalar("SELECT fvoci.app_outbox_advance($1, $2, $3::xid8, $4)")
        .bind(consumer)
        .bind(owner)
        .bind(xact)
        .bind(seq)
        .fetch_one(pool)
        .await?;
    Ok(advanced)
}

pub async fn advance_cursor_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    consumer: &str,
    owner: Uuid,
    xact: &str,
    seq: i64,
) -> Result<bool, sqlx::Error> {
    let advanced = sqlx::query_scalar("SELECT fvoci.app_outbox_advance($1, $2, $3::xid8, $4)")
        .bind(consumer)
        .bind(owner)
        .bind(xact)
        .bind(seq)
        .fetch_one(&mut **tx)
        .await?;
    Ok(advanced)
}

pub async fn record_failure(
    pool: &PgPool,
    consumer: &str,
    event_id: Uuid,
    error: &str,
    backoff_secs: i32,
) -> Result<i32, sqlx::Error> {
    let attempts = sqlx::query_scalar("SELECT fvoci.app_outbox_record_failure($1, $2, $3, $4)")
        .bind(consumer)
        .bind(event_id)
        .bind(error)
        .bind(backoff_secs.max(1))
        .fetch_one(pool)
        .await?;
    Ok(attempts)
}

pub async fn clear_failure(
    pool: &PgPool,
    consumer: &str,
    event_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let cleared = sqlx::query_scalar("SELECT fvoci.app_outbox_clear_failure($1, $2)")
        .bind(consumer)
        .bind(event_id)
        .fetch_one(pool)
        .await?;
    Ok(cleared)
}

pub async fn claim_retries(
    pool: &PgPool,
    consumer: &str,
    limit: i32,
) -> Result<Vec<OutboxRetry>, sqlx::Error> {
    let rows = sqlx::query(
        r#"
        SELECT event_id, attempts, last_error
        FROM fvoci.app_outbox_claim_retries($1, $2)
        "#,
    )
    .bind(consumer)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| OutboxRetry {
            event_id: row.get("event_id"),
            attempts: row.get("attempts"),
            last_error: row.get("last_error"),
        })
        .collect())
}

pub async fn mark_processed(
    pool: &PgPool,
    consumer: &str,
    event_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let inserted = sqlx::query_scalar("SELECT fvoci.app_outbox_mark_processed($1, $2)")
        .bind(consumer)
        .bind(event_id)
        .fetch_one(pool)
        .await?;
    Ok(inserted)
}

pub async fn mark_processed_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    consumer: &str,
    event_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let inserted = sqlx::query_scalar("SELECT fvoci.app_outbox_mark_processed($1, $2)")
        .bind(consumer)
        .bind(event_id)
        .fetch_one(&mut **tx)
        .await?;
    Ok(inserted)
}

pub async fn is_processed(
    pool: &PgPool,
    consumer: &str,
    event_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let processed = sqlx::query_scalar("SELECT fvoci.app_outbox_is_processed($1, $2)")
        .bind(consumer)
        .bind(event_id)
        .fetch_one(pool)
        .await?;
    Ok(processed)
}

pub async fn fetch_event_by_id(
    pool: &PgPool,
    event_id: Uuid,
) -> Result<Option<OutboxEvent>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    crate::db::context::set_system(&mut tx).await?;
    let row = sqlx::query(
        r#"
        SELECT
            pg_snapshot_xmin(pg_current_snapshot())::text AS snapshot_xmin,
            id AS event_id,
            seq,
            xact::text,
            workspace_id,
            actor_user_id,
            verb,
            target_type,
            target_id,
            payload,
            channel,
            created_at
        FROM fvoci.events
        WHERE id = $1
        "#,
    )
    .bind(event_id)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;

    Ok(row.map(|row| OutboxEvent {
        snapshot_xmin: row.get("snapshot_xmin"),
        id: row.get("event_id"),
        seq: row.get("seq"),
        xact: row.get("xact"),
        workspace_id: row.get("workspace_id"),
        actor_user_id: row.get("actor_user_id"),
        verb: row.get("verb"),
        target_type: row.get("target_type"),
        target_id: row.get("target_id"),
        payload: row.get("payload"),
        channel: row.get("channel"),
        created_at: row.get("created_at"),
    }))
}

pub async fn fetch_cursor(
    pool: &PgPool,
    consumer: &str,
) -> Result<Option<(String, i64)>, sqlx::Error> {
    let row = sqlx::query(
        r#"
        SELECT last_xact::text, last_seq
        FROM fvoci.outbox_consumers
        WHERE consumer = $1
        "#,
    )
    .bind(consumer)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|row| (row.get("last_xact"), row.get("last_seq"))))
}

pub async fn insert_test_event(
    pool: &PgPool,
    verb: &str,
    payload: Value,
) -> Result<Uuid, sqlx::Error> {
    let id = Uuid::now_v7();
    let mut tx = pool.begin().await?;
    crate::db::context::set_system(&mut tx).await?;
    sqlx::query(
        r#"
        INSERT INTO fvoci.events (id, verb, payload, channel)
        VALUES ($1, $2, $3, 'system')
        "#,
    )
    .bind(id)
    .bind(verb)
    .bind(payload)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(id)
}
