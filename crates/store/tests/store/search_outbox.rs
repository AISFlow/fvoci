//! What the search server is still to be told: the list every change of a searched
//! block and every going and coming of a board is put on in its own transaction, by a
//! store that keeps it, and what the worker reads and takes off.

use fvoci_model::{
    Block, BlockPatch, BlockType, Body, BodyChange, Container, ContainerKind, ContainerVisibility,
    NewPage, PageLimits, new_id,
};
use fvoci_store::{DbType, SearchChange, SearchIndexMark, SearchTarget, Store, Tenant};
use serde_json::json;

use crate::common;

/// A time by which every row is due.
const LATER: i64 = i64::MAX;

fn body(text: &str) -> Body {
    Body::parse(json!({"type": "doc", "content": [
        {"type": "paragraph", "content": [{"type": "text", "text": text}]},
    ]}))
    .expect("a document")
    .expect("a body")
}

/// A board of a new workspace of the store's organization, by a new user.
async fn board(store: &Store) -> (Container, String) {
    let user_id = common::user(store, &format!("u{}", &new_id()[24..])).await;
    let workspace_id = common::workspace(store).await;
    let mut boards = common::create_test_boards(store, &workspace_id, &user_id, 1).await;
    (boards.remove(0), user_id)
}

async fn block(store: &Store, board_id: &str, block_type: BlockType, user_id: &str) -> Block {
    let mut block = Block {
        id: new_id(),
        container_id: board_id.to_owned(),
        block_type,
        title: "환불 규정".to_owned(),
        ..Block::default()
    };
    store
        .insert_block(&mut block, user_id)
        .await
        .expect("the block is stored");
    block
}

async fn card(store: &Store, board_id: &str, user_id: &str) -> Block {
    block(store, board_id, BlockType::Card, user_id).await
}

/// The rows of the list, as the store's handle reaches them: what each is about.
async fn listed(store: &Store) -> Vec<(SearchTarget, String)> {
    let mut rows: Vec<(SearchTarget, String)> = changes(store)
        .await
        .into_iter()
        .map(|change| (change.target, change.id))
        .collect();
    rows.sort_by(|a, b| a.1.cmp(&b.1));
    rows
}

async fn changes(store: &Store) -> Vec<SearchChange> {
    store
        .due_search_changes(LATER, 1000)
        .await
        .expect("the list is read")
}

/// Takes every row off the list, as a worker that sent everything.
async fn settle_all(store: &Store) {
    for change in changes(store).await {
        assert!(
            store.settle_search_change(&change).await.expect("settled"),
            "{change:?}"
        );
    }
}

async fn patch(store: &Store, block_id: &str, patch: &BlockPatch, user_id: &str) {
    store
        .patch_block(block_id, patch, user_id)
        .await
        .expect("the block is patched");
}

#[tokio::test]
async fn a_store_that_keeps_no_list_notes_nothing() {
    let test = common::setup().await;
    let store = &test.store;
    let (board, user_id) = board(store).await;
    let card = card(store, &board.id, &user_id).await;
    store
        .delete_block(&card.id, &user_id)
        .await
        .expect("deleted");
    store
        .delete_container(&board.id, &user_id)
        .await
        .expect("deleted");

    assert_eq!(listed(store).await, []);
    assert_eq!(
        store.search_outbox_counts().await.expect("counted").waiting,
        0
    );
    test.teardown().await;
}

#[tokio::test]
async fn what_a_search_would_find_differently_is_noted_with_its_write() {
    let test = common::setup().await;
    let store = &test.store;
    store.keep_search_outbox();
    let (board, user_id) = board(store).await;

    let card = card(store, &board.id, &user_id).await;
    assert_eq!(
        listed(store).await,
        [(SearchTarget::Block, card.id.clone())],
        "a new card"
    );
    settle_all(store).await;

    // What a search does not read: a property, a view, a comment.
    let property = BlockPatch {
        updated_fields: json!({"icon": "x"}).as_object().expect("an object").clone(),
        ..BlockPatch::default()
    };
    patch(store, &card.id, &property, &user_id).await;
    block(store, &board.id, BlockType::View, &user_id).await;
    let mut comment = Block {
        id: new_id(),
        container_id: board.id.clone(),
        parent_id: card.id.clone(),
        block_type: BlockType::Comment,
        ..Block::default()
    };
    store
        .insert_block(&mut comment, &user_id)
        .await
        .expect("the comment is stored");
    assert_eq!(listed(store).await, [], "nothing a search reads changed");

    let title = BlockPatch {
        title: Some("새 제목".to_owned()),
        ..BlockPatch::default()
    };
    patch(store, &card.id, &title, &user_id).await;
    assert_eq!(
        listed(store).await,
        [(SearchTarget::Block, card.id.clone())]
    );
    settle_all(store).await;

    let read_at = store
        .get_block(&card.id)
        .await
        .expect("the card")
        .updated_at;
    let written = BlockPatch {
        body: BodyChange::Set(body("환불규정을 따른다")),
        read_at: Some(read_at),
        ..BlockPatch::default()
    };
    patch(store, &card.id, &written, &user_id).await;
    assert_eq!(
        listed(store).await,
        [(SearchTarget::Block, card.id.clone())]
    );
    settle_all(store).await;

    // A card that is deleted is searched no more, and that is noted.
    let other = self::card(store, &board.id, &user_id).await;
    settle_all(store).await;
    store
        .delete_block(&other.id, &user_id)
        .await
        .expect("deleted");
    assert_eq!(
        listed(store).await,
        [(SearchTarget::Block, other.id.clone())]
    );
    assert_eq!(
        store
            .search_documents(&[other.id.clone(), card.id.clone()])
            .await
            .expect("read")
            .into_iter()
            .map(|document| (document.id, document.title, document.text))
            .collect::<Vec<_>>(),
        [(
            card.id.clone(),
            "새 제목".to_owned(),
            "환불규정을 따른다".to_owned()
        )],
        "the live card has a document, and the deleted card has none"
    );
    test.teardown().await;
}

#[tokio::test]
async fn a_write_that_is_undone_notes_nothing() {
    let test = common::setup().await;
    let store = &test.store;
    store.keep_search_outbox();
    let (board, user_id) = board(store).await;
    let card = card(store, &board.id, &user_id).await;
    settle_all(store).await;

    // A patch made against an older card is refused, and its transaction undone.
    let stale = BlockPatch {
        title: Some("never".to_owned()),
        read_at: Some(1),
        ..BlockPatch::default()
    };
    store
        .patch_block(&card.id, &stale, &user_id)
        .await
        .expect_err("the card changed since");

    assert_eq!(listed(store).await, []);
    test.teardown().await;
}

/// Writes `text` to the card as a room writes what its editors typed: with no version
/// of its own. `seq` is the room's, one more each time.
async fn typed(store: &Store, card_id: &str, text: &str, seq: i64, user_id: &str) {
    if store.get_body_doc(card_id).await.expect("read").is_none() {
        store
            .create_body_doc(card_id, b"a state", b"a digest")
            .await
            .expect("a document is stored");
    }
    let read_at = store.get_block(card_id).await.expect("the card").updated_at;
    let as_read = crate::body_docs::as_read(store, card_id).await;
    store
        .project_body(
            card_id,
            fvoci_store::Projection::Written {
                body: Some(body(text)),
                read_at,
            },
            seq,
            b"a digest",
            user_id,
            as_read,
        )
        .await
        .expect("the room's body is written to the card");
}

#[tokio::test]
async fn keeping_a_version_of_a_body_notes_nothing() {
    let test = common::setup().await;
    let store = &test.store;
    store.keep_search_outbox();
    // One version kept: keeping the second forgets the first, and puts down again
    // what follows from the card.
    store.set_body_versions_kept(1);
    let (board, user_id) = board(store).await;
    let card = card(store, &board.id, &user_id).await;
    typed(store, &card.id, "환불규정을 따른다", 1, &user_id).await;
    let first = store.keep_body_version(&card.id).await.expect("kept");
    typed(store, &card.id, "환불규정을 바꾼다", 2, &user_id).await;
    settle_all(store).await;

    // A version is a copy of what the card holds: nothing a search finds changes.
    let second = store.keep_body_version(&card.id).await.expect("kept");
    let versions = store.body_versions(&card.id).await.expect("the versions");

    assert_eq!(
        (first, second),
        (Some(1), Some(2)),
        "two versions were kept"
    );
    assert_eq!(versions.len(), 1, "and the first forgotten: {versions:?}");
    assert_eq!(listed(store).await, [], "and nothing is noted for it");
    test.teardown().await;
}

#[tokio::test]
async fn the_trash_and_the_way_back_are_noted() {
    let test = common::setup().await;
    let store = &test.store;
    store.keep_search_outbox();
    let (board, user_id) = board(store).await;
    let card = card(store, &board.id, &user_id).await;
    let other = self::card(store, &board.id, &user_id).await;
    settle_all(store).await;

    store
        .delete_block(&card.id, &user_id)
        .await
        .expect("deleted");
    assert_eq!(
        listed(store).await,
        [(SearchTarget::Block, card.id.clone())]
    );
    assert_eq!(
        store
            .search_documents(std::slice::from_ref(&card.id))
            .await
            .expect("read"),
        [],
        "a card in the trash has no document"
    );
    settle_all(store).await;

    store
        .undelete_block(&card.id, &user_id)
        .await
        .expect("restored");
    assert_eq!(
        listed(store).await,
        [(SearchTarget::Block, card.id.clone())]
    );
    settle_all(store).await;

    store
        .delete_container(&board.id, &user_id)
        .await
        .expect("deleted");
    assert_eq!(
        listed(store).await,
        [(SearchTarget::Container, board.id.clone())]
    );
    assert_eq!(
        store
            .search_documents_of_container(&board.id, None, 10)
            .await
            .expect("read"),
        [],
        "a board in the trash has no documents"
    );
    settle_all(store).await;

    store
        .undelete_container(&board.id, &user_id)
        .await
        .expect("restored");
    let mut expected = vec![
        (SearchTarget::Block, card.id.clone()),
        (SearchTarget::Block, other.id.clone()),
    ];
    expected.sort_by(|a, b| a.1.cmp(&b.1));
    assert_eq!(listed(store).await, expected, "each card that came back");
    let first = store
        .search_documents_of_container(&board.id, None, 1)
        .await
        .expect("read");
    let second = store
        .search_documents_of_container(&board.id, Some(&first[0].id), 10)
        .await
        .expect("read");
    let mut paged = vec![first[0].id.clone(), second[0].id.clone()];
    paged.sort();
    let mut both = vec![card.id.clone(), other.id.clone()];
    both.sort();
    assert_eq!((paged, second.len()), (both, 1), "a page at a time");
    test.teardown().await;
}

#[tokio::test]
async fn a_template_and_what_is_built_in_have_no_documents() {
    let test = common::setup().await;
    let store = &test.store;
    let (board, user_id) = board(store).await;
    let mut template = Container {
        id: new_id(),
        workspace_id: board.workspace_id.clone(),
        visibility: ContainerVisibility::Open,
        is_template: true,
        ..Container::default()
    };
    store
        .insert_container(&mut template, &user_id)
        .await
        .expect("the template is stored");
    let on_template = card(store, &template.id, &user_id).await;
    let on_board = card(store, &board.id, &user_id).await;

    let documents = store
        .search_documents(&[
            on_template.id.clone(),
            on_board.id.clone(),
            "no identifier".to_owned(),
            new_id(),
        ])
        .await
        .expect("read");

    assert_eq!(documents.len(), 1, "{documents:?}");
    assert_eq!(documents[0].id, on_board.id);
    assert_eq!(documents[0].container_id, board.id);
    assert_eq!(documents[0].workspace_id, board.workspace_id);
    assert_eq!(Some(documents[0].org_id.clone()), store.tenant().org_id());
    test.teardown().await;
}

/// A card that is a template of its board is no document; a card that says it is none,
/// one without the field and a page of a wiki are.
#[tokio::test]
async fn a_card_template_of_a_board_has_no_document() {
    let test = common::setup().await;
    let store = &test.store;
    let (board, user_id) = board(store).await;
    let with_template_field = async |is_template: bool| {
        let mut block = Block {
            id: new_id(),
            container_id: board.id.clone(),
            block_type: BlockType::Card,
            title: "환불 규정".to_owned(),
            ..Block::default()
        };
        block
            .fields
            .insert("isTemplate".to_owned(), is_template.into());
        store
            .insert_block(&mut block, &user_id)
            .await
            .expect("the card is stored");
        block.id
    };
    let template = with_template_field(true).await;
    let not_a_template = with_template_field(false).await;
    let without_the_field = card(store, &board.id, &user_id).await.id;
    let mut wiki = Container {
        id: new_id(),
        workspace_id: board.workspace_id.clone(),
        visibility: ContainerVisibility::Open,
        kind: ContainerKind::Wiki,
        ..Container::default()
    };
    store
        .insert_container(&mut wiki, &user_id)
        .await
        .expect("the wiki is stored");
    let page = store
        .create_page(
            &wiki.id,
            &NewPage::default(),
            PageLimits::default(),
            &user_id,
        )
        .await
        .expect("the page is made")
        .page
        .id;

    let mut found: Vec<String> = store
        .search_documents(&[&template, &not_a_template, &without_the_field, &page])
        .await
        .expect("read")
        .into_iter()
        .map(|document| document.id)
        .collect();
    found.sort();
    let mut wanted = vec![not_a_template.clone(), without_the_field.clone(), page];
    wanted.sort();
    assert_eq!(found, wanted, "the template {template} is left out");

    let mut on_board: Vec<String> = store
        .search_documents_of_container(&board.id, None, 10)
        .await
        .expect("read")
        .into_iter()
        .map(|document| document.id)
        .collect();
    on_board.sort();
    let mut wanted = vec![not_a_template, without_the_field];
    wanted.sort();
    assert_eq!(
        on_board, wanted,
        "a board sent whole leaves its template out"
    );
    test.teardown().await;
}

/// A card that becomes a template of its board is taken out of the index, and one that
/// stops being one is put back: what a search would find of it changed.
#[tokio::test]
async fn a_card_that_becomes_a_template_or_stops_being_one_is_noted() {
    let test = common::setup().await;
    let store = &test.store;
    store.keep_search_outbox();
    let (board, user_id) = board(store).await;
    let card = card(store, &board.id, &user_id).await;
    settle_all(store).await;
    let flag = |value: serde_json::Value| BlockPatch {
        updated_fields: json!({"isTemplate": value})
            .as_object()
            .expect("an object")
            .clone(),
        ..BlockPatch::default()
    };

    patch(store, &card.id, &flag(json!(true)), &user_id).await;
    assert_eq!(
        listed(store).await,
        [(SearchTarget::Block, card.id.clone())],
        "a card became a template"
    );
    assert_eq!(
        store
            .search_documents(std::slice::from_ref(&card.id))
            .await
            .expect("read"),
        [],
        "a template has no document"
    );
    settle_all(store).await;
    patch(store, &card.id, &flag(json!(true)), &user_id).await;
    assert_eq!(listed(store).await, [], "a template stays one");

    let unflag = BlockPatch {
        deleted_fields: vec!["isTemplate".to_owned()],
        ..BlockPatch::default()
    };
    patch(store, &card.id, &unflag, &user_id).await;
    assert_eq!(
        listed(store).await,
        [(SearchTarget::Block, card.id.clone())],
        "a template became a card"
    );
    settle_all(store).await;
    patch(store, &card.id, &flag(json!(true)), &user_id).await;
    settle_all(store).await;
    patch(store, &card.id, &flag(json!(false)), &user_id).await;
    assert_eq!(
        listed(store).await,
        [(SearchTarget::Block, card.id.clone())],
        "a template that says it is none became a card"
    );
    test.teardown().await;
}

#[tokio::test]
async fn a_row_noted_again_while_it_was_sent_stays() {
    let test = common::setup().await;
    let store = &test.store;
    store.keep_search_outbox();
    let (board, user_id) = board(store).await;
    let card = card(store, &board.id, &user_id).await;
    let read = changes(store).await.remove(0);

    // The card changes while what was read of it is on its way.
    let title = BlockPatch {
        title: Some("그 사이".to_owned()),
        ..BlockPatch::default()
    };
    patch(store, &card.id, &title, &user_id).await;
    let taken_off = store.settle_search_change(&read).await.expect("asked");
    let again = changes(store).await.remove(0);

    assert!(!taken_off, "what was sent may be behind the card");
    assert!(again.seq > read.seq, "{again:?} after {read:?}");
    assert!(store.settle_search_change(&again).await.expect("asked"));
    assert_eq!(listed(store).await, []);
    test.teardown().await;
}

#[tokio::test]
async fn a_row_that_could_not_be_sent_waits_and_is_due_at_once_when_noted_again() {
    let test = common::setup().await;
    let store = &test.store;
    store.keep_search_outbox();
    let (board, user_id) = board(store).await;
    let card = card(store, &board.id, &user_id).await;
    let read = changes(store).await.remove(0);
    let now = read.seq;

    store
        .search_change_failed(&read, now + 60_000)
        .await
        .expect("noted");
    let due_now = store
        .due_search_changes(now + 1000, 10)
        .await
        .expect("read");
    let waiting = changes(store).await.remove(0);
    let counts = store.search_outbox_counts().await.expect("counted");
    let title = BlockPatch {
        title: Some("다시".to_owned()),
        ..BlockPatch::default()
    };
    patch(store, &card.id, &title, &user_id).await;
    let noted_again = store
        .due_search_changes(now + 30_000, 10)
        .await
        .expect("read");

    assert_eq!(due_now, [], "not before its time");
    assert_eq!(waiting.attempts, 1);
    assert_eq!((counts.waiting, counts.failing), (1, 1));
    assert_eq!(noted_again.len(), 1, "a change is sent at once");
    assert_eq!(noted_again[0].attempts, 0);
    test.teardown().await;
}

#[tokio::test]
async fn every_container_is_listed_for_an_index_and_the_index_remembered() {
    let test = common::setup().await;
    let store = &test.store;
    store.keep_search_outbox();
    let (board, user_id) = board(store).await;
    let mut template = Container {
        id: new_id(),
        workspace_id: board.workspace_id.clone(),
        visibility: ContainerVisibility::Open,
        is_template: true,
        ..Container::default()
    };
    store
        .insert_container(&mut template, &user_id)
        .await
        .expect("the template is stored");
    let mut all = vec![board.id.clone()];
    let system = test.system();
    // On PostgreSQL another organization's boards are listed by the same statement.
    if common::db_type() == DbType::Postgres {
        let (other_org, _) = common::organization(&test.scoped(Tenant::Unset), "the other").await;
        let theirs = test.for_org(&other_org).expect("an identifier");
        let (their_board, _) = self::board(&theirs).await;
        all.push(their_board.id);
    }
    all.sort();
    let mark = SearchIndexMark {
        name: "fvoci".to_owned(),
        uuid: "an-index".to_owned(),
        version: 1,
    };

    assert_eq!(system.search_index_mark().await.expect("read"), None);
    let count = system
        .list_every_container_for_search(&mark)
        .await
        .expect("listed");
    let rows = listed(&system).await;
    let ours = listed(store).await;
    let remembered = system.search_index_mark().await.expect("read");
    // Listed again, as for another index: the same rows, each moved on.
    let before = changes(&system).await;
    system
        .list_every_container_for_search(&SearchIndexMark {
            uuid: "another".to_owned(),
            ..mark.clone()
        })
        .await
        .expect("listed");
    let after = changes(&system).await;
    system.drop_search_outbox().await.expect("dropped");
    card(store, &board.id, &user_id).await;

    assert_eq!(usize::try_from(count).expect("a count"), all.len());
    assert_eq!(
        rows,
        all.iter()
            .map(|id| (SearchTarget::Container, id.clone()))
            .collect::<Vec<_>>(),
        "every board that is no template, of every organization"
    );
    assert_eq!(
        ours,
        [(SearchTarget::Container, board.id.clone())],
        "ours alone"
    );
    assert_eq!(remembered, Some(mark));
    assert_eq!(before.len(), after.len());
    for (before, after) in before.iter().zip(&after) {
        assert_eq!(before.id, after.id);
        assert!(after.seq > before.seq, "{after:?} after {before:?}");
    }
    assert_eq!(listed(&system).await, [], "dropped, and kept no more");
    assert_eq!(system.search_index_mark().await.expect("read"), None);
    test.teardown().await;
}

/// The other ways a card's title or body is written: several patches at once, a copy,
/// the bodies an import writes after its blocks, and the body a room of editors
/// writes to its card.
#[tokio::test]
async fn every_way_a_card_is_written_notes_it() {
    let test = common::setup().await;
    let store = &test.store;
    store.keep_search_outbox();
    let (board, user_id) = board(store).await;
    let first = card(store, &board.id, &user_id).await;
    let second = card(store, &board.id, &user_id).await;
    let mut both = vec![
        (SearchTarget::Block, first.id.clone()),
        (SearchTarget::Block, second.id.clone()),
    ];
    both.sort_by(|a, b| a.1.cmp(&b.1));
    settle_all(store).await;

    let retitled = |title: &str| BlockPatch {
        title: Some(title.to_owned()),
        ..BlockPatch::default()
    };
    store
        .patch_blocks(
            &fvoci_model::BlockPatchBatch {
                block_ids: vec![first.id.clone(), second.id.clone()],
                block_patches: vec![retitled("하나"), retitled("둘")],
            },
            &user_id,
        )
        .await
        .expect("patched together");
    assert_eq!(listed(store).await, both, "patched together");
    settle_all(store).await;

    store
        .duplicate_block(
            &board.id,
            &first.id,
            &user_id,
            fvoci_model::DuplicateAs::Copy,
        )
        .await
        .expect("copied");
    let copies = listed(store).await;
    assert_eq!(
        copies.len(),
        1,
        "the copy, and not what it was made of: {copies:?}"
    );
    assert_ne!(copies[0].1, first.id);
    settle_all(store).await;

    store
        .replace_bodies_as_written(
            &board.id,
            vec![(first.id.clone(), Some(body("가져온 본문")))],
        )
        .await
        .expect("the body is written");
    assert_eq!(
        listed(store).await,
        [(SearchTarget::Block, first.id.clone())]
    );
    settle_all(store).await;

    store
        .create_body_doc(&second.id, b"a state", b"a digest")
        .await
        .expect("a document is stored");
    let read_at = store
        .get_block(&second.id)
        .await
        .expect("the card")
        .updated_at;
    let as_read = crate::body_docs::as_read(store, &second.id).await;
    store
        .project_body(
            &second.id,
            fvoci_store::Projection::Written {
                body: Some(body("함께 쓴 본문")),
                read_at,
            },
            1,
            b"another digest",
            &user_id,
            as_read,
        )
        .await
        .expect("the room's body is written to the card");
    assert_eq!(
        listed(store).await,
        [(SearchTarget::Block, second.id.clone())]
    );
    settle_all(store).await;
    // Written again as it is, for the files it shows alone: nothing a search reads.
    let as_read = crate::body_docs::as_read(store, &second.id).await;
    store
        .project_body(
            &second.id,
            fvoci_store::Projection::Held,
            2,
            b"another digest",
            &user_id,
            as_read,
        )
        .await
        .expect("the document is marked as written");
    assert_eq!(listed(store).await, []);
    test.teardown().await;
}
