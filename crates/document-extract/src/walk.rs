use rhwp::model::control::Control;
use rhwp::model::document::Document;
use rhwp::model::paragraph::Paragraph;
use rhwp::model::table::Table;

use crate::limits::MAX_WALK_NEST_DEPTH;

pub struct WalkedBody {
    pub text: String,
    pub warnings: Vec<String>,
    pub truncated: bool,
    /// True when a table/header/footer/note walk dropped supported-scope body.
    pub omitted_supported: bool,
    /// True when a drawing shape was skipped (unsupported, but not silent).
    pub omitted_shape: bool,
}

struct WalkState {
    max_chars: usize,
    out: String,
    used: usize,
    warnings: Vec<String>,
    truncated: bool,
    omitted_supported: bool,
    omitted_shape: bool,
}

pub fn walk_body(doc: &Document, max_chars: usize) -> WalkedBody {
    let mut state = WalkState {
        max_chars,
        out: String::new(),
        used: 0,
        warnings: Vec::new(),
        truncated: false,
        omitted_supported: false,
        omitted_shape: false,
    };
    for section in &doc.sections {
        for para in &section.paragraphs {
            if state.truncated {
                break;
            }
            walk_paragraph(para, 0, &mut state);
        }
    }
    if doc.preview.as_ref().and_then(|p| p.text.as_ref()).is_some() {
        state
            .warnings
            .push("PrvText present but unused; body walk is BodyText/HWPX sections".into());
    }
    WalkedBody {
        text: state.out,
        warnings: state.warnings,
        truncated: state.truncated,
        omitted_supported: state.omitted_supported,
        omitted_shape: state.omitted_shape,
    }
}

fn walk_paragraph(para: &Paragraph, depth: usize, state: &mut WalkState) {
    if state.truncated {
        return;
    }
    if depth > MAX_WALK_NEST_DEPTH {
        state
            .warnings
            .push("walk depth exceeded; nested content dropped".into());
        state.omitted_supported = true;
        return;
    }
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
                state
                    .warnings
                    .push("shape/drawing text is unsupported and was not walked".into());
                state.omitted_shape = true;
            }
            _ => {}
        }
    }
}

fn walk_table(table: &Table, depth: usize, state: &mut WalkState) {
    if depth > MAX_WALK_NEST_DEPTH {
        state.warnings.push("table nest depth exceeded".into());
        state.omitted_supported = true;
        return;
    }
    let mut cells = Vec::new();
    for cell in &table.cells {
        if cell.row < table.row_count.max(1) && cell.col < table.col_count.max(1) {
            cells.push(cell);
        } else {
            state.warnings.push(format!(
                "table cell ({},{}) outside {}x{} grid; dropped",
                cell.row, cell.col, table.row_count, table.col_count
            ));
            state.omitted_supported = true;
        }
    }
    cells.sort_by_key(|c| (c.row, c.col));
    for cell in cells {
        for p in &cell.paragraphs {
            walk_paragraph(p, depth + 1, state);
        }
    }
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
    use super::{push_line, WalkState};

    #[test]
    fn separator_counts_toward_max_chars() {
        let mut state = WalkState {
            max_chars: 3,
            out: String::new(),
            used: 0,
            warnings: Vec::new(),
            truncated: false,
            omitted_supported: false,
            omitted_shape: false,
        };
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
}
