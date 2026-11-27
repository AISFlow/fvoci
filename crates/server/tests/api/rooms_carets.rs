//! The carets of a body edited together, over real connections: where each writer is
//! in the document is told to the room's others under the server's own word of whose
//! it is, and to nobody else.
//!
//! Nothing here differs between the databases but the test of two organizations,
//! which PostgreSQL alone holds: a caret is kept in the server's memory and never
//! stored.

use std::time::{Duration, Instant};

use fvoci_model::{ContainerMember, WorkspaceRole};
use serde_json::{Value, json};

use crate::common::{Setup, TEST_WORKSPACE_ID};
use crate::rooms::{Peer, QUIET, Told, awareness, give, paragraphs, table, two_in_a_room};

/// A caret as an editor's library says it: two places next to items of a document.
fn cursor(at: u64) -> Value {
    let place = |clock: u64| json!({"type": null, "tname": null, "item": {"client": 7, "clock": clock}, "assoc": 0});
    json!({"anchor": place(at), "head": place(at + 3)})
}

/// The same caret as the server tells it: without what is null.
fn told_cursor(at: u64) -> Value {
    let place = |clock: u64| json!({"item": {"client": 7, "clock": clock}, "assoc": 0});
    json!({"anchor": place(at), "head": place(at + 3)})
}

/// Shows the peer's caret at `at`, as an honest editor does.
async fn show(peer: &mut Peer, at: u64) {
    peer.send_caret(&awareness(
        peer.client(),
        1,
        &json!({"user": {}, "cursor": cursor(at)}),
    ))
    .await;
}

/// The state the others are told of a caret of `user_id` at `at`.
fn state_of(user_id: &str, at: u64) -> Value {
    json!({"user": {"id": user_id}, "cursor": told_cursor(at)})
}

/// Reads for a while and says that no caret came.
async fn no_caret(peer: &mut Peer, why: &str) {
    while peer.step(QUIET).await {}
    assert!(peer.carets.is_empty(), "{why}: {:?}", peer.carets);
}

/// A caret one member shows reaches the room's others and not its own connection. It
/// names the user of the connection that sent it, whatever the frame said of whose it
/// is, under a number and a clock that are the server's: nothing of the client's
/// number, clock, name or colour is sent on. Moved, and said again unmoved, it is
/// told under the same number with the next clock.
#[tokio::test]
async fn a_caret_reaches_the_others_as_the_servers_word_of_whose_it_is() {
    let th = Setup::native().start().await.init_basic().await;
    let (_table, mut one, mut two) = two_in_a_room(&th, &paragraphs(&["first"])).await;
    let (user1, user2) = (th.client.me().await.id, th.client2.me().await.id);

    // As somebody else: the other's user, a name, a colour, and the other's number.
    let forged = json!({
        "user": {"id": user2, "name": "Mallory", "color": "#ff0000"},
        "cursor": cursor(1),
        "role": "admin",
    });
    one.send_caret(&awareness(two.client(), 900, &forged)).await;
    let first = two.caret().await;
    assert_eq!(
        (first.doc, first.epoch, &first.state),
        (two.doc, two.epoch, &state_of(&user1, 1)),
        "the caret of the first user, and nothing else the frame said"
    );
    assert_eq!(first.clock, 1, "the server's first telling");
    assert!(
        first.number != two.client() && first.number != one.client(),
        "a number of the server's: {}",
        first.number
    );

    show(&mut one, 2).await;
    let moved = two.caret().await;
    assert_eq!(
        (moved.number, moved.clock, &moved.state),
        (first.number, 2, &state_of(&user1, 2))
    );
    // Said again where it is, as an editor does every fifteen seconds.
    show(&mut one, 2).await;
    let again = two.caret().await;
    assert_eq!((again.number, again.clock), (first.number, 3));

    // The second's own caret is another, and is the second user's.
    show(&mut two, 5).await;
    let theirs = one.caret().await;
    assert_eq!(theirs.state, state_of(&user2, 5));
    assert_ne!(theirs.number, first.number);
    no_caret(&mut two, "nobody is told their own caret").await;
    th.teardown().await;
}

/// Who reads is told the carets of who writes, and shows none: what a reader sends
/// of a caret goes to nobody.
#[tokio::test]
async fn a_reader_is_told_the_carets_and_shows_none() {
    let th = Setup::native().start().await.init_basic().await;
    let t = table(&th, &paragraphs(&["first"])).await;
    let (user1, user2) = (th.client.me().await.id, th.client2.me().await.id);
    give(&th, &t.board_id, &user2, "viewer").await;
    let mut writer = Peer::connect(&th.client, 101).await;
    let mut reader = Peer::connect(&th.client2, 102).await;
    writer.joined(TEST_WORKSPACE_ID, &t.card_id).await;
    let joined = reader.joined(TEST_WORKSPACE_ID, &t.card_id).await;
    assert_eq!(joined["mode"], json!("read"), "{joined}");

    show(&mut writer, 1).await;
    assert_eq!(reader.caret().await.state, state_of(&user1, 1));
    show(&mut reader, 4).await;
    reader.barrier(&th).await;
    no_caret(&mut writer, "a reader's caret is not sent on").await;

    // Let write, the same connection shows one; made a reader again, it is gone.
    give(&th, &t.board_id, &user2, "editor").await;
    assert_eq!(reader.text("docModeChanged").await["mode"], json!("write"));
    show(&mut reader, 4).await;
    let shown = writer.caret().await;
    assert_eq!(shown.state, state_of(&user2, 4));
    give(&th, &t.board_id, &user2, "viewer").await;
    let gone = writer.caret().await;
    assert_eq!(
        (gone.number, gone.clock, &gone.state),
        (shown.number, shown.clock + 1, &Value::Null),
        "who reads from now on has no caret"
    );
    th.teardown().await;
}

/// The others are told that a caret is gone when its member withdraws it, leaves the
/// room, is put out of it, or its connection ends; each under the caret's number with
/// the next clock. A member who showed none leaves without a word.
#[tokio::test]
async fn a_caret_is_gone_with_its_member() {
    let th = Setup::native().start().await.init_basic().await;
    let (t, mut one, mut two) = two_in_a_room(&th, &paragraphs(&["first"])).await;
    let user2 = th.client2.me().await.id;
    let gone = |shown: &Told, told: &Told| {
        assert_eq!(
            (told.number, told.clock, &told.state),
            (shown.number, shown.clock + 1, &Value::Null),
            "the caret is gone"
        );
    };

    // Withdrawn: the editor lost the focus.
    show(&mut two, 1).await;
    let shown = one.caret().await;
    two.send_caret(&awareness(
        two.client(),
        2,
        &json!({"user": {}, "cursor": null}),
    ))
    .await;
    gone(&shown, &one.caret().await);
    // Withdrawn again, there is nothing to tell.
    two.send_caret(&awareness(two.client(), 3, &Value::Null))
        .await;
    two.barrier(&th).await;
    no_caret(&mut one, "a caret that is not shown is not withdrawn").await;

    // In the room anew without leaving it: the caret of the member before is gone.
    show(&mut two, 2).await;
    let shown = one.caret().await;
    two.joined(TEST_WORKSPACE_ID, &t.card_id).await;
    gone(&shown, &one.caret().await);

    // Left.
    show(&mut two, 2).await;
    let anew = one.caret().await;
    assert_ne!(anew.number, shown.number, "a member anew has a caret anew");
    two.leave().await;
    gone(&anew, &one.caret().await);

    // Back in the room: put out of it.
    two.joined(TEST_WORKSPACE_ID, &t.card_id).await;
    show(&mut two, 3).await;
    let again = one.caret().await;
    th.client
        .delete(&format!(
            "/w/{TEST_WORKSPACE_ID}/containers/{}/members/{user2}",
            t.board_id
        ))
        .await
        .check_no_content();
    two.text("docLeft").await;
    gone(&again, &one.caret().await);

    // Its connection ended.
    give(&th, &t.board_id, &user2, "editor").await;
    let mut three = Peer::connect(&th.client2, 103).await;
    three.joined(TEST_WORKSPACE_ID, &t.card_id).await;
    show(&mut three, 4).await;
    let shown = one.caret().await;
    drop(three);
    gone(&shown, &one.caret().await);

    // Who showed none leaves without a word.
    let mut four = Peer::connect(&th.client2, 104).await;
    four.joined(TEST_WORKSPACE_ID, &t.card_id).await;
    four.leave().await;
    four.barrier(&th).await;
    no_caret(&mut one, "no caret, nothing gone").await;
    th.teardown().await;
}

/// A caret is told in its room alone: not to the room of another card, also where one
/// connection is in both, and under the number each receiver knows the room as.
#[tokio::test]
async fn a_caret_stays_in_its_room() {
    let th = Setup::native().start().await.init_basic().await;
    let (_table, mut one, mut two) = two_in_a_room(&th, &paragraphs(&["first"])).await;
    let other = table(&th, &paragraphs(&["another card"])).await;
    let user2 = th.client2.me().await.id;
    give(&th, &other.board_id, &user2, "editor").await;
    // The second user's other connection is in the other card's room only.
    let mut elsewhere = Peer::connect(&th.client2, 103).await;
    elsewhere.joined(TEST_WORKSPACE_ID, &other.card_id).await;
    // The first connection is in both rooms: the first card's is its room 1.
    let first_room = one.doc;
    one.joined(TEST_WORKSPACE_ID, &other.card_id).await;
    let second_room = one.doc;

    // A caret of the first card's room, sent as of it.
    one.doc = first_room;
    show(&mut one, 1).await;
    assert_eq!(two.caret().await.doc, two.doc);
    one.barrier(&th).await;
    no_caret(&mut elsewhere, "the caret of another card's room").await;

    // And one of the other card's room goes there alone.
    one.doc = second_room;
    show(&mut one, 2).await;
    assert_eq!(elsewhere.caret().await.doc, elsewhere.doc);
    one.barrier(&th).await;
    no_caret(&mut two, "the caret of another card's room").await;
    th.teardown().await;
}

/// A caret of a card of one organization reaches nobody in a room of another, also
/// where one user is in a room of each.
#[tokio::test]
async fn a_caret_stays_in_its_organization() {
    use fvoci_model::{Block, BlockType};
    // SQLite holds one organization.
    if fvoci_store::testing::db_type_of_the_tests() != fvoci_store::DbType::Postgres {
        return;
    }
    let th = Setup::native().start().await.init_basic().await;
    let (_table, mut one, mut two) = two_in_a_room(&th, &paragraphs(&["first"])).await;
    let (theirs, workspace_id, board_id) = crate::workspaces::another_organization(&th).await;
    let members = theirs.get_members_for_container(&board_id).await;
    let owner = members.expect("their members")[0].user_id.clone();
    let mut theirs_card = Block {
        id: fvoci_model::new_id(),
        container_id: board_id.clone(),
        block_type: BlockType::Card,
        title: "theirs".to_owned(),
        ..Block::default()
    };
    theirs
        .insert_block(&mut theirs_card, &owner)
        .await
        .expect("their card is stored");
    let user2 = th.client2.me().await.id;
    theirs
        .add_workspace_member(&workspace_id, &user2, WorkspaceRole::Guest)
        .await
        .expect("let into their workspace");
    let member = ContainerMember {
        container_id: board_id,
        user_id: user2,
        role: fvoci_model::ContainerRole::Editor,
        synthetic: false,
        inherited_role: None,
    };
    theirs.save_member(&member).await.expect("invited");
    let mut there = Peer::connect(&th.client2, 103).await;
    there.joined(&workspace_id, &theirs_card.id).await;

    show(&mut one, 1).await;
    two.caret().await;
    show(&mut there, 2).await;
    there.barrier(&th).await;
    one.barrier(&th).await;
    no_caret(&mut there, "a caret of our organization's card").await;
    no_caret(&mut one, "a caret of their organization's card").await;
    no_caret(&mut two, "a caret of their organization's card").await;
    th.teardown().await;
}

/// Who comes into a room is told where the carets of those in it are, after the
/// document; not the caret of who withdrew theirs.
#[tokio::test]
async fn who_comes_in_is_told_the_carets_there_are() {
    let th = Setup::native().start().await.init_basic().await;
    let (t, mut one, mut two) = two_in_a_room(&th, &paragraphs(&["first"])).await;
    let user1 = th.client.me().await.id;
    show(&mut one, 1).await;
    let shown = two.caret().await;
    show(&mut two, 2).await;
    one.caret().await;
    two.send_caret(&awareness(two.client(), 2, &json!({"cursor": null})))
        .await;
    one.caret().await;

    let mut late = Peer::connect(&th.client2, 103).await;
    late.joined(TEST_WORKSPACE_ID, &t.card_id).await;
    let told = late.caret().await;
    assert_eq!(
        (told.number, told.clock, &told.state),
        (shown.number, shown.clock, &state_of(&user1, 1)),
        "the caret as its room last told it"
    );
    no_caret(&mut late, "the one caret that is shown").await;
    th.teardown().await;
}

/// A caret frame that is not one caret of the room's document is dropped, and the
/// room goes on: too large, more than one state, a place that is none, another
/// making of the document. Nobody is put out for it.
#[tokio::test]
async fn what_is_no_caret_of_the_room_is_dropped() {
    let th = Setup::native().start().await.init_basic().await;
    let (_table, mut one, mut two) = two_in_a_room(&th, &paragraphs(&["first"])).await;
    let client = one.client();
    let state = |cursor: Value| json!({"user": {}, "cursor": cursor});

    // Larger than a caret is: an update of 1,025 bytes.
    let padded = |bytes: usize| {
        let name =
            |length: usize| json!({"user": {"name": "x".repeat(length)}, "cursor": cursor(1)});
        let empty = awareness(client, 1, &name(0)).len();
        awareness(client, 1, &name(bytes - empty))
    };
    assert_eq!(padded(1025).len(), 1025);
    one.send_caret(&padded(1025)).await;
    // A place that names nothing, and one in a root type that is not the body's.
    let mut nowhere = cursor(1);
    nowhere["head"] = json!({"assoc": 0});
    one.send_caret(&awareness(client, 1, &state(nowhere))).await;
    let mut elsewhere = cursor(1);
    elsewhere["head"] = json!({"tname": "another"});
    one.send_caret(&awareness(client, 1, &state(elsewhere)))
        .await;
    // Two states in one update.
    let mut two_states = awareness(client, 1, &state(cursor(1)));
    two_states[0] = 2;
    two_states.extend_from_slice(&awareness(client + 1, 1, &state(cursor(1)))[1..]);
    one.send_caret(&two_states).await;
    // A place with what no place has, and a caret with one end.
    let mut odd = cursor(1);
    odd["anchor"]["user"] = json!("somebody");
    one.send_caret(&awareness(client, 1, &state(odd))).await;
    one.send_caret(&awareness(
        client,
        1,
        &state(json!({"anchor": cursor(1)["anchor"]})),
    ))
    .await;
    // Bytes that are no update.
    one.send_caret(&[0xff, 0xff, 0xff]).await;
    // Of another making of the document.
    let epoch = one.epoch;
    one.epoch = epoch + 1;
    show(&mut one, 1).await;
    one.epoch = epoch;
    one.barrier(&th).await;
    no_caret(&mut two, "none of these is a caret of the room").await;

    // The room goes on: a caret of 1,024 bytes is told, and an update taken.
    one.send_caret(&padded(1024)).await;
    assert_eq!(two.caret().await.clock, 1, "the first that was one");
    one.write_acked("second").await;
    th.teardown().await;
}

/// Caret frames count among the frames a connection may send in a second, and one
/// past that is dropped: the member stays in the room, which missed nothing of the
/// document, and the others were told no more carets than the rate lets through.
#[tokio::test]
async fn carets_past_the_rate_are_dropped_and_put_nobody_out() {
    let th = Setup::native()
        .configure(|config| config.collab.collab_frames_per_second = 10)
        .start()
        .await
        .init_basic()
        .await;
    // As `two_in_a_room`, with the clock read where the first connection's allowance
    // begins to be spent: it may send ten frames at once and ten more for every
    // second since. (What it did not send before is not saved past those ten.)
    let t = table(&th, &paragraphs(&["first"])).await;
    let user2 = th.client2.me().await.id;
    give(&th, &t.board_id, &user2, "editor").await;
    let mut one = Peer::connect(&th.client, 101).await;
    let mut two = Peer::connect(&th.client2, 102).await;
    let began = Instant::now();
    one.joined(TEST_WORKSPACE_ID, &t.card_id).await;
    two.joined(TEST_WORKSPACE_ID, &t.card_id).await;
    // The joining was five of the ten; these are sixty more at once.
    for at in 0..60 {
        show(&mut one, at).await;
    }
    one.barrier(&th).await;
    // Twenty where the sixty took under a second to send; a loaded machine takes
    // longer, and the rate lets through what it does in that time, no more.
    let seconds = usize::try_from(began.elapsed().as_secs()).expect("a count of seconds");
    let most = 10 * (seconds + 2);
    while two.step(QUIET).await {}
    let told = two.carets.len();
    assert!(
        (1..=most).contains(&told),
        "what the rate let through in {seconds} s (at most {most}), and not sixty: {told}"
    );
    assert!(
        one.unread()
            .iter()
            .all(|message| message["type"] != json!("docLeft")),
        "nobody is put out for carets: {:?}",
        one.unread()
    );
    // After a pause the connection writes as before.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    one.write_acked("second").await;
    th.teardown().await;
}

/// The document made again leaves no caret: each was a place in the document before.
/// Who comes in then is told none, and a caret of the new document is told anew.
#[tokio::test]
async fn the_document_made_again_leaves_no_caret() {
    let th = Setup::native().start().await.init_basic().await;
    let (t, mut one, mut two) = two_in_a_room(&th, &paragraphs(&["first"])).await;
    show(&mut one, 1).await;
    let before = two.caret().await;
    crate::rooms::write_whole(&th.client, &t.card_id, &paragraphs(&["written whole"])).await;
    one.text("docReset").await;
    two.text("docReset").await;
    one.settle().await;
    no_caret(
        &mut two,
        "nothing is told of a caret of the document before",
    )
    .await;

    let mut late = Peer::connect(&th.client2, 103).await;
    late.joined(TEST_WORKSPACE_ID, &t.card_id).await;
    no_caret(&mut late, "a caret of the document before").await;
    show(&mut one, 2).await;
    let after = two.caret().await;
    assert_eq!(
        (after.epoch, after.number),
        (two.epoch, before.number),
        "the member's caret, in the new document"
    );
    assert!(after.clock > before.clock);
    late.caret().await;
    th.teardown().await;
}

/// A member an immediate check holds is told nothing of the carets while the store
/// is asked of them. Kept, they are told at once where each caret is and which went
/// meanwhile, after what the check says of them; put out, they are told none.
#[tokio::test]
async fn a_held_member_is_told_no_caret_until_the_check_keeps_them() {
    let th = Setup::native()
        .configure(|config| config.collab.collab_recheck_seconds = 30)
        .start()
        .await
        .init_basic()
        .await;
    let (t, mut one, mut two) = two_in_a_room(&th, &paragraphs(&["first"])).await;
    let (user1, user2) = (th.client.me().await.id, th.client2.me().await.id);
    // The first user's other tab, with a caret both are told.
    let mut tab = Peer::connect(&th.client, 103).await;
    tab.joined(TEST_WORKSPACE_ID, &t.card_id).await;
    show(&mut tab, 7).await;
    let of_tab = two.caret().await;
    assert_eq!(one.caret().await.number, of_tab.number);

    // Made a reader: kept, and held while the store is asked.
    let (reached, go) = th.app.collab().pause_next_access_read();
    let meanwhile = async {
        reached.notified().await;
        show(&mut one, 1).await;
        one.barrier(&th).await;
        tab.leave().await;
        assert_eq!(one.caret().await.state, Value::Null, "the tab's caret went");
        no_caret(&mut two, "held, they are told no caret").await;
        go.notify_one();
    };
    tokio::join!(give(&th, &t.board_id, &user2, "viewer"), meanwhile);
    assert_eq!(two.text("docModeChanged").await["mode"], json!("read"));
    let shown = two.caret().await;
    assert_eq!(shown.state, state_of(&user1, 1), "where the caret is now");
    let gone = two.caret().await;
    assert_eq!(
        (gone.number, gone.clock, &gone.state),
        (of_tab.number, of_tab.clock + 1, &Value::Null),
        "and the one that went meanwhile"
    );

    // Taken off the board: put out, and told nothing shown meanwhile.
    let (reached, go) = th.app.collab().pause_next_access_read();
    let meanwhile = async {
        reached.notified().await;
        show(&mut one, 2).await;
        one.barrier(&th).await;
        go.notify_one();
    };
    let removed = async {
        th.client
            .delete(&format!(
                "/w/{TEST_WORKSPACE_ID}/containers/{}/members/{user2}",
                t.board_id
            ))
            .await
            .check_no_content();
    };
    tokio::join!(removed, meanwhile);
    two.text("docLeft").await;
    no_caret(&mut two, "put out, they were told no caret shown after").await;
    th.teardown().await;
}

/// A page's room tells carets as a card's does: who writes the page is told under
/// the server's word of whose it is, and who only reads the wiki is told and shows
/// none.
#[tokio::test]
async fn a_caret_goes_through_the_room_of_a_page() {
    let th = Setup::native().start().await.init_basic().await;
    let d = crate::page_rooms::desk(&th, &paragraphs(&["first"])).await;
    let (user1, user2) = (th.client.me().await.id, th.client2.me().await.id);
    give(&th, &d.wiki_id, &user2, "viewer").await;
    let mut writer = Peer::connect(&th.client, 101).await;
    let mut reader = Peer::connect(&th.client2, 102).await;
    writer.joined(TEST_WORKSPACE_ID, &d.page_id).await;
    let joined = reader.joined(TEST_WORKSPACE_ID, &d.page_id).await;
    assert_eq!(joined["mode"], json!("read"), "{joined}");

    // As the reader, under the reader's number: told as the writer's.
    let forged = json!({"user": {"id": user2, "name": "Mallory"}, "cursor": cursor(1)});
    writer
        .send_caret(&awareness(reader.client(), 900, &forged))
        .await;
    let shown = reader.caret().await;
    assert_eq!(shown.state, state_of(&user1, 1));
    show(&mut reader, 4).await;
    reader.barrier(&th).await;
    no_caret(&mut writer, "a reader of the wiki shows no caret").await;
    writer.leave().await;
    let gone = reader.caret().await;
    assert_eq!(
        (gone.number, gone.clock, &gone.state),
        (shown.number, shown.clock + 1, &Value::Null),
        "gone with its writer"
    );
    th.teardown().await;
}

/// A caret is no change of the body: two who only showed theirs leave a room, and no
/// version is kept of it.
#[tokio::test]
async fn a_caret_alone_keeps_no_version() {
    let th = Setup::native().start().await.init_basic().await;
    let (t, mut one, mut two) = two_in_a_room(&th, &paragraphs(&["first"])).await;
    let before = crate::body_versions::versions(&th.client, &t.card_id).await;
    assert_eq!(
        before,
        Vec::<Value>::new(),
        "a body nobody typed in has none"
    );
    show(&mut one, 1).await;
    two.caret().await;
    show(&mut two, 2).await;
    one.caret().await;
    one.leave().await;
    two.leave().await;
    crate::rooms::no_rooms(&th).await;
    assert_eq!(
        crate::body_versions::versions(&th.client, &t.card_id).await,
        before,
        "nothing was typed"
    );
    th.teardown().await;
}
