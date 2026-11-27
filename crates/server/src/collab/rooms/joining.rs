//! What a connection that comes into a room is sent of the room's document: the
//! whole of it, or, to a connection that holds the document of this making already
//! (it was away for a while) and is let in to write, the server's state vector and
//! what the connection lacks of it, for the connection to send what the server lacks.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use bytes::Bytes;

use super::super::frame::{self, Sync};
use super::super::messages;
use super::super::queue::{NotQueued, Queued};
use super::super::{Collab, Loaded};
use super::{Connection, Room};
use crate::permissions::DocMode;

/// What a `joinDoc` asks: the room of which card of a board of which workspace, and
/// what the connection says it holds of the card's document.
#[derive(Debug)]
pub(in crate::collab) struct Entering<'join> {
    pub(in crate::collab) workspace_id: &'join str,
    pub(in crate::collab) card_id: &'join str,
    pub(in crate::collab) holding: Option<Holding>,
}

/// The document a connection says it holds as it asks to come in.
#[derive(Debug, PartialEq, Eq)]
pub(in crate::collab) struct Holding {
    /// Which making of the document.
    epoch: i64,
    /// Its state vector, version 1.
    state_vector: Vec<u8>,
}

impl Holding {
    /// What a `joinDoc` says the connection holds: `epoch`, and `stateVector` in
    /// base64. `None` for a join that names no state vector, or one that is no base64:
    /// the connection is sent the whole document, as one that holds none.
    pub(in crate::collab) fn of(epoch: i64, state_vector: &str) -> Option<Self> {
        if state_vector.is_empty() {
            return None;
        }
        let state_vector = STANDARD.decode(state_vector).ok()?;
        Some(Self {
            epoch,
            state_vector,
        })
    }
}

impl Collab {
    /// Queues what a connection let into `room` as `doc`, to be in it as `mode` says,
    /// is told first: `docJoined`, and the document as `loaded` ([`Self::first_of`]).
    /// Who is let in to read has nothing to send, and is sent the whole whatever they
    /// say they hold: what they hold that the server lacks shows against it alone.
    ///
    /// # Errors
    ///
    /// Returns why it was not queued: the connection has no room for it, or ended.
    pub(super) async fn welcome(
        &self,
        room: &Room,
        connection: &Connection,
        doc: u32,
        loaded: &Loaded,
        mode: DocMode,
        entering: &Entering<'_>,
    ) -> Result<(), NotQueued> {
        let epoch = loaded.epoch;
        let joined = messages::joined(entering.workspace_id, entering.card_id, doc, epoch, mode);
        let holding = entering.holding.as_ref().filter(|_| mode == DocMode::Write);
        let document = self.first_of(room, loaded, holding).await;
        let document = document.iter().map(|sync| Queued::sync(doc, epoch, sync));
        let first: Vec<Queued> = std::iter::once(Queued::text(doc, joined))
            .chain(document)
            .collect();
        connection.queue.push(first)
    }

    /// The sync messages a connection that comes into `room` is sent after
    /// `docJoined`, of the document as `loaded`: the whole of it (a step 2); or, for a
    /// connection that holds the document of this making, the server's state vector
    /// (a step 1) and then what the connection lacks (a step 2). Whatever keeps the
    /// difference from being read (a state vector that is none, a document that cannot
    /// be asked now, or was made again meanwhile) has the whole sent: the connection
    /// merges it all the same.
    async fn first_of(
        &self,
        room: &Room,
        loaded: &Loaded,
        holding: Option<&Holding>,
    ) -> Vec<Bytes> {
        let whole = || vec![frame::message(Sync::Missing, &loaded.state)];
        let Some(holding) = holding.filter(|holding| holding.epoch == loaded.epoch) else {
            return whole();
        };
        let (app, card_id) = (&room.app, room.card_id.as_str());
        let gone = || Some(room.gone.clone());
        // The state vector first: the difference is read of this making alone, so one
        // made again between the two is found out, and the two are of one document.
        let ours = self.state_vector_while(app, card_id, gone()).await;
        let lacked = self
            .diff_while(app, card_id, loaded.epoch, &holding.state_vector, gone())
            .await;
        match (ours, lacked) {
            (Ok(ours), Ok(lacked)) => vec![
                frame::message(Sync::StateVector, &ours),
                frame::message(Sync::Missing, &lacked),
            ],
            (Err(error), _) | (_, Err(error)) => {
                tracing::debug!(%error, card_id, "a connection that named its document is sent the whole");
                whole()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_join_names_what_it_holds_or_nothing() {
        assert_eq!(
            Holding::of(3, ""),
            None,
            "no state vector: no document held"
        );
        assert_eq!(Holding::of(3, "not base64!"), None);
        let holding = Holding::of(3, "AQID").expect("base64");
        assert_eq!(
            (holding.epoch, &holding.state_vector[..]),
            (3, &[1, 2, 3][..])
        );
    }
}
