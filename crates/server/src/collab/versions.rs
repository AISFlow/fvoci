//! Keeping versions of a card's body (`body_versions`): the body its card holds, each
//! time it is other than the card's newest version, at the times below. A version is
//! the body's JSON, so it is taken from the card after the document's body was written
//! there, by whoever holds the card's place.
//!
//! - When the last of those who wrote since the newest version leaves the room,
//!   whoever stays in it and wrote nothing, when the room closes, and when the server
//!   stops ([`Collab::version_as_left`], [`Collab::version_without_writers`]).
//! - While a body is edited on: `collab_version_minutes` after the first update no
//!   version holds, as the body is next written to the card
//!   ([`Collab::version_if_due`]) or, of a card that holds it already, with the next
//!   update ([`Collab::body_before_update`]). A body nobody edits on is kept when its
//!   writers leave, not by the clock.
//! - When the server starts, of the documents a server that ended left with what no
//!   version holds ([`Collab::recover`]): such a document is opened with a version
//!   due.
//! - When an editor asks ([`Collab::keep_by_hand`]): what the editors typed is written
//!   to the card first, by whoever holds its place as always, and the body kept then,
//!   unless it is the newest version or one was kept by hand within
//!   [`BY_HAND_FLOOR`]. It is kept as a version due would be: the clock counts again
//!   from the next update, and those who leave after leave nothing to keep.
//! - Around a body written whole: the store keeps the body before, so that it can be
//!   gone back to, and the body after, by its writer, within that write
//!   (`fvoci_store::Store::patch_block`).
//! - With the first update a document takes after it was opened, if the card's body
//!   is not its newest version ([`Collab::body_before_update`]): one written before
//!   versions were kept, or past its document (an import, a copy), so that what the
//!   editors change can be gone back to. The store keeps it within the write of the
//!   update's log.
//! - Around what an agent adds to a body through a personal API token
//!   ([`Collab::append`]): the body before, as of who wrote it, so that the body
//!   without the agent's words can be gone back to, and the body after, as the agent's
//!   ([`fvoci_store::Store::keep_agent_version`]).

use std::time::Duration;

use fvoci_store::{BodyBefore, ByAgent, ByHand};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::Collab;
use super::place::Held;
use super::whole::DueVersion;
use crate::app::App;
use crate::error::AppError;

/// How long after a version kept by hand no other is kept by hand. Every version is a
/// whole body, and the newest `collab_body_versions` (fifty) are kept: an editor who
/// presses the keys each time they stop typing would push the versions of the past
/// hours out within minutes. A minute keeps fifty presses to an hour of typing, and is
/// no wait for one who asks to keep what they just wrote: what they typed since is
/// kept as the server keeps it on its own (as its writers leave, or by the clock).
pub(crate) const BY_HAND_FLOOR: Duration = Duration::from_mins(1);

/// Whether the body of an open document is kept as a version of its card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Versioned {
    /// Not known: the document was opened, and took no update since.
    Unknown,
    /// The card's newest version is the document's body.
    Kept,
    /// The document took updates no version holds; the next version is due then.
    Due(Instant),
}

/// What became of what was typed in a room that one who typed it has left.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Left {
    /// A version holds it: nothing waits on who stays.
    Settled,
    /// No version holds it yet: one who stays wrote it too, or it could not be told
    /// or kept now. The room asks again when one of them leaves.
    Waiting,
}

impl Collab {
    /// Notes that an open document took an update by `user_id`: a version is due
    /// `collab_version_minutes` after the first one no version holds, and the user
    /// is put down as having written since the newest.
    pub(super) fn updated_by(&self, slot: &mut Held, user_id: &str) {
        let due = self.version_clock() + self.inner.version_interval;
        let Some(open) = slot.open.as_mut() else {
            return;
        };
        if !matches!(open.versioned, Versioned::Due(_)) {
            open.versioned = Versioned::Due(due);
        }
        if !user_id.is_empty() && !open.editors.iter().any(|editor| editor == user_id) {
            open.editors.push(user_id.to_owned());
        }
    }

    /// What the log of an update an open document took does with the body its card
    /// holds: kept as a version first if it is not known to be one, or if one is due
    /// (see the module).
    pub(super) fn body_before_update(&self, slot: &Held) -> BodyBefore {
        // Only of a card that holds all its document took.
        let unknown = slot.open.as_ref().is_some_and(|open| {
            open.versioned == Versioned::Unknown && open.written_seq >= open.log_end
        });
        if unknown || self.is_due(slot) {
            BodyBefore::Keep
        } else {
            BodyBefore::Leave
        }
    }

    /// After the body of an open document was written to its card: keeps it as a
    /// version if one is due. What fails is logged, and kept later.
    pub(super) async fn version_if_due(&self, app: &App, card_id: &str, slot: &mut Held) {
        if self.is_due(slot)
            && let Err(error) = self.keep_version(app, card_id, slot).await
        {
            tracing::error!(%error, card_id, "cannot keep a version of a card's body");
        }
    }

    /// Whether a version of the body the card of an open document holds is due now.
    /// Only of a card that holds all its document took: a version of less would take
    /// the names of those whose updates it lacks.
    fn is_due(&self, slot: &Held) -> bool {
        let now = self.version_clock();
        slot.open.as_ref().is_some_and(|open| {
            matches!(open.versioned, Versioned::Due(due) if due <= now)
                && open.written_seq >= open.log_end
        })
    }

    /// The time the versions are due by.
    pub(super) fn version_clock(&self) -> Instant {
        Instant::now() + self.version_time_passed()
    }

    /// How much later than now the versions take it to be: for the tests, which pass
    /// time ([`Collab::pass_version_time`]); none otherwise.
    fn version_time_passed(&self) -> Duration {
        #[cfg(feature = "test-util")]
        let passed = *self
            .inner
            .version_time_passed
            .lock()
            .expect("lock poisoned");
        #[cfg(not(feature = "test-util"))]
        let passed = Duration::ZERO;
        passed
    }

    /// Keeps the body of a card as a version an editor asks for now: what its editors
    /// typed is written to the card first, from its open document or from the log of
    /// one that is not open (opened aside for that, and closed after), and the body the
    /// card then holds is kept as a version by hand, unless it is the card's newest one
    /// already or a version was kept by hand within a minute (`BY_HAND_FLOOR`). What the
    /// store keeps is the body of the card, written by whoever holds its place: nothing
    /// is written to it here but what the document holds, and the document is left as it
    /// is for those who edit it.
    ///
    /// # Errors
    ///
    /// Returns a not-found error if `app` reaches no card of that identifier, a conflict
    /// if the card kept changing concurrently, `document_unavailable` if what the editors
    /// typed cannot otherwise be written first, and the store's errors.
    pub async fn keep_by_hand(&self, app: &App, card_id: &str) -> Result<ByHand, AppError> {
        let now = crate::auth::now_millis().saturating_add(millis(self.version_time_passed()));
        let since = now.saturating_sub(millis(BY_HAND_FLOOR));
        self.on_slot(
            app,
            card_id,
            move |collab, app, card_id, mut slot| async move {
                let (collab, app, card_id, slot) = (&collab, &app, card_id.as_str(), &mut slot);
                let kept = collab.keep_held_by_hand(app, card_id, slot, since).await;
                collab.retire(slot);
                kept
            },
        )
        .await
    }

    /// [`Collab::keep_by_hand`] on a held slot.
    async fn keep_held_by_hand(
        &self,
        app: &App,
        card_id: &str,
        slot: &mut Held,
        since: i64,
    ) -> Result<ByHand, AppError> {
        // Without a version due being kept on its own first: the one kept is by hand.
        self.write_pending(app, card_id, slot, DueVersion::Skip)
            .await?;
        let by_hand = app
            .store()
            .keep_body_version_by_hand(card_id, since)
            .await?
            .ok_or_else(|| AppError::not_found("card"))?;
        if !matches!(by_hand, ByHand::Recent(_)) {
            kept(slot);
        }
        Ok(by_hand)
    }

    /// Keeps the body of a held card as a version before an agent writes in it, as of
    /// who wrote it: what its editors typed is written to the card first.
    pub(super) async fn keep_before_agent(
        &self,
        app: &App,
        card_id: &str,
        slot: &mut Held,
    ) -> Result<(), AppError> {
        self.write_pending(app, card_id, slot, DueVersion::Skip)
            .await?;
        app.store().keep_body_version(card_id).await?;
        kept(slot);
        Ok(())
    }

    /// Writes to a held card what an agent of `user_id` wrote in its document, and keeps
    /// the body as the agent's version.
    pub(super) async fn keep_agent_version(
        &self,
        app: &App,
        card_id: &str,
        slot: &mut Held,
        user_id: &str,
        agent: ByAgent<'_>,
    ) -> Result<(), AppError> {
        self.write_pending(app, card_id, slot, DueVersion::Skip)
            .await?;
        app.store()
            .keep_agent_version(card_id, user_id, agent)
            .await?;
        kept(slot);
        Ok(())
    }

    /// Has the versions take it to be `by` later from now on: for the tests of what
    /// is kept as time passes.
    #[cfg(feature = "test-util")]
    #[doc(hidden)]
    pub fn pass_version_time(&self, by: std::time::Duration) {
        *self
            .inner
            .version_time_passed
            .lock()
            .expect("lock poisoned") += by;
    }

    /// As the last of those who wrote leaves a room, and after the document's body was
    /// written to its card: keeps it as a version if the document took updates no
    /// version holds. What fails is logged: who wrote stays put down, and the version
    /// is kept later.
    pub(super) async fn version_as_left(&self, app: &App, card_id: &str, slot: &mut Held) {
        if unkept(slot)
            && let Err(error) = self.keep_version(app, card_id, slot).await
        {
            tracing::error!(%error, card_id, "cannot keep a version of a card's body");
        }
    }

    /// For a room that one who typed has left, or writes in no more: writes the body
    /// of its open document to the card and keeps it as a version, if the document
    /// took updates no version holds and none of them is by one of `writing`, the
    /// users still in the room to write. What is not kept, by a store that failed
    /// (which is logged) or of a document that is not open (whoever reaches it next
    /// opens it with a version due), is kept when one of them leaves, and at the
    /// latest when the room closes.
    pub(super) async fn version_without_writers(
        &self,
        app: &App,
        card_id: &str,
        gone: CancellationToken,
        writing: &[String],
    ) -> Left {
        let writing = writing.to_vec();
        let left = self
            .on_slot_while(
                app,
                card_id,
                Some(gone),
                move |collab, app, card_id, mut slot| async move {
                    let Some(open) = slot.open.as_ref() else {
                        collab.retire(&mut slot);
                        return Ok(Left::Waiting);
                    };
                    if !unkept(&slot) {
                        return Ok(Left::Settled);
                    }
                    if open.editors.iter().any(|editor| writing.contains(editor)) {
                        return Ok(Left::Waiting);
                    }
                    let kept = match collab.write_now(&app, &card_id, &mut slot).await {
                        // Not what the write kept itself, a version being due.
                        Ok(()) if unkept(&slot) => {
                            collab.keep_version(&app, &card_id, &mut slot).await
                        }
                        written => written,
                    };
                    collab.retire(&mut slot);
                    kept.map(|()| Left::Settled)
                },
            )
            .await;
        match left {
            Ok(left) => left,
            Err(error) if error.is_not_found() => Left::Settled,
            Err(error) => {
                tracing::error!(%error, card_id, "cannot keep a version of a body its writers left");
                Left::Waiting
            }
        }
    }

    /// Keeps the body a card holds as a version, if it is not the card's newest. With
    /// an open document whose body the card holds, the document's body is kept from
    /// then on.
    async fn keep_version(
        &self,
        app: &App,
        card_id: &str,
        slot: &mut Held,
    ) -> Result<(), AppError> {
        app.store().keep_body_version(card_id).await?;
        kept(slot);
        Ok(())
    }
}

/// A span in whole milliseconds, as the times of versions are counted.
fn millis(span: Duration) -> i64 {
    i64::try_from(span.as_millis()).unwrap_or(i64::MAX)
}

/// Whether the open document of a held slot took updates no version holds.
fn unkept(slot: &Held) -> bool {
    slot.open
        .as_ref()
        .is_some_and(|open| matches!(open.versioned, Versioned::Due(_)))
}

/// Notes on a held slot that its card's body was kept as a version, or is its newest
/// already: nobody is put down as having written since, and an open document whose
/// body the card holds is kept.
pub(super) fn kept(slot: &mut Held) {
    if let Some(open) = slot.open.as_mut() {
        open.editors.clear();
        if open.written_seq >= open.log_end {
            open.versioned = Versioned::Kept;
        }
    }
}
