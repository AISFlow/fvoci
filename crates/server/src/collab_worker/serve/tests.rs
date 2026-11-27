use super::super::frame::read_frame;
use super::super::wire::decode_response;
use super::*;

const LIMITS: Limits = Limits {
    update_bytes: 1024,
    state_bytes: 4096,
    body_bytes: 4096,
    documents: 1,
    held_bytes: 1 << 20,
};

/// The answers of a worker to `requests`, the configuration of [`LIMITS`] first.
fn answers(requests: &[Request]) -> Vec<Response> {
    let mut input = Vec::new();
    write_frame(&mut input, &Request::Configure(LIMITS).encode(), 64).expect("written");
    for request in requests {
        write_frame(&mut input, &request.encode(), 1 << 20).expect("written");
    }
    let mut output = Vec::new();
    serve(&mut input.as_slice(), &mut output).expect("served");
    let mut output = output.as_slice();
    let configured = read_frame(&mut output, 1 << 20).expect("an answer");
    assert_eq!(
        configured.map(|frame| decode_response(&frame)),
        Some(Ok(Ok(Answer::Done))),
        "the configuration is answered"
    );
    let mut answers = Vec::new();
    while let Some(frame) = read_frame(&mut output, 1 << 20).expect("an answer") {
        answers.push(decode_response(&frame).expect("an answer"));
    }
    answers
}

#[test]
fn a_document_opens_once_and_within_the_number_of_documents() {
    let body = br#"{"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"x"}]}]}"#;
    let got = answers(&[
        Request::Open {
            doc: 1,
            source: Source::Body(Some(body.to_vec())),
            share: Share::Kept,
        },
        Request::Open {
            doc: 1,
            source: Source::Body(None),
            share: Share::Kept,
        },
        Request::Open {
            doc: 2,
            source: Source::Body(None),
            share: Share::Kept,
        },
        Request::Body(1),
        Request::Close(1),
        Request::Body(1),
    ]);
    assert!(matches!(got[0], Ok(Answer::Opened { state: Some(_), .. })));
    let refusal = |index: usize| got[index].as_ref().map_err(|refused| refused.refusal).err();
    assert_eq!(refusal(1), Some(Refusal::UnknownDocument), "open already");
    assert_eq!(refusal(2), Some(Refusal::TooManyDocuments), "one at a time");
    let Ok(Answer::Body(Some(json))) = &got[3] else {
        panic!("its body: {:?}", got[3]);
    };
    let json: serde_json::Value = serde_json::from_slice(json).expect("JSON");
    let sent: serde_json::Value = serde_json::from_slice(body).expect("JSON");
    assert_eq!(json, sent, "its body");
    assert_eq!(refusal(5), Some(Refusal::UnknownDocument), "closed");
}

#[test]
fn a_worker_that_holds_as_many_documents_as_it_may_opens_some_aside() {
    let open = |doc: DocNumber, share: Share| Request::Open {
        doc,
        source: Source::Body(None),
        share,
    };
    let aside: Vec<DocNumber> = (10..).take(ASIDE_DOCUMENTS + 1).collect();
    let mut requests = vec![open(1, Share::Kept)];
    requests.extend(aside.iter().map(|doc| open(*doc, Share::Aside)));
    requests.extend([
        Request::Close(10),
        open(30, Share::Aside),
        open(2, Share::Kept),
    ]);
    let got = answers(&requests);
    let refused = |index: usize| got[index].as_ref().map_err(|refused| refused.refusal).err();
    let last = ASIDE_DOCUMENTS;
    assert!((0..=last).all(|index| refused(index).is_none()), "{got:?}");
    assert_eq!(
        refused(last + 1),
        Some(Refusal::TooManyDocuments),
        "past the share aside"
    );
    assert_eq!(refused(last + 3), None, "one closed aside makes room aside");
    assert_eq!(
        refused(last + 4),
        Some(Refusal::TooManyDocuments),
        "none kept past the limit"
    );
}

#[test]
fn documents_aside_leave_the_kept_share_alone() {
    let open = |doc: DocNumber, share: Share| Request::Open {
        doc,
        source: Source::Body(None),
        share,
    };
    let mut requests: Vec<Request> = (10..)
        .take(ASIDE_DOCUMENTS)
        .map(|doc| open(doc, Share::Aside))
        .collect();
    requests.push(open(1, Share::Kept));
    let got = answers(&requests);
    assert!(got.iter().all(Result::is_ok), "{got:?}");
}

#[test]
fn a_worker_that_is_sent_no_configuration_first_ends() {
    let mut input = Vec::new();
    write_frame(&mut input, &Request::State(1).encode(), 64).expect("written");
    assert!(serve(&mut input.as_slice(), &mut Vec::new()).is_err());
    assert!(
        serve(&mut [].as_slice(), &mut Vec::new()).is_ok(),
        "a pipe closed at once"
    );
}

fn ask(worker: &mut Worker, request: Request) -> Response {
    worker.answer(request).expect("no fatal worker error")
}

#[test]
fn a_document_marked_for_remake_allows_only_body_and_close() {
    let mut worker = Worker::new(LIMITS);
    let opened = ask(
        &mut worker,
        Request::Open {
            doc: 1,
            source: Source::Body(None),
            share: Share::Kept,
        },
    );
    assert!(
        matches!(opened, Ok(Answer::Opened { .. })),
        "opened: {opened:?}"
    );
    worker.docs.get_mut(&1).expect("held").must_remake = true;

    let refusal = |response: Response| response.err().map(|refused| refused.refusal);
    assert_eq!(
        refusal(ask(
            &mut worker,
            Request::Take {
                doc: 1,
                update: Vec::new(),
            }
        )),
        Some(Refusal::OutOfOrder),
        "Take is blocked until remade"
    );
    assert_eq!(
        refusal(ask(
            &mut worker,
            Request::Retake {
                doc: 1,
                update: Vec::new(),
            }
        )),
        Some(Refusal::Unappliable),
        "Retake is refused cleanly so replay falls back to the card"
    );
    assert_eq!(
        refusal(ask(&mut worker, Request::State(1))),
        Some(Refusal::OutOfOrder),
        "State is blocked so unsettled state cannot be folded"
    );
    assert_eq!(
        refusal(ask(&mut worker, Request::StateVector(1))),
        Some(Refusal::OutOfOrder),
        "StateVector is blocked until remade"
    );
    assert_eq!(
        refusal(ask(
            &mut worker,
            Request::Diff {
                doc: 1,
                state_vector: vec![0],
            }
        )),
        Some(Refusal::OutOfOrder),
        "Diff is blocked until remade"
    );
    assert!(
        matches!(ask(&mut worker, Request::Body(1)), Ok(Answer::Body(None))),
        "Body is allowed for projection"
    );
    assert_eq!(
        ask(&mut worker, Request::Close(1)),
        Ok(Answer::Done),
        "Close is allowed for remake or forget"
    );
}

/// Verifies exact `held_bytes` accounting across `Open`, `Take`, repaired `Take`,
/// no-op `Take`, `State` rebase, and `Close`.
#[test]
fn since_bytes_are_counted_in_held_bytes_and_reclaimed_by_state_and_close() {
    let mut worker = Worker::new(Limits {
        update_bytes: 4096,
        state_bytes: 16 * 1024,
        body_bytes: 16 * 1024,
        documents: 4,
        held_bytes: 1 << 20,
    });
    let Ok(Answer::Opened {
        state: Some(initial_state),
        state_bytes: s0,
        ..
    }) = ask(
        &mut worker,
        Request::Open {
            doc: 1,
            source: Source::Body(None),
            share: Share::Kept,
        },
    )
    else {
        panic!("opened");
    };
    assert_eq!(worker.held_bytes, s0, "open accounts state_bytes only");

    let editor = fvoci_collab::testing::Editor::join(101, &initial_state);
    let u1 = editor.append_paragraph("hello");
    let Ok(Answer::Taken {
        repair: None,
        state_bytes: s1,
        fold: false,
        ..
    }) = ask(
        &mut worker,
        Request::Take {
            doc: 1,
            update: u1.clone(),
        },
    )
    else {
        panic!("u1 taken");
    };
    let held = worker.docs.get(&1).expect("held");
    assert_eq!(held.since.len(), 1);
    assert_eq!(held.since_bytes, size_of(&u1));
    assert_eq!(worker.held_bytes, s1 + size_of(&u1));

    // Duplicate (no-op) update does not bloat `since` or `held_bytes`.
    let before_bytes = worker.held_bytes;
    let Ok(Answer::Taken { repair: None, .. }) = ask(
        &mut worker,
        Request::Take {
            doc: 1,
            update: u1.clone(),
        },
    ) else {
        panic!("duplicate taken as no-op");
    };
    assert_eq!(worker.docs.get(&1).expect("held").since.len(), 1);
    assert_eq!(worker.held_bytes, before_bytes);

    // Repaired update adds both update and repair bytes to `since` and `held_bytes`.
    let u2 = editor.append_element("nothing-a-body-has");
    let Ok(Answer::Taken {
        repair: Some(r2),
        state_bytes: s2,
        ..
    }) = ask(
        &mut worker,
        Request::Take {
            doc: 1,
            update: u2.clone(),
        },
    )
    else {
        panic!("u2 repaired");
    };
    editor.receive(&r2);
    let expected_since = size_of(&u1) + size_of(&u2) + size_of(&r2);
    assert_eq!(worker.docs.get(&1).expect("held").since.len(), 3);
    assert_eq!(
        worker.docs.get(&1).expect("held").since_bytes,
        expected_since
    );
    assert_eq!(worker.held_bytes, s2 + expected_since);

    let state_before_rebase = worker.docs.get(&1).expect("held").doc.state();

    // `Request::State` returns exact state, rebases checkpoint, clears `since`, and
    // reclaims `since_bytes` from `held_bytes`.
    let Ok(Answer::Bytes(got_state)) = ask(&mut worker, Request::State(1)) else {
        panic!("state");
    };
    assert_eq!(got_state, state_before_rebase);
    let held = worker.docs.get(&1).expect("held");
    assert_eq!(held.since.len(), 0);
    assert_eq!(held.since_bytes, 0);
    assert_eq!(worker.held_bytes, s2);

    // Add another update then Close: all `state_bytes + since_bytes` reclaimed to 0.
    let u3 = editor.append_paragraph("after rebase");
    assert!(ask(&mut worker, Request::Take { doc: 1, update: u3 }).is_ok());
    assert!(worker.held_bytes > s2);
    assert_eq!(ask(&mut worker, Request::Close(1)), Ok(Answer::Done));
    assert_eq!(worker.held_bytes, 0, "no leak or underflow on Close");
}

/// Pre-apply (`Untouched`) and post-apply (`Dirty`) refusals preserve the held
/// document byte-for-byte and leave `held_bytes` unchanged.
#[test]
fn refused_in_place_take_preserves_state_and_held_bytes() {
    let mut worker = Worker::new(Limits {
        update_bytes: 4096,
        state_bytes: 16 * 1024,
        body_bytes: 16 * 1024,
        documents: 4,
        held_bytes: 1 << 20,
    });
    let Ok(Answer::Opened {
        state: Some(initial_state),
        ..
    }) = ask(
        &mut worker,
        Request::Open {
            doc: 1,
            source: Source::Body(None),
            share: Share::Kept,
        },
    )
    else {
        panic!("opened");
    };
    let editor = fvoci_collab::testing::Editor::join(111, &initial_state);
    let u1 = editor.append_paragraph("hello");
    assert!(ask(&mut worker, Request::Take { doc: 1, update: u1 }).is_ok());
    let u2 = editor.append_element("nothing-a-body-has");
    let Ok(Answer::Taken {
        repair: Some(r2), ..
    }) = ask(&mut worker, Request::Take { doc: 1, update: u2 })
    else {
        panic!("u2 repaired");
    };
    editor.receive(&r2);

    let state_before = worker.docs.get(&1).expect("held").doc.state();
    let held_before = worker.held_bytes;

    // Pre-apply refusal (Untouched) preserves state and held_bytes.
    assert!(
        ask(
            &mut worker,
            Request::Take {
                doc: 1,
                update: vec![1, 2, 3],
            }
        )
        .is_err()
    );
    assert_eq!(worker.docs.get(&1).expect("held").doc.state(), state_before);
    assert_eq!(worker.held_bytes, held_before);

    // Post-apply refusal (Dirty: body_bytes exceeded) rebuilds byte-for-byte from the
    // checkpoint, rebasing onto `base` and reclaiming `since_bytes`.
    let huge = editor.append_paragraph(&"x".repeat(3000));
    worker.update_limits.body_bytes = 10;
    let refused = ask(
        &mut worker,
        Request::Take {
            doc: 1,
            update: huge,
        },
    );
    assert_eq!(
        refused.err().map(|r| r.refusal),
        Some(Refusal::BodyTooLarge)
    );
    let held = worker.docs.get(&1).expect("held");
    assert_eq!(
        held.doc.state(),
        state_before,
        "rebuilt byte-for-byte identical after Dirty refusal"
    );
    assert!(
        held.settled && held.since.is_empty() && held.last_state.is_none(),
        "settled Dirty restore rebases checkpoint and reclaims since"
    );
    assert_eq!(worker.held_bytes, held.state_bytes);
    assert!(worker.held_bytes < held_before);
}

/// `since` triggers `fold: true` at `state_bytes / 2` and hard-caps with
/// `Refusal::TooManyDocuments` at `state_bytes` (rolling back cleanly); rebasing via
/// `Request::State` lets updates resume.
#[test]
fn since_fold_signal_and_hard_cap_enforce_bounds_without_corrupting_state() {
    let mut worker = Worker::new(Limits {
        update_bytes: 4096,
        state_bytes: 1024,
        body_bytes: 4096,
        documents: 1,
        held_bytes: 1 << 20,
    });
    let Ok(Answer::Opened {
        state: Some(initial_state),
        ..
    }) = ask(
        &mut worker,
        Request::Open {
            doc: 1,
            source: Source::Body(None),
            share: Share::Kept,
        },
    )
    else {
        panic!("opened");
    };
    let editor = fvoci_collab::testing::Editor::join(102, &initial_state);
    // Repaired updates store both the client update and the repair update in `since`, so
    // `since_bytes` grows faster than `state_bytes` and crosses both `fold_since_bytes`
    // (512) and `max_since_bytes` (1024) while `state_bytes` stays below 1024.
    let mut saw_fold = false;
    let mut refused_cap = false;
    let mut last_good_state = initial_state;
    let mut last_held_bytes = worker.held_bytes;
    for _ in 0..40 {
        let update = editor.append_element("nothing-a-body-has");
        match ask(&mut worker, Request::Take { doc: 1, update }) {
            Ok(Answer::Taken { repair, fold, .. }) => {
                if let Some(repair) = repair {
                    editor.receive(&repair);
                }
                saw_fold |= fold;
                last_good_state = worker.docs.get(&1).expect("held").doc.state();
                last_held_bytes = worker.held_bytes;
            }
            Err(refused) => {
                assert_eq!(refused.refusal, Refusal::TooManyDocuments);
                let held = worker.docs.get(&1).expect("held");
                assert_eq!(
                    held.doc.state(),
                    last_good_state,
                    "state rolled back byte-for-byte on since hard cap"
                );
                assert!(held.settled && held.since.is_empty());
                assert_eq!(
                    worker.held_bytes, held.state_bytes,
                    "settled rollback reclaims since_bytes cleanly"
                );
                assert!(worker.held_bytes < last_held_bytes);
                refused_cap = true;
                break;
            }
            other => panic!("unexpected: {other:?}"),
        }
    }
    assert!(saw_fold, "fold signal fired at state_bytes / 2");
    assert!(refused_cap, "hard cap refused past state_bytes");

    // After `Request::State` rebases `since`, updates can continue.
    let Ok(Answer::Bytes(rebased)) = ask(&mut worker, Request::State(1)) else {
        panic!("state");
    };
    assert_eq!(rebased, last_good_state);
    assert_eq!(worker.docs.get(&1).expect("held").since_bytes, 0);
    let editor2 = fvoci_collab::testing::Editor::join(103, &rebased);
    let small = editor2.append_paragraph("ok");
    assert!(matches!(
        ask(
            &mut worker,
            Request::Take {
                doc: 1,
                update: small,
            }
        ),
        Ok(Answer::Taken { fold: false, .. })
    ));
}

/// Tight `held_bytes` counts `since_bytes` alongside `state_bytes`, refuses updates that
/// would push `state_bytes + since_bytes` over `held_bytes` with exact rollback, and
/// frees room once `Request::State` rebases `since`.
#[test]
fn held_bytes_budget_enforces_state_plus_since_and_rolls_back_on_overflow() {
    let mut worker = Worker::new(Limits {
        update_bytes: 4096,
        state_bytes: 16 * 1024,
        body_bytes: 16 * 1024,
        documents: 2,
        held_bytes: 350,
    });
    let Ok(Answer::Opened {
        state: Some(initial_state),
        ..
    }) = ask(
        &mut worker,
        Request::Open {
            doc: 1,
            source: Source::Body(None),
            share: Share::Kept,
        },
    )
    else {
        panic!("opened");
    };
    let editor = fvoci_collab::testing::Editor::join(201, &initial_state);
    let first = editor.append_paragraph("short");
    assert!(
        ask(
            &mut worker,
            Request::Take {
                doc: 1,
                update: first
            }
        )
        .is_ok()
    );
    let before_state = worker.docs.get(&1).expect("held").doc.state();
    let before_held = worker.held_bytes;

    // A 200-byte update fits `state_bytes` (16 KiB) alone, but `state_bytes + since_bytes`
    // exceeds `held_bytes` (350): refused with `TooManyDocuments` and rolled back cleanly.
    let overflow = editor.append_paragraph(&"z".repeat(200));
    let err = ask(
        &mut worker,
        Request::Take {
            doc: 1,
            update: overflow,
        },
    )
    .expect_err("over held_bytes");
    assert_eq!(err.refusal, Refusal::TooManyDocuments);
    let held = worker.docs.get(&1).expect("held");
    assert_eq!(
        held.doc.state(),
        before_state,
        "exact rollback on held_bytes overflow"
    );
    assert!(held.settled && held.since.is_empty());
    assert_eq!(worker.held_bytes, held.state_bytes);
    assert!(worker.held_bytes < before_held);
    assert_eq!(ask(&mut worker, Request::Close(1)), Ok(Answer::Done));
    assert_eq!(worker.held_bytes, 0);
}

/// `Retake` rebases its checkpoint instead of refusing when `since` would overflow, and
/// still enforces `held_bytes` (rolling back byte-for-byte) when `state_bytes` alone
/// exceeds `held_bytes`.
#[test]
fn retake_rebases_on_since_overflow_and_rolls_back_on_held_bytes_overflow() {
    let mut worker = Worker::new(Limits {
        update_bytes: 4096,
        state_bytes: 512,
        body_bytes: 4096,
        documents: 2,
        held_bytes: 900,
    });
    let Ok(Answer::Opened {
        state: Some(initial_state),
        ..
    }) = ask(
        &mut worker,
        Request::Open {
            doc: 1,
            source: Source::Body(None),
            share: Share::Kept,
        },
    )
    else {
        panic!("opened");
    };
    let editor = fvoci_collab::testing::Editor::join(301, &initial_state);
    // Repeated `Retake` of repaired updates crosses `max_since_bytes` (512): must rebase
    // cleanly (`since_bytes == 0`, `held_bytes == state_bytes`) rather than refusing.
    let mut rebased = false;
    for _ in 0..20 {
        let update = editor.append_element("nothing-a-body-has");
        let Ok(Answer::Taken {
            repair,
            state_bytes,
            fold,
            ..
        }) = ask(&mut worker, Request::Retake { doc: 1, update })
        else {
            panic!("retake should rebase rather than refuse on since overflow");
        };
        if let Some(repair) = repair {
            editor.receive(&repair);
        }
        let held = worker.docs.get(&1).expect("held");
        if held.since_bytes == 0 {
            assert_eq!(worker.held_bytes, state_bytes);
            assert!(!fold);
            rebased = true;
            break;
        }
    }
    assert!(rebased, "retake rebased when since crossed cap");

    // Open a second document occupying ~300 bytes of `held_bytes`, then grow doc 1 to
    // ~700 bytes: fits `logged_limits().state_bytes` (900) alone, but `base_held +
    // state_bytes > held_bytes` (900), so `Retake` refuses with `TooManyDocuments` and
    // rolls back byte-for-byte.
    let body2 = serde_json::to_vec(&serde_json::json!({
        "type": "doc",
        "content": [{"type": "paragraph", "content": [{"type": "text", "text": "p".repeat(250)}]}]
    }))
    .expect("json");
    assert!(
        ask(
            &mut worker,
            Request::Open {
                doc: 2,
                source: Source::Body(Some(body2)),
                share: Share::Kept,
            }
        )
        .is_ok()
    );
    let before_state = worker.docs.get(&1).expect("held").doc.state();
    let before_held = worker.held_bytes;
    let too_big = editor.append_paragraph(&"m".repeat(450));
    let err = ask(
        &mut worker,
        Request::Retake {
            doc: 1,
            update: too_big,
        },
    )
    .expect_err("base_held + state_bytes over held_bytes");
    assert_eq!(err.refusal, Refusal::TooManyDocuments);
    assert_eq!(worker.docs.get(&1).expect("held").doc.state(), before_state);
    assert_eq!(worker.held_bytes, before_held);
}

/// Builds a two-client update that introduces 1 reading step so `reading_steps: Some(0)`
/// triggers `remake: true`.
fn one_step_update() -> Vec<u8> {
    // Client 1 types two empty paragraphs; client 2 inserts an empty paragraph with
    // right origin `(1, 1)` and no left origin, taking 1 reading step.
    let mut out = Vec::new();
    // 2 runs
    out.push(2);
    // Run 1: count=2, client=1, clock=0
    out.extend_from_slice(&[2, 1, 0]);
    out.extend_from_slice(b"\x07\x01\x07default\x03\x09paragraph");
    out.extend_from_slice(b"\x87\x01\x00\x03\x09paragraph");
    // Run 2: count=1, client=2, clock=0; right=(1, 1) (0x47)
    out.extend_from_slice(&[1, 2, 0]);
    out.extend_from_slice(b"\x47\x01\x01\x03\x09paragraph");
    // 0 deletions
    out.push(0);
    out
}

/// `commit_remake` refuses and rolls back byte-for-byte when `base_held + state_bytes`
/// exceeds `held_bytes`, and once room exists it commits `remake: true`, drops `since`,
/// and reclaims `since_bytes` from `held_bytes`.
#[test]
fn remake_take_reclaims_since_bytes_and_rolls_back_on_held_bytes_overflow() {
    let mut worker = Worker::new(Limits {
        update_bytes: 4096,
        state_bytes: 16 * 1024,
        body_bytes: 16 * 1024,
        documents: 1,
        held_bytes: 16 * 1024,
    });
    let Ok(Answer::Opened {
        state: Some(initial_state),
        ..
    }) = ask(
        &mut worker,
        Request::Open {
            doc: 1,
            source: Source::Body(None),
            share: Share::Kept,
        },
    )
    else {
        panic!("opened");
    };
    let editor = fvoci_collab::testing::Editor::join(401, &initial_state);
    let u1 = editor.append_paragraph("before remake");
    assert!(ask(&mut worker, Request::Take { doc: 1, update: u1 }).is_ok());
    assert!(worker.docs.get(&1).expect("held").since_bytes > 0);
    let before_state = worker.docs.get(&1).expect("held").doc.state();
    let before_held = worker.held_bytes;

    // Force `remake: true` via `reading_steps: Some(0)`, with `held_bytes` tight enough to
    // hold `before_state` after rollback rebase while refusing `step_update`'s larger state.
    worker.update_limits.reading_steps = Some(0);
    worker.limits.held_bytes = worker.docs[&1].state_bytes;
    let step_update = one_step_update();
    let err = ask(
        &mut worker,
        Request::Take {
            doc: 1,
            update: step_update.clone(),
        },
    )
    .expect_err("remake over held_bytes");
    assert_eq!(err.refusal, Refusal::TooManyDocuments);
    let held = worker.docs.get(&1).expect("held");
    assert!(!held.must_remake, "must_remake stays false on rollback");
    assert_eq!(held.doc.state(), before_state, "exact rollback");
    assert_eq!(worker.held_bytes, held.state_bytes);
    assert!(worker.held_bytes < before_held);

    // With room for `state_bytes`, `remake: true` commits and reclaims `since_bytes`.
    worker.limits.held_bytes = 16 * 1024;
    let Ok(Answer::Taken {
        remake: true,
        state_bytes,
        fold: false,
        ..
    }) = ask(
        &mut worker,
        Request::Take {
            doc: 1,
            update: step_update,
        },
    )
    else {
        panic!("remake committed");
    };
    let held = worker.docs.get(&1).expect("held");
    assert!(held.must_remake);
    assert_eq!(held.since_bytes, 0);
    assert_eq!(worker.held_bytes, state_bytes);
    assert_eq!(ask(&mut worker, Request::Close(1)), Ok(Answer::Done));
    assert_eq!(worker.held_bytes, 0);
}

/// If restoring the checkpoint after a `Dirty` refusal ever fails, `Worker::answer`
/// returns a fatal `FrameError` (closing the worker so the server reopens from store)
/// instead of dropping the document silently and wedging the card on `UnknownDocument`.
#[test]
fn corrupted_checkpoint_fails_worker_closed_instead_of_wedging_doc() {
    let mut worker = Worker::new(LIMITS);
    let Ok(Answer::Opened {
        state: Some(initial_state),
        ..
    }) = ask(
        &mut worker,
        Request::Open {
            doc: 1,
            source: Source::Body(None),
            share: Share::Kept,
        },
    )
    else {
        panic!("opened");
    };
    let editor = fvoci_collab::testing::Editor::join(501, &initial_state);
    // Corrupt checkpoint bytes so `restore_after_dirty` cannot rebuild after a Dirty refusal.
    worker.docs.get_mut(&1).expect("held").base = vec![0xff, 0xff, 0xff];
    worker.update_limits.body_bytes = 5;
    let huge = editor.append_paragraph("triggers dirty body_bytes refusal");
    let fatal = worker.answer(Request::Take {
        doc: 1,
        update: huge,
    });
    assert!(
        matches!(fatal, Err(FrameError::Io(_))),
        "expected fatal FrameError::Io, got {fatal:?}"
    );
    assert_eq!(worker.held_bytes, 0, "accounted bytes released");
    assert_eq!(worker.docs.len(), 0, "document removed before exit");
}

/// `Request::State` settles an unsettled held document (`settled: false -> true`),
/// rebases `since`, and fast-paths subsequent `Request::State` calls once settled.
#[test]
fn state_settles_unsettled_doc_and_fast_paths_settled_doc() {
    let mut worker = Worker::new(Limits {
        update_bytes: 4096,
        state_bytes: 16 * 1024,
        body_bytes: 16 * 1024,
        documents: 2,
        held_bytes: 1 << 20,
    });
    let Ok(Answer::Opened {
        state: Some(initial_state),
        ..
    }) = ask(
        &mut worker,
        Request::Open {
            doc: 1,
            source: Source::Body(None),
            share: Share::Kept,
        },
    )
    else {
        panic!("opened");
    };
    assert!(
        worker.docs.get(&1).expect("held").settled,
        "settled on open"
    );

    let editor = fvoci_collab::testing::Editor::join(601, &initial_state);
    let u1 = editor.append_paragraph("hello settle");
    assert!(ask(&mut worker, Request::Take { doc: 1, update: u1 }).is_ok());
    assert!(
        !worker.docs.get(&1).expect("held").settled,
        "mutating Take clears settled"
    );

    let Ok(Answer::Bytes(s1)) = ask(&mut worker, Request::State(1)) else {
        panic!("settled state");
    };
    let held = worker.docs.get(&1).expect("held");
    assert!(held.settled, "Request::State marks doc settled");
    assert_eq!(held.since_bytes, 0);
    assert_eq!(worker.held_bytes, size_of(&s1));

    // Second Request::State takes the settled fast path and returns the same state.
    let Ok(Answer::Bytes(s2)) = ask(&mut worker, Request::State(1)) else {
        panic!("fast-path state");
    };
    assert_eq!(s2, s1);
    assert!(worker.docs.get(&1).expect("held").settled);
}

/// When settling fails during `Request::State` (either via real reading-steps overflow or
/// `Request::FailNextSettle`), the worker returns `Answer::Remake`, reclaims `since_bytes`,
/// keeps `held.doc` intact for `Request::Body`, and blocks mutating/state requests until
/// `Close`.
#[test]
fn state_settle_failure_returns_remake_preserves_body_and_reclaims_since_bytes() {
    let mut worker = Worker::new(Limits {
        update_bytes: 4096,
        state_bytes: 16 * 1024,
        body_bytes: 16 * 1024,
        documents: 2,
        held_bytes: 1 << 20,
    });

    // Case 1: Real settle failure via `reading_steps: Some(0)` on a 1-step document.
    assert!(matches!(
        ask(
            &mut worker,
            Request::Open {
                doc: 1,
                source: Source::Body(None),
                share: Share::Kept,
            }
        ),
        Ok(Answer::Opened { .. })
    ));
    let step_update = one_step_update();
    assert!(
        ask(
            &mut worker,
            Request::Take {
                doc: 1,
                update: step_update,
            }
        )
        .is_ok()
    );
    let expected_body = ask(&mut worker, Request::Body(1)).expect("body before settle");
    let state_bytes_before = worker.docs.get(&1).expect("held").state_bytes;
    assert!(worker.docs.get(&1).expect("held").since_bytes > 0);

    worker.update_limits.reading_steps = Some(0);
    assert_eq!(
        ask(&mut worker, Request::State(1)),
        Ok(Answer::Remake),
        "real settle failure returns Answer::Remake"
    );
    let held = worker.docs.get(&1).expect("held");
    assert!(held.must_remake, "marked for remake");
    assert_eq!(held.since_bytes, 0, "since_bytes reclaimed");
    assert_eq!(worker.held_bytes, state_bytes_before);
    assert_eq!(
        ask(&mut worker, Request::Body(1)).expect("body preserved"),
        expected_body,
        "live doc body preserved without loss"
    );
    assert_eq!(
        ask(&mut worker, Request::State(1)).err().map(|r| r.refusal),
        Some(Refusal::OutOfOrder),
        "State blocked after must_remake"
    );
    assert_eq!(ask(&mut worker, Request::Close(1)), Ok(Answer::Done));
    assert_eq!(worker.held_bytes, 0);

    // Case 2: Forced settle failure via `Request::FailNextSettle` (test-util hook).
    #[cfg(feature = "test-util")]
    {
        worker.update_limits.reading_steps = Some(READING_STEPS);
        let Ok(Answer::Opened {
            state: Some(initial),
            ..
        }) = ask(
            &mut worker,
            Request::Open {
                doc: 2,
                source: Source::Body(None),
                share: Share::Kept,
            },
        )
        else {
            panic!("opened doc 2");
        };
        let editor = fvoci_collab::testing::Editor::join(602, &initial);
        let u1 = editor.append_paragraph("never lost");
        assert!(ask(&mut worker, Request::Take { doc: 2, update: u1 }).is_ok());
        let expected_body2 = ask(&mut worker, Request::Body(2)).expect("body 2");
        let state_bytes_2 = worker.docs.get(&2).expect("held").state_bytes;

        assert_eq!(
            ask(&mut worker, Request::FailNextSettle(2)),
            Ok(Answer::Done)
        );
        assert_eq!(ask(&mut worker, Request::State(2)), Ok(Answer::Remake));
        assert_eq!(worker.held_bytes, state_bytes_2);
        assert_eq!(
            ask(&mut worker, Request::Body(2)).expect("body 2 preserved"),
            expected_body2
        );
        assert_eq!(ask(&mut worker, Request::Close(2)), Ok(Answer::Done));
        assert_eq!(worker.held_bytes, 0);
    }
}

/// A `Dirty` refusal after an unsettled non-canonical update (two adjacent text blocks
/// that `yrs` squashes on 1st read-back) settles cleanly in O(1) without killing the
/// worker or replaying `since`, and preserves the exact body and `held_bytes`.
#[test]
fn dirty_refusal_restores_non_canonical_unsettled_state_in_o1() {
    let mut worker = Worker::new(Limits {
        update_bytes: 4096,
        state_bytes: 16 * 1024,
        body_bytes: 16 * 1024,
        documents: 1,
        held_bytes: 1 << 20,
    });
    assert!(matches!(
        ask(
            &mut worker,
            Request::Open {
                doc: 1,
                source: Source::Body(None),
                share: Share::Kept,
            }
        ),
        Ok(Answer::Opened { .. })
    ));

    // Two separate adjacent text items of client 7: `take_in_place` skips settle re-read
    // so `last_state` stays non-canonical until read-back squashes `"ab"` + `"cd"`.
    let mut non_canonical = Vec::new();
    // 1 run, 4 changes, client 7, clock 0
    non_canonical.extend_from_slice(&[1, 4, 7, 0]);
    // (7, 0) paragraph under root "default"
    non_canonical.extend_from_slice(b"\x07\x01\x07default\x03\x09paragraph");
    // (7, 1) xml_text under (7, 0)
    non_canonical.extend_from_slice(&[0x07, 0, 7, 0, 6]);
    // (7, 2..4) "ab" under (7, 1)
    non_canonical.extend_from_slice(&[0x04, 0, 7, 1, 2, b'a', b'b']);
    // (7, 4..6) "cd" after (7, 3)
    non_canonical.extend_from_slice(&[0x84, 7, 3, 2, b'c', b'd']);
    // 0 deletions
    non_canonical.push(0);

    assert!(
        ask(
            &mut worker,
            Request::Take {
                doc: 1,
                update: non_canonical,
            }
        )
        .is_ok()
    );
    let expected_body = ask(&mut worker, Request::Body(1)).expect("body");
    let current_state = worker.docs.get(&1).expect("held").doc.state();
    let editor = fvoci_collab::testing::Editor::join(701, &current_state);

    // Trigger a Dirty refusal (`body_bytes` tight): worker settles the non-canonical
    // `last_state` in O(1) without dying or losing `"abcd"`.
    worker.update_limits.body_bytes = 5;
    let rejected = editor.append_paragraph("rejected update");
    let err = ask(
        &mut worker,
        Request::Take {
            doc: 1,
            update: rejected,
        },
    )
    .expect_err("body_bytes refused");
    assert_eq!(err.refusal, Refusal::BodyTooLarge);
    assert_eq!(
        ask(&mut worker, Request::Body(1)).expect("body preserved"),
        expected_body
    );
    let held = worker.docs.get(&1).expect("held");
    assert_eq!(worker.held_bytes, held.accounted_bytes());
    assert_eq!(ask(&mut worker, Request::Close(1)), Ok(Answer::Done));
    assert_eq!(worker.held_bytes, 0);
}

/// Verifies lazy snapshot cadence (`SNAPSHOT_SINCE_GAP`) and bounded rollback: live fast-path
/// takes defer `last_state` encoding until `SNAPSHOT_SINCE_GAP` entries accumulate, a first
/// `Dirty` refusal replays only the short tail and caches `last_state` (`snapshot_since_start ==
/// since.len()`), a second `Dirty` refusal restores in O(1), and `Request::State` encodes on
/// demand and settles cleanly.
#[test]
fn lazy_snapshot_and_repeated_dirty_refusals_preserve_exact_state_and_held_bytes() {
    let mut worker = Worker::new(Limits {
        update_bytes: 4096,
        state_bytes: 64 * 1024,
        body_bytes: 64 * 1024,
        documents: 1,
        held_bytes: 1 << 20,
    });
    let Ok(Answer::Opened {
        state: Some(initial),
        ..
    }) = ask(
        &mut worker,
        Request::Open {
            doc: 1,
            source: Source::Body(None),
            share: Share::Kept,
        },
    )
    else {
        panic!("opened");
    };
    let editor = fvoci_collab::testing::Editor::join(801, &initial);
    for i in 0..5_usize {
        let u = editor.append_paragraph(&format!("lazy-{i}"));
        assert!(ask(&mut worker, Request::Take { doc: 1, update: u }).is_ok());
    }
    let held = worker.docs.get(&1).expect("held");
    assert_eq!((held.since.len(), held.snapshot_since_start), (5, 0));
    assert!(held.last_state.is_none());
    let expected_state = held.doc.state();
    let expected_held_bytes = worker.held_bytes;

    // Two consecutive Dirty refusals: 1st replays only the 5-entry tail and rebases onto
    // `base: settled_state` (`since` cleared, `settled == true`); 2nd restores from `base`
    // in O(1).
    let saved_body_limit = worker.update_limits.body_bytes;
    worker.update_limits.body_bytes = 5;
    for attempt in 0..2 {
        let rejected = fvoci_collab::testing::Editor::join(810 + attempt, &expected_state)
            .append_paragraph("rejected");
        let err = ask(
            &mut worker,
            Request::Take {
                doc: 1,
                update: rejected,
            },
        )
        .expect_err("refused");
        assert_eq!(err.refusal, Refusal::BodyTooLarge);
        let held = worker.docs.get(&1).expect("held");
        assert_eq!(held.doc.state(), expected_state);
        assert!(held.settled && held.since.is_empty() && held.last_state.is_none());
        assert_eq!(held.base, expected_state);
        assert_eq!(worker.held_bytes, held.state_bytes);
    }
    assert!(worker.held_bytes < expected_held_bytes);
    worker.update_limits.body_bytes = saved_body_limit;

    // Apply `SNAPSHOT_SINCE_GAP` more fast edits: lazy snapshot defers and advances automatically.
    let mut peer_state = expected_state;
    let (mut saw_deferred, mut saw_advanced) = (false, false);
    for i in 0..checkpoint::SNAPSHOT_SINCE_GAP {
        let client = 820 + u64::try_from(i).expect("small index");
        let u = fvoci_collab::testing::Editor::join(client, &peer_state)
            .append_paragraph(&format!("gap-{i}"));
        assert!(ask(&mut worker, Request::Take { doc: 1, update: u }).is_ok());
        let held = worker.docs.get(&1).expect("held");
        let gap = held.since.len() - held.snapshot_since_start;
        assert!(gap < checkpoint::SNAPSHOT_SINCE_GAP);
        saw_deferred |= gap > 0;
        saw_advanced |= held.snapshot_since_start > 5;
        peer_state = held.doc.state();
    }
    assert!(saw_deferred && saw_advanced);

    // One more edit leaves an unsnapshotted tail; `Request::State` encodes on demand and settles.
    let tail_u = fvoci_collab::testing::Editor::join(999, &peer_state).append_paragraph("tail");
    assert!(
        ask(
            &mut worker,
            Request::Take {
                doc: 1,
                update: tail_u
            }
        )
        .is_ok()
    );
    let pre_state = worker.docs.get(&1).expect("held").doc.state();
    let held = worker.docs.get(&1).expect("held");
    assert!(held.snapshot_since_start < held.since.len());
    let Ok(Answer::Bytes(settled_state)) = ask(&mut worker, Request::State(1)) else {
        panic!("state settled");
    };
    assert_eq!(settled_state, pre_state);
    let held = worker.docs.get(&1).expect("held");
    assert!(held.settled && held.since.is_empty() && held.snapshot_since_start == 0);
    assert_eq!(worker.held_bytes, held.state_bytes);
    assert_eq!(ask(&mut worker, Request::Close(1)), Ok(Answer::Done));
    assert_eq!(worker.held_bytes, 0);
}

/// Opening a `Source::Body` with rich marked content populates exact counted state via
/// `BodyDoc::from_body_counted`, serves the first `Take` on the prefilled incremental
/// cache without re-encoding, and rejects bodies whose encoded state exceeds `state_bytes`.
#[test]
fn open_source_body_returns_counted_state_and_takes_immediately() {
    let body_json = serde_json::to_vec(&serde_json::json!({
        "type": "doc",
        "content": [
            {"type": "paragraph", "content": [
                {"type": "text", "text": "hello ", "marks": [{"type": "bold"}]},
                {"type": "text", "text": "world", "marks": [
                    {"type": "link", "attrs": {"href": "https://example.com"}}
                ]}
            ]},
            {"type": "image", "attrs": {"fileId": "file-1", "alt": "pic"}}
        ]
    }))
    .expect("json");

    let mut worker = Worker::new(Limits {
        update_bytes: 4096,
        state_bytes: 64 * 1024,
        body_bytes: 64 * 1024,
        documents: 2,
        held_bytes: 128 * 1024,
    });
    let Ok(Answer::Opened {
        state: Some(state),
        state_bytes,
        clients,
        changes,
        client_changes,
        steps_ahead,
    }) = ask(
        &mut worker,
        Request::Open {
            doc: 1,
            source: Source::Body(Some(body_json.clone())),
            share: Share::Kept,
        },
    )
    else {
        panic!("expected Opened with initial state");
    };
    assert_eq!(state_bytes, u64::try_from(state.len()).expect("len"));
    assert_eq!(clients, 1);
    let expected = fvoci_collab::state_counts(&state).expect("counts");
    assert_eq!(changes, expected.changes);
    assert_eq!(client_changes, expected.client_changes);
    assert_eq!(steps_ahead, expected.steps_ahead);
    assert_eq!(worker.held_bytes, state_bytes);

    let editor = fvoci_collab::testing::Editor::join(901, &state);
    let update = editor.append_paragraph("first live edit");
    let Ok(Answer::Taken { repair, remake, .. }) =
        ask(&mut worker, Request::Take { doc: 1, update })
    else {
        panic!("first take succeeded");
    };
    assert!(repair.is_none());
    assert!(!remake);
    let held = worker.docs.get(&1).expect("held");
    assert_eq!(held.doc.file_ids(), vec!["file-1".to_owned()]);
    assert!(
        held.last_state.is_none(),
        "first edit used prefilled cache without full state re-encode"
    );

    // Tiny `state_bytes` limit rejects an oversized `Source::Body` before holding it.
    let mut tight = Worker::new(Limits {
        update_bytes: 4096,
        state_bytes: 8,
        body_bytes: 64 * 1024,
        documents: 2,
        held_bytes: 128 * 1024,
    });
    let err = ask(
        &mut tight,
        Request::Open {
            doc: 2,
            source: Source::Body(Some(body_json)),
            share: Share::Kept,
        },
    )
    .expect_err("oversized body state refused");
    assert_eq!(err.refusal, Refusal::DocumentTooLarge);
    assert_eq!(tight.held_bytes, 0);

    // An unvalidated stored body with >53-bit integer attrs returns a clean refusal instead
    // of panicking inside the worker.
    let over_53 = (1_i64 << 53) + 1;
    let bad_attr_json = serde_json::to_vec(&serde_json::json!({
        "type": "doc",
        "content": [{"type": "heading", "attrs": {"level": over_53}}]
    }))
    .expect("json");
    let err = ask(
        &mut worker,
        Request::Open {
            doc: 3,
            source: Source::Body(Some(bad_attr_json)),
            share: Share::Kept,
        },
    )
    .expect_err("unreadable stored body refused");
    assert_eq!(err.refusal, Refusal::Unreadable);
}

/// A table update past `TABLE_MAX_ROWS` (201 rows) or `TABLE_MAX_COLUMNS` (21 columns) is
/// refused with `Refusal::TableTooLarge` (not `Refusal::BodyTooLarge`), rolls back cleanly
/// with `held_bytes` intact, and lets subsequent valid edits succeed.
#[test]
fn oversized_table_update_returns_table_too_large_and_preserves_document() {
    let mut worker = Worker::new(Limits {
        update_bytes: 64 * 1024,
        state_bytes: 2 * 1024 * 1024,
        body_bytes: 2 * 1024 * 1024,
        documents: 2,
        held_bytes: 8 * 1024 * 1024,
    });
    let cell = serde_json::json!({"type": "tableCell", "content": [{"type": "paragraph"}]});
    let row_1 = serde_json::json!({"type": "tableRow", "content": [&cell]});
    let max_rows_body = serde_json::to_vec(&serde_json::json!({
        "type": "doc",
        "content": [{"type": "table", "content": vec![row_1; usize::from(fvoci_model::TABLE_MAX_ROWS)]}],
    }))
    .expect("JSON");
    let row_20 = serde_json::json!({
        "type": "tableRow",
        "content": vec![cell; usize::from(fvoci_model::TABLE_MAX_COLUMNS)],
    });
    let max_cols_body = serde_json::to_vec(&serde_json::json!({
        "type": "doc",
        "content": [{"type": "table", "content": [&row_20, &row_20]}],
    }))
    .expect("JSON");

    for (doc_id, body, add_row) in [(1_u64, max_rows_body, true), (2_u64, max_cols_body, false)] {
        let Ok(Answer::Opened {
            state: Some(initial),
            ..
        }) = ask(
            &mut worker,
            Request::Open {
                doc: doc_id,
                source: Source::Body(Some(body)),
                share: Share::Kept,
            },
        )
        else {
            panic!("opened doc {doc_id}");
        };
        let expected_body = ask(&mut worker, Request::Body(doc_id)).expect("body before");
        let expected_held = worker.held_bytes;
        let editor = fvoci_collab::testing::Editor::join(900 + doc_id, &initial);
        let rejected = if add_row {
            editor.add_table_row(1)
        } else {
            editor.add_table_column()
        };
        let err = ask(
            &mut worker,
            Request::Take {
                doc: doc_id,
                update: rejected,
            },
        )
        .expect_err("oversized table refused");
        assert_eq!(err.refusal, Refusal::TableTooLarge, "doc {doc_id}");
        assert_eq!(
            ask(&mut worker, Request::Body(doc_id)).expect("body preserved"),
            expected_body
        );
        assert_eq!(worker.held_bytes, expected_held);

        let clean = fvoci_collab::testing::Editor::join(910 + doc_id, &initial)
            .append_paragraph("after refusal");
        assert!(
            ask(
                &mut worker,
                Request::Take {
                    doc: doc_id,
                    update: clean,
                }
            )
            .is_ok(),
            "valid update after TableTooLarge succeeds"
        );
    }
    assert_eq!(ask(&mut worker, Request::Close(1)), Ok(Answer::Done));
    assert_eq!(ask(&mut worker, Request::Close(2)), Ok(Answer::Done));
    assert_eq!(worker.held_bytes, 0);
}

/// Whole-document `total_changes` cap is enforced consistently across `Source::Body`
/// (`DocumentTooLarge`) and `Source::State` (`Unappliable` so the server remakes cleanly).
#[test]
fn total_changes_limit_refuses_source_body_and_source_state() {
    let mut worker = Worker::new(Limits {
        update_bytes: 64 * 1024,
        state_bytes: 2 * 1024 * 1024,
        body_bytes: 2 * 1024 * 1024,
        documents: 4,
        held_bytes: 8 * 1024 * 1024,
    });
    assert_eq!(
        worker.update_limits.total_changes,
        Some(MAX_TOTAL_CHANGES),
        "default worker enables MAX_TOTAL_CHANGES"
    );
    let two_paras = serde_json::to_vec(&serde_json::json!({
        "type": "doc",
        "content": [
            {"type": "paragraph", "content": [{"type": "text", "text": "one"}]},
            {"type": "paragraph", "content": [{"type": "text", "text": "two"}]},
        ],
    }))
    .expect("json");
    let baseline = BodyDoc::from_body_counted(Some(&Body::from_stored(
        serde_json::from_slice(&two_paras).expect("value"),
    )))
    .expect("baseline counts");
    worker.update_limits.total_changes = Some(baseline.counts.changes - 1);

    let body_err = ask(
        &mut worker,
        Request::Open {
            doc: 1,
            source: Source::Body(Some(two_paras)),
            share: Share::Kept,
        },
    )
    .expect_err("Source::Body over total_changes refused");
    assert_eq!(body_err.refusal, Refusal::DocumentTooLarge);
    assert_eq!(worker.held_bytes, 0);

    let state_err = ask(
        &mut worker,
        Request::Open {
            doc: 2,
            source: Source::State(baseline.state),
            share: Share::Kept,
        },
    )
    .expect_err("Source::State over total_changes refused");
    assert_eq!(state_err.refusal, Refusal::Unappliable);
    assert_eq!(worker.held_bytes, 0);
}

/// Live `Request::Take` accepts up to `total_changes`, refuses past it with
/// `DocumentTooLarge`, and rolls back cleanly preserving body and `held_bytes`.
#[test]
fn total_changes_limit_refuses_live_take_and_preserves_document() {
    let mut worker = Worker::new(Limits {
        update_bytes: 64 * 1024,
        state_bytes: 2 * 1024 * 1024,
        body_bytes: 2 * 1024 * 1024,
        documents: 4,
        held_bytes: 8 * 1024 * 1024,
    });
    let two_paras = serde_json::to_vec(&serde_json::json!({
        "type": "doc",
        "content": [
            {"type": "paragraph", "content": [{"type": "text", "text": "one"}]},
            {"type": "paragraph", "content": [{"type": "text", "text": "two"}]},
        ],
    }))
    .expect("json");
    let baseline = BodyDoc::from_body_counted(Some(&Body::from_stored(
        serde_json::from_slice(&two_paras).expect("value"),
    )))
    .expect("baseline counts");
    let no_total = UpdateLimits {
        total_changes: None,
        ..worker.settle_limits()
    };
    let probe_update =
        fvoci_collab::testing::Editor::join(901, &baseline.state).append_paragraph("added");
    let probe_after = fvoci_collab::take_update(
        &fvoci_collab::from_state(&baseline.state, &no_total)
            .expect("open")
            .doc,
        &probe_update,
        &no_total,
    )
    .expect("probe");
    worker.update_limits.total_changes = Some(probe_after.changes);

    let Ok(Answer::Opened {
        state: Some(initial),
        changes: opened_changes,
        ..
    }) = ask(
        &mut worker,
        Request::Open {
            doc: 3,
            source: Source::Body(Some(two_paras)),
            share: Share::Kept,
        },
    )
    else {
        panic!("opened doc 3");
    };
    assert_eq!(opened_changes, baseline.counts.changes);

    let ok_update = fvoci_collab::testing::Editor::join(901, &initial).append_paragraph("fits");
    let Ok(Answer::Taken {
        changes: after_first,
        ..
    }) = ask(
        &mut worker,
        Request::Take {
            doc: 3,
            update: ok_update.clone(),
        },
    )
    else {
        panic!("update reaching exact total_changes succeeds");
    };
    assert_eq!(after_first, probe_after.changes);

    // Keep doc 3 unsettled (with non-empty `since`) so the refused `Take` exercises
    // `install_settled_restore` (`restore_limits.total_changes == None`).
    assert!(
        !worker.docs[&3].settled && !worker.docs[&3].since.is_empty(),
        "doc 3 is unsettled before rollback"
    );
    let body_at_cap = ask(&mut worker, Request::Body(3)).expect("body at cap");
    let ed2 = fvoci_collab::testing::Editor::join(902, &initial);
    ed2.receive(&ok_update);
    let over_update = ed2.append_paragraph("overflows");
    let take_err = ask(
        &mut worker,
        Request::Take {
            doc: 3,
            update: over_update,
        },
    )
    .expect_err("update past total_changes refused");
    assert_eq!(take_err.refusal, Refusal::DocumentTooLarge);
    assert_eq!(
        ask(&mut worker, Request::Body(3)).expect("body preserved"),
        body_at_cap
    );
    assert_eq!(ask(&mut worker, Request::Close(3)), Ok(Answer::Done));
    assert_eq!(worker.held_bytes, 0);
}

/// Logged updates replayed via `Request::Retake` are not discarded when crossing a
/// lowered `total_changes` cap, and `Request::State` transitions such a document cleanly
/// to `Answer::Remake` while keeping `Request::Body` intact for lossless projection.
#[test]
fn retake_and_state_settle_preserve_logged_updates_past_total_changes() {
    let mut worker = Worker::new(Limits {
        update_bytes: 64 * 1024,
        state_bytes: 2 * 1024 * 1024,
        body_bytes: 2 * 1024 * 1024,
        documents: 4,
        held_bytes: 8 * 1024 * 1024,
    });
    let body = serde_json::to_vec(&serde_json::json!({
        "type": "doc",
        "content": [
            {"type": "paragraph", "content": [{"type": "text", "text": "one"}]}
        ],
    }))
    .expect("json");
    let baseline = BodyDoc::from_body_counted(Some(&Body::from_stored(
        serde_json::from_slice(&body).expect("value"),
    )))
    .expect("baseline counts");
    worker.update_limits.total_changes = Some(baseline.counts.changes);

    let Ok(Answer::Opened { .. }) = ask(
        &mut worker,
        Request::Open {
            doc: 1,
            source: Source::State(baseline.state.clone()),
            share: Share::Kept,
        },
    ) else {
        panic!("stored state at cap opens");
    };
    let logged = fvoci_collab::testing::Editor::join(905, &baseline.state)
        .append_paragraph("logged after cap");
    let Ok(Answer::Taken {
        changes: after_retake,
        ..
    }) = ask(
        &mut worker,
        Request::Retake {
            doc: 1,
            update: logged,
        },
    )
    else {
        panic!("Retake accepts logged update past total_changes");
    };
    assert!(after_retake > baseline.counts.changes);

    // Settling via `Request::State` hits `settle_limits().total_changes` and asks for
    // lossless `Answer::Remake` while keeping `Request::Body` readable.
    assert_eq!(ask(&mut worker, Request::State(1)), Ok(Answer::Remake));
    let Ok(Answer::Body(Some(projected))) = ask(&mut worker, Request::Body(1)) else {
        panic!("body remains readable for lossless remake");
    };
    let projected_text = String::from_utf8(projected).expect("utf8");
    assert!(
        projected_text.contains("logged after cap"),
        "replayed update preserved in body: {projected_text}"
    );
    assert_eq!(ask(&mut worker, Request::Close(1)), Ok(Answer::Done));
    assert_eq!(worker.held_bytes, 0);
}

/// Verifies `Held::base` is released (`capacity == 0`) as soon as `last_state` is taken,
/// and O(1) rollback + re-snapshotting work with an empty `base`.
#[test]
fn snapshot_releases_base_and_rollback_works_without_base() {
    let mut worker = Worker::new(Limits {
        update_bytes: 16 * 1024,
        state_bytes: 8 * 1024 * 1024,
        body_bytes: 8 * 1024 * 1024,
        documents: 2,
        held_bytes: 16 * 1024 * 1024,
    });
    let initial_body = serde_json::to_vec(&serde_json::json!({
        "type": "doc",
        "content": [{"type": "paragraph", "content": [{"type": "text", "text": "seed-base"}]}],
    }))
    .expect("json");
    let Ok(Answer::Opened {
        state: Some(mut state),
        ..
    }) = ask(
        &mut worker,
        Request::Open {
            doc: 1,
            source: Source::Body(Some(initial_body)),
            share: Share::Kept,
        },
    )
    else {
        panic!("opened");
    };
    assert_ne!(worker.docs[&1].base, Vec::<u8>::new());
    assert!(worker.docs[&1].last_state.is_none());

    // 1. Advance edits until `take_snapshot` populates `last_state` and frees `Held::base`
    //    (`capacity() == 0`) with an empty tail (`snapshot_since_start == since.len()`).
    for i in 0..checkpoint::SNAPSHOT_SINCE_GAP {
        let client = 1_100 + u64::try_from(i).expect("idx");
        let u = fvoci_collab::testing::Editor::join(client, &state)
            .append_paragraph(&format!("snap1-{i}"));
        assert!(ask(&mut worker, Request::Take { doc: 1, update: u }).is_ok());
        state = worker.docs[&1].doc.state();
        if worker.docs[&1].last_state.is_some() {
            break;
        }
    }
    let held = &worker.docs[&1];
    assert!(
        held.base.is_empty() && held.base.capacity() == 0,
        "base freed once last_state is populated"
    );
    assert!(held.last_state.is_some());
    assert_eq!(held.snapshot_since_start, held.since.len());

    // 2. O(1) rollback (`tail.is_empty()`) when `base` is empty preserves `last_state` and
    //    keeps `base` empty.
    let saved_body = worker.update_limits.body_bytes;
    worker.update_limits.body_bytes = 5;
    let refused = fvoci_collab::testing::Editor::join(1_300, &state).append_paragraph("refused");
    assert_eq!(
        ask(
            &mut worker,
            Request::Take {
                doc: 1,
                update: refused,
            }
        )
        .err()
        .map(|r| r.refusal),
        Some(Refusal::BodyTooLarge)
    );
    worker.update_limits.body_bytes = saved_body;
    let held = &worker.docs[&1];
    assert!(held.base.is_empty() && held.base.capacity() == 0 && held.last_state.is_some());
    assert_eq!(held.doc.state(), state);
    assert_eq!(ask(&mut worker, Request::Close(1)), Ok(Answer::Done));
    assert_eq!(worker.held_bytes, 0);
}

/// Equal-size `Dirty` restores after re-snapshotting (`new_state_bytes == checkpoint.state_bytes`
/// with non-empty tail and empty `base`) always rebase cleanly (`base: settled_state,
/// since: empty, last_state: None, settled: true`).
#[test]
fn equal_size_dirty_restore_always_rebases_onto_settled_state() {
    let mut worker = Worker::new(Limits {
        update_bytes: 16 * 1024,
        state_bytes: 8 * 1024 * 1024,
        body_bytes: 8 * 1024 * 1024,
        documents: 1,
        held_bytes: 16 * 1024 * 1024,
    });
    let Ok(Answer::Opened {
        state: Some(mut state),
        ..
    }) = ask(
        &mut worker,
        Request::Open {
            doc: 1,
            source: Source::Body(None),
            share: Share::Kept,
        },
    )
    else {
        panic!("opened");
    };
    let editor = fvoci_collab::testing::Editor::join(1_400, &state);
    for i in 0..=(2 * checkpoint::SNAPSHOT_SINCE_GAP) {
        let u = editor.append_paragraph(&format!("snap-{i}"));
        assert!(ask(&mut worker, Request::Take { doc: 1, update: u }).is_ok());
    }
    state = worker.docs[&1].doc.state();
    let held = &worker.docs[&1];
    assert!(held.base.is_empty() && held.last_state.is_some());
    assert!(
        held.since.len() > held.snapshot_since_start,
        "non-empty tail after re-snapshot"
    );
    let expected_state_bytes = held.state_bytes;

    let saved_body = worker.update_limits.body_bytes;
    worker.update_limits.body_bytes = 5;
    let refused_tail =
        fvoci_collab::testing::Editor::join(1_900, &state).append_paragraph("refused-tail");
    assert_eq!(
        ask(
            &mut worker,
            Request::Take {
                doc: 1,
                update: refused_tail,
            }
        )
        .err()
        .map(|r| r.refusal),
        Some(Refusal::BodyTooLarge)
    );
    worker.update_limits.body_bytes = saved_body;
    let held = &worker.docs[&1];
    assert_eq!(held.state_bytes, expected_state_bytes, "equal-size restore");
    assert_eq!(held.base, state, "rebased onto settled_state");
    assert!(held.since.is_empty() && held.since_bytes == 0);
    assert!(held.last_state.is_none() && held.settled);
    assert_eq!(worker.held_bytes, held.state_bytes);
    assert_eq!(ask(&mut worker, Request::Close(1)), Ok(Answer::Done));
    assert_eq!(worker.held_bytes, 0);
}

/// Multi-client `Source::Body` (`> MAX_CLIENT_CHANGES / 4` changes) reports its actual
/// split `clients > 1` count in `Answer::Opened`.
#[test]
fn multi_client_source_body_reports_actual_client_count() {
    let mut worker = Worker::new(Limits {
        update_bytes: 16 * 1024,
        state_bytes: 8 * 1024 * 1024,
        body_bytes: 8 * 1024 * 1024,
        documents: 1,
        held_bytes: 16 * 1024 * 1024,
    });
    let marked_para = serde_json::json!({
        "type": "paragraph",
        "content": [
            {"type": "text", "text": "a", "marks": [{"type": "bold"}]},
            {"type": "text", "text": "b", "marks": [{"type": "italic"}]},
        ],
    });
    let split_body = serde_json::to_vec(&serde_json::json!({
        "type": "doc",
        "content": vec![marked_para; 22_000],
    }))
    .expect("json");
    let Ok(Answer::Opened { clients, .. }) = ask(
        &mut worker,
        Request::Open {
            doc: 1,
            source: Source::Body(Some(split_body)),
            share: Share::Kept,
        },
    ) else {
        panic!("opened multi-client body");
    };
    assert_eq!(clients, 2, "split body reports actual client count");
    assert_eq!(ask(&mut worker, Request::Close(1)), Ok(Answer::Done));
    assert_eq!(worker.held_bytes, 0);
}
