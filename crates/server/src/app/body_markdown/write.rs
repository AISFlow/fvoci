//! A body written in Markdown as [`super::markdown_nodes`] reads it back: what it holds
//! as it is comes back the same, and each block it does not is named
//! ([`ComplexBlock`]). An empty paragraph is not written, and the marks of the spaces
//! at the edge of a bold, italic or struck text are not kept.
//!
//! A toggle is written as GitHub draws one, `<details>` with its summary's line in
//! `<summary>…</summary>`, and a callout as a GitHub alert (`> [!NOTE]`, `[!TIP]`,
//! `[!WARNING]` or `[!CAUTION]`, by its tone). Whether a toggle is open is not the
//! body's, and is not written. The summary is in a block of HTML, which a renderer of
//! Markdown passes on as it is: there each `<`, `>` and `&` of the line is written as a
//! character reference, so that no letter of a body is ever a tag, and code, a formula
//! or the target of a link that holds one is named, as its own letters cannot be kept
//! there. A board view, which Markdown has none of, is a link to its view, and named.

use std::collections::HashMap;
use std::sync::Arc;

use fvoci_model::{ComplexBlock, ComplexBlockReason};
use serde_json::Value;

use super::{FILE_SCHEME, USER_SCHEME, VIEW_SCHEME};

/// What the Markdown of a body calls what the body names by identifier; one not here is
/// called by its identifier.
#[derive(Debug, Default)]
pub(crate) struct Names {
    /// The names of files, by their identifiers.
    pub(crate) files: HashMap<String, String>,
    /// The names people are shown by, by their identifiers.
    pub(crate) users: HashMap<String, String>,
    /// The names of the views board views show (their board's title and theirs), by
    /// their identifiers.
    pub(crate) views: HashMap<String, String>,
    /// Set where the body is written into an export of Markdown files
    /// (`export_markdown`): its files and the pages, cards and units it holds are
    /// linked where the export puts them, a person is named in plain text, and what
    /// else a reference names is called by its title where the reader reads it.
    pub(crate) export: Option<Exported>,
}

/// The path from a file in the folder `from` to the file `to`, both from the root of an
/// export: out of the folders `to` is not in, then down into those it is.
pub(crate) fn relative(from: &[String], to: &[String]) -> String {
    let (folders, _) = to
        .split_last()
        .map_or((to, None), |(name, folders)| (folders, Some(name)));
    let common = from
        .iter()
        .zip(folders)
        .take_while(|(one, other)| one == other)
        .count();
    let mut path = "../".repeat(from.len() - common);
    path.push_str(&to[common..].join("/"));
    path
}

/// Where an export of Markdown files puts what a body names, and where the body is.
#[derive(Debug, Clone)]
pub(crate) struct Exported {
    /// The folders of the file the body is written to, from the root of the export.
    pub(crate) from: Vec<String>,
    /// What the export holds, and the titles of what else the reader reads.
    pub(crate) places: Arc<ExportPlaces>,
}

/// What an export of Markdown files holds, by identifier: where each is, from the root
/// of the export.
#[derive(Debug, Default)]
pub(crate) struct ExportPlaces {
    /// The files, by their stored names (a body's `fileId`).
    pub(crate) files: HashMap<String, Vec<String>>,
    /// The pages, cards and units, and their titles.
    pub(crate) blocks: HashMap<String, (Vec<String>, String)>,
    /// The titles of what a reference names that the reader reads and the export does
    /// not hold.
    pub(crate) titles: HashMap<String, String>,
}

impl Exported {
    /// The link from the body's file to `to`, in `<…>` so that a space or a parenthesis
    /// of a name ends no link.
    fn link(&self, to: &[String]) -> String {
        format!("<{}>", relative(&self.from, to))
    }

    /// The link to `to` in a toggle's summary, a block of HTML: without `<…>`, its
    /// spaces written `%20` and the letters HTML reads as markup as references.
    fn link_in_summary(&self, to: &[String]) -> String {
        as_references(&destination(&relative(&self.from, to).replace(' ', "%20")))
    }

    /// The link to a file the export holds.
    fn file(&self, id: &str) -> Option<String> {
        self.places.files.get(id).map(|to| self.link(to))
    }
}

/// A body in Markdown.
#[derive(Debug)]
pub(crate) struct Written {
    pub(crate) markdown: String,
    /// The blocks the Markdown does not hold as they are, in the order of the body.
    pub(crate) complex: Vec<ComplexBlock>,
}

/// Writes the document `body` in Markdown.
pub(crate) fn body_to_markdown(body: &Value, names: &Names) -> Written {
    let mut writer = Writer {
        names,
        complex: Vec::new(),
    };
    let markdown = writer.blocks(children(body), "");
    Written {
        markdown,
        complex: writer.complex,
    }
}

fn children(node: &Value) -> &[Value] {
    node.get("content")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn kind(node: &Value) -> &str {
    node.get("type").and_then(Value::as_str).unwrap_or_default()
}

fn attr<'v>(node: &'v Value, name: &str) -> Option<&'v Value> {
    node.get("attrs").and_then(|attrs| attrs.get(name))
}

fn attr_text<'v>(node: &'v Value, name: &str) -> &'v str {
    attr(node, name).and_then(Value::as_str).unwrap_or_default()
}

/// What a run of inline nodes is written in, which marks it escapes besides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Within {
    Paragraph,
    /// A heading: a `#` anywhere could close it.
    Heading,
    /// A cell: a `|` in code ends it too.
    Cell,
    /// The summary of a toggle, in a block of HTML: [`MARKUP`] is written as character
    /// references.
    Summary,
}

/// What HTML reads as markup in the summary of a toggle.
const MARKUP: [char; 3] = ['<', '>', '&'];

/// The character reference a letter of [`MARKUP`] is written as.
fn reference_of(letter: char) -> Option<&'static str> {
    match letter {
        '<' => Some("&lt;"),
        '>' => Some("&gt;"),
        '&' => Some("&amp;"),
        _ => None,
    }
}

/// `text` with each letter of [`MARKUP`] written as its character reference.
fn as_references(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for letter in text.chars() {
        match reference_of(letter) {
            Some(reference) => out.push_str(reference),
            None => out.push(letter),
        }
    }
    out
}

struct Writer<'n> {
    names: &'n Names,
    complex: Vec<ComplexBlock>,
}

impl Writer<'_> {
    fn complex(&mut self, path: &str, reason: ComplexBlockReason) {
        self.complex.push(ComplexBlock {
            path: path.to_owned(),
            reason,
        });
    }

    /// Blocks after one another, a blank line between. Two lists one after the other
    /// are told apart by their markers, which Markdown would read as one otherwise.
    fn blocks(&mut self, nodes: &[Value], path: &str) -> String {
        let mut out = String::new();
        let mut last_marker = None;
        for (index, node) in nodes.iter().enumerate() {
            let at = format!("{path}/content/{index}");
            let marker = match kind(node) {
                "bulletList" | "taskList" => Some(if last_marker == Some('-') { '*' } else { '-' }),
                "orderedList" => Some(if last_marker == Some('.') { ')' } else { '.' }),
                _ => None,
            };
            let written = self.block(node, &at, marker.unwrap_or('-'));
            if written.is_empty() {
                continue;
            }
            last_marker = marker;
            if !out.is_empty() {
                out.push_str("\n\n");
            }
            out.push_str(&written);
        }
        out
    }

    fn block(&mut self, node: &Value, path: &str, marker: char) -> String {
        match kind(node) {
            "paragraph" => self.inlines(children(node), path, Within::Paragraph),
            "heading" => {
                let level = attr(node, "level")
                    .and_then(Value::as_u64)
                    .unwrap_or(1)
                    .clamp(1, 3);
                let hashes = "#".repeat(usize::try_from(level).unwrap_or(1));
                let inlines = children(node);
                let text = if inlines.iter().any(|inline| kind(inline) == "hardBreak") {
                    self.complex(path, ComplexBlockReason::HeadingWithBreak);
                    let one_line: Vec<Value> = inlines
                        .iter()
                        .map(|inline| match kind(inline) {
                            "hardBreak" => serde_json::json!({"type": "text", "text": " "}),
                            _ => inline.clone(),
                        })
                        .collect();
                    self.inlines(&one_line, path, Within::Heading)
                } else {
                    self.inlines(inlines, path, Within::Heading)
                };
                if text.is_empty() {
                    hashes
                } else {
                    format!("{hashes} {text}")
                }
            }
            "blockquote" => {
                let inner = self.blocks(children(node), path);
                prefixed(&inner, "> ", "> ")
            }
            "bulletList" | "taskList" | "orderedList" => self.list(node, path, marker),
            "codeBlock" => code_block(node),
            "horizontalRule" => "---".to_owned(),
            "image" => {
                let id = attr_text(node, "fileId");
                format!(
                    "![{}]({})",
                    escape(attr_text(node, "alt"), Line::Inside, Within::Paragraph),
                    self.file_link(id)
                )
            }
            "file" => {
                let id = attr_text(node, "fileId");
                let name = self.names.files.get(id).map_or(id, String::as_str);
                format!(
                    "[{}]({})",
                    escape(name, Line::Inside, Within::Paragraph),
                    self.file_link(id)
                )
            }
            "blockMath" => {
                let latex = attr_text(node, "latex");
                if !block_formula_is_written(latex) {
                    self.complex(path, ComplexBlockReason::UnwrittenFormula);
                }
                format!("$${latex}$$")
            }
            "table" => self.table(node, path),
            "details" => self.details(node, path),
            "boardView" => {
                self.complex(path, ComplexBlockReason::BoardView);
                let id = attr_text(node, "viewId");
                let name = self
                    .names
                    .views
                    .get(id)
                    .map_or("Board view", String::as_str);
                if self.names.export.is_some() {
                    // Out of the app a view is named alone: no link of the app leads
                    // anywhere there.
                    return escape(name, Line::Whole, Within::Paragraph);
                }
                format!(
                    "[{}]({VIEW_SCHEME}{id})",
                    escape(name, Line::Inside, Within::Paragraph)
                )
            }
            "callout" => {
                let marker = match attr_text(node, "tone") {
                    "tip" => "TIP",
                    "warn" => "WARNING",
                    "danger" => "CAUTION",
                    _ => "NOTE",
                };
                let inner = self.blocks(children(node), path);
                if inner.is_empty() {
                    format!("> [!{marker}]")
                } else {
                    format!("> [!{marker}]\n{}", prefixed(&inner, "> ", "> "))
                }
            }
            _ => String::new(),
        }
    }

    /// A toggle: its summary in a block of HTML of its own, and what it hides between
    /// blank lines, so that Markdown reads it as Markdown.
    fn details(&mut self, node: &Value, path: &str) -> String {
        let parts = children(node);
        let part = |index: usize| parts.get(index).map_or(&[][..], children);
        let line = self.inlines(part(0), &format!("{path}/content/0"), Within::Summary);
        let inner = self.blocks(part(1), &format!("{path}/content/1"));
        if inner.is_empty() {
            format!("<details>\n<summary>{line}</summary>\n\n</details>")
        } else {
            format!("<details>\n<summary>{line}</summary>\n\n{inner}\n\n</details>")
        }
    }

    fn list(&mut self, node: &Value, path: &str, marker: char) -> String {
        let start = attr(node, "start").and_then(Value::as_u64).unwrap_or(1);
        let mut items = Vec::new();
        for (index, item) in children(node).iter().enumerate() {
            let at = format!("{path}/content/{index}");
            let inner = self.blocks(children(item), &at);
            let (first, rest) = match kind(node) {
                "orderedList" => {
                    let number = start.saturating_add(u64::try_from(index).unwrap_or(0));
                    let first = format!("{number}{marker} ");
                    let rest = " ".repeat(first.len());
                    (first, rest)
                }
                "taskList" => {
                    let checked = attr(item, "checked").and_then(Value::as_bool) == Some(true);
                    let check = if checked { 'x' } else { ' ' };
                    (format!("{marker} [{check}] "), "  ".to_owned())
                }
                _ => (format!("{marker} "), "  ".to_owned()),
            };
            items.push(prefixed(&inner, &first, &rest));
        }
        items.join("\n")
    }

    fn table(&mut self, node: &Value, path: &str) -> String {
        let rows = children(node);
        let width = rows.first().map_or(0, |row| children(row).len());
        let mut lines = Vec::with_capacity(rows.len() + 1);
        for (row_index, row) in rows.iter().enumerate() {
            let row_path = format!("{path}/content/{row_index}");
            let cells = children(row);
            let header = row_index == 0;
            if cells.len() != width
                || cells
                    .iter()
                    .any(|cell| (kind(cell) == "tableHeader") != header)
            {
                self.complex(&row_path, ComplexBlockReason::UnevenTable);
            }
            let written: Vec<String> = cells
                .iter()
                .enumerate()
                .map(|(index, cell)| self.cell(cell, &format!("{row_path}/content/{index}")))
                .collect();
            lines.push(format!("| {} |", written.join(" | ")));
            if header {
                lines.push(format!("|{}", " --- |".repeat(width.max(1))));
            }
        }
        lines.join("\n")
    }

    fn cell(&mut self, cell: &Value, path: &str) -> String {
        let spans = ["colspan", "rowspan"]
            .iter()
            .any(|name| attr(cell, name).and_then(Value::as_u64).unwrap_or(1) > 1);
        if spans {
            self.complex(path, ComplexBlockReason::MergedCell);
        }
        let blocks = children(cell);
        let one_line = match blocks {
            [] => true,
            [only] => {
                kind(only) == "paragraph"
                    && children(only)
                        .iter()
                        .all(|inline| kind(inline) != "hardBreak")
            }
            _ => false,
        };
        if !one_line {
            self.complex(path, ComplexBlockReason::CellWithBlocks);
        }
        let written: Vec<String> = blocks
            .iter()
            .enumerate()
            .filter(|(_, block)| kind(block) == "paragraph")
            .map(|(index, block)| {
                let inlines: Vec<Value> = children(block)
                    .iter()
                    .filter(|inline| kind(inline) != "hardBreak")
                    .cloned()
                    .collect();
                self.inlines(&inlines, &format!("{path}/content/{index}"), Within::Cell)
            })
            .collect();
        written.join(" ")
    }

    /// Inline nodes, their marks opened and closed as few times as they may be.
    fn inlines(&mut self, nodes: &[Value], path: &str, within: Within) -> String {
        let pieces = pieces_of(nodes);
        let mut out = String::new();
        let mut open: Vec<&Mark> = Vec::new();
        let mut line_empty = true;
        for (index, piece) in pieces.iter().enumerate() {
            let kept = open
                .iter()
                .zip(&piece.marks)
                .take_while(|(open, wanted)| **open == *wanted)
                .count();
            while open.len() > kept {
                if let Some(mark) = open.pop() {
                    out.push_str(&self.closing(mark, path, within));
                }
            }
            for mark in &piece.marks[kept..] {
                out.push_str(mark.opening());
                open.push(mark);
                line_empty = false;
            }
            let line_ends = pieces
                .get(index + 1)
                .is_none_or(|next| matches!(next.what, What::Break));
            let line = match (line_empty, line_ends) {
                (true, true) => Line::Whole,
                (true, false) => Line::Starts,
                (false, true) => Line::Ends,
                (false, false) => Line::Inside,
            };
            match &piece.what {
                What::Text(text) => out.push_str(&escape(text, line, within)),
                What::Code(code) => {
                    if within == Within::Summary && code.contains(MARKUP) {
                        self.complex(path, ComplexBlockReason::MarkupInSummary);
                    }
                    out.push_str(&code_span(code, within));
                }
                What::Node(node) => {
                    if within == Within::Summary && attr_text(node, "latex").contains(MARKUP) {
                        self.complex(path, ComplexBlockReason::MarkupInSummary);
                    }
                    let at = format!("{path}/content/{index}");
                    let written = self.inline_node(node, &at, within);
                    out.push_str(&written);
                }
                What::Break => {
                    out.push_str("\\\n");
                    line_empty = true;
                    continue;
                }
            }
            line_empty = false;
        }
        while let Some(mark) = open.pop() {
            out.push_str(&self.closing(mark, path, within));
        }
        out
    }

    /// Where a file is linked: where the export puts it, or by its identifier.
    fn file_link(&self, id: &str) -> String {
        self.names
            .export
            .as_ref()
            .and_then(|exported| exported.file(id))
            .unwrap_or_else(|| format!("{FILE_SCHEME}{id}"))
    }

    /// A reference: `[[id]]`, or in an export a link to what it names where the export
    /// holds it, and its title where the reader reads it elsewhere. `within` is a
    /// summary's or a paragraph's.
    fn reference(&self, id: &str, within: Within) -> String {
        let Some(exported) = &self.names.export else {
            return format!("[[{id}]]");
        };
        if let Some((to, title)) = exported.places.blocks.get(id) {
            let link = if within == Within::Summary {
                exported.link_in_summary(to)
            } else {
                exported.link(to)
            };
            return format!("[{}]({link})", escape(title, Line::Inside, within));
        }
        match exported.places.titles.get(id) {
            Some(title) => escape(title, Line::Inside, within),
            None => format!("[[{id}]]"),
        }
    }

    /// The end of a mark; in a summary, a link whose target HTML would read as markup
    /// is named, and its target written without it.
    fn closing(&mut self, mark: &Mark, path: &str, within: Within) -> String {
        match mark {
            Mark::Link(href) if within == Within::Summary => {
                if href.contains(['<', '>', ' ']) {
                    self.complex(path, ComplexBlockReason::MarkupInSummary);
                }
                let target = href
                    .replace(' ', "%20")
                    .replace('<', "%3C")
                    .replace('>', "%3E");
                format!("]({})", as_references(&destination(&target)))
            }
            _ => mark.closing(),
        }
    }

    fn inline_node(&mut self, node: &Value, path: &str, within: Within) -> String {
        match kind(node) {
            "mention" => {
                let id = attr_text(node, "userId");
                // Who is not seen is named by the identifier alone.
                let name = self.names.users.get(id).map_or("", String::as_str);
                let within = if within == Within::Summary {
                    Within::Summary
                } else {
                    Within::Paragraph
                };
                if self.names.export.is_some() {
                    let shown = if name.is_empty() { id } else { name };
                    return format!("@{}", escape(shown, Line::Inside, within));
                }
                format!(
                    "@[{}]({USER_SCHEME}{id})",
                    escape(name, Line::Inside, within)
                )
            }
            "reference" => {
                let within = if within == Within::Summary {
                    Within::Summary
                } else {
                    Within::Paragraph
                };
                self.reference(attr_text(node, "id"), within)
            }
            "inlineMath" => {
                let latex = attr_text(node, "latex");
                if !inline_formula_is_written(latex) {
                    self.complex(path, ComplexBlockReason::UnwrittenFormula);
                }
                if within == Within::Summary {
                    return format!("${}$", as_references(latex));
                }
                format!("${latex}$")
            }
            _ => String::new(),
        }
    }
}

/// Prefixes the first line of `text` with `first` and each other with `rest`; a line
/// that says nothing takes the prefix without its spaces.
fn prefixed(text: &str, first: &str, rest: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (index, line) in text.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
        }
        let prefix = if index == 0 { first } else { rest };
        if line.is_empty() {
            out.push_str(prefix.trim_end());
        } else {
            out.push_str(prefix);
            out.push_str(line);
        }
    }
    out
}

fn code_block(node: &Value) -> String {
    let text: String = children(node)
        .iter()
        .filter_map(|text| text.get("text").and_then(Value::as_str))
        .collect();
    let fence = "`".repeat(longest_run(&text, '`').max(2) + 1);
    let language = attr_text(node, "language");
    if text.is_empty() {
        format!("{fence}{language}\n{fence}")
    } else {
        format!("{fence}{language}\n{text}\n{fence}")
    }
}

fn longest_run(text: &str, of: char) -> usize {
    let (mut longest, mut run) = (0, 0);
    for letter in text.chars() {
        run = if letter == of { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    longest
}

fn code_span(code: &str, within: Within) -> String {
    let fence = "`".repeat(longest_run(code, '`') + 1);
    let padded = code.starts_with('`')
        || code.ends_with('`')
        || (code.starts_with(' ') && code.ends_with(' ') && code.trim() != "");
    let code = match within {
        Within::Cell => code.replace('|', "\\|"),
        Within::Summary => as_references(code),
        Within::Paragraph | Within::Heading => code.to_owned(),
    };
    if padded {
        format!("{fence} {code} {fence}")
    } else {
        format!("{fence}{code}{fence}")
    }
}

/// Whether `$…$` holds the formula `latex` as it is: Markdown reads no `$` inside but
/// after a backslash, and no space just inside the marks.
fn inline_formula_is_written(latex: &str) -> bool {
    let unescaped_dollar = latex
        .char_indices()
        .any(|(at, letter)| letter == '$' && !latex[..at].ends_with('\\'));
    !latex.is_empty() && latex.trim() == latex && !unescaped_dollar
}

/// Whether `$$…$$` holds the formula `latex` as it is: no `$$` inside, no space at its
/// edges, and no line after the first that Markdown would read as a block of its own.
fn block_formula_is_written(latex: &str) -> bool {
    let starts_a_block = |line: &str| {
        let line = line.trim_start_matches(' ');
        line.starts_with(['#', '-', '+', '*', '>', '`', '~', '<', '=', '|'])
            || line.starts_with(|letter: char| letter.is_ascii_digit())
            || line.is_empty()
    };
    !latex.is_empty()
        && latex.trim() == latex
        && !latex.contains("$$")
        && !latex.ends_with('$')
        && !latex.split('\n').skip(1).any(starts_a_block)
}

/// Where a run of text is on its line: Markdown reads marks at the start of a line,
/// and drops spaces at either end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Line {
    Starts,
    Ends,
    Whole,
    Inside,
}

impl Line {
    fn starts(self) -> bool {
        matches!(self, Self::Starts | Self::Whole)
    }

    fn ends(self) -> bool {
        matches!(self, Self::Ends | Self::Whole)
    }
}

/// Text as Markdown reads it back the same: every mark it would read is escaped.
fn escape(text: &str, line: Line, within: Within) -> String {
    let letters: Vec<char> = text.chars().collect();
    let lead = if line.starts() {
        letters
            .iter()
            .take_while(|c| matches!(c, ' ' | '\t'))
            .count()
    } else {
        0
    };
    let trail_from = if line.ends() {
        letters.len()
            - letters[lead..]
                .iter()
                .rev()
                .take_while(|c| matches!(c, ' ' | '\t'))
                .count()
    } else {
        letters.len()
    };
    let digits = letters[lead..]
        .iter()
        .take_while(|c| c.is_ascii_digit())
        .count();
    let mut out = String::with_capacity(text.len() + 8);
    for (at, &letter) in letters.iter().enumerate() {
        let before = at.checked_sub(1).and_then(|before| letters.get(before));
        let after = letters.get(at + 1);
        let first = line.starts() && at == lead;
        if at < lead || at >= trail_from {
            out.push_str(if letter == '\t' { "&#9;" } else { "&#32;" });
            continue;
        }
        if within == Within::Summary
            && let Some(reference) = reference_of(letter)
        {
            out.push_str(reference);
            continue;
        }
        let escaped = match letter {
            '\\' | '`' | '*' | '[' | ']' | '<' | '~' | '|' | '$' => true,
            '_' => {
                !(before.is_some_and(|c| c.is_alphanumeric())
                    && after.is_some_and(|c| c.is_alphanumeric()))
            }
            '&' => after.is_some_and(|c| c.is_alphanumeric() || *c == '#'),
            '#' => within == Within::Heading || first,
            '>' | '+' | '-' | '=' => first,
            '.' | ')' => line.starts() && digits > 0 && at == lead + digits,
            _ => false,
        };
        if escaped {
            out.push('\\');
        }
        out.push(letter);
    }
    out
}

/// A mark of inline text, in the order they are opened: a link outside the others.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Mark {
    Link(String),
    Bold,
    Italic,
    Strike,
}

impl Mark {
    fn of(mark: &Value) -> Option<Self> {
        Some(match kind(mark) {
            "link" => Self::Link(attr_text(mark, "href").to_owned()),
            "bold" => Self::Bold,
            "italic" => Self::Italic,
            "strike" => Self::Strike,
            _ => return None,
        })
    }

    fn is_emphasis(&self) -> bool {
        !matches!(self, Self::Link(_))
    }

    fn opening(&self) -> &'static str {
        match self {
            Self::Link(_) => "[",
            Self::Bold => "**",
            Self::Italic => "*",
            Self::Strike => "~~",
        }
    }

    fn closing(&self) -> String {
        match self {
            Self::Link(href) => format!("]({})", destination(href)),
            Self::Bold => "**".to_owned(),
            Self::Italic => "*".to_owned(),
            Self::Strike => "~~".to_owned(),
        }
    }
}

/// A link's target as Markdown reads it back: in `<…>` where it has a space.
fn destination(href: &str) -> String {
    if href.contains(' ') {
        format!("<{}>", href.replace('<', "\\<").replace('>', "\\>"))
    } else {
        let mut out = String::with_capacity(href.len());
        for letter in href.chars() {
            if matches!(letter, '(' | ')' | '\\' | '<' | '>') {
                out.push('\\');
            }
            out.push(letter);
        }
        out
    }
}

/// What an inline piece is.
#[derive(Debug)]
enum What {
    Text(String),
    Code(String),
    /// A mention, a reference or a formula.
    Node(Value),
    Break,
}

#[derive(Debug)]
struct Piece {
    what: What,
    marks: Vec<Mark>,
}

/// The pieces of a run of inline nodes: the spaces at the edge of a bold, italic or
/// struck text are a piece of their own, without those marks, as Markdown closes no
/// such mark after a space nor opens one before. A line break at the end is dropped:
/// Markdown writes none there.
fn pieces_of(nodes: &[Value]) -> Vec<Piece> {
    let mut end = nodes.len();
    while end > 0 && kind(&nodes[end - 1]) == "hardBreak" {
        end -= 1;
    }
    let mut pieces = Vec::with_capacity(end);
    for node in &nodes[..end] {
        let all_marks = node
            .get("marks")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice);
        let mut marks: Vec<Mark> = all_marks.iter().filter_map(Mark::of).collect();
        marks.sort();
        let coded = all_marks.iter().any(|mark| kind(mark) == "code");
        let words = node.get("text").and_then(Value::as_str).unwrap_or_default();
        let what = match kind(node) {
            "text" if coded => What::Code(words.to_owned()),
            "text" => What::Text(words.to_owned()),
            "hardBreak" => What::Break,
            _ => What::Node(node.clone()),
        };
        let emphasized = marks.iter().any(Mark::is_emphasis);
        if !(emphasized && matches!(what, What::Text(_))) {
            pieces.push(Piece { what, marks });
            continue;
        }
        let core = words.trim_matches([' ', '\t']);
        let lead = &words[..words.len() - words.trim_start_matches([' ', '\t']).len()];
        let trail = &words[lead.len() + core.len()..];
        let plain: Vec<Mark> = marks
            .iter()
            .filter(|mark| !mark.is_emphasis())
            .cloned()
            .collect();
        for (text, marks) in [(lead, plain.clone()), (core, marks), (trail, plain)] {
            if !text.is_empty() {
                pieces.push(Piece {
                    what: What::Text(text.to_owned()),
                    marks,
                });
            }
        }
    }
    pieces
}
