#![cfg(feature = "db-tests")]
#![allow(dead_code)]

#[path = "support/project_harness.rs"]
mod project_harness;

#[path = "support/import_harness.rs"]
mod import_harness;

use std::io::Write;
use std::time::Duration;

use axum::body::Body;
use axum::http::StatusCode;
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use fvoci_server::db::documents::ImportFence;
use fvoci_server::db::import_jobs::{claim_next_import_job, finish_import_job, ImportStatus};
use fvoci_server::documents::import_body::create_fenced_wiki_document;
use fvoci_server::import_job::sweep_orphan_imports;
use import_harness::*;
use project_harness::{add_workspace_user, json_request, TestDb};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

#[tokio::test]
async fn markdown_zip_import_status_reads_completed() {
    let harness = TestDb::bootstrap().await;
    let fx = fixture(&harness).await;
    let (status, body) = fx
        .import(
            &fx.cookie,
            json!({
                "workspaceId": fx.workspace_id,
                "source": "markdown-zip",
                "zipBase64": B64.encode(markdown_zip_bytes())
            }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["status"], "completed");
    let doc_id = body["createdDocumentIds"][0].as_str().unwrap().to_string();

    // B1: the terminal transition is really persisted under FORCE RLS.
    let job = fx
        .job_status(&fx.cookie, body["id"].as_str().unwrap())
        .await;
    assert_eq!(job["status"], "completed", "{job}");

    let (status, meta) = json_request(
        fx.app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{}/documents/{doc_id}", fx.workspace_id),
        None,
        Some(&fx.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(meta["title"], "hello");
    let (status, doc_body) = json_request(
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
    assert_eq!(status, StatusCode::OK);
    assert!(!doc_body["contentJson"]["content"]
        .as_array()
        .unwrap()
        .is_empty());
    harness.cleanup().await;
}

#[tokio::test]
async fn markdown_zip_failure_marks_job_failed_and_returns_import_failed() {
    let harness = TestDb::bootstrap().await;
    let fx = fixture(&harness).await;
    // The third page exceeds DOCUMENT_MAX_BODY_BYTES (1 MiB).
    let huge = "a".repeat(1024 * 1024 + 16);
    let zip = zip_bytes(&[
        ("a.md", b"# one"),
        ("b.md", b"# two"),
        ("c.md", huge.as_bytes()),
    ]);
    let (status, body) = fx
        .import(
            &fx.cookie,
            json!({
                "workspaceId": fx.workspace_id,
                "source": "markdown-zip",
                "zipBase64": B64.encode(zip)
            }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "import_failed");
    let statuses: Vec<(String,)> =
        sqlx::query_as("SELECT status FROM fvoci.import_jobs WHERE workspace_id = $1")
            .bind(fx.workspace_id)
            .fetch_all(&fx.admin)
            .await
            .unwrap();
    assert_eq!(statuses, vec![("failed".to_string(),)]);
    // Source contract: the request-driven markdown path does not compensate.
    let titles: Vec<(String,)> = sqlx::query_as(
        "SELECT title FROM fvoci.documents WHERE workspace_id = $1 AND title IN ('a','b','c') ORDER BY title",
    )
    .bind(fx.workspace_id)
    .fetch_all(&fx.admin)
    .await
    .unwrap();
    assert_eq!(titles, vec![("a".into(),), ("b".into(),), ("c".into(),)]);
    harness.cleanup().await;
}

#[tokio::test]
async fn async_office_import_runs_through_spawned_runner() {
    let harness = TestDb::bootstrap().await;
    let mut fx = fixture_with_runner(&harness, true).await;
    let (status, body) = fx
        .import(
            &fx.cookie,
            json!({
                "workspaceId": fx.workspace_id,
                "source": "office-file",
                "fileName": "회의록.md",
                "zipBase64": B64.encode("# 회의록\n\n본문 내용")
            }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["status"], "running");
    let job_id = body["id"].as_str().unwrap().to_string();
    let mut last = Value::Null;
    for _ in 0..150 {
        last = fx.job_status(&fx.cookie, &job_id).await;
        if last["status"] != "running" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert_eq!(last["status"], "completed", "{last}");
    let doc_id = last["createdDocumentIds"][0].as_str().unwrap();
    let (_, meta) = json_request(
        fx.app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{}/documents/{doc_id}", fx.workspace_id),
        None,
        Some(&fx.cookie),
    )
    .await;
    assert_eq!(meta["title"], "회의록");
    let (status, has_payload, attempts, _) = job_row(&fx.admin, job_id.parse().unwrap()).await;
    assert_eq!(
        (status.as_str(), has_payload, attempts),
        ("completed", false, 1)
    );
    fx.stop_runner().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn async_notion_failure_marks_failed_and_compensates() {
    let harness = TestDb::bootstrap().await;
    let fx = fixture(&harness).await;
    let before = fx.document_count().await;
    let huge = "b".repeat(1024 * 1024 + 16);
    let zip = zip_bytes(&[
        ("Root 0123456789abcdef.md", b"# root"),
        ("Root 0123456789abcdef/Big 89abcdef.md", huge.as_bytes()),
    ]);
    let (status, body) = fx
        .import(
            &fx.cookie,
            json!({
                "workspaceId": fx.workspace_id,
                "source": "notion-zip",
                "zipBase64": B64.encode(zip)
            }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert!(fx.run_next().await);
    let job = fx
        .job_status(&fx.cookie, body["id"].as_str().unwrap())
        .await;
    assert_eq!(job["status"], "failed", "{job}");
    assert_eq!(
        fx.document_count().await,
        before,
        "created pages were compensated"
    );
    let (_, has_payload, _, _) =
        job_row(&fx.admin, body["id"].as_str().unwrap().parse().unwrap()).await;
    assert!(!has_payload, "terminal rows drop the upload");
    harness.cleanup().await;
}

#[tokio::test]
async fn notion_import_builds_nested_hierarchy_with_clean_titles() {
    let harness = TestDb::bootstrap().await;
    let fx = fixture(&harness).await;
    let zip = zip_bytes(&[
        (
            "Export/Root 0123456789abcdef0123456789abcdef/Child aaaaaaaa/Grand bbbbbbbbcccc.md",
            b"# grand",
        ),
        ("Export/Root 0123456789abcdef0123456789abcdef.md", b"# root"),
        (
            "Export/Root 0123456789abcdef0123456789abcdef/Child aaaaaaaa.md",
            b"# child",
        ),
    ]);
    let (status, body) = fx
        .import(
            &fx.cookie,
            json!({
                "workspaceId": fx.workspace_id,
                "source": "notion-zip",
                "zipBase64": B64.encode(zip)
            }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert!(fx.run_next().await);
    let job = fx
        .job_status(&fx.cookie, body["id"].as_str().unwrap())
        .await;
    assert_eq!(job["status"], "completed", "{job}");
    let rows: Vec<(Uuid, String, Option<Uuid>)> = sqlx::query_as(
        "SELECT id, title, parent_id FROM fvoci.documents WHERE workspace_id = $1 AND title IN ('Root','Child','Grand')",
    )
    .bind(fx.workspace_id)
    .fetch_all(&fx.admin)
    .await
    .unwrap();
    let find = |t: &str| rows.iter().find(|r| r.1 == t).unwrap().clone();
    let (root, _, root_parent) = find("Root");
    let (child, _, child_parent) = find("Child");
    let (_, _, grand_parent) = find("Grand");
    assert_eq!(root_parent, None);
    assert_eq!(child_parent, Some(root));
    assert_eq!(grand_parent, Some(child));
    harness.cleanup().await;
}

#[tokio::test]
async fn crashed_run_is_recovered_by_the_next_claim() {
    let harness = TestDb::bootstrap().await;
    let fx = fixture(&harness).await;
    let (status, body) = fx
        .import(
            &fx.cookie,
            json!({
                "workspaceId": fx.workspace_id,
                "source": "office-file",
                "fileName": "note.md",
                "zipBase64": B64.encode("# note")
            }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let job_id: Uuid = body["id"].as_str().unwrap().parse().unwrap();

    // First run: claims, creates one document under the lease, then "dies".
    let claim = claim_next_import_job(&fx.pool)
        .await
        .unwrap()
        .expect("claim");
    assert_eq!(claim.job_id, job_id);
    let orphan = create_fenced_wiki_document(
        &fx.pool,
        fx.workspace_id,
        claim.created_by,
        claim.session_id,
        "orphan",
        None,
        ImportFence {
            job_id,
            lease_token: claim.lease_token,
        },
    )
    .await
    .expect("fenced create");
    let (_, _, _, refs) = job_row(&fx.admin, job_id).await;
    assert_eq!(refs["documentIds"], json!([orphan.to_string()]));
    // A live lease is not stolen by another claim.
    assert!(claim_next_import_job(&fx.pool).await.unwrap().is_none());
    sqlx::query(
        "UPDATE fvoci.import_jobs SET lease_until = now() - interval '1 second' WHERE id = $1",
    )
    .bind(job_id)
    .execute(&fx.admin)
    .await
    .unwrap();

    // Recovery: the next claim compensates the dead run, then completes.
    assert!(fx.run_next().await);
    assert!(!document_exists(&fx.admin, &orphan.to_string()).await);
    let job = fx.job_status(&fx.cookie, &job_id.to_string()).await;
    assert_eq!(job["status"], "completed", "{job}");
    let (_, _, attempts, refs) = job_row(&fx.admin, job_id).await;
    assert_eq!(attempts, 2);
    let docs = refs["documentIds"].as_array().unwrap();
    assert_eq!(docs.len(), 1);
    assert!(document_exists(&fx.admin, docs[0].as_str().unwrap()).await);
    // The dead run's late writes are fenced out.
    assert!(!finish_import_job(&fx.pool, &claim, ImportStatus::Failed)
        .await
        .unwrap());
    harness.cleanup().await;
}

/// A missing seed engine (spawn failure / no seed slot) is capacity
/// pressure: the run compensates, releases its lease and is retried; only
/// the last attempt fails the job.
#[tokio::test]
async fn seed_unavailable_is_retried_then_fails_only_at_the_attempt_limit() {
    use fvoci_server::collab::seed::SeedEngine;
    use fvoci_server::import_job::{run_next_import, ImportJobSettings};

    let harness = TestDb::bootstrap().await;
    let fx = fixture(&harness).await;
    let unavailable = ImportJobSettings {
        seed: Some(SeedEngine::new(
            std::env::temp_dir().join(format!("fvoci-missing-engine-{}", Uuid::now_v7())),
            collab_engine::Limits::for_tests(),
        )),
        ..fx.settings.clone()
    };
    let available = fx.settings.clone();
    let docs_before = fx.document_count().await;
    let import = |name: &'static str| {
        fx.import(
            &fx.cookie,
            json!({
                "workspaceId": fx.workspace_id,
                "source": "office-file",
                "fileName": name,
                "zipBase64": B64.encode("# retry\n\nbody")
            }),
        )
    };
    // Skip the retry backoff (IMPORT_RETRY_BACKOFF_SECS) for released rows.
    let backdate = |job_id: Uuid| {
        let admin = fx.admin.clone();
        async move {
            sqlx::query(
                "UPDATE fvoci.import_jobs SET updated_at = now() - interval '1 hour' \
                 WHERE id = $1 AND lease_token IS NULL",
            )
            .bind(job_id)
            .execute(&admin)
            .await
            .unwrap();
        }
    };
    let run = |settings: &ImportJobSettings| {
        let settings = settings.clone();
        let (pool, storage) = (fx.pool.clone(), fx.storage.clone());
        async move {
            run_next_import(&pool, &settings, &storage, &CancellationToken::new())
                .await
                .unwrap()
        }
    };

    // Engine unavailable on the first attempt, back on the second: completed.
    let (status, body) = import("retry.md").await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let job_id: Uuid = body["id"].as_str().unwrap().parse().unwrap();
    assert!(run(&unavailable).await);
    let (state, has_payload, attempts, refs) = job_row(&fx.admin, job_id).await;
    assert_eq!(
        (state.as_str(), has_payload, attempts),
        ("running", true, 1)
    );
    assert_eq!(refs["documentIds"], json!([]));
    assert_eq!(
        fx.document_count().await,
        docs_before,
        "first run compensated"
    );
    assert!(!run(&available).await, "released row waits out the backoff");
    backdate(job_id).await;
    assert!(run(&available).await);
    let job = fx.job_status(&fx.cookie, &job_id.to_string()).await;
    assert_eq!(job["status"], "completed", "{job}");
    let (_, _, attempts, refs) = job_row(&fx.admin, job_id).await;
    assert_eq!(attempts, 2);
    assert_eq!(refs["documentIds"].as_array().unwrap().len(), 1);
    assert_eq!(fx.document_count().await, docs_before + 1);

    // Still unavailable on the last attempt: failed and compensated.
    let (status, body) = import("exhausted.md").await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let job_id: Uuid = body["id"].as_str().unwrap().parse().unwrap();
    assert!(run(&unavailable).await);
    assert_eq!(job_row(&fx.admin, job_id).await.0, "running");
    backdate(job_id).await;
    assert!(run(&unavailable).await);
    let (state, has_payload, attempts, _) = job_row(&fx.admin, job_id).await;
    assert_eq!(
        (state.as_str(), has_payload, attempts),
        ("failed", false, 2)
    );
    assert_eq!(fx.document_count().await, docs_before + 1);
    assert!(!run(&available).await, "a failed job is not claimed again");
    harness.cleanup().await;
}

#[tokio::test]
async fn expired_lease_is_swept_failed_and_compensated() {
    let harness = TestDb::bootstrap().await;
    let fx = fixture(&harness).await;
    let (_, body) = fx
        .import(
            &fx.cookie,
            json!({
                "workspaceId": fx.workspace_id,
                "source": "office-file",
                "fileName": "note.txt",
                "zipBase64": B64.encode("plain")
            }),
        )
        .await;
    let job_id: Uuid = body["id"].as_str().unwrap().parse().unwrap();
    let claim = claim_next_import_job(&fx.pool)
        .await
        .unwrap()
        .expect("claim");
    let orphan = create_fenced_wiki_document(
        &fx.pool,
        fx.workspace_id,
        claim.created_by,
        claim.session_id,
        "orphan",
        None,
        ImportFence {
            job_id,
            lease_token: claim.lease_token,
        },
    )
    .await
    .unwrap();
    sqlx::query(
        "UPDATE fvoci.import_jobs SET lease_until = now() - interval '1 second' WHERE id = $1",
    )
    .bind(job_id)
    .execute(&fx.admin)
    .await
    .unwrap();

    let swept = sweep_orphan_imports(&fx.pool, &fx.storage, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(swept, 1);
    assert!(!document_exists(&fx.admin, &orphan.to_string()).await);
    let (status, has_payload, _, _) = job_row(&fx.admin, job_id).await;
    assert_eq!((status.as_str(), has_payload), ("failed", false));
    // Idempotent, and the late worker is fenced.
    assert_eq!(
        sweep_orphan_imports(&fx.pool, &fx.storage, &CancellationToken::new())
            .await
            .unwrap(),
        0
    );
    assert!(
        !finish_import_job(&fx.pool, &claim, ImportStatus::Completed)
            .await
            .unwrap()
    );
    let fenced = create_fenced_wiki_document(
        &fx.pool,
        fx.workspace_id,
        claim.created_by,
        claim.session_id,
        "late",
        None,
        ImportFence {
            job_id,
            lease_token: claim.lease_token,
        },
    )
    .await;
    assert!(matches!(
        fenced,
        Err(fvoci_server::documents::import_body::ImportBodyError::Fenced)
    ));
    harness.cleanup().await;
}

#[tokio::test]
async fn demoted_admin_job_fails_at_execution() {
    let harness = TestDb::bootstrap().await;
    let fx = fixture(&harness).await;
    let admin_user = add_workspace_user(&fx.admin, fx.workspace_id, "admin", "importer").await;
    let (status, body) = fx
        .import(
            &admin_user.cookie,
            json!({
                "workspaceId": fx.workspace_id,
                "source": "office-file",
                "fileName": "note.md",
                "zipBase64": B64.encode("# note")
            }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    sqlx::query("UPDATE fvoci.memberships SET role = 'member' WHERE user_id = $1")
        .bind(admin_user.user_id)
        .execute(&fx.admin)
        .await
        .unwrap();
    let before = fx.document_count().await;
    assert!(fx.run_next().await);
    let job = fx
        .job_status(&fx.cookie, body["id"].as_str().unwrap())
        .await;
    assert_eq!(job["status"], "failed", "{job}");
    assert_eq!(fx.document_count().await, before);
    harness.cleanup().await;
}

#[tokio::test]
async fn import_body_contract_auth_first_415_413_and_strict_schema() {
    let harness = TestDb::bootstrap().await;
    let fx = fixture(&harness).await;
    let path = "/api/v1/import";
    // Unauthenticated: 401 before the (oversized) body is read.
    let (status, _, _) = raw_request(
        fx.app.clone(),
        "POST",
        path,
        None,
        Some("application/json"),
        Body::from(vec![b'x'; 4 * 1024 * 1024]),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, bytes, _) = raw_request(
        fx.app.clone(),
        "POST",
        path,
        Some(&fx.cookie),
        Some("text/plain"),
        Body::from("{}"),
    )
    .await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(
        serde_json::from_slice::<Value>(&bytes).unwrap()["code"],
        "unsupported_media_type"
    );
    let limit = fvoci_server::http::routes::import::IMPORT_BODY_MAX_BYTES;
    let (status, _, _) = raw_request(
        fx.app.clone(),
        "POST",
        path,
        Some(&fx.cookie),
        Some("application/json"),
        Body::from(vec![b' '; limit + 1]),
    )
    .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    let (status, body) = fx
        .import(
            &fx.cookie,
            json!({"workspaceId": fx.workspace_id, "source": "markdown-zip", "zipBase64": "AA==", "extra": 1}),
        )
        .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::BAD_REQUEST, Some("invalid_input"))
    );
    let (status, _) = fx
        .import(
            &fx.cookie,
            json!({"workspaceId": fx.workspace_id, "source": "office-file", "zipBase64": "AA==", "fileName": "x".repeat(256)}),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // Unsupported office formats fail up front instead of spinning.
    let (status, body) = fx
        .import(
            &fx.cookie,
            json!({"workspaceId": fx.workspace_id, "source": "office-file", "zipBase64": "AA==", "fileName": "deck.key"}),
        )
        .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::BAD_REQUEST, Some("import_failed"))
    );
    harness.cleanup().await;
}

#[tokio::test]
async fn upload_over_axum_default_limit_is_accepted() {
    let harness = TestDb::bootstrap().await;
    let fx = fixture(&harness).await;
    // ~3 MiB of incompressible-ish base64 in a stored entry: well above
    // axum's 2 MiB Json default, well below the 64 MiB import cap.
    let mut noise = Vec::with_capacity(3 * 1024 * 1024);
    let mut x: u32 = 0x1234_5678;
    while noise.len() < 3 * 1024 * 1024 {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        noise.extend_from_slice(&x.to_le_bytes());
    }
    let zip = {
        let mut buf = Vec::new();
        let mut zip = ZipWriter::new(std::io::Cursor::new(&mut buf));
        let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        zip.start_file("note.md", stored).unwrap();
        zip.write_all(b"# big upload").unwrap();
        zip.start_file("blob.bin", stored).unwrap();
        zip.write_all(&noise).unwrap();
        zip.finish().unwrap();
        buf
    };
    let (status, body) = fx
        .import(
            &fx.cookie,
            json!({
                "workspaceId": fx.workspace_id,
                "source": "markdown-zip",
                "zipBase64": B64.encode(zip)
            }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["createdDocumentIds"].as_array().unwrap().len(), 1);
    harness.cleanup().await;
}

/// One deflate entry inflating to `chunks` x 8 MiB of zeros. Compressing
/// 1 GiB in a debug build is slow, so one sync-flushed 8 MiB block sequence is
/// repeated (each copy only references zeros in the window) and closed with
/// an empty final block. CRC is left 0: the reader, like the source, does not
/// verify it, and the entry must be rejected before its end anyway.
fn zero_bomb_zip(chunks: usize) -> Vec<u8> {
    use flate2::{Compress, Compression, FlushCompress};
    let zeros = vec![0u8; 8 * 1024 * 1024];
    let mut block = Vec::with_capacity(64 * 1024);
    let mut compress = Compress::new(Compression::best(), false);
    compress
        .compress_vec(&zeros, &mut block, FlushCompress::Sync)
        .unwrap();
    assert_eq!(compress.total_in() as usize, zeros.len());
    let mut stream = Vec::with_capacity(block.len() * chunks + 2);
    for _ in 0..chunks {
        stream.extend_from_slice(&block);
    }
    stream.extend_from_slice(&[0x03, 0x00]);
    let size = (zeros.len() * chunks) as u32;
    let name = b"bomb.md";
    let mut out = Vec::new();
    out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
    out.extend_from_slice(&20u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&8u16.to_le_bytes());
    out.extend_from_slice(&[0; 8]);
    out.extend_from_slice(&(stream.len() as u32).to_le_bytes());
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&(name.len() as u16).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(name);
    out.extend_from_slice(&stream);
    let central_at = out.len() as u32;
    out.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
    out.extend_from_slice(&20u16.to_le_bytes());
    out.extend_from_slice(&20u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&8u16.to_le_bytes());
    out.extend_from_slice(&[0; 8]);
    out.extend_from_slice(&(stream.len() as u32).to_le_bytes());
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&(name.len() as u16).to_le_bytes());
    out.extend_from_slice(&[0; 12]);
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(name);
    let central_len = out.len() as u32 - central_at;
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&central_len.to_le_bytes());
    out.extend_from_slice(&central_at.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

#[test]
fn zero_bomb_fixture_really_inflates_past_the_budget() {
    // Sanity check of the fixture itself with a small copy count.
    use std::io::Read;
    let zip = zero_bomb_zip(2);
    let start = 30 + "bomb.md".len();
    let compressed = u32::from_le_bytes(zip[18..22].try_into().unwrap()) as usize;
    let mut out = Vec::new();
    flate2::read::DeflateDecoder::new(&zip[start..start + compressed])
        .read_to_end(&mut out)
        .unwrap();
    assert_eq!(out.len(), 16 * 1024 * 1024);
    assert!(out.iter().all(|b| *b == 0));
}

fn vm_hwm_kib() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmHWM:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|v| v.parse().ok())
        })
        .unwrap_or(0)
}

#[tokio::test]
async fn real_zip_bomb_is_rejected_within_the_inflate_budget() {
    let harness = TestDb::bootstrap().await;
    let fx = fixture(&harness).await;
    // 1 GiB of zeros as one deflate entry of about 1 MiB (see `zero_bomb_zip`).
    let bomb = zero_bomb_zip(128);
    // The rejection is the inflate budget, not a malformed archive.
    assert_eq!(
        fvoci_server::documents::import_zip::unzip_bounded(&bomb).unwrap_err(),
        fvoci_server::documents::import_zip::ZipImportError::TooLarge
    );
    assert!(bomb.len() < 8 * 1024 * 1024, "bomb is {} bytes", bomb.len());
    let before = vm_hwm_kib();
    let (status, body) = fx
        .import(
            &fx.cookie,
            json!({
                "workspaceId": fx.workspace_id,
                "source": "markdown-zip",
                "zipBase64": B64.encode(&bomb)
            }),
        )
        .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::BAD_REQUEST, Some("import_failed"))
    );
    let grown_mib = vm_hwm_kib().saturating_sub(before) / 1024;
    // The inflate budget is 200 MiB; inflating the whole entry would be 1 GiB.
    assert!(grown_mib < 600, "peak RSS grew by {grown_mib} MiB");
    harness.cleanup().await;
}

#[tokio::test]
async fn document_over_128_kib_imports_and_exports() {
    let harness = TestDb::bootstrap().await;
    let fx = fixture(&harness).await;
    let paragraph = "한글 문단과 English text 가 섞인 긴 본문입니다. ".repeat(40);
    let markdown = (0..100)
        .map(|i| format!("## 절 {i}\n\n{paragraph}\n"))
        .collect::<String>();
    assert!(markdown.len() > 200 * 1024 && markdown.len() < 1024 * 1024);
    let (status, body) = fx
        .import(
            &fx.cookie,
            json!({
                "workspaceId": fx.workspace_id,
                "source": "markdown-zip",
                "zipBase64": B64.encode(zip_bytes(&[("회의록.md", markdown.as_bytes())]))
            }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let doc_id = body["createdDocumentIds"][0].as_str().unwrap().to_string();
    for (format, content_type) in [
        ("md", "text/markdown"),
        ("pdf", "application/pdf"),
        (
            "docx",
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        ),
    ] {
        let (status, bytes, headers) = raw_request(
            fx.app.clone(),
            "GET",
            &format!(
                "/api/v1/workspaces/{}/documents/{doc_id}/{format}",
                fx.workspace_id
            ),
            Some(&fx.cookie),
            None,
            Body::empty(),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "{format}: {}",
            String::from_utf8_lossy(&bytes)
        );
        let ct = headers["content-type"].to_str().unwrap();
        assert!(ct.starts_with(content_type), "{format}: {ct}");
        let disposition = headers["content-disposition"].to_str().unwrap();
        assert!(
            disposition.contains(&format!(
                "filename*=UTF-8''%ED%9A%8C%EC%9D%98%EB%A1%9D.{format}"
            )),
            "{disposition}"
        );
        assert!(disposition.is_ascii());
        if format == "md" {
            let text = String::from_utf8(bytes).unwrap();
            assert!(text.contains("절 99"));
        } else {
            assert!(bytes.len() > 1000);
        }
    }
    harness.cleanup().await;
}

#[tokio::test]
async fn project_document_export_route() {
    let harness = TestDb::bootstrap().await;
    let fx = fixture(&harness).await;
    let project = project_harness::create_project(
        fx.app.clone(),
        &fx.cookie,
        fx.workspace_id,
        "PRJ",
        "workspace",
    )
    .await;
    let project_id = project["id"].as_str().unwrap();
    let root_id = project["rootDocumentId"].as_str().unwrap();
    let (status, doc) = json_request(
        fx.app.clone(),
        "POST",
        &format!(
            "/api/v1/workspaces/{}/projects/{project_id}/documents",
            fx.workspace_id
        ),
        Some(json!({"title": "Spec", "parentId": root_id})),
        Some(&fx.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{doc}");
    let doc_id = doc["id"].as_str().unwrap();
    let (status, bytes, _) = raw_request(
        fx.app.clone(),
        "GET",
        &format!(
            "/api/v1/workspaces/{}/projects/{project_id}/documents/{doc_id}/md",
            fx.workspace_id
        ),
        Some(&fx.cookie),
        None,
        Body::empty(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    assert!(String::from_utf8(bytes).unwrap().starts_with("# Spec"));
    // The workspace route does not serve project documents.
    let (status, _, _) = raw_request(
        fx.app.clone(),
        "GET",
        &format!(
            "/api/v1/workspaces/{}/documents/{doc_id}/md",
            fx.workspace_id
        ),
        Some(&fx.cookie),
        None,
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    harness.cleanup().await;
}

#[tokio::test]
async fn import_rejects_bearer_token() {
    let harness = TestDb::bootstrap().await;
    let fx = fixture(&harness).await;
    let (status, created) = json_request(
        fx.app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{}/api-tokens", fx.workspace_id),
        Some(json!({ "name": "import", "scopes": ["documents.read"] })),
        Some(&fx.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let secret = created["token"].as_str().unwrap();
    let (status, body, _) = project_harness::http_request(
        fx.app.clone(),
        "POST",
        "/api/v1/import",
        Some(
            json!({
                "workspaceId": fx.workspace_id,
                "source": "markdown-zip",
                "zipBase64": B64.encode(markdown_zip_bytes())
            })
            .to_string()
            .into_bytes(),
        ),
        Some("application/json"),
        None,
        &[("authorization", &format!("Bearer {secret}"))],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");
    let _ = fx.user_id;
    harness.cleanup().await;
}
