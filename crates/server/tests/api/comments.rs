//! Who may change, delete and bring back a comment on a card: its author from the
//! commenter's role up, and somebody else's by the board's admins only.

use std::time::Duration;

use fvoci_model::{Block, BlockType};
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::blocks::{
    QUIET, Socket, blocks_of, boards_with_roles, card_with_content, child_of, delete, find,
    insert_one, new_block, restore_card, titles, trash_card,
};
use crate::common::{Client, Setup, TEST_WORKSPACE_ID, TEST_WS, TestServer};

/// The path of `comment` on its card.
fn comment_path(comment: &Block) -> String {
    format!(
        "{TEST_WS}/boards/{}/cards/{}/comments/{}",
        comment.container_id, comment.parent_id, comment.id
    )
}

/// The path that brings `comment` back from the trash.
fn restore_path(comment: &Block) -> String {
    format!("{}/restore", comment_path(comment))
}

/// A comment saying `text` on `card`, by `client`'s user.
async fn comment_by(client: &Client, card: &Block, text: &str) -> Block {
    insert_one(client, &child_of(card, BlockType::Comment, text)).await
}

/// A body of one paragraph saying `text`.
fn doc(text: &str) -> Value {
    json!({"type": "doc", "content": [
        {"type": "paragraph", "content": [{"type": "text", "text": text}]},
    ]})
}

/// What `block` says in its body.
fn said(block: &Block) -> Option<Value> {
    block.body.as_ref().map(|body| body.document().clone())
}

/// Whether the board's blocks, as its admin reads them, hold `block`.
async fn holds(th: &TestServer, board_id: &str, block: &Block) -> bool {
    blocks_of(&th.client, board_id)
        .await
        .iter()
        .any(|read| read.id == block.id)
}

/// A comment can only be deleted by its author or by a board admin, not by another editor.
#[tokio::test]
async fn delete_comment_of_another_user_needs_the_permission_to_delete_others_comments() {
    let th = Setup::native().start().await.init_basic().await;
    let boards = boards_with_roles(&th).await;
    let content = card_with_content(&th, &boards).await;
    let board_id = &boards.board.id;

    // The comment was written by the commenter.
    let response = boards.editor.delete(&comment_path(&content.comment)).await;
    response.check_forbidden();
    assert_eq!(
        response.error_message(),
        "access denied to delete other users' comments",
        "an editor is refused"
    );
    assert!(
        holds(&th, board_id, &content.comment).await,
        "the comment is still there"
    );

    // Other blocks of other users are deleted by an editor as before.
    delete(&boards.editor, &content.text).await;
    th.teardown().await;
}

/// The cases that stay allowed: an editor deletes a comment of their own, and an admin
/// deletes the comment of another user.
#[tokio::test]
async fn delete_comment_by_its_author_or_by_an_admin_works() {
    let th = Setup::native().start().await.init_basic().await;
    let boards = boards_with_roles(&th).await;
    let content = card_with_content(&th, &boards).await;
    let board_id = &boards.board.id;

    let own = comment_by(&boards.editor, &content.card, "the editor's comment").await;
    delete(&boards.editor, &own).await;

    // The commenter's comment, deleted by the board's admin.
    delete(&th.client, &content.comment).await;

    let stored = blocks_of(&th.client, board_id).await;
    let mut left = titles(&stored);
    left.sort_unstable();
    assert_eq!(left, ["card", "text"], "both comments are gone");
    th.teardown().await;
}

/// A commenter deletes their own comment and brings it back, and does neither to anything
/// else: not to somebody else's comment, not to a card, and not to their comment that went
/// with a card, since the card decides what comes back.
#[tokio::test]
async fn a_commenter_deletes_and_brings_back_their_own_comment() {
    let th = Setup::native().start().await.init_basic().await;
    let boards = boards_with_roles(&th).await;
    let content = card_with_content(&th, &boards).await;
    let board_id = &boards.board.id;
    let own = &content.comment;

    delete(&boards.commenter, own).await;
    assert!(!holds(&th, board_id, own).await, "in the trash");
    boards
        .commenter
        .post_empty(&restore_path(own))
        .await
        .check_ok();
    assert!(holds(&th, board_id, own).await, "back");

    let editors = comment_by(&boards.editor, &content.card, "the editor's").await;
    boards
        .commenter
        .delete(&comment_path(&editors))
        .await
        .check_error(
            StatusCode::FORBIDDEN,
            "access denied to delete other users' comments",
        );
    trash_card(&boards.commenter, board_id, &content.card.id)
        .await
        .check_error(StatusCode::FORBIDDEN, "access denied to delete cards");

    // The card, deleted by the editor with the commenter's comment on it: the card
    // comes back, not the comment alone, which is on no card there is.
    delete(&boards.editor, &content.card).await;
    restore_card(&boards.commenter, board_id, &content.card.id)
        .await
        .check_error(StatusCode::FORBIDDEN, "access denied to restore cards");
    boards
        .commenter
        .post_empty(&restore_path(own))
        .await
        .check_error(StatusCode::NOT_FOUND, "card not found");
    assert!(
        blocks_of(&th.client, board_id).await.is_empty(),
        "all still in the trash"
    );
    th.teardown().await;
}

/// A comment or an attachment comes back from the trash only onto a card: one whose card
/// became a view, or whose card became a comment, while it was in the trash (as an older
/// server may have left them) is on no card (404), and stays in the trash.
#[tokio::test]
async fn a_block_comes_back_from_the_trash_only_onto_a_card() {
    let th = Setup::native().start().await.init_basic().await;
    let boards = boards_with_roles(&th).await;
    let content = card_with_content(&th, &boards).await;
    let board_id = &boards.board.id;

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
        .post_empty(&restore_path(&content.comment))
        .await
        .check_error(StatusCode::NOT_FOUND, "card not found");

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
        .post_empty(&format!(
            "{TEST_WS}/boards/{board_id}/cards/{}/attachments/{}/restore",
            middle.id, below.id
        ))
        .await
        .check_error(StatusCode::NOT_FOUND, "card not found");
    for block in [&content.comment, &below] {
        assert!(
            !holds(&th, board_id, block).await,
            "still in the trash: {}",
            block.id
        );
    }
    th.teardown().await;
}

/// The author of a comment deletes it and brings it back; a comment of theirs that
/// somebody else deleted is the board's admins' to bring back, not theirs.
#[tokio::test]
async fn the_author_of_a_comment_brings_back_only_what_they_deleted() {
    let th = Setup::native().start().await.init_basic().await;
    let boards = boards_with_roles(&th).await;
    let content = card_with_content(&th, &boards).await;
    let board_id = &boards.board.id;
    let own = &content.comment;
    let back = restore_path(own);

    delete(&boards.commenter, own).await;
    boards.commenter.post_empty(&back).await.check_ok();
    delete(&th.client, own).await;
    boards.commenter.post_empty(&back).await.check_error(
        StatusCode::FORBIDDEN,
        "a comment somebody else deleted is brought back by the board's admins",
    );
    assert!(!holds(&th, board_id, own).await, "still in the trash");
    th.client.post_empty(&back).await.check_ok();
    assert!(holds(&th, board_id, own).await, "back");
    th.teardown().await;
}

/// What comes back is the whole deletion, so the block that was deleted decides: a
/// comment of somebody else that went with its card comes back with the card, which an
/// editor may delete and bring back.
#[tokio::test]
async fn a_deletion_comes_back_as_its_top_block_allows() {
    let th = Setup::native().start().await.init_basic().await;
    let boards = boards_with_roles(&th).await;
    let content = card_with_content(&th, &boards).await;
    let board_id = &boards.board.id;

    delete(&boards.editor, &content.card).await;
    restore_card(&boards.editor, board_id, &content.card.id)
        .await
        .check_ok();
    for block in [&content.card, &content.comment] {
        assert!(holds(&th, board_id, block).await, "back: {}", block.id);
    }
    th.teardown().await;
}

/// A deleted comment is brought back only by who may delete it: its author, and the
/// comment of somebody else the board's admins only. An editor and a commenter are
/// refused another's. A card is brought back by an editor as before.
#[tokio::test]
async fn a_deleted_comment_is_brought_back_only_by_who_may_delete_it() {
    let th = Setup::native().start().await.init_basic().await;
    let boards = boards_with_roles(&th).await;
    let content = card_with_content(&th, &boards).await;
    let board_id = &boards.board.id;
    let others = restore_path(&content.comment);
    let refused = "access denied to undelete other users' comments";

    // The commenter's comment, deleted by the admin.
    delete(&th.client, &content.comment).await;
    boards
        .editor
        .post_empty(&others)
        .await
        .check_error(StatusCode::FORBIDDEN, refused);
    let editors = comment_by(&boards.editor, &content.card, "the editor's comment").await;
    delete(&boards.editor, &editors).await;
    boards
        .commenter
        .post_empty(&restore_path(&editors))
        .await
        .check_error(StatusCode::FORBIDDEN, refused);
    assert!(
        !holds(&th, board_id, &content.comment).await,
        "still in the trash"
    );
    th.client.post_empty(&others).await.check_ok();
    assert!(holds(&th, board_id, &content.comment).await, "back");

    // The editor's own comment, and a card, by the editor.
    let own = comment_by(&boards.editor, &content.card, "the editor's").await;
    delete(&boards.editor, &own).await;
    boards
        .editor
        .post_empty(&restore_path(&own))
        .await
        .check_ok();
    let card = insert_one(
        &th.client,
        &new_block(board_id, BlockType::Card, "card two"),
    )
    .await;
    delete(&th.client, &card).await;
    restore_card(&boards.editor, board_id, &card.id)
        .await
        .check_ok();
    for block in [&own, &card] {
        assert!(holds(&th, board_id, block).await, "back: {}", block.id);
    }
    th.teardown().await;
}

/// A comment is changed by its author with the commenter's role, and the comment of
/// somebody else by the board's admins only, as it is deleted; an editor may not change
/// it. Who changed it and when is what its `modifiedBy` and `updatedAt` say to the members.
#[tokio::test]
async fn a_comment_is_changed_by_its_author_or_an_admin() {
    let th = Setup::native().start().await.init_basic().await;
    let boards = boards_with_roles(&th).await;
    let content = card_with_content(&th, &boards).await;
    let board_id = &boards.board.id;
    let commenter = boards.commenter.me().await;
    let admin = th.client.me().await;
    let comment = &content.comment;
    let path = comment_path(comment);
    let write = |text: &str, read: &Block| json!({"body": doc(text), "readAt": read.updated_at});

    // The author, with no more than the commenter's role.
    let mut member = Socket::subscribed_to_workspace(&th, &th.client, TEST_WORKSPACE_ID).await;
    tokio::time::sleep(Duration::from_millis(3)).await;
    let read: Block = boards
        .commenter
        .patch(&path, &write("by its author", comment))
        .await
        .ok_json();
    let told = member.next_block().await;
    assert_eq!(said(&read), Some(doc("by its author")));
    assert_eq!(read.modified_by, commenter.id, "who changed it");
    assert!(read.updated_at > comment.updated_at, "and when: later");
    assert_eq!(told["modifiedBy"], json!(commenter.id), "{told}");
    assert_eq!(told["updatedAt"], json!(read.updated_at), "{told}");

    // An editor is refused, and nothing changes.
    boards
        .editor
        .patch(&path, &write("by an editor", &read))
        .await
        .check_error(
            StatusCode::FORBIDDEN,
            "access denied to change other users' comments",
        );
    let stored = blocks_of(&th.client, board_id).await;
    let stored = find(&stored, &comment.id);
    assert_eq!(
        said(stored),
        Some(doc("by its author")),
        "the editor changed nothing"
    );
    assert_eq!(stored.modified_by, commenter.id);

    // The board's admin changes it, and is who changed it last.
    while member.next_json(QUIET).await.is_some() {}
    let read: Block = th
        .client
        .patch(&path, &write("by the admin", &read))
        .await
        .ok_json();
    let told = member.next_block().await;
    assert_eq!(told["modifiedBy"], json!(admin.id), "{told}");
    assert_eq!(said(&read), Some(doc("by the admin")));
    assert_eq!(read.modified_by, admin.id);
    th.teardown().await;
}

/// The commenter's role changes the commenter's own comment, not a card, and makes no
/// card; a patch of a comment takes no `type` to make one of it. A viewer changes nothing.
#[tokio::test]
async fn a_commenter_changes_no_card_and_makes_none() {
    let th = Setup::native().start().await.init_basic().await;
    let boards = boards_with_roles(&th).await;
    let content = card_with_content(&th, &boards).await;
    let board_id = &boards.board.id;
    let own = comment_by(&boards.commenter, &content.card, "own").await;

    boards
        .commenter
        .patch(&comment_path(&own), &json!({"type": "card"}))
        .await
        .check_bad_request();
    boards
        .commenter
        .patch(
            &format!("{TEST_WS}/cards/{}", content.card.id),
            &json!({"title": "a card"}),
        )
        .await
        .check_error(StatusCode::FORBIDDEN, "access denied to patch card");
    boards
        .commenter
        .post(
            &format!("{TEST_WS}/boards/{board_id}/cards/batch-patch"),
            &json!({"cardIds": [content.card.id], "updatedProperties": {}}),
        )
        .await
        .check_error(StatusCode::FORBIDDEN, "access denied to patch cards");
    boards
        .commenter
        .post(
            &format!("{TEST_WS}/boards/{board_id}/cards"),
            &json!({"title": "made"}),
        )
        .await
        .check_error(StatusCode::FORBIDDEN, "access denied to create card");
    // A viewer changes nothing.
    boards
        .viewer
        .patch(
            &comment_path(&own),
            &json!({"body": doc("by a viewer"), "readAt": own.updated_at}),
        )
        .await
        .check_forbidden();
    let read = blocks_of(&th.client, board_id).await;
    assert_eq!(said(find(&read, &own.id)), Some(doc("own")), "not changed");
    assert_eq!(find(&read, &content.card.id).title, "card", "not changed");
    assert!(
        !titles(&read).contains(&"made"),
        "no card made: {:?}",
        titles(&read)
    );
    th.teardown().await;
}

/// The commenter's role changes the text of the commenter's own comment, its body. A
/// comment stays on its card: a patch of it takes no `parentId` (400), whoever sends it.
#[tokio::test]
async fn a_commenter_changes_the_text_of_their_comment_but_does_not_move_it() {
    let th = Setup::native().start().await.init_basic().await;
    let boards = boards_with_roles(&th).await;
    let content = card_with_content(&th, &boards).await;
    let board_id = &boards.board.id;
    let second = insert_one(&th.client, &new_block(board_id, BlockType::Card, "second")).await;
    let own = &content.comment;
    let path = comment_path(own);

    let changed: Block = boards
        .commenter
        .patch(
            &path,
            &json!({"body": doc("changed"), "readAt": own.updated_at}),
        )
        .await
        .ok_json();
    // A comment names no card to be on: its author and the board's admins, who may
    // change it, are told so; an editor may not change it at all.
    let moved = json!({"parentId": second.id, "readAt": changed.updated_at});
    for client in [&boards.commenter, &th.client] {
        client.patch(&path, &moved).await.check_bad_request();
    }
    boards.editor.patch(&path, &moved).await.check_forbidden();
    let read = blocks_of(&th.client, board_id).await;
    let comment = find(&read, &own.id);
    assert_eq!(comment.parent_id, content.card.id, "not moved");
    assert_eq!(said(comment), Some(doc("changed")), "its text changed");
    th.teardown().await;
}
