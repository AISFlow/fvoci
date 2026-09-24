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
        test_exit_after_read: None,
        test_close_stdout_hang_ms: None,
        test_exit_after_write: None,
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
    limits.max_load_bytes = 64;
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
fn load_caps_each_blob_before_aggregate() {
    let _g = SPAWN_TEST.lock().unwrap_or_else(|e| e.into_inner());
    let mut limits = Limits::for_tests();
    limits.max_input_bytes = 64;
    limits.max_output_bytes = 64;
    limits.max_load_bytes = 256;
    let mut session = spawn(limits);
    let report = session.call(&Request::Load {
        snapshot_b64: Some(vec![1; 80]),
        tail_b64: Vec::new(),
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
fn apply_complete_v1_reloads_and_oversize_cannot_persist() {
    let _g = SPAWN_TEST.lock().unwrap_or_else(|e| e.into_inner());
    let structured = load_bytes("structured.v1");
    let follow = load_bytes("followup_edit.v1");

    let mut probe = CollabEngine::new(Limits::for_tests());
    assert_ok_applied(&probe.handle(&Request::Load {
        snapshot_b64: Some(structured.clone()),
        tail_b64: Vec::new(),
        encoding: 1,
    }));
    let loaded_len = match probe.handle(&Request::Snapshot) {
        EngineStatus::Ok {
            update_b64: Some(s),
            ..
        } => collab_engine::b64::decode(&s).expect("probe snap").len() as u64,
        other => panic!("probe snapshot: {other:?}"),
    };
    assert!(
        loaded_len >= structured.len() as u64,
        "completeV1 {loaded_len} shorter than committed snapshot {}",
        structured.len()
    );

    let mut roomy = spawn(Limits::for_tests());
    assert_ok_applied(
        &roomy
            .call(&Request::Load {
                snapshot_b64: Some(structured.clone()),
                tail_b64: Vec::new(),
                encoding: 1,
            })
            .outcome,
    );
    let applied = roomy.call(&Request::Apply {
        update_b64: follow.clone(),
        encoding: 1,
    });
    assert_ok_applied(&applied.outcome);
    assert!(
        matches!(
            applied.outcome,
            EngineStatus::Ok {
                update_b64: None,
                ..
            }
        ),
        "apply must return metadata only, got {:?}",
        applied.outcome
    );
    let snap = roomy.call(&Request::Snapshot);
    let admitted = match &snap.outcome {
        EngineStatus::Ok {
            applied: true,
            durable: false,
            update_b64: Some(s),
            ..
        } => collab_engine::b64::decode(s).expect("snapshot completeV1"),
        other => panic!("snapshot must return reloadable completeV1, got {other:?}"),
    };
    drop(roomy);
    let mut reloaded = spawn(Limits::for_tests());
    assert_ok_applied(
        &reloaded
            .call(&Request::Load {
                snapshot_b64: Some(admitted),
                tail_b64: Vec::new(),
                encoding: 1,
            })
            .outcome,
    );
    let xml = xml_of(&mut reloaded);
    assert!(xml.contains("후속편집한글✨"), "{xml}");
    drop(reloaded);

    let mut tight = Limits::for_tests();
    tight.max_input_bytes = loaded_len.saturating_add(8).max(structured.len() as u64);
    tight.max_output_bytes = tight.max_input_bytes;
    tight.max_load_bytes = tight.max_input_bytes.max(64 * 1024);
    let mut session = spawn(tight);
    let pid = session.pid().expect("pid");
    assert_ok_applied(
        &session
            .call(&Request::Load {
                snapshot_b64: Some(structured.clone()),
                tail_b64: Vec::new(),
                encoding: 1,
            })
            .outcome,
    );
    let oversize = session.call(&Request::Apply {
        update_b64: follow,
        encoding: 1,
    });
    assert!(
        matches!(
            oversize.outcome,
            EngineStatus::ResourceLimit {
                kind: LimitKind::Output,
                ..
            }
        ),
        "oversize candidate must fail output before DB admission, got {:?}",
        oversize.outcome
    );
    assert!(session.pid().is_none(), "oversize apply must recycle child");
    assert_fully_reaped(pid);

    let mut original = spawn(Limits::for_tests());
    assert_ok_applied(
        &original
            .call(&Request::Load {
                snapshot_b64: Some(structured),
                tail_b64: Vec::new(),
                encoding: 1,
            })
            .outcome,
    );
    let xml = xml_of(&mut original);
    assert!(
        !xml.contains("후속편집한글✨"),
        "rejected candidate must not persist into a reloaded committed snapshot: {xml}"
    );
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
        test_exit_after_read: None,
        test_close_stdout_hang_ms: None,
        test_exit_after_write: None,
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

#[test]
fn load_with_tail_then_sync_roundtrip() {
    let _g = SPAWN_TEST.lock().unwrap_or_else(|e| e.into_inner());
    let structured = load_bytes("structured.v1");
    let follow = load_bytes("followup_edit.v1");
    let mut session = spawn(Limits::for_tests());
    let load = session.call(&Request::Load {
        snapshot_b64: Some(structured.clone()),
        tail_b64: vec![follow.clone()],
        encoding: 1,
    });
    assert_ok_applied(&load.outcome);
    let xml = xml_of(&mut session);
    assert!(xml.contains("후속편집한글✨"), "{xml}");
    let sv = sv_of(&mut session);
    let sync = session.call(&Request::Sync {
        state_vector_b64: sv,
        encoding: 1,
    });
    let sync_bytes = match &sync.outcome {
        EngineStatus::Ok {
            applied: true,
            update_b64: Some(s),
            ..
        } => collab_engine::b64::decode(s).expect("sync"),
        other => panic!("sync must return update bytes, got {other:?}"),
    };
    assert!(
        sync_bytes.len() < 64,
        "sync against own SV should be tiny, got {}",
        sync_bytes.len()
    );

    let second = session.call(&Request::Load {
        snapshot_b64: Some(structured),
        tail_b64: vec![follow],
        encoding: 1,
    });
    assert!(
        matches!(second.outcome, EngineStatus::Malformed { ref detail } if detail.contains("once per child")),
        "second load must be refused, got {:?}",
        second.outcome
    );
}

#[test]
fn load_after_apply_is_refused_but_ping_then_load_is_ok() {
    let _g = SPAWN_TEST.lock().unwrap_or_else(|e| e.into_inner());
    let structured = load_bytes("structured.v1");
    let mut after_apply = spawn(Limits::for_tests());
    assert_ok_applied(
        &after_apply
            .call(&Request::Apply {
                update_b64: structured.clone(),
                encoding: 1,
            })
            .outcome,
    );
    let refused = after_apply.call(&Request::Load {
        snapshot_b64: Some(structured.clone()),
        tail_b64: Vec::new(),
        encoding: 1,
    });
    assert!(
        matches!(refused.outcome, EngineStatus::Malformed { ref detail } if detail.contains("not after apply")),
        "load after apply must be refused, got {:?}",
        refused.outcome
    );
    drop(after_apply);

    let mut ping_first = spawn(Limits::for_tests());
    let ping = ping_first.call(&Request::Ping);
    assert!(
        matches!(ping.outcome, EngineStatus::Ok { .. }),
        "{:?}",
        ping.outcome
    );
    assert_ok_applied(
        &ping_first
            .call(&Request::Load {
                snapshot_b64: Some(structured),
                tail_b64: Vec::new(),
                encoding: 1,
            })
            .outcome,
    );
}

#[test]
fn near_max_load_duplicate_and_distinct_tails() {
    use std::time::Instant;
    let target = 7 * 1024 * 1024 + 512 * 1024;
    let snap_started = Instant::now();
    // One many-small-node snapshot is reused for the near-32MiB duplicate load.
    let snapshot = fragmented_paragraph_update(1, target, "dup-para");
    let snap_ms = snap_started.elapsed().as_millis();
    let heavy_started = Instant::now();
    // Distinct-tail output-limit case uses few large chunks, not 4 more small-node gens.
    let heavy = [
        coarse_chunk_update(11, 2 * 1024 * 1024 + 512 * 1024, "heavy-a"),
        coarse_chunk_update(12, 2 * 1024 * 1024 + 512 * 1024, "heavy-b"),
        coarse_chunk_update(13, 2 * 1024 * 1024 + 512 * 1024, "heavy-c"),
        coarse_chunk_update(14, 2 * 1024 * 1024 + 512 * 1024, "heavy-d"),
    ];
    eprintln!(
        "near_max_load fixture snapshot {}ms {}B; heavy {}ms {:?}",
        snap_ms,
        snapshot.len(),
        heavy_started.elapsed().as_millis(),
        heavy.iter().map(Vec::len).collect::<Vec<_>>()
    );
    assert!(
        snapshot.len() as u64 <= collab_engine::limits::MAX_INPUT_BYTES,
        "snapshot {} exceeds blob cap",
        snapshot.len()
    );
    let aggregate = snapshot.len() * 4;
    assert!(
        aggregate as u64 > 28 * 1024 * 1024
            && aggregate as u64 <= collab_engine::limits::MAX_LOAD_BYTES,
        "need near-32MiB aggregate, got {aggregate}"
    );

    let _g = SPAWN_TEST.lock().unwrap_or_else(|e| e.into_inner());
    let child_started = Instant::now();
    let mut dup = spawn(Limits::default());
    let load = dup.call(&Request::Load {
        snapshot_b64: Some(snapshot.clone()),
        tail_b64: vec![snapshot.clone(), snapshot.clone(), snapshot.clone()],
        encoding: 1,
    });
    assert_ok_applied(&load.outcome);
    let snap = dup.call(&Request::Snapshot);
    let restored = match snap.outcome {
        EngineStatus::Ok {
            update_b64: Some(s),
            ..
        } => collab_engine::b64::decode(&s).expect("dup snap"),
        other => panic!("duplicate tails should restore within blob cap, got {other:?}"),
    };
    assert!(
        restored.len() as u64 <= collab_engine::limits::MAX_INPUT_BYTES,
        "duplicate-tail completeV1 {} must fit 8MiB",
        restored.len()
    );
    drop(dup);
    let mut reloaded = spawn(Limits::default());
    assert_ok_applied(
        &reloaded
            .call(&Request::Load {
                snapshot_b64: Some(restored),
                tail_b64: Vec::new(),
                encoding: 1,
            })
            .outcome,
    );
    drop(reloaded);

    let mut heavy_session = spawn(Limits::default());
    let [a, b, c, d] = heavy;
    let heavy_load = heavy_session.call(&Request::Load {
        snapshot_b64: Some(a),
        tail_b64: vec![b, c, d],
        encoding: 1,
    });
    match heavy_load.outcome {
        EngineStatus::Ok { .. } => {
            let heavy_snap = heavy_session.call(&Request::Snapshot);
            assert!(
                matches!(
                    heavy_snap.outcome,
                    EngineStatus::ResourceLimit {
                        kind: LimitKind::Output | LimitKind::Memory,
                        ..
                    }
                ),
                "structurally heavy merge must not claim a reloadable 8MiB snapshot, got {:?}",
                heavy_snap.outcome
            );
        }
        EngineStatus::ResourceLimit {
            kind: LimitKind::Memory | LimitKind::Output | LimitKind::Time,
            ..
        } => {}
        other => panic!("structurally heavy input must expose resource failure, got {other:?}"),
    }
    eprintln!(
        "near_max_load child path {}ms",
        child_started.elapsed().as_millis()
    );
}

fn fragmented_paragraph_update(client_id: u64, target: usize, token: &str) -> Vec<u8> {
    // 96-byte XmlText nodes (64–128) reproduce small-paragraph memory amplification.
    xml_fragment_update(client_id, target, token, 96)
}

fn coarse_chunk_update(client_id: u64, target: usize, token: &str) -> Vec<u8> {
    xml_fragment_update(client_id, target, token, 16 * 1024)
}

fn xml_fragment_update(client_id: u64, target: usize, token: &str, payload_len: usize) -> Vec<u8> {
    use yrs::{ClientID, Options, XmlFragment, XmlTextPrelim};
    let doc = yrs::Doc::with_options(Options {
        skip_gc: true,
        offset_kind: yrs::OffsetKind::Utf16,
        client_id: ClientID::new(client_id),
        ..Options::default()
    });
    let xml = doc.get_or_insert_xml_fragment(collab_engine::FRAGMENT);
    let mut payload = String::with_capacity(payload_len);
    payload.push_str(token);
    payload.push('-');
    while payload.len() < payload_len {
        payload.push('x');
    }
    // Item metadata is larger than the payload; overestimate then top up.
    let encoded_per = payload_len.saturating_add(64).max(80);
    let mut next_i = 0u32;
    let mut insert_count = (target / encoded_per).max(1);
    let mut encodes = 0u8;
    const MAX_ENCODES: u8 = 4;
    let mut encoded = Vec::new();
    while encodes < MAX_ENCODES {
        {
            let mut txn = doc.transact_mut();
            // push_front(0) avoids XmlFragment insert(len) sibling walks.
            for k in (next_i..next_i + insert_count as u32).rev() {
                xml.push_front(&mut txn, XmlTextPrelim::new(format!("{k:06}-{payload}")));
            }
        }
        next_i += insert_count as u32;
        encoded = {
            let txn = doc.transact();
            txn.encode_state_as_update_v1(&yrs::StateVector::default())
        };
        encodes += 1;
        assert!(
            encoded.len() as u64 <= collab_engine::limits::MAX_INPUT_BYTES,
            "fragmented update {} exceeded blob cap after {encodes} encodes / {next_i} nodes",
            encoded.len()
        );
        if encoded.len() + 256 * 1024 >= target {
            break;
        }
        let missing = target.saturating_sub(encoded.len());
        insert_count = (missing / encoded_per).max(1);
    }
    assert!(
        encoded.len() + 1024 * 1024 >= target,
        "encode {} far below target {target} after {encodes} encodes / {next_i} nodes",
        encoded.len()
    );
    eprintln!(
        "xml_fragment_update token={token} payload={payload_len} nodes={next_i} encodes={encodes} bytes={}",
        encoded.len()
    );
    encoded
}

fn project_of(engine: &mut CollabEngine) -> Value {
    match engine.handle(&Request::Project { encoding: 1 }) {
        EngineStatus::Ok {
            applied: false,
            content_json: Some(json),
            update_b64: None,
            ..
        } => json,
        other => panic!("project: {other:?}"),
    }
}

#[test]
fn project_fresh_empty_doc() {
    let mut engine = CollabEngine::new(Limits::for_tests());
    let json = project_of(&mut engine);
    assert_eq!(json, expectations()["empty_doc"]["prosemirror_json"]);
}

#[test]
fn project_structured_matches_pinned_js() {
    let mut engine = CollabEngine::new(Limits::for_tests());
    assert_ok_applied(&engine.handle(&Request::Load {
        snapshot_b64: Some(load_bytes("structured.v1")),
        tail_b64: Vec::new(),
        encoding: 1,
    }));
    let json = project_of(&mut engine);
    assert_eq!(json, expectations()["structured"]["prosemirror_json"]);
}

#[test]
fn project_korean_emoji_after_mid_and_delete() {
    let mut engine = CollabEngine::new(Limits::for_tests());
    assert_ok_applied(&engine.handle(&Request::Load {
        snapshot_b64: Some(load_bytes("korean_emoji_base.v1")),
        tail_b64: vec![
            load_bytes("korean_emoji_mid_edit.v1"),
            load_bytes("korean_emoji_delete.v1"),
        ],
        encoding: 1,
    }));
    let json = project_of(&mut engine);
    assert_eq!(json, expectations()["korean_emoji"]["prosemirror_json"]);
}

#[test]
fn project_delete_only_changes_json_not_state_vector() {
    let mut engine = CollabEngine::new(Limits::for_tests());
    assert_ok_applied(&engine.handle(&Request::Load {
        snapshot_b64: Some(load_bytes("delete_only_base.v1")),
        tail_b64: Vec::new(),
        encoding: 1,
    }));
    let before = project_of(&mut engine);
    let sv_before = match engine.handle(&Request::Inspect) {
        EngineStatus::Ok {
            state_vector_b64: Some(s),
            ..
        } => collab_engine::b64::decode(&s).expect("sv"),
        other => panic!("{other:?}"),
    };
    assert_ok_applied(&engine.handle(&Request::Apply {
        update_b64: load_bytes("delete_only.v1"),
        encoding: 1,
    }));
    let after = project_of(&mut engine);
    let sv_after = match engine.handle(&Request::Inspect) {
        EngineStatus::Ok {
            state_vector_b64: Some(s),
            ..
        } => collab_engine::b64::decode(&s).expect("sv"),
        other => panic!("{other:?}"),
    };
    assert_eq!(sv_before, sv_after);
    assert_eq!(sv_before, load_bytes("sv_before_delete.bin"));
    assert_eq!(
        before,
        expectations()["delete_only"]["prosemirror_json_before"]
    );
    assert_eq!(
        after,
        expectations()["delete_only"]["prosemirror_json_after"]
    );
    assert_ne!(before, after);
}

#[test]
fn project_pending_then_dependency() {
    let mut engine = CollabEngine::new(Limits::for_tests());
    assert_ok_applied(&engine.handle(&Request::Apply {
        update_b64: load_bytes("pending_u2.v1"),
        encoding: 1,
    }));
    match engine.handle(&Request::Project { encoding: 1 }) {
        EngineStatus::Ok {
            pending: true,
            content_json: Some(json),
            ..
        } => {
            assert_eq!(json, expectations()["pending"]["prosemirror_json_u2_only"]);
        }
        other => panic!("{other:?}"),
    }
    assert_ok_applied(&engine.handle(&Request::Apply {
        update_b64: load_bytes("pending_u1.v1"),
        encoding: 1,
    }));
    match engine.handle(&Request::Project { encoding: 1 }) {
        EngineStatus::Ok {
            pending: false,
            content_json: Some(json),
            ..
        } => {
            assert_eq!(json, expectations()["pending"]["prosemirror_json_both"]);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn project_followup_and_typed_marks_ychange_empty_para() {
    let exp = expectations();
    let mut structured = CollabEngine::new(Limits::for_tests());
    assert_ok_applied(&structured.handle(&Request::Load {
        snapshot_b64: Some(load_bytes("structured.v1")),
        tail_b64: vec![load_bytes("followup_edit.v1")],
        encoding: 1,
    }));
    assert_eq!(
        project_of(&mut structured),
        exp["followup"]["prosemirror_json"]
    );

    for (file, path) in [
        ("empty_paragraph.v1", "/empty_paragraph/prosemirror_json"),
        ("typed_attrs.v1", "/typed_attrs/prosemirror_json"),
        ("marks_link_bold.v1", "/marks_link_bold/prosemirror_json"),
        ("ychange_strip.v1", "/ychange_strip/prosemirror_json"),
        ("ychange_only.v1", "/ychange_only/prosemirror_json"),
        (
            "ychange_retained_nested.v1",
            "/ychange_retained_nested/prosemirror_json",
        ),
    ] {
        let mut engine = CollabEngine::new(Limits::for_tests());
        assert_ok_applied(&engine.handle(&Request::Load {
            snapshot_b64: Some(load_bytes(file)),
            tail_b64: Vec::new(),
            encoding: 1,
        }));
        let got = project_of(&mut engine);
        let expected = exp.pointer(path).unwrap_or_else(|| panic!("{path}"));
        assert_eq!(&got, expected, "{file}");
    }
}

#[test]
fn project_does_not_block_later_load_and_counts_ops() {
    let mut limits = Limits::for_tests();
    limits.max_ops = 3;
    let mut engine = CollabEngine::new(limits);
    let _ = project_of(&mut engine);
    assert_ok_applied(&engine.handle(&Request::Load {
        snapshot_b64: Some(load_bytes("utf8_korean.v1")),
        tail_b64: Vec::new(),
        encoding: 1,
    }));
    let _ = project_of(&mut engine);
    let refused = engine.handle(&Request::Project { encoding: 1 });
    assert!(
        matches!(
            refused,
            EngineStatus::ResourceLimit {
                kind: LimitKind::Ops,
                ..
            }
        ),
        "{refused:?}"
    );
}

#[test]
fn project_output_and_depth_caps() {
    let mut tiny = Limits::for_tests();
    tiny.max_project_json_bytes = 8;
    let mut engine = CollabEngine::new(tiny);
    let over = engine.handle(&Request::Project { encoding: 1 });
    assert!(
        matches!(
            over,
            EngineStatus::ResourceLimit {
                kind: LimitKind::Output,
                ..
            }
        ),
        "{over:?}"
    );

    let nested = nested_project_update(5);
    let mut shallow = Limits::for_tests();
    shallow.max_project_depth = 2;
    let mut engine = CollabEngine::new(shallow);
    assert_ok_applied(&engine.handle(&Request::Load {
        snapshot_b64: Some(nested),
        tail_b64: Vec::new(),
        encoding: 1,
    }));
    let deep = engine.handle(&Request::Project { encoding: 1 });
    assert!(
        matches!(
            deep,
            EngineStatus::ResourceLimit {
                kind: LimitKind::Stack,
                ..
            }
        ),
        "{deep:?}"
    );
}

fn nested_project_update(depth: usize) -> Vec<u8> {
    use yrs::types::xml::XmlIn;
    use yrs::{XmlElementPrelim, XmlFragment, XmlTextPrelim};
    let doc = new_doc();
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
