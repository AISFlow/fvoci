#![cfg(all(feature = "db-tests", feature = "extract-native-tests"))]

mod support;

use std::time::Duration;

use document_extract_client::{peek_last_spawn, take_last_spawn};
use support::extract_harness::{
    app_pool, create_document, download_original, extract_job_driver_bin, extract_job_settings,
    hang_extract_job_settings, idle_extract_job_settings, pid_alive, require_extractor_bin,
    run_extract_job_driver, server_bin, setup_session, spawn_extract_for_storage,
    spawn_server_process, upload_bytes, wait_for_extract, TestDb,
};
use uuid::Uuid;

#[test]
fn validate_extractor_bin_rejects_nonexistent_path() {
    let err = fvoci_server::attachments::validate_extractor_bin(std::path::Path::new(
        "/no/such/document-extract",
    ))
    .expect_err("missing path must fail");
    assert!(err.contains("existing file") || err.contains("not found"));
}

#[tokio::test]
async fn server_process_exits_on_invalid_configured_extractor_bin() {
    let harness = TestDb::bootstrap().await;
    let storage_root = std::env::temp_dir().join(format!("fvoci-ext-srv-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&storage_root).expect("storage root");
    let invalid = std::path::PathBuf::from("/no/such/document-extract");
    let mut child = spawn_server_process(&harness, &storage_root, Some(&invalid));
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().expect("wait") {
            break status;
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("fvoci-server did not exit on invalid FVOCI_EXTRACTOR_BIN");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(
        !status.success(),
        "server must exit nonzero with invalid FVOCI_EXTRACTOR_BIN"
    );
    harness.cleanup().await;
}

#[tokio::test]
async fn server_process_starts_when_extractor_env_absent() {
    let harness = TestDb::bootstrap().await;
    let storage_root = std::env::temp_dir().join(format!("fvoci-ext-srv-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&storage_root).expect("storage root");
    let mut child = spawn_server_process(&harness, &storage_root, None);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if child.try_wait().expect("wait").is_some() {
            panic!("fvoci-server exited when FVOCI_EXTRACTOR_BIN was unset");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = child.kill();
    let _ = child.wait();
    harness.cleanup().await;
}

#[tokio::test]
async fn authenticated_hwp_upload_extracts_안녕_and_download_matches() {
    let extractor = require_extractor_bin();
    let fixture = std::fs::read("compat/fixtures/sample.hwp").expect("sample.hwp fixture");
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _owner, workspace_id, storage_root) = setup_session(&harness).await;
    let document_id = create_document(&app, &cookie, workspace_id).await;
    let job = spawn_extract_for_storage(
        &harness,
        &storage_root,
        extract_job_settings(extractor.clone()),
    )
    .await;
    let uploaded = upload_bytes(
        &app,
        &cookie,
        workspace_id,
        &document_id,
        "sample.hwp",
        &fixture,
        Some("application/x-hwp"),
    )
    .await;
    let attachment_id = Uuid::parse_str(&uploaded.attachment_id).unwrap();
    let pool = app_pool(&harness.app_url).await;
    let text = wait_for_extract(&pool, workspace_id, attachment_id, Duration::from_secs(60)).await;
    assert!(
        text.contains("안녕"),
        "expected 안녕 in extracted text, got {:?}",
        text
    );
    let downloaded = download_original(&app, &cookie, workspace_id, &uploaded.attachment_id).await;
    assert_eq!(
        downloaded, fixture,
        "download must be byte-identical to upload"
    );
    job.request_shutdown();
    job.join().await.expect("extract job join");
    pool.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn authenticated_hwpx_upload_extracts_안녕_and_download_matches() {
    let extractor = require_extractor_bin();
    let fixture = std::fs::read("compat/fixtures/sample.hwpx").expect("sample.hwpx fixture");
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _owner, workspace_id, storage_root) = setup_session(&harness).await;
    let document_id = create_document(&app, &cookie, workspace_id).await;
    let job = spawn_extract_for_storage(
        &harness,
        &storage_root,
        extract_job_settings(extractor.clone()),
    )
    .await;
    let uploaded = upload_bytes(
        &app,
        &cookie,
        workspace_id,
        &document_id,
        "sample.hwpx",
        &fixture,
        Some("application/x-hwp"),
    )
    .await;
    let attachment_id = Uuid::parse_str(&uploaded.attachment_id).unwrap();
    let pool = app_pool(&harness.app_url).await;
    let text = wait_for_extract(&pool, workspace_id, attachment_id, Duration::from_secs(60)).await;
    assert!(
        text.contains("안녕"),
        "expected 안녕 in extracted text, got {:?}",
        text
    );
    let downloaded = download_original(&app, &cookie, workspace_id, &uploaded.attachment_id).await;
    assert_eq!(
        downloaded, fixture,
        "download must be byte-identical to upload"
    );
    job.request_shutdown();
    job.join().await.expect("extract job join");
    pool.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn shutdown_before_claim_does_not_burn_attempt() {
    let extractor = require_extractor_bin();
    let fixture = std::fs::read("compat/fixtures/sample.hwp").expect("sample.hwp fixture");
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _owner, workspace_id, storage_root) = setup_session(&harness).await;
    let document_id = create_document(&app, &cookie, workspace_id).await;
    let uploaded = upload_bytes(
        &app,
        &cookie,
        workspace_id,
        &document_id,
        "prestart.hwp",
        &fixture,
        Some("application/x-hwp"),
    )
    .await;
    let attachment_id = Uuid::parse_str(&uploaded.attachment_id).unwrap();
    let job = spawn_extract_for_storage(
        &harness,
        &storage_root,
        idle_extract_job_settings(extractor),
    )
    .await;
    job.request_shutdown();
    tokio::time::timeout(Duration::from_secs(5), job.join())
        .await
        .expect("pre-start shutdown join must complete within 5s")
        .expect("extract job join");
    let pool = app_pool(&harness.app_url).await;
    let state = fvoci_server::db::attachment_extract::fetch_extract_state(
        &pool,
        workspace_id,
        attachment_id,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(state.extract_status, "pending");
    assert_eq!(state.extract_attempts, 0);
    pool.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn shutdown_during_active_parse_reaps_helper_and_releases_lease() {
    let extractor = require_extractor_bin();
    let fixture = std::fs::read("compat/fixtures/sample.hwp").expect("sample.hwp fixture");
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _owner, workspace_id, storage_root) = setup_session(&harness).await;
    let document_id = create_document(&app, &cookie, workspace_id).await;
    let uploaded = upload_bytes(
        &app,
        &cookie,
        workspace_id,
        &document_id,
        "active.hwp",
        &fixture,
        Some("application/x-hwp"),
    )
    .await;
    let attachment_id = Uuid::parse_str(&uploaded.attachment_id).unwrap();
    let _ = take_last_spawn();
    let job = spawn_extract_for_storage(
        &harness,
        &storage_root,
        hang_extract_job_settings(extractor),
    )
    .await;
    let pool = app_pool(&harness.app_url).await;
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let helper_pid = loop {
        let state = fvoci_server::db::attachment_extract::fetch_extract_state(
            &pool,
            workspace_id,
            attachment_id,
        )
        .await
        .unwrap()
        .expect("attachment row");
        let trace = peek_last_spawn();
        if state.lease_token.is_some() && trace.is_some() {
            break trace.expect("spawn trace").pid;
        }
        if std::time::Instant::now() >= deadline {
            panic!(
                "timed out waiting for active parse lease and helper pid; state={:?} trace={:?}",
                state, trace
            );
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    assert!(
        pid_alive(helper_pid),
        "helper must be alive before shutdown"
    );

    job.request_shutdown();
    let joined = tokio::time::timeout(Duration::from_secs(15), job.join())
        .await
        .expect("active-parse shutdown join must complete within 15s")
        .expect("extract job join");
    assert_eq!(joined, ());

    let reap_deadline = std::time::Instant::now() + Duration::from_secs(5);
    while pid_alive(helper_pid) {
        if std::time::Instant::now() >= reap_deadline {
            panic!("helper pid {helper_pid} still alive after shutdown join");
        }
        std::thread::sleep(Duration::from_millis(25));
    }

    let state = fvoci_server::db::attachment_extract::fetch_extract_state(
        &pool,
        workspace_id,
        attachment_id,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(state.extract_status, "pending");
    assert_eq!(state.extract_attempts, 0);
    assert!(state.lease_token.is_none());

    pool.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn fresh_process_recovers_expired_lease_and_completes_extract() {
    let extractor = require_extractor_bin();
    let fixture = std::fs::read("compat/fixtures/sample.hwp").expect("sample.hwp fixture");
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _owner, workspace_id, storage_root) = setup_session(&harness).await;
    let document_id = create_document(&app, &cookie, workspace_id).await;
    let uploaded = upload_bytes(
        &app,
        &cookie,
        workspace_id,
        &document_id,
        "recover.hwp",
        &fixture,
        Some("application/x-hwp"),
    )
    .await;
    let attachment_id = Uuid::parse_str(&uploaded.attachment_id).unwrap();

    let hang_out = run_extract_job_driver(
        &harness,
        &storage_root,
        &extractor,
        "hang_on_claim",
        workspace_id,
        attachment_id,
    );
    assert!(
        hang_out.status.success(),
        "hang_on_claim driver failed: stderr={}",
        String::from_utf8_lossy(&hang_out.stderr)
    );

    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let leased: (bool,) = sqlx::query_as(
        "SELECT extract_lease_token IS NOT NULL FROM fvoci.attachments WHERE workspace_id = $1 AND id = $2",
    )
    .bind(workspace_id)
    .bind(attachment_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert!(leased.0, "crash simulation must leave an active lease");
    sqlx::query(
        "UPDATE fvoci.attachments SET extract_lease_expires_at = now() - interval '1 second' WHERE workspace_id = $1 AND id = $2",
    )
    .bind(workspace_id)
    .bind(attachment_id)
    .execute(&admin)
    .await
    .unwrap();
    admin.close().await;

    let complete_out = run_extract_job_driver(
        &harness,
        &storage_root,
        &extractor,
        "complete_pending",
        workspace_id,
        attachment_id,
    );
    assert!(
        complete_out.status.success(),
        "complete_pending driver failed: stderr={}",
        String::from_utf8_lossy(&complete_out.stderr)
    );

    let pool = app_pool(&harness.app_url).await;
    let state = fvoci_server::db::attachment_extract::fetch_extract_state(
        &pool,
        workspace_id,
        attachment_id,
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        state.extract_text.contains("안녕"),
        "fresh process recovery must extract 안녕, got {:?}",
        state.extract_text
    );
    pool.close().await;
    harness.cleanup().await;
}

#[test]
fn extract_job_driver_binary_is_linked_for_process_tests() {
    assert!(
        extract_job_driver_bin().exists(),
        "extract-job-driver must be built for native lifecycle tests"
    );
    assert!(
        server_bin().exists(),
        "fvoci-server must be built for process tests"
    );
}
