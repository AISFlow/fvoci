//! Server-side `CommonMark` + GFM to fvoci [`Body`](fvoci_model::Body) converter.
//!
//! Converts Markdown text from Trello cards/comments and Notion pages into `ProseMirror`
//! JSON documents validated against [`Body::parse`](fvoci_model::Body::parse) (and
//! therefore the allowlist in `fvoci_model::allowlist`). Never fetches remote URLs:
//! external image URLs become plain links, disallowed link schemes drop their target, and
//! local images or files are emitted only when an [`AssetResolver`] confirms they exist
//! inside the uploaded bundle.

use fvoci_model::{BODY_MAX_BYTES, BODY_MAX_NODES, TABLE_MAX_COLUMNS, TABLE_MAX_ROWS, is_id};
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, LinkType, Options, Parser, Tag, TagEnd};
use serde_json::{Map, Value, json};

pub(crate) use super::markdown_nodes::{
    AssetResolver, BodyContext, ExternalOnlyResolver, MarkdownBlocks, MarkdownNotice,
    ResolvedTarget, blocks_to_body, is_allowed_href, linked_text_node, text_heading,
    text_paragraph,
};
use super::markdown_nodes::{
    InlineItem, active_mark_values, apply_link_mark, collect_plain_text, heading_level_num,
    image_node, is_valid_language, merge_text_nodes, partition_inlines,
    prepend_prefix_to_first_paragraph,
};
use super::summary::{ImportWarningCode, truncate_chars};
use crate::error::AppError;

mod fvoci;
pub(crate) use fvoci::fvoci_markdown_to_blocks;

/// Early cap on raw inline/cell items before merging to prevent unbounded vector growth.
const MAX_RAW_INLINES: usize = BODY_MAX_NODES * 4;

/// Index slots inside [`Converter::mark_counts`].
const MARK_BOLD: usize = 0;
const MARK_ITALIC: usize = 1;
const MARK_STRIKE: usize = 2;

/// Converts Markdown into fvoci `ProseMirror` block nodes under strict depth and node limits.
///
/// # Errors
///
/// Returns a 400 error if the Markdown contains U+0000, nests deeper than `max_depth`,
/// or produces more than [`BODY_MAX_NODES`] nodes, and a 413 error if accumulated text
/// exceeds [`BODY_MAX_BYTES`].
pub(crate) fn markdown_to_blocks(
    markdown: &str,
    context: BodyContext,
    max_depth: usize,
    resolver: &mut dyn AssetResolver,
) -> Result<MarkdownBlocks, AppError> {
    convert(markdown, context, max_depth, resolver, Options::empty())
}

fn convert(
    markdown: &str,
    context: BodyContext,
    max_depth: usize,
    resolver: &mut dyn AssetResolver,
    dialect: Options,
) -> Result<MarkdownBlocks, AppError> {
    if markdown.contains('\0') {
        return Err(AppError::bad_request(
            "markdown content contains U+0000 character",
        ));
    }
    let clean = markdown.strip_prefix('\u{FEFF}').unwrap_or(markdown);
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | dialect;
    let parser = Parser::new_ext(clean, options);
    let mut state = Converter::new(context, max_depth, resolver);
    for event in parser {
        state.handle(event)?;
    }
    state.finish()
}

/// One completed item of a Markdown list before deciding `taskList` vs `bulletList`/`orderedList`.
#[derive(Debug)]
struct ListItemDraft {
    task: Option<bool>,
    blocks: Vec<Value>,
}

/// One completed row of a Markdown table.
#[derive(Debug)]
struct TableRowDraft {
    is_header: bool,
    cells: Vec<Vec<InlineItem>>,
}

/// Open block or inline frame on the iterative converter stack.
#[derive(Debug)]
enum Frame {
    Root(Vec<Value>),
    Paragraph(Vec<InlineItem>),
    Heading {
        level: u8,
        inlines: Vec<InlineItem>,
    },
    BlockQuote(Vec<Value>),
    CodeBlock {
        language: Option<String>,
        text: String,
    },
    List {
        ordered_start: Option<u64>,
        items: Vec<ListItemDraft>,
    },
    ListItem {
        task: Option<bool>,
        blocks: Vec<Value>,
        pending_inlines: Vec<InlineItem>,
    },
    Table {
        has_alignment: bool,
        rows: Vec<TableRowDraft>,
    },
    TableRow {
        is_header: bool,
        cells: Vec<Vec<InlineItem>>,
    },
    TableCell(Vec<InlineItem>),
    Link {
        dest: String,
        title_dropped: bool,
        /// Written `[[…]]` (fvoci's dialect): text as written unless it names a block.
        wiki: bool,
        inlines: Vec<InlineItem>,
    },
    Image {
        dest: String,
        alt: String,
    },
}

struct Converter<'r> {
    context: BodyContext,
    max_depth: usize,
    container_depth: usize,
    node_count: usize,
    raw_inlines: usize,
    text_bytes: usize,
    frames: Vec<Frame>,
    mark_counts: [usize; 3],
    notices: Vec<MarkdownNotice>,
    resolver: &'r mut dyn AssetResolver,
}

impl<'r> Converter<'r> {
    fn new(context: BodyContext, max_depth: usize, resolver: &'r mut dyn AssetResolver) -> Self {
        Self {
            context,
            max_depth,
            container_depth: 0,
            node_count: 1, // Count the root `doc` node.
            raw_inlines: 0,
            text_bytes: 0,
            frames: vec![Frame::Root(Vec::new())],
            mark_counts: [0; 3],
            notices: Vec::new(),
            resolver,
        }
    }

    fn bump_nodes(&mut self, added: usize) -> Result<(), AppError> {
        self.node_count = self.node_count.saturating_add(added);
        if self.node_count > BODY_MAX_NODES {
            return Err(AppError::bad_request(format_args!(
                "markdown document exceeds maximum of {BODY_MAX_NODES} nodes"
            )));
        }
        Ok(())
    }

    fn bump_text_bytes(&mut self, added: usize) -> Result<(), AppError> {
        self.text_bytes = self.text_bytes.saturating_add(added);
        if self.text_bytes > BODY_MAX_BYTES {
            return Err(AppError::too_large(
                format_args!("markdown document exceeds maximum of {BODY_MAX_BYTES} bytes"),
                u64::try_from(BODY_MAX_BYTES).unwrap_or(u64::MAX),
            ));
        }
        Ok(())
    }

    fn bump_raw_inline(&mut self) -> Result<(), AppError> {
        self.raw_inlines = self.raw_inlines.saturating_add(1);
        if self.raw_inlines > MAX_RAW_INLINES {
            return Err(AppError::bad_request(format_args!(
                "markdown document exceeds maximum of {BODY_MAX_NODES} nodes"
            )));
        }
        Ok(())
    }

    fn push_frame(&mut self, frame: Frame) -> Result<(), AppError> {
        let max_frames = self.max_depth.saturating_add(16);
        if self.frames.len() >= max_frames {
            return Err(AppError::bad_request(format_args!(
                "markdown nesting depth exceeds {} levels",
                self.max_depth
            )));
        }
        self.frames.push(frame);
        Ok(())
    }

    fn enter_container(&mut self) -> Result<(), AppError> {
        self.container_depth = self.container_depth.saturating_add(1);
        if self.container_depth > self.max_depth {
            return Err(AppError::bad_request(format_args!(
                "markdown nesting depth exceeds {} levels",
                self.max_depth
            )));
        }
        Ok(())
    }

    fn leave_container(&mut self) {
        self.container_depth = self.container_depth.saturating_sub(1);
    }

    fn note(&mut self, code: ImportWarningCode, reason: impl Into<String>) {
        if self.notices.len() < 64 {
            self.notices.push(MarkdownNotice {
                code,
                reason: reason.into(),
            });
        }
    }

    fn handle(&mut self, event: Event<'_>) -> Result<(), AppError> {
        match event {
            Event::Start(tag) => self.start_tag(tag),
            Event::End(tag) => self.end_tag(tag),
            Event::Text(text) => self.push_text(&text),
            Event::Code(code) => self.push_inline_code(&code),
            Event::SoftBreak | Event::HardBreak => self.push_break(),
            Event::Rule => {
                self.bump_nodes(1)?;
                self.flush_list_item_inlines()?;
                self.push_block(json!({"type": "horizontalRule"}));
                Ok(())
            }
            Event::Html(html) | Event::InlineHtml(html) => {
                let trimmed = html.trim();
                if !trimmed.is_empty() {
                    self.note(
                        ImportWarningCode::MarkdownDegraded,
                        "HTML rendered as plain text",
                    );
                    self.push_text(trimmed)?;
                }
                Ok(())
            }
            Event::TaskListMarker(checked) => {
                if let Some(Frame::ListItem { task, .. }) = self.frames.last_mut() {
                    *task = Some(checked);
                }
                Ok(())
            }
            Event::InlineMath(latex) => self.push_math("inlineMath", &latex),
            Event::DisplayMath(latex) => self.push_math("blockMath", &latex),
            Event::FootnoteReference(_) => Ok(()),
        }
    }

    fn start_tag(&mut self, tag: Tag<'_>) -> Result<(), AppError> {
        match tag {
            Tag::Paragraph => self.push_frame(Frame::Paragraph(Vec::new()))?,
            Tag::Heading { level, .. } => self.start_heading(level)?,
            Tag::BlockQuote(_) => {
                self.enter_container()?;
                self.flush_list_item_inlines()?;
                self.push_frame(Frame::BlockQuote(Vec::new()))?;
            }
            Tag::CodeBlock(kind) => self.start_code_block(kind)?,
            Tag::List(start) => {
                self.enter_container()?;
                self.flush_list_item_inlines()?;
                self.push_frame(Frame::List {
                    ordered_start: start,
                    items: Vec::new(),
                })?;
            }
            Tag::Item => {
                self.enter_container()?;
                self.push_frame(Frame::ListItem {
                    task: None,
                    blocks: Vec::new(),
                    pending_inlines: Vec::new(),
                })?;
            }
            Tag::Table(alignments) => {
                self.enter_container()?;
                self.flush_list_item_inlines()?;
                let has_alignment = alignments
                    .iter()
                    .any(|align| !matches!(align, pulldown_cmark::Alignment::None));
                self.push_frame(Frame::Table {
                    has_alignment,
                    rows: Vec::new(),
                })?;
            }
            Tag::TableHead | Tag::TableRow => {
                self.bump_raw_inline()?;
                self.push_frame(Frame::TableRow {
                    is_header: matches!(tag, Tag::TableHead),
                    cells: Vec::new(),
                })?;
            }
            Tag::TableCell => {
                self.bump_raw_inline()?;
                self.push_frame(Frame::TableCell(Vec::new()))?;
            }
            Tag::Emphasis => {
                self.mark_counts[MARK_ITALIC] = self.mark_counts[MARK_ITALIC].saturating_add(1);
            }
            Tag::Strong => {
                self.mark_counts[MARK_BOLD] = self.mark_counts[MARK_BOLD].saturating_add(1);
            }
            Tag::Strikethrough => {
                self.mark_counts[MARK_STRIKE] = self.mark_counts[MARK_STRIKE].saturating_add(1);
            }
            Tag::Link {
                link_type,
                dest_url,
                title,
                ..
            } => self.push_frame(Frame::Link {
                dest: dest_url.into_string(),
                title_dropped: !title.is_empty(),
                wiki: matches!(link_type, LinkType::WikiLink { .. }),
                inlines: Vec::new(),
            })?,
            Tag::Image { dest_url, .. } => self.push_frame(Frame::Image {
                dest: dest_url.into_string(),
                alt: String::new(),
            })?,
            _ => {}
        }
        Ok(())
    }

    fn start_heading(&mut self, level: HeadingLevel) -> Result<(), AppError> {
        let raw = heading_level_num(level);
        if raw > 3 {
            self.note(
                ImportWarningCode::MarkdownDegraded,
                "heading level 4-6 clamped to level 3",
            );
        }
        self.push_frame(Frame::Heading {
            level: raw.min(3),
            inlines: Vec::new(),
        })
    }

    fn start_code_block(&mut self, kind: CodeBlockKind<'_>) -> Result<(), AppError> {
        self.flush_list_item_inlines()?;
        let language = match kind {
            CodeBlockKind::Fenced(info) => {
                let tag = info.split_whitespace().next().unwrap_or("");
                if tag.is_empty() {
                    None
                } else if is_valid_language(tag) {
                    Some(tag.to_owned())
                } else {
                    self.note(
                        ImportWarningCode::MarkdownDegraded,
                        "unsupported code block language dropped",
                    );
                    None
                }
            }
            CodeBlockKind::Indented => None,
        };
        self.push_frame(Frame::CodeBlock {
            language,
            text: String::new(),
        })
    }

    fn end_tag(&mut self, tag: TagEnd) -> Result<(), AppError> {
        match tag {
            TagEnd::Paragraph => self.finish_paragraph()?,
            TagEnd::Heading(_) => self.finish_heading()?,
            TagEnd::BlockQuote(_) => self.finish_blockquote()?,
            TagEnd::CodeBlock => self.finish_code_block()?,
            TagEnd::List(_) => self.finish_list()?,
            TagEnd::Item => self.finish_list_item()?,
            TagEnd::Table => self.finish_table()?,
            TagEnd::TableHead | TagEnd::TableRow => self.finish_table_row(),
            TagEnd::TableCell => self.finish_table_cell(),
            TagEnd::Emphasis => {
                self.mark_counts[MARK_ITALIC] = self.mark_counts[MARK_ITALIC].saturating_sub(1);
            }
            TagEnd::Strong => {
                self.mark_counts[MARK_BOLD] = self.mark_counts[MARK_BOLD].saturating_sub(1);
            }
            TagEnd::Strikethrough => {
                self.mark_counts[MARK_STRIKE] = self.mark_counts[MARK_STRIKE].saturating_sub(1);
            }
            TagEnd::Link => self.finish_link()?,
            TagEnd::Image => self.finish_image()?,
            _ => {}
        }
        Ok(())
    }

    fn finish_paragraph(&mut self) -> Result<(), AppError> {
        let Some(Frame::Paragraph(inlines)) = self.frames.pop() else {
            return Ok(());
        };
        let blocks = self.split_inlines_to_blocks(inlines)?;
        for block in blocks {
            self.push_block(block);
        }
        Ok(())
    }

    fn finish_heading(&mut self) -> Result<(), AppError> {
        let Some(Frame::Heading { level, inlines }) = self.frames.pop() else {
            return Ok(());
        };
        let (inline_nodes, extra_blocks) = partition_inlines(inlines);
        let merged = merge_text_nodes(inline_nodes);
        self.bump_nodes(1 + merged.len())?;
        let mut node = Map::new();
        node.insert("type".to_owned(), json!("heading"));
        node.insert("attrs".to_owned(), json!({"level": level}));
        if !merged.is_empty() {
            node.insert("content".to_owned(), Value::Array(merged));
        }
        self.push_block(Value::Object(node));
        for extra in extra_blocks {
            self.bump_nodes(1)?;
            self.push_block(extra);
        }
        Ok(())
    }

    fn finish_blockquote(&mut self) -> Result<(), AppError> {
        self.leave_container();
        let Some(Frame::BlockQuote(blocks)) = self.frames.pop() else {
            return Ok(());
        };
        if !blocks.is_empty() {
            self.bump_nodes(1)?;
            self.push_block(json!({"type": "blockquote", "content": blocks}));
        }
        Ok(())
    }

    fn finish_code_block(&mut self) -> Result<(), AppError> {
        let Some(Frame::CodeBlock { language, mut text }) = self.frames.pop() else {
            return Ok(());
        };
        if text.ends_with('\n') {
            text.pop();
        }
        self.bump_nodes(if text.is_empty() { 1 } else { 2 })?;
        let mut node = Map::new();
        node.insert("type".to_owned(), json!("codeBlock"));
        if let Some(language) = language {
            node.insert("attrs".to_owned(), json!({"language": language}));
        }
        if !text.is_empty() {
            node.insert(
                "content".to_owned(),
                json!([{"type": "text", "text": text}]),
            );
        }
        self.push_block(Value::Object(node));
        Ok(())
    }

    fn finish_list_item(&mut self) -> Result<(), AppError> {
        self.leave_container();
        self.flush_list_item_inlines()?;
        let Some(Frame::ListItem {
            task, mut blocks, ..
        }) = self.frames.pop()
        else {
            return Ok(());
        };
        if blocks
            .first()
            .and_then(|node| node.get("type"))
            .and_then(Value::as_str)
            != Some("paragraph")
        {
            self.bump_nodes(1)?;
            blocks.insert(0, json!({"type": "paragraph"}));
        }
        if let Some(Frame::List { items, .. }) = self.frames.last_mut() {
            items.push(ListItemDraft { task, blocks });
        }
        Ok(())
    }

    fn finish_list(&mut self) -> Result<(), AppError> {
        self.leave_container();
        let Some(Frame::List {
            ordered_start,
            items,
        }) = self.frames.pop()
        else {
            return Ok(());
        };
        if items.is_empty() {
            return Ok(());
        }
        let task_count = items.iter().filter(|item| item.task.is_some()).count();
        if task_count == items.len() {
            if ordered_start.is_some() {
                self.note(
                    ImportWarningCode::MarkdownDegraded,
                    "ordered task list converted to task list without numbering",
                );
            }
            self.bump_nodes(1 + items.len())?;
            let content: Vec<Value> = items
                .into_iter()
                .map(|item| {
                    json!({
                        "type": "taskItem",
                        "attrs": {"checked": item.task.unwrap_or(false)},
                        "content": item.blocks,
                    })
                })
                .collect();
            self.push_block(json!({"type": "taskList", "content": content}));
            return Ok(());
        }
        if task_count > 0 {
            self.note(
                ImportWarningCode::MarkdownDegraded,
                "mixed task and regular list converted to plain list",
            );
        }
        self.bump_nodes(1 + items.len())?;
        let content: Vec<Value> = items
            .into_iter()
            .map(|mut item| {
                if let Some(checked) = item.task {
                    prepend_prefix_to_first_paragraph(
                        &mut item.blocks,
                        if checked { "[x] " } else { "[ ] " },
                    );
                }
                json!({"type": "listItem", "content": item.blocks})
            })
            .collect();
        let list_node = match ordered_start {
            Some(start) => {
                let clamped = i32::try_from(start.min(1_000_000_000)).unwrap_or(1);
                json!({"type": "orderedList", "attrs": {"start": clamped}, "content": content})
            }
            None => json!({"type": "bulletList", "content": content}),
        };
        self.push_block(list_node);
        Ok(())
    }

    fn finish_table_row(&mut self) {
        let Some(Frame::TableRow { is_header, cells }) = self.frames.pop() else {
            return;
        };
        if let Some(Frame::Table { rows, .. }) = self.frames.last_mut() {
            rows.push(TableRowDraft { is_header, cells });
        }
    }

    fn finish_table_cell(&mut self) {
        let Some(Frame::TableCell(inlines)) = self.frames.pop() else {
            return;
        };
        if let Some(Frame::TableRow { cells, .. }) = self.frames.last_mut() {
            cells.push(inlines);
        }
    }

    fn finish_table(&mut self) -> Result<(), AppError> {
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
        let at_doc_root = matches!(self.frames.last(), Some(Frame::Root(_)));
        let fits_grid = at_doc_root
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

    fn finish_link(&mut self) -> Result<(), AppError> {
        let Some(Frame::Link {
            dest,
            title_dropped,
            wiki,
            inlines,
        }) = self.frames.pop()
        else {
            return Ok(());
        };
        if wiki && !is_id(&dest) {
            return self.push_text(&format!("[[{}]]", collect_plain_text(&inlines)));
        }
        if title_dropped {
            self.note(
                ImportWarningCode::MarkdownDegraded,
                "link title tooltip dropped",
            );
        }
        match self.resolver.resolve_link(&dest) {
            ResolvedTarget::InternalReference { block_id } if is_id(&block_id) => {
                self.bump_nodes(1)?;
                self.push_inline(InlineItem::Node(
                    json!({"type": "reference", "attrs": {"id": block_id}}),
                ))?;
            }
            ResolvedTarget::LocalFile { file_id } if self.context == BodyContext::Document => {
                self.bump_nodes(1)?;
                self.push_inline(InlineItem::BlockBreak(
                    json!({"type": "file", "attrs": {"fileId": file_id}}),
                ))?;
            }
            ResolvedTarget::Mention { user_id } => {
                self.drop_mention_sign();
                self.bump_nodes(1)?;
                self.push_inline(InlineItem::Node(
                    json!({"type": "mention", "attrs": {"userId": user_id}}),
                ))?;
            }
            ResolvedTarget::LocalFile { .. } => {
                self.note(
                    ImportWarningCode::MarkdownDegraded,
                    "file attachment links in comments rendered as plain text",
                );
                for item in inlines {
                    self.push_inline(item)?;
                }
            }
            ResolvedTarget::LocalImage { file_id } => {
                let alt = collect_plain_text(&inlines);
                self.bump_nodes(1)?;
                self.push_inline(InlineItem::BlockBreak(image_node(&file_id, &alt)))?;
            }
            ResolvedTarget::MissingLocal => {
                self.note(
                    ImportWarningCode::MissingLocalAsset,
                    format!(
                        "linked local file not found in bundle: {}",
                        truncate_chars(&dest, 120)
                    ),
                );
                for item in inlines {
                    self.push_inline(item)?;
                }
            }
            ResolvedTarget::ExternalUrl | ResolvedTarget::InternalReference { .. } => {
                if is_allowed_href(&dest) {
                    let link_mark = json!({"type": "link", "attrs": {"href": dest}});
                    for item in inlines {
                        self.push_inline(apply_link_mark(item, &link_mark))?;
                    }
                } else {
                    self.note(
                        ImportWarningCode::MarkdownDegraded,
                        "link with disallowed URL scheme dropped",
                    );
                    for item in inlines {
                        self.push_inline(item)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn finish_image(&mut self) -> Result<(), AppError> {
        let Some(Frame::Image { dest, alt }) = self.frames.pop() else {
            return Ok(());
        };
        match self.resolver.resolve_image(&dest) {
            ResolvedTarget::LocalImage { file_id } => {
                self.bump_nodes(1)?;
                self.push_inline(InlineItem::BlockBreak(image_node(&file_id, &alt)))?;
            }
            ResolvedTarget::LocalFile { file_id } if self.context == BodyContext::Document => {
                self.bump_nodes(1)?;
                self.push_inline(InlineItem::BlockBreak(
                    json!({"type": "file", "attrs": {"fileId": file_id}}),
                ))?;
            }
            ResolvedTarget::ExternalUrl => {
                let (node, kept_link) = linked_text_node(&alt, &dest);
                if kept_link {
                    self.note(
                        ImportWarningCode::ExternalImageLinked,
                        format!(
                            "external image URL kept as link without fetching: {}",
                            truncate_chars(&dest, 120)
                        ),
                    );
                    let len = node.get("text").and_then(Value::as_str).map_or(0, str::len);
                    self.bump_text_bytes(len)?;
                    self.bump_nodes(1)?;
                    self.push_inline(InlineItem::Node(node))?;
                } else {
                    self.note(
                        ImportWarningCode::MarkdownDegraded,
                        "external image with disallowed URL scheme dropped",
                    );
                    let safe_label = if alt.trim().is_empty() {
                        "[image]"
                    } else {
                        alt.trim()
                    };
                    self.bump_text_bytes(safe_label.len())?;
                    self.bump_nodes(1)?;
                    self.push_inline(InlineItem::Node(
                        json!({"type": "text", "text": safe_label}),
                    ))?;
                }
            }
            ResolvedTarget::MissingLocal
            | ResolvedTarget::InternalReference { .. }
            | ResolvedTarget::Mention { .. }
            | ResolvedTarget::LocalFile { .. } => {
                self.note(
                    ImportWarningCode::MissingLocalAsset,
                    format!(
                        "referenced image not found in bundle: {}",
                        truncate_chars(&dest, 120)
                    ),
                );
                let label = if alt.is_empty() { dest } else { alt };
                if !label.is_empty() {
                    self.push_text(&label)?;
                }
            }
        }
        Ok(())
    }

    fn push_text(&mut self, text: &str) -> Result<(), AppError> {
        if text.is_empty() {
            return Ok(());
        }
        self.bump_text_bytes(text.len())?;
        match self.frames.last_mut() {
            Some(Frame::CodeBlock { text: buf, .. }) => {
                buf.push_str(text);
                return Ok(());
            }
            Some(Frame::Image { alt, .. }) => {
                alt.push_str(text);
                return Ok(());
            }
            _ => {}
        }
        self.bump_raw_inline()?;
        let marks = active_mark_values(self.mark_counts);
        let node = if marks.is_empty() {
            json!({"type": "text", "text": text})
        } else {
            json!({"type": "text", "text": text, "marks": marks})
        };
        self.push_inline(InlineItem::Node(node))
    }

    fn push_inline_code(&mut self, code: &str) -> Result<(), AppError> {
        if code.is_empty() {
            return Ok(());
        }
        self.bump_text_bytes(code.len())?;
        if let Some(Frame::Image { alt, .. }) = self.frames.last_mut() {
            alt.push_str(code);
            return Ok(());
        }
        self.bump_raw_inline()?;
        if self.mark_counts.iter().any(|&count| count > 0) {
            self.note(
                ImportWarningCode::MarkdownDegraded,
                "formatting marks combined with inline code dropped",
            );
        }
        self.push_inline(InlineItem::Node(json!({
            "type": "text",
            "text": code,
            "marks": [{"type": "code"}],
        })))
    }

    fn push_break(&mut self) -> Result<(), AppError> {
        if matches!(self.frames.last(), Some(Frame::CodeBlock { .. })) {
            self.bump_text_bytes(1)?;
            if let Some(Frame::CodeBlock { text, .. }) = self.frames.last_mut() {
                text.push('\n');
            }
            return Ok(());
        }
        if matches!(self.frames.last(), Some(Frame::Image { .. })) {
            self.bump_text_bytes(1)?;
            if let Some(Frame::Image { alt, .. }) = self.frames.last_mut() {
                alt.push(' ');
            }
            return Ok(());
        }
        self.bump_nodes(1)?;
        self.push_inline(InlineItem::Node(json!({"type": "hardBreak"})))
    }

    fn push_inline(&mut self, item: InlineItem) -> Result<(), AppError> {
        if matches!(
            self.frames.last(),
            Some(Frame::Root(_) | Frame::BlockQuote(_))
        ) {
            let added = match &item {
                InlineItem::BlockBreak(_) => 1,
                InlineItem::Node(_) => 2,
            };
            self.bump_nodes(added)?;
        }
        match self.frames.last_mut() {
            Some(
                Frame::Paragraph(inlines)
                | Frame::Heading { inlines, .. }
                | Frame::TableCell(inlines)
                | Frame::Link { inlines, .. }
                | Frame::ListItem {
                    pending_inlines: inlines,
                    ..
                },
            ) => inlines.push(item),
            Some(Frame::Root(blocks) | Frame::BlockQuote(blocks)) => match item {
                InlineItem::BlockBreak(block) => blocks.push(block),
                InlineItem::Node(node) => {
                    blocks.push(json!({"type": "paragraph", "content": [node]}));
                }
            },
            _ => {}
        }
        Ok(())
    }

    fn push_block(&mut self, block: Value) {
        if let Some(
            Frame::Root(blocks) | Frame::BlockQuote(blocks) | Frame::ListItem { blocks, .. },
        ) = self.frames.last_mut()
        {
            blocks.push(block);
        }
    }

    fn flush_list_item_inlines(&mut self) -> Result<(), AppError> {
        let pending = match self.frames.last_mut() {
            Some(Frame::ListItem {
                pending_inlines, ..
            }) if !pending_inlines.is_empty() => std::mem::take(pending_inlines),
            _ => return Ok(()),
        };
        let blocks = self.split_inlines_to_blocks(pending)?;
        if let Some(Frame::ListItem {
            blocks: item_blocks,
            ..
        }) = self.frames.last_mut()
        {
            item_blocks.extend(blocks);
        }
        Ok(())
    }

    fn split_inlines_to_blocks(
        &mut self,
        inlines: Vec<InlineItem>,
    ) -> Result<Vec<Value>, AppError> {
        let mut blocks = Vec::new();
        let mut current: Vec<Value> = Vec::new();
        for item in inlines {
            match item {
                InlineItem::Node(node) => current.push(node),
                InlineItem::BlockBreak(block) => {
                    if !current.is_empty() {
                        let merged = merge_text_nodes(std::mem::take(&mut current));
                        if !merged.is_empty() {
                            self.bump_nodes(1 + merged.len())?;
                            blocks.push(json!({"type": "paragraph", "content": merged}));
                        }
                    }
                    blocks.push(block);
                }
            }
        }
        if !current.is_empty() {
            let merged = merge_text_nodes(current);
            if !merged.is_empty() {
                self.bump_nodes(1 + merged.len())?;
                blocks.push(json!({"type": "paragraph", "content": merged}));
            }
        }
        Ok(blocks)
    }

    fn finish(mut self) -> Result<MarkdownBlocks, AppError> {
        if let Some(Frame::Root(nodes)) = self.frames.drain(..).next() {
            Ok(MarkdownBlocks {
                nodes,
                notices: self.notices,
            })
        } else {
            Err(AppError::internal("markdown converter lost root frame"))
        }
    }
}
