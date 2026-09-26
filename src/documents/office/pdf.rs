//! PDF text through `pdf-extract` (source: `officeparser` over pdf.js, OCR
//! off). Runs only inside the office child; a parser panic kills the child,
//! which the parent reports as a corrupt document.

use super::render::{Block, Output, Span};
use super::Stop;

pub fn pdf(bytes: &[u8], out: &mut Output) -> Result<(), Stop> {
    if !bytes.starts_with(b"%PDF-") {
        return Err(Stop::Unsupported("pdf magic mismatch".into()));
    }
    let text = pdf_extract::extract_text_from_mem(bytes).map_err(|err| {
        let detail = err.to_string();
        let lower = detail.to_ascii_lowercase();
        if lower.contains("encrypt") || lower.contains("decrypt") || lower.contains("password") {
            Stop::Unsupported(format!("pdf: {detail}"))
        } else {
            Stop::Corrupt(format!("pdf: {detail}"))
        }
    })?;
    // pdf-extract separates text blocks with blank lines; lines inside a
    // block are visual line wraps, so they join with spaces.
    for block in text
        .replace('\r', "")
        .replace('\u{c}', "\n\n")
        .split("\n\n")
    {
        let joined = block
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        if joined.is_empty() {
            continue;
        }
        out.block(Block::Paragraph(vec![Span {
            text: joined,
            ..Span::default()
        }]))
        .map_err(|_| Stop::Full)?;
    }
    Ok(())
}
