//! A new account by the link of an invitation, on a server that confirms addresses: it
//! waits for its address unless the link was mailed to that address, holds its use of
//! the link meanwhile, and is let in by its mailed link or by the server's operator.

use std::path::Path;
use std::time::Duration;

use fvoci_model::{Invitation, UserWorkspace};
use fvoci_store::{SIGNUP_LIFETIME_MILLIS, SignupLookup};
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::admin::socket::{socket_path, socket_request};
use crate::common::{Client, Setup, TestServer};
use crate::email_verification::{
    COMPLETE, SIGNUP_PASSWORD, complete_elsewhere, complete_with_marker, link_token, marker_of,
    registration,
};
use crate::fake_smtp::{FakeSmtp, Received};

/// A server that confirms addresses, and its first user, `ada`, who confirmed hers and
/// administers the organization: signed in, with the workspace she was let into.
struct Scene {
    th: TestServer,
    smtp: FakeSmtp,
    ada: Client,
    workspace_id: String,
}

impl Scene {
    async fn start(socket: Option<&Path>) -> Self {
        Self::start_from(Setup::native(), socket).await
    }

    /// As [`Self::start`], on a server set up as `setup` says besides.
    async fn start_from(setup: Setup, socket: Option<&Path>) -> Self {
        let smtp = FakeSmtp::start_on(0).await;
        let mut setup = setup
            .smtp(smtp.settings())
            .configure(|config| config.mail.email_verification = true);
        if let Some(socket) = socket {
            setup = setup.admin_socket(socket);
        }
        let th = setup.start().await;
        th.anonymous()
            .post("/register", &registration("ada", "en"))
            .await
            .check_status(StatusCode::ACCEPTED);
        let confirmed = confirm(&th, &smtp.received(1).await[0]).await;
        confirmed.check_status(StatusCode::OK);
        let ada = th.client_with_token(&confirmed.session_cookie().expect("a session"));
        let workspace_id = confirmed.value()["workspaceId"]
            .as_str()
            .expect("her workspace")
            .to_owned();
        Self {
            th,
            smtp,
            ada,
            workspace_id,
        }
    }

    fn links(&self) -> String {
        format!("/w/{}/invitations", self.workspace_id)
    }

    /// A new link of ada's: for members, or for a guest of a new board of hers.
    async fn link(&self, role: Role) -> Invitation {
        let request = if role == Role::Guest {
            let board: fvoci_model::Container = self
                .ada
                .create_board(&fvoci_model::Container {
                    workspace_id: self.workspace_id.clone(),
                    visibility: fvoci_model::ContainerVisibility::Open,
                    ..fvoci_model::Container::default()
                })
                .await
                .created_json();
            json!({"role": "guest", "containerId": board.id, "containerRole": "viewer"})
        } else {
            json!({"role": "member"})
        };
        self.ada.post(&self.links(), &request).await.created_json()
    }

    /// Accepts the link of `token` with a new account `username` at `email`.
    async fn accept(&self, token: &str, username: &str, email: &str) -> crate::common::ApiResponse {
        self.th
            .anonymous()
            .post(
                "/invitations/accept",
                &json!({"token": token, "language": "ko", "account": {
                    "username": username,
                    "email": email,
                    "password": "Pa$$word-1",
                }}),
            )
            .await
    }

    /// The messages sent once the background work is done.
    async fn sent(&self) -> Vec<Received> {
        assert!(
            self.th.app.finish_background(Duration::from_secs(10)).await,
            "the mail is done with"
        );
        self.smtp.messages().await
    }

    async fn stop(self) {
        self.th.teardown().await;
        self.smtp.stop().await;
    }
}

/// What a link lets its acceptor in as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    Member,
    Guest,
}

/// Confirms the registration whose mail is `message`, from another browser than it was
/// made from, with its password.
async fn confirm(th: &TestServer, message: &Received) -> crate::common::ApiResponse {
    complete_elsewhere(th, &link_token(message), SIGNUP_PASSWORD).await
}

#[tokio::test]
async fn a_new_account_by_a_link_for_members_waits_for_its_address_and_joins_with_it() {
    let scene = Scene::start(None).await;
    let link = scene.link(Role::Member).await;
    let answer = scene
        .accept(
            link.token.as_deref().expect("its token"),
            "pat",
            "pat@example.test",
        )
        .await;
    answer.check_status(StatusCode::ACCEPTED);
    assert!(answer.text().is_empty(), "nothing of an account");
    assert!(
        scene
            .th
            .app
            .store()
            .get_user_by_username("pat")
            .await
            .is_err_and(|error| error.is_not_found()),
        "no account yet"
    );
    let refused = scene.th.anonymous().login("pat", "Pa$$word-1").await;
    assert_eq!(refused.error_reason(), "email_unverified", "not yet");

    let messages = scene.smtp.received(2).await;
    let mail = &messages[1];
    assert_eq!(mail.recipient, "pat@example.test");
    assert!(
        mail.decoded.contains("lang=\"ko\""),
        "the screen's language"
    );
    let confirmed = confirm(&scene.th, mail).await;
    confirmed.check_status(StatusCode::OK);
    assert_eq!(
        confirmed.value()["workspaceId"],
        scene.workspace_id.as_str()
    );
    assert_eq!(
        confirmed.value()["organizationStarted"],
        false,
        "it joined by the link"
    );
    let pat = scene
        .th
        .client_with_token(&confirmed.session_cookie().expect("a session"));
    let workspaces: Vec<UserWorkspace> = pat.get("/users/me/workspaces").await.ok_json();
    assert!(
        workspaces
            .iter()
            .any(|workspace| workspace.id == scene.workspace_id),
        "a member of the link's workspace: {workspaces:?}"
    );
    scene.stop().await;
}

#[tokio::test]
async fn a_guest_link_mailed_to_another_address_proves_nothing_of_this_one() {
    let scene = Scene::start(None).await;
    let link = scene.link(Role::Guest).await;
    scene
        .ada
        .post(
            &format!("{}/mail", scene.links()),
            &json!({"token": link.token, "recipients": ["rae@example.test"]}),
        )
        .await
        .check_no_content();
    let messages = scene.smtp.received(2).await;
    let mailed = messages[1]
        .decoded
        .lines()
        .find(|line| line.contains("/invite/"))
        .expect("the link");
    let token = mailed.rsplit_once("/invite/").expect("a token").1.trim();
    let answer = scene.accept(token, "mal", "mal@example.test").await;
    answer.check_status(StatusCode::ACCEPTED);
    scene.smtp.received(3).await;

    // The link it reserved was made like a link that is revoked now: it lets nobody in.
    scene
        .ada
        .delete(&format!("{}/{}", scene.links(), link.id))
        .await
        .check_no_content();
    let refused = confirm(&scene.th, &scene.sent().await[2]).await;
    refused.check_status(StatusCode::NOT_FOUND);
    assert_eq!(refused.error_reason(), "invitation_unavailable");
    assert!(
        scene
            .th
            .app
            .store()
            .live_signup(
                SignupLookup::Username("mal"),
                fvoci_server::auth::now_millis()
            )
            .await
            .expect("read")
            .is_none(),
        "the registration went, and its names are free"
    );
    scene.stop().await;
}

#[tokio::test]
async fn a_one_use_link_reserved_by_a_registration_lets_nobody_else_in_until_it_ends() {
    let scene = Scene::start(None).await;
    let link = scene.link(Role::Guest).await;
    let token = link.token.as_deref().expect("its token");
    let (first, second) = tokio::join!(
        scene.accept(token, "sam", "sam@example.test"),
        scene.accept(token, "sol", "sol@example.test"),
    );
    let mut statuses = [first.status, second.status];
    statuses.sort();
    assert_eq!(
        statuses,
        [StatusCode::ACCEPTED, StatusCode::NOT_FOUND],
        "one registration takes the link"
    );
    let holder = if first.status == StatusCode::ACCEPTED {
        "sam"
    } else {
        "sol"
    };
    scene
        .accept(token, "tam", "tam@example.test")
        .await
        .check_status(StatusCode::NOT_FOUND);

    // A held address is taken for an account by any link.
    let member_link = scene.link(Role::Member).await;
    let held = scene
        .accept(
            member_link.token.as_deref().expect("its token"),
            "other",
            &format!("{holder}@example.test"),
        )
        .await;
    held.check_status(StatusCode::CONFLICT);
    assert_eq!(held.refused_at(), "/account/email");

    // Seven days on, unconfirmed, the registration gives the link back.
    let later = fvoci_server::auth::now_millis() + SIGNUP_LIFETIME_MILLIS + 1;
    assert_eq!(
        scene
            .th
            .app
            .housekeeping()
            .remove_ended_signups(later)
            .await
            .expect("cleaned"),
        1
    );
    scene
        .accept(token, "tam", "tam@example.test")
        .await
        .check_status(StatusCode::ACCEPTED);
    scene.stop().await;
}

/// Whoever a link for members lets in once their address is confirmed has a welcome
/// board of their own, written in the language of the screen they registered on,
/// whether the mailed link confirms or the server's operator; a guest has none.
#[tokio::test]
async fn a_confirmed_registration_by_a_link_for_members_has_a_welcome_board() {
    let socket = socket_path();
    let scene = Scene::start_from(Setup::native().with_templates(), Some(&socket)).await;
    let members = scene.link(Role::Member).await;
    let members = members.token.as_deref().expect("its token");
    let guests = scene.link(Role::Guest).await;
    for (token, username) in [
        (members, "pat"),
        (guests.token.as_deref().expect("its token"), "gil"),
        (members, "uma"),
    ] {
        scene
            .accept(token, username, &format!("{username}@example.test"))
            .await
            .check_status(StatusCode::ACCEPTED);
    }
    let messages = scene.smtp.received(4).await;
    for mail in &messages[1..3] {
        confirm(&scene.th, mail).await.check_status(StatusCode::OK);
    }
    let target = "/api/v1/admin/users/uma/email-verification";
    socket_request(&socket, "POST", target, "")
        .await
        .check_no_content();

    let store = scene.th.app.store();
    let mut titles = Vec::new();
    for username in ["pat", "gil", "uma"] {
        let user = store.get_user_by_username(username).await.expect("in");
        let boards = store
            .texts_unchecked(&format!(
                "SELECT title FROM containers WHERE CAST(created_by AS TEXT) = '{}' \
                 AND is_template = false",
                user.id
            ))
            .await
            .expect("the boards are read");
        titles.push(boards);
    }
    let welcome = vec!["fvoci 시작하기".to_owned()];
    assert_eq!(titles, [welcome.clone(), Vec::new(), welcome]);
    scene.stop().await;
}

/// The operator confirms a registration whose mail does not arrive: one by a link for
/// members joins, and the organization it joins has it on record.
#[tokio::test]
async fn the_operator_confirms_a_registration_and_records_it_where_it_joins() {
    let socket = socket_path();
    let scene = Scene::start(Some(&socket)).await;
    let link = scene.link(Role::Member).await;
    scene
        .accept(
            link.token.as_deref().expect("its token"),
            "Uma",
            "uma@example.test",
        )
        .await
        .check_status(StatusCode::ACCEPTED);
    let target = "/api/v1/admin/users/uma/email-verification";
    socket_request(&socket, "POST", target, "")
        .await
        .check_no_content();
    scene
        .th
        .anonymous()
        .login("Uma", "Pa$$word-1")
        .await
        .check_no_content();
    socket_request(&socket, "POST", target, "")
        .await
        .check_error(404, "not-found", "registration not found");

    let workspaces: Vec<UserWorkspace> = scene.ada.get("/users/me/workspaces").await.ok_json();
    let org_id = &workspaces
        .iter()
        .find(|workspace| workspace.id == scene.workspace_id)
        .expect("her workspace")
        .org_id;
    let records: Vec<Value> = scene
        .ada
        .get(&format!("/org/{org_id}/audit"))
        .await
        .ok_json();
    let uma = scene
        .th
        .app
        .store()
        .get_user_by_username("uma")
        .await
        .expect("the account");
    let of_uma: Vec<&Value> = records
        .iter()
        .filter(|record| record["action"] == "email_verified_by_operator")
        .collect();
    assert_eq!(of_uma.len(), 1, "one record: {records:?}");
    assert_eq!(of_uma[0]["subject"]["id"], uma.id.as_str(), "{records:?}");
    assert_eq!(of_uma[0]["actor"]["kind"], "server_operator", "{records:?}");
    scene.stop().await;
    let _ = std::fs::remove_file(&socket);
}

/// A registration by a link is its registrant's to finish too: its acceptance gives the
/// browser its marker, and where the mail is opened without it or the password nothing
/// is touched, not even a registration whose link lets nobody in any more.
#[tokio::test]
async fn a_registration_by_a_link_is_finished_by_who_made_it_only() {
    let scene = Scene::start(None).await;
    let link = scene.link(Role::Member).await;
    let answer = scene
        .accept(
            link.token.as_deref().expect("its token"),
            "vera",
            "vera@example.test",
        )
        .await;
    answer.check_status(StatusCode::ACCEPTED);
    let marker = marker_of(&answer).expect("the marker of the browser that accepted");
    let token = link_token(&scene.smtp.received(2).await[1]);
    let waits = async || {
        scene
            .th
            .app
            .store()
            .live_signup(
                SignupLookup::Username("vera"),
                fvoci_server::auth::now_millis(),
            )
            .await
            .expect("read")
            .is_some()
    };

    // Revoked: confirming would remove the registration. Asking for the password does not.
    scene
        .ada
        .delete(&format!("{}/{}", scene.links(), link.id))
        .await
        .check_no_content();
    let asked = scene
        .th
        .anonymous()
        .post(COMPLETE, &json!({"token": token}))
        .await;
    asked.check_status(StatusCode::FORBIDDEN);
    assert_eq!(asked.error_reason(), "password_required");
    complete_elsewhere(&scene.th, &token, "a guess")
        .await
        .check_status(StatusCode::UNAUTHORIZED);
    assert!(waits().await, "nothing was touched");

    let refused = complete_with_marker(&scene.th, &token, &marker).await;
    refused.check_status(StatusCode::NOT_FOUND);
    assert_eq!(refused.error_reason(), "invitation_unavailable");
    assert!(!waits().await, "told to its registrant, and removed");

    // By a link that still lets in, the browser that accepted joins at once.
    let link = scene.link(Role::Member).await;
    let answer = scene
        .accept(
            link.token.as_deref().expect("its token"),
            "vito",
            "vito@example.test",
        )
        .await;
    answer.check_status(StatusCode::ACCEPTED);
    let marker = marker_of(&answer).expect("a marker");
    let token = link_token(&scene.sent().await.pop().expect("its mail"));
    let joined = complete_with_marker(&scene.th, &token, &marker).await;
    joined.check_status(StatusCode::OK);
    assert_eq!(joined.value()["workspaceId"], scene.workspace_id.as_str());
    assert_eq!(
        marker_of(&joined).as_deref(),
        Some(""),
        "the marker is done"
    );
    scene.stop().await;
}

/// A guest who confirms is told what let them in, a board or a wiki, as one who is let
/// in at once is: each has an address of its own.
#[tokio::test]
async fn a_confirmed_guest_is_told_whether_a_board_or_a_wiki_let_them_in() {
    let scene = Scene::start(None).await;
    let onto_board = scene.link(Role::Guest).await;
    let wiki = scene
        .ada
        .post(
            &format!("/w/{}/wikis", scene.workspace_id),
            &json!({"visibility": "private", "title": "Handbook"}),
        )
        .await;
    wiki.check_created();
    let onto_wiki: Invitation = scene
        .ada
        .post(
            &scene.links(),
            &json!({"role": "guest", "containerId": wiki.value()["id"], "containerRole": "viewer"}),
        )
        .await
        .created_json();

    for (link, username, kind) in [(&onto_board, "bo", "board"), (&onto_wiki, "wyn", "wiki")] {
        let email = format!("{username}@example.test");
        scene
            .accept(link.token.as_deref().expect("its token"), username, &email)
            .await
            .check_status(StatusCode::ACCEPTED);
        let sent = scene.sent().await;
        let mail = sent
            .iter()
            .rev()
            .find(|message| message.recipient == email)
            .expect("the confirmation's mail");
        let confirmed = confirm(&scene.th, mail).await;
        confirmed.check_status(StatusCode::OK);
        assert_eq!(
            confirmed.value()["containerId"],
            json!(link.container_id),
            "{kind}"
        );
        assert_eq!(confirmed.value()["containerKind"], json!(kind));
    }
    scene.stop().await;
}
