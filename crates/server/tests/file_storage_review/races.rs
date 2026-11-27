//! A second process at the database of a server cannot run the removal of doomed
//! objects beside the server's: one that does not hold the lock of the server is
//! refused before it reads the list, so no worker removes on a stale reading of what
//! holds an object.

use fvoci_model::new_id;
use fvoci_server::app::App;
use fvoci_server::auth::now_millis;
use fvoci_store::Store;

use crate::common::Setup;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_overlapping_worker_without_the_lock_of_the_server_does_not_run() {
    let ts = Setup::native().start().await.init_basic().await;
    let file = crate::recorded(&ts).await;
    let housekeeping = ts.app.housekeeping();
    housekeeping
        .store()
        .collect_unreferenced_files(i64::MAX)
        .await
        .expect("the original object is doomed");

    // The second process: its own pool, and no lock of the server.
    let settings = ts.app.settings().clone();
    let independent = Store::connect(
        ts.app.store().db_type(),
        &settings.config().database_connection,
    )
    .await
    .expect("an independent worker's pool");
    let second = App::new(settings, independent.clone(), ts.app.files().clone())
        .expect("an independent application");
    assert!(!second.holds_server_lock());
    let refused = second
        .housekeeping()
        .remove_doomed_objects(now_millis())
        .await
        .expect_err("a worker without the lock of the server does not run");
    assert_eq!(refused.status(), 503, "{refused:?}");
    assert!(
        ts.app.files().exists(&file.path).await.expect("read"),
        "the refused worker removed nothing"
    );

    // The server's own worker removes the original and forgets the job.
    housekeeping
        .remove_doomed_objects(now_millis())
        .await
        .expect("the server's worker removes the original");
    ts.app
        .store()
        .mark_writing(&file.path, i64::MAX)
        .await
        .expect("the place can be marked again once the old job is gone");
    let size = ts
        .app
        .files()
        .write(&file.path, &mut &b"replacement bytes"[..])
        .await
        .expect("the replacement is written");
    let replacement = fvoci_model::FileRecord {
        id: new_id(),
        created_at: now_millis(),
        size: i64::try_from(size).expect("a small file"),
        ..file
    };
    assert!(
        ts.app
            .store()
            .record_written_file(&replacement)
            .await
            .expect("recorded")
    );
    // The second process tries again, after the replacement is recorded.
    second
        .housekeeping()
        .remove_doomed_objects(now_millis())
        .await
        .expect_err("still without the lock");
    housekeeping
        .remove_doomed_objects(now_millis())
        .await
        .expect("the server's worker runs again");
    let still_recorded = ts
        .app
        .store()
        .get_file_record(&replacement.container_id, &replacement.stored_name)
        .await
        .is_ok();
    let still_present = ts
        .app
        .files()
        .exists(&replacement.path)
        .await
        .expect("read");
    independent.close().await;
    ts.teardown().await;
    assert!(still_recorded, "the replacement's live record remains");
    assert!(
        still_present,
        "a stale worker must preserve the replacement's bytes"
    );
}
