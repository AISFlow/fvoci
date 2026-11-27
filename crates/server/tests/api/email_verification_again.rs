//! A second registration from the browser of one that waits replaces it: whoever
//! mistyped an address registers again at once under the same name, and the use the
//! first took of a link is given back. Without the browser's marker the name and the
//! address of a registration that waits are taken, as before.

use std::time::Duration;

use fvoci_model::{Container, ContainerVisibility, Invitation};
use fvoci_store::SignupLookup;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

use crate::common::{ApiResponse, Client, SESSION_COOKIE, Setup, TestServer};
use crate::email_verification::{
    SIGNUP_COOKIE, SIGNUP_PASSWORD, complete_elsewhere, complete_with_marker, link_token,
    marker_of, registration,
};
use crate::fake_smtp::FakeSmtp;

/// A server that confirms addresses; one that takes several organizations where the
/// database holds them.
async fn verifying(smtp: &FakeSmtp) -> TestServer {
    let mut setup = Setup::native()
        .smtp(smtp.settings())
        .configure(|config| config.mail.email_verification = true);
    if several() {
        setup = setup.configure(|config| {
            fvoci_server::config::ORG_CREATION_OPEN.clone_into(&mut config.org_creation);
        });
    }
    setup.start().await
}

/// Whether the server of the tests takes several organizations: SQLite holds one.
fn several() -> bool {
    fvoci_store::testing::db_type_of_the_tests() == fvoci_store::DbType::Postgres
}

/// An organization of the server, as its founder works in it.
struct Org {
    founder: Client,
    workspace_id: String,
    id: String,
}

impl Org {
    /// Registers `username`, confirms the address, and finds what was started.
    async fn found(th: &TestServer, smtp: &FakeSmtp, username: &str) -> Self {
        post(th, "/register", &registration(username, "en"), None)
            .await
            .check_status(StatusCode::ACCEPTED);
        let token = newest_link(th, smtp, &format!("{username}@example.test")).await;
        let confirmed = complete_elsewhere(th, &token, SIGNUP_PASSWORD).await;
        confirmed.check_status(StatusCode::OK);
        let founder = th.client_with_token(&confirmed.session_cookie().expect("a session"));
        let workspace_id = confirmed.value()["workspaceId"]
            .as_str()
            .expect("the workspace")
            .to_owned();
        let org_id = th
            .app
            .store()
            .resolve_workspace(&founder.me().await.id, &workspace_id)
            .await
            .expect("read")
            .expect("the founder's workspace")
            .org_id;
        Self {
            founder,
            workspace_id,
            id: org_id,
        }
    }

    /// A new link for members.
    async fn member_link(&self) -> Invitation {
        self.founder
            .post(
                &format!("/w/{}/invitations", self.workspace_id),
                &json!({"role": "member"}),
            )
            .await
            .created_json()
    }

    /// A new link for one guest of a new board.
    async fn guest_link(&self) -> Invitation {
        let board: Container = self
            .founder
            .create_board(&Container {
                workspace_id: self.workspace_id.clone(),
                visibility: ContainerVisibility::Open,
                ..Container::default()
            })
            .await
            .created_json();
        self.founder
            .post(
                &format!("/w/{}/invitations", self.workspace_id),
                &json!({"role": "guest", "containerId": board.id, "containerRole": "viewer"}),
            )
            .await
            .created_json()
    }

    /// How often the link counts as used.
    async fn uses(&self, th: &TestServer, link: &Invitation) -> i64 {
        th.app
            .store()
            .for_org(&self.id)
            .expect("the organization")
            .invitation_by_id(&link.id)
            .await
            .expect("read")
            .expect("the link")
            .uses
    }
}

/// Posts `body` as a browser that keeps `marker` of a registration it made, if any.
async fn post(th: &TestServer, path: &str, body: &Value, marker: Option<&str>) -> ApiResponse {
    let mut request = th.anonymous().request(Method::POST, path).json(body);
    if let Some(marker) = marker {
        request = request.header("Cookie", format!("{SIGNUP_COOKIE}={marker}"));
    }
    Client::send(request).await
}

/// Accepts `link` with a new account `username` at `email`.
async fn accept(
    th: &TestServer,
    link: &Invitation,
    (username, email): (&str, &str),
    marker: Option<&str>,
) -> ApiResponse {
    let body = json!({"token": link.token, "account": {
        "username": username,
        "email": email,
        "password": SIGNUP_PASSWORD,
    }});
    post(th, "/invitations/accept", &body, marker).await
}

/// The token of the newest confirmation link mailed to `recipient`.
async fn newest_link(th: &TestServer, smtp: &FakeSmtp, recipient: &str) -> String {
    assert!(
        th.app.finish_background(Duration::from_secs(10)).await,
        "the mail is done with"
    );
    let messages = smtp.messages().await;
    let message = messages
        .iter()
        .rev()
        .find(|message| {
            message.recipient == recipient && message.decoded.contains("/verify-email#")
        })
        .unwrap_or_else(|| panic!("a confirmation mail to {recipient}"));
    link_token(message)
}

/// Whether a registration with the user name waits, and at which address.
async fn waiting_at(th: &TestServer, username: &str) -> Option<String> {
    th.app
        .store()
        .live_signup(
            SignupLookup::Username(username),
            fvoci_server::auth::now_millis(),
        )
        .await
        .expect("read")
        .map(|signup| signup.email)
}

#[tokio::test]
async fn registering_again_from_the_same_browser_replaces_the_registration_that_waits() {
    let smtp = FakeSmtp::start_on(0).await;
    let th = verifying(&smtp).await;
    let mut mistyped = registration("mina", "en");
    mistyped["email"] = json!("mian@example.test");
    let first = post(&th, "/register", &mistyped, None).await;
    first.check_status(StatusCode::ACCEPTED);
    let marker = marker_of(&first).expect("a marker");
    let first_link = newest_link(&th, &smtp, "mian@example.test").await;

    // Another browser, with no marker or another registration's: the name and the
    // address are taken, as an account's are, and nothing is replaced.
    let others = post(&th, "/register", &registration("otto", "en"), None).await;
    others.check_status(StatusCode::ACCEPTED);
    let others_marker = marker_of(&others).expect("a marker");
    for stranger in [None, Some(others_marker.as_str()), Some("no-marker")] {
        let taken = post(&th, "/register", &registration("mina", "en"), stranger).await;
        taken.check_status(StatusCode::CONFLICT);
        assert_eq!(taken.error_reason(), "username_taken", "{stranger:?}");
        let mut same_address = registration("nobody", "en");
        same_address["email"] = json!("MIAN@example.test");
        let taken = post(&th, "/register", &same_address, stranger).await;
        taken.check_status(StatusCode::CONFLICT);
        assert_eq!(taken.error_reason(), "email_taken", "{stranger:?}");
    }
    assert_eq!(
        waiting_at(&th, "otto").await.as_deref(),
        Some("otto@example.test"),
        "the other browser's registration is as it was"
    );

    // A replacement that is refused replaces nothing: the first waits still.
    let mut anothers_name = registration("otto", "en");
    anothers_name["email"] = json!("mina@example.test");
    post(&th, "/register", &anothers_name, Some(&marker))
        .await
        .check_status(StatusCode::CONFLICT);
    assert_eq!(
        waiting_at(&th, "mina").await.as_deref(),
        Some("mian@example.test")
    );

    // The same browser, the same name, the address put right: taken at once.
    let second = post(&th, "/register", &registration("mina", "en"), Some(&marker)).await;
    second.check_status(StatusCode::ACCEPTED);
    let new_marker = marker_of(&second).expect("a marker of its own");
    assert_ne!(new_marker, marker, "each registration has its own");
    assert_eq!(
        waiting_at(&th, "mina").await.as_deref(),
        Some("mina@example.test")
    );
    // The first is gone: its link confirms nothing, and its marker replaces nothing.
    let ended = complete_with_marker(&th, &first_link, &marker).await;
    ended.check_status(StatusCode::BAD_REQUEST);
    assert_eq!(ended.refused_at(), "/token", "an ended link");
    post(&th, "/register", &registration("mina", "en"), Some(&marker))
        .await
        .check_status(StatusCode::CONFLICT);

    let link = newest_link(&th, &smtp, "mina@example.test").await;
    complete_with_marker(&th, &link, &new_marker)
        .await
        .check_status(StatusCode::OK);
    th.teardown().await;
    smtp.stop().await;
}

#[tokio::test]
async fn accepting_again_gives_back_the_use_the_first_registration_took() {
    let smtp = FakeSmtp::start_on(0).await;
    let th = verifying(&smtp).await;
    let org = Org::found(&th, &smtp, "ada").await;

    // A link for members counts the person once, however often they register.
    let members = org.member_link().await;
    let first = accept(&th, &members, ("nell", "nlel@example.test"), None).await;
    first.check_status(StatusCode::ACCEPTED);
    let marker = marker_of(&first).expect("a marker");
    assert_eq!(org.uses(&th, &members).await, 1, "counted from the start");
    accept(&th, &members, ("nell", "nell@example.test"), None)
        .await
        .check_status(StatusCode::CONFLICT);
    let second = accept(&th, &members, ("nell", "nell@example.test"), Some(&marker)).await;
    second.check_status(StatusCode::ACCEPTED);
    assert_eq!(org.uses(&th, &members).await, 1, "one person, one use");
    // By another link of the organization: the first link has its use back.
    let others = org.member_link().await;
    let marker = marker_of(&second).expect("a marker");
    let third = accept(&th, &others, ("nell", "nell@example.test"), Some(&marker)).await;
    third.check_status(StatusCode::ACCEPTED);
    let nells = marker_of(&third).expect("a marker");
    assert_eq!(org.uses(&th, &members).await, 0, "given back");
    assert_eq!(org.uses(&th, &others).await, 1, "taken where it went");
    assert_eq!(
        waiting_at(&th, "nell").await.as_deref(),
        Some("nell@example.test")
    );

    // A link for one guest takes the same person again, and nobody else meanwhile.
    let guests = org.guest_link().await;
    let preview = json!({"token": guests.token});
    let first = accept(&th, &guests, ("gwen", "gewn@example.test"), None).await;
    first.check_status(StatusCode::ACCEPTED);
    let marker = marker_of(&first).expect("a marker");
    for stranger in [None, Some("no-marker")] {
        post(&th, "/invitations/preview", &preview, stranger)
            .await
            .check_status(StatusCode::NOT_FOUND);
        accept(&th, &guests, ("gary", "gary@example.test"), stranger)
            .await
            .check_status(StatusCode::NOT_FOUND);
    }
    post(&th, "/invitations/preview", &preview, Some(&marker))
        .await
        .check_status(StatusCode::OK);
    // Who is signed in accepts as their account, which replaces nothing: not shown.
    let signed_in = org
        .founder
        .request(Method::POST, "/invitations/preview")
        .header("Cookie", format!("{SIGNUP_COOKIE}={marker}"))
        .json(&preview);
    Client::send(signed_in)
        .await
        .check_status(StatusCode::NOT_FOUND);
    // So is who sends the session in the cookie beside the marker, as a browser does.
    let session = &org.founder.token;
    let both = format!("{SESSION_COOKIE}={session}; {SIGNUP_COOKIE}={marker}");
    let in_browser = th
        .anonymous()
        .request(Method::POST, "/invitations/preview")
        .header("Cookie", both)
        .json(&preview);
    Client::send(in_browser)
        .await
        .check_status(StatusCode::NOT_FOUND);
    // Another registration's browser frees nothing of this link, and keeps its own.
    accept(&th, &guests, ("gary", "gary@example.test"), Some(&nells))
        .await
        .check_status(StatusCode::NOT_FOUND);
    assert_eq!(
        waiting_at(&th, "nell").await.as_deref(),
        Some("nell@example.test"),
        "a replacement that is refused replaces nothing"
    );
    // Refused for a name that is an account's: the first registration and its use stay.
    accept(&th, &guests, ("ada", "gwen@example.test"), Some(&marker))
        .await
        .check_status(StatusCode::CONFLICT);
    assert_eq!(
        waiting_at(&th, "gwen").await.as_deref(),
        Some("gewn@example.test")
    );
    assert_eq!(org.uses(&th, &guests).await, 1, "still the first's");

    let second = accept(&th, &guests, ("gwen", "gwen@example.test"), Some(&marker)).await;
    second.check_status(StatusCode::ACCEPTED);
    let marker = marker_of(&second).expect("a marker of its own");
    assert_eq!(
        org.uses(&th, &guests).await,
        1,
        "given back, and taken again"
    );
    let link = newest_link(&th, &smtp, "gwen@example.test").await;
    let confirmed = complete_with_marker(&th, &link, &marker).await;
    confirmed.check_status(StatusCode::OK);
    assert_eq!(confirmed.value()["containerId"], json!(guests.container_id));
    assert_eq!(
        org.uses(&th, &guests).await,
        1,
        "used by the guest it let in"
    );
    th.teardown().await;
    smtp.stop().await;
}

/// Two requests at once from the browser of a registration by a link for one guest:
/// one replaces it, and the link is not taken twice.
#[tokio::test]
async fn two_replacements_at_once_take_a_link_for_one_guest_once() {
    let smtp = FakeSmtp::start_on(0).await;
    let th = verifying(&smtp).await;
    let org = Org::found(&th, &smtp, "ada").await;
    for round in 0..4 {
        let guests = org.guest_link().await;
        let name = |who: &str| format!("{who}{round}");
        let at = |who: &str| format!("{who}{round}@example.test");
        let first = accept(&th, &guests, (&name("pip"), &at("pip")), None).await;
        first.check_status(StatusCode::ACCEPTED);
        let marker = marker_of(&first).expect("a marker");
        let (one, two) = (name("one"), name("two"));
        let (at_one, at_two) = (at("one"), at("two"));
        let (first, second) = tokio::join!(
            accept(&th, &guests, (&one, &at_one), Some(&marker)),
            accept(&th, &guests, (&two, &at_two), Some(&marker)),
        );
        let mut statuses = [first.status, second.status];
        statuses.sort();
        assert_eq!(
            statuses,
            [StatusCode::ACCEPTED, StatusCode::NOT_FOUND],
            "round {round}: {} / {}",
            first.text(),
            second.text()
        );
        assert_eq!(org.uses(&th, &guests).await, 1, "round {round}");
        assert_eq!(waiting_at(&th, &name("pip")).await, None, "replaced");
    }
    th.teardown().await;
    smtp.stop().await;
}

/// The registration that is replaced may be of another kind than what replaces it:
/// made by a link and replaced by a registration of its own, by a link of another
/// organization, or by an account at the address a guest link was mailed to, which
/// waits for nothing. The use goes back to the link wherever it is.
#[tokio::test]
async fn a_replaced_registration_gives_its_use_back_wherever_its_link_is() {
    let smtp = FakeSmtp::start_on(0).await;
    let th = verifying(&smtp).await;
    let org = Org::found(&th, &smtp, "ada").await;

    // Mailed to rae: registered by it with another address, then with hers.
    let guests = org.guest_link().await;
    org.founder
        .post(
            &format!("/w/{}/invitations/mail", org.workspace_id),
            &json!({"token": guests.token, "recipients": ["rae@example.test"]}),
        )
        .await
        .check_no_content();
    assert!(
        th.app.finish_background(Duration::from_secs(10)).await,
        "the mail is done with"
    );
    let messages = smtp.messages().await;
    let mailed = messages
        .iter()
        .rev()
        .find(|message| message.recipient == "rae@example.test")
        .and_then(|message| {
            message
                .decoded
                .lines()
                .find(|line| line.contains("/invite/"))
        })
        .expect("her link");
    let child = Invitation {
        token: Some(
            mailed
                .rsplit_once("/invite/")
                .expect("a token")
                .1
                .trim()
                .to_owned(),
        ),
        ..guests.clone()
    };
    let first = accept(&th, &child, ("rae", "rea@example.test"), None).await;
    first.check_status(StatusCode::ACCEPTED);
    let marker = marker_of(&first).expect("a marker");
    let joined = accept(&th, &child, ("rae", "Rae@example.test"), Some(&marker)).await;
    joined.check_status(StatusCode::OK);
    assert_eq!(marker_of(&joined), None, "nothing waits");
    assert_eq!(
        waiting_at(&th, "rae").await,
        None,
        "replaced by the account"
    );
    th.anonymous()
        .login("rae", SIGNUP_PASSWORD)
        .await
        .check_no_content();
    // Her link is used: once, by her.
    post(
        &th,
        "/invitations/preview",
        &json!({"token": child.token}),
        None,
    )
    .await
    .check_status(StatusCode::NOT_FOUND);

    if several() {
        across_organizations(&th, &smtp, &org).await;
    }
    th.teardown().await;
    smtp.stop().await;
}

/// On a server of several organizations: a registration by a link of one organization
/// is replaced by one by a link of another, and by one of its own; the first
/// organization's link has its use back each time.
async fn across_organizations(th: &TestServer, smtp: &FakeSmtp, org: &Org) {
    let other = Org::found(th, smtp, "bea").await;
    let guests = org.guest_link().await;
    let free = async || {
        post(
            th,
            "/invitations/preview",
            &json!({"token": guests.token}),
            None,
        )
        .await
        .status
            == StatusCode::OK
    };

    let first = accept(th, &guests, ("xan", "xna@example.test"), None).await;
    first.check_status(StatusCode::ACCEPTED);
    let marker = marker_of(&first).expect("a marker");
    assert!(!free().await, "taken by the registration");
    let members = other.member_link().await;
    let second = accept(th, &members, ("xan", "xan@example.test"), Some(&marker)).await;
    second.check_status(StatusCode::ACCEPTED);
    assert_eq!(org.uses(th, &guests).await, 0, "given back across the wall");
    assert!(free().await, "open to its guest again");
    assert_eq!(other.uses(th, &members).await, 1, "and taken where it went");

    // And back, to a registration of its own: the other organization's link is free.
    let marker = marker_of(&second).expect("a marker");
    let mut own = registration("xan", "en");
    own["email"] = json!("xan@example.test");
    let third = post(th, "/register", &own, Some(&marker)).await;
    third.check_status(StatusCode::ACCEPTED);
    assert_eq!(other.uses(th, &members).await, 0, "given back");
    let link = newest_link(th, smtp, "xan@example.test").await;
    let confirmed = complete_with_marker(th, &link, &marker_of(&third).expect("a marker")).await;
    confirmed.check_status(StatusCode::OK);
    assert_eq!(confirmed.value()["organizationStarted"], true, "its own");
}

/// Previews the link of `token` as a browser that sends a session and a registration's marker, while
/// the table of the sessions is out of the store's reach: a lookup of the session fails.
async fn preview_without_sessions(th: &TestServer, token: &str, session: &str) -> ApiResponse {
    let rename = async |from: &str, to: &str| {
        th.app
            .store()
            .execute_unchecked(&format!("ALTER TABLE {from} RENAME TO {to}"))
            .await
            .expect("the table of the sessions is renamed");
    };
    let cookie = format!("{SESSION_COOKIE}={session}; {SIGNUP_COOKIE}=a-marker");
    let request = th
        .anonymous()
        .request(Method::POST, "/invitations/preview")
        .header("Cookie", cookie)
        .json(&json!({"token": token}));
    rename("sessions", "sessions_away").await;
    let response = Client::send(request).await;
    rename("sessions_away", "sessions").await;
    response
}

/// A server that confirms no address replaces no registration: the preview of a link
/// does not look the session up, whatever cookies come with it. With the sessions out
/// of reach it is shown as ever.
#[tokio::test]
async fn a_preview_on_a_server_that_confirms_no_address_asks_for_no_session() {
    let th = Setup::native().start().await.init_basic().await;
    let token = th.member_link().await;
    let response = preview_without_sessions(&th, &token, &th.client.token).await;
    response.check_status(StatusCode::OK);
    th.teardown().await;
}

/// On a server that confirms addresses the preview asks who is signed in of a browser
/// with a registration's marker. A store that fails then is an error of the server,
/// not a browser without a session: answered as one, the link would be shown to
/// register by while the browser holds a session.
#[tokio::test]
async fn a_preview_whose_session_cannot_be_looked_up_is_an_error_of_the_server() {
    let smtp = FakeSmtp::start_on(0).await;
    let th = verifying(&smtp).await;
    let org = Org::found(&th, &smtp, "ada").await;
    let token = org.member_link().await.token.expect("the link's token");
    let response = preview_without_sessions(&th, &token, &org.founder.token).await;
    response.check_error(StatusCode::INTERNAL_SERVER_ERROR, "internal server error");
    th.teardown().await;
    smtp.stop().await;
}
