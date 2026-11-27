//! Editing a card's body together, below the websocket: the documents the server holds
//! in its worker process and stores, and the body it writes to the card from them.

use std::time::{Duration, Instant};

use fvoci_collab::testing::Editor;
use fvoci_model::{Card, ContainerVisibility, Session};
use fvoci_server::app::App;
use fvoci_server::collab::{Collab, DocChange};
use serde_json::{Value, json};

use crate::common::TEST_WS;
use crate::common::{Client, Setup, TEST_WORKSPACE_ID, TestServer};

pub(crate) fn paragraphs(texts: &[&str]) -> Value {
    let content: Vec<Value> = texts
        .iter()
        .map(|text| json!({"type": "paragraph", "content": [{"type": "text", "text": text}]}))
        .collect();
    json!({"type": "doc", "content": content})
}

/// A card, with that body, on a board of the test workspace; and the application as a
/// member of the workspace reaches it, and the identifiers of the two users.
pub(crate) struct Editing {
    pub(crate) app: App,
    pub(crate) card_id: String,
    pub(crate) user1: String,
    pub(crate) user2: String,
}

pub(crate) async fn editing(th: &TestServer, body: &Value) -> Editing {
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let card: Card = th
        .client
        .post(
            &format!("{TEST_WS}/boards/{}/cards", board.id),
            &json!({"title": "Edited together", "body": body}),
        )
        .await
        .created_json();
    let user1 = th.client.me().await.id;
    let user2 = th.client2.me().await.id;
    let entered = th
        .app
        .enter_workspace(
            &Session {
                user_id: user1.clone(),
                ..Session::default()
            },
            TEST_WORKSPACE_ID,
        )
        .await
        .expect("the membership is read")
        .expect("a member");
    Editing {
        app: entered.app,
        card_id: card.id,
        user1,
        user2,
    }
}

/// An editor that loads a card's document, and which making of the document it loaded.
pub(crate) async fn joined(
    collab: &Collab,
    app: &App,
    card_id: &str,
    client: u64,
) -> (Editor, i64) {
    let loaded = collab.open(app, card_id).await.expect("opened");
    (Editor::join(client, &loaded.state), loaded.epoch)
}

pub(crate) async fn card(client: &Client, card_id: &str) -> Card {
    client
        .get(&format!("{TEST_WS}/cards/{card_id}"))
        .await
        .ok_json()
}

pub(crate) fn body_of(card: &Card) -> Option<Value> {
    card.body.as_ref().map(|body| body.document().clone())
}

/// Two editors that each write before they have the other's update: the document takes
/// both, each editor that asks for what it is missing ends with what the other has,
/// and the card is given the body of both, written by the last writer.
#[tokio::test]
async fn two_editors_converge_and_the_card_is_given_their_body() {
    let th = Setup::native().start().await.init_basic().await;
    let e = editing(&th, &paragraphs(&["first"])).await;
    let collab = e.app.collab();
    let loaded = collab.open(&e.app, &e.card_id).await.expect("opened");
    let one = Editor::join(101, &loaded.state);
    let other = Editor::join(102, &loaded.state);
    let from_one = one.append_paragraph("one");
    let from_other = other.append_paragraph("other");
    let taken = collab
        .take(&e.app, &e.card_id, loaded.epoch, &e.user1, &from_one)
        .await
        .expect("taken");
    assert_eq!(
        (taken.seq, taken.repair.is_none(), taken.remade),
        (1, true, false)
    );
    let taken = collab
        .take(&e.app, &e.card_id, loaded.epoch, &e.user2, &from_other)
        .await
        .expect("taken");
    assert_eq!(taken.seq, 2, "logged after the first");
    for editor in [&one, &other] {
        let vector = editor.state_vector();
        let missing = collab
            .diff(&e.app, &e.card_id, loaded.epoch, &vector)
            .await
            .expect("what it is missing");
        editor.receive(&missing);
    }
    let merged = one.body().expect("a body").document().clone();
    assert_eq!(
        other.body().map(|body| body.document().clone()),
        Some(merged.clone())
    );
    assert_eq!(
        merged["content"].as_array().map(Vec::len),
        Some(3),
        "{merged}"
    );

    let before = card(&th.client, &e.card_id).await;
    collab
        .write_body(&e.app, &e.card_id)
        .await
        .expect("written");
    let after = card(&th.client, &e.card_id).await;
    assert_eq!(
        body_of(&after),
        Some(merged),
        "the card holds the merged body"
    );
    assert!(after.updated_at > before.updated_at, "a change of the card");
    assert_eq!(after.modified_by, e.user2, "by the last writer");
    collab
        .write_body(&e.app, &e.card_id)
        .await
        .expect("nothing to write");
    assert_eq!(
        card(&th.client, &e.card_id).await.updated_at,
        after.updated_at,
        "a body that is written is not written again"
    );
    th.teardown().await;
}

/// The body is written two seconds after the last update without being asked for.
#[tokio::test]
async fn the_body_is_written_a_little_after_the_last_update() {
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
            &editor.append_paragraph("later"),
        )
        .await
        .expect("taken");
    let wanted = editor.body().map(|body| body.document().clone());
    let began = Instant::now();
    while body_of(&card(&th.client, &e.card_id).await) != wanted {
        assert!(
            began.elapsed() < Duration::from_secs(10),
            "written within the two seconds and some"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    th.teardown().await;
}

/// A server that is killed after it logged updates and before it wrote their body to
/// the card writes it when it starts again, and the document opens as it was.
#[tokio::test]
async fn a_server_killed_before_it_wrote_a_body_writes_it_when_it_starts_again() {
    let th = Setup::native().start().await.init_basic().await;
    let e = editing(&th, &paragraphs(&["first"])).await;
    let collab = e.app.collab();
    let (editor, editor_epoch) = joined(collab, &e.app, &e.card_id, 101).await;
    for line in ["one", "two", "three"] {
        collab
            .take(
                &e.app,
                &e.card_id,
                editor_epoch,
                &e.user1,
                &editor.append_paragraph(line),
            )
            .await
            .expect("taken");
    }
    let wanted = editor.body().map(|body| body.document().clone());
    assert_ne!(
        body_of(&card(&th.client, &e.card_id).await),
        wanted,
        "not written yet"
    );
    let card_id = e.card_id.clone();
    let th = th
        .killed_and_started_again(async |app| app.collab().abandon().await)
        .await;
    let began = Instant::now();
    while body_of(&card(&th.client, &card_id).await) != wanted {
        assert!(
            began.elapsed() < Duration::from_secs(10),
            "written as the server started"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let e = Editing {
        card_id,
        ..editing_app(&th).await
    };
    let again = Editor::join(
        103,
        &e.app
            .collab()
            .open(&e.app, &e.card_id)
            .await
            .expect("opened")
            .state,
    );
    assert_eq!(again.body().map(|body| body.document().clone()), wanted);
    th.teardown().await;
}

/// A body written to the card without the document (here: by the routes, with the
/// document closed) makes the document again from it the next time it is opened.
#[tokio::test]
async fn a_body_written_past_a_closed_document_makes_it_again() {
    let th = Setup::native().start().await.init_basic().await;
    let e = editing(&th, &paragraphs(&["first"])).await;
    let collab = e.app.collab();
    let mut events = collab.subscribe();
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
    collab.close(&e.app, &e.card_id).await.expect("closed");
    let read = card(&th.client, &e.card_id).await;
    th.client
        .patch(
            &format!("{TEST_WS}/cards/{}", e.card_id),
            &json!({"body": paragraphs(&["written whole"]), "readAt": read.updated_at}),
        )
        .await
        .check_ok();
    let loaded = collab.open(&e.app, &e.card_id).await.expect("opened");
    let fresh = Editor::join(104, &loaded.state);
    assert_eq!(
        fresh.body().map(|body| body.document().clone()),
        Some(paragraphs(&["written whole"]))
    );
    let event = events.try_recv().expect("an event");
    assert_eq!(
        (event.card_id, event.change),
        (e.card_id.clone(), fvoci_server::collab::DocChange::Remade)
    );
    let made_before = editor.append_paragraph("against the old one");
    let refused = collab
        .take(&e.app, &e.card_id, editor_epoch, &e.user1, &made_before)
        .await
        .expect_err("made against the document before");
    assert_eq!(
        refused.reason().map(fvoci_server::error::Reason::as_str),
        Some("update_out_of_step")
    );
    th.teardown().await;
}

/// A take whose caller stops waiting is done whole: the update is in the document and
/// in its log, and a document opened again from the log has it.
#[tokio::test]
async fn a_take_given_up_half_way_is_done_whole() {
    let th = Setup::native().start().await.init_basic().await;
    let e = editing(&th, &paragraphs(&["first"])).await;
    let collab = e.app.collab();
    let (editor, editor_epoch) = joined(collab, &e.app, &e.card_id, 101).await;
    let given_up = tokio::time::timeout(
        Duration::ZERO,
        collab.take(
            &e.app,
            &e.card_id,
            editor_epoch,
            &e.user1,
            &editor.append_paragraph("left"),
        ),
    )
    .await;
    assert!(given_up.is_err(), "given up before it was done");
    let wanted = editor.body().map(|body| body.document().clone());
    let (held, _) = joined(collab, &e.app, &e.card_id, 102).await;
    assert_eq!(
        held.body().map(|body| body.document().clone()),
        wanted,
        "in the document"
    );
    collab.close(&e.app, &e.card_id).await.expect("closed");
    assert_eq!(
        body_of(&card(&th.client, &e.card_id).await),
        wanted,
        "written"
    );
    collab.abandon().await;
    let (again, _) = joined(collab, &e.app, &e.card_id, 103).await;
    assert_eq!(
        again.body().map(|body| body.document().clone()),
        wanted,
        "in the log"
    );
    th.teardown().await;
}

/// The next event of a card's document that is `change`, passing over others.
fn told(
    events: &mut tokio::sync::broadcast::Receiver<fvoci_server::collab::DocEvent>,
    card_id: &str,
    change: DocChange,
) -> bool {
    while let Ok(event) = events.try_recv() {
        if event.card_id == card_id && event.change == change {
            return true;
        }
    }
    false
}

/// A whole body written over the routes to a card whose document is open: a writer who
/// read the card before the editors' last update is refused (409), as that update is
/// written to the card first; one who read it after writes it, the document is made
/// again from that body, the room is told, and an update made against the document
/// before is out of step.
#[tokio::test]
async fn a_body_written_whole_makes_the_open_document_that_body() {
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
    let whole = paragraphs(&["written whole"]);
    let path = format!("{TEST_WS}/cards/{}", e.card_id);
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
        "the editors' update was written first"
    );
    th.client
        .patch(&path, &json!({"body": whole, "readAt": read.updated_at}))
        .await
        .check_ok();
    assert!(
        told(&mut events, &e.card_id, DocChange::Replaced),
        "the room is told"
    );
    assert_eq!(
        body_of(&card(&th.client, &e.card_id).await),
        Some(whole.clone())
    );
    let (again, again_epoch) = joined(collab, &e.app, &e.card_id, 102).await;
    assert_eq!(
        again.body().map(|body| body.document().clone()),
        Some(whole.clone())
    );
    let refused = collab
        .take(
            &e.app,
            &e.card_id,
            editor_epoch,
            &e.user1,
            &editor.append_paragraph("late"),
        )
        .await
        .expect_err("made against the document before");
    assert_eq!(
        refused.reason().map(fvoci_server::error::Reason::as_str),
        Some("update_out_of_step")
    );
    let stored = e
        .app
        .store()
        .get_body_doc(&e.card_id)
        .await
        .expect("read")
        .expect("a document");
    assert_eq!(
        (stored.projected_seq, stored.state_seq, stored.updates.len()),
        (stored.log_end, stored.log_end, 0),
        "the log is folded into the document made again"
    );
    collab
        .take(
            &e.app,
            &e.card_id,
            again_epoch,
            &e.user1,
            &again.append_paragraph("on"),
        )
        .await
        .expect("an editor that loaded it again goes on");
    collab.close(&e.app, &e.card_id).await.expect("closed");
    assert_eq!(
        body_of(&card(&th.client, &e.card_id).await),
        again.body().map(|body| body.document().clone())
    );
    th.teardown().await;
}

/// A body written to the card past its open document by a writer that does not go
/// through the document (here: the store itself) is not written over when the
/// document's body is written: the document is made again from it.
#[tokio::test]
async fn a_body_written_past_an_open_document_is_not_written_over() {
    let th = Setup::native().start().await.init_basic().await;
    let e = editing(&th, &paragraphs(&["first"])).await;
    let collab = e.app.collab();
    let mut events = collab.subscribe();
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
    let whole = paragraphs(&["written past"]);
    let read = card(&th.client, &e.card_id).await;
    e.app
        .store()
        .patch_block(
            &e.card_id,
            &fvoci_model::BlockPatch {
                body: fvoci_model::BodyChange::Set(
                    fvoci_model::Body::parse(whole.clone())
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
    collab
        .write_body(&e.app, &e.card_id)
        .await
        .expect("the document's body is due");
    assert_eq!(
        body_of(&card(&th.client, &e.card_id).await),
        Some(whole.clone()),
        "not written over"
    );
    assert!(told(&mut events, &e.card_id, DocChange::Replaced));
    let (again, _) = joined(collab, &e.app, &e.card_id, 102).await;
    assert_eq!(
        again.body().map(|body| body.document().clone()),
        Some(whole)
    );
    th.teardown().await;
}

/// Only a card that the handle reaches has a document.
#[tokio::test]
async fn only_a_card_has_a_document() {
    let th = Setup::native().start().await.init_basic().await;
    let e = editing(&th, &paragraphs(&["first"])).await;
    let collab = e.app.collab();
    let unknown = collab
        .open(&e.app, crate::common::UNKNOWN_ID)
        .await
        .expect_err("no such card");
    assert_eq!(unknown.kind(), fvoci_server::error::ErrorKind::NotFound);
    th.teardown().await;
}

/// Collaborative table edits past 200 rows or 20 columns are refused over WebSocket with
/// `docNack` reason `table_too_large`, leaving the room document unpolluted for peers.
#[tokio::test]
async fn collab_table_limit_refuses_201_rows_and_21_columns_with_table_too_large() {
    use crate::rooms::{Peer, table as room_table};

    let th = Setup::native().start().await.init_basic().await;
    let cell = json!({"type": "tableCell", "content": [{"type": "paragraph"}]});
    let row_1 = json!({"type": "tableRow", "content": [&cell]});
    let max_rows_doc = json!({
        "type": "doc",
        "content": [{"type": "table", "content": vec![row_1; usize::from(fvoci_model::TABLE_MAX_ROWS)]}],
    });
    let row_20 = json!({
        "type": "tableRow",
        "content": vec![cell; usize::from(fvoci_model::TABLE_MAX_COLUMNS)],
    });
    let max_cols_doc = json!({
        "type": "doc",
        "content": [{"type": "table", "content": [&row_20, &row_20]}],
    });

    for (kind, initial_doc, add_row) in [
        ("201 rows", max_rows_doc, true),
        ("21 columns", max_cols_doc, false),
    ] {
        let t = room_table(&th, &initial_doc).await;
        let mut one = Peer::connect(&th.client, 201).await;
        one.joined(TEST_WORKSPACE_ID, &t.card_id).await;
        let bad_update = if add_row {
            one.editor().add_table_row(1)
        } else {
            one.editor().add_table_column()
        };
        one.send_update(&bad_update).await;
        let nack = one.text("docNack").await;
        assert_eq!(
            nack,
            json!({
                "type": "docNack",
                "doc": one.doc,
                "epoch": one.epoch,
                "reason": "table_too_large",
            }),
            "{kind}"
        );

        let mut two = Peer::connect(&th.client, 202).await;
        two.joined(TEST_WORKSPACE_ID, &t.card_id).await;
        assert_eq!(
            two.body().as_ref(),
            Some(&initial_doc),
            "{kind}: room document stays unpolluted"
        );
        two.write_acked("still editable").await;
    }
    th.teardown().await;
}

/// The application of a server as user1 reaches the test workspace.
pub(crate) async fn editing_app(th: &TestServer) -> Editing {
    let user1 = th.client.me().await.id;
    let user2 = th.client2.me().await.id;
    let entered = th
        .app
        .enter_workspace(
            &Session {
                user_id: user1.clone(),
                ..Session::default()
            },
            TEST_WORKSPACE_ID,
        )
        .await
        .expect("the membership is read")
        .expect("a member");
    Editing {
        app: entered.app,
        card_id: String::new(),
        user1,
        user2,
    }
}
