//! Found in the independent review of `org-o5c`: what a pending write of the
//! system's reach does to the requests of an organization.

use std::time::{Duration, Instant};

use futures_util::{SinkExt as _, StreamExt as _};
use fvoci_store::DbType;
use fvoci_store::testing::db_type_of_the_tests;
use serde_json::json;
use tokio_tungstenite::tungstenite::Message;

use crate::common::{ROOT_WORKSPACE_ID, Setup, TestServer};

/// How long the system's write below is held open.
const HELD: Duration = Duration::from_secs(4);

/// How much slower than an unhindered one a category may be made.
const SLACK: Duration = Duration::from_secs(1);

/// How long making a category takes while a slow statement of the system's reach runs
/// on a table of the trash, as a purge of a large trash does: it touches no row.
async fn category_during_a_system_write(ts: &TestServer, user: &str, name: &str) -> Duration {
    let system = ts.app.store().system();
    let slow = format!(
        "DELETE FROM deleted_containers WHERE CAST((SELECT pg_sleep({})) AS TEXT) = 'never'",
        HELD.as_secs()
    );
    let pending = tokio::spawn(async move { system.execute_unchecked(&slow).await });
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!pending.is_finished(), "the system's write is under way");
    let started = Instant::now();
    ts.client
        .post(
            &format!("/w/{ROOT_WORKSPACE_ID}/categories"),
            &json!({"name": name, "userId": user, "workspaceId": ROOT_WORKSPACE_ID}),
        )
        .await
        .check_created();
    let took = started.elapsed();
    pending
        .await
        .expect("the system's write ran")
        .expect("the system's write succeeded");
    took
}

/// The housekeeping purges the trash of every organization through the system's
/// reach (`App::housekeeping`), whose writes move the revision every organization's
/// dispatch waits for. A category change is now announced inline in its request
/// (`broadcast_category_change(..).await`), and `Hub::send` waits for every writer of
/// that revision first: so while such a write is under way, an organization's user who
/// has the app open waits for it to make a category. Without an open connection the
/// same request is not held, which shows the database is not what holds it.
#[tokio::test]
async fn a_pending_system_write_does_not_hold_an_organizations_request() {
    if db_type_of_the_tests() != DbType::Postgres {
        return;
    }
    let ts = Setup::native().start().await.init_basic().await;
    let user = ts.client.me().await.id;

    let without_listener = category_during_a_system_write(&ts, &user, "closed app").await;
    assert!(
        without_listener < SLACK,
        "with nobody listening the system's write holds nothing: {without_listener:?}"
    );

    let mut socket = ts.client.connect_websocket().await;
    socket
        .send(Message::Text(
            json!({"type": "subscribeWorkspace", "workspaceId": ROOT_WORKSPACE_ID})
                .to_string()
                .into(),
        ))
        .await
        .expect("subscription sent");
    // The subscription is registered: the write below has a listener to tell.
    crate::common::carried_out(ts.app.ws(), &mut socket).await;

    let with_listener = category_during_a_system_write(&ts, &user, "open app").await;
    assert!(
        with_listener < SLACK,
        "a category took {with_listener:?} while the system wrote and its user had the app \
         open, {without_listener:?} with the app closed: the request waited for a write \
         of no organization of its own"
    );
    drop(socket);
    ts.teardown().await;
}

/// Reads frames until a pong answers `ping`, and fails if the server closes first.
async fn still_open(socket: &mut crate::common::WebSocket, why: &str) {
    socket
        .send(Message::Ping(vec![5].into()))
        .await
        .expect("ping sent");
    let ended = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(frame) = socket.next().await {
            match frame {
                Ok(Message::Pong(_)) => return None,
                Ok(Message::Close(frame)) => return Some(format!("{frame:?}")),
                Ok(_) => {}
                Err(error) => return Some(error.to_string()),
            }
        }
        Some("ended without a close frame".to_owned())
    })
    .await
    .expect("the socket answers or ends");
    if let Some(ended) = ended {
        panic!("{why}: the connection was closed ({ended})");
    }
}

async fn subscribed(socket: &mut crate::common::WebSocket, workspace_id: &str) {
    socket
        .send(Message::Text(
            json!({"type": "subscribeWorkspace", "workspaceId": workspace_id})
                .to_string()
                .into(),
        ))
        .await
        .expect("subscription sent");
    still_open(socket, "subscribing").await;
}

/// The listeners' revision is one for every organization, and every session that ends
/// (a logout, the hourly clean-up of ended sessions, which ends them one by one) moves
/// it on. A subscription being checked meanwhile is checked again, and after eight
/// such checks it is given up and its connection closed (1013). Here another user's
/// logouts, which take nothing of this user's, end this user's connection.
#[tokio::test]
async fn other_users_logouts_do_not_end_a_subscription() {
    use crate::common::{PASSWORD, USER2_USERNAME};

    let ts = Setup::local_mode().start().await.init_basic().await;
    let mut socket = ts.client.connect_websocket().await;
    let mut pause = ts.app.ws().pause_next_subscription();
    socket
        .send(Message::Text(
            json!({"type": "subscribeWorkspace", "workspaceId": ROOT_WORKSPACE_ID})
                .to_string()
                .into(),
        ))
        .await
        .expect("subscription sent");
    tokio::time::timeout(Duration::from_secs(5), pause.reached())
        .await
        .expect("the subscription is checked");
    for _ in 0..8 {
        let next = ts.app.ws().pause_next_subscription();
        let other = ts.login(USER2_USERNAME, PASSWORD).await;
        other.post_empty("/logout").await.check_no_content();
        pause.resume();
        let checked_again = tokio::time::timeout(Duration::from_secs(1), next.reached())
            .await
            .is_ok();
        pause = next;
        if !checked_again {
            break;
        }
    }
    drop(pause);
    still_open(&mut socket, "after another user's logouts").await;
    ts.teardown().await;
}

/// A workspace that goes to the trash now closes every connection that followed it,
/// with what else that connection followed: the member's other workspace stops being
/// followed too, and the page has to connect again to hear of it.
#[tokio::test]
async fn trashing_one_workspace_leaves_a_members_other_subscription_open() {
    use crate::common::TEST_WORKSPACE_ID;

    let ts = Setup::local_mode().start().await.init_basic().await;
    let mut socket = ts.client2.connect_websocket().await;
    subscribed(&mut socket, ROOT_WORKSPACE_ID).await;
    subscribed(&mut socket, TEST_WORKSPACE_ID).await;
    let org = crate::org_admin::org_of(&ts).await;
    ts.client
        .delete(&format!("/org/{org}/workspaces/{TEST_WORKSPACE_ID}"))
        .await
        .check_no_content();
    still_open(&mut socket, "after another workspace went to the trash").await;
    ts.teardown().await;
}

/// A workspace guest who is explicitly added as a member of a private template board can
/// read both the template's metadata and its blocks (200).
#[tokio::test]
async fn a_guest_member_of_a_template_board_reads_its_blocks() {
    use crate::common::{TEST_WORKSPACE_ID, TEST_WS};
    use fvoci_model::{Container, ContainerVisibility, WorkspaceRole};

    let ts = Setup::native().start().await.init_basic().await;
    let guest = ts.client2.me().await.id;
    ts.app
        .store()
        .execute_unchecked(&format!(
            "UPDATE workspace_members SET role = '{}' WHERE user_id = '{guest}' \
             AND workspace_id = '{TEST_WORKSPACE_ID}'",
            WorkspaceRole::Guest.as_str()
        ))
        .await
        .expect("the second user becomes a guest of the workspace");
    let template: Container = ts
        .client
        .create_board(&Container {
            is_template: true,
            ..crate::boards::new_board(
                TEST_WORKSPACE_ID,
                ContainerVisibility::Private,
                "Guest's template",
            )
        })
        .await
        .created_json();
    ts.client
        .add_member(&crate::boards::member(
            &template.id,
            &guest,
            fvoci_model::ContainerRole::Viewer,
        ))
        .await
        .check_ok();
    let at = format!("{TEST_WS}/containers/{}", template.id);
    ts.client2.get(&at).await.check_ok();
    ts.client2
        .get(&format!("{TEST_WS}/boards/{}/views", template.id))
        .await
        .check_ok();
    ts.teardown().await;
}

/// What a dispatch does once it is checked (serializing its message and queueing it,
/// `Hub::queue`) runs inside `Store::if_visibility_current`, under the one lock of
/// every organization's revisions, which each write of an authorization table takes
/// as its first such statement starts. So a slow dispatch of one organization holds
/// another organization's writes. The dispatch is made slow here on purpose; a large
/// message to many listeners is the real one.
#[tokio::test]
async fn one_organizations_dispatch_does_not_hold_anothers_writes() {
    const DISPATCH: Duration = Duration::from_secs(3);

    let ts = Setup::native().start().await.init_basic().await;
    let ours = ts.app.store().clone();
    let version = ours.visibility_version().await;
    let dispatch = std::thread::spawn(move || {
        ours.if_visibility_current(version, || std::thread::sleep(DISPATCH))
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    let theirs = ts
        .app
        .store()
        .for_org(&fvoci_model::new_id())
        .expect("an identifier");
    let started = Instant::now();
    theirs
        .execute_unchecked(&format!(
            "UPDATE containers SET title = title WHERE id = '{}'",
            fvoci_model::new_id()
        ))
        .await
        .expect("a write of another organization");
    let took = started.elapsed();
    assert!(dispatch.join().expect("the dispatch ran"), "it was current");
    assert!(
        took < SLACK,
        "another organization's write took {took:?} while this one dispatched"
    );
    ts.teardown().await;
}

/// Whether a subscription being checked while the housekeeping runs once is checked
/// again; the subscription is registered either way.
async fn rechecked_by_housekeeping(ts: &TestServer) -> bool {
    let mut socket = ts.client.connect_websocket().await;
    let pause = ts.app.ws().pause_next_subscription();
    socket
        .send(Message::Text(
            json!({"type": "subscribeWorkspace", "workspaceId": ROOT_WORKSPACE_ID})
                .to_string()
                .into(),
        ))
        .await
        .expect("subscription sent");
    tokio::time::timeout(Duration::from_secs(5), pause.reached())
        .await
        .expect("the subscription is checked");
    let again = ts.app.ws().pause_next_subscription();
    fvoci_server::keep_house(&ts.app).await;
    pause.resume();
    let checked_again = tokio::time::timeout(Duration::from_secs(1), again.reached())
        .await
        .is_ok();
    drop(again);
    still_open(&mut socket, "after the housekeeping").await;
    checked_again
}

/// The housekeeping's clean-up of ended sessions closes what each still has open, one
/// session at a time (`lib.rs`, `clean_up_sessions`), and each closing moves the one
/// listeners' revision of every organization on, whether or not it closed anything.
/// So another user's session that ended, which no connection holds, makes a
/// subscription being checked meanwhile be checked again. Without an ended session
/// the same housekeeping checks nothing again.
#[tokio::test]
async fn cleaning_up_another_users_ended_session_does_not_recheck_a_subscription() {
    use crate::common::{PASSWORD, USER2_USERNAME};

    let control = Setup::local_mode().start().await.init_basic().await;
    assert!(
        !rechecked_by_housekeeping(&control).await,
        "with no ended session the housekeeping checks nothing again"
    );
    control.teardown().await;

    let ts = Setup::local_mode().start().await.init_basic().await;
    drop(ts.login(USER2_USERNAME, PASSWORD).await);
    let user2 = ts.client2.me().await.id;
    ts.app
        .store()
        .execute_unchecked(&format!(
            "UPDATE sessions SET expires_at = 1 WHERE CAST(user_id AS TEXT) = '{user2}'"
        ))
        .await
        .expect("the other user's sessions end");
    assert!(
        !rechecked_by_housekeeping(&ts).await,
        "ending a session no connection holds made this subscription be checked again"
    );
    ts.teardown().await;
}
