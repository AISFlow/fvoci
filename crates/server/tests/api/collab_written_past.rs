//! A server that another wrote past: one that lost the lock of the server learns it a
//! while later (`app/server_lock.rs`), and until then takes itself for the server. The
//! one that took the lock meanwhile writes the same documents. Every write of a stored
//! document says what its writer read of it, and the store refuses one that another
//! wrote past: the server then closes the document, and reads it again when it is next
//! asked for.
//!
//! The other server is the store written directly here, behind a document the server
//! of the test holds open and believes it alone writes.

use std::sync::Arc;

use fvoci_collab::testing::Editor;
use fvoci_model::Body;
use fvoci_server::collab::{DocChange, DocEvent, Loaded};
use fvoci_server::error::{AppError, Reason};
use fvoci_store::{BodyBefore, StoredDoc, body_digest};
use serde_json::{Value, json};
use tokio::sync::{Notify, broadcast};
use tokio::task::JoinHandle;

use crate::collab::{Editing, body_of, card, editing, paragraphs};
use crate::common::{Setup, TEST_WS, TestServer};

/// A card whose document the server holds open, with one update of its own (the
/// paragraph "ours") logged and written to the card, so that nothing waits to be
/// written a little later: the editing, the document as it was loaded, and the
/// update.
async fn ours_written(th: &TestServer) -> (Editing, Loaded, Vec<u8>) {
    let e = editing(th, &paragraphs(&["first"])).await;
    let collab = e.app.collab();
    let loaded = collab.open(&e.app, &e.card_id).await.expect("opened");
    let update = Editor::join(101, &loaded.state).append_paragraph("ours");
    collab
        .take(&e.app, &e.card_id, loaded.epoch, &e.user1, &update)
        .await
        .expect("taken");
    collab
        .write_body(&e.app, &e.card_id)
        .await
        .expect("the card holds ours");
    (e, loaded, update)
}

/// The other server logs an update (the paragraph "theirs") to the card's document,
/// which it read just before, behind the server of the test.
async fn logged_past(e: &Editing, loaded: &Loaded) {
    let update = Editor::join(202, &loaded.state).append_paragraph("theirs");
    let as_read = stored(e).await.as_read();
    e.app
        .store()
        .append_body_updates(
            &e.card_id,
            &[(&update, &e.user2)],
            as_read,
            &[],
            &[&e.user2],
            BodyBefore::Leave,
        )
        .await
        .expect("logged by the other server");
}

pub(crate) async fn stored(e: &Editing) -> StoredDoc {
    e.app
        .store()
        .get_body_doc(&e.card_id)
        .await
        .expect("read")
        .expect("a document")
}

/// The numbers of the updates the log holds.
fn logged(doc: &StoredDoc) -> Vec<i64> {
    doc.updates.iter().map(|update| update.seq).collect()
}

/// The texts of the paragraphs of a card's body, sorted: editors that wrote at once
/// are in an order their identifiers decide.
fn texts(body: Option<Value>) -> Vec<String> {
    let body = body.expect("a body");
    let mut texts: Vec<String> = body["content"]
        .as_array()
        .expect("blocks")
        .iter()
        .map(|block| {
            block["content"][0]["text"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
        .collect();
    texts.sort();
    texts
}

/// What the rooms of the card were told since `events` was subscribed.
fn told(events: &mut broadcast::Receiver<DocEvent>, e: &Editing) -> Vec<DocChange> {
    std::iter::from_fn(|| events.try_recv().ok())
        .filter(|event| event.card_id == e.card_id)
        .map(|event| event.change)
        .collect()
}

/// An update of the server's own is not logged after one of the other's it never
/// took: it is refused as `document_unavailable` and is in neither the log nor the
/// document, which the server reads again, telling the room. An editor that loads it
/// then has what both wrote, its update is logged after the other's, and the card is
/// given all of it.
#[tokio::test]
async fn an_update_is_refused_after_one_logged_past_the_server() {
    let th = Setup::native().start().await.init_basic().await;
    let (e, loaded, _) = ours_written(&th).await;
    let collab = e.app.collab();
    logged_past(&e, &loaded).await;
    let mut events = collab.subscribe();

    let late = Editor::join(103, &loaded.state).append_paragraph("late");
    let refused = collab
        .take(&e.app, &e.card_id, loaded.epoch, &e.user1, &late)
        .await
        .expect_err("the log does not end where the server left it");

    assert_eq!(refused.reason(), Some(Reason::DocumentUnavailable));
    assert_eq!(logged(&stored(&e).await), [1, 2], "not logged");
    assert_eq!(told(&mut events, &e), [DocChange::Reopened]);
    let again = collab.open(&e.app, &e.card_id).await.expect("read again");
    assert_eq!(again.epoch, loaded.epoch, "the same making");
    let editor = Editor::join(104, &again.state);
    let taken = collab
        .take(
            &e.app,
            &e.card_id,
            again.epoch,
            &e.user1,
            &editor.append_paragraph("after both"),
        )
        .await
        .expect("taken by the document as it is stored");
    assert_eq!(taken.seq, 3, "after the other's");
    collab.close(&e.app, &e.card_id).await.expect("closed");
    let after = stored(&e).await;
    assert_eq!(
        (after.state_seq, logged(&after)),
        (3, vec![]),
        "folded by the server that read all of it"
    );
    assert_eq!(
        texts(body_of(&card(&th.client, &e.card_id).await)),
        ["after both", "first", "ours", "theirs"]
    );
    th.teardown().await;
}

/// The document closes without folding its log: the state stays the one stored
/// before, and the log holds the other's update after the server's own.
#[tokio::test]
async fn a_document_written_past_is_not_folded_as_it_closes() {
    let th = Setup::native().start().await.init_basic().await;
    let (e, loaded, _) = ours_written(&th).await;
    logged_past(&e, &loaded).await;
    let before = stored(&e).await;
    assert_eq!((before.state_seq, logged(&before)), (0, vec![1, 2]));

    e.app
        .collab()
        .close(&e.app, &e.card_id)
        .await
        .expect("nothing of its own is left to write, and it closes");

    let after = stored(&e).await;
    assert_eq!(
        (after.epoch, after.state_seq, &after.state, &after.updates),
        (loaded.epoch, 0, &before.state, &before.updates),
        "neither folded nor made again"
    );
    th.teardown().await;
}

/// A body written whole to the card makes the open document again from it, which
/// empties the log. A server that the other logged past does not empty a log it has
/// not read: the store refuses, the room is told that the document is closed and
/// never that it was made again, and the other's update is still logged under the
/// making it was logged under. (The card holds what was written to it, as a body
/// written whole is to: whoever opens the document next makes it again from that.)
#[tokio::test]
async fn a_document_written_past_is_not_made_again() {
    let th = Setup::native().start().await.init_basic().await;
    let (e, loaded, _) = ours_written(&th).await;
    let collab = e.app.collab();
    logged_past(&e, &loaded).await;
    let mut events = collab.subscribe();
    let read = card(&th.client, &e.card_id).await;
    let whole = paragraphs(&["written whole"]);

    th.client
        .patch(
            &format!("{TEST_WS}/cards/{}", e.card_id),
            &json!({"body": whole, "readAt": read.updated_at}),
        )
        .await
        .check_ok();

    assert_eq!(body_of(&card(&th.client, &e.card_id).await), Some(whole));
    let told = told(&mut events, &e);
    assert!(
        told.contains(&DocChange::Closed) && !told.contains(&DocChange::Replaced),
        "the room is told to open the document anew, not that it was made again: {told:?}"
    );
    let after = stored(&e).await;
    assert_eq!(
        (after.epoch, logged(&after)),
        (loaded.epoch, vec![1, 2]),
        "the log is not emptied by the server that had not read it"
    );
    th.teardown().await;
}

/// A write of the server's own that is under way when the other server writes: one
/// update is logged and not yet written to the card, and the write of it has read the
/// card and waits before it writes it. Whichever write comes first waits: the one
/// asked for, or the one an update is followed by a little later. What comes back:
/// the editing, the document as it was loaded, the body with the update, and what to
/// notify to let the write go on and to await its end.
async fn a_write_under_way(
    th: &TestServer,
) -> (
    Editing,
    Loaded,
    Option<Body>,
    Arc<Notify>,
    JoinHandle<Result<(), AppError>>,
) {
    let (e, loaded, ours) = ours_written(th).await;
    let collab = e.app.collab();
    let (waiting, go) = collab.pause_next_projection();
    let editor = Editor::join(103, &loaded.state);
    editor.receive(&ours);
    let since = editor.append_paragraph("since");
    collab
        .take(&e.app, &e.card_id, loaded.epoch, &e.user1, &since)
        .await
        .expect("taken");
    let write = tokio::spawn({
        let (collab, app, card_id) = (collab.clone(), e.app.clone(), e.card_id.clone());
        async move { collab.write_body(&app, &card_id).await }
    });
    waiting.notified().await;
    (e, loaded, editor.body(), go, write)
}

/// The other server writes the body of the document to the card while the server is
/// about to: the server read the card before, finds it changed as it writes, takes it
/// for written past the document, and would make the document again from it. That
/// remake is refused, as the document is not as the server read it; the document is
/// closed, not made again, and the room is told to load it anew. The card holds the
/// body the other wrote, and the log what both hold.
#[tokio::test]
async fn a_card_the_other_server_wrote_does_not_make_the_document_again() {
    let th = Setup::native().start().await.init_basic().await;
    let (e, loaded, body, go, write) = a_write_under_way(&th).await;
    let mut events = e.app.collab().subscribe();
    let read = card(&th.client, &e.card_id).await;
    let as_read = stored(&e).await.as_read();
    e.app
        .store()
        .project_body(
            &e.card_id,
            fvoci_store::Projection::Written {
                body: body.clone(),
                read_at: read.updated_at,
            },
            as_read.log_end,
            &body_digest(body.as_ref()),
            &e.user2,
            as_read,
        )
        .await
        .expect("written by the other server");

    go.notify_one();
    // Refused, if it is the write that waited; nothing left to write, if the other was.
    let _ = write.await.expect("the write ended");

    assert_eq!(told(&mut events, &e), [DocChange::Reopened]);
    let after = stored(&e).await;
    assert_eq!(
        (after.epoch, logged(&after), after.projected_seq),
        (loaded.epoch, vec![1, 2], 2),
        "not made again, and written as far as the other wrote it"
    );
    assert_eq!(
        texts(body_of(&card(&th.client, &e.card_id).await)),
        ["first", "ours", "since"]
    );
    th.teardown().await;
}

/// The other server marks the card as holding the body of its document, which is the
/// body the card holds already (what it logged took back what the server logged),
/// while the server is about to write its own: the card did not change, so nothing of
/// the card tells the server. Its write is refused all the same, as the document is
/// not as it read it: the card is left as it was, the document is closed, and the
/// room is told to load it anew.
#[tokio::test]
async fn a_body_is_not_written_to_a_card_the_other_server_holds_written() {
    let th = Setup::native().start().await.init_basic().await;
    let (e, loaded, _, go, write) = a_write_under_way(&th).await;
    let mut events = e.app.collab().subscribe();
    let held = card(&th.client, &e.card_id).await;
    logged_past(&e, &loaded).await;
    let as_read = stored(&e).await.as_read();
    let unchanged = body_digest(held.body.as_ref());
    e.app
        .store()
        .project_body(
            &e.card_id,
            fvoci_store::Projection::Held,
            as_read.log_end,
            &unchanged,
            &e.user2,
            as_read,
        )
        .await
        .expect("marked as written by the other server");

    go.notify_one();
    // Refused, if it is the write that waited; nothing left to write, if the other was.
    let _ = write.await.expect("the write ended");

    assert_eq!(told(&mut events, &e), [DocChange::Reopened]);
    let left = card(&th.client, &e.card_id).await;
    assert_eq!(
        (texts(body_of(&left)), left.updated_at),
        (vec!["first".to_owned(), "ours".to_owned()], held.updated_at),
        "the card is as the other left it"
    );
    let after = stored(&e).await;
    assert_eq!(
        (after.epoch, logged(&after), after.projected_seq),
        (loaded.epoch, vec![1, 2, 3], 3),
        "written as far as the other marked it, and nothing made again"
    );
    th.teardown().await;
}
