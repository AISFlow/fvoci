//! A card's body written whole while its document holds what editors typed: the
//! editors' updates are never written over, nor is a body written past the document.

use fvoci_server::collab::{DocChange, DocEvent};
use serde_json::{Value, json};
use tokio::sync::broadcast::Receiver;

use crate::collab::{body_of, card, editing, joined};
use crate::common::Setup;
use crate::common::TEST_WS;

fn paragraphs(texts: &[&str]) -> Value {
    let content: Vec<Value> = texts
        .iter()
        .map(|text| json!({"type": "paragraph", "content": [{"type": "text", "text": text}]}))
        .collect();
    json!({"type": "doc", "content": content})
}

/// Whether the room was told `change` about the card, passing over the rest.
fn told(events: &mut Receiver<DocEvent>, card_id: &str, change: DocChange) -> bool {
    while let Ok(event) = events.try_recv() {
        if event.card_id == card_id && event.change == change {
            return true;
        }
    }
    false
}

/// A title written to the card between the read of the card and the write of the
/// document's body: the body is written all the same, with what the editors typed, and
/// the title stays.
#[tokio::test]
async fn a_title_written_while_a_body_is_written_keeps_what_the_editors_typed() {
    let th = Setup::native().start().await.init_basic().await;
    let e = editing(&th, &paragraphs(&["first"])).await;
    let collab = e.app.collab().clone();
    let mut events = collab.subscribe();
    let (editor, editor_epoch) = joined(&collab, &e.app, &e.card_id, 101).await;
    collab
        .take(
            &e.app,
            &e.card_id,
            editor_epoch,
            &e.user1,
            &editor.append_paragraph("typed"),
        )
        .await
        .expect("taken");
    let (waiting, go) = collab.pause_next_projection();
    let projection = tokio::spawn({
        let (collab, app, card_id) = (collab.clone(), e.app.clone(), e.card_id.clone());
        async move { collab.write_body(&app, &card_id).await }
    });
    waiting.notified().await;
    th.client
        .patch(
            &format!("{TEST_WS}/cards/{}", e.card_id),
            &json!({"title": "renamed"}),
        )
        .await
        .check_ok();
    go.notify_one();
    projection.await.expect("the task ends").expect("written");
    let read = card(&th.client, &e.card_id).await;
    assert_eq!(read.title, "renamed");
    assert_eq!(
        body_of(&read),
        editor.body().map(|body| body.document().clone()),
        "what the editors typed"
    );
    assert!(!told(&mut events, &e.card_id, DocChange::Replaced));
    th.teardown().await;
}

/// A body written to the card between the read of the card and the write of the
/// document's body, by a writer that does not go through the document (the store
/// itself): it is not written over, and the document is made again from it.
#[tokio::test]
async fn a_body_written_while_a_body_is_written_is_not_written_over() {
    let th = Setup::native().start().await.init_basic().await;
    let e = editing(&th, &paragraphs(&["first"])).await;
    let collab = e.app.collab().clone();
    let mut events = collab.subscribe();
    let (editor, editor_epoch) = joined(&collab, &e.app, &e.card_id, 101).await;
    collab
        .take(
            &e.app,
            &e.card_id,
            editor_epoch,
            &e.user1,
            &editor.append_paragraph("typed"),
        )
        .await
        .expect("taken");
    let (waiting, go) = collab.pause_next_projection();
    let projection = tokio::spawn({
        let (collab, app, card_id) = (collab.clone(), e.app.clone(), e.card_id.clone());
        async move { collab.write_body(&app, &card_id).await }
    });
    waiting.notified().await;
    let past = paragraphs(&["written past"]);
    let read = card(&th.client, &e.card_id).await;
    e.app
        .store()
        .patch_block(
            &e.card_id,
            &fvoci_model::BlockPatch {
                body: fvoci_model::BodyChange::Set(
                    fvoci_model::Body::parse(past.clone())
                        .expect("a body")
                        .expect("not empty"),
                ),
                read_at: Some(read.updated_at),
                ..fvoci_model::BlockPatch::default()
            },
            &e.user2,
        )
        .await
        .expect("written past the document");
    go.notify_one();
    projection.await.expect("the task ends").expect("written");
    assert_eq!(
        body_of(&card(&th.client, &e.card_id).await),
        Some(past.clone()),
        "not written over"
    );
    assert!(told(&mut events, &e.card_id, DocChange::Replaced));
    let (again, _) = joined(&collab, &e.app, &e.card_id, 102).await;
    assert_eq!(again.body().map(|body| body.document().clone()), Some(past));
    th.teardown().await;
}

/// A whole body written over the routes to a card whose document is closed with
/// updates in its log that the card does not hold (a server that ended before it wrote
/// them): they are written first, and a writer who read the card before is refused.
#[tokio::test]
async fn a_body_written_whole_over_a_log_not_written_yet_is_refused_until_read_again() {
    let th = Setup::native().start().await.init_basic().await;
    let e = editing(&th, &paragraphs(&["first"])).await;
    let collab = e.app.collab();
    let (editor, editor_epoch) = joined(collab, &e.app, &e.card_id, 101).await;
    collab
        .take(
            &e.app,
            &e.card_id,
            editor_epoch,
            &e.user1,
            &editor.append_paragraph("typed"),
        )
        .await
        .expect("taken");
    collab.abandon().await;
    let read_before = card(&th.client, &e.card_id).await;
    assert_ne!(
        body_of(&read_before),
        editor.body().map(|body| body.document().clone()),
        "not written yet"
    );
    let path = format!("{TEST_WS}/cards/{}", e.card_id);
    let whole = paragraphs(&["written whole"]);
    th.client
        .patch(
            &path,
            &json!({"body": whole, "readAt": read_before.updated_at}),
        )
        .await
        .check_status(reqwest::StatusCode::CONFLICT);
    let read = card(&th.client, &e.card_id).await;
    assert_eq!(
        body_of(&read),
        editor.body().map(|body| body.document().clone()),
        "the log was written first"
    );
    th.client
        .patch(&path, &json!({"body": whole, "readAt": read.updated_at}))
        .await
        .check_ok();
    let (again, _) = joined(collab, &e.app, &e.card_id, 102).await;
    assert_eq!(
        again.body().map(|body| body.document().clone()),
        Some(whole)
    );
    th.teardown().await;
}

/// A batch of patches that writes a card's body whole meets its open document as one
/// patch does: refused for a read before the editors' update, and the document that
/// body after.
#[tokio::test]
async fn a_batch_that_writes_a_body_whole_meets_the_open_document() {
    let th = Setup::native().start().await.init_basic().await;
    let e = editing(&th, &paragraphs(&["first"])).await;
    let collab = e.app.collab();
    let mut events = collab.subscribe();
    let (editor, editor_epoch) = joined(collab, &e.app, &e.card_id, 101).await;
    let read_before = card(&th.client, &e.card_id).await;
    collab
        .take(
            &e.app,
            &e.card_id,
            editor_epoch,
            &e.user1,
            &editor.append_paragraph("typed"),
        )
        .await
        .expect("taken");
    let whole = paragraphs(&["written in a batch"]);
    let batch = |read_at: i64| {
        json!({
            "blockIds": [e.card_id],
            "blockPatches": [{"body": whole, "readAt": read_at}],
        })
    };
    let path = format!("{TEST_WS}/boards/{}/blocks", read_before.board_id);
    th.client
        .patch(&path, &batch(read_before.updated_at))
        .await
        .check_status(reqwest::StatusCode::CONFLICT);
    let read = card(&th.client, &e.card_id).await;
    th.client
        .patch(&path, &batch(read.updated_at))
        .await
        .check_no_content();
    assert!(told(&mut events, &e.card_id, DocChange::Replaced));
    let (again, _) = joined(collab, &e.app, &e.card_id, 102).await;
    assert_eq!(
        again.body().map(|body| body.document().clone()),
        Some(whole)
    );
    th.teardown().await;
}

/// A batch whose identifiers are not in the order the cards were made in writes the
/// body of the card its patch is for around that card's document.
#[tokio::test]
async fn a_batch_writes_the_body_of_the_card_its_patch_is_for() {
    let th = Setup::native().start().await.init_basic().await;
    let older = editing(&th, &paragraphs(&["older"])).await;
    let collab = older.app.collab();
    let board_id = card(&th.client, &older.card_id).await.board_id;
    // Made after the first, on its board.
    let newer: fvoci_model::Card = th
        .client
        .post(
            &format!("{TEST_WS}/boards/{board_id}/cards"),
            &json!({"title": "newer", "body": paragraphs(&["newer"])}),
        )
        .await
        .created_json();
    let (editor, editor_epoch) = joined(collab, &older.app, &older.card_id, 101).await;
    let read_before = card(&th.client, &older.card_id).await;
    collab
        .take(
            &older.app,
            &older.card_id,
            editor_epoch,
            &older.user1,
            &editor.append_paragraph("typed"),
        )
        .await
        .expect("taken");
    let whole = paragraphs(&["written in a batch"]);
    let path = format!("{TEST_WS}/boards/{}/blocks", read_before.board_id);
    th.client
        .patch(
            &path,
            &json!({
                "blockIds": [newer.id, older.card_id],
                "blockPatches": [{"title": "x"}, {"body": whole, "readAt": read_before.updated_at}],
            }),
        )
        .await
        .check_status(reqwest::StatusCode::CONFLICT);
    assert_eq!(
        body_of(&card(&th.client, &older.card_id).await),
        editor.body().map(|body| body.document().clone()),
        "the editors' update was written first"
    );
    th.teardown().await;
}

/// A patch of boards and blocks that writes a card's body whole meets its document as a
/// patch of the card does.
#[tokio::test]
async fn a_patch_of_boards_and_blocks_meets_the_open_document() {
    let th = Setup::native().start().await.init_basic().await;
    let e = editing(&th, &paragraphs(&["first"])).await;
    let collab = e.app.collab();
    let mut events = collab.subscribe();
    let (editor, editor_epoch) = joined(collab, &e.app, &e.card_id, 101).await;
    let read_before = card(&th.client, &e.card_id).await;
    collab
        .take(
            &e.app,
            &e.card_id,
            editor_epoch,
            &e.user1,
            &editor.append_paragraph("typed"),
        )
        .await
        .expect("taken");
    let whole = paragraphs(&["written with the boards"]);
    let patch = |read_at: i64| {
        json!({
            "containerIds": [read_before.board_id],
            "containerPatches": [{"title": "the board"}],
            "blockIds": [e.card_id],
            "blockPatches": [{"body": whole, "readAt": read_at}],
        })
    };
    th.client
        .patch(
            &format!("{TEST_WS}/containers-and-blocks"),
            &patch(read_before.updated_at),
        )
        .await
        .check_status(reqwest::StatusCode::CONFLICT);
    let read = card(&th.client, &e.card_id).await;
    th.client
        .patch(
            &format!("{TEST_WS}/containers-and-blocks"),
            &patch(read.updated_at),
        )
        .await
        .check_ok();
    assert!(told(&mut events, &e.card_id, DocChange::Replaced));
    let (again, _) = joined(collab, &e.app, &e.card_id, 102).await;
    assert_eq!(
        again.body().map(|body| body.document().clone()),
        Some(whole)
    );
    th.teardown().await;
}

/// A document whose stored state is past a limit of a state lowered while the server
/// was down opens with all it took: its body, with the update the card did not hold yet, is
/// written to the card first, and it is made again from that body, which the room is
/// told, for the card's organization.
#[tokio::test]
async fn a_document_past_a_lowered_state_limit_is_written_before_it_is_made_again() {
    const LOWER: u64 = 16 * 1024 * 1024;
    let th = Setup::native()
        .configure(|config| {
            // Four times the limit it is lowered to, and a worker's memory with room
            // for it (512 MiB and 32 times the state = 2.5 GiB).
            config.collab.collab_state_bytes = 4 * LOWER;
            config.collab.collab_worker_memory_bytes = 3 * 1024 * 1024 * 1024;
            // Large updates, so that few of them bring the document past the lower
            // limit: every update is paid for by the size of the document.
            config.collab.collab_update_bytes = 1024 * 1024;
        })
        .start()
        .await
        .init_basic()
        .await;
    let e = editing(&th, &paragraphs(&["first"])).await;
    let collab = e.app.collab();
    // The two hundred updates below may take a loaded machine longer than a body
    // waits to be written: the server is killed before it writes this one.
    collab.leave_due_writes();
    let (editor, epoch) = joined(collab, &e.app, &e.card_id, 101).await;
    let take = async |update: Vec<u8>| {
        collab
            .take(&e.app, &e.card_id, epoch, &e.user1, &update)
            .await
            .expect("taken")
    };
    // What is deleted stays in the state: past the lower limit (16 MiB), under the 32 MiB
    // fold threshold and 48 MiB remake mark, and folded into the stored state at the 200th update.
    let line = "d".repeat(960 * 1024);
    for round in 0..100 {
        take(editor.append_paragraph(if round < 82 { "d" } else { &line })).await;
        take(editor.delete_last()).await;
    }
    take(editor.append_paragraph("the last word")).await;
    let stored = e
        .app
        .store()
        .get_body_doc(&e.card_id)
        .await
        .expect("read")
        .expect("a document");
    assert!(
        stored.state_seq == 200
            && u64::try_from(stored.state.len()).is_ok_and(|bytes| bytes > LOWER),
        "a stored state past the lower limit: {} bytes through {}",
        stored.state.len(),
        stored.state_seq
    );
    let wanted = editor.body().map(|body| body.document().clone());
    assert_ne!(
        body_of(&card(&th.client, &e.card_id).await),
        wanted,
        "not written yet"
    );
    let card_id = e.card_id.clone();
    let mut th = th;
    th.config.collab.collab_state_bytes = LOWER;
    let th = th
        .killed_and_started_again(async |app| app.collab().abandon().await)
        .await;
    let e = crate::collab::Editing {
        card_id,
        ..crate::collab::editing_app(&th).await
    };
    let collab = e.app.collab();
    let mut events = collab.subscribe();
    let (again, again_epoch) = joined(collab, &e.app, &e.card_id, 102).await;
    assert_eq!(again.body().map(|body| body.document().clone()), wanted);
    assert_eq!(body_of(&card(&th.client, &e.card_id).await), wanted);
    let stored = e
        .app
        .store()
        .get_body_doc(&e.card_id)
        .await
        .expect("read")
        .expect("a document");
    assert!(
        stored.state.len() < 1024 * 1024,
        "made again: {} bytes",
        stored.state.len()
    );
    assert_eq!(stored.epoch, again_epoch);
    // The start may have opened it first, and told before the subscription.
    if let Some(remade) = std::iter::from_fn(|| events.try_recv().ok())
        .find(|event| event.change == DocChange::Remade)
    {
        assert_eq!(remade.tenant, e.app.store().tenant());
    }
    th.teardown().await;
}

/// A log whose bytes cannot be read (written past the server, here by the store
/// itself) is dropped, which is logged, and the document is made again from its card:
/// it opens, instead of being refused for good.
#[tokio::test]
async fn an_unreadable_log_gives_way_to_the_card() {
    let th = Setup::native().start().await.init_basic().await;
    let e = editing(&th, &paragraphs(&["first"])).await;
    let collab = e.app.collab();
    collab.open(&e.app, &e.card_id).await.expect("opened");
    collab.abandon().await;
    let as_read = crate::collab_written_past::stored(&e).await.as_read();
    e.app
        .store()
        .append_body_updates(
            &e.card_id,
            &[(b"\xff\xff no update", e.user1.as_str())],
            as_read,
            &[],
            &[],
            fvoci_store::BodyBefore::Leave,
        )
        .await
        .expect("logged past the server");
    let (again, _) = joined(collab, &e.app, &e.card_id, 102).await;
    assert_eq!(
        again.body().map(|body| body.document().clone()),
        Some(paragraphs(&["first"])),
        "the card's body"
    );
    let stored = e
        .app
        .store()
        .get_body_doc(&e.card_id)
        .await
        .expect("read")
        .expect("a document");
    assert_eq!(
        (stored.updates.len(), stored.projected_seq),
        (0, stored.log_end),
        "the log is dropped"
    );
    th.teardown().await;
}
