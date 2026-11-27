//! Who may change, move, delete and bring back a comment: its author from the
//! commenter's role up, and somebody else's by the board's admins only.

use fvoci_model::{Block, BlockType};
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::blocks::{
    NO_CHANGES, QUIET, Socket, batch, block_path, blocks_of, blocks_path, boards_with_roles,
    card_with_content, child_of, delete, find, insert_one, new_block, title_patch, titles,
    undelete_path,
};
use crate::common::TEST_WS;
use crate::common::{Setup, TEST_WORKSPACE_ID};

/// A comment can only be deleted by its author or by a board admin, not by another editor.
#[tokio::test]
async fn delete_comment_of_another_user_needs_the_permission_to_delete_others_comments() {
    let th = Setup::native().start().await.init_basic().await;
    let boards = boards_with_roles(&th).await;
    let content = card_with_content(&th, &boards).await;
    let board_id = &boards.board.id;

    // The comment was written by the commenter.
    let response = boards
        .editor
        .delete(&block_path(board_id, &content.comment.id))
        .await;
    response.check_forbidden();
    assert_eq!(
        response.error_message(),
        "access denied to delete other users' comments",
        "an editor is refused"
    );
    let stored = blocks_of(&th.client, board_id).await;
    assert!(
        titles(&stored).contains(&"comment"),
        "the comment is still there: {:?}",
        titles(&stored)
    );

    // Other blocks of other users are deleted by an editor as before.
    delete(&boards.editor, &content.text).await;
}

/// The cases that stay allowed: an editor deletes a comment of their own, and an admin
/// deletes the comment of another user.
#[tokio::test]
async fn delete_comment_by_its_author_or_by_an_admin_works() {
    let th = Setup::native().start().await.init_basic().await;
    let boards = boards_with_roles(&th).await;
    let content = card_with_content(&th, &boards).await;
    let board_id = &boards.board.id;

    let own = child_of(&content.card, BlockType::Comment, "the editor's comment");
    let own = insert_one(&boards.editor, &own).await;
    delete(&boards.editor, &own).await;

    // The commenter's comment, deleted by the board's admin.
    delete(&th.client, &content.comment).await;

    let stored = blocks_of(&th.client, board_id).await;
    assert_eq!(titles(&stored), ["card", "text"], "both comments are gone");
}

/// A commenter deletes their own comment and brings it back, and does neither to anything
/// else: not to somebody else's comment, not to a card, and not to their comment that went
/// with a
/// card, since the card decides what comes back.
#[tokio::test]
async fn a_commenter_deletes_and_brings_back_their_own_comment() {
    let th = Setup::native().start().await.init_basic().await;
    let boards = boards_with_roles(&th).await;
    let content = card_with_content(&th, &boards).await;
    let board_id = &boards.board.id;
    let own = &content.comment;

    delete(&boards.commenter, own).await;
    assert!(!titles(&blocks_of(&th.client, board_id).await).contains(&"comment"));
    boards
        .commenter
        .post(&undelete_path(board_id, &own.id), &json!({}))
        .await
        .check_ok();
    assert!(titles(&blocks_of(&th.client, board_id).await).contains(&"comment"));

    let editors = insert_one(
        &boards.editor,
        &child_of(&content.card, BlockType::Comment, "the editor's"),
    )
    .await;
    boards
        .commenter
        .delete(&block_path(board_id, &editors.id))
        .await
        .check_error(
            StatusCode::FORBIDDEN,
            "access denied to delete other users' comments",
        );
    boards
        .commenter
        .delete(&block_path(board_id, &content.card.id))
        .await
        .check_error(StatusCode::FORBIDDEN, NO_CHANGES);

    // The card, deleted by the editor with the commenter's comment on it.
    delete(&boards.editor, &content.card).await;
    for id in [&content.card.id, &own.id] {
        boards
            .commenter
            .post(&undelete_path(board_id, id), &json!({}))
            .await
            .check_error(StatusCode::FORBIDDEN, NO_CHANGES);
    }
    assert!(
        blocks_of(&th.client, board_id).await.is_empty(),
        "all still in the trash"
    );
}

/// The refusal of a comment that is not on a card.
const OFF_A_CARD: &str =
    "the parent of a comment is a card of the board, a page of the wiki or a unit of the course";
/// The refusal of a block below a comment.
const BELOW_A_COMMENT: &str = "a comment is the parent of no block";

/// A comment is always on a card of its board and nothing is below it, however a block is
/// written: a new comment, a block that becomes one and a block that moves are refused
/// (400) at the `parentId` of the request, and a card that has comments stays a card.
#[tokio::test]
async fn a_comment_is_on_a_card_and_has_nothing_below_it() {
    let th = Setup::native().start().await.init_basic().await;
    let boards = boards_with_roles(&th).await;
    let content = card_with_content(&th, &boards).await;
    let board_id = &boards.board.id;
    let card = &content.card;
    let comment = &content.comment;

    let attachment = insert_one(
        &th.client,
        &child_of(card, BlockType::Attachment, "attachment"),
    )
    .await;
    for (new, refusal) in [
        (
            new_block(board_id, BlockType::Comment, "on the board"),
            OFF_A_CARD,
        ),
        (
            child_of(&attachment, BlockType::Comment, "on an attachment"),
            OFF_A_CARD,
        ),
        (child_of(comment, BlockType::Comment, "a reply"), OFF_A_CARD),
        (
            child_of(comment, BlockType::Attachment, "below"),
            BELOW_A_COMMENT,
        ),
    ] {
        let response = th.client.insert_blocks(board_id, &[new]).await;
        response.check_error(StatusCode::BAD_REQUEST, refusal);
        assert_eq!(response.refused_at(), "/0/parentId");
    }

    // A move, alone, in a batch and with the board. The board is no parent of anything.
    let path = block_path(board_id, &comment.id);
    for (target, refusal) in [
        (board_id.as_str(), "the parent is not a block of the board"),
        (attachment.id.as_str(), OFF_A_CARD),
    ] {
        let response = th.client.patch(&path, &json!({"parentId": target})).await;
        response.check_error(StatusCode::BAD_REQUEST, refusal);
        assert_eq!(response.refused_at(), "/parentId");
    }
    let under_comment = json!({"parentId": comment.id});
    let response = th
        .client
        .patch(&block_path(board_id, &attachment.id), &under_comment)
        .await;
    response.check_error(StatusCode::BAD_REQUEST, BELOW_A_COMMENT);
    assert_eq!(response.refused_at(), "/parentId");
    let response = th
        .client
        .patch(
            &blocks_path(board_id),
            &json!({"blockIds": [attachment.id], "blockPatches": [under_comment]}),
        )
        .await;
    response.check_error(StatusCode::BAD_REQUEST, BELOW_A_COMMENT);
    assert_eq!(response.refused_at(), "/blockPatches/0/parentId");
    let response = th
        .client
        .patch(
            &format!("{TEST_WS}/containers-and-blocks"),
            &json!({
                "containerIds": [board_id], "containerPatches": [{}],
                "blockIds": [attachment.id], "blockPatches": [under_comment],
            }),
        )
        .await;
    response.check_error(StatusCode::BAD_REQUEST, BELOW_A_COMMENT);
    assert_eq!(response.refused_at(), "/blockPatches/0/parentId");

    // An attachment cannot be placed below another attachment either, and a change of
    // type is always refused on any existing block.
    let nested_attachment = th
        .client
        .insert_blocks(
            board_id,
            &[child_of(
                &attachment,
                BlockType::Attachment,
                "below the attachment",
            )],
        )
        .await;
    nested_attachment.check_bad_request();
    assert_eq!(nested_attachment.refused_at(), "/0/parentId");
    let view = insert_one(&th.client, &new_block(board_id, BlockType::View, "view")).await;
    for (block, to) in [(&attachment, "comment"), (card, "view"), (&view, "comment")] {
        let response = th
            .client
            .patch(&block_path(board_id, &block.id), &json!({"type": to}))
            .await;
        response.check_error(StatusCode::BAD_REQUEST, "a block's type does not change");
        assert_eq!(response.refused_at(), "/type", "{to}");
    }

    // On a card it is all as before: a comment is made and moved to another card.
    let second = insert_one(&th.client, &new_block(board_id, BlockType::Card, "second")).await;
    th.client
        .patch(&path, &json!({"parentId": second.id}))
        .await
        .check_no_content();
}

/// A block comes back from the trash only where it may be: a comment whose card became a
/// view, or a block whose parent became a comment, while it was in the trash, is refused
/// (409) and stays there.
#[tokio::test]
async fn a_block_comes_back_from_the_trash_only_where_it_may_be() {
    let th = Setup::native().start().await.init_basic().await;
    let boards = boards_with_roles(&th).await;
    let content = card_with_content(&th, &boards).await;
    let board_id = &boards.board.id;
    let refused = |what: &str| format!("the block cannot come back where it was: {what}");

    delete(&boards.commenter, &content.comment).await;
    th.app
        .store()
        .execute_unchecked(&format!(
            "UPDATE blocks SET type = 'view' WHERE id = '{}'",
            content.card.id
        ))
        .await
        .expect("legacy card converted to view in DB");
    boards
        .commenter
        .post(&undelete_path(board_id, &content.comment.id), &json!({}))
        .await
        .check_error(StatusCode::CONFLICT, &refused(OFF_A_CARD));

    let middle = insert_one(&th.client, &new_block(board_id, BlockType::Card, "middle")).await;
    let below = insert_one(
        &th.client,
        &child_of(&middle, BlockType::Attachment, "below"),
    )
    .await;
    delete(&th.client, &below).await;
    th.app
        .store()
        .execute_unchecked(&format!(
            "UPDATE blocks SET type = 'comment' WHERE id = '{}'",
            middle.id
        ))
        .await
        .expect("legacy parent converted to comment in DB");
    th.client
        .post(&undelete_path(board_id, &below.id), &json!({}))
        .await
        .check_error(StatusCode::CONFLICT, &refused(BELOW_A_COMMENT));
    let read = blocks_of(&th.client, board_id).await;
    let read = titles(&read);
    assert!(
        !read.contains(&"comment") && !read.contains(&"below"),
        "both still in the trash: {read:?}"
    );
}

/// The author of a comment deletes it and brings it back; a comment of theirs that
/// somebody else deleted is the board's admins' to bring back, not theirs. (Deleting a
/// comment would take what is below it, and a block of somebody else there would
/// refuse it; a comment has nothing below it, so that check is left as a guard.)
#[tokio::test]
async fn the_author_of_a_comment_brings_back_only_what_they_deleted() {
    let th = Setup::native().start().await.init_basic().await;
    let boards = boards_with_roles(&th).await;
    let content = card_with_content(&th, &boards).await;
    let board_id = &boards.board.id;
    let own = &content.comment;
    let back = undelete_path(board_id, &own.id);

    delete(&boards.commenter, own).await;
    boards.commenter.post(&back, &json!({})).await.check_ok();
    delete(&th.client, own).await;
    boards.commenter.post(&back, &json!({})).await.check_error(
        StatusCode::FORBIDDEN,
        "a comment somebody else deleted is brought back by the board's admins",
    );
    assert!(
        !titles(&blocks_of(&th.client, board_id).await).contains(&"comment"),
        "still in the trash"
    );
    th.client.post(&back, &json!({})).await.check_ok();
    assert!(titles(&blocks_of(&th.client, board_id).await).contains(&"comment"));
}

/// What comes back is the whole deletion, so the block that was deleted decides: a
/// comment of somebody else that went with its card comes back with the card, which an
/// editor may delete and
/// bring back, naming the card or the comment.
#[tokio::test]
async fn a_deletion_comes_back_as_its_top_block_allows() {
    let th = Setup::native().start().await.init_basic().await;
    let boards = boards_with_roles(&th).await;
    let content = card_with_content(&th, &boards).await;
    let board_id = &boards.board.id;
    let others = &content.comment;

    // The card, deleted by the editor with the commenter's comment on it: the editor
    // brings it back, naming the card or the comment.
    delete(&boards.editor, &content.card).await;
    boards
        .editor
        .post(&undelete_path(board_id, &others.id), &json!({}))
        .await
        .check_ok();
    let read = blocks_of(&th.client, board_id).await;
    assert!(
        titles(&read).contains(&"card") && titles(&read).contains(&"comment"),
        "{:?}",
        titles(&read)
    );
}

/// A deleted comment is brought back only by who may delete it: its author, and the
/// comment of somebody else the board's admins only. An editor and a commenter are
/// refused another's. A card is
/// brought back by an editor as before.
#[tokio::test]
async fn a_deleted_comment_is_brought_back_only_by_who_may_delete_it() {
    let th = Setup::native().start().await.init_basic().await;
    let boards = boards_with_roles(&th).await;
    let content = card_with_content(&th, &boards).await;
    let board_id = &boards.board.id;
    let others = undelete_path(board_id, &content.comment.id);

    // The commenter's comment, deleted by the admin.
    delete(&th.client, &content.comment).await;
    boards.editor.post(&others, &json!({})).await.check_error(
        StatusCode::FORBIDDEN,
        "access denied to undelete other users' comments",
    );
    let editors = insert_one(
        &boards.editor,
        &child_of(&content.card, BlockType::Comment, "the editor's comment"),
    )
    .await;
    delete(&boards.editor, &editors).await;
    boards
        .commenter
        .post(&undelete_path(board_id, &editors.id), &json!({}))
        .await
        .check_error(
            StatusCode::FORBIDDEN,
            "access denied to undelete other users' comments",
        );
    assert!(
        !titles(&blocks_of(&th.client, board_id).await).contains(&"comment"),
        "still in the trash"
    );
    th.client.post(&others, &json!({})).await.check_ok();
    assert!(titles(&blocks_of(&th.client, board_id).await).contains(&"comment"));

    // The editor's own comment, and a card, by the editor.
    let own = insert_one(
        &boards.editor,
        &child_of(&content.card, BlockType::Comment, "the editor's"),
    )
    .await;
    delete(&boards.editor, &own).await;
    boards
        .editor
        .post(&undelete_path(board_id, &own.id), &json!({}))
        .await
        .check_ok();
    let card = insert_one(
        &th.client,
        &new_block(board_id, BlockType::Card, "card two"),
    )
    .await;
    delete(&th.client, &card).await;
    boards
        .editor
        .post(&undelete_path(board_id, &card.id), &json!({}))
        .await
        .check_ok();
    let read = blocks_of(&th.client, board_id).await;
    assert!(
        titles(&read).contains(&"the editor's") && titles(&read).contains(&"card two"),
        "{:?}",
        titles(&read)
    );
}

/// A comment is changed by its author with the commenter's role, and the comment of
/// somebody else by the board's admins only, as it is deleted; an editor may not change
/// it. `patchBlock`, `patchBlocks` and `patchContainersAndBlocks` alike. Who changed it and
/// when is what its `modifiedBy` and `updatedAt` say to the members.
#[tokio::test]
async fn a_comment_is_changed_by_its_author_or_an_admin() {
    let th = Setup::native().start().await.init_basic().await;
    let boards = boards_with_roles(&th).await;
    let content = card_with_content(&th, &boards).await;
    let board_id = &boards.board.id;
    let commenter = boards.commenter.me().await;
    let admin = th.client.me().await;
    let comment = &content.comment;
    let path = block_path(board_id, &comment.id);
    let many = format!("{TEST_WS}/boards/{board_id}/blocks");
    let together = |patch: Value| {
        json!({
            "containerIds": [board_id], "containerPatches": [{}],
            "blockIds": [comment.id], "blockPatches": [patch],
        })
    };
    let others_refused = "access denied to change other users' comments";
    let stored = |blocks: &[Block]| {
        blocks
            .iter()
            .find(|block| block.id == comment.id)
            .cloned()
            .expect("the comment is there")
    };

    // The author, with no more than the commenter's role, on both routes.
    let mut member = Socket::subscribed_to_workspace(&th, &th.client, TEST_WORKSPACE_ID).await;
    boards
        .commenter
        .patch(&path, &title_patch("by its author"))
        .await
        .check_no_content();
    let told = member.next_block().await;
    let read = stored(&blocks_of(&th.client, board_id).await);
    assert_eq!(read.title, "by its author");
    assert_eq!(read.modified_by, commenter.id, "who changed it");
    assert!(read.updated_at > comment.updated_at, "and when: later");
    assert_eq!(told["modifiedBy"], json!(commenter.id), "{told}");
    assert_eq!(told["updatedAt"], json!(read.updated_at), "{told}");
    boards
        .commenter
        .patch(&many, &batch(&[(comment, "again by its author")]))
        .await
        .check_no_content();
    th.client
        .patch(&many, &batch(&[(comment, "by the admin, many")]))
        .await
        .check_no_content();
    let read = stored(&blocks_of(&th.client, board_id).await);
    assert_eq!(
        read.modified_by, admin.id,
        "patchBlocks says who changed it"
    );
    boards
        .commenter
        .patch(&many, &batch(&[(comment, "again by its author")]))
        .await
        .check_no_content();

    // An editor is refused, on every route, and nothing changes.
    let response = boards
        .editor
        .patch(&path, &title_patch("by an editor"))
        .await;
    response.check_error(StatusCode::FORBIDDEN, others_refused);
    let response = boards
        .editor
        .patch(&many, &batch(&[(comment, "by an editor")]))
        .await;
    response.check_error(StatusCode::FORBIDDEN, others_refused);
    let response = boards
        .editor
        .patch(
            &format!("{TEST_WS}/containers-and-blocks"),
            &together(json!({"title": "by an editor"})),
        )
        .await;
    response.check_error(StatusCode::FORBIDDEN, others_refused);
    let read = stored(&blocks_of(&th.client, board_id).await);
    assert_eq!(
        read.title, "again by its author",
        "the editors changed nothing"
    );
    assert_eq!(read.modified_by, commenter.id);

    // The board's admin changes it, and is who changed it last.
    while member.next_json(QUIET).await.is_some() {}
    th.client
        .patch(&path, &title_patch("by the admin"))
        .await
        .check_no_content();
    let told = member.next_block().await;
    assert_eq!(told["modifiedBy"], json!(admin.id), "{told}");
    th.client
        .patch(
            &format!("{TEST_WS}/containers-and-blocks"),
            &together(json!({"title": "by the admin again"})),
        )
        .await
        .check_ok();
    let read = stored(&blocks_of(&th.client, board_id).await);
    assert_eq!(read.title, "by the admin again");
    assert_eq!(read.modified_by, admin.id);
}

/// The commenter's role changes the commenter's own comment, not a card, and makes no
/// card of a comment; a viewer changes nothing.
#[tokio::test]
async fn a_commenter_changes_no_card_and_makes_none() {
    let th = Setup::native().start().await.init_basic().await;
    let boards = boards_with_roles(&th).await;
    let content = card_with_content(&th, &boards).await;
    let board_id = &boards.board.id;
    let path = block_path(board_id, &content.comment.id);
    let many = format!("{TEST_WS}/boards/{board_id}/blocks");

    boards
        .commenter
        .patch(&path, &json!({"type": "card"}))
        .await
        .check_error(StatusCode::BAD_REQUEST, "a block's type does not change");
    boards
        .commenter
        .patch(
            &block_path(board_id, &content.card.id),
            &title_patch("a card"),
        )
        .await
        .check_error(StatusCode::FORBIDDEN, "access denied to make board changes");
    let own = insert_one(
        &boards.commenter,
        &child_of(&content.card, BlockType::Comment, "own"),
    )
    .await;
    boards
        .commenter
        .patch(
            &many,
            &batch(&[(&own, "own, changed"), (&content.card, "x")]),
        )
        .await
        .check_error(StatusCode::FORBIDDEN, "access denied to make board changes");
    boards
        .commenter
        .patch(
            &many,
            &json!({"blockIds": [own.id], "blockPatches": [{"type": "card"}]}),
        )
        .await
        .check_bad_request();
    // A viewer changes nothing.
    boards
        .viewer
        .patch(&block_path(board_id, &own.id), &title_patch("by a viewer"))
        .await
        .check_error(StatusCode::FORBIDDEN, "access denied to make board changes");
    let read = blocks_of(&th.client, board_id).await;
    assert!(
        titles(&read).contains(&"own") && titles(&read).contains(&"card"),
        "nothing changed: {:?}",
        titles(&read)
    );
}

/// The commenter's role changes the text of the commenter's own comment, its body; moving
/// it to another card (`parentId`) is for editors, refused as any change the role may not
/// make on
/// patchBlock and patchBlocks; patchContainersAndBlocks refuses a commenter before, by the
/// board's permission. A `parentId` that stays as it was moves nothing. An editor
/// moves their own comment and not somebody else's; an admin moves anybody's.
#[tokio::test]
async fn a_commenter_changes_the_text_of_their_comment_but_does_not_move_it() {
    let th = Setup::native().start().await.init_basic().await;
    let boards = boards_with_roles(&th).await;
    let content = card_with_content(&th, &boards).await;
    let board_id = &boards.board.id;
    let second = insert_one(&th.client, &new_block(board_id, BlockType::Card, "second")).await;
    let own = &content.comment;
    let path = block_path(board_id, &own.id);
    let doc = |text: &str| {
        json!({"type": "doc", "content": [
            {"type": "paragraph", "content": [{"type": "text", "text": text}]},
        ]})
    };

    boards
        .commenter
        .patch(
            &path,
            &json!({"body": doc("changed"), "readAt": own.updated_at, "parentId": own.parent_id}),
        )
        .await
        .check_no_content();
    let refused = "access denied to make board changes";
    boards
        .commenter
        .patch(&path, &json!({"parentId": second.id}))
        .await
        .check_error(StatusCode::FORBIDDEN, refused);
    boards
        .commenter
        .patch(
            &blocks_path(board_id),
            &json!({"blockIds": [own.id], "blockPatches": [{"parentId": second.id}]}),
        )
        .await
        .check_error(StatusCode::FORBIDDEN, refused);
    let moved_together = json!({
        "containerIds": [board_id], "containerPatches": [{}],
        "blockIds": [own.id], "blockPatches": [{"parentId": second.id}],
    });
    boards
        .commenter
        .patch(&format!("{TEST_WS}/containers-and-blocks"), &moved_together)
        .await
        .check_error(
            StatusCode::FORBIDDEN,
            "access denied to modifying board properties",
        );
    let read = blocks_of(&th.client, board_id).await;
    let comment = find(&read, &own.id);
    assert_eq!(comment.parent_id, content.card.id, "not moved");
    assert_eq!(
        comment.body.as_ref().map(|body| body.document().clone()),
        Some(doc("changed")),
        "its text changed"
    );
}

/// An editor moves their own comment to another card, alone or with the board, but not
/// somebody else's; an admin moves anybody's.
#[tokio::test]
async fn an_editor_moves_their_own_comment_and_an_admin_anybodys() {
    let th = Setup::native().start().await.init_basic().await;
    let boards = boards_with_roles(&th).await;
    let content = card_with_content(&th, &boards).await;
    let board_id = &boards.board.id;
    let second = insert_one(&th.client, &new_block(board_id, BlockType::Card, "second")).await;
    let own = &content.comment;
    let path = block_path(board_id, &own.id);

    // An editor moves their own comment, alone or with the board.
    let editors = insert_one(
        &boards.editor,
        &child_of(&content.card, BlockType::Comment, "the editor's"),
    )
    .await;
    boards
        .editor
        .patch(
            &block_path(board_id, &editors.id),
            &json!({"parentId": second.id}),
        )
        .await
        .check_no_content();
    boards
        .editor
        .patch(
            &format!("{TEST_WS}/containers-and-blocks"),
            &json!({
                "containerIds": [board_id], "containerPatches": [{}],
                "blockIds": [editors.id], "blockPatches": [{"parentId": content.card.id}],
            }),
        )
        .await
        .check_ok();
    // Naming the card a comment is on moves nothing: the commenter may.
    boards
        .commenter
        .patch(
            &path,
            &json!({"parentId": content.card.id, "title": "still on the card"}),
        )
        .await
        .check_no_content();

    // Somebody else's comment: not the editor, but the admin.
    boards
        .editor
        .patch(&path, &json!({"parentId": second.id}))
        .await
        .check_error(
            StatusCode::FORBIDDEN,
            "access denied to change other users' comments",
        );
    th.client
        .patch(&path, &json!({"parentId": second.id}))
        .await
        .check_no_content();
    let read = blocks_of(&th.client, board_id).await;
    assert_eq!(find(&read, &editors.id).parent_id, content.card.id);
    assert_eq!(
        find(&read, &own.id).parent_id,
        second.id,
        "moved by the admin"
    );
}

/// Block types are fixed at creation: neither an editor nor an admin can mutate an
/// existing attachment or card into a comment via `patchBlock` or `patchBlocks`.
#[tokio::test]
async fn a_block_type_cannot_be_mutated_to_comment_even_by_an_admin() {
    let th = Setup::native().start().await.init_basic().await;
    let boards = boards_with_roles(&th).await;
    let content = card_with_content(&th, &boards).await;
    let board_id = &boards.board.id;
    let into_comment = json!({"type": "comment", "title": "I approve"});

    for client in [&boards.editor, &th.client] {
        let single = client
            .patch(&block_path(board_id, &content.text.id), &into_comment)
            .await;
        single.check_bad_request();
        assert_eq!(single.refused_at(), "/type", "single patch refuses /type");

        let batch = client
            .patch(
                &format!("{TEST_WS}/boards/{board_id}/blocks"),
                &json!({"blockIds": [content.text.id], "blockPatches": [into_comment]}),
            )
            .await;
        batch.check_bad_request();
        assert_eq!(
            batch.refused_at(),
            "/blockPatches/0/type",
            "batch patch refuses /blockPatches/0/type"
        );
    }
    th.teardown().await;
}
