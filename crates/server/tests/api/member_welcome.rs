//! The welcome board of somebody who comes into an organization as a member: one, a
//! private board of their own, given once to an account whatever it comes into after.
//! A guest is given none.

use fvoci_model::{
    Container, ContainerVisibility, Invitation, User, Workspace, WorkspaceRole, new_id,
};
use fvoci_server::app::Inviting;
use fvoci_store::{DbType, Store, Tenant};
use serde_json::json;

use crate::common::{Client, PASSWORD, ROOT_WORKSPACE_ID, ROOT_WS, Setup, TestServer};
use crate::fake_oidc::{FakeProvider, Person};
use crate::oidc::{Browser, org_of, set_provider, with_key_and_private_addresses};

/// The title of the built-in welcome board, in English and in Korean.
const WELCOME_TITLE: &str = "Getting started with fvoci";
const WELCOME_TITLE_KO: &str = "fvoci 시작하기";

/// A server that holds the built-in templates, with its first user and a second one
/// whom a link for members let in.
async fn welcoming() -> TestServer {
    Setup::native()
        .with_templates()
        .start()
        .await
        .init_basic()
        .await
}

/// The welcome boards `user_id` has among the rows `store` reaches: the identifier and
/// the title of each board they made that is titled as the welcome board is.
async fn welcome_boards(store: &Store, user_id: &str) -> Vec<(String, String)> {
    let found = store
        .texts_unchecked(&format!(
            "SELECT CAST(id AS TEXT) || '|' || title FROM containers \
             WHERE CAST(created_by AS TEXT) = '{user_id}' AND is_template = false \
             AND title IN ('{WELCOME_TITLE}', '{WELCOME_TITLE_KO}') ORDER BY created_at, id"
        ))
        .await
        .expect("the boards are read");
    found
        .into_iter()
        .map(|row| {
            let (id, title) = row.split_once('|').expect("an identifier and a title");
            (id.to_owned(), title.to_owned())
        })
        .collect()
}

/// Whom a link lets in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum For {
    Members,
    Guest,
}

/// A new link of the first user's into the root workspace: for members, or for a guest
/// of a new board of theirs.
async fn link(ts: &TestServer, whom: For) -> String {
    let request = if whom == For::Guest {
        let board: Container = ts
            .client
            .create_board(&Container {
                workspace_id: ROOT_WORKSPACE_ID.to_owned(),
                visibility: ContainerVisibility::Open,
                title: "for a guest".to_owned(),
                ..Container::default()
            })
            .await
            .created_json();
        json!({"role": "guest", "containerId": board.id, "containerRole": "viewer"})
    } else {
        json!({"role": "member"})
    };
    let made: Invitation = ts
        .client
        .post(&format!("{ROOT_WS}/invitations"), &request)
        .await
        .created_json();
    made.token.expect("a new link comes with its token")
}

/// Accepts the link `token` with a new account `username`, by a request that accepts
/// `language` (says nothing when empty), and signs in as it.
async fn newcomer(ts: &TestServer, token: &str, username: &str, language: &str) -> Client {
    let anonymous = ts.anonymous();
    let mut request = anonymous
        .request(reqwest::Method::POST, "/invitations/accept")
        .json(&json!({
            "token": token,
            "account": {
                "username": username,
                "email": format!("{username}@sample.com"),
                "password": PASSWORD,
            },
        }));
    if !language.is_empty() {
        request = request.header(reqwest::header::ACCEPT_LANGUAGE, language);
    }
    Client::send(request).await.check_ok();
    ts.login(username, PASSWORD).await
}

/// Somewhere else to come into, with a link for members into it: a workspace of
/// another organization where the database holds several, and another workspace of
/// the test's organization where it holds one (SQLite). Returns a handle that reaches
/// its rows, and the token of the link.
async fn elsewhere(ts: &TestServer) -> (Store, String) {
    let workspace = Workspace {
        id: new_id(),
        title: "elsewhere".to_owned(),
        ..Workspace::default()
    };
    let store = ts.app.store();
    let (app, theirs) = if store.db_type() == DbType::Postgres {
        let org_id = store
            .create_organization("elsewhere", &workspace)
            .await
            .expect("a second organization is made");
        (
            ts.app.scoped(Tenant::org(&org_id).expect("an identifier")),
            store.for_org(&org_id).expect("an identifier"),
        )
    } else {
        store
            .create_workspace(&workspace)
            .await
            .expect("a second workspace is made");
        (ts.app.clone(), store.clone())
    };
    let mut owner = User {
        id: new_id(),
        username: "owner.elsewhere".to_owned(),
        email: "owner.elsewhere@sample.com".to_owned(),
        ..User::default()
    };
    theirs
        .create_user(&mut owner)
        .await
        .expect("their user is stored");
    // An account is stored as a member of the default workspace of its organization,
    // which the workspace is where it is another organization's.
    let raised = theirs
        .execute_unchecked(&format!(
            "UPDATE workspace_members SET role = '{}' WHERE CAST(user_id AS TEXT) = '{}' \
             AND CAST(workspace_id AS TEXT) = '{}'",
            WorkspaceRole::Admin.as_str(),
            owner.id,
            workspace.id
        ))
        .await
        .expect("the membership is read");
    if raised == 0 {
        theirs
            .add_workspace_member(&workspace.id, &owner.id, WorkspaceRole::Admin)
            .await
            .expect("their user administers the workspace");
    }
    let token = app
        .invite(&workspace.id, &owner.id, Inviting::Members)
        .await
        .expect("a link for members is made")
        .token
        .expect("a new link comes with its token");
    (theirs, token)
}

/// Whoever a link for members lets in has one welcome board, in the workspace they
/// came into: a private board of their own without the template's marks, written in
/// the language their request accepts. Whoever started the organization asks for
/// theirs, and is given none by this.
#[tokio::test]
async fn a_member_let_in_by_a_link_has_a_welcome_board_of_their_own() {
    let ts = welcoming().await;
    let store = ts.app.store();
    let founder = ts.client.me().await.id;
    assert_eq!(
        welcome_boards(store, &founder).await.len(),
        1,
        "welcomed automatically on sign-up"
    );

    let member = ts.client2.me().await.id;
    let boards = welcome_boards(store, &member).await;
    let [(board_id, title)] = boards.as_slice() else {
        panic!("one welcome board: {boards:?}");
    };
    assert_eq!(title, WELCOME_TITLE, "nothing said of a language: English");
    let board: Container = ts
        .client2
        .get(&format!("{ROOT_WS}/containers/{board_id}"))
        .await
        .ok_json();
    assert_eq!(board.workspace_id, ROOT_WORKSPACE_ID, "where they came in");
    assert_eq!(board.visibility, ContainerVisibility::Private);
    assert!(!board.is_template && !board.is_builtin, "{board:?}");
    for mark in [
        "onboarding",
        "templateKey",
        "templateLanguage",
        "templateBaseDate",
    ] {
        assert_eq!(board.properties.get(mark), None, "the mark {mark}");
    }

    let token = link(&ts, For::Members).await;
    let korean = newcomer(&ts, &token, "hana", "ko-KR,ko;q=0.9,en;q=0.8").await;
    let titles = async |client: &Client| -> Vec<String> {
        welcome_boards(store, &client.me().await.id)
            .await
            .into_iter()
            .map(|(_, title)| title)
            .collect()
    };
    assert_eq!(
        titles(&korean).await,
        [WELCOME_TITLE_KO],
        "the request accepts Korean"
    );

    // The language of the screen the account was made on comes before the request's.
    let request = ts
        .anonymous()
        .request(reqwest::Method::POST, "/invitations/accept")
        .header(reqwest::header::ACCEPT_LANGUAGE, "en")
        .json(&json!({
            "token": token,
            "language": "ko",
            "account": {"username": "duri", "email": "duri@sample.com", "password": PASSWORD},
        }));
    Client::send(request).await.check_ok();
    let on_a_korean_screen = ts.login("duri", PASSWORD).await;
    assert_eq!(titles(&on_a_korean_screen).await, [WELCOME_TITLE_KO]);

    // A screen in a language the server writes nothing in leaves it to the request's.
    let request = ts
        .anonymous()
        .request(reqwest::Method::POST, "/invitations/accept")
        .header(reqwest::header::ACCEPT_LANGUAGE, "ko")
        .json(&json!({
            "token": token,
            "language": "ja",
            "account": {"username": "sesi", "email": "sesi@sample.com", "password": PASSWORD},
        }));
    Client::send(request).await.check_ok();
    let on_a_japanese_screen = ts.login("sesi", PASSWORD).await;
    assert_eq!(titles(&on_a_japanese_screen).await, [WELCOME_TITLE_KO]);
    ts.teardown().await;
}

/// A guest is given no welcome board; an account is a guest's until a link for members
/// lets it in, and is given its board then.
#[tokio::test]
async fn a_guest_has_no_welcome_board_until_they_come_in_as_a_member() {
    let ts = welcoming().await;
    let store = ts.app.store();
    let token = link(&ts, For::Guest).await;
    let guest = newcomer(&ts, &token, "guest", "").await;
    let guest_id = guest.me().await.id;
    assert_eq!(welcome_boards(store, &guest_id).await, []);

    let (theirs, token) = elsewhere(&ts).await;
    guest
        .post("/invitations/accept", &json!({"token": token}))
        .await
        .check_ok();
    assert_eq!(
        welcome_boards(&theirs, &guest_id).await.len(),
        1,
        "a member there"
    );
    ts.teardown().await;
}

/// An account is given its welcome board once: not again by a link it is let in by
/// again, not as it comes into another organization or workspace, and not after it
/// asked for one as whoever starts an organization does.
#[tokio::test]
async fn an_account_is_given_its_welcome_board_once() {
    let ts = welcoming().await;
    let store = ts.app.store();
    let member = ts.client2.me().await.id;
    let founder = ts.client.me().await.id;

    let before = [
        welcome_boards(store, &founder).await,
        welcome_boards(store, &member).await,
    ];
    assert_eq!(before.each_ref().map(Vec::len), [1, 1], "{before:?}");

    let again = link(&ts, For::Members).await;
    let (theirs, away) = elsewhere(&ts).await;
    for token in [&again, &away] {
        for client in [&ts.client, &ts.client2] {
            client
                .post("/invitations/accept", &json!({"token": token}))
                .await
                .check_ok();
        }
    }
    for (user_id, before) in [&founder, &member].into_iter().zip(before) {
        let mut after = welcome_boards(store, user_id).await;
        after.extend(welcome_boards(&theirs, user_id).await);
        after.dedup();
        assert_eq!(after, before, "no other for {user_id}");
    }
    ts.teardown().await;
}

/// Whoever signs in through the organization's provider for the first time comes into
/// its default workspace and has one welcome board there, in English where the browser
/// that comes back names no language; the same person signing in twice at once, and
/// again later, has no other.
#[tokio::test]
async fn a_newcomer_through_the_provider_has_one_welcome_board() {
    let fake = FakeProvider::start().await;
    let ts = Setup::native()
        .with_templates()
        .trusting_oidc_provider(&fake.certificate_der)
        .configure(with_key_and_private_addresses)
        .start()
        .await
        .init_basic()
        .await;
    let org_id = org_of(&ts).await;
    set_provider(&ts, &org_id, &fake).await;

    fake.sign_in_as(Person::new("subject-ada", "ada@example.com"));
    let browsers = [Browser::new(&ts, &fake), Browser::new(&ts, &fake)];
    let (first, second) = tokio::join!(browsers[0].sign_in(&org_id), browsers[1].sign_in(&org_id),);
    let ada = ts.client_with_token(first.token()).me().await.id;
    assert_eq!(ts.client_with_token(second.token()).me().await.id, ada);
    Browser::new(&ts, &fake).sign_in(&org_id).await.token();

    let boards = welcome_boards(ts.app.store(), &ada).await;
    let [(board_id, title)] = boards.as_slice() else {
        panic!("one welcome board: {boards:?}");
    };
    assert_eq!(title, WELCOME_TITLE);
    let board: Container = ts
        .client_with_token(first.token())
        .get(&format!("{ROOT_WS}/containers/{board_id}"))
        .await
        .ok_json();
    assert_eq!(
        board.workspace_id, ROOT_WORKSPACE_ID,
        "the default workspace"
    );
    assert_eq!(board.visibility, ContainerVisibility::Private);
    ts.teardown().await;
    fake.stop();
}

/// Coming in does not depend on the welcome board: where the server holds none the
/// link lets the account in all the same, and the account is given its board the next
/// time it comes in somewhere.
#[tokio::test]
async fn a_member_let_in_while_there_is_no_welcome_board_is_given_it_later() {
    let ts = welcoming().await;
    let store = ts.app.store();
    ts.system_store()
        .remove_builtin_templates()
        .await
        .expect("the built-in templates are removed");
    let token = link(&ts, For::Members).await;
    let member = newcomer(&ts, &token, "late", "").await;
    let member_id = member.me().await.id;
    assert_eq!(welcome_boards(store, &member_id).await, [], "let in");

    ts.app
        .init_templates()
        .await
        .expect("the built-in templates are stored again");
    let (theirs, away) = elsewhere(&ts).await;
    member
        .post("/invitations/accept", &json!({"token": away}))
        .await
        .check_ok();
    assert_eq!(welcome_boards(&theirs, &member_id).await.len(), 1);
    ts.teardown().await;
}
