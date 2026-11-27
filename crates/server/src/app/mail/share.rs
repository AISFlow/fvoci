//! What one organization may hold of the invitation mail. Alone on its server, all of
//! the queue and of the sendings invitations may use. Among several, a fair share: at
//! most half of what the others leave of the queue (so that with `k` organizations
//! waiting each settles at a `k + 1`-th of it, and one more always finds room), never
//! less than one request; and two of the three sendings, so that another
//! organization's invitations are taken and go out whatever one organization queued.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::sync::Semaphore;

/// How many messages one organization of several sends at once, of the invitations'
/// sendings: one is always left for the others.
pub(super) const SENDING_PER_ORG: usize = super::INVITATION_CONNECTIONS - 1;

/// How many organizations the server holds, which decides what one may hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) enum Organizations {
    /// One (SQLite, or a server with one organization in use): it has no share but the
    /// whole queue, which says when it is full, and every sending of invitations.
    One,
    /// Several: each holds its fair share.
    Several,
}

/// The invitation messages each organization holds in the queue, kept in memory.
#[derive(Debug)]
pub(super) struct OrgShares {
    /// `mail_invitation_queue`.
    queue: u32,
    /// The least an organization of several may hold: one request's recipients
    /// (`invitation_mail_recipients`).
    least: u32,
    held: Mutex<HashMap<Arc<str>, Held>>,
}

#[derive(Debug)]
struct Held {
    messages: u32,
    /// The organization's own sendings, while it holds messages.
    sending: Arc<Semaphore>,
}

/// One message's part of its organization's share, given back on drop.
#[derive(Debug)]
pub(super) struct OrgPlace {
    shares: Arc<OrgShares>,
    org: Arc<str>,
    /// The organization's sendings, which the message waits for before it is sent.
    pub(super) sending: Arc<Semaphore>,
}

impl OrgShares {
    /// Shares of a queue of `queue` messages, of which an organization of several holds
    /// at least `least`.
    pub(super) fn new(queue: u32, least: u32) -> Self {
        Self {
            queue,
            least,
            held: Mutex::new(HashMap::new()),
        }
    }

    fn held(&self) -> std::sync::MutexGuard<'_, HashMap<Arc<str>, Held>> {
        self.held
            .lock()
            .expect("the organizations' mail shares are never held across a panic")
    }

    /// Takes the parts of `messages` for `org`, or `None` if they would pass what it
    /// may hold among `organizations`. Whether the queue itself has room is the
    /// queue's to say.
    pub(super) fn take(
        self: &Arc<Self>,
        org: &str,
        messages: u32,
        organizations: Organizations,
    ) -> Option<Vec<OrgPlace>> {
        let mut held = self.held();
        let others: u32 = held
            .iter()
            .filter(|(key, _)| key.as_ref() != org)
            .map(|(_, other)| other.messages)
            .sum();
        let (most, sendings) = match organizations {
            Organizations::One => (u32::MAX, super::INVITATION_CONNECTIONS),
            Organizations::Several => (
                self.least.max(self.queue.saturating_sub(others) / 2),
                SENDING_PER_ORG,
            ),
        };
        let org: Arc<str> = held
            .get_key_value(org)
            .map_or_else(|| Arc::from(org), |(key, _)| Arc::clone(key));
        let entry = held.entry(Arc::clone(&org)).or_insert_with(|| Held {
            messages: 0,
            sending: Arc::new(Semaphore::new(sendings)),
        });
        if entry.messages.saturating_add(messages) > most {
            if entry.messages == 0 {
                held.remove(&org);
            }
            return None;
        }
        entry.messages += messages;
        let sending = Arc::clone(&entry.sending);
        let places = (0..messages)
            .map(|_| OrgPlace {
                shares: Arc::clone(self),
                org: Arc::clone(&org),
                sending: Arc::clone(&sending),
            })
            .collect();
        Some(places)
    }
}

#[cfg(test)]
impl OrgShares {
    /// How many messages `org` holds.
    pub(super) fn messages_of(&self, org: &str) -> Option<u32> {
        self.held().get(org).map(|held| held.messages)
    }
}

impl Drop for OrgPlace {
    fn drop(&mut self) {
        // A drop does not panic: a poisoned lock leaves the count as it is.
        if let Ok(mut held) = self.shares.held.lock()
            && let Some(entry) = held.get_mut(&self.org)
        {
            entry.messages = entry.messages.saturating_sub(1);
            if entry.messages == 0 {
                held.remove(&self.org);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_organization_of_several_holds_half_of_what_the_others_leave() {
        let shares = Arc::new(OrgShares::new(12, 2));
        let first = shares
            .take("a", 6, Organizations::Several)
            .expect("half of the queue");
        assert!(
            shares.take("a", 1, Organizations::Several).is_none(),
            "past a's"
        );
        let other = shares
            .take("b", 3, Organizations::Several)
            .expect("half of what a leaves");
        assert!(shares.take("b", 1, Organizations::Several).is_none());
        let third = shares
            .take("c", 2, Organizations::Several)
            .expect("never less than one request");
        assert!(!Arc::ptr_eq(&first[0].sending, &other[0].sending));
        drop(first);
        let again = shares
            .take("a", 3, Organizations::Several)
            .expect("given back as they leave");
        assert!(Arc::ptr_eq(&again[0].sending, &again[2].sending));
        drop((again, other, third));
        assert!(
            shares.held().is_empty(),
            "nothing is kept of who holds nothing"
        );
        assert!(shares.take("c", 7, Organizations::Several).is_none());
        assert!(shares.held().is_empty(), "nor of a refusal");
    }

    #[test]
    fn an_organization_alone_holds_the_whole_queue_and_every_sending() {
        let shares = Arc::new(OrgShares::new(12, 2));
        let all = shares
            .take("a", 12, Organizations::One)
            .expect("the whole queue");
        assert_eq!(
            all[0].sending.available_permits(),
            super::super::INVITATION_CONNECTIONS
        );
        assert!(
            shares.take("a", 1, Organizations::One).is_some(),
            "the queue, not a share, says when it is full"
        );
    }
}
