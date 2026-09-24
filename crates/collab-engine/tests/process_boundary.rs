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
        test_exit_after_read: None,
        test_close_stdout_hang_ms: None,
        test_exit_after_write: None,
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
        test_exit_after_read: None,
        test_close_stdout_hang_ms: None,
        test_exit_after_write: None,
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

#[cfg(not(feature = "test-hang"))]
#[test]
fn production_bin_rejects_test_exit_after_read_flag() {
    let out = Command::new(bin())
        .args(["--test-exit-after-read", "7"])
        .stdin(Stdio::null())
        .output()
        .expect("run");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("unknown arg"),
        "production must not honor --test-exit-after-read: {stderr}"
    );
}

#[cfg(not(feature = "test-hang"))]
#[test]
fn production_bin_rejects_close_stdout_hang_flag() {
    let out = Command::new(bin())
        .args(["--test-close-stdout-then-hang-ms", "1"])
        .stdin(Stdio::null())
        .output()
        .expect("run");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("unknown arg"),
        "production must not honor --test-close-stdout-then-hang-ms: {stderr}"
    );
}

#[cfg(not(feature = "test-hang"))]
#[test]
fn production_bin_rejects_test_exit_after_write_flag() {
    let out = Command::new(bin())
        .args(["--test-exit-after-write", "0"])
        .stdin(Stdio::null())
        .output()
        .expect("run");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("unknown arg"),
        "production must not honor --test-exit-after-write: {stderr}"
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
            "--max-observed-rss",
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

#[cfg(feature = "test-hang")]
#[test]
fn child_applies_cumulative_cpu_budget() {
    let timeout_ms = 1_001u64;
    let max_ops = 3u32;
    let cpu = timeout_ms.div_ceil(1000) * u64::from(max_ops);
    assert_eq!(cpu, 6);
    let out = Command::new(bin())
        .args([
            "--timeout-ms",
            &timeout_ms.to_string(),
            "--max-ops",
            &max_ops.to_string(),
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
        stdout.contains(&format!("RLIMIT_CPU={cpu}")),
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
        test_exit_after_read: None,
        test_close_stdout_hang_ms: None,
        test_exit_after_write: None,
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
        test_exit_after_read: None,
        test_close_stdout_hang_ms: None,
        test_exit_after_write: None,
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
        test_exit_after_read: None,
        test_close_stdout_hang_ms: None,
        test_exit_after_write: None,
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
fn abrupt_child_exit_is_crash_not_protocol() {
    let _g = SPAWN_TEST.lock().unwrap_or_else(|e| e.into_inner());
    let mut limits = Limits::for_tests();
    limits.timeout_ms = 2_000;
    let started = Instant::now();
    let mut session = EngineSession::spawn(SpawnRequest {
        engine_bin: bin(),
        limits,
        test_hang_ms: None,
        test_exit_after_read: Some(7),
        test_close_stdout_hang_ms: None,
        test_exit_after_write: None,
    })
    .unwrap_or_else(|r| panic!("spawn: {:?}", r.outcome));
    let pid = session.pid().expect("pid");
    let report = session.call(&Request::Ping);
    match report.outcome {
        EngineStatus::WorkerFailure {
            reason: WorkerFailureReason::ChildCrash,
            ref detail,
        } => {
            assert!(
                detail.contains("exit Some(7)"),
                "abrupt exit must preserve ChildCrash exit code, got {detail}"
            );
        }
        other => panic!("expected ChildCrash for abrupt child exit, got {other:?}"),
    }
    if session.pid().is_some() {
        session.kill_and_reap();
    }
    assert_fully_reaped(pid);
    assert!(
        started.elapsed().as_secs() < 5,
        "abrupt-exit observation blocked too long"
    );
}

#[cfg(feature = "test-hang")]
#[test]
fn stdout_close_with_live_child_is_protocol() {
    let _g = SPAWN_TEST.lock().unwrap_or_else(|e| e.into_inner());
    let mut limits = Limits::for_tests();
    limits.timeout_ms = 500;
    let started = Instant::now();
    let mut session = EngineSession::spawn(SpawnRequest {
        engine_bin: bin(),
        limits,
        test_hang_ms: None,
        test_exit_after_read: None,
        test_close_stdout_hang_ms: Some(20_000),
        test_exit_after_write: None,
    })
    .unwrap_or_else(|r| panic!("spawn: {:?}", r.outcome));
    let pid = session.pid().expect("pid");
    let report = session.call(&Request::Ping);
    match report.outcome {
        EngineStatus::WorkerFailure {
            reason: WorkerFailureReason::Protocol,
            ref detail,
        } => {
            assert!(
                detail.contains("child closed stdout before a frame"),
                "live child stdout close must stay Protocol, got {detail}"
            );
        }
        other => panic!("expected Protocol for live stdout close, got {other:?}"),
    }
    if session.pid().is_some() {
        session.kill_and_reap();
    }
    assert_fully_reaped(pid);
    assert!(
        started.elapsed().as_secs() < 5,
        "live-stdout protocol wait blocked too long"
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

#[test]
fn max_ops_exhaustion_is_resource_limit() {
    let _g = SPAWN_TEST.lock().unwrap_or_else(|e| e.into_inner());
    let mut limits = Limits::for_tests();
    limits.max_ops = 2;
    let mut session = spawn(limits);
    let first = session.call(&Request::Inspect);
    assert!(
        matches!(first.outcome, EngineStatus::Ok { .. }),
        "{:?}",
        first.outcome
    );
    let second = session.call(&Request::Inspect);
    assert!(
        matches!(second.outcome, EngineStatus::Ok { .. }),
        "{:?}",
        second.outcome
    );
    let third = session.call(&Request::Inspect);
    assert!(
        matches!(
            third.outcome,
            EngineStatus::ResourceLimit {
                kind: LimitKind::Ops,
                ..
            }
        ),
        "{:?}",
        third.outcome
    );
}

#[cfg(feature = "test-hang")]
#[test]
fn delivered_frame_survives_child_exit() {
    let _g = SPAWN_TEST.lock().unwrap_or_else(|e| e.into_inner());
    let mut session = EngineSession::spawn(SpawnRequest {
        engine_bin: bin(),
        limits: Limits::for_tests(),
        test_hang_ms: None,
        test_exit_after_read: None,
        test_close_stdout_hang_ms: None,
        test_exit_after_write: Some(0),
    })
    .unwrap_or_else(|r| panic!("spawn: {:?}", r.outcome));
    let pid = session.pid().expect("pid");
    let report = session.call(&Request::Ping);
    assert!(
        matches!(report.outcome, EngineStatus::Ok { .. }),
        "complete ping frame must not be discarded as ChildCrash, got {:?}",
        report.outcome
    );
    if session.pid().is_some() {
        session.kill_and_reap();
    }
    assert_fully_reaped(pid);
}

#[test]
fn project_empty_success_keeps_child() {
    let _g = SPAWN_TEST.lock().unwrap_or_else(|e| e.into_inner());
    let mut session = spawn(Limits::for_tests());
    let pid = session.pid().expect("pid");
    let report = session.call(&Request::Project { encoding: 1 });
    match report.outcome {
        EngineStatus::Ok {
            applied: false,
            pending: false,
            content_json: Some(json),
            update_b64: None,
            ..
        } => {
            assert_eq!(json, serde_json::json!({"type":"doc","content":[]}));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(session.pid(), Some(pid));
}

#[test]
fn project_output_bound_reaps_child() {
    let _g = SPAWN_TEST.lock().unwrap_or_else(|e| e.into_inner());
    let mut limits = Limits::for_tests();
    limits.max_project_json_bytes = 8;
    let mut session = spawn(limits);
    let pid = session.pid().expect("pid");
    let report = session.call(&Request::Project { encoding: 1 });
    assert!(
        matches!(
            report.outcome,
            EngineStatus::ResourceLimit {
                kind: LimitKind::Output,
                ..
            }
        ),
        "{:?}",
        report.outcome
    );
    assert!(
        session.pid().is_none(),
        "project oversize must recycle child"
    );
    assert_fully_reaped(pid);
}

#[test]
fn project_depth_bound_reaps_child() {
    let _g = SPAWN_TEST.lock().unwrap_or_else(|e| e.into_inner());
    let nested = nested_project_elements(6);
    let mut limits = Limits::for_tests();
    limits.max_project_depth = 2;
    let mut session = spawn(limits);
    let pid = session.pid().expect("pid");
    assert!(matches!(
        session
            .call(&Request::Load {
                snapshot_b64: Some(nested),
                tail_b64: Vec::new(),
                encoding: 1,
            })
            .outcome,
        EngineStatus::Ok { .. }
    ));
    let report = session.call(&Request::Project { encoding: 1 });
    assert!(
        matches!(
            report.outcome,
            EngineStatus::ResourceLimit {
                kind: LimitKind::Stack,
                ..
            }
        ),
        "{:?}",
        report.outcome
    );
    assert!(session.pid().is_none(), "project depth must recycle child");
    assert_fully_reaped(pid);
}

fn nested_project_elements(depth: usize) -> Vec<u8> {
    use yrs::types::xml::XmlIn;
    use yrs::{XmlElementPrelim, XmlFragment, XmlTextPrelim};
    let doc = collab_engine::engine::new_doc();
    let xml = doc.get_or_insert_xml_fragment(collab_engine::FRAGMENT);
    let mut node = XmlIn::from(XmlElementPrelim::new(
        "paragraph",
        [XmlIn::from(XmlTextPrelim::new("x"))],
    ));
    for _ in 1..depth {
        node = XmlIn::from(XmlElementPrelim::new("paragraph", [node]));
    }
    {
        let mut txn = doc.transact_mut();
        let XmlIn::Element(el) = node else {
            panic!("expected element");
        };
        xml.push_back(&mut txn, el);
    }
    let txn = doc.transact();
    txn.encode_state_as_update_v1(&yrs::StateVector::default())
}

fn load_fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join(name),
    )
    .unwrap_or_else(|e| panic!("read {name}: {e}"))
}

#[test]
fn project_map_and_embed_are_malformed_document_errors_not_limits() {
    let _g = SPAWN_TEST.lock().unwrap_or_else(|e| e.into_inner());
    for (file, needle) in [
        ("map_child.v1", "non-XML child in fragment"),
        ("embed_child.v1", "non-XML child in paragraph"),
    ] {
        let mut session = spawn(Limits::for_tests());
        let pid = session.pid().expect("pid");
        assert!(matches!(
            session
                .call(&Request::Load {
                    snapshot_b64: Some(load_fixture(file)),
                    tail_b64: Vec::new(),
                    encoding: 1,
                })
                .outcome,
            EngineStatus::Ok { .. }
        ));
        assert_eq!(session.pid(), Some(pid), "{file} load must keep the child");
        let report = session.call(&Request::Project { encoding: 1 });
        match report.outcome {
            EngineStatus::Malformed { ref detail } if detail.contains(needle) => {}
            EngineStatus::ResourceLimit { .. } => {
                panic!(
                    "{file} document error must not be a resource limit: {:?}",
                    report.outcome
                )
            }
            EngineStatus::WorkerFailure { .. } => {
                panic!(
                    "{file} child must survive and return malformed, got {:?}",
                    report.outcome
                )
            }
            other => panic!("{file}: {other:?}"),
        }
        // EngineSession recycles any non-Ok response (existing parent policy).
        // The helper itself returned a complete malformed frame, not a crash.
        if session.pid().is_some() {
            session.kill_and_reap();
        }
        assert_fully_reaped(pid);
    }
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
