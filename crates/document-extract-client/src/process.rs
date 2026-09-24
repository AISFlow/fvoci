use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::limits::{Limits, MAX_CHILD_CONCURRENCY, MAX_CHILD_STDOUT_BYTES, RSS_POLL_MS};
use crate::outcome::{ExtractReport, ExtractStatus, LimitKind, WorkerFailureReason};

static CHILD_SLOTS: OnceLock<Mutex<usize>> = OnceLock::new();

fn slots() -> &'static Mutex<usize> {
    CHILD_SLOTS.get_or_init(|| Mutex::new(0))
}

/// Client-only outcome: extraction stopped before a canonical child report.
/// This is not an [`ExtractStatus`] wire variant and must never be reported as
/// a successful empty extract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cancelled {
    /// Set when a helper was spawned and then killed because of cancel.
    pub child_pid: Option<u32>,
}

enum SlotWait {
    Ready(SlotGuard),
    Cancelled,
    Failed(ExtractReport),
}

struct SlotGuard;

impl SlotGuard {
    fn acquire(deadline: Instant, cancel: &AtomicBool) -> SlotWait {
        loop {
            if cancelled(cancel) {
                return SlotWait::Cancelled;
            }
            {
                let mut used = match slots().lock() {
                    Ok(guard) => guard,
                    Err(_) => {
                        return SlotWait::Failed(worker_fail(
                            WorkerFailureReason::SlotPoison,
                            "child slot mutex poisoned",
                        ));
                    }
                };
                if *used < MAX_CHILD_CONCURRENCY {
                    *used += 1;
                    return SlotWait::Ready(Self);
                }
            }
            if Instant::now() >= deadline {
                return SlotWait::Failed(ExtractReport::new(ExtractStatus::ResourceLimit {
                    kind: LimitKind::Time,
                    detail: "timed out waiting for extract child slot".to_string(),
                }));
            }
            #[cfg(feature = "test-hang")]
            let _waiting = SlotWaitWitness::enter(cancel);
            thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for SlotGuard {
    fn drop(&mut self) {
        if let Ok(mut used) = slots().lock() {
            *used = used.saturating_sub(1);
        }
    }
}

#[derive(Debug, Clone)]
pub struct ExtractRequest {
    pub bytes: Vec<u8>,
    pub name: String,
    pub limits: Limits,
    pub extractor_bin: PathBuf,
    pub test_hang_ms: Option<u64>,
}

/// Observation of the last product helper spawned by [`extract_killable`].
/// Available only with `--features test-hang`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpawnTrace {
    pub pid: u32,
    pub helpers_joined: u8,
}

#[cfg(feature = "test-hang")]
static LAST_SPAWN: OnceLock<Mutex<Option<SpawnTrace>>> = OnceLock::new();

#[cfg(feature = "test-hang")]
static SLOT_WAIT_KEYS: OnceLock<Mutex<std::collections::HashSet<usize>>> = OnceLock::new();

#[cfg(feature = "test-hang")]
fn last_spawn_lock() -> &'static Mutex<Option<SpawnTrace>> {
    LAST_SPAWN.get_or_init(|| Mutex::new(None))
}

#[cfg(feature = "test-hang")]
fn slot_wait_keys() -> &'static Mutex<std::collections::HashSet<usize>> {
    SLOT_WAIT_KEYS.get_or_init(|| Mutex::new(std::collections::HashSet::new()))
}

#[cfg(feature = "test-hang")]
fn cancel_key(cancel: &AtomicBool) -> usize {
    cancel as *const AtomicBool as usize
}

#[cfg(feature = "test-hang")]
struct SlotWaitWitness {
    key: usize,
}

#[cfg(feature = "test-hang")]
impl SlotWaitWitness {
    fn enter(cancel: &AtomicBool) -> Self {
        let key = cancel_key(cancel);
        if let Ok(mut keys) = slot_wait_keys().lock() {
            keys.insert(key);
        }
        Self { key }
    }
}

#[cfg(feature = "test-hang")]
impl Drop for SlotWaitWitness {
    fn drop(&mut self) {
        if let Ok(mut keys) = slot_wait_keys().lock() {
            keys.remove(&self.key);
        }
    }
}

#[cfg(feature = "test-hang")]
pub fn take_last_spawn() -> Option<SpawnTrace> {
    last_spawn_lock().lock().ok().and_then(|mut g| g.take())
}

#[cfg(feature = "test-hang")]
pub fn peek_last_spawn() -> Option<SpawnTrace> {
    last_spawn_lock().lock().ok().and_then(|g| *g)
}

/// True only while `cancel` is the flag of a request currently sleeping in
/// slot wait. A different request's waiter cannot satisfy this check.
#[cfg(feature = "test-hang")]
pub fn is_slot_waiter(cancel: &AtomicBool) -> bool {
    let key = cancel_key(cancel);
    slot_wait_keys()
        .lock()
        .map(|keys| keys.contains(&key))
        .unwrap_or(false)
}

#[cfg(feature = "test-hang")]
fn record_spawn(pid: u32) {
    if let Ok(mut g) = last_spawn_lock().lock() {
        *g = Some(SpawnTrace {
            pid,
            helpers_joined: 0,
        });
    }
}

#[cfg(feature = "test-hang")]
fn record_helpers_joined(n: u8) {
    if let Ok(mut g) = last_spawn_lock().lock() {
        if let Some(trace) = g.as_mut() {
            trace.helpers_joined = n;
        }
    }
}

fn cancelled(flag: &AtomicBool) -> bool {
    flag.load(Ordering::Acquire)
}

/// Run extraction in a killable child. This call is **synchronous**: the
/// deadline is `limits.timeout_ms` from admission. There is no external
/// cancel token; use [`extract_killable_with_cancel`] for an `AtomicBool`.
/// Dropping a `JoinHandle` that wraps this function does **not** terminate
/// the child; only the watchdog kill+reap path or Linux parent-death SIGKILL
/// does. `PR_SET_PDEATHSIG` is delivered when the **spawning thread** dies,
/// which is stricter than whole-process death. This synchronous function
/// keeps that thread in the wait loop until completion; exiting a detached
/// spawn thread while a helper still runs is not a supported cancel path.
pub fn extract_killable(req: ExtractRequest) -> ExtractReport {
    match extract_killable_with_cancel(req, &AtomicBool::new(false)) {
        Ok(report) => report,
        Err(Cancelled { child_pid }) => {
            let mut report = worker_fail(
                WorkerFailureReason::Wait,
                format!("cancel flag was unset (child_pid={child_pid:?})"),
            );
            if let Some(pid) = child_pid {
                report = report.with_child_pid(pid);
            }
            report
        }
    }
}

/// Same process boundary as [`extract_killable`], plus a client-owned cancel
/// flag. The flag is an `AtomicBool` observed at slot admission, spawn, and
/// the existing `try_wait` poll; dropping a future or `JoinHandle` is not
/// cancellation. Canonical child `ExtractStatus` / `WorkerFailureReason`
/// enums are unchanged.
pub fn extract_killable_with_cancel(
    req: ExtractRequest,
    cancel: &AtomicBool,
) -> Result<ExtractReport, Cancelled> {
    if cancelled(cancel) {
        return Err(Cancelled { child_pid: None });
    }
    if let Err(detail) = req.limits.validate() {
        return Ok(worker_fail(WorkerFailureReason::InvalidLimits, detail));
    }
    if req.bytes.len() as u64 > req.limits.max_input_bytes {
        return Ok(ExtractReport::new(ExtractStatus::ResourceLimit {
            kind: LimitKind::Input,
            detail: format!(
                "input {} bytes exceeds {}-byte limit",
                req.bytes.len(),
                req.limits.max_input_bytes
            ),
        }));
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = cancel;
        return Ok(worker_fail(
            WorkerFailureReason::UnsupportedPlatform,
            "extract_killable requires Linux rlimits; refuse closed on this OS",
        ));
    }

    #[cfg(target_os = "linux")]
    {
        if cancelled(cancel) {
            return Err(Cancelled { child_pid: None });
        }
        let deadline = Instant::now() + Duration::from_millis(req.limits.timeout_ms);
        let _slot = match SlotGuard::acquire(deadline, cancel) {
            SlotWait::Ready(slot) => slot,
            SlotWait::Cancelled => return Err(Cancelled { child_pid: None }),
            SlotWait::Failed(report) => return Ok(report),
        };
        if cancelled(cancel) {
            return Err(Cancelled { child_pid: None });
        }
        spawn_child(req, deadline, cancel)
    }
}

fn worker_fail(reason: WorkerFailureReason, detail: impl Into<String>) -> ExtractReport {
    ExtractReport::new(ExtractStatus::WorkerFailure {
        reason,
        detail: detail.into(),
    })
}

fn spawn_child(
    req: ExtractRequest,
    deadline: Instant,
    cancel: &AtomicBool,
) -> Result<ExtractReport, Cancelled> {
    if cancelled(cancel) {
        return Err(Cancelled { child_pid: None });
    }
    if req.extractor_bin.as_os_str().is_empty() || !req.extractor_bin.is_file() {
        return Ok(worker_fail(
            WorkerFailureReason::MissingExecutable,
            format!(
                "extractor binary path required and must exist: {:?}",
                req.extractor_bin
            ),
        ));
    }

    let mut cmd = Command::new(&req.extractor_bin);
    cmd.arg("--name")
        .arg(&req.name)
        .arg("--max-input")
        .arg(req.limits.max_input_bytes.to_string())
        .arg("--max-output")
        .arg(req.limits.max_output_chars.to_string())
        .arg("--max-zip-entries")
        .arg(req.limits.max_zip_entries.to_string())
        .arg("--max-zip-uncompressed")
        .arg(req.limits.max_zip_uncompressed_bytes.to_string())
        .arg("--timeout-ms")
        .arg(req.limits.timeout_ms.to_string())
        .arg("--max-rss")
        .arg(req.limits.max_child_rss_bytes.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env_remove("DOCUMENT_EXTRACT_TEST_HANG_MS");

    if let Err(err) = apply_pre_exec_rlimits(&mut cmd, &req.limits) {
        return Ok(worker_fail(WorkerFailureReason::LimitApply, err));
    }

    #[cfg(feature = "test-hang")]
    if let Some(ms) = req.test_hang_ms {
        cmd.arg("--test-hang-ms").arg(ms.to_string());
    }
    #[cfg(not(feature = "test-hang"))]
    let _ = req.test_hang_ms;

    if cancelled(cancel) {
        return Err(Cancelled { child_pid: None });
    }

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(err) => {
            return Ok(worker_fail(
                WorkerFailureReason::Spawn,
                format!("failed to spawn {:?}: {err}", req.extractor_bin),
            ));
        }
    };

    let pid = child.id();
    #[cfg(feature = "test-hang")]
    record_spawn(pid);
    let stdin_join = match child.stdin.take() {
        Some(mut stdin) => {
            let payload = req.bytes;
            Some(thread::spawn(move || {
                let _ = stdin.write_all(&payload);
                let _ = stdin.flush();
                drop(stdin);
            }))
        }
        None => None,
    };
    let stdout_join = Some(take_pipe(child.stdout.take()));
    let stderr_join = Some(take_pipe(child.stderr.take()));

    let mut limit = None;
    let mut wait_status = None;
    let mut cancel_pid = None;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                wait_status = Some(status);
                break;
            }
            Ok(None) => {
                if cancelled(cancel) {
                    kill_and_reap(&mut child);
                    cancel_pid = Some(pid);
                    break;
                }
                if Instant::now() >= deadline {
                    limit = Some(LimitKind::Time);
                    kill_and_reap(&mut child);
                    break;
                }
                if let Some(rss) = child_rss_bytes(pid) {
                    if rss > req.limits.max_child_rss_bytes {
                        limit = Some(LimitKind::Memory);
                        kill_and_reap(&mut child);
                        break;
                    }
                }
                thread::sleep(Duration::from_millis(RSS_POLL_MS));
            }
            Err(err) => {
                kill_and_reap(&mut child);
                join_any(stdin_join);
                join_any(stdout_join);
                join_any(stderr_join);
                #[cfg(feature = "test-hang")]
                record_helpers_joined(3);
                return Ok(
                    worker_fail(WorkerFailureReason::Wait, format!("wait failed: {err}"))
                        .with_child_pid(pid),
                );
            }
        }
    }

    join_any(stdin_join);
    let stdout_bytes = join_pipe(stdout_join);
    let stderr_bytes = join_pipe(stderr_join);
    #[cfg(feature = "test-hang")]
    record_helpers_joined(3);

    if let Some(pid) = cancel_pid {
        let _ = (stdout_bytes, stderr_bytes);
        return Err(Cancelled {
            child_pid: Some(pid),
        });
    }

    if let Some(kind) = limit {
        return Ok(ExtractReport::new(ExtractStatus::ResourceLimit {
            kind,
            detail: format!("child pid {pid} exceeded {kind:?}; killed and reaped"),
        })
        .with_child_pid(pid));
    }

    let status = match wait_status {
        Some(status) => status,
        None => match child.wait() {
            Ok(status) => status,
            Err(err) => {
                return Ok(
                    worker_fail(WorkerFailureReason::Wait, format!("reap failed: {err}"))
                        .with_child_pid(pid),
                );
            }
        },
    };

    Ok(report_from_child_io(
        status,
        pid,
        stdout_bytes,
        stderr_bytes,
    ))
}

/// Classify child pipes and status. Stdout overflow is checked before crash
/// mapping: the helper panics after the parent closes an oversized pipe.
pub(crate) fn report_from_child_io(
    status: std::process::ExitStatus,
    pid: u32,
    stdout_bytes: Result<Vec<u8>, String>,
    stderr_bytes: Result<Vec<u8>, String>,
) -> ExtractReport {
    if matches!(&stdout_bytes, Err(err) if err == "pipe exceeded bound") {
        return ExtractReport::new(ExtractStatus::ResourceLimit {
            kind: LimitKind::Output,
            detail: format!("child stdout exceeded {MAX_CHILD_STDOUT_BYTES} bytes"),
        })
        .with_child_pid(pid);
    }

    if let Some(report) = classify_child_status(status, pid, &stderr_bytes) {
        return report.with_child_pid(pid);
    }

    match stdout_bytes {
        Ok(buf) => match serde_json::from_slice::<ExtractReport>(&buf) {
            Ok(report) => report.with_child_pid(pid),
            Err(err) => worker_fail(
                WorkerFailureReason::InvalidChildJson,
                format!("child output is not JSON: {err}"),
            )
            .with_child_pid(pid),
        },
        Err(err) => worker_fail(WorkerFailureReason::InvalidChildJson, err).with_child_pid(pid),
    }
}

fn take_pipe<T: Read + Send + 'static>(pipe: Option<T>) -> JoinHandle<Result<Vec<u8>, String>> {
    thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut out) = pipe {
            let mut tmp = [0u8; 8192];
            loop {
                match out.read(&mut tmp) {
                    Ok(0) => break,
                    Ok(n) => {
                        if buf.len() as u64 + n as u64 > MAX_CHILD_STDOUT_BYTES {
                            return Err("pipe exceeded bound".to_string());
                        }
                        buf.extend_from_slice(&tmp[..n]);
                    }
                    Err(err) => return Err(err.to_string()),
                }
            }
        }
        Ok(buf)
    })
}

fn join_pipe(handle: Option<JoinHandle<Result<Vec<u8>, String>>>) -> Result<Vec<u8>, String> {
    match handle {
        Some(h) => h
            .join()
            .unwrap_or_else(|_| Err("pipe thread panicked".into())),
        None => Ok(Vec::new()),
    }
}

fn join_any<T>(handle: Option<JoinHandle<T>>) {
    if let Some(h) = handle {
        let _ = h.join();
    }
}

fn classify_child_status(
    status: std::process::ExitStatus,
    pid: u32,
    stderr: &Result<Vec<u8>, String>,
) -> Option<ExtractReport> {
    let stderr_text = stderr
        .as_ref()
        .ok()
        .and_then(|b| std::str::from_utf8(b).ok())
        .unwrap_or("");
    let stderr_snip = stderr_text.chars().take(400).collect::<String>();

    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            if sig == 24 || sig == 14 {
                return Some(ExtractReport::new(ExtractStatus::ResourceLimit {
                    kind: LimitKind::Time,
                    detail: format!("child pid {pid} CPU/alarm signal {sig}; {stderr_snip}"),
                }));
            }
            if sig == 6 && allocation_failure_stderr(stderr_text) {
                return Some(ExtractReport::new(ExtractStatus::ResourceLimit {
                    kind: LimitKind::Memory,
                    detail: format!(
                        "child pid {pid} SIGABRT after allocator failure; {stderr_snip}"
                    ),
                }));
            }
            return Some(worker_fail(
                WorkerFailureReason::ChildCrash,
                format!("child pid {pid} signal {sig}; {stderr_snip}"),
            ));
        }
    }

    if !status.success() {
        return Some(worker_fail(
            WorkerFailureReason::ChildCrash,
            format!("child pid {pid} exit {:?}; {stderr_snip}", status.code()),
        ));
    }
    None
}

fn allocation_failure_stderr(stderr: &str) -> bool {
    let lower = stderr.to_ascii_lowercase();
    lower.contains("memory allocation of") && lower.contains("failed")
}

fn kill_and_reap(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn child_rss_bytes(pid: u32) -> Option<u64> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            let kb: u64 = rest.split_whitespace().next()?.parse().ok()?;
            return Some(kb.saturating_mul(1024));
        }
    }
    None
}

fn apply_pre_exec_rlimits(cmd: &mut Command, limits: &Limits) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::process::CommandExt;
        let as_bytes = limits.max_child_rss_bytes;
        let cpu_secs = (limits.timeout_ms / 1000).max(1);
        let expected_ppid = std::process::id() as libc::pid_t;
        unsafe {
            cmd.pre_exec(move || {
                apply_rlimits_now(as_bytes, cpu_secs)?;
                apply_parent_death_signal(expected_ppid)
            });
        }
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (cmd, limits);
        Err("rlimit pre_exec is Linux-only".into())
    }
}

/// Ask the kernel to SIGKILL this child if the forking thread dies, including
/// the fork-to-prctl race where that thread is already gone. Linux
/// `PR_SET_PDEATHSIG` is tied to the spawning thread, not to an arbitrary
/// other thread of the parent process.
#[cfg(target_os = "linux")]
fn apply_parent_death_signal(expected_ppid: libc::pid_t) -> std::io::Result<()> {
    // SAFETY: `pre_exec` runs between fork and exec. Only async-signal-safe
    // libc is used: `syscall`, `getppid`, `raise`, `_exit`.
    unsafe {
        let rc = libc::syscall(
            libc::SYS_prctl,
            libc::PR_SET_PDEATHSIG as libc::c_long,
            libc::SIGKILL as libc::c_long,
            0 as libc::c_long,
            0 as libc::c_long,
            0 as libc::c_long,
        );
        if rc != 0 {
            return Err(std::io::Error::last_os_error());
        }
        if libc::getppid() != expected_ppid {
            let _ = libc::raise(libc::SIGKILL);
            libc::_exit(127);
        }
    }
    Ok(())
}

/// Apply OS ceilings in the current process. Used from `pre_exec` and the child
/// binary before it reads input.
pub fn apply_rlimits_now(as_bytes: u64, cpu_secs: u64) -> std::io::Result<()> {
    #[cfg(target_os = "linux")]
    unsafe {
        let as_lim = libc::rlimit {
            rlim_cur: as_bytes,
            rlim_max: as_bytes,
        };
        if libc::setrlimit(libc::RLIMIT_AS, &as_lim) != 0 {
            return Err(std::io::Error::last_os_error());
        }
        let cpu_lim = libc::rlimit {
            rlim_cur: cpu_secs,
            rlim_max: cpu_secs.saturating_add(1),
        };
        if libc::setrlimit(libc::RLIMIT_CPU, &cpu_lim) != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (as_bytes, cpu_secs);
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "setrlimit is Linux-only",
        ))
    }
}

/// Feature-gated parent-death fixture. Spawns a hanging helper through the
/// production `pre_exec` path, writes the helper pid, then parks until SIGKILL.
/// Production builds omit this (required-features / `test-hang` only).
#[cfg(feature = "test-hang")]
pub fn run_parent_death_driver(extractor: PathBuf, pid_file: PathBuf) -> ! {
    let mut limits = Limits::for_tests();
    limits.timeout_ms = crate::limits::DEFAULT_TIMEOUT_MS;
    let req = ExtractRequest {
        bytes: Vec::new(),
        name: "hang.hwp".into(),
        limits,
        extractor_bin: extractor,
        test_hang_ms: Some(crate::limits::DEFAULT_TIMEOUT_MS),
    };
    let cancel = AtomicBool::new(false);
    thread::spawn(move || {
        let _ = extract_killable_with_cancel(req, &cancel);
    });
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        if let Some(trace) = peek_last_spawn() {
            if let Some(parent) = pid_file.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if std::fs::write(&pid_file, trace.pid.to_string()).is_err() {
                std::process::exit(2);
            }
            break;
        }
        if Instant::now() >= deadline {
            eprintln!("parent-death driver: helper did not spawn");
            std::process::exit(2);
        }
        thread::sleep(Duration::from_millis(10));
    }
    loop {
        thread::park();
    }
}

#[cfg(all(test, unix))]
mod child_io_tests {
    use super::report_from_child_io;
    use crate::outcome::{ExtractStatus, LimitKind, WorkerFailureReason};
    use std::os::unix::process::ExitStatusExt;
    use std::process::ExitStatus;

    #[test]
    fn stdout_overflow_beats_child_crash() {
        let status = ExitStatus::from_raw(101 << 8);
        let report = report_from_child_io(
            status,
            1,
            Err("pipe exceeded bound".into()),
            Ok(b"thread panicked".to_vec()),
        );
        assert!(
            matches!(
                report.outcome,
                ExtractStatus::ResourceLimit {
                    kind: LimitKind::Output,
                    ..
                }
            ),
            "{:?}",
            report.outcome
        );
    }

    #[test]
    fn sigabrt_with_allocator_stderr_is_memory_limit() {
        let status = ExitStatus::from_raw(6);
        let report = report_from_child_io(
            status,
            1,
            Ok(Vec::new()),
            Ok(format!(
                "{}memory allocation of 123 bytes failed",
                "warning\n".repeat(100)
            )
            .into_bytes()),
        );
        assert!(
            matches!(
                report.outcome,
                ExtractStatus::ResourceLimit {
                    kind: LimitKind::Memory,
                    ..
                }
            ),
            "{:?}",
            report.outcome
        );
    }

    #[test]
    fn sigabrt_without_allocator_stderr_is_crash() {
        let status = ExitStatus::from_raw(6);
        let report =
            report_from_child_io(status, 1, Ok(Vec::new()), Ok(b"assertion failed".to_vec()));
        assert!(
            matches!(
                report.outcome,
                ExtractStatus::WorkerFailure {
                    reason: WorkerFailureReason::ChildCrash,
                    ..
                }
            ),
            "{:?}",
            report.outcome
        );
    }
}
