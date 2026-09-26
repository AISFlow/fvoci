//! Block model shared by the office parsers and its two renderings: Markdown
//! for imports (source `officeparser` AST `.to("md")`) and plain text for
//! attachment extraction (`.to("text")`). Output is bounded while it is built,
//! so a parser stops as soon as the budget is spent.

use super::OfficeMode;

/// Widest table kept (cells past it are dropped).
pub const MAX_TABLE_COLUMNS: usize = 256;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Heading(u8, Vec<Span>),
    Paragraph(Vec<Span>),
    ListItem(u8, Vec<Span>),
    Table(Vec<Vec<String>>),
}

/// The output budget is spent; the caller stops parsing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Full;

pub struct Output {
    mode: OfficeMode,
    buf: String,
    /// Markdown: UTF-8 bytes. Text: Unicode scalar values.
    limit: usize,
    used: usize,
    last_was_item: bool,
    pub truncated: bool,
}

impl Output {
    pub fn new(mode: OfficeMode, limit: usize) -> Self {
        Self {
            mode,
            buf: String::new(),
            limit,
            used: 0,
            last_was_item: false,
            truncated: false,
        }
    }

    pub fn finish(self) -> (String, bool) {
        (self.buf, self.truncated)
    }

    pub fn is_empty(&self) -> bool {
        self.buf.trim().is_empty()
    }

    fn push(&mut self, piece: &str) -> Result<(), Full> {
        let cost = match self.mode {
            OfficeMode::Markdown => piece.len(),
            OfficeMode::Text => piece.chars().count(),
        };
        if self.used + cost <= self.limit {
            self.buf.push_str(piece);
            self.used += cost;
            return Ok(());
        }
        // Text keeps a prefix up to the limit (source slices the text);
        // Markdown is all-or-nothing for the caller to reject.
        if self.mode == OfficeMode::Text {
            let room = self.limit - self.used;
            self.buf.extend(piece.chars().take(room));
            self.used = self.limit;
        }
        self.truncated = true;
        Err(Full)
    }

    pub fn block(&mut self, block: Block) -> Result<(), Full> {
        let rendered = match self.mode {
            OfficeMode::Markdown => markdown_block(&block),
            OfficeMode::Text => text_block(&block),
        };
        let Some(rendered) = rendered else {
            return Ok(());
        };
        let is_item = matches!(block, Block::ListItem(..));
        if !self.buf.is_empty() {
            let sep = match self.mode {
                OfficeMode::Markdown if is_item && self.last_was_item => "\n",
                OfficeMode::Markdown => "\n\n",
                OfficeMode::Text => "\n",
            };
            self.push(sep)?;
        }
        self.last_was_item = is_item;
        self.push(&rendered)
    }
}

fn plain(spans: &[Span]) -> String {
    spans.iter().map(|s| s.text.as_str()).collect()
}

fn text_block(block: &Block) -> Option<String> {
    let text = match block {
        Block::Heading(_, spans) | Block::Paragraph(spans) | Block::ListItem(_, spans) => {
            plain(spans).trim().to_string()
        }
        Block::Table(rows) => rows
            .iter()
            .map(|row| {
                row.iter()
                    .map(|c| c.trim())
                    .collect::<Vec<_>>()
                    .join("\t")
                    .trim_end()
                    .to_string()
            })
            .filter(|row| !row.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
    };
    (!text.trim().is_empty()).then_some(text)
}

fn merge_spans(spans: &[Span]) -> Vec<Span> {
    let mut merged: Vec<Span> = Vec::new();
    for span in spans {
        if span.text.is_empty() {
            continue;
        }
        match merged.last_mut() {
            Some(last) if last.bold == span.bold && last.italic == span.italic => {
                last.text.push_str(&span.text)
            }
            _ => merged.push(span.clone()),
        }
    }
    merged
}

fn inline_markdown(spans: &[Span]) -> String {
    let mut out = String::new();
    for span in merge_spans(spans) {
        let marker = match (span.bold, span.italic) {
            (true, true) => "***",
            (true, false) => "**",
            (false, true) => "*",
            (false, false) => "",
        };
        let core = span.text.trim();
        if marker.is_empty() || core.is_empty() || core.contains('\n') {
            out.push_str(&escape_inline(&span.text));
            continue;
        }
        let lead = &span.text[..span.text.len() - span.text.trim_start().len()];
        let tail = &span.text[span.text.trim_end().len()..];
        out.push_str(lead);
        out.push_str(marker);
        out.push_str(&escape_inline(core));
        out.push_str(marker);
        out.push_str(tail);
    }
    // Line breaks inside a paragraph become hard breaks; leading syntax on
    // every line is escaped by the final pass.
    let lines: Vec<String> = out
        .split('\n')
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
        .collect();
    lines.join("\\\n")
}

/// Inline escaping without the line-start rules (applied per line later).
fn escape_inline(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\\' | '*' | '_' | '`' | '[' | ']' | '<' | '>' | '|' | '~' | '&' | '$' => {
                out.push('\\');
                out.push(c);
            }
            '\r' => {}
            '\t' => out.push(' '),
            _ => out.push(c),
        }
    }
    out
}

/// Line-start escaping for text that already went through [`escape_inline`].
fn escape_line_starts(text: &str) -> String {
    text.split('\n')
        .map(|line| {
            let trimmed = line.trim_start();
            let mut chars = trimmed.chars();
            match chars.next() {
                Some('#' | '-' | '+' | '=') => format!("\\{trimmed}"),
                Some(c) if c.is_ascii_digit() => {
                    let digits = trimmed.chars().take_while(|d| d.is_ascii_digit()).count();
                    match trimmed[digits..].chars().next() {
                        Some('.' | ')') => {
                            format!("{}\\{}", &trimmed[..digits], &trimmed[digits..])
                        }
                        _ => trimmed.to_string(),
                    }
                }
                _ => trimmed.to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn table_cell(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    escape_inline(&flat)
}

fn markdown_block(block: &Block) -> Option<String> {
    match block {
        Block::Heading(level, spans) => {
            let text = inline_markdown(spans).replace("\\\n", " ");
            let text = escape_line_starts(&text);
            (!text.trim().is_empty())
                .then(|| format!("{} {}", "#".repeat((*level).clamp(1, 6) as usize), text))
        }
        Block::Paragraph(spans) => {
            let text = escape_line_starts(&inline_markdown(spans));
            (!text.trim().is_empty()).then_some(text)
        }
        Block::ListItem(depth, spans) => {
            let text = inline_markdown(spans).replace("\\\n", " ");
            let text = escape_line_starts(&text);
            (!text.trim().is_empty())
                .then(|| format!("{}- {}", "  ".repeat((*depth).min(8) as usize), text))
        }
        Block::Table(rows) => {
            let rows: Vec<&Vec<String>> = rows
                .iter()
                .filter(|row| row.iter().any(|c| !c.trim().is_empty()))
                .collect();
            let width = rows
                .iter()
                .map(|row| {
                    row.iter()
                        .rposition(|c| !c.trim().is_empty())
                        .map_or(0, |at| at + 1)
                })
                .max()
                .unwrap_or(0)
                .min(MAX_TABLE_COLUMNS);
            if rows.is_empty() || width == 0 {
                return None;
            }
            let line = |row: &Vec<String>| {
                let cells: Vec<String> = (0..width)
                    .map(|i| row.get(i).map(|c| table_cell(c)).unwrap_or_default())
                    .collect();
                format!("| {} |", cells.join(" | "))
            };
            let mut out = vec![line(rows[0])];
            out.push(format!("|{}", " --- |".repeat(width)));
            out.extend(rows[1..].iter().map(|row| line(row)));
            Some(out.join("\n"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(text: &str) -> Span {
        Span {
            text: text.into(),
            ..Span::default()
        }
    }

    #[test]
    fn markdown_escapes_syntax_and_keeps_emphasis() {
        let mut out = Output::new(OfficeMode::Markdown, 10_000);
        out.block(Block::Heading(1, vec![span("# 제목 *1*")]))
            .unwrap();
        out.block(Block::Paragraph(vec![
            span("1. not a list <script> "),
            Span {
                text: "bold".into(),
                bold: true,
                italic: false,
            },
            span(" [x](y)"),
        ]))
        .unwrap();
        out.block(Block::ListItem(0, vec![span("a")])).unwrap();
        out.block(Block::ListItem(1, vec![span("b")])).unwrap();
        out.block(Block::Table(vec![
            vec!["h1".into(), "h|2".into()],
            vec!["x".into(), String::new()],
        ]))
        .unwrap();
        let (md, truncated) = out.finish();
        assert!(!truncated);
        assert_eq!(
            md,
            "# \\# 제목 \\*1\\*\n\n1\\. not a list \\<script\\> **bold** \\[x\\](y)\n\n- a\n  - b\n\n| h1 | h\\|2 |\n| --- | --- |\n| x |  |"
        );
    }

    #[test]
    fn text_mode_truncates_to_the_char_budget() {
        let mut out = Output::new(OfficeMode::Text, 5);
        assert_eq!(
            out.block(Block::Paragraph(vec![span("가나다라마바사")])),
            Err(Full)
        );
        let (text, truncated) = out.finish();
        assert_eq!(text, "가나다라마");
        assert!(truncated);
    }

    #[test]
    fn markdown_budget_is_all_or_nothing() {
        let mut out = Output::new(OfficeMode::Markdown, 4);
        assert_eq!(out.block(Block::Paragraph(vec![span("12345")])), Err(Full));
        assert!(out.truncated);
    }
}
