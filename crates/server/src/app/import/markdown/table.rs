//! The tables of Markdown: a table stands where a toggle may, in the document itself and
//! in what a toggle hides, and is read as its lines of text anywhere else.

use fvoci_model::{TABLE_MAX_COLUMNS, TABLE_MAX_ROWS};
use serde_json::json;

use super::super::summary::ImportWarningCode;
use super::{Converter, Frame, InlineItem, TableRowDraft};
use crate::error::AppError;

impl Converter<'_> {
    pub(super) fn finish_table_row(&mut self) {
        let Some(Frame::TableRow { is_header, cells }) = self.frames.pop() else {
            return;
        };
        if let Some(Frame::Table { rows, .. }) = self.frames.last_mut() {
            rows.push(TableRowDraft { is_header, cells });
        }
    }

    pub(super) fn finish_table_cell(&mut self) {
        let Some(Frame::TableCell(inlines)) = self.frames.pop() else {
            return;
        };
        if let Some(Frame::TableRow { cells, .. }) = self.frames.last_mut() {
            cells.push(inlines);
        }
    }

    pub(super) fn finish_table(&mut self) -> Result<(), AppError> {
        self.leave_container();
        let Some(Frame::Table {
            has_alignment,
            rows,
        }) = self.frames.pop()
        else {
            return Ok(());
        };
        if has_alignment {
            self.note(
                ImportWarningCode::MarkdownDegraded,
                "table column alignment is not preserved",
            );
        }
        let width = rows.iter().map(|row| row.cells.len()).max().unwrap_or(0);
        let fits_grid = self.takes_document_blocks()
            && width > 0
            && width <= usize::from(TABLE_MAX_COLUMNS)
            && !rows.is_empty()
            && rows.len() <= usize::from(TABLE_MAX_ROWS);

        if !fits_grid {
            self.note(
                ImportWarningCode::MarkdownDegraded,
                "table nested in a block or exceeding 200x20 limits converted to paragraphs",
            );
            for row in rows {
                let mut combined = Vec::new();
                for (index, cell) in row.cells.into_iter().enumerate() {
                    if index > 0 {
                        self.bump_text_bytes(3)?;
                        self.bump_raw_inline()?;
                        combined.push(InlineItem::Node(json!({"type": "text", "text": " | "})));
                    }
                    combined.extend(cell);
                }
                for block in self.split_inlines_to_blocks(combined)? {
                    self.push_block(block);
                }
            }
            return Ok(());
        }

        self.bump_nodes(1 + rows.len())?;
        let mut table_rows = Vec::with_capacity(rows.len());
        for row in rows {
            let cell_type = if row.is_header {
                "tableHeader"
            } else {
                "tableCell"
            };
            let mut row_cells = Vec::with_capacity(width);
            let mut iter = row.cells.into_iter();
            for _ in 0..width {
                let inlines = iter.next().unwrap_or_default();
                let mut cell_blocks = self.split_inlines_to_blocks(inlines)?;
                if cell_blocks.is_empty() {
                    self.bump_nodes(1)?;
                    cell_blocks.push(json!({"type": "paragraph"}));
                }
                self.bump_nodes(1)?;
                row_cells.push(json!({"type": cell_type, "content": cell_blocks}));
            }
            table_rows.push(json!({"type": "tableRow", "content": row_cells}));
        }
        self.push_block(json!({"type": "table", "content": table_rows}));
        Ok(())
    }
}
