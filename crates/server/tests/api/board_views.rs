//! The views of a board by their own routes: a board made with its first view, the
//! views listed, read, changed, deleted and restored, and none of it reaching a block
//! that is no view, or a view of another board.

use fvoci_model::{Block, BlockType, BoardViewWrite, Container, ContainerVisibility, new_id};
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::common::{Client, Setup, TEST_WORKSPACE_ID, TEST_WS, TestServer};

/// The path of the views of `board_id`.
pub(crate) fn views_of(board_id: &str) -> String {
    format!("{TEST_WS}/boards/{board_id}/views")
}

/// The views of `board_id` as `client` reads them.
pub(crate) async fn board_views(client: &Client, board_id: &str) -> Vec<Block> {
    client.get(&views_of(board_id)).await.ok_json()
}

/// A view of `board_id` called `title` that `client` adds last.
pub(crate) async fn add_view(client: &Client, board_id: &str, title: &str) -> Block {
    let write: BoardViewWrite = client
        .post(
            &views_of(board_id),
            &json!({"title": title, "viewType": "board"}),
        )
        .await
        .created_json();
    write.view
}

/// A board of the test workspace made by `client` with the first view `first_view`.
async fn board_starting_with(client: &Client, first_view: &Value) -> Container {
    client
        .post(
            &format!("{TEST_WS}/boards"),
            &json!({"title": "Viewed", "visibility": "open", "firstView": first_view}),
        )
        .await
        .created_json()
}

/// A board made with its first view has that view, first among its views, and none
/// other.
#[tokio::test]
async fn a_board_is_made_with_its_first_view() {
    let ts = Setup::native().start().await.init_basic().await;
    let board = board_starting_with(
        &ts.client,
        &json!({"title": "Plan", "viewType": "table", "fields": {"hiddenGroups": []}}),
    )
    .await;

    let views = board_views(&ts.client, &board.id).await;
    let [view] = views.as_slice() else {
        panic!("one view: {views:?}");
    };
    assert_eq!(
        (
            view.title.as_str(),
            &view.fields["viewType"],
            view.block_type
        ),
        ("Plan", &json!("table"), BlockType::View),
        "{view:?}"
    );
    assert!(
        view.fields["order"].is_string(),
        "it has its place: {view:?}"
    );
    assert_eq!(view.fields["hiddenGroups"], json!([]), "{view:?}");
    let second = add_view(&ts.client, &board.id, "Second").await;
    let titles: Vec<String> = board_views(&ts.client, &board.id)
        .await
        .into_iter()
        .map(|view| view.title)
        .collect();
    assert_eq!(titles, ["Plan", "Second"], "the first view first");
    assert_eq!(second.container_id, board.id);

    ts.teardown().await;
}

/// A first view that is refused leaves no board made, and is named under `/firstView`;
/// so is one with `scaffold`, and one that goes after another view.
#[tokio::test]
async fn a_board_whose_first_view_is_refused_is_not_made() {
    let ts = Setup::native().start().await.init_basic().await;
    let boards = format!("{TEST_WS}/boards");
    let before: Vec<Value> = ts.client.get(&boards).await.ok_json();

    for (first_view, scaffold, at) in [
        (
            json!({"title": "a\u{0}b", "viewType": "board"}),
            false,
            "/firstView/title",
        ),
        (
            json!({"title": "View", "viewType": "nope"}),
            false,
            "/firstView",
        ),
        (
            json!({"title": "View", "viewType": "board", "after": "x"}),
            false,
            "/firstView/after",
        ),
        (
            json!({"title": "View", "viewType": "board"}),
            true,
            "/firstView",
        ),
    ] {
        let response = ts
            .client
            .post(
                &boards,
                &json!({
                    "title": "Refused", "visibility": "open",
                    "firstView": first_view, "scaffold": scaffold,
                }),
            )
            .await;
        response.check_bad_request();
        assert_eq!(
            response.refused_at(),
            at,
            "{first_view}: {}",
            response.text()
        );
    }
    let after: Vec<Value> = ts.client.get(&boards).await.ok_json();
    assert_eq!(after.len(), before.len(), "no board was made");

    ts.teardown().await;
}

/// However a board starts (empty, scaffolded or with its first view), a request that
/// names the board's identifier is refused at `/id`, and a kind it may not make there is
/// refused at `/kind`, where the request names it: no board is made.
#[tokio::test]
async fn every_start_of_a_board_refuses_alike() {
    let ts = Setup::native().start().await.init_basic().await;
    let boards = format!("{TEST_WS}/boards");
    let before: Vec<Value> = ts.client.get(&boards).await.ok_json();
    let starts = [
        json!({}),
        json!({"scaffold": true}),
        json!({"firstView": {"title": "View", "viewType": "board"}}),
    ];
    for start in &starts {
        let asked = |member: Value| {
            let mut board = json!({"title": "Refused", "visibility": "private"});
            let object = board.as_object_mut().expect("an object");
            object.extend(start.as_object().expect("an object").clone());
            object.extend(member.as_object().expect("an object").clone());
            board
        };
        let response = ts
            .client
            .post(&boards, &asked(json!({"id": new_id()})))
            .await;
        response.check_error(StatusCode::BAD_REQUEST, "new board cannot have an ID");
        assert_eq!(response.refused_at(), "/id", "{start}: {}", response.text());

        let response = ts
            .client
            .post(&boards, &asked(json!({"kind": "wiki"})))
            .await;
        response.check_bad_request();
        assert_eq!(
            response.refused_at(),
            "/kind",
            "{start}: {}",
            response.text()
        );
    }
    let after: Vec<Value> = ts.client.get(&boards).await.ok_json();
    assert_eq!(after.len(), before.len(), "no board was made");

    ts.teardown().await;
}

/// A view keeps its type: a patch that takes `viewType` away, or sets it to null, is
/// refused at the member that names it, and the view stays as it was.
#[tokio::test]
async fn a_view_keeps_its_type() {
    let ts = Setup::native().start().await.init_basic().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let view = add_view(&ts.client, &board.id, "View").await;
    let path = format!("{}/{}", views_of(&board.id), view.id);
    let response = ts
        .client
        .patch(
            &path,
            &json!({"title": "Typeless", "deletedFields": ["filter", "viewType"]}),
        )
        .await;
    response.check_bad_request();
    assert_eq!(
        response.refused_at(),
        "/deletedFields/1",
        "{}",
        response.text()
    );
    let nulled = ts
        .client
        .patch(&path, &json!({"updatedFields": {"viewType": null}}))
        .await;
    nulled.check_bad_request();
    assert_eq!(
        nulled.refused_at(),
        "/updatedFields/viewType",
        "{}",
        nulled.text()
    );
    let read: Block = ts.client.get(&path).await.ok_json();
    assert_eq!(read, view, "nothing of the patch is written");

    ts.teardown().await;
}

/// A view is read, changed, deleted and restored by its board's path; a change from a
/// time it was read before it last changed is refused and changes nothing.
#[tokio::test]
async fn a_view_is_read_changed_deleted_and_restored() {
    let ts = Setup::native().start().await.init_basic().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let view = add_view(&ts.client, &board.id, "View").await;
    let path = format!("{}/{}", views_of(&board.id), view.id);
    let read: Block = ts.client.get(&path).await.ok_json();
    assert_eq!(read, view, "read as it was made");

    let patched: Block = ts
        .client
        .patch(
            &path,
            &json!({
                "title": "Renamed", "updatedFields": {"hiddenGroups": ["a"]},
                "deletedFields": ["order"], "readAt": view.updated_at,
            }),
        )
        .await
        .ok_json();
    assert_eq!(patched.title, "Renamed");
    assert_eq!(patched.fields["hiddenGroups"], json!(["a"]));
    assert!(!patched.fields.contains_key("order"), "{patched:?}");
    assert_eq!(patched.modified_by, ts.client.me().await.id);
    assert_eq!(ts.client.get(&path).await.ok_json::<Block>(), patched);
    let stale = ts
        .client
        .patch(&path, &json!({"title": "Stale", "readAt": view.updated_at}))
        .await;
    stale.check_status(StatusCode::CONFLICT);
    assert_eq!(ts.client.get(&path).await.ok_json::<Block>(), patched);

    ts.client.delete(&path).await.check_no_content();
    ts.client.get(&path).await.check_not_found();
    assert_eq!(
        board_views(&ts.client, &board.id).await,
        Vec::<Block>::new()
    );
    let restore = format!("{path}/restore");
    let restored: Block = ts.client.post_empty(&restore).await.ok_json();
    assert_eq!(
        (restored.id.as_str(), restored.title.as_str()),
        (view.id.as_str(), "Renamed")
    );
    assert_eq!(board_views(&ts.client, &board.id).await, [restored]);
    ts.client.post_empty(&restore).await.check_not_found();

    ts.teardown().await;
}

/// What a view's change refuses is named by the patch's own members, and a member a
/// view does not have is refused.
#[tokio::test]
async fn a_view_patch_names_what_it_refuses() {
    let ts = Setup::native().start().await.init_basic().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let view = add_view(&ts.client, &board.id, "View").await;
    let path = format!("{}/{}", views_of(&board.id), view.id);

    let response = ts.client.patch(&path, &json!({"title": "a\u{0}b"})).await;
    response.check_bad_request();
    assert_eq!(response.refused_at(), "/title", "{}", response.text());
    let response = ts
        .client
        .patch(&path, &json!({"updatedFields": {"viewType": "nope"}}))
        .await;
    response.check_bad_request();
    assert!(
        response.refused_at().starts_with("/updatedFields"),
        "{}",
        response.text()
    );
    for member in [json!({"parentId": "x"}), json!({"body": {"type": "doc"}})] {
        ts.client.patch(&path, &member).await.check_bad_request();
    }
    let read: Block = ts.client.get(&path).await.ok_json();
    assert_eq!(read, view, "nothing changed");

    ts.teardown().await;
}

/// The view routes reach no block of the board that is no view, and no view of another
/// board through this board's path.
#[tokio::test]
async fn the_view_routes_reach_only_the_views_of_their_board() {
    let ts = Setup::native().start().await.init_basic().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let other = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let card: Value = ts
        .client
        .post(
            &format!("{TEST_WS}/boards/{}/cards", board.id),
            &json!({"title": "A card"}),
        )
        .await
        .created_json();
    let card_id = card["id"].as_str().expect("its id");
    let elsewhere = add_view(&ts.client, &other.id, "Elsewhere").await;

    for block_id in [card_id, elsewhere.id.as_str(), "no-such-view"] {
        check_unreached(&ts, &board.id, block_id).await;
    }
    // A card in the trash is no view to restore.
    ts.client
        .post(
            &format!("{TEST_WS}/boards/{}/cards/batch-delete", board.id),
            &json!({"cardIds": [card_id]}),
        )
        .await
        .check_no_content();
    ts.client
        .post_empty(&format!("{}/{card_id}/restore", views_of(&board.id)))
        .await
        .check_not_found();
    // Nor is a view of another board.
    let other_path = format!("{}/{}", views_of(&other.id), elsewhere.id);
    ts.client.delete(&other_path).await.check_no_content();
    ts.client
        .post_empty(&format!("{}/{}/restore", views_of(&board.id), elsewhere.id))
        .await
        .check_not_found();
    assert_eq!(
        board_views(&ts.client, &other.id).await,
        Vec::<Block>::new()
    );

    ts.teardown().await;
}

/// Each view route of `board_id` answers 404 for `block_id`, and changes nothing.
async fn check_unreached(ts: &TestServer, board_id: &str, block_id: &str) {
    let path = format!("{}/{block_id}", views_of(board_id));
    let client = &ts.client;
    for response in [
        client.get(&path).await,
        client.patch(&path, &json!({"title": "Changed"})).await,
        client.delete(&path).await,
        client.post_empty(&format!("{path}/restore")).await,
    ] {
        response.check_not_found();
        assert_eq!(response.error_message(), "view not found", "{block_id}");
    }
    let blocks = ts
        .app
        .blocks(board_id, None, None)
        .await
        .expect("the board's blocks");
    assert!(
        blocks.iter().all(|block| block.title != "Changed"),
        "{blocks:?}"
    );
}

/// A personal API token that reads lists and reads the views of a board; the views are
/// changed only in a session.
#[tokio::test]
async fn a_token_reads_the_views() {
    let ts = Setup::native().start().await.init_basic().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let view = add_view(&ts.client, &board.id, "View").await;
    let agent = crate::api_tokens::agent_writing(&ts, &ts.client).await;
    assert_eq!(
        board_views(&agent, &board.id).await,
        std::slice::from_ref(&view)
    );
    let path = format!("{}/{}", views_of(&board.id), view.id);
    assert_eq!(agent.get(&path).await.ok_json::<Block>(), view);
    let refused = agent.patch(&path, &json!({"title": "By the agent"})).await;
    assert!(
        matches!(
            refused.status,
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
        ),
        "{}: {}",
        refused.status,
        refused.text()
    );

    ts.teardown().await;
}
