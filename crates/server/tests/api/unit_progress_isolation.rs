//! Isolation, authorization, and non-leakage matrix for personal unit progress and
//! course progress summaries across personal spaces, shared workspaces, roles, guests,
//! administrators, SSO/password-confirmation walls, deletion grace periods, erasures,
//! public links, websockets, search, archive exports, webhooks, and audit logs.

use std::io::Read as _;
use std::sync::{Arc, Mutex, OnceLock};

use axum::Router;
use axum::routing::post;
use fvoci_model::{
    CourseProgressSummary, MemberCourseProgress, UNIT_PROGRESS_POSITION_MAX_CHARS, Unit,
    UnitProgress, WorkspaceRole,
};
use reqwest::StatusCode;
use serde_json::{Value, json};
use tokio::net::TcpListener;

use crate::admin::socket::{socket_path, socket_request};
use crate::board_broadcasts::WorkspaceSocket;
use crate::common::{
    ApiResponse, Client, PASSWORD, ROOT_WS, Setup, TEST_WORKSPACE_ID, TEST_WS, TestServer,
    USER2_USERNAME,
};
use crate::fake_oidc::FakeProvider;
use crate::oidc::{org_of, set_provider, with_key_and_private_addresses};
use crate::own_space::own_workspace;
use crate::rooms::give;
use crate::sso::admin_through_provider;

const SENTINEL_POSITION: &str = "secret-anchor-9f4e2a";
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

fn assert_summary_has_no_personal_fields(raw_json: &str, unit_ids: &[&str]) {
    for forbidden in [
        "unitId",
        "completedAt",
        "plannedDate",
        "lastPosition",
        "updatedAt",
        SENTINEL_POSITION,
    ] {
        assert!(
            !raw_json.contains(forbidden),
            "summary leaked {forbidden}: {raw_json}"
        );
    }
    for uid in unit_ids {
        assert!(
            !raw_json.contains(uid),
            "summary leaked unit id {uid}: {raw_json}"
        );
    }
}

async fn assert_all_three_not_found(client: &Client, ws: &str, course: &str, unit: &str) {
    client
        .get(&format!("{ws}/courses/{course}/progress"))
        .await
        .check_not_found();
    client
        .put(
            &format!("{ws}/courses/{course}/units/{unit}/progress"),
            &json!({"completed": true}),
        )
        .await
        .check_not_found();
    client
        .get(&format!("{ws}/courses/{course}/progress/summary"))
        .await
        .check_not_found();
}

/// Personal-space course progress is reachable only by the space owner with a confirmed
/// password session, and blocked for other users, org/workspace admins, unconfirmed OIDC
/// sessions, and accounts waiting to be deleted.
#[tokio::test]
async fn own_space_progress_isolation_sso_password_and_deletion_grace() {
    let fake = FakeProvider::start().await;
    let th = Setup::local_mode()
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
    let org_id = org_of(&th).await;
    set_provider(&th, &org_id, &fake).await;

    let user1 = th.client.me().await;
    let own = own_workspace(&th.client).await;
    let own_ws = format!("/w/{}", own.id);
    let course = create_course(&th.client, &own_ws, "private", "Solo Rust").await;
    let chapter = create_item(&th.client, &own_ws, &course, "chapter", "Part 1").await;
    let unit = create_item(&th.client, &own_ws, &course, "unit", "Lesson 1").await;

    let saved: UnitProgress = th
        .client
        .put(
            &format!("{own_ws}/courses/{course}/units/{}/progress", unit.id),
            &json!({"completed": true, "lastPosition": SENTINEL_POSITION}),
        )
        .await
        .ok_json();
    assert_eq!(saved.unit_id, unit.id);
    assert!(saved.completed_at.is_some());
    assert_eq!(saved.last_position.as_deref(), Some(SENTINEL_POSITION));
    th.client
        .put(
            &format!("{own_ws}/courses/{course}/units/{}/progress", chapter.id),
            &json!({"completed": true}),
        )
        .await
        .check_not_found();

    let mine: Vec<UnitProgress> = th
        .client
        .get(&format!("{own_ws}/courses/{course}/progress"))
        .await
        .ok_json();
    assert_eq!(mine, vec![saved]);
    let summary: CourseProgressSummary = th
        .client
        .get(&format!("{own_ws}/courses/{course}/progress/summary"))
        .await
        .ok_json();
    assert_eq!(
        summary,
        CourseProgressSummary {
            total_units: 1,
            members: vec![MemberCourseProgress {
                user_id: user1.id,
                completed_units: 1,
            }],
        }
    );

    check_own_space_blocked_for_other_admins(&th, &org_id, &own_ws, &course, &unit.id).await;
    check_own_space_blocked_for_sso_and_deletion_grace(
        &th, &fake, &org_id, &own_ws, &course, &unit.id,
    )
    .await;
    th.teardown().await;
}

async fn check_own_space_blocked_for_other_admins(
    th: &TestServer,
    org_id: &str,
    own_ws: &str,
    course: &str,
    unit_id: &str,
) {
    let user2 = th.client2.me().await;
    th.client
        .put(
            &format!("/org/{org_id}/members/{}/role", user2.id),
            &json!({"role": "admin"}),
        )
        .await
        .check_no_content();
    assert_all_three_not_found(&th.client2, own_ws, course, unit_id).await;
    assert_all_three_not_found(&th.client2, ROOT_WS, course, unit_id).await;

    if fvoci_store::testing::db_type_of_the_tests() == fvoci_store::DbType::Postgres {
        th.anonymous()
            .post(
                "/register",
                &json!({
                    "organizationName": "Other Org",
                    "username": "otherorgadmin",
                    "email": "otherorgadmin@example.com",
                    "password": PASSWORD,
                }),
            )
            .await
            .check_created();
        let outsider = th.login("otherorgadmin", PASSWORD).await;
        assert_all_three_not_found(&outsider, own_ws, course, unit_id).await;
    }
}

async fn check_own_space_blocked_for_sso_and_deletion_grace(
    th: &TestServer,
    fake: &FakeProvider,
    org_id: &str,
    own_ws: &str,
    course: &str,
    unit_id: &str,
) {
    let progress_path = format!("{own_ws}/courses/{course}/progress");
    let unit_path = format!("{own_ws}/courses/{course}/units/{unit_id}/progress");
    let summary_path = format!("{own_ws}/courses/{course}/progress/summary");

    let provider_session = admin_through_provider(th, fake, org_id).await;
    check_unauthorized_reason(
        &provider_session.get(&progress_path).await,
        "password_sign_in_required",
    );
    check_unauthorized_reason(
        &provider_session
            .put(&unit_path, &json!({"completed": true}))
            .await,
        "password_sign_in_required",
    );
    check_unauthorized_reason(
        &provider_session.get(&summary_path).await,
        "password_sign_in_required",
    );
    let confirmed_resp = provider_session
        .post("/users/me/password/confirm", &json!({"password": PASSWORD}))
        .await;
    confirmed_resp.check_no_content();
    let confirmed = Client::new(
        &th.base_url,
        &confirmed_resp.session_cookie().expect("confirmed cookie"),
    );
    confirmed.get(&progress_path).await.check_ok();
    confirmed.get(&summary_path).await.check_ok();

    let user2 = th.client2.me().await;
    let second_space = own_workspace(&th.client2).await;
    let second_ws = format!("/w/{}", second_space.id);
    let course2 = create_course(&th.client2, &second_ws, "private", "Grace Course").await;
    let unit2 = create_item(&th.client2, &second_ws, &course2, "unit", "U1").await;
    th.client
        .put(
            &format!("/org/{org_id}/members/{}/role", user2.id),
            &json!({"role": "member"}),
        )
        .await
        .check_no_content();
    th.client2
        .post(
            "/users/me/deletion",
            &json!({"username": USER2_USERNAME, "password": PASSWORD}),
        )
        .await
        .check_ok();
    let pending = th.login(USER2_USERNAME, PASSWORD).await;
    check_unauthorized_reason(
        &pending
            .get(&format!("{second_ws}/courses/{course2}/progress"))
            .await,
        "account_deletion_pending",
    );
    check_unauthorized_reason(
        &pending
            .put(
                &format!("{second_ws}/courses/{course2}/units/{}/progress", unit2.id),
                &json!({"completed": true}),
            )
            .await,
        "account_deletion_pending",
    );
    check_unauthorized_reason(
        &pending
            .get(&format!("{second_ws}/courses/{course2}/progress/summary"))
            .await,
        "account_deletion_pending",
    );
}

/// Shared-workspace course progress isolation across viewer/editor/admin, unjoined open
/// readers, managing-only workspace admins, org admins (including after `joinOrgWorkspace`),
/// guests, removed members, public links, WS, search, export, webhooks, and audit logs.
#[tokio::test]
async fn shared_course_progress_isolation_and_zero_side_channel_leakage() {
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
    let user1 = th.client.me().await;
    let user2 = th.client2.me().await;

    let private_course = create_course(&th.client, TEST_WS, "private", "Private Course").await;
    let ch = create_item(&th.client, TEST_WS, &private_course, "chapter", "Chapter 1").await;
    let u1 = create_item(&th.client, TEST_WS, &private_course, "unit", "Unit 1").await;
    let u2 = create_item(&th.client, TEST_WS, &private_course, "unit", "Unit 2").await;
    let trashed_u = create_item(&th.client, TEST_WS, &private_course, "unit", "Trashed").await;
    th.client
        .delete(&format!(
            "{TEST_WS}/courses/{private_course}/units/{}",
            trashed_u.id
        ))
        .await
        .check_no_content();
    give(&th, &private_course, &user2.id, "viewer").await;

    let (a_completed_at, ws2, audit_ws_before, audit_org_before) = check_write_validation(
        &th,
        &org_id,
        &user2.id,
        &private_course,
        &u1.id,
        &[&ch.id, &trashed_u.id],
        &hook_seen,
    )
    .await;
    check_no_side_channels(
        &th,
        &org_id,
        &private_course,
        ws2,
        &hook_seen,
        audit_ws_before,
        audit_org_before,
    )
    .await;
    check_viewer_summary_and_public_links(
        &th,
        &user1.id,
        &user2.id,
        &private_course,
        &u1.id,
        &u2.id,
        a_completed_at,
    )
    .await;
    check_open_course_isolation(&th).await;
    check_org_admin_joining_isolation(&th, &org_id, &user2.id).await;
    check_guest_and_removed_member_isolation(&th, &user1.id, &user2.id).await;
    th.teardown().await;
}

async fn check_write_validation(
    th: &TestServer,
    org_id: &str,
    user2_id: &str,
    course: &str,
    u1_id: &str,
    bad_units: &[&str],
    hook_seen: &Arc<Mutex<Vec<String>>>,
) -> (i64, WorkspaceSocket, usize, usize) {
    let ws2 = WorkspaceSocket::subscribe(th, &th.client2, TEST_WORKSPACE_ID, user2_id).await;
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

    let u1_path = format!("{TEST_WS}/courses/{course}/units/{u1_id}/progress");
    let first: UnitProgress = th
        .client
        .put(
            &u1_path,
            &json!({"completed": true, "lastPosition": format!("  {SENTINEL_POSITION}  ")}),
        )
        .await
        .ok_json();
    let completed_at = first.completed_at.expect("completed_at set");
    assert_eq!(
        first.last_position.as_deref(),
        Some(SENTINEL_POSITION),
        "lastPosition must be trimmed"
    );
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let again: UnitProgress = th
        .client
        .put(&u1_path, &json!({"completed": true}))
        .await
        .ok_json();
    assert_eq!(
        again.completed_at,
        Some(completed_at),
        "completed: true must stay idempotent"
    );
    assert_eq!(
        again.last_position.as_deref(),
        Some(SENTINEL_POSITION),
        "omitted lastPosition must be kept"
    );

    let too_long = "x".repeat(UNIT_PROGRESS_POSITION_MAX_CHARS + 1);
    for bad_pos in [too_long.as_str(), "line1\nline2"] {
        let bad = th
            .client
            .put(&u1_path, &json!({"lastPosition": bad_pos}))
            .await;
        bad.check_bad_request();
        assert_eq!(
            bad.refused_at(),
            "/lastPosition",
            "invalid lastPosition must point at /lastPosition"
        );
    }
    for bad_unit in bad_units {
        th.client
            .put(
                &format!("{TEST_WS}/courses/{course}/units/{bad_unit}/progress"),
                &json!({"completed": true}),
            )
            .await
            .check_not_found();
    }
    (
        completed_at,
        ws2,
        audit_ws_before.len(),
        audit_org_before.len(),
    )
}

async fn check_no_side_channels(
    th: &TestServer,
    org_id: &str,
    course: &str,
    mut ws2: WorkspaceSocket,
    hook_seen: &Arc<Mutex<Vec<String>>>,
    audit_ws_before: usize,
    audit_org_before: usize,
) {
    assert_eq!(
        ws2.drain().await,
        Vec::<Value>::new(),
        "progress writes must not broadcast over WebSocket"
    );
    th.app
        .finish_background(std::time::Duration::from_secs(2))
        .await;
    assert_eq!(
        *hook_seen.lock().expect("hook lock"),
        Vec::<String>::new(),
        "progress writes must not deliver webhooks"
    );
    let audit_ws_after: Vec<Value> = th.client.get(&format!("{TEST_WS}/audit")).await.ok_json();
    let audit_org_after: Vec<Value> = th
        .client
        .get(&format!("/org/{org_id}/audit"))
        .await
        .ok_json();
    assert_eq!(
        audit_ws_after.len(),
        audit_ws_before,
        "progress writes must not emit workspace audit entries"
    );
    assert_eq!(
        audit_org_after.len(),
        audit_org_before,
        "progress writes must not emit org audit entries"
    );
    let hits: Value = th
        .client2
        .get(&format!("{TEST_WS}/search?q={SENTINEL_POSITION}"))
        .await
        .ok_json();
    assert_eq!(
        hits,
        json!([]),
        "lastPosition must not appear in workspace search"
    );

    let exported = th
        .client
        .get(&format!("{TEST_WS}/containers/{course}/archive/export"))
        .await;
    exported.check_ok();
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&exported.body)).expect("zip");
    for idx in 0..archive.len() {
        let mut entry = archive.by_index(idx).expect("entry");
        let mut content = Vec::new();
        entry.read_to_end(&mut content).expect("read entry");
        let text = String::from_utf8_lossy(&content);
        assert!(!text.contains(SENTINEL_POSITION), "export leaked position");
        assert!(!text.contains("unit_progress"), "export leaked table");
    }
}

async fn check_viewer_summary_and_public_links(
    th: &TestServer,
    user1_id: &str,
    user2_id: &str,
    course: &str,
    u1_id: &str,
    u2_id: &str,
    a_completed_at: i64,
) {
    let b_mine: Vec<UnitProgress> = th
        .client2
        .get(&format!("{TEST_WS}/courses/{course}/progress"))
        .await
        .ok_json();
    assert_eq!(
        b_mine,
        Vec::<UnitProgress>::new(),
        "viewer must not see another member's progress rows"
    );
    let summary_resp = th
        .client2
        .get(&format!("{TEST_WS}/courses/{course}/progress/summary"))
        .await;
    summary_resp.check_ok();
    assert_summary_has_no_personal_fields(&summary_resp.text(), &[u1_id, u2_id]);
    let summary: CourseProgressSummary = serde_json::from_slice(&summary_resp.body).expect("json");
    let mut expected_members = vec![
        MemberCourseProgress {
            user_id: user1_id.to_owned(),
            completed_units: 1,
        },
        MemberCourseProgress {
            user_id: user2_id.to_owned(),
            completed_units: 0,
        },
    ];
    expected_members.sort_by(|a, b| a.user_id.cmp(&b.user_id));
    assert_eq!(
        summary,
        CourseProgressSummary {
            total_units: 2,
            members: expected_members,
        },
        "summary must count only live units and active members"
    );

    // Viewer CAN record own progress on u2 without altering user1's row.
    let b_put: UnitProgress = th
        .client2
        .put(
            &format!("{TEST_WS}/courses/{course}/units/{u2_id}/progress"),
            &json!({"completed": true, "lastPosition": "b-pos"}),
        )
        .await
        .ok_json();
    assert_eq!(b_put.unit_id, u2_id, "viewer recorded own unit progress");
    let a_mine: Vec<UnitProgress> = th
        .client
        .get(&format!("{TEST_WS}/courses/{course}/progress"))
        .await
        .ok_json();
    assert_eq!(a_mine.len(), 1, "caller 1 must still have only one row");
    assert_eq!(a_mine[0].unit_id, u1_id, "caller 1 unit must be untouched");
    assert_eq!(
        a_mine[0].completed_at,
        Some(a_completed_at),
        "caller 1 completedAt must be untouched"
    );
    assert_eq!(
        a_mine[0].last_position.as_deref(),
        Some(SENTINEL_POSITION),
        "caller 1 lastPosition must be untouched"
    );

    th.client
        .post(
            &format!("{TEST_WS}/containers/{course}/public-link"),
            &json!({"enabled": true}),
        )
        .await
        .check_ok();
    let sharing: Value = th
        .client
        .get(&format!("{TEST_WS}/containers/{course}/public-link"))
        .await
        .ok_json();
    let token = sharing["token"].as_str().expect("readToken");
    let anon = th.anonymous();
    for blocked in [
        format!("/shared/{course}/progress?readToken={token}"),
        format!("/shared/{course}/progress/summary?readToken={token}"),
    ] {
        anon.get(&blocked).await.check_not_found();
    }
    for shared_route in [
        format!("/shared/{course}?readToken={token}"),
        format!("/shared/{course}/units?readToken={token}"),
        format!("/shared/{course}/units/{u1_id}?readToken={token}"),
    ] {
        let resp = anon.get(&shared_route).await;
        resp.check_ok();
        let text = resp.text();
        assert!(!text.contains(SENTINEL_POSITION), "{shared_route}");
        assert!(!text.contains("completedUnits"), "{shared_route}");
        assert!(!text.contains("completedAt"), "{shared_route}");
    }
}

async fn check_open_course_isolation(th: &TestServer) {
    // Open course: unjoined worker gets 200 [] on GET, 403 on PUT, 200 on summary; after
    // joining can record own progress.
    let open = create_course(&th.client, TEST_WS, "open", "Open Course").await;
    let open_u = create_item(&th.client, TEST_WS, &open, "unit", "Open 1").await;
    th.client
        .put(
            &format!("{TEST_WS}/courses/{open}/units/{}/progress", open_u.id),
            &json!({"completed": true, "lastPosition": SENTINEL_POSITION}),
        )
        .await
        .check_ok();
    let unjoined: Vec<UnitProgress> = th
        .client2
        .get(&format!("{TEST_WS}/courses/{open}/progress"))
        .await
        .ok_json();
    assert_eq!(
        unjoined,
        Vec::<UnitProgress>::new(),
        "unjoined open-course reader must see an empty list"
    );
    th.client2
        .put(
            &format!("{TEST_WS}/courses/{open}/units/{}/progress", open_u.id),
            &json!({"completed": true}),
        )
        .await
        .check_forbidden();
    let open_summary = th
        .client2
        .get(&format!("{TEST_WS}/courses/{open}/progress/summary"))
        .await;
    open_summary.check_ok();
    assert_eq!(
        open_summary.value()["members"],
        json!([]),
        "unjoined open-course reader must receive redacted empty members list"
    );
    assert_summary_has_no_personal_fields(&open_summary.text(), &[&open_u.id]);
    th.client2
        .post(&format!("{TEST_WS}/containers/{open}/join"), &json!({}))
        .await
        .check_ok();
    th.client2
        .put(
            &format!("{TEST_WS}/courses/{open}/units/{}/progress", open_u.id),
            &json!({"completed": true}),
        )
        .await
        .check_ok();
}

async fn check_org_admin_joining_isolation(th: &TestServer, org_id: &str, user2_id: &str) {
    // Separate workspace Lab owned by user2; org admin user1 joins via `joinOrgWorkspace`.
    let ws_resp = th
        .client
        .post(
            &format!("/org/{org_id}/workspaces"),
            &json!({"name": "Lab"}),
        )
        .await;
    ws_resp.check_created();
    let lab_id = ws_resp.value()["id"].as_str().expect("lab id").to_owned();
    let lab_ws = format!("/w/{lab_id}");
    th.app
        .store()
        .for_org(org_id)
        .expect("org")
        .add_workspace_member(&lab_id, user2_id, WorkspaceRole::Admin)
        .await
        .expect("user2 in lab");
    let lab_private = create_course(&th.client2, &lab_ws, "private", "Lab Private").await;
    let private_unit = create_item(&th.client2, &lab_ws, &lab_private, "unit", "LP1").await;
    let lab_open = create_course(&th.client2, &lab_ws, "open", "Lab Open").await;
    let open_unit = create_item(&th.client2, &lab_ws, &lab_open, "unit", "LO1").await;
    for (cid, uid) in [(&lab_private, &private_unit.id), (&lab_open, &open_unit.id)] {
        th.client2
            .put(
                &format!("{lab_ws}/courses/{cid}/units/{uid}/progress"),
                &json!({"completed": true, "lastPosition": SENTINEL_POSITION}),
            )
            .await
            .check_ok();
    }

    assert_all_three_not_found(&th.client, &lab_ws, &lab_private, &private_unit.id).await;
    th.client
        .post(
            &format!("/org/{org_id}/workspaces/{lab_id}/join"),
            &json!({}),
        )
        .await
        .check_ok();
    assert_all_three_not_found(&th.client, &lab_ws, &lab_private, &private_unit.id).await;
    let oa_open: Vec<UnitProgress> = th
        .client
        .get(&format!("{lab_ws}/courses/{lab_open}/progress"))
        .await
        .ok_json();
    assert_eq!(
        oa_open,
        Vec::<UnitProgress>::new(),
        "org admin joining workspace must not see other members' progress"
    );
    th.client
        .put(
            &format!(
                "{lab_ws}/courses/{lab_open}/units/{}/progress",
                open_unit.id
            ),
            &json!({"completed": true}),
        )
        .await
        .check_forbidden();
    let oa_summary = th
        .client
        .get(&format!("{lab_ws}/courses/{lab_open}/progress/summary"))
        .await;
    oa_summary.check_ok();
    assert!(
        oa_summary.value()["members"]
            .as_array()
            .is_some_and(|m| !m.is_empty()),
        "org admin joined to workspace has ManageRoles and sees course progress summary"
    );
    assert_summary_has_no_personal_fields(&oa_summary.text(), &[&open_unit.id]);
}

async fn check_guest_and_removed_member_isolation(th: &TestServer, user1_id: &str, user2_id: &str) {
    let guest_id = crate::guests::guest_of_the_workspace(th).await;
    assert_eq!(guest_id, user2_id, "guest helper must reuse user2");
    let invited = create_course(&th.client, TEST_WS, "private", "Guest Course").await;
    let inv_u = create_item(&th.client, TEST_WS, &invited, "unit", "G1").await;
    th.client
        .put(
            &format!("{TEST_WS}/courses/{invited}/units/{}/progress", inv_u.id),
            &json!({"completed": true, "lastPosition": SENTINEL_POSITION}),
        )
        .await
        .check_ok();
    let uninvited_open = create_course(&th.client, TEST_WS, "open", "Hidden From Guest").await;
    let uninv_u = create_item(&th.client, TEST_WS, &uninvited_open, "unit", "H1").await;
    crate::references::let_read(&th.client, &invited, &guest_id).await;

    assert_all_three_not_found(&th.client2, TEST_WS, &uninvited_open, &uninv_u.id).await;
    let g_before: Vec<UnitProgress> = th
        .client2
        .get(&format!("{TEST_WS}/courses/{invited}/progress"))
        .await
        .ok_json();
    assert_eq!(
        g_before,
        Vec::<UnitProgress>::new(),
        "invited guest must not see host progress"
    );
    let g_summary = th
        .client2
        .get(&format!("{TEST_WS}/courses/{invited}/progress/summary"))
        .await;
    g_summary.check_ok();
    assert_summary_has_no_personal_fields(&g_summary.text(), &[&inv_u.id]);
    th.client2
        .put(
            &format!("{TEST_WS}/courses/{invited}/units/{}/progress", inv_u.id),
            &json!({"completed": true}),
        )
        .await
        .check_ok();

    th.client
        .delete(&format!(
            "{TEST_WS}/containers/{invited}/members/{guest_id}"
        ))
        .await
        .check_no_content();
    assert_all_three_not_found(&th.client2, TEST_WS, &invited, &inv_u.id).await;
    let after_remove: CourseProgressSummary = th
        .client
        .get(&format!("{TEST_WS}/courses/{invited}/progress/summary"))
        .await
        .ok_json();
    assert_eq!(
        after_remove.members,
        vec![MemberCourseProgress {
            user_id: user1_id.to_owned(),
            completed_units: 1,
        }],
        "removed member must disappear from course progress summary"
    );
}

async fn user_progress_unit_ids(
    th: &TestServer,
    user_id: &str,
    course: Option<&str>,
) -> Vec<String> {
    let filter = course.map_or_else(String::new, |cid| format!(" AND container_id = '{cid}'"));
    th.app
        .store()
        .system()
        .texts_unchecked(&format!(
            "SELECT CAST(unit_id AS TEXT) FROM unit_progress WHERE user_id = '{user_id}'{filter}"
        ))
        .await
        .expect("query")
}

async fn assert_summary_members(
    client: &Client,
    ws: &str,
    course: &str,
    expected: Vec<MemberCourseProgress>,
    why: &str,
) {
    let summary: CourseProgressSummary = client
        .get(&format!("{ws}/courses/{course}/progress/summary"))
        .await
        .ok_json();
    assert_eq!(summary.members, expected, "{why}");
}

async fn mark_done(client: &Client, ws: &str, course: &str, unit_id: &str, pos: &str) {
    client
        .put(
            &format!("{ws}/courses/{course}/units/{unit_id}/progress"),
            &json!({"completed": true, "lastPosition": pos}),
        )
        .await
        .check_ok();
}

/// Account deletion grace period hides the member from summary, `leaveOrganization` deletes
/// organization progress while keeping personal-space progress, and full account erasure
/// removes every `unit_progress` row of the erased account.
#[tokio::test]
async fn deletion_grace_org_leave_and_account_erasure_clean_progress() {
    let socket = socket_path();
    let th = Setup::native()
        .admin_socket(&socket)
        .start()
        .await
        .init_basic()
        .await;
    let org_id = org_of(&th).await;
    let user1 = th.client.me().await;
    let user2 = th.client2.me().await;

    let course = create_course(&th.client, ROOT_WS, "open", "Lifecycle Course").await;
    let unit = create_item(&th.client, ROOT_WS, &course, "unit", "L1").await;
    th.client2
        .post(&format!("{ROOT_WS}/containers/{course}/join"), &json!({}))
        .await
        .check_ok();
    mark_done(&th.client2, ROOT_WS, &course, &unit.id, "shared-pos").await;

    let own2 = own_workspace(&th.client2).await;
    let own2_ws = format!("/w/{}", own2.id);
    let own_course = create_course(&th.client2, &own2_ws, "private", "Own Course").await;
    let own_unit = create_item(&th.client2, &own2_ws, &own_course, "unit", "Own 1").await;
    mark_done(&th.client2, &own2_ws, &own_course, &own_unit.id, "own-pos").await;

    th.client2
        .post(
            "/users/me/deletion",
            &json!({"username": USER2_USERNAME, "password": PASSWORD}),
        )
        .await
        .check_ok();
    let only_user1 = || {
        vec![MemberCourseProgress {
            user_id: user1.id.clone(),
            completed_units: 0,
        }]
    };
    assert_summary_members(
        &th.client,
        ROOT_WS,
        &course,
        only_user1(),
        "member in deletion grace must be hidden from summary",
    )
    .await;

    let back = th.login(USER2_USERNAME, PASSWORD).await;
    back.delete("/users/me/deletion").await.check_no_content();
    let restored: CourseProgressSummary = th
        .client
        .get(&format!("{ROOT_WS}/courses/{course}/progress/summary"))
        .await
        .ok_json();
    assert!(
        restored
            .members
            .iter()
            .any(|m| m.user_id == user2.id && m.completed_units == 1)
    );

    back.post(&format!("/org/{org_id}/leave"), &json!({}))
        .await
        .check_no_content();
    assert_summary_members(
        &th.client,
        ROOT_WS,
        &course,
        only_user1(),
        "member who left organization must disappear from summary",
    )
    .await;
    assert_eq!(
        user_progress_unit_ids(&th, &user2.id, Some(&course)).await,
        Vec::<String>::new()
    );
    let own_progress: Vec<UnitProgress> = back
        .get(&format!("{own2_ws}/courses/{own_course}/progress"))
        .await
        .ok_json();
    assert_eq!(own_progress.len(), 1);
    assert_eq!(own_progress[0].last_position.as_deref(), Some("own-pos"));

    socket_request(
        &socket,
        "POST",
        &format!("/api/v1/admin/users/{USER2_USERNAME}/erase"),
        "",
    )
    .await
    .check_no_content();
    assert_eq!(
        user_progress_unit_ids(&th, &user2.id, None).await,
        Vec::<String>::new()
    );
    th.teardown().await;
}
