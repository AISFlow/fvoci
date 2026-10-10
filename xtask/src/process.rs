//! Bounded child execution with the observable behaviour of Python
//! `subprocess.run(argv, timeout=..., capture_output=...)`: the direct child is
//! killed and reaped when the timeout expires, and both pipes are drained
//! concurrently so a chatty child cannot block on a full pipe.
//! `run_owned` is the stricter variant for new callers: a process group per
//! call and one deadline that also covers output collection.

use serde_json::{json, Value};
use std::io::{self, Read};
use std::os::unix::process::CommandExt;
use std::os::unix::process::ExitStatusExt;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Once;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Elapsed/timeout/exit record kept per step, like the former `TIMINGS` dicts.
#[derive(Clone, Debug, PartialEq)]
pub struct Timing {
    pub elapsed_seconds: f64,
    pub timeout_seconds: u64,
    /// Python `returncode`: exit code, `-signal`, or -1 when no status exists.
    pub exit_code: i64,
}

impl Timing {
    pub fn new(elapsed: Duration, timeout: Duration, exit_code: i64) -> Self {
        let elapsed = elapsed.as_secs_f64().min(86_400.0);
        Self {
            elapsed_seconds: (elapsed * 1e6).round() / 1e6,
            timeout_seconds: timeout.as_secs(),
            exit_code,
        }
    }

    pub fn to_json(&self) -> Value {
        // `min(elapsed, 86400)` yielded the integer 86400 when capped.
        let elapsed = if self.elapsed_seconds >= 86_400.0 {
            json!(86_400)
        } else {
            json!(self.elapsed_seconds)
        };
        json!({
            "elapsed_seconds": elapsed,
            "timeout_seconds": self.timeout_seconds,
            "exit_code": self.exit_code,
        })
    }

    /// `json.dumps(timing, sort_keys=True)` with Python's default separators.
    pub fn log_line(&self) -> String {
        let value = self.to_json();
        format!(
            "{{\"elapsed_seconds\": {}, \"exit_code\": {}, \"timeout_seconds\": {}}}",
            value["elapsed_seconds"], self.exit_code, self.timeout_seconds
        )
    }
}

/// Python-compatible return code: the exit code or the negated signal number.
pub fn returncode(status: ExitStatus) -> i64 {
    match (status.code(), status.signal()) {
        (Some(code), _) => i64::from(code),
        (None, Some(signal)) => -i64::from(signal),
        (None, None) => -1,
    }
}

#[derive(Debug)]
pub struct Finished {
    pub status: ExitStatus,
    /// Empty unless the call captured output.
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

#[derive(Debug)]
pub enum RunError {
    Spawn(io::Error),
    Wait(io::Error),
    /// An output pipe could not be read to the end.
    Read(io::Error),
    /// The child outlived its budget and was killed and reaped.
    Timeout(Duration),
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spawn(error) | Self::Wait(error) => write!(f, "{error}"),
            Self::Read(error) => write!(f, "cannot read output: {error}"),
            Self::Timeout(timeout) => write!(f, "timed out after {} seconds", timeout.as_secs()),
        }
    }
}

fn drain(source: Option<impl Read + Send + 'static>) -> Option<JoinHandle<Vec<u8>>> {
    source.map(|mut pipe| {
        thread::spawn(move || {
            let mut buffer = Vec::new();
            let _ = pipe.read_to_end(&mut buffer);
            buffer
        })
    })
}

fn wait_with_deadline(child: &mut Child, timeout: Duration) -> Result<ExitStatus, RunError> {
    let deadline = Instant::now() + timeout;
    let mut pause = Duration::from_millis(1);
    loop {
        if let Some(status) = child.try_wait().map_err(RunError::Wait)? {
            return Ok(status);
        }
        let now = Instant::now();
        if now >= deadline {
            // Only the direct child is signalled, as subprocess.run did.
            let _ = child.kill();
            child.wait().map_err(RunError::Wait)?;
            return Err(RunError::Timeout(timeout));
        }
        thread::sleep(pause.min(deadline - now));
        pause = (pause * 2).min(Duration::from_millis(50));
    }
}

/// Run `command` to completion within `timeout`. With `capture`, stdout and
/// stderr are collected; otherwise they are inherited. Stdin is inherited.
pub fn run(command: &mut Command, timeout: Duration, capture: bool) -> Result<Finished, RunError> {
    if capture {
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
    }
    let mut child = command.spawn().map_err(RunError::Spawn)?;
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());
    let status = wait_with_deadline(&mut child, timeout)?;
    // A grandchild holding a pipe open delays EOF; the status is already final.
    let collect = |handle: Option<JoinHandle<Vec<u8>>>| {
        handle
            .map(|h| h.join().unwrap_or_default())
            .unwrap_or_default()
    };
    Ok(Finished {
        status,
        stdout: collect(stdout),
        stderr: collect(stderr),
    })
}

/// Process group of the `run_owned` call in progress (0: none), for the
/// interrupt handler.
static OWNED_GROUP: AtomicI32 = AtomicI32::new(0);

/// SIGINT/SIGTERM/SIGHUP while an owned group runs: kill the group, then die
/// of the same signal (only async-signal-safe calls).
extern "C" fn interrupt(signal: libc::c_int) {
    let group = OWNED_GROUP.load(Ordering::SeqCst);
    unsafe {
        if group > 0 {
            libc::killpg(group, libc::SIGKILL);
        }
        libc::signal(signal, libc::SIG_DFL);
        libc::raise(signal);
    }
}

/// The owned group sits outside the terminal's foreground group, so a
/// terminal or CI interrupt reaches only us; forward it by killing the group.
/// A signal the caller ignores stays ignored.
fn forward_interrupts() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
            unsafe {
                let mut old: libc::sigaction = std::mem::zeroed();
                if libc::sigaction(signal, std::ptr::null(), &mut old) != 0
                    || old.sa_sigaction != libc::SIG_DFL
                {
                    continue;
                }
                let mut action: libc::sigaction = std::mem::zeroed();
                action.sa_sigaction = interrupt as *const () as libc::sighandler_t;
                libc::sigemptyset(&mut action.sa_mask);
                action.sa_flags = libc::SA_RESTART;
                libc::sigaction(signal, &action, std::ptr::null_mut());
            }
        }
    });
}

/// True once `pid` has exited, leaving it unreaped so its process group id
/// cannot be reused before `killpg`.
fn exited(pid: u32) -> io::Result<bool> {
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    let rc = unsafe {
        libc::waitid(
            libc::P_PID,
            pid as libc::id_t,
            &mut info,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { info.si_pid() } != 0)
}

/// Run `command` in its own process group and capture stdout and stderr; the
/// budget covers the whole call, output collection included. A descendant
/// that keeps a pipe open past the deadline is a timeout, not a success. When
/// the call ends (exit, timeout or error) every process left in the group is
/// killed and the direct child is reaped. A pipe read error or a lost reader
/// is an error, never empty output.
pub fn run_owned(command: &mut Command, timeout: Duration) -> Result<Finished, RunError> {
    let deadline = Instant::now() + timeout;
    forward_interrupts();
    command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut child = command.spawn().map_err(RunError::Spawn)?;
    let pid = child.id();
    OWNED_GROUP.store(pid as i32, Ordering::SeqCst);
    let (sender, results) = mpsc::channel::<(usize, io::Result<Vec<u8>>)>();
    let pipes: [Option<Box<dyn Read + Send>>; 2] = [
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    ];
    for (slot, pipe) in pipes.into_iter().enumerate() {
        let sender = sender.clone();
        let Some(mut pipe) = pipe else {
            let _ = sender.send((slot, Err(io::Error::other("pipe was not created"))));
            continue;
        };
        thread::spawn(move || {
            let mut buffer = Vec::new();
            let result = pipe.read_to_end(&mut buffer).map(|_| buffer);
            let _ = sender.send((slot, result));
        });
    }
    drop(sender);

    let mut output: [Option<Vec<u8>>; 2] = [None, None];
    let mut child_done = false;
    let outcome = loop {
        if !child_done {
            match exited(pid) {
                Ok(done) => child_done = done,
                Err(error) => break Err(RunError::Wait(error)),
            }
        }
        if child_done && output.iter().all(Option::is_some) {
            break Ok(());
        }
        let now = Instant::now();
        if now >= deadline {
            break Err(RunError::Timeout(timeout));
        }
        match results.recv_timeout((deadline - now).min(Duration::from_millis(10))) {
            Ok((slot, Ok(bytes))) => output[slot] = Some(bytes),
            Ok((_, Err(error))) => break Err(RunError::Read(error)),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                if output.iter().any(Option::is_none) {
                    break Err(RunError::Read(io::Error::other(
                        "an output reader was lost",
                    )));
                }
            }
        }
    };
    // The child is not reaped yet, so the group id is still ours.
    unsafe {
        libc::killpg(pid as libc::pid_t, libc::SIGKILL);
    }
    let reaped = child.wait();
    OWNED_GROUP.store(0, Ordering::SeqCst);
    outcome?;
    let status = reaped.map_err(RunError::Wait)?;
    let [stdout, stderr] = output.map(Option::unwrap_or_default);
    Ok(Finished {
        status,
        stdout,
        stderr,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owned_budget_covers_a_descendant_holding_the_pipe() {
        let started = Instant::now();
        let error = run_owned(
            Command::new("sh").args(["-c", "sleep 1.2 & printf done; exit 0"]),
            Duration::from_millis(100),
        )
        .unwrap_err();
        assert!(matches!(error, RunError::Timeout(_)), "{error}");
        assert!(started.elapsed() < Duration::from_millis(1000));
    }

    #[test]
    fn owned_timeout_kills_the_whole_group() {
        let dir = std::env::temp_dir().join(format!("xtask-owned-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let marker = dir.join("pid");
        let script = format!("sleep 30 & echo $! > '{}'; wait", marker.display());
        let error = run_owned(
            Command::new("sh").args(["-c", &script]),
            Duration::from_millis(300),
        )
        .unwrap_err();
        assert!(matches!(error, RunError::Timeout(_)));
        let pid: libc::pid_t = std::fs::read_to_string(&marker)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        // Killed by the group SIGKILL; reparented and reaped by init, so the
        // pid is gone or at most a zombie for a moment.
        let gone = (0..200).any(|_| {
            let state = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
            let alive = state
                .rsplit_once(") ")
                .is_some_and(|(_, rest)| !rest.starts_with('Z'));
            if alive {
                thread::sleep(Duration::from_millis(10));
            }
            !alive
        });
        assert!(gone, "background sleep {pid} survived the group kill");
    }

    #[test]
    fn owned_captures_status_and_both_streams() {
        let done = run_owned(
            Command::new("sh").args(["-c", "printf out; printf err >&2; exit 3"]),
            Duration::from_secs(10),
        )
        .unwrap();
        assert_eq!(done.stdout, b"out");
        assert_eq!(done.stderr, b"err");
        assert_eq!(returncode(done.status), 3);
        let killed = run_owned(
            Command::new("sh").args(["-c", "kill -9 $$"]),
            Duration::from_secs(10),
        )
        .unwrap();
        assert_eq!(returncode(killed.status), -9);
        let large = run_owned(
            Command::new("sh").args([
                "-c",
                "head -c 1000000 /dev/zero; head -c 1000000 /dev/zero >&2",
            ]),
            Duration::from_secs(20),
        )
        .unwrap();
        assert_eq!(
            (large.stdout.len(), large.stderr.len()),
            (1_000_000, 1_000_000)
        );
    }

    #[test]
    fn timeout_kills_reaps_and_reports() {
        let started = Instant::now();
        let error = run(
            Command::new("sleep").arg("30"),
            Duration::from_millis(200),
            true,
        )
        .unwrap_err();
        assert!(matches!(error, RunError::Timeout(_)));
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn captures_both_streams_and_status() {
        let done = run(
            Command::new("sh").args(["-c", "printf out; printf err >&2; exit 3"]),
            Duration::from_secs(10),
            true,
        )
        .unwrap();
        assert_eq!(done.stdout, b"out");
        assert_eq!(done.stderr, b"err");
        assert_eq!(returncode(done.status), 3);
    }

    #[test]
    fn signal_is_negative_returncode() {
        let done = run(
            Command::new("sh").args(["-c", "kill -9 $$"]),
            Duration::from_secs(10),
            true,
        )
        .unwrap();
        assert_eq!(returncode(done.status), -9);
    }

    #[test]
    fn large_output_does_not_deadlock() {
        let done = run(
            Command::new("sh").args([
                "-c",
                "head -c 1000000 /dev/zero; head -c 1000000 /dev/zero >&2",
            ]),
            Duration::from_secs(20),
            true,
        )
        .unwrap();
        assert_eq!(done.stdout.len(), 1_000_000);
        assert_eq!(done.stderr.len(), 1_000_000);
    }

    #[test]
    fn timing_record_is_bounded_like_the_python_helper() {
        // Port of test-wiring.py test_helper_archive_budget_and_timeout_timings_are_bounded.
        let capped = Timing::new(Duration::from_secs(90_000), Duration::from_secs(180), -1);
        assert_eq!(
            capped.to_json(),
            json!({"elapsed_seconds": 86400, "timeout_seconds": 180, "exit_code": -1})
        );
        let normal = Timing::new(Duration::from_millis(125), Duration::from_secs(180), 0);
        assert_eq!(
            normal.to_json(),
            json!({"elapsed_seconds": 0.125, "timeout_seconds": 180, "exit_code": 0})
        );
        assert_eq!(
            normal.log_line(),
            "{\"elapsed_seconds\": 0.125, \"exit_code\": 0, \"timeout_seconds\": 180}"
        );
    }
}
