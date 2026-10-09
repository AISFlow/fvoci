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

use crate::db::backend::Backend;
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::db::search_index::{
    list_pending_embedding_backend, next_pending_embedding_backend,
    store_chunk_embeddings_backend_with_cancel,
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
    embed_attachment_chunks_backend(
        &Backend::Postgres(pool.clone()),
        embedder,
        workspace_id,
        attachment_id,
        cancel,
    )
    .await
}

pub async fn embed_attachment_chunks_backend(
    backend: &Backend,
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
        let pending = list_pending_embedding_backend(
            backend,
            workspace_id,
            attachment_id,
            EMBED_BATCH as i64,
        )
        .await?;
        if pending.is_empty() {
            return Ok(written);
        }
        let texts: Vec<String> = pending.iter().map(|c| c.text.clone()).collect();
        let vectors = tokio::select! {
            () = cancel.cancelled() => return Ok(written),
            result = embedder.embed(&texts) => result.map_err(EmbedPassError::Embed)?,
        };
        let rows: Vec<_> = pending.into_iter().zip(vectors).collect();
        let stored = store_chunk_embeddings_backend_with_cancel(
            backend,
            workspace_id,
            attachment_id,
            &rows,
            Some(cancel),
        )
        .await?;
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
    run_embed_pass_backend(&Backend::Postgres(pool.clone()), embedder, state, cancel).await
}

pub async fn run_embed_pass_backend(
    backend: &Backend,
    embedder: &Embedder,
    state: &mut EmbedBackoff,
    cancel: &CancellationToken,
) -> Result<EmbedPassOutcome, sqlx::Error> {
    let now = Instant::now();
    if state.pass_until.is_some_and(|until| until > now) {
        return Ok(EmbedPassOutcome::Waiting);
    }
    let excluded = state.excluded(now);
    let Some((workspace_id, attachment_id)) =
        next_pending_embedding_backend(backend, &excluded).await?
    else {
        return Ok(if excluded.is_empty() {
            EmbedPassOutcome::Idle
        } else {
            EmbedPassOutcome::Waiting
        });
    };
    match embed_attachment_chunks_backend(backend, embedder, workspace_id, attachment_id, cancel)
        .await
    {
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

#[cfg(test)]
mod backend_tests {
    use super::*;
    use crate::db::attachment_extract::{
        backend_tests::Fixture, finish_extract_backend, FinishExtract,
    };
    use crate::search::embed::EmbedderEnv;
    use axum::extract::State;
    use axum::http::StatusCode;
    use axum::{routing::post, Json, Router};
    use serde_json::{json, Value};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    use tokio::sync::Notify;

    #[derive(Default)]
    struct ProviderState {
        mode: AtomicUsize,
        seen: Notify,
        release: Notify,
        calls: AtomicUsize,
    }
    struct Provider {
        state: Arc<ProviderState>,
        embedder: Embedder,
        cancel: CancellationToken,
        join: tokio::task::JoinHandle<()>,
    }
    impl Provider {
        async fn new(mode: usize) -> Self {
            let state = Arc::new(ProviderState::default());
            state.mode.store(mode, Ordering::SeqCst);
            async fn respond(
                State(s): State<Arc<ProviderState>>,
                Json(request): Json<Value>,
            ) -> (StatusCode, Json<Value>) {
                s.calls.fetch_add(1, Ordering::SeqCst);
                let mode = s.mode.load(Ordering::SeqCst);
                s.seen.notify_one();
                if mode == 1 {
                    s.release.notified().await;
                }
                if mode == 2 {
                    return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({})));
                }
                if mode == 3 {
                    return (StatusCode::OK, Json(json!({"data":[]})));
                }
                let inputs = request["input"].as_array().unwrap();
                assert!(!inputs.is_empty());
                assert!(inputs.len() <= EMBED_BATCH);
                let mut vector = vec![0.0f32; crate::search::meili::EMBEDDING_DIMENSIONS as usize];
                vector[0] = 1.0;
                (
                    StatusCode::OK,
                    Json(
                        json!({"data":inputs.iter().enumerate().map(|(i,_)|json!({"index":i,"embedding":vector})).collect::<Vec<_>>()}),
                    ),
                )
            }
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let router = Router::new()
                .route("/embeddings", post(respond))
                .with_state(state.clone());
            let cancel = CancellationToken::new();
            let stopped = cancel.clone();
            let join = tokio::spawn(async move {
                axum::serve(listener, router)
                    .with_graceful_shutdown(stopped.cancelled_owned())
                    .await
                    .unwrap();
            });
            let base = format!("http://{address}");
            let embedder = Embedder::from_values(EmbedderEnv {
                enabled: Some("1"),
                base_url: Some(&base),
                allow_private: Some("1"),
                ..Default::default()
            })
            .unwrap()
            .unwrap();
            Self {
                state,
                embedder,
                cancel,
                join,
            }
        }
        async fn seen(&self) {
            tokio::time::timeout(Duration::from_secs(5), self.state.seen.notified())
                .await
                .unwrap();
        }
        async fn finish(self) {
            self.state.release.notify_one();
            self.cancel.cancel();
            self.join.await.unwrap();
        }
    }
    async fn vector_count(f: &Fixture) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM attachment_text WHERE embedding IS NOT NULL")
            .fetch_one(&f.pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn configured_backend_job_extracts_and_embeds_nonempty_local_text() {
        let f = Fixture::new().await;
        let provider = Provider::new(0).await;
        let storage = crate::attachments::ObjectStorage::local(f.directory.join("objects"));
        let text = "Actual configured embedding 안녕";
        storage
            .put_bytes(&f.storage_key, text.as_bytes().to_vec())
            .await
            .unwrap();
        sqlx::query("UPDATE attachments SET size_bytes=?1,reserved_size_bytes=?1")
            .bind(text.len() as i64)
            .execute(&f.pool)
            .await
            .unwrap();
        let settings = crate::attachments::ExtractJobSettings {
            extractor_bin: None,
            limits: crate::db::attachment_extract::default_extract_limits(),
            office_helper: None,
            office_limits: crate::documents::office::OfficeLimits::attachment(),
            poll_interval: Duration::from_secs(30),
            retry_backoff: Duration::from_millis(1000),
            #[cfg(feature = "extract-native-tests")]
            test_hang_ms: None,
        };
        let job = crate::attachments::spawn_extract_job_backend(
            settings,
            f.backend.clone(),
            storage,
            Some(provider.embedder.clone()),
        );
        tokio::time::timeout(Duration::from_secs(5), async {
            while f.event_count("attachment.embedded").await != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        job.request_shutdown();
        job.join().await.unwrap();
        assert!(vector_count(&f).await > 0);
        assert_eq!(f.event_count("attachment.extracted").await, 1);
        assert_eq!(provider.state.calls.load(Ordering::SeqCst), 1);
        let fresh = crate::db::pool::connect_sqlite_app(&f.path, 1)
            .await
            .unwrap();
        let v: String =
            sqlx::query_scalar("SELECT embedding FROM attachment_text ORDER BY chunk_no LIMIT 1")
                .fetch_one(&fresh)
                .await
                .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&v)
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            1536
        );
        fresh.close().await;
        provider.finish().await;
        f.finish().await;
    }

    #[tokio::test]
    async fn late_provider_response_after_reextract_never_overwrites_current_text() {
        let f = Fixture::new().await;
        f.extracted("Old text 안녕").await;
        let provider = Provider::new(1).await;
        let backend = f.backend.clone();
        let embedder = provider.embedder.clone();
        let w = f.workspace;
        let a = f.attachment;
        let pending = crate::db::search_index::list_pending_embedding_backend(
            &backend,
            w,
            a,
            EMBED_BATCH as i64,
        )
        .await
        .unwrap();
        assert!(!pending.is_empty());
        let call = tokio::spawn(async move {
            embed_attachment_chunks_backend(&backend, &embedder, w, a, &CancellationToken::new())
                .await
        });
        provider.seen().await;
        sqlx::query("UPDATE attachments SET extract_status='pending',extract_attempts=0")
            .execute(&f.pool)
            .await
            .unwrap();
        let claim = f.claim().await;
        assert!(finish_extract_backend(
            &f.backend,
            &claim,
            &FinishExtract {
                status: "ok".into(),
                text: "New current text 😀".into(),
                warnings: vec![],
                rhwp_rev: None
            }
        )
        .await
        .unwrap());
        provider.state.release.notify_one();
        assert_eq!(call.await.unwrap().unwrap(), 0);
        assert_eq!(vector_count(&f).await, 0);
        assert_eq!(f.event_count("attachment.embedded").await, 0);
        assert_eq!(provider.state.calls.load(Ordering::SeqCst), 1);
        provider.state.mode.store(0, Ordering::SeqCst);
        let mut state = EmbedBackoff::default();
        let outcome = run_embed_pass_backend(
            &f.backend,
            &provider.embedder,
            &mut state,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert!(matches!(outcome,EmbedPassOutcome::Embedded{chunks,..} if chunks>0));
        assert_eq!(f.event_count("attachment.embedded").await, 1);
        assert_eq!(
            crate::db::search_index::store_chunk_embeddings_backend(
                &f.backend,
                w,
                a,
                &[(pending[0].clone(), vec![1.0; 1536])]
            )
            .await
            .unwrap(),
            0
        );
        assert_eq!(f.event_count("attachment.embedded").await, 1);
        provider.finish().await;
        f.finish().await;
    }

    #[tokio::test]
    async fn cancelled_provider_wait_preserves_pending_chunks_and_joins_resources() {
        let f = Fixture::new().await;
        f.extracted("Cancel during network wait").await;
        let provider = Provider::new(1).await;
        let cancel = CancellationToken::new();
        let c = cancel.clone();
        let backend = f.backend.clone();
        let embedder = provider.embedder.clone();
        let call = tokio::spawn(async move {
            run_embed_pass_backend(&backend, &embedder, &mut EmbedBackoff::default(), &c).await
        });
        provider.seen().await;
        cancel.cancel();
        assert_eq!(call.await.unwrap().unwrap(), EmbedPassOutcome::Cancelled);
        assert_eq!(vector_count(&f).await, 0);
        assert_eq!(f.event_count("attachment.embedded").await, 0);
        assert!(
            crate::db::search_index::next_pending_embedding_backend(&f.backend, &[])
                .await
                .unwrap()
                .is_some()
        );
        provider.finish().await;
        f.finish().await;
    }

    #[tokio::test]
    async fn provider_failure_backoff_and_event_failure_leave_vectors_unchanged() {
        let f = Fixture::new().await;
        f.extracted("Provider and event failure").await;
        let provider = Provider::new(3).await;
        let mut state = EmbedBackoff::default();
        assert_eq!(
            run_embed_pass_backend(
                &f.backend,
                &provider.embedder,
                &mut state,
                &CancellationToken::new()
            )
            .await
            .unwrap(),
            EmbedPassOutcome::Waiting
        );
        assert_eq!(
            run_embed_pass_backend(
                &f.backend,
                &provider.embedder,
                &mut state,
                &CancellationToken::new()
            )
            .await
            .unwrap(),
            EmbedPassOutcome::Waiting
        );
        assert_eq!(provider.state.calls.load(Ordering::SeqCst), 1);
        assert_eq!(vector_count(&f).await, 0);
        provider.state.mode.store(0, Ordering::SeqCst);
        sqlx::raw_sql("CREATE TRIGGER fail_embed_event BEFORE INSERT ON events WHEN NEW.verb='attachment.embedded' BEGIN SELECT RAISE(ABORT,'fixture embedding event failure'); END;").execute(&f.pool).await.unwrap();
        assert!(run_embed_pass_backend(
            &f.backend,
            &provider.embedder,
            &mut EmbedBackoff::default(),
            &CancellationToken::new()
        )
        .await
        .is_err());
        assert_eq!(vector_count(&f).await, 0);
        assert_eq!(f.event_count("attachment.embedded").await, 0);
        sqlx::raw_sql("DROP TRIGGER fail_embed_event")
            .execute(&f.pool)
            .await
            .unwrap();
        let outcome = run_embed_pass_backend(
            &f.backend,
            &provider.embedder,
            &mut EmbedBackoff::default(),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert!(matches!(outcome,EmbedPassOutcome::Embedded{chunks,..} if chunks>0));
        assert_eq!(f.event_count("attachment.embedded").await, 1);
        provider.finish().await;
        f.finish().await;
    }
    #[tokio::test]
    async fn cancellation_during_embedding_writer_wait_does_not_publish() {
        let mut f = Fixture::new().await;
        f.extracted("Cancelled after provider response while awaiting writer")
            .await;
        let other = crate::db::pool::connect_sqlite_app(&f.path, 1)
            .await
            .unwrap();
        let reservation = other.begin_with("BEGIN IMMEDIATE").await.unwrap();
        let provider = Provider::new(1).await;
        let cancel = CancellationToken::new();
        let c = cancel.clone();
        let backend = f.backend.clone();
        let embedder = provider.embedder.clone();
        let call = tokio::spawn(async move {
            run_embed_pass_backend(&backend, &embedder, &mut EmbedBackoff::default(), &c).await
        });
        provider.seen().await;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let provider_boundary = tokio::time::timeout_at(deadline, async {
            // SQLx returns a committed connection in a spawned task. Prove
            // availability through the same public pool while the provider
            // remains blocked, rather than racing that task's idle snapshot.
            let probe = f.pool.acquire().await?;
            let pool_limits = (f.pool.options().get_max_connections(), f.pool.size());
            // Negative control: a held connection in this sole-connection
            // pool really prevents another public acquisition.
            let held_probe_denied = f.pool.try_acquire().is_none();
            drop(probe);
            while f.pool.num_idle() != 1 {
                tokio::task::yield_now().await;
            }
            let released_idle = f.pool.num_idle();
            provider.state.release.notify_one();
            while f.pool.num_idle() != 0 {
                tokio::task::yield_now().await;
            }
            Ok::<_, sqlx::Error>((pool_limits, held_probe_denied, released_idle))
        })
        .await; // One unchanged budget includes the actual writer wait.
        cancel.cancel();
        // A failed barrier must also release the provider and retire the
        // original writer/call before its failure is reported.
        if !matches!(&provider_boundary, Ok(Ok(_))) {
            provider.state.release.notify_one();
        }
        let reservation_finish = reservation.rollback().await;
        let outcome = call.await;
        let vectors = vector_count(&f).await;
        let events = f.event_count("attachment.embedded").await;
        provider.finish().await;
        other.close().await;
        f.backend.close().await.unwrap();
        f.pool.close().await;
        // Resources and the original failure database are closed before the
        // oracle; on failure its directory is deliberately retained.
        let (pool_limits, held_probe_denied, released_idle) = provider_boundary
            .expect("provider availability/release/writer wait exceeded the original budget")
            .expect("provider wait must leave the app connection available");
        assert_eq!(pool_limits, (1, 1));
        assert!(
            held_probe_denied,
            "a held app connection must block acquisition"
        );
        assert_eq!(released_idle, 1, "the probe must return before writer wait");
        reservation_finish.unwrap();
        let outcome = outcome.unwrap().unwrap();
        assert_eq!(outcome, EmbedPassOutcome::Cancelled);
        assert_eq!(
            vectors, 0,
            "cancelled writer wait must preserve NULL vectors"
        );
        assert_eq!(events, 0, "cancelled writer wait must not publish an event");
        let fresh = crate::db::pool::connect_sqlite_app(&f.path, 1)
            .await
            .unwrap();
        f.pool = fresh.clone();
        f.backend = Backend::Sqlite(fresh);
        let provider = Provider::new(0).await;
        let outcome = run_embed_pass_backend(
            &f.backend,
            &provider.embedder,
            &mut EmbedBackoff::default(),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert!(matches!(outcome,EmbedPassOutcome::Embedded{chunks,..} if chunks>0));
        assert!(vector_count(&f).await > 0);
        assert_eq!(f.event_count("attachment.embedded").await, 1);
        provider.finish().await;
        f.finish().await;
    }
}
