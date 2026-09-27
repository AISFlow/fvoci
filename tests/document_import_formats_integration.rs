#![cfg(feature = "db-tests")]
#![allow(dead_code)]
//! Office-file formats (PDF / DOCX / PPTX / XLSX / ODT / ODP / ODS through the
//! office child) and the Notion remainder (CSV databases → tasks, assets →
//! attachments, `projectId`), with deferred event publication and
//! compensation, on the real app role.

#[path = "support/project_harness.rs"]
mod project_harness;

#[path = "support/import_harness.rs"]
mod import_harness;

#[path = "support/office_fixtures.rs"]
mod office_fixtures;

#[path = "support/license.rs"]
mod license_fixture;

use axum::http::StatusCode;
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use fvoci_server::db::context::defer_import_events;
use fvoci_server::db::documents::ImportFence;
use fvoci_server::db::import_jobs::claim_next_import_job;
use fvoci_server::db::quota::{QuotaLimit, StorageQuota};
use fvoci_server::db::tasks::{create_import_task, CreateTaskInput};
use fvoci_server::import_job::{run_next_import, sweep_orphan_imports, ImportJobSettings};
use import_harness::*;
use project_harness::{
    add_workspace_user, create_project, insert_minimal_project, json_request, TestDb,
};
use serde_json::{json, Value};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const PNG: &[u8] =
    b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01\0\0\0\x01\x08\x06\0\0\0\x1f\x15\xc4\x89";

/// A fresh workspace admin per import: the route allows 5 imports per user
/// per 15 minutes, and these suites import more than that.
async fn fresh_admin(fx: &Fixture) -> String {
    add_workspace_user(&fx.admin, fx.workspace_id, "admin", "importer")
        .await
        .cookie
}

async fn office_import(fx: &Fixture, file_name: &str, bytes: &[u8]) -> Uuid {
    let cookie = fresh_admin(fx).await;
    let (status, body) = fx
        .import(
            &cookie,
            json!({
                "workspaceId": fx.workspace_id,
                "source": "office-file",
                "fileName": file_name,
                "zipBase64": B64.encode(bytes)
            }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{file_name}: {body}");
    body["id"].as_str().unwrap().parse().unwrap()
}

async fn notion_import(fx: &Fixture, zip: Vec<u8>, project_id: Option<Uuid>) -> Uuid {
    let mut body = json!({
        "workspaceId": fx.workspace_id,
        "source": "notion-zip",
        "zipBase64": B64.encode(zip)
    });
    if let Some(project_id) = project_id {
        body["projectId"] = json!(project_id);
    }
    let (status, body) = fx.import(&fx.cookie, body).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["id"].as_str().unwrap().parse().unwrap()
}

async fn job_state(admin: &PgPool, job_id: Uuid) -> (String, bool, Value) {
    sqlx::query_as(
        "SELECT status, payload IS NOT NULL, created_refs FROM fvoci.import_jobs WHERE id = $1",
    )
    .bind(job_id)
    .fetch_one(admin)
    .await
    .unwrap()
}

async fn body_json(fx: &Fixture, doc_id: &str) -> String {
    let (status, body) = json_request(
        fx.app.clone(),
        "GET",
        &format!(
            "/api/v1/workspaces/{}/documents/{doc_id}/body",
            fx.workspace_id
        ),
        None,
        Some(&fx.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["contentJson"].to_string()
}

async fn count(admin: &PgPool, sql: &str, id: Uuid) -> i64 {
    sqlx::query_scalar(sql)
        .bind(id)
        .fetch_one(admin)
        .await
        .unwrap()
}

async fn events_for(admin: &PgPool, verb: &str, target: Uuid) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM fvoci.events WHERE verb = $1 AND target_id = $2")
        .bind(verb)
        .bind(target)
        .fetch_one(admin)
        .await
        .unwrap()
}

fn ids(refs: &Value, key: &str) -> Vec<Uuid> {
    refs[key]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().parse().unwrap())
        .collect()
}

#[tokio::test]
async fn office_import_every_format_creates_a_document_with_its_text() {
    let harness = TestDb::bootstrap().await;
    let fx = fixture(&harness).await;
    let cases: Vec<(&str, Vec<u8>, &str)> = vec![
        (
            "회의록.docx",
            office_fixtures::docx("회의 제목", &["docx 본문"]),
            "docx 본문",
        ),
        (
            "발표.pptx",
            office_fixtures::pptx(&[("표지", "pptx 본문")]),
            "pptx 본문",
        ),
        (
            "예산.xlsx",
            office_fixtures::xlsx("예산", &[&["항목", "금액"], &["서버", "1200"]]),
            "서버",
        ),
        (
            "문서.odt",
            office_fixtures::odt("제목", &["odt 본문"]),
            "odt 본문",
        ),
        (
            "발표.odp",
            office_fixtures::odp(&[("표지", "odp 본문")]),
            "odp 본문",
        ),
        (
            "시트.ods",
            office_fixtures::ods("시트1", &[&["ods 셀"]]),
            "ods 셀",
        ),
        (
            "report.PDF",
            office_fixtures::pdf(&["pdf body text"]),
            "pdf body text",
        ),
    ];
    for (name, bytes, expected) in cases {
        let job_id = office_import(&fx, name, &bytes).await;
        assert!(fx.run_next().await);
        let (status, has_payload, refs) = job_state(&fx.admin, job_id).await;
        assert_eq!(
            (status.as_str(), has_payload),
            ("completed", false),
            "{name}"
        );
        let docs = ids(&refs, "documentIds");
        assert_eq!(docs.len(), 1, "{name}");
        let title: String = sqlx::query_scalar("SELECT title FROM fvoci.documents WHERE id = $1")
            .bind(docs[0])
            .fetch_one(&fx.admin)
            .await
            .unwrap();
        assert_eq!(title, name.rsplit_once('.').unwrap().0);
        let body = body_json(&fx, &docs[0].to_string()).await;
        assert!(body.contains(expected), "{name}: {body}");
        if name.ends_with(".docx") {
            assert!(body.contains("\"heading\""), "{body}");
            assert!(body.contains("\"table\""), "{body}");
        }
    }
    harness.cleanup().await;
}

#[tokio::test]
async fn hostile_office_files_fail_without_leaving_documents() {
    let harness = TestDb::bootstrap().await;
    let fx = fixture(&harness).await;
    let before = fx.document_count().await;
    let mut truncated_pdf = office_fixtures::pdf(&["x"]);
    truncated_pdf.truncate(60);
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("bomb.docx", office_fixtures::docx_bomb()),
        ("renamed.docx", office_fixtures::pdf(&["not a docx"])),
        ("sheet-as.docx", office_fixtures::xlsx("s", &[&["a"]])),
        ("broken.pdf", truncated_pdf),
        (
            "traversal.odt",
            office_fixtures::zip_of(&[("../content.xml", b"<a/>"), ("content.xml", b"<a/>")]),
        ),
        // Markdown over the document body limit (1 MiB).
        (
            "huge.docx",
            office_fixtures::docx("t", &[&"본문".repeat(300_000)]),
        ),
    ];
    for (name, bytes) in cases {
        let job_id = office_import(&fx, name, &bytes).await;
        assert!(fx.run_next().await);
        let (status, has_payload, _) = job_state(&fx.admin, job_id).await;
        assert_eq!((status.as_str(), has_payload), ("failed", false), "{name}");
        assert_eq!(fx.document_count().await, before, "{name} left a document");
    }
    // Unknown office formats are refused before a job row exists.
    let jobs_before: i64 = sqlx::query_scalar("SELECT count(*) FROM fvoci.import_jobs")
        .fetch_one(&fx.admin)
        .await
        .unwrap();
    for name in ["legacy.doc", "book.epub", "noext"] {
        let cookie = fresh_admin(&fx).await;
        let (status, body) = fx
            .import(
                &cookie,
                json!({
                    "workspaceId": fx.workspace_id,
                    "source": "office-file",
                    "fileName": name,
                    "zipBase64": B64.encode(b"x")
                }),
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{name}: {body}");
        assert_eq!(body["code"], "import_failed", "{body}");
    }
    let jobs_after: i64 = sqlx::query_scalar("SELECT count(*) FROM fvoci.import_jobs")
        .fetch_one(&fx.admin)
        .await
        .unwrap();
    assert_eq!(jobs_before, jobs_after);
    harness.cleanup().await;
}

/// Notion export with one page, a PNG and a text asset in its folder, a root
/// asset, and a CSV database of three rows.
fn notion_zip(assignee_email: &str, extra_row: Option<&str>) -> Vec<u8> {
    let mut csv = format!(
        "Name,Status,Assignee,Due date\n\"Write, the spec\",완료,{assignee_email},2026-03-05\nSecond task,Unknown status,nobody,someday\n,,,\n"
    );
    if let Some(row) = extra_row {
        csv.push_str(row);
        csv.push('\n');
    }
    csv.push_str("Third task,,,\n");
    office_fixtures::zip_of(&[
        (
            "Export/Plan 0123456789abcdef.md",
            "# 계획\n\n본문".as_bytes(),
        ),
        ("Export/Plan 0123456789abcdef/diagram.png", PNG),
        (
            "Export/Plan 0123456789abcdef/notes.txt",
            "메모 텍스트".as_bytes(),
        ),
        ("Export/Plan 0123456789abcdef/empty.bin", b""),
        (
            "Export/Plan 0123456789abcdef/Tasks 89abcdef01.csv",
            csv.as_bytes(),
        ),
    ])
}

async fn done_status(admin: &PgPool, project_id: Uuid) -> Uuid {
    sqlx::query_scalar("SELECT id FROM fvoci.statuses WHERE project_id = $1 AND name = '완료'")
        .bind(project_id)
        .fetch_one(admin)
        .await
        .expect("project has a Done status")
}

#[tokio::test]
async fn notion_csv_becomes_tasks_and_assets_become_attachments() {
    let harness = TestDb::bootstrap().await;
    let fx = fixture(&harness).await;
    let project = create_project(
        fx.app.clone(),
        &fx.cookie,
        fx.workspace_id,
        "IMP",
        "workspace",
    )
    .await;
    let project_id: Uuid = project["id"].as_str().unwrap().parse().unwrap();
    let member = add_workspace_user(&fx.admin, fx.workspace_id, "member", "assignee").await;
    let email: String = sqlx::query_scalar("SELECT email FROM fvoci.users WHERE id = $1")
        .bind(member.user_id)
        .fetch_one(&fx.admin)
        .await
        .unwrap();
    let job_id = notion_import(
        &fx,
        notion_zip(&email.to_uppercase(), None),
        Some(project_id),
    )
    .await;
    assert!(fx.run_next().await);
    let (status, has_payload, refs) = job_state(&fx.admin, job_id).await;
    assert_eq!(
        (status.as_str(), has_payload),
        ("completed", false),
        "{refs}"
    );

    // Tasks: blank rows skipped; status, due and assignee matched by header.
    let tasks: Vec<(Uuid, String, Uuid, Option<chrono::NaiveDate>, Uuid)> = sqlx::query_as(
        "SELECT id, title, status_id, due_date, project_id FROM fvoci.tasks WHERE workspace_id = $1 ORDER BY number",
    )
    .bind(fx.workspace_id)
    .fetch_all(&fx.admin)
    .await
    .unwrap();
    let titles: Vec<&str> = tasks.iter().map(|t| t.1.as_str()).collect();
    assert_eq!(titles, ["Write, the spec", "Second task", "Third task"]);
    assert!(tasks.iter().all(|t| t.4 == project_id));
    assert_eq!(tasks[0].2, done_status(&fx.admin, project_id).await);
    assert_eq!(tasks[0].3, chrono::NaiveDate::from_ymd_opt(2026, 3, 5));
    assert_ne!(
        tasks[1].2, tasks[0].2,
        "unknown status falls back to the default"
    );
    assert_eq!(tasks[1].3, None);
    let assignees: Vec<(Uuid, Uuid)> =
        sqlx::query_as("SELECT task_id, user_id FROM fvoci.task_assignees WHERE workspace_id = $1")
            .bind(fx.workspace_id)
            .fetch_all(&fx.admin)
            .await
            .unwrap();
    assert_eq!(assignees, vec![(tasks[0].0, member.user_id)]);
    let task_refs = ids(&refs, "taskIds");
    assert_eq!(task_refs, tasks.iter().map(|t| t.0).collect::<Vec<_>>());

    // Assets: attachments on the page (empty file skipped), bytes stored.
    let page: Uuid = sqlx::query_scalar(
        "SELECT id FROM fvoci.documents WHERE workspace_id = $1 AND title = 'Plan'",
    )
    .bind(fx.workspace_id)
    .fetch_one(&fx.admin)
    .await
    .unwrap();
    type AttachmentRow = (String, String, String, i64, String, String, String, Uuid);
    let attachments: Vec<AttachmentRow> = sqlx::query_as(
        r#"
            SELECT name, status, mime, size_bytes, extract_status, preview_status, storage_key,
                   document_id
            FROM fvoci.attachments WHERE workspace_id = $1 ORDER BY name
            "#,
    )
    .bind(fx.workspace_id)
    .fetch_all(&fx.admin)
    .await
    .unwrap();
    assert_eq!(attachments.len(), 2, "{attachments:?}");
    let (png, txt) = (&attachments[0], &attachments[1]);
    assert_eq!(
        (png.0.as_str(), png.1.as_str(), png.2.as_str(), png.3),
        ("diagram.png", "stored", "image/png", PNG.len() as i64)
    );
    assert_eq!((png.4.as_str(), png.5.as_str()), ("skipped", "pending"));
    assert_eq!((txt.0.as_str(), txt.4.as_str()), ("notes.txt", "pending"));
    assert!(attachments.iter().all(|a| a.7 == page));
    for a in &attachments {
        assert_eq!(fx.storage.head(&a.6).await.unwrap(), Some(a.3 as u64));
    }
    assert_eq!(ids(&refs, "storedKeys").len(), 2);

    // Deferred events were all published by the completing transaction.
    for task in &tasks {
        assert_eq!(events_for(&fx.admin, "task.created", task.0).await, 1);
    }
    assert_eq!(events_for(&fx.admin, "document.created", page).await, 1);
    let completed: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM fvoci.events WHERE verb = 'attachment.completed' AND workspace_id = $1",
    )
    .bind(fx.workspace_id)
    .fetch_one(&fx.admin)
    .await
    .unwrap();
    assert_eq!(completed, 2);
    assert_eq!(
        count(
            &fx.admin,
            "SELECT count(*) FROM fvoci.import_deferred_events WHERE import_job_id = $1",
            job_id
        )
        .await,
        0
    );
    // The created and published events keep their order: page, then tasks.
    let order: Vec<String> = sqlx::query_scalar(
        "SELECT verb FROM fvoci.events WHERE workspace_id = $1 AND verb IN ('document.created','task.created') ORDER BY seq",
    )
    .bind(fx.workspace_id)
    .fetch_all(&fx.admin)
    .await
    .unwrap();
    assert_eq!(order.first().map(String::as_str), Some("document.created"));

    // Without a project the CSV is ignored (source behavior).
    let job_id = notion_import(&fx, notion_zip("x@example.com", None), None).await;
    assert!(fx.run_next().await);
    let (status, _, refs) = job_state(&fx.admin, job_id).await;
    assert_eq!(status, "completed");
    assert!(ids(&refs, "taskIds").is_empty());
    harness.cleanup().await;
}

#[tokio::test]
async fn notion_failure_mid_tasks_compensates_rows_objects_and_events() {
    let harness = TestDb::bootstrap().await;
    let fx = fixture(&harness).await;
    let project = create_project(
        fx.app.clone(),
        &fx.cookie,
        fx.workspace_id,
        "BOOM",
        "workspace",
    )
    .await;
    let project_id: Uuid = project["id"].as_str().unwrap().parse().unwrap();
    // Failure injection in the database: the fourth CSV row cannot insert.
    sqlx::query(
        r#"
        CREATE FUNCTION fvoci.test_fail_boom_task() RETURNS trigger LANGUAGE plpgsql AS $$
        BEGIN
            IF NEW.title = 'boom' THEN RAISE EXCEPTION 'injected task failure'; END IF;
            RETURN NEW;
        END $$
        "#,
    )
    .execute(&fx.admin)
    .await
    .unwrap();
    sqlx::query(
        "CREATE TRIGGER test_fail_boom_task BEFORE INSERT ON fvoci.tasks FOR EACH ROW EXECUTE FUNCTION fvoci.test_fail_boom_task()",
    )
    .execute(&fx.admin)
    .await
    .unwrap();
    let docs_before = fx.document_count().await;
    let job_id = notion_import(
        &fx,
        notion_zip("nobody@example.com", Some("boom,,,")),
        Some(project_id),
    )
    .await;
    assert!(fx.run_next().await);
    let (status, has_payload, _) = job_state(&fx.admin, job_id).await;
    assert_eq!((status.as_str(), has_payload), ("failed", false));
    assert_eq!(fx.document_count().await, docs_before);
    assert_eq!(
        count(
            &fx.admin,
            "SELECT count(*) FROM fvoci.tasks WHERE project_id = $1",
            project_id
        )
        .await,
        0
    );
    assert_eq!(
        count(
            &fx.admin,
            "SELECT count(*) FROM fvoci.attachments WHERE workspace_id = $1",
            fx.workspace_id
        )
        .await,
        0
    );
    // Nothing the failed run created was ever published.
    for verb in ["task.created", "document.created", "attachment.completed"] {
        let n: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM fvoci.events WHERE workspace_id = $1 AND verb = $2 AND created_at > (SELECT created_at FROM fvoci.import_jobs WHERE id = $3)",
        )
        .bind(fx.workspace_id)
        .bind(verb)
        .bind(job_id)
        .fetch_one(&fx.admin)
        .await
        .unwrap();
        assert_eq!(n, 0, "{verb} leaked from a compensated import");
    }
    assert_eq!(
        count(
            &fx.admin,
            "SELECT count(*) FROM fvoci.import_deferred_events WHERE import_job_id = $1",
            job_id
        )
        .await,
        0
    );
    // Stored objects of the compensated attachments are gone.
    let (_, _, refs) = job_state(&fx.admin, job_id).await;
    let keys = refs["storedKeys"].as_array().unwrap().clone();
    assert_eq!(keys.len(), 2, "{refs}");
    for key in keys {
        assert_eq!(fx.storage.head(key.as_str().unwrap()).await.unwrap(), None);
    }
    harness.cleanup().await;
}

#[tokio::test]
async fn notion_project_permissions_are_checked_at_execution() {
    let harness = TestDb::bootstrap().await;
    let fx = fixture(&harness).await;
    // A private project the importing admin is not a member of.
    let private = create_project(
        fx.app.clone(),
        &fx.cookie,
        fx.workspace_id,
        "PRIV",
        "private",
    )
    .await;
    let private_id: Uuid = private["id"].as_str().unwrap().parse().unwrap();
    let importer = add_workspace_user(&fx.admin, fx.workspace_id, "admin", "importer").await;
    let docs_before = fx.document_count().await;
    let (status, body) = fx
        .import(
            &importer.cookie,
            json!({
                "workspaceId": fx.workspace_id,
                "source": "notion-zip",
                "projectId": private_id,
                "zipBase64": B64.encode(notion_zip("x@example.com", None))
            }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert!(fx.run_next().await);
    let job_id: Uuid = body["id"].as_str().unwrap().parse().unwrap();
    assert_eq!(job_state(&fx.admin, job_id).await.0, "failed");
    assert_eq!(fx.document_count().await, docs_before);
    assert_eq!(
        count(
            &fx.admin,
            "SELECT count(*) FROM fvoci.tasks WHERE project_id = $1",
            private_id
        )
        .await,
        0
    );

    // A project id from another workspace.
    let other_ws = Uuid::now_v7();
    sqlx::query("INSERT INTO fvoci.workspaces (id, slug, name) VALUES ($1, 'other-ws', 'Other')")
        .bind(other_ws)
        .execute(&fx.admin)
        .await
        .unwrap();
    let foreign = Uuid::now_v7();
    insert_minimal_project(&fx.admin, other_ws, foreign, "FOR", fx.user_id, "workspace").await;
    let job_id = notion_import(&fx, notion_zip("x@example.com", None), Some(foreign)).await;
    assert!(fx.run_next().await);
    assert_eq!(job_state(&fx.admin, job_id).await.0, "failed");
    assert_eq!(fx.document_count().await, docs_before);
    assert_eq!(
        count(
            &fx.admin,
            "SELECT count(*) FROM fvoci.tasks WHERE project_id = $1",
            foreign
        )
        .await,
        0
    );
    // A member (not admin) cannot start an import at all.
    let member = add_workspace_user(&fx.admin, fx.workspace_id, "member", "member").await;
    let (status, _) = fx
        .import(
            &member.cookie,
            json!({
                "workspaceId": fx.workspace_id,
                "source": "notion-zip",
                "zipBase64": B64.encode(notion_zip("x@example.com", None))
            }),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    harness.cleanup().await;
}

#[tokio::test]
async fn storage_quota_refuses_imported_assets_and_compensates() {
    let harness = TestDb::bootstrap().await;
    let fx = fixture(&harness).await;
    let docs_before = fx.document_count().await;
    let job_id = notion_import(&fx, notion_zip("x@example.com", None), None).await;
    let settings = ImportJobSettings {
        quota: StorageQuota::fixed(QuotaLimit::Bytes(8), QuotaLimit::Unlimited),
        ..fx.settings.clone()
    };
    assert!(
        run_next_import(&fx.pool, &settings, &fx.storage, &CancellationToken::new())
            .await
            .unwrap()
    );
    assert_eq!(job_state(&fx.admin, job_id).await.0, "failed");
    assert_eq!(fx.document_count().await, docs_before);
    assert_eq!(
        count(
            &fx.admin,
            "SELECT count(*) FROM fvoci.attachments WHERE workspace_id = $1",
            fx.workspace_id
        )
        .await,
        0
    );
    harness.cleanup().await;
}

#[tokio::test]
async fn signed_license_storage_limit_refuses_imported_asset_and_compensates() {
    let harness = TestDb::bootstrap().await;
    let fx = fixture(&harness).await;
    let docs_before = fx.document_count().await;
    let job_id = notion_import(&fx, notion_zip("x@example.com", None), None).await;
    let settings = ImportJobSettings {
        quota: StorageQuota::from_license(license_fixture::signed_license_with_limits(
            json!({"storageBytes": 8}),
        )),
        ..fx.settings.clone()
    };
    assert!(
        run_next_import(&fx.pool, &settings, &fx.storage, &CancellationToken::new())
            .await
            .unwrap()
    );
    assert_eq!(job_state(&fx.admin, job_id).await.0, "failed");
    assert_eq!(fx.document_count().await, docs_before);
    assert_eq!(
        count(
            &fx.admin,
            "SELECT count(*) FROM fvoci.attachments WHERE workspace_id = $1",
            fx.workspace_id
        )
        .await,
        0
    );
    harness.cleanup().await;
}

/// A dead run's task, object key and parked events are undone by the next
/// claim (restart recovery) and, for an exhausted job, by the orphan sweep.
#[tokio::test]
async fn restart_and_sweep_compensate_tasks_objects_and_parked_events() {
    let harness = TestDb::bootstrap().await;
    let fx = fixture(&harness).await;
    let project = create_project(
        fx.app.clone(),
        &fx.cookie,
        fx.workspace_id,
        "REC",
        "workspace",
    )
    .await;
    let project_id: Uuid = project["id"].as_str().unwrap().parse().unwrap();

    for sweep in [false, true] {
        let job_id = notion_import(
            &fx,
            office_fixtures::zip_of(&[("Only.md", b"# only")]),
            None,
        )
        .await;
        let claim = claim_next_import_job(&fx.pool)
            .await
            .unwrap()
            .expect("claim");
        assert_eq!(claim.job_id, job_id);
        let fence = ImportFence {
            job_id,
            lease_token: claim.lease_token,
        };
        // The "dead" run: a page, a task and a stored object, events parked.
        let (page, task_id) = defer_import_events(job_id, async {
            let page = fvoci_server::documents::import_body::create_fenced_wiki_document(
                &fx.pool,
                fx.workspace_id,
                claim.created_by,
                claim.session_id,
                "orphan page",
                None,
                fence,
            )
            .await
            .expect("fenced page");
            let task = create_import_task(
                &fx.pool,
                fx.workspace_id,
                project_id,
                claim.created_by,
                claim.session_id,
                CreateTaskInput {
                    title: "orphan task",
                    task_type: "task",
                    priority: "none",
                    status_id: None,
                    start_date: None,
                    due_date: None,
                    parent_id: None,
                    milestone_id: None,
                    recurrence: None,
                },
                None,
                fence,
            )
            .await
            .unwrap()
            .unwrap()
            .expect("fenced task");
            (page, task)
        })
        .await;
        let (_, key) = fvoci_server::db::attachments::create_import_attachment(
            &fx.pool,
            &StorageQuota::default(),
            fx.workspace_id,
            claim.created_by,
            claim.session_id,
            page,
            "orphan.bin",
            3,
            fence,
        )
        .await
        .unwrap()
        .unwrap()
        .expect("fenced attachment");
        fx.storage.put_bytes(&key, b"abc".to_vec()).await.unwrap();
        assert_eq!(
            events_for(&fx.admin, "task.created", task_id).await,
            0,
            "parked, not published"
        );
        assert_eq!(
            events_for(&fx.admin, "document.created", page).await,
            0,
            "parked, not published"
        );
        assert!(
            count(
                &fx.admin,
                "SELECT count(*) FROM fvoci.import_deferred_events WHERE import_job_id = $1",
                job_id
            )
            .await
                >= 2
        );
        let expire = if sweep {
            // Both attempts used: only the sweep may take it.
            "UPDATE fvoci.import_jobs SET lease_until = now() - interval '1 second', attempts = 2 WHERE id = $1"
        } else {
            "UPDATE fvoci.import_jobs SET lease_until = now() - interval '1 second' WHERE id = $1"
        };
        sqlx::query(expire)
            .bind(job_id)
            .execute(&fx.admin)
            .await
            .unwrap();
        if sweep {
            let swept = sweep_orphan_imports(&fx.pool, &fx.storage, &CancellationToken::new())
                .await
                .unwrap();
            assert_eq!(swept, 1);
            assert_eq!(job_state(&fx.admin, job_id).await.0, "failed");
        } else {
            assert!(fx.run_next().await);
            assert_eq!(job_state(&fx.admin, job_id).await.0, "completed");
        }
        assert_eq!(
            count(
                &fx.admin,
                "SELECT count(*) FROM fvoci.tasks WHERE id = $1",
                task_id
            )
            .await,
            0
        );
        assert_eq!(
            count(
                &fx.admin,
                "SELECT count(*) FROM fvoci.documents WHERE id = $1",
                page
            )
            .await,
            0
        );
        assert_eq!(events_for(&fx.admin, "document.created", page).await, 0);
        assert_eq!(fx.storage.head(&key).await.unwrap(), None);
        assert_eq!(events_for(&fx.admin, "task.created", task_id).await, 0);
        assert_eq!(
            count(
                &fx.admin,
                "SELECT count(*) FROM fvoci.import_deferred_events WHERE import_job_id = $1",
                job_id
            )
            .await,
            0
        );
    }
    harness.cleanup().await;
}

/// The deferral trigger parks events only for a running job of the same
/// workspace; for a failed job the event is dropped with it.
#[tokio::test]
async fn deferral_trigger_parks_for_running_jobs_and_drops_for_failed_ones() {
    let harness = TestDb::bootstrap().await;
    let fx = fixture(&harness).await;
    let job_id = notion_import(&fx, office_fixtures::zip_of(&[("A.md", b"# a")]), None).await;
    let insert_event = |job: Uuid| {
        let pool = fx.pool.clone();
        let workspace_id = fx.workspace_id;
        async move {
            let event_id = Uuid::now_v7();
            let mut tx = pool.begin().await.unwrap();
            sqlx::query("SELECT set_config('app.tenant_id', $1, true)")
                .bind(workspace_id.to_string())
                .execute(&mut *tx)
                .await
                .unwrap();
            sqlx::query("SELECT set_config('app.import_defer_job', $1, true)")
                .bind(job.to_string())
                .execute(&mut *tx)
                .await
                .unwrap();
            sqlx::query(
                "INSERT INTO fvoci.events (id, workspace_id, verb, payload) VALUES ($1, $2, 'test.event', '{}')",
            )
            .bind(event_id)
            .bind(workspace_id)
            .execute(&mut *tx)
            .await
            .unwrap();
            tx.commit().await.unwrap();
            event_id
        }
    };
    let parked = insert_event(job_id).await;
    assert_eq!(
        count(
            &fx.admin,
            "SELECT count(*) FROM fvoci.events WHERE id = $1",
            parked
        )
        .await,
        0
    );
    assert_eq!(
        count(
            &fx.admin,
            "SELECT count(*) FROM fvoci.import_deferred_events WHERE id = $1",
            parked
        )
        .await,
        1
    );
    sqlx::query("UPDATE fvoci.import_jobs SET status = 'failed', payload = NULL WHERE id = $1")
        .bind(job_id)
        .execute(&fx.admin)
        .await
        .unwrap();
    let dropped = insert_event(job_id).await;
    assert_eq!(
        count(
            &fx.admin,
            "SELECT count(*) FROM fvoci.events WHERE id = $1",
            dropped
        )
        .await,
        0
    );
    assert_eq!(
        count(
            &fx.admin,
            "SELECT count(*) FROM fvoci.import_deferred_events WHERE id = $1",
            dropped
        )
        .await,
        0
    );
    // Outside a deferral scope events publish normally.
    let mut tx = fx.pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.tenant_id', $1, true)")
        .bind(fx.workspace_id.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let plain = Uuid::now_v7();
    sqlx::query("INSERT INTO fvoci.events (id, workspace_id, verb, payload) VALUES ($1, $2, 'test.event', '{}')")
        .bind(plain)
        .bind(fx.workspace_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        count(
            &fx.admin,
            "SELECT count(*) FROM fvoci.events WHERE id = $1",
            plain
        )
        .await,
        1
    );
    // Another tenant cannot read the parked row.
    let mut tx = fx.pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.tenant_id', $1, true)")
        .bind(Uuid::now_v7().to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let visible: i64 = sqlx::query_scalar("SELECT count(*) FROM fvoci.import_deferred_events")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(visible, 0);
    tx.rollback().await.unwrap();
    harness.cleanup().await;
}
