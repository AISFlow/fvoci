#![allow(dead_code)]

use std::io::{BufRead, BufReader, Read};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use fvoci_server::collab::config::derive_max_child_concurrency;
use uuid::Uuid;

use super::{TestDb, PEPPER, PUBLIC_ORIGIN};

const LOG_PUMP_EOF_WITHIN: Duration = Duration::from_secs(5);

pub struct OwnedChild {
    pub child: Option<Child>,
    pub helper_pids: Vec<u32>,
    log_pumps: Vec<std::thread::JoinHandle<()>>,
}

impl OwnedChild {
    pub fn pid(&self) -> Option<u32> {
        self.child.as_ref().map(Child::id)
    }

    pub fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        match self.child.as_mut() {
            Some(child) => child.try_wait(),
            None => Ok(None),
        }
    }

    pub fn send_sigterm(&self) {
        let Some(pid) = self.pid() else {
            return;
        };
        let _ = Command::new("kill")
            .args(["-s", "TERM", &pid.to_string()])
            .status();
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

    pub fn kill_and_wait(&mut self) {
        if let Some(mut child) = self.child.take() {
            match child.try_wait() {
                Ok(Some(_)) => {}
                _ => {
                    let pid = child.id();
                    let _ = Command::new("kill")
                        .args(["-s", "TERM", &pid.to_string()])
                        .status();
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
        }
        self.helper_pids
            .retain(|pid| std::path::Path::new(&format!("/proc/{pid}")).exists());
        for helper in &self.helper_pids {
            let _ = Command::new("kill")
                .args(["-s", "KILL", &helper.to_string()])
                .status();
        }
        self.helper_pids.clear();
        let _ = self.join_log_pumps_within(LOG_PUMP_EOF_WITHIN);
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        self.kill_and_wait();
    }
}

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

fn collab_engine_bin() -> PathBuf {
    std::env::var("FVOCI_COLLAB_ENGINE")
        .ok()
        .map(PathBuf::from)
        .filter(|p| p.is_file())
        .unwrap_or_else(fvoci_server::collab::config::require_collab_engine_for_tests)
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
            "{label} must exit from the process, not a default signal kill ({status:?}); logs={log_text}"
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
    let mut child = command.spawn().expect("spawn fvoci-server");
    let stderr = child.stderr.take().expect("stderr");
    let stdout = child.stdout.take().expect("stdout");
    let (tx, rx) = mpsc::channel::<String>();
    let log_pumps = vec![
        pump_lines(stderr, logs.clone(), tx.clone()),
        pump_lines(stdout, logs.clone(), tx),
    ];
    let deadline = Instant::now() + Duration::from_secs(10);
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
                if child.try_wait().ok().flatten().is_some() {
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let addr = match listen {
        Some(url) => {
            let parsed = url::Url::parse(&url).expect("listen url");
            SocketAddr::new(
                parsed.host_str().expect("listen host").parse().expect("ip"),
                parsed.port().expect("listen port"),
            )
        }
        None => {
            let mut failed = OwnedChild {
                child: Some(child),
                helper_pids: Vec::new(),
                log_pumps,
            };
            failed.kill_and_wait();
            panic!(
                "fvoci-server did not print listen address; logs={:?}",
                logs.lock().unwrap()
            );
        }
    };
    (
        OwnedChild {
            child: Some(child),
            helper_pids: Vec::new(),
            log_pumps,
        },
        addr,
        logs,
    )
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
                child.child = None;
                assert!(
                    child.join_log_pumps_within(LOG_PUMP_EOF_WITHIN),
                    "server exited ({status}) but its stdout/stderr stayed open for {LOG_PUMP_EOF_WITHIN:?}; a descendant inherited the pipes"
                );
                return status;
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let logs_note = format!("pid={:?}", child.pid());
                    child.kill_and_wait();
                    panic!("server still running after {within:?} ({logs_note})");
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("wait for server: {error}"),
        }
    }
}
