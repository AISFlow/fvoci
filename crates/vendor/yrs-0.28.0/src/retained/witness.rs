//! Owned, archive-only identity witnesses. No witness is integrated into a store.
//! The caller holds the canonical read transaction throughout verification.
//! Charges are conservative accounting; process limits remain necessary for
//! standard-library decoder/allocator execution. A charge refusal stops proof.
use crate::block::{Block, Item, ItemContent, ItemPtr};
use crate::types::{TypePtr, TypeRef};
use crate::updates::decoder::Decode;
use crate::updates::encoder::{Encoder, EncoderV1};
use crate::{
    Any, Doc, IdSet, OffsetKind, Options, ReadTxn, StateVector, Store, Transact, TransactionMut,
    Update, ID,
};
use std::io;
use std::mem::size_of;
use std::sync::Arc;

#[derive(Debug, Clone, Copy)]
pub struct WitnessLimits {
    pub max_blocks: usize,
    pub max_roots: usize,
    pub max_table_capacity: usize,
    pub max_depth: usize,
    pub max_native_bytes: usize,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct WitnessCost {
    pub work: usize,
    pub inspected_bytes: usize,
    pub owned_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WitnessError {
    Budget,
    Unsupported(&'static str),
    Malformed(&'static str),
    Mismatch(&'static str),
}
type Proof<T> = Result<T, WitnessError>;

/// An opaque owned copy captured before the same decoded Update is applied.
/// Contains no source-store links or borrowed Branch pointers.
pub struct OwnedInputWitness {
    items: Vec<Block>,
    ds: IdSet,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct WitnessStats {
    pub canonical_blocks: usize,
    pub input_blocks: usize,
    pub comparisons: usize,
    pub splits: usize,
    pub native_bytes: usize,
    pub native_bound: usize,
    pub clones: usize,
    pub verification_transactions: usize,
}

struct Guard<'a, F> {
    limits: WitnessLimits,
    charge: &'a mut F,
    stats: WitnessStats,
}
impl<F: FnMut(WitnessCost) -> bool> Guard<'_, F> {
    fn cost(&mut self, work: usize, inspected_bytes: usize, owned_bytes: usize) -> Proof<()> {
        if (self.charge)(WitnessCost {
            work,
            inspected_bytes,
            owned_bytes,
        }) {
            Ok(())
        } else {
            Err(WitnessError::Budget)
        }
    }
    fn reserve<T>(&mut self, v: &mut Vec<T>) -> Proof<()> {
        if v.len() == v.capacity() {
            let next = v
                .capacity()
                .checked_mul(2)
                .ok_or(WitnessError::Budget)?
                .max(4);
            let growth = next.checked_sub(v.capacity()).ok_or(WitnessError::Budget)?;
            // Growth reallocates one new block of `next` elements; the old block
            // (already charged) is released after the copy. Charging the whole
            // new block keeps the cumulative charge an upper bound of what the
            // allocator hands out, and the peak covers old plus new during copy.
            self.cost(
                v.len(),
                0,
                next.checked_mul(size_of::<T>())
                    .ok_or(WitnessError::Budget)?,
            )?;
            v.try_reserve_exact(growth)
                .map_err(|_| WitnessError::Budget)?;
        }
        Ok(())
    }
    fn text(&mut self, s: &str) -> Proof<()> {
        self.cost(1, s.len(), s.len())
    }
    fn any(&mut self, value: &Any, depth: usize) -> Proof<usize> {
        if depth > self.limits.max_depth {
            return Err(WitnessError::Budget);
        }
        self.cost(1, 0, size_of::<Any>())?;
        let mut bound = 32usize;
        match value {
            Any::String(s) => {
                self.text(s)?;
                bound = add(bound, s.len())?;
            }
            Any::Buffer(v) => {
                self.cost(1, v.len(), v.len())?;
                bound = add(bound, v.len())?;
            }
            Any::Array(v) => {
                if v.len() > self.limits.max_blocks {
                    return Err(WitnessError::Budget);
                }
                self.cost(
                    v.len(),
                    0,
                    v.len()
                        .checked_mul(size_of::<Any>())
                        .ok_or(WitnessError::Budget)?,
                )?;
                for a in v.iter() {
                    bound = add(bound, self.any(a, depth + 1)?)?;
                }
            }
            Any::Map(v) => {
                if v.capacity() > self.limits.max_table_capacity {
                    return Err(WitnessError::Budget);
                }
                self.cost(
                    v.capacity(),
                    0,
                    v.capacity().checked_mul(128).ok_or(WitnessError::Budget)?,
                )?;
                for (k, a) in v.iter() {
                    self.text(k)?;
                    bound = add(bound, add(16, add(k.len(), self.any(a, depth + 1)?)?)?)?;
                }
            }
            _ => {}
        }
        Ok(bound)
    }
    fn json_len(&mut self, value: &Any) -> Proof<usize> {
        let max = self.limits.max_native_bytes;
        let mut counter = JsonCounter {
            guard: self,
            count: 0,
            max,
            failed: false,
        };
        let result = serde_json::to_writer(&mut counter, value);
        let count = counter.count;
        let failed = counter.failed;
        drop(counter);
        if failed {
            return Err(WitnessError::Budget);
        }
        result.map_err(|_| WitnessError::Malformed("JSON content"))?;
        // EncoderV1::write_json uses the same standard Serializer into a String.
        // Precharge its possible growth before that String is allocated.
        self.cost(1, 0, count.checked_mul(2).ok_or(WitnessError::Budget)?)?;
        add(count, 16)
    }
    fn type_ref(&mut self, t: &TypeRef, root: bool) -> Proof<usize> {
        match t {
            TypeRef::Array
            | TypeRef::Map
            | TypeRef::Text
            | TypeRef::XmlFragment
            | TypeRef::XmlText
            | TypeRef::XmlHook => Ok(16),
            TypeRef::XmlElement(tag) => {
                self.text(tag)?;
                add(16, tag.len())
            }
            TypeRef::Undefined if root => Ok(16),
            _ => Err(WitnessError::Unsupported("type")),
        }
    }
    fn content(&mut self, c: &ItemContent) -> Proof<usize> {
        match c {
            ItemContent::Any(v) => {
                if v.len() > self.limits.max_blocks {
                    return Err(WitnessError::Budget);
                }
                self.cost(
                    v.len(),
                    0,
                    v.len()
                        .checked_mul(size_of::<Any>())
                        .ok_or(WitnessError::Budget)?,
                )?;
                let mut n = 16;
                for a in v {
                    n = add(n, self.any(a, 0)?)?;
                }
                Ok(n)
            }
            ItemContent::JSON(v) => {
                if v.len() > self.limits.max_blocks {
                    return Err(WitnessError::Budget);
                }
                self.cost(
                    v.len(),
                    0,
                    v.len()
                        .checked_mul(size_of::<String>())
                        .ok_or(WitnessError::Budget)?,
                )?;
                let mut n = 16;
                for s in v {
                    self.text(s)?;
                    n = add(n, add(16, s.len())?)?;
                }
                Ok(n)
            }
            ItemContent::String(s) => {
                // An owned copy is a SplittableString (SmallString<[u8; 8]>)
                // clone: pinned smallvec 1.16 collects the bytes, keeping up
                // to 8 inline and otherwise reserving next_power_of_two(len).
                let len = s.as_str().len();
                let copy = if len <= 8 {
                    0
                } else {
                    len.checked_next_power_of_two()
                        .ok_or(WitnessError::Budget)?
                };
                self.cost(1, len, copy)?;
                add(16, len)
            }
            ItemContent::Binary(v) => {
                self.cost(1, v.len(), v.len())?;
                add(16, v.len())
            }
            ItemContent::Embed(a) => {
                self.any(a, 0)?;
                self.json_len(a)
            }
            ItemContent::Format(k, a) => {
                self.text(k)?;
                self.any(a, 0)?;
                add(add(16, k.len())?, self.json_len(a)?)
            }
            ItemContent::Type(t) => {
                self.cost(1, 0, size_of::<crate::branch::Branch>())?;
                self.type_ref(&t.type_ref, false)
            }
            ItemContent::Deleted(_) => Err(WitnessError::Unsupported("deleted content")),
            ItemContent::Doc(_, _) => Err(WitnessError::Unsupported("subdocument")),
        }
    }
    /// Precharges the standard `IdSet::insert` before it runs, following the
    /// paths of `IdRanges::insert_with` on the pinned smallvec 1.16 (inline
    /// capacity 1): extending the last range in clock order allocates nothing;
    /// a disjoint tail push or a general (out-of-order) insert reallocates
    /// only when the range vector is full, to `next_power_of_two(len + 1)`
    /// entries (the unit-valued overlap path coalesces into its inline
    /// temporary and never grows); a new client inserts one key into the
    /// client BTreeMap, which can allocate a leaf plus one node per split
    /// level and a new root. Malformed ranges are refused before any effect.
    fn insert(&mut self, set: &mut IdSet, id: ID, len: u32) -> Proof<()> {
        if len == 0 || id.clock.checked_add(len).is_none() {
            return Err(WitnessError::Malformed("witness range"));
        }
        let entry = size_of::<(std::ops::Range<u32>, ())>();
        match set.get(&id.client) {
            None => {
                // Pinned BTreeMap node layout envelope: 11 key/value slots, 12
                // edges, header/alignment margin. Height is bounded by a loose
                // binary bound over the clients after insertion; a split can
                // allocate one node per level plus a new root.
                let node = add(
                    add(
                        size_of::<crate::block::ClientID>(),
                        size_of::<crate::id_set::IdRange>(),
                    )?
                    .checked_mul(11)
                    .ok_or(WitnessError::Budget)?,
                    add(
                        size_of::<usize>()
                            .checked_mul(12)
                            .ok_or(WitnessError::Budget)?,
                        64,
                    )?,
                )?;
                let height = add(set.len(), 1)?.ilog2() as usize;
                self.cost(
                    add(height, 1)?,
                    0,
                    node.checked_mul(add(height, 2)?)
                        .ok_or(WitnessError::Budget)?,
                )?;
            }
            Some(ranges) => {
                let n = ranges.inner().len();
                let growth = if n == ranges.inner().capacity() {
                    n.checked_add(1)
                        .and_then(usize::checked_next_power_of_two)
                        .and_then(|c| c.checked_mul(entry))
                        .ok_or(WitnessError::Budget)?
                } else {
                    0
                };
                match ranges.iter().last().map(|(r, _)| r.clone()) {
                    Some(last) if id.clock >= last.start && id.clock <= last.end => {
                        self.cost(1, 0, 0)?
                    }
                    Some(last) if id.clock > last.end => self.cost(1, 0, growth)?,
                    _ => self.cost(add(add(n.max(1).ilog2() as usize, 1)?, n)?, 0, growth)?,
                }
            }
        }
        set.insert(id, len);
        Ok(())
    }
    /// Charges reading an item's content in place (equality checks against a
    /// stored item): the same traversal and inspected bytes as `content`, but
    /// nothing owned, because nothing is copied.
    fn borrowed_content(&mut self, c: &ItemContent) -> Proof<()> {
        let charge = &mut *self.charge;
        let mut in_place = |cost: WitnessCost| {
            charge(WitnessCost {
                owned_bytes: 0,
                ..cost
            })
        };
        let mut reader = Guard {
            limits: self.limits,
            charge: &mut in_place,
            stats: WitnessStats::default(),
        };
        reader.content(c).map(|_| ())
    }
    fn merge_ds(&mut self, ds: &mut IdSet, other: &IdSet) -> Proof<()> {
        self.cost(other.len(), 0, 0)?;
        for (client, ranges) in other.iter() {
            for r in ranges.iter() {
                let len = r
                    .end
                    .checked_sub(r.start)
                    .filter(|n| *n > 0)
                    .ok_or(WitnessError::Malformed("DS range"))?;
                self.insert(ds, ID::new(*client, r.start), len)?;
            }
        }
        Ok(())
    }
    fn splice(&mut self, item: &Item) -> Proof<()> {
        // Standard splice copies both halves, scans UTF16 text and creates a
        // new Item plus cloned parent/key handles before normalization.
        self.content(&item.content)?;
        if let ItemContent::String(s) = &item.content {
            self.cost(
                1,
                s.as_str()
                    .len()
                    .checked_mul(2)
                    .ok_or(WitnessError::Budget)?,
                0,
            )?;
        }
        if let TypePtr::Named(name) = &item.parent {
            self.text(name)?;
        }
        if let Some(key) = &item.parent_sub {
            self.text(key)?;
        }
        self.cost(1, 0, add(size_of::<Item>(), 64)?)
    }
    fn owned_item(&mut self, source: &Item, parent: TypePtr) -> Proof<(Box<Item>, usize)> {
        self.cost(1, 0, size_of::<Item>())?;
        let mut bound = add(256, self.content(&source.content)?)?;
        if source.len == 0
            || source.id.clock.checked_add(source.len).is_none()
            || source.content.len(OffsetKind::Utf16) != source.len
        {
            return Err(WitnessError::Malformed("item range"));
        }
        if let TypePtr::Named(name) = &parent {
            self.text(name)?;
            bound = add(bound, name.len())?;
        }
        if let Some(key) = &source.parent_sub {
            self.text(key)?;
            bound = add(bound, key.len())?;
        }
        let item = Item::new(
            source.id,
            None,
            source.origin,
            None,
            source.right_origin,
            parent,
            source.parent_sub.clone(),
            source.content.clone(),
        )
        .ok_or(WitnessError::Malformed("empty item"))?;
        if let ItemContent::Type(branch) = &item.content {
            if !std::ptr::eq(
                &*branch.item.ok_or(WitnessError::Malformed("owned type"))?,
                item.as_ref(),
            ) {
                return Err(WitnessError::Malformed("owned type"));
            }
        }
        Ok((item, bound))
    }
}
fn add(a: usize, b: usize) -> Proof<usize> {
    a.checked_add(b).ok_or(WitnessError::Budget)
}

struct JsonCounter<'a, 'b, F> {
    guard: &'a mut Guard<'b, F>,
    count: usize,
    max: usize,
    failed: bool,
}
impl<F: FnMut(WitnessCost) -> bool> io::Write for JsonCounter<'_, '_, F> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let next = self.count.checked_add(bytes.len());
        if next.is_none() || next.unwrap() > self.max || self.guard.cost(1, bytes.len(), 0).is_err()
        {
            self.failed = true;
            return Err(io::Error::new(io::ErrorKind::Other, "witness JSON budget"));
        }
        self.count = next.unwrap();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Capture before applying the SAME standard decoded Update once. Failure never
/// grants permission to omit this input from a complete proof.
pub fn capture_owned_input(
    update: &Update,
    limits: WitnessLimits,
    charge: &mut impl FnMut(WitnessCost) -> bool,
) -> Proof<OwnedInputWitness> {
    let mut b = Guard {
        limits,
        charge,
        stats: WitnessStats::default(),
    };
    if update.blocks.clients.capacity() > limits.max_table_capacity {
        return Err(WitnessError::Budget);
    }
    b.cost(update.blocks.clients.capacity(), 0, 0)?;
    let mut out = OwnedInputWitness {
        items: Vec::new(),
        ds: IdSet::default(),
    };
    b.merge_ds(&mut out.ds, &update.delete_set)?;
    for (client, blocks) in &update.blocks.clients {
        for block in blocks {
            if out.items.len() >= limits.max_blocks {
                return Err(WitnessError::Budget);
            }
            b.reserve(&mut out.items)?;
            let item = match block {
                Block::Item(item) => item,
                Block::GC(_) => return Err(WitnessError::Unsupported("GC input")),
                Block::Skip(_) => return Err(WitnessError::Unsupported("Skip input")),
            };
            if item.id.client != *client || matches!(item.parent, TypePtr::Branch(_)) {
                return Err(WitnessError::Malformed("input owner"));
            }
            let (item, _) = b.owned_item(item, item.parent.clone())?;
            out.items.push(Block::Item(item));
        }
    }
    Ok(out)
}

struct Census {
    witness: OwnedInputWitness,
    intervals: IdSet,
    sv: StateVector,
    roots: Vec<(Arc<str>, TypeRef)>,
    native_bound: usize,
}
fn census<T: ReadTxn, F: FnMut(WitnessCost) -> bool>(
    tx: &T,
    b: &mut Guard<'_, F>,
) -> Proof<Census> {
    let store = tx.store();
    if store.pending_update().is_some() || store.pending_ds().is_some() {
        return Err(WitnessError::Unsupported("pending"));
    }
    let capacity = add(
        store.blocks.retained_client_capacity(),
        store.types.capacity(),
    )?;
    if capacity > b.limits.max_table_capacity || store.types.len() > b.limits.max_roots {
        return Err(WitnessError::Budget);
    }
    b.cost(
        capacity,
        0,
        store
            .blocks
            .retained_client_capacity()
            .checked_mul(512)
            .ok_or(WitnessError::Budget)?,
    )?;
    let mut out = Census {
        witness: OwnedInputWitness {
            items: Vec::new(),
            ds: IdSet::default(),
        },
        intervals: IdSet::default(),
        sv: tx.state_vector(),
        roots: Vec::new(),
        native_bound: 128,
    };
    for (name, branch) in &store.types {
        b.reserve(&mut out.roots)?;
        b.text(name)?;
        b.type_ref(&branch.type_ref, true)?;
        out.roots.push((name.clone(), branch.type_ref.clone()));
    }
    for (client, blocks) in store.blocks.iter() {
        out.native_bound = add(out.native_bound, 128)?;
        for block in blocks.iter() {
            b.cost(1, 0, 0)?;
            if out.witness.items.len() >= b.limits.max_blocks {
                return Err(WitnessError::Budget);
            }
            let item = match block.as_ref() {
                Block::Item(i) => i,
                Block::GC(_) => return Err(WitnessError::Unsupported("GC current")),
                Block::Skip(_) => return Err(WitnessError::Unsupported("Skip current")),
            };
            if item.id.client != *client {
                return Err(WitnessError::Malformed("current client"));
            }
            let parent = match &item.parent {
                TypePtr::Branch(branch) => match super::identity(branch) {
                    Some(super::BranchIdentity::Root(name)) => {
                        b.text(name)?;
                        TypePtr::Named(name.into())
                    }
                    Some(super::BranchIdentity::Nested(id)) => TypePtr::ID(id),
                    None => return Err(WitnessError::Unsupported("current owner")),
                },
                _ => return Err(WitnessError::Malformed("unresolved current owner")),
            };
            b.reserve(&mut out.witness.items)?;
            let (owned, bound) = b.owned_item(item, parent)?;
            out.native_bound = add(out.native_bound, bound)?;
            b.insert(&mut out.intervals, item.id, item.len)?;
            if item.is_deleted() {
                b.insert(&mut out.witness.ds, item.id, item.len)?;
            }
            out.witness.items.push(Block::Item(owned));
        }
    }
    let mut sv_intervals = IdSet::default();
    for (client, clock) in out.sv.iter() {
        b.insert(&mut sv_intervals, ID::new(*client, 0), *clock)?;
    }
    if out.intervals != sv_intervals {
        return Err(WitnessError::Mismatch("raw/SV coverage"));
    }
    if out.native_bound > b.limits.max_native_bytes {
        return Err(WitnessError::Budget);
    }
    Ok(out)
}

fn materialize<F: FnMut(WitnessCost) -> bool>(
    store: &mut Store,
    at: ID,
    start: bool,
    b: &mut Guard<'_, F>,
) -> Proof<()> {
    b.cost(1, 0, 512)?;
    let list = store
        .blocks
        .get_client(&at.client)
        .ok_or(WitnessError::Mismatch("missing client"))?;
    let count = list.len();
    let capacity = list.retained_capacity();
    if count == 0 || count > b.limits.max_blocks {
        return Err(WitnessError::Budget);
    }
    b.cost((count.ilog2() + 2) as usize, 0, 0)?;
    let slice = if start {
        store.blocks.get_item_clean_start(&at)
    } else {
        store.blocks.get_item_clean_end(&at)
    }
    .ok_or(WitnessError::Mismatch("missing/GC boundary"))?;
    // Even an aligned standard materialize clones linked_by. Supported archive
    // native stores have no weak/link operational dependencies.
    if slice.ptr.info.is_linked() {
        return Err(WitnessError::Unsupported("linked materialization"));
    }
    let splits = usize::from(!slice.adjacent_left()) + usize::from(!slice.adjacent_right());
    if splits != 0 {
        b.cost(store.blocks.retained_client_capacity(), 0, 0)?;
        let total = store
            .blocks
            .iter()
            .try_fold(0usize, |n, (_, list)| add(n, list.len()))?;
        if add(total, splits)? > b.limits.max_blocks {
            return Err(WitnessError::Budget);
        }
        for _ in 0..splits {
            b.splice(&slice.ptr)?;
        }
        let required = add(count, splits)?;
        // Target/toolchain-specific: Rust48a229cea RawVec grow_amortized and
        // Global allocator request max(2*capacity, required, minimum4) for Block.
        // This is not a portable Vec API guarantee or physical heap accounting.
        let growth = if required > capacity {
            let next = capacity
                .checked_mul(2)
                .ok_or(WitnessError::Budget)?
                .max(required)
                .max(4);
            next.checked_mul(size_of::<Block>())
                .ok_or(WitnessError::Budget)?
        } else {
            0
        };
        let shifts = required.checked_mul(splits).ok_or(WitnessError::Budget)?;
        b.cost(
            add(shifts, if growth != 0 { count } else { 0 })?,
            shifts
                .checked_mul(size_of::<Block>())
                .ok_or(WitnessError::Budget)?,
            growth,
        )?;
    }
    let clock = slice.clock_start();
    let len = slice.len();
    if let ItemContent::String(s) = &slice.ptr.content {
        // The post-materialize UTF16 validation scans even aligned Items.
        // Original UTF8 bytes bound any resulting substring; reserve before
        // the standard call, independently of split/copy reservations.
        b.cost(1, s.as_str().len(), 0)?;
    }
    #[cfg(test)]
    effect_probe::hit(0);
    let ptr = store.materialize(slice);
    if ptr.id().clock != clock || ptr.len() != len || ptr.content.len(OffsetKind::Utf16) != len {
        return Err(WitnessError::Mismatch("surrogate/excess boundary"));
    }
    Ok(())
}
fn compare<F: FnMut(WitnessCost) -> bool>(
    tx: &mut TransactionMut,
    witnesses: Vec<OwnedInputWitness>,
    expected: &IdSet,
    expected_ds: &IdSet,
    b: &mut Guard<'_, F>,
) -> Proof<()> {
    let mut items = Vec::new();
    let mut ds = IdSet::default();
    for witness in witnesses {
        b.merge_ds(&mut ds, &witness.ds)?;
        for item in witness.items {
            b.reserve(&mut items)?;
            items.push(item);
        }
    }
    if &ds != expected_ds {
        return Err(WitnessError::Mismatch("DS"));
    }
    for block in &items {
        let item = match block {
            Block::Item(i) => i,
            _ => return Err(WitnessError::Unsupported("nonitem witness")),
        };
        let last = item
            .id
            .clock
            .checked_add(item.len)
            .and_then(|v| v.checked_sub(1))
            .ok_or(WitnessError::Malformed("witness range"))?;
        materialize(&mut tx.store, item.id, true, b)?;
        materialize(&mut tx.store, ID::new(item.id.client, last), false, b)?;
        if let Some(at) = item.origin {
            materialize(&mut tx.store, at, false, b)?;
        }
        if let Some(at) = item.right_origin {
            materialize(&mut tx.store, at, true, b)?;
        }
    }
    let mut i = 0;
    while i < items.len() {
        let (id, len) = match &items[i] {
            Block::Item(item) => (item.id, item.len),
            _ => return Err(WitnessError::Unsupported("nonitem witness")),
        };
        let loaded = tx
            .store
            .blocks
            .get_item(&id)
            .ok_or(WitnessError::Mismatch("missing/GC"))?;
        if loaded.id() != &id {
            return Err(WitnessError::Mismatch("unaligned"));
        }
        let loaded_len = loaded.len();
        if loaded_len < len {
            if items.len() >= b.limits.max_blocks {
                return Err(WitnessError::Budget);
            }
            b.reserve(&mut items)?;
        }
        let item = match &mut items[i] {
            Block::Item(i) => i,
            _ => return Err(WitnessError::Unsupported("nonitem witness")),
        };
        if matches!(item.parent, TypePtr::Branch(_)) {
            return Err(WitnessError::Malformed("resolved before split"));
        }
        if loaded_len < item.len {
            b.splice(item)?;
            let mut ptr = ItemPtr::from(&mut *item);
            #[cfg(test)]
            effect_probe::hit(1);
            let right = ptr
                .splice(loaded_len, OffsetKind::Utf16)
                .ok_or(WitnessError::Mismatch("split"))?;
            if item.content.len(OffsetKind::Utf16) != item.len
                || right.content.len(OffsetKind::Utf16) != right.len
            {
                return Err(WitnessError::Mismatch("surrogate/excess split"));
            }
            b.stats.splits += 1;
            items.push(Block::Item(right));
        } else if loaded_len > item.len {
            return Err(WitnessError::Mismatch("excess"));
        }
        i += 1;
    }
    let mut covered = IdSet::default();
    for block in &mut items {
        b.cost(1, 0, 512)?;
        if Update::missing_dependency(block, &mut tx.store)
            .map_err(|_| WitnessError::Mismatch("parent"))?
            .is_some()
        {
            return Err(WitnessError::Mismatch("dependency"));
        }
        let item = match block {
            Block::Item(i) => i,
            _ => return Err(WitnessError::Unsupported("nonitem witness")),
        };
        let loaded = tx
            .store
            .blocks
            .get_item(&item.id)
            .ok_or(WitnessError::Mismatch("missing/GC"))?;
        b.borrowed_content(&item.content)?;
        b.borrowed_content(&loaded.content)?;
        let content_equal = match (&item.content, &loaded.content) {
            (ItemContent::Type(a), ItemContent::Type(z)) => a.type_ref == z.type_ref,
            (a, z) => a == z,
        };
        if !matches!(item.parent, TypePtr::Branch(_))
            || loaded.id() != &item.id
            || loaded.len() != item.len
            || loaded.parent != item.parent
            || loaded.parent_sub != item.parent_sub
            || loaded.origin != item.origin
            || loaded.right_origin != item.right_origin
            || !content_equal
        {
            return Err(WitnessError::Mismatch("identity/content"));
        }
        b.stats.comparisons += 1;
        b.insert(&mut covered, item.id, item.len)?;
    }
    if &covered != expected {
        return Err(WitnessError::Mismatch("missing/excess coverage"));
    }
    // Owned boxes and all temporary scratch pointers drop before the held TX.
    Ok(())
}

// Pinned Rust48a229cea/std hashbrown0.17.1 Linux table-layout bound. Fresh
// tables grow geometrically; charge all growth buffers, control groups and
// alignment. This is conservative requested-layout accounting, not allocator
// metadata/physical RSS or a portable standard-library growth guarantee.
fn table_bytes<T>(entries: usize) -> Proof<usize> {
    if entries == 0 {
        return Ok(0);
    }
    let slots = entries
        .checked_mul(4)
        .and_then(usize::checked_next_power_of_two)
        .ok_or(WitnessError::Budget)?
        .max(16);
    let slot = add(size_of::<T>(), add(std::mem::align_of::<T>().max(32), 1)?)?;
    slots
        .checked_mul(2)
        .and_then(|n| n.checked_mul(slot))
        .ok_or(WitnessError::Budget)
}
fn reserve_scratch<F: FnMut(WitnessCost) -> bool>(
    canonical: &Census,
    b: &mut Guard<'_, F>,
) -> Proof<()> {
    let roots = canonical.roots.len();
    let items = canonical.witness.items.len();
    let clients = canonical.sv.len();
    // Empty Doc: Arc StoreInner + Arc Options + GUID string/Arc, held stack
    // document/transactions and standard empty collection descriptors.
    let base = add(
        size_of::<crate::store::StoreInner>(),
        add(
            size_of::<Options>(),
            add(
                size_of::<Doc>(),
                add(
                    size_of::<TransactionMut>()
                        .checked_mul(2)
                        .ok_or(WitnessError::Budget)?,
                    256,
                )?,
            )?,
        )?,
    )?;
    let root_branches = roots
        .checked_mul(add(size_of::<crate::branch::Branch>(), 64)?)
        .ok_or(WitnessError::Budget)?;
    let root_table = table_bytes::<(Arc<str>, Box<crate::branch::Branch>)>(roots)?;
    let client_tables = add(
        table_bytes::<(crate::block::ClientID, std::collections::VecDeque<Block>)>(clients)?,
        table_bytes::<(crate::block::ClientID, u32)>(clients)?
            .checked_mul(4)
            .ok_or(WitnessError::Budget)?,
    )?;
    // Decoder and store simultaneously own separate block collections;
    // integration/transaction cleanup use further temporary block/ID vectors.
    let rows = items
        .checked_mul(add(
            size_of::<Item>(),
            add(
                size_of::<Block>()
                    .checked_mul(6)
                    .ok_or(WitnessError::Budget)?,
                add(
                    size_of::<ID>().checked_mul(4).ok_or(WitnessError::Budget)?,
                    64,
                )?,
            )?,
        )?)
        .ok_or(WitnessError::Budget)?;
    // At most one changed parent/key per actual Item. Partitioned owner maps
    // and transaction change sets are bounded by the per-entry smallest table,
    // rather than assuming every owner shares one dense HashMap.
    let parent_slot = add(
        size_of::<TypePtr>(),
        size_of::<std::collections::HashSet<Option<Arc<str>>>>(),
    )?;
    let map_slot = size_of::<(Arc<str>, ItemPtr)>();
    let sets = items
        .checked_mul(add(
            table_bytes::<(Option<Arc<str>>, ())>(1)?,
            add(
                table_bytes::<(Arc<str>, ItemPtr)>(1)?,
                add(parent_slot, map_slot)?
                    .checked_mul(32)
                    .ok_or(WitnessError::Budget)?,
            )?,
        )?)
        .ok_or(WitnessError::Budget)?;
    // Pinned alloc BTree node has eleven key/value slots and twelve child
    // edges. Per-item reservation covers decoder/transaction range maps and
    // range-vector growth, independently from encoded wire length.
    let node = add(
        add(
            size_of::<crate::block::ClientID>(),
            size_of::<crate::ids::IdRanges<()>>(),
        )?
        .checked_mul(11)
        .ok_or(WitnessError::Budget)?,
        add(
            size_of::<usize>()
                .checked_mul(12)
                .ok_or(WitnessError::Budget)?,
            64,
        )?,
    )?;
    let ranges = items
        .checked_mul(node.checked_mul(4).ok_or(WitnessError::Budget)?)
        .ok_or(WitnessError::Budget)?;
    let structural = add(
        base,
        add(
            root_branches,
            add(
                root_table,
                add(client_tables, add(rows, add(sets, ranges)?)?)?,
            )?,
        )?,
    )?;
    b.cost(
        add(
            roots.checked_mul(roots).ok_or(WitnessError::Budget)?,
            items
                .checked_mul(add(clients, 2)?)
                .ok_or(WitnessError::Budget)?,
        )?,
        0,
        structural,
    )?;
    for (name, kind) in &canonical.roots {
        b.text(name)?;
        b.type_ref(kind, true)?;
    }
    for block in &canonical.witness.items {
        let item = match block {
            Block::Item(i) => i,
            _ => return Err(WitnessError::Unsupported("scratch block")),
        };
        // All decoded payload containers/strings/Branch copies are charged
        // before the decoder, separately from their wire-size envelope.
        b.content(&item.content)?;
        if let TypePtr::Named(name) = &item.parent {
            b.text(name)?;
        }
        if let Some(key) = &item.parent_sub {
            b.text(key)?;
        }
        b.cost(1, 0, 128)?;
    }
    Ok(())
}

/// Prove canonical→scratch fidelity AND original-input identity in one scratch
/// transaction. Source is the SAME held read transaction for the entire call.
/// Success is identity proof, not authorization or archive authenticity.
pub fn verify_owned_witnesses<T: ReadTxn>(
    source: &T,
    witnesses: Vec<OwnedInputWitness>,
    limits: WitnessLimits,
    charge: &mut impl FnMut(WitnessCost) -> bool,
) -> Proof<WitnessStats> {
    let mut b = Guard {
        limits,
        charge,
        stats: WitnessStats::default(),
    };
    if witnesses.capacity() > limits.max_table_capacity || witnesses.len() > limits.max_blocks {
        return Err(WitnessError::Budget);
    }
    b.cost(witnesses.capacity(), 0, 0)?;
    // This actual source census precedes ANY encoder/clone allocation.
    let canonical = census(source, &mut b)?;
    b.stats.canonical_blocks = canonical.witness.items.len();
    b.stats.input_blocks = witnesses
        .iter()
        .try_fold(0usize, |n, w| add(n, w.items.len()))?;
    if b.stats.input_blocks > limits.max_blocks {
        return Err(WitnessError::Budget);
    }
    let bound = canonical.native_bound;
    b.stats.native_bound = bound;
    b.cost(
        1,
        bound,
        bound.max(1024).checked_mul(2).ok_or(WitnessError::Budget)?,
    )?;
    let mut encoder = EncoderV1::new();
    source.encode_state_as_update(&StateVector::default(), &mut encoder);
    let bytes = encoder.to_vec();
    if bytes.len() > bound
        || bytes.capacity() > bound.max(1024).checked_mul(2).ok_or(WitnessError::Budget)?
    {
        return Err(WitnessError::Mismatch("standard encode bound"));
    }
    b.stats.native_bytes = bytes.len();
    // Conservative delegated decoder/store accounting before standard execution.
    b.cost(
        canonical.witness.items.len(),
        bytes.len(),
        bound.checked_mul(2).ok_or(WitnessError::Budget)?,
    )?;
    reserve_scratch(&canonical, &mut b)?;
    #[cfg(test)]
    effect_probe::hit(2);
    let scratch = Doc::with_options(Options {
        skip_gc: true,
        offset_kind: OffsetKind::Utf16,
        cleanup_formatting: false,
        ..Options::default()
    });
    {
        let mut tx = scratch.transact_mut();
        for (name, kind) in &canonical.roots {
            tx.store.get_or_create_type(name.clone(), kind.clone());
        }
        tx.apply_update(
            Update::decode_v1(&bytes).map_err(|_| WitnessError::Malformed("clone decode"))?,
        )
        .map_err(|_| WitnessError::Malformed("clone apply"))?;
    }
    b.stats.clones = 1;
    {
        let mut tx = scratch.transact_mut();
        b.stats.verification_transactions = 1;
        let actual = census(&tx, &mut b)?;
        b.cost(
            canonical
                .roots
                .len()
                .checked_mul(actual.roots.len())
                .ok_or(WitnessError::Budget)?,
            0,
            0,
        )?;
        if actual.intervals != canonical.intervals
            || actual.sv != canonical.sv
            || actual.witness.ds != canonical.witness.ds
            || actual.roots.len() != canonical.roots.len()
            || !canonical
                .roots
                .iter()
                .all(|(name, kind)| actual.roots.iter().any(|(n, k)| n == name && k == kind))
        {
            return Err(WitnessError::Mismatch("canonical/clone census"));
        }
        // actual's owned boxes drop now, before comparison can materialize.
        drop(actual);
        let mut canonical_ds = IdSet::default();
        b.merge_ds(&mut canonical_ds, &canonical.witness.ds)?;
        b.cost(1, 0, size_of::<OwnedInputWitness>())?;
        compare(
            &mut tx,
            vec![canonical.witness],
            &canonical.intervals,
            &canonical_ds,
            &mut b,
        )?;
        compare(
            &mut tx,
            witnesses,
            &canonical.intervals,
            &canonical_ds,
            &mut b,
        )?;
    }
    Ok(b.stats)
}

#[cfg(test)]
mod effect_probe {
    use std::cell::Cell;
    std::thread_local! { static COUNTS: Cell<[usize; 3]> = const { Cell::new([0; 3]) }; }
    pub(super) fn hit(effect: usize) {
        COUNTS.with(|c| {
            let mut v = c.get();
            v[effect] += 1;
            c.set(v);
        });
    }
    pub(super) fn reset() {
        COUNTS.with(|c| c.set([0; 3]));
    }
    pub(super) fn counts() -> [usize; 3] {
        COUNTS.with(Cell::get)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::ClientID;
    use crate::updates::encoder::Encode;
    use crate::{GetString, Map, Snapshot, Text, XmlFragment, XmlTextPrelim};
    use std::collections::HashMap;
    use std::iter::FromIterator;

    fn limits() -> WitnessLimits {
        WitnessLimits {
            max_blocks: 100_000,
            max_roots: 128,
            max_table_capacity: 100_000,
            max_depth: 16,
            max_native_bytes: 8 * 1024 * 1024,
        }
    }
    fn doc(client: u64, gc: bool) -> Doc {
        Doc::with_options(Options {
            client_id: ClientID::new(client),
            skip_gc: !gc,
            cleanup_formatting: false,
            offset_kind: OffsetKind::Utf16,
            ..Options::default()
        })
    }
    fn id(client: u64, clock: u32) -> ID {
        ID::new(ClientID::new(client), clock)
    }
    fn bytes(d: &Doc) -> Vec<u8> {
        d.transact()
            .encode_state_as_update_v1(&StateVector::default())
    }
    fn incoming(d: &Doc, wire: &[u8]) -> OwnedInputWitness {
        let update = Update::decode_v1(wire).unwrap();
        let witness = capture_owned_input(&update, limits(), &mut |_| true).unwrap();
        d.transact_mut().apply_update(update).unwrap();
        witness
    }
    fn check(d: &Doc, witnesses: Vec<OwnedInputWitness>) -> Proof<WitnessStats> {
        let before = bytes(d);
        let snapshot = d.transact().snapshot();
        let result = verify_owned_witnesses(&d.transact(), witnesses, limits(), &mut |_| true);
        assert_eq!(bytes(d), before);
        assert_eq!(d.transact().snapshot(), snapshot);
        result
    }

    #[test]
    fn native_archive_owned_witness_public_paragraph_unicode_marks_deleted_history() {
        // Independent IDs/literals before writer output: paragraph10:0,
        // XmlText10:1, A at10:2, Unicode2..7; marks10:11/12; savedSV3.
        let writer = doc(10, false);
        let root = writer.get_or_insert_xml_fragment("prosemirror");
        let paragraph = root.push_back(
            &mut writer.transact_mut(),
            crate::XmlElementPrelim::empty("paragraph"),
        );
        let text = paragraph.push_back(&mut writer.transact_mut(), XmlTextPrelim::new("A한글🙂"));
        let cut = Snapshot::decode_v1(&[0, 1, 10, 3]).unwrap();
        let baseline = bytes(&writer);
        let sv = writer.transact().state_vector();
        text.insert(&mut writer.transact_mut(), 5, " 끝🧪");
        text.format(
            &mut writer.transact_mut(),
            1,
            2,
            HashMap::from([("bold".into(), Any::Bool(true))]),
        );
        text.remove_range(&mut writer.transact_mut(), 0, 1);
        let tail = writer.transact().encode_state_as_update_v1(&sv);
        let source = doc(20, false);
        source.get_or_insert_xml_fragment("prosemirror");
        let witnesses = vec![incoming(&source, &baseline), incoming(&source, &tail)];
        {
            let tx = source.transact();
            for (clock, expected) in [(11, Any::Bool(true)), (12, Any::Null)] {
                assert!(
                    matches!(&tx.store().blocks.get_item(&id(10,clock)).unwrap().content,
                    ItemContent::Format(key,value) if key.as_ref()=="bold" && value.as_ref()==&expected)
                );
            }
            let a = tx.store().blocks.get_item(&id(10, 2)).unwrap();
            assert!(a.is_deleted());
            assert!(matches!(&a.content,ItemContent::String(s) if s.as_str()=="A"));
        }
        let plain: &crate::TextRef = text.as_ref();
        assert_eq!(plain.get_string(&writer.transact()), "한글🙂 끝🧪");
        assert_eq!(
            text.get_string(&writer.transact()),
            "<bold>한글</bold>🙂 끝🧪"
        );
        let stats = check(&source, witnesses).unwrap();
        assert_eq!(stats.clones, 1);
        assert_eq!(stats.verification_transactions, 1);
        assert!(stats.splits > 0 && stats.comparisons >= 12);
        assert!(stats.native_bytes <= stats.native_bound);
        // Standard historical reconstruction remains independent of the proof.
        let historical = doc(30, false);
        historical.get_or_insert_xml_fragment("prosemirror");
        historical
            .transact_mut()
            .apply_update(Update::decode_v1(&bytes(&source)).unwrap())
            .unwrap();
        let prefix = {
            let mut tx = historical.transact_mut();
            tx.materialize_snapshot(&cut);
            let mut e = EncoderV1::new();
            tx.encode_state_from_snapshot(&cut, &mut e).unwrap();
            e.to_vec()
        };
        let restored = doc(40, false);
        restored
            .transact_mut()
            .apply_update(Update::decode_v1(&prefix).unwrap())
            .unwrap();
        let tx = restored.transact();
        assert_eq!(tx.snapshot(), cut);
        assert!(
            matches!(&tx.store().blocks.get_item(&id(10,0)).unwrap().content,
            ItemContent::Type(t) if t.type_ref==TypeRef::XmlElement("paragraph".into()))
        );
        assert!(
            matches!(&tx.store().blocks.get_item(&id(10,1)).unwrap().content,
            ItemContent::Type(t) if t.type_ref==TypeRef::XmlText)
        );
        assert!(
            matches!(&tx.store().blocks.get_item(&id(10,2)).unwrap().content,
            ItemContent::String(s) if s.as_str()=="A")
        );
    }

    #[test]
    fn native_archive_owned_witness_public_losing_map_inherited_replay() {
        // Independent versions10:0old,20:0loser,30:0winner, all keyref;
        // inherited inputs omit parent/key, but their canonical versions persist.
        let writer = doc(10, false);
        let map = writer.get_or_insert_map("metadata");
        map.insert(&mut writer.transact_mut(), "ref", "old한글🙂");
        let baseline = bytes(&writer);
        let mut updates = vec![baseline.clone()];
        for (client, value) in [(20, "losing🧪"), (30, "winner🙂")] {
            let peer = doc(client, false);
            peer.transact_mut()
                .apply_update(Update::decode_v1(&baseline).unwrap())
                .unwrap();
            let cut = peer.transact().state_vector();
            peer.get_or_insert_map("metadata")
                .insert(&mut peer.transact_mut(), "ref", value);
            let wire = peer.transact().encode_state_as_update_v1(&cut);
            writer
                .transact_mut()
                .apply_update(Update::decode_v1(&wire).unwrap())
                .unwrap();
            updates.push(wire);
        }
        let cut = writer.transact().state_vector();
        map.remove(&mut writer.transact_mut(), "ref");
        updates.push(writer.transact().encode_state_as_update_v1(&cut));
        updates.push(updates[1].clone()); // identical inherited replay
        let source = doc(40, false);
        source.get_or_insert_map("metadata");
        let witnesses: Vec<_> = updates.iter().map(|u| incoming(&source, u)).collect();
        {
            let tx = source.transact();
            for (client, value) in [(10, "old한글🙂"), (20, "losing🧪"), (30, "winner🙂")] {
                let item = tx.store().blocks.get_item(&id(client, 0)).unwrap();
                assert!(item.is_deleted());
                assert_eq!(item.parent_sub.as_deref(), Some("ref"));
                assert!(matches!(&item.content,ItemContent::Any(v) if v==&vec![Any::from(value)]));
            }
        }
        let stats = check(&source, witnesses).unwrap();
        assert_eq!(stats.input_blocks, 4);
        assert_eq!(stats.canonical_blocks, 3);
        assert_eq!(stats.comparisons, 7);
    }

    #[test]
    fn native_archive_owned_witness_public_forged_owner_key_origin_and_ds_rejected() {
        let source = doc(10, false);
        source
            .get_or_insert_map("metadata")
            .insert(&mut source.transact_mut(), "ref", "한글🙂");
        let wire = bytes(&source);
        for mode in 0..5 {
            let mut witness =
                capture_owned_input(&Update::decode_v1(&wire).unwrap(), limits(), &mut |_| true)
                    .unwrap();
            let item = match &mut witness.items[0] {
                Block::Item(i) => i,
                _ => unreachable!(),
            };
            match mode {
                0 => item.parent = TypePtr::Named("foreign-root".into()),
                1 => item.parent_sub = Some("forged-key".into()),
                2 => item.origin = Some(id(99, 0)),
                3 => item.right_origin = Some(id(99, 0)),
                _ => witness.ds.insert(id(10, 0), 1),
            }
            assert!(
                check(&source, vec![witness]).is_err(),
                "forgery mode{}",
                mode
            );
        }
        assert!(check(&source, vec![]).is_err());
    }

    #[test]
    fn native_archive_owned_witness_public_source_gap_precedes_encode_and_clone() {
        // Independent source A0, omitted B1, C2. Actual ordinary diff atSV2
        // applied to A-only yields rawA0+Skip1+C2/SV1, never synthesizedSkip.
        let writer = doc(10, false);
        let map = writer.get_or_insert_map("metadata");
        map.insert(&mut writer.transact_mut(), "a", "A");
        let source = doc(20, false);
        source.get_or_insert_map("metadata");
        let a = incoming(&source, &bytes(&writer));
        map.insert(&mut writer.transact_mut(), "b", "B");
        map.insert(&mut writer.transact_mut(), "c", "C");
        let sv = StateVector::from_iter([(ClientID::new(10), 2)]);
        let suffix = crate::diff_updates_v1(&bytes(&writer), &sv.encode_v1()).unwrap();
        source
            .transact_mut()
            .apply_update(Update::decode_v1(&suffix).unwrap())
            .unwrap();
        {
            let tx = source.transact();
            let rows: Vec<_> = tx
                .store()
                .blocks
                .get_client(&ClientID::new(10))
                .unwrap()
                .iter()
                .map(|r| match r.as_ref() {
                    Block::Item(i) => (i.id, i.len, false),
                    Block::Skip(r) => (r.id(), r.len, true),
                    _ => panic!("ordinary source fixture has noGC"),
                })
                .collect();
            assert_eq!(
                rows,
                vec![
                    (id(10, 0), 1, false),
                    (id(10, 1), 1, true),
                    (id(10, 2), 1, false)
                ]
            );
            assert_eq!(tx.state_vector().get(&ClientID::new(10)), 1);
            assert!(tx.store().pending_update().is_none() && tx.store().pending_ds().is_none());
            let mut charge = |_| true;
            let mut guard = Guard {
                limits: limits(),
                charge: &mut charge,
                stats: WitnessStats::default(),
            };
            assert!(matches!(
                census(&tx, &mut guard),
                Err(WitnessError::Unsupported("Skip current"))
            ));
            assert_eq!(guard.stats.clones, 0);
            assert_eq!(guard.stats.verification_transactions, 0);
        }
        assert_eq!(
            check(&source, vec![a]).err(),
            Some(WitnessError::Unsupported("Skip current"))
        );
    }

    #[test]
    fn native_archive_owned_witness_public_canonical_pass_refuses_same_id_changed_clone() {
        // Same native10:0/rootmetadata/keya; the clone value must be A, not Z.
        let source = doc(10, false);
        source
            .get_or_insert_map("metadata")
            .insert(&mut source.transact_mut(), "a", "A");
        let changed = doc(10, false);
        changed
            .get_or_insert_map("metadata")
            .insert(&mut changed.transact_mut(), "a", "Z");
        let before = bytes(&source);
        let mut charge = |_| true;
        let mut guard = Guard {
            limits: limits(),
            charge: &mut charge,
            stats: WitnessStats::default(),
        };
        let canonical = census(&source.transact(), &mut guard).unwrap();
        let actual = census(&changed.transact(), &mut guard).unwrap();
        assert!(actual.intervals == canonical.intervals && actual.sv == canonical.sv);
        assert!(actual.witness.ds == canonical.witness.ds);
        drop(actual);
        let mut ds = IdSet::default();
        guard.merge_ds(&mut ds, &canonical.witness.ds).unwrap();
        let result = compare(
            &mut changed.transact_mut(),
            vec![canonical.witness],
            &canonical.intervals,
            &ds,
            &mut guard,
        );
        assert_eq!(
            result.err(),
            Some(WitnessError::Mismatch("identity/content"))
        );
        assert_eq!(bytes(&source), before);
    }

    #[test]
    fn native_archive_owned_witness_public_standard_json_and_native_precharge_caps() {
        // Literal JSON escaping BEFORE serializer output: control, quote,
        // backslash, Korean/emoji and newline. Standard serialization only.
        let expected_json = r#""\u0000\"\\한글🙂\n""#;
        assert_eq!(expected_json.as_bytes().len(), 24);
        let mut charge = |_| true;
        let mut guard = Guard {
            limits: limits(),
            charge: &mut charge,
            stats: WitnessStats::default(),
        };
        let value = Any::from("\0\"\\한글🙂\n");
        assert_eq!(guard.json_len(&value).unwrap(), 40);
        guard.limits.max_native_bytes = 23;
        assert_eq!(guard.json_len(&value).err(), Some(WitnessError::Budget));
        // Independent conservative bound: envelope128 +client128 +item256
        // +Anyvector16 +Anystring32+A1 +owner metadata8 +key a1 =570.
        const EXPECTED_BOUND: usize = 570;
        let source = doc(10, false);
        source
            .get_or_insert_map("metadata")
            .insert(&mut source.transact_mut(), "a", "A");
        let wire = bytes(&source);
        let mut l = limits();
        l.max_native_bytes = EXPECTED_BOUND;
        let witness =
            capture_owned_input(&Update::decode_v1(&wire).unwrap(), l, &mut |_| true).unwrap();
        let stats =
            verify_owned_witnesses(&source.transact(), vec![witness], l, &mut |_| true).unwrap();
        assert_eq!(stats.native_bound, EXPECTED_BOUND);
        assert_eq!(stats.native_bytes, wire.len());
        l.max_native_bytes = EXPECTED_BOUND - 1;
        let witness =
            capture_owned_input(&Update::decode_v1(&wire).unwrap(), limits(), &mut |_| true)
                .unwrap();
        assert_eq!(
            verify_owned_witnesses(&source.transact(), vec![witness], l, &mut |_| true).err(),
            Some(WitnessError::Budget)
        );
    }

    #[test]
    fn native_archive_owned_witness_public_current_loss_foreign_parent_and_budget_refused() {
        let writer = doc(10, false);
        writer
            .get_or_insert_text("text")
            .insert(&mut writer.transact_mut(), 0, "한글🙂");
        let wire = bytes(&writer);
        let decoded = Update::decode_v1(&wire).unwrap();
        assert!(matches!(
            capture_owned_input(&decoded, limits(), &mut |_| false),
            Err(WitnessError::Budget)
        ));
        let gc = doc(20, true);
        gc.transact_mut()
            .apply_update(Update::decode_v1(&wire).unwrap())
            .unwrap();
        gc.get_or_insert_text("text")
            .remove_range(&mut gc.transact_mut(), 0, 4);
        let witness = capture_owned_input(&decoded, limits(), &mut |_| true).unwrap();
        assert!(check(&gc, vec![witness]).is_err());
        let source = doc(30, false);
        source.get_or_insert_text("text");
        let witness = incoming(&source, &wire);
        assert_eq!(
            verify_owned_witnesses(&source.transact(), vec![witness], limits(), &mut |_| false)
                .err(),
            Some(WitnessError::Budget)
        );
        let tx = source.transact();
        let root = tx.store().get_type("text").unwrap();
        let mut forged = Update::decode_v1(&wire).unwrap();
        for blocks in forged.blocks.clients.values_mut() {
            for block in blocks {
                if let Block::Item(item) = block {
                    item.parent = TypePtr::Branch(root);
                }
            }
        }
        assert!(matches!(
            capture_owned_input(&forged, limits(), &mut |_| true),
            Err(WitnessError::Malformed("input owner"))
        ));
    }

    #[test]
    fn native_archive_owned_witness_public_sparse_deep_and_replay_quota_refused() {
        let mut sparse = HashMap::with_capacity(2048);
        sparse.insert("a".to_owned(), Any::Null);
        let mut l = limits();
        l.max_table_capacity = 128;
        let mut charge = |_| true;
        let mut guard = Guard {
            limits: l,
            charge: &mut charge,
            stats: WitnessStats::default(),
        };
        assert_eq!(
            guard.any(&Any::Map(Arc::new(sparse)), 0).err(),
            Some(WitnessError::Budget)
        );
        let mut deep = Any::Null;
        for _ in 0..18 {
            deep = Any::Array(vec![deep].into());
        }
        guard.limits = limits();
        assert_eq!(guard.any(&deep, 0).err(), Some(WitnessError::Budget));
        let source = doc(10, false);
        source
            .get_or_insert_text("text")
            .insert(&mut source.transact_mut(), 0, "A");
        let wire = bytes(&source);
        let a = capture_owned_input(&Update::decode_v1(&wire).unwrap(), limits(), &mut |_| true)
            .unwrap();
        let z = capture_owned_input(&Update::decode_v1(&wire).unwrap(), limits(), &mut |_| true)
            .unwrap();
        l = limits();
        l.max_blocks = 1;
        assert_eq!(
            verify_owned_witnesses(&source.transact(), vec![a, z], l, &mut |_| true).err(),
            Some(WitnessError::Budget)
        );
    }
    // BEGIN R1/R2 independent original-source controls.
    fn fragmented_input(kind: &str, count: u32, width: u32) -> (Doc, OwnedInputWitness) {
        // Independent literal clocks: client10, fragments i*width..(i+1)*width;
        // Q is one UTF16/UTF8 unit; each Any element is the literal 한글🙂.
        let total = count * width;
        let writer = doc(10, false);
        if kind == "string" {
            writer.get_or_insert_text("items").insert(
                &mut writer.transact_mut(),
                0,
                &"Q".repeat(total as usize),
            );
        } else {
            use crate::Array;
            writer.get_or_insert_array("items").insert_range(
                &mut writer.transact_mut(),
                0,
                vec![Any::String("한글🙂".into()); total as usize],
            );
        }
        let mut decoded = Update::decode_v1(&bytes(&writer)).unwrap();
        let blocks = decoded.blocks.clients.get_mut(&ClientID::new(10)).unwrap();
        assert_eq!(blocks.len(), 1);
        let mut current = match blocks.pop_front().unwrap() {
            Block::Item(item) => item,
            _ => panic!("ordinary writer item"),
        };
        for _ in 1..count {
            let next = ItemPtr::from(&mut current)
                .splice(width, OffsetKind::Utf16)
                .unwrap();
            blocks.push_back(Block::Item(current));
            current = next;
        }
        blocks.push_back(Block::Item(current));
        let wire = decoded.encode_v1();
        let exact = Update::decode_v1(&wire).unwrap();
        let blocks = exact.blocks.clients.get(&ClientID::new(10)).unwrap();
        assert_eq!(blocks.len(), count as usize);
        for (i, block) in blocks.iter().enumerate() {
            let item = match block {
                Block::Item(item) => item,
                _ => panic!("fragment item"),
            };
            assert_eq!(item.id, id(10, i as u32 * width));
            assert_eq!(item.len, width);
            match &item.content {
                ItemContent::String(s) => assert_eq!(s.as_str(), "Q".repeat(width as usize)),
                ItemContent::Any(v) => {
                    assert_eq!(v, &vec![Any::String("한글🙂".into()); width as usize])
                }
                _ => panic!("literal content"),
            }
        }
        let source = doc(20, false);
        if kind == "string" {
            source.get_or_insert_text("items");
        } else {
            source.get_or_insert_array("items");
        }
        // Capture BEFORE applying this SAME freshly standard-decoded Update ONCE.
        let witness = capture_owned_input(&exact, limits(), &mut |_| true).unwrap();
        source.transact_mut().apply_update(exact).unwrap();
        {
            let tx = source.transact();
            let list = tx.store().blocks.get_client(&ClientID::new(10)).unwrap();
            assert_eq!(
                list.len(),
                1,
                "ordinary transaction squashes adjacent fragments"
            );
            let item = tx.store().blocks.get_item(&id(10, 0)).unwrap();
            assert_eq!(item.len, total);
            assert_eq!(tx.state_vector().get(&ClientID::new(10)), total);
            assert!(tx.snapshot().delete_set.is_empty());
        }
        (source, witness)
    }
    fn standard_scratch(source: &Doc) -> Doc {
        let scratch = doc(30, false);
        {
            let source_tx = source.transact();
            let mut tx = scratch.transact_mut();
            for (name, root) in &source_tx.store().types {
                tx.store
                    .get_or_create_type(name.clone(), root.type_ref.clone());
            }
            tx.apply_update(Update::decode_v1(&bytes(source)).unwrap())
                .unwrap();
        }
        scratch
    }
    #[test]
    fn native_archive_owned_witness_budget_fragmented_string_pre_effect() {
        let (source, witness) = fragmented_input("string", 1024, 64);
        let before = bytes(&source);
        let before_snapshot = source.transact().snapshot();
        let scratch = standard_scratch(&source);
        let mut tx = scratch.transact_mut();
        let mut unlimited = |_| true;
        let mut initial = Guard {
            limits: limits(),
            charge: &mut unlimited,
            stats: WitnessStats::default(),
        };
        let required = census(&source.transact(), &mut initial).unwrap();
        effect_probe::reset();
        let mut inspected = 0usize;
        let mut denied_at = None;
        let mut charge = |cost: WitnessCost| {
            let next = inspected.checked_add(cost.inspected_bytes).unwrap();
            if next > 256 * 1024 {
                denied_at = Some(effect_probe::counts());
                return false;
            }
            inspected = next;
            true
        };
        let mut guarded = Guard {
            limits: limits(),
            charge: &mut charge,
            stats: WitnessStats::default(),
        };
        let result = compare(
            &mut tx,
            vec![witness],
            &required.intervals,
            &required.witness.ds,
            &mut guarded,
        );
        let effects = effect_probe::counts();
        drop(guarded);
        drop(charge);
        assert_eq!(bytes(&source), before);
        assert_eq!(source.transact().snapshot(), before_snapshot);
        eprintln!("R1 original string result={result:?}, inspected={inspected}, effects={effects:?}, denied_at={denied_at:?}");
        assert_eq!(result.err(), Some(WitnessError::Budget));
        assert_eq!(
            denied_at,
            Some(effects),
            "no materialize/splice after refusal"
        );
        assert!(
            effects[0] < 16,
            "large repeated suffix copy must be refused early"
        );
        drop(tx);
        let (positive, w) = fragmented_input("string", 8, 64);
        let original = bytes(&positive);
        assert!(check(&positive, vec![w]).is_ok());
        assert_eq!(
            positive
                .get_or_insert_text("items")
                .get_string(&positive.transact()),
            "Q".repeat(512)
        );
        assert_eq!(bytes(&positive), original);
    }
    #[test]
    fn native_archive_owned_witness_budget_fragmented_any_pre_effect() {
        let (source, _) = fragmented_input("any", 64, 8);
        let before = bytes(&source);
        let scratch = standard_scratch(&source);
        let mut tx = scratch.transact_mut();
        effect_probe::reset();
        let mut denied = false;
        let mut charge = |cost: WitnessCost| {
            if cost.owned_bytes > 512 {
                denied = true;
                false
            } else {
                true
            }
        };
        let mut b = Guard {
            limits: limits(),
            charge: &mut charge,
            stats: WitnessStats::default(),
        };
        let result = materialize(&mut tx.store, id(10, 7), false, &mut b);
        let effects = effect_probe::counts();
        drop(b);
        drop(charge);
        assert_eq!(bytes(&source), before);
        eprintln!("R1 original Any result={result:?}, denied={denied}, effects={effects:?}");
        assert_eq!(result.err(), Some(WitnessError::Budget));
        assert!(denied);
        assert_eq!(effects, [0, 0, 0], "refuse before full Any split copies");
        drop(tx);
        let (positive, w) = fragmented_input("any", 8, 8);
        let original = bytes(&positive);
        assert!(check(&positive, vec![w]).is_ok());
        assert_eq!(bytes(&positive), original);
    }
    #[test]
    fn native_archive_owned_witness_budget_client_list_shift_pre_effect() {
        // Independent IDs before output: Q string10:0..63; map entries10:64..575.
        let writer = doc(10, false);
        writer
            .get_or_insert_text("items")
            .insert(&mut writer.transact_mut(), 0, &"Q".repeat(64));
        let map = writer.get_or_insert_map("meta");
        for i in 0..512 {
            map.insert(&mut writer.transact_mut(), format!("k{i}"), i);
        }
        let source = doc(20, false);
        source.get_or_insert_text("items");
        source.get_or_insert_map("meta");
        let exact = Update::decode_v1(&bytes(&writer)).unwrap();
        let _witness = capture_owned_input(&exact, limits(), &mut |_| true).unwrap();
        source.transact_mut().apply_update(exact).unwrap();
        assert_eq!(
            source
                .transact()
                .store()
                .blocks
                .get_client(&ClientID::new(10))
                .unwrap()
                .len(),
            513
        );
        assert_eq!(
            source.transact().state_vector().get(&ClientID::new(10)),
            576
        );
        let before = bytes(&source);
        let scratch = standard_scratch(&source);
        let mut tx = scratch.transact_mut();
        effect_probe::reset();
        let mut work = 0usize;
        let mut charge = |cost: WitnessCost| {
            let next = work.checked_add(cost.work).unwrap();
            if next > 128 {
                return false;
            }
            work = next;
            true
        };
        let mut b = Guard {
            limits: limits(),
            charge: &mut charge,
            stats: WitnessStats::default(),
        };
        let result = materialize(&mut tx.store, id(10, 31), false, &mut b);
        let effects = effect_probe::counts();
        drop(b);
        drop(charge);
        assert_eq!(bytes(&source), before);
        eprintln!("R1 original shift result={result:?}, work={work}, effects={effects:?}");
        assert_eq!(result.err(), Some(WitnessError::Budget));
        assert_eq!(
            effects,
            [0, 0, 0],
            "refuse before shifting 512 following blocks"
        );
    }
    #[test]
    fn native_archive_owned_witness_budget_empty_roots_pre_effect() {
        let source = doc(10, false);
        for i in 0..64 {
            source.get_or_insert_xml_fragment(format!("r{i:02}"));
        }
        let exact = Update::decode_v1(&[0, 0]).unwrap();
        let witness = capture_owned_input(&exact, limits(), &mut |_| true).unwrap();
        source.transact_mut().apply_update(exact).unwrap();
        assert_eq!(source.transact().store().types.len(), 64);
        assert!(source.transact().state_vector().is_empty());
        let before = bytes(&source);
        assert_eq!(before, [0, 0]);
        effect_probe::reset();
        let mut owned = 0usize;
        let mut denied_at = None;
        let mut charge = |cost: WitnessCost| {
            let next = owned.checked_add(cost.owned_bytes).unwrap();
            if next > 8 * 1024 {
                denied_at = Some(effect_probe::counts());
                return false;
            }
            owned = next;
            true
        };
        let result =
            verify_owned_witnesses(&source.transact(), vec![witness], limits(), &mut charge);
        let effects = effect_probe::counts();
        drop(charge);
        assert_eq!(bytes(&source), before);
        eprintln!("R2 original roots result={result:?}, owned={owned}, effects={effects:?}, denied_at={denied_at:?}");
        assert_eq!(result.err(), Some(WitnessError::Budget));
        assert_eq!(effects[2], 0, "refuse BEFORE scratch Doc/root allocations");
        assert_eq!(denied_at, Some(effects));
        let witness =
            capture_owned_input(&Update::decode_v1(&[0, 0]).unwrap(), limits(), &mut |_| {
                true
            })
            .unwrap();
        assert!(check(&source, vec![witness]).is_ok());
        assert_eq!(bytes(&source), before);
        let typed = doc(10, false);
        typed.get_or_insert_text("text");
        typed.get_or_insert_map("map");
        typed.get_or_insert_array("array");
        typed.get_or_insert_xml_fragment("xml");
        let witness =
            capture_owned_input(&Update::decode_v1(&[0, 0]).unwrap(), limits(), &mut |_| {
                true
            })
            .unwrap();
        assert!(check(&typed, vec![witness]).is_ok());
        assert_eq!(typed.transact().store().types.len(), 4);
    }
    // END R1/R2 independent original-source controls.
    // BEGIN separate explicitly scoped JSON controls.
    fn direct_json_item(values: Vec<String>) -> (Doc, Update) {
        // Maintained SDK direct Item fixture: deliberately NOT a successful
        // legacy JSON wire roundtrip claim. Ordinary standard Any writer and
        // decoder provide the root/IDs; only ItemContent is explicitly changed.
        use crate::Array;
        let writer = doc(10, false);
        writer.get_or_insert_array("items").insert_range(
            &mut writer.transact_mut(),
            0,
            values.iter().cloned().map(Any::from).collect::<Vec<_>>(),
        );
        let mut exact = Update::decode_v1(&bytes(&writer)).unwrap();
        let blocks = exact.blocks.clients.get_mut(&ClientID::new(10)).unwrap();
        assert_eq!(blocks.len(), 1);
        if let Block::Item(item) = &mut blocks[0] {
            assert_eq!(item.id, id(10, 0));
            assert_eq!(item.len, values.len() as u32);
            item.content = ItemContent::JSON(values);
        } else {
            panic!("ordinary array item");
        }
        let source = doc(20, false);
        source.get_or_insert_array("items");
        (source, exact)
    }
    #[test]
    fn native_archive_owned_witness_budget_json_item_pre_effect() {
        let literal = vec!["한글🙂".to_owned(); 512];
        let (source, exact) = direct_json_item(literal.clone());
        let _witness = capture_owned_input(&exact, limits(), &mut |_| true).unwrap();
        source.transact_mut().apply_update(exact).unwrap();
        // Same direct prepared Item captured before applying it ONCE. No JSON
        // codec success or product support is inferred from this operation.
        let before = bytes(&source);
        let before_snapshot = source.transact().snapshot();
        let (scratch, prepared) = direct_json_item(literal.clone());
        let _captured = capture_owned_input(&prepared, limits(), &mut |_| true).unwrap();
        scratch.transact_mut().apply_update(prepared).unwrap();
        let mut tx = scratch.transact_mut();
        assert_eq!(tx.store.blocks.get_item(&id(10, 0)).unwrap().len, 512);
        effect_probe::reset();
        let mut charge = |cost: WitnessCost| cost.owned_bytes <= 512;
        let mut b = Guard {
            limits: limits(),
            charge: &mut charge,
            stats: WitnessStats::default(),
        };
        let result = materialize(&mut tx.store, id(10, 7), false, &mut b);
        let effects = effect_probe::counts();
        drop(b);
        assert_eq!(bytes(&source), before);
        assert_eq!(source.transact().snapshot(), before_snapshot);
        eprintln!("R1 direct JSON item result={result:?}, effects={effects:?}");
        assert_eq!(result.err(), Some(WitnessError::Budget));
        assert_eq!(
            effects,
            [0, 0, 0],
            "refuse before JSON vectors/String copies"
        );
        drop(tx);
        assert_eq!(bytes(&source), before);
        assert_eq!(source.transact().snapshot(), before_snapshot);
        let (positive, exact) = direct_json_item(literal.clone());
        let _witness = capture_owned_input(&exact, limits(), &mut |_| true).unwrap();
        positive.transact_mut().apply_update(exact).unwrap();
        let mut tx = positive.transact_mut();
        let mut charge = |_| true;
        let mut b = Guard {
            limits: limits(),
            charge: &mut charge,
            stats: WitnessStats::default(),
        };
        materialize(&mut tx.store, id(10, 7), false, &mut b).unwrap();
        let left = tx.store.blocks.get_item(&id(10, 0)).unwrap();
        let right = tx.store.blocks.get_item(&id(10, 8)).unwrap();
        assert_eq!(left.len, 8);
        assert_eq!(right.len, 504);
        assert_eq!(left.content, ItemContent::JSON(literal[..8].to_vec()));
        assert_eq!(right.content, ItemContent::JSON(literal[8..].to_vec()));
    }
    #[test]
    fn native_archive_owned_witness_original_json_codec_semantic_roundtrip() {
        // Independent N=3, clocks10:0..2, exact Korean/emoji/plain values are
        // declared before standard encoding. Expected successful roundtrip;
        // a failure stays original evidence, never an assertErr-to-green fix.
        let expected = vec!["한글🙂".to_owned(), "ASCII".to_owned(), "🧪".to_owned()];
        let (_source, exact) = direct_json_item(expected.clone());
        let wire = exact.encode_v1();
        let decoded = Update::decode_v1(&wire);
        eprintln!("Original standard JSON codec N=3 decode={decoded:?}");
        let decoded = decoded.expect("standard legacy JSON semantic roundtrip");
        let blocks = decoded.blocks.clients.get(&ClientID::new(10)).unwrap();
        assert_eq!(blocks.len(), 1);
        let item = match &blocks[0] {
            Block::Item(i) => i,
            _ => panic!("JSON item"),
        };
        assert_eq!(item.id, id(10, 0));
        assert_eq!(item.len, 3);
        assert_eq!(item.content, ItemContent::JSON(expected));
        assert!(decoded.delete_set.is_empty());
    }
    // END separate explicitly scoped JSON controls.
    #[test]
    fn native_archive_owned_witness_budget_actual_capacity_growth_pre_effect() {
        // Literal Q clocks10:0..23 before writer output; preserve canonical
        // source while standard scratch materialization reaches actual capacity.
        let (source, _) = fragmented_input("string", 1, 24);
        let original = bytes(&source);
        let scratch = standard_scratch(&source);
        let mut tx = scratch.transact_mut();
        let mut charge = |_| true;
        let mut b = Guard {
            limits: limits(),
            charge: &mut charge,
            stats: WitnessStats::default(),
        };
        loop {
            let list = tx.store.blocks.get_client(&ClientID::new(10)).unwrap();
            if list.len() == list.retained_capacity() {
                break;
            }
            let next = list.len() as u32 - 1;
            materialize(&mut tx.store, id(10, next), false, &mut b).unwrap();
        }
        drop(b);
        let list = tx.store.blocks.get_client(&ClientID::new(10)).unwrap();
        let before_count = list.len();
        let before_capacity = list.retained_capacity();
        assert!(before_count < 24 && before_count == before_capacity);
        let before = tx.encode_state_as_update_v1(&StateVector::default());
        effect_probe::reset();
        // The callback refuses a prospective allocation coupled to movement
        // of at least the independently observed full client-list length.
        // It admits per-content/Item charges and readonly lookup charges.
        let mut refused = None;
        let mut charge = |cost: WitnessCost| {
            if cost.work >= before_count && cost.owned_bytes > 0 {
                refused = Some(cost);
                false
            } else {
                true
            }
        };
        let mut b = Guard {
            limits: limits(),
            charge: &mut charge,
            stats: WitnessStats::default(),
        };
        let result = materialize(
            &mut tx.store,
            id(10, before_count as u32 - 1),
            false,
            &mut b,
        );
        drop(b);
        drop(charge);
        assert_eq!(result.err(), Some(WitnessError::Budget));
        assert_eq!(effect_probe::counts(), [0, 0, 0]);
        let cost = refused.expect("client growth allocation reserved before effect");
        assert!(cost.owned_bytes >= (before_count + 1) * size_of::<Block>());
        assert_eq!(
            tx.store
                .blocks
                .get_client(&ClientID::new(10))
                .unwrap()
                .len(),
            before_count
        );
        assert_eq!(
            tx.store
                .blocks
                .get_client(&ClientID::new(10))
                .unwrap()
                .retained_capacity(),
            before_capacity
        );
        assert_eq!(
            tx.encode_state_as_update_v1(&StateVector::default()),
            before
        );
        let mut charge = |_| true;
        let mut b = Guard {
            limits: limits(),
            charge: &mut charge,
            stats: WitnessStats::default(),
        };
        materialize(
            &mut tx.store,
            id(10, before_count as u32 - 1),
            false,
            &mut b,
        )
        .unwrap();
        assert_eq!(
            tx.store
                .blocks
                .get_client(&ClientID::new(10))
                .unwrap()
                .len(),
            before_count + 1
        );
        assert!(
            tx.store
                .blocks
                .get_client(&ClientID::new(10))
                .unwrap()
                .retained_capacity()
                > before_capacity
        );
        assert_eq!(
            tx.store
                .blocks
                .get_item(&id(10, before_count as u32 - 1))
                .unwrap()
                .len,
            1
        );
        assert_eq!(
            tx.store
                .blocks
                .get_item(&id(10, before_count as u32))
                .unwrap()
                .content,
            ItemContent::String("Q".repeat(24 - before_count).as_str().into())
        );
        assert_eq!(bytes(&source), original);
    }

    #[test]
    fn native_archive_owned_witness_budget_aligned_string_pre_effect() {
        // Independent literal: 4 Unicode scalars, 6 UTF16 code units, 14 UTF8
        // bytes. Both boundaries align with the one unchanged native Item.
        let literal = "한글🙂🧪";
        assert_eq!(literal.encode_utf16().count(), 6);
        assert_eq!(literal.len(), 14);
        let source = doc(10, false);
        source
            .get_or_insert_text("text")
            .insert(&mut source.transact_mut(), 0, literal);
        let source_bytes = bytes(&source);
        let source_snapshot = source.transact().snapshot();
        for (at, start) in [(id(10, 0), true), (id(10, 5), false)] {
            let scratch = standard_scratch(&source);
            let mut tx = scratch.transact_mut();
            let before = tx.encode_state_as_update_v1(&StateVector::default());
            let before_snapshot = tx.snapshot();
            let item = tx.store.blocks.get_item(&id(10, 0)).unwrap();
            assert_eq!(item.len, 6);
            assert_eq!(item.content, ItemContent::String(literal.into()));
            assert_eq!(before_snapshot.state_map.get(&ClientID::new(10)), 6);
            assert_eq!(before_snapshot.delete_set, IdSet::default());
            effect_probe::reset();
            let mut refused = None;
            let mut charge = |cost: WitnessCost| {
                if cost.inspected_bytes >= literal.len() {
                    refused = Some(cost);
                    false
                } else {
                    true
                }
            };
            let mut b = Guard {
                limits: limits(),
                charge: &mut charge,
                stats: WitnessStats::default(),
            };
            let result = materialize(&mut tx.store, at, start, &mut b);
            drop(b);
            drop(charge);
            assert_eq!(bytes(&source), source_bytes);
            assert_eq!(source.transact().snapshot(), source_snapshot);
            assert_eq!(
                tx.encode_state_as_update_v1(&StateVector::default()),
                before
            );
            assert_eq!(tx.snapshot(), before_snapshot);
            eprintln!(
                "R1 aligned start={start} result={result:?}, effects={:?}, refused={refused:?}",
                effect_probe::counts()
            );
            assert_eq!(result.err(), Some(WitnessError::Budget));
            assert_eq!(effect_probe::counts(), [0, 0, 0]);
            assert!(refused.unwrap().inspected_bytes >= literal.len());

            // Admit the SAME aligned operation, then verify literal content,
            // original IDs/SV/DS and unchanged source and scratch bytes.
            let mut charge = |_| true;
            let mut b = Guard {
                limits: limits(),
                charge: &mut charge,
                stats: WitnessStats::default(),
            };
            materialize(&mut tx.store, at, start, &mut b).unwrap();
            assert_eq!(effect_probe::counts(), [1, 0, 0]);
            assert_eq!(
                tx.store
                    .blocks
                    .get_client(&ClientID::new(10))
                    .unwrap()
                    .len(),
                1
            );
            let item = tx.store.blocks.get_item(&id(10, 0)).unwrap();
            assert_eq!(item.id, id(10, 0));
            assert_eq!(item.len, 6);
            assert_eq!(item.content, ItemContent::String(literal.into()));
            assert_eq!(
                tx.encode_state_as_update_v1(&StateVector::default()),
                before
            );
            assert_eq!(tx.snapshot(), before_snapshot);
            assert_eq!(bytes(&source), source_bytes);
            assert_eq!(source.transact().snapshot(), source_snapshot);
        }
    }

    // Separate valid serialized-JSON count controls. No original oracle changes.
    fn json_count_cursor(values: &[&str], count: u32, golden: &[u8]) {
        use crate::encoding::read::Read;
        use crate::encoding::write::Write;
        use crate::updates::decoder::{Decoder, DecoderV1};
        assert_eq!(values.len(), count as usize);
        for value in values {
            let _: serde_json::Value = serde_json::from_str(value).unwrap();
        }
        let expected = ItemContent::JSON(values.iter().map(|s| (*s).to_owned()).collect());
        let mut encoder = EncoderV1::new();
        encoder.write_len(count);
        for value in values {
            encoder.write_string(value);
        }
        encoder.write_string("Z");
        let wire = encoder.to_vec();
        assert_eq!(wire, golden, "independent count/UTF8/next-field bytes");
        let mut decoder = DecoderV1::from(wire.as_slice());
        let decoded = ItemContent::decode(&mut decoder, crate::block::BLOCK_ITEM_JSON_REF_NUMBER);
        eprintln!("Original valid JSON count={count} decoded={decoded:?}");
        assert_eq!(decoded.unwrap(), expected);
        assert_eq!(decoder.read_string().unwrap(), "Z", "next field unconsumed");
        assert!(decoder.read_to_end().unwrap().is_empty());
    }

    #[test]
    fn native_archive_owned_witness_codec_json_count_zero() {
        // Content-decoder boundary only; not acceptance of an empty native Item.
        json_count_cursor(&[], 0, &[0, 1, b'Z']);
    }

    #[test]
    fn native_archive_owned_witness_codec_json_count_one() {
        // Quoted JSON String "한글🙂": exactly 12 UTF8 bytes, then field Z.
        json_count_cursor(
            &["\"한글🙂\""],
            1,
            &[
                1, 12, 34, 237, 149, 156, 234, 184, 128, 240, 159, 153, 130, 34, 1, 90,
            ],
        );
    }

    #[test]
    fn native_archive_owned_witness_codec_json_count_three() {
        json_count_cursor(
            &["\"한글🙂\"", "\"ASCII\"", "\"🧪\""],
            3,
            &[
                3, 12, 34, 237, 149, 156, 234, 184, 128, 240, 159, 153, 130, 34, 7, 34, 65, 83, 67,
                73, 73, 34, 6, 34, 240, 159, 167, 170, 34, 1, 90,
            ],
        );
    }

    #[test]
    fn native_archive_owned_witness_codec_json_following_struct_nested() {
        use crate::Array;
        let nested = r#"{"nested":["🧪",true,null],"ref":{"kind":"task","id":"00000000-0000-4000-8000-000000000001"}}"#;
        let values = vec![
            "\"한글🙂\"".to_owned(),
            nested.to_owned(),
            "\"ASCII\"".to_owned(),
        ];
        let next_value = "다음🧪";
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(nested).unwrap(),
            serde_json::json!({"nested":["🧪",true,null],"ref":{"kind":"task","id":"00000000-0000-4000-8000-000000000001"}})
        );
        // Independent native IDs: JSON 10:0..2, following map Item 10:3,
        // distinct roots items/after and map key ref, total SV4, empty DS.
        let writer = doc(10, false);
        let array = writer.get_or_insert_array("items");
        let map = writer.get_or_insert_map("after");
        {
            let mut tx = writer.transact_mut();
            array.insert_range(&mut tx, 0, values.iter().cloned().map(Any::from));
            map.insert(&mut tx, "ref", next_value);
        }
        let mut prepared = Update::decode_v1(&bytes(&writer)).unwrap();
        let blocks = prepared.blocks.clients.get_mut(&ClientID::new(10)).unwrap();
        assert_eq!(blocks.len(), 2);
        if let Block::Item(item) = &mut blocks[0] {
            assert_eq!(item.id, id(10, 0));
            assert_eq!(item.len, 3);
            assert_eq!(item.parent, TypePtr::Named("items".into()));
            item.content = ItemContent::JSON(values.clone());
        } else {
            panic!("ordinary array Item");
        }
        if let Block::Item(item) = &blocks[1] {
            assert_eq!(item.id, id(10, 3));
            assert_eq!(item.len, 1);
            assert_eq!(item.parent, TypePtr::Named("after".into()));
            assert_eq!(item.parent_sub.as_deref(), Some("ref"));
            assert_eq!(item.content, ItemContent::Any(vec![Any::from(next_value)]));
        } else {
            panic!("ordinary following map Item");
        }
        assert!(prepared.delete_set.is_empty());
        // Only the legacy Content variant is explicit; maintained standard
        // update encoder/decoder supplies framing and the following struct.
        let wire = prepared.encode_v1();
        let decoded = Update::decode_v1(&wire);
        eprintln!("Original valid JSON followed by native struct decode={decoded:?}");
        let decoded = decoded.expect("valid legacy JSON and following native struct");
        let blocks = decoded.blocks.clients.get(&ClientID::new(10)).unwrap();
        assert_eq!(blocks.len(), 2);
        match &blocks[0] {
            Block::Item(item) => {
                assert_eq!(item.id, id(10, 0));
                assert_eq!(item.len, 3);
                assert_eq!(item.content, ItemContent::JSON(values));
            }
            _ => panic!("decoded JSON Item"),
        }
        match &blocks[1] {
            Block::Item(item) => {
                assert_eq!(item.id, id(10, 3));
                assert_eq!(item.parent, TypePtr::Named("after".into()));
                assert_eq!(item.parent_sub.as_deref(), Some("ref"));
                assert_eq!(item.content, ItemContent::Any(vec![Any::from(next_value)]));
            }
            _ => panic!("decoded following Item"),
        }
        assert!(decoded.delete_set.is_empty());
        assert_eq!(decoded.encode_v1(), wire);
        let source = doc(20, false);
        source.get_or_insert_array("items");
        let map = source.get_or_insert_map("after");
        let captured = capture_owned_input(&decoded, limits(), &mut |_| true).unwrap();
        source.transact_mut().apply_update(decoded).unwrap();
        assert_eq!(source.transact().state_vector().get(&ClientID::new(10)), 4);
        assert!(source.transact().snapshot().delete_set.is_empty());
        assert_eq!(
            map.get(&source.transact(), "ref"),
            Some(crate::Out::Any(Any::from(next_value)))
        );
        assert_eq!(check(&source, vec![captured]).unwrap().comparisons, 4);
        // SDK wire/witness preservation only; no Yjs semantic reader or product
        // archive/RLS/UI completion is claimed by this fixture.
    }

    #[test]
    fn native_archive_owned_witness_codec_json_truncated_rejected() {
        use crate::encoding::write::Write;
        use crate::updates::decoder::DecoderV1;
        let mut encoder = EncoderV1::new();
        encoder.write_len(3);
        encoder.write_string("\"한글🙂\"");
        encoder.write_string("\"ASCII\"");
        let truncated_count = encoder.to_vec();
        let mut decoder = DecoderV1::from(truncated_count.as_slice());
        assert!(
            ItemContent::decode(&mut decoder, crate::block::BLOCK_ITEM_JSON_REF_NUMBER).is_err()
        );
        let mut encoder = EncoderV1::new();
        encoder.write_len(1);
        encoder.write_string("\"한글🙂\"");
        let mut truncated_string = encoder.to_vec();
        truncated_string.pop();
        let mut decoder = DecoderV1::from(truncated_string.as_slice());
        assert!(
            ItemContent::decode(&mut decoder, crate::block::BLOCK_ITEM_JSON_REF_NUMBER).is_err()
        );
    }
    #[test]
    fn native_archive_owned_witness_charge_in_order_insert_extends_without_growth() {
        // Independent literal: 100 contiguous one-clock IDs of client 10 in
        // clock order coalesce into ONE range; only the first insert (a new
        // client entry) can allocate.
        let mut charges = Vec::new();
        let mut record = |cost: WitnessCost| {
            charges.push(cost);
            true
        };
        let mut b = Guard {
            limits: limits(),
            charge: &mut record,
            stats: WitnessStats::default(),
        };
        let mut set = IdSet::default();
        for clock in 0..100 {
            b.insert(&mut set, id(10, clock), 1).unwrap();
        }
        drop(b);
        assert_eq!(set.get(&ClientID::new(10)).unwrap().len(), 1);
        assert_eq!(charges.len(), 100);
        assert!(charges[0].owned_bytes > 0, "new client map node is charged");
        assert!(
            charges[1..]
                .iter()
                .all(|c| c.owned_bytes == 0 && c.work == 1),
            "extending the last range allocates nothing: {:?}",
            &charges[1..4]
        );
    }
    #[test]
    fn native_archive_owned_witness_charge_disjoint_and_general_insert_growth() {
        // Disjoint pushes clock 0,2,4,..,16 (nine ranges) grow the per-client
        // vector only when its length equals its power-of-two capacity; an
        // out-of-order insert takes the general path (search plus shift).
        let entry = size_of::<(std::ops::Range<u32>, ())>();
        let mut charges = Vec::new();
        let mut record = |cost: WitnessCost| {
            charges.push(cost);
            true
        };
        let mut b = Guard {
            limits: limits(),
            charge: &mut record,
            stats: WitnessStats::default(),
        };
        let mut set = IdSet::default();
        for k in 0..9 {
            b.insert(&mut set, id(10, k * 2), 1).unwrap();
        }
        b.insert(&mut set, id(10, 5), 1).unwrap();
        drop(b);
        // Before each push the length was k; growth is charged at k=1,2,4,8.
        for k in 1..9usize {
            let expected = if k.is_power_of_two() {
                2 * k * entry
            } else {
                0
            };
            assert_eq!(charges[k].owned_bytes, expected, "push at length {k}");
            assert_eq!(charges[k].work, 1, "tail push at length {k}");
        }
        // General path at length 9: search ilog2(9)+1 plus a shift of up to 9.
        assert_eq!(charges[9].work, 3 + 1 + 9);
        assert_eq!(charges[9].owned_bytes, 0, "length 9 is not a growth point");
        // 5..6 coalesces 4..5 and 6..7 into one range.
        assert_eq!(set.get(&ClientID::new(10)).unwrap().len(), 8);
    }
    #[test]
    fn native_archive_owned_witness_charge_compare_reads_content_in_place() {
        // One 4000-byte retained string: verification materializes owned
        // copies only in the canonical census, the scratch decode and the
        // clone census; the two comparisons read both sides in place.
        let writer = doc(10, false);
        writer
            .get_or_insert_text("t")
            .insert(&mut writer.transact_mut(), 0, &"x".repeat(4000));
        let target = doc(20, false);
        target.get_or_insert_text("t");
        let witness = incoming(&target, &bytes(&writer));
        let (mut owned, mut read) = (0usize, 0usize);
        verify_owned_witnesses(&target.transact(), vec![witness], limits(), &mut |c| {
            if c.inspected_bytes == 4000 {
                read += 1;
                // An owned copy is a 4096-byte clone buffer.
                if c.owned_bytes == 4096 {
                    owned += 1;
                }
            }
            true
        })
        .unwrap();
        assert_eq!(
            owned, 3,
            "owned copies: canonical census, scratch decode, clone census"
        );
        // Comparisons (2 x 2) and boundary materialization scans still read.
        assert!(
            read >= 7,
            "every read is still charged as inspected work: {read}"
        );
    }
    #[test]
    fn native_archive_owned_witness_charge_malformed_range_refused_before_effect() {
        // An empty range and a clock range past u32::MAX are refused before
        // any charge or insert; the unchecked SDK clock addition never runs.
        let mut charges = Vec::new();
        let mut record = |cost: WitnessCost| {
            charges.push(cost);
            true
        };
        let mut b = Guard {
            limits: limits(),
            charge: &mut record,
            stats: WitnessStats::default(),
        };
        let mut set = IdSet::default();
        assert_eq!(
            b.insert(&mut set, id(10, 0), 0).err(),
            Some(WitnessError::Malformed("witness range"))
        );
        assert_eq!(
            b.insert(&mut set, id(10, u32::MAX), 1).err(),
            Some(WitnessError::Malformed("witness range"))
        );
        drop(b);
        assert!(charges.is_empty());
        assert!(set.is_empty());
    }
    #[test]
    fn native_archive_owned_witness_charge_new_client_split_envelope() {
        // 200 distinct clients cross leaf, root and multi-level BTreeMap
        // splits; each new key is precharged a pinned node envelope times
        // (binary height bound + 2). Existing-key tail extension charges 0.
        let node = (size_of::<ClientID>() + size_of::<crate::id_set::IdRange>()) * 11
            + size_of::<usize>() * 12
            + 64;
        let mut charges = Vec::new();
        let mut record = |cost: WitnessCost| {
            charges.push(cost);
            true
        };
        let mut b = Guard {
            limits: limits(),
            charge: &mut record,
            stats: WitnessStats::default(),
        };
        let mut set = IdSet::default();
        for client in 1..=200u64 {
            b.insert(&mut set, id(client, 0), 1).unwrap();
        }
        b.insert(&mut set, id(7, 1), 1).unwrap();
        drop(b);
        assert_eq!(set.len(), 200);
        for (k, cost) in charges[..200].iter().enumerate() {
            let height = ((k + 1) as u32).ilog2() as usize;
            assert_eq!(cost.owned_bytes, node * (height + 2), "new client #{k}");
            assert_eq!(cost.work, height + 1, "new client #{k}");
        }
        assert_eq!(charges[200].owned_bytes, 0);
        assert_eq!(charges[200].work, 1);
    }
    #[test]
    fn native_archive_owned_witness_charge_string_copy_matches_clone_layout() {
        // Independent literals: a 1000-byte retained string is copied into a
        // 1024-byte heap buffer (smallvec next_power_of_two); a 3-byte string
        // stays inline in SmallString<[u8; 8]> and allocates nothing.
        let writer = doc(10, false);
        let text = writer.get_or_insert_text("t");
        text.insert(&mut writer.transact_mut(), 0, &"x".repeat(1000));
        let long = Update::decode_v1(&bytes(&writer)).unwrap();
        let mut owned = Vec::new();
        capture_owned_input(&long, limits(), &mut |c| {
            if c.inspected_bytes == 1000 {
                owned.push(c.owned_bytes);
            }
            true
        })
        .unwrap();
        assert_eq!(owned, vec![1024]);
        let short_writer = doc(11, false);
        short_writer
            .get_or_insert_text("t")
            .insert(&mut short_writer.transact_mut(), 0, "abc");
        let short = Update::decode_v1(&bytes(&short_writer)).unwrap();
        let mut owned = Vec::new();
        capture_owned_input(&short, limits(), &mut |c| {
            if c.inspected_bytes == 3 {
                owned.push(c.owned_bytes);
            }
            true
        })
        .unwrap();
        assert_eq!(owned, vec![0]);
    }
}
