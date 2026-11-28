//! Who is told of a change of a card: the connections that follow its board and the
//! webhooks.

use super::*;

/// A text property `note` added to the cards of `board_id` by `client`.
async fn with_note(client: &Client, board_id: &str) {
    client
        .patch(
            &format!("{TEST_WS}/containers/{board_id}"),
            &json!({"updatedCardProperties": [{"id": "note", "name": "Note", "type": "text", "options": []}]}),
        )
        .await
        .check_ok();
}

/// The path of the cards of `board_id`.
fn cards_path(board_id: &str) -> String {
    format!("{TEST_WS}/boards/{board_id}/cards")
}

/// The path of the card `card_id`.
fn card_path(card_id: &str) -> String {
    format!("{TEST_WS}/cards/{card_id}")
}

/// The note `note` given to each of `cards` of `board_id` in one batch.
async fn batch_note(client: &Client, board_id: &str, cards: &[&Block], note: &str) {
    let card_ids: Vec<&str> = cards.iter().map(|card| card.id.as_str()).collect();
    client
        .post(
            &format!("{}/batch-patch", cards_path(board_id)),
            &json!({"cardIds": card_ids, "updatedProperties": {"note": note}}),
        )
        .await
        .check_ok();
}

/// `cards` of `board_id` taken to the trash (`batch-delete`) or brought back from it
/// (`batch-restore`).
async fn batch(client: &Client, board_id: &str, cards: &[&Block], what: &str) {
    let card_ids: Vec<&str> = cards.iter().map(|card| card.id.as_str()).collect();
    let response = client
        .post(
            &format!("{}/{what}", cards_path(board_id)),
            &json!({"cardIds": card_ids}),
        )
        .await;
    assert!(response.status.is_success(), "{what}: {}", response.text());
}

/// Every change of a card, of one alone or of a batch, is broadcast to the board's
/// members who listen to its workspace, with or without `disableNotify`.
#[tokio::test]
async fn card_changes_are_broadcast() {
    let th = Setup::native().start().await.init_basic().await;
    let board = create_board_in_store(&th, ContainerVisibility::Private).await;
    with_note(&th.client, &board.id).await;
    let mut member = Socket::subscribed_to_workspace(&th, &th.client, TEST_WORKSPACE_ID).await;
    let mut outsider = Socket::subscribed_to_workspace(&th, &th.client2, TEST_WORKSPACE_ID).await;
    // The change of the board's properties went out before the sockets listened.
    while member.next_json(QUIET).await.is_some() {}

    // Made: one message per block, in order.
    let card = new_block(&board.id, BlockType::Card, "card");
    let text = child_of(&card, BlockType::Attachment, "text.txt");
    let made = insert(&th.client, &board.id, &[card, text]).await;
    let (card, text) = (&made[0], &made[1]);
    for block in [card, text] {
        let announced = member.next_block().await;
        assert_eq!(announced["id"], json!(block.id), "{announced}");
        assert_eq!(announced["title"], json!(block.title), "{announced}");
        assert_eq!(announced["containerId"], json!(board.id), "{announced}");
    }
    let quiet: Card = th
        .client
        .post(
            &format!("{}?disableNotify=true", cards_path(&board.id)),
            &json!({"title": "quiet card"}),
        )
        .await
        .created_json();
    let announced = member.next_block().await;
    assert_eq!(announced["id"], json!(quiet.id), "{announced}");
    let quiet = Block::from(quiet);

    // Changed alone.
    for (query, title) in [("", "patched"), ("?disableNotify=true", "patched quietly")] {
        th.client
            .patch(
                &format!("{}{query}", card_path(&card.id)),
                &json!({"title": title}),
            )
            .await
            .check_ok();
        let announced = member.next_block().await;
        assert_eq!(announced["id"], json!(card.id), "{announced}");
        assert_eq!(announced["title"], json!(title), "{announced}");
    }

    // A batch is announced card by card, like single changes.
    batch_note(&th.client, &board.id, &[card, &quiet], "batched").await;
    let mut announced = vec![member.next_block().await, member.next_block().await];
    announced.sort_by_key(|block| block["id"].as_str().map(str::to_owned));
    let mut expected = [card.id.as_str(), quiet.id.as_str()];
    expected.sort_unstable();
    for (block, id) in announced.iter().zip(expected) {
        assert_eq!(block["id"], json!(id), "{announced:?}");
        assert_eq!(
            block["fields"]["properties"]["note"],
            json!("batched"),
            "{announced:?}"
        );
    }

    // Copied: one message per copy, the card's and its attachment's.
    let copy: Card = th
        .client
        .post_empty(&format!("{}/{}/duplicate", cards_path(&board.id), card.id))
        .await
        .created_json();
    let copies = [member.next_block().await, member.next_block().await];
    assert!(
        copies.iter().any(|block| block["id"] == json!(copy.id)),
        "the copy of the card: {copies:?}"
    );
    assert!(
        copies.iter().all(|block| block["id"] != json!(text.id)),
        "the attachment is copied, not announced as itself: {copies:?}"
    );

    // Deleted: a block with only its identifiers and the deletion time.
    batch(&th.client, &board.id, &[card], "batch-delete").await;
    let announced = member.next_block().await;
    assert_eq!(announced["id"], json!(card.id), "{announced}");
    assert_eq!(announced["containerId"], json!(board.id), "{announced}");
    assert_eq!(announced["title"], json!(""), "{announced}");
    assert!(announced["deletedAt"].as_i64() > Some(0), "{announced}");

    // Restored: the card as it comes back.
    batch(&th.client, &board.id, &[card], "batch-restore").await;
    let announced = member.next_block().await;
    assert_eq!(announced["id"], json!(card.id), "{announced}");
    assert_eq!(announced["title"], json!("patched quietly"), "{announced}");
    assert_eq!(announced.get("deletedAt"), None, "{announced}");

    // What a restore brings back with the card is told too.
    while member.next_json(QUIET).await.is_some() {}
    member.expect_quiet(&th, "nothing else was announced").await;
    outsider
        .expect_quiet(&th, "a user who is no member of the board hears nothing")
        .await;

    th.teardown().await;
}

/// Made, changed, copied and restored cards are posted to the webhook, one alone or a
/// batch; deletions are not.
#[tokio::test]
async fn card_changes_call_the_webhook() {
    let (posted, serving) = start_webhook_receiver().await;
    let th = Setup::native()
        .configure(|config| {
            config.block_change_webhooks = WEBHOOK_URL.get().into_iter().cloned().collect();
        })
        .start()
        .await
        .init_basic()
        .await;
    let board = create_board_in_store(&th, ContainerVisibility::Private).await;
    with_note(&th.client, &board.id).await;
    // What the webhook got since the last call, as (identifier, title) pairs.
    let take_posted = async || -> Vec<(String, String)> {
        assert!(
            th.app.finish_background(BACKGROUND).await,
            "the webhook calls finish"
        );
        std::mem::take(&mut *posted.lock().expect("lock poisoned"))
            .iter()
            .map(|block| {
                let text = |member: &str| block[member].as_str().unwrap_or_default().to_owned();
                (text("id"), text("title"))
            })
            .collect()
    };
    let pair = |block: &Block, title: &str| (block.id.clone(), title.to_owned());
    let nothing: [(String, String); 0] = [];

    let made = insert(
        &th.client,
        &board.id,
        &[
            new_block(&board.id, BlockType::Card, "first"),
            new_block(&board.id, BlockType::Card, "second"),
        ],
    )
    .await;
    let (first, second) = (&made[0], &made[1]);
    assert_eq!(
        take_posted().await,
        [pair(first, "first"), pair(second, "second")],
        "made cards, in order"
    );

    th.client
        .patch(
            &format!("{}?disableNotify=true", card_path(&first.id)),
            &json!({"title": "patched"}),
        )
        .await
        .check_ok();
    assert_eq!(
        take_posted().await,
        [pair(first, "patched")],
        "a changed card"
    );

    batch_note(&th.client, &board.id, &[first, second], "batched").await;
    assert_eq!(
        take_posted().await,
        [pair(first, "patched"), pair(second, "second")],
        "the cards of a batch, in order"
    );

    let copy: Value = th
        .client
        .post_empty(&format!(
            "{}/{}/duplicate",
            cards_path(&board.id),
            second.id
        ))
        .await
        .created_json();
    let copy_id = copy["id"]
        .as_str()
        .expect("the copy's identifier")
        .to_owned();
    assert_ne!(copy_id, second.id, "a copy of its own");
    assert_eq!(
        take_posted().await,
        [(copy_id, "second".to_owned())],
        "the copy, as it is made"
    );

    batch(&th.client, &board.id, &[second], "batch-delete").await;
    assert_eq!(take_posted().await, nothing, "deletions are not posted");

    batch(&th.client, &board.id, &[second], "batch-restore").await;
    assert_eq!(
        take_posted().await,
        [pair(second, "second")],
        "a restored card"
    );

    th.teardown().await;
    serving.abort();
}
