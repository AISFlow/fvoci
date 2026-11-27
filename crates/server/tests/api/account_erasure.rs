//! Erasing an account: by the housekeeping once its wait ran out, and at once on the
//! operator's word; what is left of it for the people it worked with.

use fvoci_model::{
    Block, BlockType, Container, ContainerVisibility, PersonName, Title, User, new_id,
};
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::admin::socket::{socket_path, socket_request};
use crate::common::{
    Client, PASSWORD, ROOT_WORKSPACE_ID, ROOT_WS, Setup, TestServer, USER2_USERNAME,
};

const DELETION: &str = "/users/me/deletion";

/// A server with `user1` and `user2` and the administration socket at the returned path.
async fn with_socket() -> (TestServer, std::path::PathBuf) {
    let socket = socket_path();
    let th = Setup::native()
        .admin_socket(&socket)
        .start()
        .await
        .init_basic()
        .await;
    (th, socket)
}

/// An open board of the root workspace made by the client's user, with a card of theirs.
async fn board_with_a_card(client: &Client) -> (Container, Block) {
    let board: Container = client
        .create_board(&Container {
            workspace_id: ROOT_WORKSPACE_ID.to_owned(),
            visibility: ContainerVisibility::Open,
            title: "a board".to_owned(),
            ..Container::default()
        })
        .await
        .created_json();
    let card = Block {
        id: new_id(),
        container_id: board.id.clone(),
        block_type: BlockType::Card,
        title: "a card".to_owned(),
        created_at: 1,
        updated_at: 1,
        ..Block::default()
    };
    let stored = crate::blocks::insert_in(
        client,
        ROOT_WORKSPACE_ID,
        &board.id,
        std::slice::from_ref(&card),
    )
    .await;
    (
        board,
        stored.into_iter().next().expect("the card is stored"),
    )
}

/// Who the client sees of the identifiers, asked by identifier.
async fn named(client: &Client, user_ids: &[&str]) -> Vec<User> {
    client
        .post(&format!("{ROOT_WS}/users"), &user_ids)
        .await
        .ok_json()
}

fn erase(username: &str) -> String {
    format!("/api/v1/admin/users/{username}/erase")
}

/// What is left of the second user's account opens nothing: somebody's own space is to
/// its session what a workspace that is not there is.
async fn check_it_opens_no_own_space(th: &TestServer) {
    let space = crate::own_space::own_workspace(&th.client).await;
    th.client.get(&format!("/w/{}", space.id)).await.check_ok();
    let hidden = th.client2.get(&format!("/w/{}", space.id)).await;
    let absent = th
        .client2
        .get(&format!("/w/{}", crate::common::UNKNOWN_ID))
        .await;
    hidden.check_unauthorized();
    let said = |response: &crate::common::ApiResponse| {
        let problem = response.value();
        (problem["type"].clone(), problem["detail"].clone())
    };
    assert_eq!(
        said(&hidden),
        said(&absent),
        "as a workspace that is not there"
    );
}

/// The operator erases an account at once: nobody signs in to it, its name is free,
/// what it wrote stays, and whoever reads that is told an account that is gone, by its
/// identifier alone. The instance's only administrator stays.
#[tokio::test]
async fn the_operator_erases_an_account_and_what_it_wrote_stays_under_no_name() {
    let (th, socket) = with_socket().await;
    let user2 = th.client2.me().await;
    let (board, card) = board_with_a_card(&th.client2).await;
    th.client
        .post_empty(&format!("{ROOT_WS}/containers/{}/join", board.id))
        .await
        .check_ok();
    let full_name = json!({
        "firstName": "Leaver-First", "lastName": "Leaver-Last",
        "latinFirstName": "Leaver-Latin-First", "latinLastName": "Leaver-Latin-Last",
        "customName": "Leaver Shown", "title": "prof", "credentials": ["phd", "kr_cpa"],
    });
    th.client2.patch("/users/me", &full_name).await.check_ok();
    let before = named(&th.client, &[&user2.id]).await;
    assert_eq!(before[0].username, USER2_USERNAME);
    assert_eq!(before[0].name.title, Some(Title::Prof), "seen by a member");
    let agent = crate::api_tokens::agent_reading(&th, &th.client2).await;

    socket_request(&socket, "POST", &erase("USER2"), "")
        .await
        .check_no_content();

    th.client2.get("/users/me").await.check_unauthorized();
    crate::api_tokens::check_token_ended(&agent).await;
    let mut signing_in = th.anonymous();
    signing_in
        .login(USER2_USERNAME, PASSWORD)
        .await
        .check_unauthorized();
    check_it_opens_no_own_space(&th).await;
    // What they wrote is there, under their identifier, which names nobody.
    let cards: Vec<Block> = th
        .client
        .get(&format!("{ROOT_WS}/boards/{}/blocks?all=true", board.id))
        .await
        .ok_json();
    let kept = cards
        .iter()
        .find(|block| block.id == card.id)
        .expect("the card stays");
    assert_eq!(kept.created_by, user2.id);
    let gone = named(&th.client, &[&user2.id]).await;
    assert_eq!(gone.len(), 1, "told as an account that is gone: {gone:?}");
    assert_eq!(gone[0].id, user2.id);
    assert_eq!(
        (
            gone[0].username.as_str(),
            gone[0].email.as_str(),
            gone[0].display_name.as_str()
        ),
        ("", "", ""),
        "nothing of the person"
    );
    assert_eq!(
        gone[0].name,
        PersonName::default(),
        "nothing of the name, not even which of it was shown"
    );
    assert_eq!(
        (gone[0].created_at, gone[0].updated_at, gone[0].deleted_at),
        (0, 0, None),
        "nor when it came, or went"
    );
    // Among the people of the workspace, and in a search, they are not.
    let people: Vec<User> = th.client.get(&format!("{ROOT_WS}/users")).await.ok_json();
    assert_eq!(
        people
            .iter()
            .map(|user| user.username.as_str())
            .collect::<Vec<_>>(),
        ["user1"]
    );
    for query in ["user2", "Leaver-First", "Leaver-Latin", "Leaver Shown"] {
        let found: Vec<User> = th
            .client
            .get(&format!("{ROOT_WS}/users?search={query}"))
            .await
            .ok_json();
        assert!(found.is_empty(), "{query}: {found:?}");
    }

    // There is nothing to erase twice, and nobody of that name.
    socket_request(&socket, "POST", &erase(USER2_USERNAME), "")
        .await
        .check_error(404, "not-found", "user not found");
    // The name and the address are free for somebody else.
    let again = th.add_user(USER2_USERNAME).await;
    assert_ne!(again.me().await.id, user2.id);
    // The instance keeps its administrator, whatever the operator says.
    let refused = socket_request(&socket, "POST", &erase("user1"), "").await;
    assert_eq!(refused.status, 409, "{refused:?}");
    assert!(
        refused.body.contains("\"reason\":\"last_admin\""),
        "{refused:?}"
    );
    th.client.get("/users/me").await.check_ok();
    th.teardown().await;
}

/// The housekeeping erases an account once its wait ran out, and not before; signing
/// in is over then, and the way back with it.
#[tokio::test]
async fn the_housekeeping_erases_an_account_whose_wait_ran_out() {
    let th = Setup::native().start().await.init_basic().await;
    let user2 = th.client2.me().await;
    th.client2
        .post(
            DELETION,
            &json!({"username": USER2_USERNAME, "password": PASSWORD}),
        )
        .await
        .check_ok();

    fvoci_server::keep_house(&th.app).await;
    let back = th.login(USER2_USERNAME, PASSWORD).await;
    let waiting: Value = back.get(DELETION).await.ok_json();
    assert!(
        waiting["deleteAfter"].as_i64().is_some(),
        "it waits: {waiting}"
    );

    th.app
        .store()
        .execute_unchecked(&format!(
            "UPDATE users SET delete_after = 1 WHERE id = '{}'",
            user2.id
        ))
        .await
        .expect("the wait is over");
    fvoci_server::keep_house(&th.app).await;

    let response = back.get(DELETION).await;
    response.check_unauthorized();
    assert_eq!(
        response.error_reason(),
        "",
        "the session went with the account"
    );
    back.delete(DELETION).await.check_unauthorized();
    let mut signing_in = th.anonymous();
    signing_in
        .login(USER2_USERNAME, PASSWORD)
        .await
        .check_unauthorized();
    let left = th
        .app
        .store()
        .texts_unchecked(&format!(
            "SELECT username || '|' || email FROM users WHERE id = '{}' AND erased_at > 0",
            user2.id
        ))
        .await
        .expect("read");
    assert_eq!(left, ["|"], "the row is its identifier");
    let members: Vec<Value> = th.client.get(&format!("{ROOT_WS}/members")).await.ok_json();
    assert_eq!(members.len(), 1, "out of the workspace: {members:?}");
    assert_eq!(
        th.client.get("/users/me").await.status,
        StatusCode::OK,
        "somebody else's session goes on"
    );
    th.teardown().await;
}

/// Somebody who forgot their password while their account waits is helped by the
/// operator, who sets one on the administration socket: with it they sign in, and take
/// the deletion back.
#[tokio::test]
async fn the_operator_sets_the_password_of_an_account_that_waits() {
    let (th, socket) = with_socket().await;
    th.client2
        .post(
            DELETION,
            &json!({"username": USER2_USERNAME, "password": PASSWORD}),
        )
        .await
        .check_ok();

    let another = "Another-password-1";
    socket_request(
        &socket,
        "POST",
        &format!("/api/v1/admin/users/{USER2_USERNAME}/password"),
        &json!({ "password": another }).to_string(),
    )
    .await
    .check_no_content();

    let mut old = th.anonymous();
    old.login(USER2_USERNAME, PASSWORD)
        .await
        .check_unauthorized();
    let back = th.login(USER2_USERNAME, another).await;
    let waiting: Value = back.get(DELETION).await.ok_json();
    assert!(
        waiting["deleteAfter"].as_i64().is_some(),
        "it waits still: {waiting}"
    );
    back.delete(DELETION)
        .await
        .check_status(StatusCode::NO_CONTENT);
    back.get("/users/me").await.check_ok();
    th.teardown().await;
}

/// What is left of an erased account tells nobody when it came or went: whoever asks by
/// its identifier, of its organization or (on PostgreSQL) of another, is answered the
/// identifier, no name and its three times zero.
#[tokio::test]
async fn an_erased_account_tells_nobody_when_it_came_or_went() {
    let (th, socket) = with_socket().await;
    let user2 = th.client2.me().await.id;
    let elsewhere = if th.app.store().db_type() == fvoci_store::DbType::Postgres {
        Some(of_another_organization(&th).await)
    } else {
        None
    };
    socket_request(&socket, "POST", &erase(USER2_USERNAME), "")
        .await
        .check_no_content();

    let mut answers = vec![named(&th.client, &[&user2]).await];
    if let Some((client, workspace_id)) = &elsewhere {
        answers.push(
            client
                .post(&format!("/w/{workspace_id}/users"), &[&user2])
                .await
                .ok_json(),
        );
    }
    for gone in answers {
        let told: Vec<_> = gone
            .iter()
            .map(|user| {
                (
                    user.id.as_str(),
                    user.username.as_str(),
                    user.created_at,
                    user.updated_at,
                    user.deleted_at,
                )
            })
            .collect();
        assert_eq!(told, [(user2.as_str(), "", 0, 0, None)]);
    }
    th.teardown().await;
}

/// A third account, made a member of a second organization and of its workspace only:
/// its client and that workspace's identifier.
async fn of_another_organization(th: &TestServer) -> (Client, String) {
    let (theirs, workspace_id, _) = crate::workspaces::another_organization(th).await;
    let client = th.add_user("user3").await;
    let user_id = client.me().await.id;
    for table in ["workspace_members", "organization_members"] {
        th.app
            .store()
            .execute_unchecked(&format!("DELETE FROM {table} WHERE user_id = '{user_id}'"))
            .await
            .expect("out of the first organization");
    }
    theirs
        .add_organization_member(&user_id, fvoci_model::OrgRole::Member)
        .await
        .expect("a member of the second");
    theirs
        .add_workspace_member(&workspace_id, &user_id, fvoci_model::WorkspaceRole::Member)
        .await
        .expect("a member of its workspace");
    assert!(
        th.app
            .store()
            .resolve_workspace(&user_id, ROOT_WORKSPACE_ID)
            .await
            .expect("membership is checked")
            .is_none(),
        "out of the first organization's workspace"
    );
    client
        .get(&format!("{ROOT_WS}/boards"))
        .await
        .check_not_found();
    (client, workspace_id)
}

/// Erasing an account whose own space holds uploaded files deletes those files from the
/// backing file storage (disk or S3) while keeping files in other organizations and
/// spaces intact.
async fn erasing_an_account_removes_its_own_space_files(ts: &TestServer) {
    let user2 = ts.client2.me().await;
    let own2 = crate::own_space::own_workspace(&ts.client2).await;
    let own1 = crate::own_space::own_workspace(&ts.client).await;

    let make_board_and_upload = async |client: &Client, workspace_id: &str, org_id: &str| {
        let board: Container = client
            .create_board(&Container {
                workspace_id: workspace_id.to_owned(),
                visibility: ContainerVisibility::Private,
                title: "with file".to_owned(),
                ..Container::default()
            })
            .await
            .created_json();
        let response = crate::files::upload_in(
            client,
            workspace_id,
            &board.id,
            "note.txt",
            b"own space file content",
        )
        .await;
        response.check_created();
        let file_id = crate::files::uploaded_id(&response);
        let store = ts.app.store().for_org(org_id).expect("org id");
        store
            .get_file_record(&board.id, &file_id)
            .await
            .expect("recorded")
            .path
    };

    let own2_path = make_board_and_upload(&ts.client2, &own2.id, &own2.org_id).await;
    let own1_path = make_board_and_upload(&ts.client, &own1.id, &own1.org_id).await;
    let (_, org_path) = crate::loose_objects::uploaded(ts, b"shared org file content").await;
    for path in [&own2_path, &own1_path, &org_path] {
        assert!(
            crate::loose_objects::exists(ts, path).await,
            "uploaded file exists before erasure: {path}"
        );
    }

    ts.client2
        .post(
            DELETION,
            &json!({"username": USER2_USERNAME, "password": PASSWORD}),
        )
        .await
        .check_ok();
    ts.app
        .store()
        .execute_unchecked(&format!(
            "UPDATE users SET delete_after = 1 WHERE id = '{}'",
            user2.id
        ))
        .await
        .expect("wait elapsed");

    fvoci_server::keep_house(&ts.app).await;

    assert!(
        !crate::loose_objects::exists(ts, &own2_path).await,
        "erased account's own-space file must be deleted from storage: {own2_path}"
    );
    assert!(
        crate::loose_objects::exists(ts, &own1_path).await,
        "another account's own-space file must remain"
    );
    assert!(
        crate::loose_objects::exists(ts, &org_path).await,
        "organization workspace file must remain"
    );
    assert_eq!(
        ts.app
            .housekeeping()
            .store()
            .loose_objects()
            .await
            .expect("listed"),
        [],
        "no doomed objects remain"
    );
}

#[tokio::test]
async fn erasing_an_account_removes_its_own_space_files_on_disk() {
    let ts = Setup::native().start().await.init_basic().await;
    erasing_an_account_removes_its_own_space_files(&ts).await;
    ts.teardown().await;
}

#[tokio::test]
#[ignore = "needs an S3-compatible server; see tests/storage.rs"]
async fn with_s3_erasing_an_account_removes_its_own_space_files() {
    let ts = crate::uploads::s3_server().await;
    erasing_an_account_removes_its_own_space_files(&ts).await;
    ts.teardown().await;
}
