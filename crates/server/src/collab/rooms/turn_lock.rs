//! The turn of a room, with how many of its members may write, which is read without
//! waiting for the turn.

use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::sync::{Mutex, MutexGuard};

use super::Turn;
use crate::permissions::DocMode;

/// A room's [`Turn`], and the number of its members who may write as whoever held the
/// turn last left it: none once the room is closed.
///
/// A whole write of a card asks that number while it holds the card's slot
/// ([`super::Collab::has_editors`]). Everything else takes the turn first and the slot
/// after (an update, a join, a member asked again), so a write that waited for the turn
/// there could wait for one that waits for its slot, and neither would go on.
#[derive(Debug, Default)]
pub(super) struct TurnLock {
    turn: Mutex<Turn>,
    writers: AtomicUsize,
}

impl TurnLock {
    /// Waits for the turn. What it is left with is counted as it is let go.
    pub(super) async fn lock(&self) -> TurnGuard<'_> {
        TurnGuard {
            turn: self.turn.lock().await,
            writers: &self.writers,
        }
    }

    /// How many of the members may write, as the turn was last left.
    pub(super) fn writers(&self) -> usize {
        self.writers.load(Ordering::Acquire)
    }
}

/// The turn of a room, held.
#[derive(Debug)]
pub(super) struct TurnGuard<'room> {
    turn: MutexGuard<'room, Turn>,
    writers: &'room AtomicUsize,
}

impl Deref for TurnGuard<'_> {
    type Target = Turn;

    fn deref(&self) -> &Turn {
        &self.turn
    }
}

impl DerefMut for TurnGuard<'_> {
    fn deref_mut(&mut self) -> &mut Turn {
        &mut self.turn
    }
}

impl Drop for TurnGuard<'_> {
    fn drop(&mut self) {
        let writers = if self.turn.closed {
            0
        } else {
            self.turn
                .members
                .iter()
                .filter(|member| member.mode == DocMode::Write)
                .count()
        };
        self.writers.store(writers, Ordering::Release);
    }
}
