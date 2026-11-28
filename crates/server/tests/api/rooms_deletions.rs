//! Trash snapshots and room lifetimes agree, including writes and restores in flight.

use std::time::Duration;

use fvoci_model::{Block, BlockType};
use fvoci_server::collab::DeletionStage;
use serde_json::json;

use crate::collab::{body_of, card};
use crate::common::{Setup, TEST_WORKSPACE_ID};
use crate::rooms::{Incoming, Peer, QUIET, paragraphs, two_in_a_room};

#[tokio::test]
async fn deleting_cards_and_boards_keeps_the_last_room_body_for_restore() {
    for target in ["card", "board"] {
        let th = Setup::native().start().await.init_basic().await;
        let (table, mut writer, mut other) = two_in_a_room(&th, &paragraphs(&["first"])).await;
        writer.write_acked("last acknowledged edit").await;
        let wanted = writer.body();
        assert_ne!(body_of(&card(&th.client, &table.card_id).await), wanted);
        let base = format!("/w/{TEST_WORKSPACE_ID}");
        let board = format!("{base}/containers/{}", table.board_id);
        if target == "card" {
            crate::blocks::trash_card(&th.client, &table.board_id, &table.card_id)
                .await
                .check_no_content();
        } else {
            th.client.delete(&board).await.check_no_content();
        }
        for peer in [&mut writer, &mut other] {
            assert_eq!(peer.text("docLeft").await["reason"], json!("not_found"));
        }
        assert_eq!(th.app.collab().rooms(), 0);
        let trashed = th
            .app
            .store()
            .get_deleted_block(&table.card_id)
            .await
            .expect("in the trash");
        assert_eq!(
            trashed
                .block
                .body
                .as_ref()
                .map(|body| body.document().clone()),
            wanted
        );
        if target == "card" {
            crate::blocks::restore_card(&th.client, &table.board_id, &table.card_id)
                .await
                .check_ok();
        } else {
            th.client
                .post_empty(&format!("{board}/restore"))
                .await
                .check_ok();
        }
        let mut reopened = Peer::connect(&th.client, 203).await;
        reopened.joined(TEST_WORKSPACE_ID, &table.card_id).await;
        assert_eq!(
            reopened.body(),
            wanted,
            "{target}: restore includes the last edit"
        );
        th.teardown().await;
    }
}

#[tokio::test]
async fn deletion_waits_for_a_projection_already_in_progress() {
    let th = Setup::native().start().await.init_basic().await;
    let (table, mut writer, _other) = two_in_a_room(&th, &paragraphs(&["first"])).await;
    writer.write_acked("written before deletion").await;
    let wanted = writer.body();
    let (waiting, go) = th.app.collab().pause_next_projection();
    let projection = tokio::spawn({
        let (app, id) = (th.app.clone(), table.card_id.clone());
        async move { app.collab().write_body(&app, &id).await }
    });
    waiting.notified().await;
    let path = (table.board_id.clone(), table.card_id.clone());
    let mut deletion = tokio::spawn({
        let client = th.client.clone();
        async move { crate::blocks::trash_card(&client, &path.0, &path.1).await }
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut deletion)
            .await
            .is_err(),
        "deletion cannot overtake a held projection"
    );
    assert!(th.app.store().get_block(&table.card_id).await.is_ok());
    go.notify_one();
    projection
        .await
        .expect("projection task")
        .expect("written before deletion");
    deletion.await.expect("deletion task").check_no_content();
    assert_eq!(writer.text("docLeft").await["reason"], json!("not_found"));
    let trashed = th
        .app
        .store()
        .get_deleted_block(&table.card_id)
        .await
        .expect("in the trash");
    assert_eq!(
        trashed
            .block
            .body
            .as_ref()
            .map(|body| body.document().clone()),
        wanted
    );
    th.teardown().await;
}

#[tokio::test]
async fn a_deleted_rooms_late_input_cannot_reach_the_restored_cards_new_room() {
    let th = Setup::native().start().await.init_basic().await;
    let (table, mut old, _other) = two_in_a_room(&th, &paragraphs(&["first"])).await;
    old.write_acked("before deletion").await;
    let wanted = old.body();
    let path = (table.board_id.clone(), table.card_id.clone());
    let (waiting, go) = th
        .app
        .collab()
        .pause_next_deletion(DeletionStage::AfterRemoving);
    let deletion = tokio::spawn({
        let (client, path) = (th.client.clone(), path.clone());
        async move { crate::blocks::trash_card(&client, &path.0, &path.1).await }
    });
    waiting.notified().await;
    crate::blocks::restore_card(&th.client, &path.0, &path.1)
        .await
        .check_ok();
    let mut fresh = Peer::connect(&th.client, 303).await;
    fresh.joined(TEST_WORKSPACE_ID, &table.card_id).await;
    assert_eq!(fresh.body(), wanted);
    old.send_binary(crate::rooms::frame(
        old.doc,
        old.epoch,
        crate::rooms::STEP_1,
        &old.editor().state_vector(),
    ))
    .await;
    assert_eq!(
        old.incoming(crate::rooms::QUIET).await,
        crate::rooms::Incoming::Nothing,
        "the deleted room cannot read the restored document either"
    );
    old.write("must not reach the restored card").await;
    assert_eq!(old.text("docNack").await["reason"], json!("not_found"));
    fresh.write_acked("in the new room").await;
    go.notify_one();
    deletion.await.expect("deletion task").check_no_content();
    assert_eq!(old.text("docLeft").await["reason"], json!("not_found"));
    fresh
        .write_acked("the old room did not close this one")
        .await;
    th.app
        .collab()
        .write_body(&th.app, &table.card_id)
        .await
        .expect("the new body is written");
    assert_eq!(
        body_of(&card(&th.client, &table.card_id).await),
        fresh.body()
    );
    assert!(
        !serde_json::to_string(&fresh.body())
            .expect("JSON")
            .contains("must not reach")
    );
    assert_eq!(
        th.app.collab().room_members(&th.app, &table.card_id).await,
        1
    );
    th.teardown().await;
}

/// The body as the trash holds it for `card_id`.
async fn trashed_body(th: &crate::common::TestServer, card_id: &str) -> Option<serde_json::Value> {
    th.app
        .store()
        .get_deleted_block(card_id)
        .await
        .expect("in the trash")
        .block
        .body
        .map(|body| body.document().clone())
}

/// An update that reaches the room while the deletion holds its organization's
/// documents waits for it, and is then refused: it is in no body, the trash's or the
/// other member's.
#[tokio::test]
async fn an_update_sent_during_a_deletion_is_refused_and_kept_out_of_the_trash() {
    let th = Setup::native().start().await.init_basic().await;
    let (table, mut writer, mut other) = two_in_a_room(&th, &paragraphs(&["first"])).await;
    writer.write_acked("before the deletion").await;
    let wanted = writer.body();
    let path = (table.board_id.clone(), table.card_id.clone());
    let (waiting, go) = th
        .app
        .collab()
        .pause_next_deletion(DeletionStage::BeforeRemoving);
    let deletion = tokio::spawn({
        let client = th.client.clone();
        async move { crate::blocks::trash_card(&client, &path.0, &path.1).await }
    });
    waiting.notified().await;
    writer.write("during the deletion").await;
    assert_eq!(
        writer.incoming(QUIET).await,
        Incoming::Nothing,
        "the update waits for the deletion"
    );
    go.notify_one();
    deletion.await.expect("deletion task").check_no_content();
    assert_eq!(writer.text("docNack").await["reason"], json!("not_found"));
    assert_eq!(writer.text("docLeft").await["reason"], json!("not_found"));
    assert_eq!(other.text("docLeft").await["reason"], json!("not_found"));
    assert!(!other.lines().iter().any(|line| line.contains("during")));
    assert_eq!(trashed_body(&th, &table.card_id).await, wanted);
    th.teardown().await;
}

/// A card below the card that is deleted goes to the trash with what its room held
/// last, and comes back with it when the card is restored.
#[tokio::test]
async fn a_card_below_the_deleted_card_keeps_its_rooms_last_body() {
    let th = Setup::native().start().await.init_basic().await;
    let table = crate::rooms::table(&th, &paragraphs(&["parent"])).await;
    let child = Block {
        id: fvoci_model::new_id(),
        container_id: table.board_id.clone(),
        parent_id: table.card_id.clone(),
        block_type: BlockType::Card,
        title: "below".to_owned(),
        created_at: 1,
        updated_at: 1,
        ..Block::default()
    };
    let stored = crate::blocks::insert(&th.client, &table.board_id, &[child]).await;
    let child_id = stored[0].id.clone();
    let mut writer = Peer::connect(&th.client, 401).await;
    writer.joined(TEST_WORKSPACE_ID, &child_id).await;
    writer.write_acked("typed below").await;
    let wanted = writer.body();
    crate::blocks::trash_card(&th.client, &table.board_id, &table.card_id)
        .await
        .check_no_content();
    assert_eq!(writer.text("docLeft").await["reason"], json!("not_found"));
    assert_eq!(trashed_body(&th, &child_id).await, wanted);
    crate::blocks::restore_card(&th.client, &table.board_id, &table.card_id)
        .await
        .check_ok();
    let mut reopened = Peer::connect(&th.client, 402).await;
    reopened.joined(TEST_WORKSPACE_ID, &child_id).await;
    assert_eq!(reopened.body(), wanted);
    th.teardown().await;
}

/// The deleted room emptied while the restored card has a new room: closing the old
/// one writes nothing and leaves the new room's document as it is.
#[tokio::test]
async fn the_deleted_room_emptied_after_a_restore_leaves_the_new_room_alone() {
    let th = Setup::native().start().await.init_basic().await;
    let (table, mut old, mut other) = two_in_a_room(&th, &paragraphs(&["first"])).await;
    old.write_acked("before deletion").await;
    let path = (table.board_id.clone(), table.card_id.clone());
    let (waiting, go) = th
        .app
        .collab()
        .pause_next_deletion(DeletionStage::AfterRemoving);
    let deletion = tokio::spawn({
        let (client, path) = (th.client.clone(), path.clone());
        async move { crate::blocks::trash_card(&client, &path.0, &path.1).await }
    });
    waiting.notified().await;
    crate::blocks::restore_card(&th.client, &path.0, &path.1)
        .await
        .check_ok();
    let mut fresh = Peer::connect(&th.client, 503).await;
    fresh.joined(TEST_WORKSPACE_ID, &table.card_id).await;
    let epoch = fresh.epoch;
    fresh.write_acked("in the new room").await;
    let (projections_before, release) = th.app.collab().pause_next_projection();
    old.leave().await;
    other.leave().await;
    assert!(
        tokio::time::timeout(QUIET, projections_before.notified())
            .await
            .is_err(),
        "the old room writes no body as it closes"
    );
    release.notify_one();
    assert_eq!(fresh.incoming(QUIET).await, Incoming::Nothing);
    go.notify_one();
    deletion.await.expect("deletion task").check_no_content();
    fresh.write_acked("the same document").await;
    assert_eq!(
        fresh.epoch, epoch,
        "the new room's document was not made again"
    );
    th.teardown().await;
}
