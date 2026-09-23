use std::process::{Command, Stdio};
use std::sync::Mutex;
#[cfg(feature = "test-hang")]
use std::time::Instant;

use document_extract::gen::{hwp5_known_body, zip_with_entry_count};
use document_extract::limits::{Limits, MIN_CHILD_RSS_BYTES};
use document_extract::outcome::{ExtractStatus, LimitKind};
use document_extract::process::{extract_killable, ExtractRequest};

static SPAWN_TEST: Mutex<()> = Mutex::new(());

fn bin() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_document-extract"))
}

fn request(bytes: Vec<u8>, name: &str, limits: Limits) -> ExtractRequest {
    ExtractRequest {
        bytes,
        name: name.into(),
        limits,
        extractor_bin: bin(),
        test_hang_ms: None,
    }
}

fn assert_fully_reaped(pid: u32) {
    let path = format!("/proc/{pid}");
    if !std::path::Path::new(&path).exists() {
        return;
    }
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap_or_default();
    if status.contains("State:\tZ") || status.to_ascii_lowercase().contains("zombie") {
        panic!("pid {pid} is a zombie; kill+reap failed");
    }
    panic!("pid {pid} still exists after extract_killable returned:\n{status}");
}

#[test]
fn child_process_extracts_hwp5() {
    let _g = SPAWN_TEST.lock().expect("spawn test lock");
    let report = extract_killable(request(
        hwp5_known_body(),
        "품의서.hwp",
        Limits::for_tests(),
    ));
    assert!(
        matches!(report.outcome, ExtractStatus::Ok { .. }),
        "{:?}",
        report.outcome
    );
    if let Some(pid) = report.child_pid {
        assert_fully_reaped(pid);
    }
}

#[cfg(feature = "test-hang")]
#[test]
fn extract_killable_timeout_reaps_product_helper() {
    use document_extract::{take_last_spawn, SpawnTrace};
    let _g = SPAWN_TEST.lock().expect("spawn test lock");
    let mut limits = Limits::for_tests();
    limits.timeout_ms = 500;
    let started = Instant::now();
    let report = extract_killable(ExtractRequest {
        bytes: hwp5_known_body(),
        name: "hang.hwp".into(),
        limits,
        extractor_bin: bin(),
        test_hang_ms: Some(20_000),
    });
    assert!(
        matches!(
            report.outcome,
            ExtractStatus::ResourceLimit {
                kind: LimitKind::Time,
                ..
            }
        ),
        "{:?}",
        report.outcome
    );
    let pid = report.child_pid.expect("product helper pid");
    let trace = take_last_spawn().expect("spawn trace");
    assert_eq!(
        trace,
        SpawnTrace {
            pid,
            helpers_joined: 3
        }
    );
    assert_fully_reaped(pid);
    assert!(started.elapsed().as_secs() < 5, "kill took too long");
}

#[test]
fn missing_extractor_path_is_worker_failure() {
    let report = extract_killable(ExtractRequest {
        bytes: hwp5_known_body(),
        name: "x.hwp".into(),
        limits: Limits::for_tests(),
        extractor_bin: std::path::PathBuf::from("/no/such/document-extract"),
        test_hang_ms: None,
    });
    assert!(
        matches!(
            report.outcome,
            ExtractStatus::WorkerFailure {
                reason: document_extract::WorkerFailureReason::MissingExecutable,
                ..
            }
        ),
        "{:?}",
        report.outcome
    );
}

#[test]
fn zero_timeout_is_invalid_limits() {
    let mut limits = Limits::for_tests();
    limits.timeout_ms = 0;
    let report = extract_killable(request(hwp5_known_body(), "x.hwp", limits));
    assert!(
        matches!(
            report.outcome,
            ExtractStatus::WorkerFailure {
                reason: document_extract::WorkerFailureReason::InvalidLimits,
                ..
            }
        ),
        "{:?}",
        report.outcome
    );
}

#[test]
fn killable_forwards_zip_entry_limit() {
    let _g = SPAWN_TEST.lock().expect("spawn test lock");
    let mut limits = Limits::for_tests();
    limits.max_zip_entries = 2;
    let report = extract_killable(request(zip_with_entry_count(4), "many.hwpx", limits));
    assert!(
        matches!(
            report.outcome,
            ExtractStatus::ResourceLimit {
                kind: LimitKind::ZipEntries,
                ..
            }
        ),
        "{:?}",
        report.outcome
    );
    if let Some(pid) = report.child_pid {
        assert_fully_reaped(pid);
    }
}

#[test]
fn undersize_memory_ceiling_is_invalid_limits() {
    let mut limits = Limits::for_tests();
    limits.max_child_rss_bytes = MIN_CHILD_RSS_BYTES - 1;
    let report = extract_killable(request(hwp5_known_body(), "x.hwp", limits));
    assert!(
        matches!(
            report.outcome,
            ExtractStatus::WorkerFailure {
                reason: document_extract::WorkerFailureReason::InvalidLimits,
                ..
            }
        ),
        "{:?}",
        report.outcome
    );
}

#[cfg(feature = "test-hang")]
#[test]
fn child_applies_forwarded_rlimit_as() {
    let rss = 64 * 1024 * 1024;
    let out = Command::new(bin())
        .args([
            "--name",
            "x.hwp",
            "--max-rss",
            &rss.to_string(),
            "--dump-rlimits",
        ])
        .stdin(Stdio::null())
        .output()
        .expect("dump rlimits");
    assert!(
        out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains(&format!("RLIMIT_AS={rss}")),
        "got {stdout:?}"
    );
}

#[cfg(not(feature = "test-hang"))]
#[test]
fn production_bin_rejects_dump_rlimits_flag() {
    let out = Command::new(bin())
        .args(["--dump-rlimits", "--name", "x.hwp"])
        .stdin(Stdio::null())
        .output()
        .expect("run production bin");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("unknown arg"),
        "production build must not honor --dump-rlimits: {stderr}"
    );
}

#[cfg(not(feature = "test-hang"))]
#[test]
fn production_bin_rejects_test_hang_flag() {
    let out = Command::new(bin())
        .args(["--test-hang-ms", "1", "--name", "x.hwp"])
        .stdin(Stdio::null())
        .output()
        .expect("run production bin");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("unknown arg"),
        "production build must not honor --test-hang-ms: {stderr}"
    );
}
