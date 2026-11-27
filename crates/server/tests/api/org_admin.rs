//! The routes an organization's administrators manage it by, under `/org/{orgId}`, and
//! what their doing so is to the members of its workspaces: a workspace they did not
//! join is not there for them, joining it is recorded where its members read it,
//! stopping being an administrator takes it back, a workspace in the trash is not there
//! for anybody, and the public links of the organization close and open again.

use fvoci_model::{Container, ContainerVisibility, PublicLinkChange, UserWorkspace, WorkspaceRole};
use fvoci_store::DbType;
use fvoci_store::testing::db_type_of_the_tests;
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::common::{
    ApiResponse, Client, ROOT_WORKSPACE_ID, Setup, TEST_WORKSPACE_ID, TEST_WS, TestServer,
    UNKNOWN_ID,
};
use crate::files::{upload_in, uploaded_id};
use crate::loose_objects::{exists, remove_doomed};

/// The organization of the test server: the first user administers it.
pub(crate) async fn org_of(ts: &TestServer) -> String {
    let workspaces: Vec<UserWorkspace> = ts.client.get("/users/me/workspaces").await.ok_json();
    workspaces
        .first()
        .expect("user1 is in a workspace")
        .org_id
        .clone()
}

/// Makes a workspace through the route as the first user; returns its identifier.
pub(crate) async fn make_workspace(ts: &TestServer, org_id: &str, name: &str) -> String {
    let made: Value = ts
        .client
        .post(
            &format!("/org/{org_id}/workspaces"),
            &json!({ "name": name }),
        )
        .await
        .created_json();
    made["id"].as_str().expect("an identifier").to_owned()
}

/// Lets the user into the workspace with the role, past the routes.
pub(crate) async fn let_in(
    ts: &TestServer,
    workspace_id: &str,
    user_id: &str,
    role: WorkspaceRole,
) {
    ts.app
        .store()
        .add_workspace_member(workspace_id, user_id, role)
        .await
        .expect("the user is let into the workspace");
}

/// The workspaces the client's user is in, by identifier with their role.
async fn my_workspaces(client: &Client) -> Vec<(String, WorkspaceRole)> {
    let workspaces: Vec<UserWorkspace> = client.get("/users/me/workspaces").await.ok_json();
    workspaces
        .into_iter()
        .map(|workspace| (workspace.id, workspace.role))
        .collect()
}

/// The audit log of the workspace as the client reads it: `(action, actor, subject)`.
async fn audit_of(client: &Client, workspace_id: &str) -> Vec<(String, String, String)> {
    let records: Vec<Value> = client
        .get(&format!("/w/{workspace_id}/audit"))
        .await
        .ok_json();
    records
        .iter()
        .map(|record| {
            (
                record["action"].as_str().unwrap_or_default().to_owned(),
                record["actor"]["id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
                record["subject"]["id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
            )
        })
        .collect()
}

/// What a request with a read token is answered, without what identifies the request:
/// a link that is closed answers as one that never was.
fn unlinked(response: &ApiResponse) -> (StatusCode, Value) {
    let mut problem = response.value();
    if let Some(problem) = problem.as_object_mut() {
        problem.remove("instance");
        problem.remove("requestId");
    }
    (response.status, problem)
}

/// Every route of the organization, with a body where it takes one.
fn org_routes(
    org_id: &str,
    workspace_id: &str,
    user_id: &str,
) -> Vec<(&'static str, String, Value)> {
    let org = format!("/org/{org_id}");
    let workspace = format!("{org}/workspaces/{workspace_id}");
    vec![
        ("GET", org.clone(), Value::Null),
        ("PATCH", org.clone(), json!({ "publicLinks": false })),
        ("GET", format!("{org}/members"), Value::Null),
        ("DELETE", format!("{org}/members/{user_id}"), Value::Null),
        (
            "PUT",
            format!("{org}/members/{user_id}/role"),
            json!({ "role": "admin" }),
        ),
        ("GET", format!("{org}/workspaces"), Value::Null),
        (
            "POST",
            format!("{org}/workspaces"),
            json!({ "name": "Mine" }),
        ),
        ("PATCH", workspace.clone(), json!({ "name": "Renamed" })),
        ("DELETE", workspace.clone(), Value::Null),
        ("POST", format!("{workspace}/restore"), Value::Null),
        ("POST", format!("{workspace}/join"), Value::Null),
        ("POST", format!("{workspace}/leave"), Value::Null),
        ("GET", format!("{org}/audit"), Value::Null),
    ]
}

/// Whether every route of the organization answers the client 404, as for an
/// organization that is not there; the routes that list say why not.
async fn refused_everywhere(
    client: &Client,
    org_id: &str,
    workspace_id: &str,
    user_id: &str,
) -> Vec<String> {
    let mut reached = Vec::new();
    for (method, path, body) in org_routes(org_id, workspace_id, user_id) {
        let request = client.request(method.parse().expect("a method"), &path);
        let request = if body.is_null() {
            request
        } else {
            request.json(&body)
        };
        let response = Client::send(request).await;
        if response.status != StatusCode::NOT_FOUND {
            reached.push(format!("{method} {path}: {}", response.status));
        }
    }
    reached
}

/// A user who is a guest of the workspaces and no member of the organization.
async fn outside_guest(ts: &TestServer) -> Client {
    let guest = ts.add_user("guest").await;
    let guest_id = guest.me().await.id;
    for statement in [
        format!("DELETE FROM organization_members WHERE user_id = '{guest_id}'"),
        format!("UPDATE workspace_members SET role = 'guest' WHERE user_id = '{guest_id}'"),
    ] {
        ts.app
            .store()
            .execute_unchecked(&statement)
            .await
            .expect("the user is a guest from outside");
    }
    guest
}

/// What the records say was done, in their order.
fn actions(records: &[Value]) -> Vec<&str> {
    records
        .iter()
        .map(|record| record["action"].as_str().unwrap_or_default())
        .collect()
}

/// An administrator of the organization makes a workspace and renames it, a name in
/// use is refused, and the organization's audit log records it. A member, a guest from
/// outside the organization and a stranger are told the organization is not there.
#[tokio::test]
async fn an_org_admin_makes_and_renames_a_workspace_and_nobody_else_reaches_the_routes() {
    let ts = Setup::native().start().await.init_basic().await;
    let org_id = org_of(&ts).await;
    let made = make_workspace(&ts, &org_id, "  Design  ").await;
    let renamed: Value = ts
        .client
        .patch(
            &format!("/org/{org_id}/workspaces/{made}"),
            &json!({ "name": "Product" }),
        )
        .await
        .ok_json();
    let taken = ts
        .client
        .post(
            &format!("/org/{org_id}/workspaces"),
            &json!({ "name": "Product" }),
        )
        .await;
    let blank = ts
        .client
        .post(
            &format!("/org/{org_id}/workspaces"),
            &json!({ "name": "  " }),
        )
        .await;
    let listed: Vec<Value> = ts
        .client
        .get(&format!("/org/{org_id}/workspaces"))
        .await
        .ok_json();
    let recorded: Vec<Value> = ts
        .client
        .get(&format!("/org/{org_id}/audit"))
        .await
        .ok_json();

    let member = ts.client2.me().await.id;
    let guest = outside_guest(&ts).await;
    let by_member = refused_everywhere(&ts.client2, &org_id, &made, &member).await;
    let by_guest = refused_everywhere(&guest, &org_id, &made, &member).await;
    let unknown = refused_everywhere(&ts.client, UNKNOWN_ID, &made, &member).await;
    let anonymous = ts
        .anonymous()
        .get(&format!("/org/{org_id}/workspaces"))
        .await;
    let still: Vec<Value> = ts
        .client
        .get(&format!("/org/{org_id}/workspaces"))
        .await
        .ok_json();
    ts.teardown().await;

    assert_eq!(renamed["name"], json!("Product"));
    assert_eq!(
        renamed["memberCount"],
        json!(0),
        "nobody is in it, its maker neither"
    );
    assert_eq!(renamed["role"], Value::Null);
    taken.check_status(StatusCode::CONFLICT);
    assert_eq!(taken.error_reason(), "workspace_name_taken");
    blank.check_bad_request();
    let names: Vec<(&str, bool)> = listed
        .iter()
        .map(|workspace| {
            (
                workspace["name"].as_str().unwrap_or_default(),
                workspace["isDefault"].as_bool().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(names.len(), 4, "{listed:?}");
    assert_eq!(names[3], ("Product", false));
    assert!(
        listed
            .iter()
            .any(|workspace| workspace["id"] == json!(ROOT_WORKSPACE_ID)
                && workspace["isDefault"] == json!(true)),
        "{listed:?}"
    );
    assert_eq!(
        actions(&recorded),
        ["workspace_renamed", "workspace_created"]
    );
    assert_eq!(recorded[0]["targetName"], json!("Product"));
    assert!(by_member.is_empty(), "a member reached: {by_member:?}");
    assert!(by_guest.is_empty(), "a guest reached: {by_guest:?}");
    assert!(unknown.is_empty(), "no organization: {unknown:?}");
    anonymous.check_unauthorized();
    assert_eq!(still.len(), 4, "what the others asked changed nothing");
}

/// The administrator of another organization reaches none of this one's routes.
#[tokio::test]
async fn the_administrator_of_another_organization_reaches_none_of_its_routes() {
    if db_type_of_the_tests() != DbType::Postgres {
        return;
    }
    let ts = Setup::native()
        .configure(|config| "open".clone_into(&mut config.org_creation))
        .start()
        .await
        .init_basic()
        .await;
    let ours = org_of(&ts).await;
    let made = make_workspace(&ts, &ours, "Ours").await;
    let theirs_workspace = fvoci_model::Workspace {
        id: fvoci_model::new_id(),
        ..fvoci_model::Workspace::default()
    };
    let theirs = ts
        .app
        .store()
        .create_organization("theirs", &theirs_workspace)
        .await
        .expect("a second organization");
    let their_admin = ts.client2.me().await.id;
    for table in ["workspace_members", "organization_members"] {
        ts.app
            .store()
            .execute_unchecked(&format!(
                "DELETE FROM {table} WHERE user_id = '{their_admin}'"
            ))
            .await
            .expect("the second user leaves our organization");
    }
    ts.app
        .store()
        .for_org(&theirs)
        .expect("an identifier")
        .add_organization_member(&their_admin, fvoci_model::OrgRole::Admin)
        .await
        .expect("the second user administers theirs");

    let own = ts.client2.get(&format!("/org/{theirs}/workspaces")).await;
    let reached = refused_everywhere(&ts.client2, &ours, &made, &their_admin).await;
    let across = ts
        .client2
        .post_empty(&format!("/org/{theirs}/workspaces/{made}/join"))
        .await;
    ts.teardown().await;

    own.check_ok();
    assert!(
        reached.is_empty(),
        "another organization's admin reached: {reached:?}"
    );
    across.check_not_found();
}

/// A workspace the administrator of the organization did not join is not there for
/// them; joined, it is, and its members read that it was joined. Who stops
/// administering the organization is taken out of it again, which is recorded too, and
/// a workspace they were a member of before keeps them with the role they had.
#[tokio::test]
async fn joining_a_workspace_is_recorded_for_its_members_and_taken_back_with_the_role() {
    let ts = Setup::native().start().await.init_basic().await;
    let org_id = org_of(&ts).await;
    let admin = ts.client.me().await.id;
    let member = ts.client2.me().await.id;
    let workspace = make_workspace(&ts, &org_id, "Research").await;
    let_in(&ts, &workspace, &member, WorkspaceRole::Member).await;
    let path = format!("/w/{workspace}");

    let before = ts.client.get(&path).await;
    let boards_before = ts.client.get(&format!("{path}/boards")).await;
    let joined: Value = ts
        .client
        .post_empty(&format!("/org/{org_id}/workspaces/{workspace}/join"))
        .await
        .ok_json();
    let after = ts.client.get(&path).await;
    let read_by_member = audit_of(&ts.client2, &workspace).await;
    // In a workspace they were a member of, the joining keeps what they were.
    ts.app
        .store()
        .execute_unchecked(&format!(
            "UPDATE workspace_members SET role = 'member' WHERE user_id = '{admin}' \
             AND workspace_id = '{TEST_WORKSPACE_ID}'"
        ))
        .await
        .expect("the admin is a member of the test workspace");
    ts.client
        .post_empty(&format!(
            "/org/{org_id}/workspaces/{TEST_WORKSPACE_ID}/join"
        ))
        .await
        .check_ok();
    let as_admin_there = my_workspaces(&ts.client).await;

    // The second user is made an administrator of the organization and makes the
    // first one a member.
    ts.client
        .put(
            &format!("/org/{org_id}/members/{member}/role"),
            &json!({ "role": "admin" }),
        )
        .await
        .check_no_content();
    ts.client2
        .put(
            &format!("/org/{org_id}/members/{admin}/role"),
            &json!({ "role": "member" }),
        )
        .await
        .check_no_content();
    let revoked = ts.client.get(&path).await;
    let read_after = audit_of(&ts.client2, &workspace).await;
    let as_member_there = my_workspaces(&ts.client).await;
    let no_longer = ts.client.get(&format!("/org/{org_id}/workspaces")).await;
    ts.teardown().await;

    before.check_not_found();
    boards_before.check_not_found();
    assert_eq!(joined["role"], json!("admin"));
    assert_eq!(joined["joinedAsOrgAdmin"], json!(true));
    after.check_ok();
    assert_eq!(
        read_by_member,
        [("org_admin_joined".to_owned(), admin.clone(), String::new())]
    );
    assert!(
        as_admin_there.contains(&(TEST_WORKSPACE_ID.to_owned(), WorkspaceRole::Admin)),
        "{as_admin_there:?}"
    );
    revoked.check_not_found();
    assert_eq!(
        read_after,
        [
            (
                "org_admin_revoked".to_owned(),
                member.clone(),
                admin.clone()
            ),
            ("org_admin_joined".to_owned(), admin.clone(), String::new()),
        ],
        "the newest first"
    );
    assert!(
        as_member_there.contains(&(TEST_WORKSPACE_ID.to_owned(), WorkspaceRole::Member)),
        "the role they had before: {as_member_there:?}"
    );
    assert!(
        !as_member_there.iter().any(|(id, _)| *id == workspace),
        "{as_member_there:?}"
    );
    no_longer.check_not_found();
}

/// The organization's last administrator neither steps down nor is made a member.
#[tokio::test]
async fn the_last_administrator_of_the_organization_stays_one() {
    let ts = Setup::native().start().await.init_basic().await;
    let org_id = org_of(&ts).await;
    let admin = ts.client.me().await.id;
    let refused = ts
        .client
        .put(
            &format!("/org/{org_id}/members/{admin}/role"),
            &json!({ "role": "member" }),
        )
        .await;
    let unknown = ts
        .client
        .put(
            &format!("/org/{org_id}/members/{UNKNOWN_ID}/role"),
            &json!({ "role": "admin" }),
        )
        .await;
    let still = ts.client.get(&format!("/org/{org_id}")).await;
    ts.teardown().await;

    refused.check_status(StatusCode::CONFLICT);
    assert_eq!(refused.error_reason(), "last_org_admin");
    unknown.check_not_found();
    still.check_ok();
}

/// A board of a new workspace with a file, made by the first user, who joined it.
async fn board_with_a_file(ts: &TestServer, workspace_id: &str) -> (Container, String) {
    let board: Container = ts
        .client
        .create_board(&Container {
            workspace_id: workspace_id.to_owned(),
            visibility: ContainerVisibility::Open,
            title: "Plans".to_owned(),
            ..Container::default()
        })
        .await
        .created_json();
    let response = upload_in(
        &ts.client,
        workspace_id,
        &board.id,
        "plan.txt",
        b"the plan\n",
    )
    .await;
    response.check_created();
    (board, uploaded_id(&response))
}

/// A workspace in the trash is not there for its members: not listed, its routes and
/// its boards 404, its public links too. Brought back, it is as it was. The default
/// workspace is not put in the trash, and a name in the trash may be taken.
#[tokio::test]
async fn a_workspace_in_the_trash_is_not_there_and_comes_back_as_it_was() {
    let ts = Setup::local_mode().start().await.init_basic().await;
    let org_id = org_of(&ts).await;
    let member = ts.client2.me().await.id;
    let workspace = make_workspace(&ts, &org_id, "Ops").await;
    let place = format!("/org/{org_id}/workspaces/{workspace}");
    ts.client
        .post_empty(&format!("{place}/join"))
        .await
        .check_ok();
    let_in(&ts, &workspace, &member, WorkspaceRole::Member).await;
    let (board, file_id) = board_with_a_file(&ts, &workspace).await;
    let token = ts.share(&board.id).await;
    let shared = format!("/shared/{}?readToken={token}", board.id);
    let at = format!("/w/{workspace}/containers/{}", board.id);
    let file = format!("{at}/files/{file_id}");
    ts.anonymous().get(&shared).await.check_ok();

    ts.client.delete(&place).await.check_no_content();
    let gone = [
        ts.client2.get(&format!("/w/{workspace}")).await,
        ts.client2.get(&format!("/w/{workspace}/boards")).await,
        ts.client2.get(&format!("/w/{workspace}/audit")).await,
        ts.client2.get(&at).await,
        ts.client.get(&file).await,
    ];
    let link_closed = ts.anonymous().get(&shared).await;
    let no_link = ts
        .anonymous()
        .get(&format!("/shared/{}?readToken=wrong", board.id))
        .await;
    let listed = my_workspaces(&ts.client2).await;
    let again = ts.client.delete(&place).await;
    let default = ts
        .client
        .delete(&format!("/org/{org_id}/workspaces/{ROOT_WORKSPACE_ID}"))
        .await;
    let in_the_trash: Vec<Value> = ts
        .client
        .get(&format!("/org/{org_id}/workspaces"))
        .await
        .ok_json();
    let restored: Value = ts
        .client
        .post_empty(&format!("{place}/restore"))
        .await
        .ok_json();
    let back = [
        ts.client2.get(&format!("/w/{workspace}")).await,
        ts.client2.get(&at).await,
        ts.client.get(&file).await,
        ts.anonymous().get(&shared).await,
    ];
    let listed_again = my_workspaces(&ts.client2).await;
    ts.teardown().await;

    for response in &gone {
        response.check_not_found();
    }
    assert_eq!(
        unlinked(&link_closed),
        unlinked(&no_link),
        "its link answers as a link that never was"
    );
    assert!(!listed.iter().any(|(id, _)| *id == workspace), "{listed:?}");
    again.check_not_found();
    default.check_status(StatusCode::CONFLICT);
    assert_eq!(default.error_reason(), "default_workspace");
    let trashed = in_the_trash
        .iter()
        .find(|listed| listed["id"] == json!(workspace))
        .expect("listed in the trash");
    assert!(
        trashed["deletedAt"].as_i64().unwrap_or_default() > 0,
        "{trashed}"
    );
    assert_eq!(restored.get("deletedAt"), None);
    assert_eq!(restored["memberCount"], json!(2), "its members are there");
    for response in &back {
        response.check_ok();
    }
    assert!(listed_again.contains(&(workspace.clone(), WorkspaceRole::Member)));
}

/// A workspace that has been in the trash longer than the trash keeps what is deleted
/// is removed for good by the housekeeping, and the object of its board's file goes the
/// way of the files of a board removed for good.
#[tokio::test]
async fn the_housekeeping_removes_a_workspace_in_the_trash_for_good_with_its_files() {
    let ts = Setup::native().start().await.init_basic().await;
    let org_id = org_of(&ts).await;
    let workspace = make_workspace(&ts, &org_id, "Old").await;
    let place = format!("/org/{org_id}/workspaces/{workspace}");
    ts.client
        .post_empty(&format!("{place}/join"))
        .await
        .check_ok();
    let (board, file_id) = board_with_a_file(&ts, &workspace).await;
    let path = ts
        .app
        .store()
        .get_file_record(&board.id, &file_id)
        .await
        .expect("the file is recorded")
        .path;
    ts.client.delete(&place).await.check_no_content();
    fvoci_server::keep_house(&ts.app).await;
    let kept = exists(&ts, &path).await;
    ts.client
        .post_empty(&format!("{place}/restore"))
        .await
        .check_ok();
    ts.client.delete(&place).await.check_no_content();
    // Long ago: past any time the trash keeps what is deleted.
    let aged = ts
        .app
        .store()
        .execute_unchecked(&format!(
            "UPDATE workspaces SET deleted_at = 1 WHERE id = '{workspace}'"
        ))
        .await
        .expect("the deletion is made an old one");
    fvoci_server::keep_house(&ts.app).await;
    remove_doomed(&ts, i64::MAX).await;
    let listed: Vec<Value> = ts
        .client
        .get(&format!("/org/{org_id}/workspaces"))
        .await
        .ok_json();
    let record = ts.app.store().get_file_record(&board.id, &file_id).await;
    let object = exists(&ts, &path).await;
    let restore = ts.client.post_empty(&format!("{place}/restore")).await;
    ts.teardown().await;

    assert!(kept, "a workspace just put in the trash keeps its files");
    assert_eq!(aged, 1);
    assert!(
        !listed.iter().any(|listed| listed["id"] == json!(workspace)),
        "{listed:?}"
    );
    assert!(record.is_err(), "the file's record went: {record:?}");
    assert!(!object, "the file's object went");
    restore.check_not_found();
}

/// Turned off, the public links of the organization open nothing and no board is
/// shared anew; stopping a board's sharing is not refused. Turned on again, the links
/// made before open again. The organization's audit log records both.
#[tokio::test]
async fn the_public_links_of_the_organization_close_and_open_again() {
    let ts = Setup::local_mode().start().await.init_basic().await;
    let org_id = org_of(&ts).await;
    let shared_board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let other_board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let stopped_board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let token = ts.share(&shared_board.id).await;
    ts.share(&stopped_board.id).await;
    let link = format!("/shared/{}?readToken={token}", shared_board.id);
    ts.anonymous().get(&link).await.check_ok();

    let off: Value = ts
        .client
        .patch(&format!("/org/{org_id}"), &json!({ "publicLinks": false }))
        .await
        .ok_json();
    let closed = ts.anonymous().get(&link).await;
    let no_link = ts
        .anonymous()
        .get(&format!("/shared/{}?readToken=wrong", shared_board.id))
        .await;
    let share = ts
        .client
        .post(
            &format!("{TEST_WS}/containers/{}/public-link", other_board.id),
            &json!({ "enabled": true }),
        )
        .await;
    let past_the_route = ts
        .app
        .set_public_link(&other_board.id, PublicLinkChange::On, "")
        .await;
    let stop = ts
        .client
        .post(
            &format!("{TEST_WS}/containers/{}/public-link", stopped_board.id),
            &json!({ "enabled": false }),
        )
        .await;
    let on: Value = ts
        .client
        .patch(&format!("/org/{org_id}"), &json!({ "publicLinks": true }))
        .await
        .ok_json();
    let open_again = ts.anonymous().get(&link).await;
    let recorded: Vec<Value> = ts
        .client
        .get(&format!("/org/{org_id}/audit"))
        .await
        .ok_json();
    ts.teardown().await;

    assert_eq!(off["publicLinks"], json!(false));
    assert_eq!(
        unlinked(&closed),
        unlinked(&no_link),
        "a closed link answers as a link that never was"
    );
    share.check_forbidden();
    assert_eq!(share.error_reason(), "public_links_off");
    assert!(past_the_route.is_err(), "{past_the_route:?}");
    stop.check_ok();
    assert_eq!(on["publicLinks"], json!(true));
    open_again.check_ok();
    assert_eq!(
        actions(&recorded),
        ["public_links_enabled", "public_links_disabled"]
    );
}

/// `PATCH /org/{orgId}` and `PATCH /org/{orgId}/workspaces/{workspaceId}` enforce the
/// same character rules as registration, rejecting control, invisible, and Unicode
/// bidi-override characters with 400 Bad Request.
#[tokio::test]
async fn org_and_workspace_rename_reject_control_invisible_and_bidi_characters() {
    let ts = Setup::native().start().await.init_basic().await;
    let org_id = org_of(&ts).await;
    let ws_id = make_workspace(&ts, &org_id, "Clean Name").await;

    for bad in [
        "Evil\u{202E}Name",
        "Zero\u{200B}Width",
        "Isolate\u{2066}Text",
        "Control\u{0007}Bell",
        "Line\nBreak",
    ] {
        let org_resp = ts
            .client
            .patch(&format!("/org/{org_id}"), &json!({ "name": bad }))
            .await;
        org_resp.check_bad_request();
        assert_eq!(
            org_resp.refused_at(),
            "/name",
            "org rename rejected {bad:?}"
        );

        let ws_resp = ts
            .client
            .patch(
                &format!("/org/{org_id}/workspaces/{ws_id}"),
                &json!({ "name": bad }),
            )
            .await;
        ws_resp.check_bad_request();
        assert_eq!(
            ws_resp.refused_at(),
            "/name",
            "workspace rename rejected {bad:?}"
        );
    }
    let blank_ws = ts
        .client
        .patch(
            &format!("/org/{org_id}/workspaces/{ws_id}"),
            &json!({ "name": "   " }),
        )
        .await;
    blank_ws.check_bad_request();
    assert_eq!(
        blank_ws.refused_at(),
        "/name",
        "workspace rename rejects blank name"
    );

    ts.teardown().await;
}
