//! Deterministic ordering of every list returned by the store.
//!
//! The rule is the oldest first, and the identifier where the ages are the same. A list
//! with an order of its own (the categories of a sidebar, the boards of a category)
//! follows that order and falls back to the rule where the order ties. Memberships carry
//! no time: they come by the identifier of what they name, which is the order that was
//! made in.
//!
//! Each test stores its rows through the store and then, past the store, sets the
//! times and sort orders so that the order asked for differs both from the order of
//! the identifiers and from the order of the names.

use fvoci_model::PublicContainers;
use fvoci_model::{
    Block, BlockType, Category, Container, ContainerKinds, ContainerMember, ContainerSearchField,
    ContainerVisibility, Subscription, User, WorkspaceRole, new_id,
};
use fvoci_store::{Looking, Paging, Reach, Sight, Store};

use crate::common::{self, TestStore};

/// Sets the time each of the rows was made at.
async fn set_created_at(test: &TestStore, table: &str, times: &[(&str, i64)]) {
    for (id, created_at) in times {
        common::execute_raw(
            &test.database_connection(),
            &format!("UPDATE {table} SET created_at = {created_at} WHERE id = '{id}'"),
        )
        .await;
    }
}

fn board_ids(boards: &[Container]) -> Vec<&str> {
    boards.iter().map(|board| board.id.as_str()).collect()
}

fn user_ids(users: &[User]) -> Vec<&str> {
    users.iter().map(|user| user.id.as_str()).collect()
}

/// Stores a board of the workspace and returns its identifier.
async fn stored_board(
    store: &Store,
    workspace_id: &str,
    user_id: &str,
    board: Container,
) -> String {
    let mut board = Container {
        id: new_id(),
        workspace_id: workspace_id.to_owned(),
        ..board
    };
    store
        .insert_container(&mut board, user_id)
        .await
        .expect("the board is inserted");
    board.id
}

/// Lists of boards come oldest first, and by identifier where two are of an age.
#[tokio::test]
async fn boards_come_oldest_first() {
    let test = common::setup().await;
    let store = &test.store;
    let user_id = common::user(store, "user").await;
    let workspace_id = common::workspace_of(store, &[&user_id]).await;
    let open = |title: &str| Container {
        visibility: ContainerVisibility::Open,
        title: title.to_owned(),
        ..Container::default()
    };
    let first = stored_board(store, &workspace_id, &user_id, open("board d")).await;
    let second = stored_board(store, &workspace_id, &user_id, open("board a")).await;
    let third = stored_board(store, &workspace_id, &user_id, open("board c")).await;
    let fourth = stored_board(store, &workspace_id, &user_id, open("board b")).await;
    set_created_at(
        &test,
        "containers",
        &[(&fourth, 100), (&first, 200), (&second, 200), (&third, 300)],
    )
    .await;
    let expected = [
        fourth.as_str(),
        first.as_str(),
        second.as_str(),
        third.as_str(),
    ];

    let of_workspace = store
        .get_containers_for_user_and_workspace(
            &user_id,
            &workspace_id,
            PublicContainers::Include,
            ContainerKinds::BOARDS,
        )
        .await
        .expect("the boards are read");
    assert_eq!(
        board_ids(&of_workspace),
        expected,
        "the boards of a workspace"
    );

    let found = store
        .search_containers_for_user(
            "board",
            ContainerSearchField::Title,
            &user_id,
            PublicContainers::Include,
            ContainerKinds::BOARDS,
        )
        .await
        .expect("the boards are searched");
    assert_eq!(board_ids(&found), expected, "a search of every workspace");

    let found_in_workspace = store
        .search_containers_for_user_in_workspace(
            &workspace_id,
            "board",
            &user_id,
            ContainerKinds::BOARDS,
        )
        .await
        .expect("the boards are searched");
    assert_eq!(
        board_ids(&found_in_workspace),
        expected,
        "a search of one workspace"
    );

    let asked = [third.clone(), first.clone(), fourth.clone(), second.clone()];
    let by_id = store
        .get_containers_in_workspace_by_ids(&asked, &workspace_id)
        .await
        .expect("the boards are read");
    assert_eq!(
        board_ids(&by_id),
        expected,
        "boards by identifier, whatever order they are asked for in"
    );

    test.teardown().await;
}

/// The templates of a workspace and the built-in ones come oldest first too.
#[tokio::test]
async fn templates_come_oldest_first() {
    let test = common::setup().await;
    let store = &test.store;
    let user_id = common::user(store, "user").await;
    let workspace_id = common::workspace(store).await;
    let template = || Container {
        visibility: ContainerVisibility::Open,
        is_template: true,
        ..Container::default()
    };
    let mut own = Vec::new();
    let mut builtin = Vec::new();
    for _ in 0..3 {
        own.push(stored_board(store, &workspace_id, &user_id, template()).await);
        // A template stored on behalf of no user is a built-in one.
        builtin.push(stored_board(store, &workspace_id, "", template()).await);
    }
    for ids in [&own, &builtin] {
        set_created_at(
            &test,
            "containers",
            &[(&ids[2], 100), (&ids[0], 200), (&ids[1], 200)],
        )
        .await;
    }

    let of_workspace = store
        .get_template_boards(&workspace_id, &user_id)
        .await
        .expect("the templates are read");
    assert_eq!(
        board_ids(&of_workspace),
        [own[2].as_str(), own[0].as_str(), own[1].as_str()],
        "the templates of a workspace"
    );
    let shipped = store
        .get_builtin_templates()
        .await
        .expect("the templates are read");
    assert_eq!(
        board_ids(&shipped),
        [
            builtin[2].as_str(),
            builtin[0].as_str(),
            builtin[1].as_str()
        ],
        "the built-in templates"
    );

    test.teardown().await;
}

/// The members of a board come by the identifier of the user and the memberships of a
/// user by the identifier of the board, whatever order they were added in.
#[tokio::test]
async fn memberships_come_by_identifier() {
    let test = common::setup().await;
    let store = &test.store;
    let users = [
        common::user(store, "user c").await,
        common::user(store, "user a").await,
        common::user(store, "user b").await,
    ];
    let workspace_id = common::workspace_of(store, &[&users[0]]).await;
    let open = || Container {
        visibility: ContainerVisibility::Open,
        ..Container::default()
    };
    let boards = [
        stored_board(store, &workspace_id, &users[0], open()).await,
        stored_board(store, &workspace_id, &users[0], open()).await,
        stored_board(store, &workspace_id, &users[0], open()).await,
    ];
    let join = async |board_id: &str, user_id: &str| {
        let member = ContainerMember {
            container_id: board_id.to_owned(),
            user_id: user_id.to_owned(),
            role: fvoci_model::ContainerRole::Editor,
            synthetic: false,
            inherited_role: None,
        };
        store
            .save_member(&member)
            .await
            .expect("the member is saved");
    };
    for user in [2, 0, 1] {
        join(&boards[0], &users[user]).await;
    }
    for board in [2, 1] {
        join(&boards[board], &users[0]).await;
    }

    let members = store
        .get_members_for_container(&boards[0])
        .await
        .expect("the members are read");
    let member_ids: Vec<&str> = members
        .iter()
        .map(|member| member.user_id.as_str())
        .collect();
    assert_eq!(member_ids, users.each_ref().map(String::as_str));

    let memberships = store
        .get_members_for_user(&users[0])
        .await
        .expect("the memberships are read");
    let membership_boards: Vec<&str> = memberships
        .iter()
        .map(|member| member.container_id.as_str())
        .collect();
    assert_eq!(membership_boards, boards.each_ref().map(String::as_str));

    test.teardown().await;
}

/// Lists of users come oldest first, and a search that finds more users than it returns
/// returns the oldest.
#[tokio::test]
async fn users_come_oldest_first() {
    let test = common::setup().await;
    let store = &test.store;
    let first = common::user(store, "order-c").await;
    let second = common::user(store, "order-a").await;
    let third = common::user(store, "order-b").await;
    set_created_at(
        &test,
        "users",
        &[(&third, 100), (&first, 200), (&second, 200)],
    )
    .await;
    let expected = [third.as_str(), first.as_str(), second.as_str()];

    let everybody = store
        .get_users_by_workspace()
        .await
        .expect("the users are read");
    assert_eq!(user_ids(&everybody), expected, "every user");

    let asked = [second.clone(), third.clone(), first.clone()];
    let by_id = store
        .get_users_list(&asked)
        .await
        .expect("the users are read");
    assert_eq!(
        user_ids(&by_id),
        expected,
        "users by identifier, whatever order they are asked for in"
    );

    // The search is of the users who share a workspace with the one who searches.
    let workspace_id = common::workspace_of(store, &[&first, &second, &third]).await;
    let sight = Sight::InWorkspace {
        seer_id: &first,
        workspace_id: &workspace_id,
        reach: Reach::Workspace,
        looking: Looking::AmongPeople,
    };
    let found = store
        .search_users_in_sight(sight, "order-")
        .await
        .expect("the users are searched");
    assert_eq!(user_ids(&found), expected, "a search");

    let mut many = Vec::new();
    for index in 0..12 {
        let user_id = common::user(store, &format!("many-{index:02}")).await;
        store
            .add_workspace_member(&workspace_id, &user_id, WorkspaceRole::Member)
            .await
            .expect("the user is let into the workspace");
        many.push(user_id);
    }
    // The two made last are the oldest.
    set_created_at(&test, "users", &[(&many[11], 10), (&many[10], 20)]).await;
    let found = store
        .search_users_in_sight(sight, "many-")
        .await
        .expect("the users are searched");
    let mut oldest_ten = vec![many[11].as_str(), many[10].as_str()];
    oldest_ten.extend(many[..8].iter().map(String::as_str));
    assert_eq!(
        user_ids(&found),
        oldest_ten,
        "the ten oldest of twelve matches"
    );

    test.teardown().await;
}

/// Categories with the same sort order come oldest first, not by name, and the boards
/// of a category with the same sort order come by identifier.
#[tokio::test]
async fn sidebar_ties_fall_back_to_age() {
    let test = common::setup().await;
    let store = &test.store;
    let user_id = common::user(store, "user").await;
    let workspace_id = common::workspace(store).await;
    let mut categories = Vec::new();
    for name in ["z", "y", "a"] {
        let category = Category {
            id: new_id(),
            name: name.to_owned(),
            user_id: user_id.clone(),
            workspace_id: workspace_id.clone(),
            created_at: 1,
            updated_at: 1,
            ..Category::default()
        };
        store
            .create_category(&category)
            .await
            .expect("the category is stored");
        categories.push(category.id);
    }
    common::execute_raw(
        &test.database_connection(),
        "UPDATE categories SET sort_order = 0",
    )
    .await;
    set_created_at(
        &test,
        "categories",
        &[
            (&categories[2], 100),
            (&categories[0], 200),
            (&categories[1], 200),
        ],
    )
    .await;

    let listed = store
        .get_user_categories(&user_id, &workspace_id)
        .await
        .expect("the categories are read");
    let listed_ids: Vec<&str> = listed.iter().map(|category| category.id.as_str()).collect();
    assert_eq!(
        listed_ids,
        [
            categories[2].as_str(),
            categories[0].as_str(),
            categories[1].as_str()
        ],
        "categories of one sort order"
    );

    let boards = common::create_test_boards(store, &workspace_id, &user_id, 3).await;
    let filed = [
        boards[2].id.clone(),
        boards[0].id.clone(),
        boards[1].id.clone(),
    ];
    store
        .add_update_category_container(&user_id, &categories[0], &filed)
        .await
        .expect("the boards are filed");
    let boards_of = async |category_id: &str| -> Vec<String> {
        store
            .get_user_category_containers(&user_id, &workspace_id)
            .await
            .expect("the categories are read")
            .into_iter()
            .find(|listed| listed.category.id == category_id)
            .expect("the category is listed")
            .container_metadata
            .into_iter()
            .map(|metadata| metadata.container_id)
            .collect()
    };
    assert_eq!(
        boards_of(&categories[0]).await,
        [
            boards[0].id.as_str(),
            boards[1].id.as_str(),
            boards[2].id.as_str()
        ],
        "boards of one sort order"
    );

    common::execute_raw(
        &test.database_connection(),
        &format!(
            "UPDATE category_containers SET sort_order = -10 WHERE container_id = '{}'",
            boards[2].id
        ),
    )
    .await;
    assert_eq!(
        boards_of(&categories[0]).await,
        [
            boards[2].id.as_str(),
            boards[0].id.as_str(),
            boards[1].id.as_str()
        ],
        "the sort order counts before the identifier"
    );

    test.teardown().await;
}

/// The subscribers of a block that were notified at the same time come by identifier,
/// and the subscriptions of a subscriber made at the same time by the identifier of the
/// block, also those of a workspace.
#[tokio::test]
async fn subscription_ties_fall_back_to_identifiers() {
    let test = common::setup().await;
    let store = &test.store;
    let workspace_id = common::workspace(store).await;
    let users = [
        common::user(store, "user b").await,
        common::user(store, "user a").await,
    ];
    let board_id = stored_board(
        store,
        &workspace_id,
        &users[0],
        Container {
            visibility: ContainerVisibility::Open,
            ..Container::default()
        },
    )
    .await;
    let mut cards: Vec<Block> = (0..2)
        .map(|_| Block {
            id: new_id(),
            container_id: board_id.clone(),
            block_type: BlockType::Card,
            created_at: 1,
            updated_at: 1,
            ..Block::default()
        })
        .collect();
    common::insert_blocks(store, &mut cards, &users[0]).await;
    let subscribe = async |block: &Block, user_id: &str| {
        let subscription = Subscription {
            block_type: block.block_type,
            block_id: block.id.clone(),
            user_id: user_id.to_owned(),
            notified_at: 0,
            created_at: 0,
            deleted_at: None,
        };
        store
            .create_subscription(subscription)
            .await
            .expect("the subscription is stored");
    };
    subscribe(&cards[1], &users[1]).await;
    subscribe(&cards[1], &users[0]).await;
    subscribe(&cards[0], &users[1]).await;
    common::execute_raw(
        &test.database_connection(),
        "UPDATE subscriptions SET notified_at = 5, created_at = 5",
    )
    .await;

    let subscribers = store
        .get_subscribers_for_block(&cards[1].id)
        .await
        .expect("the subscribers are read");
    let user_ids: Vec<&str> = subscribers
        .iter()
        .map(|subscriber| subscriber.user_id.as_str())
        .collect();
    assert_eq!(user_ids, users.each_ref().map(String::as_str));

    let subscriptions = store
        .get_subscriptions(&users[1])
        .await
        .expect("the subscriptions are read");
    let subscribed_blocks: Vec<&str> = subscriptions
        .iter()
        .map(|subscription| subscription.block_id.as_str())
        .collect();
    assert_eq!(
        subscribed_blocks,
        [cards[0].id.as_str(), cards[1].id.as_str()]
    );
    // Those of a workspace come in the same order, and another workspace has none.
    let in_workspace = store
        .get_subscriptions_in_workspace(&users[1], &workspace_id)
        .await
        .expect("the subscriptions of the workspace are read");
    assert_eq!(in_workspace, subscriptions, "the board's workspace");
    let elsewhere = store
        .get_subscriptions_in_workspace(&users[1], &new_id())
        .await
        .expect("the subscriptions of another workspace are read");
    assert!(elsewhere.is_empty(), "another workspace: {elsewhere:?}");

    test.teardown().await;
}

/// Stores a board of the workspace with `admin_id` its admin and `editor_id` an editor.
async fn board_administered_by(
    store: &Store,
    workspace_id: &str,
    admin_id: &str,
    editor_id: &str,
) -> String {
    let board_id = stored_board(store, workspace_id, admin_id, Container::default()).await;
    for (member, admin) in [(admin_id, true), (editor_id, false)] {
        store
            .save_member(&ContainerMember {
                container_id: board_id.clone(),
                user_id: member.to_owned(),
                role: if admin {
                    fvoci_model::ContainerRole::Admin
                } else {
                    fvoci_model::ContainerRole::Editor
                },
                synthetic: false,
                inherited_role: None,
            })
            .await
            .expect("the membership is saved");
    }
    board_id
}

/// The lists of the trash come the last deleted first, and by identifier where two
/// were deleted at once.
#[tokio::test]
async fn the_trash_comes_the_last_deleted_first() {
    let test = common::setup().await;
    let store = &test.store;
    let user_id = common::user(store, "user").await;
    let workspace_id = common::workspace(store).await;
    let board_id = stored_board(store, &workspace_id, &user_id, Container::default()).await;
    let mut cards: Vec<Block> = (0..3)
        .map(|_| Block {
            id: new_id(),
            container_id: board_id.clone(),
            block_type: BlockType::Card,
            ..Block::default()
        })
        .collect();
    common::insert_blocks(store, &mut cards, &user_id).await;
    let other_id = common::user(store, "other").await;
    let mut boards = Vec::new();
    for _ in 0..3 {
        boards.push(board_administered_by(store, &workspace_id, &user_id, &other_id).await);
    }
    for (card, board) in cards.iter().zip(&boards) {
        store
            .delete_block(&card.id, &user_id)
            .await
            .expect("the card is deleted");
        store
            .delete_container(board, &user_id)
            .await
            .expect("the board is deleted");
    }
    // The first two at one time, after the third; the identifiers decide between them.
    for (table, ids) in [
        (
            "deleted_blocks",
            cards.iter().map(|card| card.id.clone()).collect::<Vec<_>>(),
        ),
        ("deleted_containers", boards.clone()),
    ] {
        for (id, deleted_at) in ids.iter().zip([200, 200, 100]) {
            common::execute_raw(
                &test.database_connection(),
                &format!("UPDATE {table} SET deleted_at = {deleted_at} WHERE id = '{id}'"),
            )
            .await;
        }
    }
    let by_id = |mut ids: Vec<String>| {
        let third = ids.remove(2);
        ids.sort();
        ids.push(third);
        ids
    };

    let listed: Vec<String> = store
        .get_deleted_cards(&board_id, None)
        .await
        .expect("the deleted cards")
        .into_iter()
        .map(|deleted| deleted.block.id)
        .collect();
    assert_eq!(
        listed,
        by_id(cards.iter().map(|card| card.id.clone()).collect())
    );
    let listed: Vec<String> = store
        .get_deleted_containers(&workspace_id, Some(&user_id), None, ContainerKinds::BOARDS)
        .await
        .expect("the deleted boards")
        .into_iter()
        .map(|deleted| deleted.container.id)
        .collect();
    assert_eq!(listed, by_id(boards.clone()));
    let page: Vec<String> = store
        .get_deleted_containers(
            &workspace_id,
            Some(&user_id),
            Some(Paging {
                page: 1,
                per_page: std::num::NonZeroU32::new(2).expect("a page size"),
            }),
            ContainerKinds::BOARDS,
        )
        .await
        .expect("a page of the deleted boards")
        .into_iter()
        .map(|deleted| deleted.container.id)
        .collect();
    assert_eq!(page, by_id(boards.clone())[2..], "the pages cut that order");
    let not_admin = store
        .get_deleted_containers(&workspace_id, Some(&other_id), None, ContainerKinds::BOARDS)
        .await
        .expect("the deleted boards of an editor");
    assert!(not_admin.is_empty(), "only those the user was an admin of");

    test.teardown().await;
}

/// The teams of a workspace come by name in any letter case, and in the same order on
/// both databases: a name in capitals sorts among the others as in lower case, and the
/// rest by code point, beyond the Latin alphabet and the Basic Multilingual Plane too.
/// The order cannot follow the collation of PostgreSQL's database (that of the locale of
/// its image) while SQLite compares bytes.
#[tokio::test]
async fn teams_come_by_name_alike_on_both_databases() {
    let test = common::setup().await;
    let store = &test.store;
    let admin = common::user(store, "team admin").await;
    let workspace_id = common::workspace(store).await;
    store
        .add_workspace_member(&workspace_id, &admin, WorkspaceRole::Admin)
        .await
        .expect("the administrator is let into the workspace");
    for name in ["gamma", "𝔷eta", "Émile", "Beta", "alpha"] {
        store
            .create_team(&workspace_id, name, &admin)
            .await
            .expect("the team is made");
    }

    let teams = store
        .teams(&workspace_id)
        .await
        .expect("the teams are read");
    let names: Vec<&str> = teams.iter().map(|team| team.name.as_str()).collect();
    assert_eq!(
        names,
        ["alpha", "Beta", "gamma", "Émile", "𝔷eta"],
        "{:?}",
        common::db_type()
    );

    test.teardown().await;
}
