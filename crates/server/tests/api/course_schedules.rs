//! HTTP API tests for course schedules, unit amounts, time-zone "today" calculation,
//! no-auto-shift overdue semantics, manual shared/personal redistribution, workspace
//! learning today feeds, and full authorization / personal-date isolation (including
//! workspace/org admins, `joinOrgWorkspace`, guests, public links, websockets, webhooks,
//! SSO/password-confirmation walls, and archive round-trip).

use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use axum::Router;
use axum::routing::post;
use fvoci_model::{
    AmountUnit, CourseScheduleView, STUDY_DAYS_ALL, STUDY_DAYS_WEEKDAYS, Unit, UnitNode,
    UnitProgress, WorkspaceLearningToday, WorkspaceRole,
};
use reqwest::StatusCode;
use serde_json::{Value, json};
use tokio::net::TcpListener;

use crate::board_broadcasts::WorkspaceSocket;
use crate::common::{
    ApiResponse, Client, PASSWORD, ROOT_WS, Setup, TEST_WORKSPACE_ID, TEST_WS, TestServer,
};
use crate::export::import;
use crate::fake_oidc::FakeProvider;
use crate::oidc::{org_of, set_provider, with_key_and_private_addresses};
use crate::own_space::own_workspace;
use crate::sso::{Passwords, admin_through_provider, require_sso};

static WEBHOOK_URL: OnceLock<String> = OnceLock::new();

async fn add_guest(ts: &TestServer, username: &str) -> Client {
    let guest = ts.add_user(username).await;
    let guest_id = guest.me().await.id;
    ts.app
        .store()
        .execute_unchecked(&format!(
            "UPDATE workspace_members SET role = '{}' WHERE user_id = '{guest_id}' \
             AND workspace_id = '{TEST_WORKSPACE_ID}'",
            WorkspaceRole::Guest.as_str()
        ))
        .await
        .expect("guest role set");
    guest
}

fn utc_millis(iso: &str) -> i64 {
    iso.parse::<jiff::Timestamp>()
        .expect("valid timestamp")
        .as_millisecond()
}

async fn create_course(client: &Client, ws: &str, course_type: &str, title: &str) -> String {
    let made = client
        .post(
            &format!("{ws}/courses"),
            &json!({"visibility": course_type, "title": title}),
        )
        .await;
    made.check_created();
    made.value()["id"].as_str().expect("course id").to_owned()
}

async fn create_unit(client: &Client, ws: &str, course_id: &str, body: &Value) -> Unit {
    let made = client
        .post(&format!("{ws}/courses/{course_id}/units"), body)
        .await;
    made.check_created();
    made.created_json()
}

async fn start_webhook_sink() -> (String, Arc<Mutex<Vec<String>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&seen);
    let app = Router::new().route(
        "/hook",
        post(move |body: String| {
            let captured = Arc::clone(&captured);
            async move {
                captured.lock().expect("hook lock").push(body);
                StatusCode::OK
            }
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind webhook sink");
    let addr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}/hook"), seen)
}

fn check_unauthorized_reason(response: &ApiResponse, expected: &str) {
    response.check_status(StatusCode::UNAUTHORIZED);
    assert_eq!(response.error_reason(), expected, "{}", response.text());
}

async fn add_course_viewer(owner: &Client, ws: &str, course_id: &str, user_id: &str) {
    owner
        .post(
            &format!("{ws}/containers/{course_id}/members"),
            &json!({"containerId": course_id, "userId": user_id, "role": "viewer"}),
        )
        .await
        .check_ok();
}

/// Invalid unit amounts/dates and schedule fields on chapters are rejected with 400.
#[tokio::test]
async fn unit_amount_and_scheduled_date_validation_and_chapter_refusal() {
    let ts = Setup::local_mode().start().await.init_basic().await;
    let client = &ts.client;
    let course = create_course(client, TEST_WS, "private", "Algorithms").await;
    let chapter = create_unit(
        client,
        TEST_WS,
        &course,
        &json!({"kind": "chapter", "title": "Ch 1"}),
    )
    .await;

    for bad in [
        json!({"kind": "chapter", "title": "Bad", "amount": 10}),
        json!({"kind": "chapter", "title": "Bad", "amountUnit": "minutes"}),
        json!({"kind": "chapter", "title": "Bad", "scheduledDate": "2026-11-24"}),
        json!({"title": "U", "amount": 0}),
        json!({"title": "U", "amount": -5}),
        json!({"title": "U", "amount": 100_001}),
        json!({"title": "U", "amountUnit": "hours"}),
        json!({"title": "U", "scheduledDate": "2026-02-30"}),
        json!({"title": "U", "scheduledDate": "not-a-date"}),
    ] {
        client
            .post(&format!("{TEST_WS}/courses/{course}/units"), &bad)
            .await
            .check_bad_request();
    }
    for bad in [
        json!({"amount": 15}),
        json!({"amount": null}),
        json!({"amountUnit": "pages"}),
        json!({"amountUnit": null}),
        json!({"scheduledDate": "2026-11-24"}),
        json!({"scheduledDate": null}),
    ] {
        client
            .patch(
                &format!("{TEST_WS}/courses/{course}/units/{}", chapter.id),
                &bad,
            )
            .await
            .check_bad_request();
    }
    ts.teardown().await;
}

/// Unit amount, amountUnit, and scheduledDate patch/clear cleanly and round-trip through
/// archive export/import.
#[tokio::test]
async fn unit_amount_and_scheduled_date_patch_clear_and_archive_roundtrip() {
    let ts = Setup::local_mode().start().await.init_basic().await;
    let client = &ts.client;
    let course = create_course(client, TEST_WS, "private", "Algorithms").await;
    let chapter = create_unit(
        client,
        TEST_WS,
        &course,
        &json!({"kind": "chapter", "title": "Ch 1"}),
    )
    .await;

    let unit = create_unit(
        client,
        TEST_WS,
        &course,
        &json!({
            "parentId": chapter.id,
            "title": "Quicksort",
            "amount": 45,
            "amountUnit": "minutes",
            "scheduledDate": "2026-11-25"
        }),
    )
    .await;
    assert_eq!(unit.amount, Some(45));
    assert_eq!(unit.amount_unit, Some(AmountUnit::Minutes));
    assert_eq!(unit.scheduled_date.as_deref(), Some("2026-11-25"));

    let patched: Unit = client
        .patch(
            &format!("{TEST_WS}/courses/{course}/units/{}", unit.id),
            &json!({"amount": 12, "amountUnit": "problems", "scheduledDate": null}),
        )
        .await
        .ok_json();
    assert_eq!(patched.amount, Some(12));
    assert_eq!(patched.amount_unit, Some(AmountUnit::Problems));
    assert_eq!(patched.scheduled_date, None);

    let patched_again: Unit = client
        .patch(
            &format!("{TEST_WS}/courses/{course}/units/{}", unit.id),
            &json!({"scheduledDate": "2026-12-01"}),
        )
        .await
        .ok_json();
    assert_eq!(patched_again.amount, Some(12));
    assert_eq!(patched_again.scheduled_date.as_deref(), Some("2026-12-01"));

    let exported = client
        .get(&format!("{TEST_WS}/containers/{course}/archive/export"))
        .await;
    exported.check_ok();
    let other_ws = crate::boards::another_workspace(&ts, &client.me().await.id).await;
    import(client, &other_ws, &exported.body).await.check_ok();
    let courses: Vec<Value> = client
        .get(&format!("/w/{other_ws}/courses"))
        .await
        .ok_json();
    let imported_id = courses[0]["id"].as_str().expect("imported course id");
    let tree: Vec<UnitNode> = client
        .get(&format!("/w/{other_ws}/courses/{imported_id}/units"))
        .await
        .ok_json();
    let imported_unit = tree
        .iter()
        .find(|n| n.title == "Quicksort")
        .expect("imported unit");
    assert_eq!(imported_unit.amount, Some(12));
    assert_eq!(imported_unit.amount_unit, Some(AmountUnit::Problems));
    assert_eq!(imported_unit.scheduled_date.as_deref(), Some("2026-12-01"));

    ts.teardown().await;
}

/// At UTC 16:00, `Asia/Seoul` sees the next calendar day while `America/Los_Angeles` and UTC
/// ("auto") see the same day, changing `today`, `todayCount`, `overdueCount`, and
/// `getWorkspaceLearningToday` accordingly.
#[tokio::test]
async fn timezone_controls_today_and_overdue_for_seoul_vs_los_angeles_at_utc_16_00() {
    let ts = Setup::local_mode().start().await.init_basic().await;
    ts.app
        .set_schedule_now_millis(Some(utc_millis("2026-11-24T16:00:00Z")));

    let seoul = &ts.client;
    let la = &ts.client2;
    seoul
        .patch("/users/me/settings", &json!({"timeZone": "Asia/Seoul"}))
        .await
        .check_ok();
    la.patch(
        "/users/me/settings",
        &json!({"timeZone": "America/Los_Angeles"}),
    )
    .await
    .check_ok();

    let course = create_course(seoul, TEST_WS, "open", "Global Physics").await;
    la.post(&format!("{TEST_WS}/containers/{course}/join"), &json!({}))
        .await
        .check_ok();
    let u_nov24 = create_unit(
        seoul,
        TEST_WS,
        &course,
        &json!({"title": "Nov 24 unit", "scheduledDate": "2026-11-24"}),
    )
    .await;
    let u_nov25 = create_unit(
        seoul,
        TEST_WS,
        &course,
        &json!({"title": "Nov 25 unit", "scheduledDate": "2026-11-25"}),
    )
    .await;

    let seoul_view: CourseScheduleView = seoul
        .get(&format!("{TEST_WS}/courses/{course}/schedule"))
        .await
        .ok_json();
    assert_eq!(seoul_view.today, "2026-11-25");
    assert_eq!((seoul_view.overdue_count, seoul_view.today_count), (1, 1));
    assert!(
        seoul_view.units[0].overdue,
        "Nov 24 unit is overdue in Asia/Seoul"
    );
    assert!(
        !seoul_view.units[1].overdue,
        "Nov 25 unit is due today in Asia/Seoul"
    );

    let la_view: CourseScheduleView = la
        .get(&format!("{TEST_WS}/courses/{course}/schedule"))
        .await
        .ok_json();
    assert_eq!(la_view.today, "2026-11-24");
    assert_eq!((la_view.overdue_count, la_view.today_count), (0, 1));
    assert!(
        !la_view.units[0].overdue,
        "Nov 24 unit is due today in America/Los_Angeles"
    );

    let seoul_feed: WorkspaceLearningToday = seoul
        .get(&format!("{TEST_WS}/learning/today"))
        .await
        .ok_json();
    assert_eq!(seoul_feed.today, "2026-11-25");
    assert_eq!(seoul_feed.overdue_units.len(), 1);
    assert_eq!(seoul_feed.overdue_units[0].unit_id, u_nov24.id);
    assert_eq!(seoul_feed.today_units.len(), 1);
    assert_eq!(seoul_feed.today_units[0].unit_id, u_nov25.id);

    let la_feed: WorkspaceLearningToday =
        la.get(&format!("{TEST_WS}/learning/today")).await.ok_json();
    assert_eq!(la_feed.today, "2026-11-24");
    assert!(
        la_feed.overdue_units.is_empty(),
        "America/Los_Angeles has no overdue units at UTC 16:00"
    );
    assert_eq!(la_feed.today_units.len(), 1);
    assert_eq!(la_feed.today_units[0].unit_id, u_nov24.id);

    ts.teardown().await;
}

async fn create_numbered_units(
    client: &Client,
    ws: &str,
    course: &str,
    count: usize,
) -> Vec<String> {
    let mut ids = Vec::with_capacity(count);
    for i in 1..=count {
        let u = create_unit(
            client,
            ws,
            course,
            &json!({"title": format!("Unit {i}"), "amount": 20, "amountUnit": "minutes"}),
        )
        .await;
        ids.push(u.id);
    }
    ids
}

/// Overdue incomplete units never auto-shift as days pass; only an explicit redistribute
/// updates them (personal redistribution updates only incomplete units in caller's TZ,
/// while shared redistribution updates shared scheduledDate across all live units).
#[tokio::test]
async fn overdue_never_auto_shifts_until_manual_redistribute() {
    let ts = Setup::local_mode().start().await.init_basic().await;
    let client = &ts.client;
    client
        .patch("/users/me/settings", &json!({"timeZone": "UTC"}))
        .await
        .check_ok();
    ts.app
        .set_schedule_now_millis(Some(utc_millis("2026-11-02T09:00:00Z")));

    let course = create_course(client, TEST_WS, "private", "Rust Track").await;
    let unit_ids = create_numbered_units(client, TEST_WS, &course, 6).await;

    let view: CourseScheduleView = client
        .put(
            &format!("{TEST_WS}/courses/{course}/schedule"),
            &json!({
                "startDate": "2026-11-02",
                "targetDate": "2026-11-09",
                "studyDays": STUDY_DAYS_WEEKDAYS,
                "autoAssign": true
            }),
        )
        .await
        .ok_json();
    let initial_dates: Vec<Option<String>> = view
        .units
        .iter()
        .map(|u| u.scheduled_date.clone())
        .collect();
    assert_eq!(
        initial_dates,
        [
            Some("2026-11-02".to_owned()),
            Some("2026-11-03".to_owned()),
            Some("2026-11-04".to_owned()),
            Some("2026-11-05".to_owned()),
            Some("2026-11-06".to_owned()),
            Some("2026-11-09".to_owned()),
        ]
    );

    client
        .put(
            &format!("{TEST_WS}/courses/{course}/units/{}/progress", unit_ids[0]),
            &json!({"completed": true}),
        )
        .await
        .check_ok();
    ts.app
        .set_schedule_now_millis(Some(utc_millis("2026-11-05T09:00:00Z")));

    let aged: CourseScheduleView = client
        .get(&format!("{TEST_WS}/courses/{course}/schedule"))
        .await
        .ok_json();
    assert_eq!(aged.today, "2026-11-05");
    assert_eq!((aged.overdue_count, aged.today_count), (2, 1));
    assert_eq!(
        aged.units
            .iter()
            .map(|u| u.effective_date.clone())
            .collect::<Vec<_>>(),
        initial_dates,
        "overdue dates never auto-shift"
    );

    let personal: CourseScheduleView = client
        .post(
            &format!("{TEST_WS}/courses/{course}/my-schedule/redistribute"),
            &json!({}),
        )
        .await
        .ok_json();
    assert_eq!(personal.overdue_count, 0);
    assert_eq!(personal.units[0].my_planned_date, None);
    for unit in &personal.units[1..] {
        let planned = unit.my_planned_date.as_deref().expect("personal date");
        assert!(
            ("2026-11-05"..="2026-11-09").contains(&planned),
            "unexpected personal date: {planned}"
        );
    }
    assert_eq!(
        personal
            .units
            .iter()
            .map(|u| u.scheduled_date.clone())
            .collect::<Vec<_>>(),
        initial_dates
    );

    let shared: CourseScheduleView = client
        .post(
            &format!("{TEST_WS}/courses/{course}/schedule/redistribute"),
            &json!({"targetDate": "2026-11-13"}),
        )
        .await
        .ok_json();
    for unit in &shared.units {
        let date = unit.scheduled_date.as_deref().expect("shared date updated");
        assert!(
            ("2026-11-05"..="2026-11-13").contains(&date),
            "unexpected shared date: {date}"
        );
    }

    ts.teardown().await;
}

async fn assert_non_editor_cannot_edit_shared(non_editor: &Client, course: &str, unit_id: &str) {
    non_editor
        .put(
            &format!("{TEST_WS}/courses/{course}/schedule"),
            &json!({"startDate": "2026-11-20", "targetDate": "2026-11-30", "studyDays": STUDY_DAYS_ALL}),
        )
        .await
        .check_forbidden();
    non_editor
        .post(
            &format!("{TEST_WS}/courses/{course}/schedule/redistribute"),
            &json!({}),
        )
        .await
        .check_forbidden();
    non_editor
        .delete(&format!("{TEST_WS}/courses/{course}/schedule"))
        .await
        .check_forbidden();
    non_editor
        .patch(
            &format!("{TEST_WS}/courses/{course}/units/{unit_id}"),
            &json!({"amount": 99}),
        )
        .await
        .check_forbidden();
}

/// Viewers, invited guests, and unjoined readers cannot edit shared schedules or unit
/// amounts; unjoined open readers can view shared schedules but cannot set personal
/// planned dates and do not see unjoined courses in `learning/today`.
#[tokio::test]
async fn non_editors_and_unjoined_readers_permission_matrix() {
    let ts = Setup::local_mode().start().await.init_basic().await;
    ts.app
        .set_schedule_now_millis(Some(utc_millis("2026-11-24T10:00:00Z")));

    let alice = &ts.client;
    let bob = &ts.client2;
    let carol = ts.add_user("carol").await;
    let guest = add_guest(&ts, "guest-learner").await;

    let open_course = create_course(alice, TEST_WS, "open", "Open Biology").await;
    add_course_viewer(alice, TEST_WS, &open_course, &bob.me().await.id).await;
    add_course_viewer(alice, TEST_WS, &open_course, &guest.me().await.id).await;
    let u1 = create_unit(
        alice,
        TEST_WS,
        &open_course,
        &json!({"title": "Cells", "amount": 30, "amountUnit": "minutes", "scheduledDate": "2026-11-24"}),
    )
    .await;
    alice
        .put(
            &format!("{TEST_WS}/courses/{open_course}/schedule"),
            &json!({"startDate": "2026-11-20", "targetDate": "2026-11-30", "studyDays": STUDY_DAYS_ALL}),
        )
        .await
        .check_ok();

    for non_editor in [bob, &carol, &guest] {
        assert_non_editor_cannot_edit_shared(non_editor, &open_course, &u1.id).await;
    }

    let carol_view: CourseScheduleView = carol
        .get(&format!("{TEST_WS}/courses/{open_course}/schedule"))
        .await
        .ok_json();
    assert_eq!(carol_view.units[0].my_planned_date, None);
    carol
        .put(
            &format!("{TEST_WS}/courses/{open_course}/units/{}/schedule", u1.id),
            &json!({"plannedDate": "2026-11-26"}),
        )
        .await
        .check_forbidden();
    carol
        .post(
            &format!("{TEST_WS}/courses/{open_course}/my-schedule/redistribute"),
            &json!({}),
        )
        .await
        .check_forbidden();
    let carol_feed: WorkspaceLearningToday = carol
        .get(&format!("{TEST_WS}/learning/today"))
        .await
        .ok_json();
    assert!(
        carol_feed.today_units.is_empty(),
        "unjoined open course must not appear in learning/today"
    );

    ts.teardown().await;
}

async fn set_personal_date(
    client: &Client,
    course: &str,
    unit_id: &str,
    date: Option<&str>,
) -> UnitProgress {
    client
        .put(
            &format!("{TEST_WS}/courses/{course}/units/{unit_id}/schedule"),
            &json!({"plannedDate": date}),
        )
        .await
        .ok_json()
}

async fn assert_no_side_channels(
    ts: &TestServer,
    socket: &mut WorkspaceSocket,
    hooks: &Arc<Mutex<Vec<String>>>,
) {
    assert!(
        ts.app.finish_background(Duration::from_secs(5)).await,
        "background work finished"
    );
    assert!(
        socket.drain().await.is_empty(),
        "personal schedule writes must not broadcast over websockets"
    );
    assert!(
        hooks.lock().expect("hook lock").is_empty(),
        "personal schedule writes must not trigger webhooks"
    );
}

/// Personal `plannedDate` writes stay private across peers and invited guests, emit no
/// websocket broadcasts or webhooks, require the `plannedDate` key (`{}` -> 400), support
/// viewer personal redistribution, and revert to shared `scheduledDate` on `null`.
#[tokio::test]
async fn personal_planned_dates_stay_private_across_peers_guests_ws_and_webhooks() {
    let (hook_url, hooks) = start_webhook_sink().await;
    let _ = WEBHOOK_URL.set(hook_url);
    let ts = Setup::local_mode()
        .configure(|cfg| {
            cfg.block_change_webhooks = WEBHOOK_URL.get().into_iter().cloned().collect();
        })
        .start()
        .await
        .init_basic()
        .await;
    ts.app
        .set_schedule_now_millis(Some(utc_millis("2026-11-24T10:00:00Z")));

    let alice = &ts.client;
    let bob = &ts.client2;
    let bob_id = bob.me().await.id;
    let guest = add_guest(&ts, "guest-learner").await;

    let open_course = create_course(alice, TEST_WS, "open", "Open Biology").await;
    add_course_viewer(alice, TEST_WS, &open_course, &bob_id).await;
    add_course_viewer(alice, TEST_WS, &open_course, &guest.me().await.id).await;
    let u1 = create_unit(
        alice,
        TEST_WS,
        &open_course,
        &json!({"title": "Cells", "scheduledDate": "2026-11-24"}),
    )
    .await;

    let mut socket = WorkspaceSocket::subscribe(&ts, bob, TEST_WORKSPACE_ID, &bob_id).await;
    assert!(
        ts.app.finish_background(Duration::from_secs(5)).await,
        "setup webhooks finished"
    );
    hooks.lock().expect("hook lock").clear();

    assert_eq!(
        set_personal_date(alice, &open_course, &u1.id, Some("2026-11-18"))
            .await
            .planned_date
            .as_deref(),
        Some("2026-11-18")
    );
    // Omitting `plannedDate` (`{}`) must return 400 and NOT wipe Alice's stored date.
    let unit_path = format!("{TEST_WS}/courses/{open_course}/units/{}/schedule", u1.id);
    alice.put(&unit_path, &json!({})).await.check_bad_request();
    assert_eq!(
        set_personal_date(&guest, &open_course, &u1.id, Some("2026-11-28"))
            .await
            .planned_date
            .as_deref(),
        Some("2026-11-28")
    );
    assert_no_side_channels(&ts, &mut socket, &hooks).await;

    let sched_path = format!("{TEST_WS}/courses/{open_course}/schedule");
    let bob_view = bob.get(&sched_path).await;
    bob_view.check_ok();
    assert!(
        !bob_view.text().contains("2026-11-18") && !bob_view.text().contains("2026-11-28"),
        "other learners' personal plannedDate leaked to bob"
    );
    let bob_parsed: CourseScheduleView = bob_view.ok_json();
    assert_eq!(
        bob_parsed.units[0].effective_date.as_deref(),
        Some("2026-11-24")
    );
    assert!(
        !bob_parsed.units[0].overdue,
        "bob uses shared date 2026-11-24, not alice's overdue date"
    );

    let bob_red: CourseScheduleView = bob
        .post(
            &format!("{TEST_WS}/courses/{open_course}/my-schedule/redistribute"),
            &json!({"fromDate": "2026-11-29", "targetDate": "2026-11-30", "studyDays": STUDY_DAYS_ALL}),
        )
        .await
        .ok_json();
    assert_eq!(
        bob_red.units[0].my_planned_date.as_deref(),
        Some("2026-11-29")
    );
    assert_eq!(
        bob_red.units[0].scheduled_date.as_deref(),
        Some("2026-11-24")
    );
    assert_no_side_channels(&ts, &mut socket, &hooks).await;

    assert_eq!(
        set_personal_date(alice, &open_course, &u1.id, None)
            .await
            .planned_date,
        None
    );
    let alice_after: CourseScheduleView = alice.get(&sched_path).await.ok_json();
    assert_eq!(
        alice_after.units[0].effective_date.as_deref(),
        Some("2026-11-24")
    );
    assert_eq!(alice_after.overdue_count, 0);

    ts.teardown().await;
}

/// Deleting a shared schedule is idempotent, clears `schedule` to `null`, preserves unit
/// `scheduledDate` and learner `plannedDate`, and shared `autoAssign` broadcasts
/// `blockChanged` to subscribers.
#[tokio::test]
async fn delete_schedule_is_idempotent_and_shared_auto_assign_broadcasts_updates() {
    let ts = Setup::local_mode().start().await.init_basic().await;
    let alice = &ts.client;
    let bob = &ts.client2;
    let bob_id = bob.me().await.id;
    let course = create_course(alice, TEST_WS, "open", "Broadcast Course").await;
    add_course_viewer(alice, TEST_WS, &course, &bob_id).await;
    let u1 = create_unit(alice, TEST_WS, &course, &json!({"title": "Lesson 1"})).await;

    let mut socket = WorkspaceSocket::subscribe(&ts, bob, TEST_WORKSPACE_ID, &bob_id).await;
    let saved: CourseScheduleView = alice
        .put(
            &format!("{TEST_WS}/courses/{course}/schedule"),
            &json!({
                "startDate": "2026-11-24",
                "targetDate": "2026-11-26",
                "studyDays": STUDY_DAYS_ALL,
                "autoAssign": true
            }),
        )
        .await
        .ok_json();
    assert!(saved.schedule.is_some(), "schedule saved");
    assert_eq!(saved.units[0].scheduled_date.as_deref(), Some("2026-11-24"));
    assert!(
        ts.app.finish_background(Duration::from_secs(5)).await,
        "shared autoAssign background broadcast finished"
    );
    let messages = socket.drain().await;
    assert!(
        messages
            .iter()
            .any(|msg| msg["type"] == "blockChanged" && msg["block"]["id"] == u1.id),
        "shared autoAssign must broadcast blockChanged: {messages:?}"
    );

    set_personal_date(alice, &course, &u1.id, Some("2026-11-28")).await;
    alice
        .delete(&format!("{TEST_WS}/courses/{course}/schedule"))
        .await
        .check_no_content();
    alice
        .delete(&format!("{TEST_WS}/courses/{course}/schedule"))
        .await
        .check_no_content();

    let after: CourseScheduleView = alice
        .get(&format!("{TEST_WS}/courses/{course}/schedule"))
        .await
        .ok_json();
    assert_eq!(after.schedule, None);
    assert_eq!(after.units[0].scheduled_date.as_deref(), Some("2026-11-24"));
    assert_eq!(
        after.units[0].my_planned_date.as_deref(),
        Some("2026-11-28")
    );

    ts.teardown().await;
}

/// `getWorkspaceLearningToday` orders `todayUnits` by course creation then outline order,
/// orders `overdueUnits` by `effectiveDate` ascending then course/outline order, and
/// excludes trashed courses and trashed units.
#[tokio::test]
async fn workspace_learning_today_orders_multi_course_and_excludes_trash() {
    let ts = Setup::local_mode().start().await.init_basic().await;
    ts.app
        .set_schedule_now_millis(Some(utc_millis("2026-11-24T10:00:00Z")));
    let alice = &ts.client;

    let c1 = create_course(alice, TEST_WS, "private", "Course A").await;
    let c2 = create_course(alice, TEST_WS, "private", "Course B").await;
    let trashed_course = create_course(alice, TEST_WS, "private", "Trashed Course").await;

    let c1_today = create_unit(
        alice,
        TEST_WS,
        &c1,
        &json!({"title": "A Today", "scheduledDate": "2026-11-24"}),
    )
    .await;
    let c1_over_late = create_unit(
        alice,
        TEST_WS,
        &c1,
        &json!({"title": "A Overdue Nov 22", "scheduledDate": "2026-11-22"}),
    )
    .await;
    let trashed_unit = create_unit(
        alice,
        TEST_WS,
        &c1,
        &json!({"title": "Deleted Unit", "scheduledDate": "2026-11-24"}),
    )
    .await;
    let c2_today = create_unit(
        alice,
        TEST_WS,
        &c2,
        &json!({"title": "B Today", "scheduledDate": "2026-11-24"}),
    )
    .await;
    let c2_over_early = create_unit(
        alice,
        TEST_WS,
        &c2,
        &json!({"title": "B Overdue Nov 20", "scheduledDate": "2026-11-20"}),
    )
    .await;
    create_unit(
        alice,
        TEST_WS,
        &trashed_course,
        &json!({"title": "Ghost", "scheduledDate": "2026-11-24"}),
    )
    .await;

    alice
        .delete(&format!("{TEST_WS}/courses/{c1}/units/{}", trashed_unit.id))
        .await
        .check_no_content();
    alice
        .delete(&format!("{TEST_WS}/containers/{trashed_course}"))
        .await
        .check_no_content();

    let feed: WorkspaceLearningToday = alice
        .get(&format!("{TEST_WS}/learning/today"))
        .await
        .ok_json();
    assert_eq!(feed.today, "2026-11-24");
    assert_eq!(
        feed.today_units
            .iter()
            .map(|i| i.unit_id.as_str())
            .collect::<Vec<_>>(),
        [c1_today.id.as_str(), c2_today.id.as_str()]
    );
    assert_eq!(
        feed.overdue_units
            .iter()
            .map(|i| i.unit_id.as_str())
            .collect::<Vec<_>>(),
        [c2_over_early.id.as_str(), c1_over_late.id.as_str()]
    );

    ts.teardown().await;
}

/// Public read links expose shared `scheduledDate` and never leak personal `plannedDate`.
#[tokio::test]
async fn public_link_exposes_shared_date_never_personal_planned_date() {
    let ts = Setup::local_mode().start().await.init_basic().await;
    let alice = &ts.client;
    let bob = &ts.client2;

    let open_course = create_course(alice, TEST_WS, "open", "Public Course").await;
    bob.post(
        &format!("{TEST_WS}/containers/{open_course}/join"),
        &json!({}),
    )
    .await
    .check_ok();
    let u1 = create_unit(
        alice,
        TEST_WS,
        &open_course,
        &json!({"title": "Lesson", "scheduledDate": "2026-11-24"}),
    )
    .await;
    bob.put(
        &format!("{TEST_WS}/courses/{open_course}/units/{}/schedule", u1.id),
        &json!({"plannedDate": "2026-11-29"}),
    )
    .await
    .check_ok();
    alice
        .post(
            &format!("{TEST_WS}/containers/{open_course}/public-link"),
            &json!({"enabled": true}),
        )
        .await
        .check_ok();
    let token = alice
        .get(&format!("{TEST_WS}/containers/{open_course}/public-link"))
        .await
        .value()["token"]
        .as_str()
        .expect("token")
        .to_owned();
    let shared_tree = ts
        .anonymous()
        .get(&format!("/shared/{open_course}/units?readToken={token}"))
        .await;
    shared_tree.check_ok();
    assert!(
        shared_tree.text().contains("2026-11-24"),
        "shared date is included in public unit tree"
    );
    assert!(
        !shared_tree.text().contains("2026-11-29"),
        "personal plannedDate must not leak on public link"
    );

    ts.teardown().await;
}

/// An organization admin joining a workspace (`joinOrgWorkspace`) still cannot see a
/// private course's schedule, its units in `learning/today`, or its owner's personal
/// `plannedDate`.
#[tokio::test]
async fn org_admin_joining_workspace_cannot_read_private_course_schedules() {
    let ts = Setup::local_mode().start().await.init_basic().await;
    ts.app
        .set_schedule_now_millis(Some(utc_millis("2026-11-24T10:00:00Z")));

    let alice = &ts.client;
    let bob = &ts.client2;
    let org_id = org_of(&ts).await;
    let other_ws_resp = alice
        .post(
            &format!("/org/{org_id}/workspaces"),
            &json!({"name": "Private Lab"}),
        )
        .await;
    other_ws_resp.check_created();
    let other_ws_id = other_ws_resp.value()["id"]
        .as_str()
        .expect("ws id")
        .to_owned();
    let other_ws = format!("/w/{other_ws_id}");
    let bob_id = bob.me().await.id;
    ts.app
        .store()
        .for_org(&org_id)
        .expect("org")
        .add_workspace_member(&other_ws_id, &bob_id, WorkspaceRole::Admin)
        .await
        .expect("bob in lab");
    let bob_course = create_course(bob, &other_ws, "private", "Bob Secret Study").await;
    let bob_unit = create_unit(
        bob,
        &other_ws,
        &bob_course,
        &json!({"title": "Secret Unit", "scheduledDate": "2026-11-24"}),
    )
    .await;
    bob.put(
        &format!(
            "{other_ws}/courses/{bob_course}/units/{}/schedule",
            bob_unit.id
        ),
        &json!({"plannedDate": "2026-11-17"}),
    )
    .await
    .check_ok();
    alice
        .post(
            &format!("/org/{org_id}/workspaces/{other_ws_id}/join"),
            &json!({}),
        )
        .await
        .check_ok();
    alice
        .get(&format!("{other_ws}/courses/{bob_course}/schedule"))
        .await
        .check_not_found();
    let admin_feed = alice.get(&format!("{other_ws}/learning/today")).await;
    admin_feed.check_ok();
    assert!(
        !admin_feed.text().contains(&bob_course),
        "private course leaked to org admin learning/today"
    );
    assert!(
        !admin_feed.text().contains("2026-11-17"),
        "personal plannedDate leaked to org admin learning/today"
    );

    ts.teardown().await;
}

/// Personal-space course schedules require password confirmation on OIDC sessions, and
/// organization schedules sit behind the SSO wall.
#[tokio::test]
async fn own_space_and_sso_wall_protect_course_schedule_routes() {
    let fake = FakeProvider::start().await;
    let ts = Setup::local_mode()
        .trusting_oidc_provider(&fake.certificate_der)
        .configure(|cfg| {
            with_key_and_private_addresses(cfg);
            if fvoci_store::testing::db_type_of_the_tests() == fvoci_store::DbType::Postgres {
                "open".clone_into(&mut cfg.org_creation);
            }
        })
        .start()
        .await
        .init_basic()
        .await;
    let org_id = org_of(&ts).await;
    set_provider(&ts, &org_id, &fake).await;

    let own = own_workspace(&ts.client).await;
    let own_ws = format!("/w/{}", own.id);
    let own_course = create_course(&ts.client, &own_ws, "private", "Solo Schedule").await;
    let own_unit = create_unit(
        &ts.client,
        &own_ws,
        &own_course,
        &json!({"title": "Solo 1", "scheduledDate": "2026-11-24"}),
    )
    .await;

    let own_paths = [
        format!("{own_ws}/courses/{own_course}/schedule"),
        format!("{own_ws}/learning/today"),
    ];
    for path in &own_paths {
        ts.client2.get(path).await.check_not_found();
    }

    let oidc_owner = admin_through_provider(&ts, &fake, &org_id).await;
    for path in &own_paths {
        check_unauthorized_reason(&oidc_owner.get(path).await, "password_sign_in_required");
    }
    let unit_sched_path = format!(
        "{own_ws}/courses/{own_course}/units/{}/schedule",
        own_unit.id
    );
    check_unauthorized_reason(
        &oidc_owner
            .put(&unit_sched_path, &json!({"plannedDate": "2026-11-25"}))
            .await,
        "password_sign_in_required",
    );

    let confirmed_resp = oidc_owner
        .post("/users/me/password/confirm", &json!({"password": PASSWORD}))
        .await;
    confirmed_resp.check_no_content();
    let confirmed = Client::new(
        &ts.base_url,
        &confirmed_resp.session_cookie().expect("confirmed cookie"),
    );
    for path in &own_paths {
        confirmed.get(path).await.check_ok();
    }

    let org_course = create_course(&confirmed, ROOT_WS, "open", "Org Course").await;
    require_sso(&confirmed, &org_id, &fake, Passwords::Refused)
        .await
        .check_ok();
    for path in [
        format!("{ROOT_WS}/courses/{org_course}/schedule"),
        format!("{ROOT_WS}/learning/today"),
    ] {
        check_unauthorized_reason(&ts.client2.get(&path).await, "sso_required");
    }

    ts.teardown().await;
}
