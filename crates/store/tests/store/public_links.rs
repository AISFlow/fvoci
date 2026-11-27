//! Public links to containers: the state that is set is the state that is read, a linked
//! container keeps its token until a new one is asked for, and turning the link off
//! forgets the token.
use fvoci_model::{PublicLink, PublicLinkChange, new_id};
use fvoci_store::StoreError;

use crate::common;

/// Creating, updating, and reading public link settings on a board.
#[tokio::test]
async fn set_public_link_and_get_public_link() {
    let test = common::setup().await;
    let store = &test.store;
    let user_id = common::user(store, "user-id").await;
    let user_id2 = common::user(store, "user-id2").await;
    let workspace_id = common::workspace(store).await;
    let boards = common::create_test_boards(store, &workspace_id, &user_id, 1).await;
    let board_id = &boards[0].id;

    let first = store
        .set_public_link(board_id, PublicLinkChange::On, "token", &user_id)
        .await
        .expect("the public link is turned on");
    assert_eq!(
        first,
        PublicLink {
            container_id: board_id.clone(),
            enabled: true,
            token: "token".to_owned(),
            modified_by: user_id.clone(),
            updated_at: first.updated_at,
        },
        "the state that was stored is returned"
    );
    assert!(first.updated_at > 0, "the store sets the time");
    let stored = store.get_public_link(board_id).await.expect("it is read");
    assert_eq!(stored, first, "and read");

    // Upsert under a new token, by another user.
    let second = store
        .set_public_link(
            board_id,
            PublicLinkChange::OnWithNewToken,
            "token2",
            &user_id2,
        )
        .await
        .expect("the public link gets a new token");
    assert_eq!(second.token, "token2", "the new token");
    assert_eq!(second.modified_by, user_id2, "the later user");
    let stored = store.get_public_link(board_id).await.expect("it is read");
    assert_eq!(stored, second, "the later state replaced the first");

    let error = store.get_public_link(&new_id()).await.unwrap_err();
    assert!(error.is_not_found(), "got {error:?}");

    test.teardown().await;
}

/// A board with an active public link keeps its token when turned on again; the offered
/// token is used only when one is needed. Turning the public link off forgets the token,
/// and turning it on again takes a new one.
#[tokio::test]
async fn a_token_is_kept_while_enabled_and_forgotten_when_disabled() {
    let test = common::setup().await;
    let store = &test.store;
    let user_id = common::user(store, "publisher").await;
    let workspace_id = common::workspace(store).await;
    let boards = common::create_test_boards(store, &workspace_id, &user_id, 2).await;
    let (board_id, other_board_id) = (&boards[0].id, &boards[1].id);
    let token_after = async |change: PublicLinkChange, offered: &str| {
        store
            .set_public_link(board_id, change, offered, &user_id)
            .await
            .expect("the public link is set")
            .token
    };

    assert_eq!(token_after(PublicLinkChange::On, "first").await, "first");
    assert_eq!(
        token_after(PublicLinkChange::On, "second").await,
        "first",
        "enabled already: the token stays"
    );
    assert_eq!(
        token_after(PublicLinkChange::OnWithNewToken, "third").await,
        "third",
        "a new token on request"
    );

    // Switched off by nobody in particular: the modifier reads as empty.
    let off = store
        .set_public_link(board_id, PublicLinkChange::Off, "unused", "")
        .await
        .expect("public link is turned off");
    assert!(!off.enabled, "got {off:?}");
    assert_eq!(off.token, "", "the token is forgotten");
    let stored = store.get_public_link(board_id).await.expect("it is read");
    assert_eq!(stored, off, "as stored");
    assert_eq!(stored.modified_by, "", "no user is stored as none");
    assert_eq!(
        token_after(PublicLinkChange::Off, "unused").await,
        "",
        "off twice is off"
    );
    assert_eq!(
        token_after(PublicLinkChange::On, "fourth").await,
        "fourth",
        "enabled again: the token that is offered now, not the old one"
    );

    // Two disabled boards both have no token; two enabled ones cannot share a token.
    let missing = store.get_public_link(other_board_id).await.unwrap_err();
    assert_eq!(missing.to_string(), "sql: no rows in result set");
    for board in [board_id, other_board_id] {
        store
            .set_public_link(board, PublicLinkChange::Off, "unused", &user_id)
            .await
            .expect("two boards without a token are no conflict");
    }
    store
        .set_public_link(board_id, PublicLinkChange::On, "same", &user_id)
        .await
        .expect("the first board takes the token");
    let error = store
        .set_public_link(other_board_id, PublicLinkChange::On, "same", &user_id)
        .await
        .expect_err("a token opens one board");
    assert!(matches!(error, StoreError::Database(_)), "got {error:?}");
    let other = store
        .get_public_link(other_board_id)
        .await
        .expect("it is read");
    assert!(!other.enabled, "the refused change changed nothing");

    test.teardown().await;
}

/// What does not exist cannot be given a public link: a board that is not there, by a
/// well-formed identifier or by other text, and on behalf of text that names no user.
#[tokio::test]
async fn public_link_needs_a_board_and_a_user_that_exist() {
    let test = common::setup().await;
    let store = &test.store;
    let user_id = common::user(store, "publisher").await;
    let workspace_id = common::workspace(store).await;
    let boards = common::create_test_boards(store, &workspace_id, &user_id, 1).await;
    let board_id = &boards[0].id;

    for nothing in [new_id(), "not-a-board".to_owned()] {
        let missing = store.get_public_link(&nothing).await.unwrap_err();
        assert!(matches!(missing, StoreError::NoRows), "got {missing:?}");
        let error = store
            .set_public_link(&nothing, PublicLinkChange::On, "token", &user_id)
            .await
            .unwrap_err();
        assert!(error.is_not_found(), "{nothing}: got {error:?}");
    }
    let error = store
        .set_public_link(board_id, PublicLinkChange::On, "token", "not-a-user")
        .await
        .unwrap_err();
    assert!(
        matches!(&error, StoreError::InvalidId(refused) if refused == "not-a-user"),
        "got {error:?}"
    );
    let missing = store.get_public_link(board_id).await.unwrap_err();
    assert!(matches!(missing, StoreError::NoRows), "got {missing:?}");

    test.teardown().await;
}

/// Reading the state and writing the new one are one step: concurrent requests turning on
/// a board's public link all get the token that was stored.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn requests_that_enable_a_public_link_at_once_agree_on_its_token() {
    const ROUNDS: usize = 25;
    const RACERS: usize = 6;
    let test = common::setup().await;
    let user_id = common::user(&test, "publisher").await;
    let workspace_id = common::workspace(&test).await;
    let boards = common::create_test_boards(&test, &workspace_id, &user_id, ROUNDS).await;

    for (round, board) in boards.iter().enumerate() {
        let mut racers = tokio::task::JoinSet::new();
        for racer in 0..RACERS {
            let store = test.store.clone();
            let (board_id, user_id) = (board.id.clone(), user_id.clone());
            racers.spawn(async move {
                let offered = format!("token-{round}-{racer}");
                store
                    .set_public_link(&board_id, PublicLinkChange::On, &offered, &user_id)
                    .await
                    .expect("the public link is enabled")
                    .token
            });
        }
        let answered = racers.join_all().await;

        let stored = test
            .get_public_link(&board.id)
            .await
            .expect("it is read")
            .token;
        assert!(
            answered.iter().all(|token| *token == stored),
            "round {round}: all are answered the stored token {stored}: {answered:?}"
        );
    }

    test.teardown().await;
}
