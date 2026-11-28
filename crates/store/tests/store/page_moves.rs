//! Moving a page of a wiki with the pages below it: where it may go, that its siblings
//! are numbered anew where they have no room, and the limits of the tree.

use fvoci_model::{
    BlockError, ContainerKind, JsonMap, NewPage, PAGE_ORDER_FIELD, PageLimits, PageMove, PageNode,
    new_id,
};
use fvoci_store::{Store, StoreError, Written};
use serde_json::Value;

use crate::common;
use crate::pages::{below, page, refusal, stored};

fn to(parent_id: &str, after: Option<&str>) -> PageMove {
    PageMove {
        parent_id: parent_id.to_owned(),
        after: after.map(str::to_owned),
    }
}

fn limits(depth: u32, pages: u32) -> PageLimits {
    PageLimits { depth, pages }
}

async fn moved(
    store: &Store,
    user_id: &str,
    wiki_id: &str,
    page_id: &str,
    to: &PageMove,
    limits: PageLimits,
) -> Result<Written, StoreError> {
    store.move_page(wiki_id, page_id, to, limits, user_id).await
}

/// The tree as `(id, parent, order)`, in the order it lists the pages in.
async fn tree_of(store: &Store, wiki_id: &str) -> Vec<(String, String, String)> {
    let tree: Vec<PageNode> = store.get_page_tree(wiki_id).await.expect("the tree");
    tree.into_iter()
        .map(|page| (page.id, page.parent_id, page.order))
        .collect()
}

/// Gives the pages the order `order` as they are stored, past every check.
async fn set_order(store: &Store, pages: &[&str], order: Option<&str>) {
    for page_id in pages {
        let mut fields = JsonMap::new();
        if let Some(order) = order {
            fields.insert(PAGE_ORDER_FIELD.to_owned(), Value::from(order));
        }
        store
            .set_block_fields_unchecked(page_id, &fields)
            .await
            .expect("the order is set");
    }
}

/// Whether the orders of the pages below `parent_id` are each before the next.
async fn in_order(store: &Store, wiki_id: &str, parent_id: &str) -> bool {
    let orders: Vec<String> = tree_of(store, wiki_id)
        .await
        .into_iter()
        .filter(|(_, parent, _)| parent == parent_id)
        .map(|(_, _, order)| order)
        .collect();
    orders.windows(2).all(|pair| pair[0] < pair[1])
}

#[tokio::test]
async fn a_page_moves_with_what_is_below_it_where_it_is_asked() {
    let test = common::setup().await;
    let user_id = common::user(&test, "user").await;
    let workspace_id = common::workspace_of(&test, &[&user_id]).await;
    let wiki = stored(&test, &workspace_id, &user_id, ContainerKind::Wiki).await;
    let a = page(&test, &wiki, "", None).await;
    let b = page(&test, &wiki, &a, None).await;
    let c = page(&test, &wiki, &b, None).await;
    let d = page(&test, &wiki, "", None).await;

    // To the top, first; below another page, last; and back among its siblings.
    let write = moved(
        &test,
        &user_id,
        &wiki.id,
        &b,
        &to("", Some("")),
        limits(10, 10),
    )
    .await
    .expect("moved");
    assert_eq!(
        (write.first.parent_id.as_str(), write.renumbered.len()),
        ("", 0)
    );
    assert_eq!(write.first.modified_by, user_id);
    assert_eq!(
        below(&test, &wiki, "").await,
        [b.as_str(), a.as_str(), d.as_str()]
    );
    assert_eq!(
        below(&test, &wiki, &b).await,
        [c.as_str()],
        "what is below goes along"
    );
    let _ = moved(&test, &user_id, &wiki.id, &b, &to(&d, None), limits(10, 10))
        .await
        .expect("moved");
    assert_eq!(below(&test, &wiki, "").await, [a.as_str(), d.as_str()]);
    assert_eq!(below(&test, &wiki, &d).await, [b.as_str()]);
    // The wiki is no page, and no parent; the top is no parent.
    let refused = moved(
        &test,
        &user_id,
        &wiki.id,
        &b,
        &to(&wiki.id, None),
        limits(10, 10),
    )
    .await;
    assert!(
        matches!(
            refused,
            Err(StoreError::InvalidBlock {
                source: BlockError::ParentNotOnContainer,
                ..
            })
        ),
        "{refused:?}"
    );
    let _ = moved(
        &test,
        &user_id,
        &wiki.id,
        &b,
        &to("", Some(&a)),
        limits(10, 10),
    )
    .await
    .expect("moved");
    assert_eq!(
        below(&test, &wiki, "").await,
        [a.as_str(), b.as_str(), d.as_str()]
    );
    let _ = moved(&test, &user_id, &wiki.id, &a, &to("", None), limits(10, 10))
        .await
        .expect("moved");
    assert_eq!(
        below(&test, &wiki, "").await,
        [b.as_str(), d.as_str(), a.as_str()]
    );
    assert!(in_order(&test, &wiki.id, "").await);
    // Where it is already: nothing is written.
    let before = tree_of(&test, &wiki.id).await;
    let stayed = moved(
        &test,
        &user_id,
        &wiki.id,
        &d,
        &to("", Some(&b)),
        limits(10, 10),
    )
    .await
    .expect("moved where it is");
    let stored = test.get_block(&d).await.expect("the page");
    assert_eq!(stayed.first.updated_at, stored.updated_at, "not written");
    assert_eq!(tree_of(&test, &wiki.id).await, before);
    test.teardown().await;
}

#[tokio::test]
async fn a_page_moves_neither_below_itself_nor_off_its_wiki_nor_too_deep() {
    let test = common::setup().await;
    let user_id = common::user(&test, "user").await;
    let workspace_id = common::workspace_of(&test, &[&user_id]).await;
    let wiki = stored(&test, &workspace_id, &user_id, ContainerKind::Wiki).await;
    let other = stored(&test, &workspace_id, &user_id, ContainerKind::Wiki).await;
    let a = page(&test, &wiki, "", None).await;
    let b = page(&test, &wiki, &a, None).await;
    let c = page(&test, &wiki, &b, None).await;
    let d = page(&test, &wiki, "", None).await;
    let trashed = page(&test, &wiki, "", None).await;
    test.delete_block(&trashed, &user_id)
        .await
        .expect("deleted");
    let elsewhere = page(&test, &other, "", None).await;
    let before = tree_of(&test, &wiki.id).await;
    let refused = async |page_id: &str, to: PageMove, limits: PageLimits| {
        refusal(moved(&test, &user_id, &wiki.id, page_id, &to, limits).await)
    };
    let roomy = limits(10, 10);

    for parent in [&a, &b, &c] {
        let refusal = refused(&a, to(parent, None), roomy).await;
        assert_eq!(refusal, BlockError::ParentBelowBlock, "below {parent}");
    }
    let unknown = new_id();
    for parent in [&trashed, &elsewhere, &unknown] {
        let refusal = refused(&d, to(parent, None), roomy).await;
        assert_eq!(refusal, BlockError::ParentNotOnContainer, "below {parent}");
    }
    for after in [&d, &b, &trashed] {
        let refusal = refused(&d, to("", Some(after)), roomy).await;
        assert_eq!(refusal, BlockError::AfterNotSibling, "after {after}");
    }
    // d below c would be at depth 4, a below d with b and c at 4 too.
    let too_deep = BlockError::PageTooDeep { limit: 3 };
    assert_eq!(refused(&d, to(&c, None), limits(3, 10)).await, too_deep);
    assert_eq!(refused(&a, to(&d, None), limits(3, 10)).await, too_deep);
    let _ = moved(&test, &user_id, &wiki.id, &d, &to(&b, None), limits(3, 10))
        .await
        .expect("d at depth 3");
    let _ = moved(&test, &user_id, &wiki.id, &d, &to("", None), limits(3, 10))
        .await
        .expect("d at the top again");
    // A page is moved in its own wiki only, and one in the trash not at all.
    for (wiki_id, page_id) in [
        (&wiki.id, &elsewhere),
        (&other.id, &a),
        (&wiki.id, &trashed),
    ] {
        let refused = moved(
            &test,
            &user_id,
            wiki_id,
            page_id,
            &to("", None),
            limits(10, 10),
        )
        .await;
        assert!(
            matches!(&refused, Err(error) if error.is_not_found()),
            "{page_id} on {wiki_id}: {refused:?}"
        );
    }
    let mut after = tree_of(&test, &wiki.id).await;
    let mut before = before;
    for tree in [&mut before, &mut after] {
        tree.retain(|(id, _, _)| id != &d);
    }
    assert_eq!(after, before, "nothing else moved");
    test.teardown().await;
}

#[tokio::test]
async fn siblings_of_one_order_or_none_are_numbered_anew_when_a_page_goes_among_them() {
    let test = common::setup().await;
    let user_id = common::user(&test, "user").await;
    let workspace_id = common::workspace_of(&test, &[&user_id]).await;
    let wiki = stored(&test, &workspace_id, &user_id, ContainerKind::Wiki).await;
    let [p, q, r] = [
        page(&test, &wiki, "", None).await,
        page(&test, &wiki, "", None).await,
        page(&test, &wiki, "", None).await,
    ];
    let mover = page(&test, &wiki, &p, None).await;

    // One order for all: a page made between two of them.
    set_order(&test, &[&p, &q, &r], Some("V")).await;
    let listed = below(&test, &wiki, "").await;
    let new = NewPage {
        after: Some(listed[0].clone()),
        ..NewPage::default()
    };
    let write = test
        .create_page(&wiki.id, &new, limits(10, 10), &user_id)
        .await
        .expect("made without a fault");
    assert!(
        !write.renumbered.is_empty(),
        "the siblings are numbered anew"
    );
    let mut expected = listed.clone();
    expected.insert(1, write.first.id.clone());
    assert_eq!(below(&test, &wiki, "").await, expected);
    assert!(in_order(&test, &wiki.id, "").await);
    let stored_orders: Vec<String> = tree_of(&test, &wiki.id)
        .await
        .into_iter()
        .filter(|(id, _, _)| write.renumbered.iter().any(|block| &block.id == id))
        .map(|(_, _, order)| order)
        .collect();
    let told: Vec<String> = write
        .renumbered
        .iter()
        .map(|block| {
            block.fields[PAGE_ORDER_FIELD]
                .as_str()
                .expect("an order")
                .to_owned()
        })
        .collect();
    let mut sorted = told.clone();
    sorted.sort();
    assert_eq!(stored_orders, sorted, "what is told is what is stored");

    // No order at all: a page moved between two of them.
    let all = below(&test, &wiki, "").await;
    let refs: Vec<&str> = all.iter().map(String::as_str).collect();
    set_order(&test, &refs, None).await;
    let listed = below(&test, &wiki, "").await;
    let write = moved(
        &test,
        &user_id,
        &wiki.id,
        &mover,
        &to("", Some(&listed[1])),
        limits(10, 10),
    )
    .await
    .expect("moved without a fault");
    assert_ne!(write.renumbered.len(), 0);
    let mut expected = listed.clone();
    expected.insert(2, mover.clone());
    assert_eq!(below(&test, &wiki, "").await, expected);
    assert!(in_order(&test, &wiki.id, "").await);
    test.teardown().await;
}

#[tokio::test]
async fn moves_at_once_leave_no_page_below_itself() {
    let test = common::setup().await;
    let user_id = common::user(&test, "user").await;
    let workspace_id = common::workspace_of(&test, &[&user_id]).await;
    let wiki = stored(&test, &workspace_id, &user_id, ContainerKind::Wiki).await;
    for _ in 0..4 {
        let a = page(&test, &wiki, "", None).await;
        let b = page(&test, &wiki, "", None).await;
        let (to_b, to_a) = (to(&b, None), to(&a, None));
        let (a_below_b, b_below_a) = tokio::join!(
            moved(&test, &user_id, &wiki.id, &a, &to_b, limits(10, 100)),
            moved(&test, &user_id, &wiki.id, &b, &to_a, limits(10, 100)),
        );
        let refused = match (a_below_b, b_below_a) {
            (Ok(_), Err(refused)) | (Err(refused), Ok(_)) => refused,
            other => panic!("one of the two moves is refused: {other:?}"),
        };
        assert_eq!(refusal(Err::<(), _>(refused)), BlockError::ParentBelowBlock);
        let tree = tree_of(&test, &wiki.id).await;
        let at_top = tree
            .iter()
            .filter(|(id, parent, _)| (id == &a || id == &b) && parent.is_empty())
            .count();
        assert_eq!(at_top, 1, "{tree:?}");
    }
    test.teardown().await;
}

#[tokio::test]
async fn a_wiki_is_held_to_its_depth_and_its_count_of_pages_not_in_the_trash() {
    let test = common::setup().await;
    let user_id = common::user(&test, "user").await;
    let workspace_id = common::workspace_of(&test, &[&user_id]).await;
    let wiki = stored(&test, &workspace_id, &user_id, ContainerKind::Wiki).await;
    let other = stored(&test, &workspace_id, &user_id, ContainerKind::Wiki).await;
    let small = limits(2, 3);
    let make = async |parent_id: &str| {
        let new = NewPage {
            parent_id: parent_id.to_owned(),
            ..NewPage::default()
        };
        test.create_page(&wiki.id, &new, small, &user_id).await
    };
    let top = make("").await.expect("depth 1").first.id;
    let second = make(&top).await.expect("depth 2").first.id;
    assert_eq!(
        refusal(make(&second).await),
        BlockError::PageTooDeep { limit: 2 }
    );
    // Another wiki's pages are not counted.
    for _ in 0..3 {
        page(&test, &other, "", None).await;
    }
    let third = make("").await.expect("the third page").first.id;
    assert_eq!(
        refusal(make("").await),
        BlockError::TooManyPages { limit: 3 }
    );
    // Those in the trash are not counted.
    test.delete_block(&third, &user_id).await.expect("deleted");
    let _ = make("").await.expect("one in the trash leaves room");
    assert_eq!(
        test.get_page_tree(&wiki.id).await.expect("the tree").len(),
        3
    );
    test.teardown().await;
}
