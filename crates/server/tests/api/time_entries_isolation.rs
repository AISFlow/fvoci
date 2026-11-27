//! Privacy & isolation matrix for personal time tracking (`/users/me/time/*`):
//!
//! - Nobody else (same board/course member, workspace admin, org admin including after
//!   `joinOrgWorkspace`, another org's admin, workspace guest, public link reader,
//!   unauthenticated client) can read, stop, edit, or delete a user's time entries.
//! - Nobody can link time entries to private boards/courses they cannot read (nor probe
//!   their existence behind an SSO wall when omitting `workspaceId`).
//! - Zero leakage into exports, content search, audit logs, webhooks, or websockets.
//! - Personal space password wall (`password_sign_in_required`), org departure cleanup,
//!   and account erasure cleanup.

use std::io::{Cursor, Read as _};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use axum::Router;
use axum::routing::post;
use futures_util::{SinkExt as _, StreamExt as _};
use fvoci_model::{
    ActiveTimeEntry, Container, ContainerMember, ContainerVisibility, OrgRole, StartedTimeEntry,
    TimeEntry, TimeEntryList, Unit,
};
use fvoci_store::DbType;
use fvoci_store::testing::db_type_of_the_tests;
use reqwest::StatusCode;
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;
use zip::ZipArchive;

use crate::boards::new_board;
use crate::common::{
    Client, PASSWORD, ROOT_WORKSPACE_ID, Setup, TEST_WORKSPACE_ID, TEST_WS, TestServer,
};
use crate::content_search::set_role;
use crate::fake_oidc::FakeProvider;
use crate::oidc::{org_of, server_for, set_provider};
use crate::own_space::own_workspace;
use crate::rooms::give;
use crate::sso::{ADMIN_SUBJECT, Passwords, require_sso, through_provider, tie};
use crate::workspaces::another_organization;

async fn create_card(client: &Client, ws_id: &str, board_id: &str, title: &str) -> String {
    let card: Value = client
        .post(
            &format!("/w/{ws_id}/boards/{board_id}/cards"),
            &json!({"title": title}),
        )
        .await
        .created_json();
    card["id"].as_str().expect("card id").to_owned()
}

struct VictimScenario {
    board_id: String,
    card_id: String,
    unit_id: String,
    completed_id: String,
    active_id: String,
    co_member: Client,
    guest: Client,
    org_admin: Client,
}

#[derive(Clone, Copy)]
enum MemberRole {
    Admin,
    Editor,
    Viewer,
}

async fn add_container_role_in(
    owner: &Client,
    workspace_id: &str,
    container_id: &str,
    user_id: &str,
    role: MemberRole,
) {
    let member = ContainerMember {
        container_id: container_id.to_owned(),
        user_id: user_id.to_owned(),
        role: match role {
            MemberRole::Admin => fvoci_model::ContainerRole::Admin,
            MemberRole::Editor => fvoci_model::ContainerRole::Editor,
            MemberRole::Viewer => fvoci_model::ContainerRole::Viewer,
        },
        synthetic: false,
        inherited_role: None,
    };
    owner.add_member_in(workspace_id, &member).await.check_ok();
}

async fn add_container_role(owner: &Client, container_id: &str, user_id: &str, role: MemberRole) {
    add_container_role_in(owner, TEST_WORKSPACE_ID, container_id, user_id, role).await;
}

async fn seed_victim_scenario(th: &TestServer) -> VictimScenario {
    let victim = &th.client2;
    let victim_board: Container = victim
        .create_board(&new_board(
            TEST_WORKSPACE_ID,
            ContainerVisibility::Private,
            "Victim Board",
        ))
        .await
        .created_json();
    let card_id = create_card(victim, TEST_WORKSPACE_ID, &victim_board.id, "Secret Card").await;
    let victim_course: Value = victim
        .post(
            &format!("{TEST_WS}/courses"),
            &json!({"visibility": "private", "title": "Victim Course"}),
        )
        .await
        .created_json();
    let course_id = victim_course["id"].as_str().expect("course id").to_owned();
    let unit: Unit = victim
        .post(
            &format!("{TEST_WS}/courses/{course_id}/units"),
            &json!({"kind": "unit", "title": "Secret Unit"}),
        )
        .await
        .created_json();

    let co_member = th.add_user("comember").await;
    let co_id = co_member.me().await.id;
    add_container_role(victim, &victim_board.id, &co_id, MemberRole::Admin).await;
    add_container_role(victim, &course_id, &co_id, MemberRole::Admin).await;

    let guest = th.add_user("guest_user").await;
    let guest_id = guest.me().await.id;
    set_role(th, &guest_id, "guest").await;
    add_container_role(victim, &victim_board.id, &guest_id, MemberRole::Editor).await;
    add_container_role(victim, &course_id, &guest_id, MemberRole::Editor).await;

    let org_admin = th.add_user("orgadmin").await;
    let org_admin_id = org_admin.me().await.id;
    th.app
        .store()
        .execute_unchecked(&format!(
            "UPDATE organization_members SET role = '{}' WHERE user_id = '{org_admin_id}'",
            OrgRole::Admin.as_str(),
        ))
        .await
        .expect("org admin role");
    th.app
        .store()
        .remove_workspace_member(TEST_WORKSPACE_ID, &org_admin_id, "")
        .await
        .expect("remove from ws first");

    let completed: TimeEntry = victim
        .post(
            "/users/me/time/entries",
            &json!({
                "workspaceId": TEST_WORKSPACE_ID,
                "blockId": unit.id,
                "kind": "lecture",
                "startedAt": 10_000,
                "endedAt": 40_000,
                "note": "private-time-secret-token",
            }),
        )
        .await
        .created_json();
    let active: StartedTimeEntry = victim
        .post(
            "/users/me/time/start",
            &json!({
                "workspaceId": TEST_WORKSPACE_ID,
                "blockId": card_id,
                "kind": "work",
                "note": "private-active-secret",
            }),
        )
        .await
        .created_json();

    VictimScenario {
        board_id: victim_board.id,
        card_id,
        unit_id: unit.id,
        completed_id: completed.id,
        active_id: active.entry.id,
        co_member,
        guest,
        org_admin,
    }
}

async fn assert_sees_nothing_of_victim(actor: &Client, label: &str, entry_ids: &[&str]) {
    let active: ActiveTimeEntry = actor.get("/users/me/time/active").await.ok_json();
    assert_eq!(
        active.entry, None,
        "{label} must not see victim active timer"
    );
    let listed: TimeEntryList = actor.get("/users/me/time/entries").await.ok_json();
    assert!(
        listed.entries.is_empty(),
        "{label} must not list victim entries"
    );
    assert_eq!(listed.summary.total_ms, 0, "{label} must see 0 total");
    assert_eq!(listed.summary.count, 0, "{label} must see 0 count");
    actor
        .post_empty("/users/me/time/stop")
        .await
        .check_not_found();
    for entry_id in entry_ids {
        actor
            .patch(
                &format!("/users/me/time/entries/{entry_id}"),
                &json!({"note": "stolen"}),
            )
            .await
            .check_not_found();
        actor
            .delete(&format!("/users/me/time/entries/{entry_id}"))
            .await
            .check_not_found();
    }
}

async fn assert_unauthenticated_and_public_link_refused(th: &TestServer, s: &VictimScenario) {
    let anon = th.anonymous();
    anon.get("/users/me/time/active").await.check_unauthorized();
    anon.get("/users/me/time/entries")
        .await
        .check_unauthorized();
    anon.post("/users/me/time/start", &json!({"kind": "work"}))
        .await
        .check_unauthorized();
    anon.post_empty("/users/me/time/stop")
        .await
        .check_unauthorized();
    anon.patch(
        &format!("/users/me/time/entries/{}", s.completed_id),
        &json!({"note": "x"}),
    )
    .await
    .check_unauthorized();
    anon.delete(&format!("/users/me/time/entries/{}", s.completed_id))
        .await
        .check_unauthorized();

    let read_token = th.share(&s.board_id).await;
    anon.get(&format!("/users/me/time/entries?readToken={read_token}"))
        .await
        .check_unauthorized();
}

async fn assert_private_targets_refused(client: &Client, who: &str, block_ids: &[&str]) {
    for block_id in block_ids {
        let r1 = client
            .post(
                "/users/me/time/start",
                &json!({"workspaceId": TEST_WORKSPACE_ID, "blockId": block_id, "kind": "work"}),
            )
            .await;
        assert_eq!(
            r1.status,
            StatusCode::NOT_FOUND,
            "{who} targeting private block"
        );
        let r2 = client
            .post(
                "/users/me/time/start",
                &json!({"blockId": block_id, "kind": "work"}),
            )
            .await;
        assert_eq!(
            r2.status,
            StatusCode::NOT_FOUND,
            "{who} targeting private block without workspaceId"
        );
    }
}

/// Full actor isolation matrix: board/course co-member, workspace admin, org admin
/// (including after `joinOrgWorkspace`), other-org admin (PG), invited guest, public link
/// reader, and unauthenticated caller cannot read/modify/delete victim's time entries,
/// nor can non-members start timers against victim's private board/course.
#[tokio::test]
async fn nobody_else_can_read_modify_delete_or_target_private_time_entries() {
    let th = Setup::local_mode().start().await.init_basic().await;
    let org_id = org_of(&th).await;
    let s = seed_victim_scenario(&th).await;
    let victim_ids = [s.completed_id.as_str(), s.active_id.as_str()];

    assert_unauthenticated_and_public_link_refused(&th, &s).await;
    assert_sees_nothing_of_victim(&th.client, "workspace admin", &victim_ids).await;
    assert_sees_nothing_of_victim(&s.co_member, "co-member", &victim_ids).await;
    assert_sees_nothing_of_victim(&s.guest, "invited guest", &victim_ids).await;
    assert_sees_nothing_of_victim(&s.org_admin, "org admin before join", &victim_ids).await;

    s.org_admin
        .post_empty(&format!(
            "/org/{org_id}/workspaces/{TEST_WORKSPACE_ID}/join"
        ))
        .await
        .check_ok();
    assert_sees_nothing_of_victim(&s.org_admin, "org admin after join", &victim_ids).await;

    let private_blocks = [s.card_id.as_str(), s.unit_id.as_str()];
    assert_private_targets_refused(&th.client, "workspace admin", &private_blocks).await;
    assert_private_targets_refused(&s.org_admin, "org admin", &private_blocks).await;

    if db_type_of_the_tests() == DbType::Postgres {
        let (other_store, _, _) = another_organization(&th).await;
        let mut other_admin = fvoci_model::User {
            id: fvoci_model::new_id(),
            username: "other_org_admin".to_owned(),
            email: "other_org_admin@sample.com".to_owned(),
            password_hash: fvoci_server::auth::password::hash_password(PASSWORD).expect("hash"),
            ..fvoci_model::User::default()
        };
        other_store
            .create_user(&mut other_admin)
            .await
            .expect("other user");
        let other_client = th.login("other_org_admin", PASSWORD).await;
        assert_sees_nothing_of_victim(&other_client, "other org admin", &victim_ids).await;
        other_client
            .post(
                "/users/me/time/start",
                &json!({"blockId": s.card_id, "kind": "work"}),
            )
            .await
            .check_not_found();
    }

    let victim_active: ActiveTimeEntry = th.client2.get("/users/me/time/active").await.ok_json();
    assert_eq!(
        victim_active.entry.as_ref().map(|e| &e.id),
        Some(&s.active_id),
        "victim active timer intact"
    );
    let victim_list: TimeEntryList = th.client2.get("/users/me/time/entries").await.ok_json();
    assert_eq!(victim_list.summary.count, 2, "victim entry count intact");
    assert_eq!(victim_list.summary.total_ms, 30_000, "victim total intact");

    th.teardown().await;
}

/// Zero side-channel leakage: time entries never appear in exports, content search,
/// workspace/org audit logs, webhooks, or websocket broadcasts to other users.
#[tokio::test]
async fn time_entries_do_not_leak_via_export_search_audit_webhooks_or_ws() {
    static WEBHOOK_URL: OnceLock<String> = OnceLock::new();
    let webhook_hits = Arc::new(AtomicUsize::new(0));
    let hits_clone = Arc::clone(&webhook_hits);
    let router = Router::new().route(
        "/hook",
        post(move || {
            let hits = Arc::clone(&hits_clone);
            async move {
                hits.fetch_add(1, Ordering::SeqCst);
                StatusCode::OK
            }
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind hook");
    let _ = WEBHOOK_URL.set(format!(
        "http://{}/hook",
        listener.local_addr().expect("addr")
    ));
    let hook_server = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });

    let th = Setup::local_mode()
        .configure(|cfg| {
            cfg.block_change_webhooks = WEBHOOK_URL.get().into_iter().cloned().collect();
        })
        .start()
        .await
        .init_basic()
        .await;
    let org_id = org_of(&th).await;
    let board: Container = th
        .client
        .create_board(&new_board(
            TEST_WORKSPACE_ID,
            ContainerVisibility::Open,
            "Shared Board",
        ))
        .await
        .created_json();
    let user2_id = th.client2.me().await.id;
    give(&th, &board.id, &user2_id, "editor").await;
    let card_id = create_card(&th.client, TEST_WORKSPACE_ID, &board.id, "Tracked Card").await;

    tokio::time::sleep(Duration::from_millis(150)).await;
    let hooks_before = webhook_hits.load(Ordering::SeqCst);

    let mut ws = th.client2.connect_websocket().await;
    for msg in [
        json!({"type": "subscribeWorkspace", "workspaceId": TEST_WORKSPACE_ID}),
        json!({"type": "subscribeBlocks", "blockIds": [board.id, card_id]}),
    ] {
        ws.send(Message::Text(msg.to_string().into()))
            .await
            .expect("subscribe");
    }
    crate::common::carried_out(th.app.ws(), &mut ws).await;

    let secret_token = "xyzzy_unique_time_secret_98765";
    let manual_id = exercise_all_time_mutations(&th.client, &card_id, secret_token).await;

    let ws_event = tokio::time::timeout(Duration::from_millis(200), ws.next()).await;
    assert!(
        ws_event.is_err(),
        "time operations must emit zero WS broadcasts: {ws_event:?}"
    );
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(
        webhook_hits.load(Ordering::SeqCst),
        hooks_before,
        "time operations must fire zero webhooks"
    );
    assert_no_export_search_or_audit_leak(&th, &org_id, &board.id, secret_token, &manual_id).await;

    hook_server.abort();
    th.teardown().await;
}

async fn exercise_all_time_mutations(client: &Client, card_id: &str, secret_token: &str) -> String {
    let started: StartedTimeEntry = client
        .post(
            "/users/me/time/start",
            &json!({
                "workspaceId": TEST_WORKSPACE_ID,
                "blockId": card_id,
                "kind": "work",
                "note": secret_token,
            }),
        )
        .await
        .created_json();
    let manual: TimeEntry = client
        .post(
            "/users/me/time/entries",
            &json!({
                "workspaceId": TEST_WORKSPACE_ID,
                "blockId": card_id,
                "kind": "study",
                "startedAt": 1_000,
                "endedAt": 5_000,
                "note": secret_token,
            }),
        )
        .await
        .created_json();
    client
        .patch(
            &format!("/users/me/time/entries/{}", manual.id),
            &json!({"note": format!("{secret_token}_edited")}),
        )
        .await
        .check_ok();
    client.post_empty("/users/me/time/stop").await.check_ok();
    client
        .delete(&format!("/users/me/time/entries/{}", started.entry.id))
        .await
        .check_no_content();
    manual.id
}

async fn assert_no_export_search_or_audit_leak(
    th: &TestServer,
    org_id: &str,
    board_id: &str,
    secret_token: &str,
    manual_id: &str,
) {
    let search_hits: Value = th
        .client2
        .get(&format!("{TEST_WS}/search?q={secret_token}"))
        .await
        .ok_json();
    assert_eq!(search_hits, json!([]), "time notes must not be indexed");

    let exported = th
        .client2
        .get(&format!("{TEST_WS}/containers/{board_id}/archive/export"))
        .await;
    exported.check_ok();
    let mut archive = ZipArchive::new(Cursor::new(&exported.body)).expect("valid export zip");
    assert!(!archive.is_empty(), "export archive has entries");
    for idx in 0..archive.len() {
        let mut file = archive.by_index(idx).expect("zip entry");
        let mut contents = Vec::new();
        file.read_to_end(&mut contents).expect("read zip entry");
        let text = String::from_utf8_lossy(&contents);
        assert!(
            !text.contains(secret_token) && !text.contains(manual_id),
            "board export entry {:?} must never include time entries",
            file.name()
        );
    }

    for audit_path in [format!("{TEST_WS}/audit"), format!("/org/{org_id}/audit")] {
        let audit_text = th.client.get(&audit_path).await.text();
        assert!(
            !audit_text.contains(secret_token)
                && !audit_text.contains(manual_id)
                && !audit_text.contains("time_entr"),
            "{audit_path} must contain no time entry records"
        );
    }
}

/// Personal space password wall (`password_sign_in_required`) and zero-oracle private block
/// resolution behind an SSO wall when `workspaceId` is omitted.
#[tokio::test]
async fn own_space_password_wall_and_private_sso_block_no_oracle() {
    let fake = FakeProvider::start().await;
    let ts = server_for(&fake).await;
    let org_id = org_of(&ts).await;
    set_provider(&ts, &org_id, &fake).await;

    let private_board: Container = ts
        .client
        .create_board(&new_board(
            ROOT_WORKSPACE_ID,
            ContainerVisibility::Private,
            "Admin Only",
        ))
        .await
        .created_json();
    let secret_card = create_card(&ts.client, ROOT_WORKSPACE_ID, &private_board.id, "Hidden").await;
    let template_card: Value = ts
        .client
        .post(
            &format!("/w/{ROOT_WORKSPACE_ID}/boards/{}/cards", private_board.id),
            &json!({"title": "Hidden Template", "isTemplate": true}),
        )
        .await
        .created_json();
    let template_card_id = template_card["id"].as_str().expect("tmpl card").to_owned();

    let own = own_workspace(&ts.client).await;
    let own_entry: TimeEntry = ts
        .client
        .post(
            "/users/me/time/entries",
            &json!({
                "workspaceId": own.id,
                "kind": "study",
                "startedAt": 1_000,
                "endedAt": 6_000,
                "note": "personal only",
            }),
        )
        .await
        .created_json();

    tie(&ts, &ts.client, &fake, ADMIN_SUBJECT).await;
    let oidc_admin = through_provider(&ts, &fake, &org_id, ADMIN_SUBJECT).await;
    require_sso(&oidc_admin, &org_id, &fake, Passwords::Refused)
        .await
        .check_ok();

    let list_before: TimeEntryList = oidc_admin.get("/users/me/time/entries").await.ok_json();
    assert!(list_before.entries.is_empty(), "own space entry hidden");
    let refused = oidc_admin
        .post("/users/me/time/start", &json!({"kind": "study"}))
        .await;
    assert_eq!(refused.status, StatusCode::UNAUTHORIZED, "wall refuses");
    assert_eq!(
        refused.error_reason(),
        "password_sign_in_required",
        "personal space password wall reason"
    );

    let confirmed = oidc_admin
        .post("/users/me/password/confirm", &json!({"password": PASSWORD}))
        .await;
    confirmed.check_no_content();
    let confirmed_admin = ts.client_with_token(&confirmed.session_cookie().expect("new cookie"));
    let list_after: TimeEntryList = confirmed_admin
        .get("/users/me/time/entries")
        .await
        .ok_json();
    assert_eq!(list_after.entries.len(), 1, "own space entry visible");
    assert_eq!(list_after.entries[0].id, own_entry.id, "entry id matches");
    confirmed_admin
        .post("/users/me/time/start", &json!({"kind": "study"}))
        .await
        .check_created();

    assert_private_sso_block_no_oracle(
        &ts,
        &confirmed_admin,
        &private_board.id,
        &secret_card,
        &template_card_id,
    )
    .await;
    assert_closed_org_future_timer_does_not_leak_timestamp(&ts, &confirmed_admin, &secret_card)
        .await;

    ts.teardown().await;
    fake.stop();
}

async fn assert_private_sso_block_no_oracle(
    ts: &TestServer,
    oidc_admin: &Client,
    board_id: &str,
    secret_card: &str,
    template_card_id: &str,
) {
    let probe_secret = ts
        .client2
        .post(
            "/users/me/time/start",
            &json!({"blockId": secret_card, "kind": "work"}),
        )
        .await;
    let probe_missing = ts
        .client2
        .post(
            "/users/me/time/start",
            &json!({"blockId": fvoci_model::new_id(), "kind": "work"}),
        )
        .await;
    assert_eq!(
        probe_secret.status,
        StatusCode::NOT_FOUND,
        "unreadable private block behind SSO wall returns 404"
    );
    assert_eq!(
        probe_missing.status,
        StatusCode::NOT_FOUND,
        "missing block returns 404"
    );
    assert_eq!(
        probe_secret.error_message(),
        probe_missing.error_message(),
        "404 message matches missing block exactly"
    );

    let user2_id = ts.client2.me().await.id;
    add_container_role_in(
        oidc_admin,
        ROOT_WORKSPACE_ID,
        board_id,
        &user2_id,
        MemberRole::Viewer,
    )
    .await;
    // Even with read standing on the board, a template card must still answer 404 (not 401).
    let probe_template = ts
        .client2
        .post(
            "/users/me/time/start",
            &json!({"blockId": template_card_id, "kind": "work"}),
        )
        .await;
    assert_eq!(
        probe_template.status,
        StatusCode::NOT_FOUND,
        "template card behind SSO wall returns 404, not 401: {}",
        probe_template.text()
    );
    let probe_readable = ts
        .client2
        .post(
            "/users/me/time/start",
            &json!({"blockId": secret_card, "kind": "work"}),
        )
        .await;
    assert_eq!(
        probe_readable.status,
        StatusCode::UNAUTHORIZED,
        "readable block behind SSO wall returns 401"
    );
    assert_eq!(
        probe_readable.error_reason(),
        "sso_required",
        "SSO wall reason returned for readable block"
    );
}

async fn assert_closed_org_future_timer_does_not_leak_timestamp(
    ts: &TestServer,
    oidc_admin: &Client,
    card_id: &str,
) {
    let future_start = 4_000_000_000_000_i64;
    let sso_started: StartedTimeEntry = oidc_admin
        .post(
            "/users/me/time/start",
            &json!({"workspaceId": ROOT_WORKSPACE_ID, "blockId": card_id, "kind": "work"}),
        )
        .await
        .created_json();
    oidc_admin
        .patch(
            &format!("/users/me/time/entries/{}", sso_started.entry.id),
            &json!({"startedAt": future_start}),
        )
        .await
        .check_ok();

    // Password session starts a personal timer: must not leak `future_start` into startedAt!
    let own_started: StartedTimeEntry = ts
        .client
        .post("/users/me/time/start", &json!({"kind": "study"}))
        .await
        .created_json();
    assert_eq!(
        own_started.stopped, None,
        "closed org timer omitted from stopped"
    );
    assert!(
        own_started.entry.started_at < future_start,
        "closed org future startedAt must not leak into open org start: {}",
        own_started.entry.started_at
    );
}

/// Leaving an organization via HTTP (`POST /org/{orgId}/leave`) deletes all entries in
/// that org (including any running timer) without touching personal space entries; account
/// erasure wipes all remaining entries.
#[tokio::test]
async fn leaving_organization_and_account_erasure_remove_time_entries() {
    let th = Setup::local_mode().start().await.init_basic().await;
    let org_id = org_of(&th).await;
    let leaver = &th.client2;
    let leaver_id = leaver.me().await.id;

    let own = own_workspace(leaver).await;
    let own_entry: TimeEntry = leaver
        .post(
            "/users/me/time/entries",
            &json!({
                "workspaceId": own.id,
                "kind": "study",
                "startedAt": 1_000,
                "endedAt": 5_000,
                "note": "stay",
            }),
        )
        .await
        .created_json();

    leaver
        .post(
            "/users/me/time/start",
            &json!({
                "workspaceId": TEST_WORKSPACE_ID,
                "kind": "work",
                "note": "shared org running",
            }),
        )
        .await
        .check_created();

    leaver
        .post_empty(&format!("/org/{org_id}/leave"))
        .await
        .check_no_content();

    let active: ActiveTimeEntry = leaver.get("/users/me/time/active").await.ok_json();
    assert_eq!(active.entry, None, "active timer cleared when leaving org");
    let listed: TimeEntryList = leaver.get("/users/me/time/entries").await.ok_json();
    assert_eq!(
        listed.entries.len(),
        1,
        "personal entry preserved after leaving org"
    );
    assert_eq!(listed.entries[0].id, own_entry.id, "preserved entry id");

    let fresh: StartedTimeEntry = leaver
        .post("/users/me/time/start", &json!({"kind": "other"}))
        .await
        .created_json();
    assert_eq!(fresh.stopped, None, "no orphaned timer stopped");

    th.app
        .store()
        .execute_unchecked(&format!(
            "UPDATE users SET deleted_at = 1, delete_after = 1 WHERE id = '{leaver_id}'"
        ))
        .await
        .expect("mark for erasure");
    th.app.erase_due_accounts().await;

    let remaining = th
        .app
        .store()
        .system()
        .texts_unchecked(&format!(
            "SELECT CAST(id AS TEXT) FROM time_entries WHERE user_id = '{leaver_id}'"
        ))
        .await
        .expect("check db");
    assert!(
        remaining.is_empty(),
        "account erasure must remove all time entries: {remaining:?}"
    );

    th.teardown().await;
}

/// A workspace guest can track time on an invited card, is refused 404 on workspace-only
/// targets, and removing them from their last workspace in the org deletes all their
/// org time entries (including any active timer) while preserving personal space entries.
#[tokio::test]
async fn guest_tracking_and_removal_cleans_up_org_time_entries() {
    let th = Setup::local_mode().start().await.init_basic().await;
    let board: Container = th
        .client
        .create_board(&new_board(
            TEST_WORKSPACE_ID,
            ContainerVisibility::Private,
            "Guest Board",
        ))
        .await
        .created_json();
    let card_id = create_card(&th.client, TEST_WORKSPACE_ID, &board.id, "Guest Card").await;

    let guest = th.add_user("guest_timer").await;
    let guest_id = guest.me().await.id;
    set_role(&th, &guest_id, "guest").await;
    // A real guest is only in the invited workspace, not in ROOT_WORKSPACE_ID or organization_members.
    th.app
        .store()
        .remove_workspace_member(ROOT_WORKSPACE_ID, &guest_id, "")
        .await
        .expect("remove guest from root ws");
    th.app
        .store()
        .execute_unchecked(&format!(
            "DELETE FROM organization_members WHERE user_id = '{guest_id}'"
        ))
        .await
        .expect("guest has no org membership");
    add_container_role(&th.client, &board.id, &guest_id, MemberRole::Editor).await;

    // Guest cannot log a workspace-only entry (404).
    guest
        .post(
            "/users/me/time/start",
            &json!({"workspaceId": TEST_WORKSPACE_ID, "kind": "work"}),
        )
        .await
        .check_not_found();

    // Personal space completed entry stays after workspace removal.
    let own = own_workspace(&guest).await;
    let own_entry: TimeEntry = guest
        .post(
            "/users/me/time/entries",
            &json!({
                "workspaceId": own.id,
                "kind": "study",
                "startedAt": 1_000,
                "endedAt": 4_000,
                "note": "guest personal",
            }),
        )
        .await
        .created_json();

    // Guest tracks time on the invited card (active timer).
    guest
        .post(
            "/users/me/time/start",
            &json!({"blockId": card_id, "kind": "work", "note": "guest work"}),
        )
        .await
        .check_created();

    // Remove the guest from their only workspace in the org.
    let ws_admin_id = th.client.me().await.id;
    set_role(&th, &ws_admin_id, "admin").await;
    th.client
        .delete(&format!("{TEST_WS}/members/{guest_id}"))
        .await
        .check_no_content();

    let active: ActiveTimeEntry = guest.get("/users/me/time/active").await.ok_json();
    assert_eq!(
        active.entry, None,
        "removing guest cleared their active timer in that org"
    );
    let listed: TimeEntryList = guest.get("/users/me/time/entries").await.ok_json();
    assert_eq!(
        listed.entries.len(),
        1,
        "only guest personal entry remains after removal"
    );
    assert_eq!(listed.entries[0].id, own_entry.id, "personal entry intact");

    // Guest can immediately start a new personal timer without any orphan blocking it.
    let fresh: StartedTimeEntry = guest
        .post("/users/me/time/start", &json!({"kind": "other"}))
        .await
        .created_json();
    assert_eq!(fresh.stopped, None, "no orphaned guest timer");

    th.teardown().await;
}
