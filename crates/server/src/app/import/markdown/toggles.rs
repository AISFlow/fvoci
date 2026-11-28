//! Toggles and callouts in Markdown, as fvoci writes them ([`crate::app::body_markdown`])
//! and GitHub draws them. A toggle is `<details>`, its summary's line in
//! `<summary>…</summary>`, the Markdown of what it hides, then `</details>`: the summary
//! is read as Markdown of a line, and what stands between the tags in the same block of
//! HTML as text. A callout is a GitHub alert, a quote whose first line is `[!NOTE]`,
//! `[!TIP]`, `[!IMPORTANT]`, `[!WARNING]` or `[!CAUTION]` alone; an important one is a
//! note, the tones having no other place for it.
//!
//! Both stand only in the document itself and in what a toggle hides. Anywhere else a
//! toggle is read as its summary's line and then the blocks it hides, and an alert as a
//! quote. A toggle left open is closed where what holds it ends.

use pulldown_cmark::{BlockQuoteKind, Event, Parser, Tag, TagEnd};
use serde_json::{Value, json};

use super::super::summary::ImportWarningCode;
use super::{Converter, Frame, merge_text_nodes, partition_inlines};
use crate::error::AppError;

/// The tone of the callout of each kind of alert.
const fn tone_of(kind: BlockQuoteKind) -> &'static str {
    match kind {
        BlockQuoteKind::Note | BlockQuoteKind::Important => "info",
        BlockQuoteKind::Tip => "tip",
        BlockQuoteKind::Warning => "warn",
        BlockQuoteKind::Caution => "danger",
    }
}

/// What a block of HTML says of toggles, piece by piece.
#[derive(Debug, PartialEq, Eq)]
enum Piece {
    /// `<details …>`.
    Open,
    /// `<summary …>`, what follows it to `</summary>` (or to the end), and that.
    Summary { line: (usize, usize) },
    /// `</details>`.
    Close,
    /// Anything else.
    Other,
}

/// The pieces of a block of HTML, each with where it starts and ends in it. `lower` is
/// the HTML in small letters, each byte where it is in the HTML.
struct Pieces<'h> {
    lower: &'h str,
    at: usize,
}

impl Pieces<'_> {
    /// Where a tag named `name` (with its `<`) at `at` ends, after its `>`: `None` where
    /// none starts there. One whose `>` never comes ends the block.
    fn tag(&self, at: usize, name: &str) -> Option<usize> {
        let rest = self.lower[at..].strip_prefix(name)?;
        if !rest.starts_with(|letter: char| {
            letter == '>' || letter == '/' || letter.is_ascii_whitespace()
        }) {
            return None;
        }
        let after = at + name.len();
        Some(
            self.lower[after..]
                .find('>')
                .map_or(self.lower.len(), |end| after + end + 1),
        )
    }

    /// Whether a tag of a toggle starts at `at`.
    fn starts_tag(&self, at: usize) -> bool {
        ["<details", "</details", "<summary"]
            .iter()
            .any(|name| self.tag(at, name).is_some())
    }
}

impl Iterator for Pieces<'_> {
    type Item = (Piece, usize, usize);

    fn next(&mut self) -> Option<Self::Item> {
        let start = self.lower.len() - self.lower[self.at..].trim_start().len();
        if start == self.lower.len() {
            return None;
        }
        let (piece, end) = if let Some(end) = self.tag(start, "<details") {
            (Piece::Open, end)
        } else if let Some(end) = self.tag(start, "</details") {
            (Piece::Close, end)
        } else if let Some(open) = self.tag(start, "<summary") {
            // A `</summary>` after a backslash of its own is text of the summary.
            let close = self.lower[open..]
                .match_indices("</summary")
                .map(|(at, _)| open + at)
                .find(|&at| !escaped(self.lower, at));
            let end = close.map_or(self.lower.len(), |close| {
                self.tag(close, "</summary").unwrap_or(self.lower.len())
            });
            let line = (open, close.unwrap_or(self.lower.len()));
            (Piece::Summary { line }, end)
        } else {
            let first = self.lower[start..].chars().next().map_or(1, char::len_utf8);
            let end = self.lower[start + first..]
                .match_indices('<')
                .map(|(at, _)| start + first + at)
                .find(|&at| self.starts_tag(at))
                .unwrap_or(self.lower.len());
            (Piece::Other, end)
        };
        self.at = end;
        Some((piece, start, end))
    }
}

/// Whether the letter at `at` follows a backslash of its own: an odd run of them.
fn escaped(text: &str, at: usize) -> bool {
    let backslashes = text[..at]
        .bytes()
        .rev()
        .take_while(|&letter| letter == b'\\');
    backslashes.count() % 2 == 1
}

/// Whether a tag is one of inline text, read in the summary of a toggle.
const fn is_inline(tag: &Tag<'_>) -> bool {
    matches!(
        tag,
        Tag::Emphasis | Tag::Strong | Tag::Strikethrough | Tag::Link { .. } | Tag::Image { .. }
    )
}

const fn ends_inline(end: TagEnd) -> bool {
    matches!(
        end,
        TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Link | TagEnd::Image
    )
}

impl Converter<'_> {
    /// Whether a table, a toggle or a callout may stand here: in the document itself, or
    /// in what a toggle hides.
    pub(super) fn takes_document_blocks(&self) -> bool {
        matches!(
            self.frames.last(),
            Some(Frame::Root(_) | Frame::Details { kept: true, .. })
        )
    }

    /// A quote, or the callout of an alert where one may stand.
    pub(super) fn start_quote(&mut self, kind: Option<BlockQuoteKind>) -> Result<(), AppError> {
        self.enter_container()?;
        self.flush_list_item_inlines()?;
        let frame = match kind {
            Some(kind) if self.takes_document_blocks() => Frame::Callout {
                tone: tone_of(kind),
                blocks: Vec::new(),
            },
            Some(_) => {
                self.note(
                    ImportWarningCode::MarkdownDegraded,
                    "callout in a list, a quote or a callout read as a quote",
                );
                Frame::BlockQuote(Vec::new())
            }
            None => Frame::BlockQuote(Vec::new()),
        };
        self.push_frame(frame)
    }

    /// The end of a quote or of a callout, and of the toggles left open in it.
    pub(super) fn finish_quote(&mut self) -> Result<(), AppError> {
        self.close_toggles()?;
        if !matches!(self.frames.last(), Some(Frame::Callout { .. })) {
            return self.finish_blockquote();
        }
        self.leave_container();
        let Some(Frame::Callout { tone, mut blocks }) = self.frames.pop() else {
            return Ok(());
        };
        if blocks.is_empty() {
            self.bump_nodes(1)?;
            blocks.push(json!({"type": "paragraph"}));
        }
        self.bump_nodes(1)?;
        self.push_block(json!({"type": "callout", "attrs": {"tone": tone}, "content": blocks}));
        Ok(())
    }

    /// A block of HTML, read when it ends: the tags of toggles open, name and close them,
    /// and the rest is text, line by line.
    pub(super) fn finish_html(&mut self) -> Result<(), AppError> {
        let Some(Frame::Html(html)) = self.frames.pop() else {
            return Ok(());
        };
        let lower = html.to_ascii_lowercase();
        for (piece, start, end) in (Pieces {
            lower: &lower,
            at: 0,
        }) {
            match piece {
                Piece::Open => self.open_toggle()?,
                Piece::Close if matches!(self.frames.last(), Some(Frame::Details { .. })) => {
                    self.finish_toggle()?;
                }
                Piece::Summary { line: (from, to) } if self.awaits_summary() => {
                    self.read_summary(&html[from..to])?;
                }
                // The end of a toggle that is not open is HTML, as on the web.
                Piece::Close | Piece::Summary { .. } | Piece::Other => {
                    self.html_as_text(&html[start..end])?;
                }
            }
        }
        Ok(())
    }

    /// HTML that is not read, as text: each line of it a text of its own, as before.
    fn html_as_text(&mut self, html: &str) -> Result<(), AppError> {
        for line in html
            .split('\n')
            .map(str::trim)
            .filter(|line| !line.is_empty())
        {
            self.note(
                ImportWarningCode::MarkdownDegraded,
                "HTML rendered as plain text",
            );
            self.push_text(line)?;
        }
        Ok(())
    }

    fn open_toggle(&mut self) -> Result<(), AppError> {
        let kept = self.takes_document_blocks();
        if !kept {
            self.note(
                ImportWarningCode::MarkdownDegraded,
                "toggle in a list, a quote or a callout read as its summary and what it hides",
            );
        }
        self.flush_list_item_inlines()?;
        // A toggle holds its blocks two levels down, as a list does.
        self.enter_container()?;
        self.enter_container()?;
        self.push_frame(Frame::Details {
            kept,
            summary: None,
            blocks: Vec::new(),
        })
    }

    /// Whether the toggle open here has neither its summary nor a block yet.
    fn awaits_summary(&self) -> bool {
        matches!(
            self.frames.last(),
            Some(Frame::Details { summary: None, blocks, .. }) if blocks.is_empty()
        )
    }

    /// The summary of the toggle open here: Markdown of a line. A block in it, such as a
    /// formula of its own, is the first of what the toggle hides.
    fn read_summary(&mut self, line: &str) -> Result<(), AppError> {
        self.push_frame(Frame::Paragraph(Vec::new()))?;
        for event in Parser::new_ext(line.trim(), self.options) {
            match event {
                Event::Start(tag) if !is_inline(&tag) => {}
                Event::End(end) if !ends_inline(end) => {}
                Event::Rule | Event::TaskListMarker(_) => {}
                event => self.handle(event)?,
            }
        }
        let Some(Frame::Paragraph(inlines)) = self.frames.pop() else {
            return Ok(());
        };
        let (nodes, extra) = partition_inlines(inlines);
        let merged = merge_text_nodes(nodes);
        self.bump_nodes(merged.len().saturating_add(extra.len()))?;
        if let Some(Frame::Details {
            summary, blocks, ..
        }) = self.frames.last_mut()
        {
            *summary = Some(merged);
            blocks.extend(extra);
        }
        Ok(())
    }

    /// Closes the toggles left open where what holds them ends.
    pub(super) fn close_toggles(&mut self) -> Result<(), AppError> {
        while matches!(self.frames.last(), Some(Frame::Details { .. })) {
            self.finish_toggle()?;
        }
        Ok(())
    }

    fn finish_toggle(&mut self) -> Result<(), AppError> {
        let Some(Frame::Details {
            kept,
            summary,
            mut blocks,
        }) = self.frames.pop()
        else {
            return Ok(());
        };
        self.leave_container();
        self.leave_container();
        let line = summary.unwrap_or_default();
        if !kept {
            if !line.is_empty() {
                self.bump_nodes(1)?;
                self.push_block(json!({"type": "paragraph", "content": line}));
            }
            for block in blocks {
                self.push_block(block);
            }
            return Ok(());
        }
        if blocks.is_empty() {
            self.bump_nodes(1)?;
            blocks.push(json!({"type": "paragraph"}));
        }
        self.bump_nodes(3)?;
        let summary = if line.is_empty() {
            json!({"type": "detailsSummary"})
        } else {
            json!({"type": "detailsSummary", "content": line})
        };
        let content: Value = json!({"type": "detailsContent", "content": blocks});
        self.push_block(json!({"type": "details", "content": [summary, content]}));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{Piece, Pieces};

    fn pieces(html: &str) -> Vec<(Piece, &str)> {
        let lower = html.to_ascii_lowercase();
        let found: Vec<(Piece, usize, usize)> = Pieces {
            lower: &lower,
            at: 0,
        }
        .collect();
        found
            .into_iter()
            .map(|(piece, start, end)| (piece, &html[start..end]))
            .collect()
    }

    #[test]
    fn a_block_of_html_is_cut_at_the_tags_of_toggles_whatever_their_case() {
        let html = "<DETAILS open>\n<Summary>A *b*</SUMMARY>\nloose <i>x</i>\n</details >\n";
        let found = pieces(html);
        assert_eq!(
            found,
            [
                (Piece::Open, "<DETAILS open>"),
                (
                    Piece::Summary { line: (24, 29) },
                    "<Summary>A *b*</SUMMARY>"
                ),
                (Piece::Other, "loose <i>x</i>\n"),
                (Piece::Close, "</details >"),
            ]
        );
        assert_eq!(&html[24..29], "A *b*", "the summary's line");
    }

    #[test]
    fn a_tag_that_only_starts_like_one_is_text_and_one_never_closed_ends_the_block() {
        assert_eq!(
            pieces("<detailsx>é<summary>s"),
            [
                (Piece::Other, "<detailsx>é"),
                (Piece::Summary { line: (21, 22) }, "<summary>s"),
            ]
        );
        assert_eq!(pieces("<details "), [(Piece::Open, "<details ")]);
    }
}
