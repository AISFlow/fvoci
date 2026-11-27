//! An update that adds nodes after the last of a body's document
//! ([`BodyDoc::append_update`]): taken as any other, it leaves the body with the nodes
//! after what it had, marks and all, and what an editor typed meanwhile stays.

use fvoci_collab::yrs::updates::decoder::Decode as _;
use fvoci_collab::yrs::{
    Doc, GetString as _, ReadTxn as _, Text as _, Transact as _, Update, XmlFragment as _, XmlOut,
};
use fvoci_collab::{
    BodyDoc, FIELD, FIRST_APPENDER, MAX_CLIENT_CHANGES, MAX_CLIENTS, MAX_TOTAL_CHANGES,
    PLACING_STEPS, READING_STEPS, UpdateLimits,
};
use fvoci_model::{BODY_MAX_BYTES, Body};
use serde_json::{Value, json};

const LIMITS: UpdateLimits = UpdateLimits {
    update_bytes: 64 * 1024,
    state_bytes: 16 * 1024 * 1024,
    body_bytes: BODY_MAX_BYTES,
    placing_steps: Some(PLACING_STEPS),
    clients: Some(MAX_CLIENTS),
    client_changes: Some(MAX_CLIENT_CHANGES),
    total_changes: Some(MAX_TOTAL_CHANGES),
    reading_steps: Some(READING_STEPS),
};

fn body_of(content: &Value) -> Body {
    Body::parse(json!({"type": "doc", "content": content}))
        .expect("a body")
        .expect("not empty")
}

fn paragraph(text: &str) -> Value {
    json!({"type": "paragraph", "content": [{"type": "text", "text": text}]})
}

/// What is appended: blocks of several kinds, text with marks among them.
fn appended() -> Vec<Value> {
    vec![
        json!({"type": "heading", "attrs": {"level": 2}, "content": [{"type": "text", "text": "From the agent"}]}),
        json!({"type": "paragraph", "content": [
            {"type": "text", "text": "plain "},
            {"type": "text", "text": "bold", "marks": [{"type": "bold"}]},
            {"type": "text", "text": " and "},
            {"type": "text", "text": "a link", "marks": [{"type": "link", "attrs": {"href": "https://example.com/"}}, {"type": "italic"}]},
            {"type": "text", "text": " after"},
        ]}),
        json!({"type": "codeBlock", "attrs": {"language": "rust"}, "content": [{"type": "text", "text": "fn main() {}"}]}),
        json!({"type": "blockMath", "attrs": {"latex": "x^2"}}),
    ]
}

/// The JSON of a document's body.
fn json_of(doc: &BodyDoc) -> Value {
    serde_json::to_value(doc.body().expect("a body").expect("not empty")).expect("JSON")
}

/// Takes `update` into `doc`, as the worker does.
fn taken(doc: &BodyDoc, update: &[u8]) -> BodyDoc {
    let changed = doc.take(update, &LIMITS).expect("the update is taken");
    assert!(changed.repair.update.is_none(), "nothing to repair");
    changed.doc
}

/// The clients an update writes as.
fn writers(update: &[u8]) -> Vec<u64> {
    let update = Update::decode_v1(update).expect("an update");
    update
        .state_vector()
        .iter()
        .map(|(client, _)| client.get())
        .collect()
}

#[test]
fn the_nodes_follow_the_body_with_their_marks() {
    let doc = BodyDoc::from_body(Some(&body_of(&json!([paragraph("first")]))));
    let update = doc.append_update(&appended()).expect("an update");
    let after = taken(&doc, &update);
    let mut expected = vec![paragraph("first")];
    expected.extend(appended());
    assert_eq!(json_of(&after), json!({"type": "doc", "content": expected}));

    let empty = BodyDoc::new();
    let first = taken(
        &empty,
        &empty.append_update(&appended()).expect("an update"),
    );
    assert_eq!(
        json_of(&first),
        json!({"type": "doc", "content": appended()})
    );
}

#[test]
fn each_append_writes_as_a_client_of_its_own_and_both_of_two_stay() {
    let doc = BodyDoc::from_body(Some(&body_of(&json!([paragraph("first")]))));
    let one = doc.append_update(&[paragraph("one")]).expect("an update");
    let two = doc.append_update(&[paragraph("two")]).expect("an update");
    let (one_writer, two_writer) = (writers(&one), writers(&two));
    assert_eq!(one_writer.len(), 1, "one client: {one_writer:?}");
    assert!(one_writer[0] >= FIRST_APPENDER && two_writer[0] >= FIRST_APPENDER);
    assert!(one_writer[0] < 1 << 53, "a number JavaScript keeps");
    assert_ne!(one_writer, two_writer);

    let both = taken(&taken(&doc, &one), &two);
    let text = json_of(&both).to_string();
    assert!(
        text.contains("\"one\"") && text.contains("\"two\""),
        "{text}"
    );
}

#[test]
fn what_an_editor_typed_meanwhile_stays_whichever_comes_first() {
    let doc = BodyDoc::from_body(Some(&body_of(&json!([paragraph("first")]))));
    // An editor holds the document and types into its paragraph, not yet sent.
    let editor = Doc::new();
    editor
        .transact_mut()
        .apply_update(Update::decode_v1(&doc.state()).expect("a state"))
        .expect("applied");
    let seen = editor.transact().state_vector();
    {
        let root = editor.get_or_insert_xml_fragment(FIELD);
        let mut txn = editor.transact_mut();
        let Some(XmlOut::Element(paragraph)) = root.get(&txn, 0) else {
            panic!("a paragraph");
        };
        let Some(XmlOut::Text(text)) = paragraph.get(&txn, 0) else {
            panic!("its text");
        };
        let end = text.get_string(&txn).len();
        text.insert(&mut txn, u32::try_from(end).expect("short"), " typed");
    }
    let typed = editor.transact().encode_diff_v1(&seen);

    let update = doc
        .append_update(&[paragraph("from the agent")])
        .expect("an update");
    let server = taken(&taken(&doc, &update), &typed);
    let other_order = taken(&taken(&doc, &typed), &update);
    let expected = json!({"type": "doc", "content": [
        paragraph("first typed"), paragraph("from the agent"),
    ]});
    assert_eq!(json_of(&server), expected);
    assert_eq!(json_of(&other_order), expected);
    // The editor takes the append as another editor's update.
    editor
        .transact_mut()
        .apply_update(Update::decode_v1(&update).expect("the update"))
        .expect("applied");
    assert_eq!(
        editor.transact().state_vector(),
        Update::decode_v1(&server.state())
            .expect("a state")
            .state_vector(),
        "the editor holds what the server holds"
    );
}
