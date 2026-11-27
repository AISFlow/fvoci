//! Permission changes reach every affected connection without waiting for the timer.

use std::time::Duration;

use serde_json::json;

use crate::common::{Setup, TEST_WORKSPACE_ID};
use crate::rooms::{PATIENCE, Peer, give, paragraphs, two_in_a_room};

/// The periodic check cannot account for any notice received by these tests unless the
/// test has every room ask now (`Collab::ask_again_now`): it asks once an hour, and each
/// of its reads, and of an immediate check's, may take as long. A notice that comes at
/// all came without it otherwise, however slow the machine.
pub(crate) fn seldom() -> Setup {
    Setup::native().configure(|config| config.collab.collab_recheck_seconds = 3600)
}

#[tokio::test]
async fn a_role_change_and_removal_reach_every_connection_immediately() {
    let th = seldom().start().await.init_basic().await;
    let (table, mut owner, mut member) = two_in_a_room(&th, &paragraphs(&["first"])).await;
    let user = th.client2.me().await.id;
    let mut other_tab = Peer::connect(&th.client2, 103).await;
    other_tab.joined(TEST_WORKSPACE_ID, &table.card_id).await;
    let card: fvoci_model::Card = th
        .client
        .post(
            &format!("/w/{TEST_WORKSPACE_ID}/boards/{}/cards", table.board_id),
            &json!({"title": "Another card"}),
        )
        .await
        .created_json();
    let mut another_card = Peer::connect(&th.client2, 104).await;
    another_card.joined(TEST_WORKSPACE_ID, &card.id).await;

    give(&th, &table.board_id, &user, "viewer").await;
    // Written as soon as the change has answered: every room it reaches had it by then.
    for peer in [&mut member, &mut other_tab, &mut another_card] {
        peer.write("refused").await;
    }
    for peer in [&mut member, &mut other_tab, &mut another_card] {
        assert_eq!(peer.text("docModeChanged").await["mode"], json!("read"));
        assert_eq!(
            peer.text("docNack").await["reason"],
            json!("document_read_only")
        );
    }
    owner.write_acked("still writing").await;
    th.client
        .delete(&format!(
            "/w/{TEST_WORKSPACE_ID}/containers/{}/members/{user}",
            table.board_id
        ))
        .await
        .check_no_content();
    // Out of the room as soon as the removal has answered.
    assert_eq!(
        th.app.collab().room_members(&th.app, &table.card_id).await,
        1
    );
    for peer in [&mut member, &mut other_tab, &mut another_card] {
        assert_eq!(peer.text("docLeft").await["reason"], json!("not_found"));
    }
    th.teardown().await;
}

#[tokio::test]
async fn removal_from_the_workspace_reaches_the_room_immediately() {
    let th = seldom().start().await.init_basic().await;
    let (_table, _owner, mut member) = two_in_a_room(&th, &paragraphs(&["first"])).await;
    let user = th.client2.me().await.id;
    workspace_admin(&th).await;
    th.client
        .delete(&format!("/w/{TEST_WORKSPACE_ID}/members/{user}"))
        .await
        .check_no_content();
    assert_eq!(member.text("docLeft").await["reason"], json!("not_found"));
    th.teardown().await;
}

pub(crate) async fn workspace_admin(th: &crate::common::TestServer) {
    let admin = th.client.me().await.id;
    th.app
        .store()
        .execute_unchecked(&format!(
            "UPDATE workspace_members SET role = 'admin' WHERE user_id = '{admin}' \
             AND workspace_id = '{TEST_WORKSPACE_ID}'"
        ))
        .await
        .expect("the first user administers this workspace");
}

async fn template_reader(th: &crate::common::TestServer) -> (String, Peer) {
    let board: fvoci_model::Container = th
        .client
        .create_board(&fvoci_model::Container {
            is_template: true,
            ..crate::boards::new_board(
                TEST_WORKSPACE_ID,
                fvoci_model::ContainerVisibility::Open,
                "Template",
            )
        })
        .await
        .created_json();
    let card: fvoci_model::Card = th
        .client
        .post(
            &format!("/w/{TEST_WORKSPACE_ID}/boards/{}/cards", board.id),
            &json!({"title": "Notes"}),
        )
        .await
        .created_json();
    let mut reader = Peer::connect(&th.client2, 201).await;
    reader.joined(TEST_WORKSPACE_ID, &card.id).await;
    (board.id, reader)
}

#[tokio::test]
async fn becoming_a_guest_closes_an_uninvited_template_room_immediately() {
    let th = seldom().start().await.init_basic().await;
    workspace_admin(&th).await;
    let (_board, mut reader) = template_reader(&th).await;
    let user = th.client2.me().await.id;
    th.client
        .put(
            &format!("/w/{TEST_WORKSPACE_ID}/members/{user}"),
            &json!({"role": "guest"}),
        )
        .await
        .check_ok();
    assert_eq!(reader.text("docLeft").await["reason"], json!("not_found"));
    th.teardown().await;
}

#[tokio::test]
async fn a_template_closed_to_the_workspace_puts_out_its_uninvited_readers() {
    let th = seldom().start().await.init_basic().await;
    let (board, mut reader) = template_reader(&th).await;
    th.client
        .patch(
            &format!("/w/{TEST_WORKSPACE_ID}/containers/{board}"),
            &json!({"visibility": "private"}),
        )
        .await
        .check_ok();
    assert_eq!(reader.text("docLeft").await["reason"], json!("not_found"));
    th.teardown().await;
}

#[tokio::test]
async fn a_batch_closing_a_template_puts_out_its_uninvited_readers() {
    let th = seldom().start().await.init_basic().await;
    let (board, mut reader) = template_reader(&th).await;
    th.client.patch(&format!("/w/{TEST_WORKSPACE_ID}/containers-and-blocks"), &json!({
        "containerIds": [board], "containerPatches": [{"visibility": "private"}], "blockIds": [], "blockPatches": [],
    })).await.check_ok();
    assert_eq!(reader.text("docLeft").await["reason"], json!("not_found"));
    th.teardown().await;
}

/// One batch of three boards closed at once, the first with no room: each room of
/// the others is reached by the one check of the batch.
#[tokio::test]
async fn a_batch_closing_several_templates_puts_out_the_readers_of_each() {
    let th = seldom().start().await.init_basic().await;
    let roomless = th
        .create_board(TEST_WORKSPACE_ID, fvoci_model::ContainerVisibility::Open)
        .await
        .id;
    let (first, mut first_reader) = template_reader(&th).await;
    let (second, mut second_reader) = template_reader(&th).await;
    th.client
        .patch(
            &format!("/w/{TEST_WORKSPACE_ID}/containers-and-blocks"),
            &json!({
                "containerIds": [roomless, first, second],
                "containerPatches": [{"visibility": "private"}, {"visibility": "private"}, {"visibility": "private"}],
                "blockIds": [], "blockPatches": [],
            }),
        )
        .await
        .check_ok();
    for reader in [&mut first_reader, &mut second_reader] {
        assert_eq!(reader.text("docLeft").await["reason"], json!("not_found"));
    }
    th.teardown().await;
}

#[tokio::test]
async fn trashing_a_board_or_workspace_empties_its_rooms_immediately() {
    for trash_workspace in [false, true] {
        let th = seldom().start().await.init_basic().await;
        let (table, mut owner, mut member) = two_in_a_room(&th, &paragraphs(&["first"])).await;
        let path = if trash_workspace {
            let org = crate::org_admin::org_of(&th).await;
            format!("/org/{org}/workspaces/{TEST_WORKSPACE_ID}")
        } else {
            format!("/w/{TEST_WORKSPACE_ID}/containers/{}", table.board_id)
        };
        th.client.delete(&path).await.check_no_content();
        for peer in [&mut owner, &mut member] {
            assert_eq!(peer.text("docLeft").await["reason"], json!("not_found"));
        }
        crate::rooms::no_rooms(&th).await;
        th.teardown().await;
    }
}

#[tokio::test]
async fn ending_an_organization_administrators_participation_puts_them_out() {
    for ending in ["leave", "organization", "instance"] {
        let th = seldom().start().await.init_basic().await;
        let table = crate::rooms::table(&th, &paragraphs(&["first"])).await;
        let user = th.client2.me().await.id;
        let org = crate::org_admin::org_of(&th).await;
        give(&th, &table.board_id, &user, "editor").await;
        crate::workspace_removal::remove_from_the_test_workspace(&th, &user).await;
        th.client
            .put(
                &format!("/org/{org}/members/{user}/role"),
                &json!({"role": "admin"}),
            )
            .await
            .check_no_content();
        th.client2
            .post_empty(&format!("/org/{org}/workspaces/{TEST_WORKSPACE_ID}/join"))
            .await
            .check_ok();
        if ending == "instance" {
            th.client
                .put(&format!("/users/{user}/admin"), &json!({"isAdmin": true}))
                .await
                .check_ok();
        }
        let mut peer = Peer::connect(&th.client2, 301).await;
        peer.joined(TEST_WORKSPACE_ID, &table.card_id).await;
        if ending == "instance" {
            th.client
                .put(&format!("/users/{user}/admin"), &json!({"isAdmin": false}))
                .await
                .check_ok();
        } else if ending == "organization" {
            th.client
                .put(
                    &format!("/org/{org}/members/{user}/role"),
                    &json!({"role": "member"}),
                )
                .await
                .check_no_content();
        } else {
            th.client2
                .post_empty(&format!("/org/{org}/workspaces/{TEST_WORKSPACE_ID}/leave"))
                .await
                .check_ok();
        }
        assert_eq!(peer.text("docLeft").await["reason"], json!("not_found"));
        th.teardown().await;
    }
}

/// A periodic answer read before a change cannot restore the old write permission:
/// once it has gone on, while the next asking is held with its own answers, the
/// member still reads only.
#[tokio::test]
async fn an_old_periodic_answer_does_not_undo_an_immediate_demotion() {
    let th = seldom().start().await.init_basic().await;
    let (table, _owner, mut member) = two_in_a_room(&th, &paragraphs(&["first"])).await;
    let user = th.client2.me().await.id;
    let collab = th.app.collab();
    let (waiting, go) = collab.pause_next_recheck();
    collab.ask_again_now();
    tokio::time::timeout(PATIENCE, waiting.notified())
        .await
        .expect("the periodic read pauses");
    give(&th, &table.board_id, &user, "viewer").await;
    assert_eq!(member.text("docModeChanged").await["mode"], json!("read"));
    let (next, after) = collab.pause_next_recheck();
    go.notify_one();
    collab.ask_again_now();
    tokio::time::timeout(PATIENCE, next.notified())
        .await
        .expect("the next periodic read pauses");
    assert_eq!(
        member.incoming(crate::rooms::QUIET).await,
        crate::rooms::Incoming::Nothing
    );
    member.write("not allowed by the old answer").await;
    assert_eq!(
        member.text("docNack").await["reason"],
        json!("document_read_only")
    );
    after.notify_one();
    th.teardown().await;
}

/// A stuck immediate read cannot hold the room and all future checks forever.
#[tokio::test]
async fn an_unanswered_immediate_check_has_a_deadline() {
    let th = Setup::native()
        .configure(|config| config.collab.collab_recheck_seconds = 1)
        .start()
        .await
        .init_basic()
        .await;
    let (table, _owner, mut member) = two_in_a_room(&th, &paragraphs(&["first"])).await;
    let user = th.client2.me().await.id;
    let waiting = th.app.collab().stall_next_access_check();
    tokio::time::timeout(Duration::from_secs(3), async {
        give(&th, &table.board_id, &user, "viewer").await;
        waiting.notified().await;
        assert_eq!(
            member.text("docLeft").await["reason"],
            json!("document_unavailable")
        );
    })
    .await
    .expect("the unanswered permission read expires before the mutation answers");
    give(&th, &table.board_id, &user, "editor").await;
    member.joined(TEST_WORKSPACE_ID, &table.card_id).await;
    member
        .write_acked("the next check and room still work")
        .await;
    th.teardown().await;
}

/// A board whose only admin leaves the workspace passes to who took them out: that
/// administrator, reading the card as a viewer, may write in its room at once.
#[tokio::test]
async fn a_board_passed_to_the_workspace_admin_lets_them_write_at_once() {
    let th = seldom().start().await.init_basic().await;
    let t = crate::rooms::table(&th, &paragraphs(&["first"])).await;
    let (me, other) = (th.client.me().await.id, th.client2.me().await.id);
    let path = |user: &str| {
        format!(
            "/w/{TEST_WORKSPACE_ID}/containers/{}/members/{user}",
            t.board_id
        )
    };
    let role = |user: &str, admin: bool| fvoci_model::ContainerMember {
        container_id: t.board_id.clone(),
        user_id: user.to_owned(),
        role: if admin {
            fvoci_model::ContainerRole::Admin
        } else {
            fvoci_model::ContainerRole::Viewer
        },
        synthetic: false,
        inherited_role: None,
    };
    // The other user is the board's only admin; the first a viewer of it.
    give(&th, &t.board_id, &other, "viewer").await;
    th.client
        .put(&path(&other), &role(&other, true))
        .await
        .check_ok();
    th.client
        .put(&path(&me), &role(&me, false))
        .await
        .check_ok();
    let mut reader = Peer::connect(&th.client, 601).await;
    let joined = reader.joined(TEST_WORKSPACE_ID, &t.card_id).await;
    assert_eq!(joined["mode"], json!("read"), "{joined}");
    workspace_admin(&th).await;
    th.client
        .delete(&format!("/w/{TEST_WORKSPACE_ID}/members/{other}"))
        .await
        .check_no_content();
    assert_eq!(reader.text("docModeChanged").await["mode"], json!("write"));
    th.teardown().await;
}
