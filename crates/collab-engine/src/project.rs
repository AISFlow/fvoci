//! Bounded y-tiptap 3.0.9 `yXmlFragmentToProsemirrorJSON` + source `withoutYChange`.
//!
//! Exact algorithm from installed `@tiptap/y-tiptap` 3.0.9
//! `yXmlFragmentToProsemirrorJSON` and FVOCI source SHA
//! `393795261322b916e588043cf94feca999175843` `packages/editor/src/collab-tiptap.ts`.
//! Traversal is over Yrs XmlFragment / XmlText delta, never XML strings.

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
    let bytes = serde_json::to_vec(&json).map_err(|err| EngineStatus::Malformed {
        detail: format!("project json encode: {err}"),
    })?;
    if bytes.len() as u64 > limits.max_project_json_bytes {
        return Err(output_limit(
            "content_json",
            bytes.len() as u64,
            limits.max_project_json_bytes,
        ));
    }
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
}

fn output_limit(what: &str, len: u64, max: u64) -> EngineStatus {
    EngineStatus::ResourceLimit {
        kind: LimitKind::Output,
        detail: format!("{what} {len} bytes exceeds {max}-byte project json limit"),
    }
}

/// y-tiptap 3.0.9 `yXmlFragmentToProsemirrorJSON`.
fn y_xml_fragment_to_prosemirror_json<T: ReadTxn>(
    txn: &T,
    frag: &yrs::XmlFragmentRef,
    budget: &mut Budget,
) -> Result<Value, EngineStatus> {
    let mut content = Vec::new();
    for child in frag.children(txn) {
        match serialize(txn, child, 1, budget)? {
            Serialized::Node(v) => content.push(v),
            Serialized::Nodes(vs) => content.push(Value::Array(vs)),
        }
    }
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
        XmlOut::Text(text) => Ok(Serialized::Nodes(serialize_xml_text(txn, &text, budget)?)),
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
    obj.insert("type".into(), Value::String(tag));

    let mut attrs = Map::new();
    for (key, value) in el.attributes(txn) {
        budget.add_string(key, "attr name")?;
        if matches!(&value, Out::Any(Any::Undefined)) {
            continue;
        }
        let json = out_to_json(txn, value, budget)?;
        attrs.insert(key.to_string(), json);
    }
    if !attrs.is_empty() {
        obj.insert("attrs".into(), Value::Object(attrs));
    }

    let mut content = Vec::new();
    for child in el.children(txn) {
        match serialize(txn, child, depth.saturating_add(1), budget)? {
            Serialized::Node(v) => content.push(v),
            Serialized::Nodes(vs) => content.extend(vs),
        }
    }
    if !content.is_empty() {
        obj.insert("content".into(), Value::Array(content));
    }
    Ok(Value::Object(obj))
}

fn serialize_xml_text<T: ReadTxn>(
    txn: &T,
    text: &XmlTextRef,
    budget: &mut Budget,
) -> Result<Vec<Value>, EngineStatus> {
    let mut nodes = Vec::new();
    for diff in text.diff(txn, YChange::identity) {
        budget.add_node()?;
        let insert = match diff.insert {
            Out::Any(Any::String(s)) => s.to_string(),
            other => {
                return Err(EngineStatus::Malformed {
                    detail: format!("project: XmlText insert is not a string ({other:?})"),
                });
            }
        };
        budget.add_string(&insert, "text")?;
        let mut node = Map::new();
        node.insert("type".into(), Value::String("text".into()));
        node.insert("text".into(), Value::String(insert));
        if let Some(attrs) = diff.attributes {
            let mut marks = Vec::new();
            let mut keys: Vec<_> = attrs.keys().cloned().collect();
            keys.sort();
            for key in keys {
                let value = attrs.get(&key).expect("attrs key");
                budget.add_string(key.as_ref(), "mark type")?;
                let type_name = yattr2markname(key.as_ref());
                let mut mark = Map::new();
                mark.insert("type".into(), Value::String(type_name.to_string()));
                // y-tiptap: `if (Object.keys(attrs)) { mark.attrs = attrs; }` is always
                // true for objects/arrays/boxed primitives, including `{}`.
                mark.insert("attrs".into(), any_to_json(value, budget)?);
                marks.push(Value::Object(mark));
            }
            node.insert("marks".into(), Value::Array(marks));
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

fn out_to_json<T: ReadTxn>(
    _txn: &T,
    value: Out,
    budget: &mut Budget,
) -> Result<Value, EngineStatus> {
    match value {
        Out::Any(any) => any_to_json(&any, budget),
        other => Err(EngineStatus::Malformed {
            detail: format!("project: non-JSON XML attribute ({other:?})"),
        }),
    }
}

fn any_to_json(any: &Any, budget: &mut Budget) -> Result<Value, EngineStatus> {
    match any {
        Any::Null => Ok(Value::Null),
        Any::Undefined => Err(EngineStatus::Malformed {
            detail: "project: undefined JSON value".into(),
        }),
        Any::Bool(b) => Ok(Value::Bool(*b)),
        Any::Number(n) => number_to_json(*n),
        Any::String(s) => {
            budget.add_string(s, "json string")?;
            Ok(Value::String(s.to_string()))
        }
        Any::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items.iter() {
                out.push(any_to_json(item, budget)?);
            }
            Ok(Value::Array(out))
        }
        Any::Map(entries) => {
            let mut map = Map::new();
            for (k, v) in entries.iter() {
                if matches!(v, Any::Undefined) {
                    continue;
                }
                budget.add_string(k, "json key")?;
                map.insert(k.clone(), any_to_json(v, budget)?);
            }
            Ok(Value::Object(map))
        }
        Any::Buffer(_) => Err(EngineStatus::Malformed {
            detail: "project: binary attribute is unsupported".into(),
        }),
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

/// Source `withoutYChange`: drop `ychange` keys and marks with `type === "ychange"`.
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
                            .map(without_ychange)
                            .collect();
                        // y-tiptap omits `marks` when the run has no remaining
                        // attributes after ychange-only formatting is stripped.
                        if !kept.is_empty() {
                            out.insert(key.clone(), Value::Array(kept));
                        }
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
    fn without_ychange_omits_empty_marks() {
        let raw = json!({
            "type": "text",
            "text": "원문",
            "marks": [{ "type": "ychange", "attrs": { "type": "added" } }]
        });
        assert_eq!(
            super::without_ychange(&raw),
            json!({ "type": "text", "text": "원문" })
        );
    }
}
