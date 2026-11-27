//! Integration tests for searching boards across a user's accessible boards and within a
//! single workspace.

use fvoci_model::{
    Container, ContainerKinds, ContainerSearchField, ContainerVisibility, PublicContainers, new_id,
};
use serde_json::json;

use crate::boards::{NOT_AN_ID, new_board, object, sorted_board_ids};
use crate::common;

/// A table-driven case for searching a user's accessible boards.
struct SearchCase<'a> {
    name: &'static str,
    term: &'static str,
    search_field: ContainerSearchField,
    public_boards: PublicContainers,
    expected_boards: Vec<&'a Container>,
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "table-driven search cases share one seeded store fixture"
)]
async fn search_containers_for_user() {
    let test = common::setup().await;
    let user_id = common::user(&test, "user").await;
    let workspace_id_1 = common::workspace_of(&test, &[&user_id]).await;
    let workspace_id_2 = common::workspace_of(&test, &[&user_id]).await;

    // "should return empty if user is not a member of any board and there are no public
    // boards on the workspace"
    {
        let boards = test
            .search_containers_for_user(
                "",
                ContainerSearchField::Title,
                &user_id,
                PublicContainers::Include,
                ContainerKinds::BOARDS,
            )
            .await
            .expect("the search succeeds");
        assert!(boards.is_empty(), "got {boards:?}");
    }

    let mut board1 = Container {
        title: "Public Board with admin".to_owned(),
        properties: object(json!({ "foo": "bar1" })),
        ..new_board(&new_id(), &workspace_id_1, ContainerVisibility::Open)
    };
    test.insert_container_with_admin(&mut board1, &user_id)
        .await
        .expect("board 1 is inserted");

    let mut board2 = Container {
        title: "Public Board".to_owned(),
        properties: object(json!({ "foo": "bar2" })),
        ..new_board(&new_id(), &workspace_id_1, ContainerVisibility::Open)
    };
    test.insert_container(&mut board2, &user_id)
        .await
        .expect("board 2 is inserted");

    let mut board3 = Container {
        title: "Private Board with admin".to_owned(),
        ..new_board(&new_id(), &workspace_id_1, ContainerVisibility::Private)
    };
    test.insert_container_with_admin(&mut board3, &user_id)
        .await
        .expect("board 3 is inserted");

    let mut board4 = Container {
        title: "Private Board".to_owned(),
        ..new_board(&new_id(), &workspace_id_1, ContainerVisibility::Private)
    };
    test.insert_container(&mut board4, &user_id)
        .await
        .expect("board 4 is inserted");

    let mut board5 = Container {
        title: "Public Board with admin in workspace 2".to_owned(),
        ..new_board(&new_id(), &workspace_id_2, ContainerVisibility::Open)
    };
    test.insert_container_with_admin(&mut board5, &user_id)
        .await
        .expect("board 5 is inserted");

    let cases = [
        SearchCase {
            name: "should find all private boards that the user is a member of and public \
                   boards with an empty term",
            term: "",
            search_field: ContainerSearchField::Title,
            public_boards: PublicContainers::Include,
            expected_boards: vec![&board1, &board2, &board3, &board5],
        },
        SearchCase {
            name: "should find all with term board",
            term: "board",
            search_field: ContainerSearchField::Title,
            public_boards: PublicContainers::Include,
            expected_boards: vec![&board1, &board2, &board3, &board5],
        },
        SearchCase {
            name: "should find all with term board where the user is member of",
            term: "board",
            search_field: ContainerSearchField::Title,
            public_boards: PublicContainers::Exclude,
            expected_boards: vec![&board1, &board3, &board5],
        },
        SearchCase {
            name: "should find only public as per the term, wether user is a member or not",
            term: "public",
            search_field: ContainerSearchField::Title,
            public_boards: PublicContainers::Include,
            expected_boards: vec![&board1, &board2, &board5],
        },
        SearchCase {
            name: "should find only private as per the term, wether user is a member or not",
            term: "priv",
            search_field: ContainerSearchField::Title,
            public_boards: PublicContainers::Include,
            expected_boards: vec![&board3],
        },
        SearchCase {
            name: "should find no board in workspace 2 with a non matching term",
            term: "non-matching-term",
            search_field: ContainerSearchField::Title,
            public_boards: PublicContainers::Include,
            expected_boards: vec![],
        },
        SearchCase {
            name: "should find all boards with a named property",
            term: "foo",
            search_field: ContainerSearchField::PropertyName,
            public_boards: PublicContainers::Include,
            expected_boards: vec![&board1, &board2],
        },
        SearchCase {
            name: "should find no boards with a non-existing named property",
            term: "bogus",
            search_field: ContainerSearchField::PropertyName,
            public_boards: PublicContainers::Include,
            expected_boards: vec![],
        },
    ];

    for case in cases {
        let found = test
            .search_containers_for_user(
                case.term,
                case.search_field,
                &user_id,
                case.public_boards,
                ContainerKinds::BOARDS,
            )
            .await
            .expect(case.name);
        assert_eq!(
            sorted_board_ids(&found),
            sorted_board_ids(case.expected_boards),
            "{}",
            case.name
        );
    }

    test.teardown().await;
}

#[tokio::test]
async fn search_containers_for_user_in_workspace() {
    let test = common::setup().await;
    let user_id = common::user(&test, "user").await;

    // "should return empty list if there are no resutls": for a workspace that does not
    // exist, and for text that is no identifier.
    for workspace_id in [new_id(), NOT_AN_ID.to_owned()] {
        let boards = test
            .search_containers_for_user_in_workspace(
                &workspace_id,
                "",
                &user_id,
                ContainerKinds::BOARDS,
            )
            .await
            .expect("the search succeeds");
        assert!(
            boards.is_empty(),
            "workspace {workspace_id}: got {boards:?}"
        );
    }

    test.teardown().await;
}

/// Searching within a workspace returns open boards and the user's own private boards.
#[tokio::test]
async fn search_containers_for_user_in_workspace_finds_open_boards_and_own_private_boards() {
    let test = common::setup().await;
    let user_id = common::user(&test, "user").await;
    let other_user_id = common::user(&test, "other-user").await;
    let workspace_id_1 = common::workspace_of(&test, &[&user_id, &other_user_id]).await;
    let workspace_id_2 = common::workspace(&test).await;

    let mut open = Container {
        title: "Open Roadmap".to_owned(),
        ..new_board(&new_id(), &workspace_id_1, ContainerVisibility::Open)
    };
    test.insert_container(&mut open, &other_user_id)
        .await
        .expect("the board is inserted");
    let mut own_private = Container {
        title: "Private Roadmap".to_owned(),
        ..new_board(&new_id(), &workspace_id_1, ContainerVisibility::Private)
    };
    test.insert_container_with_admin(&mut own_private, &user_id)
        .await
        .expect("the board is inserted");
    let mut foreign_private = Container {
        title: "Secret Roadmap".to_owned(),
        ..new_board(&new_id(), &workspace_id_1, ContainerVisibility::Private)
    };
    test.insert_container_with_admin(&mut foreign_private, &other_user_id)
        .await
        .expect("the board is inserted");
    let mut other_workspace = Container {
        title: "Open Roadmap elsewhere".to_owned(),
        ..new_board(&new_id(), &workspace_id_2, ContainerVisibility::Open)
    };
    test.insert_container(&mut other_workspace, &user_id)
        .await
        .expect("the board is inserted");
    let mut template = Container {
        title: "Roadmap template".to_owned(),
        is_template: true,
        ..new_board(&new_id(), &workspace_id_1, ContainerVisibility::Open)
    };
    test.insert_container(&mut template, &user_id)
        .await
        .expect("the board is inserted");

    let search = async |term: &str| {
        let boards = test
            .search_containers_for_user_in_workspace(
                &workspace_id_1,
                term,
                &user_id,
                ContainerKinds::BOARDS,
            )
            .await
            .expect("the search succeeds");
        sorted_board_ids(&boards)
    };
    let open_and_own_private = sorted_board_ids([&open, &own_private]);
    assert_eq!(
        search("").await,
        open_and_own_private,
        "an empty term matches every board the user can reach"
    );
    assert_eq!(
        search("ROADMAP").await,
        open_and_own_private,
        "case does not matter"
    );
    assert_eq!(
        search("roadmap open").await,
        [open.id.as_str()],
        "every word must match"
    );
    assert_eq!(
        search("  private  ").await,
        [own_private.id.as_str()],
        "spaces around the term do not count"
    );
    assert_eq!(
        search("secret").await,
        Vec::<String>::new(),
        "a private board of somebody else is not found"
    );

    test.teardown().await;
}
