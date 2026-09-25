//! Public-share renderers for Tiptap JSON documents.
//!
//! Ports, at source SHA `393795261322b916e588043cf94feca999175843`:
//! - `packages/editor/src/html.ts` `tiptapDocToSafeHtml` =
//!   `sanitizeRenderedHtml(tiptapDocToHtml(doc))` with the options in
//!   `packages/editor/src/sanitize.ts` (sanitize-html 2.17.7 / htmlparser2 12,
//!   `disallowedTagsMode: "discard"`, `allowedSchemes: [http, https, mailto]`,
//!   `allowProtocolRelative: false`);
//! - `packages/editor/src/md.ts` `tiptapDocToMd`;
//! - `packages/editor/src/json.ts` `isTiptapDoc`.
//!
//! The HTML renderer is not a general sanitizer. The source renderer only emits
//! a fixed, small tag set, so this port emits what sanitize-html *outputs* for
//! each construct directly:
//! - text: htmlparser2 decodes the renderer's `&amp; &lt; &gt; &quot;` back to
//!   the original text and sanitize-html's `escapeHtml(text, false)` re-escapes
//!   only `& < >` — double quotes in text are emitted raw;
//! - attribute values: `escapeHtml(value, true)` escapes `& < > "`;
//! - `h4`/`h5`/`h6` (and non-integer heading levels such as `h2.5`) are not in
//!   `allowedTags`, so the tag is discarded and its content kept;
//! - `data-embed` is not an allowed attribute → `<div>ref</div>`;
//! - empty allowed attributes (`data-math`, `data-mermaid`, `data-math-inline`)
//!   are emitted without a value (`<pre data-math>`), because sanitize-html only
//!   writes `=""` for `allowedEmptyAttributes` (`alt`);
//! - `br`/`hr` are in sanitize-html's `selfClosing` list → `<br />`, `<hr />`;
//! - `<a href>` keeps `href` only when launder's `naughtyHref` accepts it (see
//!   [`href_allowed`]); otherwise the tag stays as a bare `<a>`;
//! - htmlparser2's `openImpliesClose` has `a → {a}`: a link mark opened
//!   directly inside another link mark closes the outer one first. This is the
//!   only implied-close rule the renderer can trigger (block tags never open
//!   while a `<p>`/heading/cell is the innermost open element).
//!
//! Known differences vs the TS source:
//! - `tiptapDocToMd`'s `selfCheckedMd` re-parses its first pass with the
//!   Markdown parser (`mdToTiptapJson`) and, when the inline math/text shape
//!   disagrees, retries with `$` escaped and wider math fences. The parser is
//!   not ported, so [`self_checked_md`] always returns the first pass. This can
//!   only differ for paragraphs/headings whose first-pass output contains `$`
//!   and whose reparse would not match the source math shape.
//! - `callout` `kind.toUpperCase()` uses Rust's `str::to_uppercase`; both are
//!   full Unicode case mappings but may differ across Unicode versions.
//! - Recursion has no explicit depth limit (neither does the source); callers
//!   pass `serde_json` values, whose parser caps nesting at 128.

use serde_json::Value;

use crate::collab::derived_body::emoji_glyph;

/// Source `isTiptapDoc` (`packages/editor/src/json.ts`): a non-array object
/// with `type === "doc"` whose `content`, when present, is an array.
pub fn is_tiptap_doc(doc: &Value) -> bool {
    let Some(obj) = doc.as_object() else {
        return false;
    };
    if obj.get("type").and_then(Value::as_str) != Some("doc") {
        return false;
    }
    match obj.get("content") {
        None => true,
        Some(content) => content.is_array(),
    }
}

// ---------------------------------------------------------------------------
// Shared node accessors (JS `isRecord`-style: missing / wrong type → default).
// `Value::get(&str)` returns `None` for arrays and scalars, which matches a JS
// property read on an array record for the keys used here.

fn node_type(n: &Value) -> &str {
    n.get("type").and_then(Value::as_str).unwrap_or("")
}

fn node_content(n: &Value) -> Option<&Vec<Value>> {
    n.get("content").and_then(Value::as_array)
}

fn node_attr<'a>(n: &'a Value, key: &str) -> Option<&'a Value> {
    n.get("attrs").and_then(|a| a.get(key))
}

fn str_attr<'a>(n: &'a Value, key: &str) -> &'a str {
    node_attr(n, key).and_then(Value::as_str).unwrap_or("")
}

fn text_of(n: &Value) -> Option<&str> {
    n.get("text").and_then(Value::as_str)
}

/// Clamped heading level as a JS number (`Math.min(6, Math.max(1, level))`,
/// default 1 when `level` is not a number).
fn heading_level(n: &Value) -> f64 {
    match node_attr(n, "level").and_then(Value::as_f64) {
        Some(level) => level.clamp(1.0, 6.0),
        None => 1.0,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Mark {
    ty: String,
    href: Option<String>,
}

fn raw_marks(n: &Value) -> impl Iterator<Item = Mark> + '_ {
    n.get("marks")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|m| {
            let ty = m.get("type").and_then(Value::as_str)?;
            let href = m
                .get("attrs")
                .and_then(|a| a.get("href"))
                .and_then(Value::as_str)
                .map(str::to_string);
            Some(Mark {
                ty: ty.to_string(),
                href,
            })
        })
}

// ---------------------------------------------------------------------------
// HTML

/// sanitize-html `escapeHtml(text, false)` applied to already-decoded text.
fn push_text(out: &mut String, s: &str) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
}

/// sanitize-html `escapeHtml(value, true)` for attribute values.
fn push_attr(out: &mut String, s: &str) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
}

/// launder 1.7.1 `naughtyHref` with `allowedSchemes: [http, https, mailto]`
/// and `allowProtocolRelative: false`, negated.
///
/// `cleanHref` first removes every char in `\x00-\x20` and every
/// `<!-- ... -->` comment; then `^([a-zA-Z][a-zA-Z0-9.\-+]*):` detects a
/// scheme (compared lower-cased). Without a scheme, a leading pair of `/` or
/// `\` is protocol-relative (rejected); anything else is relative (kept). The
/// emitted value is the original, uncleaned href.
fn href_allowed(href: &str) -> bool {
    let mut cleaned: String = href.chars().filter(|&c| c > '\u{20}').collect();
    while let Some(first) = cleaned.find("<!--") {
        let Some(rel) = cleaned[first + 4..].find("-->") else {
            break;
        };
        let last = first + 4 + rel;
        cleaned.replace_range(first..last + 3, "");
    }
    let bytes = cleaned.as_bytes();
    let scheme_len = if bytes.first().is_some_and(u8::is_ascii_alphabetic) {
        let rest = bytes[1..]
            .iter()
            .take_while(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'+'))
            .count();
        let len = 1 + rest;
        (bytes.get(len) == Some(&b':')).then_some(len)
    } else {
        None
    };
    match scheme_len {
        Some(len) => {
            let scheme = cleaned[..len].to_ascii_lowercase();
            matches!(scheme.as_str(), "http" | "https" | "mailto")
        }
        None => {
            !(bytes.len() >= 2
                && matches!(bytes[0], b'/' | b'\\')
                && matches!(bytes[1], b'/' | b'\\'))
        }
    }
}

enum HtmlTag<'a> {
    Plain(&'static str),
    Link(&'a str),
}

/// Source `wrapMarks` followed by sanitization. Marks wrap innermost-first;
/// htmlparser2 closes an open `<a>` when another `<a>` opens directly inside it.
fn push_wrapped(out: &mut String, inner: &str, marks: &[Mark]) {
    let tags: Vec<HtmlTag<'_>> = marks
        .iter()
        .filter_map(|m| match m.ty.as_str() {
            "bold" => Some(HtmlTag::Plain("strong")),
            "italic" => Some(HtmlTag::Plain("em")),
            "strike" => Some(HtmlTag::Plain("s")),
            "code" => Some(HtmlTag::Plain("code")),
            "link" => m
                .href
                .as_deref()
                .filter(|h| !h.is_empty())
                .map(HtmlTag::Link),
            _ => None,
        })
        .collect();
    let mut open: Vec<&'static str> = Vec::new();
    for tag in tags.iter().rev() {
        match tag {
            HtmlTag::Plain(name) => {
                out.push('<');
                out.push_str(name);
                out.push('>');
                open.push(name);
            }
            HtmlTag::Link(href) => {
                if open.last() == Some(&"a") {
                    out.push_str("</a>");
                    open.pop();
                }
                if href_allowed(href) {
                    out.push_str("<a href=\"");
                    push_attr(out, href);
                    out.push_str("\">");
                } else {
                    out.push_str("<a>");
                }
                open.push("a");
            }
        }
    }
    out.push_str(inner);
    for name in open.iter().rev() {
        out.push_str("</");
        out.push_str(name);
        out.push('>');
    }
}

fn inline_html(out: &mut String, nodes: Option<&Vec<Value>>) {
    let Some(nodes) = nodes else {
        return;
    };
    for n in nodes {
        match node_type(n) {
            "hardBreak" => out.push_str("<br />"),
            "mention" => {
                out.push('@');
                push_text(out, str_attr(n, "label"));
            }
            "text" => {
                let mut inner = String::new();
                push_text(&mut inner, text_of(n).unwrap_or(""));
                let marks: Vec<Mark> = raw_marks(n).collect();
                push_wrapped(out, &inner, &marks);
            }
            "mathInline" => {
                let mut inner = String::from("<span data-math-inline>");
                push_text(&mut inner, str_attr(n, "latex"));
                inner.push_str("</span>");
                let marks: Vec<Mark> = raw_marks(n).collect();
                push_wrapped(out, &inner, &marks);
            }
            "emoji" => push_text(out, &emoji_glyph(n)),
            _ => inline_html(out, node_content(n)),
        }
    }
}

fn list_html(out: &mut String, n: &Value, tag: &str) {
    out.push('<');
    out.push_str(tag);
    out.push('>');
    for item in node_content(n).into_iter().flatten() {
        out.push_str("<li>");
        blocks_html(out, node_content(item));
        out.push_str("</li>");
    }
    out.push_str("</");
    out.push_str(tag);
    out.push('>');
}

fn table_html(out: &mut String, n: &Value) {
    out.push_str("<table>");
    for row in node_content(n)
        .into_iter()
        .flatten()
        .filter(|r| node_type(r) == "tableRow")
    {
        out.push_str("<tr>");
        for cell in node_content(row).into_iter().flatten() {
            let tag = if node_type(cell) == "tableHeader" {
                "th"
            } else {
                "td"
            };
            out.push('<');
            out.push_str(tag);
            out.push('>');
            blocks_html(out, node_content(cell));
            out.push_str("</");
            out.push_str(tag);
            out.push('>');
        }
        out.push_str("</tr>");
    }
    out.push_str("</table>");
}

fn block_html(out: &mut String, n: &Value) {
    let content = node_content(n);
    match node_type(n) {
        "attachment" => {
            out.push_str("<p>");
            push_text(out, str_attr(n, "name"));
            out.push_str("</p>");
        }
        "paragraph" => {
            out.push_str("<p>");
            inline_html(out, content);
            out.push_str("</p>");
        }
        "heading" => {
            let level = heading_level(n);
            // `h1`-`h3` are allowed; `h4`-`h6` and non-integer names such as
            // `h2.5` are discarded by sanitize-html with their text kept.
            let tag = if level == 1.0 {
                Some("h1")
            } else if level == 2.0 {
                Some("h2")
            } else if level == 3.0 {
                Some("h3")
            } else {
                None
            };
            if let Some(tag) = tag {
                out.push('<');
                out.push_str(tag);
                out.push('>');
            }
            inline_html(out, content);
            if let Some(tag) = tag {
                out.push_str("</");
                out.push_str(tag);
                out.push('>');
            }
        }
        "blockquote" => {
            out.push_str("<blockquote>");
            blocks_html(out, content);
            out.push_str("</blockquote>");
        }
        "codeBlock" => {
            out.push_str("<pre><code>");
            inline_html(out, content);
            out.push_str("</code></pre>");
        }
        "bulletList" => list_html(out, n, "ul"),
        "orderedList" => list_html(out, n, "ol"),
        "listItem" => blocks_html(out, content),
        "horizontalRule" => out.push_str("<hr />"),
        "math" => {
            out.push_str("<pre data-math>");
            push_text(out, str_attr(n, "latex"));
            out.push_str("</pre>");
        }
        "mermaid" => {
            out.push_str("<pre data-mermaid>");
            push_text(out, str_attr(n, "source"));
            out.push_str("</pre>");
        }
        "table" => table_html(out, n),
        "embed" => {
            out.push_str("<div>");
            push_text(out, str_attr(n, "ref"));
            out.push_str("</div>");
        }
        _ => {
            if content.is_some() {
                blocks_html(out, content);
            }
        }
    }
}

fn blocks_html(out: &mut String, nodes: Option<&Vec<Value>>) {
    for n in nodes.into_iter().flatten() {
        block_html(out, n);
    }
}

/// Source `tiptapDocToSafeHtml`. Callers check [`is_tiptap_doc`] first.
pub fn tiptap_doc_to_safe_html(doc: &Value) -> String {
    let mut out = String::new();
    blocks_html(&mut out, node_content(doc));
    out
}

// ---------------------------------------------------------------------------
// Markdown

/// `md.ts` `marksOf`: only emitted marks (code/bold/italic/strike/highlight and
/// links with a non-empty href) take part in run splitting.
fn md_marks(n: &Value) -> Vec<Mark> {
    raw_marks(n)
        .filter(|m| {
            matches!(
                m.ty.as_str(),
                "code" | "bold" | "italic" | "strike" | "highlight"
            ) || (m.ty == "link" && m.href.as_deref().is_some_and(|h| !h.is_empty()))
        })
        .collect()
}

#[derive(Clone, Copy, Default)]
struct MdOpts {
    escape_dollars: bool,
    wide_math: bool,
    raw_html: bool,
}

fn escape_md_inline(text: &str, opts: MdOpts) -> String {
    let literal = if opts.raw_html {
        text.to_string()
    } else {
        // `text.replace(/\\+|</g, ...)`: `<` → `\<`; a backslash run is doubled
        // only when the next char is `<`.
        let mut out = String::with_capacity(text.len());
        let bytes = text.as_bytes();
        let mut i = 0;
        while i < text.len() {
            match bytes[i] {
                b'<' => {
                    out.push_str("\\<");
                    i += 1;
                }
                b'\\' => {
                    let start = i;
                    while i < text.len() && bytes[i] == b'\\' {
                        i += 1;
                    }
                    let run = &text[start..i];
                    out.push_str(run);
                    if bytes.get(i) == Some(&b'<') {
                        out.push_str(run);
                    }
                }
                _ => {
                    let ch = text[i..].chars().next().unwrap_or_default();
                    out.push(ch);
                    i += ch.len_utf8().max(1);
                }
            }
        }
        out
    };
    let out = literal.replace("![", "!\\[");
    if opts.escape_dollars {
        out.replace('$', "\\$")
    } else {
        out
    }
}

fn escape_md_line_starts(text: &str, math_first: bool, code_first: bool) -> String {
    text.split('\n')
        .enumerate()
        .map(|(index, line)| {
            if line.starts_with("$$") && !(math_first && index == 0) {
                let run = line.bytes().take_while(|&b| b == b'$').count();
                return format!("{}{}", "\\$".repeat(run), &line[run..]);
            }
            if line.starts_with('#')
                || line.starts_with('>')
                || line.starts_with('-')
                || (line.starts_with('`') && !(code_first && index == 0))
            {
                return format!("\\{line}");
            }
            // `line.replace(/^(\d+)\./, "$1\\.")` — `\d` is ASCII here.
            let digits = line.bytes().take_while(u8::is_ascii_digit).count();
            if digits > 0 && line.as_bytes().get(digits) == Some(&b'.') {
                return format!("{}\\{}", &line[..digits], &line[digits..]);
            }
            line.to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// JS `\s` / `String.prototype.trim` whitespace (WhiteSpace + LineTerminator).
fn is_js_space(c: char) -> bool {
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

fn math_inline_md(raw: &str, wide: bool) -> String {
    // `raw.replace(/\s+/g, " ").trim()`
    let mut collapsed = String::with_capacity(raw.len());
    let mut in_space = false;
    for c in raw.chars() {
        if is_js_space(c) {
            if !in_space {
                collapsed.push(' ');
            }
            in_space = true;
        } else {
            collapsed.push(c);
            in_space = false;
        }
    }
    let latex = collapsed.trim_matches(' ');
    if latex.is_empty() {
        return String::new();
    }
    let fence = fence_for(latex, '$', if wide { 2 } else { 1 });
    let pad = if latex.starts_with('$') || latex.ends_with('$') {
        " "
    } else {
        ""
    };
    format!("{fence}{pad}{latex}{pad}{fence}")
}

struct MdRun {
    md: String,
    marks: Vec<Mark>,
}

/// Source `mergeTextNodes`: any record carrying a string `text` merges into a
/// previous such record with the same emitted marks (keeping the previous
/// record's other fields, as `{ ...prev, text }` does).
fn merge_text_nodes(nodes: &[Value]) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::with_capacity(nodes.len());
    for n in nodes {
        if let (Some(text), Some(prev)) = (text_of(n), out.last_mut()) {
            if n.is_object() {
                if let Some(prev_text) = text_of(prev) {
                    if prev.is_object() && md_marks(prev) == md_marks(n) {
                        let merged = format!("{prev_text}{text}");
                        if let Some(obj) = prev.as_object_mut() {
                            obj.insert("text".to_string(), Value::String(merged));
                        }
                        continue;
                    }
                }
            }
        }
        out.push(n.clone());
    }
    out
}

fn md_runs(nodes: &[Value], opts: MdOpts) -> Vec<MdRun> {
    let mut out: Vec<MdRun> = Vec::new();
    for n in merge_text_nodes(nodes) {
        let marks = md_marks(&n);
        let md = match node_type(&n) {
            "text" => {
                let text = text_of(&n).unwrap_or("");
                if marks.iter().any(|m| m.ty == "code") {
                    text.to_string()
                } else {
                    escape_md_inline(text, opts)
                }
            }
            "mathInline" => {
                let md = math_inline_md(str_attr(&n, "latex"), opts.wide_math);
                if !md.is_empty() && out.last().is_some_and(|r| r.md.ends_with('$')) {
                    format!("<!---->{md}")
                } else {
                    md
                }
            }
            "hardBreak" => "<br>".to_string(),
            "mention" => {
                let label = str_attr(&n, "label");
                if label.is_empty() {
                    String::new()
                } else {
                    format!("@{label}")
                }
            }
            "emoji" => emoji_glyph(&n),
            _ => {
                let children = node_content(&n).map(Vec::as_slice).unwrap_or(&[]);
                out.extend(md_runs(children, opts));
                continue;
            }
        };
        if let Some(prev) = out.last_mut() {
            if prev.marks == marks {
                prev.md.push_str(&md);
                continue;
            }
        }
        out.push(MdRun { md, marks });
    }
    out
}

/// Source `fenceFor`: one longer than the longest `marker` run, at least `minimum`.
fn fence_for(text: &str, marker: char, minimum: usize) -> String {
    let mut length = minimum;
    let mut run = 0usize;
    for c in text.chars() {
        if c == marker {
            run += 1;
            length = length.max(run + 1);
        } else {
            run = 0;
        }
    }
    marker.to_string().repeat(length)
}

fn literal_text(nodes: Option<&Vec<Value>>) -> String {
    nodes
        .into_iter()
        .flatten()
        .map(|n| text_of(n).unwrap_or(""))
        .collect()
}

fn fenced_code(text: &str, language: &str) -> String {
    let fence = fence_for(text, '`', 3);
    format!("{fence}{language}\n{text}\n{fence}")
}

/// `/^ .* $/.test(text) && /[^ ]/.test(text)` — `.` excludes JS line terminators.
fn space_padded_non_blank(text: &str) -> bool {
    text.len() >= 2
        && text.starts_with(' ')
        && text.ends_with(' ')
        && !text.contains(['\n', '\r', '\u{2028}', '\u{2029}'])
        && text.chars().any(|c| c != ' ')
}

fn wrap_md_marks(run: &MdRun) -> String {
    let marks = &run.marks;
    let has = |ty: &str| marks.iter().any(|m| m.ty == ty);
    let mut text = run.md.clone();
    if has("code") {
        let fence = fence_for(&text, '`', 1);
        let pad = if text.starts_with('`') || text.ends_with('`') || space_padded_non_blank(&text) {
            " "
        } else {
            ""
        };
        text = format!("{fence}{pad}{text}{pad}{fence}");
    }
    if has("bold") {
        text = format!("**{text}**");
    }
    if has("italic") {
        text = format!("*{text}*");
    }
    if has("strike") {
        text = format!("~~{text}~~");
    }
    if has("highlight") {
        text = format!("=={text}==");
    }
    let link = marks
        .iter()
        .find(|m| m.ty == "link" && m.href.as_deref().is_some_and(|h| !h.is_empty()));
    match link.and_then(|m| m.href.as_deref()) {
        Some(href) => format!("[{text}]({href})"),
        None => text,
    }
}

fn inline_md(nodes: Option<&Vec<Value>>, opts: MdOpts) -> String {
    let Some(nodes) = nodes else {
        return String::new();
    };
    md_runs(nodes, opts).iter().map(wrap_md_marks).collect()
}

/// Source `selfCheckedMd`, **first pass only**.
///
/// Known difference: the source re-parses the first pass with `mdToTiptapJson`
/// when it contains `$` and, if the inline math/text shape differs from the
/// document, retries with `escapeDollars`/`wideMath` (accepting the retry only
/// when its reparse matches). The Markdown parser is not ported, so outputs
/// differ only for paragraphs/headings containing `$` where that reparse would
/// have disagreed.
fn self_checked_md(content: Option<&Vec<Value>>, wrap: impl Fn(&str) -> String) -> String {
    wrap(&inline_md(content, MdOpts::default()))
}

fn cell_text(cell: &Value) -> String {
    node_content(cell)
        .into_iter()
        .flatten()
        .map(|p| inline_md(node_content(p), MdOpts::default()))
        .collect::<Vec<_>>()
        .join(" ")
        .replace('|', "\\|")
}

fn gfm_table(n: &Value) -> String {
    let rows: Vec<&Value> = node_content(n)
        .into_iter()
        .flatten()
        .filter(|r| node_type(r) == "tableRow")
        .collect();
    let Some(first) = rows.first() else {
        return String::new();
    };
    let cells_of = |row: &Value| -> Vec<Value> { node_content(row).cloned().unwrap_or_default() };
    let line = |cells: &[Value]| -> String {
        format!(
            "| {} |",
            cells.iter().map(cell_text).collect::<Vec<_>>().join(" | ")
        )
    };
    let header = cells_of(first);
    let sep = format!("| {} |", vec!["---"; header.len()].join(" | "));
    let mut lines = vec![line(&header), sep];
    for row in &rows[1..] {
        lines.push(line(&cells_of(row)));
    }
    lines.join("\n")
}

fn list_md(n: &Value, ordered: bool, task: bool) -> String {
    node_content(n)
        .into_iter()
        .flatten()
        .enumerate()
        .map(|(i, item)| {
            let marker = if task {
                if node_attr(item, "checked") == Some(&Value::Bool(true)) {
                    "- [x] ".to_string()
                } else {
                    "- [ ] ".to_string()
                }
            } else if ordered {
                format!("{}. ", i + 1)
            } else {
                "- ".to_string()
            };
            let inner = blocks_md(node_content(item));
            let mut lines = inner.split('\n');
            let head = lines.next().unwrap_or("");
            let rest: Vec<&str> = lines.collect();
            let first = format!("{marker}{head}");
            if rest.is_empty() {
                return first;
            }
            let pad = " ".repeat(if task { 2 } else { marker.len() });
            let mut out = vec![first];
            out.extend(rest.iter().map(|l| {
                if l.is_empty() {
                    String::new()
                } else {
                    format!("{pad}{l}")
                }
            }));
            out.join("\n")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn block_md(n: &Value) -> String {
    let content = node_content(n);
    match node_type(n) {
        "paragraph" => {
            let first = content.and_then(|c| c.first());
            let math_first = first.is_some_and(|f| node_type(f) == "mathInline");
            let code_first = first.is_some_and(|f| md_marks(f).iter().any(|m| m.ty == "code"));
            self_checked_md(content, |inline| {
                escape_md_line_starts(inline, math_first, code_first)
            })
        }
        "heading" => {
            // `"#".repeat(nLevel)` truncates a fractional level.
            let hashes = "#".repeat(heading_level(n).trunc() as usize);
            self_checked_md(content, |inline| format!("{hashes} {inline}"))
        }
        "blockquote" => blocks_md(content)
            .split('\n')
            .map(|l| format!("> {l}"))
            .collect::<Vec<_>>()
            .join("\n"),
        "callout" => {
            let upper = str_attr(n, "kind").to_uppercase();
            let kind = if upper.is_empty() {
                "NOTE".to_string()
            } else {
                upper
            };
            let inner = blocks_md(content);
            let mut lines = vec![format!("> [!{kind}]")];
            lines.extend(inner.split('\n').map(|l| format!("> {l}")));
            lines.join("\n")
        }
        "details" => {
            let kids = content.map(Vec::as_slice).unwrap_or(&[]);
            let summary_node = kids.iter().find(|c| node_type(c) == "detailsSummary");
            let body_node = kids.iter().find(|c| node_type(c) == "detailsContent");
            let summary = inline_md(
                summary_node.and_then(node_content),
                MdOpts {
                    raw_html: true,
                    ..MdOpts::default()
                },
            );
            let body = blocks_md(body_node.and_then(node_content));
            format!("<details><summary>{summary}</summary>\n\n{body}\n\n</details>")
        }
        "math" => {
            let latex = str_attr(n, "latex");
            let fence = fence_for(latex, '$', 2);
            format!("{fence}\n{latex}\n{fence}")
        }
        "mermaid" => {
            let source = str_attr(n, "source");
            let source = if source.is_empty() {
                literal_text(content)
            } else {
                source.to_string()
            };
            fenced_code(&source, "mermaid")
        }
        "attachment" => {
            let id = str_attr(n, "id");
            let name = str_attr(n, "name");
            if node_attr(n, "image") == Some(&Value::Bool(true)) {
                format!("![{name}](attachment:{id})")
            } else {
                format!("[{name}](attachment:{id})")
            }
        }
        "taskList" => list_md(n, false, true),
        "taskItem" => blocks_md(content),
        "codeBlock" => fenced_code(&literal_text(content), str_attr(n, "language")),
        "bulletList" => list_md(n, false, false),
        "orderedList" => list_md(n, true, false),
        "listItem" => blocks_md(content),
        "horizontalRule" => "---".to_string(),
        "table" => gfm_table(n),
        "embed" => {
            let entity = match str_attr(n, "entity") {
                "" => "document",
                e => e,
            };
            let reference = str_attr(n, "ref");
            if entity == "url" {
                return reference.to_string();
            }
            let label = if entity == "document" { "doc" } else { entity };
            format!("[[{label}:{reference}]]")
        }
        _ => match content {
            Some(_) => blocks_md(content),
            None => String::new(),
        },
    }
}

fn blocks_md(nodes: Option<&Vec<Value>>) -> String {
    nodes
        .into_iter()
        .flatten()
        .map(block_md)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Source `tiptapDocToMd` (see the module docs for the self-check difference).
/// Callers check [`is_tiptap_doc`] first.
pub fn tiptap_doc_to_md(doc: &Value) -> String {
    let md = blocks_md(node_content(doc));
    if md.is_empty() {
        md
    } else {
        format!("{md}\n")
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use super::*;

    fn para(content: Value) -> Value {
        json!({"type": "doc", "content": [{"type": "paragraph", "content": content}]})
    }

    fn link(text: &str, href: &str) -> Value {
        json!({"type": "text", "text": text, "marks": [{"type": "link", "attrs": {"href": href}}]})
    }

    #[test]
    fn is_tiptap_doc_predicate() {
        assert!(is_tiptap_doc(&json!({"type": "doc"})));
        assert!(is_tiptap_doc(&json!({"type": "doc", "content": []})));
        assert!(!is_tiptap_doc(&json!({"type": "doc", "content": null})));
        assert!(!is_tiptap_doc(&json!({"type": "doc", "content": {}})));
        assert!(!is_tiptap_doc(&json!({"type": "paragraph"})));
        assert!(!is_tiptap_doc(&json!([{"type": "doc"}])));
        assert!(!is_tiptap_doc(&json!("doc")));
        assert!(!is_tiptap_doc(&Value::Null));
    }

    #[test]
    fn empty_doc_renders_empty() {
        for doc in [
            json!({"type": "doc"}),
            json!({"type": "doc", "content": []}),
        ] {
            assert_eq!(tiptap_doc_to_safe_html(&doc), "");
            assert_eq!(tiptap_doc_to_md(&doc), "");
        }
        let empty_para = json!({"type": "doc", "content": [{"type": "paragraph"}]});
        assert_eq!(tiptap_doc_to_safe_html(&empty_para), "<p></p>");
        assert_eq!(tiptap_doc_to_md(&empty_para), "");
    }

    #[test]
    fn html_escapes_script_text_and_attribute_quotes() {
        let doc = para(json!([
            {"type": "text", "text": "<script>alert(\"x\")</script> & &amp;"},
            link("a", "https://e.com/?a=1&b=\"2\"<>"),
        ]));
        assert_eq!(
            tiptap_doc_to_safe_html(&doc),
            "<p>&lt;script&gt;alert(\"x\")&lt;/script&gt; &amp; &amp;amp;\
             <a href=\"https://e.com/?a=1&amp;b=&quot;2&quot;&lt;&gt;\">a</a></p>"
        );
    }

    #[test]
    fn html_link_scheme_filtering() {
        let cases = [
            ("javascript:alert(1)", "<a>x</a>"),
            ("JAVASCRIPT:alert(1)", "<a>x</a>"),
            (" java\tscript:alert(1)", "<a>x</a>"),
            ("jav<!-- c -->ascript:alert(1)", "<a>x</a>"),
            ("data:text/html,x", "<a>x</a>"),
            ("//evil.com/x", "<a>x</a>"),
            ("\\\\evil.com", "<a>x</a>"),
            (" / /evil.com", "<a>x</a>"),
            ("mailto:a@b.c", "<a href=\"mailto:a@b.c\">x</a>"),
            ("HTTPS://X.COM", "<a href=\"HTTPS://X.COM\">x</a>"),
            ("/docs/1#x", "<a href=\"/docs/1#x\">x</a>"),
            ("1http:x", "<a href=\"1http:x\">x</a>"),
            ("", "x"),
        ];
        for (href, want) in cases {
            let doc = para(json!([link("x", href)]));
            assert_eq!(
                tiptap_doc_to_safe_html(&doc),
                format!("<p>{want}</p>"),
                "href {href:?}"
            );
        }
    }

    #[test]
    fn html_heading_levels_and_discarded_tags() {
        let doc = json!({"type": "doc", "content": [
            {"type": "heading", "attrs": {"level": 2}, "content": [{"type": "text", "text": "a"}]},
            {"type": "heading", "attrs": {"level": 5}, "content": [
                {"type": "text", "text": "b", "marks": [{"type": "bold"}]}
            ]},
            {"type": "embed", "attrs": {"ref": "r<1>"}},
            {"type": "horizontalRule"},
            {"type": "paragraph", "content": [{"type": "text", "text": "x"}, {"type": "hardBreak"}]},
        ]});
        assert_eq!(
            tiptap_doc_to_safe_html(&doc),
            "<h2>a</h2><strong>b</strong><div>r&lt;1&gt;</div><hr /><p>x<br /></p>"
        );
    }

    #[test]
    fn math_mermaid_and_inline_math() {
        let doc = json!({"type": "doc", "content": [
            {"type": "math", "attrs": {"latex": "a<b $$x$$"}},
            {"type": "mermaid", "attrs": {"source": "A-->B"}},
            {"type": "paragraph", "content": [
                {"type": "text", "text": "x "},
                {"type": "mathInline", "attrs": {"latex": "a\n b"}},
                {"type": "mathInline", "attrs": {"latex": "$c"}},
            ]},
        ]});
        assert_eq!(
            tiptap_doc_to_safe_html(&doc),
            "<pre data-math>a&lt;b $$x$$</pre><pre data-mermaid>A--&gt;B</pre>\
             <p>x <span data-math-inline>a\n b</span><span data-math-inline>$c</span></p>"
        );
        assert_eq!(
            tiptap_doc_to_md(&doc),
            "$$$\na<b $$x$$\n$$$\n\n```mermaid\nA-->B\n```\n\nx $a b$<!---->$$ $c $$\n"
        );
    }

    #[test]
    fn table_in_html_and_md() {
        let cell = |ty: &str, text: &str| json!({"type": ty, "content": [{"type": "paragraph", "content": [{"type": "text", "text": text}]}]});
        let doc = json!({"type": "doc", "content": [{"type": "table", "content": [
            {"type": "tableRow", "content": [cell("tableHeader", "A|B"), cell("tableHeader", "C")]},
            {"type": "tableRow", "content": [cell("tableCell", "1"), cell("tableCell", "2")]},
        ]}]});
        assert_eq!(
            tiptap_doc_to_safe_html(&doc),
            "<table><tr><th><p>A|B</p></th><th><p>C</p></th></tr>\
             <tr><td><p>1</p></td><td><p>2</p></td></tr></table>"
        );
        assert_eq!(
            tiptap_doc_to_md(&doc),
            "| A\\|B | C |\n| --- | --- |\n| 1 | 2 |\n"
        );
    }

    #[test]
    fn md_nested_lists_use_marker_width() {
        let p = |t: &str| json!({"type": "paragraph", "content": [{"type": "text", "text": t}]});
        let doc = json!({"type": "doc", "content": [{"type": "orderedList", "content": [
            {"type": "listItem", "content": [p("one"), {"type": "bulletList", "content": [
                {"type": "listItem", "content": [p("sub"), p("sub2")]}
            ]}]},
            {"type": "listItem", "content": [p("two")]},
        ]}]});
        assert_eq!(
            tiptap_doc_to_md(&doc),
            "1. one\n\n   - sub\n\n     sub2\n2. two\n"
        );
    }

    #[test]
    fn md_code_span_fences_and_padding() {
        let code =
            |t: &str| para(json!([{"type": "text", "text": t, "marks": [{"type": "code"}]}]));
        assert_eq!(tiptap_doc_to_md(&code("a`b")), "``a`b``\n");
        assert_eq!(tiptap_doc_to_md(&code("`x``")), "``` `x`` ```\n");
        assert_eq!(tiptap_doc_to_md(&code(" pad ")), "`  pad  `\n");
        assert_eq!(tiptap_doc_to_md(&code("  ")), "`  `\n");
    }

    #[test]
    fn md_inline_and_line_start_escapes() {
        let doc = para(json!([
            {"type": "text", "text": "see !"},
            {"type": "text", "text": "[img](x) \\< <b>"},
        ]));
        assert_eq!(tiptap_doc_to_md(&doc), "see !\\[img](x) \\\\\\< \\<b>\n");
        let doc = para(json!([{"type": "text", "text": "# h\n- d\n> q\n12. n\n$$ m\n1) ok"}]));
        assert_eq!(
            tiptap_doc_to_md(&doc),
            "\\# h\n\\- d\n\\> q\n12\\. n\n\\$\\$ m\n1) ok\n"
        );
    }

    #[test]
    fn emoji_from_attrs() {
        let doc = para(json!([
            {"type": "emoji", "attrs": {"emoji": "<\u{1F600}>", "name": "smile"}},
            {"type": "emoji", "attrs": {"name": "nosuchcode_zz"}},
            {"type": "emoji", "attrs": {}},
        ]));
        assert_eq!(
            tiptap_doc_to_safe_html(&doc),
            "<p>&lt;\u{1F600}&gt;:nosuchcode_zz:</p>"
        );
        assert_eq!(tiptap_doc_to_md(&doc), "<\u{1F600}>:nosuchcode_zz:\n");
    }

    /// Known difference: the source's `selfCheckedMd` reparses this first pass,
    /// sees the text `$$` turn into math, and emits the second pass
    /// `"$$$ $$ $$$\\$\\$ after\n"`. Without the Markdown parser the port keeps
    /// the first pass.
    #[test]
    fn self_check_keeps_first_pass() {
        let doc = para(json!([
            {"type": "mathInline", "attrs": {"latex": "$$"}},
            {"type": "text", "text": "$$ after"},
        ]));
        assert_eq!(tiptap_doc_to_md(&doc), "$$$ $$ $$$$$ after\n");
    }

    /// Outputs of the TS source (`tiptapDocToSafeHtml` / `tiptapDocToMd`) at
    /// SHA `393795261322b916e588043cf94feca999175843`, with sanitize-html
    /// 2.17.7, captured by running the source modules under bun.
    const ORACLE_CASES: &str = r##"
[
{"doc":{"type":"doc"},"html":"","md":""},
{"doc":{"type":"doc","content":[]},"html":"","md":""},
{"doc":{"type":"doc","content":[{"type":"paragraph"}]},"html":"<p></p>","md":""},
{"doc":{"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"<script>alert(\"x\")</script> & 'q' &amp;"}]}]},"html":"<p>&lt;script&gt;alert(\"x\")&lt;/script&gt; &amp; 'q' &amp;amp;</p>","md":"\\<script>alert(\"x\")\\</script> & 'q' &amp;\n"},
{"doc":{"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"a","marks":[{"type":"link","attrs":{"href":"https://e.com/?a=1&b=\"2\"<>"}}]},{"type":"text","text":"b","marks":[{"type":"link","attrs":{"href":"javascript:alert(1)"}}]},{"type":"text","text":"c","marks":[{"type":"link","attrs":{"href":"JAVASCRIPT:alert(1)"}}]},{"type":"text","text":"d","marks":[{"type":"link","attrs":{"href":" java\tscript:alert(1)"}}]},{"type":"text","text":"e","marks":[{"type":"link","attrs":{"href":"//evil.com/x"}}]},{"type":"text","text":"f","marks":[{"type":"link","attrs":{"href":"mailto:a@b.c"}}]},{"type":"text","text":"g","marks":[{"type":"link","attrs":{"href":"/docs/1#x"}}]},{"type":"text","text":"h","marks":[{"type":"link","attrs":{"href":"\\\\evil.com"}}]},{"type":"text","text":"i","marks":[{"type":"link","attrs":{"href":"jav<!-- x -->ascript:alert(1)"}}]},{"type":"text","text":"j","marks":[{"type":"link","attrs":{"href":"HTTPS://X.COM"}}]},{"type":"text","text":"k","marks":[{"type":"link","attrs":{"href":"data:text/html,x"}}]},{"type":"text","text":"l","marks":[{"type":"link","attrs":{"href":""}}]},{"type":"text","text":"m","marks":[{"type":"link","attrs":{"href":"1http:x"}}]},{"type":"text","text":"n","marks":[{"type":"link","attrs":{"href":" / /x"}}]}]}]},"html":"<p><a href=\"https://e.com/?a=1&amp;b=&quot;2&quot;&lt;&gt;\">a</a><a>b</a><a>c</a><a>d</a><a>e</a><a href=\"mailto:a@b.c\">f</a><a href=\"/docs/1#x\">g</a><a>h</a><a>i</a><a href=\"HTTPS://X.COM\">j</a><a>k</a>l<a href=\"1http:x\">m</a><a>n</a></p>","md":"[a](https://e.com/?a=1&b=\"2\"<>)[b](javascript:alert(1))[c](JAVASCRIPT:alert(1))[d]( java\tscript:alert(1))[e](//evil.com/x)[f](mailto:a@b.c)[g](/docs/1#x)[h](\\\\evil.com)[i](jav<!-- x -->ascript:alert(1))[j](HTTPS://X.COM)[k](data:text/html,x)l[m](1http:x)[n]( / /x)\n"},
{"doc":{"type":"doc","content":[{"type":"heading","attrs":{"level":1},"content":[{"type":"text","text":"H1"}]},{"type":"heading","attrs":{"level":3},"content":[{"type":"text","text":"H3","marks":[{"type":"bold"}]}]},{"type":"heading","attrs":{"level":5},"content":[{"type":"text","text":"H5 <x>","marks":[{"type":"italic"}]}]},{"type":"heading","attrs":{"level":9},"content":[{"type":"text","text":"H9"}]},{"type":"heading","attrs":{"level":0},"content":[{"type":"text","text":"H0"}]},{"type":"heading","attrs":{"level":2.5},"content":[{"type":"text","text":"H2.5"}]},{"type":"heading","attrs":{"level":"2"},"content":[{"type":"text","text":"Hs"}]},{"type":"heading","content":[{"type":"text","text":"# not"}]}]},"html":"<h1>H1</h1><h3><strong>H3</strong></h3><em>H5 &lt;x&gt;</em>H9<h1>H0</h1>H2.5<h1>Hs</h1><h1># not</h1>","md":"# H1\n\n### **H3**\n\n##### *H5 \\<x>*\n\n###### H9\n\n# H0\n\n## H2.5\n\n# Hs\n\n# # not\n"},
{"doc":{"type":"doc","content":[{"type":"embed","attrs":{"ref":"abc<\"&>","entity":"document"}},{"type":"embed","attrs":{"ref":"https://x.y","entity":"url"}},{"type":"embed","attrs":{"ref":"T-1","entity":"task"}},{"type":"embed","attrs":{}}]},"html":"<div>abc&lt;\"&amp;&gt;</div><div>https://x.y</div><div>T-1</div><div></div>","md":"[[doc:abc<\"&>]]\n\nhttps://x.y\n\n[[task:T-1]]\n\n[[doc:]]\n"},
{"doc":{"type":"doc","content":[{"type":"math","attrs":{"latex":"a<b & $$x$$"}},{"type":"mermaid","attrs":{"source":"graph TD; A-->B\n```"}},{"type":"mermaid","content":[{"type":"text","text":"flow"}]},{"type":"paragraph","content":[{"type":"text","text":"x "},{"type":"mathInline","attrs":{"latex":"a\n  b"}},{"type":"mathInline","attrs":{"latex":"$c"}},{"type":"text","text":" y","marks":[{"type":"bold"}]}]},{"type":"paragraph","content":[{"type":"mathInline","attrs":{"latex":"q"},"marks":[{"type":"bold"},{"type":"link","attrs":{"href":"http://z"}}]}]},{"type":"paragraph","content":[{"type":"mathInline","attrs":{"latex":"   "}}]}]},"html":"<pre data-math>a&lt;b &amp; $$x$$</pre><pre data-mermaid>graph TD; A--&gt;B\n```</pre><pre data-mermaid></pre><p>x <span data-math-inline>a\n  b</span><span data-math-inline>$c</span><strong> y</strong></p><p><a href=\"http://z\"><strong><span data-math-inline>q</span></strong></a></p><p><span data-math-inline>   </span></p>","md":"$$$\na<b & $$x$$\n$$$\n\n````mermaid\ngraph TD; A-->B\n```\n````\n\n```mermaid\nflow\n```\n\nx $a b$<!---->$$ $c $$** y**\n\n[**$q$**](http://z)\n"},
{"doc":{"type":"doc","content":[{"type":"table","content":[{"type":"tableRow","content":[{"type":"tableHeader","content":[{"type":"paragraph","content":[{"type":"text","text":"A|B"}]}]},{"type":"tableHeader","content":[{"type":"paragraph","content":[{"type":"text","text":"C"}]},{"type":"paragraph","content":[{"type":"text","text":"D","marks":[{"type":"code"}]}]}]}]},{"type":"tableRow","content":[{"type":"tableCell","content":[{"type":"paragraph","content":[{"type":"text","text":"1"},{"type":"hardBreak"},{"type":"text","text":"2"}]}]},{"type":"tableCell","content":[]}]},{"type":"other"}]},{"type":"table","content":[]}]},"html":"<table><tr><th><p>A|B</p></th><th><p>C</p><p><code>D</code></p></th></tr><tr><td><p>1<br />2</p></td><td></td></tr></table><table></table>","md":"| A\\|B | C `D` |\n| --- | --- |\n| 1<br>2 |  |\n"},
{"doc":{"type":"doc","content":[{"type":"orderedList","content":[{"type":"listItem","content":[{"type":"paragraph","content":[{"type":"text","text":"one"}]},{"type":"bulletList","content":[{"type":"listItem","content":[{"type":"paragraph","content":[{"type":"text","text":"sub"}]},{"type":"paragraph","content":[{"type":"text","text":"sub2"}]}]}]}]},{"type":"listItem","content":[{"type":"paragraph","content":[{"type":"text","text":"two"}]}]},{"type":"listItem","content":[]}]},{"type":"taskList","content":[{"type":"taskItem","attrs":{"checked":true},"content":[{"type":"paragraph","content":[{"type":"text","text":"done"}]},{"type":"paragraph","content":[{"type":"text","text":"more"}]}]},{"type":"taskItem","attrs":{"checked":"true"},"content":[{"type":"paragraph","content":[{"type":"text","text":"todo"}]}]}]},{"type":"bulletList","content":[{"type":"listItem","content":[{"type":"paragraph","content":[{"type":"text","text":"- dash"}]}]}]}]},"html":"<ol><li><p>one</p><ul><li><p>sub</p><p>sub2</p></li></ul></li><li><p>two</p></li><li></li></ol><p>done</p><p>more</p><p>todo</p><ul><li><p>- dash</p></li></ul>","md":"1. one\n\n   - sub\n\n     sub2\n2. two\n3. \n\n- [x] done\n\n  more\n- [ ] todo\n\n- \\- dash\n"},
{"doc":{"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"a`b","marks":[{"type":"code"}]}]},{"type":"paragraph","content":[{"type":"text","text":"`x``","marks":[{"type":"code"}]}]},{"type":"paragraph","content":[{"type":"text","text":" pad ","marks":[{"type":"code"}]}]},{"type":"paragraph","content":[{"type":"text","text":"  ","marks":[{"type":"code"}]}]},{"type":"paragraph","content":[{"type":"text","text":" a\nb ","marks":[{"type":"code"}]}]},{"type":"paragraph","content":[{"type":"text","text":"x","marks":[{"type":"code"},{"type":"bold"},{"type":"italic"},{"type":"strike"},{"type":"highlight"},{"type":"link","attrs":{"href":"https://l"}},{"type":"underline"}]}]}]},"html":"<p><code>a`b</code></p><p><code>`x``</code></p><p><code> pad </code></p><p><code>  </code></p><p><code> a\nb </code></p><p><a href=\"https://l\"><s><em><strong><code>x</code></strong></em></s></a></p>","md":"``a`b``\n\n``` `x`` ```\n\n`  pad  `\n\n`  `\n\n` a\nb `\n\n[==~~***`x`***~~==](https://l)\n"},
{"doc":{"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"see !"},{"type":"text","text":"[img](x) and \\< and \\\\x and <b>"}]},{"type":"paragraph","content":[{"type":"text","text":"# h\n- d\n> q\n12. n\n$$ m\n`c\n1) ok\n$x"}]},{"type":"paragraph","content":[{"type":"text","text":"`code`\nx","marks":[{"type":"code"}]}]},{"type":"paragraph","content":[{"type":"text","text":"bold ","marks":[{"type":"bold"}]},{"type":"text","text":"under","marks":[{"type":"bold"},{"type":"underline"}]},{"type":"mathInline","attrs":{"latex":"x"},"marks":[{"type":"bold"}]}]},{"type":"paragraph","content":[{"type":"mention","attrs":{"label":"kim <a>"}},{"type":"mention","attrs":{}},{"type":"text","text":"t"}]}]},"html":"<p>see ![img](x) and \\&lt; and \\\\x and &lt;b&gt;</p><p># h\n- d\n&gt; q\n12. n\n$$ m\n`c\n1) ok\n$x</p><p><code>`code`\nx</code></p><p><strong>bold </strong><strong>under</strong><strong><span data-math-inline>x</span></strong></p><p>@kim &lt;a&gt;@t</p>","md":"see !\\[img](x) and \\\\\\< and \\\\x and \\<b>\n\n\\# h\n\\- d\n\\> q\n12\\. n\n\\$\\$ m\n\\`c\n1) ok\n$x\n\n`` `code`\nx ``\n\n**bold under$x$**\n\n@kim <a>t\n"},
{"doc":{"type":"doc","content":[{"type":"paragraph","content":[{"type":"emoji","attrs":{"emoji":"<😀>","name":"smile"}},{"type":"emoji","attrs":{"name":"smile"}},{"type":"emoji","attrs":{"name":"nosuchcode_zz"}},{"type":"emoji","attrs":{}}]},{"type":"attachment","attrs":{"id":"u1","name":"a<b>.png","image":true}},{"type":"attachment","attrs":{"id":"u2","name":"f.pdf"}},{"type":"codeBlock","attrs":{"language":"rust"},"content":[{"type":"text","text":"fn x() { \"<>\" }\n```"}]},{"type":"codeBlock","content":[{"type":"text","text":"x","marks":[{"type":"code"},{"type":"link","attrs":{"href":"https://a"}}]}]},{"type":"horizontalRule"},{"type":"blockquote","content":[{"type":"paragraph","content":[{"type":"text","text":"q1"}]},{"type":"paragraph","content":[{"type":"text","text":"q2"}]}]},{"type":"blockquote"},{"type":"callout","attrs":{"kind":"warning"},"content":[{"type":"paragraph","content":[{"type":"text","text":"careful"}]}]},{"type":"callout"},{"type":"details","content":[{"type":"detailsSummary","content":[{"type":"text","text":"sum <b> \\ ![x"}]},{"type":"detailsContent","content":[{"type":"paragraph","content":[{"type":"text","text":"body"}]}]}]}]},"html":"<p>&lt;😀&gt;😄:nosuchcode_zz:</p><p>a&lt;b&gt;.png</p><p>f.pdf</p><pre><code>fn x() { \"&lt;&gt;\" }\n```</code></pre><pre><code><a href=\"https://a\"><code>x</code></a></code></pre><hr /><blockquote><p>q1</p><p>q2</p></blockquote><blockquote></blockquote><p>careful</p><p>body</p>","md":"<😀>😄:nosuchcode_zz:\n\n![a<b>.png](attachment:u1)\n\n[f.pdf](attachment:u2)\n\n````rust\nfn x() { \"<>\" }\n```\n````\n\n```\nx\n```\n\n---\n\n> q1\n> \n> q2\n\n> \n\n> [!WARNING]\n> careful\n\n> [!NOTE]\n> \n\n<details><summary>sum <b> \\ !\\[x</summary>\n\nbody\n\n</details>\n"},
{"doc":{"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"nest","marks":[{"type":"link","attrs":{"href":"https://in"}},{"type":"link","attrs":{"href":"https://out"}}]}]},{"type":"paragraph","content":[{"type":"text","text":"nest2","marks":[{"type":"link","attrs":{"href":"https://1"}},{"type":"link","attrs":{"href":"javascript:x"}},{"type":"bold"},{"type":"link","attrs":{"href":"https://3"}}]}]},{"type":"paragraph","content":[{"type":"text","text":"n3","marks":[{"type":"link","attrs":{"href":"https://1"}},{"type":"highlight"},{"type":"link","attrs":{"href":"https://2"}}]}]},{"type":"unknownBlock","content":[{"type":"paragraph","content":[{"type":"text","text":"inner"}]}]},{"type":"text","text":"bare text at block level"},{"type":"paragraph","content":[{"type":"wrapper","content":[{"type":"text","text":"w1"},{"type":"hardBreak"}]},{"type":"text","text":"w2"}]},{"type":"paragraph","content":[{"type":"mention","text":"mt","attrs":{"label":"L"}},{"type":"text","text":"tt"}]},[1,2],{"type":"paragraph","content":[{"type":"text","text":"ctl\u0001\r\nx "}]}]},"html":"<p><a href=\"https://out\"></a><a href=\"https://in\">nest</a></p><p><a href=\"https://3\"><strong><a></a><a href=\"https://1\">nest2</a></strong></a></p><p><a href=\"https://2\"></a><a href=\"https://1\">n3</a></p><p>inner</p><p>w1<br />w2</p><p>@Ltt</p><p>ctl\u0001\r\nx </p>","md":"[nest](https://in)\n\n[**nest2**](https://1)\n\n[==n3==](https://1)\n\ninner\n\nw1<br>w2\n\n@L\n\nctl\u0001\r\nx \n"}
]
"##;

    #[test]
    fn matches_ts_oracle_outputs() {
        let cases: Vec<Value> = serde_json::from_str(ORACLE_CASES).expect("oracle fixture");
        assert!(!cases.is_empty());
        for (i, case) in cases.iter().enumerate() {
            let doc = &case["doc"];
            assert!(is_tiptap_doc(doc), "case {i}");
            assert_eq!(
                tiptap_doc_to_safe_html(doc),
                case["html"].as_str().unwrap(),
                "html case {i}"
            );
            assert_eq!(
                tiptap_doc_to_md(doc),
                case["md"].as_str().unwrap(),
                "md case {i}"
            );
        }
    }
}
