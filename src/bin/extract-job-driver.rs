//! Subprocess driver for process-boundary extract job lifecycle tests.
//!
//! Modes (EXTRACT_JOB_DRIVER_MODE):
//! - `claim_crash_recovery`: spawn job with hang helper, wait until lease is held, exit without
//!   shutdown (simulates owner crash after claim; helper may not have spawned yet).
//! - `shutdown_active_parse`: spawn hang job, wait for lease and helper pid, shutdown cleanly,
//!   and verify helper reap plus lease release (isolated from parallel LAST_SPAWN users).
//! - `complete_pending`: spawn job, wait until attachment extract finishes, shutdown cleanly.

use std::path::PathBuf;
use std::time::Duration;

use document_extract_client::{peek_last_spawn, take_last_spawn};
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

fn pid_alive(pid: u32) -> bool {
    std::path::Path::new(&format!("/proc/{pid}")).exists()
}

fn hang_settings(extractor_bin: PathBuf) -> ExtractJobSettings {
    ExtractJobSettings {
        extractor_bin,
        limits: document_extract_client::Limits::for_tests(),
        poll_interval: Duration::from_millis(50),
        retry_backoff: Duration::from_millis(50),
        test_hang_ms: Some(20_000),
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
        "claim_crash_recovery" => {
            let _job = spawn_extract_job(
                hang_settings(extractor_bin),
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
        "shutdown_active_parse" => {
            let _ = take_last_spawn();
            let job = spawn_extract_job(
                hang_settings(extractor_bin),
                pool.clone(),
                LocalStorage::new(PathBuf::from(storage_root)),
            );
            let deadline = std::time::Instant::now() + Duration::from_secs(30);
            let helper_pid = loop {
                let state = fetch_extract_state(&pool, workspace_id, attachment_id)
                    .await
                    .map_err(|e| format!("fetch state failed: {e}"))?
                    .ok_or_else(|| "attachment row missing".to_string())?;
                let trace = peek_last_spawn();
                if let (Some(_), Some(trace)) = (state.lease_token.as_ref(), trace) {
                    break trace.pid;
                }
                if std::time::Instant::now() >= deadline {
                    return Err(format!(
                        "timed out waiting for active parse lease and helper pid; state={state:?} trace={trace:?}"
                    ));
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            };
            if !pid_alive(helper_pid) {
                return Err(format!("helper pid {helper_pid} not alive before shutdown"));
            }

            job.request_shutdown();
            tokio::time::timeout(Duration::from_secs(15), job.join())
                .await
                .map_err(|_| "active-parse shutdown join timed out after 15s".to_string())?
                .map_err(|e| format!("extract job join failed: {e}"))?;

            let reap_deadline = std::time::Instant::now() + Duration::from_secs(5);
            while pid_alive(helper_pid) {
                if std::time::Instant::now() >= reap_deadline {
                    return Err(format!(
                        "helper pid {helper_pid} still alive after shutdown join"
                    ));
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }

            let state = fetch_extract_state(&pool, workspace_id, attachment_id)
                .await
                .map_err(|e| format!("fetch state failed: {e}"))?
                .ok_or_else(|| "attachment row missing".to_string())?;
            if state.extract_status != "pending" {
                return Err(format!(
                    "expected pending after shutdown, got {:?}",
                    state.extract_status
                ));
            }
            if state.extract_attempts != 0 {
                return Err(format!(
                    "expected zero attempts after shutdown, got {}",
                    state.extract_attempts
                ));
            }
            if state.lease_token.is_some() {
                return Err("lease token must be released after shutdown".into());
            }
            pool.close().await;
            Ok(())
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
