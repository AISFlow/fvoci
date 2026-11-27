//! Notifications and subscriptions for course units: mentions in unit bodies (HTTP and
//! collab rooms), mentions and watcher alerts on unit comments, auto-watching on unit
//! creation and commenting, manual subscriptions (units allowed, chapters refused),
//! losing access hiding subjects, and zero notifications/outbox rows in own spaces.

use std::time::Duration;

use fvoci_model::{
    ContainerKind, NotificationKind, OrgKind, SearchKind, Subscription, UserWorkspace,
};
use serde_json::{Value, json};

use crate::common::{Client, Setup, TEST_WORKSPACE_ID, TEST_WS};
use crate::notifications::{give, kinds, mentioning, plain, told, unread};
use crate::rooms::{PATIENCE, Peer};

fn id(value: &Value) -> String {
    value["id"].as_str().expect("an id").to_owned()
}

async fn make_course(client: &Client, at: &str, course_type: &str) -> (String, String, String) {
    let course = client
        .post(
            &format!("{at}/courses"),
            &json!({"visibility": course_type, "title": "Notified Course"}),
        )
        .await;
    course.check_created();
    let course = id(&course.value());
    let units = format!("{at}/courses/{course}/units");
    let chapter = client
        .post(&units, &json!({"kind": "chapter", "title": "Ch 1"}))
        .await;
    chapter.check_created();
    let chapter = id(&chapter.value());
    let unit = client
        .post(&units, &json!({"parentId": chapter, "title": "Unit 1"}))
        .await;
    unit.check_created();
    (course, chapter, id(&unit.value()))
}

async fn unit_holds(client: &Client, path: &str, expected: &Value) {
    let deadline = tokio::time::Instant::now() + PATIENCE;
    loop {
        let body = client.get(path).await.value()["body"].clone();
        if &body == expected {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "unit body stayed {body}, expected {expected}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Mentions in unit bodies (HTTP + collab), comments alerting watchers without duplicating
/// mentions, manual unit subscriptions (and chapter 404), open-course mention alerts, and
/// hiding subjects when access ends.
#[tokio::test]
async fn workspace_course_unit_mentions_comments_watching_and_subscriptions() {
    let th = Setup::native().start().await.init_basic().await;
    let (a, b) = (th.client.me().await, th.client2.me().await);
    let (course, chapter, unit) = make_course(&th.client, TEST_WS, "private").await;
    give(&th, &course, &b.id, "editor").await;

    // 1) Creating the unit auto-subscribed `a` (creator), and NOT to the chapter.
    let a_subs: Vec<Subscription> = th
        .client
        .get(&format!("{TEST_WS}/subscriptions/{}", a.id))
        .await
        .ok_json();
    assert!(a_subs.iter().any(|s| s.block_id == unit));
    assert!(!a_subs.iter().any(|s| s.block_id == chapter));

    // 2) `b` comments on the unit -> `a` (watcher) gets a `comment` notification.
    let comment = th
        .client2
        .post(
            &format!("{TEST_WS}/courses/{course}/units/{unit}/comments"),
            &json!({"body": plain("question on lesson")}),
        )
        .await;
    comment.check_created();
    let comment_id = id(&comment.value());
    let a_told = told(&th, &th.client).await;
    assert_eq!(a_told.len(), 1, "{a_told:?}");
    let subject = a_told[0].subject.as_ref().expect("visible subject");
    assert_eq!(
        (
            subject.kind,
            subject.container_kind,
            subject.block_kind,
            subject.block_id.as_str(),
            subject.comment_id.as_str()
        ),
        (
            NotificationKind::Comment,
            ContainerKind::Course,
            SearchKind::Unit,
            unit.as_str(),
            comment_id.as_str()
        )
    );

    // 3) `a` types a mention of `b` in the unit's collab room and comments mentioning `b`.
    let mut peer = Peer::connect(&th.client, 501).await;
    peer.joined(TEST_WORKSPACE_ID, &unit).await;
    let update = peer.editor().append_mention(&b.id);
    peer.send_update(&update).await;
    peer.text("docAck").await;
    unit_holds(
        &th.client,
        &format!("{TEST_WS}/courses/{course}/units/{unit}"),
        &mentioning(&[&b.id]),
    )
    .await;
    peer.leave().await;
    th.client
        .post(
            &format!("{TEST_WS}/courses/{course}/units/{unit}/comments"),
            &json!({"body": mentioning(&[&b.id])}),
        )
        .await
        .check_created();
    let b_told = told(&th, &th.client2).await;
    assert_eq!(kinds(&b_told), (vec!["mention"], 0), "{b_told:?}");
    assert_eq!(unread(&th.client2).await, 1);

    verify_manual_unit_subscriptions(&th.client2, &unit, &chapter, &b.id).await;

    // 5) Removing `b` clears the subject; open-course mention notifies an unjoined member.
    th.client
        .delete(&format!("{TEST_WS}/containers/{course}/members/{}", b.id))
        .await
        .check_no_content();
    assert_eq!(kinds(&told(&th, &th.client2).await), (vec![], 1));

    let (open, _, open_unit) = make_course(&th.client, TEST_WS, "open").await;
    let open_path = format!("{TEST_WS}/courses/{open}/units/{open_unit}");
    let read_at = th.client.get(&open_path).await.value()["updatedAt"].clone();
    th.client
        .patch(
            &open_path,
            &json!({"readAt": read_at, "body": mentioning(&[&b.id])}),
        )
        .await
        .check_ok();
    assert_eq!(kinds(&told(&th, &th.client2).await), (vec!["mention"], 1));
    th.teardown().await;
}

async fn verify_manual_unit_subscriptions(
    client: &crate::common::Client,
    unit: &str,
    chapter: &str,
    user_id: &str,
) {
    client
        .delete(&format!("{TEST_WS}/subscriptions/{unit}/{user_id}"))
        .await
        .check_no_content();
    client
        .post(
            &format!("{TEST_WS}/subscriptions"),
            &json!({"blockId": unit, "blockType": "unit", "userId": user_id}),
        )
        .await
        .check_created();
    client
        .post(
            &format!("{TEST_WS}/subscriptions"),
            &json!({"blockId": chapter, "blockType": "unit", "userId": user_id}),
        )
        .await
        .check_not_found();
    client
        .post(
            &format!("{TEST_WS}/subscriptions"),
            &json!({"blockId": chapter, "blockType": "chapter", "userId": user_id}),
        )
        .await
        .check_bad_request();
}

/// Own-space courses never create notifications, outbox entries, or auto-subscriptions,
/// even when a unit or comment mentions a colleague from another organization.
#[tokio::test]
async fn own_space_course_produces_zero_notifications_and_zero_outbox_rows() {
    let th = Setup::native().start().await.init_basic().await;
    let (owner_id, colleague_id) = (th.client.me().await.id, th.client2.me().await.id);
    let workspaces: Vec<UserWorkspace> = th.client.get("/users/me/workspaces").await.ok_json();
    let own = workspaces
        .into_iter()
        .find(|w| w.kind == OrgKind::Personal)
        .expect("own space");
    let own_ws = format!("/w/{}", own.id);

    let (course, _, unit) = make_course(&th.client, &own_ws, "private").await;
    let unit_path = format!("{own_ws}/courses/{course}/units/{unit}");
    let read_at = th.client.get(&unit_path).await.value()["updatedAt"].clone();
    th.client
        .patch(
            &unit_path,
            &json!({"readAt": read_at, "body": mentioning(&[&colleague_id, &owner_id])}),
        )
        .await
        .check_ok();
    th.client
        .post(
            &format!("{unit_path}/comments"),
            &json!({"body": mentioning(&[&colleague_id, &owner_id])}),
        )
        .await
        .check_created();

    // Outbox must hold zero rows for this personal org, and no auto-subscription is stored.
    let pending = th
        .app
        .housekeeping()
        .store()
        .due_notification_events(i64::MAX, 100)
        .await
        .expect("due events");
    assert!(
        pending.iter().all(|e| e.org_id != own.org_id),
        "own-space course must not enqueue notification_outbox rows: {pending:?}"
    );
    let own_subs: Vec<Subscription> = th
        .client
        .get(&format!("{own_ws}/subscriptions/{owner_id}"))
        .await
        .ok_json();
    assert!(
        own_subs.is_empty(),
        "own-space course must not auto-subscribe: {own_subs:?}"
    );

    // Tending notifications produces zero notifications for either user.
    assert_eq!(told(&th, &th.client).await, []);
    assert_eq!(told(&th, &th.client2).await, []);
    assert_eq!(unread(&th.client).await, 0);
    assert_eq!(unread(&th.client2).await, 0);
    th.teardown().await;
}
