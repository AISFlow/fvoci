#[cfg(feature = "test-hang")]
use std::process::Child;
use std::process::{Command, Stdio};
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;
#[cfg(feature = "test-hang")]
use std::sync::{mpsc, Arc};
#[cfg(feature = "test-hang")]
use std::time::{Duration, Instant};

use document_extract::gen::{hwp5_known_body, zip_with_entry_count};
use document_extract::limits::{Limits, MIN_CHILD_RSS_BYTES};
use document_extract::outcome::{ExtractStatus, LimitKind};
use document_extract::process::{
    extract_killable, extract_killable_with_cancel, Cancelled, ExtractRequest,
};

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

#[cfg(feature = "test-hang")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PidView {
    Gone,
    Zombie,
    Live,
}

#[cfg(feature = "test-hang")]
fn proc_starttime(pid: u32) -> Option<u64> {
    let raw = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rest = raw.rsplit_once(')')?.1;
    rest.split_whitespace().nth(19)?.parse().ok()
}

#[cfg(feature = "test-hang")]
fn observe_pid(pid: u32, starttime: Option<u64>) -> PidView {
    if let Some(expected) = starttime {
        match proc_starttime(pid) {
            None => return PidView::Gone,
            Some(got) if got != expected => return PidView::Gone,
            Some(_) => {}
        }
    } else if !std::path::Path::new(&format!("/proc/{pid}")).exists() {
        return PidView::Gone;
    }
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap_or_default();
    let zombie = status.lines().any(|line| {
        line.strip_prefix("State:")
            .map(|rest| rest.trim().starts_with('Z'))
            .unwrap_or(false)
    }) || status.to_ascii_lowercase().contains("zombie");
    if zombie {
        return PidView::Zombie;
    }
    if !std::path::Path::new(&format!("/proc/{pid}")).exists() {
        return PidView::Gone;
    }
    PidView::Live
}

#[cfg(feature = "test-hang")]
fn helper_ppid(pid: u32) -> Option<u32> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("PPid:") {
            return rest.split_whitespace().next()?.parse().ok();
        }
    }
    None
}

#[cfg(feature = "test-hang")]
fn helper_exe(pid: u32) -> Option<std::path::PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    let cmd = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let exe = cmd.split(|b| *b == 0).next().unwrap_or(&cmd);
    Some(std::path::PathBuf::from(std::ffi::OsStr::from_bytes(exe)))
}

#[cfg(feature = "test-hang")]
fn is_helper_for_parent(pid: u32, parent: u32, extractor: &std::path::Path) -> bool {
    if helper_ppid(pid) != Some(parent) {
        return false;
    }
    let Some(exe) = helper_exe(pid) else {
        return false;
    };
    exe == extractor || exe.file_name() == extractor.file_name()
}

#[cfg(feature = "test-hang")]
fn extractor_children(extractor: &std::path::Path) -> Vec<(u32, u64)> {
    let me = std::process::id();
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return out;
    };
    for ent in entries.flatten() {
        let pid: u32 = match ent.file_name().to_str().and_then(|s| s.parse().ok()) {
            Some(pid) => pid,
            None => continue,
        };
        if !is_helper_for_parent(pid, me, extractor) {
            continue;
        }
        if let Some(start) = proc_starttime(pid) {
            out.push((pid, start));
        }
    }
    out
}

#[cfg(feature = "test-hang")]
fn wait_owned_helper(
    extractor: &std::path::Path,
    preexisting: &[(u32, u64)],
    deadline: Instant,
) -> u32 {
    use document_extract::process::peek_last_spawn;
    let me = std::process::id();
    let mut found = None;
    wait_until(
        deadline,
        || {
            let Some(trace) = peek_last_spawn() else {
                return false;
            };
            if !is_helper_for_parent(trace.pid, me, extractor) {
                return false;
            }
            let start = proc_starttime(trace.pid);
            if preexisting
                .iter()
                .any(|(pid, st)| *pid == trace.pid && start == Some(*st))
            {
                return false;
            }
            if observe_pid(trace.pid, start) != PidView::Live {
                return false;
            }
            found = Some(trace.pid);
            true
        },
        "owned extractor helper spawn",
    );
    found.expect("owned helper pid")
}

#[cfg(feature = "test-hang")]
fn wait_until(deadline: Instant, mut pred: impl FnMut() -> bool, what: &str) {
    loop {
        if pred() {
            return;
        }
        if Instant::now() >= deadline {
            panic!("timed out waiting for {what}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(feature = "test-hang")]
fn recv_bounded<T>(rx: mpsc::Receiver<T>, timeout: Duration, what: &str) -> T {
    rx.recv_timeout(timeout)
        .unwrap_or_else(|err| panic!("timed out waiting for {what}: {err}"))
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

#[test]
fn helper_stdin_is_capped_at_max_input_plus_one() {
    let _g = SPAWN_TEST.lock().unwrap();
    let oversized = vec![b'X'; 64];
    let mut child = Command::new(bin())
        .args(["--name", "x.hwp", "--max-input", "16"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    {
        use std::io::Write;
        let mut stdin = child.stdin.take().expect("stdin");
        let _ = stdin.write_all(&oversized);
    }
    let finished = child.wait_with_output().expect("wait");
    assert!(
        finished.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&finished.stderr)
    );
    let stdout = String::from_utf8_lossy(&finished.stdout);
    assert!(stdout.contains("\"status\":\"resource_limit\""), "{stdout}");
    assert!(stdout.contains("\"kind\":\"input\""), "{stdout}");
}

#[test]
fn cancel_before_spawn_is_cancelled_not_success() {
    let cancel = AtomicBool::new(true);
    let result = extract_killable_with_cancel(
        request(hwp5_known_body(), "품의서.hwp", Limits::for_tests()),
        &cancel,
    );
    match result {
        Err(Cancelled { child_pid: None }) => {}
        Ok(report) => panic!(
            "cancel must not report extraction success/empty: {:?}",
            report.outcome
        ),
        Err(other) => panic!("expected Cancelled without child, got {other:?}"),
    }
}

#[cfg(feature = "test-hang")]
#[test]
fn cancel_while_waiting_slot_then_next_extract_works() {
    use document_extract::process::{is_slot_waiter, peek_last_spawn, take_last_spawn, SpawnTrace};
    let _g = SPAWN_TEST.lock().expect("spawn test lock");
    let _ = take_last_spawn();
    let extractor = bin();
    let preexisting = extractor_children(&extractor);

    let holder_cancel = Arc::new(AtomicBool::new(false));
    let mut hang_limits = Limits::for_tests();
    hang_limits.timeout_ms = 8_000;
    let holder_req = ExtractRequest {
        bytes: hwp5_known_body(),
        name: "hang.hwp".into(),
        limits: hang_limits,
        extractor_bin: extractor.clone(),
        test_hang_ms: Some(20_000),
    };
    let holder_flag = Arc::clone(&holder_cancel);
    let (holder_tx, holder_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = holder_tx.send(extract_killable_with_cancel(
            holder_req,
            holder_flag.as_ref(),
        ));
    });
    let holder_pid = wait_owned_helper(
        &extractor,
        &preexisting,
        Instant::now() + Duration::from_secs(5),
    );

    let waiter_cancel = Arc::new(AtomicBool::new(false));
    let waiter_flag = Arc::clone(&waiter_cancel);
    let (waiter_tx, waiter_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = waiter_tx.send(extract_killable_with_cancel(
            request(hwp5_known_body(), "품의서.hwp", Limits::for_tests()),
            waiter_flag.as_ref(),
        ));
    });
    wait_until(
        Instant::now() + Duration::from_secs(2),
        || is_slot_waiter(waiter_cancel.as_ref()),
        "this request waiting for extract child slot",
    );
    waiter_cancel.store(true, std::sync::atomic::Ordering::Release);
    match recv_bounded(waiter_rx, Duration::from_secs(2), "slot-wait cancel") {
        Err(Cancelled { child_pid: None }) => {}
        Ok(report) => panic!(
            "slot-wait cancel must not report extraction: {:?}",
            report.outcome
        ),
        other => panic!("expected Cancelled without child, got {other:?}"),
    }
    assert_eq!(peek_last_spawn().map(|t| t.pid), Some(holder_pid));
    assert!(
        is_helper_for_parent(holder_pid, std::process::id(), &extractor),
        "holder helper pid {holder_pid} is not this test's extractor child"
    );

    holder_cancel.store(true, std::sync::atomic::Ordering::Release);
    match recv_bounded(holder_rx, Duration::from_secs(5), "holder cancel") {
        Err(Cancelled {
            child_pid: Some(pid),
        }) => assert_eq!(pid, holder_pid),
        other => panic!("expected Cancelled holder helper, got {other:?}"),
    }
    let trace = take_last_spawn().expect("holder spawn trace");
    assert_eq!(
        trace,
        SpawnTrace {
            pid: holder_pid,
            helpers_joined: 3
        }
    );
    assert_fully_reaped(holder_pid);

    let next = extract_killable(request(
        hwp5_known_body(),
        "품의서.hwp",
        Limits::for_tests(),
    ));
    assert!(
        matches!(next.outcome, ExtractStatus::Ok { .. }),
        "{:?}",
        next.outcome
    );
    if let Some(pid) = next.child_pid {
        assert_fully_reaped(pid);
    }
}

#[cfg(feature = "test-hang")]
#[test]
fn cancel_running_hanging_helper_reaps_and_joins() {
    use document_extract::process::{take_last_spawn, SpawnTrace};
    let _g = SPAWN_TEST.lock().expect("spawn test lock");
    let _ = take_last_spawn();
    let extractor = bin();
    let preexisting = extractor_children(&extractor);
    let cancel = Arc::new(AtomicBool::new(false));
    let mut limits = Limits::for_tests();
    limits.timeout_ms = 8_000;
    let req = ExtractRequest {
        bytes: hwp5_known_body(),
        name: "hang.hwp".into(),
        limits,
        extractor_bin: extractor.clone(),
        test_hang_ms: Some(20_000),
    };
    let flag = Arc::clone(&cancel);
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(extract_killable_with_cancel(req, flag.as_ref()));
    });
    let pid = wait_owned_helper(
        &extractor,
        &preexisting,
        Instant::now() + Duration::from_secs(5),
    );
    cancel.store(true, std::sync::atomic::Ordering::Release);
    match recv_bounded(rx, Duration::from_secs(5), "running cancel") {
        Err(Cancelled {
            child_pid: Some(got),
        }) => assert_eq!(got, pid),
        Ok(report) => panic!(
            "running cancel must not report extraction: {:?}",
            report.outcome
        ),
        other => panic!("expected Cancelled with helper pid, got {other:?}"),
    }
    let trace = take_last_spawn().expect("spawn trace");
    assert_eq!(
        trace,
        SpawnTrace {
            pid,
            helpers_joined: 3
        }
    );
    assert_fully_reaped(pid);

    let next = extract_killable(request(
        hwp5_known_body(),
        "품의서.hwp",
        Limits::for_tests(),
    ));
    assert!(
        matches!(next.outcome, ExtractStatus::Ok { .. }),
        "{:?}",
        next.outcome
    );
    if let Some(pid) = next.child_pid {
        assert_fully_reaped(pid);
    }
}

/// Parent SIGKILL must terminate the hanging helper via PDEATHSIG without this
/// client's watchdog. The dead parent cannot reap: Gone or Zombie both count
/// as terminated. Live is failure. Cleanup may SIGKILL only this owned pair.
#[cfg(feature = "test-hang")]
#[test]
fn parent_sigkill_terminates_helper_without_client_watchdog() {
    let _g = SPAWN_TEST.lock().expect("spawn test lock");
    let dir = std::env::temp_dir().join(format!(
        "fvoci-pdeath-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("pid dir");
    let pid_file = dir.join("helper.pid");
    let driver = Command::new(env!("CARGO_BIN_EXE_extract-parent-death-driver"))
        .arg(bin())
        .arg(&pid_file)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn parent-death driver");

    struct Owned {
        driver: Child,
        helper: Option<(u32, u64)>,
        dir: std::path::PathBuf,
    }
    impl Drop for Owned {
        fn drop(&mut self) {
            let _ = self.driver.kill();
            let _ = self.driver.wait();
            if let Some((pid, start)) = self.helper {
                if proc_starttime(pid) == Some(start) {
                    unsafe {
                        libc::kill(pid as libc::pid_t, libc::SIGKILL);
                    }
                }
            }
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
    let mut owned = Owned {
        driver,
        helper: None,
        dir,
    };
    wait_until(
        Instant::now() + Duration::from_secs(8),
        || {
            std::fs::read_to_string(&pid_file)
                .ok()
                .and_then(|s| s.trim().parse::<u32>().ok())
                .is_some()
        },
        "helper pid file",
    );
    let helper_pid: u32 = std::fs::read_to_string(&pid_file)
        .expect("pid file")
        .trim()
        .parse()
        .expect("helper pid");
    let start = proc_starttime(helper_pid).expect("helper starttime");
    owned.helper = Some((helper_pid, start));
    let driver_pid_u32 = owned.driver.id();
    assert!(
        is_helper_for_parent(helper_pid, driver_pid_u32, &bin()),
        "pid file must name this driver's hanging extractor helper"
    );
    assert_eq!(
        observe_pid(helper_pid, Some(start)),
        PidView::Live,
        "helper must be alive before parent SIGKILL so PDEATHSIG is the killer"
    );
    let driver_pid = driver_pid_u32 as libc::pid_t;
    let kill_rc = unsafe { libc::kill(driver_pid, libc::SIGKILL) };
    assert_eq!(kill_rc, 0, "SIGKILL driver");
    let _ = owned.driver.wait().expect("reap driver");
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut view = observe_pid(helper_pid, Some(start));
    while view == PidView::Live && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
        view = observe_pid(helper_pid, Some(start));
    }
    assert_ne!(
        view,
        PidView::Live,
        "helper must terminate without client watchdog (gone or zombie), view={view:?}"
    );
    owned.helper = None;
}
