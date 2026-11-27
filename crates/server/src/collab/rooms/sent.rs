//! What a connection sends into a room: an update, taken into the room's document and
//! sent on to the others, and a state vector, answered with what the connection lacks.

use std::sync::Arc;

use bytes::Bytes;

use super::super::frame::{self, Sync};
use super::super::messages::{self, reason_of};
use super::super::place::Unanswered;
use super::super::queue::Queued;
use super::super::{Collab, Taken, refusals};
use super::{Connection, Member, Room, held};
use crate::app::App;
use crate::permissions::DocMode;

impl Collab {
    /// Takes an update a connection sent into the room's document, and sends it on:
    /// `docAck` to the connection once the log has it, the update to the others, and
    /// what a repair changed to all, but to the members an immediate check holds:
    /// they are sent it once it keeps them. An update that is not taken is answered
    /// with `docNack` and goes to nobody.
    pub(in crate::collab) async fn update(
        &self,
        room: &Arc<Room>,
        connection: &Connection,
        doc: u32,
        epoch: i64,
        update: Bytes,
    ) {
        let Some(mut turn) = self.turn_to_send(room, connection, doc).await else {
            return;
        };
        let Some(index) = turn.index_of(connection, doc) else {
            return;
        };
        let sender = connection.id;
        let to_sender = |member: &Member| member.connection.id == sender;
        let refusal = if turn.members[index].mode == DocMode::Read {
            Some(reason_of(&refusals::read_only()))
        } else if self.rooms_ended() {
            Some(reason_of(&refusals::unavailable()))
        } else if epoch == turn.epoch {
            None
        } else {
            Some(reason_of(&refusals::out_of_step()))
        };
        // The state the held are left at, if this is the first thing they miss.
        let before = if refusal.is_none() && turn.awaits_first_miss() {
            let state_vector = self
                .state_vector_while(&room.app, &room.card_id, Some(room.gone.clone()))
                .await;
            Some(state_vector.map_or(held::Missed::Whole, held::Missed::Since))
        } else {
            None
        };
        let taken = match refusal {
            Some(reason) => Err(reason),
            None => self
                .take_while(
                    &room.app,
                    &room.card_id,
                    epoch,
                    &connection.user_id,
                    &update,
                    Some(room.gone.clone()),
                )
                .await
                .map_err(|unanswered| {
                    if matches!(unanswered, Unanswered::Died(_)) {
                        // Answered first, and closed once the step is done.
                        connection.ended_worker.cancel();
                    }
                    let error = unanswered.into_error();
                    error.log();
                    reason_of(&error)
                }),
        };
        match taken {
            Err(reason) => turn.answer(connection, doc, messages::nack(doc, epoch, &reason)),
            Ok(taken) => {
                // Told whatever waits for the sender: the update is logged, and is
                // sent on to the others also if the sender is put out for its queue.
                turn.answer(connection, doc, messages::ack(doc, epoch, taken.seq));
                if !turn.typed.contains(&connection.user_id) {
                    turn.typed.push(connection.user_id.clone());
                }
                if taken.remade {
                    // The update is in the body the document was made again from.
                    // If it cannot be opened for now, the room's task hears of the
                    // making and tries again.
                    self.anew(room, &mut turn, None).await;
                } else {
                    if let Some(before) = &before {
                        turn.miss(before);
                    }
                    let update = frame::message(Sync::Update, &update);
                    turn.send(
                        |member| !to_sender(member) && !member.is_held(),
                        |member| [Queued::sync(member.doc, epoch, &update)],
                    );
                    if let Some(repair) = &taken.repair {
                        let repair = frame::message(Sync::Update, repair);
                        turn.send(
                            |member| !member.is_held(),
                            |member| [Queued::sync(member.doc, epoch, &repair)],
                        );
                    }
                }
            }
        }
        self.shut_if_empty(room, &mut turn).await;
    }

    /// Sends the room of a card of `app`'s organization, if one is open, the update that
    /// added an agent's nodes to its document at `epoch` ([`Collab::append`]), as an
    /// editor's is sent on: to every member, and what a repair changed after it. A member
    /// an immediate check holds is sent the document once it keeps them. A document made
    /// again since is sent whole, as it holds the nodes; one of another epoch sent it
    /// already. Who came into the room after the nodes were added holds them, and is
    /// sent nothing it lacks: an update taken twice is taken once.
    pub(in crate::collab) async fn send_appended(
        &self,
        app: &App,
        card_id: &str,
        epoch: i64,
        update: &[u8],
        taken: &Taken,
    ) {
        let room = self
            .rooms_index()
            .get(app.store().tenant(), card_id)
            .map(Arc::clone);
        let Some(room) = room else {
            return;
        };
        let mut turn = room.turn.lock().await;
        if taken.remade {
            self.anew(&room, &mut turn, None).await;
        } else if turn.epoch == epoch {
            if turn.awaits_first_miss() {
                turn.miss(&held::Missed::Whole);
            }
            for sent in std::iter::once(update).chain(taken.repair.as_deref()) {
                let sent = frame::message(Sync::Update, sent);
                turn.send(
                    |member| !member.is_held(),
                    |member| [Queued::sync(member.doc, epoch, &sent)],
                );
            }
        }
        self.shut_if_empty(&room, &mut turn).await;
    }

    /// Sends a connection what the room's document has that a holder of
    /// `state_vector` has not, as a sync step 2: the answer to a sync step 1. What
    /// cannot be answered (another epoch, bytes that are no state vector) is not. Like
    /// an update, it waits while an immediate check holds the connection: answered
    /// then, if it keeps them.
    pub(in crate::collab) async fn missing(
        &self,
        room: &Arc<Room>,
        connection: &Connection,
        doc: u32,
        epoch: i64,
        state_vector: Bytes,
    ) {
        let Some(mut turn) = self.turn_to_send(room, connection, doc).await else {
            return;
        };
        if turn.index_of(connection, doc).is_none() || epoch != turn.epoch || self.rooms_ended() {
            self.shut_if_empty(room, &mut turn).await;
            return;
        }
        match self
            .diff_while(
                &room.app,
                &room.card_id,
                epoch,
                &state_vector,
                Some(room.gone.clone()),
            )
            .await
        {
            Ok(missing) if !room.gone.is_cancelled() => {
                let missing = frame::message(Sync::Missing, &missing);
                turn.send(
                    |member| member.connection.id == connection.id,
                    |_| [Queued::sync(doc, epoch, &missing)],
                );
            }
            Ok(_) => {}
            Err(error) => error.log(),
        }
        self.shut_if_empty(room, &mut turn).await;
    }
}
