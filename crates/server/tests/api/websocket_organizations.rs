//! What one organization takes back reaches the connections of its own workspaces and
//! links only: a subscription of another organization's being checked meanwhile is not
//! checked again for it.

use std::time::Duration;

use futures_util::SinkExt as _;
use fvoci_model::{
    Block, BlockType, ContainerVisibility, OrgRole, PublicLinkChange, WorkspaceRole, new_id,
};
use fvoci_store::DbType;
use fvoci_store::testing::db_type_of_the_tests;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

use crate::common::{PASSWORD, Setup, TEST_WORKSPACE_ID, TestServer, USER1_USERNAME, WebSocket};
use crate::org_admin::org_of;

const WAIT: Duration = Duration::from_secs(5);

/// Sends `command`, holds its check, runs what the first organization takes back
/// meanwhile (`revoke`), and returns whether the subscription was checked again.
async fn checked_again_after(
    ts: &TestServer,
    socket: &mut WebSocket,
    command: Value,
    revoke: impl AsyncFnOnce(),
) -> bool {
    let pause = ts.app.ws().pause_next_subscription();
    socket
        .send(Message::Text(command.to_string().into()))
        .await
        .expect("subscription sent");
    tokio::time::timeout(WAIT, pause.reached())
        .await
        .expect("the subscription is checked");
    let again = ts.app.ws().pause_next_subscription();
    revoke().await;
    pause.resume();
    let checked_again = tokio::time::timeout(Duration::from_secs(1), again.reached())
        .await
        .is_ok();
    drop(again);
    checked_again
}

/// The first organization's board is shared, its link turned off, a workspace of it
/// trashed and its administrator logged out.
async fn first_organization_takes_back(ts: &TestServer) {
    let shared = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    ts.share(&shared.id).await;
    ts.client
        .post(
            &format!(
                "/w/{TEST_WORKSPACE_ID}/containers/{}/public-link",
                shared.id
            ),
            &json!({"enabled": false}),
        )
        .await
        .check_ok();
    let org = org_of(ts).await;
    ts.client
        .delete(&format!("/org/{org}/workspaces/{TEST_WORKSPACE_ID}"))
        .await
        .check_no_content();
    ts.login(USER1_USERNAME, PASSWORD)
        .await
        .post_empty("/logout")
        .await
        .check_no_content();
}

/// The second user follows a workspace of a second organization; none of what the first
/// organization takes back concerns that workspace or that user.
#[tokio::test]
async fn one_organizations_revocations_do_not_check_anothers_workspace_again() {
    // An SQLite database holds one organization.
    if db_type_of_the_tests() != DbType::Postgres {
        return;
    }
    let ts = Setup::local_mode().start().await.init_basic().await;
    let member = ts.client2.me().await.id;
    let (theirs, their_workspace, _board) = crate::workspaces::another_organization(&ts).await;
    theirs
        .add_organization_member(&member, OrgRole::Member)
        .await
        .expect("the second user joins their organization");
    theirs
        .add_workspace_member(&their_workspace, &member, WorkspaceRole::Member)
        .await
        .expect("the second user is let into their workspace");
    let mut socket = ts.client2.connect_websocket().await;
    let command = json!({"type": "subscribeWorkspace", "workspaceId": their_workspace});
    assert!(
        !checked_again_after(&ts, &mut socket, command, async || {
            first_organization_takes_back(&ts).await;
        })
        .await,
        "the first organization's revocations checked the second's subscription again"
    );
    ts.teardown().await;
}

/// A reader without a session follows a card of the second organization's shared board
/// by its link; the first organization turning one of its own links off does not
/// concern that board.
#[tokio::test]
async fn one_organizations_links_do_not_check_anothers_reader_again() {
    if db_type_of_the_tests() != DbType::Postgres {
        return;
    }
    let ts = Setup::local_mode().start().await.init_basic().await;
    let (theirs, their_workspace, their_board) = crate::workspaces::another_organization(&ts).await;
    let owner = theirs
        .get_container(&their_board)
        .await
        .expect("their board")
        .created_by;
    let mut card = Block {
        id: new_id(),
        container_id: their_board.clone(),
        block_type: BlockType::Card,
        ..Block::default()
    };
    theirs
        .insert_block(&mut card, &owner)
        .await
        .expect("their card is stored");
    let token = ts
        .app
        .scoped(theirs.tenant())
        .set_public_link(&their_board, PublicLinkChange::On, "")
        .await
        .expect("their board is shared")
        .token;
    let mut socket = ts.anonymous().as_browser().connect_websocket().await;
    let command = json!({
        "type": "subscribeBlocks", "workspaceId": their_workspace,
        "blockIds": [card.id], "readToken": token,
    });
    assert!(
        !checked_again_after(&ts, &mut socket, command, async || {
            first_organization_takes_back(&ts).await;
        })
        .await,
        "the first organization's links checked a reader of the second's again"
    );
    ts.teardown().await;
}
