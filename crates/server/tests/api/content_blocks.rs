//! Moving a content block within its card: the route is gone.
//!
//! A card's content is its body, so there are no content
//! blocks to order and the route `moveContentBlock` was taken out with the card's
//! `contentOrder`.

use fvoci_model::{Block, BlockType, ContainerVisibility, new_id};

use crate::common::{Setup, TEST_WORKSPACE_ID};

/// The route of `moveContentBlock` answers 404 for two blocks of one card, which it
/// moved (204) or refused (400) before.
#[tokio::test]
async fn moving_a_content_block_is_no_route() {
    let ts = Setup::native().start().await.init_basic().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let block = |id: &str, parent: &str, block_type: BlockType| Block {
        id: id.to_owned(),
        container_id: board.id.clone(),
        parent_id: parent.to_owned(),
        block_type,
        created_at: 1,
        updated_at: 1,
        ..Block::default()
    };
    let (card, first, second) = (new_id(), new_id(), new_id());
    let created = crate::blocks::insert(
        &ts.client,
        &board.id,
        &[
            block(&card, "", BlockType::Card),
            block(&first, &card, BlockType::Attachment),
            block(&second, &card, BlockType::Attachment),
        ],
    )
    .await;
    let [_, first, second] = &created[..] else {
        panic!("three blocks are made: {created:?}");
    };
    for position in ["before", "after"] {
        let response = ts
            .client
            .post_empty(&format!(
                "/content-blocks/{}/moveto/{position}/{}",
                first.id, second.id
            ))
            .await;
        response.check_status(reqwest::StatusCode::NOT_FOUND);
    }
    ts.teardown().await;
}
