//! DOCX writer over the shared export model (`export_model`), on `docx-rs`.
//!
//! Source `export/docx.ts`: `tiptapDocToMd` -> remark/rehype -> `@m2d/core`.
//! This writer keeps the product meaning of that path without the Markdown
//! round trip: title as the `Title` paragraph, headings as `Heading1..6`,
//! bullet/ordered/task lists as Word numbering levels, tables on a grid with
//! a bold header row, code/mermaid/math source in a monospace font, quotes and
//! callouts (with the `[!KIND]` label) as left-bordered paragraphs,
//! attachments and embeds as their text placeholders. Fonts are named only
//! (`Consolas`, as `@m2d/core`); nothing is embedded and no attachment bytes
//! are read. Intentional differences from the TS output (D1–D16, e.g.
//! underline/highlight are kept, unsafe link schemes are not turned into
//! hyperlinks) are listed in `compat/fixtures/export-docx/README.md`.
//!
//! Runs in the `--internal-markdown` child only (`tiptap-to-docx`).

use std::io::Cursor;

use docx_rs::{
    AbstractNumbering, AlignmentType, BorderType, BreakType, Docx, Hyperlink, HyperlinkType,
    IndentLevel, Level, LevelJc, LevelOverride, LevelText, NumberFormat, Numbering, NumberingId,
    Paragraph, ParagraphBorder, ParagraphBorderPosition, Run, RunFonts, Shading, ShdType,
    SpecialIndentType, Start, Style, StyleType, Table, TableCell, TableRow, WidthType,
};

use crate::documents::export_model::{Block, ExportDoc, Inline, ListItem, ListKind, Marks};

/// Source `limits.ts`/`docx.ts`: the serializer output cap.
pub const DOCX_MAX_OUTPUT_BYTES: usize = 20_000_000;

pub const DOCX_CONTENT_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document";

const CODE_FONT: &str = "Consolas";
/// A4 portrait with 1-inch margins (the `@m2d/core` section): text width.
const TEXT_WIDTH_TWIPS: usize = 11906 - 2 * 1440;
const LIST_INDENT: i32 = 720;
const QUOTE_INDENT: i32 = 360;
/// Word numbering has nine levels (0..=8); deeper lists stay on the last.
const MAX_LIST_LEVEL: usize = 8;
const ABSTRACT_BULLET: usize = 2;
const ABSTRACT_ORDERED: usize = 3;
const ABSTRACT_TASK: usize = 4;

#[derive(Debug, thiserror::Error)]
pub enum DocxError {
    /// Source `ExportLimitError("maxOutputBytes")`.
    #[error("docx exceeds {DOCX_MAX_OUTPUT_BYTES} bytes")]
    TooLarge,
    #[error("docx pack failed: {0}")]
    Pack(String),
}

pub fn write_docx(doc: &ExportDoc) -> Result<Vec<u8>, DocxError> {
    let mut w = Writer { next_num_id: 2 };
    let mut body = Vec::new();
    if let Some(title) = &doc.title {
        body.push(Child::P(
            Paragraph::new()
                .style("Title")
                .add_run(Run::new().add_text(xml_text(title))),
        ));
    }
    let mut numberings = Vec::new();
    w.blocks(&doc.blocks, Cx::default(), &mut body, &mut numberings);

    let mut docx = styles(Docx::new())
        .page_size(11906, 16838)
        .add_abstract_numbering(list_numbering(ABSTRACT_BULLET, ListKind::Bullet))
        .add_abstract_numbering(list_numbering(ABSTRACT_ORDERED, ListKind::Ordered))
        .add_abstract_numbering(list_numbering(ABSTRACT_TASK, ListKind::Task));
    for n in numberings {
        docx = docx.add_numbering(n);
    }
    for child in body {
        docx = match child {
            Child::P(p) => docx.add_paragraph(p),
            Child::T(t) => docx.add_table(t),
        };
    }
    let mut stored = Cursor::new(Vec::new());
    docx.pack(&mut stored)
        .map_err(|e| DocxError::Pack(e.to_string()))?;
    let bytes = deflate_package(stored.get_ref()).map_err(|e| DocxError::Pack(e.to_string()))?;
    if bytes.len() > DOCX_MAX_OUTPUT_BYTES {
        return Err(DocxError::TooLarge);
    }
    Ok(bytes)
}

/// `docx-rs` writes every part `Stored`; Word files are normally deflated
/// (the TS output is ~1/100 the size on text-heavy bodies). Same parts, same
/// order, compressed.
fn deflate_package(stored: &[u8]) -> docx_zip::result::ZipResult<Vec<u8>> {
    use docx_zip::write::SimpleFileOptions;
    let mut archive = docx_zip::ZipArchive::new(Cursor::new(stored))?;
    let mut out = docx_zip::ZipWriter::new(Cursor::new(Vec::with_capacity(stored.len() / 4)));
    let options =
        SimpleFileOptions::default().compression_method(docx_zip::CompressionMethod::Deflated);
    for i in 0..archive.len() {
        let mut part = archive.by_index(i)?;
        // OPC has no directory parts; copied with `start_file`, docx-rs's
        // directory entries would become empty files named `word/` and break
        // extraction with `unzip`. Most DOCX producers omit them.
        if part.is_dir() {
            continue;
        }
        out.start_file(part.name().to_string(), options)?;
        std::io::copy(&mut part, &mut out)?;
    }
    Ok(out.finish()?.into_inner())
}

fn styles(docx: Docx) -> Docx {
    let heading = |level: usize, size: usize| {
        Style::new(format!("Heading{level}"), StyleType::Paragraph)
            .name(format!("Heading {level}"))
            .based_on("Normal")
            .next("Normal")
            .q_format(true)
            .bold()
            .size(size)
            .outline_lvl(level - 1)
    };
    docx.add_style(
        Style::new("Title", StyleType::Paragraph)
            .name("Title")
            .based_on("Normal")
            .next("Normal")
            .q_format(true)
            .size(56),
    )
    .add_style(heading(1, 32))
    .add_style(heading(2, 26))
    .add_style(heading(3, 24))
    .add_style(heading(4, 22))
    .add_style(heading(5, 22))
    .add_style(heading(6, 22))
    .add_style(
        Style::new("ListParagraph", StyleType::Paragraph)
            .name("List Paragraph")
            .based_on("Normal")
            .q_format(true),
    )
    .add_style(
        Style::new("Hyperlink", StyleType::Character)
            .name("Hyperlink")
            .color("0563C1")
            .underline("single"),
    )
}

fn list_numbering(id: usize, kind: ListKind) -> AbstractNumbering {
    let mut abs = AbstractNumbering::new(id);
    for level in 0..=MAX_LIST_LEVEL {
        let (format, text) = match kind {
            ListKind::Bullet => ("bullet", ["●", "○", "■"][level % 3].to_string()),
            ListKind::Ordered => ("decimal", format!("%{}.", level + 1)),
            // The checkbox glyph is part of the item text.
            ListKind::Task => ("none", String::new()),
        };
        abs = abs.add_level(
            Level::new(
                level,
                Start::new(1),
                NumberFormat::new(format),
                LevelText::new(text),
                LevelJc::new("left"),
            )
            .indent(
                Some(LIST_INDENT * (level as i32 + 1)),
                Some(SpecialIndentType::Hanging(360)),
                None,
                None,
            ),
        );
    }
    abs
}

/// Body children in document order before they are added to the `Docx`.
// Short-lived and moved straight into docx-rs, which stores them unboxed too.
#[allow(clippy::large_enum_variant)]
enum Child {
    P(Paragraph),
    T(Table),
}

/// Where a block sits: quote/callout nesting and the list level it continues.
#[derive(Clone, Copy, Default)]
struct Cx {
    quote_depth: i32,
    /// Level of the enclosing list item (continuation blocks indent to it).
    list_level: Option<usize>,
}

impl Cx {
    fn indent(self) -> i32 {
        let list = self
            .list_level
            .map(|l| LIST_INDENT * (l as i32 + 1))
            .unwrap_or(0);
        list + QUOTE_INDENT * self.quote_depth
    }

    /// Paragraph properties every block paragraph in this context shares.
    fn frame(self, mut p: Paragraph) -> Paragraph {
        let indent = self.indent();
        if indent > 0 {
            p = p.indent(Some(indent), None, None, None);
        }
        if self.quote_depth > 0 {
            p.property = p.property.set_border(
                ParagraphBorder::new(ParagraphBorderPosition::Left)
                    .val(BorderType::Single)
                    .size(12)
                    .space(8)
                    .color("AAAAAA"),
            );
        }
        p
    }
}

struct Writer {
    next_num_id: usize,
}

impl Writer {
    fn blocks(
        &mut self,
        blocks: &[Block],
        cx: Cx,
        out: &mut Vec<Child>,
        nums: &mut Vec<Numbering>,
    ) {
        for b in blocks {
            self.block(b, cx, out, nums);
        }
    }

    fn block(&mut self, block: &Block, cx: Cx, out: &mut Vec<Child>, nums: &mut Vec<Numbering>) {
        match block {
            Block::Heading { level, inlines } => {
                let p = runs(Paragraph::new().style(&format!("Heading{level}")), inlines);
                out.push(Child::P(cx.frame(p)));
            }
            // Source `blocksMd` drops blocks that render as "".
            Block::Paragraph(inlines) if !inlines.is_empty() => {
                out.push(Child::P(cx.frame(runs(Paragraph::new(), inlines))));
            }
            Block::Paragraph(_) => {}
            Block::List { kind, items } => self.list(*kind, items, cx, out, nums),
            Block::Table(rows) => out.push(Child::T(self.table(rows, nums))),
            Block::Code { text, .. } | Block::Mermaid(text) | Block::Math(text) => {
                out.push(Child::P(cx.frame(code_paragraph(text))));
            }
            Block::Blockquote(blocks) => {
                let inner = Cx {
                    quote_depth: cx.quote_depth + 1,
                    ..cx
                };
                self.blocks(blocks, inner, out, nums);
            }
            Block::Callout { kind, blocks } => {
                let inner = Cx {
                    quote_depth: cx.quote_depth + 1,
                    ..cx
                };
                // Source `> [!KIND]` followed by the body: the label and a
                // leading paragraph read as one paragraph.
                let label = Inline::Text {
                    text: format!("[!{}]", kind.to_uppercase()),
                    marks: Marks::default(),
                };
                match blocks.split_first() {
                    Some((Block::Paragraph(first), rest)) if !first.is_empty() => {
                        let mut inlines = vec![label];
                        let spaced = matches!(first.first(),
                            Some(Inline::Text { text, .. }) if text.starts_with(char::is_whitespace));
                        if !spaced {
                            inlines.push(Inline::Text {
                                text: " ".into(),
                                marks: Marks::default(),
                            });
                        }
                        inlines.extend(first.iter().cloned());
                        out.push(Child::P(inner.frame(runs(Paragraph::new(), &inlines))));
                        self.blocks(rest, inner, out, nums);
                    }
                    _ => {
                        out.push(Child::P(inner.frame(runs(Paragraph::new(), &[label]))));
                        self.blocks(blocks, inner, out, nums);
                    }
                }
            }
            Block::Attachment { name, .. } => {
                if !name.is_empty() {
                    out.push(Child::P(cx.frame(Paragraph::new().add_run(text_run(name)))));
                }
            }
            Block::Embed { entity, reference } => {
                let p = if entity == "url" {
                    if reference.is_empty() {
                        return;
                    }
                    let marks = Marks {
                        link: Some(reference.clone()),
                        ..Marks::default()
                    };
                    runs(
                        Paragraph::new(),
                        &[Inline::Text {
                            text: reference.clone(),
                            marks,
                        }],
                    )
                } else {
                    let entity = if entity == "document" { "doc" } else { entity };
                    Paragraph::new().add_run(text_run(&format!("[[{entity}:{reference}]]")))
                };
                out.push(Child::P(cx.frame(p)));
            }
            Block::HorizontalRule => {
                let mut p = cx.frame(Paragraph::new());
                p.property = p.property.set_border(
                    ParagraphBorder::new(ParagraphBorderPosition::Bottom)
                        .val(BorderType::Single)
                        .size(6)
                        .space(1)
                        .color("AAAAAA"),
                );
                out.push(Child::P(p));
            }
            Block::Details { summary, blocks } => {
                if !summary.is_empty() {
                    out.push(Child::P(cx.frame(runs(Paragraph::new(), summary))));
                }
                self.blocks(blocks, cx, out, nums);
            }
        }
    }

    fn list(
        &mut self,
        kind: ListKind,
        items: &[ListItem],
        cx: Cx,
        out: &mut Vec<Child>,
        nums: &mut Vec<Numbering>,
    ) {
        let level = cx.list_level.map_or(0, |l| (l + 1).min(MAX_LIST_LEVEL));
        let abstract_id = match kind {
            ListKind::Bullet => ABSTRACT_BULLET,
            ListKind::Ordered => ABSTRACT_ORDERED,
            ListKind::Task => ABSTRACT_TASK,
        };
        // Each list is its own numbering instance: ordered lists restart at 1
        // (source: `${i + 1}. `, `start` ignored).
        let num_id = self.next_num_id;
        self.next_num_id += 1;
        nums.push(
            Numbering::new(num_id, abstract_id).add_override(LevelOverride::new(level).start(1)),
        );
        let item_cx = Cx {
            list_level: Some(level),
            ..cx
        };
        for item in items {
            let (first, rest) = match item.blocks.split_first() {
                Some((Block::Paragraph(first), rest)) => (first.as_slice(), rest),
                _ => (&[][..], item.blocks.as_slice()),
            };
            let mut p = Paragraph::new()
                .style("ListParagraph")
                .numbering(NumberingId::new(num_id), IndentLevel::new(level));
            if let Some(checked) = item.checked {
                p = p.add_run(text_run(if checked { "☑ " } else { "☐ " }));
            }
            p = runs(p, first);
            // Quote framing without the list indent (numbering sets it).
            let frame = Cx {
                list_level: None,
                ..cx
            };
            let mut p = frame.frame(p);
            if frame.indent() > 0 {
                // Keep the level indent from the numbering, shifted by quotes.
                p = p.indent(
                    Some(LIST_INDENT * (level as i32 + 1) + frame.indent()),
                    Some(SpecialIndentType::Hanging(360)),
                    None,
                    None,
                );
            }
            out.push(Child::P(p));
            self.blocks(rest, item_cx, out, nums);
        }
    }

    fn table(
        &mut self,
        rows: &[crate::documents::export_model::TableRow],
        nums: &mut Vec<Numbering>,
    ) -> Table {
        let columns = rows.iter().map(|r| r.cells.len()).max().unwrap_or(0).max(1);
        let width = TEXT_WIDTH_TWIPS / columns;
        let mut out_rows = Vec::new();
        for (index, row) in rows.iter().enumerate() {
            // Source GFM: the first row is the header row.
            let header = index == 0;
            let mut cells = Vec::new();
            for c in 0..columns {
                let mut cell = TableCell::new().width(width, WidthType::Dxa);
                if header {
                    cell = cell.shading(Shading::new().shd_type(ShdType::Clear).fill("F2F2F2"));
                }
                let mut body = Vec::new();
                if let Some(src) = row.cells.get(c) {
                    self.blocks(&src.blocks, Cx::default(), &mut body, nums);
                }
                // A cell must end with a paragraph (ECMA-376 §17.4.66); Word
                // rejects a `w:tc` whose last child is a nested table.
                if !matches!(body.last(), Some(Child::P(_))) {
                    body.push(Child::P(Paragraph::new()));
                }
                for child in body {
                    cell = match child {
                        Child::P(p) if header => cell.add_paragraph(embolden(p)),
                        Child::P(p) => cell.add_paragraph(p),
                        Child::T(t) => cell.add_table(t),
                    };
                }
                cells.push(cell);
            }
            out_rows.push(TableRow::new(cells));
        }
        Table::new(out_rows)
            .set_grid(vec![width; columns])
            .width(width * columns, WidthType::Dxa)
    }
}

/// Header cells: every run bold (source: `@m2d/table` header row).
fn embolden(mut p: Paragraph) -> Paragraph {
    use docx_rs::ParagraphChild;
    p = p.align(AlignmentType::Left);
    for child in &mut p.children {
        match child {
            ParagraphChild::Run(run) => {
                let r = std::mem::take(run.as_mut());
                **run = r.bold();
            }
            ParagraphChild::Hyperlink(link) => {
                for c in &mut link.children {
                    if let ParagraphChild::Run(run) = c {
                        let r = std::mem::take(run.as_mut());
                        **run = r.bold();
                    }
                }
            }
            _ => {}
        }
    }
    p
}

fn code_fonts() -> RunFonts {
    RunFonts::new()
        .ascii(CODE_FONT)
        .hi_ansi(CODE_FONT)
        .east_asia(CODE_FONT)
        .cs(CODE_FONT)
}

/// Source text as monospace lines separated by line breaks.
fn code_paragraph(text: &str) -> Paragraph {
    let mut run = Run::new().fonts(code_fonts());
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            run = run.add_break(BreakType::TextWrapping);
        }
        let line = line.strip_suffix('\r').unwrap_or(line);
        // Tabs are Word tab stops in source text.
        for (j, part) in line.split('\t').enumerate() {
            if j > 0 {
                run = run.add_tab();
            }
            if !part.is_empty() {
                run = run.add_text(xml_text(part));
            }
        }
    }
    let mut p = Paragraph::new().keep_lines(true).add_run(run);
    p.property = p
        .property
        .shading(Shading::new().shd_type(ShdType::Clear).fill("F2F2F2"));
    p
}

fn text_run(text: &str) -> Run {
    Run::new().add_text(xml_text(text))
}

fn styled_run(text: &str, marks: &Marks) -> Run {
    let mut run = text_run(text);
    if marks.bold {
        run = run.bold();
    }
    if marks.italic {
        run = run.italic();
    }
    if marks.strike {
        run = run.strike();
    }
    if marks.underline {
        run = run.underline("single");
    }
    if marks.highlight {
        run = run.highlight("yellow");
    }
    if marks.code {
        run = run.fonts(code_fonts());
    }
    run
}

fn runs(mut p: Paragraph, inlines: &[Inline]) -> Paragraph {
    for inline in inlines {
        let (text, marks) = match inline {
            Inline::HardBreak => {
                p = p.add_run(Run::new().add_break(BreakType::TextWrapping));
                continue;
            }
            Inline::Text { text, marks } => (text.clone(), marks),
            // Source `mathInlineMd`: `$latex$` (whitespace runs folded), left
            // as text by the DOCX Markdown parser (no math extension).
            Inline::Math { latex, marks } => {
                let latex = latex.split_whitespace().collect::<Vec<_>>().join(" ");
                if latex.is_empty() {
                    continue;
                }
                (format!("${latex}$"), marks)
            }
        };
        let run = styled_run(&text, marks);
        match marks.link.as_deref().and_then(hyperlink_target) {
            Some(target) => {
                p = p.add_hyperlink(
                    Hyperlink::new(target, HyperlinkType::External).add_run(run.style("Hyperlink")),
                );
            }
            None => p = p.add_run(run),
        }
    }
    p
}

/// External hyperlinks only for absolute http(s)/mailto URLs. The stored
/// href is kept when it is already a plain ASCII URI; otherwise the URL
/// parser's percent-encoded form is the relationship target (it must be a
/// valid URI). Other hrefs (relative, `javascript:`, `data:`, `attachment:`)
/// keep their text only.
pub(crate) fn hyperlink_target(href: &str) -> Option<String> {
    let href = href.trim();
    let url = url::Url::parse(href).ok()?;
    if !matches!(url.scheme(), "http" | "https" | "mailto") {
        return None;
    }
    let plain = href.bytes().all(|b| b.is_ascii_graphic());
    let candidate = if plain {
        href.to_string()
    } else {
        url.to_string()
    };
    Some(rfc3986_uri(&candidate))
}

/// Percent-encodes every byte RFC 3986 does not allow in a URI (outside
/// unreserved, reserved and `%` starting a valid escape). The WHATWG
/// serialization leaves some of them (`|`, `^`, `"` in a fragment) and a stored
/// ASCII href can carry any of them; the relationship target must be a URI.
fn rfc3986_uri(uri: &str) -> String {
    let bytes = uri.as_bytes();
    let mut out = String::with_capacity(uri.len());
    for (i, &b) in bytes.iter().enumerate() {
        let allowed = b.is_ascii_alphanumeric()
            || b"-._~:/?#[]@!$&'()*+,;=".contains(&b)
            || (b == b'%'
                && bytes.get(i + 1).is_some_and(u8::is_ascii_hexdigit)
                && bytes.get(i + 2).is_some_and(u8::is_ascii_hexdigit));
        if allowed {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Text of one run. XML 1.0 forbids most C0 controls (and U+FFFE/U+FFFF)
/// even escaped, and the stored JSON may hold them: NUL reads as U+FFFD (as
/// the TS Markdown parser replaced it), the others are dropped. Line breaks
/// (CR LF, CR, LF) and tabs inside prose read as one space, as the TS export
/// and the share page render them; hard breaks are `HardBreak` inlines.
fn xml_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\0' => out.push('\u{FFFD}'),
            '\r' => {
                chars.next_if_eq(&'\n');
                out.push(' ');
            }
            '\n' | '\t' => out.push(' '),
            '\u{FFFE}' | '\u{FFFF}' => {}
            c if c < '\u{20}' => {}
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::documents::export_model::export_doc;
    use serde_json::json;

    fn document_xml(bytes: &[u8]) -> String {
        docx_rs::read_docx(bytes).expect("docx-rs reads its own output");
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut xml = String::new();
        std::io::Read::read_to_string(&mut zip.by_name("word/document.xml").unwrap(), &mut xml)
            .unwrap();
        xml
    }

    #[test]
    fn writes_a_readable_docx() {
        let doc = json!({"type": "doc", "content": [
            {"type": "heading", "attrs": {"level": 2}, "content": [{"type": "text", "text": "제목"}]},
            {"type": "paragraph", "content": [
                {"type": "text", "text": "b", "marks": [{"type": "bold"}]},
                {"type": "text", "text": "a&b", "marks": [{"type": "link", "attrs": {"href": "https://e.com/?a=1&b=2"}}]},
                {"type": "text", "text": "js", "marks": [{"type": "link", "attrs": {"href": "javascript:alert(1)"}}]},
                {"type": "text", "text": "ctl\u{1}x"}]},
            {"type": "bulletList", "content": [{"type": "listItem", "content": [
                {"type": "paragraph", "content": [{"type": "text", "text": "one"}]},
                {"type": "orderedList", "content": [{"type": "listItem", "content": [
                    {"type": "paragraph", "content": [{"type": "text", "text": "sub"}]}]}]}]}]},
            {"type": "table", "content": [{"type": "tableRow", "content": [
                {"type": "tableHeader", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "H"}]}]}]},
                {"type": "tableRow", "content": [
                {"type": "tableCell", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "c"}]}]},
                {"type": "tableCell", "content": []}]}]},
            {"type": "codeBlock", "content": [{"type": "text", "text": "a\nb"}]}
        ]});
        let bytes = write_docx(&export_doc("문서", &doc)).unwrap();
        let xml = document_xml(&bytes);
        assert!(xml.contains("<w:pStyle w:val=\"Title\" />"), "{xml}");
        assert!(xml.contains("Heading2"));
        assert!(xml.contains("제목"));
        assert!(xml.contains("ctlx"));
        assert!(!xml.contains("javascript"));
        assert_eq!(xml.matches("<w:hyperlink").count(), 1);
        assert_eq!(xml.matches("<w:gridCol").count(), 2);
        assert!(xml.contains("Consolas"));
        let mut zip = zip::ZipArchive::new(Cursor::new(&bytes)).unwrap();
        let mut rels = String::new();
        std::io::Read::read_to_string(
            &mut zip.by_name("word/_rels/document.xml.rels").unwrap(),
            &mut rels,
        )
        .unwrap();
        assert!(rels.contains("https://e.com/?a=1&amp;b=2\""), "{rels}");
        // OPC parts only: a directory entry would extract as an empty file.
        for i in 0..zip.len() {
            let name = zip.by_index(i).unwrap().name().to_string();
            assert!(!name.ends_with('/'), "{name}");
        }
    }

    /// ECMA-376: `w:tc` ends with a `w:p`, also after a nested table.
    #[test]
    fn cell_ending_in_a_table_gets_a_trailing_paragraph() {
        let inner = json!({"type": "table", "content": [{"type": "tableRow", "content": [
            {"type": "tableCell", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "in"}]}]}]}]});
        let doc = json!({"type": "doc", "content": [{"type": "table", "content": [
            {"type": "tableRow", "content": [
                {"type": "tableCell", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "a"}]}, inner]},
                {"type": "tableCell", "content": [inner]}]}]}]});
        let xml = document_xml(&write_docx(&export_doc("", &doc)).unwrap());
        assert_eq!(xml.matches("<w:tbl>").count(), 3, "{xml}");
        let squashed: String = xml.split_whitespace().collect();
        assert!(!squashed.contains("</w:tbl></w:tc>"), "{xml}");
    }

    #[test]
    fn hyperlink_targets_are_rfc3986_uris() {
        let cases = [
            ("https://e.com/a b?q=x y", "https://e.com/a%20b?q=x%20y"),
            (
                "https://e.com/문서?제목=회의",
                "https://e.com/%EB%AC%B8%EC%84%9C?%EC%A0%9C%EB%AA%A9=%ED%9A%8C%EC%9D%98",
            ),
            (
                "https://e.com/a\"b<c>d&e='f'",
                "https://e.com/a%22b%3Cc%3Ed&e='f'",
            ),
            (
                "https://e.com/p|q^r`s{t}?w=%zz&x=%41#h|i",
                "https://e.com/p%7Cq%5Er%60s%7Bt%7D?w=%25zz&x=%41#h%7Ci",
            ),
            ("https://e.com/?a=1&b=2", "https://e.com/?a=1&b=2"),
        ];
        for (href, want) in cases {
            assert_eq!(hyperlink_target(href).as_deref(), Some(want), "{href}");
        }
        for href in ["javascript:alert('x y')", "/docs/1 2", "data:text/html,<b>"] {
            assert_eq!(hyperlink_target(href), None, "{href}");
        }
    }

    #[test]
    fn empty_doc_and_title_only() {
        let bytes = write_docx(&export_doc("", &json!({"type": "doc"}))).unwrap();
        document_xml(&bytes);
    }
}
