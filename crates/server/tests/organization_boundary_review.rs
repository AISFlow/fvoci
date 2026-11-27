//! Independent organization-boundary regressions, using the existing API harness.

#[path = "api/common.rs"]
mod common;

use std::time::Duration;

use futures_util::{SinkExt as _, StreamExt as _};
use fvoci_model::PublicLinkChange;
use fvoci_model::{
    Block, BlockType, Container, ContainerMember, ContainerVisibility, Credential, OrgRole,
    PersonName, Title, User, UserWorkspace, Workspace, WorkspaceRole, new_id,
};
use fvoci_store::testing::db_type_of_the_tests;
use fvoci_store::{DbType, Store, Tenant};
use reqwest::StatusCode;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

use crate::common::ROOT_WS;
use common::{HIDDEN_WORKSPACE_ID, ROOT_WORKSPACE_ID, Setup, TestServer, UNKNOWN_ID};

fn open(config: &mut fvoci_server::config::Configuration) {
    "open".clone_into(&mut config.org_creation);
}

/// A second organization, with its own member, workspace and private board.
async fn other_organization(ts: &TestServer) -> (Store, Workspace, User, Container) {
    let workspace = Workspace {
        id: new_id(),
        title: "Other organization's workspace".to_owned(),
        ..Workspace::default()
    };
    let org_id = ts
        .app
        .store()
        .create_organization("Other organization", &workspace)
        .await
        .expect("a second organization is created");
    let store = ts.app.store().for_org(&org_id).expect("an organization ID");
    let mut owner = User {
        id: new_id(),
        username: "confidential.other.owner".to_owned(),
        email: "other.owner@example.com".to_owned(),
        name: PersonName {
            first_name: "Confidential-First".to_owned(),
            last_name: "Confidential-Last".to_owned(),
            latin_first_name: "Confidential-Latin".to_owned(),
            custom_name: "Other organization's owner".to_owned(),
            title: Some(Title::Prof),
            credentials: vec![Credential::Phd],
            ..PersonName::default()
        },
        ..User::default()
    };
    store
        .create_user(&mut owner)
        .await
        .expect("their owner exists");
    let mut board = Container {
        id: new_id(),
        workspace_id: workspace.id.clone(),
        visibility: ContainerVisibility::Private,
        title: "Other organization's confidential board".to_owned(),
        ..Container::default()
    };
    store
        .insert_container_with_admin(&mut board, &owner.id)
        .await
        .expect("their board exists");
    (store, workspace, owner, board)
}

/// Keep client2's real session, but make the user belong only to the second org.
async fn move_second_user(ts: &TestServer, store: &Store, workspace: &Workspace) {
    let user_id = ts.client2.me().await.id;
    for table in ["workspace_members", "organization_members"] {
        ts.app
            .store()
            .execute_unchecked(&format!("DELETE FROM {table} WHERE user_id = '{user_id}'"))
            .await
            .expect("memberships of the first organization are removed");
    }
    store
        .add_organization_member(&user_id, OrgRole::Member)
        .await
        .expect("the user belongs to the second organization");
    store
        .add_workspace_member(&workspace.id, &user_id, WorkspaceRole::Member)
        .await
        .expect("the user belongs to its workspace");
    assert!(
        ts.app
            .store()
            .resolve_workspace(&user_id, ROOT_WORKSPACE_ID)
            .await
            .expect("membership is checked")
            .is_none(),
        "the user has no access to the first workspace"
    );
}

#[tokio::test]
async fn another_organizations_user_cannot_discover_the_oldest_workspace() {
    if db_type_of_the_tests() != DbType::Postgres {
        return;
    }
    let ts = Setup::native()
        .configure(open)
        .start()
        .await
        .init_basic()
        .await;
    let (store, workspace, _, _) = other_organization(&ts).await;
    move_second_user(&ts, &store, &workspace).await;
    ts.client2
        .get(&format!("{ROOT_WS}/boards"))
        .await
        .check_not_found();

    let workspace = ts.client2.get(ROOT_WS).await;
    let boards = ts.client2.get(&format!("{ROOT_WS}/boards")).await;
    let workspaces: Vec<UserWorkspace> = ts.client2.get("/users/me/workspaces").await.ok_json();
    ts.teardown().await;

    assert_eq!(
        (
            workspace.status,
            boards.status,
            workspaces
                .iter()
                .any(|workspace| workspace.id == ROOT_WORKSPACE_ID)
        ),
        (StatusCode::NOT_FOUND, StatusCode::NOT_FOUND, false),
        "legacy routes must hide a workspace from a user of another organization"
    );
}

#[tokio::test]
async fn another_organizations_user_cannot_join_and_edit_an_open_board() {
    if db_type_of_the_tests() != DbType::Postgres {
        return;
    }
    let ts = Setup::native()
        .configure(open)
        .start()
        .await
        .init_basic()
        .await;
    let board = ts
        .create_board(ROOT_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let (store, workspace, _, _) = other_organization(&ts).await;
    move_second_user(&ts, &store, &workspace).await;

    let joined = ts
        .client2
        .post(
            &format!("{ROOT_WS}/containers/{}/join", board.id),
            &json!({}),
        )
        .await;
    let changed = ts
        .client2
        .patch(
            &format!("{ROOT_WS}/containers/{}", board.id),
            &json!({"title": "Changed by another organization"}),
        )
        .await;
    let stored = ts
        .app
        .store()
        .get_container(&board.id)
        .await
        .expect("the original board exists");
    ts.teardown().await;

    assert_eq!(
        (joined.status, changed.status, stored.title),
        (StatusCode::NOT_FOUND, StatusCode::NOT_FOUND, board.title),
        "an unrelated organization's session must neither join nor change this board"
    );
}

#[tokio::test]
async fn a_workspace_guest_cannot_gain_editor_access_through_the_legacy_join_route() {
    let ts = Setup::native().start().await.init_basic().await;
    let board = ts
        .create_board(ROOT_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let user_id = ts.client2.me().await.id;
    ts.app
        .store()
        .execute_unchecked(&format!(
            "UPDATE workspace_members SET role = 'guest' WHERE user_id = '{user_id}'"
        ))
        .await
        .expect("the workspace role becomes guest");
    let visible: Vec<Container> = ts.client2.get(&format!("{ROOT_WS}/boards")).await.ok_json();
    assert!(visible.is_empty(), "the guest is not on this open board");

    let joined = ts
        .client2
        .post(
            &format!("{ROOT_WS}/containers/{}/join", board.id),
            &json!({}),
        )
        .await;
    let changed = ts
        .client2
        .patch(
            &format!("{ROOT_WS}/containers/{}", board.id),
            &json!({"title": "Changed by a guest"}),
        )
        .await;
    let stored = ts
        .app
        .store()
        .get_container(&board.id)
        .await
        .expect("the original board exists");
    ts.teardown().await;

    assert_eq!(
        (joined.status, changed.status, stored.title),
        (StatusCode::NOT_FOUND, StatusCode::NOT_FOUND, board.title),
        "the guest-only workspace role must also constrain legacy routes"
    );
}

#[tokio::test]
async fn a_workspace_user_search_and_profile_lookup_do_not_expose_another_organizations_users() {
    if db_type_of_the_tests() != DbType::Postgres {
        return;
    }
    let ts = Setup::native()
        .configure(open)
        .start()
        .await
        .init_basic()
        .await;
    let (_, _, owner, _) = other_organization(&ts).await;
    let mut found: Vec<User> = Vec::new();
    // By the login name, and by every part of the name.
    for query in [
        "confidential.other",
        "Confidential-First",
        "confidential-last",
        "Confidential-Latin",
        "owner",
    ] {
        let mut answer: Vec<User> = ts
            .client2
            .get(&format!("{ROOT_WS}/users?search={query}"))
            .await
            .ok_json();
        found.append(&mut answer);
    }
    let profile = ts
        .client2
        .get(&format!("{ROOT_WS}/users/{}", owner.id))
        .await;
    let by_ids = ts
        .client2
        .post(&format!("{ROOT_WS}/users"), &vec![owner.id.clone()])
        .await;
    let by_nobody = ts
        .client2
        .post(&format!("{ROOT_WS}/users"), &vec![UNKNOWN_ID.to_owned()])
        .await;
    ts.teardown().await;

    assert_eq!(
        (
            found.iter().any(|user| user.id == owner.id),
            profile.status,
            by_ids.status
        ),
        (false, StatusCode::NOT_FOUND, StatusCode::OK),
        "the ordinary user shares neither a workspace nor an organization with this owner"
    );
    // Left out of the list as somebody who does not exist is.
    assert_eq!(by_ids.text(), "[]");
    assert_eq!(
        (by_ids.status, by_ids.text()),
        (by_nobody.status, by_nobody.text())
    );
}

#[tokio::test]
async fn a_valid_shared_link_reads_the_board_in_its_own_organization() {
    if db_type_of_the_tests() != DbType::Postgres {
        return;
    }
    let ts = Setup::local_mode()
        .configure(open)
        .start()
        .await
        .init_basic()
        .await;
    let (store, _, owner, board) = other_organization(&ts).await;
    let sharing = store
        .set_public_link(
            &board.id,
            PublicLinkChange::On,
            &fvoci_server::auth::secret::new_secret(),
            &owner.id,
        )
        .await
        .expect("the board is shared");
    assert!(
        ts.app
            .authenticator()
            .is_valid_read_token(&board.id, &sharing.token)
            .await
            .expect("the token is checked"),
        "the read token is valid"
    );
    let response = ts
        .anonymous()
        .get(&format!("/shared/{}?readToken={}", board.id, sharing.token))
        .await;
    ts.teardown().await;

    response.check_ok();
    assert_eq!(
        response.value()["id"],
        board.id,
        "the token opens its board"
    );
}

async fn websocket_organization(ts: &TestServer) -> (Store, Workspace, User, Container) {
    if db_type_of_the_tests() == DbType::Postgres {
        other_organization(ts).await
    } else {
        (
            ts.app.store().clone(),
            Workspace {
                id: ROOT_WORKSPACE_ID.to_owned(),
                ..Workspace::default()
            },
            ts.client.me().await,
            ts.create_board(ROOT_WORKSPACE_ID, ContainerVisibility::Private)
                .await,
        )
    }
}

async fn admit_workspace_member(store: &Store, workspace_id: &str, user_id: &str) {
    if store
        .resolve_workspace(user_id, workspace_id)
        .await
        .expect("workspace lookup")
        .is_none()
    {
        store
            .add_workspace_member(workspace_id, user_id, WorkspaceRole::Member)
            .await
            .expect("initial workspace membership");
    }
}

#[tokio::test]
async fn removing_workspace_membership_stops_websocket_content_from_that_organization() {
    let setup = Setup::native();
    let setup = if db_type_of_the_tests() == DbType::Postgres {
        setup.configure(open)
    } else {
        setup
    };
    let ts = setup.start().await.init_basic().await;
    let (store, workspace, owner, board) = websocket_organization(&ts).await;
    let user_id = ts.client2.me().await.id;
    admit_workspace_member(&store, &workspace.id, &user_id).await;
    store
        .save_member(&ContainerMember {
            user_id: user_id.clone(),
            container_id: board.id.clone(),
            role: fvoci_model::ContainerRole::Viewer,
            synthetic: false,
            inherited_role: None,
        })
        .await
        .expect("initial board membership");
    let mut socket = ts.client2.connect_websocket().await;
    socket
        .send(Message::Text(
            json!({"type": "subscribeWorkspace", "workspaceId": workspace.id})
                .to_string()
                .into(),
        ))
        .await
        .expect("subscription sent");
    // The subscription is registered before the membership ends.
    common::carried_out(ts.app.ws(), &mut socket).await;

    store
        .execute_unchecked(&format!(
            "DELETE FROM workspace_members WHERE user_id = '{user_id}'"
        ))
        .await
        .expect("workspace membership is revoked");
    ts.client2
        .get(&format!("/w/{}/boards", workspace.id))
        .await
        .check_not_found();
    let mut block = Block {
        id: new_id(),
        container_id: board.id.clone(),
        block_type: BlockType::Card,
        title: "Confidential change after workspace removal".to_owned(),
        ..Block::default()
    };
    store
        .insert_block(&mut block, &owner.id)
        .await
        .expect("an authorized user creates a card");
    let org_id = match store.tenant() {
        Tenant::Org(id) => id,
        tenant => panic!("expected an organization, got {tenant:?}"),
    };
    ts.app
        .scoped(Tenant::Org(org_id))
        .ws()
        .broadcast_block_change(&workspace.id, &block)
        .await;
    let leaked = tokio::time::timeout(Duration::from_millis(500), async {
        while let Some(message) = socket.next().await {
            if let Ok(Message::Text(text)) = message {
                let value: Value = serde_json::from_str(&text).expect("a JSON update");
                if value["block"]["id"] == block.id {
                    return Some(value);
                }
            }
        }
        None
    })
    .await
    .ok()
    .flatten();
    let _ = socket.close(None).await;
    ts.teardown().await;

    assert!(
        leaked.is_none(),
        "a removed workspace member received another organization's card: {leaked:?}"
    );
}

#[tokio::test]
async fn a_hidden_workspace_in_the_same_organization_also_rejects_legacy_board_creation() {
    let ts = Setup::native().start().await.init_basic().await;
    ts.client2
        .get(&format!("/w/{HIDDEN_WORKSPACE_ID}/boards"))
        .await
        .check_not_found();
    let create_in = async |workspace_id: &str| {
        ts.client2
            .post(
                &format!("/w/{workspace_id}/boards"),
                &json!({"visibility": "private", "title": "Created inside a hidden workspace"}),
            )
            .await
    };
    let hidden = create_in(HIDDEN_WORKSPACE_ID).await;
    let absent = create_in(UNKNOWN_ID).await;
    // How many boards the hidden workspace has: a statement that changes nothing, for the
    // number of rows it finds.
    let boards = ts
        .app
        .store()
        .execute_unchecked(&format!(
            "UPDATE containers SET title = title WHERE workspace_id = '{HIDDEN_WORKSPACE_ID}'"
        ))
        .await
        .expect("the boards of the hidden workspace are counted");
    ts.teardown().await;
    // The path names the workspace: a hidden one is not there, as one that does not exist.
    hidden.check_error(StatusCode::NOT_FOUND, "workspace not found");
    assert_eq!(
        (hidden.status, hidden.error_message()),
        (absent.status, absent.error_message()),
        "a hidden workspace answers as one that does not exist"
    );
    assert_eq!(boards, 0, "and no board is made in it");
}
