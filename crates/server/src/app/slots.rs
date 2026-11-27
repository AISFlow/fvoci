//! How many exports, and how many imports, run at once: one per user, and a configured
//! number on the whole server.
//!
//! Both write and read whole archives on the server's disk and take its time; a user who
//! starts many, or many users at once, would take it from everyone else. A request over
//! a limit is refused at once (429) rather than queued: the client says when to try
//! again.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, PoisonError};

/// Why a slot was not given.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Busy {
    /// The user has one running.
    User,
    /// The server runs as many as it may.
    Server,
}

/// The running jobs of one kind.
#[derive(Debug)]
pub(crate) struct Slots {
    inner: Arc<Mutex<Held>>,
}

#[derive(Debug)]
struct Held {
    /// How many may run on the server at once.
    capacity: usize,
    /// The users that have one running.
    users: HashSet<String>,
}

impl Slots {
    /// Slots for `capacity` jobs on the server at once, at least one.
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Held {
                capacity: capacity.max(1),
                users: HashSet::new(),
            })),
        }
    }

    /// How many slots are held.
    pub(crate) fn held(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .users
            .len()
    }

    /// Takes a slot for the user, held until the [`Slot`] is dropped.
    pub(crate) fn take(&self, user_id: &str) -> Result<Slot, Busy> {
        // The set has nothing a panic could leave half done.
        let mut held = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        if held.users.contains(user_id) {
            return Err(Busy::User);
        }
        if held.users.len() >= held.capacity {
            return Err(Busy::Server);
        }
        held.users.insert(user_id.to_owned());
        Ok(Slot {
            slots: Arc::clone(&self.inner),
            user_id: user_id.to_owned(),
        })
    }
}

/// A running job's place; it is given back when this is dropped, also when the request
/// that holds it is cancelled.
#[derive(Debug)]
pub struct Slot {
    slots: Arc<Mutex<Held>>,
    user_id: String,
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut held = self.slots.lock().unwrap_or_else(PoisonError::into_inner);
        held.users.remove(&self.user_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_per_user_and_the_capacity_on_the_server() {
        let slots = Slots::new(2);
        let first = slots.take("a").expect("a's first");
        assert_eq!(slots.take("a").err(), Some(Busy::User), "a's second");
        let second = slots.take("b").expect("b's first");
        assert_eq!(
            slots.take("c").err(),
            Some(Busy::Server),
            "the server is full"
        );
        drop(first);
        assert!(slots.take("c").is_ok(), "a's slot was given back");
        drop(second);
        assert!(slots.take("a").is_ok(), "and a may start again");
    }

    #[test]
    fn at_least_one_runs() {
        let slots = Slots::new(0);
        assert!(slots.take("a").is_ok());
    }
}
