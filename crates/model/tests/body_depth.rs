//! The deepest body is one the JSON reader takes: `serde_json` reads 127 levels of
//! arrays and objects and fails at the next, and a body that could be stored and not
//! read back would be lost to every route that answers it.

use fvoci_model::{BODY_MAX_DEPTH, Body};
use serde_json::{Value, json};

/// The levels of JSON `serde_json` reads.
const READ: usize = 127;

/// The levels a body is wrapped in where it is wrapped deepest: an answer that holds
/// a list of blocks (`{"blocks": [{"body": …}]}`), with room for two more.
const WRAPPED: usize = 5;

/// A body as deep as one goes whose deepest node takes the most levels of JSON: a
/// text with a mark that has attributes.
fn deepest() -> Value {
    let link = json!({"type": "link", "attrs": {"href": "https://example.com/"}});
    let mut node = json!({"type": "paragraph", "content": [
        {"type": "text", "text": "deep", "marks": [link]},
    ]});
    // The document, the quotes, the paragraph and its text.
    for _ in 0..BODY_MAX_DEPTH - 3 {
        node = json!({"type": "blockquote", "content": [node]});
    }
    json!({"type": "doc", "content": [node]})
}

/// How many arrays and objects are open at the deepest place of a JSON text.
fn levels(text: &str) -> usize {
    let mut open = 0_usize;
    let mut most = 0;
    let mut quoted = false;
    let mut escaped = false;
    for byte in text.bytes() {
        match byte {
            _ if escaped => escaped = false,
            b'\\' if quoted => escaped = true,
            b'"' => quoted = !quoted,
            b'[' | b'{' if !quoted => {
                open += 1;
                most = most.max(open);
            }
            b']' | b'}' if !quoted => open -= 1,
            _ => {}
        }
    }
    most
}

#[test]
fn the_deepest_body_is_read_back_from_the_deepest_place_it_is_written_in() {
    let document = deepest();
    let body = Body::parse(document.clone())
        .expect("the deepest body is a body")
        .expect("and not an empty one");
    assert_eq!(body.document(), &document);
    assert!(
        Body::parse(json!({"type": "doc", "content": [
            {"type": "blockquote", "content": [document["content"][0].clone()]},
        ]}))
        .is_err(),
        "a level more is no body"
    );

    let stored = serde_json::to_string(&document).expect("JSON");
    assert_eq!(levels(&stored), 2 * BODY_MAX_DEPTH + 2);
    let mut wrapped = stored;
    for _ in 0..WRAPPED {
        wrapped = format!(r#"{{"in":{wrapped}}}"#);
    }
    assert!(levels(&wrapped) <= READ, "{} levels", levels(&wrapped));
    let read: Value = serde_json::from_str(&wrapped).expect("read back");
    let mut inner = &read;
    for _ in 0..WRAPPED {
        inner = &inner["in"];
    }
    assert_eq!(inner, &document);
}
