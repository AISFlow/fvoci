use std::process::{Command, Stdio};
use std::sync::Mutex;
#[cfg(feature = "test-hang")]
use std::time::Instant;

use collab_engine::limits::{Limits, MIN_CHILD_STACK_BYTES};
use collab_engine::outcome::{EngineStatus, LimitKind};
use collab_engine::process::EngineSession;
use collab_engine::protocol::Request;
use collab_engine::{SpawnRequest, WorkerFailureReason};
use yrs::{Any, Map, ReadTxn, StateVector, Transact};

static SPAWN_TEST: Mutex<()> = Mutex::new(());

fn bin() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_collab-engine"))
}

fn spawn(limits: Limits) -> EngineSession {
    EngineSession::spawn(SpawnRequest {
        engine_bin: bin(),
        limits,
        test_hang_ms: None,
    })
    .unwrap_or_else(|r| panic!("spawn: {:?}", r.outcome))
}

fn nested_any_update(depth: usize) -> Vec<u8> {
    let mut any = Any::from(Vec::<Any>::new());
    for _ in 0..depth {
        any = Any::from(vec![any]);
    }
    let doc = collab_engine::engine::new_doc();
    let map = doc.get_or_insert_map("bomb");
    {
        let mut txn = doc.transact_mut();
        map.insert(&mut txn, "k", any);
    }
    let txn = doc.transact();
    txn.encode_state_as_update_v1(&StateVector::default())
}

#[cfg(feature = "test-hang")]
#[test]
fn extract_killable_timeout_reaps_product_helper() {
    use collab_engine::take_last_spawn;
    let _g = SPAWN_TEST.lock().unwrap_or_else(|e| e.into_inner());
    let mut limits = Limits::for_tests();
    limits.timeout_ms = 500;
    let started = Instant::now();
    let mut session = EngineSession::spawn(SpawnRequest {
        engine_bin: bin(),
        limits,
        test_hang_ms: Some(20_000),
    })
    .unwrap_or_else(|r| panic!("spawn: {:?}", r.outcome));
    let report = session.call(&Request::Ping);
    assert!(
        matches!(
            report.outcome,
            EngineStatus::ResourceLimit {
                kind: LimitKind::Time,
                ..
            }
        ),
        "{:?}",
        report.outcome
    );
    let pid = report.child_pid.expect("pid");
    let trace = take_last_spawn().expect("spawn trace");
    assert_eq!(trace.pid, pid);
    assert!(trace.helpers_joined >= 1);
    assert_fully_reaped(pid);
    assert!(started.elapsed().as_secs() < 5, "kill took too long");
}

#[cfg(not(feature = "test-hang"))]
#[test]
fn production_bin_rejects_dump_rlimits_flag() {
    let out = Command::new(bin())
        .args(["--dump-rlimits"])
        .stdin(Stdio::null())
        .output()
        .expect("run");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("unknown arg"),
        "production must not honor --dump-rlimits: {stderr}"
    );
}

#[cfg(not(feature = "test-hang"))]
#[test]
fn production_bin_rejects_test_hang_flag() {
    let out = Command::new(bin())
        .args(["--test-hang-ms", "1"])
        .stdin(Stdio::null())
        .output()
        .expect("run");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("unknown arg"),
        "production must not honor --test-hang-ms: {stderr}"
    );
}

#[cfg(feature = "test-hang")]
#[test]
fn child_applies_forwarded_rlimit_as_and_stack() {
    let rss = 64 * 1024 * 1024;
    let stack = 2 * 1024 * 1024;
    let out = Command::new(bin())
        .args([
            "--max-as",
            &rss.to_string(),
            "--max-stack",
            &stack.to_string(),
            "--dump-rlimits",
        ])
        .stdin(Stdio::null())
        .output()
        .expect("dump rlimits");
    assert!(
        out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains(&format!("RLIMIT_AS={rss}")),
        "got {stdout:?}"
    );
    assert!(
        stdout.contains(&format!("RLIMIT_STACK={stack}")),
        "got {stdout:?}"
    );
}

#[test]
fn zero_timeout_is_invalid_limits() {
    let mut limits = Limits::for_tests();
    limits.timeout_ms = 0;
    let report = EngineSession::spawn(SpawnRequest {
        engine_bin: bin(),
        limits,
        test_hang_ms: None,
    });
    let err = report.err().expect("invalid");
    assert!(
        matches!(
            err.outcome,
            EngineStatus::WorkerFailure {
                reason: WorkerFailureReason::InvalidLimits,
                ..
            }
        ),
        "{:?}",
        err.outcome
    );
}

#[test]
fn nested_any_recursion_is_isolated_from_host() {
    let _g = SPAWN_TEST.lock().unwrap_or_else(|e| e.into_inner());
    let mut limits = Limits::for_tests();
    limits.max_child_stack_bytes = MIN_CHILD_STACK_BYTES;
    limits.timeout_ms = 2_000;
    let bytes = nested_any_update(800);
    assert!(
        bytes.len() < 64 * 1024,
        "recursion fixture must stay small, got {}",
        bytes.len()
    );
    let mut session = spawn(limits);
    let pid = session.pid().expect("pid");
    let report = session.call(&Request::Apply {
        update_b64: bytes,
        encoding: 1,
    });
    match report.outcome {
        EngineStatus::ResourceLimit {
            kind: LimitKind::Stack | LimitKind::Time | LimitKind::Memory,
            ..
        }
        | EngineStatus::Malformed { .. }
        | EngineStatus::WorkerFailure {
            reason: WorkerFailureReason::ChildCrash,
            ..
        }
        | EngineStatus::Ok { .. } => {}
        other => panic!("unexpected outcome for nested any: {other:?}"),
    }
    if session.pid().is_some() {
        session.kill_and_reap();
    }
    assert_fully_reaped(pid);
}

#[test]
fn two_document_rooms_can_live_together() {
    let _g = SPAWN_TEST.lock().unwrap_or_else(|e| e.into_inner());
    let mut a = spawn(Limits::for_tests());
    let mut b = spawn(Limits::for_tests());
    let pa = a.call(&Request::Ping);
    let pb = b.call(&Request::Ping);
    assert!(
        matches!(pa.outcome, EngineStatus::Ok { .. }),
        "{:?}",
        pa.outcome
    );
    assert!(
        matches!(pb.outcome, EngineStatus::Ok { .. }),
        "{:?}",
        pb.outcome
    );
}

#[test]
fn ninth_live_child_is_immediate_resource_limit() {
    let _g = SPAWN_TEST.lock().unwrap_or_else(|e| e.into_inner());
    let mut live = Vec::new();
    for i in 0..collab_engine::limits::MAX_CHILD_CONCURRENCY {
        live.push(spawn(Limits::for_tests()));
        let _ = i;
    }
    let ninth = EngineSession::spawn(SpawnRequest {
        engine_bin: bin(),
        limits: Limits::for_tests(),
        test_hang_ms: None,
    });
    let err = ninth.err().expect("9th must be refused");
    assert!(
        matches!(
            err.outcome,
            EngineStatus::ResourceLimit {
                kind: LimitKind::Ops,
                ..
            }
        ),
        "{:?}",
        err.outcome
    );
    drop(live);
}

#[cfg(feature = "test-hang")]
#[test]
fn write_times_out_when_child_stops_reading() {
    let _g = SPAWN_TEST.lock().unwrap_or_else(|e| e.into_inner());
    let mut limits = Limits::for_tests();
    limits.timeout_ms = 500;
    limits.max_input_bytes = 256 * 1024;
    limits.max_output_bytes = 256 * 1024;
    limits.max_load_bytes = 256 * 1024;
    let started = Instant::now();
    let mut session = EngineSession::spawn(SpawnRequest {
        engine_bin: bin(),
        limits,
        test_hang_ms: Some(20_000),
    })
    .unwrap_or_else(|r| panic!("spawn: {:?}", r.outcome));
    let pid = session.pid().expect("pid");
    let report = session.call(&Request::Apply {
        update_b64: vec![1; 128 * 1024],
        encoding: 1,
    });
    assert!(
        matches!(
            report.outcome,
            EngineStatus::ResourceLimit {
                kind: LimitKind::Time,
                ..
            }
        ),
        "{:?}",
        report.outcome
    );
    assert_fully_reaped(pid);
    assert!(
        started.elapsed().as_secs() < 5,
        "send timeout blocked too long"
    );
}

#[cfg(feature = "test-hang")]
#[test]
fn child_scrubs_database_url() {
    let out = Command::new(bin())
        .env("DATABASE_APP_URL", "postgres://secret")
        .args(["--dump-rlimits"])
        .stdin(Stdio::null())
        .output()
        .expect("dump");
    assert!(
        out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("DATABASE_APP_URL=unset"), "{stdout}");
}

fn assert_fully_reaped(pid: u32) {
    let path = format!("/proc/{pid}");
    if !std::path::Path::new(&path).exists() {
        return;
    }
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap_or_default();
    if status.contains("State:\tZ") || status.to_ascii_lowercase().contains("zombie") {
        panic!("pid {pid} is a zombie");
    }
    panic!("pid {pid} still exists:\n{status}");
}
