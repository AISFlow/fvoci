//! Integration tests for combined boards-and-blocks routes (`POST`/`PATCH`/`DELETE
//! /containers-and-blocks`), covering validation, permissions, atomicity, and broadcasts.
//!
//! Helper fixtures arrange prerequisite boards and blocks directly through the store.
//!
//! The identifiers in a request that creates boards and blocks (`"board-id-1"`, `"bd"`,
//! `"b1"`) only tie the request's blocks to its boards: nothing is stored under them, so
//! they need not be identifiers.

use fvoci_model::{
    Block, BlockPatch, BlockType, Container, ContainerKinds, ContainerPatch, ContainerRole,
    ContainerVisibility, ContainersAndBlocks, DeleteContainersAndBlocks, PatchContainersAndBlocks,
    new_id,
};
use fvoci_store::BlockFilter;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

use crate::board_broadcasts::WorkspaceSocket;
use crate::boards::{
    board_with_admin, board_without_members, insert_block, member, members_of, new_board,
};
use crate::common::TEST_WS;
use crate::common::{
    ApiResponse, Client, ROOT_WORKSPACE_ID, Setup, TEST_WORKSPACE_ID, TestServer, UNKNOWN_ID,
};

const PATH: &str = "/w/00000000-0000-7000-8000-000000000001/containers-and-blocks";

// -- helpers --------------------------------------------------------------------------

fn request_board(
    id: &str,
    workspace_id: &str,
    board_type: ContainerVisibility,
    title: &str,
) -> Container {
    Container {
        id: id.to_owned(),
        ..new_board(workspace_id, board_type, title)
    }
}

fn request_block(id: &str, board_id: &str, title: &str) -> Block {
    Block {
        id: id.to_owned(),
        container_id: board_id.to_owned(),
        title: title.to_owned(),
        block_type: BlockType::Card,
        created_at: 1,
        updated_at: 1,
        ..Block::default()
    }
}

/// `th.Server.App().InsertBlock(&model.Block{ID, BoardID, Title}, userID)`, under a new
/// identifier.
async fn stored_block(th: &TestServer, board_id: &str, user_id: &str) -> Block {
    insert_block(
        th,
        Block {
            id: new_id(),
            container_id: board_id.to_owned(),
            title: "initial title".to_owned(),
            ..Block::default()
        },
        user_id,
    )
    .await
}

async fn blocks_of(th: &TestServer, board_id: &str) -> Vec<Block> {
    th.app
        .store()
        .get_blocks(&BlockFilter::container(board_id))
        .await
        .expect("the blocks are read")
}

fn title_patches(count: usize, title: &str) -> (Vec<ContainerPatch>, Vec<BlockPatch>) {
    let board_patch = ContainerPatch {
        title: Some(title.to_owned()),
        ..ContainerPatch::default()
    };
    let block_patch = BlockPatch {
        title: Some(title.to_owned()),
        ..BlockPatch::default()
    };
    (vec![board_patch; count], vec![block_patch; count])
}

fn strings(ids: &[&str]) -> Vec<String> {
    ids.iter().map(|id| (*id).to_owned()).collect()
}

fn expect_bad_request(response: &ApiResponse, message: &str) {
    response.check_error(StatusCode::BAD_REQUEST, message);
}

/// Two open boards of user1, each with one block, used by the batch patch tests. The
/// first is in the test workspace, the second in `second_workspace`, and has user1 as
/// admin only if `second` says so.
struct TwoBoards {
    board1: Container,
    board2: Container,
    block1: Block,
    block2: Block,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SecondBoard {
    WithAdmin,
    WithoutMembers,
}

async fn two_boards(th: &TestServer, second: SecondBoard, second_workspace: &str) -> TwoBoards {
    let user_id = th.client.me().await.id;
    let board1 = board_with_admin(
        th,
        new_board(
            TEST_WORKSPACE_ID,
            ContainerVisibility::Open,
            "initial title",
        ),
        &user_id,
    )
    .await;
    let board2 = new_board(second_workspace, ContainerVisibility::Open, "initial title");
    let board2 = match second {
        SecondBoard::WithAdmin => board_with_admin(th, board2, &user_id).await,
        SecondBoard::WithoutMembers => board_without_members(th, board2, &user_id).await,
    };
    let block1 = stored_block(th, &board1.id, &user_id).await;
    let block2 = stored_block(th, &board2.id, &user_id).await;
    TwoBoards {
        board1,
        board2,
        block1,
        block2,
    }
}

async fn assert_titles_unchanged(th: &TestServer, boards: &TwoBoards) {
    for board in [&boards.board1, &boards.board2] {
        let stored = th.app.container(&board.id).await.expect("the board exists");
        assert_eq!(stored.title, "initial title", "board {}", board.id);
    }
    for block in [&boards.block1, &boards.block2] {
        let stored = th
            .app
            .store()
            .get_block(&block.id)
            .await
            .expect("the block exists");
        assert_eq!(stored.title, "initial title", "block {}", block.id);
    }
}

// -- creating boards and blocks together ----------------------------------------------

/// Unauthenticated `POST /containers-and-blocks` is rejected.
#[tokio::test]
async fn create_boards_and_blocks_a_non_authenticated_user_should_be_rejected() {
    let th = Setup::native().start().await;

    let response = th.client.post(PATH, &ContainersAndBlocks::default()).await;

    response.check_unauthorized();
    th.teardown().await;
}

/// Rejects creating boards and blocks when no boards are provided.
#[tokio::test]
async fn create_boards_and_blocks_no_boards() {
    let th = Setup::native().start().await.init_basic().await;
    let request = ContainersAndBlocks {
        containers: vec![],
        blocks: vec![Block {
            created_at: 0,
            updated_at: 0,
            ..request_block("block-id", "board-id", "")
        }],
    };

    let response = th.client.post(PATH, &request).await;

    expect_bad_request(&response, "at least one board is required");
    th.teardown().await;
}

/// Rejects creating boards and blocks when no blocks are provided.
#[tokio::test]
async fn create_boards_and_blocks_no_blocks() {
    let th = Setup::native().start().await.init_basic().await;
    let request = ContainersAndBlocks {
        containers: vec![request_board(
            "board-id",
            TEST_WORKSPACE_ID,
            ContainerVisibility::Private,
            "",
        )],
        blocks: vec![],
    };

    let response = th.client.post(PATH, &request).await;

    expect_bad_request(&response, "at least one block is required");
    th.teardown().await;
}

/// Rejects blocks referencing nonexistent boards in `POST /containers-and-blocks`.
#[tokio::test]
async fn create_boards_and_blocks_blocks_from_nonexistent_boards() {
    let th = Setup::native().start().await.init_basic().await;
    let request = ContainersAndBlocks {
        containers: vec![request_board(
            "board-id",
            TEST_WORKSPACE_ID,
            ContainerVisibility::Private,
            "",
        )],
        blocks: vec![request_block("block-id", "nonexistent-board-id", "")],
    };

    let response = th.client.post(PATH, &request).await;

    expect_bad_request(&response, "the block is on no board of the request");
    assert_eq!(response.refused_at(), "/blocks/0/containerId");
    th.teardown().await;
}

/// Rejects boards missing temporary identifiers in `POST /containers-and-blocks`.
#[tokio::test]
async fn create_boards_and_blocks_boards_with_no_ids() {
    let th = Setup::native().start().await.init_basic().await;
    let request = ContainersAndBlocks {
        containers: vec![
            request_board(
                "board-id",
                TEST_WORKSPACE_ID,
                ContainerVisibility::Private,
                "",
            ),
            request_board("", TEST_WORKSPACE_ID, ContainerVisibility::Private, ""),
        ],
        blocks: vec![request_block("block-id", "board-id", "")],
    };

    let response = th.client.post(PATH, &request).await;

    expect_bad_request(
        &response,
        "boards need an ID to be referenced from the blocks",
    );
    th.teardown().await;
}

/// Rejects creating boards belonging to different workspaces in one request.
#[tokio::test]
async fn create_boards_and_blocks_boards_from_different_workspaces() {
    let th = Setup::native().start().await.init_basic().await;
    let request = ContainersAndBlocks {
        containers: vec![
            request_board(
                "board-id-1",
                TEST_WORKSPACE_ID,
                ContainerVisibility::Private,
                "",
            ),
            request_board(
                "board-id-2",
                ROOT_WORKSPACE_ID,
                ContainerVisibility::Private,
                "",
            ),
        ],
        blocks: vec![request_block("block-id", "board-id-1", "")],
    };

    let response = th.client.post(PATH, &request).await;

    // The path names the workspace, and the second board names another.
    expect_bad_request(&response, "the board is made in the workspace of the path");
    assert_eq!(response.refused_at(), "/containers/1/workspaceId");
    th.teardown().await;
}

/// Creates boards and blocks together and maps temporary IDs to generated ones.
#[tokio::test]
async fn create_boards_and_blocks_creating_boards_and_blocks() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;
    let request = ContainersAndBlocks {
        containers: vec![
            request_board(
                "board-id-1",
                TEST_WORKSPACE_ID,
                ContainerVisibility::Open,
                "public board",
            ),
            request_board(
                "board-id-2",
                TEST_WORKSPACE_ID,
                ContainerVisibility::Private,
                "private board",
            ),
        ],
        blocks: vec![
            request_block("block-id-1", "board-id-1", "block 1"),
            request_block("block-id-2", "board-id-2", "block 2"),
        ],
    };

    let created: ContainersAndBlocks = th.client.post(PATH, &request).await.created_json();

    assert_eq!(created.containers.len(), 2, "two boards");
    assert_eq!(created.blocks.len(), 2, "two blocks");

    // Each board should have been created with a new ID, and its block should be there
    // too.
    for (term, title, board_type, old_id, block_title) in [
        (
            "public",
            "public board",
            ContainerVisibility::Open,
            "board-id-1",
            "block 1",
        ),
        (
            "private",
            "private board",
            ContainerVisibility::Private,
            "board-id-2",
            "block 2",
        ),
    ] {
        let found = th
            .app
            .store()
            .search_containers_for_user_in_workspace(
                TEST_WORKSPACE_ID,
                term,
                &user1.id,
                ContainerKinds::BOARDS,
            )
            .await
            .expect("the boards are searched");
        assert_eq!(found.len(), 1, "one board matches {term:?}: {found:?}");
        let board = &found[0];
        assert_eq!(board.title, title, "the title");
        assert_eq!(board.visibility, board_type, "the type");
        assert_ne!(board.id, old_id, "a new identifier");
        assert!(
            created.containers.contains(board),
            "the answer has the board as stored"
        );
        let blocks = blocks_of(&th, &board.id).await;
        assert_eq!(blocks.len(), 1, "one block: {blocks:?}");
        assert_eq!(blocks[0].title, block_title, "the block's title");
        assert_ne!(blocks[0].id, old_id.replace("board", "block"), "a new id");

        // user should be an admin of both newly created boards
        let members = members_of(&th, &board.id).await;
        assert_eq!(
            members,
            vec![member(
                &board.id,
                &user1.id,
                fvoci_model::ContainerRole::Admin
            )],
            "the creator administers the board"
        );
    }

    // Both boards are filed under the creator's default category.
    let categories = th
        .app
        .user_category_containers(&user1.id, TEST_WORKSPACE_ID)
        .await
        .expect("the categories are read");
    let default = categories
        .iter()
        .find(|entry| entry.category.name == "Boards")
        .expect("the default category exists");
    assert_eq!(default.container_metadata.len(), 2, "both boards are filed");

    th.teardown().await;
}

/// A card as a request names it, with the times the route insists on.
fn card_json(board_id: &str) -> Value {
    json!({"id": "b1", "containerId": board_id, "type": "card", "createdAt": 1, "updatedAt": 1})
}

/// The boards of the test workspace that a client's user sees.
async fn workspace_boards(client: &Client) -> Vec<Container> {
    client.get(&format!("{TEST_WS}/boards")).await.ok_json()
}

/// Additional validation refusals for `POST /containers-and-blocks`.
#[tokio::test]
async fn create_boards_and_blocks_refusals() {
    let th = Setup::native().start().await.init_basic().await;
    let board = json!({"id": "bd", "workspaceId": TEST_WORKSPACE_ID, "visibility": "private"});

    // A body that is no JSON object is a bad request.
    for body in ["{bad", "", "null", "[]"] {
        let response = th.client.send_raw_body(Method::POST, PATH, body).await;
        response.check_bad_request();
        assert_eq!(response.problem_type(), "/problems/bad-request", "{body}");
    }

    // Every board needs an identifier for the blocks to name, the first one too.
    for boards in [
        json!([{"workspaceId": TEST_WORKSPACE_ID, "visibility": "private", "title": "no id"}]),
        json!([board, {"workspaceId": TEST_WORKSPACE_ID, "visibility": "private"}]),
    ] {
        let response = th
            .client
            .post(
                PATH,
                &json!({"containers": boards, "blocks": [card_json("")]}),
            )
            .await;
        expect_bad_request(
            &response,
            "boards need an ID to be referenced from the blocks",
        );
    }

    // Blocks are checked one by one: a block without a `type` is refused at `/blocks/0/type`,
    // while `createdAt` and `updatedAt` are server-assigned and need not be sent by clients.
    let response = th
        .client
        .post(
            PATH,
            &json!({"containers": [board], "blocks": [{"id": "b1", "containerId": "bd"}]}),
        )
        .await;
    expect_bad_request(&response, "the block has no type");
    assert_eq!(response.refused_at(), "/blocks/0/type");

    // A board that names a workspace other than the path's, one that does not exist and
    // text that is no identifier among them, is refused; the path names the workspace.
    for board in [
        json!({"id": "bd", "workspaceId": UNKNOWN_ID, "visibility": "private"}),
        json!({"id": "bd", "workspaceId": ROOT_WORKSPACE_ID, "visibility": "private"}),
        json!({"id": "bd", "workspaceId": "nope", "visibility": "private"}),
    ] {
        let response = th
            .client
            .post(
                PATH,
                &json!({"containers": [board], "blocks": [card_json("bd")]}),
            )
            .await;
        expect_bad_request(&response, "the board is made in the workspace of the path");
        assert_eq!(
            response.refused_at(),
            "/containers/0/workspaceId",
            "the member at fault: {board}"
        );
    }

    // A block that fails the store's validation: a bad request that names the block by
    // its new identifier. Nothing is stored.
    let mut long_title = card_json("bd");
    long_title["title"] = json!("x".repeat(70_000));
    let response = th
        .client
        .post(
            PATH,
            &json!({"containers": [board], "blocks": [long_title]}),
        )
        .await;
    response.check_bad_request();
    assert!(
        response
            .error_message()
            .ends_with(": block title size limit exceeded"),
        "{}",
        response.error_message()
    );

    assert_eq!(
        workspace_boards(&th.client).await,
        vec![],
        "nothing was created"
    );
    th.teardown().await;
}

/// `POST /containers-and-blocks` accepts creating a template board with its blocks.
#[tokio::test]
async fn create_boards_and_blocks_accepts_a_template() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;

    // A template is created like a board but filed under no category.
    let created: ContainersAndBlocks = th
        .client
        .post(
            PATH,
            &json!({
                "containers": [{"id": "bd", "workspaceId": TEST_WORKSPACE_ID, "visibility": "open", "isTemplate": true}],
                "blocks": [card_json("bd")],
            }),
        )
        .await
        .created_json();
    assert!(created.containers[0].is_template, "a template");
    let categories = th
        .app
        .user_category_containers(&user1.id, TEST_WORKSPACE_ID)
        .await
        .expect("the categories are read");
    assert!(
        !categories
            .iter()
            .flat_map(|entry| &entry.container_metadata)
            .any(|metadata| metadata.container_id == created.containers[0].id),
        "the template is in no category"
    );

    th.teardown().await;
}

/// A board of a type or minimum role outside their sets is refused as `POST /boards`
/// refuses it, at the member of that board, with words that do not repeat the value.
#[tokio::test]
async fn create_boards_and_blocks_refuses_unknown_board_types_and_roles() {
    let th = Setup::native().start().await.init_basic().await;
    let block = card_json("bd");

    for (board, member, message) in [
        (
            json!({"id": "bd", "workspaceId": TEST_WORKSPACE_ID, "visibility": "invalid"}),
            "visibility",
            "the visibility of a board is `open` or `private`",
        ),
        (
            json!({"id": "bd", "workspaceId": TEST_WORKSPACE_ID}),
            "visibility",
            "the visibility of a board is `open` or `private`",
        ),
        (
            json!({"id": "bd", "workspaceId": TEST_WORKSPACE_ID, "visibility": "open", "minimumRole": "boss"}),
            "minimumRole",
            "the minimum role of a board is `viewer`, `commenter` or `editor`",
        ),
    ] {
        let response = th
            .client
            .post(PATH, &json!({"containers": [board], "blocks": [block]}))
            .await;
        expect_bad_request(&response, message);
        assert_eq!(response.refused_at(), format!("/containers/0/{member}"));
    }

    assert_eq!(
        workspace_boards(&th.client).await,
        vec![],
        "nothing was created"
    );
    th.teardown().await;
}

/// Creating boards and blocks broadcasts each board, block, and membership to the creator.
#[tokio::test]
async fn create_boards_and_blocks_is_broadcast() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;
    let mut socket =
        WorkspaceSocket::subscribe(&th, &th.client, TEST_WORKSPACE_ID, &user1.id).await;
    let request = ContainersAndBlocks {
        containers: vec![request_board(
            "board-id-1",
            TEST_WORKSPACE_ID,
            ContainerVisibility::Open,
            "public board",
        )],
        blocks: vec![request_block("block-id-1", "board-id-1", "block 1")],
    };

    let created: ContainersAndBlocks = th.client.post(PATH, &request).await.created_json();

    let board_id = &created.containers[0].id;
    // The board and block messages are sent before the creator is a member in the eyes
    // of the hub only if the membership were written later; it is written with the
    // board, so all three arrive.
    socket
        .expect("containerChanged", "/container/id", board_id)
        .await;
    let message = socket
        .expect("blockChanged", "/block/id", &created.blocks[0].id)
        .await;
    assert_eq!(message["block"]["title"], json!("block 1"), "{message}");
    let message = socket
        .expect("containerMemberChanged", "/member/containerId", board_id)
        .await;
    assert_eq!(message["member"]["userId"], json!(user1.id), "{message}");
    th.teardown().await;
}

// -- patching boards and blocks together ----------------------------------------------

async fn patch(client: &Client, request: &impl serde::Serialize) -> ApiResponse {
    client.patch(PATH, request).await
}

/// Unauthenticated `PATCH /containers-and-blocks` is rejected.
#[tokio::test]
async fn patch_boards_and_blocks_a_non_authenticated_user_should_be_rejected() {
    let th = Setup::native().start().await;

    let response = patch(&th.client, &PatchContainersAndBlocks::default()).await;

    response.check_unauthorized();
    th.teardown().await;
}

/// Rejects `PATCH /containers-and-blocks` when no board IDs are given.
#[tokio::test]
async fn patch_boards_and_blocks_no_board_ids() {
    let th = Setup::native().start().await.init_basic().await;
    let boards = two_boards(&th, SecondBoard::WithAdmin, TEST_WORKSPACE_ID).await;
    let (board_patches, block_patches) = title_patches(2, "new title 1");
    let request = PatchContainersAndBlocks {
        container_ids: vec![],
        container_patches: board_patches,
        block_ids: strings(&[&boards.block1.id, &boards.block2.id]),
        block_patches,
    };

    let response = patch(&th.client, &request).await;

    expect_bad_request(&response, "at least one board is required");
    assert_titles_unchanged(&th, &boards).await;
    th.teardown().await;
}

/// Rejects `PATCH /containers-and-blocks` when board IDs and patches have different lengths.
#[tokio::test]
async fn patch_boards_and_blocks_missmatch_board_ids_and_patches() {
    let th = Setup::native().start().await.init_basic().await;
    let boards = two_boards(&th, SecondBoard::WithAdmin, TEST_WORKSPACE_ID).await;
    let (mut board_patches, block_patches) = title_patches(2, "new title 1");
    board_patches.truncate(1);
    let request = PatchContainersAndBlocks {
        container_ids: strings(&[&boards.board1.id, &boards.board2.id]),
        container_patches: board_patches,
        block_ids: strings(&[&boards.block1.id, &boards.block2.id]),
        block_patches,
    };

    let response = patch(&th.client, &request).await;

    expect_bad_request(&response, "board ids and patches need to match");
    assert_titles_unchanged(&th, &boards).await;
    th.teardown().await;
}

/// Rejects `PATCH /containers-and-blocks` when block patches exist without block IDs.
#[tokio::test]
async fn patch_boards_and_blocks_no_block_ids() {
    let th = Setup::native().start().await.init_basic().await;
    let boards = two_boards(&th, SecondBoard::WithAdmin, TEST_WORKSPACE_ID).await;
    let (board_patches, block_patches) = title_patches(2, "new title 1");
    let request = PatchContainersAndBlocks {
        container_ids: strings(&[&boards.board1.id, &boards.board2.id]),
        container_patches: board_patches,
        block_ids: vec![],
        block_patches,
    };

    let response = patch(&th.client, &request).await;

    expect_bad_request(&response, "block ids and patches need to match");
    assert_titles_unchanged(&th, &boards).await;
    th.teardown().await;
}

/// Rejects `PATCH /containers-and-blocks` when block IDs and patches have different lengths.
#[tokio::test]
async fn patch_boards_and_blocks_missmatch_block_ids_and_patches() {
    let th = Setup::native().start().await.init_basic().await;
    let boards = two_boards(&th, SecondBoard::WithAdmin, TEST_WORKSPACE_ID).await;
    let (board_patches, mut block_patches) = title_patches(2, "new title 1");
    block_patches.truncate(1);
    let request = PatchContainersAndBlocks {
        container_ids: strings(&[&boards.board1.id, &boards.board2.id]),
        container_patches: board_patches,
        block_ids: strings(&[&boards.block1.id, &boards.block2.id]),
        block_patches,
    };

    let response = patch(&th.client, &request).await;

    expect_bad_request(&response, "block ids and patches need to match");
    assert_titles_unchanged(&th, &boards).await;
    th.teardown().await;
}

/// Rejects `PATCH /containers-and-blocks` when a block does not belong to any board.
#[tokio::test]
async fn patch_boards_and_blocks_block_that_doesnt_belong_to_any_board() {
    let th = Setup::native().start().await.init_basic().await;
    let boards = two_boards(&th, SecondBoard::WithAdmin, TEST_WORKSPACE_ID).await;
    let (mut board_patches, block_patches) = title_patches(2, "new title 1");
    board_patches.truncate(1);
    let request = PatchContainersAndBlocks {
        container_ids: strings(&[&boards.board1.id]),
        container_patches: board_patches,
        block_ids: strings(&[&boards.block1.id, &boards.block2.id]),
        block_patches,
    };

    let response = patch(&th.client, &request).await;

    expect_bad_request(
        &response,
        "the request names a block that is not on its boards",
    );
    assert_titles_unchanged(&th, &boards).await;
    th.teardown().await;
}

/// Updates nothing if the user lacks permission on any of the target boards.
#[tokio::test]
async fn patch_boards_and_blocks_without_permissions_for_one_board_nothing_is_updated() {
    let th = Setup::native().start().await.init_basic().await;
    let boards = two_boards(&th, SecondBoard::WithoutMembers, TEST_WORKSPACE_ID).await;
    let (board_patches, block_patches) = title_patches(2, "new title 2");
    let request = PatchContainersAndBlocks {
        container_ids: strings(&[&boards.board1.id, &boards.board2.id]),
        container_patches: board_patches,
        block_ids: strings(&[&boards.block1.id, &boards.block2.id]),
        block_patches,
    };

    let response = patch(&th.client, &request).await;

    response.check_forbidden();
    assert_eq!(
        response.error_message(),
        "access denied to modifying board properties",
        "why"
    );
    assert_titles_unchanged(&th, &boards).await;
    th.teardown().await;
}

/// Rejects `PATCH /containers-and-blocks` across boards belonging to different workspaces.
#[tokio::test]
async fn patch_boards_and_blocks_boards_belonging_to_different_workspaces_should_be_rejected() {
    let th = Setup::native().start().await.init_basic().await;
    let boards = two_boards(&th, SecondBoard::WithAdmin, ROOT_WORKSPACE_ID).await;
    let (board_patches, block_patches) = title_patches(2, "new title 3");
    let request = PatchContainersAndBlocks {
        container_ids: strings(&[&boards.board1.id, &boards.board2.id]),
        container_patches: board_patches,
        block_ids: strings(&[&boards.block1.id, &boards.block2.id]),
        block_patches,
    };

    let response = patch(&th.client, &request).await;

    // A board of another workspace is not there under this one's path.
    response.check_error(StatusCode::NOT_FOUND, "board not found");
    assert_titles_unchanged(&th, &boards).await;
    th.teardown().await;
}

/// Rejects `PATCH /containers-and-blocks` when any single patch is invalid.
#[tokio::test]
async fn patch_boards_and_blocks_patches_should_be_rejected_if_one_is_invalid() {
    let th = Setup::native().start().await.init_basic().await;
    let boards = two_boards(&th, SecondBoard::WithoutMembers, TEST_WORKSPACE_ID).await;
    let request = json!({
        "containerIds": [boards.board1.id, boards.board2.id],
        "containerPatches": [{"title": "new title 4"}, {"visibility": "invalid"}],
        "blockIds": [boards.block1.id, boards.block2.id],
        "blockPatches": [{"title": "new title 4"}, {"title": "new title 4"}],
    });

    let response = patch(&th.client, &request).await;

    expect_bad_request(
        &response,
        "the visibility of a board is `open` or `private`",
    );
    assert_eq!(response.refused_at(), "/containerPatches/1/visibility");
    assert_titles_unchanged(&th, &boards).await;

    // The body is checked whole before any board is looked at: a patch refused for its
    // value is refused also when a board before it may not be changed.
    let swapped = json!({
        "containerIds": [boards.board2.id, boards.board1.id],
        "containerPatches": [{"title": "new title 4"}, {"minimumRole": "boss"}],
        "blockIds": [],
        "blockPatches": [],
    });
    let response = patch(&th.client, &swapped).await;
    expect_bad_request(
        &response,
        "the minimum role of a board is `viewer`, `commenter` or `editor`",
    );
    assert_eq!(response.refused_at(), "/containerPatches/1/minimumRole");
    let role = json!({
        "containerIds": [boards.board1.id],
        "containerPatches": [{"minimumRole": "boss"}],
        "blockIds": [],
        "blockPatches": [],
    });
    expect_bad_request(
        &patch(&th.client, &role).await,
        "the minimum role of a board is `viewer`, `commenter` or `editor`",
    );

    th.teardown().await;
}

/// Rejects `PATCH /containers-and-blocks` when a block does not belong to the patched boards.
#[tokio::test]
async fn patch_boards_and_blocks_rejects_a_block_of_a_board_that_is_not_patched() {
    let th = Setup::native().start().await.init_basic().await;
    let boards = two_boards(&th, SecondBoard::WithAdmin, TEST_WORKSPACE_ID).await;
    let (mut board_patches, block_patches) = title_patches(2, "new patched title");
    board_patches.truncate(1);
    let request = PatchContainersAndBlocks {
        container_ids: strings(&[&boards.board1.id]),
        container_patches: board_patches,
        block_ids: strings(&[&boards.block1.id, &boards.block2.id]),
        block_patches,
    };

    let response = patch(&th.client, &request).await;

    response.check_bad_request();
    assert_titles_unchanged(&th, &boards).await;
    th.teardown().await;
}

/// Applies related board and block patches together when all are valid.
#[tokio::test]
async fn patch_boards_and_blocks_patches_should_be_applied_if_valid_and_related() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;
    let boards = two_boards(&th, SecondBoard::WithAdmin, TEST_WORKSPACE_ID).await;
    let new_title = "new other title";
    let (board_patches, block_patches) = title_patches(2, new_title);
    let request = PatchContainersAndBlocks {
        container_ids: strings(&[&boards.board1.id, &boards.board2.id]),
        container_patches: board_patches,
        block_ids: strings(&[&boards.block1.id, &boards.block2.id]),
        block_patches,
    };
    let mut socket =
        WorkspaceSocket::subscribe(&th, &th.client, TEST_WORKSPACE_ID, &user1.id).await;

    let patched: ContainersAndBlocks = patch(&th.client, &request).await.ok_json();

    assert_eq!(patched.containers.len(), 2, "two boards");
    assert_eq!(patched.blocks.len(), 2, "two blocks");
    assert_eq!(
        patched
            .containers
            .iter()
            .map(|board| board.id.as_str())
            .collect::<Vec<_>>(),
        vec![boards.board1.id.as_str(), boards.board2.id.as_str()],
        "in the order of the request"
    );

    // ensure that the entities have been updated
    for board in [&boards.board1, &boards.board2] {
        let stored = th.app.container(&board.id).await.expect("the board exists");
        assert_eq!(stored.title, new_title, "board {}", board.id);
        assert!(patched.containers.contains(&stored), "answered as stored");
    }
    for block in [&boards.block1, &boards.block2] {
        let stored = th
            .app
            .store()
            .get_block(&block.id)
            .await
            .expect("the block exists");
        assert_eq!(stored.title, new_title, "block {}", block.id);
        assert_eq!(stored.modified_by, user1.id, "modified by the user");
    }

    // The members are told about every block and board.
    for block in [&boards.block1, &boards.block2] {
        let message = socket.expect("blockChanged", "/block/id", &block.id).await;
        assert_eq!(message["block"]["title"], json!(new_title), "{message}");
    }
    for board in [&boards.board1, &boards.board2] {
        let message = socket
            .expect("containerChanged", "/container/id", &board.id)
            .await;
        assert_eq!(message["container"]["title"], json!(new_title), "{message}");
    }

    th.teardown().await;
}

/// A body of `PATCH /containers-and-blocks` that cannot be decoded is a bad request, not
/// a server error.
#[tokio::test]
async fn patch_boards_and_blocks_undecodable_bodies_are_bad_requests() {
    let th = Setup::native().start().await.init_basic().await;
    let boards = two_boards(&th, SecondBoard::WithAdmin, TEST_WORKSPACE_ID).await;
    let first_id = &boards.board1.id;

    let null_patch = format!("{{\"boardIds\":[\"{first_id}\"],\"boardPatches\":[null]}}");
    let number_type =
        format!("{{\"boardIds\":[\"{first_id}\"],\"boardPatches\":[{{\"type\":5}}]}}");
    for body in ["{bad", "null", "[]", &null_patch, &number_type] {
        let response = th.client.send_raw_body(Method::PATCH, PATH, body).await;
        response.check_bad_request();
        assert_eq!(response.problem_type(), "/problems/bad-request", "{body}");
    }

    // An empty object is decoded, and then found to list no boards.
    expect_bad_request(
        &patch(&th.client, &json!({})).await,
        "at least one board is required",
    );
    th.teardown().await;
}

/// Validation order and edge cases for `PATCH /containers-and-blocks`.
#[tokio::test]
async fn patch_boards_and_blocks_checks() {
    let th = Setup::native().start().await.init_basic().await;
    let user2 = th.client2.me().await;
    let boards = two_boards(&th, SecondBoard::WithAdmin, TEST_WORKSPACE_ID).await;
    let first_id = &boards.board1.id;

    // A board that does not exist, and a private board of somebody else, answer alike.
    let private = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    for board_id in [UNKNOWN_ID, "nonexistent", private.id.as_str()] {
        let response = patch(
            &th.client2,
            &json!({"containerIds": [board_id], "containerPatches": [{"title": "n"}]}),
        )
        .await;
        response.check_error(StatusCode::NOT_FOUND, "board not found");
    }

    // A block that does not exist.
    for block_id in [UNKNOWN_ID, "nonexistent"] {
        let response = patch(
            &th.client,
            &json!({
                "containerIds": [first_id], "containerPatches": [{"title": "n"}],
                "blockIds": [block_id], "blockPatches": [{"title": "n"}],
            }),
        )
        .await;
        expect_bad_request(
            &response,
            "the request names a block that is not on its boards",
        );
    }

    // An editor patches board properties and blocks, but not the board type.
    th.client
        .add_member(&member(first_id, &user2.id, ContainerRole::Editor))
        .await
        .check_ok();
    let response = patch(
        &th.client2,
        &json!({
            "containerIds": [first_id], "containerPatches": [{"visibility": "private"}],
            "blockIds": [boards.block1.id], "blockPatches": [{"title": "n"}],
        }),
    )
    .await;
    response.check_forbidden();
    assert_eq!(
        response.error_message(),
        "access denied to modifying board type",
        "why"
    );
    assert_titles_unchanged(&th, &boards).await;

    // Boards only, without block fields: the response returns an empty list of blocks.
    let response = patch(
        &th.client2,
        &json!({"containerIds": [first_id], "containerPatches": [{"description": "by the editor"}]}),
    )
    .await;
    let value = response.value();
    response.check_ok();
    assert_eq!(value["blocks"], json!([]), "no blocks");
    assert_eq!(
        value["containers"][0]["description"],
        json!("by the editor"),
        "{value}"
    );
    assert_eq!(
        value["containers"][0]["modifiedBy"],
        json!(user2.id),
        "{value}"
    );

    // The same board twice: both patches are applied, and the board is answered twice.
    let response = patch(
        &th.client,
        &json!({
            "containerIds": [first_id, first_id],
            "containerPatches": [{"title": "t1"}, {"icon": "i2"}],
        }),
    )
    .await;
    let value: Value = response.ok_json();
    assert_eq!(
        value["containers"][0]["icon"],
        json!(""),
        "after the first patch"
    );
    assert_eq!(value["containers"][1]["title"], json!("t1"), "after both");
    assert_eq!(value["containers"][1]["icon"], json!("i2"), "after both");

    // A commenter patches nothing.
    th.app
        .update_container_member(&member(first_id, &user2.id, ContainerRole::Commenter))
        .await
        .expect("user2 becomes a commenter");
    patch(
        &th.client2,
        &json!({"containerIds": [first_id], "containerPatches": [{"title": "n"}]}),
    )
    .await
    .check_forbidden();

    th.teardown().await;
}

// -- deleting boards and blocks together ----------------------------------------------

async fn delete(client: &Client, request: &impl serde::Serialize) -> ApiResponse {
    client.delete_json(PATH, request).await
}

/// Unauthenticated `DELETE /containers-and-blocks` is rejected.
#[tokio::test]
async fn delete_boards_and_blocks_a_non_authenticated_user_should_be_rejected() {
    let th = Setup::native().start().await;

    let response = delete(&th.client, &DeleteContainersAndBlocks::default()).await;

    response.check_unauthorized();
    th.teardown().await;
}

/// Rejects `DELETE /containers-and-blocks` when no board IDs are provided.
#[tokio::test]
async fn delete_boards_and_blocks_no_boards() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;
    // a board and a block are required for the permission checks
    let board = board_with_admin(
        &th,
        new_board(TEST_WORKSPACE_ID, ContainerVisibility::Open, ""),
        &user1.id,
    )
    .await;
    let block = stored_block(&th, &board.id, &user1.id).await;
    let request = DeleteContainersAndBlocks {
        containers: vec![],
        blocks: vec![block.id],
    };

    let response = delete(&th.client, &request).await;

    // That the request lists no board is found before anything is looked up, the block
    // against the listed boards included.
    expect_bad_request(&response, "at least one board is required");
    expect_bad_request(
        &delete(&th.client, &json!({})).await,
        "at least one board is required",
    );
    assert_eq!(blocks_of(&th, &board.id).await.len(), 1, "nothing deleted");
    th.teardown().await;
}

/// Rejects `DELETE /containers-and-blocks` across boards in different workspaces.
#[tokio::test]
async fn delete_boards_and_blocks_boards_from_different_workspaces() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;
    let board = board_with_admin(
        &th,
        new_board(TEST_WORKSPACE_ID, ContainerVisibility::Open, ""),
        &user1.id,
    )
    .await;
    let block = stored_block(&th, &board.id, &user1.id).await;
    let other_workspaces_board = board_with_admin(
        &th,
        new_board(ROOT_WORKSPACE_ID, ContainerVisibility::Open, ""),
        &user1.id,
    )
    .await;
    let request = DeleteContainersAndBlocks {
        containers: strings(&[&board.id, &other_workspaces_board.id]),
        blocks: vec![block.id],
    };

    let response = delete(&th.client, &request).await;

    // A board of another workspace is not there under this one's path.
    response.check_error(StatusCode::NOT_FOUND, "board not found");
    th.app
        .container(&board.id)
        .await
        .expect("the board is there");
    assert_eq!(blocks_of(&th, &board.id).await.len(), 1, "with its block");
    th.teardown().await;
}

/// Deletes nothing if the user lacks permission on any of the target boards.
#[tokio::test]
async fn delete_boards_and_blocks_without_permissions_to_one_board_nothing_is_deleted() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;
    // the user is an admin of the first board
    let board1 = board_with_admin(
        &th,
        new_board(TEST_WORKSPACE_ID, ContainerVisibility::Open, ""),
        &user1.id,
    )
    .await;
    // but not of the second
    let board2 = board_without_members(
        &th,
        new_board(TEST_WORKSPACE_ID, ContainerVisibility::Open, ""),
        &user1.id,
    )
    .await;
    // Use a real block of the first board so the missing permission is the only reason it fails.
    let block = stored_block(&th, &board1.id, &user1.id).await;
    let request = DeleteContainersAndBlocks {
        containers: strings(&[&board1.id, &board2.id]),
        blocks: vec![block.id],
    };

    let response = delete(&th.client, &request).await;

    response.check_forbidden();
    assert_eq!(
        response.error_message(),
        "access denied to delete board",
        "why"
    );
    th.app
        .container(&board1.id)
        .await
        .expect("board 1 is there");
    th.app
        .container(&board2.id)
        .await
        .expect("board 2 is there");
    assert_eq!(blocks_of(&th, &board1.id).await.len(), 1, "and the block");
    th.teardown().await;
}

/// Deletes all specified boards and blocks when the request is valid.
#[tokio::test]
async fn delete_boards_and_blocks_all_boards_and_blocks_should_be_deleted() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;
    // The application layer stores the boards and blocks under the identifiers it is
    // given.
    let board_ids = [new_id(), new_id()];
    let block_ids = [new_id(), new_id()];
    let request = ContainersAndBlocks {
        containers: vec![
            request_board(
                &board_ids[0],
                TEST_WORKSPACE_ID,
                ContainerVisibility::Open,
                "public board",
            ),
            request_board(
                &board_ids[1],
                TEST_WORKSPACE_ID,
                ContainerVisibility::Private,
                "private board",
            ),
        ],
        blocks: vec![
            request_block(&block_ids[0], &board_ids[0], "block 1"),
            request_block(&block_ids[1], &board_ids[1], "block 2"),
        ],
    };
    let created = th
        .app
        .create_containers_and_blocks_with_admin(request, &user1.id)
        .await
        .expect("the boards and blocks are created");
    assert_eq!(created.containers.len(), 2, "two boards");
    assert_eq!(created.blocks.len(), 2, "two blocks");

    // ensure that the entities have been successfully created
    for board_id in &board_ids {
        th.app.container(board_id).await.expect("the board exists");
    }
    for block_id in &block_ids {
        th.app
            .store()
            .get_block(block_id)
            .await
            .expect("the block exists");
    }
    let mut socket =
        WorkspaceSocket::subscribe(&th, &th.client, TEST_WORKSPACE_ID, &user1.id).await;

    // call the API to delete boards and blocks
    let response = delete(
        &th.client,
        &DeleteContainersAndBlocks {
            containers: board_ids.to_vec(),
            blocks: block_ids.to_vec(),
        },
    )
    .await;
    response.check_no_content();

    // ensure that the entities have been successfully deleted
    for board_id in &board_ids {
        let error = th
            .app
            .container(board_id)
            .await
            .expect_err("the board is gone");
        assert!(error.is_not_found(), "{error:?}");
    }
    for block_id in &block_ids {
        let error = th
            .app
            .store()
            .get_block(block_id)
            .await
            .expect_err("the block is gone");
        assert!(error.is_not_found(), "{error:?}");
    }

    // Those who were members are told about every deleted block and board, although
    // the memberships went with the boards.
    for block_id in &block_ids {
        let message = socket.expect("blockChanged", "/block/id", block_id).await;
        assert!(
            message["block"]["deletedAt"].as_i64() > Some(0),
            "a deletion: {message}"
        );
    }
    for board_id in &board_ids {
        let message = socket
            .expect("containerChanged", "/container/id", board_id)
            .await;
        assert!(
            message["container"]["deletedAt"].as_i64() > Some(0),
            "a deletion: {message}"
        );
    }

    th.teardown().await;
}

/// Validation order and edge cases for `DELETE /containers-and-blocks`.
#[tokio::test]
async fn delete_boards_and_blocks_checks() {
    let th = Setup::native().start().await.init_basic().await;
    let boards = two_boards(&th, SecondBoard::WithAdmin, TEST_WORKSPACE_ID).await;
    let first_id = &boards.board1.id;

    // A body that is no JSON object is a bad request.
    for body in ["{bad", "[]", "null"] {
        let response = th.client.send_raw_body(Method::DELETE, PATH, body).await;
        response.check_bad_request();
        assert_eq!(response.problem_type(), "/problems/bad-request", "{body}");
    }

    for nothing in [UNKNOWN_ID, "nonexistent"] {
        let response = delete(&th.client, &json!({"containers": [nothing]})).await;
        response.check_error(StatusCode::NOT_FOUND, "board not found");

        expect_bad_request(
            &delete(&th.client, &json!({"containers": [], "blocks": [nothing]})).await,
            "at least one board is required",
        );
        // A block that does not exist answers as one of a board that is not listed.
        expect_bad_request(
            &delete(
                &th.client,
                &json!({"containers": [first_id], "blocks": [nothing]}),
            )
            .await,
            "the request names a block that is not on its boards",
        );
    }

    // A block of a board that is not listed.
    expect_bad_request(
        &delete(
            &th.client,
            &json!({"containers": [first_id], "blocks": [boards.block2.id]}),
        )
        .await,
        "the request names a block that is not on its boards",
    );

    // Somebody who is not a member.
    delete(
        &th.client2,
        &json!({"containers": [first_id], "blocks": [boards.block1.id]}),
    )
    .await
    .check_forbidden();
    assert_titles_unchanged(&th, &boards).await;

    // Boards only: the board goes, with its blocks.
    delete(&th.client, &json!({"containers": [first_id]}))
        .await
        .check_no_content();
    let error = th
        .app
        .container(first_id)
        .await
        .expect_err("the board is gone");
    assert!(error.is_not_found(), "{error:?}");
    assert_eq!(blocks_of(&th, first_id).await, vec![], "and its block");
    th.app
        .container(&boards.board2.id)
        .await
        .expect("the other board is untouched");

    th.teardown().await;
}
