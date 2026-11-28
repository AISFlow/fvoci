//! The writes of what a request reached (`App::reach`) refuse by themselves, whoever
//! calls them: one who may not make the write is refused, and a block of another
//! container than the one reached is not there to it. A route asks before it writes
//! and hands over what it found, so these call the writes with no route in front.

use fvoci_model::{BlockPatch, BlockType, CardPatch, ContainerVisibility, NewPage};
use fvoci_server::app::ChangeNotification;
use fvoci_server::error::ErrorKind;
use serde_json::Value;

use crate::board_views::add_view;
use crate::common::{Setup, TEST_WORKSPACE_ID, TestServer};
use crate::notifications::{card_with, comment_on, give, plain};
use crate::search_pages::{new_page, new_wiki};

const SEND: ChangeNotification = ChangeNotification::Send;

/// A board of the test workspace with a view and a card with a comment, made by the
/// first client's user: the blocks of it, as the app reads them.
struct Board {
    id: String,
    view: fvoci_model::Block,
    card: fvoci_model::Block,
    comment: fvoci_model::Block,
}

async fn a_board(th: &TestServer) -> Board {
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let view = add_view(&th.client, &board.id, "View").await;
    let card = card_with(&th.client, &board.id, &Value::Null).await;
    let comment = comment_on(&th.client, &card, &plain("A word")).await;
    let card = th
        .app
        .block_of_board(&board.id, &card.id, BlockType::Card)
        .await
        .expect("the card is read");
    Board {
        id: board.id,
        view,
        card,
        comment,
    }
}

#[tokio::test]
async fn a_write_refuses_who_may_not_make_it_however_it_is_called() {
    let th = Setup::native().start().await.init_basic().await;
    let viewer = th.client2.me().await.id;
    let board = a_board(&th).await;
    let wiki_id = new_wiki(&th.client).await;
    let page = new_page(&th.client, &wiki_id, "", "Page", "text").await;
    for container in [&board.id, &wiki_id] {
        give(&th, container, &viewer, "viewer").await;
    }

    let on = th.app.reach(&viewer, &board.id).await.expect("reached");
    let refused = [
        th.app
            .patch_view(&on, &board.view, &BlockPatch::default(), SEND)
            .await
            .expect_err("a viewer changes no view"),
        th.app
            .patch_card_on(&on, &board.card, CardPatch::default(), SEND, None)
            .await
            .expect_err("a viewer changes no card"),
    ];
    let on = th.app.reach(&viewer, &wiki_id).await.expect("reached");
    let page = th.app.page_of(&wiki_id, &page.id).await.expect("the page");
    let refused = refused.into_iter().chain([
        th.app
            .create_page(&on, &NewPage::default())
            .await
            .expect_err("a viewer makes no page"),
        th.app
            .delete_page(&on, &page)
            .await
            .expect_err("a viewer deletes no page"),
    ]);
    for error in refused {
        assert_eq!(error.kind(), ErrorKind::Forbidden, "{error:?}");
    }
    th.app
        .page_of(&wiki_id, &page.id)
        .await
        .expect("the page is still there");

    th.teardown().await;
}

#[tokio::test]
async fn a_write_finds_no_block_of_another_container_than_the_one_reached() {
    let th = Setup::native().start().await.init_basic().await;
    let owner = th.client.me().await.id;
    let (a, b) = (a_board(&th).await, a_board(&th).await);
    let (wiki_a, wiki_b) = (new_wiki(&th.client).await, new_wiki(&th.client).await);
    let page_b = new_page(&th.client, &wiki_b, "", "Page", "text").await;
    let page_b = th.app.page_of(&wiki_b, &page_b.id).await.expect("the page");

    let on = th.app.reach(&owner, &a.id).await.expect("reached");
    let title = BlockPatch {
        title: Some("Moved".to_owned()),
        ..BlockPatch::default()
    };
    let missing = [
        th.app
            .patch_view(&on, &b.view, &title, SEND)
            .await
            .expect_err("a view of B is none of A"),
        th.app
            .patch_card_on(&on, &b.card, CardPatch::default(), SEND, None)
            .await
            .expect_err("a card of B is none of A"),
        th.app
            .delete_comment(&on, &b.comment, "denied", SEND)
            .await
            .expect_err("a comment of B is none of A"),
    ];
    let on = th.app.reach(&owner, &wiki_a).await.expect("reached");
    let missing = missing.into_iter().chain([th
        .app
        .delete_page(&on, &page_b)
        .await
        .expect_err("a page of B is none of A")]);
    for error in missing {
        assert!(error.is_not_found(), "{error:?}");
    }
    for block in [&b.view, &b.card, &b.comment] {
        let read = th
            .app
            .block_of_board(&b.id, &block.id, block.block_type)
            .await
            .expect("the block of B is still there");
        assert_eq!(read.title, block.title, "nothing of it is written");
    }
    th.app
        .page_of(&wiki_b, &page_b.id)
        .await
        .expect("the page of B is still there");

    th.teardown().await;
}
