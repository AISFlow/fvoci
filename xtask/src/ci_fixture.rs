//! Shared pieces of the CI fixture steps (`schema-baseline`,
//! `selected-install`): secret identities, private evidence files, captured
//! children, and SIGINT (a cancelled workflow step) turned into a failure so
//! the caller's cleanup of owned resources still runs.

use crate::host::hex;
use std::fs::{OpenOptions, Permissions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

extern "C" fn on_interrupt(_: libc::c_int) {
    INTERRUPTED.store(true, Ordering::SeqCst);
}

/// Record SIGINT instead of dying, so owned resources are retired first.
/// Children still get the default disposition across exec.
pub fn catch_interrupt() -> Result<(), String> {
    let handler = on_interrupt as extern "C" fn(libc::c_int) as libc::sighandler_t;
    // SAFETY: the handler only stores to an atomic, which is async-signal-safe.
    if unsafe { libc::signal(libc::SIGINT, handler) } == libc::SIG_ERR {
        return Err(format!("SIGINT handler: {}", io::Error::last_os_error()));
    }
    Ok(())
}

pub fn interrupted() -> bool {
    INTERRUPTED.load(Ordering::SeqCst)
}

/// `secrets.token_hex(bytes)`: lowercase hex of kernel CSPRNG bytes.
pub fn random_hex(bytes: usize) -> Result<String, String> {
    let mut buffer = vec![0u8; bytes];
    let mut filled = 0;
    while filled < bytes {
        // SAFETY: the pointer and length describe the unfilled tail of buffer.
        let read =
            unsafe { libc::getrandom(buffer[filled..].as_mut_ptr().cast(), bytes - filled, 0) };
        if read < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(format!("getrandom: {error}"));
        }
        filled += read as usize;
    }
    Ok(hex(&buffer))
}

/// Create `path` (never an existing file), set mode 0600 regardless of the
/// umask, then write `data`.
pub fn write_private(path: &Path, data: &[u8]) -> Result<(), String> {
    let failed = |e: io::Error| format!("{}: {e}", path.display());
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(failed)?;
    file.set_permissions(Permissions::from_mode(0o600))
        .map_err(failed)?;
    file.write_all(data).map_err(failed)
}

/// How a captured child's output is collected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Streams {
    /// stdout and stderr each into its own buffer.
    Separate,
    /// stderr into the stdout pipe (`stderr=subprocess.STDOUT`), in order.
    Merged,
}

#[derive(Debug)]
pub struct Captured {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    /// Empty for `Streams::Merged`.
    pub stderr: Vec<u8>,
}

fn reader(mut pipe: impl Read + Send + 'static) -> JoinHandle<io::Result<Vec<u8>>> {
    thread::spawn(move || {
        let mut buffer = Vec::new();
        pipe.read_to_end(&mut buffer).map(|_| buffer)
    })
}

fn collect(handle: Option<JoinHandle<io::Result<Vec<u8>>>>) -> Result<Vec<u8>, String> {
    match handle {
        None => Ok(Vec::new()),
        Some(handle) => handle
            .join()
            .map_err(|_| "output reader panicked".to_owned())?
            .map_err(|e| format!("output: {e}")),
    }
}

/// Wait for the child and for every helper thread (output readers, stdin
/// feeder): a grandchild can keep a pipe open after the child exits. With
/// `cancellable`, a SIGINT ends the wait at any point; the child is killed and
/// reaped if it is still running, and leftover threads are detached.
fn wait(
    child: &mut Child,
    finished: impl Fn() -> bool,
    cancellable: bool,
) -> Result<ExitStatus, String> {
    let mut pause = Duration::from_millis(1);
    let mut exited = None;
    loop {
        if exited.is_none() {
            exited = child.try_wait().map_err(|e| format!("wait: {e}"))?;
        }
        if let Some(status) = exited.filter(|_| finished()) {
            return Ok(status);
        }
        if cancellable && interrupted() {
            if exited.is_none() {
                let _ = child.kill();
                child.wait().map_err(|e| format!("wait: {e}"))?;
            }
            return Err("interrupted".into());
        }
        thread::sleep(pause);
        pause = (pause * 2).min(Duration::from_millis(50));
    }
}

/// Run `command` to completion with stdin fed from `input` (or /dev/null) and
/// its output captured. With `cancellable`, a SIGINT kills and reaps the
/// child and the call fails, like `subprocess.run` on KeyboardInterrupt.
pub fn capture(
    command: &mut Command,
    input: Option<&[u8]>,
    streams: Streams,
    cancellable: bool,
) -> Result<Captured, String> {
    // A cancelled step starts no further side-effecting command.
    if cancellable && interrupted() {
        return Err("interrupted".into());
    }
    command.stdin(if input.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    let merged = match streams {
        Streams::Separate => {
            command.stdout(Stdio::piped()).stderr(Stdio::piped());
            None
        }
        Streams::Merged => {
            let (read, write) = io::pipe().map_err(|e| format!("pipe: {e}"))?;
            let copy = write.try_clone().map_err(|e| format!("pipe: {e}"))?;
            command.stdout(copy).stderr(write);
            Some(read)
        }
    };
    let spawned = command.spawn();
    // Drop the parent's copies of the merged write end, or EOF never comes.
    command.stdout(Stdio::null()).stderr(Stdio::null());
    let mut child = spawned.map_err(|e| format!("spawn: {e}"))?;
    let feeder = match (child.stdin.take(), input) {
        (Some(mut stdin), Some(input)) => {
            let input = input.to_vec();
            Some(thread::spawn(move || stdin.write_all(&input)))
        }
        _ => None,
    };
    let stdout = match merged {
        Some(read) => Some(reader(read)),
        None => child.stdout.take().map(reader),
    };
    let stderr = child.stderr.take().map(reader);
    let done = |handle: &Option<JoinHandle<_>>| handle.as_ref().is_none_or(JoinHandle::is_finished);
    let status = wait(
        &mut child,
        || done(&stdout) && done(&stderr) && feeder.as_ref().is_none_or(JoinHandle::is_finished),
        cancellable,
    )?;
    let stdout = collect(stdout)?;
    let stderr = collect(stderr)?;
    if let Some(feeder) = feeder {
        // A child that exits without reading all of stdin is judged by its
        // status, as `communicate()` ignores the broken pipe.
        let _ = feeder.join();
    }
    Ok(Captured {
        status,
        stdout,
        stderr,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::returncode;

    #[test]
    fn random_hex_has_requested_length_and_differs() {
        let first = random_hex(32).unwrap();
        assert_eq!(first.len(), 64);
        assert!(first
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
        assert_ne!(first, random_hex(32).unwrap());
    }

    #[test]
    fn private_file_is_new_and_0600() {
        let base = crate::host::mkdtemp("xtask-ci-fixture-", &std::env::temp_dir()).unwrap();
        let path = base.join("evidence.log");
        write_private(&path, b"data").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        assert!(write_private(&path, b"again").is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"data");
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn separate_streams_feed_stdin_and_keep_status() {
        let done = capture(
            Command::new("sh").args(["-c", "cat; printf err >&2; exit 3"]),
            Some(b"input"),
            Streams::Separate,
            true,
        )
        .unwrap();
        assert_eq!(done.stdout, b"input");
        assert_eq!(done.stderr, b"err");
        assert_eq!(returncode(done.status), 3);
    }

    #[test]
    fn merged_streams_keep_order_and_reach_eof() {
        let done = capture(
            Command::new("sh").args(["-c", "printf a; printf b >&2; printf c"]),
            None,
            Streams::Merged,
            true,
        )
        .unwrap();
        assert_eq!(done.stdout, b"abc");
        assert!(done.stderr.is_empty());
        assert!(done.status.success());
    }

    #[test]
    fn stdin_is_null_without_input() {
        let done = capture(
            Command::new("sh").args(["-c", "cat; echo done"]),
            None,
            Streams::Separate,
            true,
        )
        .unwrap();
        assert_eq!(done.stdout, b"done\n");
    }
}
