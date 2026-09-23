use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use collab_engine::engine::new_doc;
use collab_engine::outcome::{EngineStatus, LimitKind};
use collab_engine::protocol::Request;
use collab_engine::{CollabEngine, EngineSession, Limits, SpawnRequest, WorkerFailureReason};
use serde_json::Value;
use yrs::types::ToJson;
use yrs::updates::decoder::Decode;
use yrs::{ReadTxn, Transact, Update};

static SPAWN_TEST: Mutex<()> = Mutex::new(());

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

fn load_bytes(name: &str) -> Vec<u8> {
    fs::read(fixtures().join(name)).unwrap_or_else(|e| panic!("read {name}: {e}"))
}

fn expectations() -> Value {
    serde_json::from_slice(&load_bytes("expectations.json")).expect("expectations.json")
}

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_collab-engine"))
}

fn spawn(limits: Limits) -> EngineSession {
    EngineSession::spawn(SpawnRequest {
        engine_bin: bin(),
        limits,
        test_hang_ms: None,
    })
    .unwrap_or_else(|r| panic!("spawn: {:?}", r.outcome))
}

fn xml_of(session: &mut EngineSession) -> String {
    match session.call(&Request::Inspect).outcome {
        EngineStatus::Ok {
            xml_string: Some(s),
            ..
        } => s,
        other => panic!("inspect: {other:?}"),
    }
}

fn pending_of(session: &mut EngineSession) -> bool {
    match session.call(&Request::Inspect).outcome {
        EngineStatus::Ok { pending, .. } => pending,
        other => panic!("inspect: {other:?}"),
    }
}

fn sv_of(session: &mut EngineSession) -> Vec<u8> {
    match session.call(&Request::Inspect).outcome {
        EngineStatus::Ok {
            state_vector_b64: Some(s),
            ..
        } => collab_engine::b64::decode(&s).expect("sv b64"),
        other => panic!("inspect sv: {other:?}"),
    }
}

fn assert_ok_applied(status: &EngineStatus) {
    assert!(
        matches!(
            status,
            EngineStatus::Ok {
                applied: true,
                durable: false,
                ..
            }
        ),
        "expected applied+not-durable, got {status:?}"
    );
}

#[test]
fn structured_prosemirror_roundtrip_via_child() {
    let _g = SPAWN_TEST.lock().unwrap_or_else(|e| e.into_inner());
    let exp = expectations();
    let mut session = spawn(Limits::for_tests());
    let report = session.call(&Request::Load {
        snapshot_b64: Some(load_bytes("structured.v1")),
        tail_b64: Vec::new(),
        encoding: 1,
    });
    assert_ok_applied(&report.outcome);
    let xml = xml_of(&mut session);
    for token in exp["structured"]["must_include"].as_array().unwrap() {
        let t = token.as_str().unwrap();
        assert!(xml.contains(t), "missing {t} in {xml}");
    }
    assert!(xml.contains("p-alpha-001"), "{xml}");
    assert!(xml.contains("tbl-001"), "{xml}");
    assert!(xml.contains("user-42"), "{xml}");
    assert!(xml.contains("https://example.invalid/wiki/안녕"), "{xml}");
    if let Some(pid) = session.pid() {
        session.kill_and_reap();
        assert_fully_reaped(pid);
    }
}

#[test]
fn korean_emoji_middle_edit_and_delete() {
    let _g = SPAWN_TEST.lock().unwrap_or_else(|e| e.into_inner());
    let mut session = spawn(Limits::for_tests());
    let load = session.call(&Request::Load {
        snapshot_b64: Some(load_bytes("korean_emoji_base.v1")),
        tail_b64: Vec::new(),
        encoding: 1,
    });
    assert_ok_applied(&load.outcome);
    let mid = session.call(&Request::Apply {
        update_b64: load_bytes("korean_emoji_mid_edit.v1"),
        encoding: 1,
    });
    assert_ok_applied(&mid.outcome);
    let del = session.call(&Request::Apply {
        update_b64: load_bytes("korean_emoji_delete.v1"),
        encoding: 1,
    });
    assert_ok_applied(&del.outcome);
    let xml = xml_of(&mut session);
    assert!(xml.contains("가"), "{xml}");
    assert!(xml.contains("중"), "{xml}");
    assert!(xml.contains("🚀"), "{xml}");
    assert!(xml.contains("마바사"), "{xml}");
    assert!(!xml.contains("나다"), "{xml}");
}

#[test]
fn load_caps_cumulative_tail_before_apply() {
    let _g = SPAWN_TEST.lock().unwrap_or_else(|e| e.into_inner());
    let mut limits = Limits::for_tests();
    limits.max_input_bytes = 64;
    limits.max_output_bytes = 64;
    let mut session = spawn(limits);
    let report = session.call(&Request::Load {
        snapshot_b64: Some(vec![1; 40]),
        tail_b64: vec![vec![2; 40]],
        encoding: 1,
    });
    assert!(
        matches!(
            report.outcome,
            EngineStatus::ResourceLimit {
                kind: LimitKind::Input,
                ..
            }
        ),
        "{:?}",
        report.outcome
    );
}

#[test]
fn delete_only_state_vector_matches_yjs_fixture() {
    let _g = SPAWN_TEST.lock().unwrap_or_else(|e| e.into_inner());
    let exp = expectations();
    let mut session = spawn(Limits::for_tests());
    let load = session.call(&Request::Load {
        snapshot_b64: Some(load_bytes("delete_only_base.v1")),
        tail_b64: Vec::new(),
        encoding: 1,
    });
    assert_ok_applied(&load.outcome);
    let sv_before = sv_of(&mut session);
    let del = session.call(&Request::Apply {
        update_b64: load_bytes("delete_only.v1"),
        encoding: 1,
    });
    assert_ok_applied(&del.outcome);
    let sv_after = sv_of(&mut session);
    let yjs_before = load_bytes("sv_before_delete.bin");
    let yjs_after = load_bytes("sv_after_delete.bin");
    assert_eq!(sv_before, yjs_before, "engine SV before delete");
    assert_eq!(sv_after, yjs_after, "engine SV after delete-only");
    let unchanged = exp["delete_only"]["state_vector_unchanged"]
        .as_bool()
        .expect("flag");
    assert_eq!(sv_before == sv_after, unchanged);
}

#[test]
fn pending_out_of_order_then_dependency_and_duplicate() {
    let _g = SPAWN_TEST.lock().unwrap_or_else(|e| e.into_inner());
    let mut session = spawn(Limits::for_tests());
    let u2 = session.call(&Request::Apply {
        update_b64: load_bytes("pending_u2.v1"),
        encoding: 1,
    });
    assert_ok_applied(&u2.outcome);
    assert!(
        matches!(u2.outcome, EngineStatus::Ok { pending: true, .. }),
        "{:?}",
        u2.outcome
    );
    let snap_pending = session.call(&Request::Snapshot);
    let pending_bytes = match &snap_pending.outcome {
        EngineStatus::Ok {
            update_b64: Some(s),
            pending: true,
            ..
        } => collab_engine::b64::decode(s).expect("snap"),
        other => panic!("snapshot while pending: {other:?}"),
    };

    let u1 = session.call(&Request::Apply {
        update_b64: load_bytes("pending_u1.v1"),
        encoding: 1,
    });
    assert_ok_applied(&u1.outcome);
    let dup = session.call(&Request::Apply {
        update_b64: load_bytes("pending_u2.v1"),
        encoding: 1,
    });
    assert_ok_applied(&dup.outcome);
    let xml = xml_of(&mut session);
    assert!(xml.contains("one"), "{xml}");
    assert!(xml.contains("two한글"), "{xml}");
    assert!(!pending_of(&mut session));

    drop(session);
    let mut reloaded = spawn(Limits::for_tests());
    let load = reloaded.call(&Request::Load {
        snapshot_b64: Some(pending_bytes),
        tail_b64: Vec::new(),
        encoding: 1,
    });
    assert_ok_applied(&load.outcome);
    assert!(pending_of(&mut reloaded), "pending must survive snapshot");
    let dep = reloaded.call(&Request::Apply {
        update_b64: load_bytes("pending_u1.v1"),
        encoding: 1,
    });
    assert_ok_applied(&dep.outcome);
    let xml2 = xml_of(&mut reloaded);
    assert!(xml2.contains("one") && xml2.contains("two한글"), "{xml2}");
}

#[test]
fn fresh_child_reload_then_js_followup_converges() {
    let _g = SPAWN_TEST.lock().unwrap_or_else(|e| e.into_inner());
    let mut session = spawn(Limits::for_tests());
    session.call(&Request::Load {
        snapshot_b64: Some(load_bytes("structured.v1")),
        tail_b64: Vec::new(),
        encoding: 1,
    });
    let snap = session.call(&Request::Snapshot);
    let bytes = match snap.outcome {
        EngineStatus::Ok {
            update_b64: Some(s),
            ..
        } => collab_engine::b64::decode(&s).expect("complete v1"),
        other => panic!("{other:?}"),
    };
    drop(session);
    let mut fresh = spawn(Limits::for_tests());
    let load = fresh.call(&Request::Load {
        snapshot_b64: Some(bytes),
        tail_b64: Vec::new(),
        encoding: 1,
    });
    assert_ok_applied(&load.outcome);
    let follow = fresh.call(&Request::Apply {
        update_b64: load_bytes("followup_edit.v1"),
        encoding: 1,
    });
    assert_ok_applied(&follow.outcome);
    let xml = xml_of(&mut fresh);
    assert!(xml.contains("후속편집한글✨"), "{xml}");
    assert!(xml.contains("안녕 본문"), "{xml}");
}

#[test]
fn skip_gc_revision_snapshot_restores_deleted_text() {
    let limits = Limits::for_tests();
    let mut engine = CollabEngine::new(limits);
    let after = engine.handle(&Request::Load {
        snapshot_b64: Some(load_bytes("revision_after.v1")),
        tail_b64: Vec::new(),
        encoding: 1,
    });
    assert_ok_applied(&after);
    let snap = load_bytes("revision_snapshot.bin");
    match engine.encode_from_revision_snapshot(&snap) {
        Ok(restored) => {
            let doc = new_doc();
            {
                let update = Update::decode_v1(&restored).expect("restore decode");
                doc.transact_mut()
                    .apply_update(update)
                    .expect("restore apply");
            }
            let txn = doc.transact();
            let arr = txn.get_array("t").expect("array t");
            let json = arr.to_json(&txn);
            let text = json.to_string();
            assert!(
                text.contains("스냅샷-본문"),
                "skip_gc restore must keep deleted revision text, got {text}"
            );
        }
        Err(status) => panic!(
            "concrete Yjs 13.6.32 snapshot incompatibility with yrs 0.28.0+small-client: {status:?} fixture=fixtures/revision_snapshot.bin ({} bytes). Do not vendor a new parser.",
            snap.len()
        ),
    }
}

#[test]
fn invalid_utf8_is_malformed_result_and_child_is_recycled() {
    let _g = SPAWN_TEST.lock().unwrap_or_else(|e| e.into_inner());
    let mut bytes = load_bytes("utf8_korean.v1");
    let marker = [0xEC, 0x95, 0x88]; // first 3 bytes of 안
    let pos = bytes
        .windows(3)
        .position(|w| w == marker)
        .expect("안녕 utf8 in fixture");
    bytes[pos] = 0xFF;
    let mut session = spawn(Limits::for_tests());
    let pid = session.pid().expect("pid");
    let report = session.call(&Request::Apply {
        update_b64: bytes,
        encoding: 1,
    });
    assert!(
        matches!(report.outcome, EngineStatus::Malformed { .. }),
        "invalid UTF-8 must be Result/malformed, got {:?}",
        report.outcome
    );
    assert!(session.pid().is_none(), "rejected child must be reaped");
    assert_fully_reaped(pid);
}

#[test]
fn oversize_candidate_is_resource_not_host_oom() {
    let _g = SPAWN_TEST.lock().unwrap_or_else(|e| e.into_inner());
    let mut limits = Limits::for_tests();
    limits.max_input_bytes = 64;
    limits.max_output_bytes = 64;
    let mut session = spawn(limits);
    let pid = session.pid().expect("pid");
    let report = session.call(&Request::Apply {
        update_b64: vec![1; 128],
        encoding: 1,
    });
    assert!(
        matches!(
            report.outcome,
            EngineStatus::ResourceLimit {
                kind: LimitKind::Input,
                ..
            }
        ),
        "{:?}",
        report.outcome
    );
    assert_fully_reaped(pid);
}

#[test]
fn encoding_v2_is_unsupported() {
    let _g = SPAWN_TEST.lock().unwrap_or_else(|e| e.into_inner());
    let mut session = spawn(Limits::for_tests());
    let report = session.call(&Request::Apply {
        update_b64: load_bytes("structured.v1"),
        encoding: 2,
    });
    assert!(
        matches!(
            report.outcome,
            EngineStatus::Unsupported {
                reason: collab_engine::UnsupportedReason::EncodingV2,
                ..
            }
        ),
        "{:?}",
        report.outcome
    );
}

#[test]
fn missing_binary_is_worker_failure() {
    let report = EngineSession::spawn(SpawnRequest {
        engine_bin: PathBuf::from("/no/such/collab-engine"),
        limits: Limits::for_tests(),
        test_hang_ms: None,
    });
    let err = report.err().expect("spawn fail");
    assert!(
        matches!(
            err.outcome,
            EngineStatus::WorkerFailure {
                reason: WorkerFailureReason::MissingExecutable,
                ..
            }
        ),
        "{:?}",
        err.outcome
    );
}

fn assert_fully_reaped(pid: u32) {
    let path = format!("/proc/{pid}");
    if !Path::new(&path).exists() {
        return;
    }
    let status = fs::read_to_string(format!("/proc/{pid}/status")).unwrap_or_default();
    if status.contains("State:\tZ") || status.to_ascii_lowercase().contains("zombie") {
        panic!("pid {pid} is a zombie; kill+reap failed");
    }
    panic!("pid {pid} still exists after session ended:\n{status}");
}
