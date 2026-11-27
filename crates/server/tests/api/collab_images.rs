//! The files of the images and file blocks a body edited together shows, between the
//! update that shows them and the write of the body to the card. Both kinds of node
//! live by the same rules: each test runs for each.

use fvoci_collab::BodyDoc;
use fvoci_model::{Body, FileRecord, new_id};
use serde_json::{Value, json};

use crate::collab::{Editing, body_of, card, editing, joined};
use crate::common::{Setup, TestServer};

/// A file of the card's board, recorded long ago and shown by nothing.
async fn old_file(e: &Editing, th: &TestServer) -> FileRecord {
    let board_id = card(&th.client, &e.card_id).await.board_id;
    let file = FileRecord {
        id: new_id(),
        container_id: board_id.clone(),
        stored_name: format!("7{}.png", new_id()),
        created_at: 1,
        path: format!("boards/{board_id}/image.png"),
        name: "image.png".to_owned(),
        extension: ".png".to_owned(),
        size: 7,
        ..FileRecord::default()
    };
    e.app
        .store()
        .save_file_record(&file)
        .await
        .expect("a file is recorded");
    file
}

/// The kinds of node of a body that name a file.
pub(crate) const FILE_NODES: [&str; 2] = ["image", "file"];

/// An update that makes an empty document show an image of `stored_name`.
pub(crate) fn showing(stored_name: &str) -> Vec<u8> {
    showing_as("image", stored_name)
}

/// An update that makes an empty document show a node of `kind` naming `stored_name`.
pub(crate) fn showing_as(kind: &str, stored_name: &str) -> Vec<u8> {
    let body = Body::parse(json!({"type": "doc", "content": [
        {"type": kind, "attrs": {"fileId": stored_name}}
    ]}))
    .expect("a body")
    .expect("not empty");
    BodyDoc::from_body(Some(&body)).state()
}

/// An update that shows an image or a file block whose file the board does not have
/// (none, or another board's) is refused and not logged, and the document goes on:
/// what another editor typed is taken and written.
#[tokio::test]
async fn a_file_the_board_does_not_have_is_refused_and_the_document_goes_on() {
    let th = Setup::native().start().await.init_basic().await;
    let other = editing(&th, &Value::Null).await;
    let elsewhere = old_file(&other, &th).await.stored_name;
    for kind in FILE_NODES {
        for missing in ["7nothing.png", elsewhere.as_str()] {
            refused_and_going_on(&th, kind, missing).await;
        }
    }
    th.teardown().await;
}

async fn refused_and_going_on(th: &TestServer, kind: &str, missing: &str) {
    let e = editing(th, &Value::Null).await;
    let collab = e.app.collab();
    let (editor, epoch) = joined(collab, &e.app, &e.card_id, 101).await;
    let typed = editor.append_paragraph("typed");
    collab
        .take(&e.app, &e.card_id, epoch, &e.user1, &typed)
        .await
        .expect("taken");
    let refused = collab
        .take(
            &e.app,
            &e.card_id,
            epoch,
            &e.user2,
            &showing_as(kind, missing),
        )
        .await
        .expect_err("no file of the board");
    assert_eq!(
        refused.reason().map(fvoci_server::error::Reason::as_str),
        Some("update_refused"),
        "{kind}"
    );
    let stored = e
        .app
        .store()
        .get_body_doc(&e.card_id)
        .await
        .expect("read")
        .expect("a document");
    assert_eq!(stored.log_end, 1, "the refused update is not logged");
    collab
        .write_body(&e.app, &e.card_id)
        .await
        .expect("written");
    assert_eq!(
        body_of(&card(&th.client, &e.card_id).await),
        editor.body().map(|body| body.document().clone()),
        "what was typed before is written: {kind} of {missing}"
    );
}

/// An image or a file block shown by an update and deleted before the body is written
/// leaves its file shown by nothing once the body is written, as if it had never been
/// shown.
#[tokio::test]
async fn a_file_deleted_before_the_body_is_written_is_shown_no_more() {
    let th = Setup::native().start().await.init_basic().await;
    for kind in FILE_NODES {
        deleted_before_written(&th, kind).await;
    }
    th.teardown().await;
}

async fn deleted_before_written(th: &TestServer, kind: &str) {
    let e = editing(th, &Value::Null).await;
    let file = old_file(&e, th).await;
    let collab = e.app.collab();
    let (_, epoch) = joined(collab, &e.app, &e.card_id, 101).await;
    collab
        .take(
            &e.app,
            &e.card_id,
            epoch,
            &e.user1,
            &showing_as(kind, &file.stored_name),
        )
        .await
        .expect("taken");
    let held = e
        .app
        .store()
        .collect_unreferenced_files(i64::MAX)
        .await
        .expect("collected");
    assert!(
        held.iter().all(|collected| collected.id != file.id),
        "shown while its update waits to be written: {kind}"
    );
    let (editor, epoch) = joined(collab, &e.app, &e.card_id, 102).await;
    collab
        .take(&e.app, &e.card_id, epoch, &e.user1, &editor.delete_last())
        .await
        .expect("taken");
    collab
        .write_body(&e.app, &e.card_id)
        .await
        .expect("written");
    let collected = e
        .app
        .store()
        .collect_unreferenced_files(i64::MAX)
        .await
        .expect("collected");
    assert!(
        collected.iter().any(|collected| collected.id == file.id),
        "shown by nothing once the body is written: {kind} {collected:?}"
    );
}
