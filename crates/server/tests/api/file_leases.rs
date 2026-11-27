//! Who has a file open in the editor: one lease of a file at a time, held by an editor
//! of its board and told to its readers, with the holder named only to whom presence
//! names them; a lease keeps nobody from writing a version. Its term is counted in the
//! server's unit tests (`app::file_leases`).

use fvoci_model::{ContainerMember, ContainerVisibility};
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::common::{Client, PASSWORD, Setup, TEST_WORKSPACE_ID, TEST_WS};
use crate::files::{upload, uploaded_id};

fn lease_path(board_id: &str, file_id: &str) -> String {
    format!("{TEST_WS}/containers/{board_id}/files/{file_id}/lease")
}

async fn hold(client: &Client, board_id: &str, file_id: &str, lease_id: &str) -> Value {
    client
        .put(
            &lease_path(board_id, file_id),
            &json!({"leaseId": lease_id}),
        )
        .await
        .ok_json()
}

async fn refused(client: &Client, board_id: &str, file_id: &str, lease_id: &str) {
    let answer = client
        .put(
            &lease_path(board_id, file_id),
            &json!({"leaseId": lease_id}),
        )
        .await;
    assert_eq!(answer.status, StatusCode::CONFLICT, "{answer:?}");
    let problem: Value = answer.json();
    assert_eq!(
        problem["reason"], "file_being_edited",
        "a lease refused for being held says so"
    );
}

async fn told(client: &Client, board_id: &str, file_id: &str) -> (bool, String) {
    let lease: Value = client.get(&lease_path(board_id, file_id)).await.ok_json();
    (
        lease["editing"].as_bool().expect("a flag"),
        lease["editorId"].as_str().expect("a string").to_owned(),
    )
}

async fn release(client: &Client, board_id: &str, file_id: &str, lease_id: &str) {
    client
        .delete(&format!("{}/{lease_id}", lease_path(board_id, file_id)))
        .await
        .check_no_content();
}

fn lease_id(held: &Value) -> String {
    held["leaseId"].as_str().expect("a lease").to_owned()
}

/// One editor holds a file's lease at a time, the same person's other window included;
/// the others are told who; holding on keeps it, letting it go frees the file for the
/// next; and a lease keeps nobody from writing a version.
#[tokio::test]
async fn one_editor_holds_a_file_s_lease_at_a_time() {
    let ts = Setup::native().start().await.init_basic().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let file_id = uploaded_id(&upload(&ts.client, &board.id, "a.hwpx", b"first").await);
    let other_file = uploaded_id(&upload(&ts.client, &board.id, "b.hwpx", b"other").await);
    let (owner, editor) = (ts.client.me().await.id, ts.client2.me().await.id);
    let member = ContainerMember {
        container_id: board.id.clone(),
        user_id: editor.clone(),
        role: fvoci_model::ContainerRole::Editor,
        synthetic: false,
        inherited_role: None,
    };
    ts.client.add_member(&member).await.check_ok();

    assert_eq!(
        told(&ts.client2, &board.id, &file_id).await,
        (false, String::new())
    );
    let held = hold(&ts.client, &board.id, &file_id, "").await;
    assert_eq!(held["seconds"], 90);
    let lease = lease_id(&held);

    refused(&ts.client2, &board.id, &file_id, "").await;
    refused(&ts.client, &board.id, &file_id, "").await;
    // Naming the holder's lease is no way in for another.
    refused(&ts.client2, &board.id, &file_id, &lease).await;
    assert_eq!(
        told(&ts.client2, &board.id, &file_id).await,
        (true, owner.clone())
    );
    assert_eq!(
        told(&ts.client, &board.id, &file_id).await,
        (true, owner.clone())
    );
    // Another file of the board is another lease.
    let theirs = lease_id(&hold(&ts.client2, &board.id, &other_file, "").await);

    let again = hold(&ts.client, &board.id, &file_id, &lease).await;
    assert_eq!(lease_id(&again), lease, "held on to, it is the same lease");

    // The editor writes a version while the lease is the owner's.
    let next = uploaded_id(&upload(&ts.client2, &board.id, "next.hwpx", b"second").await);
    ts.client2
        .post(
            &format!("{TEST_WS}/containers/{}/files/{file_id}/versions", board.id),
            &json!({"baseVersion": 1, "fileId": next}),
        )
        .await
        .check_created();

    // Letting another's lease go is answered and changes nothing.
    release(&ts.client2, &board.id, &file_id, &lease).await;
    assert_eq!(told(&ts.client2, &board.id, &file_id).await, (true, owner));
    release(&ts.client, &board.id, &file_id, &lease).await;
    assert_eq!(
        told(&ts.client2, &board.id, &file_id).await,
        (false, String::new())
    );
    let taken = lease_id(&hold(&ts.client2, &board.id, &file_id, "").await);
    assert_ne!(taken, lease);
    assert_eq!(told(&ts.client, &board.id, &file_id).await, (true, editor));
    // A lease let go of is no more its holder's to hold on to.
    refused(&ts.client, &board.id, &file_id, &lease).await;
    release(&ts.client2, &board.id, &file_id, &theirs).await;
    ts.teardown().await;
}

/// Readers of the board are told whether somebody edits a file and hold no lease; who
/// is not on the board, and who reads it by its link, are told nothing; a file the
/// board does not have is 404.
#[tokio::test]
async fn the_lease_is_held_by_editors_and_told_to_readers() {
    let ts = Setup::local_mode().start().await.init_basic().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let file_id = uploaded_id(&upload(&ts.client, &board.id, "a.hwpx", b"first").await);
    let path = lease_path(&board.id, &file_id);
    let lease = lease_id(&hold(&ts.client, &board.id, &file_id, "").await);

    // Not on the board.
    let answer = ts.client2.get(&path).await;
    assert!(
        matches!(answer.status, StatusCode::FORBIDDEN | StatusCode::NOT_FOUND),
        "{}",
        answer.status
    );
    assert_ne!(
        ts.client2.put(&path, &json!({"leaseId": ""})).await.status,
        StatusCode::OK
    );

    let viewer = ContainerMember {
        container_id: board.id.clone(),
        user_id: ts.client2.me().await.id,
        role: fvoci_model::ContainerRole::Viewer,
        synthetic: false,
        inherited_role: None,
    };
    ts.client.add_member(&viewer).await.check_ok();
    assert!(told(&ts.client2, &board.id, &file_id).await.0);
    ts.client2
        .put(&path, &json!({"leaseId": ""}))
        .await
        .check_forbidden();
    ts.client2
        .delete(&format!("{path}/{lease}"))
        .await
        .check_forbidden();

    // By the link: nothing of the lease.
    let token = ts.share(&board.id).await;
    let anonymous = ts.anonymous();
    anonymous
        .get(&format!(
            "/shared/{}/files/{file_id}/lease?readToken={token}",
            board.id
        ))
        .await
        .check_not_found();
    anonymous
        .get(&format!("{path}?readToken={token}"))
        .await
        .check_unauthorized();

    // A file of no such name, and one of another board.
    let elsewhere = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let theirs = uploaded_id(&upload(&ts.client, &elsewhere.id, "b.hwpx", b"b").await);
    for (board_id, file) in [(&board.id, "none.hwpx"), (&board.id, theirs.as_str())] {
        ts.client
            .get(&lease_path(board_id, file))
            .await
            .check_not_found();
        ts.client
            .put(&lease_path(board_id, file), &json!({"leaseId": ""}))
            .await
            .check_not_found();
    }
    assert!(told(&ts.client, &board.id, &file_id).await.0, "still held");
    ts.teardown().await;
}

/// The holder is named as presence names people: a guest of the workspace sees who
/// shares a board role with them, a role through a team included, and so is told who
/// edits a file of a board they read. (Whoever holds a lease edits the board, and so
/// shares it with every reader of it that is no anonymous one: the empty `editorId` of
/// a holder the reader does not see stays the server's guard, not a case a test can
/// make.)
#[tokio::test]
async fn a_guest_on_the_board_is_told_who_edits_as_presence_names_them() {
    use crate::rooms::give_in;
    use crate::rooms_changes::workspace_admin;

    let ts = Setup::native().start().await.init_basic().await;
    workspace_admin(&ts).await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let file_id = uploaded_id(&upload(&ts.client, &board.id, "a.hwpx", b"first").await);
    // The holder edits through a team: no role of their own on the board.
    let team: Value = ts
        .client
        .post(
            &format!("/w/{TEST_WORKSPACE_ID}/teams"),
            &json!({ "name": "Writers" }),
        )
        .await
        .ok_json();
    let team_id = team["id"].as_str().expect("a team has an id");
    let holder = ts.client2.me().await.id;
    ts.client
        .put(
            &format!("/w/{TEST_WORKSPACE_ID}/teams/{team_id}/members/{holder}"),
            &json!({}),
        )
        .await
        .check_no_content();
    ts.client
        .put(
            &format!(
                "/w/{TEST_WORKSPACE_ID}/containers/{}/teams/{team_id}",
                board.id
            ),
            &json!({ "role": "editor" }),
        )
        .await
        .check_no_content();
    let guest = ts
        .register_and_login("lease.guest", "lease.guest@sample.com", PASSWORD)
        .await;
    let guest_id = guest.me().await.id;
    give_in(&ts, TEST_WORKSPACE_ID, &board.id, &guest_id, "viewer").await;
    ts.client
        .put(
            &format!("{TEST_WS}/members/{guest_id}"),
            &json!({"role": "guest"}),
        )
        .await
        .check_ok();

    let lease = lease_id(&hold(&ts.client2, &board.id, &file_id, "").await);
    let seen = ts
        .app
        .users_in_workspace(
            &guest_id,
            TEST_WORKSPACE_ID,
            fvoci_model::WorkspaceRole::Guest,
            std::slice::from_ref(&holder),
        )
        .await
        .expect("the users are read");
    assert_eq!(seen.len(), 1, "presence names the holder to the guest");
    assert_eq!(
        told(&guest, &board.id, &file_id).await,
        (true, holder.clone())
    );
    // Whoever works in the workspace is told who.
    assert_eq!(told(&ts.client, &board.id, &file_id).await, (true, holder));
    release(&ts.client2, &board.id, &file_id, &lease).await;
    ts.teardown().await;
}
