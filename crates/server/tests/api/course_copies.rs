//! Copying a course (`duplicateContainer` for `ContainerKind::Course`), creating course
//! templates and courses from templates (including the built-in course template with day
//! shifting), authorization/guest/SSO/cross-org isolation, and course Prometheus metrics.

use std::collections::HashMap;

use fvoci_model::DuplicateAs;
use fvoci_model::{
    AmountUnit, Container, ContainerKind, ContainerMember, ContainerVisibility,
    ContainersAndBlocks, CourseScheduleView, UnitNode, UnitProgress, WorkspaceRole,
};
use fvoci_store::DbType;
use serde_json::{Value, json};

use crate::common::{
    Client, PASSWORD, ROOT_WORKSPACE_ID, ROOT_WS, Setup, TEST_WORKSPACE_ID, TEST_WS, TestServer,
};
use crate::export::import;
use crate::fake_oidc::FakeProvider;
use crate::files::{upload, uploaded_id};
use crate::metrics::scrape;
use crate::oidc::{org_of, set_provider, with_key_and_private_addresses};
use crate::own_space::{own_workspace, workspaces_and_own};
use crate::references::made;
use crate::references_shared::references_in;
use crate::references_to::body_naming;
use crate::sso::{Passwords, admin_through_provider, require_sso};

/// 2026-12-31 20:00 UTC: the last evening of 2026 in UTC, and 2027-01-01 in Seoul.
const NEW_YEARS_EVE: i64 = 1_798_747_200_000;

const TEMPLATE_MARKS: [&str; 5] = [
    "onboarding",
    "templateKey",
    "templateLanguage",
    "templateBaseDate",
    "templateSchedule",
];

type OutlineRow = (
    String,
    String,
    Option<u32>,
    Option<AmountUnit>,
    Option<String>,
);

async fn create_course(client: &Client, ws: &str, course_type: &str, title: &str) -> String {
    let made = client
        .post(
            &format!("{ws}/courses"),
            &json!({"visibility": course_type, "title": title, "description": "Study plan", "icon": "🎓"}),
        )
        .await;
    made.check_created();
    made.value()["id"].as_str().expect("course id").to_owned()
}

async fn create_unit(client: &Client, ws: &str, course: &str, payload: &Value) -> String {
    let made = client
        .post(&format!("{ws}/courses/{course}/units"), payload)
        .await;
    made.check_created();
    made.value()["id"].as_str().expect("unit id").to_owned()
}

async fn write_unit_body(client: &Client, ws: &str, course: &str, unit: &str, body: &Value) {
    let path = format!("{ws}/courses/{course}/units/{unit}");
    let read_at = client.get(&path).await.value()["updatedAt"].clone();
    client
        .patch(&path, &json!({"readAt": read_at, "body": body}))
        .await
        .check_ok();
}

async fn unit_tree(client: &Client, ws: &str, course: &str) -> Vec<UnitNode> {
    client
        .get(&format!("{ws}/courses/{course}/units"))
        .await
        .ok_json()
}

async fn unit_body(client: &Client, ws: &str, course: &str, unit: &str) -> Value {
    client
        .get(&format!("{ws}/courses/{course}/units/{unit}"))
        .await
        .ok_json::<Value>()["body"]
        .clone()
}

fn titled_outline(tree: &[UnitNode]) -> Vec<OutlineRow> {
    let titles: HashMap<&str, &str> = tree
        .iter()
        .map(|node| (node.id.as_str(), node.title.as_str()))
        .collect();
    tree.iter()
        .map(|node| {
            let parent = titles
                .get(node.parent_id.as_str())
                .copied()
                .unwrap_or("")
                .to_owned();
            (
                node.title.clone(),
                parent,
                node.amount,
                node.amount_unit,
                node.scheduled_date.clone(),
            )
        })
        .collect()
}

fn node_id<'a>(tree: &'a [UnitNode], title: &str) -> &'a str {
    tree.iter()
        .find(|node| node.title == title)
        .unwrap_or_else(|| panic!("node titled {title}"))
        .id
        .as_str()
}

struct RichCourse {
    id: String,
    u1: String,
    u2: String,
    pic: String,
    doc: String,
}

async fn seed_rich_course_personal_and_shared_state(ts: &TestServer, course: &str, u1: &str) {
    let client = &ts.client;
    client
        .put(
            &format!("{TEST_WS}/courses/{course}/schedule"),
            &json!({"startDate": "2026-12-01", "targetDate": "2026-12-10", "studyDays": 31}),
        )
        .await
        .check_ok();
    client
        .post(
            &format!("{TEST_WS}/courses/{course}/units/{u1}/comments"),
            &json!({"body": body_naming("private discussion", &[])}),
        )
        .await
        .check_created();
    client
        .post(
            &format!("{TEST_WS}/containers/{course}/public-link"),
            &json!({"enabled": true}),
        )
        .await
        .check_ok();
    let other_id = ts.client2.me().await.id;
    client
        .post(
            &format!("{TEST_WS}/containers/{course}/members"),
            &json!({"containerId": course, "userId": other_id, "role": "editor"}),
        )
        .await
        .check_ok();
    client
        .put(
            &format!("{TEST_WS}/courses/{course}/units/{u1}/progress"),
            &json!({"completed": true, "lastPosition": "chapter-1-mark"}),
        )
        .await
        .check_ok();
    client
        .put(
            &format!("{TEST_WS}/courses/{course}/units/{u1}/schedule"),
            &json!({"plannedDate": "2026-12-09"}),
        )
        .await
        .check_ok();
}

async fn seed_rich_course(ts: &TestServer, external_card: &str) -> RichCourse {
    let client = &ts.client;
    let course = create_course(client, TEST_WS, "open", "Rust Systems").await;
    let ch1 = create_unit(
        client,
        TEST_WS,
        &course,
        &json!({"kind": "chapter", "title": "Chapter 1"}),
    )
    .await;
    let u1 = create_unit(
        client,
        TEST_WS,
        &course,
        &json!({
            "parentId": ch1, "title": "Ownership",
            "amount": 45, "amountUnit": "minutes", "scheduledDate": "2026-12-02"
        }),
    )
    .await;
    let u2 = create_unit(
        client,
        TEST_WS,
        &course,
        &json!({
            "parentId": ch1, "title": "Borrowing",
            "amount": 20, "amountUnit": "pages", "scheduledDate": "2026-12-04"
        }),
    )
    .await;
    let trashed = create_unit(
        client,
        TEST_WS,
        &course,
        &json!({"parentId": ch1, "title": "Trashed unit"}),
    )
    .await;
    client
        .delete(&format!("{TEST_WS}/courses/{course}/units/{trashed}"))
        .await
        .check_no_content();

    let pic = uploaded_id(&upload(client, &course, "diagram.png", b"png-bytes\n").await);
    let doc = uploaded_id(&upload(client, &course, "notes.pdf", b"%PDF-notes\n").await);
    write_unit_body(
        client,
        TEST_WS,
        &course,
        &u2,
        &json!({"type": "doc", "content": [
            {"type": "image", "attrs": {"fileId": pic}},
            {"type": "file", "attrs": {"fileId": doc}},
        ]}),
    )
    .await;
    write_unit_body(
        client,
        TEST_WS,
        &course,
        &u1,
        &body_naming("see", &[&u2, external_card, &u1]),
    )
    .await;
    seed_rich_course_personal_and_shared_state(ts, &course, &u1).await;
    RichCourse {
        id: course,
        u1,
        u2,
        pic,
        doc,
    }
}

async fn verify_copied_course_personal_excluded(ts: &TestServer, copy_id: &str, copy_u1: &str) {
    let client = &ts.client;
    let schedule_view: CourseScheduleView = client
        .get(&format!("{TEST_WS}/courses/{copy_id}/schedule"))
        .await
        .ok_json();
    let sched = schedule_view.schedule.expect("shared schedule copied");
    assert_eq!(sched.start_date, "2026-12-01", "copied schedule startDate");
    assert_eq!(
        sched.target_date, "2026-12-10",
        "copied schedule targetDate"
    );
    assert_eq!(sched.study_days, 31, "copied schedule studyDays");
    assert!(
        schedule_view
            .units
            .iter()
            .all(|u| u.my_planned_date.is_none() && !u.completed),
        "no personal plannedDate or completion copied"
    );

    let progress: Vec<UnitProgress> = client
        .get(&format!("{TEST_WS}/courses/{copy_id}/progress"))
        .await
        .ok_json();
    assert_eq!(progress, [], "personal progress excluded");
    let comments: Vec<Value> = client
        .get(&format!(
            "{TEST_WS}/courses/{copy_id}/units/{copy_u1}/comments"
        ))
        .await
        .ok_json();
    assert_eq!(comments, Vec::<Value>::new(), "comments excluded");
    let sharing = client
        .get(&format!("{TEST_WS}/containers/{copy_id}/public-link"))
        .await
        .value();
    assert_eq!(sharing["enabled"], json!(false), "sharing not copied");
    let members: Vec<ContainerMember> = client
        .get(&format!("{TEST_WS}/containers/{copy_id}/members"))
        .await
        .ok_json();
    assert_eq!(members.len(), 1, "only copier is a member");
    assert_eq!(
        ts.app
            .store()
            .body_versions(copy_u1)
            .await
            .expect("versions"),
        [],
        "body versions excluded"
    );
}

async fn verify_copied_course(
    ts: &TestServer,
    src: &RichCourse,
    copy: &Container,
    external_card: &str,
) {
    let client = &ts.client;
    assert_eq!(copy.kind, ContainerKind::Course, "copy kind");
    assert_eq!(
        copy.visibility,
        ContainerVisibility::Private,
        "copy is private"
    );
    assert_eq!(copy.title, "Rust Systems copy", "copy title");
    assert_eq!(copy.description, "Study plan", "copy description");
    assert_eq!(copy.icon, "🎓", "copy icon");

    let source_tree = unit_tree(client, TEST_WS, &src.id).await;
    let copy_tree = unit_tree(client, TEST_WS, &copy.id).await;
    assert_eq!(
        titled_outline(&copy_tree),
        titled_outline(&source_tree),
        "outline and amounts preserved"
    );
    assert_eq!(copy_tree.len(), 3, "trashed unit is excluded");
    let (copy_u1, copy_u2) = (
        node_id(&copy_tree, "Ownership"),
        node_id(&copy_tree, "Borrowing"),
    );
    assert_ne!(copy_u1, src.u1, "u1 gets new id");
    assert_ne!(copy_u2, src.u2, "u2 gets new id");

    let refs = references_in(&unit_body(client, TEST_WS, &copy.id, copy_u1).await);
    assert_eq!(
        refs,
        [
            copy_u2.to_owned(),
            external_card.to_owned(),
            copy_u1.to_owned()
        ],
        "internal refs remapped and external ref preserved"
    );

    let body2 = unit_body(client, TEST_WS, &copy.id, copy_u2).await;
    for (idx, (orig, bytes)) in [
        (&src.pic, &b"png-bytes\n"[..]),
        (&src.doc, &b"%PDF-notes\n"[..]),
    ]
    .into_iter()
    .enumerate()
    {
        let copied_file = body2["content"][idx]["attrs"]["fileId"]
            .as_str()
            .expect("copied fileId");
        assert_ne!(copied_file, orig, "copied file gets new id");
        let fetched = client
            .get(&format!(
                "{TEST_WS}/containers/{}/files/{copied_file}",
                copy.id
            ))
            .await;
        fetched.check_ok();
        assert_eq!(fetched.body, bytes, "copied file bytes match");
    }

    verify_copied_course_personal_excluded(ts, &copy.id, copy_u1).await;
}

#[tokio::test]
async fn course_duplication_copies_structure_amounts_schedule_files_and_refs_without_personal_data()
{
    let ts = Setup::local_mode().start().await.init_basic().await;
    let elsewhere = made(&ts.client, TEST_WS, "open", "elsewhere").await;
    let src = seed_rich_course(&ts, &elsewhere.card).await;

    let copied: ContainersAndBlocks = ts
        .client
        .post(
            &format!("{TEST_WS}/containers/{}/duplicate", src.id),
            &json!({}),
        )
        .await
        .created_json();
    verify_copied_course(&ts, &src, &copied.containers[0], &elsewhere.card).await;
    ts.teardown().await;
}

async fn verify_builtin_course_template_copy(ts: &TestServer) {
    let client = &ts.client;
    let builtin_ko = client
        .get(&format!("{ROOT_WS}/templates"))
        .await
        .ok_json::<Vec<Container>>()
        .into_iter()
        .find(|c| {
            c.is_builtin
                && c.kind == ContainerKind::Course
                && c.properties.get("templateKey") == Some(&json!("course-study-plan"))
        })
        .expect("built-in Korean course template");

    let user = client.me().await;
    let (utc_copy, _) = ts
        .app
        .duplicate_container_at(
            &builtin_ko.id,
            &user.id,
            ROOT_WORKSPACE_ID,
            DuplicateAs::Copy,
            None,
            NEW_YEARS_EVE,
        )
        .await
        .expect("built-in course template copied");
    let copy_course = &utc_copy.containers[0];
    assert_eq!(copy_course.kind, ContainerKind::Course, "copy is course");
    assert!(!copy_course.is_template, "copy is not a template");
    assert!(!copy_course.is_builtin, "copy is not built-in");
    for mark in TEMPLATE_MARKS {
        assert!(
            !copy_course.properties.contains_key(mark),
            "mark {mark} stripped"
        );
    }

    let sched: CourseScheduleView = client
        .get(&format!("{ROOT_WS}/courses/{}/schedule", copy_course.id))
        .await
        .ok_json();
    let s = sched.schedule.expect("built-in schedule materialized");
    assert_eq!(s.start_date, "2026-12-31", "UTC shifted startDate");
    assert_eq!(s.target_date, "2027-01-13", "UTC shifted targetDate");
    assert_eq!(s.study_days, 31, "weekday studyDays preserved");
    assert_eq!(
        sched.units[0].scheduled_date.as_deref(),
        Some("2026-12-31"),
        "first unit scheduledDate shifted"
    );
    assert_eq!(sched.units.len(), 5, "five units from template");

    let tree = unit_tree(client, ROOT_WS, &copy_course.id).await;
    let practice_id = node_id(&tree, "실전 연습 문제 풀기");
    let concepts_id = node_id(&tree, "핵심 개념 정리");
    let u4_refs = references_in(&unit_body(client, ROOT_WS, &copy_course.id, practice_id).await);
    assert_eq!(
        u4_refs,
        [concepts_id.to_owned()],
        "practice unit references copied concepts unit"
    );

    client
        .patch("/users/me/settings", &json!({"timeZone": "Asia/Seoul"}))
        .await
        .check_ok();
    let (seoul_copy, _) = ts
        .app
        .duplicate_container_at(
            &builtin_ko.id,
            &user.id,
            ROOT_WORKSPACE_ID,
            DuplicateAs::Copy,
            None,
            NEW_YEARS_EVE,
        )
        .await
        .expect("seoul copy");
    let seoul_sched: CourseScheduleView = client
        .get(&format!(
            "{ROOT_WS}/courses/{}/schedule",
            seoul_copy.containers[0].id
        ))
        .await
        .ok_json();
    assert_eq!(
        seoul_sched.schedule.expect("seoul schedule").start_date,
        "2027-01-01",
        "Seoul timezone shifts one extra day across midnight"
    );
}

async fn verify_user_course_template_round_trip(client: &Client) {
    let src = create_course(client, ROOT_WS, "open", "Linear Algebra").await;
    let u = create_unit(
        client,
        ROOT_WS,
        &src,
        &json!({"title": "Vectors", "amount": 30, "amountUnit": "minutes", "scheduledDate": "2026-12-05"}),
    )
    .await;
    client
        .put(
            &format!("{ROOT_WS}/courses/{src}/schedule"),
            &json!({"startDate": "2026-12-01", "targetDate": "2026-12-20", "studyDays": 31}),
        )
        .await
        .check_ok();
    client
        .put(
            &format!("{ROOT_WS}/courses/{src}/units/{u}/progress"),
            &json!({"completed": true, "lastPosition": "p.12"}),
        )
        .await
        .check_ok();

    let tpl: ContainersAndBlocks = client
        .post(
            &format!("{ROOT_WS}/containers/{src}/duplicate?asTemplate=true"),
            &json!({}),
        )
        .await
        .created_json();
    let tpl_course = &tpl.containers[0];
    assert!(tpl_course.is_template, "marked as template");
    assert_eq!(tpl_course.kind, ContainerKind::Course, "template is course");
    assert_eq!(tpl_course.title, "New course template", "en default title");

    let listed: Vec<Container> = client.get(&format!("{ROOT_WS}/templates")).await.ok_json();
    assert!(
        listed.iter().any(|c| c.id == tpl_course.id),
        "course template listed by GET /templates"
    );
    let live_courses: Vec<Container> = client.get(&format!("{ROOT_WS}/courses")).await.ok_json();
    assert!(
        !live_courses.iter().any(|c| c.id == tpl_course.id),
        "course template excluded from GET /courses"
    );
    let today: Value = client
        .get(&format!("{ROOT_WS}/learning/today"))
        .await
        .ok_json();
    assert!(
        !today.to_string().contains(&tpl_course.id),
        "course template excluded from GET /learning/today"
    );
    let from_tpl: ContainersAndBlocks = client
        .post(
            &format!("{ROOT_WS}/containers/{}/duplicate", tpl_course.id),
            &json!({}),
        )
        .await
        .created_json();
    let made = &from_tpl.containers[0];
    assert!(!made.is_template, "copy is live course");
    assert_eq!(made.title, "New course template", "copy keeps title");
    let sched: CourseScheduleView = client
        .get(&format!("{ROOT_WS}/courses/{}/schedule", made.id))
        .await
        .ok_json();
    assert_eq!(
        sched.schedule.expect("schedule preserved").target_date,
        "2026-12-20",
        "schedule targetDate preserved"
    );
    let progress: Vec<UnitProgress> = client
        .get(&format!("{ROOT_WS}/courses/{}/progress", made.id))
        .await
        .ok_json();
    assert_eq!(
        progress,
        [],
        "course from template has no personal progress"
    );

    client
        .patch("/users/me/settings", &json!({"language": "ko"}))
        .await
        .check_ok();
    let ko_tpl: ContainersAndBlocks = client
        .post(
            &format!("{ROOT_WS}/containers/{src}/duplicate?asTemplate=true"),
            &json!({}),
        )
        .await
        .created_json();
    assert_eq!(
        ko_tpl.containers[0].title, "새 학습 템플릿",
        "Korean template default title"
    );
}

#[tokio::test]
async fn course_template_round_trip_and_builtin_course_template_day_shift_and_unmarking() {
    let ts = Setup::native()
        .with_templates()
        .start()
        .await
        .init_basic()
        .await;
    verify_user_course_template_round_trip(&ts.client).await;
    verify_builtin_course_template_copy(&ts).await;
    ts.teardown().await;
}

async fn verify_guest_outsider_and_sso_refusals(
    ts: &TestServer,
    provider: &FakeProvider,
    open_course: &str,
    priv_course: &str,
) {
    let guest = ts.add_user("course_guest").await;
    let guest_id = guest.me().await.id;
    ts.app
        .store()
        .execute_unchecked(&format!(
            "UPDATE workspace_members SET role = '{}' WHERE user_id = '{guest_id}' \
             AND workspace_id = '{TEST_WORKSPACE_ID}'",
            WorkspaceRole::Guest.as_str()
        ))
        .await
        .expect("guest role");
    ts.client
        .post(
            &format!("{TEST_WS}/containers/{priv_course}/members"),
            &json!({"containerId": priv_course, "userId": guest_id, "role": "viewer"}),
        )
        .await
        .check_ok();
    guest
        .post(
            &format!("{TEST_WS}/containers/{priv_course}/duplicate"),
            &json!({}),
        )
        .await
        .check_not_found();

    let outsider = ts.add_user("course_outsider").await;
    let outsider_own = own_workspace(&outsider).await.id;
    outsider
        .post(
            &format!("/w/{outsider_own}/containers/{open_course}/duplicate"),
            &json!({}),
        )
        .await
        .check_not_found();

    let org_id = org_of(ts).await;
    set_provider(ts, &org_id, provider).await;
    let sso_admin = admin_through_provider(ts, provider, &org_id).await;
    require_sso(&sso_admin, &org_id, provider, Passwords::Refused)
        .await
        .check_ok();
    ts.client
        .post(
            &format!("{TEST_WS}/containers/{open_course}/duplicate"),
            &json!({}),
        )
        .await
        .check_unauthorized();
}

#[tokio::test]
async fn course_duplication_permissions_own_space_guest_sso_and_cross_org_isolation() {
    let provider = FakeProvider::start().await;
    let ts = Setup::native()
        .with_templates()
        .trusting_oidc_provider(&provider.certificate_der)
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
    let owner = &ts.client;
    let member = &ts.client2;
    let member_id = member.me().await.id;

    // Shared workspace: open course readable by unjoined member, but duplication requires membership (403); private 404.
    let open_course = create_course(owner, TEST_WS, "open", "Open Study").await;
    let priv_course = create_course(owner, TEST_WS, "private", "Private Study").await;
    member
        .post(
            &format!("{TEST_WS}/containers/{open_course}/duplicate"),
            &json!({}),
        )
        .await
        .check_forbidden();
    member
        .post(
            &format!("{TEST_WS}/containers/{open_course}/join"),
            &json!({}),
        )
        .await
        .check_ok();
    member
        .post(
            &format!("{TEST_WS}/containers/{open_course}/duplicate"),
            &json!({}),
        )
        .await
        .check_created();
    member
        .post(
            &format!("{TEST_WS}/containers/{priv_course}/duplicate"),
            &json!({}),
        )
        .await
        .check_not_found();

    owner
        .post(
            &format!("{TEST_WS}/containers/{priv_course}/members"),
            &json!({"containerId": priv_course, "userId": member_id, "role": "viewer"}),
        )
        .await
        .check_ok();
    member
        .post(
            &format!("{TEST_WS}/containers/{priv_course}/duplicate"),
            &json!({}),
        )
        .await
        .check_created();

    let own_ws = format!("/w/{}", own_workspace(owner).await.id);
    let own_course = create_course(owner, &own_ws, "private", "My Study").await;
    owner
        .post(
            &format!("{own_ws}/containers/{own_course}/duplicate"),
            &json!({}),
        )
        .await
        .check_created();
    let builtin = owner
        .get(&format!("{own_ws}/templates"))
        .await
        .ok_json::<Vec<Container>>()
        .into_iter()
        .find(|c| c.is_builtin && c.kind == ContainerKind::Course)
        .expect("built-in course template in own space");
    owner
        .post(
            &format!("{own_ws}/containers/{}/duplicate", builtin.id),
            &json!({}),
        )
        .await
        .check_created();

    verify_guest_outsider_and_sso_refusals(&ts, &provider, &open_course, &priv_course).await;
    ts.teardown().await;
}

async fn exercise_course_metric_events(client: &Client) {
    let course = create_course(client, TEST_WS, "open", "Metrics Course").await;
    let unit = create_unit(client, TEST_WS, &course, &json!({"title": "Unit 1"})).await;

    let builtin = client
        .get(&format!("{TEST_WS}/templates"))
        .await
        .ok_json::<Vec<Container>>()
        .into_iter()
        .find(|c| c.is_builtin && c.kind == ContainerKind::Course)
        .expect("built-in course template");
    client
        .post(
            &format!("{TEST_WS}/containers/{}/duplicate", builtin.id),
            &json!({}),
        )
        .await
        .check_created();
    client
        .post(
            &format!("{TEST_WS}/containers/{course}/duplicate"),
            &json!({}),
        )
        .await
        .check_created();

    let exported = client
        .get(&format!("{TEST_WS}/containers/{course}/archive/export"))
        .await;
    exported.check_ok();
    import(client, TEST_WORKSPACE_ID, &exported.body)
        .await
        .check_ok();

    for completed in [true, false] {
        client
            .put(
                &format!("{TEST_WS}/courses/{course}/units/{unit}/progress"),
                &json!({"completed": completed}),
            )
            .await
            .check_ok();
    }
    client
        .put(
            &format!("{TEST_WS}/courses/{course}/schedule"),
            &json!({
                "startDate": "2026-12-01", "targetDate": "2026-12-10",
                "studyDays": 31, "autoAssign": true
            }),
        )
        .await
        .check_ok();
    client
        .post(
            &format!("{TEST_WS}/courses/{course}/schedule/redistribute"),
            &json!({"fromDate": "2026-12-02", "targetDate": "2026-12-12", "studyDays": 31}),
        )
        .await
        .check_ok();
    client
        .post(
            &format!("{TEST_WS}/courses/{course}/my-schedule/redistribute"),
            &json!({"fromDate": "2026-12-03", "targetDate": "2026-12-12", "studyDays": 31}),
        )
        .await
        .check_ok();
}

#[tokio::test]
async fn course_metrics_record_creations_copies_completions_and_schedule_redistributions() {
    let ts = Setup::native()
        .with_templates()
        .metrics_at("127.0.0.1:0")
        .start()
        .await
        .init_basic()
        .await;
    exercise_course_metric_events(&ts.client).await;

    let body = scrape(&ts).await;
    for expected in [
        r#"fvoci_courses_created_total{source="blank"} 1"#,
        r#"fvoci_courses_created_total{source="template"} 1"#,
        r#"fvoci_courses_created_total{source="duplicate"} 1"#,
        r#"fvoci_courses_created_total{source="import"} 1"#,
        r#"fvoci_containers_copied_total{kind="course",work="duplicate"} 2"#,
        r#"fvoci_containers_copied_total{kind="course",work="export"} 1"#,
        r#"fvoci_containers_copied_total{kind="course",work="import"} 1"#,
        r#"fvoci_course_unit_completions_total{action="completed"} 1"#,
        r#"fvoci_course_unit_completions_total{action="uncompleted"} 1"#,
        r#"fvoci_course_schedule_redistributions_total{scope="shared_auto"} 1"#,
        r#"fvoci_course_schedule_redistributions_total{scope="shared_manual"} 1"#,
        r#"fvoci_course_schedule_redistributions_total{scope="personal"} 1"#,
    ] {
        assert!(body.contains(expected), "missing `{expected}` in:\n{body}");
    }
    for forbidden in ["org_id=", "course_id=", "user_id="] {
        assert!(
            !body.contains(forbidden),
            "high-cardinality label `{forbidden}` found"
        );
    }
    ts.teardown().await;
}

/// Across two separate organizations under PostgreSQL RLS (`org_creation = "open"`):
/// org 2 cannot duplicate or export org 1's open course or user course template, while
/// org 2 CAN list and duplicate the built-in course template into its own workspace.
#[tokio::test]
async fn course_duplication_cross_org_isolation_and_builtin_templates() {
    if fvoci_store::testing::db_type_of_the_tests() != DbType::Postgres {
        return;
    }
    let ts = Setup::native()
        .on_an_empty_database()
        .with_templates()
        .configure(|cfg| "open".clone_into(&mut cfg.org_creation))
        .start()
        .await;
    for (username, org_name) in [("course_alpha", "Alpha Org"), ("course_beta", "Beta Org")] {
        ts.anonymous()
            .post(
                "/register",
                &json!({
                    "username": username,
                    "email": format!("{username}@example.com"),
                    "password": PASSWORD,
                    "organizationName": org_name,
                }),
            )
            .await
            .check_created();
    }
    let alpha = ts.login("course_alpha", PASSWORD).await;
    let beta = ts.login("course_beta", PASSWORD).await;
    let alpha_ws = format!("/w/{}", workspaces_and_own(&alpha).await.0[0].id);
    let beta_ws = format!("/w/{}", workspaces_and_own(&beta).await.0[0].id);

    let open_course = create_course(&alpha, &alpha_ws, "open", "Alpha Open Course").await;
    let user_tpl: ContainersAndBlocks = alpha
        .post(
            &format!("{alpha_ws}/containers/{open_course}/duplicate?asTemplate=true"),
            &json!({}),
        )
        .await
        .created_json();
    let user_tpl_id = &user_tpl.containers[0].id;

    for target in [&open_course, user_tpl_id] {
        beta.post(
            &format!("{beta_ws}/containers/{target}/duplicate"),
            &json!({}),
        )
        .await
        .check_not_found();
    }
    beta.get(&format!(
        "{beta_ws}/containers/{open_course}/archive/export"
    ))
    .await
    .check_not_found();

    let builtin = beta
        .get(&format!("{beta_ws}/templates"))
        .await
        .ok_json::<Vec<Container>>()
        .into_iter()
        .find(|c| c.is_builtin && c.kind == ContainerKind::Course)
        .expect("org 2 lists built-in course template under RLS");
    let copied: ContainersAndBlocks = beta
        .post(
            &format!("{beta_ws}/containers/{}/duplicate", builtin.id),
            &json!({}),
        )
        .await
        .created_json();
    let made = &copied.containers[0];
    assert_eq!(made.kind, ContainerKind::Course, "org 2 built-in copy kind");
    let sched: CourseScheduleView = beta
        .get(&format!("{beta_ws}/courses/{}/schedule", made.id))
        .await
        .ok_json();
    assert!(sched.schedule.is_some(), "org 2 built-in schedule copied");
    assert_eq!(sched.units.len(), 5, "org 2 built-in five units copied");
    ts.teardown().await;
}
