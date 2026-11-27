//! Leaving an organization (`leaveOrganization`) asked about one the asker is not in: by
//! a member or an administrator of another organization, by a guest of its workspace,
//! about an own space, and without a session. Each is answered as about an organization
//! that is not there, and nobody's place changes. On SQLite, which holds one
//! organization that people work in together, every row is run but the other
//! organization's.

use base64::Engine as _;
use fvoci_model::{OrgKind, UserWorkspace, Workspace, WorkspaceRole, new_id};
use fvoci_server::auth::sealed::SecretKey;
use fvoci_store::DbType;
use reqwest::StatusCode;
use serde_json::json;

use super::common::{ApiResponse, Client, ROOT_WORKSPACE_ID, Setup, TestServer, UNKNOWN_ID};
use super::fake_oidc::{CLIENT_ID, CLIENT_SECRET, FakeProvider};
use super::{answer, db_type_of_the_tests};

/// The organization that people work in together and the own space of the account of
/// `client`, by identifier, as its list of workspaces has them.
async fn organizations_of(client: &Client) -> (Option<String>, String) {
    let listed: Vec<UserWorkspace> = client.get("/users/me/workspaces").await.ok_json();
    let shared = listed
        .iter()
        .find(|workspace| workspace.kind == OrgKind::Shared)
        .map(|workspace| workspace.org_id.clone());
    let space = listed
        .iter()
        .find(|workspace| workspace.kind == OrgKind::Personal)
        .expect("an own space is listed");
    (shared, space.org_id.clone())
}

/// An account that is a guest of the root workspace and no member of its organization.
async fn a_guest(ts: &TestServer) -> Client {
    let guest = ts.add_user("leaving-guest").await;
    let guest_id = guest.me().await.id;
    let store = ts.app.store();
    for statement in [
        format!("DELETE FROM workspace_members WHERE user_id = '{guest_id}'"),
        format!("DELETE FROM organization_members WHERE user_id = '{guest_id}'"),
    ] {
        store
            .execute_unchecked(&statement)
            .await
            .expect("the account is no member of the organization");
    }
    store
        .add_workspace_member(ROOT_WORKSPACE_ID, &guest_id, WorkspaceRole::Guest)
        .await
        .expect("a guest of the root workspace");
    guest
}

/// An account that is `role` (`admin` or `member`) of another organization, and no
/// member of the first.
async fn of_another_organization(ts: &TestServer, username: &str, role: &str) -> Client {
    let client = ts.add_user(username).await;
    let user_id = client.me().await.id;
    let store = ts.app.store();
    for table in ["workspace_members", "organization_members"] {
        store
            .execute_unchecked(&format!("DELETE FROM {table} WHERE user_id = '{user_id}'"))
            .await
            .expect("the account leaves the first organization");
    }
    let workspace = Workspace {
        id: new_id(),
        ..Workspace::default()
    };
    let org_id = store
        .create_organization(&format!("Elsewhere of {username}"), &workspace)
        .await
        .expect("another organization");
    store
        .for_org(&org_id)
        .expect("an identifier")
        .execute_unchecked(&format!(
            "INSERT INTO organization_members (org_id, user_id, role, created_at) \
             VALUES ('{org_id}', '{user_id}', '{role}', 1)"
        ))
        .await
        .expect("in the other organization");
    client
}

/// Who is in which organization as what, past the routes.
async fn every_place(ts: &TestServer) -> Vec<String> {
    ts.app
        .store()
        .system()
        .texts_unchecked(
            "SELECT org_id || ' ' || user_id || ' ' || role FROM organization_members \
             ORDER BY 1",
        )
        .await
        .expect("read")
}

/// What `client` is answered asking to leave the organization `org_id`.
async fn leave(client: &Client, org_id: &str) -> ApiResponse {
    client
        .post(&format!("/org/{org_id}/leave"), &json!({}))
        .await
}

#[tokio::test]
async fn who_is_not_in_an_organization_does_not_leave_it() {
    let ts = Setup::native().start().await.init_basic().await;
    let (org, founders_space) = organizations_of(&ts.client).await;
    let org = org.expect("the first organization");
    let (_, members_space) = organizations_of(&ts.client2).await;
    let guest = a_guest(&ts).await;
    let mut askers = vec![
        ("a guest of its workspace", guest, org.clone()),
        (
            "its member, about another's own space",
            ts.client.clone(),
            members_space.clone(),
        ),
        (
            "its member, about their own space",
            ts.client2.clone(),
            members_space,
        ),
        (
            "its administrator, about their own space",
            ts.client.clone(),
            founders_space,
        ),
    ];
    if db_type_of_the_tests() == DbType::Postgres {
        for (who, username, role) in [
            (
                "a member of another organization",
                "member-elsewhere",
                "member",
            ),
            (
                "an administrator of another organization",
                "admin-elsewhere",
                "admin",
            ),
        ] {
            let client = of_another_organization(&ts, username, role).await;
            askers.push((who, client, org.clone()));
        }
    }
    let before = every_place(&ts).await;

    let mut violations = Vec::new();
    for (who, client, org_id) in &askers {
        let (asked, unknown) = (leave(client, org_id).await, leave(client, UNKNOWN_ID).await);
        if asked.status != StatusCode::NOT_FOUND || answer(&asked) != answer(&unknown) {
            violations.push(format!(
                "{who}: {:?}, and about nothing {:?}",
                answer(&asked),
                answer(&unknown)
            ));
        }
    }
    let nobody = ts.anonymous();
    let (asked, unknown) = (leave(&nobody, &org).await, leave(&nobody, UNKNOWN_ID).await);
    let after = every_place(&ts).await;
    ts.teardown().await;

    assert!(violations.is_empty(), "{}", violations.join("\n"));
    assert_eq!(asked.status, StatusCode::UNAUTHORIZED, "without a session");
    assert_eq!(answer(&asked), answer(&unknown), "without a session");
    assert_eq!(after, before, "nobody's place changes");
}

/// What `client` is answered asking to remove `user_id` from `org_id`.
async fn remove_member(client: &Client, org_id: &str, user_id: &str) -> ApiResponse {
    client
        .delete(&format!("/org/{org_id}/members/{user_id}"))
        .await
}

/// Builds the matrix of callers who must receive 404 (identical to an unknown org)
/// when calling `DELETE /org/{orgId}/members/{userId}`.
async fn non_admin_removal_askers(
    ts: &TestServer,
    org: &str,
    founders_space: String,
    members_space: String,
    founder_id: &str,
    member_id: &str,
) -> Vec<(&'static str, Client, String, String)> {
    let guest = a_guest(ts).await;
    let mut askers = vec![
        (
            "a non-admin member of the organization",
            ts.client2.clone(),
            org.to_owned(),
            founder_id.to_owned(),
        ),
        (
            "a guest of its workspace",
            guest,
            org.to_owned(),
            member_id.to_owned(),
        ),
        (
            "its administrator, about their own space",
            ts.client.clone(),
            founders_space,
            founder_id.to_owned(),
        ),
        (
            "its administrator, about another's own space",
            ts.client.clone(),
            members_space.clone(),
            member_id.to_owned(),
        ),
        (
            "its member, about their own space",
            ts.client2.clone(),
            members_space,
            member_id.to_owned(),
        ),
    ];
    if db_type_of_the_tests() == DbType::Postgres {
        for (who, username, role) in [
            (
                "a member of another organization",
                "remove-member-elsewhere",
                "member",
            ),
            (
                "an administrator of another organization",
                "remove-admin-elsewhere",
                "admin",
            ),
        ] {
            let client = of_another_organization(ts, username, role).await;
            askers.push((who, client, org.to_owned(), member_id.to_owned()));
        }
    }
    askers
}

/// Removing a member (`DELETE /org/{orgId}/members/{userId}`) is refused as 404
/// (identical to an unknown organization) for a non-admin member, a workspace guest,
/// a member or administrator of another organization, and for any personal space;
/// 401 without a session; and 401 `sso_required` when an administrator's password
/// session tries to cross an organization's SSO wall. Nothing changes in any case.
#[tokio::test]
async fn who_does_not_administer_an_organization_cannot_remove_its_members() {
    let fake = FakeProvider::start().await;
    let ts = Setup::native()
        .trusting_oidc_provider(&fake.certificate_der)
        .configure(|config| {
            let key = base64::engine::general_purpose::STANDARD.encode([42_u8; 32]);
            config.secret_key = Some(SecretKey::from_base64(&key).expect("a key"));
            config.oidc_allow_private_addresses = true;
        })
        .start()
        .await
        .init_basic()
        .await;
    let (org, founders_space) = organizations_of(&ts.client).await;
    let org = org.expect("the first organization");
    let (_, members_space) = organizations_of(&ts.client2).await;
    let founder_id = ts.client.me().await.id;
    let member_id = ts.client2.me().await.id;
    let askers = non_admin_removal_askers(
        &ts,
        &org,
        founders_space,
        members_space,
        &founder_id,
        &member_id,
    )
    .await;
    let before = every_place(&ts).await;

    let mut violations = Vec::new();
    for (who, client, org_id, target_id) in &askers {
        let asked = remove_member(client, org_id, target_id).await;
        let unknown = remove_member(client, UNKNOWN_ID, target_id).await;
        if asked.status != StatusCode::NOT_FOUND || answer(&asked) != answer(&unknown) {
            violations.push(format!(
                "{who}: {:?}, and about nothing {:?}",
                answer(&asked),
                answer(&unknown)
            ));
        }
    }

    let nobody = ts.anonymous();
    let unauth = remove_member(&nobody, &org, &member_id).await;
    let unauth_unknown = remove_member(&nobody, UNKNOWN_ID, &member_id).await;
    let walled = enforce_sso_and_remove(&ts, &fake, &org, &member_id).await;
    let after = every_place(&ts).await;
    ts.teardown().await;
    fake.stop();

    assert!(violations.is_empty(), "{}", violations.join("\n"));
    assert_eq!(unauth.status, StatusCode::UNAUTHORIZED, "without a session");
    assert_eq!(
        answer(&unauth),
        answer(&unauth_unknown),
        "without a session"
    );
    assert_eq!(
        walled.status,
        StatusCode::UNAUTHORIZED,
        "SSO wall: {}",
        walled.text()
    );
    let problem = walled.check_problem();
    assert_eq!(problem["reason"], "sso_required", "{problem}");
    assert_eq!(problem["orgId"], org, "{problem}");
    assert_eq!(after, before, "nobody's place changes");
}

/// Configures the fake OIDC provider, enforces SSO on `org`, and attempts to remove
/// `member_id` with the administrator's password session.
async fn enforce_sso_and_remove(
    ts: &TestServer,
    fake: &FakeProvider,
    org: &str,
    member_id: &str,
) -> ApiResponse {
    ts.client
        .put(
            &format!("/org/{org}/oidc"),
            &json!({
                "issuer": fake.issuer,
                "clientId": CLIENT_ID,
                "clientSecret": CLIENT_SECRET,
                "enabled": true,
            }),
        )
        .await
        .check_ok();
    ts.app
        .store()
        .for_org(org)
        .expect("org")
        .execute_unchecked(&format!(
            "UPDATE oidc_providers SET sso_required = true WHERE org_id = '{org}'"
        ))
        .await
        .expect("sso enforced");
    remove_member(&ts.client, org, member_id).await
}
