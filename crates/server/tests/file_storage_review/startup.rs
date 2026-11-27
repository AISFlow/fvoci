//! A restart must not mistake an overlapping process's active writes for crash debris.

use std::path::Path;
use std::time::Duration;

use fvoci_model::ContainerVisibility;
use fvoci_server::Server;
use fvoci_server::storage::WRITING_DIR;
use tokio::io::AsyncWriteExt as _;

use crate::common::{Setup, TEST_WORKSPACE_ID};

#[tokio::test]
async fn starting_a_second_server_preserves_the_first_servers_active_local_write() {
    let ts = Setup::native().start().await.init_basic().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let app = ts.app.clone();
    let (mut writer, mut reader) = tokio::io::duplex(64);
    let saving =
        tokio::spawn(async move { app.save_file(&mut reader, &board.id, "in-flight.txt").await });
    writer.write_all(b"in-flight bytes").await.expect("sent");
    let writing = Path::new(&ts.app.config().file_storage_path).join(WRITING_DIR);
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Ok(mut entries) = tokio::fs::read_dir(&writing).await
                && entries
                    .next_entry()
                    .await
                    .expect("directory read")
                    .is_some()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the first process has created its partial file");

    // One server serves a database: the second waits for the lock of the server, which
    // the first holds, and does not start, cleaning nothing up.
    let mut config = ts.app.config().clone();
    config.server_lock_wait_seconds = 1;
    let restarted = Server::new(config, "").await;
    writer
        .shutdown()
        .await
        .expect("the first upload finishes sending");
    let result = saving.await.expect("the first write finishes");
    ts.teardown().await;
    let refusal = restarted.expect_err("an overlapping start is refused");
    assert!(
        refusal.to_string().contains("fvoci-server process"),
        "the refusal names who holds the lock: {refusal:#}"
    );
    assert!(
        result.is_ok(),
        "startup must not destroy an active write: {result:?}"
    );
}
