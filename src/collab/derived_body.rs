//! Bounded Tiptap body extraction for collab-derived `content_json` projection.
//!
//! Ports `packages/editor/src/extract.ts`, `emoji-glyph.ts`, and
//! `packages/core/src/body.ts` `prepareBodyUpdate` at source SHA
//! `393795261322b916e588043cf94feca999175843`.
//!
//! Internal reference extraction is implemented for parity testing only; the
//! references table is not wired — callers must not treat refs as persisted.
//!
//! Skip-if-equal in the DB boundary compares `content_json` only (source parity).
//! If text extraction changes while JSON is unchanged, `text`/`chosung` can
//! stay stale until the next projection that changes JSON.

use std::collections::HashMap;
use std::sync::OnceLock;

use serde::Deserialize;
use serde_json::Value;
use unicode_normalization::UnicodeNormalization;
use uuid::Uuid;

/// Product REST `DOCUMENT_MAX_BODY_BYTES` (1 MiB JSON), matching source default.
pub const DOCUMENT_MAX_BODY_BYTES: usize = 1024 * 1024;

/// Source `TIPTAP_WALK_MAX_DEPTH` from `extract.ts`.
const TIPTAP_WALK_MAX_DEPTH: u32 = 64;

const CHOSUNG: [&str; 19] = [
    "ㄱ", "ㄲ", "ㄴ", "ㄷ", "ㄸ", "ㄹ", "ㅁ", "ㅂ", "ㅃ", "ㅅ", "ㅆ", "ㅇ", "ㅈ", "ㅉ", "ㅊ", "ㅋ",
    "ㅌ", "ㅍ", "ㅎ",
];
const HANGUL_FIRST: u32 = 0xAC00;
const HANGUL_LAST: u32 = 0xD7A3;
const SYLLABLES_PER_CHOSUNG: u32 = 21 * 28;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DerivedBodyError {
    TooLarge,
    InvalidDocumentBody(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedDerivedBody {
    content_json: Value,
    text: String,
    chosung: String,
}

impl PreparedDerivedBody {
    pub fn content_json(&self) -> &Value {
        &self.content_json
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn chosung(&self) -> &str {
        &self.chosung
    }

    pub fn into_parts(self) -> (Value, String, String) {
        (self.content_json, self.text, self.chosung)
    }
}

/// Document/task block references extracted from Tiptap JSON. Not persisted in this slice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InternalRef {
    pub kind: InternalRefKind,
    pub id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InternalRefKind {
    Document,
    Task,
}

pub fn prepare_derived_body(content_json: Value) -> Result<PreparedDerivedBody, DerivedBodyError> {
    prepare_derived_body_with_cap(content_json, DOCUMENT_MAX_BODY_BYTES)
}

pub fn prepare_derived_body_with_cap(
    content_json: Value,
    max_bytes: usize,
) -> Result<PreparedDerivedBody, DerivedBodyError> {
    // Byte cap matches source `Buffer.byteLength(JSON.stringify(...))`. Depth is
    // bounded by the engine Project op; serde_json does not recurse here.
    let byte_length = serde_json::to_vec(&content_json)
        .map_err(|_| {
            DerivedBodyError::InvalidDocumentBody("contentJson could not be serialized".into())
        })?
        .len();
    if byte_length > max_bytes {
        return Err(DerivedBodyError::TooLarge);
    }
    if !is_tiptap_doc(&content_json) {
        return Err(DerivedBodyError::InvalidDocumentBody(
            "contentJson must be a Tiptap doc".into(),
        ));
    }
    let text = extract_text(&content_json).nfc().collect::<String>();
    let chosung = to_chosung(&text);
    Ok(PreparedDerivedBody {
        content_json,
        text,
        chosung,
    })
}

pub fn extract_internal_refs(root: &Value) -> Vec<InternalRef> {
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    walk_tiptap(root, 0, &mut |node| {
        let node_type = node.get("type").and_then(Value::as_str);
        let attrs = node.get("attrs").and_then(Value::as_object);
        match node_type {
            Some("mention") => {
                let entity = attrs.and_then(|a| a.get("entity")).and_then(Value::as_str);
                let id = attrs.and_then(|a| a.get("id")).and_then(Value::as_str);
                if let (Some(entity), Some(id)) = (entity, id) {
                    add_internal_ref(&mut seen, &mut out, entity, id);
                }
            }
            Some("embed") => {
                let entity = attrs.and_then(|a| a.get("entity")).and_then(Value::as_str);
                let id = attrs.and_then(|a| a.get("ref")).and_then(Value::as_str);
                if let (Some(entity), Some(id)) = (entity, id) {
                    add_internal_ref(&mut seen, &mut out, entity, id);
                }
            }
            _ => {}
        }
    });
    out
}

fn add_internal_ref(
    seen: &mut std::collections::BTreeSet<String>,
    out: &mut Vec<InternalRef>,
    entity: &str,
    id: &str,
) {
    if !is_uuid(id) {
        return;
    }
    let key = format!("{entity}:{id}");
    if seen.contains(&key) {
        return;
    }
    seen.insert(key);
    let kind = match entity {
        "document" => InternalRefKind::Document,
        "task" => InternalRefKind::Task,
        _ => return,
    };
    out.push(InternalRef {
        kind,
        id: id.to_string(),
    });
}

fn is_uuid(value: &str) -> bool {
    Uuid::parse_str(value).is_ok()
}

/// Matches `packages/editor/src/json.ts` `isTiptapDoc`.
fn is_tiptap_doc(value: &Value) -> bool {
    let Some(obj) = value.as_object() else {
        return false;
    };
    if obj.get("type").and_then(Value::as_str) != Some("doc") {
        return false;
    }
    match obj.get("content") {
        None => true,
        Some(Value::Array(_)) => true,
        Some(_) => false,
    }
}

#[derive(Debug, Deserialize)]
struct EmojiShortcodesFile {
    shortcodes: HashMap<String, String>,
}

fn emoji_shortcode_lookup() -> &'static HashMap<String, String> {
    static LOOKUP: OnceLock<HashMap<String, String>> = OnceLock::new();
    LOOKUP.get_or_init(|| {
        let raw = include_str!("emoji_shortcodes.json");
        serde_json::from_str::<EmojiShortcodesFile>(raw)
            .expect("emoji_shortcodes.json must parse")
            .shortcodes
    })
}

fn shortcode_to_emoji(name: &str) -> Option<&str> {
    emoji_shortcode_lookup().get(name).map(String::as_str)
}

pub fn extract_text(root: &Value) -> String {
    if let Some(content) = root.get("content").and_then(Value::as_array) {
        if root.get("type").and_then(Value::as_str) == Some("doc") {
            return content
                .iter()
                .map(|child| tiptap_text(child, 1))
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join("\n");
        }
    }
    tiptap_text(root, 0)
}

fn str_field(value: Option<&Value>) -> &str {
    value.and_then(Value::as_str).unwrap_or("")
}

fn tiptap_text(node: &Value, depth: u32) -> String {
    if depth > TIPTAP_WALK_MAX_DEPTH {
        return String::new();
    }
    if let Some(text) = node.as_str() {
        return text.to_string();
    }
    let obj = node.as_object();
    if obj.is_none() {
        return String::new();
    }
    let obj = obj.expect("checked");
    if let Some(text) = obj.get("text").and_then(Value::as_str) {
        return text.to_string();
    }
    let node_type = obj.get("type").and_then(Value::as_str);
    let attrs = obj.get("attrs");
    match node_type {
        Some("mention") => {
            let label = str_field(attrs.and_then(|a| a.get("label")));
            if !label.is_empty() {
                return label.to_string();
            }
            return str_field(attrs.and_then(|a| a.get("id"))).to_string();
        }
        Some("embed") => return str_field(attrs.and_then(|a| a.get("ref"))).to_string(),
        Some("mermaid") => return str_field(attrs.and_then(|a| a.get("source"))).to_string(),
        Some("math") | Some("mathInline") => {
            return str_field(attrs.and_then(|a| a.get("latex"))).to_string();
        }
        Some("attachment") => return str_field(attrs.and_then(|a| a.get("name"))).to_string(),
        Some("hardBreak") => return "\n".to_string(),
        Some("emoji") => return emoji_glyph(node),
        _ => {}
    }
    let content = obj.get("content").and_then(Value::as_array);
    if content.is_none() {
        return String::new();
    }
    let content = content.expect("checked");
    if node_type == Some("table") {
        return content
            .iter()
            .map(|row| {
                let cells = row
                    .get("content")
                    .and_then(Value::as_array)
                    .map(|c| c.as_slice())
                    .unwrap_or(&[]);
                cells
                    .iter()
                    .map(|cell| tiptap_text(cell, depth + 2))
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect::<Vec<_>>()
            .join("\n");
    }
    let parts = content
        .iter()
        .map(|child| tiptap_text(child, depth + 1))
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>();
    match node_type {
        Some("doc")
        | Some("blockquote")
        | Some("bulletList")
        | Some("orderedList")
        | Some("listItem")
        | Some("callout")
        | Some("details")
        | Some("detailsContent")
        | Some("taskList")
        | Some("taskItem") => parts.join("\n"),
        _ => parts.join(""),
    }
}

/// Source `packages/editor/src/emoji-glyph.ts` using pinned `@tiptap/extension-emoji`
/// shortcode data in `emoji_shortcodes.json`.
fn emoji_glyph(node: &Value) -> String {
    let attrs = node.get("attrs").and_then(Value::as_object);
    if let Some(glyph) = attrs.and_then(|a| a.get("emoji")).and_then(Value::as_str) {
        if !glyph.is_empty() {
            return glyph.to_string();
        }
    }
    let name = attrs
        .and_then(|a| a.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("");
    if name.is_empty() {
        return String::new();
    }
    shortcode_to_emoji(name)
        .map(str::to_string)
        .unwrap_or_else(|| format!(":{name}:"))
}

pub fn to_chosung(text: &str) -> String {
    text.chars()
        .map(|ch| {
            let cp = ch as u32;
            if !(HANGUL_FIRST..=HANGUL_LAST).contains(&cp) {
                return ch.to_string();
            }
            let idx = (cp - HANGUL_FIRST) / SYLLABLES_PER_CHOSUNG;
            CHOSUNG
                .get(idx as usize)
                .map(|s| s.to_string())
                .unwrap_or_else(|| ch.to_string())
        })
        .collect()
}

fn walk_tiptap(node: &Value, depth: u32, visit: &mut dyn FnMut(&Value)) {
    if depth > TIPTAP_WALK_MAX_DEPTH {
        return;
    }
    if !node.is_object() {
        return;
    }
    visit(node);
    if let Some(content) = node.get("content").and_then(Value::as_array) {
        for child in content {
            walk_tiptap(child, depth + 1, visit);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;
    use std::path::PathBuf;

    fn fixture(name: &str) -> Value {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/collab-derived")
            .join(name);
        let raw = fs::read_to_string(path).expect("fixture");
        serde_json::from_str(&raw).expect("json")
    }

    #[test]
    fn korean_chosung_matches_fixture() {
        let doc = fixture("korean_chosung.json");
        let prepared = prepare_derived_body(doc).expect("prepare");
        assert_eq!(prepared.text(), "한글 테스트");
        assert_eq!(prepared.chosung(), "ㅎㄱ ㅌㅅㅌ");
    }

    #[test]
    fn nfc_normalizes_decomposed_hangul() {
        let doc = json!({
            "type": "doc",
            "content": [{
                "type": "paragraph",
                "content": [{"type": "text", "text": "\u{1100}\u{1161}\u{11a8}"}]
            }]
        });
        let prepared = prepare_derived_body(doc).expect("prepare");
        assert_eq!(prepared.text(), "각");
    }

    #[test]
    fn emoji_uses_attrs_glyph_only() {
        let doc = fixture("emoji_attrs.json");
        let prepared = prepare_derived_body(doc).expect("prepare");
        assert_eq!(prepared.text(), "서버🎉");
    }

    #[test]
    fn emoji_shortcode_cases_match_js_oracle_fixture() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/collab-derived/emoji_glyph_expected.json");
        let raw = fs::read_to_string(path).expect("fixture");
        let cases: serde_json::Value = serde_json::from_str(&raw).expect("json");
        for case in cases["cases"].as_array().expect("cases") {
            let id = case["id"].as_str().expect("id");
            let node = &case["node"];
            let expected = case["expected"].as_str().expect("expected");
            assert_eq!(emoji_glyph(node), expected, "case {id}");
            if let Some(context) = case["context"].as_str() {
                let doc = json!({
                    "type": "doc",
                    "content": [{
                        "type": "paragraph",
                        "content": [
                            {"type": "text", "text": context},
                            node
                        ]
                    }]
                });
                let prepared = prepare_derived_body(doc).expect("prepare");
                assert_eq!(prepared.text(), format!("{context}{expected}"), "case {id}");
            }
        }
    }

    #[test]
    fn is_tiptap_doc_matches_source_json_contract() {
        assert!(is_tiptap_doc(&json!({"type": "doc"})));
        assert!(is_tiptap_doc(&json!({"type": "doc", "content": []})));
        assert!(!is_tiptap_doc(&json!([])));
        assert!(!is_tiptap_doc(&json!({"type": "doc", "content": {}})));
        assert!(!is_tiptap_doc(&json!({"type": "doc", "content": null})));
        assert!(!is_tiptap_doc(&json!("doc")));
        assert!(!is_tiptap_doc(&json!({"type":"paragraph"})));
    }

    #[test]
    fn prepare_rejects_malformed_doc_content_shapes() {
        for malformed in [
            json!({"type": "doc", "content": null}),
            json!({"type": "doc", "content": {}}),
            json!({"type": "doc", "content": "paragraph"}),
            json!([{"type": "paragraph"}]),
        ] {
            let err = prepare_derived_body(malformed).unwrap_err();
            assert_eq!(
                err,
                DerivedBodyError::InvalidDocumentBody("contentJson must be a Tiptap doc".into())
            );
        }
    }

    #[test]
    fn prepare_accepts_doc_with_missing_or_empty_content() {
        assert!(prepare_derived_body(json!({"type": "doc"})).is_ok());
        assert!(prepare_derived_body(json!({"type": "doc", "content": []})).is_ok());
    }

    #[test]
    fn rejects_non_doc_root() {
        let err = prepare_derived_body(json!({"type":"paragraph"})).unwrap_err();
        assert_eq!(
            err,
            DerivedBodyError::InvalidDocumentBody("contentJson must be a Tiptap doc".into())
        );
    }

    #[test]
    fn internal_ref_rejects_non_uuid_ids() {
        let doc = json!({
            "type": "doc",
            "content": [{
                "type": "paragraph",
                "content": [{
                    "type": "mention",
                    "attrs": {"entity": "document", "id": "not-a-uuid"}
                }]
            }]
        });
        assert!(extract_internal_refs(&doc).is_empty());
    }

    #[test]
    fn rejects_oversize_body() {
        let huge = "x".repeat(DOCUMENT_MAX_BODY_BYTES);
        let doc = json!({
            "type": "doc",
            "content": [{"type": "paragraph", "content": [{"type": "text", "text": huge}]}]
        });
        assert_eq!(
            prepare_derived_body(doc).unwrap_err(),
            DerivedBodyError::TooLarge
        );
    }

    #[test]
    fn internal_refs_extracted_but_not_persisted_gap_documented() {
        let doc = json!({
            "type": "doc",
            "content": [{
                "type": "paragraph",
                "content": [{
                    "type": "mention",
                    "attrs": {
                        "entity": "document",
                        "id": "550e8400-e29b-41d4-a716-446655440000"
                    }
                }]
            }]
        });
        let refs = extract_internal_refs(&doc);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].kind, InternalRefKind::Document);
        // Integration gap: DB boundary does not persist refs (no references table).
    }
}
