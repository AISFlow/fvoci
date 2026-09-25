use std::future::{poll_fn, Future, IntoFuture};
use std::io::Write;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use tokio::signal;
use tokio::task::JoinHandle;
use tracing_subscriber::EnvFilter;

use fvoci_server::attachments::{spawn_extract_job, ExtractJobHandle, ExtractJobSettings};
use fvoci_server::auth::AuthService;
use fvoci_server::collab::hub::ShutdownStatus;
use fvoci_server::collab::{CollabConfig, CollabHub};
use fvoci_server::config::Config;
use fvoci_server::db::{migrate, pool, Db};
use fvoci_server::http::rate_limit::RateLimiter;
use fvoci_server::http::{router, state::AppState};
use fvoci_server::outbox::{
    spawn_outbox_dispatcher, OutboxDispatcherHandle, OutboxDispatcherSettings,
};

#[derive(Debug)]
struct ShutdownDeadlineExceeded {
    rooms: Option<usize>,
    sockets_held: usize,
}

impl std::fmt::Display for ShutdownDeadlineExceeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "server shutdown deadline exceeded (rooms={:?}, sockets_held={})",
            self.rooms, self.sockets_held
        )
    }
}

impl std::error::Error for ShutdownDeadlineExceeded {}

#[derive(Debug)]
struct ShutdownObservedFailure {
    idle_task_failed: bool,
    start_task_failures: usize,
    actor_failures: usize,
}

impl std::fmt::Display for ShutdownObservedFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "server shutdown failed (idle_task_failed={}, start_task_failures={}, actor_failures={})",
            self.idle_task_failed, self.start_task_failures, self.actor_failures
        )
    }
}

impl std::error::Error for ShutdownObservedFailure {}

#[derive(Debug)]
struct ShutdownTaskPanicked;

impl std::fmt::Display for ShutdownTaskPanicked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "server shutdown failed (collaboration task panicked)")
    }
}

impl std::error::Error for ShutdownTaskPanicked {}

struct HubShutdownTask {
    join: JoinHandle<ShutdownStatus>,
    finished: Option<tokio::sync::oneshot::Receiver<ShutdownStatus>>,
}

#[derive(Debug)]
enum HubOutcome {
    Clean,
    Failed(ShutdownStatus),
    Panicked,
}

struct DrainOutcome {
    serve: Result<(), std::io::Error>,
    hub: HubOutcome,
    extract: Result<(), String>,
    outbox: Result<(), String>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("fvoci_server=info".parse()?))
        .init();

    let config = Config::from_env()?;

    let pool = pool::connect_app(&config.app_database_url).await?;
    if let Err(message) = migrate::assert_app_role(&pool).await {
        pool.close().await;
        return Err(message.into());
    }
    if let Err(message) = migrate::assert_schema_current(&pool).await {
        pool.close().await;
        return Err(message.into());
    }
    if let Some(meili) = config.meili.as_ref() {
        use fvoci_server::search::meili::MeiliError;
        match fvoci_server::search::meili::ensure_meili_index(meili).await {
            Ok(()) => {
                tracing::info!(url = %meili.url, index = %meili.index_uid, "meilisearch enabled");
            }
            // A rejected key is a configuration error: refuse to start.
            Err(error @ (MeiliError::Http(401) | MeiliError::Http(403) | MeiliError::Config)) => {
                pool.close().await;
                return Err(format!("meilisearch configuration rejected: {error}").into());
            }
            // Like the source, an unavailable Meili must not take documents and
            // collaboration down; search ensures the index lazily and reports a
            // problem response until Meili is reachable.
            Err(error) => {
                tracing::warn!(url = %meili.url, index = %meili.index_uid, %error, "meilisearch not ready at startup; search will retry lazily");
            }
        }
    } else {
        tracing::info!("meilisearch disabled (FVOCI_MEILI_URL unset)");
    }
    run_server(config, pool).await
}

struct InstalledShutdownSignals {
    #[cfg(unix)]
    interrupt: signal::unix::Signal,
    #[cfg(unix)]
    terminate: signal::unix::Signal,
}

fn install_shutdown_signals() -> std::io::Result<InstalledShutdownSignals> {
    #[cfg(unix)]
    {
        Ok(InstalledShutdownSignals {
            interrupt: signal::unix::signal(signal::unix::SignalKind::interrupt())?,
            terminate: signal::unix::signal(signal::unix::SignalKind::terminate())?,
        })
    }
    #[cfg(not(unix))]
    {
        Ok(InstalledShutdownSignals {})
    }
}

async fn wait_installed_shutdown_signals(mut signals: InstalledShutdownSignals) {
    #[cfg(unix)]
    {
        tokio::select! {
            result = signals.interrupt.recv() => {
                if result.is_some() {
                    eprintln!("shutdown signal received (Ctrl+C)");
                }
            }
            result = signals.terminate.recv() => {
                if result.is_some() {
                    eprintln!("shutdown signal received (SIGTERM)");
                }
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = signals;
        if signal::ctrl_c().await.is_ok() {
            eprintln!("shutdown signal received (Ctrl+C)");
        }
    }
}

/// Runs `announce` once, after the first poll of `fut` left it pending (the
/// accept loop and shutdown wait are armed). A future that completes on its
/// first poll never announces readiness.
async fn announce_after_first_pending_poll<F, A>(fut: F, announce: A) -> F::Output
where
    F: Future,
    A: FnOnce(),
{
    tokio::pin!(fut);
    let mut announce = Some(announce);
    poll_fn(move |cx| {
        let output = fut.as_mut().poll(cx);
        if output.is_pending() {
            if let Some(announce) = announce.take() {
                announce();
            }
        } else {
            announce = None;
        }
        output
    })
    .await
}

async fn run_server(config: Config, pool: sqlx::PgPool) -> Result<(), Box<dyn std::error::Error>> {
    // Replace the default SIGTERM/SIGINT handlers before bind or any readiness
    // advertisement. Tokio buffers signals received between install and recv.
    let shutdown_signals = install_shutdown_signals()?;

    let listener = tokio::net::TcpListener::bind(config.bind).await?;
    let addr = listener.local_addr()?;
    let public_origin =
        fvoci_server::http::guard::resolve_public_origin(&config.public_origin, addr)?;

    let collab = CollabConfig::from_env().map(|cfg| Arc::new(CollabHub::new(cfg, pool.clone())));
    let extract_job = match ExtractJobSettings::from_env()? {
        Some(settings) => {
            tracing::info!(
                extractor = %settings.extractor_bin.display(),
                "attachment native extraction enabled"
            );
            Some(spawn_extract_job(
                settings,
                pool.clone(),
                fvoci_server::attachments::LocalStorage::new(config.storage_root.clone()),
            ))
        }
        None => {
            tracing::info!("attachment native extraction disabled (FVOCI_EXTRACTOR_BIN unset)");
            None
        }
    };
    let mut consumers: Vec<std::sync::Arc<dyn fvoci_server::outbox::OutboxConsumer>> = Vec::new();
    if let Some(meili) = config.meili.clone() {
        consumers.push(fvoci_server::search::index::search_index_consumer(meili));
    }
    let outbox_dispatcher = spawn_outbox_dispatcher(
        OutboxDispatcherSettings::from_env(),
        pool.clone(),
        consumers,
    );
    if outbox_dispatcher.is_some() {
        tracing::info!("outbox dispatcher started");
    } else {
        tracing::info!("outbox dispatcher idle (no consumers registered)");
    }
    let state = AppState {
        auth: Arc::new(AuthService {
            db: Db::new(pool.clone()),
            password_keys: config.password_keys.clone(),
        }),
        branding_name: config.branding_name.clone(),
        public_origin,
        cookie_secure: config.cookie_secure,
        rate_limiter: RateLimiter::new(),
        storage: fvoci_server::attachments::LocalStorage::new(config.storage_root.clone()),
        upload: config.upload.clone(),
        collab: collab.clone(),
        meili: config.meili.clone(),
    };

    let deadline = config.shutdown_deadline;
    let (signaled_tx, signaled_rx) = tokio::sync::oneshot::channel::<Instant>();
    let hub_task = Arc::new(tokio::sync::Mutex::new(None::<HubShutdownTask>));
    let extract_task = Arc::new(tokio::sync::Mutex::new(extract_job));
    let outbox_task = Arc::new(tokio::sync::Mutex::new(outbox_dispatcher));
    let collab_for_signal = collab.clone();
    let hub_task_for_signal = hub_task.clone();
    let extract_task_for_signal = extract_task.clone();
    let outbox_task_for_signal = outbox_task.clone();

    let serve = announce_after_first_pending_poll(
        axum::serve(
            listener,
            router(state, config.static_dir.clone())
                .into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async move {
            wait_installed_shutdown_signals(shutdown_signals).await;
            let started = Instant::now();
            if let Some(job) = extract_task_for_signal.lock().await.as_ref() {
                job.request_shutdown();
                tracing::info!("attachment extract shutdown started concurrently with HTTP drain");
            }
            if let Some(job) = outbox_task_for_signal.lock().await.as_ref() {
                job.request_shutdown();
                tracing::info!("outbox dispatcher shutdown started concurrently with HTTP drain");
            }
            if let Some(hub) = collab_for_signal {
                hub.begin_shutdown();
                let (finished_tx, finished_rx) = tokio::sync::oneshot::channel();
                let join = tokio::spawn(async move {
                    let status = hub.shutdown().await;
                    let _ = finished_tx.send(status);
                    status
                });
                *hub_task_for_signal.lock().await = Some(HubShutdownTask {
                    join,
                    finished: Some(finished_rx),
                });
                tracing::info!("collaboration shutdown started concurrently with HTTP drain");
            }
            let _ = signaled_tx.send(started);
        })
        .into_future(),
        move || {
            eprintln!("fvoci-server listening on http://{addr}");
            let _ = std::io::stderr().flush();
        },
    );

    let mut serve_task = tokio::spawn(serve);
    let mut signaled_rx = Some(signaled_rx);
    tokio::select! {
        biased;
        serve_result = &mut serve_task => {
            let started = signaled_rx
                .take()
                .and_then(|mut rx| rx.try_recv().ok())
                .unwrap_or_else(Instant::now);
            let drain_pool = pool.clone();
            wait_for_deadline(
                async {
                    let hub = join_hub_finished(&hub_task, collab.clone()).await;
                    let extract = join_extract_finished(&extract_task).await;
                    let outbox = join_outbox_finished(&outbox_task).await;
                    drain_pool.close().await;
                    DrainOutcome {
                        serve: map_serve_result(serve_result),
                        hub,
                        extract,
                        outbox,
                    }
                },
                Some(started),
                deadline,
                &hub_task,
                collab.as_ref(),
            )
            .await
        }
        started = async {
            match signaled_rx.as_mut() {
                Some(rx) => rx.await.ok(),
                None => None,
            }
        } => {
            let _ = signaled_rx.take();
            let drain_pool = pool.clone();
            wait_for_deadline(
                async {
                    let serve = map_serve_result(serve_task.await);
                    let hub = join_hub_finished(&hub_task, collab.clone()).await;
                    let extract = join_extract_finished(&extract_task).await;
                    let outbox = join_outbox_finished(&outbox_task).await;
                    drain_pool.close().await;
                    DrainOutcome {
                        serve,
                        hub,
                        extract,
                        outbox,
                    }
                },
                started,
                deadline,
                &hub_task,
                collab.as_ref(),
            )
            .await
        }
    }
}

fn map_serve_result(
    result: Result<Result<(), std::io::Error>, tokio::task::JoinError>,
) -> Result<(), std::io::Error> {
    match result {
        Ok(result) => result,
        Err(error) => Err(std::io::Error::other(error)),
    }
}

async fn join_extract_finished(
    extract_task: &tokio::sync::Mutex<Option<ExtractJobHandle>>,
) -> Result<(), String> {
    if let Some(job) = extract_task.lock().await.take() {
        job.request_shutdown();
        job.join().await?;
    }
    Ok(())
}

async fn join_outbox_finished(
    outbox_task: &tokio::sync::Mutex<Option<OutboxDispatcherHandle>>,
) -> Result<(), String> {
    if let Some(job) = outbox_task.lock().await.take() {
        job.request_shutdown();
        job.join().await?;
    }
    Ok(())
}

async fn join_hub_finished(
    hub_task: &tokio::sync::Mutex<Option<HubShutdownTask>>,
    collab: Option<Arc<CollabHub>>,
) -> HubOutcome {
    let finished = hub_task
        .lock()
        .await
        .as_mut()
        .and_then(|task| task.finished.take());
    if let Some(finished) = finished {
        match finished.await {
            Ok(status) if status.is_clean() => HubOutcome::Clean,
            Ok(status) => HubOutcome::Failed(status),
            Err(_) => HubOutcome::Panicked,
        }
    } else if let Some(hub) = collab {
        status_outcome(hub.shutdown().await)
    } else {
        HubOutcome::Clean
    }
}

fn status_outcome(status: ShutdownStatus) -> HubOutcome {
    if status.is_clean() {
        HubOutcome::Clean
    } else {
        HubOutcome::Failed(status)
    }
}

async fn wait_for_deadline<F>(
    work: F,
    started: Option<Instant>,
    deadline: std::time::Duration,
    hub_task: &tokio::sync::Mutex<Option<HubShutdownTask>>,
    collab: Option<&Arc<CollabHub>>,
) -> Result<(), Box<dyn std::error::Error>>
where
    F: std::future::Future<Output = DrainOutcome>,
{
    let remaining = match started {
        Some(started) => {
            let remaining = deadline.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                detach_hub_task(hub_task).await;
                return shutdown_deadline_error(collab, deadline);
            }
            remaining
        }
        None => deadline,
    };

    // The final task join belongs to the same deadline as HTTP, rooms and DB.
    // A completion notification is not proof that its owning task has exited.
    let drained = async {
        let outcome = work.await;
        let joined = join_owned_hub_task(hub_task).await;
        (outcome, joined)
    };
    match tokio::time::timeout(remaining, drained).await {
        Ok((outcome, joined)) => {
            if let Some(error) = hub_failure_error(joined)
                .or_else(|| hub_failure_error(outcome.hub))
                .or_else(|| extract_failure_error(outcome.extract))
                .or_else(|| extract_failure_error(outcome.outbox))
            {
                return Err(error);
            }
            outcome.serve?;
            Ok(())
        }
        Err(_) => {
            detach_hub_task(hub_task).await;
            shutdown_deadline_error(collab, deadline)
        }
    }
}

async fn join_owned_hub_task(hub_task: &tokio::sync::Mutex<Option<HubShutdownTask>>) -> HubOutcome {
    match hub_task.lock().await.take() {
        Some(task) => match task.join.await {
            Ok(status) => status_outcome(status),
            Err(_) => HubOutcome::Panicked,
        },
        None => HubOutcome::Clean,
    }
}

fn hub_failure_error(outcome: HubOutcome) -> Option<Box<dyn std::error::Error>> {
    match outcome {
        HubOutcome::Clean => None,
        HubOutcome::Failed(status) => Some(shutdown_status_error(status)),
        HubOutcome::Panicked => Some(shutdown_panic_error()),
    }
}

fn extract_failure_error(result: Result<(), String>) -> Option<Box<dyn std::error::Error>> {
    match result {
        Ok(()) => None,
        Err(message) => Some(std::io::Error::other(message).into()),
    }
}

async fn detach_hub_task(hub_task: &tokio::sync::Mutex<Option<HubShutdownTask>>) {
    // Tokio JoinHandle drop detaches; it does not abort and does not reap.
    let _ = hub_task.lock().await.take();
}

fn shutdown_deadline_error(
    collab: Option<&Arc<CollabHub>>,
    deadline: std::time::Duration,
) -> Result<(), Box<dyn std::error::Error>> {
    let progress = collab.map(|hub| hub.shutdown_progress());
    let rooms = progress.and_then(|p| p.rooms);
    let sockets_held = progress.map(|p| p.sockets_held).unwrap_or(0);
    eprintln!(
        "server shutdown deadline exceeded (rooms={rooms:?}, sockets_held={sockets_held}, deadline_ms={})",
        deadline.as_millis()
    );
    let _ = std::io::stderr().flush();
    tracing::error!(
        ?rooms,
        sockets_held,
        deadline_ms = deadline.as_millis() as u64,
        "server shutdown deadline exceeded"
    );
    Err(Box::new(ShutdownDeadlineExceeded {
        rooms,
        sockets_held,
    }))
}

fn shutdown_status_error(status: ShutdownStatus) -> Box<dyn std::error::Error> {
    let error = ShutdownObservedFailure {
        idle_task_failed: status.idle_task_failed,
        start_task_failures: status.start_task_failures,
        actor_failures: status.actor_failures,
    };
    eprintln!("{error}");
    tracing::error!(
        idle_task_failed = status.idle_task_failed,
        start_task_failures = status.start_task_failures,
        actor_failures = status.actor_failures,
        "server shutdown failed"
    );
    Box::new(error)
}

fn shutdown_panic_error() -> Box<dyn std::error::Error> {
    let error = ShutdownTaskPanicked;
    eprintln!("{error}");
    tracing::error!("server shutdown failed (collaboration task panicked)");
    Box::new(error)
}

#[cfg(test)]
mod shutdown_outcome_tests {
    use super::*;

    #[tokio::test]
    async fn final_task_join_cannot_escape_shutdown_deadline() {
        let (release, pending) = tokio::sync::oneshot::channel();
        let (finished, observed) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let _ = pending.await;
            let _ = finished.send(());
            ShutdownStatus::default()
        });
        let owned = tokio::sync::Mutex::new(Some(HubShutdownTask {
            join: task,
            finished: None,
        }));
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            wait_for_deadline(
                async {
                    DrainOutcome {
                        serve: Ok(()),
                        hub: HubOutcome::Clean,
                        extract: Ok(()),
                        outbox: Ok(()),
                    }
                },
                Some(Instant::now()),
                std::time::Duration::from_millis(20),
                &owned,
                None,
            ),
        )
        .await;
        // Always finish the owned test task, including on the regression path.
        let _ = release.send(());
        tokio::time::timeout(std::time::Duration::from_secs(1), observed)
            .await
            .expect("test task must finish after release")
            .expect("task must not be aborted");
        let error = result
            .expect("final join must obey the inner shutdown deadline")
            .expect_err("pending final join cannot report successful shutdown");
        assert!(error.downcast_ref::<ShutdownDeadlineExceeded>().is_some());
    }

    #[test]
    fn observed_failure_is_not_deadline_or_success() {
        let error = ShutdownObservedFailure {
            idle_task_failed: false,
            start_task_failures: 1,
            actor_failures: 0,
        };
        let text = error.to_string();
        assert!(text.contains("shutdown failed"), "{text}");
        assert!(text.contains("start_task_failures=1"), "{text}");
        assert!(!text.contains("deadline"), "{text}");
    }

    #[test]
    fn join_panic_is_not_deadline_or_success() {
        let text = ShutdownTaskPanicked.to_string();
        assert!(text.contains("shutdown failed"), "{text}");
        assert!(text.contains("panicked"), "{text}");
        assert!(!text.contains("deadline"), "{text}");
    }

    #[test]
    fn deadline_error_mentions_deadline() {
        let text = ShutdownDeadlineExceeded {
            rooms: Some(1),
            sockets_held: 2,
        }
        .to_string();
        assert!(text.contains("deadline exceeded"), "{text}");
        assert!(!text.contains("shutdown failed"), "{text}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unix_stop_signals_install_without_waiting() {
        // Tokio unix signal registration requires a runtime; install must not wait for a signal.
        install_shutdown_signals().expect("SIGTERM and SIGINT must install before listen");
    }

    #[tokio::test]
    async fn readiness_is_announced_only_after_a_pending_first_poll() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::task::Poll;

        let polls = Arc::new(AtomicUsize::new(0));
        let announced_after = Arc::new(AtomicUsize::new(usize::MAX));
        let seen = polls.clone();
        let inner = poll_fn(move |cx| {
            if seen.fetch_add(1, Ordering::SeqCst) == 0 {
                cx.waker().wake_by_ref();
                Poll::Pending
            } else {
                Poll::Ready(())
            }
        });
        let observed = polls.clone();
        let record = announced_after.clone();
        announce_after_first_pending_poll(inner, move || {
            record.store(observed.load(Ordering::SeqCst), Ordering::SeqCst);
        })
        .await;
        assert_eq!(
            announced_after.load(Ordering::SeqCst),
            1,
            "announce must follow the first poll"
        );
        assert_eq!(polls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn readiness_is_not_announced_when_first_poll_completes() {
        let announced = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = announced.clone();
        announce_after_first_pending_poll(std::future::ready(()), move || {
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
        })
        .await;
        assert!(!announced.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn unclean_status_is_failure() {
        let status = ShutdownStatus {
            idle_task_failed: true,
            start_task_failures: 0,
            actor_failures: 0,
        };
        assert!(!status.is_clean());
        match status_outcome(status) {
            HubOutcome::Failed(observed) => assert_eq!(observed, status),
            other => panic!("expected failed outcome, got {other:?}"),
        }
    }
}
