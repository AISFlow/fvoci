//! What the category routes tell over websocket connections.

use std::time::Duration;

use futures_util::{SinkExt as _, StreamExt as _};
use fvoci_model::{Category, CategoryContainerMetadata, Container, ContainerVisibility};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

use crate::common::ROOT_WS;
use crate::common::{Client, ROOT_WORKSPACE_ID, Setup, TestServer};
use crate::sidebar::{
    category, category_boards, create_category, file_board, open_board, put_empty,
};

/// How long to wait for a message that should arrive.
const PATIENCE: Duration = Duration::from_secs(5);

type Stream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// The next JSON message on the socket, if one arrives in time.
async fn next_message(stream: &mut Stream, wait: Duration) -> Option<Value> {
    loop {
        match tokio::time::timeout(wait, stream.next()).await {
            Ok(Some(Ok(Message::Text(text)))) => {
                return Some(serde_json::from_str(&text).expect("messages are JSON"));
            }
            Ok(Some(Ok(Message::Ping(_) | Message::Pong(_)))) => {}
            _ => return None,
        }
    }
}

/// A socket of the first client's user, subscribed to the workspace.
async fn subscribed_socket(th: &TestServer, workspace_id: &str, user_id: &str) -> Stream {
    subscribed_socket_of(th, &th.client, workspace_id, user_id).await
}

/// A socket of the client's user, `user_id`, subscribed to the workspace.
async fn subscribed_socket_of(
    th: &TestServer,
    client: &Client,
    workspace_id: &str,
    user_id: &str,
) -> Stream {
    let mut stream = client.connect_websocket().await;
    let command = json!({"type": "subscribeWorkspace", "workspaceId": workspace_id});
    stream
        .send(Message::Text(command.to_string().into()))
        .await
        .expect("the command is sent");
    // Commands are not acknowledged: wait until a broadcast comes through.
    let probe = category(workspace_id, user_id, "subscription probe");
    for _ in 0..50 {
        th.app.ws().broadcast_category_change(&probe).await;
        if next_message(&mut stream, Duration::from_millis(100))
            .await
            .is_some()
        {
            while next_message(&mut stream, Duration::from_millis(150))
                .await
                .is_some()
            {}
            return stream;
        }
    }
    panic!("the socket was never subscribed to workspace {workspace_id}");
}

/// The next message, which must arrive.
async fn expect_message(stream: &mut Stream, after: &str) -> Value {
    next_message(stream, PATIENCE)
        .await
        .unwrap_or_else(|| panic!("a message arrives after {after}"))
}

/// Creating, updating, reordering and deleting categories is sent to the user's
/// connection.
#[tokio::test]
async fn category_changes_are_sent_to_the_users_connection() {
    let th = Setup::native().start().await.init_basic().await;
    let user = th.client.me().await;
    let default_id = category_boards(&th.client, ROOT_WORKSPACE_ID).await[0]
        .category
        .id
        .clone();
    let mut socket = subscribed_socket(&th, ROOT_WORKSPACE_ID, &user.id).await;

    let created =
        create_category(&th.client, &category(ROOT_WORKSPACE_ID, &user.id, "Cat A")).await;
    assert_eq!(
        expect_message(&mut socket, "creating a category").await,
        json!({"type": "categoryChanged", "workspaceId": ROOT_WORKSPACE_ID, "category": created}),
        "after creating a category"
    );

    let path = format!("{ROOT_WS}/categories/{}", created.id);
    let change = Category {
        name: "Cat B".to_owned(),
        ..created.clone()
    };
    let renamed: Category = th.client.put(&path, &change).await.ok_json();
    assert_eq!(
        expect_message(&mut socket, "updating a category").await,
        json!({"type": "categoryChanged", "workspaceId": ROOT_WORKSPACE_ID, "category": renamed}),
        "after updating a category"
    );

    let category_order = [default_id.as_str(), created.id.as_str()];
    th.client
        .put(&format!("{ROOT_WS}/category-order"), &category_order)
        .await
        .check_ok();
    assert_eq!(
        expect_message(&mut socket, "reordering the categories").await,
        json!({
            "type": "categoriesReordered",
            "categoryOrder": category_order,
            "workspaceId": ROOT_WORKSPACE_ID,
        }),
        "after reordering the categories"
    );

    // Deleting moves the boards to the default category first, which is its own message.
    let board_id = open_board(&th, ROOT_WORKSPACE_ID).await;
    file_board(&th.client, ROOT_WORKSPACE_ID, &created.id, &board_id).await;
    expect_message(&mut socket, "filing a board").await;
    let stored = th
        .app
        .category(&created.id)
        .await
        .expect("the category is stored");
    th.client.delete(&path).await.check_no_content();
    let moved = json!({"containerId": board_id, "categoryId": default_id, "hidden": false});
    assert_eq!(
        expect_message(&mut socket, "deleting a category").await,
        json!({"type": "containerCategoriesChanged", "workspaceId": ROOT_WORKSPACE_ID, "containerCategories": [moved]}),
        "after deleting a category: its board is in the default category"
    );
    // The answer has no body: the message is where the deleted category is told.
    let message = expect_message(&mut socket, "deleting a category").await;
    let deleted_at = message["category"]["deletedAt"]
        .as_i64()
        .unwrap_or_default();
    assert!(deleted_at > 0, "marked as deleted: {message}");
    let deleted = Category {
        deleted_at: Some(deleted_at),
        ..stored
    };
    assert_eq!(
        message,
        json!({"type": "categoryChanged", "workspaceId": ROOT_WORKSPACE_ID, "category": deleted}),
        "after deleting a category: the category as it was"
    );

    th.teardown().await;
}

/// Filing, hiding, showing and reordering the boards of a category is sent to the user's
/// connection.
#[tokio::test]
async fn category_container_changes_are_sent_to_the_users_connection() {
    let th = Setup::native().start().await.init_basic().await;
    let user = th.client.me().await;
    category_boards(&th.client, ROOT_WORKSPACE_ID).await;
    let created =
        create_category(&th.client, &category(ROOT_WORKSPACE_ID, &user.id, "Cat A")).await;
    let path = format!("{ROOT_WS}/categories/{}", created.id);
    let mut socket = subscribed_socket(&th, ROOT_WORKSPACE_ID, &user.id).await;
    let board_change = |boards: Value| json!({"type": "containerCategoriesChanged", "workspaceId": ROOT_WORKSPACE_ID, "containerCategories": boards});
    let first = open_board(&th, ROOT_WORKSPACE_ID).await;
    let second = open_board(&th, ROOT_WORKSPACE_ID).await;
    let shown = json!({"containerId": first, "categoryId": created.id, "hidden": false});
    let hidden = json!({"containerId": first, "categoryId": created.id, "hidden": true});

    // Filing a board sends all the boards of the category.
    file_board(&th.client, ROOT_WORKSPACE_ID, &created.id, &first).await;
    assert_eq!(
        expect_message(&mut socket, "filing a board").await,
        board_change(json!([shown])),
        "after filing a board"
    );
    file_board(&th.client, ROOT_WORKSPACE_ID, &created.id, &second).await;
    let message = expect_message(&mut socket, "filing a second board").await;
    assert_eq!(
        message["containerCategories"].as_array().map(Vec::len),
        Some(2),
        "after filing a second board: {message}"
    );

    // Hiding and showing send the one board.
    put_empty(&th.client, &format!("{path}/containers/{first}/hide"))
        .await
        .check_no_content();
    assert_eq!(
        expect_message(&mut socket, "hiding a board").await,
        board_change(json!([hidden])),
        "after hiding a board"
    );
    put_empty(&th.client, &format!("{path}/containers/{first}/unhide"))
        .await
        .check_no_content();
    assert_eq!(
        expect_message(&mut socket, "showing a board").await,
        board_change(json!([shown])),
        "after showing a board"
    );

    let board_order = [second.as_str(), first.as_str()];
    th.client
        .put(&format!("{path}/container-order"), &board_order)
        .await
        .check_ok();
    assert_eq!(
        expect_message(&mut socket, "reordering the boards").await,
        json!({
            "type": "categoryContainersReordered",
            "categoryId": created.id,
            "containerOrder": board_order,
            "workspaceId": ROOT_WORKSPACE_ID,
        }),
        "after reordering the boards"
    );

    th.teardown().await;
}

/// Concurrent first-time requests from a new user create a single default category
/// between them rather than racing to create duplicates.
#[tokio::test]
async fn the_default_category_is_created_once_by_concurrent_requests() {
    let th = Setup::native().start().await.init_basic().await;
    let board = |title: String| {
        let client = th.client.clone();
        async move {
            let body =
                json!({"workspaceId": ROOT_WORKSPACE_ID, "visibility": "private", "title": title});
            client.post(&format!("{ROOT_WS}/boards"), &body).await
        }
    };

    let responses =
        futures_util::future::join_all((0..12).map(|n| board(format!("board {n}")))).await;
    for response in &responses {
        response.check_created();
    }

    let categories = th
        .client
        .get(&format!("{ROOT_WS}/categories"))
        .await
        .value();
    let categories = categories.as_array().expect("a list of categories");
    assert_eq!(categories.len(), 1, "one default category: {categories:?}");
    assert_eq!(
        categories[0]["name"],
        json!("Boards"),
        "default category name"
    );
    assert_eq!(
        categories[0]["containerMetadata"].as_array().map(Vec::len),
        Some(12),
        "with all twelve boards filed"
    );
    th.teardown().await;
}

/// The first message of the action that arrives, passing over the others.
async fn message_of(stream: &mut Stream, action: &str) -> Value {
    loop {
        let message = expect_message(stream, action).await;
        if message["type"] == json!(action) {
            return message;
        }
    }
}

/// A deleted board that comes back is where each user had filed it, hidden as it was, or
/// in the user's default category if that one went meanwhile; each such user's connection
/// is told the board's
/// entry and the category's order, and a user who had not filed it is told nothing.
#[tokio::test]
async fn a_restored_board_is_filed_where_it_was_and_the_users_are_told() {
    let th = Setup::native().start().await.init_basic().await;
    let user = th.client.me().await;
    let other = th.client2.me().await;
    let third = th.add_user("third").await;
    let third_user = third.me().await;
    let other_default = category_boards(&th.client2, ROOT_WORKSPACE_ID).await[0]
        .category
        .id
        .clone();
    let created =
        create_category(&th.client, &category(ROOT_WORKSPACE_ID, &user.id, "Cat A")).await;
    let gone = create_category(
        &th.client2,
        &category(ROOT_WORKSPACE_ID, &other.id, "Cat B"),
    )
    .await;
    let board: Container = th
        .client
        .create_board(&Container {
            workspace_id: ROOT_WORKSPACE_ID.to_owned(),
            visibility: ContainerVisibility::Open,
            ..Container::default()
        })
        .await
        .created_json();
    file_board(&th.client, ROOT_WORKSPACE_ID, &created.id, &board.id).await;
    file_board(&th.client2, ROOT_WORKSPACE_ID, &gone.id, &board.id).await;
    let category_path = format!("{ROOT_WS}/categories/{}", created.id);
    put_empty(
        &th.client,
        &format!("{category_path}/containers/{}/hide", board.id),
    )
    .await
    .check_no_content();
    let mut socket = subscribed_socket(&th, ROOT_WORKSPACE_ID, &user.id).await;
    let mut other_socket =
        subscribed_socket_of(&th, &th.client2, ROOT_WORKSPACE_ID, &other.id).await;
    let mut third_socket =
        subscribed_socket_of(&th, &third, ROOT_WORKSPACE_ID, &third_user.id).await;

    th.client
        .delete(&format!("{ROOT_WS}/containers/{}", board.id))
        .await
        .check_no_content();
    th.client2
        .delete(&format!("{ROOT_WS}/categories/{}", gone.id))
        .await
        .check_no_content();
    th.client
        .post_empty(&format!("{ROOT_WS}/containers/{}/restore", board.id))
        .await
        .check_ok();

    let entry = |category_id: &str, hidden: bool| {
        json!({
            "type": "containerCategoriesChanged",
            "workspaceId": ROOT_WORKSPACE_ID,
            "containerCategories": [{"containerId": board.id, "categoryId": category_id, "hidden": hidden}],
        })
    };
    assert_eq!(
        message_of(&mut socket, "containerCategoriesChanged").await,
        entry(&created.id, true)
    );
    let order = message_of(&mut socket, "categoryContainersReordered").await;
    assert_eq!(order["categoryId"], json!(created.id), "{order}");
    assert_eq!(order["containerOrder"], json!([board.id]), "{order}");
    assert_eq!(
        message_of(&mut other_socket, "containerCategoriesChanged").await,
        entry(&other_default, false),
        "the other user's category went: the default one"
    );
    while let Some(message) = next_message(&mut third_socket, Duration::from_millis(500)).await {
        assert_ne!(
            message["type"],
            json!("containerCategoriesChanged"),
            "{message}"
        );
    }
    let filed = category_boards(&th.client, ROOT_WORKSPACE_ID)
        .await
        .into_iter()
        .find(|entry| entry.category.id == created.id)
        .expect("the category");
    assert_eq!(
        filed.container_metadata,
        [CategoryContainerMetadata {
            container_id: board.id.clone(),
            hidden: true,
        }],
        "the sidebar has it where it was"
    );

    th.teardown().await;
}
