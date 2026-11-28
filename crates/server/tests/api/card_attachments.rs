//! The attachments of a card by their own routes: listed, deleted and restored, and
//! none of it reaching an attachment of another card, or a block that is no attachment.

use fvoci_model::{Block, BlockType, Card, ContainerVisibility};
use serde_json::json;

use crate::common::{Client, Setup, TEST_WORKSPACE_ID, TEST_WS, TestServer};
use crate::files::{upload, uploaded_id};

/// The path of the attachments of `card_id` of `board_id`.
pub(crate) fn attachments_of(board_id: &str, card_id: &str) -> String {
    format!("{TEST_WS}/boards/{board_id}/cards/{card_id}/attachments")
}

/// The attachments of the card as `client` reads them.
pub(crate) async fn card_attachments(client: &Client, board_id: &str, card_id: &str) -> Vec<Block> {
    client
        .get(&attachments_of(board_id, card_id))
        .await
        .ok_json()
}

/// A card of `board_id` called `title`, made by `client`.
async fn card_on(client: &Client, board_id: &str, title: &str) -> Card {
    client
        .post(
            &format!("{TEST_WS}/boards/{board_id}/cards"),
            &json!({"title": title}),
        )
        .await
        .created_json()
}

/// A file uploaded to `board_id` and attached to `card_id`.
async fn attached(ts: &TestServer, board_id: &str, card_id: &str, name: &str) -> Block {
    let uploaded = upload(&ts.client, board_id, name, b"attached bytes").await;
    uploaded.check_created();
    ts.client
        .post(
            &attachments_of(board_id, card_id),
            &json!({"fileId": uploaded_id(&uploaded)}),
        )
        .await
        .created_json()
}

/// An attachment is listed with its card, deleted from it, and restored to it; one not
/// in the trash is not there to restore.
#[tokio::test]
async fn an_attachment_is_listed_deleted_and_restored() {
    let ts = Setup::native().start().await.init_basic().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let card = card_on(&ts.client, &board.id, "With a file").await;
    let first = attached(&ts, &board.id, &card.id, "first.txt").await;
    let second = attached(&ts, &board.id, &card.id, "second.txt").await;
    assert_eq!(
        (first.block_type, first.parent_id.as_str()),
        (BlockType::Attachment, card.id.as_str())
    );
    assert_eq!(
        card_attachments(&ts.client, &board.id, &card.id).await,
        [first.clone(), second.clone()],
        "oldest first"
    );

    let path = format!("{}/{}", attachments_of(&board.id, &card.id), first.id);
    ts.client.delete(&path).await.check_no_content();
    assert_eq!(
        card_attachments(&ts.client, &board.id, &card.id).await,
        std::slice::from_ref(&second)
    );
    let restored: Block = ts
        .client
        .post_empty(&format!("{path}/restore"))
        .await
        .ok_json();
    assert_eq!(
        (restored.id.as_str(), restored.title.as_str()),
        (first.id.as_str(), "first.txt")
    );
    assert_eq!(
        card_attachments(&ts.client, &board.id, &card.id)
            .await
            .len(),
        2
    );
    ts.client
        .post_empty(&format!("{path}/restore"))
        .await
        .check_not_found();

    ts.teardown().await;
}

/// The attachment routes reach no attachment of another card, no block that is no
/// attachment, and no card of another board; an attachment that went to the trash with
/// its card comes back only with it.
#[tokio::test]
async fn the_attachment_routes_reach_only_the_attachments_of_their_card() {
    let ts = Setup::native().start().await.init_basic().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let other = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let card = card_on(&ts.client, &board.id, "With a file").await;
    let sibling = card_on(&ts.client, &board.id, "Another").await;
    let elsewhere = card_on(&ts.client, &other.id, "Elsewhere").await;
    let attachment = attached(&ts, &board.id, &card.id, "kept.txt").await;

    // Another card of the board, a card of another board.
    for (board_id, card_id) in [(&board.id, &sibling.id), (&other.id, &elsewhere.id)] {
        let path = format!("{}/{}", attachments_of(board_id, card_id), attachment.id);
        ts.client.delete(&path).await.check_not_found();
        ts.client
            .post_empty(&format!("{path}/restore"))
            .await
            .check_not_found();
    }
    ts.client
        .get(&attachments_of(&board.id, &elsewhere.id))
        .await
        .check_not_found();
    // A card is no attachment of itself, nor an attachment a card.
    let as_attachment = format!("{}/{}", attachments_of(&board.id, &card.id), sibling.id);
    ts.client.delete(&as_attachment).await.check_not_found();
    ts.client
        .get(&attachments_of(&board.id, &attachment.id))
        .await
        .check_not_found();
    assert_eq!(
        card_attachments(&ts.client, &board.id, &card.id).await,
        std::slice::from_ref(&attachment)
    );

    // Gone with its card, it comes back with it.
    let cards = format!("{TEST_WS}/boards/{}/cards", board.id);
    ts.client
        .post(
            &format!("{cards}/batch-delete"),
            &json!({"cardIds": [card.id]}),
        )
        .await
        .check_no_content();
    let path = format!("{}/{}", attachments_of(&board.id, &card.id), attachment.id);
    ts.client
        .post_empty(&format!("{path}/restore"))
        .await
        .check_not_found();
    ts.client
        .post(
            &format!("{cards}/batch-restore"),
            &json!({"cardIds": [card.id]}),
        )
        .await
        .check_ok();
    let back: Vec<String> = card_attachments(&ts.client, &board.id, &card.id)
        .await
        .into_iter()
        .map(|block| block.id)
        .collect();
    assert_eq!(back, [attachment.id]);

    ts.teardown().await;
}
