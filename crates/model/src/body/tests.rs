use serde_json::json;

use super::*;

#[test]
fn a_number_is_written_in_groups_of_three_digits() {
    for (number, written) in [
        (0, "0"),
        (999, "999"),
        (1_000, "1,000"),
        (100_000, "100,000"),
        (1_234_567, "1,234,567"),
    ] {
        assert_eq!(in_thousands(number), written);
    }
}

fn paragraph(text: &str) -> Value {
    json!({"type": "paragraph", "content": [{"type": "text", "text": text}]})
}

fn document(blocks: &[Value]) -> Value {
    json!({"type": "doc", "content": blocks})
}

#[test]
fn null_and_an_empty_paragraph_are_no_body() {
    for empty in [
        Value::Null,
        document(&[json!({"type": "paragraph"})]),
        document(&[json!({"type": "paragraph", "content": []})]),
    ] {
        assert_eq!(Body::parse(empty.clone()), Ok(None), "{empty}");
    }
    let body = Body::parse(document(&[paragraph("x")]))
        .expect("a body")
        .expect("not empty");
    assert_eq!(body.document(), &document(&[paragraph("x")]));
}

/// An image or a file block whose file has a new name points at it; one whose file
/// has none becomes an empty paragraph, and the body stays valid.
#[test]
fn an_image_or_a_file_block_without_its_file_becomes_an_empty_paragraph() {
    for kind in ["image", "file"] {
        let node = |file: &str| json!({"type": kind, "attrs": {"fileId": file}});
        let body = Body::parse(document(&[
            node("kept.png"),
            json!({"type": "blockquote", "content": [node("lost.png")]}),
            paragraph("x"),
        ]))
        .expect("a body")
        .expect("not empty");
        let body = body
            .with_files_renamed(|old| (old == "kept.png").then(|| "copy.png".to_owned()))
            .expect("still a body");
        let expected = document(&[
            node("copy.png"),
            json!({"type": "blockquote", "content": [{"type": "paragraph"}]}),
            paragraph("x"),
        ]);
        assert_eq!(body.document(), &expected, "{kind}");
        assert!(
            Body::parse(expected).is_ok_and(|body| body.is_some()),
            "still a valid body: {kind}"
        );
        assert_eq!(body.file_ids(), ["copy.png"], "{kind}");
    }
}

/// The files of a body are those of its images and of its file blocks, each once,
/// in document order; only a file block makes a body one a comment may not have.
#[test]
fn the_files_of_a_body_are_those_of_its_images_and_file_blocks() {
    let image = |file: &str| json!({"type": "image", "attrs": {"fileId": file}});
    let file = |file: &str| json!({"type": "file", "attrs": {"fileId": file}});
    let body = Body::parse(document(&[
        file("report.pdf"),
        json!({"type": "bulletList", "content": [{"type": "listItem", "content": [
            paragraph("item"), image("chart.png"), file("report.pdf"),
        ]}]}),
        file("minutes.hwp"),
    ]))
    .expect("a body")
    .expect("not empty");
    assert_eq!(body.file_ids(), ["report.pdf", "chart.png", "minutes.hwp"]);
    assert!(body.has_file_blocks(), "file blocks");
    assert_eq!(body.text(), "item", "a file block gives the text nothing");
    let pictures = Body::parse(document(&[image("chart.png")]))
        .expect("a body")
        .expect("not empty");
    assert!(!pictures.has_file_blocks(), "an image is no file block");
}

/// The users a body mentions are named once each, in the order they come, also
/// from deep in a list; a mention that was emptied names nobody.
#[test]
fn the_mentions_of_a_body_name_each_user_once() {
    let mention = |id: &str| json!({"type": "mention", "attrs": {"userId": id}});
    let (first, second) = (crate::new_id(), crate::new_id());
    let mut body = Body::parse(document(&[
        json!({"type": "paragraph", "content": [mention(&second), mention(&first)]}),
        json!({"type": "bulletList", "content": [{"type": "listItem", "content": [
            {"type": "paragraph", "content": [mention(&second)]},
        ]}]}),
    ]))
    .expect("a body")
    .expect("not empty");
    assert_eq!(body.mention_ids(), [second.as_str(), first.as_str()]);
    body.clear_mentions();
    let nobody: [&str; 0] = [];
    assert_eq!(body.mention_ids(), nobody);
}

/// Every mention loses its user, also deep in a list; nothing else changes.
#[test]
fn clearing_the_mentions_empties_every_user_id() {
    let mention = |id: &str| json!({"type": "mention", "attrs": {"userId": id}});
    let id = "01a1c8e0-e250-735f-b851-4aaaa2ad2d83";
    let mut body = Body::parse(document(&[
        json!({"type": "paragraph", "content": [mention(id), {"type": "text", "text": "x"}]}),
        json!({"type": "bulletList", "content": [{"type": "listItem", "content": [
            {"type": "paragraph", "content": [mention(id)]},
        ]}]}),
    ]))
    .expect("a body")
    .expect("not empty");
    body.clear_mentions();
    let value = serde_json::to_value(&body).expect("JSON");
    assert_eq!(value["content"][0]["content"][0], mention(""));
    assert_eq!(value["content"][0]["content"][1]["text"], json!("x"));
    assert_eq!(
        value["content"][1]["content"][0]["content"][0]["content"][0],
        mention("")
    );
}

/// Every reference that names none of what is shown loses its identifier, also deep
/// in a list; one that names what is shown stays, and nothing else changes. And a
/// reference is renamed where there is a new name for what it names.
#[test]
fn references_are_emptied_but_for_what_is_shown_and_renamed_where_there_is_a_name() {
    let reference = |id: &str| json!({"type": "reference", "attrs": {"id": id}});
    let shown = "01a1c8e0-e250-735f-b851-4aaaa2ad2d83";
    let outside = "01a1c8e0-e250-735f-b851-4aaaa2ad2d84";
    let copy = "01a1c8e0-e250-735f-b851-4aaaa2ad2d85";
    let parsed = Body::parse(document(&[
        json!({"type": "paragraph", "content": [
            reference(shown), {"type": "text", "text": "x"}, reference(outside),
        ]}),
        json!({"type": "bulletList", "content": [{"type": "listItem", "content": [
            {"type": "paragraph", "content": [reference(outside), reference(shown)]},
        ]}]}),
    ]))
    .expect("a body")
    .expect("not empty");

    let mut body = parsed.clone();
    body.clear_references_but(&HashSet::from([shown.to_owned()]));
    let value = serde_json::to_value(&body).expect("JSON");
    let line = &value["content"][0]["content"];
    assert_eq!(line[0], reference(shown));
    assert_eq!(line[1]["text"], json!("x"));
    assert_eq!(line[2], reference(""));
    let item = &value["content"][1]["content"][0]["content"][0]["content"];
    assert_eq!((&item[0], &item[1]), (&reference(""), &reference(shown)));
    assert!(!value.to_string().contains(outside));

    let mut body = parsed;
    body.rename_links(|id| (id == shown).then(|| copy.to_owned()));
    assert_eq!(body.reference_ids(), [copy, outside]);
    let value = serde_json::to_value(&body).expect("JSON");
    assert_eq!(value["content"][0]["content"][1]["text"], json!("x"));
}

/// A board view shows a view by its identifier: it is told with the references where a
/// body's links are asked, emptied for a reader by link whatever is shown, and renamed
/// with them where they are copied: a copy names, and shows, the copies.
#[test]
fn board_views_are_linked_emptied_for_a_link_and_renamed_with_references() {
    let reference = |id: &str| json!({"type": "reference", "attrs": {"id": id}});
    let view = |id: &str| json!({"type": "boardView", "attrs": {"viewId": id}});
    let card = "01a1c8e0-e250-735f-b851-4aaaa2ad2d83";
    let table = "01a1c8e0-e250-735f-b851-4aaaa2ad2d86";
    let kanban = "01a1c8e0-e250-735f-b851-4aaaa2ad2d87";
    let copy = "01a1c8e0-e250-735f-b851-4aaaa2ad2d88";
    let toggle = |hidden: Value| {
        json!({"type": "details", "content": [
            {"type": "detailsSummary"},
            {"type": "detailsContent", "content": [hidden]},
        ]})
    };
    let parsed = Body::parse(document(&[
        json!({"type": "paragraph", "content": [reference(card), reference(table)]}),
        view(table),
        toggle(view(kanban)),
        view(table),
    ]))
    .expect("a body")
    .expect("not empty");
    assert_eq!(parsed.board_view_ids(), [table, kanban]);
    assert_eq!(
        parsed.board_view_count(),
        3,
        "one view shown twice counts twice"
    );
    assert!(parsed.has_board_views());
    assert_eq!(
        parsed.reference_ids(),
        [card, table],
        "a board view is no reference"
    );
    assert_eq!(parsed.linked_ids(), [card, table, kanban]);

    let mut shown = parsed.clone();
    shown.clear_references_but(&HashSet::from([table.to_owned(), kanban.to_owned()]));
    let value = serde_json::to_value(&shown).expect("JSON");
    assert_eq!(value["content"][0]["content"][1], reference(table));
    assert_eq!(
        value["content"][1],
        view(""),
        "a board view loads no board by a link"
    );
    assert_eq!(value["content"][2]["content"][1]["content"][0], view(""));
    assert!(
        shown.board_view_ids().is_empty(),
        "an emptied one shows nothing"
    );
    assert_eq!(shown.board_view_count(), 3);

    let mut renamed = parsed;
    renamed.rename_links(|id| (id == table).then(|| copy.to_owned()));
    assert_eq!(renamed.reference_ids(), [card, copy]);
    assert_eq!(
        renamed.board_view_ids(),
        [copy, kanban],
        "the board views with the references"
    );
    assert_eq!(renamed.board_view_count(), 3);
}

/// A body shows at most ten board views, wherever they stand; the eleventh is refused
/// once all else of the body is allowed.
#[test]
fn a_body_shows_at_most_ten_board_views() {
    let view = |at: usize| json!({"type": "boardView", "attrs": {"viewId": format!("01a1c8e0-e250-735f-b851-4aaaa2ad{at:04x}")}});
    let ten: Vec<Value> = (0..BODY_MAX_BOARD_VIEWS).map(view).collect();
    assert!(Body::parse(document(&ten)).is_ok(), "ten");
    let mut eleven = ten;
    eleven.push(json!({"type": "details", "content": [
        {"type": "detailsSummary"},
        {"type": "detailsContent", "content": [view(99)]},
    ]}));
    assert_eq!(
        Body::parse(document(&eleven)),
        Err(BodyError::TooManyBoardViews)
    );
    eleven.push(json!({"type": "boardView"}));
    assert!(
        matches!(
            Body::parse(document(&eleven)),
            Err(BodyError::InvalidNode { .. })
        ),
        "a node that is not allowed is told first"
    );
}

/// A body that was an image or a file block without its file and nothing else is no
/// body.
#[test]
fn a_body_of_a_lost_file_alone_is_no_body() {
    for kind in ["image", "file"] {
        let body = Body::parse(document(&[
            json!({"type": kind, "attrs": {"fileId": "lost.png"}}),
        ]))
        .expect("a body")
        .expect("not empty");
        assert_eq!(body.with_files_renamed(|_| None), None, "{kind}");
    }
}

/// The references of a body are the blocks it names, each once, wherever they stand.
#[test]
fn the_references_of_a_body_are_named_once_in_order() {
    let reference = |id: &str| json!({"type": "reference", "attrs": {"id": id}});
    let first = "01a1c8e0-e250-735f-b851-4aaaa2ad2d83";
    let second = "01a1c8e0-e250-735f-b851-4aaaa2ad2d84";
    let body = Body::parse(document(&[
        json!({"type": "paragraph", "content": [reference(second), {"type": "text", "text": "x"}]}),
        json!({"type": "bulletList", "content": [{"type": "listItem", "content": [
            {"type": "paragraph", "content": [reference(first), reference(second)]},
        ]}]}),
    ]))
    .expect("a body")
    .expect("not empty");
    assert_eq!(body.reference_ids(), [second, first]);
    assert_eq!(body.text(), "x", "a reference gives the text nothing");
    let none = Body::from_stored(document(&[paragraph("x")]));
    assert!(none.reference_ids().is_empty(), "no reference");
}

#[test]
fn what_is_no_document_is_refused() {
    for value in [
        json!("text"),
        json!([]),
        json!({"type": "paragraph", "content": [paragraph("x")]}),
        json!({"type": "doc"}),
        json!({"type": "doc", "content": []}),
    ] {
        assert_eq!(
            Body::parse(value.clone()),
            Err(BodyError::NotADocument),
            "{value}"
        );
    }
}

#[test]
fn malformed_nodes_are_refused() {
    for node in [
        json!({"type": "paragraph", "extra": 1}),
        json!({"type": "paragraph", "attrs": null}),
        json!({"content": []}),
        json!({"type": "text", "text": ""}),
        json!({"type": "text", "text": 1}),
        json!({"type": "paragraph", "marks": [1]}),
        json!({"type": "paragraph", "content": {}}),
        json!(7),
    ] {
        let value = document(&[paragraph("x"), node.clone()]);
        assert!(
            matches!(Body::parse(value), Err(BodyError::InvalidNode { .. })),
            "{node}"
        );
    }
}

/// A document nested `depth` levels deep, the document counted.
fn nested(depth: usize) -> Value {
    let mut node = json!({"type": "paragraph"});
    for _ in 2..depth {
        node = json!({"type": "blockquote", "content": [node]});
    }
    document(&[node])
}

#[test]
fn the_depth_is_limited() {
    assert!(
        Body::parse(nested(BODY_MAX_DEPTH)).is_ok(),
        "{BODY_MAX_DEPTH} levels"
    );
    assert_eq!(
        Body::parse(nested(BODY_MAX_DEPTH + 1)),
        Err(BodyError::TooDeep)
    );
}

#[test]
fn the_nodes_are_limited() {
    // The document and its paragraphs.
    let fits = document(&vec![json!({"type": "horizontalRule"}); BODY_MAX_NODES - 1]);
    assert!(Body::parse(fits).is_ok(), "{BODY_MAX_NODES} nodes");
    let over = document(&vec![json!({"type": "horizontalRule"}); BODY_MAX_NODES]);
    assert_eq!(Body::parse(over), Err(BodyError::TooManyNodes));
}

#[test]
fn the_size_is_limited() {
    // The JSON of the document around the text of its one paragraph.
    let frame = serde_json::to_vec(&document(&[paragraph("")]))
        .expect("JSON")
        .len();
    let fits = "x".repeat(BODY_MAX_BYTES - frame);
    assert!(
        Body::parse(document(&[paragraph(&fits)])).is_ok(),
        "{BODY_MAX_BYTES} bytes"
    );
    let over = "x".repeat(BODY_MAX_BYTES - frame + 1);
    assert_eq!(
        Body::parse(document(&[paragraph(&over)])),
        Err(BodyError::TooLarge)
    );
}

#[test]
fn task_items_are_counted() {
    let item = |checked: bool| json!({"type": "taskItem", "attrs": {"checked": checked}, "content": [paragraph("t")]});
    let body = Body::parse(document(&[
        json!({"type": "taskList", "content": [item(true), item(false), item(true)]}),
    ]))
    .expect("a body")
    .expect("not empty");
    assert_eq!(body.tasks(), TaskCount { total: 3, done: 2 });
}

/// The first image, wherever it is; a file block before it is no picture.
#[test]
fn the_first_image_is_the_first_in_document_order() {
    let image = |file: &str| json!({"type": "image", "attrs": {"fileId": file}});
    let body = Body::from_stored(json!({"type": "doc", "content": [
        paragraph("before"),
        {"type": "file", "attrs": {"fileId": "report.pdf"}},
        {"type": "bulletList", "content": [
            {"type": "listItem", "content": [paragraph("item"), image("in-a-list.png")]},
        ]},
        image("after.png"),
    ]}));
    assert_eq!(body.first_image_file_id(), Some("in-a-list.png"));
    let none = Body::from_stored(json!({"type": "doc", "content": [paragraph("text")]}));
    assert_eq!(none.first_image_file_id(), None);
}

#[test]
fn appending_bodies_merges_blocks_and_enforces_size_and_node_limits() {
    let first = Body::parse(document(&[paragraph("one")]))
        .expect("valid")
        .expect("non-empty");
    let second = Body::parse(document(&[paragraph("two")]))
        .expect("valid")
        .expect("non-empty");

    assert_eq!(
        Body::append_to(None, second.clone()).expect("append to empty"),
        second
    );
    let merged = Body::append_to(Some(&first), second).expect("merged");
    assert_eq!(
        merged.document(),
        &document(&[paragraph("one"), paragraph("two")])
    );

    // Exceeding BODY_MAX_NODES across existing + extra is refused.
    let half = BODY_MAX_NODES / 2;
    let left = Body::parse(document(&vec![json!({"type": "horizontalRule"}); half]))
        .expect("valid")
        .expect("non-empty");
    let right = Body::parse(document(&vec![json!({"type": "horizontalRule"}); half]))
        .expect("valid")
        .expect("non-empty");
    assert_eq!(
        Body::append_to(Some(&left), right),
        Err(BodyError::TooManyNodes)
    );

    // Exceeding BODY_MAX_BYTES across existing + extra is refused.
    let big = Body::parse(document(&[paragraph(
        &"a".repeat(BODY_MAX_BYTES / 2 + 1024),
    )]))
    .expect("valid")
    .expect("non-empty");
    assert_eq!(
        Body::append_to(Some(&big), big.clone()),
        Err(BodyError::TooLarge)
    );
}
