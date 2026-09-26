//! Markdown -> Tiptap JSON, source `packages/editor/src/markdown/parse.ts`
//! (`mdToTiptapJson`).
//!
//! The source parses with `unified().use(remarkParse).use(remarkGfm)
//! .use(remarkMath)` and walks the mdast. `markdown` (markdown-rs) is the Rust
//! port of the same micromark/mdast-util-from-markdown pipeline, so its mdast
//! is walked here node for node with the same rules. Offsets from the mdast are
//! UTF-8 byte offsets into the same `source`, so every `source.slice(...)` of
//! the original reads the same characters.

use markdown::mdast::{self, Node};
use markdown::unist::Position;
use markdown::{Constructs, ParseOptions};
use serde_json::{json, Map, Value};

fn parse_options() -> ParseOptions {
    // remark-gfm + remark-math defaults (single-dollar inline math on).
    ParseOptions {
        constructs: Constructs {
            math_flow: true,
            math_text: true,
            ..Constructs::gfm()
        },
        gfm_strikethrough_single_tilde: true,
        math_text_single_dollar: true,
        ..ParseOptions::default()
    }
}

/// Deepest mdast the walker recurses into. Far beyond any document whose
/// Tiptap JSON fits [`MAX_TIPTAP_DEPTH`] (inline marks flatten, so the mdast
/// can be deeper than the JSON); the `--internal-markdown` child runs the walk
/// on a stack sized for it.
pub const MAX_MDAST_DEPTH: usize = 4096;

/// Deepest Tiptap JSON (objects and arrays) a conversion may return: the
/// document must stay readable by `serde_json` (recursion limit 128), which
/// also parsed the Node helper's `{"ok":true,"contentJson":…}` reply, so 126.
pub const MAX_TIPTAP_DEPTH: usize = 126;

/// The Markdown nests deeper than the stored document may (source: the TS
/// parser throws `RangeError`, the helper answers `invalid_input`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("markdown nests too deeply")]
pub struct TooDeep;

/// Source `mdToTiptapJson`, rejecting documents nested past
/// [`MAX_MDAST_DEPTH`] / [`MAX_TIPTAP_DEPTH`].
pub fn md_to_tiptap(markdown: &str) -> Result<Value, TooDeep> {
    // micromark's preprocess turns U+0000 into U+FFFD; markdown-rs keeps it.
    // Parse the replaced text (its offsets index `parsed`) and read raw
    // `source` slices back from the original, as the source does.
    let nuls: Vec<usize> = markdown.match_indices('\0').map(|(i, _)| i).collect();
    let parsed = if nuls.is_empty() {
        std::borrow::Cow::Borrowed(markdown)
    } else {
        std::borrow::Cow::Owned(markdown.replace('\0', "\u{FFFD}"))
    };
    // Only MDX constructs can make `to_mdast` fail; none are enabled.
    let root = markdown::to_mdast(&parsed, &parse_options())
        .expect("markdown without MDX constructs always parses");
    if mdast_depth(&root) > MAX_MDAST_DEPTH {
        drop_iteratively(root);
        return Err(TooDeep);
    }
    let children = match &root {
        Node::Root(r) => r.children.as_slice(),
        _ => &[],
    };
    let cx = Cx {
        source: &parsed,
        original: markdown,
        nuls: &nuls,
    };
    let content = cx.blocks_from_nodes(children);
    let doc = if content.is_empty() {
        json!({ "type": "doc", "content": [{ "type": "paragraph" }] })
    } else {
        json!({ "type": "doc", "content": content })
    };
    if json_depth(&doc) > MAX_TIPTAP_DEPTH {
        return Err(TooDeep);
    }
    Ok(doc)
}

/// Nesting depth of an mdast tree, without recursion.
fn mdast_depth(root: &Node) -> usize {
    let mut max = 0;
    let mut stack = vec![(root, 1usize)];
    while let Some((node, depth)) = stack.pop() {
        max = max.max(depth);
        for child in node.children().into_iter().flatten() {
            stack.push((child, depth + 1));
        }
    }
    max
}

/// Drops a (possibly very deep) mdast without recursing.
fn drop_iteratively(root: Node) {
    let mut stack = vec![root];
    while let Some(mut node) = stack.pop() {
        if let Some(children) = node.children_mut() {
            stack.append(children);
        }
    }
}

/// Nesting depth of objects/arrays in a JSON value, without recursion.
fn json_depth(value: &Value) -> usize {
    let mut max = 0;
    let mut stack = vec![(value, 0usize)];
    while let Some((value, depth)) = stack.pop() {
        let children: Box<dyn Iterator<Item = &Value>> = match value {
            Value::Array(items) => Box::new(items.iter()),
            Value::Object(map) => Box::new(map.values()),
            _ => continue,
        };
        max = max.max(depth + 1);
        stack.extend(children.map(|c| (c, depth + 1)));
    }
    max
}

/// Source `tiptapDocToSafeHtml(mdToTiptapJson(md))` (legal documents).
pub fn md_to_safe_html(markdown: &str) -> Result<String, TooDeep> {
    Ok(crate::share_render::tiptap_doc_to_safe_html(&md_to_tiptap(
        markdown,
    )?))
}

/// JS `\s` (and `String.prototype.trim`): WhiteSpace + LineTerminator.
fn js_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r' | ' ' | '\u{A0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200A}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
                | '\u{FEFF}'
    )
}

fn js_trim(s: &str) -> &str {
    s.trim_matches(js_space)
}

fn ref_entity(kind: &str) -> &'static str {
    match kind {
        "doc" => "document",
        "task" => "task",
        _ => "project",
    }
}

// ---------------------------------------------------------------------------
// Tokens: TOKEN / TOKEN_MATH / REF_FULL, hand-matched (the `regex` crate has no
// look-ahead and a different `\s`). Every alternative is deterministic: each
// variable part is a maximal run of a class that excludes its terminator.

enum Token<'a> {
    Ref {
        kind: &'a str,
        id: &'a str,
        label: Option<&'a str>,
    },
    Mention {
        kind: &'a str,
        id: &'a str,
        label: &'a str,
    },
    Highlight,
    Math(&'a str),
}

/// Maximal non-empty run of chars not in `stop`, starting at `at`.
fn run_until(s: &str, at: usize, stop: &[char]) -> Option<usize> {
    let end = s[at..].find(stop).map_or(s.len(), |i| at + i);
    (end > at).then_some(end)
}

fn kind_prefix<'a>(s: &'a str, at: usize, kinds: &[&'static str]) -> Option<(&'a str, usize)> {
    let rest = &s[at..];
    kinds.iter().find_map(|k| {
        rest.strip_prefix(k)?.strip_prefix(':')?;
        Some((&s[at..at + k.len()], at + k.len() + 1))
    })
}

/// `\[\[(doc|task|project):([^|\]]+)(?:\|([^\]]+))?\]\]` at `at`.
fn match_ref(s: &str, at: usize) -> Option<(Token<'_>, usize)> {
    if !s[at..].starts_with("[[") {
        return None;
    }
    let (kind, id_start) = kind_prefix(s, at + 2, &["doc", "task", "project"])?;
    let id_end = run_until(s, id_start, &['|', ']'])?;
    let id = &s[id_start..id_end];
    if s[id_end..].starts_with("]]") {
        return Some((
            Token::Ref {
                kind,
                id,
                label: None,
            },
            id_end + 2,
        ));
    }
    if !s[id_end..].starts_with('|') {
        return None;
    }
    let label_end = run_until(s, id_end + 1, &[']'])?;
    if !s[label_end..].starts_with("]]") {
        return None;
    }
    let label = &s[id_end + 1..label_end];
    Some((
        Token::Ref {
            kind,
            id,
            label: Some(label),
        },
        label_end + 2,
    ))
}

/// `@\[(user|group):([^|\]]+)\|([^\]]+)\]` at `at`.
fn match_mention(s: &str, at: usize) -> Option<(Token<'_>, usize)> {
    if !s[at..].starts_with("@[") {
        return None;
    }
    let (kind, id_start) = kind_prefix(s, at + 2, &["user", "group"])?;
    let id_end = run_until(s, id_start, &['|', ']'])?;
    if !s[id_end..].starts_with('|') {
        return None;
    }
    let label_end = run_until(s, id_end + 1, &[']'])?;
    Some((
        Token::Mention {
            kind,
            id: &s[id_start..id_end],
            label: &s[id_end + 1..label_end],
        },
        label_end + 1,
    ))
}

/// `\$([^\s{$\n](?:[^$\n]*[^\s$\n])?)\$(?!\d)` at `at`.
fn match_math(s: &str, at: usize) -> Option<(Token<'_>, usize)> {
    if !s[at..].starts_with('$') {
        return None;
    }
    let start = at + 1;
    let end = start + s[start..].find(['$', '\n'])?;
    if !s[end..].starts_with('$') {
        return None;
    }
    let inner = &s[start..end];
    let first = inner.chars().next()?;
    let last = inner.chars().next_back()?;
    if js_space(first) || first == '{' || js_space(last) {
        return None;
    }
    if s[end + 1..].starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    Some((Token::Math(inner), end + 1))
}

/// Source `text.matchAll(TOKEN | TOKEN_MATH)`: leftmost matches, left to right.
fn tokens(s: &str, with_math: bool) -> Vec<(usize, usize, Token<'_>)> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < s.len() {
        let hit = match s.as_bytes()[at] {
            b'[' => match_ref(s, at),
            b'@' => match_mention(s, at),
            b'=' if s[at..].starts_with("==") => Some((Token::Highlight, at + 2)),
            b'$' if with_math => match_math(s, at),
            _ => None,
        };
        match hit {
            Some((tok, end)) => {
                out.push((at, end, tok));
                at = end;
            }
            None => at += s[at..].chars().next().map_or(1, char::len_utf8),
        }
    }
    out
}

/// `REF_FULL.exec(text)`: the whole string is one `[[kind:id(|label)?]]`.
fn ref_full(s: &str) -> Option<(&str, &str, Option<&str>)> {
    match match_ref(s, 0)? {
        (Token::Ref { kind, id, label }, end) if end == s.len() => Some((kind, id, label)),
        _ => None,
    }
}

/// `DETAILS_OPEN = /^<details>\s*<summary>([\s\S]*?)<\/summary>\s*$/`.
fn details_summary(value: &str) -> Option<&str> {
    let rest = value.strip_prefix("<details>")?;
    let rest = rest
        .trim_start_matches(js_space)
        .strip_prefix("<summary>")?;
    let mut from = 0;
    while let Some(i) = rest[from..].find("</summary>") {
        let close = from + i;
        if rest[close + "</summary>".len()..].chars().all(js_space) {
            return Some(&rest[..close]);
        }
        from = close + 1;
    }
    None
}

/// `/^<br\s*\/?>$/i`.
fn is_br(v: &str) -> bool {
    let Some(head) = v.get(..3) else {
        return false;
    };
    if !head.eq_ignore_ascii_case("<br") {
        return false;
    }
    let rest = v[3..].trim_start_matches(js_space);
    rest == ">" || rest == "/>"
}

/// `CALLOUT_MARKER = /^\[!(NOTE|TIP|WARNING|CAUTION)\]\n?/` -> (kind, match length).
fn callout_marker(v: &str) -> Option<(&'static str, usize)> {
    for (tag, kind) in [
        ("[!NOTE]", "note"),
        ("[!TIP]", "tip"),
        ("[!WARNING]", "warning"),
        ("[!CAUTION]", "caution"),
    ] {
        if let Some(rest) = v.strip_prefix(tag) {
            return Some((kind, tag.len() + usize::from(rest.starts_with('\n'))));
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Marks

/// Source `withMarks`: keep the first of each emitted mark type (underline is
/// dropped here, as in the source).
fn with_marks(mut node: Map<String, Value>, marks: &[Value]) -> Value {
    let mut clean: Vec<Value> = Vec::new();
    let mut seen: Vec<&str> = Vec::new();
    for mark in marks {
        let ty = mark["type"].as_str().unwrap_or("");
        if seen.contains(&ty)
            || !matches!(
                ty,
                "bold" | "italic" | "strike" | "code" | "link" | "highlight"
            )
        {
            continue;
        }
        seen.push(ty);
        clean.push(mark.clone());
    }
    if !clean.is_empty() {
        node.insert("marks".into(), Value::Array(clean));
    }
    Value::Object(node)
}

fn make_text(text: &str, marks: &[Value]) -> Value {
    let mut node = Map::new();
    node.insert("type".into(), json!("text"));
    node.insert("text".into(), json!(text));
    with_marks(node, marks)
}

fn math_inline(latex: &str, marks: &[Value]) -> Value {
    let mut node = Map::new();
    node.insert("type".into(), json!("mathInline"));
    node.insert("attrs".into(), json!({ "latex": latex }));
    with_marks(node, marks)
}

fn plus(marks: &[Value], mark: Value) -> Vec<Value> {
    let mut out = marks.to_vec();
    out.push(mark);
    out
}

fn with_highlight(marks: &[Value], on: bool) -> Vec<Value> {
    if on {
        plus(marks, json!({ "type": "highlight" }))
    } else {
        marks.to_vec()
    }
}

/// Source `tokenizeText`.
fn tokenize_text(text: &str, marks: &[Value], with_math: bool) -> Vec<Value> {
    let mut out = Vec::new();
    let mut last = 0;
    let mut highlight = false;
    for (start, end, tok) in tokens(text, with_math) {
        if start > last {
            out.push(make_text(
                &text[last..start],
                &with_highlight(marks, highlight),
            ));
        }
        match tok {
            Token::Ref { kind, id, label } => out.push(json!({
                "type": "mention",
                "attrs": { "entity": ref_entity(kind), "id": id, "label": label.unwrap_or("") },
            })),
            Token::Mention { kind, id, label } => out.push(json!({
                "type": "mention",
                "attrs": { "entity": kind, "id": id, "label": label },
            })),
            Token::Highlight => highlight = !highlight,
            Token::Math(latex) => out.push(math_inline(latex, &with_highlight(marks, highlight))),
        }
        last = end;
    }
    if last < text.len() {
        out.push(make_text(&text[last..], &with_highlight(marks, highlight)));
    }
    out
}

fn end_offset(p: &Option<Position>) -> Option<usize> {
    p.as_ref().map(|p| p.end.offset)
}

struct MathSpan {
    start: usize,
    end: usize,
    fence: usize,
}

// ---------------------------------------------------------------------------
// Walk

struct Cx<'a> {
    /// The parsed text; mdast offsets index it.
    source: &'a str,
    /// The caller's text (differs from `source` only where it had U+0000).
    original: &'a str,
    /// Byte offsets of U+0000 in `original`.
    nuls: &'a [usize],
}

impl Cx<'_> {
    /// `source.slice(start, end)` of the original text for `source` offsets.
    fn original_slice(&self, start: usize, end: usize) -> &str {
        // Each U+0000 (1 byte) became U+FFFD (3 bytes) in `source`.
        let map = |r: usize| {
            let before = self
                .nuls
                .iter()
                .enumerate()
                .take_while(|(k, p)| **p + 2 * k < r)
                .count();
            r - 2 * before
        };
        &self.original[map(start)..map(end)]
    }

    /// Source `blocksFromNodes`.
    fn blocks_from_nodes(&self, nodes: &[Node]) -> Vec<Value> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < nodes.len() {
            let node = &nodes[i];
            if let Node::Html(h) = node {
                if let Some(summary) = details_summary(&h.value) {
                    let mut j = i + 1;
                    while j < nodes.len() && !is_details_close(&nodes[j]) {
                        j += 1;
                    }
                    let inner = self.blocks_from_nodes(&nodes[i + 1..j]);
                    let summary_content = if summary.is_empty() {
                        json!([])
                    } else {
                        json!([{ "type": "text", "text": summary }])
                    };
                    let inner = if inner.is_empty() {
                        json!([{ "type": "paragraph" }])
                    } else {
                        Value::Array(inner)
                    };
                    out.push(json!({
                        "type": "details",
                        "content": [
                            { "type": "detailsSummary", "content": summary_content },
                            { "type": "detailsContent", "content": inner },
                        ],
                    }));
                    i = j + 1;
                    continue;
                }
            }
            if let Node::List(list) = node {
                out.push(self.list_to_node(list));
                i += 1;
                continue;
            }
            if let Some(blk) = self.node_to_block(node) {
                out.push(blk);
            }
            i += 1;
        }
        out
    }

    fn node_to_block(&self, node: &Node) -> Option<Value> {
        match node {
            Node::Paragraph(p) => Some(self.paragraph_to_block(&p.children)),
            Node::Heading(h) => Some(json!({
                "type": "heading",
                "attrs": { "level": h.depth.clamp(1, 6) },
                "content": self.map_phrasing(&h.children, &[]),
            })),
            Node::Blockquote(bq) => Some(self.blockquote_to_block(&bq.children)),
            Node::Code(code) => {
                let lang = code.lang.as_deref().unwrap_or("");
                if lang.to_lowercase() == "mermaid" {
                    return Some(json!({ "type": "mermaid", "attrs": { "source": code.value } }));
                }
                let content = if code.value.is_empty() {
                    json!([])
                } else {
                    json!([{ "type": "text", "text": code.value }])
                };
                Some(json!({
                    "type": "codeBlock",
                    "attrs": { "language": lang },
                    "content": content,
                }))
            }
            Node::Math(m) => Some(json!({ "type": "math", "attrs": { "latex": m.value } })),
            Node::Table(t) => Some(self.table_to_block(t)),
            Node::ThematicBreak(_) => Some(json!({ "type": "horizontalRule" })),
            _ => None,
        }
    }

    fn paragraph_to_block(&self, children: &[Node]) -> Value {
        if let [only] = children {
            if let Node::Text(t) = only {
                if let Some((kind, id, label)) = ref_full(js_trim(&t.value)) {
                    if !(kind == "doc" && label.is_some()) {
                        return json!({
                            "type": "embed",
                            "attrs": { "entity": ref_entity(kind), "ref": id },
                        });
                    }
                }
            }
            let attachment = match only {
                Node::Link(l) => Some((&l.url, plain_text(&l.children), false)),
                Node::Image(img) => Some((&img.url, img.alt.clone(), true)),
                _ => None,
            };
            if let Some((url, name, image)) = attachment {
                // `ATTACHMENT_URL = /^attachment:(.+)$/`: `.` stops at line terminators.
                if let Some(id) = url.strip_prefix("attachment:") {
                    if !id.is_empty() && !id.contains(['\n', '\r', '\u{2028}', '\u{2029}']) {
                        return json!({
                            "type": "attachment",
                            "attrs": { "id": id, "name": name, "image": image },
                        });
                    }
                }
            }
        }
        json!({ "type": "paragraph", "content": self.map_phrasing(children, &[]) })
    }

    /// Source `blockquoteToBlock`: a leading `[!KIND]` marker makes a callout.
    fn blockquote_to_block(&self, children: &[Node]) -> Value {
        if let Some(Node::Paragraph(first)) = children.first() {
            if let Some(Node::Text(first_text)) = first.children.first() {
                if let Some((kind, len)) = callout_marker(&first_text.value) {
                    let remainder = &first_text.value[len..];
                    let mut para_children = Vec::with_capacity(first.children.len());
                    if !remainder.is_empty() {
                        para_children.push(Node::Text(mdast::Text {
                            value: remainder.to_string(),
                            position: first_text.position.clone(),
                        }));
                    }
                    para_children.extend(first.children[1..].iter().cloned());
                    let mut nodes = vec![Node::Paragraph(mdast::Paragraph {
                        children: para_children,
                        position: first.position.clone(),
                    })];
                    nodes.extend(children[1..].iter().cloned());
                    return json!({
                        "type": "callout",
                        "attrs": { "kind": kind },
                        "content": self.blocks_from_nodes(&nodes),
                    });
                }
            }
        }
        json!({ "type": "blockquote", "content": self.blocks_from_nodes(children) })
    }

    fn list_item_content(&self, li: &Node) -> Value {
        let content = self.blocks_from_nodes(li.children().map_or(&[][..], |c| c.as_slice()));
        if content.is_empty() {
            json!([{ "type": "paragraph" }])
        } else {
            Value::Array(content)
        }
    }

    fn list_to_node(&self, list: &mdast::List) -> Value {
        let checks: Vec<Option<bool>> = list
            .children
            .iter()
            .map(|li| match li {
                Node::ListItem(item) => item.checked,
                _ => None,
            })
            .collect();
        if checks.iter().any(Option::is_some) {
            let items: Vec<Value> = list
                .children
                .iter()
                .zip(&checks)
                .map(|(li, c)| {
                    json!({
                        "type": "taskItem",
                        "attrs": { "checked": *c == Some(true) },
                        "content": self.list_item_content(li),
                    })
                })
                .collect();
            return json!({ "type": "taskList", "content": items });
        }
        let items: Vec<Value> = list
            .children
            .iter()
            .map(|li| json!({ "type": "listItem", "content": self.list_item_content(li) }))
            .collect();
        json!({
            "type": if list.ordered { "orderedList" } else { "bulletList" },
            "content": items,
        })
    }

    fn table_to_block(&self, t: &mdast::Table) -> Value {
        let rows: Vec<Value> = t
            .children
            .iter()
            .enumerate()
            .map(|(row_index, row)| {
                let cells: Vec<Value> = row
                    .children()
                    .map_or(&[][..], |c| c.as_slice())
                    .iter()
                    .map(|cell| {
                        let kids = cell.children().map_or(&[][..], |c| c.as_slice());
                        json!({
                            "type": if row_index == 0 { "tableHeader" } else { "tableCell" },
                            "content": [{
                                "type": "paragraph",
                                "content": self.map_phrasing(kids, &[]),
                            }],
                        })
                    })
                    .collect();
                json!({ "type": "tableRow", "content": cells })
            })
            .collect();
        json!({ "type": "table", "content": rows })
    }

    /// Source `mathSpan`: fence width and inner raw text from the source.
    fn math_span(&self, position: &Option<Position>) -> Option<MathSpan> {
        let p = position.as_ref()?;
        let (start, end) = (p.start.offset, p.end.offset);
        let raw = self.source.get(start..end)?;
        let fence = raw.len() - raw.trim_start_matches('$').len();
        if fence == 0 || raw.len() < fence * 2 {
            return None;
        }
        Some(MathSpan { start, end, fence })
    }

    /// Source `isMathText` (Pandoc/GitHub `$` rules).
    fn is_math_text(&self, span: &MathSpan, value: &str) -> bool {
        if js_trim(value).is_empty() {
            return false;
        }
        if span.fence > 1 {
            return value == js_trim(value);
        }
        let inner = &self.source[span.start + span.fence..span.end - span.fence];
        if inner != js_trim(inner) || inner.starts_with('{') {
            return false;
        }
        !self.source[span.end..].starts_with(|c: char| c.is_ascii_digit())
    }

    /// Source `mapPhrasing`.
    fn map_phrasing(&self, nodes: &[Node], marks: &[Value]) -> Vec<Value> {
        let mut out = Vec::new();
        let mut underline = false;
        let mut i = 0;
        while i < nodes.len() {
            let n = &nodes[i];
            if let Node::Html(h) = n {
                let v = js_trim(&h.value);
                if v == "<u>" {
                    underline = true;
                } else if v == "</u>" {
                    underline = false;
                } else if is_br(v) {
                    out.push(json!({ "type": "hardBreak" }));
                }
                i += 1;
                continue;
            }
            let next = if underline {
                plus(marks, json!({ "type": "underline" }))
            } else {
                marks.to_vec()
            };
            match n {
                Node::Text(t) => out.extend(tokenize_text(&t.value, &next, false)),
                Node::InlineCode(c) => {
                    out.push(make_text(&c.value, &plus(&next, json!({ "type": "code" }))))
                }
                Node::Strong(s) => out.extend(
                    self.map_phrasing(&s.children, &plus(&next, json!({ "type": "bold" }))),
                ),
                Node::Emphasis(e) => out.extend(
                    self.map_phrasing(&e.children, &plus(&next, json!({ "type": "italic" }))),
                ),
                Node::Delete(d) => out.extend(
                    self.map_phrasing(&d.children, &plus(&next, json!({ "type": "strike" }))),
                ),
                Node::Link(l) => out.extend(self.map_phrasing(
                    &l.children,
                    &plus(&next, json!({ "type": "link", "attrs": { "href": l.url } })),
                )),
                Node::Image(img) => {
                    if img.url.starts_with("http://") || img.url.starts_with("https://") {
                        let text = if img.alt.is_empty() {
                            &img.url
                        } else {
                            &img.alt
                        };
                        out.push(make_text(
                            text,
                            &plus(
                                &next,
                                json!({ "type": "link", "attrs": { "href": img.url } }),
                            ),
                        ));
                    } else if !img.alt.is_empty() {
                        out.push(make_text(&img.alt, &next));
                    }
                }
                Node::Break(_) => out.push(json!({ "type": "hardBreak" })),
                Node::InlineMath(m) => {
                    let span = self.math_span(&m.position);
                    let Some(span) = span.filter(|s| !self.is_math_text(s, &m.value)) else {
                        out.push(math_inline(&m.value, &next));
                        i += 1;
                        continue;
                    };
                    // micromark paired a currency `$`: rescan the raw source of this
                    // span plus the following text siblings with the Pandoc rule.
                    let mut stop = span.end;
                    let mut last = i;
                    while let Some(Node::Text(t)) = nodes.get(last + 1) {
                        let Some(offset) = end_offset(&t.position) else {
                            break;
                        };
                        stop = offset;
                        last += 1;
                    }
                    let region = self.original_slice(span.start, stop);
                    if region.contains('\\') {
                        let fence = "$".repeat(span.fence);
                        out.push(make_text(&format!("{fence}{}{fence}", m.value), &next));
                        i += 1;
                        continue;
                    }
                    out.extend(tokenize_text(region, &next, true));
                    i = last;
                }
                _ => {}
            }
            i += 1;
        }
        out
    }
}

fn is_details_close(n: &Node) -> bool {
    matches!(n, Node::Html(h) if js_trim(&h.value) == "</details>")
}

/// Source `plainText`: text/code values through strong/emphasis/delete.
fn plain_text(nodes: &[Node]) -> String {
    let mut out = String::new();
    for n in nodes {
        match n {
            Node::Text(t) => out.push_str(&t.value),
            Node::InlineCode(c) => out.push_str(&c.value),
            Node::Strong(s) => out.push_str(&plain_text(&s.children)),
            Node::Emphasis(e) => out.push_str(&plain_text(&e.children)),
            Node::Delete(d) => out.push_str(&plain_text(&d.children)),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn oracle_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("compat/fixtures/markdown-oracle")
    }

    /// `(name, markdown)` for every corpus input (`*.md`, not `*.roundtrip.md`).
    fn corpus() -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = std::fs::read_dir(oracle_dir())
            .expect("oracle dir")
            .map(|e| e.expect("entry").path())
            .filter(|p| {
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                name.ends_with(".md") && !name.ends_with(".roundtrip.md")
            })
            .map(|p| {
                let stem = p.file_stem().and_then(|s| s.to_str()).unwrap().to_string();
                (stem, std::fs::read_to_string(&p).expect("utf-8 markdown"))
            })
            .collect();
        out.sort();
        assert!(out.len() >= 30, "oracle corpus missing: {}", out.len());
        out
    }

    fn expected(name: &str, ext: &str) -> String {
        std::fs::read_to_string(oracle_dir().join(format!("{name}.{ext}"))).expect("oracle output")
    }

    /// Exact JSON equality with the TS `mdToTiptapJson` output (object key order
    /// is not semantic; the fixture is written with sorted keys).
    #[test]
    fn md_to_tiptap_matches_ts_oracle() {
        let mut failed = Vec::new();
        for (name, md) in corpus() {
            let want: Value = serde_json::from_str(&expected(&name, "json")).unwrap();
            let got = md_to_tiptap(&md).unwrap();
            if got != want {
                failed.push(format!(
                    "{name}\n  want {}\n  got  {}",
                    serde_json::to_string(&want).unwrap(),
                    serde_json::to_string(&got).unwrap()
                ));
            }
        }
        assert!(
            failed.is_empty(),
            "{} mismatches:\n{}",
            failed.len(),
            failed.join("\n")
        );
    }

    /// md -> Tiptap -> md (Rust `tiptap_doc_to_md`) equals the TS
    /// `tiptapDocToMd(mdToTiptapJson(md))` output byte for byte.
    #[test]
    fn md_round_trip_matches_ts_oracle() {
        let mut failed = Vec::new();
        for (name, md) in corpus() {
            let want = expected(&name, "roundtrip.md");
            let got = crate::share_render::tiptap_doc_to_md(&md_to_tiptap(&md).unwrap());
            if got != want {
                failed.push(format!("{name}\n  want {want:?}\n  got  {got:?}"));
            }
        }
        assert!(
            failed.is_empty(),
            "{} mismatches:\n{}",
            failed.len(),
            failed.join("\n")
        );
    }

    /// Legal publish: `tiptapDocToSafeHtml(mdToTiptapJson(md))`.
    #[test]
    fn md_to_safe_html_matches_ts_oracle() {
        let mut failed = Vec::new();
        for (name, md) in corpus() {
            let want = expected(&name, "html");
            let got = md_to_safe_html(&md).unwrap();
            if got != want {
                failed.push(format!("{name}\n  want {want:?}\n  got  {got:?}"));
            }
        }
        assert!(
            failed.is_empty(),
            "{} mismatches:\n{}",
            failed.len(),
            failed.join("\n")
        );
    }

    #[test]
    fn empty_and_blank_input_is_one_empty_paragraph() {
        for md in ["", "   \n\n", "<!-- only a comment -->", "[r]: https://x"] {
            assert_eq!(
                md_to_tiptap(md).unwrap(),
                json!({ "type": "doc", "content": [{ "type": "paragraph" }] }),
                "{md:?}"
            );
        }
    }

    #[test]
    fn link_hrefs_are_kept_verbatim_for_render_time_sanitizing() {
        // parse.ts stores the mdast url as-is; schemes are filtered by the HTML
        // renderer (`tiptap_doc_to_safe_html`), not here.
        let doc = md_to_tiptap("[x](javascript:alert(1))").unwrap();
        assert_eq!(
            doc["content"][0]["content"][0]["marks"][0],
            json!({ "type": "link", "attrs": { "href": "javascript:alert(1)" } })
        );
    }

    /// Deep containers are refused (not a stack overflow); the TS parser
    /// throws `RangeError` and the Node helper answers `invalid_input`.
    #[test]
    fn deep_nesting_is_rejected_without_overflowing() {
        let run = |md: String| {
            std::thread::Builder::new()
                .stack_size(2 << 20)
                .spawn(move || md_to_tiptap(&md).map(|_| ()))
                .unwrap()
                .join()
                .expect("no stack overflow / panic")
        };
        // doc + content (2) + 2 per blockquote + paragraph/content/text (3):
        // 60 blockquotes = 125 levels fit, 61 = 127 do not.
        assert_eq!(run("> ".repeat(60) + "x"), Ok(()));
        assert_eq!(run("> ".repeat(61) + "x"), Err(TooDeep));
        assert_eq!(run("> ".repeat(20_000) + "x"), Err(TooDeep));
        // Inline nesting flattens into marks: deep but storable.
        assert_eq!(run("*a ".repeat(200) + &"b* ".repeat(200)), Ok(()));
        // Links do not nest (the innermost wins), so this stays shallow.
        assert_eq!(
            run("[".repeat(10_000) + "x" + &"](u)".repeat(10_000)),
            Ok(())
        );
    }
}
