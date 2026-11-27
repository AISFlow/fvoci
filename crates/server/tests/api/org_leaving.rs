//! Leaving an organization at the account's own asking (`leaveOrganization`): what the
//! member who leaves and the others are told, what stands in the way, and how somebody
//! who left comes back through the organization's provider.

use std::time::Duration;

use futures_util::{SinkExt as _, StreamExt as _};
use fvoci_model::{Container, OrgKind, OrgRole, UserOrganization, UserWorkspace};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

use crate::common::{
    ApiResponse, Client, ROOT_WORKSPACE_ID, Setup, TestServer, UNKNOWN_ID, WebSocket,
};
use crate::fake_oidc::{FakeProvider, Person};
use crate::oidc::{Browser, Outcome, org_of, server_for, set_provider};
use crate::sso::{through_provider, tie};

/// How long a connection is waited on for what it is to be told.
const WAIT: Duration = Duration::from_secs(5);

/// The subject the member has at the fake provider.
const MEMBER_SUBJECT: &str = "subject-of-user2";

/// What `client` is answered asking to leave the organization `org_id`.
async fn leave(client: &Client, org_id: &str) -> ApiResponse {
    client
        .post(&format!("/org/{org_id}/leave"), &json!({}))
        .await
}

/// A connection of `client`'s that follows the root workspace.
async fn following_the_root(ts: &TestServer, client: &Client) -> WebSocket {
    let mut socket = client.connect_websocket().await;
    let subscribe = json!({"type": "subscribeWorkspace", "workspaceId": ROOT_WORKSPACE_ID});
    socket
        .send(Message::Text(subscribe.to_string().into()))
        .await
        .expect("the subscription is sent");
    crate::common::carried_out(ts.app.ws(), &mut socket).await;
    socket
}

/// The first message with the action that the connection is told.
async fn told(socket: &mut WebSocket, action: &str) -> Value {
    tokio::time::timeout(WAIT, async {
        while let Some(frame) = socket.next().await {
            if let Message::Text(text) = frame.expect("a websocket frame") {
                let message: Value = serde_json::from_str(&text).expect("JSON");
                if message["type"] == action {
                    return message;
                }
            }
        }
        panic!("the connection ended before {action}");
    })
    .await
    .unwrap_or_else(|_| panic!("{action} is told"))
}

/// The organizations that people work in together that `client`'s account is listed
/// workspaces of, by identifier.
async fn shared_organizations_of(client: &Client) -> Vec<String> {
    let listed: Vec<UserWorkspace> = client.get("/users/me/workspaces").await.ok_json();
    listed
        .into_iter()
        .filter(|workspace| workspace.kind == OrgKind::Shared)
        .map(|workspace| workspace.org_id)
        .collect()
}

/// Who is in which workspace and organization as what, past the routes.
async fn every_place(ts: &TestServer) -> Vec<String> {
    ts.app
        .store()
        .system()
        .texts_unchecked(
            "SELECT 'org ' || org_id || ' ' || user_id || ' ' || role \
             FROM organization_members UNION ALL \
             SELECT 'workspace ' || workspace_id || ' ' || user_id || ' ' || role \
             FROM workspace_members ORDER BY 1",
        )
        .await
        .expect("read")
}

/// The member leaves: the connection of theirs that follows the organization's
/// workspace is told it follows it no more and stays open, a board they alone
/// administered is announced to who follows the workspace with its new admin, and the
/// workspace, the organization and the leaving are not there for them from then on. Their
/// session stays.
#[tokio::test]
async fn a_member_who_leaves_is_told_and_finds_the_organization_no_more() {
    let ts = Setup::native().start().await.init_basic().await;
    let org_id = org_of(&ts).await;
    let (admin_id, member_id) = (ts.client.me().await.id, ts.client2.me().await.id);
    let board: Container = ts
        .client2
        .post(
            &format!("/w/{ROOT_WORKSPACE_ID}/boards"),
            &json!({"workspaceId": ROOT_WORKSPACE_ID, "visibility": "private", "title": "The member's"}),
        )
        .await
        .created_json();
    let mut other_tab = following_the_root(&ts, &ts.client2).await;
    let mut admins = following_the_root(&ts, &ts.client).await;

    leave(&ts.client2, &org_id).await.check_no_content();

    let ended = told(&mut other_tab, "workspaceUnsubscribed").await;
    assert_eq!(ended["workspaceId"], ROOT_WORKSPACE_ID, "{ended}");
    let passed = told(&mut admins, "containerMemberChanged").await;
    assert_eq!(
        (
            &passed["member"]["containerId"],
            &passed["member"]["userId"]
        ),
        (&json!(board.id), &json!(admin_id)),
        "{passed}"
    );
    crate::common::carried_out(ts.app.ws(), &mut other_tab).await;
    ts.client2
        .get(&format!("/w/{ROOT_WORKSPACE_ID}"))
        .await
        .check_not_found();
    assert_eq!(
        shared_organizations_of(&ts.client2).await,
        Vec::<String>::new()
    );
    assert_eq!(ts.client2.me().await.id, member_id, "the session stays");
    leave(&ts.client2, &org_id).await.check_not_found();
    ts.teardown().await;
}

/// What stands in the way of leaving is refused with its reason, and nothing changes:
/// the server's only administrator stays in its first organization, the only
/// administrator of one with somebody else in it stays, and so does its only person.
#[tokio::test]
async fn what_stands_in_the_way_of_leaving_is_refused_and_changes_nothing() {
    let ts = Setup::native().start().await.init_basic().await;
    let org_id = org_of(&ts).await;
    let member_id = ts.client2.me().await.id;
    let mut refusals = Vec::new();
    let mut refused = async |label: &str| {
        let before = every_place(&ts).await;
        let answer = leave(&ts.client, &org_id).await;
        assert_eq!(every_place(&ts).await, before, "{label}: nothing changes");
        assert_eq!(answer.status.as_u16(), 409, "{label}: {}", answer.text());
        let problem = answer.value();
        refusals.push((
            label.to_owned(),
            problem["reason"].clone(),
            problem["type"].clone(),
        ));
    };

    refused("the server's only administrator").await;
    // Somebody else runs the server.
    ts.app
        .store()
        .execute_unchecked(&format!(
            "UPDATE users SET is_admin = true WHERE id = '{member_id}'"
        ))
        .await
        .expect("another administrator of the server");
    refused("the organization's only administrator").await;
    leave(&ts.client2, &org_id).await.check_no_content();
    // Who left the server's own organization runs the server no more.
    let made = ts
        .client2
        .put(
            &format!("/users/{member_id}/admin"),
            &json!({"isAdmin": true}),
        )
        .await;
    assert_eq!(
        (made.status.as_u16(), made.error_reason()),
        (403, "admin_required".to_owned()),
        "{}",
        made.text()
    );
    // Somebody else runs it from outside the organization.
    ts.app
        .store()
        .execute_unchecked(&format!(
            "UPDATE users SET is_admin = true WHERE id = '{member_id}'"
        ))
        .await
        .expect("an administrator of the server outside the organization");
    refused("the organization's only person").await;

    assert_eq!(
        refusals,
        [
            (
                "the server's only administrator".to_owned(),
                json!("last_instance_admin"),
                json!("/problems/last-instance-admin")
            ),
            (
                "the organization's only administrator".to_owned(),
                json!("sole_org_admin"),
                json!("/problems/sole-org-admin")
            ),
            (
                "the organization's only person".to_owned(),
                json!("alone_in_organization"),
                json!("/problems/alone-in-organization")
            ),
        ]
    );
    ts.teardown().await;
}

/// Who leaves is put out of the rooms of the organization's cards as it leaves, not at
/// the next periodic asking; who stays goes on writing.
#[tokio::test]
async fn who_leaves_is_put_out_of_the_rooms_at_once() {
    let ts = crate::rooms_changes::seldom()
        .start()
        .await
        .init_basic()
        .await;
    let org_id = org_of(&ts).await;
    let body = crate::rooms::paragraphs(&["first"]);
    let (table, mut one, mut two) = crate::rooms::two_in_a_room(&ts, &body).await;
    let collab = ts.app.collab();
    assert_eq!(collab.room_members(&ts.app, &table.card_id).await, 2);
    leave(&ts.client2, &org_id).await.check_no_content();
    // Out of the room as soon as the leaving has answered; who stays is left.
    assert_eq!(collab.room_members(&ts.app, &table.card_id).await, 1);
    let left = two.text("docLeft").await;
    assert_eq!(
        left,
        json!({"type": "docLeft", "doc": two.doc, "reason": "not_found"})
    );
    one.write_acked("after the member left").await;
    ts.teardown().await;
}

/// Somebody who left an organization whose provider lets newcomers in (`autoJoin`)
/// comes back as a member by signing in through it, with no invitation; where it lets
/// nobody new in, they are refused as a newcomer is. This pins what is so now.
#[tokio::test]
async fn who_left_comes_back_through_the_provider_as_a_newcomer_would() {
    let fake = FakeProvider::start().await;
    let ts = server_for(&fake).await;
    let org_id = org_of(&ts).await;
    set_provider(&ts, &org_id, &fake).await;
    let member_id = ts.client2.me().await.id;
    tie(&ts, &ts.client2, &fake, MEMBER_SUBJECT).await;
    leave(&ts.client2, &org_id).await.check_no_content();
    let closed =
        json!({"issuer": fake.issuer, "clientId": crate::fake_oidc::CLIENT_ID, "autoJoin": false});
    ts.client
        .put(&format!("/org/{org_id}/oidc"), &closed)
        .await
        .check_ok();
    fake.sign_in_as(Person::new(
        MEMBER_SUBJECT,
        &format!("{MEMBER_SUBJECT}@example.com"),
    ));
    let refused = Browser::new(&ts, &fake).sign_in(&org_id).await;

    set_provider(&ts, &org_id, &fake).await;
    let back = through_provider(&ts, &fake, &org_id, MEMBER_SUBJECT).await;

    assert_eq!(refused, Outcome::Refused("oidc-join-closed".to_owned()));
    assert_eq!(back.me().await.id, member_id, "the same account");
    assert_eq!(
        shared_organizations_of(&back).await,
        [org_id],
        "a member again"
    );
    ts.teardown().await;
    fake.stop();
}

/// An SSO-only account without a password is refused when leaving (409
/// `password_required`, nothing changed) until it sets a password; meanwhile an
/// organization administrator can still remove a passwordless SSO-only account.
#[tokio::test]
async fn passwordless_sso_account_cannot_leave_until_password_is_set_but_admin_can_remove_it() {
    let fake = FakeProvider::start().await;
    let ts = server_for(&fake).await;
    let org_id = org_of(&ts).await;
    set_provider(&ts, &org_id, &fake).await;

    let sso_one = through_provider(&ts, &fake, &org_id, "sso-no-pass-1").await;
    let sso_one_id = sso_one.me().await.id;
    let me_one: Value = sso_one.get("/users/me").await.ok_json();
    assert_eq!(me_one["hasPassword"], json!(false));

    let before = every_place(&ts).await;
    let refused = leave(&sso_one, &org_id).await;
    assert_eq!(refused.status.as_u16(), 409, "{}", refused.text());
    let problem = refused.check_problem();
    assert_eq!(problem["reason"], "password_required", "{problem}");
    assert_eq!(problem["type"], "/problems/password-required", "{problem}");
    assert_eq!(every_place(&ts).await, before, "nothing changed");

    // Admin can still remove the passwordless SSO-only member.
    ts.client
        .delete(&format!("/org/{org_id}/members/{sso_one_id}"))
        .await
        .check_no_content();
    let orgs: Vec<UserOrganization> = sso_one.get("/users/me/organizations").await.ok_json();
    assert!(orgs.is_empty(), "removed from organization: {orgs:?}");

    // Another passwordless SSO-only member sets a password and then can leave.
    let sso_two = through_provider(&ts, &fake, &org_id, "sso-no-pass-2").await;
    let refused = leave(&sso_two, &org_id).await;
    assert_eq!(refused.status.as_u16(), 409, "{}", refused.text());
    assert_eq!(refused.error_reason(), "password_required");

    let set = sso_two
        .put(
            "/users/me/password",
            &json!({ "password": "A-strong-password-1!" }),
        )
        .await;
    set.check_no_content();
    let cookie = set.session_cookie().expect("new session cookie");
    let sso_two = ts.client_with_token(&cookie);
    leave(&sso_two, &org_id).await.check_no_content();
    let orgs: Vec<UserOrganization> = sso_two.get("/users/me/organizations").await.ok_json();
    assert!(orgs.is_empty(), "left after setting password: {orgs:?}");

    ts.teardown().await;
    fake.stop();
}

/// Removing a member via `DELETE /org/{orgId}/members/{userId}` refuses self-removal
/// (409 `cannot_remove_self`) and unknown members (404), evicts the member from all
/// workspaces and collab rooms within 1 second (even under a 1-hour periodic recheck),
/// sends `workspaceUnsubscribed`, hands over sole-admin boards under the admin's name,
/// revokes admin joinings, and records `org_member_removed` in the org audit log.
#[tokio::test]
async fn org_admin_removes_member_with_realtime_eviction_and_audit() {
    let ts = crate::rooms_changes::seldom()
        .start()
        .await
        .init_basic()
        .await;
    let org_id = org_of(&ts).await;
    let (admin_id, member_id) = (ts.client.me().await.id, ts.client2.me().await.id);

    // Self-removal is 409 cannot_remove_self; unknown user is 404.
    let self_remove = ts
        .client
        .delete(&format!("/org/{org_id}/members/{admin_id}"))
        .await;
    assert_eq!(self_remove.status.as_u16(), 409, "{}", self_remove.text());
    let problem = self_remove.check_problem();
    assert_eq!(problem["reason"], "cannot_remove_self", "{problem}");
    assert_eq!(problem["type"], "/problems/cannot-remove-self", "{problem}");
    ts.client
        .delete(&format!("/org/{org_id}/members/{UNKNOWN_ID}"))
        .await
        .check_not_found();

    // Give member a private board (sole admin), org admin role + a joined workspace.
    let private_board: Container = ts
        .client2
        .post(
            &format!("/w/{ROOT_WORKSPACE_ID}/boards"),
            &json!({"workspaceId": ROOT_WORKSPACE_ID, "visibility": "private", "title": "Removed member board"}),
        )
        .await
        .created_json();
    ts.client
        .put(
            &format!("/org/{org_id}/members/{member_id}/role"),
            &json!({ "role": "admin" }),
        )
        .await
        .check_no_content();
    let second_ws: Value = ts
        .client
        .post(
            &format!("/org/{org_id}/workspaces"),
            &json!({ "name": "Ops" }),
        )
        .await
        .created_json();
    let second_ws_id = second_ws["id"].as_str().expect("workspace id").to_owned();
    ts.client2
        .post_empty(&format!("/org/{org_id}/workspaces/{second_ws_id}/join"))
        .await
        .check_ok();

    // Put both in a collab room (under 1-hour periodic recheck) and follow WS.
    let body = crate::rooms::paragraphs(&["shared"]);
    let (table, mut one, mut two) = crate::rooms::two_in_a_room(&ts, &body).await;
    let mut member_ws = following_the_root(&ts, &ts.client2).await;
    let mut admin_ws = following_the_root(&ts, &ts.client).await;

    ts.client
        .delete(&format!("/org/{org_id}/members/{member_id}"))
        .await
        .check_no_content();

    // Collab room evicts within 1 second (1-hour recheck would not have fired).
    assert_eq!(
        ts.app.collab().room_members(&ts.app, &table.card_id).await,
        1
    );
    let doc_left = tokio::time::timeout(Duration::from_secs(1), two.text("docLeft"))
        .await
        .expect("docLeft arrived within 1 second");
    assert_eq!(
        doc_left,
        json!({"type": "docLeft", "doc": two.doc, "reason": "not_found"})
    );
    one.write_acked("admin keeps writing").await;

    // WS receives workspaceUnsubscribed and board handover containerMemberChanged.
    let unsub = told(&mut member_ws, "workspaceUnsubscribed").await;
    assert_eq!(unsub["workspaceId"], ROOT_WORKSPACE_ID, "{unsub}");
    let passed = told(&mut admin_ws, "containerMemberChanged").await;
    assert_eq!(
        (
            &passed["member"]["containerId"],
            &passed["member"]["userId"]
        ),
        (&json!(private_board.id), &json!(admin_id)),
        "{passed}"
    );

    // Removed member can no longer reach workspace, sees no orgs, and repeat removal is 404.
    ts.client2
        .get(&format!("/w/{ROOT_WORKSPACE_ID}"))
        .await
        .check_not_found();
    let member_orgs: Vec<UserOrganization> =
        ts.client2.get("/users/me/organizations").await.ok_json();
    assert!(member_orgs.is_empty(), "{member_orgs:?}");
    ts.client
        .delete(&format!("/org/{org_id}/members/{member_id}"))
        .await
        .check_not_found();

    assert_removal_audit_logs(&ts.client, &org_id, &second_ws_id, &admin_id, &member_id).await;
    ts.teardown().await;
}

/// Verifies `org_member_removed` in the org audit log and `org_admin_revoked` in the
/// joined workspace's audit log after removing a member.
async fn assert_removal_audit_logs(
    admin: &Client,
    org_id: &str,
    joined_ws_id: &str,
    admin_id: &str,
    member_id: &str,
) {
    let org_audit: Vec<Value> = admin.get(&format!("/org/{org_id}/audit")).await.ok_json();
    let entry = org_audit
        .iter()
        .find(|r| r["action"] == "org_member_removed")
        .expect("org_member_removed in org audit log");
    assert_eq!(entry["actor"]["id"], admin_id, "org audit actor");
    assert_eq!(entry["subject"]["id"], member_id, "org audit subject");
    assert_eq!(entry["targetId"], org_id, "org audit target");

    admin
        .post_empty(&format!("/org/{org_id}/workspaces/{joined_ws_id}/join"))
        .await
        .check_ok();
    let ws_audit: Vec<Value> = admin
        .get(&format!("/w/{joined_ws_id}/audit"))
        .await
        .ok_json();
    let revoked = ws_audit
        .iter()
        .find(|r| r["action"] == "org_admin_revoked")
        .expect("org_admin_revoked in joined workspace audit");
    assert_eq!(revoked["actor"]["id"], admin_id, "revoked actor");
    assert_eq!(revoked["subject"]["id"], member_id, "revoked subject");
}

/// `GET /users/me/organizations` lists shared organizations with their real name and
/// role, including an organization where the user has been removed from every workspace,
/// and excludes personal spaces and guest-only organizations.
#[tokio::test]
async fn get_my_organizations_returns_shared_orgs_including_zero_workspace_orgs() {
    let ts = Setup::native().start().await.init_basic().await;
    let org_id = org_of(&ts).await;
    let member_id = ts.client2.me().await.id;
    ts.client
        .patch(&format!("/org/{org_id}"), &json!({ "name": "Acme Corp" }))
        .await
        .check_ok();

    let admin_orgs: Vec<UserOrganization> =
        ts.client.get("/users/me/organizations").await.ok_json();
    assert_eq!(
        admin_orgs,
        vec![UserOrganization {
            id: org_id.clone(),
            name: "Acme Corp".to_owned(),
            role: OrgRole::Admin,
        }]
    );

    // Remove user2 from every shared workspace of the org.
    let member_workspaces: Vec<UserWorkspace> =
        ts.client2.get("/users/me/workspaces").await.ok_json();
    for ws in member_workspaces
        .into_iter()
        .filter(|ws| ws.kind == OrgKind::Shared)
    {
        if ws.id != ROOT_WORKSPACE_ID {
            ts.client
                .post_empty(&format!("/org/{org_id}/workspaces/{}/join", ws.id))
                .await
                .check_ok();
        }
        ts.client
            .delete(&format!("/w/{}/members/{member_id}", ws.id))
            .await
            .check_no_content();
    }
    assert!(
        shared_organizations_of(&ts.client2).await.is_empty(),
        "user2 has zero shared workspaces left"
    );
    let member_orgs: Vec<UserOrganization> =
        ts.client2.get("/users/me/organizations").await.ok_json();
    assert_eq!(
        member_orgs,
        vec![UserOrganization {
            id: org_id.clone(),
            name: "Acme Corp".to_owned(),
            role: OrgRole::Member,
        }],
        "zero-workspace org is still listed so the user can leave it"
    );

    // A workspace guest who is no organization member gets an empty list.
    let guest = ts.add_user("org-list-guest").await;
    let guest_id = guest.me().await.id;
    for stmt in [
        format!("DELETE FROM organization_members WHERE user_id = '{guest_id}'"),
        format!("UPDATE workspace_members SET role = 'guest' WHERE user_id = '{guest_id}'"),
    ] {
        ts.app
            .store()
            .execute_unchecked(&stmt)
            .await
            .expect("guest setup");
    }
    let guest_orgs: Vec<UserOrganization> = guest.get("/users/me/organizations").await.ok_json();
    assert!(
        guest_orgs.is_empty(),
        "guest-only org excluded: {guest_orgs:?}"
    );

    ts.teardown().await;
}
