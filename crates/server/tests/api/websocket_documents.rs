//! Workspace revocation reaches the rooms of a connection that follows no workspace:
//! it is put out of the rooms of that workspace, and keeps its connection.

use std::time::Duration;

use futures_util::{SinkExt as _, StreamExt as _};
use serde_json::json;
use tokio_tungstenite::tungstenite::Message;

use crate::common::{ROOT_WORKSPACE_ID, Setup, TEST_WORKSPACE_ID, TestServer};
use crate::org_admin::{make_workspace, org_of};
use crate::rooms::{Incoming, Peer, QUIET, UPDATE, frame, give, paragraphs, table, two_in_a_room};

const WAIT: Duration = Duration::from_secs(5);

async fn server() -> TestServer {
    let ts = Setup::native().start().await.init_basic().await;
    let org = org_of(&ts).await;
    ts.client
        .post_empty(&format!("/org/{org}/workspaces/{TEST_WORKSPACE_ID}/join"))
        .await
        .check_ok();
    ts
}

#[derive(Clone, Copy, Debug)]
enum Revocation {
    Remove,
    Guest,
    Trash,
    WithdrawAdmin,
    DemoteAdmin,
    /// Who runs the server no more: the organization's administrator neither.
    NoMoreInstanceAdmin,
}

impl Revocation {
    const ALL: [Self; 6] = [
        Self::Remove,
        Self::Guest,
        Self::Trash,
        Self::WithdrawAdmin,
        Self::DemoteAdmin,
        Self::NoMoreInstanceAdmin,
    ];

    /// Whether it takes the card away: a guest of the workspace who was made an editor
    /// of the card's board is still invited to that board.
    fn takes_the_card(self) -> bool {
        !matches!(self, Self::Guest)
    }

    fn client(self, ts: &TestServer) -> &crate::common::Client {
        match self {
            Self::WithdrawAdmin | Self::DemoteAdmin | Self::NoMoreInstanceAdmin => &ts.client,
            _ => &ts.client2,
        }
    }

    async fn apply(self, ts: &TestServer, user: &str) {
        let member = format!("/w/{TEST_WORKSPACE_ID}/members/{user}");
        match self {
            Self::Remove => ts.client.delete(&member).await.check_no_content(),
            Self::Guest => ts
                .client
                .put(&member, &json!({"role": "guest"}))
                .await
                .check_ok(),
            Self::Trash => {
                let org = org_of(ts).await;
                ts.client
                    .delete(&format!("/org/{org}/workspaces/{TEST_WORKSPACE_ID}"))
                    .await
                    .check_no_content();
            }
            Self::WithdrawAdmin => {
                let org = org_of(ts).await;
                ts.client
                    .post_empty(&format!("/org/{org}/workspaces/{TEST_WORKSPACE_ID}/leave"))
                    .await
                    .check_ok();
            }
            Self::NoMoreInstanceAdmin => {
                let other = ts.client2.me().await.id;
                ts.client
                    .put(&format!("/users/{other}/admin"), &json!({"isAdmin": true}))
                    .await
                    .check_ok();
                ts.client2
                    .put(&format!("/users/{user}/admin"), &json!({"isAdmin": false}))
                    .await
                    .check_ok();
            }
            Self::DemoteAdmin => {
                let org = org_of(ts).await;
                let other = ts.client2.me().await.id;
                ts.client
                    .put(
                        &format!("/org/{org}/members/{other}/role"),
                        &json!({"role": "admin"}),
                    )
                    .await
                    .check_no_content();
                ts.client2
                    .put(
                        &format!("/org/{org}/members/{user}/role"),
                        &json!({"role": "member"}),
                    )
                    .await
                    .check_no_content();
            }
        }
    }
}

#[tokio::test]
async fn document_only_sockets_are_put_out_when_workspace_access_ends() {
    for ending in Revocation::ALL {
        let ts = server().await;
        let room = table(&ts, &paragraphs(&["initial"])).await;
        let other = ts.client2.me().await.id;
        give(&ts, &room.board_id, &other, "editor").await;
        let client = ending.client(&ts);
        let user = client.me().await.id;
        let mut peer = Peer::connect(client, 202).await;
        peer.joined(TEST_WORKSPACE_ID, &room.card_id).await;
        ending.apply(&ts, &user).await;
        if !ending.takes_the_card() {
            peer.write_acked("still invited").await;
            ts.teardown().await;
            continue;
        }
        let left = peer.text("docLeft").await;
        assert_eq!(
            left["reason"], "not_found",
            "{ending:?} puts a document-only socket out before the periodic recheck"
        );
        let refused = peer.join(TEST_WORKSPACE_ID, &room.card_id).await;
        assert_eq!(
            refused["type"], "docRefused",
            "{ending:?}: the connection stays, and is let in no more"
        );
        ts.teardown().await;
    }
}

#[tokio::test]
async fn document_admission_cannot_miss_concurrent_workspace_revocation() {
    for ending in Revocation::ALL {
        let ts = server().await;
        let room = table(&ts, &paragraphs(&["initial"])).await;
        let other = ts.client2.me().await.id;
        give(&ts, &room.board_id, &other, "editor").await;
        let client = ending.client(&ts);
        let user = client.me().await.id;
        let mut socket = client.connect_websocket().await;
        let (reached, resume) = ts.app.collab().pause_next_admission();
        socket
            .send(Message::Text(
                json!({"type": "joinDoc", "workspaceId": TEST_WORKSPACE_ID,
                    "cardId": room.card_id})
                .to_string()
                .into(),
            ))
            .await
            .expect("document join sent");
        tokio::time::timeout(WAIT, reached.notified())
            .await
            .expect("document admission reaches the registration seam");
        ending.apply(&ts, &user).await;
        resume.notify_one();
        let message = tokio::time::timeout(WAIT, socket.next())
            .await
            .expect("revocation ends admission promptly")
            .expect("an answer")
            .expect("a protocol frame");
        let Message::Text(text) = message else {
            panic!("{ending:?} must refuse before sending a document: {message:?}");
        };
        let answer: serde_json::Value = serde_json::from_str(&text).expect("JSON");
        let expected = if ending.takes_the_card() {
            "docRefused"
        } else {
            "docJoined"
        };
        assert_eq!(answer["type"], expected, "{ending:?}: {answer}");
        ts.teardown().await;
    }
}

#[tokio::test]
async fn a_revoked_document_connection_cannot_write_or_receive_another_update() {
    let ts = server().await;
    let (_, mut one, mut two) = two_in_a_room(&ts, &paragraphs(&["initial"])).await;
    // Sidebar subscriptions have a separate lifetime from the document.
    for action in ["subscribeWorkspace", "unsubscribeWorkspace"] {
        two.send_json(json!({"type": action, "workspaceId": TEST_WORKSPACE_ID}))
            .await;
    }
    two.write_acked("before removal").await;
    one.settle().await;
    let late = two.editor().append_paragraph("must not be stored");
    let user = ts.client2.me().await.id;
    Revocation::Remove.apply(&ts, &user).await;
    let left = two.text("docLeft").await;
    assert_eq!(left["reason"], "not_found", "{left}");
    two.try_send_binary(frame(two.doc, two.epoch, UPDATE, &late))
        .await;
    one.write_acked("after removal").await;
    loop {
        match two.incoming(QUIET).await {
            Incoming::Nothing => break,
            Incoming::Frame(_) | Incoming::Caret(_) => panic!("no later document update"),
            Incoming::Closed => panic!("the connection stays"),
            Incoming::Text(_) => {}
        }
    }
    one.settle().await;
    assert_eq!(
        one.lines(),
        ["initial", "before removal", "after removal"],
        "revoked writes are dropped"
    );
    ts.teardown().await;
}

#[tokio::test]
async fn leaving_a_document_removes_its_workspace_watch() {
    let ts = server().await;
    let room = table(&ts, &paragraphs(&["initial"])).await;
    let user = ts.client2.me().await.id;
    give(&ts, &room.board_id, &user, "editor").await;
    let mut peer = Peer::connect(&ts.client2, 202).await;
    peer.joined(TEST_WORKSPACE_ID, &room.card_id).await;
    peer.leave().await;
    let refused = peer.join("no-workspace", "no-card").await;
    assert_eq!(refused["type"], "docRefused", "later command is answered");
    Revocation::Remove.apply(&ts, &user).await;
    assert_eq!(
        peer.incoming(QUIET).await,
        Incoming::Nothing,
        "no active document was revoked"
    );
    ts.teardown().await;
}

#[tokio::test]
async fn malformed_and_server_evicted_documents_leave_no_workspace_watch() {
    let ts = Setup::native()
        .configure(|config| config.collab.collab_recheck_seconds = 1)
        .start()
        .await
        .init_basic()
        .await;
    let org = org_of(&ts).await;
    ts.client
        .post_empty(&format!("/org/{org}/workspaces/{TEST_WORKSPACE_ID}/join"))
        .await
        .check_ok();
    let room = table(&ts, &paragraphs(&["initial"])).await;
    let user = ts.client2.me().await.id;
    give(&ts, &room.board_id, &user, "editor").await;
    let mut peer = Peer::connect(&ts.client2, 202).await;
    peer.joined(TEST_WORKSPACE_ID, &room.card_id).await;
    ts.client
        .delete(&format!(
            "/w/{TEST_WORKSPACE_ID}/containers/{}/members/{user}",
            room.board_id
        ))
        .await
        .check_no_content();
    let left = peer.text("docLeft").await;
    assert_eq!(
        left["reason"], "not_found",
        "the board's room evicts the member"
    );
    peer.send_json(json!({"type": "joinDoc", "workspaceId": TEST_WORKSPACE_ID,
        "cardId": room.card_id, "doc": null}))
        .await;
    peer.barrier(&ts).await;
    Revocation::Remove.apply(&ts, &user).await;
    assert_eq!(
        peer.incoming(QUIET).await,
        Incoming::Nothing,
        "no stale watch after eviction or malformed input"
    );
    ts.teardown().await;
}

#[tokio::test]
async fn unrelated_admin_withdrawal_does_not_close_pending_document_admission() {
    let ts = server().await;
    let room = table(&ts, &paragraphs(&["initial"])).await;
    let org = org_of(&ts).await;
    let other = make_workspace(&ts, &org, "Unrelated").await;
    let at = format!("/org/{org}/workspaces/{other}");
    ts.client.post_empty(&format!("{at}/join")).await.check_ok();
    let mut socket = ts.client.connect_websocket().await;
    let (reached, resume) = ts.app.collab().pause_next_admission();
    socket
        .send(Message::Text(
            json!({"type": "joinDoc", "workspaceId": TEST_WORKSPACE_ID,
        "cardId": room.card_id})
            .to_string()
            .into(),
        ))
        .await
        .expect("join sent");
    tokio::time::timeout(WAIT, reached.notified())
        .await
        .expect("admission is pending");
    ts.client
        .post_empty(&format!("{at}/leave"))
        .await
        .check_ok();
    resume.notify_one();
    let message = tokio::time::timeout(WAIT, socket.next())
        .await
        .expect("admission resumes")
        .expect("connection stays open")
        .expect("protocol frame");
    let Message::Text(text) = message else {
        panic!("expected document admission, got {message:?}");
    };
    let joined: serde_json::Value = serde_json::from_str(&text).expect("JSON message");
    assert_eq!(
        joined["type"], "docJoined",
        "unrelated withdrawal preserves admission"
    );
    ts.teardown().await;
}

#[tokio::test]
async fn an_admission_read_before_admin_revocation_is_asked_again() {
    for demote in [false, true] {
        let ts = server().await;
        let org = org_of(&ts).await;
        let workspace = make_workspace(&ts, &org, "Joined only").await;
        let at = format!("/org/{org}/workspaces/{workspace}");
        ts.client.post_empty(&format!("{at}/join")).await.check_ok();
        let board = ts
            .create_board(&workspace, fvoci_model::ContainerVisibility::Private)
            .await;
        let card: fvoci_model::Card = ts
            .client
            .post(
                &format!("/w/{workspace}/boards/{}/cards", board.id),
                &json!({"title": "Joined card", "body": paragraphs(&["initial"])}),
            )
            .await
            .created_json();
        if demote {
            let other = ts.client2.me().await.id;
            ts.client
                .put(
                    &format!("/org/{org}/members/{other}/role"),
                    &json!({"role": "admin"}),
                )
                .await
                .check_no_content();
        }
        let mut socket = ts.client.connect_websocket().await;
        let (reached, resume) = ts.app.collab().pause_next_admission();
        socket
            .send(Message::Text(
                json!({"type": "joinDoc", "workspaceId": workspace,
            "cardId": card.id})
                .to_string()
                .into(),
            ))
            .await
            .expect("join sent");
        tokio::time::timeout(WAIT, reached.notified())
            .await
            .expect("admission was read, room not yet held");
        if demote {
            let user = ts.client.me().await.id;
            ts.client2
                .put(
                    &format!("/org/{org}/members/{user}/role"),
                    &json!({"role": "member"}),
                )
                .await
                .check_no_content();
        } else {
            ts.client
                .post_empty(&format!("{at}/leave"))
                .await
                .check_ok();
        }
        resume.notify_one();
        let message = tokio::time::timeout(WAIT, socket.next())
            .await
            .expect("admission resumes")
            .expect("a refusal")
            .expect("protocol frame");
        let Message::Text(text) = message else {
            panic!("expected a refusal, got {message:?}");
        };
        let refused: serde_json::Value = serde_json::from_str(&text).expect("JSON message");
        assert_eq!(
            refused["type"], "docRefused",
            "unadmitted connection receives no document"
        );
        assert_eq!(
            refused["reason"], "not_found",
            "the former administrator cannot enter"
        );
        ts.teardown().await;
    }
}

/// Access to one workspace ending ends what the connection followed of it, and tells
/// it; the connection's room of a card of another workspace goes on.
#[tokio::test]
async fn ending_one_workspace_leaves_a_room_of_another() {
    let ts = server().await;
    let room = table(&ts, &paragraphs(&["initial"])).await;
    let user = ts.client2.me().await.id;
    give(&ts, &room.board_id, &user, "editor").await;
    let mut peer = Peer::connect(&ts.client2, 202).await;
    peer.joined(TEST_WORKSPACE_ID, &room.card_id).await;
    peer.send_json(json!({"type": "subscribeWorkspace", "workspaceId": ROOT_WORKSPACE_ID}))
        .await;
    let ack = peer.text("workspaceSubscribed").await;
    assert_eq!(ack["workspaceId"], ROOT_WORKSPACE_ID, "{ack}");
    ts.client
        .delete(&format!("/w/{ROOT_WORKSPACE_ID}/members/{user}"))
        .await
        .check_no_content();
    let ended = peer.text("workspaceUnsubscribed").await;
    assert_eq!(ended["workspaceId"], ROOT_WORKSPACE_ID, "{ended}");
    peer.write_acked("still in the room").await;
    ts.teardown().await;
}
