//! Integration tests for filing, moving, reordering, and hiding boards within sidebar
//! categories. Every identifier is generated, and referenced users, workspaces, categories,
//! and boards are stored first.

use fvoci_model::{
    Category, CategoryContainerMetadata, CategoryContainers, SidebarVisibility, new_id,
};
use fvoci_store::{Store, StoreError};

use crate::categories::{create_boards, ids, now_millis, visible};
use crate::common;

/// The user and the workspace whose sidebar a test looks at.
struct Sidebar {
    user_id: String,
    workspace_id: String,
}

impl Sidebar {
    /// Stores a user and a workspace.
    async fn create(store: &Store) -> Self {
        Self {
            user_id: common::user(store, "user").await,
            workspace_id: common::workspace(store).await,
        }
    }

    /// Creates a category of the user in the workspace and returns its identifier.
    async fn create_category(&self, store: &Store, name: &str) -> String {
        create_category(store, &self.user_id, &self.workspace_id, name).await
    }

    /// The categories of the user in the workspace, each with its boards.
    async fn category_boards(&self, store: &Store) -> Vec<CategoryContainers> {
        store
            .get_user_category_containers(&self.user_id, &self.workspace_id)
            .await
            .expect("the categories are listed")
    }

    /// The boards of the category with the given identifier, in display order.
    async fn boards_of(&self, store: &Store, category_id: &str) -> Vec<CategoryContainerMetadata> {
        self.category_boards(store)
            .await
            .into_iter()
            .find(|category_boards| category_boards.category.id == category_id)
            .expect("the category is listed")
            .container_metadata
    }
}

/// Creates a category of a user in a workspace and returns its identifier.
async fn create_category(store: &Store, user_id: &str, workspace_id: &str, name: &str) -> String {
    let category = Category {
        id: new_id(),
        name: name.to_owned(),
        user_id: user_id.to_owned(),
        workspace_id: workspace_id.to_owned(),
        ..Category::default()
    };
    store
        .create_category(&category)
        .await
        .expect("the category is created");
    category.id
}

/// The identifiers of the boards, in the given order.
fn board_ids(metadata: &[CategoryContainerMetadata]) -> Vec<&str> {
    metadata
        .iter()
        .map(|board| board.container_id.as_str())
        .collect()
}

/// Listing a user's categories together with the boards filed in each.
#[tokio::test]
async fn get_user_category_containers() {
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

    let category3 = Category {
        id: new_id(),
        name: "Category 3".to_owned(),
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

    // Adding Board 1 and Board 2 to Category 1.
    let [board1, board2, board3] = create_boards(store, &workspace_id, &user_id).await;
    store
        .add_update_category_container(&user_id, &category1.id, std::slice::from_ref(&board1))
        .await
        .expect("board 1 is added");

    store
        .add_update_category_container(&user_id, &category1.id, std::slice::from_ref(&board2))
        .await
        .expect("board 2 is added");

    // Adding Board 3 to Category 2
    store
        .add_update_category_container(&user_id, &category2.id, std::slice::from_ref(&board3))
        .await
        .expect("board 3 is added");

    // we'll leave category 3 empty

    let user_category_boards = store
        .get_user_category_containers(&user_id, &workspace_id)
        .await
        .expect("the categories are listed");

    // we created 3 categories for the user
    assert_eq!(
        user_category_boards.len(),
        3,
        "got {user_category_boards:?}"
    );

    // Verify that all three categories appear with their expected board counts.
    let board_count = |category: &Category| {
        user_category_boards
            .iter()
            .find(|category_boards| category_boards.category.id == category.id)
            .map(|category_boards| category_boards.container_metadata.len())
    };
    assert_eq!(board_count(&category1), Some(2), "boards 1 and 2");
    assert_eq!(board_count(&category2), Some(1), "board 3");
    assert_eq!(board_count(&category3), Some(0), "an empty category");

    // "get empty category boards"
    {
        let user_category_boards = store
            .get_user_category_containers(&new_id(), &new_id())
            .await
            .expect("an unknown user has no categories");
        assert!(
            user_category_boards.is_empty(),
            "got {user_category_boards:?}"
        );
    }

    test.teardown().await;
}

/// Filing a board in a category and filing it again.
#[tokio::test]
async fn add_update_category_container() {
    let test = common::setup().await;
    let store = &test.store;
    let sidebar = Sidebar::create(store).await;
    let user_id = &sidebar.user_id;

    // Create five boards to file and move across categories.
    let [board1, board2, board3, board4, board5] =
        create_boards(store, &sidebar.workspace_id, user_id).await;

    let category_id = sidebar.create_category(store, "Category").await;

    // adding a few boards to the category
    store
        .add_update_category_container(user_id, &category_id, &ids(&[&board1, &board2]))
        .await
        .expect("the boards are added");

    // verify inserted data
    let category_boards = sidebar.category_boards(store).await;
    assert_eq!(category_boards.len(), 1, "got {category_boards:?}");
    assert_eq!(category_boards[0].category.id, category_id, "the category");
    let metadata = &category_boards[0].container_metadata;
    assert_eq!(metadata.len(), 2, "got {metadata:?}");
    assert!(metadata.contains(&visible(&board1)), "{metadata:?}");
    assert!(metadata.contains(&visible(&board2)), "{metadata:?}");

    // adding new boards to the same category
    store
        .add_update_category_container(user_id, &category_id, std::slice::from_ref(&board3))
        .await
        .expect("a third board is added");

    // verify inserted data
    let category_boards = sidebar.category_boards(store).await;
    assert_eq!(category_boards.len(), 1, "got {category_boards:?}");
    assert_eq!(category_boards[0].category.id, category_id, "the category");
    let metadata = &category_boards[0].container_metadata;
    assert_eq!(metadata.len(), 3, "got {metadata:?}");
    assert!(metadata.contains(&visible(&board3)), "{metadata:?}");

    // passing empty array
    store
        .add_update_category_container(user_id, &category_id, &[])
        .await
        .expect("no boards are added");

    // verify inserted data
    let category_boards = sidebar.category_boards(store).await;
    assert_eq!(category_boards.len(), 1, "got {category_boards:?}");
    assert_eq!(category_boards[0].category.id, category_id, "the category");
    let metadata = &category_boards[0].container_metadata;
    assert_eq!(metadata.len(), 3, "got {metadata:?}");

    // passing duplicate data in input
    store
        .add_update_category_container(user_id, &category_id, &ids(&[&board4, &board4]))
        .await
        .expect("a board named twice is added once");

    // verify inserted data
    let category_boards = sidebar.category_boards(store).await;
    assert_eq!(category_boards.len(), 1, "got {category_boards:?}");
    assert_eq!(category_boards[0].category.id, category_id, "the category");
    let metadata = &category_boards[0].container_metadata;
    assert_eq!(metadata.len(), 4, "got {metadata:?}");
    assert!(metadata.contains(&visible(&board4)), "{metadata:?}");

    // adding already added board
    store
        .add_update_category_container(user_id, &category_id, &ids(&[&board1, &board2]))
        .await
        .expect("boards that are in the category already are accepted");

    // verify inserted data
    let category_boards = sidebar.category_boards(store).await;
    assert_eq!(category_boards.len(), 1, "got {category_boards:?}");
    assert_eq!(category_boards[0].category.id, category_id, "the category");
    let metadata = &category_boards[0].container_metadata;
    assert_eq!(metadata.len(), 4, "got {metadata:?}");

    // passing already added board along with a new board
    store
        .add_update_category_container(user_id, &category_id, &ids(&[&board1, &board5]))
        .await
        .expect("a known and a new board are accepted together");

    // verify inserted data
    let category_boards = sidebar.category_boards(store).await;
    assert_eq!(category_boards.len(), 1, "got {category_boards:?}");
    assert_eq!(category_boards[0].category.id, category_id, "the category");
    let metadata = &category_boards[0].container_metadata;
    assert_eq!(metadata.len(), 5, "got {metadata:?}");
    assert!(metadata.contains(&visible(&board5)), "{metadata:?}");

    test.teardown().await;
}

/// Setting whether a board shows in its category.
#[tokio::test]
async fn set_container_visibility() {
    let test = common::setup().await;
    let store = &test.store;
    let sidebar = Sidebar::create(store).await;
    let user_id = &sidebar.user_id;

    // Create the board the category will hold.
    let [board1] = create_boards(store, &sidebar.workspace_id, user_id).await;

    let category_id = sidebar.create_category(store, "Category").await;

    // adding a few boards to the category
    store
        .add_update_category_container(user_id, &category_id, std::slice::from_ref(&board1))
        .await
        .expect("the board is added");

    store
        .set_container_visibility(user_id, &category_id, &board1, SidebarVisibility::Visible)
        .await
        .expect("the board is made visible");

    // verify set visibility
    let category_boards = sidebar.category_boards(store).await;
    assert_eq!(category_boards.len(), 1, "got {category_boards:?}");
    assert_eq!(category_boards[0].category.id, category_id, "the category");
    let metadata = &category_boards[0].container_metadata;
    assert_eq!(metadata.len(), 1, "got {metadata:?}");
    assert!(!metadata[0].hidden, "got {metadata:?}");

    store
        .set_container_visibility(user_id, &category_id, &board1, SidebarVisibility::Hidden)
        .await
        .expect("the board is hidden");

    // verify set visibility
    let category_boards = sidebar.category_boards(store).await;
    let metadata = &category_boards[0].container_metadata;
    assert!(metadata[0].hidden, "got {metadata:?}");

    test.teardown().await;
}

/// Where boards land when they are added, reordered and moved, and what a move keeps.
#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "each step starts from the places the steps before it left the boards in"
)]
async fn sort_orders_of_added_reordered_and_moved_boards() {
    let test = common::setup().await;
    let store = &test.store;
    let sidebar = Sidebar::create(store).await;
    let user_id = &sidebar.user_id;
    let someone_else = common::user(store, "someone-else").await;
    let a = sidebar.create_category(store, "a").await;
    let b = sidebar.create_category(store, "b").await;
    let elsewhere = create_category(store, &someone_else, &sidebar.workspace_id, "elsewhere").await;
    let [one, two, three, four, theirs] =
        create_boards(store, &sidebar.workspace_id, user_id).await;

    // An identifier that names no board of the category is ignored and comes back with
    // the rest; the place it would have taken stays empty.
    store
        .add_update_category_container(user_id, &a, &ids(&[&one, &two, &three]))
        .await
        .expect("the boards are added");
    let order = ids(&[&three, &new_id(), &one, &two]);
    let echoed = store
        .reorder_category_containers(&a, order.clone())
        .await
        .expect("an identifier that names no board of the category is accepted");
    assert_eq!(echoed, order, "the order comes back as it was given");
    assert_eq!(
        board_ids(&sidebar.boards_of(store, &a).await),
        [&three, &one, &two],
        "the order asked for"
    );

    // SQL-like non-UUID text is refused before the reorder statement runs.
    let spliced = "x' THEN 5 ELSE 5 END --";
    let error = store
        .reorder_category_containers(&a, ids(&[&two, spliced, &one, &three]))
        .await
        .expect_err("text that is not an identifier is refused");
    assert!(
        matches!(&error, StoreError::InvalidId(refused) if refused == spliced),
        "got {error:?}"
    );
    assert_eq!(
        board_ids(&sidebar.boards_of(store, &a).await),
        [&three, &one, &two],
        "the refused order moved nothing"
    );

    // A board added later sorts before the reordered ones.
    store
        .add_update_category_container(user_id, &a, std::slice::from_ref(&four))
        .await
        .expect("a fourth board is added");
    assert_eq!(
        board_ids(&sidebar.boards_of(store, &a).await),
        [&four, &three, &one, &two],
        "a new board comes first"
    );

    // A board moved to another category keeps its hidden flag and its sort order.
    store
        .set_container_visibility(user_id, &a, &two, SidebarVisibility::Hidden)
        .await
        .expect("the board is hidden");
    store
        .add_update_category_container(user_id, &b, &ids(&[&two, &one, &two]))
        .await
        .expect("the boards are moved");
    assert_eq!(
        board_ids(&sidebar.boards_of(store, &a).await),
        [&four, &three],
        "the moved boards have left their category"
    );
    assert_eq!(
        sidebar.boards_of(store, &b).await,
        [
            visible(&one),
            CategoryContainerMetadata {
                container_id: two.clone(),
                hidden: true,
            },
        ],
        "the moved boards keep their order and their hidden flag"
    );

    // Visibility is keyed by user, category and board; a mismatch changes nothing.
    store
        .set_container_visibility(user_id, &a, &two, SidebarVisibility::Visible)
        .await
        .expect("a board of another category is not found, which is not an error");
    store
        .set_container_visibility(&someone_else, &b, &two, SidebarVisibility::Visible)
        .await
        .expect("a board of another user is not found, which is not an error");
    assert!(
        sidebar.boards_of(store, &b).await[1].hidden,
        "the board is still hidden"
    );

    // Rows are unique per user and board, so another user may hold the same board in a
    // category of their own.
    store
        .add_update_category_container(&someone_else, &elsewhere, std::slice::from_ref(&one))
        .await
        .expect("the board is added for another user");
    assert_eq!(
        board_ids(&sidebar.boards_of(store, &b).await),
        [&one, &two],
        "the user's own place for the board is untouched"
    );

    // A category lists the rows of every user: a board that another user puts into this
    // category shows up in it.
    store
        .add_update_category_container(&someone_else, &b, std::slice::from_ref(&theirs))
        .await
        .expect("another user's board is added to the category");
    assert_eq!(
        board_ids(&sidebar.boards_of(store, &b).await),
        [&theirs, &one, &two],
        "the other user's board is listed with the category"
    );

    // An empty order changes nothing.
    let echoed = store
        .reorder_category_containers(&b, Vec::new())
        .await
        .expect("an empty order is accepted");
    assert!(echoed.is_empty(), "got {echoed:?}");
    assert_eq!(
        board_ids(&sidebar.boards_of(store, &b).await),
        [&theirs, &one, &two],
        "nothing moved"
    );

    test.teardown().await;
}

/// A board of the category that a new order leaves out keeps its sort order on both
/// SQLite and PostgreSQL.
#[tokio::test]
async fn reorder_category_containers_keeps_the_sort_order_of_unlisted_boards() {
    let test = common::setup().await;
    let store = &test.store;
    let sidebar = Sidebar::create(store).await;
    let category_id = sidebar.create_category(store, "Category").await;
    let [x, y, z] = create_boards(store, &sidebar.workspace_id, &sidebar.user_id).await;

    store
        .add_update_category_container(&sidebar.user_id, &category_id, &ids(&[&x, &y, &z]))
        .await
        .expect("the boards are added");
    store
        .reorder_category_containers(&category_id, ids(&[&x, &y, &z]))
        .await
        .expect("the boards are ordered");
    assert_eq!(
        board_ids(&sidebar.boards_of(store, &category_id).await),
        [&x, &y, &z],
        "the order asked for"
    );

    // The boards have the sort orders 10, 11 and 12. An order that names the first two
    // gives them 10 and 11 again, and the third stays behind them on both databases.
    store
        .reorder_category_containers(&category_id, ids(&[&y, &x]))
        .await
        .expect("an order that leaves out a board is accepted");
    assert_eq!(
        board_ids(&sidebar.boards_of(store, &category_id).await),
        [&y, &x, &z],
        "the board the order leaves out keeps the last place"
    );

    // The numbering starts over at 10 with every order, so a board that is left out may
    // come to share its sort order with one that is named: here `y`, which keeps 10,
    // with `z`. The two come before `x`, in no particular order.
    store
        .reorder_category_containers(&category_id, ids(&[&z, &x]))
        .await
        .expect("an order that leaves out a board is accepted");
    let boards = sidebar.boards_of(store, &category_id).await;
    let mut first_two = board_ids(&boards[..2]);
    first_two.sort_unstable();
    let mut expected = [y.as_str(), z.as_str()];
    expected.sort_unstable();
    assert_eq!(first_two, expected, "the boards with sort order 10");
    assert_eq!(boards[2].container_id, x, "the board with sort order 11");

    test.teardown().await;
}

/// Deleting a category removes it, and the boards filed under it are filed under nothing:
/// no category lists them until they are filed again, which files them as new.
#[tokio::test]
async fn deleting_a_category_files_its_boards_under_nothing() {
    let test = common::setup().await;
    let store = &test.store;
    let sidebar = Sidebar::create(store).await;
    let user_id = &sidebar.user_id;
    let doomed = sidebar.create_category(store, "doomed").await;
    let kept = sidebar.create_category(store, "kept").await;
    let [one, two, three] = create_boards(store, &sidebar.workspace_id, user_id).await;

    store
        .add_update_category_container(user_id, &doomed, &ids(&[&one, &two]))
        .await
        .expect("two boards are filed under the category to delete");
    store
        .add_update_category_container(user_id, &kept, std::slice::from_ref(&three))
        .await
        .expect("a board is filed under the category that stays");
    store
        .set_container_visibility(user_id, &doomed, &one, SidebarVisibility::Hidden)
        .await
        .expect("the board is hidden");

    store
        .delete_category(&doomed, user_id, &sidebar.workspace_id)
        .await
        .expect("the category is deleted");

    let error = store
        .get_category(&doomed)
        .await
        .expect_err("a deleted category is not found");
    assert!(matches!(error, StoreError::NotFound(_)), "got {error:?}");
    let listed = sidebar.category_boards(store).await;
    assert_eq!(
        listed.len(),
        1,
        "only the category that stays is listed: {listed:?}"
    );
    assert_eq!(listed[0].category.id, kept, "the category that stays");
    assert_eq!(
        listed[0].container_metadata,
        [visible(&three)],
        "its board, and none of the deleted category's"
    );
    for board_id in [&one, &two] {
        store
            .get_container(board_id)
            .await
            .expect("a board outlives the category it was filed under");
    }

    // A board that moves between categories keeps its hidden flag. This one comes back
    // visible: it was filed under nothing, and is filed as new.
    store
        .add_update_category_container(user_id, &kept, std::slice::from_ref(&one))
        .await
        .expect("a board of the deleted category is filed again");
    let mut boards = sidebar.boards_of(store, &kept).await;
    boards.sort_by(|left, right| left.container_id.cmp(&right.container_id));
    let mut expected = [visible(&one), visible(&three)];
    expected.sort_by(|left, right| left.container_id.cmp(&right.container_id));
    assert_eq!(boards, expected, "the board is filed as new, visible");

    test.teardown().await;
}

/// Deleting a category that is another user's, or in another workspace, or that does not
/// exist succeeds and changes nothing: the category stays, with its boards filed as they
/// were.
#[tokio::test]
async fn deleting_a_category_of_another_user_or_workspace_changes_nothing() {
    let test = common::setup().await;
    let store = &test.store;
    let sidebar = Sidebar::create(store).await;
    let user_id = &sidebar.user_id;
    let someone_else = common::user(store, "someone-else").await;
    let another_workspace = common::workspace(store).await;
    let category_id = sidebar.create_category(store, "Category").await;
    let [one, two] = create_boards(store, &sidebar.workspace_id, user_id).await;

    store
        .add_update_category_container(user_id, &category_id, &ids(&[&one, &two]))
        .await
        .expect("the boards are filed");
    // Sort orders of their own, so that the listing has one order.
    store
        .reorder_category_containers(&category_id, ids(&[&two, &one]))
        .await
        .expect("the boards are ordered");
    store
        .set_container_visibility(user_id, &category_id, &two, SidebarVisibility::Hidden)
        .await
        .expect("a board is hidden");
    let before = sidebar.category_boards(store).await;
    assert_eq!(
        before[0].container_metadata,
        [
            CategoryContainerMetadata {
                container_id: two.clone(),
                hidden: true,
            },
            visible(&one),
        ],
        "the boards as they were filed"
    );

    for (id, deleter, workspace, what) in [
        (
            &category_id,
            &someone_else,
            &sidebar.workspace_id,
            "another user",
        ),
        (
            &category_id,
            user_id,
            &another_workspace,
            "another workspace",
        ),
        (
            &new_id(),
            user_id,
            &sidebar.workspace_id,
            "no such category",
        ),
    ] {
        store
            .delete_category(id, deleter, workspace)
            .await
            .expect("deleting nothing succeeds");
        assert_eq!(
            sidebar.category_boards(store).await,
            before,
            "{what}: the category and its boards are as they were"
        );
    }

    test.teardown().await;
}

/// A filing requires an existing user, board, and category: if any of the three is
/// missing, nothing in the call is stored.
#[tokio::test]
async fn a_filing_needs_its_user_its_category_and_its_board() {
    let test = common::setup().await;
    let store = &test.store;
    let sidebar = Sidebar::create(store).await;
    let user_id = &sidebar.user_id;
    let category_id = sidebar.create_category(store, "Category").await;
    let [filed, board] = create_boards(store, &sidebar.workspace_id, user_id).await;
    store
        .add_update_category_container(user_id, &category_id, std::slice::from_ref(&filed))
        .await
        .expect("a board of the user is filed under the category");

    let [nobody, no_category, no_board] = [new_id(), new_id(), new_id()];
    for (user, category, boards, what) in [
        (&nobody, &category_id, ids(&[&board]), "no such user"),
        (user_id, &no_category, ids(&[&board]), "no such category"),
        (
            user_id,
            &category_id,
            ids(&[&board, &no_board]),
            "no such board",
        ),
    ] {
        let error = store
            .add_update_category_container(user, category, &boards)
            .await
            .expect_err("a filing that refers to nothing is refused");
        assert!(
            matches!(error, StoreError::NotFound(_)),
            "{what}: got {error:?}"
        );
        assert_eq!(
            sidebar.boards_of(store, &category_id).await,
            [visible(&filed)],
            "{what}: nothing of the refused call is filed"
        );
    }

    test.teardown().await;
}
