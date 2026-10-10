#![allow(dead_code)]

use std::io::{BufRead, BufReader, Read};
use std::net::SocketAddr;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use fvoci_server::collab::config::derive_max_child_concurrency;
use uuid::Uuid;

use super::{TestDb, PEPPER, PUBLIC_ORIGIN};

const LOG_PUMP_EOF_WITHIN: Duration = Duration::from_secs(5);

/// A spawned server that leads its own process group, so the collab-engine
/// helpers it spawns (same group: the product does not setsid them) die with
/// it on cleanup. A terminal Ctrl-C reaches only the test process, not this
/// group, and a signal-killed test runs no Drop; so the server also carries
/// `PR_SET_PDEATHSIG(SIGKILL)` and dies with the test thread that spawned it
/// (its helpers then die by their own parent-death signal). Spawn only from a
/// thread that outlives the handle (a test thread or the runtime's block_on
/// thread, never a blocking-pool thread).
pub struct OwnedChild {
    pub child: Option<Child>,
    /// Helpers a test observed, kept for its own `wait_pids_exit` assertion.
    /// Cleanup does not need it: they share the server's process group,
    /// which `kill_and_wait` SIGKILLs.
    pub helper_pids: Vec<u32>,
    pgid: libc::pid_t,
    log_pumps: Vec<std::thread::JoinHandle<()>>,
}

/// `kill(2)`; `ESRCH` (no such process or group) is `Ok(false)`.
fn send_signal(target: libc::pid_t, signal: libc::c_int) -> std::io::Result<bool> {
    // SAFETY: kill(2) takes plain integers and touches no memory.
    if unsafe { libc::kill(target, signal) } == 0 {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        Ok(false)
    } else {
        Err(error)
    }
}

/// A cleanup failure fails the test, except while already unwinding (Drop
/// after an earlier panic), where a second panic would abort and hide the
/// first one.
fn cleanup_failure(message: String) {
    if std::thread::panicking() {
        eprintln!("{message}");
    } else {
        panic!("{message}");
    }
}

impl OwnedChild {
    pub fn spawn(command: &mut Command) -> std::io::Result<Self> {
        // SAFETY: getpid has no preconditions.
        let parent = unsafe { libc::getpid() };
        // SAFETY: the hook only calls async-signal-safe prctl/getppid and
        // builds errors from raw errno values (no allocation).
        unsafe {
            command.pre_exec(move || {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL, 0, 0, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                // The test died before the death signal was armed.
                if libc::getppid() != parent {
                    return Err(std::io::Error::from_raw_os_error(libc::ESRCH));
                }
                Ok(())
            });
        }
        let child = command.process_group(0).spawn()?;
        let pgid = libc::pid_t::try_from(child.id()).expect("pid fits pid_t");
        Ok(Self {
            child: Some(child),
            helper_pids: Vec::new(),
            pgid,
            log_pumps: Vec::new(),
        })
    }

    pub fn pid(&self) -> Option<u32> {
        self.child.as_ref().map(Child::id)
    }

    /// The leader's exit status if it has exited, WITHOUT reaping it
    /// (`waitid(WNOWAIT)`): the zombie leader keeps its pid and group id
    /// pinned, so later signals here can never reach a recycled id. Only
    /// `kill_and_wait` reaps.
    pub fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        use std::os::unix::process::ExitStatusExt;
        let Some(child) = self.child.as_ref() else {
            return Ok(None);
        };
        // SAFETY: an all-zero siginfo_t is valid; waitid only writes into it.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let options = libc::WEXITED | libc::WNOHANG | libc::WNOWAIT;
        // SAFETY: `info` is a valid, exclusively borrowed siginfo_t.
        if unsafe { libc::waitid(libc::P_PID, child.id(), &mut info, options) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: waitid filled a SIGCHLD siginfo (or left it zeroed).
        let (pid, status) = unsafe { (info.si_pid(), info.si_status()) };
        if pid == 0 {
            return Ok(None);
        }
        let raw = match info.si_code {
            libc::CLD_EXITED => (status & 0xff) << 8,
            libc::CLD_KILLED => status,
            libc::CLD_DUMPED => status | 0x80,
            code => {
                return Err(std::io::Error::other(format!(
                    "unexpected waitid si_code {code} for pid {pid}"
                )))
            }
        };
        Ok(Some(ExitStatus::from_raw(raw)))
    }

    /// SIGTERM to the server only: the tests observe the server's own helper
    /// drain, so the helpers must not be signalled directly.
    pub fn send_sigterm(&self) {
        let pid = self
            .pid()
            .expect("no live server handle to send SIGTERM to");
        let pid = libc::pid_t::try_from(pid).expect("pid fits pid_t");
        match send_signal(pid, libc::SIGTERM) {
            Ok(true) => {}
            Ok(false) => panic!("SIGTERM to server pid {pid}: no such process"),
            Err(error) => panic!("SIGTERM to server pid {pid}: {error}"),
        }
    }

    /// SIGKILL to the server only (not its group), so whatever kills the
    /// helpers afterwards is the product's own parent-death handling.
    pub fn kill_leader(&mut self) {
        let child = self
            .child
            .as_mut()
            .expect("no live server handle to SIGKILL");
        if let Err(error) = child.kill() {
            panic!("SIGKILL to server pid {}: {error}", child.id());
        }
    }

    fn join_log_pumps_within(&mut self, within: Duration) -> bool {
        let deadline = Instant::now() + within;
        while self.log_pumps.iter().any(|pump| !pump.is_finished()) {
            if Instant::now() >= deadline {
                self.log_pumps.clear();
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        for pump in self.log_pumps.drain(..) {
            let _ = pump.join();
        }
        true
    }

    /// SIGKILLs the whole process group (any helper still alive, even after
    /// the server itself exited), then reaps the leader. Safe because nothing
    /// but this call reaps the leader: until then its zombie pins the group
    /// id. Returns the leader's status; `None` only after an earlier call.
    pub fn kill_and_wait(&mut self) -> Option<ExitStatus> {
        let mut reaped = None;
        if let Some(mut child) = self.child.take() {
            if let Err(error) = send_signal(-self.pgid, libc::SIGKILL) {
                cleanup_failure(format!(
                    "SIGKILL to server process group {}: {error}",
                    self.pgid
                ));
            }
            match child.wait() {
                Ok(status) => reaped = Some(status),
                Err(error) => cleanup_failure(format!("reap server pid {}: {error}", self.pgid)),
            }
        }
        let _ = self.join_log_pumps_within(LOG_PUMP_EOF_WITHIN);
        reaped
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        self.kill_and_wait();
    }
}

// The /proc reads below observe the helpers themselves (which processes a
// room owns and whether they exited with their server); that observation is
// what the calling tests assert. Kill/reap goes through the handle above.

fn process_comm(pid: u32) -> Option<String> {
    std::fs::read_to_string(format!("/proc/{pid}/comm"))
        .ok()
        .map(|s| s.trim().to_string())
}

fn direct_children(pid: u32) -> Vec<u32> {
    let mut pids = Vec::new();
    let Ok(entries) = std::fs::read_dir(format!("/proc/{pid}/task")) else {
        return pids;
    };
    for entry in entries.flatten() {
        if let Ok(text) = std::fs::read_to_string(entry.path().join("children")) {
            for token in text.split_whitespace() {
                if let Ok(child) = token.parse::<u32>() {
                    pids.push(child);
                }
            }
        }
    }
    pids.sort_unstable();
    pids.dedup();
    pids
}

pub fn collab_engine_descendants(root: u32) -> Vec<u32> {
    let mut found = Vec::new();
    let mut stack = vec![root];
    let mut seen = std::collections::HashSet::new();
    while let Some(pid) = stack.pop() {
        if !seen.insert(pid) {
            continue;
        }
        for child in direct_children(pid) {
            stack.push(child);
            if process_comm(child).is_some_and(|comm| comm.starts_with("collab-engine")) {
                found.push(child);
            }
        }
    }
    found
}

fn pid_alive(pid: u32) -> bool {
    PathBuf::from(format!("/proc/{pid}")).exists()
}

pub fn wait_pids_exit(pids: &[u32], within: Duration) {
    let deadline = Instant::now() + within;
    loop {
        if pids.iter().all(|pid| !pid_alive(*pid)) {
            return;
        }
        if Instant::now() >= deadline {
            let live: Vec<u32> = pids.iter().copied().filter(|pid| pid_alive(*pid)).collect();
            panic!("helper pids still live after parent exit: {live:?}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn server_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_fvoci-server"))
}

/// The helper the process server runs is the one the caller selected
/// explicitly (CI: the sealed `--features worker` build). No unset fallback:
/// the fallback's `crates/collab-engine/target/debug/collab-engine` is also
/// written by a local `cargo test --features test-hang` there, with test-only
/// helper controls compiled in. Set-but-unusable values are rejected by the
/// product's own resolver.
fn collab_engine_bin() -> PathBuf {
    let Some(selected) = std::env::var_os("FVOCI_COLLAB_ENGINE") else {
        panic!(
            "process-server tests require FVOCI_COLLAB_ENGINE; build the helper with `cargo build --locked --offline --manifest-path crates/collab-engine/Cargo.toml --features worker --bin collab-engine` and point the variable at it"
        );
    };
    fvoci_server::collab::config::collab_engine_path_for_tests().unwrap_or_else(|| {
        panic!("FVOCI_COLLAB_ENGINE={selected:?} is not an existing regular file")
    })
}

fn pump_lines<R: Read + Send + 'static>(
    stream: R,
    logs: Arc<Mutex<Vec<String>>>,
    tx: mpsc::Sender<String>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let reader = BufReader::new(stream);
        for line in reader.lines().map_while(Result::ok) {
            if let Ok(mut held) = logs.lock() {
                held.push(line.clone());
            }
            let _ = tx.send(line);
        }
    })
}

pub fn collected_log_text(logs: &Arc<Mutex<Vec<String>>>) -> String {
    logs.lock().expect("server logs").join("\n")
}

fn headers_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|i| i + 4)
}

pub fn wait_for_http_100_continue(stream: &mut std::net::TcpStream, within: Duration) {
    let deadline = Instant::now() + within;
    let mut buf = Vec::new();
    let mut tmp = [0u8; 512];
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            panic!(
                "did not receive HTTP 100 Continue within {within:?}; got {:?}",
                String::from_utf8_lossy(&buf)
            );
        }
        stream
            .set_read_timeout(Some(remaining))
            .expect("100 Continue read timeout");
        match stream.read(&mut tmp) {
            Ok(0) => panic!(
                "server closed before HTTP 100 Continue; got {:?}",
                String::from_utf8_lossy(&buf)
            ),
            Ok(n) => {
                buf.extend_from_slice(&tmp[..n]);
                if let Some(end) = headers_end(&buf) {
                    let head = String::from_utf8_lossy(&buf[..end]);
                    let status_line = head.lines().next().unwrap_or("");
                    if status_line.starts_with("HTTP/1.1 100 ")
                        || status_line.starts_with("HTTP/1.0 100 ")
                    {
                        return;
                    }
                    panic!(
                        "expected HTTP 100 Continue proving body poll; server responded {status_line:?} headers={head:?}"
                    );
                }
            }
            Err(err)
                if err.kind() == std::io::ErrorKind::WouldBlock
                    || err.kind() == std::io::ErrorKind::TimedOut
                    || err.kind() == std::io::ErrorKind::Interrupted =>
            {
                continue;
            }
            Err(err) => panic!(
                "read HTTP 100 Continue: {err}; got {:?}",
                String::from_utf8_lossy(&buf)
            ),
        }
    }
}

pub fn assert_nonzero_deadline_exit(
    status: ExitStatus,
    elapsed: Duration,
    deadline_ms: u64,
    log_text: &str,
    label: &str,
) {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        assert!(
            status.signal().is_none(),
            "{label} must exit from the process, not a default signal kill (got {status}); logs={log_text}"
        );
    }
    assert!(
        !status.success(),
        "{label} must be nonzero, got {status}; logs={log_text}"
    );
    assert!(
        elapsed < Duration::from_millis(deadline_ms + 1_500),
        "{label} must be bound by the deadline, elapsed={elapsed:?} deadline={deadline_ms}ms"
    );
    assert!(
        log_text.contains("shutdown deadline exceeded")
            || log_text.contains("server shutdown deadline exceeded"),
        "{label} must report deadline failure, logs={log_text}"
    );
}

fn spawn_server_process_inner(
    harness: &TestDb,
    deadline_ms: u64,
    auth_wait_ms: Option<u64>,
    extra_env: &[(&str, String)],
) -> (OwnedChild, SocketAddr, Arc<Mutex<Vec<String>>>) {
    let engine = collab_engine_bin();
    let logs = Arc::new(Mutex::new(Vec::new()));
    let storage_root =
        std::env::temp_dir().join(format!("fvoci-collab-process-store-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&storage_root).expect("collab process storage root");
    let mut command = Command::new(server_bin());
    command
        .env("DATABASE_APP_URL", &harness.app_url)
        .env_remove("DATABASE_URL")
        .env_remove("FVOCI_MIGRATION_URL")
        .env("PASSWORD_PEPPER_KEYS", PEPPER)
        .env("PASSWORD_PEPPER_ACTIVE_KEY_ID", "test")
        .env("FVOCI_BIND", "127.0.0.1:0")
        .env("FVOCI_PUBLIC_ORIGIN", PUBLIC_ORIGIN)
        .env("FVOCI_COOKIE_SECURE", "0")
        .env("FVOCI_COLLAB_ENGINE", &engine)
        .env("FVOCI_STORAGE_DIR", &storage_root)
        .env("FVOCI_SHUTDOWN_DEADLINE_MS", deadline_ms.to_string())
        .env("FVOCI_COLLAB_IDLE_MS", "60000")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(auth_wait_ms) = auth_wait_ms {
        command.env("FVOCI_COLLAB_AUTH_WAIT_MS", auth_wait_ms.to_string());
    }
    for (key, value) in extra_env {
        command.env(key, value);
    }
    let mut owned = OwnedChild::spawn(&mut command).expect("spawn fvoci-server");
    let child = owned.child.as_mut().expect("spawned child");
    let stderr = child.stderr.take().expect("stderr");
    let stdout = child.stdout.take().expect("stdout");
    let (tx, rx) = mpsc::channel::<String>();
    owned.log_pumps = vec![
        pump_lines(stderr, logs.clone(), tx.clone()),
        pump_lines(stdout, logs.clone(), tx),
    ];
    let startup = Duration::from_secs(10);
    let deadline = Instant::now() + startup;
    let mut listen = None;
    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(line) => {
                if let Some(rest) = line.strip_prefix("fvoci-server listening on ") {
                    listen = Some(rest.trim().to_string());
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if owned.try_wait().ok().flatten().is_some() {
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let Some(url) = listen else {
        let exited = owned.try_wait();
        // Either way its group (any helper it already spawned) is killed.
        let reaped = owned.kill_and_wait();
        let outcome = match (exited, reaped) {
            (Ok(Some(status)), _) => format!("exited on its own before listening: {status}"),
            (Ok(None), Some(status)) => format!(
                "still not listening after {startup:?}; harness sent SIGKILL to its process group, reaped: {status}"
            ),
            (Ok(None), None) => "still not listening; reap failed".to_string(),
            (Err(error), _) => format!("exit check failed: {error}"),
        };
        panic!(
            "fvoci-server did not print listen address ({outcome}); logs={:?}",
            logs.lock().unwrap()
        );
    };
    let parsed = url::Url::parse(&url).expect("listen url");
    let addr = SocketAddr::new(
        parsed.host_str().expect("listen host").parse().expect("ip"),
        parsed.port().expect("listen port"),
    );
    (owned, addr, logs)
}

pub fn spawn_server_process(
    harness: &TestDb,
    deadline_ms: u64,
) -> (OwnedChild, SocketAddr, Arc<Mutex<Vec<String>>>) {
    spawn_server_process_with_auth_wait(harness, deadline_ms, None)
}

pub fn spawn_server_process_with_auth_wait(
    harness: &TestDb,
    deadline_ms: u64,
    auth_wait_ms: Option<u64>,
) -> (OwnedChild, SocketAddr, Arc<Mutex<Vec<String>>>) {
    spawn_server_process_inner(harness, deadline_ms, auth_wait_ms, &[])
}

/// `spawn_server_process` with extra or overriding environment variables.
pub fn spawn_server_process_with_env(
    harness: &TestDb,
    deadline_ms: u64,
    extra_env: &[(&str, String)],
) -> (OwnedChild, SocketAddr, Arc<Mutex<Vec<String>>>) {
    spawn_server_process_inner(harness, deadline_ms, None, extra_env)
}

pub fn spawn_capacity_probe_server_process(
    harness: &TestDb,
    max_rooms: usize,
    peers: usize,
    deadline_ms: u64,
) -> (OwnedChild, SocketAddr, Arc<Mutex<Vec<String>>>) {
    let max_connections = peers.max(2);
    let max_sockets = max_rooms * peers + 64;
    let memory_budget = 8_u64 * 1024 * 1024 * 1024;
    let max_children = derive_max_child_concurrency(max_rooms);
    spawn_server_process_inner(
        harness,
        deadline_ms,
        None,
        &[
            ("FVOCI_COLLAB_MAX_ROOMS", max_rooms.to_string()),
            (
                "FVOCI_COLLAB_MAX_CHILDREN",
                max_children.max(max_rooms).to_string(),
            ),
            ("FVOCI_COLLAB_MEMORY_BUDGET", memory_budget.to_string()),
            ("FVOCI_COLLAB_MAX_SOCKETS", max_sockets.to_string()),
            ("FVOCI_COLLAB_MAX_CONNECTIONS", max_connections.to_string()),
            ("FVOCI_COLLAB_IDLE_MS", "3600000".to_string()),
        ],
    )
}

pub fn wait_for_exit(child: &mut OwnedChild, within: Duration) -> ExitStatus {
    let deadline = Instant::now() + within;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                // The exited leader stays unreaped (see `try_wait`) until
                // `kill_and_wait`/Drop, which also SIGKILLs any helper left
                // in its group, including after this assertion fails.
                assert!(
                    child.join_log_pumps_within(LOG_PUMP_EOF_WITHIN),
                    "server exited ({status}) but its stdout/stderr stayed open for {LOG_PUMP_EOF_WITHIN:?}; a descendant inherited the pipes"
                );
                return status;
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let pid = child.pid();
                    let reaped = match child.kill_and_wait() {
                        Some(status) => status.to_string(),
                        None => "not reaped".to_string(),
                    };
                    panic!(
                        "server pid={pid:?} still running after {within:?}; harness sent SIGKILL to its process group, reaped: {reaped}"
                    );
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("wait for server: {error}"),
        }
    }
}
