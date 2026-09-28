use std::process::{Command, Stdio};
use std::sync::Mutex;
#[cfg(feature = "test-hang")]
use std::time::Instant;

use collab_engine::limits::{Limits, MIN_CHILD_STACK_BYTES};
use collab_engine::outcome::{EngineStatus, LimitKind};
use collab_engine::process::EngineSession;
use collab_engine::protocol::{preflight_wire_json, Request};
use collab_engine::{SpawnRequest, WorkerFailureReason};
use serde_json::json;
use yrs::{Any, Map, ReadTxn, StateVector, Transact};

static SPAWN_TEST: Mutex<()> = Mutex::new(());

fn bin() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_collab-engine"))
}

fn spawn(limits: Limits) -> EngineSession {
    EngineSession::spawn(SpawnRequest {
        engine_bin: bin(),
        limits,
        slot_kind: collab_engine::process::ChildSlotKind::Primary,
        slot_wait: None,
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
        slot_kind: collab_engine::process::ChildSlotKind::Primary,
        slot_wait: None,
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
        slot_kind: collab_engine::process::ChildSlotKind::Primary,
        slot_wait: None,
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
fn limit_plus_one_live_child_is_immediate_resource_limit() {
    let _g = SPAWN_TEST.lock().unwrap_or_else(|e| e.into_inner());
    let cap = 4usize;
    collab_engine::process::set_max_child_concurrency(cap);
    let mut live = Vec::new();
    for i in 0..cap {
        live.push(spawn(Limits::for_tests()));
        let _ = i;
    }
    let over = EngineSession::spawn(SpawnRequest {
        engine_bin: bin(),
        limits: Limits::for_tests(),
        slot_kind: collab_engine::process::ChildSlotKind::Primary,
        slot_wait: None,
        test_hang_ms: None,
        test_exit_after_read: None,
        test_close_stdout_hang_ms: None,
        test_exit_after_write: None,
    });
    let err = over.err().expect("limit+1 must be refused");
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

#[test]
fn seed_pool_is_bounded_and_separate_from_primary() {
    use collab_engine::process::ChildSlotKind;
    use std::time::{Duration, Instant};

    let _g = SPAWN_TEST.lock().unwrap_or_else(|e| e.into_inner());
    let cap = 2usize;
    collab_engine::process::set_max_seed_child_concurrency(cap);
    let seed_req = |wait: Option<Duration>| SpawnRequest {
        engine_bin: bin(),
        limits: Limits::for_tests(),
        slot_kind: ChildSlotKind::Seed,
        slot_wait: wait,
        test_hang_ms: None,
        test_exit_after_read: None,
        test_close_stdout_hang_ms: None,
        test_exit_after_write: None,
    };
    let held: Vec<EngineSession> = (0..cap)
        .map(|_| EngineSession::spawn(seed_req(None)).unwrap_or_else(|r| panic!("{:?}", r.outcome)))
        .collect();
    let wait = Duration::from_millis(300);
    let started = Instant::now();
    let over = EngineSession::spawn(seed_req(Some(wait)))
        .err()
        .expect("seed cap+1 must be refused");
    assert!(started.elapsed() >= wait, "refused before the bounded wait");
    assert!(
        matches!(
            over.outcome,
            EngineStatus::ResourceLimit {
                kind: LimitKind::Ops,
                ..
            }
        ),
        "{:?}",
        over.outcome
    );
    // Saturated seeds leave the primary (room) pool untouched.
    let mut room = spawn(Limits::for_tests());
    assert!(matches!(
        room.call(&Request::Ping).outcome,
        EngineStatus::Ok { .. }
    ));
    drop(room);
    // A waiting seed takes a slot as soon as one is released.
    let mut held = held;
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        held.pop();
        held
    });
    let mut waited = EngineSession::spawn(seed_req(Some(Duration::from_secs(5))))
        .unwrap_or_else(|r| panic!("waiting seed: {:?}", r.outcome));
    assert!(matches!(
        waited.call(&Request::Ping).outcome,
        EngineStatus::Ok { .. }
    ));
    drop(release.join().unwrap());
    collab_engine::process::set_max_seed_child_concurrency(
        collab_engine::limits::DEFAULT_MAX_SEED_CHILD_CONCURRENCY,
    );
}

#[cfg(feature = "test-hang")]
#[test]
fn child_sets_oom_score_adj() {
    let _g = SPAWN_TEST.lock().unwrap_or_else(|e| e.into_inner());
    let mut session = spawn(Limits::for_tests());
    let pid = session.pid().expect("pid");
    // The helper raises its own value at startup, before it reads a frame:
    // after the first reply it is in place.
    assert!(matches!(
        session.call(&Request::Ping).outcome,
        EngineStatus::Ok { .. }
    ));
    assert_eq!(
        collab_engine::process::child_oom_score_adj(pid),
        Some(1000),
        "helper must raise oom_score_adj so cgroup OOM prefers helpers"
    );
    session.kill_and_reap();
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
        slot_kind: collab_engine::process::ChildSlotKind::Primary,
        slot_wait: None,
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
        slot_kind: collab_engine::process::ChildSlotKind::Primary,
        slot_wait: None,
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
        slot_kind: collab_engine::process::ChildSlotKind::Primary,
        slot_wait: None,
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

#[test]
fn revision_snapshots_equal_wire_preflight_and_child_malformed() {
    let limits = Limits::for_tests();
    let oversized = vec![0u8; limits.max_input_bytes as usize + 1];
    let left = collab_engine::b64::encode(&oversized);
    let wire = json!({
        "op": "revision_snapshots_equal",
        "left_b64": left,
        "right_b64": "AA==",
    });
    assert!(
        matches!(
            preflight_wire_json(&wire, &limits),
            Err(EngineStatus::ResourceLimit {
                kind: LimitKind::Input,
                ..
            })
        ),
        "oversize left_b64 must be capped on the wire"
    );

    let _g = SPAWN_TEST.lock().unwrap_or_else(|e| e.into_inner());
    let mut session = spawn(limits);
    let load = session.call(&Request::Load {
        snapshot_b64: Some(vec![0, 0]),
        tail_b64: Vec::new(),
        encoding: 1,
    });
    assert!(
        matches!(load.outcome, EngineStatus::Ok { .. }),
        "{:?}",
        load.outcome
    );
    let snap = match session.call(&Request::RevisionSnapshot).outcome {
        EngineStatus::Ok {
            update_b64: Some(bytes),
            ..
        } => collab_engine::b64::decode(&bytes).expect("snap"),
        other => panic!("revision_snapshot: {other:?}"),
    };
    let report = session.call(&Request::RevisionSnapshotsEqual {
        left_b64: snap,
        right_b64: vec![0xFF, 0x01, 0x02, 0x03],
    });
    assert!(
        matches!(report.outcome, EngineStatus::Malformed { .. }),
        "garbage snapshots must be Malformed in the child, got {:?}",
        report.outcome
    );
}

#[cfg(feature = "test-hang")]
#[test]
fn delivered_frame_survives_child_exit() {
    let _g = SPAWN_TEST.lock().unwrap_or_else(|e| e.into_inner());
    let mut session = EngineSession::spawn(SpawnRequest {
        engine_bin: bin(),
        limits: Limits::for_tests(),
        slot_kind: collab_engine::process::ChildSlotKind::Primary,
        slot_wait: None,
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

#[cfg(feature = "test-hang")]
#[derive(Debug, PartialEq, Eq)]
enum PidView {
    Gone,
    Zombie,
    Live,
}

/// `/proc/<pid>/stat` field 22: pins a pid to one process across pid reuse.
#[cfg(feature = "test-hang")]
fn proc_starttime(pid: u32) -> Option<u64> {
    let raw = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rest = raw.rsplit_once(')')?.1;
    rest.split_whitespace().nth(19)?.parse().ok()
}

#[cfg(feature = "test-hang")]
fn proc_status_field(pid: u32, field: &str) -> Option<String> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    status
        .lines()
        .find_map(|line| line.strip_prefix(field))
        .map(|rest| rest.trim().to_string())
}

#[cfg(feature = "test-hang")]
fn observe_pid(pid: u32, starttime: u64) -> PidView {
    if proc_starttime(pid) != Some(starttime) {
        return PidView::Gone;
    }
    match proc_status_field(pid, "State:") {
        None => PidView::Gone,
        Some(state) if state.starts_with('Z') => PidView::Zombie,
        Some(_) => PidView::Live,
    }
}

/// A `collab-parent-death-driver` run and the helper it spawned. Drop SIGKILLs
/// only this owned pair (the helper only while its start time still matches).
#[cfg(feature = "test-hang")]
struct ParentDeathRun {
    driver: std::process::Child,
    helper: (u32, u64),
    dir: std::path::PathBuf,
}

#[cfg(feature = "test-hang")]
impl ParentDeathRun {
    fn start(extra_args: &[&str]) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "fvoci-collab-pdeath-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("pid dir");
        let pid_file = dir.join("helper.pid");
        let mut driver = Command::new(env!("CARGO_BIN_EXE_collab-parent-death-driver"))
            .arg(bin())
            .arg(&pid_file)
            .args(extra_args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn parent-death driver");
        let deadline = Instant::now() + std::time::Duration::from_secs(10);
        let helper_pid = loop {
            if let Some(pid) = std::fs::read_to_string(&pid_file)
                .ok()
                .and_then(|s| s.trim().parse::<u32>().ok())
            {
                break pid;
            }
            if let Ok(Some(status)) = driver.try_wait() {
                let _ = std::fs::remove_dir_all(&dir);
                panic!("parent-death driver exited before writing a pid: {status}");
            }
            if Instant::now() >= deadline {
                let _ = driver.kill();
                let _ = driver.wait();
                let _ = std::fs::remove_dir_all(&dir);
                panic!("parent-death driver wrote no helper pid");
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        let start = proc_starttime(helper_pid).expect("helper start time");
        Self {
            driver,
            helper: (helper_pid, start),
            dir,
        }
    }

    fn helper_view(&self) -> PidView {
        observe_pid(self.helper.0, self.helper.1)
    }

    /// Poll until the helper is no longer running (gone, or a zombie nobody
    /// has reaped yet), for at most `within`.
    fn wait_helper_terminated(&self, within: std::time::Duration) -> PidView {
        let deadline = Instant::now() + within;
        loop {
            let view = self.helper_view();
            if view != PidView::Live || Instant::now() >= deadline {
                return view;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}

#[cfg(feature = "test-hang")]
impl Drop for ParentDeathRun {
    fn drop(&mut self) {
        let _ = self.driver.kill();
        let _ = self.driver.wait();
        let (pid, start) = self.helper;
        if proc_starttime(pid) == Some(start) {
            unsafe {
                libc::kill(pid as libc::pid_t, libc::SIGKILL);
            }
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A server that dies (SIGKILL, crash) mid-request must not leave its helper
/// running until RLIMIT_CPU: PDEATHSIG kills it without the parent's watchdog.
#[cfg(feature = "test-hang")]
#[test]
fn parent_sigkill_kills_hanging_collab_helper() {
    let _g = SPAWN_TEST.lock().unwrap_or_else(|e| e.into_inner());
    let mut run = ParentDeathRun::start(&[]);
    let (helper_pid, _) = run.helper;
    let driver_pid = run.driver.id();
    assert_eq!(
        proc_status_field(helper_pid, "PPid:").as_deref(),
        Some(driver_pid.to_string().as_str()),
        "pid file must name this driver's helper"
    );
    assert_eq!(
        proc_status_field(helper_pid, "Name:").as_deref(),
        Some("collab-engine")
    );
    assert_eq!(
        run.helper_view(),
        PidView::Live,
        "helper must be alive before the parent dies"
    );
    let rc = unsafe { libc::kill(driver_pid as libc::pid_t, libc::SIGKILL) };
    assert_eq!(rc, 0, "SIGKILL driver");
    let _ = run.driver.wait().expect("reap driver");
    let view = run.wait_helper_terminated(std::time::Duration::from_secs(1));
    assert_ne!(
        view,
        PidView::Live,
        "helper must die with its parent (PDEATHSIG), view={view:?}"
    );
}

/// PDEATHSIG follows the spawning thread, not the process: a session leaked
/// past its thread loses its helper while the process lives on. This is why
/// an `EngineSession` must never outlive the thread that spawned it.
#[cfg(feature = "test-hang")]
#[test]
fn spawning_thread_exit_kills_collab_helper() {
    let _g = SPAWN_TEST.lock().unwrap_or_else(|e| e.into_inner());
    let mut run = ParentDeathRun::start(&["--exit-spawning-thread"]);
    let view = run.wait_helper_terminated(std::time::Duration::from_secs(1));
    assert_ne!(
        view,
        PidView::Live,
        "helper must die when its spawning thread exits, view={view:?}"
    );
    assert!(
        run.driver.try_wait().expect("driver status").is_none(),
        "the driver process itself must still be running"
    );
}

/// A non-dumpable server hides its environ from same-uid children, and its
/// helpers still end up at `oom_score_adj` 1000 with readable RSS. Runs in a
/// dedicated driver process: `PR_SET_DUMPABLE` is process-wide and must not
/// touch this shared test process. The driver spawns 500 helpers from four
/// threads while every CPU spins, so a race between spawn and the helper's
/// own write would show up as a helper below 1000.
///
/// Needs a non-root runner and skips as root: root with `CAP_SYS_PTRACE`
/// still reads the environ, and root without it gets EPERM, not EACCES.
#[cfg(feature = "test-hang")]
#[test]
fn non_dumpable_parent_hides_environ_and_helpers_keep_oom_score_adj() {
    if unsafe { libc::geteuid() } == 0 {
        eprintln!(
            "skipping non_dumpable_parent_hides_environ_and_helpers_keep_oom_score_adj: \
             the same-uid environ check needs a non-root runner"
        );
        return;
    }
    let _g = SPAWN_TEST.lock().unwrap_or_else(|e| e.into_inner());
    let marker = format!(
        "marker-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    );
    let out = Command::new(env!("CARGO_BIN_EXE_collab-non-dumpable-driver"))
        .arg(bin())
        .arg("500")
        .env("FVOCI_NON_DUMPABLE_MARKER", &marker)
        .stdin(Stdio::null())
        .output()
        .expect("run non-dumpable driver");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "driver failed: {}\nstdout={stdout}\nstderr={stderr}",
        out.status
    );
    assert!(
        stdout.contains("control=readable after=EACCES helpers=500 oom_1000_rss_ok=500 failures=0"),
        "stdout={stdout}"
    );
}
