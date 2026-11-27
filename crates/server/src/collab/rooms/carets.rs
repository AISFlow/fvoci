//! The carets of a room: where each member who writes is in the document, told to
//! the room's others and to nobody else, and kept nowhere but here.
//!
//! A caret is its member's: the connection that sent it, in the room it sent it to.
//! Whose it is, the number the others know it under and which telling of it this is
//! are the server's to say; of what the client sent only the place is sent on
//! ([`caret::said`]). A member who reads has none. The others are told it is gone
//! when its member withdraws it, reads from now on, or is no member any more
//! ([`Turn::take_out`]); the document made again leaves none, as every member then
//! holds another document.
//!
//! A member an immediate check holds is told nothing of the carets, and once the
//! check keeps them, where every caret is now and which went meanwhile.

use std::time::Duration;

use bytes::Bytes;
use tokio::time::Instant;

use super::super::Collab;
use super::super::caret::{self, Said};
use super::{Asked, Connection, Member, Room, Turn};
use crate::permissions::DocMode;

/// How long after a caret was last heard of it is still told to who comes in: its
/// client says it again every fifteen seconds, and who holds it lets it go after
/// thirty without a word.
const TOLD_FOR: Duration = Duration::from_secs(30);

/// How many carets that went a held member is told of at most, once let go: the
/// rest they let go by themselves ([`TOLD_FOR`]).
const MOST_MISSED: usize = 64;

/// A number for a caret: drawn at random, so that it says nothing of how many carets
/// the server has told (in whatever room or organization), among 2^52 past what the
/// client of a Y document has for a number (32 bits): none is the number a receiver's
/// own editor holds its own state under, two of one room are not the same but by a
/// chance nobody meets, and a browser counts with it.
fn number() -> u64 {
    let mut bytes = [0; 8];
    getrandom::fill(&mut bytes).expect("the operating system supplies random bytes");
    (u64::from_le_bytes(bytes) >> 12) | (1 << 32)
}

/// The caret of a member.
#[derive(Debug)]
pub(super) struct Caret {
    /// What the others were last told of it.
    told: Telling,
    /// Where it is, if it is shown.
    shown: Option<Shown>,
}

/// One telling of a caret: the number the others know the caret under, the server's
/// ([`number`]), and which telling of it this is. A later one takes an earlier one's
/// place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Telling {
    number: u64,
    clock: u64,
}

struct Shown {
    /// The awareness state the others are told ([`caret::state_of`]).
    state: String,
    heard: Instant,
}

impl std::fmt::Debug for Shown {
    /// Without where the caret is: nothing of a caret is written anywhere, be it by
    /// a log line of what holds it.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Shown")
            .field("heard", &self.heard)
            .finish_non_exhaustive()
    }
}

/// The carets that went while a check held a member: the telling that said so of each.
pub(super) type CaretsMissed = Vec<Telling>;

impl Turn {
    /// Takes a member out of the room. The others are told that its caret is gone.
    pub(super) fn take_out(&mut self, index: usize) -> Member {
        let mut member = self.members.swap_remove(index);
        if let Some(gone) = member.caret.as_mut().and_then(Caret::withdraw) {
            self.tell_caret(None, gone, None);
        }
        member
    }

    /// Withdraws the caret of a member who stays: it reads from now on, or says it
    /// has none.
    pub(super) fn withdraw_caret(&mut self, index: usize) {
        let member = &mut self.members[index];
        let but = member.connection.id;
        if let Some(gone) = member.caret.as_mut().and_then(Caret::withdraw) {
            self.tell_caret(Some(but), gone, None);
        }
    }

    /// Shows the caret of a member where they said it is, under `number` if it had
    /// none yet.
    fn show_caret(&mut self, index: usize, cursor: &caret::Cursor, number: impl FnOnce() -> u64) {
        let member = &mut self.members[index];
        let state = caret::state_of(&member.connection.user_id, cursor);
        let caret = member.caret.get_or_insert_with(|| Caret {
            told: Telling {
                number: number(),
                clock: 0,
            },
            shown: None,
        });
        caret.told.clock += 1;
        let (but, told) = (member.connection.id, caret.told);
        self.tell_caret(Some(but), told, Some(&state));
        if let Some(caret) = &mut self.members[index].caret {
            caret.shown = Some(Shown {
                state,
                heard: Instant::now(),
            });
        }
    }

    /// Tells every member, but the connection `but` and whom a check holds, a caret:
    /// where it is, or that it is gone. A connection with no room for it is not told:
    /// it reads nothing, and a caret is said again.
    fn tell_caret(&mut self, but: Option<u64>, told: Telling, state: Option<&str>) {
        let update = caret::told(told.number, told.clock, state);
        let epoch = self.epoch;
        for member in &mut self.members {
            if Some(member.connection.id) == but {
                continue;
            }
            if member.is_held() {
                let missed = member.carets_missed.get_or_insert_default();
                if state.is_none() && missed.len() < MOST_MISSED {
                    missed.push(told);
                }
                continue;
            }
            let queue = &member.connection.queue;
            queue.push_caret(member.doc, epoch, told.number, &update);
        }
    }

    /// Forgets every caret: the document was made again, and a caret is a place in
    /// the one before. Nobody is told: each member lets go of that document.
    pub(super) fn forget_carets(&mut self) {
        for member in &mut self.members {
            if let Some(caret) = &mut member.caret {
                caret.shown = None;
            }
            member.carets_missed = None;
        }
    }

    /// Tells the member at `index` where the others' carets are, but those nothing
    /// was heard of for [`TOLD_FOR`].
    pub(super) fn send_carets(&self, index: usize) {
        let to = &self.members[index];
        for other in &self.members {
            let Some(caret) = &other.caret else { continue };
            let Some(shown) = &caret.shown else { continue };
            if other.connection.id == to.connection.id || shown.heard.elapsed() >= TOLD_FOR {
                continue;
            }
            let Telling { number, clock } = caret.told;
            let update = caret::told(number, clock, Some(&shown.state));
            let queue = &to.connection.queue;
            queue.push_caret(to.doc, self.epoch, number, &update);
        }
    }

    /// Tells the member `asked` names, whom a check let go of and kept, what they
    /// were not told of the carets meanwhile: where each is now, and which went (of
    /// those, [`MOST_MISSED`] at most).
    pub(super) fn send_carets_missed(&mut self, asked: &Asked) {
        let Some(index) = self.members.iter().position(|member| asked.is(member)) else {
            return;
        };
        let Some(missed) = self.members[index].carets_missed.take() else {
            return;
        };
        self.send_carets(index);
        let to = &self.members[index];
        for gone in missed {
            let shown_again = self.members.iter().any(|member| {
                member
                    .caret
                    .as_ref()
                    .is_some_and(|caret| caret.told.number == gone.number && caret.shown.is_some())
            });
            if !shown_again {
                let update = caret::told(gone.number, gone.clock, None);
                let queue = &to.connection.queue;
                queue.push_caret(to.doc, self.epoch, gone.number, &update);
            }
        }
    }
}

impl Caret {
    /// Shows it no more; the telling that says so, if it was shown.
    fn withdraw(&mut self) -> Option<Telling> {
        self.shown.take()?;
        self.told.clock += 1;
        Some(self.told)
    }
}

impl Collab {
    /// Takes what a member said of its caret, in an awareness update, and tells the
    /// room's others. Nothing is told for a member who reads, for another making of
    /// the document than the room holds, or for what names no caret
    /// ([`caret::said`]); nobody is told so, and nothing of it is kept or logged. Like
    /// an update, it waits while an immediate check holds the member.
    pub(in crate::collab) async fn caret(
        &self,
        room: &Room,
        connection: &Connection,
        doc: u32,
        epoch: i64,
        update: Bytes,
    ) {
        let Some(said) = caret::said(update) else {
            tracing::debug!(doc, "websocket: a caret frame that names no caret");
            return;
        };
        let Some(mut turn) = self.turn_to_send(room, connection, doc).await else {
            return;
        };
        let Some(index) = turn.index_of(connection, doc) else {
            return;
        };
        let member = &turn.members[index];
        // Held still only as the server stops, which takes nothing more.
        if epoch != turn.epoch || member.mode == DocMode::Read || member.is_held() {
            return;
        }
        match said {
            Said::Shown(cursor) => turn.show_caret(index, &cursor, number),
            Said::Withdrawn => turn.withdraw_caret(index),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use tokio_util::sync::CancellationToken;

    use crate::collab::queue::{Capacity, Queue};
    use crate::collab::rooms::{Joined, Who};

    fn member(id: u64, checking: Option<u64>) -> Member {
        Member {
            connection: Arc::new(Connection {
                id,
                user_id: format!("user {id}"),
                making: crate::app::SessionKept::with_password(),
                queue: Queue::new(Capacity {
                    messages: 512,
                    bytes: 1 << 20,
                }),
                joined: Mutex::new(Joined::default()),
                ended_worker: CancellationToken::new(),
            }),
            doc: 1,
            workspace_id: "workspace".to_owned(),
            mode: DocMode::Write,
            checking,
            missed: None,
            caret: None,
            carets_missed: None,
        }
    }

    /// A caret an editor says: at the start of the body.
    fn said() -> caret::Cursor {
        let state = r#"{"cursor":{"anchor":{"tname":"default"},"head":{"tname":"default"}}}"#;
        let mut update = vec![1, 1, 1, u8::try_from(state.len()).expect("a short state")];
        update.extend_from_slice(state.as_bytes());
        match caret::said(Bytes::from(update)) {
            Some(Said::Shown(cursor)) => cursor,
            other => panic!("a caret: {other:?}"),
        }
    }

    /// Takes what waits for a member, and says how many messages it was.
    fn taken(member: &Member) -> usize {
        use futures_util::FutureExt as _;
        let queue = &member.connection.queue;
        std::iter::from_fn(|| queue.next().now_or_never().flatten()).count()
    }

    /// Who comes in is told the carets heard of within thirty seconds, and no other.
    #[tokio::test(start_paused = true)]
    async fn a_caret_nothing_was_heard_of_is_not_told_to_who_comes_in() {
        let mut turn = Turn {
            members: vec![member(1, None), member(2, None), member(3, None)],
            ..Turn::default()
        };
        turn.show_caret(0, &said(), || 7);
        tokio::time::advance(Duration::from_secs(29)).await;
        turn.show_caret(1, &said(), || 8);
        assert_eq!(taken(&turn.members[2]), 2, "told as each was shown");
        turn.send_carets(2);
        assert_eq!(taken(&turn.members[2]), 2, "both, to who comes in now");
        tokio::time::advance(Duration::from_secs(1)).await;
        turn.send_carets(2);
        assert_eq!(taken(&turn.members[2]), 1, "the one heard of since");
    }

    /// A held member is told nothing, and once let go, of no more carets that went
    /// than the bound.
    #[test]
    fn a_held_member_is_told_of_so_many_carets_that_went_and_no_more() {
        let mut turn = Turn {
            members: vec![member(1, None), member(2, Some(1))],
            ..Turn::default()
        };
        for number in 1..=MOST_MISSED + 10 {
            let number = u64::try_from(number).expect("a small number");
            turn.show_caret(0, &said(), || number);
            turn.withdraw_caret(0);
            // The next is a caret anew, as of a member come in anew.
            turn.members[0].caret = None;
        }
        assert_eq!(taken(&turn.members[1]), 0, "held, told nothing");
        let held = &mut turn.members[1];
        held.checking = None;
        let asked = Asked {
            connection: held.connection.id,
            doc: held.doc,
            who: Who {
                user_id: held.connection.user_id.clone(),
                making: fvoci_store::Making::with_password(),
                workspace_id: held.workspace_id.clone(),
            },
        };
        turn.send_carets_missed(&asked);
        assert_eq!(taken(&turn.members[1]), MOST_MISSED);
        assert!(turn.members[1].carets_missed.is_none());
    }

    /// A caret's number is past what a Y document's client has, a number a browser
    /// counts with, and no two drawn are the same; what holds a caret says nothing of
    /// where it is when it is written out.
    #[test]
    fn a_number_is_drawn_and_a_caret_is_not_written_out() {
        let numbers: std::collections::HashSet<u64> = (0..1000).map(|_| number()).collect();
        assert_eq!(numbers.len(), 1000);
        assert!(
            numbers
                .iter()
                .all(|number| (1 << 32..1 << 53).contains(number))
        );
        let mut turn = Turn {
            members: vec![member(1, None), member(2, None)],
            ..Turn::default()
        };
        turn.show_caret(0, &said(), || 7);
        let written = format!("{turn:?}");
        assert!(
            !written.contains("default") && !written.contains("cursor"),
            "{written}"
        );
    }
}
