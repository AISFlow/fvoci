//! Block-level body edit (source `packages/editor/src/extract.ts`
//! `replaceTiptapNodeById` at `393795261322b916e588043cf94feca999175843`).

use serde_json::{Map, Value};

/// Source `TIPTAP_WALK_MAX_DEPTH`: nodes deeper than this are not visited.
const TIPTAP_WALK_MAX_DEPTH: usize = 64;

/// Source `UNIQUE_ID_NODE_TYPES`: only these node types carry a block id.
pub const UNIQUE_ID_NODE_TYPES: &[&str] = &[
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

/// Replacement node of `PATCH …/blocks/{blockId}`.
#[derive(Debug, Clone)]
pub struct BlockNode {
    pub r#type: String,
    pub attrs: Option<Map<String, Value>>,
    pub content: Option<Vec<Value>>,
    pub marks: Option<Vec<Value>>,
    pub text: Option<String>,
}

/// Replaces the first (preorder) unique-id node whose `attrs.id` is `block_id`.
/// The replacement keeps `block_id` as its id. `None` when no node matches.
pub fn replace_node_by_id(doc: &Value, block_id: &str, node: &BlockNode) -> Option<Value> {
    let mut next = doc.clone();
    if replace_in(&mut next, block_id, node, 0) {
        Some(next)
    } else {
        None
    }
}

fn replace_in(value: &mut Value, block_id: &str, node: &BlockNode, depth: usize) -> bool {
    if depth > TIPTAP_WALK_MAX_DEPTH {
        return false;
    }
    let Some(obj) = value.as_object_mut() else {
        return false;
    };
    let id_matches = obj
        .get("attrs")
        .and_then(Value::as_object)
        .and_then(|attrs| attrs.get("id"))
        .and_then(Value::as_str)
        == Some(block_id);
    let type_ok = obj
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|t| UNIQUE_ID_NODE_TYPES.contains(&t));
    if id_matches && type_ok {
        apply(obj, block_id, node);
        return true;
    }
    if let Some(Value::Array(children)) = obj.get_mut("content") {
        for child in children {
            if replace_in(child, block_id, node, depth + 1) {
                return true;
            }
        }
    }
    false
}

fn apply(obj: &mut Map<String, Value>, block_id: &str, node: &BlockNode) {
    obj.insert("type".into(), Value::String(node.r#type.clone()));
    let mut attrs = node.attrs.clone().unwrap_or_default();
    attrs.insert("id".into(), Value::String(block_id.to_string()));
    obj.insert("attrs".into(), Value::Object(attrs));
    match &node.content {
        Some(content) => obj.insert("content".into(), Value::Array(content.clone())),
        None => obj.remove("content"),
    };
    match &node.text {
        Some(text) => obj.insert("text".into(), Value::String(text.clone())),
        None => obj.remove("text"),
    };
    match &node.marks {
        Some(marks) => obj.insert("marks".into(), Value::Array(marks.clone())),
        None => obj.remove("marks"),
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn para(id: &str, text: &str) -> Value {
        json!({"type":"paragraph","attrs":{"id":id},"content":[{"type":"text","text":text}]})
    }

    #[test]
    fn replaces_first_matching_block_and_keeps_id() {
        let doc = json!({"type":"doc","content":[para("a","하나"), para("b","둘")]});
        let node = BlockNode {
            r#type: "heading".into(),
            attrs: Some(serde_json::from_value(json!({"level":2,"id":"zzz"})).unwrap()),
            content: Some(vec![json!({"type":"text","text":"제목 😀"})]),
            marks: None,
            text: None,
        };
        let out = replace_node_by_id(&doc, "b", &node).unwrap();
        assert_eq!(
            out["content"][1],
            json!({"type":"heading","attrs":{"level":2,"id":"b"},"content":[{"type":"text","text":"제목 😀"}]})
        );
        assert_eq!(out["content"][0], doc["content"][0]);
    }

    #[test]
    fn missing_block_or_non_unique_type_is_none() {
        let doc = json!({"type":"doc","content":[
            {"type":"mention","attrs":{"id":"m"}},
            para("a","x"),
        ]});
        let node = BlockNode {
            r#type: "paragraph".into(),
            attrs: None,
            content: None,
            marks: None,
            text: None,
        };
        assert!(replace_node_by_id(&doc, "m", &node).is_none());
        assert!(replace_node_by_id(&doc, "nope", &node).is_none());
        let out = replace_node_by_id(&doc, "a", &node).unwrap();
        assert_eq!(
            out["content"][1],
            json!({"type":"paragraph","attrs":{"id":"a"}})
        );
    }

    #[test]
    fn nested_block_is_found_in_preorder() {
        let doc = json!({"type":"doc","content":[
            {"type":"blockquote","attrs":{"id":"q"},"content":[para("inner","x")]}
        ]});
        let node = BlockNode {
            r#type: "paragraph".into(),
            attrs: None,
            content: Some(vec![json!({"type":"text","text":"y"})]),
            marks: None,
            text: None,
        };
        let out = replace_node_by_id(&doc, "inner", &node).unwrap();
        assert_eq!(out["content"][0]["content"][0]["content"][0]["text"], "y");
    }
}
