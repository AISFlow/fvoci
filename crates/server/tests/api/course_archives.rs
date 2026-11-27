//! Course archive export and import round-trip (T12 streaming path): preserves chapters,
//! units, hierarchy, bodies, attachments, internal `[[` unit references, unit amounts,
//! shared `scheduledDate`, and shared `course_schedules`; strictly excludes all personal
//! `unit_progress` (`completedAt`, `lastPosition`, personal `plannedDate`); rejects
//! malicious course archives with 400 and full transactional rollback.

use std::io::{Cursor, Read as _, Write as _};

use fvoci_model::{
    AmountUnit, Container, ContainerKind, CourseScheduleView, UnitNode, UnitProgress,
};
use serde_json::{Value, json};

use crate::boards::another_workspace;
use crate::common::{Client, Setup, TEST_WORKSPACE_ID, TEST_WS};
use crate::export::import;
use crate::files::{upload, uploaded_id};
use crate::references::made;
use crate::references_shared::references_in;
use crate::references_to::body_naming;

fn archive_of(boards: &[&[Value]]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default();
    writer.start_file("version.json", options).expect("start");
    writer
        .write_all(br#"{"version":1,"date":1}"#)
        .expect("write");
    for (at, lines) in boards.iter().enumerate() {
        writer
            .start_file(format!("c{at}/container.jsonl"), options)
            .expect("start");
        for line in *lines {
            writer
                .write_all(format!("{line}\n").as_bytes())
                .expect("write");
        }
    }
    writer.finish().expect("finish").into_inner()
}

fn read_all_jsonl(zip_bytes: &[u8]) -> String {
    let mut archive = zip::ZipArchive::new(Cursor::new(zip_bytes)).expect("valid zip");
    let mut out = String::new();
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).expect("entry");
        if entry.name().expect("zip entry name").ends_with(".jsonl") {
            let mut text = String::new();
            entry.read_to_string(&mut text).expect("utf-8 jsonl");
            out.push_str(&text);
        }
    }
    out
}

fn board_line(id: &str, kind: &str, title: &str) -> Value {
    json!({"type": "container", "data": {
        "id": id, "workspaceId": TEST_WORKSPACE_ID, "visibility": "private", "kind": kind,
        "title": title, "createdAt": 1, "updatedAt": 1,
    }})
}

fn unit_line(id: &str, course: &str, parent: &str, fields: &Value) -> Value {
    json!({"type": "block", "data": {
        "id": id, "containerId": course, "parentId": parent, "type": "unit",
        "title": id, "fields": fields, "createdAt": 1, "updatedAt": 1,
    }})
}

fn schedule_line(course: &str, start: &str, target: &str, study_days: i32) -> Value {
    json!({"type": "courseSchedule", "data": {
        "containerId": course,
        "startDate": start,
        "targetDate": target,
        "studyDays": study_days,
    }})
}

async fn courses_at(client: &Client, at: &str) -> Vec<Container> {
    client.get(&format!("{at}/courses")).await.ok_json()
}

struct SeededCourse {
    id: String,
    pic: String,
}

async fn seed_course_for_archive(client: &Client, external_card: &str) -> SeededCourse {
    let course = client
        .post(
            &format!("{TEST_WS}/courses"),
            &json!({"visibility": "open", "title": "Distributed Systems", "description": "8-week plan"}),
        )
        .await
        .created_json::<Container>();
    let ch1 = client
        .post(
            &format!("{TEST_WS}/courses/{}/units", course.id),
            &json!({"kind": "chapter", "title": "Foundations"}),
        )
        .await
        .created_json::<Value>()["id"]
        .as_str()
        .expect("ch1")
        .to_owned();
    let u1 = client
        .post(
            &format!("{TEST_WS}/courses/{}/units", course.id),
            &json!({
                "parentId": ch1, "title": "Clocks and Time",
                "amount": 60, "amountUnit": "minutes", "scheduledDate": "2026-12-03"
            }),
        )
        .await
        .created_json::<Value>()["id"]
        .as_str()
        .expect("u1")
        .to_owned();
    let u2 = client
        .post(
            &format!("{TEST_WS}/courses/{}/units", course.id),
            &json!({
                "parentId": ch1, "title": "Consensus Lab",
                "amount": 15, "amountUnit": "problems", "scheduledDate": "2026-12-07"
            }),
        )
        .await
        .created_json::<Value>()["id"]
        .as_str()
        .expect("u2")
        .to_owned();

    let pic = uploaded_id(&upload(client, &course.id, "raft.png", b"raft-diagram\n").await);
    for (unit_id, body) in [
        (&u1, body_naming("prereq", &[&u2, external_card])),
        (
            &u2,
            json!({"type": "doc", "content": [{"type": "image", "attrs": {"fileId": pic}}]}),
        ),
    ] {
        let path = format!("{TEST_WS}/courses/{}/units/{unit_id}", course.id);
        let read_at = client.get(&path).await.value()["updatedAt"].clone();
        client
            .patch(&path, &json!({"readAt": read_at, "body": body}))
            .await
            .check_ok();
    }

    // Shared schedule + personal unit_progress (which must NEVER be exported).
    client
        .put(
            &format!("{TEST_WS}/courses/{}/schedule", course.id),
            &json!({"startDate": "2026-12-01", "targetDate": "2026-12-20", "studyDays": 42}),
        )
        .await
        .check_ok();
    client
        .put(
            &format!("{TEST_WS}/courses/{}/units/{u1}/progress", course.id),
            &json!({"completed": true, "lastPosition": "secret-pos-42"}),
        )
        .await
        .check_ok();
    client
        .put(
            &format!("{TEST_WS}/courses/{}/units/{u1}/schedule", course.id),
            &json!({"plannedDate": "2026-12-25"}),
        )
        .await
        .check_ok();
    SeededCourse { id: course.id, pic }
}

async fn verify_imported_course(
    client: &Client,
    other_ws: &str,
    src: &SeededCourse,
    external_card: &str,
) {
    let imported_courses = courses_at(client, other_ws).await;
    assert_eq!(imported_courses.len(), 1, "one imported course");
    let imported = &imported_courses[0];
    assert_eq!(imported.kind, ContainerKind::Course, "imported kind");
    assert_eq!(imported.title, "Distributed Systems", "imported title");
    assert_ne!(imported.id, src.id, "imported course gets new id");

    let tree: Vec<UnitNode> = client
        .get(&format!("{other_ws}/courses/{}/units", imported.id))
        .await
        .ok_json();
    assert_eq!(tree.len(), 3, "one chapter and two units imported");
    let imp_u1 = tree
        .iter()
        .find(|n| n.title == "Clocks and Time")
        .expect("u1");
    let imp_u2 = tree
        .iter()
        .find(|n| n.title == "Consensus Lab")
        .expect("u2");
    assert_eq!(imp_u1.amount, Some(60), "u1 amount preserved");
    assert_eq!(
        imp_u1.amount_unit,
        Some(AmountUnit::Minutes),
        "u1 amountUnit preserved"
    );
    assert_eq!(
        imp_u1.scheduled_date.as_deref(),
        Some("2026-12-03"),
        "u1 scheduledDate preserved"
    );
    assert_eq!(imp_u2.amount, Some(15), "u2 amount preserved");
    assert_eq!(
        imp_u2.amount_unit,
        Some(AmountUnit::Problems),
        "u2 amountUnit preserved"
    );
    assert_eq!(
        imp_u2.scheduled_date.as_deref(),
        Some("2026-12-07"),
        "u2 scheduledDate preserved"
    );

    let imp_u1_body = client
        .get(&format!(
            "{other_ws}/courses/{}/units/{}",
            imported.id, imp_u1.id
        ))
        .await
        .ok_json::<Value>()["body"]
        .clone();
    assert_eq!(
        references_in(&imp_u1_body),
        [imp_u2.id.clone(), external_card.to_owned()],
        "internal ref remapped and external ref kept"
    );
    let imp_u2_body = client
        .get(&format!(
            "{other_ws}/courses/{}/units/{}",
            imported.id, imp_u2.id
        ))
        .await
        .ok_json::<Value>()["body"]
        .clone();
    let imp_file = imp_u2_body["content"][0]["attrs"]["fileId"]
        .as_str()
        .expect("imported fileId");
    assert_ne!(imp_file, src.pic, "imported file gets new id");
    let file_resp = client
        .get(&format!(
            "{other_ws}/containers/{}/files/{imp_file}",
            imported.id
        ))
        .await;
    file_resp.check_ok();
    assert_eq!(file_resp.body, b"raft-diagram\n", "imported file bytes");

    let sched_view: CourseScheduleView = client
        .get(&format!("{other_ws}/courses/{}/schedule", imported.id))
        .await
        .ok_json();
    let sched = sched_view.schedule.expect("shared schedule restored");
    assert_eq!(sched.start_date, "2026-12-01", "schedule startDate");
    assert_eq!(sched.target_date, "2026-12-20", "schedule targetDate");
    assert_eq!(sched.study_days, 42, "schedule studyDays");
    assert!(
        sched_view
            .units
            .iter()
            .all(|u| u.my_planned_date.is_none() && !u.completed),
        "no personal planned dates or completions on imported course"
    );
    let progress: Vec<UnitProgress> = client
        .get(&format!("{other_ws}/courses/{}/progress", imported.id))
        .await
        .ok_json();
    assert_eq!(progress, [], "imported course has no personal progress");
}

#[tokio::test]
async fn course_archive_round_trip_preserves_units_amounts_schedule_files_refs_and_excludes_personal_progress()
 {
    let ts = Setup::native().start().await.init_basic().await;
    let client = &ts.client;
    let elsewhere = made(client, TEST_WS, "open", "elsewhere").await;
    let src = seed_course_for_archive(client, &elsewhere.card).await;

    let exported = client
        .get(&format!("{TEST_WS}/containers/{}/archive/export", src.id))
        .await;
    exported.check_ok();
    let jsonl = read_all_jsonl(&exported.body);
    assert!(
        jsonl.contains(r#""type":"courseSchedule""#),
        "shared courseSchedule exported: {jsonl}"
    );
    for forbidden in [
        "unit_progress",
        "unitProgress",
        "plannedDate",
        "planned_date",
        "lastPosition",
        "last_position",
        "secret-pos-42",
        "2026-12-25",
        "completedAt",
    ] {
        assert!(
            !jsonl.contains(forbidden),
            "personal field `{forbidden}` leaked into archive: {jsonl}"
        );
    }

    let me = client.me().await.id;
    let other_ws_id = another_workspace(&ts, &me).await;
    let other_ws = format!("/w/{other_ws_id}");
    let summary = import(client, &other_ws_id, &exported.body)
        .await
        .ok_json::<Value>();
    assert_eq!(
        summary["imported"]["courses"],
        json!(1),
        "one course counted"
    );
    assert_eq!(summary["imported"]["units"], json!(2), "two units counted");

    verify_imported_course(client, &other_ws, &src, &elsewhere.card).await;
    ts.teardown().await;
}

fn bad_schedule_cases() -> Vec<(&'static str, Vec<Value>)> {
    vec![
        (
            "studyDays == 0",
            vec![
                board_line("c1", "course", "Bad Course"),
                schedule_line("c1", "2026-12-01", "2026-12-10", 0),
            ],
        ),
        (
            "studyDays == 128",
            vec![
                board_line("c1", "course", "Bad Course"),
                schedule_line("c1", "2026-12-01", "2026-12-10", 128),
            ],
        ),
        (
            "studyDays == -1",
            vec![
                board_line("c1", "course", "Bad Course"),
                schedule_line("c1", "2026-12-01", "2026-12-10", -1),
            ],
        ),
        (
            "startDate > targetDate",
            vec![
                board_line("c1", "course", "Bad Course"),
                schedule_line("c1", "2026-12-20", "2026-12-10", 31),
            ],
        ),
        (
            "span > 3660 days",
            vec![
                board_line("c1", "course", "Bad Course"),
                schedule_line("c1", "2020-01-01", "2035-01-01", 31),
            ],
        ),
        (
            "no matching study day in range",
            vec![
                board_line("c1", "course", "Bad Course"),
                schedule_line("c1", "2026-12-05", "2026-12-06", 31),
            ],
        ),
        (
            "courseSchedule on board",
            vec![
                board_line("b1", "board", "Not A Course"),
                schedule_line("b1", "2026-12-01", "2026-12-10", 31),
            ],
        ),
        (
            "courseSchedule on wiki",
            vec![
                board_line("w1", "wiki", "Not A Course"),
                schedule_line("w1", "2026-12-01", "2026-12-10", 31),
            ],
        ),
        (
            "duplicate courseSchedule",
            vec![
                board_line("c1", "course", "Dup Schedule"),
                schedule_line("c1", "2026-12-01", "2026-12-10", 31),
                schedule_line("c1", "2026-12-02", "2026-12-11", 31),
            ],
        ),
        (
            "courseSchedule naming unknown containerId",
            vec![
                board_line("c1", "course", "Unknown Target"),
                schedule_line("non-existent-id", "2026-12-01", "2026-12-10", 31),
            ],
        ),
        (
            "courseSchedule with empty containerId and multiple containers",
            vec![
                board_line("c1", "course", "Course One"),
                board_line("c2", "course", "Course Two"),
                json!({"type": "courseSchedule", "data": {
                    "containerId": "", "startDate": "2026-12-01",
                    "targetDate": "2026-12-10", "studyDays": 31
                }}),
            ],
        ),
    ]
}

fn bad_unit_and_field_cases() -> Vec<(&'static str, Vec<Value>)> {
    vec![
        (
            "legacy boardId field in courseSchedule",
            vec![
                board_line("c1", "course", "Legacy Field"),
                json!({"type": "courseSchedule", "data": {
                    "boardId": "c1", "startDate": "2026-12-01",
                    "targetDate": "2026-12-10", "studyDays": 31
                }}),
            ],
        ),
        (
            "unknown field in courseSchedule",
            vec![
                board_line("c1", "course", "Extra Field"),
                json!({"type": "courseSchedule", "data": {
                    "containerId": "c1", "startDate": "2026-12-01",
                    "targetDate": "2026-12-10", "studyDays": 31, "plannedDate": "2026-12-05"
                }}),
            ],
        ),
        (
            "invalid unit amount == 0",
            vec![
                board_line("c1", "course", "Bad Unit"),
                unit_line("u1", "c1", "", &json!({"order": "V", "amount": 0})),
            ],
        ),
        (
            "invalid unit amount > 100000",
            vec![
                board_line("c1", "course", "Bad Unit"),
                unit_line("u1", "c1", "", &json!({"order": "V", "amount": 100_001})),
            ],
        ),
        (
            "invalid unit scheduledDate",
            vec![
                board_line("c1", "course", "Bad Unit Date"),
                unit_line(
                    "u1",
                    "c1",
                    "",
                    &json!({"order": "V", "scheduledDate": "2026-02-30"}),
                ),
            ],
        ),
    ]
}

#[tokio::test]
async fn malicious_course_archives_are_refused_with_400_and_full_rollback() {
    let ts = Setup::native().start().await.init_basic().await;
    let client = &ts.client;
    let innocent_board = [board_line("good-board", "board", "Must Not Be Stored")];

    for (label, bad_lines) in bad_schedule_cases()
        .into_iter()
        .chain(bad_unit_and_field_cases())
    {
        let bytes = archive_of(&[&innocent_board, &bad_lines]);
        let resp = import(client, TEST_WORKSPACE_ID, &bytes).await;
        resp.check_bad_request();
        assert!(!resp.text().is_empty(), "{label}: error message present");
    }

    let boards: Vec<Container> = client.get(&format!("{TEST_WS}/boards")).await.ok_json();
    assert!(
        !boards.iter().any(|b| b.title == "Must Not Be Stored"),
        "rollback must leave zero boards from refused archives"
    );
    let courses = courses_at(client, TEST_WS).await;
    assert_eq!(courses, [], "rollback must leave zero courses");
    ts.teardown().await;
}
