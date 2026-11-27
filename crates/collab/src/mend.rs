//! Mending the grid of a table in a Y document that the allowlist refuses: what the
//! server does alone.
//!
//! Two edits that are each valid can merge into a table that does not fill its grid:
//! one adds a column while another adds a row, which has a cell fewer than the rest; one
//! deletes a row that a cell above reached into. Deleting cannot mend that without
//! losing what people wrote, so the server writes: a span is narrowed to what fits, a
//! row is given the empty cells it lacks at its end, and a cell that fails (one left
//! without content, which every editor deletes) is deleted and an empty one is put in
//! its place, so that the cells after it stay in their columns. Writing is not the same
//! whoever does it, as deleting is: every editor that wrote the same would add cells of
//! its own. So the server mends the one it holds, and its editors are sent what it
//! wrote. (The editor's table plugin pads an unfilled table after any change, another
//! editor's too, so two editors that see one would each pad it.)

use fvoci_model::{BodyError, CellFit, TableGrid};
use serde_json::{Map, Value, json};
use yrs::block::{
    BLOCK_ITEM_ANY_REF_NUMBER, BLOCK_ITEM_TYPE_REF_NUMBER, HAS_ORIGIN, HAS_PARENT_SUB,
    HAS_RIGHT_ORIGIN,
};
use yrs::branch::Branch;
use yrs::encoding::write::Write as _;
use yrs::types::TYPE_REFS_XML_ELEMENT;
use yrs::updates::decoder::Decode as _;
use yrs::updates::encoder::{Encode as _, Encoder as _, EncoderV1};
use yrs::{
    Any, BranchID, ClientID, Doc, ID, IdSet, Number, Out, ReadTxn as _, Transact as _, Update,
    Xml as _, XmlElementRef,
};

use crate::repair::Cuts;

/// The highest clock `scan` accepts: 31 bits.
const MAX_CLOCK: u32 = 0x7fff_ffff;

/// Where a cell of a row that stays comes from.
#[derive(Debug)]
pub(crate) enum CellAt {
    /// The element the row holds.
    Kept(XmlElementRef),
    /// An empty cell put where one that fails was; holds the failed element that
    /// `cuts` deletes so the replacement can be placed right before it.
    Put(XmlElementRef),
}

/// A row of a table that stays, with where each of its cells comes from, in order.
#[derive(Debug)]
pub(crate) struct SurveyedRow {
    pub(crate) element: XmlElementRef,
    pub(crate) cells: Vec<CellAt>,
}

/// The item to the left of a cell that a mend puts into a row.
#[derive(Debug, Clone)]
pub(crate) enum LeftAnchor {
    /// At the start of the row (no cell before it).
    Start,
    /// Immediately after an existing kept cell.
    Kept(XmlElementRef),
    /// Immediately after the cell put by the preceding `Mend::Put` in this row.
    PreviousPut,
}

/// One change the server writes to mend a table, made after the deletions.
#[derive(Debug)]
pub(crate) enum Mend {
    /// An empty cell put among the children of a row where `replaced` (deleted by
    /// `cuts`) stood, immediately after `left`.
    Put {
        row: XmlElementRef,
        left: LeftAnchor,
        replaced: XmlElementRef,
        cell: Value,
    },
    /// Empty cells put at the end of a row, immediately after `after`.
    Pad {
        row: XmlElementRef,
        after: LeftAnchor,
        cells: Vec<Value>,
    },
    /// The spans of a cell written as they fit; `None` for a span that is left as it is.
    Spans {
        cell: XmlElementRef,
        columns: Option<u16>,
        rows: Option<u16>,
    },
}

impl Mend {
    /// How many cells it puts or writes the spans of.
    pub(crate) fn cells(&self) -> usize {
        match self {
            Self::Put { .. } | Self::Spans { .. } => 1,
            Self::Pad { cells, .. } => cells.len(),
        }
    }

    /// How many Y items this mend writes as `writer`, or `None` on `u32` overflow.
    fn item_count(&self) -> Option<u32> {
        match self {
            // Element + child paragraph + colspan + rowspan.
            Self::Put { .. } => Some(4),
            Self::Pad { cells, .. } => u32::try_from(cells.len()).ok()?.checked_mul(4),
            Self::Spans { columns, rows, .. } => {
                Some(u32::from(columns.is_some()) + u32::from(rows.is_some()))
            }
        }
    }
}

/// A single item to write in the mender's update run.
enum MendItem {
    /// An XML element child of `parent` (a row or a new cell).
    Element {
        left: Option<ID>,
        right: Option<ID>,
        parent: ID,
        tag: String,
    },
    /// An integer attribute on `parent` (a cell), replacing `left` if present.
    Attr {
        left: Option<ID>,
        parent: ID,
        key: &'static str,
        value: i64,
    },
}

impl MendItem {
    fn encode(&self, encoder: &mut EncoderV1) {
        match self {
            Self::Element {
                left,
                right,
                parent,
                tag,
            } => {
                let info = BLOCK_ITEM_TYPE_REF_NUMBER
                    | if left.is_some() { HAS_ORIGIN } else { 0 }
                    | if right.is_some() { HAS_RIGHT_ORIGIN } else { 0 };
                encoder.write_info(info);
                if let Some(left) = left {
                    encoder.write_left_id(left);
                }
                if let Some(right) = right {
                    encoder.write_right_id(right);
                }
                if left.is_none() && right.is_none() {
                    encoder.write_parent_info(false);
                    encoder.write_left_id(parent);
                }
                encoder.write_type_ref(TYPE_REFS_XML_ELEMENT);
                encoder.write_key(tag);
            }
            Self::Attr {
                left,
                parent,
                key,
                value,
            } => {
                let info = BLOCK_ITEM_ANY_REF_NUMBER
                    | HAS_PARENT_SUB
                    | if left.is_some() { HAS_ORIGIN } else { 0 };
                encoder.write_info(info);
                if let Some(left) = left {
                    encoder.write_left_id(left);
                } else {
                    encoder.write_parent_info(false);
                    encoder.write_left_id(parent);
                    encoder.write_string(key);
                }
                encoder.write_len(1);
                encoder.write_any(&Any::Number(Number::Int(*value)));
            }
        }
    }
}

/// Mends of a document that every one targets nested elements, and whose items written
/// as `writer` stay within the 31-bit clock limit: the only mends that are written. They
/// hold the document they were checked against, the only one they are written to.
pub(crate) struct CheckedMends<'a> {
    doc: &'a Doc,
    mends: &'a [Mend],
    writer: u64,
    /// How many items writing them takes.
    items: u32,
}

/// Checks `mends` of `doc`, before anything is changed, to write them as `writer`.
///
/// # Errors
///
/// Returns that the document cannot be mended when a mend targets what is not nested in
/// it, or its items would run the clock of `writer` past the limit.
pub(crate) fn check_mends<'a>(
    doc: &'a Doc,
    writer: u64,
    mends: &'a [Mend],
) -> Result<CheckedMends<'a>, BodyError> {
    let items = mends
        .iter()
        .try_fold(0_u32, |acc, mend| acc.checked_add(mend.item_count()?))
        .ok_or_else(unmendable)?;
    let txn = doc.transact();
    let start = txn.state_vector().get(&ClientID::new(writer));
    if start.checked_add(items).is_none_or(|end| end > MAX_CLOCK) {
        return Err(unmendable());
    }
    for mend in mends {
        match mend {
            Mend::Put {
                row,
                left,
                replaced,
                ..
            } => {
                nested_id(row)?;
                nested_id(replaced)?;
                if let LeftAnchor::Kept(element) = left {
                    nested_id(element)?;
                }
            }
            Mend::Pad { row, after, .. } => {
                nested_id(row)?;
                if let LeftAnchor::Kept(element) = after {
                    nested_id(element)?;
                }
            }
            Mend::Spans {
                cell,
                columns,
                rows,
            } => {
                nested_id(cell)?;
                for (name, span) in [("colspan", columns), ("rowspan", rows)] {
                    if span.is_some() {
                        nested_attr_id(&txn, cell, name)?;
                    }
                }
            }
        }
    }
    Ok(CheckedMends {
        doc,
        mends,
        writer,
        items,
    })
}

impl CheckedMends<'_> {
    /// Deletes from the document what `cuts` delete, then writes the mends to it as
    /// their writer, without copying it; returns the version 1 repair update of both.
    ///
    /// # Errors
    ///
    /// Returns that the document cannot be mended if what is written does not apply.
    pub(crate) fn apply_after(self, cuts: &[Cuts]) -> Result<Vec<u8>, BodyError> {
        let doc = self.doc;
        let mut deleted = {
            let mut txn = doc.transact_mut();
            for cuts in cuts {
                cuts.make(&mut txn);
            }
            txn.delete_set().clone()
        };
        let items = self.write(doc, &mut deleted)?;
        apply_encoded_mends(doc, items, &mut deleted)
    }

    /// Encodes the items of the mends, written as their writer after what `doc` holds,
    /// as the run of a version 1 update; what a mend deletes is added to `deleted`.
    fn write(&self, doc: &Doc, deleted: &mut IdSet) -> Result<Vec<u8>, BodyError> {
        let writer_id = ClientID::new(self.writer);
        let start_clock = doc.transact().state_vector().get(&writer_id);
        let mut clock = start_clock;
        let mut items: Vec<MendItem> =
            Vec::with_capacity(usize::try_from(self.items).map_err(|_| unmendable())?);
        let mut last_put: Option<ID> = None;

        for mend in self.mends {
            match mend {
                Mend::Put {
                    row,
                    left,
                    replaced,
                    cell,
                } => {
                    let row_id = nested_id(row)?;
                    let left_id = resolve_anchor(left, last_put)?;
                    let right_id = Some(nested_id(replaced)?);
                    let cell_id = push_cell(
                        &mut items, writer_id, &mut clock, row_id, left_id, right_id, cell,
                    );
                    last_put = Some(cell_id);
                }
                Mend::Pad { row, after, cells } => {
                    let row_id = nested_id(row)?;
                    let mut left_id = resolve_anchor(after, last_put)?;
                    for cell in cells {
                        let cell_id = push_cell(
                            &mut items, writer_id, &mut clock, row_id, left_id, None, cell,
                        );
                        left_id = Some(cell_id);
                    }
                    last_put = None;
                }
                Mend::Spans {
                    cell,
                    columns,
                    rows,
                } => {
                    let cell_id = nested_id(cell)?;
                    for (name, span) in [("colspan", columns), ("rowspan", rows)] {
                        if let Some(span) = span {
                            let prev = remove_attr(doc, cell, name, deleted)?;
                            items.push(MendItem::Attr {
                                left: prev,
                                parent: cell_id,
                                key: name,
                                value: i64::from(*span),
                            });
                            clock += 1;
                        }
                    }
                }
            }
        }

        let mut encoder = EncoderV1::new();
        let count = u32::try_from(items.len()).map_err(|_| unmendable())?;
        encoder.write_var(1_u32);
        encoder.write_var(count);
        encoder.write_client(writer_id);
        encoder.write_var(start_clock);
        for item in &items {
            item.encode(&mut encoder);
        }
        Ok(encoder.to_vec())
    }
}

/// Applies the encoded `writer` run to `doc` and appends `deleted` as the delete set to
/// return the complete version 1 repair update.
fn apply_encoded_mends(
    doc: &Doc,
    mut blocks: Vec<u8>,
    deleted: &mut IdSet,
) -> Result<Vec<u8>, BodyError> {
    // Empty delete set: deletions from `cuts` and `remove_attr` are already in `doc`.
    blocks.push(0);
    let update = Update::decode_v1(&blocks).map_err(|_| unmendable())?;
    blocks.pop();
    {
        let mut txn = doc.transact_mut();
        txn.apply_update(update).map_err(|_| unmendable())?;
        if !txn.delete_set().is_empty() {
            deleted.merge_with(txn.delete_set().clone());
        }
    }
    let mut ds_encoder = EncoderV1::new();
    deleted.encode(&mut ds_encoder);
    blocks.extend_from_slice(&ds_encoder.to_vec());
    Ok(blocks)
}

/// Pushes the items of an empty cell (`tableCell` or `tableHeader`, its child
/// `paragraph`, and its `colspan`/`rowspan` attributes) and returns the cell's ID.
fn push_cell(
    items: &mut Vec<MendItem>,
    writer: ClientID,
    clock: &mut u32,
    row_id: ID,
    left: Option<ID>,
    right: Option<ID>,
    cell: &Value,
) -> ID {
    let kind = cell
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("tableCell")
        .to_owned();
    let cell_id = ID::new(writer, *clock);
    items.push(MendItem::Element {
        left,
        right,
        parent: row_id,
        tag: kind,
    });
    *clock += 1;
    items.push(MendItem::Element {
        left: None,
        right: None,
        parent: cell_id,
        tag: "paragraph".to_owned(),
    });
    *clock += 1;
    let given = cell.get("attrs");
    for name in ["colspan", "rowspan"] {
        let value = given
            .and_then(|attrs| attrs.get(name))
            .and_then(Value::as_i64)
            .unwrap_or(1);
        items.push(MendItem::Attr {
            left: None,
            parent: cell_id,
            key: name,
            value,
        });
        *clock += 1;
    }
    cell_id
}

/// Returns the exact nested item ID of `cell`'s attribute `name` if it holds a nested
/// shared type (`Branch`), or `Ok(None)` if absent or not a nested shared type.
fn nested_attr_id<T: yrs::ReadTxn>(
    txn: &T,
    cell: &XmlElementRef,
    name: &str,
) -> Result<Option<ID>, BodyError> {
    match cell.get_attribute(txn, name) {
        Some(Out::YText(value)) => branch_id(value.as_ref()).map(Some),
        Some(Out::YArray(value)) => branch_id(value.as_ref()).map(Some),
        Some(Out::YMap(value)) => branch_id(value.as_ref()).map(Some),
        Some(Out::YXmlElement(value)) => branch_id(value.as_ref()).map(Some),
        Some(Out::YXmlFragment(value)) => branch_id(value.as_ref()).map(Some),
        Some(Out::YXmlText(value)) => branch_id(value.as_ref()).map(Some),
        _ => Ok(None),
    }
}

/// Removes attribute `name` from `cell`, merges the resulting deletion into `deleted`,
/// and returns the last ID of the removed attribute item (if any) to use as the
/// replacement's left origin.
///
/// For nested shared-type attributes (`ItemContent::Type`), reads the attribute's own
/// `BranchID::Nested` ID before removing it so recursively deleted descendants in
/// `txn.delete_set()` are never mistaken for the attribute item's ID.
fn remove_attr(
    doc: &Doc,
    cell: &XmlElementRef,
    name: &str,
    deleted: &mut IdSet,
) -> Result<Option<ID>, BodyError> {
    let mut txn = doc.transact_mut();
    let nested = nested_attr_id(&txn, cell, name)?;
    cell.remove_attribute(&mut txn, &name);
    let ds = txn.delete_set();
    let prev = nested.or_else(|| {
        ds.iter().next().and_then(|(client, ranges)| {
            ranges
                .iter()
                .next_back()
                .and_then(|range| range.end.checked_sub(1))
                .map(|last| ID::new(*client, last))
        })
    });
    if !ds.is_empty() {
        deleted.merge_with(ds.clone());
    }
    Ok(prev)
}

fn resolve_anchor(anchor: &LeftAnchor, last_put: Option<ID>) -> Result<Option<ID>, BodyError> {
    match anchor {
        LeftAnchor::Start => Ok(None),
        LeftAnchor::Kept(element) => Ok(Some(nested_id(element)?)),
        LeftAnchor::PreviousPut => Ok(last_put),
    }
}

fn nested_id(element: &XmlElementRef) -> Result<ID, BodyError> {
    branch_id(element.as_ref())
}

fn branch_id(branch: &Branch) -> Result<ID, BodyError> {
    match branch.id() {
        BranchID::Nested(id) => Ok(id),
        BranchID::Root(_) => Err(unmendable()),
    }
}

fn unmendable() -> BodyError {
    BodyError::InvalidNode {
        path: String::new(),
        problem: "the table cannot be mended in place",
    }
}

/// The JSON of an empty cell of a kind: one empty paragraph.
pub(crate) fn empty_cell(kind: &str) -> Value {
    json!({"type": kind, "content": [{"type": "paragraph"}]})
}

/// Mends the grid of a table whose rows each stay: the JSON of the table as its grid
/// mends it, and the changes that make the shared content so. `rows` are the rows of
/// the table's content, in order, each with where its cells come from.
///
/// # Errors
///
/// Returns [`BodyError::TableTooLarge`], with an empty path, for a table past a limit,
/// which no mending brings under it.
pub(crate) fn mend_grid(
    mut table: Value,
    rows: &[SurveyedRow],
) -> Result<(Value, Vec<Mend>), BodyError> {
    let grid = TableGrid::of(&table).map_err(|_| BodyError::TableTooLarge {
        path: String::new(),
    })?;
    let mut mends = Vec::new();
    let nodes = table
        .get_mut("content")
        .and_then(Value::as_array_mut)
        .map_or(&mut [][..], Vec::as_mut_slice);
    debug_assert_eq!(grid.rows().len(), rows.len(), "a row surveyed for each row");
    debug_assert_eq!(grid.rows().len(), nodes.len(), "a node for each row");
    for ((fit, surveyed), node) in grid.rows().iter().zip(rows).zip(nodes) {
        let cells = content_mut(node)?;
        debug_assert_eq!(
            fit.cells.len(),
            surveyed.cells.len(),
            "a cell surveyed for each"
        );
        debug_assert_eq!(fit.cells.len(), cells.len(), "a node for each cell");
        let mut prev = LeftAnchor::Start;
        for ((cell, at), json) in fit.cells.iter().zip(&surveyed.cells).zip(cells.iter_mut()) {
            write_spans(json, *cell);
            match at {
                CellAt::Put(replaced) => {
                    mends.push(Mend::Put {
                        row: surveyed.element.clone(),
                        left: prev,
                        replaced: replaced.clone(),
                        cell: json.clone(),
                    });
                    prev = LeftAnchor::PreviousPut;
                }
                CellAt::Kept(element) => {
                    if cell.columns.changed || cell.rows.changed {
                        mends.push(Mend::Spans {
                            cell: element.clone(),
                            columns: cell.columns.changed.then_some(cell.columns.span),
                            rows: cell.rows.changed.then_some(cell.rows.span),
                        });
                    }
                    prev = LeftAnchor::Kept(element.clone());
                }
            }
        }
        if fit.missing > 0 {
            // A row of headers is given headers, a row of cells cells.
            let kind = cells
                .last()
                .and_then(|last| last.get("type"))
                .and_then(Value::as_str)
                .unwrap_or("tableCell")
                .to_owned();
            let pad = vec![empty_cell(&kind); fit.missing];
            cells.extend(pad.iter().cloned());
            mends.push(Mend::Pad {
                row: surveyed.element.clone(),
                after: prev,
                cells: pad,
            });
        }
    }
    Ok((table, mends))
}

/// The children of a node, made a list if it has none.
fn content_mut(node: &mut Value) -> Result<&mut Vec<Value>, BodyError> {
    let members = node.as_object_mut().ok_or_else(unmendable)?;
    let content = members
        .entry("content")
        .or_insert_with(|| Value::Array(Vec::new()));
    if !content.is_array() {
        *content = Value::Array(Vec::new());
    }
    content.as_array_mut().ok_or_else(unmendable)
}

/// Writes into the JSON of a cell the spans that changed, in the stored form: a span of
/// one is left out.
fn write_spans(cell: &mut Value, fit: CellFit) {
    let Some(members) = cell.as_object_mut() else {
        return;
    };
    let mut attrs = match members.remove("attrs") {
        Some(Value::Object(attrs)) => attrs,
        _ => Map::new(),
    };
    for (name, reach) in [("colspan", fit.columns), ("rowspan", fit.rows)] {
        if !reach.changed {
            continue;
        }
        if reach.span == 1 {
            attrs.remove(name);
        } else {
            attrs.insert(name.to_owned(), Value::from(reach.span));
        }
    }
    if !attrs.is_empty() {
        members.insert("attrs".to_owned(), Value::Object(attrs));
    }
}
