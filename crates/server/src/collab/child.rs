//! What a worker process is held to on Unix, set in the child between `fork` and
//! `exec`: no core file, a handful of files, and a process group of its own. Linux
//! also limits the address space and ties the worker's death to the server's.
//! The address-space limit is Linux-only because macOS rejects it with `EINVAL`.
//!
//! A worker also ends when the server's end of its input closes, which is when the
//! server ends however it ends; death with the server (`PR_SET_PDEATHSIG`) also ends a
//! worker that is busy with a request then. Linux sends that signal when the thread
//! that started the worker ends, not the process: the server starts its workers from
//! its runtime's tasks, whose threads live as long as the runtime, and never from a
//! thread of blocking work, which ends when it has been idle for a while.

use tokio::process::Command;

/// How many files a worker may have open: its standard input, output and error, and
/// a few the runtime of the language may open. A worker opens none of its own.
const OPEN_FILES: libc::rlim_t = 16;

/// Holds the worker that `command` starts to its limits.
pub(super) fn limit(command: &mut Command, memory_bytes: u64) {
    // Ctrl-C in a terminal goes to the process group of the server; the worker is not
    // to die before the server is done with it.
    command.process_group(0);
    #[cfg(target_os = "linux")]
    let parent = libc::pid_t::try_from(std::process::id()).unwrap_or(0);
    #[cfg(target_os = "linux")]
    let memory: libc::rlim_t = memory_bytes;
    #[cfg(not(target_os = "linux"))]
    let _ = memory_bytes;
    // SAFETY: the closure runs in the child between `fork` and `exec`, where only
    // functions that are safe in a signal handler may be called. It calls `setrlimit`,
    // `prctl` and `getppid`, which are, and makes an `io::Error` from the last error
    // number, which allocates nothing (no message of its own). It takes no lock and touches no memory of the
    // server but the values it captured by copy.
    unsafe {
        command.pre_exec(move || {
            #[cfg(target_os = "linux")]
            set_limit(libc::RLIMIT_AS, memory)?;
            set_limit(libc::RLIMIT_CORE, 0)?;
            set_limit(libc::RLIMIT_NOFILE, OPEN_FILES)?;
            #[cfg(target_os = "linux")]
            die_with(parent)?;
            Ok(())
        });
    }
}

/// Sets the soft and the hard limit of a resource.
fn set_limit(resource: Resource, value: libc::rlim_t) -> std::io::Result<()> {
    let limit = libc::rlimit {
        rlim_cur: value,
        rlim_max: value,
    };
    // SAFETY: `limit` is a valid `rlimit` that lives across the call, which only reads
    // it.
    let result = unsafe { libc::setrlimit(resource, &raw const limit) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// The type `setrlimit` takes its resource as in this C library.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
type Resource = libc::__rlimit_resource_t;
/// The type `setrlimit` takes its resource as in this C library.
#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
type Resource = libc::c_int;

/// Has the kernel kill the worker when the server's thread that started it ends, and
/// ends the worker at once if the server ended before that was asked.
#[cfg(target_os = "linux")]
fn die_with(parent: libc::pid_t) -> std::io::Result<()> {
    let signal = libc::c_ulong::try_from(libc::SIGKILL).unwrap_or(9);
    // SAFETY: `PR_SET_PDEATHSIG` takes a signal number and reads no memory.
    if unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, signal) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: `getppid` takes nothing and does not fail.
    if unsafe { libc::getppid() } != parent {
        // The server ended before this was asked: no process is there to serve.
        return Err(std::io::Error::from_raw_os_error(libc::ESRCH));
    }
    Ok(())
}

/// A signal a test sends a worker ([`super::worker::Worker::stop_at`]).
#[cfg(feature = "test-util")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Signal {
    /// `SIGSTOP`: the process stops where it is, and reads nothing more.
    Stop,
    /// `SIGCONT`: it goes on.
    Continue,
}

/// Sends `signal` to the worker of `pid`, which its task has started and not reaped.
#[cfg(feature = "test-util")]
pub(super) fn signal(pid: u32, signal: Signal) {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return;
    };
    let number = match signal {
        Signal::Stop => libc::SIGSTOP,
        Signal::Continue => libc::SIGCONT,
    };
    // SAFETY: `kill` takes a process identifier and a signal number and reads no memory
    // of this process.
    let _ = unsafe { libc::kill(pid, number) };
}
