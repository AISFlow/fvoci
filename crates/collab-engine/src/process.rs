use std::io::Read;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::frame::{read_frame, write_frame, FrameError};
use crate::limits::{Limits, DEFAULT_MAX_CHILD_CONCURRENCY, MAX_CHILD_STDERR_BYTES, RSS_POLL_MS};
use crate::outcome::{EngineReport, EngineStatus, LimitKind, WorkerFailureReason};
use crate::protocol::Request;

static PRIMARY_CHILD_SLOTS: OnceLock<Mutex<usize>> = OnceLock::new();
static VALIDATOR_CHILD_SLOTS: OnceLock<Mutex<usize>> = OnceLock::new();
static MAX_PRIMARY_CHILD_CONCURRENCY_RUNTIME: AtomicUsize =
    AtomicUsize::new(DEFAULT_MAX_CHILD_CONCURRENCY);
static MAX_VALIDATOR_CHILD_CONCURRENCY_RUNTIME: AtomicUsize =
    AtomicUsize::new(DEFAULT_MAX_CHILD_CONCURRENCY);
static LIVE_CHILD_PIDS: OnceLock<Mutex<Vec<u32>>> = OnceLock::new();

/// Configure the process-wide primary (per-room) live-child cap. Last write wins.
pub fn set_max_child_concurrency(limit: usize) {
    MAX_PRIMARY_CHILD_CONCURRENCY_RUNTIME.store(limit.max(1), Ordering::Release);
}

fn max_primary_child_concurrency_limit() -> usize {
    MAX_PRIMARY_CHILD_CONCURRENCY_RUNTIME
        .load(Ordering::Acquire)
        .max(1)
}

/// Current primary live-child cap for this process.
pub fn max_child_concurrency() -> usize {
    max_primary_child_concurrency_limit()
}

/// Configure the process-wide validator live-child cap. Last write wins.
pub fn set_max_validator_child_concurrency(limit: usize) {
    MAX_VALIDATOR_CHILD_CONCURRENCY_RUNTIME.store(limit.max(1), Ordering::Release);
}

fn max_validator_child_concurrency_limit() -> usize {
    MAX_VALIDATOR_CHILD_CONCURRENCY_RUNTIME
        .load(Ordering::Acquire)
        .max(1)
}

/// Current validator live-child cap for this process.
pub fn max_validator_child_concurrency() -> usize {
    max_validator_child_concurrency_limit()
}

fn primary_slots() -> &'static Mutex<usize> {
    PRIMARY_CHILD_SLOTS.get_or_init(|| Mutex::new(0))
}

fn validator_slots() -> &'static Mutex<usize> {
    VALIDATOR_CHILD_SLOTS.get_or_init(|| Mutex::new(0))
}

fn wait_poll_timeout(deadline: Instant) -> Duration {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Duration::ZERO;
    }
    remaining.min(Duration::from_millis(RSS_POLL_MS))
}

fn live_child_pids() -> &'static Mutex<Vec<u32>> {
    LIVE_CHILD_PIDS.get_or_init(|| Mutex::new(Vec::new()))
}

fn register_live_child_pid(pid: u32) {
    if let Ok(mut pids) = live_child_pids().lock() {
        pids.push(pid);
    }
}

fn unregister_live_child_pid(pid: u32) {
    if let Ok(mut pids) = live_child_pids().lock() {
        pids.retain(|p| *p != pid);
    }
}

/// Sum VmRSS across registered live helper children (best-effort `/proc` read).
pub fn sum_live_children_rss_bytes() -> u64 {
    let pids = live_child_pids()
        .lock()
        .map(|guard| guard.clone())
        .unwrap_or_default();
    pids.iter().filter_map(|pid| child_rss_bytes(*pid)).sum()
}

#[cfg(feature = "test-hang")]
pub fn live_child_pids_for_tests() -> Vec<u32> {
    live_child_pids()
        .lock()
        .map(|guard| guard.clone())
        .unwrap_or_default()
}

/// Which live-child pool a spawn consumes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChildSlotKind {
    #[default]
    Primary,
    Validator,
}

struct SlotGuard {
    kind: ChildSlotKind,
}

impl SlotGuard {
    fn try_acquire(kind: ChildSlotKind) -> Result<Self, EngineReport> {
        let (mutex, cap, label) = match kind {
            ChildSlotKind::Primary => (
                primary_slots(),
                max_primary_child_concurrency_limit(),
                "primary collab children",
            ),
            ChildSlotKind::Validator => (
                validator_slots(),
                max_validator_child_concurrency_limit(),
                "validator collab children",
            ),
        };
        let mut used = mutex.lock().map_err(|_| {
            worker_fail(WorkerFailureReason::SlotPoison, "child slot mutex poisoned")
        })?;
        if *used < cap {
            *used += 1;
            return Ok(Self { kind });
        }
        Err(EngineReport::new(EngineStatus::ResourceLimit {
            kind: LimitKind::Ops,
            detail: format!("live {label} at cap {cap}"),
        }))
    }

    fn acquire_with_wait(kind: ChildSlotKind, wait: Duration) -> Result<Self, EngineReport> {
        let deadline = Instant::now() + wait;
        loop {
            match Self::try_acquire(kind) {
                Ok(guard) => return Ok(guard),
                Err(report) => {
                    if !matches!(
                        report.outcome,
                        EngineStatus::ResourceLimit {
                            kind: LimitKind::Ops,
                            ..
                        }
                    ) {
                        return Err(report);
                    }
                    if Instant::now() >= deadline {
                        return Err(report);
                    }
                    thread::sleep(wait_poll_timeout(deadline));
                }
            }
        }
    }
}

impl Drop for SlotGuard {
    fn drop(&mut self) {
        let mutex = match self.kind {
            ChildSlotKind::Primary => primary_slots(),
            ChildSlotKind::Validator => validator_slots(),
        };
        if let Ok(mut used) = mutex.lock() {
            *used = used.saturating_sub(1);
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SpawnPhaseTimings {
    pub slot_wait_us: u64,
    pub spawn_us: u64,
}

#[derive(Debug, Clone)]
pub struct SpawnRequest {
    pub engine_bin: PathBuf,
    pub limits: Limits,
    pub slot_kind: ChildSlotKind,
    /// When set for [`ChildSlotKind::Validator`], block up to this duration for a slot.
    pub slot_wait: Option<Duration>,
    pub test_hang_ms: Option<u64>,
    /// `--features test-hang` only: child exits with this code after one request frame.
    pub test_exit_after_read: Option<i32>,
    /// `--features test-hang` only: close stdout then stay alive for this many ms.
    pub test_close_stdout_hang_ms: Option<u64>,
    /// `--features test-hang` only: write one response frame then exit with this code.
    pub test_exit_after_write: Option<i32>,
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
        Self::spawn_with_timings(req).map(|(session, _)| session)
    }

    pub fn spawn_with_timings(
        req: SpawnRequest,
    ) -> Result<(Self, SpawnPhaseTimings), EngineReport> {
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
            let slot_started = Instant::now();
            let slot = match (req.slot_kind, req.slot_wait) {
                (ChildSlotKind::Primary, _) => SlotGuard::try_acquire(ChildSlotKind::Primary)?,
                (ChildSlotKind::Validator, Some(wait)) => {
                    SlotGuard::acquire_with_wait(ChildSlotKind::Validator, wait)?
                }
                (ChildSlotKind::Validator, None) => {
                    SlotGuard::try_acquire(ChildSlotKind::Validator)?
                }
            };
            let slot_wait_us = slot_started.elapsed().as_micros() as u64;
            let spawn_started = Instant::now();
            let session = spawn_child(req, slot)?;
            let spawn_us = spawn_started.elapsed().as_micros() as u64;
            Ok((
                session,
                SpawnPhaseTimings {
                    slot_wait_us,
                    spawn_us,
                },
            ))
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
            match rx.recv_timeout(wait_poll_timeout(deadline)) {
                Ok((result, stdin)) => {
                    live.stdin = Some(stdin);
                    let _ = writer.join();
                    #[cfg(feature = "test-hang")]
                    add_helpers_joined(1);
                    return match result {
                        Ok(()) => Ok(()),
                        Err(err) => Err(classify_pipe_close(
                            live,
                            pid,
                            deadline,
                            format!("write frame: {err}"),
                        )),
                    };
                }
                Err(RecvTimeoutError::Timeout) => {
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
                        Ok(None) => {}
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
                Err(RecvTimeoutError::Disconnected) => {
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
            if Instant::now() >= deadline {
                let _ = live.child.kill();
                let _ = live.child.wait();
                let _ = reader.join();
                #[cfg(feature = "test-hang")]
                add_helpers_joined(1);
                break Err(EngineReport::new(EngineStatus::ResourceLimit {
                    kind: LimitKind::Time,
                    detail: format!("child pid {pid} exceeded wall timeout; killed and reaped"),
                })
                .with_child_pid(pid));
            }
            match rx.recv_timeout(wait_poll_timeout(deadline)) {
                Ok((result, stdout)) => {
                    live.stdout = Some(stdout);
                    let _ = reader.join();
                    #[cfg(feature = "test-hang")]
                    add_helpers_joined(1);
                    break match result {
                        Ok(Some(buf)) => parse_child_json(&buf, pid),
                        Ok(None) => Err(classify_pipe_close(
                            live,
                            pid,
                            deadline,
                            "child closed stdout before a frame".into(),
                        )),
                        Err(FrameError::TooLarge { len, max }) => {
                            Err(EngineReport::new(EngineStatus::ResourceLimit {
                                kind: LimitKind::Frame,
                                detail: format!("child stdout frame {len} exceeds {max}"),
                            })
                            .with_child_pid(pid))
                        }
                        Err(FrameError::Io(detail)) => {
                            Err(classify_pipe_close(live, pid, deadline, detail))
                        }
                    };
                }
                Err(RecvTimeoutError::Timeout) => match live.child.try_wait() {
                    Ok(Some(status)) => {
                        let _ = reader.join();
                        #[cfg(feature = "test-hang")]
                        add_helpers_joined(1);
                        break delivered_frame_or_exit(live, &rx, status, pid);
                    }
                    Ok(None) => {
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
                Err(RecvTimeoutError::Disconnected) => {
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
            unregister_live_child_pid(self.pid);
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
        .arg("--max-project-json")
        .arg(req.limits.max_project_json_bytes.to_string())
        .arg("--max-project-depth")
        .arg(req.limits.max_project_depth.to_string())
        .arg("--max-project-nodes")
        .arg(req.limits.max_project_nodes.to_string())
        .arg("--max-project-string")
        .arg(req.limits.max_project_string_bytes.to_string())
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
    #[cfg(feature = "test-hang")]
    if let Some(code) = req.test_exit_after_read {
        cmd.arg("--test-exit-after-read").arg(code.to_string());
    }
    #[cfg(feature = "test-hang")]
    if let Some(ms) = req.test_close_stdout_hang_ms {
        cmd.arg("--test-close-stdout-then-hang-ms")
            .arg(ms.to_string());
    }
    #[cfg(feature = "test-hang")]
    if let Some(code) = req.test_exit_after_write {
        cmd.arg("--test-exit-after-write").arg(code.to_string());
    }
    #[cfg(not(feature = "test-hang"))]
    let _ = (
        req.test_hang_ms,
        req.test_exit_after_read,
        req.test_close_stdout_hang_ms,
        req.test_exit_after_write,
    );

    let mut child = cmd.spawn().map_err(|err| {
        worker_fail(
            WorkerFailureReason::Spawn,
            format!("failed to spawn {:?}: {err}", req.engine_bin),
        )
    })?;

    let pid = child.id();
    register_live_child_pid(pid);
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

type FrameRead = (
    Result<Option<Vec<u8>>, FrameError>,
    std::process::ChildStdout,
);

/// If the child already wrote a complete frame, that reply wins over the exit
/// status. Discarding it used to turn a valid `Frame`/`Ok` into `ChildCrash`.
fn delivered_frame_or_exit(
    live: &mut LiveChild,
    rx: &mpsc::Receiver<FrameRead>,
    status: std::process::ExitStatus,
    pid: u32,
) -> Result<EngineReport, EngineReport> {
    if let Ok((result, stdout)) = rx.try_recv() {
        live.stdout = Some(stdout);
        match result {
            Ok(Some(buf)) => {
                if live.stderr_join.is_some() {
                    join_any(live.stderr_join.take());
                    #[cfg(feature = "test-hang")]
                    add_helpers_joined(1);
                }
                return parse_child_json(&buf, pid);
            }
            Ok(None) => {}
            Err(FrameError::TooLarge { len, max }) => {
                if live.stderr_join.is_some() {
                    join_any(live.stderr_join.take());
                    #[cfg(feature = "test-hang")]
                    add_helpers_joined(1);
                }
                return Err(EngineReport::new(EngineStatus::ResourceLimit {
                    kind: LimitKind::Frame,
                    detail: format!("child stdout frame {len} exceeds {max}"),
                })
                .with_child_pid(pid));
            }
            Err(FrameError::Io(_)) => {}
        }
    }
    let stderr = join_pipe(live.stderr_join.take());
    #[cfg(feature = "test-hang")]
    add_helpers_joined(1);
    Err(classify_child_exit(status, pid, &stderr))
}

/// EOF/IO on a child pipe is not itself a protocol verdict. A crashing child
/// closes stdout as it dies; reporting `Protocol` there races `try_wait`'s
/// `ChildCrash` / rlimit mapping. Poll for an observed exit only until the
/// same request deadline so a live child that closed stdout cannot hang us.
fn classify_pipe_close(
    live: &mut LiveChild,
    pid: u32,
    deadline: Instant,
    protocol_detail: String,
) -> EngineReport {
    loop {
        match live.child.try_wait() {
            Ok(Some(status)) => {
                let stderr = join_pipe(live.stderr_join.take());
                #[cfg(feature = "test-hang")]
                add_helpers_joined(1);
                return classify_child_exit(status, pid, &stderr);
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = live.child.kill();
                    let _ = live.child.wait();
                    if live.stderr_join.is_some() {
                        join_any(live.stderr_join.take());
                        #[cfg(feature = "test-hang")]
                        add_helpers_joined(1);
                    }
                    return worker_fail(WorkerFailureReason::Protocol, protocol_detail)
                        .with_child_pid(pid);
                }
                thread::sleep(wait_poll_timeout(deadline));
            }
            Err(err) => {
                let _ = live.child.kill();
                let _ = live.child.wait();
                if live.stderr_join.is_some() {
                    join_any(live.stderr_join.take());
                    #[cfg(feature = "test-hang")]
                    add_helpers_joined(1);
                }
                return worker_fail(WorkerFailureReason::Wait, format!("wait failed: {err}"))
                    .with_child_pid(pid);
            }
        }
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
            if sig == 6 && stack_overflow_stderr(stderr_text) {
                return EngineReport::new(EngineStatus::ResourceLimit {
                    kind: LimitKind::Stack,
                    detail: format!("child pid {pid} SIGABRT after stack overflow; {stderr_snip}"),
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

fn stack_overflow_stderr(stderr: &str) -> bool {
    stderr.to_ascii_lowercase().contains("overflowed its stack")
}

fn worker_fail(reason: WorkerFailureReason, detail: impl Into<String>) -> EngineReport {
    EngineReport::new(EngineStatus::WorkerFailure {
        reason,
        detail: detail.into(),
    })
}

/// Raise the soft `RLIMIT_NOFILE` to the hard ceiling for the server process.
pub fn raise_nofile_to_hard_limit() {
    #[cfg(target_os = "linux")]
    unsafe {
        let mut lim = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        if libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) != 0 {
            return;
        }
        if lim.rlim_max > lim.rlim_cur {
            lim.rlim_cur = lim.rlim_max;
            let _ = libc::setrlimit(libc::RLIMIT_NOFILE, &lim);
        }
    }
}

/// Raise `oom_score_adj` so cgroup/kernel OOM prefers helpers over the parent server.
fn apply_child_oom_score_adj() -> std::io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        let pid = std::process::id();
        let path = format!("/proc/{pid}/oom_score_adj\0");
        let fd = unsafe { libc::open(path.as_ptr() as *const libc::c_char, libc::O_WRONLY) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let value = b"1000";
        let written =
            unsafe { libc::write(fd, value.as_ptr() as *const libc::c_void, value.len()) };
        let close_err = unsafe { libc::close(fd) };
        if written < 0 || close_err != 0 {
            return Err(std::io::Error::last_os_error());
        }
        if written as usize != value.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WriteZero,
                "short write to oom_score_adj",
            ));
        }
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        Ok(())
    }
}

#[cfg(feature = "test-hang")]
pub fn child_oom_score_adj(pid: u32) -> Option<i32> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/oom_score_adj")).ok()?;
    text.trim().parse().ok()
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
        let cpu_secs = limits.cpu_budget_secs();
        unsafe {
            cmd.pre_exec(move || {
                apply_rlimits_now(as_bytes, cpu_secs, stack_bytes)?;
                apply_child_oom_score_adj()?;
                Ok(())
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

#[cfg(all(test, unix))]
mod classify_exit_tests {
    use super::{classify_child_exit, stack_overflow_stderr};
    use crate::outcome::{EngineStatus, LimitKind};
    use std::os::unix::process::ExitStatusExt;
    use std::process::ExitStatus;

    #[test]
    fn sigabrt_stack_overflow_is_stack_limit() {
        let status = ExitStatus::from_raw(6);
        let stderr = Ok(b"thread 'main' has overflowed its stack\n".to_vec());
        let report = classify_child_exit(status, 42, &stderr);
        assert!(
            matches!(
                report.outcome,
                EngineStatus::ResourceLimit {
                    kind: LimitKind::Stack,
                    ..
                }
            ),
            "{:?}",
            report.outcome
        );
        assert!(stack_overflow_stderr(
            "thread 'main' has overflowed its stack"
        ));
    }

    #[test]
    fn sigabrt_allocator_failure_is_memory_limit() {
        let status = ExitStatus::from_raw(6);
        let stderr = Ok(b"memory allocation of 216 bytes failed\n".to_vec());
        let report = classify_child_exit(status, 42, &stderr);
        assert!(
            matches!(
                report.outcome,
                EngineStatus::ResourceLimit {
                    kind: LimitKind::Memory,
                    ..
                }
            ),
            "{:?}",
            report.outcome
        );
    }
}
