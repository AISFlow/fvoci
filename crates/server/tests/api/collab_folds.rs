//! What is stored of a document after its room is done with it: the log folded into
//! the state as the document closes (its room empties, or the server stops), also
//! when the server that held it was killed and the next one starts, and never longer
//! than the fold's bound in a room that stays open. The log's rows are counted in the
//! table, whether or not the state holds them.

use std::time::{Duration, Instant};

use fvoci_collab::reading_steps;
use fvoci_collab::testing::Editor;
use fvoci_model::ContainerVisibility;
use fvoci_server::collab::FOLD_AFTER;
use serde_json::{Value, json};

use crate::body_versions::versions;
use crate::collab::{Editing, body_of, card, editing, editing_app, joined};
use crate::common::{Setup, TEST_WORKSPACE_ID, TEST_WS, TestServer};
use crate::notifications::{board_with_owner, card_with, give, kinds, told};
use crate::rooms::{Peer, paragraphs, table};

/// The state's last update, the log's last, the last one the card holds, and how many
/// rows the log has.
async fn stored(e: &Editing) -> (i64, i64, i64, usize) {
    let store = e.app.store();
    // The rows first: a fold that comes between the two reads is then seen whole in
    // the second (it changes the state and the log in one transaction).
    let rows = store
        .logged_body_updates(&e.card_id)
        .await
        .expect("counted");
    let stored = store
        .get_body_doc(&e.card_id)
        .await
        .expect("read")
        .expect("a document");
    (
        stored.state_seq,
        stored.log_end,
        stored.projected_seq,
        usize::try_from(rows).expect("a count"),
    )
}

/// Waits until what is stored of the document has no log, and says so of it.
async fn folded(e: &Editing, why: &str) {
    let began = Instant::now();
    loop {
        let (state_seq, log_end, projected, rows) = stored(e).await;
        if rows == 0 {
            assert_eq!((state_seq, projected), (log_end, log_end), "{why}");
            return;
        }
        assert!(
            began.elapsed() < Duration::from_secs(20),
            "{why}: {rows} rows left"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Writes `lines` into the card's document as its first user, each an update.
async fn typed(e: &Editing, lines: &[String]) -> Editor {
    let collab = e.app.collab();
    let (editor, epoch) = joined(collab, &e.app, &e.card_id, 101).await;
    for line in lines {
        collab
            .take(
                &e.app,
                &e.card_id,
                epoch,
                &e.user1,
                &editor.append_paragraph(line),
            )
            .await
            .expect("taken");
    }
    editor
}

fn lines(count: usize) -> Vec<String> {
    (0..count).map(|n| format!("line {n}")).collect()
}

/// The document of the card as a server opens it now, and the body.
async fn reopened(th: &TestServer, card_id: &str) -> Option<Value> {
    let e = Editing {
        card_id: card_id.to_owned(),
        ..editing_app(th).await
    };
    let (again, _) = joined(e.app.collab(), &e.app, &e.card_id, 109).await;
    again.body().map(|body| body.document().clone())
}

/// The room of a card that empties, its last editor gone after 30 updates (fewer
/// than a fold's worth), stores its document as its state alone: the log is empty,
/// and the document opened again is the same.
#[tokio::test]
async fn a_room_that_empties_folds_its_log() {
    let th = Setup::native().start().await.init_basic().await;
    let t = table(&th, &paragraphs(&["first"])).await;
    let mut peer = Peer::connect(&th.client, 101).await;
    peer.joined(TEST_WORKSPACE_ID, &t.card_id).await;
    for line in lines(30) {
        peer.write_acked(&line).await;
    }
    let wanted = peer.body();
    let e = Editing {
        card_id: t.card_id.clone(),
        ..editing_app(&th).await
    };
    let (_, _, _, rows) = stored(&e).await;
    assert_eq!(rows, 30, "logged, not folded yet");

    peer.leave().await;
    folded(&e, "the room emptied").await;
    assert_eq!(body_of(&card(&th.client, &e.card_id).await), wanted);
    assert_eq!(reopened(&th, &e.card_id).await, wanted, "the same document");
    th.teardown().await;
}

/// A document closed through the place of the documents, and the documents of a
/// server that stops, are stored as their state alone.
#[tokio::test]
async fn a_document_closed_or_stopped_folds_its_log() {
    let th = Setup::native().start().await.init_basic().await;
    let closed = editing(&th, &paragraphs(&["closed"])).await;
    let stopped = editing(&th, &paragraphs(&["stopped"])).await;
    let wanted_closed = typed(&closed, &lines(30))
        .await
        .body()
        .map(|body| body.document().clone());
    typed(&stopped, &lines(20)).await;
    closed
        .app
        .collab()
        .close(&closed.app, &closed.card_id)
        .await
        .expect("closed");
    folded(&closed, "closed").await;
    assert_eq!(
        body_of(&card(&th.client, &closed.card_id).await),
        wanted_closed
    );
    assert_eq!(reopened(&th, &closed.card_id).await, wanted_closed);

    stopped.app.collab().end(&stopped.app).await;
    folded(&stopped, "the server stopped").await;
    th.teardown().await;
}

/// What a server that was killed left in the log, written to the card or not yet, is
/// folded by the next server as it starts: the log is empty and the body is the same.
#[tokio::test]
async fn the_log_a_killed_server_left_is_folded_by_the_next() {
    let th = Setup::native().start().await.init_basic().await;
    let written = editing(&th, &paragraphs(&["written"])).await;
    let unwritten = editing(&th, &paragraphs(&["unwritten"])).await;
    let wanted_written = typed(&written, &lines(40)).await;
    // Its body is written to the card and kept as a version (its time has come): it
    // is finished, and only its log is left, which nothing else would have folded.
    written
        .app
        .collab()
        .pass_version_time(Duration::from_hours(1));
    written
        .app
        .collab()
        .write_body(&written.app, &written.card_id)
        .await
        .expect("written");
    let (_, written_end, projected, rows) = stored(&written).await;
    assert_eq!((projected, rows), (written_end, 40), "written, not folded");
    let unfinished = written
        .app
        .store()
        .unfinished_body_docs()
        .await
        .expect("read");
    assert!(
        unfinished
            .iter()
            .all(|(card_id, _)| *card_id != written.card_id),
        "written and kept: {unfinished:?}"
    );
    let wanted_unwritten = typed(&unwritten, &lines(25)).await;
    let wanted = [
        (
            written.card_id.clone(),
            wanted_written.body().map(|body| body.document().clone()),
        ),
        (
            unwritten.card_id.clone(),
            wanted_unwritten.body().map(|body| body.document().clone()),
        ),
    ];

    let th = th
        .killed_and_started_again(async |app| app.collab().abandon().await)
        .await;
    for (card_id, body) in &wanted {
        let e = Editing {
            card_id: card_id.clone(),
            ..editing_app(&th).await
        };
        folded(&e, &format!("as the server started, {card_id}")).await;
        assert_eq!(
            &body_of(&card(&th.client, card_id).await),
            body,
            "{card_id}"
        );
        assert_eq!(&reopened(&th, card_id).await, body, "{card_id}");
    }
    th.teardown().await;
}

/// In a room that stays open, the log never holds a fold's worth of updates: after
/// each, it is folded once it would.
#[tokio::test]
async fn the_log_of_a_room_that_stays_open_does_not_grow() {
    let th = Setup::native().start().await.init_basic().await;
    let e = editing(&th, &paragraphs(&["first"])).await;
    let collab = e.app.collab();
    let (editor, epoch) = joined(collab, &e.app, &e.card_id, 101).await;
    let bound = usize::try_from(FOLD_AFTER).expect("a count");
    for n in 0..(3 * bound + 17) {
        let update = if n % 4 == 3 {
            editor.delete_last()
        } else {
            editor.append_paragraph(&format!("line {n}"))
        };
        collab
            .take(&e.app, &e.card_id, epoch, &e.user1, &update)
            .await
            .expect("taken");
        let (_, _, _, rows) = stored(&e).await;
        assert!(rows < bound, "after update {n}: {rows} rows");
    }
    let wanted = editor.body().map(|body| body.document().clone());
    collab.abandon().await;
    assert_eq!(
        reopened(&th, &e.card_id).await,
        wanted,
        "the folded state and the log after it"
    );
    th.teardown().await;
}

/// When updates are large enough that the worker's rollback log (`since`) reaches half
/// of `collab_state_bytes` well before `FOLD_AFTER` updates, the worker asks the server
/// to fold (`Answer::Taken::fold`) and the server folds early so `since` stays bounded.
#[tokio::test]
async fn large_updates_trigger_fold_before_fold_after_count() {
    let th = Setup::native().start().await.init_basic().await;
    let e = editing(&th, &paragraphs(&["first"])).await;
    let collab = e.app.collab();
    let (editor, epoch) = joined(collab, &e.app, &e.card_id, 101).await;
    // Default `collab_state_bytes` is 16 MiB, so `fold_since_bytes` is 8 MiB.
    // Sending ~60 KiB updates crosses 8 MiB in ~137 updates, well below FOLD_AFTER (200),
    // while deleting periodically keeps the live body well under 8 MiB.
    let payload = "a".repeat(60 * 1024);
    let mut folded_early = false;
    for n in 1..180_usize {
        let update = editor.append_paragraph(&payload);
        collab
            .take(&e.app, &e.card_id, epoch, &e.user1, &update)
            .await
            .expect("taken");
        let (state_seq, log_end, _, rows) = stored(&e).await;
        if state_seq > 0 && log_end < FOLD_AFTER {
            assert_eq!(rows, 0, "early fold cleared log rows at step {n}");
            assert_eq!(state_seq, log_end, "state caught up to log_end");
            folded_early = true;
            break;
        }
        // Keep the live body small so body_bytes (8 MiB) is not hit before since_bytes (8 MiB).
        if n % 4 == 0 {
            let del = editor.delete_last();
            collab
                .take(&e.app, &e.card_id, epoch, &e.user1, &del)
                .await
                .expect("deleted");
        }
    }
    assert!(
        folded_early,
        "worker fold signal triggered a server fold before FOLD_AFTER ({FOLD_AFTER}) updates"
    );
    th.teardown().await;
}

/// Waits until a saved body version names `actor_id` and returns its stored body JSON.
async fn wait_version_body(th: &TestServer, card_id: &str, actor_id: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    let version = loop {
        let kept = versions(&th.client, card_id).await;
        if let Some(newest) = kept.first()
            && newest["editors"]
                .as_array()
                .is_some_and(|editors| editors.iter().any(|editor| editor == &json!(actor_id)))
        {
            break newest["version"].as_i64().expect("version number");
        }
        assert!(Instant::now() < deadline, "version not kept for {actor_id}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    let version_body: Value = th
        .client
        .get(&format!(
            "{TEST_WS}/cards/{card_id}/body-versions/{version}"
        ))
        .await
        .ok_json();
    version_body["body"].clone()
}

/// When the settle check fails during a live room fold (`FOLD_AFTER`), the server recovers
/// via T3 remake without losing any updates, mentions, or versions: peers receive
/// `docReset` for epoch 2, the log is cleared, and the remade doc takes 0 reading steps.
#[tokio::test]
async fn collab_fold_settle_failure_recovers_via_remake_preserving_body_mentions_and_version() {
    let th = Setup::native().start().await.init_basic().await;
    let (a, b) = (th.client.me().await, th.client2.me().await);
    let board = board_with_owner(&th, ContainerVisibility::Private).await;
    give(&th, &board.id, &b.id, "editor").await;
    let initial = card_with(&th.client, &board.id, &paragraphs(&["start"])).await;
    let e = Editing {
        card_id: initial.id.clone(),
        ..editing_app(&th).await
    };

    let collab = e.app.collab();
    let (editor, epoch) = joined(collab, &e.app, &e.card_id, 100).await;
    let before_fold = usize::try_from(FOLD_AFTER).expect("fits") - 2;
    for n in 0..before_fold {
        collab
            .take(
                &e.app,
                &e.card_id,
                epoch,
                &a.id,
                &editor.append_paragraph(&format!("fold-line-{n}")),
            )
            .await
            .expect("taken");
    }

    let mut one = Peer::connect(&th.client, 101).await;
    let mut two = Peer::connect(&th.client2, 102).await;
    one.joined(TEST_WORKSPACE_ID, &e.card_id).await;
    two.joined(TEST_WORKSPACE_ID, &e.card_id).await;

    let mention = one.editor().append_mention(&b.id);
    one.send_update(&mention).await;
    one.text("docAck").await;
    two.settle().await;

    // Force the next settle check (triggered by the FOLD_AFTER-th update's fold) to fail.
    e.app
        .collab()
        .fail_next_settle(&e.app, &e.card_id)
        .await
        .expect("armed fail_next_settle");
    e.app.collab().pass_version_time(Duration::from_hours(1));

    let trigger = one.editor().append_paragraph("fold-trigger-line");
    let expected = one.body().expect("expected body");
    one.send_update(&trigger).await;
    one.text("docAck").await;

    for peer in [&mut one, &mut two] {
        let reset = peer.text("docReset").await;
        assert_eq!(
            reset,
            json!({"type": "docReset", "doc": peer.doc, "epoch": 2}),
            "peer receives docReset for remade epoch 2"
        );
        peer.settle().await;
        assert_eq!(peer.epoch, 2);
        assert_eq!(peer.body().as_ref(), Some(&expected));
    }

    let doc = e
        .app
        .store()
        .get_body_doc(&e.card_id)
        .await
        .expect("read")
        .expect("body doc");
    let rows = e
        .app
        .store()
        .logged_body_updates(&e.card_id)
        .await
        .expect("rows");
    assert_eq!(doc.epoch, 2, "remade to epoch 2");
    assert_eq!(rows, 0, "log cleared after remake");
    assert_eq!(
        reading_steps(&doc.state, u64::MAX).expect("steps"),
        0,
        "remade doc has 0 reading steps"
    );
    assert_eq!(
        body_of(&card(&th.client, &e.card_id).await),
        Some(expected.clone())
    );

    let b_told = told(&th, &th.client2).await;
    assert_eq!(kinds(&b_told), (vec!["mention"], 0), "{b_told:?}");
    assert_eq!(b_told[0].subject.as_ref().expect("seen").actor_id, a.id);

    let saved = wait_version_body(&th, &e.card_id, &a.id).await;
    assert_eq!(saved, expected, "body version holds all updates");

    one.leave().await;
    two.leave().await;
    th.teardown().await;
}

/// When the settle check fails while closing a document (`fold_as_it_closes`), the server
/// recovers via T3 remake without losing the body, mention notification, or saved version,
/// clears the log, and stores a 0-step epoch 2 document that reopens cleanly.
#[tokio::test]
async fn collab_close_settle_failure_recovers_via_remake_preserving_body_mentions_and_version() {
    let th = Setup::native().start().await.init_basic().await;
    let (a, b) = (th.client.me().await, th.client2.me().await);
    let board = board_with_owner(&th, ContainerVisibility::Private).await;
    give(&th, &board.id, &b.id, "editor").await;
    let initial = card_with(&th.client, &board.id, &paragraphs(&["close-start"])).await;
    let e = Editing {
        card_id: initial.id.clone(),
        ..editing_app(&th).await
    };

    let mut peer = Peer::connect(&th.client, 201).await;
    peer.joined(TEST_WORKSPACE_ID, &e.card_id).await;
    peer.write_acked("close-line-1").await;
    let mention = peer.editor().append_mention(&b.id);
    peer.send_update(&mention).await;
    peer.text("docAck").await;
    let expected = peer.body().expect("expected body");

    e.app
        .collab()
        .fail_next_settle(&e.app, &e.card_id)
        .await
        .expect("armed fail_next_settle");
    peer.leave().await;
    folded(&e, "close settle failure recovered via remake").await;

    let doc = e
        .app
        .store()
        .get_body_doc(&e.card_id)
        .await
        .expect("read")
        .expect("body doc");
    assert_eq!(doc.epoch, 2, "remade to epoch 2 on close");
    assert_eq!(
        reading_steps(&doc.state, u64::MAX).expect("steps"),
        0,
        "remade doc has 0 reading steps"
    );
    assert_eq!(
        body_of(&card(&th.client, &e.card_id).await),
        Some(expected.clone())
    );
    assert_eq!(
        reopened(&th, &e.card_id).await,
        Some(expected.clone()),
        "reopens with exact body"
    );

    let b_told = told(&th, &th.client2).await;
    assert_eq!(kinds(&b_told), (vec!["mention"], 0), "{b_told:?}");
    assert_eq!(b_told[0].subject.as_ref().expect("seen").actor_id, a.id);

    let saved = wait_version_body(&th, &e.card_id, &a.id).await;
    assert_eq!(saved, expected, "close version holds all updates");
    th.teardown().await;
}

/// When `Collab::open` encounters a settle failure on an already-open document with
/// unwritten updates, it flushes the body and remakes into a 0-step epoch 2 document
/// without losing unwritten edits or mentions.
#[tokio::test]
async fn collab_open_while_unsettled_settle_failure_remakes_without_losing_edits() {
    let th = Setup::native().start().await.init_basic().await;
    let (a, b) = (th.client.me().await, th.client2.me().await);
    let board = board_with_owner(&th, ContainerVisibility::Private).await;
    give(&th, &board.id, &b.id, "editor").await;
    let initial = card_with(&th.client, &board.id, &paragraphs(&["open-start"])).await;
    let e = Editing {
        card_id: initial.id.clone(),
        ..editing_app(&th).await
    };
    let collab = e.app.collab();
    collab.leave_due_writes();

    let (editor, epoch) = joined(collab, &e.app, &e.card_id, 301).await;
    collab
        .take(
            &e.app,
            &e.card_id,
            epoch,
            &a.id,
            &editor.append_paragraph("unwritten-before-open"),
        )
        .await
        .expect("taken");
    collab
        .take(
            &e.app,
            &e.card_id,
            epoch,
            &a.id,
            &editor.append_mention(&b.id),
        )
        .await
        .expect("mention taken");
    let expected = editor.body().map(|body| body.document().clone());

    collab
        .fail_next_settle(&e.app, &e.card_id)
        .await
        .expect("armed fail_next_settle");
    let loaded = collab
        .open(&e.app, &e.card_id)
        .await
        .expect("open recovered via remake");
    assert_eq!(loaded.epoch, 2, "remade to epoch 2");
    assert_eq!(
        reading_steps(&loaded.state, u64::MAX).expect("steps"),
        0,
        "remade state takes 0 reading steps"
    );
    let joined_after = Editor::join(302, &loaded.state);
    assert_eq!(
        joined_after.body().map(|body| body.document().clone()),
        expected,
        "loaded state keeps unwritten edits"
    );
    assert_eq!(body_of(&card(&th.client, &e.card_id).await), expected);

    let b_told = told(&th, &th.client2).await;
    assert_eq!(kinds(&b_told), (vec!["mention"], 0), "{b_told:?}");
    assert_eq!(b_told[0].subject.as_ref().expect("seen").actor_id, a.id);
    th.teardown().await;
}
