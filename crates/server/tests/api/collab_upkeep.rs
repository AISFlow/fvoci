//! Keeping a card's document small: its log folded into its state, and a document that
//! grew near its limit, or names many clients, made again from its body. What it holds
//! is the same afterwards.

use std::time::Instant;

use fvoci_collab::testing::Editor;
use fvoci_server::collab::DocChange;
use serde_json::{Value, json};

use crate::collab::{body_of, card, editing, joined};
use crate::common::Setup;

fn paragraphs(texts: &[&str]) -> Value {
    let content: Vec<Value> = texts
        .iter()
        .map(|text| json!({"type": "paragraph", "content": [{"type": "text", "text": text}]}))
        .collect();
    json!({"type": "doc", "content": content})
}

/// Whether the tests run in a debug build: the times they print are of that.
fn profile() -> &'static str {
    if cfg!(debug_assertions) {
        "debug build"
    } else {
        "release build"
    }
}

/// A document that takes a thousand and fifty updates folds its log into its state at
/// every two hundredth; the store holds the state and the log after it, and the
/// document opened again from them is what it was. Prints how long the opening and the
/// updates took.
#[tokio::test]
async fn a_thousand_updates_are_folded_into_the_state_and_the_document_stays_the_same() {
    let th = Setup::native().start().await.init_basic().await;
    let e = editing(&th, &paragraphs(&["first"])).await;
    let other = editing(&th, &paragraphs(&["other"])).await;
    let collab = e.app.collab();
    let began = Instant::now();
    collab
        .open(&other.app, &other.card_id)
        .await
        .expect("opened");
    let first_open = began.elapsed();
    let began = Instant::now();
    let loaded = collab.open(&e.app, &e.card_id).await.expect("opened");
    let opened = began.elapsed();
    let editor = Editor::join(101, &loaded.state);
    let stored = async || {
        let stored = e
            .app
            .store()
            .get_body_doc(&e.card_id)
            .await
            .expect("read")
            .expect("a document");
        (stored.state_seq, stored.log_end, stored.updates.len())
    };
    let began = Instant::now();
    for n in 0..1050 {
        let update = if n % 3 == 2 {
            editor.delete_last()
        } else {
            editor.append_paragraph(&format!("line {n}"))
        };
        collab
            .take(&e.app, &e.card_id, loaded.epoch, &e.user1, &update)
            .await
            .expect("taken");
        if n + 1 == 450 {
            assert_eq!(
                stored().await,
                (400, 450, 50),
                "folded at the four hundredth"
            );
        }
    }
    let taken = began.elapsed();
    eprintln!(
        "measured ({}): the first open {} ms (the worker started with it), an open {} ms, \
         1050 updates {} ms ({} µs each: taken, logged, folded at every 200th, and the body \
         written to the card meanwhile)",
        profile(),
        first_open.as_millis(),
        opened.as_millis(),
        taken.as_millis(),
        taken.as_micros() / 1050
    );
    let wanted = editor.body().map(|body| body.document().clone());
    assert_eq!(stored().await, (1000, 1050, 50), "folded at the thousandth");

    collab.abandon().await;
    let (again, _) = joined(collab, &e.app, &e.card_id, 102).await;
    assert_eq!(
        again.body().map(|body| body.document().clone()),
        wanted,
        "the folded state and the log after it"
    );
    collab.close(&e.app, &e.card_id).await.expect("closed");
    assert_eq!(body_of(&card(&th.client, &e.card_id).await), wanted);
    th.teardown().await;
}

/// A document whose state nears its limit (what was deleted is kept in it) is written
/// to its card and made again from its body: the update that brought it there is taken,
/// the caller and the room are told, the body is the same, the state is small again, and
/// an update made against the document before is out of step.
#[tokio::test]
async fn a_document_near_its_limit_is_made_again_from_its_body() {
    let th = Setup::native()
        // Large updates, so that few of them bring the document near its limit: every
        // update is paid for by the size of the document.
        .configure(|config| config.collab.collab_update_bytes = 1024 * 1024)
        .start()
        .await
        .init_basic()
        .await;
    let e = editing(&th, &paragraphs(&["kept"])).await;
    let collab = e.app.collab();
    let mut events = collab.subscribe();
    let loaded = collab.open(&e.app, &e.card_id).await.expect("opened");
    let editor = Editor::join(101, &loaded.state);
    let idle = Editor::join(105, &loaded.state);
    let round = rounds_until_remade(&e, &editor, loaded.epoch).await;
    // Each round leaves 480 KiB deleted in the state: three quarters of the 16 MiB a
    // state may be is reached around the 25th round.
    assert!(
        round >= 24,
        "only once it was near the limit: round {round}"
    );
    let event = events.try_recv().expect("an event");
    assert_eq!(
        (event.card_id, event.change),
        (e.card_id.clone(), DocChange::Remade)
    );
    let wanted = editor.body().map(|body| body.document().clone());
    let stored = e
        .app
        .store()
        .get_body_doc(&e.card_id)
        .await
        .expect("read")
        .expect("a document");
    // At most the paragraph of the round, not the megabytes that were deleted.
    assert!(
        stored.state.len() < 1024 * 1024,
        "small again: {} bytes",
        stored.state.len()
    );
    assert_eq!(
        (stored.state_seq, stored.projected_seq, stored.updates.len()),
        (stored.log_end, stored.log_end, 0)
    );
    assert_eq!(
        body_of(&card(&th.client, &e.card_id).await),
        wanted,
        "written"
    );
    let (again, again_epoch) = joined(collab, &e.app, &e.card_id, 102).await;
    assert_eq!(again.body().map(|body| body.document().clone()), wanted);
    for (late, who) in [
        (&editor, "the editor that wrote"),
        (&idle, "an editor that did not"),
    ] {
        let refused = collab
            .take(
                &e.app,
                &e.card_id,
                loaded.epoch,
                &e.user1,
                &late.append_paragraph("late"),
            )
            .await
            .expect_err("made against the document before");
        assert_eq!(
            refused.reason().map(fvoci_server::error::Reason::as_str),
            Some("update_out_of_step"),
            "{who}"
        );
    }
    for line in ["on", "and on", "and further"] {
        let taken = collab
            .take(
                &e.app,
                &e.card_id,
                again_epoch,
                &e.user1,
                &again.append_paragraph(line),
            )
            .await
            .expect("an editor that loaded it again goes on");
        assert!(!taken.remade, "not made again at every update");
    }
    th.teardown().await;
}

/// A document that names three quarters of the clients a document may name is made
/// again from its body, which names one: the update that brought it there is taken, the
/// room is told, and an editor that loads it again goes on.
#[tokio::test]
async fn a_document_of_many_clients_is_made_again_from_its_body() {
    let th = Setup::native()
        // Room for an update of thousands of clients.
        .configure(|config| config.collab.collab_update_bytes = 1024 * 1024)
        .start()
        .await
        .init_basic()
        .await;
    let e = editing(&th, &paragraphs(&["kept"])).await;
    let collab = e.app.collab();
    let mut events = collab.subscribe();
    let loaded = collab.open(&e.app, &e.card_id).await.expect("opened");
    let editor = Editor::join(101, &loaded.state);
    let first = collab
        .take(
            &e.app,
            &e.card_id,
            loaded.epoch,
            &e.user1,
            &editor.append_paragraph("a"),
        )
        .await
        .expect("taken");
    assert!(!first.remade, "not made again for an editor");
    let many = u64::try_from(fvoci_collab::MAX_CLIENTS * 3 / 4).expect("a count fits");
    let update = of_clients((101, 0), 1_000_000, many);
    let start = Instant::now();
    let crowded = collab
        .take(&e.app, &e.card_id, loaded.epoch, &e.user1, &update)
        .await
        .expect("taken");
    println!(
        "{many} clients taken and made again in {:?} ({})",
        start.elapsed(),
        profile()
    );
    assert!(crowded.remade, "made again");
    assert!(
        std::iter::from_fn(|| events.try_recv().ok())
            .any(|event| event.card_id == e.card_id && event.change == DocChange::Remade),
        "the room is told"
    );
    let stored = e
        .app
        .store()
        .get_body_doc(&e.card_id)
        .await
        .expect("read")
        .expect("a document");
    assert!(
        stored.state.len() < 1024,
        "a body of two lines: {} bytes",
        stored.state.len()
    );
    let wanted = editor.body().map(|body| body.document().clone());
    let (again, again_epoch) = joined(collab, &e.app, &e.card_id, 102).await;
    assert_eq!(again.body().map(|body| body.document().clone()), wanted);
    let taken = collab
        .take(
            &e.app,
            &e.card_id,
            again_epoch,
            &e.user1,
            &again.append_paragraph("on"),
        )
        .await
        .expect("an editor that loaded it again goes on");
    assert!(!taken.remade, "not made again at every update");
    th.teardown().await;
}

/// An update of `count` clients from `first` on, each of one stretch of deleted content
/// after the change of the client before, the first after `after`: as the Y library
/// writes an update (version 1).
fn of_clients(after: (u64, u64), first: u64, count: u64) -> Vec<u8> {
    fn number(out: &mut Vec<u8>, mut number: u64) {
        while number >= 0x80 {
            out.push(number.to_le_bytes()[0] | 0x80);
            number >>= 7;
        }
        out.push(number.to_le_bytes()[0]);
    }
    let mut out = Vec::new();
    number(&mut out, count);
    let mut left = after;
    for client in first..first + count {
        // One change of the client, from its first clock.
        for value in [1, client, 0] {
            number(&mut out, value);
        }
        // Deleted content (1) after a neighbour (0x80): the neighbour and the length.
        out.push(0x81);
        for value in [left.0, left.1, 1] {
            number(&mut out, value);
        }
        left = (client, 0);
    }
    // Nothing deleted.
    number(&mut out, 0);
    out
}

/// Adds a paragraph of 480 KiB and deletes it again, round after round, until the
/// document is made again; the round it was.
async fn rounds_until_remade(e: &crate::collab::Editing, editor: &Editor, epoch: i64) -> usize {
    let collab = e.app.collab();
    let line = "z".repeat(480 * 1024);
    let mut remade = None;
    for round in 0..50 {
        let added = collab
            .take(
                &e.app,
                &e.card_id,
                epoch,
                &e.user1,
                &editor.append_paragraph(&line),
            )
            .await
            .expect("taken");
        if added.remade {
            remade = Some(round);
            break;
        }
        let deleted = collab
            .take(&e.app, &e.card_id, epoch, &e.user1, &editor.delete_last())
            .await
            .expect("taken");
        if deleted.remade {
            remade = Some(round);
            break;
        }
    }
    remade.expect("made again before the state reached its limit")
}
