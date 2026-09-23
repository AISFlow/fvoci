use std::io::Read;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::sync::{Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::frame::{read_frame, write_frame, FrameError};
use crate::limits::{Limits, MAX_CHILD_CONCURRENCY, MAX_CHILD_STDERR_BYTES, RSS_POLL_MS};
use crate::outcome::{EngineReport, EngineStatus, LimitKind, WorkerFailureReason};
use crate::protocol::Request;

static CHILD_SLOTS: OnceLock<Mutex<usize>> = OnceLock::new();

fn slots() -> &'static Mutex<usize> {
    CHILD_SLOTS.get_or_init(|| Mutex::new(0))
}

struct SlotGuard;

impl SlotGuard {
    fn try_acquire() -> Result<Self, EngineReport> {
        let mut used = slots().lock().map_err(|_| {
            worker_fail(WorkerFailureReason::SlotPoison, "child slot mutex poisoned")
        })?;
        if *used < MAX_CHILD_CONCURRENCY {
            *used += 1;
            return Ok(Self);
        }
        Err(EngineReport::new(EngineStatus::ResourceLimit {
            kind: LimitKind::Ops,
            detail: format!(
                "live collab children at cap {MAX_CHILD_CONCURRENCY}; parent room map owns per-document uniqueness"
            ),
        }))
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
pub struct SpawnRequest {
    pub engine_bin: PathBuf,
    pub limits: Limits,
    pub test_hang_ms: Option<u64>,
}

/// Observation of the last product helper spawned by [`EngineSession::spawn`].
/// Available only with `--features test-hang`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpawnTrace {
    pub pid: u32,
    pub helpers_joined: u8,
}

#[cfg(feature = "test-hang")]
static LAST_SPAWN: OnceLock<Mutex<Option<SpawnTrace>>> = OnceLock::new();

#[cfg(feature = "test-hang")]
fn last_spawn_lock() -> &'static Mutex<Option<SpawnTrace>> {
    LAST_SPAWN.get_or_init(|| Mutex::new(None))
}

#[cfg(feature = "test-hang")]
pub fn take_last_spawn() -> Option<SpawnTrace> {
    last_spawn_lock().lock().ok().and_then(|mut g| g.take())
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
fn add_helpers_joined(n: u8) {
    if let Ok(mut g) = last_spawn_lock().lock() {
        if let Some(trace) = g.as_mut() {
            trace.helpers_joined = trace.helpers_joined.saturating_add(n);
        }
    }
}

struct LiveChild {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: Option<std::process::ChildStdout>,
    stderr_join: Option<JoinHandle<Result<Vec<u8>, String>>>,
    limits: Limits,
    _slot: SlotGuard,
}

/// Parent handle to one native child that owns a single Yrs Doc.
///
/// Drop kills and reaps. A rejected or uncertain candidate must be recycled
/// via [`EngineSession::kill_and_reap`] and a fresh load of the last committed
/// snapshot — never Yrs undo as a DB rollback. Success here is not durable.
pub struct EngineSession {
    live: Option<LiveChild>,
    pid: u32,
}

impl EngineSession {
    pub fn spawn(req: SpawnRequest) -> Result<Self, EngineReport> {
        if let Err(detail) = req.limits.validate() {
            return Err(worker_fail(WorkerFailureReason::InvalidLimits, detail));
        }

        #[cfg(not(target_os = "linux"))]
        {
            let _ = req;
            return Err(worker_fail(
                WorkerFailureReason::UnsupportedPlatform,
                "EngineSession requires Linux rlimits; refuse closed on this OS",
            ));
        }

        #[cfg(target_os = "linux")]
        {
            let slot = SlotGuard::try_acquire()?;
            spawn_child(req, slot)
        }
    }

    pub fn pid(&self) -> Option<u32> {
        self.live.as_ref().map(|_| self.pid)
    }

    pub fn call(&mut self, request: &Request) -> EngineReport {
        let (max_frame, timeout_ms, limits) = match self.live.as_ref() {
            Some(live) => (
                live.limits.max_frame_bytes,
                live.limits.timeout_ms,
                live.limits,
            ),
            None => {
                return worker_fail(WorkerFailureReason::SessionDead, "child already reaped")
                    .with_child_pid(self.pid);
            }
        };
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        if let Err(outcome) = request.preflight(&limits) {
            self.kill_and_reap();
            return EngineReport::new(outcome).with_child_pid(self.pid);
        }
        if Instant::now() >= deadline {
            self.kill_and_reap();
            return EngineReport::new(EngineStatus::ResourceLimit {
                kind: LimitKind::Time,
                detail: "deadline expired before send".into(),
            })
            .with_child_pid(self.pid);
        }
        let payload = match serde_json::to_vec(request) {
            Ok(p) => p,
            Err(err) => {
                self.kill_and_reap();
                return worker_fail(
                    WorkerFailureReason::Protocol,
                    format!("serialize request: {err}"),
                )
                .with_child_pid(self.pid);
            }
        };
        if payload.len() as u64 > max_frame {
            self.kill_and_reap();
            return EngineReport::new(EngineStatus::ResourceLimit {
                kind: LimitKind::Frame,
                detail: format!("request frame {} exceeds {max_frame}", payload.len()),
            })
            .with_child_pid(self.pid);
        }

        if let Err(report) = self.write_payload(payload, max_frame, deadline) {
            self.kill_and_reap();
            return report;
        }
        match self.wait_frame(deadline) {
            Ok(report) => {
                if !matches!(report.outcome, EngineStatus::Ok { .. }) {
                    self.kill_and_reap();
                    EngineReport {
                        child_pid: Some(self.pid),
                        ..report
                    }
                } else {
                    report.with_child_pid(self.pid)
                }
            }
            Err(report) => {
                self.kill_and_reap();
                report
            }
        }
    }

    fn write_payload(
        &mut self,
        payload: Vec<u8>,
        max_frame: u64,
        deadline: Instant,
    ) -> Result<(), EngineReport> {
        let live = self
            .live
            .as_mut()
            .ok_or_else(|| worker_fail(WorkerFailureReason::SessionDead, "child gone"))?;
        let mut stdin = live
            .stdin
            .take()
            .ok_or_else(|| worker_fail(WorkerFailureReason::Protocol, "stdin closed"))?;
        let rss_cap = live.limits.max_observed_rss_bytes;
        let pid = self.pid;
        let (tx, rx) = mpsc::channel();
        let writer = thread::spawn(move || {
            let result = write_frame(&mut stdin, &payload, max_frame);
            let _ = tx.send((result, stdin));
        });
        loop {
            match rx.try_recv() {
                Ok((result, stdin)) => {
                    live.stdin = Some(stdin);
                    let _ = writer.join();
                    #[cfg(feature = "test-hang")]
                    add_helpers_joined(1);
                    return result.map_err(|err| {
                        worker_fail(WorkerFailureReason::Protocol, format!("write frame: {err}"))
                            .with_child_pid(pid)
                    });
                }
                Err(mpsc::TryRecvError::Empty) => {
                    if Instant::now() >= deadline {
                        let _ = live.child.kill();
                        let _ = live.child.wait();
                        let _ = writer.join();
                        #[cfg(feature = "test-hang")]
                        add_helpers_joined(1);
                        return Err(EngineReport::new(EngineStatus::ResourceLimit {
                            kind: LimitKind::Time,
                            detail: format!(
                                "child pid {pid} exceeded wall timeout during send; killed and reaped"
                            ),
                        })
                        .with_child_pid(pid));
                    }
                    if let Some(rss) = child_rss_bytes(pid) {
                        if rss > rss_cap {
                            let _ = live.child.kill();
                            let _ = live.child.wait();
                            let _ = writer.join();
                            #[cfg(feature = "test-hang")]
                            add_helpers_joined(1);
                            return Err(EngineReport::new(EngineStatus::ResourceLimit {
                                kind: LimitKind::Memory,
                                detail: format!(
                                    "child pid {pid} VmRSS {rss} exceeded observed cap {rss_cap}"
                                ),
                            })
                            .with_child_pid(pid));
                        }
                    }
                    match live.child.try_wait() {
                        Ok(Some(status)) => {
                            let _ = writer.join();
                            #[cfg(feature = "test-hang")]
                            add_helpers_joined(1);
                            let stderr = join_pipe(live.stderr_join.take());
                            #[cfg(feature = "test-hang")]
                            add_helpers_joined(1);
                            return Err(classify_child_exit(status, pid, &stderr));
                        }
                        Ok(None) => thread::sleep(Duration::from_millis(RSS_POLL_MS)),
                        Err(err) => {
                            let _ = live.child.kill();
                            let _ = live.child.wait();
                            let _ = writer.join();
                            #[cfg(feature = "test-hang")]
                            add_helpers_joined(1);
                            return Err(worker_fail(
                                WorkerFailureReason::Wait,
                                format!("wait failed: {err}"),
                            )
                            .with_child_pid(pid));
                        }
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    let _ = writer.join();
                    #[cfg(feature = "test-hang")]
                    add_helpers_joined(1);
                    return Err(worker_fail(
                        WorkerFailureReason::Protocol,
                        "stdin writer disconnected",
                    )
                    .with_child_pid(pid));
                }
            }
        }
    }

    fn wait_frame(&mut self, deadline: Instant) -> Result<EngineReport, EngineReport> {
        let live = self
            .live
            .as_mut()
            .ok_or_else(|| worker_fail(WorkerFailureReason::SessionDead, "child gone"))?;
        let mut stdout = live
            .stdout
            .take()
            .ok_or_else(|| worker_fail(WorkerFailureReason::Protocol, "stdout closed"))?;
        let max = live.limits.max_frame_bytes;
        let rss_cap = live.limits.max_observed_rss_bytes;
        let pid = self.pid;
        let (tx, rx) = mpsc::channel();
        let reader = thread::spawn(move || {
            let result = read_frame(&mut stdout, max);
            let _ = tx.send((result, stdout));
        });

        let outcome = loop {
            match rx.try_recv() {
                Ok((result, stdout)) => {
                    live.stdout = Some(stdout);
                    let _ = reader.join();
                    #[cfg(feature = "test-hang")]
                    add_helpers_joined(1);
                    break match result {
                        Ok(Some(buf)) => parse_child_json(&buf, pid),
                        Ok(None) => Err(worker_fail(
                            WorkerFailureReason::Protocol,
                            "child closed stdout before a frame",
                        )
                        .with_child_pid(pid)),
                        Err(FrameError::TooLarge { len, max }) => {
                            Err(EngineReport::new(EngineStatus::ResourceLimit {
                                kind: LimitKind::Frame,
                                detail: format!("child stdout frame {len} exceeds {max}"),
                            })
                            .with_child_pid(pid))
                        }
                        Err(FrameError::Io(detail)) => {
                            Err(worker_fail(WorkerFailureReason::Protocol, detail)
                                .with_child_pid(pid))
                        }
                    };
                }
                Err(mpsc::TryRecvError::Empty) => match live.child.try_wait() {
                    Ok(Some(status)) => {
                        let _ = reader.join();
                        #[cfg(feature = "test-hang")]
                        add_helpers_joined(1);
                        let stderr = join_pipe(live.stderr_join.take());
                        #[cfg(feature = "test-hang")]
                        add_helpers_joined(1);
                        break Err(classify_child_exit(status, pid, &stderr));
                    }
                    Ok(None) => {
                        if Instant::now() >= deadline {
                            let _ = live.child.kill();
                            let _ = live.child.wait();
                            let _ = reader.join();
                            #[cfg(feature = "test-hang")]
                            add_helpers_joined(1);
                            break Err(EngineReport::new(EngineStatus::ResourceLimit {
                                kind: LimitKind::Time,
                                detail: format!(
                                    "child pid {pid} exceeded wall timeout; killed and reaped"
                                ),
                            })
                            .with_child_pid(pid));
                        }
                        if let Some(rss) = child_rss_bytes(pid) {
                            if rss > rss_cap {
                                let _ = live.child.kill();
                                let _ = live.child.wait();
                                let _ = reader.join();
                                #[cfg(feature = "test-hang")]
                                add_helpers_joined(1);
                                break Err(EngineReport::new(EngineStatus::ResourceLimit {
                                        kind: LimitKind::Memory,
                                        detail: format!(
                                            "child pid {pid} VmRSS {rss} exceeded observed cap {rss_cap}"
                                        ),
                                    })
                                    .with_child_pid(pid));
                            }
                        }
                        thread::sleep(Duration::from_millis(RSS_POLL_MS));
                    }
                    Err(err) => {
                        let _ = live.child.kill();
                        let _ = live.child.wait();
                        let _ = reader.join();
                        #[cfg(feature = "test-hang")]
                        add_helpers_joined(1);
                        break Err(worker_fail(
                            WorkerFailureReason::Wait,
                            format!("wait failed: {err}"),
                        )
                        .with_child_pid(pid));
                    }
                },
                Err(mpsc::TryRecvError::Disconnected) => {
                    let _ = reader.join();
                    #[cfg(feature = "test-hang")]
                    add_helpers_joined(1);
                    break Err(worker_fail(
                        WorkerFailureReason::Protocol,
                        "stdout reader disconnected",
                    )
                    .with_child_pid(pid));
                }
            }
        };
        outcome
    }

    /// Kill, wait, join helpers. Safe to call twice. Session cannot be reused.
    pub fn kill_and_reap(&mut self) {
        if let Some(mut live) = self.live.take() {
            let _ = live.child.kill();
            let _ = live.child.wait();
            if live.stderr_join.is_some() {
                join_any(live.stderr_join.take());
                #[cfg(feature = "test-hang")]
                add_helpers_joined(1);
            }
            drop(live.stdin);
            drop(live.stdout);
            drop(live._slot);
        }
    }
}

impl Drop for EngineSession {
    fn drop(&mut self) {
        self.kill_and_reap();
    }
}

fn spawn_child(req: SpawnRequest, slot: SlotGuard) -> Result<EngineSession, EngineReport> {
    if req.engine_bin.as_os_str().is_empty() || !req.engine_bin.is_file() {
        return Err(worker_fail(
            WorkerFailureReason::MissingExecutable,
            format!(
                "engine binary path required and must exist: {:?}",
                req.engine_bin
            ),
        ));
    }

    let mut cmd = Command::new(&req.engine_bin);
    cmd.env_clear()
        .arg("--max-input")
        .arg(req.limits.max_input_bytes.to_string())
        .arg("--max-output")
        .arg(req.limits.max_output_bytes.to_string())
        .arg("--max-load")
        .arg(req.limits.max_load_bytes.to_string())
        .arg("--max-frame")
        .arg(req.limits.max_frame_bytes.to_string())
        .arg("--max-tail")
        .arg(req.limits.max_tail_updates.to_string())
        .arg("--max-ops")
        .arg(req.limits.max_ops.to_string())
        .arg("--timeout-ms")
        .arg(req.limits.timeout_ms.to_string())
        .arg("--max-as")
        .arg(req.limits.max_child_as_bytes.to_string())
        .arg("--max-observed-rss")
        .arg(req.limits.max_observed_rss_bytes.to_string())
        .arg("--max-stack")
        .arg(req.limits.max_child_stack_bytes.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    if let Err(err) = apply_pre_exec_rlimits(&mut cmd, &req.limits) {
        return Err(worker_fail(WorkerFailureReason::LimitApply, err));
    }

    #[cfg(feature = "test-hang")]
    if let Some(ms) = req.test_hang_ms {
        cmd.arg("--test-hang-ms").arg(ms.to_string());
    }
    #[cfg(not(feature = "test-hang"))]
    let _ = req.test_hang_ms;

    let mut child = cmd.spawn().map_err(|err| {
        worker_fail(
            WorkerFailureReason::Spawn,
            format!("failed to spawn {:?}: {err}", req.engine_bin),
        )
    })?;

    let pid = child.id();
    #[cfg(feature = "test-hang")]
    record_spawn(pid);

    let stdin = child.stdin.take().ok_or_else(|| {
        let _ = child.kill();
        let _ = child.wait();
        worker_fail(WorkerFailureReason::Spawn, "child stdin missing")
    })?;
    let stdout = child.stdout.take().ok_or_else(|| {
        let _ = child.kill();
        let _ = child.wait();
        worker_fail(WorkerFailureReason::Spawn, "child stdout missing")
    })?;
    let stderr_join = Some(take_pipe(child.stderr.take()));

    Ok(EngineSession {
        live: Some(LiveChild {
            child,
            stdin: Some(stdin),
            stdout: Some(stdout),
            stderr_join,
            limits: req.limits,
            _slot: slot,
        }),
        pid,
    })
}

fn parse_child_json(buf: &[u8], pid: u32) -> Result<EngineReport, EngineReport> {
    match serde_json::from_slice::<EngineReport>(buf) {
        Ok(report) => Ok(report.with_child_pid(pid)),
        Err(err) => Err(worker_fail(
            WorkerFailureReason::InvalidChildJson,
            format!("child output is not JSON: {err}"),
        )
        .with_child_pid(pid)),
    }
}

fn take_pipe<T: Read + Send + 'static>(pipe: Option<T>) -> JoinHandle<Result<Vec<u8>, String>> {
    thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut out) = pipe {
            let mut tmp = [0u8; 4096];
            loop {
                match out.read(&mut tmp) {
                    Ok(0) => break,
                    Ok(n) => {
                        if buf.len() as u64 + n as u64 > MAX_CHILD_STDERR_BYTES {
                            buf.extend_from_slice(&tmp[..n]);
                            buf.truncate(MAX_CHILD_STDERR_BYTES as usize);
                            break;
                        }
                        buf.extend_from_slice(&tmp[..n]);
                    }
                    Err(err)
                        if err.kind() == std::io::ErrorKind::WouldBlock
                            || err.kind() == std::io::ErrorKind::Interrupted =>
                    {
                        thread::sleep(Duration::from_millis(RSS_POLL_MS));
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

fn classify_child_exit(
    status: std::process::ExitStatus,
    pid: u32,
    stderr: &Result<Vec<u8>, String>,
) -> EngineReport {
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
                return EngineReport::new(EngineStatus::ResourceLimit {
                    kind: LimitKind::Time,
                    detail: format!("child pid {pid} CPU/alarm signal {sig}; {stderr_snip}"),
                })
                .with_child_pid(pid);
            }
            if sig == 11 {
                return EngineReport::new(EngineStatus::ResourceLimit {
                    kind: LimitKind::Stack,
                    detail: format!(
                        "child pid {pid} SIGSEGV (stack/recursion isolation); {stderr_snip}"
                    ),
                })
                .with_child_pid(pid);
            }
            if sig == 6 && allocation_failure_stderr(stderr_text) {
                return EngineReport::new(EngineStatus::ResourceLimit {
                    kind: LimitKind::Memory,
                    detail: format!(
                        "child pid {pid} SIGABRT after allocator failure; {stderr_snip}"
                    ),
                })
                .with_child_pid(pid);
            }
            return worker_fail(
                WorkerFailureReason::ChildCrash,
                format!("child pid {pid} signal {sig}; {stderr_snip}"),
            )
            .with_child_pid(pid);
        }
    }

    if !status.success() {
        return worker_fail(
            WorkerFailureReason::ChildCrash,
            format!("child pid {pid} exit {:?}; {stderr_snip}", status.code()),
        )
        .with_child_pid(pid);
    }
    worker_fail(
        WorkerFailureReason::ChildCrash,
        format!("child pid {pid} exited before response; {stderr_snip}"),
    )
    .with_child_pid(pid)
}

fn allocation_failure_stderr(stderr: &str) -> bool {
    let lower = stderr.to_ascii_lowercase();
    lower.contains("memory allocation of") && lower.contains("failed")
}

fn worker_fail(reason: WorkerFailureReason, detail: impl Into<String>) -> EngineReport {
    EngineReport::new(EngineStatus::WorkerFailure {
        reason,
        detail: detail.into(),
    })
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
        let as_bytes = limits.max_child_as_bytes;
        let stack_bytes = limits.max_child_stack_bytes;
        let cpu_secs = (limits.timeout_ms / 1000).max(1);
        unsafe {
            cmd.pre_exec(move || apply_rlimits_now(as_bytes, cpu_secs, stack_bytes));
        }
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (cmd, limits);
        Err("rlimit pre_exec is Linux-only".into())
    }
}

/// Apply OS ceilings in the current process. Used from `pre_exec` and the child
/// binary before it reads frames.
pub fn apply_rlimits_now(as_bytes: u64, cpu_secs: u64, stack_bytes: u64) -> std::io::Result<()> {
    #[cfg(target_os = "linux")]
    unsafe {
        let stack_lim = libc::rlimit {
            rlim_cur: stack_bytes,
            rlim_max: stack_bytes,
        };
        if libc::setrlimit(libc::RLIMIT_STACK, &stack_lim) != 0 {
            return Err(std::io::Error::last_os_error());
        }
        let cpu_lim = libc::rlimit {
            rlim_cur: cpu_secs,
            rlim_max: cpu_secs.saturating_add(1),
        };
        if libc::setrlimit(libc::RLIMIT_CPU, &cpu_lim) != 0 {
            return Err(std::io::Error::last_os_error());
        }
        let as_lim = libc::rlimit {
            rlim_cur: as_bytes,
            rlim_max: as_bytes,
        };
        if libc::setrlimit(libc::RLIMIT_AS, &as_lim) != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (as_bytes, cpu_secs, stack_bytes);
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "setrlimit is Linux-only",
        ))
    }
}
