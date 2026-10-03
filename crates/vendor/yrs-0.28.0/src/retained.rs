//! Read-only views of stored versions and decoded input.
//!
//! Borrowed view functions do not parse, integrate, repair or re-encode state.
//! Callbacks run while the caller holds the transaction. Do not reenter a write
//! transaction for the same document: the existing lock can block that callback.
//! Consumers must separately budget inspected content, owned copies and ancestry.
//! The separate owned-witness API performs archive-only scratch normalization;
//! it neither mutates the canonical read transaction nor integrates witnesses.
//!
//! Borrowed content cannot escape a callback:
//! ```compile_fail
//! use yrs::{Doc, ReadTxn, Transact};
//! use yrs::retained::{VisitLimits, RetainedEvent, RetainedEntry, RetainedContent};
//! use std::ops::ControlFlow;
//! let doc = Doc::new();
//! let tx = doc.transact();
//! let mut escaped: Option<&str> = None;
//! tx.visit_retained(VisitLimits::default(), |event| {
//!     if let RetainedEvent::Block(RetainedEntry::Item { content: RetainedContent::String(s), .. }) = event {
//!         escaped = Some(s);
//!     }
//!     ControlFlow::<()>::Continue(())
//! }).unwrap();
//! println!("{:?}", escaped);
//! ```

use crate::block::{Block, Item, ItemContent};
use crate::branch::Branch;
use crate::types::{TypePtr, TypeRef};
use crate::{Any, ReadTxn, Update, ID};
use std::ops::ControlFlow;

mod witness;
pub use witness::{
    capture_owned_input, verify_owned_witnesses, OwnedInputWitness, WitnessCost, WitnessError,
    WitnessLimits, WitnessStats,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BranchIdentity<'a> {
    Root(&'a str),
    Nested(ID),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParentView<'a> {
    Root,
    Nested(BranchIdentity<'a>),
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeKind {
    Array,
    Map,
    Text,
    XmlElement,
    XmlFragment,
    XmlText,
    XmlHook,
    Subdocument,
    WeakLink,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BranchView<'a> {
    pub id: BranchIdentity<'a>,
    pub kind: TypeKind,
    pub xml_tag: Option<&'a str>,
    pub parent: ParentView<'a>,
    pub deleted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnerView<'a> {
    Resolved(BranchView<'a>),
    Unavailable,
}

#[derive(Debug, Clone, Copy)]
pub enum RetainedContent<'a> {
    Values(&'a [Any]),
    JsonValues(&'a [String]),
    String(&'a str),
    Embed(&'a Any),
    Format {
        key: &'a str,
        value: &'a Any,
    },
    Type {
        kind: TypeKind,
        xml_tag: Option<&'a str>,
    },
    Binary(&'a [u8]),
    Subdocument,
    Deleted {
        native_clock_len: u32,
    },
}

#[derive(Debug, Clone, Copy)]
pub enum RetainedEntry<'a> {
    Item {
        id: ID,
        native_clock_len: u32,
        owner: OwnerView<'a>,
        map_key: Option<&'a str>,
        origin: Option<ID>,
        right_origin: Option<ID>,
        deleted: bool,
        content: RetainedContent<'a>,
    },
    Gc {
        id: ID,
        native_clock_len: u32,
    },
    Skip {
        id: ID,
        native_clock_len: u32,
    },
}

#[derive(Debug, Clone, Copy)]
pub enum RetainedEvent<'a> {
    Root(BranchView<'a>),
    Block(RetainedEntry<'a>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputParent<'a> {
    Named(&'a str),
    Id(ID),
    Branch(BranchIdentity<'a>),
    Unknown,
}

#[derive(Debug, Clone, Copy)]
pub enum InputEntry<'a> {
    Item {
        id: ID,
        native_clock_len: u32,
        parent: InputParent<'a>,
        map_key: Option<&'a str>,
        origin: Option<ID>,
        right_origin: Option<ID>,
        content: RetainedContent<'a>,
    },
    Gc {
        id: ID,
        native_clock_len: u32,
    },
    Skip {
        id: ID,
        native_clock_len: u32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetainedSummary {
    pub client_count: usize,
    pub client_table_capacity: usize,
    pub root_count: usize,
    pub root_table_capacity: usize,
    pub pending_update: bool,
    pub pending_deletions: bool,
    pub skip_gc: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct VisitLimits {
    pub max_blocks: usize,
    pub max_roots: usize,
    /// Sum of client and root HashMap capacities, checked before iteration.
    pub max_table_capacity: usize,
}
impl Default for VisitLimits {
    fn default() -> Self {
        Self {
            max_blocks: 100_000,
            max_roots: 128,
            max_table_capacity: 100_000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisitError {
    TableCapacityLimit,
    RootLimit,
    UnavailableRoot,
    BlockLimit,
    EmptyRange { id: ID },
    RangeOverflow { id: ID },
}

pub fn summary<T: ReadTxn>(tx: &T) -> RetainedSummary {
    let store = tx.store();
    RetainedSummary {
        client_count: store.blocks.iter().len(),
        client_table_capacity: store.blocks.retained_client_capacity(),
        root_count: store.types.len(),
        root_table_capacity: store.types.capacity(),
        pending_update: store.pending_update().is_some(),
        pending_deletions: store.pending_ds().is_some(),
        skip_gc: store.skip_gc,
    }
}

pub(crate) fn visit<T: ReadTxn, E>(
    tx: &T,
    limits: VisitLimits,
    mut callback: impl for<'a> FnMut(RetainedEvent<'a>) -> ControlFlow<E>,
) -> Result<ControlFlow<E>, VisitError> {
    let s = summary(tx);
    let capacity = s
        .client_table_capacity
        .checked_add(s.root_table_capacity)
        .ok_or(VisitError::TableCapacityLimit)?;
    if capacity > limits.max_table_capacity {
        return Err(VisitError::TableCapacityLimit);
    }
    if s.root_count > limits.max_roots {
        return Err(VisitError::RootLimit);
    }
    let store = tx.store();
    for branch in store.types.values() {
        if let Some(v) = branch_view(branch) {
            if let ControlFlow::Break(v) = callback(RetainedEvent::Root(v)) {
                return Ok(ControlFlow::Break(v));
            }
        }
        // A broken root identity is explicit failure; never invent one.
        else {
            return Err(VisitError::UnavailableRoot);
        }
    }
    let mut visited = 0usize;
    for (_, blocks) in store.blocks.iter() {
        for block in blocks.iter() {
            visited = visited.checked_add(1).ok_or(VisitError::BlockLimit)?;
            if visited > limits.max_blocks {
                return Err(VisitError::BlockLimit);
            }
            let block = block.as_ref();
            check_range(block.id(), block.len())?;
            let view = match block {
                Block::Item(item) => RetainedEntry::Item {
                    id: item.id,
                    native_clock_len: item.len,
                    owner: match &item.parent {
                        TypePtr::Branch(b) => branch_view(b)
                            .map(OwnerView::Resolved)
                            .unwrap_or(OwnerView::Unavailable),
                        _ => OwnerView::Unavailable,
                    },
                    map_key: item.parent_sub.as_deref(),
                    origin: item.origin,
                    right_origin: item.right_origin,
                    deleted: item.is_deleted(),
                    content: content(&item.content),
                },
                Block::GC(r) => RetainedEntry::Gc {
                    id: r.id(),
                    native_clock_len: r.len,
                },
                Block::Skip(r) => RetainedEntry::Skip {
                    id: r.id(),
                    native_clock_len: r.len,
                },
            };
            if let ControlFlow::Break(v) = callback(RetainedEvent::Block(view)) {
                return Ok(ControlFlow::Break(v));
            }
        }
    }
    Ok(ControlFlow::Continue(()))
}

/// Borrowed input content also cannot escape its callback.
/// ```compile_fail
/// use yrs::retained::{visit_input, VisitLimits, InputEntry, RetainedContent};
/// use yrs::Update;
/// use std::ops::ControlFlow;
/// let update = Update::default();
/// let mut escaped: Option<&str> = None;
/// visit_input(&update, VisitLimits::default(), |entry| {
///     if let InputEntry::Item { content: RetainedContent::String(s), .. } = entry {
///         escaped = Some(s);
///     }
///     ControlFlow::<()>::Continue(())
/// }).unwrap();
/// println!("{:?}", escaped);
/// ```

/// Visits already decoded input without resolving parent/key or applying it.
/// Input Unknown/absent key are deliberately distinct from integrated ownership.
pub fn visit_input<E>(
    update: &Update,
    limits: VisitLimits,
    mut callback: impl for<'a> FnMut(InputEntry<'a>) -> ControlFlow<E>,
) -> Result<ControlFlow<E>, VisitError> {
    if update.blocks.clients.capacity() > limits.max_table_capacity {
        return Err(VisitError::TableCapacityLimit);
    }
    let mut visited = 0usize;
    for blocks in update.blocks.clients.values() {
        for block in blocks {
            visited = visited.checked_add(1).ok_or(VisitError::BlockLimit)?;
            if visited > limits.max_blocks {
                return Err(VisitError::BlockLimit);
            }
            check_range(block.id(), block.len())?;
            let view = match block {
                Block::Item(item) => InputEntry::Item {
                    id: item.id,
                    native_clock_len: item.len,
                    parent: input_parent(item),
                    map_key: item.parent_sub.as_deref(),
                    origin: item.origin,
                    right_origin: item.right_origin,
                    content: content(&item.content),
                },
                Block::GC(r) => InputEntry::Gc {
                    id: r.id(),
                    native_clock_len: r.len,
                },
                Block::Skip(r) => InputEntry::Skip {
                    id: r.id(),
                    native_clock_len: r.len,
                },
            };
            if let ControlFlow::Break(v) = callback(view) {
                return Ok(ControlFlow::Break(v));
            }
        }
    }
    Ok(ControlFlow::Continue(()))
}

fn check_range(id: ID, len: u32) -> Result<(), VisitError> {
    if len == 0 {
        return Err(VisitError::EmptyRange { id });
    }
    id.clock
        .checked_add(len)
        .ok_or(VisitError::RangeOverflow { id })?;
    Ok(())
}
fn identity(branch: &Branch) -> Option<BranchIdentity<'_>> {
    if let Some(item) = branch.item {
        Some(BranchIdentity::Nested(item.id))
    } else {
        branch.name.as_deref().map(BranchIdentity::Root)
    }
}
fn branch_view(branch: &Branch) -> Option<BranchView<'_>> {
    let id = identity(branch)?;
    let parent = match branch.item.as_ref() {
        None if branch.name.is_some() => ParentView::Root,
        Some(item) => match &item.parent {
            TypePtr::Branch(b) => identity(b)
                .map(ParentView::Nested)
                .unwrap_or(ParentView::Unavailable),
            _ => ParentView::Unavailable,
        },
        _ => ParentView::Unavailable,
    };
    let (kind, xml_tag) = kind(&branch.type_ref);
    Some(BranchView {
        id,
        kind,
        xml_tag,
        parent,
        deleted: branch.is_deleted(),
    })
}
fn input_parent(item: &Item) -> InputParent<'_> {
    match &item.parent {
        TypePtr::Named(n) => InputParent::Named(n),
        TypePtr::ID(id) => InputParent::Id(*id),
        TypePtr::Branch(b) => identity(b)
            .map(InputParent::Branch)
            .unwrap_or(InputParent::Unknown),
        TypePtr::Unknown => InputParent::Unknown,
    }
}
fn kind(t: &TypeRef) -> (TypeKind, Option<&str>) {
    match t {
        TypeRef::Array => (TypeKind::Array, None),
        TypeRef::Map => (TypeKind::Map, None),
        TypeRef::Text => (TypeKind::Text, None),
        TypeRef::XmlElement(tag) => (TypeKind::XmlElement, Some(tag)),
        TypeRef::XmlFragment => (TypeKind::XmlFragment, None),
        TypeRef::XmlText => (TypeKind::XmlText, None),
        TypeRef::XmlHook => (TypeKind::XmlHook, None),
        TypeRef::SubDoc => (TypeKind::Subdocument, None),
        #[cfg(feature = "weak")]
        TypeRef::WeakLink(_) => (TypeKind::WeakLink, None),
        TypeRef::Undefined => (TypeKind::Unknown, None),
    }
}
fn content(c: &ItemContent) -> RetainedContent<'_> {
    match c {
        ItemContent::Any(v) => RetainedContent::Values(v),
        ItemContent::JSON(v) => RetainedContent::JsonValues(v),
        ItemContent::String(v) => RetainedContent::String(v.as_str()),
        ItemContent::Embed(v) => RetainedContent::Embed(v),
        ItemContent::Format(k, v) => RetainedContent::Format { key: k, value: v },
        ItemContent::Type(b) => {
            let (kind, xml_tag) = kind(&b.type_ref);
            RetainedContent::Type { kind, xml_tag }
        }
        ItemContent::Binary(v) => RetainedContent::Binary(v),
        ItemContent::Doc(_, _) => RetainedContent::Subdocument,
        ItemContent::Deleted(len) => RetainedContent::Deleted {
            native_clock_len: *len,
        },
    }
}

#[cfg(test)]
#[path = "../tests/retained_accessor.rs"]
mod tests;
