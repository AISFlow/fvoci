//! Card hierarchy rules in the store: depth limit 3, cycle detection, parent validity,
//! template card constraints, trash/restore/purge cascades, and duplication remapping.

use std::collections::HashMap;

use fvoci_model::DuplicateAs;
use fvoci_model::{
    Block, BlockError, BlockPatch, BlockType, Body, MAX_CARD_DEPTH, Subscription, new_id,
};
use fvoci_store::{BlockFilter, CopyChanges, CopyTitles, Store, StoreError};
use serde_json::json;

use crate::common;

/// A board with one user, returning `(store, user_id, workspace_id, board_id)`.
async fn board_fixture() -> (common::TestStore, String, String, String) {
    let test = common::setup().await;
    let user_id = common::user(&test, "hier-user").await;
    let workspace_id = common::workspace_of(&test, &[&user_id]).await;
    let boards = common::create_test_boards(&test, &workspace_id, &user_id, 1).await;
    let board_id = boards[0].id.clone();
    (test, user_id, workspace_id, board_id)
}

fn card(board_id: &str, parent_id: &str, title: &str) -> Block {
    Block {
        id: new_id(),
        container_id: board_id.to_owned(),
        parent_id: parent_id.to_owned(),
        block_type: BlockType::Card,
        title: title.to_owned(),
        ..Block::default()
    }
}

fn template_card(board_id: &str, parent_id: &str, title: &str) -> Block {
    let mut block = card(board_id, parent_id, title);
    block.fields.insert("isTemplate".to_owned(), json!(true));
    block
}

fn block_of(board_id: &str, parent_id: &str, block_type: BlockType, title: &str) -> Block {
    Block {
        id: new_id(),
        container_id: board_id.to_owned(),
        parent_id: parent_id.to_owned(),
        block_type,
        title: title.to_owned(),
        ..Block::default()
    }
}

async fn save(store: &Store, mut block: Block, user_id: &str) -> Block {
    store
        .insert_block(&mut block, user_id)
        .await
        .expect("block stored");
    block
}

fn reparent_patch(parent_id: &str) -> BlockPatch {
    BlockPatch {
        parent_id: Some(parent_id.to_owned()),
        ..BlockPatch::default()
    }
}

fn body_doc(text: &str) -> Body {
    Body::from_stored(json!({
        "type": "doc",
        "content": [{"type": "paragraph", "content": [{"type": "text", "text": text}]}]
    }))
}

#[track_caller]
fn assert_block_error(result: Result<(), StoreError>, expected: &BlockError) {
    match result {
        Err(StoreError::InvalidBlock { source, .. }) => {
            assert_eq!(&source, expected, "unexpected block error variant");
        }
        other => panic!("expected InvalidBlock({expected:?}), got {other:?}"),
    }
}

#[tokio::test]
async fn depths_1_2_3_allowed_and_depth_4_rejected_on_create_and_move() {
    let (test, user_id, _, board_id) = board_fixture().await;
    let store = &test.store;

    // Reverse-order batch insert of a 3-level chain succeeds via parents_first.
    let d1 = card(&board_id, "", "d1");
    let d2 = card(&board_id, &d1.id, "d2");
    let d3 = card(&board_id, &d2.id, "d3");
    let (d1_id, d2_id, d3_id) = (d1.id.clone(), d2.id.clone(), d3.id.clone());
    let mut batch = [d3, d2, d1];
    store
        .insert_blocks(&mut batch, &user_id)
        .await
        .expect("3-level chain in reverse order succeeds");

    // Creating depth 4 under d3 is refused.
    let mut d4 = card(&board_id, &d3_id, "d4");
    let too_deep = BlockError::CardTooDeep {
        limit: MAX_CARD_DEPTH,
    };
    assert_block_error(store.insert_block(&mut d4, &user_id).await, &too_deep);

    // Comments and attachments are still allowed on a depth-3 card.
    let comment = save(
        store,
        block_of(&board_id, &d3_id, BlockType::Comment, "c"),
        &user_id,
    )
    .await;
    let attach = save(
        store,
        block_of(&board_id, &d3_id, BlockType::Attachment, "a"),
        &user_id,
    )
    .await;
    assert_eq!(comment.parent_id, d3_id, "comment parent");
    assert_eq!(attach.parent_id, d3_id, "attachment parent");

    // Leaf card moving under d2 -> depth 3 (allowed); under d3 -> depth 4 (rejected).
    let leaf = save(store, card(&board_id, "", "leaf"), &user_id).await;
    let leaf_comment = save(
        store,
        block_of(&board_id, &leaf.id, BlockType::Comment, "lc"),
        &user_id,
    )
    .await;
    store
        .patch_block(&leaf.id, &reparent_patch(&d2_id), &user_id)
        .await
        .expect("leaf with comment moves to depth 3");
    assert_eq!(
        store
            .get_block(&leaf_comment.id)
            .await
            .expect("comment")
            .parent_id,
        leaf.id,
        "comment stays attached to leaf"
    );
    assert_block_error(
        store
            .patch_block(&leaf.id, &reparent_patch(&d3_id), &user_id)
            .await
            .map(|_| ()),
        &too_deep,
    );

    // Subtree height check: moving height-2 subtree under depth-1 succeeds (child -> depth 3),
    // but moving it under depth-2 fails because child would be depth 4.
    let sub_root = save(store, card(&board_id, "", "sub-root"), &user_id).await;
    let sub_child = save(store, card(&board_id, &sub_root.id, "sub-child"), &user_id).await;
    let other_root = save(store, card(&board_id, "", "other-root"), &user_id).await;
    store
        .patch_block(&sub_root.id, &reparent_patch(&other_root.id), &user_id)
        .await
        .expect("height-2 subtree under depth-1 reaches depth 3");
    assert_block_error(
        store
            .patch_block(&sub_root.id, &reparent_patch(&d2_id), &user_id)
            .await
            .map(|_| ()),
        &too_deep,
    );
    assert_eq!(
        store
            .get_block(&sub_child.id)
            .await
            .expect("sub-child")
            .parent_id,
        sub_root.id,
        "sub-child stays under sub-root"
    );

    // Height-3 subtree (d1 -> d2 -> d3) cannot move under any card, but moving to root is ok.
    assert_block_error(
        store
            .patch_block(&d1_id, &reparent_patch(&other_root.id), &user_id)
            .await
            .map(|_| ()),
        &too_deep,
    );
    store
        .patch_block(&d1_id, &reparent_patch(""), &user_id)
        .await
        .expect("moving to root succeeds");

    test.teardown().await;
}

#[tokio::test]
async fn self_reference_and_cycles_are_rejected() {
    let (test, user_id, _, board_id) = board_fixture().await;
    let store = &test.store;

    // Self-parent on create.
    let mut self_card = card(&board_id, "", "self");
    self_card.parent_id = self_card.id.clone();
    assert_block_error(
        store.insert_block(&mut self_card, &user_id).await,
        &BlockError::ParentBelowBlock,
    );

    // Live 3-node chain: root -> child -> grand.
    let root = save(store, card(&board_id, "", "root"), &user_id).await;
    let child = save(store, card(&board_id, &root.id, "child"), &user_id).await;
    let grand = save(store, card(&board_id, &child.id, "grand"), &user_id).await;

    // Self, 2-node cycle (root -> child), and 3-node cycle (root -> grand).
    for target in [&root.id, &child.id, &grand.id] {
        assert_block_error(
            store
                .patch_block(&root.id, &reparent_patch(target), &user_id)
                .await
                .map(|_| ()),
            &BlockError::ParentBelowBlock,
        );
    }

    // Batch insert with brand-new 2-node and 3-node cycles stores nothing.
    let mut first = card(&board_id, "", "first");
    let mut second = card(&board_id, "", "second");
    first.parent_id = second.id.clone();
    second.parent_id = first.id.clone();
    let first_id = first.id.clone();
    assert_block_error(
        store.insert_blocks(&mut [first, second], &user_id).await,
        &BlockError::ParentBelowBlock,
    );
    assert!(
        store.get_block(&first_id).await.is_err(),
        "rejected 2-cycle stores nothing"
    );

    let mut node1 = card(&board_id, "", "node1");
    let mut node2 = card(&board_id, "", "node2");
    let mut node3 = card(&board_id, "", "node3");
    node1.parent_id = node3.id.clone();
    node2.parent_id = node1.id.clone();
    node3.parent_id = node2.id.clone();
    let node1_id = node1.id.clone();
    assert_block_error(
        store
            .insert_blocks(&mut [node1, node2, node3], &user_id)
            .await,
        &BlockError::ParentBelowBlock,
    );
    assert!(
        store.get_block(&node1_id).await.is_err(),
        "rejected 3-cycle stores nothing"
    );

    test.teardown().await;
}

#[tokio::test]
async fn invalid_parents_and_template_constraints_are_rejected() {
    let (test, user_id, workspace_id, board_id) = board_fixture().await;
    let store = &test.store;
    let other_board = common::create_test_boards(&test, &workspace_id, &user_id, 1).await[0]
        .id
        .clone();

    let other_card = save(store, card(&other_board, "", "foreign"), &user_id).await;
    let root = save(store, card(&board_id, "", "root"), &user_id).await;
    let child = save(store, card(&board_id, &root.id, "child"), &user_id).await;
    let view = save(
        store,
        block_of(&board_id, "", BlockType::View, "v"),
        &user_id,
    )
    .await;
    let attach = save(
        store,
        block_of(&board_id, &root.id, BlockType::Attachment, "att"),
        &user_id,
    )
    .await;
    let comment = save(
        store,
        block_of(&board_id, &root.id, BlockType::Comment, "cm"),
        &user_id,
    )
    .await;
    let tmpl = save(store, template_card(&board_id, "", "tmpl"), &user_id).await;
    let trashed = save(store, card(&board_id, "", "trashed"), &user_id).await;
    store
        .delete_block(&trashed.id, &user_id)
        .await
        .expect("trashed");

    let missing_id = new_id();
    for (parent_id, expected) in [
        (missing_id.as_str(), BlockError::ParentNotOnContainer),
        (other_card.id.as_str(), BlockError::ParentNotOnContainer),
        (trashed.id.as_str(), BlockError::ParentNotOnContainer),
        (view.id.as_str(), BlockError::CardNotBelowCard),
        (attach.id.as_str(), BlockError::CardNotBelowCard),
        (comment.id.as_str(), BlockError::ParentIsComment),
        (tmpl.id.as_str(), BlockError::CardBelowTemplate),
    ] {
        let mut candidate = card(&board_id, parent_id, "bad");
        assert_block_error(
            store.insert_block(&mut candidate, &user_id).await,
            &expected,
        );
    }

    // Template card cannot have a parent on create or via patch.
    let mut bad_tmpl = template_card(&board_id, &root.id, "bad-tmpl");
    assert_block_error(
        store.insert_block(&mut bad_tmpl, &user_id).await,
        &BlockError::TemplateCardBelowBlock,
    );
    let make_template = BlockPatch {
        updated_fields: serde_json::from_value(json!({ "isTemplate": true })).expect("map"),
        ..BlockPatch::default()
    };
    assert_block_error(
        store
            .patch_block(&child.id, &make_template, &user_id)
            .await
            .map(|_| ()),
        &BlockError::TemplateCardBelowBlock,
    );

    // Card with sub-cards cannot become a template.
    let parent_only = save(store, card(&board_id, "", "parent-only"), &user_id).await;
    let _sub = save(store, card(&board_id, &parent_only.id, "sub"), &user_id).await;
    assert_block_error(
        store
            .patch_block(&parent_only.id, &make_template, &user_id)
            .await
            .map(|_| ()),
        &BlockError::TemplateCardHasChildren,
    );

    test.teardown().await;
}

#[tokio::test]
async fn trash_and_restore_handle_card_hierarchies() {
    let (test, user_id, _, board_id) = board_fixture().await;
    let store = &test.store;

    let mut d1 = card(&board_id, "", "d1");
    d1.body = Some(body_doc("body-1"));
    let d1 = save(store, d1, &user_id).await;
    let mut d2 = card(&board_id, &d1.id, "d2");
    d2.body = Some(body_doc("body-2"));
    let d2 = save(store, d2, &user_id).await;
    let d3 = save(store, card(&board_id, &d2.id, "d3"), &user_id).await;
    let cm = save(
        store,
        block_of(&board_id, &d3.id, BlockType::Comment, "c3"),
        &user_id,
    )
    .await;
    let att = save(
        store,
        block_of(&board_id, &d2.id, BlockType::Attachment, "a2"),
        &user_id,
    )
    .await;
    store
        .create_subscription(Subscription {
            block_type: BlockType::Card,
            block_id: d2.id.clone(),
            user_id: user_id.clone(),
            notified_at: 0,
            created_at: 0,
            deleted_at: None,
        })
        .await
        .expect("subscribed to d2");

    // Deleting d1 moves d1, d2, d3, comment, attachment, and d2's subscription into trash.
    let deleted = store
        .delete_block_family(&d1.id, &user_id)
        .await
        .expect("deleted family");
    assert_eq!(deleted.len(), 5, "entire family moved to trash");
    assert_eq!(
        store
            .get_blocks(&BlockFilter::container(&board_id))
            .await
            .expect("blocks"),
        Vec::new(),
        "no live blocks remain on board"
    );
    let listed = store
        .get_deleted_cards(&board_id, None)
        .await
        .expect("deleted cards");
    assert_eq!(listed.len(), 1, "only top card of deletion is listed");
    assert_eq!(listed[0].block.id, d1.id, "top deleted card is d1");

    // Restoring d1 restores the full hierarchy and subscriptions intact.
    let restored = store
        .undelete_block_family(&d1.id, &user_id)
        .await
        .expect("restored family");
    assert_eq!(restored.len(), 5, "entire family restored");
    assert_eq!(
        store.get_block(&d2.id).await.expect("d2").parent_id,
        d1.id,
        "d2 parent preserved"
    );
    assert_eq!(
        store.get_block(&d3.id).await.expect("d3").parent_id,
        d2.id,
        "d3 parent preserved"
    );
    assert_eq!(
        store.get_block(&cm.id).await.expect("cm").parent_id,
        d3.id,
        "comment parent preserved"
    );
    assert_eq!(
        store.get_block(&att.id).await.expect("att").parent_id,
        d2.id,
        "attachment parent preserved"
    );
    let subs = store.get_subscriptions(&user_id).await.expect("subs");
    assert!(
        subs.iter().any(|sub| sub.block_id == d2.id),
        "subscription restored with sub-card"
    );

    // Delete child d2 (with d3) first, then delete d1; restoring d2 re-roots d2 to ""
    // while preserving d3 below d2.
    store.delete_block(&d2.id, &user_id).await.expect("del d2");
    store.delete_block(&d1.id, &user_id).await.expect("del d1");
    let back_d2 = store
        .undelete_block(&d2.id, &user_id)
        .await
        .expect("d2 re-rooted");
    assert_eq!(back_d2.parent_id, "", "orphaned sub-card re-roots to top");
    assert_eq!(
        store.get_block(&d3.id).await.expect("d3").parent_id,
        d2.id,
        "grandchild remains under re-rooted parent"
    );

    test.teardown().await;
}

#[tokio::test]
async fn restore_refusals_and_purge_handle_card_hierarchies() {
    let (test, user_id, _, board_id) = board_fixture().await;
    let store = &test.store;

    let d1 = save(store, card(&board_id, "", "d1"), &user_id).await;
    let mut d2 = card(&board_id, &d1.id, "d2");
    d2.body = Some(body_doc("body-2"));
    let d2 = save(store, d2, &user_id).await;
    let d3 = save(store, card(&board_id, &d2.id, "d3"), &user_id).await;
    store
        .create_subscription(Subscription {
            block_type: BlockType::Card,
            block_id: d2.id.clone(),
            user_id: user_id.clone(),
            notified_at: 0,
            created_at: 0,
            deleted_at: None,
        })
        .await
        .expect("subscribed to d2");

    // Delete d2 (height 2), move d1 under another root (d1 becomes depth 2)
    // -> restoring d2 must fail with NotRestorable(CardTooDeep).
    store.delete_block(&d2.id, &user_id).await.expect("del d2");
    let other_root = save(store, card(&board_id, "", "other"), &user_id).await;
    store
        .patch_block(&d1.id, &reparent_patch(&other_root.id), &user_id)
        .await
        .expect("d1 now depth 2");
    let err = store
        .undelete_block(&d2.id, &user_id)
        .await
        .expect_err("depth 2 + height 2 = 4 must not restore");
    assert!(
        matches!(
            err,
            StoreError::NotRestorable {
                source: BlockError::CardTooDeep {
                    limit: MAX_CARD_DEPTH
                },
                ..
            }
        ),
        "got {err:?}"
    );

    // Move d1 back to root, make d1 a template -> restoring d2 fails with CardBelowTemplate.
    store
        .patch_block(&d1.id, &reparent_patch(""), &user_id)
        .await
        .expect("d1 root");
    let make_template = BlockPatch {
        updated_fields: serde_json::from_value(json!({ "isTemplate": true })).expect("map"),
        ..BlockPatch::default()
    };
    store
        .patch_block(&d1.id, &make_template, &user_id)
        .await
        .expect("d1 template");
    let err = store
        .undelete_block(&d2.id, &user_id)
        .await
        .expect_err("cannot restore under template card");
    assert!(
        matches!(
            err,
            StoreError::NotRestorable {
                source: BlockError::CardBelowTemplate,
                ..
            }
        ),
        "got {err:?}"
    );

    // Purge clears trashed sub-cards, body versions, and subscriptions.
    store.purge_trash(i64::MAX).await.expect("purged");
    assert!(
        store.get_deleted_block(&d2.id).await.is_err(),
        "d2 purged from trash"
    );
    assert!(
        store.get_deleted_block(&d3.id).await.is_err(),
        "d3 purged from trash"
    );
    assert!(
        store
            .get_subscriptions(&user_id)
            .await
            .expect("subs")
            .iter()
            .all(|sub| sub.block_id != d2.id),
        "subscription purged"
    );
    assert_eq!(
        store.body_versions(&d2.id).await.expect("versions"),
        Vec::new(),
        "body versions purged"
    );

    test.teardown().await;
}

#[tokio::test]
async fn duplicate_container_remaps_card_hierarchy() {
    let (test, user_id, _workspace_id, board_id) = board_fixture().await;
    let store = &test.store;

    // Create child BEFORE parent, then reparent child under parent so oldest_first puts child first.
    let child = save(store, card(&board_id, "", "child"), &user_id).await;
    let parent = save(store, card(&board_id, "", "parent"), &user_id).await;
    store
        .patch_block(&child.id, &reparent_patch(&parent.id), &user_id)
        .await
        .expect("reparented");
    let grand = save(store, card(&board_id, &child.id, "grand"), &user_id).await;

    let titles = CopyTitles {
        copy_of: |title| format!("{title} copy"),
        new_template: "Template",
    };
    let (copied, _) = store
        .duplicate_container(
            &board_id,
            &user_id,
            None,
            DuplicateAs::Copy,
            &titles,
            CopyChanges::default(),
        )
        .await
        .expect("board duplicated");
    let by_title: HashMap<&str, &Block> = copied
        .blocks
        .iter()
        .map(|block| (block.title.as_str(), block))
        .collect();
    assert_eq!(
        by_title["parent"].parent_id, "",
        "copied root has no parent"
    );
    assert_eq!(
        by_title["child"].parent_id, by_title["parent"].id,
        "copied child points to copied parent"
    );
    assert_eq!(
        by_title["grand"].parent_id, by_title["child"].id,
        "copied grand points to copied child"
    );
    assert_ne!(by_title["parent"].id, parent.id, "parent got new id");
    assert_ne!(by_title["child"].id, child.id, "child got new id");
    assert_ne!(by_title["grand"].id, grand.id, "grand got new id");

    test.teardown().await;
}

#[tokio::test]
async fn duplicate_block_preserves_subcards_and_strips_for_template() {
    let (test, user_id, _workspace_id, board_id) = board_fixture().await;
    let store = &test.store;

    let parent = save(store, card(&board_id, "", "parent"), &user_id).await;
    let child = save(store, card(&board_id, &parent.id, "child"), &user_id).await;
    let grand = save(store, card(&board_id, &child.id, "grand"), &user_id).await;
    let child_att = save(
        store,
        block_of(&board_id, &child.id, BlockType::Attachment, "att"),
        &user_id,
    )
    .await;
    let grand_att = save(
        store,
        block_of(&board_id, &grand.id, BlockType::Attachment, "grand-att"),
        &user_id,
    )
    .await;

    // Duplicating a depth-1 card as Copy preserves all 3 levels and sub-card attachments.
    let dup_root =
        common::duplicate_card(store, &board_id, &parent.id, &user_id, DuplicateAs::Copy)
            .await
            .expect("root card duplicated as copy");
    let dup_by_title: HashMap<&str, &Block> = dup_root
        .iter()
        .map(|block| (block.title.as_str(), block))
        .collect();
    assert_eq!(dup_root.len(), 5, "root + child + grand + both attachments");
    assert_eq!(dup_by_title["parent"].parent_id, "", "root copy stays root");
    assert_eq!(
        dup_by_title["child"].parent_id, dup_by_title["parent"].id,
        "child copy follows root copy"
    );
    assert_eq!(
        dup_by_title["grand"].parent_id, dup_by_title["child"].id,
        "grand copy follows child copy"
    );
    assert_eq!(
        dup_by_title["grand-att"].parent_id, dup_by_title["grand"].id,
        "grand attachment follows grand copy"
    );

    // Duplicating a depth-2 card as Copy stays a sibling under parent and keeps grand + attachments.
    let dup_copy = common::duplicate_card(store, &board_id, &child.id, &user_id, DuplicateAs::Copy)
        .await
        .expect("card duplicated as copy");
    assert_eq!(
        dup_copy[0].parent_id, parent.id,
        "copy stays sibling under same parent"
    );
    assert_eq!(
        dup_copy.len(),
        4,
        "child copy + grand copy + child att + grand att"
    );

    // Duplicating as Template clears parent_id and omits descendant cards and their attachments.
    let dup_tmpl =
        common::duplicate_card(store, &board_id, &child.id, &user_id, DuplicateAs::Template)
            .await
            .expect("card duplicated as template");
    assert_eq!(dup_tmpl[0].parent_id, "", "template has no parent");
    assert!(dup_tmpl[0].is_template(), "duplicated as template");
    assert_eq!(dup_tmpl.len(), 2, "template + own attachment only");
    assert_eq!(
        dup_tmpl[1].block_type,
        BlockType::Attachment,
        "only non-card attachment copied"
    );
    assert_eq!(
        dup_tmpl[1].parent_id, dup_tmpl[0].id,
        "attachment points to template"
    );
    assert_eq!(child_att.parent_id, child.id, "original attachment intact");
    assert_eq!(
        grand_att.parent_id, grand.id,
        "original grand attachment intact"
    );

    test.teardown().await;
}

/// `Store::duplicate_block` remaps intra-subtree Gantt relation IDs between copied parent
/// and sub-cards to the newly generated copy IDs while keeping relations pointing to
/// external cards on the same board.
#[tokio::test]
async fn duplicate_block_remaps_subcard_gantt_relations_and_keeps_external_cards() {
    use fvoci_model::ContainerPatch;

    let (test, user_id, _workspace_id, board_id) = board_fixture().await;
    let store = &test.store;

    let prop_id = "dep_rel";
    let def = fvoci_model::CardPropertyDefinition::new(
        prop_id,
        "Depends On",
        fvoci_model::PropertyType::Relation,
    );
    store
        .patch_container(
            &board_id,
            &ContainerPatch {
                updated_card_properties: vec![def],
                ..ContainerPatch::default()
            },
            &user_id,
        )
        .await
        .expect("relation property added");

    let external = save(store, card(&board_id, "", "external"), &user_id).await;
    let parent = save(store, card(&board_id, "", "parent"), &user_id).await;
    let c1 = save(store, card(&board_id, &parent.id, "c1"), &user_id).await;
    let c2 = save(store, card(&board_id, &parent.id, "c2"), &user_id).await;

    let set_rel = |targets: &[&str]| {
        let links: Vec<serde_json::Value> = targets
            .iter()
            .map(|target| json!({ "card": target }))
            .collect();
        BlockPatch {
            updated_fields: serde_json::from_value(json!({
                "properties": { prop_id: links }
            }))
            .expect("patch fields"),
            ..BlockPatch::default()
        }
    };
    store
        .patch_block(&c1.id, &set_rel(&[&c2.id, &external.id]), &user_id)
        .await
        .expect("c1 relates to sibling c2 and external");
    store
        .patch_block(&c2.id, &set_rel(&[&parent.id]), &user_id)
        .await
        .expect("c2 relates to parent");

    let copies = common::duplicate_card(store, &board_id, &parent.id, &user_id, DuplicateAs::Copy)
        .await
        .expect("duplicated subtree");
    let by_title: HashMap<&str, &Block> = copies
        .iter()
        .map(|block| (block.title.as_str(), block))
        .collect();
    let dup_parent = by_title["parent"];
    let dup_c1 = by_title["c1"];
    let dup_c2 = by_title["c2"];

    assert_eq!(
        dup_c1.fields["properties"][prop_id],
        json!([
            {"card": dup_c2.id, "kind": "fs", "lagDays": 0},
            {"card": external.id, "kind": "fs", "lagDays": 0}
        ]),
        "c1 copy points to copied sibling c2 and preserves external card"
    );
    assert_eq!(
        dup_c2.fields["properties"][prop_id],
        json!([{"card": dup_parent.id, "kind": "fs", "lagDays": 0}]),
        "c2 copy points to copied parent"
    );

    test.teardown().await;
}
