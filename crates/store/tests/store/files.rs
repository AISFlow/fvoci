//! Integration tests for container-scoped file metadata records, retention, and cleanup.

use std::time::{SystemTime, UNIX_EPOCH};

use fvoci_model::{Block, BlockType, FileRecord, new_id};
use fvoci_store::StoreError;
use serde_json::json;

use crate::common;

fn now_millis() -> i64 {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is past the epoch")
        .as_millis();
    i64::try_from(millis).expect("the current time fits in 64 bits")
}

/// Saving and retrieving a file record through its container and stored name.
#[tokio::test]
async fn file_store() {
    let test = common::setup().await;
    let store = &test.store;
    let user_id = common::user(store, "michael.scott").await;
    let workspace_id = common::workspace(store).await;
    let boards = common::create_test_boards(store, &workspace_id, &user_id, 1).await;
    let board_id = &boards[0].id;

    let file = FileRecord {
        id: new_id(),
        container_id: board_id.clone(),
        stored_name: "7salesreport.sales".to_owned(),
        created_at: now_millis(),
        name: "Dunder Mifflin Sales Report 2022".to_owned(),
        extension: ".sales".to_owned(),
        size: 112_233,
        ..FileRecord::default()
    };

    store.save_file_record(&file).await.unwrap();

    // A file is looked up through its board by its stored name.
    let retrieved = store
        .get_file_record(board_id, "7salesreport.sales")
        .await
        .unwrap();
    assert_eq!(retrieved.id, file.id);
    assert_eq!(retrieved.container_id, *board_id);
    assert_eq!(retrieved.stored_name, "7salesreport.sales");
    assert_eq!(retrieved.name, "Dunder Mifflin Sales Report 2022");
    assert_eq!(retrieved.extension, ".sales");
    assert_eq!(retrieved.size, 112_233);

    let error = store
        .get_file_record(board_id, "nonexistent")
        .await
        .unwrap_err();
    assert!(matches!(error, StoreError::NotFound(_)), "got {error:?}");

    test.teardown().await;
}

/// Every member of a file record is stored and read back accurately.
#[tokio::test]
async fn file_record_stores_every_member() {
    let test = common::setup().await;
    let store = &test.store;
    let user_id = common::user(store, "pam.beesly").await;
    let workspace_id = common::workspace(store).await;
    let boards = common::create_test_boards(store, &workspace_id, &user_id, 1).await;
    let board_id = &boards[0].id;

    let file = FileRecord {
        id: new_id(),
        container_id: board_id.clone(),
        stored_name: "7abc.png".to_owned(),
        created_at: 1_700_000_000_000,
        path: "boards/20240101/7abc.png".to_owned(),
        name: "picture.png".to_owned(),
        extension: ".png".to_owned(),
        size: 5,
        ..FileRecord::default()
    };
    store.save_file_record(&file).await.unwrap();

    let stored = store.get_file_record(board_id, "7abc.png").await.unwrap();
    assert_eq!(
        stored, file,
        "every member comes back as it was saved, the given creation time included"
    );

    // A board has one file under a stored name, and a record has one identifier.
    let same_name = FileRecord {
        id: new_id(),
        ..file.clone()
    };
    let error = store.save_file_record(&same_name).await.unwrap_err();
    assert!(
        matches!(error, StoreError::Database(_)),
        "a second file of the board under the stored name: got {error:?}"
    );
    let same_id = FileRecord {
        stored_name: "7def.png".to_owned(),
        ..file.clone()
    };
    let error = store.save_file_record(&same_id).await.unwrap_err();
    assert!(
        matches!(error, StoreError::Database(_)),
        "a second record with the identifier: got {error:?}"
    );
    let stored_again = store.get_file_record(board_id, "7abc.png").await.unwrap();
    assert_eq!(stored_again, stored, "the refused records changed nothing");

    // The stored name is compared exactly.
    let missing = store
        .get_file_record(board_id, "7ABC.PNG")
        .await
        .unwrap_err();
    assert_eq!(missing.to_string(), "{file} not found");

    // Text that is not an identifier names no board: nothing is found under it, and
    // nothing is stored under it.
    let missing = store
        .get_file_record("not-a-board", "7abc.png")
        .await
        .unwrap_err();
    assert!(
        matches!(missing, StoreError::NotFound(_)),
        "got {missing:?}"
    );
    let no_board = FileRecord {
        id: new_id(),
        container_id: "not-a-board".to_owned(),
        ..file.clone()
    };
    let error = store.save_file_record(&no_board).await.unwrap_err();
    assert!(
        matches!(&error, StoreError::InvalidId(refused) if refused == "not-a-board"),
        "got {error:?}"
    );
    let no_id = FileRecord {
        id: "not-an-id".to_owned(),
        stored_name: "7ghi.png".to_owned(),
        ..file.clone()
    };
    let error = store.save_file_record(&no_id).await.unwrap_err();
    assert!(
        matches!(&error, StoreError::InvalidId(refused) if refused == "not-an-id"),
        "got {error:?}"
    );

    test.teardown().await;
}

/// A file record belongs to one board: another board does not find it and may hold a file
/// of its own under the same stored name. Deleting the block that shows a file leaves the
/// records of both boards.
#[tokio::test]
async fn a_file_is_found_through_its_own_board_only() {
    let test = common::setup().await;
    let store = &test.store;
    let user_id = common::user(store, "jim.halpert").await;
    let workspace_id = common::workspace(store).await;
    let boards = common::create_test_boards(store, &workspace_id, &user_id, 3).await;
    let (own, other, third) = (&boards[0].id, &boards[1].id, &boards[2].id);
    let stored_name = "7shared.png";

    let file_of = |board_id: &str, uploaded_as: &str| FileRecord {
        id: new_id(),
        container_id: board_id.to_owned(),
        stored_name: stored_name.to_owned(),
        created_at: now_millis(),
        path: format!("boards/{board_id}/{stored_name}"),
        name: uploaded_as.to_owned(),
        extension: ".png".to_owned(),
        size: 7,
        ..FileRecord::default()
    };

    let own_file = file_of(own, "mine.png");
    store.save_file_record(&own_file).await.unwrap();

    let found = store.get_file_record(own, stored_name).await.unwrap();
    assert_eq!(found, own_file, "the board finds its file");
    for stranger in [other, third] {
        let error = store
            .get_file_record(stranger, stored_name)
            .await
            .unwrap_err();
        assert!(
            matches!(error, StoreError::NotFound(_)),
            "another board does not find the file: got {error:?}"
        );
    }

    // The stored name is taken within a board, not across boards.
    let other_file = file_of(other, "theirs.png");
    store
        .save_file_record(&other_file)
        .await
        .expect("another board holds a file under the same stored name");
    let found = store.get_file_record(own, stored_name).await.unwrap();
    assert_eq!(found, own_file, "each board finds its own file");
    let found = store.get_file_record(other, stored_name).await.unwrap();
    assert_eq!(found, other_file, "each board finds its own file");
    let error = store.get_file_record(third, stored_name).await.unwrap_err();
    assert!(
        matches!(error, StoreError::NotFound(_)),
        "a board without the file still finds none: got {error:?}"
    );

    // Deleting the image block of one board leaves both records as they are: nothing
    // marks a file record deleted.
    let mut host_card = Block {
        id: new_id(),
        container_id: other.clone(),
        block_type: BlockType::Card,
        created_by: user_id.clone(),
        ..Block::default()
    };
    store.insert_block(&mut host_card, &user_id).await.unwrap();
    let mut image = Block {
        id: new_id(),
        container_id: other.clone(),
        parent_id: host_card.id.clone(),
        block_type: BlockType::Attachment,
        created_by: user_id.clone(),
        fields: serde_json::from_value(json!({ "fileId": stored_name }))
            .expect("the fields are a JSON object"),
        ..Block::default()
    };
    store.insert_block(&mut image, &user_id).await.unwrap();
    store.delete_block(&image.id, &user_id).await.unwrap();

    let kept = store.get_file_record(other, stored_name).await.unwrap();
    assert_eq!(
        kept, other_file,
        "the file record of the block's board outlives the block"
    );
    let kept = store.get_file_record(own, stored_name).await.unwrap();
    assert_eq!(kept, own_file, "the file of the other board is untouched");

    test.teardown().await;
}

/// A file record of the board under the stored name, as an upload makes it.
fn record_of(board_id: &str, stored_name: &str) -> FileRecord {
    FileRecord {
        id: new_id(),
        container_id: board_id.to_owned(),
        stored_name: stored_name.to_owned(),
        created_at: now_millis(),
        path: format!("boards/{board_id}/{stored_name}"),
        name: "upload.png".to_owned(),
        extension: ".png".to_owned(),
        size: 7,
        ..FileRecord::default()
    }
}

/// A file record requires an existing board: saving one for a missing board returns
/// not-found and stores nothing.
#[tokio::test]
async fn a_file_record_for_a_board_that_does_not_exist_is_refused() {
    let test = common::setup().await;
    let store = &test.store;
    let user_id = common::user(store, "kelly.kapoor").await;
    let workspace_id = common::workspace(store).await;
    let boards = common::create_test_boards(store, &workspace_id, &user_id, 1).await;

    let no_board = new_id();
    let record = record_of(&no_board, "7orphan.png");
    let error = store
        .save_file_record(&record)
        .await
        .expect_err("a record for a board that does not exist is refused");
    assert!(
        matches!(&error, StoreError::NotFound(what) if what == "board"),
        "got {error:?}"
    );

    for board_id in [&no_board, &boards[0].id] {
        let error = store
            .get_file_record(board_id, "7orphan.png")
            .await
            .expect_err("the refused record is not stored");
        assert!(
            matches!(error, StoreError::NotFound(_)),
            "under board {board_id}: got {error:?}"
        );
    }

    // The same record for a board that exists is stored.
    let record = FileRecord {
        container_id: boards[0].id.clone(),
        ..record
    };
    store
        .save_file_record(&record)
        .await
        .expect("a record for a board that exists is stored");

    test.teardown().await;
}

/// A board has at most one file under a given stored name: a second record is refused as
/// a duplicate and changes nothing, while two different boards may reuse the same name.
#[tokio::test]
async fn a_stored_name_is_taken_once_per_board() {
    let test = common::setup().await;
    let store = &test.store;
    let user_id = common::user(store, "oscar.martinez").await;
    let workspace_id = common::workspace(store).await;
    let boards = common::create_test_boards(store, &workspace_id, &user_id, 2).await;
    let (one, other) = (&boards[0].id, &boards[1].id);
    let stored_name = "7ledger.png";

    let first = record_of(one, stored_name);
    store
        .save_file_record(&first)
        .await
        .expect("the board's first file under the name is stored");

    let second = FileRecord {
        name: "another upload.png".to_owned(),
        size: 9,
        ..record_of(one, stored_name)
    };
    let error = store
        .save_file_record(&second)
        .await
        .expect_err("a second record of the board under the name is refused");
    assert!(
        matches!(
            &error,
            StoreError::Database(sqlx::Error::Database(refusal)) if refusal.is_unique_violation()
        ),
        "the refusal is the database's, of a duplicate: got {error:?}"
    );
    let found = store
        .get_file_record(one, stored_name)
        .await
        .expect("the board still finds its file");
    assert_eq!(found, first, "the refused record changed nothing");

    let of_other = FileRecord {
        name: "theirs.png".to_owned(),
        ..record_of(other, stored_name)
    };
    store
        .save_file_record(&of_other)
        .await
        .expect("another board has a file of its own under the same name");
    for (board_id, expected) in [(one, &first), (other, &of_other)] {
        let found = store
            .get_file_record(board_id, stored_name)
            .await
            .expect("each board finds a file under the name");
        assert_eq!(&found, expected, "board {board_id} finds its own file");
    }

    test.teardown().await;
}

/// A file record goes with its board: while the board is in the trash the record is not
/// found, and it comes back, unchanged, when the board is restored. The record of
/// another board stays throughout.
#[tokio::test]
async fn a_file_record_goes_with_its_board_and_comes_back_with_it() {
    let test = common::setup().await;
    let store = &test.store;
    let user_id = common::user(store, "angela.martin").await;
    let workspace_id = common::workspace(store).await;
    let boards = common::create_test_boards(store, &workspace_id, &user_id, 2).await;
    let (doomed, other) = (&boards[0].id, &boards[1].id);
    let stored_name = "7party.png";

    let records = [
        record_of(doomed, stored_name),
        record_of(doomed, "7second.png"),
    ];
    for record in &records {
        store
            .save_file_record(record)
            .await
            .expect("a file of the board is stored");
    }
    let of_other = record_of(other, stored_name);
    store
        .save_file_record(&of_other)
        .await
        .expect("a file of the other board is stored");

    store
        .delete_container(doomed, &user_id)
        .await
        .expect("the board is deleted");
    for record in &records {
        let error = store
            .get_file_record(doomed, &record.stored_name)
            .await
            .expect_err("the files of a deleted board are not found");
        assert!(
            matches!(error, StoreError::NotFound(_)),
            "{}: got {error:?}",
            record.stored_name
        );
    }
    let kept = store
        .get_file_record(other, stored_name)
        .await
        .expect("the other board still finds its file");
    assert_eq!(kept, of_other, "the file of the other board is untouched");

    store
        .undelete_container(doomed, &user_id)
        .await
        .expect("the board is restored");
    for record in &records {
        let back = store
            .get_file_record(doomed, &record.stored_name)
            .await
            .expect("the files come back with their board");
        assert_eq!(&back, record, "the record is as it was, its place included");
    }
    let kept = store
        .get_file_record(other, stored_name)
        .await
        .expect("the other board still finds its file");
    assert_eq!(kept, of_other, "the file of the other board is untouched");

    test.teardown().await;
}

/// The stored names of the files that no block has shown since before `before`, after
/// they were collected.
async fn collected(store: &fvoci_store::Store, before: i64) -> Vec<String> {
    let mut names: Vec<String> = store
        .collect_unreferenced_files(before)
        .await
        .expect("the files no block shows are collected")
        .into_iter()
        .map(|file| file.stored_name)
        .collect();
    names.sort();
    names
}

/// A card whose body shows the files.
fn card_showing(board_id: &str, names: &[&str]) -> Block {
    let mut content =
        vec![json!({"type": "paragraph", "content": [{"type": "text", "text": "x"}]})];
    content.extend(names.iter().map(
        |name| json!({"type": crate::image_files::node_kind(name), "attrs": {"fileId": name}}),
    ));
    Block {
        id: new_id(),
        container_id: board_id.to_owned(),
        block_type: BlockType::Card,
        body: fvoci_model::Body::parse(json!({"type": "doc", "content": content}))
            .expect("the body may be stored"),
        ..Block::default()
    }
}

/// A file referenced by an image block or a card's body stays; one that no block shows,
/// live or in the trash, is collected once the grace period has passed since it became
/// unreferenced, and its record goes.
#[tokio::test]
async fn a_file_no_block_shows_is_collected_after_the_grace_period() {
    let test = common::setup().await;
    let store = &test.store;
    let user_id = common::user(store, "toby.flenderson").await;
    let workspace_id = common::workspace(store).await;
    let boards = common::create_test_boards(store, &workspace_id, &user_id, 1).await;
    let board_id = &boards[0].id;
    let started = now_millis();
    let later = started + 3_600_000;
    for name in ["7image.png", "7body.png", "7block.pdf", "7none.png"] {
        store
            .save_file_record(&record_of(board_id, name))
            .await
            .expect("a file of the board is stored");
    }
    let mut card = card_showing(board_id, &["7body.png", "7block.pdf"]);
    common::insert_blocks(store, std::slice::from_mut(&mut card), &user_id).await;
    let mut image = Block {
        id: new_id(),
        container_id: board_id.clone(),
        parent_id: card.id.clone(),
        block_type: BlockType::Attachment,
        fields: serde_json::from_value(json!({ "fileId": "7image.png" }))
            .expect("the fields are a JSON object"),
        ..Block::default()
    };
    common::insert_blocks(store, std::slice::from_mut(&mut image), &user_id).await;

    assert_eq!(
        collected(store, started - 1).await,
        Vec::<String>::new(),
        "within the grace period nothing is collected"
    );
    assert_eq!(
        collected(store, later).await,
        ["7none.png"],
        "the file no block shows, counted from its upload"
    );
    let error = store
        .get_file_record(board_id, "7none.png")
        .await
        .expect_err("the record of a collected file is gone");
    assert!(matches!(error, StoreError::NotFound(_)), "{error:?}");

    // The body no longer shows its file; the version kept of the body before does.
    card.body = None;
    store
        .insert_block(&mut card, &user_id)
        .await
        .expect("the card is changed");
    // A block in the trash still shows its file: it can come back.
    store
        .delete_block(&image.id, &user_id)
        .await
        .expect("the image is deleted");
    assert_eq!(
        collected(store, later).await,
        Vec::<String>::new(),
        "a version of the body can be gone back to with its file"
    );
    // Once that version is kept no longer, nothing shows the file.
    store.set_body_versions_kept(1);
    card.body = card_showing(board_id, &[]).body;
    store
        .insert_block(&mut card, &user_id)
        .await
        .expect("the card is changed again");
    assert_eq!(collected(store, later).await, ["7block.pdf", "7body.png"]);

    // Once the image is gone for good, so is what it showed.
    store
        .purge_trash(now_millis() + 60_000)
        .await
        .expect("the trash is purged");
    assert_eq!(collected(store, started - 1).await, Vec::<String>::new());
    assert_eq!(collected(store, later).await, ["7image.png"]);

    test.teardown().await;
}

/// A block that comes back from the trash shows its files again, and an unreferenced file
/// of a restored board is counted from the restoring time, not from its upload.
#[tokio::test]
async fn a_restored_block_or_board_shows_its_files_again() {
    let test = common::setup().await;
    let store = &test.store;
    let user_id = common::user(store, "creed.bratton").await;
    let workspace_id = common::workspace(store).await;
    let boards = common::create_test_boards(store, &workspace_id, &user_id, 1).await;
    let board_id = &boards[0].id;
    let mut old = record_of(board_id, "7old.png");
    old.created_at = 1;
    store.save_file_record(&old).await.expect("an old file");
    for name in ["7shown.png", "7shown.pdf"] {
        store
            .save_file_record(&record_of(board_id, name))
            .await
            .expect("a shown file");
    }
    let mut card = card_showing(board_id, &["7shown.png", "7shown.pdf"]);
    common::insert_blocks(store, std::slice::from_mut(&mut card), &user_id).await;

    store
        .delete_block(&card.id, &user_id)
        .await
        .expect("the card is deleted");
    store
        .undelete_block(&card.id, &user_id)
        .await
        .expect("the card is restored");
    store
        .delete_container(board_id, &user_id)
        .await
        .expect("the board is deleted");
    let restored_at = now_millis();
    store
        .undelete_container(board_id, &user_id)
        .await
        .expect("the board is restored");

    assert_eq!(
        collected(store, restored_at - 1).await,
        Vec::<String>::new(),
        "the old file is counted from the restoring"
    );
    assert_eq!(
        collected(store, now_millis() + 3_600_000).await,
        ["7old.png"],
        "the card that came back shows its file"
    );

    test.teardown().await;
}

/// A body that shows the files, or says `text` without any.
fn body_showing(text: &str, names: &[&str]) -> Option<fvoci_model::Body> {
    let mut content =
        vec![json!({"type": "paragraph", "content": [{"type": "text", "text": text}]})];
    content.extend(
        names
            .iter()
            .map(|name| json!({"type": "image", "attrs": {"fileId": name}})),
    );
    fvoci_model::Body::parse(json!({"type": "doc", "content": content}))
        .expect("the body may be stored")
}

/// The texts of the versions of a comment.
async fn version_texts(store: &fvoci_store::Store, comment_id: &str) -> Vec<(i64, String)> {
    store
        .get_comment_versions(comment_id)
        .await
        .expect("the versions are read")
        .into_iter()
        .map(|version| {
            let text = version.body.map_or_default(|body| body.text());
            (version.version, text)
        })
        .collect()
}

/// A patch of a comment's title that sets its body to the text, or keeps the body.
pub(crate) fn title_and_body(body: Option<&str>) -> fvoci_model::BlockPatch {
    fvoci_model::BlockPatch {
        title: Some("title".to_owned()),
        body: body.map_or(fvoci_model::BodyChange::Keep, |words| {
            fvoci_model::BodyChange::Set(body_showing(words, &[]).expect("a body"))
        }),
        ..fvoci_model::BlockPatch::default()
    }
}

/// A board with the files `7first.png` and `7second.png`, and a comment on a card of it,
/// posted by `author`, whose body says `v1` and shows the first file.
pub(crate) async fn posted_comment(store: &fvoci_store::Store, author: &str) -> Block {
    let workspace_id = common::workspace(store).await;
    let boards = common::create_test_boards(store, &workspace_id, author, 1).await;
    let board_id = &boards[0].id;
    for name in ["7first.png", "7second.png"] {
        store
            .save_file_record(&record_of(board_id, name))
            .await
            .expect("a file of the board is stored");
    }
    let mut card = card_showing(board_id, &[]);
    common::insert_blocks(store, std::slice::from_mut(&mut card), author).await;
    let mut comment = Block {
        id: new_id(),
        container_id: board_id.clone(),
        parent_id: card.id.clone(),
        block_type: BlockType::Comment,
        body: body_showing("v1", &["7first.png"]),
        ..Block::default()
    };
    common::insert_blocks(store, std::slice::from_mut(&mut comment), author).await;
    store.get_block(&comment.id).await.expect("the comment")
}

/// Each change of a comment's body keeps what it said before, with who wrote it and when,
/// up to the set number of versions (the first and the newest). The files of the kept
/// versions stay, and the versions go with the comment when it is gone for good, not
/// while it is in the trash.
#[tokio::test]
async fn a_changed_comment_keeps_what_it_said_before() {
    let test = common::setup().await;
    let store = &test.store;
    let author = common::user(store, "meredith.palmer").await;
    let editor = common::user(store, "kelly.kapoor").await;
    let posted = posted_comment(store, &author).await;
    let id = &posted.id;
    assert_eq!(posted.edited_at, 0, "never changed");
    assert_eq!(version_texts(store, id).await, [], "no versions yet");

    // A change of the title alone makes no version.
    store
        .patch_block(id, &title_and_body(None), &author)
        .await
        .expect("the title is changed");
    assert_eq!(version_texts(store, id).await, []);

    store
        .patch_block(id, &title_and_body(Some("v2")), &editor)
        .await
        .expect("the body is changed");
    let changed = store.get_block(id).await.expect("the comment");
    assert_eq!(
        changed.edited_at, changed.updated_at,
        "when the body changed"
    );
    let versions = store.get_comment_versions(id).await.expect("the versions");
    assert_eq!(versions.len(), 1, "{versions:?}");
    assert_eq!(versions[0].version, 1);
    assert_eq!(
        versions[0].written_by, author,
        "the text it was posted with"
    );
    assert_eq!(versions[0].written_at, posted.created_at);
    // A body that stays makes none either.
    store
        .patch_block(id, &title_and_body(Some("v2")), &editor)
        .await
        .expect("the same body again");
    assert_eq!(version_texts(store, id).await, [(1, "v1".to_owned())]);

    // The first version shows its file: it stays while the version is kept.
    let later = now_millis() + 3_600_000;
    assert_eq!(collected(store, later).await, ["7second.png"]);

    // With three kept, the first and the two newest stay.
    store.set_comment_versions_kept(3);
    for words in ["v3", "v4", "v5", "v6"] {
        store
            .patch_block(id, &title_and_body(Some(words)), &editor)
            .await
            .expect("the body is changed");
    }
    let versions = store.get_comment_versions(id).await.expect("the versions");
    assert_eq!(
        version_texts(store, id).await,
        [
            (1, "v1".to_owned()),
            (4, "v4".to_owned()),
            (5, "v5".to_owned())
        ],
        "the first, then the newest; the body has v6"
    );
    assert_eq!(versions[1].written_by, editor, "who changed it to v4");
    store.set_comment_versions_kept(fvoci_store::DEFAULT_COMMENT_VERSIONS_KEPT);

    // In the trash the versions stay, and back from it too.
    store.delete_block(id, &author).await.expect("deleted");
    assert_eq!(version_texts(store, id).await.len(), 3);
    store.undelete_block(id, &author).await.expect("restored");
    assert_eq!(version_texts(store, id).await.len(), 3);
    assert_eq!(collected(store, later).await, Vec::<String>::new());

    // Gone for good, the versions go, and so does what only they showed.
    store.delete_block(id, &author).await.expect("deleted");
    store
        .purge_trash(now_millis() + 60_000)
        .await
        .expect("the trash is purged");
    assert_eq!(version_texts(store, id).await, []);
    assert_eq!(collected(store, later).await, ["7first.png"]);

    test.teardown().await;
}

/// A patch that writes a body saying `words` and showing `names`.
fn body_patch(words: &str, names: &[&str]) -> fvoci_model::BlockPatch {
    fvoci_model::BlockPatch {
        body: fvoci_model::BodyChange::Set(body_showing(words, names).expect("a body")),
        ..fvoci_model::BlockPatch::default()
    }
}

/// With one comment version kept, only the first stays, and the file of a version that
/// goes is collected once its grace window passes.
#[tokio::test]
async fn pruning_comment_versions_releases_files_shown_only_in_dropped_versions() {
    let test = common::setup().await;
    let store = &test.store;
    let author = common::user(store, "jan.levinson").await;
    let posted = posted_comment(store, &author).await;
    let id = &posted.id;
    let later = now_millis() + 3_600_000;

    store
        .patch_block(id, &body_patch("v2", &["7second.png"]), &author)
        .await
        .expect("the comment is changed");
    store
        .patch_block(id, &body_patch("v3", &[]), &author)
        .await
        .expect("the comment is changed again");
    assert_eq!(
        collected(store, later).await,
        Vec::<String>::new(),
        "the versions' files stay while kept"
    );

    store.set_comment_versions_kept(1);
    store
        .patch_block(id, &body_patch("v4", &[]), &author)
        .await
        .expect("the comment is changed with one version kept");
    assert_eq!(
        version_texts(store, id).await,
        [(1, "v1".to_owned())],
        "only the first"
    );
    assert_eq!(
        collected(store, later).await,
        ["7second.png"],
        "the file of a version that went"
    );
    store.set_comment_versions_kept(fvoci_store::DEFAULT_COMMENT_VERSIONS_KEPT);

    test.teardown().await;
}

/// The organization the keys of a board's files begin with is that of the board's
/// workspace; a board the handle does not reach has none (PostgreSQL: another
/// organization's).
#[tokio::test]
async fn a_boards_files_are_of_its_workspaces_organization() {
    let test = common::setup().await;
    let user_id = common::user(&test, "owner").await;
    let workspace_id = common::workspace(&test).await;
    let boards = common::create_test_boards(&test, &workspace_id, &user_id, 1).await;
    assert_eq!(
        test.org_of_container(&boards[0].id).await.expect("found"),
        test.org_id,
        "the test's organization"
    );
    let missing = test.org_of_container(&new_id()).await;
    assert!(
        matches!(missing, Err(StoreError::NotFound(_))),
        "{missing:?}"
    );
    if common::db_type() == fvoci_store::DbType::Postgres {
        let unset = test.scoped(fvoci_store::Tenant::Unset);
        let (other_org, other_workspace) = common::organization(&unset, "the other").await;
        let theirs = test.for_org(&other_org).expect("an identifier");
        let other_user = common::user(&theirs, "theirs").await;
        let other_boards =
            common::create_test_boards(&theirs, &other_workspace, &other_user, 1).await;
        let other_board = &other_boards[0].id;
        assert_eq!(
            theirs.org_of_container(other_board).await.expect("found"),
            other_org,
            "theirs by their handle"
        );
        let from_ours = test.org_of_container(other_board).await;
        assert!(
            matches!(from_ours, Err(StoreError::NotFound(_))),
            "not by ours: {from_ours:?}"
        );
    }
    test.teardown().await;
}
