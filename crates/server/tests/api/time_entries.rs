//! HTTP API tests for personal time tracking (`/users/me/time/*`): active timer
//! lifecycle, atomic cross-org switching, card/unit/targetless linking, target validation,
//! survival across target deletion, filtering, pagination, summaries, and manual edits.

use fvoci_model::{
    ActiveTimeEntry, Container, ContainerVisibility, StartedTimeEntry, TIME_ENTRY_NOTE_MAX_CHARS,
    TimeEntry, TimeEntryKind, TimeEntryList, TimeTargetKind, Unit, new_id,
};
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::boards::new_board;
use crate::common::{Client, Setup, TEST_WORKSPACE_ID, TEST_WS};
use crate::own_space::own_workspace;

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

async fn create_course_with_chapter_and_unit(
    client: &Client,
    ws_id: &str,
    course_type: &str,
    title: &str,
) -> (String, Unit, Unit) {
    let course: Value = client
        .post(
            &format!("/w/{ws_id}/courses"),
            &json!({"visibility": course_type, "title": title}),
        )
        .await
        .created_json();
    let course_id = course["id"].as_str().expect("course id").to_owned();
    let chapter: Unit = client
        .post(
            &format!("/w/{ws_id}/courses/{course_id}/units"),
            &json!({"kind": "chapter", "title": "Chapter 1"}),
        )
        .await
        .created_json();
    let unit: Unit = client
        .post(
            &format!("/w/{ws_id}/courses/{course_id}/units"),
            &json!({"kind": "unit", "parentId": chapter.id, "title": "Lesson 1"}),
        )
        .await
        .created_json();
    (course_id, chapter, unit)
}

/// Full timer lifecycle: start on a board card, atomically switch to a course unit,
/// switch across organizations (shared org <-> personal space), stop, and verify 404 when
/// no timer is running.
#[tokio::test]
async fn timer_lifecycle_and_cross_org_atomic_switch() {
    let th = Setup::local_mode().start().await.init_basic().await;
    let client = &th.client;

    let active: ActiveTimeEntry = client.get("/users/me/time/active").await.ok_json();
    assert_eq!(active.entry, None, "no active timer initially");
    client
        .post_empty("/users/me/time/stop")
        .await
        .check_not_found();

    let board: Container = client
        .create_board(&new_board(
            TEST_WORKSPACE_ID,
            ContainerVisibility::Private,
            "Shared Board",
        ))
        .await
        .created_json();
    let card_id = create_card(client, TEST_WORKSPACE_ID, &board.id, "Implement T2").await;
    let (course_id, _, unit) =
        create_course_with_chapter_and_unit(client, TEST_WORKSPACE_ID, "private", "Rust Course")
            .await;

    let own = own_workspace(client).await;
    let own_board: Container = client
        .create_board(&new_board(
            &own.id,
            ContainerVisibility::Private,
            "Personal",
        ))
        .await
        .created_json();
    let own_card_id = create_card(client, &own.id, &own_board.id, "Own Task").await;

    let started1 = start_on_own_card(client, &own.id, &own_board.id, &own_card_id).await;
    let started2 = switch_to_shared_card(client, &started1.entry.id, &board.id, &card_id).await;
    switch_to_unit_targetless_and_stop(client, &started2.entry.id, &course_id, &unit.id).await;

    th.teardown().await;
}

async fn start_on_own_card(
    client: &Client,
    own_ws_id: &str,
    own_board_id: &str,
    own_card_id: &str,
) -> StartedTimeEntry {
    let started: StartedTimeEntry = client
        .post(
            "/users/me/time/start",
            &json!({
                "workspaceId": own_ws_id,
                "containerId": own_board_id,
                "blockId": own_card_id,
                "kind": "study",
                "note": "personal study",
            }),
        )
        .await
        .created_json();
    assert_eq!(started.stopped, None, "no timer stopped on first start");
    assert_eq!(started.entry.ended_at, None, "first timer is active");
    assert_eq!(
        started.entry.target_kind,
        Some(TimeTargetKind::Card),
        "target kind is card"
    );
    assert_eq!(
        started.entry.target_title.as_deref(),
        Some("Own Task"),
        "target title snapshot captured"
    );
    assert_eq!(
        started.entry.block_id.as_deref(),
        Some(own_card_id),
        "linked block id matches"
    );
    let active: ActiveTimeEntry = client.get("/users/me/time/active").await.ok_json();
    assert_eq!(
        active.entry.as_ref().map(|e| &e.id),
        Some(&started.entry.id),
        "active endpoint returns running entry"
    );
    started
}

async fn switch_to_shared_card(
    client: &Client,
    previous_id: &str,
    board_id: &str,
    card_id: &str,
) -> StartedTimeEntry {
    let started: StartedTimeEntry = client
        .post(
            "/users/me/time/start",
            &json!({
                "blockId": card_id,
                "kind": "work",
                "note": "shared card",
            }),
        )
        .await
        .created_json();
    let stopped = started.stopped.as_ref().expect("personal timer stopped");
    assert_eq!(stopped.id, previous_id, "first timer was stopped");
    assert!(
        stopped.ended_at.is_some_and(|e| e >= stopped.started_at),
        "stopped timer has valid endedAt"
    );
    assert_eq!(
        started.entry.workspace_id.as_deref(),
        Some(TEST_WORKSPACE_ID),
        "resolved workspace id"
    );
    assert_eq!(
        started.entry.container_id.as_deref(),
        Some(board_id),
        "resolved container id"
    );
    assert_eq!(
        started.entry.block_id.as_deref(),
        Some(card_id),
        "linked shared card id"
    );
    assert_eq!(
        started.entry.target_kind,
        Some(TimeTargetKind::Card),
        "shared target kind is card"
    );
    assert_eq!(
        started.entry.target_title.as_deref(),
        Some("Implement T2"),
        "shared target title captured"
    );
    started
}

async fn switch_to_unit_targetless_and_stop(
    client: &Client,
    previous_id: &str,
    course_id: &str,
    unit_id: &str,
) {
    let started3: StartedTimeEntry = client
        .post(
            "/users/me/time/start",
            &json!({
                "workspaceId": TEST_WORKSPACE_ID,
                "containerId": course_id,
                "blockId": unit_id,
                "kind": "lecture",
            }),
        )
        .await
        .created_json();
    assert_eq!(
        started3.stopped.as_ref().map(|e| e.id.as_str()),
        Some(previous_id),
        "card timer was stopped"
    );
    assert_eq!(
        started3.entry.target_kind,
        Some(TimeTargetKind::Unit),
        "course unit target kind"
    );
    assert_eq!(
        started3.entry.target_title.as_deref(),
        Some("Lesson 1"),
        "course unit title snapshot"
    );

    let started4: StartedTimeEntry = client
        .post(
            "/users/me/time/start",
            &json!({"kind": "other", "note": "free focus"}),
        )
        .await
        .created_json();
    assert_eq!(
        started4.stopped.as_ref().map(|e| &e.id),
        Some(&started3.entry.id),
        "unit timer stopped when switching to targetless"
    );
    assert_eq!(started4.entry.workspace_id, None, "no workspaceId");
    assert_eq!(started4.entry.container_id, None, "no containerId");
    assert_eq!(started4.entry.block_id, None, "no blockId");
    assert_eq!(started4.entry.target_kind, None, "no targetKind");
    assert_eq!(started4.entry.target_title, None, "no targetTitle");

    let stopped4: TimeEntry = client.post_empty("/users/me/time/stop").await.ok_json();
    assert_eq!(stopped4.id, started4.entry.id, "stopped active timer");
    assert!(
        stopped4.ended_at.is_some_and(|e| e >= stopped4.started_at),
        "stopped active timer has valid endedAt"
    );
    let active: ActiveTimeEntry = client.get("/users/me/time/active").await.ok_json();
    assert_eq!(active.entry, None, "no active timer after stop");
    client
        .post_empty("/users/me/time/stop")
        .await
        .check_not_found();
}

/// Invalid targets (chapter, comment, view, template card, template board, wiki page,
/// diary entry, mismatched containerId, nonexistent ID) all return 404.
#[tokio::test]
async fn invalid_target_types_are_rejected_with_not_found() {
    let th = Setup::local_mode().start().await.init_basic().await;
    let rejected_targets = build_invalid_target_payloads(&th.client).await;

    for (label, body) in rejected_targets {
        let resp = th.client.post("/users/me/time/start", &body).await;
        assert_eq!(
            resp.status,
            StatusCode::NOT_FOUND,
            "start {label}: {}",
            resp.text()
        );
        let mut manual = body.clone();
        manual["startedAt"] = json!(1_000);
        manual["endedAt"] = json!(2_000);
        let resp = th.client.post("/users/me/time/entries", &manual).await;
        assert_eq!(
            resp.status,
            StatusCode::NOT_FOUND,
            "manual {label}: {}",
            resp.text()
        );
    }

    th.teardown().await;
}

async fn seed_invalid_board_blocks(
    client: &Client,
) -> (String, String, String, String, String, String) {
    let board: Container = client
        .create_board(&new_board(
            TEST_WORKSPACE_ID,
            ContainerVisibility::Private,
            "Board",
        ))
        .await
        .created_json();
    let card_id = create_card(client, TEST_WORKSPACE_ID, &board.id, "Real Card").await;

    let template_card: Value = client
        .post(
            &format!("{TEST_WS}/boards/{}/cards", board.id),
            &json!({"title": "Card Template", "isTemplate": true}),
        )
        .await
        .created_json();
    let comment: Value = client
        .post(
            &format!("{TEST_WS}/boards/{}/blocks", board.id),
            &json!([{
                "containerId": board.id,
                "parentId": card_id,
                "type": "comment",
                "title": "a comment",
                "createdAt": 1,
                "updatedAt": 1,
            }]),
        )
        .await
        .created_json();
    let view: Value = client
        .post(
            &format!("{TEST_WS}/boards/{}/blocks", board.id),
            &json!([{
                "containerId": board.id,
                "type": "view",
                "title": "Board view",
                "fields": {"viewType": "board"},
                "createdAt": 1,
                "updatedAt": 1,
            }]),
        )
        .await
        .created_json();

    let mut tmpl_board = new_board(
        TEST_WORKSPACE_ID,
        ContainerVisibility::Private,
        "Board Template",
    );
    tmpl_board.is_template = true;
    let tmpl_board: Container = client.create_board(&tmpl_board).await.created_json();
    let tmpl_board_card_id =
        create_card(client, TEST_WORKSPACE_ID, &tmpl_board.id, "Inside Template").await;

    (
        card_id,
        template_card["id"].as_str().expect("tmpl card").to_owned(),
        comment[0]["id"].as_str().expect("comment").to_owned(),
        view[0]["id"].as_str().expect("view").to_owned(),
        tmpl_board.id,
        tmpl_board_card_id,
    )
}

async fn build_invalid_target_payloads(client: &Client) -> Vec<(&'static str, Value)> {
    let (card_id, template_card_id, comment_id, view_id, tmpl_board_id, tmpl_board_card_id) =
        seed_invalid_board_blocks(client).await;
    let (course_id, chapter, _unit) =
        create_course_with_chapter_and_unit(client, TEST_WORKSPACE_ID, "private", "Course").await;

    let wiki: Value = client
        .post(
            &format!("{TEST_WS}/wikis"),
            &json!({"visibility": "private", "title": "Wiki"}),
        )
        .await
        .created_json();
    let wiki_id = wiki["id"].as_str().expect("wiki id").to_owned();
    let page: Value = client
        .post(
            &format!("{TEST_WS}/wikis/{wiki_id}/pages"),
            &json!({"title": "Wiki Page"}),
        )
        .await
        .created_json();
    let page_id = page["id"].as_str().expect("page id").to_owned();

    let own = own_workspace(client).await;
    let diary: Value = client
        .post(
            &format!("/w/{}/diaries", own.id),
            &json!({"title": "Diary"}),
        )
        .await
        .created_json();
    let diary_id = diary["id"].as_str().expect("diary id").to_owned();
    let diary_entry: Value = client
        .post(
            &format!("/w/{}/diaries/{diary_id}/entries", own.id),
            &json!({"date": "2026-11-23"}),
        )
        .await
        .created_json();
    let diary_entry_id = diary_entry["id"].as_str().expect("diary entry").to_owned();

    vec![
        (
            "template card",
            json!({"workspaceId": TEST_WORKSPACE_ID, "blockId": template_card_id, "kind": "work"}),
        ),
        (
            "comment",
            json!({"workspaceId": TEST_WORKSPACE_ID, "blockId": comment_id, "kind": "work"}),
        ),
        (
            "view",
            json!({"workspaceId": TEST_WORKSPACE_ID, "blockId": view_id, "kind": "work"}),
        ),
        (
            "card in template board",
            json!({"workspaceId": TEST_WORKSPACE_ID, "blockId": tmpl_board_card_id, "kind": "work"}),
        ),
        (
            "template board container",
            json!({"workspaceId": TEST_WORKSPACE_ID, "containerId": tmpl_board_id, "kind": "work"}),
        ),
        (
            "course chapter",
            json!({"workspaceId": TEST_WORKSPACE_ID, "containerId": course_id, "blockId": chapter.id, "kind": "study"}),
        ),
        (
            "wiki page",
            json!({"workspaceId": TEST_WORKSPACE_ID, "blockId": page_id, "kind": "study"}),
        ),
        (
            "wiki container",
            json!({"workspaceId": TEST_WORKSPACE_ID, "containerId": wiki_id, "kind": "study"}),
        ),
        (
            "diary entry",
            json!({"workspaceId": own.id, "blockId": diary_entry_id, "kind": "other"}),
        ),
        (
            "mismatched containerId",
            json!({"workspaceId": TEST_WORKSPACE_ID, "containerId": course_id, "blockId": card_id, "kind": "work"}),
        ),
        (
            "nonexistent block",
            json!({"workspaceId": TEST_WORKSPACE_ID, "blockId": new_id(), "kind": "work"}),
        ),
        (
            "nonexistent workspace",
            json!({"workspaceId": new_id(), "blockId": card_id, "kind": "work"}),
        ),
    ]
}

/// Time entries survive when their target card or course unit is moved to the trash or
/// permanently purged (no `ON DELETE CASCADE`).
#[tokio::test]
async fn time_entries_survive_target_trash_and_permanent_purge() {
    let th = Setup::local_mode().start().await.init_basic().await;
    let client = &th.client;

    let board: Container = client
        .create_board(&new_board(
            TEST_WORKSPACE_ID,
            ContainerVisibility::Private,
            "Board",
        ))
        .await
        .created_json();
    let card_id = create_card(client, TEST_WORKSPACE_ID, &board.id, "Transient Card").await;
    let (course_id, _, unit) =
        create_course_with_chapter_and_unit(client, TEST_WORKSPACE_ID, "private", "Course").await;

    let card_entry: TimeEntry = client
        .post(
            "/users/me/time/entries",
            &json!({
                "workspaceId": TEST_WORKSPACE_ID,
                "blockId": card_id,
                "kind": "work",
                "startedAt": 10_000,
                "endedAt": 20_000,
                "note": "card work",
            }),
        )
        .await
        .created_json();
    let unit_entry: TimeEntry = client
        .post(
            "/users/me/time/entries",
            &json!({
                "workspaceId": TEST_WORKSPACE_ID,
                "blockId": unit.id,
                "kind": "lecture",
                "startedAt": 30_000,
                "endedAt": 55_000,
                "note": "unit lecture",
            }),
        )
        .await
        .created_json();

    client
        .delete(&format!("{TEST_WS}/boards/{}/blocks/{card_id}", board.id))
        .await
        .check_no_content();
    client
        .delete(&format!("{TEST_WS}/courses/{course_id}/units/{}", unit.id))
        .await
        .check_no_content();
    th.app
        .store()
        .system()
        .purge_trash(i64::MAX)
        .await
        .expect("purge trash");

    let listed: TimeEntryList = client.get("/users/me/time/entries").await.ok_json();
    assert_eq!(listed.entries.len(), 2, "entries survive target purge");
    assert_eq!(listed.summary.total_ms, 35_000, "summary total intact");
    assert_eq!(listed.summary.by_kind.work, 10_000, "work total intact");
    assert_eq!(
        listed.summary.by_kind.lecture, 25_000,
        "lecture total intact"
    );
    assert_eq!(listed.entries[0].id, unit_entry.id, "newest entry first");
    assert_eq!(
        listed.entries[0].target_title.as_deref(),
        Some("Lesson 1"),
        "unit title snapshot preserved"
    );
    assert_eq!(listed.entries[1].id, card_entry.id, "older entry second");
    assert_eq!(
        listed.entries[1].target_title.as_deref(),
        Some("Transient Card"),
        "card title snapshot preserved"
    );

    let edited: TimeEntry = client
        .patch(
            &format!("/users/me/time/entries/{}", card_entry.id),
            &json!({"note": "still editable"}),
        )
        .await
        .ok_json();
    assert_eq!(edited.note, "still editable", "survived entry editable");
    client
        .delete(&format!("/users/me/time/entries/{}", unit_entry.id))
        .await
        .check_no_content();

    th.teardown().await;
}

/// Manual logging, filtering (`from`, `to`, `kind`, `blockId`, `containerId`), pagination
/// with full-filter summaries, unlimited duration, validation, and PATCH/DELETE semantics.
#[tokio::test]
async fn manual_entries_filters_pagination_validation_and_patch() {
    let th = Setup::local_mode().start().await.init_basic().await;
    let client = &th.client;

    let board: Container = client
        .create_board(&new_board(
            TEST_WORKSPACE_ID,
            ContainerVisibility::Private,
            "Board",
        ))
        .await
        .created_json();
    let card_id = create_card(client, TEST_WORKSPACE_ID, &board.id, "Task A").await;

    let active: StartedTimeEntry = client
        .post("/users/me/time/start", &json!({"kind": "other"}))
        .await
        .created_json();

    let hundred_hours = 100 * 60 * 60 * 1_000_i64;
    let long_entry: TimeEntry = client
        .post(
            "/users/me/time/entries",
            &json!({
                "workspaceId": TEST_WORKSPACE_ID,
                "blockId": card_id,
                "kind": "work",
                "startedAt": 1_000,
                "endedAt": 1_000 + hundred_hours,
                "note": "marathon",
            }),
        )
        .await
        .created_json();
    let study_entry: TimeEntry = client
        .post(
            "/users/me/time/entries",
            &json!({"kind": "study", "startedAt": 5_000, "endedAt": 15_000}),
        )
        .await
        .created_json();

    let still_active: ActiveTimeEntry = client.get("/users/me/time/active").await.ok_json();
    assert_eq!(
        still_active.entry.as_ref().map(|e| &e.id),
        Some(&active.entry.id),
        "manual logging does not stop active timer"
    );

    assert_filtered_summaries(
        client,
        &board.id,
        &card_id,
        &long_entry.id,
        &study_entry.id,
        hundred_hours,
    )
    .await;
    assert_patch_and_delete(client, &active.entry, &study_entry.id).await;
    th.teardown().await;
}

async fn assert_filtered_summaries(
    client: &Client,
    board_id: &str,
    card_id: &str,
    long_id: &str,
    study_id: &str,
    hundred_hours: i64,
) {
    let page0: TimeEntryList = client
        .get("/users/me/time/entries?from=1000&to=10000&perPage=1&page=0")
        .await
        .ok_json();
    assert_eq!(page0.entries.len(), 1, "first page has one item");
    assert_eq!(page0.entries[0].id, study_id, "newest item on page 0");
    assert_eq!(page0.summary.count, 2, "summary counts all matching rows");
    assert_eq!(
        page0.summary.total_ms,
        hundred_hours + 10_000,
        "summary sums all matching rows"
    );
    assert_eq!(page0.summary.by_kind.work, hundred_hours, "work total");
    assert_eq!(page0.summary.by_kind.study, 10_000, "study total");

    let by_block: TimeEntryList = client
        .get(&format!(
            "/users/me/time/entries?blockId={card_id}&kind=work"
        ))
        .await
        .ok_json();
    assert_eq!(by_block.entries.len(), 1, "block+kind filter matches one");
    assert_eq!(by_block.entries[0].id, long_id, "matched card entry");
    assert_eq!(by_block.summary.total_ms, hundred_hours, "block total");

    let by_container: TimeEntryList = client
        .get(&format!("/users/me/time/entries?containerId={board_id}"))
        .await
        .ok_json();
    assert_eq!(
        by_container.entries.len(),
        1,
        "containerId filter matches one"
    );
    assert_eq!(by_container.entries[0].id, long_id, "matched board entry");
}

async fn assert_patch_and_delete(client: &Client, active: &TimeEntry, study_id: &str) {
    let stop_at = active.started_at + 4_000;
    let patched: TimeEntry = client
        .patch(
            &format!("/users/me/time/entries/{}", active.id),
            &json!({"endedAt": stop_at, "kind": "lecture", "note": "converted"}),
        )
        .await
        .ok_json();
    assert_eq!(patched.ended_at, Some(stop_at), "patch stopped timer");
    assert_eq!(patched.kind, TimeEntryKind::Lecture, "patch updated kind");
    let now_none: ActiveTimeEntry = client.get("/users/me/time/active").await.ok_json();
    assert_eq!(now_none.entry, None, "no active timer after patch stop");

    assert_bad_requests(client, study_id).await;

    client
        .delete(&format!("/users/me/time/entries/{study_id}"))
        .await
        .check_no_content();
    client
        .delete(&format!("/users/me/time/entries/{study_id}"))
        .await
        .check_not_found();
}

async fn assert_bad_requests(client: &Client, entry_id: &str) {
    let too_long_note = "a".repeat(TIME_ENTRY_NOTE_MAX_CHARS + 1);
    for (label, resp) in [
        (
            "endedAt < startedAt on create",
            client
                .post(
                    "/users/me/time/entries",
                    &json!({"kind": "study", "startedAt": 500, "endedAt": 400}),
                )
                .await,
        ),
        (
            "note too long on start",
            client
                .post(
                    "/users/me/time/start",
                    &json!({"kind": "study", "note": too_long_note}),
                )
                .await,
        ),
        (
            "invalid kind query",
            client.get("/users/me/time/entries?kind=sleeping").await,
        ),
        (
            "to < from query",
            client.get("/users/me/time/entries?from=2000&to=1000").await,
        ),
        (
            "patch endedAt < existing startedAt",
            client
                .patch(
                    &format!("/users/me/time/entries/{entry_id}"),
                    &json!({"endedAt": 4_999}),
                )
                .await,
        ),
        (
            "patch explicit null endedAt",
            client
                .patch(
                    &format!("/users/me/time/entries/{entry_id}"),
                    &json!({"endedAt": null}),
                )
                .await,
        ),
    ] {
        assert_eq!(
            resp.status,
            StatusCode::BAD_REQUEST,
            "{label}: {}",
            resp.text()
        );
    }
}

/// Starting a timer on a unit of an open course works without explicitly joining the course.
#[tokio::test]
async fn workspace_member_can_track_time_on_open_course_unit() {
    let th = Setup::local_mode().start().await.init_basic().await;
    let (_course_id, _, unit) =
        create_course_with_chapter_and_unit(&th.client, TEST_WORKSPACE_ID, "open", "Open Course")
            .await;

    let started: StartedTimeEntry = th
        .client2
        .post(
            "/users/me/time/start",
            &json!({"blockId": unit.id, "kind": "lecture"}),
        )
        .await
        .created_json();
    assert_eq!(
        started.entry.block_id.as_deref(),
        Some(unit.id.as_str()),
        "open course unit linked"
    );
    assert_eq!(
        started.entry.target_kind,
        Some(TimeTargetKind::Unit),
        "open course unit target kind"
    );

    th.teardown().await;
}

/// Container-only (`board`/`course`), workspace-only targets, no-op `PATCH {}`,
/// moving `startedAt` past `endedAt` (verifying `/startedAt` pointer), and `from`/`to`
/// boundary validation.
#[tokio::test]
async fn container_workspace_targets_and_patch_boundary_cases() {
    let th = Setup::local_mode().start().await.init_basic().await;
    let client = &th.client;

    let board: Container = client
        .create_board(&new_board(
            TEST_WORKSPACE_ID,
            ContainerVisibility::Private,
            "Board Target",
        ))
        .await
        .created_json();
    let (course_id, _, _) =
        create_course_with_chapter_and_unit(client, TEST_WORKSPACE_ID, "private", "Course Target")
            .await;

    // Board-only containerId (omitting workspaceId).
    let on_board: TimeEntry = client
        .post(
            "/users/me/time/entries",
            &json!({"containerId": board.id, "kind": "work", "startedAt": 100, "endedAt": 300}),
        )
        .await
        .created_json();
    assert_eq!(
        on_board.target_kind,
        Some(TimeTargetKind::Board),
        "board-only targetKind"
    );
    assert_eq!(
        on_board.target_title.as_deref(),
        Some("Board Target"),
        "board-only title snapshot"
    );
    assert_eq!(on_board.block_id, None, "board-only has no blockId");

    // Course-only containerId + workspace-only target.
    let on_course: TimeEntry = client
        .post(
            "/users/me/time/entries",
            &json!({"containerId": course_id, "kind": "lecture", "startedAt": 300, "endedAt": 500}),
        )
        .await
        .created_json();
    assert_eq!(
        on_course.target_kind,
        Some(TimeTargetKind::Course),
        "course-only targetKind"
    );
    let on_ws: TimeEntry = client
        .post(
            "/users/me/time/entries",
            &json!({"workspaceId": TEST_WORKSPACE_ID, "kind": "other", "startedAt": 500, "endedAt": 500}),
        )
        .await
        .created_json();
    assert_eq!(
        on_ws.workspace_id.as_deref(),
        Some(TEST_WORKSPACE_ID),
        "workspace-only stores workspaceId"
    );
    assert_eq!(on_ws.target_kind, None, "workspace-only has no targetKind");

    // Empty PATCH {} succeeds as a no-op; moving startedAt > endedAt reports /startedAt.
    assert_patch_and_query_boundaries(client, &on_board.id, &on_course.id).await;
    th.teardown().await;
}

async fn assert_patch_and_query_boundaries(
    client: &Client,
    board_entry_id: &str,
    course_entry_id: &str,
) {
    let noop: TimeEntry = client
        .patch(
            &format!("/users/me/time/entries/{board_entry_id}"),
            &json!({}),
        )
        .await
        .ok_json();
    assert_eq!(noop.started_at, 100, "empty patch leaves startedAt intact");
    let bad_start = client
        .patch(
            &format!("/users/me/time/entries/{board_entry_id}"),
            &json!({"startedAt": 301}),
        )
        .await;
    assert_eq!(
        bad_start.status,
        StatusCode::BAD_REQUEST,
        "startedAt > endedAt"
    );
    assert_eq!(
        bad_start.value()["errors"][0]["pointer"],
        "/startedAt",
        "pointer names /startedAt: {}",
        bad_start.text()
    );

    let exact: TimeEntryList = client
        .get("/users/me/time/entries?from=300&to=300")
        .await
        .ok_json();
    assert_eq!(exact.entries.len(), 1, "from == to matches inclusive point");
    assert_eq!(exact.entries[0].id, course_entry_id, "matched entry at 300");
    for bad_query in [
        "/users/me/time/entries?from=-1",
        "/users/me/time/entries?to=99999999999999999999",
    ] {
        let resp = client.get(bad_query).await;
        assert_eq!(
            resp.status,
            StatusCode::BAD_REQUEST,
            "{bad_query} rejected: {}",
            resp.text()
        );
    }
}
