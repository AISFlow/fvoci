//! Integration tests for the WebSocket endpoint over a real connection: authentication,
//! subscriptions, event fan-out, and keepalive pings. Unit tests of the listener table
//! live next to the hub in `ws::tests`.

use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;

use futures_util::{SinkExt as _, StreamExt as _};
use fvoci_model::{
    Block, BlockType, Category, Container, ContainerMember, ContainerPatch, ContainerVisibility,
    new_id,
};
use fvoci_server::app::Former;
use fvoci_server::permissions::ContainerScope;
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use crate::common::ROOT_WS;
use crate::common::{
    Client, HIDDEN_WORKSPACE_ID, ROOT_WORKSPACE_ID, Setup, TEST_WORKSPACE_ID, TestServer,
};

/// How long to wait for a message that should arrive.
pub(crate) const PATIENCE: Duration = Duration::from_secs(5);
/// How long to wait before deciding that a message does not arrive.
pub(crate) const QUIET: Duration = Duration::from_millis(300);

pub(crate) struct Socket {
    stream: WebSocketStream<MaybeTlsStream<TcpStream>>,
}

/// What a socket yields next.
#[derive(Debug, PartialEq)]
pub(crate) enum Event {
    Json(Value),
    Ping,
    Closed,
    Nothing,
}

impl Socket {
    /// A connection without a session, as a page opens one that nobody is signed in
    /// on.
    pub(crate) async fn connect(th: &TestServer) -> Self {
        let stream = th.anonymous().as_browser().connect_websocket().await;
        Self { stream }
    }

    /// A connection without a session from `address`, as the proxy in front names it
    /// (`X-Forwarded-For`): for a server that trusts 127.0.0.1 as its proxy.
    pub(crate) async fn connect_from(th: &TestServer, address: &str) -> Self {
        let headers = [
            ("Origin", "http://localhost:8000".to_owned()),
            ("X-Forwarded-For", address.to_owned()),
        ];
        let stream = th
            .anonymous()
            .try_connect_websocket(&headers)
            .await
            .expect("the websocket endpoint accepts the connection");
        Self { stream }
    }

    /// A connection of the client's session: whose it is is settled by the handshake.
    pub(crate) async fn connect_as(client: &Client) -> Self {
        let stream = client.connect_websocket().await;
        Self { stream }
    }

    pub(crate) async fn send(&mut self, command: Value) {
        self.stream
            .send(Message::Text(command.to_string().into()))
            .await
            .expect("the command is sent");
    }

    pub(crate) async fn next(&mut self, wait: Duration) -> Event {
        loop {
            match tokio::time::timeout(wait, self.stream.next()).await {
                Err(_) => return Event::Nothing,
                Ok(None | Some(Err(_) | Ok(Message::Close(_)))) => return Event::Closed,
                Ok(Some(Ok(Message::Text(text)))) => {
                    return Event::Json(serde_json::from_str(&text).expect("messages are JSON"));
                }
                Ok(Some(Ok(Message::Ping(_)))) => return Event::Ping,
                Ok(Some(Ok(_))) => {}
            }
        }
    }

    /// The next message by which the server says it stopped what the connection
    /// followed (`workspaceUnsubscribed`, `blocksUnsubscribed`), skipping pings and
    /// other messages; `None` if the connection closes, or nothing such comes within
    /// `wait`.
    pub(crate) async fn unsubscribed_message(&mut self, wait: Duration) -> Option<Value> {
        loop {
            match self.next(wait).await {
                Event::Json(message) => {
                    if matches!(
                        message["type"].as_str(),
                        Some("workspaceUnsubscribed" | "blocksUnsubscribed")
                    ) {
                        return Some(message);
                    }
                }
                Event::Ping => {}
                Event::Closed | Event::Nothing => return None,
            }
        }
    }

    /// The `type` of [`Self::unsubscribed_message`].
    pub(crate) async fn unsubscribed(&mut self, wait: Duration) -> Option<String> {
        let message = self.unsubscribed_message(wait).await?;
        message["type"].as_str().map(str::to_owned)
    }

    /// The next JSON message, skipping pings.
    pub(crate) async fn next_json(&mut self, wait: Duration) -> Option<Value> {
        loop {
            match self.next(wait).await {
                Event::Json(value) => return Some(value),
                Event::Ping => {}
                Event::Closed | Event::Nothing => return None,
            }
        }
    }
}

pub(crate) fn category(workspace_id: &str, user_id: &str, name: &str) -> Category {
    Category {
        id: new_id(),
        name: name.to_owned(),
        user_id: user_id.to_owned(),
        workspace_id: workspace_id.to_owned(),
        ..Category::default()
    }
}

/// Subscribes an authenticated socket to a workspace and waits for its `workspaceSubscribed`
/// acknowledgement.
pub(crate) async fn subscribe_to_workspace(
    _th: &TestServer,
    socket: &mut Socket,
    workspace_id: &str,
    _user_id: &str,
) {
    socket
        .send(json!({"type": "subscribeWorkspace", "workspaceId": workspace_id}))
        .await;
    while let Some(message) = socket.next_json(PATIENCE).await {
        if message["type"] == json!("workspaceSubscribed")
            && message["workspaceId"] == json!(workspace_id)
        {
            return;
        }
    }
    panic!("the socket was never subscribed to workspace {workspace_id}");
}

/// The category messages go to every connection of the user that follows the workspace:
/// another tab of the user hears them too, another user does not.
#[tokio::test]
async fn category_messages_reach_every_tab_of_the_user() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;
    let mut first = Socket::connect_as(&th.client).await;
    subscribe_to_workspace(&th, &mut first, ROOT_WORKSPACE_ID, &user1.id).await;
    let mut second = Socket::connect_as(&th.client).await;
    subscribe_to_workspace(&th, &mut second, ROOT_WORKSPACE_ID, &user1.id).await;
    while first.next_json(Duration::from_millis(150)).await.is_some() {}
    let user2 = th.client2.me().await;
    let mut other = Socket::connect_as(&th.client2).await;
    subscribe_to_workspace(&th, &mut other, ROOT_WORKSPACE_ID, &user2.id).await;

    th.client
        .post(
            &format!("{ROOT_WS}/categories"),
            &json!({"name": "both tabs", "userId": user1.id, "workspaceId": ROOT_WORKSPACE_ID}),
        )
        .await
        .check_created();
    for socket in [&mut first, &mut second] {
        let message = socket.next_json(PATIENCE).await.expect("told");
        assert_eq!(message["type"], json!("categoryChanged"), "{message}");
        assert_eq!(message["category"]["name"], json!("both tabs"));
    }
    assert_eq!(other.next(QUIET).await, Event::Nothing, "another user");
    th.teardown().await;
}

/// A connection follows the workspaces its user is a member of, a guest too: what is
/// told of a board goes to its members only, so a guest hears the boards they were
/// invited to. The subscription to a workspace the user is not in is ignored.
#[tokio::test]
async fn a_workspace_is_followed_by_its_members_and_guests() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;
    let user2 = th.client2.me().await;
    th.app
        .store()
        .execute_unchecked(&format!(
            "UPDATE workspace_members SET role = 'guest' \
             WHERE user_id = '{}' AND workspace_id = '{ROOT_WORKSPACE_ID}'",
            user2.id
        ))
        .await
        .expect("the second user becomes a guest of the root workspace");

    // The subscription that works comes last on each connection: once it is seen to
    // work, the server has gone through the one before it.
    let mut outsider = Socket::connect_as(&th.client).await;
    outsider
        .send(json!({"type": "subscribeWorkspace", "workspaceId": HIDDEN_WORKSPACE_ID}))
        .await;
    subscribe_to_workspace(&th, &mut outsider, ROOT_WORKSPACE_ID, &user1.id).await;
    let mut guest = Socket::connect_as(&th.client2).await;
    guest
        .send(json!({"type": "subscribeWorkspace", "workspaceId": ROOT_WORKSPACE_ID}))
        .await;
    subscribe_to_workspace(&th, &mut guest, TEST_WORKSPACE_ID, &user2.id).await;

    th.app
        .ws()
        .broadcast_category_change(&category(HIDDEN_WORKSPACE_ID, &user1.id, "hidden"))
        .await;
    th.app
        .ws()
        .broadcast_category_change(&category(ROOT_WORKSPACE_ID, &user2.id, "a guest's"))
        .await;
    assert_eq!(
        outsider.next(QUIET).await,
        Event::Nothing,
        "not in the workspace"
    );
    let told = guest.next(QUIET).await;
    let Event::Json(told) = told else {
        panic!("a guest of the workspace is told its own category: {told:?}");
    };
    assert_eq!(told["category"]["name"], json!("a guest's"), "{told}");
    th.teardown().await;
}

/// A connection of a session subscribes to a workspace and is told about the user's category
/// changes; on a connection without a session the subscription is ignored.
#[tokio::test]
async fn workspace_subscription_needs_authentication() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;

    // No session: the subscription is ignored, and so is an unknown action.
    let mut anonymous = Socket::connect(&th).await;
    anonymous
        .send(json!({"type": "subscribeWorkspace", "workspaceId": ROOT_WORKSPACE_ID}))
        .await;
    anonymous.send(json!({"type": "NO_SUCH_ACTION"})).await;
    anonymous.send(json!("not a command")).await;

    let mut socket = Socket::connect_as(&th.client).await;
    subscribe_to_workspace(&th, &mut socket, ROOT_WORKSPACE_ID, &user1.id).await;

    let change = category(ROOT_WORKSPACE_ID, &user1.id, "Projects");
    th.app.ws().broadcast_category_change(&change).await;
    let message = socket
        .next_json(PATIENCE)
        .await
        .expect("the change arrives");
    assert_eq!(message["type"], json!("categoryChanged"), "{message}");
    assert_eq!(
        message["workspaceId"],
        json!(ROOT_WORKSPACE_ID),
        "{message}"
    );
    assert_eq!(message["category"]["name"], json!("Projects"), "{message}");
    assert_eq!(message["category"]["userId"], json!(user1.id), "{message}");
    assert!(message.get("containerCategories").is_none(), "{message}");

    assert_eq!(
        anonymous.next(QUIET).await,
        Event::Nothing,
        "the anonymous socket hears nothing"
    );

    // After unsubscribing, nothing arrives any more. The second subscription to another
    // workspace shows that the server has processed the commands before it.
    socket
        .send(json!({"type": "unsubscribeWorkspace", "workspaceId": ROOT_WORKSPACE_ID}))
        .await;
    subscribe_to_workspace(&th, &mut socket, TEST_WORKSPACE_ID, &user1.id).await;
    th.app.ws().broadcast_category_change(&change).await;
    assert_eq!(
        socket.next(QUIET).await,
        Event::Nothing,
        "unsubscribed from the workspace"
    );

    th.teardown().await;
}

/// There is no message to sign in with: a connection is whose its handshake says, and
/// stays that. `AUTH`, which used to carry the token, is an action like any unknown one.
#[tokio::test]
async fn a_message_does_not_sign_a_connection_in() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;
    let mut socket = Socket::connect(&th).await;

    socket
        .send(json!({"type": "AUTH", "token": th.client.token}))
        .await;
    socket
        .send(json!({"type": "subscribeWorkspace", "workspaceId": ROOT_WORKSPACE_ID}))
        .await;
    // The server reads the two messages in its own time, and a change that is sent
    // before it has read them would be missed by any connection. So the change is sent
    // again and again, and a connection of the user, which subscribes after the other
    // one sent its messages, shows that the changes do arrive where they belong.
    let mut signed_in = Socket::connect_as(&th.client).await;
    subscribe_to_workspace(&th, &mut signed_in, ROOT_WORKSPACE_ID, &user1.id).await;
    let change = category(ROOT_WORKSPACE_ID, &user1.id, "Projects");
    for round in 0..5 {
        th.app.ws().broadcast_category_change(&change).await;
        let heard = signed_in.next_json(Duration::from_secs(5)).await;
        assert_eq!(
            heard.expect("the user's connection hears the change")["type"],
            json!("categoryChanged"),
            "round {round}"
        );
        assert_eq!(
            socket.next(Duration::from_millis(50)).await,
            Event::Nothing,
            "round {round}: the connection is nobody's: it hears nothing, and it is not \
             closed either"
        );
    }
    assert_eq!(socket.next(QUIET).await, Event::Nothing);
    th.teardown().await;
}

/// Board and block changes go to the workspace's subscribers who are members of the board.
#[tokio::test]
async fn board_changes_go_to_subscribed_members() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;
    let user2 = th.client2.me().await;

    let mut board = Container {
        id: new_id(),
        workspace_id: ROOT_WORKSPACE_ID.to_owned(),
        visibility: ContainerVisibility::Private,
        title: "Roadmap".to_owned(),
        ..Container::default()
    };
    th.app
        .store()
        .insert_container_with_admin(&mut board, &user1.id)
        .await
        .expect("the board is stored with user1 as admin");

    let mut member = Socket::connect_as(&th.client).await;
    subscribe_to_workspace(&th, &mut member, ROOT_WORKSPACE_ID, &user1.id).await;
    let mut outsider = Socket::connect_as(&th.client2).await;
    subscribe_to_workspace(&th, &mut outsider, ROOT_WORKSPACE_ID, &user2.id).await;

    th.app
        .ws()
        .broadcast_container_change(ROOT_WORKSPACE_ID, &board)
        .await;
    let message = member
        .next_json(PATIENCE)
        .await
        .expect("the member is told");
    assert_eq!(message["type"], json!("containerChanged"), "{message}");
    assert_eq!(
        message["workspaceId"],
        json!(ROOT_WORKSPACE_ID),
        "{message}"
    );
    assert_eq!(message["container"]["id"], json!(board.id), "{message}");
    assert_eq!(message["container"]["title"], json!("Roadmap"), "{message}");

    let mut block = Block {
        id: new_id(),
        container_id: board.id.clone(),
        block_type: BlockType::Card,
        title: "A card".to_owned(),
        ..Block::default()
    };
    th.app
        .store()
        .insert_block(&mut block, &user1.id)
        .await
        .expect("the block is stored");
    th.app
        .ws()
        .broadcast_block_change(ROOT_WORKSPACE_ID, &block)
        .await;
    let message = member
        .next_json(PATIENCE)
        .await
        .expect("the member is told");
    assert_eq!(message["type"], json!("blockChanged"), "{message}");
    assert_eq!(message["block"]["id"], json!(block.id), "{message}");
    assert_eq!(message["block"]["title"], json!("A card"), "{message}");

    for (gone_id, gone_type, expected_type) in [
        (block.id.as_str(), block.block_type, "card"),
        ("view-1", BlockType::View, "view"),
    ] {
        th.app
            .ws()
            .broadcast_block_delete(ROOT_WORKSPACE_ID, gone_id, &board.id, gone_type)
            .await;
        let message = member.next_json(PATIENCE).await.expect("member told");
        assert_eq!(
            (
                &message["type"],
                &message["block"]["id"],
                &message["block"]["type"]
            ),
            (
                &json!("blockChanged"),
                &json!(gone_id),
                &json!(expected_type)
            ),
            "{message}"
        );
        assert!(
            message["block"]["deletedAt"].as_i64() > Some(0),
            "{message}"
        );
    }

    // Removing a member tells the members and the removed user.
    th.app
        .ws()
        .broadcast_member_delete(ROOT_WORKSPACE_ID, &board.id, &user2.id)
        .await;
    let message = member.next_json(PATIENCE).await.expect("member told");
    assert_eq!(message["type"], "containerMemberRemoved", "{message}");
    assert_eq!(message["member"]["userId"], json!(user2.id), "{message}");
    let message = outsider.next_json(PATIENCE).await.expect("removed told");
    assert_eq!(message["type"], "containerMemberRemoved", "{message}");

    // Everything before that passed the user who is no member by.
    assert_eq!(
        outsider.next(QUIET).await,
        Event::Nothing,
        "nothing else for the outsider"
    );

    th.teardown().await;
}

/// Anonymous readers of a shared board subscribe to its blocks with the read token.
#[tokio::test]
async fn block_subscription_needs_the_read_token() {
    let th = Setup::local_mode().start().await.init_basic().await;
    let user1 = th.client.me().await;

    let mut board = Container {
        id: new_id(),
        workspace_id: ROOT_WORKSPACE_ID.to_owned(),
        visibility: ContainerVisibility::Private,
        ..Container::default()
    };
    th.app
        .store()
        .insert_container_with_admin(&mut board, &user1.id)
        .await
        .expect("the board is stored");
    let mut block = Block {
        id: new_id(),
        container_id: board.id.clone(),
        block_type: BlockType::Card,
        ..Block::default()
    };
    th.app
        .store()
        .insert_block(&mut block, &user1.id)
        .await
        .expect("the block is stored");
    let token = th.share(&board.id).await;

    let subscribe = |read_token: &str| {
        json!({
            "type": "subscribeBlocks",
            "workspaceId": ROOT_WORKSPACE_ID,
            "readToken": read_token,
            "blockIds": [block.id],
        })
    };

    let mut wrong = Socket::connect(&th).await;
    wrong.send(subscribe("not-the-read-token")).await;
    let mut reader = Socket::connect(&th).await;
    reader.send(subscribe(&token)).await;

    let ack = reader
        .next_json(PATIENCE)
        .await
        .expect("blocksSubscribed ACK");
    assert_eq!(ack["type"], json!("blocksSubscribed"), "{ack}");
    th.app
        .ws()
        .broadcast_block_change(ROOT_WORKSPACE_ID, &block)
        .await;
    let message = reader
        .next_json(PATIENCE)
        .await
        .expect("the reader is told about the block");
    assert_eq!(message["type"], json!("blockChanged"), "{message}");
    assert_eq!(message["block"]["id"], json!(block.id), "{message}");

    assert_eq!(
        wrong.next(QUIET).await,
        Event::Nothing,
        "a wrong read token subscribes to nothing"
    );

    th.teardown().await;
}

/// The server pings every connection at the configured interval so intermediate proxies
/// do not close idle sockets.
#[tokio::test]
async fn the_server_pings_at_the_configured_interval() {
    let th = Setup::native()
        .configure(|config| {
            config.websocket_ping_seconds = 1;
            config.skip_websocket_pings = false;
        })
        .start()
        .await;
    let mut socket = Socket::connect(&th).await;

    // The first tick of the interval is immediate; the second comes a second later.
    assert_eq!(
        socket.next(PATIENCE).await,
        Event::Ping,
        "a ping on connect"
    );
    let started = std::time::Instant::now();
    assert_eq!(socket.next(PATIENCE).await, Event::Ping, "and another one");
    let waited = started.elapsed();
    assert!(
        waited >= Duration::from_millis(500) && waited < Duration::from_secs(3),
        "about a second apart, was {waited:?}"
    );

    th.teardown().await;
}

/// Without a ping interval configured, nothing is sent to an idle connection.
#[tokio::test]
async fn no_pings_when_turned_off() {
    let th = Setup::native().start().await;
    let mut socket = Socket::connect(&th).await;

    assert_eq!(
        socket.next(QUIET).await,
        Event::Nothing,
        "an idle connection stays quiet"
    );
    th.teardown().await;
}

/// Commands whose fields have the wrong JSON type fail typed deserialization, are
/// dropped, and leave the WebSocket connection open.
#[tokio::test]
async fn commands_with_members_of_the_wrong_type_are_ignored() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;
    let mut socket = Socket::connect_as(&th.client).await;

    let malformed = [
        json!({"type": "subscribeWorkspace", "workspaceId": 5}),
        json!({"type": "subscribeBlocks", "workspaceId": ROOT_WORKSPACE_ID, "blockIds": "not-a-list"}),
        json!({"type": "subscribeBlocks", "workspaceId": ROOT_WORKSPACE_ID, "blockIds": [1, 2, 3]}),
        json!({"type": "subscribeBlocks", "workspaceId": ROOT_WORKSPACE_ID, "readToken": {"a": 1}, "blockIds": []}),
        json!({"type": 7}),
        json!([1, 2, 3]),
        json!(null),
    ];
    for command in malformed {
        socket.send(command).await;
    }

    // The same connection still subscribes and is told about changes.
    subscribe_to_workspace(&th, &mut socket, ROOT_WORKSPACE_ID, &user1.id).await;
    let change = category(ROOT_WORKSPACE_ID, &user1.id, "After the noise");
    th.app.ws().broadcast_category_change(&change).await;
    let message = socket
        .next_json(PATIENCE)
        .await
        .expect("the change arrives");
    assert_eq!(
        message["category"]["name"],
        json!("After the noise"),
        "{message}"
    );

    th.teardown().await;
}

/// A password that an administrator sets signs the user out everywhere, live websocket
/// connections included: a connection checks its token only once, at its handshake.
/// Connections of other users stay.
#[tokio::test]
async fn resetting_a_password_closes_the_websocket_connections_of_that_user() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;
    let user2 = th.client2.me().await;
    let mut socket1 = Socket::connect_as(&th.client).await;
    subscribe_to_workspace(&th, &mut socket1, ROOT_WORKSPACE_ID, &user1.id).await;
    let mut socket2 = Socket::connect_as(&th.client2).await;
    subscribe_to_workspace(&th, &mut socket2, ROOT_WORKSPACE_ID, &user2.id).await;

    th.app
        .reset_user_password(&user1.username, "another-Pa$$word")
        .await
        .expect("the password is set");

    assert_eq!(
        socket1.next(PATIENCE).await,
        Event::Closed,
        "the connection of the user whose password was set"
    );
    th.app
        .ws()
        .broadcast_category_change(&category(ROOT_WORKSPACE_ID, &user2.id, "still listening"))
        .await;
    let message = socket2
        .next_json(PATIENCE)
        .await
        .expect("the other user's connection still gets messages");
    assert_eq!(message["type"], json!("categoryChanged"), "{message}");
    th.teardown().await;
}

/// A board, a card on it, and the read token the board is shared under.
pub(crate) async fn shared_board_with_a_card(
    th: &TestServer,
    user_id: &str,
) -> (Container, Block, String) {
    let mut board = Container {
        id: new_id(),
        workspace_id: ROOT_WORKSPACE_ID.to_owned(),
        visibility: ContainerVisibility::Private,
        ..Container::default()
    };
    th.app
        .store()
        .insert_container_with_admin(&mut board, user_id)
        .await
        .expect("the board is stored");
    let mut block = Block {
        id: new_id(),
        container_id: board.id.clone(),
        block_type: BlockType::Card,
        ..Block::default()
    };
    th.app
        .store()
        .insert_block(&mut block, user_id)
        .await
        .expect("the block is stored");
    let token = th.share(&board.id).await;
    (board, block, token)
}

/// Subscribes a connection without a session to a block with a read token and waits
/// until a change of the block comes through; returns that message.
async fn follow_block(th: &TestServer, block: &Block, read_token: &str) -> (Socket, Value) {
    follow_block_on(th, Socket::connect(th).await, block, read_token).await
}

/// [`follow_block`] on the given connection.
pub(crate) async fn follow_block_on(
    th: &TestServer,
    mut reader: Socket,
    block: &Block,
    read_token: &str,
) -> (Socket, Value) {
    reader
        .send(json!({
            "type": "subscribeBlocks",
            "workspaceId": ROOT_WORKSPACE_ID,
            "readToken": read_token,
            "blockIds": [block.id],
        }))
        .await;
    let ack = reader
        .next_json(PATIENCE)
        .await
        .expect("blocksSubscribed ACK");
    assert_eq!(ack["type"], json!("blocksSubscribed"), "{ack}");
    th.app
        .ws()
        .broadcast_block_change(ROOT_WORKSPACE_ID, block)
        .await;
    if let Some(message) = reader.next_json(PATIENCE).await {
        while reader.next_json(Duration::from_millis(150)).await.is_some() {}
        return (reader, message);
    }
    panic!("the reader was never told about the block");
}

/// Why a connection ended is counted in the metrics: here a message over the limit
/// broke a connection off. A read token that stopped opening its board ends only what
/// followed it, and the message that says so is counted among those queued.
#[tokio::test]
async fn why_a_connection_ended_is_counted() {
    use crate::metrics::{sample, scrape_until};

    let th = Setup::local_mode()
        .metrics_at("127.0.0.1:0")
        .start()
        .await
        .init_basic()
        .await;
    let user1 = th.client.me().await;
    let (board, block, token) = shared_board_with_a_card(&th, &user1.id).await;
    let (mut reader, _) = follow_block(&th, &block, &token).await;
    th.app.ws().end_readers_of_container(&board.id);
    assert_eq!(
        reader.unsubscribed(PATIENCE).await.as_deref(),
        Some("blocksUnsubscribed"),
        "the share ended, and the reader is told"
    );

    let mut talker = Socket::connect(&th).await;
    // The server may close before the whole message is written.
    let _ = talker
        .stream
        .send(Message::Text("x".repeat(64 * 1024 + 1).into()))
        .await;
    assert_eq!(talker.next(PATIENCE).await, Event::Closed, "over the limit");

    let ended = |body: &str, reason: &str| {
        sample(
            body,
            "fvoci_websocket_disconnections_total",
            &[("reason", reason)],
        )
    };
    let told = |body: &str| {
        sample(
            body,
            "fvoci_websocket_messages_queued_total",
            &[("type", "blocksUnsubscribed")],
        )
    };
    let body = scrape_until(&th, |body| {
        told(body) == Some(1.0) && ended(body, "read_error") == Some(1.0)
    })
    .await;
    assert_eq!(told(&body), Some(1.0), "the ended share is told: {body}");
    assert_eq!(ended(&body, "read_error"), Some(1.0), "{body}");
    assert_eq!(
        ended(&body, "slow_consumer"),
        Some(0.0),
        "there from the start: {body}"
    );
    th.teardown().await;
}

/// A board's member and a reader without a session follow the same board by its read
/// token: the member's connection, which sees the board, is told a comment on the card
/// as a member is; the reader by the link alone is not told who wrote it or whom its
/// body mentions. (A card's own message carries no body.)
#[tokio::test]
async fn a_member_following_by_read_token_is_told_as_a_member() {
    let th = Setup::local_mode().start().await.init_basic().await;
    let user1 = th.client.me().await;
    let (_board, block, token) = shared_board_with_a_card(&th, &user1.id).await;
    let comment = Block {
        id: new_id(),
        parent_id: block.id.clone(),
        container_id: block.container_id.clone(),
        block_type: BlockType::Comment,
        created_by: user1.id.clone(),
        body: fvoci_model::Body::parse(json!({"type": "doc", "content": [
            {"type": "paragraph", "content": [{"type": "mention", "attrs": {"userId": user1.id}}]},
        ]}))
        .expect("a body"),
        ..Block::default()
    };
    let mention = "/block/body/content/0/content/0/attrs/userId";
    let (mut member, _) =
        follow_block_on(&th, Socket::connect_as(&th.client).await, &block, &token).await;
    let (mut reader, _) = follow_block(&th, &block, &token).await;
    th.app
        .ws()
        .broadcast_block_change(ROOT_WORKSPACE_ID, &comment)
        .await;
    // What the reader's following broadcast of the card may still be on its way.
    let of_the_comment = async |socket: &mut Socket| loop {
        let told = socket
            .next_json(PATIENCE)
            .await
            .expect("the comment is told");
        if told["block"]["id"] == comment.id {
            break told;
        }
    };
    let told = of_the_comment(&mut member).await;
    assert_eq!(told["block"]["createdBy"], json!(user1.id), "{told}");
    assert_eq!(told.pointer(mention), Some(&json!(user1.id)), "{told}");
    let told = of_the_comment(&mut reader).await;
    assert_eq!(told["block"]["createdBy"], json!(""), "{told}");
    assert_eq!(told.pointer(mention), Some(&json!("")), "{told}");
    th.teardown().await;
}

/// Whoever follows a board by its read token alone is not told who made or changed a
/// block, over the websocket as over HTTP, nor whom the card's body mentions: the
/// message carries the summary of the body, which names nobody. A member is told who.
#[tokio::test]
async fn a_reader_by_read_token_is_not_told_who_wrote_a_block() {
    let th = Setup::local_mode().start().await.init_basic().await;
    let user1 = th.client.me().await;
    let (_board, mut block, token) = shared_board_with_a_card(&th, &user1.id).await;
    assert_eq!(block.created_by, user1.id, "the block has an author");
    block.body = fvoci_model::Body::parse(json!({"type": "doc", "content": [
        {"type": "paragraph", "content": [{"type": "mention", "attrs": {"userId": user1.id}}]},
    ]}))
    .expect("a body");
    let names_nobody = |message: &Value| {
        assert_eq!(message["block"]["body"], Value::Null, "{message}");
        assert_eq!(message["summary"]["hasBody"], json!(true), "{message}");
        assert!(
            !message.to_string().contains(&user1.id),
            "nor who is mentioned: {message}"
        );
    };

    let (mut reader, message) = follow_block(&th, &block, &token).await;
    assert_eq!(message["block"]["id"], json!(block.id), "{message}");
    assert_eq!(message["block"]["createdBy"], json!(""), "{message}");
    assert_eq!(message["block"]["modifiedBy"], json!(""), "{message}");
    names_nobody(&message);

    let mut member = Socket::connect_as(&th.client).await;
    subscribe_to_workspace(&th, &mut member, ROOT_WORKSPACE_ID, &user1.id).await;
    th.app
        .ws()
        .broadcast_block_change(ROOT_WORKSPACE_ID, &block)
        .await;
    let message = member
        .next_json(PATIENCE)
        .await
        .expect("the member is told");
    assert_eq!(message["block"]["createdBy"], json!(user1.id), "{message}");
    assert_eq!(message["block"]["body"], Value::Null, "{message}");
    // The same broadcast, as the reader has it: what the member is told does not reach
    // the reader, nor the reverse.
    let message = reader
        .next_json(PATIENCE)
        .await
        .expect("the reader is told too");
    assert_eq!(message["block"]["createdBy"], json!(""), "{message}");
    names_nobody(&message);

    // A signed-in user who is no member of the board follows it as the token's reader:
    // nor are they told who the values of the board's person properties name.
    let definitions = json!([
        {"id": "owner", "name": "Owner", "type": "person"},
        {"id": "workspace", "name": "Workspace", "type": "multiPerson"},
    ]);
    let patch: ContainerPatch =
        serde_json::from_value(json!({"updatedCardProperties": definitions}))
            .expect("a board patch");
    th.app
        .store()
        .patch_container(&block.container_id, &patch, &user1.id)
        .await
        .expect("the board is stored");
    block.fields.insert(
        "properties".to_owned(),
        json!({"owner": user1.id, "workspace": [user1.id, user1.id]}),
    );
    let stranger = Socket::connect_as(&th.client2).await;
    let (_stranger, message) = follow_block_on(&th, stranger, &block, &token).await;
    assert_eq!(message["block"]["createdBy"], json!(""), "{message}");
    names_nobody(&message);
    assert_eq!(
        message["block"]["fields"]["properties"],
        json!({"owner": "", "workspace": ["", ""]}),
        "{message}"
    );

    // The board's member, by the same link, is told the people as a member is.
    let member = Socket::connect_as(&th.client).await;
    let (_member, message) = follow_block_on(&th, member, &block, &token).await;
    assert_eq!(
        message["block"]["fields"]["properties"],
        json!({"owner": user1.id, "workspace": [user1.id, user1.id]}),
        "{message}"
    );

    th.teardown().await;
}

/// A read token that stops opening its board stops feeding connections: turning
/// sharing off, sharing under a new token and deleting the board each end what the
/// connections followed of the board with the old link, and tell them.
#[tokio::test]
async fn a_read_token_that_no_longer_opens_its_board_no_longer_feeds_a_connection() {
    let th = Setup::local_mode().start().await.init_basic().await;
    let user1 = th.client.me().await;
    let sharing = |board_id: &str| format!("{ROOT_WS}/containers/{board_id}/public-link");

    // PublicLink turned off.
    let (board, block, token) = shared_board_with_a_card(&th, &user1.id).await;
    let (mut reader, _) = follow_block(&th, &block, &token).await;
    // PublicLink what is shared changes nothing for who follows it.
    th.client
        .post(&sharing(&board.id), &json!({"enabled": true}))
        .await
        .check_ok();
    assert_eq!(reader.next(QUIET).await, Event::Nothing, "still connected");
    th.client
        .post(&sharing(&board.id), &json!({"enabled": false}))
        .await
        .check_ok();
    assert_eq!(
        reader.unsubscribed(PATIENCE).await.as_deref(),
        Some("blocksUnsubscribed"),
        "sharing off"
    );

    // A new token: the old link is dead, the new one is followed.
    let (board, block, token) = shared_board_with_a_card(&th, &user1.id).await;
    let (mut reader, _) = follow_block(&th, &block, &token).await;
    let renewed = th
        .client
        .post(
            &sharing(&board.id),
            &json!({"enabled": true, "regenerate": true}),
        )
        .await
        .value()["token"]
        .as_str()
        .expect("the new token is answered")
        .to_owned();
    assert_eq!(
        reader.unsubscribed(PATIENCE).await.as_deref(),
        Some("blocksUnsubscribed"),
        "a new token"
    );
    let (mut with_new_link, _) = follow_block(&th, &block, &renewed).await;

    // The board deleted: its sharing went with it.
    th.client
        .delete(&format!("{ROOT_WS}/containers/{}", board.id))
        .await
        .check_no_content();
    assert_eq!(
        with_new_link.unsubscribed(PATIENCE).await.as_deref(),
        Some("blocksUnsubscribed"),
        "the board is gone"
    );

    // The board deleted with others at once, as `deleteContainersAndBlocks` does.
    let (board, block, token) = shared_board_with_a_card(&th, &user1.id).await;
    let (mut reader, _) = follow_block(&th, &block, &token).await;
    th.client
        .delete_json(
            &format!("{ROOT_WS}/containers-and-blocks"),
            &json!({"containers": [board.id], "blocks": [block.id]}),
        )
        .await
        .check_no_content();
    assert_eq!(
        reader.unsubscribed(PATIENCE).await.as_deref(),
        Some("blocksUnsubscribed"),
        "the board is gone with others"
    );

    th.teardown().await;
}

/// A session that ends by its idle limit, which nobody announces, takes its websocket
/// connection with it at the next ping (the hourly clean-up of ended sessions is not
/// waited for); a session in use keeps its connection.
#[tokio::test]
async fn a_session_that_runs_out_closes_its_websocket_connection() {
    let th = Setup::native()
        .configure(|config| {
            config.websocket_ping_seconds = 1;
            config.skip_websocket_pings = false;
            config.session_idle_days = 3;
        })
        .start()
        .await
        .init_basic()
        .await;
    let user1 = th.client.me().await;
    let elsewhere = th
        .login(crate::common::USER1_USERNAME, crate::common::PASSWORD)
        .await;
    let mut here = Socket::connect_as(&th.client).await;
    subscribe_to_workspace(&th, &mut here, ROOT_WORKSPACE_ID, &user1.id).await;
    let mut there = Socket::connect_as(&elsewhere).await;
    let session = th
        .app
        .store()
        .get_session(
            &fvoci_server::auth::secret::token_digest(&th.client.token),
            0,
            i64::MIN,
        )
        .await
        .expect("the session is stored");
    let three_days = 3 * 24 * 60 * 60 * 1000;
    th.app
        .store()
        .set_session_last_used(
            &session.id,
            fvoci_server::auth::now_millis() - three_days - 60_000,
        )
        .await
        .expect("the last use is moved back");

    let until = std::time::Instant::now() + PATIENCE;
    let mut ended = Event::Nothing;
    while std::time::Instant::now() < until {
        ended = here.next(PATIENCE).await;
        if ended != Event::Ping {
            break;
        }
    }
    assert_eq!(ended, Event::Closed, "the session ran out");
    let until = std::time::Instant::now() + Duration::from_millis(2500);
    while std::time::Instant::now() < until {
        assert_eq!(
            there.next(PATIENCE).await,
            Event::Ping,
            "the user's other session stays connected"
        );
    }
    th.teardown().await;
}

/// A session that ends takes its websocket connections with it: logging out closes the
/// connection of that session and leaves the user's other session connected.
#[tokio::test]
async fn logging_out_closes_the_websocket_connections_of_that_session() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;
    let elsewhere = th
        .login(crate::common::USER1_USERNAME, crate::common::PASSWORD)
        .await;
    let mut here = Socket::connect_as(&th.client).await;
    subscribe_to_workspace(&th, &mut here, ROOT_WORKSPACE_ID, &user1.id).await;
    let mut there = Socket::connect_as(&elsewhere).await;

    th.client.post_empty("/logout").await.check_no_content();

    assert_eq!(
        here.next(PATIENCE).await,
        Event::Closed,
        "the session ended"
    );
    assert_eq!(
        there.next(QUIET).await,
        Event::Nothing,
        "the user's other session stays connected"
    );
    th.teardown().await;
}

/// Checks a value against a member's description in `contracts/websocket.json`: a JSON
/// type, a schema of `contracts/openapi.json` by name, an inline object, or a list.
fn check_against(openapi: &Value, what: &Value, value: &Value, at: &str) {
    match what {
        Value::String(reference) if let Some(name) = reference.strip_prefix('#') => {
            let schema = &openapi["components"]["schemas"][name];
            if let Some(variants) = schema["enum"].as_array() {
                assert!(
                    variants.contains(value),
                    "{at}: {value} is among {variants:?} of {name}"
                );
            } else {
                let properties = schema["properties"]
                    .as_object()
                    .unwrap_or_else(|| panic!("{at}: openapi.json has the schema {name}"));
                let object = value
                    .as_object()
                    .unwrap_or_else(|| panic!("{at}: an object, not {value}"));
                for key in object.keys() {
                    assert!(
                        properties.contains_key(key),
                        "{at}: `{key}` is no member of {name} in openapi.json: {value}"
                    );
                }
                for key in schema["required"].as_array().into_iter().flatten() {
                    let key = key.as_str().expect("required names are strings");
                    assert!(
                        object.contains_key(key),
                        "{at}: {name} has `{key}`: {value}"
                    );
                }
            }
        }
        Value::String(kind) => {
            let matches = match kind.as_str() {
                "string" => value.is_string(),
                "boolean" => value.is_boolean(),
                "integer" => value.is_i64() || value.is_u64(),
                other => panic!("{at}: unknown type {other} in websocket.json"),
            };
            assert!(matches, "{at}: a {kind}, not {value}");
        }
        Value::Array(items) => {
            let [item] = items.as_slice() else {
                panic!("{at}: a list is described by one item");
            };
            let list = value
                .as_array()
                .unwrap_or_else(|| panic!("{at}: a list, not {value}"));
            for (index, element) in list.iter().enumerate() {
                check_against(openapi, item, element, &format!("{at}[{index}]"));
            }
        }
        Value::Object(members) => {
            let object = value
                .as_object()
                .unwrap_or_else(|| panic!("{at}: an object, not {value}"));
            let expected: BTreeSet<&String> = members.keys().collect();
            let actual: BTreeSet<&String> = object.keys().collect();
            assert_eq!(actual, expected, "{at}: the members of {value}");
            for (key, member) in members {
                check_against(openapi, member, &object[key], &format!("{at}.{key}"));
            }
        }
        other => panic!("{at}: cannot read {other} in websocket.json"),
    }
}

/// Checks one message the server sent against `contracts/websocket.json`.
pub(crate) fn check_message(contract: &Value, openapi: &Value, message: &Value) {
    let kind = message["type"].as_str().expect("a message has a type");
    let entry = &contract["messages"][kind];
    let mut members = entry["members"]
        .as_object()
        .unwrap_or_else(|| panic!("contracts/websocket.json lists {kind}"))
        .clone();
    // A member the entry names optional is checked where the message has it.
    let optional: Vec<&str> = entry["optional"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|name| {
            name.as_str()
                .expect("optional members are named by strings")
        })
        .collect();
    members.retain(|key, _| !optional.contains(&key.as_str()) || message.get(key).is_some());
    check_against(openapi, &Value::Object(members), message, kind);
}

pub(crate) fn read_contract(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../contracts")
        .join(name);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// Checks the messages a socket hears against the websocket contract, and remembers
/// which kinds it heard.
struct ContractCheck<'contract> {
    contract: &'contract Value,
    openapi: &'contract Value,
    heard: BTreeSet<String>,
}

impl ContractCheck<'_> {
    /// Waits for the next message, which must be of `kind`, and checks it.
    async fn hear(&mut self, socket: &mut Socket, kind: &str) {
        let message = socket.next_json(PATIENCE).await.expect("the message comes");
        assert_eq!(message["type"], json!(kind), "{message}");
        check_message(self.contract, self.openapi, &message);
        self.heard.insert(kind.to_owned());
    }
}

/// A private board of the user's with one card, stored.
async fn board_with_a_card(th: &TestServer, user_id: &str) -> (Container, Block) {
    let mut board = Container {
        id: new_id(),
        workspace_id: ROOT_WORKSPACE_ID.to_owned(),
        visibility: ContainerVisibility::Private,
        title: "Roadmap".to_owned(),
        ..Container::default()
    };
    th.app
        .store()
        .insert_container_with_admin(&mut board, user_id)
        .await
        .expect("the board is stored");
    let mut block = Block {
        id: new_id(),
        container_id: board.id.clone(),
        block_type: BlockType::Card,
        title: "A card".to_owned(),
        ..Block::default()
    };
    th.app
        .store()
        .insert_block(&mut block, user_id)
        .await
        .expect("the block is stored");
    (board, block)
}

/// Who read a board for the invited only, now gone: its `members`.
fn of_the_invited(members: &[ContainerMember]) -> Former<'_> {
    Former {
        members,
        scope: ContainerScope::InvitedOnly,
    }
}

/// Every message the server sends has exactly the members `contracts/websocket.json`
/// lists for it, and what it carries has the members of its schema in
/// `contracts/openapi.json`. The messages come from the hub's broadcasts over a real
/// connection, with stored boards, blocks and categories.
#[tokio::test]
async fn messages_match_the_websocket_contract() {
    let contract = read_contract("websocket.json");
    let openapi = read_contract("openapi.json");
    let th = Setup::local_mode().start().await.init_basic().await;
    let user1 = th.client.me().await;

    let (board, block) = board_with_a_card(&th, &user1.id).await;
    let members = th
        .app
        .store()
        .get_members_for_container(&board.id)
        .await
        .expect("the members are read");

    let mut socket = Socket::connect_as(&th.client).await;
    subscribe_to_workspace(&th, &mut socket, ROOT_WORKSPACE_ID, &user1.id).await;

    let ws = th.app.ws();
    let category = category(ROOT_WORKSPACE_ID, &user1.id, "Work");
    let mut contract_check = ContractCheck {
        contract: &contract,
        openapi: &openapi,
        heard: BTreeSet::new(),
    };

    ws.broadcast_container_change(ROOT_WORKSPACE_ID, &board)
        .await;
    contract_check.hear(&mut socket, "containerChanged").await;
    ws.broadcast_block_change(ROOT_WORKSPACE_ID, &block).await;
    contract_check.hear(&mut socket, "blockChanged").await;
    ws.broadcast_block_delete(ROOT_WORKSPACE_ID, &block.id, &board.id, block.block_type)
        .await;
    contract_check.hear(&mut socket, "blockChanged").await;
    let admin = members
        .first()
        .expect("the board has its admin as a member");
    ws.broadcast_member_change(ROOT_WORKSPACE_ID, &board.id, admin)
        .await;
    contract_check
        .hear(&mut socket, "containerMemberChanged")
        .await;
    ws.broadcast_member_delete(ROOT_WORKSPACE_ID, &board.id, &user1.id)
        .await;
    contract_check
        .hear(&mut socket, "containerMemberRemoved")
        .await;
    ws.broadcast_category_change(&category).await;
    contract_check.hear(&mut socket, "categoryChanged").await;
    ws.broadcast_category_container_change(
        ROOT_WORKSPACE_ID,
        &user1.id,
        &[fvoci_model::ContainerCategoryWebsocketData {
            container_id: board.id.clone(),
            category_id: category.id.clone(),
            hidden: false,
        }],
    )
    .await;
    contract_check
        .hear(&mut socket, "containerCategoriesChanged")
        .await;
    ws.broadcast_category_reorder(
        ROOT_WORKSPACE_ID,
        &user1.id,
        std::slice::from_ref(&category.id),
    )
    .await;
    contract_check
        .hear(&mut socket, "categoriesReordered")
        .await;
    ws.broadcast_category_containers_reorder(
        ROOT_WORKSPACE_ID,
        &user1.id,
        &category.id,
        std::slice::from_ref(&board.id),
    )
    .await;
    contract_check
        .hear(&mut socket, "categoryContainersReordered")
        .await;
    hear_the_account_changes(&th, &mut socket, &mut contract_check, &user1.id).await;
    // The board deletion goes to the former members, with the board's identifiers only.
    ws.broadcast_container_delete(ROOT_WORKSPACE_ID, &board.id, of_the_invited(&members))
        .await;
    contract_check.hear(&mut socket, "containerChanged").await;
    hear_the_endings(&th, &mut socket, &mut contract_check, &block, &user1.id).await;
    hear_the_answers(&mut socket, &mut contract_check, &board.id).await;

    // Every message of the contract was heard; those of the bodies edited together
    // are heard by `rooms_contract`.
    let listed: BTreeSet<&String> = contract["messages"]
        .as_object()
        .expect("the messages are an object")
        .keys()
        .filter(|kind| !kind.starts_with("doc"))
        .collect();
    let heard: BTreeSet<&String> = contract_check.heard.iter().collect();
    assert_eq!(
        heard, listed,
        "the test hears every message the contract lists"
    );

    th.teardown().await;
}

/// Hears the configuration, notification and person broadcasts on `socket`.
async fn hear_the_account_changes(
    th: &TestServer,
    socket: &mut Socket,
    contract_check: &mut ContractCheck<'_>,
    user_id: &str,
) {
    let ws = th.app.ws();
    ws.broadcast_config_change(&th.app.client_config()).await;
    contract_check.hear(socket, "clientConfigChanged").await;
    let org_id = th
        .app
        .store()
        .tenant()
        .org_id()
        .expect("the test server's organization");
    ws.notifications_changed(user_id, &[&org_id]).await;
    contract_check.hear(socket, "notificationsChanged").await;
    ws.broadcast_person_change(ROOT_WORKSPACE_ID, user_id).await;
    contract_check.hear(socket, "userChanged").await;
}

/// Hears what the server answers the connection itself: its presence on the board
/// `board_id`, and a Ping.
async fn hear_the_answers(
    socket: &mut Socket,
    contract_check: &mut ContractCheck<'_>,
    board_id: &str,
) {
    socket
        .send(
            json!({"type": "setPresence", "workspaceId": ROOT_WORKSPACE_ID,
            "containerId": board_id}),
        )
        .await;
    contract_check.hear(socket, "presenceChanged").await;
    socket.send(json!({"type": "ping"})).await;
    contract_check.hear(socket, "pong").await;
}

/// What ends of a shared board, and of a workspace, ends what followed it, and is told.
async fn hear_the_endings(
    th: &TestServer,
    socket: &mut Socket,
    contract_check: &mut ContractCheck<'_>,
    block: &Block,
    user_id: &str,
) {
    let token = th.share(&block.container_id).await;
    socket
        .send(
            json!({"type": "subscribeBlocks", "workspaceId": ROOT_WORKSPACE_ID,
            "blockIds": [block.id], "readToken": token}),
        )
        .await;
    contract_check.hear(socket, "blocksSubscribed").await;
    th.app.ws().end_readers_of_container(&block.container_id);
    contract_check.hear(socket, "blocksUnsubscribed").await;
    th.app.ws().workspace_role_changed_to(
        ROOT_WORKSPACE_ID,
        user_id,
        fvoci_model::WorkspaceRole::Member,
    );
    contract_check.hear(socket, "workspaceRoleChanged").await;
    contract_check.hear(socket, "workspaceUnsubscribed").await;
    socket
        .send(json!({"type": "subscribeWorkspace", "workspaceId": ROOT_WORKSPACE_ID}))
        .await;
    contract_check.hear(socket, "workspaceSubscribed").await;
}

/// A private board of the root workspace with the text properties `a`, `b` and `c`, in that
/// order, and `admin` as its admin.
async fn board_with_three_properties(th: &TestServer, admin: &str) -> Container {
    let property = |id: &str| {
        fvoci_model::CardPropertyDefinition::new(id, id, fvoci_model::PropertyType::Text)
    };
    let mut board = Container {
        id: new_id(),
        workspace_id: ROOT_WORKSPACE_ID.to_owned(),
        visibility: ContainerVisibility::Private,
        card_properties: vec![property("a"), property("b"), property("c")],
        ..Container::default()
    };
    th.app
        .store()
        .insert_container_with_admin(&mut board, admin)
        .await
        .expect("the board is stored with the admin");
    board
}

/// The card properties of a board are put in a new order by naming all of them: the
/// answer is the order, the board has it, and the listeners hear `containerChanged`. An
/// order that leaves one out,
/// names one twice or names one the board does not have is 400 and changes nothing; a
/// commenter may not, nor a stranger.
#[tokio::test]
async fn card_properties_are_put_in_order_and_the_board_is_told() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;
    let user2 = th.client2.me().await;
    let editor_client = th.add_user("editor").await;
    let editor = editor_client.me().await;
    let board = board_with_three_properties(&th, &user1.id).await;
    for (user, role) in [(&user2.id, "commenter"), (&editor.id, "editor")] {
        th.client
            .add_member_in(
                ROOT_WORKSPACE_ID,
                &fvoci_model::ContainerMember {
                    container_id: board.id.clone(),
                    user_id: user.clone(),
                    role: role.parse().expect("a valid role"),
                    synthetic: false,
                    inherited_role: None,
                },
            )
            .await
            .check_ok();
    }
    let path = format!("{ROOT_WS}/boards/{}/property-order", board.id);
    let ids = |board: &Value| -> Vec<Value> {
        board["cardProperties"]
            .as_array()
            .expect("properties")
            .iter()
            .map(|property| property["id"].clone())
            .collect()
    };
    let mut socket = Socket::connect_as(&th.client).await;
    subscribe_to_workspace(&th, &mut socket, ROOT_WORKSPACE_ID, &user1.id).await;

    let answered: Vec<String> = th.client.put(&path, &["c", "a", "b"]).await.ok_json();
    assert_eq!(answered, ["c", "a", "b"]);
    let message = socket
        .next_json(PATIENCE)
        .await
        .expect("the board's listeners are told");
    assert_eq!(message["type"], json!("containerChanged"), "{message}");
    assert_eq!(
        ids(&message["container"]),
        [json!("c"), json!("a"), json!("b")]
    );
    let stored = th
        .client
        .get(&format!("{ROOT_WS}/containers/{}", board.id))
        .await
        .value();
    assert_eq!(ids(&stored), [json!("c"), json!("a"), json!("b")]);
    assert_eq!(stored["modifiedBy"], json!(user1.id), "{stored}");

    // An editor may, as with patchContainer; the board is marked as changed by them.
    let answered: Vec<String> = editor_client.put(&path, &["b", "c", "a"]).await.ok_json();
    assert_eq!(answered, ["b", "c", "a"]);
    let by_editor = th
        .client
        .get(&format!("{ROOT_WS}/containers/{}", board.id))
        .await
        .value();
    assert_eq!(by_editor["modifiedBy"], json!(editor.id), "{by_editor}");
    assert!(
        by_editor["updatedAt"].as_i64() >= stored["updatedAt"].as_i64(),
        "{by_editor}"
    );
    socket.next_json(PATIENCE).await.expect("told again");

    for order in [
        json!(["c", "a"]),
        json!(["c", "a", "b", "b"]),
        json!(["c", "a", "b", "d"]),
        json!(["c", "a", "d"]),
        json!({"order": ["a"]}),
    ] {
        th.client.put(&path, &order).await.check_bad_request();
    }
    th.client2
        .put(&path, &["a", "b", "c"])
        .await
        .check_status(reqwest::StatusCode::FORBIDDEN);
    let stranger = th.add_user("stranger").await;
    stranger
        .put(&path, &["a", "b", "c"])
        .await
        .check_status(reqwest::StatusCode::NOT_FOUND);
    let stored = th
        .client
        .get(&format!("{ROOT_WS}/containers/{}", board.id))
        .await
        .value();
    assert_eq!(
        ids(&stored),
        [json!("b"), json!("c"), json!("a")],
        "nothing refused changed it"
    );
}
