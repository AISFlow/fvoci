//! Bounded child execution with the observable behaviour of Python
//! `subprocess.run(argv, timeout=..., capture_output=...)`: the timeout covers
//! the child's exit and EOF on both captured pipes, the direct child is killed
//! and reaped when the timeout expires, and both pipes are drained
//! concurrently so a chatty child cannot block on a full pipe.
//! `run_owned` is the stricter variant for new callers: a process group per
//! call, killed as a whole when the call ends.

use serde_json::{json, Value};
use std::io::{self, Read};
use std::os::unix::process::CommandExt;
use std::os::unix::process::ExitStatusExt;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::Once;
use std::thread;
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
    /// The child or its captured pipes outlived the budget. `run` killed and
    /// reaped a direct child still running at the deadline; `run_owned` killed
    /// its whole process group.
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

type Output = [Option<Vec<u8>>; 2];

/// Read each captured pipe of `child` to EOF on its own thread. A stream that
/// was not captured is complete and empty; a captured stream without a pipe
/// is a read error.
fn read_pipes(
    child: &mut Child,
    capture: bool,
) -> (Receiver<(usize, io::Result<Vec<u8>>)>, Output) {
    let (sender, results) = mpsc::channel();
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
    let mut output: Output = [None, None];
    for (slot, pipe) in pipes.into_iter().enumerate() {
        let Some(mut pipe) = pipe else {
            if capture {
                let _ = sender.send((slot, Err(io::Error::other("pipe was not created"))));
            } else {
                output[slot] = Some(Vec::new());
            }
            continue;
        };
        let sender = sender.clone();
        thread::spawn(move || {
            let mut buffer = Vec::new();
            let result = pipe.read_to_end(&mut buffer).map(|_| buffer);
            let _ = sender.send((slot, result));
        });
    }
    (results, output)
}

/// True once `pid` has exited, leaving it unreaped so its pid and process
/// group id cannot be reused before the caller's cleanup.
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

/// Wait until `pid` has exited and every captured pipe reached EOF, like
/// `Popen.communicate(timeout=...)`: a descendant that keeps a pipe open past
/// the deadline is a timeout even when `pid` already exited. A pipe read
/// error or a lost reader is an error, never empty output. `pid` is left
/// unreaped.
fn settle(
    pid: u32,
    results: &Receiver<(usize, io::Result<Vec<u8>>)>,
    output: &mut Output,
    deadline: Instant,
    timeout: Duration,
) -> Result<(), RunError> {
    let mut child_done = false;
    let mut pause = Duration::from_millis(1);
    loop {
        if !child_done {
            child_done = exited(pid).map_err(RunError::Wait)?;
        }
        if child_done && output.iter().all(Option::is_some) {
            return Ok(());
        }
        let now = Instant::now();
        if now >= deadline {
            return Err(RunError::Timeout(timeout));
        }
        let wait = pause.min(deadline - now);
        pause = (pause * 2).min(Duration::from_millis(10));
        match results.recv_timeout(wait) {
            Ok((slot, Ok(bytes))) => output[slot] = Some(bytes),
            Ok((_, Err(error))) => return Err(RunError::Read(error)),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                if output.iter().any(Option::is_none) {
                    return Err(RunError::Read(io::Error::other(
                        "an output reader was lost",
                    )));
                }
                // Every pipe is read (or none was captured) and the child
                // still runs: poll its exit.
                thread::sleep(wait);
            }
        }
    }
}

fn finished(reaped: io::Result<ExitStatus>, output: Output) -> Result<Finished, RunError> {
    let status = reaped.map_err(RunError::Wait)?;
    let [stdout, stderr] = output.map(Option::unwrap_or_default);
    Ok(Finished {
        status,
        stdout,
        stderr,
    })
}

/// Run `command` to completion within `timeout`. With `capture`, stdout and
/// stderr are collected and the budget covers EOF on both pipes; otherwise
/// they are inherited and only the direct child's exit is waited for. Stdin is
/// inherited. On timeout or error only the direct child is killed (a no-op
/// once it exited) and reaped, as `subprocess.run` did; the reader threads are
/// detached and finish when the last pipe writer closes.
pub fn run(command: &mut Command, timeout: Duration, capture: bool) -> Result<Finished, RunError> {
    if capture {
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
    }
    let deadline = Instant::now() + timeout;
    let mut child = command.spawn().map_err(RunError::Spawn)?;
    let (results, mut output) = read_pipes(&mut child, capture);
    let outcome = settle(child.id(), &results, &mut output, deadline, timeout);
    if outcome.is_err() {
        // Still unreaped, so the pid cannot have been reused.
        let _ = child.kill();
    }
    let reaped = child.wait();
    outcome?;
    finished(reaped, output)
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

/// Block the forwarded signals in this thread; returns the previous mask.
fn hold_interrupts() -> libc::sigset_t {
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        let mut old: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
            libc::sigaddset(&mut set, signal);
        }
        libc::pthread_sigmask(libc::SIG_BLOCK, &set, &mut old);
        old
    }
}

fn release_interrupts(old: libc::sigset_t) {
    unsafe {
        libc::pthread_sigmask(libc::SIG_SETMASK, &old, std::ptr::null_mut());
    }
}

/// Run `command` in its own process group and capture stdout and stderr within
/// `timeout`, with the deadline and output rules of `run`. When the call ends
/// (exit, timeout or error) every process left in the group is killed and the
/// direct child is reaped.
pub fn run_owned(command: &mut Command, timeout: Duration) -> Result<Finished, RunError> {
    let deadline = Instant::now() + timeout;
    forward_interrupts();
    command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    // An interrupt between spawn and recording the group would leave the group
    // running outside the foreground group: hold the forwarded signals until
    // the group is recorded. The child would inherit the held mask, so it
    // restores the caller's mask before exec.
    let held = hold_interrupts();
    unsafe {
        command.pre_exec(move || {
            libc::sigprocmask(libc::SIG_SETMASK, &held, std::ptr::null_mut());
            Ok(())
        });
    }
    let spawned = command.spawn();
    if let Ok(child) = &spawned {
        OWNED_GROUP.store(child.id() as i32, Ordering::SeqCst);
    }
    release_interrupts(held);
    let mut child = spawned.map_err(RunError::Spawn)?;
    let pid = child.id();
    let (results, mut output) = read_pipes(&mut child, true);
    let outcome = settle(pid, &results, &mut output, deadline, timeout);
    // The child is not reaped yet, so the group id is still ours; forget it
    // before reaping so the interrupt handler never signals a reused id.
    unsafe {
        libc::killpg(pid as libc::pid_t, libc::SIGKILL);
    }
    OWNED_GROUP.store(0, Ordering::SeqCst);
    let reaped = child.wait();
    outcome?;
    finished(reaped, output)
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
    fn run_timeout_kills_only_the_direct_child() {
        // subprocess.run never signalled the group; a descendant runs on.
        let dir = std::env::temp_dir().join(format!("xtask-run-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let marker = dir.join("pid");
        let script = format!("sleep 30 & echo $! > '{}'; wait", marker.display());
        let error = run(
            Command::new("sh").args(["-c", &script]),
            Duration::from_millis(300),
            true,
        )
        .unwrap_err();
        assert!(matches!(error, RunError::Timeout(_)));
        let pid: libc::pid_t = std::fs::read_to_string(&marker)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        let alive = unsafe { libc::kill(pid, 0) } == 0;
        unsafe {
            libc::kill(pid, libc::SIGKILL);
        }
        assert!(alive, "run signalled the descendant {pid}");
    }

    /// CPU time of the calling thread, where run_owned's wait loop runs.
    fn thread_cpu() -> Duration {
        let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
        assert_eq!(
            unsafe { libc::getrusage(libc::RUSAGE_THREAD, &mut usage) },
            0
        );
        let time = |t: libc::timeval| {
            Duration::from_secs(t.tv_sec as u64) + Duration::from_micros(t.tv_usec as u64)
        };
        time(usage.ru_utime) + time(usage.ru_stime)
    }

    #[test]
    fn owned_wait_after_the_pipes_close_does_not_spin() {
        // Both readers finish at once; the child runs on for a second.
        let before = thread_cpu();
        let done = run_owned(
            Command::new("sh").args(["-c", "exec >&- 2>&-; sleep 1"]),
            Duration::from_secs(10),
        )
        .unwrap();
        let used = thread_cpu() - before;
        assert!(done.status.success());
        assert!(
            used < Duration::from_millis(200),
            "waiting used {used:?} CPU"
        );
    }

    #[test]
    fn owned_child_starts_with_no_blocked_signals() {
        let done = run_owned(
            // The spawned program reads its own mask (a shell blocks signals
            // around its own forks).
            Command::new("sed").args(["-n", "s/^SigBlk:[[:space:]]*//p", "/proc/self/status"]),
            Duration::from_secs(10),
        )
        .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&done.stdout).trim(),
            "0000000000000000"
        );
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
