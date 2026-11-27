//! The rooms there are, by organization and by board: a change finds the rooms it
//! reaches among its own organization's, and those of the boards it names without
//! looking at any other.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use fvoci_store::Tenant;

use super::{AccessScope, Room};

#[derive(Debug, Default)]
pub(super) struct Index {
    by_tenant: HashMap<Tenant, Shelf>,
}

/// The rooms of one organization.
#[derive(Debug, Default)]
struct Shelf {
    by_card: HashMap<String, Arc<Room>>,
    /// The cards of each board that have a room.
    by_container: HashMap<String, HashSet<String>>,
}

impl Index {
    /// The room of the card `card_id` of `tenant`, if it has one.
    pub(super) fn get(&self, tenant: Tenant, card_id: &str) -> Option<&Arc<Room>> {
        self.by_tenant.get(&tenant)?.by_card.get(card_id)
    }

    /// Counts `room` among the rooms of `tenant`: of a card that has none.
    pub(super) fn insert(&mut self, tenant: Tenant, room: Arc<Room>) {
        debug_assert!(
            self.get(tenant, &room.card_id).is_none(),
            "a card has one room at most: {}",
            room.card_id
        );
        let of_tenant = self.by_tenant.entry(tenant).or_default();
        of_tenant
            .by_container
            .entry(room.container_id.clone())
            .or_default()
            .insert(room.card_id.clone());
        of_tenant.by_card.insert(room.card_id.clone(), room);
    }

    /// Takes the room of the card `card_id` of `tenant` out of the rooms.
    pub(super) fn remove(&mut self, tenant: Tenant, card_id: &str) -> Option<Arc<Room>> {
        let of_tenant = self.by_tenant.get_mut(&tenant)?;
        let room = of_tenant.by_card.remove(card_id)?;
        if let Some(cards) = of_tenant.by_container.get_mut(&room.container_id) {
            cards.remove(card_id);
            if cards.is_empty() {
                of_tenant.by_container.remove(&room.container_id);
            }
        }
        if of_tenant.by_card.is_empty() {
            self.by_tenant.remove(&tenant);
        }
        Some(room)
    }

    /// Takes `room` out of the rooms of `tenant` if it is the one there: not the
    /// room made anew for its card since.
    pub(super) fn remove_room(&mut self, tenant: Tenant, room: &Arc<Room>) {
        if self
            .get(tenant, &room.card_id)
            .is_some_and(|there| Arc::ptr_eq(there, room))
        {
            self.remove(tenant, &room.card_id);
        }
    }

    /// The rooms of `tenant`.
    pub(super) fn of_tenant(&self, tenant: Tenant) -> impl Iterator<Item = &Arc<Room>> {
        self.by_tenant
            .get(&tenant)
            .into_iter()
            .flat_map(|of_tenant| of_tenant.by_card.values())
    }

    /// The rooms of `tenant` that `scope` reaches: those of the boards or cards it
    /// names looked up, the others found among the organization's.
    pub(super) fn reached(&self, tenant: Tenant, scope: &AccessScope) -> Vec<Arc<Room>> {
        let Some(of_tenant) = self.by_tenant.get(&tenant) else {
            return Vec::new();
        };
        let named = |cards: &mut dyn Iterator<Item = &String>| -> Vec<Arc<Room>> {
            cards
                .filter_map(|card_id| of_tenant.by_card.get(card_id))
                .cloned()
                .collect()
        };
        if let Some(containers) = scope.named_containers() {
            let mut cards = containers
                .iter()
                .filter_map(|container_id| of_tenant.by_container.get(container_id))
                .flatten();
            named(&mut cards)
        } else if let Some(card_ids) = scope.named_cards() {
            named(&mut card_ids.iter())
        } else {
            of_tenant
                .by_card
                .values()
                .filter(|room| scope.contains(room))
                .cloned()
                .collect()
        }
    }

    /// How many rooms there are, for the tests that count them.
    #[cfg(feature = "test-util")]
    pub(super) fn len(&self) -> usize {
        self.by_tenant
            .values()
            .map(|of_tenant| of_tenant.by_card.len())
            .sum()
    }
}
