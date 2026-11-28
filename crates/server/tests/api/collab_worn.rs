//! Documents that many sessions wrote one after the other. Each session is a client
//! of the document, and the Y library reads the clients from the highest identifier
//! down: one whose changes name those of a client it has not read waits while that
//! one's are read, and the library looks through all that wait each time. A document
//! of thousands of such sessions is read in few steps until a session whose client is
//! read first writes after the last of them, and that update was refused. The
//! document is made again from its body before that: as it is closed with nobody in
//! its room, as it is opened, and under its editors if its room never empties.

use fvoci_collab::testing::Editor;
use fvoci_collab::{READING_STEPS, reading_steps, steps_ahead};
use fvoci_server::collab::DocChange;
use serde_json::{Value, json};

use crate::body_versions::versions;
use crate::collab::{Editing, body_of, card, editing, joined, paragraphs};
use crate::collab_slow::{Run, number, paragraphs_of, roomy, stored_before, update_of};
use crate::collab_written_past::stored;
use crate::common::{TEST_WS, TestServer};

/// The client of the first of the sessions that are read in the order they wrote:
/// each next one's is one below.
const FIRST: u64 = 1 << 40;
/// A client read before all of those.
const READ_FIRST: u64 = 1 << 41;
/// Where a document is made again as it is closed with nobody in its room: an eighth
/// of the steps a document may take to read.
const AT_REST: u64 = READING_STEPS / 8;

/// The changes of `sessions` sessions that each wrote `each` empty paragraphs after
/// the last of the session before, the first at the beginning of the body. The client
/// of each is one below that of the session before, so that each is read after those
/// it wrote after: the document is read in no steps.
fn one_after_the_other(sessions: u64, each: u64) -> Vec<Run> {
    const PARAGRAPH: &[u8] = b"\x03\x09paragraph";
    (0..sessions)
        .map(|session| {
            let client = FIRST - session;
            let mut changes = Vec::new();
            if session == 0 {
                // A shared type (7) under the root of that name.
                changes.extend_from_slice(b"\x07\x01\x07default");
            } else {
                // A shared type (7) after a neighbour (0x80): the last of the session
                // before.
                changes.push(0x87);
                number(&mut changes, client + 1);
                number(&mut changes, each - 1);
            }
            changes.extend_from_slice(PARAGRAPH);
            for clock in 1..each {
                changes.push(0x87);
                number(&mut changes, client);
                number(&mut changes, clock - 1);
                changes.extend_from_slice(PARAGRAPH);
            }
            Run {
                client,
                clock: 0,
                count: each,
                changes,
            }
        })
        .collect()
}

fn steps_of(state: &[u8]) -> u64 {
    reading_steps(state, u64::MAX).expect("a state")
}

fn ahead_of(state: &[u8]) -> u64 {
    steps_ahead(state).expect("a state")
}

/// A document stored before the steps one more client may leave it with were counted:
/// 6,000 sessions wrote eight paragraphs each, one after the other, and it is read in
/// a step. The first paragraph of a session whose client is read before the others
/// would leave it past the steps it may take to read, and was refused. The document
/// is made again from its body as it is opened, before anybody holds it, and the
/// update is taken; the body is what it was.
#[tokio::test]
async fn a_stored_document_of_thousands_of_sessions_is_made_again_before_an_update_is_refused() {
    const SESSIONS: u64 = 6_000;
    const EACH: u64 = 8;
    let th = roomy().await;
    let left = update_of(&one_after_the_other(SESSIONS, EACH));
    let (e, state) = stored_before(&th, None, &left).await;
    // A step for the first paragraph, which is at the beginning as the card's is.
    assert!(steps_of(&state) <= 1, "read in the order it was written");
    assert!(
        ahead_of(&state) > READING_STEPS,
        "one more session may leave it past its steps"
    );
    let with_one_more = Editor::join(READ_FIRST, &state);
    let _ = with_one_more.append_paragraph("read first");
    // All it holds: what one that holds nothing (a state vector of no client) lacks.
    assert!(
        steps_of(&with_one_more.missing_from(&[0])) > READING_STEPS,
        "and such a session does"
    );
    let before = stored(&e).await;
    let collab = e.app.collab();

    let (editor, epoch) = joined(collab, &e.app, &e.card_id, READ_FIRST).await;
    assert_eq!(epoch, before.epoch + 1, "made again as it was opened");
    let made = stored(&e).await;
    assert!(ahead_of(&made.state) < AT_REST, "a body's own document");
    let body = editor.body().map(|body| body.document().clone());
    // The card's paragraph, those of the sessions, and the one that was logged.
    let all = usize::try_from(SESSIONS * EACH).expect("a count fits") + 2;
    assert_eq!(paragraphs_of(body.as_ref()), all);
    assert_eq!(body_of(&card(&th.client, &e.card_id).await), body);
    collab
        .take(
            &e.app,
            &e.card_id,
            epoch,
            &e.user1,
            &editor.append_paragraph("read first"),
        )
        .await
        .expect("the update of a session read before the others is taken");
    th.teardown().await;
}

/// A document whose room never empties is made again under its editors, before an
/// update is refused: sessions wrote it one after the other, and one more client may
/// leave it with nearly three quarters of the steps it may take to read. Sessions
/// that are read before the others come into its room and write, none leaving. The
/// update that brings it to three quarters of those steps ahead is taken, and the
/// document made again from its body, which the room is told; none is refused, and
/// an editor that loads it again goes on.
#[tokio::test]
async fn a_document_whose_room_never_empties_is_made_again_before_an_update_is_refused() {
    const SESSIONS: u64 = 2_085;
    const EACH: u64 = 23;
    let th = roomy().await;
    let left = update_of(&one_after_the_other(SESSIONS, EACH));
    let (e, state) = stored_before(&th, None, &left).await;
    let ahead = ahead_of(&state);
    assert!(
        (READING_STEPS / 10 * 7..READING_STEPS / 4 * 3).contains(&ahead),
        "under three quarters of the steps: {ahead}"
    );
    let collab = e.app.collab();
    let mut events = collab.subscribe();
    let first_epoch = stored(&e).await.epoch;
    let mut written = 0_u64;
    let last = loop {
        let (editor, epoch) = joined(collab, &e.app, &e.card_id, READ_FIRST + written).await;
        assert_eq!(epoch, first_epoch, "not made again as it was opened");
        let taken = collab
            .take(
                &e.app,
                &e.card_id,
                epoch,
                &e.user1,
                &editor.append_paragraph("read first"),
            )
            .await
            .unwrap_or_else(|error| panic!("update {written} is refused: {error:?}"));
        written += 1;
        if taken.remade {
            break editor;
        }
        assert!(written < 40, "made again before {written} sessions wrote");
    };
    assert!(
        std::iter::from_fn(|| events.try_recv().ok())
            .any(|event| event.card_id == e.card_id && event.change == DocChange::Remade),
        "the room is told"
    );
    let made = stored(&e).await;
    assert_eq!(made.epoch, first_epoch + 1, "made again");
    assert!(ahead_of(&made.state) < AT_REST, "a body's own document");
    let (again, again_epoch) = joined(collab, &e.app, &e.card_id, 7).await;
    assert_eq!(
        again.body().map(|body| body.document().clone()),
        last.body().map(|body| body.document().clone()),
        "with what the last update left"
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
    th.teardown().await;
}

/// What a session does: it opens the document, writes `each` empty paragraphs after
/// the last one, and leaves, and the document is closed as its room is empty. The
/// editor as it left, and which making of the document it held.
async fn session(e: &Editing, client: u64, each: u64) -> (Editor, i64) {
    let collab = e.app.collab();
    let (editor, epoch) = joined(collab, &e.app, &e.card_id, client).await;
    let held = editor.state_vector();
    for _ in 0..each {
        let _ = editor.append_element("paragraph");
    }
    collab
        .take(
            &e.app,
            &e.card_id,
            epoch,
            &e.user1,
            &editor.missing_from(&held),
        )
        .await
        .unwrap_or_else(|error| panic!("the update of session {client} is refused: {error:?}"));
    collab.close(&e.app, &e.card_id).await.expect("closed");
    (editor, epoch)
}

/// The comment blocks of a card, as a client reads them.
async fn comments(th: &TestServer, board_id: &str, card_id: &str) -> Vec<Value> {
    th.client
        .get(&format!(
            "{TEST_WS}/boards/{board_id}/cards/{card_id}/comments"
        ))
        .await
        .ok_json()
}

/// Sessions that open a card's document one after the other, each writing `each`
/// paragraphs after the last and leaving the room empty, until the document was made
/// again from its body as it closed and a few sessions wrote into that one: no update
/// is refused, whichever client is read first, and after every close what is stored
/// is under the mark at rest. As it is made again the body is the one the last session
/// left, and the versions and the comments of the card are as they were. `written`
/// sessions wrote the document before; how many did here until it was made again.
async fn sessions_until_made_again(th: &TestServer, e: &Editing, written: u64, each: u64) -> u64 {
    let collab = e.app.collab();
    let board_id = card(&th.client, &e.card_id).await.board_id;
    th.client
        .post(
            &format!("{TEST_WS}/boards/{board_id}/cards/{}/comments", e.card_id),
            &json!({"body": paragraphs(&["a comment"])}),
        )
        .await
        .created_json::<Value>();
    let commented = comments(th, &board_id, &e.card_id).await;
    assert_eq!(commented.len(), 1, "the card has a comment");

    collab.open(&e.app, &e.card_id).await.expect("opened");
    collab.close(&e.app, &e.card_id).await.expect("closed");
    let first = stored(e).await;
    let mut ahead = ahead_of(&first.state);
    assert!(ahead < AT_REST, "under the mark at rest to begin with");
    let mut made_again_after = None;
    let mut sessions = 0_u64;
    let mut after = 0;
    while after < 3 {
        // The second session here and every thirty-first are read before all the
        // others, and the rest in the order they wrote: the steps of the reading
        // leap with those.
        let number = written + sessions;
        let client = if number % 31 == 30 || sessions == 1 {
            READ_FIRST + number
        } else {
            FIRST - number
        };
        // The versions are read where the document may be made again next.
        let kept = if made_again_after.is_none() && ahead >= AT_REST / 10 * 9 {
            Some(versions(&th.client, &e.card_id).await)
        } else {
            None
        };
        let (editor, _) = session(e, client, each).await;
        sessions += 1;
        let closed = stored(e).await;
        ahead = ahead_of(&closed.state);
        assert!(
            ahead < AT_REST,
            "after session {number} one more may leave it with {ahead} steps"
        );
        if made_again_after.is_some() {
            after += 1;
            continue;
        }
        if closed.epoch == first.epoch {
            continue;
        }
        made_again_after = Some(sessions);
        let left = editor.body().map(|body| body.document().clone());
        assert_eq!(
            body_of(&card(&th.client, &e.card_id).await),
            left,
            "the card holds what the session left"
        );
        let (again, _) = joined(collab, &e.app, &e.card_id, 7).await;
        assert_eq!(
            again.body().map(|body| body.document().clone()),
            left,
            "and so does the document made again"
        );
        collab.close(&e.app, &e.card_id).await.expect("closed");
        // The close kept a version as any does, and with it let go of the oldest
        // where the card has as many as it may: the others are as they were.
        let kept = kept.expect("read before the session that had it made again");
        assert!(!kept.is_empty(), "the card had versions before");
        let kept_now = versions(&th.client, &e.card_id).await;
        assert!(kept_now.len() >= kept.len(), "no fewer versions");
        let newest = kept
            .iter()
            .filter_map(|kept| kept["version"].as_i64())
            .max();
        for now in &kept_now {
            if now["version"].as_i64() > newest {
                continue;
            }
            let before = kept
                .iter()
                .find(|kept| kept["version"] == now["version"])
                .unwrap_or_else(|| panic!("version {} was there before", now["version"]));
            for field in ["keptAt", "editors"] {
                assert_eq!(now[field], before[field], "{field} of a version");
            }
        }
        assert_eq!(
            comments(th, &board_id, &e.card_id).await,
            commented,
            "the comment is as it was"
        );
    }
    made_again_after.expect("made again")
}

/// Sessions of a hundred paragraphs each, one after the other: the shape of 2,500
/// sessions of 100 changes, whose last update was refused. 405 of them wrote the
/// document that is stored, and the next ones write through the server, each opening
/// and closing it. It is made again from its body as it closes, long before an update
/// would be refused, and none is.
#[tokio::test]
async fn a_document_sessions_wrote_one_after_the_other_is_made_again_as_it_closes() {
    const WRITTEN: u64 = 405;
    const EACH: u64 = 100;
    let th = roomy().await;
    let left = update_of(&one_after_the_other(WRITTEN, EACH));
    let (e, state) = stored_before(&th, None, &left).await;
    let ahead = ahead_of(&state);
    assert!(
        (AT_REST / 10 * 9..AT_REST).contains(&ahead),
        "near the mark at rest: {ahead}"
    );
    let sessions = sessions_until_made_again(&th, &e, WRITTEN, EACH).await;
    assert!(sessions <= 6, "made again after {sessions} sessions");
    th.teardown().await;
}

/// The three shapes whose last update was refused (2,500 sessions of 100 changes,
/// 4,000 of 20, 8,000 of 8), written through the server from an empty card, each
/// session opening and closing the document, until it was made again as it closed:
/// after 410, 916 and 1,448 sessions. Minutes each in a debug build, so not run with
/// the others:
///
/// ```sh
/// cargo test -p fvoci-server --features test-util --test api -- --ignored written_from_the_first
/// ```
async fn written_from_the_first(each: u64, made_again_after: std::ops::Range<u64>) {
    let th = roomy().await;
    let e = editing(&th, &paragraphs(&["first"])).await;
    let sessions = sessions_until_made_again(&th, &e, 0, each).await;
    assert!(
        made_again_after.contains(&sessions),
        "made again after {sessions} sessions of {each}"
    );
    th.teardown().await;
}

#[tokio::test]
#[ignore = "minutes in a debug build"]
async fn sessions_of_a_hundred_changes_written_from_the_first() {
    written_from_the_first(100, 400..420).await;
}

#[tokio::test]
#[ignore = "minutes in a debug build"]
async fn sessions_of_twenty_changes_written_from_the_first() {
    written_from_the_first(20, 905..925).await;
}

#[tokio::test]
#[ignore = "minutes in a debug build"]
async fn sessions_of_eight_changes_written_from_the_first() {
    written_from_the_first(8, 1_435..1_460).await;
}
