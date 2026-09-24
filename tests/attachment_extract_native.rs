#![cfg(all(feature = "db-tests", feature = "extract-native-tests"))]

mod support;

use std::path::PathBuf;
use std::time::Duration;

use support::extract_harness::{
    app_pool, create_document, download_original, extract_job_settings, idle_extract_job_settings,
    require_extractor_bin, setup_session, spawn_extract_for_storage, upload_bytes, wait_for_extract,
    TestDb,
};
use uuid::Uuid;

#[tokio::test]
async fn startup_rejects_missing_extractor_bin_env() {
    let err = fvoci_server::attachments::validate_extractor_bin(
        std::path::Path::new("/no/such/document-extract"),
    )
        .expect_err("missing path must fail");
    assert!(err.contains("existing file") || err.contains("not found"));
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
    assert_eq!(downloaded, fixture, "download must be byte-identical to upload");
    job.request_shutdown();
    job.join().await;
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
    assert_eq!(downloaded, fixture, "download must be byte-identical to upload");
    job.request_shutdown();
    job.join().await;
    pool.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn graceful_shutdown_while_pending_does_not_burn_attempt() {
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
        "shutdown.hwp",
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
    job.join().await;
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
async fn restart_job_resumes_durable_pending_without_memory() {
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
        "restart.hwp",
        &fixture,
        Some("application/x-hwp"),
    )
    .await;
    let attachment_id = Uuid::parse_str(&uploaded.attachment_id).unwrap();
    let first = spawn_extract_for_storage(
        &harness,
        &storage_root,
        idle_extract_job_settings(extractor.clone()),
    )
    .await;
    first.request_shutdown();
    first.join().await;
    let second = spawn_extract_for_storage(
        &harness,
        &storage_root,
        extract_job_settings(extractor),
    )
    .await;
    let pool = app_pool(&harness.app_url).await;
    let text = wait_for_extract(&pool, workspace_id, attachment_id, Duration::from_secs(60)).await;
    assert!(text.contains("안녕"), "restart must resume pending extract");
    second.request_shutdown();
    second.join().await;
    pool.close().await;
    harness.cleanup().await;
}
