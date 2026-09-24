//! Bounded y-tiptap 3.0.9 `yXmlFragmentToProsemirrorJSON` + source `withoutYChange`.
//!
//! Traversal matches installed `@tiptap/y-tiptap` 3.0.9
//! `yXmlFragmentToProsemirrorJSON` and FVOCI source SHA
//! `393795261322b916e588043cf94feca999175843` `packages/editor/src/collab-tiptap.ts`
//! over Yrs XmlFragment / XmlText delta, never XML strings.
//!
//! y-tiptap emits marks in Y.Text format-item order (`Object.keys` of
//! `currentAttributes`). yrs 0.28 does not expose that item chain, so Project
//! emits marks sorted by raw attribute name. ProseMirror re-ranks marks on
//! load; JSON-level mark-array order may differ from the JS oracle. Typed mark
//! contents are preserved. This is not exact raw JS JSON.

use std::io::{self, Write};

use serde_json::{Map, Value};
use yrs::any::Number;
use yrs::types::text::YChange;
use yrs::{Any, Out, ReadTxn, Text, Xml, XmlElementRef, XmlFragment, XmlOut, XmlTextRef};

use crate::limits::Limits;
use crate::outcome::{EngineStatus, LimitKind};

pub fn project_prosemirror<T: ReadTxn>(txn: &T, limits: &Limits) -> Result<Value, EngineStatus> {
    let mut budget = Budget::new(limits);
    let json = match txn.get_xml_fragment(crate::FRAGMENT) {
        Some(frag) => y_xml_fragment_to_prosemirror_json(txn, &frag, &mut budget)?,
        None => empty_doc(),
    };
    let json = without_ychange(&json);
    bound_json_bytes(&json, limits.max_project_json_bytes)?;
    Ok(json)
}

fn empty_doc() -> Value {
    serde_json::json!({ "type": "doc", "content": [] })
}

struct Budget {
    max_depth: u32,
    max_nodes: u32,
    max_string: u64,
    max_json: u64,
    nodes: u32,
    strings: u64,
}

impl Budget {
    fn new(limits: &Limits) -> Self {
        Self {
            max_depth: limits.max_project_depth,
            max_nodes: limits.max_project_nodes,
            max_string: limits.max_project_string_bytes,
            max_json: limits.max_project_json_bytes,
            nodes: 0,
            strings: 0,
        }
    }

    fn add_node(&mut self) -> Result<(), EngineStatus> {
        if self.nodes >= self.max_nodes {
            return Err(EngineStatus::ResourceLimit {
                kind: LimitKind::Memory,
                detail: format!(
                    "project node count {} reached max {}",
                    self.nodes, self.max_nodes
                ),
            });
        }
        self.nodes += 1;
        Ok(())
    }

    fn add_string(&mut self, s: &str, what: &str) -> Result<(), EngineStatus> {
        let n = s.len() as u64;
        if n > self.max_string {
            return Err(output_limit(what, n, self.max_string));
        }
        self.strings = self.strings.saturating_add(n);
        if self.strings > self.max_json {
            return Err(output_limit(
                "project string total",
                self.strings,
                self.max_json,
            ));
        }
        Ok(())
    }

    fn check_depth(&self, depth: u32) -> Result<(), EngineStatus> {
        if depth > self.max_depth {
            return Err(EngineStatus::ResourceLimit {
                kind: LimitKind::Stack,
                detail: format!("project nesting {depth} exceeds max {}", self.max_depth),
            });
        }
        Ok(())
    }

    /// Refuse before `Vec`/`Map` allocation when `extra` nested values cannot fit.
    fn ensure_nodes(&self, extra: usize) -> Result<(), EngineStatus> {
        let extra = u32::try_from(extra).unwrap_or(u32::MAX);
        if extra > self.max_nodes.saturating_sub(self.nodes) {
            return Err(EngineStatus::ResourceLimit {
                kind: LimitKind::Memory,
                detail: format!(
                    "project node count {} reached max {}",
                    self.nodes, self.max_nodes
                ),
            });
        }
        Ok(())
    }
}

fn output_limit(what: &str, len: u64, max: u64) -> EngineStatus {
    EngineStatus::ResourceLimit {
        kind: LimitKind::Output,
        detail: format!("{what} {len} bytes exceeds {max}-byte project json limit"),
    }
}

/// Serialize with a counting writer so an oversized document never gets a full output buffer.
fn bound_json_bytes(json: &Value, max: u64) -> Result<(), EngineStatus> {
    let mut writer = CountingWriter {
        written: 0,
        max,
        hit_cap: false,
    };
    match serde_json::to_writer(&mut writer, json) {
        Ok(()) => Ok(()),
        Err(_) if writer.hit_cap => Err(output_limit(
            "content_json",
            writer.written.saturating_add(1),
            max,
        )),
        Err(_) => Err(EngineStatus::Malformed {
            detail: "project json encode".into(),
        }),
    }
}

struct CountingWriter {
    written: u64,
    max: u64,
    hit_cap: bool,
}

impl Write for CountingWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = buf.len() as u64;
        match self.written.checked_add(n) {
            Some(total) if total <= self.max => {
                self.written = total;
                Ok(buf.len())
            }
            _ => {
                self.hit_cap = true;
                Err(io::Error::new(io::ErrorKind::WriteZero, "project json cap"))
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// y-tiptap 3.0.9 `yXmlFragmentToProsemirrorJSON`.
fn y_xml_fragment_to_prosemirror_json<T: ReadTxn>(
    txn: &T,
    frag: &yrs::XmlFragmentRef,
    budget: &mut Budget,
) -> Result<Value, EngineStatus> {
    let mut content = Vec::new();
    let mut yielded = 0u32;
    for child in frag.children(txn) {
        yielded = yielded.saturating_add(1);
        match serialize(txn, child, 1, budget)? {
            Serialized::Node(v) => content.push(v),
            Serialized::Nodes(vs) => content.push(Value::Array(vs)),
        }
    }
    reject_truncated_xml_children(yielded, frag.len(txn), "fragment")?;
    Ok(serde_json::json!({ "type": "doc", "content": content }))
}

enum Serialized {
    Node(Value),
    Nodes(Vec<Value>),
}

fn serialize<T: ReadTxn>(
    txn: &T,
    item: XmlOut,
    depth: u32,
    budget: &mut Budget,
) -> Result<Serialized, EngineStatus> {
    match item {
        XmlOut::Text(text) => Ok(Serialized::Nodes(serialize_xml_text(
            txn, &text, depth, budget,
        )?)),
        XmlOut::Element(el) => Ok(Serialized::Node(serialize_xml_element(
            txn, &el, depth, budget,
        )?)),
        XmlOut::Fragment(_) => Err(EngineStatus::Malformed {
            detail: "project: nested XmlFragment is unsupported".into(),
        }),
    }
}

fn serialize_xml_element<T: ReadTxn>(
    txn: &T,
    el: &XmlElementRef,
    depth: u32,
    budget: &mut Budget,
) -> Result<Value, EngineStatus> {
    budget.check_depth(depth)?;
    budget.add_node()?;
    let tag = el
        .try_tag()
        .map(|t| t.as_ref().to_string())
        .ok_or_else(|| EngineStatus::Malformed {
            detail: "project: XmlElement missing tag".into(),
        })?;
    budget.add_string(&tag, "node type")?;

    let mut obj = Map::new();
    obj.insert("type".into(), Value::String(tag.clone()));

    let mut attrs = Map::new();
    for (key, value) in el.attributes(txn) {
        budget.add_string(key, "attr name")?;
        if matches!(&value, Out::Any(Any::Undefined)) {
            continue;
        }
        let json = out_to_json(value, depth.saturating_add(1), budget)?;
        attrs.insert(key.to_string(), json);
    }
    if !attrs.is_empty() {
        obj.insert("attrs".into(), Value::Object(attrs));
    }

    let mut content = Vec::new();
    let mut yielded = 0u32;
    for child in el.children(txn) {
        yielded = yielded.saturating_add(1);
        match serialize(txn, child, depth.saturating_add(1), budget)? {
            Serialized::Node(v) => content.push(v),
            Serialized::Nodes(vs) => content.extend(vs),
        }
    }
    reject_truncated_xml_children(yielded, el.len(txn), &tag)?;
    if !content.is_empty() {
        obj.insert("content".into(), Value::Array(content));
    }
    Ok(Value::Object(obj))
}

/// yrs `XmlNodes` ends at the first non-XML child (`try_from(...).ok()`),
/// dropping that child and every later sibling. y-tiptap throws instead.
fn reject_truncated_xml_children(
    yielded: u32,
    declared: u32,
    where_: &str,
) -> Result<(), EngineStatus> {
    if yielded == declared {
        return Ok(());
    }
    Err(EngineStatus::Malformed {
        detail: format!("project: non-XML child in {where_}"),
    })
}

fn serialize_xml_text<T: ReadTxn>(
    txn: &T,
    text: &XmlTextRef,
    depth: u32,
    budget: &mut Budget,
) -> Result<Vec<Value>, EngineStatus> {
    budget.check_depth(depth)?;
    let mut nodes = Vec::new();
    // `YChange` stays on `Diff.ychange`; y-tiptap `toDelta()` has no snapshot, so ignore it.
    for diff in text.diff(txn, YChange::identity) {
        budget.add_node()?;
        let insert = match diff.insert {
            Out::Any(Any::String(s)) => s.to_string(),
            other => {
                return Err(EngineStatus::Malformed {
                    detail: format!(
                        "project: XmlText insert is not a string ({})",
                        out_kind(&other)
                    ),
                });
            }
        };
        budget.add_string(&insert, "text")?;
        let mut node = Map::new();
        node.insert("type".into(), Value::String("text".into()));
        node.insert("text".into(), Value::String(insert));
        if let Some(attrs) = diff.attributes {
            // Yjs `toDelta()` (no snapshot) never puts the reserved key `ychange` on
            // attributes; y-tiptap therefore omits `marks` when that was the only attr.
            // Hashed `ychange--xxxxxxxx` is a real mark name and is kept.
            let mut mark_attrs: Vec<_> = attrs
                .iter()
                .filter(|(key, _)| key.as_ref() != "ychange")
                .collect();
            // yrs 0.28 `Attrs` is a HashMap; iteration is not Y.Text format-item
            // order. Sort by raw attribute key (bytes) before `yattr2markname`.
            mark_attrs.sort_by(|a, b| a.0.as_ref().cmp(b.0.as_ref()));
            if !mark_attrs.is_empty() {
                budget.ensure_nodes(mark_attrs.len())?;
                let mut marks = Vec::with_capacity(mark_attrs.len());
                for (key, value) in mark_attrs {
                    budget.add_node()?;
                    budget.add_string(key.as_ref(), "mark type")?;
                    let type_name = yattr2markname(key.as_ref());
                    let mut mark = Map::new();
                    mark.insert("type".into(), Value::String(type_name.to_string()));
                    // y-tiptap: `if (Object.keys(attrs)) { mark.attrs = attrs; }` is always
                    // true for objects/arrays/boxed primitives, including `{}`.
                    mark.insert(
                        "attrs".into(),
                        any_to_json(value, depth.saturating_add(1), budget)?,
                    );
                    marks.push(Value::Object(mark));
                }
                node.insert("marks".into(), Value::Array(marks));
            }
        }
        nodes.push(Value::Object(node));
    }
    Ok(nodes)
}

/// y-tiptap `hashedMarkNameRegex = /(.*)(--[a-zA-Z0-9+/=]{8})$/`
pub fn yattr2markname(attr_name: &str) -> &str {
    let bytes = attr_name.as_bytes();
    if bytes.len() < 10 {
        return attr_name;
    }
    let split = bytes.len() - 10;
    if bytes[split] != b'-' || bytes[split + 1] != b'-' {
        return attr_name;
    }
    let suffix = &bytes[split + 2..];
    if suffix.len() == 8
        && suffix
            .iter()
            .all(|c| c.is_ascii_alphanumeric() || *c == b'+' || *c == b'/' || *c == b'=')
    {
        return &attr_name[..split];
    }
    attr_name
}

fn out_kind(value: &Out) -> &'static str {
    match value {
        Out::Any(Any::Null) => "null",
        Out::Any(Any::Undefined) => "undefined",
        Out::Any(Any::Bool(_)) => "bool",
        Out::Any(Any::Number(_)) => "number",
        Out::Any(Any::String(_)) => "string",
        Out::Any(Any::Buffer(_)) => "buffer",
        Out::Any(Any::Array(_)) => "array",
        Out::Any(Any::Map(_)) => "map",
        Out::YText(_) => "text",
        Out::YArray(_) => "array-ref",
        Out::YMap(_) => "map-ref",
        Out::YXmlElement(_) => "xml-element",
        Out::YXmlFragment(_) => "xml-fragment",
        Out::YXmlText(_) => "xml-text",
        Out::YDoc(_) => "doc",
        Out::UndefinedRef(_) => "undefined-ref",
    }
}

fn out_to_json(value: Out, depth: u32, budget: &mut Budget) -> Result<Value, EngineStatus> {
    match value {
        Out::Any(any) => any_to_json(&any, depth, budget),
        other => Err(EngineStatus::Malformed {
            detail: format!("project: non-JSON XML attribute ({})", out_kind(&other)),
        }),
    }
}

fn any_to_json(any: &Any, depth: u32, budget: &mut Budget) -> Result<Value, EngineStatus> {
    budget.check_depth(depth)?;
    match any {
        Any::Undefined => Err(EngineStatus::Malformed {
            detail: "project: undefined JSON value".into(),
        }),
        Any::Buffer(_) => Err(EngineStatus::Malformed {
            detail: "project: binary attribute is unsupported".into(),
        }),
        Any::Null => {
            budget.add_node()?;
            Ok(Value::Null)
        }
        Any::Bool(b) => {
            budget.add_node()?;
            Ok(Value::Bool(*b))
        }
        Any::Number(n) => {
            budget.add_node()?;
            number_to_json(*n)
        }
        Any::String(s) => {
            budget.add_node()?;
            budget.add_string(s, "json string")?;
            Ok(Value::String(s.to_string()))
        }
        Any::Array(items) => {
            budget.add_node()?;
            budget.ensure_nodes(items.len())?;
            let mut out = Vec::with_capacity(items.len());
            let child_depth = depth.saturating_add(1);
            for item in items.iter() {
                out.push(any_to_json(item, child_depth, budget)?);
            }
            Ok(Value::Array(out))
        }
        Any::Map(entries) => {
            budget.add_node()?;
            let defined = entries
                .values()
                .filter(|v| !matches!(v, Any::Undefined))
                .count();
            budget.ensure_nodes(defined)?;
            let mut map = Map::new();
            let child_depth = depth.saturating_add(1);
            for (k, v) in entries.iter() {
                if matches!(v, Any::Undefined) {
                    continue;
                }
                budget.add_string(k, "json key")?;
                map.insert(k.clone(), any_to_json(v, child_depth, budget)?);
            }
            Ok(Value::Object(map))
        }
    }
}

fn number_to_json(n: Number) -> Result<Value, EngineStatus> {
    match n.as_i64() {
        Some(i) => Ok(Value::Number(i.into())),
        None => {
            let f = n.as_f64().ok_or_else(|| EngineStatus::Malformed {
                detail: "project: number is not JSON".into(),
            })?;
            serde_json::Number::from_f64(f)
                .map(Value::Number)
                .ok_or_else(|| EngineStatus::Malformed {
                    detail: "project: non-finite number".into(),
                })
        }
    }
}

/// Source `withoutYChange`: drop `ychange` keys; filter marks with `type === "ychange"`
/// without recursing into survivors (nested ychange in retained mark attrs stays).
/// Empty `marks: []` is retained.
fn without_ychange(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(without_ychange).collect()),
        Value::Object(obj) => {
            let mut out = Map::new();
            for (key, child) in obj {
                if key == "ychange" {
                    continue;
                }
                if key == "marks" {
                    if let Value::Array(marks) = child {
                        let kept: Vec<Value> = marks
                            .iter()
                            .filter(|mark| {
                                !mark.as_object().is_some_and(|m| {
                                    m.get("type").and_then(Value::as_str) == Some("ychange")
                                })
                            })
                            .cloned()
                            .collect();
                        out.insert(key.clone(), Value::Array(kept));
                        continue;
                    }
                }
                out.insert(key.clone(), without_ychange(child));
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::yattr2markname;
    use serde_json::json;

    #[test]
    fn hashed_mark_suffix_is_stripped() {
        assert_eq!(yattr2markname("link"), "link");
        assert_eq!(yattr2markname("bold--abcdEFG="), "bold");
        assert_eq!(yattr2markname("link--////++++"), "link");
        assert_eq!(yattr2markname("ychange--abcd1234"), "ychange");
        assert_eq!(yattr2markname("short--ab"), "short--ab");
    }

    #[test]
    fn without_ychange_strips_attr_and_mark() {
        let raw = json!({
            "type": "paragraph",
            "attrs": { "id": "p", "ychange": { "type": "added" } },
            "content": [{
                "type": "text",
                "text": "ab",
                "marks": [
                    { "type": "ychange", "attrs": { "type": "added" } },
                    { "type": "bold", "attrs": {} }
                ]
            }]
        });
        let got = super::without_ychange(&raw);
        assert_eq!(
            got,
            json!({
                "type": "paragraph",
                "attrs": { "id": "p" },
                "content": [{
                    "type": "text",
                    "text": "ab",
                    "marks": [{ "type": "bold", "attrs": {} }]
                }]
            })
        );
    }

    #[test]
    fn without_ychange_retains_empty_marks() {
        let raw = json!({
            "type": "text",
            "text": "원문",
            "marks": [{ "type": "ychange", "attrs": { "type": "added" } }]
        });
        assert_eq!(
            super::without_ychange(&raw),
            json!({ "type": "text", "text": "원문", "marks": [] })
        );
    }

    #[test]
    fn without_ychange_keeps_nested_ychange_in_surviving_marks() {
        let raw = json!({
            "type": "text",
            "text": "한글",
            "marks": [{
                "type": "link",
                "attrs": { "href": "https://x.invalid", "ychange": { "type": "nested" } }
            }]
        });
        assert_eq!(super::without_ychange(&raw), raw);
    }

    #[test]
    fn any_to_json_budgets_numeric_array_before_alloc() {
        let items: Vec<yrs::Any> = (0..32).map(|_| yrs::Any::from(1)).collect();
        let any = yrs::Any::from(items);
        let mut limits = crate::limits::Limits::for_tests();
        limits.max_project_nodes = 4;
        let mut budget = super::Budget::new(&limits);
        let err = super::any_to_json(&any, 1, &mut budget).expect_err("node cap");
        assert!(
            matches!(
                err,
                crate::outcome::EngineStatus::ResourceLimit {
                    kind: crate::outcome::LimitKind::Memory,
                    ..
                }
            ),
            "{err:?}"
        );
    }

    #[test]
    fn any_to_json_budgets_bool_array_and_deep_map() {
        let bools: Vec<yrs::Any> = (0..8).map(|_| yrs::Any::from(true)).collect();
        let mut limits = crate::limits::Limits::for_tests();
        limits.max_project_nodes = 3;
        let mut budget = super::Budget::new(&limits);
        let err = super::any_to_json(&yrs::Any::from(bools), 1, &mut budget)
            .expect_err("bool array node cap");
        assert!(matches!(
            err,
            crate::outcome::EngineStatus::ResourceLimit {
                kind: crate::outcome::LimitKind::Memory,
                ..
            }
        ));

        let mut inner = std::collections::HashMap::new();
        inner.insert("k".into(), yrs::Any::from(1));
        let mut nested = yrs::Any::Map(std::sync::Arc::new(inner));
        for _ in 0..6 {
            let mut map = std::collections::HashMap::new();
            map.insert("n".into(), nested);
            nested = yrs::Any::Map(std::sync::Arc::new(map));
        }
        let mut shallow = crate::limits::Limits::for_tests();
        shallow.max_project_depth = 3;
        let mut budget = super::Budget::new(&shallow);
        let err = super::any_to_json(&nested, 1, &mut budget).expect_err("map depth");
        assert!(matches!(
            err,
            crate::outcome::EngineStatus::ResourceLimit {
                kind: crate::outcome::LimitKind::Stack,
                ..
            }
        ));
    }

    #[test]
    fn bound_json_bytes_fails_before_storing_output() {
        let json = json!({ "type": "doc", "content": [] });
        let err = super::bound_json_bytes(&json, 8).expect_err("cap");
        assert!(matches!(
            err,
            crate::outcome::EngineStatus::ResourceLimit {
                kind: crate::outcome::LimitKind::Output,
                ..
            }
        ));
        super::bound_json_bytes(&json, 10_000).expect("fits");
    }

    #[test]
    fn truncated_xml_children_are_malformed() {
        super::reject_truncated_xml_children(2, 2, "fragment").expect("match");
        let err = super::reject_truncated_xml_children(1, 3, "paragraph").expect_err("trunc");
        match err {
            crate::outcome::EngineStatus::Malformed { detail } => {
                assert!(detail.contains("non-XML child in paragraph"), "{detail}");
            }
            other => panic!("{other:?}"),
        }
    }
}
