//! The shared test vectors of `contracts/body-vectors.json`: every case is read as a
//! request's body would be, and must be accepted, taken for no body, or refused for the
//! reason the case names.

use std::path::Path;

use fvoci_model::{BODY_VERSION, Body, BodyError};
use serde_json::{Value, json};

/// The kind of a refusal, as the vectors name it.
fn reason(error: &BodyError) -> &'static str {
    match error {
        BodyError::NotADocument => "notDocument",
        BodyError::TooLarge => "tooLarge",
        BodyError::TooDeep => "tooDeep",
        BodyError::TooManyNodes => "tooManyNodes",
        BodyError::TableTooLarge { .. } => "tableTooLarge",
        BodyError::InvalidNode { .. } => "invalidNode",
    }
}

/// The document a case's `build` stands for, as the description of the vectors says.
fn build(recipe: &Value) -> Value {
    let size = |name: &str| recipe[name].as_u64().and_then(|n| usize::try_from(n).ok());
    if let (Some(rows), Some(columns)) = (size("rows"), size("columns")) {
        assert_eq!(
            recipe.as_object().map(serde_json::Map::len),
            Some(2),
            "{recipe}"
        );
        let cell = json!({"type": "tableCell", "content": [{"type": "paragraph"}]});
        let row = json!({"type": "tableRow", "content": vec![cell; columns]});
        return json!({"type": "doc", "content": [{"type": "table", "content": vec![row; rows]}]});
    }
    assert_eq!(
        recipe.as_object().map(serde_json::Map::len),
        Some(1),
        "a build names one size: {recipe}"
    );
    if let Some(depth) = size("depth") {
        let mut node = json!({"type": "paragraph"});
        for _ in 0..depth - 2 {
            node = json!({"type": "blockquote", "content": [node]});
        }
        return json!({"type": "doc", "content": [node]});
    }
    if let Some(nodes) = size("nodes") {
        let breaks = vec![json!({"type": "hardBreak"}); nodes - 2];
        return json!({"type": "doc", "content": [{"type": "paragraph", "content": breaks}]});
    }
    if let Some(bytes) = size("bytes") {
        let with = |text: String| {
            json!({"type": "doc", "content": [
                {"type": "paragraph", "content": [{"type": "text", "text": text}]},
            ]})
        };
        let frame = serde_json::to_vec(&with(String::new()))
            .expect("a document serializes")
            .len();
        let document = with("a".repeat(bytes - frame));
        assert_eq!(
            serde_json::to_vec(&document).expect("serializes").len(),
            bytes,
            "the built document has the size asked for"
        );
        return document;
    }
    panic!("an unknown build: {recipe}");
}

fn vectors() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../contracts/body-vectors.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

#[test]
fn the_shared_vectors_hold() {
    let vectors = vectors();
    assert_eq!(
        vectors["bodyVersion"],
        Value::from(BODY_VERSION),
        "the vectors are written for the allowlist this server checks"
    );
    let cases = vectors["cases"].as_array().expect("the cases are a list");
    assert!(!cases.is_empty(), "there are cases");
    for case in cases {
        let name = case["name"].as_str().expect("a case has a name");
        let sent = match (case.get("body"), case.get("build")) {
            (Some(body), None) => body.clone(),
            (None, Some(recipe)) => build(recipe),
            _ => panic!("{name}: a case has a body or a build, one of them"),
        };
        assert!(
            case.get("text").is_none() || case["expect"] == "accept",
            "{name}: only an accepted case has a text"
        );
        let got = Body::parse(sent.clone());
        match case["expect"].as_str() {
            Some("accept") => {
                let body = got
                    .unwrap_or_else(|error| panic!("{name}: refused ({error})"))
                    .unwrap_or_else(|| panic!("{name}: taken for no body"));
                assert_eq!(body.document(), &sent, "{name}: stored as sent");
                if let Some(text) = case.get("text") {
                    assert_eq!(&Value::from(body.text()), text, "{name}: its plain text");
                }
            }
            Some("empty") => {
                assert!(matches!(got, Ok(None)), "{name}: no body, not {got:?}");
            }
            Some("reject") => {
                let error = got.expect_err(name);
                assert_eq!(
                    Some(reason(&error)),
                    case["reason"].as_str(),
                    "{name}: {error}"
                );
            }
            other => panic!("{name}: unknown expectation {other:?}"),
        }
    }
}
