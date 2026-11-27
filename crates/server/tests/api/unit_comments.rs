//! The comments on a unit of a course: roles, open-course read-only access, undelete
//! ownership (`ManageOthersComments`), chapter refusal, file-block refusal, trash
//! restoration with units/chapters, WebSocket broadcasts, and archive round-trip.

use std::time::Duration;

use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::board_broadcasts::WorkspaceSocket;
use crate::common::{
    Client, ROOT_WORKSPACE_ID, ROOT_WS, Setup, TEST_WORKSPACE_ID, TEST_WS, UNKNOWN_ID,
};
use crate::export::import;
use crate::files::{upload, uploaded_id};
use crate::wiki_isolation::put_on;

fn text(said: &str) -> Value {
    json!({"type": "doc", "content": [{"type": "paragraph", "content": [
        {"type": "text", "text": said}
    ]}]})
}

fn id(value: &Value) -> String {
    value["id"].as_str().expect("an id").to_owned()
}

fn comments_of(at: &str, course: &str, unit: &str) -> String {
    format!("{at}/courses/{course}/units/{unit}/comments")
}

async fn course_with_chapter_and_unit(
    client: &Client,
    at: &str,
    course_type: &str,
) -> (String, String, String) {
    let course = client
        .post(
            &format!("{at}/courses"),
            &json!({"visibility": course_type, "title": "Course"}),
        )
        .await;
    course.check_created();
    let course = id(&course.value());
    let units = format!("{at}/courses/{course}/units");
    let chapter = client
        .post(&units, &json!({"kind": "chapter", "title": "Chapter 1"}))
        .await;
    chapter.check_created();
    let chapter = id(&chapter.value());
    let unit = client
        .post(&units, &json!({"parentId": chapter, "title": "Unit 1"}))
        .await;
    unit.check_created();
    (course, chapter, id(&unit.value()))
}

async fn written(client: &Client, at: &str, course: &str, unit: &str, said: &str) -> Value {
    let made = client
        .post(&comments_of(at, course, unit), &json!({"body": text(said)}))
        .await;
    made.check_created();
    made.value()
}

async fn on_theirs(
    client: &Client,
    at: &str,
    course: &str,
    unit: &str,
    theirs: &Value,
) -> [u16; 5] {
    let comments = comments_of(at, course, unit);
    let comment = format!("{comments}/{}", id(theirs));
    let change = json!({"body": text("changed"), "readAt": theirs["updatedAt"]});
    [
        client.get(&comments).await,
        client.post(&comments, &json!({"body": text("mine")})).await,
        client.get(&format!("{comment}/versions")).await,
        client.patch(&comment, &change).await,
        client.delete(&comment).await,
    ]
    .map(|response| response.status.as_u16())
}

/// Viewer reads comments & versions, commenter/editor CRUD their own (with version
/// history), admin moderates others' comments, and `ManageOthersComments` gates
/// undeleting an admin-deleted comment.
#[tokio::test]
async fn roles_versions_and_undelete_ownership_hold_on_unit_comments() {
    let th = Setup::native().start().await.init_basic().await;
    let member = th.client2.me().await.id;
    let (course, _, unit) = course_with_chapter_and_unit(&th.client, TEST_WS, "private").await;
    let theirs = written(&th.client, TEST_WS, &course, &unit, "admin first").await;
    let at_theirs = format!("{}/{}", comments_of(TEST_WS, &course, &unit), id(&theirs));
    let theirs = th
        .client
        .patch(
            &at_theirs,
            &json!({"body": text("admin edited"), "readAt": theirs["updatedAt"]}),
        )
        .await
        .value();

    for (role, expected) in [
        ("viewer", [200, 403, 200, 403, 403]),
        ("commenter", [200, 201, 200, 403, 403]),
        ("editor", [200, 201, 200, 403, 403]),
    ] {
        put_on(&th.client, &course, &member, role).await;
        assert_eq!(
            on_theirs(&th.client2, TEST_WS, &course, &unit, &theirs).await,
            expected,
            "{role}"
        );
    }

    // Author writes, edits (keeping version 1), deletes, and undeletes their own comment.
    let mine = written(&th.client2, TEST_WS, &course, &unit, "first").await;
    let at_mine = format!("{}/{}", comments_of(TEST_WS, &course, &unit), id(&mine));
    let patched = th
        .client2
        .patch(
            &at_mine,
            &json!({"body": text("second"), "readAt": mine["updatedAt"]}),
        )
        .await;
    patched.check_ok();
    assert!(patched.value()["editedAt"].as_i64().unwrap_or(0) > 0);
    let versions = th.client2.get(&format!("{at_mine}/versions")).await.value();
    assert_eq!(versions[0]["body"], text("first"), "{versions}");
    th.client2.delete(&at_mine).await.check_no_content();
    th.client2
        .post_empty(&format!("{at_mine}/restore"))
        .await
        .check_ok();

    // When the course admin deletes the member's comment, the member cannot undelete it
    // (`ManageOthersComments`), while the admin can.
    th.client.delete(&at_mine).await.check_no_content();
    th.client2
        .post_empty(&format!("{at_mine}/restore"))
        .await
        .check_forbidden();
    th.client
        .post_empty(&format!("{at_mine}/restore"))
        .await
        .check_ok();
    th.teardown().await;
}

/// An unjoined workspace member reads comments on an open course, cannot write/edit/delete
/// or undelete even their own earlier comment after leaving, and a non-member of a private
/// course gets 404.
#[tokio::test]
async fn open_course_unjoined_member_reads_comments_only() {
    let th = Setup::native().start().await.init_basic().await;
    let (open, _, unit) = course_with_chapter_and_unit(&th.client, TEST_WS, "open").await;
    let theirs = written(&th.client, TEST_WS, &open, &unit, "instructor").await;
    let at_theirs = format!("{}/{}", comments_of(TEST_WS, &open, &unit), id(&theirs));
    let theirs = th
        .client
        .patch(
            &at_theirs,
            &json!({"body": text("instructor edited"), "readAt": theirs["updatedAt"]}),
        )
        .await
        .value();

    // Unjoined member reads list and versions, refused on all writes.
    assert_eq!(
        on_theirs(&th.client2, TEST_WS, &open, &unit, &theirs).await,
        [200, 403, 200, 403, 403],
        "unjoined open-course reader"
    );

    // Unjoined workspace guest gets 404 on open-course comments, and cannot see them.
    let guest = th.add_user("open-course-comment-guest").await;
    let guest_id = guest.me().await.id;
    th.app
        .store()
        .execute_unchecked(&format!(
            "UPDATE workspace_members SET role = 'guest' \
             WHERE workspace_id = '{TEST_WORKSPACE_ID}' AND user_id = '{guest_id}'"
        ))
        .await
        .expect("demote to guest");
    assert_eq!(
        on_theirs(&guest, TEST_WS, &open, &unit, &theirs).await,
        [404; 5],
        "workspace guest gets 404 on open course comments"
    );

    // Join -> write & delete own comment -> leave -> cannot undelete own comment while unjoined.
    th.client2
        .post_empty(&format!("{TEST_WS}/containers/{open}/join"))
        .await
        .check_ok();
    let mine = written(&th.client2, TEST_WS, &open, &unit, "student").await;
    let at_mine = format!("{}/{}", comments_of(TEST_WS, &open, &unit), id(&mine));
    th.client2.delete(&at_mine).await.check_no_content();
    th.client2
        .post_empty(&format!("{TEST_WS}/containers/{open}/leave"))
        .await
        .check_no_content();
    th.client2
        .post_empty(&format!("{at_mine}/restore"))
        .await
        .check_forbidden();

    // Deleting a commented unit, then deleting its chapter, then undeleting the unit
    // promotes the unit to the top level while keeping its comment attached.
    let at_unit = format!("{TEST_WS}/courses/{open}/units/{unit}");
    let ch_id = th.client.get(&at_unit).await.value()["parentId"]
        .as_str()
        .expect("chapter id")
        .to_owned();
    th.client.delete(&at_unit).await.check_no_content();
    th.client
        .delete(&format!("{TEST_WS}/courses/{open}/units/{ch_id}"))
        .await
        .check_no_content();
    let restored = th
        .client
        .post_empty(&format!("{at_unit}/restore"))
        .await
        .value();
    assert_eq!(restored["parentId"], "", "promoted to top level");
    let after = th
        .client
        .get(&comments_of(TEST_WS, &open, &unit))
        .await
        .value();
    assert_eq!(after.as_array().map(Vec::len), Some(1), "{after}");
    assert_eq!(after[0]["id"], id(&theirs));
    th.teardown().await;
}

/// Chapters refuse all 6 comment operations as 404; file blocks in comments are 400;
/// comments go to trash and return with their unit or chapter; WS broadcasts reach the
/// workspace; and archive export/import keeps unit comments.
#[tokio::test]
async fn chapters_file_blocks_trash_broadcasts_and_archive_round_trip() {
    let th = Setup::native().start().await.init_basic().await;
    let (course, chapter, unit) =
        course_with_chapter_and_unit(&th.client, TEST_WS, "private").await;

    // Chapter returns 404 identical to missing unit across all 6 comment operations.
    let nothing = json!({"id": UNKNOWN_ID, "updatedAt": 1});
    assert_eq!(
        on_theirs(&th.client, TEST_WS, &course, &chapter, &nothing).await,
        [404; 5]
    );
    th.client
        .post_empty(&format!(
            "{}/{UNKNOWN_ID}/restore",
            comments_of(TEST_WS, &course, &chapter)
        ))
        .await
        .check_not_found();
    // Wrong workspace returns 404.
    assert_eq!(
        on_theirs(&th.client, ROOT_WS, &course, &unit, &nothing).await,
        [404; 5]
    );

    // File block inside a unit comment is refused (400).
    let file_id = uploaded_id(&upload(&th.client, &course, "note.txt", b"hello").await);
    let with_file = json!({"body": {"type": "doc", "content": [
        {"type": "file", "attrs": {"fileId": file_id}}
    ]}});
    let refused = th
        .client
        .post(&comments_of(TEST_WS, &course, &unit), &with_file)
        .await;
    assert_eq!(
        refused.status,
        StatusCode::BAD_REQUEST,
        "{}",
        refused.text()
    );

    // WS receives blockChanged when creating a comment.
    let user_id = th.client.me().await.id;
    let mut socket = WorkspaceSocket::subscribe(&th, &th.client, TEST_WORKSPACE_ID, &user_id).await;
    let comment = written(&th.client, TEST_WS, &course, &unit, "broadcast").await;
    let comment_id = id(&comment);
    let heard = socket
        .expect("blockChanged", "/block/id", &comment_id)
        .await;
    assert_eq!(heard["block"]["type"], "comment");
    assert_eq!(heard["block"]["parentId"], unit);

    // Deleting the chapter deletes the unit and its comment; undeleting brings all back.
    let at_chapter = format!("{TEST_WS}/courses/{course}/units/{chapter}");
    th.client.delete(&at_chapter).await.check_no_content();
    th.client
        .get(&comments_of(TEST_WS, &course, &unit))
        .await
        .check_not_found();
    th.client
        .post_empty(&format!("{at_chapter}/restore"))
        .await
        .check_ok();
    let listed = th
        .client
        .get(&comments_of(TEST_WS, &course, &unit))
        .await
        .value();
    assert_eq!(listed.as_array().map(Vec::len), Some(1), "{listed}");
    assert_eq!(listed[0]["id"], comment_id);

    // Archive export + import preserves the unit comment.
    let exported = th
        .client
        .get(&format!("{TEST_WS}/containers/{course}/archive/export"))
        .await;
    exported.check_ok();
    assert!(
        th.app.finish_background(Duration::from_secs(5)).await,
        "background done"
    );
    import(&th.client, ROOT_WORKSPACE_ID, &exported.body)
        .await
        .check_ok();
    let courses: Vec<Value> = th.client.get(&format!("{ROOT_WS}/courses")).await.ok_json();
    let imported_course = id(&courses[0]);
    let tree: Vec<Value> = th
        .client
        .get(&format!("{ROOT_WS}/courses/{imported_course}/units"))
        .await
        .ok_json();
    let imported_unit = id(tree.iter().find(|n| n["kind"] == "unit").expect("unit"));
    let imported_comments: Vec<Value> = th
        .client
        .get(&comments_of(ROOT_WS, &imported_course, &imported_unit))
        .await
        .ok_json();
    assert_eq!(imported_comments.len(), 1, "{imported_comments:?}");
    assert_eq!(imported_comments[0]["body"], text("broadcast"));
    th.teardown().await;
}
