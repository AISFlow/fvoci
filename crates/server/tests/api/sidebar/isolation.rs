//! The categories of one user are none of another's.

use super::*;

// ---------------------------------------------------------------------------------------
// Cross-user category isolation

/// The state two users start from: each has listed their categories, and the first has a
/// custom category with one board in it.
struct TwoUsers {
    th: TestServer,
    /// The first user's custom category.
    category: Category,
    /// The board in it.
    board_id: String,
    /// The second user's identifier.
    intruder_id: String,
}

impl TwoUsers {
    async fn start() -> Self {
        let th = Setup::native().start().await.init_basic().await;
        let owner = th.client.me().await;
        let intruder = th.client2.me().await;
        category_boards(&th.client, ROOT_WORKSPACE_ID).await;
        category_boards(&th.client2, ROOT_WORKSPACE_ID).await;
        let board = board_with_admin(&th, ROOT_WORKSPACE_ID, &owner.id).await;
        let category = create_category(
            &th.client,
            &category(ROOT_WORKSPACE_ID, &owner.id, "Private"),
        )
        .await;
        file_board(&th.client, ROOT_WORKSPACE_ID, &category.id, &board.id).await;
        Self {
            th,
            category,
            board_id: board.id,
            intruder_id: intruder.id,
        }
    }

    /// What the owner sees of their categories.
    async fn owner_view(&self) -> Vec<CategoryContainers> {
        category_boards(&self.th.client, ROOT_WORKSPACE_ID).await
    }
}

/// Filing a board into a category owned by another user is refused before anything is written,
/// so the board never appears in the owner's category.
#[tokio::test]
async fn filing_a_board_into_another_users_category_is_refused() {
    let two = TwoUsers::start().await;
    let before = two.owner_view().await;
    let intruder_before = category_boards(&two.th.client2, ROOT_WORKSPACE_ID).await;

    // Another user's category answers as one that does not exist.
    for category_id in [two.category.id.as_str(), UNKNOWN_ID, "nope"] {
        for board_id in [UNKNOWN_ID, "planted", two.board_id.as_str()] {
            let response = two
                .th
                .client2
                .post_empty(&format!(
                    "{ROOT_WS}/categories/{category_id}/containers/{board_id}"
                ))
                .await;
            check_error(&response, StatusCode::NOT_FOUND, "category not found");
        }
    }

    assert_eq!(
        two.owner_view().await,
        before,
        "the owner's categories are untouched"
    );
    assert_eq!(
        category_boards(&two.th.client2, ROOT_WORKSPACE_ID).await,
        intruder_before,
        "and nothing was filed for the intruder either"
    );

    two.th.teardown().await;
}

/// The legitimate counterpart of [`filing_a_board_into_another_users_category_is_refused`]:
/// each user files boards under their own categories, and neither sees the other's.
#[tokio::test]
async fn filing_a_board_into_an_own_category_works() {
    let two = TwoUsers::start().await;
    let own = create_category(
        &two.th.client2,
        &category(ROOT_WORKSPACE_ID, &two.intruder_id, "Mine"),
    )
    .await;

    // The same board identifier, filed by each user under a category of their own.
    file_board(&two.th.client2, ROOT_WORKSPACE_ID, &own.id, &two.board_id).await;

    let listed = category_boards(&two.th.client2, ROOT_WORKSPACE_ID).await;
    assert_eq!(
        board_ids(entry(&listed, &own.id)),
        [two.board_id.as_str()],
        "filed for the second user"
    );
    let owner_view = two.owner_view().await;
    assert_eq!(
        board_ids(entry(&owner_view, &two.category.id)),
        [two.board_id.as_str()],
        "and still filed for the first, once"
    );

    two.th.teardown().await;
}

/// Deleting an already-deleted category returns 404 for every caller rather than leaking it.
#[tokio::test]
async fn another_users_deleted_category_is_not_handed_out() {
    let two = TwoUsers::start().await;
    let path = format!("{ROOT_WS}/categories/{}", two.category.id);
    two.th.client.delete(&path).await.check_no_content();

    for client in [&two.th.client2, &two.th.client] {
        let response = client.delete(&path).await;
        check_error(&response, StatusCode::NOT_FOUND, "category not found");
    }

    two.th.teardown().await;
}

/// For a user who does not own it, a category does not exist: every route that names it
/// answers as for an identifier nobody has.
#[tokio::test]
async fn another_users_category_cannot_be_changed() {
    let two = TwoUsers::start().await;
    let before = two.owner_view().await;
    let intruder = &two.th.client2;
    let path = format!("{ROOT_WS}/categories/{}", two.category.id);

    // Update, whoever the body says the category belongs to.
    let as_owner = Category {
        name: "Taken over".to_owned(),
        ..two.category.clone()
    };
    let as_intruder = Category {
        user_id: two.intruder_id.clone(),
        ..as_owner.clone()
    };
    for body in [&as_owner, &as_intruder] {
        let response = intruder.put(&path, body).await;
        check_error(&response, StatusCode::NOT_FOUND, "category not found");
    }

    // Delete.
    let response = intruder.delete(&path).await;
    check_error(&response, StatusCode::NOT_FOUND, "category not found");

    // Reorder the boards in it.
    let response = intruder
        .put(&format!("{path}/container-order"), &[two.board_id.as_str()])
        .await;
    check_error(&response, StatusCode::NOT_FOUND, "category not found");

    // Reorder the categories: they are not among the intruder's.
    let order: Vec<&str> = ids(&before);
    let response = intruder
        .put(&format!("{ROOT_WS}/category-order"), &order)
        .await;
    response.check_bad_request();

    // The same answers as for a category nobody has.
    for nobodys in [UNKNOWN_ID, "nope"] {
        let nobody = &format!("{ROOT_WS}/categories/{nobodys}");
        let response = intruder.put(nobody, &as_intruder).await;
        check_error(&response, StatusCode::NOT_FOUND, "category not found");
        let response = intruder.delete(nobody).await;
        check_error(&response, StatusCode::NOT_FOUND, "category not found");
    }

    // Hide and show.
    let place = format!("{path}/containers/{}", two.board_id);
    let response = put_empty(intruder, &format!("{place}/hide")).await;
    check_error(&response, StatusCode::NOT_FOUND, "category not found");
    assert_eq!(
        two.owner_view().await,
        before,
        "hiding by another user changes nothing"
    );
    put_empty(&two.th.client, &format!("{place}/hide"))
        .await
        .check_no_content();
    let hidden_by_owner = two.owner_view().await;
    assert_ne!(hidden_by_owner, before, "the owner hides the board");
    let response = put_empty(intruder, &format!("{place}/unhide")).await;
    check_error(&response, StatusCode::NOT_FOUND, "category not found");
    assert_eq!(
        two.owner_view().await,
        hidden_by_owner,
        "showing by another user changes nothing"
    );
    put_empty(&two.th.client, &format!("{place}/unhide"))
        .await
        .check_no_content();

    assert_eq!(
        two.owner_view().await,
        before,
        "the owner's view is as before"
    );

    two.th.teardown().await;
}
