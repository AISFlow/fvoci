//! Rows intentionally lacking a live-parent foreign key still need tenant-local keys.
//!
//! Each test stores, through one organization's handle, a row that names what is
//! another organization's, as a mistaken write below the server's checks would: what
//! the row may then take, block, read, restore or keep alive of the other's.

#[expect(
    dead_code,
    reason = "the existing store harness is shared by many suites"
)]
#[path = "store/common.rs"]
mod common;

use fvoci_model::{
    Block, BlockPatch, BlockType, Body, BodyChange, Category, Container, ContainerKinds,
    ContainerVisibility, FileRecord, Upload, UploadPurpose, new_id,
};
use fvoci_store::{DbType, SYSTEM_ORG_ID, SYSTEM_WORKSPACE_ID, Store};

fn body(text: &str) -> Body {
    Body::from_stored(serde_json::json!({
        "type": "doc", "content": [{"type": "paragraph", "content": [{"type": "text", "text": text}]}]
    }))
}

/// A block of `block_type` on a board, below `parent_id` (empty for none), stored.
async fn block(
    store: &Store,
    board_id: &str,
    parent_id: &str,
    block_type: BlockType,
    user_id: &str,
) -> Block {
    let mut block = Block {
        id: new_id(),
        container_id: board_id.to_owned(),
        parent_id: parent_id.to_owned(),
        block_type,
        ..Block::default()
    };
    store
        .insert_block(&mut block, user_id)
        .await
        .expect("the block is stored");
    block
}

/// The second organization of a test: its handle, its workspace, a user of it and an
/// open board in that workspace.
struct Other {
    store: Store,
    workspace_id: String,
    user_id: String,
    board_id: String,
}

async fn other(test: &common::TestStore) -> Other {
    let (org_id, workspace_id) = common::organization(test, "B").await;
    let store = test.for_org(&org_id).expect("B's organization");
    let user_id = common::user(&store, "B-owner").await;
    let boards = common::create_test_boards(&store, &workspace_id, &user_id, 1).await;
    Other {
        store,
        workspace_id,
        user_id,
        board_id: boards[0].id.clone(),
    }
}

/// A write allowed by A's RLS must not reserve the next history slot of B's comment.
/// This deliberately models a mistaken tenant write below the server checks, as the
/// preceding review's composite-key tests did; it is not an HTTP injection test.
#[tokio::test]
async fn one_orgs_history_row_cannot_prevent_another_orgs_comment_edit() {
    if common::db_type() != DbType::Postgres {
        return;
    }
    let test = common::setup().await;
    let b = other(&test).await;
    // A comment is on a card of its board.
    let card = block(&b.store, &b.board_id, "", BlockType::Card, &b.user_id).await;
    let mut comment = Block {
        id: new_id(),
        container_id: b.board_id.clone(),
        parent_id: card.id.clone(),
        block_type: BlockType::Comment,
        body: Some(body("B's original comment")),
        ..Block::default()
    };
    b.store
        .insert_block(&mut comment, &b.user_id)
        .await
        .expect("B's comment exists");
    let inserted = test
        .execute_unchecked(&format!(
            "INSERT INTO comment_versions (block_id, version, body, files, written_at) \
         VALUES ('{}', 1, NULL, '[]', 0)",
            comment.id
        ))
        .await;
    assert!(
        b.store
            .get_comment_versions(&comment.id)
            .await
            .expect("B's visible history")
            .is_empty(),
        "B cannot read A's row"
    );
    let edited = b
        .store
        .patch_block(
            &comment.id,
            &BlockPatch {
                body: BodyChange::Set(body("B's authorized change")),
                ..BlockPatch::default()
            },
            &b.user_id,
        )
        .await;
    test.teardown().await;
    assert_eq!(
        inserted.as_ref().ok(),
        Some(&1),
        "A stores its own row under B's comment"
    );
    assert!(
        edited.is_ok(),
        "A's own history row prevented B from editing its comment: insert={inserted:?}, edit={edited:?}"
    );
}

/// The same collision is possible for the references that outlive a deleted block.
#[tokio::test]
async fn one_orgs_file_reference_cannot_prevent_another_orgs_attachment_edit() {
    if common::db_type() != DbType::Postgres {
        return;
    }
    let test = common::setup().await;
    let b = other(&test).await;
    let b_card = block(&b.store, &b.board_id, "", BlockType::Card, &b.user_id).await;
    let attachment = block(
        &b.store,
        &b.board_id,
        &b_card.id,
        BlockType::Attachment,
        &b.user_id,
    )
    .await;
    let inserted = test
        .execute_unchecked(&format!(
            "INSERT INTO file_refs (block_id, container_id, name) VALUES ('{}', '{}', 'B-file.txt')",
            attachment.id, b.board_id
        ))
        .await;
    let edited = b
        .store
        .patch_block(
            &attachment.id,
            &BlockPatch {
                updated_fields: serde_json::json!({"fileId": "B-file.txt"})
                    .as_object()
                    .expect("fields")
                    .clone(),
                ..BlockPatch::default()
            },
            &b.user_id,
        )
        .await;
    test.teardown().await;
    assert_eq!(
        inserted.as_ref().ok(),
        Some(&1),
        "A stores its own row under B's attachment"
    );
    assert!(
        edited.is_ok(),
        "A's own reference row prevented B from updating its attachment: insert={inserted:?}, edit={edited:?}"
    );
}

/// The trash keeps a block or a board under the identifier it had: a row of A's trash
/// under the identifier of B's card or board does not keep B from deleting either, nor
/// from restoring it.
#[tokio::test]
async fn one_orgs_trash_row_cannot_prevent_another_orgs_deletion() {
    if common::db_type() != DbType::Postgres {
        return;
    }
    let test = common::setup().await;
    let b = other(&test).await;
    let card = block(&b.store, &b.board_id, "", BlockType::Card, &b.user_id).await;
    let forged = [
        format!(
            "INSERT INTO deleted_blocks (id, container_id, deleted_at, deletion_id, block) \
             VALUES ('{}', '{}', 0, '{}', '{{}}')",
            card.id,
            b.board_id,
            new_id()
        ),
        format!(
            "INSERT INTO deleted_containers (id, workspace_id, deleted_at, deletion_id, container, members, \
             files) VALUES ('{}', '{}', 0, '{}', '{{}}', '[]', '[]')",
            b.board_id,
            test.workspace_id,
            new_id()
        ),
    ];
    for statement in &forged {
        assert_eq!(
            test.execute_unchecked(statement).await.ok(),
            Some(1),
            "A stores its own row: {statement}"
        );
    }
    let deleted_card = b.store.delete_block(&card.id, &b.user_id).await;
    let restored_card = b.store.undelete_block(&card.id, &b.user_id).await;
    let deleted_board = b.store.delete_container(&b.board_id, &b.user_id).await;
    let restored_board = b.store.undelete_container(&b.board_id, &b.user_id).await;
    test.teardown().await;
    assert!(
        deleted_card.is_ok() && restored_card.is_ok(),
        "A's trash row kept B's card: delete={deleted_card:?}, restore={restored_card:?}"
    );
    assert!(
        deleted_board.is_ok() && restored_board.is_ok(),
        "A's trash row kept B's board: delete={deleted_board:?}, restore={restored_board:?}"
    );
}

/// A row of A's trash that names B's board or workspace (`deleted_blocks.container_id`,
/// `deleted_containers.workspace_id`, which have no key: what they name may be in the trash
/// too) is not among what B finds in its trash, and does not come back into B's board
/// or workspace: the keys of the live tables refuse it.
#[tokio::test]
async fn a_trash_row_under_another_orgs_parent_is_neither_listed_nor_restored() {
    if common::db_type() != DbType::Postgres {
        return;
    }
    let test = common::setup().await;
    let user_id = common::user(&test, "A-owner").await;
    let ours = common::create_test_boards(&test, &test.workspace_id, &user_id, 2).await;
    let b = other(&test).await;

    let card = block(&test, &ours[0].id, "", BlockType::Card, &user_id).await;
    test.delete_block(&card.id, &user_id)
        .await
        .expect("A's card goes to the trash");
    test.delete_container(&ours[1].id, &user_id)
        .await
        .expect("A's board goes to the trash");
    let moved = [
        format!(
            "UPDATE deleted_blocks SET container_id = '{0}', \
             block = jsonb_set(block::jsonb, '{{boardId}}', '\"{0}\"')::json WHERE id = '{1}'",
            b.board_id, card.id
        ),
        format!(
            "UPDATE deleted_containers SET workspace_id = '{0}', \
             container = jsonb_set(container::jsonb, '{{workspaceId}}', '\"{0}\"')::json WHERE id = '{1}'",
            b.workspace_id, ours[1].id
        ),
    ];
    for statement in &moved {
        assert_eq!(
            test.execute_unchecked(statement).await.ok(),
            Some(1),
            "A's row names B's parent: {statement}"
        );
    }

    let b_cards = b
        .store
        .get_deleted_cards(&b.board_id, None)
        .await
        .expect("B's trash of the board");
    let b_boards = b
        .store
        .get_deleted_containers(
            &b.workspace_id,
            Some(&user_id),
            None,
            ContainerKinds::BOARDS,
        )
        .await
        .expect("B's trash of the workspace");
    let restored_card = test.undelete_block(&card.id, &user_id).await;
    let restored_board = test.undelete_container(&ours[1].id, &user_id).await;
    let b_blocks = b
        .store
        .get_blocks(&fvoci_store::BlockFilter::container(&b.board_id))
        .await
        .expect("B's blocks");
    test.teardown().await;

    assert!(b_cards.is_empty(), "B lists A's card: {b_cards:?}");
    assert!(b_boards.is_empty(), "B lists A's board: {b_boards:?}");
    assert!(
        restored_card.is_err() && !common::contains_block_with_id(&b_blocks, &card.id),
        "A's card came back onto B's board: {restored_card:?}"
    );
    assert!(
        restored_board.is_err(),
        "A's board came back into B's workspace: {restored_board:?}"
    );
}

/// A reference of A's (`file_refs.container_id` has no key: the block may be in the
/// trash) that names B's board and the stored name of B's file does not keep that
/// file shown when the housekeeping, which reaches every organization, marks the files
/// that no block shows: the file is collected as one that nothing of B shows.
#[tokio::test]
async fn one_orgs_file_reference_keeps_no_file_of_another_alive() {
    if common::db_type() != DbType::Postgres {
        return;
    }
    let test = common::setup().await;
    let user_id = common::user(&test, "A-owner").await;
    let ours = common::create_test_boards(&test, &test.workspace_id, &user_id, 1).await;
    let ours = block(&test, &ours[0].id, "", BlockType::Card, &user_id).await;
    let b = other(&test).await;
    let file = FileRecord {
        id: new_id(),
        container_id: b.board_id.clone(),
        stored_name: "B-file.txt".to_owned(),
        name: "B-file.txt".to_owned(),
        path: "B/B-file.txt".to_owned(),
        created_at: 1,
        ..FileRecord::default()
    };
    b.store
        .save_file_record(&file)
        .await
        .expect("B's file is recorded, shown by no block");
    test.execute_unchecked(&format!(
        "INSERT INTO file_refs (block_id, container_id, name) VALUES ('{}', '{}', '{}')",
        ours.id, b.board_id, file.stored_name
    ))
    .await
    .expect("A's block names B's file");

    let system = test.system();
    system.purge_trash(0).await.expect("the trash is purged");
    let collected = system
        .collect_unreferenced_files(i64::MAX)
        .await
        .expect("the files no block shows are collected");
    test.teardown().await;
    assert!(
        collected.iter().any(|collected| collected.id == file.id),
        "A's reference kept B's file shown: {collected:?}"
    );
}

/// The purge, which reaches every organization, forgets what blocks gone for good
/// showed and the versions they had: a reference and a version of A's under the
/// identifier of B's live block (A has no such block) are forgotten, and B's own are
/// kept.
#[tokio::test]
async fn the_purge_forgets_a_row_whose_block_is_only_another_orgs() {
    if common::db_type() != DbType::Postgres {
        return;
    }
    let test = common::setup().await;
    let user_id = common::user(&test, "A-owner").await;
    let ours = common::create_test_boards(&test, &test.workspace_id, &user_id, 1).await;
    let b = other(&test).await;
    let b_card = block(&b.store, &b.board_id, "", BlockType::Card, &b.user_id).await;
    let attachment = block(
        &b.store,
        &b.board_id,
        &b_card.id,
        BlockType::Attachment,
        &b.user_id,
    )
    .await;
    let forged = [
        format!(
            "INSERT INTO file_refs (block_id, container_id, name) VALUES ('{}', '{}', 'A-file.txt')",
            attachment.id, ours[0].id
        ),
        format!(
            "INSERT INTO comment_versions (block_id, version, body, files, written_at) \
             VALUES ('{}', 1, NULL, '[]', 0)",
            attachment.id
        ),
    ];
    for statement in &forged {
        assert_eq!(
            test.execute_unchecked(statement).await.ok(),
            Some(1),
            "A stores its own row: {statement}"
        );
    }
    let b_reference = format!(
        "INSERT INTO file_refs (block_id, container_id, name) VALUES ('{}', '{}', 'B-file.txt')",
        attachment.id, b.board_id
    );
    assert_eq!(
        b.store.execute_unchecked(&b_reference).await.ok(),
        Some(1),
        "B's own reference"
    );

    test.system()
        .purge_trash(0)
        .await
        .expect("the trash is purged");
    let left = [
        test.count_rows("file_refs").await,
        test.count_rows("comment_versions").await,
        b.store.count_rows("file_refs").await,
    ];
    test.teardown().await;
    assert_eq!(
        left.map(Result::ok),
        [Some(0), Some(0), Some(1)],
        "A's reference and version, then B's reference"
    );
}

/// An upload goes to a board of its organization, or an import to a workspace of it
/// (`uploads.container_id` names either, and has no key: the board may go to the trash and
/// for good while the upload's row stays for its clean-up). The schema checks it when
/// the row is stored.
#[tokio::test]
async fn an_upload_names_a_board_or_a_workspace_of_its_own_organization() {
    if common::db_type() != DbType::Postgres {
        return;
    }
    let test = common::setup().await;
    let user_id = common::user(&test, "A-owner").await;
    let ours = common::create_test_boards(&test, &test.workspace_id, &user_id, 1).await;
    let b = other(&test).await;
    let upload = |purpose: UploadPurpose, target: &str| Upload {
        id: new_id(),
        user_id: user_id.clone(),
        purpose,
        container_id: target.to_owned(),
        workspace_id: target.to_owned(),
        name: "upload.bin".to_owned(),
        size: 1,
        part_size: 1,
        created_at: 1,
        expires_at: i64::MAX,
        ..Upload::default()
    };
    let mut answers = Vec::new();
    for (purpose, target) in [
        (UploadPurpose::File, b.board_id.as_str()),
        (UploadPurpose::Import, b.workspace_id.as_str()),
        (UploadPurpose::File, test.workspace_id.as_str()),
        (UploadPurpose::Import, ours[0].id.as_str()),
        (UploadPurpose::File, ours[0].id.as_str()),
        (UploadPurpose::Import, test.workspace_id.as_str()),
    ] {
        let answer = test.create_upload(&upload(purpose, target)).await;
        answers.push((
            purpose,
            answer.map_err(|error| common::refused_over_a_foreign_key(&error)),
        ));
    }
    test.teardown().await;
    assert_eq!(
        answers,
        [
            (UploadPurpose::File, Err(true)),
            (UploadPurpose::Import, Err(true)),
            (UploadPurpose::File, Err(true)),
            (UploadPurpose::Import, Err(true)),
            (UploadPurpose::File, Ok(())),
            (UploadPurpose::Import, Ok(())),
        ],
        "B's board and workspace, ours of the other purpose, then ours"
    );
}

/// A category of a user's takes a board of its organization or a built-in template,
/// the system organization's (`category_containers.container_id` refers to the board without
/// the organization), and the same user files the same template in two
/// organizations, once in each.
#[tokio::test]
async fn a_category_takes_a_board_of_its_organization_or_a_template() {
    if common::db_type() != DbType::Postgres {
        return;
    }
    let test = common::setup().await;
    let user_id = common::user(&test, "A-owner").await;
    let b = other(&test).await;
    let system = test
        .for_org(SYSTEM_ORG_ID)
        .expect("the system organization");
    let mut template = Container {
        id: new_id(),
        workspace_id: SYSTEM_WORKSPACE_ID.to_owned(),
        visibility: ContainerVisibility::Open,
        is_template: true,
        ..Container::default()
    };
    system
        .insert_container(&mut template, &user_id)
        .await
        .expect("the template is stored");
    let category = |store: &Store, workspace_id: &str| {
        let category = Category {
            id: new_id(),
            name: "Boards".to_owned(),
            user_id: user_id.clone(),
            workspace_id: workspace_id.to_owned(),
            ..Category::default()
        };
        let store = store.clone();
        async move {
            store
                .create_category(&category)
                .await
                .expect("the category is made");
            category.id
        }
    };
    let ours = category(&test, &test.workspace_id).await;
    let theirs = category(&b.store, &b.workspace_id).await;

    let b_board = test
        .add_update_category_container(&user_id, &ours, std::slice::from_ref(&b.board_id))
        .await;
    let filed_in_a = test
        .add_update_category_container(&user_id, &ours, std::slice::from_ref(&template.id))
        .await;
    let filed_in_b = b
        .store
        .add_update_category_container(&user_id, &theirs, std::slice::from_ref(&template.id))
        .await;
    let sidebar_of_a = test
        .get_user_category_containers(&user_id, &test.workspace_id)
        .await
        .expect("A's sidebar");
    let sidebar_of_b = b
        .store
        .get_user_category_containers(&user_id, &b.workspace_id)
        .await
        .expect("B's sidebar");
    test.teardown().await;

    assert!(
        b_board
            .as_ref()
            .is_err_and(fvoci_store::StoreError::is_not_found),
        "A filed B's board: {b_board:?}"
    );
    assert!(
        filed_in_a.is_ok() && filed_in_b.is_ok(),
        "the template is filed in each organization: {filed_in_a:?}, {filed_in_b:?}"
    );
    for filed in [&sidebar_of_a, &sidebar_of_b] {
        let boards: Vec<&str> = filed
            .iter()
            .flat_map(|category| &category.container_metadata)
            .map(|board| board.container_id.as_str())
            .collect();
        assert_eq!(boards, [template.id.as_str()], "{filed:?}");
    }
}

/// The keys of the organizations' tables, from the catalog: each that does not name the
const NO_KEY_TRIGGERS: &[(&str, &str)] = &[
    // No key: what keeps a record of the audit log from being changed, or
    // removed but with its workspace.
    ("audit_log", "audit_log_kept"),
    (
        "category_containers",
        "category_containers_container_of_its_organization",
    ),
    // No key says a diary is in its owner's own space alone.
    ("containers", "diaries_of_a_space"),
    // Course child records outlive trashing in `deleted_containers` until purge.
    ("course_exams", "course_exams_course_of_its_organization"),
    (
        "course_meetings",
        "course_meetings_course_of_its_organization",
    ),
    ("exam_scores", "exam_scores_course_of_its_organization"),
    // No key says who may be in a space: its owner alone, and no link leads
    // into one.
    ("invitations", "invitations_of_a_space"),
    ("organization_members", "organization_members_of_a_space"),
    // No key says a row stays what it was: an organization keeps its kind and
    // the account whose space it is.
    ("organizations", "organizations_keep_their_kind"),
    // Nor that no link leads into a diary.
    ("public_links", "public_links_of_no_diary"),
    ("unit_notes", "unit_notes_course_of_its_organization"),
    ("unit_progress", "unit_progress_course_of_its_organization"),
    ("unit_reviews", "unit_reviews_course_of_its_organization"),
    ("uploads", "uploads_target_of_its_organization"),
    ("workspace_members", "workspace_members_of_a_space"),
];

/// organization is unique across them all for a reason, and a later table whose key
/// borrows another table's identifier without the organization fails here. The rows
/// whose board cannot have a key are checked by a trigger when they are stored.
#[tokio::test]
async fn every_key_without_the_organization_is_one_of_the_few_that_need_none() {
    if common::db_type() != DbType::Postgres {
        return;
    }
    let test = common::setup().await;
    let mut conn = common::connect_past_the_store(&test.database_connection()).await;
    let keys: Vec<(String, String)> = sqlx::query_as(
        "SELECT c.relname::text, \
         substring(pg_get_indexdef(i.indexrelid) FROM 'USING btree (.*)$') \
         FROM pg_index AS i JOIN pg_class AS c ON c.oid = i.indrelid \
         WHERE i.indisunique AND c.relnamespace = current_schema()::regnamespace \
         ORDER BY 1, 2",
    )
    .fetch_all(&mut conn)
    .await
    .expect("the keys are in the catalog");
    let triggers: Vec<(String, String)> = sqlx::query_as(
        "SELECT tgrelid::regclass::text, tgname::text FROM pg_trigger \
         WHERE NOT tgisinternal AND tgrelid IN \
         (SELECT oid FROM pg_class WHERE relnamespace = current_schema()::regnamespace) \
         ORDER BY 1, 2",
    )
    .fetch_all(&mut conn)
    .await
    .expect("the triggers are in the catalog");
    drop(conn);
    test.teardown().await;

    let without: Vec<(&str, &str)> = keys
        .iter()
        .filter(|(table, _)| fvoci_store::testing::ORG_TABLES.contains(&table.as_str()))
        .filter(|(_, columns)| !columns.contains("org_id"))
        .map(|(table, columns)| (table.as_str(), columns.as_str()))
        .collect();
    assert_eq!(
        without,
        [
            // Held to its organization by the key to the workspace: (workspace_id,
            // org_id). A token is the server's; a workspace is of one organization.
            ("api_token_workspaces", "(token_id, workspace_id)"),
            // Held to its organization by the key to the container: (container_id, org_id).
            ("blocks", "(container_id, id)"),
            // A card's number on its board, held the same way: no organization's card
            // can stand on another's board to take one of its numbers.
            ("blocks", "(container_id, number)"),
            // An identifier the server makes for the row.
            ("blocks", "(id)"),
            ("categories", "(id)"),
            ("category_containers", "(id)"),
            ("container_members", "(container_id, user_id)"),
            ("containers", "(id)"),
            ("files", "(container_id, name)"),
            ("files", "(id)"),
            // The digest of a secret the server makes; a link is found by it before its
            // organization is known.
            ("invitations", "(token_hash)"),
            // The digest of a secret the server makes, which only the browser it was
            // made for holds; a held sign-in is read by it with its organization.
            ("oidc_held_sign_ins", "(held_hash)"),
            // The digest of a secret the server makes; a sign-in is found by it before
            // its organization is known.
            ("oidc_login_flows", "(state_hash)"),
            // An identifier the server makes; a sign-in names it with the organization.
            ("oidc_providers", "(id)"),
            ("organizations", "(id)"),
            // One system organization over them all.
            ("organizations", "(is_system) WHERE is_system"),
            // One space for an account over them all: an account is the server's.
            ("organizations", "(owner_id) WHERE (owner_id IS NOT NULL)"),
            ("public_links", "(container_id)"),
            // A secret the server makes.
            ("public_links", "(token)"),
            ("subscriptions", "(block_id, user_id)"),
            // Server-generated identifier for each personal time entry.
            ("time_entries", "(id)"),
            // At most one active timer per account across every organization.
            ("time_entries", "(user_id) WHERE (ended_at IS NULL)"),
            ("upload_parts", "(upload_id, number)"),
            ("uploads", "(id)"),
            ("workspace_members", "(workspace_id, user_id)"),
            ("workspaces", "(id)"),
        ],
        "the keys of the organizations' tables that do not name the organization"
    );
    let trigger_pairs: Vec<(&str, &str)> = triggers
        .iter()
        .map(|(table, name)| (table.as_str(), name.as_str()))
        .collect();
    assert_eq!(
        trigger_pairs, NO_KEY_TRIGGERS,
        "the checks of what has no key"
    );
}
