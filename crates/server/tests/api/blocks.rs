//! The blocks of a board in the tests: the fixtures that make, read and delete them by
//! the routes of their types ([`crate::block_insert`]), the websocket the tests listen
//! with, and the tests of the application layer's block writes, through the real store.
//!
//! Each test sets up its own isolated server instance and creates prerequisite boards
//! through the store via [`create_board_in_store`].

use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use futures_util::{SinkExt as _, StreamExt as _};
use fvoci_model::{
    BLOCK_TITLE_MAX_CHARS, Block, BlockPatch, BlockPatchBatch, BlockType, Card, Category,
    Container, ContainerMember, ContainerVisibility, JsonMap, new_id,
};
use fvoci_server::app::ChangeNotification;
use fvoci_server::error::ErrorKind;
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use crate::common::TEST_WS;
use crate::common::{ApiResponse, Client, Setup, TEST_WORKSPACE_ID, TestServer, UNKNOWN_ID};

mod told;

/// How long to wait for a websocket message that should arrive.
const PATIENCE: Duration = Duration::from_secs(5);
/// How long to wait before deciding that a websocket message does not arrive.
pub(crate) const QUIET: Duration = Duration::from_millis(300);
/// How long background notifications may take to finish.
const BACKGROUND: Duration = Duration::from_secs(10);

// -- helpers --------------------------------------------------------------------------

/// What a user may do on a board they are added to in a test.
#[derive(Debug, Clone, Copy)]
enum Role {
    Editor,
    Commenter,
    Viewer,
}

/// Stores `board` directly with the first client's user as its administrator.
async fn store_board(th: &TestServer, mut board: Container) -> Container {
    let admin = th.client.me().await;
    board.id = new_id();
    th.app
        .store()
        .insert_container_with_admin(&mut board, &admin.id)
        .await
        .expect("the board is stored with the first user as admin");
    board
}

/// Creates an empty board in [`TEST_WORKSPACE_ID`] through the store, with the first client's
/// user as its administrator.
pub(crate) async fn create_board_in_store(
    th: &TestServer,
    board_type: ContainerVisibility,
) -> Container {
    let board = Container {
        workspace_id: TEST_WORKSPACE_ID.to_owned(),
        visibility: board_type,
        ..Container::default()
    };
    store_board(th, board).await
}

/// Makes the client's user a member of the board, through the store.
async fn add_member_in_store(th: &TestServer, board_id: &str, client: &Client, role: Role) {
    let member = ContainerMember {
        container_id: board_id.to_owned(),
        user_id: client.me().await.id,
        role: match role {
            Role::Editor => fvoci_model::ContainerRole::Editor,
            Role::Commenter => fvoci_model::ContainerRole::Commenter,
            Role::Viewer => fvoci_model::ContainerRole::Viewer,
        },
        synthetic: false,
        inherited_role: None,
    };
    th.app
        .store()
        .save_member(&member)
        .await
        .expect("the membership is stored");
}

/// A private board of the first user (`th.client`, its administrator) with one more
/// member of each role, and a second private board only the first user is on.
/// `th.client2` is a member of neither.
pub(crate) struct Boards {
    pub(crate) board: Container,
    pub(crate) other_board: Container,
    pub(crate) editor: Client,
    pub(crate) commenter: Client,
    pub(crate) viewer: Client,
}

pub(crate) async fn boards_with_roles(th: &TestServer) -> Boards {
    let board = create_board_in_store(th, ContainerVisibility::Private).await;
    let other_board = create_board_in_store(th, ContainerVisibility::Private).await;
    let editor = th.add_user("editor").await;
    let commenter = th.add_user("commenter").await;
    let viewer = th.add_user("viewer").await;
    add_member_in_store(th, &board.id, &editor, Role::Editor).await;
    add_member_in_store(th, &board.id, &commenter, Role::Commenter).await;
    add_member_in_store(th, &board.id, &viewer, Role::Viewer).await;
    Boards {
        board,
        other_board,
        editor,
        commenter,
        viewer,
    }
}

/// A card of the first user with a text and a comment of the commenter on it, titled
/// by their types, and a card titled `other` on the other board.
pub(crate) struct Content {
    pub(crate) card: Block,
    pub(crate) text: Block,
    pub(crate) comment: Block,
    pub(crate) other_card: Block,
}

pub(crate) async fn card_with_content(th: &TestServer, boards: &Boards) -> Content {
    let board_id = &boards.board.id;
    let card = insert_one(&th.client, &new_block(board_id, BlockType::Card, "card")).await;
    let text = insert_one(&th.client, &child_of(&card, BlockType::Attachment, "text")).await;
    let comment = insert_one(
        &boards.commenter,
        &child_of(&card, BlockType::Comment, "comment"),
    )
    .await;
    let other_card = insert_one(
        &th.client,
        &new_block(&boards.other_board.id, BlockType::Card, "other"),
    )
    .await;
    Content {
        card,
        text,
        comment,
        other_card,
    }
}

/// Builds a test block with a freshly generated identifier and both timestamps set to 1.
pub(crate) fn new_block(board_id: &str, block_type: BlockType, title: &str) -> Block {
    Block {
        id: new_id(),
        container_id: board_id.to_owned(),
        created_at: 1,
        updated_at: 1,
        block_type,
        title: title.to_owned(),
        ..Block::default()
    }
}

pub(crate) fn child_of(parent: &Block, block_type: BlockType, title: &str) -> Block {
    Block {
        parent_id: parent.id.clone(),
        ..new_block(&parent.container_id, block_type, title)
    }
}

/// What a board answers that is not there, or that the user cannot see.
pub(crate) const NO_BOARD: &str = "board not found";

/// What a block answers that is not there, or that is on another board than the path's.
pub(crate) const NO_BLOCK: &str = "block not found";

pub(crate) use crate::block_insert::insert_in;

/// Makes `blocks` on a board of the test workspace by the routes of their types
/// ([`insert_in`]), expecting success.
pub(crate) async fn insert(client: &Client, board_id: &str, blocks: &[Block]) -> Vec<Block> {
    insert_in(client, TEST_WORKSPACE_ID, board_id, blocks).await
}

/// Inserts one block and returns it as the server answered.
pub(crate) async fn insert_one(client: &Client, block: &Block) -> Block {
    insert(client, &block.container_id, std::slice::from_ref(block))
        .await
        .pop()
        .expect("one block was inserted")
}

/// Every block of a board of the test workspace as `client` reads them by the routes of
/// their types ([`blocks_of_in`]).
pub(crate) async fn blocks_of(client: &Client, board_id: &str) -> Vec<Block> {
    blocks_of_in(client, TEST_WORKSPACE_ID, board_id).await
}

/// Every live block of `board_id` of `workspace_id` as `client` reads them by the routes
/// of their types: its views, then card by card the comments and attachments of the card
/// and the card itself (templates too), each card as `getCard` answers it.
pub(crate) async fn blocks_of_in(
    client: &Client,
    workspace_id: &str,
    board_id: &str,
) -> Vec<Block> {
    let board = format!("/w/{workspace_id}/boards/{board_id}");
    let cards: Vec<Card> = client
        .get(&format!("{board}/cards?perPage=1000"))
        .await
        .ok_json();
    let mut blocks: Vec<Block> = client.get(&format!("{board}/views")).await.ok_json();
    for card in cards {
        for below in ["comments", "attachments"] {
            let found: Vec<Block> = client
                .get(&format!("{board}/cards/{}/{below}", card.id))
                .await
                .ok_json();
            blocks.extend(found);
        }
        let card: Card = client
            .get(&format!("/w/{workspace_id}/cards/{}", card.id))
            .await
            .ok_json();
        blocks.push(Block::from(card));
    }
    blocks
}

/// Every live block of `board_id` as the store holds them, for a test that reads what a
/// write left rather than what a reader is answered.
pub(crate) async fn stored_blocks(th: &TestServer, board_id: &str) -> Vec<Block> {
    th.app
        .blocks(board_id, None, None)
        .await
        .expect("the blocks of the board are read")
}

/// Deletes `block` of the test workspace by the route of its type, expecting success.
pub(crate) async fn delete(client: &Client, block: &Block) {
    crate::block_insert::delete_in(client, TEST_WORKSPACE_ID, block).await;
}

/// Makes the card `card_id` a template in the store, as an older server's `patchBlock`
/// did: no route makes a card a template now.
pub(crate) async fn make_template_unchecked(th: &TestServer, card_id: &str) {
    let store = th.app.store();
    let mut fields = store
        .get_block(card_id)
        .await
        .expect("the card is stored")
        .fields;
    fields.insert("isTemplate".to_owned(), serde_json::Value::Bool(true));
    store
        .set_block_fields_unchecked(card_id, &fields)
        .await
        .expect("written as an older server would have");
}

/// Takes the card `card_id` of `board_id` of the test workspace to the trash, alone in
/// a `batchDeleteCards`.
pub(crate) async fn trash_card(client: &Client, board_id: &str, card_id: &str) -> ApiResponse {
    client
        .post(
            &format!("{TEST_WS}/boards/{board_id}/cards/batch-delete"),
            &json!({ "cardIds": [card_id] }),
        )
        .await
}

/// Brings the card `card_id` of `board_id` of the test workspace back from the trash,
/// alone in a `batchRestoreCards`.
pub(crate) async fn restore_card(client: &Client, board_id: &str, card_id: &str) -> ApiResponse {
    client
        .post(
            &format!("{TEST_WS}/boards/{board_id}/cards/batch-restore"),
            &json!({ "cardIds": [card_id] }),
        )
        .await
}

pub(crate) fn ids(blocks: &[Block]) -> Vec<&str> {
    blocks.iter().map(|block| block.id.as_str()).collect()
}

/// The titles of the blocks, sorted, because lists of blocks have no defined order.
pub(crate) fn titles(blocks: &[Block]) -> Vec<&str> {
    let mut titles: Vec<&str> = blocks.iter().map(|block| block.title.as_str()).collect();
    titles.sort_unstable();
    titles
}

pub(crate) fn find<'a>(blocks: &'a [Block], block_id: &str) -> &'a Block {
    blocks
        .iter()
        .find(|block| block.id == block_id)
        .unwrap_or_else(|| panic!("block {block_id} is among {:?}", ids(blocks)))
}

pub(crate) fn title_patch(title: &str) -> BlockPatch {
    BlockPatch {
        title: Some(title.to_owned()),
        ..BlockPatch::default()
    }
}

/// A batch that gives each block the title next to it.
pub(crate) fn batch(patches: &[(&Block, &str)]) -> BlockPatchBatch {
    BlockPatchBatch {
        block_ids: patches.iter().map(|(block, _)| block.id.clone()).collect(),
        block_patches: patches
            .iter()
            .map(|(_, title)| title_patch(title))
            .collect(),
    }
}

pub(crate) fn fields(value: Value) -> JsonMap {
    serde_json::from_value(value).expect("the fields are a JSON object")
}

/// A client connected to the websocket endpoint.
pub(crate) struct Socket {
    stream: WebSocketStream<MaybeTlsStream<TcpStream>>,
}

impl Socket {
    /// Connects, authenticates as the client's user and subscribes to `workspace_id`;
    /// returns once the server has acted on the subscription. Commands have no
    /// acknowledgement, so the wait is for a broadcast to the user to come through.
    pub(crate) async fn subscribed_to_workspace(
        th: &TestServer,
        client: &Client,
        workspace_id: &str,
    ) -> Self {
        let stream = client.connect_websocket().await;
        let mut socket = Self { stream };
        socket
            .send(json!({"type": "subscribeWorkspace", "workspaceId": workspace_id}))
            .await;

        let probe = Category {
            id: new_id(),
            name: "subscription probe".to_owned(),
            user_id: client.me().await.id,
            workspace_id: workspace_id.to_owned(),
            ..Category::default()
        };
        for _ in 0..50 {
            th.app.ws().broadcast_category_change(&probe).await;
            if socket.next_json(Duration::from_millis(100)).await.is_some() {
                // Later probes may still be queued; let them drain.
                while socket.next_json(QUIET).await.is_some() {}
                return socket;
            }
        }
        panic!("the socket was never subscribed to workspace {workspace_id}");
    }

    async fn send(&mut self, command: Value) {
        self.stream
            .send(Message::Text(command.to_string().into()))
            .await
            .expect("the command is sent");
    }

    /// The next JSON message, or `None` if none arrives within `wait`.
    pub(crate) async fn next_json(&mut self, wait: Duration) -> Option<Value> {
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

    /// The block of the next message, which must be a block change in the test workspace.
    pub(crate) async fn next_block(&mut self) -> Value {
        let message = self
            .next_json(PATIENCE)
            .await
            .expect("a block change is announced");
        assert_eq!(message["type"], json!("blockChanged"), "{message}");
        assert_eq!(
            message["workspaceId"],
            json!(TEST_WORKSPACE_ID),
            "{message}"
        );
        message["block"].clone()
    }

    /// Checks that nothing more is announced once the server's background work is done.
    async fn expect_quiet(&mut self, th: &TestServer, why: &str) {
        assert!(
            th.app.finish_background(BACKGROUND).await,
            "background work finishes"
        );
        let message = self.next_json(QUIET).await;
        assert_eq!(message, None, "{why}");
    }
}

// -- Application-layer block tests, through the real store --------------------------------

/// Inserting a single block via the application layer succeeds.
#[tokio::test]
async fn app_insert_block_success_scenario() {
    let th = Setup::one_user().start().await;
    let board = create_board_in_store(&th, ContainerVisibility::Open).await;
    let mut block = new_block(&board.id, BlockType::Card, "A card");

    th.app
        .insert_block_unchecked(&mut block, &th.stranger().await, ChangeNotification::Send)
        .await
        .expect("the block is inserted");

    let stored = th.app.block(&block.id).await.expect("the block is stored");
    assert_eq!(stored.title, "A card", "as given");
    assert_eq!(
        stored.created_by,
        th.stranger().await,
        "created by the modifier"
    );
    assert_eq!(
        stored.modified_by,
        th.stranger().await,
        "and modified by them"
    );

    th.teardown().await;
}

/// Application-layer `insert_block` fails when the block is rejected or the board does not exist.
#[tokio::test]
async fn app_insert_block_error_scenario() {
    let th = Setup::one_user().start().await;
    let board = create_board_in_store(&th, ContainerVisibility::Open).await;

    let mut too_long = new_block(
        &board.id,
        BlockType::Card,
        &"a".repeat(BLOCK_TITLE_MAX_CHARS + 1),
    );
    let error = th
        .app
        .insert_block_unchecked(
            &mut too_long,
            &th.stranger().await,
            ChangeNotification::Send,
        )
        .await
        .expect_err("the title is too long");
    assert_eq!(error.kind(), ErrorKind::BadRequest, "{error:?}");
    assert!(
        th.app.block(&too_long.id).await.is_err(),
        "nothing was stored"
    );

    let mut homeless = new_block("no-such-board", BlockType::Card, "");
    let error = th
        .app
        .insert_block_unchecked(
            &mut homeless,
            &th.stranger().await,
            ChangeNotification::Send,
        )
        .await
        .expect_err("the board does not exist");
    assert!(error.is_not_found(), "{error:?}");

    th.teardown().await;
}

/// Application-layer `patch_blocks` applies a batch of patches.
#[tokio::test]
async fn app_patch_blocks_success_scenario() {
    let th = Setup::one_user().start().await;
    let board = create_board_in_store(&th, ContainerVisibility::Open).await;
    let block = insert_one(&th.client, &new_block(&board.id, BlockType::Card, "old")).await;

    th.app
        .patch_blocks_unchecked(
            TEST_WORKSPACE_ID,
            &batch(&[(&block, "new title")]),
            &th.stranger().await,
            ChangeNotification::Send,
        )
        .await
        .expect("the batch is applied");

    let stored = th.app.block(&block.id).await.expect("the block exists");
    assert_eq!(stored.title, "new title", "patched");
    assert_eq!(stored.modified_by, th.stranger().await, "by the modifier");

    th.teardown().await;
}

/// Application-layer `patch_blocks` fails atomically for an unknown identifier, patching nothing.
#[tokio::test]
async fn app_patch_blocks_error_scenario() {
    let th = Setup::one_user().start().await;
    let board = create_board_in_store(&th, ContainerVisibility::Open).await;
    let block = insert_one(&th.client, &new_block(&board.id, BlockType::Card, "old")).await;
    let missing = new_block(&board.id, BlockType::Card, "never stored");

    let error = th
        .app
        .patch_blocks_unchecked(
            TEST_WORKSPACE_ID,
            &batch(&[(&block, "new title"), (&missing, "new title")]),
            &th.stranger().await,
            ChangeNotification::Send,
        )
        .await
        .expect_err("one block does not exist");
    assert!(error.is_not_found(), "{error:?}");

    let stored = th.app.block(&block.id).await.expect("the block exists");
    assert_eq!(stored.title, "old", "nothing was patched");

    th.teardown().await;
}

/// Application-layer `delete_block` deletes an existing block.
#[tokio::test]
async fn app_delete_block_success_scenario() {
    let th = Setup::one_user().start().await;
    let board = create_board_in_store(&th, ContainerVisibility::Open).await;
    let block = insert_one(&th.client, &new_block(&board.id, BlockType::Card, "")).await;

    th.app
        .delete_block_unchecked(&block.id, &th.stranger().await, ChangeNotification::Send)
        .await
        .expect("the block is deleted");

    let error = th
        .app
        .block(&block.id)
        .await
        .expect_err("the block is gone");
    assert!(error.is_not_found(), "{error:?}");
    let deleted = th
        .app
        .deleted_block(&block.id)
        .await
        .expect("the trash is read")
        .expect("the block is in the trash");
    assert!(
        deleted.block.deleted_at.is_some_and(|at| at > 0),
        "the trash records the deletion"
    );
    assert_eq!(deleted.deleted_by, th.stranger().await, "and who deleted");

    th.teardown().await;
}

/// Application-layer `delete_block` fails when the block does not exist.
#[tokio::test]
async fn app_delete_block_error_scenario() {
    let th = Setup::one_user().start().await;

    let error = th
        .app
        .delete_block_unchecked(
            "no-such-block",
            &th.stranger().await,
            ChangeNotification::Send,
        )
        .await
        .expect_err("there is no such block");
    assert!(error.is_not_found(), "{error:?}");
    assert_eq!(error.to_string(), "not found", "no identifier in it");

    th.teardown().await;
}

/// Application-layer `undelete_block` restores a deleted block.
#[tokio::test]
async fn app_undelete_block_success_scenario() {
    let th = Setup::one_user().start().await;
    let board = create_board_in_store(&th, ContainerVisibility::Open).await;
    let block = insert_one(&th.client, &new_block(&board.id, BlockType::Card, "back")).await;
    th.app
        .delete_block_unchecked(&block.id, &th.stranger().await, ChangeNotification::Send)
        .await
        .expect("the block is deleted");

    let undeleted = th
        .app
        .undelete_block_unchecked(&block.id, &th.stranger().await)
        .await
        .expect("the block is undeleted");
    assert_eq!(undeleted.id, block.id, "the restored block is returned");
    assert_eq!(undeleted.title, "back", "with its content");

    let stored = th.app.block(&block.id).await.expect("the block is stored");
    assert_eq!(stored.deleted_at, None, "and not deleted");

    th.teardown().await;
}

/// What is not in the trash is not there to restore: a block that never existed, by a
/// well-formed identifier or by text that is none, and a block that exists.
#[tokio::test]
async fn app_undelete_block_that_is_not_in_the_trash() {
    let th = Setup::one_user().start().await;
    let board = create_board_in_store(&th, ContainerVisibility::Open).await;
    let block = insert_one(&th.client, &new_block(&board.id, BlockType::Card, "here")).await;

    for block_id in [UNKNOWN_ID, "no-such-block", block.id.as_str()] {
        let error = th
            .app
            .undelete_block_unchecked(block_id, &th.stranger().await)
            .await
            .expect_err("nothing to restore");
        assert!(error.is_not_found(), "{block_id}: {error:?}");
    }
    let stored = th.app.block(&block.id).await.expect("the block is stored");
    assert_eq!(stored.modified_by, block.modified_by, "and untouched");

    th.teardown().await;
}

/// Application-layer `insert_blocks` inserts multiple blocks at once.
#[tokio::test]
async fn app_insert_blocks_success_scenario() {
    let th = Setup::one_user().start().await;
    let board = create_board_in_store(&th, ContainerVisibility::Open).await;
    let mut blocks = vec![new_block(&board.id, BlockType::Card, "A card")];

    th.app
        .insert_blocks_unchecked(&mut blocks, &th.stranger().await, ChangeNotification::Send)
        .await
        .expect("the blocks are inserted");

    let stored = th
        .app
        .blocks_for_container(&board.id)
        .await
        .expect("readable");
    assert_eq!(ids(&stored), [blocks[0].id.as_str()], "the block is stored");
    assert_eq!(
        blocks[0].modified_by,
        th.stranger().await,
        "the caller's copy says who stored it"
    );

    th.app
        .insert_blocks_unchecked(&mut [], &th.stranger().await, ChangeNotification::Send)
        .await
        .expect("no blocks are no error");

    th.teardown().await;
}

/// Application-layer `insert_blocks` rejects oversized blocks and blocks spanning multiple
/// boards before storing anything.
#[tokio::test]
async fn app_insert_blocks_error_scenario() {
    let th = Setup::one_user().start().await;
    let board = create_board_in_store(&th, ContainerVisibility::Open).await;
    let other = create_board_in_store(&th, ContainerVisibility::Open).await;

    let mut too_long = vec![new_block(
        &board.id,
        BlockType::Card,
        &"a".repeat(BLOCK_TITLE_MAX_CHARS + 1),
    )];
    let error = th
        .app
        .insert_blocks_unchecked(
            &mut too_long,
            &th.stranger().await,
            ChangeNotification::Send,
        )
        .await
        .expect_err("the title is too long");
    assert_eq!(error.kind(), ErrorKind::BadRequest, "{error:?}");

    let mut mixed = vec![
        new_block(&board.id, BlockType::Card, ""),
        new_block(&other.id, BlockType::Card, ""),
    ];
    let error = th
        .app
        .insert_blocks_unchecked(&mut mixed, &th.stranger().await, ChangeNotification::Send)
        .await
        .expect_err("two boards in one call");
    assert_eq!(error.kind(), ErrorKind::BadRequest, "{error:?}");
    assert_eq!(
        error.to_string(),
        "the blocks belong to more than one board",
        "what is wrong with the request"
    );

    let stored = th
        .app
        .blocks_for_container(&board.id)
        .await
        .expect("readable");
    assert!(stored.is_empty(), "nothing was stored: {:?}", ids(&stored));

    th.teardown().await;
}

/// Where the webhook test's receiver listens. The server's configuration is changed by
/// a plain function, which finds the address here.
static WEBHOOK_URL: OnceLock<String> = OnceLock::new();

/// Starts an HTTP server that keeps the JSON bodies posted to it, and puts its address
/// into [`WEBHOOK_URL`].
async fn start_webhook_receiver() -> (Arc<Mutex<Vec<Value>>>, tokio::task::JoinHandle<()>) {
    let posted: Arc<Mutex<Vec<Value>>> = Arc::default();
    let keep = {
        let posted = Arc::clone(&posted);
        move |axum::Json(block): axum::Json<Value>| async move {
            posted.lock().expect("lock poisoned").push(block);
        }
    };
    let hook = axum::Router::new().route("/hook", axum::routing::post(keep));
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("a local port is free");
    let address = listener.local_addr().expect("the listener has an address");
    WEBHOOK_URL
        .set(format!("http://{address}/hook"))
        .expect("only one test sets the webhook address");
    let serving = tokio::spawn(async move {
        axum::serve(listener, hook)
            .await
            .expect("the webhook receiver serves");
    });
    (posted, serving)
}
