//! A registration that waits for its address is no account: no list or search of people
//! names it, the identifier its account will have is nobody's wherever a person is
//! named by one, and its address is no account's where one is looked up. Confirmed, it
//! is found by all of them.

use std::time::Duration;

use fvoci_model::{Container, ContainerVisibility, Invitation};
use fvoci_store::{Signup, SignupLookup};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

use crate::common::{ApiResponse, Client, Setup, TestServer};
use crate::email_verification::{SIGNUP_PASSWORD, complete_elsewhere, link_token, registration};
use crate::fake_smtp::FakeSmtp;

/// The registration with the user name, which waits.
async fn waiting(th: &TestServer, username: &str) -> Signup {
    th.app
        .store()
        .live_signup(
            SignupLookup::Username(username),
            fvoci_server::auth::now_millis(),
        )
        .await
        .expect("read")
        .unwrap_or_else(|| panic!("{username} waits"))
}

/// Accepts the link of `invitation` with a new account `username`, from a screen in
/// Korean; it waits.
async fn accept(th: &TestServer, invitation: &Invitation, username: &str) {
    th.anonymous()
        .post(
            "/invitations/accept",
            &json!({"token": invitation.token, "language": "ko", "account": {
                "username": username,
                "email": format!("{username}@unseen.test"),
                "password": SIGNUP_PASSWORD,
            }}),
        )
        .await
        .check_status(StatusCode::ACCEPTED);
}

/// Where `ada` looks people up.
#[derive(Clone, Copy)]
struct Places<'a> {
    workspace: &'a str,
    org: &'a str,
    board: &'a str,
}

impl Places<'_> {
    fn users(self) -> String {
        format!("/w/{}/users", self.workspace)
    }

    /// The lists that name people, by name or by identifier: who can be seen in the
    /// workspace, its members, the members of the board, the organization's members
    /// and workspaces, and the records of both.
    fn lists(self) -> Vec<String> {
        let Self {
            workspace,
            org,
            board,
        } = self;
        vec![
            self.users(),
            format!("/w/{workspace}/members"),
            format!("/w/{workspace}/containers/{board}/members"),
            format!("/w/{workspace}/audit"),
            format!("/w/{workspace}/invitations"),
            format!("/org/{org}/members?perPage=200"),
            format!("/org/{org}/workspaces"),
            format!("/org/{org}/audit"),
        ]
    }

    /// The requests that name a person by `id`, and what each sends.
    fn by_identifier(self, id: &str) -> Vec<(Method, String, Value)> {
        let Self {
            workspace,
            org,
            board,
        } = self;
        let on_board = format!("/w/{workspace}/containers/{board}/members");
        vec![
            (Method::GET, format!("{}/{id}", self.users()), Value::Null),
            (
                Method::POST,
                on_board.clone(),
                json!({"userId": id, "role": "viewer"}),
            ),
            (
                Method::PUT,
                format!("{on_board}/{id}"),
                json!({"role": "editor"}),
            ),
            (Method::DELETE, format!("{on_board}/{id}"), Value::Null),
            (
                Method::PUT,
                format!("/w/{workspace}/members/{id}"),
                json!({"role": "member"}),
            ),
            (
                Method::DELETE,
                format!("/w/{workspace}/members/{id}"),
                Value::Null,
            ),
            (
                Method::PUT,
                format!("/org/{org}/members/{id}/role"),
                json!({"role": "admin"}),
            ),
            (
                Method::PUT,
                format!("/users/{id}/admin"),
                json!({"isAdmin": true}),
            ),
        ]
    }
}

/// Sends one of [`Places::by_identifier`].
async fn send(ada: &Client, (method, path, body): (Method, String, Value)) -> ApiResponse {
    let mut request = ada.request(method, &path);
    if !body.is_null() {
        request = request.json(&body);
    }
    Client::send(request).await
}

/// Every list of people names nothing of the registrations: not their names, not the
/// identifiers their accounts will have. (No list carries an address.)
async fn lists_name_nothing(ada: &Client, places: Places<'_>, hidden: &[Signup]) {
    for path in places.lists() {
        let answer = ada.get(&path).await;
        answer.check_status(StatusCode::OK);
        let text = answer.text().to_lowercase();
        for signup in hidden {
            for part in [&signup.username, &signup.id] {
                assert!(
                    !text.contains(&part.to_lowercase()),
                    "{path} names {part} of a registration: {text}"
                );
            }
        }
    }
}

/// By the identifier its account will have, a registration is nobody: every request
/// that names a person by one is answered as for an identifier of nobody (refused,
/// or, where taking nobody off is no error, with nothing).
async fn identifiers_are_nobodys(ada: &Client, places: Places<'_>, hidden: &[Signup]) {
    let nobody = fvoci_model::new_id();
    let ids: Vec<&str> = hidden.iter().map(|signup| signup.id.as_str()).collect();
    let by_ids = ada.post(&places.users(), &ids).await;
    by_ids.check_status(StatusCode::OK);
    assert_eq!(by_ids.value(), json!([]), "by their identifiers");
    for id in ids {
        let asked = places.by_identifier(id);
        for (of_it, of_nobody) in asked.into_iter().zip(places.by_identifier(&nobody)) {
            let what = format!("{} {}", of_it.0, of_it.1);
            let (it, none) = (send(ada, of_it).await, send(ada, of_nobody).await);
            assert_eq!(it.status, none.status, "{what}: {}", it.text());
            assert_eq!(
                refusal(&it.text().replace(id, &nobody)),
                refusal(&none.text()),
                "{what}: answered as for nobody"
            );
        }
    }
}

/// What an answer says, without what names the one request it answers; an answer
/// without a body says nothing.
fn refusal(text: &str) -> Value {
    let mut said: Value = serde_json::from_str(text).unwrap_or_default();
    if let Some(members) = said.as_object_mut() {
        members.remove("instance");
        members.remove("requestId");
    }
    said
}

/// The mail of an invitation is written in the language of the account that has the
/// recipient's address, if one has: a registration in Korean that waits at the address
/// is none, and the mail is in the sender's language.
async fn its_address_is_no_accounts(
    th: &TestServer,
    smtp: &FakeSmtp,
    ada: &Client,
    (workspace_id, link): (&str, &Invitation),
    signup: &Signup,
) {
    ada.post(
        &format!("/w/{workspace_id}/invitations/mail"),
        &json!({"token": link.token, "recipients": [signup.email], "language": "en"}),
    )
    .await
    .check_no_content();
    assert!(
        th.app.finish_background(Duration::from_secs(10)).await,
        "the mail is done with"
    );
    let messages = smtp.messages().await;
    let invitation = messages
        .iter()
        .rev()
        .find(|message| message.recipient == signup.email && message.decoded.contains("/invite/"))
        .expect("the invitation's mail");
    assert!(
        invitation.decoded.contains("lang=\"en\""),
        "in the sender's language: {}",
        invitation.decoded
    );
}

/// A server that confirms addresses, its first user `ada`, who confirmed hers, a board
/// of hers, and the registrations that wait: by a link for members (the first), by a
/// link for a guest of the board, and for an organization of its own where the server
/// takes another (SQLite holds one organization).
struct Scene {
    th: TestServer,
    smtp: FakeSmtp,
    ada: Client,
    workspace_id: String,
    board: Container,
    for_members: Invitation,
    hidden: Vec<Signup>,
}

impl Scene {
    async fn start() -> Self {
        let several = fvoci_store::testing::db_type_of_the_tests() == fvoci_store::DbType::Postgres;
        let smtp = FakeSmtp::start_on(0).await;
        let mut setup = Setup::native()
            .smtp(smtp.settings())
            .configure(|config| config.mail.email_verification = true);
        if several {
            setup = setup.configure(|config| {
                fvoci_server::config::ORG_CREATION_OPEN.clone_into(&mut config.org_creation);
            });
        }
        let th = setup.start().await;
        th.anonymous()
            .post("/register", &registration("ada", "en"))
            .await
            .check_status(StatusCode::ACCEPTED);
        let token = link_token(&smtp.received(1).await[0]);
        let confirmed = complete_elsewhere(&th, &token, SIGNUP_PASSWORD).await;
        confirmed.check_status(StatusCode::OK);
        let ada = th.client_with_token(&confirmed.session_cookie().expect("a session"));
        let workspace_id = confirmed.value()["workspaceId"]
            .as_str()
            .expect("her workspace")
            .to_owned();
        let board: Container = ada
            .create_board(&Container {
                workspace_id: workspace_id.clone(),
                visibility: ContainerVisibility::Open,
                ..Container::default()
            })
            .await
            .created_json();

        let mut usernames = vec!["unseen-member", "unseen-guest"];
        if several {
            let mut own = registration("unseen-own", "en");
            own["email"] = json!("unseen-own@unseen.test");
            th.anonymous()
                .post("/register", &own)
                .await
                .check_status(StatusCode::ACCEPTED);
            usernames.push("unseen-own");
        }
        let links = format!("/w/{workspace_id}/invitations");
        let for_members: Invitation = ada
            .post(&links, &json!({"role": "member"}))
            .await
            .created_json();
        accept(&th, &for_members, "unseen-member").await;
        let for_a_guest: Invitation = ada
            .post(
                &links,
                &json!({"role": "guest", "containerId": board.id, "containerRole": "viewer"}),
            )
            .await
            .created_json();
        accept(&th, &for_a_guest, "unseen-guest").await;
        let mut hidden = Vec::new();
        for username in usernames {
            hidden.push(waiting(&th, username).await);
        }
        Self {
            th,
            smtp,
            ada,
            workspace_id,
            board,
            for_members,
            hidden,
        }
    }
}

#[tokio::test]
async fn a_registration_that_waits_is_in_no_lookup_of_people() {
    let Scene {
        th,
        smtp,
        ada,
        workspace_id,
        board,
        for_members,
        hidden,
    } = Scene::start().await;
    let me = ada.me().await;
    let org_id = th
        .app
        .store()
        .resolve_workspace(&me.id, &workspace_id)
        .await
        .expect("read")
        .expect("her workspace")
        .org_id;
    let places = Places {
        workspace: &workspace_id,
        org: &org_id,
        board: &board.id,
    };
    let found = async |typed: &str| {
        let found = ada.get(&format!("{}?search={typed}", places.users())).await;
        found.check_status(StatusCode::OK);
        found.value()
    };
    lists_name_nothing(&ada, places, &hidden).await;
    // Found by what is typed of a name: an account is, a registration is not.
    assert_eq!(
        found("ad").await[0]["username"],
        "ada",
        "an account is found"
    );
    for typed in ["unseen", "unseen-member", "UNSEEN-GUEST", "seen-"] {
        assert_eq!(found(typed).await, json!([]), "searching {typed}");
    }
    let by_id = ada.post(&places.users(), &[&me.id]).await;
    assert_eq!(
        by_id.value()[0]["username"],
        "ada",
        "an account is, by its identifier"
    );
    identifiers_are_nobodys(&ada, places, &hidden).await;
    its_address_is_no_accounts(&th, &smtp, &ada, (&workspace_id, &for_members), &hidden[0]).await;
    // Still nowhere after those were asked.
    lists_name_nothing(&ada, places, &hidden).await;

    // Confirmed, the one that came by the link for members is a person like another.
    let member = &hidden[0];
    assert!(
        th.app.finish_background(Duration::from_secs(10)).await,
        "the mail is done with"
    );
    let messages = smtp.messages().await;
    let mail = messages
        .iter()
        .find(|message| {
            message.recipient == member.email && message.decoded.contains("/verify-email#")
        })
        .expect("its confirmation mail");
    complete_elsewhere(&th, &link_token(mail), SIGNUP_PASSWORD)
        .await
        .check_status(StatusCode::OK);
    assert_eq!(
        found("unseen").await[0]["id"],
        json!(member.id),
        "found now"
    );
    let by_id = ada.post(&places.users(), &[&member.id]).await;
    assert_eq!(
        by_id.value()[0]["username"],
        "unseen-member",
        "by its identifier"
    );
    let members = ada.get(&format!("/w/{workspace_id}/members")).await;
    assert!(members.text().contains(&member.id), "{}", members.text());
    th.teardown().await;
    smtp.stop().await;
}
