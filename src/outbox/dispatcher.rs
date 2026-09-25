use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use sqlx::PgPool;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};
use uuid::Uuid;

use crate::db::outbox::{
    advance_cursor, claim_retries, clear_failure, fetch_event_by_id, fetch_failure_state,
    is_processed, lease_consumer, mark_processed, read_events, record_failure, release_consumer,
    OutboxEvent, OUTBOX_DEFAULT_BATCH, OUTBOX_FAILURE_BACKOFF_MS, OUTBOX_LEASE_SECS,
    OUTBOX_MAX_ATTEMPTS,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryMode {
    /// Apply the consumer effect and advance the cursor in one transaction.
    PgOnly,
    /// Idempotent at-least-once delivery for external side effects.
    External,
}

#[derive(Debug, thiserror::Error)]
pub enum OutboxProcessError {
    #[error("delivery failed: {0}")]
    Delivery(String),
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
}

pub trait OutboxConsumer: Send + Sync {
    fn name(&self) -> &str;
    fn delivery_mode(&self) -> DeliveryMode;
    fn max_attempts(&self) -> i32 {
        OUTBOX_MAX_ATTEMPTS
    }

    /// Deliver one event. For `PgOnly`, implementations must apply their effect
    /// and advance the cursor in the same transaction via `advance_cursor_tx`.
    /// `External` implementations must be idempotent: a duplicate delivery after
    /// a crash or overlapping lease must converge to the same side effect.
    fn deliver<'a>(
        &'a self,
        pool: &'a PgPool,
        lease_owner: Uuid,
        event: &'a OutboxEvent,
    ) -> Pin<Box<dyn Future<Output = Result<(), OutboxProcessError>> + Send + 'a>>;
}

#[derive(Debug, Clone)]
pub struct OutboxDispatcherSettings {
    pub poll_interval: Duration,
    pub lease_ttl: Duration,
    pub batch_limit: i32,
    pub failure_backoff: Duration,
}

impl Default for OutboxDispatcherSettings {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(1),
            lease_ttl: Duration::from_secs(OUTBOX_LEASE_SECS as u64),
            batch_limit: OUTBOX_DEFAULT_BATCH,
            failure_backoff: Duration::from_millis(OUTBOX_FAILURE_BACKOFF_MS as u64),
        }
    }
}

impl OutboxDispatcherSettings {
    pub fn from_env() -> Self {
        let poll_secs = parse_positive_u64(
            "FVOCI_OUTBOX_POLL_SECS",
            std::env::var("FVOCI_OUTBOX_POLL_SECS").ok().as_deref(),
            1,
        )
        .unwrap_or(1);
        Self {
            poll_interval: Duration::from_secs(poll_secs),
            ..Self::default()
        }
    }

    fn backoff_ms(&self) -> i32 {
        self.failure_backoff.as_millis().clamp(1, 60_000) as i32
    }
}

pub struct OutboxDispatcherHandle {
    cancel: CancellationToken,
    join: tokio::task::JoinHandle<()>,
    pub wake: Arc<Notify>,
}

impl OutboxDispatcherHandle {
    pub fn request_shutdown(&self) {
        self.cancel.cancel();
    }

    pub async fn join(self) -> Result<(), String> {
        self.join
            .await
            .map_err(|err| format!("outbox dispatcher task join failed: {err}"))?;
        Ok(())
    }
}

pub fn spawn_outbox_dispatcher(
    settings: OutboxDispatcherSettings,
    pool: PgPool,
    consumers: Vec<Arc<dyn OutboxConsumer>>,
) -> Option<OutboxDispatcherHandle> {
    if consumers.is_empty() {
        return None;
    }
    let cancel = CancellationToken::new();
    let wake = Arc::new(Notify::new());
    let child_cancel = cancel.child_token();
    let join = tokio::spawn(run_dispatcher_loop(
        settings,
        pool,
        consumers,
        child_cancel,
        wake.clone(),
    ));
    Some(OutboxDispatcherHandle { cancel, join, wake })
}

async fn run_dispatcher_loop(
    settings: OutboxDispatcherSettings,
    pool: PgPool,
    consumers: Vec<Arc<dyn OutboxConsumer>>,
    cancel: CancellationToken,
    wake: Arc<Notify>,
) {
    let owners: Vec<(Arc<dyn OutboxConsumer>, Uuid)> = consumers
        .into_iter()
        .map(|consumer| (consumer, Uuid::now_v7()))
        .collect();

    while !cancel.is_cancelled() {
        let mut worked = false;
        for (consumer, owner) in &owners {
            if cancel.is_cancelled() {
                break;
            }
            match process_consumer_cycle(&settings, &pool, consumer, *owner, &cancel).await {
                Ok(did_work) => worked |= did_work,
                Err(err) => {
                    warn!(consumer = consumer.name(), error = %err, "outbox consumer cycle failed");
                }
            }
        }

        if cancel.is_cancelled() {
            break;
        }

        let delay = if worked {
            Duration::from_millis(50)
        } else {
            settings.poll_interval
        };
        tokio::select! {
            () = cancel.cancelled() => break,
            () = wake.notified() => {},
            () = tokio::time::sleep(delay) => {},
        }
    }

    for (consumer, owner) in owners {
        if let Err(err) = release_consumer(&pool, consumer.name(), owner).await {
            warn!(
                consumer = consumer.name(),
                error = %err,
                "outbox lease release during shutdown failed"
            );
        }
    }
}

async fn process_consumer_cycle(
    settings: &OutboxDispatcherSettings,
    pool: &PgPool,
    consumer: &Arc<dyn OutboxConsumer>,
    owner: Uuid,
    cancel: &CancellationToken,
) -> Result<bool, sqlx::Error> {
    let ttl_secs = settings.lease_ttl.as_secs().clamp(1, 3600) as i64;
    if !lease_consumer(pool, consumer.name(), owner, ttl_secs).await? {
        return Ok(false);
    }

    if cancel.is_cancelled() {
        let _ = release_consumer(pool, consumer.name(), owner).await?;
        return Ok(true);
    }

    let mut worked = process_retries(settings, pool, consumer, owner, cancel).await?;

    let events = read_events(pool, consumer.name(), settings.batch_limit).await?;

    for event in events {
        if cancel.is_cancelled() {
            let _ = release_consumer(pool, consumer.name(), owner).await?;
            return Ok(true);
        }
        if !lease_consumer(pool, consumer.name(), owner, ttl_secs).await? {
            return Ok(worked);
        }

        let failure = fetch_failure_state(pool, consumer.name(), event.id).await?;
        if failure.as_ref().is_some_and(|row| row.dead_at.is_some()) {
            let _ = advance_cursor(pool, consumer.name(), owner, &event.xact, event.seq).await?;
            worked = true;
            continue;
        }
        if failure
            .as_ref()
            .is_some_and(|row| row.next_attempt_at > Utc::now())
        {
            return Ok(worked);
        }

        worked = true;
        if let Err(err) = deliver_one(pool, consumer, owner, &event).await {
            warn!(
                consumer = consumer.name(),
                event_id = %event.id,
                error = %err,
                "outbox delivery failed"
            );
            handle_failure(settings, pool, consumer, owner, &event, &err.to_string()).await?;
            break;
        }
        let _ = clear_failure(pool, consumer.name(), event.id).await?;
    }

    Ok(worked)
}

async fn process_retries(
    settings: &OutboxDispatcherSettings,
    pool: &PgPool,
    consumer: &Arc<dyn OutboxConsumer>,
    owner: Uuid,
    cancel: &CancellationToken,
) -> Result<bool, sqlx::Error> {
    let retries = claim_retries(pool, consumer.name(), settings.batch_limit).await?;
    let mut worked = false;
    let ttl_secs = settings.lease_ttl.as_secs().clamp(1, 3600) as i64;
    for retry in retries {
        if cancel.is_cancelled() {
            return Ok(worked);
        }
        if !lease_consumer(pool, consumer.name(), owner, ttl_secs).await? {
            return Ok(worked);
        }
        let Some(event) = fetch_event_by_id(pool, retry.event_id).await? else {
            let _ = clear_failure(pool, consumer.name(), retry.event_id).await?;
            continue;
        };
        worked = true;
        if let Err(err) = deliver_retry(pool, consumer, owner, &event).await {
            handle_failure(settings, pool, consumer, owner, &event, &err.to_string()).await?;
        } else {
            let _ = clear_failure(pool, consumer.name(), event.id).await?;
        }
    }
    Ok(worked)
}

async fn deliver_retry(
    pool: &PgPool,
    consumer: &Arc<dyn OutboxConsumer>,
    owner: Uuid,
    event: &OutboxEvent,
) -> Result<(), OutboxProcessError> {
    match consumer.delivery_mode() {
        DeliveryMode::PgOnly => consumer.deliver(pool, owner, event).await?,
        DeliveryMode::External => {
            if is_processed(pool, consumer.name(), event.id).await? {
                return Ok(());
            }
            consumer.deliver(pool, owner, event).await?;
            let _ = mark_processed(pool, consumer.name(), event.id).await?;
        }
    }
    Ok(())
}

async fn deliver_one(
    pool: &PgPool,
    consumer: &Arc<dyn OutboxConsumer>,
    owner: Uuid,
    event: &OutboxEvent,
) -> Result<(), OutboxProcessError> {
    match consumer.delivery_mode() {
        DeliveryMode::PgOnly => consumer.deliver(pool, owner, event).await?,
        DeliveryMode::External => {
            if is_processed(pool, consumer.name(), event.id).await? {
                advance_cursor(pool, consumer.name(), owner, &event.xact, event.seq).await?;
                return Ok(());
            }
            consumer.deliver(pool, owner, event).await?;
            let _ = mark_processed(pool, consumer.name(), event.id).await?;
            if !advance_cursor(pool, consumer.name(), owner, &event.xact, event.seq).await? {
                return Err(OutboxProcessError::Delivery(
                    "cursor advance rejected after external delivery".into(),
                ));
            }
        }
    }
    debug!(
        consumer = consumer.name(),
        event_id = %event.id,
        xact = %event.xact,
        seq = event.seq,
        "outbox event delivered"
    );
    Ok(())
}

async fn handle_failure(
    settings: &OutboxDispatcherSettings,
    pool: &PgPool,
    consumer: &Arc<dyn OutboxConsumer>,
    owner: Uuid,
    event: &OutboxEvent,
    error: &str,
) -> Result<(), sqlx::Error> {
    let attempts = record_failure(
        pool,
        consumer.name(),
        event.id,
        error,
        settings.backoff_ms(),
        consumer.max_attempts(),
    )
    .await?;
    if attempts >= consumer.max_attempts() {
        warn!(
            consumer = consumer.name(),
            event_id = %event.id,
            attempts,
            "outbox event dead-lettered; advancing cursor"
        );
        let _ = advance_cursor(pool, consumer.name(), owner, &event.xact, event.seq).await?;
    }
    Ok(())
}

fn parse_positive_u64(name: &str, raw: Option<&str>, default: u64) -> Result<u64, String> {
    let Some(raw) = raw else {
        return Ok(default);
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(format!("{name} must be a positive integer"));
    }
    let value: u64 = trimmed
        .parse()
        .map_err(|e| format!("invalid {name}: {e}"))?;
    if value == 0 {
        return Err(format!("{name} must be a positive integer"));
    }
    Ok(value)
}
