//! Store tests for courses: chapter/unit hierarchy, fractional ordering, moves, trash
//! and restore, independent chapter (200) and unit (2,000) limits, and concurrent writes.

use fvoci_model::{
    Block, BlockError, BlockType, Body, CHAPTERS_PER_COURSE, Container, ContainerKind,
    ContainerVisibility, CourseLimits, NewUnit, UNITS_PER_COURSE, UnitKind, UnitMove, new_id,
};
use fvoci_store::{DbType, Store, StoreError, Written};
use serde_json::json;

use crate::boards::new_board;
use crate::common::{self, db_type};

async fn a_course(store: &Store, workspace_id: &str, user_id: &str) -> Container {
    let mut course = Container {
        kind: ContainerKind::Course,
        title: "Course".to_owned(),
        ..new_board(&new_id(), workspace_id, ContainerVisibility::Private)
    };
    store
        .insert_container_with_admin(&mut course, user_id)
        .await
        .expect("course stored");
    course
}

async fn add_item(
    store: &Store,
    course_id: &str,
    user_id: &str,
    kind: UnitKind,
    parent_id: &str,
    title: &str,
    after: Option<&str>,
) -> Block {
    let new = NewUnit {
        kind,
        parent_id: parent_id.to_owned(),
        after: after.map(str::to_owned),
        title: title.to_owned(),
        ..NewUnit::default()
    };
    store
        .create_unit(course_id, &new, CourseLimits::DEFAULT, user_id)
        .await
        .expect("unit or chapter stored")
        .first
}

/// The identifiers of the blocks that came back from the trash, the asked-for block first.
fn came_back(restored: &Written) -> Vec<&str> {
    std::iter::once(&restored.first)
        .chain(&restored.below)
        .map(|block| block.id.as_str())
        .collect()
}

fn outline_titles(nodes: &[fvoci_model::UnitNode]) -> Vec<(&str, UnitKind, &str)> {
    nodes
        .iter()
        .map(|node| (node.title.as_str(), node.kind, node.parent_id.as_str()))
        .collect()
}

fn block_error<T: std::fmt::Debug>(result: Result<T, StoreError>) -> BlockError {
    match result {
        Err(StoreError::InvalidBlock { source, .. }) => source,
        other => panic!("expected StoreError::InvalidBlock, got {other:?}"),
    }
}

#[tokio::test]
async fn outline_ordering_chapters_and_type_rules_are_enforced() {
    let test = common::setup().await;
    let user_id = common::user(&test, "author").await;
    let workspace_id = common::workspace_of(&test, &[&user_id]).await;
    let course = a_course(&test, &workspace_id, &user_id).await;

    let add = async |kind, parent: &str, title: &str, after: Option<&str>| {
        add_item(&test, &course.id, &user_id, kind, parent, title, after).await
    };
    let ch1 = add(UnitKind::Chapter, "", "Ch 1", None).await;
    let ch2 = add(UnitKind::Chapter, "", "Ch 2", None).await;
    let ch0 = add(UnitKind::Chapter, "", "Ch 0", Some("")).await;
    let u_top = add(UnitKind::Unit, "", "Intro", Some(&ch0.id)).await;
    let u1 = add(UnitKind::Unit, &ch1.id, "U1", None).await;
    let u3 = add(UnitKind::Unit, &ch1.id, "U3", None).await;
    let u2 = add(UnitKind::Unit, &ch1.id, "U2", Some(&u1.id)).await;

    let tree = test.get_unit_tree(&course.id).await.expect("unit tree");
    let top: Vec<(&str, UnitKind)> = tree
        .iter()
        .filter(|node| node.parent_id.is_empty())
        .map(|node| (node.title.as_str(), node.kind))
        .collect();
    assert_eq!(
        top,
        [
            ("Ch 0", UnitKind::Chapter),
            ("Intro", UnitKind::Unit),
            ("Ch 1", UnitKind::Chapter),
            ("Ch 2", UnitKind::Chapter),
        ]
    );
    let in_ch1: Vec<&str> = tree
        .iter()
        .filter(|node| node.parent_id == ch1.id)
        .map(|node| node.title.as_str())
        .collect();
    assert_eq!(in_ch1, ["U1", "U2", "U3"]);
    assert_eq!(
        (u2.parent_id.as_str(), u3.parent_id.as_str()),
        (ch1.id.as_str(), ch1.id.as_str())
    );
    let _ = (ch2, u_top);
    test.teardown().await;
}

#[tokio::test]
async fn course_and_block_type_rules_are_enforced() {
    let test = common::setup().await;
    let user_id = common::user(&test, "author").await;
    let workspace_id = common::workspace_of(&test, &[&user_id]).await;
    let course = a_course(&test, &workspace_id, &user_id).await;
    let add = async |kind, parent: &str, title: &str| {
        add_item(&test, &course.id, &user_id, kind, parent, title, None).await
    };
    let ch1 = add(UnitKind::Chapter, "", "Ch 1").await;
    let u_top = add(UnitKind::Unit, "", "Top").await;
    let u1 = add(UnitKind::Unit, &ch1.id, "U1").await;

    // Chapter below chapter or unit is refused.
    for parent in [&ch1.id, &u_top.id] {
        let bad = NewUnit {
            kind: UnitKind::Chapter,
            parent_id: parent.clone(),
            ..NewUnit::default()
        };
        assert_eq!(
            block_error(
                test.create_unit(&course.id, &bad, CourseLimits::DEFAULT, &user_id)
                    .await
            ),
            BlockError::ChapterBelowBlock
        );
    }

    // Unit below unit is refused (max depth 2: chapter -> unit).
    let below_unit = NewUnit {
        kind: UnitKind::Unit,
        parent_id: u1.id.clone(),
        ..NewUnit::default()
    };
    assert_eq!(
        block_error(
            test.create_unit(&course.id, &below_unit, CourseLimits::DEFAULT, &user_id)
                .await
        ),
        BlockError::UnitParentNotChapter
    );

    // Chapter with body is refused; non-unit/chapter blocks on a course are refused;
    // unit/chapter off a course are refused.
    let mut ch_body = Block {
        id: new_id(),
        container_id: course.id.clone(),
        block_type: BlockType::Chapter,
        title: "No body".to_owned(),
        body: Some(Body::from_stored(json!({"type": "doc", "content": []}))),
        ..Block::default()
    };
    assert_eq!(
        block_error(test.insert_block(&mut ch_body, &user_id).await),
        BlockError::BodyNotAllowed
    );

    for bad_type in [BlockType::Card, BlockType::Page, BlockType::Entry] {
        let mut fields = fvoci_model::JsonMap::new();
        let title = if bad_type == BlockType::Entry {
            fields.insert("date".to_owned(), json!("2026-11-22"));
            "2026-11-22".to_owned()
        } else {
            "wrong".to_owned()
        };
        let mut wrong = Block {
            id: new_id(),
            container_id: course.id.clone(),
            block_type: bad_type,
            title,
            fields,
            ..Block::default()
        };
        assert_eq!(
            block_error(test.insert_block(&mut wrong, &user_id).await),
            BlockError::NotUnitOnCourse
        );
    }

    let mut board = new_board(&new_id(), &workspace_id, ContainerVisibility::Private);
    test.insert_container_with_admin(&mut board, &user_id)
        .await
        .expect("board");
    for block_type in [BlockType::Unit, BlockType::Chapter] {
        let mut off = Block {
            id: new_id(),
            container_id: board.id.clone(),
            block_type,
            title: "off".to_owned(),
            ..Block::default()
        };
        assert_eq!(
            block_error(test.insert_block(&mut off, &user_id).await),
            BlockError::UnitOffCourse
        );
    }

    test.teardown().await;
}

#[tokio::test]
async fn course_comments_restore_order_and_sharing_rules_hold() {
    let test = common::setup().await;
    let user_id = common::user(&test, "author").await;
    let workspace_id = common::workspace_of(&test, &[&user_id]).await;
    let course = a_course(&test, &workspace_id, &user_id).await;
    let ch1 = add_item(
        &test,
        &course.id,
        &user_id,
        UnitKind::Chapter,
        "",
        "Ch 1",
        None,
    )
    .await;
    let u1 = add_item(
        &test,
        &course.id,
        &user_id,
        UnitKind::Unit,
        &ch1.id,
        "U1",
        None,
    )
    .await;

    // Comments are allowed on units, and refused at the top of a course or on a chapter.
    for bad_parent in ["", ch1.id.as_str()] {
        let mut bad_comment = Block {
            id: new_id(),
            container_id: course.id.clone(),
            parent_id: bad_parent.to_owned(),
            block_type: BlockType::Comment,
            ..Block::default()
        };
        assert_eq!(
            block_error(test.insert_block(&mut bad_comment, &user_id).await),
            BlockError::CommentNotOnCardOrPage
        );
    }
    let mut unit_comment = Block {
        id: new_id(),
        container_id: course.id.clone(),
        parent_id: u1.id.clone(),
        block_type: BlockType::Comment,
        ..Block::default()
    };
    test.insert_block(&mut unit_comment, &user_id)
        .await
        .expect("comment on a unit is stored");

    // Deleting and undeleting a chapter with a commented unit restores chapter -> unit -> comment
    // and excludes the comment from the chapter's `below` count.
    test.delete_block(&ch1.id, &user_id)
        .await
        .expect("delete chapter with commented unit");
    let trashed = test
        .get_deleted_units(&course.id, None)
        .await
        .expect("deleted units");
    assert_eq!(trashed.len(), 1);
    assert_eq!(trashed[0].below, 1, "comment is not counted in below");
    let restored = test
        .undelete_unit(&course.id, &ch1.id, CourseLimits::DEFAULT, &user_id)
        .await
        .expect("undelete chapter with commented unit");
    let restored_ids = came_back(&restored);
    assert_eq!(
        restored_ids,
        [ch1.id.as_str(), u1.id.as_str(), unit_comment.id.as_str()],
        "restored in parent-before-child order"
    );

    // Sharing is allowed on courses (workspace and own space) and still refused on diaries.
    let shared = test
        .set_public_link(
            &course.id,
            fvoci_model::PublicLinkChange::On,
            "course-share-token",
            &user_id,
        )
        .await
        .expect("course sharing succeeds");
    assert!(shared.enabled && !shared.token.is_empty());
    let (own, own_ws) = common::in_own_space(&test, &user_id).await;
    let diary = own
        .create_diary(
            crate::diaries::new_diary(&own_ws),
            &user_id,
            fvoci_model::DiaryMaking::Another,
        )
        .await
        .expect("diary stored in own space")
        .diary;
    let diary_share = own
        .set_public_link(
            &diary.id,
            fvoci_model::PublicLinkChange::On,
            "diary-share-token",
            &user_id,
        )
        .await;
    assert!(
        matches!(diary_share, Err(StoreError::NotFound { .. })),
        "diary sharing stays refused: {diary_share:?}"
    );
    test.teardown().await;
}

#[tokio::test]
async fn moving_units_and_chapters_preserves_children_and_refuses_bad_targets() {
    let test = common::setup().await;
    let user_id = common::user(&test, "author").await;
    let workspace_id = common::workspace_of(&test, &[&user_id]).await;
    let course = a_course(&test, &workspace_id, &user_id).await;

    let add = async |kind, parent: &str, title: &str| {
        add_item(&test, &course.id, &user_id, kind, parent, title, None).await
    };
    let ch1 = add(UnitKind::Chapter, "", "Ch 1").await;
    let ch2 = add(UnitKind::Chapter, "", "Ch 2").await;
    let u1 = add(UnitKind::Unit, &ch1.id, "U1").await;
    let u2 = add(UnitKind::Unit, &ch1.id, "U2").await;

    // Move U1 into Ch 2, then move Ch 1 after Ch 2: U2 stays inside Ch 1.
    let _ = test
        .move_unit(
            &course.id,
            &u1.id,
            &UnitMove {
                parent_id: ch2.id.clone(),
                after: None,
            },
            &user_id,
        )
        .await
        .expect("move unit into ch2");
    let _ = test
        .move_unit(
            &course.id,
            &ch1.id,
            &UnitMove {
                parent_id: String::new(),
                after: Some(ch2.id.clone()),
            },
            &user_id,
        )
        .await
        .expect("move ch1 after ch2");

    let tree = test.get_unit_tree(&course.id).await.expect("tree");
    let top: Vec<&str> = tree
        .iter()
        .filter(|n| n.parent_id.is_empty())
        .map(|n| n.title.as_str())
        .collect();
    assert_eq!(top, ["Ch 2", "Ch 1"]);
    assert_eq!(
        tree.iter()
            .find(|n| n.id == u1.id)
            .map(|n| n.parent_id.as_str()),
        Some(ch2.id.as_str())
    );
    assert_eq!(
        tree.iter()
            .find(|n| n.id == u2.id)
            .map(|n| n.parent_id.as_str()),
        Some(ch1.id.as_str())
    );

    // Move U2 to top level.
    let _ = test
        .move_unit(
            &course.id,
            &u2.id,
            &UnitMove {
                parent_id: String::new(),
                after: Some(String::new()),
            },
            &user_id,
        )
        .await
        .expect("move u2 to top");
    let tree = test.get_unit_tree(&course.id).await.expect("tree");
    assert_eq!(
        outline_titles(&tree)
            .into_iter()
            .filter(|(_, _, parent)| parent.is_empty())
            .map(|(title, _, _)| title)
            .collect::<Vec<_>>(),
        ["U2", "Ch 2", "Ch 1"]
    );

    assert_bad_moves_refused(
        &test, &course.id, &user_id, &ch1.id, &ch2.id, &u1.id, &u2.id,
    )
    .await;
    test.teardown().await;
}

async fn assert_bad_moves_refused(
    test: &Store,
    course_id: &str,
    user_id: &str,
    ch1: &str,
    ch2: &str,
    u1: &str,
    u2: &str,
) {
    let missing = new_id();
    for (moving, target, expected) in [
        (ch1, ch2, BlockError::ChapterBelowBlock),
        (ch1, u2, BlockError::ChapterBelowBlock),
        (u1, u2, BlockError::UnitParentNotChapter),
        (u1, missing.as_str(), BlockError::ParentNotOnContainer),
    ] {
        let mv = UnitMove {
            parent_id: target.to_owned(),
            after: None,
        };
        assert_eq!(
            block_error(test.move_unit(course_id, moving, &mv, user_id).await),
            expected,
            "move {moving} under {target}"
        );
    }
}

#[tokio::test]
async fn deleting_and_undeleting_chapters_and_orphaned_units_behave_consistently() {
    let test = common::setup().await;
    let user_id = common::user(&test, "author").await;
    let workspace_id = common::workspace_of(&test, &[&user_id]).await;
    let course = a_course(&test, &workspace_id, &user_id).await;

    let ch = add_item(
        &test,
        &course.id,
        &user_id,
        UnitKind::Chapter,
        "",
        "Ch 1",
        None,
    )
    .await;
    let u1 = add_item(
        &test,
        &course.id,
        &user_id,
        UnitKind::Unit,
        &ch.id,
        "U1",
        None,
    )
    .await;
    let u2 = add_item(
        &test,
        &course.id,
        &user_id,
        UnitKind::Unit,
        &ch.id,
        "U2",
        None,
    )
    .await;

    // Delete chapter: takes both child units in one deletion.
    test.delete_block(&ch.id, &user_id)
        .await
        .expect("chapter deleted");
    assert_eq!(
        test.get_unit_tree(&course.id).await.expect("tree"),
        Vec::new()
    );

    let trash = test
        .get_deleted_units(&course.id, None)
        .await
        .expect("trash");
    assert_eq!(trash.len(), 1);
    assert_eq!(trash[0].id, ch.id);
    assert_eq!(trash[0].kind, UnitKind::Chapter);
    assert_eq!(trash[0].below, 2);

    // A unit deleted with its chapter cannot be undeleted on its own.
    let child_alone = test
        .undelete_unit(&course.id, &u1.id, CourseLimits::DEFAULT, &user_id)
        .await;
    assert!(matches!(child_alone, Err(error) if error.is_not_found()));

    // Undelete chapter: brings chapter and both units back.
    let restored = test
        .undelete_unit(&course.id, &ch.id, CourseLimits::DEFAULT, &user_id)
        .await
        .expect("chapter restored");
    assert_eq!(restored.below.len(), 2);
    let tree = test.get_unit_tree(&course.id).await.expect("tree");
    assert_eq!(tree.len(), 3);
    assert_eq!(
        tree.iter().filter(|node| node.parent_id == ch.id).count(),
        2
    );

    // Delete U1 separately, then delete its chapter, then undelete U1 -> comes back at top level.
    test.delete_block(&u1.id, &user_id)
        .await
        .expect("u1 deleted");
    test.delete_block(&ch.id, &user_id)
        .await
        .expect("ch deleted");
    let orphan = test
        .undelete_unit(&course.id, &u1.id, CourseLimits::DEFAULT, &user_id)
        .await
        .expect("u1 restored to top");
    assert_eq!(orphan.first.parent_id, "");
    let tree = test.get_unit_tree(&course.id).await.expect("tree");
    assert_eq!(tree.len(), 1);
    assert_eq!(
        (tree[0].id.as_str(), tree[0].parent_id.as_str()),
        (u1.id.as_str(), "")
    );

    let _ = u2;
    test.teardown().await;
}

/// Inserts `count` rows into `blocks` by cloning `template_id` with unique IDs and ranks.
async fn bulk_clone_blocks(test: &common::TestStore, template_id: &str, prefix: &str, count: u32) {
    let system = test.system();
    let query = match db_type() {
        DbType::Sqlite => "SELECT name FROM pragma_table_info('blocks') ORDER BY cid",
        DbType::Postgres => {
            "SELECT column_name::text FROM information_schema.columns \
             WHERE table_schema = current_schema() AND table_name = 'blocks' \
             ORDER BY ordinal_position"
        }
    };
    let columns = system.texts_unchecked(query).await.expect("columns");
    let (numbers, id_expr, fields_expr) = match db_type() {
        DbType::Sqlite => (
            format!(
                "(WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < {count}) SELECT i FROM n) AS n"
            ),
            format!("printf('{prefix}-0000-4000-8000-%012x', n.i)"),
            "json_object('order', printf('V%06x', n.i))".to_owned(),
        ),
        DbType::Postgres => (
            format!("generate_series(1, {count}) AS n(i)"),
            format!("('{prefix}-0000-4000-8000-' || lpad(to_hex(n.i), 12, '0'))::uuid"),
            "jsonb_build_object('order', 'V' || lpad(to_hex(n.i), 6, '0'))".to_owned(),
        ),
    };
    let values: Vec<String> = columns
        .iter()
        .map(|col| match col.as_str() {
            "id" => id_expr.clone(),
            "fields" => fields_expr.clone(),
            other => format!("t.{other}"),
        })
        .collect();
    system
        .execute_unchecked(&format!(
            "INSERT INTO blocks ({}) SELECT {} FROM {numbers}, \
             (SELECT * FROM blocks WHERE id = '{template_id}' LIMIT 1) AS t",
            columns.join(", "),
            values.join(", ")
        ))
        .await
        .expect("bulk cloned");
}

#[tokio::test]
async fn real_limits_allow_200_chapters_and_2000_units_independently() {
    let test = common::setup().await;
    let user_id = common::user(&test, "author").await;
    let workspace_id = common::workspace_of(&test, &[&user_id]).await;
    let course = a_course(&test, &workspace_id, &user_id).await;

    // Seed 1 + 198 = 199 chapters and 1 + 1998 = 1999 units.
    let seed_ch = add_item(
        &test,
        &course.id,
        &user_id,
        UnitKind::Chapter,
        "",
        "Ch seed",
        None,
    )
    .await;
    let seed_u = add_item(
        &test,
        &course.id,
        &user_id,
        UnitKind::Unit,
        &seed_ch.id,
        "U seed",
        None,
    )
    .await;
    bulk_clone_blocks(&test, &seed_ch.id, "c1000000", CHAPTERS_PER_COURSE - 2).await;
    bulk_clone_blocks(&test, &seed_u.id, "a1000000", UNITS_PER_COURSE - 2).await;

    // 200th chapter succeeds; 201st chapter fails with TooManyChapters(200).
    let ch_200 = add_item(
        &test,
        &course.id,
        &user_id,
        UnitKind::Chapter,
        "",
        "Ch 200",
        None,
    )
    .await;
    let ch_201 = test
        .create_unit(
            &course.id,
            &NewUnit {
                kind: UnitKind::Chapter,
                title: "Ch 201".to_owned(),
                ..NewUnit::default()
            },
            CourseLimits::DEFAULT,
            &user_id,
        )
        .await;
    assert_eq!(
        block_error(ch_201),
        BlockError::TooManyChapters {
            limit: CHAPTERS_PER_COURSE
        }
    );

    // 200 chapters did NOT count toward the 2,000-unit limit: 2,000th unit succeeds!
    let u_2000 = add_item(
        &test,
        &course.id,
        &user_id,
        UnitKind::Unit,
        &ch_200.id,
        "U 2000",
        None,
    )
    .await;
    let u_2001 = test
        .create_unit(
            &course.id,
            &NewUnit {
                kind: UnitKind::Unit,
                title: "U 2001".to_owned(),
                ..NewUnit::default()
            },
            CourseLimits::DEFAULT,
            &user_id,
        )
        .await;
    assert_eq!(
        block_error(u_2001),
        BlockError::TooManyUnits {
            limit: UNITS_PER_COURSE
        }
    );

    assert_trash_frees_and_blocks_undelete(&test, &course.id, &user_id, &u_2000.id, &ch_200.id)
        .await;
    test.teardown().await;
}

async fn assert_trash_frees_and_blocks_undelete(
    test: &Store,
    course_id: &str,
    user_id: &str,
    unit_id: &str,
    chapter_id: &str,
) {
    test.delete_block(unit_id, user_id)
        .await
        .expect("u_2000 deleted");
    add_item(
        test,
        course_id,
        user_id,
        UnitKind::Unit,
        "",
        "U replacement",
        None,
    )
    .await;
    let over = test
        .undelete_unit(course_id, unit_id, CourseLimits::DEFAULT, user_id)
        .await;
    assert_eq!(
        block_error(over),
        BlockError::TooManyUnits {
            limit: UNITS_PER_COURSE
        },
        "undeleting unit into full course must fail"
    );

    test.delete_block(chapter_id, user_id)
        .await
        .expect("ch_200 deleted");
    add_item(
        test,
        course_id,
        user_id,
        UnitKind::Chapter,
        "",
        "Ch replacement",
        None,
    )
    .await;
    let over_ch = test
        .undelete_unit(course_id, chapter_id, CourseLimits::DEFAULT, user_id)
        .await;
    assert_eq!(
        block_error(over_ch),
        BlockError::TooManyChapters {
            limit: CHAPTERS_PER_COURSE
        },
        "undeleting chapter into full course must fail"
    );
}

#[tokio::test]
async fn concurrent_unit_creations_keep_distinct_orders_and_respect_limit() {
    let test = common::setup().await;
    let user_id = common::user(&test, "author").await;
    let workspace_id = common::workspace_of(&test, &[&user_id]).await;
    let course = a_course(&test, &workspace_id, &user_id).await;

    let make = |title: &str, limits: CourseLimits| {
        let store = test.clone();
        let course_id = course.id.clone();
        let user_id = user_id.clone();
        let title = title.to_owned();
        async move {
            let new = NewUnit {
                kind: UnitKind::Unit,
                title,
                ..NewUnit::default()
            };
            store.create_unit(&course_id, &new, limits, &user_id).await
        }
    };

    // Six concurrent creations into the same course all succeed with distinct orders.
    let results = tokio::join!(
        make("t1", CourseLimits::DEFAULT),
        make("t2", CourseLimits::DEFAULT),
        make("t3", CourseLimits::DEFAULT),
        make("t4", CourseLimits::DEFAULT),
        make("t5", CourseLimits::DEFAULT),
        make("t6", CourseLimits::DEFAULT),
    );
    let created: Vec<Block> = [
        results.0, results.1, results.2, results.3, results.4, results.5,
    ]
    .into_iter()
    .map(|r| r.expect("concurrent create succeeds").first)
    .collect();
    assert_eq!(created.len(), 6);
    let tree = test.get_unit_tree(&course.id).await.expect("tree");
    let orders: std::collections::HashSet<&str> = tree.iter().map(|n| n.order.as_str()).collect();
    assert_eq!(orders.len(), 6, "all concurrent units have distinct orders");

    // Two concurrent creations competing for the 7th and final slot: one wins, one hits limit.
    let tight = CourseLimits {
        units: 7,
        chapters: 10,
    };
    let (a, b) = tokio::join!(make("race-a", tight), make("race-b", tight));
    let mut ok = 0;
    let mut too_many = 0;
    for res in [a, b] {
        match res {
            Ok(_) => ok += 1,
            Err(StoreError::InvalidBlock {
                source: BlockError::TooManyUnits { limit: 7 },
                ..
            }) => too_many += 1,
            Err(other) => panic!("unexpected error: {other:?}"),
        }
    }
    assert_eq!(
        (ok, too_many),
        (1, 1),
        "exactly one caller wins the last slot"
    );

    test.teardown().await;
}
