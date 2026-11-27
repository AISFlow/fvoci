//! Taking an update, and keeping a document and its card in step afterwards: writing
//! the body to the card, folding the log into the state, and making a document that
//! grew too large again from its body.

use fvoci_model::Body;
use fvoci_store::{BodyBefore, ByAgent, Projection, StoreError, body_digest};
use serde_json::Value;
use tokio::time::{Instant, sleep_until};

use super::open::{Made, unexpected};
use super::place::{AskedFor, Held, OpenDoc, Opening, Unanswered, died_over};
use super::senders::SenderOf;
use super::versions::Versioned;
use super::{Collab, DocChange, FOLD_AFTER, REMAKE_RETRY, Taken};
use crate::app::App;
use crate::collab_worker::wire::{Answer, Request};
use crate::error::{AppError, Reason};

/// How many times the body of a document is written to a card that keeps changing
/// meanwhile (its title, its fields) before the write is given up for now.
const PROJECTION_TRIES: u32 = 3;

/// Whether [`Collab::remake_and_tell`] should emit [`DocChange::Closed`] when a
/// required remake (`must_remake`) cannot run or fails and forgets the open slot.
/// Callers with an active room (`keep_up`) tell the room so its watcher retries
/// opening and remaking; callers that already run inside `ensure_open` or closing
/// stay quiet so the watcher does not spin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OnForget {
    Quiet,
    TellClosed,
}

/// Who sent an update: the user, and the agent they sent it through, if one.
#[derive(Debug, Clone, Copy)]
pub(super) struct Sent<'a> {
    pub(super) user_id: &'a str,
    pub(super) agent: Option<ByAgent<'a>>,
}

/// An update as it was sent: its bytes, and who sent it.
#[derive(Debug, Clone, Copy)]
struct Sending<'a> {
    update: &'a [u8],
    by: Sent<'a>,
}

impl Collab {
    /// Takes an update into the document of a held slot and logs it with what a repair
    /// deleted; a document whose log could not be written is closed, so that the next
    /// request opens it as it is stored. An update that ends the worker is put down to
    /// its sender ([`super::senders`]).
    pub(super) async fn take_in(
        &self,
        app: &App,
        card_id: &str,
        slot: &mut Held,
        epoch: i64,
        user_id: &str,
        update: &[u8],
    ) -> Result<Taken, Unanswered> {
        #[cfg(feature = "test-util")]
        let stall = self.stall_for(app, card_id);
        let request = |doc| {
            #[cfg(feature = "test-util")]
            if let Some(millis) = stall {
                self.stop_at_document(doc, millis);
            }
            Request::Take {
                doc,
                update: update.to_vec(),
            }
        };
        self.at_epoch(app, card_id, slot, epoch).await?;
        let answer = match self
            .ask_for(
                app,
                card_id,
                slot,
                request,
                AskedFor::Sender(user_id.into()),
            )
            .await
        {
            Err(refused @ (Unanswered::Refused(_) | Unanswered::TooManySteps(_))) => {
                self.remake_refused(app, card_id, slot, &refused).await;
                return Err(refused.into_error().into());
            }
            answered => answered?,
        };
        let by = Sent {
            user_id,
            agent: None,
        };
        self.record_taken(app, card_id, slot, epoch, Sending { update, by }, answer)
            .await
    }

    /// Adds nodes (`nodes`, the JSON of a list of a body's top-level nodes) after the
    /// last of the document of a held slot, as `by` through `sender` (the key its
    /// deaths are put down to, [`super::senders`]), and logs the update that added them
    /// as an editor's is logged, an agent's as sent through its token. Returns the
    /// update, for the editors.
    pub(super) async fn append_in(
        &self,
        app: &App,
        card_id: &str,
        slot: &mut Held,
        by: Sent<'_>,
        sender: &str,
        nodes: &[u8],
    ) -> Result<(Vec<u8>, i64, Taken), Unanswered> {
        self.ensure_open(app, card_id, slot).await?;
        let epoch = slot.open.as_ref().expect("the document is open").epoch;
        let request = |doc| Request::Append {
            doc,
            nodes: nodes.to_vec(),
        };
        let answer = match self
            .ask_for(
                app,
                card_id,
                slot,
                request,
                AskedFor::Sender(SenderOf {
                    key: sender,
                    owner: by.user_id,
                }),
            )
            .await
        {
            Err(refused @ (Unanswered::Refused(_) | Unanswered::TooManySteps(_))) => {
                self.remake_refused(app, card_id, slot, &refused).await;
                return Err(refused.into_error().into());
            }
            answered => answered?,
        };
        let Answer::Appended { update, taken } = answer else {
            return Err(unexpected(&answer).into());
        };
        let taken = self
            .record_taken(
                app,
                card_id,
                slot,
                epoch,
                Sending {
                    update: &update,
                    by,
                },
                *taken,
            )
            .await?;
        Ok((update, epoch, taken))
    }

    /// Keeps what the worker answered (`answer`) of an update it took (`sending`) into
    /// the document of a held slot at `epoch`: logs the update with what a repair
    /// deleted, and the document's numbers as they are now.
    async fn record_taken(
        &self,
        app: &App,
        card_id: &str,
        slot: &mut Held,
        epoch: i64,
        sending: Sending<'_>,
        answer: Answer,
    ) -> Result<Taken, Unanswered> {
        let user_id = sending.by.user_id;
        let Answer::Taken {
            repair,
            state_bytes,
            clients,
            changes,
            client_changes,
            steps_ahead,
            files,
            remake,
            fold,
        } = answer
        else {
            return Err(unexpected(&answer).into());
        };
        if super::place::still_at_epoch(slot, epoch).is_err() {
            // Made again while the update was asked (a worker that ended, and a card
            // written past the document): the new document took an update made against
            // the one before, which is not logged, so it is closed.
            self.forget(slot).await;
            return Err(super::refusals::out_of_step().into());
        }
        let seqs = self
            .log_taken(app, card_id, slot, sending, repair.as_deref(), &files)
            .await?;
        let open = slot.open.as_mut().expect("the document is open");
        let now = Instant::now();
        open.log_end = seqs.last().copied().unwrap_or(open.log_end);
        open.state_bytes = state_bytes;
        open.clients = clients;
        open.changes = changes;
        open.client_changes = client_changes;
        open.steps_ahead = steps_ahead;
        open.must_remake |= remake;
        open.wants_fold |= fold;
        user_id.clone_into(&mut open.last_writer);
        open.last_update = now;
        open.first_unwritten.get_or_insert(now);
        self.schedule_write(app, card_id, open);
        self.updated_by(slot, user_id);
        let remade = self.keep_up(app, card_id, slot).await;
        Ok(Taken {
            seq: seqs.first().copied().unwrap_or_default(),
            repair,
            remade,
        })
    }

    /// Logs an update that the worker took (`sending`), with its repair if any, noting
    /// its writer for versions and holding any new files it names.
    async fn log_taken(
        &self,
        app: &App,
        card_id: &str,
        slot: &mut Held,
        sending: Sending<'_>,
        repair: Option<&[u8]>,
        files: &[String],
    ) -> Result<Vec<i64>, AppError> {
        let Sending { update, by } = sending;
        let user_id = by.user_id;
        let mut entries: Vec<(&[u8], &str)> = vec![(update, user_id)];
        if let Some(repair) = repair {
            entries.push((repair, ""));
        }
        // Put down as having written since the newest version, unless they are: a
        // body kept first is by those put down before.
        let body_before = self.body_before_update(slot);
        let known = user_id.is_empty()
            || body_before == BodyBefore::Leave
                && slot
                    .open
                    .as_ref()
                    .is_some_and(|open| open.editors.iter().any(|editor| editor == user_id));
        let new_editors: &[&str] = if known { &[] } else { &[user_id] };
        let as_read = slot.open.as_ref().expect("the document is open").as_read();
        #[cfg(feature = "test-util")]
        self.paused_before_log().await;
        // An agent's update is logged as sent through its token: what it typed is the
        // agent's when the body is written and kept, whoever writes it.
        let store = match by.agent {
            Some(agent) => app.store().by_agent(agent),
            None => app.store().clone(),
        };
        let logged = store
            .append_body_updates(card_id, &entries, as_read, files, new_editors, body_before)
            .await;
        let seqs = match logged {
            Ok(seqs) => seqs,
            Err(error) => return Err(self.not_logged(app, card_id, slot, error).await),
        };
        if body_before == BodyBefore::Keep {
            // The store kept the body the card held, by those put down before.
            super::versions::kept(slot);
        }
        Ok(seqs)
    }

    /// Opens again, as it is stored and at once, a document that holds an update its
    /// worker took and the store did not log, so that what others typed is still
    /// written; what the update is refused with. So is a document another process
    /// wrote since it was read here, and its room is told: what its editors hold
    /// lacks what that one wrote.
    async fn not_logged(
        &self,
        app: &App,
        card_id: &str,
        slot: &mut Held,
        error: StoreError,
    ) -> AppError {
        self.forget(slot).await;
        if let Err(reopened) = self.ensure_open(app, card_id, slot).await {
            tracing::warn!(%reopened, card_id, "cannot open a document again after an update that was not logged");
        }
        match error {
            StoreError::InvalidBlock { .. } => AppError::bad_request(
                "the update names a file (of an image or a file block) the board or wiki \
                 does not have",
            )
            .with_reason(Reason::UpdateRefused),
            StoreError::DocumentWritten { .. } => {
                self.tell(app, card_id, DocChange::Reopened);
                error.into()
            }
            other => other.into(),
        }
    }

    /// Makes the document again from its body if its state is near its limit, it
    /// names many clients or one more client may leave it with many steps to read
    /// ([`OpenDoc::crowded`]), and folds its log into its state if it is long, after
    /// writing the body to its card ([`Collab::write_now`]); whether it was made
    /// again, by itself or because its card was written past it. The
    /// update before is logged whatever happens here: what fails is logged, and tried
    /// again after a later update.
    async fn keep_up(&self, app: &App, card_id: &str, slot: &mut Held) -> bool {
        let Some(open) = slot.open.as_ref() else {
            return false;
        };
        let limit = self.inner.limits.state_bytes;
        if (open.grown_near_limit(limit) || open.crowded())
            && self
                .remake_and_tell(app, card_id, slot, OnForget::TellClosed)
                .await
        {
            return true;
        }
        let long = |slot: &Held| {
            slot.open.as_ref().is_some_and(|open| {
                !open.must_remake
                    && (open.wants_fold || open.log_end - open.state_seq >= FOLD_AFTER)
            })
        };
        // Not by a process that lost the lock of the server, as at closing.
        if !app.holds_server_lock() || !long(slot) {
            return false;
        }
        // The body first: the log says who sent which update, which is how a name in
        // the body is known to be somebody's doing, and a fold takes that away. A
        // body that cannot be written now is written later, by its last typist's
        // name alone.
        match self.write_or_replace(app, card_id, slot).await {
            Ok(true) => return true,
            Ok(false) => self.version_if_due(app, card_id, slot).await,
            Err(error) => {
                tracing::warn!(%error, card_id, "cannot write a card's body before its log is folded");
            }
        }
        if long(slot) {
            match self.fold(app, card_id, slot, OnForget::TellClosed).await {
                Ok(remade) => return remade,
                Err(error) => {
                    tracing::error!(%error, card_id, "cannot fold a document's log into its state");
                }
            }
        }
        false
    }

    /// Makes a document whose update was refused for the size of its state again from
    /// its body, if it grew since it was made (what was deleted is in its state), and
    /// one whose update was refused for the steps its reading would take, unless this
    /// server made it and it is as it was made (the steps are of what was written into
    /// it, and a body's own document has none: such an update may be the paragraph of
    /// an editor whose client is read before thousands of others its changes wait
    /// for), so that the editors who load it again can go on; whether it was made
    /// again. A document opened from what was stored is not known to be as it was
    /// made, and is made again for such an update once.
    pub(super) async fn remake_refused(
        &self,
        app: &App,
        card_id: &str,
        slot: &mut Held,
        refused: &Unanswered,
    ) -> bool {
        let limit = self.inner.limits.state_bytes;
        let wanted = slot.open.as_ref().is_some_and(|open| match refused {
            Unanswered::TooManySteps(_) => open.state_bytes > open.made_bytes,
            Unanswered::Refused(error) => {
                error.reason() == Some(Reason::DocumentTooLarge) && open.remake_on_too_large(limit)
            }
            Unanswered::Died(_) => false,
        });
        wanted
            && self
                .remake_and_tell(app, card_id, slot, OnForget::Quiet)
                .await
    }

    /// Makes the document again from its body and tells the room, unless that failed
    /// a little before (it is tried again after [`REMAKE_RETRY`]); whether it was made
    /// again. A document that must be remade ([`OpenDoc::must_remake`]) is never kept
    /// open or folded if the remake cannot run or fails: it is closed (`forget`), and
    /// its logged updates stay in the store for the next opening or recovery. When
    /// `on_forget` is [`OnForget::TellClosed`], forgetting a `must_remake` document
    /// tells the room ([`DocChange::Closed`]) so its watcher retries opening and
    /// remaking instead of leaving the logged update unprojected.
    pub(super) async fn remake_and_tell(
        &self,
        app: &App,
        card_id: &str,
        slot: &mut Held,
        on_forget: OnForget,
    ) -> bool {
        let must_remake = slot.open.as_ref().is_some_and(|open| open.must_remake);
        // A process that lost the lock of the server would be refused by `remake_from`;
        // it does not try, and so logs no failure.
        if !app.holds_server_lock()
            || (!must_remake
                && slot
                    .remake_failed_at
                    .is_some_and(|at| at.elapsed() < REMAKE_RETRY))
        {
            if must_remake {
                self.forget(slot).await;
                if on_forget == OnForget::TellClosed {
                    self.tell(app, card_id, DocChange::Closed);
                }
            }
            return false;
        }
        match self.remake(app, card_id, slot).await {
            Ok(replaced) => {
                // A card written past the document made it again from that, and the
                // room was told so.
                if !replaced {
                    self.tell(app, card_id, DocChange::Remade);
                }
                slot.remake_failed_at = None;
                true
            }
            Err(error) => {
                tracing::error!(%error, card_id, "cannot make a document that grew large again");
                if must_remake {
                    self.forget(slot).await;
                    if on_forget == OnForget::TellClosed {
                        self.tell(app, card_id, DocChange::Closed);
                    }
                }
                slot.remake_failed_at = Some(Instant::now());
                false
            }
        }
    }

    /// Writes, versions, remakes if worn, folds if settled, and forgets a held slot as
    /// it closes. If the slot was already forgotten (for instance after a transient
    /// `must_remake` failure) while its log still has updates unwritten to the card,
    /// opens it aside first so closing does not strand those updates.
    pub(super) async fn close_held(
        &self,
        app: &App,
        card_id: &str,
        slot: &mut Held,
    ) -> Result<(), AppError> {
        let opened = if slot.open.is_none()
            && app
                .store()
                .has_unwritten_body(card_id)
                .await
                .unwrap_or(false)
        {
            self.open_aside(app, card_id, slot).await
        } else {
            Ok(())
        };
        let written = match opened {
            Ok(()) => self.write_now(app, card_id, slot).await,
            Err(error) => Err(error),
        };
        if written.is_ok() {
            self.version_as_left(app, card_id, slot).await;
            self.remake_worn(app, card_id, slot).await;
        }
        self.fold_as_it_closes(app, card_id, slot).await;
        self.forget(slot).await;
        written
    }

    /// Makes the open document of a slot again from its body if it is worn
    /// ([`OpenDoc::worn`]), for a document that is closed next with its body on its
    /// card and nobody in its room: nobody holds the document it was, so nothing an
    /// editor wrote is lost and nobody loads it anew, and the next to open it finds
    /// one far from the steps it may take to read, whichever client is read first.
    /// Its versions and what else its card holds are as they were. One that is not
    /// worn is folded by what closes it; one that could not be made again is closed
    /// as it is stored, its log taken again when it is next opened.
    ///
    /// A connection that dropped and comes back with the document it held, and with
    /// changes the server did not take, finds another making of it and loads that:
    /// those changes are lost, as they are after any making again.
    pub(super) async fn remake_worn(&self, app: &App, card_id: &str, slot: &mut Held) {
        if slot.open.as_ref().is_some_and(OpenDoc::worn) {
            self.remake_and_tell(app, card_id, slot, OnForget::Quiet)
                .await;
        }
    }

    /// Folds the log of the open document of a slot into its state, if it holds
    /// updates the state does not, for a document that is closed next: what is stored
    /// of it is its state alone. A fold that fails is logged; the log stays, and is
    /// taken again when the document is opened. Not by a process that lost the lock of
    /// the server (it is going down, and the next server may write the same document).
    pub(super) async fn fold_as_it_closes(&self, app: &App, card_id: &str, slot: &mut Held) {
        if app.holds_server_lock()
            && slot
                .open
                .as_ref()
                .is_some_and(|open| !open.must_remake && open.state_seq < open.log_end)
            && let Err(error) = self.fold(app, card_id, slot, OnForget::Quiet).await
        {
            tracing::error!(%error, card_id, "cannot fold a document's log into its state as it closes");
        }
    }

    /// Stores the document's settled state in place of its log; returns `Ok(true)` if
    /// settling failed and the document was remade from its body instead. A document
    /// another process wrote since it was read here is closed: its numbers are the
    /// store's no more, and it is read again when it is next asked for.
    async fn fold(
        &self,
        app: &App,
        card_id: &str,
        slot: &mut Held,
        on_forget: OnForget,
    ) -> Result<bool, AppError> {
        if slot.open.as_ref().is_some_and(|open| open.must_remake) {
            return Ok(false);
        }
        let state = match self.ask(app, card_id, slot, Request::State).await? {
            Answer::Bytes(state) => state,
            Answer::Remake => {
                if let Some(open) = slot.open.as_mut() {
                    open.must_remake = true;
                }
                return if self.remake_and_tell(app, card_id, slot, on_forget).await {
                    Ok(true)
                } else {
                    Err(super::refusals::unavailable())
                };
            }
            other => return Err(unexpected(&other)),
        };
        let open = slot.open.as_mut().expect("the document is open");
        let through = open.log_end;
        let folded = app
            .store()
            .fold_body_doc(card_id, &state, through, open.as_read())
            .await;
        match folded {
            Ok(()) => {
                open.state_seq = through;
                open.wants_fold = false;
                Ok(false)
            }
            Err(error @ StoreError::DocumentWritten { .. }) => {
                self.written_past(app, card_id, slot).await;
                Err(error.into())
            }
            Err(error) => Err(error.into()),
        }
    }

    /// Marks the open document of `card_id` in its worker so its next `Request::State`
    /// settle check fails and returns [`Answer::Remake`], for testing lossless settle
    /// failure recovery at fold, close, and open.
    #[cfg(feature = "test-util")]
    #[doc(hidden)]
    pub async fn fail_next_settle(&self, app: &App, card_id: &str) -> Result<(), AppError> {
        self.on_slot(
            app,
            card_id,
            move |collab, app, card_id, mut slot| async move {
                let (collab, app, card_id, slot) = (&collab, &app, card_id.as_str(), &mut slot);
                match collab
                    .ask(app, card_id, slot, Request::FailNextSettle)
                    .await?
                {
                    Answer::Done => Ok(()),
                    other => Err(unexpected(&other)),
                }
            },
        )
        .await
    }

    /// Closes a document another process wrote since it was read here, and tells its
    /// room ([`DocChange::Reopened`]): its numbers are the store's no more, and its
    /// editors may lack what the other wrote. The next request, the room's among them,
    /// reads it again as it is stored.
    async fn written_past(&self, app: &App, card_id: &str, slot: &mut Held) {
        self.forget(slot).await;
        self.tell(app, card_id, DocChange::Reopened);
    }

    /// Writes the document's body to its card, and makes the document again from it;
    /// whether the card was written past the document, and the document was made again
    /// from that instead.
    async fn remake(&self, app: &App, card_id: &str, slot: &mut Held) -> Result<bool, AppError> {
        if self.write_or_replace(app, card_id, slot).await? {
            return Ok(true);
        }
        self.version_if_due(app, card_id, slot).await;
        let body = app.store().get_block(card_id).await?.body;
        self.remake_held(app, card_id, slot, body.as_ref()).await?;
        Ok(false)
    }

    /// Makes the open document of a held slot again from `body`, the body its card was
    /// given whole past the document, and tells the room ([`DocChange::Replaced`]).
    pub(super) async fn replaced(
        &self,
        app: &App,
        card_id: &str,
        slot: &mut Held,
        body: Option<&Body>,
    ) -> Result<(), AppError> {
        self.remake_held(app, card_id, slot, body).await?;
        // The body the card was written is no version until it is kept as one, and
        // who is put down as having written since the newest may be so no more: a
        // version kept with the write names them.
        let due = Versioned::Due(self.version_clock());
        if let Some(open) = slot.open.as_mut() {
            open.versioned = due;
            open.editors.clear();
        }
        self.tell(app, card_id, DocChange::Replaced);
        Ok(())
    }

    /// Makes the open document of a held slot again from `body`; a document that could
    /// not be made again is closed (the store holds it as it was, or as it was made).
    async fn remake_held(
        &self,
        app: &App,
        card_id: &str,
        slot: &mut Held,
        body: Option<&Body>,
    ) -> Result<(), AppError> {
        let Some(open) = slot.open.as_mut() else {
            return Ok(());
        };
        match self.remake_from(app, card_id, open, body).await {
            Ok(()) => Ok(()),
            Err(Opening::Refused(error)) => {
                self.forget(slot).await;
                Err(error)
            }
            Err(Opening::WrittenSince(error)) => {
                self.written_past(app, card_id, slot).await;
                Err(error)
            }
            Err(Opening::Died) => {
                self.lost(slot);
                died_over(slot);
                Err(super::refusals::unavailable())
            }
        }
    }

    /// Makes an open document again from `body`, the body its card holds, in the same
    /// worker, and stores it in place of the one before and its log. The one before is
    /// closed first, so that a worker that holds as many documents as it may has room
    /// for it; whatever fails, `open` is no document to go on with: the caller closes
    /// it (its number is the new one's once that is open). Refused to a process that
    /// lost the lock of the server: making a document again empties its log, as a fold
    /// does, and the log may hold what the server that took the lock wrote since. A
    /// process learns that it lost the lock a while after it did
    /// (`app/server_lock.rs`), so the store is told what was read of the document,
    /// and refuses one that was written since.
    pub(super) async fn remake_from(
        &self,
        app: &App,
        card_id: &str,
        open: &mut OpenDoc,
        body: Option<&Body>,
    ) -> Result<(), Opening> {
        if !app.holds_server_lock() {
            return Err(Opening::Refused(super::refusals::unavailable()));
        }
        self.close_number(open.worker, open.generation, open.number)
            .await;
        let number = self.next_number();
        let Made {
            generation,
            state,
            state_bytes,
            clients,
            changes,
            client_changes,
            steps_ahead,
        } = self
            .open_from_body(open.worker, number, body, open.share())
            .await?;
        open.generation = generation;
        open.number = number;
        let digest = body_digest(body);
        open.epoch = app
            .store()
            .remake_body_doc(card_id, &state, &digest, open.as_read())
            .await?;
        open.state_seq = open.log_end;
        open.written_seq = open.log_end;
        open.body_digest = digest;
        open.state_bytes = state_bytes;
        open.made_bytes = state_bytes;
        open.clients = clients;
        open.changes = changes;
        open.made_changes = changes;
        open.client_changes = client_changes;
        open.steps_ahead = steps_ahead;
        open.must_remake = false;
        open.wants_fold = false;
        open.first_unwritten = None;
        Ok(())
    }

    /// Writes the body of the document to its card, if the card does not hold it yet:
    /// as the last writer changed it, with what any change of a body brings (its text,
    /// task counts, the files it shows), and told to the card's listeners and the
    /// webhooks. A body that is the card's already is only marked as written.
    ///
    /// A card whose body is no longer the one the document last matched was written
    /// past the document, whole: that body is not written over; the document is made
    /// again from it ([`DocChange::Replaced`]), and the updates the card did not hold
    /// are gone with the document before.
    ///
    /// The body the card then holds is kept as a version if one is due
    /// ([`Collab::version_if_due`]).
    pub(super) async fn write_now(
        &self,
        app: &App,
        card_id: &str,
        slot: &mut Held,
    ) -> Result<(), AppError> {
        self.write_or_replace(app, card_id, slot).await?;
        self.version_if_due(app, card_id, slot).await;
        Ok(())
    }

    /// As [`Collab::write_now`], keeping no version; whether the card was written past
    /// the document, and the document was made again from it.
    pub(super) async fn write_or_replace(
        &self,
        app: &App,
        card_id: &str,
        slot: &mut Held,
    ) -> Result<bool, AppError> {
        let Some(open) = slot.open.as_ref() else {
            return Ok(false);
        };
        if open.written_seq >= open.log_end {
            return Ok(false);
        }
        let held = app.store().get_block(card_id).await?;
        if body_digest(held.body.as_ref()) != open.body_digest {
            self.replaced(app, card_id, slot, held.body.as_ref())
                .await?;
            return Ok(true);
        }
        let json = match self.ask(app, card_id, slot, Request::Body).await? {
            Answer::Body(json) => json,
            other => return Err(unexpected(&other)),
        };
        let body = match json {
            None => None,
            Some(json) => {
                let value: Value = serde_json::from_slice(&json).map_err(AppError::internal)?;
                // The worker's body is held to the allowlist again here, by the server.
                Body::parse(value).map_err(AppError::internal)?
            }
        };
        let digest = body_digest(body.as_ref());
        let open = slot.open.as_ref().expect("the document is open");
        let through = open.log_end;
        let changed = digest != open.body_digest;
        let last_writer = open.last_writer.clone();
        let as_read = open.as_read();
        let mut read_at = held.updated_at;
        #[cfg(feature = "test-util")]
        self.paused_before_projection().await;
        let mut tries = 0;
        loop {
            let projected = app
                .store()
                .project_body(
                    card_id,
                    if changed {
                        Projection::Written {
                            body: body.clone(),
                            read_at,
                        }
                    } else {
                        Projection::Held
                    },
                    through,
                    &digest,
                    &last_writer,
                    as_read,
                )
                .await;
            if let Err(error @ StoreError::DocumentWritten { .. }) = projected {
                self.written_past(app, card_id, slot).await;
                return Err(error.into());
            }
            let Err(StoreError::Stale { .. }) = projected else {
                projected?;
                break;
            };
            // The card changed since it was read: its body was written past the
            // document, or something else of it was (a title), and the body is written
            // again with the card as it is now.
            let held = app.store().get_block(card_id).await?;
            let open = slot.open.as_ref().expect("the document is open");
            if body_digest(held.body.as_ref()) != open.body_digest {
                self.replaced(app, card_id, slot, held.body.as_ref())
                    .await?;
                return Ok(true);
            }
            tries += 1;
            if tries >= PROJECTION_TRIES {
                return Err(AppError::conflict(
                    "the card kept changing while the body of its document was written to it",
                ));
            }
            read_at = held.updated_at;
        }
        let open = slot.open.as_mut().expect("the document is open");
        open.written_seq = through;
        open.body_digest = digest;
        open.first_unwritten = None;
        if changed {
            announce(app, card_id).await;
        }
        Ok(false)
    }

    /// Has a task write the body of the document when it is due ([`OpenDoc::write_at`]),
    /// unless one waits to already.
    pub(super) fn schedule_write(&self, app: &App, card_id: &str, open: &mut OpenDoc) {
        if open.writing_scheduled {
            return;
        }
        #[cfg(feature = "test-util")]
        if self
            .inner
            .due_writes_left
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            return;
        }
        open.writing_scheduled = true;
        let collab = self.clone();
        let app_of_task = app.clone();
        let card_id = card_id.to_owned();
        app.spawn_background(async move {
            collab.write_when_due(&app_of_task, &card_id).await;
        });
    }

    /// Waits until the body of a document is due to be written, and writes it.
    async fn write_when_due(&self, app: &App, card_id: &str) {
        loop {
            let due = {
                let Some(container_id) = self.existing_slot_container_id(app, card_id) else {
                    return;
                };
                let _gate = self.reading(app.store().tenant(), &container_id).await;
                let Some(mut slot) = self.existing_slot(app, card_id).await else {
                    return;
                };
                let Some(open) = slot.open.as_mut() else {
                    return;
                };
                let Some(due) = open.write_at() else {
                    open.writing_scheduled = false;
                    return;
                };
                due
            };
            sleep_until(due).await;
            let Some(container_id) = self.existing_slot_container_id(app, card_id) else {
                return;
            };
            let _gate = self.reading(app.store().tenant(), &container_id).await;
            let Some(mut slot) = self.existing_slot(app, card_id).await else {
                return;
            };
            if slot
                .open
                .as_ref()
                .and_then(OpenDoc::write_at)
                .is_some_and(|due| due > Instant::now())
            {
                continue;
            }
            if let Err(error) = self.write_now(app, card_id, &mut slot).await {
                tracing::error!(%error, card_id, "cannot write a card's body from its document");
                if error.is_not_found() {
                    // The card went (to the trash): its document waits in the store
                    // for the card to come back.
                    self.forget(&mut slot).await;
                    self.retire(&mut slot);
                    return;
                }
                // Tried again a little later, whether or not anyone types.
                if let Some(open) = slot.open.as_mut() {
                    open.retry_at = Some(Instant::now() + super::WRITE_QUIET);
                }
                continue;
            }
            if let Some(open) = slot.open.as_mut() {
                open.retry_at = None;
                open.writing_scheduled = false;
            }
            return;
        }
    }
}

/// Tells the card's listeners and the webhooks that its body changed, in the
/// background, as a patch of the card does.
async fn announce(app: &App, card_id: &str) {
    let announced = async {
        let block = app.store().get_block(card_id).await?;
        let container = app.store().get_container(&block.container_id).await?;
        Ok::<_, fvoci_store::StoreError>((block, container))
    };
    let (block, container) = match announced.await {
        Ok(read) => read,
        Err(error) => {
            tracing::error!(%error, card_id, "cannot read a card whose body was written to tell of it");
            return;
        }
    };
    let app_of_task = app.clone();
    app.spawn_background(async move {
        app_of_task
            .ws()
            .broadcast_block_change(&container.workspace_id, &block)
            .await;
        app_of_task.tell_webhooks(&block);
    });
}
