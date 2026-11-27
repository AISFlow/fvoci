//! The trash of a wiki's pages: what it lists, bringing a page back with the pages below
//! it, at the top where the page it was below is gone, and held to the wiki's limits.

use fvoci_model::{BlockError, ContainerKind, PageLimits, PageMove, new_id};
use fvoci_store::{Paging, SearchTarget, Store};

use crate::common;
use crate::pages::{below, page, refusal, stored};

/// Later than any change a test notes.
const LATER: i64 = i64::MAX;

const ROOMY: PageLimits = PageLimits {
    depth: 10,
    pages: 100,
};

async fn restore(store: &Store, wiki_id: &str, page_id: &str, user_id: &str) -> Vec<String> {
    store
        .undelete_page(wiki_id, page_id, ROOMY, user_id)
        .await
        .expect("the page comes back")
        .pages
        .into_iter()
        .map(|block| block.id)
        .collect()
}

#[tokio::test]
async fn a_page_deleted_with_those_below_is_listed_once_and_comes_back_with_them() {
    let test = common::setup().await;
    let user_id = common::user(&test, "user").await;
    let workspace_id = common::workspace_of(&test, &[&user_id]).await;
    let wiki = stored(&test, &workspace_id, &user_id, ContainerKind::Wiki).await;
    let other = stored(&test, &workspace_id, &user_id, ContainerKind::Wiki).await;
    let a = page(&test, &wiki, "", None).await;
    let b = page(&test, &wiki, &a, None).await;
    let c = page(&test, &wiki, &b, None).await;
    let d = page(&test, &wiki, "", None).await;
    let elsewhere = page(&test, &other, "", None).await;
    let order_of_a = test.get_block(&a).await.expect("a").fields["order"].clone();

    test.delete_block(&d, &user_id).await.expect("deleted");
    test.delete_block(&a, &user_id).await.expect("deleted");
    test.delete_block(&elsewhere, &user_id)
        .await
        .expect("deleted");
    let listed = test
        .get_deleted_pages(&wiki.id, None)
        .await
        .expect("listed");
    let seen: Vec<(&str, u32, &str)> = listed
        .iter()
        .map(|one| (one.id.as_str(), one.below, one.deleted_by.as_str()))
        .collect();
    assert_eq!(
        seen,
        [
            (a.as_str(), 2, user_id.as_str()),
            (d.as_str(), 0, user_id.as_str())
        ]
    );
    assert!(
        listed[0].deleted_at >= listed[1].deleted_at,
        "the last deleted first"
    );
    let second = Paging {
        page: 1,
        per_page: std::num::NonZeroU32::new(1).expect("a page size"),
    };
    let paged = test
        .get_deleted_pages(&wiki.id, Some(second))
        .await
        .expect("listed");
    assert_eq!(
        paged.iter().map(|one| one.id.as_str()).collect::<Vec<_>>(),
        [d.as_str()]
    );
    // A page that went with the page it was below comes back with that page only.
    let refused = test.undelete_page(&wiki.id, &b, ROOMY, &user_id).await;
    assert!(
        matches!(&refused, Err(error) if error.is_not_found()),
        "{refused:?}"
    );

    // The page asked for first, then those below it, parents first.
    assert_eq!(
        restore(&test, &wiki.id, &a, &user_id).await,
        [a.as_str(), b.as_str(), c.as_str()]
    );
    assert_eq!(below(&test, &wiki, "").await, [a.as_str()]);
    assert_eq!(below(&test, &wiki, &a).await, [b.as_str()]);
    assert_eq!(below(&test, &wiki, &b).await, [c.as_str()]);
    let back = test.get_block(&a).await.expect("a");
    assert_eq!(back.fields["order"], order_of_a, "at the place it had");
    let listed = test
        .get_deleted_pages(&wiki.id, None)
        .await
        .expect("listed");
    assert_eq!(
        listed.iter().map(|one| one.id.as_str()).collect::<Vec<_>>(),
        [d.as_str()]
    );

    // Brought back once; the trash of another wiki, and what is no page, are not there.
    let unknown = new_id();
    for (wiki_id, page_id) in [
        (&wiki.id, &a),
        (&wiki.id, &elsewhere),
        (&other.id, &d),
        (&wiki.id, &unknown),
    ] {
        let refused = test.undelete_page(wiki_id, page_id, ROOMY, &user_id).await;
        assert!(
            matches!(&refused, Err(error) if error.is_not_found()),
            "{page_id} of {wiki_id}: {refused:?}"
        );
    }
    test.teardown().await;
}

#[tokio::test]
async fn a_page_whose_parent_is_gone_comes_back_last_at_the_top() {
    let test = common::setup().await;
    let user_id = common::user(&test, "user").await;
    let workspace_id = common::workspace_of(&test, &[&user_id]).await;
    let wiki = stored(&test, &workspace_id, &user_id, ContainerKind::Wiki).await;
    let a = page(&test, &wiki, "", None).await;
    let b = page(&test, &wiki, &a, None).await;
    let c = page(&test, &wiki, &b, None).await;
    let d = page(&test, &wiki, "", None).await;

    test.delete_block(&b, &user_id).await.expect("deleted");
    test.delete_block(&a, &user_id).await.expect("deleted");
    let restored = test
        .undelete_page(&wiki.id, &b, ROOMY, &user_id)
        .await
        .expect("it comes back");
    assert_eq!(restored.pages[0].parent_id, "", "at the top");
    assert_eq!(
        below(&test, &wiki, "").await,
        [d.as_str(), b.as_str()],
        "last"
    );
    assert_eq!(
        below(&test, &wiki, &b).await,
        [c.as_str()],
        "with what is below it"
    );
    // Its parent comes back where it was, and the page stays where it went.
    restore(&test, &wiki.id, &a, &user_id).await;
    assert_eq!(below(&test, &wiki, &a).await, Vec::<String>::new());
    test.teardown().await;
}

#[tokio::test]
async fn a_page_whose_place_was_taken_meanwhile_comes_back_beside_it() {
    let test = common::setup().await;
    let user_id = common::user(&test, "user").await;
    let workspace_id = common::workspace_of(&test, &[&user_id]).await;
    let wiki = stored(&test, &workspace_id, &user_id, ContainerKind::Wiki).await;
    let a = page(&test, &wiki, "", None).await;
    let b = page(&test, &wiki, "", None).await;
    test.delete_block(&b, &user_id).await.expect("deleted");
    // Made last where b was, it takes b's order.
    let c = page(&test, &wiki, "", None).await;
    let order = |id: &str| {
        let id = id.to_owned();
        let test = &test;
        async move { test.get_block(&id).await.expect("a page").fields["order"].clone() }
    };
    let restored = test
        .undelete_page(&wiki.id, &b, ROOMY, &user_id)
        .await
        .expect("it comes back");
    let orders = [order(&a).await, order(&b).await, order(&c).await];
    assert_ne!(orders[1], orders[2], "each its own place");
    assert_ne!(orders[1], orders[0]);
    let top = below(&test, &wiki, "").await;
    assert_eq!(top.len(), 3);
    assert_eq!(top[0], a, "the page before it stays before it");
    assert_eq!(restored.pages[0].fields["order"], orders[1]);
    test.teardown().await;
}

#[tokio::test]
async fn a_page_comes_back_only_within_the_limits_of_the_wiki_as_it_is_now() {
    let test = common::setup().await;
    let user_id = common::user(&test, "user").await;
    let workspace_id = common::workspace_of(&test, &[&user_id]).await;
    let wiki = stored(&test, &workspace_id, &user_id, ContainerKind::Wiki).await;
    let a = page(&test, &wiki, "", None).await;
    let b = page(&test, &wiki, &a, None).await;
    let c = page(&test, &wiki, &b, None).await;
    let d = page(&test, &wiki, "", None).await;

    // b and c deleted, a moved below d: back below a they would be at depth 4.
    test.delete_block(&b, &user_id).await.expect("deleted");
    let to_d = PageMove {
        parent_id: d.clone(),
        after: None,
    };
    test.move_page(&wiki.id, &a, &to_d, ROOMY, &user_id)
        .await
        .expect("moved");
    let three_deep = PageLimits {
        depth: 3,
        pages: 100,
    };
    assert_eq!(
        refusal(test.undelete_page(&wiki.id, &b, three_deep, &user_id).await),
        BlockError::PageTooDeep { limit: 3 }
    );
    // Two pages back would make four where three may be.
    let three_pages = PageLimits {
        depth: 10,
        pages: 3,
    };
    assert_eq!(
        refusal(
            test.undelete_page(&wiki.id, &b, three_pages, &user_id)
                .await
        ),
        BlockError::TooManyPages { limit: 3 }
    );
    assert_eq!(
        test.get_deleted_pages(&wiki.id, None)
            .await
            .expect("listed")
            .len(),
        1
    );
    assert_eq!(
        restore(&test, &wiki.id, &b, &user_id).await,
        [b.as_str(), c.as_str()]
    );
    test.teardown().await;
}

#[tokio::test]
async fn pages_that_come_back_are_noted_for_the_search_server() {
    let test = common::setup().await;
    let store = &test.store;
    store.keep_search_outbox();
    let user_id = common::user(store, "user").await;
    let workspace_id = common::workspace_of(store, &[&user_id]).await;
    let wiki = stored(store, &workspace_id, &user_id, ContainerKind::Wiki).await;
    let a = page(store, &wiki, "", None).await;
    let b = page(store, &wiki, &a, None).await;
    store.delete_block(&a, &user_id).await.expect("deleted");
    for change in store.due_search_changes(LATER, 100).await.expect("due") {
        store.settle_search_change(&change).await.expect("settled");
    }

    restore(store, &wiki.id, &a, &user_id).await;
    let mut noted: Vec<(SearchTarget, String)> = store
        .due_search_changes(LATER, 100)
        .await
        .expect("due")
        .into_iter()
        .map(|change| (change.target, change.id))
        .collect();
    noted.sort_by(|one, other| one.1.cmp(&other.1));
    let mut expected = vec![
        (SearchTarget::Block, a.clone()),
        (SearchTarget::Block, b.clone()),
    ];
    expected.sort_by(|one, other| one.1.cmp(&other.1));
    assert_eq!(noted, expected);
    test.teardown().await;
}

/// The comments on the pages go to the trash with them and come back with them, and
/// are no pages: the trash says how many pages went below a page, and the limits count
/// the pages that come back.
#[tokio::test]
async fn the_comments_of_deleted_pages_are_no_pages_below_them() {
    let test = common::setup().await;
    let user_id = common::user(&test, "user").await;
    let workspace_id = common::workspace_of(&test, &[&user_id]).await;
    let wiki = stored(&test, &workspace_id, &user_id, ContainerKind::Wiki).await;
    let a = page(&test, &wiki, "", None).await;
    let b = page(&test, &wiki, &a, None).await;
    let mut comments = Vec::new();
    for on in [&a, &a, &b] {
        let mut comment = fvoci_model::Block {
            id: new_id(),
            container_id: wiki.id.clone(),
            parent_id: on.clone(),
            block_type: fvoci_model::BlockType::Comment,
            ..fvoci_model::Block::default()
        };
        test.insert_block(&mut comment, &user_id)
            .await
            .expect("a comment on the page");
        comments.push(comment.id);
    }
    test.delete_block(&a, &user_id).await.expect("deleted");
    let listed = test
        .get_deleted_pages(&wiki.id, None)
        .await
        .expect("listed");
    let seen: Vec<(&str, u32)> = listed
        .iter()
        .map(|one| (one.id.as_str(), one.below))
        .collect();
    assert_eq!(
        seen,
        [(a.as_str(), 1)],
        "one page below, whatever its comments"
    );
    // Two pages come back within a limit of two, with the comments.
    let two = PageLimits {
        depth: 10,
        pages: 2,
    };
    let back = test
        .undelete_page(&wiki.id, &a, two, &user_id)
        .await
        .expect("the pages come back within the limit");
    let back: Vec<&str> = back.pages.iter().map(|block| block.id.as_str()).collect();
    assert_eq!(back.first(), Some(&a.as_str()), "the page asked for first");
    for id in [&b].into_iter().chain(&comments) {
        assert!(back.contains(&id.as_str()), "{id} came back: {back:?}");
        test.get_block(id).await.expect("there again");
    }
    test.teardown().await;
}
