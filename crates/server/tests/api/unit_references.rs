//! References to and from course units (`LinkedKind::Unit`): resolving, backlinks
//! (`getReferencesTo`), title search (`searchReferences`), chapter exclusion, and
//! isolation across private courses, guests, workspace admins, other workspaces, and own spaces.

use fvoci_model::{OrgKind, UserWorkspace};
use serde_json::{Value, json};

use crate::common::{Client, ROOT_WS, Setup, TEST_WORKSPACE_ID, TEST_WS};
use crate::references::{HIDDEN, check_as_nothing, let_read, made, named, resolved};
use crate::references_to::{body_naming, card_names, page_names};

fn id(value: &Value) -> String {
    value["id"].as_str().expect("an id").to_owned()
}

async fn make_course(
    client: &Client,
    at: &str,
    course_type: &str,
    title: &str,
) -> (String, String, String) {
    let course = client
        .post(
            &format!("{at}/courses"),
            &json!({"visibility": course_type, "title": format!("{title} course")}),
        )
        .await;
    course.check_created();
    let course = id(&course.value());
    let units = format!("{at}/courses/{course}/units");
    let chapter = client
        .post(
            &units,
            &json!({"kind": "chapter", "title": format!("{title} chapter")}),
        )
        .await;
    chapter.check_created();
    let chapter = id(&chapter.value());
    let unit = client
        .post(&units, &json!({"parentId": chapter, "title": title}))
        .await;
    unit.check_created();
    (course, chapter, id(&unit.value()))
}

async fn write_unit_body(client: &Client, at: &str, course: &str, unit: &str, body: &Value) {
    let path = format!("{at}/courses/{course}/units/{unit}");
    let read_at = client.get(&path).await.value()["updatedAt"].clone();
    client
        .patch(&path, &json!({"readAt": read_at, "body": body}))
        .await
        .check_ok();
}

/// `LinkedKind::Unit` resolves with `containerKind: "course"`, empty `icon`, and no
/// `number`; chapters are excluded; backlinks work from unit bodies, unit comments,
/// cards, and pages; and `searchReferences` finds live units only.
#[tokio::test]
async fn linked_kind_unit_works_in_resolve_backlinks_and_search() {
    let th = Setup::native().start().await.init_basic().await;
    let other = made(&th.client, TEST_WS, "private", "Reference Target").await;
    let (course, chapter, unit) =
        make_course(&th.client, TEST_WS, "private", "Ownership Basics").await;
    let second_unit = id(&th
        .client
        .post(
            &format!("{TEST_WS}/courses/{course}/units"),
            &json!({"parentId": chapter, "title": "Borrow Checker"}),
        )
        .await
        .value());

    // Resolve returns unit with kind="unit", containerKind="course", empty icon, no number;
    // chapter is silently omitted.
    let answer = resolved(&th.client, TEST_WS, &[&unit, &chapter, &other.card]).await;
    answer.check_ok();
    assert_eq!(
        answer.value(),
        json!([
            {"id": unit, "kind": "unit", "title": "Ownership Basics", "icon": "",
                "containerId": course, "containerKind": "course"},
            {"id": other.card, "kind": "card", "title": "Reference Target", "number": 1,
                "icon": "", "containerId": other.board, "containerKind": "board"},
        ])
    );

    // Backlinks: card body, page body, second unit body, and a comment on second unit
    // all reference `unit`.
    card_names(&th.client, TEST_WS, &other.card, &[&unit]).await;
    page_names(&th.client, TEST_WS, &other, &[&unit]).await;
    write_unit_body(
        &th.client,
        TEST_WS,
        &course,
        &second_unit,
        &body_naming("see", &[&unit]),
    )
    .await;
    th.client
        .post(
            &format!("{TEST_WS}/courses/{course}/units/{second_unit}/comments"),
            &json!({"body": body_naming("comment", &[&unit])}),
        )
        .await
        .check_created();

    let backlinks = th
        .client
        .get(&format!("{TEST_WS}/references/to/{unit}"))
        .await;
    assert_eq!(
        named(&backlinks),
        vec![other.card.clone(), other.page.clone(), second_unit.clone()],
        "backlinks include card, page, and unit (deduplicated with its comment)"
    );
    // Chapter has no backlinks (404).
    th.client
        .get(&format!("{TEST_WS}/references/to/{chapter}"))
        .await
        .check_not_found();

    // `searchReferences` finds live units and omits chapters & trashed units.
    let found = th
        .client
        .get(&format!("{TEST_WS}/references/search?q=Ownership"))
        .await;
    assert_eq!(named(&found), vec![unit.clone()]);
    let ch_search = th
        .client
        .get(&format!("{TEST_WS}/references/search?q=chapter"))
        .await;
    assert!(named(&ch_search).is_empty(), "chapters are never searched");

    // Trashing the unit removes it from resolve, backlinks, and search.
    th.client
        .delete(&format!("{TEST_WS}/courses/{course}/units/{unit}"))
        .await
        .check_no_content();
    check_as_nothing(&th.client, TEST_WS, &[&unit], &other.card, "trashed unit").await;
    th.client
        .get(&format!("{TEST_WS}/references/to/{unit}"))
        .await
        .check_not_found();
    let after_trash = th
        .client
        .get(&format!("{TEST_WS}/references/search?q=Ownership"))
        .await;
    assert_eq!(named(&after_trash), Vec::<String>::new());
    th.teardown().await;
}

/// Private courses are hidden from non-members (including workspace admins), open courses
async fn promote_to_workspace_admin(th: &crate::common::TestServer, user_id: &str) {
    th.app
        .store()
        .execute_unchecked(&format!(
            "UPDATE workspace_members SET role = 'admin' \
             WHERE workspace_id = '{TEST_WORKSPACE_ID}' AND user_id = '{user_id}'"
        ))
        .await
        .expect("promote client to workspace admin");
}

/// are visible to unjoined workspace members but hidden from guests, and units from other
/// workspaces or own spaces never leak.
#[tokio::test]
async fn unit_references_enforce_course_workspace_and_own_space_isolation() {
    let th = Setup::native().start().await.init_basic().await;
    let (private_course, _, private_unit) =
        make_course(&th.client, TEST_WS, "private", &format!("{HIDDEN} private")).await;
    let (_open_course, _, open_unit) = make_course(&th.client, TEST_WS, "open", "Open Unit").await;
    let (_root_course, _, root_unit) =
        make_course(&th.client, ROOT_WS, "open", &format!("{HIDDEN} root")).await;
    let known = made(&th.client2, TEST_WS, "private", "Known").await;

    // 1) Workspace member (client2) sees open_unit, never private_unit or root_unit.
    let open_res = resolved(&th.client2, TEST_WS, &[&open_unit]).await;
    assert_eq!(named(&open_res), vec![open_unit.clone()]);
    check_as_nothing(
        &th.client2,
        TEST_WS,
        &[&private_unit, &root_unit],
        &known.card,
        "member cannot see private or other-workspace unit",
    )
    .await;
    th.client2
        .get(&format!("{TEST_WS}/references/to/{private_unit}"))
        .await
        .check_not_found();
    let member_search = th
        .client2
        .get(&format!("{TEST_WS}/references/search?q={HIDDEN}"))
        .await;
    assert_eq!(named(&member_search), Vec::<String>::new());

    // 2) Workspace admin (client) lists private courses in the workspace, yet does NOT
    //    resolve an unjoined private course unit made by client2.
    let admin_id = th.client.me().await.id;
    promote_to_workspace_admin(&th, &admin_id).await;
    let (their_course, _, their_private_unit) = make_course(
        &th.client2,
        TEST_WS,
        "private",
        &format!("{HIDDEN} admin-blind"),
    )
    .await;
    let listed: Vec<Value> = th.client.get(&format!("{TEST_WS}/courses")).await.ok_json();
    assert!(
        listed.iter().any(|c| c["id"] == their_course),
        "workspace admin sees the private course in list"
    );
    check_as_nothing(
        &th.client,
        TEST_WS,
        &[&their_private_unit],
        &open_unit,
        "workspace admin cannot resolve unjoined private course unit",
    )
    .await;

    // 3) Guest cannot see open course unit or private course unit.
    let guest = th.add_user("course-ref-guest").await;
    let guest_id = guest.me().await.id;
    th.client
        .put(
            &format!("{TEST_WS}/members/{guest_id}"),
            &json!({"role": "guest"}),
        )
        .await
        .check_ok();
    let_read(&th.client2, &known.board, &guest_id).await;
    check_as_nothing(
        &guest,
        TEST_WS,
        &[&open_unit, &private_unit],
        &known.card,
        "guest cannot see open or uninvited private course unit",
    )
    .await;
    // Once invited to the private course as viewer, guest sees private_unit.
    let_read(&th.client, &private_course, &guest_id).await;
    assert_eq!(
        named(&resolved(&guest, TEST_WS, &[&private_unit]).await),
        vec![private_unit.clone()]
    );

    // 4) Own space unit is isolated from the organization workspace (even for the owner!).
    let workspaces: Vec<UserWorkspace> = th.client.get("/users/me/workspaces").await.ok_json();
    let own = workspaces
        .into_iter()
        .find(|w| w.kind == OrgKind::Personal)
        .expect("own space")
        .id;
    let own_ws = format!("/w/{own}");
    let (_own_course, _, own_unit) =
        make_course(&th.client, &own_ws, "private", &format!("{HIDDEN} own")).await;
    assert_eq!(
        named(&resolved(&th.client, &own_ws, &[&own_unit]).await),
        vec![own_unit.clone()]
    );
    check_as_nothing(
        &th.client,
        TEST_WS,
        &[&own_unit],
        &open_unit,
        "own-space unit does not resolve in organization workspace",
    )
    .await;
    th.client
        .get(&format!("{TEST_WS}/references/to/{own_unit}"))
        .await
        .check_not_found();
    th.teardown().await;
}
