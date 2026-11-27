//! A new image reference survives a purge that started counting references earlier.

use std::time::Duration;

use fvoci_model::{Block, BlockPatch, BlockType, Body, BodyChange, FileRecord, new_id};
use fvoci_store::DbType;
use serde_json::json;
use sqlx::PgConnection;

use crate::common;

/// A trigger in this test's schema pauses the purge after its statement's snapshot,
/// while its row lock still lets a new image's FOR KEY SHARE through.
async fn pause_marking(test: &common::TestStore) -> (PgConnection, i64) {
    let mut gate = common::connect_past_the_store(&test.database_connection()).await;
    let key: i64 = sqlx::query_scalar("SELECT pg_backend_pid()::bigint")
        .fetch_one(&mut gate)
        .await
        .expect("the connection has an identifier");
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(key)
        .execute(&mut gate)
        .await
        .expect("the test closes the gate");
    let trigger = format!(
        "CREATE FUNCTION pause_file_mark() RETURNS trigger LANGUAGE plpgsql AS $$ \
         BEGIN PERFORM pg_advisory_xact_lock({key}::bigint); RETURN NEW; END $$; \
         CREATE TRIGGER pause_file_mark BEFORE UPDATE ON files FOR EACH ROW \
         WHEN (OLD.unreferenced_at IS NULL AND NEW.unreferenced_at IS NOT NULL) \
         EXECUTE FUNCTION pause_file_mark()"
    );
    sqlx::raw_sql(sqlx::AssertSqlSafe(trigger))
        .execute(&mut gate)
        .await
        .expect("the test-only seam is installed");
    (gate, key)
}

async fn wait_for_mark(gate: &mut PgConnection, key: i64) -> i32 {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let waiting: Option<i32> = sqlx::query_scalar(
                "SELECT pid FROM pg_locks WHERE locktype = 'advisory' \
                 AND objid::bigint = $1 AND NOT granted LIMIT 1",
            )
            .bind(key)
            .fetch_optional(&mut *gate)
            .await
            .expect("the waiting purge can be observed");
            if let Some(pid) = waiting {
                break pid;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the purge reaches the marking statement")
}

#[tokio::test]
async fn a_new_image_survives_a_purge_that_saw_no_reference() {
    let test = common::setup().await;
    let user = common::user(&test, "purge.writer").await;
    let workspace = common::workspace(&test).await;
    let boards = common::create_test_boards(&test, &workspace, &user, 1).await;
    let board = &boards[0].id;
    let file = FileRecord {
        id: new_id(),
        container_id: board.clone(),
        stored_name: "7shown.png".to_owned(),
        path: format!("boards/{board}/7shown.png"),
        created_at: 1,
        ..FileRecord::default()
    };
    test.save_file_record(&file)
        .await
        .expect("the file is stored");
    let body = Body::parse(json!({"type": "doc", "content": [
        {"type": "image", "attrs": {"fileId": "7shown.png"}}
    ]}))
    .expect("a valid body")
    .expect("a nonempty body");
    let mut old = Block {
        id: new_id(),
        container_id: board.clone(),
        block_type: BlockType::Card,
        body: Some(body.clone()),
        ..Block::default()
    };
    test.insert_block(&mut old, &user)
        .await
        .expect("the old reference");
    test.delete_block(&old.id, &user)
        .await
        .expect("the old card is trashed");
    let mut live = Block {
        id: new_id(),
        body: None,
        ..old
    };
    test.insert_block(&mut live, &user)
        .await
        .expect("the live card");
    let patch = BlockPatch {
        body: BodyChange::Set(body.clone()),
        ..BlockPatch::default()
    };
    let system = test.system();
    if common::db_type() == DbType::Postgres {
        race_with_mark(&test, &live.id, &patch, &user).await;
    } else {
        // SQLite's single writer orders these transactions; it has no stale snapshot.
        system
            .purge_trash(i64::MAX)
            .await
            .expect("the purge commits");
        test.patch_block(&live.id, &patch, &user)
            .await
            .expect("the image is shown");
    }
    assert_eq!(
        test.get_block(&live.id).await.expect("the live card").body,
        Some(body)
    );
    let collected = system
        .collect_unreferenced_files(i64::MAX)
        .await
        .expect("collected");
    assert!(
        collected.is_empty(),
        "a committed reference keeps its file: {collected:?}"
    );
    assert!(
        test.due_doomed_objects(i64::MAX, 10)
            .await
            .expect("due objects")
            .is_empty(),
        "the server must never be told to delete the referenced bytes"
    );
    assert_fresh_retention(&test, &live.id, &user, &file).await;
    test.teardown().await;
}

/// Accept either the pre-fix writer's commit or its wait for the purge's row lock.
async fn race_with_mark(test: &common::TestStore, card: &str, patch: &BlockPatch, user: &str) {
    let (mut gate, key) = pause_marking(test).await;
    let purger = test.system();
    let purge = tokio::spawn(async move { purger.purge_trash(i64::MAX).await });
    let purge_pid = wait_for_mark(&mut gate, key).await;
    let writer = test.store.clone();
    let (card, patch, user) = (card.to_owned(), patch.clone(), user.to_owned());
    let write = tokio::spawn(async move { writer.patch_block(&card, &patch, &user).await });
    let reached = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if write.is_finished() {
                break;
            }
            let blocked: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM pg_stat_activity \
                 WHERE $1 = ANY(pg_blocking_pids(pid)))",
            )
            .bind(purge_pid)
            .fetch_one(&mut gate)
            .await
            .expect("the writer's wait");
            if blocked {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(key)
        .execute(&mut gate)
        .await
        .expect("the purge can finish");
    reached.expect("the writer commits or waits for the purge");
    write
        .await
        .expect("the writer task ends")
        .expect("the image is shown");
    purge
        .await
        .expect("the purge task ends")
        .expect("the purge commits");
}

async fn assert_fresh_retention(
    test: &common::TestStore,
    card: &str,
    user: &str,
    file: &FileRecord,
) {
    let unmarked = common::try_execute_raw(
        &test.database_connection(),
        &format!(
            "UPDATE files SET unreferenced_at = unreferenced_at \
         WHERE id = '{}' AND unreferenced_at IS NULL",
            file.id,
        ),
    )
    .await
    .expect("the reference has no collection mark");
    assert_eq!(
        unmarked, 1,
        "the new reference must clear the stale purge timestamp"
    );
    let system = test.system();
    let before_removal = crate::blocks::now_millis();
    // One version of the body is kept, so that the one that shows the file, of the
    // body before, goes with this write and is no reference.
    test.set_body_versions_kept(1);
    test.patch_block(
        card,
        &BlockPatch {
            body: BodyChange::Clear,
            ..BlockPatch::default()
        },
        user,
    )
    .await
    .expect("the last reference goes");
    assert!(
        system
            .collect_unreferenced_files(before_removal)
            .await
            .expect("collected")
            .is_empty(),
        "a fresh full retention interval starts"
    );
    assert_eq!(
        system
            .collect_unreferenced_files(i64::MAX)
            .await
            .expect("collected after retention"),
        std::slice::from_ref(file),
        "the unshown file is collected only after its fresh retention interval"
    );
}
