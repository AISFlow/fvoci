//! Where the cells of a table of a body fall: the rows laid out one under another, each
//! cell in the first column of its row that no cell above still covers, as the web app's
//! editor lays a table out (`prosemirror-tables`).
//!
//! A stored table fills its grid: no cell spans past the last row or over another cell,
//! and every row is as wide as the widest. The allowlist refuses a table that does not,
//! and the repair of a document edited together mends one (`crates/collab`) with what
//! [`TableGrid`] says: the spans that fit, and how many cells each row lacks.

use serde_json::Value;
use thiserror::Error;

/// The most rows a table of a body has.
pub const TABLE_MAX_ROWS: u16 = 200;

/// The most columns a table of a body has, as its cells' spans add up across a row.
pub const TABLE_MAX_COLUMNS: u16 = 20;

/// A table past [`TABLE_MAX_ROWS`] or [`TABLE_MAX_COLUMNS`]: a limit, which no deletion
/// of what fails is meant to bring a table under.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("a table has more than {TABLE_MAX_ROWS} rows or more than {TABLE_MAX_COLUMNS} columns")]
pub struct TableTooLarge;

/// One span of a cell, across columns or down rows: how far it reaches where it fits,
/// and whether the cell says so already.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reach {
    /// How many columns or rows the cell covers where it fits, at least one.
    pub span: u16,
    /// Whether the cell's attribute gives another value, or one that is not allowed:
    /// the attribute has to be written as [`Self::span`]. A cell without the attribute
    /// spans one.
    pub changed: bool,
}

/// Where a cell falls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellFit {
    /// Across columns (`colspan`).
    pub columns: Reach,
    /// Down rows (`rowspan`).
    pub rows: Reach,
}

/// Where the cells of one row fall.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowFit {
    /// Its cells, in order.
    pub cells: Vec<CellFit>,
    /// How many cells of one column it lacks to be as wide as the table: as many as
    /// the columns of the row no cell covers, which all come after its own last cell.
    pub missing: usize,
}

/// The grid of a table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableGrid {
    width: usize,
    rows: Vec<RowFit>,
}

impl TableGrid {
    /// Lays out a table: the JSON of a node whose content is its rows, whose content in
    /// turn is their cells. What a child is, is not asked; a span that is no whole
    /// number from one to its limit is taken as one.
    ///
    /// A cell that would cover a column already covered spans as far as the first such
    /// column; a cell that would reach past the last row reaches to it.
    ///
    /// # Errors
    ///
    /// Returns [`TableTooLarge`] for a table of more than [`TABLE_MAX_ROWS`] rows, or one
    /// a row of which reaches past [`TABLE_MAX_COLUMNS`] columns.
    pub fn of(table: &Value) -> Result<Self, TableTooLarge> {
        let rows = content_of(table);
        if rows.len() > usize::from(TABLE_MAX_ROWS) {
            return Err(TableTooLarge);
        }
        let mut covered = Covered::new(rows.len());
        let fits = rows
            .iter()
            .enumerate()
            .map(|(row, node)| lay_out_row(&mut covered, row, content_of(node)))
            .collect::<Result<Vec<_>, _>>()?;
        let width = covered.width();
        let rows = fits
            .into_iter()
            .enumerate()
            .map(|(row, cells)| RowFit {
                cells,
                missing: width - covered.count(row),
            })
            .collect();
        Ok(Self { width, rows })
    }

    /// How many columns the table has: as many as its widest row covers.
    #[must_use]
    pub const fn width(&self) -> usize {
        self.width
    }

    /// Where the cells of each row fall, the rows in order.
    #[must_use]
    pub fn rows(&self) -> &[RowFit] {
        &self.rows
    }

    /// Whether the table fills its grid as it is: it has a cell, every span fits as the
    /// cell states it, and no row lacks a cell.
    #[must_use]
    pub fn is_filled(&self) -> bool {
        self.width > 0
            && self.rows.iter().all(|row| {
                row.missing == 0
                    && row
                        .cells
                        .iter()
                        .all(|cell| !cell.columns.changed && !cell.rows.changed)
            })
    }
}

/// The children of a node; none if it has none.
fn content_of(node: &Value) -> &[Value] {
    node.get("content")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

/// A span a cell states: the attribute `name` if it is a whole number from one to
/// `limit` (however it is written, as the allowlist reads it), one if it has none, and
/// `None` if it has one that is not allowed.
fn stated_span(cell: &Value, name: &str, limit: u16) -> Option<u16> {
    let Some(value) = cell.get("attrs").and_then(|attrs| attrs.get(name)) else {
        return Some(1);
    };
    let number = value.as_f64()?;
    // The whole number it is, found among the few allowed rather than cast to: the
    // first that is not below it, if that one is not above it either.
    let whole = (1..=limit).find(|whole| f64::from(*whole) >= number)?;
    (f64::from(whole) <= number).then_some(whole)
}

/// Lays out the cells of one row, marking what each covers in this row and below.
fn lay_out_row(
    covered: &mut Covered,
    row: usize,
    cells: &[Value],
) -> Result<Vec<CellFit>, TableTooLarge> {
    let mut column = 0;
    let mut fits = Vec::with_capacity(cells.len());
    for cell in cells {
        while covered.is(row, column) {
            column += 1;
        }
        let stated_columns = stated_span(cell, "colspan", TABLE_MAX_COLUMNS);
        let stated_rows = stated_span(cell, "rowspan", TABLE_MAX_ROWS);
        let wanted_columns = usize::from(stated_columns.unwrap_or(1));
        // The first column is free, the cell being put there.
        let columns = (column..column + wanted_columns)
            .take_while(|at| !covered.is(row, *at))
            .count();
        // No cell above covers these columns in this row, so none covers them below:
        // a cell covers the rows from its own down without a gap.
        let wanted_rows = usize::from(stated_rows.unwrap_or(1));
        let rows = wanted_rows.min(covered.height() - row);
        if column + columns > usize::from(TABLE_MAX_COLUMNS) {
            return Err(TableTooLarge);
        }
        for below in row..row + rows {
            covered.cover(below, column..column + columns);
        }
        fits.push(CellFit {
            columns: reach(columns, stated_columns),
            rows: reach(rows, stated_rows),
        });
        column += columns;
    }
    Ok(fits)
}

/// The reach of a span that fits as `span`, which the cell stated as `stated`.
fn reach(span: usize, stated: Option<u16>) -> Reach {
    // A span is at most its limit, which is a `u16`.
    let span = u16::try_from(span).unwrap_or(u16::MAX);
    Reach {
        span,
        changed: stated != Some(span),
    }
}

/// The columns of each row that a cell covers, from the left; a row's columns past
/// the last covered one are not kept.
#[derive(Debug)]
struct Covered(Vec<Vec<bool>>);

impl Covered {
    fn new(height: usize) -> Self {
        Self(vec![Vec::new(); height])
    }

    fn height(&self) -> usize {
        self.0.len()
    }

    fn is(&self, row: usize, column: usize) -> bool {
        self.0
            .get(row)
            .and_then(|columns| columns.get(column))
            .copied()
            .unwrap_or(false)
    }

    fn cover(&mut self, row: usize, columns: std::ops::Range<usize>) {
        let Some(line) = self.0.get_mut(row) else {
            return;
        };
        if line.len() < columns.end {
            line.resize(columns.end, false);
        }
        line[columns].fill(true);
    }

    /// How many columns of a row are covered.
    fn count(&self, row: usize) -> usize {
        self.0
            .get(row)
            .map_or(0, |columns| columns.iter().filter(|at| **at).count())
    }

    /// How far the widest row reaches.
    fn width(&self) -> usize {
        self.0.iter().map(Vec::len).max().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    fn cell(colspan: u16, rowspan: u16) -> Value {
        json!({"type": "tableCell", "attrs": {"colspan": colspan, "rowspan": rowspan},
            "content": [{"type": "paragraph"}]})
    }

    fn table(rows: &[&[Value]]) -> Value {
        let rows: Vec<Value> = rows
            .iter()
            .map(|cells| json!({"type": "tableRow", "content": cells}))
            .collect();
        json!({"type": "table", "content": rows})
    }

    fn spans(grid: &TableGrid) -> Vec<Vec<(u16, u16)>> {
        grid.rows()
            .iter()
            .map(|row| {
                row.cells
                    .iter()
                    .map(|cell| (cell.columns.span, cell.rows.span))
                    .collect()
            })
            .collect()
    }

    fn missing(grid: &TableGrid) -> Vec<usize> {
        grid.rows().iter().map(|row| row.missing).collect()
    }

    #[test]
    fn a_table_of_spans_that_meet_fills_its_grid() {
        // A B B
        // A C D
        let grid = TableGrid::of(&table(&[
            &[cell(1, 2), cell(2, 1)],
            &[cell(1, 1), cell(1, 1)],
        ]))
        .expect("within the limits");
        assert_eq!(grid.width(), 3);
        assert!(grid.is_filled(), "{grid:?}");
        assert_eq!(missing(&grid), [0, 0]);
    }

    #[test]
    fn a_row_covered_by_the_cells_above_needs_none_of_its_own() {
        let grid = TableGrid::of(&table(&[&[cell(1, 2), cell(1, 2)], &[]])).expect("within");
        assert!(grid.is_filled(), "{grid:?}");
    }

    #[test]
    fn a_short_row_lacks_cells_and_a_long_one_makes_the_others_short() {
        let grid = TableGrid::of(&table(&[
            &[cell(1, 1), cell(1, 1)],
            &[cell(1, 1)],
            &[cell(1, 1), cell(1, 1), cell(1, 1)],
        ]))
        .expect("within");
        assert_eq!(grid.width(), 3);
        assert_eq!(missing(&grid), [1, 2, 0]);
        assert!(!grid.is_filled());
    }

    #[test]
    fn a_span_past_the_last_row_reaches_to_it() {
        let grid =
            TableGrid::of(&table(&[&[cell(1, 1), cell(1, 3)], &[cell(1, 1)]])).expect("within");
        assert_eq!(spans(&grid), [vec![(1, 1), (1, 2)], vec![(1, 1)]]);
        assert!(grid.rows()[0].cells[1].rows.changed, "written again");
        assert_eq!(missing(&grid), [0, 0]);
    }

    #[test]
    fn a_cell_that_would_cover_a_covered_column_stops_before_it() {
        // Row 1's first cell would span over the column row 0's second cell covers.
        let grid = TableGrid::of(&table(&[
            &[cell(1, 1), cell(1, 2), cell(1, 1)],
            &[cell(3, 1)],
        ]))
        .expect("within");
        assert_eq!(spans(&grid), [vec![(1, 1), (1, 2), (1, 1)], vec![(1, 1)]]);
        assert!(grid.rows()[1].cells[0].columns.changed);
        // Its third column is left to fill.
        assert_eq!(missing(&grid), [0, 1]);
    }

    #[test]
    fn spans_not_allowed_are_taken_as_one_and_written_again() {
        let odd = json!({"type": "tableCell", "attrs": {"colspan": "2", "rowspan": 0},
            "content": [{"type": "paragraph"}]});
        let grid = TableGrid::of(&table(&[&[odd]])).expect("within");
        assert_eq!(spans(&grid), [vec![(1, 1)]]);
        let only = grid.rows()[0].cells[0];
        assert!(only.columns.changed && only.rows.changed, "{only:?}");
        let written = json!({"type": "tableCell", "attrs": {"colspan": 2.0},
            "content": [{"type": "paragraph"}]});
        let grid = TableGrid::of(&table(&[&[written], &[cell(1, 1), cell(1, 1)]])).expect("within");
        assert!(!grid.rows()[0].cells[0].columns.changed, "2.0 is 2");
        assert!(grid.is_filled());
    }

    #[test]
    fn a_table_without_cells_does_not_fill_a_grid() {
        let grid = TableGrid::of(&table(&[&[], &[]])).expect("within");
        assert_eq!(grid.width(), 0);
        assert!(!grid.is_filled());
    }

    #[test]
    fn the_limits_are_two_hundred_rows_and_twenty_columns() {
        let row = [cell(1, 1)];
        let rows = vec![&row[..]; usize::from(TABLE_MAX_ROWS)];
        assert!(TableGrid::of(&table(&rows)).is_ok());
        let rows = vec![&row[..]; usize::from(TABLE_MAX_ROWS) + 1];
        assert_eq!(TableGrid::of(&table(&rows)), Err(TableTooLarge));

        let wide = vec![cell(1, 1); usize::from(TABLE_MAX_COLUMNS)];
        assert!(TableGrid::of(&table(&[&wide])).is_ok());
        let wider = vec![cell(1, 1); usize::from(TABLE_MAX_COLUMNS) + 1];
        assert_eq!(TableGrid::of(&table(&[&wider])), Err(TableTooLarge));
        // Spans count: ten cells of two columns, and one more.
        let mut spanned = vec![cell(2, 1); 10];
        assert!(TableGrid::of(&table(&[&spanned])).is_ok());
        spanned.push(cell(1, 1));
        assert_eq!(TableGrid::of(&table(&[&spanned])), Err(TableTooLarge));
    }
}
