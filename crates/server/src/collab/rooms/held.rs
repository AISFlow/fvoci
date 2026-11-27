//! What a room does not send a member while an immediate check holds them, and sends
//! them once the check keeps them: nothing written after a change reaches somebody
//! the change may have shut out before the store said so.

use super::super::Collab;
use super::super::frame::{self, Sync};
use super::super::messages::reason_of;
use super::super::queue::Queued;
use super::{Asked, Member, Room, Turn};

/// What the room did not send a member while it held them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Missed {
    /// What the document took after it was of this state vector (version 1), in the
    /// room's epoch: the member holds the document as it was then.
    Since(Vec<u8>),
    /// The document was made anew, or what it was before is not known: the member is
    /// sent the document anew.
    Whole,
}

impl Member {
    /// Whether the room holds what it sends from the member: an immediate check holds
    /// them.
    pub(super) fn is_held(&self) -> bool {
        self.checking.is_some()
    }
}

impl Turn {
    /// Whether a held member missed nothing yet: what the room sends next is the first
    /// thing they miss, and the state of the document before it is to be kept for them.
    pub(super) fn awaits_first_miss(&self) -> bool {
        self.members
            .iter()
            .any(|member| member.is_held() && member.missed.is_none())
    }

    /// Notes for every held member that the room sends what they do not get: the
    /// document anew takes the place of what they missed before; a document state
    /// keeps the earlier one a member missed since.
    pub(super) fn miss(&mut self, missed: &Missed) {
        for member in self.members.iter_mut().filter(|member| member.is_held()) {
            if matches!(missed, Missed::Whole) || member.missed.is_none() {
                member.missed = Some(missed.clone());
            }
        }
    }
}

impl Collab {
    /// Sends the member `asked` names, whom an immediate check let go of and kept,
    /// what the room did not send them meanwhile: what the document has that they do
    /// not, as one update, or the document anew if it was made anew meanwhile or that
    /// difference cannot be read. A member who can be sent neither is put out, with the
    /// reason the document could not be opened: they do not hold the room's document.
    pub(super) async fn send_missed(&self, room: &Room, turn: &mut Turn, asked: &Asked) {
        let Some(index) = turn.members.iter().position(|member| asked.is(member)) else {
            return;
        };
        let Some(missed) = turn.members[index].missed.take() else {
            return;
        };
        let (doc, epoch) = (asked.doc, turn.epoch);
        let gone = Some(room.gone.clone());
        if let Missed::Since(state_vector) = missed {
            let diff = self
                .diff_while(&room.app, &room.card_id, epoch, &state_vector, gone.clone())
                .await;
            match diff {
                Ok(_) if room.gone.is_cancelled() => return,
                Ok(diff) => {
                    let diff = frame::message(Sync::Update, &diff);
                    turn.send(
                        |member| asked.is(member),
                        |_| [Queued::sync(doc, epoch, &diff)],
                    );
                    return;
                }
                Err(error) => {
                    tracing::warn!(%error, "a held member is sent the document anew in place of what they missed");
                }
            }
        }
        let loaded = self.open_while(&room.app, &room.card_id, gone).await;
        if room.gone.is_cancelled() {
            return;
        }
        match loaded {
            // The room holds this document: only the member is behind.
            Ok(loaded) if loaded.epoch == epoch => {
                turn.push_anew(&loaded, |member| asked.is(member));
            }
            // Made anew since the room was sent it, and the room not told yet.
            Ok(loaded) => turn.send_anew(&loaded),
            Err(error) => {
                error.log();
                if let Some(index) = turn.members.iter().position(|member| asked.is(member)) {
                    turn.put_out(index, &reason_of(&error));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use tokio_util::sync::CancellationToken;

    use crate::collab::queue::{Capacity, Queue};
    use crate::collab::rooms::{Connection, Joined};
    use crate::permissions::DocMode;

    fn member(id: u64, checking: Option<u64>) -> Member {
        Member {
            connection: Arc::new(Connection {
                id,
                user_id: format!("user {id}"),
                making: crate::app::SessionKept::with_password(),
                queue: Queue::new(Capacity {
                    messages: 8,
                    bytes: 1024,
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

    fn missed(turn: &Turn) -> Vec<Option<Missed>> {
        turn.members
            .iter()
            .map(|member| member.missed.clone())
            .collect()
    }

    #[test]
    fn only_the_held_miss_and_the_earliest_state_is_kept() {
        let mut turn = Turn {
            members: vec![member(1, None), member(2, Some(7))],
            ..Turn::default()
        };
        assert!(turn.awaits_first_miss());
        turn.miss(&Missed::Since(vec![1]));
        assert!(!turn.awaits_first_miss());
        turn.miss(&Missed::Since(vec![2]));
        assert_eq!(missed(&turn), [None, Some(Missed::Since(vec![1]))]);
        turn.miss(&Missed::Whole);
        turn.miss(&Missed::Since(vec![3]));
        assert_eq!(missed(&turn), [None, Some(Missed::Whole)]);
    }
}
