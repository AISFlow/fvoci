//! The upkeep of the search: seeing that the index on the search server is the one
//! the database keeps its list for, and sending what the list says changed.
//!
//! The database remembers which index its list of changes has been kept for
//! (`Store::search_index_mark`): its name, the identifier the search server gave it,
//! and the version of the definition. An index that is not that one lacks an unknown
//! part of what is stored: one that was just made, one made from an older definition,
//! one a server kept no list for (no search server for a while, another database).
//! Such an index is made again, and every board listed to be sent, before anything
//! else is. The command of the operator ([`reindex`]) does the same on purpose.

use std::time::Duration;

use anyhow::Context as _;
use fvoci_store::{SearchIndexMark, Store};
use tokio::time::Instant;

use super::client::{ClientError, IndexFound};
use super::find::index_refused;
use super::worker::{self, Stopped};
use super::{Engine, IndexState, Pause, index};
use crate::app::App;
use crate::error::{AppError, Reason};
use crate::metrics::SearchSent;
use crate::scheduler::RecurringTask;

/// How often the upkeep of the search runs.
const UPKEEP_INTERVAL: Duration = Duration::from_secs(1);
/// How long stopping waits for a run of the upkeep that is under way: a request to a
/// search server that does not answer is not waited for.
const STOP_WAIT: Duration = Duration::from_secs(1);
/// After how many runs in a row with nothing to send the list is counted again, for
/// the metrics.
const COUNT_EVERY_IDLE: u32 = 30;

/// What became of the index when it was looked at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Synced {
    /// It is the one the list has been kept for.
    Kept,
    /// It was made, or made again, and so many boards listed to be sent.
    Made { listed: u64 },
    /// The search server holds something else under the name: the search is off.
    Refused,
}

/// Sees that the index is on the search server, made from this server's definition,
/// and the one the database keeps its list for; makes it, and lists every board,
/// where it is not.
async fn sync_index(store: &Store, engine: &Engine) -> Result<Synced, Stopped> {
    let client = &engine.client;
    let mark = store.search_index_mark().await?;
    match client.describe_index().await? {
        IndexFound::Foreign => {
            tracing::error!(
                "search_index names an index without this server's mark: one that is \
                 not its own, or one the search server made by itself for a document \
                 written after the server's index was removed. It is left as it is, \
                 and bodies are not searched: name another index, or remove it"
            );
            return Ok(Synced::Refused);
        }
        IndexFound::Ours { version, .. } if version > index::VERSION => {
            tracing::error!(
                index = version,
                server = index::VERSION,
                "the search index was made by a newer server; it is left as it is, and \
                 bodies are not searched"
            );
            return Ok(Synced::Refused);
        }
        IndexFound::Ours { version, uuid } => {
            let kept = SearchIndexMark {
                name: client.index().to_owned(),
                uuid,
                version,
            };
            if version == index::VERSION && mark.as_ref() == Some(&kept) {
                return Ok(Synced::Kept);
            }
            tracing::info!(
                from = version,
                to = index::VERSION,
                "the search index is not the one this database has kept its changes \
                 for; making it again"
            );
            client.delete_index().await?;
        }
        IndexFound::Absent => {}
    }
    client.create_index().await?;
    let IndexFound::Ours { version, uuid } = client.describe_index().await? else {
        return Err(Stopped::Search(ClientError::Unreadable));
    };
    let made = SearchIndexMark {
        name: client.index().to_owned(),
        uuid,
        version,
    };
    let listed = store.list_every_container_for_search(&made).await?;
    tracing::info!(
        listed,
        "made the search index; every board is to be sent to it"
    );
    Ok(Synced::Made { listed })
}

/// One run of the upkeep, for a server that holds the lock: the index first, where it
/// is not known to be there, and then what the list says is due. `app` reaches every
/// organization. Returns how many rows of the list it looked at.
async fn run(app: &App, engine: &Engine) -> Result<usize, Stopped> {
    if engine.index_state() == IndexState::Unknown {
        match sync_index(app.store(), engine).await? {
            Synced::Refused => return Err(Stopped::Search(ClientError::NoIndex)),
            // Past every version the index holds, whatever the clock says now. An
            // index just made holds none.
            Synced::Kept => {
                if let Some(highest) = engine.client.highest_version().await? {
                    engine.versions.past(highest);
                }
            }
            Synced::Made { .. } => {}
        }
        engine.set_index_state(IndexState::Ready);
    }
    worker::work_off(app.store(), &engine.client, &engine.versions, app.metrics()).await
}

/// Turns the search off for as long as the server runs: the search server cannot
/// hold the index. Nothing is noted for it any more, and what was is forgotten: the
/// index a later start finds is made again. If that cannot be forgotten, the search
/// is not off yet, and the upkeep comes here again.
async fn refuse(app: &App, engine: &Engine, pause: &mut Pause) {
    match app.store().drop_search_outbox().await {
        Ok(()) => engine.set_index_state(IndexState::Refused),
        Err(error) => {
            tracing::error!(%error, "unable to forget what the search server was to be told");
            pause.wait(Instant::now());
        }
    }
}

/// What a run that stopped makes of the upkeep: a search server that cannot hold the
/// index turns the search off, one that did not take a request is left alone for a
/// while, and an index that is gone is looked for again.
async fn stopped(app: &App, engine: &Engine, pause: &mut Pause, stopped: Stopped) {
    let error = match stopped {
        Stopped::Search(ClientError::NoNori) => {
            tracing::error!(
                "the search server has no nori analysis plugin (analysis-nori): bodies \
                 are not searched until it is installed there and this server started again"
            );
            return refuse(app, engine, pause).await;
        }
        // Said where it was found.
        Stopped::Search(ClientError::NoIndex) => return refuse(app, engine, pause).await,
        Stopped::IndexLost => {
            tracing::warn!(
                "the search index is gone, or is not the one this database kept its \
                 changes for; looking after it again"
            );
            engine.lose_index();
            return;
        }
        Stopped::Search(error) => error,
        Stopped::Store(error) => {
            tracing::error!(%error, "the upkeep of the search could not ask the database");
            pause.wait(Instant::now());
            return;
        }
    };
    app.metrics().search_sent(SearchSent::Unavailable);
    if error.is_missing_index() {
        engine.lose_index();
    }
    // Said once for a row of the same failure: the next ones say the same.
    let refused = matches!(error, ClientError::Refused { .. });
    if pause.failed(Instant::now(), error.clone()) {
        if refused {
            tracing::error!(%error, "the search server refused the upkeep of the search; trying again");
        } else {
            tracing::warn!(%error, "the search server did not take the upkeep of the search; trying again");
        }
    }
}

/// Counts the list for the metrics: after a run that looked at rows or stopped, and
/// now and then otherwise. It is counted while the search server is away too, which
/// is when it grows.
async fn count(app: &App, pause: &mut Pause, looked_at: Option<usize>) {
    pause.idle_runs = match looked_at {
        Some(0) => pause.idle_runs.saturating_add(1),
        Some(_) | None => 0,
    };
    if !pause.idle_runs.is_multiple_of(COUNT_EVERY_IDLE) {
        return;
    }
    match app.store().search_outbox_counts().await {
        Ok(counts) => app
            .metrics()
            .search_outbox_counted(counts.waiting, counts.failing),
        Err(error) => {
            tracing::warn!(%error, "what the search server is to be told was not counted");
        }
    }
}

/// Does the upkeep of the search once, now: what its task does every interval. It
/// sees to the index, and sends what is due of what changed. A search server that
/// does not take a request is left alone for a while, longer each time. The server
/// does it by itself while it serves; this is for its tests.
pub async fn tend(app: &App) {
    let Some(engine) = app.search().engine() else {
        return;
    };
    let mut pause = engine.upkeep.lock().await;
    if !pause.is_over(Instant::now()) {
        return;
    }
    let app = app.housekeeping();
    if let Err(error) = app.require_server_lock() {
        tracing::warn!(%error, "the search is not looked after");
        return;
    }
    match run(&app, engine).await {
        Ok(looked_at) => {
            pause.until = None;
            pause.failures = 0;
            pause.last = None;
            count(&app, &mut pause, Some(looked_at)).await;
        }
        Err(error) => {
            stopped(&app, engine, &mut pause, error).await;
            count(&app, &mut pause, None).await;
        }
    }
}

/// Makes the index again and lists every board to be sent to it, now: the command of
/// the operator, for an index that is thought to be wrong. The boards are sent by the
/// upkeep afterwards, and until it has, searches find what was sent so far. Returns
/// how many boards were listed.
///
/// # Errors
///
/// Returns a 404 with the reason `search_not_configured` on a server without a search
/// server; a 503 with the reason `search_unavailable` where the search server cannot
/// hold the index, or does not take the requests; a 503 if this process does not hold
/// the lock of the server; and the store's error if the database cannot be asked.
pub async fn reindex(app: &App) -> Result<u64, AppError> {
    let Some(engine) = app.search().engine.as_ref() else {
        return Err(AppError::not_found("search").with_reason(Reason::SearchNotConfigured));
    };
    if engine.index_state() == IndexState::Refused {
        return Err(index_refused());
    }
    let mut pause = engine.upkeep.lock().await;
    let app = app.housekeeping();
    app.require_server_lock()?;
    app.store().forget_search_index().await?;
    engine.set_index_state(IndexState::Unknown);
    match sync_index(app.store(), engine).await {
        Ok(Synced::Made { listed }) => {
            engine.set_index_state(IndexState::Ready);
            *pause = Pause::default();
            Ok(listed)
        }
        // The index it was kept for was just forgotten: none is the kept one.
        Ok(Synced::Kept) => Err(AppError::internal("an index kept for no list")),
        Ok(Synced::Refused) => {
            refuse(&app, engine, &mut pause).await;
            Err(index_refused())
        }
        Err(Stopped::Store(error)) => Err(error),
        Err(error) => {
            stopped(&app, engine, &mut pause, error).await;
            Err(
                AppError::service_unavailable("the search server did not take the requests")
                    .with_reason(Reason::SearchUnavailable),
            )
        }
    }
}

/// What a server does about the search before it serves, and before anything is
/// written: one without a search server forgets what an earlier one noted and which
/// index it kept, so that an index found later is made again.
///
/// # Errors
///
/// Returns an error if that cannot be forgotten: what such a server writes is noted
/// nowhere, and an index kept in the database's memory would be taken for whole.
pub(crate) async fn settle_at_start(app: &App) -> anyhow::Result<()> {
    if app.search().configured() {
        return Ok(());
    }
    app.housekeeping()
        .store()
        .drop_search_outbox()
        .await
        .context("unable to forget what a search server was to be told")
}

/// Starts the task of the search's upkeep ([`tend`]); `None` where no search server
/// is configured.
pub(crate) fn start_upkeep(app: &App) -> Option<RecurringTask> {
    if !app.search().configured() || app.config().search.upkeep_by_hand {
        return None;
    }
    let app = app.clone();
    Some(RecurringTask::spawn_starting_now(
        "searchUpkeep",
        UPKEEP_INTERVAL,
        move || {
            let app = app.clone();
            async move { tend(&app).await }
        },
    ))
}

/// Stops the task of the upkeep, waiting a moment for a run that is under way and no
/// longer: what a run has not settled is done again by the next server.
pub(crate) async fn stop_upkeep(task: Option<RecurringTask>) {
    if let Some(task) = task
        && tokio::time::timeout(STOP_WAIT, task.cancel())
            .await
            .is_err()
    {
        tracing::info!("the upkeep of the search was under way when the server stopped");
    }
}
