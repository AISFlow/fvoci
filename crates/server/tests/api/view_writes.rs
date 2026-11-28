//! Stale writes of views (`patchBoardView`): a patch whose `readAt` predates the view's
//! current `updatedAt` is rejected with 409 Conflict, while two editors moving different
//! views concurrently both succeed and two editors moving the same view from the same
//! `readAt` let exactly one win.

use std::sync::Arc;

use fvoci_model::{Block, ContainerVisibility};
use reqwest::StatusCode;
use serde_json::{Value, json};
use tokio::sync::Barrier;

use crate::board_views::{add_view, views_of};
use crate::common::{Setup, TEST_WORKSPACE_ID};

/// Creates an open board with `count` views, the second user an editor of it, and returns
/// `(board_id, views)`.
async fn board_with_views(ts: &crate::common::TestServer, count: usize) -> (String, Vec<Block>) {
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let user2_id = ts.client2.me().await.id;
    crate::rooms::give(ts, &board.id, &user2_id, "editor").await;
    let mut views = Vec::new();
    for i in 0..count {
        views.push(add_view(&ts.client, &board.id, &format!("View {i}")).await);
    }
    (board.id, views)
}

/// The path of the view `view_id` of `board_id`.
fn view_path(board_id: &str, view_id: &str) -> String {
    format!("{}/{view_id}", views_of(board_id))
}

/// Reads a single view back from the board.
async fn read_view(client: &crate::common::Client, board_id: &str, view_id: &str) -> Block {
    client.get(&view_path(board_id, view_id)).await.ok_json()
}

/// Patching a view with a `readAt` older than its current `updatedAt` returns 409 Conflict
/// and leaves the stored view untouched; retrying with the fresh `updatedAt` succeeds.
#[tokio::test]
async fn patching_a_view_with_a_stale_read_at_returns_409() {
    let ts = Setup::native().start().await.init_basic().await;
    let (board_id, views) = board_with_views(&ts, 1).await;
    let view = &views[0];
    let path = view_path(&board_id, &view.id);

    // First write advances updatedAt.
    let first = ts
        .client
        .patch(
            &path,
            &json!({
                "readAt": view.updated_at,
                "updatedFields": {"order": "W5"},
            }),
        )
        .await;
    first.check_ok();
    let after_first = read_view(&ts.client, &board_id, &view.id).await;
    assert_eq!(
        after_first.fields["order"],
        json!("W5"),
        "first write updated order"
    );
    assert!(
        after_first.updated_at > view.updated_at,
        "updatedAt advances: before={} after={}",
        view.updated_at,
        after_first.updated_at
    );

    // A stale write carrying the old readAt is rejected with 409.
    let stale = ts
        .client2
        .patch(
            &path,
            &json!({
                "readAt": view.updated_at,
                "updatedFields": {"order": "Z9"},
            }),
        )
        .await;
    stale.check_status(StatusCode::CONFLICT);
    assert_eq!(
        stale.value()["detail"],
        json!("the block has changed since it was read"),
        "{}",
        stale.text()
    );
    let unchanged = read_view(&ts.client, &board_id, &view.id).await;
    assert_eq!(
        unchanged.fields["order"],
        json!("W5"),
        "stale write left the view unchanged"
    );

    // Retrying with the fresh updatedAt succeeds.
    ts.client2
        .patch(
            &path,
            &json!({
                "readAt": unchanged.updated_at,
                "updatedFields": {"order": "X5"},
            }),
        )
        .await
        .check_ok();
    let updated = read_view(&ts.client, &board_id, &view.id).await;
    assert_eq!(
        updated.fields["order"],
        json!("X5"),
        "retry with fresh updatedAt persisted"
    );
    ts.teardown().await;
}

/// Two users moving two different views on the same board at the same moment both succeed,
/// whereas two users moving the same view from the same `readAt` at the same moment let
/// one succeed (200) and reject the other (409).
#[tokio::test]
async fn two_users_moving_views_concurrently_succeed_on_distinct_views_and_conflict_on_the_same() {
    let ts = Setup::native().start().await.init_basic().await;
    let (board_id, views) = board_with_views(&ts, 3).await;
    let (view_a, view_b, shared) = (&views[0], &views[1], &views[2]);

    // User 1 moves view_a while User 2 moves view_b at the same moment.
    let gate = Arc::new(Barrier::new(2));
    let (path_a, path_b) = (
        view_path(&board_id, &view_a.id),
        view_path(&board_id, &view_b.id),
    );
    let body_a = json!({"readAt": view_a.updated_at, "updatedFields": {"order": "V8"}});
    let body_b = json!({"readAt": view_b.updated_at, "updatedFields": {"order": "W8"}});
    let (res_a, res_b) = tokio::join!(
        async {
            gate.wait().await;
            ts.client.patch(&path_a, &body_a).await
        },
        async {
            gate.wait().await;
            ts.client2.patch(&path_b, &body_b).await
        },
    );
    res_a.check_ok();
    res_b.check_ok();
    assert_eq!(
        read_view(&ts.client, &board_id, &view_a.id).await.fields["order"],
        json!("V8"),
        "first user's move persisted"
    );
    assert_eq!(
        read_view(&ts.client, &board_id, &view_b.id).await.fields["order"],
        json!("W8"),
        "second user's move persisted"
    );

    // User 1 and User 2 move the same view concurrently with the same readAt.
    let gate = Arc::new(Barrier::new(2));
    let shared_path = view_path(&board_id, &shared.id);
    let move_1 = json!({"readAt": shared.updated_at, "updatedFields": {"order": "Y1"}});
    let move_2 = json!({"readAt": shared.updated_at, "updatedFields": {"order": "Y2"}});
    let (first, second) = tokio::join!(
        async {
            gate.wait().await;
            ts.client.patch(&shared_path, &move_1).await
        },
        async {
            gate.wait().await;
            ts.client2.patch(&shared_path, &move_2).await
        },
    );
    let mut statuses = [first.status.as_u16(), second.status.as_u16()];
    statuses.sort_unstable();
    assert_eq!(
        statuses,
        [200, 409],
        "one concurrent move wins and the other gets 409: first={} second={}",
        first.text(),
        second.text()
    );
    let winner_order: Value = if first.status.as_u16() == 200 {
        json!("Y1")
    } else {
        json!("Y2")
    };
    assert_eq!(
        read_view(&ts.client, &board_id, &shared.id).await.fields["order"],
        winner_order,
        "the winning move's order stands"
    );
    ts.teardown().await;
}
