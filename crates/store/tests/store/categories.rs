//! Integration tests for sidebar category storage, ordering, updates, and deletion.
//! Every identifier is generated, and the user, workspace, and boards referenced by a
//! category are stored first.

use std::time::{SystemTime, UNIX_EPOCH};

use fvoci_model::{Category, CategoryContainerMetadata, CategoryType, new_id};
use fvoci_store::{Store, StoreError};

use crate::common::{self, TestStore};

/// `utils.GetMillis`.
pub(crate) fn now_millis() -> i64 {
    let since_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is past 1970");
    i64::try_from(since_epoch.as_millis()).expect("the time fits 64 bits")
}

/// New identifiers, as many as the pattern they are bound to names.
pub(crate) fn new_ids<const N: usize>() -> [String; N] {
    std::array::from_fn(|_| new_id())
}

/// Stores `N` open boards of the workspace on behalf of the user and returns their
/// identifiers.
pub(crate) async fn create_boards<const N: usize>(
    store: &Store,
    workspace_id: &str,
    user_id: &str,
) -> [String; N] {
    let boards = common::create_test_boards(store, workspace_id, user_id, N).await;
    let ids: Vec<String> = boards.into_iter().map(|board| board.id).collect();
    ids.try_into()
        .expect("as many boards are stored as were asked for")
}

/// Owned identifiers from borrowed ones.
pub(crate) fn ids(ids: &[&str]) -> Vec<String> {
    ids.iter().map(|&id| id.to_owned()).collect()
}

/// `model.CategoryBoardMetadata{BoardID: board_id, Hidden: false}`.
pub(crate) fn visible(board_id: &str) -> CategoryContainerMetadata {
    CategoryContainerMetadata {
        container_id: board_id.to_owned(),
        hidden: false,
    }
}

/// The identifiers of the categories, in the given order.
fn category_ids(categories: &[Category]) -> Vec<&str> {
    categories
        .iter()
        .map(|category| category.id.as_str())
        .collect()
}

/// The sort orders of the categories, in the given order.
fn sort_orders(categories: &[Category]) -> Vec<i64> {
    categories
        .iter()
        .map(|category| category.sort_order)
        .collect()
}

/// Runs statements on the test's database, bypassing the store, to stage rows the store
/// itself never writes.
async fn execute_raw(test: &TestStore, sql: &str) {
    common::execute_raw(&test.database_connection(), sql).await;
}

/// Creating, fetching, and looking up missing sidebar categories.
#[tokio::test]
async fn get_create_category() {
    let test = common::setup().await;
    let store = &test.store;
    let user_id = common::user(store, "user").await;
    let workspace_id = common::workspace(store).await;

    // "save uncollapsed category"
    {
        let now = now_millis();
        let category = Category {
            id: new_id(),
            name: "Category".to_owned(),
            user_id: user_id.clone(),
            workspace_id: workspace_id.clone(),
            created_at: now,
            updated_at: now,
            deleted_at: None,
            collapsed: false,
            ..Category::default()
        };

        store
            .create_category(&category)
            .await
            .expect("the category is created");

        let created_category = store
            .get_category(&category.id)
            .await
            .expect("the category is found");
        assert_eq!(created_category.name, "Category", "the name");
        assert_eq!(created_category.user_id, user_id, "the user");
        assert_eq!(created_category.workspace_id, workspace_id, "the workspace");
        assert!(!created_category.collapsed, "got {created_category:?}");
    }

    // "save collapsed category"
    {
        let now = now_millis();
        let category = Category {
            id: new_id(),
            name: "Category".to_owned(),
            user_id: user_id.clone(),
            workspace_id: workspace_id.clone(),
            created_at: now,
            updated_at: now,
            deleted_at: None,
            collapsed: true,
            ..Category::default()
        };

        store
            .create_category(&category)
            .await
            .expect("the category is created");

        let created_category = store
            .get_category(&category.id)
            .await
            .expect("the category is found");
        assert_eq!(created_category.name, "Category", "the name");
        assert_eq!(created_category.user_id, user_id, "the user");
        assert_eq!(created_category.workspace_id, workspace_id, "the workspace");
        assert!(created_category.collapsed, "got {created_category:?}");
    }

    // "get nonexistent category"
    {
        // A missing category ID returns a not-found error.
        let nonexistent = new_id();
        let error = store
            .get_category(&nonexistent)
            .await
            .expect_err("there is no such category");
        assert!(matches!(error, StoreError::NotFound(_)), "got {error:?}");
        assert!(error.is_not_found(), "got {error:?}");
        assert_eq!(
            error.to_string(),
            format!("{{category ID={nonexistent}}} not found"),
            "the error names the category"
        );

        // Text that is not a UUID identifier names no category either.
        let error = store
            .get_category("nonexistent")
            .await
            .expect_err("text that is not an identifier names no category");
        assert!(matches!(error, StoreError::NotFound(_)), "got {error:?}");
        assert_eq!(
            error.to_string(),
            "{category ID=nonexistent} not found",
            "the error names what was asked for"
        );
    }

    test.teardown().await;
}

/// Updating a category.
#[tokio::test]
async fn update_category() {
    let test = common::setup().await;
    let store = &test.store;
    let user_id = common::user(store, "user").await;
    let workspace_id = common::workspace(store).await;

    let now = now_millis();
    let category = Category {
        id: new_id(),
        name: "Category 1".to_owned(),
        user_id: user_id.clone(),
        workspace_id: workspace_id.clone(),
        created_at: now,
        updated_at: now,
        deleted_at: None,
        collapsed: false,
        ..Category::default()
    };

    store
        .create_category(&category)
        .await
        .expect("the category is created");

    let update_now = now_millis();
    let mut updated_category = Category {
        id: category.id.clone(),
        name: "Category 1 New".to_owned(),
        user_id,
        workspace_id,
        created_at: now,
        updated_at: update_now,
        deleted_at: None,
        collapsed: true,
        ..Category::default()
    };

    store
        .update_category(&updated_category)
        .await
        .expect("the category is updated");

    let fetched_category = store
        .get_category(&category.id)
        .await
        .expect("the category is found");
    assert_eq!(fetched_category.id, category.id, "the same category");
    assert_eq!(fetched_category.name, "Category 1 New", "the new name");
    assert!(fetched_category.collapsed, "got {fetched_category:?}");

    // now lets try to un-collapse the same category
    updated_category.collapsed = false;
    store
        .update_category(&updated_category)
        .await
        .expect("the category is updated");

    let fetched_category = store
        .get_category(&category.id)
        .await
        .expect("the category is found");
    assert_eq!(fetched_category.id, category.id, "the same category");
    assert_eq!(fetched_category.name, "Category 1 New", "the name stays");
    assert!(!fetched_category.collapsed, "got {fetched_category:?}");

    test.teardown().await;
}

/// Deleting a category.
#[tokio::test]
async fn delete_category() {
    let test = common::setup().await;
    let store = &test.store;
    let user_id = common::user(store, "user").await;
    let workspace_id = common::workspace(store).await;

    let now = now_millis();
    let category = Category {
        id: new_id(),
        name: "Category 1".to_owned(),
        user_id: user_id.clone(),
        workspace_id: workspace_id.clone(),
        created_at: now,
        updated_at: now,
        deleted_at: None,
        collapsed: false,
        ..Category::default()
    };

    store
        .create_category(&category)
        .await
        .expect("the category is created");

    store
        .delete_category(&category.id, &user_id, &workspace_id)
        .await
        .expect("the category is deleted");

    // A deleted category is removed and no longer found.
    let error = store
        .get_category(&category.id)
        .await
        .expect_err("a deleted category is not found");
    assert!(matches!(error, StoreError::NotFound(_)), "got {error:?}");

    test.teardown().await;
}

/// Listing a user's categories in a workspace, excluding deleted ones.
#[tokio::test]
async fn get_user_categories() {
    let test = common::setup().await;
    let store = &test.store;
    let user_id = common::user(store, "user").await;
    let workspace_id = common::workspace(store).await;

    let now = now_millis();
    let category1 = Category {
        id: new_id(),
        name: "Category 1".to_owned(),
        user_id: user_id.clone(),
        workspace_id: workspace_id.clone(),
        created_at: now,
        updated_at: now,
        deleted_at: None,
        collapsed: false,
        ..Category::default()
    };
    store
        .create_category(&category1)
        .await
        .expect("category 1 is created");

    let category2 = Category {
        id: new_id(),
        name: "Category 2".to_owned(),
        user_id: user_id.clone(),
        workspace_id: workspace_id.clone(),
        created_at: now,
        updated_at: now,
        deleted_at: None,
        collapsed: false,
        ..Category::default()
    };
    store
        .create_category(&category2)
        .await
        .expect("category 2 is created");

    // Duplicate category names are allowed; this third category shares "Category 2".
    let category3 = Category {
        id: new_id(),
        name: "Category 2".to_owned(),
        user_id: user_id.clone(),
        workspace_id: workspace_id.clone(),
        created_at: now,
        updated_at: now,
        deleted_at: None,
        collapsed: false,
        ..Category::default()
    };
    store
        .create_category(&category3)
        .await
        .expect("category 3 is created");

    let user_categories = store
        .get_user_category_containers(&user_id, &workspace_id)
        .await
        .expect("the categories are listed");
    assert_eq!(user_categories.len(), 3, "got {user_categories:?}");

    test.teardown().await;
}

/// Reordering the categories of a user.
#[tokio::test]
async fn reorder_categories() {
    let test = common::setup().await;
    let store = &test.store;
    let user_id = common::user(store, "user").await;
    let workspace_id = common::workspace(store).await;

    // setup
    let [category1, category2, category3] = new_ids();
    for (id, name) in [
        (&category1, "Category 1"),
        (&category2, "Category 2"),
        (&category3, "Category 3"),
    ] {
        store
            .create_category(&Category {
                id: id.clone(),
                name: name.to_owned(),
                category_type: CategoryType::Custom,
                user_id: user_id.clone(),
                workspace_id: workspace_id.clone(),
                ..Category::default()
            })
            .await
            .expect("the category is created");
    }

    // verify the current order
    let categories = store
        .get_user_categories(&user_id, &workspace_id)
        .await
        .expect("the categories are listed");

    // the categories should show up in reverse insertion order (latest one first)
    assert_eq!(
        category_ids(&categories),
        [&category3, &category2, &category1],
        "the latest category comes first"
    );

    // re-ordering categories normally
    store
        .reorder_categories(
            &user_id,
            &workspace_id,
            ids(&[&category2, &category3, &category1]),
        )
        .await
        .expect("the categories are reordered");

    // verify the board order
    let categories = store
        .get_user_categories(&user_id, &workspace_id)
        .await
        .expect("the categories are listed");
    assert_eq!(
        category_ids(&categories),
        [&category2, &category3, &category1],
        "the order asked for"
    );

    // lets try specifying a non existing category ID.
    // It shouldn't cause any problem
    store
        .reorder_categories(
            &user_id,
            &workspace_id,
            ids(&[&category1, &category2, &category3, &new_id()]),
        )
        .await
        .expect("an unknown identifier is ignored");

    let categories = store
        .get_user_categories(&user_id, &workspace_id)
        .await
        .expect("the categories are listed");
    assert_eq!(
        category_ids(&categories),
        [&category1, &category2, &category3],
        "the order asked for, without the unknown category"
    );

    test.teardown().await;
}

/// Reordering the boards of a category.
#[tokio::test]
async fn reorder_category_containers() {
    let test = common::setup().await;
    let store = &test.store;
    let user_id = common::user(store, "user").await;
    let workspace_id = common::workspace(store).await;

    // setup
    // Boards filed in a category must exist first.
    let category_id = new_id();
    store
        .create_category(&Category {
            id: category_id.clone(),
            name: "Category 1".to_owned(),
            category_type: CategoryType::Custom,
            user_id: user_id.clone(),
            workspace_id: workspace_id.clone(),
            ..Category::default()
        })
        .await
        .expect("the category is created");

    let [board1, board2, board3, board4] = create_boards(store, &workspace_id, &user_id).await;

    store
        .add_update_category_container(
            &user_id,
            &category_id,
            &ids(&[&board1, &board2, &board3, &board4]),
        )
        .await
        .expect("the boards are added");

    // verify current order
    let category_boards = store
        .get_user_category_containers(&user_id, &workspace_id)
        .await
        .expect("the categories are listed");
    assert_eq!(category_boards.len(), 1, "got {category_boards:?}");
    let metadata = &category_boards[0].container_metadata;
    assert_eq!(metadata.len(), 4, "got {metadata:?}");
    for board_id in [&board1, &board2, &board3, &board4] {
        assert!(
            metadata.contains(&visible(board_id)),
            "board {board_id} is not in {metadata:?}"
        );
    }

    // reordering
    let order = ids(&[&board3, &board1, &board2, &board4]);
    let new_order = store
        .reorder_category_containers(&category_id, order.clone())
        .await
        .expect("the boards are reordered");
    assert_eq!(new_order, order, "the order is returned as it was given");

    // verify new order
    let category_boards = store
        .get_user_category_containers(&user_id, &workspace_id)
        .await
        .expect("the categories are listed");
    assert_eq!(category_boards.len(), 1, "got {category_boards:?}");
    let metadata = &category_boards[0].container_metadata;
    let listed: Vec<&String> = metadata.iter().map(|board| &board.container_id).collect();
    assert_eq!(
        listed,
        [&board3, &board1, &board2, &board4],
        "the boards are listed in the new order"
    );

    test.teardown().await;
}

/// The sort-order arithmetic of creating and reordering, and which rows each statement
/// reaches.
#[tokio::test]
async fn sort_orders_of_created_and_reordered_categories() {
    let test = common::setup().await;
    let store = &test.store;
    let user_id = common::user(store, "user").await;
    let other_user_id = common::user(store, "other").await;
    let workspace_id = common::workspace(store).await;
    let [a, b, c, other] = new_ids();

    let category = |id: &str, user_id: &str, sort_order: i64| Category {
        id: id.to_owned(),
        name: format!("Category {id}"),
        user_id: user_id.to_owned(),
        workspace_id: workspace_id.clone(),
        sort_order,
        ..Category::default()
    };
    let sort_order = async |id: &str| {
        store
            .get_category(id)
            .await
            .expect("the category is found")
            .sort_order
    };

    // A new category is bumped together with the ones that were there before it.
    store
        .create_category(&category(&other, &other_user_id, 0))
        .await
        .expect("another user's category is created");
    store
        .create_category(&category(&a, &user_id, 0))
        .await
        .expect("a is created");
    assert_eq!(sort_order(&a).await, 10, "a moves down with itself");
    store
        .create_category(&category(&b, &user_id, 70))
        .await
        .expect("b is created");
    assert_eq!(sort_order(&a).await, 20, "a moves down for b");
    assert_eq!(sort_order(&b).await, 80, "b moves down with itself");
    assert_eq!(
        sort_order(&other).await,
        10,
        "another user's categories stay where they are"
    );

    // A deleted category is gone. An order that still names it leaves its place empty.
    store
        .delete_category(&a, &user_id, &workspace_id)
        .await
        .expect("a is deleted");
    store
        .create_category(&category(&c, &user_id, 0))
        .await
        .expect("c is created");
    assert_eq!(sort_order(&b).await, 90, "b moves down for c");
    assert_eq!(sort_order(&c).await, 10, "c moves down with itself");

    let order = ids(&[&b, &a, &c, &other]);
    let echoed = store
        .reorder_categories(&user_id, &workspace_id, order.clone())
        .await
        .expect("the categories are reordered");
    assert_eq!(echoed, order, "the order comes back as it was given");
    assert_eq!(sort_order(&b).await, 10, "b is first, from the first gap");
    assert_eq!(
        sort_order(&c).await,
        30,
        "c is third: the deleted category is named second"
    );
    let error = store
        .get_category(&a)
        .await
        .expect_err("an order that names a deleted category does not bring it back");
    assert!(error.is_not_found(), "got {error:?}");
    assert_eq!(
        sort_order(&other).await,
        10,
        "another user's category is out of reach"
    );

    // An empty order changes nothing.
    let echoed = store
        .reorder_categories(&user_id, &workspace_id, Vec::new())
        .await
        .expect("an empty order is accepted");
    assert!(echoed.is_empty(), "got {echoed:?}");
    assert_eq!(sort_order(&c).await, 30, "c stays third");

    test.teardown().await;
}

/// Category reorder statements bind identifiers as data parameters, and non-UUID text is
/// refused before the statement runs.
#[tokio::test]
async fn reorder_categories_treats_identifiers_as_data() {
    let test = common::setup().await;
    let store = &test.store;
    let user_id = common::user(store, "user").await;
    let workspace_id = common::workspace(store).await;

    let [first, second] = new_ids();
    for id in [&first, &second] {
        store
            .create_category(&Category {
                id: id.clone(),
                name: id.clone(),
                user_id: user_id.clone(),
                workspace_id: workspace_id.clone(),
                ..Category::default()
            })
            .await
            .expect("the category is created");
    }

    // Spliced into `CASE id WHEN '<id>' THEN <n> ...`, the first entry would move every
    // category of the user to position 5.
    let spliced = "x' THEN 5 ELSE 5 END --";
    let error = store
        .reorder_categories(&user_id, &workspace_id, ids(&[spliced, &first, &second]))
        .await
        .expect_err("text that is not an identifier is refused");
    assert!(
        matches!(&error, StoreError::InvalidId(refused) if refused == spliced),
        "got {error:?}"
    );

    // Nothing moved, the categories the order names after the refused entry included.
    let categories = store
        .get_user_categories(&user_id, &workspace_id)
        .await
        .expect("the categories are listed");
    assert_eq!(
        category_ids(&categories),
        [&second, &first],
        "the later category still comes first"
    );
    assert_eq!(sort_orders(&categories), [10, 20], "as they were created");

    // An identifier with nothing behind it is ignored, wherever the order names it.
    store
        .reorder_categories(&user_id, &workspace_id, ids(&[&new_id(), &first, &second]))
        .await
        .expect("an identifier that names no category is accepted");
    let categories = store
        .get_user_categories(&user_id, &workspace_id)
        .await
        .expect("the categories are listed");
    assert_eq!(
        category_ids(&categories),
        [&first, &second],
        "the order asked for"
    );
    assert_eq!(
        sort_orders(&categories),
        [20, 30],
        "the unknown identifier takes the first place with it"
    );

    test.teardown().await;
}

/// A system category of the user in the workspace, for the tests below.
fn boards_category(user_id: &str, workspace_id: &str) -> Category {
    Category {
        id: new_id(),
        name: "Boards".to_owned(),
        user_id: user_id.to_owned(),
        workspace_id: workspace_id.to_owned(),
        created_at: 1000,
        updated_at: 1000,
        category_type: CategoryType::System,
        ..Category::default()
    }
}

/// An update writes the name, the modification time and the collapsed state, and nothing
/// else.
#[tokio::test]
async fn update_category_leaves_other_members_and_rows_alone() {
    let test = common::setup().await;
    let store = &test.store;
    let user_id = common::user(store, "user").await;
    let someone_else = common::user(store, "someone-else").await;
    let workspace_id = common::workspace(store).await;
    let another_workspace = common::workspace(store).await;

    let category = boards_category(&user_id, &workspace_id);
    store
        .create_category(&category)
        .await
        .expect("the category is created");

    store
        .update_category(&Category {
            name: "Renamed".to_owned(),
            user_id: someone_else,
            workspace_id: another_workspace,
            created_at: 5,
            updated_at: 2000,
            deleted_at: Some(3000),
            collapsed: true,
            sort_order: 500,
            category_type: CategoryType::Custom,
            ..category.clone()
        })
        .await
        .expect("the category is updated");
    let stored = store
        .get_category(&category.id)
        .await
        .expect("the category is found");
    assert_eq!(
        stored,
        Category {
            name: "Renamed".to_owned(),
            updated_at: 2000,
            collapsed: true,
            sort_order: 10,
            ..category.clone()
        },
        "only the name, the modification time and the collapsed state change"
    );

    // Updating a category that does not exist succeeds and writes nothing.
    let nonexistent = new_id();
    store
        .update_category(&Category {
            id: nonexistent.clone(),
            ..category
        })
        .await
        .expect("updating nothing succeeds");
    let error = store
        .get_category(&nonexistent)
        .await
        .expect_err("an update does not create a category");
    assert!(error.is_not_found(), "got {error:?}");

    test.teardown().await;
}

/// A deletion needs the owner and the workspace to match. A deleted category is gone: it
/// is not found and not listed, and updating or deleting it again succeeds and brings
/// nothing back.
#[tokio::test]
async fn delete_category_matches_owner_and_workspace_and_happens_once() {
    let test = common::setup().await;
    let store = &test.store;
    let user_id = common::user(store, "user").await;
    let someone_else = common::user(store, "someone-else").await;
    let workspace_id = common::workspace(store).await;
    let another_workspace = common::workspace(store).await;

    let category = boards_category(&user_id, &workspace_id);
    store
        .create_category(&category)
        .await
        .expect("the category is created");
    let created = store
        .get_category(&category.id)
        .await
        .expect("the category is found");

    store
        .delete_category(&category.id, &someone_else, &workspace_id)
        .await
        .expect("deleting nothing succeeds");
    store
        .delete_category(&category.id, &user_id, &another_workspace)
        .await
        .expect("deleting nothing succeeds");
    let stored = store
        .get_category(&category.id)
        .await
        .expect("the category is found");
    assert_eq!(
        stored, created,
        "neither another user nor another workspace deletes the category"
    );
    let listed = store
        .get_user_categories(&user_id, &workspace_id)
        .await
        .expect("the categories are listed");
    assert_eq!(category_ids(&listed), [&category.id], "still listed");

    store
        .delete_category(&category.id, &user_id, &workspace_id)
        .await
        .expect("the category is deleted");
    let error = store
        .get_category(&category.id)
        .await
        .expect_err("a deleted category is not found");
    assert!(matches!(error, StoreError::NotFound(_)), "got {error:?}");
    let listed = store
        .get_user_categories(&user_id, &workspace_id)
        .await
        .expect("the categories are listed");
    assert!(
        listed.is_empty(),
        "a deleted category is not listed: {listed:?}"
    );

    store
        .update_category(&Category {
            name: "Renamed again".to_owned(),
            ..category.clone()
        })
        .await
        .expect("updating a deleted category succeeds");
    store
        .delete_category(&category.id, &user_id, &workspace_id)
        .await
        .expect("deleting a deleted category succeeds");
    let error = store
        .get_category(&category.id)
        .await
        .expect_err("neither brings the deleted category back");
    assert!(matches!(error, StoreError::NotFound(_)), "got {error:?}");
    let listed = store
        .get_user_category_containers(&user_id, &workspace_id)
        .await
        .expect("the categories are listed");
    assert!(listed.is_empty(), "still not listed: {listed:?}");

    test.teardown().await;
}

/// A category belongs to an existing user in an existing workspace: without either, it is
/// not stored.
#[tokio::test]
async fn a_category_of_a_user_or_a_workspace_that_does_not_exist_is_refused() {
    let test = common::setup().await;
    let store = &test.store;
    let user_id = common::user(store, "user").await;
    let workspace_id = common::workspace(store).await;

    let existing = boards_category(&user_id, &workspace_id);
    store
        .create_category(&existing)
        .await
        .expect("a category of a user in a workspace is created");
    let before = store
        .get_user_categories(&user_id, &workspace_id)
        .await
        .expect("the categories are listed");

    let of_nobody = boards_category(&new_id(), &workspace_id);
    let in_no_workspace = boards_category(&user_id, &new_id());
    for (category, what) in [
        (&of_nobody, "no such user"),
        (&in_no_workspace, "no such workspace"),
    ] {
        let error = store
            .create_category(category)
            .await
            .expect_err("a category that refers to nothing is refused");
        assert!(
            common::refused_over_a_foreign_key(&error),
            "{what}: got {error:?}"
        );
        let error = store
            .get_category(&category.id)
            .await
            .expect_err("the refused category is not stored");
        assert!(error.is_not_found(), "{what}: got {error:?}");
    }

    let after = store
        .get_user_categories(&user_id, &workspace_id)
        .await
        .expect("the categories are listed");
    assert_eq!(after, before, "the refused categories changed nothing");

    test.teardown().await;
}

/// Rows inserted with column defaults read back uncollapsed with sort order zero, and
/// boards filed under them default to visible. (A type is not among what may be left out,
/// and is one of the two the schema knows: the schema tests have the refusals.)
#[tokio::test]
async fn categories_written_by_others() {
    let test = common::setup().await;
    let store = &test.store;
    let user_id = common::user(store, "user").await;
    let workspace_id = common::workspace(store).await;
    let [board_id] = create_boards(store, &workspace_id, &user_id).await;
    let [custom_type, system_type] = new_ids();

    execute_raw(
        &test,
        &format!(
            "INSERT INTO categories (id, name, user_id, workspace_id, created_at, updated_at, type) \
             VALUES \
             ('{custom_type}', 'A', '{user_id}', '{workspace_id}', 1, 1, 'custom'), \
             ('{system_type}', 'B', '{user_id}', '{workspace_id}', 1, 1, 'system')"
        ),
    )
    .await;
    execute_raw(
        &test,
        &format!(
            "INSERT INTO category_containers (id, user_id, category_id, container_id, created_at, \
             updated_at) VALUES ('{}', '{user_id}', '{custom_type}', '{board_id}', 1, 1)",
            new_id()
        ),
    )
    .await;

    let listed = store
        .get_user_category_containers(&user_id, &workspace_id)
        .await
        .expect("the categories are listed");
    assert_eq!(listed.len(), 2, "got {listed:?}");
    for entry in &listed {
        let category = &entry.category;
        let expected_type = if category.id == system_type {
            CategoryType::System
        } else {
            CategoryType::Custom
        };
        assert_eq!(category.category_type, expected_type, "{category:?}");
        assert_eq!(category.sort_order, 0, "{category:?}");
        assert_eq!(category.deleted_at, None, "{category:?}");
        assert!(!category.collapsed, "{category:?}");
    }
    let with_board = listed
        .iter()
        .find(|entry| entry.category.id == custom_type)
        .expect("the category is listed");
    assert_eq!(
        with_board.container_metadata,
        [visible(&board_id)],
        "the board of the category"
    );
    for category_id in [&custom_type, &system_type] {
        store
            .get_category(category_id)
            .await
            .expect("each row is read on its own too");
    }

    test.teardown().await;
}
