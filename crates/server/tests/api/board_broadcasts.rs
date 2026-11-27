//! What the board routes tell over websocket connections, and the socket the tests listen with.

use std::time::Duration;

use futures_util::{SinkExt as _, StreamExt as _};
use fvoci_model::{Container, ContainerVisibility, ContainersAndBlocks};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use crate::blocks::title_patch;
use crate::boards::{
    board_with_admin, delete_member, duplicate_board, editor, insert_block, member, new_board,
    patch_board, undelete_board, update_member, view_block,
};
use crate::common::TEST_WS;
use crate::common::{Client, Setup, TEST_WORKSPACE_ID, TestServer};

/// How long to wait for a message that should arrive.
const PATIENCE: Duration = Duration::from_secs(5);
/// How long to wait before deciding that no more messages arrive.
const QUIET: Duration = Duration::from_millis(300);

/// A websocket connection of one user, subscribed to one workspace.
pub(crate) struct WorkspaceSocket {
    stream: WebSocketStream<MaybeTlsStream<TcpStream>>,
}

impl WorkspaceSocket {
    /// Connects as the client and subscribes to the workspace, awaiting `workspaceSubscribed`.
    pub(crate) async fn subscribe(
        _th: &TestServer,
        client: &Client,
        workspace_id: &str,
        _user_id: &str,
    ) -> Self {
        let stream = client.connect_websocket().await;
        let mut socket = Self { stream };
        socket
            .send(json!({"type": "subscribeWorkspace", "workspaceId": workspace_id}))
            .await;
        socket
            .expect("workspaceSubscribed", "/workspaceId", workspace_id)
            .await;
        socket
    }

    async fn send(&mut self, command: Value) {
        self.stream
            .send(Message::Text(command.to_string().into()))
            .await
            .expect("the command is sent");
    }

    /// The next JSON message, if one arrives in time.
    async fn next(&mut self, wait: Duration) -> Option<Value> {
        loop {
            match tokio::time::timeout(wait, self.stream.next()).await {
                Ok(Some(Ok(Message::Text(text)))) => {
                    return Some(serde_json::from_str(&text).expect("messages are JSON"));
                }
                Ok(Some(Ok(Message::Ping(_) | Message::Pong(_)))) => {}
                _ => return None,
            }
        }
    }

    /// Reads messages until none arrives for a while and returns them.
    pub(crate) async fn drain(&mut self) -> Vec<Value> {
        let mut messages = Vec::new();
        while let Some(message) = self.next(QUIET).await {
            messages.push(message);
        }
        messages
    }

    /// Waits for a message with the given action whose `path` member equals `expected`,
    /// skipping others, and returns it.
    pub(crate) async fn expect(&mut self, action: &str, path: &str, expected: &str) -> Value {
        let mut skipped = Vec::new();
        while let Some(message) = self.next(PATIENCE).await {
            if message["type"] == json!(action) && message.pointer(path) == Some(&json!(expected)) {
                return message;
            }
            skipped.push(message);
        }
        panic!("no {action} message with {path} = {expected} arrived; skipped: {skipped:?}");
    }
}

/// The messages with the given action among `messages`.
fn with_action<'m>(messages: &'m [Value], action: &str) -> Vec<&'m Value> {
    messages
        .iter()
        .filter(|message| message["type"] == json!(action))
        .collect()
}

/// Creating a board tells its creator about the board and the membership; patching and
/// deleting it notify the members.
#[tokio::test]
async fn board_changes_are_broadcast_to_the_members() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;
    let user2 = th.client2.me().await;
    let mut socket1 =
        WorkspaceSocket::subscribe(&th, &th.client, TEST_WORKSPACE_ID, &user1.id).await;
    let mut socket2 =
        WorkspaceSocket::subscribe(&th, &th.client2, TEST_WORKSPACE_ID, &user2.id).await;

    let board: Container = th
        .client
        .create_board(&new_board(
            TEST_WORKSPACE_ID,
            ContainerVisibility::Private,
            "Roadmap",
        ))
        .await
        .created_json();
    let message = socket1
        .expect("containerChanged", "/container/id", &board.id)
        .await;
    assert_eq!(
        message["workspaceId"],
        json!(TEST_WORKSPACE_ID),
        "{message}"
    );
    assert_eq!(message["container"]["title"], json!("Roadmap"), "{message}");
    let message = socket1
        .expect("containerMemberChanged", "/member/containerId", &board.id)
        .await;
    assert_eq!(message["member"]["userId"], json!(user1.id), "{message}");
    assert_eq!(message["member"]["role"], json!("admin"), "{message}");

    patch_board(&th.client, &board.id, &title_patch("Roadmap 2"))
        .await
        .check_ok();
    let message = socket1
        .expect("containerChanged", "/container/id", &board.id)
        .await;
    assert_eq!(
        message["container"]["title"],
        json!("Roadmap 2"),
        "{message}"
    );

    th.client
        .delete(&format!("{TEST_WS}/containers/{}", board.id))
        .await
        .check_no_content();
    let message = socket1
        .expect("containerChanged", "/container/id", &board.id)
        .await;
    assert!(
        message["container"]["deletedAt"].as_i64() > Some(0),
        "a deletion: {message}"
    );

    undelete_board(&th.client, &board.id).await.check_ok();
    let message = socket1
        .expect("containerChanged", "/container/id", &board.id)
        .await;
    assert_eq!(
        message["container"].get("deletedAt"),
        None,
        "restored: {message}"
    );

    let heard_by_outsider = socket2.drain().await;
    assert_eq!(
        heard_by_outsider,
        Vec::<Value>::new(),
        "a user who is not a member hears nothing"
    );
    th.teardown().await;
}

/// Adding, updating and removing a member notify the board's members, including the
/// removed user.
#[tokio::test]
async fn member_changes_are_broadcast() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;
    let user2 = th.client2.me().await;
    let board = board_with_admin(
        &th,
        new_board(TEST_WORKSPACE_ID, ContainerVisibility::Private, "title"),
        &user1.id,
    )
    .await;
    let mut socket1 =
        WorkspaceSocket::subscribe(&th, &th.client, TEST_WORKSPACE_ID, &user1.id).await;
    let mut socket2 =
        WorkspaceSocket::subscribe(&th, &th.client2, TEST_WORKSPACE_ID, &user2.id).await;

    th.client
        .add_member(&editor(&board.id, &user2.id))
        .await
        .check_ok();
    for socket in [&mut socket1, &mut socket2] {
        let message = socket
            .expect("containerMemberChanged", "/member/userId", &user2.id)
            .await;
        assert_eq!(
            message["workspaceId"],
            json!(TEST_WORKSPACE_ID),
            "{message}"
        );
        assert_eq!(
            message["member"]["containerId"],
            json!(board.id),
            "{message}"
        );
        assert_eq!(message["member"]["role"], json!("editor"), "{message}");
    }

    update_member(
        &th.client,
        &member(&board.id, &user2.id, fvoci_model::ContainerRole::Viewer),
    )
    .await
    .check_ok();
    let message = socket2
        .expect("containerMemberChanged", "/member/userId", &user2.id)
        .await;
    assert_eq!(message["member"]["role"], json!("viewer"), "{message}");

    delete_member(&th.client, &board.id, &user2.id)
        .await
        .check_no_content();
    for socket in [&mut socket1, &mut socket2] {
        let message = socket
            .expect("containerMemberRemoved", "/member/userId", &user2.id)
            .await;
        assert_eq!(
            message["member"]["containerId"],
            json!(board.id),
            "{message}"
        );
    }
    th.teardown().await;
}

/// When a template changes between open and private, non-member workspace users are
/// notified that they gained or lost access.
#[tokio::test]
async fn patch_container_type_of_a_template_is_broadcast_to_the_workspace_users() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;
    let user2 = th.client2.me().await;
    let template = board_with_admin(
        &th,
        Container {
            is_template: true,
            ..new_board(TEST_WORKSPACE_ID, ContainerVisibility::Open, "a template")
        },
        &user1.id,
    )
    .await;
    let mut socket1 =
        WorkspaceSocket::subscribe(&th, &th.client, TEST_WORKSPACE_ID, &user1.id).await;
    let mut socket2 =
        WorkspaceSocket::subscribe(&th, &th.client2, TEST_WORKSPACE_ID, &user2.id).await;

    // Private: every user who is not a member is told that their access ended. The
    // message goes to the members and to that user.
    patch_board(&th.client, &template.id, &json!({"visibility": "private"}))
        .await
        .check_ok();
    for socket in [&mut socket1, &mut socket2] {
        let message = socket
            .expect("containerMemberRemoved", "/member/userId", &user2.id)
            .await;
        assert_eq!(
            message["workspaceId"],
            json!(TEST_WORKSPACE_ID),
            "{message}"
        );
        assert_eq!(
            message["member"]["containerId"],
            json!(template.id),
            "{message}"
        );
    }
    let rest = socket1.drain().await;
    assert_eq!(
        with_action(&rest, "containerMemberRemoved"),
        Vec::<&Value>::new(),
        "nothing about the member themselves: {rest:?}"
    );

    // Open: the members are told about a viewer membership of every other user.
    patch_board(&th.client, &template.id, &json!({"visibility": "open"}))
        .await
        .check_ok();
    let message = socket1
        .expect("containerMemberChanged", "/member/userId", &user2.id)
        .await;
    assert_eq!(
        message["member"],
        json!({
            "containerId": template.id,
            "userId": user2.id,
            "role": "viewer",
            "synthetic": true,
        }),
        "a viewer"
    );
    let rest = socket1.drain().await;
    assert_eq!(
        with_action(&rest, "containerMemberChanged"),
        Vec::<&Value>::new(),
        "nothing about the member themselves: {rest:?}"
    );

    // A board that is not a template: no synthetic member messages are sent; making it
    // private tells unjoined workspace workers that it is gone for them (`deletedAt > 0`),
    // just like an open wiki or course.
    let board = board_with_admin(
        &th,
        new_board(TEST_WORKSPACE_ID, ContainerVisibility::Open, "a board"),
        &user1.id,
    )
    .await;
    socket1.drain().await;
    socket2.drain().await;
    patch_board(&th.client, &board.id, &json!({"visibility": "private"}))
        .await
        .check_ok();
    socket1
        .expect("containerChanged", "/container/id", &board.id)
        .await;
    let rest = socket1.drain().await;
    assert_eq!(
        with_action(&rest, "containerMemberRemoved"),
        Vec::<&Value>::new(),
        "no member messages for a board: {rest:?}"
    );
    let closed_msg = socket2
        .expect("containerChanged", "/container/id", &board.id)
        .await;
    assert!(
        closed_msg["container"]["deletedAt"].as_i64() > Some(0),
        "unjoined workspace worker hears closed board as deleted: {closed_msg}"
    );
    assert_eq!(
        with_action(&socket2.drain().await, "containerMemberRemoved"),
        Vec::<&Value>::new(),
        "no synthetic member messages for a non-template board"
    );

    th.teardown().await;
}

/// Duplicating a board notifies its new owner about the copy, its blocks, and the
/// membership.
#[tokio::test]
async fn duplicate_container_is_broadcast() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;
    let board = board_with_admin(
        &th,
        new_board(TEST_WORKSPACE_ID, ContainerVisibility::Open, "Source"),
        &user1.id,
    )
    .await;
    insert_block(&th, view_block(&board.id), &user1.id).await;
    let mut socket =
        WorkspaceSocket::subscribe(&th, &th.client, TEST_WORKSPACE_ID, &user1.id).await;

    let copy: ContainersAndBlocks = duplicate_board(&th.client, &board.id, "")
        .await
        .created_json();

    let copy_id = &copy.containers[0].id;
    socket
        .expect("containerChanged", "/container/id", copy_id)
        .await;
    socket
        .expect("blockChanged", "/block/id", &copy.blocks[0].id)
        .await;
    let message = socket
        .expect("containerMemberChanged", "/member/containerId", copy_id)
        .await;
    assert_eq!(message["member"]["userId"], json!(user1.id), "{message}");
    th.teardown().await;
}
