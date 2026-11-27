//! Integration tests for board search routes and visibility rules.

use fvoci_model::{Container, ContainerVisibility, JsonMap, new_id};
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::common::ROOT_WS;
use crate::common::{
    ApiResponse, Client, ROOT_WORKSPACE_ID, Setup, TEST_WORKSPACE_ID, TestServer, UNKNOWN_ID,
};

/// Whether the creator of a board becomes an admin member of it.
#[derive(Clone, Copy)]
enum Membership {
    Admin,
    None,
}

/// Inserts a board directly into the store, optionally adding the creator as admin.
async fn create_board(
    th: &TestServer,
    board: Container,
    user_id: &str,
    membership: Membership,
) -> Container {
    let mut board = Container {
        id: new_id(),
        ..board
    };
    let store = th.app.store();
    match membership {
        Membership::Admin => {
            store
                .insert_container_with_admin(&mut board, user_id)
                .await
                .expect("the board is inserted with its administrator");
        }
        Membership::None => {
            store
                .insert_container(&mut board, user_id)
                .await
                .expect("the board is inserted");
        }
    }
    board
}

fn titled(title: &str, board_type: ContainerVisibility, workspace_id: &str) -> Container {
    Container {
        title: title.to_owned(),
        visibility: board_type,
        workspace_id: workspace_id.to_owned(),
        ..Container::default()
    }
}

/// Searches boards within one workspace by query term.
async fn search_workspace(client: &Client, workspace_id: &str, term: &str) -> ApiResponse {
    client
        .get(&format!("/w/{workspace_id}/boards/search?q={term}"))
        .await
}

/// The identifiers of the boards in an answer, sorted.
#[track_caller]
fn found_ids(response: &ApiResponse) -> Vec<String> {
    let mut found: Vec<String> = response
        .ok_json::<Vec<Container>>()
        .into_iter()
        .map(|board| board.id)
        .collect();
    found.sort_unstable();
    found
}

fn sorted_ids(boards: &[&Container]) -> Vec<String> {
    let mut expected: Vec<String> = boards.iter().map(|board| board.id.clone()).collect();
    expected.sort_unstable();
    expected
}

#[track_caller]
fn check_error(response: &ApiResponse, status: StatusCode, message: &str) {
    response.check_error(status, message);
}

/// The answer when nothing is found: an empty list, not `null`.
#[track_caller]
fn check_nothing_found(response: &ApiResponse) {
    response.check_ok();
    assert_eq!(response.text(), "[]", "an empty list");
}

/// Unauthenticated callers are rejected by the board search route.
#[tokio::test]
async fn search_boards_a_non_authenticated_user_should_be_rejected() {
    let th = Setup::native().start().await.init_basic().await;

    let response = search_workspace(&th.anonymous(), TEST_WORKSPACE_ID, "term").await;

    check_error(&response, StatusCode::UNAUTHORIZED, "no session token");
    th.teardown().await;
}

/// Search returns all matching open boards plus matching private boards the user is a
/// member of, across five search cases.
#[tokio::test]
async fn search_boards_all_the_matching_private_boards_that_the_user_is_a_member_of_and_all_matching_public_boards_should_be_returned()
 {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;
    let workspace_id = TEST_WORKSPACE_ID;
    let board = async |title: &str,
                       board_type: ContainerVisibility,
                       workspace_id: &str,
                       membership: Membership| {
        create_board(
            &th,
            titled(title, board_type, workspace_id),
            &user1.id,
            membership,
        )
        .await
    };

    let board1 = board(
        "public board where user1 is admin",
        ContainerVisibility::Open,
        workspace_id,
        Membership::Admin,
    )
    .await;
    let board2 = board(
        "public board where user1 is not member",
        ContainerVisibility::Open,
        workspace_id,
        Membership::None,
    )
    .await;
    let board3 = board(
        "private board where user1 is admin",
        ContainerVisibility::Private,
        workspace_id,
        Membership::Admin,
    )
    .await;
    board(
        "private board where user1 is not member",
        ContainerVisibility::Private,
        workspace_id,
        Membership::None,
    )
    .await;
    let board5 = board(
        "private board where user1 is admin, but in other workspace",
        ContainerVisibility::Private,
        ROOT_WORKSPACE_ID,
        Membership::Admin,
    )
    .await;

    // The board of the other workspace is found under that one.
    assert_eq!(
        found_ids(&search_workspace(&th.client, ROOT_WORKSPACE_ID, "admin").await),
        sorted_ids(&[&board5]),
        "the board of the root workspace"
    );
    let cases = [
        (
            "should return all boards of the workspace where user1 is member or that are public",
            &th.client,
            "board",
            vec![&board1, &board2, &board3],
        ),
        (
            "matching a full word",
            &th.client,
            "admin",
            vec![&board1, &board3],
        ),
        (
            "matching part of the word",
            &th.client,
            "ubli",
            vec![&board1, &board2],
        ),
        (
            "case insensitive",
            &th.client,
            "UBLI",
            vec![&board1, &board2],
        ),
        (
            "user2 can only see the public boards, as he's not a member of any",
            &th.client2,
            "board",
            vec![&board1, &board2],
        ),
    ];
    for (name, client, term, expected) in cases {
        let response = search_workspace(client, workspace_id, term).await;
        assert_eq!(found_ids(&response), sorted_ids(&expected), "{name}");
    }

    th.teardown().await;
}

// ---------------------------------------------------------------------------------------
// Additional route-level search behaviors and retired endpoints.

/// A server with two users and two boards of the first: an open one titled "b one" and a
/// private one titled "b two", both in workspace `0`.
async fn two_boards() -> (TestServer, Container, Container) {
    let th = Setup::native().start().await.init_basic().await;
    let user = th.client.me().await;
    let open = create_board(
        &th,
        titled("b one", ContainerVisibility::Open, ROOT_WORKSPACE_ID),
        &user.id,
        Membership::Admin,
    )
    .await;
    let private = create_board(
        &th,
        titled("b two", ContainerVisibility::Private, ROOT_WORKSPACE_ID),
        &user.id,
        Membership::Admin,
    )
    .await;
    (th, open, private)
}

/// Without a search term nothing is found, whatever boards there are.
#[tokio::test]
async fn search_without_a_term_finds_nothing() {
    let (th, _open, _private) = two_boards().await;

    for path in [
        &format!("{ROOT_WS}/boards/search"),
        &format!("{ROOT_WS}/boards/search?q="),
        &format!("{ROOT_WS}/boards/search?field=title"),
        &format!("{ROOT_WS}/boards/search"),
        &format!("{ROOT_WS}/boards/search?q="),
    ] {
        check_nothing_found(&th.client.get(path).await);
    }
    // And a term that matches no board answers the same.
    check_nothing_found(&search_workspace(&th.client, ROOT_WORKSPACE_ID, "zzzzqq").await);
    check_nothing_found(
        &th.client
            .get(&format!("{ROOT_WS}/boards/search?q=zzzzqq"))
            .await,
    );

    th.teardown().await;
}

/// The search within a workspace: what each user finds, and how the term is read.
#[tokio::test]
async fn search_boards_of_a_workspace() {
    let (th, open, private) = two_boards().await;

    let response = search_workspace(&th.client, ROOT_WORKSPACE_ID, "b").await;
    assert_eq!(
        found_ids(&response),
        sorted_ids(&[&open, &private]),
        "the member finds both"
    );
    // The boards are answered in full.
    let found: Vec<Container> = response.ok_json();
    let found_open = found
        .iter()
        .find(|board| board.id == open.id)
        .expect("the open board is found");
    assert_eq!(*found_open, open, "the board as stored");

    let response = search_workspace(&th.client2, ROOT_WORKSPACE_ID, "b").await;
    assert_eq!(
        found_ids(&response),
        sorted_ids(&[&open]),
        "another user finds the open board only"
    );
    check_nothing_found(&search_workspace(&th.client2, ROOT_WORKSPACE_ID, "two").await);

    // The term is decoded from the query: `+` and `%20` are spaces, and every word of
    // the term must be in the title.
    for term in ["b+one", "b%20one", "ONE+b"] {
        let response = search_workspace(&th.client, ROOT_WORKSPACE_ID, term).await;
        assert_eq!(found_ids(&response), sorted_ids(&[&open]), "term {term}");
    }
    check_nothing_found(&search_workspace(&th.client, ROOT_WORKSPACE_ID, "one+two").await);

    // The boards of the root workspace are not found under another workspace.
    check_nothing_found(&search_workspace(&th.client, TEST_WORKSPACE_ID, "b").await);
    // The workspace has to be one, though.
    for no_workspace in [UNKNOWN_ID, "another-workspace"] {
        let response = search_workspace(&th.client, no_workspace, "b").await;
        response.check_error(StatusCode::NOT_FOUND, "workspace not found");
    }

    th.teardown().await;
}

/// The `field` parameter of the search within a workspace.
#[tokio::test]
async fn search_boards_by_field() {
    let (th, open, _private) = two_boards().await;
    let user = th.client.me().await;
    let mut properties = JsonMap::new();
    properties.insert("status".to_owned(), json!("green"));
    let with_property = create_board(
        &th,
        Container {
            properties,
            ..titled(
                "with a property",
                ContainerVisibility::Open,
                ROOT_WORKSPACE_ID,
            )
        },
        &user.id,
        Membership::Admin,
    )
    .await;
    let search = async |query: &str| {
        th.client
            .get(&format!("{ROOT_WS}/boards/search?{query}"))
            .await
    };

    // The title is the default field.
    let response = search("q=one&field=title").await;
    assert_eq!(found_ids(&response), sorted_ids(&[&open]), "by title");
    let response = search("q=one&field=").await;
    assert_eq!(found_ids(&response), sorted_ids(&[&open]), "an empty field");

    // By property name: the term is the name of a property of the board.
    let response = search("q=status&field=property_name").await;
    assert_eq!(
        found_ids(&response),
        sorted_ids(&[&with_property]),
        "by property name"
    );
    check_nothing_found(&search("q=one&field=property_name").await);
    check_nothing_found(&search("q=status").await);

    // An unknown field is refused, even without a term; the names are case-sensitive.
    for query in ["q=one&field=bogus", "field=bogus", "q=one&field=TITLE"] {
        let response = search(query).await;
        check_error(
            &response,
            StatusCode::BAD_REQUEST,
            "invalid board search field",
        );
    }

    th.teardown().await;
}

/// The searches answer the same on both databases: a property name is a name, not a
/// path, and is found with a `null` value too; a title and a user name are matched
/// ignoring the case of every letter; `%` and `_` are characters.
#[tokio::test]
async fn searches_answer_the_same_on_both_databases() {
    let th = Setup::native().start().await.init_basic().await;
    let user = th.client.me().await;
    let mut properties = JsonMap::new();
    properties.insert("a.b".to_owned(), json!("x"));
    properties.insert("empty".to_owned(), Value::Null);
    properties.insert("nested".to_owned(), json!({"inner": 1}));
    let board = create_board(
        &th,
        Container {
            properties,
            ..titled(
                "Ärger 100% über_all",
                ContainerVisibility::Open,
                ROOT_WORKSPACE_ID,
            )
        },
        &user.id,
        Membership::Admin,
    )
    .await;
    let search = async |query: &str| {
        th.client
            .get(&format!("{ROOT_WS}/boards/search?{query}"))
            .await
    };
    for query in [
        "q=a.b&field=property_name",
        "q=empty&field=property_name",
        "q=%C3%A4rger",
        "q=%C3%84RGER%20%C3%BCBER",
        "q=100%25",
        "q=r_a",
    ] {
        let response = search(query).await;
        assert_eq!(found_ids(&response), sorted_ids(&[&board]), "{query}");
    }
    for query in [
        "q=nested.inner&field=property_name",
        "q=a&field=property_name",
        "q=%5B&field=property_name",
        "q=a%00&field=property_name",
        "q=10%25%25",
        "q=r%25a",
    ] {
        let response = search(query).await;
        response.check_ok();
        check_nothing_found(&response);
    }
    th.teardown().await;
}

/// The search of a workspace finds no board of another workspace, and no template; the
/// search of every workspace (`/boards/search`) is gone.
#[tokio::test]
async fn search_all_boards() {
    let (th, open, private) = two_boards().await;
    let user = th.client.me().await;
    let elsewhere = create_board(
        &th,
        titled("b three", ContainerVisibility::Private, TEST_WORKSPACE_ID),
        &user.id,
        Membership::Admin,
    )
    .await;
    let template = create_board(
        &th,
        Container {
            is_template: true,
            ..titled("b template", ContainerVisibility::Open, ROOT_WORKSPACE_ID)
        },
        &user.id,
        Membership::Admin,
    )
    .await;

    let response = search_workspace(&th.client, ROOT_WORKSPACE_ID, "b").await;
    assert_eq!(
        found_ids(&response),
        sorted_ids(&[&open, &private]),
        "the boards of the workspace, and no template"
    );
    assert!(
        !found_ids(&response).contains(&template.id),
        "templates are left out"
    );
    assert_eq!(
        found_ids(&search_workspace(&th.client, TEST_WORKSPACE_ID, "b").await),
        sorted_ids(&[&elsewhere]),
        "the other workspace's"
    );
    th.client
        .get("/boards/search?q=b")
        .await
        .check_error(StatusCode::NOT_FOUND, "route not found");
    th.teardown().await;
}

/// Retired channel-search and linkable-board routes do not exist and return 404.
#[tokio::test]
async fn search_routes_of_the_host_application_do_not_exist() {
    let th = Setup::native().start().await.init_basic().await;
    let paths = [
        &format!("/teams/{ROOT_WORKSPACE_ID}/channels"),
        &format!("/teams/{ROOT_WORKSPACE_ID}/channels?search=x"),
        &format!("{ROOT_WS}/boards/search/linkable?q=b"),
    ];

    for path in paths {
        for client in [&th.client, &th.anonymous()] {
            let response = client.get(path).await;
            check_error(&response, StatusCode::NOT_FOUND, "route not found");
        }
    }
    th.teardown().await;
}

/// App-level search within a single workspace returns open boards and the user's own
/// private boards.
#[tokio::test]
async fn search_containers_for_user_in_workspace() {
    let (th, open, private) = two_boards().await;
    let user = th.client.me().await;
    let other = th.client2.me().await;
    create_board(
        &th,
        titled("b three", ContainerVisibility::Open, TEST_WORKSPACE_ID),
        &user.id,
        Membership::Admin,
    )
    .await;
    let search = async |workspace_id: &str, term: &str, user_id: &str| {
        let mut found: Vec<String> = th
            .app
            .search_containers_for_user_in_workspace(workspace_id, term, user_id)
            .await
            .expect("the search runs")
            .into_iter()
            .map(|board| board.id)
            .collect();
        found.sort_unstable();
        found
    };

    assert_eq!(
        search(ROOT_WORKSPACE_ID, "b", &user.id).await,
        sorted_ids(&[&open, &private]),
        "the member finds the boards of the workspace"
    );
    assert_eq!(
        search(ROOT_WORKSPACE_ID, "b", &other.id).await,
        sorted_ids(&[&open]),
        "another user finds the open one"
    );
    assert_eq!(
        search(ROOT_WORKSPACE_ID, "two", &user.id).await,
        sorted_ids(&[&private]),
        "by title"
    );
    assert!(
        search(UNKNOWN_ID, "b", &user.id).await.is_empty(),
        "nothing in a workspace that is none"
    );

    th.teardown().await;
}
