//! A session that gives its account's password: it opens the account's own space from
//! then on, beside what it opened before and nothing else, until it ends.

use std::time::Duration;

use fvoci_model::new_id;
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::common::{Client, TestServer};
use crate::fake_oidc::{FakeProvider, Person};
use crate::first_password::{
    FIRST_PASSWORD, board_in_the_space, boards_of, card_in_the_space, check_asks_for_the_password,
    counted_in_the_space, login, told_of_the_space, told_on,
};
use crate::oidc::{Browser, org_of, server_for, set_provider};
use crate::own_space::workspaces_and_own;
use crate::rooms::Peer;
use crate::sso::{Passwords, admin_through_provider, require_sso};

const CONFIRM: &str = "/users/me/password/confirm";

/// A person the provider signs in under `name`.
fn person(name: &str) -> Person {
    let mut person = Person::new(&format!("subject-{name}"), &format!("{name}@example.com"));
    person.preferred_username = Some(name.to_owned());
    person
}

/// A session of an account made through the provider that then gave the account its
/// first password: one the account's own space is closed to.
async fn with_a_first_password(ts: &TestServer, browser: &Browser, org_id: &str) -> Client {
    let client = ts.client_with_token(browser.sign_in(org_id).await.token());
    let set = client
        .put("/users/me/password", &json!({"password": FIRST_PASSWORD}))
        .await;
    set.check_no_content();
    ts.client_with_token(&set.session_cookie().expect("the session's new cookie"))
}

/// Gives the password in the session, and answers the session under its new cookie.
async fn confirm(ts: &TestServer, client: &Client, password: &str) -> Client {
    let confirmed = client.post(CONFIRM, &json!({"password": password})).await;
    confirmed.check_no_content();
    let cookie = confirmed
        .session_cookie()
        .expect("the session's new cookie");
    // What the session opens changed: the value that opened less is good no more.
    client.get("/users/me").await.check_unauthorized();
    ts.client_with_token(&cookie)
}

/// Whether the session is told the organization takes only its provider's sessions.
async fn asks_for_the_provider(client: &Client, path: &str) -> bool {
    let answer = client.get(path).await;
    answer.status == StatusCode::UNAUTHORIZED && answer.error_reason() == "sso_required"
}

/// How many sessions of the account are stored as having confirmed its password.
async fn confirmed_sessions(ts: &TestServer, user_id: &str) -> usize {
    let query = format!(
        "SELECT CAST(id AS TEXT) FROM sessions WHERE user_id = '{user_id}' \
         AND password_confirmed_at <> 0"
    );
    let store = ts.app.store().system();
    store.texts_unchecked(&query).await.expect("read").len()
}

/// A session made through the organization's provider gives the account's password
/// once and from then on opens the account's own space and the organization that
/// takes only that provider's sessions, both: the routes, the list of the workspaces,
/// the list of the boards and what deleting the account comes to. It is that session
/// alone: another of the account stays as it was. A wrong password changes nothing,
/// and nobody confirms one without a session or for an account that has none.
#[tokio::test]
async fn a_session_that_gave_the_password_opens_the_space_beside_its_organization() {
    let fake = FakeProvider::start().await;
    let ts = server_for(&fake).await;
    let org_id = org_of(&ts).await;
    set_provider(&ts, &org_id, &fake).await;
    let browser = Browser::new(&ts, &fake);
    fake.sign_in_as(person("ada"));
    let client = with_a_first_password(&ts, &browser, &org_id).await;
    let user_id = client.me().await.id;
    let admin = admin_through_provider(&ts, &fake, &org_id).await;
    require_sso(&admin, &org_id, &fake, Passwords::Refused)
        .await
        .check_ok();
    fake.sign_in_as(person("ada"));

    let (listed, own) = workspaces_and_own(&client).await;
    assert!(own.password_required, "closed to the session: {own:?}");
    let own_path = format!("/w/{}", own.id);
    let org_path = format!("/w/{}", listed[0].id);
    let system = ts.app.store().system();
    let board = {
        // Past the routes: the session does not open the space yet.
        let board = board_in_the_space(&ts, &own, &user_id).await;
        check_asks_for_the_password(&client, &own_path).await;
        board
    };

    // A wrong password is the session's to try again, and nothing is recorded.
    let wrong = client
        .post(CONFIRM, &json!({"password": "a wrong guess"}))
        .await;
    wrong.check_error(StatusCode::BAD_REQUEST, "the password is wrong");
    assert_eq!(
        wrong.error_reason(),
        "invalid_credentials",
        "{}",
        wrong.text()
    );
    assert_eq!(wrong.refused_at(), "/password", "{}", wrong.text());
    assert_eq!(
        wrong.session_cookie(),
        None,
        "no new cookie: {}",
        wrong.text()
    );
    check_asks_for_the_password(&client, &own_path).await;
    assert_eq!(confirmed_sessions(&ts, &user_id).await, 0);
    ts.anonymous()
        .post(CONFIRM, &json!({"password": FIRST_PASSWORD}))
        .await
        .check_unauthorized();

    let later = ts.client_with_token(browser.sign_in(&org_id).await.token());
    let confirmed = confirm(&ts, &client, FIRST_PASSWORD).await;
    assert_eq!(confirmed_sessions(&ts, &user_id).await, 1);
    let (listed, opened) = workspaces_and_own(&confirmed).await;
    assert!(!opened.password_required, "opened: {opened:?}");
    assert!(
        listed.iter().all(|workspace| !workspace.sso_required),
        "and the organization with it: {listed:?}"
    );
    confirmed.get(&own_path).await.check_ok();
    confirmed
        .get(&format!("{own_path}/boards"))
        .await
        .check_ok();
    confirmed.get(&org_path).await.check_ok();
    assert!(boards_of(&confirmed).await.contains(&board), "listed");
    assert_eq!(counted_in_the_space(&confirmed).await, json!(1));

    // The other session of the account gave nothing, and is where it was.
    check_asks_for_the_password(&later, &own_path).await;
    later.get(&org_path).await.check_ok();
    assert!(!boards_of(&later).await.contains(&board), "left out");

    // A session made with the password opened the space already; giving the
    // password again opens nothing of an organization that takes none.
    let (status, by_password) = login(&ts, "ada", FIRST_PASSWORD).await;
    assert!(status.is_success(), "the password signs in: {status}");
    assert!(asks_for_the_provider(&by_password, &org_path).await);
    let by_password = confirm(&ts, &by_password, FIRST_PASSWORD).await;
    by_password.get(&own_path).await.check_ok();
    assert!(
        asks_for_the_provider(&by_password, &org_path).await,
        "a password given twice is no provider"
    );

    // Nor does it open the organization to a session made through another provider:
    // the one that confirmed, written down as another's.
    let another = new_id();
    let moved = system
        .execute_unchecked(&format!(
            "UPDATE sessions SET login_provider_id = '{another}' \
             WHERE user_id = '{user_id}' AND password_confirmed_at <> 0 \
             AND login_provider_id IS NOT NULL"
        ))
        .await;
    moved.expect("the session is another provider's");
    confirmed.get(&own_path).await.check_ok();
    assert!(
        asks_for_the_provider(&confirmed, &org_path).await,
        "what a session confirmed is no provider either"
    );

    // An account without a password has none to confirm.
    fake.sign_in_as(person("grace"));
    let without = ts.client_with_token(browser.sign_in(&org_id).await.token());
    without
        .post(CONFIRM, &json!({"password": FIRST_PASSWORD}))
        .await
        .check_status(StatusCode::CONFLICT);
    without.get("/users/me").await.check_ok();
    ts.teardown().await;
    fake.stop();
}

/// What a session has confirmed stands in for nothing where the password itself is
/// asked for: the account's address is changed by the password in that request, in a
/// session of the provider that has shown it as in one that has not. With it, the
/// account made through the provider, which has no address, is given its first.
#[tokio::test]
async fn the_address_is_changed_by_the_password_whatever_the_session_confirmed() {
    const ASK: &str = "/users/me/email-change";
    let fake = FakeProvider::start().await;
    let ts = server_for(&fake).await;
    let org_id = org_of(&ts).await;
    set_provider(&ts, &org_id, &fake).await;
    let browser = Browser::new(&ts, &fake);
    fake.sign_in_as(person("ada"));
    let unconfirmed = with_a_first_password(&ts, &browser, &org_id).await;
    let refused = async |client: &Client, what: &str| {
        let without = client
            .post(ASK, &json!({"email": "ada@home.example"}))
            .await;
        assert!(
            matches!(
                without.status,
                StatusCode::BAD_REQUEST | StatusCode::UNPROCESSABLE_ENTITY
            ),
            "{what}, no password: {} {}",
            without.status,
            without.text()
        );
        let wrong = client
            .post(
                ASK,
                &json!({"email": "ada@home.example", "password": "a wrong guess"}),
            )
            .await;
        wrong.check_status(StatusCode::BAD_REQUEST);
        assert_eq!(wrong.error_reason(), "invalid_credentials", "{what}");
        let me: fvoci_model::Me = client.get("/users/me").await.ok_json();
        assert_eq!((me.email.as_str(), me.pending_email), ("", None), "{what}");
    };
    refused(&unconfirmed, "a session that has not shown the password").await;
    let confirmed = confirm(&ts, &unconfirmed, FIRST_PASSWORD).await;
    refused(&confirmed, "a session that has shown the password").await;

    // The server of this test sends no mail: the password changes the address at once.
    confirmed
        .post(
            ASK,
            &json!({"email": "ada@home.example", "password": FIRST_PASSWORD}),
        )
        .await
        .check_no_content();
    let me: fvoci_model::Me = confirmed.get("/users/me").await.ok_json();
    assert_eq!(
        (me.email.as_str(), me.pending_email),
        ("ada@home.example", None)
    );
    ts.teardown().await;
    fake.stop();
}

/// What a session confirmed goes with the session, and a change of the password ends
/// the account's other sessions: the one that changed it keeps what it confirmed, the
/// others are gone with theirs, and after the operator's reset no session of the
/// account is left that confirmed the password before it.
#[tokio::test]
async fn what_a_session_confirmed_ends_with_a_change_of_the_password() {
    let fake = FakeProvider::start().await;
    let ts = server_for(&fake).await;
    let org_id = org_of(&ts).await;
    set_provider(&ts, &org_id, &fake).await;
    let browser = Browser::new(&ts, &fake);
    fake.sign_in_as(person("ada"));
    let first = with_a_first_password(&ts, &browser, &org_id).await;
    let user_id = first.me().await.id;
    let (_, own) = workspaces_and_own(&first).await;
    let own_path = format!("/w/{}", own.id);
    let second = ts.client_with_token(browser.sign_in(&org_id).await.token());
    let first = confirm(&ts, &first, FIRST_PASSWORD).await;
    let second = confirm(&ts, &second, FIRST_PASSWORD).await;
    assert_eq!(confirmed_sessions(&ts, &user_id).await, 2);

    let changed = first
        .post(
            "/users/me/password",
            &json!({"oldPassword": FIRST_PASSWORD, "newPassword": "another Pa$$word"}),
        )
        .await;
    changed.check_no_content();
    let first = ts.client_with_token(&changed.session_cookie().expect("its new cookie"));
    first.get(&own_path).await.check_ok();
    second.get("/users/me").await.check_unauthorized();
    assert_eq!(
        confirmed_sessions(&ts, &user_id).await,
        1,
        "the other session went, with what it confirmed"
    );

    let reset = ts.app.reset_user_password("ada", "a third Pa$$word").await;
    reset.expect("the operator sets the password");
    first.get("/users/me").await.check_unauthorized();
    assert_eq!(confirmed_sessions(&ts, &user_id).await, 0);
    let after = ts.client_with_token(browser.sign_in(&org_id).await.token());
    check_asks_for_the_password(&after, &own_path).await;
    // The old password confirms nothing any more.
    after
        .post(CONFIRM, &json!({"password": FIRST_PASSWORD}))
        .await
        .check_bad_request();
    let after = confirm(&ts, &after, "a third Pa$$word").await;
    after.get(&own_path).await.check_ok();
    ts.teardown().await;
    fake.stop();
}

/// Whether the connection was closed by the server, having read what was on its way.
async fn closed(peer: &mut Peer) -> bool {
    while peer.step(Duration::from_secs(5)).await {}
    peer.unread().contains(&json!({"type": "closed"}))
}

/// The server of the tests, looking at the session of each connection every second.
async fn server_looking_at_sessions(fake: &FakeProvider) -> TestServer {
    crate::common::Setup::native()
        .trusting_oidc_provider(&fake.certificate_der)
        .configure(|config| {
            crate::oidc::with_key_and_private_addresses(config);
            config.websocket_ping_seconds = 1;
            config.skip_websocket_pings = false;
        })
        .start()
        .await
        .init_basic()
        .await
}

/// The connections of a session are asked as the session is. One of a provider's
/// session follows nothing of the space and is refused its rooms; as the session gives
/// the password its connections stay open, though the session goes on under another
/// token and each connection looks at its session every second: the same connection
/// then follows the space and comes into the room of its card. Another session's
/// connection is told nothing still. When the password is changed elsewhere the
/// session ends, and its connection in the room is closed with it.
#[tokio::test]
async fn a_connection_follows_the_space_once_its_session_gave_the_password() {
    let fake = FakeProvider::start().await;
    let ts = server_looking_at_sessions(&fake).await;
    let org_id = org_of(&ts).await;
    set_provider(&ts, &org_id, &fake).await;
    let browser = Browser::new(&ts, &fake);
    fake.sign_in_as(person("ada"));
    let client = with_a_first_password(&ts, &browser, &org_id).await;
    let other = ts.client_with_token(browser.sign_in(&org_id).await.token());
    let user_id = client.me().await.id;
    let (_, own) = workspaces_and_own(&client).await;
    let board = board_in_the_space(&ts, &own, &user_id).await;
    let card = card_in_the_space(&ts, &own, &board, &user_id).await;
    let at = (&own, board.as_str());

    let mut following = client.connect_websocket().await;
    assert!(!told_on(&ts, &mut following, &client, at).await, "before");
    let mut within = Peer::connect(&client, 311).await;
    let refused: Value = within.join(&own.id, &card).await;
    assert_eq!(refused["type"], json!("docRefused"), "{refused}");
    assert_eq!(refused["reason"], json!("not_found"), "{refused}");

    let client = confirm(&ts, &client, FIRST_PASSWORD).await;
    // Longer than two looks at the session, whose token of the handshakes is good no
    // more: the connections go on under the one the session has now.
    let looked_twice = tokio::time::Instant::now() + Duration::from_millis(2500);
    while tokio::time::Instant::now() < looked_twice && !closed_by_now(&within) {
        within.step(Duration::from_millis(200)).await;
    }
    assert!(
        !within.unread().contains(&json!({"type": "closed"})),
        "the connection of the session stays open: {:?}",
        within.unread()
    );
    assert!(
        told_on(&ts, &mut following, &client, at).await,
        "the connection that was told nothing follows the space where it is"
    );
    within.joined(&own.id, &card).await;
    assert!(
        !told_of_the_space(&ts, &other, at).await,
        "a session that gave nothing is told nothing"
    );
    let mut others = Peer::connect(&other, 313).await;
    let refused: Value = others.join(&own.id, &card).await;
    assert_eq!(refused["type"], json!("docRefused"), "{refused}");

    // The password is changed from another session: this one ends, and what it
    // confirmed with it.
    let (status, elsewhere) = login(&ts, "ada", FIRST_PASSWORD).await;
    assert!(status.is_success(), "the password signs in: {status}");
    elsewhere
        .post(
            "/users/me/password",
            &json!({"oldPassword": FIRST_PASSWORD, "newPassword": "another Pa$$word"}),
        )
        .await
        .check_no_content();
    assert!(
        closed(&mut within).await,
        "the connection in the room ends with its session: {:?}",
        within.unread()
    );
    client.get("/users/me").await.check_unauthorized();
    ts.teardown().await;
    fake.stop();
}

/// Whether the peer has read that its connection closed.
fn closed_by_now(peer: &Peer) -> bool {
    peer.unread().contains(&json!({"type": "closed"}))
}

/// The connections of the session that sets the account's first password, and then
/// changes it, stay through both: the session goes on under another token each time,
/// and each connection looks at its session every second under the one it has now.
#[tokio::test]
async fn the_connection_of_the_session_that_sets_or_changes_the_password_stays() {
    let fake = FakeProvider::start().await;
    let ts = server_looking_at_sessions(&fake).await;
    let org_id = org_of(&ts).await;
    set_provider(&ts, &org_id, &fake).await;
    let browser = Browser::new(&ts, &fake);
    fake.sign_in_as(person("ada"));
    let client = ts.client_with_token(browser.sign_in(&org_id).await.token());
    let mut peer = Peer::connect(&client, 321).await;

    let set = client
        .put("/users/me/password", &json!({"password": FIRST_PASSWORD}))
        .await;
    set.check_no_content();
    let client = ts.client_with_token(&set.session_cookie().expect("the session's new cookie"));
    let changed = client
        .post(
            "/users/me/password",
            &json!({"oldPassword": FIRST_PASSWORD, "newPassword": "another Pa$$word"}),
        )
        .await;
    changed.check_no_content();
    let looked_twice = tokio::time::Instant::now() + Duration::from_millis(2500);
    while tokio::time::Instant::now() < looked_twice && !closed_by_now(&peer) {
        peer.step(Duration::from_millis(200)).await;
    }
    assert!(
        !peer.unread().contains(&json!({"type": "closed"})),
        "the connection of the session stays open: {:?}",
        peer.unread()
    );
    peer.barrier(&ts).await;
    ts.teardown().await;
    fake.stop();
}
