//! The blocks a body names (`body_links`): every way of writing a body puts them down
//! again, they outlive the trash, and they go with what is gone for good.

use fvoci_model::{
    Block, BlockPatch, BlockType, Body, BodyChange, Container, ContainerKind, ContainerVisibility,
    NewPage, PageLimits, new_id,
};
use fvoci_store::Store;
use serde_json::json;

use crate::boards::new_board;
use crate::body_versions::type_body;
use crate::common;

/// What a block that names nothing, or that nothing names, has in the table.
const NO_ROWS: [&str; 0] = [];

/// A body that says `text` and names the blocks, each in a paragraph of its own.
pub(crate) fn body_naming(text: &str, targets: &[&str]) -> Option<Body> {
    let mut content =
        vec![json!({"type": "paragraph", "content": [{"type": "text", "text": text}]})];
    content.extend(targets.iter().map(|target| {
        json!({"type": "paragraph", "content": [
            {"type": "reference", "attrs": {"id": target}},
        ]})
    }));
    Body::parse(json!({"type": "doc", "content": content})).expect("the body may be stored")
}

/// A board of `kind` of the workspace, with `user_id` as its administrator.
async fn container(
    store: &Store,
    workspace_id: &str,
    user_id: &str,
    kind: ContainerKind,
) -> Container {
    let mut board = Container {
        kind,
        ..new_board(&new_id(), workspace_id, ContainerVisibility::Private)
    };
    store
        .insert_container_with_admin(&mut board, user_id)
        .await
        .expect("the board is stored");
    board
}

/// A block of the board below `parent_id`, with the body, made by the user.
async fn block_with(
    store: &Store,
    board: &Container,
    parent_id: &str,
    block_type: BlockType,
    body: Option<Body>,
) -> String {
    let mut block = Block {
        id: new_id(),
        container_id: board.id.clone(),
        parent_id: parent_id.to_owned(),
        block_type,
        body,
        ..Block::default()
    };
    common::insert_blocks(store, std::slice::from_mut(&mut block), &board.created_by).await;
    block.id
}

/// A card of the board with the body.
async fn card_with(store: &Store, board: &Container, body: Option<Body>) -> String {
    block_with(store, board, "", BlockType::Card, body).await
}

/// Writes the block's body whole.
async fn write_body(store: &Store, block_id: &str, body: Option<Body>, user_id: &str) {
    let patch = BlockPatch {
        body: body.map_or(BodyChange::Clear, BodyChange::Set),
        ..BlockPatch::default()
    };
    store
        .patch_block(block_id, &patch, user_id)
        .await
        .expect("the body is written");
}

/// The rows of the table whose `column` is the block: `source>target@board`, sorted.
async fn rows(store: &Store, column: &str, block_id: &str) -> Vec<String> {
    let mut rows = store
        .texts_unchecked(&format!(
            "SELECT CAST(source_id AS TEXT) || '>' || CAST(target_id AS TEXT) || '@' || \
             CAST(container_id AS TEXT) FROM body_links WHERE CAST({column} AS TEXT) = '{block_id}'"
        ))
        .await
        .expect("the rows are read");
    rows.sort_unstable();
    rows
}

/// What the rows of a source that names the targets from the board are.
fn naming(source: &str, targets: &[&str], board: &Container) -> Vec<String> {
    let mut rows: Vec<String> = targets
        .iter()
        .map(|target| format!("{source}>{target}@{}", board.id))
        .collect();
    rows.sort_unstable();
    rows
}

/// A card made with a body, a body written whole, and a body taken away each put down
/// what the body names, and what it names no more goes. What is named need not be
/// there: a block of nowhere is written as any other, and nothing is refused.
#[tokio::test]
async fn a_body_written_whole_puts_down_what_it_names() {
    let test = common::setup().await;
    let store = &test.store;
    let user = common::user(store, "pam.beesly").await;
    let workspace_id = common::workspace_of(store, &[&user]).await;
    let board = container(store, &workspace_id, &user, ContainerKind::Board).await;
    let first = card_with(store, &board, None).await;
    let second = card_with(store, &board, None).await;
    let nowhere = new_id();

    let source = card_with(store, &board, body_naming("made", &[&first, &nowhere])).await;
    assert_eq!(
        rows(store, "source_id", &source).await,
        naming(&source, &[&first, &nowhere], &board),
        "a card made with a body"
    );

    // Named twice, a block has one row; the one named no more loses its.
    write_body(
        store,
        &source,
        body_naming("changed", &[&second, &first, &second]),
        &user,
    )
    .await;
    assert_eq!(
        rows(store, "source_id", &source).await,
        naming(&source, &[&first, &second], &board),
        "a body written whole"
    );
    assert_eq!(rows(store, "target_id", &nowhere).await, NO_ROWS);

    // A change of something else leaves the rows.
    let renamed = BlockPatch {
        title: Some("named".to_owned()),
        ..BlockPatch::default()
    };
    store
        .patch_block(&source, &renamed, &user)
        .await
        .expect("renamed");
    assert_eq!(
        rows(store, "source_id", &source).await,
        naming(&source, &[&first, &second], &board)
    );

    write_body(store, &source, None, &user).await;
    assert_eq!(
        rows(store, "source_id", &source).await,
        NO_ROWS,
        "no body names nothing"
    );
    assert_eq!(store.count_rows("body_links").await.expect("counted"), 0);
    test.teardown().await;
}

/// What editors typed in a block's document is written to the block and counted as any
/// body is, and so is a body an import wrote after its block.
#[tokio::test]
async fn a_typed_body_and_a_body_written_after_its_block_put_down_what_they_name() {
    let test = common::setup().await;
    let store = &test.store;
    let user = common::user(store, "jim.halpert").await;
    let workspace_id = common::workspace_of(store, &[&user]).await;
    let board = container(store, &workspace_id, &user, ContainerKind::Board).await;
    let target = card_with(store, &board, None).await;
    let other = card_with(store, &board, None).await;

    let typed = card_with(store, &board, body_naming("before", &[&other])).await;
    type_body(store, &typed, body_naming("typed", &[&target]), &user).await;
    assert_eq!(
        rows(store, "source_id", &typed).await,
        naming(&typed, &[&target], &board),
        "the body of a document, written to its block"
    );
    // The block holds the body already: written again as it is, the rows stay.
    let digest = fvoci_store::body_digest(body_naming("typed", &[&target]).as_ref());
    let as_read = crate::body_docs::as_read(store, &typed).await;
    store
        .project_body(
            &typed,
            fvoci_store::Projection::Held,
            1,
            &digest,
            &user,
            as_read,
        )
        .await
        .expect("marked as written");
    assert_eq!(
        rows(store, "source_id", &typed).await,
        naming(&typed, &[&target], &board)
    );

    let imported = card_with(store, &board, body_naming("stored", &[&other])).await;
    store
        .replace_bodies_as_written(
            &board.id,
            vec![(imported.clone(), body_naming("pointed", &[&target, &typed]))],
        )
        .await
        .expect("the bodies are written");
    assert_eq!(
        rows(store, "source_id", &imported).await,
        naming(&imported, &[&target, &typed], &board),
        "a body written after its block"
    );
    assert_eq!(
        rows(store, "target_id", &other).await,
        NO_ROWS,
        "nothing names the other card any more"
    );
    test.teardown().await;
}

/// A page names a card and a card a page, and a comment names both: the row holds the
/// board or the wiki of the block that names. The versions kept of a body name
/// nothing: the rows are of the body the block holds.
#[tokio::test]
async fn cards_pages_and_comments_name_each_other_across_a_board_and_a_wiki() {
    let test = common::setup().await;
    let store = &test.store;
    let user = common::user(store, "oscar.martinez").await;
    let workspace_id = common::workspace_of(store, &[&user]).await;
    let board = container(store, &workspace_id, &user, ContainerKind::Board).await;
    let wiki = container(store, &workspace_id, &user, ContainerKind::Wiki).await;
    let card = card_with(store, &board, None).await;
    let page = store
        .create_page(&wiki.id, &NewPage::default(), PageLimits::default(), &user)
        .await
        .expect("the page is made")
        .first
        .id;

    write_body(store, &card, body_naming("see the page", &[&page]), &user).await;
    write_body(store, &page, body_naming("see the card", &[&card]), &user).await;
    let comment = block_with(
        store,
        &board,
        &card,
        BlockType::Comment,
        body_naming("both", &[&card, &page]),
    )
    .await;
    assert_eq!(
        rows(store, "source_id", &card).await,
        naming(&card, &[&page], &board)
    );
    assert_eq!(
        rows(store, "source_id", &page).await,
        naming(&page, &[&card], &wiki)
    );
    assert_eq!(
        rows(store, "source_id", &comment).await,
        naming(&comment, &[&card, &page], &board)
    );

    // The body before is kept as a version, and names nothing in the table.
    write_body(store, &card, body_naming("see nothing", &[]), &user).await;
    assert!(
        !store
            .body_versions(&card)
            .await
            .expect("the versions")
            .is_empty(),
        "the body that named the page is a version"
    );
    assert_eq!(rows(store, "source_id", &card).await, NO_ROWS);
    assert_eq!(
        rows(store, "target_id", &page).await,
        naming(&comment, &[&page], &board),
        "the comment alone"
    );
    test.teardown().await;
}

/// The rows stay while the block that names, or the block named, is in the trash, and
/// are there when it comes back; they go when either is gone for good, and the rows of
/// the blocks that stay are left.
#[tokio::test]
async fn the_rows_outlive_the_trash_and_go_with_what_is_gone_for_good() {
    let test = common::setup().await;
    let store = &test.store;
    let user = common::user(store, "angela.martin").await;
    let workspace_id = common::workspace_of(store, &[&user]).await;
    let board = container(store, &workspace_id, &user, ContainerKind::Board).await;
    let target = card_with(store, &board, None).await;
    let kept_target = card_with(store, &board, None).await;
    let source = card_with(
        store,
        &board,
        body_naming("names", &[&target, &kept_target]),
    )
    .await;
    let doomed_source = card_with(store, &board, body_naming("names", &[&kept_target])).await;
    let kept_source = card_with(store, &board, body_naming("names", &[&kept_target])).await;

    // In the trash and back again: the rows were kept, and are put down as they were.
    for block_id in [&source, &target] {
        store.delete_block(block_id, &user).await.expect("deleted");
        assert_eq!(
            rows(store, "source_id", &source).await,
            naming(&source, &[&target, &kept_target], &board),
            "kept while {block_id} is in the trash"
        );
        store
            .undelete_block(block_id, &user)
            .await
            .expect("restored");
        assert_eq!(
            rows(store, "source_id", &source).await,
            naming(&source, &[&target, &kept_target], &board),
            "there when {block_id} is back"
        );
    }

    // Gone for good: the block that named takes its rows, the block named the rows
    // that named it.
    for block_id in [&doomed_source, &target] {
        store.delete_block(block_id, &user).await.expect("deleted");
    }
    store.purge_trash(i64::MAX).await.expect("purged");
    assert_eq!(rows(store, "source_id", &doomed_source).await, NO_ROWS);
    assert_eq!(rows(store, "target_id", &target).await, NO_ROWS);
    assert_eq!(
        rows(store, "source_id", &source).await,
        naming(&source, &[&kept_target], &board),
        "what the card that stays still names"
    );
    assert_eq!(
        rows(store, "source_id", &kept_source).await,
        naming(&kept_source, &[&kept_target], &board)
    );

    // A board gone for good takes the rows of its blocks, whichever side they are on.
    let other = container(store, &workspace_id, &user, ContainerKind::Board).await;
    let from_other = card_with(store, &other, body_naming("names", &[&kept_target])).await;
    write_body(
        store,
        &kept_source,
        body_naming("names", &[&from_other]),
        &user,
    )
    .await;
    store
        .delete_container(&other.id, &user)
        .await
        .expect("deleted");
    assert_eq!(
        rows(store, "source_id", &from_other).await,
        naming(&from_other, &[&kept_target], &other),
        "kept while the board is in the trash"
    );
    store.purge_trash(i64::MAX).await.expect("purged");
    assert_eq!(rows(store, "source_id", &from_other).await, NO_ROWS);
    assert_eq!(rows(store, "target_id", &from_other).await, NO_ROWS);
    assert_eq!(
        rows(store, "source_id", &source).await,
        naming(&source, &[&kept_target], &board)
    );

    // What is gone for good is told from what never was by nothing: a row that names
    // no block goes with a purge, whichever it is, and is put down again when the body
    // that names it is written. No reader is shown such a row.
    let nowhere = new_id();
    write_body(
        store,
        &source,
        body_naming("names", &[&kept_target, &nowhere]),
        &user,
    )
    .await;
    assert_eq!(rows(store, "target_id", &nowhere).await.len(), 1);
    store.purge_trash(i64::MAX).await.expect("purged");
    assert_eq!(rows(store, "target_id", &nowhere).await, NO_ROWS);
    let renamed = BlockPatch {
        title: Some("named".to_owned()),
        ..BlockPatch::default()
    };
    store
        .patch_block(&source, &renamed, &user)
        .await
        .expect("renamed");
    assert_eq!(
        rows(store, "source_id", &source).await,
        naming(&source, &[&kept_target, &nowhere], &board),
        "what the body names, again"
    );
    test.teardown().await;
}

/// The titles are looked through some at a time, the block changed last first: a title
/// far down is found, as many as asked for are, and of the boards asked about only.
#[tokio::test]
async fn a_title_is_found_past_the_blocks_read_at_once() {
    let test = common::setup().await;
    let store = &test.store;
    let user = common::user(store, "kevin.malone").await;
    let workspace_id = common::workspace_of(store, &[&user]).await;
    let board = container(store, &workspace_id, &user, ContainerKind::Board).await;
    let other = container(store, &workspace_id, &user, ContainerKind::Board).await;
    let titled = |board: &Container, title: String| Block {
        id: new_id(),
        container_id: board.id.clone(),
        block_type: BlockType::Card,
        title,
        ..Block::default()
    };
    // The oldest is what is looked for; six hundred changed after it are not.
    let mut needle = titled(&board, "The Needle".to_owned());
    common::insert_blocks(store, std::slice::from_mut(&mut needle), &user).await;
    let mut elsewhere = titled(&other, "a needle elsewhere".to_owned());
    common::insert_blocks(store, std::slice::from_mut(&mut elsewhere), &user).await;
    let mut hay: Vec<Block> = (0..600)
        .map(|number| titled(&board, format!("hay {number}")))
        .collect();
    common::insert_blocks(store, &mut hay, &user).await;

    let found = store
        .titled_blocks(&[&board.id], "needle the", 20)
        .await
        .expect("searched");
    assert_eq!(
        found.iter().map(|block| &block.id).collect::<Vec<_>>(),
        [&needle.id]
    );
    assert!(found[0].body.is_none() && found[0].container_id == board.id);
    let both = store
        .titled_blocks(&[&board.id, &other.id], "NEEDLE", 20)
        .await
        .expect("searched");
    assert_eq!(both.len(), 2);
    let some = store
        .titled_blocks(&[&board.id], "hay", 7)
        .await
        .expect("searched");
    assert_eq!(some.len(), 7);
    let every = store
        .titled_blocks(&[&board.id], "", 1000)
        .await
        .expect("searched");
    assert_eq!(every.len(), 601);
    // Each once, though they are read in several goes and many were changed at once.
    let distinct: std::collections::HashSet<&str> =
        every.iter().map(|block| block.id.as_str()).collect();
    assert_eq!(distinct.len(), 601);
    assert!(
        every
            .windows(2)
            .all(|pair| pair[0].updated_at >= pair[1].updated_at),
        "the one changed last first"
    );
    for nowhere in [&[][..], &[new_id().as_str()][..]] {
        let none = store
            .titled_blocks(nowhere, "", 20)
            .await
            .expect("searched");
        assert_eq!(none.len(), 0, "{nowhere:?}");
    }
    test.teardown().await;
}

/// Of many identifiers, those that name a card of the board are told: not a card of
/// another board, a block that is no card, one in the trash, or nothing.
#[tokio::test]
async fn the_cards_of_a_board_are_told_among_many_identifiers() {
    let test = common::setup().await;
    let store = &test.store;
    let user = common::user(store, "oscar.martinez").await;
    let workspace_id = common::workspace_of(store, &[&user]).await;
    let board = container(store, &workspace_id, &user, ContainerKind::Board).await;
    let other = container(store, &workspace_id, &user, ContainerKind::Board).await;
    let block = |board: &Container, block_type: BlockType| Block {
        id: new_id(),
        container_id: board.id.clone(),
        block_type,
        ..Block::default()
    };
    let mut cards: Vec<Block> = (0..600).map(|_| block(&board, BlockType::Card)).collect();
    // A template of the board's cards is a card of the board.
    cards[0]
        .fields
        .insert("isTemplate".to_owned(), serde_json::Value::Bool(true));
    common::insert_blocks(store, &mut cards, &user).await;
    let mut others = vec![
        block(&other, BlockType::Card),
        block(&board, BlockType::View),
        block(&board, BlockType::Card),
    ];
    common::insert_blocks(store, &mut others, &user).await;
    store
        .delete_block(&others[2].id, &user)
        .await
        .expect("the card goes to the trash");

    let unknown = new_id();
    let mut asked: Vec<&str> = others.iter().map(|block| block.id.as_str()).collect();
    asked.push(&unknown);
    asked.extend(cards.iter().map(|card| card.id.as_str()));
    let told = store
        .blocks_on_container_among(&board.id, BlockType::Card, &asked)
        .await
        .expect("asked");
    assert_eq!(told.len(), 600);
    assert!(cards.iter().all(|card| told.contains(&card.id)));
    let none = store
        .blocks_on_container_among(&board.id, BlockType::Card, &[])
        .await
        .expect("asked");
    assert_eq!(none.len(), 0);
    test.teardown().await;
}

/// Of a wiki, the pages on it are told: not a page of another wiki, one in the trash,
/// or a card.
#[tokio::test]
async fn the_pages_of_a_wiki_are_told_among_identifiers() {
    let test = common::setup().await;
    let store = &test.store;
    let user = common::user(store, "kelly.kapoor").await;
    let workspace_id = common::workspace_of(store, &[&user]).await;
    let wiki = container(store, &workspace_id, &user, ContainerKind::Wiki).await;
    let other = container(store, &workspace_id, &user, ContainerKind::Wiki).await;
    let board = container(store, &workspace_id, &user, ContainerKind::Board).await;
    let block = |board: &Container, block_type: BlockType| Block {
        id: new_id(),
        container_id: board.id.clone(),
        block_type,
        ..Block::default()
    };
    let mut blocks = vec![
        block(&wiki, BlockType::Page),
        block(&wiki, BlockType::Page),
        block(&other, BlockType::Page),
        block(&board, BlockType::Card),
    ];
    common::insert_blocks(store, &mut blocks, &user).await;
    store
        .delete_block(&blocks[1].id, &user)
        .await
        .expect("the page goes to the trash");
    let asked: Vec<&str> = blocks.iter().map(|block| block.id.as_str()).collect();
    let told = store
        .blocks_on_container_among(&wiki.id, BlockType::Page, &asked)
        .await
        .expect("asked");
    assert_eq!(
        told,
        std::collections::HashSet::from([blocks[0].id.clone()])
    );
    test.teardown().await;
}
