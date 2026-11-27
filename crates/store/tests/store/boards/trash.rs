//! A board in the trash: what goes with it, what comes back with it, and what the purge
//! removes.

use super::*;

/// Deleting and restoring a board moves its blocks to the trash and back with it.
#[tokio::test]
async fn delete_and_undelete_container_take_the_blocks_along() {
    let test = common::setup().await;
    let user_id = common::user(&test, "user").await;
    let deleter_id = common::user(&test, "deleter").await;
    let restorer_id = common::user(&test, "restorer").await;
    let workspace_id = common::workspace(&test).await;
    let board_id = new_id();
    let card_id = new_id();
    let text_id = new_id();

    let mut board = new_board(&board_id, &workspace_id, ContainerVisibility::Open);
    test.insert_container(&mut board, &user_id)
        .await
        .expect("the board is inserted");
    let mut blocks = vec![
        Block {
            id: card_id.clone(),
            container_id: board_id.clone(),
            parent_id: String::new(),
            ..Block::default()
        },
        Block {
            id: text_id.clone(),
            container_id: board_id.clone(),
            parent_id: card_id.clone(),
            ..Block::default()
        },
    ];
    common::insert_blocks(&test, &mut blocks, &user_id).await;
    let member = ContainerMember {
        container_id: board_id.clone(),
        user_id: user_id.clone(),
        role: fvoci_model::ContainerRole::Admin,
        synthetic: false,
        inherited_role: None,
    };
    test.save_member(&member)
        .await
        .expect("the member is saved");

    test.delete_container(&board_id, &deleter_id)
        .await
        .expect("the board is deleted");
    let left = test
        .get_blocks(&BlockFilter::container(&board_id))
        .await
        .expect("the blocks are read");
    assert!(left.is_empty(), "got {left:?}");
    // Memberships move to the trash with a deleted board.
    let members = test
        .get_members_for_container(&board_id)
        .await
        .expect("the members are read");
    assert!(members.is_empty(), "got {members:?}");
    // The deleted board is recorded in the trash along with who deleted it.
    let deleted = test
        .get_deleted_container(&board_id)
        .await
        .expect("the board is in the trash");
    assert_eq!(deleted.deleted_by, deleter_id, "who deleted the board");
    assert_eq!(
        deleted.container.modified_by, user_id,
        "the board as it was"
    );
    let deleted_at = deleted.container.deleted_at.expect("deletion time");
    assert!(
        deleted_at >= board.updated_at,
        "deleted at {deleted_at} after the last modification at {}",
        board.updated_at
    );
    assert_eq!(
        sorted_user_ids(&deleted.members),
        [user_id.as_str()],
        "the membership is kept with the board"
    );

    test.undelete_container(&board_id, &restorer_id)
        .await
        .expect("the board is restored");
    let restored = test
        .get_container(&board_id)
        .await
        .expect("the board is back");
    assert_eq!(restored.modified_by, restorer_id, "who restored the board");
    assert_eq!(restored.created_by, user_id, "the creator stays");
    assert_eq!(
        restored.created_at, board.created_at,
        "the creation time stays"
    );
    assert_eq!(restored.deleted_at, None, "the board is not deleted");
    let back = test
        .get_blocks(&BlockFilter::container(&board_id))
        .await
        .expect("the blocks are read");
    assert_eq!(
        common::sorted_ids(&back),
        sorted([&card_id, &text_id]),
        "both blocks are back"
    );

    test.teardown().await;
}

/// What a board is made of is not found once the board is deleted, and is in the trash.
#[tokio::test]
async fn delete_container_moves_the_board_and_what_is_its_own_to_the_trash() {
    let test = common::setup().await;
    let full = FullBoard::store(&test).await;
    let deleter_id = common::user(&test, "deleter").await;
    let board_id = full.board.id.as_str();

    test.delete_container(board_id, &deleter_id)
        .await
        .expect("the board is deleted");

    let error = test
        .get_container(board_id)
        .await
        .expect_err("the board is gone");
    assert!(error.is_not_found(), "got {error:?}");
    let left = blocks_of(&test, board_id).await;
    assert!(left.is_empty(), "blocks are left: {left:?}");
    let members = members_of(&test, board_id).await;
    assert!(members.is_empty(), "memberships are left: {members:?}");
    let of_admin = test
        .get_members_for_user(&full.admin_id)
        .await
        .expect("the memberships are read");
    assert!(
        of_admin.is_empty(),
        "the user is left a member: {of_admin:?}"
    );
    let error = test
        .get_file_record(board_id, &full.file.stored_name)
        .await
        .expect_err("the file record is gone");
    assert!(error.is_not_found(), "got {error:?}");

    let deleted = test
        .get_deleted_container(board_id)
        .await
        .expect("the board is in the trash");
    let deleted_at = deleted.container.deleted_at.expect("deletion time");
    assert!(
        deleted_at >= full.board.updated_at,
        "deleted at {deleted_at}, after the board was stored at {}",
        full.board.updated_at
    );
    assert_eq!(
        deleted.container,
        Container {
            deleted_at: Some(deleted_at),
            ..full.board.clone()
        },
        "the board as it was, marked as deleted"
    );
    assert_eq!(deleted.deleted_by, deleter_id, "who deleted the board");
    assert_eq!(
        sorted_members(deleted.members),
        full.members,
        "the memberships as they were"
    );

    for block in &full.blocks {
        let error = test
            .get_block(&block.id)
            .await
            .expect_err("the block is gone");
        assert!(error.is_not_found(), "block {}: got {error:?}", block.title);
        let in_trash = test
            .get_deleted_block(&block.id)
            .await
            .expect("the block is in the trash");
        assert_eq!(
            in_trash.block,
            Block {
                deleted_at: Some(deleted_at),
                ..block.clone()
            },
            "{} as it was, deleted when its board was",
            block.title
        );
        assert_eq!(
            in_trash.deleted_by, deleter_id,
            "who deleted {}",
            block.title
        );
    }

    test.teardown().await;
}

#[tokio::test]
async fn delete_container_ends_sharing_category_filing_and_subscriptions() {
    let test = common::setup().await;
    let full = FullBoard::store(&test).await;

    // Before: the board is shared, filed and subscribed to.
    test.get_public_link(&full.board.id)
        .await
        .expect("the board is shared");
    let categories = test
        .get_user_category_containers(&full.editor_id, &full.workspace_id)
        .await
        .expect("the categories are read");
    assert_eq!(
        categories[0].container_metadata.len(),
        1,
        "the board is filed under the category: {categories:?}"
    );
    assert_eq!(
        subscribed_block_ids(&test, &full.editor_id).await,
        [full.card_id.as_str()],
        "the editor subscribes to the card"
    );

    test.delete_container(&full.board.id, &full.admin_id)
        .await
        .expect("the board is deleted");

    full.assert_sharing_gone_and_watchers(&test, &[]).await;
    assert_eq!(
        full.filed_by_editor(&test).await,
        Vec::<String>::new(),
        "the board is filed nowhere while in the trash"
    );

    test.teardown().await;
}

#[tokio::test]
async fn undelete_container_brings_back_the_board_its_blocks_files_and_members() {
    let test = common::setup().await;
    let full = FullBoard::store(&test).await;
    let restorer_id = common::user(&test, "restorer").await;
    let board_id = full.board.id.as_str();

    test.delete_container(board_id, &full.admin_id)
        .await
        .expect("the board is deleted");
    let returned = test
        .undelete_container(board_id, &restorer_id)
        .await
        .expect("the board is restored");

    let restored = test
        .get_container(board_id)
        .await
        .expect("the board is back");
    assert_eq!(returned, restored, "what is returned is what is stored");
    assert!(
        restored.updated_at >= full.board.updated_at,
        "restoring is a modification: {} after {}",
        restored.updated_at,
        full.board.updated_at
    );
    assert_eq!(
        restored,
        Container {
            modified_by: restorer_id.clone(),
            updated_at: restored.updated_at,
            ..full.board.clone()
        },
        "the board as it was, modified by whoever restored it"
    );

    // Each block is back where it was: on its board, on its parent.
    let expected: Vec<Block> = full
        .blocks
        .iter()
        .map(|block| Block {
            modified_by: restorer_id.clone(),
            updated_at: restored.updated_at,
            ..block.clone()
        })
        .collect();
    assert_eq!(
        blocks_of(&test, board_id).await,
        expected,
        "the blocks as they were, modified by whoever restored them"
    );

    let file = test
        .get_file_record(board_id, &full.file.stored_name)
        .await
        .expect("the file record is back");
    assert_eq!(file, full.file, "the file record as it was, path included");
    assert_eq!(
        members_of(&test, board_id).await,
        full.members,
        "the memberships as they were"
    );

    // Nothing of the board stays in the trash.
    let error = test
        .get_deleted_container(board_id)
        .await
        .expect_err("the board is out of the trash");
    assert!(error.is_not_found(), "got {error:?}");
    for block in &full.blocks {
        let error = test
            .get_deleted_block(&block.id)
            .await
            .expect_err("the block is out of the trash");
        assert!(error.is_not_found(), "block {}: got {error:?}", block.title);
    }

    test.teardown().await;
}

/// The places in the sidebars and the subscriptions to its blocks come back with the board;
/// its sharing does not.
#[tokio::test]
async fn undelete_container_brings_back_category_filing_and_subscriptions_but_not_sharing() {
    let test = common::setup().await;
    let full = FullBoard::store(&test).await;

    test.delete_container(&full.board.id, &full.admin_id)
        .await
        .expect("the board is deleted");
    test.undelete_container(&full.board.id, &full.admin_id)
        .await
        .expect("the board is restored");

    test.get_block(&full.card_id)
        .await
        .expect("the card is back");
    full.assert_sharing_gone_and_watchers(&test, &[&full.admin_id, &full.editor_id])
        .await;
    assert_eq!(
        full.filed_by_editor(&test).await,
        std::slice::from_ref(&full.board.id),
        "the editor has it where it was"
    );

    test.teardown().await;
}

/// The row of a user may be gone by the time a board comes back: their membership stays
/// behind.
#[tokio::test]
async fn undelete_container_leaves_out_the_membership_of_a_user_who_is_gone() {
    let test = common::setup().await;
    let full = FullBoard::store(&test).await;
    let board_id = full.board.id.as_str();
    // A member who made and changed nothing: the row of a user whom a board or a block
    // names cannot be deleted.
    let leaver_id = common::user(&test, "leaver").await;
    test.save_member(&editor_of(board_id, &leaver_id))
        .await
        .expect("the member is stored");

    test.delete_container(board_id, &full.admin_id)
        .await
        .expect("the board is deleted");
    let deleted = test
        .get_deleted_container(board_id)
        .await
        .expect("the board is in the trash");
    assert_eq!(
        sorted_user_ids(&deleted.members),
        sorted([&full.admin_id, &full.editor_id, &leaver_id]),
        "all three memberships are in the trash"
    );

    common::delete_user_row(&test, &leaver_id).await;
    test.get_user_by_id(&leaver_id)
        .await
        .expect_err("the user is gone");

    test.undelete_container(board_id, &full.admin_id)
        .await
        .expect("the board is restored");
    assert_eq!(
        members_of(&test, board_id).await,
        full.members,
        "the memberships of the users who still exist"
    );

    test.teardown().await;
}

/// A user who is only deactivated still has a row, and a membership can refer to it:
/// it comes back with the board.
#[tokio::test]
async fn undelete_container_brings_back_the_membership_of_a_user_who_was_deactivated() {
    let test = common::setup().await;
    let full = FullBoard::store(&test).await;
    let board_id = full.board.id.as_str();
    let deactivated_id = common::user(&test, "deactivated").await;
    let membership = editor_of(board_id, &deactivated_id);
    test.save_member(&membership)
        .await
        .expect("the member is stored");
    let members_before = members_of(&test, board_id).await;
    assert_eq!(members_before.len(), 3, "got {members_before:?}");

    test.delete_container(board_id, &full.admin_id)
        .await
        .expect("the board is deleted");
    common::execute_raw(
        &test.database_connection(),
        &format!("UPDATE users SET deleted_at = 1700000000000 WHERE id = '{deactivated_id}'"),
    )
    .await;
    test.get_user_by_id(&deactivated_id)
        .await
        .expect_err("a deactivated user is not found");

    test.undelete_container(board_id, &full.admin_id)
        .await
        .expect("the board is restored");
    assert_eq!(
        members_of(&test, board_id).await,
        members_before,
        "all three memberships are back, the deactivated user's among them"
    );

    test.teardown().await;
}

/// What is in the trash refers to nothing, so the row of the user who made a board can
/// go while the board is there. The board comes back all the same, as made by nobody.
#[tokio::test]
async fn undelete_container_restores_a_board_whose_creator_is_gone_as_made_by_nobody() {
    let test = common::setup().await;
    let creator_id = common::user(&test, "creator").await;
    let editor_id = common::user(&test, "editor").await;
    let workspace_id = common::workspace_of(&test, &[&creator_id]).await;

    let mut board = new_board(&new_id(), &workspace_id, ContainerVisibility::Open);
    test.insert_container_with_admin(&mut board, &creator_id)
        .await
        .expect("the board and its administrator are stored");
    test.save_member(&editor_of(&board.id, &editor_id))
        .await
        .expect("the editor is stored");
    let mut card = new_block(&board.id, "", BlockType::Card);
    test.insert_block(&mut card, &creator_id)
        .await
        .expect("the creator's card is stored");
    let mut paragraph = new_block(&board.id, &card.id, BlockType::Attachment);
    test.insert_block(&mut paragraph, &editor_id)
        .await
        .expect("the editor's text is stored");

    test.delete_container(&board.id, &editor_id)
        .await
        .expect("the board is deleted");
    // Nothing that exists names the creator any more.
    common::delete_user_row(&test, &creator_id).await;

    let returned = test
        .undelete_container(&board.id, &editor_id)
        .await
        .expect("the board is restored though its creator is gone");
    assert_eq!(returned.created_by, "", "the board was made by nobody");
    let restored = test
        .get_container(&board.id)
        .await
        .expect("the board is back");
    assert_eq!(returned, restored, "what is returned is what is stored");
    assert_eq!(
        restored,
        Container {
            created_by: String::new(),
            modified_by: editor_id.clone(),
            updated_at: restored.updated_at,
            ..board.clone()
        },
        "the board as it was, but for its creator"
    );

    let restored_card = test.get_block(&card.id).await.expect("the card is back");
    assert_eq!(
        restored_card.created_by, "",
        "the card of the user who is gone was made by nobody"
    );
    let restored_paragraph = test
        .get_block(&paragraph.id)
        .await
        .expect("the text is back");
    assert_eq!(
        restored_paragraph.created_by, editor_id,
        "the text keeps its creator, who still exists"
    );
    assert_eq!(
        restored_paragraph.parent_id, card.id,
        "the text is on the card"
    );

    assert_eq!(
        members_of(&test, &board.id).await,
        [editor_of(&board.id, &editor_id)],
        "the editor is a member again; the creator, who is gone, is not"
    );

    test.teardown().await;
}

/// As for a board: a block whose creator's row went while the block was in the trash
/// comes back as made by nobody.
#[tokio::test]
async fn undelete_block_restores_a_block_whose_creator_is_gone_as_made_by_nobody() {
    let test = common::setup().await;
    let owner_id = common::user(&test, "owner").await;
    let author_id = common::user(&test, "author").await;
    let workspace_id = common::workspace(&test).await;
    let board_id = stored_board(&test, &workspace_id, &owner_id).await;

    let mut card = new_block(&board_id, "", BlockType::Card);
    test.insert_block(&mut card, &author_id)
        .await
        .expect("the author's card is stored");
    let mut paragraph = new_block(&board_id, &card.id, BlockType::Attachment);
    test.insert_block(&mut paragraph, &owner_id)
        .await
        .expect("the owner's text is stored");

    // The card goes with the text on it. Nothing that exists names the author then.
    test.delete_block(&card.id, &owner_id)
        .await
        .expect("the card is deleted");
    common::delete_user_row(&test, &author_id).await;
    let in_trash = test
        .get_deleted_block(&card.id)
        .await
        .expect("the card is in the trash");
    assert_eq!(
        in_trash.block.created_by, author_id,
        "the trash describes what was"
    );

    let returned = test
        .undelete_block(&card.id, &owner_id)
        .await
        .expect("the card is restored though its creator is gone");
    assert_eq!(returned.created_by, "", "the card was made by nobody");
    let restored = test.get_block(&card.id).await.expect("the card is back");
    assert_eq!(returned, restored, "what is returned is what is stored");
    assert_eq!(
        restored,
        Block {
            created_by: String::new(),
            modified_by: owner_id.clone(),
            updated_at: restored.updated_at,
            ..card.clone()
        },
        "the card as it was, but for its creator"
    );
    let restored_paragraph = test
        .get_block(&paragraph.id)
        .await
        .expect("the text is back with its card");
    assert_eq!(
        restored_paragraph.created_by, owner_id,
        "the text keeps its creator, who still exists"
    );

    test.teardown().await;
}

/// A block that was deleted on its own is a deletion of its own: it does not come back
/// with the board, and it can be restored once the board is back.
#[tokio::test]
async fn a_block_deleted_before_its_board_stays_in_the_trash_when_the_board_is_restored() {
    let test = common::setup().await;
    let user_id = common::user(&test, "user").await;
    let workspace_id = common::workspace(&test).await;
    let board_id = stored_board(&test, &workspace_id, &user_id).await;

    let card = new_block(&board_id, "", BlockType::Card);
    let paragraph = new_block(&board_id, &card.id, BlockType::Card);
    let checkbox = new_block(&board_id, &paragraph.id, BlockType::Attachment);
    let (card_id, text_id, checkbox_id) =
        (card.id.clone(), paragraph.id.clone(), checkbox.id.clone());
    let mut blocks = [card, paragraph, checkbox];
    test.insert_blocks(&mut blocks, &user_id)
        .await
        .expect("the blocks are stored");

    // The text goes, with the checkbox on it; then the board, with the card.
    test.delete_block(&text_id, &user_id)
        .await
        .expect("the text is deleted");
    test.delete_container(&board_id, &user_id)
        .await
        .expect("the board is deleted");

    // While its board is in the trash a block has nothing to come back to.
    let error = test
        .undelete_block(&text_id, &user_id)
        .await
        .expect_err("the board of the text does not exist");
    assert!(error.is_not_found(), "got {error:?}");

    test.undelete_container(&board_id, &user_id)
        .await
        .expect("the board is restored");
    assert_eq!(
        common::sorted_ids(&blocks_of(&test, &board_id).await),
        [card_id.as_str()],
        "only the card went with the board, and only the card is back"
    );
    for block_id in [&text_id, &checkbox_id] {
        test.get_deleted_block(block_id)
            .await
            .expect("what was deleted on its own is still in the trash");
    }

    let restored = test
        .undelete_block(&text_id, &user_id)
        .await
        .expect("the text is restored");
    assert_eq!(restored.parent_id, card_id, "the text is back on its card");
    let back = blocks_of(&test, &board_id).await;
    assert_eq!(
        common::sorted_ids(&back),
        sorted([&card_id, &text_id, &checkbox_id]),
        "the text and the checkbox on it are back"
    );
    let checkbox = back
        .iter()
        .find(|block| block.id == checkbox_id)
        .expect("the checkbox is among the blocks");
    assert_eq!(checkbox.parent_id, text_id, "the checkbox is on the text");

    test.teardown().await;
}

/// Only what is in the trash is restored: a board that exists, an identifier no board
/// has had and text that is no identifier are not found, and nothing is written.
#[tokio::test]
async fn undelete_container_of_what_is_not_in_the_trash_is_not_found_and_writes_nothing() {
    let test = common::setup().await;
    let user_id = common::user(&test, "user").await;
    let other_user_id = common::user(&test, "other-user").await;
    let workspace_id = common::workspace_of(&test, &[&user_id, &other_user_id]).await;
    let board_id = stored_board(&test, &workspace_id, &user_id).await;
    let mut card = new_block(&board_id, "", BlockType::Card);
    test.insert_block(&mut card, &user_id)
        .await
        .expect("the card is stored");
    let board_before = test
        .get_container(&board_id)
        .await
        .expect("the board exists");
    let blocks_before = blocks_of(&test, &board_id).await;

    // A later modification time would show.
    tokio::time::sleep(Duration::from_millis(5)).await;
    let error = test
        .undelete_container(&board_id, &other_user_id)
        .await
        .expect_err("a board that exists is not in the trash");
    assert!(error.is_not_found(), "got {error:?}");
    let board_after = test
        .get_container(&board_id)
        .await
        .expect("the board exists");
    assert_eq!(board_after, board_before, "the board is as it was");
    assert_eq!(
        blocks_of(&test, &board_id).await,
        blocks_before,
        "its blocks are as they were"
    );

    for unknown_id in [new_id(), NOT_AN_ID.to_owned()] {
        let error = test
            .undelete_container(&unknown_id, &user_id)
            .await
            .expect_err("no such board is in the trash");
        assert!(error.is_not_found(), "board {unknown_id}: got {error:?}");
        let error = test
            .get_container(&unknown_id)
            .await
            .expect_err("no board was made");
        assert!(error.is_not_found(), "board {unknown_id}: got {error:?}");
        let error = test
            .get_deleted_container(&unknown_id)
            .await
            .expect_err("nothing was put in the trash");
        assert!(error.is_not_found(), "board {unknown_id}: got {error:?}");
    }
    let boards = test
        .get_containers_for_user_and_workspace(
            &user_id,
            &workspace_id,
            PublicContainers::Include,
            ContainerKinds::BOARDS,
        )
        .await
        .expect("the boards are read");
    assert_eq!(
        sorted_board_ids(&boards),
        [board_id.as_str()],
        "the one board of the workspace"
    );

    test.teardown().await;
}

#[tokio::test]
async fn purge_trash_removes_an_old_deleted_board_with_its_blocks_and_keeps_newer_trash() {
    let test = common::setup().await;
    let user_id = common::user(&test, "user").await;
    let workspace_id = common::workspace(&test).await;
    let (old_id, old_card_id, old_text_id) =
        board_with_card_and_text(&test, &workspace_id, &user_id).await;
    let (newer_id, newer_card_id, newer_text_id) =
        board_with_card_and_text(&test, &workspace_id, &user_id).await;

    test.delete_container(&old_id, &user_id)
        .await
        .expect("the old board is deleted");
    let cutoff = deleted_at(&test, &old_id).await + 1;
    // What follows is deleted at the cutoff or later.
    tokio::time::sleep(Duration::from_millis(5)).await;
    test.delete_container(&newer_id, &user_id)
        .await
        .expect("the newer board is deleted");

    let purged = test.purge_trash(cutoff).await.expect("the trash is purged");
    assert_eq!(
        purged,
        common::purged_without_files(3),
        "the old board and its two blocks"
    );

    let error = test
        .get_deleted_container(&old_id)
        .await
        .expect_err("the old board is gone for good");
    assert!(error.is_not_found(), "got {error:?}");
    for block_id in [&old_card_id, &old_text_id] {
        let error = test
            .get_deleted_block(block_id)
            .await
            .expect_err("the blocks of the old board are gone for good");
        assert!(error.is_not_found(), "block {block_id}: got {error:?}");
    }
    let error = test
        .undelete_container(&old_id, &user_id)
        .await
        .expect_err("there is nothing to restore");
    assert!(error.is_not_found(), "got {error:?}");

    test.undelete_container(&newer_id, &user_id)
        .await
        .expect("what is newer than the cutoff is restored");
    assert_eq!(
        common::sorted_ids(&blocks_of(&test, &newer_id).await),
        sorted([&newer_card_id, &newer_text_id]),
        "the blocks of the newer board are back"
    );

    test.teardown().await;
}

#[tokio::test]
async fn purge_trash_removes_a_block_deleted_on_its_own_with_its_purged_board() {
    let test = common::setup().await;
    let user_id = common::user(&test, "user").await;
    let workspace_id = common::workspace(&test).await;
    let (board_id, card_id, text_id) =
        board_with_card_and_text(&test, &workspace_id, &user_id).await;

    test.delete_block(&text_id, &user_id)
        .await
        .expect("the text is deleted on its own");
    test.delete_container(&board_id, &user_id)
        .await
        .expect("the board is deleted");
    let cutoff = deleted_at(&test, &board_id).await + 1;

    let purged = test.purge_trash(cutoff).await.expect("the trash is purged");
    assert_eq!(
        purged,
        common::purged_without_files(3),
        "the board, its card and the text deleted before"
    );
    for block_id in [&card_id, &text_id] {
        let error = test
            .get_deleted_block(block_id)
            .await
            .expect_err("the block is gone for good");
        assert!(error.is_not_found(), "block {block_id}: got {error:?}");
    }
    let again = test
        .purge_trash(cutoff)
        .await
        .expect("the trash is purged again");
    assert_eq!(
        again,
        common::purged_without_files(0),
        "nothing is left to purge"
    );

    test.teardown().await;
}

/// The cutoff itself is not old: what was deleted at that very millisecond stays.
#[tokio::test]
async fn purge_trash_keeps_what_was_deleted_at_the_cutoff_or_later() {
    let test = common::setup().await;
    let user_id = common::user(&test, "user").await;
    let workspace_id = common::workspace(&test).await;
    let (kept_board_id, kept_card_id, lone_text_id) =
        board_with_card_and_text(&test, &workspace_id, &user_id).await;
    let (board_id, card_id, text_id) =
        board_with_card_and_text(&test, &workspace_id, &user_id).await;

    // A block deleted on its own on a board that stays, and a board deleted whole.
    test.delete_block(&lone_text_id, &user_id)
        .await
        .expect("the text is deleted");
    let lone_deleted_at = test
        .get_deleted_block(&lone_text_id)
        .await
        .expect("the text is in the trash")
        .block
        .deleted_at
        .expect("deletion time");
    test.delete_container(&board_id, &user_id)
        .await
        .expect("the board is deleted");
    let cutoff = lone_deleted_at.min(deleted_at(&test, &board_id).await);

    let purged = test.purge_trash(cutoff).await.expect("the trash is purged");
    assert_eq!(
        purged,
        common::purged_without_files(0),
        "nothing was deleted before the cutoff {cutoff}"
    );

    let restored = test
        .undelete_block(&lone_text_id, &user_id)
        .await
        .expect("the text is restored");
    assert_eq!(
        restored.parent_id, kept_card_id,
        "the text is back on its card"
    );
    assert_eq!(
        common::sorted_ids(&blocks_of(&test, &kept_board_id).await),
        sorted([&kept_card_id, &lone_text_id]),
        "the board that stayed has both blocks again"
    );
    test.undelete_container(&board_id, &user_id)
        .await
        .expect("the board is restored");
    assert_eq!(
        common::sorted_ids(&blocks_of(&test, &board_id).await),
        sorted([&card_id, &text_id]),
        "the blocks of the deleted board are back"
    );

    test.teardown().await;
}

/// A built-in template is removed past the trash. A block of it that was deleted before
/// has nothing to come back to, and goes with the next purge however new it is.
#[tokio::test]
async fn purge_trash_removes_deleted_blocks_of_a_board_that_is_gone_for_good() {
    let test = common::setup().await;
    let user_id = common::user(&test, "user").await;
    let workspace_id = common::workspace(&test).await;

    let mut template = Container {
        is_template: true,
        ..new_board(&new_id(), &workspace_id, ContainerVisibility::Open)
    };
    test.insert_container(&mut template, "")
        .await
        .expect("the built-in template is stored");
    let mut orphan = new_block(&template.id, "", BlockType::Card);
    test.insert_block(&mut orphan, "")
        .await
        .expect("the card of the template is stored");
    let (_, _, kept_text_id) = board_with_card_and_text(&test, &workspace_id, &user_id).await;

    test.delete_block(&orphan.id, &user_id)
        .await
        .expect("the card of the template is deleted");
    test.delete_block(&kept_text_id, &user_id)
        .await
        .expect("the text of the other board is deleted");
    test.remove_builtin_templates()
        .await
        .expect("the built-in templates are removed");
    test.get_deleted_block(&orphan.id)
        .await
        .expect("the card is still in the trash");

    // Nothing is older than the beginning of time.
    let purged = test.purge_trash(0).await.expect("the trash is purged");
    assert_eq!(
        purged,
        common::purged_without_files(1),
        "the card whose board is gone for good"
    );
    let error = test
        .get_deleted_block(&orphan.id)
        .await
        .expect_err("the card is gone for good");
    assert!(error.is_not_found(), "got {error:?}");
    test.undelete_block(&kept_text_id, &user_id)
        .await
        .expect("a deleted block of a board that exists stays, and is restored");

    test.teardown().await;
}

/// A purge hands back the file records of the boards it removed for good, whole, and of
/// no other board: nothing else says where those files are.
#[tokio::test]
async fn purge_trash_returns_the_file_records_of_the_boards_it_removed() {
    let test = common::setup().await;
    let user_id = common::user(&test, "user").await;
    let workspace_id = common::workspace(&test).await;
    let (old_id, ..) = board_with_card_and_text(&test, &workspace_id, &user_id).await;
    let (newer_id, ..) = board_with_card_and_text(&test, &workspace_id, &user_id).await;
    let (kept_id, ..) = board_with_card_and_text(&test, &workspace_id, &user_id).await;

    let mut of_old = vec![file_of(&old_id, "7a.png"), file_of(&old_id, "7b.png")];
    of_old.sort_by(|a, b| a.id.cmp(&b.id));
    let of_newer = file_of(&newer_id, "7a.png");
    let of_kept = file_of(&kept_id, "7a.png");
    for file in of_old.iter().chain([&of_newer, &of_kept]) {
        test.save_file_record(file)
            .await
            .expect("the file record is stored");
    }

    test.delete_container(&old_id, &user_id)
        .await
        .expect("the old board is deleted");
    let cutoff = deleted_at(&test, &old_id).await + 1;
    // What follows is deleted at the cutoff or later.
    tokio::time::sleep(Duration::from_millis(5)).await;
    test.delete_container(&newer_id, &user_id)
        .await
        .expect("the newer board is deleted");

    let Purged { records, mut files } =
        test.purge_trash(cutoff).await.expect("the trash is purged");
    files.sort_by(|a, b| a.id.cmp(&b.id));
    assert_eq!(records, 3, "the old board and its two blocks");
    assert_eq!(
        files, of_old,
        "the file records of the board that went, as they were stored"
    );

    // The files of the board that is still in the trash come back with it, and those
    // of the board that exists were never touched.
    test.undelete_container(&newer_id, &user_id)
        .await
        .expect("what is newer than the cutoff is restored");
    for file in [&of_newer, &of_kept] {
        let stored = test
            .get_file_record(&file.container_id, &file.stored_name)
            .await
            .expect("the file record is there");
        assert_eq!(&stored, file, "as it was stored");
    }
    let again = test
        .purge_trash(cutoff)
        .await
        .expect("the trash is purged again");
    assert_eq!(
        again,
        common::purged_without_files(0),
        "nothing is left to purge, and no file record is handed back twice"
    );

    test.teardown().await;
}
