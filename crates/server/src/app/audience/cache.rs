//! Reusable authorization reads at a committed store visibility revision.

use std::collections::HashMap;
use std::fmt;
use std::hash::Hash;
use std::sync::Mutex;

use fvoci_store::{Store, StoreError, VisibilityScope, VisibilityVersion};

/// Bounds the memory of one scope's slot even when a client probes arbitrary workspace
/// identifiers.
const MAX_ENTRIES: usize = 4096;

pub(super) struct Cache<K, V> {
    state: Mutex<HashMap<VisibilityScope, Slot<K, V>>>,
}

/// What was read in one scope (an organization, or everything) at one revision.
struct Slot<K, V> {
    revision: VisibilityVersion,
    values: HashMap<K, V>,
}

impl<K, V> Default for Cache<K, V> {
    fn default() -> Self {
        Self {
            state: Mutex::new(HashMap::new()),
        }
    }
}

impl<K, V> fmt::Debug for Cache<K, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Some keys contain read tokens, which must never appear in diagnostics.
        f.debug_struct("Cache").finish_non_exhaustive()
    }
}

impl<K: Eq + Hash + Clone, V: Clone> Cache<K, V> {
    /// The value of `key` at the store's current revision: cached, or read and kept
    /// for that revision. A value read while the revision is unsettled, or while a
    /// write landed, is answered without being kept: a caller that acts on it as
    /// current checks the revision again (`Store::if_visibility_current`).
    pub(super) async fn read<F: Future<Output = Result<V, StoreError>> + Send>(
        &self,
        store: &Store,
        key: K,
        read: impl FnOnce() -> F + Send,
    ) -> Result<V, StoreError> {
        let revision = store.visibility_version().await;
        if !revision.cacheable() {
            return read().await;
        }
        if let Some(value) = self.slot(revision, |slot| slot.values.get(&key).cloned()) {
            return Ok(value);
        }
        let value = read().await?;
        if store.visibility_version().await == revision {
            self.slot(revision, |slot| {
                // Invalid workspace IDs must not grow a long-lived connection's cache
                // forever; one scope's probing empties only its own slot, and gives
                // back what it held.
                if slot.values.len() >= MAX_ENTRIES {
                    slot.values = HashMap::new();
                }
                slot.values.insert(key, value.clone());
            });
        }
        Ok(value)
    }

    /// Runs `with` on the slot of the revision's scope, emptied first if it was of
    /// another revision.
    fn slot<T>(&self, revision: VisibilityVersion, with: impl FnOnce(&mut Slot<K, V>) -> T) -> T {
        let mut slots = self.state.lock().expect("audience cache lock");
        let slot = slots.entry(revision.scope()).or_insert_with(|| Slot {
            revision,
            values: HashMap::new(),
        });
        if slot.revision != revision {
            slot.values = HashMap::new();
            slot.revision = revision;
        }
        with(slot)
    }
}
