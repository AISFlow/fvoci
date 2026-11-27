//! Body versions of a course unit: roles, `keepUnitBodyVersion` (including flushing an
//! open collaborative room), `restoreUnitBodyVersion`, file-ref preservation, missing-file
//! 409 conflict, chapter refusal, and trash/workspace isolation.

use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::common::{Client, ROOT_WS, Setup, TEST_WORKSPACE_ID, TEST_WS, UNKNOWN_ID};
use crate::files::{upload, uploaded_id};
use crate::rooms::{Peer, paragraphs};
use crate::wiki_isolation::put_on;

fn id(value: &Value) -> String {
    value["id"].as_str().expect("an id").to_owned()
}

fn unit_path(at: &str, course: &str, unit: &str) -> String {
    format!("{at}/courses/{course}/units/{unit}")
}

fn versions_path(at: &str, course: &str, unit: &str) -> String {
    format!("{}/body-versions", unit_path(at, course, unit))
}

async fn write_bodies(client: &Client, course: &str, unit: &str, bodies: &[Value]) {
    let at = unit_path(TEST_WS, course, unit);
    for body in bodies {
        let read = client.get(&at).await.value();
        client
            .patch(&at, &json!({"readAt": read["updatedAt"], "body": body}))
            .await
            .check_ok();
    }
}

async fn course_with_unit(client: &Client, course_type: &str) -> (String, String, String) {
    let course = client
        .post(
            &format!("{TEST_WS}/courses"),
            &json!({"visibility": course_type, "title": "Course"}),
        )
        .await;
    course.check_created();
    let course = id(&course.value());
    let units = format!("{TEST_WS}/courses/{course}/units");
    let chapter = client
        .post(&units, &json!({"kind": "chapter", "title": "Ch 1"}))
        .await;
    chapter.check_created();
    let chapter = id(&chapter.value());
    let unit = client
        .post(&units, &json!({"parentId": chapter, "title": "U 1"}))
        .await;
    unit.check_created();
    (course, chapter, id(&unit.value()))
}

/// Roles and open-course unjoined readers: readers list/get versions, editors/admins
/// keep by hand (including from an open collab room) and restore versions.
#[tokio::test]
async fn unit_body_versions_roles_keep_by_hand_and_restore() {
    let th = Setup::native().start().await.init_basic().await;
    let member = th.client2.me().await.id;
    let (course, _, unit) = course_with_unit(&th.client, "private").await;
    write_bodies(
        &th.client,
        &course,
        &unit,
        &[paragraphs(&["first"]), paragraphs(&["second"])],
    )
    .await;
    let versions = versions_path(TEST_WS, &course, &unit);

    // Non-member sees 404; viewer & commenter read (200) but cannot keep or restore (403).
    th.client2.get(&versions).await.check_not_found();
    for role in ["viewer", "commenter"] {
        put_on(&th.client, &course, &member, role).await;
        th.client2.get(&versions).await.check_ok();
        th.client2.get(&format!("{versions}/1")).await.check_ok();
        th.client2.post_empty(&versions).await.check_forbidden();
        th.client2
            .post_empty(&format!("{versions}/1/restore"))
            .await
            .check_forbidden();
    }

    // Editor restores version 1 and sees it become current.
    put_on(&th.client, &course, &member, "editor").await;
    let restored = th
        .client2
        .post_empty(&format!("{versions}/1/restore"))
        .await;
    restored.check_ok();
    assert_eq!(restored.value()["body"], paragraphs(&["first"]));
    assert_eq!(restored.value()["modifiedBy"], json!(member));

    // Keep by hand when unchanged returns `unchanged`.
    let unchanged = th.client2.post_empty(&versions).await;
    unchanged.check_ok();
    assert_eq!(unchanged.value()["outcome"], "unchanged");

    // Typing in a collab room then calling `keepUnitBodyVersion` flushes and keeps manual.
    let mut peer = Peer::connect(&th.client2, 401).await;
    peer.joined(TEST_WORKSPACE_ID, &unit).await;
    peer.write_acked("typed in room").await;
    let kept = th.client2.post_empty(&versions).await;
    kept.check_ok();
    assert_eq!(kept.value()["outcome"], "kept");
    assert_eq!(kept.value()["version"]["cause"], "manual");
    let manual_num = kept.value()["version"]["version"].as_i64().expect("number");
    let read_manual = th
        .client2
        .get(&format!("{versions}/{manual_num}"))
        .await
        .value();
    assert_eq!(
        read_manual["body"],
        paragraphs(&["first", "typed in room"]),
        "{read_manual}"
    );

    // Typing again within a minute and keeping returns `recent`.
    peer.write_acked("soon after").await;
    let recent = th.client2.post_empty(&versions).await;
    recent.check_ok();
    assert_eq!(recent.value()["outcome"], "recent");

    // Restoring while the collab room is open flushes and sends docReset to the peer.
    let reset_restore = th
        .client2
        .post_empty(&format!("{versions}/{manual_num}/restore"))
        .await;
    reset_restore.check_ok();
    let reset = peer.text("docReset").await;
    assert_eq!(reset["type"], "docReset", "{reset}");
    peer.settle().await;
    assert_eq!(peer.lines(), ["first", "typed in room"]);
    peer.leave().await;
    th.teardown().await;
}

/// Open course: unjoined workspace members read body versions (200) and cannot keep or
/// restore (403); workspace guests get 404 across all 4 body-version operations.
#[tokio::test]
async fn open_course_versions_distinguish_unjoined_member_and_guest() {
    let th = Setup::native().start().await.init_basic().await;
    let (open, _, open_unit) = course_with_unit(&th.client, "open").await;
    write_bodies(&th.client, &open, &open_unit, &[paragraphs(&["open"])]).await;
    let open_versions = versions_path(TEST_WS, &open, &open_unit);
    th.client2.get(&open_versions).await.check_ok();
    th.client2
        .get(&format!("{open_versions}/1"))
        .await
        .check_ok();
    th.client2
        .post_empty(&open_versions)
        .await
        .check_forbidden();
    th.client2
        .post_empty(&format!("{open_versions}/1/restore"))
        .await
        .check_forbidden();

    let guest = th.add_user("open-course-version-guest").await;
    let guest_id = guest.me().await.id;
    th.app
        .store()
        .execute_unchecked(&format!(
            "UPDATE workspace_members SET role = 'guest' \
             WHERE workspace_id = '{TEST_WORKSPACE_ID}' AND user_id = '{guest_id}'"
        ))
        .await
        .expect("demote to guest");
    guest.get(&open_versions).await.check_not_found();
    guest
        .get(&format!("{open_versions}/1"))
        .await
        .check_not_found();
    guest.post_empty(&open_versions).await.check_not_found();
    guest
        .post_empty(&format!("{open_versions}/1/restore"))
        .await
        .check_not_found();
    th.teardown().await;
}

/// Restoring a unit version keeps file references intact, refuses with 409 when a file
/// referenced by the older version was deleted, and rejects chapters/trash/wrong paths as 404.
#[tokio::test]
async fn unit_versions_file_refs_chapters_and_trash_isolation() {
    let th = Setup::native().start().await.init_basic().await;
    let (course, chapter, unit) = course_with_unit(&th.client, "private").await;

    // Upload a file, embed it in version 1, overwrite with plain text in version 2,
    // and restore version 1: file remains accessible and restored body references it.
    let file_id = uploaded_id(&upload(&th.client, &course, "diagram.png", b"png-bytes").await);
    let with_image = json!({"type": "doc", "content": [
        {"type": "image", "attrs": {"fileId": file_id}}
    ]});
    write_bodies(
        &th.client,
        &course,
        &unit,
        &[with_image.clone(), paragraphs(&["plain"])],
    )
    .await;
    let versions = versions_path(TEST_WS, &course, &unit);
    let restored = th.client.post_empty(&format!("{versions}/1/restore")).await;
    restored.check_ok();
    assert_eq!(restored.value()["body"], with_image);
    th.client
        .get(&format!("{TEST_WS}/containers/{course}/files/{file_id}"))
        .await
        .check_ok();

    // A version whose body references a missing file is refused with 409 Conflict.
    let missing_file = json!({"type": "doc", "content": [
        {"type": "image", "attrs": {"fileId": "7missing.png"}}
    ]});
    th.app
        .store()
        .execute_unchecked(&format!(
            "UPDATE body_versions SET body = '{missing_file}' WHERE block_id = '{unit}' AND version = 2"
        ))
        .await
        .expect("planted missing file");
    let conflict = th.client.post_empty(&format!("{versions}/2/restore")).await;
    assert_eq!(conflict.status, StatusCode::CONFLICT, "{}", conflict.text());

    // Chapter returns 404 for all 4 body-version operations.
    let ch_versions = versions_path(TEST_WS, &course, &chapter);
    th.client.get(&ch_versions).await.check_not_found();
    th.client.post_empty(&ch_versions).await.check_not_found();
    th.client
        .get(&format!("{ch_versions}/1"))
        .await
        .check_not_found();
    th.client
        .post_empty(&format!("{ch_versions}/1/restore"))
        .await
        .check_not_found();

    // Wrong workspace returns 404.
    let wrong_ws = versions_path(ROOT_WS, &course, &unit);
    th.client.get(&wrong_ws).await.check_not_found();
    th.client
        .post_empty(&format!("{wrong_ws}/1/restore"))
        .await
        .check_not_found();

    // Trashed unit returns 404 across all 4 version endpoints.
    th.client
        .delete(&unit_path(TEST_WS, &course, &unit))
        .await
        .check_no_content();
    for response in [
        th.client.get(&versions).await,
        th.client.post_empty(&versions).await,
        th.client.get(&format!("{versions}/1")).await,
        th.client.post_empty(&format!("{versions}/1/restore")).await,
    ] {
        assert_eq!(
            response.status,
            StatusCode::NOT_FOUND,
            "{}",
            response.text()
        );
    }

    // Missing unit returns 404.
    th.client
        .get(&versions_path(TEST_WS, &course, UNKNOWN_ID))
        .await
        .check_not_found();
    th.teardown().await;
}
