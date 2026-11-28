//! The stars of a user in a workspace (`getFavorites` and the routes beside it): what
//! they are put on, what is listed of them as the user reads it now, and who is told.

use fvoci_model::{
    ContainerMember, ContainerRole, ContainerVisibility, Favorite, FavoriteKind, IssuedApiToken,
};
use serde_json::{Value, json};

use crate::api_tokens_diaries::{Space, a_space_with_a_diary};
use crate::board_broadcasts::WorkspaceSocket;
use crate::board_views::add_view;
use crate::common::{Client, PASSWORD, Setup, TEST_WORKSPACE_ID, TEST_WS, TestServer, UNKNOWN_ID};
use crate::diaries::last_of;

/// The stars of the client in the workspace `ws`.
async fn favorites(client: &Client, ws: &str) -> Vec<Favorite> {
    client.get(&format!("{ws}/favorites")).await.ok_json()
}

/// What the stars are on, in their order.
async fn starred(client: &Client, ws: &str) -> Vec<String> {
    favorites(client, ws)
        .await
        .into_iter()
        .map(|favorite| favorite.target_id)
        .collect()
}

async fn star(client: &Client, ws: &str, target_id: &str) -> crate::common::ApiResponse {
    client
        .put(&format!("{ws}/favorites/{target_id}"), &Value::Null)
        .await
}

/// A card made on the board, by its identifier.
async fn card_on(client: &Client, board_id: &str, title: &str) -> String {
    let card: Value = client
        .post(
            &format!("{TEST_WS}/boards/{board_id}/cards?disableNotify=true"),
            &json!({"title": title}),
        )
        .await
        .created_json();
    card["id"].as_str().expect("an identifier").to_owned()
}

#[tokio::test]
async fn stars_are_listed_in_their_order_with_what_they_are_on_now() {
    let th = Setup::local_mode().start().await.init_basic().await;
    let client = &th.client;
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let card = card_on(client, &board.id, "the card").await;

    assert!(favorites(client, TEST_WS).await.is_empty(), "none yet");
    star(client, TEST_WS, &board.id).await.check_no_content();
    star(client, TEST_WS, &card).await.check_no_content();
    // Starred again, it stays where it was.
    star(client, TEST_WS, &board.id).await.check_no_content();

    // A view of the board, which a reference resolves for a board view to show, is
    // starred with its board alone.
    let view = add_view(client, &board.id, "Plan").await;
    star(client, TEST_WS, &view.id).await.check_not_found();

    let listed = favorites(client, TEST_WS).await;
    assert_eq!(listed.len(), 2, "{listed:?}");
    assert_eq!(listed[0].target_id, board.id);
    assert_eq!(listed[0].kind, FavoriteKind::Container);
    assert_eq!(listed[1].target_id, card);
    assert_eq!(listed[1].kind, FavoriteKind::Card);
    assert_eq!(listed[1].title, "the card");
    assert_eq!(listed[1].container_id, board.id);
    assert_eq!(listed[1].number, Some(1));

    // The title is read as it is now.
    client
        .patch(
            &format!("{TEST_WS}/cards/{card}"),
            &json!({"title": "renamed"}),
        )
        .await
        .check_ok();
    assert_eq!(favorites(client, TEST_WS).await[1].title, "renamed");

    client
        .put(&format!("{TEST_WS}/favorite-order"), &json!([card]))
        .await
        .check_no_content();
    assert_eq!(
        starred(client, TEST_WS).await,
        [card.clone(), board.id.clone()]
    );

    client
        .delete(&format!("{TEST_WS}/favorites/{card}"))
        .await
        .check_no_content();
    // Taken off again, or off nothing: no star there, one answer.
    client
        .delete(&format!("{TEST_WS}/favorites/{card}"))
        .await
        .check_not_found();
    client
        .delete(&format!("{TEST_WS}/favorites/not-an-id"))
        .await
        .check_not_found();
    assert_eq!(
        starred(client, TEST_WS).await,
        std::slice::from_ref(&board.id)
    );

    // Nothing, and no form of an identifier, are alike.
    star(client, TEST_WS, UNKNOWN_ID).await.check_not_found();
    star(client, TEST_WS, "not-an-id").await.check_not_found();
    client
        .put(&format!("{TEST_WS}/favorite-order"), &json!(["not-an-id"]))
        .await
        .check_bad_request();

    th.teardown().await;
}

#[tokio::test]
async fn a_star_on_what_the_user_does_not_read_now_is_left_out_until_they_do() {
    let th = Setup::local_mode().start().await.init_basic().await;
    let owner = &th.client;
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let card = card_on(owner, &board.id, "kept").await;
    let reader = th.add_user("reader").await;
    let reader_id = reader.me().await.id;
    owner
        .add_member(&ContainerMember::new(
            &board.id,
            reader_id.clone(),
            ContainerRole::Viewer,
        ))
        .await
        .check_ok();
    star(&reader, TEST_WS, &board.id).await.check_no_content();
    star(&reader, TEST_WS, &card).await.check_no_content();
    assert_eq!(starred(&reader, TEST_WS).await.len(), 2);

    // In the trash, the card is left out, and back again once it comes back.
    owner
        .post(
            &format!("{TEST_WS}/boards/{}/cards/batch-delete", board.id),
            &json!({"cardIds": [card]}),
        )
        .await
        .check_no_content();
    assert_eq!(
        starred(&reader, TEST_WS).await,
        std::slice::from_ref(&board.id)
    );
    owner
        .post(
            &format!("{TEST_WS}/boards/{}/cards/batch-restore", board.id),
            &json!({"cardIds": [card]}),
        )
        .await
        .check_ok();
    assert_eq!(starred(&reader, TEST_WS).await.len(), 2);

    // Out of the board, neither the board nor its card is listed, nor starred again.
    owner
        .delete(&format!(
            "{TEST_WS}/containers/{}/members/{reader_id}",
            board.id
        ))
        .await
        .check_no_content();
    assert_eq!(starred(&reader, TEST_WS).await, Vec::<String>::new());
    star(&reader, TEST_WS, &card).await.check_not_found();
    star(&reader, TEST_WS, &board.id).await.check_not_found();

    // Somebody else's stars are nobody's to see: the owner's list is their own.
    assert_eq!(starred(owner, TEST_WS).await, Vec::<String>::new());

    th.teardown().await;
}

#[tokio::test]
async fn no_star_goes_on_a_diary_nor_on_an_entry_and_no_token_reaches_the_stars() {
    let Space {
        th,
        ws,
        diary,
        diary_id,
        entry,
        board_id,
        agent,
        ..
    } = a_space_with_a_diary().await;
    let client = &th.client;
    let entry_id = last_of(&entry).to_owned();
    assert!(diary.ends_with(&diary_id));

    let unknown = star(client, &ws, UNKNOWN_ID).await;
    unknown.check_not_found();
    for id in [&diary_id, &entry_id] {
        let refused = star(client, &ws, id).await;
        refused.check_not_found();
        assert_eq!(
            refused.value()["detail"],
            unknown.value()["detail"],
            "a diary is as nothing is"
        );
    }
    // What is no diary in the space takes a star.
    star(client, &ws, &board_id).await.check_no_content();
    assert_eq!(starred(client, &ws).await, std::slice::from_ref(&board_id));

    // A personal API token is refused every route of the stars.
    for response in [
        agent.get(&format!("{ws}/favorites")).await,
        star(&agent, &ws, &board_id).await,
        agent.delete(&format!("{ws}/favorites/{board_id}")).await,
        agent
            .put(&format!("{ws}/favorite-order"), &json!([board_id]))
            .await,
    ] {
        response.check_forbidden();
        assert_eq!(response.error_reason(), "token_forbidden_endpoint");
    }

    th.teardown().await;
}

#[tokio::test]
async fn a_token_of_the_workspace_is_refused_the_stars_too() {
    let th: TestServer = Setup::local_mode().start().await.init_basic().await;
    let issued: IssuedApiToken = th
        .client
        .post(
            "/users/me/api-tokens",
            &json!({
                "name": "agent", "scope": "read", "workspaceIds": [TEST_WORKSPACE_ID],
                "password": PASSWORD,
            }),
        )
        .await
        .created_json();
    let agent = th.client_with_token(&issued.token);
    let response = agent.get(&format!("{TEST_WS}/favorites")).await;
    response.check_forbidden();
    assert_eq!(response.error_reason(), "token_forbidden_endpoint");
    th.teardown().await;
}

#[tokio::test]
async fn the_users_connections_to_the_workspace_are_told_and_nobody_elses() {
    let th = Setup::local_mode().start().await.init_basic().await;
    let me = th.client.me().await.id;
    let other_id = th.client2.me().await.id;
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let mut mine = WorkspaceSocket::subscribe(&th, &th.client, TEST_WORKSPACE_ID, &me).await;
    let mut theirs =
        WorkspaceSocket::subscribe(&th, &th.client2, TEST_WORKSPACE_ID, &other_id).await;
    mine.drain().await;
    theirs.drain().await;

    star(&th.client, TEST_WS, &board.id)
        .await
        .check_no_content();
    mine.expect("favoritesChanged", "/workspaceId", TEST_WORKSPACE_ID)
        .await;
    let told: Vec<Value> = theirs.drain().await;
    assert!(
        told.iter()
            .all(|message| message["type"] != "favoritesChanged"),
        "{told:?}"
    );
    // A star that was there already tells nothing.
    star(&th.client, TEST_WS, &board.id)
        .await
        .check_no_content();
    let again: Vec<Value> = mine.drain().await;
    assert!(
        again
            .iter()
            .all(|message| message["type"] != "favoritesChanged"),
        "{again:?}"
    );

    th.teardown().await;
}
