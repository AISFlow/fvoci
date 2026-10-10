//! Bounded child execution with the observable behaviour of Python
//! `subprocess.run(argv, timeout=..., capture_output=...)`: the timeout covers
//! the child's exit and EOF on both captured pipes, the direct child is killed
//! and reaped when the timeout expires, and both pipes are drained
//! concurrently so a chatty child cannot block on a full pipe.

use serde_json::{json, Value};
use std::io::{self, Read};
use std::os::unix::process::ExitStatusExt;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
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
    /// The child or its captured pipes outlived the budget; a direct child
    /// still running at the deadline was killed and reaped.
    Timeout(Duration),
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spawn(error) | Self::Wait(error) => write!(f, "{error}"),
            Self::Timeout(timeout) => write!(f, "timed out after {} seconds", timeout.as_secs()),
        }
    }
}

/// Read `pipe` to EOF on its own thread and send the bytes tagged `index`.
fn drain(index: usize, mut pipe: impl Read + Send + 'static, sender: Sender<(usize, Vec<u8>)>) {
    thread::spawn(move || {
        let mut buffer = Vec::new();
        let _ = pipe.read_to_end(&mut buffer);
        let _ = sender.send((index, buffer));
    });
}

/// Run `command` to completion within `timeout`. With `capture`, stdout and
/// stderr are collected; otherwise they are inherited. Stdin is inherited.
///
/// Like `Popen.communicate(timeout=...)`, the budget covers both the direct
/// child's exit and EOF on every captured pipe: a descendant that keeps a pipe
/// open past the deadline makes the call time out even when the direct child
/// already exited. On timeout only the direct child is killed (when still
/// running) and reaped, as `subprocess.run` did; the reader threads are
/// detached and finish when the last pipe writer closes.
pub fn run(command: &mut Command, timeout: Duration, capture: bool) -> Result<Finished, RunError> {
    if capture {
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
    }
    let deadline = Instant::now() + timeout;
    let mut child = command.spawn().map_err(RunError::Spawn)?;
    let (sender, receiver) = mpsc::channel();
    let mut pending = 0;
    if let Some(pipe) = child.stdout.take() {
        drain(0, pipe, sender.clone());
        pending += 1;
    }
    if let Some(pipe) = child.stderr.take() {
        drain(1, pipe, sender.clone());
        pending += 1;
    }
    drop(sender);
    let mut output = [Vec::new(), Vec::new()];
    let mut status: Option<ExitStatus> = None;
    let mut pause = Duration::from_millis(1);
    loop {
        if status.is_none() {
            status = child.try_wait().map_err(RunError::Wait)?;
        }
        if let (Some(status), 0) = (status, pending) {
            let [stdout, stderr] = output;
            return Ok(Finished {
                status,
                stdout,
                stderr,
            });
        }
        let now = Instant::now();
        if now >= deadline {
            if status.is_none() {
                let _ = child.kill();
                child.wait().map_err(RunError::Wait)?;
            }
            return Err(RunError::Timeout(timeout));
        }
        let wait = pause.min(deadline - now);
        pause = (pause * 2).min(Duration::from_millis(50));
        if pending == 0 {
            thread::sleep(wait);
            continue;
        }
        match receiver.recv_timeout(wait) {
            Ok((index, bytes)) => {
                output[index] = bytes;
                pending -= 1;
            }
            Err(RecvTimeoutError::Timeout) => {}
            // A reader ended without sending; its output is lost, so fail now.
            Err(RecvTimeoutError::Disconnected) => {
                if status.is_none() {
                    let _ = child.kill();
                    child.wait().map_err(RunError::Wait)?;
                }
                return Err(RunError::Wait(io::Error::other("output reader failed")));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
