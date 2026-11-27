//! Administrators of the instance: who becomes one, and who may make others one.

use fvoci_model::{RegisterRequest, User};
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::common::{
    ApiResponse, Client, ORG_NAME, PASSWORD, ROOT_WS, Setup, TestServer, UNKNOWN_ID,
};

const ADMINS_ONLY: &str = "only an administrator of the instance may do this";

fn registration(username: &str) -> RegisterRequest {
    RegisterRequest {
        username: username.to_owned(),
        email: format!("{username}@sample.com"),
        password: PASSWORD.to_owned(),
        display_name: String::new(),
        organization_name: ORG_NAME.to_owned(),
        language: String::new(),
    }
}

async fn put_admin_flag(client: &Client, user_id: &str, flag: serde_json::Value) -> ApiResponse {
    client
        .put(
            &format!("/users/{user_id}/admin"),
            &json!({"isAdmin": flag}),
        )
        .await
}

/// `PUT /users/{id}/admin` with `true`.
async fn make_admin(client: &Client, user_id: &str) -> ApiResponse {
    put_admin_flag(client, user_id, json!(true)).await
}

/// `PUT /users/{id}/admin` with `false`.
async fn unmake_admin(client: &Client, user_id: &str) -> ApiResponse {
    put_admin_flag(client, user_id, json!(false)).await
}

#[track_caller]
fn check_admins_only(response: &ApiResponse) {
    response.check_error(StatusCode::FORBIDDEN, ADMINS_ONLY);
    assert_eq!(response.error_reason(), "admin_required", "the reason");
}

/// The first user to register is the administrator; whoever registers after is not.
/// Each learns it from `GET /users/me`, and others see it in the profile.
#[tokio::test]
async fn the_first_user_is_the_administrator_and_later_ones_are_not() {
    let th = Setup::native().start().await.init_basic().await;

    let first: Value = th.client.get("/users/me").await.ok_json();
    let second: Value = th.client2.get("/users/me").await.ok_json();
    assert_eq!(first["isAdmin"], json!(true), "the first user: {first}");
    assert_eq!(second["isAdmin"], json!(false), "the second user: {second}");

    let first_id = first["id"].as_str().expect("the user has an identifier");
    let seen: User = th
        .client2
        .get(&format!("{ROOT_WS}/users/{first_id}"))
        .await
        .ok_json();
    assert!(seen.is_admin, "who the administrator is, is no secret");

    th.teardown().await;
}

/// On a server that holds one organization only the first user registers. Of several
/// registrations that arrive at once at an instance without users, one is accepted,
/// starts the organization and is the administrator; the others are told that they need an
/// invitation, and nothing of them is left: no account, and no second organization.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn of_registrations_that_arrive_at_once_one_starts_the_organization() {
    let th = Setup::native().start().await;

    let attempts = (0..6).map(|index| {
        let client = th.anonymous();
        async move {
            client
                .post("/register", &registration(&format!("early{index}")))
                .await
        }
    });
    let answers = futures_util::future::join_all(attempts).await;

    let accepted: Vec<usize> = (0..answers.len())
        .filter(|index| answers[*index].status == StatusCode::CREATED)
        .collect();
    assert_eq!(accepted.len(), 1, "one registration is the first");
    assert_eq!(
        answers[accepted[0]].value()["isAdmin"],
        json!(true),
        "the answer says so"
    );
    for (index, answer) in answers.iter().enumerate() {
        if index != accepted[0] {
            answer.check_status(StatusCode::FORBIDDEN);
            assert_eq!(answer.error_reason(), "invitation_required", "{index}");
        }
    }
    let winner = th.login(&format!("early{}", accepted[0]), PASSWORD).await;
    let me: User = winner.get("/users/me").await.ok_json();
    assert!(me.is_admin, "and that user is the administrator");
    assert_eq!(
        th.app.registered_user_count().await.expect("the users"),
        1,
        "nobody else has an account"
    );
    // In the organization and in their own space, which nobody else got either.
    for (table, rows) in [("organization_members", 2), ("workspace_members", 2)] {
        assert_eq!(
            th.app
                .store()
                .system()
                .count_rows(table)
                .await
                .expect("the rows are counted"),
            rows,
            "{table}: the winner's and nobody else's"
        );
    }

    th.teardown().await;
}

/// `PUT /users/{id}/admin`: an administrator makes another user one and ends it again;
/// nobody else may; the instance keeps at least one administrator.
#[tokio::test]
async fn an_administrator_makes_and_unmakes_administrators() {
    let th = Setup::native().start().await.init_basic().await;
    let admin = th.client.me().await;
    let other = th.client2.me().await;

    // Nobody but an administrator, whoever the request names.
    for user_id in [other.id.as_str(), admin.id.as_str(), UNKNOWN_ID] {
        check_admins_only(&make_admin(&th.client2, user_id).await);
    }
    th.anonymous()
        .put(
            &format!("/users/{}/admin", other.id),
            &json!({"isAdmin": true}),
        )
        .await
        .check_unauthorized();

    // The body says which: a boolean.
    let path = format!("/users/{}/admin", other.id);
    for body in [
        json!({}),
        json!({"isAdmin": null}),
        json!({"isAdmin": "yes"}),
    ] {
        let response = th.client.put(&path, &body).await;
        response.check_status(StatusCode::BAD_REQUEST);
    }
    // A user that does not exist, by a well-formed identifier and by other text.
    for user_id in [UNKNOWN_ID, "nobody"] {
        make_admin(&th.client, user_id)
            .await
            .check_error(StatusCode::NOT_FOUND, "user not found");
    }

    // The only administrator cannot stop being one.
    let response = unmake_admin(&th.client, &admin.id).await;
    response.check_error(
        StatusCode::CONFLICT,
        "the instance must keep an administrator",
    );
    assert_eq!(response.error_reason(), "last_admin", "the reason");

    // Handing the role over: make the other one an administrator, then step down.
    let made: User = make_admin(&th.client, &other.id).await.ok_json();
    assert_eq!(made.id, other.id, "the user is answered");
    assert!(made.is_admin, "as an administrator");
    let again: User = make_admin(&th.client, &other.id).await.ok_json();
    assert!(
        again.is_admin,
        "making an administrator one changes nothing"
    );
    let stepped_down: User = unmake_admin(&th.client, &admin.id).await.ok_json();
    assert!(
        !stepped_down.is_admin,
        "the first administrator stepped down"
    );

    // Now the roles are the other way round.
    check_admins_only(&make_admin(&th.client, &admin.id).await);
    let response = unmake_admin(&th.client2, &other.id).await;
    assert_eq!(response.error_reason(), "last_admin", "the new only one");
    assert!(th.client2.me().await.is_admin, "is still the administrator");

    th.teardown().await;
}

/// Whoever an invitation lets in is no administrator, also when the administrator is
/// not there any more to object: the role is not for whoever comes next.
#[tokio::test]
async fn nobody_becomes_an_administrator_by_coming_later() {
    let th: TestServer = Setup::native().start().await.init_basic().await;

    let third = th.add_user("third").await;
    assert!(!third.me().await.is_admin, "the third user");
    // Without an invitation nobody gets in after the first user.
    let response = th
        .anonymous()
        .post("/register", &registration("fourth"))
        .await;
    response.check_status(StatusCode::FORBIDDEN);
    assert_eq!(response.error_reason(), "invitation_required");

    th.teardown().await;
}
