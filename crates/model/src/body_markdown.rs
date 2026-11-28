//! What a body written in Markdown does not hold as it is: the blocks a whole body
//! written back from that Markdown would change.

use serde::{Deserialize, Serialize};

/// A block of a body that its Markdown does not hold as it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ComplexBlock {
    /// Where the block is in the body, as a JSON pointer into it (`/content/2` is the
    /// third block of the body).
    pub path: String,
    /// Why the Markdown does not hold it.
    pub reason: ComplexBlockReason,
}

/// Why the Markdown of a body does not hold one of its blocks as it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ComplexBlockReason {
    /// A cell of a table spans more than one column or row.
    MergedCell,
    /// A cell of a table holds more than one paragraph, a block that is no paragraph,
    /// or a line break.
    CellWithBlocks,
    /// A table whose first row is not all of header cells, or with a header cell
    /// below it, or whose rows have not as many cells.
    UnevenTable,
    /// A formula whose `LaTeX` has a `$` of its own, begins or ends with a space, or
    /// (one of its own block) has a line that Markdown would read as a block.
    UnwrittenFormula,
    /// A heading holds a line break: a heading of Markdown is one line, and is written
    /// without its breaks.
    HeadingWithBreak,
    /// The summary of a toggle has code, a formula or the target of a link with a `<`,
    /// a `>` or an `&` (or a target with a space): the summary is in a block of HTML,
    /// where those are written as character references, which code and a formula do not
    /// read back.
    MarkupInSummary,
    /// A board view: Markdown has none, and it is written as a link to its view, named
    /// by its board and its own title where the reader sees them.
    BoardView,
}
