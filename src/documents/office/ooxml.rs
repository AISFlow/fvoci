//! DOCX / PPTX / XLSX body text (source `officeparser`, OOXML parts only:
//! no embedded objects, OCR, comments or speaker notes).

use std::collections::HashMap;

use super::render::{Block, Output, Span, MAX_TABLE_COLUMNS};
use super::xml::{attr, attr_qualified, local, relationships, walk, Xml};
use super::Stop;

fn flag_on(e: &quick_xml::events::BytesStart<'_>) -> bool {
    !matches!(
        attr(e, b"val").as_deref(),
        Some("0") | Some("false") | Some("off") | Some("none")
    )
}

fn plain(spans: &[Span]) -> String {
    spans.iter().map(|s| s.text.as_str()).collect()
}

/// Nested elements whose text is not body text (alternate renderings,
/// deleted text, field codes).
fn skipped(name: &[u8]) -> bool {
    matches!(name, b"Fallback" | b"delText" | b"instrText" | b"rPh")
}

#[derive(Default)]
struct Skip(usize);

impl Skip {
    /// Returns true while inside a skipped subtree.
    fn event(&mut self, event: &Xml<'_>) -> bool {
        match event {
            Xml::Start(e) if self.0 > 0 || skipped(&local(e)) => {
                self.0 += 1;
                true
            }
            Xml::End(_) if self.0 > 0 => {
                self.0 -= 1;
                true
            }
            _ => self.0 > 0,
        }
    }
}

#[derive(Default)]
struct TableBuild {
    rows: Vec<Vec<String>>,
    row: Vec<String>,
    cell: Option<String>,
}

impl TableBuild {
    fn append_cell_text(&mut self, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        if let Some(cell) = self.cell.as_mut() {
            if !cell.is_empty() {
                cell.push(' ');
            }
            cell.push_str(text);
        }
    }

    fn flatten(self) -> String {
        self.rows
            .iter()
            .map(|row| row.join(" "))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// `heading N` / `Title` style names or an outline level → heading level.
fn heading_from_style_name(name: &str) -> Option<u8> {
    let lower = name.to_ascii_lowercase();
    if lower == "title" {
        return Some(1);
    }
    if lower == "subtitle" {
        return Some(2);
    }
    let digits = lower
        .strip_prefix("heading")
        .map(|rest| rest.trim_start_matches([' ', '_']))?;
    let level: u8 = digits.parse().ok()?;
    (1..=9).contains(&level).then_some(level.min(6))
}

/// `word/styles.xml`: paragraph style id → heading level.
fn docx_heading_styles(styles: &[u8]) -> HashMap<String, u8> {
    let mut out = HashMap::new();
    let mut current: Option<String> = None;
    let _ = walk(
        styles,
        |event| {
            match event {
                Xml::Start(e) if local(&e) == b"style" => current = attr(&e, b"styleId"),
                Xml::Start(e) | Xml::Empty(e) => {
                    let Some(id) = current.clone() else {
                        return Ok(());
                    };
                    match local(&e).as_slice() {
                        b"name" => {
                            if let Some(level) = attr(&e, b"val")
                                .as_deref()
                                .and_then(heading_from_style_name)
                            {
                                out.insert(id, level);
                            }
                        }
                        b"outlineLvl" => {
                            if let Some(level) = attr(&e, b"val").and_then(|v| v.parse::<u8>().ok())
                            {
                                if level < 9 {
                                    out.entry(id).or_insert((level + 1).min(6));
                                }
                            }
                        }
                        _ => {}
                    }
                }
                Xml::End(name) if name == b"style" => current = None,
                _ => {}
            }
            Ok::<(), ()>(())
        },
        |_| (),
    );
    out
}

#[derive(Default)]
struct Para {
    heading: Option<u8>,
    list: Option<u8>,
    spans: Vec<Span>,
}

pub fn docx(parts: &HashMap<String, Vec<u8>>, out: &mut Output) -> Result<(), Stop> {
    let document = parts
        .get("word/document.xml")
        .ok_or_else(|| Stop::Unsupported("docx without word/document.xml".into()))?;
    let styles = parts
        .get("word/styles.xml")
        .map(|s| docx_heading_styles(s))
        .unwrap_or_default();
    let mut skip = Skip::default();
    let mut paras: Vec<Para> = Vec::new();
    let mut tables: Vec<TableBuild> = Vec::new();
    let (mut in_text, mut in_run, mut in_numpr) = (false, false, false);
    let (mut bold, mut italic) = (false, false);
    walk(
        document,
        |event| {
            if skip.event(&event) {
                return Ok(());
            }
            let is_empty = matches!(event, Xml::Empty(_));
            match event {
                Xml::Start(e) | Xml::Empty(e) => {
                    match local(&e).as_slice() {
                        // Self-closing containers have no content and no End.
                        b"p" | b"r" | b"t" | b"tbl" | b"tr" | b"tc" if is_empty => {}
                        b"p" => paras.push(Para::default()),
                        b"pStyle" => {
                            if let (Some(para), Some(id)) = (paras.last_mut(), attr(&e, b"val")) {
                                para.heading = styles
                                    .get(&id)
                                    .copied()
                                    .or_else(|| heading_from_style_name(&id));
                            }
                        }
                        b"outlineLvl" => {
                            if let (Some(para), Some(level)) = (
                                paras.last_mut(),
                                attr(&e, b"val").and_then(|v| v.parse::<u8>().ok()),
                            ) {
                                if level < 9 && para.heading.is_none() {
                                    para.heading = Some((level + 1).min(6));
                                }
                            }
                        }
                        b"numPr" => {
                            in_numpr = !is_empty;
                            if let Some(para) = paras.last_mut() {
                                para.list.get_or_insert(0);
                            }
                        }
                        b"ilvl" if in_numpr => {
                            if let (Some(para), Some(level)) = (
                                paras.last_mut(),
                                attr(&e, b"val").and_then(|v| v.parse::<u8>().ok()),
                            ) {
                                para.list = Some(level.min(8));
                            }
                        }
                        b"r" => {
                            in_run = true;
                            bold = false;
                            italic = false;
                        }
                        b"b" if in_run => bold = flag_on(&e),
                        b"i" if in_run => italic = flag_on(&e),
                        b"t" => in_text = true,
                        b"tab" if in_run => push_text(&mut paras, "\t", bold, italic),
                        b"br" | b"cr" if in_run => push_text(&mut paras, "\n", bold, italic),
                        b"tbl" => tables.push(TableBuild::default()),
                        b"tr" => {
                            if let Some(t) = tables.last_mut() {
                                t.row = Vec::new();
                            }
                        }
                        b"tc" => {
                            if let Some(t) = tables.last_mut() {
                                t.cell = Some(String::new());
                            }
                        }
                        _ => {}
                    }
                }
                Xml::End(name) => match name.as_slice() {
                    b"t" => in_text = false,
                    b"r" => in_run = false,
                    b"numPr" => in_numpr = false,
                    b"p" => {
                        let Some(para) = paras.pop() else {
                            return Ok(());
                        };
                        if let Some(table) = tables.last_mut() {
                            table.append_cell_text(&plain(&para.spans));
                            return Ok(());
                        }
                        let block = match (para.heading, para.list) {
                            (Some(level), _) => Block::Heading(level, para.spans),
                            (None, Some(depth)) => Block::ListItem(depth, para.spans),
                            (None, None) => Block::Paragraph(para.spans),
                        };
                        out.block(block).map_err(|_| Stop::Full)?;
                    }
                    b"tc" => {
                        if let Some(t) = tables.last_mut() {
                            if let Some(cell) = t.cell.take() {
                                if t.row.len() < MAX_TABLE_COLUMNS {
                                    t.row.push(cell);
                                }
                            }
                        }
                    }
                    b"tr" => {
                        if let Some(t) = tables.last_mut() {
                            let row = std::mem::take(&mut t.row);
                            t.rows.push(row);
                        }
                    }
                    b"tbl" => {
                        let Some(table) = tables.pop() else {
                            return Ok(());
                        };
                        if let Some(outer) = tables.last_mut() {
                            outer.append_cell_text(&table.flatten());
                        } else {
                            out.block(Block::Table(table.rows))
                                .map_err(|_| Stop::Full)?;
                        }
                    }
                    _ => {}
                },
                Xml::Text(text) if in_text => push_text(&mut paras, &text, bold, italic),
                Xml::Text(_) => {}
            }
            Ok(())
        },
        Stop::Corrupt,
    )
}

fn push_text(paras: &mut [Para], text: &str, bold: bool, italic: bool) {
    if let Some(para) = paras.last_mut() {
        para.spans.push(Span {
            text: text.to_string(),
            bold,
            italic,
        });
    }
}

fn slide_number(name: &str) -> Option<u32> {
    name.strip_prefix("ppt/slides/slide")?
        .strip_suffix(".xml")?
        .parse()
        .ok()
}

/// Slide parts in presentation order (`p:sldIdLst`), falling back to the
/// numeric part names when the list cannot be resolved.
fn slide_order(parts: &HashMap<String, Vec<u8>>) -> Vec<String> {
    let rels = parts
        .get("ppt/_rels/presentation.xml.rels")
        .map(|r| relationships(r, "ppt"))
        .unwrap_or_default();
    let mut ordered = Vec::new();
    if let Some(presentation) = parts.get("ppt/presentation.xml") {
        let _ = walk(
            presentation,
            |event| {
                if let Xml::Start(e) | Xml::Empty(e) = event {
                    if local(&e) == b"sldId" {
                        let id = attr_qualified(&e, b"r:id").or_else(|| attr(&e, b"id"));
                        if let Some(target) = id.and_then(|id| rels.get(&id)) {
                            if parts.contains_key(target) {
                                ordered.push(target.clone());
                            }
                        }
                    }
                }
                Ok::<(), ()>(())
            },
            |_| (),
        );
    }
    if ordered.is_empty() {
        let mut numbered: Vec<(u32, String)> = parts
            .keys()
            .filter_map(|name| slide_number(name).map(|n| (n, name.clone())))
            .collect();
        numbered.sort();
        ordered = numbered.into_iter().map(|(_, name)| name).collect();
    }
    ordered
}

pub fn pptx(parts: &HashMap<String, Vec<u8>>, out: &mut Output) -> Result<(), Stop> {
    if !parts.contains_key("ppt/presentation.xml") {
        return Err(Stop::Unsupported(
            "pptx without ppt/presentation.xml".into(),
        ));
    }
    for slide in slide_order(parts) {
        let Some(xml) = parts.get(&slide) else {
            continue;
        };
        pptx_slide(xml, out)?;
    }
    Ok(())
}

fn pptx_slide(xml: &[u8], out: &mut Output) -> Result<(), Stop> {
    let mut skip = Skip::default();
    let mut title_shape = false;
    let mut paras: Vec<Para> = Vec::new();
    let mut tables: Vec<TableBuild> = Vec::new();
    let (mut in_text, mut in_run) = (false, false);
    let (mut bold, mut italic) = (false, false);
    walk(
        xml,
        |event| {
            if skip.event(&event) {
                return Ok(());
            }
            let is_empty = matches!(event, Xml::Empty(_));
            match event {
                Xml::Start(e) | Xml::Empty(e) => match local(&e).as_slice() {
                    b"p" | b"r" | b"fld" | b"t" | b"tbl" | b"tr" | b"tc" | b"sp" if is_empty => {}
                    b"sp" => title_shape = false,
                    b"ph" => {
                        title_shape = matches!(
                            attr(&e, b"type").as_deref(),
                            Some("title") | Some("ctrTitle")
                        )
                    }
                    b"p" => paras.push(Para::default()),
                    b"r" | b"fld" => {
                        in_run = true;
                        bold = false;
                        italic = false;
                    }
                    b"rPr" if in_run => {
                        bold = attr(&e, b"b").is_some_and(|v| v == "1" || v == "true");
                        italic = attr(&e, b"i").is_some_and(|v| v == "1" || v == "true");
                    }
                    b"t" => in_text = true,
                    b"br" => push_text(&mut paras, "\n", false, false),
                    b"tbl" => tables.push(TableBuild::default()),
                    b"tr" => {
                        if let Some(t) = tables.last_mut() {
                            t.row = Vec::new();
                        }
                    }
                    b"tc" => {
                        if let Some(t) = tables.last_mut() {
                            t.cell = Some(String::new());
                        }
                    }
                    _ => {}
                },
                Xml::End(name) => match name.as_slice() {
                    b"t" => in_text = false,
                    b"r" | b"fld" => in_run = false,
                    b"sp" => title_shape = false,
                    b"p" => {
                        let Some(para) = paras.pop() else {
                            return Ok(());
                        };
                        if let Some(table) = tables.last_mut() {
                            table.append_cell_text(&plain(&para.spans));
                            return Ok(());
                        }
                        let block = if title_shape {
                            Block::Heading(2, para.spans)
                        } else {
                            Block::Paragraph(para.spans)
                        };
                        out.block(block).map_err(|_| Stop::Full)?;
                    }
                    b"tc" => {
                        if let Some(t) = tables.last_mut() {
                            if let Some(cell) = t.cell.take() {
                                if t.row.len() < MAX_TABLE_COLUMNS {
                                    t.row.push(cell);
                                }
                            }
                        }
                    }
                    b"tr" => {
                        if let Some(t) = tables.last_mut() {
                            let row = std::mem::take(&mut t.row);
                            t.rows.push(row);
                        }
                    }
                    b"tbl" => {
                        let Some(table) = tables.pop() else {
                            return Ok(());
                        };
                        if let Some(outer) = tables.last_mut() {
                            outer.append_cell_text(&table.flatten());
                        } else {
                            out.block(Block::Table(table.rows))
                                .map_err(|_| Stop::Full)?;
                        }
                    }
                    _ => {}
                },
                Xml::Text(text) if in_text => push_text(&mut paras, &text, bold, italic),
                Xml::Text(_) => {}
            }
            Ok(())
        },
        Stop::Corrupt,
    )
}

fn shared_strings(xml: &[u8], budget: usize) -> Result<Vec<String>, Stop> {
    let mut out = Vec::new();
    let mut skip = Skip::default();
    let mut current: Option<String> = None;
    let mut in_text = false;
    let mut total = 0usize;
    walk(
        xml,
        |event| {
            if skip.event(&event) {
                return Ok(());
            }
            match event {
                Xml::Start(e) => match local(&e).as_slice() {
                    b"si" => current = Some(String::new()),
                    b"t" => in_text = true,
                    _ => {}
                },
                Xml::Empty(e) if local(&e) == b"si" => out.push(String::new()),
                Xml::End(name) => match name.as_slice() {
                    b"si" => out.push(current.take().unwrap_or_default()),
                    b"t" => in_text = false,
                    _ => {}
                },
                Xml::Text(text) if in_text => {
                    total += text.len();
                    // A string table larger than the whole output budget
                    // cannot render; stop before holding it all.
                    if total > budget.saturating_mul(4).max(1 << 20) {
                        return Err(Stop::Full);
                    }
                    if let Some(s) = current.as_mut() {
                        s.push_str(&text);
                    }
                }
                _ => {}
            }
            Ok(())
        },
        Stop::Corrupt,
    )?;
    Ok(out)
}

/// `B12` → column index 1.
fn column_index(reference: &str) -> Option<usize> {
    let letters: String = reference
        .chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect();
    if letters.is_empty() || letters.len() > 3 {
        return None;
    }
    let mut index = 0usize;
    for c in letters.chars() {
        index = index * 26 + (c.to_ascii_uppercase() as usize - 'A' as usize + 1);
    }
    Some(index - 1)
}

pub fn xlsx(parts: &HashMap<String, Vec<u8>>, out: &mut Output, budget: usize) -> Result<(), Stop> {
    let workbook = parts
        .get("xl/workbook.xml")
        .ok_or_else(|| Stop::Unsupported("xlsx without xl/workbook.xml".into()))?;
    let rels = parts
        .get("xl/_rels/workbook.xml.rels")
        .map(|r| relationships(r, "xl"))
        .unwrap_or_default();
    let strings = match parts.get("xl/sharedStrings.xml") {
        Some(xml) => shared_strings(xml, budget)?,
        None => Vec::new(),
    };
    let mut sheets: Vec<(String, String)> = Vec::new();
    walk(
        workbook,
        |event| {
            if let Xml::Start(e) | Xml::Empty(e) = event {
                if local(&e) == b"sheet" {
                    let name = attr(&e, b"name").unwrap_or_default();
                    let id = attr_qualified(&e, b"r:id").or_else(|| attr(&e, b"id"));
                    if let Some(target) = id.and_then(|id| rels.get(&id)) {
                        sheets.push((name, target.clone()));
                    }
                }
            }
            Ok(())
        },
        Stop::Corrupt,
    )?;
    for (name, part) in sheets {
        let Some(xml) = parts.get(&part) else {
            continue;
        };
        let rows = xlsx_rows(xml, &strings, budget)?;
        if rows
            .iter()
            .all(|row| row.iter().all(|c| c.trim().is_empty()))
        {
            continue;
        }
        out.block(Block::Heading(
            2,
            vec![Span {
                text: name,
                ..Span::default()
            }],
        ))
        .map_err(|_| Stop::Full)?;
        out.block(Block::Table(rows)).map_err(|_| Stop::Full)?;
    }
    Ok(())
}

fn xlsx_rows(xml: &[u8], strings: &[String], budget: usize) -> Result<Vec<Vec<String>>, Stop> {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut cell: Option<(usize, Option<String>, String)> = None;
    let (mut in_value, mut in_inline) = (false, false);
    let mut size = 0usize;
    let mut skip = Skip::default();
    walk(
        xml,
        |event| {
            if skip.event(&event) {
                return Ok(());
            }
            match event {
                Xml::Start(e) | Xml::Empty(e) if local(&e) == b"row" => row = Vec::new(),
                Xml::Start(e) => match local(&e).as_slice() {
                    b"c" => {
                        let col = attr(&e, b"r")
                            .and_then(|r| column_index(&r))
                            .unwrap_or(row.len());
                        cell = Some((col, attr(&e, b"t"), String::new()));
                    }
                    b"v" => in_value = true,
                    b"is" => in_inline = true,
                    _ => {}
                },
                Xml::End(name) => match name.as_slice() {
                    b"v" => in_value = false,
                    b"is" => in_inline = false,
                    b"c" => {
                        if let Some((col, kind, raw)) = cell.take() {
                            let value = match kind.as_deref() {
                                Some("s") => raw
                                    .trim()
                                    .parse::<usize>()
                                    .ok()
                                    .and_then(|i| strings.get(i).cloned())
                                    .unwrap_or_default(),
                                Some("b") => match raw.trim() {
                                    "1" => "TRUE".into(),
                                    "0" => "FALSE".into(),
                                    other => other.to_string(),
                                },
                                _ => raw,
                            };
                            if col < MAX_TABLE_COLUMNS && !value.is_empty() {
                                size += value.len() + 3;
                                if row.len() <= col {
                                    row.resize(col + 1, String::new());
                                }
                                row[col] = value;
                            }
                        }
                    }
                    b"row" => {
                        if row.iter().any(|c| !c.trim().is_empty()) {
                            rows.push(std::mem::take(&mut row));
                        }
                        if size > budget {
                            return Err(Stop::Full);
                        }
                    }
                    _ => {}
                },
                Xml::Text(text) if in_value || in_inline => {
                    if let Some((_, _, raw)) = cell.as_mut() {
                        raw.push_str(&text);
                    }
                }
                _ => {}
            }
            Ok(())
        },
        Stop::Corrupt,
    )?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_parse_from_cell_references() {
        assert_eq!(column_index("A1"), Some(0));
        assert_eq!(column_index("AB7"), Some(27));
        assert_eq!(column_index("XFD1048576"), Some(16383));
        assert_eq!(column_index("12"), None);
    }

    #[test]
    fn heading_style_names() {
        assert_eq!(heading_from_style_name("heading 2"), Some(2));
        assert_eq!(heading_from_style_name("Heading1"), Some(1));
        assert_eq!(heading_from_style_name("Title"), Some(1));
        assert_eq!(heading_from_style_name("Normal"), None);
    }
}
