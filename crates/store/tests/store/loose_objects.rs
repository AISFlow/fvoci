//! The objects of the file storage that may have no record: marked while they are
//! written, doomed with the record that goes, and kept apart by organization.

use fvoci_model::{FileRecord, Upload, new_id};
use fvoci_store::{DbType, LooseObject, LooseState, SYSTEM_ORG_ID, Store, StoreError, Tenant};

use crate::common;

/// A record of a file of the board at `path`, as the server makes one.
fn file_at(board_id: &str, path: &str) -> FileRecord {
    FileRecord {
        id: new_id(),
        container_id: board_id.to_owned(),
        stored_name: path.rsplit('/').next().unwrap_or(path).to_owned(),
        path: path.to_owned(),
        size: 3,
        created_at: 1,
        ..FileRecord::default()
    }
}

/// A board of a new workspace of the test's organization, by a new user.
async fn board(store: &Store) -> (String, String) {
    let user_id = common::user(store, &format!("u{}", &new_id()[24..])).await;
    let workspace_id = common::workspace(store).await;
    let boards = common::create_test_boards(store, &workspace_id, &user_id, 1).await;
    (boards[0].id.clone(), user_id)
}

fn paths_in(objects: &[LooseObject], state: LooseState) -> Vec<&str> {
    objects
        .iter()
        .filter(|object| object.state == state)
        .map(|object| object.path.as_str())
        .collect()
}

/// The purge of the trash and the collection of the files no block shows doom the
/// objects of the records they remove, in the transaction that removes them: by the
/// system's handle, as the server runs them, each row is the organization's of its
/// record.
#[tokio::test]
async fn the_records_that_go_doom_their_objects_in_the_same_transaction() {
    let test = common::setup().await;
    let (board_id, user_id) = board(&test).await;
    let purged_path = format!("boards/20261112/{board_id}/purged.png");
    test.save_file_record(&file_at(&board_id, &purged_path))
        .await
        .expect("a file of the board");
    test.delete_container(&board_id, &user_id)
        .await
        .expect("the board goes to the trash");
    let (kept_board, _) = board(&test).await;
    let unshown_path = format!("boards/20261112/{kept_board}/unshown.png");
    test.save_file_record(&file_at(&kept_board, &unshown_path))
        .await
        .expect("a file no block shows");

    let system = test.system();
    let purged = system.purge_trash(i64::MAX).await.expect("purged");
    assert_eq!(purged.files.len(), 1, "the board's file went: {purged:?}");
    let collected = system
        .collect_unreferenced_files(i64::MAX)
        .await
        .expect("collected");
    assert_eq!(collected.len(), 1, "the file no block shows went");

    let listed = test.loose_objects().await.expect("listed");
    let mut expected = [purged_path.as_str(), unshown_path.as_str()];
    expected.sort_unstable();
    assert_eq!(
        paths_in(&listed, LooseState::Doomed),
        expected,
        "both objects are doomed, as the test's organization's"
    );
    let due = test
        .due_doomed_objects(i64::MAX, 10)
        .await
        .expect("the due ones are read");
    assert_eq!(due.len(), 2, "and due at once: {due:?}");
    assert!(
        due.iter()
            .all(|object| object.attempts == 0 && object.last_error.is_empty()),
        "never tried: {due:?}"
    );
    test.teardown().await;
}

/// A file is recorded by the transaction that takes its mark of writing away, and not
/// without the mark: a writer whose object was doomed meanwhile, and perhaps removed
/// and taken off the list, records nothing and dooms what it wrote again.
#[tokio::test]
async fn a_written_file_is_recorded_only_while_it_is_marked() {
    let test = common::setup().await;
    let (board_id, _) = board(&test).await;
    let path = format!("boards/20261112/{board_id}/written.png");
    let file = file_at(&board_id, &path);

    let unmarked_path = format!("boards/20261112/{board_id}/unmarked.png");
    assert!(
        !test
            .record_written_file(&file_at(&board_id, &unmarked_path))
            .await
            .expect("read"),
        "nothing is being written there"
    );
    test.mark_writing(&path, i64::MAX).await.expect("marked");
    test.mark_writing(&path, i64::MAX)
        .await
        .expect_err("a place is marked once");
    assert!(test.record_written_file(&file).await.expect("recorded"));
    let listed = test.loose_objects().await.expect("listed");
    assert_eq!(
        paths_in(&listed, LooseState::Doomed),
        [unmarked_path.as_str()],
        "the record took the mark away; what was written without one is doomed"
    );
    test.get_file_record(&board_id, &file.stored_name)
        .await
        .expect("the record is there");

    // Doomed while it was written: whoever wrote it records nothing.
    let doomed_path = format!("boards/20261112/{board_id}/doomed.png");
    test.mark_writing(&doomed_path, i64::MAX)
        .await
        .expect("marked");
    assert!(test.doom_writing(&doomed_path, 0).await.expect("doomed"));
    assert!(
        !test.doom_writing(&doomed_path, 0).await.expect("read"),
        "a doomed object is none of the writer's"
    );
    test.forget_doomed_object(&doomed_path, i64::MAX)
        .await
        .expect("removed by the housekeeping before the writer is done");
    assert!(
        !test
            .record_written_file(&file_at(&board_id, &doomed_path))
            .await
            .expect("read"),
        "the object is doomed"
    );
    let listed = test.loose_objects().await.expect("listed");
    let mut doomed = [doomed_path.as_str(), unmarked_path.as_str()];
    doomed.sort_unstable();
    assert_eq!(
        paths_in(&listed, LooseState::Doomed),
        doomed,
        "doomed again: what the late writer wrote is the housekeeping's to remove"
    );
    let missing = test
        .get_file_record(&board_id, "doomed.png")
        .await
        .expect_err("nothing is recorded");
    assert!(missing.is_not_found(), "{missing:?}");
    test.teardown().await;
}

/// A mark of writing that was to be over is doomed; a removal that failed is noted and
/// waits until it is due again; one that went takes the object off the list.
#[tokio::test]
async fn stale_marks_are_doomed_and_failed_removals_wait() {
    let test = common::setup().await;
    test.mark_writing("boards/a", 100).await.expect("marked");
    test.mark_writing("boards/b", 200).await.expect("marked");
    assert_eq!(test.doom_stale_writing(99).await.expect("doomed"), 0);
    assert_eq!(test.doom_stale_writing(100).await.expect("doomed"), 1);
    let due = test.due_doomed_objects(150, 10).await.expect("read");
    assert_eq!(
        due.iter()
            .map(|object| object.path.as_str())
            .collect::<Vec<_>>(),
        ["boards/a"],
        "the stale one alone, due when it was to be over"
    );

    test.doomed_object_failed("boards/a", 150, "io", 1_000)
        .await
        .expect("noted");
    assert_eq!(
        test.due_doomed_objects(999, 10).await.expect("read"),
        [],
        "not due before it is tried again"
    );
    let again = test.due_doomed_objects(1_000, 10).await.expect("read");
    assert_eq!(
        (
            again[0].attempts,
            again[0].last_attempt_at,
            again[0].last_error.as_str()
        ),
        (1, 150, "io"),
        "{again:?}"
    );
    let counts = test.loose_object_counts().await.expect("counted");
    assert_eq!((counts.writing, counts.doomed), (1, 1), "{counts:?}");
    assert_eq!(
        counts.failing,
        [],
        "failed once, not yet tried again: {counts:?}"
    );
    test.doomed_object_failed("boards/a", 1_000, "object_store", 2_000)
        .await
        .expect("noted");
    let counts = test.loose_object_counts().await.expect("counted");
    assert_eq!(
        counts.failing,
        [("object_store".to_owned(), 1)],
        "failed again when tried again, by the last error: {counts:?}"
    );

    test.forget_doomed_object("boards/b", i64::MAX)
        .await
        .expect("a mark of writing is not forgotten so");
    test.forget_doomed_object("boards/a", 1_999)
        .await
        .expect("read");
    assert_eq!(
        test.loose_objects().await.expect("listed").len(),
        2,
        "a doom due after the run's time is left"
    );
    test.forget_doomed_object("boards/a", 2_000)
        .await
        .expect("forgotten");
    let listed = test.loose_objects().await.expect("listed");
    assert_eq!(paths_in(&listed, LooseState::Writing), ["boards/b"]);
    assert_eq!(paths_in(&listed, LooseState::Doomed), Vec::<&str>::new());
    test.teardown().await;
}

/// The objects one organization lists are none of another's, and a handle of no
/// organization lists none: the system's handle reaches them all.
#[tokio::test]
async fn the_list_of_one_organization_is_not_anothers() {
    if common::db_type() != DbType::Postgres {
        return;
    }
    let test = common::setup().await;
    let unset = test.scoped(Tenant::Unset);
    let (other_org, _) = common::organization(&unset, "the other").await;
    let theirs = test.for_org(&other_org).expect("an identifier");
    test.mark_writing("boards/ours", i64::MAX)
        .await
        .expect("marked");
    theirs
        .mark_writing("boards/theirs", i64::MAX)
        .await
        .expect("marked");
    unset
        .mark_writing("boards/nobodys", i64::MAX)
        .await
        .expect_err("a row is some organization's");

    let paths = async |store: &Store| -> Vec<String> {
        store
            .loose_objects()
            .await
            .expect("listed")
            .into_iter()
            .map(|object| object.path)
            .collect()
    };
    assert_eq!(paths(&test).await, ["boards/ours"]);
    assert_eq!(paths(&theirs).await, ["boards/theirs"]);
    assert_eq!(paths(&unset).await, Vec::<String>::new());
    assert_eq!(
        paths(&test.system()).await,
        ["boards/ours", "boards/theirs"]
    );
    assert!(
        !test.doom_writing("boards/theirs", 0).await.expect("read"),
        "theirs is not ours to doom"
    );
    assert_eq!(
        test.doom_stale_writing(i64::MAX).await.expect("doomed"),
        1,
        "ours alone"
    );
    test.teardown().await;
}

/// The purge of the trash by the system's handle, over two organizations, dooms each
/// object as the organization's of its record.
#[tokio::test]
async fn the_system_dooms_each_object_as_its_organizations() {
    if common::db_type() != DbType::Postgres {
        return;
    }
    let test = common::setup().await;
    let unset = test.scoped(Tenant::Unset);
    let (other_org, _) = common::organization(&unset, "the other").await;
    let theirs = test.for_org(&other_org).expect("an identifier");
    for (store, name) in [(&test.store, "ours"), (&theirs, "theirs")] {
        let (board_id, user_id) = board(store).await;
        store
            .save_file_record(&file_at(&board_id, &format!("boards/{name}")))
            .await
            .expect("a file of the board");
        store
            .delete_container(&board_id, &user_id)
            .await
            .expect("the board goes to the trash");
    }
    let purged = test.system().purge_trash(i64::MAX).await.expect("purged");
    assert_eq!(purged.files.len(), 2, "{purged:?}");
    for (store, name) in [(&test.store, "ours"), (&theirs, "theirs")] {
        let listed = store.loose_objects().await.expect("listed");
        assert_eq!(
            paths_in(&listed, LooseState::Doomed),
            [format!("boards/{name}").as_str()],
            "each organization lists its own"
        );
    }
    test.teardown().await;
}

/// A mark of writing that one organization puts at a place, as a wrong call of the store
/// could, does not keep another organization from marking the place and recording its
/// file there: a place is listed once by an organization.
#[tokio::test]
async fn a_mark_of_one_organization_does_not_block_anothers_write() {
    if common::db_type() != DbType::Postgres {
        return;
    }
    let test = common::setup().await;
    let unset = test.scoped(Tenant::Unset);
    let (other_org, _) = common::organization(&unset, "the other").await;
    let theirs = test.for_org(&other_org).expect("an identifier");
    let (board_id, _) = board(&theirs).await;
    let path = format!("boards/20261112/{board_id}/theirs.png");
    test.mark_writing(&path, i64::MAX)
        .await
        .expect("ours, at their place");
    theirs
        .mark_writing(&path, i64::MAX)
        .await
        .expect("their mark is not kept out by ours");
    assert!(
        theirs
            .record_written_file(&file_at(&board_id, &path))
            .await
            .expect("recorded"),
        "their file is recorded"
    );
    let ours = test.loose_objects().await.expect("listed");
    assert_eq!(paths_in(&ours, LooseState::Writing), [path.as_str()]);
    assert_eq!(theirs.loose_objects().await.expect("listed"), []);
    // Ours, gone stale and doomed, is one row of the place; theirs a record of it.
    assert_eq!(test.doom_stale_writing(i64::MAX).await.expect("doomed"), 1);
    let held = test.held_objects(&[&path], None).await.expect("read");
    assert!(held.contains(&path), "their record holds it, asked of all");
    test.teardown().await;
}

/// What holds an object, asked of every organization: a record of a live board or of
/// one in the trash, a write under way, an upload session but the one excepted; and
/// not a doomed object, nor a place nothing names.
#[tokio::test]
async fn what_holds_an_object() {
    let test = common::setup().await;
    let (board_id, user_id) = board(&test).await;
    let trashed = format!("boards/20261112/{board_id}/trashed.png");
    test.save_file_record(&file_at(&board_id, &trashed))
        .await
        .expect("a file of the board");
    test.delete_container(&board_id, &user_id)
        .await
        .expect("the board goes to the trash");
    let (live_board, user_id) = board(&test).await;
    let live = format!("boards/20261112/{live_board}/live.png");
    test.save_file_record(&file_at(&live_board, &live))
        .await
        .expect("a file of the board");
    test.mark_writing("boards/being-written", i64::MAX)
        .await
        .expect("marked");
    test.mark_writing("boards/doomed", 0).await.expect("marked");
    test.doom_stale_writing(1).await.expect("doomed");
    let upload = Upload {
        id: new_id(),
        user_id,
        container_id: live_board.clone(),
        name: "a.bin".to_owned(),
        size: 1,
        part_size: 1,
        created_at: 1,
        expires_at: i64::MAX,
        path: "boards/uploading".to_owned(),
        ..Upload::default()
    };
    test.create_upload(&upload)
        .await
        .expect("an upload is opened");

    let asked = [
        trashed.as_str(),
        live.as_str(),
        "boards/being-written",
        "boards/doomed",
        "boards/uploading",
        "boards/nothing",
    ];
    let held = test.held_objects(&asked, None).await.expect("read");
    let mut expected = vec![
        trashed.clone(),
        live.clone(),
        "boards/being-written".to_owned(),
        "boards/uploading".to_owned(),
    ];
    expected.sort_unstable();
    assert_eq!(held.into_iter().collect::<Vec<_>>(), expected);
    let held = test
        .held_objects(&["boards/uploading"], Some(&upload.id))
        .await
        .expect("read");
    assert!(
        held.is_empty(),
        "the excepted session holds nothing: {held:?}"
    );
    test.teardown().await;
}

/// A place one organization doomed, as a wrong call of the store could, is held while
/// another organization writes there: the housekeeping does not remove an object under
/// a write.
#[tokio::test]
async fn another_organizations_write_holds_a_doomed_place() {
    if common::db_type() != DbType::Postgres {
        return;
    }
    let test = common::setup().await;
    let unset = test.scoped(Tenant::Unset);
    let (other_org, _) = common::organization(&unset, "the other").await;
    let theirs = test.for_org(&other_org).expect("an identifier");
    test.mark_writing("boards/p", 0).await.expect("ours");
    test.doom_stale_writing(1).await.expect("ours is doomed");
    theirs
        .mark_writing("boards/p", i64::MAX)
        .await
        .expect("theirs is being written");
    let held = test.held_objects(&["boards/p"], None).await.expect("read");
    assert!(held.contains("boards/p"), "their write holds it");
    test.teardown().await;
}

/// What a comparison of the storage with the records dooms, by the system's handle, is
/// the organization's whose key it is, of a board live or in the trash, and the system
/// organization's when it is no key or the key of no organization there is; once.
#[tokio::test]
async fn an_unrecorded_object_is_doomed_as_its_keys_organizations() {
    let test = common::setup().await;
    let (board_id, user_id) = board(&test).await;
    let (trashed_board, _) = board(&test).await;
    test.delete_container(&trashed_board, &user_id)
        .await
        .expect("the board goes to the trash");
    let org = &test.org_id;
    let ours = format!("{org}/{board_id}/stray");
    let trashed = format!("{org}/{trashed_board}/stray");
    let nobodys = format!("{}/{board_id}/stray", new_id());
    let no_key = "boards/20261112/stray.png";
    let system = test.system();
    let paths = [ours.as_str(), trashed.as_str(), nobodys.as_str(), no_key];
    assert_eq!(system.doom_unrecorded(&paths).await.expect("doomed"), 4);
    assert_eq!(
        system.doom_unrecorded(&paths).await.expect("doomed"),
        0,
        "once"
    );
    let listed = test.loose_objects().await.expect("listed");
    if common::db_type() == DbType::Postgres {
        let mut expected = [ours.as_str(), trashed.as_str()];
        expected.sort_unstable();
        assert_eq!(paths_in(&listed, LooseState::Doomed), expected, "ours");
        let system_org = test.for_org(SYSTEM_ORG_ID).expect("an identifier");
        let listed = system_org.loose_objects().await.expect("listed");
        let mut expected = [nobodys.as_str(), no_key];
        expected.sort_unstable();
        assert_eq!(
            paths_in(&listed, LooseState::Doomed),
            expected,
            "the system organization's"
        );
    } else {
        assert_eq!(listed.len(), 4, "{listed:?}");
    }
    test.teardown().await;
}

/// By a handle of an organization, the key of another is refused and nothing of the
/// batch is doomed; its own keys, and places of no key, are its own.
#[tokio::test]
async fn an_organization_dooms_no_key_of_another() {
    let test = common::setup().await;
    let (board_id, _) = board(&test).await;
    let ours = format!("{}/{board_id}/stray", test.org_id);
    let theirs = format!("{}/{board_id}/stray", new_id());
    let refused = test
        .doom_unrecorded(&[ours.as_str(), theirs.as_str()])
        .await;
    assert!(
        matches!(&refused, Err(StoreError::InvalidPath(path)) if *path == theirs),
        "{refused:?}"
    );
    assert_eq!(
        test.loose_objects().await.expect("listed"),
        [],
        "nothing doomed"
    );
    let paths = [ours.as_str(), "boards/20261112/stray.png"];
    assert_eq!(test.doom_unrecorded(&paths).await.expect("doomed"), 2);
    test.teardown().await;
}

/// The places the records name, live or kept with a board in the trash, read a page at a
/// time after the last one read, each once and in order.
#[tokio::test]
async fn the_recorded_places_are_read_a_page_at_a_time() {
    let test = common::setup().await;
    let (live_board, user_id) = board(&test).await;
    let (trashed_board, _) = board(&test).await;
    let mut expected = Vec::new();
    for (board_id, names) in [
        (&live_board, ["b", "d", "f"]),
        (&trashed_board, ["a", "c", "e"]),
    ] {
        for name in names {
            let path = format!("boards/{name}");
            test.save_file_record(&file_at(board_id, &path))
                .await
                .expect("a file of the board");
            expected.push(path);
        }
    }
    test.delete_container(&trashed_board, &user_id)
        .await
        .expect("the board goes to the trash");
    expected.sort_unstable();
    let mut read = Vec::new();
    let mut after = String::new();
    loop {
        let page = test
            .system()
            .recorded_paths_after(&after, 2)
            .await
            .expect("a page");
        assert!(page.len() <= 2, "{page:?}");
        let Some(last) = page.last() else { break };
        after.clone_from(last);
        read.extend(page);
    }
    assert_eq!(read, expected);
    test.teardown().await;
}

/// A place is spelled one way ([`fvoci_model::is_storage_path`]): a record, a mark of
/// writing and an upload session take no other spelling, and a place asked for in
/// another is held, whatever names it, so that nothing is removed under it.
#[tokio::test]
async fn a_place_is_taken_and_compared_in_one_spelling() {
    let test = common::setup().await;
    let (board_id, user_id) = board(&test).await;
    let alias = format!("boards//20261112/{board_id}/x.png");
    let is_invalid_path = |error: &StoreError| matches!(error, StoreError::InvalidPath(_));
    let error = test
        .save_file_record(&file_at(&board_id, &alias))
        .await
        .expect_err("a record of another spelling");
    assert!(is_invalid_path(&error), "{error:?}");
    let error = test
        .mark_writing(&alias, i64::MAX)
        .await
        .expect_err("a mark of another spelling");
    assert!(is_invalid_path(&error), "{error:?}");
    let upload = Upload {
        id: new_id(),
        user_id,
        container_id: board_id.clone(),
        name: "x.png".to_owned(),
        size: 3,
        part_size: 3,
        expires_at: i64::MAX,
        multipart_id: "multipart".to_owned(),
        path: alias.clone(),
        ..Upload::default()
    };
    let error = test
        .create_upload(&upload)
        .await
        .expect_err("a session of another spelling");
    assert!(is_invalid_path(&error), "{error:?}");

    let place = format!("boards/20261112/{board_id}/x.png");
    let held = test
        .held_objects(&[&place, &alias, "./x.png"], None)
        .await
        .expect("read");
    let mut expected = vec![alias.clone(), "./x.png".to_owned()];
    expected.sort_unstable();
    assert_eq!(
        held.into_iter().collect::<Vec<_>>(),
        expected,
        "nothing holds the place; the other spellings are held"
    );
    test.teardown().await;
}

/// A row stored before the rule of one spelling that names a place otherwise keeps
/// every object: whether it holds a place cannot be told. A record of the trash of
/// that time comes back as it was.
#[tokio::test]
async fn a_row_of_another_spelling_holds_every_place() {
    let test = common::setup().await;
    let (board_id, user_id) = board(&test).await;
    let place = format!("boards/20261112/{board_id}/x.png");
    test.save_file_record(&file_at(&board_id, &place))
        .await
        .expect("a record");
    test.execute_unchecked(&format!(
        "UPDATE files SET path = 'boards//20261112/{board_id}/x.png'"
    ))
    .await
    .expect("a record of the time before the rule");
    let elsewhere = "boards/20261112/other/y.png";
    let held = test.held_objects(&[elsewhere], None).await.expect("read");
    assert!(held.contains(elsewhere), "{held:?}");

    test.delete_container(&board_id, &user_id)
        .await
        .expect("the board goes to the trash");
    test.undelete_container(&board_id, &user_id)
        .await
        .expect("it comes back with its record as it was");
    test.teardown().await;
}
