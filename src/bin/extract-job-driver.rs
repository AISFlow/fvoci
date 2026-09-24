//! Subprocess driver for process-boundary extract job lifecycle tests.
//!
//! Modes (EXTRACT_JOB_DRIVER_MODE):
//! - `hang_on_claim`: spawn job with hang helper, wait until lease is held, exit without shutdown.
//! - `complete_pending`: spawn job, wait until attachment extract finishes, shutdown cleanly.

use std::path::PathBuf;
use std::time::Duration;

use fvoci_server::attachments::{spawn_extract_job, ExtractJobSettings, LocalStorage};
use fvoci_server::db::attachment_extract::fetch_extract_state;
use fvoci_server::db::pool;
use uuid::Uuid;

#[tokio::main]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("extract-job-driver failed: {err}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    let mode = std::env::var("EXTRACT_JOB_DRIVER_MODE")
        .map_err(|_| "EXTRACT_JOB_DRIVER_MODE is required".to_string())?;
    let app_url = std::env::var("DATABASE_APP_URL")
        .or_else(|_| std::env::var("FVOCI_APP_DATABASE_URL"))
        .map_err(|_| "DATABASE_APP_URL is required".to_string())?;
    let storage_root = std::env::var("FVOCI_STORAGE_DIR")
        .map_err(|_| "FVOCI_STORAGE_DIR is required".to_string())?;
    let extractor_raw = std::env::var("FVOCI_EXTRACTOR_BIN")
        .map_err(|_| "FVOCI_EXTRACTOR_BIN is required".to_string())?;
    let extractor_bin = PathBuf::from(extractor_raw.trim());
    fvoci_server::attachments::validate_extractor_bin(&extractor_bin)?;

    let workspace_id = Uuid::parse_str(
        &std::env::var("EXTRACT_JOB_WORKSPACE_ID")
            .map_err(|_| "EXTRACT_JOB_WORKSPACE_ID is required".to_string())?,
    )
    .map_err(|e| format!("invalid EXTRACT_JOB_WORKSPACE_ID: {e}"))?;
    let attachment_id = Uuid::parse_str(
        &std::env::var("EXTRACT_JOB_ATTACHMENT_ID")
            .map_err(|_| "EXTRACT_JOB_ATTACHMENT_ID is required".to_string())?,
    )
    .map_err(|e| format!("invalid EXTRACT_JOB_ATTACHMENT_ID: {e}"))?;

    let pool = pool::connect_app(&app_url)
        .await
        .map_err(|e| format!("app pool connect failed: {e}"))?;

    match mode.as_str() {
        "hang_on_claim" => {
            let settings = ExtractJobSettings {
                extractor_bin,
                limits: document_extract_client::Limits::for_tests(),
                poll_interval: Duration::from_millis(50),
                retry_backoff: Duration::from_millis(50),
                test_hang_ms: Some(20_000),
            };
            let _job = spawn_extract_job(
                settings,
                pool.clone(),
                LocalStorage::new(PathBuf::from(storage_root)),
            );
            let deadline = std::time::Instant::now() + Duration::from_secs(30);
            while std::time::Instant::now() < deadline {
                let state = fetch_extract_state(&pool, workspace_id, attachment_id)
                    .await
                    .map_err(|e| format!("fetch state failed: {e}"))?
                    .ok_or_else(|| "attachment row missing".to_string())?;
                if state.lease_token.is_some() {
                    std::process::exit(0);
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            Err("timed out waiting for extract lease".into())
        }
        "complete_pending" => {
            let settings = ExtractJobSettings {
                extractor_bin,
                limits: document_extract_client::Limits::for_tests(),
                poll_interval: Duration::from_millis(100),
                retry_backoff: Duration::from_millis(100),
                test_hang_ms: None,
            };
            let job = spawn_extract_job(
                settings,
                pool.clone(),
                LocalStorage::new(PathBuf::from(storage_root)),
            );
            let deadline = std::time::Instant::now() + Duration::from_secs(120);
            while std::time::Instant::now() < deadline {
                let state = fetch_extract_state(&pool, workspace_id, attachment_id)
                    .await
                    .map_err(|e| format!("fetch state failed: {e}"))?
                    .ok_or_else(|| "attachment row missing".to_string())?;
                if state.extract_status != "pending" {
                    job.request_shutdown();
                    job.join()
                        .await
                        .map_err(|e| format!("extract job join failed: {e}"))?;
                    pool.close().await;
                    return Ok(());
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err("timed out waiting for extract completion".into())
        }
        other => Err(format!("unknown EXTRACT_JOB_DRIVER_MODE: {other}")),
    }
}
