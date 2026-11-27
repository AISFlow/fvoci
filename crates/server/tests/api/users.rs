//! Integration tests for user registration, login, lookup, password changes, profiles,
//! and workspace metadata.

use fvoci_model::{ChangePasswordRequest, LoginRequest, RegisterRequest, Session, User, new_id};
use fvoci_server::auth::secret::new_secret;
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::common::ROOT_WS;
use crate::common::{
    ORG_NAME, PASSWORD, ROOT_WORKSPACE_ID, Setup, TestServer, UNKNOWN_ID, USER1_USERNAME,
};

const FAKE_USERNAME: &str = "fakeUsername";
const FAKE_EMAIL: &str = "mock@test.com";

fn register_request(password: &str) -> RegisterRequest {
    RegisterRequest {
        username: FAKE_USERNAME.to_owned(),
        email: FAKE_EMAIL.to_owned(),
        password: password.to_owned(),
        display_name: String::new(),
        organization_name: ORG_NAME.to_owned(),
        language: String::new(),
    }
}

/// The request that accepts the link of `token` with an account to make.
fn acceptance(token: &str, account: &Value) -> Value {
    json!({"token": token, "account": account})
}

/// Accepts the link of `token` with an account to make, as nobody.
async fn accept(th: &TestServer, token: &str, account: &Value) -> crate::common::ApiResponse {
    th.anonymous()
        .post("/invitations/accept", &acceptance(token, account))
        .await
}

fn login_request(username: &str, email: &str, password: &str) -> LoginRequest {
    LoginRequest {
        username: username.to_owned(),
        email: email.to_owned(),
        password: password.to_owned(),
    }
}

/// Registering a new user succeeds and rejects duplicate usernames or emails.
#[tokio::test]
async fn user_register() {
    let th = Setup::native().start().await;
    let request = register_request(&new_id());

    th.client.post("/register", &request).await.check_created();

    // The same user a second time: the server has its organization, and an invitation is needed.
    let again = th.client.post("/register", &request).await;
    again.check_error(
        StatusCode::FORBIDDEN,
        "this server has its organization; an invitation lets you in",
    );
    assert_eq!(again.error_reason(), "invitation_required");

    // With an invitation, the name is found to be taken.
    let link = th.member_link().await;
    let mut account = json!({
        "username": request.username, "email": "another@test.com", "password": new_id(),
    });
    let taken = th
        .client
        .post("/invitations/accept", &acceptance(&link, &account))
        .await;
    taken.check_status(StatusCode::CONFLICT);
    assert_eq!(taken.error_reason(), "username_taken", "the name is taken");
    assert_eq!(taken.refused_at(), "/account/username");

    account["username"] = json!("another");
    account["email"] = json!(request.email);
    let taken = th
        .client
        .post("/invitations/accept", &acceptance(&link, &account))
        .await;
    taken.check_status(StatusCode::CONFLICT);
    assert_eq!(taken.error_reason(), "email_taken", "the address is taken");
    assert_eq!(taken.refused_at(), "/account/email");

    th.teardown().await;
}

/// Logging in with a non-existent user fails with 401.
#[tokio::test]
async fn user_login_with_nonexist_user() {
    let th = Setup::native().start().await;
    let request = login_request("nonexistuser", "", &new_id());

    let response = th.client.post("/login", &request).await;

    response.check_unauthorized();
    assert_eq!(
        response.error_message(),
        "incorrect login",
        "the answer does not say why"
    );
    th.teardown().await;
}

/// Logging in with a registered user succeeds and issues a session cookie.
#[tokio::test]
async fn user_login_with_registered_user() {
    let th = Setup::native().start().await;
    let password = new_id();
    th.client
        .post("/register", &register_request(&password))
        .await
        .check_created();

    let response = th
        .client
        .post(
            "/login",
            &login_request(FAKE_USERNAME, FAKE_EMAIL, &password),
        )
        .await;

    // A login has no body, so no token in one: a script of the page never sees it.
    response.check_no_content();
    let token = response
        .session_cookie()
        .expect("the token comes in the session cookie");
    assert_eq!(
        token.len(),
        fvoci_server::auth::secret::SECRET_LENGTH,
        "the session token is a secret, not an identifier: its length"
    );
    assert!(
        !fvoci_model::is_id(&token),
        "the session token does not have the form of an identifier"
    );
    th.teardown().await;
}

/// Login error cases return uniform authentication failures, and a user can sign in by
/// username or email address.
#[tokio::test]
async fn login_cases() {
    let th = Setup::native().start().await;
    th.client
        .post("/register", &register_request("testPassword"))
        .await
        .check_created();

    let failures = [
        ("fail, missing login information", "", "", ""),
        ("fail, invalid username", "badUsername", "", ""),
        ("fail, invalid email", "", "badEmail", ""),
        ("fail, invalid password", FAKE_USERNAME, "", "badPassword"),
    ];
    for (title, username, email, password) in failures {
        let response = th
            .client
            .post("/login", &login_request(username, email, password))
            .await;
        assert_eq!(response.status, StatusCode::UNAUTHORIZED, "{title}");
        assert_eq!(response.error_message(), "incorrect login", "{title}");
    }

    let successes = [
        ("success, using username", FAKE_USERNAME, ""),
        ("success, using email", "", FAKE_EMAIL),
    ];
    for (title, username, email) in successes {
        let response = th
            .client
            .post("/login", &login_request(username, email, "testPassword"))
            .await;
        assert_eq!(
            response.status,
            StatusCode::NO_CONTENT,
            "{title}: {}",
            response.text()
        );
    }

    // There is one way to sign in. A `type` member is no member of a login: whatever it
    // says, the right credentials sign in and the wrong ones do not.
    for login_type in [
        json!("normal"),
        json!("saml"),
        json!(""),
        json!(7),
        json!(null),
    ] {
        let with_type = |password: &str| json!({"type": login_type, "username": FAKE_USERNAME, "password": password});
        let response = th.client.post("/login", &with_type("testPassword")).await;
        assert_eq!(
            response.status,
            StatusCode::NO_CONTENT,
            "type {login_type}: {}",
            response.text()
        );
        assert!(
            response.session_cookie().is_some(),
            "type {login_type}: the login opens a session"
        );
        let response = th.client.post("/login", &with_type("badPassword")).await;
        assert_eq!(
            response.status,
            StatusCode::UNAUTHORIZED,
            "type {login_type}"
        );
    }

    // A body is a JSON object. The members in a list, in the order the server happens to
    // declare them, are no login.
    let response = th
        .client
        .send_raw_body(
            reqwest::Method::POST,
            "/login",
            &format!(r#"["{FAKE_USERNAME}","","testPassword"]"#),
        )
        .await;
    response.check_bad_request();
    assert_eq!(
        response.problem_type(),
        "/problems/bad-request",
        "{}",
        response.text()
    );

    // A body that is not JSON is the sender's mistake.
    let response = th
        .client
        .send_raw_body(reqwest::Method::POST, "/login", "{not json")
        .await;
    response.check_bad_request();
    assert_eq!(
        response.problem_type(),
        "/problems/bad-request",
        "{}",
        response.text()
    );

    th.teardown().await;
}

/// Registration validation cases through the API.
#[tokio::test]
async fn register_user_cases() {
    let th = Setup::native().start().await;

    // "fail, missing login information" and "fail, invalid password": the request is
    // checked before a user is created.
    let cases = [
        (
            json!({"username": "", "email": "new@example.com", "password": "testPassword", "organizationName": "Ours"}),
            "username is required",
        ),
        (
            json!({"username": "", "email": "", "password": ""}),
            "4 members of the request were refused",
        ),
        (
            json!({"username": "newUsername", "email": "", "password": "testPassword", "organizationName": "Ours"}),
            "email is required",
        ),
        (
            json!({"username": "newUsername", "email": "not-an-address", "password": "testPassword", "organizationName": "Ours"}),
            "invalid email format",
        ),
        (
            json!({"username": "newUsername", "email": "new@example.com", "password": "", "organizationName": "Ours"}),
            "password is required",
        ),
        (
            json!({"username": "newUsername", "email": "new@example.com", "password": "test", "organizationName": "Ours"}),
            "password must be at least",
        ),
        (
            json!({"username": "newUsername", "email": "new@example.com", "password": "testPassword"}),
            "organization name is required",
        ),
        (
            json!({"username": "newUsername", "email": "new@example.com", "password": "testPassword", "organizationName": "  "}),
            "organization name is required",
        ),
        (
            json!({"username": "newUsername", "email": "new@example.com", "password": "testPassword", "organizationName": "x".repeat(65)}),
            "an organization name has at most 64 characters",
        ),
    ];
    for (body, message) in cases {
        let response = th.client.post("/register", &body).await;
        assert_eq!(
            response.status,
            StatusCode::BAD_REQUEST,
            "{body}: {}",
            response.text()
        );
        assert!(
            response.error_message().contains(message),
            "{body}: expected {message:?} in {:?}",
            response.error_message()
        );
    }
    assert_eq!(
        th.app.registered_user_count().await.expect("user count"),
        0,
        "no refused request created a user"
    );

    // "success"
    let body = json!({"username": " newUsername ", "email": " new@example.com ", "password": "testPassword", "organizationName": "Our Firm"});
    let made: User = th.client.post("/register", &body).await.created_json();
    assert_eq!(made.username, "newUsername", "the name is answered trimmed");
    let user = th
        .app
        .store()
        .get_user_by_username("newUsername")
        .await
        .expect("the name is stored trimmed");
    assert_eq!(
        user.email, "new@example.com",
        "the address is stored trimmed"
    );
    assert!(
        user.password_hash
            .starts_with("$argon2id$v=19$m=19456,t=2,p=1$"),
        "the password is stored as an Argon2id hash with the current parameters: {}",
        user.password_hash
    );
    assert!(
        !user.password_hash.contains("testPassword"),
        "and not as itself"
    );

    th.teardown().await;
}

/// A registration answers the user that was made: the members a profile has and no
/// others, so neither the address nor anything of the password. The first user of an
/// instance is its administrator. Registering signs nobody in: no cookie is set.
#[tokio::test]
async fn a_registration_answers_the_user_that_was_made() {
    let th = Setup::native().start().await;
    let first = th
        .anonymous()
        .post(
            "/register",
            &json!({
                "username": "first",
                "email": "first@sample.com",
                "password": PASSWORD,
                "organizationName": ORG_NAME,
            }),
        )
        .await;

    let (response, username, is_admin) = (&first, "first", true);
    response.check_created();
    let made = response.value();
    let mut members: Vec<&str> = made
        .as_object()
        .expect("the answer is an object")
        .keys()
        .map(String::as_str)
        .collect();
    members.sort_unstable();
    assert_eq!(
        members,
        [
            "createdAt",
            "credentials",
            "customName",
            "displayName",
            "firstName",
            "id",
            "isAdmin",
            "lastName",
            "latinFirstName",
            "latinLastName",
            "nameShown",
            "title",
            "updatedAt",
            "username"
        ],
        "{username}: the members of the answer, sorted: {made}"
    );
    assert_eq!(made["username"], json!(username), "{made}");
    assert_eq!(made["isAdmin"], json!(is_admin), "{made}");
    assert_eq!(made.get("deletedAt"), None, "{made}");
    assert!(
        made["id"].as_str().is_some_and(fvoci_model::is_id),
        "{username}: the identifier the server gave: {made}"
    );
    assert_eq!(
        response.set_cookies(),
        Vec::<&str>::new(),
        "{username}: a registration opens no session"
    );

    // The answer is the user as the API tells it afterwards, who is told of their own
    // account besides that it has the password and the address it was registered with.
    let client = th.login(username, PASSWORD).await;
    let mut me = client.get("/users/me").await.value();
    let own = me.as_object_mut().expect("an object");
    assert_eq!(
        own.remove("hasPassword"),
        Some(json!(true)),
        "{username}: the account has the password it registered with"
    );
    assert_eq!(
        own.remove("email"),
        Some(json!("first@sample.com")),
        "{username}: and the address"
    );
    assert_eq!(
        me, made,
        "{username}: the same as the user reads about themselves"
    );

    th.teardown().await;
}

/// `GET /users/me` rejects unauthenticated requests.
#[tokio::test]
async fn get_me_not_login_yet() {
    let th = Setup::native().start().await;

    let response = th.client.get("/users/me").await;

    response.check_unauthorized();
    assert_eq!(
        response.error_message(),
        "no session token",
        "there is no session"
    );
    th.teardown().await;
}

/// `GET /users/me` returns the signed-in user's profile.
#[tokio::test]
async fn get_me_logged_in() {
    let th = Setup::native().start().await;
    let password = new_id();
    let client = th
        .register_and_login(FAKE_USERNAME, FAKE_EMAIL, &password)
        .await;

    let response = client.get("/users/me").await;

    let me: User = response.ok_json();
    assert_eq!(me.username, FAKE_USERNAME, "the name is");
    let raw = response.value();
    assert_eq!(
        raw,
        json!({
            "id": me.id,
            "username": FAKE_USERNAME,
            "displayName": "",
            "firstName": "",
            "lastName": "",
            "latinFirstName": "",
            "latinLastName": "",
            "customName": "",
            "nameShown": "name",
            "title": null,
            "credentials": [],
            "createdAt": me.created_at,
            "updatedAt": me.updated_at,
            // The first user of an instance administers it.
            "isAdmin": true,
            // Told to the user themselves only: whether, never what.
            "hasPassword": true,
            // And their own address, which no other answer carries; nothing waits to
            // replace it, so `pendingEmail` is left out.
            "email": FAKE_EMAIL,
        }),
        "the members of a user on the wire to themselves: no password, nothing else"
    );
    assert!(me.created_at > 0, "a creation time: {raw}");

    // Unknown query parameters such as `channelID` are ignored on `/users/me`.
    let with_channel = client.get("/users/me?channelID=any-channel").await.value();
    assert_eq!(with_channel, raw, "the same answer");

    th.teardown().await;
}

/// `GET /users/{id}` returns an existing user and 404 for a non-existent user.
#[tokio::test]
async fn get_user() {
    let th = Setup::native().start().await;
    let client = th
        .register_and_login(FAKE_USERNAME, FAKE_EMAIL, &new_id())
        .await;
    let me = client.me().await;

    // "me's id"
    let user: User = client
        .get(&format!("{ROOT_WS}/users/{}", me.id))
        .await
        .ok_json();
    assert_eq!(user.id, me.id, "the same user");
    assert_eq!(user.username, me.username, "the same name");

    // "nonexist user"
    let response = client.get(&format!("{ROOT_WS}/users/nonexistid")).await;
    response.check_error(StatusCode::NOT_FOUND, "user not found");

    th.teardown().await;
}

/// App-level user lookup cases.
#[tokio::test]
async fn app_get_user_cases() {
    let th = Setup::native().start().await.init_basic().await;
    let me = th.client.me().await;

    assert!(th.app.user("").await.is_err(), "fail, missing id");
    assert!(th.app.user("badID").await.is_err(), "fail, invalid id");
    assert_eq!(
        th.app.user(&me.id).await.expect("success").id,
        me.id,
        "success"
    );

    th.teardown().await;
}

/// The users-list routes and the profile fields other users see.
#[tokio::test]
async fn users_list_and_profiles() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;
    let user2 = th.client2.me().await;

    let listed: Vec<User> = th
        .client
        .post(&format!("{ROOT_WS}/users"), &[&user1.id, &user2.id])
        .await
        .ok_json();
    let mut names: Vec<&str> = listed.iter().map(|user| user.username.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, ["user1", "user2"], "both users are listed");

    let empty: Vec<String> = Vec::new();
    let response = th.client.post(&format!("{ROOT_WS}/users"), &empty).await;
    response.check_bad_request();
    assert_eq!(
        response.error_message(),
        "User IDs are empty",
        "an empty list is refused"
    );

    // At most 200 identifiers at once: more is a bad request, as an empty list is.
    let many = |n: usize| (0..n).map(|_| UNKNOWN_ID.to_owned()).collect::<Vec<_>>();
    let at_most: Vec<User> = th
        .client
        .post(&format!("{ROOT_WS}/users"), &many(200))
        .await
        .ok_json();
    assert!(at_most.is_empty(), "{at_most:?}");
    th.client
        .post(&format!("{ROOT_WS}/users"), &many(201))
        .await
        .check_error(
            StatusCode::BAD_REQUEST,
            "at most 200 user IDs are asked for at once",
        );

    // An identifier of nobody finds nothing; the list is not refused.
    let nobody: Vec<User> = th
        .client
        .post(&format!("{ROOT_WS}/users"), &["nonexistent"])
        .await
        .ok_json();
    assert!(nobody.is_empty(), "{nobody:?}");

    let workspace_users: Vec<User> = th.client.get(&format!("{ROOT_WS}/users")).await.ok_json();
    assert_eq!(workspace_users.len(), 2, "the workspace has both users");
    let found: Vec<User> = th
        .client
        .get(&format!("{ROOT_WS}/users?search=user2"))
        .await
        .ok_json();
    assert_eq!(found.len(), 1, "the search narrows the list");
    assert_eq!(found[0].id, user2.id, "to the user that matches");

    let by_id: Vec<User> = th
        .client
        .post(&format!("{ROOT_WS}/users"), &[&user2.id])
        .await
        .ok_json();
    assert_eq!(by_id.len(), 1, "one user was asked for");

    // The members of a user on the wire, as another user sees them.
    let profile = th
        .client
        .get(&format!("{ROOT_WS}/users/{}", user2.id))
        .await
        .value();
    assert_eq!(
        profile,
        json!({
            "id": user2.id,
            "username": "user2",
            "displayName": "",
            "firstName": "",
            "lastName": "",
            "latinFirstName": "",
            "latinLastName": "",
            "customName": "",
            "nameShown": "name",
            "title": null,
            "credentials": [],
            "createdAt": user2.created_at,
            "updatedAt": user2.updated_at,
            "isAdmin": false,
        }),
        "the profile of another user"
    );

    th.teardown().await;
}

/// Changing a user's password updates the credential used for subsequent logins.
#[tokio::test]
async fn user_change_password() {
    let th = Setup::native().start().await;
    let password = new_id();
    let client = th
        .register_and_login(FAKE_USERNAME, FAKE_EMAIL, &password)
        .await;
    let new_password = new_id();

    let request = ChangePasswordRequest {
        old_password: password.clone(),
        new_password: new_password.clone(),
    };
    client
        .post("/users/me/password", &request)
        .await
        .check_no_content();

    // The new password signs in and the old one no longer does.
    th.login(FAKE_USERNAME, &new_password).await;
    let mut old = th.anonymous();
    old.login(FAKE_USERNAME, &password)
        .await
        .check_unauthorized();

    th.teardown().await;
}

/// App-level password change validation cases.
#[tokio::test]
async fn app_change_password_cases() {
    let th = Setup::native().start().await.init_basic().await;
    let session = th
        .app
        .authenticator()
        .get_session(&th.client.token)
        .await
        .expect("the first client has a session")
        .session;

    // "fail, missing login information" and "fail, invalid userId": a session of nobody,
    // and one of a user who is not there.
    for user_id in [String::new(), "badID".to_owned(), UNKNOWN_ID.to_owned()] {
        let of_nobody = fvoci_model::Session {
            user_id,
            ..session.clone()
        };
        assert!(
            th.app
                .change_password(&of_nobody, "", "", None)
                .await
                .is_err(),
            "a session without a user changes no password: {of_nobody:?}"
        );
    }
    assert!(
        th.app
            .change_password(&session, "wrongPassword", "newPassword", None)
            .await
            .is_err(),
        "fail, invalid password"
    );
    th.app
        .change_password(&session, PASSWORD, "newPassword", None)
        .await
        .expect("success, with the right old password");

    th.teardown().await;
}

/// App-level administrative password update cases.
#[tokio::test]
async fn app_update_user_password_cases() {
    let th = Setup::native().start().await.init_basic().await;

    assert!(
        th.app.reset_user_password("", "").await.is_err(),
        "fail, missing login information"
    );
    assert!(
        th.app.reset_user_password("badUsername", "").await.is_err(),
        "fail, invalid username"
    );
    th.app
        .reset_user_password("user1", "testPassword")
        .await
        .expect("success, username");
    th.login("user1", "testPassword").await;

    th.teardown().await;
}

/// Password changes always operate on the signed-in session's own account; paths naming
/// an arbitrary user ID do not exist, preventing one user from changing another's password.
#[tokio::test]
async fn a_password_is_changed_for_the_session_and_for_no_user_named_in_the_path() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;
    let user2 = th.client2.me().await;
    let request = ChangePasswordRequest {
        old_password: PASSWORD.to_owned(),
        new_password: "another-password".to_owned(),
    };

    for user_id in [&user1.id, &user2.id] {
        for path in [
            format!("/users/{user_id}/changepassword"),
            format!("/users/{user_id}/password"),
        ] {
            let response = th.client.post(&path, &request).await;
            response.check_error(StatusCode::NOT_FOUND, "route not found");
        }
    }
    // No password changed.
    th.login("user1", PASSWORD).await;
    th.login("user2", PASSWORD).await;

    // A wrong old password on one's own account is rejected with 400.
    let wrong = ChangePasswordRequest {
        old_password: "not-the-password".to_owned(),
        new_password: "another-password".to_owned(),
    };
    let response = th.client.post("/users/me/password", &wrong).await;
    response.check_bad_request();
    th.login("user1", PASSWORD).await;

    th.teardown().await;
}

/// Logging out ends the session.
#[tokio::test]
async fn logout_ends_the_session() {
    let th = Setup::native().start().await.init_basic().await;

    th.client.post_empty("/logout").await.check_no_content();

    th.client.get("/users/me").await.check_unauthorized();
    // The other user's session is untouched.
    th.client2.get("/users/me").await.check_ok();
    th.teardown().await;
}

/// The state a client keeps with the account is read and patched through the session:
/// the answer to both is an object, the value of every entry by its name. No path names
/// the user whose state it is.
#[tokio::test]
async fn client_state_is_read_and_patched_by_its_owner() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;
    let user2 = th.client2.me().await;

    let before = th.client.get(CLIENT_STATE).await;
    before.check_ok();
    assert_eq!(before.text(), "{}", "a new user has no client state");

    let patch = json!({"updatedFields": {"theme": "dark", "welcomePageViewed": "1"}});
    let after = th.client.patch(CLIENT_STATE, &patch).await;
    after.check_ok();
    assert_eq!(
        after.value(),
        json!({"theme": "dark", "welcomePageViewed": "1"}),
        "the answer to a patch is all of the state"
    );
    assert_eq!(
        th.client.get(CLIENT_STATE).await.value(),
        after.value(),
        "as stored"
    );

    let patch = json!({"deletedFields": ["theme"]});
    let after = th.client.patch(CLIENT_STATE, &patch).await.value();
    assert_eq!(
        after,
        json!({"welcomePageViewed": "1"}),
        "one entry is left"
    );

    // Each user has their own, and nobody reads it without a session.
    assert_eq!(
        th.client2.get(CLIENT_STATE).await.value(),
        json!({}),
        "the state of another user is not this one's"
    );
    th.anonymous().get(CLIENT_STATE).await.check_unauthorized();
    th.anonymous()
        .patch(CLIENT_STATE, &patch)
        .await
        .check_unauthorized();

    // The state is patched: the other methods are not allowed on its path.
    let response = th.client.put(CLIENT_STATE, &patch).await;
    response.check_error(StatusCode::METHOD_NOT_ALLOWED, "method not allowed");

    // The paths that name a user are no routes: not for the user's own identifier, not
    // for another user's, not for the identifier of nobody.
    let patch = json!({"updatedFields": {"theme": "light"}});
    for user in [user1.id.as_str(), user2.id.as_str(), "nobody"] {
        for path in [
            format!("/users/{user}/config"),
            format!("/users/{user}/client-state"),
        ] {
            th.client
                .put(&path, &patch)
                .await
                .check_error(StatusCode::NOT_FOUND, "route not found");
            th.client
                .patch(&path, &patch)
                .await
                .check_error(StatusCode::NOT_FOUND, "route not found");
        }
    }
    th.client
        .get("/users/me/config")
        .await
        .check_error(StatusCode::NOT_FOUND, "route not found");
    for client in [&th.client, &th.client2] {
        assert!(
            client
                .get(CLIENT_STATE)
                .await
                .value()
                .get("theme")
                .is_none(),
            "none of these stored anything"
        );
    }

    th.teardown().await;
}

// -- settings and the bag of client state ----------------------------------------------

const SETTINGS: &str = "/users/me/settings";
const CLIENT_STATE: &str = "/users/me/client-state";

/// `GET /users/me/settings` answers every setting; what was never set has its default.
/// `PATCH` changes the settings it names, answers all of them, and `null` puts a setting
/// back. Settings are the account's: another session of the user reads the same.
#[tokio::test]
async fn settings_are_read_changed_and_reset() {
    let th = Setup::native().start().await.init_basic().await;
    let defaults = json!({
        "timeZone": "auto", "weekStart": "auto", "hourCycle": "auto",
        "theme": "auto", "cardOpen": "center", "density": "auto", "haptics": true,
        "language": "auto",
        "notifyMentions": true, "notifyAssignments": true, "notifyComments": true,
        "notifyDue": true,
    });

    assert_eq!(th.client.get(SETTINGS).await.value(), defaults, "defaults");

    let changed = th
        .client
        .patch(
            SETTINGS,
            &json!({
                "timeZone": "Asia/Seoul", "weekStart": "monday", "hourCycle": "h23",
                "theme": "dark", "cardOpen": "side", "density": "compact", "haptics": false,
                "language": "ko",
                "notifyMentions": false, "notifyAssignments": false, "notifyComments": false,
                "notifyDue": false,
            }),
        )
        .await;
    changed.check_ok();
    let all_set = json!({
        "timeZone": "Asia/Seoul", "weekStart": "monday", "hourCycle": "h23",
        "theme": "dark", "cardOpen": "side", "density": "compact", "haptics": false,
        "language": "ko",
        "notifyMentions": false, "notifyAssignments": false, "notifyComments": false,
        "notifyDue": false,
    });
    assert_eq!(changed.value(), all_set, "the answer is every setting");
    assert_eq!(th.client.get(SETTINGS).await.value(), all_set, "as stored");
    let elsewhere = th.login(USER1_USERNAME, PASSWORD).await;
    assert_eq!(
        elsewhere.get(SETTINGS).await.value(),
        all_set,
        "another session of the user has the same settings"
    );
    assert_eq!(
        th.client2.get(SETTINGS).await.value(),
        defaults,
        "another user has their own"
    );

    // What the change does not name stays; `null` puts a setting back to its default.
    let partly = th
        .client
        .patch(
            SETTINGS,
            &json!({
                "theme": null, "haptics": null, "cardOpen": "full", "language": "en-US",
                "notifyComments": null, "notifyDue": null,
            }),
        )
        .await
        .value();
    assert_eq!(
        partly,
        json!({
            "timeZone": "Asia/Seoul", "weekStart": "monday", "hourCycle": "h23",
            "theme": "auto", "cardOpen": "full", "density": "compact", "haptics": true,
            "language": "en-US",
            "notifyMentions": false, "notifyAssignments": false, "notifyComments": true,
            "notifyDue": true,
        }),
        "four put back, two changed, the rest as they were"
    );
    assert_eq!(
        th.client.patch(SETTINGS, &json!({})).await.value(),
        partly,
        "a change of nothing answers the settings"
    );
    let reset = th
        .client
        .patch(SETTINGS, &json!({"language": null}))
        .await
        .value();
    assert_eq!(
        reset["language"],
        json!("auto"),
        "the browser's language again"
    );

    th.anonymous().get(SETTINGS).await.check_unauthorized();
    th.anonymous()
        .patch(SETTINGS, &json!({"theme": "dark"}))
        .await
        .check_unauthorized();

    th.teardown().await;
}

/// The settings are a closed set, and so are their values. What is no setting, and what
/// is no value of one, is refused with the member that is wrong, and changes nothing:
/// not even the valid members of the same request.
#[tokio::test]
async fn settings_refuse_what_is_no_setting_or_no_value_of_one() {
    let th = Setup::native().start().await.init_basic().await;
    let before = th
        .client
        .patch(SETTINGS, &json!({"density": "comfortable"}))
        .await
        .value();

    let refused = [
        (json!({"timeZone": "Mars/Olympus_Mons"}), "timeZone"),
        (json!({"timeZone": "asia/seoul"}), "timeZone"),
        (json!({"timeZone": ""}), "timeZone"),
        (json!({"weekStart": "tuesday"}), "weekStart"),
        (json!({"hourCycle": "h24"}), "hourCycle"),
        (json!({"theme": "sepia"}), "theme"),
        (json!({"theme": "Dark"}), "theme"),
        (json!({"cardOpen": "auto"}), "cardOpen"),
        (json!({"density": "cozy"}), "density"),
        (json!({"haptics": "true"}), "haptics"),
        (json!({"haptics": 1}), "haptics"),
        (json!({"notifyComments": "off"}), "notifyComments"),
        (json!({"notifyMentions": 0}), "notifyMentions"),
        (json!({"language": "Korean language"}), "language"),
        (json!({"language": ""}), "language"),
        (json!({"language": "ko_KR"}), "language"),
        (json!({"language": "Auto"}), "language"),
        (json!({"language": "en-US-US"}), "language"),
        (json!({"language": 82}), "language"),
        (json!({"theme": 3}), "theme"),
        (json!({"locale": "ko"}), "locale"),
        (json!({"timezone": "auto"}), "timezone"),
        (json!({"density": "compact", "theme": "sepia"}), "theme"),
    ];
    for (body, field) in &refused {
        let response = th.client.patch(SETTINGS, body).await;
        response.check_bad_request();
        assert_eq!(response.refused_at(), format!("/{field}"), "{body}");
    }
    let response = th
        .client
        .patch(SETTINGS, &json!({"cardOpen": "auto"}))
        .await;
    response.check_error(
        StatusCode::BAD_REQUEST,
        "cardOpen must be one of \"center\", \"side\", \"full\"",
    );
    let response = th.client.patch(SETTINGS, &json!({"locale": "ko"})).await;
    response.check_error(StatusCode::BAD_REQUEST, "there is no setting \"locale\"");
    for body in ["[]", "null", "5", "{"] {
        th.client
            .send_raw_body(reqwest::Method::PATCH, SETTINGS, body)
            .await
            .check_bad_request();
    }

    assert_eq!(
        th.client.get(SETTINGS).await.value(),
        before,
        "none of these changed anything"
    );
    // Names the time zone database keeps for compatibility are names, kept as given.
    let old_name = th
        .client
        .patch(SETTINGS, &json!({"timeZone": "Asia/Calcutta"}))
        .await
        .value();
    assert_eq!(old_name["timeZone"], json!("Asia/Calcutta"));

    th.teardown().await;
}

/// The bag of client state takes names and values the server does not know, within
/// bounds: the characters and length of a name, the size of a value, and how many
/// entries there are. A setting and an entry of the bag do not touch each other.
#[tokio::test]
async fn client_state_is_bounded_and_apart_from_the_settings() {
    let th = Setup::native().start().await.init_basic().await;
    let set = async |updated: Value| {
        th.client
            .patch(CLIENT_STATE, &json!({"updatedFields": updated}))
            .await
    };

    set(json!({"theme": "of-the-client", "tour.step-1_x": "1"}))
        .await
        .check_ok();
    assert_eq!(
        th.client.get(SETTINGS).await.value()["theme"],
        json!("auto"),
        "an entry of the bag named like a setting is no setting"
    );
    th.client
        .patch(SETTINGS, &json!({"theme": "dark"}))
        .await
        .check_ok();
    let bag = th.client.get(CLIENT_STATE).await.value();
    assert_eq!(
        bag,
        json!({"theme": "of-the-client", "tour.step-1_x": "1"}),
        "the bag is untouched by the setting"
    );

    for name in ["", "with space", "키", "a/b", &"n".repeat(65)] {
        let response = set(json!({name: "v"})).await;
        response.check_bad_request();
        // The entry itself, its name written as a JSON Pointer writes it.
        assert_eq!(
            response.refused_at(),
            format!("/updatedFields/{}", name.replace('/', "~1")),
            "{name:?}"
        );
    }
    set(json!({"n".repeat(64): "v".repeat(4096)}))
        .await
        .check_ok();
    let response = set(json!({"big": "v".repeat(4097)})).await;
    response.check_error(
        StatusCode::BAD_REQUEST,
        "the value of an entry is at most 4096 bytes",
    );

    // The bag holds a hundred entries; three are there.
    let many = |range: std::ops::Range<usize>| -> Value {
        range
            .map(|index| (format!("entry-{index}"), json!("v")))
            .collect::<serde_json::Map<String, Value>>()
            .into()
    };
    set(many(0..97)).await.check_ok();
    let response = set(many(97..98)).await;
    response.check_error(
        StatusCode::BAD_REQUEST,
        "a user keeps at most 100 entries of client state",
    );
    assert_eq!(response.value()["limit"], json!(100), "the limit is told");
    assert_eq!(
        th.client
            .get(CLIENT_STATE)
            .await
            .value()
            .as_object()
            .map(serde_json::Map::len),
        Some(100),
        "the refused entry was not stored"
    );

    th.teardown().await;
}

/// The workspace routes.
#[tokio::test]
async fn workspaces() {
    let th = Setup::native().start().await.init_basic().await;

    let workspaces = th.client.get("/users/me/workspaces").await.value();
    assert_eq!(
        workspaces.as_array().map(Vec::len),
        Some(3),
        "the two workspaces of the test server, and the user's own: {workspaces}"
    );
    assert_eq!(
        workspaces[0]["id"],
        json!(ROOT_WORKSPACE_ID),
        "the oldest workspace, the root workspace, comes first: {workspaces}"
    );

    // A workspace is looked up strictly by its identifier; unknown or invalid IDs return 404.
    for no_workspace in [UNKNOWN_ID, "some-other-workspace", "0"] {
        let response = th.client.get(&format!("/w/{no_workspace}")).await;
        response.check_error(StatusCode::NOT_FOUND, "workspace not found");
    }
    let workspace = th.client.get(ROOT_WS).await.value();
    assert_eq!(workspace["id"], json!(ROOT_WORKSPACE_ID), "{workspace}");
    let mut members: Vec<&str> = workspace
        .as_object()
        .expect("the workspace is an object")
        .keys()
        .map(String::as_str)
        .collect();
    members.sort_unstable();
    assert_eq!(
        members,
        [
            "guestInvitations",
            "id",
            "modifiedBy",
            "settings",
            "title",
            "updatedAt"
        ],
        "a workspace carries no token of anything: {workspace}"
    );
    // The route that renewed the signup token is gone with the token.
    th.client
        .post_empty(&format!("{ROOT_WS}/signup-token"))
        .await
        .check_status(StatusCode::NOT_FOUND);

    th.anonymous().get(ROOT_WS).await.check_unauthorized();
    th.teardown().await;
}

/// The root workspace (the oldest workspace of the organization) is created once and
/// returned consistently on subsequent lookups.
#[tokio::test]
async fn app_get_root_workspace() {
    let th = Setup::native().start().await;

    let first = th.root_workspace().await;
    let second = th.root_workspace().await;

    assert_eq!(
        first.id, ROOT_WORKSPACE_ID,
        "the root workspace has the global identifier"
    );
    assert_eq!(first.modified_by, "", "nobody modified it");
    assert_eq!(first, second, "reading does not change it");
    th.teardown().await;
}

// -- password hashes and session tokens -----------------------------------------------

/// The session token is handed out once, to whoever signed in. What the server keeps is
/// its digest: nothing in the store is something a session could be opened with.
#[tokio::test]
async fn the_store_keeps_the_digest_of_a_session_token_and_not_the_token() {
    let th = Setup::native().start().await.init_basic().await;
    let token = th.client.token.clone();
    let now = fvoci_server::auth::now_millis();
    let store = th.app.store();

    let session = store
        .get_session(&fvoci_server::auth::secret::token_digest(&token), now, 0)
        .await
        .expect("the session is stored under the digest of its token");
    assert_eq!(
        session.user_id,
        th.client.me().await.id,
        "the user's session"
    );
    let by_token = store.get_session(&token, now, 0).await;
    assert!(
        by_token.is_err(),
        "nothing is stored under the token itself: {by_token:?}"
    );
    // And the digest is no token: presenting it does not sign anyone in.
    th.client_with_token(&fvoci_server::auth::secret::token_digest(&token))
        .get("/users/me")
        .await
        .check_unauthorized();

    th.teardown().await;
}

// -- how long a session lasts ---------------------------------------------------------

const MINUTE: i64 = 60 * 1000;
const DAY: i64 = 24 * 60 * MINUTE;

/// The stored session of the harness's first client, whatever its age.
async fn stored_session(th: &TestServer) -> Session {
    th.app
        .store()
        .get_session(
            &fvoci_server::auth::secret::token_digest(&th.client.token),
            0,
            i64::MIN,
        )
        .await
        .expect("the session is stored")
}

/// A session that nobody used for the idle limit has ended, and one that was used a
/// little more recently has not. The limit is `session_idle_days`.
#[tokio::test]
async fn a_session_ends_when_it_was_not_used_for_the_idle_limit() {
    let th = Setup::native()
        .configure(|config| config.session_idle_days = 3)
        .start()
        .await
        .init_basic()
        .await;
    let session = stored_session(&th).await;
    let store = th.app.store();
    let now = fvoci_server::auth::now_millis();

    store
        .set_session_last_used(&session.id, now - 3 * DAY + MINUTE)
        .await
        .expect("the last use is moved back");
    th.client.get("/users/me").await.check_ok();

    store
        .set_session_last_used(&session.id, now - 3 * DAY - MINUTE)
        .await
        .expect("the last use is moved back further");
    th.client.get("/users/me").await.check_unauthorized();
    // The other client's session is its own.
    th.client2.get("/users/me").await.check_ok();

    th.teardown().await;
}

/// A session ends `session_max_days` after it began, however recently it was used.
#[tokio::test]
async fn a_session_ends_at_its_absolute_limit_however_much_it_is_used() {
    let th = Setup::native()
        .configure(|config| config.session_max_days = 7)
        .start()
        .await
        .init_basic()
        .await;
    let session = stored_session(&th).await;
    assert_eq!(
        session.expires_at,
        session.created_at + 7 * DAY,
        "a new session ends seven days after it began"
    );
    assert_eq!(
        session.last_used_at, session.created_at,
        "and counts as used when it began"
    );

    // A session that began long enough ago, used a moment ago.
    let user = th.client.me().await;
    let now = fvoci_server::auth::now_millis();
    let old_token = new_secret();
    let old = Session {
        id: new_id(),
        user_id: user.id,
        created_at: now - 7 * DAY - MINUTE,
        expires_at: now - MINUTE,
        ..Session::default()
    };
    let store = th.app.store();
    let password_hash = store
        .get_user_by_id(&old.user_id)
        .await
        .expect("the user is stored")
        .password_hash;
    let stored = store
        .create_session(
            &old,
            &fvoci_server::auth::secret::token_digest(&old_token),
            &password_hash,
        )
        .await
        .expect("the old session is stored");
    assert!(
        stored,
        "the password is the one the session is opened under"
    );
    store
        .touch_session(&old.id, now)
        .await
        .expect("the old session was just used");

    th.client_with_token(&old_token)
        .get("/users/me")
        .await
        .check_unauthorized();
    th.client.get("/users/me").await.check_ok();

    th.teardown().await;
}

/// When a session was last used is written down at most every ten minutes: requests in
/// between do not write to the database.
#[tokio::test]
async fn the_use_of_a_session_is_recorded_at_most_every_ten_minutes() {
    let th = Setup::native().start().await.init_basic().await;
    let session = stored_session(&th).await;
    let store = th.app.store();

    th.client.get("/users/me").await.check_ok();
    assert_eq!(
        stored_session(&th).await.last_used_at,
        session.last_used_at,
        "a use right after the last recorded one is not recorded"
    );

    let now = fvoci_server::auth::now_millis();
    let nine_minutes_ago = now - 9 * MINUTE;
    store
        .set_session_last_used(&session.id, nine_minutes_ago)
        .await
        .expect("the last use is moved back");
    th.client.get("/users/me").await.check_ok();
    assert_eq!(
        stored_session(&th).await.last_used_at,
        nine_minutes_ago,
        "nor is a use nine minutes after it"
    );

    store
        .set_session_last_used(&session.id, now - 11 * MINUTE)
        .await
        .expect("the last use is moved back further");
    th.client.get("/users/me").await.check_ok();
    let recorded = stored_session(&th).await.last_used_at;
    assert!(
        recorded >= now,
        "a use eleven minutes after it is: {recorded} against {now}"
    );

    th.teardown().await;
}

/// A password that is right, but stored with other hashing parameters than the current
/// ones, is hashed anew at login: the parameters can be raised without asking anyone to
/// set their password again.
#[tokio::test]
async fn a_login_renews_a_hash_made_with_other_parameters() {
    let th = Setup::native().start().await.init_basic().await;
    let store = th.app.store();
    let user = store
        .get_user_by_username(crate::common::USER1_USERNAME)
        .await
        .expect("the user is stored");

    // A hash as an earlier version with cheaper parameters would have stored it.
    let cheap = argon2_params(8, 1);
    let old_hash = fvoci_server::auth::password::hash_password_with(PASSWORD, cheap)
        .expect("a hash with other parameters");
    assert!(
        old_hash.starts_with("$argon2id$v=19$m=8,t=1,p=1$"),
        "{old_hash}"
    );
    assert!(
        store
            .replace_password_hash(&user.id, &user.password_hash, &old_hash)
            .await
            .expect("the old hash is planted"),
        "planted"
    );

    // A wrong password changes nothing.
    th.anonymous()
        .login(crate::common::USER1_USERNAME, "not the password")
        .await
        .check_unauthorized();
    let stored = store.get_user_by_id(&user.id).await.expect("the user");
    assert_eq!(
        stored.password_hash, old_hash,
        "a failed login renews nothing"
    );

    // The right one signs in and leaves a current hash behind.
    th.login(crate::common::USER1_USERNAME, PASSWORD).await;
    let stored = store.get_user_by_id(&user.id).await.expect("the user");
    assert!(
        stored
            .password_hash
            .starts_with("$argon2id$v=19$m=19456,t=2,p=1$"),
        "the hash was renewed with the current parameters: {}",
        stored.password_hash
    );
    // The session that was open stays: the password did not change.
    th.client.get("/users/me").await.check_ok();
    // And the password still is the password.
    th.login(crate::common::USER1_USERNAME, PASSWORD).await;
    let again = store.get_user_by_id(&user.id).await.expect("the user");
    assert_eq!(
        again.password_hash, stored.password_hash,
        "a current hash is left alone"
    );

    th.teardown().await;
}

/// Argon2 parameters with the given memory (KiB) and iterations.
fn argon2_params(memory_kib: u32, iterations: u32) -> argon2::Params {
    argon2::Params::new(memory_kib, iterations, 1, None).expect("valid parameters")
}

/// A user who changes their password signs every other device out: whoever else knew
/// the old password does not stay signed in. The session the change came through stays.
#[tokio::test]
async fn changing_the_password_ends_the_other_sessions_of_the_user() {
    let th = Setup::native().start().await.init_basic().await;
    let here = &th.client;
    let phone = th.login(crate::common::USER1_USERNAME, PASSWORD).await;
    let tablet = th.login(crate::common::USER1_USERNAME, PASSWORD).await;
    for client in [here, &phone, &tablet] {
        client.get("/users/me").await.check_ok();
    }

    // A refused change ends nothing.
    here.post(
        "/users/me/password",
        &json!({"oldPassword": "not the password", "newPassword": "another password"}),
    )
    .await
    .check_bad_request();
    phone.get("/users/me").await.check_ok();

    let changed = here
        .post(
            "/users/me/password",
            &json!({"oldPassword": PASSWORD, "newPassword": "another password"}),
        )
        .await;
    changed.check_no_content();

    // The session the change came through goes on, under a new token.
    let new_token = changed
        .session_cookie()
        .expect("the answer sets the cookie anew");
    assert_ne!(new_token, here.token, "a new token");
    here.get("/users/me").await.check_unauthorized();
    th.client_with_token(&new_token)
        .get("/users/me")
        .await
        .check_ok();
    for (client, device) in [(&phone, "phone"), (&tablet, "tablet")] {
        let response = client.get("/users/me").await;
        assert_eq!(
            response.status,
            StatusCode::UNAUTHORIZED,
            "the session on the {device} ended with the old password"
        );
    }
    // The other user is not signed out by it.
    th.client2.get("/users/me").await.check_ok();
    // The old password opens nothing any more; the new one does.
    th.anonymous()
        .login(crate::common::USER1_USERNAME, PASSWORD)
        .await
        .check_unauthorized();
    th.login(crate::common::USER1_USERNAME, "another password")
        .await;

    th.teardown().await;
}

/// A password is hashed whole, whatever its length up to the limit, and a password
/// longer than the limit is refused where it is set and wrong where it is tried.
#[tokio::test]
async fn passwords_are_bounded_by_characters_below_and_by_bytes_above() {
    let th = Setup::native().start().await.init_basic().await;
    let link = th.member_link().await;
    let register = |username: &str, password: &str| {
        let body = acceptance(
            &link,
            &json!({
                "username": username,
                "email": format!("{username}@sample.com"),
                "password": password,
            }),
        );
        let anonymous = th.anonymous();
        async move { anonymous.post("/invitations/accept", &body).await }
    };

    // Longer than the 72 bytes the previous hash looked at; every byte counts.
    let long = "p".repeat(200);
    register("long", &long).await.check_ok();
    th.login("long", &long).await;
    let mut other_end = "p".repeat(199);
    other_end.push('q');
    th.anonymous()
        .login("long", &other_end)
        .await
        .check_unauthorized();

    // The limits, where a password is set.
    let longest = "가".repeat(341);
    register("longest", &longest).await.check_ok();
    th.login("longest", &longest).await;
    for (username, password, message) in [
        (
            "toolong",
            "x".repeat(1025),
            "password must be at most 1024 bytes",
        ),
        (
            "short",
            "가나다라".to_owned(),
            "password must be at least 8 characters",
        ),
    ] {
        let response = register(username, &password).await;
        response.check_error(StatusCode::BAD_REQUEST, message);
    }

    // A password longer than the limit was never set: trying it is a wrong password
    // like any other, with the same answer.
    let response = th.anonymous().login("long", &"x".repeat(10_000)).await;
    response.check_error(StatusCode::UNAUTHORIZED, "incorrect login");
    assert_eq!(response.error_reason(), "invalid_credentials");

    th.teardown().await;
}

/// A user may set a display name at registration or via `patchMe`, clear it with an empty
/// name or `null`, and be found by search via display name as well as login name. At most 64
/// UTF-16 code units and no control characters.
#[tokio::test]
async fn a_user_has_a_display_name_of_their_choosing() {
    let th = Setup::native().start().await.init_basic().await;
    let link = th.member_link().await;
    let jim = json!({"username": "jim", "email": "jim@sample.com", "password": PASSWORD,
                     "displayName": "Jim Halpert"});
    accept(&th, &link, &jim).await.check_ok();
    let jim = th.login("jim", PASSWORD).await;
    assert_eq!(jim.me().await.display_name, "Jim Halpert");

    let me = th.client.me().await;
    assert_eq!(me.display_name, "", "none unless chosen");
    let changed: User = th
        .client
        .patch("/users/me", &json!({"customName": "Michael Scott 🎉"}))
        .await
        .ok_json();
    assert_eq!(changed.display_name, "Michael Scott 🎉");
    assert_eq!(th.client.me().await.display_name, "Michael Scott 🎉");

    // Found by either name, whatever the case.
    let search = |query: &str| format!("{ROOT_WS}/users?search={query}");
    let found: Vec<User> = jim.get(&search("scott")).await.ok_json();
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].id, me.id);
    assert_eq!(found[0].display_name, "Michael Scott 🎉", "others see it");
    let found: Vec<User> = th.client.get(&search("HALPERT")).await.ok_json();
    assert_eq!(found.len(), 1, "{found:?}");

    // The limits: 64 UTF-16 code units, no control characters, no other member.
    let longest = format!("{}🎉", "가".repeat(62));
    th.client
        .patch("/users/me", &json!({"customName": longest}))
        .await
        .check_ok();
    for (body, field) in [
        (
            json!({"customName": format!("{}🎉", "가".repeat(63))}),
            "customName",
        ),
        (json!({"customName": "two\nlines"}), "customName"),
        (json!({"customName": "a\u{0}b"}), "customName"),
        (json!({"customName": "\u{200B}"}), "customName"),
        (json!({"customName": "\u{202E}evil"}), "customName"),
        (json!({"customName": "line\u{2028}break"}), "customName"),
        (json!({"customName": 1}), "customName"),
        (json!({"username": "renamed"}), "username"),
    ] {
        let response = th.client.patch("/users/me", &body).await;
        response.check_bad_request();
        assert_eq!(
            response.refused_at(),
            format!("/{field}"),
            "{body}: {}",
            response.text()
        );
    }
    assert_eq!(
        th.client.me().await.display_name,
        longest,
        "nothing changed"
    );
    let pam = json!({"username": "pam", "email": "pam@sample.com", "password": PASSWORD,
                     "displayName": "x".repeat(65)});
    let response = accept(&th, &link, &pam).await;
    response.check_bad_request();
    assert_eq!(response.refused_at(), "/account/displayName");
    // Left out where the account is made: none. `null` is no name, and refused.
    let mut account = json!({"username": "dwight", "email": "dwight@sample.com",
                             "password": PASSWORD});
    accept(&th, &link, &account).await.check_ok();
    let made = th.login("dwight", PASSWORD).await.me().await;
    assert_eq!(made.display_name, "");
    account["username"] = json!("angela");
    account["email"] = json!("angela@sample.com");
    account["displayName"] = Value::Null;
    accept(&th, &link, &account).await.check_bad_request();

    // White space around a name goes; empty, blank or null: none.
    let trimmed: User = jim
        .patch("/users/me", &json!({"customName": "  Jim  "}))
        .await
        .ok_json();
    assert_eq!(trimmed.display_name, "Jim");
    for cleared in [json!(""), json!("   "), Value::Null] {
        let user: User = jim
            .patch("/users/me", &json!({"customName": cleared}))
            .await
            .ok_json();
        assert_eq!(user.display_name, "");
    }
    th.teardown().await;
}

/// Usernames containing `@`, ASCII/Unicode control characters, or more than 64 characters
/// are refused on registration and invitation acceptance so a username can never shadow
/// another user's email login.
#[tokio::test]
async fn username_rejects_at_sign_control_characters_and_overlong_names() {
    let th = Setup::native().start().await;
    for (bad_username, expected_reason) in [
        ("alice@example.com", "invalid_characters"),
        ("bob\0null", "invalid_characters"),
        (&"x".repeat(65), "too_long"),
    ] {
        let reg = th
            .anonymous()
            .post(
                "/register",
                &json!({
                    "username": bad_username,
                    "email": "founder@example.com",
                    "password": PASSWORD,
                    "organizationName": ORG_NAME,
                }),
            )
            .await;
        reg.check_bad_request();
        assert_eq!(reg.refused_at(), "/username", "{bad_username:?}");
        assert_eq!(reg.value()["errors"][0]["reason"], json!(expected_reason));
    }

    th.anonymous()
        .post(
            "/register",
            &json!({
                "username": "alice",
                "email": "alice@example.com",
                "password": PASSWORD,
                "organizationName": ORG_NAME,
            }),
        )
        .await
        .check_created();
    let link = th.member_link().await;

    for (bad_username, expected_reason) in [
        ("alice@example.com", "invalid_characters"),
        ("bob@corp.org", "invalid_characters"),
        ("bob\0null", "invalid_characters"),
        ("bob\nline", "invalid_characters"),
        ("\nalice\u{0085}", "invalid_characters"),
        ("\u{200B}spoof", "invalid_characters"),
        (&"x".repeat(65), "too_long"),
    ] {
        let resp = accept(
            &th,
            &link,
            &json!({
                "username": bad_username,
                "email": "attacker@example.com",
                "password": PASSWORD,
            }),
        )
        .await;
        resp.check_bad_request();
        assert_eq!(resp.refused_at(), "/account/username", "{bad_username:?}");
        assert_eq!(
            resp.value()["errors"][0]["reason"],
            json!(expected_reason),
            "{bad_username:?}: {}",
            resp.text()
        );
    }

    // Boundary 64-character username without `@` or control chars succeeds.
    let max_ok = "u".repeat(64);
    accept(
        &th,
        &link,
        &json!({
            "username": max_ok,
            "email": "valid64@example.com",
            "password": PASSWORD,
        }),
    )
    .await
    .check_ok();

    // Alice's email login (sending both username and email as the login input, matching
    // the web sign-in payload) remains completely unshadowed.
    let login_resp = th
        .anonymous()
        .post(
            "/login",
            &login_request("alice@example.com", "alice@example.com", PASSWORD),
        )
        .await;
    login_resp.check_no_content();
    let token = login_resp.session_cookie().expect("session cookie");
    let signed_in = crate::common::Client::new(&th.base_url, &token);
    assert_eq!(signed_in.me().await.username, "alice");
    th.teardown().await;
}
