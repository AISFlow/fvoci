//! Feature-gated fixture for a non-dumpable parent. Production builds omit
//! this binary (`required-features = ["test-hang"]`). `PR_SET_DUMPABLE` is
//! process-wide, so this runs in its own process, never inside a shared test
//! binary.
//!
//! `collab-non-dumpable-driver <collab-engine> <helpers>` checks, and prints
//! one summary line, exiting 0 only if all hold:
//! 1. while dumpable, a same-uid child reads this process's environ (control);
//! 2. after `make_process_non_dumpable`, that read fails with EACCES;
//! 3. `<helpers>` product helpers spawned from several threads under CPU load
//!    each answer Ping and then show `oom_score_adj` 1000, their RSS stays
//!    readable, and the OOM backstop is not reported missing.
//!
//! It refuses to run as root: root with `CAP_SYS_PTRACE` still reads the
//! environ, and root without it gets EPERM, not EACCES. The test that runs it
//! skips as root.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use collab_engine::limits::Limits;
use collab_engine::outcome::EngineStatus;
use collab_engine::process::{
    child_oom_score_adj, make_process_non_dumpable, oom_backstop_missing,
    set_max_child_concurrency, sum_live_children_rss_bytes, ChildSlotKind, EngineSession,
    SpawnRequest,
};
use collab_engine::protocol::Request;

const PROBE_FLAG: &str = "--probe-parent-environ";
const MARKER_ENV: &str = "FVOCI_NON_DUMPABLE_MARKER";
const SPAWNERS: usize = 4;

fn main() {
    let mut args = std::env::args().skip(1);
    let first = args.next().expect("collab-engine bin path or probe flag");
    if first == PROBE_FLAG {
        probe_parent_environ();
    }
    let engine_bin = PathBuf::from(first);
    let helpers: usize = args
        .next()
        .expect("helper count")
        .parse()
        .expect("helper count");
    if std::env::var(MARKER_ENV).map_or(true, |marker| marker.is_empty()) {
        fail(&format!("{MARKER_ENV} must name a non-empty marker"));
    }
    if unsafe { libc::geteuid() } == 0 {
        fail("needs a non-root user: the same-uid environ check does not hold for root");
    }

    let control = run_probe();
    if control != "readable marker=true" {
        fail(&format!("control read while dumpable: {control}"));
    }
    if let Err(err) = make_process_non_dumpable() {
        fail(&format!("make_process_non_dumpable: {err}"));
    }
    let denied = run_probe();
    if denied != format!("error errno={}", libc::EACCES) {
        fail(&format!("read after PR_SET_DUMPABLE 0: {denied}"));
    }

    set_max_child_concurrency(SPAWNERS * 2);
    let stop = Arc::new(AtomicBool::new(false));
    let load: Vec<_> = (0..std::thread::available_parallelism().map_or(4, |n| n.get()))
        .map(|_| {
            let stop = stop.clone();
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    std::hint::spin_loop();
                }
            })
        })
        .collect();
    let spawners: Vec<_> = (0..SPAWNERS)
        .map(|i| {
            let engine_bin = engine_bin.clone();
            let count = helpers / SPAWNERS + usize::from(i < helpers % SPAWNERS);
            std::thread::spawn(move || spawn_and_check(engine_bin, count))
        })
        .collect();
    let mut failures = Vec::new();
    let mut checked = 0usize;
    for spawner in spawners {
        let (ok, mut errs) = spawner.join().expect("spawner thread");
        checked += ok;
        failures.append(&mut errs);
    }
    stop.store(true, Ordering::Relaxed);
    for thread in load {
        let _ = thread.join();
    }
    if oom_backstop_missing() {
        failures.push("oom_backstop_missing() reported true".into());
    }
    println!(
        "control=readable after=EACCES helpers={helpers} oom_1000_rss_ok={checked} failures={}",
        failures.len()
    );
    if !failures.is_empty() || checked != helpers {
        for failure in failures.iter().take(10) {
            eprintln!("non-dumpable driver: {failure}");
        }
        std::process::exit(1);
    }
}

/// Spawn, Ping, and check `count` helpers one at a time on this thread.
fn spawn_and_check(engine_bin: PathBuf, count: usize) -> (usize, Vec<String>) {
    let mut ok = 0;
    let mut failures = Vec::new();
    for _ in 0..count {
        let mut session = match EngineSession::spawn(SpawnRequest {
            engine_bin: engine_bin.clone(),
            limits: Limits::for_tests(),
            slot_kind: ChildSlotKind::Primary,
            slot_wait: None,
            test_hang_ms: None,
            test_exit_after_read: None,
            test_close_stdout_hang_ms: None,
            test_exit_after_write: None,
        }) {
            Ok(session) => session,
            Err(report) => {
                failures.push(format!("spawn: {:?}", report.outcome));
                continue;
            }
        };
        let pid = session.pid().expect("helper pid");
        let ping = session.call(&Request::Ping);
        if !matches!(ping.outcome, EngineStatus::Ok { .. }) {
            failures.push(format!("helper {pid} ping: {:?}", ping.outcome));
            continue;
        }
        let oom = child_oom_score_adj(pid);
        let rss = vm_rss_kib(pid);
        let rss_sum = sum_live_children_rss_bytes();
        if oom != Some(1000) {
            failures.push(format!("helper {pid} oom_score_adj {oom:?}"));
        } else if rss.is_none_or(|kib| kib == 0) || rss_sum == 0 {
            failures.push(format!(
                "helper {pid} RSS unreadable: VmRSS {rss:?} kiB, live sum {rss_sum}"
            ));
        } else {
            ok += 1;
        }
        session.kill_and_reap();
    }
    (ok, failures)
}

fn vm_rss_kib(pid: u32) -> Option<u64> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    status
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

/// Re-run this binary as a same-uid child that tries to read our environ.
fn run_probe() -> String {
    let out = std::process::Command::new(std::env::current_exe().expect("current exe"))
        .arg(PROBE_FLAG)
        .env_clear()
        .env(
            MARKER_ENV,
            std::env::var_os(MARKER_ENV).expect("marker env"),
        )
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run probe");
    if !out.status.success() {
        fail(&format!("probe exited {}", out.status));
    }
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Probe mode: report whether the parent's environ is readable, and whether
/// it holds the marker the parent was started with.
fn probe_parent_environ() -> ! {
    let parent = std::os::unix::process::parent_id();
    let marker = std::env::var(MARKER_ENV).unwrap_or_default();
    match std::fs::read(format!("/proc/{parent}/environ")) {
        Ok(environ) => {
            let found = !marker.is_empty()
                && environ
                    .split(|b| *b == 0)
                    .any(|entry| entry == format!("{MARKER_ENV}={marker}").as_bytes());
            println!("readable marker={found}");
        }
        Err(err) => println!("error errno={}", err.raw_os_error().unwrap_or(-1)),
    }
    std::process::exit(0);
}

fn fail(message: &str) -> ! {
    eprintln!("non-dumpable driver: {message}");
    std::process::exit(1);
}
