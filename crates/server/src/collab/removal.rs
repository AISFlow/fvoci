//! Trash snapshots include pending edits, and no document work crosses deletion.

use std::collections::BTreeSet;
use std::future::Future;
use std::sync::Arc;

use fvoci_store::Tenant;
#[cfg(feature = "test-util")]
use tokio::sync::Notify;
use tokio::sync::{OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock};

use super::{Collab, place};
use crate::app::App;
use crate::error::AppError;

/// What a deletion takes to the trash, read while the tree of cards and pages cannot
/// change.
#[derive(Debug)]
pub(crate) enum Deletion {
    /// A block and its descendants, at any depth.
    Block(String),
    /// Multiple blocks on one container and their descendants, at any depth.
    Blocks {
        container_id: String,
        block_ids: Vec<String>,
    },
    /// Every card of these boards, and every page of these wikis.
    Containers(Vec<String>),
}

/// Keeps a structural store mutation on one side of a document deletion for the touched
/// container(s): structure locks are acquired before document gates, in sorted
/// `container_id` order, and never called while a document gate or slot is already held.
#[derive(Debug)]
#[must_use = "the gates are let go when this is dropped"]
pub(crate) struct StructureChange {
    _structures: Vec<ContainerReadGuard>,
    _held: Vec<ContainerReadGuard>,
}

#[derive(Debug, Clone, Copy)]
enum LockMap {
    Gates,
    Structures,
}

impl LockMap {
    fn of(self, inner: &super::Inner) -> &std::sync::Mutex<super::ContainerLocks> {
        match self {
            Self::Gates => &inner.gates,
            Self::Structures => &inner.structures,
        }
    }
}

/// Tracks one reference to a per-container [`RwLock`] and removes its map entry on drop
/// when no other caller holds or waits on that key (`strong_count == 2` while the map
/// mutex is held).
#[derive(Debug)]
struct TrackedLock {
    inner: Arc<super::Inner>,
    which: LockMap,
    key: (Tenant, String),
    lock: Arc<RwLock<()>>,
}

impl TrackedLock {
    async fn read_owned(self) -> ContainerReadGuard {
        let guard = Arc::clone(&self.lock).read_owned().await;
        ContainerReadGuard {
            _guard: guard,
            _tracked: self,
        }
    }

    async fn write_owned(self) -> ContainerWriteGuard {
        let guard = Arc::clone(&self.lock).write_owned().await;
        ContainerWriteGuard {
            _guard: guard,
            _tracked: self,
        }
    }
}

impl Drop for TrackedLock {
    fn drop(&mut self) {
        let mut map = self
            .which
            .of(&self.inner)
            .lock()
            .expect("container locks are not poisoned");
        // The map entry plus `self.lock`: nobody else holds or waits on this `RwLock`,
        // and nobody can clone it out of the map while `map` is locked.
        if Arc::strong_count(&self.lock) == 2 {
            map.remove(&self.key);
        }
    }
}

/// Self-pruning read guard for a container gate or structure lock; `_guard` drops before
/// `_tracked` in declaration order so the `RwLock` permit is released first.
#[derive(Debug)]
pub(super) struct ContainerReadGuard {
    _guard: OwnedRwLockReadGuard<()>,
    _tracked: TrackedLock,
}

/// Self-pruning write guard for a container gate or structure lock; `_guard` drops
/// before `_tracked` in declaration order so the `RwLock` permit is released first.
#[derive(Debug)]
struct ContainerWriteGuard {
    _guard: OwnedRwLockWriteGuard<()>,
    _tracked: TrackedLock,
}

impl Collab {
    /// Hold only around direct store writes on `container_id`, never around another
    /// collab operation or while holding a document gate: a deletion waiting for the
    /// gate would hold the second read back.
    pub(crate) async fn changing_structure(
        &self,
        app: &App,
        container_id: &str,
    ) -> StructureChange {
        let tenant = app.store().tenant();
        let structure = self.structure(tenant, container_id).read_owned().await;
        let gate = self.reading(tenant, container_id).await;
        StructureChange {
            _structures: vec![structure],
            _held: vec![gate],
        }
    }

    /// Hold around direct store writes spanning multiple containers, acquiring structure
    /// locks and then document gates in sorted `container_id` order.
    pub(crate) async fn changing_structures<I, S>(
        &self,
        app: &App,
        container_ids: I,
    ) -> StructureChange
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let tenant = app.store().tenant();
        let sorted: BTreeSet<String> = container_ids
            .into_iter()
            .map(|id| id.as_ref().to_owned())
            .collect();
        let mut structures = Vec::with_capacity(sorted.len());
        for id in &sorted {
            structures.push(self.structure(tenant, id).read_owned().await);
        }
        let held = self.reading_many(tenant, &sorted).await;
        StructureChange {
            _structures: structures,
            _held: held,
        }
    }

    fn tracked_lock(&self, which: LockMap, tenant: Tenant, container_id: &str) -> TrackedLock {
        debug_assert!(
            !container_id.is_empty(),
            "container lock key requires a non-empty container_id"
        );
        let key = (tenant, container_id.to_owned());
        let lock = {
            let mut map = which
                .of(&self.inner)
                .lock()
                .expect("container locks are not poisoned");
            Arc::clone(map.entry(key.clone()).or_default())
        };
        TrackedLock {
            inner: Arc::clone(&self.inner),
            which,
            key,
            lock,
        }
    }

    /// The gate of a container's documents: document work reads it, a deletion writes
    /// it. Neither organizations nor unrelated containers wait for each other's
    /// deletions.
    fn gate(&self, tenant: Tenant, container_id: &str) -> TrackedLock {
        self.tracked_lock(LockMap::Gates, tenant, container_id)
    }

    /// The structure lock of a container: `changing_structure` reads it and the
    /// `Deletion::Block` fast-path writes it so a room-bearing block cannot move under a
    /// non-room block between the fast-path check and the deletion.
    fn structure(&self, tenant: Tenant, container_id: &str) -> TrackedLock {
        self.tracked_lock(LockMap::Structures, tenant, container_id)
    }

    /// Taken before any slot of the container, never while one is held.
    pub(super) async fn reading(&self, tenant: Tenant, container_id: &str) -> ContainerReadGuard {
        self.gate(tenant, container_id).read_owned().await
    }

    /// Acquires read gates for `container_ids` in their `BTreeSet` order.
    pub(super) async fn reading_many(
        &self,
        tenant: Tenant,
        container_ids: &BTreeSet<String>,
    ) -> Vec<ContainerReadGuard> {
        let mut guards = Vec::with_capacity(container_ids.len());
        for id in container_ids {
            guards.push(self.reading(tenant, id).await);
        }
        guards
    }

    async fn writing(&self, tenant: Tenant, container_ids: &[String]) -> Vec<ContainerWriteGuard> {
        let sorted: BTreeSet<&str> = container_ids.iter().map(String::as_str).collect();
        let mut guards = Vec::with_capacity(sorted.len());
        for id in sorted {
            guards.push(self.gate(tenant, id).write_owned().await);
        }
        guards
    }

    /// Writes pending bodies before the trash snapshot, then closes the deleted rooms.
    /// The task owns the entire operation even when its HTTP caller goes away.
    pub(crate) async fn remove<T, W, F>(
        &self,
        app: &App,
        deletion: Deletion,
        remove: W,
    ) -> Result<T, AppError>
    where
        T: Send + 'static,
        W: FnOnce(App) -> F + Send + 'static,
        F: Future<Output = Result<T, AppError>> + Send + 'static,
    {
        let (collab, scoped) = (self.clone(), app.clone());
        app.spawn_tracked(async move { collab.remove_held(&scoped, deletion, remove).await })
            .await
            .map_err(|error| {
                AppError::internal(format_args!("the document deletion ended: {error}"))
            })?
    }

    /// Pauses the next deletion at `stage` until the second is notified; the first is
    /// notified when it gets there. For the tests of what happens meanwhile.
    #[cfg(feature = "test-util")]
    #[doc(hidden)]
    #[must_use]
    pub fn pause_next_deletion(&self, stage: DeletionStage) -> (Arc<Notify>, Arc<Notify>) {
        let (waiting, go) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
        *self
            .inner
            .deletion_pause
            .lock()
            .expect("the deletion pause is not poisoned") =
            Some((stage, Arc::clone(&waiting), Arc::clone(&go)));
        (waiting, go)
    }

    /// Returns `(gates_len, structures_len, has_empty_container_key)` across `gates`,
    /// `structures`, and `slots` for lock-pruning and key-hygiene tests.
    #[cfg(feature = "test-util")]
    #[doc(hidden)]
    #[must_use]
    pub fn lock_and_slot_stats(&self) -> (usize, usize, bool) {
        let gates = self.inner.gates.lock().expect("gates");
        let structures = self.inner.structures.lock().expect("structures");
        let slots = self.inner.slots.lock().expect("slots");
        let has_empty = gates.keys().any(|(_, c)| c.is_empty())
            || structures.keys().any(|(_, c)| c.is_empty())
            || slots.values().any(|(c, _)| c.is_empty());
        (gates.len(), structures.len(), has_empty)
    }

    #[cfg(feature = "test-util")]
    async fn paused_at(&self, stage: DeletionStage) {
        let pause = {
            let mut pause = self
                .inner
                .deletion_pause
                .lock()
                .expect("the deletion pause is not poisoned");
            pause
                .take_if(|(at, _, _)| *at == stage)
                .map(|(_, waiting, go)| (waiting, go))
        };
        if let Some((waiting, go)) = pause {
            waiting.notify_one();
            go.notified().await;
        }
    }

    async fn remove_held<T, W, F>(
        &self,
        app: &App,
        deletion: Deletion,
        remove: W,
    ) -> Result<T, AppError>
    where
        W: FnOnce(App) -> F,
        F: Future<Output = Result<T, AppError>>,
    {
        let tenant = app.store().tenant();
        let container_ids = match &deletion {
            Deletion::Block(block_id) => {
                let Some(container_id) = app.store().find_block_container_id(block_id).await?
                else {
                    return remove(app.clone()).await;
                };
                // Serialize the fast-path check and removal with `changing_structure` on
                // this container so a unit/card cannot move under a chapter/comment
                // between checking `cards` and deleting the subtree, without waiting on
                // open document work when no room-bearing block is involved.
                let structure = self.structure(tenant, &container_id).write_owned().await;
                if deletion.cards(app).await?.is_empty() {
                    let removed = remove(app.clone()).await;
                    drop(structure);
                    return removed;
                }
                drop(structure);
                vec![container_id]
            }
            Deletion::Blocks { container_id, .. } => {
                let structure = self.structure(tenant, container_id).write_owned().await;
                if deletion.cards(app).await?.is_empty() {
                    let removed = remove(app.clone()).await;
                    drop(structure);
                    return removed;
                }
                drop(structure);
                vec![container_id.clone()]
            }
            Deletion::Containers(ids) => ids.clone(),
        };
        let documents = self.writing(tenant, &container_ids).await;
        let card_ids = deletion.cards(app).await?;
        self.write_deleted(app, &card_ids).await;
        #[cfg(feature = "test-util")]
        self.paused_at(DeletionStage::BeforeRemoving).await;
        let result = remove(app.clone()).await?;
        // Mark and detach the old rooms before new document work can begin. A restored
        // card may open a new room while the old connections are still being told.
        let rooms = self.detach_deleted_rooms(app, &card_ids);
        for card_id in &card_ids {
            if let Some(mut slot) = self.existing_slot(app, card_id).await {
                self.forget(&mut slot).await;
                self.retire(&mut slot);
            }
        }
        drop(documents);
        #[cfg(feature = "test-util")]
        self.paused_at(DeletionStage::AfterRemoving).await;
        self.finish_deleted_rooms(rooms).await;
        Ok(result)
    }

    /// Writes to the cards about to be deleted what their documents hold and they do
    /// not. What cannot be written stays in the log, and the card restored opens with
    /// it; so does all of it if the store cannot say which logs are not written.
    async fn write_deleted(&self, app: &App, card_ids: &[String]) {
        let open = self.open_among(app, card_ids);
        let pending = match app.store().with_unwritten_bodies(card_ids).await {
            Ok(unwritten) => merged(open, unwritten),
            Err(error) => {
                tracing::error!(%error, "cannot find the deleted cards' unwritten logs; those closed keep them");
                open
            }
        };
        for card_id in &pending {
            let container_id = match self.slot_container_id(app, card_id).await {
                Ok(Some(container_id)) => container_id,
                Ok(None) => continue,
                Err(error) => {
                    tracing::error!(%error, card_id, "cannot find a deleted card's container; its log keeps it");
                    continue;
                }
            };
            let mut slot = self.slot_in(app, card_id, &container_id).await;
            if let Err(error) = self
                .write_pending(app, card_id, &mut slot, super::whole::DueVersion::Keep)
                .await
            {
                tracing::error!(%error, card_id, "cannot write a deleted card's body; its log keeps it");
            }
            self.retire(&mut slot);
        }
    }

    /// Those of `card_ids` whose document may hold what the card does not: open here,
    /// or with a log not written to the card, found in one query. The open ones are
    /// read first: a document that closes between the two without writing its body
    /// leaves its log for the query.
    async fn pending_among(&self, app: &App, card_ids: &[String]) -> Result<Vec<String>, AppError> {
        let open = self.open_among(app, card_ids);
        let unwritten = app.store().with_unwritten_bodies(card_ids).await?;
        Ok(merged(open, unwritten))
    }

    /// Those of `card_ids` that have a slot here: a document open, or closed for a while.
    fn open_among(&self, app: &App, card_ids: &[String]) -> Vec<String> {
        let tenant = app.store().tenant();
        let slots = self.inner.slots.lock().expect("the slots are not poisoned");
        card_ids
            .iter()
            .filter(|id| slots.contains_key(&place::key_of(tenant, id)))
            .cloned()
            .collect()
    }

    /// [`Self::write_body`] for many cards: of those that may hold something unwritten
    /// only, found at once, so a board of cards nobody edits costs one query.
    ///
    /// # Errors
    ///
    /// The errors of [`Self::write_body`], and the store's.
    pub(crate) async fn write_bodies(
        &self,
        app: &App,
        card_ids: &[String],
    ) -> Result<(), AppError> {
        for card_id in self.pending_among(app, card_ids).await? {
            self.write_body(app, &card_id).await?;
        }
        Ok(())
    }
}

/// The cards of both lists, each once.
fn merged(open: Vec<String>, unwritten: Vec<String>) -> Vec<String> {
    let mut pending: Vec<String> = open.into_iter().chain(unwritten).collect();
    pending.sort();
    pending.dedup();
    pending
}

/// Where a deletion is paused for a test ([`Collab::pause_next_deletion`]).
#[cfg(feature = "test-util")]
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeletionStage {
    /// The bodies are written, the gate is held, the store has not deleted yet.
    BeforeRemoving,
    /// The store deleted and the rooms are detached; the gate is let go and the
    /// rooms' members are not told yet.
    AfterRemoving,
}

/// A deletion paused at a stage: told when it gets there, and waiting to go on.
#[cfg(feature = "test-util")]
pub(super) type DeletionPause = (DeletionStage, Arc<Notify>, Arc<Notify>);

impl Deletion {
    async fn cards(&self, app: &App) -> Result<Vec<String>, AppError> {
        let mut cards = Vec::new();
        match self {
            Self::Block(id) => {
                // Deleted meanwhile: deleting it again takes nothing along.
                let root = match app.store().get_block(id).await {
                    Ok(root) => root,
                    Err(error) if error.is_not_found() => return Ok(cards),
                    Err(error) => return Err(error.into()),
                };
                let mut blocks = app.store().get_blocks_below(id).await?;
                blocks.push(root);
                cards.extend(
                    blocks
                        .into_iter()
                        .filter(|block| block.block_type.has_room())
                        .map(|block| block.id),
                );
            }
            Self::Blocks { block_ids, .. } => {
                for id in block_ids {
                    let root = match app.store().get_block(id).await {
                        Ok(root) => root,
                        Err(error) if error.is_not_found() => continue,
                        Err(error) => return Err(error.into()),
                    };
                    let mut blocks = app.store().get_blocks_below(id).await?;
                    blocks.push(root);
                    cards.extend(
                        blocks
                            .into_iter()
                            .filter(|block| block.block_type.has_room())
                            .map(|block| block.id),
                    );
                }
            }
            Self::Containers(ids) => {
                cards = app.store().room_block_ids_of_containers(ids).await?;
            }
        }
        cards.sort();
        cards.dedup();
        Ok(cards)
    }
}

#[cfg(test)]
mod tests {
    use fvoci_model::new_id;
    use fvoci_store::Tenant;

    use super::Collab;
    use crate::collab::worker::WorkerCommand;
    use crate::config::collab::CollabSettings;

    fn test_collab() -> Collab {
        let settings = CollabSettings {
            worker_command: Some(WorkerCommand {
                program: std::path::PathBuf::from("/bin/true"),
                args: Vec::new(),
            }),
            ..CollabSettings::default()
        };
        Collab::new(&settings).expect("collab")
    }

    #[tokio::test]
    async fn container_gate_and_structure_maps_prune_when_guards_and_cancelled_waiters_drop() {
        let collab = test_collab();
        let tenant = Tenant::System;
        let c1 = new_id();
        let c2 = new_id();

        let r1 = collab.reading(tenant, &c1).await;
        let r2 = collab.reading(tenant, &c1).await;
        let s1 = collab.structure(tenant, &c1).read_owned().await;
        let w2 = collab.writing(tenant, std::slice::from_ref(&c2)).await;

        assert_eq!(collab.inner.gates.lock().expect("gates").len(), 2);
        assert_eq!(collab.inner.structures.lock().expect("structures").len(), 1);

        // Start and cancel a waiter while `w2` holds the write lock on `c2`.
        let cancelled = tokio::time::timeout(
            std::time::Duration::from_millis(5),
            collab.reading(tenant, &c2),
        )
        .await;
        assert!(cancelled.is_err(), "read waiter must block behind write");

        drop(r1);
        assert_eq!(
            collab.inner.gates.lock().expect("gates").len(),
            2,
            "c1 gate remains while r2 still holds it"
        );
        drop(r2);
        assert_eq!(
            collab.inner.gates.lock().expect("gates").len(),
            1,
            "c1 gate pruned once last reader drops"
        );
        drop(w2);
        assert!(
            collab.inner.gates.lock().expect("gates").is_empty(),
            "c2 gate pruned even after a cancelled waiter"
        );
        drop(s1);
        assert!(
            collab
                .inner
                .structures
                .lock()
                .expect("structures")
                .is_empty(),
            "c1 structure lock pruned once last guard drops"
        );
    }

    #[test]
    #[should_panic(expected = "non-empty container_id")]
    fn empty_container_id_lock_key_is_rejected() {
        let collab = test_collab();
        let _ = collab.gate(Tenant::System, "");
    }
}
