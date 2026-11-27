//! The category routes: what each takes and refuses, and the exact answers.

use super::*;

// ---------------------------------------------------------------------------------------
// Category route validation and exact HTTP responses

/// Each route of the categories refuses a request without a session token (401).
#[tokio::test]
async fn category_routes_need_a_session() {
    let th = Setup::native().start().await.init_basic().await;
    let anonymous = th.anonymous();
    let requests = [
        (Method::GET, &format!("{ROOT_WS}/categories")),
        (Method::POST, &format!("{ROOT_WS}/categories")),
        (Method::PUT, &format!("{ROOT_WS}/category-order")),
        (Method::PUT, &format!("{ROOT_WS}/categories/any")),
        (Method::DELETE, &format!("{ROOT_WS}/categories/any")),
        (
            Method::PUT,
            &format!("{ROOT_WS}/categories/any/container-order"),
        ),
        (
            Method::POST,
            &format!("{ROOT_WS}/categories/any/containers/any"),
        ),
        (
            Method::PUT,
            &format!("{ROOT_WS}/categories/any/containers/any/hide"),
        ),
        (
            Method::PUT,
            &format!("{ROOT_WS}/categories/any/containers/any/unhide"),
        ),
    ];

    for (method, path) in requests {
        let response = anonymous.send_raw_body(method.clone(), path, "{}").await;
        check_error(&response, StatusCode::UNAUTHORIZED, "no session token");
    }

    th.teardown().await;
}

/// The category routes are about a workspace that exists. A path that names no workspace answers
/// that, whatever else it names, and alike for an identifier of nothing and for text
/// that is no identifier. (While the server had one workspace and took any name for it, these
/// requests were answered for their category.)
#[tokio::test]
async fn category_routes_need_a_workspace_that_exists() {
    let th = Setup::native().start().await.init_basic().await;
    let user = th.client.me().await;
    category_boards(&th.client, ROOT_WORKSPACE_ID).await;
    let created =
        create_category(&th.client, &category(ROOT_WORKSPACE_ID, &user.id, "Cat A")).await;
    let board = board_with_admin(&th, ROOT_WORKSPACE_ID, &user.id).await;
    file_board(&th.client, ROOT_WORKSPACE_ID, &created.id, &board.id).await;
    let before = category_boards(&th.client, ROOT_WORKSPACE_ID).await;

    for workspace_id in [UNKNOWN_ID, "1"] {
        let categories = categories_path(workspace_id);
        let one = format!("{categories}/{}", created.id);
        let place = format!("{one}/containers/{}", board.id);
        let order = format!("{one}/container-order");
        let new = json!({"name": "New", "userId": user.id, "workspaceId": workspace_id});
        let change = json!({
            "id": created.id, "name": "Renamed", "userId": user.id, "workspaceId": workspace_id,
        });
        let requests = [
            (Method::GET, categories.clone(), Value::Null),
            (Method::POST, categories.clone(), new),
            (
                Method::PUT,
                format!("/w/{workspace_id}/category-order"),
                json!(ids(&before)),
            ),
            (Method::PUT, one.clone(), change),
            (Method::DELETE, one.clone(), Value::Null),
            (Method::PUT, order, json!([board.id])),
            (Method::POST, place.clone(), Value::Null),
            (Method::PUT, format!("{place}/hide"), Value::Null),
            (Method::PUT, format!("{place}/unhide"), Value::Null),
        ];
        for (method, path, body) in requests {
            let response = th
                .client
                .send_raw_body(method, &path, &body.to_string())
                .await;
            check_error(&response, StatusCode::NOT_FOUND, "workspace not found");
        }
    }

    let after = category_boards(&th.client, ROOT_WORKSPACE_ID).await;
    assert_eq!(after, before, "none of these changed anything");

    th.teardown().await;
}

/// The default category is created by the first listing, once per workspace, and is the same
/// from then on.
#[tokio::test]
async fn the_default_category_is_created_once_per_workspace() {
    let th = Setup::native().start().await.init_basic().await;

    let first = category_boards(&th.client, ROOT_WORKSPACE_ID).await;
    let second = category_boards(&th.client, ROOT_WORKSPACE_ID).await;
    let other_workspace = category_boards(&th.client, TEST_WORKSPACE_ID).await;
    let other_user = category_boards(&th.client2, ROOT_WORKSPACE_ID).await;

    assert_eq!(names(&first), ["Boards"], "created by the first listing");
    assert_eq!(second, first, "and not again by the second");
    assert_eq!(
        names(&other_workspace),
        ["Boards"],
        "each workspace has its own"
    );
    assert_ne!(
        ids(&other_workspace),
        ids(&first),
        "with its own identifier"
    );
    assert_eq!(
        other_workspace[0].category.workspace_id, TEST_WORKSPACE_ID,
        "in that workspace"
    );
    assert_eq!(names(&other_user), ["Boards"], "each user has their own");
    assert_ne!(ids(&other_user), ids(&first), "with its own identifier");

    th.teardown().await;
}

/// The default category is recognised by `CategoryType::System`, so a custom category
/// named "Boards" does not suppress creating the system default category.
#[tokio::test]
async fn a_custom_category_named_boards_stands_in_for_the_default_one() {
    let th = Setup::native().start().await.init_basic().await;
    let user = th.client.me().await;
    let custom =
        create_category(&th.client, &category(ROOT_WORKSPACE_ID, &user.id, "Boards")).await;

    let listed = category_boards(&th.client, ROOT_WORKSPACE_ID).await;

    assert_eq!(listed.len(), 2, "custom and system default coexist");
    assert_eq!(listed[0].category.id, custom.id);
    assert_eq!(listed[0].category.category_type, CategoryType::Custom);
    assert_eq!(listed[1].category.category_type, CategoryType::System);

    th.teardown().await;
}

/// What creating a category answers, member by member.
#[tokio::test]
async fn create_category_answers_the_stored_category() {
    let th = Setup::native().start().await.init_basic().await;
    let user = th.client.me().await;

    let response = th
        .client
        .post(
            &format!("{ROOT_WS}/categories"),
            &json!({"name": "Cat A", "userId": user.id, "workspaceId": ROOT_WORKSPACE_ID}),
        )
        .await;
    let created: Category = response.created_json();
    assert_eq!(
        response.value(),
        json!({
            "id": created.id,
            "name": "Cat A",
            "userId": user.id,
            "workspaceId": ROOT_WORKSPACE_ID,
            "createdAt": created.created_at,
            "updatedAt": created.created_at,
            "collapsed": false,
            // Preserved quirk: a category created at the top is stored one place down.
            "sortOrder": 10,
            "type": "custom",
        }),
        "the created category"
    );
    assert!(is_id(&created.id), "a generated identifier: {}", created.id);
    assert!(created.created_at > 0, "a creation time");

    // What the client may choose: the times, the collapsed state and the type. A
    // negative sort order counts as zero and a `sorting` member is no member of a
    // category. The identifier is the server's to make.
    let response = th
        .client
        .post(
            &format!("{ROOT_WS}/categories"),
            &json!({
                "name": "With id",
                "userId": user.id,
                "workspaceId": ROOT_WORKSPACE_ID,
                "sortOrder": -5,
                "createdAt": 5,
                "sorting": "alpha",
                "collapsed": true,
                "type": "system",
            }),
        )
        .await;
    let answered = response.created_json::<Value>();
    let made_id = answered["id"].as_str().expect("an identifier").to_owned();
    assert!(is_id(&made_id), "a generated identifier: {made_id}");
    assert_eq!(
        answered,
        json!({
            "id": made_id,
            "name": "With id",
            "userId": user.id,
            "workspaceId": ROOT_WORKSPACE_ID,
            "createdAt": 5,
            "updatedAt": 5,
            "collapsed": true,
            "sortOrder": 10,
            "type": "system",
        }),
        "the created category"
    );

    // A blank type is no type, and not the default; `null` is none (`request_forms`).
    for blank in [json!("  "), json!("")] {
        let response = th
            .client
            .post(
                &format!("{ROOT_WS}/categories"),
                &json!({"name": "Blank type", "userId": user.id, "workspaceId": ROOT_WORKSPACE_ID, "type": blank}),
            )
            .await;
        response.check_bad_request();
        assert_eq!(
            response.refused_at(),
            "/type",
            "type {blank} is refused, not read as custom"
        );
    }

    th.teardown().await;
}

/// The refusals of creating a category.
#[tokio::test]
async fn create_category_refuses_invalid_requests() {
    let th = Setup::native().start().await.init_basic().await;
    let user = th.client.me().await;
    let other = th.client2.me().await;
    let path = &format!("{ROOT_WS}/categories");
    let invalid_type = r#"type must be "system" or "custom""#;
    // A category is made for the session's user and nobody else, whatever else the
    // body says.
    for body in [
        json!({"name": "x", "userId": other.id, "workspaceId": ROOT_WORKSPACE_ID}),
        json!({"name": "x", "workspaceId": ROOT_WORKSPACE_ID}),
        json!({"name": "x", "userId": other.id, "workspaceId": ROOT_WORKSPACE_ID, "type": "bogus"}),
        // Wrong-case field names are ignored as unknown, leaving `userId` absent.
        json!({"name": "x", "userID": user.id, "workspaceID": ROOT_WORKSPACE_ID}),
    ] {
        let response = th.client.post(path, &body).await;
        check_error(
            &response,
            StatusCode::FORBIDDEN,
            "a user makes categories for themselves only",
        );
        assert_eq!(response.refused_at(), "/userId", "{body}");
    }

    // Another workspace than the path's: one that exists, one that does not, and text that
    // is no identifier.
    for workspace_id in [TEST_WORKSPACE_ID, UNKNOWN_ID, "1"] {
        let body = json!({"name": "x", "userId": user.id, "workspaceId": workspace_id});
        let response = th.client.post(path, &body).await;
        check_error(&response, StatusCode::BAD_REQUEST, "workspaceId mismatch");
    }

    let cases = [
        (
            "a blank name",
            json!({"name": "  ", "userId": user.id, "workspaceId": ROOT_WORKSPACE_ID}),
            "the category has no name".to_owned(),
        ),
        (
            "a `null` name, no name and no string",
            json!({"name": null, "userId": user.id, "workspaceId": ROOT_WORKSPACE_ID}),
            "invalid type: null, expected a string".to_owned(),
        ),
        (
            "a blank type, which is no type and not the default",
            json!({"name": "x", "userId": user.id, "workspaceId": ROOT_WORKSPACE_ID, "type": " "}),
            invalid_type.to_owned(),
        ),
        (
            "a blank name and an unknown type: both are refused",
            json!({"name": "", "userId": user.id, "workspaceId": ROOT_WORKSPACE_ID, "type": "bogus"}),
            "2 members of the request were refused".to_owned(),
        ),
    ];
    for (case, body, message) in cases {
        let response = th.client.post(path, &body).await;
        response.check_bad_request();
        assert_eq!(response.error_message(), message, "{case}");
    }

    // A body that is no JSON object with members of the right types is a bad request.
    for malformed in [
        "{not json",
        "",
        "[]",
        "null",
        r#"{"name": 5}"#,
        r#"{"type": 5}"#,
    ] {
        let response = th.client.send_raw_body(Method::POST, path, malformed).await;
        response.check_bad_request();
        assert_eq!(
            response.problem_type(),
            "/problems/bad-request",
            "{malformed}"
        );
    }

    // The server makes the identifier. A request that names one is refused whether or
    // not somebody has a category of that identifier: the answer must not tell.
    let others = create_category(
        &th.client2,
        &category(ROOT_WORKSPACE_ID, &other.id, "Theirs"),
    )
    .await;
    for id in ["fresh", UNKNOWN_ID, others.id.as_str()] {
        let named =
            json!({"id": id, "name": "x", "userId": user.id, "workspaceId": ROOT_WORKSPACE_ID});
        let response = th.client.post(path, &named).await;
        check_error(
            &response,
            StatusCode::BAD_REQUEST,
            "a new category cannot have an ID",
        );
        assert_eq!(response.refused_at(), "/id", "the member at fault");
    }
    th.client
        .post(
            path,
            &json!({"name": "x", "userId": user.id, "workspaceId": ROOT_WORKSPACE_ID}),
        )
        .await
        .check_created();

    let listed = category_boards(&th.client, ROOT_WORKSPACE_ID).await;
    assert_eq!(
        names(&listed),
        ["x", "Boards"],
        "only the valid one was stored"
    );

    th.teardown().await;
}

/// What updating a category answers and changes.
#[tokio::test]
async fn update_category_changes_the_name_and_the_collapsed_state_only() {
    let th = Setup::native().start().await.init_basic().await;
    let user = th.client.me().await;
    let created =
        create_category(&th.client, &category(ROOT_WORKSPACE_ID, &user.id, "Cat A")).await;

    let response = th
        .client
        .put(
            &format!("{ROOT_WS}/categories/{}", created.id),
            &json!({
                "id": created.id,
                "name": "Cat A2",
                "userId": user.id,
                "workspaceId": ROOT_WORKSPACE_ID,
                "collapsed": true,
                "sortOrder": 99,
                "type": "system",
                "deletedAt": 77,
                "createdAt": 3,
            }),
        )
        .await;

    let updated: Category = response.ok_json();
    assert_eq!(
        response.value(),
        json!({
            "id": created.id,
            "name": "Cat A2",
            "userId": user.id,
            "workspaceId": ROOT_WORKSPACE_ID,
            "createdAt": created.created_at,
            "updatedAt": updated.updated_at,
            "collapsed": true,
            "sortOrder": created.sort_order,
            "type": "custom",
        }),
        "the updated category"
    );
    assert!(
        updated.updated_at >= created.updated_at,
        "the update time moved"
    );

    th.teardown().await;
}

/// The refusals of updating a category.
#[tokio::test]
async fn update_category_refuses_invalid_requests() {
    let th = Setup::native().start().await.init_basic().await;
    let user = th.client.me().await;
    let other = th.client2.me().await;
    let created =
        create_category(&th.client, &category(ROOT_WORKSPACE_ID, &user.id, "Cat A")).await;
    let path = format!("{ROOT_WS}/categories/{}", created.id);
    let body = |change: Value| {
        let mut body = json!({"id": created.id, "name": "n", "userId": user.id, "workspaceId": ROOT_WORKSPACE_ID});
        for (name, value) in change.as_object().expect("an object") {
            body[name] = value.clone();
        }
        body
    };

    let bad_requests = [
        (
            body(json!({"id": "other"})),
            "the category of the body is not the one of the path",
        ),
        (
            body(json!({"id": UNKNOWN_ID})),
            "the category of the body is not the one of the path",
        ),
        (
            body(json!({"userId": other.id})),
            "the user of the body is not the session's",
        ),
        (
            body(json!({"workspaceId": TEST_WORKSPACE_ID})),
            "workspaceId mismatch",
        ),
        (
            body(json!({"workspaceId": UNKNOWN_ID})),
            "workspaceId mismatch",
        ),
        (body(json!({"workspaceId": "1"})), "workspaceId mismatch"),
        (body(json!({"name": ""})), "the category has no name"),
        (
            body(json!({"type": "bogus"})),
            r#"type must be "system" or "custom""#,
        ),
    ];
    for (request, message) in bad_requests {
        let response = th.client.put(&path, &request).await;
        check_error(&response, StatusCode::BAD_REQUEST, message);
    }

    // A category of another workspace than the path's, one that does not exist, and
    // another user's all answer alike, whatever the body.
    let others = create_category(
        &th.client2,
        &category(ROOT_WORKSPACE_ID, &other.id, "Theirs"),
    )
    .await;
    for (route, request) in [
        (
            format!("{TEST_WS}/categories/{}", created.id),
            body(json!({"workspaceId": TEST_WORKSPACE_ID})),
        ),
        (
            format!("{ROOT_WS}/categories/{UNKNOWN_ID}"),
            json!({"id": UNKNOWN_ID, "name": "n", "userId": user.id, "workspaceId": ROOT_WORKSPACE_ID}),
        ),
        (
            format!("{ROOT_WS}/categories/nope"),
            json!({"id": "nope", "name": "n", "userId": user.id, "workspaceId": ROOT_WORKSPACE_ID}),
        ),
        (
            format!("{ROOT_WS}/categories/{}", others.id),
            json!({"id": others.id, "name": "n", "userId": user.id, "workspaceId": ROOT_WORKSPACE_ID}),
        ),
    ] {
        let response = th.client.put(&route, &request).await;
        check_error(&response, StatusCode::NOT_FOUND, "category not found");
        let response = th.client.send_raw_body(Method::PUT, &route, "{nope").await;
        check_error(&response, StatusCode::NOT_FOUND, "category not found");
    }

    // A body that cannot be decoded is a bad request.
    th.client
        .send_raw_body(Method::PUT, &path, "{nope")
        .await
        .check_bad_request();

    let stored = th.app.category(&created.id).await.expect("it is stored");
    assert_eq!(stored, created, "none of these changed the category");

    // A deleted category cannot be updated.
    th.client.delete(&path).await.check_no_content();
    let response = th.client.put(&path, &body(json!({}))).await;
    check_error(&response, StatusCode::NOT_FOUND, "category not found");

    th.teardown().await;
}

/// What deleting a category answers, and its refusals.
#[tokio::test]
async fn delete_category_over_the_api() {
    let th = Setup::native().start().await.init_basic().await;
    let user = th.client.me().await;
    let default_id = category_boards(&th.client, ROOT_WORKSPACE_ID).await[0]
        .category
        .id
        .clone();
    let created =
        create_category(&th.client, &category(ROOT_WORKSPACE_ID, &user.id, "Cat A")).await;
    let path = format!("{ROOT_WS}/categories/{}", created.id);

    for missing in [UNKNOWN_ID, "nope"] {
        let response = th
            .client
            .delete(&format!("{ROOT_WS}/categories/{missing}"))
            .await;
        check_error(&response, StatusCode::NOT_FOUND, "category not found");
    }

    // In another workspace than its own the category does not exist.
    let response = th
        .client
        .delete(&format!("{TEST_WS}/categories/{}", created.id))
        .await;
    check_error(&response, StatusCode::NOT_FOUND, "category not found");

    let response = th
        .client
        .delete(&format!("{ROOT_WS}/categories/{default_id}"))
        .await;
    check_error(
        &response,
        StatusCode::FORBIDDEN,
        "a system category cannot be deleted",
    );

    let listed = category_boards(&th.client, ROOT_WORKSPACE_ID).await;
    assert_eq!(names(&listed), ["Cat A", "Boards"], "nothing was deleted");

    // A deletion has no result to answer with.
    let response = th.client.delete(&path).await;
    response.check_no_content();
    let error = th
        .app
        .category(&created.id)
        .await
        .expect_err("the category is gone from the store");
    assert!(error.is_not_found(), "not found: {error:?}");

    // It is gone: there is nothing to delete a second time.
    let response = th.client.delete(&path).await;
    check_error(&response, StatusCode::NOT_FOUND, "category not found");

    let listed = category_boards(&th.client, ROOT_WORKSPACE_ID).await;
    assert_eq!(names(&listed), ["Boards"], "the category is gone");

    th.teardown().await;
}

/// A reorder request must name the user's categories and nothing else.
#[tokio::test]
async fn reorder_categories_over_the_api() {
    let th = Setup::native().start().await.init_basic().await;
    let user = th.client.me().await;
    let path = &format!("{ROOT_WS}/category-order");

    // With no categories, the empty order is accepted and answered with the empty
    // list, never `null`. The user has none before the first listing creates the
    // default one.
    let response = th.client.send_raw_body(Method::PUT, path, "[]").await;
    response.check_ok();
    assert_eq!(response.value(), json!([]), "the answer to the empty order");
    th.client
        .send_raw_body(Method::PUT, path, "null")
        .await
        .check_bad_request();

    let default_id = category_boards(&th.client, ROOT_WORKSPACE_ID).await[0]
        .category
        .id
        .clone();
    let second = create_category(
        &th.client,
        &category(ROOT_WORKSPACE_ID, &user.id, "Category 2"),
    )
    .await;
    let third = create_category(
        &th.client,
        &category(ROOT_WORKSPACE_ID, &user.id, "Category 3"),
    )
    .await;
    let before = [third.id.as_str(), second.id.as_str(), default_id.as_str()];
    let listed = category_boards(&th.client, ROOT_WORKSPACE_ID).await;
    assert_eq!(ids(&listed), before, "the order to begin with");

    let refused = [
        json!([default_id, second.id]),
        json!([default_id, second.id, "zzz"]),
        json!([default_id, second.id, UNKNOWN_ID]),
        json!([default_id, second.id, third.id, third.id]),
        json!([]),
        Value::Null,
        json!({}),
        json!("reorder"),
    ];
    for body in refused {
        let response = th.client.put(path, &body).await;
        response.check_bad_request();
        assert_eq!(response.problem_type(), "/problems/bad-request", "{body}");
    }
    th.client
        .send_raw_body(Method::PUT, path, "[")
        .await
        .check_bad_request();
    let listed = category_boards(&th.client, ROOT_WORKSPACE_ID).await;
    assert_eq!(ids(&listed), before, "a refused order changes nothing");

    // Another user's order of these categories is refused: they are not theirs.
    category_boards(&th.client2, ROOT_WORKSPACE_ID).await;
    let response = th.client2.put(path, &json!([default_id])).await;
    response.check_bad_request();
    let listed = category_boards(&th.client, ROOT_WORKSPACE_ID).await;
    assert_eq!(ids(&listed), before, "and changes nothing");

    let new_order = [default_id.as_str(), third.id.as_str(), second.id.as_str()];
    let response = th.client.put(path, &new_order).await;
    assert_eq!(response.ok_json::<Vec<String>>(), new_order, "the answer");
    let listed = category_boards(&th.client, ROOT_WORKSPACE_ID).await;
    assert_eq!(ids(&listed), new_order, "the new order");
    let places: Vec<i64> = listed
        .iter()
        .map(|entry| entry.category.sort_order)
        .collect();
    assert_eq!(
        places,
        [10, 20, 30],
        "the listing tells each category's place, from the first gap"
    );

    // Naming one category twice is refused with 400 and changes nothing.
    let twice = [default_id.as_str(), default_id.as_str(), second.id.as_str()];
    th.client.put(path, &twice).await.check_bad_request();
    let listed = category_boards(&th.client, ROOT_WORKSPACE_ID).await;
    assert_eq!(ids(&listed), new_order, "unchanged after duplicate IDs");

    // A new category comes first, also after a reorder.
    let new = create_category(&th.client, &category(ROOT_WORKSPACE_ID, &user.id, "New")).await;
    let listed = category_boards(&th.client, ROOT_WORKSPACE_ID).await;
    assert_eq!(ids(&listed)[0], new.id, "the new category is first");

    th.teardown().await;
}

/// A board reorder request must name the boards of the category and nothing else.
#[tokio::test]
async fn reorder_category_containers_over_the_api() {
    let th = Setup::native().start().await.init_basic().await;
    let user = th.client.me().await;
    let default_id = category_boards(&th.client, ROOT_WORKSPACE_ID).await[0]
        .category
        .id
        .clone();
    let created =
        create_category(&th.client, &category(ROOT_WORKSPACE_ID, &user.id, "Cat A")).await;
    let boards = open_boards(&th, ROOT_WORKSPACE_ID, 3).await;
    for board_id in &boards {
        file_board(&th.client, ROOT_WORKSPACE_ID, &created.id, board_id).await;
    }
    let path = format!("{ROOT_WS}/categories/{}/container-order", created.id);
    let order = async || {
        let listed = category_boards(&th.client, ROOT_WORKSPACE_ID).await;
        board_ids(entry(&listed, &created.id))
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };

    let new_order = [boards[1].as_str(), boards[2].as_str(), boards[0].as_str()];
    let response = th.client.put(&path, &new_order).await;
    assert_eq!(response.ok_json::<Vec<String>>(), new_order, "the answer");
    assert_eq!(order().await, new_order, "the new order");

    let refused = [
        json!([boards[0], boards[1]]),
        json!([boards[0], boards[1], "other"]),
        json!([boards[0], boards[1], UNKNOWN_ID]),
        json!([]),
        Value::Null,
        json!({}),
    ];
    for body in refused {
        let response = th.client.put(&path, &body).await;
        response.check_bad_request();
        assert_eq!(response.problem_type(), "/problems/bad-request", "{body}");
    }
    th.client
        .send_raw_body(Method::PUT, &path, "{")
        .await
        .check_bad_request();
    assert_eq!(order().await, new_order, "a refused order changes nothing");

    // A category of another workspace than the path's, and one that does not exist.
    for route in [
        format!("{TEST_WS}/categories/{}/container-order", created.id),
        format!("{ROOT_WS}/categories/{UNKNOWN_ID}/container-order"),
        format!("{ROOT_WS}/categories/nope/container-order"),
    ] {
        let response = th.client.put(&route, &new_order).await;
        check_error(&response, StatusCode::NOT_FOUND, "category not found");
    }
    assert_eq!(order().await, new_order, "a refused order changes nothing");

    // The empty order of an empty category is answered with the empty list, not `null`.
    let response = th
        .client
        .send_raw_body(
            Method::PUT,
            &format!("{ROOT_WS}/categories/{default_id}/container-order"),
            "[]",
        )
        .await;
    response.check_ok();
    assert_eq!(response.value(), json!([]), "the answer to the empty order");

    // A deleted category has no boards to order.
    th.client
        .delete(&format!("{ROOT_WS}/categories/{}", created.id))
        .await
        .check_no_content();
    let response = th.client.put(&path, &new_order).await;
    check_error(&response, StatusCode::NOT_FOUND, "category not found");

    th.teardown().await;
}

/// Filing a board under a category: the answer, the move, and the refusals.
#[tokio::test]
async fn update_category_container_over_the_api() {
    let th = Setup::native().start().await.init_basic().await;
    let user = th.me_as_no_admin(&th.client).await;
    let board = board_with_admin(&th, ROOT_WORKSPACE_ID, &user.id).await;
    let default_id = category_boards(&th.client, ROOT_WORKSPACE_ID).await[0]
        .category
        .id
        .clone();
    let created =
        create_category(&th.client, &category(ROOT_WORKSPACE_ID, &user.id, "Cat A")).await;
    let board_path = |workspace_id: &str, category_id: &str| {
        format!(
            "/w/{workspace_id}/categories/{category_id}/containers/{}",
            board.id
        )
    };
    let places = async || {
        let listed = category_boards(&th.client, ROOT_WORKSPACE_ID).await;
        (
            board_ids(entry(&listed, &created.id))
                .into_iter()
                .map(str::to_owned)
                .collect::<Vec<_>>(),
            board_ids(entry(&listed, &default_id))
                .into_iter()
                .map(str::to_owned)
                .collect::<Vec<_>>(),
        )
    };
    assert_eq!(
        places().await,
        (vec![], vec![board.id.clone()]),
        "the board starts in the default category"
    );

    let response = th
        .client
        .post_empty(&board_path(ROOT_WORKSPACE_ID, &created.id))
        .await;
    response.check_no_content();
    assert_eq!(
        places().await,
        (vec![board.id.clone()], vec![]),
        "the board moved"
    );

    // A nonexistent, cross-workspace, or deleted category is refused without moving the board.
    let deleted = create_category(
        &th.client,
        &category(ROOT_WORKSPACE_ID, &user.id, "Deleted"),
    )
    .await;
    th.client
        .delete(&format!("{ROOT_WS}/categories/{}", deleted.id))
        .await
        .check_no_content();
    for path in [
        board_path(ROOT_WORKSPACE_ID, UNKNOWN_ID),
        board_path(ROOT_WORKSPACE_ID, "nope"),
        board_path(TEST_WORKSPACE_ID, &default_id),
        board_path(ROOT_WORKSPACE_ID, &deleted.id),
    ] {
        let response = th.client.post_empty(&path).await;
        check_error(&response, StatusCode::NOT_FOUND, "category not found");
        assert_eq!(
            places().await,
            (vec![board.id.clone()], vec![]),
            "the board stays after {path}"
        );
    }

    // Only a board the user can see may be filed in a category.
    let private = board_of_somebody_else(&th, ROOT_WORKSPACE_ID).await;
    for board_id in [UNKNOWN_ID, "no-such-board", private.as_str()] {
        let response = th
            .client
            .post_empty(&format!(
                "{ROOT_WS}/categories/{default_id}/containers/{board_id}"
            ))
            .await;
        check_error(&response, StatusCode::NOT_FOUND, "board not found");
    }
    assert_eq!(
        places().await,
        (vec![board.id.clone()], vec![]),
        "nothing was filed"
    );

    th.teardown().await;
}

/// Hiding and showing need a category of the user's and a board the user can see. A
/// board that is not filed in the category is answered as a success and changes
/// nothing.
#[tokio::test]
async fn hiding_a_board_that_is_not_in_the_category_changes_nothing() {
    let th = Setup::native().start().await.init_basic().await;
    let user = th.client.me().await;
    let board = board_with_admin(&th, ROOT_WORKSPACE_ID, &user.id).await;
    let before = category_boards(&th.client, ROOT_WORKSPACE_ID).await;
    assert_eq!(
        before[0].container_metadata,
        [visible(&board.id)],
        "one board"
    );
    let default_id = &before[0].category.id;
    let board_id = &board.id;

    // What does not exist is asked for by an identifier of nothing and by text that is
    // no identifier: the answers are the same.
    for nothing in [UNKNOWN_ID, "nope"] {
        for (path, missing) in [
            (
                format!("{ROOT_WS}/categories/{nothing}/containers/{nothing}/hide"),
                "category not found",
            ),
            (
                format!("{ROOT_WS}/categories/{nothing}/containers/{board_id}/hide"),
                "category not found",
            ),
            (
                format!("{ROOT_WS}/categories/{nothing}/containers/{nothing}/unhide"),
                "category not found",
            ),
            (
                format!("{ROOT_WS}/categories/{default_id}/containers/{nothing}/hide"),
                "board not found",
            ),
            (
                format!("{ROOT_WS}/categories/{default_id}/containers/{nothing}/unhide"),
                "board not found",
            ),
        ] {
            let response = put_empty(&th.client, &path).await;
            check_error(&response, StatusCode::NOT_FOUND, missing);
        }
    }

    // A board the user sees, in a category of the user's it is not filed in.
    let other = create_category(&th.client, &category(ROOT_WORKSPACE_ID, &user.id, "Other")).await;
    let before = category_boards(&th.client, ROOT_WORKSPACE_ID).await;
    let hide = format!(
        "{ROOT_WS}/categories/{}/containers/{board_id}/hide",
        other.id
    );
    let response = put_empty(&th.client, &hide).await;
    response.check_no_content();

    let after = category_boards(&th.client, ROOT_WORKSPACE_ID).await;
    assert_eq!(after, before, "nothing changed");

    th.teardown().await;
}

/// The orders have paths of their own, so no word is reserved where the category
/// routes have an identifier: `reorder` there names a category or a board like any
/// other text, and no category or board has it for an identifier.
#[tokio::test]
async fn the_word_reorder_is_an_identifier_like_any_other() {
    let th = Setup::native().start().await.init_basic().await;
    let user = th.client.me().await;
    category_boards(&th.client, ROOT_WORKSPACE_ID).await;
    let created =
        create_category(&th.client, &category(ROOT_WORKSPACE_ID, &user.id, "Cat A")).await;
    let board = board_with_admin(&th, ROOT_WORKSPACE_ID, &user.id).await;
    file_board(&th.client, ROOT_WORKSPACE_ID, &created.id, &board.id).await;
    let before = category_boards(&th.client, ROOT_WORKSPACE_ID).await;

    // Among the categories: an update and a deletion of the category `reorder`.
    let as_category = format!("{ROOT_WS}/categories/reorder");
    let mut reversed = ids(&before);
    reversed.reverse();
    let response = th.client.put(&as_category, &reversed).await;
    check_error(&response, StatusCode::NOT_FOUND, "category not found");
    let response = th.client.delete(&as_category).await;
    check_error(&response, StatusCode::NOT_FOUND, "category not found");

    // Among the boards of a category: the place of the board `reorder`, which takes
    // `POST` and `PUT` (treating `reorder` as a container ID, never a reorder route).
    let as_board = format!("{ROOT_WS}/categories/{}/containers/reorder", created.id);
    let response = th.client.put(&as_board, &json!({})).await;
    check_error(&response, StatusCode::NOT_FOUND, "board not found");
    let response = th.client.delete(&as_board).await;
    check_error(
        &response,
        StatusCode::METHOD_NOT_ALLOWED,
        "method not allowed",
    );
    let response = th.client.post_empty(&as_board).await;
    check_error(&response, StatusCode::NOT_FOUND, "board not found");

    let after = category_boards(&th.client, ROOT_WORKSPACE_ID).await;
    assert_eq!(after, before, "none of these changed anything");

    // The paths of the orders take `PUT` and nothing else.
    let response = th.client.delete(&format!("{ROOT_WS}/category-order")).await;
    check_error(
        &response,
        StatusCode::METHOD_NOT_ALLOWED,
        "method not allowed",
    );
    assert_eq!(response.header("allow"), "PUT", "what the path takes");

    let order = format!("{ROOT_WS}/categories/{}/container-order", created.id);
    let response = th.client.post_empty(&order).await;
    check_error(
        &response,
        StatusCode::METHOD_NOT_ALLOWED,
        "method not allowed",
    );
    assert_eq!(response.header("allow"), "PUT", "what the path takes");

    th.teardown().await;
}
