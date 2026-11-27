//! The first password of an account that has none: one made through a provider is
//! told so, sets one from its session once, and signs in with it from then on, into
//! its own space and into whatever takes a password.

use fvoci_model::{
    Block, BlockType, Container, ContainerMember, ContainerVisibility, UserWorkspace, new_id,
};
use fvoci_server::app::ChangeNotification;
use fvoci_store::testing::db_type_of_the_tests;
use fvoci_store::{DbType, Tenant};
use std::time::Duration;

use reqwest::StatusCode;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

use crate::common::{Client, PASSWORD, Setup, TestServer, carried_out};
use crate::fake_oidc::{FakeProvider, Person};
use crate::oidc::{Browser, org_of, server_for, set_provider, with_key_and_private_addresses};
use crate::own_space::workspaces_and_own;
use crate::rooms::Peer;
use crate::sso::{Passwords, admin_through_provider, require_sso};

pub(crate) const FIRST_PASSWORD: &str = "a first Pa$$word";

/// Whether the account of the client is told it has a password.
async fn has_password(client: &Client) -> Value {
    let me: Value = client.get("/users/me").await.ok_json();
    me["hasPassword"].clone()
}

/// A board in the user's own space that they administer, and its identifier.
pub(crate) async fn board_in_the_space(
    ts: &TestServer,
    own: &UserWorkspace,
    user_id: &str,
) -> String {
    let mut board = Container {
        id: new_id(),
        workspace_id: own.id.clone(),
        visibility: ContainerVisibility::Private,
        title: "a board of the space".to_owned(),
        ..Container::default()
    };
    let tenant = Tenant::org(&own.org_id).expect("an identifier");
    ts.app
        .scoped(tenant)
        .store()
        .insert_container_with_admin(&mut board, user_id)
        .await
        .expect("a board of the space");
    board.id
}

/// The boards the session's user is listed as a member of, in every organization.
pub(crate) async fn boards_of(client: &Client) -> Vec<String> {
    let members: Vec<ContainerMember> = client.get("/users/me/memberships").await.ok_json();
    members
        .into_iter()
        .map(|member| member.container_id)
        .collect()
}

/// How many boards the session is told its account's own space holds, where it asks
/// what deleting the account comes to: nothing for a session the space is closed to.
pub(crate) async fn counted_in_the_space(client: &Client) -> Value {
    let deletion: Value = client.get("/users/me/deletion").await.ok_json();
    let space = &deletion["organizations"][0];
    assert_eq!(space["kind"], json!("personal"), "{deletion}");
    space["boards"].clone()
}

/// Signs in with the name and the password, and answers what the server said.
pub(crate) async fn login(ts: &TestServer, username: &str, password: &str) -> (StatusCode, Client) {
    let mut client = ts.anonymous();
    let answer = client.login(username, password).await;
    (answer.status, client)
}

/// An account that a provider's sign-in made has no password and is told so. A
/// session of its made through the provider just now, and no older one, sets the
/// first one without being asked for an old one, once: the other
/// sessions of the account end, this one goes on under its new cookie, and a second
/// first password is refused. The password then signs in, and that session opens the
/// account's own space; an organization that takes only its provider's sessions is not
/// opened by it, and says so, while the own space is.
#[tokio::test]
async fn an_account_made_through_a_provider_sets_its_first_password_once() {
    let fake = FakeProvider::start().await;
    let ts = server_for(&fake).await;
    let org_id = org_of(&ts).await;
    set_provider(&ts, &org_id, &fake).await;
    let browser = Browser::new(&ts, &fake);
    let mut person = Person::new("subject-ada", "ada@example.com");
    person.preferred_username = Some("ada".to_owned());
    fake.sign_in_as(person);
    let client = ts.client_with_token(browser.sign_in(&org_id).await.token());
    let elsewhere = ts.client_with_token(browser.sign_in(&org_id).await.token());
    assert_eq!(has_password(&client).await, json!(false));
    assert_eq!(
        has_password(&ts.client).await,
        json!(true),
        "an account that registered with one"
    );
    let (refused, _) = login(&ts, "ada", FIRST_PASSWORD).await;
    assert_eq!(
        refused,
        StatusCode::UNAUTHORIZED,
        "no password signs in yet"
    );

    // A password is checked as any new one is, and nobody sets one without a session.
    let short = client
        .put("/users/me/password", &json!({"password": "short"}))
        .await;
    short.check_bad_request();
    assert_eq!(short.refused_at(), "/password", "{}", short.text());
    ts.anonymous()
        .put("/users/me/password", &json!({"password": FIRST_PASSWORD}))
        .await
        .check_unauthorized();
    // An account that has a password changes it by the old one, not here.
    ts.client
        .put("/users/me/password", &json!({"password": FIRST_PASSWORD}))
        .await
        .check_status(StatusCode::CONFLICT);
    assert_eq!(has_password(&client).await, json!(false), "nothing was set");

    // A session made a while ago vouches for nothing: who holds one signs in through
    // the provider again, and sets the password from the session that comes of it.
    let ada = client.me().await.id;
    ts.app
        .store()
        .execute_unchecked(&format!(
            "UPDATE sessions SET created_at = created_at - 600000 WHERE user_id = '{ada}'"
        ))
        .await
        .expect("the sessions are aged");
    let stale = client
        .put("/users/me/password", &json!({"password": FIRST_PASSWORD}))
        .await;
    stale.check_forbidden();
    assert_eq!(stale.error_reason(), "sign_in_again", "{}", stale.text());
    assert_eq!(has_password(&client).await, json!(false), "nothing was set");
    let before = client;
    let client = ts.client_with_token(browser.sign_in(&org_id).await.token());

    let set = client
        .put("/users/me/password", &json!({"password": FIRST_PASSWORD}))
        .await;
    set.check_no_content();
    let cookie = set.session_cookie().expect("the session's new cookie");
    client.get("/users/me").await.check_unauthorized();
    elsewhere.get("/users/me").await.check_unauthorized();
    before.get("/users/me").await.check_unauthorized();
    let client = ts.client_with_token(&cookie);
    assert_eq!(has_password(&client).await, json!(true));
    client
        .put(
            "/users/me/password",
            &json!({"password": "another Pa$$word"}),
        )
        .await
        .check_status(StatusCode::CONFLICT);

    // The password signs in, in any letter case of the name, and opens what the
    // provider's session opened.
    let (status, by_password) = login(&ts, "ADA", FIRST_PASSWORD).await;
    assert!(status.is_success(), "the password signs in: {status}");
    let (listed, own) = workspaces_and_own(&by_password).await;
    assert!(!listed.is_empty(), "a member of the organization");
    let own_path = format!("/w/{}", own.id);
    let org_path = format!("/w/{}", listed[0].id);
    by_password.get(&own_path).await.check_ok();
    by_password.get(&org_path).await.check_ok();

    // The organization takes only its provider's sessions from now on: the password
    // opens the account's own space still, and not the organization.
    let admin = admin_through_provider(&ts, &fake, &org_id).await;
    require_sso(&admin, &org_id, &fake, Passwords::Refused)
        .await
        .check_ok();
    let (status, by_password) = login(&ts, "ada", FIRST_PASSWORD).await;
    assert!(status.is_success(), "the password signs in: {status}");
    by_password.get(&own_path).await.check_ok();
    let closed = by_password.get(&org_path).await;
    assert_eq!(closed.error_reason(), "sso_required", "{}", closed.text());
    let (listed, own_after) = workspaces_and_own(&by_password).await;
    assert!(listed.iter().all(|workspace| workspace.sso_required));
    assert_eq!(own_after, own, "their own space asks for no provider");
    ts.teardown().await;
    fake.stop();
}

/// What a session is told is closed to it for want of the account's password.
pub(crate) async fn check_asks_for_the_password(client: &Client, path: &str) {
    let refused = client.get(path).await;
    refused.check_unauthorized();
    assert_eq!(
        refused.error_reason(),
        "password_sign_in_required",
        "{path}: {}",
        refused.text()
    );
}

/// The account's own space takes any session of it until it has a password, and from
/// then on only one made with the password: the provider's session that set it is
/// told so by the routes of the space, by the list of the workspaces, by the list of
/// the boards and by what deleting the account comes to, and goes on opening the
/// organization. A session made with the password opens both.
#[tokio::test]
async fn a_space_takes_the_password_alone_once_its_account_has_one() {
    let fake = FakeProvider::start().await;
    let ts = server_for(&fake).await;
    let org_id = org_of(&ts).await;
    set_provider(&ts, &org_id, &fake).await;
    let browser = Browser::new(&ts, &fake);
    let mut person = Person::new("subject-ada", "ada@example.com");
    person.preferred_username = Some("ada".to_owned());
    fake.sign_in_as(person);
    let client = ts.client_with_token(browser.sign_in(&org_id).await.token());

    // Without a password the space takes the provider's session.
    let (listed, own) = workspaces_and_own(&client).await;
    assert!(!own.password_required, "no password to ask for: {own:?}");
    let own_path = format!("/w/{}", own.id);
    let org_path = format!("/w/{}", listed[0].id);
    client.get(&own_path).await.check_ok();
    let board = board_in_the_space(&ts, &own, &client.me().await.id).await;
    assert!(boards_of(&client).await.contains(&board), "listed");
    assert_eq!(counted_in_the_space(&client).await, json!(1));

    let set = client
        .put("/users/me/password", &json!({"password": FIRST_PASSWORD}))
        .await;
    set.check_no_content();
    let cookie = set.session_cookie().expect("the session's new cookie");
    let client = ts.client_with_token(&cookie);

    // The session that set it was made through the provider, and is told wherever
    // the space is that it takes the password now.
    let (_, closed) = workspaces_and_own(&client).await;
    assert!(closed.password_required, "{closed:?}");
    assert!(!closed.sso_required, "no provider is asked for: {closed:?}");
    check_asks_for_the_password(&client, &own_path).await;
    check_asks_for_the_password(&client, &format!("{own_path}/boards")).await;
    for suffix in ["/archive/export", "/boards/search?q=board", "/audit"] {
        check_asks_for_the_password(&client, &format!("{own_path}{suffix}")).await;
    }
    assert!(!boards_of(&client).await.contains(&board), "left out");
    assert_eq!(counted_in_the_space(&client).await, Value::Null);
    client.get(&org_path).await.check_ok();
    // And so is a session made through the provider afterwards.
    let later = ts.client_with_token(browser.sign_in(&org_id).await.token());
    check_asks_for_the_password(&later, &own_path).await;
    later.get(&org_path).await.check_ok();

    let (status, by_password) = login(&ts, "ada", FIRST_PASSWORD).await;
    assert!(status.is_success(), "the password signs in: {status}");
    let (_, opened) = workspaces_and_own(&by_password).await;
    assert!(!opened.password_required, "by the password: {opened:?}");
    by_password.get(&own_path).await.check_ok();
    by_password.get(&org_path).await.check_ok();
    assert!(boards_of(&by_password).await.contains(&board), "listed");
    assert_eq!(counted_in_the_space(&by_password).await, json!(1));
    ts.teardown().await;
    fake.stop();
}

/// A card on a board of the user's own space, past the routes, and its identifier.
pub(crate) async fn card_in_the_space(
    ts: &TestServer,
    own: &UserWorkspace,
    board: &str,
    by: &str,
) -> String {
    let mut card = Block {
        id: new_id(),
        container_id: board.to_owned(),
        block_type: BlockType::Card,
        title: "a card of the space".to_owned(),
        ..Block::default()
    };
    let tenant = Tenant::org(&own.org_id).expect("an identifier");
    ts.app
        .scoped(tenant)
        .insert_block(&mut card, by, ChangeNotification::Suppress)
        .await
        .expect("a card of the space");
    card.id
}

/// The room of a card of the space is the space's: a connection of the provider's
/// session is in it while the account has no password, is put out of it as the first
/// password is set, staying open, and is refused the room from then on as one that
/// asks for a card that is not there. A connection of the password's session comes in.
#[tokio::test]
async fn the_room_of_a_card_of_a_space_takes_the_password_alone_once_there_is_one() {
    let fake = FakeProvider::start().await;
    let ts = server_for(&fake).await;
    let org_id = org_of(&ts).await;
    set_provider(&ts, &org_id, &fake).await;
    let browser = Browser::new(&ts, &fake);
    let mut person = Person::new("subject-ada", "ada@example.com");
    person.preferred_username = Some("ada".to_owned());
    fake.sign_in_as(person);
    let client = ts.client_with_token(browser.sign_in(&org_id).await.token());
    let (_, own) = workspaces_and_own(&client).await;
    let user_id = client.me().await.id;
    let board = board_in_the_space(&ts, &own, &user_id).await;
    let card = card_in_the_space(&ts, &own, &board, &user_id).await;

    let mut before = Peer::connect(&client, 301).await;
    before.joined(&own.id, &card).await;

    let set = client
        .put("/users/me/password", &json!({"password": FIRST_PASSWORD}))
        .await;
    set.check_no_content();
    let cookie = set.session_cookie().expect("the session's new cookie");
    let client = ts.client_with_token(&cookie);
    while before.step(Duration::from_millis(500)).await {}
    let told = before.unread();
    assert!(
        told.iter()
            .any(|message| message["type"] == json!("docLeft")),
        "the connection that was in the room is put out of it: {told:?}"
    );
    assert!(
        !told.contains(&json!({"type": "closed"})),
        "and stays open, as the session that set the password goes on: {told:?}"
    );
    let refused = before.join(&own.id, &card).await;
    assert_eq!(refused["type"], json!("docRefused"), "{refused}");

    let mut by_provider = Peer::connect(&client, 302).await;
    let refused = by_provider.join(&own.id, &card).await;
    assert_eq!(refused["type"], json!("docRefused"), "{refused}");
    assert_eq!(refused["reason"], json!("not_found"), "{refused}");

    let (status, by_password) = login(&ts, "ada", FIRST_PASSWORD).await;
    assert!(status.is_success(), "the password signs in: {status}");
    let mut with_password = Peer::connect(&by_password, 303).await;
    with_password.joined(&own.id, &card).await;
    ts.teardown().await;
    fake.stop();
}

/// Whether a connection of the client's session that follows the workspace of the
/// space is told of a card made on its board.
pub(crate) async fn told_of_the_space(
    ts: &TestServer,
    client: &Client,
    at: (&UserWorkspace, &str),
) -> bool {
    let mut socket = client.connect_websocket().await;
    told_on(ts, &mut socket, client, at).await
}

/// Whether the connection `socket`, of the client's session, is told of a card made
/// on the board of the space once it asks to follow the space's workspace.
pub(crate) async fn told_on(
    ts: &TestServer,
    socket: &mut crate::common::WebSocket,
    client: &Client,
    at: (&UserWorkspace, &str),
) -> bool {
    use futures_util::{SinkExt as _, StreamExt as _};

    let (own, board) = at;
    let follow = json!({"type": "subscribeWorkspace", "workspaceId": own.id});
    let follow = Message::Text(follow.to_string().into());
    socket.send(follow).await.expect("the command is sent");
    carried_out(ts.app.ws(), socket).await;
    let by = client.me().await.id;
    let card = card_in_the_space(ts, own, board, &by).await;
    assert!(
        ts.app.finish_background(Duration::from_secs(5)).await,
        "the change is told"
    );
    let mut told = false;
    while let Ok(Some(Ok(message))) =
        tokio::time::timeout(Duration::from_millis(300), socket.next()).await
    {
        if let Message::Text(text) = message {
            told |= text.contains(&card);
        }
    }
    told
}

/// How what a connection followed ended: the reason of the `workspaceUnsubscribed`
/// it is told, `closed` if the server closed it, or `nothing`.
async fn unsubscribed_or_closed(socket: &mut crate::common::WebSocket) -> String {
    use futures_util::StreamExt as _;

    loop {
        match tokio::time::timeout(Duration::from_secs(2), socket.next()).await {
            Ok(Some(Ok(Message::Text(text)))) => {
                let message: Value = serde_json::from_str(&text).expect("a message");
                if message["type"] == json!("workspaceUnsubscribed") {
                    return message["reason"].as_str().unwrap_or_default().to_owned();
                }
            }
            Ok(Some(Ok(Message::Close(_)) | Err(_)) | None) => return "closed".to_owned(),
            Ok(Some(Ok(_))) => {}
            Err(_) => return "nothing".to_owned(),
        }
    }
}

/// What the server answered a connection that followed a space is not kept past the
/// account's first password, whoever set it: its person from a session, or the
/// server's operator. A connection of a provider's session is told of the space
/// before, and one made after is told nothing; the password's session is told. The
/// connection that followed the space as its own session set the password stays open
/// and is told that it follows the space no more.
#[tokio::test]
async fn a_first_password_ends_what_a_provider_session_is_told_of_the_space() {
    let fake = FakeProvider::start().await;
    let ts = server_for(&fake).await;
    let org_id = org_of(&ts).await;
    set_provider(&ts, &org_id, &fake).await;
    let browser = Browser::new(&ts, &fake);
    for (name, by_operator) in [("ada", false), ("grace", true)] {
        let mut person = Person::new(&format!("subject-{name}"), &format!("{name}@example.com"));
        person.preferred_username = Some(name.to_owned());
        fake.sign_in_as(person);
        let client = ts.client_with_token(browser.sign_in(&org_id).await.token());
        let (_, own) = workspaces_and_own(&client).await;
        let board = board_in_the_space(&ts, &own, &client.me().await.id).await;
        let at = (&own, board.as_str());
        let mut followed = client.connect_websocket().await;
        assert!(
            told_on(&ts, &mut followed, &client, at).await,
            "{name}: before"
        );

        // Nothing is written between the password and the next connection but the
        // password: the session that set it goes on, and the operator ends every one.
        let later = if by_operator {
            let set = ts.app.reset_user_password(name, FIRST_PASSWORD).await;
            set.expect("the operator sets the password");
            ts.client_with_token(browser.sign_in(&org_id).await.token())
        } else {
            let password = json!({"password": FIRST_PASSWORD});
            let set = client.put("/users/me/password", &password).await;
            set.check_no_content();
            ts.client_with_token(&set.session_cookie().expect("the session's new cookie"))
        };
        let ended = unsubscribed_or_closed(&mut followed).await;
        let want = if by_operator {
            "closed"
        } else {
            "password sign-in required"
        };
        assert_eq!(
            ended, want,
            "{name}: the connection that followed the space"
        );
        assert!(
            !told_of_the_space(&ts, &later, at).await,
            "{name}: a provider's session is told nothing of the space"
        );
        let (status, by_password) = login(&ts, name, FIRST_PASSWORD).await;
        assert!(status.is_success(), "the password signs in: {status}");
        assert!(
            told_of_the_space(&ts, &by_password, at).await,
            "{name}: after"
        );
    }
    ts.teardown().await;
    fake.stop();
}

fn open(config: &mut fvoci_server::config::Configuration) {
    "open".clone_into(&mut config.org_creation);
}

/// An organization that takes only its provider's sessions asks the provider of who
/// has a place in it, and of nobody else. An account that registers without an
/// organization on the same server signs in with its password and opens its own
/// space, which asks for no provider; a member's password opens their own space and
/// not the organization.
#[tokio::test]
async fn no_provider_is_asked_of_an_account_in_no_organization() {
    if db_type_of_the_tests() != DbType::Postgres {
        return;
    }
    let fake = FakeProvider::start().await;
    let ts = Setup::native()
        .trusting_oidc_provider(&fake.certificate_der)
        .configure(with_key_and_private_addresses)
        .configure(open)
        .start()
        .await
        .init_basic()
        .await;
    let org_id = org_of(&ts).await;
    set_provider(&ts, &org_id, &fake).await;
    let admin = admin_through_provider(&ts, &fake, &org_id).await;
    require_sso(&admin, &org_id, &fake, Passwords::Refused)
        .await
        .check_ok();

    let alone = json!({
        "username": "carol", "email": "carol@sample.com", "password": PASSWORD,
    });
    ts.anonymous()
        .post("/register", &alone)
        .await
        .check_created();
    let (status, carol) = login(&ts, "carol", PASSWORD).await;
    assert!(status.is_success(), "the password signs in: {status}");
    let (listed, own) = workspaces_and_own(&carol).await;
    assert_eq!(listed, [], "in no organization");
    assert!(!own.sso_required, "their own space asks for no provider");
    carol.get(&format!("/w/{}", own.id)).await.check_ok();

    // The member's password session: the organization is closed to it and says why.
    let member = ts.client2.me().await;
    let (status, by_password) = login(&ts, &member.username, PASSWORD).await;
    assert!(status.is_success(), "the password signs in: {status}");
    let (listed, own) = workspaces_and_own(&by_password).await;
    assert!(listed.iter().all(|workspace| workspace.sso_required));
    let closed = by_password.get(&format!("/w/{}", listed[0].id)).await;
    assert_eq!(closed.error_reason(), "sso_required", "{}", closed.text());
    by_password.get(&format!("/w/{}", own.id)).await.check_ok();

    let store = ts.app.store();
    let asked = async |user_id: &str| {
        store
            .required_provider_for(user_id, &org_id)
            .await
            .expect("read")
    };
    assert!(asked(&member.id).await.is_some(), "asked of a member");
    assert_eq!(
        asked(&carol.me().await.id).await,
        None,
        "and not of an account that has no place in the organization"
    );
    ts.teardown().await;
    fake.stop();
}
