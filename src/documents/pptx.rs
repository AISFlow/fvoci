//! PPTX writer over the shared export model (`export_model`), with the zip
//! writer the DOCX export already links and hand-written PresentationML.
//!
//! Source `export/pptx.ts` (pptxgenjs): a 10 x 5.625 in (16:9) deck, text
//! boxes stacked from the top of a slide at x = 0.5 in, 9 in wide, with a
//! cursor that opens the next slide when less than `MIN_H` is left; a
//! top-level horizontal rule always starts a new slide; the title is the
//! first heading (28 pt, later headings on a slide 16 pt); paragraphs 14 pt;
//! one text box per top-level list item; tables as real tables (12 pt);
//! code, display math and Mermaid source in the mono font (12 pt); quotes
//! indented; attachments/embeds as their text placeholders. Box heights use
//! the TS `estimateH` line estimate. Fonts are named only (`Noto Sans KR`,
//! `Noto Sans Mono CJK KR`), nothing is embedded, no attachment bytes are
//! read. Intentional differences from the TS output (P1–P15: marks and safe
//! hyperlinks kept, list nesting as levels with the right numbers, task
//! checkboxes, blocks taller than the rest of a slide continue on the next
//! slide instead of shrinking, tables split across slides, ...) are listed in
//! `compat/fixtures/export-pptx/README.md`.
//!
//! Runs in the `--internal-markdown` child only (`tiptap-to-pptx`).

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::io::{Cursor, Write as _};

use crate::documents::docx::hyperlink_target;
use crate::documents::export_model::{Block, ExportDoc, Inline, ListKind, Marks, TableRow};

/// Source `limits.ts`/`pptx.ts`: the serializer output cap.
pub const PPTX_MAX_OUTPUT_BYTES: usize = 20_000_000;

pub const PPTX_CONTENT_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.presentationml.presentation";

const BODY_FONT: &str = "Noto Sans KR";
const MONO_FONT: &str = "Noto Sans Mono CJK KR";

/// Layout in inches (source constants; pptxgenjs `LAYOUT_16x9`).
const SLIDE_W: f64 = 10.0;
const SLIDE_H: f64 = 5.625;
const X: f64 = 0.5;
const W: f64 = 9.0;
const TOP: f64 = 0.4;
const BOTTOM: f64 = 0.4;
const MIN_H: f64 = 0.34;
const GAP: f64 = 0.04;
const TABLE_GAP: f64 = 0.08;
const QUOTE_INDENT: f64 = 0.3;
const BODY_SIZE: f64 = 14.0;
const CODE_SIZE: f64 = 12.0;
const TABLE_SIZE: f64 = 12.0;
const FIRST_HEADING: (f64, f64) = (28.0, 0.55);
const LATER_HEADING: (f64, f64) = (16.0, 0.38);
/// Minimum table row height (source: `rows.length * 0.32`).
const ROW_H: f64 = 0.32;
/// Cell insets (DrawingML defaults 0.1 in / 0.05 in).
const CELL_PAD_X: f64 = 0.1;
const CELL_PAD_Y: f64 = 0.05;

const EMU_PER_INCH: f64 = 914_400.0;
/// Indent of one list/quote level in EMU (0.3125 in).
const LEVEL_EMU: i64 = 285_750;
/// DrawingML paragraph levels are 0..=8.
const MAX_LEVEL: usize = 8;

const QUOTE_BAR: &str = "7D797A";
const RULE: &str = "DDDDDD";
const HEADER_FILL: &str = "F3F4F6";
const HIGHLIGHT: &str = "FFFF00";

#[derive(Debug, thiserror::Error)]
pub enum PptxError {
    /// Source `ExportLimitError("maxOutputBytes")`.
    #[error("pptx exceeds {PPTX_MAX_OUTPUT_BYTES} bytes")]
    TooLarge,
    #[error("pptx pack failed: {0}")]
    Pack(String),
}

pub fn write_pptx(doc: &ExportDoc) -> Result<Vec<u8>, PptxError> {
    let mut deck = Deck::new();
    if let Some(title) = &doc.title {
        deck.heading(&[Inline::Text {
            text: title.clone(),
            marks: Marks::default(),
        }]);
    }
    for block in &doc.blocks {
        if matches!(block, Block::HorizontalRule) {
            deck.new_slide();
        } else {
            deck.block(block);
        }
    }
    let bytes =
        pack(doc.title.as_deref(), &deck.slides).map_err(|e| PptxError::Pack(e.to_string()))?;
    if bytes.len() > PPTX_MAX_OUTPUT_BYTES {
        return Err(PptxError::TooLarge);
    }
    Ok(bytes)
}

// ---------------------------------------------------------------------------
// Text model of one box

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Bullet {
    None,
    Dot,
    /// 1-based number of the item among its siblings.
    Number(usize),
}

#[derive(Debug, Clone)]
enum Run {
    Text { text: String, marks: Marks },
    Break,
}

#[derive(Debug, Clone)]
struct Para {
    runs: Vec<Run>,
    level: usize,
    bullet: Bullet,
    mono: bool,
    bold: bool,
}

impl Para {
    fn new(level: usize) -> Self {
        Self {
            runs: Vec::new(),
            level: level.min(MAX_LEVEL),
            bullet: Bullet::None,
            mono: false,
            bold: false,
        }
    }

    fn text(text: &str, level: usize) -> Self {
        let mut p = Self::new(level);
        p.push(text, Marks::default());
        p
    }

    fn inlines(inlines: &[Inline], level: usize) -> Self {
        let mut p = Self::new(level);
        for inline in inlines {
            match inline {
                Inline::Text { text, marks } => p.push(text, marks.clone()),
                // Source: the LaTeX text inline, as the other exports.
                Inline::Math { latex, marks } => p.push(latex, marks.clone()),
                Inline::HardBreak => p.runs.push(Run::Break),
            }
        }
        p
    }

    /// A line break inside stored text is a line break on the slide (source:
    /// pptxgenjs splits text runs at `\n`), unlike DOCX where it reads as a
    /// space.
    fn push(&mut self, text: &str, marks: Marks) {
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        for (i, line) in text.split('\n').enumerate() {
            if i > 0 {
                self.runs.push(Run::Break);
            }
            let line = clean(line);
            if !line.is_empty() {
                self.runs.push(Run::Text {
                    text: line,
                    marks: marks.clone(),
                });
            }
        }
    }

    fn is_empty(&self) -> bool {
        !self
            .runs
            .iter()
            .any(|r| matches!(r, Run::Text { text, .. } if !text.is_empty()))
    }

    /// Source `estimateH`: each line (hard break) takes
    /// `max(1, ceil(chars / cpl))` rows.
    fn lines(&self, cpl: usize) -> usize {
        let mut lines = 0;
        let mut chars = 0usize;
        for run in &self.runs {
            match run {
                Run::Text { text, .. } => chars += text.chars().count(),
                Run::Break => {
                    lines += chars.div_ceil(cpl).max(1);
                    chars = 0;
                }
            }
        }
        lines + chars.div_ceil(cpl).max(1)
    }

    /// Splits after `max` estimated rows: the head fits, the tail continues
    /// (without its bullet) in the next box.
    fn split(mut self, max: usize, cpl: usize) -> (Para, Para) {
        let mut tail = Para {
            runs: Vec::new(),
            bullet: Bullet::None,
            ..self.clone()
        };
        let runs = std::mem::take(&mut self.runs);
        let (mut done, mut col) = (0usize, 0usize);
        let mut split = false;
        for run in runs {
            if split {
                tail.runs.push(run);
                continue;
            }
            match run {
                Run::Break => {
                    done += 1;
                    col = 0;
                    if done >= max {
                        split = true;
                    } else {
                        self.runs.push(Run::Break);
                    }
                }
                Run::Text { text, marks } => {
                    let mut head = String::new();
                    let mut rest = String::new();
                    for c in text.chars() {
                        if !split && col == cpl {
                            done += 1;
                            col = 0;
                            split = done >= max;
                        }
                        if split {
                            rest.push(c);
                        } else {
                            head.push(c);
                            col += 1;
                        }
                    }
                    if !head.is_empty() {
                        self.runs.push(Run::Text {
                            text: head,
                            marks: marks.clone(),
                        });
                    }
                    if !rest.is_empty() {
                        tail.runs.push(Run::Text { text: rest, marks });
                    }
                }
            }
        }
        (self, tail)
    }
}

/// Characters per estimated row (source `estimateH`: one em per character).
fn cpl(font_size: f64, w: f64) -> usize {
    (((w * 72.0) / font_size).floor() as usize).max(8)
}

fn line_h(font_size: f64) -> f64 {
    font_size / 72.0 * 1.25
}

#[derive(Debug, Clone, Copy)]
struct BoxStyle {
    x: f64,
    w: f64,
    size: f64,
    /// Fixed box height (headings); never split.
    fixed: Option<f64>,
    /// Left bar color (quotes, callouts) and background.
    bar: Option<&'static str>,
    fill: Option<&'static str>,
}

impl BoxStyle {
    fn body() -> Self {
        Self {
            x: X,
            w: W,
            size: BODY_SIZE,
            fixed: None,
            bar: None,
            fill: None,
        }
    }

    fn mono() -> Self {
        Self {
            size: CODE_SIZE,
            ..Self::body()
        }
    }

    fn indented(bar: &'static str, fill: Option<&'static str>) -> Self {
        Self {
            x: X + QUOTE_INDENT,
            w: W - QUOTE_INDENT,
            bar: Some(bar),
            fill,
            ..Self::body()
        }
    }
}

// ---------------------------------------------------------------------------
// Slides

#[derive(Default)]
struct Slide {
    shapes: String,
    next_id: u32,
    /// External hyperlink targets; relationship `rId{i + 2}`.
    links: Vec<String>,
}

impl Slide {
    fn id(&mut self) -> u32 {
        self.next_id += 1;
        self.next_id + 1
    }

    fn link(&mut self, target: String) -> String {
        let index = match self.links.iter().position(|l| *l == target) {
            Some(i) => i,
            None => {
                self.links.push(target);
                self.links.len() - 1
            }
        };
        format!("rId{}", index + 2)
    }
}

struct Deck {
    slides: Vec<Slide>,
    y: f64,
    /// Headings on the current slide (source `cur.headings`).
    headings: usize,
}

impl Deck {
    fn new() -> Self {
        Self {
            slides: vec![Slide::default()],
            y: TOP,
            headings: 0,
        }
    }

    fn slide(&mut self) -> &mut Slide {
        self.slides.last_mut().expect("a deck has a slide")
    }

    /// Source `addSlide`.
    fn new_slide(&mut self) {
        self.slides.push(Slide::default());
        self.y = TOP;
        self.headings = 0;
    }

    fn remaining(&self) -> f64 {
        SLIDE_H - BOTTOM - self.y
    }

    fn heading(&mut self, inlines: &[Inline]) {
        // Source: the counter moves even when the heading is empty.
        let (size, h) = if self.headings == 0 {
            FIRST_HEADING
        } else {
            LATER_HEADING
        };
        self.headings += 1;
        let mut p = Para::inlines(inlines, 0);
        p.bold = true;
        self.text_box(
            vec![p],
            BoxStyle {
                size,
                fixed: Some(h),
                ..BoxStyle::body()
            },
        );
    }

    fn block(&mut self, block: &Block) {
        match block {
            Block::Heading { inlines, .. } => self.heading(inlines),
            Block::Paragraph(inlines) => {
                self.text_box(vec![Para::inlines(inlines, 0)], BoxStyle::body())
            }
            Block::List { kind, items } => {
                // Source: one box per item.
                for (i, item) in items.iter().enumerate() {
                    let mut paras = Vec::new();
                    list_item(*kind, i, item.checked, &item.blocks, 0, &mut paras);
                    self.text_box(paras, BoxStyle::body());
                }
            }
            Block::Table(rows) => self.table(rows),
            Block::Code { text, .. } => self.text_box(mono_paras(text, 0), BoxStyle::mono()),
            Block::Math(source) | Block::Mermaid(source) => {
                self.text_box(mono_paras(source, 0), BoxStyle::mono())
            }
            Block::Blockquote(blocks) => {
                let mut paras = Vec::new();
                flatten(blocks, 0, &mut paras);
                self.text_box(paras, BoxStyle::indented(QUOTE_BAR, None));
            }
            Block::Callout { kind, blocks } => {
                let mut paras = Vec::new();
                flatten(blocks, 0, &mut paras);
                let (bar, fill) = callout_color(kind);
                self.text_box(paras, BoxStyle::indented(bar, Some(fill)));
            }
            Block::Attachment { .. } | Block::Embed { .. } => {
                let mut paras = Vec::new();
                flatten(std::slice::from_ref(block), 0, &mut paras);
                self.text_box(paras, BoxStyle::body());
            }
            // Source: only a top-level rule opens a slide (`write_pptx`).
            Block::HorizontalRule => {}
            Block::Details { summary, blocks } => {
                let mut p = Para::inlines(summary, 0);
                p.bold = true;
                self.text_box(vec![p], BoxStyle::body());
                for b in blocks {
                    self.block(b);
                }
            }
        }
    }

    /// Source `addBody`: skips empty text, opens a slide when less than
    /// `MIN_H` (or a heading's fixed height) is left, advances the cursor by
    /// the `estimateH` height. Text taller than the rest of the slide
    /// continues in a box on the next slide (source: shrunk into the rest).
    fn text_box(&mut self, paras: Vec<Para>, style: BoxStyle) {
        let mut queue: VecDeque<Para> = paras.into_iter().filter(|p| !p.is_empty()).collect();
        if queue.is_empty() {
            return;
        }
        if let Some(h) = style.fixed {
            if self.remaining() < h {
                self.new_slide();
            }
            self.place(queue.make_contiguous(), style, h);
            self.y += h + GAP;
            return;
        }
        let cpl = cpl(style.size, style.w);
        let line = line_h(style.size);
        loop {
            if self.remaining() < MIN_H {
                self.new_slide();
            }
            let rest = self.remaining();
            let fit = ((rest / line).floor() as usize).max(1);
            let total: usize = queue.iter().map(|p| p.lines(cpl)).sum();
            if total <= fit {
                let h = (total as f64 * line).max(MIN_H).min(rest);
                self.place(queue.make_contiguous(), style, h);
                self.y += h + GAP;
                return;
            }
            let mut head = Vec::new();
            let mut used = 0;
            while let Some(p) = queue.pop_front() {
                let n = p.lines(cpl);
                if used + n <= fit {
                    used += n;
                    head.push(p);
                    continue;
                }
                if fit > used {
                    let (a, b) = p.split(fit - used, cpl);
                    if !a.is_empty() {
                        head.push(a);
                    }
                    if !b.is_empty() {
                        queue.push_front(b);
                    }
                } else {
                    queue.push_front(p);
                }
                break;
            }
            if !head.is_empty() {
                self.place(&head, style, rest);
            }
            self.y = SLIDE_H - BOTTOM;
            if queue.is_empty() {
                return;
            }
        }
    }

    fn place(&mut self, paras: &[Para], style: BoxStyle, h: f64) {
        let y = self.y;
        let slide = self.slide();
        if let Some(bar) = style.bar {
            let id = slide.id();
            rect(&mut slide.shapes, id, X, y, 0.03, h, bar);
        }
        let id = slide.id();
        let mut body = String::new();
        for p in paras {
            paragraph(&mut body, slide, p, style.size);
        }
        let s = &mut slide.shapes;
        let _ = write!(
            s,
            r#"<p:sp><p:nvSpPr><p:cNvPr id="{id}" name="Text {id}"/><p:cNvSpPr txBox="1"/><p:nvPr/></p:nvSpPr><p:spPr>{xfrm}<a:prstGeom prst="rect"><a:avLst/></a:prstGeom>{fill}</p:spPr><p:txBody><a:bodyPr wrap="square" lIns="91440" tIns="45720" rIns="91440" bIns="45720" rtlCol="0" anchor="t"><a:normAutofit/></a:bodyPr><a:lstStyle/>{body}</p:txBody></p:sp>"#,
            xfrm = xfrm(style.x, y, style.w, h),
            fill = match style.fill {
                Some(c) => format!(r#"<a:solidFill><a:srgbClr val="{c}"/></a:solidFill>"#),
                None => "<a:noFill/>".into(),
            },
        );
    }

    /// Source `addTable` (12 pt cells, rows at least 0.32 in). Rows that do
    /// not fit the rest of the slide continue in a table on the next slide,
    /// repeating a header row (source: one table, drawn past the slide).
    fn table(&mut self, rows: &[TableRow]) {
        if rows.is_empty() {
            return;
        }
        let cols = rows.iter().map(|r| r.cells.len()).max().unwrap_or(0).max(1);
        let col_w = W / cols as f64;
        let cpl = cpl(TABLE_SIZE, (col_w - 2.0 * CELL_PAD_X).max(0.1));
        let cells: Vec<Vec<(Vec<Para>, bool)>> = rows
            .iter()
            .map(|r| {
                (0..cols)
                    .map(|i| match r.cells.get(i) {
                        Some(c) => {
                            let mut paras = Vec::new();
                            flatten(&c.blocks, 0, &mut paras);
                            (paras, c.header)
                        }
                        None => (Vec::new(), false),
                    })
                    .collect()
            })
            .collect();
        let heights: Vec<f64> = cells
            .iter()
            .map(|row| {
                let lines = row
                    .iter()
                    .map(|(paras, _)| paras.iter().map(|p| p.lines(cpl)).sum::<usize>().max(1))
                    .max()
                    .unwrap_or(1);
                (lines as f64 * line_h(TABLE_SIZE) + 2.0 * CELL_PAD_Y).max(ROW_H)
            })
            .collect();
        let header = cells[0].iter().all(|(_, h)| *h) && cells.len() > 1;
        let mut next = 0;
        while next < cells.len() {
            if self.remaining() < heights[next].min(SLIDE_H - TOP - BOTTOM) {
                self.new_slide();
            }
            let mut chunk: Vec<usize> = Vec::new();
            let mut h = 0.0;
            if header && next > 0 {
                chunk.push(0);
                h += heights[0];
            }
            let first = chunk.len();
            while next < cells.len() {
                let rh = heights[next];
                if chunk.len() > first && h + rh > self.remaining() {
                    break;
                }
                chunk.push(next);
                h += rh;
                next += 1;
            }
            let y = self.y;
            let slide = self.slide();
            let id = slide.id();
            let mut xml = String::new();
            let _ = write!(
                xml,
                r#"<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="{id}" name="Table {id}"/><p:cNvGraphicFramePr><a:graphicFrameLocks noGrp="1"/></p:cNvGraphicFramePr><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="{x}" y="{y}"/><a:ext cx="{cx}" cy="{cy}"/></p:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/table"><a:tbl><a:tblPr firstRow="{fr}" bandRow="0"/><a:tblGrid>"#,
                x = emu(X),
                y = emu(y),
                cx = emu(W),
                cy = emu(h),
                fr = u8::from(header),
            );
            for _ in 0..cols {
                let _ = write!(xml, r#"<a:gridCol w="{}"/>"#, emu(col_w));
            }
            xml.push_str("</a:tblGrid>");
            for &r in &chunk {
                let _ = write!(xml, r#"<a:tr h="{}">"#, emu(heights[r]));
                for (paras, is_header) in &cells[r] {
                    xml.push_str("<a:tc><a:txBody><a:bodyPr/><a:lstStyle/>");
                    let paras: Vec<Para> = paras
                        .iter()
                        .cloned()
                        .map(|mut p| {
                            p.bold |= *is_header;
                            p
                        })
                        .collect();
                    if paras.is_empty() {
                        empty_paragraph(&mut xml, TABLE_SIZE);
                    }
                    for p in &paras {
                        paragraph(&mut xml, slide, p, TABLE_SIZE);
                    }
                    xml.push_str("</a:txBody><a:tcPr>");
                    for side in ["lnL", "lnR", "lnT", "lnB"] {
                        let _ = write!(
                            xml,
                            r#"<a:{side} w="12700"><a:solidFill><a:srgbClr val="{RULE}"/></a:solidFill></a:{side}>"#
                        );
                    }
                    if *is_header {
                        let _ = write!(
                            xml,
                            r#"<a:solidFill><a:srgbClr val="{HEADER_FILL}"/></a:solidFill>"#
                        );
                    }
                    xml.push_str("</a:tcPr></a:tc>");
                }
                xml.push_str("</a:tr>");
            }
            xml.push_str("</a:tbl></a:graphicData></a:graphic></p:graphicFrame>");
            slide.shapes.push_str(&xml);
            self.y += h + TABLE_GAP;
        }
    }
}

fn callout_color(kind: &str) -> (&'static str, &'static str) {
    match kind {
        "tip" | "success" => ("2F9E44", "EDF8EF"),
        "warning" | "caution" => ("D97A06", "FDF5E6"),
        "danger" | "error" => ("C92A2A", "FCEDED"),
        "note" | "info" => ("1A56DB", "EBF2FD"),
        _ => ("868E96", "F3F4F6"),
    }
}

/// One paragraph per source line (code, math and Mermaid source).
fn mono_paras(text: &str, level: usize) -> Vec<Para> {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut paras: Vec<Para> = text
        .split('\n')
        .map(|line| {
            let mut p = Para::new(level);
            p.mono = true;
            let line = clean_keep_tabs(line);
            // An empty line keeps its row (space), unlike empty paragraphs.
            p.runs.push(Run::Text {
                text: if line.is_empty() { " ".into() } else { line },
                marks: Marks::default(),
            });
            p
        })
        .collect();
    while paras
        .last()
        .is_some_and(|p| matches!(p.runs.as_slice(), [Run::Text { text, .. }] if text == " "))
    {
        paras.pop();
    }
    paras
}

/// A list item's blocks: the first paragraph carries the bullet/number (or
/// the task box), later blocks continue at the item's indent.
fn list_item(
    kind: ListKind,
    index: usize,
    checked: Option<bool>,
    blocks: &[Block],
    level: usize,
    out: &mut Vec<Para>,
) {
    let start = out.len();
    flatten_item(blocks, level, out);
    let bullet = match kind {
        ListKind::Bullet => Bullet::Dot,
        ListKind::Ordered => Bullet::Number(index + 1),
        ListKind::Task => Bullet::None,
    };
    match out.get_mut(start) {
        Some(first) if first.level == level.min(MAX_LEVEL) && first.bullet == Bullet::None => {
            first.bullet = bullet;
            if let Some(done) = checked {
                let mark = if done { "\u{2611} " } else { "\u{2610} " };
                first.runs.insert(
                    0,
                    Run::Text {
                        text: mark.into(),
                        marks: Marks::default(),
                    },
                );
            }
        }
        _ => {
            // Empty item or one that starts with a nested list: a marker
            // paragraph of its own.
            let mut p = Para::new(level);
            p.bullet = bullet;
            if let Some(done) = checked {
                p.push(if done { "\u{2611}" } else { "\u{2610}" }, Marks::default());
            } else {
                p.push("\u{00a0}", Marks::default());
            }
            out.insert(start, p);
        }
    }
}

/// Item content: nested lists go one level deeper, other blocks stay at the
/// item's level (their indent without a marker).
fn flatten_item(blocks: &[Block], level: usize, out: &mut Vec<Para>) {
    for b in blocks {
        if let Block::List { kind, items } = b {
            for (i, item) in items.iter().enumerate() {
                list_item(*kind, i, item.checked, &item.blocks, level + 1, out);
            }
        } else {
            flatten(std::slice::from_ref(b), level, out);
        }
    }
}

/// Blocks inside one text box (quotes, callouts, list items, table cells):
/// one paragraph per block; nested lists and quotes indent a level.
fn flatten(blocks: &[Block], level: usize, out: &mut Vec<Para>) {
    for b in blocks {
        match b {
            Block::Paragraph(inlines) => out.push(Para::inlines(inlines, level)),
            Block::Heading { inlines, .. } => {
                let mut p = Para::inlines(inlines, level);
                p.bold = true;
                out.push(p);
            }
            Block::List { kind, items } => {
                for (i, item) in items.iter().enumerate() {
                    list_item(*kind, i, item.checked, &item.blocks, level, out);
                }
            }
            Block::Table(rows) => {
                // A table inside a box: one line per row, cells joined.
                for row in rows {
                    let texts: Vec<String> = row
                        .cells
                        .iter()
                        .map(|c| {
                            let mut paras = Vec::new();
                            flatten(&c.blocks, 0, &mut paras);
                            paras.iter().map(plain).collect::<Vec<_>>().join(" ")
                        })
                        .collect();
                    out.push(Para::text(&texts.join(" | "), level));
                }
            }
            Block::Code { text, .. } | Block::Math(text) | Block::Mermaid(text) => {
                out.extend(mono_paras(text, level));
            }
            Block::Blockquote(blocks) | Block::Callout { blocks, .. } => {
                flatten(blocks, level + 1, out);
            }
            Block::Attachment { name, .. } => out.push(Para::text(name, level)),
            Block::Embed { entity, reference } => {
                if reference.is_empty() {
                    continue;
                }
                if entity == "url" {
                    let marks = Marks {
                        link: Some(reference.clone()),
                        ..Marks::default()
                    };
                    let mut p = Para::new(level);
                    p.push(reference, marks);
                    out.push(p);
                } else {
                    let entity = if entity == "document" { "doc" } else { entity };
                    out.push(Para::text(&format!("[[{entity}:{reference}]]"), level));
                }
            }
            Block::HorizontalRule => {}
            Block::Details { summary, blocks } => {
                let mut p = Para::inlines(summary, level);
                p.bold = true;
                out.push(p);
                flatten(blocks, level, out);
            }
        }
    }
}

fn plain(p: &Para) -> String {
    let mut s = String::new();
    for r in &p.runs {
        match r {
            Run::Text { text, .. } => s.push_str(text),
            Run::Break => s.push(' '),
        }
    }
    s
}

// ---------------------------------------------------------------------------
// XML

fn emu(inches: f64) -> i64 {
    (inches * EMU_PER_INCH).round() as i64
}

fn xfrm(x: f64, y: f64, w: f64, h: f64) -> String {
    format!(
        r#"<a:xfrm><a:off x="{}" y="{}"/><a:ext cx="{}" cy="{}"/></a:xfrm>"#,
        emu(x),
        emu(y),
        emu(w),
        emu(h)
    )
}

fn rect(out: &mut String, id: u32, x: f64, y: f64, w: f64, h: f64, color: &str) {
    let _ = write!(
        out,
        r#"<p:sp><p:nvSpPr><p:cNvPr id="{id}" name="Bar {id}"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr>{xfrm}<a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val="{color}"/></a:solidFill><a:ln><a:noFill/></a:ln></p:spPr></p:sp>"#,
        xfrm = xfrm(x, y, w, h),
    );
}

fn size_attr(size: f64) -> i64 {
    (size * 100.0).round() as i64
}

fn fonts(mono: bool) -> String {
    let face = if mono { MONO_FONT } else { BODY_FONT };
    format!(r#"<a:latin typeface="{face}"/><a:ea typeface="{face}"/><a:cs typeface="{face}"/>"#)
}

fn empty_paragraph(out: &mut String, size: f64) {
    let _ = write!(
        out,
        r#"<a:p><a:endParaRPr lang="ko-KR" sz="{}" dirty="0"/></a:p>"#,
        size_attr(size)
    );
}

fn paragraph(out: &mut String, slide: &mut Slide, p: &Para, size: f64) {
    let sz = size_attr(size);
    out.push_str("<a:p>");
    let indent = LEVEL_EMU * p.level as i64;
    match p.bullet {
        Bullet::None => {
            let _ = write!(
                out,
                r#"<a:pPr marL="{indent}" lvl="{}" indent="0"><a:buNone/></a:pPr>"#,
                p.level
            );
        }
        Bullet::Dot => {
            let _ = write!(
                out,
                r#"<a:pPr marL="{}" lvl="{}" indent="-{LEVEL_EMU}"><a:buFont typeface="Arial"/><a:buChar char="&#8226;"/></a:pPr>"#,
                indent + LEVEL_EMU,
                p.level
            );
        }
        Bullet::Number(n) => {
            let _ = write!(
                out,
                r#"<a:pPr marL="{}" lvl="{}" indent="-{LEVEL_EMU}"><a:buFont typeface="+mj-lt"/><a:buAutoNum type="arabicPeriod" startAt="{n}"/></a:pPr>"#,
                indent + LEVEL_EMU,
                p.level
            );
        }
    }
    for run in &p.runs {
        match run {
            Run::Break => {
                let _ = write!(
                    out,
                    r#"<a:br><a:rPr lang="ko-KR" sz="{sz}" dirty="0"/></a:br>"#
                );
            }
            Run::Text { text, marks } => {
                let bold = p.bold || marks.bold;
                let _ = write!(out, r#"<a:r><a:rPr lang="ko-KR" sz="{sz}""#);
                if bold {
                    out.push_str(r#" b="1""#);
                }
                if marks.italic {
                    out.push_str(r#" i="1""#);
                }
                if marks.underline {
                    out.push_str(r#" u="sng""#);
                }
                if marks.strike {
                    out.push_str(r#" strike="sngStrike""#);
                }
                out.push_str(r#" dirty="0">"#);
                if marks.highlight {
                    let _ = write!(
                        out,
                        r#"<a:highlight><a:srgbClr val="{HIGHLIGHT}"/></a:highlight>"#
                    );
                }
                out.push_str(&fonts(p.mono || marks.code));
                if let Some(target) = marks.link.as_deref().and_then(hyperlink_target) {
                    let rid = slide.link(target);
                    let _ = write!(out, r#"<a:hlinkClick r:id="{rid}"/>"#);
                }
                let _ = write!(out, "</a:rPr><a:t>{}</a:t></a:r>", esc(text));
            }
        }
    }
    let _ = write!(
        out,
        r#"<a:endParaRPr lang="ko-KR" sz="{sz}" dirty="0"/></a:p>"#
    );
}

/// XML 1.0 forbids most C0 controls (and U+FFFE/U+FFFF) even escaped, and the
/// stored JSON may hold them: NUL reads as U+FFFD, the others are dropped;
/// tabs read as one space (line breaks were split off by the caller).
fn clean(text: &str) -> String {
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

/// `clean` for one source line of code: tabs stay.
fn clean_keep_tabs(line: &str) -> String {
    line.split('\t').map(clean).collect::<Vec<_>>().join("\t")
}

fn esc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Package

const XML_HEAD: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#;
const NS: &str = r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main""#;
const REL_NS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const EMPTY_TREE: &str = r#"<p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/><a:chOff x="0" y="0"/><a:chExt cx="0" cy="0"/></a:xfrm></p:grpSpPr>"#;

fn pack(title: Option<&str>, slides: &[Slide]) -> docx_zip::result::ZipResult<Vec<u8>> {
    use docx_zip::write::SimpleFileOptions;
    let mut zip = docx_zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options =
        SimpleFileOptions::default().compression_method(docx_zip::CompressionMethod::Deflated);
    let part = |zip: &mut docx_zip::ZipWriter<Cursor<Vec<u8>>>,
                name: &str,
                body: &str|
     -> docx_zip::result::ZipResult<()> {
        zip.start_file(name, options)?;
        zip.write_all(XML_HEAD.as_bytes())?;
        zip.write_all(body.as_bytes())?;
        Ok(())
    };

    let mut types = String::from(
        r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/><Override PartName="/ppt/slideMasters/slideMaster1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slideMaster+xml"/><Override PartName="/ppt/slideLayouts/slideLayout1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slideLayout+xml"/><Override PartName="/ppt/theme/theme1.xml" ContentType="application/vnd.openxmlformats-officedocument.theme+xml"/><Override PartName="/ppt/presProps.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presProps+xml"/><Override PartName="/ppt/viewProps.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.viewProps+xml"/><Override PartName="/ppt/tableStyles.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.tableStyles+xml"/><Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/><Override PartName="/docProps/app.xml" ContentType="application/vnd.openxmlformats-officedocument.extended-properties+xml"/>"#,
    );
    for i in 1..=slides.len() {
        let _ = write!(
            types,
            r#"<Override PartName="/ppt/slides/slide{i}.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/>"#
        );
    }
    types.push_str("</Types>");
    part(&mut zip, "[Content_Types].xml", &types)?;

    part(
        &mut zip,
        "_rels/.rels",
        &format!(
            r#"<Relationships xmlns="{REL_NS}"><Relationship Id="rId1" Type="{REL}/officeDocument" Target="ppt/presentation.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/><Relationship Id="rId3" Type="{REL}/extended-properties" Target="docProps/app.xml"/></Relationships>"#
        ),
    )?;
    let title_xml = title
        .map(|t| format!("<dc:title>{}</dc:title>", esc(&clean(t))))
        .unwrap_or_default();
    part(
        &mut zip,
        "docProps/core.xml",
        &format!(
            r#"<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/" xmlns:dcmitype="http://purl.org/dc/dcmitype/" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">{title_xml}<dc:creator>FVOCI</dc:creator></cp:coreProperties>"#
        ),
    )?;
    part(
        &mut zip,
        "docProps/app.xml",
        &format!(
            r#"<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties" xmlns:vt="http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes"><Application>FVOCI</Application><Slides>{}</Slides></Properties>"#,
            slides.len()
        ),
    )?;

    let mut pres = format!(
        r#"<p:presentation {NS} saveSubsetFonts="1"><p:sldMasterIdLst><p:sldMasterId id="2147483648" r:id="rId1"/></p:sldMasterIdLst><p:sldIdLst>"#
    );
    let mut pres_rels = format!(
        r#"<Relationships xmlns="{REL_NS}"><Relationship Id="rId1" Type="{REL}/slideMaster" Target="slideMasters/slideMaster1.xml"/><Relationship Id="rId2" Type="{REL}/theme" Target="theme/theme1.xml"/><Relationship Id="rId3" Type="{REL}/presProps" Target="presProps.xml"/><Relationship Id="rId4" Type="{REL}/viewProps" Target="viewProps.xml"/><Relationship Id="rId5" Type="{REL}/tableStyles" Target="tableStyles.xml"/>"#
    );
    for i in 1..=slides.len() {
        let _ = write!(pres, r#"<p:sldId id="{}" r:id="rId{}"/>"#, 255 + i, 5 + i);
        let _ = write!(
            pres_rels,
            r#"<Relationship Id="rId{}" Type="{REL}/slide" Target="slides/slide{i}.xml"/>"#,
            5 + i
        );
    }
    let _ = write!(
        pres,
        r#"</p:sldIdLst><p:sldSz cx="{}" cy="{}"/><p:notesSz cx="6858000" cy="9144000"/></p:presentation>"#,
        emu(SLIDE_W),
        emu(SLIDE_H)
    );
    pres_rels.push_str("</Relationships>");
    part(&mut zip, "ppt/presentation.xml", &pres)?;
    part(&mut zip, "ppt/_rels/presentation.xml.rels", &pres_rels)?;
    part(
        &mut zip,
        "ppt/presProps.xml",
        &format!("<p:presentationPr {NS}/>"),
    )?;
    part(
        &mut zip,
        "ppt/viewProps.xml",
        &format!(r#"<p:viewPr {NS}><p:gridSpacing cx="76200" cy="76200"/></p:viewPr>"#),
    )?;
    part(
        &mut zip,
        "ppt/tableStyles.xml",
        r#"<a:tblStyleLst xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" def="{5C22544A-7EE6-4342-B048-85BDC9FD1C3A}"/>"#,
    )?;
    part(
        &mut zip,
        "ppt/slideMasters/slideMaster1.xml",
        &format!(
            r#"<p:sldMaster {NS}><p:cSld><p:bg><p:bgRef idx="1001"><a:schemeClr val="bg1"/></p:bgRef></p:bg><p:spTree>{EMPTY_TREE}</p:spTree></p:cSld><p:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/><p:sldLayoutIdLst><p:sldLayoutId id="2147483649" r:id="rId1"/></p:sldLayoutIdLst></p:sldMaster>"#
        ),
    )?;
    part(
        &mut zip,
        "ppt/slideMasters/_rels/slideMaster1.xml.rels",
        &format!(
            r#"<Relationships xmlns="{REL_NS}"><Relationship Id="rId1" Type="{REL}/slideLayout" Target="../slideLayouts/slideLayout1.xml"/><Relationship Id="rId2" Type="{REL}/theme" Target="../theme/theme1.xml"/></Relationships>"#
        ),
    )?;
    part(
        &mut zip,
        "ppt/slideLayouts/slideLayout1.xml",
        &format!(
            r#"<p:sldLayout {NS} type="blank" preserve="1"><p:cSld name="Blank"><p:spTree>{EMPTY_TREE}</p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sldLayout>"#
        ),
    )?;
    part(
        &mut zip,
        "ppt/slideLayouts/_rels/slideLayout1.xml.rels",
        &format!(
            r#"<Relationships xmlns="{REL_NS}"><Relationship Id="rId1" Type="{REL}/slideMaster" Target="../slideMasters/slideMaster1.xml"/></Relationships>"#
        ),
    )?;
    part(&mut zip, "ppt/theme/theme1.xml", &theme())?;

    for (i, slide) in slides.iter().enumerate() {
        let n = i + 1;
        part(
            &mut zip,
            &format!("ppt/slides/slide{n}.xml"),
            &format!(
                r#"<p:sld {NS}><p:cSld><p:spTree>{EMPTY_TREE}{}</p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sld>"#,
                slide.shapes
            ),
        )?;
        let mut rels = format!(
            r#"<Relationships xmlns="{REL_NS}"><Relationship Id="rId1" Type="{REL}/slideLayout" Target="../slideLayouts/slideLayout1.xml"/>"#
        );
        for (j, target) in slide.links.iter().enumerate() {
            let _ = write!(
                rels,
                r#"<Relationship Id="rId{}" Type="{REL}/hyperlink" Target="{}" TargetMode="External"/>"#,
                j + 2,
                esc(target)
            );
        }
        rels.push_str("</Relationships>");
        part(
            &mut zip,
            &format!("ppt/slides/_rels/slide{n}.xml.rels"),
            &rels,
        )?;
    }
    Ok(zip.finish()?.into_inner())
}

/// A minimal complete theme (a slide master requires one): Office colors,
/// the body font as major/minor Latin and East Asian face, and the three
/// entries each format-scheme list must hold.
fn theme() -> String {
    let colors = [
        ("dk1", "000000"),
        ("lt1", "FFFFFF"),
        ("dk2", "44546A"),
        ("lt2", "E7E6E6"),
        ("accent1", "4472C4"),
        ("accent2", "ED7D31"),
        ("accent3", "A5A5A5"),
        ("accent4", "FFC000"),
        ("accent5", "5B9BD5"),
        ("accent6", "70AD47"),
        ("hlink", "0563C1"),
        ("folHlink", "954F72"),
    ];
    let mut scheme = String::new();
    for (name, rgb) in colors {
        let _ = write!(scheme, r#"<a:{name}><a:srgbClr val="{rgb}"/></a:{name}>"#);
    }
    let font = format!(
        r#"<a:latin typeface="{BODY_FONT}"/><a:ea typeface="{BODY_FONT}"/><a:cs typeface=""/>"#
    );
    let fill = r#"<a:solidFill><a:schemeClr val="phClr"/></a:solidFill>"#;
    let line = r#"<a:ln w="9525"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln>"#;
    let effect = "<a:effectStyle><a:effectLst/></a:effectStyle>";
    format!(
        r#"<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" name="FVOCI"><a:themeElements><a:clrScheme name="FVOCI">{scheme}</a:clrScheme><a:fontScheme name="FVOCI"><a:majorFont>{font}</a:majorFont><a:minorFont>{font}</a:minorFont></a:fontScheme><a:fmtScheme name="FVOCI"><a:fillStyleLst>{fill}{fill}{fill}</a:fillStyleLst><a:lnStyleLst>{line}{line}{line}</a:lnStyleLst><a:effectStyleLst>{effect}{effect}{effect}</a:effectStyleLst><a:bgFillStyleLst>{fill}{fill}{fill}</a:bgFillStyleLst></a:fmtScheme></a:themeElements><a:objectDefaults/><a:extraClrSchemeLst/></a:theme>"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::documents::export_model::export_doc;
    use serde_json::json;

    fn parts(bytes: &[u8]) -> Vec<(String, String)> {
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        (0..zip.len())
            .map(|i| {
                let mut f = zip.by_index(i).unwrap();
                let mut s = String::new();
                std::io::Read::read_to_string(&mut f, &mut s).unwrap();
                (f.name().to_string(), s)
            })
            .collect()
    }

    fn slide(parts: &[(String, String)], n: usize) -> &str {
        let name = format!("ppt/slides/slide{n}.xml");
        &parts.iter().find(|(p, _)| *p == name).unwrap().1
    }

    fn texts(xml: &str) -> Vec<String> {
        xml.split("<a:t>")
            .skip(1)
            .map(|s| s.split("</a:t>").next().unwrap().to_string())
            .collect()
    }

    #[test]
    fn title_slides_rules_and_parts() {
        let doc = json!({"type": "doc", "content": [
            {"type": "heading", "attrs": {"level": 2}, "content": [{"type": "text", "text": "H"}]},
            {"type": "paragraph", "content": [{"type": "text", "text": "a<&>\"", "marks": [{"type": "bold"},
                {"type": "link", "attrs": {"href": "https://x.test/a b"}}]}]},
            {"type": "horizontalRule"},
            {"type": "paragraph", "content": [{"type": "text", "text": "next\u{1}\u{0}"}]},
            {"type": "blockquote", "content": [{"type": "horizontalRule"}]}
        ]});
        let bytes = write_pptx(&export_doc("제목", &doc)).unwrap();
        let parts = parts(&bytes);
        assert!(parts.iter().all(|(n, _)| !n.ends_with('/')));
        assert_eq!(
            parts
                .iter()
                .filter(|(n, _)| n.starts_with("ppt/slides/slide"))
                .count(),
            2
        );
        let s1 = slide(&parts, 1);
        assert_eq!(texts(s1), ["제목", "H", "a&lt;&amp;&gt;&quot;"]);
        // The title is the first heading (28 pt), the next one 16 pt.
        assert!(s1.contains(r#"sz="2800" b="1""#) && s1.contains(r#"sz="1600" b="1""#));
        assert!(s1.contains(r#"<a:hlinkClick r:id="rId2"/>"#));
        let rels = &parts
            .iter()
            .find(|(n, _)| n == "ppt/slides/_rels/slide1.xml.rels")
            .unwrap()
            .1;
        assert!(rels.contains(r#"Target="https://x.test/a%20b" TargetMode="External""#));
        assert_eq!(texts(slide(&parts, 2)), ["next\u{FFFD}"]);
        let core = &parts
            .iter()
            .find(|(n, _)| n == "docProps/core.xml")
            .unwrap()
            .1;
        assert!(core.contains("<dc:title>제목</dc:title>"));
    }

    #[test]
    fn lists_have_levels_numbers_and_task_boxes() {
        let item = |t: &str, extra: Vec<serde_json::Value>| {
            let mut c =
                vec![json!({"type": "paragraph", "content": [{"type": "text", "text": t}]})];
            c.extend(extra);
            json!({"type": "listItem", "content": c})
        };
        let doc = json!({"type": "doc", "content": [
            {"type": "orderedList", "content": [item("one", vec![]), item("two", vec![
                json!({"type": "bulletList", "content": [item("nested", vec![])]})])]},
            {"type": "taskList", "content": [
                {"type": "taskItem", "attrs": {"checked": true}, "content": [{"type": "paragraph", "content": [{"type": "text", "text": "done"}]}]}]}
        ]});
        let bytes = write_pptx(&export_doc("", &doc)).unwrap();
        let s1 = slide(&parts(&bytes), 1).to_string();
        assert_eq!(texts(&s1), ["one", "two", "nested", "\u{2611} ", "done"]);
        assert!(s1.contains(r#"startAt="1""#) && s1.contains(r#"startAt="2""#));
        assert!(s1.contains(r#"lvl="1" indent="-285750"><a:buFont typeface="Arial"/><a:buChar"#));
    }

    #[test]
    fn long_text_continues_on_the_next_slide_and_tables_split() {
        let para = json!({"type": "paragraph", "content": [{"type": "text", "text": "가".repeat(46 * 30)}]});
        let cell = |t: &str, h: bool| {
            json!({"type": if h {"tableHeader"} else {"tableCell"},
            "content": [{"type": "paragraph", "content": [{"type": "text", "text": t}]}]})
        };
        let mut rows =
            vec![json!({"type": "tableRow", "content": [cell("A", true), cell("B", true)]})];
        for i in 0..30 {
            rows.push(json!({"type": "tableRow", "content": [cell(&i.to_string(), false)]}));
        }
        let doc = json!({"type": "doc", "content": [para, {"type": "table", "content": rows}]});
        let bytes = write_pptx(&export_doc("", &doc)).unwrap();
        let parts = parts(&bytes);
        let slides = parts
            .iter()
            .filter(|(n, _)| n.starts_with("ppt/slides/slide"))
            .count();
        assert!(slides >= 4, "{slides}");
        let all: String = (1..=slides)
            .map(|n| texts(slide(&parts, n)).concat())
            .collect();
        assert_eq!(all.matches('가').count(), 46 * 30);
        // Every table part repeats the header row; ragged rows are padded.
        let tables: usize = (1..=slides)
            .map(|n| slide(&parts, n).matches("<a:tbl>").count())
            .sum();
        let headers: usize = (1..=slides)
            .map(|n| texts(slide(&parts, n)).iter().filter(|t| *t == "A").count())
            .sum();
        assert!(tables >= 2);
        assert_eq!(tables, headers);
        for n in 1..=slides {
            let s = slide(&parts, n);
            assert_eq!(s.matches("<a:tc>").count() % 2, 0);
        }
    }

    #[test]
    fn empty_doc_is_one_empty_slide() {
        let bytes = write_pptx(&export_doc("", &json!({"type": "doc"}))).unwrap();
        let parts = parts(&bytes);
        assert!(texts(slide(&parts, 1)).is_empty());
        let core = &parts
            .iter()
            .find(|(n, _)| n == "docProps/core.xml")
            .unwrap()
            .1;
        assert!(!core.contains("dc:title"));
    }
}
