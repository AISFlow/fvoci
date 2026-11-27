//! The place of each card's document: one at a time works on it, and what is known of
//! it while it is open.

use std::sync::Arc;
use std::time::Duration;

use fvoci_store::{DocAsRead, StoreError, Tenant};
use tokio::sync::{Mutex, OwnedMutexGuard, OwnedSemaphorePermit};
use tokio::time::Instant;

use super::senders::SenderOf;
use super::versions::Versioned;
use super::worker::{Blame, CallError};
use super::{Collab, DocChange, DocEvent, refusals, taken_from};
use crate::app::App;
use crate::collab_worker::wire::{Answer, DocNumber, Refusal, Request, Share};
use crate::error::AppError;

/// How long a document that ended its worker twice, the second within
/// [`DEATHS_REMEMBERED`] of the first, is closed the first time; it doubles with each
/// further time.
const FIRST_CLOSING: Duration = Duration::from_secs(10);

/// The longest a document is closed for ending its worker.
const LONGEST_CLOSING: Duration = Duration::from_mins(5);

/// How long a death of the worker over a document is remembered: the deaths within it
/// of the one before are counted together. Longer than the longest closing, so that a
/// document closed that long is closed again at its next death.
const DEATHS_REMEMBERED: Duration = Duration::from_mins(15);

/// A slot, held by the one request that works on its document.
pub(super) type Held = OwnedMutexGuard<Slot>;

/// What is known of a card's document. A slot is an organization's card's: the same
/// identifier in another organization is another slot.
#[derive(Debug)]
pub(super) struct Slot {
    /// Its key in the place.
    key: String,
    /// The card whose document the slot holds, in its organization.
    pub(super) card_id: String,
    /// The container owning the card.
    pub(super) container_id: String,
    /// The organization of the card.
    pub(super) tenant: Tenant,
    /// The slot is no longer in the place: whoever waited for it looks again.
    retired: bool,
    pub(super) open: Option<OpenDoc>,
    /// How many requests about it ended its worker, each a little after the one before
    /// (within [`DEATHS_REMEMBERED`]): a request that ends the worker each time it is
    /// sent again, between which others are answered, is counted, and so is an opening
    /// that ends it. The slot stays in the place while it remembers a death.
    deaths: u32,
    /// When the last of them ended it.
    last_death: Option<Instant>,
    /// Until when it is closed for that.
    closed_until: Option<Instant>,
    /// When it could not be made again from its body last.
    pub(super) remake_failed_at: Option<Instant>,
}

/// A document that a worker holds.
#[derive(Debug)]
pub(super) struct OpenDoc {
    /// Which worker, of which generation, under which number.
    pub(super) worker: usize,
    pub(super) generation: u64,
    pub(super) number: DocNumber,
    /// Which making of the document it is ([`super::Loaded::epoch`]).
    pub(super) epoch: i64,
    /// The last update its stored state holds.
    pub(super) state_seq: i64,
    /// The last update logged.
    pub(super) log_end: i64,
    /// The last update whose body the card holds.
    pub(super) written_seq: i64,
    /// The digest of the body the card holds, as of `written_seq`.
    pub(super) body_digest: Vec<u8>,
    /// The size of its state.
    pub(super) state_bytes: u64,
    /// The size of its state when it was made from its body; 0 if it was not here.
    pub(super) made_bytes: u64,
    /// How many clients it names.
    pub(super) clients: u64,
    /// How many changes it holds across all clients, deleted ones too.
    pub(super) changes: u64,
    /// How many changes it held when it was made from its body; 0 if it was not here.
    pub(super) made_changes: u64,
    /// The most changes one of its clients has, deleted ones too.
    pub(super) client_changes: u64,
    /// The most steps the Y library may take to read it from its state once one more
    /// client has written into it ([`fvoci_collab::steps_ahead`]): no fewer than it
    /// takes now.
    pub(super) steps_ahead: u64,
    /// Whether the held document was not read back from its state because reading it
    /// would take more steps than [`fvoci_collab::READING_STEPS`]: its body must be
    /// written to its card and the document made again from it before its state is
    /// folded or another update is taken.
    pub(super) must_remake: bool,
    /// Whether the worker asked for a fold because its rollback log (`since`) grew
    /// large enough to rebase.
    pub(super) wants_fold: bool,
    /// Who sent the last update; what writes the body writes it as them.
    pub(super) last_writer: String,
    /// When the first update whose body is not written was taken.
    pub(super) first_unwritten: Option<Instant>,
    /// When the last update was taken.
    pub(super) last_update: Instant,
    /// Whether a task waits to write its body.
    pub(super) writing_scheduled: bool,
    /// Not before when its body is written again, after a write that failed.
    pub(super) retry_at: Option<Instant>,
    /// Whether its body is kept as a version of its card ([`super::versions`]).
    pub(super) versioned: Versioned,
    /// Who is known to be put down as having written the body since its newest
    /// version: the first update of anybody else puts them down.
    pub(super) editors: Vec<String>,
    /// Opened aside ([`Share::Aside`]), only to write its log to its card: its place
    /// in the server's share aside, given back as it is closed. `None` for a document
    /// counted in its worker's load.
    pub(super) aside: Option<OwnedSemaphorePermit>,
}

impl OpenDoc {
    /// Whether it names three quarters of the clients a document may name
    /// ([`fvoci_collab::MAX_CLIENTS`]), a client of it has three quarters of the
    /// changes one may have ([`fvoci_collab::MAX_CLIENT_CHANGES`]), or one more client
    /// may leave it with three quarters of the steps to read that an update may leave
    /// it with ([`fvoci_collab::READING_STEPS`]): it is made again from its body,
    /// which names one client (or a handful for the largest bodies), of fewer changes
    /// than that, and is read in no such steps, before updates are refused.
    ///
    /// The steps are those one more client may leave it with
    /// ([`fvoci_collab::steps_ahead`]), not those it takes: a document that sessions
    /// wrote one after the other goes from few steps to past the limit with the first
    /// paragraph of one session, whose client is read before the others, and that
    /// update would be refused. The room is told, and its editors load the document
    /// anew, where they would have had an update refused and then loaded it anew. A
    /// document of that many steps ahead names fewer clients than three quarters of
    /// those it may (each client has a change, and each change is counted once for
    /// each client: 10,034 clients are three quarters of the steps), so the mark of
    /// the clients is for limits without a count of steps.
    pub(super) fn crowded(&self) -> bool {
        self.must_remake || crowded(self.clients, self.client_changes, self.steps_ahead)
    }

    /// Whether it is made again from its body as it is closed with nobody in its
    /// room, where no editor has to load it anew: it must be remade, it names an
    /// eighth of the clients a document may name, or one more client may leave it with
    /// an eighth of the steps to read that an update may leave it with. A room that
    /// empties now and then keeps its document far from where it is made again under
    /// its editors ([`Self::crowded`]).
    ///
    /// For the bodies of the most changes (~647k–660k across 5–6 split clients) that is
    /// every 25 sessions at the latest, for a body of 10,000 changes every 1,677, and for
    /// smaller ones every 2,048. A body's own document is far under it, so none is made
    /// again each time it is closed.
    pub(super) fn worn(&self) -> bool {
        self.must_remake || worn(self.clients, self.steps_ahead)
    }

    /// Whether its state size or total change count has reached three quarters of its
    /// limit and grown by at least a quarter since it was made from its body. A freshly
    /// made body near either limit (such as a 100k-node marked document at ~595k–660k
    /// changes) is not remade on every edit.
    pub(super) fn grown_near_limit(&self, state_limit: u64) -> bool {
        grown_near_limit(self.state_bytes, self.made_bytes, state_limit)
            || grown_near_limit(
                self.changes,
                self.made_changes,
                fvoci_collab::MAX_TOTAL_CHANGES,
            )
    }

    /// Whether a `DocumentTooLarge` refusal should remake the document to reclaim deleted
    /// tombstones: true when the document is at three quarters of its state-size or
    /// total-change limit and has grown since it was made from its body (or is crowded).
    pub(super) fn remake_on_too_large(&self, state_limit: u64) -> bool {
        refused_near_limit(self.state_bytes, self.made_bytes, state_limit)
            || refused_near_limit(
                self.changes,
                self.made_changes,
                fvoci_collab::MAX_TOTAL_CHANGES,
            )
            || self.crowded()
    }

    /// The share of its worker's documents it is counted in.
    pub(super) fn share(&self) -> Share {
        if self.aside.is_some() {
            Share::Aside
        } else {
            Share::Kept
        }
    }
}

/// Whether a document of `clients` clients that one more client may leave with
/// `steps_ahead` steps to read is made again from its body as it is closed with nobody
/// in its room ([`OpenDoc::worn`]).
fn worn(clients: u64, steps_ahead: u64) -> bool {
    let an_eighth = |count: u64, limit: u64| count >= limit / 8;
    let max_clients = u64::try_from(fvoci_collab::MAX_CLIENTS).unwrap_or(u64::MAX);
    an_eighth(clients, max_clients) || an_eighth(steps_ahead, fvoci_collab::READING_STEPS)
}

/// Whether `current` reached three quarters of `limit` and grew by at least a quarter
/// of `limit` above `made` (what the document started at when made from its body).
fn grown_near_limit(current: u64, made: u64, limit: u64) -> bool {
    current.saturating_mul(4) >= limit.saturating_mul(3)
        && current >= made.saturating_add(limit / 4)
}

/// Whether `current` is strictly above `made` and at three quarters of `limit`, so a
/// `DocumentTooLarge` refusal can reclaim tombstones by remaking from the body.
fn refused_near_limit(current: u64, made: u64, limit: u64) -> bool {
    current > made && current.saturating_mul(4) >= limit.saturating_mul(3)
}

/// Whether a document of `clients` clients, of which one has `client_changes` changes,
/// that one more client may leave with `steps_ahead` steps to read, is made again from
/// its body ([`OpenDoc::crowded`]).
fn crowded(clients: u64, client_changes: u64, steps_ahead: u64) -> bool {
    let three_quarters =
        |count: u64, limit: u64| count.saturating_mul(4) >= limit.saturating_mul(3);
    let max_clients = u64::try_from(fvoci_collab::MAX_CLIENTS).unwrap_or(u64::MAX);
    three_quarters(clients, max_clients)
        || three_quarters(client_changes, fvoci_collab::MAX_CLIENT_CHANGES)
        || three_quarters(steps_ahead, fvoci_collab::READING_STEPS)
}

impl OpenDoc {
    /// The numbers of what is stored of it as this server knows them: as it read them
    /// when it opened the document, and as its own writes left them since. Every
    /// write of the document names them, so that the store refuses a server that
    /// another wrote past (one that lost the lock of the server and has not noticed
    /// yet). A server that is refused closes the document, and reads it again when it
    /// is next asked for.
    pub(super) fn as_read(&self) -> DocAsRead {
        DocAsRead {
            epoch: self.epoch,
            state_seq: self.state_seq,
            log_end: self.log_end,
            projected_seq: self.written_seq,
        }
    }

    /// When its body is to be written, if it has updates whose body is not.
    pub(super) fn write_at(&self) -> Option<Instant> {
        let first = self.first_unwritten?;
        let due = (self.last_update + super::WRITE_QUIET).min(first + super::WRITE_LONGEST);
        Some(self.retry_at.map_or(due, |retry_at| due.max(retry_at)))
    }
}

/// The key of an organization's card in the place.
pub(super) fn key_of(tenant: Tenant, card_id: &str) -> String {
    format!("{tenant:?} {card_id}")
}

impl Collab {
    /// Resolves the `container_id` of `card_id`, checking the in-memory `slots` table
    /// first and falling back to a lightweight store lookup across live and deleted
    /// blocks. Returns `Ok(None)` if no such block exists.
    pub(super) async fn slot_container_id(
        &self,
        app: &App,
        card_id: &str,
    ) -> Result<Option<String>, AppError> {
        if let Some(container_id) = self.existing_slot_container_id(app, card_id) {
            return Ok(Some(container_id));
        }
        Ok(app.store().find_any_block_container_id(card_id).await?)
    }

    /// Returns the `container_id` of `card_id` if a slot currently exists in memory.
    pub(super) fn existing_slot_container_id(&self, app: &App, card_id: &str) -> Option<String> {
        let key = key_of(app.store().tenant(), card_id);
        let slots = self.inner.slots.lock().expect("the slots are not poisoned");
        slots
            .get(&key)
            .map(|(container_id, _)| container_id.clone())
    }

    /// The slot of a card of `app`'s organization in `container_id`, held.
    pub(super) async fn slot_in(&self, app: &App, card_id: &str, container_id: &str) -> Held {
        debug_assert!(
            !container_id.is_empty(),
            "slot_in requires a non-empty container_id"
        );
        let tenant = app.store().tenant();
        let key = key_of(tenant, card_id);
        loop {
            let shared = {
                let mut slots = self.inner.slots.lock().expect("the slots are not poisoned");
                let entry = slots.entry(key.clone()).or_insert_with(|| {
                    (
                        container_id.to_owned(),
                        Arc::new(Mutex::new(Slot {
                            key: key.clone(),
                            card_id: card_id.to_owned(),
                            container_id: container_id.to_owned(),
                            tenant,
                            retired: false,
                            open: None,
                            deaths: 0,
                            last_death: None,
                            closed_until: None,
                            remake_failed_at: None,
                        })),
                    )
                });
                Arc::clone(&entry.1)
            };
            let slot = shared.lock_owned().await;
            if !slot.retired {
                return slot;
            }
        }
    }

    /// The slot of a card of `app`'s organization if it has one, held.
    pub(super) async fn existing_slot(&self, app: &App, card_id: &str) -> Option<Held> {
        let key = key_of(app.store().tenant(), card_id);
        loop {
            let shared = {
                let slots = self.inner.slots.lock().expect("the slots are not poisoned");
                Arc::clone(&slots.get(&key)?.1)
            };
            let slot = shared.lock_owned().await;
            if !slot.retired {
                return Some(slot);
            }
        }
    }

    /// How many bytes the state of a card's open document is, as its worker counts it
    /// against what it may hold; `None` if it is not open. For the tests that measure.
    #[cfg(feature = "test-util")]
    #[doc(hidden)]
    pub async fn held_state_bytes(&self, app: &App, card_id: &str) -> Option<u64> {
        let slot = self.existing_slot(app, card_id).await?;
        slot.open.as_ref().map(|open| open.state_bytes)
    }

    /// How many clients a card's open document names, as recorded on [`OpenDoc`]; `None`
    /// if it is not open.
    #[cfg(feature = "test-util")]
    #[doc(hidden)]
    pub async fn held_clients(&self, app: &App, card_id: &str) -> Option<u64> {
        let slot = self.existing_slot(app, card_id).await?;
        slot.open.as_ref().map(|open| open.clients)
    }

    /// How many worker processes ended, of every worker: for the tests of how many an
    /// update ends.
    #[cfg(feature = "test-util")]
    #[doc(hidden)]
    pub async fn workers_ended(&self) -> u64 {
        let mut ended = 0;
        for worker in &self.inner.workers {
            ended += worker
                .ended()
                .await
                .expect("the worker's task runs")
                .workers;
        }
        ended
    }

    /// How many users, of every organization, have an update that holds or waits for
    /// their turn ([`super::senders::Senders::turn`]): none once every update is done.
    #[cfg(feature = "test-util")]
    #[doc(hidden)]
    #[must_use]
    pub fn senders_in_line(&self) -> usize {
        self.inner.senders.lines()
    }

    /// Every slot.
    pub(super) fn all_slots(&self) -> Vec<Arc<Mutex<Slot>>> {
        let slots = self.inner.slots.lock().expect("the slots are not poisoned");
        slots.values().map(|(_, slot)| Arc::clone(slot)).collect()
    }

    /// Takes a slot out of the place, unless its document is open, or it remembers that
    /// its document is closed for a while or that it ended its worker.
    pub(super) fn retire(&self, slot: &mut Held) {
        let now = Instant::now();
        let closed = slot.closed_until.is_some_and(|until| until > now);
        let remembered = slot
            .last_death
            .is_some_and(|last| now.duration_since(last) <= DEATHS_REMEMBERED);
        if slot.retired || slot.open.is_some() || closed || remembered {
            return;
        }
        slot.retired = true;
        let mut slots = self.inner.slots.lock().expect("the slots are not poisoned");
        if slots
            .get(&slot.key)
            .is_some_and(|(_, there)| Arc::ptr_eq(there, OwnedMutexGuard::mutex(slot)))
        {
            slots.remove(&slot.key);
        }
    }

    /// Closes the document in its worker, if it is open, and forgets it.
    pub(super) async fn forget(&self, slot: &mut Held) {
        if let Some(open) = slot.open.take() {
            if open.aside.is_none() {
                taken_from(&self.inner.loads[open.worker]);
            }
            self.close_number(open.worker, open.generation, open.number)
                .await;
        }
    }

    /// Asks the worker that holds the document a request made for its number, opening
    /// the document first if it is not open, and again once if its worker is gone.
    ///
    /// # Errors
    ///
    /// Returns a refusal of the worker as the reason it stands for, `document_unavailable`
    /// if the worker ended over it (and the document is closed for a while if that keeps
    /// happening), and the errors of opening the document.
    pub(super) async fn ask(
        &self,
        app: &App,
        card_id: &str,
        slot: &mut Held,
        request: impl Fn(DocNumber) -> Request,
    ) -> Result<Answer, AppError> {
        self.ask_for(app, card_id, slot, request, AskedFor::Server)
            .await
            .map_err(Unanswered::into_error)
    }

    /// [`Collab::ask`] for an [`AskedFor`], telling a death of the worker over the request
    /// apart.
    ///
    /// # Errors
    ///
    /// Returns [`Unanswered::Died`] if the worker ended over the request, and the
    /// errors of [`Collab::ask`] otherwise as [`Unanswered::Refused`]; for a sender
    /// held back, `document_unavailable` before the request is sent.
    pub(super) async fn ask_for(
        &self,
        app: &App,
        card_id: &str,
        slot: &mut Held,
        request: impl Fn(DocNumber) -> Request,
        asked_for: AskedFor<'_>,
    ) -> Result<Answer, Unanswered> {
        let blame = asked_for.blame();
        for _ in 0..2 {
            self.ensure_open(app, card_id, slot).await?;
            // The sender's turn, with the slot held and its document open: a room's turn,
            // the organization's gate, the slot, the sender's turn, then the worker's
            // queue, and whoever holds a sender's turn waits for no room, gate or slot. In
            // it, again after the waits for the document and for the turn, the sender is
            // checked, the update sent and its death put down: of the updates a sender
            // sent together, to one worker or several, those after the first that ended
            // a worker are not sent. A try that finds the worker gone gives the turn back
            // and waits for it again.
            let _turn = match asked_for {
                AskedFor::Sender(sender) => {
                    let turn = self
                        .inner
                        .senders
                        .turn(app.store().tenant(), sender.key)
                        .await;
                    self.held_back(app, sender.key)?;
                    Some(turn)
                }
                AskedFor::Server => None,
            };
            let open = slot.open.as_ref().expect("the document is open");
            let worker = &self.inner.workers[open.worker];
            match worker
                .call_blaming(Some(open.generation), &request(open.number), blame)
                .await
            {
                Ok((_, answer)) => return Ok(answer),
                Err(CallError::Lost) => {
                    let aside = open.aside.is_some();
                    self.lost(slot);
                    self.tell(app, card_id, DocChange::Reopened);
                    if aside {
                        // Opened aside for its log alone: not opened again in the kept
                        // share, which may be full; the log stays for the next try.
                        return Err(refusals::unavailable().into());
                    }
                }
                Err(CallError::Refused(refused)) if refused.refusal == Refusal::Closed => {
                    tracing::error!(
                        card_id,
                        reason = %refused.message,
                        "a collab worker closed a document after a failed rollback"
                    );
                    self.lost(slot);
                    self.tell(app, card_id, DocChange::Reopened);
                    return Err(refusals::unavailable().into());
                }
                Err(CallError::Refused(refused)) => {
                    if refused.rolled_back
                        && let AskedFor::Sender(sender) = asked_for
                    {
                        self.inner.senders.rolled_back(
                            app.store().tenant(),
                            sender,
                            Instant::now(),
                        );
                    }
                    let error = refusals::refused(&refused, &self.inner.limits);
                    return Err(match refused.refusal {
                        Refusal::TooManySteps => Unanswered::TooManySteps(error),
                        _ => Unanswered::Refused(error),
                    });
                }
                Err(CallError::Died(death)) => {
                    tracing::warn!(%death, card_id, "a collab worker ended over a card's document");
                    self.lost(slot);
                    died_over(slot);
                    if let AskedFor::Sender(sender) = asked_for {
                        tracing::warn!(
                            card_id,
                            sender = sender.key,
                            "the update ended the worker; its sender is held back"
                        );
                        self.inner.senders.ended_worker(
                            app.store().tenant(),
                            sender,
                            Instant::now(),
                        );
                    }
                    return Err(Unanswered::Died(refusals::unavailable()));
                }
                Err(CallError::Unavailable(error)) => {
                    tracing::error!(%error, card_id, "no collab worker for a card's document");
                    return Err(refusals::unavailable().into());
                }
            }
        }
        Err(refusals::unavailable().into())
    }

    /// Opens the document if it is not open, unless it is closed for a while. A slot
    /// whose document did not open leaves the place, unless it is closed for a while.
    ///
    /// # Errors
    ///
    /// Returns `document_unavailable` while it is closed, and the errors of opening it.
    pub(super) async fn ensure_open(
        &self,
        app: &App,
        card_id: &str,
        slot: &mut Held,
    ) -> Result<(), AppError> {
        if slot.open.is_some() {
            return Ok(());
        }
        if closed_for_a_while(slot) {
            return Err(refusals::unavailable());
        }
        let error = match self.open_stored(app, card_id, None).await {
            Ok(open) => {
                let past_limit = open.state_bytes > self.inner.limits.state_bytes
                    || open.changes > fvoci_collab::MAX_TOTAL_CHANGES
                    || open.crowded();
                slot.open = Some(open);
                if past_limit {
                    // Stored or logged under a larger limit: its body is written to the
                    // card, and it is made again from that.
                    // Boxed: the remake asks the open document, through here.
                    Box::pin(self.remake_and_tell(
                        app,
                        card_id,
                        slot,
                        super::upkeep::OnForget::Quiet,
                    ))
                    .await;
                    if slot.open.is_none() {
                        // The remake failed and closed it: the store holds it.
                        return Err(refusals::unavailable());
                    }
                }
                return Ok(());
            }
            Err(Opening::Refused(error) | Opening::WrittenSince(error)) => error,
            Err(Opening::Died) => {
                died_over(slot);
                refusals::unavailable()
            }
        };
        self.retire(slot);
        Err(error)
    }

    /// Opens the document aside ([`Share::Aside`]), only to write its log to its card:
    /// within the server's share aside, waiting for a place in it, so that a server
    /// that holds as many documents as it may still writes a log. Closed by the one
    /// that opened it ([`Collab::forget`]) before the slot is let go.
    ///
    /// # Errors
    ///
    /// As [`Collab::ensure_open`].
    pub(super) async fn open_aside(
        &self,
        app: &App,
        card_id: &str,
        slot: &mut Held,
    ) -> Result<(), AppError> {
        if closed_for_a_while(slot) {
            return Err(refusals::unavailable());
        }
        let place = Arc::clone(&self.inner.aside)
            .acquire_owned()
            .await
            .map_err(|_| refusals::unavailable())?;
        match self.open_stored(app, card_id, Some(place)).await {
            Ok(open) => {
                slot.open = Some(open);
                Ok(())
            }
            Err(Opening::Refused(error) | Opening::WrittenSince(error)) => Err(error),
            Err(Opening::Died) => {
                died_over(slot);
                Err(refusals::unavailable())
            }
        }
    }

    /// Opens the document if it is not open, and refuses a request made against
    /// another making of it than `epoch` (`update_out_of_step`).
    ///
    /// # Errors
    ///
    /// As [`Collab::ensure_open`], and `update_out_of_step`.
    pub(super) async fn at_epoch(
        &self,
        app: &App,
        card_id: &str,
        slot: &mut Held,
        epoch: i64,
    ) -> Result<(), AppError> {
        self.ensure_open(app, card_id, slot).await?;
        still_at_epoch(slot, epoch)
    }

    /// Forgets a document whose worker is gone.
    pub(super) fn lost(&self, slot: &mut Slot) {
        if let Some(open) = slot.open.take()
            && open.aside.is_none()
        {
            taken_from(&self.inner.loads[open.worker]);
        }
    }

    /// Tells the subscribers what happened to a card's document.
    pub(super) fn tell(&self, app: &App, card_id: &str, change: DocChange) {
        // Nobody may be listening.
        let _ = self.inner.events.send(DocEvent {
            tenant: app.store().tenant(),
            card_id: card_id.to_owned(),
            change,
        });
    }
}

/// Whether the document is closed for a while, after its worker ended over it.
fn closed_for_a_while(slot: &Slot) -> bool {
    slot.closed_until
        .is_some_and(|until| until > Instant::now())
}

/// Counts a death of the worker over the document, and closes the document for a
/// while from the second (see [`Slot`]).
pub(super) fn died_over(slot: &mut Slot) {
    let now = Instant::now();
    if slot
        .last_death
        .is_some_and(|last| now.duration_since(last) > DEATHS_REMEMBERED)
    {
        slot.deaths = 0;
    }
    slot.last_death = Some(now);
    slot.deaths = slot.deaths.saturating_add(1);
    if slot.deaths >= 2 {
        let doublings = (slot.deaths - 2).min(16);
        let closing = FIRST_CLOSING
            .saturating_mul(1 << doublings)
            .min(LONGEST_CLOSING);
        slot.closed_until = Some(now + closing);
    }
}

/// Why a request about a document got no answer ([`Collab::ask_for`]).
#[derive(Debug)]
pub(super) enum Unanswered {
    /// It was refused, the document could not be opened, no worker could be started,
    /// or its sender is held back: the error to answer with.
    Refused(AppError),
    /// It was refused as the document would take more steps to read with it than it
    /// may ([`fvoci_collab::READING_STEPS`]): the error to answer with
    /// (`document_too_large`). The document is made again from its body
    /// ([`Collab::remake_refused`]).
    TooManySteps(AppError),
    /// The worker ended over it: killed after its time, or broken. An update that did
    /// is in neither the document nor the log, its sender is held back
    /// ([`super::senders`]), and the connection that sent it is closed. The error to
    /// answer with (`document_unavailable`).
    Died(AppError),
}

/// Who a request is asked for, and so whom a death of the worker over it is put down to.
#[derive(Debug, Clone, Copy)]
pub(super) enum AskedFor<'user> {
    /// The server, for itself or for what an editor reads: the worker
    /// ([`Blame::Worker`]).
    Server,
    /// The sender whose update the request takes ([`Blame::Sender`]): asked in their
    /// turn ([`super::senders::Senders::turn`]), held back for a death, and refused
    /// while held back.
    Sender(SenderOf<'user>),
}

impl AskedFor<'_> {
    /// Whom a death of the worker over the request is put down to.
    fn blame(self) -> Blame {
        match self {
            Self::Server => Blame::Worker,
            Self::Sender(_) => Blame::Sender,
        }
    }
}

impl Unanswered {
    /// The error to answer with.
    pub(super) fn into_error(self) -> AppError {
        match self {
            Self::Refused(error) | Self::TooManySteps(error) | Self::Died(error) => error,
        }
    }
}

impl From<AppError> for Unanswered {
    fn from(error: AppError) -> Self {
        Self::Refused(error)
    }
}

/// Why a document could not be opened.
#[derive(Debug)]
pub(super) enum Opening {
    /// Refused, or the store failed: the error to answer with.
    Refused(AppError),
    /// The worker ended over it.
    Died,
    /// Another process wrote what is stored of the document since it was read here
    /// ([`StoreError::DocumentWritten`]): the error to answer with. What is stored
    /// is that process's, and nothing here writes over it.
    WrittenSince(AppError),
}

impl From<AppError> for Opening {
    fn from(error: AppError) -> Self {
        Self::Refused(error)
    }
}

impl From<StoreError> for Opening {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::DocumentWritten { .. } => Self::WrittenSince(error.into()),
            other => Self::Refused(other.into()),
        }
    }
}

/// Refuses a request made against another making of the document than `epoch`: one the
/// document was made again under while the request was asked (a worker that ended,
/// whose document was opened again from a card written past it).
///
/// # Errors
///
/// Returns `update_out_of_step`.
pub(super) fn still_at_epoch(slot: &Slot, epoch: i64) -> Result<(), AppError> {
    if slot.open.as_ref().is_some_and(|open| open.epoch == epoch) {
        Ok(())
    } else {
        Err(refusals::out_of_step())
    }
}

#[cfg(test)]
mod tests {
    use fvoci_collab::{MAX_CLIENT_CHANGES, MAX_CLIENTS, MAX_TOTAL_CHANGES, READING_STEPS};

    use super::{crowded, grown_near_limit, refused_near_limit, worn};

    #[test]
    fn total_changes_remake_marks_allow_fresh_marked_bodies_and_recover_grown_ones() {
        // Worst-case valid 100k marked body starts around 660k (< 786,432): normal typing
        // does not remake on every keystroke, and a single hostile jump refused while
        // fresh does not trigger a pointless remake loop.
        let fresh = 660_372;
        assert!(!grown_near_limit(fresh, fresh, MAX_TOTAL_CHANGES));
        assert!(!grown_near_limit(fresh + 1_000, fresh, MAX_TOTAL_CHANGES));
        assert!(!refused_near_limit(fresh, fresh, MAX_TOTAL_CHANGES));
        // Once real edits accumulate tombstones above `fresh`, a `DocumentTooLarge`
        // refusal remakes to reclaim them.
        assert!(refused_near_limit(fresh + 10_000, fresh, MAX_TOTAL_CHANGES));
        // A small body that grew to 3/4 of MAX_TOTAL_CHANGES across many clients is
        // proactively remade before hitting the cap.
        let three_quarters = MAX_TOTAL_CHANGES * 3 / 4;
        assert!(!grown_near_limit(
            three_quarters - 1,
            5_000,
            MAX_TOTAL_CHANGES
        ));
        assert!(grown_near_limit(three_quarters, 5_000, MAX_TOTAL_CHANGES));
        // Stored doc opened from DB (`made == 0`) near the cap remakes once on refusal.
        assert!(refused_near_limit(700_000, 0, MAX_TOTAL_CHANGES));
    }

    #[test]
    fn a_document_is_made_again_at_three_quarters_of_its_clients_or_of_a_client_s_changes() {
        let clients = u64::try_from(MAX_CLIENTS).expect("fits");
        assert!(!crowded(1, 1, 0));
        assert!(!crowded(
            clients * 3 / 4 - 1,
            MAX_CLIENT_CHANGES * 3 / 4 - 1,
            READING_STEPS * 3 / 4 - 1
        ));
        assert!(crowded(clients * 3 / 4, 1, 0), "12,288 clients");
        assert!(
            crowded(1, MAX_CLIENT_CHANGES * 3 / 4, 0),
            "393,216 changes of a client"
        );
        assert!(
            crowded(1, 1, READING_STEPS * 3 / 4),
            "786,432 steps to read it"
        );
    }

    #[test]
    fn a_document_is_made_again_as_it_closes_at_an_eighth_of_its_clients_or_steps_ahead() {
        let clients = u64::try_from(MAX_CLIENTS).expect("fits");
        assert!(!worn(1, 0));
        assert!(!worn(clients / 8 - 1, READING_STEPS / 8 - 1));
        assert!(worn(clients / 8, 0), "2,048 clients");
        assert!(worn(1, READING_STEPS / 8), "131,072 steps ahead");
        assert!(
            !crowded(clients / 8, 1, READING_STEPS / 8),
            "far from where it is made again under its editors"
        );
    }
}
