use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use super::backend::{Backend, DbTransaction, FamilyTx, OperationTx};
use super::codec::{Cell, FamilyRow};

/// PostgreSQL transaction visibility and SQLite committed writer order are
/// distinct contracts. A family event never invents an XID or xmin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventVisibility {
    Postgres { snapshot_xmin: String, xact: String },
    SqliteFamily,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutboxCursor {
    Postgres { xact: String, seq: i64 },
    SqliteFamily { seq: i64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendOutboxEvent {
    pub visibility: EventVisibility,
    pub id: Uuid,
    pub seq: i64,
    pub workspace_id: Option<Uuid>,
    pub actor_user_id: Option<Uuid>,
    pub verb: String,
    pub target_type: Option<String>,
    pub target_id: Option<Uuid>,
    pub payload: Value,
    pub channel: String,
    pub created_at: DateTime<Utc>,
}

impl BackendOutboxEvent {
    pub fn cursor(&self) -> OutboxCursor {
        match &self.visibility {
            EventVisibility::Postgres { xact, .. } => OutboxCursor::Postgres {
                xact: xact.clone(),
                seq: self.seq,
            },
            EventVisibility::SqliteFamily => OutboxCursor::SqliteFamily { seq: self.seq },
        }
    }
}

impl From<OutboxEvent> for BackendOutboxEvent {
    fn from(event: OutboxEvent) -> Self {
        Self {
            visibility: EventVisibility::Postgres {
                snapshot_xmin: event.snapshot_xmin,
                xact: event.xact,
            },
            id: event.id,
            seq: event.seq,
            workspace_id: event.workspace_id,
            actor_user_id: event.actor_user_id,
            verb: event.verb,
            target_type: event.target_type,
            target_id: event.target_id,
            payload: event.payload,
            channel: event.channel,
            created_at: event.created_at,
        }
    }
}

pub const OUTBOX_LEASE_SECS: i64 = 30;
pub const OUTBOX_DEFAULT_BATCH: i32 = 100;
/// Failed attempts after which an event is dead-lettered: the cursor passes
/// it, and it is delivered again only after a manual SQL call to
/// `fvoci.app_outbox_requeue`, or when `--recover-outbox` replays a window
/// that contains it (see the `crate::outbox` module doc).
pub const OUTBOX_MAX_ATTEMPTS: i32 = 5;
/// Delay before the first retry; `app_outbox_record_failure` doubles it
/// after each further failure (up to 60 s). With the defaults the fifth
/// failure, and so the dead letter, comes about 15 s (1 + 2 + 4 + 8) plus
/// attempt time after the first.
pub const OUTBOX_FAILURE_BACKOFF_MS: i32 = 1000;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxFailureState {
    pub attempts: i32,
    pub next_attempt_at: DateTime<Utc>,
    pub dead_at: Option<DateTime<Utc>>,
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
    owner: Uuid,
    event_id: Uuid,
    error: &str,
    backoff_ms: i32,
    max_attempts: i32,
) -> Result<i32, sqlx::Error> {
    let attempts =
        sqlx::query_scalar("SELECT fvoci.app_outbox_record_failure($1, $2, $3, $4, $5, $6)")
            .bind(consumer)
            .bind(owner)
            .bind(event_id)
            .bind(error)
            .bind(backoff_ms.max(1))
            .bind(max_attempts.max(1))
            .fetch_one(pool)
            .await?;
    Ok(attempts)
}

pub async fn fetch_failure_state(
    pool: &PgPool,
    consumer: &str,
    event_id: Uuid,
) -> Result<Option<OutboxFailureState>, sqlx::Error> {
    let row = sqlx::query(
        r#"
        SELECT attempts, next_attempt_at, dead_at
        FROM fvoci.app_outbox_failure_state($1, $2)
        "#,
    )
    .bind(consumer)
    .bind(event_id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|row| OutboxFailureState {
        attempts: row.get("attempts"),
        next_attempt_at: row.get("next_attempt_at"),
        dead_at: row.get("dead_at"),
    }))
}

pub async fn requeue(pool: &PgPool, consumer: &str, event_id: Uuid) -> Result<bool, sqlx::Error> {
    let requeued = sqlx::query_scalar("SELECT fvoci.app_outbox_requeue($1, $2)")
        .bind(consumer)
        .bind(event_id)
        .fetch_one(pool)
        .await?;
    Ok(requeued)
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

pub fn is_outbox_xid_epoch_mismatch(err: &sqlx::Error) -> bool {
    match err {
        sqlx::Error::Database(db) => {
            db.code().as_deref() == Some("22000")
                && db.message().contains("outbox xid epoch mismatch")
        }
        _ => false,
    }
}

fn family_tx<'a>(tx: &'a mut DbTransaction<'_>) -> Result<&'a mut FamilyTx, sqlx::Error> {
    match tx {
        DbTransaction::SqliteFamily(tx) => Ok(tx),
        DbTransaction::Postgres(_) => Err(sqlx::Error::Protocol(
            "SQLite outbox operation received PostgreSQL transaction".into(),
        )),
    }
}

fn check_consumer(consumer: &str) -> Result<(), sqlx::Error> {
    let bytes = consumer.as_bytes();
    if !bytes.is_empty()
        && bytes.len() <= 63
        && bytes[0].is_ascii_lowercase()
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'_' || *b == b'-')
    {
        Ok(())
    } else {
        Err(sqlx::Error::Protocol("invalid outbox consumer name".into()))
    }
}

async fn family_now(tx: &mut FamilyTx) -> Result<i64, sqlx::Error> {
    let rows = tx
        .query(
            "SELECT unixepoch()*1000000+CAST(substr(strftime('%f','now'),4,3) AS INTEGER)*1000",
            &[],
        )
        .await?;
    rows.first()
        .ok_or(sqlx::Error::RowNotFound)?
        .cell(0)?
        .integer()
}

async fn finish_write(tx: DbTransaction<'_>) -> Result<(), sqlx::Error> {
    // Retain the typed ambiguous outcome as the error source. Dispatchers must
    // reconcile their marker/cursor on a fresh transaction, never retry a write
    // merely because the COMMIT response was lost.
    tx.commit()
        .await
        .map_err(|e| sqlx::Error::AnyDriverError(Box::new(e)))
}

async fn family_ensure(tx: &mut FamilyTx, consumer: &str) -> Result<(), sqlx::Error> {
    tx.require_writer()?;
    check_consumer(consumer)?;
    let now = family_now(tx).await?;
    tx.execute(
        "INSERT INTO outbox_consumers(consumer,last_seq,updated_at) VALUES(?1,0,?2) ON CONFLICT(consumer) DO NOTHING",
        &[Cell::text(consumer),Cell::Integer(now)],
    ).await?;
    Ok(())
}

pub async fn ensure_consumer_backend(backend: &Backend, consumer: &str) -> Result<(), sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return ensure_consumer(pool, consumer).await;
    }
    let mut tx = backend.begin_write().await?;
    family_ensure(family_tx(&mut tx)?, consumer).await?;
    finish_write(tx).await
}

pub async fn lease_consumer_backend(
    backend: &Backend,
    consumer: &str,
    owner: Uuid,
    ttl_secs: i64,
) -> Result<bool, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return lease_consumer(pool, consumer, owner, ttl_secs).await;
    }
    let mut tx = backend.begin_write().await?;
    let family = family_tx(&mut tx)?;
    family_ensure(family, consumer).await?;
    let now = family_now(family).await?;
    let until = now
        .checked_add(ttl_secs.clamp(1, 3600) * 1_000_000)
        .ok_or_else(|| sqlx::Error::Protocol("outbox lease instant overflow".into()))?;
    let changed = family.execute(
        "UPDATE outbox_consumers SET lease_owner=?2,lease_until=?3,updated_at=?4 WHERE consumer=?1 AND (lease_owner IS NULL OR lease_until<?4 OR lease_owner=?2)",
        &[Cell::text(consumer),Cell::uuid(owner),Cell::Integer(until),Cell::Integer(now)],
    ).await?;
    finish_write(tx).await?;
    Ok(changed == 1)
}

pub async fn release_consumer_backend(
    backend: &Backend,
    consumer: &str,
    owner: Uuid,
) -> Result<bool, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return release_consumer(pool, consumer, owner).await;
    }
    let mut tx = backend.begin_write().await?;
    let family = family_tx(&mut tx)?;
    let now = family_now(family).await?;
    let changed = family.execute(
        "UPDATE outbox_consumers SET lease_owner=NULL,lease_until=NULL,updated_at=?3 WHERE consumer=?1 AND lease_owner=?2",
        &[Cell::text(consumer),Cell::uuid(owner),Cell::Integer(now)],
    ).await?;
    finish_write(tx).await?;
    Ok(changed == 1)
}

fn decode_family_event(row: &FamilyRow) -> Result<BackendOutboxEvent, sqlx::Error> {
    let seq = row.cell(1)?.integer()?;
    if seq <= 0 {
        return Err(sqlx::Error::Protocol(
            "event sequence must be positive".into(),
        ));
    }
    Ok(BackendOutboxEvent {
        visibility: EventVisibility::SqliteFamily,
        id: row.cell(0)?.id()?,
        seq,
        workspace_id: row.cell(2)?.optional(Cell::id)?,
        actor_user_id: row.cell(3)?.optional(Cell::id)?,
        verb: row.cell(4)?.string()?,
        target_type: row.cell(5)?.optional(Cell::string)?,
        target_id: row.cell(6)?.optional(Cell::id)?,
        payload: row.cell(7)?.value()?,
        channel: row.cell(8)?.string()?,
        created_at: row.cell(9)?.datetime()?,
    })
}

pub async fn read_events_backend(
    backend: &Backend,
    consumer: &str,
    limit: i32,
) -> Result<Vec<BackendOutboxEvent>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return Ok(read_events(pool, consumer, limit)
            .await?
            .into_iter()
            .map(Into::into)
            .collect());
    }
    let mut tx = backend.begin_read().await?;
    // One snapshot contains the cursor and committed event rows. The emitter
    // allocates seq under BEGIN IMMEDIATE in the same event transaction, so an
    // earlier uncommitted writer cannot surface behind an advanced cursor.
    let rows = family_tx(&mut tx)?.query(
        "SELECT e.id,e.seq,e.workspace_id,e.actor_user_id,e.verb,e.target_type,e.target_id,e.payload,e.channel,e.created_at FROM events e JOIN outbox_consumers c ON c.consumer=?1 WHERE e.seq>c.last_seq ORDER BY e.seq LIMIT ?2",
        &[Cell::text(consumer),Cell::Integer(i64::from(limit.max(0)))],
    ).await?;
    let events = rows
        .iter()
        .map(decode_family_event)
        .collect::<Result<_, _>>()?;
    tx.rollback().await?;
    Ok(events)
}

pub async fn advance_cursor_backend(
    backend: &Backend,
    consumer: &str,
    owner: Uuid,
    cursor: &OutboxCursor,
) -> Result<bool, sqlx::Error> {
    let mut tx = backend.begin_write().await?;
    let advanced = advance_cursor_backend_tx(&mut tx, consumer, owner, cursor).await?;
    finish_write(tx).await?;
    Ok(advanced)
}

pub async fn advance_cursor_backend_tx(
    tx: &mut DbTransaction<'_>,
    consumer: &str,
    owner: Uuid,
    cursor: &OutboxCursor,
) -> Result<bool, sqlx::Error> {
    tx.operation()
        .advance_outbox_cursor(consumer, owner, cursor)
        .await
}

impl OperationTx<'_, '_> {
    /// Read the event in the caller's current transaction and authority. The
    /// consumer owns system context, commit and rollback; this lookup opens no
    /// second connection and does not broaden the caller's context.
    pub(crate) async fn outbox_event_by_id(
        &mut self,
        event_id: Uuid,
    ) -> Result<Option<BackendOutboxEvent>, sqlx::Error> {
        match self {
            Self::Postgres(tx) => {
                let row = sqlx::query(
                    r#"
                    SELECT pg_snapshot_xmin(pg_current_snapshot())::text AS snapshot_xmin,
                           id AS event_id, seq, xact::text, workspace_id,
                           actor_user_id, verb, target_type, target_id, payload,
                           channel, created_at
                    FROM fvoci.events
                    WHERE id = $1
                    "#,
                )
                .bind(event_id)
                .fetch_optional(&mut ***tx)
                .await?;
                row.map(|row| {
                    Ok(BackendOutboxEvent {
                        visibility: EventVisibility::Postgres {
                            snapshot_xmin: row.try_get("snapshot_xmin")?,
                            xact: row.try_get("xact")?,
                        },
                        id: row.try_get("event_id")?,
                        seq: row.try_get("seq")?,
                        workspace_id: row.try_get("workspace_id")?,
                        actor_user_id: row.try_get("actor_user_id")?,
                        verb: row.try_get("verb")?,
                        target_type: row.try_get("target_type")?,
                        target_id: row.try_get("target_id")?,
                        payload: row.try_get("payload")?,
                        channel: row.try_get("channel")?,
                        created_at: row.try_get("created_at")?,
                    })
                })
                .transpose()
            }
            Self::SqliteFamily(tx) => {
                tx.require_system_context()?;
                let rows = tx.query(
                    "SELECT id,seq,workspace_id,actor_user_id,verb,target_type,target_id,payload,channel,created_at FROM events WHERE id=?1",
                    &[Cell::uuid(event_id)],
                ).await?;
                rows.first().map(decode_family_event).transpose()
            }
        }
    }

    pub(crate) async fn advance_outbox_cursor(
        &mut self,
        consumer: &str,
        owner: Uuid,
        cursor: &OutboxCursor,
    ) -> Result<bool, sqlx::Error> {
        match (self, cursor) {
            (Self::Postgres(tx), OutboxCursor::Postgres { xact, seq }) => {
                advance_cursor_tx(tx, consumer, owner, xact, *seq).await
            }
            (Self::SqliteFamily(tx), OutboxCursor::SqliteFamily { seq }) => {
                tx.require_writer()?;
                let now = family_now(tx).await?;
                let changed = tx.execute(
                    "UPDATE outbox_consumers SET last_seq=?3,updated_at=?4 WHERE consumer=?1 AND lease_owner=?2 AND lease_until>?4 AND last_seq<?3 AND EXISTS(SELECT 1 FROM events WHERE seq=?3)",
                    &[Cell::text(consumer),Cell::uuid(owner),Cell::Integer(*seq),Cell::Integer(now)],
                ).await?;
                if changed == 1 {
                    tx.execute(
                        "UPDATE outbox_failures SET skipped_at=COALESCE(skipped_at,?3),updated_at=?3 WHERE consumer=?1 AND event_id=(SELECT id FROM events WHERE seq=?2) AND dead_at IS NOT NULL",
                        &[Cell::text(consumer),Cell::Integer(*seq),Cell::Integer(now)],
                    ).await?;
                    Ok(true)
                } else {
                    let cleared = tx.execute(
                        "DELETE FROM outbox_failures WHERE consumer=?1 AND event_id=(SELECT id FROM events WHERE seq=?3) AND skipped_at IS NOT NULL AND dead_at IS NULL AND EXISTS(SELECT 1 FROM outbox_consumers WHERE consumer=?1 AND lease_owner=?2 AND lease_until>?4 AND last_seq>=?3)",
                        &[Cell::text(consumer),Cell::uuid(owner),Cell::Integer(*seq),Cell::Integer(now)],
                    ).await?;
                    Ok(cleared == 1)
                }
            }
            _ => Err(sqlx::Error::Protocol(
                "outbox cursor belongs to a different backend".into(),
            )),
        }
    }

    pub(crate) async fn mark_processed(
        &mut self,
        consumer: &str,
        event_id: Uuid,
    ) -> Result<bool, sqlx::Error> {
        match self {
            Self::Postgres(tx) => mark_processed_tx(tx, consumer, event_id).await,
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                let now = family_now(tx).await?;
                let changed = tx.execute(
                    "INSERT INTO processed_events(consumer,event_id,processed_at) VALUES(?1,?2,?3) ON CONFLICT(consumer,event_id) DO NOTHING",
                    &[Cell::text(consumer),Cell::uuid(event_id),Cell::Integer(now)],
                ).await?;
                Ok(changed == 1)
            }
        }
    }
}

pub async fn mark_processed_backend_tx(
    tx: &mut DbTransaction<'_>,
    consumer: &str,
    event_id: Uuid,
) -> Result<bool, sqlx::Error> {
    tx.operation().mark_processed(consumer, event_id).await
}

pub async fn mark_processed_backend(
    backend: &Backend,
    consumer: &str,
    event_id: Uuid,
) -> Result<bool, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return mark_processed(pool, consumer, event_id).await;
    }
    let mut tx = backend.begin_write().await?;
    let marked = mark_processed_backend_tx(&mut tx, consumer, event_id).await?;
    finish_write(tx).await?;
    Ok(marked)
}

pub async fn is_processed_backend(
    backend: &Backend,
    consumer: &str,
    event_id: Uuid,
) -> Result<bool, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return is_processed(pool, consumer, event_id).await;
    }
    let mut tx = backend.begin_read().await?;
    let rows = family_tx(&mut tx)?
        .query(
            "SELECT EXISTS(SELECT 1 FROM processed_events WHERE consumer=?1 AND event_id=?2)",
            &[Cell::text(consumer), Cell::uuid(event_id)],
        )
        .await?;
    let marked = rows
        .first()
        .ok_or(sqlx::Error::RowNotFound)?
        .cell(0)?
        .boolean()?;
    tx.rollback().await?;
    Ok(marked)
}

pub async fn fetch_event_by_id_backend(
    backend: &Backend,
    event_id: Uuid,
) -> Result<Option<BackendOutboxEvent>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return Ok(fetch_event_by_id(pool, event_id).await?.map(Into::into));
    }
    let mut tx = backend.begin_read().await?;
    let rows = family_tx(&mut tx)?.query(
        "SELECT id,seq,workspace_id,actor_user_id,verb,target_type,target_id,payload,channel,created_at FROM events WHERE id=?1",
        &[Cell::uuid(event_id)],
    ).await?;
    let event = rows.first().map(decode_family_event).transpose()?;
    tx.rollback().await?;
    Ok(event)
}

pub async fn fetch_cursor_backend(
    backend: &Backend,
    consumer: &str,
) -> Result<Option<OutboxCursor>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return Ok(fetch_cursor(pool, consumer)
            .await?
            .map(|(xact, seq)| OutboxCursor::Postgres { xact, seq }));
    }
    let mut tx = backend.begin_read().await?;
    let rows = family_tx(&mut tx)?
        .query(
            "SELECT last_seq FROM outbox_consumers WHERE consumer=?1",
            &[Cell::text(consumer)],
        )
        .await?;
    let cursor = rows
        .first()
        .map(|row| {
            row.cell(0)?
                .integer()
                .map(|seq| OutboxCursor::SqliteFamily { seq })
        })
        .transpose()?;
    tx.rollback().await?;
    Ok(cursor)
}

/// PostgreSQL's xmin stall is an actual cluster observation; it has no family
/// equivalent. None means not applicable, never an invented healthy zero.
pub async fn outbox_ages_backend(
    backend: &Backend,
    consumers: &[String],
) -> Result<(i64, Option<i64>), sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        let (lag, stall): (i64, i64) = sqlx::query_as(
            "SELECT fvoci.app_outbox_lag_seconds($1),fvoci.app_oldest_write_xact_age_seconds()",
        )
        .bind(consumers)
        .fetch_one(pool)
        .await?;
        return Ok((lag, Some(stall)));
    }
    let mut tx = backend.begin_read().await?;
    let names = serde_json::to_value(consumers).map_err(|e| sqlx::Error::Encode(Box::new(e)))?;
    let rows=family_tx(&mut tx)?.query("SELECT min(e.created_at),(unixepoch()*1000000+CAST(substr(strftime('%f','now'),4,3) AS INTEGER)*1000) FROM outbox_consumers c JOIN events e ON e.seq>c.last_seq WHERE c.consumer IN (SELECT value FROM json_each(?1))", &[Cell::json(&names)?]).await?;
    let row = rows.first().ok_or(sqlx::Error::RowNotFound)?;
    let oldest = row.cell(0)?.optional(Cell::integer)?;
    let now = row.cell(1)?.integer()?;
    let lag = match oldest {
        None => 0,
        Some(oldest) => {
            now.checked_sub(oldest)
                .and_then(|age| age.max(0).checked_add(500_000))
                .ok_or_else(|| sqlx::Error::Protocol("outbox lag duration overflow".into()))?
                / 1_000_000
        }
    };
    tx.rollback().await?;
    Ok((lag, None))
}

pub async fn record_failure_backend(
    backend: &Backend,
    consumer: &str,
    owner: Uuid,
    event_id: Uuid,
    error: &str,
    backoff_ms: i32,
    max_attempts: i32,
) -> Result<i32, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return record_failure(
            pool,
            consumer,
            owner,
            event_id,
            error,
            backoff_ms,
            max_attempts,
        )
        .await;
    }
    let mut tx = backend.begin_write().await?;
    let family = family_tx(&mut tx)?;
    let now = family_now(family).await?;
    let rows = family.query(
        "SELECT last_seq FROM outbox_consumers WHERE consumer=?1 AND lease_owner=?2 AND lease_until>?3",
        &[Cell::text(consumer),Cell::uuid(owner),Cell::Integer(now)],
    ).await?;
    let Some(lease) = rows.first() else {
        tx.rollback().await?;
        return Ok(0);
    };
    let cursor = lease.cell(0)?.integer()?;
    let rows = family
        .query(
            "SELECT attempts,dead_at FROM outbox_failures WHERE consumer=?1 AND event_id=?2",
            &[Cell::text(consumer), Cell::uuid(event_id)],
        )
        .await?;
    let existing = rows
        .first()
        .map(|row| {
            Ok::<_, sqlx::Error>((
                row.cell(0)?.integer()?,
                row.cell(1)?.optional(Cell::datetime)?,
            ))
        })
        .transpose()?;
    if let Some((attempts, Some(_))) = existing {
        let attempts = checked_attempts(attempts)?;
        tx.rollback().await?;
        return Ok(attempts);
    }
    if existing.is_none() {
        let rows = family
            .query(
                "SELECT EXISTS(SELECT 1 FROM events WHERE id=?1 AND seq<=?2)",
                &[Cell::uuid(event_id), Cell::Integer(cursor)],
            )
            .await?;
        if rows
            .first()
            .ok_or(sqlx::Error::RowNotFound)?
            .cell(0)?
            .boolean()?
        {
            tx.rollback().await?;
            return Ok(0);
        }
    }
    let previous = existing
        .map(|(attempts, _)| checked_attempts(attempts))
        .transpose()?;
    let attempts = previous
        .unwrap_or(0)
        .checked_add(1)
        .ok_or_else(|| sqlx::Error::Protocol("outbox attempt count overflow".into()))?;
    let base = i64::from(backoff_ms.max(1));
    let delay_ms = previous
        .map(|old| (base * (1_i64 << old.min(20))).min(60_000))
        .unwrap_or(base);
    let next = now
        .checked_add(delay_ms * 1000)
        .ok_or_else(|| sqlx::Error::Protocol("outbox retry instant overflow".into()))?;
    let dead = if attempts >= max_attempts.max(1) {
        Cell::Integer(now)
    } else {
        Cell::Null
    };
    // PostgreSQL left(text,4000) counts Unicode characters, not bytes.
    let error: String = error.chars().take(4000).collect();
    family.execute(
        "INSERT INTO outbox_failures(consumer,event_id,attempts,last_error,next_attempt_at,dead_at,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?7) ON CONFLICT(consumer,event_id) DO UPDATE SET attempts=excluded.attempts,last_error=excluded.last_error,next_attempt_at=excluded.next_attempt_at,dead_at=excluded.dead_at,updated_at=excluded.updated_at",
        &[Cell::text(consumer),Cell::uuid(event_id),Cell::Integer(i64::from(attempts)),Cell::text(error),Cell::Integer(next),dead,Cell::Integer(now)],
    ).await?;
    finish_write(tx).await?;
    Ok(attempts)
}

fn checked_attempts(value: i64) -> Result<i32, sqlx::Error> {
    let attempts = i32::try_from(value)
        .map_err(|_| sqlx::Error::Protocol("outbox attempt count exceeds i32".into()))?;
    if attempts <= 0 {
        return Err(sqlx::Error::Protocol(
            "outbox attempt count must be positive".into(),
        ));
    }
    Ok(attempts)
}

pub async fn fetch_failure_state_backend(
    backend: &Backend,
    consumer: &str,
    event_id: Uuid,
) -> Result<Option<OutboxFailureState>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return fetch_failure_state(pool, consumer, event_id).await;
    }
    let mut tx = backend.begin_read().await?;
    let rows = family_tx(&mut tx)?.query(
        "SELECT attempts,next_attempt_at,dead_at FROM outbox_failures WHERE consumer=?1 AND event_id=?2",
        &[Cell::text(consumer),Cell::uuid(event_id)],
    ).await?;
    let state = rows
        .first()
        .map(|row| {
            Ok::<_, sqlx::Error>(OutboxFailureState {
                attempts: checked_attempts(row.cell(0)?.integer()?)?,
                next_attempt_at: row.cell(1)?.datetime()?,
                dead_at: row.cell(2)?.optional(Cell::datetime)?,
            })
        })
        .transpose()?;
    tx.rollback().await?;
    Ok(state)
}

pub async fn requeue_backend(
    backend: &Backend,
    consumer: &str,
    event_id: Uuid,
) -> Result<bool, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return requeue(pool, consumer, event_id).await;
    }
    let mut tx = backend.begin_write().await?;
    let family = family_tx(&mut tx)?;
    let now = family_now(family).await?;
    let changed = family.execute(
        "UPDATE outbox_failures SET attempts=1,dead_at=NULL,next_attempt_at=?3,updated_at=?3 WHERE consumer=?1 AND event_id=?2 AND skipped_at IS NOT NULL",
        &[Cell::text(consumer),Cell::uuid(event_id),Cell::Integer(now)],
    ).await?;
    finish_write(tx).await?;
    Ok(changed == 1)
}

pub async fn clear_failure_backend(
    backend: &Backend,
    consumer: &str,
    event_id: Uuid,
) -> Result<bool, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return clear_failure(pool, consumer, event_id).await;
    }
    let mut tx = backend.begin_write().await?;
    let changed = family_tx(&mut tx)?
        .execute(
            "DELETE FROM outbox_failures WHERE consumer=?1 AND event_id=?2",
            &[Cell::text(consumer), Cell::uuid(event_id)],
        )
        .await?;
    finish_write(tx).await?;
    Ok(changed == 1)
}

pub async fn claim_retries_backend(
    backend: &Backend,
    consumer: &str,
    limit: i32,
) -> Result<Vec<OutboxRetry>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return claim_retries(pool, consumer, limit).await;
    }
    let mut tx = backend.begin_read().await?;
    let family = family_tx(&mut tx)?;
    let now = family_now(family).await?;
    let rows = family.query(
        "SELECT f.event_id,f.attempts,f.last_error FROM outbox_failures f JOIN outbox_consumers c ON c.consumer=f.consumer JOIN events e ON e.id=f.event_id WHERE f.consumer=?1 AND f.dead_at IS NULL AND f.skipped_at IS NOT NULL AND f.next_attempt_at<=?2 AND e.seq<=c.last_seq ORDER BY f.next_attempt_at,f.event_id LIMIT ?3",
        &[Cell::text(consumer),Cell::Integer(now),Cell::Integer(i64::from(limit.max(0)))],
    ).await?;
    let retries = rows
        .iter()
        .map(|row| {
            Ok::<_, sqlx::Error>(OutboxRetry {
                event_id: row.cell(0)?.id()?,
                attempts: checked_attempts(row.cell(1)?.integer()?)?,
                last_error: row.cell(2)?.string()?,
            })
        })
        .collect::<Result<_, _>>()?;
    tx.rollback().await?;
    Ok(retries)
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
