//! The previews the card list gives when asked.

use fvoci_model::{Container, ContainerVisibility, new_id};
use serde_json::{Value, json};

use crate::cards::cards_path;
use crate::common::TEST_WS;
use crate::common::{Setup, TEST_WORKSPACE_ID, TestServer};

/// An open board with a person property `owner` and two cards: "with", owned by the
/// first user, whose body has a mention, a text whose cut falls before an emoji, an image
/// in a list and another after it; and "without", with no body. Returns the board, the
/// file of the image in the list, and the first user's identifier.
async fn board_with_previewed_card(th: &TestServer) -> (Container, String, String) {
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let me = th.client.me().await;
    let upload = crate::files::upload(&th.client, &board.id, "picture.png", b"png").await;
    upload.check_created();
    let file_id = crate::files::uploaded_id(&upload);
    let later = crate::files::upload(&th.client, &board.id, "later.png", b"png").await;
    later.check_created();
    let later_id = crate::files::uploaded_id(&later);
    th.client
        .patch(
            &format!("{TEST_WS}/containers/{}", board.id),
            &json!({"updatedCardProperties": [
                {"id": "owner", "name": "Owner", "type": "person", "options": []},
            ]}),
        )
        .await
        .check_ok();
    // 199 two-byte letters and an emoji of two UTF-16 units: the cut falls before it.
    let opening = format!("{}🎉", "가".repeat(199));
    let paragraph =
        |text: &str| json!({"type": "paragraph", "content": [{"type": "text", "text": text}]});
    // A mention first, which the text leaves out; the first image is in a list, before
    // another.
    let body = json!({"type": "doc", "content": [
        {"type": "paragraph", "content": [{"type": "mention", "attrs": {"userId": me.id}}]},
        paragraph(&opening),
        {"type": "bulletList", "content": [{"type": "listItem", "content": [
            paragraph("item"),
            {"type": "image", "attrs": {"fileId": file_id}},
        ]}]},
        {"type": "image", "attrs": {"fileId": later_id}},
        paragraph("more"),
    ]});
    let _ = new_id();
    for card in [
        json!({"title": "with", "body": body, "properties": {"owner": me.id}}),
        json!({"title": "without"}),
    ] {
        th.client
            .post(&cards_path(&board.id), &card)
            .await
            .check_created();
    }
    (board, file_id, me.id)
}

/// With `preview=true` the card list gives each card the file of its body's first image
/// and the beginning of its text, at most 200 UTF-16 code units cut between characters;
/// without it, no preview. A reader
/// with the board's read token gets the same previews, the cards naming no users, and
/// gets the image with the same token.
#[tokio::test]
async fn the_card_list_gives_previews_when_asked() {
    let th = Setup::local_mode().start().await.init_basic().await;
    let (board, file_id, me_id) = board_with_previewed_card(&th).await;
    let previews = |cards: &[Value]| -> Vec<(Value, Value)> {
        cards
            .iter()
            .map(|card| (card["title"].clone(), card["preview"].clone()))
            .collect()
    };
    let expected = vec![
        (
            json!("with"),
            json!({"imageFileId": file_id, "text": "가".repeat(199)}),
        ),
        (json!("without"), json!({"imageFileId": "", "text": ""})),
    ];

    let plain: Vec<Value> = th.client.get(&cards_path(&board.id)).await.ok_json();
    assert!(
        plain.iter().all(|card| card.get("preview").is_none()),
        "no previews unless asked: {plain:?}"
    );
    let asked: Vec<Value> = th
        .client
        .get(&format!("{}?preview=true", cards_path(&board.id)))
        .await
        .ok_json();
    assert_eq!(previews(&asked), expected);

    let token = th.share(&board.id).await;
    let shared = format!("/shared/{}/cards?preview=true&readToken={token}", board.id);
    let read: Vec<Value> = th.anonymous().get(&shared).await.ok_json();
    assert_eq!(
        previews(&read),
        expected,
        "the same for a reader by the link"
    );
    // Anonymous, or signed in, a member too: no users; a member without the link: as ever.
    let outsider: Vec<Value> = th.client2.get(&shared).await.ok_json();
    assert_eq!(previews(&outsider), expected);
    for card in read.iter().chain(&outsider) {
        assert_eq!(card["createdBy"], json!(""), "{card}");
        assert_eq!(card["modifiedBy"], json!(""), "{card}");
    }
    assert_eq!(
        outsider[0]["properties"]["owner"],
        json!(""),
        "{outsider:?}"
    );
    let member: Vec<Value> = th.client.get(&shared).await.ok_json();
    assert_eq!(member[0]["createdBy"], json!(""));
    let member: Vec<Value> = th
        .client
        .get(&format!("{}?preview=true", cards_path(&board.id)))
        .await
        .ok_json();
    assert_eq!(member[0]["createdBy"], json!(me_id));
    assert_eq!(member[0]["properties"]["owner"], json!(me_id));
    th.anonymous()
        .get(&format!(
            "/shared/{}/files/{file_id}?readToken={token}",
            board.id
        ))
        .await
        .check_ok();

    th.anonymous()
        .get(&cards_path(&board.id))
        .await
        .check_unauthorized();
    th.anonymous()
        .get(&format!("/shared/{}/cards?readToken=nope", board.id))
        .await
        .check_not_found();
}
