//! Documents made slow to read: changes that each walk a long list every time the Y
//! library reads the document from its state. An update that leaves a valid body past
//! the steps it may take to read (including one repaired in place) is logged, written
//! to the card, and the document made again from that body so the update is kept, as
//! one near the steps it may take is, and one stored before the steps were counted is
//! opened without ending a worker.

use std::time::Duration;

use fvoci_collab::testing::Editor;
use fvoci_collab::{BodyDoc, READING_STEPS, UpdateLimits, reading_steps};
use fvoci_model::ContainerVisibility;
use fvoci_server::collab::DocChange;
use fvoci_store::{BodyBefore, body_digest};
use serde_json::{Value, json};

use crate::body_versions::versions;
use crate::collab::{Editing, body_of, card, editing, editing_app, joined, paragraphs};
use crate::collab_written_past::stored;
use crate::common::{Setup, TEST_WORKSPACE_ID, TEST_WS, TestServer};
use crate::notifications::{board_with_owner, card_with, give, kinds, told};
use crate::rooms::Peer;

/// The client that types the list, and the one whose changes walk it.
const TYPIST: u64 = 7;
const WALKER: u64 = 1 << 40;

pub(crate) fn number(out: &mut Vec<u8>, mut number: u64) {
    while number >= 0x80 {
        out.push(number.to_le_bytes()[0] | 0x80);
        number >>= 7;
    }
    out.push(number.to_le_bytes()[0]);
}

/// The changes of one client from `clock` on in an update, as the Y library writes
/// them (version 1).
pub(crate) struct Run {
    pub(crate) client: u64,
    pub(crate) clock: u64,
    pub(crate) count: u64,
    pub(crate) changes: Vec<u8>,
}

/// An update of the changes of clients, with nothing deleted.
pub(crate) fn update_of(runs: &[Run]) -> Vec<u8> {
    let mut out = Vec::new();
    number(&mut out, u64::try_from(runs.len()).expect("a count fits"));
    for run in runs {
        for value in [run.count, run.client, run.clock] {
            number(&mut out, value);
        }
        out.extend_from_slice(&run.changes);
    }
    number(&mut out, 0);
    out
}

/// `children` empty paragraphs typed one after the other by one client, the first as
/// a child of the root: each names the one before it to its left.
fn typing(children: u64) -> Run {
    const PARAGRAPH: &[u8] = b"\x03\x09paragraph";
    let mut changes = Vec::new();
    // A shared type (7) under the root of that name.
    changes.extend_from_slice(b"\x07\x01\x07default");
    changes.extend_from_slice(PARAGRAPH);
    for clock in 1..children {
        // A shared type (7) after a neighbour (0x80).
        changes.push(0x87);
        number(&mut changes, TYPIST);
        number(&mut changes, clock - 1);
        changes.extend_from_slice(PARAGRAPH);
    }
    Run {
        client: TYPIST,
        clock: 0,
        count: children,
        changes,
    }
}

fn typed_in_order(children: u64) -> Vec<u8> {
    update_of(&[typing(children)])
}

/// `count` changes from `clock` on, each deleted content of one place (1) that names
/// the last of `children` typed paragraphs to its right (0x40) and nothing to its left:
/// the library walks the whole list to place each, every time it reads the document.
fn walkers(clock: u64, count: u64, children: u64) -> Run {
    let mut changes = Vec::new();
    for _ in 0..count {
        changes.push(0x41);
        number(&mut changes, TYPIST);
        number(&mut changes, children - 1);
        number(&mut changes, 1);
    }
    Run {
        client: WALKER,
        clock,
        count,
        changes,
    }
}

fn walking(clock: u64, count: u64, children: u64) -> Vec<u8> {
    update_of(&[walkers(clock, count, children)])
}

/// Walking changes plus one invalid element (`nothing-a-body-has`) so applying it
/// crosses `READING_STEPS` and also requires a repair.
fn repair_needed_walkers(past: u64, children: u64) -> Run {
    let mut run = walkers(0, past, children);
    run.count += 1;
    run.changes.push(0x87);
    number(&mut run.changes, TYPIST);
    number(&mut run.changes, children - 1);
    run.changes.extend_from_slice(b"\x03\x12nothing-a-body-has");
    run
}

/// A server that takes an update large enough to type a long list in.
pub(crate) async fn roomy() -> TestServer {
    Setup::native()
        .configure(|config| config.collab.collab_update_bytes = 1024 * 1024)
        .start()
        .await
        .init_basic()
        .await
}

pub(crate) fn paragraphs_of(body: Option<&Value>) -> usize {
    body.and_then(|body| body["content"].as_array())
        .map_or(0, Vec::len)
}

/// An update that leaves a document at three quarters of the steps it may take to
/// read is taken, and the document is made again from its body, which takes none: the
/// room is told, and an editor that loads it again goes on.
#[tokio::test]
async fn a_document_that_takes_many_steps_to_read_is_made_again_from_its_body() {
    const CHILDREN: u64 = 10_000;
    let th = roomy().await;
    let e = editing(&th, &paragraphs(&["kept"])).await;
    let collab = e.app.collab();
    let mut events = collab.subscribe();
    let loaded = collab.open(&e.app, &e.card_id).await.expect("opened");
    let take = async |update: Vec<u8>| {
        collab
            .take(&e.app, &e.card_id, loaded.epoch, &e.user1, &update)
            .await
    };
    let typed = take(typed_in_order(CHILDREN)).await.expect("typed");
    assert!(!typed.remade, "a list typed in order takes no steps");

    let near = READING_STEPS * 3 / 4 / CHILDREN + 1;
    let taken = take(walking(0, near, CHILDREN)).await.expect("taken");
    assert!(taken.remade, "made again");
    assert!(
        std::iter::from_fn(|| events.try_recv().ok())
            .any(|event| event.card_id == e.card_id && event.change == DocChange::Remade),
        "the room is told"
    );
    let made = stored(&e).await;
    assert_eq!(
        reading_steps(&made.state, u64::MAX).expect("a state"),
        0,
        "a body's own document"
    );
    let (again, again_epoch) = joined(collab, &e.app, &e.card_id, 102).await;
    let body = again.body().map(|body| body.document().clone());
    assert_eq!(paragraphs_of(body.as_ref()), 10_001);
    assert_eq!(body_of(&card(&th.client, &e.card_id).await), body);
    let on = collab
        .take(
            &e.app,
            &e.card_id,
            again_epoch,
            &e.user1,
            &again.append_paragraph("on"),
        )
        .await
        .expect("an editor that loaded it again goes on");
    assert!(!on.remade, "not made again at every update");
    th.teardown().await;
}

/// An update that leaves a valid body past the steps a document may take to read is
/// taken, logged, written to its card, and the document made again from that body so
/// the update is kept — both without repair and when repaired in place (whether on a
/// freshly made document or an edited one).
#[tokio::test]
async fn a_document_whose_update_is_past_its_steps_is_taken_and_made_again_from_its_body() {
    const CHILDREN: u64 = 10_000;
    let past = READING_STEPS / CHILDREN + 5;
    let th = roomy().await;

    // 1. Past the steps without repair on a freshly made document.
    let e = editing(&th, &paragraphs(&["kept"])).await;
    let collab = e.app.collab();
    let mut events = collab.subscribe();
    let loaded = collab.open(&e.app, &e.card_id).await.expect("opened");
    let both = update_of(&[walkers(0, past, CHILDREN), typing(CHILDREN)]);
    let taken = collab
        .take(&e.app, &e.card_id, loaded.epoch, &e.user1, &both)
        .await
        .expect("past the steps without repair is taken");
    assert!(taken.remade, "made again with the update");
    assert!(taken.repair.is_none(), "no repair needed");
    assert!(
        std::iter::from_fn(|| events.try_recv().ok())
            .any(|ev| ev.card_id == e.card_id && ev.change == DocChange::Remade),
        "the room is told"
    );
    let made = stored(&e).await;
    assert_eq!(made.epoch, loaded.epoch + 1, "made again");
    assert_eq!(reading_steps(&made.state, u64::MAX).expect("state"), 0);
    let (again, again_epoch) = joined(collab, &e.app, &e.card_id, 102).await;
    assert_eq!(again_epoch, made.epoch, "rejoined at the remade epoch");
    let body = again.body().map(|body| body.document().clone());
    assert_eq!(paragraphs_of(body.as_ref()), 10_001);
    assert_eq!(body_of(&card(&th.client, &e.card_id).await), body);
    let on = collab
        .take(
            &e.app,
            &e.card_id,
            again_epoch,
            &e.user1,
            &again.append_paragraph("on"),
        )
        .await
        .expect("an editor that loaded it again goes on");
    assert!(!on.remade, "not made again at every update");

    // 2. Past the steps WITH an in-place repair on a freshly made document.
    let e_fresh = editing(&th, &paragraphs(&["kept"])).await;
    assert_repaired_past_steps_remakes(
        &th,
        &e_fresh,
        None,
        update_of(&[repair_needed_walkers(past, CHILDREN), typing(CHILDREN)]),
        10_001,
    )
    .await;

    // 3. Past the steps WITH an in-place repair on an edited document (`state_bytes > made_bytes`).
    let e_edited = editing(&th, &paragraphs(&["kept"])).await;
    assert_repaired_past_steps_remakes(
        &th,
        &e_edited,
        Some(typed_in_order(CHILDREN)),
        update_of(&[repair_needed_walkers(past, CHILDREN)]),
        10_001,
    )
    .await;

    th.teardown().await;
}

async fn assert_repaired_past_steps_remakes(
    th: &TestServer,
    e: &Editing,
    prefix: Option<Vec<u8>>,
    heavy_with_repair: Vec<u8>,
    expected_paragraphs: usize,
) {
    let collab = e.app.collab();
    let mut events = collab.subscribe();
    let loaded = collab.open(&e.app, &e.card_id).await.expect("opened");
    if let Some(first) = prefix {
        let taken = collab
            .take(&e.app, &e.card_id, loaded.epoch, &e.user1, &first)
            .await
            .expect("prefix taken");
        assert!(!taken.remade, "prefix stays settled");
    }
    let taken = collab
        .take(
            &e.app,
            &e.card_id,
            loaded.epoch,
            &e.user1,
            &heavy_with_repair,
        )
        .await
        .expect("past the steps with repair is taken and remade");
    assert!(taken.remade, "remade after in-place repair");
    assert!(taken.repair.is_some(), "repair update returned");
    assert!(
        std::iter::from_fn(|| events.try_recv().ok())
            .any(|ev| ev.card_id == e.card_id && ev.change == DocChange::Remade),
        "the room is told"
    );
    let after = stored(e).await;
    assert_eq!(after.epoch, loaded.epoch + 1, "made again");
    assert_eq!(
        reading_steps(&after.state, u64::MAX).expect("state"),
        0,
        "remade after repair takes 0 steps"
    );
    assert_eq!(
        paragraphs_of(body_of(&card(&th.client, &e.card_id).await).as_ref()),
        expected_paragraphs,
        "all valid paragraphs preserved and unknown node deleted"
    );
}

/// A card whose document is stored as updates that were taken before the steps of a
/// reading were counted left it: a list of `CHILDREN` paragraphs typed in order, which
/// its card holds, `walkers` changes that walk the list, and after that state a logged
/// update of one more paragraph, which its card does not hold. No worker runs.
async fn stored_slow(th: &TestServer, walkers: u64) -> (Editing, Vec<u8>) {
    let typed = typed_in_order(CHILDREN);
    stored_before(th, Some(&typed), &walking(0, walkers, CHILDREN)).await
}

/// A card whose document is stored as a server before this one left it: what `typed`
/// wrote through this server, which its card holds, then `left`, an update taken with
/// no count of clients or steps, and after that state a logged update of one more
/// paragraph, which its card does not hold. No worker runs.
pub(crate) async fn stored_before(
    th: &TestServer,
    typed: Option<&[u8]>,
    left: &[u8],
) -> (Editing, Vec<u8>) {
    let uncounted = UpdateLimits {
        update_bytes: 1024 * 1024,
        state_bytes: 16 * 1024 * 1024,
        body_bytes: fvoci_model::BODY_MAX_BYTES,
        placing_steps: None,
        clients: None,
        client_changes: None,
        total_changes: None,
        reading_steps: None,
    };
    let e = editing(th, &paragraphs(&["first"])).await;
    let collab = e.app.collab();
    let loaded = collab.open(&e.app, &e.card_id).await.expect("opened");
    if let Some(typed) = typed {
        collab
            .take(&e.app, &e.card_id, loaded.epoch, &e.user1, typed)
            .await
            .expect("typed");
    }
    collab.close(&e.app, &e.card_id).await.expect("written");
    collab.abandon().await;
    let before = stored(&e).await;
    assert_eq!(
        before.projected_seq, before.log_end,
        "the card holds the list"
    );
    let state = BodyDoc::from_state(&before.state, &uncounted)
        .expect("read")
        .doc
        .take(left, &uncounted)
        .expect("taken before the steps were counted")
        .doc
        .state();
    let store = e.app.store();
    store
        .fold_body_doc(&e.card_id, &state, before.state_seq, before.as_read())
        .await
        .expect("stored past the server");
    let late = Editor::join(104, &state).append_paragraph("typed late");
    store
        .append_body_updates(
            &e.card_id,
            &[(late.as_slice(), e.user1.as_str())],
            before.as_read(),
            &[],
            &[],
            BodyBefore::Leave,
        )
        .await
        .expect("logged past the server");
    (e, state)
}

/// The paragraphs the documents stored slow hold ([`stored_slow`]).
const CHILDREN: u64 = 20_000;

/// Starts the first worker of a server, by a document opened in it; its process.
async fn worker_of(th: &TestServer, e: &Editing) -> u32 {
    let other = editing(th, &paragraphs(&["other"])).await;
    let collab = e.app.collab();
    collab
        .open(&other.app, &other.card_id)
        .await
        .expect("opened");
    collab.worker_pids().await[0].expect("a worker runs")
}

fn last_text(body: Option<&Value>) -> Option<&str> {
    body?["content"].as_array()?.last()?["content"][0]["text"].as_str()
}

/// A document stored before the steps of a reading were counted, which takes more
/// than three quarters of the steps a document may take to read and no more than
/// them, is opened with all it holds: its body, with the update its card did not hold yet, is
/// written to the card, and it is made again from that body. The worker that opened it
/// runs on.
#[tokio::test]
async fn a_stored_document_that_takes_many_steps_to_read_is_written_and_made_again() {
    let th = roomy().await;
    let (e, state) = stored_slow(&th, READING_STEPS * 9 / 10 / CHILDREN).await;
    let steps = reading_steps(&state, u64::MAX).expect("a state");
    assert!(
        steps * 4 > READING_STEPS * 3 && steps <= READING_STEPS,
        "{steps} steps"
    );
    let before = stored(&e).await;
    let collab = e.app.collab();
    let worker = worker_of(&th, &e).await;
    let mut events = collab.subscribe();

    let (again, again_epoch) = joined(collab, &e.app, &e.card_id, 102).await;
    assert_eq!(
        collab.worker_pids().await[0],
        Some(worker),
        "the worker runs on"
    );
    let body = again.body().map(|body| body.document().clone());
    assert_eq!(paragraphs_of(body.as_ref()), 20_002);
    assert_eq!(last_text(body.as_ref()), Some("typed late"));
    assert_eq!(body_of(&card(&th.client, &e.card_id).await), body);
    assert!(
        std::iter::from_fn(|| events.try_recv().ok())
            .any(|event| event.card_id == e.card_id && event.change == DocChange::Remade),
        "the room is told"
    );
    let made = stored(&e).await;
    assert_eq!(made.epoch, again_epoch);
    assert_eq!(made.epoch, before.epoch + 1, "made again");
    assert_eq!(reading_steps(&made.state, u64::MAX).expect("a state"), 0);
    th.teardown().await;
}

/// A document stored before the steps of a reading were counted that would take more
/// of them than a document may is not read: it is made again from its card's
/// body, without the update the card did not hold, and the log after it is dropped.
/// The worker it was opened in runs on, and the document is edited.
#[tokio::test]
async fn a_stored_document_past_the_steps_it_may_take_is_made_again_from_its_card() {
    let th = roomy().await;
    let (e, state) = stored_slow(&th, READING_STEPS / CHILDREN + 8).await;
    let steps = reading_steps(&state, u64::MAX).expect("a state");
    assert!(steps > READING_STEPS, "{steps} steps");
    let before = stored(&e).await;
    let collab = e.app.collab();
    let worker = worker_of(&th, &e).await;

    let (again, again_epoch) = joined(collab, &e.app, &e.card_id, 102).await;
    assert_eq!(
        collab.worker_pids().await[0],
        Some(worker),
        "the worker runs on"
    );
    let body = again.body().map(|body| body.document().clone());
    assert_eq!(paragraphs_of(body.as_ref()), 20_001, "the card's body");
    assert_eq!(body_of(&card(&th.client, &e.card_id).await), body);
    let made = stored(&e).await;
    assert_eq!(made.epoch, before.epoch + 1, "made again");
    assert_eq!(
        (made.updates.len(), made.projected_seq),
        (0, made.log_end),
        "the log is dropped"
    );
    assert_eq!(reading_steps(&made.state, u64::MAX).expect("a state"), 0);
    collab
        .take(
            &e.app,
            &e.card_id,
            again_epoch,
            &e.user1,
            &again.append_paragraph("on"),
        )
        .await
        .expect("edited");
    th.teardown().await;
}

/// Updates that were logged and together leave a valid body past the steps it may take
/// to read are replayed with `must_remake` set, written to the card, and the document
/// made again from that body without losing the logged tail; the worker runs on.
#[tokio::test]
async fn logged_updates_that_leave_a_document_past_its_steps_are_kept_and_made_again() {
    let th = roomy().await;
    let e = editing(&th, &paragraphs(&["first"])).await;
    let collab = e.app.collab();
    collab.open(&e.app, &e.card_id).await.expect("opened");
    collab.abandon().await;
    let each = READING_STEPS / CHILDREN / 2 + 8;
    let updates = [
        typed_in_order(CHILDREN),
        walking(0, each, CHILDREN),
        walking(each, each, CHILDREN),
    ];
    let entries: Vec<(&[u8], &str)> = updates
        .iter()
        .map(|update| (update.as_slice(), e.user1.as_str()))
        .collect();
    let before = stored(&e).await;
    e.app
        .store()
        .append_body_updates(
            &e.card_id,
            &entries,
            before.as_read(),
            &[],
            &[],
            BodyBefore::Leave,
        )
        .await
        .expect("logged past the server");
    let worker = worker_of(&th, &e).await;

    let (again, _) = joined(collab, &e.app, &e.card_id, 102).await;
    assert_eq!(
        collab.worker_pids().await[0],
        Some(worker),
        "the worker runs on"
    );
    let body = again.body().map(|body| body.document().clone());
    assert_eq!(
        paragraphs_of(body.as_ref()),
        20_001,
        "all logged updates are kept"
    );
    assert_eq!(body_of(&card(&th.client, &e.card_id).await), body);
    let made = stored(&e).await;
    assert_eq!(made.epoch, before.epoch + 1, "made again");
    assert_eq!(made.updates.len(), 0, "the log is cleared by remake");
    assert_eq!(reading_steps(&made.state, u64::MAX).expect("a state"), 0);
    th.teardown().await;
}

/// Builds an update that takes a freshly typed list of `children` paragraphs past
/// `READING_STEPS` while also inserting a paragraph mentioning `mentioned_user` (so
/// the update carries both a visible change and a mention, with no repair needed).
fn walking_with_mention(children: u64, mentioned_user: &str) -> Vec<u8> {
    let past = READING_STEPS / children + 5;
    let mut run = walkers(0, past, children);
    // Append a paragraph after the last typed paragraph, containing a mention of `mentioned_user`.
    let paragraph_clock = past;
    run.changes.push(0x87);
    number(&mut run.changes, TYPIST);
    number(&mut run.changes, children - 1);
    run.changes.extend_from_slice(b"\x03\x09paragraph");
    // Mention element inside that paragraph (parent = (WALKER, paragraph_clock)).
    let mention_clock = past + 1;
    run.changes.push(0x07);
    run.changes.push(0x00);
    number(&mut run.changes, WALKER);
    number(&mut run.changes, paragraph_clock);
    run.changes.extend_from_slice(b"\x03\x07mention");
    // Attribute `userId = mentioned_user` on the mention element (content Any = 8, key = 0x20).
    run.changes.push(0x28);
    run.changes.push(0x00);
    number(&mut run.changes, WALKER);
    number(&mut run.changes, mention_clock);
    number(&mut run.changes, 6);
    run.changes.extend_from_slice(b"userId");
    // 1 Any value: string (tag 119) + length + UTF-8 bytes.
    number(&mut run.changes, 1);
    run.changes.push(119);
    number(
        &mut run.changes,
        u64::try_from(mentioned_user.len()).expect("fits"),
    );
    run.changes.extend_from_slice(mentioned_user.as_bytes());
    run.count += 3;
    update_of(&[run])
}

/// An update past `READING_STEPS` sent over a room WebSocket: the sender gets
/// `docAck`, both peers in the room get `docReset` of the next epoch with the update
/// in the remade document, the card body and a saved version hold the update, and a
/// mention in that update (as well as one typed earlier by another editor) still
/// notifies its recipient with the right actor.
#[tokio::test]
async fn an_update_past_reading_steps_in_a_room_acks_resets_keeps_body_version_and_mentions() {
    const LIST: u64 = 10_000;
    let th = roomy().await;
    let (a, b) = (th.client.me().await, th.client2.me().await);
    let c = th.add_user("third").await;
    let c_id = c.me().await.id;
    let board = board_with_owner(&th, ContainerVisibility::Private).await;
    for user in [&b.id, &c_id] {
        give(&th, &board.id, user, "editor").await;
    }
    let initial = card_with(&th.client, &board.id, &paragraphs(&["kept"])).await;
    let mut one = Peer::connect(&th.client, 101).await;
    let mut two = Peer::connect(&th.client2, 102).await;
    one.joined(TEST_WORKSPACE_ID, &initial.id).await;
    two.joined(TEST_WORKSPACE_ID, &initial.id).await;

    // Typed list plus an unprojected mention of C by B, then A sends an update past
    // `READING_STEPS` that also mentions B.
    one.send_update(&typed_in_order(LIST)).await;
    let first_ack = one.text("docAck").await;
    assert_eq!(first_ack["epoch"], json!(1), "{first_ack}");
    two.settle().await;
    let b_mentions_c = two.editor().append_mention(&c_id);
    two.send_update(&b_mentions_c).await;
    two.text("docAck").await;
    one.settle().await;

    let heavy = walking_with_mention(LIST, &b.id);
    one.send_update(&heavy).await;
    let ack = one.text("docAck").await;
    assert_eq!(ack["doc"], json!(one.doc), "{ack}");
    assert_eq!(ack["epoch"], json!(1), "{ack}");
    assert!(ack["seq"].as_i64().is_some_and(|seq| seq >= 3), "{ack}");

    for peer in [&mut one, &mut two] {
        let reset = peer.text("docReset").await;
        assert_eq!(
            reset,
            json!({"type": "docReset", "doc": peer.doc, "epoch": 2}),
            "room peer receives docReset for the remade epoch"
        );
        peer.settle().await;
        assert_eq!(peer.epoch, 2, "peer synced to epoch 2");
        let body = peer.body().expect("remade body");
        assert_eq!(
            paragraphs_of(Some(&body)),
            usize::try_from(LIST).expect("fits") + 3,
            "kept + typed list + B's mention + A's heavy mention"
        );
    }

    let remade_body = one.body().expect("body");
    assert_eq!(
        body_of(&card(&th.client, &initial.id).await),
        Some(remade_body.clone()),
        "card body matches the remade document"
    );

    // Mentions from both the earlier unprojected update (B -> C) and the remake-triggering
    // update (A -> B) are delivered with the right actors.
    let b_told = told(&th, &th.client2).await;
    assert_eq!(kinds(&b_told), (vec!["mention"], 0), "{b_told:?}");
    assert_eq!(
        b_told[0].subject.as_ref().expect("seen").actor_id,
        a.id,
        "A mentioned B in the update that triggered remake"
    );
    let c_told = told(&th, &c).await;
    assert_eq!(kinds(&c_told), (vec!["mention"], 0), "{c_told:?}");
    assert_eq!(
        c_told[0].subject.as_ref().expect("seen").actor_id,
        b.id,
        "B mentioned C before A's remake-triggering update"
    );

    // Leaving the room keeps a version holding the update.
    one.leave().await;
    two.leave().await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let newest_version = loop {
        let kept = versions(&th.client, &initial.id).await;
        if let Some(newest) = kept.first()
            && newest["editors"]
                .as_array()
                .is_some_and(|editors| editors.iter().any(|editor| editor == &json!(a.id)))
        {
            break newest["version"].as_i64().expect("version number");
        }
        assert!(tokio::time::Instant::now() < deadline, "version not kept");
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    let version_body: Value = th
        .client
        .get(&format!(
            "{TEST_WS}/cards/{}/body-versions/{newest_version}",
            initial.id
        ))
        .await
        .ok_json();
    assert_eq!(
        version_body["body"], remade_body,
        "version holds the update"
    );
    th.teardown().await;
}

/// Waits until `card_id` has `expected` paragraphs in its projected body.
async fn wait_for_paragraphs(th: &TestServer, card_id: &str, expected: usize) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while paragraphs_of(body_of(&card(&th.client, card_id).await).as_ref()) != expected {
        assert!(
            tokio::time::Instant::now() < deadline,
            "recovered card body did not reach {expected} paragraphs"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Stopping the server at `paused_before_projection` (after logging an update past
/// `READING_STEPS` and before writing its body to the card) and starting it again
/// recovers the update without losing the tail, writes it to the card, remakes the
/// document to 0 reading steps, and delivers its mention notification.
#[tokio::test]
async fn closing_at_paused_before_projection_keeps_an_update_past_reading_steps() {
    const LIST: u64 = 10_000;
    let th = roomy().await;
    let (a, b) = (th.client.me().await, th.client2.me().await);
    let board = board_with_owner(&th, ContainerVisibility::Private).await;
    give(&th, &board.id, &b.id, "editor").await;
    let card_id = card_with(&th.client, &board.id, &paragraphs(&["kept"]))
        .await
        .id;
    let app = editing_app(&th).await.app;
    let collab = app.collab().clone();
    let loaded = collab.open(&app, &card_id).await.expect("opened");
    collab
        .take(&app, &card_id, loaded.epoch, &a.id, &typed_in_order(LIST))
        .await
        .expect("typed");

    // Pause the projection inside the remake that follows logging the heavy update.
    let mut events = collab.subscribe();
    let (waiting, go) = collab.pause_next_projection();
    let heavy = walking_with_mention(LIST, &b.id);
    let taking = tokio::spawn({
        let (collab, app, card_id, user_id) =
            (collab.clone(), app.clone(), card_id.clone(), a.id.clone());
        async move {
            collab
                .take(&app, &card_id, loaded.epoch, &user_id, &heavy)
                .await
        }
    });
    waiting.notified().await;

    // The update is logged, and the card does not hold it yet.
    assert_eq!(
        paragraphs_of(body_of(&card(&th.client, &card_id).await).as_ref()),
        1,
        "card has not been projected yet"
    );
    let logged = app
        .store()
        .get_body_doc(&card_id)
        .await
        .expect("read")
        .expect("doc");
    assert_eq!(
        (logged.projected_seq, logged.log_end, logged.updates.len()),
        (0, 2, 2),
        "both updates are logged and not projected yet"
    );

    // Simulate crash at paused_before_projection: close the store so the paused
    // projection cannot commit, wake it so its slot releases, and start a fresh server.
    let th = th
        .killed_and_started_again(async |old_app| {
            old_app.store().close().await;
            go.notify_one();
            let taken = taking
                .await
                .expect("joined")
                .expect("update was logged before store closed");
            assert!(!taken.remade, "remake failed because store closed");
            assert_eq!(
                collab.held_state_bytes(&app, &card_id).await,
                None,
                "failed must_remake slot is forgotten instead of kept open"
            );
            assert!(
                std::iter::from_fn(|| events.try_recv().ok())
                    .any(|event| event.card_id == card_id && event.change == DocChange::Closed),
                "room is told DocChange::Closed when must_remake remake fails"
            );
        })
        .await;

    let expected = usize::try_from(LIST).expect("fits") + 2;
    wait_for_paragraphs(&th, &card_id, expected).await;
    let restarted = Editing {
        card_id: card_id.clone(),
        ..editing_app(&th).await
    };
    let (again, again_epoch) = joined(restarted.app.collab(), &restarted.app, &card_id, 202).await;
    let body = again.body().map(|body| body.document().clone());
    assert_eq!(
        paragraphs_of(body.as_ref()),
        expected,
        "kept all paragraphs"
    );
    assert_eq!(
        body_of(&card(&th.client, &card_id).await),
        body,
        "card matches recovered doc"
    );
    let made = stored(&restarted).await;
    assert_eq!(made.epoch, loaded.epoch + 1, "remade cleanly on recovery");
    assert_eq!(again_epoch, made.epoch, "rejoined at recovered epoch");
    assert_eq!(made.updates.len(), 0, "log cleared after recovery remake");
    assert_eq!(
        reading_steps(&made.state, u64::MAX).expect("a state"),
        0,
        "recovered document has 0 reading steps"
    );
    let b_told = told(&th, &th.client2).await;
    assert_eq!(kinds(&b_told), (vec!["mention"], 0), "{b_told:?}");
    assert_eq!(
        b_told[0].subject.as_ref().expect("seen").actor_id,
        a.id,
        "recovered mention credits A"
    );
    th.teardown().await;
}

/// If a stored log has another update *after* one that already left the document
/// unsettled past `READING_STEPS` (as from a pre-counting server), replay stops
/// without handing the slow state back to `yrs` and falls back to the card's body
/// without ending the worker.
#[tokio::test]
async fn logged_updates_after_one_past_reading_steps_fall_back_to_card_without_ending_worker() {
    let th = roomy().await;
    let e = editing(&th, &paragraphs(&["first"])).await;
    let collab = e.app.collab();
    collab.open(&e.app, &e.card_id).await.expect("opened");
    collab.abandon().await;
    let before = stored(&e).await;
    let each = READING_STEPS / CHILDREN / 2 + 8;
    let trailing = Editor::join(105, &before.state).append_paragraph("after slow");
    let updates = [
        typed_in_order(CHILDREN),
        walking(0, each, CHILDREN),
        walking(each, each, CHILDREN),
        trailing,
    ];
    let entries: Vec<(&[u8], &str)> = updates
        .iter()
        .map(|update| (update.as_slice(), e.user1.as_str()))
        .collect();
    e.app
        .store()
        .append_body_updates(
            &e.card_id,
            &entries,
            before.as_read(),
            &[],
            &[],
            BodyBefore::Leave,
        )
        .await
        .expect("logged past the server");
    let worker = worker_of(&th, &e).await;

    let (again, again_epoch) = joined(collab, &e.app, &e.card_id, 102).await;
    assert_eq!(
        collab.worker_pids().await[0],
        Some(worker),
        "the worker runs on without being killed"
    );
    let body = again.body().map(|body| body.document().clone());
    assert_eq!(
        paragraphs_of(body.as_ref()),
        1,
        "fell back cleanly to the card's body"
    );
    assert_eq!(
        body_of(&card(&th.client, &e.card_id).await),
        body,
        "card and remade document match"
    );
    let made = stored(&e).await;
    assert_eq!(made.epoch, before.epoch + 1, "remade from card");
    assert_eq!(again_epoch, made.epoch, "rejoined at remade epoch");
    assert_eq!(made.updates.len(), 0, "log cleared by fallback remake");
    assert_eq!(
        reading_steps(&made.state, u64::MAX).expect("a state"),
        0,
        "remade document takes 0 steps"
    );
    th.teardown().await;
}

/// Closing a card whose `must_remake` slot was already forgotten while its log still
/// holds unwritten updates opens it aside, projects the body and mentions, and remakes
/// it to 0 reading steps instead of stranding the log.
#[tokio::test]
async fn close_finishes_a_forgotten_must_remake_log() {
    const LIST: u64 = 10_000;
    let th = roomy().await;
    let (a, b) = (th.client.me().await, th.client2.me().await);
    let board = board_with_owner(&th, ContainerVisibility::Private).await;
    give(&th, &board.id, &b.id, "editor").await;
    let card_a = card_with(&th.client, &board.id, &paragraphs(&["kept"])).await;
    let edit_a = Editing {
        card_id: card_a.id.clone(),
        ..editing_app(&th).await
    };
    let collab = edit_a.app.collab();
    collab.open(&edit_a.app, &card_a.id).await.expect("opened");
    collab.abandon().await;
    let before = stored(&edit_a).await;
    let typed = typed_in_order(LIST);
    let heavy = walking_with_mention(LIST, &b.id);
    edit_a
        .app
        .store()
        .append_body_updates(
            &card_a.id,
            &[
                (typed.as_slice(), a.id.as_str()),
                (heavy.as_slice(), a.id.as_str()),
            ],
            before.as_read(),
            &[],
            &[a.id.as_str()],
            BodyBefore::Leave,
        )
        .await
        .expect("logged");
    assert_eq!(
        collab.held_state_bytes(&edit_a.app, &card_a.id).await,
        None,
        "slot is forgotten before close"
    );
    collab
        .close(&edit_a.app, &card_a.id)
        .await
        .expect("close opens aside and finishes unwritten must_remake log");
    assert_eq!(
        paragraphs_of(body_of(&card(&th.client, &card_a.id).await).as_ref()),
        usize::try_from(LIST).expect("fits") + 2,
        "close projected the forgotten must_remake updates"
    );
    let made = stored(&edit_a).await;
    assert_eq!(made.epoch, before.epoch + 1, "remade during close");
    assert_eq!(made.updates.len(), 0, "log cleared by remake");
    assert_eq!(
        reading_steps(&made.state, u64::MAX).expect("a state"),
        0,
        "remade state takes 0 steps"
    );
    let b_told = told(&th, &th.client2).await;
    assert_eq!(kinds(&b_told), (vec!["mention"], 0), "{b_told:?}");
    assert_eq!(
        b_told[0].subject.as_ref().expect("seen").actor_id,
        a.id,
        "mention credits A"
    );
    th.teardown().await;
}

/// Recovering a card that crashed *after* `project_body` and *before*
/// `remake_body_doc` (`projected_seq == log_end`, `state_seq < log_end`) remakes the
/// document to 0 reading steps without re-projecting or duplicating mention
/// notifications.
#[tokio::test]
async fn recovering_after_projection_and_before_remake_dedups_mentions() {
    const LIST: u64 = 10_000;
    let th = roomy().await;
    let (a, b) = (th.client.me().await, th.client2.me().await);
    let board = board_with_owner(&th, ContainerVisibility::Private).await;
    give(&th, &board.id, &b.id, "editor").await;
    let card_b = card_with(&th.client, &board.id, &paragraphs(&["kept"])).await;
    let edit_b = Editing {
        card_id: card_b.id.clone(),
        ..editing_app(&th).await
    };
    let collab = edit_b.app.collab();
    collab.open(&edit_b.app, &card_b.id).await.expect("opened");
    collab.abandon().await;
    let before = stored(&edit_b).await;
    let typed = typed_in_order(LIST);
    let heavy = walking_with_mention(LIST, &b.id);
    let seqs = edit_b
        .app
        .store()
        .append_body_updates(
            &card_b.id,
            &[
                (typed.as_slice(), a.id.as_str()),
                (heavy.as_slice(), a.id.as_str()),
            ],
            before.as_read(),
            &[],
            &[a.id.as_str()],
            BodyBefore::Leave,
        )
        .await
        .expect("logged");
    let after_log = stored(&edit_b).await;
    let uncounted = UpdateLimits {
        update_bytes: 1024 * 1024,
        state_bytes: 16 * 1024 * 1024,
        body_bytes: fvoci_model::BODY_MAX_BYTES,
        placing_steps: None,
        clients: None,
        client_changes: None,
        total_changes: None,
        reading_steps: None,
    };
    let projected_body = BodyDoc::from_state(&before.state, &uncounted)
        .expect("opened")
        .doc
        .take(&typed, &uncounted)
        .expect("typed")
        .doc
        .take(&heavy, &uncounted)
        .expect("heavy")
        .repair
        .body;
    let digest = body_digest(projected_body.as_ref());
    let store = edit_b.app.store();
    let read_at = store.get_block(&card_b.id).await.expect("card").updated_at;
    store
        .project_body(
            &card_b.id,
            fvoci_store::Projection::Written {
                body: projected_body,
                read_at,
            },
            *seqs.last().expect("seq"),
            &digest,
            &a.id,
            after_log.as_read(),
        )
        .await
        .expect("projected before crash");

    assert_eq!(
        kinds(&told(&th, &th.client2).await),
        (vec!["mention"], 0),
        "mention delivered once at projection"
    );
    collab.recover(&th.app).await;

    let made = stored(&edit_b).await;
    assert_eq!(made.epoch, before.epoch + 1, "remade by recover");
    assert_eq!(made.updates.len(), 0, "unfolded log cleared by remake");
    assert_eq!(
        reading_steps(&made.state, u64::MAX).expect("a state"),
        0,
        "recovered document takes 0 steps"
    );
    assert_eq!(
        paragraphs_of(body_of(&card(&th.client, &card_b.id).await).as_ref()),
        usize::try_from(LIST).expect("fits") + 2,
        "card kept its projected body"
    );
    assert_eq!(
        kinds(&told(&th, &th.client2).await),
        (vec!["mention"], 0),
        "no duplicate mention notification after post-projection recovery"
    );
    th.teardown().await;
}

/// Stopping the server at `paused_before_projection` after logging BOTH an update past
/// `READING_STEPS` that needed an in-place repair AND its logged repair row recovers on
/// restart without failing replay on the repair row: the repaired body (with the update's
/// valid paragraphs and without the invalid node) is written to the card and the document
/// is remade to 0 reading steps.
#[tokio::test]
async fn closing_at_paused_before_projection_keeps_a_repaired_update_past_reading_steps() {
    const LIST: u64 = 10_000;
    let past = READING_STEPS / LIST + 5;
    let th = roomy().await;
    let a = th.client.me().await;
    let e = editing(&th, &paragraphs(&["kept"])).await;
    let app = e.app.clone();
    let collab = app.collab().clone();
    let loaded = collab.open(&app, &e.card_id).await.expect("opened");
    collab
        .take(&app, &e.card_id, loaded.epoch, &a.id, &typed_in_order(LIST))
        .await
        .expect("typed");

    let (waiting, go) = collab.pause_next_projection();
    let heavy_with_repair = update_of(&[repair_needed_walkers(past, LIST)]);
    let taking = tokio::spawn({
        let (collab, app, card_id, user_id) =
            (collab.clone(), app.clone(), e.card_id.clone(), a.id.clone());
        async move {
            collab
                .take(&app, &card_id, loaded.epoch, &user_id, &heavy_with_repair)
                .await
        }
    });
    waiting.notified().await;

    // Both the client update and its repair update are logged (3 rows total with typed),
    // and the card has not been projected yet.
    let logged = app
        .store()
        .get_body_doc(&e.card_id)
        .await
        .expect("read")
        .expect("doc");
    assert_eq!(
        (logged.projected_seq, logged.log_end, logged.updates.len()),
        (0, 3, 3),
        "typed + heavy update + logged repair row are all persisted"
    );
    assert!(
        logged.updates[2].written_by.is_empty(),
        "third row is the repair update"
    );

    let th = th
        .killed_and_started_again(async |old_app| {
            old_app.store().close().await;
            go.notify_one();
            let taken = taking
                .await
                .expect("joined")
                .expect("update and repair were logged before store closed");
            assert!(taken.repair.is_some(), "repair update returned");
            assert!(!taken.remade, "remake failed because store closed");
        })
        .await;

    let expected = usize::try_from(LIST).expect("fits") + 1;
    wait_for_paragraphs(&th, &e.card_id, expected).await;
    let restarted = Editing {
        card_id: e.card_id.clone(),
        ..editing_app(&th).await
    };
    let (again, again_epoch) =
        joined(restarted.app.collab(), &restarted.app, &e.card_id, 202).await;
    let body = again.body().map(|body| body.document().clone());
    assert_eq!(
        paragraphs_of(body.as_ref()),
        expected,
        "all valid paragraphs kept and unknown element deleted"
    );
    let made = stored(&restarted).await;
    assert_eq!(made.epoch, loaded.epoch + 1, "remade on restart");
    assert_eq!(again_epoch, made.epoch, "rejoined at remade epoch");
    assert_eq!(made.updates.len(), 0, "log cleared after remake");
    assert_eq!(
        reading_steps(&made.state, u64::MAX).expect("a state"),
        0,
        "recovered document takes 0 reading steps"
    );
    th.teardown().await;
}

/// Stopping the server at `paused_before_projection` after logging an update past
/// `READING_STEPS` that required an in-place table mend (`mended > 0`) and its logged
/// repair row recovers on restart: `replay` skips the logged mend row, projects the
/// mended 3-row 2-column table and all paragraphs, and remakes to 0 reading steps.
#[tokio::test]
async fn closing_at_paused_before_projection_keeps_table_mend_past_reading_steps() {
    const LIST: u64 = 10_000;
    let past = READING_STEPS / LIST + 5;
    let cell = json!({"type": "tableCell", "content": [{"type": "paragraph"}]});
    let row = json!({"type": "tableRow", "content": [cell.clone(), cell]});
    let table_doc =
        json!({"type": "doc", "content": [{"type": "table", "content": [row.clone(), row]}]});

    let th = roomy().await;
    let a = th.client.me().await;
    let e = editing(&th, &table_doc).await;
    let app = e.app.clone();
    let collab = app.collab().clone();
    let loaded = collab.open(&app, &e.card_id).await.expect("opened");
    collab
        .take(&app, &e.card_id, loaded.epoch, &a.id, &typed_in_order(LIST))
        .await
        .expect("typed");
    let before = stored(&e).await;
    let mut state = before.state.clone();
    for update in &before.updates {
        state = fvoci_collab::yrs::merge_updates_v1([&state, &update.bytes]).expect("merged");
    }
    let editor = Editor::join(303, &state);
    let short_row = editor.add_table_row(1);
    let walkers_bytes = walking(0, past, LIST);
    let combined =
        fvoci_collab::yrs::merge_updates_v1([&short_row, &walkers_bytes]).expect("merged");

    let (waiting, go) = collab.pause_next_projection();
    let taking = tokio::spawn({
        let (collab, app, card_id, user_id) =
            (collab.clone(), app.clone(), e.card_id.clone(), a.id.clone());
        async move {
            collab
                .take(&app, &card_id, loaded.epoch, &user_id, &combined)
                .await
        }
    });
    waiting.notified().await;

    let th = th
        .killed_and_started_again(async |old_app| {
            old_app.store().close().await;
            go.notify_one();
            let taken = taking.await.expect("joined").expect("logged before close");
            assert!(taken.repair.is_some(), "table mend update returned");
            assert!(!taken.remade, "remake failed because store closed");
        })
        .await;

    let expected = usize::try_from(LIST).expect("fits") + 1;
    wait_for_paragraphs(&th, &e.card_id, expected).await;
    let restarted = Editing {
        card_id: e.card_id.clone(),
        ..editing_app(&th).await
    };
    let (again, again_epoch) =
        joined(restarted.app.collab(), &restarted.app, &e.card_id, 304).await;
    let body = again.body().expect("body");
    let rows = body.document()["content"]
        .as_array()
        .expect("content")
        .iter()
        .find(|n| n["type"] == "table")
        .expect("table")["content"]
        .as_array()
        .expect("rows");
    assert_eq!(rows.len(), 3, "three rows preserved");
    assert_eq!(
        rows[2]["content"].as_array().expect("cells").len(),
        2,
        "third row padded to 2 cells by mend"
    );
    let made = stored(&restarted).await;
    assert_eq!(made.epoch, loaded.epoch + 1, "remade on restart");
    assert_eq!(again_epoch, made.epoch, "rejoined at remade epoch");
    assert_eq!(made.updates.len(), 0, "log cleared after remake");
    assert_eq!(reading_steps(&made.state, u64::MAX).expect("state"), 0);
    th.teardown().await;
}
