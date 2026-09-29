//! Feature-gated parent-death fixture. Production builds omit this binary
//! (`required-features = ["test-hang"]`). It is not a daemon.
//!
//! `collab-parent-death-driver <collab-engine> <pid-file> [--exit-spawning-thread]`
//! spawns one product helper that hangs without reading stdin, writes the
//! helper pid to `<pid-file>` and parks. With `--exit-spawning-thread` the
//! helper is spawned on a short-lived thread that leaks the session and exits
//! while this process stays alive.

use std::path::{Path, PathBuf};
use std::time::Duration;

use collab_engine::limits::Limits;
use collab_engine::process::{ChildSlotKind, EngineSession, SpawnRequest};

fn main() {
    let mut args = std::env::args().skip(1);
    let engine_bin = PathBuf::from(args.next().expect("collab-engine bin path"));
    let pid_file = PathBuf::from(args.next().expect("helper pid file path"));
    let exit_spawning_thread = match args.next().as_deref() {
        None => false,
        Some("--exit-spawning-thread") => true,
        Some(other) => {
            eprintln!("parent-death driver: unknown arg {other}");
            std::process::exit(2);
        }
    };

    if exit_spawning_thread {
        let spawner = std::thread::spawn(move || {
            let session = spawn_hanging(engine_bin);
            let pid = session.pid().expect("helper pid");
            // Leak on purpose: the session is never dropped, so only the
            // spawning thread's exit can end the helper.
            std::mem::forget(session);
            pid
        });
        let pid = spawner.join().expect("spawning thread");
        write_pid(&pid_file, pid);
        park_forever();
    }

    let session = spawn_hanging(engine_bin);
    write_pid(&pid_file, session.pid().expect("helper pid"));
    park_forever();
}

fn spawn_hanging(engine_bin: PathBuf) -> EngineSession {
    EngineSession::spawn(SpawnRequest {
        engine_bin,
        limits: Limits::for_tests(),
        slot_kind: ChildSlotKind::Primary,
        slot_wait: None,
        // The helper sleeps before it reads stdin: a stand-in for a helper
        // busy with a request when its parent dies.
        test_hang_ms: Some(60_000),
        test_exit_after_read: None,
        test_close_stdout_hang_ms: None,
        test_exit_after_write: None,
    })
    .unwrap_or_else(|report| {
        eprintln!("parent-death driver: spawn failed: {:?}", report.outcome);
        std::process::exit(2);
    })
}

fn write_pid(pid_file: &Path, pid: u32) {
    let tmp = pid_file.with_extension("tmp");
    if std::fs::write(&tmp, pid.to_string()).is_err() || std::fs::rename(&tmp, pid_file).is_err() {
        eprintln!("parent-death driver: cannot write {pid_file:?}");
        std::process::exit(2);
    }
}

fn park_forever() -> ! {
    loop {
        std::thread::park_timeout(Duration::from_secs(60));
    }
}
