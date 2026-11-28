//! Integration tests for atomic multi-board and block creation, deletion, and
//! duplication. Every identifier is generated, referenced workspaces and users are stored
//! first, and atomic rollback assertions run on both SQLite and PostgreSQL.

use fvoci_model::{
    BLOCK_TITLE_MAX_CHARS, Block, BlockError, BlockPatch, BlockType, Container, ContainerKinds,
    ContainerMember, ContainerVisibility, ContainersAndBlocks, DeleteContainersAndBlocks, is_id,
    new_id,
};
use fvoci_model::{DuplicateAs, PublicContainers};
use fvoci_store::{
    BlockFilter, CopyChanges, CopyTitles, Creator, NewContainers, Store, StoreError,
};

use crate::common;

/// The titles of copies, in English, as a caller of the store brings them.
const TITLES: CopyTitles = CopyTitles {
    copy_of: |title| format!("{title} copy"),
    new_template: "New board template",
};

/// New identifiers, as many as the pattern they are bound to names.
fn new_ids<const N: usize>() -> [String; N] {
    std::array::from_fn(|_| new_id())
}

/// The identifiers, sorted.
fn sorted(ids: &[&str]) -> Vec<String> {
    let mut ids: Vec<String> = ids.iter().map(|&id| id.to_owned()).collect();
    ids.sort();
    ids
}

fn new_board(id: &str, workspace_id: &str, board_type: ContainerVisibility) -> Container {
    Container {
        id: id.to_owned(),
        workspace_id: workspace_id.to_owned(),
        visibility: board_type,
        ..Container::default()
    }
}

fn new_card(id: &str, board_id: &str) -> Block {
    Block {
        id: id.to_owned(),
        container_id: board_id.to_owned(),
        block_type: BlockType::Card,
        ..Block::default()
    }
}

fn sorted_board_ids(boards: &[Container]) -> Vec<String> {
    let mut ids: Vec<String> = boards.iter().map(|board| board.id.clone()).collect();
    ids.sort();
    ids
}

fn is_title_size_limit_exceeded(error: &StoreError) -> bool {
    matches!(
        error,
        StoreError::InvalidBlock {
            source: BlockError::TitleSizeLimitExceeded,
            ..
        }
    )
}

/// Inserts a new open board of the workspace on behalf of the user and returns its
/// identifier.
async fn insert_new_board(store: &Store, workspace_id: &str, user_id: &str) -> String {
    let mut board = new_board(&new_id(), workspace_id, ContainerVisibility::Open);
    store
        .insert_container(&mut board, user_id)
        .await
        .expect("the board is inserted");
    board.id
}

/// Inserts a new block of the given board on behalf of the user and returns its
/// identifier.
async fn insert_new_block(store: &Store, board_id: &str, user_id: &str) -> String {
    let mut block = Block {
        id: new_id(),
        container_id: board_id.to_owned(),
        ..Block::default()
    };
    store
        .insert_block(&mut block, user_id)
        .await
        .expect("the block is inserted");
    block.id
}

/// Two boards with two blocks each, used by the batch-deletion test cases.
struct TwoBoards {
    board_ids: [String; 2],
    block_ids: [String; 4],
}

impl TwoBoards {
    /// Inserts the boards and the blocks. `last_block_board_id` overrides the board of
    /// the fourth block; it must name a board that exists.
    async fn insert(
        store: &Store,
        workspace_id: &str,
        user_id: &str,
        last_block_board_id: Option<&str>,
    ) -> Self {
        let board1 = insert_new_board(store, workspace_id, user_id).await;
        let block1 = insert_new_block(store, &board1, user_id).await;
        let block2 = insert_new_block(store, &board1, user_id).await;
        let board2 = insert_new_board(store, workspace_id, user_id).await;
        let block3 = insert_new_block(store, &board2, user_id).await;
        let block4 = insert_new_block(store, last_block_board_id.unwrap_or(&board2), user_id).await;
        Self {
            board_ids: [board1, board2],
            block_ids: [block1, block2, block3, block4],
        }
    }

    /// "all the entities should still exist"
    async fn assert_all_exist(&self, store: &Store) {
        for board_id in &self.board_ids {
            let result = store.get_container(board_id).await;
            assert!(result.is_ok(), "board {board_id}: {result:?}");
        }
        for block_id in &self.block_ids {
            let result = store.get_block(block_id).await;
            assert!(result.is_ok(), "block {block_id}: {result:?}");
        }
    }
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "sequential batch-creation cases share one store fixture"
)]
async fn create_containers_and_blocks() {
    let test = common::setup().await;
    let user_id = common::user(&test, "user").await;
    let workspace_id = common::workspace_of(&test, &[&user_id]).await;

    let stored = test
        .get_containers_for_user_and_workspace(
            &user_id,
            &workspace_id,
            PublicContainers::Include,
            ContainerKinds::BOARDS,
        )
        .await
        .expect("the query succeeds");
    assert!(stored.is_empty(), "got {stored:?}");

    // "create boards and blocks"
    {
        let [board1, board2, board3] = new_ids();
        let [block1, block2] = new_ids();
        let mut new_bab = ContainersAndBlocks {
            containers: vec![
                new_board(&board1, &workspace_id, ContainerVisibility::Open),
                new_board(&board2, &workspace_id, ContainerVisibility::Private),
                new_board(&board3, &workspace_id, ContainerVisibility::Open),
            ],
            blocks: vec![new_card(&block1, &board1), new_card(&block2, &board2)],
        };

        test.create_containers(
            NewContainers::whole(&mut new_bab).creator(Creator::Nobody),
            &user_id,
        )
        .await
        .map_err(StoreError::from)
        .expect("the boards and blocks are created");
        assert_eq!(new_bab.containers.len(), 3, "got {:?}", new_bab.containers);
        assert_eq!(new_bab.blocks.len(), 2, "got {:?}", new_bab.blocks);

        assert_eq!(
            sorted_board_ids(&new_bab.containers),
            sorted(&[&board1, &board2, &board3]),
            "the boards keep their identifiers"
        );
        assert_eq!(
            common::sorted_ids(&new_bab.blocks),
            sorted(&[&block1, &block2]),
            "the blocks keep their identifiers"
        );
    }

    // "create boards and blocks with admin"
    {
        let [board4, board5, board6] = new_ids();
        let [block3, block4] = new_ids();
        let mut new_bab = ContainersAndBlocks {
            containers: vec![
                new_board(&board4, &workspace_id, ContainerVisibility::Open),
                new_board(&board5, &workspace_id, ContainerVisibility::Private),
                new_board(&board6, &workspace_id, ContainerVisibility::Open),
            ],
            blocks: vec![new_card(&block3, &board4), new_card(&block4, &board5)],
        };

        let members = test
            .create_containers(NewContainers::whole(&mut new_bab), &user_id)
            .await
            .map_err(StoreError::from)
            .expect("the boards, blocks and admins are created");
        assert_eq!(new_bab.containers.len(), 3, "got {:?}", new_bab.containers);
        assert_eq!(new_bab.blocks.len(), 2, "got {:?}", new_bab.blocks);
        assert_eq!(members.len(), 3, "got {members:?}");

        assert_eq!(
            sorted_board_ids(&new_bab.containers),
            sorted(&[&board4, &board5, &board6]),
            "the boards keep their identifiers"
        );
        assert_eq!(
            common::sorted_ids(&new_bab.blocks),
            sorted(&[&block3, &block4]),
            "the blocks keep their identifiers"
        );

        let mut member_board_ids = Vec::new();
        for member in &members {
            assert_eq!(user_id, member.user_id, "the creator is the member");
            member_board_ids.push(member.container_id.clone());
        }
        member_board_ids.sort_unstable();
        assert_eq!(
            member_board_ids,
            sorted(&[&board4, &board5, &board6]),
            "one membership per board"
        );
    }

    // The two subtests that follow name the same boards and blocks: neither stores any.
    let [board7, board8, board9] = new_ids();
    let [block5, block6] = new_ids();

    // "on failure, nothing should be saved"
    {
        // one of the blocks is invalid as it doesn't have BoardID
        let mut new_bab = ContainersAndBlocks {
            containers: vec![
                new_board(&board7, &workspace_id, ContainerVisibility::Open),
                new_board(&board8, &workspace_id, ContainerVisibility::Private),
                new_board(&board9, &workspace_id, ContainerVisibility::Open),
            ],
            blocks: vec![new_card(&block5, &board7), new_card(&block6, "")],
        };

        let error = test
            .create_containers(
                NewContainers::whole(&mut new_bab).creator(Creator::Nobody),
                &user_id,
            )
            .await
            .map_err(StoreError::from)
            .expect_err("a block without a board is rejected");
        assert!(
            matches!(
                error,
                StoreError::InvalidBlock {
                    source: BlockError::EmptyContainerId,
                    ..
                }
            ),
            "got {error:?}"
        );

        let error = test
            .create_containers(NewContainers::whole(&mut new_bab), &user_id)
            .await
            .map_err(StoreError::from)
            .expect_err("a block without a board is rejected");
        assert!(
            matches!(error, StoreError::InvalidBlock { .. }),
            "got {error:?}"
        );

        // Verify rollback on both databases: neither the boards nor the blocks are saved.
        for board_id in [&board7, &board8, &board9] {
            let error = test
                .get_container(board_id)
                .await
                .expect_err("the board was not saved");
            assert!(error.is_not_found(), "board {board_id}: {error:?}");
        }
        let error = test
            .get_block(&block5)
            .await
            .expect_err("the block was not saved");
        assert!(error.is_not_found(), "got {error:?}");
        let members = test
            .get_members_for_user(&user_id)
            .await
            .expect("the memberships are read");
        assert_eq!(members.len(), 3, "only the three of the previous subtest");
    }

    // "should apply block size limits"
    {
        // one of the blocks is invalid as it has a title too large
        let mut new_bab = ContainersAndBlocks {
            containers: vec![
                new_board(&board7, &workspace_id, ContainerVisibility::Open),
                new_board(&board8, &workspace_id, ContainerVisibility::Private),
                new_board(&board9, &workspace_id, ContainerVisibility::Open),
            ],
            blocks: vec![
                new_card(&block5, &board7),
                Block {
                    title: "A".repeat(BLOCK_TITLE_MAX_CHARS + 1),
                    ..new_card(&block6, &board8)
                },
            ],
        };

        let error = test
            .create_containers(
                NewContainers::whole(&mut new_bab).creator(Creator::Nobody),
                &user_id,
            )
            .await
            .map_err(StoreError::from)
            .expect_err("a block with an oversized title is rejected");
        assert!(is_title_size_limit_exceeded(&error), "got {error:?}");

        let error = test
            .create_containers(NewContainers::whole(&mut new_bab), &user_id)
            .await
            .map_err(StoreError::from)
            .expect_err("a block with an oversized title is rejected");
        assert!(is_title_size_limit_exceeded(&error), "got {error:?}");
    }

    test.teardown().await;
}

#[tokio::test]
async fn delete_containers_and_blocks() {
    let test = common::setup().await;
    let workspace_id = common::workspace(&test).await;
    let user_id = common::user(&test, "user").await;

    // "should not delete anything if a block doesn't belong to any of the boards"
    {
        // Place the fourth block on an existing third board not included in the delete request.
        let different_board_id = insert_new_board(&test, &workspace_id, &user_id).await;
        let entities =
            TwoBoards::insert(&test, &workspace_id, &user_id, Some(&different_board_id)).await;

        let dbab = DeleteContainersAndBlocks {
            containers: entities.board_ids.to_vec(),
            blocks: entities.block_ids.to_vec(),
        };

        let expected_error_msg = format!(
            "block {} doesn't belong to any of the boards in the delete request",
            entities.block_ids[3]
        );
        let error = test
            .delete_containers_and_blocks(&dbab, &user_id)
            .await
            .expect_err("the fourth block belongs to another board");
        assert_eq!(error.to_string(), expected_error_msg, "got {error:?}");

        // all the entities should still exist
        entities.assert_all_exist(&test).await;
    }

    // "should not delete anything if a board doesn't exist"
    {
        let entities = TwoBoards::insert(&test, &workspace_id, &user_id, None).await;

        let mut boards = entities.board_ids.to_vec();
        boards.push(new_id());
        let dbab = DeleteContainersAndBlocks {
            containers: boards,
            blocks: entities.block_ids.to_vec(),
        };

        let error = test
            .delete_containers_and_blocks(&dbab, &user_id)
            .await
            .expect_err("the third board does not exist");
        assert!(error.is_not_found(), "got {error:?}");

        // all the entities should still exist
        entities.assert_all_exist(&test).await;
    }

    // "should not delete anything if a block doesn't exist"
    {
        let entities = TwoBoards::insert(&test, &workspace_id, &user_id, None).await;

        let mut blocks = entities.block_ids.to_vec();
        blocks.push(new_id());
        let dbab = DeleteContainersAndBlocks {
            containers: entities.board_ids.to_vec(),
            blocks,
        };

        let error = test
            .delete_containers_and_blocks(&dbab, &user_id)
            .await
            .expect_err("the fifth block does not exist");
        assert!(error.is_not_found(), "got {error:?}");

        // all the entities should still exist
        entities.assert_all_exist(&test).await;
    }

    // "should work properly if all the entities are related"
    {
        let entities = TwoBoards::insert(&test, &workspace_id, &user_id, None).await;

        let dbab = DeleteContainersAndBlocks {
            containers: entities.board_ids.to_vec(),
            blocks: entities.block_ids.to_vec(),
        };

        test.delete_containers_and_blocks(&dbab, &user_id)
            .await
            .expect("the boards and blocks are deleted");

        for board_id in &entities.board_ids {
            let error = test
                .get_container(board_id)
                .await
                .expect_err("the board is gone");
            assert!(error.is_not_found(), "board {board_id}: {error:?}");
        }
        for block_id in &entities.block_ids {
            let error = test
                .get_block(block_id)
                .await
                .expect_err("the block is gone");
            assert!(error.is_not_found(), "block {block_id}: {error:?}");
        }

        // Verify the deleted boards landed in the trash.
        for board_id in &entities.board_ids {
            let deleted = test
                .get_deleted_container(board_id)
                .await
                .expect("the board is in the trash");
            assert_eq!(deleted.deleted_by, user_id, "who deleted board {board_id}");
        }
        for block_id in &entities.block_ids {
            let deleted = test
                .get_deleted_block(block_id)
                .await
                .expect("the block is in the trash");
            assert_eq!(deleted.deleted_by, user_id, "who deleted block {block_id}");
        }
    }

    test.teardown().await;
}

#[tokio::test]
async fn duplicate_container() {
    let test = common::setup().await;
    let workspace_id = common::workspace(&test).await;
    let user_id = common::user(&test, "user").await;

    let [board1, board2, board3] = new_ids();
    let [block1, block1a, block2] = new_ids();
    let mut new_bab = ContainersAndBlocks {
        containers: vec![
            new_board(&board1, &workspace_id, ContainerVisibility::Open),
            new_board(&board2, &workspace_id, ContainerVisibility::Private),
            new_board(&board3, &workspace_id, ContainerVisibility::Open),
        ],
        blocks: vec![
            new_card(&block1, &board1),
            Block {
                block_type: BlockType::Comment,
                parent_id: block1.clone(),
                ..new_card(&block1a, &board1)
            },
            new_card(&block2, &board2),
        ],
    };

    test.create_containers(
        NewContainers::whole(&mut new_bab).creator(Creator::Nobody),
        &user_id,
    )
    .await
    .map_err(StoreError::from)
    .expect("the boards and blocks are created");
    assert_eq!(new_bab.containers.len(), 3, "got {:?}", new_bab.containers);
    assert_eq!(new_bab.blocks.len(), 3, "got {:?}", new_bab.blocks);

    // "duplicate existing board as no template"
    {
        common::works_in(&test, &workspace_id, &user_id).await;
        let (bab, members) = test
            .duplicate_container(
                &board1,
                &user_id,
                Some(&workspace_id),
                DuplicateAs::Copy,
                &TITLES,
                CopyChanges::default(),
            )
            .await
            .expect("the board is duplicated");
        assert_eq!(members.len(), 1, "got {members:?}");
        assert_eq!(bab.containers.len(), 1, "got {:?}", bab.containers);
        assert_eq!(bab.blocks.len(), 1, "got {:?}", bab.blocks);
        assert!(
            !bab.containers[0].is_template,
            "got {:?}",
            bab.containers[0]
        );
    }

    // "duplicate existing board as template"
    {
        let (bab, members) = test
            .duplicate_container(
                &board1,
                &user_id,
                Some(&workspace_id),
                DuplicateAs::Template,
                &TITLES,
                CopyChanges::default(),
            )
            .await
            .expect("the board is duplicated");
        assert_eq!(members.len(), 1, "got {members:?}");
        assert_eq!(bab.containers.len(), 1, "got {:?}", bab.containers);
        assert_eq!(bab.blocks.len(), 1, "got {:?}", bab.blocks);
        assert!(bab.containers[0].is_template, "got {:?}", bab.containers[0]);
    }

    // "duplicate not existing board"
    {
        let error = test
            .duplicate_container(
                &new_id(),
                &user_id,
                Some(&workspace_id),
                DuplicateAs::Copy,
                &TITLES,
                CopyChanges::default(),
            )
            .await
            .expect_err("there is no such board");
        assert!(error.is_not_found(), "got {error:?}");
    }

    test.teardown().await;
}

// Additional batch-operation and duplication invariants.

/// Duplicating a board copies its blocks, rewrites internal references, omits comments,
/// and applies template/title options.
#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "the four kinds of duplication build on one another"
)]
async fn duplicate_container_copies_under_new_ids_as_a_private_board_of_the_user() {
    let test = common::setup().await;
    let workspace_id = common::workspace(&test).await;
    let other_workspace_id = common::workspace(&test).await;
    let author = common::user(&test, "author").await;
    let copier = common::user(&test, "copier").await;

    let source_id = new_id();
    let [card_id, text_id, comment_id] = new_ids();
    let mut source = ContainersAndBlocks {
        containers: vec![Container {
            title: "Roadmap".to_owned(),
            description: "Plans".to_owned(),
            icon: "🗺".to_owned(),
            ..new_board(&source_id, &workspace_id, ContainerVisibility::Open)
        }],
        blocks: vec![
            Block {
                title: "Card".to_owned(),
                ..new_card(&card_id, &source_id)
            },
            Block {
                parent_id: card_id.clone(),
                block_type: BlockType::Attachment,
                title: "Text".to_owned(),
                ..new_card(&text_id, &source_id)
            },
            Block {
                parent_id: card_id.clone(),
                block_type: BlockType::Comment,
                ..new_card(&comment_id, &source_id)
            },
        ],
    };
    test.create_containers(
        NewContainers::whole(&mut source).creator(Creator::Nobody),
        &author,
    )
    .await
    .map_err(StoreError::from)
    .expect("the board and its blocks are created");

    // board -> board, same workspace
    common::works_in(&test, &workspace_id, &copier).await;
    let (copy, members) = test
        .duplicate_container(
            &source_id,
            &copier,
            None,
            DuplicateAs::Copy,
            &TITLES,
            CopyChanges::default(),
        )
        .await
        .expect("the board is duplicated");
    let board = &copy.containers[0];
    assert_ne!(board.id, source_id, "the copy has an identifier of its own");
    assert!(is_id(&board.id), "got {}", board.id);
    assert_eq!(board.title, "Roadmap copy", "the title says it is a copy");
    assert_eq!(board.description, "Plans", "the description is copied");
    assert_eq!(board.icon, "🗺", "the icon is copied");
    assert_eq!(
        board.visibility,
        ContainerVisibility::Private,
        "a copy is private"
    );
    assert_eq!(
        board.workspace_id, workspace_id,
        "the copy stays in the workspace"
    );
    assert_eq!(board.created_by, copier, "the copier made the copy");
    assert_eq!(board.modified_by, copier, "the copier made the copy");
    assert!(!board.is_template, "a copy of a board is a board");
    assert!(
        !board.is_builtin,
        "a user's copy is not a built-in template"
    );
    assert_eq!(
        members,
        [ContainerMember {
            container_id: board.id.clone(),
            user_id: copier.clone(),
            role: fvoci_model::ContainerRole::Admin,
            synthetic: false,
            inherited_role: None
        }],
        "the copier administers the copy"
    );

    // The blocks keep their shape under new identifiers; the comment is left behind.
    assert_eq!(copy.blocks.len(), 2, "got {:?}", copy.blocks);
    let card = copy
        .blocks
        .iter()
        .find(|block| block.block_type == BlockType::Card)
        .expect("the card is copied");
    let paragraph = copy
        .blocks
        .iter()
        .find(|block| block.block_type == BlockType::Attachment)
        .expect("the text is copied");
    assert_ne!(card.id, card_id, "the card has an identifier of its own");
    assert!(is_id(&card.id), "got {}", card.id);
    assert_eq!(card.container_id, board.id, "the card is on the copy");
    assert_eq!(
        card.parent_id, "",
        "a block directly on its board is copied without a parent"
    );
    assert_eq!(paragraph.parent_id, card.id, "the text follows its card");
    assert_eq!(paragraph.container_id, board.id, "the text is on the copy");
    assert_eq!(card.created_by, copier, "the copier made the card");

    // What is returned is what is stored.
    let stored_board = test
        .get_container(&board.id)
        .await
        .expect("the copy exists");
    assert_eq!(&stored_board, board, "the stored copy");
    let stored_blocks = test
        .get_blocks(&BlockFilter::container(&board.id))
        .await
        .expect("the blocks are read");
    assert_eq!(
        common::sorted_ids(&stored_blocks),
        common::sorted_ids(&copy.blocks),
        "the stored blocks of the copy"
    );
    // The source is untouched.
    let source_board = test
        .get_container(&source_id)
        .await
        .expect("the source exists");
    assert_eq!(source_board, source.containers[0], "the source board");

    // board -> template, other workspace
    common::works_in(&test, &other_workspace_id, &copier).await;
    let (template, _) = test
        .duplicate_container(
            &source_id,
            &copier,
            Some(&other_workspace_id),
            DuplicateAs::Template,
            &TITLES,
            CopyChanges::default(),
        )
        .await
        .expect("the board is duplicated");
    let template = &template.containers[0];
    assert_eq!(template.title, "New board template", "a new template");
    assert_eq!(
        template.workspace_id, other_workspace_id,
        "in the workspace asked for"
    );
    assert!(template.is_template, "got {template:?}");

    // template -> template
    let (template_copy, _) = test
        .duplicate_container(
            &template.id,
            &copier,
            None,
            DuplicateAs::Template,
            &TITLES,
            CopyChanges::default(),
        )
        .await
        .expect("the template is duplicated");
    assert_eq!(
        template_copy.containers[0].title, "New board template copy",
        "a copy of a template"
    );
    assert_eq!(
        template_copy.containers[0].workspace_id, other_workspace_id,
        "in the workspace of the template"
    );

    // template -> board
    let (from_template, _) = test
        .duplicate_container(
            &template.id,
            &copier,
            None,
            DuplicateAs::Copy,
            &TITLES,
            CopyChanges::default(),
        )
        .await
        .expect("the template is duplicated");
    assert_eq!(
        from_template.containers[0].title, "New board template",
        "a board made from a template keeps the title"
    );
    assert!(
        !from_template.containers[0].is_template,
        "got {:?}",
        from_template.containers[0]
    );

    test.teardown().await;
}

/// A board without blocks, or with comments only, is copied as a board without blocks.
#[tokio::test]
async fn duplicate_container_copies_a_board_without_blocks() {
    let test = common::setup().await;
    let workspace_id = common::workspace(&test).await;
    let user_id = common::user(&test, "user").await;

    let empty_id = insert_new_board(&test, &workspace_id, &user_id).await;
    let only_comments_id = insert_new_board(&test, &workspace_id, &user_id).await;
    let mut card = new_card(&new_id(), &only_comments_id);
    test.insert_block(&mut card, &user_id)
        .await
        .expect("the card is inserted");
    let card_id = card.id;
    let mut comment = Block {
        block_type: BlockType::Comment,
        parent_id: card_id.clone(),
        ..new_card(&new_id(), &only_comments_id)
    };
    test.insert_block(&mut comment, &user_id)
        .await
        .expect("the comment is inserted");
    // A comment stored before a comment had to be on a card: on the board itself, the
    // card gone.
    common::execute_raw(
        &test.database_connection(),
        &format!(
            "UPDATE blocks SET parent_id = NULL WHERE id = '{}'",
            comment.id
        ),
    )
    .await;
    test.delete_block(&card_id, &user_id)
        .await
        .expect("the card is deleted");

    common::works_in(&test, &workspace_id, &user_id).await;
    for (board_id, what) in [(&empty_id, "empty"), (&only_comments_id, "comments only")] {
        let (copy, members) = test
            .duplicate_container(
                board_id,
                &user_id,
                None,
                DuplicateAs::Copy,
                &TITLES,
                CopyChanges::default(),
            )
            .await
            .expect("a board without blocks to copy is copied");
        assert_eq!(copy.containers.len(), 1, "{what}: one board");
        assert_ne!(&copy.containers[0].id, board_id, "{what}: a new identifier");
        assert!(copy.blocks.is_empty(), "{what}: no blocks");
        assert_eq!(members.len(), 1, "{what}: the copying user");
        let stored = test
            .get_blocks(&BlockFilter::container(&copy.containers[0].id))
            .await
            .expect("the copy's blocks are read");
        assert!(stored.is_empty(), "{what}: nothing was stored");
    }

    test.teardown().await;
}

/// Listed blocks are validated before deletion begins, so listing a child block after its
/// parent succeeds regardless of order.
#[tokio::test]
async fn delete_containers_and_blocks_accepts_a_block_listed_after_its_parent() {
    let test = common::setup().await;
    let workspace_id = common::workspace(&test).await;
    let user_id = common::user(&test, "user").await;

    for (order, child_first) in [("the parent first", false), ("the child first", true)] {
        let board_id = insert_new_board(&test, &workspace_id, &user_id).await;
        let card_id = insert_new_block(&test, &board_id, &user_id).await;
        let mut child = Block {
            id: new_id(),
            container_id: board_id.clone(),
            parent_id: card_id.clone(),
            ..Block::default()
        };
        test.insert_block(&mut child, &user_id)
            .await
            .expect("the child is inserted");

        let mut blocks = vec![card_id.clone(), child.id.clone()];
        if child_first {
            blocks.reverse();
        }
        let request = DeleteContainersAndBlocks {
            containers: vec![board_id.clone()],
            blocks,
        };
        test.delete_containers_and_blocks(&request, &user_id)
            .await
            .unwrap_or_else(|error| panic!("{order}: the request is refused: {error:?}"));

        let error = test
            .get_container(&board_id)
            .await
            .expect_err("the board is gone");
        assert!(error.is_not_found(), "{order}: got {error:?}");
        for block_id in [&card_id, &child.id] {
            let error = test
                .get_block(block_id)
                .await
                .expect_err("the block is gone");
            assert!(
                error.is_not_found(),
                "{order}, block {block_id}: got {error:?}"
            );
        }
    }

    test.teardown().await;
}

/// A board goes to the trash with all its blocks, listed or not, as one deletion: they
/// come back together when the board is restored.
#[tokio::test]
async fn delete_containers_and_blocks_takes_every_block_of_the_boards_to_the_trash() {
    let test = common::setup().await;
    let workspace_id = common::workspace(&test).await;
    let user_id = common::user(&test, "user").await;

    let board_id = insert_new_board(&test, &workspace_id, &user_id).await;
    let listed_id = insert_new_block(&test, &board_id, &user_id).await;
    let unlisted_id = insert_new_block(&test, &board_id, &user_id).await;
    let mut child_of_unlisted = Block {
        id: new_id(),
        container_id: board_id.clone(),
        parent_id: unlisted_id.clone(),
        ..Block::default()
    };
    test.insert_block(&mut child_of_unlisted, &user_id)
        .await
        .expect("the child is inserted");
    let all_block_ids = sorted(&[&listed_id, &unlisted_id, &child_of_unlisted.id]);

    let request = DeleteContainersAndBlocks {
        containers: vec![board_id.clone()],
        blocks: vec![listed_id.clone()],
    };
    test.delete_containers_and_blocks(&request, &user_id)
        .await
        .expect("the board and its blocks are deleted");

    let left = test
        .get_blocks(&BlockFilter::container(&board_id))
        .await
        .expect("the blocks are read");
    assert!(left.is_empty(), "blocks are left: {left:?}");
    for block_id in &all_block_ids {
        test.get_deleted_block(block_id)
            .await
            .unwrap_or_else(|error| panic!("block {block_id} is not in the trash: {error:?}"));
    }

    test.undelete_container(&board_id, &user_id)
        .await
        .expect("the board is restored");
    let back = test
        .get_blocks(&BlockFilter::container(&board_id))
        .await
        .expect("the blocks are read");
    assert_eq!(
        common::sorted_ids(&back),
        all_block_ids,
        "every block is back with the board"
    );
    let child = back
        .iter()
        .find(|block| block.id == child_of_unlisted.id)
        .expect("the child is among the blocks");
    assert_eq!(child.parent_id, unlisted_id, "the child is on its parent");

    test.teardown().await;
}

/// A block in the trash is no block of the board: a request that lists one is refused,
/// and nothing is deleted.
#[tokio::test]
async fn delete_containers_and_blocks_refuses_a_block_that_is_in_the_trash() {
    let test = common::setup().await;
    let workspace_id = common::workspace(&test).await;
    let user_id = common::user(&test, "user").await;

    let board_id = insert_new_board(&test, &workspace_id, &user_id).await;
    let kept_id = insert_new_block(&test, &board_id, &user_id).await;
    let deleted_id = insert_new_block(&test, &board_id, &user_id).await;
    test.delete_block(&deleted_id, &user_id)
        .await
        .expect("the block is deleted");

    let request = DeleteContainersAndBlocks {
        containers: vec![board_id.clone()],
        blocks: vec![kept_id.clone(), deleted_id],
    };
    let error = test
        .delete_containers_and_blocks(&request, &user_id)
        .await
        .expect_err("the second block does not exist");
    assert!(error.is_not_found(), "got {error:?}");
    test.get_container(&board_id)
        .await
        .expect("the board stays");
    test.get_block(&kept_id).await.expect("its block stays");

    test.teardown().await;
}

/// The blocks of one request are stored parents first, wherever in the list each appears,
/// so foreign-key parent checks accept a child listed before its parent.
#[tokio::test]
async fn create_containers_and_blocks_accepts_a_block_listed_before_its_parent() {
    let test = common::setup().await;
    let workspace_id = common::workspace(&test).await;
    let user_id = common::user(&test, "user").await;

    let [board_id, card_id, text_id, checkbox_id] = new_ids();
    let mut new_bab = ContainersAndBlocks {
        containers: vec![new_board(
            &board_id,
            &workspace_id,
            ContainerVisibility::Open,
        )],
        blocks: vec![
            Block {
                parent_id: text_id.clone(),
                block_type: BlockType::Attachment,
                ..new_card(&checkbox_id, &board_id)
            },
            Block {
                parent_id: card_id.clone(),
                ..new_card(&text_id, &board_id)
            },
            new_card(&card_id, &board_id),
        ],
    };
    test.create_containers(
        NewContainers::whole(&mut new_bab).creator(Creator::Nobody),
        &user_id,
    )
    .await
    .map_err(StoreError::from)
    .expect("children listed before their parents are accepted");
    assert_eq!(
        new_bab
            .blocks
            .iter()
            .map(|block| block.id.as_str())
            .collect::<Vec<_>>(),
        [&checkbox_id, &text_id, &card_id],
        "the request keeps its order"
    );

    let stored = test
        .get_blocks(&BlockFilter::container(&board_id))
        .await
        .expect("the blocks are read");
    let mut parents: Vec<(&str, &str)> = stored
        .iter()
        .map(|block| (block.id.as_str(), block.parent_id.as_str()))
        .collect();
    parents.sort_unstable();
    let mut expected = [
        (card_id.as_str(), ""),
        (text_id.as_str(), card_id.as_str()),
        (checkbox_id.as_str(), text_id.as_str()),
    ];
    expected.sort_unstable();
    assert_eq!(parents, expected, "each block is on its parent");

    test.teardown().await;
}

/// A block may be older than the block it is on: one that was moved there. The copies
/// must be stored parent first all the same.
#[tokio::test]
async fn duplicate_container_copies_a_block_that_is_older_than_its_parent() {
    let test = common::setup().await;
    let workspace_id = common::workspace(&test).await;
    let user_id = common::user(&test, "user").await;

    let board_id = insert_new_board(&test, &workspace_id, &user_id).await;
    let text_id = insert_new_block(&test, &board_id, &user_id).await;
    let card_id = insert_new_block(&test, &board_id, &user_id).await;
    let onto_the_card = BlockPatch {
        parent_id: Some(card_id.clone()),
        ..BlockPatch::default()
    };
    test.patch_block(&text_id, &onto_the_card, &user_id)
        .await
        .expect("the text is moved onto the card");
    let source = test
        .get_blocks(&BlockFilter::container(&board_id))
        .await
        .expect("the blocks are read");
    assert_eq!(
        source.iter().map(|block| &block.id).collect::<Vec<_>>(),
        [&text_id, &card_id],
        "the board lists the text before the card it is on"
    );

    common::works_in(&test, &workspace_id, &user_id).await;
    let (copy, _) = test
        .duplicate_container(
            &board_id,
            &user_id,
            None,
            DuplicateAs::Copy,
            &TITLES,
            CopyChanges::default(),
        )
        .await
        .expect("the board is duplicated");
    let stored = test
        .get_blocks(&BlockFilter::container(&copy.containers[0].id))
        .await
        .expect("the blocks of the copy are read");
    assert_eq!(stored.len(), 2, "both blocks are copied: {stored:?}");
    let card = stored
        .iter()
        .find(|block| block.parent_id.is_empty())
        .expect("the copy of the card is directly on the board");
    let paragraph = stored
        .iter()
        .find(|block| !block.parent_id.is_empty())
        .expect("the copy of the text is on a block");
    assert_eq!(paragraph.parent_id, card.id, "the text follows its card");

    test.teardown().await;
}
