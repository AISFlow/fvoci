//! Per-top-level-block projection and validation cache for [`crate::BodyDoc`].
//!
//! A `ProseMirror`/`TipTap` body lives under the root `XmlFragment("default")` as a flat
//! sequence of independent top-level block elements (`paragraph`, `heading`, lists,
//! `blockquote`, `codeBlock`, `horizontalRule`, `image`, `file`, `blockMath`, `table`,
//! `details`, `callout`).
//! Ordinary edits mutate only a handful of those blocks. Storing a compact summary per
//! live top-level block (keyed by its immutable Yjs [`ID`], with no retained
//! `serde_json::Value` tree) lets [`crate::BodyDoc::take_in_place`] project, validate,
//! and size only the blocks touched by an update while keeping peak heap and per-doc
//! memory small.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use fvoci_model::{BODY_MAX_BOARD_VIEWS, BODY_MAX_BYTES, BODY_MAX_NODES, Body, BodyError};
use serde_json::json;
use yrs::branch::Branch;
use yrs::{
    BranchID, ClientID, Doc, ID, IdSet, ReadTxn, Transact as _, Xml as _, XmlElementRef,
    XmlFragment as _, XmlFragmentRef, XmlOut,
};

use crate::FIELD;
use crate::project::{json_size_of, project_element};
use crate::read::{self, Child};

/// Compact JSON byte length of `{"content":[],"type":"doc"}` around a document's blocks.
const DOC_WRAPPER_BYTES: usize = r#"{"content":[],"type":"doc"}"#.len();

/// Compact JSON byte length of a single empty paragraph `{"type":"paragraph"}`.
const EMPTY_PARAGRAPH_BYTES: usize = r#"{"type":"paragraph"}"#.len();

/// What a single in-place `apply_update` transaction changed in the document, captured
/// right after `TransactionMut::commit` in `O(changed)` time without walking the root.
#[derive(Debug)]
pub(crate) struct ApplyDelta {
    /// Nested branch IDs from `TransactionMut::changed_parent_types` (every existing
    /// modified branch and its ancestors up to the root).
    pub(crate) changed_branches: Vec<ID>,
    /// All ID ranges deleted by the transaction.
    pub(crate) delete_set: IdSet,
    /// Shared-type IDs newly introduced without an explicit wire `member` (from `update::read`).
    pub(crate) new_types: Vec<ID>,
}

/// Compact validated summary of one live top-level block under `FIELD`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BlockSummary {
    /// Total JSON nodes in this block (the block itself plus all descendants).
    pub(crate) nodes: u32,
    /// Exact compact JSON byte length of this block as serialized by `serde_json`.
    pub(crate) json_bytes: u32,
    /// Whether this block is an empty paragraph (`{"type":"paragraph"}`).
    pub(crate) is_empty_paragraph: bool,
    /// Whether any `tableRow` inside this block contains an empty `XmlText` child,
    /// which `repair::Survey::row` deletes during a repair even though `project` skips it.
    pub(crate) has_empty_row_text: bool,
    /// The board views in this block: the block itself, or those of what a toggle hides.
    pub(crate) board_views: u32,
}

/// A validated block's summary together with any file IDs it references.
#[derive(Debug)]
pub(crate) struct BlockEntry {
    pub(crate) summary: BlockSummary,
    pub(crate) file_ids: Box<[String]>,
}

/// Incremental per-block cache for a valid [`crate::BodyDoc`].
///
/// Invariant when synchronized with a `Doc`:
/// - Every key `id` in `blocks` is a live (`!is_deleted()`) direct `XmlElement` child of
///   the root fragment `FIELD` (`parent_sub` is `None`).
/// - `blocks.len() == root.len(&txn)` (so the root holds no `XmlText`, `XmlFragment`, or
///   non-XML countable item).
/// - `total_nodes`, `total_board_views`, `total_json_bytes`, and `file_ids` match the
///   exact sums/entries over `blocks`.
#[derive(Debug, Clone, Default)]
pub(crate) struct BlockCache {
    blocks: BTreeMap<ID, BlockSummary>,
    /// Sparse map of deduplicated document-order `fileId`s for blocks that contain at
    /// least one `image` or `file` node. Empty for text-only documents (zero allocation).
    file_ids: HashMap<ID, Box<[String]>>,
    total_nodes: u64,
    total_board_views: u64,
    total_json_bytes: u64,
}

impl BlockCache {
    /// Creates an empty cache matching an empty document.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Number of cached live top-level blocks.
    #[must_use]
    pub(crate) fn len(&self) -> usize {
        self.blocks.len()
    }

    /// Whether there are no cached top-level blocks.
    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    /// Whether `id` is currently cached as a live top-level block.
    #[must_use]
    fn contains(&self, id: &ID) -> bool {
        self.blocks.contains_key(id)
    }

    /// Returns the cached summary for `id`, if present.
    #[must_use]
    pub(crate) fn get(&self, id: &ID) -> Option<&BlockSummary> {
        self.blocks.get(id)
    }

    /// Inserts or replaces the cached entry for `id`, keeping running sums exact.
    pub(crate) fn upsert(&mut self, id: ID, entry: BlockEntry) {
        if let Some(old) = self.blocks.insert(id, entry.summary) {
            self.total_nodes = self.total_nodes.saturating_sub(u64::from(old.nodes));
            self.total_board_views = self
                .total_board_views
                .saturating_sub(u64::from(old.board_views));
            self.total_json_bytes = self
                .total_json_bytes
                .saturating_sub(u64::from(old.json_bytes));
        }
        self.total_nodes = self
            .total_nodes
            .saturating_add(u64::from(entry.summary.nodes));
        self.total_board_views = self
            .total_board_views
            .saturating_add(u64::from(entry.summary.board_views));
        self.total_json_bytes = self
            .total_json_bytes
            .saturating_add(u64::from(entry.summary.json_bytes));
        if entry.file_ids.is_empty() {
            self.file_ids.remove(&id);
        } else {
            self.file_ids.insert(id, entry.file_ids);
        }
    }

    /// Removes `id` from the cache if present, updating running sums.
    pub(crate) fn remove(&mut self, id: &ID) {
        if let Some(old) = self.blocks.remove(id) {
            self.total_nodes = self.total_nodes.saturating_sub(u64::from(old.nodes));
            self.total_board_views = self
                .total_board_views
                .saturating_sub(u64::from(old.board_views));
            self.total_json_bytes = self
                .total_json_bytes
                .saturating_sub(u64::from(old.json_bytes));
        }
        self.file_ids.remove(id);
    }

    /// Removes every cached top-level block belonging to `client` with `clock` in
    /// `[start, end)`.
    fn remove_range(&mut self, client: ClientID, start: u32, end: u32) {
        if start >= end {
            return;
        }
        let lo = ID::new(client, start);
        let hi = ID::new(client, end);
        let removed: Vec<ID> = self.blocks.range(lo..hi).map(|(id, _)| *id).collect();
        for id in removed {
            self.remove(&id);
        }
    }

    /// Empties all cached blocks and releases backing table allocations.
    pub(crate) fn clear(&mut self) {
        *self = Self::new();
    }

    /// Removes any cached entries that are not currently live `XmlElement` list children
    /// of `root` (`parent_sub` is `None`), keeping running totals and `file_ids` exact.
    pub(crate) fn retain_live_root_elements<T: ReadTxn>(&mut self, txn: &T, root: &XmlFragmentRef) {
        let live: HashSet<ID> = read::Siblings::of(txn, root)
            .filter_map(|child| match child {
                Child::Element(element) => element_id(&element),
                Child::Text(_) | Child::Other => None,
            })
            .collect();
        let stale: Vec<ID> = self
            .blocks
            .keys()
            .copied()
            .filter(|id| !live.contains(id))
            .collect();
        for id in stale {
            self.remove(&id);
        }
    }

    /// Whether the cached blocks represent a single empty paragraph (`Body::parse` -> `None`).
    #[must_use]
    pub(crate) fn is_empty_document(&self) -> bool {
        self.blocks.len() == 1
            && self
                .blocks
                .values()
                .next()
                .is_some_and(|summary| summary.is_empty_paragraph)
    }

    /// Exact compact JSON byte length of the non-empty body, or `None` when the document
    /// has no body (empty root or one empty paragraph).
    #[must_use]
    pub(crate) fn body_bytes(&self) -> Option<usize> {
        if self.is_empty() || self.is_empty_document() {
            return None;
        }
        Some(
            usize::try_from(document_json_bytes(
                self.total_json_bytes,
                self.blocks.len(),
            ))
            .unwrap_or(usize::MAX),
        )
    }

    /// Enforces whole-document node count and JSON byte limits with the exact precedence
    /// of [`Body::parse`] (`TooManyNodes`, then empty-doc check, then `TooLarge`).
    ///
    /// # Errors
    ///
    /// Returns [`BodyError::TooManyNodes`] or [`BodyError::TooLarge`] when over the limit.
    pub(crate) fn check_whole_limits(&self) -> Result<(), BodyError> {
        check_totals(DocTotals {
            blocks: self.blocks.len(),
            nodes: self.total_nodes,
            board_views: self.total_board_views,
            json_bytes: self.total_json_bytes,
            is_empty_doc: self.is_empty_document(),
        })
    }

    /// Builds a fresh cache by projecting and validating every top-level block in `doc`
    /// one at a time, immediately dropping each block's JSON tree and stopping early as
    /// soon as `BODY_MAX_NODES` or `BODY_MAX_BYTES` is exceeded.
    ///
    /// # Errors
    ///
    /// Returns a [`BodyError`] if `doc` is not a valid body.
    pub(crate) fn build(doc: &Doc) -> Result<Self, BodyError> {
        let txn = doc.transact();
        let Some(root) = txn.get_xml_fragment(FIELD) else {
            return Ok(Self::new());
        };
        let Some(children) = read::children(&txn, &root) else {
            return Err(BodyError::InvalidNode {
                path: String::new(),
                problem: "holds something that is neither a node nor a text",
            });
        };
        let mut cache = Self::new();
        let max_nodes = u64::try_from(BODY_MAX_NODES).unwrap_or(u64::MAX);
        let max_bytes = u64::try_from(BODY_MAX_BYTES).unwrap_or(u64::MAX);
        for (index, child) in children.into_iter().enumerate() {
            match child {
                Child::Element(element) => {
                    let Some(id) = element_id(&element) else {
                        return Err(BodyError::InvalidNode {
                            path: format!("/content/{index}"),
                            problem: "root child is not a nested element",
                        });
                    };
                    let entry = project_and_validate_block(&txn, &element)?;
                    cache.upsert(id, entry);
                    if 1_u64.saturating_add(cache.total_nodes) > max_nodes {
                        return Err(BodyError::TooManyNodes);
                    }
                    if cache.len() >= 2
                        && document_json_bytes(cache.total_json_bytes, cache.len()) > max_bytes
                    {
                        return Err(BodyError::TooLarge);
                    }
                }
                Child::Text(_) => {
                    return Err(BodyError::InvalidNode {
                        path: format!("/content/{index}"),
                        problem: "a text stands at the root",
                    });
                }
                Child::Other => {
                    return Err(BodyError::InvalidNode {
                        path: format!("/content/{index}"),
                        problem: "shared content is neither a node nor a text",
                    });
                }
            }
        }
        cache.check_whole_limits()?;
        Ok(cache)
    }

    /// Updates `self` incrementally from `delta` after an in-place update on `doc`.
    ///
    /// Returns `Ok(())` if the updated document is a valid body within all allowlist,
    /// node, depth, table, and `BODY_MAX_BYTES` limits. Returns `Err(dirty)` with the
    /// set of dirty top-level block IDs if any block failed validation, the root child
    /// count no longer matches the cached elements, or whole-document limits were
    /// exceeded — signaling that incremental repair should run.
    pub(crate) fn apply_delta(
        &mut self,
        doc: &Doc,
        delta: &ApplyDelta,
    ) -> Result<(), BTreeSet<ID>> {
        let txn = doc.transact();
        let Some(root) = txn.get_xml_fragment(FIELD) else {
            self.clear();
            return Ok(());
        };

        self.drop_deleted(delta);
        let dirty = self.dirty_blocks(&txn, &root, delta);
        if self.reproject(&txn, &dirty).is_none()
            || !self.root_holds_only_blocks(&txn, &root)
            || self.check_whole_limits().is_err()
        {
            return Err(dirty);
        }
        Ok(())
    }

    /// Drops the top-level blocks deleted in `delta`: O(log N + deleted).
    fn drop_deleted(&mut self, delta: &ApplyDelta) {
        for (&client, ranges) in delta.delete_set.iter() {
            for range in ranges.iter() {
                self.remove_range(client, range.start, range.end);
            }
        }
    }

    /// The top-level blocks to read again after `delta`: the live elements it put
    /// straight into the root's list (`parent_sub` is `None`), and the cached blocks
    /// whose subtrees or attributes it changed.
    fn dirty_blocks<T: ReadTxn>(
        &self,
        txn: &T,
        root: &XmlFragmentRef,
        delta: &ApplyDelta,
    ) -> BTreeSet<ID> {
        let first_child_id = root.first_child().map(|first| first.id());
        let mut list_check = RootListCheck::default();
        let mut dirty = BTreeSet::new();
        for &id in &delta.new_types {
            let Some(branch) = BranchID::get_nested(txn, &id) else {
                continue;
            };
            if branch.is_deleted() {
                continue;
            }
            if let Ok(XmlOut::Element(element)) = XmlOut::try_from(branch)
                && list_check.is_direct_list_child(
                    txn,
                    self,
                    root,
                    first_child_id.as_ref(),
                    id,
                    &element,
                )
            {
                dirty.insert(id);
            }
        }
        for id in &delta.changed_branches {
            if self.contains(id) {
                dirty.insert(*id);
            }
        }
        dirty
    }

    /// Projects and validates the `dirty` blocks again, and caches what they are now.
    /// `None` at the first that is gone, is no element or fails.
    fn reproject<T: ReadTxn>(&mut self, txn: &T, dirty: &BTreeSet<ID>) -> Option<()> {
        for &id in dirty {
            let branch = BranchID::get_nested(txn, &id)?;
            let Ok(XmlOut::Element(element)) = XmlOut::try_from(branch) else {
                return None;
            };
            let entry = project_and_validate_block(txn, &element).ok()?;
            self.upsert(id, entry);
        }
        Some(())
    }

    /// Whether the root holds the cached blocks and nothing else: as many children as
    /// blocks (no stray text, fragment or item that is no XML), and, when it holds any,
    /// a first child that no formatting item (`ItemContent::Format`, which is not
    /// counted) shadows.
    fn root_holds_only_blocks<T: ReadTxn>(&self, txn: &T, root: &XmlFragmentRef) -> bool {
        let root_len = usize::try_from(root.len(txn)).unwrap_or(usize::MAX);
        self.len() == root_len && (root_len == 0 || root.first_child().is_some())
    }

    /// How many board views the cached blocks have, matching [`Body::board_view_count`].
    #[must_use]
    pub(crate) fn board_views(&self) -> u64 {
        self.total_board_views
    }

    /// Returns the deduplicated stored file names referenced by `image` and `file` nodes
    /// in exact document order, matching [`Body::file_ids`].
    #[must_use]
    pub(crate) fn file_ids_in_order(&self, doc: &Doc) -> Vec<String> {
        match self.file_ids.len() {
            0 => Vec::new(),
            1 => self
                .file_ids
                .values()
                .next()
                .map_or_else(Vec::new, |ids| ids.to_vec()),
            total_file_blocks => {
                let txn = doc.transact();
                let Some(root) = txn.get_xml_fragment(FIELD) else {
                    return Vec::new();
                };
                let mut seen: HashSet<&str> = HashSet::new();
                let mut out: Vec<String> = Vec::new();
                let mut remaining = total_file_blocks;
                for child in read::Siblings::of(&txn, &root) {
                    if let Child::Element(element) = child
                        && let Some(id) = element_id(&element)
                        && let Some(ids) = self.file_ids.get(&id)
                    {
                        for file_id in ids {
                            if seen.insert(file_id.as_str()) {
                                out.push(file_id.clone());
                            }
                        }
                        remaining -= 1;
                        if remaining == 0 {
                            break;
                        }
                    }
                }
                out
            }
        }
    }
}

/// Projects and validates a single top-level `XmlElement` at depth 2 using
/// [`fvoci_model::Body::parse`], returning its compact summary and file IDs while
/// immediately dropping the temporary JSON tree.
pub(crate) fn project_and_validate_block<T: ReadTxn>(
    txn: &T,
    element: &XmlElementRef,
) -> Result<BlockEntry, BodyError> {
    let (value, nodes, has_empty_row_text) = project_element(txn, element)?;
    let wrapped = json!({"type": "doc", "content": [value]});
    let parsed = Body::parse(wrapped)?;
    match parsed {
        None => Ok(BlockEntry {
            summary: BlockSummary {
                nodes: 1,
                json_bytes: u32::try_from(EMPTY_PARAGRAPH_BYTES).unwrap_or(u32::MAX),
                is_empty_paragraph: true,
                has_empty_row_text,
                board_views: 0,
            },
            file_ids: Box::new([]),
        }),
        Some(body) => {
            let block_bytes = json_size_of(body.document()).saturating_sub(DOC_WRAPPER_BYTES);
            let file_ids: Box<[String]> = body.file_ids().into_iter().map(str::to_owned).collect();
            Ok(BlockEntry {
                summary: BlockSummary {
                    nodes: u32::try_from(nodes).unwrap_or(u32::MAX),
                    json_bytes: u32::try_from(block_bytes).unwrap_or(u32::MAX),
                    is_empty_paragraph: false,
                    has_empty_row_text,
                    board_views: u32::try_from(body.board_view_count()).unwrap_or(u32::MAX),
                },
                file_ids,
            })
        }
    }
}

/// Exact compact JSON byte length of `{"content":[...],"type":"doc"}` for `block_count`
/// non-empty blocks whose individual compact JSON lengths sum to `sum_block_bytes`.
#[must_use]
fn document_json_bytes(sum_block_bytes: u64, block_count: usize) -> u64 {
    let wrapper = u64::try_from(DOC_WRAPPER_BYTES).unwrap_or(u64::MAX);
    let commas = u64::try_from(block_count.saturating_sub(1)).unwrap_or(u64::MAX);
    wrapper
        .saturating_add(sum_block_bytes)
        .saturating_add(commas)
}

/// Whole-document aggregate counts checked against `BODY_MAX_NODES` and `BODY_MAX_BYTES`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct DocTotals {
    pub(crate) blocks: usize,
    pub(crate) nodes: u64,
    pub(crate) board_views: u64,
    pub(crate) json_bytes: u64,
    pub(crate) is_empty_doc: bool,
}

/// Enforces `BODY_MAX_NODES`, `BODY_MAX_BOARD_VIEWS` and `BODY_MAX_BYTES` in the exact
/// order of [`Body::parse`].
pub(crate) fn check_totals(totals: DocTotals) -> Result<(), BodyError> {
    if totals.blocks == 0 {
        return Ok(());
    }
    let max_nodes = u64::try_from(BODY_MAX_NODES).unwrap_or(u64::MAX);
    if 1_u64.saturating_add(totals.nodes) > max_nodes {
        return Err(BodyError::TooManyNodes);
    }
    if totals.board_views > u64::try_from(BODY_MAX_BOARD_VIEWS).unwrap_or(u64::MAX) {
        return Err(BodyError::TooManyBoardViews);
    }
    if totals.is_empty_doc {
        return Ok(());
    }
    let max_bytes = u64::try_from(BODY_MAX_BYTES).unwrap_or(u64::MAX);
    if document_json_bytes(totals.json_bytes, totals.blocks) > max_bytes {
        return Err(BodyError::TooLarge);
    }
    Ok(())
}

/// Memoized classifier verifying that newly inserted `XmlElement`s belong to `root`'s
/// sequence list (`parent_sub` is `None`) rather than a map entry (`parent_sub` is
/// `Some(_)`), visiting each root-list item at most once across `apply_delta`.
///
/// In `yrs 0.28`, `Xml::parent()` returns `Some(XmlOut::Fragment(root))` for map entries
/// attached to `root` as well as list children, whereas each `Branch` links sequence
/// items (`parent_sub == None`) into `branch.start` and map entries into disjoint chains
/// in `branch.map`. Walking `Siblings::next_back` until hitting either an already-cached
/// (or already-confirmed) root-list element or the list's leftmost `XmlOut` and matching
/// that head against `root.first_child()` distinguishes `branch.start` from every map
/// chain (including multi-client concurrent map chains on the same key).
#[derive(Default)]
struct RootListCheck {
    in_list: HashSet<ID>,
    not_in_list: HashSet<ID>,
}

impl RootListCheck {
    fn is_direct_list_child<T: ReadTxn>(
        &mut self,
        txn: &T,
        cache: &BlockCache,
        root: &XmlFragmentRef,
        first_child_id: Option<&BranchID>,
        id: ID,
        element: &XmlElementRef,
    ) -> bool {
        if !matches!(element.parent(), Some(XmlOut::Fragment(parent)) if parent == *root) {
            return false;
        }
        if cache.contains(&id) || self.in_list.contains(&id) {
            return true;
        }
        if self.not_in_list.contains(&id) {
            return false;
        }
        let mut visited = vec![id];
        let mut leftmost = BranchID::Nested(id);
        let mut siblings = element.siblings(txn);
        while let Some(prev) = siblings.next_back() {
            let prev_branch_id = prev.id();
            if let BranchID::Nested(prev_id) = &prev_branch_id {
                if matches!(prev, XmlOut::Element(_))
                    && (cache.contains(prev_id) || self.in_list.contains(prev_id))
                {
                    self.in_list.extend(visited);
                    return true;
                }
                if self.not_in_list.contains(prev_id) {
                    self.not_in_list.extend(visited);
                    return false;
                }
                if matches!(prev, XmlOut::Element(_)) {
                    visited.push(*prev_id);
                }
            }
            leftmost = prev_branch_id;
        }
        if first_child_id == Some(&leftmost) {
            self.in_list.extend(visited);
            true
        } else {
            self.not_in_list.extend(visited);
            false
        }
    }
}

/// Returns the immutable nested [`ID`] of an XML element, or `None` for a root branch.
#[must_use]
pub(crate) fn element_id(element: &XmlElementRef) -> Option<ID> {
    let branch: &Branch = element.as_ref();
    match branch.id() {
        BranchID::Nested(id) => Some(id),
        BranchID::Root(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_wrapper_and_empty_paragraph_byte_constants_match_serde_json() {
        let empty_doc = serde_json::to_vec(&json!({"type": "doc", "content": []})).expect("json");
        assert_eq!(empty_doc.len(), DOC_WRAPPER_BYTES);
        let empty_para = serde_json::to_vec(&json!({"type": "paragraph"})).expect("json");
        assert_eq!(empty_para.len(), EMPTY_PARAGRAPH_BYTES);

        let blocks = [
            json!({"type": "paragraph"}),
            json!({"type": "paragraph", "content": [{"type": "text", "text": "한글😀"}]}),
            json!({"type": "horizontalRule"}),
        ];
        let sum: u64 = blocks
            .iter()
            .map(|b| u64::try_from(json_size_of(b)).expect("fits"))
            .sum();
        let whole =
            u64::try_from(json_size_of(&json!({"type": "doc", "content": blocks}))).expect("fits");
        assert_eq!(whole, document_json_bytes(sum, blocks.len()));
    }

    fn apply_raw_update_with_delta(doc: &Doc, update_bytes: &[u8]) -> ApplyDelta {
        use yrs::updates::decoder::Decode as _;
        let mut txn = doc.transact_mut();
        let before = txn.state_vector();
        txn.apply_update(yrs::Update::decode_v1(update_bytes).expect("valid yrs v1 update"))
            .expect("applies");
        txn.commit();
        let after = txn.state_vector();
        // Collect every newly introduced nested branch ID across `before..after` (including
        // map entries, proving `BlockCache` rejects them even when present in `new_types`).
        let mut new_types = Vec::new();
        for (&client, &end_clock) in after.iter() {
            let start = before.get(&client);
            for clock in start..end_clock {
                let id = ID::new(client, clock);
                if BranchID::get_nested(&txn, &id).is_some() {
                    new_types.push(id);
                }
            }
        }
        let changed_branches = txn
            .changed_parent_types()
            .iter()
            .filter_map(|branch| match branch.id() {
                BranchID::Nested(id) => Some(id),
                BranchID::Root(_) => None,
            })
            .collect();
        let delete_set = txn.delete_set().clone();
        ApplyDelta {
            changed_branches,
            delete_set,
            new_types,
        }
    }

    fn doc_with_paragraph(client: u64, text: &str) -> Doc {
        use yrs::{OffsetKind, Options, XmlElementPrelim, XmlTextPrelim};
        let doc = Doc::with_options(Options {
            offset_kind: OffsetKind::Utf16,
            skip_gc: true,
            ..Options::with_client_id(ClientID::new(client))
        });
        {
            let root = doc.get_or_insert_xml_fragment(FIELD);
            let mut txn = doc.transact_mut();
            let p = root.push_back(&mut txn, XmlElementPrelim::empty("paragraph"));
            p.push_back(&mut txn, XmlTextPrelim::new(text));
        }
        doc
    }

    /// Attack scenario 1: injecting an invalid root list item (`XmlText("evil")`) together
    /// with valid `XmlElement` entries in `root`'s map (`parent_sub = Some("k")`, including
    /// `image`/`file` nodes and multi-client concurrent map entries) in a single update
    /// must NOT satisfy `cache.len() == root_len`: repair must delete `XmlText("evil")`,
    /// match `repaired(&ref_doc)` + `to_body(&doc)`, and never leak map-entry `file_ids`.
    #[test]
    fn root_map_entry_plus_evil_list_item_repairs_and_never_leaks_file_ids() {
        use yrs::updates::decoder::Decode as _;
        use yrs::{Map as _, OffsetKind, Options, XmlElementPrelim, XmlTextPrelim};

        let doc = doc_with_paragraph(1, "P1");
        let base_state = crate::update::state_of(&doc);
        let mut cache = BlockCache::build(&doc).expect("valid initial cache");
        assert_eq!(cache.len(), 1);

        // Build a single update on peer 20 that injects BOTH `XmlText("evil")` into
        // `root`'s sequence list AND a valid `paragraph` + `image`/`file` into `root`'s map
        // (`parent_sub = Some("k")` / `Some("f")`), plus a concurrent map entry on `"k"`
        // from peer 19 so `M_20.siblings().next_back()` is `Some(M_19)`!
        let make_peer = |cid: u64| {
            let peer = Doc::with_options(Options {
                offset_kind: OffsetKind::Utf16,
                skip_gc: true,
                ..Options::with_client_id(ClientID::new(cid))
            });
            peer.get_or_insert_xml_fragment(FIELD);
            peer.transact_mut()
                .apply_update(yrs::Update::decode_v1(&base_state).expect("base"))
                .expect("apply");
            peer
        };
        let peer_lo = make_peer(19);
        let before = peer_lo.transact().state_vector();
        {
            let root_map = peer_lo.get_or_insert_map(FIELD);
            let mut txn = peer_lo.transact_mut();
            let m_lo = root_map.insert(&mut txn, "k", XmlElementPrelim::empty("paragraph"));
            m_lo.push_back(&mut txn, XmlTextPrelim::new("concurrent-map-para"));
        }
        let u_lo = peer_lo.transact().encode_diff_v1(&before);

        let peer_hi = make_peer(20);
        {
            let root = peer_hi.get_or_insert_xml_fragment(FIELD);
            let root_map = peer_hi.get_or_insert_map(FIELD);
            let mut txn = peer_hi.transact_mut();
            root.push_back(&mut txn, XmlTextPrelim::new("evil"));
            let bq = root_map.insert(&mut txn, "k", XmlElementPrelim::empty("blockquote"));
            let img = bq.push_back(&mut txn, XmlElementPrelim::empty("image"));
            img.insert_attribute(&mut txn, "fileId", "secret-map.png");
            let file = root_map.insert(&mut txn, "f", XmlElementPrelim::empty("file"));
            file.insert_attribute(&mut txn, "fileId", "secret-map.pdf");
        }
        let u_hi = peer_hi.transact().encode_diff_v1(&before);
        let combined = yrs::merge_updates_v1([&u_lo, &u_hi]).expect("merged");

        let ref_doc = make_peer(99);
        ref_doc
            .transact_mut()
            .apply_update(yrs::Update::decode_v1(&combined).expect("decode"))
            .expect("apply ref");
        let expected = crate::repair::repaired(&ref_doc).expect("ref repairs");
        assert_eq!(expected.deleted, 1, "cold repair deletes XmlText(\"evil\")");

        let delta = apply_raw_update_with_delta(&doc, &combined);
        // Confirm `yrs 0.28` really reports `Some(XmlOut::Fragment(root))` for the map entries.
        {
            let txn = doc.transact();
            let root = txn.get_xml_fragment(FIELD).expect("root");
            let map_elements = delta
                .new_types
                .iter()
                .filter_map(|id| BranchID::get_nested(&txn, id))
                .filter_map(|b| XmlOut::try_from(b).ok())
                .filter(|out| {
                    matches!(out, XmlOut::Element(el)
                        if matches!(el.parent(), Some(XmlOut::Fragment(p)) if p == root))
                })
                .count();
            assert_eq!(map_elements, 3, "all 3 map elements report root as parent");
        }

        let (got_repair, _) =
            crate::repair::validate_or_repair_in_place(&doc, &mut cache, &delta).expect("repairs");
        assert_eq!(got_repair.deleted, 1, "XmlText(\"evil\") must be repaired");
        assert_eq!(got_repair.update, expected.update);
        assert_eq!(crate::project::to_body(&doc), Ok(expected.body));
        assert_eq!(cache.len(), 1);
        assert_eq!(
            cache.file_ids_in_order(&doc),
            Vec::<String>::new(),
            "map-entry image/file IDs must never appear in file_ids()"
        );
    }

    /// Scenario 2: injecting ONLY a valid `XmlElement("paragraph")` in `root`'s map
    /// (`parent_sub = Some("k")`) leaves `cache.len() == root_len == 1` without triggering
    /// a false `Dirty` error, and `deleting_with_cache` purges any pre-existing non-list
    /// entry in `cache`.
    #[test]
    fn root_map_entry_alone_keeps_cache_consistent_and_purges_on_repair() {
        use yrs::updates::decoder::Decode as _;
        use yrs::{Map as _, OffsetKind, Options, XmlElementPrelim, XmlTextPrelim};

        let doc = doc_with_paragraph(1, "P1");
        let base_state = crate::update::state_of(&doc);
        let mut cache = BlockCache::build(&doc).expect("valid initial cache");

        let peer = Doc::with_options(Options {
            offset_kind: OffsetKind::Utf16,
            skip_gc: true,
            ..Options::with_client_id(ClientID::new(30))
        });
        peer.get_or_insert_xml_fragment(FIELD);
        peer.transact_mut()
            .apply_update(yrs::Update::decode_v1(&base_state).expect("base"))
            .expect("apply");
        let before = peer.transact().state_vector();
        {
            let root_map = peer.get_or_insert_map(FIELD);
            let mut txn = peer.transact_mut();
            let m = root_map.insert(&mut txn, "k", XmlElementPrelim::empty("paragraph"));
            m.push_back(&mut txn, XmlTextPrelim::new("map-only"));
        }
        let map_only = peer.transact().encode_diff_v1(&before);

        let ref_clean = {
            let d = Doc::with_options(Options {
                offset_kind: OffsetKind::Utf16,
                skip_gc: true,
                ..Options::with_client_id(ClientID::new(98))
            });
            d.get_or_insert_xml_fragment(FIELD);
            d.transact_mut()
                .apply_update(yrs::Update::decode_v1(&base_state).expect("base"))
                .expect("apply");
            d.transact_mut()
                .apply_update(yrs::Update::decode_v1(&map_only).expect("map_only"))
                .expect("apply");
            d
        };
        let expected = crate::repair::repaired(&ref_clean).expect("cold repair");
        assert_eq!(expected.deleted, 0);

        let delta = apply_raw_update_with_delta(&doc, &map_only);
        let map_id = *delta.new_types.first().expect("map element ID");
        let (got_repair, _) = crate::repair::validate_or_repair_in_place(&doc, &mut cache, &delta)
            .expect("map-only update must not trigger false Dirty error");
        assert_eq!(got_repair.deleted, 0);
        assert!(got_repair.update.is_none());
        assert_eq!(cache.len(), 1);
        assert_eq!(crate::project::to_body(&doc), Ok(expected.body.clone()));

        // Even if a non-list ID were somehow polluted into `cache`, `deleting_with_cache`
        // (`refresh_cache_after_repair`) purges it cleanly.
        cache.upsert(
            map_id,
            BlockEntry {
                summary: BlockSummary {
                    nodes: 2,
                    json_bytes: 40,
                    is_empty_paragraph: false,
                    has_empty_row_text: false,
                    board_views: 0,
                },
                file_ids: vec!["bogus.png".to_owned()].into_boxed_slice(),
            },
        );
        assert_eq!(cache.len(), 2);
        let empty_delta = ApplyDelta {
            changed_branches: Vec::new(),
            delete_set: IdSet::new(),
            new_types: Vec::new(),
        };
        let (purged_repair, _) =
            crate::repair::validate_or_repair_in_place(&doc, &mut cache, &empty_delta)
                .expect("purges polluted map entry cleanly");
        assert_eq!(purged_repair.deleted, 0);
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.file_ids_in_order(&doc), Vec::<String>::new());
        assert_eq!(crate::project::to_body(&doc), Ok(expected.body));
    }
}
