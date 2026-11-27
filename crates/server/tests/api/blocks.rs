//! Integration and application-layer tests for block routes: permissions, validation,
//! error messages, batch mutations, undeletion, and websocket change broadcasts.
//!
//! Each test sets up its own isolated server instance and creates prerequisite boards
//! through the store via [`create_board_in_store`].

use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use futures_util::{SinkExt as _, StreamExt as _};
use fvoci_model::{
    BLOCK_FIELDS_MAX_CHARS, BLOCK_TITLE_MAX_CHARS, Block, BlockPatch, BlockPatchBatch, BlockType,
    Category, Container, ContainerMember, ContainerVisibility, JsonMap, new_id,
};
use fvoci_server::app::ChangeNotification;
use fvoci_server::error::ErrorKind;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use crate::common::TEST_WS;
use crate::common::{Client, Setup, TEST_WORKSPACE_ID, TestServer, UNKNOWN_ID};

mod told;
mod writes;

/// How long to wait for a websocket message that should arrive.
const PATIENCE: Duration = Duration::from_secs(5);
/// How long to wait before deciding that a websocket message does not arrive.
pub(crate) const QUIET: Duration = Duration::from_millis(300);
/// How long background notifications may take to finish.
const BACKGROUND: Duration = Duration::from_secs(10);

/// The refusal for a user who may not manage the cards of a board.
pub(crate) const NO_CHANGES: &str = "access denied to make board changes";
/// The refusal for a user who may not comment on a board.
const NO_COMMENTS: &str = "access denied to post card comments";

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

pub(crate) fn blocks_path(board_id: &str) -> String {
    format!("{TEST_WS}/boards/{board_id}/blocks")
}

pub(crate) fn block_path(board_id: &str, block_id: &str) -> String {
    format!("{TEST_WS}/boards/{board_id}/blocks/{block_id}")
}

pub(crate) fn undelete_path(board_id: &str, block_id: &str) -> String {
    format!("{}/restore", block_path(board_id, block_id))
}

fn duplicate_path(board_id: &str, block_id: &str) -> String {
    format!("{}/duplicate", block_path(board_id, block_id))
}

/// What a board answers that is not there, or that the user cannot see.
pub(crate) const NO_BOARD: &str = "board not found";

/// What a block answers that is not there, or that is on another board than the path's.
pub(crate) const NO_BLOCK: &str = "block not found";

pub(crate) use crate::block_insert::insert_in;

/// `InsertBlocks`, expecting success.
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

/// `GetBlocksForBoard`.
pub(crate) async fn blocks_of(client: &Client, board_id: &str) -> Vec<Block> {
    client.get(&blocks_path(board_id)).await.ok_json()
}

/// `PatchBlock`, expecting success.
async fn patch(client: &Client, block: &Block, patch: &BlockPatch) {
    let response = client
        .patch(&block_path(&block.container_id, &block.id), patch)
        .await;
    response.check_no_content();
}

/// `DeleteBlock`, expecting success.
pub(crate) async fn delete(client: &Client, block: &Block) {
    let response = client
        .delete(&block_path(&block.container_id, &block.id))
        .await;
    response.check_no_content();
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

// -- Block HTTP route tests -------------------------------------------------------------

/// Fetching blocks on a board returns the inserted blocks.
#[tokio::test]
async fn get_blocks() {
    let th = Setup::one_user().start().await;
    let board = create_board_in_store(&th, ContainerVisibility::Open).await;

    let new_blocks = [
        new_block(&board.id, BlockType::Card, ""),
        new_block(&board.id, BlockType::Card, ""),
    ];
    let inserted = insert(&th.client, &board.id, &new_blocks).await;
    assert_eq!(inserted.len(), 2, "both blocks are answered");

    let blocks = blocks_of(&th.client, &board.id).await;
    assert_eq!(blocks.len(), 2, "both blocks are stored");
    let block_ids = ids(&blocks);
    assert!(block_ids.contains(&inserted[0].id.as_str()), "the first");
    assert!(block_ids.contains(&inserted[1].id.as_str()), "the second");

    th.teardown().await;
}

/// Creating a single block via `POST /blocks`.
#[tokio::test]
async fn post_block_create_a_single_block() {
    let th = Setup::one_user().start().await;
    let board = create_board_in_store(&th, ContainerVisibility::Open).await;

    let block = new_block(&board.id, BlockType::Card, "New title");
    let inserted = insert(&th.client, &board.id, std::slice::from_ref(&block)).await;
    assert_eq!(inserted.len(), 1, "one block is answered");

    let blocks = blocks_of(&th.client, &board.id).await;
    assert_eq!(blocks.len(), 1, "one block is stored");
    assert_eq!(blocks[0].id, inserted[0].id, "the answered block");

    th.teardown().await;
}

/// Creating multiple blocks in a single `POST /blocks` call.
#[tokio::test]
async fn post_block_create_a_couple_of_blocks_in_the_same_call() {
    let th = Setup::one_user().start().await;
    let board = create_board_in_store(&th, ContainerVisibility::Open).await;
    let first = insert_one(
        &th.client,
        &new_block(&board.id, BlockType::Card, "New title"),
    )
    .await;

    let new_blocks = [
        new_block(&board.id, BlockType::Card, ""),
        new_block(&board.id, BlockType::Card, ""),
    ];
    let inserted = insert(&th.client, &board.id, &new_blocks).await;
    assert_eq!(inserted.len(), 2, "both blocks are answered");
    assert_ne!(inserted[0].id, new_blocks[0].id, "a new identifier");
    assert_ne!(inserted[1].id, new_blocks[1].id, "a new identifier");

    let blocks = blocks_of(&th.client, &board.id).await;
    assert_eq!(blocks.len(), 3, "three blocks are stored");
    let block_ids = ids(&blocks);
    for expected in [&first.id, &inserted[0].id, &inserted[1].id] {
        assert!(
            block_ids.contains(&expected.as_str()),
            "{expected} is stored"
        );
    }

    th.teardown().await;
}

/// Updating an existing block through `POST /blocks` is not allowed.
#[tokio::test]
async fn post_block_update_a_block_should_not_be_possible_through_the_insert_endpoint() {
    let th = Setup::one_user().start().await;
    let board = create_board_in_store(&th, ContainerVisibility::Open).await;
    let first = insert_one(
        &th.client,
        &new_block(&board.id, BlockType::Card, "New title"),
    )
    .await;

    let update = Block {
        id: first.id.clone(),
        updated_at: 20,
        ..new_block(&board.id, BlockType::Card, "Updated title")
    };
    let inserted = insert(&th.client, &board.id, std::slice::from_ref(&update)).await;
    assert_eq!(inserted.len(), 1, "one block is answered");
    assert_ne!(inserted[0].id, first.id, "the identifier was replaced");

    let blocks = blocks_of(&th.client, &board.id).await;
    assert_eq!(blocks.len(), 2, "a second block was stored");
    assert_eq!(
        find(&blocks, &inserted[0].id).title,
        "Updated title",
        "the new block has the title"
    );
    assert_eq!(
        find(&blocks, &first.id).title,
        "New title",
        "the first block is untouched"
    );

    th.teardown().await;
}

/// Fixture block used by the single-block patch tests.
async fn patch_block_fixture(th: &TestServer) -> Block {
    let board = create_board_in_store(th, ContainerVisibility::Open).await;
    let block = Block {
        fields: fields(json!({"test": "test value", "test2": "test value 2"})),
        ..new_block(&board.id, BlockType::Card, "New title")
    };
    insert_one(&th.client, &block).await
}

/// Patching a block's basic fields updates them.
#[tokio::test]
async fn patch_block_patch_a_block_basic_field() {
    let th = Setup::one_user().start().await;
    let block = patch_block_fixture(&th).await;

    patch(&th.client, &block, &title_patch("Updated title")).await;

    let blocks = blocks_of(&th.client, &block.container_id).await;
    assert_eq!(blocks.len(), 1, "still one block");
    assert_eq!(find(&blocks, &block.id).title, "Updated title", "the title");

    th.teardown().await;
}

fn custom_fields_patch() -> BlockPatch {
    BlockPatch {
        updated_fields: fields(json!({"test": "new test value", "test3": "new field"})),
        ..BlockPatch::default()
    }
}

/// Patching a block's custom `fields` updates them.
#[tokio::test]
async fn patch_block_patch_a_block_custom_fields() {
    let th = Setup::one_user().start().await;
    let block = patch_block_fixture(&th).await;
    patch(&th.client, &block, &title_patch("Updated title")).await;

    patch(&th.client, &block, &custom_fields_patch()).await;

    let blocks = blocks_of(&th.client, &block.container_id).await;
    assert_eq!(blocks.len(), 1, "still one block");
    let updated = find(&blocks, &block.id);
    assert_eq!(updated.fields["test"], json!("new test value"), "replaced");
    assert_eq!(updated.fields["test3"], json!("new field"), "added");
    assert_eq!(updated.title, "Updated title", "the title is untouched");

    th.teardown().await;
}

/// Patching a block with `deletedFields` removes custom fields.
#[tokio::test]
async fn patch_block_patch_a_block_to_remove_custom_fields() {
    let th = Setup::one_user().start().await;
    let block = patch_block_fixture(&th).await;
    patch(&th.client, &block, &custom_fields_patch()).await;

    let remove = BlockPatch {
        deleted_fields: vec!["test".to_owned(), "test3".to_owned(), "test100".to_owned()],
        ..BlockPatch::default()
    };
    patch(&th.client, &block, &remove).await;

    let blocks = blocks_of(&th.client, &block.container_id).await;
    assert_eq!(blocks.len(), 1, "still one block");
    let updated = find(&blocks, &block.id);
    assert_eq!(updated.fields.get("test"), None, "removed");
    assert_eq!(updated.fields["test2"], json!("test value 2"), "kept");
    assert_eq!(updated.fields.get("test3"), None, "removed");

    th.teardown().await;
}

/// Creating a block before testing deletion.
#[tokio::test]
async fn delete_block_create_a_block() {
    let th = Setup::one_user().start().await;
    let board = create_board_in_store(&th, ContainerVisibility::Open).await;

    let block = new_block(&board.id, BlockType::Card, "New title");
    let inserted = insert(&th.client, &board.id, std::slice::from_ref(&block)).await;
    assert_eq!(inserted.len(), 1, "one block is answered");
    assert!(!inserted[0].id.is_empty(), "it has an identifier");
    assert_ne!(inserted[0].id, block.id, "a new one");

    let blocks = blocks_of(&th.client, &board.id).await;
    assert_eq!(
        ids(&blocks),
        [inserted[0].id.as_str()],
        "the block is stored"
    );

    th.teardown().await;
}

/// Deleting a block removes it from the board's active block list.
#[tokio::test]
async fn delete_block_delete_a_block() {
    let th = Setup::one_user().start().await;
    let board = create_board_in_store(&th, ContainerVisibility::Open).await;
    let block = insert_one(
        &th.client,
        &new_block(&board.id, BlockType::Card, "New title"),
    )
    .await;

    delete(&th.client, &block).await;

    let response = th.client.get(&blocks_path(&board.id)).await;
    response.check_ok();
    assert_eq!(
        response.text(),
        "[]",
        "no blocks are left, as an empty list"
    );

    th.teardown().await;
}

/// Fixture block for undelete tests: a view on an open board of the first user.
async fn undelete_block_fixture(th: &TestServer) -> Block {
    let board = create_board_in_store(th, ContainerVisibility::Open).await;
    assert!(
        blocks_of(&th.client, &board.id).await.is_empty(),
        "the board starts empty"
    );
    insert_one(
        &th.client,
        &new_block(&board.id, BlockType::View, "New title"),
    )
    .await
}

/// Creating the fixture block used by undelete tests.
#[tokio::test]
async fn undelete_block_create_a_block() {
    let th = Setup::native().start().await.init_basic().await;
    let board = create_board_in_store(&th, ContainerVisibility::Open).await;

    let block = new_block(&board.id, BlockType::View, "New title");
    let inserted = insert(&th.client, &board.id, std::slice::from_ref(&block)).await;
    assert_eq!(inserted.len(), 1, "one block is answered");
    assert!(!inserted[0].id.is_empty(), "it has an identifier");
    assert_ne!(inserted[0].id, block.id, "a new one");

    let blocks = blocks_of(&th.client, &board.id).await;
    assert_eq!(
        ids(&blocks),
        [inserted[0].id.as_str()],
        "the block is stored"
    );

    th.teardown().await;
}

/// Deleting the fixture block hides it from active blocks.
#[tokio::test]
async fn undelete_block_delete_a_block() {
    let th = Setup::native().start().await.init_basic().await;
    let block = undelete_block_fixture(&th).await;

    delete(&th.client, &block).await;

    let blocks = blocks_of(&th.client, &block.container_id).await;
    assert!(blocks.is_empty(), "the block is gone: {:?}", ids(&blocks));

    th.teardown().await;
}

/// Undeleting a deleted block restores it to the board.
#[tokio::test]
async fn undelete_block_undelete_a_block() {
    let th = Setup::native().start().await.init_basic().await;
    let block = undelete_block_fixture(&th).await;
    delete(&th.client, &block).await;

    let undeleted: Block = th
        .client
        .post_empty(&undelete_path(&block.container_id, &block.id))
        .await
        .ok_json();
    assert_eq!(undeleted.id, block.id, "the answer is the restored block");
    assert_eq!(undeleted.title, "New title", "with its content");
    assert_eq!(undeleted.deleted_at, None, "and not deleted");

    let blocks = blocks_of(&th.client, &block.container_id).await;
    assert_eq!(ids(&blocks), [block.id.as_str()], "the block is back");

    th.teardown().await;
}

/// Rejects undeleting a block without write permission on the board.
#[tokio::test]
async fn undelete_block_try_to_undelete_a_block_without_permissions() {
    let th = Setup::native().start().await.init_basic().await;
    let block = undelete_block_fixture(&th).await;
    let path = undelete_path(&block.container_id, &block.id);
    delete(&th.client, &block).await;
    th.client.post_empty(&path).await.check_ok();
    delete(&th.client, &block).await;

    let response = th.client2.post_empty(&path).await;
    response.check_error(StatusCode::FORBIDDEN, NO_CHANGES);

    let blocks = blocks_of(&th.client, &block.container_id).await;
    assert!(blocks.is_empty(), "the block stays deleted");

    th.teardown().await;
}

// -- Application-layer block tests, through the real store --------------------------------

/// Inserting a single block via the application layer succeeds.
#[tokio::test]
async fn app_insert_block_success_scenario() {
    let th = Setup::one_user().start().await;
    let board = create_board_in_store(&th, ContainerVisibility::Open).await;
    let mut block = new_block(&board.id, BlockType::Card, "A card");

    th.app
        .insert_block(&mut block, &th.stranger().await, ChangeNotification::Send)
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
        .insert_block(
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
        .insert_block(
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
        .patch_blocks(
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
        .patch_blocks(
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
        .delete_block(&block.id, &th.stranger().await, ChangeNotification::Send)
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
        .delete_block(
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
        .delete_block(&block.id, &th.stranger().await, ChangeNotification::Send)
        .await
        .expect("the block is deleted");

    let undeleted = th
        .app
        .undelete_block(&block.id, &th.stranger().await)
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
            .undelete_block(block_id, &th.stranger().await)
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
        .insert_blocks(&mut blocks, &th.stranger().await, ChangeNotification::Send)
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
        .insert_blocks(&mut [], &th.stranger().await, ChangeNotification::Send)
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
        .insert_blocks(
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
        .insert_blocks(&mut mixed, &th.stranger().await, ChangeNotification::Send)
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

// -- GET /boards/{board_id}/blocks ------------------------------------------------------

/// Who may read the blocks of a board, and what the others are told.
#[tokio::test]
async fn get_blocks_permissions() {
    let th = Setup::native().start().await.init_basic().await;
    let boards = boards_with_roles(&th).await;
    let open_board = create_board_in_store(&th, ContainerVisibility::Open).await;
    card_with_content(&th, &boards).await;
    let path = blocks_path(&boards.board.id);

    let response = th.anonymous().get(&path).await;
    response.check_unauthorized();
    assert_eq!(response.error_message(), "no session token", "no session");

    // Without a session the board is not even looked up.
    let response = th.anonymous().get(&blocks_path("no-such-board")).await;
    response.check_unauthorized();
    assert_eq!(
        response.error_message(),
        "no session token",
        "no session, no board"
    );

    let response = th.client.get(&blocks_path("no-such-board")).await;
    response.check_error(StatusCode::NOT_FOUND, NO_BOARD);

    // No member: an open board can be read by workspace members; the private one does
    // not exist.
    let open_blocks: Vec<Block> = th.client2.get(&blocks_path(&open_board.id)).await.ok_json();
    assert!(
        open_blocks.is_empty(),
        "open board is readable in the workspace"
    );
    let response = th.client2.get(&blocks_path(&boards.board.id)).await;
    response.check_error(StatusCode::NOT_FOUND, NO_BOARD);

    for (role, client) in [
        ("viewer", &boards.viewer),
        ("commenter", &boards.commenter),
        ("editor", &boards.editor),
        ("admin", &th.client),
    ] {
        let blocks: Vec<Block> = client.get(&path).await.ok_json();
        assert_eq!(titles(&blocks), ["card", "comment", "text"], "{role}");
    }

    th.teardown().await;
}

/// The blocks of an open template are for everyone in the workspace; a private template is
/// for its members.
#[tokio::test]
async fn get_blocks_of_templates() {
    let th = Setup::native().start().await.init_basic().await;
    let template = |board_type| Container {
        workspace_id: TEST_WORKSPACE_ID.to_owned(),
        visibility: board_type,
        is_template: true,
        ..Container::default()
    };
    let open_template = store_board(&th, template(ContainerVisibility::Open)).await;
    let private_template = store_board(&th, template(ContainerVisibility::Private)).await;
    for board in [&open_template, &private_template] {
        let block = new_block(&board.id, BlockType::Card, "in a template");
        insert_one(&th.client, &block).await;
    }

    let blocks: Vec<Block> = th
        .client2
        .get(&blocks_path(&open_template.id))
        .await
        .ok_json();
    assert_eq!(titles(&blocks), ["in a template"], "no membership needed");

    // A private template is for its members; for everyone else it does not exist.
    let response = th.client2.get(&blocks_path(&private_template.id)).await;
    response.check_error(StatusCode::NOT_FOUND, NO_BOARD);

    let blocks: Vec<Block> = th
        .client
        .get(&blocks_path(&private_template.id))
        .await
        .ok_json();
    assert_eq!(blocks.len(), 1, "a member reads the private template");

    th.teardown().await;
}

/// The list parameters of `GET /boards/{board_id}/blocks`: `all`, `parentId` and
/// `type`.
#[tokio::test]
async fn get_blocks_filters() {
    let th = Setup::one_user().start().await;
    let board = create_board_in_store(&th, ContainerVisibility::Open).await;
    let path = blocks_path(&board.id);

    let empty = th.client.get(&path).await;
    empty.check_ok();
    assert_eq!(empty.text(), "[]", "an empty board is an empty list");

    let card = insert_one(&th.client, &new_block(&board.id, BlockType::Card, "card")).await;
    let children = [
        child_of(&card, BlockType::Attachment, "text"),
        child_of(&card, BlockType::Comment, "comment"),
    ];
    insert(&th.client, &board.id, &children).await;
    insert_one(&th.client, &new_block(&board.id, BlockType::Card, "card 2")).await;
    let children_of_card = format!("parentId={}", card.id);

    let everything = vec!["card", "card 2", "comment", "text"];
    let none: Vec<&str> = Vec::new();
    let cases = [
        ("no parameters", String::new(), everything.clone()),
        ("all", "?all=true".to_owned(), everything.clone()),
        (
            "all wins over the filters",
            "?all=true&type=card&parentId=x".to_owned(),
            everything.clone(),
        ),
        (
            "an empty all is no all",
            "?all=".to_owned(),
            everything.clone(),
        ),
        (
            "the children of the card",
            format!("?{children_of_card}"),
            vec!["comment", "text"],
        ),
        ("by type", "?type=card".to_owned(), vec!["card", "card 2"]),
        (
            "by type and parent",
            format!("?type=attachment&{children_of_card}"),
            vec!["text"],
        ),
        (
            "no card has the card as parent",
            format!("?type=card&{children_of_card}"),
            none.clone(),
        ),
        (
            "a type the server does not know is matched as written: no block has it",
            "?type=h1".to_owned(),
            none.clone(),
        ),
        (
            "unknown is a type as any other",
            "?type=unknown".to_owned(),
            none.clone(),
        ),
        ("the type is case-sensitive", "?type=Card".to_owned(), none),
    ];
    for (case, query, expected) in cases {
        let blocks: Vec<Block> = th.client.get(&format!("{path}{query}")).await.ok_json();
        assert_eq!(titles(&blocks), expected, "{case}");
    }
    th.client
        .get(&format!("{path}?page=1&perPage=1"))
        .await
        .check_bad_request();

    let no_children = th.client.get(&format!("{path}?parentId=nobody")).await;
    no_children.check_ok();
    assert_eq!(no_children.text(), "[]", "no match is an empty list");

    th.teardown().await;
}

/// `GET /boards/{board_id}/blocks?blockId=`: one block, if it is on the board.
#[tokio::test]
async fn get_blocks_by_block_id() {
    let th = Setup::one_user().start().await;
    let board = create_board_in_store(&th, ContainerVisibility::Open).await;
    let other_board = create_board_in_store(&th, ContainerVisibility::Open).await;
    let path = blocks_path(&board.id);
    let card = insert_one(&th.client, &new_block(&board.id, BlockType::Card, "card")).await;
    insert_one(&th.client, &child_of(&card, BlockType::Attachment, "text")).await;
    let other_block = insert_one(
        &th.client,
        &new_block(&other_board.id, BlockType::Card, "other"),
    )
    .await;

    let blocks: Vec<Block> = th
        .client
        .get(&format!("{path}?blockId={}&type=text", card.id))
        .await
        .ok_json();
    assert_eq!(
        ids(&blocks),
        [card.id.as_str()],
        "the one block, whatever the filters say"
    );

    let response = th
        .client
        .get(&format!("{path}?blockId=no-such-block"))
        .await;
    response.check_error(StatusCode::NOT_FOUND, NO_BLOCK);

    let response = th
        .client
        .get(&format!("{path}?blockId={}", other_block.id))
        .await;
    // A block of another board answers as one that does not exist.
    response.check_error(StatusCode::NOT_FOUND, NO_BLOCK);

    th.teardown().await;
}

/// A query parameter has one spelling. Any other spelling of it is refused with 400
/// (`deny_unknown_fields`), while the exact spelling filters or checks as documented.
#[tokio::test]
async fn a_query_parameter_spelled_another_way_is_no_parameter() {
    let th = Setup::local_mode().start().await.init_basic().await;
    let boards = boards_with_roles(&th).await;
    let content = card_with_content(&th, &boards).await;
    let path = blocks_path(&boards.board.id);

    for query in [
        format!("parent_id={}", content.card.id),
        "parent_id=nobody".to_owned(),
        format!("parentID={}", content.card.id),
        format!("block_id={}", content.text.id),
        "block_id=no-such-block".to_owned(),
        format!("blockID={}", content.text.id),
    ] {
        th.client
            .get(&format!("{path}?{query}"))
            .await
            .check_bad_request();
    }
    let children: Vec<Block> = th
        .client
        .get(&format!("{path}?parentId={}", content.card.id))
        .await
        .ok_json();
    assert_eq!(
        titles(&children),
        ["comment", "text"],
        "the parameter itself filters"
    );

    // No read token, under any name: anonymous requests are refused for missing session,
    // strict query routes refuse unknown parameters, and container routes hide private boards.
    let read_token = th.share(&boards.board.id).await;
    let anonymous = th.anonymous();
    for name in ["readToken", "read_token", "readtoken", "ReadToken"] {
        let blocks_with_token = format!("{path}?{name}={read_token}");
        let response = anonymous.get(&blocks_with_token).await;
        response.check_unauthorized();
        assert_eq!(
            response.problem_type(),
            "/problems/unauthorized",
            "{blocks_with_token}"
        );
        th.client2.get(&blocks_with_token).await.check_bad_request();

        let container_with_token = format!(
            "{TEST_WS}/containers/{}?{name}={read_token}",
            boards.board.id
        );
        anonymous
            .get(&container_with_token)
            .await
            .check_unauthorized();
        th.client2
            .get(&container_with_token)
            .await
            .check_error(StatusCode::NOT_FOUND, NO_BOARD);
    }

    // Source board parameter: a missing source is 404, and other spellings are 400.
    let view = new_block(&boards.board.id, BlockType::View, "posted");
    let response = th
        .client
        .post(&format!("{path}?sourceBoardId=no-such-board"), &[&view])
        .await;
    response.check_error(StatusCode::NOT_FOUND, "source board not found");
    for name in ["sourceBoardID", "source_board_id"] {
        th.client
            .post(&format!("{path}?{name}=no-such-board"), &[&view])
            .await
            .check_bad_request();
    }

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
