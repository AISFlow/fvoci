//! Duplicating a page of a wiki with every page below it: the copies right after the
//! page, under new identifiers, with their titles, icons, places and bodies, held to the
//! wiki's limits.

use fvoci_model::{BlockError, BlockPatch, Body, BodyChange, ContainerKind, PageLimits, new_id};
use serde_json::json;

use crate::common;
use crate::pages::{below, page, refusal, stored};

const ROOMY: PageLimits = PageLimits {
    depth: 10,
    pages: 100,
};

fn text(line: &str) -> Body {
    Body::parse(json!({"type": "doc", "content": [
        {"type": "paragraph", "content": [{"type": "text", "text": line}]}
    ]}))
    .expect("a body")
    .expect("not empty")
}

#[tokio::test]
async fn a_page_is_copied_with_the_pages_below_it_right_after_it() {
    let test = common::setup().await;
    let user_id = common::user(&test, "user").await;
    let other_id = common::user(&test, "other").await;
    let workspace_id = common::workspace_of(&test, &[&user_id, &other_id]).await;
    let wiki = stored(&test, &workspace_id, &user_id, ContainerKind::Wiki).await;
    let a = page(&test, &wiki, "", None).await;
    let b = page(&test, &wiki, &a, None).await;
    let c = page(&test, &wiki, &b, None).await;
    let b2 = page(&test, &wiki, &a, None).await;
    let d = page(&test, &wiki, "", None).await;
    let body = BlockPatch {
        title: Some("Bee".to_owned()),
        body: BodyChange::Set(text("below")),
        ..BlockPatch::default()
    };
    test.patch_block(&b, &body, &user_id)
        .await
        .expect("patched");

    let copies = test
        .duplicate_page(&wiki.id, &a, Some("A (copy)"), ROOMY, &other_id)
        .await
        .expect("copied");
    // A parent before what is below it: a, b and b2, c.
    let ids: Vec<&str> = copies.pages.iter().map(|copy| copy.id.as_str()).collect();
    assert_eq!(ids.len(), 4);
    for original in [&a, &b, &c, &b2, &d] {
        assert!(!ids.contains(&original.as_str()), "new identifiers");
    }
    let top = &copies.pages[0];
    assert_eq!(top.title, "A (copy)");
    assert_eq!(top.created_by, other_id, "made by whoever copies");
    assert_eq!(
        below(&test, &wiki, "").await,
        [a.as_str(), ids[0], d.as_str()],
        "right after the page"
    );
    // Below the copy as below the page, in the same order, with titles and bodies.
    let copied_b = copies
        .pages
        .iter()
        .find(|copy| copy.title == "Bee")
        .expect("a copy of b");
    assert_eq!(
        below(&test, &wiki, ids[0]).await,
        [copied_b.id.as_str(), ids[2]]
    );
    assert_eq!(
        below(&test, &wiki, &copied_b.id).await.len(),
        1,
        "and what is below that"
    );
    assert_eq!(copied_b.body, Some(text("below")));
    assert_eq!(
        test.get_block(&b).await.expect("b").title,
        "Bee",
        "the page stays"
    );
    assert_eq!(below(&test, &wiki, &a).await, [b.as_str(), b2.as_str()]);

    // Left out, the title is the page's own.
    let again = test
        .duplicate_page(&wiki.id, &c, None, ROOMY, &user_id)
        .await
        .expect("copied");
    assert_eq!(again.pages.len(), 1);
    assert_eq!(again.pages[0].parent_id, b);
    test.teardown().await;
}

#[tokio::test]
async fn a_copy_is_held_to_the_limits_and_made_of_a_page_of_the_wiki_only() {
    let test = common::setup().await;
    let user_id = common::user(&test, "user").await;
    let workspace_id = common::workspace_of(&test, &[&user_id]).await;
    let wiki = stored(&test, &workspace_id, &user_id, ContainerKind::Wiki).await;
    let other = stored(&test, &workspace_id, &user_id, ContainerKind::Wiki).await;
    let a = page(&test, &wiki, "", None).await;
    let b = page(&test, &wiki, &a, None).await;
    page(&test, &wiki, &b, None).await;

    // Three pages and three more would be six where five may be.
    let five = PageLimits {
        depth: 10,
        pages: 5,
    };
    assert_eq!(
        refusal(
            test.duplicate_page(&wiki.id, &a, None, five, &user_id)
                .await
        ),
        BlockError::TooManyPages { limit: 5 }
    );
    // Deeper than a limit lowered since: the copy would be as deep.
    let two_deep = PageLimits {
        depth: 2,
        pages: 100,
    };
    assert_eq!(
        refusal(
            test.duplicate_page(&wiki.id, &a, None, two_deep, &user_id)
                .await
        ),
        BlockError::PageTooDeep { limit: 2 }
    );
    assert_eq!(below(&test, &wiki, "").await, [a.as_str()], "nothing made");

    let long = "x".repeat(16_384);
    assert_eq!(
        refusal(
            test.duplicate_page(&wiki.id, &a, Some(&long), ROOMY, &user_id)
                .await
        ),
        BlockError::TitleSizeLimitExceeded
    );
    let unknown = new_id();
    for (wiki_id, page_id) in [(&other.id, &a), (&wiki.id, &unknown)] {
        let refused = test
            .duplicate_page(wiki_id, page_id, None, ROOMY, &user_id)
            .await;
        assert!(
            matches!(&refused, Err(error) if error.is_not_found()),
            "{page_id} of {wiki_id}: {refused:?}"
        );
    }
    test.delete_block(&b, &user_id).await.expect("deleted");
    let gone = test
        .duplicate_page(&wiki.id, &b, None, ROOMY, &user_id)
        .await;
    assert!(
        matches!(&gone, Err(error) if error.is_not_found()),
        "{gone:?}"
    );
    test.teardown().await;
}
