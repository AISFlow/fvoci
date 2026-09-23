use std::collections::BTreeMap;

use rhwp::model::control::Control;
use rhwp::model::document::{Document, Section};
use rhwp::model::image::Picture;
use rhwp::model::paragraph::Paragraph;
use rhwp::model::shape::{Caption, CaptionDirection};
use rhwp::model::table::Table;

use crate::limits::{MAX_WALK_NEST_DEPTH, MAX_WARNING_ENTRIES};
use crate::outcome::DocFormat;

pub struct WalkedBody {
    pub text: String,
    pub warnings: Vec<String>,
    pub truncated: bool,
    /// True when a table/header/footer/note walk dropped supported-scope body.
    pub omitted_supported: bool,
    /// True when a drawing shape was skipped (unsupported, but not walked).
    pub omitted_shape: bool,
    /// True when the pinned parser replaced a section with `Section::default`.
    pub omitted_section: bool,
    /// True when at least one section was a successful parse, not a default stub.
    pub recovered_section: bool,
}

struct WalkState {
    max_chars: usize,
    out: String,
    used: usize,
    warning_counts: BTreeMap<&'static str, (String, usize)>,
    truncated: bool,
    omitted_supported: bool,
    omitted_shape: bool,
    omitted_section: bool,
    recovered_section: bool,
}

pub fn walk_body(doc: &Document, format: DocFormat, max_chars: usize) -> WalkedBody {
    let mut state = WalkState {
        max_chars,
        out: String::new(),
        used: 0,
        warning_counts: BTreeMap::new(),
        truncated: false,
        omitted_supported: false,
        omitted_shape: false,
        omitted_section: false,
        recovered_section: false,
    };
    for (i, section) in doc.sections.iter().enumerate() {
        if section_omitted_by_parser(section, format) {
            note(
                &mut state,
                "omitted_section",
                format!("section {i} dropped by parser"),
            );
            state.omitted_section = true;
            state.omitted_supported = true;
            continue;
        }
        state.recovered_section = true;
        for para in &section.paragraphs {
            if state.truncated {
                break;
            }
            walk_paragraph(para, 0, &mut state);
        }
    }
    if doc.preview.as_ref().and_then(|p| p.text.as_ref()).is_some() {
        note(
            &mut state,
            "prvtext",
            "PrvText present but unused; body walk is parser IR sections".into(),
        );
    }
    WalkedBody {
        text: state.out,
        warnings: flush_warnings(&state.warning_counts),
        truncated: state.truncated,
        omitted_supported: state.omitted_supported,
        omitted_shape: state.omitted_shape,
        omitted_section: state.omitted_section,
        recovered_section: state.recovered_section,
    }
}

/// HWP5: `parse_sections_strict` sets `raw_stream` only on a successful
/// `parse_body_text_section`; `Section::default()` (failed stream) leaves it
/// `None`. A valid empty BodyText section still stores `Some` (possibly empty
/// bytes). HWPX never sets `raw_stream`. Hangul and this crate's empty HWPX
/// fixture always emit ≥1 `<hp:p>`; a successful parse of `<hs:sec/>` with
/// zero paragraphs is indistinguishable from the drop stub, so it is treated
/// as omitted rather than silent Empty.
pub(crate) fn section_omitted_by_parser(section: &Section, format: DocFormat) -> bool {
    match format {
        DocFormat::Hwp5 => section.raw_stream.is_none(),
        DocFormat::Hwpx => section.paragraphs.is_empty(),
    }
}

fn walk_paragraph(para: &Paragraph, depth: usize, state: &mut WalkState) {
    if state.truncated {
        return;
    }
    if depth > MAX_WALK_NEST_DEPTH {
        note(
            state,
            "walk_depth",
            "walk depth exceeded; nested content dropped".into(),
        );
        state.omitted_supported = true;
        return;
    }
    // Paragraph text is emitted first; nested controls follow. An inline
    // treat-as-char table is therefore after that paragraph's text, not at
    // its character anchor (indexing order, not page reading order).
    push_line(para.text.trim(), state);
    for control in &para.controls {
        if state.truncated {
            return;
        }
        match control {
            Control::Table(table) => walk_table(table, depth, state),
            Control::Header(h) => {
                for p in &h.paragraphs {
                    walk_paragraph(p, depth + 1, state);
                }
            }
            Control::Footer(f) => {
                for p in &f.paragraphs {
                    walk_paragraph(p, depth + 1, state);
                }
            }
            Control::Footnote(f) => {
                for p in &f.paragraphs {
                    walk_paragraph(p, depth + 1, state);
                }
            }
            Control::Endnote(e) => {
                for p in &e.paragraphs {
                    walk_paragraph(p, depth + 1, state);
                }
            }
            Control::Shape(_) => {
                note(
                    state,
                    "shape",
                    "shape/drawing text is unsupported and was not walked".into(),
                );
                state.omitted_shape = true;
            }
            Control::Picture(pic) => walk_picture(pic, depth, state),
            Control::Equation(eq) => {
                if !eq.script.trim().is_empty() {
                    push_line(eq.script.trim(), state);
                }
            }
            Control::Form(form) => {
                if !form.caption.trim().is_empty() {
                    push_line(form.caption.trim(), state);
                }
                if !form.text.trim().is_empty() {
                    push_line(form.text.trim(), state);
                }
            }
            // HiddenComment is intentionally not extracted (hidden; not body).
            _ => {}
        }
    }
}

fn walk_picture(pic: &Picture, depth: usize, state: &mut WalkState) {
    if let Some(cap) = &pic.caption {
        walk_caption(cap, depth, state);
    }
}

fn walk_table(table: &Table, depth: usize, state: &mut WalkState) {
    if depth > MAX_WALK_NEST_DEPTH {
        note(state, "table_depth", "table nest depth exceeded".into());
        state.omitted_supported = true;
        return;
    }
    let caption_before_cells = table
        .caption
        .as_ref()
        .map(caption_before_cells)
        .unwrap_or(false);
    if caption_before_cells {
        if let Some(cap) = &table.caption {
            walk_caption(cap, depth, state);
        }
    }
    let mut cells = Vec::new();
    for cell in &table.cells {
        if cell.row < table.row_count.max(1) && cell.col < table.col_count.max(1) {
            cells.push(cell);
        } else {
            note(
                state,
                "table_cell_range",
                format!(
                    "table cell ({},{}) outside {}x{} grid; dropped",
                    cell.row, cell.col, table.row_count, table.col_count
                ),
            );
            state.omitted_supported = true;
        }
    }
    cells.sort_by_key(|c| (c.row, c.col));
    for cell in cells {
        for p in &cell.paragraphs {
            walk_paragraph(p, depth + 1, state);
        }
    }
    if !caption_before_cells {
        if let Some(cap) = &table.caption {
            walk_caption(cap, depth, state);
        }
    }
}

/// Visual/reading order: Top and Left captions precede row-major cells;
/// Bottom and Right follow the cells. Hangul's default caption side is Bottom.
fn caption_before_cells(cap: &Caption) -> bool {
    matches!(
        cap.direction,
        CaptionDirection::Top | CaptionDirection::Left
    )
}

fn walk_caption(cap: &Caption, depth: usize, state: &mut WalkState) {
    for p in &cap.paragraphs {
        walk_paragraph(p, depth + 1, state);
    }
}

fn note(state: &mut WalkState, kind: &'static str, message: String) {
    if let Some((_, n)) = state.warning_counts.get_mut(kind) {
        *n += 1;
        return;
    }
    if state.warning_counts.len() < MAX_WARNING_ENTRIES {
        state.warning_counts.insert(kind, (message, 1));
        return;
    }
    if let Some((_, n)) = state.warning_counts.get_mut("warning_cap") {
        *n += 1;
    }
}

fn flush_warnings(counts: &BTreeMap<&'static str, (String, usize)>) -> Vec<String> {
    counts
        .values()
        .map(|(msg, n)| {
            if *n > 1 {
                format!("{msg} ×{n}")
            } else {
                msg.clone()
            }
        })
        .collect()
}

fn push_line(line: &str, state: &mut WalkState) {
    if line.is_empty() || state.truncated {
        return;
    }
    let sep = usize::from(!state.out.is_empty());
    let line_chars = line.chars().count();
    if state.used + sep > state.max_chars {
        state.truncated = true;
        return;
    }
    if state.used + sep + line_chars > state.max_chars {
        if sep > 0 {
            state.out.push('\n');
            state.used += 1;
        }
        let room = state.max_chars.saturating_sub(state.used);
        state.out.extend(line.chars().take(room));
        state.used += room;
        state.truncated = true;
        return;
    }
    if sep > 0 {
        state.out.push('\n');
    }
    state.out.push_str(line);
    state.used += sep + line_chars;
}

#[cfg(test)]
mod push_line_tests {
    use super::{flush_warnings, note, push_line, WalkState};
    use std::collections::BTreeMap;

    fn test_state(max_chars: usize) -> WalkState {
        WalkState {
            max_chars,
            out: String::new(),
            used: 0,
            warning_counts: BTreeMap::new(),
            truncated: false,
            omitted_supported: false,
            omitted_shape: false,
            omitted_section: false,
            recovered_section: false,
        }
    }

    #[test]
    fn separator_counts_toward_max_chars() {
        let mut state = test_state(3);
        push_line("ab", &mut state);
        push_line("c", &mut state);
        assert!(state.truncated);
        assert!(
            state.out.chars().count() <= 3,
            "{:?} used={}",
            state.out,
            state.used
        );
        assert_eq!(state.used, state.out.chars().count());
    }

    #[test]
    fn warnings_dedupe_and_count() {
        let mut state = test_state(100);
        note(
            &mut state,
            "shape",
            "shape/drawing text is unsupported and was not walked".into(),
        );
        note(
            &mut state,
            "shape",
            "shape/drawing text is unsupported and was not walked".into(),
        );
        note(
            &mut state,
            "shape",
            "shape/drawing text is unsupported and was not walked".into(),
        );
        let warnings = flush_warnings(&state.warning_counts);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].ends_with("×3"), "{warnings:?}");
    }
}

#[cfg(test)]
mod omitted_signals {
    use super::section_omitted_by_parser;
    use crate::gen::{hwp5_empty_body, hwpx_empty_body};
    use crate::outcome::DocFormat;
    use rhwp::parse_document;

    #[test]
    fn hwp5_empty_keeps_raw_stream() {
        let doc = parse_document(&hwp5_empty_body()).expect("parse empty hwp5");
        assert_eq!(doc.sections.len(), 1);
        assert!(doc.sections[0].raw_stream.is_some());
        assert!(!section_omitted_by_parser(
            &doc.sections[0],
            DocFormat::Hwp5
        ));
    }

    #[test]
    fn hwpx_empty_fixture_has_a_paragraph() {
        let doc = parse_document(&hwpx_empty_body()).expect("parse empty hwpx");
        assert_eq!(doc.sections.len(), 1);
        assert!(
            !doc.sections[0].paragraphs.is_empty(),
            "genuine empty HWPX must keep ≥1 paragraph"
        );
        assert!(!section_omitted_by_parser(
            &doc.sections[0],
            DocFormat::Hwpx
        ));
    }
}
