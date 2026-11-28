//! Whole-table limit and size accounting for [`super::Survey`].
//!
//! Tables are still surveyed and mended whole through [`crate::mend::mend_grid`] and
//! [`fvoci_model::TableGrid`], while each kept cell carries its pre-validated inner
//! metrics alongside a lightweight stub cell so cell subtrees are never cloned.

use fvoci_model::{BODY_MAX_BYTES, BODY_MAX_DEPTH, BODY_MAX_NODES, BodyError, TABLE_MAX_COLUMNS};
use serde_json::Value;

use super::judge::{BLOCK_WRAPPER_BYTES, CONTENT_FIELD_OVERHEAD, NodeSummary, dedup_strings};
pub(super) use super::judge::{TABLE_CELL_SHELL_BYTES, TABLE_HEADER_SHELL_BYTES};
use crate::project::json_size_of;

/// Compact JSON byte lengths of table wrapper shells and the stub cell content carried
/// on lightweight cells passed to `TableGrid`.
pub(super) const TABLE_SHELL_BYTES: usize = r#"{"type":"table"}"#.len();
pub(super) const TABLE_ROW_SHELL_BYTES: usize = r#"{"type":"tableRow"}"#.len();
pub(super) const STUB_CELL_CONTENT_BYTES: usize = r#","content":[{"type":"paragraph"}]"#.len();

/// Real inner summary of a kept table cell, carried beside its lightweight stub cell so
/// `TableGrid` and `mend::mend_grid` run whole without cloning cell subtrees.
#[derive(Debug)]
pub(super) struct KeptCellMetrics {
    /// Subtree nodes of the cell (`1 + inner_nodes`).
    pub(super) nodes: usize,
    /// Subtree height of the cell (`1 + max_inner_height`).
    pub(super) height: usize,
    /// Unspanned shell bytes (`tableCell` or `tableHeader`).
    pub(super) unspanned_shell_bytes: usize,
    /// Exact compact JSON bytes of `,"content":[...]` inside this cell.
    pub(super) content_suffix_bytes: usize,
    /// File IDs referenced inside this cell.
    pub(super) file_ids: Vec<String>,
}

/// Metrics of one cell slot in a surveyed table row (`Kept` vs `Put`).
#[derive(Debug)]
pub(super) enum RowCellMetrics {
    Kept(KeptCellMetrics),
    Put { unspanned_shell_bytes: usize },
}

/// Checks standalone row node, depth, width, and byte limits in the exact precedence
/// order of `Body::parse` (`passes(&row, Place::TableRow)`).
pub(super) fn check_standalone_row_limits(
    cell_metrics: &[RowCellMetrics],
) -> Result<(), BodyError> {
    // Wrapper `doc -> table` = 2 nodes, depth 2; row itself = +1 node, depth 3.
    let mut nodes = 3_usize;
    let mut row_bytes = TABLE_ROW_SHELL_BYTES;
    for (idx, metric) in cell_metrics.iter().enumerate() {
        let (cell_nodes, cell_height, cell_bytes) = match metric {
            RowCellMetrics::Kept(kept) => (
                kept.nodes,
                kept.height,
                kept.unspanned_shell_bytes
                    .saturating_add(kept.content_suffix_bytes),
            ),
            RowCellMetrics::Put {
                unspanned_shell_bytes,
            } => (2, 2, unspanned_shell_bytes + STUB_CELL_CONTENT_BYTES),
        };
        nodes = nodes.saturating_add(cell_nodes);
        if nodes > BODY_MAX_NODES {
            return Err(BodyError::TooManyNodes);
        }
        if 3 + cell_height > BODY_MAX_DEPTH {
            return Err(BodyError::TooDeep);
        }
        if idx == 0 {
            row_bytes = row_bytes.saturating_add(CONTENT_FIELD_OVERHEAD);
        } else {
            row_bytes = row_bytes.saturating_add(1);
        }
        row_bytes = row_bytes.saturating_add(cell_bytes);
    }
    if cell_metrics.len() > usize::from(TABLE_MAX_COLUMNS) {
        return Err(BodyError::TableTooLarge {
            path: String::new(),
        });
    }
    let table_wrapper = BLOCK_WRAPPER_BYTES + TABLE_SHELL_BYTES + CONTENT_FIELD_OVERHEAD;
    if table_wrapper.saturating_add(row_bytes) > BODY_MAX_BYTES {
        return Err(BodyError::TooLarge);
    }
    Ok(())
}

/// Computes the exact post-mend summary of a table from its mended lightweight grid and
/// kept cells' metrics, checking `width > 0` and whole-table node, depth, and byte limits.
pub(super) fn summarize_mended_table(
    mended_table: &Value,
    row_metrics: &[Vec<RowCellMetrics>],
) -> Result<Option<NodeSummary>, BodyError> {
    let rows = mended_table
        .get("content")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    if rows.is_empty() {
        return Ok(None);
    }
    let mut total_cells = 0_usize;
    // Wrapper `doc` (1 node, depth 1) + `table` (1 node, depth 2).
    let mut nodes = 2_usize;
    let mut max_row_height = 0_usize;
    let mut rows_json_bytes = 0_usize;
    let mut file_ids = Vec::new();

    for (row_val, metrics) in rows.iter().zip(row_metrics) {
        let cells = row_val
            .get("content")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice);
        nodes = nodes.saturating_add(1);
        if nodes > BODY_MAX_NODES {
            return Err(BodyError::TooManyNodes);
        }
        let mut max_cell_height = 0_usize;
        let mut cells_json_bytes = 0_usize;
        for (idx, cell_val) in cells.iter().enumerate() {
            total_cells += 1;
            let (cell_nodes, cell_height, cell_bytes) = match metrics.get(idx) {
                Some(RowCellMetrics::Kept(kept)) => {
                    let mended_shell =
                        json_size_of(cell_val).saturating_sub(STUB_CELL_CONTENT_BYTES);
                    file_ids.extend(kept.file_ids.iter().cloned());
                    (
                        kept.nodes,
                        kept.height,
                        mended_shell.saturating_add(kept.content_suffix_bytes),
                    )
                }
                Some(RowCellMetrics::Put { .. }) | None => (2, 2, json_size_of(cell_val)),
            };
            nodes = nodes.saturating_add(cell_nodes);
            if nodes > BODY_MAX_NODES {
                return Err(BodyError::TooManyNodes);
            }
            if 3 + cell_height > BODY_MAX_DEPTH {
                return Err(BodyError::TooDeep);
            }
            max_cell_height = max_cell_height.max(cell_height);
            cells_json_bytes = cells_json_bytes.saturating_add(cell_bytes);
        }
        let row_bytes = if cells.is_empty() {
            TABLE_ROW_SHELL_BYTES
        } else {
            TABLE_ROW_SHELL_BYTES
                .saturating_add(CONTENT_FIELD_OVERHEAD)
                .saturating_add(cells_json_bytes)
                .saturating_add(cells.len() - 1)
        };
        max_row_height = max_row_height.max(1 + max_cell_height);
        rows_json_bytes = rows_json_bytes.saturating_add(row_bytes);
    }

    if total_cells == 0 {
        return Ok(None);
    }
    let table_json_bytes = TABLE_SHELL_BYTES
        .saturating_add(CONTENT_FIELD_OVERHEAD)
        .saturating_add(rows_json_bytes)
        .saturating_add(rows.len() - 1);
    if BLOCK_WRAPPER_BYTES.saturating_add(table_json_bytes) > BODY_MAX_BYTES {
        return Err(BodyError::TooLarge);
    }
    Ok(Some(NodeSummary {
        kind: "table",
        nodes: nodes - 1,
        height: 1 + max_row_height,
        json_bytes: table_json_bytes,
        is_empty_paragraph: false,
        file_ids: dedup_strings(file_ids),
        // A cell holds no board view: one in a cell fails it, and the cell goes.
        board_views: 0,
    }))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn table_byte_constants_match_serde_json() {
        let len = |v: Value| serde_json::to_vec(&v).expect("json").len();
        assert_eq!(TABLE_SHELL_BYTES, len(json!({"type": "table"})));
        assert_eq!(TABLE_ROW_SHELL_BYTES, len(json!({"type": "tableRow"})));
        assert_eq!(TABLE_CELL_SHELL_BYTES, len(json!({"type": "tableCell"})));
        assert_eq!(
            TABLE_HEADER_SHELL_BYTES,
            len(json!({"type": "tableHeader"}))
        );
        let with_stub = len(json!({"type": "tableCell", "content": [{"type": "paragraph"}]}));
        assert_eq!(with_stub - TABLE_CELL_SHELL_BYTES, STUB_CELL_CONTENT_BYTES);
    }
}
