//! Stored Tiptap JSON -> the document model the export writers render
//! (DOCX now; PDF and PPTX are meant to read the same model).
//!
//! One walk that decides what each node *means* (source: the node handling
//! shared by `md.ts` `blockMd`/`mdRuns`, `pdf.tsx` and `pptx.ts`
//! `inlineText`/`block`): title prepend, blocks, inlines and marks. Layout
//! (pages, slides, fonts, spacing) belongs to each writer.
//!
//! Tolerant like the TS walkers: a node without a string `type` or an array
//! `content` is skipped, unknown blocks contribute their children, unknown
//! inlines contribute their children's text, and inline nodes found where a
//! block is expected (bare text at block level) are dropped as in `blockMd`.

use serde_json::Value;

use crate::collab::derived_body::emoji_glyph;

/// A document ready to render: the visible title (if any) and its blocks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportDoc {
    pub title: Option<String>,
    pub blocks: Vec<Block>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    /// Level clamped to 1..=6 (source: `Math.min(6, Math.max(1, level))`).
    Heading {
        level: u8,
        inlines: Vec<Inline>,
    },
    Paragraph(Vec<Inline>),
    List {
        kind: ListKind,
        items: Vec<ListItem>,
    },
    Table(Vec<TableRow>),
    Code {
        language: String,
        text: String,
    },
    /// `kind` as stored (lowercase in the editor), `"note"` when missing.
    Callout {
        kind: String,
        blocks: Vec<Block>,
    },
    Blockquote(Vec<Block>),
    /// Display math: the LaTeX source (no rendering, as in every TS export).
    Math(String),
    /// Mermaid source text (no rendering).
    Mermaid(String),
    /// Attachment placeholder: no bytes are fetched.
    Attachment {
        id: String,
        name: String,
        image: bool,
    },
    /// `entity` is `"document"` when missing (`url`, `task`, `project`, ...).
    Embed {
        entity: String,
        reference: String,
    },
    HorizontalRule,
    Details {
        summary: Vec<Inline>,
        blocks: Vec<Block>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListKind {
    Bullet,
    Ordered,
    Task,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListItem {
    /// `Some` for task items (`attrs.checked === true`).
    pub checked: Option<bool>,
    pub blocks: Vec<Block>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableRow {
    pub cells: Vec<TableCell>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableCell {
    /// Stored as `tableHeader`.
    pub header: bool,
    pub blocks: Vec<Block>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inline {
    /// Text, a mention (`@label`) or an emoji glyph, with the node's marks.
    Text {
        text: String,
        marks: Marks,
    },
    /// Inline LaTeX source.
    Math {
        latex: String,
        marks: Marks,
    },
    HardBreak,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Marks {
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub code: bool,
    pub underline: bool,
    pub highlight: bool,
    /// First `link` mark with a non-empty string `href`, unsanitised: each
    /// writer applies its own scheme policy.
    pub link: Option<String>,
}

/// Source `visibleTitle`: line breaks (CR, LF, U+2028, U+2029) become one
/// space, then JS `trim()`.
pub fn visible_title(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    let mut in_break = false;
    for c in title.chars() {
        if matches!(c, '\r' | '\n' | '\u{2028}' | '\u{2029}') {
            if !in_break {
                out.push(' ');
            }
            in_break = true;
        } else {
            in_break = false;
            out.push(c);
        }
    }
    js_trim(&out).to_string()
}

fn js_trim(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_whitespace() || c == '\u{FEFF}')
}

/// Walks a stored Tiptap doc (the caller checked `is_tiptap_doc`).
pub fn export_doc(title: &str, doc: &Value) -> ExportDoc {
    let title = visible_title(title);
    ExportDoc {
        title: (!title.is_empty()).then_some(title),
        blocks: blocks(content(doc)),
    }
}

fn node_type(n: &Value) -> &str {
    n.get("type").and_then(Value::as_str).unwrap_or("")
}

fn content(n: &Value) -> &[Value] {
    n.get("content")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn has_content(n: &Value) -> bool {
    n.get("content").is_some_and(Value::is_array)
}

fn attr<'a>(n: &'a Value, key: &str) -> Option<&'a Value> {
    n.get("attrs").filter(|a| a.is_object())?.get(key)
}

fn str_attr(n: &Value, key: &str) -> String {
    attr(n, key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn blocks(nodes: &[Value]) -> Vec<Block> {
    let mut out = Vec::new();
    for n in nodes {
        block(n, &mut out);
    }
    out
}

fn block(n: &Value, out: &mut Vec<Block>) {
    let kids = content(n);
    match node_type(n) {
        "paragraph" => out.push(Block::Paragraph(inlines(kids))),
        "heading" => {
            let level = attr(n, "level")
                .and_then(Value::as_f64)
                .map(|l| l.clamp(1.0, 6.0) as u8)
                .unwrap_or(1);
            out.push(Block::Heading {
                level,
                inlines: inlines(kids),
            });
        }
        "blockquote" => out.push(Block::Blockquote(blocks(kids))),
        "callout" => {
            let kind = str_attr(n, "kind");
            out.push(Block::Callout {
                kind: if kind.is_empty() { "note".into() } else { kind },
                blocks: blocks(kids),
            });
        }
        "details" => {
            let summary = kids.iter().find(|c| node_type(c) == "detailsSummary");
            let body = kids.iter().find(|c| node_type(c) == "detailsContent");
            out.push(Block::Details {
                summary: summary.map(|s| inlines(content(s))).unwrap_or_default(),
                blocks: body.map(|b| blocks(content(b))).unwrap_or_default(),
            });
        }
        "math" => out.push(Block::Math(str_attr(n, "latex"))),
        "mermaid" => {
            let source = str_attr(n, "source");
            out.push(Block::Mermaid(if source.is_empty() {
                literal_text(kids)
            } else {
                source
            }));
        }
        "attachment" => out.push(Block::Attachment {
            id: str_attr(n, "id"),
            name: str_attr(n, "name"),
            image: attr(n, "image") == Some(&Value::Bool(true)),
        }),
        "codeBlock" => out.push(Block::Code {
            language: str_attr(n, "language"),
            text: literal_text(kids),
        }),
        "bulletList" => out.push(list(ListKind::Bullet, kids)),
        "orderedList" => out.push(list(ListKind::Ordered, kids)),
        "taskList" => out.push(list(ListKind::Task, kids)),
        "horizontalRule" => out.push(Block::HorizontalRule),
        "table" => out.push(table(kids)),
        "embed" => {
            let entity = str_attr(n, "entity");
            out.push(Block::Embed {
                entity: if entity.is_empty() {
                    "document".into()
                } else {
                    entity
                },
                reference: str_attr(n, "ref"),
            });
        }
        // `listItem`/`taskItem` outside a list and unknown containers
        // contribute their children; leaves (bare text, atoms) nothing.
        _ if has_content(n) => {
            for kid in kids {
                block(kid, out);
            }
        }
        _ => {}
    }
}

fn list(kind: ListKind, items: &[Value]) -> Block {
    let items = items
        .iter()
        .map(|item| ListItem {
            checked: (kind == ListKind::Task)
                .then(|| attr(item, "checked") == Some(&Value::Bool(true))),
            blocks: blocks(content(item)),
        })
        .collect();
    Block::List { kind, items }
}

/// Source `gfmTable`: only `tableRow` children are rows.
fn table(rows: &[Value]) -> Block {
    Block::Table(
        rows.iter()
            .filter(|r| node_type(r) == "tableRow")
            .map(|r| TableRow {
                cells: content(r)
                    .iter()
                    .map(|c| TableCell {
                        header: node_type(c) == "tableHeader",
                        blocks: blocks(content(c)),
                    })
                    .collect(),
            })
            .collect(),
    )
}

/// Source `literalText`: the `text` of direct children only.
fn literal_text(nodes: &[Value]) -> String {
    nodes
        .iter()
        .filter_map(|n| n.get("text").and_then(Value::as_str))
        .collect()
}

fn marks(n: &Value) -> Marks {
    let mut out = Marks::default();
    let Some(list) = n.get("marks").and_then(Value::as_array) else {
        return out;
    };
    for m in list {
        match m.get("type").and_then(Value::as_str) {
            Some("bold") => out.bold = true,
            Some("italic") => out.italic = true,
            Some("strike") => out.strike = true,
            Some("code") => out.code = true,
            Some("underline") => out.underline = true,
            Some("highlight") => out.highlight = true,
            Some("link") if out.link.is_none() => {
                if let Some(href) = attr(m, "href").and_then(Value::as_str) {
                    if !href.is_empty() {
                        out.link = Some(href.to_string());
                    }
                }
            }
            _ => {}
        }
    }
    out
}

fn inlines(nodes: &[Value]) -> Vec<Inline> {
    let mut out = Vec::new();
    push_inlines(nodes, &mut out);
    out
}

fn push_text(out: &mut Vec<Inline>, text: String, marks: Marks) {
    if text.is_empty() {
        return;
    }
    // Adjacent text with the same marks is one run (source `mergeTextNodes`).
    if let Some(Inline::Text {
        text: prev,
        marks: prev_marks,
    }) = out.last_mut()
    {
        if *prev_marks == marks {
            prev.push_str(&text);
            return;
        }
    }
    out.push(Inline::Text { text, marks });
}

fn push_inlines(nodes: &[Value], out: &mut Vec<Inline>) {
    for n in nodes {
        match node_type(n) {
            "text" => {
                let text = n.get("text").and_then(Value::as_str).unwrap_or("");
                push_text(out, text.to_string(), marks(n));
            }
            "mathInline" => out.push(Inline::Math {
                latex: str_attr(n, "latex"),
                marks: marks(n),
            }),
            "hardBreak" => out.push(Inline::HardBreak),
            "mention" => {
                let label = str_attr(n, "label");
                if !label.is_empty() {
                    push_text(out, format!("@{label}"), marks(n));
                }
            }
            "emoji" => push_text(out, emoji_glyph(n), marks(n)),
            _ => push_inlines(content(n), out),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn text(t: &str) -> Inline {
        Inline::Text {
            text: t.into(),
            marks: Marks::default(),
        }
    }

    #[test]
    fn title_is_visible_title() {
        assert_eq!(visible_title("  a\r\n\u{2028}b \n"), "a b");
        let doc = export_doc(" \n ", &json!({"type": "doc"}));
        assert_eq!(doc.title, None);
        assert!(doc.blocks.is_empty());
    }

    #[test]
    fn inlines_marks_atoms() {
        let doc = json!({"type": "doc", "content": [{"type": "paragraph", "content": [
            {"type": "text", "text": "a", "marks": [{"type": "bold"}, {"type": "link", "attrs": {"href": ""}},
                {"type": "link", "attrs": {"href": "https://x"}}, {"type": "link", "attrs": {"href": "https://y"}}]},
            {"type": "text", "text": "b", "marks": [{"type": "link", "attrs": {"href": "https://x"}}, {"type": "bold"}]},
            {"type": "mention", "attrs": {"id": "u", "label": "김"}},
            {"type": "mention", "attrs": {"id": "u"}},
            {"type": "emoji", "attrs": {"name": "smile"}},
            {"type": "hardBreak"},
            {"type": "mathInline", "attrs": {"latex": "x^2"}},
            {"type": "wrapper", "content": [{"type": "text", "text": "w", "marks": [{"type": "underline"}]}]},
            {"text": "no type"}
        ]}]});
        let got = export_doc("", &doc).blocks;
        let link = Marks {
            bold: true,
            link: Some("https://x".into()),
            ..Marks::default()
        };
        assert_eq!(
            got,
            vec![Block::Paragraph(vec![
                Inline::Text {
                    text: "ab".into(),
                    marks: link
                },
                text("@김😄"),
                Inline::HardBreak,
                Inline::Math {
                    latex: "x^2".into(),
                    marks: Marks::default()
                },
                Inline::Text {
                    text: "w".into(),
                    marks: Marks {
                        underline: true,
                        ..Marks::default()
                    }
                },
            ])]
        );
    }

    #[test]
    fn blocks_and_fallbacks() {
        let doc = json!({"type": "doc", "content": [
            {"type": "heading", "attrs": {"level": 9}, "content": [{"type": "text", "text": "h"}]},
            {"type": "heading", "content": [{"type": "text", "text": "h1"}]},
            {"type": "text", "text": "bare text is dropped"},
            {"type": "unknownBlock", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "inner"}]}]},
            {"type": "taskList", "content": [{"type": "taskItem", "attrs": {"checked": true}, "content": [
                {"type": "paragraph", "content": [{"type": "text", "text": "t"}]}]},
                {"type": "taskItem", "attrs": {"checked": "true"}, "content": []}]},
            {"type": "table", "content": [{"type": "tableRow", "content": [
                {"type": "tableHeader", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "H"}]}]},
                {"type": "tableCell", "content": []}]}, {"type": "paragraph"}]},
            {"type": "callout", "content": []},
            {"type": "mermaid", "content": [{"type": "text", "text": "graph"}]},
            {"type": "embed", "attrs": {"ref": "r"}},
            {"type": "attachment", "attrs": {"id": "a", "name": "n.png", "image": true}},
            {"type": "codeBlock", "attrs": {"language": "rs"}, "content": [{"type": "text", "text": "x"}, {"type": "hardBreak"}, {"type": "text", "text": "y"}]}
        ]});
        let got = export_doc("", &doc).blocks;
        assert_eq!(
            got,
            vec![
                Block::Heading {
                    level: 6,
                    inlines: vec![text("h")]
                },
                Block::Heading {
                    level: 1,
                    inlines: vec![text("h1")]
                },
                Block::Paragraph(vec![text("inner")]),
                Block::List {
                    kind: ListKind::Task,
                    items: vec![
                        ListItem {
                            checked: Some(true),
                            blocks: vec![Block::Paragraph(vec![text("t")])]
                        },
                        ListItem {
                            checked: Some(false),
                            blocks: vec![]
                        },
                    ]
                },
                Block::Table(vec![TableRow {
                    cells: vec![
                        TableCell {
                            header: true,
                            blocks: vec![Block::Paragraph(vec![text("H")])]
                        },
                        TableCell {
                            header: false,
                            blocks: vec![]
                        },
                    ]
                }]),
                Block::Callout {
                    kind: "note".into(),
                    blocks: vec![]
                },
                Block::Mermaid("graph".into()),
                Block::Embed {
                    entity: "document".into(),
                    reference: "r".into()
                },
                Block::Attachment {
                    id: "a".into(),
                    name: "n.png".into(),
                    image: true
                },
                Block::Code {
                    language: "rs".into(),
                    text: "xy".into()
                },
            ]
        );
    }
}
