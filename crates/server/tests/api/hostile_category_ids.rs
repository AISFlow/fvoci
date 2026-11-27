//! Identifiers of categories and boards that are no identifiers, handed to the sidebar's
//! writes: they are data, never SQL or a path.

use crate::common::ROOT_WS;
use crate::common::{ROOT_WORKSPACE_ID, Setup, TestServer};
use crate::sidebar::{
    NOT_AN_IDENTIFIER, board_ids, category, category_boards, check_error, create_category, entry,
    file_board, ids, open_board, open_boards,
};
use fvoci_model::Category;
use fvoci_store::StoreError;
use reqwest::StatusCode;

/// Text as one segment of a path, with every byte that is no letter or digit
/// percent-encoded, so that the server is handed exactly this text.
fn path_segment(text: &str) -> String {
    text.bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

/// Checks that the store refused to write because `text` is no identifier.
#[track_caller]
fn check_not_an_identifier<T: std::fmt::Debug>(written: Result<T, StoreError>, text: &str) {
    match written {
        Err(StoreError::InvalidId(refused)) => {
            assert_eq!(refused, text, "the text that was refused");
        }
        other => panic!("the store refuses to write with {text:?}, but answered {other:?}"),
    }
}

/// The stored categories of a user in the root workspace, places included.
async fn stored_categories(th: &TestServer, user_id: &str) -> Vec<Category> {
    th.app
        .store()
        .get_user_categories(user_id, ROOT_WORKSPACE_ID)
        .await
        .expect("the categories are read")
}

/// Category reorder endpoints bind identifiers as SQL parameters and reject non-UUID
/// strings so SQL-injection payloads cannot alter the reorder statement.
///
/// Here the server makes the identifiers, and text that is not one reaches a statement
/// as a bound value only: no category can be stored under it, an order that names it is
/// refused, the store does not run a statement that would write with it, and in a path
/// it names nothing. None of that changes a row, the user's or anybody else's.
#[tokio::test]
async fn reorder_categories_treats_a_hostile_identifier_as_data() {
    let th = Setup::native().start().await.init_basic().await;
    let user = th.client.me().await;
    let other = th.client2.me().await;
    let default_id = category_boards(&th.client, ROOT_WORKSPACE_ID).await[0]
        .category
        .id
        .clone();
    let plain = create_category(&th.client, &category(ROOT_WORKSPACE_ID, &user.id, "plain")).await;
    let second =
        create_category(&th.client, &category(ROOT_WORKSPACE_ID, &user.id, "second")).await;
    category_boards(&th.client2, ROOT_WORKSPACE_ID).await;
    create_category(
        &th.client2,
        &category(ROOT_WORKSPACE_ID, &other.id, "theirs"),
    )
    .await;
    let before = stored_categories(&th, &user.id).await;
    let others_before = stored_categories(&th, &other.id).await;
    let path = &format!("{ROOT_WS}/category-order");

    for hostile_id in [
        "a' THEN 777 WHEN 'zz",
        "q'q",
        "x'; DROP TABLE categories; --",
        "zz' OR '1'='1",
    ] {
        // The API generates UUID identifiers; passing non-UUID text directly to the store
        // is also refused.
        let hostile = Category {
            id: hostile_id.to_owned(),
            ..category(ROOT_WORKSPACE_ID, &user.id, "hostile")
        };
        let error = th
            .app
            .create_category(hostile)
            .await
            .expect_err("no category is stored under text that is no identifier");
        assert_eq!(error.status(), StatusCode::BAD_REQUEST, "{error}");
        assert_eq!(error.to_string(), NOT_AN_IDENTIFIER, "for {hostile_id:?}");

        // An order that names it where a category belongs is refused like any other
        // that names something the user does not have.
        let order = [second.id.as_str(), hostile_id, default_id.as_str()];
        let response = th.client.put(path, &order).await;
        response.check_bad_request();
        assert_eq!(
            response.problem_type(),
            "/problems/bad-request",
            "for {hostile_id:?}"
        );

        // Asked past that check, the store does not run the statement.
        let written = th
            .app
            .store()
            .reorder_categories(&user.id, ROOT_WORKSPACE_ID, order.map(str::to_owned).into())
            .await;
        check_not_an_identifier(written, hostile_id);

        // In a path it names no category.
        let named = format!("{ROOT_WS}/categories/{}", path_segment(hostile_id));
        let change = category(ROOT_WORKSPACE_ID, &user.id, "renamed");
        let response = th.client.put(&named, &change).await;
        check_error(&response, StatusCode::NOT_FOUND, "category not found");
        let response = th.client.delete(&named).await;
        check_error(&response, StatusCode::NOT_FOUND, "category not found");
    }

    assert_eq!(
        stored_categories(&th, &user.id).await,
        before,
        "the user's categories are as they were, each in its place"
    );
    assert_eq!(
        stored_categories(&th, &other.id).await,
        others_before,
        "and so are another user's"
    );

    // An order of identifiers is still taken, and is exactly the order asked for.
    let new_order = [plain.id.as_str(), default_id.as_str(), second.id.as_str()];
    let response = th.client.put(path, &new_order).await;
    assert_eq!(response.ok_json::<Vec<String>>(), new_order, "the answer");
    let listed = category_boards(&th.client, ROOT_WORKSPACE_ID).await;
    assert_eq!(ids(&listed), new_order, "exactly the order asked for");
    assert_eq!(
        stored_categories(&th, &other.id).await,
        others_before,
        "which moves nobody else's categories"
    );

    th.teardown().await;
}

/// Category-board reorder endpoints bind container identifiers as SQL parameters and
/// reject non-UUID strings so SQL-injection payloads cannot break or alter the statement.
///
/// Here only a board that exists is filed, and no board has text that is no identifier
/// for its identifier: the request that would file it finds no such board, an order
/// that names it is refused, the store does not run a statement that would write with
/// it, and as the category of a path it names nothing. None of that changes what is
/// filed where, nor in which order.
#[tokio::test]
async fn reorder_category_containers_treats_a_hostile_identifier_as_data() {
    let th = Setup::native().start().await.init_basic().await;
    let user = th.client.me().await;
    category_boards(&th.client, ROOT_WORKSPACE_ID).await;
    let created =
        create_category(&th.client, &category(ROOT_WORKSPACE_ID, &user.id, "Cat A")).await;
    let other = create_category(&th.client, &category(ROOT_WORKSPACE_ID, &user.id, "Cat B")).await;
    let bystander = open_board(&th, ROOT_WORKSPACE_ID).await;
    file_board(&th.client, ROOT_WORKSPACE_ID, &other.id, &bystander).await;
    let boards = open_boards(&th, ROOT_WORKSPACE_ID, 3).await;
    for board_id in &boards {
        file_board(&th.client, ROOT_WORKSPACE_ID, &created.id, board_id).await;
    }
    let category_path = format!("{ROOT_WS}/categories/{}", created.id);
    let path = format!("{category_path}/container-order");
    let new_order = [boards[2].as_str(), boards[0].as_str(), boards[1].as_str()];
    let response = th.client.put(&path, &new_order).await;
    assert_eq!(response.ok_json::<Vec<String>>(), new_order, "the answer");
    let before = category_boards(&th.client, ROOT_WORKSPACE_ID).await;
    assert_eq!(
        board_ids(entry(&before, &created.id)),
        new_order,
        "exactly the order asked for"
    );

    for hostile_id in [
        "q'q",
        "b' THEN 5 WHEN 'c",
        "x' OR category_id <> '",
        "x'; DROP TABLE category_boards; --",
    ] {
        let segment = path_segment(hostile_id);

        // The request that files a board finds no board of that name, and the store,
        // asked past the route, does not file one.
        let response = th
            .client
            .post_empty(&format!("{category_path}/containers/{segment}"))
            .await;
        check_error(&response, StatusCode::NOT_FOUND, "board not found");
        let filed = th
            .app
            .store()
            .add_update_category_container(&user.id, &created.id, &[hostile_id.to_owned()])
            .await;
        check_not_an_identifier(filed, hostile_id);

        // An order that names it where a board belongs is refused like any other that
        // names a board the category does not have; the store does not run the statement.
        let order = [boards[0].as_str(), hostile_id, boards[2].as_str()];
        let response = th.client.put(&path, &order).await;
        response.check_bad_request();
        assert_eq!(
            response.problem_type(),
            "/problems/bad-request",
            "for {hostile_id:?}"
        );
        let written = th
            .app
            .store()
            .reorder_category_containers(&created.id, order.map(str::to_owned).into())
            .await;
        check_not_an_identifier(written, hostile_id);

        // As the category of the path it names nothing.
        let response = th
            .client
            .put(
                &format!("{ROOT_WS}/categories/{segment}/container-order"),
                &new_order,
            )
            .await;
        check_error(&response, StatusCode::NOT_FOUND, "category not found");
    }

    let listed = category_boards(&th.client, ROOT_WORKSPACE_ID).await;
    assert_eq!(
        board_ids(entry(&listed, &created.id)),
        new_order,
        "the boards of the category are in the order they were"
    );
    assert_eq!(
        board_ids(entry(&listed, &other.id)),
        [bystander.as_str()],
        "the other category is untouched"
    );
    assert_eq!(listed, before, "and nothing else changed either");

    th.teardown().await;
}
