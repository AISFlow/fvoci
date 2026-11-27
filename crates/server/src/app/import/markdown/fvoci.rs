//! The Markdown fvoci writes a body in and reads back: formulas (`$…$`, `$$…$$`),
//! references (`[[id]]`) and mentions (`@[name](fvoci-user://id)`), besides what the
//! importers read.

use pulldown_cmark::Options;
use serde_json::{Value, json};

use super::{AssetResolver, BodyContext, Converter, Frame, InlineItem, MarkdownBlocks, convert};
use crate::error::AppError;

/// Converts Markdown as fvoci writes a body in it ([`crate::app::body_markdown`]):
/// as [`markdown_to_blocks`] does, and besides `$…$` is an inline formula, `$$…$$` a
/// formula of its own, and `[[id]]` a reference to a card or a page.
///
/// # Errors
///
/// As [`markdown_to_blocks`].
pub(crate) fn fvoci_markdown_to_blocks(
    markdown: &str,
    max_depth: usize,
    resolver: &mut dyn AssetResolver,
) -> Result<MarkdownBlocks, AppError> {
    let dialect = Options::ENABLE_MATH | Options::ENABLE_WIKILINKS;
    convert(
        markdown,
        BodyContext::Document,
        max_depth,
        resolver,
        dialect,
    )
}

impl Converter<'_> {
    /// A formula: in a block of its own for `blockMath`, read where a text would be
    /// otherwise; one that says nothing is left out.
    pub(super) fn push_math(&mut self, kind: &str, latex: &str) -> Result<(), AppError> {
        // A formula of its own block may stand on lines between its marks.
        let latex = if kind == "blockMath" {
            latex.trim_matches('\n')
        } else {
            latex
        };
        if latex.trim().is_empty() {
            return Ok(());
        }
        self.bump_text_bytes(latex.len())?;
        self.bump_raw_inline()?;
        self.bump_nodes(1)?;
        let node = json!({"type": kind, "attrs": {"latex": latex}});
        self.push_inline(if kind == "blockMath" {
            InlineItem::BlockBreak(node)
        } else {
            InlineItem::Node(node)
        })
    }

    /// Takes the `@` before a mention (`@[name](fvoci-user://id)`) off the text it ends.
    pub(super) fn drop_mention_sign(&mut self) {
        let Some(
            Frame::Paragraph(inlines)
            | Frame::Heading { inlines, .. }
            | Frame::TableCell(inlines)
            | Frame::Link { inlines, .. }
            | Frame::ListItem {
                pending_inlines: inlines,
                ..
            },
        ) = self.frames.last_mut()
        else {
            return;
        };
        let Some(InlineItem::Node(node)) = inlines.last_mut() else {
            return;
        };
        let Some(text) = node.get("text").and_then(Value::as_str) else {
            return;
        };
        let Some(kept) = text.strip_suffix('@').map(str::to_owned) else {
            return;
        };
        if kept.is_empty() {
            inlines.pop();
        } else {
            node["text"] = Value::String(kept);
        }
    }
}
