//! The writes of the block routes: inserting, patching one block or many, deleting,
//! restoring and duplicating, and what each refuses.

use super::*;

// -- POST /boards/{board_id}/blocks -----------------------------------------------------

/// What an insert answers and what it stores: new identifiers, the user as creator, and
/// server timestamps.
#[tokio::test]
async fn post_blocks_answer_and_stored_block() {
    let th = Setup::native().start().await.init_basic().await;
    let board = create_board_in_store(&th, ContainerVisibility::Private).await;
    let card = insert_one(&th.client, &new_block(&board.id, BlockType::Card, "card")).await;
    let user = th.client.me().await;
    let before = fvoci_server::auth::now_millis();

    let view = new_block(&board.id, BlockType::View, "view");
    let text = child_of(&card, BlockType::Attachment, "text");
    let response = th
        .client
        .insert_blocks(&board.id, &[view.clone(), text])
        .await;
    let answered: Vec<Block> = response.created_json();

    assert_eq!(answered.len(), 2, "both blocks are answered, in order");
    assert_ne!(answered[0].id, view.id, "the view has a new identifier");
    assert!(
        fvoci_model::is_id(&answered[0].id),
        "a generated one: {}",
        answered[0].id
    );
    assert_eq!(
        answered[1].parent_id, card.id,
        "the attachment stays linked to its card"
    );
    for block in &answered {
        assert_eq!(block.created_by, user.id, "created by the user");
        assert_eq!(block.modified_by, user.id, "modified by the user");
        assert!(
            block.created_at >= before,
            "created now, whatever the request says"
        );
        assert!(block.updated_at >= before, "the modification time is now");
    }
    assert_eq!(response.value()[0]["fields"], json!({}), "no null fields");

    let stored = blocks_of(&th.client, &board.id).await;
    assert_eq!(stored, [card, answered[0].clone(), answered[1].clone()]);

    th.teardown().await;
}

/// Blocks may come without identifiers or timestamps, as `NewBlock` allows, and the
/// blocks of one request are listed in the order they were sent in.
#[tokio::test]
async fn post_blocks_without_identifiers_and_in_the_order_of_the_request() {
    let th = Setup::native().start().await.init_basic().await;
    let board = create_board_in_store(&th, ContainerVisibility::Private).await;
    let card = insert_one(&th.client, &new_block(&board.id, BlockType::Card, "anchor")).await;

    let nameless: Vec<Value> = (0..3)
        .map(|index| {
            json!({
                "containerId": board.id,
                "type": "view",
                "title": format!("nameless {index}"),
            })
        })
        .collect();
    let answered: Vec<Block> = th
        .client
        .post(&blocks_path(&board.id), &nameless)
        .await
        .created_json();
    assert_eq!(answered.len(), 3);
    for block in &answered {
        assert!(fvoci_model::is_id(&block.id), "{}", block.id);
        assert_eq!(block.parent_id, "", "a view on the board has no parent");
    }
    assert_ne!(
        answered[0].id, answered[1].id,
        "each has its own identifier"
    );

    let mut request: Vec<Block> = Vec::new();
    for index in 0..20 {
        request.push(child_of(
            &card,
            BlockType::Attachment,
            &format!("text {index}"),
        ));
        request.push(new_block(
            &board.id,
            BlockType::View,
            &format!("view {index}"),
        ));
    }
    let made: Vec<Block> = th
        .client
        .insert_blocks(&board.id, &request)
        .await
        .created_json();
    let listed = blocks_of(&th.client, &board.id).await;
    let in_order = |blocks: &[Block]| -> Vec<String> {
        blocks.iter().map(|block| block.title.clone()).collect()
    };
    let sent = in_order(&request);
    assert_eq!(in_order(&made), sent, "answered as sent");
    let expected: Vec<String> = std::iter::once("anchor".to_owned())
        .chain(in_order(&answered))
        .chain(sent)
        .collect();
    assert_eq!(in_order(&listed), expected, "listed as sent");

    th.teardown().await;
}

/// The refusals of an insert that come before write-role checks: missing/invalid `type`,
/// `type: "card"` (which belongs to `createCard`), and mismatched `containerId`.
#[tokio::test]
async fn post_blocks_validation() {
    let th = Setup::native().start().await.init_basic().await;
    let board = create_board_in_store(&th, ContainerVisibility::Private).await;
    let other_board = create_board_in_store(&th, ContainerVisibility::Private).await;
    let path = blocks_path(&board.id);
    let valid = json!({"id": "x1", "containerId": board.id, "type": "view"});
    let without = |member: &str| {
        let mut block = valid.as_object().expect("the block is an object").clone();
        block.remove(member);
        json!([block])
    };
    let mut elsewhere = valid.clone();
    elsewhere["containerId"] = json!(other_board.id);
    let mut raw_card = valid.clone();
    raw_card["type"] = json!("card");

    let elsewhere_words = "the block is on another board than the path";
    let cases = [
        (without("type"), "the block has no type"),
        (json!([raw_card]), "a card is made by createCard"),
        (without("containerId"), elsewhere_words),
        (json!([elsewhere]), elsewhere_words),
    ];
    for (body, message) in &cases {
        let response = th.client.post(&path, body).await;
        response.check_error(StatusCode::BAD_REQUEST, message);
        // A user who is no member of the private board learns nothing from the body.
        let response = th.client2.post(&path, body).await;
        response.check_error(StatusCode::NOT_FOUND, NO_BOARD);
    }

    // A body that is not a JSON list of blocks (including `null`) is a bad request.
    for body in [
        "not json",
        "null",
        "{\"a\":1}",
        "[null]",
        "[{\"createdAt\":\"x\"}]",
    ] {
        let response = th.client.send_raw_body(Method::POST, &path, body).await;
        response.check_bad_request();
        assert_eq!(response.problem_type(), "/problems/bad-request", "{body}");
    }

    assert!(
        blocks_of(&th.client, &board.id).await.is_empty(),
        "nothing was stored"
    );
    th.teardown().await;
}

/// A request with an empty block list creates nothing and still requires access to the board.
#[tokio::test]
async fn post_blocks_without_blocks() {
    let th = Setup::native().start().await.init_basic().await;
    let board = create_board_in_store(&th, ContainerVisibility::Private).await;
    let path = blocks_path(&board.id);

    let response = th.client.send_raw_body(Method::POST, &path, "[]").await;
    response.check_created();
    assert_eq!(response.text(), "[]", "an empty list for a member");
    for (client, path) in [
        (&th.client2, path.as_str()),
        (
            &th.client2,
            &format!("{TEST_WS}/boards/no-such-board/blocks"),
        ),
        (
            &th.client,
            &format!("{TEST_WS}/boards/no-such-board/blocks"),
        ),
    ] {
        let response = client.send_raw_body(Method::POST, path, "[]").await;
        response.check_error(StatusCode::NOT_FOUND, NO_BOARD);
    }

    let response = th
        .anonymous()
        .send_raw_body(Method::POST, &path, "[]")
        .await;
    response.check_unauthorized();
    assert_eq!(
        response.error_message(),
        "no session token",
        "a session is needed all the same"
    );

    // With a block, the missing board shows as a missing permission.
    let block = new_block("no-such-board", BlockType::Card, "");
    let response = th
        .client
        .post(&format!("{TEST_WS}/boards/no-such-board/blocks"), &[block])
        .await;
    response.check_error(StatusCode::NOT_FOUND, NO_BOARD);

    th.teardown().await;
}

/// Comments need the permission to comment, everything else the permission to manage
/// cards. The comments are on a card of the board.
#[tokio::test]
async fn post_blocks_permissions_by_block_type() {
    let th = Setup::native().start().await.init_basic().await;
    let boards = boards_with_roles(&th).await;
    let board_id = &boards.board.id;
    let on = insert_one(&th.client, &new_block(board_id, BlockType::Card, "card")).await;
    let attachment = || child_of(&on, BlockType::Attachment, "attachment");
    let comment = || child_of(&on, BlockType::Comment, "comment");

    let cases: [(&str, &Client, Vec<Block>, Option<&str>); 8] = [
        (
            "editor, attachment",
            &boards.editor,
            vec![attachment()],
            None,
        ),
        ("editor, comment", &boards.editor, vec![comment()], None),
        (
            "commenter, attachment",
            &boards.commenter,
            vec![attachment()],
            Some(NO_CHANGES),
        ),
        (
            "commenter, comment",
            &boards.commenter,
            vec![comment()],
            None,
        ),
        (
            "commenter, comment and attachment",
            &boards.commenter,
            vec![comment(), attachment()],
            Some(NO_CHANGES),
        ),
        (
            "viewer, attachment",
            &boards.viewer,
            vec![attachment()],
            Some(NO_CHANGES),
        ),
        (
            "viewer, comment",
            &boards.viewer,
            vec![comment()],
            Some(NO_COMMENTS),
        ),
        (
            "admin, attachment and comment",
            &th.client,
            vec![attachment(), comment()],
            None,
        ),
    ];
    let mut expected_count = 1;
    for (case, client, blocks, refusal) in &cases {
        let response = client.insert_blocks(board_id, blocks).await;
        if let Some(message) = refusal {
            response.check_forbidden();
            assert_eq!(response.error_message(), *message, "{case}");
        } else {
            response.check_created();
            expected_count += blocks.len();
        }
    }
    // For who is no member of the private board (the outsider) there is no board to add to.
    for blocks in [vec![attachment()], vec![comment()]] {
        let response = th.client2.insert_blocks(board_id, &blocks).await;
        response.check_error(StatusCode::NOT_FOUND, NO_BOARD);
    }
    assert_eq!(
        blocks_of(&th.client, board_id).await.len(),
        expected_count,
        "only the allowed inserts stored blocks"
    );

    th.teardown().await;
}

/// The title and fields size limits, and what a refused block leaves behind.
#[tokio::test]
async fn post_blocks_size_limits() {
    let th = Setup::native().start().await.init_basic().await;
    let board = create_board_in_store(&th, ContainerVisibility::Private).await;
    let on = insert_one(&th.client, &new_block(&board.id, BlockType::Card, "on")).await;

    let longest = child_of(
        &on,
        BlockType::Attachment,
        &"a".repeat(BLOCK_TITLE_MAX_CHARS),
    );
    insert_one(&th.client, &longest).await;

    let too_long = child_of(
        &on,
        BlockType::Attachment,
        &"a".repeat(BLOCK_TITLE_MAX_CHARS + 1),
    );
    let response = th
        .client
        .insert_blocks(&board.id, std::slice::from_ref(&too_long))
        .await;
    response.check_bad_request();
    let message = response.error_message();
    assert!(
        message.starts_with("error validating block ")
            && message.ends_with(": block title size limit exceeded"),
        "{message}"
    );
    assert!(
        !message.contains(&too_long.id),
        "the message names the generated identifier: {message}"
    );

    let too_big = Block {
        fields: fields(json!({"big": "a".repeat(BLOCK_FIELDS_MAX_CHARS)})),
        ..child_of(&on, BlockType::Attachment, "big fields")
    };
    let response = th.client.insert_blocks(&board.id, &[too_big]).await;
    response.check_bad_request();
    let message = response.error_message();
    assert!(
        message.ends_with(": block fields size limit exceeded"),
        "{message}"
    );

    // A request is stored atomically: whole or not at all.
    let blocks = [
        child_of(&on, BlockType::Attachment, "before"),
        too_long,
        child_of(&on, BlockType::Attachment, "after"),
    ];
    th.client
        .insert_blocks(&board.id, &blocks)
        .await
        .check_bad_request();
    let stored = blocks_of(&th.client, &board.id).await;
    assert_eq!(
        titles(&stored).len(),
        2,
        "nothing of the refused request, next to the card and longest-title attachment"
    );

    th.teardown().await;
}

// -- PATCH /boards/{board_id}/blocks/{block_id} -----------------------------------------

/// Patch refusals are evaluated in order: permission, block existence, then body validation.
#[tokio::test]
async fn patch_block_refusals() {
    let th = Setup::native().start().await.init_basic().await;
    let boards = boards_with_roles(&th).await;
    let content = card_with_content(&th, &boards).await;
    let board_id = &boards.board.id;
    let card_path = block_path(board_id, &content.card.id);

    // A commenter changes their own comment only
    // (`a_comment_is_changed_by_its_author_or_an_admin`), a viewer nothing.
    for (who, client, path) in [
        ("commenter, card", &boards.commenter, card_path.clone()),
        ("viewer", &boards.viewer, card_path.clone()),
    ] {
        let response = client.patch(&path, &title_patch("x")).await;
        response.check_error(StatusCode::FORBIDDEN, NO_CHANGES);
        assert_eq!(response.problem_type(), "/problems/forbidden", "{who}");
    }
    // For who is no member of the private board it does not exist, whatever the block:
    // the outsider, and an editor of one board on another board's path.
    for (client, path) in [
        (&th.client2, card_path.clone()),
        (&th.client2, block_path(board_id, "no-such-block")),
        (
            &boards.editor,
            block_path(&boards.other_board.id, &content.card.id),
        ),
    ] {
        let response = client.patch(&path, &title_patch("x")).await;
        response.check_error(StatusCode::NOT_FOUND, NO_BOARD);
    }

    let response = th
        .client
        .patch(&block_path(board_id, "no-such-block"), &title_patch("x"))
        .await;
    response.check_error(StatusCode::NOT_FOUND, NO_BLOCK);

    let response = th
        .client
        .patch(
            &block_path(board_id, &content.other_card.id),
            &title_patch("x"),
        )
        .await;
    // A block of another board answers as one that does not exist.
    response.check_error(StatusCode::NOT_FOUND, NO_BLOCK);

    // A body that is not a patch object (including lists and `null`) is a bad request.
    for body in ["not json", "null", "{\"title\":5}", "[]", "[\"x\"]"] {
        let response = th
            .client
            .send_raw_body(Method::PATCH, &card_path, body)
            .await;
        response.check_bad_request();
        assert_eq!(response.problem_type(), "/problems/bad-request", "{body}");
    }

    let too_long = title_patch(&"a".repeat(BLOCK_TITLE_MAX_CHARS + 1));
    let response = th.client.patch(&card_path, &too_long).await;
    response.check_bad_request();
    assert_eq!(
        response.error_message(),
        format!(
            "error validating block {}: block title size limit exceeded",
            content.card.id
        ),
        "the title limit"
    );

    let stored = blocks_of(&th.client, board_id).await;
    assert_eq!(
        titles(&stored),
        ["card", "comment", "text"],
        "nothing was patched"
    );

    th.teardown().await;
}

/// What a patch changes and what it leaves alone.
#[tokio::test]
async fn patch_block_changes() {
    let th = Setup::native().start().await.init_basic().await;
    let boards = boards_with_roles(&th).await;
    let content = card_with_content(&th, &boards).await;
    let card = &content.card;
    let card_path = block_path(&boards.board.id, &card.id);
    let created = blocks_of(&th.client, &boards.board.id).await;
    let created_at = find(&created, &card.id).created_at;

    patch(&boards.editor, card, &title_patch("by the editor")).await;
    let stored = blocks_of(&th.client, &boards.board.id).await;
    let patched = find(&stored, &card.id);
    assert_eq!(patched.title, "by the editor", "the editor's title");
    assert_eq!(
        patched.modified_by,
        boards.editor.me().await.id,
        "modified by the editor"
    );
    assert!(patched.updated_at > card.updated_at, "at a later time");
    assert_eq!(patched.created_at, created_at, "created when it was");

    // The empty patch, and members a patch does not have, change nothing but the
    // modifier.
    patch(&th.client, card, &BlockPatch::default()).await;
    let foreign_members = json!({
        "containerId": boards.other_board.id,
        "id": "another",
        "createdBy": "someone",
        "deletedAt": 5,
    });
    th.client
        .patch(&format!("{card_path}?disableNotify=true"), &foreign_members)
        .await
        .check_no_content();
    let stored = blocks_of(&th.client, &boards.board.id).await;
    let patched = find(&stored, &card.id);
    assert_eq!(patched.title, "by the editor", "the title stays");
    assert_eq!(
        patched.container_id, boards.board.id,
        "a patch moves no block"
    );
    assert_eq!(
        patched.created_by, card.created_by,
        "nor changes its creator"
    );
    assert_eq!(
        patched.modified_by,
        th.client.me().await.id,
        "the last patch was the admin's"
    );
    assert_eq!(
        titles(&blocks_of(&th.client, &boards.other_board.id).await),
        ["other"],
        "the other board is as it was"
    );

    th.teardown().await;
}

// -- PATCH /boards/{board_id}/blocks ----------------------------------------------------

/// What a batch patch refuses; none of the refused batches changes anything.
#[tokio::test]
async fn patch_blocks_batch_refusals() {
    let th = Setup::native().start().await.init_basic().await;
    let boards = boards_with_roles(&th).await;
    let content = card_with_content(&th, &boards).await;
    let (card, text) = (&content.card, &content.text);
    let path = blocks_path(&boards.board.id);
    let missing = new_block(&boards.board.id, BlockType::Card, "never stored");

    // A nonexistent block or a block from another board is not found: the batch only
    // operates on blocks of the board named in the path.
    for patches in [
        batch(&[(card, "card 2"), (&missing, "x")]),
        batch(&[(card, "card 2"), (&content.other_card, "other 2")]),
    ] {
        for client in [&th.client, &boards.editor] {
            let response = client.patch(&path, &patches).await;
            response.check_error(StatusCode::NOT_FOUND, NO_BLOCK);
        }
    }

    // Users without card-change permission are refused; non-members of a private board
    // receive 404.
    let response = boards
        .commenter
        .patch(&path, &batch(&[(card, "card 2")]))
        .await;
    response.check_error(StatusCode::FORBIDDEN, NO_CHANGES);
    let response = th.client2.patch(&path, &batch(&[(card, "card 2")])).await;
    response.check_error(StatusCode::NOT_FOUND, NO_BOARD);
    let response = th
        .client
        .patch(
            &format!("{TEST_WS}/boards/no-such-board/blocks"),
            &batch(&[(card, "card 2")]),
        )
        .await;
    response.check_error(StatusCode::NOT_FOUND, NO_BOARD);

    // A duplicate block identifier in the same batch is rejected.
    let response = th
        .client
        .patch(&path, &batch(&[(card, "card 2"), (card, "card 3")]))
        .await;
    response.check_error(StatusCode::BAD_REQUEST, "a block is listed twice");
    assert_eq!(response.refused_at(), "/blockIds/1");

    // The batch must contain exactly as many patches as block identifiers.
    let mut short = batch(&[(card, "card 2"), (text, "text 2")]);
    short.block_patches.pop();
    let mut long = batch(&[(card, "card 2")]);
    long.block_patches.push(title_patch("ignored"));
    for uneven in [&short, &long] {
        let response = th.client.patch(&path, uneven).await;
        response.check_error(
            StatusCode::BAD_REQUEST,
            "blockIds and blockPatches differ in length",
        );
    }

    let too_long = "a".repeat(BLOCK_TITLE_MAX_CHARS + 1);
    let response = th
        .client
        .patch(&path, &batch(&[(card, "card 2"), (text, &too_long)]))
        .await;
    response.check_bad_request();
    assert_eq!(
        response.error_message(),
        format!(
            "error validating block {}: block title size limit exceeded",
            text.id
        ),
        "the title limit"
    );

    // A body that is not a valid patch batch is a bad request.
    for body in ["not json", "null", "{\"blockIds\":5}", "[]", "[[], []]"] {
        let response = th.client.send_raw_body(Method::PATCH, &path, body).await;
        response.check_bad_request();
        assert_eq!(response.problem_type(), "/problems/bad-request", "{body}");
    }

    // A batch is all or none.
    let stored = blocks_of(&th.client, &boards.board.id).await;
    assert_eq!(
        titles(&stored),
        ["card", "comment", "text"],
        "nothing changed"
    );

    th.teardown().await;
}

/// What a batch patch changes, and the batches that change nothing.
#[tokio::test]
async fn patch_blocks_batch_changes() {
    let th = Setup::native().start().await.init_basic().await;
    let boards = boards_with_roles(&th).await;
    let content = card_with_content(&th, &boards).await;
    let (card, text) = (&content.card, &content.text);
    let path = blocks_path(&boards.board.id);

    let patches = batch(&[(card, "card 1"), (text, "text 1")]);
    assert_eq!(
        serde_json::to_value(&patches).expect("a batch is JSON"),
        json!({
            "blockIds": [card.id, text.id],
            "blockPatches": [
                {"title": "card 1", "updatedFields": {}, "deletedFields": []},
                {"title": "text 1", "updatedFields": {}, "deletedFields": []},
            ],
        }),
        "the members of a batch on the wire"
    );
    let response = boards.editor.patch(&path, &patches).await;
    response.check_no_content();
    let stored = blocks_of(&th.client, &boards.board.id).await;
    assert_eq!(
        titles(&stored),
        ["card 1", "comment", "text 1"],
        "both are patched"
    );
    assert_eq!(
        find(&stored, &card.id).modified_by,
        boards.editor.me().await.id,
        "by the editor"
    );

    // An empty batch, and one whose members are spelled differently, do nothing.
    for body in [
        json!({}),
        json!({"blockIds": [], "blockPatches": []}),
        json!({"block_ids": [card.id], "block_patches": [{"title": "snake"}]}),
    ] {
        let response = boards.editor.patch(&path, &body).await;
        assert_eq!(response.status, StatusCode::NO_CONTENT, "{body}");
        assert!(response.body.is_empty(), "{body}: {}", response.text());
    }
    let stored = blocks_of(&th.client, &boards.board.id).await;
    assert_eq!(
        titles(&stored),
        ["card 1", "comment", "text 1"],
        "none of them patched anything"
    );

    let quiet = format!("{path}?disableNotify=true");
    th.client
        .patch(&quiet, &batch(&[(text, "text 2"), (card, "card 2")]))
        .await
        .check_no_content();
    let stored = blocks_of(&th.client, &boards.board.id).await;
    assert_eq!(titles(&stored), ["card 2", "comment", "text 2"], "patched");
    let other = blocks_of(&th.client, &boards.other_board.id).await;
    assert_eq!(titles(&other), ["other"], "the other board is as it was");

    th.teardown().await;
}

// -- DELETE /boards/{board_id}/blocks/{block_id} ----------------------------------------

/// Deleting a block that is no comment needs the permission to manage cards, and
/// nobody below the commenter's role deletes anything; blocks that are not on the
/// board are refused.
#[tokio::test]
async fn delete_block_refusals() {
    let th = Setup::native().start().await.init_basic().await;
    let boards = boards_with_roles(&th).await;
    let content = card_with_content(&th, &boards).await;
    let board_id = &boards.board.id;

    for (who, client, path) in [
        (
            "commenter, a card",
            &boards.commenter,
            block_path(board_id, &content.card.id),
        ),
        (
            "viewer, a comment",
            &boards.viewer,
            block_path(board_id, &content.comment.id),
        ),
        (
            "viewer",
            &boards.viewer,
            block_path(board_id, &content.card.id),
        ),
    ] {
        let response = client.delete(&path).await;
        response.check_error(StatusCode::FORBIDDEN, NO_CHANGES);
        assert_eq!(response.problem_type(), "/problems/forbidden", "{who}");
    }
    // For who is no member of the private board it does not exist, whatever the block.
    for (client, path) in [
        (&th.client2, block_path(board_id, &content.card.id)),
        (&th.client2, block_path(board_id, "no-such-block")),
        (
            &boards.editor,
            block_path(&boards.other_board.id, &content.card.id),
        ),
    ] {
        let response = client.delete(&path).await;
        response.check_error(StatusCode::NOT_FOUND, NO_BOARD);
    }

    let response = th
        .client
        .delete(&block_path(board_id, "no-such-block"))
        .await;
    response.check_error(StatusCode::NOT_FOUND, NO_BLOCK);

    let response = th
        .client
        .delete(&block_path(board_id, &content.other_card.id))
        .await;
    // A block of another board answers as one that does not exist.
    response.check_error(StatusCode::NOT_FOUND, NO_BLOCK);

    assert_eq!(
        blocks_of(&th.client, board_id).await.len(),
        3,
        "nothing was deleted"
    );
    assert_eq!(
        blocks_of(&th.client, &boards.other_board.id).await.len(),
        1,
        "on the other board neither"
    );

    th.teardown().await;
}

/// Deleting a block takes its direct children along, images included, and a deleted
/// block is gone for the next deletion.
#[tokio::test]
async fn delete_block_takes_the_children_along() {
    let th = Setup::native().start().await.init_basic().await;
    let boards = boards_with_roles(&th).await;
    let content = card_with_content(&th, &boards).await;
    let board_id = &boards.board.id;
    let image = Block {
        fields: fields(json!({"fileId": "7abcdefghijklmnopqrstuvwxyz.png"})),
        ..child_of(&content.card, BlockType::Attachment, "image")
    };
    insert_one(&th.client, &image).await;
    insert_one(&th.client, &new_block(board_id, BlockType::Card, "card 2")).await;

    // The comment is the commenter's; an admin may delete it.
    delete(&th.client, &content.comment).await;
    let stored = blocks_of(&th.client, board_id).await;
    assert_eq!(
        titles(&stored),
        ["card", "card 2", "image", "text"],
        "the comment is gone"
    );

    // Deleting a card also deletes all of its descendants (including image blocks).
    let response = boards
        .editor
        .delete(&format!(
            "{}?disableNotify=true",
            block_path(board_id, &content.card.id)
        ))
        .await;
    response.check_no_content();
    let stored = blocks_of(&th.client, board_id).await;
    assert_eq!(titles(&stored), ["card 2"], "only the other card is left");

    for block in [&content.card, &content.text] {
        let response = th.client.delete(&block_path(board_id, &block.id)).await;
        response.check_error(StatusCode::NOT_FOUND, NO_BLOCK);
    }

    th.teardown().await;
}

// -- POST /boards/{board_id}/blocks/{block_id}/restore ---------------------------------

/// Undelete refusals are checked in order (board, block, permission), and undeleting
/// restores the block.
#[tokio::test]
async fn undelete_block_refusals_and_children() {
    let th = Setup::native().start().await.init_basic().await;
    let boards = boards_with_roles(&th).await;
    let content = card_with_content(&th, &boards).await;
    let (board_id, card) = (&boards.board.id, &content.card);
    delete(&th.client, card).await;

    let response = th
        .client
        .post_empty(&undelete_path("no-such-board", &card.id))
        .await;
    response.check_error(StatusCode::NOT_FOUND, NO_BOARD);

    // A block that never existed is not found; for a non-member the board is not found either.
    let response = th
        .client
        .post_empty(&undelete_path(board_id, "no-such-block"))
        .await;
    response.check_error(StatusCode::NOT_FOUND, NO_BLOCK);
    let response = th
        .client2
        .post_empty(&undelete_path(board_id, "no-such-block"))
        .await;
    response.check_error(StatusCode::NOT_FOUND, NO_BOARD);

    let response = th
        .client
        .post_empty(&undelete_path(&boards.other_board.id, &card.id))
        .await;
    // A block of another board answers as one that does not exist.
    response.check_error(StatusCode::NOT_FOUND, NO_BLOCK);

    for client in [&boards.commenter, &boards.viewer] {
        let response = client.post_empty(&undelete_path(board_id, &card.id)).await;
        response.check_error(StatusCode::FORBIDDEN, NO_CHANGES);
    }
    let response = th
        .client2
        .post_empty(&undelete_path(board_id, &card.id))
        .await;
    response.check_error(StatusCode::NOT_FOUND, NO_BOARD);
    assert!(
        blocks_of(&th.client, board_id).await.is_empty(),
        "still deleted"
    );

    let undeleted: Block = boards
        .editor
        .post_empty(&undelete_path(board_id, &card.id))
        .await
        .ok_json();
    assert_eq!(undeleted.id, card.id, "the card is answered");
    let stored = blocks_of(&th.client, board_id).await;
    assert_eq!(
        titles(&stored),
        ["card", "comment", "text"],
        "the children are back too"
    );

    // A block that is not deleted is not in the trash: there is nothing to restore.
    th.client
        .post_empty(&undelete_path(board_id, &card.id))
        .await
        .check_error(StatusCode::NOT_FOUND, "block not found");
    assert_eq!(
        blocks_of(&th.client, board_id).await.len(),
        3,
        "and nothing changes"
    );

    th.teardown().await;
}

// -- POST /boards/{board_id}/blocks/{block_id}/duplicate --------------------------------

/// A comment needs the permission to comment and everything else the permission to
/// manage cards; the board and the block are looked up first.
#[tokio::test]
async fn duplicate_block_refusals() {
    let th = Setup::native().start().await.init_basic().await;
    let boards = boards_with_roles(&th).await;
    let content = card_with_content(&th, &boards).await;
    let (board_id, card) = (&boards.board.id, &content.card);

    let response = th
        .client
        .post_empty(&duplicate_path("no-such-board", &card.id))
        .await;
    response.check_error(StatusCode::NOT_FOUND, NO_BOARD);

    let response = th
        .client
        .post_empty(&duplicate_path(board_id, "no-such-block"))
        .await;
    response.check_error(StatusCode::NOT_FOUND, NO_BLOCK);

    let response = th
        .client
        .post_empty(&duplicate_path(board_id, &content.other_card.id))
        .await;
    // A block of another board answers as one that does not exist.
    response.check_error(StatusCode::NOT_FOUND, NO_BLOCK);

    for client in [&boards.commenter, &boards.viewer] {
        let response = client.post_empty(&duplicate_path(board_id, &card.id)).await;
        response.check_error(StatusCode::FORBIDDEN, "access denied to modify board cards");
    }
    let response = th
        .client2
        .post_empty(&duplicate_path(board_id, &card.id))
        .await;
    response.check_error(StatusCode::NOT_FOUND, NO_BOARD);
    let comment_path = duplicate_path(board_id, &content.comment.id);
    let response = boards.viewer.post_empty(&comment_path).await;
    response.check_forbidden();
    assert_eq!(
        response.error_message(),
        "access denied to comment on board cards",
        "a viewer and a comment"
    );

    // A commenter may comment, but comments cannot be duplicated.
    let response = boards.commenter.post_empty(&comment_path).await;
    response.check_error(StatusCode::BAD_REQUEST, "a comment cannot be duplicated");

    assert_eq!(
        blocks_of(&th.client, board_id).await.len(),
        3,
        "nothing was copied"
    );
    th.teardown().await;
}

/// Block insert requests can exceed Axum's default 2 MB body limit: three blocks that
/// are each within the fields limit produce a payload larger than 2 MB and are accepted.
#[tokio::test]
async fn post_blocks_larger_than_two_megabytes() {
    let th = Setup::one_user().start().await;
    let board = create_board_in_store(&th, ContainerVisibility::Open).await;
    // Views, whose fields may be that large.
    let big = |title: &str| Block {
        fields: fields(json!({"groupBy": "a".repeat(750_000)})),
        ..new_block(&board.id, BlockType::View, title)
    };
    let blocks = [big("1"), big("2"), big("3")];
    let size = serde_json::to_vec(&blocks).expect("blocks are JSON").len();
    assert!(size > 2 * 1024 * 1024, "the body has {size} bytes");

    let inserted = insert(&th.client, &board.id, &blocks).await;
    assert_eq!(titles(&inserted), ["1", "2", "3"], "all three are answered");

    // A patch is not limited either.
    let grow = BlockPatch {
        updated_fields: fields(json!({"dateProperty": "b".repeat(40_000)})),
        ..BlockPatch::default()
    };
    patch(&th.client, &inserted[0], &grow).await;
    let stored = blocks_of(&th.client, &board.id).await;
    assert_eq!(stored.len(), 3, "all three are stored");

    th.teardown().await;
}

/// Duplicating a card copies it and its content, not its comments, under new
/// identifiers.
#[tokio::test]
async fn duplicate_block_copies() {
    let th = Setup::native().start().await.init_basic().await;
    let boards = boards_with_roles(&th).await;
    let content = card_with_content(&th, &boards).await;
    let (board_id, card) = (&boards.board.id, &content.card);
    let path = duplicate_path(board_id, &card.id);

    let copies: Vec<Block> = boards.editor.post_empty(&path).await.created_json();
    assert_eq!(
        titles(&copies),
        ["card", "text"],
        "the comment is not copied"
    );
    let (card_copy, text_copy) = (&copies[0], &copies[1]);
    assert_eq!(
        card_copy.block_type,
        BlockType::Card,
        "the block itself is first"
    );
    assert_ne!(card_copy.id, card.id, "under a new identifier");
    assert_ne!(text_copy.id, content.text.id, "the child too");
    assert_eq!(
        text_copy.parent_id, card_copy.id,
        "the child follows the copy"
    );
    assert_eq!(
        card_copy.fields["isTemplate"],
        json!(false),
        "an ordinary copy"
    );
    assert_eq!(
        card_copy.created_by,
        boards.editor.me().await.id,
        "the copy is the editor's"
    );

    let copies: Vec<Block> = th
        .client
        .post_empty(&format!("{path}?asTemplate=true"))
        .await
        .created_json();
    assert_eq!(copies[0].fields["isTemplate"], json!(true), "a template");

    let stored = blocks_of(&th.client, board_id).await;
    assert_eq!(
        titles(&stored),
        ["card", "card", "card", "comment", "text", "text", "text"],
        "both copies are stored"
    );
    assert_eq!(
        find(&stored, &card.id).fields.get("isTemplate"),
        Some(&json!(false)),
        "the source is untouched"
    );

    th.teardown().await;
}
