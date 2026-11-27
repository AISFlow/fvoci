//! A check of a request's own shape tells of everything it refuses at once: every
//! entry of `errors` with where it is, what is wrong and why.

use fvoci_model::ContainerVisibility;
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::common::TEST_WS;
use crate::common::{ApiResponse, Setup, TEST_WORKSPACE_ID};

/// The entries of `errors` as `(place, reason)`, in their order.
#[track_caller]
fn refused(response: &ApiResponse) -> Vec<(String, String)> {
    let places = response.refused_places();
    let body = response.value();
    let reasons = body["errors"]
        .as_array()
        .map(|errors| {
            errors
                .iter()
                .map(|entry| entry["reason"].as_str().unwrap_or_default().to_owned())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    places.into_iter().zip(reasons).collect()
}

fn pairs(expected: &[(&str, &str)]) -> Vec<(String, String)> {
    expected
        .iter()
        .map(|(place, reason)| ((*place).to_owned(), (*reason).to_owned()))
        .collect()
}

/// A registration whose name, email address, password and organization name are all wrong is
/// told of all four in one answer, and of a fifth with them; nothing is registered.
#[tokio::test]
async fn a_registration_is_told_of_every_member_that_is_wrong_at_once() {
    let th = Setup::native().start().await;
    let anonymous = th.anonymous();

    let response = anonymous
        .post(
            "/register",
            &json!({"username": " ", "email": "not-an-address", "password": "short"}),
        )
        .await;
    response.check_error(
        StatusCode::BAD_REQUEST,
        "4 members of the request were refused",
    );
    assert_eq!(
        response.value()["errors"],
        json!([
            {"pointer": "/username", "detail": "username is required", "reason": "required"},
            {"pointer": "/email", "detail": "invalid email format", "reason": "invalid_format"},
            {
                "pointer": "/password",
                "detail": "password must be at least 8 characters",
                "reason": "too_short",
            },
            {"pointer": "/organizationName", "detail": "organization name is required", "reason": "required"},
        ]),
        "each member with its own words and cause, in the order of the members"
    );

    let response = anonymous
        .post(
            "/register",
            &json!({
                "username": "", "email": "", "password": "", "displayName": "a\u{202e}b",
            }),
        )
        .await;
    response.check_bad_request();
    assert_eq!(
        refused(&response),
        pairs(&[
            ("/username", "required"),
            ("/email", "required"),
            ("/displayName", "invalid_characters"),
            ("/password", "required"),
            ("/organizationName", "required"),
        ])
    );

    // An address that has the form of one and is too long is told as that.
    let response = anonymous
        .post(
            "/register",
            &json!({
                "username": "pam", "email": format!("{}@sample.com", "a".repeat(250)),
                "password": "Pa$$word", "organizationName": "\u{202e}ours",
            }),
        )
        .await;
    response.check_bad_request();
    assert_eq!(
        refused(&response),
        pairs(&[
            ("/email", "too_long"),
            ("/organizationName", "invalid_characters")
        ])
    );
    assert_eq!(
        response.value()["errors"][0]["detail"],
        json!("email must be at most 254 bytes")
    );

    // One member wrong: the answer's detail is that member's.
    let response = anonymous
        .post(
            "/register",
            &json!({
                "username": "pam", "email": "pam@sample.com", "password": "short",
                "organizationName": "Ours",
            }),
        )
        .await;
    response.check_error(
        StatusCode::BAD_REQUEST,
        "password must be at least 8 characters",
    );
    assert_eq!(refused(&response), pairs(&[("/password", "too_short")]));

    // Nothing was registered: the next one is still the first user, the administrator.
    let made: Value = anonymous
        .post(
            "/register",
            &json!({
                "username": "pam", "email": "pam@sample.com", "password": "Pa$$word",
                "organizationName": "Ours",
            }),
        )
        .await
        .created_json();
    assert_eq!(made["isAdmin"], json!(true), "{made}");
    th.teardown().await;
}

/// The other requests about the user: a change of the password, of the account, of the
/// settings and of the client state each list everything they refuse.
#[tokio::test]
async fn the_requests_of_a_user_list_everything_they_refuse() {
    let th = Setup::native().start().await.init_basic().await;

    let response = th
        .client
        .post(
            "/users/me/password",
            &json!({"oldPassword": "", "newPassword": "short"}),
        )
        .await;
    response.check_bad_request();
    assert_eq!(
        refused(&response),
        pairs(&[("/oldPassword", "required"), ("/newPassword", "too_short")])
    );

    let response = th
        .client
        .patch(
            "/users/me",
            &json!({"customName": 7, "username": "other", "email": "x"}),
        )
        .await;
    response.check_bad_request();
    assert_eq!(
        refused(&response),
        pairs(&[
            ("/customName", "invalid"),
            ("/email", "unknown"),
            ("/username", "unknown"),
        ]),
        "in the order of the names"
    );

    // A name of the client's own is not sent back whole in the words: it may be long.
    let long = "n".repeat(300);
    let response = th.client.patch("/users/me", &json!({&long: 1})).await;
    response.check_error(
        StatusCode::BAD_REQUEST,
        &format!("there is no member {:?}", format!("{}…", "n".repeat(64))),
    );
    assert_eq!(
        response.refused_at(),
        format!("/{long}"),
        "the place is exact"
    );

    let before = th.client.get("/users/me/settings").await.value();
    let response = th
        .client
        .patch(
            "/users/me/settings",
            &json!({"theme": "sepia", "locale": "ko", "density": "compact", "haptics": 1}),
        )
        .await;
    response.check_error(
        StatusCode::BAD_REQUEST,
        "3 members of the request were refused",
    );
    assert_eq!(
        refused(&response),
        pairs(&[
            ("/haptics", "invalid"),
            ("/locale", "unknown"),
            ("/theme", "invalid"),
        ])
    );
    assert_eq!(
        th.client.get("/users/me/settings").await.value(),
        before,
        "the member that was fine is not stored either"
    );

    let response = th
        .client
        .patch(
            "/users/me/client-state",
            &json!({"updatedFields": {
                "fine": "x", "no good": "x", "a/b": "x", "long": "x".repeat(4097),
            }}),
        )
        .await;
    response.check_bad_request();
    assert_eq!(
        refused(&response),
        pairs(&[
            ("/updatedFields/a~1b", "invalid_characters"),
            ("/updatedFields/long", "too_long"),
            ("/updatedFields/no good", "invalid_characters"),
        ]),
        "each entry where it is, the name written as a JSON Pointer writes it"
    );
    th.teardown().await;
}

/// The requests about the cards of a board: the parameters of their list, the property
/// values of a card, and its document.
#[tokio::test]
async fn the_requests_of_cards_list_everything_they_refuse() {
    let th = Setup::local_mode().start().await.init_basic().await;
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let cards = format!("{TEST_WS}/boards/{}/cards", board.id);

    let response = th.client.get(&format!("{cards}?page=x&perPage=1.5")).await;
    response.check_error(
        StatusCode::BAD_REQUEST,
        "2 things of the request were refused",
    );
    assert_eq!(
        response.value()["errors"],
        json!([
            {
                "parameter": "page",
                "detail": "invalid `page` parameter: not a whole number",
                "reason": "invalid",
            },
            {
                "parameter": "perPage",
                "detail": "invalid `perPage` parameter: not a whole number",
                "reason": "invalid",
            },
        ])
    );

    let values = json!({"fine": "text", "number": 7, "flag": true, "list": ["a", 1]});
    let response = th
        .client
        .post(
            &cards,
            &json!({"title": "Card", "icon": "ab", "boardId": "another", "properties": values}),
        )
        .await;
    response.check_bad_request();
    assert_eq!(
        refused(&response),
        pairs(&[
            ("/icon", "invalid"),
            ("/properties/flag", "invalid"),
            ("/properties/list", "invalid"),
            ("/properties/number", "invalid"),
            ("/boardId", "invalid"),
        ]),
        "the icon and the board with the values"
    );
    let made: Value = th
        .client
        .post(&cards, &json!({"title": "Card"}))
        .await
        .created_json();
    let card = made["id"].as_str().expect("an id");
    let response = th
        .client
        .patch(
            &format!("{TEST_WS}/cards/{card}"),
            &json!({
                "icon": "ab", "body": {"type": "doc", "content": [{"type": "paragraph"}]},
                "updatedProperties": values, "deletedProperties": ["flag"],
            }),
        )
        .await;
    response.check_bad_request();
    assert_eq!(
        refused(&response),
        pairs(&[
            ("/readAt", "required"),
            ("/icon", "invalid"),
            ("/updatedProperties/list", "invalid"),
            ("/updatedProperties/number", "invalid"),
        ]),
        "the icon and the missing readAt with the values; a value the patch also deletes \
         is not looked at"
    );
    check_more(&th.client, card).await;

    // A document that may not be stored is told of by its first node that is refused.
    let document = json!({"type": "doc", "content": [
        {"type": "paragraph"}, {"type": "table"}, {"type": "table"},
    ]});
    let response = th
        .client
        .post(&cards, &json!({"title": "Card", "body": document}))
        .await;
    response.check_bad_request();
    assert_eq!(response.refused_at(), "/body/content/1");

    th.teardown().await;
}

/// A value that is set and deleted in one patch is gone, whatever it was, as before; and
/// of more refusals than an answer lists the first hundred are listed, and the detail
/// says so.
async fn check_more(client: &crate::common::Client, card: &str) {
    client
        .patch(
            &format!("{TEST_WS}/cards/{card}"),
            &json!({"updatedProperties": {"flag": true}, "deletedProperties": ["flag"]}),
        )
        .await
        .check_ok();

    let many: serde_json::Map<String, Value> =
        (0..150).map(|n| (format!("p{n:03}"), json!(n))).collect();
    let response = client
        .patch(
            &format!("{TEST_WS}/cards/{card}"),
            &json!({"updatedProperties": many}),
        )
        .await;
    response.check_error(
        StatusCode::BAD_REQUEST,
        "150 members of the request were refused; the first 100 are listed",
    );
    assert_eq!(
        response.refused_places().len(),
        100,
        "no more entries than an answer lists"
    );
}

/// The opening of an upload and the change of the sharing of a board.
#[tokio::test]
async fn an_upload_and_the_sharing_list_everything_they_refuse() {
    let th = Setup::local_mode().start().await.init_basic().await;
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;

    let response = th
        .client
        .post(
            &format!("{TEST_WS}/containers/{}/uploads", board.id),
            &json!({"name": "", "size": -1, "contentType": "a".repeat(256)}),
        )
        .await;
    response.check_bad_request();
    assert_eq!(
        refused(&response),
        pairs(&[
            ("/name", "required"),
            ("/contentType", "too_long"),
            ("/size", "invalid"),
        ])
    );

    let response = th
        .client
        .post(
            &format!("{TEST_WS}/containers/{}/public-link", board.id),
            &json!({"token": "mine", "regenerate": "yes"}),
        )
        .await;
    response.check_bad_request();
    assert_eq!(
        refused(&response),
        pairs(&[
            ("/token", "invalid"),
            ("/enabled", "required"),
            ("/regenerate", "invalid"),
        ])
    );
    th.teardown().await;
}

/// The first user's name for the organization is its first workspace's too: where the
/// organization was given another workspace of that name before anybody signed up, it
/// is refused as taken, at the name, and nobody is made.
#[tokio::test]
async fn the_first_user_is_told_the_organizations_name_is_another_workspaces() {
    let th = Setup::native().start().await;
    th.app
        .store()
        .system()
        .execute_unchecked(&format!(
            "INSERT INTO workspaces (id, settings, created_at, updated_at, org_id, name) \
             SELECT '{}', '{{}}', 1, 1, org_id, 'Design' FROM workspaces WHERE id = '{}'",
            fvoci_model::new_id(),
            crate::common::ROOT_WORKSPACE_ID
        ))
        .await
        .expect("another workspace of the organization");
    let register = async |name: &str| {
        th.anonymous()
            .post(
                "/register",
                &json!({
                    "username": "first", "email": "first@sample.com",
                    "password": crate::common::PASSWORD, "organizationName": name,
                }),
            )
            .await
    };

    let taken = register("Design").await;
    taken.check_status(StatusCode::CONFLICT);
    assert_eq!(taken.error_reason(), "workspace_name_taken");
    assert_eq!(taken.refused_at(), "/organizationName");
    assert_eq!(
        th.app.registered_user_count().await.expect("user count"),
        0,
        "nobody is made"
    );
    register("Design studio").await.check_created();
    th.teardown().await;
}
