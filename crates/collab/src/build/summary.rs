//! Zero-allocation canonical stored-form [`BlockCache`] prefill from a [`fvoci_model::Body`].
//!
//! Validates every node, attribute, mark, table grid, and limit against the exact
//! allowlist rules of [`fvoci_model::Body::parse`] while computing each top-level block's
//! canonical [`BlockSummary`] and deduplicated document-order `file_ids` in one `O(N)`
//! pass without projecting the Yjs document back to `serde_json::Value`. Any body outside
//! the allowlist (for example a malformed `Body::from_stored`) returns `None` so callers
//! fall back cleanly to `BlockCache::build` / `build_or_repair_cache`.

use std::collections::HashSet;

use fvoci_model::{BODY_MAX_DEPTH, TABLE_MAX_COLUMNS, TABLE_MAX_ROWS, TableGrid, is_id};
use serde_json::{Map, Value};
use yrs::{ID, Number};

use crate::block_cache::{BlockCache, BlockEntry, BlockSummary};
use crate::schema::{self, AtFallback, Attribute, Fallback};

const EMPTY_PARAGRAPH_BYTES: usize = r#"{"type":"paragraph"}"#.len();
const ELEMENT_SHELL_BYTES: usize = r#"{"type":""}"#.len();
const ATTRS_FIELD_OVERHEAD: usize = r#","attrs":{}"#.len();
const CONTENT_FIELD_OVERHEAD: usize = r#","content":[]"#.len();
const TEXT_SHELL_BYTES: usize = r#"{"text":"","type":"text"}"#.len();
const MARKS_FIELD_OVERHEAD: usize = r#""marks":[],"#.len();
const PLAIN_MARK_SHELL_BYTES: usize = r#"{"type":""}"#.len();
const LINK_MARK_SHELL_BYTES: usize = r#"{"attrs":{"href":},"type":""}"#.len();

const NODE_MEMBERS: [&str; 5] = ["type", "attrs", "content", "marks", "text"];
const MARK_MEMBERS: [&str; 2] = ["type", "attrs"];
const LATEX_MAX: usize = 4000;
const HREF_MAX: usize = 2048;
const LINK_SCHEMES: [&str; 3] = ["http", "https", "mailto"];

const BLOCKS: &[&str] = &[
    "paragraph",
    "heading",
    "bulletList",
    "orderedList",
    "taskList",
    "blockquote",
    "codeBlock",
    "horizontalRule",
    "image",
    "file",
    "blockMath",
];
const DOCUMENT_BLOCKS: &[&str] = &[
    "paragraph",
    "heading",
    "bulletList",
    "orderedList",
    "taskList",
    "blockquote",
    "codeBlock",
    "horizontalRule",
    "image",
    "file",
    "blockMath",
    "table",
];
const INLINE: &[&str] = &["text", "hardBreak", "mention", "reference", "inlineMath"];
const LEAF: &[&str] = &[];

#[derive(Clone, Copy)]
enum AttrRule {
    Text(usize),
    NonEmptyText(usize),
    Integer(i32, i32),
    Boolean,
    Language,
    Id,
}

#[derive(Clone, Copy)]
struct AttrSpec {
    name: &'static str,
    rule: AttrRule,
    required: bool,
}

const fn req(name: &'static str, rule: AttrRule) -> AttrSpec {
    AttrSpec {
        name,
        rule,
        required: true,
    }
}

const fn opt(name: &'static str, rule: AttrRule) -> AttrSpec {
    AttrSpec {
        name,
        rule,
        required: false,
    }
}

const HEADING_ATTRS: &[AttrSpec] = &[req("level", AttrRule::Integer(1, 3))];
const ORDERED_LIST_ATTRS: &[AttrSpec] = &[opt("start", AttrRule::Integer(0, 1_000_000_000))];
const TASK_ITEM_ATTRS: &[AttrSpec] = &[req("checked", AttrRule::Boolean)];
const CODE_BLOCK_ATTRS: &[AttrSpec] = &[opt("language", AttrRule::Language)];
const IMAGE_ATTRS: &[AttrSpec] = &[
    req("fileId", AttrRule::NonEmptyText(100)),
    opt("alt", AttrRule::Text(1000)),
];
const FILE_ATTRS: &[AttrSpec] = &[req("fileId", AttrRule::NonEmptyText(100))];
const CELL_ATTRS: &[AttrSpec] = &[
    opt("colspan", AttrRule::Integer(1, 20)),
    opt("rowspan", AttrRule::Integer(1, 200)),
];
const _: () = assert!(
    TABLE_MAX_COLUMNS == 20 && TABLE_MAX_ROWS == 200,
    "cell span rules must match fvoci_model table limits"
);
const MENTION_ATTRS: &[AttrSpec] = &[req("userId", AttrRule::Id)];
const REFERENCE_ATTRS: &[AttrSpec] = &[req("id", AttrRule::Id)];
const MATH_ATTRS: &[AttrSpec] = &[req("latex", AttrRule::NonEmptyText(LATEX_MAX))];

#[derive(Clone, Copy)]
struct NodeSpec {
    attrs: &'static [AttrSpec],
    children: &'static [&'static str],
    min_children: usize,
}

const fn node_spec_def(
    attrs: &'static [AttrSpec],
    children: &'static [&'static str],
    min_children: usize,
) -> NodeSpec {
    NodeSpec {
        attrs,
        children,
        min_children,
    }
}

fn node_spec(kind: &str) -> Option<NodeSpec> {
    Some(match kind {
        "paragraph" => node_spec_def(&[], INLINE, 0),
        "heading" => node_spec_def(HEADING_ATTRS, INLINE, 0),
        "bulletList" => node_spec_def(&[], &["listItem"], 1),
        "orderedList" => node_spec_def(ORDERED_LIST_ATTRS, &["listItem"], 1),
        "listItem" | "blockquote" => node_spec_def(&[], BLOCKS, 1),
        "table" => node_spec_def(&[], &["tableRow"], 1),
        "tableRow" => node_spec_def(&[], &["tableHeader", "tableCell"], 0),
        "tableHeader" | "tableCell" => node_spec_def(CELL_ATTRS, BLOCKS, 1),
        "taskList" => node_spec_def(&[], &["taskItem"], 1),
        "taskItem" => node_spec_def(TASK_ITEM_ATTRS, BLOCKS, 1),
        "codeBlock" => node_spec_def(CODE_BLOCK_ATTRS, &["text"], 0),
        "horizontalRule" | "hardBreak" => node_spec_def(&[], LEAF, 0),
        "image" => node_spec_def(IMAGE_ATTRS, LEAF, 0),
        "file" => node_spec_def(FILE_ATTRS, LEAF, 0),
        "mention" => node_spec_def(MENTION_ATTRS, LEAF, 0),
        "reference" => node_spec_def(REFERENCE_ATTRS, LEAF, 0),
        "inlineMath" | "blockMath" => node_spec_def(MATH_ATTRS, LEAF, 0),
        _ => return None,
    })
}

/// Builds a prefilled [`BlockCache`] for `document` paired with its newly emitted root
/// element [`ID`]s (in forward document order).
///
/// Returns `None` if `document` fails any allowlist rule or exceeds whole-document limits.
pub(super) fn prefill_block_cache(document: &Value, ids: &[ID]) -> Option<BlockCache> {
    let blocks = validate_root_document(document)?;
    if blocks.len() != ids.len() {
        return None;
    }
    if blocks.is_empty() {
        return Some(BlockCache::new());
    }
    let mut cache = BlockCache::new();
    for (node, &id) in blocks.iter().zip(ids) {
        let entry = canonical_block_entry(node)?;
        cache.upsert(id, entry);
    }
    cache.check_whole_limits().ok()?;
    Some(cache)
}

/// Checks root `"doc"` node framing and returns its top-level content slice.
fn validate_root_document(document: &Value) -> Option<&[Value]> {
    let members = document.as_object()?;
    check_node_members(members)?;
    if members.get("type").and_then(Value::as_str) != Some("doc")
        || members.contains_key("text")
        || members.contains_key("marks")
    {
        return None;
    }
    if matches!(
        check_attrs_match(members.get("attrs"), &[]),
        AttrsCheck::Invalid
    ) {
        return None;
    }
    match members.get("content") {
        None => Some(&[]),
        Some(Value::Array(blocks)) => Some(blocks.as_slice()),
        Some(_) => None,
    }
}

/// Computes the exact canonical [`BlockEntry`] of one validated top-level block at depth 2.
fn canonical_block_entry(node: &Value) -> Option<BlockEntry> {
    let kind = node.get("type")?.as_str()?;
    let mut files = FileCollector::default();
    let (nodes, bytes) = count_element(node, 2, DOCUMENT_BLOCKS, &mut files)?;
    let is_empty_paragraph = kind == "paragraph" && nodes == 1 && bytes == EMPTY_PARAGRAPH_BYTES;
    let summary = BlockSummary {
        nodes: u32::try_from(nodes).ok()?,
        json_bytes: u32::try_from(bytes).ok()?,
        is_empty_paragraph,
        has_empty_row_text: false,
    };
    Some(BlockEntry {
        summary,
        file_ids: files.into_boxed_slice(),
    })
}

/// Collects deduplicated document-order `fileId`s for one top-level block with zero
/// allocation when a block contains no `image` or `file` nodes.
#[derive(Default)]
struct FileCollector<'a> {
    ids: Vec<String>,
    seen: HashSet<&'a str>,
}

impl<'a> FileCollector<'a> {
    fn push(&mut self, id: &'a str) {
        if self.seen.insert(id) {
            self.ids.push(id.to_owned());
        }
    }

    fn into_boxed_slice(self) -> Box<[String]> {
        self.ids.into_boxed_slice()
    }
}

/// Validates and counts canonical stored-form `(nodes, compact_json_bytes)` for an element.
fn count_element<'a>(
    node: &'a Value,
    depth: usize,
    allowed: &[&str],
    files: &mut FileCollector<'a>,
) -> Option<(usize, usize)> {
    if depth > BODY_MAX_DEPTH {
        return None;
    }
    let members = node.as_object()?;
    check_node_members(members)?;
    let kind = members.get("type")?.as_str()?;
    if !allowed.contains(&kind) || members.contains_key("text") || members.contains_key("marks") {
        return None;
    }
    let spec = node_spec(kind)?;
    let schema_attrs = schema::node_attributes(kind)?;
    let given_attrs = match check_attrs_match(members.get("attrs"), spec.attrs) {
        AttrsCheck::Invalid => return None,
        AttrsCheck::Absent => None,
        AttrsCheck::Present(map) => Some(map),
    };
    let (attrs_count, attrs_bytes) = count_element_attrs(kind, schema_attrs, given_attrs, files)?;
    let children = match members.get("content") {
        None => &[][..],
        Some(Value::Array(arr)) => arr.as_slice(),
        Some(_) => return None,
    };
    if children.len() < spec.min_children || (!children.is_empty() && spec.children.is_empty()) {
        return None;
    }
    if matches!(kind, "listItem" | "taskItem")
        && children
            .first()
            .and_then(|child| child.get("type"))
            .and_then(Value::as_str)
            != Some("paragraph")
    {
        return None;
    }
    if kind == "codeBlock" && children.iter().any(|child| child.get("marks").is_some()) {
        return None;
    }
    let (child_count, child_nodes, child_bytes) =
        count_content(children, depth + 1, spec.children, files)?;
    if kind == "table" {
        let grid = TableGrid::of(node).ok()?;
        if grid.width() == 0 || !grid.is_filled() {
            return None;
        }
    }
    let mut bytes = ELEMENT_SHELL_BYTES + kind.len();
    if attrs_count > 0 {
        bytes += ATTRS_FIELD_OVERHEAD + attrs_bytes;
    }
    if child_count > 0 {
        bytes += CONTENT_FIELD_OVERHEAD + child_bytes + (child_count - 1);
    }
    Some((1 + child_nodes, bytes))
}

fn check_node_members(members: &Map<String, Value>) -> Option<()> {
    for (name, member) in members {
        if !NODE_MEMBERS.contains(&name.as_str())
            || member.is_null()
            || (name != "content" && holds_nul(member))
        {
            return None;
        }
    }
    Some(())
}

enum AttrsCheck<'a> {
    Absent,
    Present(&'a Map<String, Value>),
    Invalid,
}

fn check_attrs_match<'a>(attrs: Option<&'a Value>, specs: &[AttrSpec]) -> AttrsCheck<'a> {
    let given = match attrs {
        None => None,
        Some(Value::Object(map)) => Some(map),
        Some(_) => return AttrsCheck::Invalid,
    };
    if let Some(map) = given {
        for (name, val) in map {
            let Some(spec) = specs.iter().find(|spec| spec.name == name) else {
                return AttrsCheck::Invalid;
            };
            if !attr_rule_allows(spec.rule, val) {
                return AttrsCheck::Invalid;
            }
        }
    }
    for spec in specs {
        if spec.required && !given.is_some_and(|map| map.contains_key(spec.name)) {
            return AttrsCheck::Invalid;
        }
    }
    match given {
        None => AttrsCheck::Absent,
        Some(map) => AttrsCheck::Present(map),
    }
}

fn count_element_attrs<'a>(
    kind: &str,
    schema_attrs: &[Attribute],
    given: Option<&'a Map<String, Value>>,
    files: &mut FileCollector<'a>,
) -> Option<(usize, usize)> {
    let mut count = 0_usize;
    let mut bytes = 0_usize;
    for &attr in schema_attrs {
        let val = given.and_then(|attrs| attrs.get(attr.name));
        match canonical_attr_bytes(kind, attr, val, files) {
            AttrBytes::Unsupported => return None,
            AttrBytes::Omitted => {}
            AttrBytes::Bytes(len) => {
                if count > 0 {
                    bytes += 1;
                }
                bytes += attr.name.len() + 3 + len;
                count += 1;
            }
        }
    }
    Some((count, bytes))
}

enum AttrBytes {
    Omitted,
    Bytes(usize),
    Unsupported,
}

fn canonical_attr_bytes<'a>(
    kind: &str,
    attr: Attribute,
    given: Option<&'a Value>,
    files: &mut FileCollector<'a>,
) -> AttrBytes {
    match given {
        None => match attr.fallback {
            Fallback::None => AttrBytes::Omitted,
            Fallback::Integer(number) => {
                if attr.at_fallback == AtFallback::LeftOut {
                    AttrBytes::Omitted
                } else {
                    AttrBytes::Bytes(int_json_len(number))
                }
            }
            Fallback::Boolean(truth) => {
                if attr.at_fallback == AtFallback::LeftOut {
                    AttrBytes::Omitted
                } else {
                    AttrBytes::Bytes(if truth { 4 } else { 5 })
                }
            }
        },
        Some(Value::Bool(truth)) => {
            if attr.at_fallback == AtFallback::LeftOut && attr.fallback == Fallback::Boolean(*truth)
            {
                return AttrBytes::Omitted;
            }
            AttrBytes::Bytes(if *truth { 4 } else { 5 })
        }
        Some(Value::Number(num)) => {
            let Some(whole) = num
                .as_i64()
                .or_else(|| match Number::try_i64(num.as_f64()?) {
                    Number::Int(n) => Some(n),
                    Number::Float(_) => None,
                })
            else {
                return AttrBytes::Unsupported;
            };
            if attr.at_fallback == AtFallback::LeftOut && attr.fallback == Fallback::Integer(whole)
            {
                return AttrBytes::Omitted;
            }
            AttrBytes::Bytes(int_json_len(whole))
        }
        Some(Value::String(text)) => {
            if matches!(kind, "image" | "file") && attr.name == "fileId" {
                files.push(text.as_str());
            }
            AttrBytes::Bytes(json_str_len(text))
        }
        Some(_) => AttrBytes::Unsupported,
    }
}

/// Validates and counts canonical `(direct_children, total_nodes, sum_child_bytes)`.
fn count_content<'a>(
    nodes: &'a [Value],
    depth: usize,
    allowed: &[&str],
    files: &mut FileCollector<'a>,
) -> Option<(usize, usize, usize)> {
    let text_allowed = allowed.contains(&"text");
    let mut direct_children = 0_usize;
    let mut total_nodes = 0_usize;
    let mut sum_bytes = 0_usize;
    let mut i = 0_usize;
    while i < nodes.len() {
        if text_allowed && nodes[i].get("type").and_then(Value::as_str) == Some("text") {
            let start = i;
            while i < nodes.len() && nodes[i].get("type").and_then(Value::as_str) == Some("text") {
                i += 1;
            }
            let (runs, run_bytes) = count_text_stretch(&nodes[start..i], depth)?;
            direct_children += runs;
            total_nodes += runs;
            sum_bytes += run_bytes;
        } else {
            let (n, b) = count_element(&nodes[i], depth, allowed, files)?;
            direct_children += 1;
            total_nodes += n;
            sum_bytes += b;
            i += 1;
        }
    }
    Some((direct_children, total_nodes, sum_bytes))
}

#[derive(Clone, PartialEq, Eq)]
struct CanonicalMark<'a> {
    rank: usize,
    name: &'a str,
    href: Option<&'a str>,
}

/// Validates and counts canonical `(coalesced_runs, sum_run_bytes)` for a text stretch.
fn count_text_stretch(nodes: &[Value], depth: usize) -> Option<(usize, usize)> {
    if depth > BODY_MAX_DEPTH {
        return None;
    }
    let mut prev_marks: Vec<CanonicalMark<'_>> = Vec::new();
    let mut curr_marks: Vec<CanonicalMark<'_>> = Vec::new();
    let mut has_prev = false;
    let mut runs = 0_usize;
    let mut total_bytes = 0_usize;

    for node in nodes {
        let letters = validate_text_node(node, &mut curr_marks)?;
        let inner_len = json_str_inner_len(letters);
        if has_prev && curr_marks == prev_marks {
            total_bytes += inner_len;
        } else {
            runs += 1;
            let mut run_bytes = TEXT_SHELL_BYTES + inner_len;
            if !curr_marks.is_empty() {
                run_bytes += MARKS_FIELD_OVERHEAD + (curr_marks.len() - 1);
                for mark in &curr_marks {
                    run_bytes += canonical_mark_bytes(mark);
                }
            }
            total_bytes += run_bytes;
            let reuse = std::mem::replace(&mut prev_marks, std::mem::take(&mut curr_marks));
            curr_marks = reuse;
            has_prev = true;
        }
    }
    Some((runs, total_bytes))
}

fn validate_text_node<'a>(
    node: &'a Value,
    out_marks: &mut Vec<CanonicalMark<'a>>,
) -> Option<&'a str> {
    let members = node.as_object()?;
    check_node_members(members)?;
    if matches!(
        check_attrs_match(members.get("attrs"), &[]),
        AttrsCheck::Invalid
    ) {
        return None;
    }
    if let Some(content) = members.get("content") {
        let arr = content.as_array()?;
        if !arr.is_empty() {
            return None;
        }
    }
    let letters = members.get("text")?.as_str()?;
    if letters.is_empty() {
        return None;
    }
    extract_canonical_marks(members.get("marks"), out_marks)?;
    Some(letters)
}

fn extract_canonical_marks<'a>(
    given: Option<&'a Value>,
    out: &mut Vec<CanonicalMark<'a>>,
) -> Option<()> {
    out.clear();
    let Some(marks) = given else {
        return Some(());
    };
    let marks = marks.as_array()?;
    for mark in marks {
        let obj = mark.as_object()?;
        if obj
            .iter()
            .any(|(k, v)| !MARK_MEMBERS.contains(&k.as_str()) || v.is_null())
        {
            return None;
        }
        let name = obj.get("type")?.as_str()?;
        let rank = schema::mark_rank(name)?;
        if out.iter().any(|existing| existing.name == name) {
            return None;
        }
        let attrs = match obj.get("attrs") {
            None => None,
            Some(Value::Object(map)) => Some(map),
            Some(_) => return None,
        };
        let href = match schema::mark_attributes(name) {
            [] => {
                if attrs.is_some_and(|map| !map.is_empty()) {
                    return None;
                }
                None
            }
            ["href"] => {
                let map = attrs?;
                if map.len() != 1 {
                    return None;
                }
                let href = map.get("href")?.as_str()?;
                if !is_allowed_href(href) {
                    return None;
                }
                Some(href)
            }
            _ => return None,
        };
        out.push(CanonicalMark { rank, name, href });
    }
    out.sort_unstable_by(|a, b| (a.rank, a.name).cmp(&(b.rank, b.name)));
    Some(())
}

fn canonical_mark_bytes(mark: &CanonicalMark<'_>) -> usize {
    match mark.href {
        None => PLAIN_MARK_SHELL_BYTES + mark.name.len(),
        Some(href) => LINK_MARK_SHELL_BYTES + json_str_len(href) + mark.name.len(),
    }
}

fn attr_rule_allows(rule: AttrRule, value: &Value) -> bool {
    match rule {
        AttrRule::Text(max) => value.as_str().is_some_and(|text| utf16_len(text) <= max),
        AttrRule::NonEmptyText(max) => value
            .as_str()
            .is_some_and(|text| (1..=max).contains(&utf16_len(text))),
        AttrRule::Integer(low, high) => value.as_f64().is_some_and(|number| {
            number.fract() == 0.0 && (f64::from(low)..=f64::from(high)).contains(&number)
        }),
        AttrRule::Boolean => value.is_boolean(),
        AttrRule::Language => value.as_str().is_some_and(is_language),
        AttrRule::Id => value.as_str().is_some_and(is_id),
    }
}

fn utf16_len(text: &str) -> usize {
    if text.is_ascii() {
        return text.len();
    }
    text.encode_utf16().count()
}

fn is_language(text: &str) -> bool {
    (1..=40).contains(&text.len())
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '#' | '.' | '_' | '-'))
}

fn is_allowed_href(href: &str) -> bool {
    let length = utf16_len(href);
    if length == 0 || length > HREF_MAX {
        return false;
    }
    let compact: String = href
        .chars()
        .filter(|c| !matches!(c, '\u{0}'..=' ' | '\u{7f}'))
        .collect();
    if compact.is_empty() {
        return false;
    }
    match href_scheme(&compact) {
        Some(scheme) => LINK_SCHEMES
            .iter()
            .any(|allowed| scheme.eq_ignore_ascii_case(allowed)),
        None => !compact.starts_with("//") && !compact.contains('\\'),
    }
}

fn href_scheme(link: &str) -> Option<&str> {
    let (scheme, _) = link.split_once(':')?;
    let mut chars = scheme.chars();
    let first = chars.next()?;
    (first.is_ascii_alphabetic()
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-')))
    .then_some(scheme)
}

fn holds_nul(value: &Value) -> bool {
    match value {
        Value::String(text) => text.contains('\0'),
        Value::Array(items) => items.iter().any(holds_nul),
        Value::Object(members) => members
            .iter()
            .any(|(name, member)| name.contains('\0') || holds_nul(member)),
        Value::Null | Value::Bool(_) | Value::Number(_) => false,
    }
}

fn int_json_len(value: i64) -> usize {
    let mut digits = 1_usize;
    let mut n = value.unsigned_abs();
    while n >= 10 {
        n /= 10;
        digits += 1;
    }
    digits + usize::from(value < 0)
}

fn json_str_len(s: &str) -> usize {
    json_str_inner_len(s) + 2
}

fn json_str_inner_len(s: &str) -> usize {
    if s.bytes().all(|b| b >= 0x20 && b != b'"' && b != b'\\') {
        return s.len();
    }
    let mut count = 0_usize;
    for b in s.bytes() {
        count += match b {
            b'"' | b'\\' | 0x08 | 0x0c | b'\n' | b'\r' | b'\t' => 2,
            0x00..=0x1f => 6,
            _ => 1,
        };
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_byte_constants_match_serde_json() {
        use serde_json::json;

        assert_eq!(
            EMPTY_PARAGRAPH_BYTES,
            serde_json::to_vec(&json!({"type": "paragraph"}))
                .expect("json")
                .len(),
            "empty paragraph bytes"
        );
        assert_eq!(
            ELEMENT_SHELL_BYTES + "heading".len(),
            serde_json::to_vec(&json!({"type": "heading"}))
                .expect("json")
                .len(),
            "element shell bytes"
        );
        assert_eq!(
            ELEMENT_SHELL_BYTES + "heading".len() + ATTRS_FIELD_OVERHEAD + r#""level":2"#.len(),
            serde_json::to_vec(&json!({"type": "heading", "attrs": {"level": 2}}))
                .expect("json")
                .len(),
            "attrs field overhead"
        );
        assert_eq!(
            ELEMENT_SHELL_BYTES + "paragraph".len() + CONTENT_FIELD_OVERHEAD,
            serde_json::to_vec(&json!({"type": "paragraph", "content": []}))
                .expect("json")
                .len(),
            "content field overhead"
        );
        assert_eq!(
            TEXT_SHELL_BYTES + "hello".len(),
            serde_json::to_vec(&json!({"type": "text", "text": "hello"}))
                .expect("json")
                .len(),
            "text shell bytes"
        );
        let plain_mark = serde_json::to_vec(&json!({"type": "bold"})).expect("json");
        assert_eq!(
            PLAIN_MARK_SHELL_BYTES + "bold".len(),
            plain_mark.len(),
            "plain mark shell bytes"
        );
        let href = "https://example.com/a?b=1&c=\"d\"";
        let link_mark =
            serde_json::to_vec(&json!({"type": "link", "attrs": {"href": href}})).expect("json");
        assert_eq!(
            LINK_MARK_SHELL_BYTES + json_str_len(href) + "link".len(),
            link_mark.len(),
            "link mark shell bytes"
        );
        let marked_text = serde_json::to_vec(&json!({
            "type": "text",
            "text": "hi",
            "marks": [{"type": "link", "attrs": {"href": href}}, {"type": "bold"}]
        }))
        .expect("json");
        assert_eq!(
            TEXT_SHELL_BYTES
                + "hi".len()
                + MARKS_FIELD_OVERHEAD
                + 1
                + link_mark.len()
                + plain_mark.len(),
            marked_text.len(),
            "marked text byte sum"
        );
    }

    #[test]
    fn json_str_len_matches_serde_json_for_all_ascii_and_unicode() {
        for byte in 0_u8..=0x7f {
            let s = String::from(char::from(byte));
            let expected = serde_json::to_string(&s).expect("json").len();
            assert_eq!(json_str_len(&s), expected, "byte 0x{byte:02x}");
        }
        for sample in [
            "",
            "plain",
            "한글😀",
            "line\n\t\"\\break\x00\x1f",
            "\u{2028}\u{2029}",
        ] {
            let expected = serde_json::to_string(sample).expect("json").len();
            assert_eq!(json_str_len(sample), expected, "sample {sample:?}");
        }
    }

    #[test]
    fn int_json_len_matches_display_len() {
        for n in [
            i64::MIN,
            -1_000_000,
            -10,
            -1,
            0,
            1,
            9,
            10,
            99,
            100,
            i64::MAX,
        ] {
            assert_eq!(int_json_len(n), n.to_string().len(), "n={n}");
        }
    }

    #[test]
    fn prefill_rejects_every_rejected_body_vector() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../contracts/body-vectors.json");
        let text = std::fs::read_to_string(&path).expect("read body vectors");
        let file: Value = serde_json::from_str(&text).expect("parse body vectors");
        let cases = file["cases"].as_array().expect("cases array");
        let mut rejected = 0_usize;
        for case in cases {
            if case["expect"] != "reject" {
                continue;
            }
            rejected += 1;
            let name = case["name"].as_str().expect("case name");
            let doc = &case["input"];
            let block_count = doc
                .get("content")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            let fake_ids: Vec<ID> = (0..block_count)
                .map(|i| ID::new(yrs::ClientID::new(1), u32::try_from(i).expect("idx")))
                .collect();
            assert!(
                prefill_block_cache(doc, &fake_ids).is_none(),
                "reject vector {name:?} must not prefill BlockCache"
            );
        }
        assert!(rejected > 0, "checked rejected body vectors");
    }
}
