//! The pages of a wiki: what keeps a page on a wiki and below a page, where a new page
//! goes among the pages of its parent, and the tree.

use fvoci_model::{
    Block, BlockError, BlockPatch, BlockType, Container, ContainerKind, ContainerKinds,
    ContainerVisibility, NewPage, PageLimits, PageNode, new_id,
};
use fvoci_store::{DbType, Store, StoreError, Tenant};

use crate::boards::new_board;
use crate::common;

/// A board of `kind` with `user_id` as its administrator.
pub(crate) async fn stored(
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

fn block(board_id: &str, parent_id: &str, block_type: BlockType) -> Block {
    Block {
        id: new_id(),
        container_id: board_id.to_owned(),
        parent_id: parent_id.to_owned(),
        block_type,
        ..Block::default()
    }
}

pub(crate) fn refusal(result: Result<impl std::fmt::Debug, StoreError>) -> BlockError {
    match result {
        Err(StoreError::InvalidBlock { source, .. }) => source,
        other => panic!("the block is refused: {other:?}"),
    }
}

/// A page made below `parent_id` at the place `after` asks for; its identifier.
pub(crate) async fn page(
    store: &Store,
    wiki: &Container,
    parent_id: &str,
    after: Option<&str>,
) -> String {
    let new = NewPage {
        parent_id: parent_id.to_owned(),
        after: after.map(str::to_owned),
        ..NewPage::default()
    };
    store
        .create_page(&wiki.id, &new, PageLimits::default(), &wiki.created_by)
        .await
        .expect("the page is made")
        .page
        .id
}

/// The pages of the wiki below `parent_id`, as the tree has them.
pub(crate) async fn below(store: &Store, wiki: &Container, parent_id: &str) -> Vec<String> {
    let tree = store.get_page_tree(&wiki.id).await.expect("the tree");
    tree.into_iter()
        .filter(|page| page.parent_id == parent_id)
        .map(|page| page.id)
        .collect()
}

#[tokio::test]
async fn a_page_is_a_block_of_a_wiki_and_a_wiki_has_no_other() {
    let test = common::setup().await;
    let user_id = common::user(&test, "user").await;
    let workspace_id = common::workspace_of(&test, &[&user_id]).await;
    let board = stored(&test, &workspace_id, &user_id, ContainerKind::Board).await;
    let wiki = stored(&test, &workspace_id, &user_id, ContainerKind::Wiki).await;
    let mut cards = [block(&board.id, "", BlockType::Card)];
    common::insert_blocks(&test, &mut cards, &user_id).await;
    let [card] = cards;
    let on_wiki = page(&test, &wiki, "", None).await;

    // A page on a board: alone, among others, below a card, and by the making of pages.
    for parent in ["", card.id.as_str()] {
        let mut stray = block(&board.id, parent, BlockType::Page);
        assert_eq!(
            refusal(test.insert_block(&mut stray, &user_id).await),
            BlockError::PageOffWiki,
            "below {parent:?}"
        );
        let mut among = [block(&board.id, "", BlockType::View), stray];
        assert_eq!(
            refusal(test.insert_blocks(&mut among, &user_id).await),
            BlockError::PageOffWiki
        );
    }
    assert_eq!(
        refusal(
            test.create_page(
                &board.id,
                &NewPage::default(),
                PageLimits::default(),
                &user_id
            )
            .await
        ),
        BlockError::PageOffWiki
    );
    // What a board has, on a wiki: at its top and below a page. A comment is on a page
    // (`a_comment_of_a_wiki_is_on_a_page_and_goes_to_the_trash_with_it`).
    for block_type in [BlockType::Card, BlockType::View, BlockType::Attachment] {
        for parent in ["", on_wiki.as_str()] {
            let mut stray = block(&wiki.id, parent, block_type);
            assert_eq!(
                refusal(test.insert_block(&mut stray, &user_id).await),
                BlockError::NotPageOnWiki,
                "{block_type} below {parent:?}"
            );
        }
    }
    // Nothing of what was refused is there.
    let blocks_of = async |board: &Container| {
        let filter = fvoci_store::BlockFilter::container(&board.id);
        let blocks = test.get_blocks(&filter).await.expect("the blocks");
        let mut found: Vec<(String, BlockType)> = blocks
            .into_iter()
            .map(|block| (block.id, block.block_type))
            .collect();
        found.sort_by(|one, other| one.0.cmp(&other.0));
        found
    };
    assert_eq!(
        blocks_of(&board).await,
        [(card.id.clone(), BlockType::Card)]
    );
    assert_eq!(blocks_of(&wiki).await, [(on_wiki.clone(), BlockType::Page)]);
    // A handle of boards finds no page, and one of wikis no card.
    for (id, kinds, found) in [
        (&on_wiki, ContainerKinds::WIKIS, true),
        (&on_wiki, ContainerKinds::BOARDS, false),
        (&card.id, ContainerKinds::WIKIS, false),
    ] {
        let read = test.get_block_within(id, Some(&workspace_id), kinds).await;
        assert_eq!(read.is_ok(), found, "{kinds:?}: {read:?}");
    }
    test.teardown().await;
}

#[tokio::test]
async fn the_parent_of_a_page_is_a_page_of_its_wiki() {
    let test = common::setup().await;
    let user_id = common::user(&test, "user").await;
    let workspace_id = common::workspace_of(&test, &[&user_id]).await;
    let wiki = stored(&test, &workspace_id, &user_id, ContainerKind::Wiki).await;
    let other = stored(&test, &workspace_id, &user_id, ContainerKind::Wiki).await;
    let top = page(&test, &wiki, "", None).await;
    let elsewhere = page(&test, &other, "", None).await;

    // Below a page of the wiki, as deep as it goes.
    let child = page(&test, &wiki, &top, None).await;
    let grandchild = page(&test, &wiki, &child, None).await;
    assert_eq!(below(&test, &wiki, "").await, std::slice::from_ref(&top));
    assert_eq!(
        below(&test, &wiki, &top).await,
        std::slice::from_ref(&child)
    );
    assert_eq!(below(&test, &wiki, &child).await, [grandchild]);

    // A page of another wiki, the other wiki, the wiki itself and nothing are no parent.
    for parent in [
        elsewhere.as_str(),
        other.id.as_str(),
        wiki.id.as_str(),
        &new_id(),
    ] {
        let new = NewPage {
            parent_id: parent.to_owned(),
            ..NewPage::default()
        };
        assert_eq!(
            refusal(
                test.create_page(&wiki.id, &new, PageLimits::default(), &user_id)
                    .await
            ),
            BlockError::ParentNotOnContainer,
            "below {parent}"
        );
        let mut moved = test.get_block(&child).await.expect("the page");
        parent.clone_into(&mut moved.parent_id);
        assert_eq!(
            refusal(test.insert_block(&mut moved, &user_id).await),
            BlockError::ParentNotOnContainer,
            "moved below {parent}"
        );
    }
    // Of a page that is wrong in what it is below and in what it goes after, and of
    // one whose title is wrong besides, the first of these is what is refused.
    let mut new = NewPage {
        parent_id: elsewhere.clone(),
        after: Some(new_id()),
        ..NewPage::default()
    };
    assert_eq!(
        refusal(
            test.create_page(&wiki.id, &new, PageLimits::default(), &user_id)
                .await
        ),
        BlockError::ParentNotOnContainer
    );
    new.title = "a\0b".to_owned();
    assert_eq!(
        refusal(
            test.create_page(&wiki.id, &new, PageLimits::default(), &user_id)
                .await
        ),
        BlockError::NulInTitle
    );
    assert_eq!(below(&test, &wiki, &top).await, [child]);
    assert_eq!(below(&test, &other, "").await, [elsewhere]);
    // A wiki that is not there has no pages to make.
    let nowhere = test
        .create_page(
            &new_id(),
            &NewPage::default(),
            PageLimits::default(),
            &user_id,
        )
        .await;
    assert!(
        matches!(&nowhere, Err(error) if error.is_not_found()),
        "{nowhere:?}"
    );
    test.teardown().await;
}

#[tokio::test]
async fn a_new_page_goes_where_it_is_asked_among_the_pages_of_its_parent() {
    let test = common::setup().await;
    let user_id = common::user(&test, "user").await;
    let workspace_id = common::workspace_of(&test, &[&user_id]).await;
    let wiki = stored(&test, &workspace_id, &user_id, ContainerKind::Wiki).await;

    // Last when no place is asked for, first after nothing, and right after a page.
    let first = page(&test, &wiki, "", None).await;
    let last = page(&test, &wiki, "", None).await;
    let before_all = page(&test, &wiki, "", Some("")).await;
    let after_first = page(&test, &wiki, "", Some(&first)).await;
    let after_last = page(&test, &wiki, "", Some(&last)).await;
    let top = [&before_all, &first, &after_first, &last, &after_last];
    assert_eq!(below(&test, &wiki, "").await, top.map(String::clone));
    // The pages of another parent have their own order.
    let child = page(&test, &wiki, &first, Some("")).await;
    let second_child = page(&test, &wiki, &first, Some(&child)).await;
    assert_eq!(
        below(&test, &wiki, &first).await,
        [child.clone(), second_child]
    );

    // After a page of another parent, or after nothing that is there.
    for (parent, after) in [("", child.as_str()), (first.as_str(), last.as_str())] {
        let new = NewPage {
            parent_id: parent.to_owned(),
            after: Some(after.to_owned()),
            ..NewPage::default()
        };
        assert_eq!(
            refusal(
                test.create_page(&wiki.id, &new, PageLimits::default(), &user_id)
                    .await
            ),
            BlockError::AfterNotSibling,
            "below {parent:?} after {after}"
        );
    }
    let unknown = NewPage {
        after: Some(new_id()),
        ..NewPage::default()
    };
    assert_eq!(
        refusal(
            test.create_page(&wiki.id, &unknown, PageLimits::default(), &user_id)
                .await
        ),
        BlockError::AfterNotSibling
    );
    assert_eq!(below(&test, &wiki, "").await.len(), 5);
    test.teardown().await;
}

#[tokio::test]
async fn pages_made_at_once_get_places_of_their_own() {
    let test = common::setup().await;
    let user_id = common::user(&test, "user").await;
    let workspace_id = common::workspace_of(&test, &[&user_id]).await;
    let wiki = stored(&test, &workspace_id, &user_id, ContainerKind::Wiki).await;
    let first = page(&test, &wiki, "", None).await;

    // Half of them at the end, half right after the first page.
    let (end, next) = (None, Some(first.as_str()));
    let made = tokio::join!(
        page(&test, &wiki, "", end),
        page(&test, &wiki, "", next),
        page(&test, &wiki, "", end),
        page(&test, &wiki, "", next),
        page(&test, &wiki, "", end),
        page(&test, &wiki, "", next),
    );
    let made = [made.0, made.1, made.2, made.3, made.4, made.5];
    let tree: Vec<PageNode> = test.get_page_tree(&wiki.id).await.expect("the tree");
    let mut orders: Vec<&str> = tree.iter().map(|page| page.order.as_str()).collect();
    assert_eq!(orders.len(), made.len() + 1);
    orders.dedup();
    assert_eq!(orders.len(), made.len() + 1, "no two pages have one order");
    assert_eq!(tree[0].id, first);
    test.teardown().await;
}

#[tokio::test]
async fn the_tree_has_every_page_that_is_not_in_the_trash_and_no_body() {
    let test = common::setup().await;
    let user_id = common::user(&test, "user").await;
    let workspace_id = common::workspace_of(&test, &[&user_id]).await;
    let wiki = stored(&test, &workspace_id, &user_id, ContainerKind::Wiki).await;
    let board = stored(&test, &workspace_id, &user_id, ContainerKind::Board).await;
    let new = NewPage {
        title: "Welcome".to_owned(),
        icon: "👋".to_owned(),
        ..NewPage::default()
    };
    let made = test
        .create_page(&wiki.id, &new, PageLimits::default(), &user_id)
        .await
        .expect("the page is made")
        .page;
    let child = page(&test, &wiki, &made.id, None).await;
    let grandchild = page(&test, &wiki, &child, None).await;
    let kept = page(&test, &wiki, "", None).await;

    let tree = test.get_page_tree(&wiki.id).await.expect("the tree");
    assert_eq!(
        tree[0],
        PageNode {
            id: made.id.clone(),
            parent_id: String::new(),
            title: "Welcome".to_owned(),
            icon: "👋".to_owned(),
            order: tree[0].order.clone(),
            created_at: made.created_at,
            updated_at: made.updated_at,
        }
    );
    assert!(!tree[0].order.is_empty() && made.created_by == user_id);
    assert_eq!(tree.len(), 4);
    let of_board = test.get_page_tree(&board.id).await.expect("the tree");
    assert!(of_board.is_empty(), "a board has no pages: {of_board:?}");

    // A page goes to the trash with the pages below it.
    test.delete_block(&made.id, &user_id)
        .await
        .expect("the page goes to the trash");
    let ids: Vec<String> = test
        .get_page_tree(&wiki.id)
        .await
        .expect("the tree")
        .into_iter()
        .map(|page| page.id)
        .collect();
    assert_eq!(ids, [kept]);
    for gone in [&made.id, &child, &grandchild] {
        let read = test.get_block(gone).await;
        assert!(
            matches!(&read, Err(error) if error.is_not_found()),
            "{read:?}"
        );
    }
    test.teardown().await;
}

#[tokio::test]
async fn the_body_of_a_page_has_a_room_in_its_wiki_as_that_of_a_card_in_its_board() {
    let test = common::setup().await;
    let user_id = common::user(&test, "user").await;
    let workspace_id = common::workspace_of(&test, &[&user_id]).await;
    let wiki = stored(&test, &workspace_id, &user_id, ContainerKind::Wiki).await;
    let board = stored(&test, &workspace_id, &user_id, ContainerKind::Board).await;
    let page_id = page(&test, &wiki, "", None).await;
    let mut on_board = [
        block(&board.id, "", BlockType::Card),
        block(&board.id, "", BlockType::View),
    ];
    common::insert_blocks(&test, &mut on_board, &user_id).await;
    let [card, view] = &on_board;
    let mut comment = [block(&board.id, &card.id, BlockType::Comment)];
    common::insert_blocks(&test, &mut comment, &user_id).await;

    for (block_id, expected) in [
        (&page_id, Some(&wiki.id)),
        (&card.id, Some(&board.id)),
        // A view has no body, a comment's is written whole by its author, and a wiki
        // is no block of itself.
        (&view.id, None),
        (&comment[0].id, None),
        (&wiki.id, None),
    ] {
        let found = test
            .board_of_card(block_id)
            .await
            .expect("the board of the block is read");
        assert_eq!(found.as_ref(), expected, "{block_id}");
    }
    test.teardown().await;
}

/// The wikis and pages of two organizations stay apart (PostgreSQL; SQLite holds one
/// organization): a handle of one finds no tree and no page of the other's, either
/// way, and makes no page in the other's wiki; a handle that names no organization
/// finds none of either. Nor does a page of the other's pass for a parent or a
/// sibling of one's own, which the wiki a page is asked for in refuses already.
#[tokio::test]
async fn the_pages_of_two_organizations_are_out_of_each_others_reach() {
    if common::db_type() != DbType::Postgres {
        return;
    }
    let test = common::setup().await;
    let unset = test.scoped(Tenant::Unset);
    let (other_org, their_workspace) = common::organization(&unset, "the other").await;
    let ours = &test.store;
    let theirs = test.for_org(&other_org).expect("an identifier");
    let user_id = common::user(ours, "anybody").await;
    let workspace_id = common::workspace(ours).await;
    let wiki_in = async |store: &Store, workspace_id: &str| {
        let mut wiki = Container {
            kind: ContainerKind::Wiki,
            ..new_board(&new_id(), workspace_id, ContainerVisibility::Open)
        };
        store
            .insert_container(&mut wiki, &user_id)
            .await
            .expect("the wiki is stored");
        wiki
    };
    let our_wiki = wiki_in(ours, &workspace_id).await;
    let their_wiki = wiki_in(&theirs, &their_workspace).await;
    let our_page = page(ours, &our_wiki, "", None).await;
    let their_page = page(&theirs, &their_wiki, "", None).await;

    // Their wiki has no tree for us and takes no page of ours, and their page is no
    // block; nor ours for them. These read the blocks by nothing but their wiki or
    // their identifier: it is the wall that keeps them apart.
    for (store, wiki, page_id) in [
        (ours, &their_wiki, &their_page),
        (&theirs, &our_wiki, &our_page),
        (&unset, &their_wiki, &their_page),
        (&unset, &our_wiki, &our_page),
    ] {
        let tree = store.get_page_tree(&wiki.id).await.expect("a tree");
        assert!(tree.is_empty(), "{tree:?}");
        let found = store.board_of_card(page_id).await.expect("read");
        assert_eq!(found, None, "the wiki of the other's page");
    }
    let read = theirs.get_block(&our_page).await;
    assert!(
        matches!(&read, Err(error) if error.is_not_found()),
        "{read:?}"
    );
    let made = ours
        .create_page(
            &their_wiki.id,
            &NewPage::default(),
            PageLimits::default(),
            &user_id,
        )
        .await;
    assert!(
        matches!(&made, Err(error) if error.is_not_found()),
        "{made:?}"
    );
    let read = ours.get_block(&their_page).await;
    assert!(
        matches!(&read, Err(error) if error.is_not_found()),
        "{read:?}"
    );
    let wikis = ours
        .get_containers_of_workspace(&their_workspace, ContainerKinds::WIKIS)
        .await
        .expect("the wikis of the workspace");
    assert!(wikis.is_empty(), "{wikis:?}");

    // Nor is their page a parent or a sibling for a page of ours.
    let below_theirs = NewPage {
        parent_id: their_page.clone(),
        ..NewPage::default()
    };
    assert_eq!(
        refusal(
            ours.create_page(&our_wiki.id, &below_theirs, PageLimits::default(), &user_id)
                .await
        ),
        BlockError::ParentNotOnContainer
    );
    let after_theirs = NewPage {
        after: Some(their_page.clone()),
        ..NewPage::default()
    };
    assert_eq!(
        refusal(
            ours.create_page(&our_wiki.id, &after_theirs, PageLimits::default(), &user_id)
                .await
        ),
        BlockError::AfterNotSibling
    );

    // Each has its own, as it was.
    assert_eq!(below(ours, &our_wiki, "").await, [our_page]);
    let found = theirs.board_of_card(&their_page).await.expect("read");
    assert_eq!(found.as_ref(), Some(&their_wiki.id));
    assert_eq!(below(&theirs, &their_wiki, "").await, [their_page]);
    test.teardown().await;
}

/// A comment on a wiki is on a page of it, as one on a board is on a card of it: not at
/// the wiki's top, not on a page of another wiki, not moved off its page, and nothing is
/// below it. It goes to the trash with its page and comes back with it, and on its own
/// comes back onto its page.
#[tokio::test]
async fn a_comment_of_a_wiki_is_on_a_page_and_goes_to_the_trash_with_it() {
    let test = common::setup().await;
    let user_id = common::user(&test, "user").await;
    let workspace_id = common::workspace_of(&test, &[&user_id]).await;
    let board = stored(&test, &workspace_id, &user_id, ContainerKind::Board).await;
    let wiki = stored(&test, &workspace_id, &user_id, ContainerKind::Wiki).await;
    let other_wiki = stored(&test, &workspace_id, &user_id, ContainerKind::Wiki).await;
    let on_wiki = page(&test, &wiki, "", None).await;
    let below = page(&test, &wiki, &on_wiki, None).await;
    let elsewhere = page(&test, &other_wiki, "", None).await;

    let mut comment = block(&wiki.id, &on_wiki, BlockType::Comment);
    test.insert_block(&mut comment, &user_id)
        .await
        .expect("a comment on a page is stored");
    // At the top of the wiki: its parent is what is wrong, as on a board.
    let mut on_top = block(&wiki.id, "", BlockType::Comment);
    assert_eq!(
        refusal(test.insert_block(&mut on_top, &user_id).await),
        BlockError::CommentNotOnCardOrPage
    );
    // On a page of another wiki, and as a board's on a wiki's page.
    for (board_id, parent) in [(&wiki.id, &elsewhere), (&board.id, &on_wiki)] {
        let mut stray = block(board_id, parent, BlockType::Comment);
        assert_eq!(
            refusal(test.insert_block(&mut stray, &user_id).await),
            BlockError::ParentNotOnContainer,
            "on {board_id} below {parent}"
        );
    }
    // Moved to the top of the wiki, to another page, and with a block below it.
    let moved = |parent_id: &str| BlockPatch {
        parent_id: Some(parent_id.to_owned()),
        ..BlockPatch::default()
    };
    assert_eq!(
        refusal(test.patch_block(&comment.id, &moved(""), &user_id).await),
        BlockError::CommentNotOnCardOrPage
    );
    test.patch_block(&comment.id, &moved(&below), &user_id)
        .await
        .expect("a comment moves to another page of its wiki");
    let mut reply = block(&wiki.id, &comment.id, BlockType::Comment);
    assert_eq!(
        refusal(test.insert_block(&mut reply, &user_id).await),
        BlockError::CommentNotOnCardOrPage
    );
    // The tree has the pages only.
    let tree = test.get_page_tree(&wiki.id).await.expect("the tree");
    assert_eq!(tree.len(), 2, "{tree:?}");

    // With its page to the trash, and back with it.
    test.delete_block(&on_wiki, &user_id)
        .await
        .expect("the page is deleted");
    let deleted = test.get_deleted_block(&comment.id).await;
    assert!(
        deleted.is_ok(),
        "the comment went with its page: {deleted:?}"
    );
    assert!(test.get_block(&comment.id).await.is_err());
    test.undelete_block(&on_wiki, &user_id)
        .await
        .expect("the page comes back");
    let back = test.get_block(&comment.id).await.expect("and its comment");
    assert_eq!(
        (back.parent_id.as_str(), back.container_id.as_str()),
        (below.as_str(), wiki.id.as_str())
    );
    // On its own, onto its page.
    test.delete_block(&comment.id, &user_id)
        .await
        .expect("the comment is deleted");
    test.undelete_block(&comment.id, &user_id)
        .await
        .expect("the comment comes back onto its page");
    test.teardown().await;
}
