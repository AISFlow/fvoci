//! Real sockets: board visits, open cards, deduplication, token readers and boundaries.

use fvoci_model::PublicLink;
use serde_json::{Value, json};

use crate::common::{Setup, TEST_WORKSPACE_ID, TEST_WS};
use crate::rooms::{give, paragraphs, table};
use crate::websocket::{PATIENCE, QUIET, Socket, check_message, read_contract};

pub(crate) async fn visit(socket: &mut Socket, board: &str, card: &str) {
    socket
        .send(
            json!({"type":"setPresence", "workspaceId":TEST_WORKSPACE_ID,
        "containerId":board, "cardId":card}),
        )
        .await;
}

pub(crate) async fn people(socket: &mut Socket, count: usize) -> Value {
    for _ in 0..20 {
        let message = socket.next_json(PATIENCE).await.expect("presence arrives");
        assert_eq!(
            message["type"], "presenceChanged",
            "only presence is subscribed"
        );
        if message["people"].as_array().expect("people").len() == count {
            return message;
        }
    }
    panic!("the expected people never arrived");
}

#[tokio::test]
async fn visits_show_cards_deduplicate_tabs_and_leave_on_disconnect() {
    let th = Setup::native().start().await.init_basic().await;
    let t = table(&th, &paragraphs(&["hello"])).await;
    let other = th.client2.me().await.id;
    give(&th, &t.board_id, &other, "viewer").await;
    let mut one = Socket::connect_as(&th.client).await;
    visit(&mut one, &t.board_id, "").await;
    let initial = people(&mut one, 1).await;
    check_message(
        &read_contract("websocket.json"),
        &read_contract("openapi.json"),
        &initial,
    );
    let mut two = Socket::connect_as(&th.client2).await;
    visit(&mut two, &t.board_id, &t.card_id).await;
    let shown = people(&mut one, 2).await;
    let person = shown["people"]
        .as_array()
        .expect("people")
        .iter()
        .find(|person| person["userId"] == other)
        .expect("the other reader");
    assert_eq!(
        person["cardIds"],
        json!([t.card_id]),
        "the open card is named"
    );
    let mut duplicate = Socket::connect_as(&th.client2).await;
    visit(&mut duplicate, &t.board_id, &t.card_id).await;
    let _ = people(&mut duplicate, 2).await;
    assert!(
        one.next_json(QUIET).await.is_none(),
        "a duplicate tab changes no roster"
    );
    drop(duplicate);
    assert!(
        one.next_json(QUIET).await.is_none(),
        "the user's other tab remains"
    );
    visit(&mut two, &t.board_id, "").await;
    let closed = people(&mut one, 2).await;
    assert!(
        closed["people"]
            .as_array()
            .expect("people")
            .iter()
            .all(|person| person["cardIds"] == json!([])),
        "closing the card clears its readers"
    );
    drop(two);
    let _ = people(&mut one, 1).await;
    assert!(
        one.next_json(QUIET).await.is_none(),
        "unchanged rosters are quiet"
    );
    th.teardown().await;
}

#[tokio::test]
async fn token_visits_count_anonymously_and_receive_nothing() {
    let th = Setup::local_mode().start().await.init_basic().await;
    let t = table(&th, &paragraphs(&["hello"])).await;
    let sharing: PublicLink = th
        .client
        .post(
            &format!("{TEST_WS}/containers/{}/public-link", t.board_id),
            &json!({"enabled":true}),
        )
        .await
        .ok_json();
    let mut member = Socket::connect_as(&th.client).await;
    visit(&mut member, &t.board_id, "").await;
    let _ = people(&mut member, 1).await;
    // Even the owner's signed-in socket is anonymous when it explicitly uses a link.
    let mut reader = Socket::connect_as(&th.client).await;
    reader
        .send(json!({"type":"setPresence", "containerId":t.board_id,
        "readToken":sharing.token}))
        .await;
    let roster = people(&mut member, 1).await;
    assert_eq!(roster["anonymousCount"], 1, "one anonymous connection");
    assert!(
        reader.next_json(QUIET).await.is_none(),
        "a reader hears no names or identifiers"
    );
    drop(reader);
    assert_eq!(
        people(&mut member, 1).await["anonymousCount"],
        0,
        "the reader left"
    );
    th.teardown().await;
}

#[tokio::test]
async fn no_membership_no_presence_and_revoked_membership_expires() {
    let th = Setup::native()
        .configure(|c| c.collab.collab_recheck_seconds = 1)
        .start()
        .await
        .init_basic()
        .await;
    let t = table(&th, &paragraphs(&["hello"])).await;
    let mut one = Socket::connect_as(&th.client).await;
    let mut two = Socket::connect_as(&th.client2).await;
    visit(&mut one, &t.board_id, "").await;
    let _ = people(&mut one, 1).await;
    visit(&mut two, &t.board_id, "").await;
    assert!(
        two.next_json(QUIET).await.is_none(),
        "an uninvited user hears nothing"
    );
    assert!(
        one.next_json(QUIET).await.is_none(),
        "the uninvited user is absent"
    );
    let other = th.client2.me().await.id;
    give(&th, &t.board_id, &other, "viewer").await;
    visit(&mut two, &t.board_id, "").await;
    let _ = people(&mut one, 2).await;
    let _ = people(&mut two, 2).await;
    th.client
        .delete(&format!(
            "{TEST_WS}/containers/{}/members/{other}",
            t.board_id
        ))
        .await
        .check_no_content();
    let _ = people(&mut one, 1).await;
    assert!(
        two.next_json(QUIET).await.is_none(),
        "the removed member receives nothing"
    );
    th.teardown().await;
}

#[tokio::test]
async fn leaving_and_reentering_within_one_batch_gets_a_new_roster() {
    let th = Setup::native().start().await.init_basic().await;
    let t = table(&th, &paragraphs(&["hello"])).await;
    let mut socket = Socket::connect_as(&th.client).await;
    visit(&mut socket, &t.board_id, "").await;
    let before = people(&mut socket, 1).await;
    visit(&mut socket, "", "").await;
    visit(&mut socket, &t.board_id, "").await;
    assert_eq!(
        people(&mut socket, 1).await,
        before,
        "reentry receives the roster again"
    );
    th.teardown().await;
}

#[tokio::test]
async fn a_guest_sees_an_unshared_template_reader_only_as_a_count() {
    use crate::common::{PASSWORD, ROOT_WORKSPACE_ID, ROOT_WS};
    use crate::rooms::give_in;
    use fvoci_model::{Container, ContainerVisibility};

    let th = Setup::native().start().await.init_basic().await;
    let third = th
        .register_and_login("template.reader", "template.reader@sample.com", PASSWORD)
        .await;
    let third_id = third.me().await.id;
    let guest_id = th.client2.me().await.id;
    let mut template =
        crate::boards::new_board(ROOT_WORKSPACE_ID, ContainerVisibility::Open, "Template");
    template.is_template = true;
    let template: Container = th.client.create_board(&template).await.created_json();
    give_in(&th, ROOT_WORKSPACE_ID, &template.id, &guest_id, "viewer").await;
    th.client
        .put(
            &format!("{ROOT_WS}/members/{guest_id}"),
            &json!({"role":"guest"}),
        )
        .await
        .check_ok();
    let mut owner = Socket::connect_as(&th.client).await;
    let mut guest = Socket::connect_as(&th.client2).await;
    for socket in [&mut owner, &mut guest] {
        socket.send(json!({"type":"setPresence", "workspaceId":ROOT_WORKSPACE_ID, "containerId":template.id})).await;
    }
    let _ = people(&mut guest, 2).await;
    let _ = people(&mut owner, 2).await;
    let mut reader = Socket::connect_as(&third).await;
    reader.send(json!({"type":"setPresence", "workspaceId":ROOT_WORKSPACE_ID, "containerId":template.id})).await;
    let roster = people(&mut guest, 2).await;
    assert_eq!(
        roster["hiddenCount"], 1,
        "the guest does not share a board role with the template reader"
    );
    assert!(
        !roster.to_string().contains(&third_id),
        "no hidden user's identifier leaves the server"
    );
    assert_eq!(
        people(&mut owner, 3).await["hiddenCount"],
        0,
        "a workspace member sees the reader"
    );
    let private = th
        .create_board(ROOT_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let mut uninvited = Socket::connect_as(&th.client2).await;
    uninvited
        .send(
            json!({"type":"setPresence", "workspaceId":ROOT_WORKSPACE_ID, "containerId":private.id}),
        )
        .await;
    assert!(
        uninvited.next_json(QUIET).await.is_none(),
        "a guest sees no uninvited board presence"
    );
    th.teardown().await;
}

#[tokio::test]
async fn visits_never_cross_a_workspace_or_organization_wall() {
    let th = Setup::native().start().await.init_basic().await;
    let own = table(&th, &paragraphs(&["own"])).await;
    let (elsewhere, foreign) = if th.app.store().db_type() == fvoci_store::DbType::Postgres {
        let (_, workspace, board) = crate::workspaces::another_organization(&th).await;
        (workspace, board)
    } else {
        let workspace = fvoci_model::Workspace {
            id: fvoci_model::new_id(),
            ..fvoci_model::Workspace::default()
        };
        assert!(
            th.app
                .store()
                .create_organization("another", &workspace)
                .await
                .is_err(),
            "SQLite enforces its single organization before any foreign presence can exist"
        );
        (workspace.id, fvoci_model::new_id())
    };
    let mut socket = Socket::connect_as(&th.client).await;
    for (workspace, board) in [
        (crate::common::ROOT_WORKSPACE_ID, own.board_id.as_str()),
        (elsewhere.as_str(), foreign.as_str()),
        (TEST_WORKSPACE_ID, foreign.as_str()),
    ] {
        socket
            .send(json!({"type":"setPresence", "workspaceId":workspace, "containerId":board}))
            .await;
        assert!(
            socket.next_json(QUIET).await.is_none(),
            "no roster outside the board's workspace and organization"
        );
    }
    th.teardown().await;
}

#[tokio::test]
async fn a_member_put_out_of_the_board_leaves_the_roster_before_the_periodic_check() {
    // The periodic check is a minute away: only the change itself can put them out.
    let th = Setup::native()
        .configure(|c| c.collab.collab_recheck_seconds = 60)
        .start()
        .await
        .init_basic()
        .await;
    let t = table(&th, &paragraphs(&["hello"])).await;
    let other = th.client2.me().await.id;
    give(&th, &t.board_id, &other, "viewer").await;
    let mut one = Socket::connect_as(&th.client).await;
    let mut two = Socket::connect_as(&th.client2).await;
    visit(&mut one, &t.board_id, "").await;
    visit(&mut two, &t.board_id, &t.card_id).await;
    let _ = people(&mut one, 2).await;
    let _ = people(&mut two, 2).await;
    let removed_at = tokio::time::Instant::now();
    th.client
        .delete(&format!(
            "{TEST_WS}/containers/{}/members/{other}",
            t.board_id
        ))
        .await
        .check_no_content();
    let roster = people(&mut one, 1).await;
    assert!(
        removed_at.elapsed() < std::time::Duration::from_secs(2),
        "the change put them out, not the periodic check"
    );
    assert!(
        !roster.to_string().contains(&other),
        "the removed member is gone from the roster"
    );
    assert!(
        two.next_json(QUIET).await.is_none(),
        "the removed member receives nothing more"
    );
    th.teardown().await;
}

/// The other reader's entry in `roster`.
fn entry_of<'r>(roster: &'r Value, user_id: &str) -> Option<&'r Value> {
    roster["people"]
        .as_array()
        .expect("people")
        .iter()
        .find(|person| person["userId"] == user_id)
}

#[tokio::test]
async fn a_refused_visit_ends_the_one_before_and_a_deleted_card_leaves_the_board_visit() {
    let th = Setup::native()
        .configure(|c| c.collab.collab_recheck_seconds = 60)
        .start()
        .await
        .init_basic()
        .await;
    let t = table(&th, &paragraphs(&["hello"])).await;
    let other = th.client2.me().await.id;
    give(&th, &t.board_id, &other, "viewer").await;
    let mut one = Socket::connect_as(&th.client).await;
    let mut two = Socket::connect_as(&th.client2).await;
    visit(&mut one, &t.board_id, "").await;
    visit(&mut two, &t.board_id, &t.card_id).await;
    let shown = people(&mut one, 2).await;
    assert_eq!(
        entry_of(&shown, &other).expect("the other reader")["cardIds"],
        json!([t.card_id])
    );
    th.client
        .delete(&format!(
            "{TEST_WS}/boards/{}/blocks/{}",
            t.board_id, t.card_id
        ))
        .await
        .check_no_content();
    let after = people(&mut one, 2).await;
    assert_eq!(
        entry_of(&after, &other).expect("still on the board")["cardIds"],
        json!([]),
        "a deleted card is closed, and its reader stays on the board"
    );
    // A card that is not the board's opens nothing, and the visit stays.
    visit(&mut two, &t.board_id, &t.card_id).await;
    assert!(
        one.next_json(QUIET).await.is_none(),
        "asking for the deleted card again changes nothing"
    );
    // A board the reader may not read: the visit asked before ends as well.
    let private = th
        .create_board(TEST_WORKSPACE_ID, fvoci_model::ContainerVisibility::Private)
        .await;
    visit(&mut two, &private.id, "").await;
    let alone = people(&mut one, 1).await;
    assert!(
        entry_of(&alone, &other).is_none(),
        "a refused visit does not leave the one before in its place"
    );
    th.teardown().await;
}
