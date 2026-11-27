//! Editing a card's body together: the one place that holds the Y document of each
//! body being edited, in a worker process ([`worker`]), and stores it.
//!
//! The websocket hands the commands and frames of each of its connections to the
//! connection's [`Seat`], and the rooms of the connections (`rooms`) call:
//!
//! - [`Collab::open`]: the whole document, opened from what is stored, or made from the
//!   card's body the first time (by the server, never by a client).
//! - [`Collab::take`]: takes an editor's update, logs it, and only then gives it back,
//!   with what a repair changed, to be sent to the other editors.
//! - [`Collab::state_vector`], [`Collab::diff`]: what an editor that comes back is
//!   missing.
//! - [`Collab::write_body`], [`Collab::close`]: writes the document's body to the
//!   card now; closes the document after that.
//! - [`Collab::subscribe`]: what happens to documents that editors have to know of
//!   ([`DocChange`]).
//!
//! An update is taken by the worker into a copy of the document, logged in
//! `body_updates`, and then given back: nothing is sent to anybody that the log does
//! not have. The body of the document is written to the card (`blocks.body`, which
//! everything else reads) two seconds after the last update, ten seconds after the
//! first one that is not written at the latest, and when the document is closed or the
//! server stops. Versions of the body are kept as `versions` says. The log is folded
//! into the stored state every [`FOLD_AFTER`] updates, once the body is written to its
//! card (the log says who typed what), and when the document closes; a
//! document whose state comes near its limit is made again from its body. A server that
//! ended before it wrote a body or folded a log does so when it starts again
//! ([`Collab::recover`]).

mod admission;
mod append;
mod caret;
#[cfg(unix)]
mod child;
mod frame;
mod messages;
mod open;
mod place;
mod queue;
mod refusals;
mod removal;
mod rooms;
mod seat;
mod senders;
mod upkeep;
mod versions;
mod whole;
pub mod worker;

pub use append::Appender;
pub use queue::Outgoing;
pub(crate) use removal::Deletion;
#[cfg(feature = "test-util")]
pub use removal::DeletionStage;
pub(crate) use rooms::{AccessChange, AccessScope, Wall};
pub use seat::{Outbox, Seat};
pub use whole::{OverEditors, cards_written_whole};

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fvoci_store::Tenant;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::app::App;
use crate::collab_worker::wire::{ASIDE_DOCUMENTS, Answer, Limits, Refusal, Refused, Request};
use crate::config::collab::CollabSettings;
use crate::error::AppError;
use place::{Held, Slot, Unanswered};
use worker::{Worker, WorkerCommand, WorkerSettings};

/// How many updates the log of a document holds before they are folded into its state.
pub const FOLD_AFTER: i64 = 200;

/// What a server that ended left of a document ([`Collab::recover`]).
#[derive(Debug, Clone, Copy)]
enum Leftover {
    /// A body it did not write, or a version it did not keep.
    Unfinished,
    /// Written and kept, and a log it did not fold.
    Unfolded,
}

/// How long a server that stops waits to write the bodies of the open documents.
const END_WAIT: Duration = Duration::from_secs(5);

/// How long after a document could not be made again from its body it is tried again.
const REMAKE_RETRY: Duration = Duration::from_mins(1);

/// How long after the last update the body of a document is written to its card.
pub const WRITE_QUIET: Duration = Duration::from_secs(2);

/// How long after the first update that is not written the body is written at the
/// latest, whatever comes after it.
pub const WRITE_LONGEST: Duration = Duration::from_secs(10);

/// How many events a subscriber may fall behind before it misses some.
const EVENTS: usize = 256;

/// Runs the executable as the worker ([`crate::collab_worker`]) if its first argument
/// says it was started as one: `None` if it was not, else how the worker ended. The
/// executable does nothing else then: no configuration, runtime or log.
#[must_use]
pub fn run_as_worker() -> Option<anyhow::Result<()>> {
    crate::collab_worker::run_if_started_as_worker()
        .map(|ended| ended.map_err(|error| anyhow::anyhow!("the collab worker ended: {error}")))
}

/// The place of the documents of the bodies being edited. Cloning is cheap; every clone
/// is the same place.
#[derive(Debug, Clone)]
pub struct Collab {
    inner: Arc<Inner>,
}

type ContainerLocks = HashMap<(Tenant, String), Arc<tokio::sync::RwLock<()>>>;
type CardSlots = HashMap<String, (String, Arc<tokio::sync::Mutex<Slot>>)>;

#[derive(Debug)]
struct Inner {
    workers: Vec<Worker>,
    /// Per `(organization, container_id)`: a deletion excludes document work and changes
    /// to that container's card tree until it is done ([`removal`]).
    gates: Mutex<ContainerLocks>,
    /// Per `(organization, container_id)`: serializes `changing_structure` (read) with
    /// the `Deletion::Block` fast-path check (write) without blocking document work.
    structures: Mutex<ContainerLocks>,
    /// Home worker index assigned to each organization (`H-3`).
    tenant_slots: Mutex<HashMap<Tenant, usize>>,
    next_tenant_slot: AtomicUsize,
    /// How many documents each worker was given, of those that are open, but those
    /// opened aside.
    loads: Vec<AtomicUsize>,
    /// The share of documents opened aside, only to write their log to their card
    /// ([`Share::Aside`](crate::collab_worker::wire::Share::Aside)): of the whole server, so that a worker never holds more of
    /// them than it takes, whichever they are opened in. Who finds none free waits.
    aside: Arc<tokio::sync::Semaphore>,
    /// The documents that are open, or were and are closed for a while, by card: each
    /// entry stores `(container_id, slot)`.
    slots: Mutex<CardSlots>,
    /// The next number a document is opened under in a worker.
    numbers: AtomicU64,
    limits: Limits,
    /// How long after the first update no version of the body holds the next version
    /// is due (`collab_version_minutes`).
    version_interval: Duration,
    /// How much later than it is the versions take it to be: for the tests of what is
    /// kept as time passes.
    #[cfg(feature = "test-util")]
    version_time_passed: Mutex<Duration>,
    events: broadcast::Sender<DocEvent>,
    /// The connections that hold the documents together.
    rooms: rooms::Rooms,
    /// Who sent updates that ended a worker, and is held back for it.
    senders: senders::Senders,
    /// The cards whose next updates find the worker's process stopped for so many
    /// milliseconds ([`worker::Worker::stop_at`]), by their key in the place: for the
    /// tests of an update that ends its worker.
    #[cfg(feature = "test-util")]
    stalled_updates: Mutex<HashMap<String, u64>>,
    /// No body is written because it is due ([`Collab::leave_due_writes`]).
    #[cfg(feature = "test-util")]
    due_writes_left: std::sync::atomic::AtomicBool,
    /// What the next write of a body waits for, between reading its card and writing it:
    /// for the tests of what is written to the card meanwhile.
    #[cfg(feature = "test-util")]
    projection_pause: Mutex<Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>>,
    /// Where the next deletion waits, for the tests of what happens meanwhile.
    #[cfg(feature = "test-util")]
    deletion_pause: Mutex<Option<removal::DeletionPause>>,
    /// What the next update waits for, between the worker taking it and the log
    /// having it: for the tests of what is sent on meanwhile.
    #[cfg(feature = "test-util")]
    log_pause: Mutex<Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>>,
}

/// A document as an editor loads it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Loaded {
    /// The whole document, as a version 1 update.
    pub state: Vec<u8>,
    /// Which making of the document it is: what the editor sends with its updates and
    /// its state vectors. One more each time the document is made again.
    pub epoch: i64,
}

/// An update a document took.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Taken {
    /// Its number in the document's log.
    pub seq: i64,
    /// What a repair changed so that the document stays a body, as an update for every
    /// editor, the one that sent the update too; logged after it.
    pub repair: Option<Vec<u8>>,
    /// Whether the document was made again from its body after it (its state came near
    /// its limit): every editor loads it anew ([`DocChange::Remade`] says so as well),
    /// and the update and its repair, made against the document before, are not sent
    /// to anyone.
    pub remade: bool,
}

/// Something that happened to a document that the editors who hold it have to know.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct DocEvent {
    /// The organization of the card: the same identifier in another organization is
    /// another card.
    pub tenant: Tenant,
    /// The card whose document it is, in that organization.
    pub card_id: String,
    /// What happened.
    pub change: DocChange,
}

/// What happened to a document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DocChange {
    /// It was made again from its body: an update made against the one before is
    /// refused (reason `update_out_of_step`). Editors load it anew.
    Remade,
    /// Its body was written whole over HTTP, and the document made again from it.
    /// Editors load it anew.
    Replaced,
    /// The worker that held it ended, or another process wrote what is stored of it
    /// (a server that took the lock of the server, or the one that lost it and has
    /// not noticed yet): it is opened again from what is stored. What an editor sent
    /// and was refused for that is not in it. Editors load it anew.
    Reopened,
    /// Its card's body was written whole while it was not open (its worker had ended
    /// over it), or it was closed after such a write because it could not be made
    /// again from that body: the next request opens it from the card. Editors load
    /// it anew.
    Closed,
}

impl Collab {
    /// The place, with workers started as `settings` say when they are first needed.
    ///
    /// # Errors
    ///
    /// Returns why the executable to start the workers from is not known.
    pub fn new(settings: &CollabSettings) -> anyhow::Result<Self> {
        let command = match &settings.worker_command {
            Some(command) => command.clone(),
            None => WorkerCommand::this_executable()?,
        };
        let limits = Limits {
            update_bytes: settings.collab_update_bytes,
            state_bytes: settings.collab_state_bytes,
            body_bytes: u64::try_from(fvoci_model::BODY_MAX_BYTES).unwrap_or(u64::MAX),
            documents: u64::from(settings.collab_documents_per_worker),
            held_bytes: settings.held_bytes(),
        };
        let worker_settings = WorkerSettings {
            command,
            limits,
            timeout: Duration::from_millis(settings.collab_worker_timeout_ms),
            memory_bytes: settings.collab_worker_memory_bytes,
        };
        let count = usize::try_from(settings.collab_workers).unwrap_or(1).max(1);
        Ok(Self {
            inner: Arc::new(Inner {
                gates: Mutex::new(HashMap::new()),
                structures: Mutex::new(HashMap::new()),
                tenant_slots: Mutex::new(HashMap::new()),
                next_tenant_slot: AtomicUsize::new(0),
                workers: (0..count)
                    .map(|_| Worker::new(worker_settings.clone()))
                    .collect(),
                loads: (0..count).map(|_| AtomicUsize::new(0)).collect(),
                aside: Arc::new(tokio::sync::Semaphore::new(ASIDE_DOCUMENTS)),
                slots: Mutex::new(HashMap::new()),
                numbers: AtomicU64::new(1),
                limits,
                version_interval: settings.version_interval(),
                #[cfg(feature = "test-util")]
                version_time_passed: Mutex::new(Duration::ZERO),
                events: broadcast::channel(EVENTS).0,
                rooms: rooms::Rooms::new(settings),
                senders: senders::Senders::default(),
                #[cfg(feature = "test-util")]
                stalled_updates: Mutex::new(HashMap::new()),
                #[cfg(feature = "test-util")]
                due_writes_left: std::sync::atomic::AtomicBool::new(false),
                #[cfg(feature = "test-util")]
                projection_pause: Mutex::new(None),
                #[cfg(feature = "test-util")]
                deletion_pause: Mutex::new(None),
                #[cfg(feature = "test-util")]
                log_pause: Mutex::new(None),
            }),
        })
    }

    /// What happens to documents from now on.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<DocEvent> {
        self.inner.events.subscribe()
    }

    /// Runs `work` on a card's slot, held, in a task of its own: what a caller that
    /// stops waiting leaves is a step done whole, never half of one (an update taken
    /// and not logged, a document made and not counted). Shutdown waits for it.
    async fn on_slot<T, W, F>(&self, app: &App, card_id: &str, work: W) -> Result<T, AppError>
    where
        T: Send + 'static,
        W: FnOnce(Self, App, String, Held) -> F + Send + 'static,
        F: Future<Output = Result<T, AppError>> + Send + 'static,
    {
        self.on_slot_while(app, card_id, None, work).await
    }

    /// A detached room cannot open, read or write a restored card's document.
    async fn on_slot_while<T, W, F>(
        &self,
        app: &App,
        card_id: &str,
        gone: Option<CancellationToken>,
        work: W,
    ) -> Result<T, AppError>
    where
        T: Send + 'static,
        W: FnOnce(Self, App, String, Held) -> F + Send + 'static,
        F: Future<Output = Result<T, AppError>> + Send + 'static,
    {
        let (collab, app_of_task, card_id) = (self.clone(), app.clone(), card_id.to_owned());
        let task = app.spawn_tracked(async move {
            let container_id = collab
                .slot_container_id(&app_of_task, &card_id)
                .await?
                .ok_or_else(|| AppError::not_found("card"))?;
            let _gate = collab
                .reading(app_of_task.store().tenant(), &container_id)
                .await;
            if gone.as_ref().is_some_and(CancellationToken::is_cancelled) {
                return Err(AppError::not_found("card"));
            }
            let slot = collab.slot_in(&app_of_task, &card_id, &container_id).await;
            work(collab, app_of_task, card_id, slot).await
        });
        task.await.map_err(|error| {
            AppError::internal(format_args!("the work on a document ended: {error}"))
        })?
    }

    /// The whole document of a card's body, as a version 1 update, and which making of
    /// it it is; opened if it is not open: from what is stored, or made from the card's
    /// body the first time. An editor sends the epoch with what it asks after
    /// ([`Collab::take`], [`Collab::diff`]): a document made again since
    /// ([`DocChange::Remade`], [`DocChange::Replaced`]) refuses it as out of step.
    ///
    /// # Errors
    ///
    /// Returns a not-found error if `app` reaches no card of that identifier, the
    /// reasons of [`crate::error::Reason`] for the document (too many open,
    /// unavailable), and the store's errors.
    pub async fn open(&self, app: &App, card_id: &str) -> Result<Loaded, AppError> {
        self.open_while(app, card_id, None).await
    }

    async fn open_while(
        &self,
        app: &App,
        card_id: &str,
        gone: Option<CancellationToken>,
    ) -> Result<Loaded, AppError> {
        self.on_slot_while(
            app,
            card_id,
            gone,
            move |collab, app, card_id, mut slot| async move {
                let (collab, app, card_id, slot) = (&collab, &app, card_id.as_str(), &mut slot);
                let state = match collab.ask(app, card_id, slot, Request::State).await? {
                    Answer::Bytes(state) => state,
                    Answer::Remake => {
                        if let Some(open) = slot.open.as_mut() {
                            open.must_remake = true;
                        }
                        if !collab
                            .remake_and_tell(app, card_id, slot, upkeep::OnForget::Quiet)
                            .await
                        {
                            return Err(refusals::unavailable());
                        }
                        bytes(collab.ask(app, card_id, slot, Request::State).await?)?
                    }
                    other => return Err(open::unexpected(&other)),
                };
                let epoch = slot
                    .open
                    .as_ref()
                    .map(|open| open.epoch)
                    .ok_or_else(refusals::unavailable)?;
                Ok(Loaded { state, epoch })
            },
        )
        .await
    }

    /// The state vector of a card's document, version 1, opened if it is not open.
    ///
    /// # Errors
    ///
    /// As [`Collab::open`].
    pub async fn state_vector(&self, app: &App, card_id: &str) -> Result<Vec<u8>, AppError> {
        self.state_vector_while(app, card_id, None).await
    }

    async fn state_vector_while(
        &self,
        app: &App,
        card_id: &str,
        gone: Option<CancellationToken>,
    ) -> Result<Vec<u8>, AppError> {
        self.on_slot_while(
            app,
            card_id,
            gone,
            move |collab, app, card_id, mut slot| async move {
                let (collab, app, card_id, slot) = (&collab, &app, card_id.as_str(), &mut slot);
                bytes(collab.ask(app, card_id, slot, Request::StateVector).await?)
            },
        )
        .await
    }

    /// What a card's document, of `epoch`, has that a holder of `state_vector` has not,
    /// as a version 1 update.
    ///
    /// # Errors
    ///
    /// As [`Collab::open`], `update_out_of_step` if the document is not of `epoch` any
    /// more, and a refusal (`update_refused`) for bytes that are no state vector.
    pub async fn diff(
        &self,
        app: &App,
        card_id: &str,
        epoch: i64,
        state_vector: &[u8],
    ) -> Result<Vec<u8>, AppError> {
        self.diff_while(app, card_id, epoch, state_vector, None)
            .await
    }

    async fn diff_while(
        &self,
        app: &App,
        card_id: &str,
        epoch: i64,
        state_vector: &[u8],
        gone: Option<CancellationToken>,
    ) -> Result<Vec<u8>, AppError> {
        self.held_to_update_bytes(state_vector)?;
        let state_vector = state_vector.to_vec();
        self.on_slot_while(
            app,
            card_id,
            gone,
            move |collab, app, card_id, mut slot| async move {
                let (collab, app, card_id, slot) = (&collab, &app, card_id.as_str(), &mut slot);
                let request = |doc| Request::Diff {
                    doc,
                    state_vector: state_vector.clone(),
                };
                collab.at_epoch(app, card_id, slot, epoch).await?;
                let diff = bytes(collab.ask(app, card_id, slot, request).await?)?;
                place::still_at_epoch(slot, epoch)?;
                Ok(diff)
            },
        )
        .await
    }

    /// Takes an editor's update, made against the document of `epoch`, into a card's
    /// document, by `user_id`, and logs it; what comes back may be sent to the other
    /// editors, the update first and then the repair. Opens the document if it is not
    /// open.
    ///
    /// # Errors
    ///
    /// As [`Collab::open`], and the refusals of an update (`update_refused`,
    /// `update_out_of_step`, also for a document that is not of `epoch` any more,
    /// `update_too_large`, `document_too_large`, `table_too_large`). A refused update is
    /// in neither the document nor the log. So is one under way when the worker ended
    /// (`document_unavailable`).
    pub async fn take(
        &self,
        app: &App,
        card_id: &str,
        epoch: i64,
        user_id: &str,
        update: &[u8],
    ) -> Result<Taken, AppError> {
        self.take_while(app, card_id, epoch, user_id, update, None)
            .await
            .map_err(Unanswered::into_error)
    }

    /// A room that was deleted cannot write later, even if its card was restored. The
    /// updates of a user whose update ended a worker are refused for a while
    /// ([`senders`]) before anything else, and again in their turn before they are sent.
    async fn take_while(
        &self,
        app: &App,
        card_id: &str,
        epoch: i64,
        user_id: &str,
        update: &[u8],
        gone: Option<CancellationToken>,
    ) -> Result<Taken, Unanswered> {
        self.held_back(app, user_id)?;
        self.held_to_update_bytes(update)?;
        let (user_id, update) = (user_id.to_owned(), update.to_vec());
        self.on_slot_while(
            app,
            card_id,
            gone,
            move |collab, app, card_id, mut slot| async move {
                let (collab, app, card_id, slot) = (&collab, &app, card_id.as_str(), &mut slot);
                Ok(collab
                    .take_in(app, card_id, slot, epoch, &user_id, &update)
                    .await)
            },
        )
        .await?
    }

    /// Has the worker's process stopped for `millis` milliseconds as it is sent each
    /// next update of a card of `app`'s organization, until [`Collab::take_updates`]:
    /// for the tests of an update that ends its worker.
    #[cfg(feature = "test-util")]
    #[doc(hidden)]
    pub fn stall_updates(&self, app: &App, card_id: &str, millis: u64) {
        let key = place::key_of(app.store().tenant(), card_id);
        let mut stalled = self.inner.stalled_updates.lock().expect("not poisoned");
        stalled.insert(key, millis);
    }

    /// Has the worker take the updates of a card again ([`Collab::stall_updates`]).
    #[cfg(feature = "test-util")]
    #[doc(hidden)]
    pub fn take_updates(&self, app: &App, card_id: &str) {
        let key = place::key_of(app.store().tenant(), card_id);
        let mut stalled = self.inner.stalled_updates.lock().expect("not poisoned");
        stalled.remove(&key);
    }

    /// How long the worker's process is to be stopped as it is sent an update of a card
    /// ([`Collab::stall_updates`]).
    #[cfg(feature = "test-util")]
    fn stall_for(&self, app: &App, card_id: &str) -> Option<u64> {
        let key = place::key_of(app.store().tenant(), card_id);
        let stalled = self.inner.stalled_updates.lock().expect("not poisoned");
        stalled.get(&key).copied()
    }

    /// Writes the body of a card's document to the card now, if it has updates whose
    /// body the card does not hold, including the log of a document no longer open.
    ///
    /// # Errors
    ///
    /// Returns the errors of reading the body from the worker and of writing it.
    pub async fn write_body(&self, app: &App, card_id: &str) -> Result<(), AppError> {
        self.on_slot(
            app,
            card_id,
            move |collab, app, card_id, mut slot| async move {
                let (collab, app, card_id, slot) = (&collab, &app, card_id.as_str(), &mut slot);
                let written = collab
                    .write_pending(app, card_id, slot, whole::DueVersion::Keep)
                    .await;
                collab.retire(slot);
                written
            },
        )
        .await
    }

    /// Writes the body of a card's document to the card, keeps it as a version if the
    /// document took updates no version holds, folds its log into its state (or makes
    /// the document again from the body, if many sessions wrote it), and closes the
    /// document. The next request opens it again from what is stored. A
    /// body that cannot be written now is still in the stored state (the card is
    /// marked as not holding it) and is written when the document is opened again, or
    /// when the server starts: the document is closed all the same.
    ///
    /// # Errors
    ///
    /// Returns why the body could not be written.
    pub async fn close(&self, app: &App, card_id: &str) -> Result<(), AppError> {
        self.on_slot(
            app,
            card_id,
            move |collab, app, card_id, mut slot| async move {
                let (collab, app, card_id, slot) = (&collab, &app, card_id.as_str(), &mut slot);
                let written = collab.close_held(app, card_id, slot).await;
                collab.retire(slot);
                written
            },
        )
        .await
    }

    /// Lets nobody into a room any more, writes the body of every open document, keeps
    /// it as a version if its editors changed it since the newest, folds its log,
    /// closes the documents, and ends the workers: for a server that stops. `app` is
    /// any handle of the server: each document is written as its card's organization.
    /// The bodies come first and the folds after them: what is not done within five
    /// seconds (a write of a whole body that holds a card, a database that does not
    /// answer) is left, a body to the next start, a log to the next start or opening.
    pub async fn end(&self, app: &App) {
        self.end_rooms();
        let deadline = tokio::time::Instant::now() + END_WAIT;
        let slots = self.all_slots();
        for slot in &slots {
            let written = self.on_ending(app, slot, deadline, async |collab, app, card_id, slot| {
                match collab.write_now(app, card_id, slot).await {
                    Ok(()) => collab.version_as_left(app, card_id, slot).await,
                    Err(error) => {
                        tracing::error!(%error, card_id, "cannot write a card's body as the server stops");
                    }
                }
            });
            if written.await.is_err() {
                tracing::warn!("the bodies of documents are left for the next start to write");
                break;
            }
        }
        for slot in &slots {
            let folded = self.on_ending(app, slot, deadline, async |collab, app, card_id, slot| {
                collab.fold_as_it_closes(app, card_id, slot).await;
            });
            if folded.await.is_err() {
                tracing::warn!("the logs of documents are left for the next start to fold");
                break;
            }
        }
        for slot in &slots {
            let closed = tokio::time::timeout_at(deadline, async {
                // In the order of the gate and then the slot, as everything else.
                let (tenant, container_id) = {
                    let guard = slot.lock().await;
                    (guard.tenant, guard.container_id.clone())
                };
                let _gate = self.reading(tenant, &container_id).await;
                let mut slot = Arc::clone(slot).lock_owned().await;
                self.forget(&mut slot).await;
                self.retire(&mut slot);
            });
            if closed.await.is_err() {
                break;
            }
        }
        // A worker on a request past the wait is ended under it: what it did not write
        // is in the log.
        for worker in &self.inner.workers {
            if tokio::time::timeout_at(deadline, worker.end())
                .await
                .is_err()
            {
                worker.end_now().await;
            }
        }
    }

    /// Writes the bodies of the documents that a server that ended did not write, keeps
    /// as versions those it did not keep, and folds the logs it did not fold, each as
    /// its organization: for a server that starts. `app` reaches every organization.
    /// What cannot be done is logged and left for when the document is opened.
    pub async fn recover(&self, app: &App) {
        // Writes to cards and to what is stored of documents: only the server of the
        // database does.
        if let Err(error) = app.require_server_lock() {
            tracing::warn!(%error, "the bodies a run that ended did not write are not written");
            return;
        }
        let unfinished = match app.store().unfinished_body_docs().await {
            Ok(unfinished) => unfinished,
            Err(error) => {
                tracing::error!(%error, "cannot read which bodies are not written or not kept");
                return;
            }
        };
        self.see_to(app, unfinished, Leftover::Unfinished).await;
        // Read after those are seen to, which folded theirs: those left were written
        // and kept, and their logs alone are left.
        match app.store().unfolded_body_docs().await {
            Ok(unfolded) => self.see_to(app, unfolded, Leftover::Unfolded).await,
            Err(error) => tracing::error!(%error, "cannot read which logs are not folded"),
        }
    }

    /// Opens each of `docs` (cards with their organizations), writes its body, keeps a
    /// version if one is due, folds its log and closes it, while this process holds
    /// the lock of the server.
    async fn see_to(&self, app: &App, docs: Vec<(String, String)>, left: Leftover) {
        for (card_id, org_id) in docs {
            if let Err(error) = app.require_server_lock() {
                tracing::warn!(%error, "what a run that ended left is left for the next start");
                return;
            }
            let tenant = match Tenant::org(&org_id) {
                Ok(tenant) => tenant,
                Err(error) => {
                    tracing::error!(%error, card_id, "the organization of a document is no identifier");
                    continue;
                }
            };
            if let Err(error) = self.close_after_open(&app.scoped(tenant), &card_id).await {
                tracing::error!(%error, card_id, "cannot open, write or fold a document a server that ended had left");
                continue;
            }
            match left {
                Leftover::Unfinished => {
                    tracing::info!(card_id, "saw to a body a server that ended had left");
                }
                Leftover::Unfolded => {
                    tracing::debug!(card_id, "saw to a log a server that ended had left");
                }
            }
        }
    }

    /// Opens a card's document aside, writes its body, which keeps it as a version if
    /// one is due (a document opened with what no version holds is), folds its log,
    /// and closes it. A document a room holds open meanwhile (somebody came back while
    /// the start saw to the others) is written and folded, and stays open.
    async fn close_after_open(&self, app: &App, card_id: &str) -> Result<(), AppError> {
        self.on_slot(
            app,
            card_id,
            move |collab, app, card_id, mut slot| async move {
                let (collab, app, card_id, slot) = (&collab, &app, card_id.as_str(), &mut slot);
                let held_by_a_room = slot.open.is_some();
                if !held_by_a_room {
                    collab.open_aside(app, card_id, slot).await?;
                }
                let mut written = collab.write_now(app, card_id, slot).await;
                if written.is_ok()
                    && slot.open.as_ref().is_some_and(|open| open.must_remake)
                    && !collab
                        .remake_and_tell(app, card_id, slot, upkeep::OnForget::Quiet)
                        .await
                {
                    written = Err(refusals::unavailable());
                }
                collab.fold_as_it_closes(app, card_id, slot).await;
                if !held_by_a_room {
                    collab.forget(slot).await;
                    collab.retire(slot);
                }
                written
            },
        )
        .await
    }

    /// Does `work` on the open document of `slot`, as its card's organization, under
    /// the document gate, by `deadline`: for a server that stops. A slot without an
    /// open document is passed over.
    async fn on_ending(
        &self,
        app: &App,
        slot: &Arc<tokio::sync::Mutex<Slot>>,
        deadline: tokio::time::Instant,
        work: impl AsyncFnOnce(&Self, &App, &str, &mut Held),
    ) -> Result<(), tokio::time::error::Elapsed> {
        tokio::time::timeout_at(deadline, async {
            // The slot is let go before the gate is waited for: a deletion holds the
            // gate while it waits for slots.
            let (tenant, container_id) = {
                let guard = slot.lock().await;
                (guard.tenant, guard.container_id.clone())
            };
            let _gate = self.reading(tenant, &container_id).await;
            let mut slot = Arc::clone(slot).lock_owned().await;
            let app = app.scoped(slot.tenant);
            if let Some(card_id) = slot.open.is_some().then(|| slot.card_id.clone()) {
                work(self, &app, &card_id, &mut slot).await;
            }
        })
        .await
    }

    /// The process identifiers of the running workers, for the tests of their deaths.
    #[cfg(feature = "test-util")]
    #[doc(hidden)]
    pub async fn worker_pids(&self) -> Vec<Option<u32>> {
        let mut pids = Vec::new();
        for worker in &self.inner.workers {
            pids.push(worker.pid().await);
        }
        pids
    }

    /// The process identifier of the worker currently holding `card_id`'s open document,
    /// if any, for multi-worker isolation tests.
    #[cfg(feature = "test-util")]
    #[doc(hidden)]
    pub async fn document_worker_pid(&self, app: &App, card_id: &str) -> Option<u32> {
        let slot = self.existing_slot(app, card_id).await?;
        let worker = slot.open.as_ref()?.worker;
        drop(slot);
        self.inner.workers[worker].pid().await
    }

    /// How many of the first worker's processes ended, and its deaths in a row: for the
    /// tests of whom a death is put down to.
    #[cfg(feature = "test-util")]
    #[doc(hidden)]
    pub async fn first_worker_ended(&self) -> worker::Ended {
        self.inner.workers[0]
            .ended()
            .await
            .expect("the worker's task runs")
    }

    /// Has the first worker take `millis` milliseconds over a request, for the tests
    /// of a worker that does not answer in time.
    ///
    /// # Errors
    ///
    /// Returns how the request ended.
    #[cfg(feature = "test-util")]
    #[doc(hidden)]
    pub async fn stall_worker(&self, millis: u64) -> Result<(), worker::CallError> {
        self.inner.workers[0]
            .stall(Duration::from_millis(millis), worker::Blame::Worker)
            .await
            .map(|_| ())
    }

    /// Has the process of whichever worker is sent the next request about the document
    /// `doc` stopped for `millis` milliseconds first ([`worker::Worker::stop_at`]).
    #[cfg(feature = "test-util")]
    fn stop_at_document(&self, doc: crate::collab_worker::wire::DocNumber, millis: u64) {
        for worker in &self.inner.workers {
            worker.stop_at(worker::StopAt::Document(doc), Duration::from_millis(millis));
        }
    }

    /// Has the worker that holds a card's document take `millis` milliseconds over a
    /// request about it, opening the document first: its process is stopped as it is
    /// sent a request for the document's body. For the tests of a document whose
    /// requests end its worker.
    ///
    /// # Errors
    ///
    /// As a request about the document.
    #[cfg(feature = "test-util")]
    #[doc(hidden)]
    pub async fn stall_document(
        &self,
        app: &App,
        card_id: &str,
        millis: u64,
    ) -> Result<(), AppError> {
        self.on_slot(
            app,
            card_id,
            move |collab, app, card_id, mut slot| async move {
                let (collab, app, card_id, slot) = (&collab, &app, card_id.as_str(), &mut slot);
                collab
                    .ask(app, card_id, slot, |doc| {
                        collab.stop_at_document(doc, millis);
                        Request::Body(doc)
                    })
                    .await
                    .map(|_| ())
            },
        )
        .await
    }

    /// Refuses the updates of a user of `app`'s organization while they, or their
    /// organization, are held back for updates that ended a worker ([`senders`]).
    fn held_back(&self, app: &App, user_id: &str) -> Result<(), AppError> {
        let now = tokio::time::Instant::now();
        let tenant = app.store().tenant();
        if self
            .inner
            .senders
            .held_until(tenant, user_id, now)
            .is_some()
        {
            return Err(refusals::held_back());
        }
        Ok(())
    }

    /// Refuses what an editor sends that is larger than an update may be, before it is
    /// copied into a frame for the worker (which refuses it as well).
    fn held_to_update_bytes(&self, sent: &[u8]) -> Result<(), AppError> {
        let limit = usize::try_from(self.inner.limits.update_bytes).unwrap_or(usize::MAX);
        if sent.len() <= limit {
            return Ok(());
        }
        let refused = Refused {
            refusal: Refusal::UpdateTooLarge,
            message: format!("{} bytes, before the worker", sent.len()),
            rolled_back: false,
        };
        Err(refusals::refused(&refused, &self.inner.limits))
    }

    /// Has no body written to its card because it is due, from here on: a document is
    /// written when it is asked to be, and when it is closed. For the tests of a server
    /// that is killed with updates its cards do not hold, however long the machine
    /// took to take them.
    #[cfg(feature = "test-util")]
    #[doc(hidden)]
    pub fn leave_due_writes(&self) {
        self.inner
            .due_writes_left
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

#[cfg(feature = "test-util")]
mod test_pauses;

/// The bytes of an answer that carries them.
fn bytes(answer: Answer) -> Result<Vec<u8>, AppError> {
    match answer {
        Answer::Bytes(bytes) => Ok(bytes),
        other => Err(AppError::internal(format_args!(
            "the worker answered with no bytes: {other:?}"
        ))),
    }
}

/// Counts a document given to a worker.
fn given(load: &AtomicUsize) {
    load.fetch_add(1, Ordering::Relaxed);
}

/// Counts a document taken from a worker.
fn taken_from(load: &AtomicUsize) {
    let _ = load.try_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
        count.checked_sub(1)
    });
}

/// Another organization's wall than [`Wall::for_tests`]: an organization is named here
/// rather than beside the walls, among the few places that name one for a handle.
#[cfg(test)]
pub(crate) fn another_wall_for_tests() -> Wall {
    Wall::of_tenant(Tenant::org("01930000-0000-7000-8000-0000000000aa").expect("an identifier"))
}
