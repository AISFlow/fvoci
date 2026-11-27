//! Who is told of a change of a block: the connections that follow it and the webhooks.

use super::*;

// -- who is told about a change -----------------------------------------------------------

/// Every change of a block—including batch patches—is broadcast to the board's members
/// who listen to its workspace, with or without `disableNotify`.
#[tokio::test]
async fn block_changes_are_broadcast() {
    let th = Setup::native().start().await.init_basic().await;
    let board = create_board_in_store(&th, ContainerVisibility::Private).await;
    let mut member = Socket::subscribed_to_workspace(&th, &th.client, TEST_WORKSPACE_ID).await;
    let mut outsider = Socket::subscribed_to_workspace(&th, &th.client2, TEST_WORKSPACE_ID).await;
    let path = blocks_path(&board.id);

    // Insert: one message per block, in order.
    let card = new_block(&board.id, BlockType::Card, "card");
    let text = child_of(&card, BlockType::Attachment, "text");
    let inserted = insert(&th.client, &board.id, &[card, text]).await;
    let (card, text) = (&inserted[0], &inserted[1]);
    for block in [card, text] {
        let announced = member.next_block().await;
        assert_eq!(announced["id"], json!(block.id), "{announced}");
        assert_eq!(announced["title"], json!(block.title), "{announced}");
        assert_eq!(announced["containerId"], json!(board.id), "{announced}");
    }
    let quiet_view = new_block(&board.id, BlockType::View, "quiet view");
    let quiet: Vec<Block> = th
        .client
        .post(&format!("{path}?disableNotify=true"), &[quiet_view])
        .await
        .created_json();
    let announced = member.next_block().await;
    assert_eq!(announced["id"], json!(quiet[0].id), "{announced}");

    // Patch.
    patch(&th.client, card, &title_patch("patched")).await;
    let announced = member.next_block().await;
    assert_eq!(announced["id"], json!(card.id), "{announced}");
    assert_eq!(announced["title"], json!("patched"), "{announced}");
    th.client
        .patch(
            &format!("{}?disableNotify=true", block_path(&board.id, &card.id)),
            &title_patch("patched quietly"),
        )
        .await
        .check_no_content();
    let announced = member.next_block().await;
    assert_eq!(announced["title"], json!("patched quietly"), "{announced}");

    // A batch patch is announced block by block, like single patches.
    th.client
        .patch(&path, &batch(&[(card, "batch 1"), (text, "batch 2")]))
        .await
        .check_no_content();
    let mut announced = vec![member.next_block().await, member.next_block().await];
    announced.sort_by_key(|block| block["title"].as_str().map(str::to_owned));
    assert_eq!(announced[0]["id"], json!(card.id), "{announced:?}");
    assert_eq!(announced[0]["title"], json!("batch 1"), "{announced:?}");
    assert_eq!(announced[1]["id"], json!(text.id), "{announced:?}");
    assert_eq!(announced[1]["title"], json!("batch 2"), "{announced:?}");

    // Duplicate: one message per copy.
    let copies: Vec<Block> = th
        .client
        .post_empty(&duplicate_path(&board.id, &card.id))
        .await
        .created_json();
    assert_eq!(copies.len(), 2, "the card and its text");
    for copy in &copies {
        let announced = member.next_block().await;
        assert_eq!(announced["id"], json!(copy.id), "{announced}");
    }

    // Delete: a block with only its identifiers and the deletion time.
    delete(&th.client, card).await;
    let announced = member.next_block().await;
    assert_eq!(announced["id"], json!(card.id), "{announced}");
    assert_eq!(announced["containerId"], json!(board.id), "{announced}");
    assert_eq!(announced["title"], json!(""), "{announced}");
    assert!(announced["deletedAt"].as_i64() > Some(0), "{announced}");

    // Undelete: the block as restored.
    th.client
        .post_empty(&undelete_path(&board.id, &card.id))
        .await
        .check_ok();
    let announced = member.next_block().await;
    assert_eq!(announced["id"], json!(card.id), "{announced}");
    assert_eq!(announced["title"], json!("batch 1"), "{announced}");
    assert_eq!(announced.get("deletedAt"), None, "{announced}");

    member.expect_quiet(&th, "nothing else was announced").await;
    outsider
        .expect_quiet(&th, "a user who is no member of the board hears nothing")
        .await;

    th.teardown().await;
}

/// Inserted, patched and undeleted blocks are posted to the webhook; deletions and
/// duplicates are not.
#[tokio::test]
async fn block_changes_call_the_webhook() {
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

    let card = new_block(&board.id, BlockType::Card, "card");
    let text = child_of(&card, BlockType::Attachment, "text");
    let inserted = insert(&th.client, &board.id, &[card, text]).await;
    let (card, text) = (&inserted[0], &inserted[1]);
    assert_eq!(
        take_posted().await,
        [pair(card, "card"), pair(text, "text")],
        "inserted blocks, in order"
    );

    th.client
        .patch(
            &format!("{}?disableNotify=true", block_path(&board.id, &card.id)),
            &title_patch("patched"),
        )
        .await
        .check_no_content();
    assert_eq!(
        take_posted().await,
        [pair(card, "patched")],
        "a patched block"
    );

    th.client
        .patch(
            &blocks_path(&board.id),
            &batch(&[(card, "batch 1"), (text, "batch 2")]),
        )
        .await
        .check_no_content();
    assert_eq!(
        take_posted().await,
        [pair(card, "batch 1"), pair(text, "batch 2")],
        "the blocks of a batch patch, in order"
    );

    th.client
        .post_empty(&duplicate_path(&board.id, &text.id))
        .await
        .check_created();
    assert_eq!(take_posted().await, nothing, "duplicates are not posted");

    delete(&th.client, text).await;
    assert_eq!(take_posted().await, nothing, "deletions are not posted");

    th.client
        .post_empty(&undelete_path(&board.id, &text.id))
        .await
        .check_ok();
    assert_eq!(
        take_posted().await,
        [pair(text, "batch 2")],
        "an undeleted block"
    );

    th.teardown().await;
    serving.abort();
}

/// `sourceBoardId` triggers copying files referenced by inserted blocks from that board;
/// the caller must have read access to the source board, otherwise the request is refused
/// before anything is copied or inserted.
#[tokio::test]
async fn post_blocks_with_a_source_board_the_user_cannot_read_is_refused() {
    let th = Setup::native().start().await.init_basic().await;
    let foreign = create_board_in_store(&th, ContainerVisibility::Private).await;
    let outsider = &th.client2;
    let outsider_id = outsider.me().await.id;
    let mut own = Container {
        id: new_id(),
        workspace_id: TEST_WORKSPACE_ID.to_owned(),
        visibility: ContainerVisibility::Private,
        ..Container::default()
    };
    th.app
        .store()
        .insert_container_with_admin(&mut own, &outsider_id)
        .await
        .expect("the outsider's own board is stored");

    let image = Block {
        fields: fields(json!({"fileId": "7aaaaaaaaaaaaaaaaaaaaaaaaaa.png"})),
        ..new_block(&own.id, BlockType::Attachment, "")
    };
    for source in [foreign.id.as_str(), "no-such-board"] {
        let response = outsider
            .request(
                Method::POST,
                &format!("{TEST_WS}/boards/{}/blocks?sourceBoardId={source}", own.id),
            )
            .json(&[&image])
            .send()
            .await
            .expect("the server answers");
        // One answer for a source that is not there and one the user cannot read.
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "source {source}");
        let body: Value = response.json().await.expect("an error body");
        assert_eq!(
            body["type"],
            json!("/problems/not-found"),
            "source {source}"
        );
        assert_eq!(
            body["detail"],
            json!("source board not found"),
            "source {source}"
        );
    }
    let stored = th
        .app
        .blocks_for_container(&own.id)
        .await
        .expect("the board's blocks are read");
    assert!(stored.is_empty(), "nothing was inserted: {stored:?}");
}

/// The legitimate use of `sourceBoardId`: copying from a board the user is on.
#[tokio::test]
async fn post_blocks_with_a_source_board_the_user_can_read_still_works() {
    let th = Setup::native().start().await.init_basic().await;
    let source = create_board_in_store(&th, ContainerVisibility::Private).await;
    let target = create_board_in_store(&th, ContainerVisibility::Private).await;

    let view = new_block(&target.id, BlockType::View, "copied view");
    let response = th
        .client
        .request(
            Method::POST,
            &format!(
                "{TEST_WS}/boards/{}/blocks?sourceBoardId={}",
                target.id, source.id
            ),
        )
        .json(&[&view])
        .send()
        .await
        .expect("the server answers");
    assert_eq!(
        response.status(),
        StatusCode::CREATED,
        "a member copies from their own board"
    );
    let inserted: Vec<Block> = response.json().await.expect("the inserted blocks");
    assert_eq!(inserted.len(), 1, "the view was inserted");
    assert_eq!(inserted[0].title, "copied view", "as sent");
}
