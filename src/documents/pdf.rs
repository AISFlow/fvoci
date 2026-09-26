//! Export model -> PDF (source `export/pdf.tsx`, `@react-pdf/renderer`).
//!
//! krilla writes the file and subsets the embedded fonts; this module does
//! the flow the React PDF tree did: A4 pages with the TS paddings, 12 pt text
//! at line height 1.5, blocks stacked with the TS margins, lines broken at
//! UAX #14 opportunities, pages broken between lines and table rows (a row
//! taller than the space left is split between its lines).
//!
//! Text is shaped per run with rustybuzz. Each character takes the first
//! font of its run's chain that has a glyph (body: Noto Sans KR -> Noto Sans
//! Mono CJK KR -> Noto Emoji; code: Mono first); pictographs try Noto Emoji
//! first as the TS `EMOJI_RE` runs did, and combining marks, joiners and
//! variation selectors stay with the character before them. The fonts are
//! the files `scripts/document-convert` embeds (`packages/editor/src/fonts`,
//! SIL OFL 1.1), compiled into this binary so the child needs no path or
//! environment. Noto Sans KR is a variable font: text uses its `wght` 400
//! instance and bold (headings, the bold mark) 700.
//!
//! Beyond the TS output (see the export report for the table): marks are
//! drawn (bold, synthetic oblique italic, underline, strike, highlight, code
//! font), links get URI annotations for absolute http(s)/mailto targets,
//! task items a checkbox, code blocks a background, callouts a coloured bar
//! and background, details their summary, embeds a `[[entity:ref]]` text.
//! No attachment bytes are read and nothing is fetched.

use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;

use krilla::action::{Action, LinkAction};
use krilla::annotation::{Annotation, LinkAnnotation, Target};
use krilla::color::rgb;
use krilla::geom::{PathBuilder, Point, Rect, Transform};
use krilla::metadata::Metadata;
use krilla::page::PageSettings;
use krilla::paint::{Fill, Stroke};
use krilla::surface::Surface;
use krilla::text::{Font, GlyphId, KrillaGlyph, Tag};
use krilla::Document;
use unicode_linebreak::{linebreaks, BreakOpportunity};

use crate::documents::docx::hyperlink_target;
use crate::documents::export_model::{Block, ExportDoc, Inline, ListItem, ListKind, TableRow};

/// Source `limits.ts` / `convert.mjs` `MAX_OUTPUT_BYTES`.
pub const PDF_MAX_OUTPUT_BYTES: usize = 20_000_000;
pub const PDF_CONTENT_TYPE: &str = "application/pdf";

/// Font files are read at run time (child op only), never compiled into
/// `fvoci-server`: ~34 MB of `include_bytes!` would count against RLIMIT_AS
/// in every same-binary child (the image preview child broke at 96 MiB).
/// `FVOCI_EXPORT_FONT_DIR` points at the shipped directory; the default is
/// the repository copy for development and tests.
pub const FONT_DIR_ENV: &str = "FVOCI_EXPORT_FONT_DIR";
const SANS_FILE: &str = "NotoSansKR.ttf";
const MONO_FILE: &str = "NotoSansMonoCJKkr.ttf";
const EMOJI_FILE: &str = "NotoEmoji.ttf";

fn font_dir() -> std::path::PathBuf {
    std::env::var_os(FONT_DIR_ENV)
        .filter(|v| !v.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/packages/editor/src/fonts"
            ))
        })
}

/// Reads one font file once per process; the bytes live for the process
/// (the export child is short-lived) because the shaper borrows them.
fn font_file(name: &str) -> Option<&'static [u8]> {
    let path = font_dir().join(name);
    match std::fs::read(&path) {
        Ok(bytes) => Some(Box::leak(bytes.into_boxed_slice())),
        Err(error) => {
            tracing::error!(path = %path.display(), %error, "pdf export font unavailable");
            None
        }
    }
}

// Source `pdf.tsx` page and block styles (points).
const PAGE_W: f32 = 595.28;
const PAGE_H: f32 = 841.89;
const PAD_TOP: f32 = 35.0;
const PAD_BOTTOM: f32 = 65.0;
const PAD_X: f32 = 35.0;
const CONTENT_TOP: f32 = PAD_TOP;
const CONTENT_BOTTOM: f32 = PAGE_H - PAD_BOTTOM;
const BODY_SIZE: f32 = 12.0;
const MONO_SIZE: f32 = 10.0;
const LINE_HEIGHT: f32 = 1.5;
const PARAGRAPH_GAP: f32 = 6.0;
const HEADING_GAP: f32 = 8.0;
const LIST_GAP: f32 = 6.0;
const ITEM_GAP: f32 = 2.0;
const MARKER_W: f32 = 18.0;
const TABLE_GAP: f32 = 8.0;
const CELL_BORDER: f32 = 1.0;
const CELL_PAD: f32 = 4.0;
const QUOTE_BAR: f32 = 2.0;
const QUOTE_PAD: f32 = 8.0;
const RULE_GAP: f32 = 8.0;
const CODE_PAD: f32 = 6.0;
/// Narrowest text column: deeper nesting keeps this width (and runs past
/// the right margin) instead of breaking every character onto its own line.
const MIN_WIDTH: f32 = 36.0;

const BLACK: Rgb = (0, 0, 0);
const GRAY_TEXT: Rgb = (0x55, 0x55, 0x55);
const LINK: Rgb = (0x1a, 0x56, 0xdb);
const RULE: Rgb = (0xdd, 0xdd, 0xdd);
const QUOTE: Rgb = (0x7d, 0x79, 0x7a);
const HEADER_BG: Rgb = (0xf3, 0xf4, 0xf6);
const CODE_BG: Rgb = (0xf5, 0xf6, 0xf8);
const HIGHLIGHT: Rgb = (0xff, 0xf0, 0x8a);

type Rgb = (u8, u8, u8);

#[derive(Debug, thiserror::Error)]
pub enum PdfError {
    #[error("pdf exceeds the output cap")]
    TooLarge,
    #[error("pdf write failed: {0}")]
    Write(String),
}

/// A written PDF and what the tests and the oracle comparison check.
#[derive(Debug)]
pub struct RenderedPdf {
    pub bytes: Vec<u8>,
    pub pages: usize,
    /// Glyphs no embedded font has (drawn as `.notdef`).
    pub missing_glyphs: usize,
}

/// The PDF of `doc`, at most [`PDF_MAX_OUTPUT_BYTES`].
pub fn write_pdf(doc: &ExportDoc) -> Result<Vec<u8>, PdfError> {
    render_pdf(doc).map(|r| r.bytes)
}

pub fn render_pdf(doc: &ExportDoc) -> Result<RenderedPdf, PdfError> {
    let faces = Faces::load().ok_or_else(|| PdfError::Write("font load failed".into()))?;
    let mut layout = Layout {
        faces: &faces,
        cache: HashMap::new(),
        missing: 0,
        containers: 0,
    };
    let none: Deco = Rc::from(Vec::new());
    let mut entries = Vec::new();
    let width = PAGE_W - 2.0 * PAD_X;
    // Source `titledDocument`: the title is a level-1 heading before the body.
    if let Some(title) = &doc.title {
        let heading = Block::Heading {
            level: 1,
            inlines: vec![Inline::Text {
                text: title.clone(),
                marks: Default::default(),
            }],
        };
        layout.block(&heading, PAD_X, width, &none, &mut entries);
    }
    layout.blocks(&doc.blocks, PAD_X, width, &none, &mut entries);
    let missing = layout.missing;
    let pages = paginate(entries);

    let mut document = Document::new();
    if let Some(title) = &doc.title {
        document.set_metadata(Metadata::new().title(title.clone()));
    }
    let settings =
        PageSettings::from_wh(PAGE_W, PAGE_H).ok_or_else(|| PdfError::Write("page".into()))?;
    let page_count = pages.len();
    for placed in pages {
        let mut page = document.start_page_with(settings.clone());
        let mut links = Vec::new();
        {
            let mut surface = page.surface();
            let entries: Vec<(f32, &Entry)> = placed.iter().map(|p| (p.y, &p.entry)).collect();
            draw_entries(&mut surface, &faces, &entries, &mut links);
            surface.finish();
        }
        for (rect, uri) in links {
            let link =
                LinkAnnotation::new(rect, Target::Action(Action::Link(LinkAction::new(uri))));
            page.add_annotation(Annotation::new_link(link, None));
        }
        page.finish();
    }
    let bytes = document
        .finish()
        .map_err(|e| PdfError::Write(format!("{e:?}")))?;
    if bytes.len() > PDF_MAX_OUTPUT_BYTES {
        return Err(PdfError::TooLarge);
    }
    Ok(RenderedPdf {
        bytes,
        pages: page_count,
        missing_glyphs: missing,
    })
}

// ---------------------------------------------------------------------------
// Fonts

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum FaceId {
    Sans,
    SansBold,
    Mono,
    Emoji,
    EmojiBold,
}

struct Face {
    pdf: Font,
    shaper: rustybuzz::Face<'static>,
    upem: f32,
}

struct Faces {
    sans: Face,
    sans_bold: Face,
    mono: Face,
    emoji: Face,
    emoji_bold: Face,
    /// Noto Sans KR hhea ascender and descender (em, descender positive):
    /// every line's baseline, whatever fonts it mixes.
    ascent: f32,
    descent: f32,
}

impl Faces {
    fn load() -> Option<Self> {
        fn face(data: &'static [u8], weight: Option<f32>) -> Option<Face> {
            let coords: Vec<(Tag, f32)> = weight
                .map(|w| vec![(Tag::new(b"wght"), w)])
                .unwrap_or_default();
            let pdf = Font::new_variable(data.into(), 0, &coords)?;
            let mut shaper = rustybuzz::Face::from_slice(data, 0)?;
            if let Some(w) = weight {
                shaper.set_variation(rustybuzz::ttf_parser::Tag::from_bytes(b"wght"), w);
            }
            let upem = shaper.units_per_em() as f32;
            Some(Face { pdf, shaper, upem })
        }
        let sans_data = font_file(SANS_FILE)?;
        let mono_data = font_file(MONO_FILE)?;
        let emoji_data = font_file(EMOJI_FILE)?;
        let sans = face(sans_data, Some(400.0))?;
        let ascent = f32::from(sans.shaper.ascender()) / sans.upem;
        let descent = -f32::from(sans.shaper.descender()) / sans.upem;
        Some(Self {
            sans,
            sans_bold: face(sans_data, Some(700.0))?,
            mono: face(mono_data, None)?,
            emoji: face(emoji_data, Some(400.0))?,
            emoji_bold: face(emoji_data, Some(700.0))?,
            ascent,
            descent,
        })
    }

    fn get(&self, id: FaceId) -> &Face {
        match id {
            FaceId::Sans => &self.sans,
            FaceId::SansBold => &self.sans_bold,
            FaceId::Mono => &self.mono,
            FaceId::Emoji => &self.emoji,
            FaceId::EmojiBold => &self.emoji_bold,
        }
    }

    fn has(&self, id: FaceId, c: char) -> bool {
        self.get(id).shaper.glyph_index(c).is_some()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    Sans,
    Mono,
}

/// Fallback order for a run (source: page font Noto Sans KR, `mono` for
/// code/math/mermaid, `Noto Emoji` for pictograph runs).
fn chain(family: Family, bold: bool) -> [FaceId; 3] {
    let (sans, emoji) = if bold {
        (FaceId::SansBold, FaceId::EmojiBold)
    } else {
        (FaceId::Sans, FaceId::Emoji)
    };
    match family {
        Family::Sans => [sans, FaceId::Mono, emoji],
        Family::Mono => [FaceId::Mono, sans, emoji],
    }
}

/// Characters the TS export drew with Noto Emoji (`\p{Extended_Pictographic}`,
/// by block) plus regional indicators, which only the emoji font covers.
fn prefers_emoji(c: char) -> bool {
    matches!(c as u32,
        0xA9 | 0xAE | 0x203C | 0x2049 | 0x2122 | 0x2139 | 0x2194..=0x2199
        | 0x21A9..=0x21AA | 0x231A..=0x231B | 0x2328 | 0x2388 | 0x23CF
        | 0x23E9..=0x23F3 | 0x23F8..=0x23FA | 0x24C2 | 0x25AA..=0x25AB | 0x25B6
        | 0x25C0 | 0x25FB..=0x25FE | 0x2600..=0x27BF | 0x2934..=0x2935
        | 0x2B05..=0x2B07 | 0x2B1B..=0x2B1C | 0x2B50 | 0x2B55 | 0x3030 | 0x303D
        | 0x3297 | 0x3299 | 0x1F000..=0x1FAFF | 0x1FC00..=0x1FFFD)
}

/// Marks, joiners, variation selectors, emoji modifiers/tags and trailing
/// Hangul jamo: shaped with the character before them.
fn joins_previous(c: char) -> bool {
    matches!(c as u32,
        0x0300..=0x036F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF
        | 0xFE20..=0xFE2F | 0x200C..=0x200D | 0xFE00..=0xFE0F | 0xE0100..=0xE01EF
        | 0x1F3FB..=0x1F3FF | 0xE0020..=0xE007F | 0x1160..=0x11FF | 0xD7B0..=0xD7FF
        | 0x3099..=0x309A)
}

// ---------------------------------------------------------------------------
// Layout model

#[derive(Debug, Clone, PartialEq)]
struct Style {
    family: Family,
    size: f32,
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
    highlight: bool,
    color: Rgb,
    link: Option<Rc<str>>,
}

impl Style {
    fn base(family: Family, size: f32, bold: bool) -> Self {
        Self {
            family,
            size,
            bold,
            italic: false,
            underline: false,
            strike: false,
            highlight: false,
            color: BLACK,
            link: None,
        }
    }
}

/// One shaped string in one font; advances in em.
struct Shaped {
    text: String,
    glyphs: Vec<KrillaGlyph>,
    advance: f32,
}

#[derive(Clone)]
struct Piece {
    face: FaceId,
    style: Rc<Style>,
    shaped: Rc<Shaped>,
}

impl Piece {
    fn width(&self) -> f32 {
        self.shaped.advance * self.style.size
    }
}

struct Line {
    height: f32,
    /// Baseline below the line top.
    baseline: f32,
    pieces: Vec<(f32, Piece)>,
    checkbox: Option<(f32, bool)>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Decoration {
    /// `id` tells adjacent containers of the same kind apart.
    Fill {
        x0: f32,
        x1: f32,
        color: Rgb,
        id: u32,
    },
    Bar {
        x: f32,
        width: f32,
        color: Rgb,
        id: u32,
    },
}

/// Container chrome drawn beside every entry inside the container, so it
/// continues across page breaks.
type Deco = Rc<[Decoration]>;

enum Item {
    Line(Line),
    Space(f32),
    Rule { x0: f32, x1: f32 },
    Row(Row),
}

struct Entry {
    item: Item,
    deco: Deco,
}

impl Entry {
    fn height(&self) -> f32 {
        match &self.item {
            Item::Line(l) => l.height,
            Item::Space(h) => *h,
            Item::Rule { .. } => 1.0,
            Item::Row(r) => r.height,
        }
    }
}

struct Cell {
    x: f32,
    w: f32,
    header: bool,
    entries: Vec<Entry>,
}

struct Row {
    cells: Vec<Cell>,
    height: f32,
}

impl Row {
    fn new(cells: Vec<Cell>) -> Self {
        let inner = cells
            .iter()
            .map(|c| c.entries.iter().map(Entry::height).sum::<f32>())
            .fold(0.0, f32::max);
        Self {
            cells,
            height: inner + 2.0 * (CELL_BORDER + CELL_PAD),
        }
    }
}

struct Span {
    range: Range<usize>,
    style: Rc<Style>,
}

struct Layout<'f> {
    faces: &'f Faces,
    cache: HashMap<(FaceId, String), Rc<Shaped>>,
    missing: usize,
    containers: u32,
}

fn with_deco(deco: &Deco, extra: &[Decoration]) -> Deco {
    deco.iter().chain(extra).copied().collect::<Vec<_>>().into()
}

fn space(h: f32, deco: &Deco) -> Entry {
    Entry {
        item: Item::Space(h),
        deco: deco.clone(),
    }
}

fn callout_color(kind: &str) -> (Rgb, Rgb) {
    match kind {
        "tip" | "success" => ((0x2f, 0x9e, 0x44), (0xed, 0xf8, 0xef)),
        "warning" | "caution" => ((0xd9, 0x7a, 0x06), (0xfd, 0xf5, 0xe6)),
        "danger" | "error" => ((0xc9, 0x2a, 0x2a), (0xfc, 0xed, 0xed)),
        "note" | "info" => ((0x1a, 0x56, 0xdb), (0xeb, 0xf2, 0xfd)),
        _ => ((0x86, 0x8e, 0x96), (0xf3, 0xf4, 0xf6)),
    }
}

impl Layout<'_> {
    fn blocks(&mut self, blocks: &[Block], x: f32, w: f32, deco: &Deco, out: &mut Vec<Entry>) {
        for b in blocks {
            self.block(b, x, w, deco, out);
        }
    }

    fn block(&mut self, block: &Block, x: f32, w: f32, deco: &Deco, out: &mut Vec<Entry>) {
        let w = w.max(MIN_WIDTH);
        match block {
            Block::Heading { level, inlines } => {
                // Source `headingSize`: 24/18/14, then 12.
                let size = match level {
                    1 => 24.0,
                    2 => 18.0,
                    3 => 14.0,
                    _ => 12.0,
                };
                let spans = self.inline_spans(inlines, &Style::base(Family::Sans, size, true));
                self.paragraph(spans, x, w, deco, out);
                out.push(space(HEADING_GAP, deco));
            }
            Block::Paragraph(inlines) => {
                let spans =
                    self.inline_spans(inlines, &Style::base(Family::Sans, BODY_SIZE, false));
                self.paragraph(spans, x, w, deco, out);
                out.push(space(PARAGRAPH_GAP, deco));
            }
            Block::List { kind, items } => {
                self.list(*kind, items, x, w, deco, out);
                // Source: bullet/ordered lists have margins; task lists
                // fell through to plain containers (children only).
                if *kind != ListKind::Task {
                    out.push(space(LIST_GAP, deco));
                }
            }
            Block::Table(rows) => {
                self.table(rows, x, w, deco, out);
                out.push(space(TABLE_GAP, deco));
            }
            Block::Code { text, .. } => {
                let id = self.next_id();
                let inner = with_deco(
                    deco,
                    &[Decoration::Fill {
                        x0: x,
                        x1: x + w,
                        color: CODE_BG,
                        id,
                    }],
                );
                // Source: mono 10 pt, margin 6; the background adds no height.
                self.plain(
                    text,
                    Style::base(Family::Mono, MONO_SIZE, false),
                    x + CODE_PAD,
                    w - 2.0 * CODE_PAD,
                    &inner,
                    out,
                );
                out.push(space(PARAGRAPH_GAP, deco));
            }
            Block::Math(text) | Block::Mermaid(text) => {
                self.plain(
                    text,
                    Style::base(Family::Mono, MONO_SIZE, false),
                    x,
                    w,
                    deco,
                    out,
                );
                out.push(space(PARAGRAPH_GAP, deco));
            }
            Block::Blockquote(blocks) => {
                let id = self.next_id();
                let inner = with_deco(
                    deco,
                    &[Decoration::Bar {
                        x,
                        width: QUOTE_BAR,
                        color: QUOTE,
                        id,
                    }],
                );
                let indent = QUOTE_BAR + QUOTE_PAD;
                self.blocks(blocks, x + indent, w - indent, &inner, out);
                out.push(space(PARAGRAPH_GAP, deco));
            }
            Block::Callout { kind, blocks } => {
                let (bar, bg) = callout_color(kind);
                let id = self.next_id();
                let inner = with_deco(
                    deco,
                    &[
                        Decoration::Fill {
                            x0: x,
                            x1: x + w,
                            color: bg,
                            id,
                        },
                        Decoration::Bar {
                            x,
                            width: 3.0,
                            color: bar,
                            id,
                        },
                    ],
                );
                // Source: the children only (their own margins); the chrome
                // adds no height, so pages break where the TS export's did.
                self.blocks(blocks, x + 12.0, w - 18.0, &inner, out);
            }
            Block::Attachment { name, .. } => {
                // Source: the attachment name as a paragraph; no bytes.
                self.plain(
                    name,
                    Style::base(Family::Sans, BODY_SIZE, false),
                    x,
                    w,
                    deco,
                    out,
                );
                out.push(space(PARAGRAPH_GAP, deco));
            }
            Block::Embed { entity, reference } => {
                if reference.is_empty() {
                    return;
                }
                let mut style = Style::base(Family::Sans, BODY_SIZE, false);
                let text = if entity == "url" {
                    if let Some(href) = hyperlink_target(reference) {
                        style.link = Some(href.into());
                        style.color = LINK;
                        style.underline = true;
                    }
                    reference.clone()
                } else {
                    style.color = GRAY_TEXT;
                    let entity = if entity == "document" { "doc" } else { entity };
                    format!("[[{entity}:{reference}]]")
                };
                self.plain(&text, style, x, w, deco, out);
                out.push(space(PARAGRAPH_GAP, deco));
            }
            Block::HorizontalRule => {
                out.push(space(RULE_GAP, deco));
                out.push(Entry {
                    item: Item::Rule { x0: x, x1: x + w },
                    deco: deco.clone(),
                });
                out.push(space(RULE_GAP, deco));
            }
            Block::Details { summary, blocks } => {
                if !summary.is_empty() {
                    let spans =
                        self.inline_spans(summary, &Style::base(Family::Sans, BODY_SIZE, true));
                    self.paragraph(spans, x, w, deco, out);
                    out.push(space(PARAGRAPH_GAP, deco));
                }
                self.blocks(blocks, x, w, deco, out);
            }
        }
    }

    fn list(
        &mut self,
        kind: ListKind,
        items: &[ListItem],
        x: f32,
        w: f32,
        deco: &Deco,
        out: &mut Vec<Entry>,
    ) {
        let marker_style = Rc::new(Style::base(Family::Sans, BODY_SIZE, false));
        for (i, item) in items.iter().enumerate() {
            let mut entries = Vec::new();
            self.blocks(&item.blocks, x + MARKER_W, w - MARKER_W, deco, &mut entries);
            if !matches!(
                entries.first(),
                Some(Entry {
                    item: Item::Line(_),
                    ..
                })
            ) {
                entries.insert(
                    0,
                    Entry {
                        item: Item::Line(self.empty_line(BODY_SIZE)),
                        deco: deco.clone(),
                    },
                );
            }
            let Item::Line(line) = &mut entries[0].item else {
                continue;
            };
            match kind {
                ListKind::Task => line.checkbox = Some((x, item.checked == Some(true))),
                ListKind::Bullet | ListKind::Ordered => {
                    // Source: `•`, or the item index (not `start`) and a dot.
                    let text = if kind == ListKind::Bullet {
                        "•".to_string()
                    } else {
                        format!("{}.", i + 1)
                    };
                    let pieces = self.pieces(
                        &text,
                        0..text.len(),
                        &[Span {
                            range: 0..text.len(),
                            style: marker_style.clone(),
                        }],
                    );
                    let mut mx = x;
                    for p in pieces {
                        let pw = p.width();
                        line.pieces.insert(0, (mx, p));
                        mx += pw;
                    }
                }
            }
            out.extend(entries);
            if kind != ListKind::Task {
                out.push(space(ITEM_GAP, deco));
            }
        }
    }

    fn table(&mut self, rows: &[TableRow], x: f32, w: f32, deco: &Deco, out: &mut Vec<Entry>) {
        let none: Deco = Rc::from(Vec::new());
        let inset = CELL_BORDER + CELL_PAD;
        for row in rows {
            // Source: equal flex columns per row.
            let n = row.cells.len();
            if n == 0 {
                continue;
            }
            let cw = w / n as f32;
            let cells = row
                .cells
                .iter()
                .enumerate()
                .map(|(i, cell)| {
                    let cx = x + i as f32 * cw;
                    let mut entries = Vec::new();
                    self.blocks(
                        &cell.blocks,
                        cx + inset,
                        cw - 2.0 * inset,
                        &none,
                        &mut entries,
                    );
                    Cell {
                        x: cx,
                        w: cw,
                        header: cell.header,
                        entries,
                    }
                })
                .collect();
            out.push(Entry {
                item: Item::Row(Row::new(cells)),
                deco: deco.clone(),
            });
        }
    }

    fn next_id(&mut self) -> u32 {
        self.containers += 1;
        self.containers
    }

    fn empty_line(&self, size: f32) -> Line {
        let height = size * LINE_HEIGHT;
        Line {
            height,
            baseline: self.baseline(size),
            pieces: Vec::new(),
            checkbox: None,
        }
    }

    fn baseline(&self, size: f32) -> f32 {
        let height = size * LINE_HEIGHT;
        (height - (self.faces.ascent + self.faces.descent) * size) / 2.0 + self.faces.ascent * size
    }

    fn plain(
        &mut self,
        text: &str,
        style: Style,
        x: f32,
        w: f32,
        deco: &Deco,
        out: &mut Vec<Entry>,
    ) {
        let text = clean_text(text);
        let spans = vec![Span {
            range: 0..text.len(),
            style: Rc::new(style),
        }];
        self.paragraph((text, spans), x, w, deco, out);
    }

    /// Source `inlineRuns`: text/mention/emoji and inline LaTeX as text with
    /// the block's style plus the node's marks; hard breaks as `\n`.
    fn inline_spans(&self, inlines: &[Inline], base: &Style) -> (String, Vec<Span>) {
        let mut text = String::new();
        let mut spans: Vec<Span> = Vec::new();
        for inline in inlines {
            let (s, marks) = match inline {
                Inline::Text { text, marks } | Inline::Math { latex: text, marks } => {
                    (clean_text(text), Some(marks))
                }
                Inline::HardBreak => ("\n".to_string(), None),
            };
            if s.is_empty() {
                continue;
            }
            let mut style = base.clone();
            if let Some(m) = marks {
                style.bold |= m.bold;
                style.italic = m.italic;
                style.underline = m.underline;
                style.strike = m.strike;
                style.highlight = m.highlight;
                if m.code {
                    style.family = Family::Mono;
                }
                if let Some(href) = m.link.as_deref().and_then(hyperlink_target) {
                    style.link = Some(href.into());
                    style.color = LINK;
                    style.underline = true;
                }
            }
            let start = text.len();
            text.push_str(&s);
            spans.push(Span {
                range: start..text.len(),
                style: Rc::new(style),
            });
        }
        (text, spans)
    }

    /// Greedy line filling at UAX #14 opportunities; a word wider than the
    /// line breaks between clusters.
    fn paragraph(
        &mut self,
        (text, spans): (String, Vec<Span>),
        x: f32,
        w: f32,
        deco: &Deco,
        out: &mut Vec<Entry>,
    ) {
        if text.is_empty() {
            return;
        }
        let size = spans.iter().map(|s| s.style.size).fold(0.0, f32::max);
        let mut line = LineBuilder::new(x);
        let mut start = 0;
        for (end, opportunity) in linebreaks(&text) {
            let segment = &text[start..end];
            let body_len = segment.trim_end_matches(['\n', '\r']).len();
            let core_len = segment[..body_len]
                .trim_end_matches([' ', '\u{3000}'])
                .len();
            let core = self.pieces(&text, start..start + core_len, &spans);
            let trailing = self.pieces(&text, start + core_len..start + body_len, &spans);
            let core_w: f32 = core.iter().map(Piece::width).sum();
            if !line.is_empty() && line.width + core_w > w {
                out.push(self.flush(&mut line, size, deco));
            }
            if core_w > w {
                for piece in core {
                    for part in split_clusters(&piece) {
                        let pw = part.width();
                        if !line.is_empty() && line.width + pw > w {
                            out.push(self.flush(&mut line, size, deco));
                        }
                        line.push(part);
                    }
                }
            } else {
                for piece in core {
                    line.push(piece);
                }
            }
            for piece in trailing {
                line.push_trailing(piece);
            }
            if opportunity == BreakOpportunity::Mandatory && end < text.len() {
                out.push(self.flush(&mut line, size, deco));
            }
            start = end;
        }
        if !line.is_empty() {
            out.push(self.flush(&mut line, size, deco));
        }
    }

    fn flush(&self, builder: &mut LineBuilder, size: f32, deco: &Deco) -> Entry {
        let mut pieces = std::mem::take(&mut builder.pieces);
        // Spaces at the end of a wrapped line are not drawn.
        pieces.truncate(builder.trailing_from.min(pieces.len()));
        builder.width = 0.0;
        builder.trailing_from = 0;
        let mut placed = Vec::with_capacity(pieces.len());
        let mut cx = builder.x;
        for p in pieces {
            let pw = p.width();
            placed.push((cx, p));
            cx += pw;
        }
        let mut line = self.empty_line(size);
        line.pieces = placed;
        Entry {
            item: Item::Line(line),
            deco: deco.clone(),
        }
    }

    /// `text[range]` split into runs of one style and one font, shaped.
    fn pieces(&mut self, text: &str, range: Range<usize>, spans: &[Span]) -> Vec<Piece> {
        let mut out = Vec::new();
        if range.is_empty() {
            return out;
        }
        let first = spans.partition_point(|s| s.range.end <= range.start);
        for span in &spans[first..] {
            if span.range.start >= range.end {
                break;
            }
            let lo = span.range.start.max(range.start);
            let hi = span.range.end.min(range.end);
            let faces = chain(span.style.family, span.style.bold);
            let mut run_start = lo;
            let mut run_face: Option<FaceId> = None;
            for (i, c) in text[lo..hi].char_indices() {
                let at = lo + i;
                let face = match run_face {
                    Some(prev) if joins_previous(c) => prev,
                    _ => self.face_for(c, &faces),
                };
                if run_face != Some(face) {
                    if let Some(prev) = run_face {
                        out.push(self.piece(prev, &span.style, &text[run_start..at]));
                    }
                    run_start = at;
                    run_face = Some(face);
                }
            }
            if let Some(face) = run_face {
                out.push(self.piece(face, &span.style, &text[run_start..hi]));
            }
        }
        out
    }

    fn face_for(&self, c: char, faces: &[FaceId; 3]) -> FaceId {
        if prefers_emoji(c) && self.faces.has(faces[2], c) {
            return faces[2];
        }
        faces
            .iter()
            .copied()
            .find(|f| self.faces.has(*f, c))
            .unwrap_or(faces[0])
    }

    fn piece(&mut self, face: FaceId, style: &Rc<Style>, text: &str) -> Piece {
        let shaped = match self.cache.get(&(face, text.to_string())) {
            Some(s) => s.clone(),
            None => {
                let s = Rc::new(shape(self.faces.get(face), text));
                self.missing += s
                    .glyphs
                    .iter()
                    .filter(|g| g.glyph_id == GlyphId::new(0))
                    .count();
                self.cache.insert((face, text.to_string()), s.clone());
                s
            }
        };
        Piece {
            face,
            style: style.clone(),
            shaped,
        }
    }
}

struct LineBuilder {
    x: f32,
    width: f32,
    pieces: Vec<Piece>,
    /// First piece of the trailing spaces after the last word.
    trailing_from: usize,
}

impl LineBuilder {
    fn new(x: f32) -> Self {
        Self {
            x,
            width: 0.0,
            pieces: Vec::new(),
            trailing_from: 0,
        }
    }

    fn is_empty(&self) -> bool {
        self.pieces.is_empty()
    }

    fn push(&mut self, piece: Piece) {
        self.width += piece.width();
        self.pieces.push(piece);
        self.trailing_from = self.pieces.len();
    }

    /// Spaces after a word: kept inside the line, dropped at its end.
    fn push_trailing(&mut self, piece: Piece) {
        self.width += piece.width();
        self.pieces.push(piece);
    }
}

/// Stored text for drawing: CR LF / CR -> LF (a line break, as the TS Text
/// rendered it), tab -> four spaces, NUL -> U+FFFD, other C0 controls and
/// DEL dropped.
fn clean_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push('\n');
            }
            '\n' => out.push('\n'),
            '\t' => out.push_str("    "),
            '\0' => out.push('\u{FFFD}'),
            c if c.is_control() && (c as u32) < 0xA0 => {}
            c => out.push(c),
        }
    }
    out
}

fn shape(face: &Face, text: &str) -> Shaped {
    let mut buffer = rustybuzz::UnicodeBuffer::new();
    buffer.push_str(text);
    buffer.guess_segment_properties();
    buffer.set_direction(rustybuzz::Direction::LeftToRight);
    let output = rustybuzz::shape(&face.shaper, &[], buffer);
    let infos = output.glyph_infos();
    let positions = output.glyph_positions();
    let mut glyphs = Vec::with_capacity(infos.len());
    let mut advance = 0.0;
    for (i, (info, pos)) in infos.iter().zip(positions).enumerate() {
        let start = info.cluster as usize;
        let end = infos[i + 1..]
            .iter()
            .find(|next| next.cluster != info.cluster)
            .map_or(text.len(), |next| next.cluster as usize);
        let x_advance = pos.x_advance as f32 / face.upem;
        advance += x_advance;
        glyphs.push(KrillaGlyph::new(
            GlyphId::new(info.glyph_id),
            x_advance,
            pos.x_offset as f32 / face.upem,
            pos.y_offset as f32 / face.upem,
            pos.y_advance as f32 / face.upem,
            start..end.max(start),
            None,
        ));
    }
    Shaped {
        text: text.to_string(),
        glyphs,
        advance,
    }
}

/// One piece per cluster, for a word wider than its line.
fn split_clusters(piece: &Piece) -> Vec<Piece> {
    let shaped = &piece.shaped;
    let mut out = Vec::new();
    let mut i = 0;
    while i < shaped.glyphs.len() {
        let range = shaped.glyphs[i].text_range.clone();
        let mut j = i + 1;
        while j < shaped.glyphs.len() && shaped.glyphs[j].text_range.start == range.start {
            j += 1;
        }
        let offset = range.start;
        let glyphs: Vec<KrillaGlyph> = shaped.glyphs[i..j]
            .iter()
            .map(|g| {
                let mut g = g.clone();
                g.text_range = g.text_range.start - offset..g.text_range.end - offset;
                g
            })
            .collect();
        let advance = glyphs.iter().map(|g| g.x_advance).sum();
        out.push(Piece {
            face: piece.face,
            style: piece.style.clone(),
            shaped: Rc::new(Shaped {
                text: shaped.text[range].to_string(),
                glyphs,
                advance,
            }),
        });
        i = j;
    }
    out
}

// ---------------------------------------------------------------------------
// Pagination

struct Placed {
    y: f32,
    entry: Entry,
}

struct Pager {
    pages: Vec<Vec<Placed>>,
    y: f32,
}

fn paginate(entries: Vec<Entry>) -> Vec<Vec<Placed>> {
    let mut pager = Pager {
        pages: vec![Vec::new()],
        y: CONTENT_TOP,
    };
    for entry in entries {
        pager.place(entry);
    }
    // Trailing gaps can leave an empty last page only if nothing was drawn
    // on it; the TS document always has at least one page.
    if pager.pages.len() > 1 && pager.pages.last().is_some_and(Vec::is_empty) {
        pager.pages.pop();
    }
    pager.pages
}

impl Pager {
    fn at_top(&self) -> bool {
        self.pages.last().is_none_or(Vec::is_empty)
    }

    fn new_page(&mut self) {
        self.pages.push(Vec::new());
        self.y = CONTENT_TOP;
    }

    fn push(&mut self, entry: Entry) {
        let h = entry.height();
        let y = self.y;
        if let Some(page) = self.pages.last_mut() {
            page.push(Placed { y, entry });
        }
        self.y += h;
    }

    fn place(&mut self, entry: Entry) {
        let h = entry.height();
        match entry.item {
            Item::Space(_) => {
                if self.y + h > CONTENT_BOTTOM {
                    self.new_page();
                } else if !(self.at_top() && entry.deco.is_empty()) {
                    self.push(entry);
                }
            }
            Item::Row(row) => self.place_row(row, entry.deco),
            _ => {
                if self.y + h > CONTENT_BOTTOM && !self.at_top() {
                    self.new_page();
                }
                self.push(entry);
            }
        }
    }

    fn place_row(&mut self, mut row: Row, deco: Deco) {
        loop {
            if self.y + row.height <= CONTENT_BOTTOM {
                break;
            }
            if !self.at_top() && row.height <= CONTENT_BOTTOM - CONTENT_TOP {
                self.new_page();
                continue;
            }
            match split_row(row, CONTENT_BOTTOM - self.y) {
                Ok((head, tail)) => {
                    self.push(Entry {
                        item: Item::Row(head),
                        deco: deco.clone(),
                    });
                    self.new_page();
                    row = tail;
                }
                Err(whole) if !self.at_top() => {
                    self.new_page();
                    row = whole;
                }
                // Nothing fits even on a fresh page (pathological nesting):
                // draw it past the bottom margin rather than loop.
                Err(whole) => {
                    row = whole;
                    break;
                }
            }
        }
        self.push(Entry {
            item: Item::Row(row),
            deco,
        });
    }
}

/// The part of `row` that fits in `avail` and the rest, or the row back when
/// no cell can start in `avail`.
fn split_row(row: Row, avail: f32) -> Result<(Row, Row), Row> {
    let inner = avail - 2.0 * (CELL_BORDER + CELL_PAD);
    let mut heads = Vec::with_capacity(row.cells.len());
    let mut tails = Vec::with_capacity(row.cells.len());
    let mut any = false;
    for cell in row.cells {
        let (head, tail) = split_entries(cell.entries, inner);
        any |= !head.is_empty();
        heads.push(Cell {
            x: cell.x,
            w: cell.w,
            header: cell.header,
            entries: head,
        });
        tails.push(Cell {
            x: cell.x,
            w: cell.w,
            header: cell.header,
            entries: tail,
        });
    }
    if !any {
        let cells = heads
            .into_iter()
            .zip(tails)
            .map(|(mut h, t)| {
                h.entries.extend(t.entries);
                h
            })
            .collect();
        return Err(Row::new(cells));
    }
    Ok((Row::new(heads), Row::new(tails)))
}

fn split_entries(entries: Vec<Entry>, avail: f32) -> (Vec<Entry>, Vec<Entry>) {
    let mut head = Vec::new();
    let mut tail = Vec::new();
    let mut used = 0.0;
    let mut iter = entries.into_iter();
    for entry in iter.by_ref() {
        let h = entry.height();
        if used + h <= avail {
            used += h;
            head.push(entry);
            continue;
        }
        match entry.item {
            Item::Row(row) => match split_row(row, avail - used) {
                Ok((a, b)) => {
                    head.push(Entry {
                        item: Item::Row(a),
                        deco: entry.deco.clone(),
                    });
                    tail.push(Entry {
                        item: Item::Row(b),
                        deco: entry.deco,
                    });
                }
                Err(row) => tail.push(Entry {
                    item: Item::Row(row),
                    deco: entry.deco,
                }),
            },
            item => tail.push(Entry {
                item,
                deco: entry.deco,
            }),
        }
        break;
    }
    tail.extend(iter);
    (head, tail)
}

// ---------------------------------------------------------------------------
// Drawing

fn fill(color: Rgb) -> Fill {
    Fill {
        paint: rgb::Color::new(color.0, color.1, color.2).into(),
        ..Fill::default()
    }
}

fn fill_rect(surface: &mut Surface, x: f32, y: f32, w: f32, h: f32, color: Rgb) {
    let Some(rect) = Rect::from_xywh(x, y, w, h) else {
        return;
    };
    let mut pb = PathBuilder::new();
    pb.push_rect(rect);
    if let Some(path) = pb.finish() {
        surface.set_stroke(None);
        surface.set_fill(Some(fill(color)));
        surface.draw_path(&path);
    }
}

fn stroke_rect(surface: &mut Surface, x: f32, y: f32, w: f32, h: f32, width: f32, color: Rgb) {
    let Some(rect) = Rect::from_xywh(x, y, w, h) else {
        return;
    };
    let mut pb = PathBuilder::new();
    pb.push_rect(rect);
    if let Some(path) = pb.finish() {
        surface.set_fill(None);
        surface.set_stroke(Some(Stroke {
            paint: rgb::Color::new(color.0, color.1, color.2).into(),
            width,
            ..Stroke::default()
        }));
        surface.draw_path(&path);
        surface.set_stroke(None);
    }
}

/// Entries stacked from their `y`: container chrome first, one rectangle
/// per run of adjacent entries (no seams between lines), then the entries.
fn draw_entries(
    surface: &mut Surface,
    faces: &Faces,
    entries: &[(f32, &Entry)],
    links: &mut Vec<(Rect, String)>,
) {
    let mut spans: Vec<(Decoration, f32, f32)> = Vec::new();
    let mut open: Vec<usize> = Vec::new();
    for (y, entry) in entries {
        let bottom = y + entry.height();
        let mut still_open = Vec::new();
        for d in entry.deco.iter() {
            match open
                .iter()
                .find(|&&i| spans[i].0 == *d && (spans[i].2 - y).abs() < 0.01)
            {
                Some(&i) => {
                    spans[i].2 = bottom;
                    still_open.push(i);
                }
                None => {
                    spans.push((*d, *y, bottom));
                    still_open.push(spans.len() - 1);
                }
            }
        }
        open = still_open;
    }
    for (d, y0, y1) in spans {
        match d {
            Decoration::Fill { x0, x1, color, .. } => {
                fill_rect(surface, x0, y0, x1 - x0, y1 - y0, color)
            }
            Decoration::Bar {
                x, width, color, ..
            } => fill_rect(surface, x, y0, width, y1 - y0, color),
        }
    }
    for (y, entry) in entries {
        draw_entry(surface, faces, entry, *y, links);
    }
}

fn draw_entry(
    surface: &mut Surface,
    faces: &Faces,
    entry: &Entry,
    y: f32,
    links: &mut Vec<(Rect, String)>,
) {
    match &entry.item {
        Item::Space(_) => {}
        Item::Rule { x0, x1 } => fill_rect(surface, *x0, y, x1 - x0, 1.0, RULE),
        Item::Line(line) => draw_line(surface, faces, line, y, links),
        Item::Row(row) => {
            for cell in &row.cells {
                if cell.header {
                    fill_rect(surface, cell.x, y, cell.w, row.height, HEADER_BG);
                }
                let half = CELL_BORDER / 2.0;
                stroke_rect(
                    surface,
                    cell.x + half,
                    y + half,
                    cell.w - CELL_BORDER,
                    row.height - CELL_BORDER,
                    CELL_BORDER,
                    RULE,
                );
                let mut cy = y + CELL_BORDER + CELL_PAD;
                let entries: Vec<(f32, &Entry)> = cell
                    .entries
                    .iter()
                    .map(|e| {
                        let at = cy;
                        cy += e.height();
                        (at, e)
                    })
                    .collect();
                draw_entries(surface, faces, &entries, links);
            }
        }
    }
}

fn draw_line(
    surface: &mut Surface,
    faces: &Faces,
    line: &Line,
    y: f32,
    links: &mut Vec<(Rect, String)>,
) {
    let base = y + line.baseline;
    if let Some((x, checked)) = line.checkbox {
        let side = BODY_SIZE * 0.7;
        let top = base - side;
        stroke_rect(surface, x + 0.5, top, side, side, 0.8, GRAY_TEXT);
        if checked {
            let mut pb = PathBuilder::new();
            pb.move_to(x + 0.5 + side * 0.2, top + side * 0.5);
            pb.line_to(x + 0.5 + side * 0.42, top + side * 0.75);
            pb.line_to(x + 0.5 + side * 0.82, top + side * 0.22);
            if let Some(path) = pb.finish() {
                surface.set_fill(None);
                surface.set_stroke(Some(Stroke {
                    paint: rgb::Color::new(GRAY_TEXT.0, GRAY_TEXT.1, GRAY_TEXT.2).into(),
                    width: 1.2,
                    ..Stroke::default()
                }));
                surface.draw_path(&path);
                surface.set_stroke(None);
            }
        }
    }
    // Backgrounds first, then text runs, then lines over the text.
    for (x, p) in &line.pieces {
        if p.style.highlight {
            let s = p.style.size;
            fill_rect(
                surface,
                *x,
                base - faces.ascent * s,
                p.width(),
                (faces.ascent + faces.descent) * s,
                HIGHLIGHT,
            );
        }
    }
    let mut i = 0;
    while i < line.pieces.len() {
        let (x, first) = &line.pieces[i];
        let mut j = i + 1;
        let mut end = x + first.width();
        while j < line.pieces.len()
            && same_run(first, &line.pieces[j].1)
            && (line.pieces[j].0 - end).abs() < 0.01
        {
            end += line.pieces[j].1.width();
            j += 1;
        }
        draw_run(surface, faces, *x, base, &line.pieces[i..j]);
        i = j;
    }
    for (x, p) in &line.pieces {
        let s = p.style.size;
        if p.style.underline {
            fill_rect(
                surface,
                *x,
                base + s * 0.1,
                p.width(),
                s * 0.06,
                p.style.color,
            );
        }
        if p.style.strike {
            fill_rect(
                surface,
                *x,
                base - s * 0.3,
                p.width(),
                s * 0.06,
                p.style.color,
            );
        }
    }
    // One annotation per run of pieces with the same target.
    let mut i = 0;
    while i < line.pieces.len() {
        let Some(uri) = line.pieces[i].1.style.link.clone() else {
            i += 1;
            continue;
        };
        let x0 = line.pieces[i].0;
        let mut x1 = x0;
        while i < line.pieces.len() && line.pieces[i].1.style.link.as_deref() == Some(&*uri) {
            x1 = line.pieces[i].0 + line.pieces[i].1.width();
            i += 1;
        }
        if let Some(rect) = Rect::from_ltrb(x0, y, x1.max(x0 + 0.1), y + line.height) {
            links.push((rect, uri.to_string()));
        }
    }
}

fn same_run(a: &Piece, b: &Piece) -> bool {
    a.face == b.face
        && a.style.size == b.style.size
        && a.style.color == b.style.color
        && a.style.italic == b.style.italic
        && a.style.bold == b.style.bold
}

/// Pieces of one font, size and paint drawn as one glyph run (their text
/// concatenated so each glyph keeps its source text for extraction).
fn draw_run(surface: &mut Surface, faces: &Faces, x: f32, base: f32, pieces: &[(f32, Piece)]) {
    let first = &pieces[0].1;
    let style = &first.style;
    let mut text = String::new();
    let mut glyphs = Vec::new();
    for (_, p) in pieces {
        let offset = text.len();
        text.push_str(&p.shaped.text);
        glyphs.extend(p.shaped.glyphs.iter().map(|g| {
            let mut g = g.clone();
            g.text_range = g.text_range.start + offset..g.text_range.end + offset;
            g
        }));
    }
    let color = rgb::Color::new(style.color.0, style.color.1, style.color.2);
    surface.set_fill(Some(fill(style.color)));
    // Only the monospace face has no bold instance: stroke its outlines.
    let synthetic_bold = style.bold && first.face == FaceId::Mono;
    surface.set_stroke(synthetic_bold.then(|| Stroke {
        paint: color.into(),
        width: style.size * 0.03,
        ..Stroke::default()
    }));
    if style.italic {
        // Synthetic oblique about the baseline (no italic faces ship).
        surface.push_transform(&Transform::from_row(1.0, 0.0, -0.2, 1.0, 0.2 * base, 0.0));
    }
    surface.draw_glyphs(
        Point::from_xy(x, base),
        &glyphs,
        faces.get(first.face).pdf.clone(),
        &text,
        style.size,
        false,
    );
    if style.italic {
        surface.pop();
    }
    surface.set_stroke(None);
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::documents::export_model::export_doc;

    fn layout_of(doc: &serde_json::Value) -> (Vec<Vec<Placed>>, usize) {
        let faces = Faces::load().unwrap();
        let mut layout = Layout {
            faces: &faces,
            cache: HashMap::new(),
            missing: 0,
            containers: 0,
        };
        let none: Deco = Rc::from(Vec::new());
        let mut entries = Vec::new();
        layout.blocks(
            &export_doc("", doc).blocks,
            PAD_X,
            PAGE_W - 2.0 * PAD_X,
            &none,
            &mut entries,
        );
        let missing = layout.missing;
        (paginate(entries), missing)
    }

    fn line_texts(page: &[Placed]) -> Vec<String> {
        fn walk(e: &Entry, out: &mut Vec<String>) {
            match &e.item {
                Item::Line(l) => out.push(
                    l.pieces
                        .iter()
                        .map(|(_, p)| p.shaped.text.as_str())
                        .collect(),
                ),
                Item::Row(r) => {
                    for c in &r.cells {
                        for e in &c.entries {
                            walk(e, out);
                        }
                    }
                }
                _ => {}
            }
        }
        let mut out = Vec::new();
        for p in page {
            walk(&p.entry, &mut out);
        }
        out
    }

    fn para(text: &str) -> serde_json::Value {
        json!({"type": "paragraph", "content": [{"type": "text", "text": text}]})
    }

    #[test]
    fn fallback_picks_a_font_per_character() {
        let faces = Faces::load().unwrap();
        let layout = Layout {
            faces: &faces,
            cache: HashMap::new(),
            missing: 0,
            containers: 0,
        };
        let body = chain(Family::Sans, false);
        assert_eq!(layout.face_for('한', &body), FaceId::Sans);
        assert_eq!(layout.face_for('a', &body), FaceId::Sans);
        assert_eq!(layout.face_for('😀', &body), FaceId::Emoji);
        assert_eq!(layout.face_for('🇰', &body), FaceId::Emoji);
        let code = chain(Family::Mono, true);
        assert_eq!(layout.face_for('a', &code), FaceId::Mono);
        assert_eq!(layout.face_for('😀', &code), FaceId::EmojiBold);
    }

    #[test]
    fn korean_emoji_and_astral_text_have_glyphs() {
        let doc = json!({"type": "doc", "content": [
            para("한글 텍스트 😀 👩‍💻 🇰🇷 𠜎 漢字 ㄱ ᄒ\u{1161}\u{11AB} café e\u{301} ✅"),
            {"type": "codeBlock", "content": [{"type": "text", "text": "fn main() { println!(\"안녕\"); }"}]},
        ]});
        let (pages, missing) = layout_of(&doc);
        assert_eq!(missing, 0);
        assert_eq!(pages.len(), 1);
        let lines = line_texts(&pages[0]);
        assert!(lines[0].contains("한글 텍스트 😀"), "{lines:?}");
        // Characters none of the shipped fonts cover stay .notdef (as in TS)
        // and are counted.
        let (_, missing) = layout_of(&json!({"type": "doc", "content": [para("𝐀")]}));
        assert_eq!(missing, 1);
    }

    #[test]
    fn long_text_wraps_and_paginates() {
        let words = "가나다라 마바사 abcdefgh ".repeat(40);
        let content: Vec<_> = (0..40).map(|_| para(&words)).collect();
        let (pages, missing) = layout_of(&json!({"type": "doc", "content": content}));
        assert_eq!(missing, 0);
        assert!(pages.len() > 5, "{}", pages.len());
        let width = PAGE_W - 2.0 * PAD_X;
        for page in &pages {
            for p in page {
                assert!(p.y >= CONTENT_TOP - 0.01);
                assert!(p.y + p.entry.height() <= CONTENT_BOTTOM + 0.01);
                if let Item::Line(l) = &p.entry.item {
                    let end = l
                        .pieces
                        .last()
                        .map_or(PAD_X, |(x, piece)| x + piece.width());
                    assert!(end <= PAD_X + width + 0.01, "line overflows: {end}");
                }
            }
        }
        // No text is lost across the breaks.
        let all: String = pages.iter().flat_map(|p| line_texts(p)).collect();
        assert_eq!(all.matches("마바사").count(), 40 * 40);
    }

    #[test]
    fn unbreakable_word_breaks_between_clusters() {
        let url = "https://example.com/".to_string() + &"x".repeat(400);
        let (pages, _) = layout_of(&json!({"type": "doc", "content": [para(&url)]}));
        let lines = line_texts(&pages[0]);
        assert!(lines.len() >= 3, "{lines:?}");
        assert_eq!(lines.concat(), url);
    }

    #[test]
    fn hard_breaks_and_newlines_start_lines() {
        let doc = json!({"type": "doc", "content": [
            {"type": "paragraph", "content": [
                {"type": "text", "text": "a"}, {"type": "hardBreak"}, {"type": "text", "text": "b\nc"}
            ]},
            {"type": "codeBlock", "content": [{"type": "text", "text": "x\r\n  y\tz"}]},
        ]});
        let (pages, _) = layout_of(&doc);
        assert_eq!(line_texts(&pages[0]), vec!["a", "b", "c", "x", "  y    z"]);
    }

    #[test]
    fn lists_tables_and_markers() {
        let doc = json!({"type": "doc", "content": [
            {"type": "bulletList", "content": [
                {"type": "listItem", "content": [para("one")]},
            ]},
            {"type": "orderedList", "content": [
                {"type": "listItem", "content": [para("first")]},
                {"type": "listItem", "content": [para("second")]},
            ]},
            {"type": "taskList", "content": [
                {"type": "taskItem", "attrs": {"checked": true}, "content": [para("done")]},
            ]},
            {"type": "table", "content": [
                {"type": "tableRow", "content": [
                    {"type": "tableHeader", "content": [para("h1")]},
                    {"type": "tableHeader", "content": [para("h2")]},
                ]},
                {"type": "tableRow", "content": [
                    {"type": "tableCell", "content": [para("c1")]},
                    {"type": "tableCell", "content": [para("c2")]},
                ]},
            ]},
        ]});
        let (pages, _) = layout_of(&doc);
        let lines = line_texts(&pages[0]);
        assert_eq!(
            lines,
            vec!["•one", "1.first", "2.second", "done", "h1", "h2", "c1", "c2"]
        );
        let checkbox = pages[0].iter().find_map(|p| match &p.entry.item {
            Item::Line(l) => l.checkbox,
            _ => None,
        });
        assert_eq!(checkbox, Some((PAD_X, true)));
        let rows: Vec<&Row> = pages[0]
            .iter()
            .filter_map(|p| match &p.entry.item {
                Item::Row(r) => Some(r),
                _ => None,
            })
            .collect();
        assert_eq!(rows.len(), 2);
        assert!(rows[0].cells.iter().all(|c| c.header));
        assert!((rows[0].cells[1].x - (PAD_X + (PAGE_W - 2.0 * PAD_X) / 2.0)).abs() < 0.01);
    }

    #[test]
    fn a_tall_table_row_splits_across_pages() {
        let cell: Vec<_> = (0..120).map(|i| para(&format!("line {i}"))).collect();
        let doc = json!({"type": "doc", "content": [
            {"type": "table", "content": [{"type": "tableRow", "content": [
                {"type": "tableCell", "content": cell},
                {"type": "tableCell", "content": [para("short")]},
            ]}]},
        ]});
        let (pages, _) = layout_of(&doc);
        assert!(pages.len() >= 3, "{}", pages.len());
        let all: Vec<String> = pages.iter().flat_map(|p| line_texts(p)).collect();
        assert_eq!(all.iter().filter(|l| l.starts_with("line ")).count(), 120);
        for page in &pages {
            for p in page {
                assert!(p.y + p.entry.height() <= CONTENT_BOTTOM + 0.01);
            }
        }
    }

    #[test]
    fn links_only_for_safe_schemes() {
        let doc = json!({"type": "doc", "content": [{"type": "paragraph", "content": [
            {"type": "text", "text": "ok", "marks": [{"type": "link", "attrs": {"href": "https://e.com/a"}}]},
            {"type": "text", "text": " bad", "marks": [{"type": "link", "attrs": {"href": "javascript:alert(1)"}}]},
        ]}]});
        let (pages, _) = layout_of(&doc);
        let Item::Line(line) = &pages[0][0].entry.item else {
            panic!("line")
        };
        let links: Vec<_> = line
            .pieces
            .iter()
            .map(|(_, p)| (p.shaped.text.as_str(), p.style.link.as_deref()))
            .collect();
        assert_eq!(
            links,
            vec![("ok", Some("https://e.com/a")), (" ", None), ("bad", None)]
        );
        // Annotation URIs are RFC 3986 (the DOCX writer's encoding).
        let doc = json!({"type": "doc", "content": [{"type": "paragraph", "content": [
            {"type": "text", "text": "m", "marks": [{"type": "link", "attrs": {"href": "mailto:홍 <g@e.com>"}}]},
        ]}]});
        let (pages, _) = layout_of(&doc);
        let Item::Line(line) = &pages[0][0].entry.item else {
            panic!("line")
        };
        assert_eq!(
            line.pieces[0].1.style.link.as_deref(),
            Some("mailto:%ED%99%8D%20%3Cg@e.com%3E")
        );
    }

    #[test]
    fn writes_a_pdf_with_title_and_link() {
        let doc = json!({"type": "doc", "content": [{"type": "paragraph", "content": [
            {"type": "text", "text": "링크", "marks": [{"type": "link", "attrs": {"href": "https://e.com/"}}]},
        ]}]});
        let rendered = render_pdf(&export_doc("제목", &doc)).unwrap();
        assert!(rendered.bytes.starts_with(b"%PDF-"));
        assert_eq!(rendered.pages, 1);
        assert_eq!(rendered.missing_glyphs, 0);
        let pdf = pdf_extract::Document::load_mem(&rendered.bytes).unwrap();
        assert_eq!(pdf.get_pages().len(), 1);
        let raw = String::from_utf8_lossy(&rendered.bytes);
        assert!(raw.contains("https://e.com/"));
        // Subset fonts only: far below the 33 MB of the three font files.
        assert!(rendered.bytes.len() < 200_000, "{}", rendered.bytes.len());
    }

    #[test]
    fn empty_doc_is_one_page() {
        let rendered = render_pdf(&export_doc("", &json!({"type": "doc", "content": []}))).unwrap();
        assert_eq!(rendered.pages, 1);
    }
}
