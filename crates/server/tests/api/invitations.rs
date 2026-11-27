//! Signing up with the name of an organization, and the links that let others into it.

use fvoci_model::{
    Container, ContainerVisibility, INVITATION_LIFETIME_MILLIS, Invitation, UserWorkspace,
};
use fvoci_store::DbType;
use fvoci_store::testing::db_type_of_the_tests;
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::common::{
    ApiResponse, Client, ORG_NAME, PASSWORD, ROOT_WORKSPACE_ID, Setup, TestServer, container_path,
};
use crate::own_space::workspaces_and_own;

fn open(config: &mut fvoci_server::config::Configuration) {
    "open".clone_into(&mut config.org_creation);
}

/// A registration that starts the organization `organization_name`.
fn registration(username: &str, organization_name: &str) -> Value {
    json!({
        "username": username,
        "email": format!("{username}@sample.com"),
        "password": PASSWORD,
        "organizationName": organization_name,
    })
}

/// The account `username` asks for when they accept a link.
fn account(username: &str) -> Value {
    json!({
        "username": username,
        "email": format!("{username}@sample.com"),
        "password": PASSWORD,
    })
}

/// What an answer says, without what names the one request.
fn said(response: &ApiResponse) -> (StatusCode, Value) {
    let mut body: Value = serde_json::from_slice(&response.body).unwrap_or(Value::Null);
    if let Some(object) = body.as_object_mut() {
        object.remove("instance");
        object.remove("requestId");
    }
    (response.status, body)
}

/// Makes a link for members into the root workspace, over the route.
async fn member_link(client: &Client) -> Invitation {
    client
        .post(
            &format!("/w/{ROOT_WORKSPACE_ID}/invitations"),
            &json!({"role": "member"}),
        )
        .await
        .created_json()
}

/// One column of a link, as text; nothing if there is no such link.
async fn of_the_link(th: &TestServer, link_id: &str, column: &str) -> Vec<String> {
    th.app
        .store()
        .texts_unchecked(&format!(
            "SELECT CAST({column} AS TEXT) FROM invitations WHERE CAST(id AS TEXT) = '{link_id}'"
        ))
        .await
        .expect("the link is read")
}

/// How many links the server has stored.
async fn links(th: &TestServer) -> i64 {
    th.app
        .store()
        .system()
        .count_rows("invitations")
        .await
        .expect("the links are counted")
}

/// A server that holds one organization takes registrations until it has its first user, and
/// says so to the page that offers to register.
#[tokio::test]
async fn a_server_takes_registrations_until_it_has_its_organization() {
    let th = Setup::native().start().await;
    let options = th.anonymous().get("/login-options").await.value();
    assert_eq!(options["registrationOpen"], json!(true), "{options}");

    th.anonymous()
        .post("/register", &registration("first", "  The Firm "))
        .await
        .check_created();

    let options = th.anonymous().get("/login-options").await.value();
    assert_eq!(options["registrationOpen"], json!(false), "{options}");
    assert_eq!(options["organization"]["name"], json!("The Firm"));
    let refused = th
        .anonymous()
        .post("/register", &registration("second", "Another Firm"))
        .await;
    refused.check_status(StatusCode::FORBIDDEN);
    assert_eq!(refused.error_reason(), "invitation_required");
    assert_eq!(
        refused.problem_type(),
        "/problems/invitation-required",
        "a kind of its own"
    );
    // Nobody registers after the first user, whatever they send: no word about the
    // request, and no password hashed.
    for body in [json!({}), json!({"username": "first", "password": "x"})] {
        let refused = th.anonymous().post("/register", &body).await;
        refused.check_status(StatusCode::FORBIDDEN);
        assert_eq!(refused.error_reason(), "invitation_required", "{body}");
        assert_eq!(refused.refused_places(), Vec::<String>::new(), "{body}");
    }
    assert_eq!(
        th.app.registered_user_count().await.expect("user count"),
        1,
        "nobody else got in"
    );
    // Nothing of the refused ones is there: no second organization, and the first
    // keeps its name, which is its first workspace's too, without the white space.
    let first = th.login("first", PASSWORD).await;
    let (workspaces, _own) = workspaces_and_own(&first).await;
    assert_eq!(workspaces.len(), 1, "{workspaces:?}");
    assert_eq!(workspaces[0].name, "The Firm");
    assert_eq!(workspaces[0].role, fvoci_model::WorkspaceRole::Admin);

    th.teardown().await;
}

/// Two who signed up on a server where everybody starts an organization
/// (`org_creation` is `open`): the server, and each of them with the workspace they
/// started.
struct TwoOrganizations {
    th: TestServer,
    alice: Client,
    alpha: UserWorkspace,
    bob: Client,
    beta: UserWorkspace,
}

async fn two_organizations() -> TwoOrganizations {
    let th = Setup::native()
        .on_an_empty_database()
        .configure(open)
        .start()
        .await;
    for (username, name) in [("alice", "Alpha"), ("bob", "Beta")] {
        let options = th.anonymous().get("/login-options").await.value();
        assert_eq!(options["registrationOpen"], json!(true), "{options}");
        let made = th
            .anonymous()
            .post("/register", &registration(username, name))
            .await;
        made.check_created();
        assert_eq!(
            made.value()["isAdmin"],
            json!(username == "alice"),
            "only the first user administers the server"
        );
    }
    let alice = th.login("alice", PASSWORD).await;
    let bob = th.login("bob", PASSWORD).await;
    let of = async |client: &Client| -> UserWorkspace {
        let (workspaces, _own) = workspaces_and_own(client).await;
        assert_eq!(workspaces.len(), 1, "the one they started: {workspaces:?}");
        workspaces
            .into_iter()
            .next()
            .expect("a workspace of their own")
    };
    let (alpha, beta) = (of(&alice).await, of(&bob).await);
    TwoOrganizations {
        th,
        alice,
        alpha,
        bob,
        beta,
    }
}

/// Makes a board of the workspace that is open to its members.
async fn open_board(client: &Client, workspace_id: &str) -> Container {
    let board: Container = client
        .post(
            &format!("/w/{workspace_id}/boards"),
            &json!({"workspaceId": workspace_id, "visibility": "open", "title": "Open"}),
        )
        .await
        .created_json();
    assert_eq!(board.visibility, ContainerVisibility::Open, "{board:?}");
    board
}

/// On a server where everybody starts an organization, two who sign up have one each,
/// and neither reaches anything of the other's.
#[tokio::test]
async fn where_everybody_starts_an_organization_each_has_one_of_their_own() {
    if db_type_of_the_tests() != DbType::Postgres {
        return;
    }
    let TwoOrganizations {
        th,
        alice,
        alpha,
        bob,
        beta,
    } = two_organizations().await;
    assert_eq!((alpha.name.as_str(), beta.name.as_str()), ("Alpha", "Beta"));
    assert_ne!(alpha.org_id, beta.org_id, "two organizations");
    assert_eq!(beta.role, fvoci_model::WorkspaceRole::Admin);

    // Each works in their own, and the other's is not there for them.
    let board = open_board(&alice, &alpha.id).await;
    for path in [
        format!("/w/{}", alpha.id),
        format!("/w/{}/boards", alpha.id),
        format!("/w/{}/users", alpha.id),
        container_path(&board),
    ] {
        bob.get(&path).await.check_status(StatusCode::NOT_FOUND);
    }
    let boards: Vec<Container> = bob.get(&format!("/w/{}/boards", beta.id)).await.ok_json();
    assert_eq!(boards, Vec::new(), "nothing of Alpha in Beta");

    // Behind the wall, each organization has its own rows and none of the other's.
    for (org_id, boards) in [(&alpha.org_id, 1), (&beta.org_id, 0)] {
        let store = th.app.store().for_org(org_id).expect("an identifier");
        for (table, rows) in [
            ("organizations", 1),
            ("workspaces", 1),
            ("organization_members", 1),
            ("workspace_members", 1),
            ("containers", boards),
        ] {
            assert_eq!(
                store.count_rows(table).await.expect("the rows are counted"),
                rows,
                "{table} of {org_id}"
            );
        }
    }

    // An account is the server's, not an organization's: a name is taken across them.
    let taken = th
        .anonymous()
        .post(
            "/register",
            &json!({
                "username": "ALICE", "email": "other@sample.com", "password": PASSWORD,
                "organizationName": "Gamma",
            }),
        )
        .await;
    taken.check_status(StatusCode::CONFLICT);
    assert_eq!(taken.error_reason(), "username_taken");
    assert_eq!(
        th.app
            .store()
            .system()
            .count_rows("organizations")
            .await
            .expect("the rows are counted"),
        // The two organizations, the system's own, and the space of each of the two
        // accounts: the one that was refused has none.
        5,
        "a refused registration leaves no organization behind, and no space"
    );

    th.teardown().await;
}

/// Somebody of another organization who accepts a link for members becomes a member of
/// the link's workspace and of its organization, and stays what they are in their own.
#[tokio::test]
async fn who_accepts_a_link_of_another_organization_is_a_member_of_both() {
    if db_type_of_the_tests() != DbType::Postgres {
        return;
    }
    let TwoOrganizations {
        th,
        alice,
        alpha,
        bob,
        beta,
    } = two_organizations().await;
    let board = open_board(&alice, &alpha.id).await;
    bob.get(&container_path(&board))
        .await
        .check_status(StatusCode::NOT_FOUND);

    let link = alice
        .post(
            &format!("/w/{}/invitations", alpha.id),
            &json!({"role": "member"}),
        )
        .await
        .created_json::<Invitation>()
        .token
        .expect("the token of a new link");
    let accepted = bob
        .post("/invitations/accept", &json!({"token": link}))
        .await;
    accepted.check_ok();
    assert_eq!(accepted.value(), json!({"workspaceId": alpha.id}));
    let (mut workspaces, _own) = workspaces_and_own(&bob).await;
    workspaces.sort_by(|a, b| a.name.cmp(&b.name));
    let seen: Vec<(&str, &str, fvoci_model::WorkspaceRole)> = workspaces
        .iter()
        .map(|workspace| {
            (
                workspace.name.as_str(),
                workspace.org_id.as_str(),
                workspace.role,
            )
        })
        .collect();
    assert_eq!(
        seen,
        [
            (
                "Alpha",
                alpha.org_id.as_str(),
                fvoci_model::WorkspaceRole::Member
            ),
            (
                "Beta",
                beta.org_id.as_str(),
                fvoci_model::WorkspaceRole::Admin
            ),
        ]
    );
    bob.get(&container_path(&board)).await.check_ok();
    let alphas = th
        .app
        .store()
        .for_org(&alpha.org_id)
        .expect("an identifier");
    assert_eq!(
        alphas
            .count_rows("organization_members")
            .await
            .expect("the rows are counted"),
        2,
        "a member of the organization too"
    );

    th.teardown().await;
}

/// A workspace's administrator makes a link for members; nobody else does. The link's
/// token is told once and stored as its digest; whoever accepts the link, with a new
/// account or with the one they have, is a member of the workspace.
#[tokio::test]
async fn a_workspace_administrator_invites_members_by_a_link() {
    let th = Setup::native().start().await.init_basic().await;
    let admin = th.client.me().await;
    let invitations = format!("/w/{ROOT_WORKSPACE_ID}/invitations");

    // Only an administrator of the workspace, and only for members.
    let refused = th
        .client2
        .post(&invitations, &json!({"role": "member"}))
        .await;
    refused.check_error(
        StatusCode::FORBIDDEN,
        "only the administrators of the workspace invite its members",
    );
    th.anonymous()
        .post(&invitations, &json!({"role": "member"}))
        .await
        .check_unauthorized();
    let before = links(&th).await;
    for (body, place) in [
        (json!({"role": "admin"}), "/role"),
        (
            json!({"role": "member", "containerId": admin.id}),
            "/containerId",
        ),
        (
            json!({"role": "member", "containerRole": "editor"}),
            "/containerRole",
        ),
    ] {
        let refused = th.client.post(&invitations, &body).await;
        refused.check_bad_request();
        assert_eq!(refused.refused_at(), place, "{body}");
    }
    assert_eq!(links(&th).await, before, "a refused request makes no link");

    let link = member_link(&th.client).await;
    assert_eq!(links(&th).await, before + 1);
    let token = link.token.clone().expect("the token of a new link");
    assert_eq!(
        token.len(),
        fvoci_server::auth::secret::SECRET_LENGTH,
        "256 random bits: {link:?}"
    );
    assert_eq!(link.workspace_id, ROOT_WORKSPACE_ID);
    assert_eq!(link.role, fvoci_model::WorkspaceRole::Member);
    assert_eq!(link.created_by, admin.id);
    assert_eq!(
        link.expires_at - link.created_at,
        INVITATION_LIFETIME_MILLIS
    );
    assert_eq!(
        (link.max_uses, link.uses),
        (None, 0),
        "as often as it is used"
    );
    assert_eq!((&link.container_id, link.container_role), (&None, None));

    // The store has the digest of the token, and the token nowhere.
    let stored = of_the_link(&th, &link.id, "token_hash").await;
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].len(), 64, "a SHA-256 digest in hexadecimal");
    assert!(!stored[0].contains(&token) && stored[0] != token);

    // Somebody without an account accepts with one to make. It signs nobody in.
    let accepted = th
        .anonymous()
        .post(
            "/invitations/accept",
            &json!({"token": token, "account": account("newcomer")}),
        )
        .await;
    accepted.check_ok();
    assert_eq!(accepted.value(), json!({"workspaceId": ROOT_WORKSPACE_ID}));
    assert_eq!(accepted.set_cookies(), Vec::<&str>::new());
    let newcomer = th.login("newcomer", PASSWORD).await;
    assert!(!newcomer.me().await.is_admin);
    let (workspaces, _own) = workspaces_and_own(&newcomer).await;
    assert_eq!(
        workspaces.len(),
        1,
        "the workspace of the link: {workspaces:?}"
    );
    assert_eq!(workspaces[0].id, ROOT_WORKSPACE_ID);
    assert_eq!(workspaces[0].role, fvoci_model::WorkspaceRole::Member);
    assert_eq!(workspaces[0].name, ORG_NAME);
    assert_eq!(of_the_link(&th, &link.id, "uses").await, ["1"]);

    // Who is a member already is let in again, and uses nothing of the link; a role
    // above a member's stays what it is.
    for client in [&newcomer, &th.client] {
        let again = client
            .post("/invitations/accept", &json!({"token": token}))
            .await;
        again.check_ok();
        assert_eq!(again.value(), json!({"workspaceId": ROOT_WORKSPACE_ID}));
    }
    assert_eq!(of_the_link(&th, &link.id, "uses").await, ["1"]);
    let workspaces: Vec<UserWorkspace> = th.client.get("/users/me/workspaces").await.ok_json();
    assert_eq!(workspaces[0].role, fvoci_model::WorkspaceRole::Admin);

    // The link lets in as many as use it.
    th.anonymous()
        .post(
            "/invitations/accept",
            &json!({"token": token, "account": account("another")}),
        )
        .await
        .check_ok();
    assert_eq!(of_the_link(&th, &link.id, "uses").await, ["2"]);

    th.teardown().await;
}

/// A request that accepts says who accepts in one way: a session, or an account to
/// make. Neither, and both, are refused for what they are, whatever the token.
#[tokio::test]
async fn a_request_that_accepts_has_a_session_or_an_account_to_make() {
    let th = Setup::native().start().await.init_basic().await;
    for token in [th.member_link().await, "no-link".to_owned()] {
        let both = th
            .client2
            .post(
                "/invitations/accept",
                &json!({"token": token, "account": account("a-third")}),
            )
            .await;
        both.check_bad_request();
        assert_eq!(both.refused_at(), "/account");
        th.anonymous()
            .post("/invitations/accept", &json!({"token": token}))
            .await
            .check_unauthorized();
    }
    th.teardown().await;
}

/// Whoever holds a link sees where it leads and who made it, signed in or not, and
/// nothing else: a name each. Looking uses nothing of the link.
#[tokio::test]
async fn a_link_shows_whoever_holds_it_where_it_leads_and_who_made_it() {
    let th = Setup::native().start().await.init_basic().await;
    let admin = th.client.me().await;
    let link = member_link(&th.client).await;
    let token = link.token.clone().expect("the token of a new link");

    for client in [&th.anonymous(), &th.client2] {
        let preview = client
            .post("/invitations/preview", &json!({"token": token}))
            .await;
        preview.check_ok();
        assert_eq!(
            preview.value(),
            json!({"workspaceName": ORG_NAME, "inviterName": admin.username})
        );
    }
    th.client
        .patch("/users/me", &json!({"customName": "Ada Lovelace"}))
        .await
        .check_ok();
    let preview = th
        .anonymous()
        .post("/invitations/preview", &json!({"token": token}))
        .await
        .value();
    assert_eq!(preview["inviterName"], json!("Ada Lovelace"), "{preview}");
    assert_eq!(of_the_link(&th, &link.id, "uses").await, ["0"]);

    th.teardown().await;
}

/// A token that lets nobody in is answered in one way, whatever is wrong with it and
/// whoever asks: no such link, one that ended, and one whose maker may no longer
/// invite. Nothing is stored for it.
#[tokio::test]
async fn a_link_that_lets_nobody_in_answers_as_no_link_at_all() {
    let th = Setup::native().start().await.init_basic().await;
    let third = th.add_user("third").await;
    let users = th.app.registered_user_count().await.expect("the users");

    let ended = member_link(&th.client).await;
    th.app
        .store()
        .execute_unchecked(&format!(
            "UPDATE invitations SET expires_at = created_at WHERE CAST(id AS TEXT) = '{}'",
            ended.id
        ))
        .await
        .expect("the link is ended");
    // The maker of this one stops administering the workspace after making it.
    th.app
        .store()
        .execute_unchecked(&format!(
            "UPDATE workspace_members SET role = 'admin' WHERE CAST(user_id AS TEXT) = '{}'",
            third.me().await.id
        ))
        .await
        .expect("a second administrator");
    let orphaned = member_link(&third).await;
    th.me_as_no_admin(&third).await;

    let tokens = [
        ("no such link", fvoci_server::auth::secret::new_secret()),
        ("no token", String::new()),
        ("an ended link", ended.token.clone().expect("a token")),
        (
            "a link of a former administrator",
            orphaned.token.clone().expect("a token"),
        ),
    ];
    let mut answers = Vec::new();
    for (case, token) in &tokens {
        let signed_in = th
            .client2
            .post("/invitations/accept", &json!({"token": token}))
            .await;
        let with_account = th
            .anonymous()
            .post(
                "/invitations/accept",
                &json!({"token": token, "account": account("nobody")}),
            )
            .await;
        let looked_at = th
            .anonymous()
            .post("/invitations/preview", &json!({"token": token}))
            .await;
        for answer in [signed_in, with_account, looked_at] {
            answer.check_status(StatusCode::NOT_FOUND);
            assert_eq!(answer.error_reason(), "invitation_unavailable", "{case}");
            assert_eq!(
                answer.problem_type(),
                "/problems/invitation-unavailable",
                "{case}"
            );
            answers.push((*case, said(&answer)));
        }
    }
    for (case, answer) in &answers {
        assert_eq!(answer, &answers[0].1, "{case}: one answer for all");
    }
    assert_eq!(
        th.app.registered_user_count().await.expect("the users"),
        users,
        "no account was made for a link that lets nobody in"
    );
    for link_id in [&ended.id, &orphaned.id] {
        assert_eq!(of_the_link(&th, link_id, "uses").await, ["0"]);
    }

    th.teardown().await;
}

/// Makes a board for the invited only in the root workspace, as `client`, who is its
/// admin.
async fn private_board(client: &Client, title: &str) -> Container {
    client
        .post(
            &format!("/w/{ROOT_WORKSPACE_ID}/boards"),
            &json!({"workspaceId": ROOT_WORKSPACE_ID, "visibility": "private", "title": title}),
        )
        .await
        .created_json()
}

/// Asks for a link that invites a guest to the board, in `role` if one is named.
async fn ask_for_a_guest_link(client: &Client, board_id: &str, role: Option<&str>) -> ApiResponse {
    let mut body = json!({"role": "guest", "containerId": board_id});
    if let Some(role) = role {
        body["containerRole"] = json!(role);
    }
    client
        .post(&format!("/w/{ROOT_WORKSPACE_ID}/invitations"), &body)
        .await
}

/// Makes a link that invites a guest to the board, as an editor.
async fn guest_link(client: &Client, board_id: &str) -> Invitation {
    ask_for_a_guest_link(client, board_id, None)
        .await
        .created_json()
}

async fn role_in_the_root_workspace(client: &Client) -> Option<fvoci_model::WorkspaceRole> {
    let workspaces: Vec<UserWorkspace> = client.get("/users/me/workspaces").await.ok_json();
    workspaces
        .iter()
        .find(|workspace| workspace.id == ROOT_WORKSPACE_ID)
        .map(|workspace| workspace.role)
}

/// The links of the root workspace as `client` is answered them, by identifier.
async fn listed(client: &Client) -> Vec<String> {
    let answer = client
        .get(&format!("/w/{ROOT_WORKSPACE_ID}/invitations"))
        .await;
    answer.check_ok();
    assert!(
        !answer.text().contains("\"token\""),
        "no link is listed with its token: {}",
        answer.text()
    );
    answer
        .json::<Vec<Invitation>>()
        .into_iter()
        .map(|link| link.id)
        .collect()
}

/// Who adds the members of a board invites a guest to it, and nobody else does: the
/// refusals say what the user may know of the board, and no more.
#[tokio::test]
async fn who_adds_the_members_of_a_board_asks_a_guest_to_it() {
    let th = Setup::native().start().await.init_basic().await;
    let third = th.add_user("third").await;
    let board = private_board(&th.client2, "Plans").await;
    let hidden = private_board(&th.client, "Not theirs").await;
    let elsewhere: Container = th
        .client2
        .post(
            &format!("/w/{}/boards", crate::common::TEST_WORKSPACE_ID),
            &json!({"workspaceId": crate::common::TEST_WORKSPACE_ID, "visibility": "private", "title": "Elsewhere"}),
        )
        .await
        .created_json();
    let template: Container = th
        .client2
        .post(
            &format!("/w/{ROOT_WORKSPACE_ID}/boards"),
            &json!({"workspaceId": ROOT_WORKSPACE_ID, "visibility": "private", "title": "A template", "isTemplate": true}),
        )
        .await
        .created_json();
    let before = links(&th).await;

    // What the request says.
    let no_board = th
        .client2
        .post(
            &format!("/w/{ROOT_WORKSPACE_ID}/invitations"),
            &json!({"role": "guest"}),
        )
        .await;
    no_board.check_bad_request();
    assert_eq!(no_board.refused_at(), "/containerId");
    let too_much = ask_for_a_guest_link(&th.client2, &board.id, Some("admin")).await;
    too_much.check_bad_request();
    assert_eq!(too_much.refused_at(), "/containerRole");
    let to_a_template = ask_for_a_guest_link(&th.client2, &template.id, None).await;
    to_a_template.check_bad_request();
    assert_eq!(to_a_template.refused_at(), "/containerId");

    // A board the user does not see, and one of another workspace, is not there.
    let absent = said(&ask_for_a_guest_link(&th.client2, crate::common::UNKNOWN_ID, None).await);
    assert_eq!(absent.0, StatusCode::NOT_FOUND);
    for (case, client, board_id) in [
        ("a board for the invited, to who is not", &third, &board.id),
        ("another's board for the invited", &th.client2, &hidden.id),
        ("a board of another workspace", &th.client2, &elsewhere.id),
    ] {
        let refused = said(&ask_for_a_guest_link(client, board_id, None).await);
        assert_eq!(refused, absent, "{case}");
    }
    // Who is on the board and does not add its members is refused.
    th.client2
        .add_member_in(
            &board.workspace_id,
            &fvoci_model::ContainerMember {
                container_id: board.id.clone(),
                user_id: third.me().await.id,
                role: fvoci_model::ContainerRole::Editor,
                synthetic: false,
                inherited_role: None,
            },
        )
        .await
        .check_ok();
    ask_for_a_guest_link(&third, &board.id, None)
        .await
        .check_error(
            StatusCode::FORBIDDEN,
            "only who adds the members of the board invites guests to it",
        );
    assert_eq!(links(&th).await, before, "a refused request makes no link");

    // An admin of the board, who is no administrator of the workspace; and an
    // administrator of the workspace, who is not on the board.
    let link = guest_link(&th.client2, &board.id).await;
    assert_eq!(link.role, fvoci_model::WorkspaceRole::Guest);
    assert_eq!(link.container_id.as_deref(), Some(board.id.as_str()));
    assert_eq!(
        link.container_role,
        Some(fvoci_model::ContainerRole::Editor),
        "unless said"
    );
    assert_eq!((link.max_uses, link.uses), (Some(1), 0), "for one person");
    assert_eq!(
        link.expires_at - link.created_at,
        INVITATION_LIFETIME_MILLIS
    );
    assert_eq!(link.token.as_ref().map(String::len), Some(43));
    let by_the_administrator: Invitation =
        ask_for_a_guest_link(&th.client, &board.id, Some("viewer"))
            .await
            .created_json();
    assert_eq!(
        by_the_administrator.container_role,
        Some(fvoci_model::ContainerRole::Viewer)
    );

    th.teardown().await;
}

/// Whoever accepts a link for a guest is on its board and a guest of the workspace: the
/// board and nothing else of it, and no member of its organization. The link lets one
/// in, and then answers as no link at all.
#[tokio::test]
async fn a_guest_comes_in_by_a_link_onto_one_board() {
    let th = Setup::native().start().await.init_basic().await;
    let board = private_board(&th.client2, "Plans").await;
    let other = private_board(&th.client2, "Not for guests").await;
    let link = guest_link(&th.client2, &board.id).await;
    let token = link.token.clone().expect("the token of a new link");
    let inviter = th.client2.me().await;

    let preview = th
        .anonymous()
        .post("/invitations/preview", &json!({"token": token}))
        .await;
    preview.check_ok();
    assert_eq!(
        preview.value(),
        json!({"workspaceName": ORG_NAME, "inviterName": inviter.username, "containerTitle": "Plans", "containerKind": "board"})
    );
    let members_of_the_organization = async || {
        th.app
            .store()
            .system()
            .texts_unchecked(
                "SELECT 'member' FROM organization_members AS m \
                 JOIN organizations AS o ON o.id = m.org_id WHERE o.owner_id IS NULL",
            )
            .await
            .expect("the members are counted")
            .len()
    };
    let before = members_of_the_organization().await;

    let accepted = th
        .anonymous()
        .post(
            "/invitations/accept",
            &json!({"token": token, "account": account("visitor")}),
        )
        .await;
    accepted.check_ok();
    assert_eq!(
        accepted.value(),
        json!({"workspaceId": ROOT_WORKSPACE_ID, "containerId": board.id, "containerKind": "board"})
    );
    let visitor = th.login("visitor", PASSWORD).await;
    assert_eq!(
        role_in_the_root_workspace(&visitor).await,
        Some(fvoci_model::WorkspaceRole::Guest)
    );
    assert_eq!(
        members_of_the_organization().await,
        before,
        "a guest is no member of the organization"
    );
    visitor.get(&container_path(&board)).await.check_ok();
    let member: fvoci_model::ContainerMember = visitor
        .get(&format!(
            "/w/{}/containers/{}/members",
            board.workspace_id, board.id
        ))
        .await
        .ok_json::<Vec<fvoci_model::ContainerMember>>()
        .into_iter()
        .find(|member| member.user_id != inviter.id)
        .expect("the guest is a member of the board");
    assert_eq!(
        member.role,
        fvoci_model::ContainerRole::Editor,
        "{member:?}"
    );
    visitor
        .get(&container_path(&other))
        .await
        .check_status(StatusCode::NOT_FOUND);
    let boards: Vec<Container> = visitor
        .get(&format!("/w/{ROOT_WORKSPACE_ID}/boards"))
        .await
        .ok_json();
    let titles: Vec<&str> = boards.iter().map(|board| board.title.as_str()).collect();
    assert_eq!(titles, ["Plans"], "the board of the link and no other");

    assert_guest_link_is_exhausted(&th, &visitor, &link.id, &token).await;

    th.teardown().await;
}

/// Verifies that a single-use guest invitation link answers 404 to every subsequent
/// preview or accept call.
async fn assert_guest_link_is_exhausted(
    th: &TestServer,
    visitor: &Client,
    link_id: &str,
    token: &str,
) {
    let no_link = said(
        &th.anonymous()
            .post("/invitations/preview", &json!({"token": "no-such-link"}))
            .await,
    );
    assert_eq!(no_link.0, StatusCode::NOT_FOUND, "unknown link answers 404");
    let again = [
        th.anonymous()
            .post("/invitations/preview", &json!({"token": token}))
            .await,
        th.anonymous()
            .post(
                "/invitations/accept",
                &json!({"token": token, "account": account("second")}),
            )
            .await,
        th.client
            .post("/invitations/accept", &json!({"token": token}))
            .await,
        visitor
            .post("/invitations/accept", &json!({"token": token}))
            .await,
    ];
    for answer in &again {
        assert_eq!(said(answer), no_link, "used up");
    }
    assert_eq!(
        of_the_link(th, link_id, "uses").await,
        ["1"],
        "guest link recorded one use"
    );
}

/// The links of a workspace are listed and revoked by its administrators, and by
/// anybody else their own; revoked, a link answers as no link at all.
#[tokio::test]
async fn links_are_listed_and_revoked_by_who_made_them_and_by_the_administrators() {
    let th = Setup::native().start().await.init_basic().await;
    let board = private_board(&th.client2, "Plans").await;
    let invitations = format!("/w/{ROOT_WORKSPACE_ID}/invitations");
    let already = listed(&th.client).await;

    let for_members = member_link(&th.client).await;
    let theirs = guest_link(&th.client2, &board.id).await;
    let another = guest_link(&th.client2, &board.id).await;
    let mut expected = vec![
        another.id.clone(),
        theirs.id.clone(),
        for_members.id.clone(),
    ];
    expected.extend(already.clone());
    assert_eq!(
        listed(&th.client).await,
        expected,
        "all of them, the newest first"
    );
    assert_eq!(
        listed(&th.client2).await,
        [another.id.clone(), theirs.id.clone()],
        "their own"
    );
    th.anonymous().get(&invitations).await.check_unauthorized();

    // A link that is not theirs to revoke answers as one that is not there.
    let revoke =
        async |client: &Client, id: &str| client.delete(&format!("{invitations}/{id}")).await;
    let absent = said(&revoke(&th.client2, crate::common::UNKNOWN_ID).await);
    assert_eq!(absent.0, StatusCode::NOT_FOUND);
    for id in [for_members.id.as_str(), "no identifier"] {
        assert_eq!(said(&revoke(&th.client2, id).await), absent, "{id}");
    }
    assert_eq!(listed(&th.client).await, expected, "nothing was revoked");

    // Their own, and any by an administrator.
    revoke(&th.client2, &theirs.id).await.check_no_content();
    assert_eq!(said(&revoke(&th.client2, &theirs.id).await), absent, "once");
    revoke(&th.client, &another.id).await.check_no_content();
    revoke(&th.client, &for_members.id).await.check_no_content();
    assert_eq!(listed(&th.client).await, already);
    assert_eq!(listed(&th.client2).await, Vec::<String>::new());

    // Revoked, each lets nobody in and shows nothing.
    let no_link = said(
        &th.anonymous()
            .post("/invitations/preview", &json!({"token": "no-such-link"}))
            .await,
    );
    for link in [&for_members, &theirs, &another] {
        let token = link.token.as_deref().expect("a token");
        let looked = th
            .anonymous()
            .post("/invitations/preview", &json!({"token": token}))
            .await;
        let accepted = th
            .anonymous()
            .post(
                "/invitations/accept",
                &json!({"token": token, "account": account("late")}),
            )
            .await;
        assert_eq!(said(&looked), no_link);
        assert_eq!(said(&accepted), no_link);
    }

    th.teardown().await;
}

/// Whether the link of `token` answers as no link at all, to who looks at it and to who
/// accepts it: status and body.
async fn lets_nobody_in(th: &TestServer, token: &str) -> bool {
    let no_link = said(
        &th.anonymous()
            .post("/invitations/preview", &json!({"token": "no-such-link"}))
            .await,
    );
    assert_eq!(no_link.0, StatusCode::NOT_FOUND, "{no_link:?}");
    let looked = th
        .anonymous()
        .post("/invitations/preview", &json!({"token": token}))
        .await;
    if looked.status == StatusCode::OK {
        return false;
    }
    assert_eq!(said(&looked), no_link, "looked at");
    // Accepting agrees, and makes no account.
    let accepted = th
        .anonymous()
        .post(
            "/invitations/accept",
            &json!({"token": token, "account": account("nobody")}),
        )
        .await;
    assert_eq!(said(&accepted), no_link, "accepted");
    true
}

/// A link for a guest lets nobody in from the moment who made it could not make it: the
/// maker stops adding the board's members, or the board goes. It answers as no link at
/// all then.
#[tokio::test]
async fn a_link_for_a_guest_lets_in_while_its_maker_may_still_invite() {
    let th = Setup::native().start().await.init_basic().await;
    let third = th.add_user("third").await;
    let board = private_board(&th.client2, "Plans").await;
    let token = guest_link(&th.client2, &board.id)
        .await
        .token
        .expect("a token");
    let users = th.app.registered_user_count().await.expect("the users");

    // The maker hands the board to somebody else and stays as an editor.
    let as_member = |user_id: &str, admin: bool| fvoci_model::ContainerMember {
        container_id: board.id.clone(),
        user_id: user_id.to_owned(),
        role: if admin {
            fvoci_model::ContainerRole::Admin
        } else {
            fvoci_model::ContainerRole::Editor
        },
        synthetic: false,
        inherited_role: None,
    };
    let third_id = third.me().await.id;
    let maker_id = th.client2.me().await.id;
    th.client2
        .add_member_in(&board.workspace_id, &as_member(&third_id, true))
        .await
        .check_ok();
    let role_of_the_maker = format!(
        "/w/{}/containers/{}/members/{maker_id}",
        board.workspace_id, board.id
    );
    third
        .put(&role_of_the_maker, &as_member(&maker_id, false))
        .await
        .check_ok();
    assert!(lets_nobody_in(&th, &token).await, "no admin of the board");
    // Listed all the same, to be revoked.
    assert_eq!(listed(&th.client2).await.len(), 1);
    third
        .put(&role_of_the_maker, &as_member(&maker_id, true))
        .await
        .check_ok();
    let look = th
        .anonymous()
        .post("/invitations/preview", &json!({"token": token}))
        .await;
    assert_eq!(look.status, StatusCode::OK, "an admin of the board again");

    // The board goes to the trash, and its links with it, for good.
    th.client2
        .delete(&container_path(&board))
        .await
        .check_no_content();
    assert!(lets_nobody_in(&th, &token).await, "no board");
    assert_eq!(listed(&th.client2).await, Vec::<String>::new());
    assert_eq!(
        th.app.registered_user_count().await.expect("the users"),
        users,
        "no account was made by a link that let nobody in"
    );

    th.teardown().await;
}

/// A workspace's administrators say whether guests are invited to its boards. Turned
/// off, no link for a guest is made and those there are revoked, for good; members are
/// invited as before, and who is a guest stays one.
#[tokio::test]
async fn a_workspace_stops_inviting_guests_and_its_links_for_them_go() {
    let th = Setup::native().start().await.init_basic().await;
    let board = private_board(&th.client2, "Plans").await;
    let used = guest_link(&th.client2, &board.id).await;
    th.anonymous()
        .post(
            "/invitations/accept",
            &json!({"token": used.token, "account": account("visitor")}),
        )
        .await
        .check_ok();
    let visitor = th.login("visitor", PASSWORD).await;
    let for_a_guest = guest_link(&th.client2, &board.id).await;
    let token = for_a_guest.token.clone().expect("a token");
    let mut links_for_members = listed(&th.client).await;
    links_for_members.retain(|id| id != &for_a_guest.id);
    let for_members = member_link(&th.client).await;
    links_for_members.insert(0, for_members.id.clone());
    assert!(!lets_nobody_in(&th, &token).await);

    let switch = format!("/w/{ROOT_WORKSPACE_ID}/guest-invitations");
    th.client2
        .put(&switch, &json!({"enabled": false}))
        .await
        .check_error(
            StatusCode::FORBIDDEN,
            "only the administrators of the workspace say whether guests are invited",
        );
    visitor
        .put(&switch, &json!({"enabled": false}))
        .await
        .check_status(StatusCode::NOT_FOUND);
    th.client.put(&switch, &json!({})).await.check_bad_request();
    assert!(!lets_nobody_in(&th, &token).await, "nothing was switched");

    let off = th.client.put(&switch, &json!({"enabled": false})).await;
    off.check_ok();
    assert_eq!(off.value()["guestInvitations"], json!(false));
    assert_eq!(off.value()["id"], json!(ROOT_WORKSPACE_ID));
    assert!(lets_nobody_in(&th, &token).await, "switched off");
    for client in [&th.client, &th.client2] {
        ask_for_a_guest_link(client, &board.id, None)
            .await
            .check_error(
                StatusCode::FORBIDDEN,
                "guests are not invited to the boards of this workspace",
            );
    }
    assert_eq!(
        listed(&th.client).await,
        links_for_members,
        "the links for members are as they were, the one for a guest is gone"
    );
    assert!(!lets_nobody_in(&th, for_members.token.as_deref().expect("a token")).await);
    // Who is a guest already stays one, on their board.
    visitor.get(&container_path(&board)).await.check_ok();

    let on = th.client.put(&switch, &json!({"enabled": true})).await;
    on.check_ok();
    assert_eq!(on.value()["guestInvitations"], json!(true));
    assert!(lets_nobody_in(&th, &token).await, "revoked for good");
    let anew = guest_link(&th.client2, &board.id).await;
    assert!(!lets_nobody_in(&th, anew.token.as_deref().expect("a token")).await);

    th.teardown().await;
}

/// A link raises what somebody is in the workspace and never lowers it: a member who
/// accepts a guest's link is on the board and a member still, a guest who accepts a
/// members' link is a member with the boards they were on, and who has all a link
/// gives uses nothing of it.
#[tokio::test]
async fn a_link_never_lowers_what_somebody_is_in_the_workspace() {
    let th = Setup::native().start().await.init_basic().await;
    let board = private_board(&th.client, "Plans").await;
    let role_on_the_board = async |client: &Client| -> Option<fvoci_model::ContainerRole> {
        let user_id = client.me().await.id;
        th.client
            .get(&format!(
                "/w/{}/containers/{}/members",
                board.workspace_id, board.id
            ))
            .await
            .ok_json::<Vec<fvoci_model::ContainerMember>>()
            .into_iter()
            .find(|member| member.user_id == user_id)
            .map(|member| member.role)
    };

    // The admin of the board accepts a link to it as a viewer: nothing changes, and
    // the link is still there for somebody else.
    let as_viewer: Invitation = ask_for_a_guest_link(&th.client, &board.id, Some("viewer"))
        .await
        .created_json();
    let accept = async |client: &Client, link: &Invitation| {
        client
            .post("/invitations/accept", &json!({"token": link.token}))
            .await
    };
    accept(&th.client, &as_viewer).await.check_ok();
    assert_eq!(
        role_on_the_board(&th.client).await,
        Some(fvoci_model::ContainerRole::Admin)
    );
    assert_eq!(
        role_in_the_root_workspace(&th.client).await,
        Some(fvoci_model::WorkspaceRole::Admin),
        "the administrator of the workspace is one still"
    );
    assert_eq!(of_the_link(&th, &as_viewer.id, "uses").await, ["0"]);

    // A member of the workspace accepts it: on the board, a member still.
    let accepted = accept(&th.client2, &as_viewer).await;
    accepted.check_ok();
    assert_eq!(
        accepted.value(),
        json!({"workspaceId": ROOT_WORKSPACE_ID, "containerId": board.id, "containerKind": "board"})
    );
    assert_eq!(
        role_in_the_root_workspace(&th.client2).await,
        Some(fvoci_model::WorkspaceRole::Member)
    );
    assert_eq!(
        role_on_the_board(&th.client2).await,
        Some(fvoci_model::ContainerRole::Viewer)
    );

    // A guest of the board accepts a link for members: a member, with the board.
    let as_editor = guest_link(&th.client, &board.id).await;
    th.anonymous()
        .post(
            "/invitations/accept",
            &json!({"token": as_editor.token, "account": account("visitor")}),
        )
        .await
        .check_ok();
    let visitor = th.login("visitor", PASSWORD).await;
    assert_eq!(
        role_in_the_root_workspace(&visitor).await,
        Some(fvoci_model::WorkspaceRole::Guest)
    );
    let for_members = member_link(&th.client).await;
    accept(&visitor, &for_members).await.check_ok();
    assert_eq!(
        role_in_the_root_workspace(&visitor).await,
        Some(fvoci_model::WorkspaceRole::Member)
    );
    assert_eq!(
        role_on_the_board(&visitor).await,
        Some(fvoci_model::ContainerRole::Editor)
    );
    assert_eq!(of_the_link(&th, &for_members.id, "uses").await, ["1"]);
    // And a guest's link after that does not make them a guest again.
    let again = guest_link(&th.client, &board.id).await;
    accept(&visitor, &again).await.check_ok();
    assert_eq!(
        role_in_the_root_workspace(&visitor).await,
        Some(fvoci_model::WorkspaceRole::Member)
    );
    assert_eq!(
        of_the_link(&th, &again.id, "uses").await,
        ["0"],
        "nothing to give"
    );

    th.teardown().await;
}

/// A workspace's administrator who manages a board and does not read it puts nobody on
/// it off the record: whoever accepts their link for a guest is recorded as added to
/// the board by them, as whom they add themselves is. A link of somebody who is on the
/// board is not recorded, and neither is an acceptance that gives no role on it.
#[tokio::test]
async fn a_guest_let_in_by_a_manager_who_does_not_read_the_board_is_on_the_record() {
    let th = Setup::native().start().await.init_basic().await;
    let (manager_id, member_id) = (th.client.me().await.id, th.client2.me().await.id);
    let board = private_board(&th.client2, "Not the administrator's").await;
    th.client
        .get(&format!(
            "/w/{}/boards/{}/cards",
            board.workspace_id, board.id
        ))
        .await
        .check_status(StatusCode::NOT_FOUND);
    let records = async || {
        th.app
            .store()
            .get_audit_log(ROOT_WORKSPACE_ID)
            .await
            .expect("the audit log is read")
            .into_iter()
            .map(|entry| {
                (
                    entry.actor.person_id().unwrap_or_default().to_owned(),
                    entry.action,
                    entry.target_id,
                    entry.subject_id,
                )
            })
            .collect::<Vec<_>>()
    };
    let accept = async |link: &Invitation, username: &str| -> String {
        let token = link.token.as_deref().expect("the token of a new link");
        th.anonymous()
            .post(
                "/invitations/accept",
                &json!({"token": token, "account": account(username)}),
            )
            .await
            .check_ok();
        th.login(username, PASSWORD).await.me().await.id
    };

    // By a link of who is on the board: nothing to record.
    let by_a_member = guest_link(&th.client2, &board.id).await;
    accept(&by_a_member, "asked-by-a-member").await;
    assert_eq!(records().await, [], "a member's guest is not recorded");

    // By a link of the administrator, who is not: recorded when it is accepted, not
    // when it is made.
    let by_the_manager = guest_link(&th.client, &board.id).await;
    assert_eq!(by_the_manager.created_by, manager_id);
    assert_eq!(records().await, [], "a link alone puts nobody on the board");
    let guest_id = accept(&by_the_manager, "asked-by-the-manager").await;
    assert_eq!(
        records().await,
        [(
            manager_id.clone(),
            fvoci_model::AuditAction::AddedToContainer,
            board.id.clone(),
            guest_id.clone(),
        )]
    );
    // The guest is on the board; the administrator still does not read it.
    let members = th
        .app
        .store()
        .get_members_for_container(&board.id)
        .await
        .expect("the members of the board");
    assert!(
        members.iter().any(|member| member.user_id == guest_id),
        "{members:?}"
    );
    assert!(
        members.iter().all(|member| member.user_id != manager_id),
        "{members:?}"
    );
    assert!(
        members.iter().any(|member| member.user_id == member_id),
        "{members:?}"
    );

    // Who has the role already is given none: another link of the administrator's,
    // accepted by the same guest, adds no record.
    let again = guest_link(&th.client, &board.id).await;
    let guest = th.login("asked-by-the-manager", PASSWORD).await;
    guest
        .post(
            "/invitations/accept",
            &json!({"token": again.token.as_deref().expect("a token")}),
        )
        .await
        .check_ok();
    assert_eq!(
        records().await.len(),
        1,
        "nothing was given, nothing recorded"
    );

    th.teardown().await;
}

/// Makes a wiki of the root workspace of `visibility` (`private` or `open`) titled `title`, as
/// `client`, who is its admin: its identifier.
async fn wiki_in_the_root(client: &Client, visibility: &str, title: &str) -> String {
    let made = client
        .post(
            &format!("/w/{ROOT_WORKSPACE_ID}/wikis"),
            &json!({"visibility": visibility, "title": title}),
        )
        .await;
    made.check_created();
    made.value()["id"].as_str().expect("an id").to_owned()
}

/// A guest is invited onto a wiki as onto a board: the link names the wiki, its maker
/// is who adds the wiki's members, the guest is at most an editor of it, and comes in
/// to that wiki and nothing else of the workspace, an open wiki among it. A wiki of
/// another workspace is no wiki to invite to.
#[tokio::test]
async fn a_guest_comes_in_by_a_link_onto_one_wiki() {
    let th = Setup::native().start().await.init_basic().await;
    let wiki = wiki_in_the_root(&th.client2, "private", "Handbook").await;
    let open = wiki_in_the_root(&th.client2, "open", "Open to the workspace").await;
    let elsewhere = th
        .client2
        .post(
            &format!("/w/{}/wikis", crate::common::TEST_WORKSPACE_ID),
            &json!({"visibility": "private"}),
        )
        .await
        .value()["id"]
        .as_str()
        .expect("an id")
        .to_owned();
    let refused = ask_for_a_guest_link(&th.client2, &wiki, Some("admin")).await;
    refused.check_bad_request();
    assert_eq!(refused.refused_at(), "/containerRole");
    let of_elsewhere = ask_for_a_guest_link(&th.client2, &elsewhere, None).await;
    let of_nothing = ask_for_a_guest_link(&th.client2, crate::common::UNKNOWN_ID, None).await;
    of_elsewhere.check_not_found();
    assert_eq!(of_elsewhere.problem(), of_nothing.problem());
    // Who reads the wiki and does not add its members makes no link to it.
    let reader = th.add_user("wiki.reader").await;
    let reader_id = reader.me().await.id;
    let member = json!({"containerId": wiki, "userId": reader_id, "role": "viewer"});
    th.client2
        .post(
            &format!("/w/{ROOT_WORKSPACE_ID}/containers/{wiki}/members"),
            &member,
        )
        .await
        .check_ok();
    ask_for_a_guest_link(&reader, &wiki, None)
        .await
        .check_forbidden();

    let token = guest_link(&th.client2, &wiki)
        .await
        .token
        .expect("the token of a new link");
    let preview: Value = th
        .anonymous()
        .post("/invitations/preview", &json!({"token": token}))
        .await
        .ok_json();
    assert_eq!(preview["containerTitle"], json!("Handbook"));
    assert_eq!(preview["containerKind"], json!("wiki"));
    let accepted = th
        .anonymous()
        .post(
            "/invitations/accept",
            &json!({"token": token, "account": account("wiki.guest")}),
        )
        .await;
    accepted.check_ok();
    assert_eq!(
        accepted.value(),
        json!({"workspaceId": ROOT_WORKSPACE_ID, "containerId": wiki, "containerKind": "wiki"})
    );
    let guest = th.login("wiki.guest", PASSWORD).await;
    assert_eq!(
        role_in_the_root_workspace(&guest).await,
        Some(fvoci_model::WorkspaceRole::Guest)
    );
    let wikis: Vec<Container> = guest
        .get(&format!("/w/{ROOT_WORKSPACE_ID}/wikis"))
        .await
        .ok_json();
    let wikis: Vec<String> = wikis.into_iter().map(|wiki| wiki.id).collect();
    assert_eq!(wikis, std::slice::from_ref(&wiki), "the invited wiki alone");
    let pages = format!("/w/{ROOT_WORKSPACE_ID}/wikis/{wiki}/pages");
    guest
        .post(&pages, &json!({"title": "by the guest"}))
        .await
        .check_created();
    for not_theirs in [open.as_str(), elsewhere.as_str()] {
        guest
            .get(&format!("/w/{ROOT_WORKSPACE_ID}/wikis/{not_theirs}/pages"))
            .await
            .check_not_found();
    }
    let boards: Vec<Container> = guest
        .get(&format!("/w/{ROOT_WORKSPACE_ID}/boards"))
        .await
        .ok_json();
    assert!(boards.is_empty(), "{boards:?}");
    th.teardown().await;
}
