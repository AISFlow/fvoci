//! Independent correctness checks: canonical bodies, every mark transition, and UTF-16.

mod support;

use fvoci_collab::testing::{repair, to_body, with_update};
use fvoci_collab::{from_body_by, new_doc, state_of};
use fvoci_model::{BODY_MAX_BYTES, Body};
use serde_json::{Value, json};
use support::{ROOMY, body_of, body_vectors, json_of, stored_form};

const MARKS: [&str; 5] = ["link", "bold", "italic", "strike", "code"];

fn marks(mask: u8, href: &str) -> Vec<Value> {
    MARKS
        .iter()
        .enumerate()
        .filter(|(bit, _)| mask & (1 << bit) != 0)
        .map(|(_, name)| match *name {
            "link" => json!({"type": name, "attrs": {"href": href}}),
            _ => json!({"type": name}),
        })
        .collect()
}

fn text(letters: &str, marks: &[Value]) -> Value {
    if marks.is_empty() {
        json!({"type": "text", "text": letters})
    } else {
        json!({"type": "text", "text": letters, "marks": marks})
    }
}

fn assert_round_trip(input: &Value, expected: &Value, context: &str) {
    let expected = &stored_form(expected);
    let doc = from_body_by(&body_of(input), 41);
    let direct = to_body(&doc).unwrap_or_else(|error| panic!("{context}: {error}"));
    assert_eq!(json_of(direct.as_ref()), *expected, "{context}: direct");
    let before = state_of(&doc);
    let travelled = with_update(&new_doc(), &before, &ROOMY).expect("a body's state applies");
    let back = to_body(&travelled).expect("a body after transport");
    assert_eq!(json_of(back.as_ref()), *expected, "{context}: transported");
    let unchanged = repair(&travelled).expect("a body needs no repair");
    assert_eq!(unchanged.deleted, 0, "{context}: no deletion");
    assert_eq!(unchanged.update, None, "{context}: no update");
    assert_eq!(unchanged.body, back, "{context}: repair keeps the body");
    assert_eq!(state_of(&travelled), before, "{context}: unchanged state");
    if let Some(back) = back {
        let rebuilt = from_body_by(&back, 42);
        assert_eq!(
            to_body(&rebuilt),
            Ok(Some(back)),
            "{context}: canonical fixed point"
        );
    }
}

#[test]
fn every_pair_of_mark_sets_survives_unicode_boundaries_and_changed_link_targets() {
    // Different targets prevent adjacent linked runs from being joined. The two
    // equal unlinked sets are joined in the canonical JSON, as in the web vectors.
    for first in 0..32 {
        for second in 0..32 {
            let a = marks(first, "/first/한글");
            let b = marks(second, "https://example.com/😀");
            let input = json!({"type": "doc", "content": [{"type": "paragraph", "content": [
                text("A😀e\u{301}", &a), text("𝒳한글👩‍👩‍👧‍👦Z", &b),
            ]}]});
            let expected = if a == b {
                json!({"type": "doc", "content": [{"type": "paragraph", "content": [
                    text("A😀e\u{301}𝒳한글👩‍👩‍👧‍👦Z", &a),
                ]}]})
            } else {
                input.clone()
            };
            assert_round_trip(&input, &expected, &format!("mark sets {first}/{second}"));
        }
    }
}

#[test]
fn unicode_in_every_string_attribute_and_mermaid_is_preserved() {
    let words = "😀e\u{301}\u{1112}\u{1161}\u{11ab}글𝒳👩‍👩‍👧‍👦\r\n\t\u{2028}\u{2029}";
    let input = json!({"type": "doc", "content": [
        {"type": "heading", "attrs": {"level": 3}, "content": [text(words, &[])]},
        {"type": "paragraph", "content": [
            {"type": "inlineMath", "attrs": {"latex": words}},
            {"type": "mention", "attrs": {"userId": "01930000-0000-7000-8000-00000000c002"}},
            {"type": "reference", "attrs": {"id": "01930000-0000-7000-8000-00000000b002"}},
            {"type": "hardBreak"},
            text(words, &marks(31, "https://example.com/😀e\u{301}")),
        ]},
        {"type": "blockMath", "attrs": {"latex": words}},
        {"type": "image", "attrs": {"fileId": "unicode-image.png", "alt": words}},
        {"type": "file", "attrs": {"fileId": "unicode-doc.pdf"}},
        {"type": "codeBlock", "attrs": {"language": "mermaid"}, "content": [
            text("graph TD\n A[\"😀e\u{301}\"]-->B[\"한글\"]", &[]),
        ]},
    ]});
    assert_round_trip(&input, &input, "Unicode in text and attributes");

    for (kind, file_id) in [("image", "😀e\u{301}.png"), ("file", "😀e\u{301}.pdf")] {
        let rejected = json!({"type": "doc", "content": [
            {"type": kind, "attrs": {"fileId": file_id}},
        ]});
        assert!(
            Body::parse(rejected).is_err(),
            "non-ASCII {kind} fileId must be rejected by strict ASCII allowlist"
        );
    }
}

fn collect_kinds(node: &Value, nodes: &mut Vec<String>, marks: &mut Vec<String>) {
    if let Some(kind) = node["type"].as_str() {
        nodes.push(kind.to_owned());
    }
    if let Some(given) = node["marks"].as_array() {
        marks.extend(
            given
                .iter()
                .filter_map(|mark| mark["type"].as_str().map(str::to_owned)),
        );
    }
    if let Some(children) = node["content"].as_array() {
        for child in children {
            collect_kinds(child, nodes, marks);
        }
    }
}

#[test]
fn binding_vectors_exercise_every_allowlisted_node_and_mark() {
    let mut nodes = Vec::new();
    let mut marks = Vec::new();
    for vector in body_vectors() {
        collect_kinds(&vector.body, &mut nodes, &mut marks);
        assert_round_trip(&vector.body, &vector.vector.json, &vector.vector.name);
    }
    nodes.sort_unstable();
    nodes.dedup();
    marks.sort_unstable();
    marks.dedup();
    assert_eq!(
        nodes,
        [
            "blockMath",
            "blockquote",
            "boardView",
            "bulletList",
            "callout",
            "codeBlock",
            "details",
            "detailsContent",
            "detailsSummary",
            "doc",
            "file",
            "hardBreak",
            "heading",
            "horizontalRule",
            "image",
            "inlineMath",
            "listItem",
            "mention",
            "orderedList",
            "paragraph",
            "reference",
            "table",
            "tableCell",
            "tableHeader",
            "tableRow",
            "taskItem",
            "taskList",
            "text",
        ]
    );
    assert_eq!(marks, ["bold", "code", "italic", "link", "strike"]);
}

#[test]
fn an_accepted_body_at_the_byte_limit_stays_valid_when_list_defaults_are_filled() {
    let document = |letters: &str| {
        json!({"type": "doc", "content": [
            {"type": "orderedList", "content": [
                {"type": "listItem", "content": [{"type": "paragraph"}]},
            ]},
            {"type": "paragraph", "content": [text(letters, &[])]},
        ]})
    };
    let frame = serde_json::to_vec(&document("")).expect("JSON").len();
    let input = document(&"x".repeat(BODY_MAX_BYTES - frame));
    assert_eq!(
        serde_json::to_vec(&input).expect("JSON").len(),
        BODY_MAX_BYTES
    );
    let mut canonical = input.clone();
    canonical["content"][0]["attrs"] = json!({"start": 1});
    assert_eq!(
        serde_json::to_vec(&canonical).expect("JSON").len(),
        BODY_MAX_BYTES + 20,
        "filling the list's default adds twenty bytes",
    );
    let fits = document(&"x".repeat(BODY_MAX_BYTES - frame - 20));
    assert!(
        to_body(&from_body_by(&body_of(&fits), 41)).is_ok(),
        "with twenty bytes of headroom the same structure round-trips",
    );
    let body = Body::parse(input)
        .expect("accepted at the byte limit")
        .expect("nonempty");
    let doc = from_body_by(&body, 41);
    let projected = to_body(&doc);
    let repaired = repair(&doc);
    assert!(
        projected.is_ok(),
        "an accepted body must stay valid after opening: projection {:?}, repair {:?}",
        projected.err(),
        repaired.err(),
    );
}

#[test]
fn extra_binding_vectors_agree_in_both_directions() {
    let vectors: Value = serde_json::from_str(include_str!("data/review-binding.json"))
        .expect("the extra binding fixture is JSON");
    let mut made = Vec::new();
    for case in vectors["cases"].as_array().expect("binding cases") {
        let name = case["name"].as_str().expect("a case name");
        assert_round_trip(&case["body"], &case["json"], name);
        let update = support::from_base64(case["update"].as_str().expect("an update"));
        let doc = with_update(&new_doc(), &update, &ROOMY).expect("a normal binding document");
        let body = to_body(&doc).unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(
            json_of(body.as_ref()),
            stored_form(&case["json"]),
            "{name}: yjs to Rust"
        );
        let ours = from_body_by(&body_of(&case["body"]), 1);
        made.push(json!({"name": name, "update": support::to_base64(&state_of(&ours))}));
    }
    // Optional artifact for the JS checker to read through the actual binding.
    // Kept outside tracked source when used during review.
    if let Some(path) = std::env::var_os("FVOCI_REVIEW_BINDING_OUTPUT") {
        std::fs::write(path, serde_json::to_vec(&made).expect("JSON"))
            .expect("write the optional binding-check artifact");
    }
}

#[test]
fn null_attributes_are_distinguished_from_absent_and_empty_values() {
    use fvoci_collab::FIELD;
    use yrs::{Any, Transact, Xml, XmlElementPrelim, XmlFragment};

    // Null attributes are not accepted stored JSON. In shared content, the editor
    // reads null as absent: optional strings disappear; required values still fail.
    for (kind, attribute, value) in [
        ("image", "alt", json!(null)),
        ("image", "alt", json!("")),
        ("codeBlock", "language", json!(null)),
        ("heading", "level", json!(null)),
        ("taskItem", "checked", json!(null)),
    ] {
        let mut node = json!({"type": kind, "attrs": {attribute: value}});
        if kind == "image" {
            node["attrs"]["fileId"] = json!("file.png");
        }
        if kind == "taskItem" {
            node["content"] = json!([{"type": "paragraph"}]);
            node = json!({"type": "taskList", "content": [node]});
        }
        let input = json!({"type": "doc", "content": [node]});
        assert_eq!(
            Body::parse(input).is_ok(),
            !value.is_null(),
            "{kind}/{attribute}"
        );
    }

    for value in [None, Some(Any::Null), Some(Any::from(""))] {
        let doc = new_doc();
        {
            let root = doc.get_or_insert_xml_fragment(FIELD);
            let mut txn = doc.transact_mut();
            let image = root.push_back(&mut txn, XmlElementPrelim::empty("image"));
            image.insert_attribute(&mut txn, "fileId", "file.png");
            if let Some(value) = value.clone() {
                image.insert_attribute(&mut txn, "alt", value);
            }
        }
        let mut expected = json!({"type": "doc", "content": [
            {"type": "image", "attrs": {"fileId": "file.png"}},
        ]});
        if value == Some(Any::from("")) {
            expected["content"][0]["attrs"]["alt"] = json!("");
        }
        assert_eq!(
            json_of(to_body(&doc).expect("valid optional attr").as_ref()),
            expected
        );
        let fixed = repair(&doc).expect("no repair needed");
        assert_eq!(fixed.deleted, 0);
    }
}

#[test]
fn every_allowed_unicode_scalar_survives_a_marked_round_trip() {
    // U+0000 is explicitly forbidden by Body; unpaired surrogates are not Unicode
    // scalar values and cannot occur in a Rust/serde_json string. Chunk the rest so
    // every individual body stays well below its byte limit.
    let mut scalars = (1..=0x10_ffff).filter_map(char::from_u32);
    let mut chunk = 0_u32;
    loop {
        let letters: String = scalars.by_ref().take(16_384).collect();
        if letters.is_empty() {
            break;
        }
        let input = json!({"type": "doc", "content": [{"type": "paragraph", "content": [
            text(&letters, &marks(31, "/unicode")),
            {"type": "hardBreak"},
            text("😀e\u{301}", &[]),
        ]}]});
        assert_round_trip(&input, &input, &format!("Unicode scalar chunk {chunk}"));
        chunk += 1;
    }
    assert_eq!(chunk, 68, "all non-NUL Unicode scalars were exercised");
}
