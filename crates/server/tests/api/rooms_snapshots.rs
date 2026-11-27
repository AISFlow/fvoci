//! Snapshots requested immediately after an acknowledged room update include it.

use std::io::{Cursor, Read as _};

use fvoci_model::{Block, BlockType, ContainersAndBlocks};
use serde_json::Value;

use crate::collab::{body_of, card};
use crate::common::{Setup, TEST_WORKSPACE_ID, TestServer};
use crate::rooms::{Peer, Table, paragraphs, table};

#[tokio::test]
async fn copies_templates_and_archives_include_the_last_acknowledged_update() {
    let th = Setup::native().start().await.init_basic().await;
    let table = table(&th, &paragraphs(&["first"])).await;
    let mut writer = Peer::connect(&th.client, 101).await;
    writer.joined(TEST_WORKSPACE_ID, &table.card_id).await;
    for operation in [
        "card",
        "card template",
        "board",
        "board template",
        "archive",
    ] {
        writer.write_acked(operation).await;
        let wanted = writer.body();
        assert_ne!(
            body_of(&card(&th.client, &table.card_id).await),
            wanted,
            "{operation}: the delayed projection has not stored this update"
        );
        let copied = snapshot(&th, &table, operation).await;
        assert_eq!(
            copied, wanted,
            "{operation}: the snapshot includes the pending update"
        );
        assert_eq!(
            body_of(&card(&th.client, &table.card_id).await),
            wanted,
            "{operation}: the source card is written first"
        );
    }
    th.teardown().await;
}

async fn snapshot(th: &TestServer, table: &Table, operation: &str) -> Option<Value> {
    let base = format!("/w/{TEST_WORKSPACE_ID}/containers/{}", table.board_id);
    let template = if operation.ends_with("template") {
        "?asTemplate=true"
    } else {
        ""
    };
    let blocks: Vec<Block> = if operation.starts_with("card") {
        th.client
            .post_empty(&format!(
                "/w/{TEST_WORKSPACE_ID}/boards/{}/blocks/{}/duplicate{template}",
                table.board_id, table.card_id
            ))
            .await
            .created_json()
    } else if operation.starts_with("board") {
        let copy: ContainersAndBlocks = th
            .client
            .post_empty(&format!("{base}/duplicate{template}"))
            .await
            .created_json();
        copy.blocks
    } else {
        let response = th.client.get(&format!("{base}/archive/export")).await;
        response.check_ok();
        let mut zip = zip::ZipArchive::new(Cursor::new(response.body)).expect("a ZIP archive");
        let mut lines = String::new();
        zip.by_name(&format!("{}/container.jsonl", table.board_id))
            .expect("the board")
            .read_to_string(&mut lines)
            .expect("JSONL");
        lines
            .lines()
            .map(|line| serde_json::from_str::<fvoci_model::ArchiveLine>(line).expect("a line"))
            .filter(|line| line.line_type == "block")
            .map(|line| serde_json::from_value(line.data).expect("a block"))
            .collect()
    };
    blocks
        .into_iter()
        .find(|block| block.block_type == BlockType::Card)
        .expect("the copied card")
        .body
        .map(|body| body.document().clone())
}

#[tokio::test]
async fn a_copy_recovers_updates_whose_document_is_no_longer_open() {
    let th = Setup::native().start().await.init_basic().await;
    let e = crate::collab::editing(&th, &paragraphs(&["first"])).await;
    let board_id = e
        .app
        .store()
        .get_block(&e.card_id)
        .await
        .expect("the card")
        .container_id;
    let (editor, epoch) = crate::collab::joined(e.app.collab(), &e.app, &e.card_id, 102).await;
    e.app
        .collab()
        .take(
            &e.app,
            &e.card_id,
            epoch,
            &e.user1,
            &editor.append_paragraph("logged before the worker ended"),
        )
        .await
        .expect("logged");
    let wanted = editor.body().map(|body| body.document().clone());
    e.app.collab().abandon().await;
    assert_ne!(
        body_of(&card(&th.client, &e.card_id).await),
        wanted,
        "only the log has the update"
    );
    assert_eq!(
        snapshot(
            &th,
            &Table {
                board_id,
                card_id: e.card_id
            },
            "card"
        )
        .await,
        wanted
    );
    th.teardown().await;
}

/// The duplication API also copies nested child cards, whose rooms may be open.
/// Template copies omit sub-cards (templates cannot hold sub-cards) while still flushing
/// the copied card's own room.
#[tokio::test]
async fn copying_a_card_or_template_includes_pending_bodies_of_child_cards() {
    let th = Setup::native().start().await.init_basic().await;
    let table = table(&th, &paragraphs(&["parent"])).await;
    let child = Block {
        id: fvoci_model::new_id(),
        container_id: table.board_id.clone(),
        parent_id: table.card_id.clone(),
        block_type: BlockType::Card,
        title: "Child".to_owned(),
        body: fvoci_model::Body::parse(paragraphs(&["child"])).expect("a body"),
        created_at: 1,
        updated_at: 1,
        ..Block::default()
    };
    let children = crate::blocks::insert(&th.client, &table.board_id, &[child]).await;
    let child_id = &children[0].id;
    let mut parent_writer = Peer::connect(&th.client, 102).await;
    parent_writer
        .joined(TEST_WORKSPACE_ID, &table.card_id)
        .await;
    let mut writer = Peer::connect(&th.client, 103).await;
    writer.joined(TEST_WORKSPACE_ID, child_id).await;

    writer.write_acked("false").await;
    let wanted_child = writer.body();
    assert_ne!(
        body_of(&card(&th.client, child_id).await),
        wanted_child,
        "the child has a pending update"
    );
    let copies: Vec<Block> = th
        .client
        .post_empty(&format!(
            "/w/{TEST_WORKSPACE_ID}/boards/{}/blocks/{}/duplicate?asTemplate=false",
            table.board_id, table.card_id
        ))
        .await
        .created_json();
    let child_copy = copies
        .iter()
        .find(|block| block.title == "Child")
        .expect("the copied child");
    assert_eq!(
        child_copy.body.as_ref().map(|body| body.document().clone()),
        wanted_child
    );

    parent_writer.write_acked("true").await;
    let wanted_parent = parent_writer.body();
    let template_copies: Vec<Block> = th
        .client
        .post_empty(&format!(
            "/w/{TEST_WORKSPACE_ID}/boards/{}/blocks/{}/duplicate?asTemplate=true",
            table.board_id, table.card_id
        ))
        .await
        .created_json();
    assert!(
        template_copies.iter().all(|block| block.title != "Child"),
        "a template copy omits child cards"
    );
    assert_eq!(
        template_copies[0]
            .body
            .as_ref()
            .map(|body| body.document().clone()),
        wanted_parent
    );
    th.teardown().await;
}

/// A server that holds as many documents as it may (one worker of one document, open
/// in a room): a copy and an export still write the log of a card whose document is
/// not open, in the worker's share aside, and copy what the editors typed. Each has a
/// card of its own whose log is not written yet: the copy's card, then the export's.
#[tokio::test]
async fn a_full_server_still_writes_a_closed_documents_log_before_a_copy_or_an_export() {
    let th = Setup::native()
        .configure(|config| {
            config.collab.collab_workers = 1;
            config.collab.collab_documents_per_worker = 1;
        })
        .start()
        .await
        .init_basic()
        .await;
    let e = crate::collab::editing(&th, &paragraphs(&["first"])).await;
    let board_id = e
        .app
        .store()
        .get_block(&e.card_id)
        .await
        .expect("the card")
        .container_id;
    let card_in_board = |title: &str, body: Option<Value>| {
        let path = format!("/w/{TEST_WORKSPACE_ID}/boards/{board_id}/cards");
        let body = serde_json::json!({ "title": title, "body": body });
        let client = &th.client;
        async move {
            let card: fvoci_model::Card = client.post(&path, &body).await.created_json();
            card.id
        }
    };
    let exported = card_in_board("Exported", Some(paragraphs(&["first"]))).await;
    let mut wanted = Vec::new();
    for (card_id, client) in [(&e.card_id, 102), (&exported, 103)] {
        let (editor, epoch) = crate::collab::joined(e.app.collab(), &e.app, card_id, client).await;
        e.app
            .collab()
            .take(
                &e.app,
                card_id,
                epoch,
                &e.user1,
                &editor.append_paragraph("logged, and its document closed"),
            )
            .await
            .expect("logged");
        wanted.push(editor.body().map(|body| body.document().clone()));
        // Its document closed without its body written: only the log has the update.
        e.app.collab().abandon().await;
    }
    let (open, refused) = (
        card_in_board("Open", None).await,
        card_in_board("Refused", None).await,
    );
    let mut holder = Peer::connect(&th.client, 104).await;
    holder.joined(TEST_WORKSPACE_ID, &open).await;
    let mut other = Peer::connect(&th.client, 105).await;
    let answer = other.join(TEST_WORKSPACE_ID, &refused).await;
    assert_eq!(
        (&answer["type"], &answer["reason"]),
        (
            &serde_json::json!("docRefused"),
            &serde_json::json!("too_many_documents")
        ),
        "the server holds as many documents as it may: {answer}"
    );
    for ((operation, card_id), wanted) in [("card", &e.card_id), ("archive", &exported)]
        .into_iter()
        .zip(wanted)
    {
        assert_ne!(
            body_of(&card(&th.client, card_id).await),
            wanted,
            "{operation}: only the log has the update"
        );
        let table = Table {
            board_id: board_id.clone(),
            card_id: card_id.clone(),
        };
        assert_eq!(
            snapshot(&th, &table, operation).await,
            wanted,
            "{operation}"
        );
    }
    th.teardown().await;
}
