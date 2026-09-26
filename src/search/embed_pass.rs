//! Index-time embedding of attachment text chunks (source `embedChunks` in
//! `packages/jobs/src/extract-text.ts`).
//!
//! The source embeds right after extraction and leaves failures to the job
//! retry plus a manual `fvoci reindex` backfill. Here the attachment extract
//! loop runs one pass after each claim cycle: it picks the next attachment
//! with chunks lacking a vector (a fresh extraction, an older extraction from
//! before the embedder was configured, or an earlier failure), embeds its
//! pending chunks in batches of [`EMBED_BATCH`] and stores them with an
//! `attachment.embedded` event, so the search-index consumer copies the
//! vectors into Meili. Indexing never waits on the provider: chunks are
//! indexed lexically first and gain their vector later.
//!
//! A failed attachment backs off on its own (so one bad input cannot starve
//! the rest) and the whole pass backs off too (so a provider outage is not
//! hammered attachment by attachment).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::db::search_index::{
    list_pending_embedding, next_pending_embedding, store_chunk_embeddings,
};
use crate::search::embed::{EmbedError, Embedder, EMBED_BATCH};

const BACKOFF_BASE: Duration = Duration::from_secs(5);
const BACKOFF_MAX: Duration = Duration::from_secs(15 * 60);
/// Excluded ids are bound into one query; keep the list bounded.
const MAX_TRACKED_FAILURES: usize = 1000;

#[derive(Debug)]
pub enum EmbedPassError {
    Db(sqlx::Error),
    Embed(EmbedError),
}

impl std::fmt::Display for EmbedPassError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Db(err) => write!(f, "database error: {err}"),
            Self::Embed(err) => write!(f, "{err}"),
        }
    }
}

impl From<sqlx::Error> for EmbedPassError {
    fn from(value: sqlx::Error) -> Self {
        Self::Db(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbedPassOutcome {
    /// Nothing is waiting for a vector.
    Idle,
    /// Backing off after a failure.
    Waiting,
    Cancelled,
    Embedded {
        attachment_id: Uuid,
        chunks: u64,
    },
}

fn backoff(attempts: u32) -> Duration {
    BACKOFF_BASE
        .saturating_mul(2u32.saturating_pow(attempts.saturating_sub(1).min(16)))
        .min(BACKOFF_MAX)
}

/// Failure bookkeeping for one loop (process-local; a restart retries at once).
#[derive(Debug, Default)]
pub struct EmbedBackoff {
    attachments: HashMap<Uuid, (u32, Instant)>,
    pass_until: Option<Instant>,
    pass_failures: u32,
}

impl EmbedBackoff {
    fn excluded(&mut self, now: Instant) -> Vec<Uuid> {
        self.attachments.retain(|_, (_, until)| *until > now);
        self.attachments.keys().copied().collect()
    }

    fn failed(&mut self, attachment_id: Uuid, now: Instant) {
        if self.attachments.len() >= MAX_TRACKED_FAILURES
            && !self.attachments.contains_key(&attachment_id)
        {
            return;
        }
        let entry = self.attachments.entry(attachment_id).or_insert((0, now));
        entry.0 = entry.0.saturating_add(1);
        entry.1 = now + backoff(entry.0);
        self.pass_failures = self.pass_failures.saturating_add(1);
        self.pass_until = Some(now + backoff(self.pass_failures));
    }

    fn succeeded(&mut self, attachment_id: Uuid) {
        self.attachments.remove(&attachment_id);
        self.pass_failures = 0;
        self.pass_until = None;
    }
}

/// Embeds every pending chunk of one attachment. Stops early (returning what
/// was stored) when cancelled; a provider or validation error is returned
/// after the batches before it were stored.
pub async fn embed_attachment_chunks(
    pool: &PgPool,
    embedder: &Embedder,
    workspace_id: Uuid,
    attachment_id: Uuid,
    cancel: &CancellationToken,
) -> Result<u64, EmbedPassError> {
    let mut written = 0u64;
    loop {
        if cancel.is_cancelled() {
            return Ok(written);
        }
        let pending =
            list_pending_embedding(pool, workspace_id, attachment_id, EMBED_BATCH as i64).await?;
        if pending.is_empty() {
            return Ok(written);
        }
        let texts: Vec<String> = pending.iter().map(|c| c.text.clone()).collect();
        let vectors = tokio::select! {
            () = cancel.cancelled() => return Ok(written),
            result = embedder.embed(&texts) => result.map_err(EmbedPassError::Embed)?,
        };
        let rows: Vec<_> = pending.into_iter().zip(vectors).collect();
        let stored = store_chunk_embeddings(pool, workspace_id, attachment_id, &rows).await?;
        written += stored;
        if stored == 0 {
            // Every chunk changed under us (re-extracted); the next pass sees
            // the new rows. Never spin on the same batch.
            return Ok(written);
        }
    }
}

/// One step of the backfill: the next attachment with pending chunks.
pub async fn run_embed_pass(
    pool: &PgPool,
    embedder: &Embedder,
    state: &mut EmbedBackoff,
    cancel: &CancellationToken,
) -> Result<EmbedPassOutcome, sqlx::Error> {
    let now = Instant::now();
    if state.pass_until.is_some_and(|until| until > now) {
        return Ok(EmbedPassOutcome::Waiting);
    }
    let excluded = state.excluded(now);
    let Some((workspace_id, attachment_id)) = next_pending_embedding(pool, &excluded).await? else {
        return Ok(if excluded.is_empty() {
            EmbedPassOutcome::Idle
        } else {
            EmbedPassOutcome::Waiting
        });
    };
    match embed_attachment_chunks(pool, embedder, workspace_id, attachment_id, cancel).await {
        Ok(_) if cancel.is_cancelled() => Ok(EmbedPassOutcome::Cancelled),
        Ok(chunks) => {
            state.succeeded(attachment_id);
            Ok(EmbedPassOutcome::Embedded {
                attachment_id,
                chunks,
            })
        }
        Err(EmbedPassError::Db(err)) => Err(err),
        Err(EmbedPassError::Embed(err)) => {
            // The error carries no URL, secret or provider body.
            tracing::warn!(
                %workspace_id,
                %attachment_id,
                error = %err,
                "attachment chunk embedding failed; lexical search unaffected, retrying later"
            );
            state.failed(attachment_id, Instant::now());
            Ok(EmbedPassOutcome::Waiting)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_and_caps() {
        assert_eq!(backoff(1), Duration::from_secs(5));
        assert_eq!(backoff(2), Duration::from_secs(10));
        assert_eq!(backoff(40), BACKOFF_MAX);
    }

    #[test]
    fn failed_attachment_is_excluded_until_due_and_success_clears_pass_wait() {
        let mut state = EmbedBackoff::default();
        let now = Instant::now();
        let id = Uuid::now_v7();
        state.failed(id, now);
        assert_eq!(state.excluded(now), vec![id]);
        assert!(state.pass_until.is_some());
        assert!(state.excluded(now + Duration::from_secs(6)).is_empty());
        state.failed(id, now);
        state.succeeded(id);
        assert!(state.pass_until.is_none());
        assert!(state.excluded(now).is_empty());
    }
}
