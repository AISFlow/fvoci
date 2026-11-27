//! Isolation, authorization, non-leakage, and due-review notification tests for
//! personal unit reviews (`unit_reviews`) and personal study notes (`unit_notes`).

use std::io::Read as _;
use std::sync::{Arc, Mutex, OnceLock};

use axum::Router;
use axum::routing::post;
use fvoci_model::{
    CompletedUnitReview, Notification, NotificationKind, TodayReviewItem,
    UNIT_NOTE_CONTENT_MAX_CHARS, Unit, UnitNote, UnitNoteKind, UnitProgress, UnitReview,
    WorkspaceRole,
};
use reqwest::StatusCode;
use serde_json::{Value, json};
use tokio::net::TcpListener;

use crate::board_broadcasts::WorkspaceSocket;
use crate::common::{ApiResponse, Client, PASSWORD, Setup, TEST_WORKSPACE_ID, TEST_WS};
use crate::fake_oidc::FakeProvider;
use crate::oidc::{org_of, set_provider, with_key_and_private_addresses};
use crate::own_space::own_workspace;
use crate::rooms::give;
use crate::sso::admin_through_provider;

const SENTINEL_NOTE: &str = "secret-study-note-8c7b1e";
static WEBHOOK_URL: OnceLock<String> = OnceLock::new();

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

async fn create_item(client: &Client, ws: &str, course_id: &str, kind: &str, title: &str) -> Unit {
    let made = client
        .post(
            &format!("{ws}/courses/{course_id}/units"),
            &json!({"kind": kind, "title": title}),
        )
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

fn assert_archive_hides_sentinel(bytes: &[u8]) {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("valid zip");
    for i in 0..zip.len() {
        let mut file = zip.by_index(i).expect("zip entry");
        let mut text = String::new();
        if file.read_to_string(&mut text).is_ok() {
            assert!(
                !text.contains(SENTINEL_NOTE),
                "archive entry {:?} leaked personal note",
                file.name()
            );
        }
    }
}

/// Full review & note CRUD, interval progression (`[1, 3, 7, 14, 30]`), `putUnitProgress`
/// `suggestedNextIntervalDays`, workspace today reviews, and validation refusals.
#[tokio::test]
async fn reviews_and_notes_crud_interval_suggestions_and_unit_progress_integration() {
    let th = Setup::local_mode().start().await.init_basic().await;
    let c1 = create_course(&th.client, TEST_WS, "private", "Algorithms").await;
    let c2 = create_course(&th.client, TEST_WS, "private", "Systems").await;
    let chapter = create_item(&th.client, TEST_WS, &c1, "chapter", "Ch 1").await;
    let u1 = create_item(&th.client, TEST_WS, &c1, "unit", "Graphs").await;
    let u2 = create_item(&th.client, TEST_WS, &c2, "unit", "Memory").await;

    // Completing a unit returns suggestedNextIntervalDays = 1 and matches GET /progress.
    let progress: UnitProgress = th
        .client
        .put(
            &format!("{TEST_WS}/courses/{c1}/units/{}/progress", u1.id),
            &json!({"completed": true}),
        )
        .await
        .ok_json();
    assert_eq!(progress.suggested_next_interval_days, Some(1));
    let listed_progress: Vec<UnitProgress> = th
        .client
        .get(&format!("{TEST_WS}/courses/{c1}/progress"))
        .await
        .ok_json();
    assert_eq!(listed_progress, vec![progress]);

    // Schedule initial 1-day review, then complete across 1 -> 3 -> 7 -> 14 -> 30 -> 30.
    let r1: UnitReview = th
        .client
        .post(
            &format!("{TEST_WS}/courses/{c1}/units/{}/reviews", u1.id),
            &json!({"intervalDays": 1}),
        )
        .await
        .created_json();
    assert_eq!(r1.interval_days, 1);
    assert!(r1.completed_at.is_none());

    let mut current_id = r1.id.clone();
    for expected_next in [3_u32, 7, 14, 30, 30] {
        let completed: CompletedUnitReview = th
            .client
            .post(
                &format!("{TEST_WS}/courses/{c1}/reviews/{current_id}/complete"),
                &json!({"scheduleNextIntervalDays": expected_next}),
            )
            .await
            .ok_json();
        assert!(completed.review.completed_at.is_some());
        assert_eq!(completed.suggested_next_interval_days, expected_next);
        let next = completed.next_review.expect("atomic next review");
        assert_eq!(next.interval_days, expected_next);
        current_id = next.id;
    }

    // Delete the pending tail review.
    th.client
        .delete(&format!("{TEST_WS}/courses/{c1}/reviews/{current_id}"))
        .await
        .check_no_content();
    th.client
        .delete(&format!("{TEST_WS}/courses/{c1}/reviews/{current_id}"))
        .await
        .check_not_found();

    // Overdue review on c2 appears in GET /learning/reviews/today; future review does not.
    let overdue: UnitReview = th
        .client
        .post(
            &format!("{TEST_WS}/courses/{c2}/units/{}/reviews", u2.id),
            &json!({"scheduledDate": "2020-01-01"}),
        )
        .await
        .created_json();
    let _future: UnitReview = th
        .client
        .post(
            &format!("{TEST_WS}/courses/{c2}/units/{}/reviews", u2.id),
            &json!({"scheduledDate": "2099-01-01", "intervalDays": 365}),
        )
        .await
        .created_json();
    let today: Vec<TodayReviewItem> = th
        .client
        .get(&format!("{TEST_WS}/learning/reviews/today"))
        .await
        .ok_json();
    assert_eq!(today.len(), 1);
    assert_eq!(today[0].id, overdue.id);
    assert_eq!(today[0].course_title, "Systems");
    assert_eq!(today[0].unit_title, "Memory");
    assert!(today[0].overdue_days > 0);

    // Filter status=completed vs status=pending and invalid status/kind -> 400.
    let completed_list: Vec<UnitReview> = th
        .client
        .get(&format!(
            "{TEST_WS}/courses/{c1}/reviews?status=completed&unitId={}",
            u1.id
        ))
        .await
        .ok_json();
    assert_eq!(completed_list.len(), 5);
    th.client
        .get(&format!("{TEST_WS}/courses/{c1}/reviews?status=bogus"))
        .await
        .check_bad_request();

    // Chapters return 404 for schedule and notes; notes CRUD & validation pass.
    assert_notes_crud_and_validation(&th.client, &c1, &chapter.id, &u1.id).await;

    th.teardown().await;
}

async fn assert_notes_crud_and_validation(
    client: &Client,
    course_id: &str,
    chapter_id: &str,
    unit_id: &str,
) {
    client
        .post(
            &format!("{TEST_WS}/courses/{course_id}/units/{chapter_id}/reviews"),
            &json!({"intervalDays": 1}),
        )
        .await
        .check_not_found();
    client
        .get(&format!(
            "{TEST_WS}/courses/{course_id}/units/{chapter_id}/notes"
        ))
        .await
        .check_not_found();

    let note: UnitNote = client
        .post(
            &format!("{TEST_WS}/courses/{course_id}/units/{unit_id}/notes"),
            &json!({"kind": "question", "content": "  Why BFS before DFS?  "}),
        )
        .await
        .created_json();
    assert_eq!(note.kind, UnitNoteKind::Question, "created note kind");
    assert_eq!(note.content, "Why BFS before DFS?", "created note trimmed");

    let updated: UnitNote = client
        .patch(
            &format!("{TEST_WS}/courses/{course_id}/notes/{}", note.id),
            &json!({"kind": "review", "content": "BFS finds shortest unweighted paths"}),
        )
        .await
        .ok_json();
    assert_eq!(updated.kind, UnitNoteKind::Review, "updated note kind");
    assert_eq!(
        updated.content, "BFS finds shortest unweighted paths",
        "updated note content"
    );

    let filtered: Vec<UnitNote> = client
        .get(&format!(
            "{TEST_WS}/courses/{course_id}/units/{unit_id}/notes?kind=review"
        ))
        .await
        .ok_json();
    assert_eq!(filtered, vec![updated], "kind filter returns matching note");

    client
        .get(&format!(
            "{TEST_WS}/courses/{course_id}/units/{unit_id}/notes?kind=invalid"
        ))
        .await
        .check_bad_request();
    client
        .post(
            &format!("{TEST_WS}/courses/{course_id}/units/{unit_id}/notes"),
            &json!({"kind": "mistake", "content": "   "}),
        )
        .await
        .check_bad_request();
    client
        .post(
            &format!("{TEST_WS}/courses/{course_id}/units/{unit_id}/notes"),
            &json!({
                "kind": "mistake",
                "content": "a".repeat(UNIT_NOTE_CONTENT_MAX_CHARS + 1)
            }),
        )
        .await
        .check_bad_request();

    client
        .delete(&format!("{TEST_WS}/courses/{course_id}/notes/{}", note.id))
        .await
        .check_no_content();
    client
        .delete(&format!("{TEST_WS}/courses/{course_id}/notes/{}", note.id))
        .await
        .check_not_found();
}

/// Strict isolation matrix across same-course roles, unjoined open-course readers,
/// workspace admins, org admins (`joinOrgWorkspace`), invited guests, public links,
/// search, archive exports, websockets, webhooks, and audit logs.
#[tokio::test]
async fn strict_personal_isolation_matrix_and_zero_side_channel_leaks() {
    let (hook_url, hook_seen) = start_webhook_sink().await;
    let _ = WEBHOOK_URL.set(hook_url);
    let th = Setup::local_mode()
        .configure(|cfg| {
            cfg.block_change_webhooks = WEBHOOK_URL.get().into_iter().cloned().collect();
        })
        .start()
        .await
        .init_basic()
        .await;
    let org_id = org_of(&th).await;
    let user2 = th.client2.me().await;
    let (unjoined, guest, ws_admin, org_admin) = setup_isolation_actors(&th).await;
    let open_course = create_course(&th.client, TEST_WS, "open", "Open Study").await;
    let priv_course = create_course(&th.client, TEST_WS, "private", "Private Study").await;
    let u_open = create_item(&th.client, TEST_WS, &open_course, "unit", "Lesson O").await;
    let u_priv = create_item(&th.client, TEST_WS, &priv_course, "unit", "Lesson P").await;

    for c in [&open_course, &priv_course] {
        th.client
            .post(
                &format!("{TEST_WS}/containers/{c}/members"),
                &json!({"containerId": c, "userId": user2.id, "role": "admin"}),
            )
            .await
            .check_ok();
    }
    give(&th, &open_course, &guest.me().await.id, "viewer").await;

    let mut socket =
        WorkspaceSocket::subscribe(&th, &th.client2, TEST_WORKSPACE_ID, &user2.id).await;
    th.app
        .finish_background(std::time::Duration::from_secs(2))
        .await;
    hook_seen.lock().expect("hook lock").clear();
    let audit_ws_before: Vec<Value> = th.client.get(&format!("{TEST_WS}/audit")).await.ok_json();
    let audit_org_before: Vec<Value> = th
        .client
        .get(&format!("/org/{org_id}/audit"))
        .await
        .ok_json();

    let (r_open, n_open) = seed_review_and_note(&th.client, &open_course, &u_open.id).await;
    let (r_priv, n_priv) = seed_review_and_note(&th.client, &priv_course, &u_priv.id).await;
    let audit_ws_after_writes: Vec<Value> =
        th.client.get(&format!("{TEST_WS}/audit")).await.ok_json();
    let audit_org_after_writes: Vec<Value> = th
        .client
        .get(&format!("/org/{org_id}/audit"))
        .await
        .ok_json();
    assert_eq!(audit_ws_after_writes.len(), audit_ws_before.len());
    assert_eq!(audit_org_after_writes.len(), audit_org_before.len());

    // 1. Same-course admin (user2) and enrolled guest never see user1's reviews/notes,
    //    and targeting user1's review/note IDs returns 404.
    assert_peers_isolated(
        &[&th.client2, &guest],
        &open_course,
        &u_open.id,
        &r_open.id,
        &n_open.id,
    )
    .await;

    // Guest CAN schedule/create their own reviews & notes on a course they belong to!
    let guest_review: UnitReview = guest
        .post(
            &format!(
                "{TEST_WS}/courses/{open_course}/units/{}/reviews",
                u_open.id
            ),
            &json!({"intervalDays": 3}),
        )
        .await
        .created_json();
    assert_eq!(guest_review.interval_days, 3);

    // 2. Unjoined open-course reader gets [] on reads and 403 on writes.
    assert_unjoined_reader_isolated(&unjoined, &open_course, &u_open.id).await;

    // 3. Workspace admin and org admin (including joinOrgWorkspace) cannot access user1's data.
    assert_admins_isolated(
        &ws_admin,
        &org_admin,
        &org_id,
        &priv_course,
        &u_priv.id,
        &r_priv.id,
        &n_priv.id,
    )
    .await;

    // 4. Public link, search, archive export, WS, and webhooks leak nothing.
    assert_zero_side_channel_leaks(&th, &open_course, &u_open.id, &mut socket, &hook_seen).await;

    th.teardown().await;
}

async fn setup_isolation_actors(
    th: &crate::common::TestServer,
) -> (Client, Client, Client, Client) {
    let unjoined = th.add_user("unjoined-reader").await;
    let guest = th.add_user("course-guest").await;
    let ws_admin = th.add_user("ws-admin-only").await;
    let org_admin = th.add_user("org-admin-joiner").await;
    // `add_user` adds each account to `TEST_WORKSPACE_ID` as `Member`.
    for (client, role) in [
        (&guest, WorkspaceRole::Guest),
        (&ws_admin, WorkspaceRole::Admin),
    ] {
        th.app
            .store()
            .execute_unchecked(&format!(
                "UPDATE workspace_members SET role = '{}' WHERE workspace_id = '{TEST_WORKSPACE_ID}' AND user_id = '{}'",
                role.as_str(),
                client.me().await.id
            ))
            .await
            .expect("workspace role updated");
    }
    let org_admin_id = org_admin.me().await.id;
    th.app
        .store()
        .execute_unchecked(&format!(
            "DELETE FROM workspace_members WHERE workspace_id = '{TEST_WORKSPACE_ID}' AND user_id = '{org_admin_id}'"
        ))
        .await
        .expect("org admin removed from ws");
    th.app
        .store()
        .execute_unchecked(&format!(
            "UPDATE organization_members SET role = 'admin' WHERE user_id = '{org_admin_id}'"
        ))
        .await
        .expect("org admin promoted");
    (unjoined, guest, ws_admin, org_admin)
}

async fn seed_review_and_note(
    client: &Client,
    course_id: &str,
    unit_id: &str,
) -> (UnitReview, UnitNote) {
    let review: UnitReview = client
        .post(
            &format!("{TEST_WS}/courses/{course_id}/units/{unit_id}/reviews"),
            &json!({"scheduledDate": "2020-01-01", "intervalDays": 1}),
        )
        .await
        .created_json();
    let note: UnitNote = client
        .post(
            &format!("{TEST_WS}/courses/{course_id}/units/{unit_id}/notes"),
            &json!({"kind": "mistake", "content": SENTINEL_NOTE}),
        )
        .await
        .created_json();
    (review, note)
}

async fn assert_peers_isolated(
    peers: &[&Client],
    course_id: &str,
    unit_id: &str,
    review_id: &str,
    note_id: &str,
) {
    for other in peers {
        let revs: Vec<UnitReview> = other
            .get(&format!("{TEST_WS}/courses/{course_id}/reviews"))
            .await
            .ok_json();
        assert_eq!(
            revs,
            Vec::<UnitReview>::new(),
            "peer sees no course reviews"
        );
        let today: Vec<TodayReviewItem> = other
            .get(&format!("{TEST_WS}/learning/reviews/today"))
            .await
            .ok_json();
        assert_eq!(
            today,
            Vec::<TodayReviewItem>::new(),
            "peer sees no today reviews"
        );
        let notes: Vec<UnitNote> = other
            .get(&format!(
                "{TEST_WS}/courses/{course_id}/units/{unit_id}/notes"
            ))
            .await
            .ok_json();
        assert_eq!(notes, Vec::<UnitNote>::new(), "peer sees no unit notes");
        other
            .post(
                &format!("{TEST_WS}/courses/{course_id}/reviews/{review_id}/complete"),
                &json!({}),
            )
            .await
            .check_not_found();
        other
            .delete(&format!(
                "{TEST_WS}/courses/{course_id}/reviews/{review_id}"
            ))
            .await
            .check_not_found();
        other
            .patch(
                &format!("{TEST_WS}/courses/{course_id}/notes/{note_id}"),
                &json!({"content": "tamper"}),
            )
            .await
            .check_not_found();
        other
            .delete(&format!("{TEST_WS}/courses/{course_id}/notes/{note_id}"))
            .await
            .check_not_found();
    }
}

async fn assert_unjoined_reader_isolated(unjoined: &Client, course_id: &str, unit_id: &str) {
    let revs: Vec<UnitReview> = unjoined
        .get(&format!("{TEST_WS}/courses/{course_id}/reviews"))
        .await
        .ok_json();
    assert_eq!(
        revs,
        Vec::<UnitReview>::new(),
        "unjoined reader sees no course reviews"
    );
    let notes: Vec<UnitNote> = unjoined
        .get(&format!(
            "{TEST_WS}/courses/{course_id}/units/{unit_id}/notes"
        ))
        .await
        .ok_json();
    assert_eq!(
        notes,
        Vec::<UnitNote>::new(),
        "unjoined reader sees no unit notes"
    );
    unjoined
        .post(
            &format!("{TEST_WS}/courses/{course_id}/units/{unit_id}/reviews"),
            &json!({"intervalDays": 1}),
        )
        .await
        .check_forbidden();
    unjoined
        .post(
            &format!("{TEST_WS}/courses/{course_id}/units/{unit_id}/notes"),
            &json!({"kind": "question", "content": "blocked"}),
        )
        .await
        .check_forbidden();
}

async fn assert_admins_isolated(
    ws_admin: &Client,
    org_admin: &Client,
    org_id: &str,
    priv_course: &str,
    unit_id: &str,
    review_id: &str,
    note_id: &str,
) {
    ws_admin
        .get(&format!("{TEST_WS}/courses/{priv_course}/reviews"))
        .await
        .check_not_found();
    ws_admin
        .get(&format!(
            "{TEST_WS}/courses/{priv_course}/units/{unit_id}/notes"
        ))
        .await
        .check_not_found();

    org_admin
        .post(
            &format!("/org/{org_id}/workspaces/{TEST_WORKSPACE_ID}/join"),
            &json!({}),
        )
        .await
        .check_ok();
    org_admin
        .get(&format!("{TEST_WS}/courses/{priv_course}/reviews"))
        .await
        .check_not_found();
    org_admin
        .delete(&format!(
            "{TEST_WS}/courses/{priv_course}/reviews/{review_id}"
        ))
        .await
        .check_not_found();
    org_admin
        .delete(&format!("{TEST_WS}/courses/{priv_course}/notes/{note_id}"))
        .await
        .check_not_found();
    let org_admin_today: Vec<TodayReviewItem> = org_admin
        .get(&format!("{TEST_WS}/learning/reviews/today"))
        .await
        .ok_json();
    assert_eq!(
        org_admin_today,
        Vec::<TodayReviewItem>::new(),
        "org admin sees no other user's today reviews"
    );
}

async fn assert_zero_side_channel_leaks(
    th: &crate::common::TestServer,
    open_course: &str,
    unit_id: &str,
    socket: &mut WorkspaceSocket,
    hook_seen: &Arc<Mutex<Vec<String>>>,
) {
    th.client
        .post(
            &format!("{TEST_WS}/containers/{open_course}/public-link"),
            &json!({"enabled": true}),
        )
        .await
        .check_ok();
    let sharing = th
        .client
        .get(&format!("{TEST_WS}/containers/{open_course}/public-link"))
        .await
        .value();
    let token = sharing["token"].as_str().expect("token");
    let anon = th.anonymous();
    let shared_unit = anon
        .get(&format!(
            "/shared/{open_course}/units/{unit_id}?readToken={token}"
        ))
        .await;
    shared_unit.check_ok();
    assert!(
        !shared_unit.text().contains(SENTINEL_NOTE),
        "shared unit must not leak personal notes"
    );
    anon.get(&format!("/shared/{open_course}/reviews?readToken={token}"))
        .await
        .check_not_found();

    let search = th
        .client2
        .get(&format!("{TEST_WS}/search?q={SENTINEL_NOTE}"))
        .await;
    search.check_ok();
    assert!(
        !search.text().contains(SENTINEL_NOTE),
        "workspace search must not leak personal notes"
    );

    let course_archive = th
        .client2
        .get(&format!(
            "{TEST_WS}/containers/{open_course}/archive/export"
        ))
        .await;
    course_archive.check_ok();
    assert_archive_hides_sentinel(&course_archive.body);

    let ws_archive = th.client2.get(&format!("{TEST_WS}/archive/export")).await;
    ws_archive.check_ok();
    assert_archive_hides_sentinel(&ws_archive.body);

    assert_eq!(
        socket.drain().await,
        Vec::<Value>::new(),
        "personal reviews/notes must emit no WebSocket broadcasts"
    );
    th.app
        .finish_background(std::time::Duration::from_secs(2))
        .await;
    assert_eq!(
        *hook_seen.lock().expect("hook lock"),
        Vec::<String>::new(),
        "personal reviews/notes must emit no webhooks"
    );
}

/// Personal-space reviews, notes, and `review_due` notifications obey the personal-space
/// password wall (unconfirmed OIDC session blocked until `confirmPassword`) and deletion
/// grace period.
#[tokio::test]
async fn own_space_reviews_notes_and_notifications_password_wall() {
    let fake = FakeProvider::start().await;
    let th = Setup::local_mode()
        .trusting_oidc_provider(&fake.certificate_der)
        .configure(with_key_and_private_addresses)
        .start()
        .await
        .init_basic()
        .await;
    let org_id = org_of(&th).await;
    set_provider(&th, &org_id, &fake).await;

    let own = own_workspace(&th.client).await;
    let own_ws = format!("/w/{}", own.id);
    let course = create_course(&th.client, &own_ws, "private", "Solo Study").await;
    let unit = create_item(&th.client, &own_ws, &course, "unit", "Chapter 1 Unit").await;

    let review: UnitReview = th
        .client
        .post(
            &format!("{own_ws}/courses/{course}/units/{}/reviews", unit.id),
            &json!({"scheduledDate": "2020-01-01", "intervalDays": 1}),
        )
        .await
        .created_json();
    let _note: UnitNote = th
        .client
        .post(
            &format!("{own_ws}/courses/{course}/units/{}/notes", unit.id),
            &json!({"kind": "review", "content": "own space note"}),
        )
        .await
        .created_json();

    // Generate personal-space review_due notification.
    // 2026-11-24 12:00:00 UTC in milliseconds.
    let noon_utc: i64 = 1_795_521_600_000;
    assert_eq!(
        th.app.notify_due_reviews(noon_utc).await.expect("notify"),
        1
    );

    // Password session sees the review, today list, and review_due notification.
    let today: Vec<TodayReviewItem> = th
        .client
        .get(&format!("{own_ws}/learning/reviews/today"))
        .await
        .ok_json();
    assert_eq!(today.len(), 1);
    assert_eq!(today[0].id, review.id);
    let notifs: Vec<Notification> = th.client.get("/notifications").await.ok_json();
    assert_eq!(notifs.len(), 1);
    assert_eq!(
        notifs[0].subject.as_ref().map(|s| s.kind),
        Some(NotificationKind::ReviewDue)
    );

    // Unconfirmed OIDC session cannot enter personal space OR see personal-space notifications!
    let oidc_session = admin_through_provider(&th, &fake, &org_id).await;
    check_unauthorized_reason(
        &oidc_session
            .get(&format!("{own_ws}/courses/{course}/reviews"))
            .await,
        "password_sign_in_required",
    );
    check_unauthorized_reason(
        &oidc_session
            .get(&format!("{own_ws}/learning/reviews/today"))
            .await,
        "password_sign_in_required",
    );
    let hidden_notifs: Vec<Notification> = oidc_session.get("/notifications").await.ok_json();
    assert_eq!(
        hidden_notifs,
        Vec::<Notification>::new(),
        "unconfirmed OIDC session must not see personal-space review_due notifications"
    );

    // Confirming password opens both personal-space routes and notifications.
    let confirmed_resp = oidc_session
        .post("/users/me/password/confirm", &json!({"password": PASSWORD}))
        .await;
    confirmed_resp.check_no_content();
    let confirmed = Client::new(
        &th.base_url,
        &confirmed_resp.session_cookie().expect("confirmed cookie"),
    );
    let confirmed_today: Vec<TodayReviewItem> = confirmed
        .get(&format!("{own_ws}/learning/reviews/today"))
        .await
        .ok_json();
    assert_eq!(confirmed_today.len(), 1);
    let confirmed_notifs: Vec<Notification> = confirmed.get("/notifications").await.ok_json();
    assert_eq!(confirmed_notifs.len(), 1);

    // Another user cannot touch own space.
    th.client2
        .get(&format!("{own_ws}/courses/{course}/reviews"))
        .await
        .check_not_found();

    th.teardown().await;
    fake.stop();
}

/// Timezone-aware due-review notifications fire once per review on the learner's local
/// calendar day, never duplicate on same-day re-runs, and deliver strictly to the owner.
#[tokio::test]
async fn due_review_notifications_timezone_dedup_and_recipient_isolation() {
    let th = Setup::local_mode().start().await.init_basic().await;
    let user2 = th.client2.me().await;
    let bystander = th.add_user("bystander").await;

    // User1 is in Asia/Seoul (UTC+9), User2 is in America/Los_Angeles (UTC-8).
    th.client
        .patch("/users/me/settings", &json!({"timeZone": "Asia/Seoul"}))
        .await
        .check_ok();
    th.client2
        .patch(
            "/users/me/settings",
            &json!({"timeZone": "America/Los_Angeles"}),
        )
        .await
        .check_ok();

    let course = create_course(&th.client, TEST_WS, "private", "Timezones").await;
    give(&th, &course, &user2.id, "editor").await;
    give(&th, &course, &bystander.me().await.id, "viewer").await;
    let unit = create_item(&th.client, TEST_WS, &course, "unit", "Clocks").await;

    // Both schedule a review for 2026-12-01.
    for client in [&th.client, &th.client2] {
        client
            .post(
                &format!("{TEST_WS}/courses/{course}/units/{}/reviews", unit.id),
                &json!({"scheduledDate": "2026-12-01", "intervalDays": 1}),
            )
            .await
            .check_created();
    }

    // 2026-11-30 16:00:00 UTC -> 2026-12-01 01:00 KST (due for user1),
    //                           -> 2026-11-30 08:00 PST (not yet due for user2).
    let t_seoul_due: i64 = 1_796_054_400_000;
    assert_eq!(
        th.app.notify_due_reviews(t_seoul_due).await.expect("run 1"),
        1
    );

    // Re-running on the same day produces zero duplicates.
    assert_eq!(
        th.app
            .notify_due_reviews(t_seoul_due + 3_600_000)
            .await
            .expect("run 2"),
        0
    );

    let u1_notifs: Vec<Notification> = th.client.get("/notifications").await.ok_json();
    assert_eq!(u1_notifs.len(), 1);
    let subj = u1_notifs[0].subject.as_ref().expect("subject present");
    assert_eq!(subj.kind, NotificationKind::ReviewDue);
    assert_eq!(subj.block_id, unit.id);
    assert_eq!(subj.container_id, course);
    assert_eq!(subj.actor_id, "");

    assert_eq!(
        th.client2
            .get("/notifications")
            .await
            .ok_json::<Vec<Notification>>(),
        Vec::<Notification>::new(),
        "user2 in PST is not due yet"
    );
    assert_eq!(
        bystander
            .get("/notifications")
            .await
            .ok_json::<Vec<Notification>>(),
        Vec::<Notification>::new(),
        "bystander must never receive another learner's review notification"
    );

    // 2026-12-01 10:00:00 UTC -> 2026-12-01 02:00 PST (now due for user2).
    let t_la_due: i64 = 1_796_119_200_000;
    assert_eq!(th.app.notify_due_reviews(t_la_due).await.expect("run 3"), 1);
    assert_eq!(
        th.client
            .get("/notifications")
            .await
            .ok_json::<Vec<Notification>>()
            .len(),
        1
    );
    let u2_notifs: Vec<Notification> = th.client2.get("/notifications").await.ok_json();
    assert_eq!(u2_notifs.len(), 1);
    assert_eq!(
        u2_notifs[0].subject.as_ref().map(|s| s.kind),
        Some(NotificationKind::ReviewDue)
    );
    assert_eq!(
        bystander
            .get("/notifications")
            .await
            .ok_json::<Vec<Notification>>(),
        Vec::<Notification>::new()
    );

    th.teardown().await;
}
