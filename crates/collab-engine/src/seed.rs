//! Tiptap JSON → Yjs updateV1 seed (writer; the inverse of [`crate::project`]).
//!
//! Mirrors FVOCI source SHA `393795261322b916e588043cf94feca999175843`
//! `packages/editor/src/collab-tiptap.ts` `tiptapJsonToYUpdate`:
//! `Y.Doc({ gc: false })`, `prosemirrorJSONToYXmlFragment(schema, {type: "doc",
//! content: json.content ?? []}, doc.getXmlFragment("prosemirror"))`, then
//! `Y.encodeStateAsUpdate`. `@tiptap/y-tiptap` 3.0.9:
//!
//! - `Node.fromJSON` (prosemirror-model) validates first: unknown node/mark type,
//!   non-string or empty text, non-array `content`/`marks` throw. Attributes are
//!   rebuilt from the schema (`computeAttrs`): unknown keys dropped, missing keys
//!   take the schema default, explicit `null` stays `null`.
//! - `createTypeFromElementNode`: `Y.XmlElement(type)` with every attr that is not
//!   `null` and not `ychange`.
//! - `normalizePNodeContent` + `createTypeFromTextNodes`: consecutive text nodes →
//!   one `Y.XmlText`, `applyDelta([{insert, attributes: marksToAttributes(marks)}])`.
//!   `marksToAttributes` skips `ychange` and keys by mark name; the value is the
//!   full computed `mark.attrs` object (`{}` for attr-less marks).
//!
//! Overlapping marks (`!type.excludes(type)`) would be keyed `name--<hash>`. No mark
//! of the FVOCI schema overlaps (see [`SCHEMA_MARKS`] and the schema fixture test),
//! so that branch is unreachable here and not implemented.
//!
//! Byte equality with Yjs is not a goal (client ids and clocks differ); the decoded
//! `prosemirror` tree is (compat/fixtures/yjs-seed).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde_json::Value;
use yrs::types::{Attrs, Delta};
use yrs::{
    Any, In, Number, ReadTxn, StateVector, Text, Transact, TransactionMut, Xml, XmlElementPrelim,
    XmlFragment, XmlTextPrelim,
};

use crate::limits::Limits;
use crate::outcome::{EngineStatus, LimitKind};

/// Stack guard for the recursive writer. Inputs arrive through a serde_json
/// parse (recursion limit 128), so this is never reached in practice.
const MAX_SEED_DEPTH: u32 = 256;

/// Exact current packages/editor/src/extract.ts UNIQUE_ID_NODE_TYPES.
/// `embed.id` is a block identity; `embed.ref` is a resource reference.
const UNIQUE_ID_NODE_TYPES: &[&str] = &[
    "heading",
    "paragraph",
    "blockquote",
    "codeBlock",
    "embed",
    "listItem",
    "table",
    "horizontalRule",
    "callout",
    "mermaid",
    "math",
    "details",
    "detailsContent",
    "detailsSummary",
    "taskList",
    "taskItem",
];

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AttrDefault {
    Null,
    Str(&'static str),
    Int(i64),
    Bool(bool),
    EmptyArray,
}

pub type AttrSpec = (&'static str, AttrDefault);

use AttrDefault::{Bool as B, EmptyArray as EA, Int as I, Null as N, Str as S};

/// `getSchema(createFvociExtensions())` node attrs in schema order
/// (`compat/fixtures/yjs-seed/schema.json`).
pub const SCHEMA_NODES: &[(&str, &[AttrSpec])] = &[
    ("paragraph", &[("id", N), ("ychange", N), ("textAlign", N)]),
    ("blockquote", &[("id", N), ("ychange", N)]),
    ("bulletList", &[("ychange", N)]),
    ("doc", &[]),
    ("hardBreak", &[]),
    (
        "heading",
        &[
            ("id", N),
            ("ychange", N),
            ("data-toc-id", N),
            ("textAlign", N),
            ("level", I(1)),
        ],
    ),
    ("horizontalRule", &[("id", N), ("ychange", N)]),
    ("listItem", &[("id", N), ("ychange", N)]),
    (
        "orderedList",
        &[("ychange", N), ("start", I(1)), ("type", N)],
    ),
    ("text", &[]),
    (
        "codeBlock",
        &[
            ("id", N),
            ("ychange", N),
            ("language", N),
            ("highlightLines", EA),
        ],
    ),
    ("table", &[("id", N), ("ychange", N)]),
    (
        "tableCell",
        &[
            ("background", N),
            ("ychange", N),
            ("colspan", I(1)),
            ("rowspan", I(1)),
            ("colwidth", N),
            ("align", N),
        ],
    ),
    (
        "tableHeader",
        &[
            ("background", N),
            ("ychange", N),
            ("colspan", I(1)),
            ("rowspan", I(1)),
            ("colwidth", N),
            ("align", N),
        ],
    ),
    ("tableRow", &[("ychange", N)]),
    ("details", &[("id", N), ("ychange", N), ("open", B(false))]),
    ("detailsContent", &[("id", N), ("ychange", N)]),
    ("detailsSummary", &[("id", N), ("ychange", N)]),
    ("taskList", &[("id", N), ("ychange", N)]),
    (
        "taskItem",
        &[("id", N), ("ychange", N), ("checked", B(false))],
    ),
    ("callout", &[("id", N), ("ychange", N), ("kind", S("note"))]),
    ("mermaid", &[("id", N), ("ychange", N), ("source", S(""))]),
    ("math", &[("id", N), ("ychange", N), ("latex", S(""))]),
    ("mathInline", &[("latex", S(""))]),
    (
        "attachment",
        &[
            ("ychange", N),
            ("id", N),
            ("name", S("")),
            ("image", B(false)),
            ("width", N),
            ("align", N),
            ("caption", N),
            ("previewWidth", N),
            ("previewHeight", N),
        ],
    ),
    ("emoji", &[("name", N)]),
    (
        "mention",
        &[("entity", S("user")), ("id", S("")), ("label", S(""))],
    ),
    (
        "embed",
        &[
            ("id", N),
            ("ychange", N),
            ("entity", S("document")),
            ("ref", S("")),
        ],
    ),
];

/// Mark attrs in schema order; every FVOCI mark excludes itself (non-overlapping).
pub const SCHEMA_MARKS: &[(&str, &[AttrSpec])] = &[
    (
        "link",
        &[
            ("href", N),
            ("target", S("_blank")),
            ("rel", S("noopener noreferrer nofollow")),
            ("class", N),
            ("title", N),
        ],
    ),
    ("textStyle", &[("color", N)]),
    ("bold", &[]),
    ("code", &[]),
    ("italic", &[]),
    ("strike", &[]),
    ("underline", &[]),
    ("ychange", &[("user", N), ("type", N), ("color", N)]),
    ("highlight", &[("color", N)]),
];

fn node_spec(name: &str) -> Option<(&'static str, &'static [AttrSpec])> {
    SCHEMA_NODES.iter().find(|(n, _)| *n == name).copied()
}

fn mark_spec(name: &str) -> Option<(&'static str, &'static [AttrSpec])> {
    SCHEMA_MARKS.iter().find(|(n, _)| *n == name).copied()
}

fn malformed(detail: impl Into<String>) -> EngineStatus {
    EngineStatus::Malformed {
        detail: format!("seed: {}", detail.into()),
    }
}

/// Validated ProseMirror element (the `Node.fromJSON` result y-tiptap walks).
struct PElement {
    tag: &'static str,
    /// Non-null, non-`ychange` attrs in schema order.
    attrs: Vec<(&'static str, Any)>,
    children: Vec<PChild>,
}

/// A mark after `Mark.fromJSON`: `(type name, computed attrs in schema order)`.
type PMark = (&'static str, Vec<(&'static str, Any)>);

enum PChild {
    Element(PElement),
    /// Consecutive text nodes: `(text, marks in Mark.setFrom rank order)`.
    Text(Vec<(String, Vec<PMark>)>),
}

/// `tiptapJsonToYUpdate(json)`: updateV1 of a fresh gc-disabled UTF-16 Doc whose
/// `prosemirror` fragment holds `json.content`. `json` must already be a Tiptap
/// doc object (the parent checks `isTiptapDoc`); the top-level `type` is ignored
/// like the source, which always rebuilds `{type: "doc", content}`.
pub fn tiptap_to_yjs_update(json: &Value, limits: &Limits) -> Result<Vec<u8>, EngineStatus> {
    seed_tiptap(json, limits, false)
}

/// Independent body for a NEW document: use the canonical validated tree and
/// fresh native Doc, refreshing only configured block ids. No source update,
/// history, resource identity or caller-owned document identity is reused.
pub fn tiptap_to_independent_yjs_update(
    json: &Value,
    limits: &Limits,
) -> Result<Vec<u8>, EngineStatus> {
    seed_tiptap(json, limits, true)
}

fn seed_tiptap(json: &Value, limits: &Limits, independent: bool) -> Result<Vec<u8>, EngineStatus> {
    let Some(obj) = json.as_object() else {
        return Err(malformed("contentJson is not an object"));
    };
    let mut children = match obj.get("content") {
        None => Vec::new(),
        Some(content) => fragment_from_json(content, 1)?,
    };
    if independent {
        let mut used_ids = HashSet::new();
        collect_block_ids(&children, &mut used_ids);
        refresh_block_ids(&mut children, &mut used_ids);
    }
    let doc = crate::engine::new_doc();
    let frag = doc.get_or_insert_xml_fragment(crate::FRAGMENT);
    let bytes = {
        let mut txn = doc.transact_mut();
        for child in children {
            write_child(&mut txn, &frag, child);
        }
        txn.encode_state_as_update_v1(&StateVector::default())
    };
    if bytes.len() as u64 > limits.max_output_bytes {
        return Err(EngineStatus::ResourceLimit {
            kind: LimitKind::Output,
            detail: format!(
                "seed update {} bytes exceeds {}-byte limit",
                bytes.len(),
                limits.max_output_bytes
            ),
        });
    }
    Ok(bytes)
}

fn collect_block_ids(children: &[PChild], ids: &mut HashSet<Arc<str>>) {
    for child in children {
        if let PChild::Element(element) = child {
            if UNIQUE_ID_NODE_TYPES.contains(&element.tag) {
                for (name, value) in &element.attrs {
                    if let ("id", Any::String(id)) = (*name, value) {
                        ids.insert(id.clone());
                    }
                }
            }
            collect_block_ids(&element.children, ids);
        }
    }
}

fn refresh_block_ids(children: &mut [PChild], used: &mut HashSet<Arc<str>>) {
    for child in children {
        if let PChild::Element(element) = child {
            if UNIQUE_ID_NODE_TYPES.contains(&element.tag) {
                // Reuse the maintained Yrs UUID generator. Avoid source ids
                // and any ids already assigned in this seed; child time limits
                // still bound the whole operation, including collision retry.
                let id = loop {
                    let id = yrs::uuid_v4();
                    if used.insert(id.clone()) {
                        break id;
                    }
                };
                element.attrs.retain(|(name, _)| *name != "id");
                // id is the first schema attribute for every configured type.
                element.attrs.insert(0, ("id", Any::String(id)));
            }
            refresh_block_ids(&mut element.children, used);
        }
    }
}

fn check_depth(depth: u32) -> Result<(), EngineStatus> {
    if depth > MAX_SEED_DEPTH {
        return Err(EngineStatus::ResourceLimit {
            kind: LimitKind::Stack,
            detail: format!("seed nesting {depth} exceeds max {MAX_SEED_DEPTH}"),
        });
    }
    Ok(())
}

/// JS truthiness of a JSON value (`if (json.content)`, `if (json.marks)`).
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0 && !f.is_nan()),
        Value::String(s) => !s.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// `Fragment.fromJSON` + `normalizePNodeContent`.
fn fragment_from_json(content: &Value, depth: u32) -> Result<Vec<PChild>, EngineStatus> {
    if !truthy(content) {
        return Ok(Vec::new());
    }
    let Value::Array(items) = content else {
        return Err(malformed("content is not an array"));
    };
    let mut out: Vec<PChild> = Vec::with_capacity(items.len());
    for item in items {
        match node_from_json(item, depth)? {
            // `Fragment.fromArray` joins a text node into the previous one when
            // `sameMarkup` (same mark set, deep-equal attrs).
            Parsed::Text(text, marks) => match out.last_mut() {
                Some(PChild::Text(run)) => match run.last_mut() {
                    Some((prev, prev_marks)) if *prev_marks == marks => prev.push_str(&text),
                    _ => run.push((text, marks)),
                },
                _ => out.push(PChild::Text(vec![(text, marks)])),
            },
            Parsed::Element(el) => out.push(PChild::Element(el)),
        }
    }
    Ok(out)
}

enum Parsed {
    Text(String, Vec<PMark>),
    Element(PElement),
}

/// prosemirror-model `Node.fromJSON`.
fn node_from_json(json: &Value, depth: u32) -> Result<Parsed, EngineStatus> {
    check_depth(depth)?;
    let Some(obj) = json.as_object() else {
        return Err(malformed("node is not an object"));
    };
    let marks = match obj.get("marks") {
        Some(marks) if truthy(marks) => {
            let Value::Array(marks) = marks else {
                return Err(malformed("marks is not an array"));
            };
            let mut parsed = Vec::with_capacity(marks.len());
            for mark in marks {
                parsed.push(mark_from_json(mark, depth + 1)?);
            }
            parsed
        }
        _ => Vec::new(),
    };
    let Some(type_name) = obj.get("type").and_then(Value::as_str) else {
        return Err(malformed("node type is not a string"));
    };
    if type_name == "text" {
        let Some(text) = obj.get("text").and_then(Value::as_str) else {
            return Err(malformed("text node without string text"));
        };
        if text.is_empty() {
            return Err(malformed("empty text node"));
        }
        let mut marks = marks;
        // `Mark.setFrom`: stable sort by schema rank.
        marks.sort_by_key(|(name, _)| SCHEMA_MARKS.iter().position(|(n, _)| n == name));
        return Ok(Parsed::Text(text.to_string(), marks));
    }
    let Some((tag, spec)) = node_spec(type_name) else {
        return Err(malformed(format!("unknown node type {type_name:?}")));
    };
    // y-tiptap encodes marks on XmlText, not on an emoji XmlElement. Refuse
    // an unrepresentable imported atom before producing a seed; marked
    // Unicode text remains supported and comparison-only ychange is ignored.
    if tag == "emoji" && marks.iter().any(|(name, _)| *name != "ychange") {
        return Err(malformed("marked emoji must be Unicode text"));
    }
    let children = match obj.get("content") {
        None => Vec::new(),
        Some(content) => fragment_from_json(content, depth + 1)?,
    };
    let mut attrs = Vec::new();
    for (key, value) in compute_attrs(spec, obj.get("attrs"), depth + 1)? {
        if key != "ychange" && !matches!(value, Any::Null) {
            attrs.push((key, value));
        }
    }
    Ok(Parsed::Element(PElement {
        tag,
        attrs,
        children,
    }))
}

/// prosemirror-model `Mark.fromJSON`: `(name, computed attrs)`.
fn mark_from_json(json: &Value, depth: u32) -> Result<PMark, EngineStatus> {
    check_depth(depth)?;
    let Some(obj) = json.as_object() else {
        return Err(malformed("mark is not an object"));
    };
    let name = obj.get("type").and_then(Value::as_str).unwrap_or_default();
    let Some((name, spec)) = mark_spec(name) else {
        return Err(malformed(format!("unknown mark type {name:?}")));
    };
    Ok((name, compute_attrs(spec, obj.get("attrs"), depth + 1)?))
}

/// prosemirror-model `computeAttrs`: `value && value[name]`, `undefined` → default.
fn compute_attrs(
    spec: &'static [AttrSpec],
    given: Option<&Value>,
    depth: u32,
) -> Result<Vec<(&'static str, Any)>, EngineStatus> {
    let given = given.and_then(Value::as_object);
    let mut out = Vec::with_capacity(spec.len());
    for (name, default) in spec {
        let value = match given.and_then(|g| g.get(*name)) {
            Some(v) => json_to_any(v, depth)?,
            None => default_any(*default),
        };
        out.push((*name, value));
    }
    Ok(out)
}

fn default_any(default: AttrDefault) -> Any {
    match default {
        AttrDefault::Null => Any::Null,
        AttrDefault::Str(s) => Any::String(Arc::from(s)),
        AttrDefault::Int(i) => Any::Number(Number::Int(i)),
        AttrDefault::Bool(b) => Any::Bool(b),
        AttrDefault::EmptyArray => Any::Array(Arc::from(Vec::<Any>::new())),
    }
}

/// JSON → Yrs `Any`, numbers encoded like lib0 `writeAny` (31-bit integers as
/// varint, everything else as float32/float64).
fn json_to_any(value: &Value, depth: u32) -> Result<Any, EngineStatus> {
    check_depth(depth)?;
    Ok(match value {
        Value::Null => Any::Null,
        Value::Bool(b) => Any::Bool(*b),
        Value::Number(n) => {
            let f = n
                .as_f64()
                .ok_or_else(|| malformed("number is not representable"))?;
            if f.fract() == 0.0 && f.abs() <= 0x7FFF_FFFF as f64 {
                Any::Number(Number::Int(f as i64))
            } else {
                Any::Number(Number::Float(f))
            }
        }
        Value::String(s) => Any::String(Arc::from(s.as_str())),
        Value::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(json_to_any(item, depth + 1)?);
            }
            Any::Array(Arc::from(out))
        }
        Value::Object(map) => {
            let mut out = HashMap::with_capacity(map.len());
            for (k, v) in map {
                out.insert(k.clone(), json_to_any(v, depth + 1)?);
            }
            Any::Map(Arc::new(out))
        }
    })
}

/// y-tiptap `marksToAttributes` (a later mark of the same type overwrites the
/// earlier value).
fn marks_to_attributes(marks: Vec<PMark>) -> Attrs {
    let mut out = Attrs::new();
    for (name, attrs) in marks {
        if name == "ychange" {
            continue;
        }
        let map: HashMap<String, Any> =
            attrs.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
        out.insert(Arc::from(name), Any::Map(Arc::new(map)));
    }
    out
}

fn write_child<F: XmlFragment>(txn: &mut TransactionMut, parent: &F, child: PChild) {
    match child {
        PChild::Element(el) => {
            let node = parent.push_back(txn, XmlElementPrelim::empty(el.tag));
            for (key, value) in el.attrs {
                node.insert_attribute(txn, key, value);
            }
            for child in el.children {
                write_child(txn, &node, child);
            }
        }
        PChild::Text(run) => {
            let text = parent.push_back(txn, XmlTextPrelim::new(""));
            let delta = run.into_iter().map(|(insert, marks)| {
                Delta::Inserted(
                    In::Any(Any::String(Arc::from(insert))),
                    Some(Box::new(marks_to_attributes(marks))),
                )
            });
            text.apply_delta(txn, delta);
        }
    }
}

/// Decoded check used by tests: the fragment of a seeded update is readable.
#[cfg(test)]
fn project(update: &[u8]) -> Value {
    use yrs::updates::decoder::Decode;
    let doc = crate::engine::new_doc();
    doc.transact_mut()
        .apply_update(yrs::Update::decode_v1(update).expect("decode"))
        .expect("apply");
    let txn = doc.transact();
    crate::project::project_prosemirror(&txn, &Limits::for_tests()).expect("project")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn seed(json: &Value) -> Vec<u8> {
        tiptap_to_yjs_update(json, &Limits::for_tests()).expect("seed")
    }

    fn independent(json: &Value) -> Vec<u8> {
        tiptap_to_independent_yjs_update(json, &Limits::for_tests()).expect("independent seed")
    }

    #[test]
    fn independent_seed_refreshes_exact_editor_block_types_and_native_identity() {
        use yrs::updates::decoder::Decode;

        let expected = [
            "heading",
            "paragraph",
            "blockquote",
            "codeBlock",
            "embed",
            "listItem",
            "table",
            "horizontalRule",
            "callout",
            "mermaid",
            "math",
            "details",
            "detailsContent",
            "detailsSummary",
            "taskList",
            "taskItem",
        ];
        assert_eq!(UNIQUE_ID_NODE_TYPES, expected);
        let editor = include_str!("../../../packages/editor/src/extract.ts");
        let policy = editor
            .split("export const UNIQUE_ID_NODE_TYPES = [")
            .nth(1)
            .expect("editor policy")
            .split(']')
            .next()
            .expect("policy end");
        let actual: Vec<_> = policy
            .lines()
            .filter_map(|line| {
                line.trim()
                    .strip_prefix('"')
                    .and_then(|s| s.split('"').next())
            })
            .collect();
        assert_eq!(actual, expected);
        let input = json!({"type":"doc","content": expected.iter().enumerate().map(|(i, name)| {
            json!({"type":name,"attrs":{"id":format!("source-{i}")}})
        }).collect::<Vec<_>>()});
        let original = input.clone();
        let old = seed(&input);
        let mut all_ids = HashSet::new();
        let mut clients = HashSet::new();
        for (index, bytes) in [old, independent(&input), independent(&input)]
            .iter()
            .enumerate()
        {
            let projected = project(bytes);
            for (i, name) in expected.iter().enumerate() {
                assert_eq!(projected["content"][i]["type"], *name);
                let id = projected["content"][i]["attrs"]["id"]
                    .as_str()
                    .expect("block id");
                if index == 0 {
                    assert_eq!(id, format!("source-{i}"));
                } else {
                    assert!(!id.starts_with("source-"));
                    assert_eq!(id.len(), 36);
                    assert_eq!(&id[14..15], "4");
                    assert!(all_ids.insert(id.to_owned()), "duplicate copied block id");
                }
            }
            let doc = crate::engine::new_doc();
            doc.transact_mut()
                .apply_update(yrs::Update::decode_v1(bytes).expect("update"))
                .expect("integrate");
            let snapshot = doc.transact().snapshot();
            assert!(
                snapshot.delete_set.is_empty(),
                "fresh seed imported deletions"
            );
            assert_eq!(
                snapshot.state_map.len(),
                1,
                "fresh seed imported other clients"
            );
            let client = *snapshot.state_map.iter().next().expect("seed client").0;
            assert!(clients.insert(client), "observed reused native client");
        }
        assert_eq!(input, original, "borrowed input changed");
    }

    #[test]
    fn independent_seed_preserves_literal_nested_content_marks_and_resource_targets() {
        let input = json!({"type":"doc","content":[
            {"type":"table","attrs":{"id":"source-table"},"content":[
                {"type":"tableRow","content":[{"type":"tableCell","attrs":{
                    "colspan":2,"rowspan":3,"colwidth":[120,80],"background":"red","align":"right"
                },"content":[{"type":"paragraph","attrs":{"id":"source-p"},"content":[
                    {"type":"text","text":"한글😀","marks":[{"type":"bold"},{"type":"link","attrs":{"href":"https://example.invalid/a#b"}}]},
                    {"type":"mathInline","attrs":{"latex":"x^2"}},
                    {"type":"mention","attrs":{"entity":"document","id":"resource-document","label":"Doc"}}
                ]}]}]}
            ]},
            {"type":"attachment","attrs":{"id":"resource-attachment","name":"image.png","image":true}},
            {"type":"embed","attrs":{"id":"source-embed","entity":"document","ref":"resource-embed"}},
            {"type":"orderedList","attrs":{"start":7},"content":[{"type":"listItem","attrs":{"id":"source-item"},"content":[{"type":"math","attrs":{"id":"source-math","latex":"\\frac{1}{2}"}}]}]},
            {"type":"taskList","attrs":{"id":"source-tasks"},"content":[{"type":"taskItem","attrs":{"id":"source-task","checked":true}}]}
        ]});
        let original = input.clone();
        let expected = json!({"type":"doc","content":[
            {"type":"table","attrs":{"id":"<fresh>"},"content":[
                {"type":"tableRow","content":[{"type":"tableCell","attrs":{
                    "colspan":2,"rowspan":3,"colwidth":[120,80],"background":"red","align":"right"
                },"content":[{"type":"paragraph","attrs":{"id":"<fresh>"},"content":[
                    {"type":"text","text":"한글😀","marks":[{"type":"bold","attrs":{}},{"type":"link","attrs":{"href":"https://example.invalid/a#b","target":"_blank","rel":"noopener noreferrer nofollow","class":null,"title":null}}]},
                    {"type":"mathInline","attrs":{"latex":"x^2"}},
                    {"type":"mention","attrs":{"entity":"document","id":"resource-document","label":"Doc"}}
                ]}]}]}
            ]},
            {"type":"attachment","attrs":{"id":"resource-attachment","name":"image.png","image":true}},
            {"type":"embed","attrs":{"id":"<fresh>","entity":"document","ref":"resource-embed"}},
            {"type":"orderedList","attrs":{"start":7},"content":[{"type":"listItem","attrs":{"id":"<fresh>"},"content":[{"type":"math","attrs":{"id":"<fresh>","latex":"\\frac{1}{2}"}}]}]},
            {"type":"taskList","attrs":{"id":"<fresh>"},"content":[{"type":"taskItem","attrs":{"id":"<fresh>","checked":true}}]}
        ]});
        fn normalize_blocks(node: &mut Value) {
            // Fixture types only; resource id/ref fields are never normalized.
            if matches!(
                node["type"].as_str(),
                Some(
                    "table" | "paragraph" | "embed" | "listItem" | "math" | "taskList" | "taskItem"
                )
            ) {
                let id = node["attrs"]["id"].as_str().expect("fresh id");
                assert!(!id.starts_with("source-"));
                node["attrs"]["id"] = json!("<fresh>");
            }
            if let Some(children) = node.get_mut("content").and_then(Value::as_array_mut) {
                for child in children {
                    normalize_blocks(child);
                }
            }
        }
        let mut observed = project(&independent(&input));
        normalize_blocks(&mut observed);
        assert_eq!(observed, expected, "literal semantic oracle");
        assert_eq!(input, original);
        let legacy = project(&seed(&input));
        assert_eq!(legacy["content"][0]["attrs"]["id"], "source-table");
        assert_eq!(legacy["content"][2]["attrs"]["id"], "source-embed");
        assert_eq!(legacy["content"][2]["attrs"]["ref"], "resource-embed");
    }

    #[test]
    fn independent_seed_rejects_malformed_and_oversize_without_truncation() {
        for input in [
            json!(null),
            json!({"content":{}}),
            json!({"content":[{"type":"nope"}]}),
            json!({"content":[{"type":"text","text":""}]}),
            json!({"content":[{"type":"paragraph","marks":[{"type":"nope"}]}]}),
        ] {
            assert!(matches!(
                tiptap_to_independent_yjs_update(&input, &Limits::for_tests()),
                Err(EngineStatus::Malformed { .. })
            ));
        }
        let input = json!({"content":[{"type":"paragraph","content":[{"type":"text","text":"x".repeat(4096)}]}]});
        let limits = Limits {
            max_output_bytes: 1024,
            ..Limits::for_tests()
        };
        assert!(matches!(
            tiptap_to_independent_yjs_update(&input, &limits),
            Err(EngineStatus::ResourceLimit {
                kind: LimitKind::Output,
                ..
            })
        ));
    }

    #[test]
    fn empty_doc_has_empty_fragment() {
        for doc in [
            json!({"type": "doc"}),
            json!({"type": "doc", "content": []}),
            json!({"type": "doc", "content": null}),
        ] {
            assert_eq!(project(&seed(&doc)), json!({"type": "doc", "content": []}));
        }
    }

    #[test]
    fn defaults_are_filled_and_nulls_dropped() {
        let doc = json!({"type": "doc", "content": [
            {"type": "heading", "attrs": {"id": null, "bogus": 1}, "content": [{"type": "text", "text": "가"}]},
            {"type": "codeBlock", "attrs": {"language": "rs"}},
        ]});
        assert_eq!(
            project(&seed(&doc)),
            json!({"type": "doc", "content": [
                {"type": "heading", "attrs": {"level": 1}, "content": [{"type": "text", "text": "가"}]},
                {"type": "codeBlock", "attrs": {"language": "rs", "highlightLines": []}},
            ]})
        );
    }

    #[test]
    fn marks_carry_full_attrs_and_skip_ychange() {
        let doc = json!({"type": "doc", "content": [{"type": "paragraph", "content": [
            {"type": "text", "text": "a😀", "marks": [{"type": "bold"}, {"type": "link", "attrs": {"href": "https://x"}}, {"type": "ychange", "attrs": {"type": "added"}}]},
            {"type": "text", "text": "b"},
        ]}]});
        assert_eq!(
            project(&seed(&doc)),
            json!({"type": "doc", "content": [{"type": "paragraph", "content": [
                {"type": "text", "text": "a😀", "marks": [
                    {"type": "bold", "attrs": {}},
                    {"type": "link", "attrs": {"href": "https://x", "target": "_blank", "rel": "noopener noreferrer nofollow", "class": null, "title": null}},
                ]},
                {"type": "text", "text": "b"},
            ]}]})
        );
    }

    #[test]
    fn invalid_nodes_are_malformed() {
        for doc in [
            json!({"type": "doc", "content": {}}),
            json!({"type": "doc", "content": [{"type": "nope"}]}),
            json!({"type": "doc", "content": [{"type": "paragraph", "content": [{"type": "text", "text": ""}]}]}),
            json!({"type": "doc", "content": [{"type": "paragraph", "content": [{"type": "text"}]}]}),
            json!({"type": "doc", "content": [{"type": "paragraph", "marks": [{"type": "nope"}]}]}),
            json!({"type": "doc", "content": [{"type": "paragraph", "marks": {}}]}),
            json!({"type": "doc", "content": [1]}),
        ] {
            let err = tiptap_to_yjs_update(&doc, &Limits::for_tests()).expect_err("malformed");
            assert!(
                matches!(err, EngineStatus::Malformed { .. }),
                "{doc}: {err:?}"
            );
        }
    }

    #[test]
    fn output_cap_is_enforced() {
        let doc = json!({"type": "doc", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "x".repeat(4096)}]}]});
        let limits = Limits {
            max_output_bytes: 1024,
            ..Limits::for_tests()
        };
        let err = tiptap_to_yjs_update(&doc, &limits).expect_err("cap");
        assert!(matches!(
            err,
            EngineStatus::ResourceLimit {
                kind: LimitKind::Output,
                ..
            }
        ));
    }

    #[test]
    fn marked_emoji_atoms_are_refused_but_marked_unicode_and_diff_metadata_are_supported() {
        let input = json!({"type":"doc","content":[{"type":"paragraph","content":[
            {"type":"emoji","attrs":{"name":"grinning"},"marks":[{"type":"bold"}]}
        ]}]});
        assert!(matches!(
            tiptap_to_yjs_update(&input, &Limits::for_tests()),
            Err(EngineStatus::Malformed { .. })
        ));
        let unicode = json!({"type":"doc","content":[{"type":"paragraph","content":[
            {"type":"text","text":"😀한글","marks":[{"type":"bold"}]},
            {"type":"emoji","attrs":{"name":"grinning"},"marks":[{"type":"ychange"}]}
        ]}]});
        let observed = project(&seed(&unicode));
        assert_eq!(observed["content"][0]["content"][0]["text"], "😀한글");
        assert_eq!(
            observed["content"][0]["content"][0]["marks"][0]["type"],
            "bold"
        );
        assert_eq!(observed["content"][0]["content"][1]["type"], "emoji");
    }
}
