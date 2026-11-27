//! Signing up without the name of an organization, and starting one later: an account
//! that starts nothing has its own space and is in no organization, the first user of
//! a server names the organization they start, and an account that is there starts a
//! few and no more.

use fvoci_model::{Container, ContainerVisibility, OrgKind, UserWorkspace, WorkspaceRole};
use fvoci_store::DbType;
use fvoci_store::testing::db_type_of_the_tests;
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::boards::new_board;
use crate::common::{ApiResponse, Client, PASSWORD, Setup, TestServer};
use crate::email_verification::{COMPLETE, SIGNUP_PASSWORD, link_token};
use crate::fake_smtp::FakeSmtp;
use crate::own_space::workspaces_and_own;

fn open(config: &mut fvoci_server::config::Configuration) {
    "open".clone_into(&mut config.org_creation);
}

/// A registration of `username`, with the name of an organization if one is given.
fn registration(username: &str, organization_name: Option<&str>) -> Value {
    let mut request = json!({
        "username": username,
        "email": format!("{username}@sample.com"),
        "password": PASSWORD,
    });
    if let Some(name) = organization_name {
        request["organizationName"] = json!(name);
    }
    request
}

/// Checks that the answer refuses a registration for naming no organization.
#[track_caller]
fn check_names_none(response: &ApiResponse) {
    response.check_bad_request();
    assert_eq!(
        (
            response.refused_at().as_str(),
            response.value()["errors"][0]["reason"].as_str()
        ),
        ("/organizationName", Some("required")),
        "{}",
        response.text()
    );
}

/// Checks that the answer is the refusal of a server that has its one organization.
#[track_caller]
fn check_invitation_required(response: &ApiResponse) {
    response.check_forbidden();
    assert_eq!(
        response.error_reason(),
        "invitation_required",
        "{}",
        response.text()
    );
}

/// How many organizations of people the database of the server holds.
async fn organizations_of_people(th: &TestServer) -> usize {
    let query = format!(
        "SELECT 'row' FROM organizations WHERE kind = '{}' AND NOT is_system",
        OrgKind::Shared.as_str()
    );
    let found = th.app.store().system().texts_unchecked(&query).await;
    found.expect("read").len()
}

/// A server as somebody runs it for themselves, which is every server on SQLite: until
/// it has its first user a registration names the organization it starts, and the
/// login page is told so. The first account starts the one organization and has its
/// own space beside it. Nobody registers after that, with or without the name of an
/// organization, and nobody starts a second one: the second person comes in by a link,
/// into the same organization, with a space of their own.
#[tokio::test]
async fn the_first_user_names_the_organization_and_the_rest_come_by_invitation() {
    let th = Setup::native().start().await;
    let options = th.anonymous().get("/login-options").await.value();
    assert_eq!(
        (
            &options["organizationRequired"],
            &options["registrationOpen"]
        ),
        (&json!(true), &json!(true)),
        "{options}"
    );
    for name in [None, Some(""), Some("  ")] {
        let refused = th
            .anonymous()
            .post("/register", &registration("early", name))
            .await;
        check_names_none(&refused);
    }
    assert!(
        !th.app.store().is_bootstrapped().await.expect("read"),
        "a registration that is refused is not the first"
    );
    assert_eq!(th.app.registered_user_count().await.expect("count"), 0);

    let th = th.init_basic().await;
    let options = th.anonymous().get("/login-options").await.value();
    assert_eq!(
        (
            &options["organizationRequired"],
            &options["registrationOpen"]
        ),
        (&json!(false), &json!(false)),
        "{options}"
    );
    for name in [None, Some("Another")] {
        let refused = th
            .anonymous()
            .post("/register", &registration("late", name))
            .await;
        check_invitation_required(&refused);
    }
    for client in [&th.client, &th.client2] {
        let refused = client.post("/org", &json!({"name": "Another"})).await;
        check_invitation_required(&refused);
    }
    th.anonymous()
        .post("/org", &json!({"name": "Another"}))
        .await
        .check_unauthorized();

    let (firsts, first_own) = workspaces_and_own(&th.client).await;
    let (seconds, second_own) = workspaces_and_own(&th.client2).await;
    assert_ne!(first_own.org_id, second_own.org_id, "a space for each");
    let org_id = &firsts[0].org_id;
    assert!(
        firsts
            .iter()
            .chain(&seconds)
            .all(|workspace| &workspace.org_id == org_id),
        "one organization for both: {firsts:?} {seconds:?}"
    );
    assert_eq!(organizations_of_people(&th).await, 1);
    assert_eq!(th.app.registered_user_count().await.expect("count"), 2);
    th.teardown().await;
}

/// A server that takes several organizations, with `alice`, its first user, in the
/// one she started, and `carol`, who registered without the name of one.
struct WithOneAlone {
    th: TestServer,
    alice: Client,
    carol: Client,
}

async fn with_one_alone() -> WithOneAlone {
    let th = Setup::native()
        .on_an_empty_database()
        .configure(open)
        .start()
        .await;
    th.anonymous()
        .post("/register", &registration("alice", Some("Alpha")))
        .await
        .check_created();
    let made = th
        .anonymous()
        .post("/register", &registration("carol", None))
        .await;
    made.check_created();
    assert_eq!(made.value()["isAdmin"], json!(false), "{}", made.text());
    let alice = th.login("alice", PASSWORD).await;
    let carol = th.login("carol", PASSWORD).await;
    WithOneAlone { th, alice, carol }
}

/// On SQLite, multi-organization mode (`org_creation = "open"`) is rejected at config
/// check time, and under email verification the first user starts the one organization
/// while a second registration (with or without an organization name) is refused with
/// `invitation_required`.
#[tokio::test]
async fn sqlite_refuses_open_org_creation_and_rejects_second_verified_signup() {
    let mut config = fvoci_server::config::Configuration {
        database_kind: "sqlite".to_owned(),
        org_creation: "open".to_owned(),
        ..fvoci_server::config::Configuration::default()
    };
    let refused = config
        .clone()
        .check()
        .expect_err("SQLite refuses open org_creation");
    assert!(
        refused.to_string().contains("postgres"),
        "expected postgres requirement, got: {refused}"
    );
    config.org_creation = "first_user_only".to_owned();
    config.check().expect("first_user_only is valid on SQLite");

    let smtp = FakeSmtp::start_on(0).await;
    let th = Setup::native()
        .on_an_empty_database()
        .smtp(smtp.settings())
        .configure(|cfg| cfg.mail.email_verification = true)
        .start()
        .await;
    let waiting = |username: &str, organization_name: Option<&str>| {
        let mut request = registration(username, organization_name);
        request["email"] = json!(format!("{username}@example.test"));
        request["password"] = json!(SIGNUP_PASSWORD);
        request
    };
    th.anonymous()
        .post("/register", &waiting("ivan", Some("Ours")))
        .await
        .check_status(StatusCode::ACCEPTED);
    let message = smtp.received(1).await.pop().expect("its message");
    th.anonymous()
        .post(
            COMPLETE,
            &json!({"token": link_token(&message), "password": SIGNUP_PASSWORD}),
        )
        .await
        .check_ok();
    for name in [None, Some("Another")] {
        let second = th
            .anonymous()
            .post("/register", &waiting("kim", name))
            .await;
        check_invitation_required(&second);
    }
    th.teardown().await;
    smtp.stop().await;
}

/// Who registers without the name of an organization, on a server that takes several,
/// has their own space and nothing else: one workspace is listed, they work in it, and
/// they are in no organization, so nothing that counts organizations counts one for
/// them. The login page names the organization it named before; the server's own
/// organization and its administrators are who they were; the account goes, when it
/// asks to, with its own space and hands nothing over; and no organization's provider
/// is asked of its session.
#[tokio::test]
async fn who_registers_without_an_organization_has_their_own_space_alone_on_postgres() {
    if db_type_of_the_tests() != DbType::Postgres {
        return;
    }
    let WithOneAlone { th, alice, carol } = with_one_alone().await;
    let options = th.anonymous().get("/login-options").await.value();

    // A blank name is none, as one that is left out.
    th.anonymous()
        .post("/register", &registration("dave", Some("  ")))
        .await
        .check_created();
    assert_eq!(
        th.anonymous().get("/login-options").await.value(),
        options,
        "an account in no organization changes nothing of the login page"
    );
    assert_eq!(organizations_of_people(&th).await, 1, "Alpha alone");

    let (listed, own) = workspaces_and_own(&carol).await;
    assert_eq!(listed, [], "no workspace but their own");
    let me = carol.me().await;
    assert!(!me.is_admin, "no administrator of the server");
    let board: Container = carol
        .create_board(&new_board(&own.id, ContainerVisibility::Private, "mine"))
        .await
        .created_json();
    assert_eq!(board.workspace_id, own.id);

    // Nothing of Alpha is theirs, and nothing of the server's.
    let (alpha, _) = workspaces_and_own(&alice).await;
    carol
        .get(&format!("/w/{}", alpha[0].id))
        .await
        .check_not_found();
    carol
        .put(&format!("/users/{}/admin", me.id), &json!({"admin": true}))
        .await
        .check_forbidden();
    assert!(alice.me().await.is_admin, "the first user runs the server");
    assert_eq!(
        th.app
            .store()
            .required_provider_for(&me.id, &alpha[0].org_id)
            .await
            .expect("read"),
        None,
        "no provider is asked of an account that is in no organization (where one is \
         asked of its members, `first_password` asks the same)"
    );

    // The account goes with its own space, and leaves no organization without anybody.
    let standing: Value = carol.get("/users/me/deletion").await.ok_json();
    assert_eq!(
        (
            &standing["lastInstanceAdmin"],
            &standing["soleAdminOf"],
            standing["organizations"].as_array().map(Vec::len)
        ),
        (&json!(false), &json!([]), Some(1)),
        "{standing}"
    );
    assert_eq!(standing["organizations"][0]["id"], json!(own.org_id));
    carol
        .post(
            "/users/me/deletion",
            &json!({"username": "carol", "password": PASSWORD}),
        )
        .await
        .check_ok();
    th.teardown().await;
}

/// The workspace `createOrganization` answers for `name`.
async fn start(client: &Client, name: &str) -> ApiResponse {
    client.post("/org", &json!({"name": name})).await
}

/// Checks that the answer refuses one organization more than an account makes.
#[track_caller]
fn check_too_many(response: &ApiResponse) {
    response.check_bad_request();
    assert_eq!(
        (response.error_reason().as_str(), &response.value()["limit"]),
        ("too_many_organizations", &json!(3)),
        "{}",
        response.text()
    );
    assert_eq!(
        response.problem_type(),
        "/problems/too-many-organizations",
        "{}",
        response.text()
    );
}

/// An account that is there starts an organization: it is answered the first
/// workspace, as it is listed from then on, and administers it. The name is read as a
/// registration's is. An account makes three and no more, the one it registered with
/// among them, and is told the limit; one that started nothing makes three.
#[tokio::test]
async fn an_account_starts_organizations_and_no_more_than_three_on_postgres() {
    if db_type_of_the_tests() != DbType::Postgres {
        return;
    }
    let WithOneAlone { th, alice, carol } = with_one_alone().await;
    let (_, own) = workspaces_and_own(&carol).await;

    for (name, reason) in [("", "required"), ("  ", "required")] {
        let refused = start(&carol, name).await;
        refused.check_bad_request();
        assert_eq!(refused.refused_at(), "/name", "{}", refused.text());
        assert_eq!(
            refused.value()["errors"][0]["reason"],
            json!(reason),
            "{}",
            refused.text()
        );
    }
    let long = start(&carol, &"x".repeat(65)).await;
    long.check_bad_request();
    assert_eq!(long.refused_at(), "/name", "{}", long.text());
    carol
        .post("/org", &json!({"name": "Gamma", "kind": "personal"}))
        .await
        .check_bad_request();
    assert_eq!(organizations_of_people(&th).await, 1, "nothing was made");

    let gamma: UserWorkspace = start(&carol, "  Gamma ").await.created_json();
    assert_eq!(
        (
            gamma.kind,
            gamma.role,
            gamma.name.as_str(),
            gamma.sso_required
        ),
        (OrgKind::Shared, WorkspaceRole::Admin, "Gamma", false),
        "{gamma:?}"
    );
    let (listed, own_after) = workspaces_and_own(&carol).await;
    assert_eq!(
        listed,
        std::slice::from_ref(&gamma),
        "listed as it was answered"
    );
    assert_eq!(own_after, own, "their own space is what it was");
    // The administrator of the workspace and of the organization: a link is theirs to
    // make, and the organization's settings theirs to read.
    carol
        .post(
            &format!("/w/{}/invitations", gamma.id),
            &json!({"role": "member"}),
        )
        .await
        .check_created();
    carol
        .get(&format!("/org/{}/members", gamma.org_id))
        .await
        .check_ok();
    alice
        .get(&format!("/w/{}", gamma.id))
        .await
        .check_not_found();
    assert!(
        !carol.me().await.is_admin,
        "the server is run by who ran it"
    );

    // Two more for who started nothing, and two more for who registered with one.
    for name in ["Delta", "Epsilon"] {
        start(&carol, name).await.check_created();
    }
    check_too_many(&start(&carol, "Zeta").await);
    for name in ["Beta", "Eta"] {
        start(&alice, name).await.check_created();
    }
    check_too_many(&start(&alice, "Theta").await);
    assert_eq!(organizations_of_people(&th).await, 6);
    assert_eq!(workspaces_and_own(&alice).await.0.len(), 3);
    th.teardown().await;
}

/// An organization is counted to who made it after they have left it: an account that
/// made three hands one to somebody else, who is let in, made its administrator, takes
/// the maker's role and removes them from its workspace, and the maker leaves it
/// (`leaveOrganization`). The maker is listed the two that are left, finds the third
/// no more, and is still refused a fourth with the answer of the limit. Who took it
/// over did not make it: it is not counted to them.
#[tokio::test]
async fn an_organization_is_counted_to_who_made_it_after_they_left_it_on_postgres() {
    if db_type_of_the_tests() != DbType::Postgres {
        return;
    }
    let WithOneAlone { th, alice, carol } = with_one_alone().await;
    let gamma: UserWorkspace = start(&carol, "Gamma").await.created_json();
    for name in ["Delta", "Epsilon"] {
        start(&carol, name).await.check_created();
    }
    check_too_many(&start(&carol, "Zeta").await);

    let (alice_id, carol_id) = (alice.me().await.id, carol.me().await.id);
    let (workspace, organization) = (format!("/w/{}", gamma.id), format!("/org/{}", gamma.org_id));
    let link = carol
        .post(
            &format!("{workspace}/invitations"),
            &json!({"role": "member"}),
        )
        .await;
    link.check_created();
    alice
        .post(
            "/invitations/accept",
            &json!({"token": link.value()["token"]}),
        )
        .await
        .check_ok();
    let admin = json!({"role": "admin"});
    carol
        .put(&format!("{organization}/members/{alice_id}/role"), &admin)
        .await
        .check_no_content();
    carol
        .put(&format!("{workspace}/members/{alice_id}"), &admin)
        .await
        .check_ok();
    alice
        .put(
            &format!("{organization}/members/{carol_id}/role"),
            &json!({"role": "member"}),
        )
        .await
        .check_no_content();
    alice
        .delete(&format!("{workspace}/members/{carol_id}"))
        .await
        .check_no_content();
    carol
        .post(&format!("{organization}/leave"), &json!({}))
        .await
        .check_no_content();

    let (listed, _) = workspaces_and_own(&carol).await;
    let names: Vec<&str> = listed
        .iter()
        .map(|workspace| workspace.name.as_str())
        .collect();
    assert_eq!(names, ["Delta", "Epsilon"], "the two that are left");
    carol.get(&workspace).await.check_not_found();
    carol
        .get(&format!("{organization}/members"))
        .await
        .check_not_found();
    check_too_many(&start(&carol, "Zeta").await);

    // Alice made the one she registered with, and none of Gamma: two more are hers.
    for name in ["Beta", "Eta"] {
        start(&alice, name).await.check_created();
    }
    check_too_many(&start(&alice, "Theta").await);
    assert_eq!(organizations_of_people(&th).await, 6);
    th.teardown().await;
}

/// On a server that asks registrations to confirm their address, one that names no
/// organization waits as the others do, and its link makes the account with its own
/// space alone: the answer leads into that space and says no organization was
/// started. Before the server has its first user it is refused as anywhere, and
/// nothing is mailed.
#[tokio::test]
async fn a_registration_without_an_organization_is_confirmed_into_its_own_space_on_postgres() {
    if db_type_of_the_tests() != DbType::Postgres {
        return;
    }
    let smtp = FakeSmtp::start_on(0).await;
    let th = Setup::native()
        .on_an_empty_database()
        .smtp(smtp.settings())
        .configure(|config| {
            config.mail.email_verification = true;
            open(config);
        })
        .start()
        .await;
    let waiting = |username: &str, organization_name: Option<&str>| {
        let mut request = registration(username, organization_name);
        request["email"] = json!(format!("{username}@example.test"));
        request["password"] = json!(SIGNUP_PASSWORD);
        request
    };
    let confirm = async |count: usize| {
        let message = smtp.received(count).await.pop().expect("its message");
        let request = json!({"token": link_token(&message), "password": SIGNUP_PASSWORD});
        th.anonymous().post(COMPLETE, &request).await
    };
    check_names_none(
        &th.anonymous()
            .post("/register", &waiting("early", None))
            .await,
    );
    th.anonymous()
        .post("/register", &waiting("ivan", Some("Ours")))
        .await
        .check_status(StatusCode::ACCEPTED);
    let first = confirm(1).await;
    first.check_ok();
    assert_eq!(first.value()["organizationStarted"], json!(true));

    th.anonymous()
        .post("/register", &waiting("kim", None))
        .await
        .check_status(StatusCode::ACCEPTED);
    let confirmed = confirm(2).await;
    confirmed.check_ok();
    let answer = confirmed.value();
    assert_eq!(answer["organizationStarted"], json!(false), "{answer}");
    assert_eq!(answer.get("containerId"), None, "{answer}");
    let kim = th.client_with_token(&confirmed.session_cookie().expect("a session"));
    let (listed, own) = workspaces_and_own(&kim).await;
    assert_eq!(listed, [], "no workspace but their own");
    assert_eq!(answer["workspaceId"], json!(own.id), "{answer}");
    let boards: Vec<Container> = kim.get(&format!("/w/{}/boards", own.id)).await.ok_json();
    assert_eq!(boards, [], "nobody is welcomed into a space with a board");
    assert_eq!(organizations_of_people(&th).await, 1);
    th.teardown().await;
    smtp.stop().await;
}
