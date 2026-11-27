//! Opening a card's document from what is stored: its state and the updates logged
//! after it, or, the first time, its card's body.

use std::hash::{Hash, Hasher};
use std::sync::atomic::Ordering;

use fvoci_model::{Block, Body};
use fvoci_store::{LoggedUpdate, StoreError, StoredDoc, Tenant, body_digest};
use tokio::sync::OwnedSemaphorePermit;
use tokio::time::Instant;

use super::place::{OpenDoc, Opening};
use super::versions::Versioned;
use super::worker::CallError;
use super::{Collab, DocChange, given, refusals};
use crate::app::App;
use crate::collab_worker::wire::{Answer, DocNumber, Request, Share, Source};
use crate::error::{AppError, Reason};

impl Collab {
    /// Opens a card's document in the worker that holds the fewest: from what is
    /// stored, or made from the card's body if nothing is. A document whose body the
    /// card holds but that is no longer the body the card has was left behind by a
    /// write of the body that did not go through it (an import, a copy, a restore):
    /// it is made again from the card's body.
    ///
    /// With a place of the share aside (`aside`), a stored document is opened aside
    /// ([`Share::Aside`]), only to write its log to its card: it is not counted in its
    /// worker's load, and no write of its body is scheduled. A document never stored is
    /// made as any other.
    pub(super) async fn open_stored(
        &self,
        app: &App,
        card_id: &str,
        aside: Option<OwnedSemaphorePermit>,
    ) -> Result<OpenDoc, Opening> {
        let block = card(app, card_id).await?;
        let worker = self.worker_for(app.store().tenant());
        let number = self.next_number();
        let mut open = match app.store().get_body_doc(card_id).await? {
            None => self.make(app, &block, worker, number).await?,
            Some(stored) => {
                self.reopen(app, &block, worker, number, stored, aside)
                    .await?
            }
        };
        if open.aside.is_none() {
            given(&self.inner.loads[worker]);
            if open.first_unwritten.is_some() {
                self.schedule_write(app, card_id, &mut open);
            }
        }
        Ok(open)
    }

    /// The first document of a card, made from its body and stored.
    async fn make(
        &self,
        app: &App,
        block: &Block,
        worker: usize,
        number: DocNumber,
    ) -> Result<OpenDoc, Opening> {
        let Made {
            generation,
            state,
            state_bytes,
            clients,
            changes,
            client_changes,
            steps_ahead,
        } = self
            .open_from_body(worker, number, block.body.as_ref(), Share::Kept)
            .await?;
        let digest = body_digest(block.body.as_ref());
        if let Err(error) = app
            .store()
            .create_body_doc(&block.id, &state, &digest)
            .await
        {
            self.close_number(worker, generation, number).await;
            return Err(error.into());
        }
        Ok(OpenDoc {
            worker,
            generation,
            number,
            epoch: 1,
            state_seq: 0,
            log_end: 0,
            written_seq: 0,
            body_digest: digest,
            state_bytes,
            made_bytes: state_bytes,
            clients,
            changes,
            made_changes: changes,
            client_changes,
            steps_ahead,
            must_remake: false,
            wants_fold: false,
            last_writer: block.modified_by.clone(),
            first_unwritten: None,
            last_update: Instant::now(),
            writing_scheduled: false,
            retry_at: None,
            versioned: Versioned::Unknown,
            editors: Vec::new(),
            aside: None,
        })
    }

    /// A stored document, with the updates logged after its state taken again.
    ///
    /// What was stored and logged is taken again whatever the limits are now: what was
    /// taken is taken (a document past the limit of a state, or that takes many steps
    /// to read, is written to its card and made again from it once it is open). A
    /// document that cannot be opened so is made again from its card's body when its
    /// card holds all of it, or when what is stored cannot be read (bytes that are no
    /// update, that build on what is not there, that exceed
    /// [`fvoci_collab::MAX_TOTAL_CHANGES`], or that the Y library would take longer to
    /// read than a document may, [`fvoci_collab::READING_STEPS`]):
    /// the updates the card does not hold are then dropped, and that is logged.
    /// Otherwise (a worker that is full or ended, the store) the open fails, and the
    /// next one tries again.
    async fn reopen(
        &self,
        app: &App,
        block: &Block,
        worker: usize,
        number: DocNumber,
        stored: StoredDoc,
        aside: Option<OwnedSemaphorePermit>,
    ) -> Result<OpenDoc, Opening> {
        let unwritten = stored.projected_seq < stored.log_end;
        // Updates the card does not hold, or whose writers no version names (a keep
        // that failed, a server that ended): a version is due at once, so that leaving
        // the document keeps it, whoever opened it.
        let versioned = if unwritten || !stored.unkept_editors.is_empty() {
            Versioned::Due(self.version_clock())
        } else {
            Versioned::Unknown
        };
        let mut open = OpenDoc {
            worker,
            // Not open yet: no generation is 0.
            generation: 0,
            number,
            epoch: stored.epoch,
            state_seq: stored.state_seq,
            log_end: stored.log_end,
            written_seq: stored.projected_seq,
            body_digest: stored.body_digest,
            state_bytes: 0,
            made_bytes: 0,
            clients: 0,
            changes: 0,
            made_changes: 0,
            client_changes: 0,
            steps_ahead: 0,
            must_remake: false,
            wants_fold: false,
            // Who sent the last update logged, which the fold of the log keeps.
            last_writer: if stored.last_writer.is_empty() {
                block.modified_by.clone()
            } else {
                stored.last_writer
            },
            first_unwritten: unwritten.then(Instant::now),
            last_update: Instant::now(),
            writing_scheduled: false,
            retry_at: None,
            versioned,
            editors: stored.unkept_editors,
            aside,
        };
        let opened = self
            .open_and_replay(app, &block.id, &mut open, stored.state, stored.updates)
            .await;
        if let Err(failed) = opened {
            self.close_number(worker, open.generation, number).await;
            let for_good = refused_for_good(&failed);
            // What another process wrote since is that process's: not made again here.
            if matches!(failed, Opening::Died | Opening::WrittenSince(_))
                || (unwritten && !for_good)
            {
                return Err(failed);
            }
            if unwritten {
                tracing::error!(
                    card_id = block.id,
                    "what is stored of a document cannot be read; it is made again from its card, without the updates the card does not hold"
                );
            } else {
                tracing::error!(
                    card_id = block.id,
                    "what is stored of a document is not taken again; it is made again from its card"
                );
            }
            return self.made_from_card(app, block, open).await;
        }
        let written_elsewhere = body_digest(block.body.as_ref()) != open.body_digest;
        if !unwritten && written_elsewhere {
            return self.made_from_card(app, block, open).await;
        }
        Ok(open)
    }

    /// Makes a document again from its card's body, in place of what is stored of it.
    async fn made_from_card(
        &self,
        app: &App,
        block: &Block,
        mut open: OpenDoc,
    ) -> Result<OpenDoc, Opening> {
        if let Err(failed) = self
            .remake_from(app, &block.id, &mut open, block.body.as_ref())
            .await
        {
            self.close_number(open.worker, open.generation, open.number)
                .await;
            return Err(failed);
        }
        self.tell(app, &block.id, DocChange::Remade);
        Ok(open)
    }

    /// Opens a stored state in the worker of `open` under its number, and takes the
    /// logged updates again.
    async fn open_and_replay(
        &self,
        app: &App,
        card_id: &str,
        open: &mut OpenDoc,
        state: Vec<u8>,
        updates: Vec<LoggedUpdate>,
    ) -> Result<(), Opening> {
        let opened = self
            .call(
                open.worker,
                None,
                &Request::Open {
                    doc: open.number,
                    source: Source::State(state),
                    share: open.share(),
                },
            )
            .await?;
        let (
            generation,
            Answer::Opened {
                state,
                state_bytes,
                clients,
                changes,
                client_changes,
                steps_ahead,
            },
        ) = opened
        else {
            return Err(unexpected(&opened.1).into());
        };
        open.generation = generation;
        open.state_bytes = state_bytes;
        open.clients = clients;
        open.changes = changes;
        open.client_changes = client_changes;
        open.steps_ahead = steps_ahead;
        let state_seq = open.state_seq;
        self.replay(app, card_id, open, state, state_seq, updates)
            .await
    }

    /// Takes the logged updates again into a document opened from its stored state,
    /// after the state the worker repaired as it opened it is stored.
    async fn replay(
        &self,
        app: &App,
        card_id: &str,
        open: &mut OpenDoc,
        repaired: Option<Vec<u8>>,
        state_seq: i64,
        updates: Vec<LoggedUpdate>,
    ) -> Result<(), Opening> {
        // What a repair changed in the stored state is stored, so that the state and
        // the log after it stay the document's.
        if let Some(repaired) = repaired {
            app.store()
                .fold_body_doc(card_id, &repaired, state_seq, open.as_read())
                .await?;
        }
        let mut skip_repair: Option<Vec<u8>> = None;
        for update in updates {
            if update.written_by.is_empty()
                && skip_repair.as_deref() == Some(update.bytes.as_slice())
            {
                // The preceding `Retake` left the document past `READING_STEPS` and
                // already applied this repair in place; skip the logged repair row.
                skip_repair = None;
                continue;
            }
            skip_repair = None;
            if open.must_remake {
                // Another logged update follows one that already left the document
                // unsettled past `READING_STEPS`: do not hand the slow state back to
                // `yrs`; fall back to remaking from the card's body.
                return Err(Opening::Refused(
                    AppError::bad_request(
                        "a logged update follows one that left the document past the steps it may take to read",
                    )
                    .with_reason(Reason::UpdateRefused),
                ));
            }
            let request = Request::Retake {
                doc: open.number,
                update: update.bytes,
            };
            match self
                .call(open.worker, Some(open.generation), &request)
                .await?
            {
                (
                    _,
                    Answer::Taken {
                        repair,
                        state_bytes,
                        clients,
                        changes,
                        client_changes,
                        steps_ahead,
                        remake,
                        fold,
                        ..
                    },
                ) => {
                    open.state_bytes = state_bytes;
                    open.clients = clients;
                    open.changes = changes;
                    open.client_changes = client_changes;
                    open.steps_ahead = steps_ahead;
                    open.must_remake |= remake;
                    open.wants_fold |= fold;
                    if remake {
                        skip_repair = repair;
                    }
                }
                (_, other) => return Err(unexpected(&other).into()),
            }
            if !update.written_by.is_empty() {
                open.last_writer = update.written_by;
            }
        }
        Ok(())
    }

    /// Opens a document made from a body in a worker, counted in `share`.
    pub(super) async fn open_from_body(
        &self,
        worker: usize,
        number: DocNumber,
        body: Option<&Body>,
        share: Share,
    ) -> Result<Made, Opening> {
        let json = body
            .map(|body| serde_json::to_vec(body.document()))
            .transpose()
            .map_err(AppError::internal)?;
        let request = Request::Open {
            doc: number,
            source: Source::Body(json),
            share,
        };
        match self.call(worker, None, &request).await? {
            (
                generation,
                Answer::Opened {
                    state: Some(state),
                    state_bytes,
                    clients,
                    changes,
                    client_changes,
                    steps_ahead,
                },
            ) => Ok(Made {
                generation,
                state,
                state_bytes,
                clients,
                changes,
                client_changes,
                steps_ahead,
            }),
            (_, other) => Err(unexpected(&other).into()),
        }
    }

    /// Asks a worker a request while a document is opened.
    async fn call(
        &self,
        worker: usize,
        generation: Option<u64>,
        request: &Request,
    ) -> Result<(u64, Answer), Opening> {
        match self.inner.workers[worker].call(generation, request).await {
            Ok(answered) => Ok(answered),
            Err(CallError::Refused(refused)) => Err(Opening::Refused(refusals::refused(
                &refused,
                &self.inner.limits,
            ))),
            Err(CallError::Died(death)) => {
                tracing::warn!(%death, "a collab worker ended while it opened a document");
                Err(Opening::Died)
            }
            Err(CallError::Lost | CallError::Unavailable(_)) => {
                Err(Opening::Refused(refusals::unavailable()))
            }
        }
    }

    /// Closes a number in a worker, if the worker of that generation still holds it.
    pub(super) async fn close_number(&self, worker: usize, generation: u64, number: DocNumber) {
        let _ = self.inner.workers[worker]
            .call(Some(generation), &Request::Close(number))
            .await;
    }

    /// Chooses the worker for a new document of `tenant`: assigns each organization a
    /// sticky home worker (`H-3`) so one organization's worker death does not disconnect
    /// other organizations' sessions, spilling over around the ring when full.
    fn worker_for(&self, tenant: Tenant) -> usize {
        const MAX_TRACKED_TENANTS: usize = 4096;
        let count = self.inner.loads.len();
        if count <= 1 {
            return 0;
        }
        let preferred = {
            let mut slots = self
                .inner
                .tenant_slots
                .lock()
                .expect("tenant_slots mutex is never poisoned");
            if let Some(&slot) = slots.get(&tenant) {
                slot
            } else if slots.len() < MAX_TRACKED_TENANTS {
                let slot = self.inner.next_tenant_slot.fetch_add(1, Ordering::Relaxed) % count;
                slots.insert(tenant, slot);
                slot
            } else {
                tenant_worker(tenant, count)
            }
        };
        let cap = usize::try_from(self.inner.limits.documents).unwrap_or(usize::MAX);
        for step in 0..count {
            let index = (preferred + step) % count;
            if self.inner.loads[index].load(Ordering::Relaxed) < cap {
                return index;
            }
        }
        self.least_loaded()
    }

    /// The worker that was given the fewest documents.
    fn least_loaded(&self) -> usize {
        self.inner
            .loads
            .iter()
            .enumerate()
            .min_by_key(|(_, load)| load.load(Ordering::Relaxed))
            .map_or(0, |(index, _)| index)
    }

    /// A number no document was opened under.
    pub(super) fn next_number(&self) -> DocNumber {
        self.inner.numbers.fetch_add(1, Ordering::Relaxed)
    }
}

/// Deterministic FNV-1a worker index in `0..count` for `tenant`.
fn tenant_worker(tenant: Tenant, count: usize) -> usize {
    struct Fnv1a(u64);
    impl Hasher for Fnv1a {
        fn finish(&self) -> u64 {
            self.0
        }
        fn write(&mut self, bytes: &[u8]) {
            for &byte in bytes {
                self.0 = (self.0 ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
    }
    let mut hasher = Fnv1a(0xcbf2_9ce4_8422_2325);
    tenant.hash(&mut hasher);
    usize::try_from(hasher.finish() % u64::try_from(count).unwrap_or(1)).unwrap_or(0)
}

/// The card or the page of that identifier as `app` reaches it: the block whose body
/// has a room.
async fn card(app: &App, card_id: &str) -> Result<Block, Opening> {
    match app.store().get_block(card_id).await {
        Ok(block) if block.block_type.has_room() => Ok(block),
        Ok(_) | Err(StoreError::NotFound(_)) => Err(AppError::not_found("card").into()),
        Err(error) => Err(error.into()),
    }
}

/// The error of an answer that is not one to the request: a defect.
pub(super) fn unexpected(answer: &Answer) -> AppError {
    AppError::internal(format_args!("the worker answered with {answer:?}"))
}

/// Whether an open failed because what is stored cannot be read, which no later try
/// changes.
fn refused_for_good(failed: &Opening) -> bool {
    let Opening::Refused(error) = failed else {
        return false;
    };
    matches!(
        error.reason(),
        Some(Reason::UpdateRefused | Reason::UpdateOutOfStep)
    )
}

/// A document made from a body in a worker ([`Collab::open_from_body`]).
#[derive(Debug)]
pub(super) struct Made {
    /// The generation of the worker that holds it.
    pub(super) generation: u64,
    /// Its state.
    pub(super) state: Vec<u8>,
    /// The size of its state.
    pub(super) state_bytes: u64,
    /// How many clients it names ([`OpenDoc::clients`]).
    pub(super) clients: u64,
    /// How many changes it holds across all clients ([`OpenDoc::changes`]).
    pub(super) changes: u64,
    /// The most changes of one of its clients ([`OpenDoc::client_changes`]).
    pub(super) client_changes: u64,
    /// The most steps the Y library may take to read it once one more client has
    /// written into it ([`OpenDoc::steps_ahead`]).
    pub(super) steps_ahead: u64,
}
