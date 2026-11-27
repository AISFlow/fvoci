//! When the server keeps the versions of a card's body besides when its editors leave:
//! by the clock while they edit on, as many as it is set to, and around its own end
//! and start.

use std::time::Duration;

use fvoci_collab::testing::Editor;
use serde_json::Value;

use crate::body_versions::{edit_and_leave, kept_after, lines, said, versions};
use crate::collab::{Editing, card, editing, editing_app, joined};
use crate::common::Setup;
use crate::rooms::{paragraphs, write_whole};

/// Has the user's editor write a line into the card's open document.
async fn typed(e: &Editing, editor: &Editor, epoch: i64, by: &str, line: &str) {
    e.app
        .collab()
        .take(
            &e.app,
            &e.card_id,
            epoch,
            by,
            &editor.append_paragraph(line),
        )
        .await
        .expect("taken");
}

/// While a body is edited on, a version is kept once the interval has passed
/// (`collab_version_minutes`) since the first change no version holds: with the next
/// change, of the body before it, or as the body is next written to the card. Until
/// then nothing is kept, however much is typed and written.
#[tokio::test]
async fn a_body_edited_on_is_kept_as_the_interval_passes() {
    let th = Setup::native()
        .configure(|config| config.collab.collab_version_minutes = 2)
        .start()
        .await
        .init_basic()
        .await;
    let e = editing(&th, &paragraphs(&["first"])).await;
    let collab = e.app.collab();
    let interval = Duration::from_mins(2);
    let (editor, epoch) = joined(collab, &e.app, &e.card_id, 101).await;
    let nobody: Vec<&str> = Vec::new();

    for line in ["one", "two"] {
        typed(&e, &editor, epoch, &e.user1, line).await;
        collab
            .write_body(&e.app, &e.card_id)
            .await
            .expect("written");
    }
    assert_eq!(
        said(&versions(&th.client, &e.card_id).await),
        [(1, nobody.clone(), false)],
        "the body before the editors came, and none of theirs yet"
    );
    collab.pass_version_time(Duration::from_mins(1));
    typed(&e, &editor, epoch, &e.user1, "three").await;
    collab
        .write_body(&e.app, &e.card_id)
        .await
        .expect("written");
    assert_eq!(versions(&th.client, &e.card_id).await.len(), 1, "not yet");

    // Due: the next change keeps the body before it, by who wrote it.
    collab.pass_version_time(Duration::from_mins(1));
    typed(&e, &editor, epoch, &e.user2, "four").await;
    collab
        .write_body(&e.app, &e.card_id)
        .await
        .expect("written");
    let kept = versions(&th.client, &e.card_id).await;
    assert_eq!(
        said(&kept),
        [
            (2, vec![e.user1.as_str()], false),
            (1, nobody.clone(), false)
        ]
    );
    assert_eq!(
        lines(&th.client, &e.card_id, &kept[0]).await,
        ["first", "one", "two", "three"]
    );

    // The next is due an interval after that change: kept as the body is written.
    typed(&e, &editor, epoch, &e.user1, "five").await;
    assert_eq!(versions(&th.client, &e.card_id).await.len(), 2, "not yet");
    collab.pass_version_time(interval);
    collab
        .write_body(&e.app, &e.card_id)
        .await
        .expect("written");
    let kept = versions(&th.client, &e.card_id).await;
    let mut both = vec![e.user1.as_str(), e.user2.as_str()];
    both.sort_unstable();
    assert_eq!(
        said(&kept)[..2],
        [(3, both, true), (2, vec![&e.user1], false)]
    );
    assert_eq!(
        lines(&th.client, &e.card_id, &kept[0]).await,
        ["first", "one", "two", "three", "four", "five"]
    );

    // Time alone keeps nothing, and leaving a body that is a version keeps no other.
    collab.pass_version_time(interval);
    collab
        .write_body(&e.app, &e.card_id)
        .await
        .expect("written");
    collab.close(&e.app, &e.card_id).await.expect("closed");
    assert_eq!(versions(&th.client, &e.card_id).await, kept);
    th.teardown().await;
}

/// The server keeps as many versions as it is set to (`collab_body_versions`),
/// however they came: the oldest go.
#[tokio::test]
async fn the_server_keeps_as_many_versions_as_it_is_set_to() {
    let th = Setup::native()
        .configure(|config| config.collab.collab_body_versions = 2)
        .start()
        .await
        .init_basic()
        .await;
    let e = editing(&th, &paragraphs(&["first"])).await;
    write_whole(&th.client, &e.card_id, &paragraphs(&["second"])).await;
    write_whole(&th.client, &e.card_id, &paragraphs(&["third"])).await;
    let kept = versions(&th.client, &e.card_id).await;
    assert_eq!(
        kept.iter()
            .map(|version| &version["version"])
            .collect::<Vec<_>>(),
        [3, 2],
        "the first went"
    );
    edit_and_leave(&e, &e.app, &e.user2, "typed").await;
    let kept = versions(&th.client, &e.card_id).await;
    assert_eq!(
        said(&kept),
        [
            (4, vec![e.user2.as_str()], true),
            (3, vec![e.user1.as_str()], false)
        ]
    );
    assert_eq!(
        lines(&th.client, &e.card_id, &kept[1]).await,
        ["third"],
        "a version that stays reads as it did"
    );
    th.teardown().await;
}

/// The server prunes the oldest versions once their combined serialized JSON size
/// exceeds `body_versions_max_bytes`, while always keeping the newest version even if it
/// alone exceeds the budget.
#[tokio::test]
async fn the_server_prunes_versions_to_fit_the_byte_budget() {
    const BUDGET: u64 = 320;
    let sample = paragraphs(&["first version with some padding text"]);
    let one_bytes =
        u64::try_from(serde_json::to_string(&sample).expect("json").len()).expect("fits u64");
    assert!(
        one_bytes * 2 <= BUDGET && one_bytes * 3 > BUDGET,
        "budget {BUDGET} fits two copies of {one_bytes} bytes, not three"
    );
    let th = Setup::native()
        .configure(|config| config.collab.body_versions_max_bytes = BUDGET)
        .start()
        .await
        .init_basic()
        .await;
    let e = editing(&th, &sample).await;
    write_whole(
        &th.client,
        &e.card_id,
        &paragraphs(&["second version with some padding text"]),
    )
    .await;
    write_whole(
        &th.client,
        &e.card_id,
        &paragraphs(&["third version with some padding text"]),
    )
    .await;
    let kept = versions(&th.client, &e.card_id).await;
    assert_eq!(
        kept.iter()
            .map(|version| &version["version"])
            .collect::<Vec<_>>(),
        [3, 2],
        "byte budget pruned version 1 while keeping versions 3 and 2"
    );
    edit_and_leave(&e, &e.app, &e.user2, "typed").await;
    let kept = versions(&th.client, &e.card_id).await;
    assert_eq!(
        kept.iter()
            .map(|version| &version["version"])
            .collect::<Vec<_>>(),
        [4, 3],
        "collaborative version keep also enforces the byte budget"
    );
    // A single body larger than the budget still keeps itself as the newest version.
    let oversized = "x".repeat(usize::try_from(BUDGET + 256).expect("fits usize"));
    write_whole(&th.client, &e.card_id, &paragraphs(&[&oversized])).await;
    let kept = versions(&th.client, &e.card_id).await;
    assert_eq!(
        kept.iter()
            .map(|version| &version["version"])
            .collect::<Vec<_>>(),
        [5],
        "newest version survives even when larger than the budget"
    );
    assert_eq!(
        lines(&th.client, &e.card_id, &kept[0]).await,
        [oversized],
        "oversized newest version reads back intact"
    );
    th.teardown().await;
}

/// A server that is stopped keeps what the editors of its open documents typed as
/// versions before it ends: the server that starts after it finds them kept.
#[tokio::test]
async fn a_server_that_stops_keeps_what_was_typed_as_a_version() {
    let th = Setup::native().start().await.init_basic().await;
    let e = editing(&th, &paragraphs(&["first"])).await;
    let (editor, epoch) = joined(e.app.collab(), &e.app, &e.card_id, 101).await;
    typed(&e, &editor, epoch, &e.user1, "typed").await;
    let (card_id, user1) = (e.card_id.clone(), e.user1.clone());
    drop(e);

    let th = th
        .killed_and_started_again(async |app| {
            // As a server that is asked to stop ends its documents; what it kept is
            // read before another server starts and sees to what was left.
            app.collab().end(app).await;
            let kept = app.store().count_rows("body_versions").await;
            assert_eq!(kept.expect("counted"), 2, "kept as the server ended");
        })
        .await;
    let kept = versions(&th.client, &card_id).await;
    assert_eq!(
        said(&kept),
        [(2, vec![user1.as_str()], true), (1, Vec::new(), false)]
    );
    assert_eq!(
        lines(&th.client, &card_id, &kept[0]).await,
        ["first", "typed"]
    );
    th.teardown().await;
}

/// A server that is killed leaves bodies no version holds: one its card does not hold
/// yet, and one its card holds. The server that starts after it keeps both, by who
/// typed them, once.
#[tokio::test]
async fn a_server_that_starts_keeps_what_a_killed_one_left() {
    let th = Setup::native().start().await.init_basic().await;
    let unwritten = editing(&th, &paragraphs(&["first"])).await;
    let written = editing(&th, &paragraphs(&["other"])).await;
    let collab = th.app.collab();
    for (e, by) in [(&unwritten, &unwritten.user1), (&written, &written.user2)] {
        let (editor, epoch) = joined(collab, &e.app, &e.card_id, 101).await;
        typed(e, &editor, epoch, by, "typed").await;
    }
    collab
        .write_body(&written.app, &written.card_id)
        .await
        .expect("written");
    let typed_into = card(&th.client, &written.card_id).await;
    let (user1, user2) = (unwritten.user1.clone(), unwritten.user2.clone());
    let (unwritten, written) = (unwritten.card_id.clone(), written.card_id.clone());

    let th = th
        .killed_and_started_again(async |app| app.collab().abandon().await)
        .await;
    for (card_id, by, before) in [(&unwritten, &user1, "first"), (&written, &user2, "other")] {
        let kept = kept_after(&th.client, card_id, 2).await;
        assert_eq!(
            said(&kept),
            [(2, vec![by.as_str()], true), (1, Vec::new(), false)],
            "{before}"
        );
        assert_eq!(
            lines(&th.client, card_id, &kept[0]).await,
            [before, "typed"]
        );
    }
    assert_eq!(
        card(&th.client, &written).await.updated_at,
        typed_into.updated_at,
        "a card that held its body is not written again"
    );

    // Opened and left by the server that started: nothing is kept again.
    let e = Editing {
        card_id: written.clone(),
        ..editing_app(&th).await
    };
    joined(e.app.collab(), &e.app, &e.card_id, 102).await;
    e.app
        .collab()
        .close(&e.app, &e.card_id)
        .await
        .expect("closed");
    let kept: Vec<Value> = versions(&th.client, &written).await;
    assert_eq!(kept.len(), 2, "{kept:?}");
    th.teardown().await;
}
