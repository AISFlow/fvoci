//! Editing a card's body together when the worker process dies or does not answer, and
//! what is past the limits of a document.

use std::time::{Duration, Instant};

use fvoci_collab::testing::Editor;
use fvoci_server::collab::{Collab, DocChange};
use fvoci_server::error::{AppError, Reason};
use serde_json::{Value, json};

use crate::collab::{Editing, body_of, card, editing, joined};
use crate::common::{Setup, TestServer};

fn paragraphs(texts: &[&str]) -> Value {
    let content: Vec<Value> = texts
        .iter()
        .map(|text| json!({"type": "paragraph", "content": [{"type": "text", "text": text}]}))
        .collect();
    json!({"type": "doc", "content": content})
}

fn reason_of(error: &AppError) -> Option<&'static str> {
    error.reason().map(Reason::as_str)
}

async fn worker_pid(e: &Editing) -> u32 {
    let pids = e.app.collab().worker_pids().await;
    pids[0].expect("a worker runs")
}

#[cfg(unix)]
pub(crate) fn kill(pid: u32) {
    let status = std::process::Command::new("kill")
        .args(["-KILL", &pid.to_string()])
        .status()
        .expect("kill runs");
    assert!(status.success(), "kill -KILL {pid}");
}

/// A server whose workers give up on a request after 5 s.
async fn impatient() -> TestServer {
    Setup::native()
        .configure(|config| config.collab.collab_worker_timeout_ms = 5000)
        .start()
        .await
        .init_basic()
        .await
}

/// A worker killed with a document open: the server goes on answering, the next
/// request about the document starts another worker and opens the document again from
/// what is stored, with every update it took, and the room is told.
#[cfg(unix)]
#[tokio::test]
async fn a_killed_worker_leaves_the_server_answering_and_the_document_opens_again() {
    let th = Setup::native().start().await.init_basic().await;
    let e = editing(&th, &paragraphs(&["first"])).await;
    let collab = e.app.collab();
    let mut events = collab.subscribe();
    let (editor, editor_epoch) = joined(collab, &e.app, &e.card_id, 101).await;
    for line in ["one", "two"] {
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
    let killed = worker_pid(&e).await;
    kill(killed);
    // The server finds the worker gone (here: before the next request is asked of it,
    // which is not counted against its document).
    while e.app.collab().worker_pids().await[0].is_some() {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let read = card(&th.client, &e.card_id).await;
    assert_eq!(read.id, e.card_id, "the server answers");
    let (again, again_epoch) = joined(collab, &e.app, &e.card_id, 102).await;
    assert_eq!(again.body().map(|body| body.document().clone()), wanted);
    assert_ne!(worker_pid(&e).await, killed, "another worker");
    let event = events.try_recv().expect("an event");
    assert_eq!(
        (event.card_id, event.change),
        (e.card_id.clone(), DocChange::Reopened)
    );
    collab
        .take(
            &e.app,
            &e.card_id,
            again_epoch,
            &e.user1,
            &again.append_paragraph("three"),
        )
        .await
        .expect("taken by the new worker");
    collab.close(&e.app, &e.card_id).await.expect("closed");
    assert_eq!(
        body_of(&card(&th.client, &e.card_id).await),
        again.body().map(|body| body.document().clone())
    );
    th.teardown().await;
}

/// A request that the worker does not answer in time ends the worker and is refused;
/// the server answers meanwhile, and the document opens again from what is stored. A
/// document whose requests end the worker twice, a little apart, is closed for a while,
/// even if it was closed between them.
#[tokio::test]
async fn a_request_past_the_time_limit_ends_the_worker_and_is_refused() {
    let th = impatient().await;
    let e = editing(&th, &paragraphs(&["first"])).await;
    let other = editing(&th, &paragraphs(&["elsewhere"])).await;
    let collab = e.app.collab();
    let (editor, editor_epoch) = joined(collab, &e.app, &e.card_id, 101).await;
    collab
        .take(
            &e.app,
            &e.card_id,
            editor_epoch,
            &e.user1,
            &editor.append_paragraph("one"),
        )
        .await
        .expect("taken");
    let wanted = editor.body().map(|body| body.document().clone());
    let before = worker_pid(&e).await;

    let stalled = collab
        .stall_document(&e.app, &e.card_id, 30_000)
        .await
        .expect_err("past the time limit");
    assert_eq!(reason_of(&stalled), Some("document_unavailable"));
    let read = card(&th.client, &e.card_id).await;
    assert_eq!(read.id, e.card_id, "the server answers");
    let (again, _) = joined(collab, &e.app, &e.card_id, 102).await;
    assert_eq!(again.body().map(|body| body.document().clone()), wanted);
    assert_ne!(worker_pid(&e).await, before, "another worker");
    // Closed and opened again, it remembers the death.
    collab.close(&e.app, &e.card_id).await.expect("closed");

    let stalled = collab
        .stall_document(&e.app, &e.card_id, 30_000)
        .await
        .expect_err("past the time limit");
    assert_eq!(reason_of(&stalled), Some("document_unavailable"));
    let closed = collab
        .open(&e.app, &e.card_id)
        .await
        .expect_err("closed for a while");
    assert_eq!(reason_of(&closed), Some("document_unavailable"));
    collab
        .open(&other.app, &other.card_id)
        .await
        .expect("another card's document opens");
    th.teardown().await;
}

/// An update that ends the worker that takes it, past the time limit or by breaking it,
/// is in neither the document nor its log; its sender's updates are refused for a
/// while, to other cards too, and another user of the organization still writes.
#[cfg(unix)]
#[tokio::test]
async fn an_update_that_ends_the_worker_is_not_kept_and_its_sender_is_held_back() {
    const TIMEOUT: Duration = Duration::from_secs(3);
    let th = Setup::native()
        .configure(|config| {
            config.collab.collab_worker_timeout_ms =
                u64::try_from(TIMEOUT.as_millis()).expect("fits");
        })
        .start()
        .await
        .init_basic()
        .await;
    let e = editing(&th, &paragraphs(&["first"])).await;
    let other = editing(&th, &paragraphs(&["elsewhere"])).await;
    let collab = e.app.collab();
    let body = |editor: &Editor| editor.body().map(|body| body.document().clone());

    // Past the time limit.
    let (editor, epoch) = joined(collab, &e.app, &e.card_id, 101).await;
    let before = body(&editor);
    let pid = worker_pid(&e).await;
    collab.stall_updates(&e.app, &e.card_id, 30_000);
    let ended = collab
        .take(
            &e.app,
            &e.card_id,
            epoch,
            &e.user1,
            &editor.append_paragraph("never"),
        )
        .await
        .expect_err("it ended the worker");
    assert_eq!(reason_of(&ended), Some("document_unavailable"));
    collab.take_updates(&e.app, &e.card_id);
    let (again, _) = joined(collab, &e.app, &e.card_id, 102).await;
    assert_eq!(body(&again), before, "in neither the document nor its log");
    assert_ne!(worker_pid(&e).await, pid, "another worker");

    let (elsewhere, elsewhere_epoch) = joined(collab, &other.app, &other.card_id, 103).await;
    let held = collab
        .take(
            &other.app,
            &other.card_id,
            elsewhere_epoch,
            &e.user1,
            &elsewhere.append_paragraph("held"),
        )
        .await
        .expect_err("held back");
    assert_eq!(reason_of(&held), Some("document_unavailable"));
    let (by_another, by_another_epoch) = joined(collab, &other.app, &other.card_id, 104).await;
    collab
        .take(
            &other.app,
            &other.card_id,
            by_another_epoch,
            &e.user2,
            &by_another.append_paragraph("by another"),
        )
        .await
        .expect("another user of the organization writes");

    // Broken: the worker is killed while it is on the update.
    let before = body(&by_another);
    let pid = worker_pid(&other).await;
    collab.stall_updates(&other.app, &other.card_id, 30_000);
    let started = Instant::now();
    let ending = tokio::spawn({
        let (collab, app, card_id, user) = (
            collab.clone(),
            other.app.clone(),
            other.card_id.clone(),
            e.user2.clone(),
        );
        let update = by_another.append_paragraph("broken");
        async move {
            collab
                .take(&app, &card_id, by_another_epoch, &user, &update)
                .await
        }
    });
    tokio::time::sleep(Duration::from_millis(500)).await;
    kill(pid);
    let ended = ending
        .await
        .expect("ended")
        .expect_err("it ended the worker");
    assert!(
        started.elapsed() < TIMEOUT,
        "broken, not past the time limit"
    );
    assert_eq!(reason_of(&ended), Some("document_unavailable"));
    collab.take_updates(&other.app, &other.card_id);
    let (again, again_epoch) = joined(collab, &other.app, &other.card_id, 105).await;
    assert_eq!(body(&again), before, "in neither the document nor its log");
    let held = collab
        .take(
            &other.app,
            &other.card_id,
            again_epoch,
            &e.user2,
            &again.append_paragraph("held"),
        )
        .await
        .expect_err("held back");
    assert_eq!(reason_of(&held), Some("document_unavailable"));
    th.teardown().await;
}

/// Whether a refusal is that of a sender held back, not of an update that ended the
/// worker: both are `document_unavailable`.
fn held_back(error: &AppError) -> bool {
    error.to_string().contains("refused for a while")
}

/// Updates of one user to several cards sent together, which would each end the worker:
/// the first does, and the others wait for the sender's turn and are refused as held
/// back before they are sent, not taken one after another.
#[cfg(unix)]
#[tokio::test]
async fn the_updates_a_sender_sent_together_end_one_worker() {
    const TIMEOUT: Duration = Duration::from_secs(2);
    const CARDS: u64 = 4;
    let th = Setup::native()
        .configure(|config| {
            config.collab.collab_worker_timeout_ms =
                u64::try_from(TIMEOUT.as_millis()).expect("fits");
        })
        .start()
        .await
        .init_basic()
        .await;
    let mut sent = Vec::new();
    for client in 0..CARDS {
        let e = editing(&th, &paragraphs(&["x"])).await;
        let collab = e.app.collab();
        let (editor, epoch) = joined(collab, &e.app, &e.card_id, 200 + client).await;
        collab.stall_updates(&e.app, &e.card_id, 30_000);
        let update = editor.append_paragraph("ending");
        sent.push((e, epoch, update));
    }
    let collab = sent[0].0.app.collab().clone();
    let before = collab.first_worker_ended().await;
    let takes: Vec<_> = sent
        .iter()
        .map(|(e, epoch, update)| {
            let (collab, app, card_id, user) = (
                collab.clone(),
                e.app.clone(),
                e.card_id.clone(),
                e.user1.clone(),
            );
            let (epoch, update) = (*epoch, update.clone());
            tokio::spawn(async move { collab.take(&app, &card_id, epoch, &user, &update).await })
        })
        .collect();
    let mut refusals = Vec::new();
    for take in takes {
        refusals.push(take.await.expect("ended").expect_err("not taken"));
    }
    assert!(
        refusals
            .iter()
            .all(|error| reason_of(error) == Some("document_unavailable")),
        "{refusals:?}"
    );
    assert_eq!(
        refusals.iter().filter(|error| held_back(error)).count(),
        refusals.len() - 1,
        "{refusals:?}"
    );
    assert_eq!(
        collab.first_worker_ended().await.workers - before.workers,
        1,
        "one worker ended"
    );
    let (e, ..) = &sent[0];
    collab.take_updates(&e.app, &e.card_id);
    let (by_another, epoch) = joined(&collab, &e.app, &e.card_id, 300).await;
    collab
        .take(
            &e.app,
            &e.card_id,
            epoch,
            &e.user2,
            &by_another.append_paragraph("by another"),
        )
        .await
        .expect("another user of the organization writes");
    th.teardown().await;
}

/// How long the workers of [`two_workers`] take over a request before they are ended.
const TWO_WORKERS_TIMEOUT: Duration = Duration::from_secs(3);

/// A server of two workers, which give up on a request after [`TWO_WORKERS_TIMEOUT`].
async fn two_workers() -> TestServer {
    Setup::native()
        .configure(|config| {
            config.collab.collab_workers = 2;
            config.collab.collab_documents_per_worker = 1;
            config.collab.collab_worker_timeout_ms = 3_000;
        })
        .start()
        .await
        .init_basic()
        .await
}

/// Updates of one user sent together to two cards whose documents are on two workers,
/// each of which would end its worker: one ends its worker, and the other is refused as
/// held back without reaching the other, which goes on with the documents it holds.
#[cfg(unix)]
#[tokio::test]
async fn the_updates_a_sender_sent_together_to_two_workers_end_one() {
    let th = two_workers().await;
    let first = editing(&th, &paragraphs(&["x"])).await;
    let second = editing(&th, &paragraphs(&["y"])).await;
    let collab = first.app.collab().clone();
    // Each opened on the worker given fewer documents: one on each.
    let (on_first, first_epoch) = joined(&collab, &first.app, &first.card_id, 201).await;
    let (on_second, second_epoch) = joined(&collab, &second.app, &second.card_id, 202).await;
    let pids = collab.worker_pids().await;
    assert!(
        pids.iter().all(Option::is_some),
        "a document on each worker: {pids:?}"
    );
    let before = collab.workers_ended().await;
    let takes: Vec<_> = [
        (&first, first_epoch, on_first.append_paragraph("ending")),
        (&second, second_epoch, on_second.append_paragraph("ending")),
    ]
    .into_iter()
    .map(|(e, epoch, update)| {
        collab.stall_updates(&e.app, &e.card_id, 30_000);
        let (collab, app, card_id, user) = (
            collab.clone(),
            e.app.clone(),
            e.card_id.clone(),
            e.user1.clone(),
        );
        tokio::spawn(async move { collab.take(&app, &card_id, epoch, &user, &update).await })
    })
    .collect();
    let mut refusals = Vec::new();
    for take in takes {
        refusals.push(take.await.expect("ended").expect_err("not taken"));
    }
    assert!(
        refusals
            .iter()
            .all(|error| reason_of(error) == Some("document_unavailable")),
        "{refusals:?}"
    );
    assert_eq!(
        refusals.iter().filter(|error| held_back(error)).count(),
        1,
        "{refusals:?}"
    );
    assert_eq!(collab.workers_ended().await - before, 1, "one worker ended");
    // The worker that did not end still holds its document, and takes another user's.
    for e in [&first, &second] {
        collab.take_updates(&e.app, &e.card_id);
    }
    let still = collab.worker_pids().await;
    let kept = pids
        .iter()
        .zip(&still)
        .filter(|(was, is)| was == is)
        .count();
    assert_eq!(kept, 1, "{pids:?} then {still:?}");
    for (e, client) in [(&first, 301), (&second, 302)] {
        let (by_another, epoch) = joined(&collab, &e.app, &e.card_id, client).await;
        collab
            .take(
                &e.app,
                &e.card_id,
                epoch,
                &e.user2,
                &by_another.append_paragraph("by another"),
            )
            .await
            .expect("another user of the organization writes");
    }
    assert_eq!(collab.senders_in_line(), 0, "no turn is kept");
    th.teardown().await;
}

/// Waits until some user's update holds or waits for their turn.
async fn until_someone_is_in_line(collab: &Collab) {
    let waited = tokio::time::timeout(Duration::from_secs(60), async {
        while collab.senders_in_line() == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(waited.is_ok(), "no update came to its turn");
}

/// While an update of one user is on its way to ending a worker, holding their turn,
/// the updates of other users are taken by the other worker: of another user of the
/// organization, and of a user of another organization. Once it ended, killed, no turn
/// is kept in either organization.
#[cfg(unix)]
#[tokio::test]
async fn others_are_taken_while_an_update_of_a_sender_ends_a_worker() {
    // A time limit the others' updates are far within, however busy the machine: the
    // worker is killed under the update once they are taken.
    let th = Setup::native()
        .configure(|config| {
            config.collab.collab_workers = 2;
            config.collab.collab_documents_per_worker =
                if fvoci_store::testing::db_type_of_the_tests() == fvoci_store::DbType::Postgres {
                    2
                } else {
                    1
                };
            config.collab.collab_worker_timeout_ms = 120_000;
        })
        .start()
        .await
        .init_basic()
        .await;
    let ending = editing(&th, &paragraphs(&["x"])).await;
    let other = editing(&th, &paragraphs(&["y"])).await;
    let collab = ending.app.collab().clone();
    let (on_ending, ending_epoch) = joined(&collab, &ending.app, &ending.card_id, 201).await;
    // SQLite holds one organization. On PostgreSQL, fill the second slot on the first
    // worker so `other` spills to the second worker alongside the second organization.
    let theirs = if fvoci_store::testing::db_type_of_the_tests() == fvoci_store::DbType::Postgres {
        let filler = editing(&th, &paragraphs(&["z"])).await;
        joined(&collab, &filler.app, &filler.card_id, 203).await;
        Some(their_card(&th).await)
    } else {
        None
    };
    let (on_other, other_epoch) = joined(&collab, &other.app, &other.card_id, 202).await;
    let pids = collab.worker_pids().await;
    assert!(pids.iter().all(Option::is_some), "{pids:?}");

    collab.stall_updates(&ending.app, &ending.card_id, 100_000);
    let take = tokio::spawn({
        let (collab, app, card_id, user) = (
            collab.clone(),
            ending.app.clone(),
            ending.card_id.clone(),
            ending.user1.clone(),
        );
        let update = on_ending.append_paragraph("ending");
        async move {
            collab
                .take(&app, &card_id, ending_epoch, &user, &update)
                .await
        }
    });
    until_someone_is_in_line(&collab).await;
    collab
        .take(
            &other.app,
            &other.card_id,
            other_epoch,
            &other.user2,
            &on_other.append_paragraph("by another"),
        )
        .await
        .expect("another user of the organization writes meanwhile");
    if let Some((app, card_id, user)) = &theirs {
        let (on_theirs, epoch) = joined(&collab, app, card_id, 204).await;
        collab
            .take(
                app,
                card_id,
                epoch,
                user,
                &on_theirs.append_paragraph("theirs"),
            )
            .await
            .expect("a user of another organization writes meanwhile");
    }
    assert!(!take.is_finished(), "the worker had not ended yet");
    kill(pids[0].expect("the first worker runs"));
    let ended = take.await.expect("ended").expect_err("it ended the worker");
    assert!(!held_back(&ended), "{ended:?}");
    assert_eq!(collab.senders_in_line(), 0, "no turn is kept");
    th.teardown().await;
}

/// A card of another organization, with no body, as the application of that
/// organization reaches it, and the user that made it.
async fn their_card(th: &TestServer) -> (fvoci_server::app::App, String, String) {
    let (theirs, _, their_board) = crate::workspaces::another_organization(th).await;
    let members = theirs.get_members_for_container(&their_board).await;
    let owner = members.expect("their members")[0].user_id.clone();
    let mut card = fvoci_model::Block {
        id: fvoci_model::new_id(),
        container_id: their_board,
        block_type: fvoci_model::BlockType::Card,
        ..fvoci_model::Block::default()
    };
    theirs
        .insert_block(&mut card, &owner)
        .await
        .expect("their card is stored");
    (th.app.scoped(theirs.tenant()), card.id, owner)
}

/// The turn of a sender is given back however their update ends: answered, sent again
/// to the worker started after it lost the document, ended with the worker that is
/// killed under it, or past the time limit while the caller no longer waits for it.
/// Their next update waits for the one before to be done with its turn, and is then
/// refused as held back.
#[cfg(unix)]
#[tokio::test]
async fn the_turn_of_a_sender_is_given_back_however_their_update_ends() {
    let th = two_workers().await;
    let first = editing(&th, &paragraphs(&["x"])).await;
    let second = editing(&th, &paragraphs(&["y"])).await;
    let collab = first.app.collab().clone();
    let (on_first, first_epoch) = joined(&collab, &first.app, &first.card_id, 201).await;
    let (on_second, second_epoch) = joined(&collab, &second.app, &second.card_id, 202).await;
    let take = |e: &Editing, epoch: i64, user: &str, update: Vec<u8>| {
        let (collab, app, card_id, user) = (
            collab.clone(),
            e.app.clone(),
            e.card_id.clone(),
            user.to_owned(),
        );
        async move { collab.take(&app, &card_id, epoch, &user, &update).await }
    };

    // Answered.
    take(
        &first,
        first_epoch,
        &first.user1,
        on_first.append_paragraph("one"),
    )
    .await
    .expect("taken");
    assert_eq!(collab.senders_in_line(), 0, "answered");

    // Lost: the worker ended between two requests, and the update is sent again.
    let mut events = collab.subscribe();
    let pid = collab.worker_pids().await[0].expect("a worker runs");
    kill(pid);
    while collab.worker_pids().await[0] == Some(pid) {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    take(
        &first,
        first_epoch,
        &first.user1,
        on_first.append_paragraph("two"),
    )
    .await
    .expect("taken by the worker started again");
    let reopened = std::iter::from_fn(|| events.try_recv().ok())
        .any(|event| event.card_id == first.card_id && event.change == DocChange::Reopened);
    assert!(
        reopened,
        "the update found its worker gone and was sent again"
    );
    assert_eq!(collab.senders_in_line(), 0, "sent again");

    // Killed under the update. Its identifier first: the worker answers nothing else
    // while it stalls.
    let pid = collab.worker_pids().await[0].expect("a worker runs");
    collab.stall_updates(&first.app, &first.card_id, 30_000);
    let started = Instant::now();
    let ending = tokio::spawn(take(
        &first,
        first_epoch,
        &first.user1,
        on_first.append_paragraph("broken"),
    ));
    until_someone_is_in_line(&collab).await;
    // Killed once it is on the update, which it is while it does not tell its pid.
    while tokio::time::timeout(Duration::from_millis(100), collab.worker_pids())
        .await
        .is_ok()
    {}
    kill(pid);
    let ended = ending
        .await
        .expect("ended")
        .expect_err("it ended the worker");
    assert!(!held_back(&ended), "{ended:?}");
    assert!(
        started.elapsed() < TWO_WORKERS_TIMEOUT,
        "broken, not past the time limit"
    );
    assert_eq!(collab.senders_in_line(), 0, "killed");
    collab.take_updates(&first.app, &first.card_id);

    // Past the time limit, given up by its caller: the take goes on in a task of its
    // own, and the next update of the sender, sent as soon as it holds the turn, waits
    // for it.
    let (again, again_epoch) = joined(&collab, &first.app, &first.card_id, 203).await;
    collab.stall_updates(&second.app, &second.card_id, 30_000);
    let given_up = tokio::time::timeout(
        Duration::ZERO,
        take(
            &second,
            second_epoch,
            &second.user2,
            on_second.append_paragraph("given up"),
        ),
    )
    .await;
    assert!(given_up.is_err(), "given up before it was done");
    until_someone_is_in_line(&collab).await;
    let next = take(
        &first,
        again_epoch,
        &second.user2,
        again.append_paragraph("next"),
    )
    .await
    .expect_err("after the one given up");
    assert!(held_back(&next), "{next:?}");
    assert_eq!(collab.senders_in_line(), 0, "past the time limit");
    th.teardown().await;
}

/// An update that ends the worker is put down to its sender, not to the worker: the
/// deaths in a row that make the next worker wait to start do not count it.
#[cfg(unix)]
#[tokio::test]
async fn a_death_put_down_to_a_sender_does_not_make_the_next_worker_wait_longer() {
    let th = Setup::native()
        .configure(|config| config.collab.collab_worker_timeout_ms = 2_000)
        .start()
        .await
        .init_basic()
        .await;
    let e = editing(&th, &paragraphs(&["first"])).await;
    let collab = e.app.collab();
    let (editor, epoch) = joined(collab, &e.app, &e.card_id, 101).await;
    collab.stall_updates(&e.app, &e.card_id, 30_000);
    let before = collab.first_worker_ended().await;
    let ended = collab
        .take(
            &e.app,
            &e.card_id,
            epoch,
            &e.user1,
            &editor.append_paragraph("ending"),
        )
        .await
        .expect_err("it ended the worker");
    assert!(!held_back(&ended), "{ended:?}");
    let after = collab.first_worker_ended().await;
    assert_eq!(after.workers, before.workers + 1, "the worker ended");
    assert_eq!(after.in_a_row, 0, "the sender's death is not the worker's");
    collab
        .stall_worker(30_000)
        .await
        .expect_err("the worker ends");
    let after = collab.first_worker_ended().await;
    assert_eq!(after.in_a_row, 1, "the worker's death is");
    th.teardown().await;
}

/// A body of letters of every mark in turn with formulas, as many as a body may have:
/// of the bodies measured, the one whose document takes the most memory to take an
/// update for (`cost.rs`, `memory`: 103 MB held, 721 MB more to repair an update).
fn marked_body() -> Value {
    let every = json!({"type": "text", "text": "a", "marks": [
        {"type": "bold"}, {"type": "italic"}, {"type": "strike"}, {"type": "code"},
        {"type": "link", "attrs": {"href": "/"}},
    ]});
    let formula = json!({"type": "inlineMath", "attrs": {"latex": "x"}});
    let content: Vec<Value> = [every, formula].into_iter().cycle().take(200).collect();
    let paragraph = json!({"type": "paragraph", "content": content});
    let size = serde_json::to_vec(&paragraph).expect("JSON").len() + 1;
    let count =
        ((fvoci_model::BODY_MAX_BYTES - 1024) / size).min((fvoci_model::BODY_MAX_NODES - 10) / 201);
    json!({"type": "doc", "content": vec![paragraph; count]})
}

/// Lists of ten entries, as many as a body may have: of the bodies measured, the one
/// whose document takes the most memory for its state (24.5 times).
fn lists_body() -> Value {
    let entry = json!({"type": "listItem", "content": [
        {"type": "paragraph", "content": [{"type": "text", "text": "entry"}]},
    ]});
    let list = json!({"type": "bulletList", "content": vec![entry; 10]});
    json!({"type": "doc", "content": vec![list; (fvoci_model::BODY_MAX_NODES - 10) / 31]})
}

/// The documents that take a worker the most memory, at the memory a worker has by
/// default: while it holds as many documents of lists (the most memory for their state)
/// as it counts room for, it takes a letter typed into the document that takes the
/// most memory to take an update for, and an update of it the server repairs, and goes
/// on.
#[cfg(unix)]
#[tokio::test]
async fn the_documents_that_take_the_most_memory_are_edited_by_a_worker_that_holds_its_share() {
    let th = Setup::native()
        // Time for a build without optimizations; the memory is as it comes.
        .configure(|config| config.collab.collab_worker_timeout_ms = 120_000)
        .start()
        .await
        .init_basic()
        .await;
    let marked = editing(&th, &marked_body()).await;
    let collab = marked.app.collab();
    let (editor, epoch) = joined(collab, &marked.app, &marked.card_id, 101).await;
    let lists = lists_body();
    let mut held = 0;
    loop {
        let more = editing(&th, &lists).await;
        match collab.open(&more.app, &more.card_id).await {
            Ok(_) => held += 1,
            Err(error) => {
                assert_eq!(reason_of(&error), Some("too_many_documents"), "{error}");
                break;
            }
        }
        assert!(held < 40, "the worker counts room for {held} and more");
    }
    assert!(
        held >= 5,
        "the worker holds {held} documents of 100,000 nodes"
    );
    let pid = worker_pid(&marked).await;
    let typed = collab
        .take(
            &marked.app,
            &marked.card_id,
            epoch,
            &marked.user1,
            &editor.append_paragraph("typed"),
        )
        .await
        .expect("a letter typed");
    assert!(!typed.remade, "not made again");
    let repaired = collab
        .take(
            &marked.app,
            &marked.card_id,
            epoch,
            &marked.user1,
            &editor.append_element("nothing-a-body-has"),
        )
        .await
        .expect("an update the server repairs");
    assert!(repaired.repair.is_some(), "repaired");
    assert_eq!(worker_pid(&marked).await, pid, "the same worker");
    th.teardown().await;
}

/// A worker holds no more documents than it may; an update larger than the limit is
/// refused, and so is one whose body would be larger than a body may be. What was
/// refused is in neither the document nor its log.
#[tokio::test]
async fn what_is_past_the_limits_of_a_document_is_refused() {
    let th = Setup::native()
        .configure(|config| config.collab.collab_documents_per_worker = 2)
        .start()
        .await
        .init_basic()
        .await;
    let first = editing(&th, &paragraphs(&["first"])).await;
    let second = editing(&th, &paragraphs(&["second"])).await;
    let third = editing(&th, &paragraphs(&["third"])).await;
    let collab = first.app.collab();
    let loaded = collab
        .open(&first.app, &first.card_id)
        .await
        .expect("opened");
    collab
        .open(&second.app, &second.card_id)
        .await
        .expect("opened");
    let refused = collab
        .open(&third.app, &third.card_id)
        .await
        .expect_err("a third document");
    assert_eq!(reason_of(&refused), Some("too_many_documents"));
    collab
        .close(&second.app, &second.card_id)
        .await
        .expect("closed");
    collab
        .open(&third.app, &third.card_id)
        .await
        .expect("room for it once another is closed");

    let editor = Editor::join(101, &loaded.state);
    let large = editor.append_paragraph(&"x".repeat(65 * 1024));
    let refused = collab
        .take(
            &first.app,
            &first.card_id,
            loaded.epoch,
            &first.user1,
            &large,
        )
        .await
        .expect_err("larger than an update may be");
    assert_eq!(reason_of(&refused), Some("update_too_large"));

    let editor = Editor::join(102, &loaded.state);
    let mut refused = None;
    let mut taken = 0;
    // As many as a body has room for, and one more.
    for _ in 0..=fvoci_model::BODY_MAX_BYTES / (60 * 1024) {
        let update = editor.append_paragraph(&"y".repeat(60 * 1024));
        match collab
            .take(
                &first.app,
                &first.card_id,
                loaded.epoch,
                &first.user1,
                &update,
            )
            .await
        {
            Ok(_) => taken += 1,
            Err(error) => {
                refused = Some(error);
                break;
            }
        }
    }
    let refused = refused.expect("a body larger than a body may be is refused");
    assert_eq!(reason_of(&refused), Some("document_too_large"));
    collab
        .close(&first.app, &first.card_id)
        .await
        .expect("the body that was taken is written");
    // Opened again from its stored state and log.
    let (held, _) = joined(collab, &first.app, &first.card_id, 103).await;
    let body = held.body().expect("a body");
    let paragraphs = body.document()["content"].as_array().map_or(0, Vec::len);
    assert_eq!(
        paragraphs,
        1 + taken,
        "the first and those that were taken, not the refused ones"
    );
    assert_eq!(
        body_of(&card(&th.client, &first.card_id).await),
        Some(body.document().clone())
    );
    th.teardown().await;
}
