//! ODT / ODP / ODS body text from `content.xml` (source `officeparser`;
//! notes, annotations, tracked deletions and embedded objects are skipped).

use std::collections::HashMap;

use super::render::{Block, Output, Span, MAX_TABLE_COLUMNS};
use super::xml::{attr, local, walk, Xml};
use super::Stop;

/// Repeated rows of an ODS row (`number-rows-repeated`) kept at most; real
/// sheets repeat empty rows to the sheet end, which are dropped anyway.
const MAX_ROW_REPEAT: usize = 64;
/// Spaces a single `text:s c="…"` may expand to.
const MAX_SPACE_RUN: usize = 64;

fn skipped(name: &[u8]) -> bool {
    matches!(
        name,
        b"note" | b"annotation" | b"tracked-changes" | b"notes" | b"object" | b"forms"
    )
}

#[derive(Default, Clone, Copy)]
struct Fmt {
    bold: bool,
    italic: bool,
}

#[derive(Default)]
struct Para {
    heading: Option<u8>,
    list_depth: Option<u8>,
    title: bool,
    spans: Vec<Span>,
}

struct Cell {
    text: String,
    repeat: usize,
}

#[derive(Default)]
struct TableBuild {
    rows: Vec<Vec<String>>,
    row: Vec<String>,
    row_repeat: usize,
    cell: Option<Cell>,
}

impl TableBuild {
    fn flatten(&self) -> String {
        self.rows
            .iter()
            .map(|row| row.join(" "))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

fn repeat_attr(e: &quick_xml::events::BytesStart<'_>, name: &[u8]) -> usize {
    attr(e, name)
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(1)
        .max(1)
}

/// Expected `mimetype` entry of each flavor.
pub fn mimetype_for(kind: super::OfficeKind) -> &'static str {
    match kind {
        super::OfficeKind::Odt => "application/vnd.oasis.opendocument.text",
        super::OfficeKind::Odp => "application/vnd.oasis.opendocument.presentation",
        _ => "application/vnd.oasis.opendocument.spreadsheet",
    }
}

pub fn odf(
    parts: &HashMap<String, Vec<u8>>,
    kind: super::OfficeKind,
    out: &mut Output,
) -> Result<(), Stop> {
    if let Some(mimetype) = parts.get("mimetype") {
        if String::from_utf8_lossy(mimetype).trim() != mimetype_for(kind) {
            return Err(Stop::Unsupported("odf mimetype does not match".into()));
        }
    }
    let content = parts
        .get("content.xml")
        .ok_or_else(|| Stop::Unsupported("odf without content.xml".into()))?;
    let spreadsheet = kind == super::OfficeKind::Ods;

    let mut styles: HashMap<String, Fmt> = HashMap::new();
    let mut style_name: Option<String> = None;
    let mut skip = 0usize;
    let mut body = false;
    let mut paras: Vec<Para> = Vec::new();
    let mut fmts: Vec<Fmt> = Vec::new();
    let mut lists = 0u8;
    let mut title_frames = 0usize;
    let mut frames: Vec<bool> = Vec::new();
    let mut tables: Vec<TableBuild> = Vec::new();

    let push_text = |paras: &mut Vec<Para>, fmts: &[Fmt], text: &str| {
        if let Some(para) = paras.last_mut() {
            let fmt = fmts.last().copied().unwrap_or_default();
            para.spans.push(Span {
                text: text.to_string(),
                bold: fmt.bold,
                italic: fmt.italic,
            });
        }
    };

    walk(
        content,
        |event| {
            // Skipped subtrees.
            match &event {
                Xml::Start(e) if skip > 0 || skipped(&local(e)) => {
                    skip += 1;
                    return Ok(());
                }
                Xml::End(_) if skip > 0 => {
                    skip -= 1;
                    return Ok(());
                }
                _ if skip > 0 => return Ok(()),
                _ => {}
            }
            let is_empty = matches!(event, Xml::Empty(_));
            match event {
                Xml::Start(e) | Xml::Empty(e) => {
                    let name = local(&e);
                    if !body {
                        // Automatic styles: bold / italic text styles.
                        match name.as_slice() {
                            b"body" => body = true,
                            b"style" => style_name = attr(&e, b"name"),
                            b"text-properties" => {
                                if let Some(style) = style_name.clone() {
                                    let bold = attr(&e, b"font-weight").is_some_and(|w| {
                                        w == "bold" || w.parse::<u32>().is_ok_and(|n| n >= 600)
                                    });
                                    let italic = attr(&e, b"font-style")
                                        .is_some_and(|s| s == "italic" || s == "oblique");
                                    styles.insert(style, Fmt { bold, italic });
                                }
                            }
                            _ => {}
                        }
                        return Ok(());
                    }
                    match name.as_slice() {
                        // A self-closing paragraph, span, list or frame has
                        // no content and no End event.
                        b"p" | b"h" | b"span" | b"list" | b"frame" | b"table" | b"table-row"
                            if is_empty => {}
                        b"p" | b"h" => {
                            let heading = (name.as_slice() == b"h").then(|| {
                                attr(&e, b"outline-level")
                                    .and_then(|v| v.parse::<u8>().ok())
                                    .unwrap_or(1)
                                    .clamp(1, 6)
                            });
                            let fmt = attr(&e, b"style-name")
                                .and_then(|s| styles.get(&s).copied())
                                .unwrap_or_default();
                            fmts.push(fmt);
                            paras.push(Para {
                                heading,
                                list_depth: (lists > 0).then(|| lists - 1),
                                title: title_frames > 0,
                                spans: Vec::new(),
                            });
                        }
                        b"span" => {
                            let parent = fmts.last().copied().unwrap_or_default();
                            let own = attr(&e, b"style-name").and_then(|s| styles.get(&s).copied());
                            fmts.push(match own {
                                Some(f) => Fmt {
                                    bold: parent.bold || f.bold,
                                    italic: parent.italic || f.italic,
                                },
                                None => parent,
                            });
                        }
                        b"list" => lists = lists.saturating_add(1),
                        b"s" => {
                            let count = attr(&e, b"c")
                                .and_then(|v| v.parse::<usize>().ok())
                                .unwrap_or(1)
                                .min(MAX_SPACE_RUN);
                            push_text(&mut paras, &fmts, &" ".repeat(count));
                        }
                        b"tab" => push_text(&mut paras, &fmts, "\t"),
                        b"line-break" => push_text(&mut paras, &fmts, "\n"),
                        b"frame" => {
                            let title = attr(&e, b"class").as_deref() == Some("title");
                            frames.push(title);
                            if title {
                                title_frames += 1;
                            }
                        }
                        b"table" => {
                            if spreadsheet && tables.is_empty() {
                                if let Some(name) = attr(&e, b"name") {
                                    out.block(Block::Heading(
                                        2,
                                        vec![Span {
                                            text: name,
                                            ..Span::default()
                                        }],
                                    ))
                                    .map_err(|_| Stop::Full)?;
                                }
                            }
                            tables.push(TableBuild::default());
                        }
                        b"table-row" => {
                            if let Some(t) = tables.last_mut() {
                                t.row = Vec::new();
                                t.row_repeat = repeat_attr(&e, b"number-rows-repeated");
                            }
                        }
                        b"table-cell" | b"covered-table-cell" => {
                            if let Some(t) = tables.last_mut() {
                                t.cell = Some(Cell {
                                    text: String::new(),
                                    repeat: repeat_attr(&e, b"number-columns-repeated"),
                                });
                            }
                        }
                        _ => {}
                    }
                    // A self-closing cell has no End event: close it now.
                    if is_empty && matches!(&name[..], b"table-cell" | b"covered-table-cell") {
                        close_cell(&mut tables);
                    }
                }
                Xml::End(name) if body => match name.as_slice() {
                    b"p" | b"h" => {
                        fmts.pop();
                        let Some(para) = paras.pop() else {
                            return Ok(());
                        };
                        if let Some(table) = tables.last_mut() {
                            if let Some(cell) = table.cell.as_mut() {
                                let text: String =
                                    para.spans.iter().map(|s| s.text.as_str()).collect();
                                let text = text.trim();
                                if !text.is_empty() {
                                    if !cell.text.is_empty() {
                                        cell.text.push(' ');
                                    }
                                    cell.text.push_str(text);
                                }
                            }
                            return Ok(());
                        }
                        let block = match (para.heading, para.list_depth) {
                            _ if para.title => Block::Heading(2, para.spans),
                            (Some(level), _) => Block::Heading(level, para.spans),
                            (None, Some(depth)) => Block::ListItem(depth, para.spans),
                            (None, None) => Block::Paragraph(para.spans),
                        };
                        out.block(block).map_err(|_| Stop::Full)?;
                    }
                    b"span" => {
                        fmts.pop();
                    }
                    b"list" => lists = lists.saturating_sub(1),
                    b"frame" => {
                        if frames.pop() == Some(true) {
                            title_frames = title_frames.saturating_sub(1);
                        }
                    }
                    b"table-cell" | b"covered-table-cell" => close_cell(&mut tables),
                    b"table-row" => close_row(&mut tables),
                    b"table" => {
                        let Some(table) = tables.pop() else {
                            return Ok(());
                        };
                        if let Some(outer) = tables.last_mut() {
                            let text = table.flatten();
                            if let Some(cell) = outer.cell.as_mut() {
                                if !cell.text.is_empty() {
                                    cell.text.push(' ');
                                }
                                cell.text.push_str(text.trim());
                            }
                        } else {
                            out.block(Block::Table(table.rows))
                                .map_err(|_| Stop::Full)?;
                        }
                    }
                    _ => {}
                },
                Xml::End(name) => {
                    if name == b"style" {
                        style_name = None;
                    }
                }
                Xml::Text(text) => {
                    if body {
                        push_text(&mut paras, &fmts, &text);
                    }
                }
            }
            Ok(())
        },
        Stop::Corrupt,
    )
}

fn close_cell(tables: &mut [TableBuild]) {
    let Some(t) = tables.last_mut() else {
        return;
    };
    let Some(cell) = t.cell.take() else {
        return;
    };
    let room = MAX_TABLE_COLUMNS.saturating_sub(t.row.len());
    // A long run of empty repeated cells only matters if content follows it;
    // trailing empties are trimmed at render time.
    let repeat = cell.repeat.min(room);
    for _ in 0..repeat {
        t.row.push(cell.text.clone());
    }
}

fn close_row(tables: &mut [TableBuild]) {
    let Some(t) = tables.last_mut() else {
        return;
    };
    let row = std::mem::take(&mut t.row);
    if row.iter().any(|c| !c.trim().is_empty()) {
        for _ in 0..t.row_repeat.min(MAX_ROW_REPEAT) {
            t.rows.push(row.clone());
        }
    }
}
