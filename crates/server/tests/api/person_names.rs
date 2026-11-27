//! A person's name: its parts, which of them they are shown by, their title and
//! credentials, changed by them (`patchMe`) and seen by whoever sees them.

use fvoci_model::{
    ContainerVisibility, Credential, NameShown, OrgMember, Title, User, WorkspaceMember,
};
use serde_json::{Value, json};

use crate::board_broadcasts::WorkspaceSocket;
use crate::boards::{add_member, board_with_admin, editor, new_board};
use crate::common::{ROOT_WORKSPACE_ID, ROOT_WS, Setup, TEST_WORKSPACE_ID};
use crate::org_admin::org_of;

/// A person writes the parts of their name and chooses one to be shown by; whoever sees
/// them sees every part, and the name they chose.
#[tokio::test]
async fn a_person_names_themselves_and_others_see_the_name_they_chose() {
    let th = Setup::native().start().await.init_basic().await;
    let kim: User = th
        .client2
        .patch(
            "/users/me",
            &json!({
                "firstName": "영환", "lastName": "김", "latinFirstName": "Younghwan",
                "latinLastName": "Kim", "customName": "YH", "title": "dr",
                "credentials": ["phd", "kr_attorney"],
            }),
        )
        .await
        .ok_json();
    assert_eq!(
        kim.display_name, "김영환",
        "a new account is shown by its name"
    );
    assert_eq!(kim.name.title, Some(Title::Dr));
    assert_eq!(
        kim.name.credentials,
        [Credential::Phd, Credential::KrAttorney]
    );

    let profile: Value = th
        .client
        .get(&format!("{ROOT_WS}/users/{}", kim.id))
        .await
        .value();
    for (member, value) in [
        ("displayName", json!("김영환")),
        ("firstName", json!("영환")),
        ("latinLastName", json!("Kim")),
        ("customName", json!("YH")),
        ("nameShown", json!("name")),
        ("title", json!("dr")),
        ("credentials", json!(["phd", "kr_attorney"])),
    ] {
        assert_eq!(profile[member], value, "{member} of {profile}");
    }

    for (shown, want) in [
        ("latin_name", "Younghwan Kim"),
        ("custom_name", "YH"),
        ("username", ""),
    ] {
        let changed: User = th
            .client2
            .patch("/users/me", &json!({ "nameShown": shown }))
            .await
            .ok_json();
        assert_eq!(changed.display_name, want, "{shown}");
        let other: Vec<User> = th
            .client
            .post(&format!("{ROOT_WS}/users"), &[&kim.id])
            .await
            .ok_json();
        assert_eq!(other[0].display_name, want, "others see it: {shown}");
    }
    let cleared: User = th
        .client2
        .patch(
            "/users/me",
            &json!({"nameShown": "latin_name", "latinFirstName": null, "latinLastName": "  "}),
        )
        .await
        .ok_json();
    assert_eq!(cleared.display_name, "YH", "down the list");
    assert_eq!(cleared.name.name_shown, NameShown::LatinName);

    // Found by every part of the name.
    for query in ["영환", "김영환", "YH"] {
        let found: Vec<User> = th
            .client
            .get(&format!("{ROOT_WS}/users?search={query}"))
            .await
            .ok_json();
        assert_eq!(found.len(), 1, "{query}: {found:?}");
        assert_eq!(found[0].id, kim.id);
    }
    th.teardown().await;
}

/// A title or a credential that is no code of its list, the same credential twice, more
/// than three, or a choice that is none are refused, naming each, and change nothing.
#[tokio::test]
async fn codes_off_their_lists_are_refused() {
    let th = Setup::native().start().await.init_basic().await;
    let before = th.client2.me().await;
    for (body, pointers) in [
        (json!({"title": "sir"}), vec!["/title"]),
        (json!({"title": ["dr"]}), vec!["/title"]),
        (
            json!({"credentials": ["phd", "esq"]}),
            vec!["/credentials/1"],
        ),
        (
            json!({"credentials": ["phd", "phd"]}),
            vec!["/credentials/1"],
        ),
        (
            json!({"credentials": ["phd", "md", "mba", "ms"]}),
            vec!["/credentials"],
        ),
        (json!({"credentials": "phd"}), vec!["/credentials"]),
        (json!({"nameShown": "nickname"}), vec!["/nameShown"]),
        (
            json!({"firstName": "a\u{200B}", "displayName": "x", "title": "dr"}),
            vec!["/displayName", "/firstName"],
        ),
    ] {
        let response = th.client2.patch("/users/me", &body).await;
        response.check_bad_request();
        assert_eq!(
            response.refused_places(),
            pointers,
            "{body}: {}",
            response.text()
        );
    }
    assert_eq!(th.client2.me().await, before, "nothing changed");
    th.teardown().await;
}

/// The administrators' lists of the members of the organization and of the workspace
/// carry each member's name whole, with the title and credentials, as they are now.
#[tokio::test]
async fn the_lists_of_members_carry_the_title_and_credentials() {
    let th = Setup::native().start().await.init_basic().await;
    let kim: User = th
        .client2
        .patch(
            "/users/me",
            &json!({
                "firstName": "영환", "lastName": "김", "latinFirstName": "Younghwan",
                "title": "prof", "credentials": ["md", "phd"],
            }),
        )
        .await
        .ok_json();
    let org_id = org_of(&th).await;
    let org: Vec<OrgMember> = th
        .client
        .get(&format!("/org/{org_id}/members"))
        .await
        .ok_json();
    let workspace: Vec<WorkspaceMember> = th
        .client
        .get(&format!("/w/{ROOT_WORKSPACE_ID}/members"))
        .await
        .ok_json();
    let in_org = org
        .iter()
        .find(|member| member.user_id == kim.id)
        .expect("a member of the organization");
    let in_workspace = workspace
        .iter()
        .find(|member| member.user_id == kim.id)
        .expect("a member of the workspace");
    for (list, display_name, name) in [
        ("organization", &in_org.display_name, &in_org.name),
        ("workspace", &in_workspace.display_name, &in_workspace.name),
    ] {
        assert_eq!(display_name, "김영환", "{list}");
        assert_eq!(name, &kim.name, "{list}");
        assert_eq!(name.title, Some(Title::Prof), "{list}");
        assert_eq!(
            name.credentials,
            [Credential::Md, Credential::Phd],
            "{list}"
        );
    }
    th.teardown().await;
}

/// The `userChanged` messages among `messages`.
fn renames(messages: &[Value]) -> Vec<&Value> {
    messages
        .iter()
        .filter(|message| message["type"] == "userChanged")
        .collect()
}

/// A change of a name is told to whoever follows a workspace of the person: who they
/// are, not what their name is now, which the page reads by the rule of any reading.
#[tokio::test]
async fn a_change_of_a_name_is_told_to_the_members_who_follow_the_workspace() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;
    let user2 = th.client2.me().await;
    let mut watching =
        WorkspaceSocket::subscribe(&th, &th.client, TEST_WORKSPACE_ID, &user1.id).await;
    let mut own = WorkspaceSocket::subscribe(&th, &th.client2, TEST_WORKSPACE_ID, &user2.id).await;
    th.client2
        .patch("/users/me", &json!({"title": "dr"}))
        .await
        .check_ok();
    let told = watching.expect("userChanged", "/userId", &user2.id).await;
    assert_eq!(
        told,
        json!({"type": "userChanged", "workspaceId": TEST_WORKSPACE_ID, "userId": user2.id}),
        "who, and nothing of the name"
    );
    let contract = crate::websocket::read_contract("websocket.json");
    let openapi = crate::websocket::read_contract("openapi.json");
    crate::websocket::check_message(&contract, &openapi, &told);
    own.expect("userChanged", "/userId", &user2.id).await;
    // A request that changes nothing tells nothing: none, or the title it has.
    watching.drain().await;
    let before = th.client2.me().await;
    for body in [json!({}), json!({"title": "dr", "nameShown": null})] {
        th.client2.patch("/users/me", &body).await.check_ok();
    }
    assert_eq!(th.client2.me().await, before, "not even the time");
    assert_eq!(
        renames(&watching.drain().await),
        Vec::<&Value>::new(),
        "nothing changed"
    );
    th.teardown().await;
}

/// A guest of the workspace is told of the people they see there, who are on a board
/// with them, and not of the others.
#[tokio::test]
async fn a_guest_is_told_only_of_whom_they_see() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;
    let user2 = th.client2.me().await;
    th.app
        .store()
        .execute_unchecked(&format!(
            "UPDATE workspace_members SET role = 'guest' WHERE user_id = '{}' \
             AND workspace_id = '{TEST_WORKSPACE_ID}'",
            user2.id
        ))
        .await
        .expect("the second user is made a guest");
    let mut guest =
        WorkspaceSocket::subscribe(&th, &th.client2, TEST_WORKSPACE_ID, &user2.id).await;
    th.client
        .patch("/users/me", &json!({"credentials": ["phd"]}))
        .await
        .check_ok();
    assert_eq!(
        renames(&guest.drain().await),
        Vec::<&Value>::new(),
        "not on a board with the guest"
    );
    let board = board_with_admin(
        &th,
        new_board(TEST_WORKSPACE_ID, ContainerVisibility::Private, "Shared"),
        &user1.id,
    )
    .await;
    add_member(&th, &editor(&board.id, &user2.id)).await;
    guest.drain().await;
    th.client
        .patch("/users/me", &json!({"credentials": ["phd", "md"]}))
        .await
        .check_ok();
    guest.expect("userChanged", "/userId", &user1.id).await;
    th.teardown().await;
}
