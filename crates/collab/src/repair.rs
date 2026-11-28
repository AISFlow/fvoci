//! Repairing a Y document that the allowlist refuses.

mod judge;
mod table;

use std::collections::BTreeSet;

use fvoci_model::{BODY_MAX_DEPTH, Body, BodyError};
use serde_json::{Value, json};
use yrs::{
    Array, ArrayRef, Doc, ID, IdSet, ReadTxn, Transact, TransactionMut, XmlElementRef,
    XmlFragment as _, XmlFragmentRef,
};

use self::judge::{
    ContentAccum, NodeSummary, is_valid_root_block, judge_element, judge_text_nodes,
};
use self::table::{
    KeptCellMetrics, RowCellMetrics, TABLE_CELL_SHELL_BYTES, TABLE_HEADER_SHELL_BYTES,
    check_standalone_row_limits, summarize_mended_table,
};
use crate::FIELD;
use crate::block_cache::{
    ApplyDelta, BlockCache, BlockEntry, BlockSummary, DocTotals, check_totals, element_id,
    project_and_validate_block,
};
use crate::mend::{self, CellAt, Mend, SurveyedRow};
use crate::project::{project, to_body};
use crate::read::{self, Child};
use crate::schema;

/// What an in-place repair did on a [`crate::BodyDoc`], without materializing the full
/// [`Body`] JSON tree on the hot path.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct InPlaceRepair {
    /// How many nodes and texts were deleted, each with all it held.
    pub deleted: usize,
    /// How many cells of tables were put in, or had their spans written again, to mend
    /// the tables' grids.
    pub mended: usize,
    /// The deletions and what mended the tables, as a version 1 update for the editors
    /// that hold the document; `None` if nothing was changed.
    pub update: Option<Vec<u8>>,
}

impl InPlaceRepair {
    /// No repair was needed.
    #[must_use]
    pub(crate) const fn none() -> Self {
        Self {
            deleted: 0,
            mended: 0,
            update: None,
        }
    }
}

/// What a repair did.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Repair {
    /// How many nodes and texts were deleted, each with all it held.
    pub deleted: usize,
    /// How many cells of tables were put in, or had their spans written again, to mend
    /// the tables' grids (`mend.rs`): by the server alone, as no editor does. With
    /// [`Self::deleted`] 0, the document was a body already and is as it was.
    pub mended: usize,
    /// The deletions and what mended the tables, as a version 1 update for the editors
    /// that hold the document; `None` if nothing was changed.
    pub update: Option<Vec<u8>>,
    /// The body of the document as it now is; `None` for no body.
    pub body: Option<Body>,
}

/// Repairs a Y document in place: deletes from it what keeps it from being a body,
/// mends any broken table grid, and gives the body that is left.
///
/// Two edits that are each valid can merge into a document that is not: two people each
/// delete one of the last two entries of a list, and a list without entries is left.
/// Every editor deletes shared content its schema refuses (as long as it is made
/// without its content check, with which it keeps the content and stops editing),
/// and so does this, with the
/// allowlist as the judge: a node that fails by itself is deleted with all it holds (a
/// kind nobody knows, an attribute that is not allowed, too few children, a child of a
/// kind it may not hold, a list entry that does not start with a paragraph), from the
/// bottom up, so that a parent is judged with the children it has left. A text is
/// deleted whole if a run of it fails. Deleting is the same whoever does it and however
/// often, so the editors and the server end with the same document.
///
/// A table is mended, not only deleted from: what the merge of two edits leaves in a
/// table that does not fill its grid (a row a cell short, a span past the last row) is
/// written here, as `mend.rs` says, and the editors are sent it. A cell that fails is
/// deleted as every editor deletes it, and an empty one is put in its place; a row that
/// holds anything but cells goes whole, as in an editor.
///
/// The allowlist refuses more than the editor's schema, which checks no value: a
/// heading of level 9 or a link to `javascript:` stays in an editor and is deleted here.
/// At the root an editor wraps a node the document may not hold (a list entry, a
/// mention) instead of deleting it; this deletes it, and a text at the root, on which
/// an editor's binding throws. And a node that holds letters or values where its
/// children belong, which an editor passes over, is deleted whole.
///
/// The deletions and table mends are applied directly to `doc` without copying it;
/// nothing is changed if `doc` is a body already or if surveying and validating what
/// would be left returns an error. What the repair writes is written as a client named
/// by the document ([`writer_for`]): the same document is repaired into the same
/// changes. A server that opens a document takes its logged updates again, each
/// followed by the repair logged after it; the update is repaired as it was, and the
/// logged repair then adds nothing, where one written as a new client would have added
/// its cells to the first's.
///
/// # Errors
///
/// Nothing is deleted when an error is returned before mutations begin.
///
/// Returns [`BodyError::TooLarge`], [`BodyError::TooDeep`], [`BodyError::TooManyNodes`] or
/// [`BodyError::TableTooLarge`] if what would be left is past a limit, or a node that is
/// judged is past one by itself: deleting what fails is not meant to bring a document
/// under a limit. Returns [`BodyError::InvalidNode`] with an empty path if the root
/// itself holds letters or values (there is no node to delete in their place) or if
/// more than a thousand separate stretches of children would have to be deleted (a
/// repair is for what a merge of edits leaves, and each stretch costs a walk through
/// its parent). Any other error means the document would still be refused with the
/// deletions.
pub(crate) fn repaired(doc: &Doc) -> Result<Repair, BodyError> {
    repaired_by(doc, None)
}

/// The first client a repair writes as: past every client an editor picks, which are
/// below 2³².
const FIRST_WRITER: u64 = 1 << 32;

/// How many clients a repair may write as: those from [`FIRST_WRITER`] up to 2⁵³, past
/// which no client is (an editor in a browser reads one as a number).
const WRITERS: u64 = (1 << 53) - FIRST_WRITER;

/// The client a repair of `doc` writes as, named by what the document holds: where the
/// changes of each of its clients end. Two documents with the same changes are repaired
/// as one client into the same changes; once it has written, the document holds more,
/// and its next repair is another client's.
fn writer_for(doc: &Doc) -> u64 {
    let mut ends: Vec<(u64, u32)> = doc
        .transact()
        .state_vector()
        .iter()
        .map(|(client, end)| (client.get(), *end))
        .collect();
    ends.sort_unstable();
    // FNV-1a over 64 bits: the same in every build and on every machine, which the
    // standard library's hashers are not promised to be.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for (client, end) in ends {
        for byte in client.to_le_bytes().into_iter().chain(end.to_le_bytes()) {
            hash = (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    FIRST_WRITER + hash % WRITERS
}

/// [`repaired`], what it writes written as `writer`, or as the client the document names
/// ([`writer_for`]) if `None`: a test names one to have the same changes from any
/// document.
///
/// # Errors
///
/// As [`repaired`].
pub(crate) fn repaired_by(doc: &Doc, writer: Option<u64>) -> Result<Repair, BodyError> {
    if let Ok(body) = to_body(doc) {
        return Ok(Repair {
            deleted: 0,
            mended: 0,
            update: None,
            body,
        });
    }
    deleting(doc, writer)
}

/// Repairs `doc` in place when it is no body, after surveying and validating what would
/// be left before mutating anything.
///
/// # Errors
///
/// As [`repaired`].
fn deleting(doc: &Doc, writer: Option<u64>) -> Result<Repair, BodyError> {
    let writer = writer.unwrap_or_else(|| writer_for(doc));
    let root = doc.get_or_insert_xml_fragment(FIELD);
    let survey = {
        let txn = doc.transact();
        Survey { txn: &txn }.root_cached(&root, None)?
    };
    check_totals(DocTotals {
        blocks: survey.kept_blocks,
        nodes: survey.kept_nodes,
        board_views: survey.kept_board_views,
        json_bytes: survey.kept_json_bytes,
        is_empty_doc: survey.only_empty_paragraph,
    })?;
    let Changes {
        cuts,
        mends,
        cut_elements: _,
    } = survey.changes;
    if cuts.iter().map(Cuts::stretches).sum::<usize>() > MAX_STRETCHES {
        return Err(BodyError::InvalidNode {
            path: String::new(),
            problem: "more separate parts of the document fail than a repair deletes",
        });
    }
    let checked = if mends.is_empty() {
        None
    } else {
        Some(mend::check_mends(doc, writer, &mends)?)
    };
    let deleted = cuts.iter().map(|cuts| cuts.indexes.len()).sum();
    let mended = mends.iter().map(Mend::cells).sum();
    let Some(checked) = checked else {
        let mut txn = doc.transact_mut();
        for cuts in &cuts {
            cuts.make(&mut txn);
        }
        let body = project(&txn, &root)?;
        let update = (deleted > 0).then(|| txn.encode_update_v1());
        drop(txn);
        return Ok(Repair {
            deleted,
            mended: 0,
            update,
            body,
        });
    };
    let update = checked.apply_after(&cuts)?;
    let body = {
        let txn = doc.transact();
        project(&txn, &root)?
    };
    Ok(Repair {
        deleted,
        mended,
        update: Some(update),
        body,
    })
}

/// Validates `doc` incrementally using `cache` and `delta`, repairing only dirty top-level
/// blocks (plus any tables with empty text inside rows) when validation fails.
///
/// Returns the repair summary and the `IdSet` of items deleted by a deletion-only repair
/// (empty when no repair ran or when table mends also added items).
pub(crate) fn validate_or_repair_in_place(
    doc: &Doc,
    cache: &mut BlockCache,
    delta: &ApplyDelta,
) -> Result<(InPlaceRepair, IdSet), BodyError> {
    match cache.apply_delta(doc, delta) {
        Ok(()) => Ok((InPlaceRepair::none(), IdSet::new())),
        Err(dirty) => deleting_with_cache(doc, cache, Some(&dirty)),
    }
}

/// Validates `doc` block-by-block into a fresh [`BlockCache`], repairing `doc` in place
/// and populating the cache if `doc` is not yet a valid body.
pub(crate) fn build_or_repair_cache(
    doc: &Doc,
) -> Result<(BlockCache, InPlaceRepair, IdSet), BodyError> {
    if let Ok(cache) = BlockCache::build(doc) {
        return Ok((cache, InPlaceRepair::none(), IdSet::new()));
    }
    let mut cache = BlockCache::new();
    let (repair, repair_ds) = deleting_with_cache(doc, &mut cache, None)?;
    Ok((cache, repair, repair_ds))
}

/// Repairs `doc` in place while reusing clean entries from `cache` (when `dirty` is
/// `Some`) and refreshing `cache` for all surveyed blocks after applying cuts and mends.
fn deleting_with_cache(
    doc: &Doc,
    cache: &mut BlockCache,
    dirty: Option<&BTreeSet<ID>>,
) -> Result<(InPlaceRepair, IdSet), BodyError> {
    let writer = writer_for(doc);
    let root = doc.get_or_insert_xml_fragment(FIELD);
    let survey = {
        let txn = doc.transact();
        Survey { txn: &txn }.root_cached(&root, dirty.map(|ids| (&*cache, ids)))?
    };
    check_totals(DocTotals {
        blocks: survey.kept_blocks,
        nodes: survey.kept_nodes,
        board_views: survey.kept_board_views,
        json_bytes: survey.kept_json_bytes,
        is_empty_doc: survey.only_empty_paragraph,
    })?;
    let Changes {
        cuts,
        mends,
        cut_elements,
    } = survey.changes;
    if cuts.iter().map(Cuts::stretches).sum::<usize>() > MAX_STRETCHES {
        return Err(BodyError::InvalidNode {
            path: String::new(),
            problem: "more separate parts of the document fail than a repair deletes",
        });
    }
    let checked = if mends.is_empty() {
        None
    } else {
        Some(mend::check_mends(doc, writer, &mends)?)
    };
    let deleted = cuts.iter().map(|cuts| cuts.indexes.len()).sum();
    let mended = mends.iter().map(Mend::cells).sum();
    let Some(checked) = checked else {
        let mut txn = doc.transact_mut();
        for cuts in &cuts {
            cuts.make(&mut txn);
        }
        refresh_cache_after_repair(
            &txn,
            &root,
            cache,
            survey.surveyed_kept,
            &survey.surveyed_removed,
            &cut_elements,
            RepairKind::CutsOnly,
        )?;
        let update = (deleted > 0).then(|| txn.encode_update_v1());
        txn.commit();
        let repair_ds = txn.delete_set().clone();
        drop(txn);
        return Ok((
            InPlaceRepair {
                deleted,
                mended: 0,
                update,
            },
            repair_ds,
        ));
    };
    let update = checked.apply_after(&cuts)?;
    {
        let txn = doc.transact();
        refresh_cache_after_repair(
            &txn,
            &root,
            cache,
            survey.surveyed_kept,
            &survey.surveyed_removed,
            &cut_elements,
            RepairKind::WithMends,
        )?;
    }
    Ok((
        InPlaceRepair {
            deleted,
            mended,
            update: Some(update),
        },
        IdSet::new(),
    ))
}

/// Whether an in-place repair performed only cuts or also inserted table-mend cells.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RepairKind {
    CutsOnly,
    WithMends,
}

/// Updates `cache` for all blocks surveyed during repair and re-verifies root and cut
/// parent integrity and whole-document limits against `doc`, mirroring
/// `project(&txn, &root)?` without re-projecting clean or non-mended large blocks.
fn refresh_cache_after_repair<T: ReadTxn>(
    txn: &T,
    root: &XmlFragmentRef,
    cache: &mut BlockCache,
    kept: Vec<KeptSurveyBlock>,
    removed: &[ID],
    cut_elements: &[XmlElementRef],
    kind: RepairKind,
) -> Result<(), BodyError> {
    for id in removed {
        cache.remove(id);
    }
    for element in cut_elements {
        // A cut can leave a parent that holds what is no XML (a formatting item first,
        // which is not counted). Reading the whole document says where.
        if !read::holds_only_xml(txn, element) {
            let _ = project(txn, root)?;
            return Err(BodyError::InvalidNode {
                path: String::new(),
                problem: "holds something that is neither a node nor a text",
            });
        }
    }
    for block in kept {
        let entry = if kind == RepairKind::WithMends && block.may_be_mended {
            project_and_validate_block(txn, &block.element)?
        } else {
            block.entry
        };
        cache.upsert(block.id, entry);
    }
    let root_len = usize::try_from(root.len(txn)).unwrap_or(usize::MAX);
    if cache.len() > root_len {
        cache.retain_live_root_elements(txn, root);
    }
    if cache.len() != root_len || (root_len > 0 && root.first_child().is_none()) {
        return Err(BodyError::InvalidNode {
            path: String::new(),
            problem: "holds something that is neither a node nor a text",
        });
    }
    cache.check_whole_limits()?;
    Ok(())
}

/// How many stretches of neighbouring children a repair deletes at most. The library
/// deletes by index and walks to it from the front of the parent, over what was deleted
/// before as well, so a repair costs about this many walks through a parent. What two
/// edits leave when they merge is a stretch or two; a document that fails in a thousand
/// separate places was made to.
const MAX_STRETCHES: usize = 1000;

/// The children of one parent to delete: their indexes, in order.
#[derive(Debug)]
pub(crate) struct Cuts {
    parent: ArrayRef,
    indexes: Vec<u32>,
}

impl Cuts {
    /// The stretches of neighbouring children among them: where each begins and how
    /// many it is long.
    fn each_stretch(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        self.indexes
            .chunk_by(|one, next| one.checked_add(1) == Some(*next))
            .filter_map(|stretch| Some((*stretch.first()?, u32::try_from(stretch.len()).ok()?)))
    }

    fn stretches(&self) -> usize {
        self.each_stretch().count()
    }

    /// Deletes the children, each stretch of neighbours at once, from the last to the
    /// first, so that no deletion moves the place of another.
    pub(crate) fn make(&self, txn: &mut TransactionMut<'_>) {
        let stretches: Vec<(u32, u32)> = self.each_stretch().collect();
        for (start, length) in stretches.into_iter().rev() {
            // The indexes were read from this parent, so the stretch is there;
            // deleting past the end of a parent would panic in the library.
            if start.saturating_add(length) <= self.parent.len(txn) {
                self.parent.remove_range(txn, start, length);
            }
        }
    }
}

/// A walk over shared content that judges every node and notes which to delete,
/// without changing anything.
struct Survey<'txn, T> {
    txn: &'txn T,
}

/// What a survey found below one non-table element.
#[derive(Debug, Default)]
struct ElementSurvey {
    content: ContentAccum,
    changes: Changes,
}

/// Outcome of surveying one child under a parent.
#[derive(Debug)]
enum ChildOutcome {
    Element(NodeSummary),
    Text(Vec<judge::TextRun>),
}

/// A kept top-level block surveyed during repair, with its exact post-repair cache entry.
#[derive(Debug)]
struct KeptSurveyBlock {
    id: ID,
    element: XmlElementRef,
    entry: BlockEntry,
    /// Whether a mend may have changed it after its survey: a table, or a toggle, which
    /// may hold tables.
    may_be_mended: bool,
}

/// What [`Survey::root_cached`] collected without materializing clean or non-table blocks'
/// JSON trees.
#[derive(Debug, Default)]
struct CachedRootSurvey {
    kept_blocks: usize,
    kept_nodes: u64,
    kept_board_views: u64,
    kept_json_bytes: u64,
    only_empty_paragraph: bool,
    changes: Changes,
    surveyed_kept: Vec<KeptSurveyBlock>,
    surveyed_removed: Vec<ID>,
}

/// What a repair changes below a node: the children to delete, parent by parent, and
/// the mends of tables, made after them.
#[derive(Debug, Default)]
struct Changes {
    cuts: Vec<Cuts>,
    mends: Vec<Mend>,
    /// Non-root `XmlElement` parents that had a cut at index 0, checked post-cut for
    /// exposed non-countable `ItemContent::Format`.
    cut_elements: Vec<XmlElementRef>,
}

impl Changes {
    /// Notes the children of a parent that are to be deleted, if there are any.
    fn note(&mut self, parent: &ArrayRef, indexes: Vec<u32>) {
        if !indexes.is_empty() {
            self.cuts.push(Cuts {
                parent: parent.clone(),
                indexes,
            });
        }
    }

    /// Notes cuts on a non-root element parent, recording it if index 0 is cut.
    fn note_element(&mut self, element: &XmlElementRef, indexes: Vec<u32>) {
        if indexes.first() == Some(&0) {
            self.cut_elements.push(element.clone());
        }
        self.note(element.as_ref(), indexes);
    }

    /// Takes on the changes below a node that stays.
    fn take(&mut self, below: Self) {
        self.cuts.extend(below.cuts);
        self.mends.extend(below.mends);
        self.cut_elements.extend(below.cut_elements);
    }

    /// How many changes of each kind there are, to go back to.
    fn mark(&self) -> (usize, usize, usize) {
        (self.cuts.len(), self.mends.len(), self.cut_elements.len())
    }

    /// Drops what was noted after a mark: the changes below a node that goes.
    fn rewind(&mut self, (cuts, mends, cut_elements): (usize, usize, usize)) {
        self.cuts.truncate(cuts);
        self.mends.truncate(mends);
        self.cut_elements.truncate(cut_elements);
    }
}

/// What [`Survey::row`] produced for one surviving table row.
#[derive(Debug)]
struct RowSurvey {
    light_row: Value,
    surveyed_row: SurveyedRow,
    cell_metrics: Vec<RowCellMetrics>,
}

impl<T: ReadTxn> Survey<'_, T> {
    /// Surveys the document while skipping clean top-level blocks from `cached` (blocks
    /// not in `dirty` and without empty text inside a `tableRow`), walking `root`'s
    /// linked list directly without allocating a `Vec` of all root children.
    fn root_cached(
        &self,
        root: &XmlFragmentRef,
        cached: Option<(&BlockCache, &BTreeSet<ID>)>,
    ) -> Result<CachedRootSurvey, BodyError> {
        let mut out = CachedRootSurvey::default();
        let mut own = Vec::new();
        let mut count = 0_u32;
        let mut children = read::Siblings::of(self.txn, root);
        while let Some(child) = children.next() {
            let index = count;
            count = count.saturating_add(1);
            if let Some((cache, dirty)) = cached
                && let Child::Element(element) = &child
                && let Some(id) = element_id(element)
                && !dirty.contains(&id)
                && let Some(summary) = cache.get(&id)
                && !summary.has_empty_row_text
            {
                out.kept_blocks += 1;
                out.kept_nodes = out.kept_nodes.saturating_add(u64::from(summary.nodes));
                out.kept_board_views = out
                    .kept_board_views
                    .saturating_add(u64::from(summary.board_views));
                out.kept_json_bytes = out
                    .kept_json_bytes
                    .saturating_add(u64::from(summary.json_bytes));
                out.only_empty_paragraph = out.kept_blocks == 1 && summary.is_empty_paragraph;
                continue;
            }
            if let Err(err) = self.survey_root_child(index, child, &mut out, &mut own) {
                // Whatever a child fails, a root that holds what is no XML fails as that.
                let rest = u32::try_from(children.count()).unwrap_or(u32::MAX);
                if count.saturating_add(rest) != root.len(self.txn) {
                    return Err(BodyError::InvalidNode {
                        path: String::new(),
                        problem: "the root holds something that is neither a node nor a text",
                    });
                }
                return Err(err);
            }
        }
        if count != root.len(self.txn) {
            return Err(BodyError::InvalidNode {
                path: String::new(),
                problem: "the root holds something that is neither a node nor a text",
            });
        }
        out.changes.note(root.as_ref(), own);
        Ok(out)
    }

    fn survey_root_child(
        &self,
        index: u32,
        child: Child,
        out: &mut CachedRootSurvey,
        own: &mut Vec<u32>,
    ) -> Result<(), BodyError> {
        let noted = out.changes.mark();
        let elem = match &child {
            Child::Element(element) => element_id(element).map(|id| (id, element.clone())),
            Child::Text(_) | Child::Other => None,
        };
        match self.child(child, 2, &mut out.changes)? {
            Some(ChildOutcome::Element(summary)) if is_valid_root_block(&summary) => {
                let nodes = u64::try_from(summary.nodes).unwrap_or(u64::MAX);
                let bytes = u64::try_from(summary.json_bytes).unwrap_or(u64::MAX);
                out.kept_blocks += 1;
                out.kept_nodes = out.kept_nodes.saturating_add(nodes);
                out.kept_board_views = out
                    .kept_board_views
                    .saturating_add(u64::try_from(summary.board_views).unwrap_or(u64::MAX));
                out.kept_json_bytes = out.kept_json_bytes.saturating_add(bytes);
                out.only_empty_paragraph = out.kept_blocks == 1 && summary.is_empty_paragraph;
                if let Some((id, element)) = elem {
                    let may_be_mended = matches!(summary.kind, "table" | "details");
                    out.surveyed_kept.push(KeptSurveyBlock {
                        id,
                        element,
                        entry: BlockEntry {
                            summary: BlockSummary {
                                nodes: u32::try_from(summary.nodes).unwrap_or(u32::MAX),
                                json_bytes: u32::try_from(summary.json_bytes).unwrap_or(u32::MAX),
                                is_empty_paragraph: summary.is_empty_paragraph,
                                has_empty_row_text: false,
                                board_views: u32::try_from(summary.board_views).unwrap_or(u32::MAX),
                            },
                            file_ids: summary.file_ids.into_boxed_slice(),
                        },
                        may_be_mended,
                    });
                }
            }
            _ => {
                out.changes.rewind(noted);
                own.push(index);
                if let Some((id, _)) = elem {
                    out.surveyed_removed.push(id);
                }
            }
        }
        Ok(())
    }

    /// Surveys the children of an element, which are at `depth`. `None` if the element
    /// holds something that is neither a node nor a text, and so goes whole.
    fn element(
        &self,
        element: &XmlElementRef,
        depth: usize,
    ) -> Result<Option<ElementSurvey>, BodyError> {
        let Some(children) = read::children(self.txn, element) else {
            return Ok(None);
        };
        let mut found = ElementSurvey::default();
        let mut own = Vec::new();
        for (index, child) in children.into_iter().enumerate() {
            match self.child(child, depth, &mut found.changes)? {
                Some(ChildOutcome::Element(summary)) => found.content.push_element(summary),
                Some(ChildOutcome::Text(runs)) => found.content.push_text_runs(runs),
                None => own.push(index_of(index)),
            }
        }
        found.changes.note_element(element, own);
        Ok(Some(found))
    }

    /// Judges a table at `depth` whole: its rows (with zero-clone cell-content surveys),
    /// then its grid via [`mend::mend_grid`] and [`fvoci_model::TableGrid`]. `None` if it
    /// is to be deleted.
    fn table(
        &self,
        table: &XmlElementRef,
        depth: usize,
        changes: &mut Changes,
    ) -> Result<Option<NodeSummary>, BodyError> {
        let Some(children) = read::children(self.txn, table) else {
            return Ok(None);
        };
        let mut below = Changes::default();
        let mut light_rows = Vec::new();
        let mut rows = Vec::new();
        let mut row_metrics = Vec::new();
        let mut own = Vec::new();
        let mut has_non_row = false;
        for (index, child) in children.into_iter().enumerate() {
            let stays = match child {
                Child::Element(row) if row.tag().as_ref() == "tableRow" => {
                    self.row(row, depth + 1, &mut below)?.map(|surveyed| {
                        light_rows.push(surveyed.light_row);
                        rows.push(surveyed.surveyed_row);
                        row_metrics.push(surveyed.cell_metrics);
                    })
                }
                other => match self.child(other, depth + 1, &mut below)? {
                    Some(ChildOutcome::Text(runs)) if runs.is_empty() => Some(()),
                    Some(_) => {
                        has_non_row = true;
                        Some(())
                    }
                    None => None,
                },
            };
            if stays.is_none() {
                own.push(index_of(index));
            }
        }
        // Anything but rows that stays makes the table go, as in an editor.
        if has_non_row || light_rows.is_empty() {
            return Ok(None);
        }
        below.note_element(table, own);
        let light_table = read::element_node(self.txn, table, light_rows);
        let (mended_table, mends) = mend::mend_grid(light_table, &rows)?;
        let Some(summary) = summarize_mended_table(&mended_table, &row_metrics)? else {
            return Ok(None);
        };
        changes.take(below);
        changes.mends.extend(mends);
        Ok(Some(summary))
    }

    /// Judges a row of a table at `depth`: its stub JSON row for `TableGrid` (with an
    /// empty cell where a cell that fails was), its `SurveyedRow`, and real cell metrics.
    /// `None` if the row is to be deleted.
    fn row(
        &self,
        row: XmlElementRef,
        depth: usize,
        changes: &mut Changes,
    ) -> Result<Option<RowSurvey>, BodyError> {
        if depth > BODY_MAX_DEPTH {
            return Err(BodyError::TooDeep);
        }
        let Some(children) = read::children(self.txn, &row) else {
            return Ok(None);
        };
        let mut below = Changes::default();
        let mut light_cells = Vec::new();
        let mut cells = Vec::new();
        let mut cell_metrics = Vec::new();
        let mut own = Vec::new();
        let mut has_non_cell = false;
        for (index, child) in children.into_iter().enumerate() {
            let cell = match &child {
                Child::Element(element) if is_cell(element.tag()) => Some(element.clone()),
                _ => None,
            };
            match (self.child(child, depth + 1, &mut below)?, cell) {
                (Some(ChildOutcome::Element(summary)), Some(element)) => {
                    let unspanned = if summary.kind == "tableHeader" {
                        TABLE_HEADER_SHELL_BYTES
                    } else {
                        TABLE_CELL_SHELL_BYTES
                    };
                    light_cells.push(read::element_node(
                        self.txn,
                        &element,
                        vec![json!({"type": "paragraph"})],
                    ));
                    cells.push(CellAt::Kept(element));
                    cell_metrics.push(RowCellMetrics::Kept(KeptCellMetrics {
                        nodes: summary.nodes,
                        height: summary.height,
                        unspanned_shell_bytes: unspanned,
                        content_suffix_bytes: summary.json_bytes.saturating_sub(unspanned),
                        file_ids: summary.file_ids,
                    }));
                }
                // A text with nothing in it, which an editor passes over, goes: then
                // the row's children are its cells, and a cell put in counts among them.
                (Some(ChildOutcome::Text(runs)), None) if runs.is_empty() => {
                    own.push(index_of(index));
                }
                (Some(_), None) => has_non_cell = true,
                (None, Some(element)) => {
                    own.push(index_of(index));
                    let unspanned = if element.tag().as_ref() == "tableHeader" {
                        TABLE_HEADER_SHELL_BYTES
                    } else {
                        TABLE_CELL_SHELL_BYTES
                    };
                    light_cells.push(read::element_node(
                        self.txn,
                        &element,
                        vec![json!({"type": "paragraph"})],
                    ));
                    cells.push(CellAt::Put(element));
                    cell_metrics.push(RowCellMetrics::Put {
                        unspanned_shell_bytes: unspanned,
                    });
                }
                (None, None) => own.push(index_of(index)),
                (Some(ChildOutcome::Text(_)), Some(_)) => {
                    return Err(BodyError::InvalidNode {
                        path: String::new(),
                        problem: "a table cell yielded text instead of an element",
                    });
                }
            }
        }
        // Anything but cells that stays makes the row go, as in an editor.
        if has_non_cell {
            return Ok(None);
        }
        // Check standalone row limits in exact `Body::parse` precedence order.
        check_standalone_row_limits(&cell_metrics)?;
        below.note_element(&row, own);
        let light_row = read::element_node(self.txn, &row, light_cells);
        changes.take(below);
        Ok(Some(RowSurvey {
            light_row,
            surveyed_row: SurveyedRow {
                element: row,
                cells,
            },
            cell_metrics,
        }))
    }

    /// Judges one child at `depth`: its summary if it stays, `None` if it is to be
    /// deleted. The deletions below a child that stays are added to `changes`; those
    /// below one that goes are dropped, as they go with it.
    fn child(
        &self,
        child: Child,
        depth: usize,
        changes: &mut Changes,
    ) -> Result<Option<ChildOutcome>, BodyError> {
        match child {
            Child::Element(element) => {
                if depth > BODY_MAX_DEPTH {
                    return Err(BodyError::TooDeep);
                }
                if element.tag().as_ref() == "table" {
                    return self
                        .table(&element, depth, changes)
                        .map(|opt| opt.map(ChildOutcome::Element));
                }
                let Some(below) = self.element(&element, depth + 1)? else {
                    return Ok(None);
                };
                let shell = read::element_node(self.txn, &element, Vec::new());
                let tag = element.tag();
                if let Some(summary) = judge_element(
                    tag.as_ref(),
                    schema::place(tag.as_ref()),
                    shell,
                    below.content,
                )? {
                    changes.take(below.changes);
                    Ok(Some(ChildOutcome::Element(summary)))
                } else {
                    Ok(None)
                }
            }
            Child::Text(text) => {
                let Some(nodes) = read::text_nodes(self.txn, &text) else {
                    return Ok(None);
                };
                if !nodes.is_empty() && depth > BODY_MAX_DEPTH {
                    return Err(BodyError::TooDeep);
                }
                Ok(judge_text_nodes(nodes)?.map(ChildOutcome::Text))
            }
            Child::Other => Ok(None),
        }
    }
}

/// The index of a child. A Y document has no more children under one parent than fit.
fn index_of(index: usize) -> u32 {
    u32::try_from(index).unwrap_or(u32::MAX)
}

/// Whether an element of this name is a cell of a table.
fn is_cell(tag: &str) -> bool {
    matches!(tag, "tableCell" | "tableHeader")
}
