//! The largest body on every way a body comes to the server or is copied by it: the
//! routes that write one, the room it is edited in, an archive, a copy of its card or
//! its board, and a version that is restored. A body of exactly
//! [`BODY_MAX_BYTES`] is taken whole on each; a byte more is refused where a client can
//! send one, and the answer says why.

use std::io::{Cursor, Write as _};

use fvoci_model::{BODY_MAX_BYTES, BODY_MAX_DEPTH, Card, ContainerVisibility, ContainersAndBlocks};
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::boards::duplicate_board;
use crate::body_versions::{said, versions};
use crate::collab::{body_of, card};
use crate::common::{ApiResponse, Setup, TEST_WORKSPACE_ID, TEST_WS};
use crate::export::{boards_of, import};
use crate::rooms::{Peer, paragraphs, table, write_whole};

/// How many bytes a body is as it is stored.
fn stored_bytes(document: &Value) -> usize {
    serde_json::to_vec(document).expect("JSON").len()
}

/// A document of paragraphs, the first of letters and then one of each of `others`,
/// that is `bytes` long as it is stored.
fn document_of(bytes: usize, others: &[&str]) -> Value {
    let with = |first: &str| {
        let mut texts = vec![first];
        texts.extend(others);
        paragraphs(&texts)
    };
    let frame = stored_bytes(&with(""));
    let document = with(&"a".repeat(bytes - frame));
    assert_eq!(stored_bytes(&document), bytes, "as long as asked for");
    document
}

/// Checks the answer to a body that is a byte too large: a 400 that names the body and
/// says the limit.
#[track_caller]
fn check_too_large(response: &ApiResponse) {
    response.check_error(
        StatusCode::BAD_REQUEST,
        &format!("the body is larger than {BODY_MAX_BYTES} bytes"),
    );
    assert_eq!(response.refused_at(), "/body", "{}", response.text());
}

#[tokio::test]
async fn the_largest_body_is_written_copied_and_restored_and_a_byte_more_is_refused() {
    let th = Setup::native().start().await.init_basic().await;
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let cards = format!("{TEST_WS}/boards/{}/cards", board.id);
    let most = document_of(BODY_MAX_BYTES, &[]);
    let more = document_of(BODY_MAX_BYTES + 1, &[]);

    let refused = th
        .client
        .post(&cards, &json!({"title": "More", "body": more}))
        .await;
    check_too_large(&refused);
    let made: Card = th
        .client
        .post(&cards, &json!({"title": "Most", "body": most}))
        .await
        .created_json();
    let stored = async |card_id: &str| body_of(&card(&th.client, card_id).await);
    assert!(stored(&made.id).await.as_ref() == Some(&most), "as written");

    // Written over, with a byte more refused there too, and brought back as a version.
    write_whole(&th.client, &made.id, &paragraphs(&["small"])).await;
    let read = card(&th.client, &made.id).await;
    let refused = th
        .client
        .patch(
            &format!("{TEST_WS}/cards/{}", made.id),
            &json!({"body": more, "readAt": read.updated_at}),
        )
        .await;
    check_too_large(&refused);
    let kept = versions(&th.client, &made.id).await;
    let first = said(&kept)
        .iter()
        .map(|(version, _, _)| *version)
        .min()
        .expect("a version of the body the card was made with");
    th.client
        .post_empty(&format!(
            "{TEST_WS}/cards/{}/body-versions/{first}/restore",
            made.id
        ))
        .await
        .check_ok();
    assert!(stored(&made.id).await.as_ref() == Some(&most), "restored");

    // A copy of the card, and copies of its board, as a board and as a template.
    let copies: Vec<Value> = th
        .client
        .post_empty(&format!(
            "{TEST_WS}/boards/{}/blocks/{}/duplicate",
            board.id, made.id
        ))
        .await
        .created_json();
    let copy = copies
        .iter()
        .find(|block| block["type"] == json!("card"))
        .and_then(|block| block["id"].as_str())
        .expect("the copy of the card");
    assert!(stored(copy).await.as_ref() == Some(&most), "copied");
    for template in [false, true] {
        let of_the_board: ContainersAndBlocks = duplicate_board(
            &th.client,
            &board.id,
            &format!("?asTemplate={template}&toWorkspace={TEST_WORKSPACE_ID}"),
        )
        .await
        .created_json();
        let in_the_copy: Vec<&str> = of_the_board
            .blocks
            .iter()
            .filter(|block| block.title == "Most")
            .map(|block| block.id.as_str())
            .collect();
        assert_eq!(in_the_copy.len(), 2, "the card and its copy");
        for card_id in in_the_copy {
            assert!(
                stored(card_id).await.as_ref() == Some(&most),
                "in the copy of the board, as a template: {template}"
            );
        }
    }
    th.teardown().await;
}

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

/// The deepest body is read back wherever it was written: the JSON reader takes 127
/// levels, and a body is written inside a block, a list of blocks, a line of an
/// archive. A level deeper is refused by the limit of a body, not by the reader.
#[tokio::test]
async fn the_deepest_body_is_read_back_from_every_place_it_is_written_in() {
    let th = Setup::native().start().await.init_basic().await;
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let cards = format!("{TEST_WS}/boards/{}/cards", board.id);
    let deep = deepest();
    let deeper = json!({"type": "doc", "content": [
        {"type": "blockquote", "content": [deep["content"][0].clone()]},
    ]});
    let refused = th
        .client
        .post(&cards, &json!({"title": "Deeper", "body": deeper}))
        .await;
    refused.check_error(
        StatusCode::BAD_REQUEST,
        &format!("the body nests deeper than {BODY_MAX_DEPTH} levels"),
    );
    let made: Card = th
        .client
        .post(&cards, &json!({"title": "Deep", "body": deep}))
        .await
        .created_json();
    let stored = async |card_id: &str| body_of(&card(&th.client, card_id).await);
    assert_eq!(stored(&made.id).await.as_ref(), Some(&deep));

    // To the trash and back, which stores the block as one value.
    let block = format!("{TEST_WS}/boards/{}/blocks/{}", board.id, made.id);
    th.client.delete(&block).await.check_no_content();
    th.client
        .post_empty(&format!("{block}/restore"))
        .await
        .check_ok();
    assert_eq!(stored(&made.id).await.as_ref(), Some(&deep));

    // In the answer of a copy of the board, a list of blocks inside an object.
    let copied: Value = duplicate_board(
        &th.client,
        &board.id,
        &format!("?asTemplate=false&toWorkspace={TEST_WORKSPACE_ID}"),
    )
    .await
    .created_json();
    let copy = copied["blocks"]
        .as_array()
        .and_then(|blocks| blocks.iter().find(|block| block["title"] == json!("Deep")))
        .expect("the copy of the card");
    assert_eq!(copy["body"], deep);

    // In a line of an archive, and in the room, where it is made a shared document
    // and read back from it.
    import(&th.client, TEST_WORKSPACE_ID, &archive_of(&deep))
        .await
        .check_ok();
    let imported = boards_of(&th).await;
    let from_archive = imported
        .iter()
        .find(|board| board.title == "Limits")
        .expect("the board of the archive");
    let listed: Vec<Value> = th
        .client
        .get(&format!("{TEST_WS}/boards/{}/cards", from_archive.id))
        .await
        .ok_json();
    let card_id = listed[0]["id"].as_str().expect("the card of the archive");
    assert_eq!(stored(card_id).await.as_ref(), Some(&deep));
    let mut one = Peer::connect(&th.client, 101).await;
    one.joined(TEST_WORKSPACE_ID, &made.id).await;
    assert_eq!(one.body().as_ref(), Some(&deep));
    one.write_acked("under it").await;
    th.teardown().await;
}

/// An archive of one board with one card of the given body.
fn archive_of(body: &Value) -> Vec<u8> {
    let board = json!({"type": "container", "data": {
        "id": "b1", "workspaceId": TEST_WORKSPACE_ID, "visibility": "private", "title": "Limits",
        "createdAt": 1, "updatedAt": 1,
    }});
    let card = json!({"type": "block", "data": {
        "id": "c1", "containerId": "b1", "parentId": "", "type": "card", "title": "Most",
        "createdAt": 1, "updatedAt": 1, "body": body,
    }});
    let options = zip::write::SimpleFileOptions::default();
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    writer.start_file("version.json", options).expect("start");
    writer
        .write_all(br#"{"version":1,"date":1}"#)
        .expect("write");
    writer
        .start_file("b1/container.jsonl", options)
        .expect("start");
    writer
        .write_all(format!("{board}\n{card}\n").as_bytes())
        .expect("write");
    writer.finish().expect("finish").into_inner()
}

#[tokio::test]
async fn an_archive_brings_the_largest_body_and_one_of_a_byte_more_is_refused_whole() {
    let th = Setup::native().start().await.init_basic().await;
    let boards = async || boards_of(&th).await;
    let before = boards().await.len();

    let more = document_of(BODY_MAX_BYTES + 1, &[]);
    let refused = import(&th.client, TEST_WORKSPACE_ID, &archive_of(&more)).await;
    refused.check_error(
        StatusCode::BAD_REQUEST,
        &format!("invalid block in archive line 2: the body is larger than {BODY_MAX_BYTES} bytes"),
    );
    assert_eq!(boards().await.len(), before, "nothing of it is made");

    let most = document_of(BODY_MAX_BYTES, &[]);
    import(&th.client, TEST_WORKSPACE_ID, &archive_of(&most))
        .await
        .check_ok();
    let imported = boards().await;
    let board = imported
        .iter()
        .find(|board| board.title == "Limits")
        .expect("the board of the archive");
    let cards: Vec<Value> = th
        .client
        .get(&format!("{TEST_WS}/boards/{}/cards", board.id))
        .await
        .ok_json();
    let card_id = cards[0]["id"].as_str().expect("the card of the archive");
    assert!(
        body_of(&card(&th.client, card_id).await).as_ref() == Some(&most),
        "the body as the archive had it"
    );
    th.teardown().await;
}

/// In the room of a body an update that brings it to its largest is taken, and the
/// next, which would make it a letter larger, is refused by its name and changes
/// nothing: who comes into the room then is given the largest body.
#[tokio::test]
async fn the_room_takes_the_update_to_the_largest_body_and_refuses_the_next() {
    let th = Setup::native().start().await.init_basic().await;
    // A paragraph short of the largest body.
    let most = document_of(BODY_MAX_BYTES, &["last"]);
    let begun = {
        let mut begun = most.clone();
        let content = begun["content"].as_array_mut().expect("content");
        content.pop();
        begun
    };
    let t = table(&th, &begun).await;
    let mut one = Peer::connect(&th.client, 101).await;
    one.joined(TEST_WORKSPACE_ID, &t.card_id).await;
    one.write_acked("last").await;

    let letter = one.editor().type_at_end("x");
    one.send_update(&letter).await;
    let nack = one.text("docNack").await;
    assert_eq!(
        nack,
        json!({
            "type": "docNack", "doc": one.doc, "epoch": one.epoch,
            "reason": "document_too_large",
        })
    );
    let mut two = Peer::connect(&th.client, 102).await;
    two.joined(TEST_WORKSPACE_ID, &t.card_id).await;
    assert!(
        two.body().as_ref() == Some(&most),
        "the body with the update that was taken, without the one that was refused"
    );
    th.teardown().await;
}
