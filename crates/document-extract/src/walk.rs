use rhwp::model::control::Control;
use rhwp::model::document::Document;
use rhwp::model::paragraph::Paragraph;
use rhwp::model::table::Table;

use crate::limits::MAX_WALK_NEST_DEPTH;

pub struct WalkedBody {
    pub text: String,
    pub warnings: Vec<String>,
    pub truncated: bool,
}

pub fn walk_body(doc: &Document, max_chars: usize) -> WalkedBody {
    let mut out = String::new();
    let mut warnings = Vec::new();
    let mut truncated = false;
    for section in &doc.sections {
        for para in &section.paragraphs {
            if truncated {
                break;
            }
            walk_paragraph(para, 0, max_chars, &mut out, &mut warnings, &mut truncated);
        }
    }
    if doc.preview.as_ref().and_then(|p| p.text.as_ref()).is_some() {
        warnings.push("PrvText present but unused; body walk is BodyText/HWPX sections".into());
    }
    WalkedBody {
        text: out,
        warnings,
        truncated,
    }
}

fn walk_paragraph(
    para: &Paragraph,
    depth: usize,
    max_chars: usize,
    out: &mut String,
    warnings: &mut Vec<String>,
    truncated: &mut bool,
) {
    if *truncated {
        return;
    }
    if depth > MAX_WALK_NEST_DEPTH {
        warnings.push("walk depth exceeded; nested content dropped".into());
        return;
    }
    push_line(para.text.trim(), max_chars, out, truncated);
    for control in &para.controls {
        if *truncated {
            return;
        }
        match control {
            Control::Table(table) => walk_table(table, depth, max_chars, out, warnings, truncated),
            Control::Header(h) => {
                for p in &h.paragraphs {
                    walk_paragraph(p, depth + 1, max_chars, out, warnings, truncated);
                }
            }
            Control::Footer(f) => {
                for p in &f.paragraphs {
                    walk_paragraph(p, depth + 1, max_chars, out, warnings, truncated);
                }
            }
            Control::Footnote(f) => {
                for p in &f.paragraphs {
                    walk_paragraph(p, depth + 1, max_chars, out, warnings, truncated);
                }
            }
            Control::Endnote(e) => {
                for p in &e.paragraphs {
                    walk_paragraph(p, depth + 1, max_chars, out, warnings, truncated);
                }
            }
            Control::Shape(_) => {}
            _ => {}
        }
    }
}

fn walk_table(
    table: &Table,
    depth: usize,
    max_chars: usize,
    out: &mut String,
    warnings: &mut Vec<String>,
    truncated: &mut bool,
) {
    if depth > MAX_WALK_NEST_DEPTH {
        warnings.push("table nest depth exceeded".into());
        return;
    }
    let mut cells: Vec<_> = table
        .cells
        .iter()
        .filter(|c| c.row < table.row_count.max(1) && c.col < table.col_count.max(1))
        .collect();
    cells.sort_by_key(|c| (c.row, c.col));
    for cell in cells {
        for p in &cell.paragraphs {
            walk_paragraph(p, depth + 1, max_chars, out, warnings, truncated);
        }
    }
}

fn push_line(line: &str, max_chars: usize, out: &mut String, truncated: &mut bool) {
    if line.is_empty() || *truncated {
        return;
    }
    let remaining = max_chars.saturating_sub(out.chars().count());
    if remaining == 0 {
        *truncated = true;
        return;
    }
    if !out.is_empty() {
        out.push('\n');
    }
    if line.chars().count() <= remaining {
        out.push_str(line);
        return;
    }
    out.extend(line.chars().take(remaining));
    *truncated = true;
}
