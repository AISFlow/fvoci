//! Integration, regression, and 50×200 open-board delivery tests for `server-authority`:
//! - Atomic `duplicateCard` pre-copy rollback on storage copy failure (`0` orphan/original-file blocks)
//! - Caller-scoped `my-permissions` isolation across `containerChanged`, `containerMemberChanged`,
//!   and `workspaceRoleChanged`
//! - Immediate Collab room and WebSocket eviction when an open board switches to `private`
//! - Fractional-indexed board views, multi-card `card-moves`, `batch-delete`/`batch-restore`,
//!   formal card comments, verified card attachments, cross-workspace copy (`allowCopyOut`,
//!   external `[[` & `@mention` rewriting, `container_copied_out` audit), and `'Today'` hub
//! - 50 open boards × 200 WebSocket connections card-edit broadcast benchmark

use std::time::{Duration, Instant};

use fvoci_model::{Container, ContainerVisibility, OrgKind, UserWorkspace, WorkspaceRole};
use fvoci_server::app::tend_notifications;
use fvoci_server::storage::FaultyOp;
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::board_broadcasts::WorkspaceSocket;
use crate::common::{Setup, TEST_WORKSPACE_ID, TEST_WS, TestServer};
use crate::files::{upload, uploaded_id};
use crate::org_admin::org_of;
use crate::rooms::{Incoming, Peer, QUIET, paragraphs};
use crate::rooms_changes::seldom;

fn text_doc(text: &str) -> Value {
    paragraphs(&[text])
}

/// 1) Mandatory regression test: if storage `Copy` fails while duplicating a card with
/// an attached file and an embedded file block, `POST .../cards/{cardId}/duplicate`
/// returns 500, leaves **zero** cloned blocks referencing the source card's `fileId` (and
/// zero extra cards on the board), and leaves no unsettled `Writing` loose objects.
#[tokio::test]
async fn atomic_card_duplicate_rolls_back_when_file_copy_fails() {
    let ts = Setup::native().start().await.init_basic().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let up = upload(&ts.client, &board.id, "spec.txt", b"important bytes").await;
    up.check_created();
    let orig_file_id = uploaded_id(&up);

    let up2 = upload(&ts.client, &board.id, "attach.txt", b"attachment bytes").await;
    up2.check_created();
    let second_file_id = uploaded_id(&up2);

    let body = json!({
        "type": "doc",
        "content": [{
            "type": "file",
            "attrs": { "fileId": orig_file_id }
        }]
    });
    let card: Value = ts
        .client
        .post(
            &format!("{TEST_WS}/boards/{}/cards", board.id),
            &json!({"title": "Original", "body": body}),
        )
        .await
        .created_json();
    let card_id = card["id"].as_str().expect("card id");
    attach_named(&ts, &board.id, card_id, &second_file_id).await;

    // First verify happy-path atomic duplication remaps both body fileId and attachment fileId.
    let copy_ok: Value = ts
        .client
        .post(
            &format!("{TEST_WS}/boards/{}/cards/{card_id}/duplicate", board.id),
            &json!({}),
        )
        .await
        .created_json();
    let copy_id = copy_ok["id"].as_str().expect("copied card id");
    let copy_detail: Value = ts
        .client
        .get(&format!("{TEST_WS}/cards/{copy_id}"))
        .await
        .ok_json();
    let copied_body_file = copy_detail["body"]["content"][0]["attrs"]["fileId"]
        .as_str()
        .expect("copied body fileId");
    assert_ne!(
        copied_body_file, orig_file_id,
        "copied body uses new fileId"
    );

    // Inject storage Copy failure on the second file copy (attachment after body file).
    let faults = ts.app.files().faults().expect("faults enabled");
    faults.fail_after(FaultyOp::Copy, 1, 1);

    let cards_before: Vec<Value> = ts
        .client
        .get(&format!("{TEST_WS}/boards/{}/cards", board.id))
        .await
        .ok_json();
    let failed = ts
        .client
        .post(
            &format!("{TEST_WS}/boards/{}/cards/{card_id}/duplicate", board.id),
            &json!({}),
        )
        .await;
    assert_eq!(
        failed.status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "storage copy failure aborts duplicateCard with 500"
    );

    let cards_after: Vec<Value> = ts
        .client
        .get(&format!("{TEST_WS}/boards/{}/cards", board.id))
        .await
        .ok_json();
    assert_eq!(
        cards_after.len(),
        cards_before.len(),
        "no partial duplicate card committed"
    );

    assert_zero_cloned_refs_to_original(&ts, &board.id, card_id, &orig_file_id).await;
    ts.teardown().await;
}

/// Attaches the file to the card, under the file's name, and checks that an attachment
/// given a title of its own (an edit kept apart) is shown under it; that one goes again.
async fn attach_named(ts: &TestServer, board_id: &str, card_id: &str, file_id: &str) {
    let attachments = format!("{TEST_WS}/boards/{board_id}/cards/{card_id}/attachments");
    let attached: Value = ts
        .client
        .post(&attachments, &json!({"fileId": file_id}))
        .await
        .created_json();
    assert_eq!(attached["title"], "attach.txt", "named as its file");
    let apart: Value = ts
        .client
        .post(
            &attachments,
            &json!({"fileId": file_id, "title": "attach (edited).txt"}),
        )
        .await
        .created_json();
    assert_eq!(
        apart["title"], "attach (edited).txt",
        "an edit kept apart is shown under the name its uploader gave it"
    );
    let apart_id = apart["id"].as_str().expect("its id");
    ts.client
        .delete(&format!("{attachments}/{apart_id}"))
        .await
        .check_no_content();
}

async fn assert_zero_cloned_refs_to_original(
    ts: &TestServer,
    board_id: &str,
    card_id: &str,
    orig_file_id: &str,
) {
    let all_blocks: Vec<Value> = crate::blocks::blocks_of(&ts.client, board_id)
        .await
        .iter()
        .map(|block| serde_json::to_value(block).expect("a block as JSON"))
        .collect();
    let refs_to_orig: Vec<&Value> = all_blocks
        .iter()
        .filter(|b| {
            let id = b["id"].as_str().unwrap_or_default();
            let parent = b["parentId"].as_str().unwrap_or_default();
            id != card_id && parent != card_id && b.to_string().contains(orig_file_id)
        })
        .collect();
    assert!(
        refs_to_orig.is_empty(),
        "0 cloned blocks may reference original fileId: {refs_to_orig:?}"
    );
}

/// 2) Mandatory regression test: `Container` JSON never embeds `myPermissions`, and two
/// connections with different roles (`admin` vs `viewer` vs unjoined `member`) retain
/// their own `my-permissions` across `containerChanged`, `containerMemberChanged`, and
/// `workspaceRoleChanged`.
#[tokio::test]
async fn caller_scoped_my_permissions_never_bleed_across_websocket_broadcasts() {
    let ts = Setup::native().start().await.init_basic().await;
    let admin_id = ts.client.me().await.id;
    let viewer_id = ts.client2.me().await.id;
    let unjoined_client = ts.add_user("unjoined.reader").await;
    let unjoined_id = unjoined_client.me().await.id;

    let board: Container = ts
        .client
        .post(
            &format!("{TEST_WS}/boards"),
            &json!({"visibility": "open", "title": "Shared Open"}),
        )
        .await
        .created_json();
    ts.client
        .post(
            &format!("{TEST_WS}/containers/{}/members", board.id),
            &json!({"userId": viewer_id, "role": "viewer"}),
        )
        .await
        .check_ok();

    let mut admin_ws =
        WorkspaceSocket::subscribe(&ts, &ts.client, TEST_WORKSPACE_ID, &admin_id).await;
    let mut viewer_ws =
        WorkspaceSocket::subscribe(&ts, &ts.client2, TEST_WORKSPACE_ID, &viewer_id).await;
    let mut unjoined_ws =
        WorkspaceSocket::subscribe(&ts, &unjoined_client, TEST_WORKSPACE_ID, &unjoined_id).await;

    let raw_container: Value = ts
        .client
        .get(&format!("{TEST_WS}/containers/{}", board.id))
        .await
        .ok_json();
    assert!(
        raw_container.get("myPermissions").is_none(),
        "Container struct must never carry myPermissions"
    );

    let perms_path = format!("{TEST_WS}/containers/{}/my-permissions", board.id);
    assert_initial_permissions(&ts, &unjoined_client, &perms_path).await;

    // Admin patches board title -> broadcasts containerChanged to all three sockets.
    ts.client
        .patch(
            &format!("{TEST_WS}/containers/{}", board.id),
            &json!({"title": "Updated Title"}),
        )
        .await
        .check_ok();
    for sock in [&mut admin_ws, &mut viewer_ws, &mut unjoined_ws] {
        let msg = sock
            .expect("containerChanged", "/container/id", &board.id)
            .await;
        assert!(
            msg["container"].get("myPermissions").is_none(),
            "containerChanged payload must not embed caller permissions: {msg}"
        );
    }
    let viewer_after: Value = ts.client2.get(&perms_path).await.ok_json();
    let unjoined_after: Value = unjoined_client.get(&perms_path).await.ok_json();
    assert_eq!(
        viewer_after["canEdit"],
        json!(false),
        "viewer stays read-only"
    );
    assert_eq!(
        unjoined_after["canEdit"],
        json!(false),
        "unjoined stays read-only"
    );

    // Promote viewer to editor -> containerMemberChanged + updated my-permissions.
    ts.client
        .put(
            &format!("{TEST_WS}/containers/{}/members/{viewer_id}", board.id),
            &json!({"role": "editor"}),
        )
        .await
        .check_ok();
    viewer_ws
        .expect("containerMemberChanged", "/member/userId", &viewer_id)
        .await;
    let promoted: Value = ts.client2.get(&perms_path).await.ok_json();
    assert_eq!(promoted["canEdit"], json!(true), "promoted editor can edit");

    verify_workspace_role_promotion_updates_permissions(
        &ts,
        &admin_id,
        &unjoined_id,
        &unjoined_client,
        &mut unjoined_ws,
        &perms_path,
    )
    .await;
    ts.teardown().await;
}

async fn assert_initial_permissions(
    ts: &TestServer,
    unjoined_client: &crate::common::Client,
    perms_path: &str,
) {
    let admin_p: Value = ts.client.get(perms_path).await.ok_json();
    let viewer_p: Value = ts.client2.get(perms_path).await.ok_json();
    let unjoined_p: Value = unjoined_client.get(perms_path).await.ok_json();
    assert_eq!(admin_p["canEdit"], json!(true), "admin can edit");
    assert_eq!(
        admin_p["canManageRoles"],
        json!(true),
        "admin manages roles"
    );
    assert_eq!(viewer_p["canEdit"], json!(false), "viewer cannot edit");
    assert_eq!(
        viewer_p["canComment"],
        json!(false),
        "viewer cannot comment"
    );
    assert_eq!(unjoined_p["canEdit"], json!(false), "unjoined cannot edit");
    assert_eq!(
        unjoined_p["canComment"],
        json!(false),
        "unjoined cannot comment"
    );
}

async fn verify_workspace_role_promotion_updates_permissions(
    ts: &TestServer,
    admin_id: &str,
    unjoined_id: &str,
    unjoined_client: &crate::common::Client,
    unjoined_ws: &mut WorkspaceSocket,
    perms_path: &str,
) {
    ts.app
        .store()
        .execute_unchecked(&format!(
            "UPDATE workspace_members SET role = '{}' WHERE CAST(user_id AS TEXT) = '{admin_id}' \
             AND CAST(workspace_id AS TEXT) = '{TEST_WORKSPACE_ID}'",
            WorkspaceRole::Admin.as_str()
        ))
        .await
        .expect("grant test workspace admin");
    ts.client
        .put(
            &format!("{TEST_WS}/members/{unjoined_id}"),
            &json!({"role": "admin"}),
        )
        .await
        .check_ok();
    let role_msg = unjoined_ws
        .expect("workspaceRoleChanged", "/userId", unjoined_id)
        .await;
    assert_eq!(
        role_msg["role"],
        json!("admin"),
        "workspaceRoleChanged carries admin"
    );
    let ws_admin_p: Value = unjoined_client.get(perms_path).await.ok_json();
    assert_eq!(
        ws_admin_p["canManageRoles"],
        json!(true),
        "workspace admin manages roles"
    );
}

/// 3) Mandatory regression test: an unjoined workspace member reading an open board and
/// its card Collab room is immediately evicted (`docLeft` + `containerChanged` deletion
/// tombstone + zero subsequent `blockChanged`) the instant the board switches to `private`.
#[tokio::test]
async fn open_board_switched_to_private_immediately_evicts_unjoined_readers() {
    let ts = seldom().start().await.init_basic().await;
    let reader_id = ts.client2.me().await.id;
    let board: Container = ts
        .client
        .post(
            &format!("{TEST_WS}/boards"),
            &json!({"visibility": "open", "title": "Open Roadmap"}),
        )
        .await
        .created_json();
    let card: Value = ts
        .client
        .post(
            &format!("{TEST_WS}/boards/{}/cards", board.id),
            &json!({"title": "Card 1", "body": text_doc("visible while open")}),
        )
        .await
        .created_json();
    let card_id = card["id"].as_str().expect("card id").to_owned();

    let mut reader_ws =
        WorkspaceSocket::subscribe(&ts, &ts.client2, TEST_WORKSPACE_ID, &reader_id).await;
    let mut reader_room = Peer::connect(&ts.client2, 2).await;
    let joined = reader_room.join(TEST_WORKSPACE_ID, &card_id).await;
    assert_eq!(joined["type"], json!("docJoined"), "{joined}");
    assert_eq!(joined["mode"], json!("read"), "open board joins read-only");

    // Switch open board to private -> immediate Collab room eviction + WS tombstone!
    ts.client
        .patch(
            &format!("{TEST_WS}/containers/{}", board.id),
            &json!({"visibility": "private"}),
        )
        .await
        .check_ok();

    let left = reader_room.text("docLeft").await;
    assert_eq!(left["reason"], json!("not_found"), "{left}");

    let gone = reader_ws
        .expect("containerChanged", "/container/id", &board.id)
        .await;
    assert!(
        gone["container"]["deletedAt"].is_number(),
        "unjoined reader receives deletion tombstone on open->private: {gone}"
    );

    // Subsequent card edits must NOT leak to the evicted reader over WS or Collab.
    ts.client
        .patch(
            &format!("{TEST_WS}/cards/{card_id}"),
            &json!({"title": "Top Secret"}),
        )
        .await
        .check_ok();
    assert_eq!(
        reader_room.incoming(QUIET).await,
        Incoming::Nothing,
        "evicted room receives nothing"
    );
    let leaked = reader_ws.drain().await;
    assert!(
        !leaked.iter().any(|m| m["type"] == json!("blockChanged")),
        "no blockChanged may leak after board becomes private: {leaked:?}"
    );
    ts.teardown().await;
}

/// Exercises fractional-indexed views, `card-moves` (validation, WS order, and assignee
/// notification), `batch-delete` / `batch-restore`, formal card comments, and verified
/// card attachments.
#[tokio::test]
async fn board_views_card_moves_batch_ops_comments_and_attachments() {
    let ts = Setup::native().start().await.init_basic().await;
    let admin_id = ts.client.me().await.id;
    let member_id = ts.client2.me().await.id;
    let board: Container = ts
        .client
        .post(
            &format!("{TEST_WS}/boards"),
            &json!({"visibility": "open", "title": "Ops", "scaffold": true}),
        )
        .await
        .created_json();
    ts.client
        .post(
            &format!("{TEST_WS}/containers/{}/members", board.id),
            &json!({"userId": member_id, "role": "editor"}),
        )
        .await
        .check_ok();

    let assignee_prop = board
        .card_properties
        .iter()
        .find(|p| p.property_type == fvoci_model::PropertyType::Person)
        .expect("scaffold person property")
        .id
        .clone();

    let v_id = verify_board_view_endpoints(&ts, &board.id).await;

    let c1: Value = ts
        .client
        .post(
            &format!("{TEST_WS}/boards/{}/cards", board.id),
            &json!({"title": "One"}),
        )
        .await
        .created_json();
    let c2: Value = ts
        .client
        .post(
            &format!("{TEST_WS}/boards/{}/cards", board.id),
            &json!({"title": "Two"}),
        )
        .await
        .created_json();
    let (c1_id, c2_id) = (
        c1["id"].as_str().expect("c1").to_owned(),
        c2["id"].as_str().expect("c2").to_owned(),
    );

    let mut member_ws =
        WorkspaceSocket::subscribe(&ts, &ts.client2, TEST_WORKSPACE_ID, &member_id).await;
    let _ = member_ws.drain().await;

    // Duplicate cardIds -> 400.
    let dup_bad = ts
        .client
        .post(
            &format!("{TEST_WS}/boards/{}/views/{v_id}/card-moves", board.id),
            &json!({"cardIds": [c1_id, c1_id]}),
        )
        .await;
    assert_eq!(
        dup_bad.status,
        StatusCode::BAD_REQUEST,
        "duplicate cardIds rejected with 400"
    );

    // Valid card-moves with person assignment -> WS sends view BEFORE cards, plus notification!
    let moved: Value = ts
        .client
        .post(
            &format!("{TEST_WS}/boards/{}/views/{v_id}/card-moves", board.id),
            &json!({
                "cardIds": [c2_id, c1_id],
                "afterCardId": "",
                "groupPropertyId": assignee_prop,
                "groupValue": member_id,
            }),
        )
        .await
        .ok_json();
    assert_eq!(
        moved["cards"].as_array().map(Vec::len),
        Some(2),
        "both moved cards returned"
    );
    let events = member_ws.drain().await;
    let block_events: Vec<&str> = events
        .iter()
        .filter(|m| m["type"] == json!("blockChanged"))
        .filter_map(|m| m["block"]["type"].as_str())
        .collect();
    assert_eq!(
        block_events.first().copied(),
        Some("view"),
        "view blockChanged must precede card blockChanged: {block_events:?}"
    );
    tend_notifications(&ts.app).await;
    let unread: Value = ts.client2.get("/notifications/unread").await.ok_json();
    assert!(
        unread["count"].as_i64().unwrap_or(0) >= 1,
        "assignee notified"
    );

    verify_batch_delete_and_restore(&ts, &board.id, &c1_id, &c2_id).await;
    verify_card_comments_and_attachments(&ts, &board.id, &c1_id, &admin_id).await;
    ts.teardown().await;
}

async fn verify_batch_delete_and_restore(
    ts: &TestServer,
    board_id: &str,
    c1_id: &str,
    c2_id: &str,
) {
    ts.client
        .post(
            &format!("{TEST_WS}/boards/{board_id}/cards/batch-delete"),
            &json!({"cardIds": [c1_id, c2_id]}),
        )
        .await
        .check_no_content();
    let restored: Value = ts
        .client
        .post(
            &format!("{TEST_WS}/boards/{board_id}/cards/batch-restore"),
            &json!({"cardIds": [c1_id, c2_id]}),
        )
        .await
        .ok_json();
    assert_eq!(
        restored["cards"].as_array().map(Vec::len),
        Some(2),
        "both deleted cards restored"
    );
}

async fn verify_board_view_endpoints(ts: &TestServer, board_id: &str) -> String {
    let v_created: Value = ts
        .client
        .post(
            &format!("{TEST_WS}/boards/{board_id}/views"),
            &json!({"title": "Table", "viewType": "table", "fields": {"viewType": "table"}}),
        )
        .await
        .created_json();
    let v_id = v_created["view"]["id"]
        .as_str()
        .expect("view id")
        .to_owned();
    let v_dup: Value = ts
        .client
        .post(
            &format!("{TEST_WS}/boards/{board_id}/views/{v_id}/duplicate"),
            &json!({"title": "Table Copy", "after": v_id}),
        )
        .await
        .created_json();
    let dup_id = v_dup["view"]["id"].as_str().expect("dup view id");
    ts.client
        .post(
            &format!("{TEST_WS}/boards/{board_id}/views/{dup_id}/move"),
            &json!({"after": ""}),
        )
        .await
        .check_ok();
    v_id
}

async fn verify_card_comments_and_attachments(
    ts: &TestServer,
    board_id: &str,
    card_id: &str,
    _admin_id: &str,
) {
    let comments_url = format!("{TEST_WS}/boards/{board_id}/cards/{card_id}/comments");
    let created: Value = ts
        .client2
        .post(&comments_url, &json!({"body": text_doc("hello card")}))
        .await
        .created_json();
    let comment_id = created["id"].as_str().expect("comment id");
    let patched: Value = ts
        .client2
        .patch(
            &format!("{comments_url}/{comment_id}"),
            &json!({"body": text_doc("edited card"), "readAt": created["updatedAt"]}),
        )
        .await
        .ok_json();
    assert!(
        patched["editedAt"].as_i64().unwrap_or(0) > 0,
        "editedAt stamped on comment edit"
    );
    let versions: Vec<Value> = ts
        .client2
        .get(&format!("{comments_url}/{comment_id}/versions"))
        .await
        .ok_json();
    assert_eq!(versions.len(), 1, "previous comment version stored");
    ts.client2
        .delete(&format!("{comments_url}/{comment_id}"))
        .await
        .check_no_content();
    ts.client2
        .post(&format!("{comments_url}/{comment_id}/restore"), &json!({}))
        .await
        .check_ok();

    // Verified attachment rejects file uploaded to a different board.
    let other_board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let foreign_up = upload(&ts.client, &other_board.id, "other.txt", b"foreign").await;
    let foreign_file = uploaded_id(&foreign_up);
    let rejected = ts
        .client
        .post(
            &format!("{TEST_WS}/boards/{board_id}/cards/{card_id}/attachments"),
            &json!({"fileId": foreign_file}),
        )
        .await;
    assert_eq!(
        rejected.status,
        StatusCode::BAD_REQUEST,
        "foreign board fileId rejected"
    );
}

/// 4) Cross-workspace container copy (`allowCopyOut`, non-member 403, external `[[` and
/// absent `@mention` sanitization, `container_copied_out` audit log) and `'Today'` hub.
#[tokio::test]
async fn cross_workspace_copy_and_today_hub_enforce_boundaries() {
    let ts = Setup::local_mode().start().await.init_basic().await;
    let user2_id = ts.client2.me().await.id;
    let workspaces: Vec<UserWorkspace> = ts.client.get("/users/me/workspaces").await.ok_json();
    let personal_ws = workspaces
        .iter()
        .find(|w| w.kind == OrgKind::Personal)
        .expect("personal workspace")
        .id
        .clone();

    let other_board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let external_card: Value = ts
        .client
        .post(
            &format!("{TEST_WS}/boards/{}/cards", other_board.id),
            &json!({"title": "External Target"}),
        )
        .await
        .created_json();
    let ext_id = external_card["id"].as_str().expect("ext id");

    let source_board: Container = ts
        .client
        .post(
            &format!("{TEST_WS}/boards"),
            &json!({"visibility": "open", "title": "Product Specs", "scaffold": true}),
        )
        .await
        .created_json();
    let rich_body = json!({
        "type": "doc",
        "content": [{
            "type": "paragraph",
            "content": [
                {"type": "reference", "attrs": {"id": ext_id}},
                {"type": "text", "text": " "},
                {"type": "mention", "attrs": {"userId": user2_id}}
            ]
        }]
    });
    let today_iso = "2026-01-01";
    let due_prop = source_board.due_date_property_id.clone().expect("due prop");
    let assignee_prop = source_board
        .card_properties
        .iter()
        .find(|p| p.property_type == fvoci_model::PropertyType::Person)
        .expect("person prop")
        .id
        .clone();
    let admin_id = ts.client.me().await.id;
    ts.client
        .post(
            &format!("{TEST_WS}/boards/{}/cards", source_board.id),
            &json!({
                "title": "Due Task",
                "body": rich_body,
                "properties": {
                    due_prop: json!({"from": today_iso}),
                    assignee_prop: admin_id,
                }
            }),
        )
        .await
        .check_created();

    // Non-member on open board cannot copy-to-workspace -> 403!
    let user2_wss: Vec<UserWorkspace> = ts.client2.get("/users/me/workspaces").await.ok_json();
    let user2_personal = user2_wss
        .iter()
        .find(|w| w.kind == OrgKind::Personal)
        .expect("user2 personal")
        .id
        .clone();
    let non_member_copy = ts
        .client2
        .post(
            &format!("{TEST_WS}/containers/{}/copy-to-workspace", source_board.id),
            &json!({"targetWorkspaceId": user2_personal}),
        )
        .await;
    assert_eq!(
        non_member_copy.status,
        StatusCode::FORBIDDEN,
        "non-member on open board cannot copy-to-workspace"
    );

    assert_copied_card_sanitized(&ts, &source_board.id, &personal_ws).await;
    verify_copy_out_audit_disable_and_today(&ts, &source_board.id, &personal_ws).await;
    ts.teardown().await;
}

async fn assert_copied_card_sanitized(ts: &TestServer, source_board_id: &str, personal_ws: &str) {
    let copied: Value = ts
        .client
        .post(
            &format!("{TEST_WS}/containers/{source_board_id}/copy-to-workspace"),
            &json!({"targetWorkspaceId": personal_ws, "title": "My Personal Copy"}),
        )
        .await
        .created_json();
    let copied_board_id = copied["container"]["id"].as_str().expect("copied id");
    let copied_cards: Vec<Value> = ts
        .client
        .get(&format!("/w/{personal_ws}/boards/{copied_board_id}/cards"))
        .await
        .ok_json();
    let copied_card_id = copied_cards[0]["id"].as_str().expect("copied card id");
    let copied_detail: Value = ts
        .client
        .get(&format!("/w/{personal_ws}/cards/{copied_card_id}"))
        .await
        .ok_json();
    let body_str = copied_detail["body"].to_string();
    assert!(
        !body_str.contains("\"reference\"") && !body_str.contains("\"mention\""),
        "external reference and non-target mention must be sanitized to text: {body_str}"
    );
}

async fn verify_copy_out_audit_disable_and_today(
    ts: &TestServer,
    source_board_id: &str,
    personal_ws: &str,
) {
    let org_id = org_of(ts).await;
    let audit: Value = ts
        .client
        .get(&format!("/org/{org_id}/audit"))
        .await
        .ok_json();
    assert!(
        audit.to_string().contains("container_copied_out"),
        "audit log records container_copied_out: {audit}"
    );
    ts.client
        .patch(&format!("/org/{org_id}"), &json!({"allowCopyOut": false}))
        .await
        .check_ok();
    let disabled = ts
        .client
        .post(
            &format!("{TEST_WS}/containers/{source_board_id}/copy-to-workspace"),
            &json!({"targetWorkspaceId": personal_ws}),
        )
        .await;
    assert_eq!(
        disabled.status,
        StatusCode::FORBIDDEN,
        "allowCopyOut=false forbids cross-org copy"
    );
    assert_eq!(
        disabled.problem().1["reason"],
        json!("copy_out_disabled"),
        "reason is copy_out_disabled"
    );

    let me_today: Value = ts.client.get("/users/me/today").await.ok_json();
    assert_eq!(
        me_today["lockedWorkspaceCount"],
        json!(0),
        "no locked workspaces"
    );
    assert!(
        me_today["cards"]
            .as_array()
            .is_some_and(|cards| cards.len() >= 2),
        "today hub includes overdue cards from shared and personal workspaces: {me_today}"
    );
}

/// 5) Benchmark: 50 open boards × 200 concurrent WebSocket connections in one workspace.
/// Measures single-card-edit fanout latency across all 200 connections.
#[tokio::test]
async fn open_board_50_boards_200_connections_delivery_benchmark() {
    let ts = Setup::native().start().await.init_basic().await;
    let user2_id = ts.client2.me().await.id;

    let mut target_card_id = String::new();
    for idx in 0..50 {
        let board: Container = ts
            .client
            .post(
                &format!("{TEST_WS}/boards"),
                &json!({"visibility": "open", "title": format!("Open {idx}")}),
            )
            .await
            .created_json();
        let card: Value = ts
            .client
            .post(
                &format!("{TEST_WS}/boards/{}/cards", board.id),
                &json!({"title": format!("Card {idx}")}),
            )
            .await
            .created_json();
        if idx == 25 {
            target_card_id = card["id"].as_str().expect("card id").to_owned();
        }
    }

    let mut sockets = Vec::with_capacity(200);
    for _ in 0..200 {
        sockets
            .push(WorkspaceSocket::subscribe(&ts, &ts.client2, TEST_WORKSPACE_ID, &user2_id).await);
    }

    let started = Instant::now();
    ts.client
        .patch(
            &format!("{TEST_WS}/cards/{target_card_id}"),
            &json!({"title": "Benchmarked Update"}),
        )
        .await
        .check_ok();
    for sock in &mut sockets {
        sock.expect("blockChanged", "/block/id", &target_card_id)
            .await;
    }
    let elapsed = started.elapsed();
    eprintln!(
        "[server-authority benchmark] 50 open boards x 200 WS connections: card edit + 200-way delivery completed in {:.2} ms",
        elapsed.as_secs_f64() * 1000.0
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "50x200 open-board WS fanout took too long: {elapsed:?}"
    );
    ts.teardown().await;
}
