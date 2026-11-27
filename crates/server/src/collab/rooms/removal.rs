//! Detaching deleted rooms without waiting on their pending document operations.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::{Collab, Room};
use crate::app::App;
use crate::collab::messages::reason_of;
use crate::error::AppError;

impl Collab {
    /// Called with the document deletion gate held, before allowing any new work.
    pub(in crate::collab) fn detach_deleted_rooms(
        &self,
        app: &App,
        card_ids: &[String],
    ) -> Vec<Arc<Room>> {
        self.inner
            .rooms
            .walls
            .raise(app.store().tenant(), super::AccessScope::cards(card_ids));
        let tenant = app.store().tenant();
        let detached: Vec<Arc<Room>> = {
            let mut rooms = self.rooms_index();
            card_ids
                .iter()
                .filter_map(|id| rooms.remove(tenant, id))
                .collect()
        };
        for room in &detached {
            room.deleted.store(true, Ordering::SeqCst);
            room.gone.cancel();
        }
        detached
    }

    /// A normal last departure writes the body and folds the log, or makes the
    /// document again from the body if many sessions wrote it; a deletion already
    /// wrote it. Check after the document gate, since deletion can pass a close
    /// waiting for it.
    pub(in crate::collab) async fn close_room_document(
        &self,
        room: &Arc<Room>,
    ) -> Result<(), crate::error::AppError> {
        let closing = Arc::clone(room);
        self.on_slot(
            &room.app,
            &room.card_id,
            move |collab, app, card_id, mut slot| async move {
                let written = if closing.deleted.load(Ordering::SeqCst) {
                    Ok(())
                } else {
                    collab.close_held(&app, &card_id, &mut slot).await
                };
                collab.retire(&mut slot);
                written
            },
        )
        .await
    }

    /// The documents were already forgotten: closing an old room must not close the
    /// new document of a card that somebody restored meanwhile.
    pub(in crate::collab) async fn finish_deleted_rooms(&self, rooms: Vec<Arc<Room>>) {
        // As for a card that is not there.
        let reason = reason_of(&AppError::not_found("card"));
        for room in rooms {
            let mut turn = room.turn.lock().await;
            while !turn.members.is_empty() {
                turn.put_out(0, &reason);
            }
            turn.closed = true;
        }
    }
}
