//! Single-pass, zero-clone node-level allowlist judgment for [`super::Survey`].
//!
//! Instead of cloning every surviving subtree and re-running `Body::parse` (and JSON
//! serialization) at every ancestor level, each non-table node validates only its own
//! shell (type, place, and attributes via [`Body::parse`]) plus its direct surviving
//! children's summaries in exact [`fvoci_model::Body::parse`] precedence order. Tables
//! continue to be surveyed and mended whole via [`fvoci_model::TableGrid`].

use std::collections::HashSet;

use fvoci_model::{
    BODY_MAX_BYTES, BODY_MAX_DEPTH, BODY_MAX_NODES, Body, BodyError, TABLE_MAX_COLUMNS,
};
use serde_json::{Value, json};

use crate::project::json_size_of;
use crate::schema::Place;

/// Compact JSON byte length of `,"content":[]` added when a non-empty `content` array is
/// inserted into an element JSON object that already has `"type"`.
pub(super) const CONTENT_FIELD_OVERHEAD: usize = r#","content":[]"#.len();

/// Compact JSON wrapper byte length for `Place::Block` (`{"content":[],"type":"doc"}`).
pub(super) const BLOCK_WRAPPER_BYTES: usize = r#"{"content":[],"type":"doc"}"#.len();

const LINE_WRAPPER_BYTES: usize =
    r#"{"content":[{"content":[],"type":"paragraph"}],"type":"doc"}"#.len();
const LIST_ENTRY_WRAPPER_BYTES: usize =
    r#"{"content":[{"content":[],"type":"bulletList"}],"type":"doc"}"#.len();
const TASK_ENTRY_WRAPPER_BYTES: usize =
    r#"{"content":[{"content":[],"type":"taskList"}],"type":"doc"}"#.len();
const TABLE_ROW_WRAPPER_BYTES: usize =
    r#"{"content":[{"content":[],"type":"table"}],"type":"doc"}"#.len();
const DETAILS_PART_WRAPPER_BYTES: usize =
    r#"{"content":[{"content":[],"type":"details"}],"type":"doc"}"#.len();
const TABLE_CELL_WRAPPER_BYTES: usize =
    r#"{"content":[{"content":[{"content":[],"type":"tableRow"}],"type":"table"}],"type":"doc"}"#
        .len();
pub(super) const TABLE_CELL_SHELL_BYTES: usize = r#"{"type":"tableCell"}"#.len();
pub(super) const TABLE_HEADER_SHELL_BYTES: usize = r#"{"type":"tableHeader"}"#.len();
const EMPTY_TABLE_CELL_BYTES: usize =
    r#"{"content":[{"type":"paragraph"}],"type":"tableCell"}"#.len();

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
    "details",
    "callout",
    "boardView",
];

const INLINE: &[&str] = &["text", "hardBreak", "mention", "reference", "inlineMath"];
const LEAF: &[&str] = &[];
const LIST_ITEM_ONLY: &[&str] = &["listItem"];
const TASK_ITEM_ONLY: &[&str] = &["taskItem"];
const TEXT_ONLY: &[&str] = &["text"];
const TABLE_ROW_ONLY: &[&str] = &["tableRow"];
const TABLE_CELLS: &[&str] = &["tableHeader", "tableCell"];
/// The parts of a toggle, in the order it holds them.
const DETAILS_PARTS: &[&str] = &["detailsSummary", "detailsContent"];

/// Structural rule for one allowed non-text node type (matching `fvoci_model::allowlist`).
#[derive(Clone, Copy)]
struct KindSpec {
    name: &'static str,
    children: &'static [&'static str],
    min_children: usize,
    has_attrs: bool,
}

impl KindSpec {
    const fn plain(
        name: &'static str,
        children: &'static [&'static str],
        min_children: usize,
    ) -> Self {
        Self {
            name,
            children,
            min_children,
            has_attrs: false,
        }
    }

    const fn with_attrs(
        name: &'static str,
        children: &'static [&'static str],
        min_children: usize,
    ) -> Self {
        Self {
            name,
            children,
            min_children,
            has_attrs: true,
        }
    }
}

fn kind_spec(kind: &str) -> Option<KindSpec> {
    let spec = match kind {
        "doc" => KindSpec::plain("doc", DOCUMENT_BLOCKS, 1),
        "paragraph" => KindSpec::plain("paragraph", INLINE, 0),
        "heading" => KindSpec::with_attrs("heading", INLINE, 0),
        "bulletList" => KindSpec::plain("bulletList", LIST_ITEM_ONLY, 1),
        "orderedList" => KindSpec::with_attrs("orderedList", LIST_ITEM_ONLY, 1),
        "listItem" => KindSpec::plain("listItem", BLOCKS, 1),
        "blockquote" => KindSpec::plain("blockquote", BLOCKS, 1),
        "taskList" => KindSpec::plain("taskList", TASK_ITEM_ONLY, 1),
        "taskItem" => KindSpec::with_attrs("taskItem", BLOCKS, 1),
        "codeBlock" => KindSpec::with_attrs("codeBlock", TEXT_ONLY, 0),
        "table" => KindSpec::plain("table", TABLE_ROW_ONLY, 1),
        "tableRow" => KindSpec::plain("tableRow", TABLE_CELLS, 0),
        "tableHeader" => KindSpec::with_attrs("tableHeader", BLOCKS, 1),
        "tableCell" => KindSpec::with_attrs("tableCell", BLOCKS, 1),
        "horizontalRule" => KindSpec::plain("horizontalRule", LEAF, 0),
        "hardBreak" => KindSpec::plain("hardBreak", LEAF, 0),
        "image" => KindSpec::with_attrs("image", LEAF, 0),
        "file" => KindSpec::with_attrs("file", LEAF, 0),
        "mention" => KindSpec::with_attrs("mention", LEAF, 0),
        "reference" => KindSpec::with_attrs("reference", LEAF, 0),
        "inlineMath" => KindSpec::with_attrs("inlineMath", LEAF, 0),
        "blockMath" => KindSpec::with_attrs("blockMath", LEAF, 0),
        "details" => KindSpec::plain("details", DETAILS_PARTS, DETAILS_PARTS.len()),
        "detailsSummary" => KindSpec::plain("detailsSummary", INLINE, 0),
        "detailsContent" => KindSpec::plain("detailsContent", DOCUMENT_BLOCKS, 1),
        "callout" => KindSpec::with_attrs("callout", BLOCKS, 1),
        "boardView" => KindSpec::with_attrs("boardView", LEAF, 0),
        _ => return None,
    };
    Some(spec)
}

/// Summary of one surviving JSON child inside a parent element.
#[derive(Debug, Clone)]
pub(super) struct ChildSlot {
    pub(super) kind: &'static str,
    pub(super) nodes: usize,
    pub(super) height: usize,
    pub(super) json_bytes: usize,
    pub(super) has_marks: bool,
}

/// Validated summary of a surviving element subtree (no retained `serde_json::Value` tree).
#[derive(Debug)]
pub(super) struct NodeSummary {
    pub(super) kind: &'static str,
    pub(super) nodes: usize,
    pub(super) height: usize,
    pub(super) json_bytes: usize,
    pub(super) is_empty_paragraph: bool,
    pub(super) file_ids: Vec<String>,
    /// The board views in the subtree, the element itself counted.
    pub(super) board_views: usize,
}

/// One validated text run from `read::text_nodes`, ready to append into [`ContentAccum`].
#[derive(Debug)]
pub(super) struct TextRun {
    /// Exact compact JSON byte length of the full `{"type":"text",...}` node.
    full_json_bytes: usize,
    /// Exact compact JSON byte length of the escaped `text` content (excluding outer `"`).
    text_content_bytes: usize,
    /// `Some(marks_value)` if marked, `None` if unmarked.
    marks: Option<Value>,
}

/// Whether the immediately preceding child in [`ContentAccum`] was a `text` node, and if
/// so, its optional `marks` value for adjacent-text merge detection.
#[derive(Debug, Default)]
enum TrailingText {
    #[default]
    NotText,
    Text(Option<Value>),
}

/// Surviving children of one parent, mirroring `read::Content` adjacent text merging in
/// `O(1)` memory per text merge without retaining a `serde_json::Value` tree.
#[derive(Debug, Default)]
pub(super) struct ContentAccum {
    pub(super) children: Vec<ChildSlot>,
    trailing_text: TrailingText,
    /// Referenced `fileId`s from `image`/`file` nodes below this parent (document order).
    pub(super) file_ids: Vec<String>,
    /// The board views below this parent.
    pub(super) board_views: usize,
}

impl ContentAccum {
    pub(super) fn push_element(&mut self, summary: NodeSummary) {
        self.trailing_text = TrailingText::NotText;
        self.children.push(ChildSlot {
            kind: summary.kind,
            nodes: summary.nodes,
            height: summary.height,
            json_bytes: summary.json_bytes,
            has_marks: false,
        });
        if !summary.file_ids.is_empty() {
            self.file_ids.extend(summary.file_ids);
        }
        self.board_views = self.board_views.saturating_add(summary.board_views);
    }

    pub(super) fn push_text_runs(&mut self, runs: Vec<TextRun>) {
        for run in runs {
            if let TrailingText::Text(prev_marks) = &self.trailing_text
                && *prev_marks == run.marks
                && let Some(last) = self.children.last_mut()
            {
                last.json_bytes = last.json_bytes.saturating_add(run.text_content_bytes);
                continue;
            }
            let has_marks = run.marks.is_some();
            self.children.push(ChildSlot {
                kind: "text",
                nodes: 1,
                height: 1,
                json_bytes: run.full_json_bytes,
                has_marks,
            });
            self.trailing_text = TrailingText::Text(run.marks);
        }
    }

    /// Deduplicates `file_ids` preserving first-appearance document order.
    pub(super) fn dedup_file_ids(self) -> Vec<String> {
        dedup_strings(self.file_ids)
    }
}

/// Deduplicates `ids` preserving first-appearance document order (zero hash allocation
/// for 0 or 1 items).
pub(super) fn dedup_strings(mut ids: Vec<String>) -> Vec<String> {
    if ids.len() <= 1 {
        return ids;
    }
    let mut seen: HashSet<String> = HashSet::with_capacity(ids.len());
    ids.retain(|id| seen.insert(id.clone()));
    ids
}

/// Validates `nodes` from one `XmlText`, using one batched `Body::parse` call whenever
/// the combined runs fit within `BODY_MAX_NODES` and `BODY_MAX_BYTES`, or per-run
/// validation at the limit boundary so an invalid run in an oversized `XmlText` still
/// deletes the `XmlText` rather than raising a limit error.
pub(super) fn judge_text_nodes(nodes: Vec<Value>) -> Result<Option<Vec<TextRun>>, BodyError> {
    if nodes.is_empty() {
        return Ok(Some(Vec::new()));
    }
    let pre_sizes: Vec<(usize, usize)> = nodes
        .iter()
        .map(|node| {
            let full = json_size_of(node);
            let content = node
                .get("text")
                .map_or(0, |text| json_size_of(text).saturating_sub(2));
            (full, content)
        })
        .collect();
    let total_json: usize = pre_sizes
        .iter()
        .map(|(full, _)| *full)
        .fold(0, usize::saturating_add);
    let combined_bytes = LINE_WRAPPER_BYTES
        .saturating_add(total_json)
        .saturating_add(nodes.len().saturating_sub(1));

    if nodes.len().saturating_add(2) <= BODY_MAX_NODES && combined_bytes <= BODY_MAX_BYTES {
        let wrapped = json!({"type": "doc", "content": [{"type": "paragraph", "content": nodes}]});
        return match Body::parse(wrapped) {
            Ok(Some(body)) => {
                let arr = body.document()["content"][0]["content"]
                    .as_array()
                    .map_or(&[][..], Vec::as_slice);
                let runs = arr
                    .iter()
                    .zip(pre_sizes)
                    .map(|(node, (full_json_bytes, text_content_bytes))| TextRun {
                        full_json_bytes,
                        text_content_bytes,
                        marks: node.get("marks").cloned(),
                    })
                    .collect();
                Ok(Some(runs))
            }
            Ok(None) => Ok(Some(Vec::new())),
            Err(BodyError::InvalidNode { .. } | BodyError::NotADocument) => Ok(None),
            Err(limit) => Err(limit),
        };
    }

    let mut runs = Vec::with_capacity(nodes.len());
    for (node, (full_json_bytes, text_content_bytes)) in nodes.into_iter().zip(pre_sizes) {
        let marks = node.get("marks").cloned();
        let wrapped = json!({"type": "doc", "content": [{"type": "paragraph", "content": [node]}]});
        match Body::parse(wrapped) {
            Ok(_) => runs.push(TextRun {
                full_json_bytes,
                text_content_bytes,
                marks,
            }),
            Err(BodyError::InvalidNode { .. } | BodyError::NotADocument) => return Ok(None),
            Err(limit) => return Err(limit),
        }
    }
    Ok(Some(runs))
}

/// Validates a single non-table element's shell (type, place, and attributes) and its
/// direct surviving children without cloning any child JSON subtree.
///
/// Returns `Ok(None)` if the element fails allowlist validation (and should be deleted),
/// or `Err(limit)` if the element alone exceeds a depth, node, or byte limit.
pub(super) fn judge_element(
    tag: &str,
    place: Place,
    shell: Value,
    below: ContentAccum,
) -> Result<Option<NodeSummary>, BodyError> {
    let Some(spec) = kind_spec(tag) else {
        return Ok(None);
    };
    if !allowed_in_place(spec.name, place) {
        return Ok(None);
    }
    let shell_bytes = if place == Place::TableCell {
        if spec.name == "tableHeader" {
            TABLE_HEADER_SHELL_BYTES
        } else {
            TABLE_CELL_SHELL_BYTES
        }
    } else {
        json_size_of(&shell)
    };
    let own_file_id = extract_file_id(spec.name, &shell);

    if spec.has_attrs && !validate_shell_attrs(spec, place, shell)? {
        return Ok(None);
    }
    if !check_structural_rules(spec, &below.children) {
        return Ok(None);
    }
    let Some((subtree_nodes, max_child_height, children_json_bytes)) =
        check_children_and_limits(spec, place, &below.children)?
    else {
        return Ok(None);
    };

    let node_json_bytes = if below.children.is_empty() {
        shell_bytes
    } else {
        shell_bytes
            .saturating_add(CONTENT_FIELD_OVERHEAD)
            .saturating_add(children_json_bytes)
            .saturating_add(below.children.len() - 1)
    };
    let is_empty_paragraph = spec.name == "paragraph" && below.children.is_empty();
    let (_, _, wrapper_bytes) = wrapper_metrics(place);
    if !(place == Place::Block && is_empty_paragraph) {
        let checked_bytes = if place == Place::TableRow && below.children.is_empty() {
            wrapper_bytes
                .saturating_add(node_json_bytes)
                .saturating_add(CONTENT_FIELD_OVERHEAD)
                .saturating_add(EMPTY_TABLE_CELL_BYTES)
        } else {
            wrapper_bytes.saturating_add(node_json_bytes)
        };
        if checked_bytes > BODY_MAX_BYTES {
            return Err(BodyError::TooLarge);
        }
    }

    let board_views = below
        .board_views
        .saturating_add(usize::from(spec.name == "boardView"));
    let mut file_ids = below.dedup_file_ids();
    if let Some(id) = own_file_id {
        file_ids.insert(0, id);
    }
    Ok(Some(NodeSummary {
        kind: spec.name,
        nodes: subtree_nodes,
        height: 1 + max_child_height,
        json_bytes: node_json_bytes,
        is_empty_paragraph,
        file_ids,
        board_views,
    }))
}

fn check_structural_rules(spec: KindSpec, children: &[ChildSlot]) -> bool {
    if children.len() < spec.min_children {
        return false;
    }
    if !children.is_empty() && spec.children.is_empty() {
        return false;
    }
    if matches!(spec.name, "listItem" | "taskItem")
        && children.first().map(|child| child.kind) != Some("paragraph")
    {
        return false;
    }
    if spec.name == "details"
        && !children
            .iter()
            .map(|child| child.kind)
            .eq(DETAILS_PARTS.iter().copied())
    {
        return false;
    }
    if spec.name == "codeBlock" && children.iter().any(|child| child.has_marks) {
        return false;
    }
    true
}

fn check_children_and_limits(
    spec: KindSpec,
    place: Place,
    children: &[ChildSlot],
) -> Result<Option<(usize, usize, usize)>, BodyError> {
    let (wrapper_nodes, wrapper_depth, _) = wrapper_metrics(place);
    let mut nodes = wrapper_nodes + 1;
    let node_depth = wrapper_depth + 1;
    if nodes > BODY_MAX_NODES {
        return Err(BodyError::TooManyNodes);
    }
    if node_depth > BODY_MAX_DEPTH {
        return Err(BodyError::TooDeep);
    }
    let mut max_child_height = 0_usize;
    let mut children_json_bytes = 0_usize;
    for child in children {
        nodes = nodes.saturating_add(1);
        if nodes > BODY_MAX_NODES {
            return Err(BodyError::TooManyNodes);
        }
        if node_depth + 1 > BODY_MAX_DEPTH {
            return Err(BodyError::TooDeep);
        }
        if !spec.children.contains(&child.kind) {
            return Ok(None);
        }
        // Subtree-summary check: checks total nodes of this child before its max height.
        // If one child simultaneously exceeds both `BODY_MAX_NODES` and `BODY_MAX_DEPTH`
        // under its wrapper, this reports `TooManyNodes` (both map to `Refusal::BodyTooLarge`).
        nodes = nodes.saturating_add(child.nodes - 1);
        if nodes > BODY_MAX_NODES {
            return Err(BodyError::TooManyNodes);
        }
        if node_depth + child.height > BODY_MAX_DEPTH {
            return Err(BodyError::TooDeep);
        }
        max_child_height = max_child_height.max(child.height);
        children_json_bytes = children_json_bytes.saturating_add(child.json_bytes);
    }
    // `passes` validates a standalone `tableRow` inside a 1-row table (`TableGrid::of`),
    // which rejects rows wider than `TABLE_MAX_COLUMNS` with `TableTooLarge`.
    if spec.name == "tableRow" && children.len() > usize::from(TABLE_MAX_COLUMNS) {
        return Err(BodyError::TableTooLarge {
            path: String::new(),
        });
    }
    Ok(Some((
        nodes - wrapper_nodes,
        max_child_height,
        children_json_bytes,
    )))
}

/// Checks whether a validated [`NodeSummary`] is allowed as a top-level document block
/// (`Place::Block`), without cloning or re-walking its subtree.
pub(super) fn is_valid_root_block(summary: &NodeSummary) -> bool {
    DOCUMENT_BLOCKS.contains(&summary.kind)
}

/// Validates a node's attributes via `Body::parse` using only its own shell (with one
/// minimal dummy child when required by `min_children`).
fn validate_shell_attrs(spec: KindSpec, place: Place, mut shell: Value) -> Result<bool, BodyError> {
    enum Wrapper {
        Document,
        Block,
        Line,
        ListEntry,
        TaskEntry,
        TableRow,
    }
    let wrapper = match place {
        Place::Document => Wrapper::Document,
        Place::Block => Wrapper::Block,
        Place::Line => Wrapper::Line,
        Place::ListEntry => Wrapper::ListEntry,
        Place::TaskEntry => Wrapper::TaskEntry,
        Place::TableRow => Wrapper::TableRow,
        // The parts of a toggle have no attributes; `passes` strips `colspan`/`rowspan`
        // for standalone cell validation, and table cells have no other allowlisted
        // attributes.
        Place::DetailsPart | Place::TableCell => return Ok(true),
    };
    if spec.min_children > 0 {
        let dummy = match spec.name {
            "orderedList" => json!({"type": "listItem", "content": [{"type": "paragraph"}]}),
            _ => json!({"type": "paragraph"}),
        };
        if let Some(obj) = shell.as_object_mut() {
            obj.insert("content".to_owned(), Value::Array(vec![dummy]));
        }
    }
    let around = |kind: &str, inner: Value| json!({"type": kind, "content": [inner]});
    let document = match wrapper {
        Wrapper::Document => shell,
        Wrapper::Block => around("doc", shell),
        Wrapper::Line => around("doc", around("paragraph", shell)),
        Wrapper::ListEntry => around("doc", around("bulletList", shell)),
        Wrapper::TaskEntry => around("doc", around("taskList", shell)),
        Wrapper::TableRow => around("doc", around("table", shell)),
    };
    match Body::parse(document) {
        Ok(_) => Ok(true),
        Err(BodyError::InvalidNode { .. } | BodyError::NotADocument) => Ok(false),
        Err(limit) => Err(limit),
    }
}

fn allowed_in_place(kind: &str, place: Place) -> bool {
    match place {
        Place::Document => kind == "doc",
        Place::Block => DOCUMENT_BLOCKS.contains(&kind),
        Place::Line => INLINE.contains(&kind),
        Place::ListEntry => kind == "listItem",
        Place::TaskEntry => kind == "taskItem",
        Place::TableRow => kind == "tableRow",
        Place::TableCell => TABLE_CELLS.contains(&kind),
        Place::DetailsPart => DETAILS_PARTS.contains(&kind),
    }
}

fn wrapper_metrics(place: Place) -> (usize, usize, usize) {
    match place {
        Place::Document => (0, 0, 0),
        Place::Block => (1, 1, BLOCK_WRAPPER_BYTES),
        Place::Line => (2, 2, LINE_WRAPPER_BYTES),
        Place::ListEntry => (2, 2, LIST_ENTRY_WRAPPER_BYTES),
        Place::TaskEntry => (2, 2, TASK_ENTRY_WRAPPER_BYTES),
        Place::TableRow => (2, 2, TABLE_ROW_WRAPPER_BYTES),
        Place::TableCell => (3, 3, TABLE_CELL_WRAPPER_BYTES),
        Place::DetailsPart => (2, 2, DETAILS_PART_WRAPPER_BYTES),
    }
}

fn extract_file_id(kind: &str, shell: &Value) -> Option<String> {
    if !matches!(kind, "image" | "file") {
        return None;
    }
    shell
        .get("attrs")?
        .get("fileId")?
        .as_str()
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapper_byte_constants_match_serde_json() {
        let len = |v: Value| serde_json::to_vec(&v).expect("json").len();
        assert_eq!(
            BLOCK_WRAPPER_BYTES,
            len(json!({"type": "doc", "content": []}))
        );
        assert_eq!(
            LINE_WRAPPER_BYTES,
            len(json!({"type": "doc", "content": [{"type": "paragraph", "content": []}]}))
        );
        assert_eq!(
            LIST_ENTRY_WRAPPER_BYTES,
            len(json!({"type": "doc", "content": [{"type": "bulletList", "content": []}]}))
        );
        assert_eq!(
            TASK_ENTRY_WRAPPER_BYTES,
            len(json!({"type": "doc", "content": [{"type": "taskList", "content": []}]}))
        );
        assert_eq!(
            TABLE_ROW_WRAPPER_BYTES,
            len(json!({"type": "doc", "content": [{"type": "table", "content": []}]}))
        );
        assert_eq!(
            TABLE_CELL_WRAPPER_BYTES,
            len(json!({
                "type": "doc",
                "content": [{"type": "table", "content": [{"type": "tableRow", "content": []}]}]
            }))
        );
        assert_eq!(
            DETAILS_PART_WRAPPER_BYTES,
            len(json!({"type": "doc", "content": [{"type": "details", "content": []}]}))
        );
        assert_eq!(TABLE_CELL_SHELL_BYTES, len(json!({"type": "tableCell"})));
        assert_eq!(
            TABLE_HEADER_SHELL_BYTES,
            len(json!({"type": "tableHeader"}))
        );
        assert_eq!(
            EMPTY_TABLE_CELL_BYTES,
            len(json!({"type": "tableCell", "content": [{"type": "paragraph"}]}))
        );
        let with = len(json!({"type": "heading", "attrs": {"level": 2}, "content": []}));
        let without = len(json!({"type": "heading", "attrs": {"level": 2}}));
        assert_eq!(with - without, CONTENT_FIELD_OVERHEAD);
    }

    #[test]
    fn text_merge_byte_math_matches_serde_json_across_unicode_and_escapes() {
        let samples = ["a", "한글😀", "quote\"slash\\", "ctrl\n\t\x01end"];
        for a in samples {
            for b in samples {
                let first = json!({"type": "text", "text": a});
                let second = json!({"type": "text", "text": b});
                let merged = json!({"type": "text", "text": format!("{a}{b}")});
                let r1_full = json_size_of(&first);
                let r2_content = json_size_of(&second["text"]).saturating_sub(2);
                assert_eq!(r1_full + r2_content, json_size_of(&merged), "{a:?} + {b:?}");
            }
        }
    }

    #[test]
    fn kind_spec_matches_schema_nodes_and_allowlist() {
        for (name, attrs) in crate::schema::NODES {
            if name == "text" {
                continue;
            }
            let spec = kind_spec(name).unwrap_or_else(|| panic!("missing kind_spec for {name}"));
            assert_eq!(spec.name, name);
            assert_eq!(
                spec.has_attrs,
                !attrs.is_empty(),
                "has_attrs drift for schema node {name}"
            );
        }
        assert!(kind_spec("text").is_none());
        assert!(kind_spec("unknown-node").is_none());
    }
}
