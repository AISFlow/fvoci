//! Reactions to blocks: there once and gone when taken off, at most twenty emoji on a
//! block, nothing of the block written, into the trash with it and back (also one added
//! as the block goes, those of cards that go and come back together, and those of a
//! workspace's guests), and gone with whoever leaves the organization, is no longer a
//! guest of any of its workspaces, or is on the way out of their account.

use std::collections::HashMap;

use fvoci_model::{
    Block, BlockType, Emoji, REACTION_KINDS_PER_BLOCK, Reaction, WorkspaceRole, new_id,
};
use fvoci_store::{DbType, Departure, MembershipChange, Reacted, Store, StoreError, Tenant};

use crate::common::{self, TestStore, db_type};
use crate::lock_gate::{Gate, ended};

fn emoji(text: &str) -> Emoji {
    text.parse().expect("an emoji")
}

/// The reactions `emoji` of the users, in that order.
fn reaction(text: &str, user_ids: &[&str]) -> Reaction {
    Reaction {
        emoji: emoji(text),
        user_ids: user_ids.iter().map(|&id| id.to_owned()).collect(),
    }
}

/// A card on a board of a workspace that the users work in, and a comment on it.
async fn card_and_comment(store: &Store, users: &[&str]) -> (Block, Block) {
    let workspace_id = common::workspace_of(store, users).await;
    card_and_comment_in(store, &workspace_id, users[0]).await
}

/// A card on a board of the workspace `workspace_id` that `user_id` makes, and a
/// comment on it.
async fn card_and_comment_in(store: &Store, workspace_id: &str, user_id: &str) -> (Block, Block) {
    let boards = common::create_test_boards(store, workspace_id, user_id, 1).await;
    let card = Block {
        id: new_id(),
        container_id: boards[0].id.clone(),
        block_type: BlockType::Card,
        ..Block::default()
    };
    let comment = Block {
        id: new_id(),
        container_id: boards[0].id.clone(),
        parent_id: card.id.clone(),
        block_type: BlockType::Comment,
        ..Block::default()
    };
    let mut blocks = [card, comment];
    common::insert_blocks(store, &mut blocks, user_id).await;
    let [card, comment] = blocks;
    (card, comment)
}

async fn react(store: &Store, block_id: &str, user_id: &str, text: &str) -> Reacted {
    store
        .add_reaction(block_id, user_id, &emoji(text))
        .await
        .expect("the reaction is added")
}

async fn reactions(store: &Store, block_id: &str) -> Vec<Reaction> {
    store
        .reactions_of(&[block_id])
        .await
        .expect("the reactions are read")
        .remove(block_id)
        .unwrap_or_default()
}

async fn rows(store: &Store) -> i64 {
    store
        .system()
        .count_rows("reactions")
        .await
        .expect("counted")
}

#[tokio::test]
async fn a_reaction_is_there_once_and_goes_when_taken_off() {
    let test = common::setup().await;
    let (ann, bob) = (
        common::user(&test, "ann").await,
        common::user(&test, "bob").await,
    );
    let (card, comment) = card_and_comment(&test, &[&ann, &bob]).await;

    assert_eq!(react(&test, &comment.id, &ann, "🎉").await, Reacted::Added);
    assert_eq!(
        react(&test, &comment.id, &ann, "🎉").await,
        Reacted::AlreadyThere,
        "a second time adds nothing"
    );
    assert_eq!(react(&test, &comment.id, &bob, "🎉").await, Reacted::Added);
    assert_eq!(react(&test, &comment.id, &bob, "👍").await, Reacted::Added);
    assert_eq!(react(&test, &card.id, &bob, "👀").await, Reacted::Added);
    let both = test
        .reactions_of(&[&comment.id, &card.id, &new_id()])
        .await
        .expect("read");
    assert_eq!(
        both,
        HashMap::from([
            (
                comment.id.clone(),
                vec![reaction("🎉", &[&ann, &bob]), reaction("👍", &[&bob])]
            ),
            (card.id.clone(), vec![reaction("👀", &[&bob])]),
        ]),
        "by block, the blocks without any left out"
    );

    let taken_off = test.remove_reaction(&comment.id, &ann, &emoji("🎉")).await;
    assert!(taken_off.expect("taken off"), "there was one");
    let again = test.remove_reaction(&comment.id, &ann, &emoji("🎉")).await;
    assert!(!again.expect("asked"), "there is none any more");
    assert_eq!(
        reactions(&test, &comment.id).await,
        [reaction("🎉", &[&bob]), reaction("👍", &[&bob])]
    );
    test.teardown().await;
}

#[tokio::test]
async fn a_block_is_answered_with_twenty_emoji_at_most() {
    let test = common::setup().await;
    let (ann, bob) = (
        common::user(&test, "ann").await,
        common::user(&test, "bob").await,
    );
    let (_, comment) = card_and_comment(&test, &[&ann, &bob]).await;
    let animals: Vec<String> = ('\u{1F400}'..='\u{1F43F}').map(String::from).collect();
    for animal in &animals[..REACTION_KINDS_PER_BLOCK] {
        assert_eq!(
            react(&test, &comment.id, &ann, animal).await,
            Reacted::Added
        );
    }
    let one_more = &animals[REACTION_KINDS_PER_BLOCK];
    assert_eq!(
        react(&test, &comment.id, &bob, one_more).await,
        Reacted::TooManyKinds,
        "a twenty-first emoji is refused"
    );
    assert_eq!(
        react(&test, &comment.id, &bob, &animals[0]).await,
        Reacted::Added,
        "one of the twenty is not"
    );
    let kinds = reactions(&test, &comment.id).await;
    assert_eq!(kinds.len(), REACTION_KINDS_PER_BLOCK);
    assert_eq!(kinds[0], reaction(&animals[0], &[&ann, &bob]));
    test.teardown().await;
}

#[tokio::test]
async fn a_reaction_writes_nothing_of_the_block() {
    let test = common::setup().await;
    let (ann, bob) = (
        common::user(&test, "ann").await,
        common::user(&test, "bob").await,
    );
    let (card, comment) = card_and_comment(&test, &[&ann, &bob]).await;
    let before = [
        test.get_block(&card.id).await.expect("the card"),
        test.get_block(&comment.id).await.expect("the comment"),
    ];
    react(&test, &card.id, &bob, "🚀").await;
    react(&test, &comment.id, &bob, "❤️").await;
    test.remove_reaction(&card.id, &bob, &emoji("🚀"))
        .await
        .expect("taken off");
    let after = [
        test.get_block(&card.id).await.expect("the card"),
        test.get_block(&comment.id).await.expect("the comment"),
    ];
    assert_eq!(after, before, "the blocks are as they were");
    test.teardown().await;
}

#[tokio::test]
async fn whoever_is_not_in_the_workspace_reacts_to_nothing() {
    let test = common::setup().await;
    let ann = common::user(&test, "ann").await;
    let stranger = common::user(&test, "stranger").await;
    let gone = common::user(&test, "gone").await;
    let (card, _) = card_and_comment(&test, &[&ann, &gone]).await;
    let refused = |result: Result<Reacted, StoreError>| match result {
        Err(StoreError::NotFound(_)) => "not found",
        Err(StoreError::UserNotFound(_)) => "no account in use",
        other => panic!("refused: {other:?}"),
    };
    assert_eq!(
        refused(test.add_reaction(&card.id, &stranger, &emoji("👍")).await),
        "not found"
    );
    assert_eq!(
        refused(test.add_reaction(&new_id(), &ann, &emoji("👍")).await),
        "not found"
    );
    test.execute_unchecked(&format!(
        "UPDATE users SET deleted_at = 1 WHERE id = '{gone}'"
    ))
    .await
    .expect("the account is put out of use");
    assert_eq!(
        refused(test.add_reaction(&card.id, &gone, &emoji("👍")).await),
        "no account in use"
    );
    assert_eq!(rows(&test).await, 0);
    test.teardown().await;
}

/// A card that goes into the trash takes the reactions to it and to its comment along,
/// and brings them back as they were: those of an account erased meanwhile come back
/// to nothing.
#[tokio::test]
async fn reactions_go_into_the_trash_with_the_block_and_come_back() {
    let test = common::setup().await;
    let ann = common::user(&test, "ann").await;
    let bob = common::user(&test, "bob").await;
    let erased = common::user(&test, "erased").await;
    let (card, comment) = card_and_comment(&test, &[&ann, &bob, &erased]).await;
    for (block, user, text) in [
        (&card.id, &bob, "👍"),
        (&card.id, &erased, "👍"),
        (&card.id, &ann, "🎉"),
        (&comment.id, &erased, "😄"),
        (&comment.id, &ann, "😄"),
    ] {
        react(&test, block, user, text).await;
    }
    let before = (
        reactions(&test, &card.id).await,
        reactions(&test, &comment.id).await,
    );

    test.delete_block(&card.id, &ann)
        .await
        .expect("to the trash");
    assert_eq!(rows(&test).await, 0, "they are in the trash with the card");
    test.undelete_block(&card.id, &ann)
        .await
        .expect("out of the trash");
    let back = (
        reactions(&test, &card.id).await,
        reactions(&test, &comment.id).await,
    );
    assert_eq!(back, before, "they come back as they were");

    test.delete_block(&card.id, &ann)
        .await
        .expect("to the trash again");
    test.execute_unchecked(&format!(
        "UPDATE users SET deleted_at = 1, erase_started_at = 1, erased_at = 1 \
         WHERE id = '{erased}'"
    ))
    .await
    .expect("the account is erased");
    test.undelete_block(&card.id, &ann)
        .await
        .expect("out of the trash");
    assert_eq!(
        (
            reactions(&test, &card.id).await,
            reactions(&test, &comment.id).await,
        ),
        (
            vec![reaction("👍", &[&bob]), reaction("🎉", &[&ann])],
            vec![reaction("😄", &[&ann])],
        ),
        "those of the erased account do not"
    );
    test.teardown().await;
}

/// Gives the user a stored password hash, so that leaving is not refused to an account
/// that could not sign in again.
async fn give_password(test: &TestStore, user_id: &str) {
    test.execute_unchecked(&format!(
        "UPDATE users SET password_hash = '$argon2id$stub' WHERE id = '{user_id}'"
    ))
    .await
    .expect("password set");
}

/// Whoever leaves the organization takes their reactions out of it, also those its
/// trash keeps; those of others stay.
#[tokio::test]
async fn whoever_leaves_the_organization_takes_their_reactions_along() {
    let test = common::setup().await;
    let founder = common::user(&test, "founder").await;
    let leaver = common::user(&test, "leaver").await;
    give_password(&test, &founder).await;
    give_password(&test, &leaver).await;
    let (card, comment) = card_and_comment(&test, &[&founder, &leaver]).await;
    for (block, user, text) in [
        (&card.id, &leaver, "👍"),
        (&card.id, &founder, "👍"),
        (&comment.id, &leaver, "👀"),
        (&comment.id, &founder, "🚀"),
    ] {
        react(&test, block, user, text).await;
    }
    test.delete_block(&comment.id, &founder)
        .await
        .expect("the comment to the trash");

    let left = test.leave_organization(&leaver).await.expect("asked");
    assert!(matches!(left, Departure::Left(_)), "they leave: {left:?}");

    assert_eq!(
        reactions(&test, &card.id).await,
        [reaction("👍", &[&founder])]
    );
    let kept = test
        .system()
        .texts_unchecked(&format!(
            "SELECT CAST(block AS TEXT) FROM deleted_blocks WHERE id = '{}'",
            comment.id
        ))
        .await
        .expect("read");
    assert!(
        kept.len() == 1 && !kept[0].contains(&leaver),
        "the trash keeps none of theirs: {kept:?}"
    );
    test.undelete_block(&comment.id, &founder)
        .await
        .expect("out of the trash");
    assert_eq!(
        reactions(&test, &comment.id).await,
        [reaction("🚀", &[&founder])]
    );
    test.teardown().await;
}

/// A reaction added while its block is going into the trash waits for it and then finds
/// no block; the trash keeps what was there, and nothing is lost on the way back.
/// PostgreSQL only: SQLite takes one write at a time.
#[tokio::test]
async fn a_reaction_added_as_the_block_goes_into_the_trash_is_not_lost() {
    if db_type() != DbType::Postgres {
        return;
    }
    let test = common::setup().await;
    let ann = common::user(&test, "ann").await;
    let bob = common::user(&test, "bob").await;
    let (card, comment) = card_and_comment(&test, &[&ann, &bob]).await;
    react(&test, &comment.id, &ann, "👍").await;

    // The deletion stops once it read what goes to the trash, before it writes there.
    let mut gate = Gate::before(&test, "INSERT", "deleted_blocks", "true").await;
    let second = gate.second_store(&test).await;
    let (first, deleter) = (test.store.clone(), ann.clone());
    let card_id = card.id.clone();
    let deleting = tokio::spawn(async move { first.delete_block(&card_id, &deleter).await });
    let holder = gate.stopped(&deleting).await;
    let (comment_id, adder) = (comment.id.clone(), bob.clone());
    let reacting =
        tokio::spawn(async move { second.add_reaction(&comment_id, &adder, &emoji("🎉")).await });
    gate.waits_for(holder, &reacting).await;
    gate.open().await;

    ended(deleting).await.expect("to the trash");
    let reacted = ended(reacting).await;
    assert!(
        matches!(reacted, Err(StoreError::NotFound(_))),
        "the block is gone by then: {reacted:?}"
    );
    test.undelete_block(&card.id, &ann)
        .await
        .expect("out of the trash");
    assert_eq!(
        reactions(&test, &comment.id).await,
        [reaction("👍", &[&ann])]
    );
    test.teardown().await;
}

/// The reaction of an account that is asked to be deleted does not come back with its
/// block, as such an account adds none; and on PostgreSQL neither does that of one who
/// is no longer in the organization of the block.
#[tokio::test]
async fn reactions_come_back_of_those_still_here_alone() {
    let test = common::setup().await;
    let ann = common::user(&test, "ann").await;
    let going = common::user(&test, "going").await;
    let gone = common::user(&test, "gone").await;
    let (card, _) = card_and_comment(&test, &[&ann, &going, &gone]).await;
    for user in [&ann, &going, &gone] {
        react(&test, &card.id, user, "👍").await;
    }
    test.delete_block(&card.id, &ann)
        .await
        .expect("to the trash");
    test.execute_unchecked(&format!(
        "UPDATE users SET deleted_at = 1 WHERE id = '{going}'"
    ))
    .await
    .expect("the account is asked to go");
    let postgres = db_type() == DbType::Postgres;
    if postgres {
        // Out of the organization and its workspaces, as a leaving takes them.
        for table in ["workspace_members", "organization_members"] {
            test.execute_unchecked(&format!("DELETE FROM {table} WHERE user_id = '{gone}'"))
                .await
                .expect("out of the organization");
        }
    }
    test.undelete_block(&card.id, &ann)
        .await
        .expect("out of the trash");
    let back: Vec<&str> = if postgres {
        vec![&ann]
    } else {
        vec![&ann, &gone]
    };
    assert_eq!(reactions(&test, &card.id).await, [reaction("👍", &back)]);
    test.teardown().await;
}

/// A guest of a workspace is in no `organization_members`: their reaction comes back
/// with its block while they are a guest. Removed from the last workspace of the
/// organization they were a guest of, they take their reactions along, those the trash
/// keeps too, as a leaving does.
#[tokio::test]
async fn a_guests_reactions_come_back_while_they_are_one() {
    let test = common::setup().await;
    let ann = common::user(&test, "ann").await;
    let outside = test.scoped(Tenant::Unset);
    let guest = common::user(&outside, "guest").await;
    let leaving = common::user(&outside, "leaving").await;
    let workspace_id = common::workspace_of(&test, &[&ann]).await;
    let (card, comment) = card_and_comment_in(&test, &workspace_id, &ann).await;
    for user in [&guest, &leaving] {
        test.add_workspace_member(&workspace_id, user, WorkspaceRole::Guest)
            .await
            .expect("a guest");
    }
    for user in [&ann, &guest, &leaving] {
        react(&test, &card.id, user, "👍").await;
        react(&test, &comment.id, user, "🎉").await;
    }
    test.delete_block(&comment.id, &ann)
        .await
        .expect("to the trash");

    let removed = test
        .remove_workspace_member(&workspace_id, &leaving, &ann)
        .await
        .expect("asked");
    assert!(
        matches!(removed, MembershipChange::Done { .. }),
        "{removed:?}"
    );
    assert_eq!(
        reactions(&test, &card.id).await,
        [reaction("👍", &[&ann, &guest])],
        "a reaction to a block still there goes with them"
    );
    test.undelete_block(&comment.id, &ann)
        .await
        .expect("out of the trash");
    assert_eq!(
        reactions(&test, &comment.id).await,
        [reaction("🎉", &[&ann, &guest])],
        "the guest's comes back, and none of whoever is no longer one"
    );
    test.teardown().await;
}

/// Cards that go into the trash together, and come back together, bring back the
/// reactions of all: in whatever order they are named, each block and each account is
/// held once, in the order of their identifiers.
#[tokio::test]
async fn cards_that_go_and_come_back_together_keep_their_reactions() {
    let test = common::setup().await;
    let ann = common::user(&test, "ann").await;
    let bob = common::user(&test, "bob").await;
    let (first, _) = card_and_comment(&test, &[&ann, &bob]).await;
    let second = Block {
        id: new_id(),
        container_id: first.container_id.clone(),
        block_type: BlockType::Card,
        ..Block::default()
    };
    let mut cards = [second];
    common::insert_blocks(&test, &mut cards, &ann).await;
    let [second] = cards;
    let (low, high) = if first.id < second.id {
        (&first, &second)
    } else {
        (&second, &first)
    };
    let (earlier, later) = if ann < bob {
        (&ann, &bob)
    } else {
        (&bob, &ann)
    };
    // The card held first is answered by the account held last, and the other way round.
    react(&test, &low.id, later, "👍").await;
    react(&test, &high.id, earlier, "🎉").await;

    let board = &first.container_id;
    test.batch_delete_cards(board, &[high.id.clone(), low.id.clone()], &ann)
        .await
        .expect("both to the trash");
    assert_eq!(rows(&test).await, 0);
    test.batch_restore_cards(board, &[low.id.clone(), high.id.clone()], &ann)
        .await
        .expect("both out of the trash");
    assert_eq!(
        (
            reactions(&test, &low.id).await,
            reactions(&test, &high.id).await,
        ),
        (
            vec![reaction("👍", &[later])],
            vec![reaction("🎉", &[earlier])],
        )
    );
    test.teardown().await;
}
