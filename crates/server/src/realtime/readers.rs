//! How many connections may read one shared board by its public link at once
//! (`public_link_connections_per_container`), and how many of those may come from one
//! address (`public_link_connections_per_address`; an IPv6 address counts by its /64).
//!
//! A connection reads a board by its link when it follows the board's blocks by the
//! link's read token, or visits the board with that token (`presence.rs`); it is one
//! reader however it does. One that would be one too many is not taken: its
//! subscription is answered `blocksUnsubscribed`, its visit is not kept and so not
//! counted among the board's anonymous readers. A reader already taken stays.

use super::{Listener, ListenerId, Listeners};
use crate::app::App;
use crate::auth::limiter::AddressKey;

/// The limits of the readers by a public link, from the configuration.
#[derive(Debug, Clone, Copy)]
pub(super) struct ReaderLimits {
    per_container: usize,
    per_address: usize,
}

impl ReaderLimits {
    pub(super) fn of(app: &App) -> Self {
        let config = app.config();
        Self {
            per_container: as_count(config.public_link_connections_per_container),
            per_address: as_count(config.public_link_connections_per_address),
        }
    }
}

fn as_count(setting: u32) -> usize {
    usize::try_from(setting).unwrap_or(usize::MAX)
}

impl Listener {
    /// Whether the connection reads `container_id` by its public link: a visit whose
    /// lease lapsed counts no more, as it is on no roster.
    fn reads_by_link(&self, container_id: &str) -> bool {
        self.shared_containers
            .iter()
            .any(|permit| permit.container == container_id)
            || (self.presence.reads_shared_container(container_id) && self.presence.is_current())
    }
}

impl Listeners {
    /// Whether the connection `id` may read `container_id` by its public link: it does
    /// already, or the board has fewer such readers than `limits` allow, and so has the
    /// connection's address among them. A connection with no address (the
    /// administration socket) is held to the board's limit alone.
    pub(super) fn admits_reader(
        &self,
        id: ListenerId,
        container_id: &str,
        limits: ReaderLimits,
    ) -> bool {
        let Some(listener) = self.by_id.get(&id) else {
            return false;
        };
        if listener.reads_by_link(container_id) {
            return true;
        }
        let (mut on_container, mut from_address) = (0, 0);
        for other in self
            .by_id
            .values()
            .filter(|other| other.reads_by_link(container_id))
        {
            on_container += 1;
            if same_address(listener.address, other.address) {
                from_address += 1;
            }
        }
        on_container < limits.per_container && from_address < limits.per_address
    }
}

fn same_address(one: Option<AddressKey>, other: Option<AddressKey>) -> bool {
    one.is_some() && one == other
}

#[cfg(test)]
mod tests {
    use std::net::IpAddr;

    use super::super::tests::add_listener_from;
    use super::*;

    const LIMITS: ReaderLimits = ReaderLimits {
        per_container: 3,
        per_address: 2,
    };

    fn address(text: &str) -> AddressKey {
        AddressKey::from(text.parse::<IpAddr>().expect("an address"))
    }

    /// Makes `id` a reader of the board, as a taken subscription does.
    fn read(listeners: &mut Listeners, id: ListenerId, container_id: &str) {
        assert!(listeners.admits_reader(id, container_id, LIMITS));
        let listener = listeners.by_id.get_mut(&id).expect("the listener");
        listener
            .shared_containers
            .push(crate::realtime::ReadPermit {
                container: container_id.to_owned(),
                token: String::new(),
                workspace: String::new(),
                blocks: Vec::new(),
            });
    }

    #[test]
    fn an_address_and_its_ipv6_network_read_a_board_up_to_their_limit() {
        let mut listeners = Listeners::default();
        let one = add_listener_from(&mut listeners, "", Some(address("2001:db8::1")));
        let two = add_listener_from(&mut listeners, "", Some(address("2001:db8::2")));
        let three = add_listener_from(&mut listeners, "", Some(address("2001:db8::3")));
        read(&mut listeners, one, "board");
        read(&mut listeners, two, "board");
        assert!(
            !listeners.admits_reader(three, "board", LIMITS),
            "a third of the same /64"
        );
        assert!(
            listeners.admits_reader(three, "another board", LIMITS),
            "each board counts its own readers"
        );
        assert!(
            listeners.admits_reader(one, "board", LIMITS),
            "a reader taken stays taken"
        );
    }

    #[test]
    fn a_board_reads_up_to_its_limit_from_any_addresses() {
        let mut listeners = Listeners::default();
        for at in ["192.0.2.1", "192.0.2.2", "192.0.2.3"] {
            let id = add_listener_from(&mut listeners, "", Some(address(at)));
            read(&mut listeners, id, "board");
        }
        let fourth = add_listener_from(&mut listeners, "", Some(address("198.51.100.1")));
        assert!(!listeners.admits_reader(fourth, "board", LIMITS));
        let unaddressed = add_listener_from(&mut listeners, "", None);
        assert!(!listeners.admits_reader(unaddressed, "board", LIMITS));
    }

    #[test]
    fn connections_without_an_address_are_held_to_the_containers_limit_alone() {
        let mut listeners = Listeners::default();
        for _ in 0..3 {
            let id = add_listener_from(&mut listeners, "", None);
            read(&mut listeners, id, "board");
        }
        let fourth = add_listener_from(&mut listeners, "", None);
        assert!(!listeners.admits_reader(fourth, "board", LIMITS));
    }
}
