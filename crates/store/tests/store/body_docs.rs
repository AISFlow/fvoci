//! The fence of a stored document: what writes the document, its log, or its body to
//! its card says what it read of the document ([`DocAsRead`]), and is refused once
//! another writer changed any of those numbers. The other writer is a server that
//! took the lock of the server while the one before had not noticed yet that it lost
//! it; here it is a write between a read and the write under test.

use fvoci_store::{BodyBefore, DocAsRead, Store, StoreError, StoredDoc, body_digest};

use crate::body_versions::{board_with_files, body_showing, card_with};
use crate::common;

/// The state the first document of the tests is stored with.
const FIRST_STATE: &[u8] = b"the first state";

/// A card with a document that logged one update, as its server read it.
async fn card_with_a_document(store: &Store) -> (String, StoredDoc) {
    let author = common::user(store, "kelly.kapoor").await;
    let board_id = board_with_files(store, &author, &[]).await;
    let card_id = card_with(store, &board_id, body_showing("one", &[]), &author).await;
    store
        .create_body_doc(&card_id, FIRST_STATE, b"a digest")
        .await
        .expect("a document");
    log(store, &card_id, b"the first update").await;
    let read = stored(store, &card_id).await;
    (card_id, read)
}

/// The numbers of a block's document as they are stored now: what a writer that reads
/// it just before it writes names.
pub(crate) async fn as_read(store: &Store, block_id: &str) -> DocAsRead {
    stored(store, block_id).await.as_read()
}

/// Logs an update of the server's own to the card's document, by a writer that read
/// the document just before.
async fn log(store: &Store, card_id: &str, update: &[u8]) {
    let now = as_read(store, card_id).await;
    append(store, card_id, &[update], now)
        .await
        .expect("logged");
}

/// Logs updates of the server's own by a writer that read the document as `as_read`.
async fn append(
    store: &Store,
    card_id: &str,
    updates: &[&[u8]],
    as_read: DocAsRead,
) -> Result<Vec<i64>, StoreError> {
    let entries: Vec<(&[u8], &str)> = updates.iter().map(|update| (*update, "")).collect();
    store
        .append_body_updates(card_id, &entries, as_read, &[], &[], BodyBefore::Leave)
        .await
}

async fn stored(store: &Store, card_id: &str) -> StoredDoc {
    store
        .get_body_doc(card_id)
        .await
        .expect("read")
        .expect("a document")
}

/// The updates the log holds, in order.
fn logged(doc: &StoredDoc) -> Vec<&[u8]> {
    doc.updates
        .iter()
        .map(|update| update.bytes.as_slice())
        .collect()
}

fn written_since(result: Result<impl std::fmt::Debug, StoreError>, card_id: &str) {
    match result {
        Err(StoreError::DocumentWritten { block_id }) => {
            assert_eq!(block_id, card_id, "the block the refusal names");
        }
        other => panic!("refused as written since it was read: {other:?}"),
    }
}

/// A server that reads a document once and then goes by what its own writes left is
/// let through by the fence at every write: it logs (two updates in one write are
/// numbered one after the other), writes the body to the card, folds, logs again, and
/// makes the document again, and never reads the document in between.
#[tokio::test]
async fn a_writer_that_keeps_count_of_its_own_writes_is_let_through() {
    let test = common::setup().await;
    let store = &test.store;
    let (card_id, read) = card_with_a_document(store).await;
    let mut known = read.as_read();

    let seqs = append(store, &card_id, &[b"an update", b"its repair"], known)
        .await
        .expect("logged");
    assert_eq!(seqs, [read.log_end + 1, read.log_end + 2]);
    known.log_end = seqs[1];

    let writer = common::user(store, "ryan.howard").await;
    store
        .project_body(
            &card_id,
            fvoci_store::Projection::Held,
            known.log_end,
            b"a digest",
            &writer,
            known,
        )
        .await
        .expect("the body is written to the card");
    known.projected_seq = known.log_end;

    store
        .fold_body_doc(&card_id, b"folded", known.log_end, known)
        .await
        .expect("folded");
    known.state_seq = known.log_end;
    let folded = stored(store, &card_id).await;
    assert_eq!(
        (folded.state.as_slice(), logged(&folded), folded.as_read()),
        (b"folded".as_slice(), vec![], known),
        "what the writer counted is what is stored"
    );

    let seqs = append(store, &card_id, &[b"one more"], known)
        .await
        .expect("logged after the fold");
    known.log_end = seqs[0];

    let epoch = store
        .remake_body_doc(&card_id, b"made again", b"its digest", known)
        .await
        .expect("made again");
    let remade = stored(store, &card_id).await;
    assert_eq!(epoch, read.epoch + 1);
    assert_eq!(
        (remade.state.as_slice(), logged(&remade), remade.as_read()),
        (
            b"made again".as_slice(),
            vec![],
            DocAsRead {
                epoch,
                state_seq: known.log_end,
                log_end: known.log_end,
                projected_seq: known.log_end,
            }
        ),
        "the numbers of the updates go on"
    );
    test.teardown().await;
}

/// Another server logged an update since the read: an update of the first is not
/// logged after one it never took, and nothing else of that write is done (who wrote
/// is not put down). The other's update stays the last.
#[tokio::test]
async fn an_update_is_not_logged_after_one_its_writer_never_saw() {
    let test = common::setup().await;
    let store = &test.store;
    let (card_id, read) = card_with_a_document(store).await;
    let editor = common::user(store, "ryan.howard").await;
    log(store, &card_id, b"the other server's").await;

    let appended = store
        .append_body_updates(
            &card_id,
            &[(b"built on less", &editor)],
            read.as_read(),
            &[],
            &[&editor],
            BodyBefore::Leave,
        )
        .await;

    written_since(appended, &card_id);
    let left = stored(store, &card_id).await;
    assert_eq!(
        (logged(&left), left.log_end, &left.unkept_editors),
        (
            vec![b"the first update".as_slice(), b"the other server's"],
            read.log_end + 1,
            &read.unkept_editors
        ),
        "nothing of the refused write is there"
    );
    test.teardown().await;
}

/// Another server logged an update between the read and the fold: the fold is refused,
/// and the state and the log stay as the other left them. The document read again
/// folds.
#[tokio::test]
async fn a_fold_of_a_document_that_logged_since_it_was_read_is_refused() {
    let test = common::setup().await;
    let store = &test.store;
    let (card_id, read) = card_with_a_document(store).await;
    log(store, &card_id, b"the other server's").await;

    let folded = store
        .fold_body_doc(&card_id, b"folded", read.log_end, read.as_read())
        .await;

    written_since(folded, &card_id);
    let left = stored(store, &card_id).await;
    assert_eq!(
        (left.state.as_slice(), left.state_seq, logged(&left)),
        (
            FIRST_STATE,
            0,
            vec![b"the first update".as_slice(), b"the other server's"]
        ),
        "nothing folded"
    );
    store
        .fold_body_doc(&card_id, b"folded", left.log_end, left.as_read())
        .await
        .expect("as read now, it folds");
    test.teardown().await;
}

/// Another server folded the log between the read and a fold that stores a state up
/// to where the first read it (a state repaired as the document is opened): the fold
/// is refused, though the log ends where it did and the making is the same. The
/// other's state, which holds the log it removed, is not put back behind. Nor does
/// the first log an update or make the document again: a reader between whose read
/// of the document and of its log the other folded holds neither the log nor the
/// state with it.
#[tokio::test]
async fn a_write_behind_a_fold_since_the_read_is_refused() {
    let test = common::setup().await;
    let store = &test.store;
    let (card_id, read) = card_with_a_document(store).await;
    store
        .fold_body_doc(
            &card_id,
            b"the other's, with the log",
            read.log_end,
            read.as_read(),
        )
        .await
        .expect("the other server folds");

    let folded = store
        .fold_body_doc(&card_id, b"repaired", read.state_seq, read.as_read())
        .await;
    let appended = append(store, &card_id, &[b"built on less"], read.as_read()).await;
    let remade = store
        .remake_body_doc(&card_id, b"made of less", b"its digest", read.as_read())
        .await;

    written_since(folded, &card_id);
    written_since(appended, &card_id);
    written_since(remade, &card_id);
    let left = stored(store, &card_id).await;
    assert_eq!(
        (left.state.as_slice(), left.as_read(), logged(&left)),
        (
            b"the other's, with the log".as_slice(),
            DocAsRead {
                state_seq: read.log_end,
                ..read.as_read()
            },
            vec![]
        ),
        "the state that holds the folded log stays"
    );
    test.teardown().await;
}

/// Another server logged an update between the read and the remake: the remake is
/// refused, and the log it would have emptied holds the other's update still, under
/// the same epoch.
#[tokio::test]
async fn a_remake_of_a_document_that_logged_since_it_was_read_is_refused() {
    let test = common::setup().await;
    let store = &test.store;
    let (card_id, read) = card_with_a_document(store).await;
    log(store, &card_id, b"the other server's").await;

    let remade = store
        .remake_body_doc(&card_id, b"made again", b"its digest", read.as_read())
        .await;

    written_since(remade, &card_id);
    let left = stored(store, &card_id).await;
    assert_eq!(
        (left.state.as_slice(), left.epoch, logged(&left)),
        (
            FIRST_STATE,
            read.epoch,
            vec![b"the first update".as_slice(), b"the other server's"]
        ),
        "the log is not emptied"
    );
    assert!(
        store.has_unwritten_body(&card_id).await.expect("read"),
        "what the card does not hold is still to write"
    );
    test.teardown().await;
}

/// Another server wrote the body of its document to the card since the read: to the
/// first the card looks written past the document, and it would make the document
/// again from that body, emptying a log that holds what the body does not. The remake
/// is refused, though the log ends where it did and the making is the same.
#[tokio::test]
async fn a_remake_after_another_wrote_the_body_to_the_card_is_refused() {
    let test = common::setup().await;
    let store = &test.store;
    let (card_id, read) = card_with_a_document(store).await;
    let writer = common::user(store, "ryan.howard").await;
    let theirs = body_showing("the other's", &[]);
    let digest = body_digest(theirs.as_ref());
    let read_at = store
        .get_block(&card_id)
        .await
        .expect("the card")
        .updated_at;
    store
        .project_body(
            &card_id,
            fvoci_store::Projection::Written {
                body: theirs,
                read_at,
            },
            read.log_end,
            &digest,
            &writer,
            read.as_read(),
        )
        .await
        .expect("the other server writes its body to the card");

    let remade = store
        .remake_body_doc(&card_id, b"made again", &digest, read.as_read())
        .await;

    written_since(remade, &card_id);
    let left = stored(store, &card_id).await;
    assert_eq!(
        (left.state.as_slice(), left.epoch, logged(&left)),
        (
            FIRST_STATE,
            read.epoch,
            vec![b"the first update".as_slice()]
        ),
        "the log is not emptied"
    );
    store
        .remake_body_doc(&card_id, b"made again", &digest, left.as_read())
        .await
        .expect("as read now, it is made again");
    test.teardown().await;
}

/// Another server logged an update that takes back what the first logged, and marked
/// the card as holding its body, which is the body the card held before either: the
/// card did not change, so nothing of the card tells the first that it was written
/// past. Its body, with what the other took back, is not written to the card.
#[tokio::test]
async fn a_body_is_not_written_to_a_card_the_other_holds_written() {
    let test = common::setup().await;
    let store = &test.store;
    let (card_id, read) = card_with_a_document(store).await;
    let writer = common::user(store, "ryan.howard").await;
    let held = store.get_block(&card_id).await.expect("the card");
    log(store, &card_id, b"the other's, taking the first back").await;
    let theirs = as_read(store, &card_id).await;
    let unchanged = body_digest(held.body.as_ref());
    store
        .project_body(
            &card_id,
            fvoci_store::Projection::Held,
            theirs.log_end,
            &unchanged,
            &writer,
            theirs,
        )
        .await
        .expect("the other server marks the card as holding its body");

    let read_at = store
        .get_block(&card_id)
        .await
        .expect("the card")
        .updated_at;
    let ours = body_showing("with the first update", &[]);
    let written = store
        .project_body(
            &card_id,
            fvoci_store::Projection::Written {
                body: ours.clone(),
                read_at,
            },
            read.log_end,
            &body_digest(ours.as_ref()),
            &writer,
            read.as_read(),
        )
        .await;

    written_since(written, &card_id);
    let left = store.get_block(&card_id).await.expect("the card");
    assert_eq!(
        (left.body, left.updated_at),
        (held.body, held.updated_at),
        "the card is as the other left it"
    );
    assert_eq!(
        as_read(store, &card_id).await,
        DocAsRead {
            projected_seq: theirs.log_end,
            ..theirs
        },
        "marked as written by the other alone"
    );
    test.teardown().await;
}

/// Writers that read the document at once and each log an update: one is let
/// through, and every other is refused, whichever comes first.
#[tokio::test]
async fn of_writers_that_read_the_same_only_one_logs() {
    /// How many write at once.
    const WRITERS: usize = 8;
    let test = common::setup().await;
    let store = &test.store;
    let (card_id, read) = card_with_a_document(store).await;
    let as_read = read.as_read();

    let writes: Vec<_> = (0..WRITERS)
        .map(|_| {
            let (store, card_id) = (store.clone(), card_id.clone());
            tokio::spawn(async move { append(&store, &card_id, &[b"at once"], as_read).await })
        })
        .collect();
    let mut logged_by = 0;
    for write in writes {
        match write.await.expect("the write ended") {
            Ok(seqs) => {
                assert_eq!(seqs, [read.log_end + 1], "numbered after the first");
                logged_by += 1;
            }
            refused => written_since(refused, &card_id),
        }
    }

    assert_eq!(logged_by, 1, "one writer");
    let left = stored(store, &card_id).await;
    assert_eq!(
        (logged(&left).len(), left.log_end),
        (2, read.log_end + 1),
        "one update after the first"
    );
    test.teardown().await;
}

/// Another server made the document again between the read and the write: an update,
/// a fold and a remake against the making before are refused, though the log ends
/// where it did.
#[tokio::test]
async fn a_write_of_a_document_made_again_since_it_was_read_is_refused() {
    let test = common::setup().await;
    let store = &test.store;
    let (card_id, read) = card_with_a_document(store).await;
    let theirs = store
        .remake_body_doc(&card_id, b"the other's", b"its digest", read.as_read())
        .await
        .expect("the other server makes it again");
    // What only the making tells apart: the other numbers as the remake left them.
    let before = DocAsRead {
        epoch: read.epoch,
        ..stored(store, &card_id).await.as_read()
    };

    let appended = append(store, &card_id, &[b"against the one before"], before).await;
    let folded = store
        .fold_body_doc(&card_id, b"folded", before.log_end, before)
        .await;
    let remade = store
        .remake_body_doc(&card_id, b"made again", b"another digest", before)
        .await;

    written_since(appended, &card_id);
    written_since(folded, &card_id);
    written_since(remade, &card_id);
    let left = stored(store, &card_id).await;
    assert_eq!(
        (left.state.as_slice(), left.epoch, logged(&left)),
        (b"the other's".as_slice(), theirs, vec![]),
        "the other's making stays, and its log is empty"
    );
    test.teardown().await;
}

/// A block without a document is not found, whatever was read: nothing is refused as
/// written since, and a body is not written to a card that has no document.
#[tokio::test]
async fn a_write_of_no_document_is_not_found() {
    let test = common::setup().await;
    let store = &test.store;
    let author = common::user(store, "kelly.kapoor").await;
    let board_id = board_with_files(store, &author, &[]).await;
    let card_id = card_with(store, &board_id, body_showing("one", &[]), &author).await;
    let as_read = DocAsRead {
        epoch: 1,
        state_seq: 0,
        log_end: 0,
        projected_seq: 0,
    };

    let held = store.get_block(&card_id).await.expect("the card");
    let body = body_showing("of no document", &[]);
    let written = store
        .project_body(
            &card_id,
            fvoci_store::Projection::Written {
                body: body.clone(),
                read_at: held.updated_at,
            },
            1,
            &body_digest(body.as_ref()),
            &author,
            as_read,
        )
        .await;
    let appended = append(store, &card_id, &[b"an update"], as_read).await;
    let folded = store.fold_body_doc(&card_id, b"folded", 0, as_read).await;
    let remade = store
        .remake_body_doc(&card_id, b"made again", b"a digest", as_read)
        .await;

    assert!(
        matches!(appended, Err(StoreError::NotFound(_))),
        "{appended:?}"
    );
    assert!(matches!(folded, Err(StoreError::NotFound(_))), "{folded:?}");
    assert!(matches!(remade, Err(StoreError::NotFound(_))), "{remade:?}");
    assert!(
        matches!(written, Err(StoreError::NotFound(_))),
        "{written:?}"
    );
    let left = store.get_block(&card_id).await.expect("the card");
    assert_eq!(
        (left.body, left.updated_at),
        (held.body, held.updated_at),
        "the card is as it was"
    );
    test.teardown().await;
}
