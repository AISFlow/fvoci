//! Keeping a card's body as a version now, as an editor asks (`keepCardBodyVersion`,
//! `keepPageBodyVersion`): what is kept, what is said when nothing is, how it sits with
//! the versions the server keeps on its own, and who may ask.

use std::time::Duration;

use fvoci_collab::testing::Editor;
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::body_versions::{lines, versions};
use crate::collab::{Editing, editing, joined};
use crate::common::{Client, ROOT_WS, Setup, TEST_WS};
use crate::rooms::{give, paragraphs, table, two_in_a_room, write_whole};
use crate::wiki_isolation::{put_on, wiki_of};

/// What a client is answered as it asks to keep the card's body now.
async fn keep_now(client: &Client, card_id: &str) -> Value {
    client
        .post_empty(&format!("{TEST_WS}/cards/{card_id}/body-versions"))
        .await
        .ok_json()
}

/// The outcome of an answer, and the number of the version it names.
fn outcome(answer: &Value) -> (&str, Option<i64>) {
    (
        answer["outcome"].as_str().expect("an outcome"),
        answer["version"]["version"].as_i64(),
    )
}

/// The numbers and causes of the versions of a card, the newest first.
fn causes(kept: &[Value]) -> Vec<(i64, &str)> {
    kept.iter()
        .map(|version| {
            (
                version["version"].as_i64().expect("a number"),
                version["cause"].as_str().expect("a cause"),
            )
        })
        .collect()
}

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

/// Two edit a card together and one asks to keep it: the version is the body both
/// typed, by both, and the room goes on as it was. Asked again it keeps nothing, and
/// says so; within the minute after, nor does what was typed since, which the server
/// keeps on its own as its writers leave.
#[tokio::test]
async fn what_two_typed_together_is_kept_as_one_asks() {
    let th = Setup::native().start().await.init_basic().await;
    let (t, mut one, mut two) = two_in_a_room(&th, &paragraphs(&["first"])).await;
    let (user1, user2) = (th.client.me().await.id, th.client2.me().await.id);
    one.write_acked("one").await;
    two.write_acked("two").await;

    let kept = keep_now(&th.client, &t.card_id).await;
    assert_eq!(outcome(&kept), ("kept", Some(2)), "{kept}");
    let mut by_both = vec![user1.clone(), user2.clone()];
    by_both.sort();
    assert_eq!(kept["version"]["editors"], json!(by_both), "{kept}");
    assert_eq!(kept["version"]["cause"], "manual");
    assert_eq!(kept["version"]["isCurrent"], true);
    let listed = versions(&th.client, &t.card_id).await;
    assert_eq!(listed[0], kept["version"], "as the list has it");
    assert_eq!(causes(&listed), [(2, "manual"), (1, "auto")]);
    assert_eq!(
        lines(&th.client, &t.card_id, &listed[0]).await,
        ["first", "one", "two"]
    );

    let again = keep_now(&th.client2, &t.card_id).await;
    assert_eq!(outcome(&again), ("unchanged", Some(2)), "{again}");

    // The room goes on as it was: what is typed is taken under the same document.
    two.write_acked("three").await;
    one.settle().await;
    assert!(
        one.unread()
            .iter()
            .chain(two.unread().iter())
            .all(|told| told["type"] != "docReset"),
        "nobody is sent the document again: {:?} {:?}",
        one.unread(),
        two.unread()
    );
    let soon = keep_now(&th.client, &t.card_id).await;
    assert_eq!(outcome(&soon), ("recent", Some(2)), "{soon}");
    assert_eq!(versions(&th.client, &t.card_id).await.len(), 2);

    one.leave().await;
    two.leave().await;
    let left = crate::body_versions::kept_after(&th.client, &t.card_id, 3).await;
    assert_eq!(causes(&left), [(3, "auto"), (2, "manual"), (1, "auto")]);
    assert_eq!(left[0]["editors"], json!([user2]));
    th.teardown().await;
}

/// A body kept by hand is not kept again as its writers leave, nor as the room closes:
/// it is the newest version.
#[tokio::test]
async fn a_body_kept_by_hand_is_not_kept_again_as_its_writers_leave() {
    let th = Setup::native().start().await.init_basic().await;
    let e = editing(&th, &paragraphs(&["first"])).await;
    let collab = e.app.collab();
    collab.leave_due_writes();
    let (editor, epoch) = joined(collab, &e.app, &e.card_id, 101).await;
    typed(&e, &editor, epoch, &e.user1, "typed").await;

    let kept = keep_now(&th.client, &e.card_id).await;
    assert_eq!(outcome(&kept), ("kept", Some(2)), "{kept}");
    collab.close(&e.app, &e.card_id).await.expect("closed");
    let after = versions(&th.client, &e.card_id).await;
    assert_eq!(causes(&after), [(2, "manual"), (1, "auto")]);
    assert_eq!(after[0]["isCurrent"], true);
    th.teardown().await;
}

/// The clock that keeps a body edited on counts again from a version kept by hand: one
/// kept when the clock was due is by hand, not the server's first, and the next the
/// clock keeps comes the interval after the first update after it. A minute after the
/// last version kept by hand, another is kept by hand.
#[tokio::test]
async fn the_clock_counts_again_from_a_version_kept_by_hand() {
    let th = Setup::native()
        .configure(|config| config.collab.collab_version_minutes = 2)
        .start()
        .await
        .init_basic()
        .await;
    let e = editing(&th, &paragraphs(&["first"])).await;
    let collab = e.app.collab();
    // Written only when asked: a write the machine was late for would find the clock
    // due first.
    collab.leave_due_writes();
    let (editor, epoch) = joined(collab, &e.app, &e.card_id, 101).await;
    typed(&e, &editor, epoch, &e.user1, "one").await;
    collab.pass_version_time(Duration::from_mins(3));

    let kept = keep_now(&th.client, &e.card_id).await;
    assert_eq!(outcome(&kept), ("kept", Some(2)), "{kept}");
    assert_eq!(
        causes(&versions(&th.client, &e.card_id).await),
        [(2, "manual"), (1, "auto")],
        "the version due is the one asked for"
    );

    typed(&e, &editor, epoch, &e.user1, "two").await;
    collab
        .write_body(&e.app, &e.card_id)
        .await
        .expect("written");
    collab.pass_version_time(Duration::from_secs(90));
    typed(&e, &editor, epoch, &e.user1, "three").await;
    collab
        .write_body(&e.app, &e.card_id)
        .await
        .expect("written");
    assert_eq!(
        versions(&th.client, &e.card_id).await.len(),
        2,
        "not due yet: the interval counts from the first update after it"
    );
    collab.pass_version_time(Duration::from_mins(1));
    typed(&e, &editor, epoch, &e.user1, "four").await;
    collab
        .write_body(&e.app, &e.card_id)
        .await
        .expect("written");
    let kept = versions(&th.client, &e.card_id).await;
    assert_eq!(causes(&kept), [(3, "auto"), (2, "manual"), (1, "auto")]);
    assert_eq!(
        lines(&th.client, &e.card_id, &kept[0]).await,
        ["first", "one", "two", "three"],
        "the body before the update that found it due"
    );

    // Past the minute after the one kept by hand.
    let again = keep_now(&th.client, &e.card_id).await;
    assert_eq!(outcome(&again), ("kept", Some(4)), "{again}");
    collab.close(&e.app, &e.card_id).await.expect("closed");
    th.teardown().await;
}

/// The version names who wrote since the one before, not who asks: nobody for a body
/// nobody is known to have written, which is then kept as of when it was asked for. No
/// body and no version keep nothing, and name no version. A card that is not open, with
/// a body that is its newest version, keeps nothing.
#[tokio::test]
async fn who_asks_is_not_named_for_asking() {
    let th = Setup::native().start().await.init_basic().await;
    let e = editing(&th, &paragraphs(&["first"])).await;
    let before = fvoci_server::auth::now_millis();
    let kept = keep_now(&th.client, &e.card_id).await;
    assert_eq!(outcome(&kept), ("kept", Some(1)), "{kept}");
    assert_eq!(kept["version"]["editors"], json!([]), "{kept}");
    assert!(
        kept["version"]["keptAt"].as_i64() >= Some(before),
        "as of when it was asked for: {kept}"
    );

    // Typed by the second user, asked for by the first, past the minute.
    let collab = e.app.collab();
    collab.leave_due_writes();
    let (editor, epoch) = joined(collab, &e.app, &e.card_id, 101).await;
    typed(&e, &editor, epoch, &e.user2, "theirs").await;
    collab.pass_version_time(Duration::from_secs(61));
    let kept = keep_now(&th.client, &e.card_id).await;
    assert_eq!(outcome(&kept), ("kept", Some(2)), "{kept}");
    assert_eq!(kept["version"]["editors"], json!([e.user2]), "{kept}");
    collab.close(&e.app, &e.card_id).await.expect("closed");

    // Written whole, with no room: its versions are the store's, and nothing is left.
    write_whole(&th.client, &e.card_id, &paragraphs(&["whole"])).await;
    let whole = keep_now(&th.client, &e.card_id).await;
    assert_eq!(outcome(&whole), ("unchanged", Some(3)), "{whole}");
    assert_eq!(whole["version"]["cause"], "auto");

    let empty = table(&th, &Value::Null).await;
    let none = keep_now(&th.client, &empty.card_id).await;
    assert_eq!(none, json!({"outcome": "unchanged", "version": null}));
    th.teardown().await;
}

/// Only who edits the board keeps a version: not its viewers or commenters, not who is
/// no member of it, not a reader by its link, not through another workspace, and not
/// of a card in the trash. The role table holds the rest (`role_table.rs`).
#[tokio::test]
async fn only_who_edits_the_board_keeps_a_version() {
    let th = Setup::native().start().await.init_basic().await;
    let t = table(&th, &paragraphs(&["first"])).await;
    let user2 = th.client2.me().await.id;
    let at = format!("{TEST_WS}/cards/{}/body-versions", t.card_id);
    th.client2
        .post_empty(&at)
        .await
        .check_error(StatusCode::NOT_FOUND, "card not found");
    for role in ["viewer", "commenter"] {
        give(&th, &t.board_id, &user2, role).await;
        th.client2.post_empty(&at).await.check_error(
            StatusCode::FORBIDDEN,
            "access denied to keep a version of the card's body",
        );
    }
    let token = th.share(&t.board_id).await;
    th.anonymous()
        .post_empty(&format!("{at}?readToken={token}"))
        .await
        .check_unauthorized();
    th.client
        .post_empty(&at.replace(TEST_WS, ROOT_WS))
        .await
        .check_error(StatusCode::NOT_FOUND, "card not found");
    assert_eq!(versions(&th.client, &t.card_id).await, Vec::<Value>::new());

    give(&th, &t.board_id, &user2, "editor").await;
    assert_eq!(outcome(&keep_now(&th.client2, &t.card_id).await).0, "kept");
    crate::blocks::trash_card(&th.client, &t.board_id, &t.card_id)
        .await
        .check_no_content();
    th.client
        .post_empty(&at)
        .await
        .check_error(StatusCode::NOT_FOUND, "card not found");
    th.teardown().await;
}

/// A page's body is kept now as a card's is, for who edits the wiki; a page in the trash
/// is not there.
#[tokio::test]
async fn a_pages_body_is_kept_now_by_who_edits_the_wiki() {
    let th = Setup::native().start().await.init_basic().await;
    let (wiki, page) = wiki_of(&th.client, "private").await;
    let at = format!("{TEST_WS}/wikis/{wiki}/pages/{page}");
    let read = th.client.get(&at).await.value();
    th.client
        .patch(
            &at,
            &json!({"readAt": read["updatedAt"], "body": paragraphs(&["paged"])}),
        )
        .await
        .check_ok();
    let keep = format!("{at}/body-versions");
    let unchanged: Value = th.client.post_empty(&keep).await.ok_json();
    assert_eq!(outcome(&unchanged), ("unchanged", Some(1)), "{unchanged}");

    let user2 = th.client2.me().await.id;
    put_on(&th.client, &wiki, &user2, "viewer").await;
    th.client2.post_empty(&keep).await.check_error(
        StatusCode::FORBIDDEN,
        "access denied to keep a version of the page's body",
    );
    // Through the card's path, a page is no card.
    th.client
        .post_empty(&format!("{TEST_WS}/cards/{page}/body-versions"))
        .await
        .check_error(StatusCode::NOT_FOUND, "card not found");

    th.client.delete(&at).await.check_no_content();
    th.client.post_empty(&keep).await.check_not_found();
    th.teardown().await;
}

/// A diary entry's body is kept by hand in the user's own space with the same cooldown,
/// unchanged/recent/kept outcomes, and isolation from other routes and workspaces.
#[tokio::test]
async fn a_diary_entrys_body_is_kept_now_with_cooldown_and_isolation() {
    let th = Setup::native().start().await.init_basic().await;
    let (_, own) = crate::own_space::workspaces_and_own(&th.client).await;
    let ws = format!("/w/{}", own.id);
    let diary_val = th
        .client
        .post(&format!("{ws}/diaries"), &json!({"title": "Journal"}))
        .await
        .created_json::<Value>();
    let diary_id = diary_val["id"].as_str().expect("diary id").to_owned();
    let entry_val = th
        .client
        .post(
            &format!("{ws}/diaries/{diary_id}/entries"),
            &json!({"date": "2024-11-21", "mood": "good"}),
        )
        .await
        .created_json::<Value>();
    let entry_id = entry_val["id"].as_str().expect("entry id").to_owned();
    let entry_at = format!("{ws}/diaries/{diary_id}/entries/{entry_id}");
    let keep = format!("{entry_at}/body-versions");

    // Empty entry: unchanged with no version.
    let none: Value = th.client.post_empty(&keep).await.ok_json();
    assert_eq!(none, json!({"outcome": "unchanged", "version": null}));

    // Write via collab room, keep by hand, verify cooldown and unchanged.
    let mut peer = crate::rooms::Peer::connect(&th.client, 101).await;
    peer.joined(&own.id, &entry_id).await;
    peer.write_acked("first thought").await;

    let kept: Value = th.client.post_empty(&keep).await.ok_json();
    assert_eq!(outcome(&kept), ("kept", Some(1)), "{kept}");
    assert_eq!(kept["version"]["cause"], "manual");
    assert_eq!(kept["version"]["isCurrent"], true);

    let unchanged: Value = th.client.post_empty(&keep).await.ok_json();
    assert_eq!(outcome(&unchanged), ("unchanged", Some(1)), "{unchanged}");

    peer.write_acked("second thought").await;
    let recent: Value = th.client.post_empty(&keep).await.ok_json();
    assert_eq!(outcome(&recent), ("recent", Some(1)), "{recent}");

    th.app.collab().pass_version_time(Duration::from_secs(61));
    let kept_again: Value = th.client.post_empty(&keep).await.ok_json();
    assert_eq!(outcome(&kept_again), ("kept", Some(2)), "{kept_again}");
    assert_eq!(kept_again["version"]["cause"], "manual");
    peer.leave().await;

    // Isolation: card route, organization workspace, another user, and trashed entry are 404.
    th.client
        .post_empty(&format!("{ws}/cards/{entry_id}/body-versions"))
        .await
        .check_not_found();
    th.client
        .post_empty(&format!(
            "{TEST_WS}/diaries/{diary_id}/entries/{entry_id}/body-versions"
        ))
        .await
        .check_not_found();
    th.client2.post_empty(&keep).await.check_not_found();

    th.client.delete(&entry_at).await.check_no_content();
    th.client.post_empty(&keep).await.check_not_found();
    th.teardown().await;
}

/// If a card with unwritten collaborative updates is deleted before `keep_by_hand` or
/// `write_whole` writes those updates back, the missing card surfaces as 404 `NotFound`
/// rather than 503 `document_unavailable`.
#[tokio::test]
async fn deleting_a_card_with_unwritten_updates_surfaces_not_found_not_unavailable() {
    let th = Setup::native().start().await.init_basic().await;
    let e = editing(&th, &paragraphs(&["first"])).await;
    let collab = e.app.collab();
    collab.leave_due_writes();
    let (editor, epoch) = joined(collab, &e.app, &e.card_id, 101).await;
    typed(&e, &editor, epoch, &e.user1, "unsaved").await;

    // Delete the card directly in the store while its collab document still holds
    // unwritten updates.
    e.app
        .store()
        .delete_block(&e.card_id, &e.user1)
        .await
        .expect("card deleted");

    let error = collab
        .keep_by_hand(&e.app, &e.card_id)
        .await
        .expect_err("deleted card cannot keep a version");
    assert!(
        error.is_not_found(),
        "expected 404 NotFound, got {:?} (reason {:?}): {error}",
        error.kind(),
        error.reason()
    );

    let write_error = collab
        .write_whole(
            &e.app,
            vec![(e.card_id.clone(), String::new())],
            |_app| async { Ok(()) },
        )
        .await
        .expect_err("deleted card cannot be written whole");
    assert!(
        write_error.is_not_found(),
        "expected 404 NotFound from write_whole, got {:?} (reason {:?}): {write_error}",
        write_error.kind(),
        write_error.reason()
    );

    // Unknown card IDs return 404 without creating an empty `""` gate or slot key, and
    // once idle `gates` and `structures` prune back to empty.
    let missing_id = fvoci_model::new_id();
    assert!(
        collab
            .open(&e.app, &missing_id)
            .await
            .unwrap_err()
            .is_not_found(),
        "opening missing card must return 404"
    );
    assert!(
        collab
            .write_whole(&e.app, vec![(missing_id, String::new())], |_app| async {
                Ok(())
            })
            .await
            .unwrap_err()
            .is_not_found(),
        "writing missing card must return 404"
    );
    drop(editor);
    e.app
        .finish_background(std::time::Duration::from_secs(2))
        .await;
    assert_eq!(
        collab.lock_and_slot_stats(),
        (0, 0, false),
        "gates and structures must prune to empty and never insert an empty container key"
    );
    th.teardown().await;
}
