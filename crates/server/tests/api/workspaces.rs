//! The workspaces of a user and the routes under `/w/{workspaceId}`: the first routes
//! behind the wall of the workspace. A user who is no member of a workspace is told
//! what somebody is told who names none (`visibility.rs` holds the other routes to the
//! same rule).

use fvoci_model::{
    Container, ContainerMember, ContainerVisibility, OrgRole, UserWorkspace, Workspace,
    WorkspaceRole,
};
use fvoci_store::{DbType, Store};
use serde_json::{Value, json};

use crate::common::ROOT_WS;
use crate::common::TEST_WS;
use crate::common::{
    ApiResponse, Client, HIDDEN_WORKSPACE_ID, ROOT_WORKSPACE_ID, Setup, TEST_WORKSPACE_ID,
    TestServer, UNKNOWN_ID,
};

/// The answer to a request for the boards of a workspace that is not there for the
/// user: the problem `not-found`, and the same words whatever the reason. What names
/// the one request (`instance`, `requestId`) is the only thing that differs.
#[track_caller]
fn not_there(response: &ApiResponse) -> Value {
    response.check_not_found();
    let mut body = response.check_problem();
    assert_eq!(body["type"], "/problems/not-found", "{body}");
    let members = body.as_object_mut().expect("a problem");
    members.remove("instance");
    members.remove("requestId");
    body
}

/// The identifiers of the boards an answer lists.
fn ids(boards: &[Container]) -> Vec<&str> {
    boards.iter().map(|board| board.id.as_str()).collect()
}

/// A user finds the workspaces they are a member of, with what they are in each, and
/// nobody finds them without a session.
#[tokio::test]
async fn a_user_lists_the_workspaces_they_are_a_member_of() {
    let ts = Setup::native().start().await.init_basic().await;

    let (first, _) = crate::own_space::workspaces_and_own(&ts.client).await;
    let (second, _) = crate::own_space::workspaces_and_own(&ts.client2).await;
    // Whoever registers gets into the default workspace, the root workspace, which the
    // first user administers; the test server lets its users into its second workspace too.
    let of = |workspaces: &[UserWorkspace]| -> Vec<(String, WorkspaceRole)> {
        workspaces
            .iter()
            .map(|workspace| (workspace.id.clone(), workspace.role))
            .collect()
    };
    let in_both = |root_role| {
        vec![
            (ROOT_WORKSPACE_ID.to_owned(), root_role),
            (TEST_WORKSPACE_ID.to_owned(), WorkspaceRole::Member),
        ]
    };
    assert_eq!(of(&first), in_both(WorkspaceRole::Admin), "the first user");
    assert_eq!(
        of(&second),
        in_both(WorkspaceRole::Member),
        "the second user"
    );
    assert_eq!(first[0].org_id, second[0].org_id, "one organization");
    assert_eq!(
        ts.client.get("/users/me/workspaces").await.value()[0],
        serde_json::json!({
            "id": ROOT_WORKSPACE_ID,
            "orgId": first[0].org_id,
            "kind": "shared",
            "name": crate::common::ORG_NAME,
            "role": "admin",
            "ssoRequired": false,
            "passwordRequired": false,
        }),
        "the members of a workspace in the list"
    );

    // A further workspace of the organization shows once the user is let into it.
    let user2 = ts.client2.me().await.id;
    ts.app
        .store()
        .add_workspace_member(HIDDEN_WORKSPACE_ID, &user2, WorkspaceRole::Guest)
        .await
        .expect("the user is let in");
    let (second, _) = crate::own_space::workspaces_and_own(&ts.client2).await;
    let mut expected = in_both(WorkspaceRole::Member);
    expected.push((HIDDEN_WORKSPACE_ID.to_owned(), WorkspaceRole::Guest));
    assert_eq!(of(&second), expected, "oldest workspace first");

    ts.anonymous()
        .get("/users/me/workspaces")
        .await
        .check_unauthorized();
    ts.teardown().await;
}

/// The boards of a workspace are for its members. For anybody else the workspace is
/// not there: a workspace of the same organization that the user is not in, an
/// identifier nobody has, and text that is no identifier all answer alike.
#[tokio::test]
async fn the_boards_of_a_workspace_are_for_its_members() {
    let ts = Setup::native().start().await.init_basic().await;
    let open = ts
        .create_board(ROOT_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let private = ts
        .create_board(ROOT_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    // A board of the hidden workspace, which no user of the test could make there.
    let stranger = ts.stranger().await;
    let mut elsewhere = Container {
        id: fvoci_model::new_id(),
        workspace_id: HIDDEN_WORKSPACE_ID.to_owned(),
        visibility: ContainerVisibility::Open,
        ..Container::default()
    };
    ts.app
        .store()
        .insert_container(&mut elsewhere, &stranger)
        .await
        .expect("a board of the hidden workspace is stored");

    // A member gets what the workspace's route gets: the boards they are on, and the open
    // ones.
    let boards: Vec<Container> = ts.client.get(&format!("{ROOT_WS}/boards")).await.ok_json();
    assert_eq!(ids(&boards), [open.id.as_str(), private.id.as_str()]);
    let of_the_workspace: Vec<Container> =
        ts.client.get(&format!("{ROOT_WS}/boards")).await.ok_json();
    assert_eq!(boards, of_the_workspace, "as the workspace's route answers");
    let boards: Vec<Container> = ts.client2.get(&format!("{ROOT_WS}/boards")).await.ok_json();
    assert_eq!(ids(&boards), [open.id.as_str()], "the open board only");

    // Nobody is a member of the hidden workspace: it is there for neither user.
    let hidden = not_there(
        &ts.client
            .get(&format!("/w/{HIDDEN_WORKSPACE_ID}/boards"))
            .await,
    );
    assert_eq!(hidden["detail"], "workspace not found", "{hidden}");
    // The system's workspace is nobody's to work in, a member's neither.
    let user1 = ts.client.me().await.id;
    ts.system_store()
        .add_workspace_member(
            fvoci_store::SYSTEM_WORKSPACE_ID,
            &user1,
            WorkspaceRole::Member,
        )
        .await
        .expect("a membership of the system's workspace is stored");
    for nothing in [
        UNKNOWN_ID,
        "no-identifier",
        "%FF",
        fvoci_store::SYSTEM_WORKSPACE_ID,
    ] {
        let absent = not_there(&ts.client.get(&format!("/w/{nothing}/boards")).await);
        assert_eq!(absent, hidden, "{nothing} answers as a workspace of others");
    }
    let (listed, _own) = crate::own_space::workspaces_and_own(&ts.client).await;
    assert_eq!(listed.len(), 2, "the system's is not listed: {listed:?}");
    ts.anonymous()
        .get(&format!("{ROOT_WS}/boards"))
        .await
        .check_unauthorized();
    ts.anonymous()
        .get(&format!("/w/{UNKNOWN_ID}/boards"))
        .await
        .check_unauthorized();

    // Let in, the user finds the workspace and its boards; a guest finds only the
    // boards they are on.
    let user2 = ts.client2.me().await.id;
    let store = ts.app.store();
    store
        .add_workspace_member(HIDDEN_WORKSPACE_ID, &user1, WorkspaceRole::Member)
        .await
        .expect("the first user is let in");
    store
        .add_workspace_member(HIDDEN_WORKSPACE_ID, &user2, WorkspaceRole::Guest)
        .await
        .expect("the second user is let in as a guest");
    let boards: Vec<Container> = ts
        .client
        .get(&format!("/w/{HIDDEN_WORKSPACE_ID}/boards"))
        .await
        .ok_json();
    assert_eq!(ids(&boards), [elsewhere.id.as_str()]);
    let boards: Vec<Container> = ts
        .client2
        .get(&format!("/w/{HIDDEN_WORKSPACE_ID}/boards"))
        .await
        .ok_json();
    assert!(
        boards.is_empty(),
        "an open board is not a guest's: {boards:?}"
    );

    ts.teardown().await;
}

/// A second organization on the server of a test, with a workspace and an open board
/// in it that a user of its own made. Returns a handle that reaches its rows, and the
/// identifiers of the workspace and the board.
pub(crate) async fn another_organization(ts: &TestServer) -> (Store, String, String) {
    let workspace = Workspace {
        id: fvoci_model::new_id(),
        title: "theirs".to_owned(),
        ..Workspace::default()
    };
    let org_id = ts
        .app
        .store()
        .create_organization("another", &workspace)
        .await
        .expect("a second organization is made");
    let theirs = ts.app.store().for_org(&org_id).expect("an identifier");
    let mut owner = fvoci_model::User {
        id: fvoci_model::new_id(),
        username: "owner.elsewhere".to_owned(),
        email: "owner.elsewhere@sample.com".to_owned(),
        ..fvoci_model::User::default()
    };
    theirs
        .create_user(&mut owner)
        .await
        .expect("their user is stored");
    let mut board = Container {
        id: fvoci_model::new_id(),
        workspace_id: workspace.id.clone(),
        visibility: ContainerVisibility::Open,
        title: "theirs".to_owned(),
        ..Container::default()
    };
    theirs
        .insert_container_with_admin(&mut board, &owner.id)
        .await
        .expect("their board is stored");
    (theirs, workspace.id, board.id)
}

/// The workspaces a user lists of the organizations they are in, by identifier: the
/// one of their own space, which is listed after them, is not among these.
async fn workspaces_of(client: &Client) -> Vec<String> {
    let (listed, _own) = crate::own_space::workspaces_and_own(client).await;
    listed.into_iter().map(|workspace| workspace.id).collect()
}

/// A workspace of another organization is not there, and answers as one that nobody
/// has. For a user who is let into it, the route works in that organization: it lists
/// the boards there, and its board is read under it, not under the server's workspace.
#[tokio::test]
async fn a_workspace_of_another_organization_is_reached_by_its_members_only() {
    // SQLite holds one organization.
    if fvoci_store::testing::db_type_of_the_tests() != DbType::Postgres {
        return;
    }
    let ts = Setup::native().start().await.init_basic().await;
    let (theirs, their_workspace, their_board) = another_organization(&ts).await;
    let mine = ts
        .create_board(ROOT_WORKSPACE_ID, ContainerVisibility::Open)
        .await;

    let their_boards = format!("/w/{their_workspace}/boards");
    let hidden = not_there(&ts.client.get(&their_boards).await);
    let absent = not_there(&ts.client.get(&format!("/w/{UNKNOWN_ID}/boards")).await);
    assert_eq!(hidden, absent, "theirs answers as one nobody has");
    let mine_before = workspaces_of(&ts.client).await;
    assert_eq!(
        mine_before,
        [ROOT_WORKSPACE_ID, TEST_WORKSPACE_ID],
        "theirs is not"
    );

    // Let into their organization and workspace, the user reaches both organizations,
    // each through its own workspace.
    let user1 = ts.client.me().await.id;
    theirs
        .add_organization_member(&user1, OrgRole::Member)
        .await
        .expect("the user joins their organization");
    theirs
        .add_workspace_member(&their_workspace, &user1, WorkspaceRole::Member)
        .await
        .expect("the user is let into their workspace");
    let (listed, _own) = crate::own_space::workspaces_and_own(&ts.client).await;
    let found: Vec<(&str, &str)> = listed
        .iter()
        .map(|workspace| (workspace.id.as_str(), workspace.name.as_str()))
        .collect();
    assert_eq!(
        found,
        [
            (ROOT_WORKSPACE_ID, crate::common::ORG_NAME),
            (TEST_WORKSPACE_ID, ""),
            (their_workspace.as_str(), "theirs")
        ],
        "the older organization first"
    );
    assert_ne!(listed[0].org_id, listed[2].org_id, "two organizations");

    let boards: Vec<Container> = ts.client.get(&their_boards).await.ok_json();
    assert_eq!(ids(&boards), [their_board.as_str()], "their open board");
    let boards: Vec<Container> = ts.client.get(&format!("{ROOT_WS}/boards")).await.ok_json();
    assert_eq!(ids(&boards), [mine.id.as_str()], "and mine in mine");
    // The second user was let into nothing.
    not_there(&ts.client2.get(&their_boards).await);
    // Let into the workspace as a guest, without belonging to their organization, the
    // user finds the workspace, and in it the boards they are on: none.
    let user2 = ts.client2.me().await.id;
    theirs
        .add_workspace_member(&their_workspace, &user2, WorkspaceRole::Guest)
        .await
        .expect("the second user is let in as a guest");
    let boards: Vec<Container> = ts.client2.get(&their_boards).await.ok_json();
    assert!(boards.is_empty(), "a guest is on no board: {boards:?}");
    let of_the_guest = workspaces_of(&ts.client2).await;
    assert!(of_the_guest.contains(&their_workspace), "{of_the_guest:?}");

    // The order of the list is by the age of the organization, not by its identifier:
    // made older than the server's, theirs comes first.
    let aged = theirs
        .execute_unchecked("UPDATE organizations SET created_at = 1")
        .await
        .expect("their organization is made the older");
    assert_eq!(aged, 1, "their organization, and no other");
    assert_eq!(
        workspaces_of(&ts.client).await,
        [
            their_workspace.as_str(),
            ROOT_WORKSPACE_ID,
            TEST_WORKSPACE_ID
        ],
        "the older first"
    );

    // Under the server's workspace their board is not there, also for a member of it;
    // under its own workspace it is.
    theirs
        .save_member(&ContainerMember {
            container_id: their_board.clone(),
            user_id: user1.clone(),
            role: fvoci_model::ContainerRole::Editor,
            synthetic: false,
            inherited_role: None,
        })
        .await
        .expect("the user is put on their board");
    ts.client
        .get(&format!("{ROOT_WS}/containers/{their_board}"))
        .await
        .check_not_found();
    ts.client
        .get(&format!("/w/{their_workspace}/containers/{their_board}"))
        .await
        .check_ok();
    // A body that names their workspace under the server's is refused: the path names it.
    let board = json!({"workspaceId": their_workspace, "visibility": "private", "title": "new"});
    let refused = ts.client.post(&format!("{ROOT_WS}/boards"), &board).await;
    refused.check_error(
        reqwest::StatusCode::BAD_REQUEST,
        "the board is made in the workspace of the path",
    );

    ts.teardown().await;
}

/// A workspace is available to its members in every organization.
#[tokio::test]
async fn workspace_routes_are_available_to_members_of_another_organization() {
    // SQLite holds one organization.
    if fvoci_store::testing::db_type_of_the_tests() != DbType::Postgres {
        return;
    }
    let ts = Setup::native().start().await.init_basic().await;
    let (theirs, their_workspace, _) = another_organization(&ts).await;
    let user1 = ts.client.me().await.id;
    theirs
        .add_organization_member(&user1, OrgRole::Member)
        .await
        .expect("the user joins their organization");
    theirs
        .add_workspace_member(&their_workspace, &user1, WorkspaceRole::Member)
        .await
        .expect("the user is let into their workspace");
    for path in [
        "",
        "/categories",
        "/templates",
        "/users",
        "/trash/containers",
    ] {
        ts.client
            .get(&format!("/w/{their_workspace}{path}"))
            .await
            .check_ok();
    }
    let search = format!("/w/{their_workspace}/boards/search?q=x");
    ts.client.get(&search).await.check_ok();
    let category = json!({"name": "x", "userId": user1, "workspaceId": their_workspace});
    let posted = ts
        .client
        .post(&format!("/w/{their_workspace}/categories"), &category)
        .await;
    posted.check_created();
    let by_ids = ts
        .client
        .post(&format!("/w/{their_workspace}/users"), &json!([user1]))
        .await;
    by_ids.check_ok();
    ts.client
        .get(&format!("/w/{their_workspace}/boards"))
        .await
        .check_ok();
    ts.teardown().await;
}

/// What an answer says, without what differs from one request to the next.
fn said(response: &ApiResponse) -> (reqwest::StatusCode, Value) {
    let mut body: Value = serde_json::from_str(&response.text()).unwrap_or(Value::Null);
    if let Some(members) = body.as_object_mut() {
        members.remove("instance");
        members.remove("requestId");
    }
    (response.status, body)
}

/// The routes that name a workspace answer for a workspace the user is not in as for one that
/// does not exist, each of them: by the status and by every word of the answer.
#[tokio::test]
async fn a_workspace_the_user_is_not_in_answers_as_one_that_is_not_there() {
    let ts = Setup::native().start().await.init_basic().await;
    let user1 = ts.client.me().await.id;
    let nothing = json!({});
    let category =
        |workspace: &str| json!({"name": "mine", "userId": user1, "workspaceId": workspace});
    for route in [
        "",
        "/boards",
        "/boards/search?q=a",
        "/categories",
        "/templates",
        "/trash/containers",
        "/users",
    ] {
        let hidden = ts
            .client
            .get(&format!("/w/{HIDDEN_WORKSPACE_ID}{route}"))
            .await;
        let absent = ts.client.get(&format!("/w/{UNKNOWN_ID}{route}")).await;
        assert_eq!(said(&hidden), said(&absent), "GET /w/<id>{route}");
        hidden.check_error(reqwest::StatusCode::NOT_FOUND, "workspace not found");
    }
    for route in ["/onboard", "/invitations", "/archive/uploads"] {
        let hidden = ts
            .client
            .post(&format!("/w/{HIDDEN_WORKSPACE_ID}{route}"), &nothing)
            .await;
        let absent = ts
            .client
            .post(&format!("/w/{UNKNOWN_ID}{route}"), &nothing)
            .await;
        assert_eq!(said(&hidden), said(&absent), "POST /w/<id>{route}");
        hidden.check_error(reqwest::StatusCode::NOT_FOUND, "workspace not found");
    }
    let hidden = ts
        .client
        .post(
            &format!("/w/{HIDDEN_WORKSPACE_ID}/categories"),
            &category(HIDDEN_WORKSPACE_ID),
        )
        .await;
    let absent = ts
        .client
        .post(
            &format!("/w/{UNKNOWN_ID}/categories"),
            &category(UNKNOWN_ID),
        )
        .await;
    assert_eq!(said(&hidden), said(&absent), "POST /w/<id>/categories");
    hidden.check_error(reqwest::StatusCode::NOT_FOUND, "workspace not found");
    let (listed, _own) = crate::own_space::workspaces_and_own(&ts.client).await;
    let listed: Vec<&str> = listed
        .iter()
        .map(|workspace| workspace.id.as_str())
        .collect();
    assert_eq!(
        listed,
        [ROOT_WORKSPACE_ID, TEST_WORKSPACE_ID],
        "the workspaces of the user"
    );
    ts.teardown().await;
}

/// Stores an open board in a workspace, made by the stranger: the board as it is given,
/// with an identifier of its own.
async fn open_board_in(ts: &TestServer, workspace_id: &str, board: Container) -> Container {
    let stranger = ts.stranger().await;
    let mut board = Container {
        id: fvoci_model::new_id(),
        workspace_id: workspace_id.to_owned(),
        visibility: ContainerVisibility::Open,
        ..board
    };
    ts.app
        .store()
        .insert_container(&mut board, &stranger)
        .await
        .expect("the board is stored");
    board
}

/// An open board is open to who works in its workspace. In a workspace the user is not in, it
/// and an open template answer as a board that does not exist, no search finds it,
/// and the workspace's archive is one without boards.
#[tokio::test]
async fn an_open_board_of_a_workspace_the_user_is_not_in_is_not_there() {
    let ts = Setup::native().start().await.init_basic().await;
    let titled = |title: &str| Container {
        title: title.to_owned(),
        ..Container::default()
    };
    let board = open_board_in(&ts, HIDDEN_WORKSPACE_ID, titled("Concealed plans")).await;
    let template = Container {
        is_template: true,
        ..titled("Concealed template")
    };
    let template = open_board_in(&ts, HIDDEN_WORKSPACE_ID, template).await;
    let nothing = json!({});

    let pair = |id: &str, rest: &str| {
        (
            format!("{ROOT_WS}/boards/{id}{rest}"),
            format!("{ROOT_WS}/boards/{UNKNOWN_ID}{rest}"),
        )
    };
    for (hidden, absent) in [
        pair(&board.id, ""),
        pair(&board.id, "/views"),
        pair(&template.id, "/views"),
    ] {
        let (hidden_answer, absent_answer) =
            (ts.client.get(&hidden).await, ts.client.get(&absent).await);
        assert_eq!(said(&hidden_answer), said(&absent_answer), "GET {hidden}");
        hidden_answer.check_not_found();
    }
    for (hidden, absent) in [
        pair(&board.id, "/join"),
        pair(&template.id, "/duplicate"),
        (
            format!(
                "{ROOT_WS}/containers/{}/duplicate?toWorkspace={ROOT_WORKSPACE_ID}",
                template.id
            ),
            format!("{ROOT_WS}/containers/{UNKNOWN_ID}/duplicate?toWorkspace={ROOT_WORKSPACE_ID}"),
        ),
    ] {
        let hidden_answer = ts.client.post(&hidden, &nothing).await;
        let absent_answer = ts.client.post(&absent, &nothing).await;
        assert_eq!(said(&hidden_answer), said(&absent_answer), "POST {hidden}");
        hidden_answer.check_not_found();
    }

    for search in [
        format!("{ROOT_WS}/boards/search?q=Concealed"),
        format!("{ROOT_WS}/boards/search?q=Concealed"),
    ] {
        let found: Vec<Container> = ts.client.get(&search).await.ok_json();
        assert!(found.is_empty(), "{search} finds {found:?}");
    }
    // What is in an archive: the names of its entries. One without boards holds its
    // version and nothing else.
    let exported = async |workspace: &str| {
        let archive = ts
            .client
            .get(&format!("/w/{workspace}/archive/export"))
            .await;
        archive.check_ok();
        let archive = zip::ZipArchive::new(std::io::Cursor::new(archive.body))
            .expect("the export is an archive");
        let mut names: Vec<String> = archive
            .file_names()
            .filter_map(Result::ok)
            .map(|name| name.to_string())
            .collect();
        names.sort();
        names
    };
    // A workspace the user is no member of exports nothing: it is not there.
    ts.client
        .get(&format!("/w/{HIDDEN_WORKSPACE_ID}/archive/export"))
        .await
        .check_not_found();

    // To who works in the workspace, the board is found and is in the archive.
    ts.app
        .store()
        .add_workspace_member(
            HIDDEN_WORKSPACE_ID,
            &ts.client.me().await.id,
            WorkspaceRole::Member,
        )
        .await
        .expect("the user is let into the hidden workspace");
    let found: Vec<Container> = ts
        .client
        .get(&format!(
            "/w/{HIDDEN_WORKSPACE_ID}/boards/search?q=Concealed"
        ))
        .await
        .ok_json();
    assert_eq!(ids(&found), [board.id.as_str()], "a member finds it");
    let names = exported(HIDDEN_WORKSPACE_ID).await;
    assert!(
        names.iter().any(|name| name.starts_with(&board.id)),
        "and exports it: {names:?}"
    );
    ts.teardown().await;
}

/// A read token opens its board in the board's own organization, whichever the request
/// would otherwise work in: the board, its blocks and its cards. It opens that board
/// and no other of the organization.
#[tokio::test]
async fn a_read_token_opens_its_board_in_the_boards_organization_and_no_other() {
    // SQLite holds one organization.
    if fvoci_store::testing::db_type_of_the_tests() != DbType::Postgres {
        return;
    }
    let ts = Setup::local_mode().start().await.init_basic().await;
    let (theirs, their_workspace, shared) = another_organization(&ts).await;
    let owner = theirs
        .get_container(&shared)
        .await
        .expect("their board is read")
        .created_by;
    let mut card = fvoci_model::Block {
        id: fvoci_model::new_id(),
        container_id: shared.clone(),
        block_type: fvoci_model::BlockType::Card,
        title: "their card".to_owned(),
        ..fvoci_model::Block::default()
    };
    theirs
        .insert_block(&mut card, &owner)
        .await
        .expect("their card is stored");
    let mut other = Container {
        id: fvoci_model::new_id(),
        workspace_id: their_workspace,
        visibility: ContainerVisibility::Open,
        ..Container::default()
    };
    theirs
        .insert_container(&mut other, &owner)
        .await
        .expect("another board of theirs is stored");
    let token = theirs
        .set_public_link(
            &shared,
            fvoci_model::PublicLinkChange::On,
            "a-read-token-of-theirs",
            &owner,
        )
        .await
        .expect("their board is shared")
        .token;

    // Without a session, and with the session of a user of another organization.
    for client in [ts.anonymous(), ts.client.clone()] {
        let board: Container = client
            .get(&format!("/shared/{shared}?readToken={token}"))
            .await
            .ok_json();
        assert_eq!(board.id, shared, "the board");
        for part in ["blocks", "cards"] {
            let listed: Vec<Value> = client
                .get(&format!("/shared/{shared}/{part}?readToken={token}"))
                .await
                .ok_json();
            let listed: Vec<&Value> = listed.iter().map(|one| &one["id"]).collect();
            assert_eq!(listed, [&json!(card.id)], "its {part}");
        }
    }
    // The token is the one board's.
    for path in [
        format!("/shared/{}?readToken={token}", other.id),
        format!("/shared/{}/blocks?readToken={token}", other.id),
        format!("/shared/{}/cards?readToken={token}", other.id),
    ] {
        ts.anonymous().get(&path).await.check_not_found();
        ts.client.get(&path).await.check_not_found();
    }
    ts.teardown().await;
}

/// A board takes the people of its workspace, its guests too, and nobody else: whoever is
/// not in the workspace is, to who adds members, a user who does not exist, and an archive
/// that names such a member is imported without them.
#[tokio::test]
async fn a_board_takes_the_people_of_its_workspace() {
    let ts = Setup::native().start().await.init_basic().await;
    let user1 = ts.client.me().await.id;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    // The stranger is in the root workspace, as everybody is, and not in the board's.
    let stranger = ts.stranger().await;
    let members = format!("{TEST_WS}/containers/{}/members", board.id);
    let add = async |user_id: &str| {
        ts.client
            .post(&members, &json!({"userId": user_id, "role": "editor"}))
            .await
    };

    let outside = add(&stranger).await;
    assert_eq!(
        said(&outside),
        said(&add(UNKNOWN_ID).await),
        "not in the workspace"
    );
    outside.check_error(reqwest::StatusCode::BAD_REQUEST, "no such user");
    ts.app
        .store()
        .add_workspace_member(TEST_WORKSPACE_ID, &stranger, WorkspaceRole::Guest)
        .await
        .expect("the stranger becomes a guest of the workspace");
    add(&stranger).await.check_ok();

    // The archive of the board names both members. Imported into a workspace that the
    // stranger is not in, the board has the importing user alone.
    let archive = ts
        .client
        .get(&format!("{TEST_WS}/containers/{}/archive/export", board.id))
        .await;
    archive.check_ok();
    let members_after_import_into = async |workspace_id: &str| {
        crate::export::import(&ts.client, workspace_id, &archive.body)
            .await
            .check_ok();
        let boards: Vec<Container> = ts
            .client
            .get(&format!("/w/{workspace_id}/boards"))
            .await
            .ok_json();
        let imported = boards
            .iter()
            .rfind(|imported| imported.id != board.id)
            .expect("the imported board is in the workspace");
        let members: Vec<ContainerMember> = ts
            .client
            .get(&format!(
                "/w/{workspace_id}/containers/{}/members",
                imported.id
            ))
            .await
            .ok_json();
        let mut members: Vec<String> = members.into_iter().map(|one| one.user_id).collect();
        members.sort();
        members
    };
    let mut both = vec![user1.clone(), stranger.clone()];
    both.sort();
    assert_eq!(members_after_import_into(TEST_WORKSPACE_ID).await, both);
    ts.app
        .store()
        .add_workspace_member(HIDDEN_WORKSPACE_ID, &user1, WorkspaceRole::Member)
        .await
        .expect("the first user is let into the hidden workspace");
    assert_eq!(
        members_after_import_into(HIDDEN_WORKSPACE_ID).await,
        [user1]
    );
    ts.teardown().await;
}

/// A parameter of the path that is no text: the workspace's names a workspace that is
/// not there (404), any other a malformed request (400), on every route under `/w/`.
#[tokio::test]
async fn a_workspace_that_is_no_text_is_not_there_and_another_parameter_is_malformed() {
    let ts = Setup::native().start().await.init_basic().await;
    for route in [
        "/w/%FF/categories",
        "/w/%FF/boards",
        "/w/%FF/containers/x",
        "/w/%FF",
    ] {
        ts.client
            .get(route)
            .await
            .check_error(reqwest::StatusCode::NOT_FOUND, "workspace not found");
    }
    ts.client
        .put(&format!("{ROOT_WS}/categories/%FF"), &json!({"name": "x"}))
        .await
        .check_error(reqwest::StatusCode::BAD_REQUEST, "the request is malformed");
    ts.client
        .get(&format!("{ROOT_WS}/containers/%FF"))
        .await
        .check_error(reqwest::StatusCode::BAD_REQUEST, "the request is malformed");
    ts.teardown().await;
}
