//! A permission change that reaches many people in many rooms: the store is asked
//! about each board and person once, some at a time, so that a slow store keeps the
//! change waiting for a few reads' time, not for all of them one after another.

use std::time::Duration;

use serde_json::json;
use tokio::time::Instant;

use crate::common::{Setup, TEST_WORKSPACE_ID};
use crate::rooms::{Peer, give, paragraphs, table};

/// How many boards the workspace has, each with a card everybody is in the room of.
const BOARDS: usize = 12;

/// How long each read of the immediate check takes, as a slow store would.
const READ: Duration = Duration::from_millis(300);

/// What the trash may take to answer: 48 reads (12 boards, 4 people) take 6 reads'
/// time eight at a time (1.8 s), and 48 one after another (14.4 s).
const ANSWERED_WITHIN: Duration = Duration::from_secs(7);

#[tokio::test]
async fn trashing_a_large_workspace_reads_everybody_at_once_and_puts_them_out() {
    let th = Setup::native()
        .configure(|config| {
            // The periodic check stays away: only the immediate one puts anybody out.
            config.collab.collab_recheck_seconds = 3600;
            config.collab.collab_documents_per_connection = 16;
            config.collab.collab_frames_per_second = 1000;
        })
        .start()
        .await
        .init_basic()
        .await;
    let (third, fourth) = (th.add_user("third").await, th.add_user("fourth").await);
    let clients = [&th.client, &th.client2, &third, &fourth];
    let mut others = Vec::new();
    for client in &clients[1..] {
        others.push(client.me().await.id);
    }
    let mut cards = Vec::new();
    for board in 0..BOARDS {
        let t = table(&th, &paragraphs(&[&format!("board {board}")])).await;
        for user in &others {
            give(&th, &t.board_id, user, "editor").await;
        }
        cards.push(t.card_id);
    }
    let mut peers = Vec::new();
    for (client, editor) in clients.into_iter().zip(201..) {
        let mut peer = Peer::connect(client, editor).await;
        for card_id in &cards {
            peer.joined(TEST_WORKSPACE_ID, card_id).await;
        }
        peers.push(peer);
    }
    th.app.collab().slow_access_reads(READ);
    let org = crate::org_admin::org_of(&th).await;
    let started = Instant::now();
    th.client
        .delete(&format!("/org/{org}/workspaces/{TEST_WORKSPACE_ID}"))
        .await
        .check_no_content();
    let answered_in = started.elapsed();
    for (index, peer) in peers.iter_mut().enumerate() {
        let mut reasons = Vec::new();
        while reasons.len() < BOARDS {
            reasons.push(peer.text("docLeft").await["reason"].clone());
        }
        assert!(
            reasons.iter().all(|reason| *reason == json!("not_found")),
            "person {index} was put out of every room as of a card not there: {reasons:?}"
        );
    }
    assert!(
        answered_in < ANSWERED_WITHIN,
        "the trash answered in {answered_in:?}: the reads were not made some at a time"
    );
    th.teardown().await;
}
