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
    is_outbox_xid_epoch_mismatch, is_processed, lease_consumer, mark_processed, read_events,
    record_failure, release_consumer, OutboxEvent, OUTBOX_DEFAULT_BATCH, OUTBOX_FAILURE_BACKOFF_MS,
    OUTBOX_LEASE_SECS, OUTBOX_MAX_ATTEMPTS,
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
    /// Retrying an event at or below the cursor is allowed only after a
    /// dead-letter skip was requeued; `advance_cursor_tx` then returns true and
    /// deletes that failure row. `External` implementations must be idempotent:
    /// a duplicate delivery after a crash or overlapping lease must converge to
    /// the same side effect.
    fn deliver<'a>(
        &'a self,
        pool: &'a PgPool,
        lease_owner: Uuid,
        event: &'a OutboxEvent,
    ) -> Pin<Box<dyn Future<Output = Result<(), OutboxProcessError>> + Send + 'a>>;

    /// Deliver a read batch in order. Returns how many leading events are durably
    /// done (all their external effects confirmed), plus the first error if any.
    fn deliver_batch<'a>(
        &'a self,
        pool: &'a PgPool,
        lease_owner: Uuid,
        events: &'a [OutboxEvent],
    ) -> Pin<Box<dyn Future<Output = (usize, Option<OutboxProcessError>)> + Send + 'a>> {
        Box::pin(async move {
            let mut done = 0usize;
            for event in events {
                match self.deliver(pool, lease_owner, event).await {
                    Ok(()) => done += 1,
                    Err(err) => return (done, Some(err)),
                }
            }
            (done, None)
        })
    }

    /// Wall-clock budget for one `deliver_batch` call, independent of event count.
    /// When `Some(budget)` exceeds the dispatcher lease, at most one event is
    /// passed. `None` means the call is bounded only by the lease timeout and
    /// [`Self::batch_event_cap`].
    fn batch_time_budget(&self) -> Option<Duration> {
        None
    }

    /// Max events this consumer wants in one `deliver_batch`. The dispatcher
    /// also applies [`OutboxDispatcherSettings::batch_limit`].
    fn batch_event_cap(&self) -> usize {
        usize::MAX
    }

    /// Work the consumer owns beyond the event stream, e.g. sending the
    /// per-target retries that `deliver` fanned out into its own table. Runs
    /// under this consumer's lease after every event pass, cancelled at
    /// `budget` (the lease minus a margin). Returns whether anything was done.
    fn run_due<'a>(
        &'a self,
        _pool: &'a PgPool,
        _budget: Duration,
    ) -> Pin<Box<dyn Future<Output = Result<bool, OutboxProcessError>> + Send + 'a>> {
        Box::pin(async { Ok(false) })
    }
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

    let mut worked = process_event_pass(settings, pool, consumer, owner, cancel, ttl_secs).await?;
    if cancel.is_cancelled() || !lease_consumer(pool, consumer.name(), owner, ttl_secs).await? {
        return Ok(worked);
    }
    let budget = lease_batch_timeout(settings.lease_ttl);
    match tokio::time::timeout(budget, consumer.run_due(pool, budget)).await {
        Ok(Ok(did)) => worked |= did,
        Ok(Err(err)) => {
            warn!(consumer = consumer.name(), error = %err, "outbox consumer due work failed");
        }
        Err(_) => {
            warn!(
                consumer = consumer.name(),
                "outbox consumer due work exceeded lease budget"
            );
        }
    }
    Ok(worked)
}

async fn process_event_pass(
    settings: &OutboxDispatcherSettings,
    pool: &PgPool,
    consumer: &Arc<dyn OutboxConsumer>,
    owner: Uuid,
    cancel: &CancellationToken,
    ttl_secs: i64,
) -> Result<bool, sqlx::Error> {
    // Fail closed on restore xid epoch before the retry sweep. `read` evaluates
    // snapshot and comparison in one statement; a separate xmax helper raced.
    let events = match read_events(pool, consumer.name(), settings.batch_limit).await {
        Ok(events) => events,
        Err(err) if is_outbox_xid_epoch_mismatch(&err) => {
            tracing::error!(
                consumer = consumer.name(),
                "outbox xid epoch mismatch; refuse to advance; run fvoci-migrate --recover-outbox"
            );
            return Ok(false);
        }
        Err(err) => return Err(err),
    };

    let mut worked = process_retries(settings, pool, consumer, owner, cancel).await?;

    if consumer.delivery_mode() == DeliveryMode::External {
        worked |=
            process_external_events(settings, pool, consumer, owner, cancel, ttl_secs, events)
                .await?;
    } else {
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
                let _ =
                    advance_cursor(pool, consumer.name(), owner, &event.xact, event.seq).await?;
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
    }

    Ok(worked)
}

async fn process_external_events(
    settings: &OutboxDispatcherSettings,
    pool: &PgPool,
    consumer: &Arc<dyn OutboxConsumer>,
    owner: Uuid,
    cancel: &CancellationToken,
    ttl_secs: i64,
    events: Vec<OutboxEvent>,
) -> Result<bool, sqlx::Error> {
    let mut worked = false;
    let mut pending: Vec<OutboxEvent> = Vec::new();

    for event in events {
        if cancel.is_cancelled() {
            if !pending.is_empty() {
                deliver_external_pending(settings, pool, consumer, owner, ttl_secs, &pending)
                    .await?;
            }
            let _ = release_consumer(pool, consumer.name(), owner).await?;
            return Ok(true);
        }

        let failure = fetch_failure_state(pool, consumer.name(), event.id).await?;
        if failure.as_ref().is_some_and(|row| row.dead_at.is_some()) {
            if !pending.is_empty() {
                // Do not advance the cursor past undelivered pending events.
                break;
            }
            let _ = advance_cursor(pool, consumer.name(), owner, &event.xact, event.seq).await?;
            worked = true;
            continue;
        }
        if failure
            .as_ref()
            .is_some_and(|row| row.next_attempt_at > Utc::now())
        {
            break;
        }

        if is_processed(pool, consumer.name(), event.id).await? {
            if !pending.is_empty() {
                break;
            }
            if !advance_cursor(pool, consumer.name(), owner, &event.xact, event.seq).await? {
                warn!(
                    consumer = consumer.name(),
                    event_id = %event.id,
                    "cursor advance rejected for already-processed event"
                );
                return Ok(worked);
            }
            debug!(
                consumer = consumer.name(),
                event_id = %event.id,
                xact = %event.xact,
                seq = event.seq,
                "outbox event already processed"
            );
            worked = true;
            continue;
        }

        pending.push(event);
    }

    if !pending.is_empty() {
        worked |=
            deliver_external_pending(settings, pool, consumer, owner, ttl_secs, &pending).await?;
    }

    Ok(worked)
}

fn external_deliver_chunk_len(
    settings: &OutboxDispatcherSettings,
    consumer: &dyn OutboxConsumer,
    pending_len: usize,
) -> usize {
    let cap = pending_len
        .min(settings.batch_limit.max(1) as usize)
        .min(consumer.batch_event_cap().max(1));
    match consumer.batch_time_budget() {
        Some(budget) if budget > settings.lease_ttl => cap.min(1),
        _ => cap,
    }
}

fn lease_batch_timeout(lease: Duration) -> Duration {
    let margin = Duration::from_millis(500);
    lease.saturating_sub(margin).max(Duration::from_millis(1))
}

async fn deliver_external_pending(
    settings: &OutboxDispatcherSettings,
    pool: &PgPool,
    consumer: &Arc<dyn OutboxConsumer>,
    owner: Uuid,
    ttl_secs: i64,
    pending: &[OutboxEvent],
) -> Result<bool, sqlx::Error> {
    let mut offset = 0usize;
    let mut worked = false;
    while offset < pending.len() {
        if !lease_consumer(pool, consumer.name(), owner, ttl_secs).await? {
            return Ok(worked);
        }
        let remaining = &pending[offset..];
        let chunk_len = external_deliver_chunk_len(settings, consumer.as_ref(), remaining.len());
        if chunk_len == 0 {
            break;
        }
        let chunk = &remaining[..chunk_len];
        let outcome = deliver_external_chunk(settings, pool, consumer, owner, chunk).await?;
        worked = true;
        offset += outcome.done;
        if outcome.stop {
            break;
        }
    }
    Ok(worked)
}

struct ExternalChunkOutcome {
    done: usize,
    stop: bool,
}

async fn deliver_external_chunk(
    settings: &OutboxDispatcherSettings,
    pool: &PgPool,
    consumer: &Arc<dyn OutboxConsumer>,
    owner: Uuid,
    chunk: &[OutboxEvent],
) -> Result<ExternalChunkOutcome, sqlx::Error> {
    if chunk.is_empty() {
        return Ok(ExternalChunkOutcome {
            done: 0,
            stop: true,
        });
    }

    let (done, err) = match tokio::time::timeout(
        lease_batch_timeout(settings.lease_ttl),
        consumer.deliver_batch(pool, owner, chunk),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => (
            0,
            Some(OutboxProcessError::Delivery(
                "deliver_batch exceeded lease budget".into(),
            )),
        ),
    };
    let done = done.min(chunk.len());

    for event in chunk.iter().take(done) {
        let _ = mark_processed(pool, consumer.name(), event.id).await?;
        if !advance_cursor(pool, consumer.name(), owner, &event.xact, event.seq).await? {
            warn!(
                consumer = consumer.name(),
                event_id = %event.id,
                "cursor advance rejected after external delivery"
            );
            return Ok(ExternalChunkOutcome { done, stop: true });
        }
        let _ = clear_failure(pool, consumer.name(), event.id).await?;
        debug!(
            consumer = consumer.name(),
            event_id = %event.id,
            xact = %event.xact,
            seq = event.seq,
            "outbox event delivered"
        );
    }

    if let Some(err) = err {
        if let Some(failed) = chunk.get(done) {
            warn!(
                consumer = consumer.name(),
                event_id = %failed.id,
                error = %err,
                "outbox delivery failed"
            );
            handle_failure(settings, pool, consumer, owner, failed, &err.to_string()).await?;
        } else {
            warn!(
                consumer = consumer.name(),
                done,
                chunk = chunk.len(),
                error = %err,
                "deliver_batch returned error after completing the chunk"
            );
        }
        return Ok(ExternalChunkOutcome { done, stop: true });
    }

    Ok(ExternalChunkOutcome {
        done,
        stop: done < chunk.len(),
    })
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
    consumer.deliver(pool, owner, event).await?;
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
        owner,
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
