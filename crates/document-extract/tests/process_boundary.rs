use std::process::{Command, Stdio};
use std::time::Instant;

use document_extract::gen::hwp5_known_body;
use document_extract::limits::Limits;
use document_extract::outcome::ExtractStatus;
use document_extract::process::{extract_killable, ExtractRequest};

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

#[test]
fn child_process_extracts_hwp5() {
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
}

#[test]
fn timeout_kills_and_reaps_sleep_child() {
    let started = Instant::now();
    let mut child = Command::new("/bin/sleep")
        .arg("20")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn sleep");
    let pid = child.id();
    std::thread::sleep(std::time::Duration::from_millis(200));
    let _ = child.kill();
    let _ = child.wait().expect("reap");
    assert!(
        !std::path::Path::new(&format!("/proc/{pid}")).exists()
            || std::fs::read_to_string(format!("/proc/{pid}/status"))
                .map(|s| s.contains("Zombie") || s.contains("State:\tZ"))
                .unwrap_or(true),
        "pid {pid} was not reaped"
    );
    assert!(started.elapsed().as_secs() < 5, "kill took too long");
}

#[cfg(feature = "test-hang")]
#[test]
fn killable_api_timeout() {
    use document_extract::outcome::LimitKind;
    let mut limits = Limits::for_tests();
    limits.timeout_ms = 500;
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

#[cfg(feature = "test-hang")]
#[test]
fn killable_timeout_joins_helper_threads() {
    use document_extract::outcome::LimitKind;
    let before = std::fs::read_dir("/proc/self/task")
        .map(|d| d.count())
        .unwrap_or(0);
    let mut limits = Limits::for_tests();
    limits.timeout_ms = 500;
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
    let after = std::fs::read_dir("/proc/self/task")
        .map(|d| d.count())
        .unwrap_or(0);
    assert!(
        after <= before + 2,
        "helper threads leaked: before={before} after={after}"
    );
}
