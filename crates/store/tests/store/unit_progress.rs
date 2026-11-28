//! Store tests for `unit_progress`: personal progress CRUD, idempotent
//! completion, trash/restore/purge lifecycle, course progress summaries, and tenant
//! isolation.

use fvoci_model::{
    Block, BlockType, CompletionChange, Container, ContainerKind, ContainerMember, ContainerRole,
    ContainerVisibility, CourseLimits, CourseProgressSummary, LastPositionChange,
    MemberCourseProgress, NewUnit, UnitKind, UnitProgress, UnitProgressChange, WorkspaceRole,
    new_id,
};
use fvoci_store::{DbType, Joining, Store, Tenant};

use crate::common::{self, db_type};

async fn create_course(store: &Store, workspace_id: &str, owner_id: &str) -> Container {
    let mut course = Container {
        id: new_id(),
        workspace_id: workspace_id.to_owned(),
        visibility: ContainerVisibility::Private,
        kind: ContainerKind::Course,
        title: "Course".to_owned(),
        ..Container::default()
    };
    store
        .insert_container_with_admin(&mut course, owner_id)
        .await
        .expect("course created");
    course
}

async fn create_item(
    store: &Store,
    course_id: &str,
    kind: UnitKind,
    parent_id: &str,
    title: &str,
    user_id: &str,
) -> Block {
    store
        .create_unit(
            course_id,
            &NewUnit {
                kind,
                parent_id: parent_id.to_owned(),
                title: title.to_owned(),
                ..NewUnit::default()
            },
            CourseLimits::DEFAULT,
            user_id,
        )
        .await
        .expect("item created")
        .first
}

fn change(completion: CompletionChange, last_position: LastPositionChange) -> UnitProgressChange {
    UnitProgressChange {
        completion,
        last_position,
    }
}

async fn save_role(store: &Store, course_id: &str, user_id: &str, role: Option<ContainerRole>) {
    match role {
        Some(role) => {
            let member = ContainerMember {
                container_id: course_id.to_owned(),
                user_id: user_id.to_owned(),
                role,
                synthetic: false,
                inherited_role: None,
            };
            store.save_member(&member).await.expect("member saved");
        }
        None => {
            store
                .delete_member(course_id, user_id)
                .await
                .expect("member removed");
        }
    }
}

/// Upserts refuse non-unit targets (chapters, units of another course, board cards, and
/// unknown identifiers) with `NotFound`.
#[tokio::test]
async fn upsert_progress_refuses_non_unit_and_cross_course_targets() {
    let test = common::setup().await;
    let alice = common::user(&test, "alice").await;
    common::works_in(&test, &test.workspace_id, &alice).await;

    let course_a = create_course(&test, &test.workspace_id, &alice).await;
    let course_b = create_course(&test, &test.workspace_id, &alice).await;
    let chapter = create_item(&test, &course_a.id, UnitKind::Chapter, "", "Ch 1", &alice).await;
    let unit_b = create_item(&test, &course_b.id, UnitKind::Unit, "", "Unit B", &alice).await;
    let boards = common::create_test_boards(&test, &test.workspace_id, &alice, 1).await;
    let mut card = Block {
        id: new_id(),
        container_id: boards[0].id.clone(),
        block_type: BlockType::Card,
        title: "Card".to_owned(),
        ..Block::default()
    };
    test.insert_block(&mut card, &alice)
        .await
        .expect("card created");

    for (label, cid, uid) in [
        ("chapter", course_a.id.as_str(), chapter.id.as_str()),
        (
            "other course unit",
            course_a.id.as_str(),
            unit_b.id.as_str(),
        ),
        ("board card", boards[0].id.as_str(), card.id.as_str()),
        ("unknown unit", course_a.id.as_str(), "not-a-uuid"),
    ] {
        let err = test
            .upsert_unit_progress(
                cid,
                uid,
                &alice,
                &change(CompletionChange::Complete, LastPositionChange::Keep),
            )
            .await
            .expect_err(label);
        assert!(err.is_not_found(), "{label}: {err:?}");
    }
    test.teardown().await;
}

/// Completion is idempotent (preserves initial `completed_at`), uncompletion clears it,
/// `last_position` supports keep/set/clear, `planned_date` is preserved, and rows are
/// isolated per user.
#[tokio::test]
async fn upsert_progress_is_idempotent_preserves_planned_date_and_scopes_by_user() {
    let test = common::setup().await;
    let alice = common::user(&test, "alice").await;
    let bob = common::user(&test, "bob").await;
    common::works_in(&test, &test.workspace_id, &alice).await;
    common::works_in(&test, &test.workspace_id, &bob).await;

    let course = create_course(&test, &test.workspace_id, &alice).await;
    let unit = create_item(&test, &course.id, UnitKind::Unit, "", "Unit A", &alice).await;

    let first = test
        .upsert_unit_progress(
            &course.id,
            &unit.id,
            &alice,
            &change(
                CompletionChange::Complete,
                LastPositionChange::Set("paragraph-3".to_owned()),
            ),
        )
        .await
        .expect("completed");
    let first_completed_at = first.completed_at.expect("completed_at set");
    assert_eq!(first.last_position.as_deref(), Some("paragraph-3"));
    assert_eq!(first.planned_date, None);

    test.execute_unchecked(&format!(
        "UPDATE unit_progress SET planned_date = '2026-12-01' \
         WHERE unit_id = '{}' AND user_id = '{alice}'",
        unit.id
    ))
    .await
    .expect("planned_date seeded");

    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let second = test
        .upsert_unit_progress(
            &course.id,
            &unit.id,
            &alice,
            &change(CompletionChange::Complete, LastPositionChange::Keep),
        )
        .await
        .expect("idempotent complete");
    assert_eq!(second.completed_at, Some(first_completed_at));
    assert_eq!(second.last_position.as_deref(), Some("paragraph-3"));
    assert_eq!(second.planned_date.as_deref(), Some("2026-12-01"));

    let cleared = test
        .upsert_unit_progress(
            &course.id,
            &unit.id,
            &alice,
            &change(CompletionChange::Incomplete, LastPositionChange::Clear),
        )
        .await
        .expect("cleared");
    assert_eq!(cleared.completed_at, None);
    assert_eq!(cleared.last_position, None);
    assert_eq!(cleared.planned_date.as_deref(), Some("2026-12-01"));

    assert_eq!(
        test.get_my_course_progress(&course.id, &bob)
            .await
            .expect("bob read"),
        Vec::<UnitProgress>::new()
    );
    assert_eq!(
        test.get_my_course_progress(&course.id, &alice)
            .await
            .expect("alice read"),
        vec![cleared]
    );
    test.teardown().await;
}

/// Progress rows stay in the database while a unit or course is in the trash (excluded
/// from active reads and summaries), reappear on restore, and are removed when permanently
/// purged.
#[tokio::test]
async fn trash_preserves_progress_until_permanent_purge() {
    let test = common::setup().await;
    let alice = common::user(&test, "alice").await;
    common::works_in(&test, &test.workspace_id, &alice).await;

    let course = create_course(&test, &test.workspace_id, &alice).await;
    let unit1 = create_item(&test, &course.id, UnitKind::Unit, "", "Unit 1", &alice).await;
    let unit2 = create_item(&test, &course.id, UnitKind::Unit, "", "Unit 2", &alice).await;
    for uid in [&unit1.id, &unit2.id] {
        test.upsert_unit_progress(
            &course.id,
            uid,
            &alice,
            &change(CompletionChange::Complete, LastPositionChange::Keep),
        )
        .await
        .expect("completed");
    }

    test.delete_block(&unit1.id, &alice)
        .await
        .expect("unit1 trashed");
    let during_trash = test
        .get_my_course_progress(&course.id, &alice)
        .await
        .expect("progress during trash");
    assert_eq!(during_trash.len(), 1);
    assert_eq!(during_trash[0].unit_id, unit2.id);
    assert_eq!(
        test.get_course_progress_summary(&course.id)
            .await
            .expect("summary")
            .total_units,
        1
    );
    assert_eq!(test.count_rows("unit_progress").await.expect("count"), 2);

    let err = test
        .upsert_unit_progress(
            &course.id,
            &unit1.id,
            &alice,
            &change(CompletionChange::Complete, LastPositionChange::Keep),
        )
        .await
        .expect_err("trashed unit refused");
    assert!(err.is_not_found(), "{err:?}");

    let _ = test
        .undelete_unit(&course.id, &unit1.id, CourseLimits::DEFAULT, &alice)
        .await
        .expect("unit1 restored");
    assert_eq!(
        test.get_my_course_progress(&course.id, &alice)
            .await
            .expect("after restore")
            .len(),
        2
    );

    test.delete_block(&unit1.id, &alice)
        .await
        .expect("unit1 trashed again");
    test.system()
        .purge_trash(i64::MAX)
        .await
        .expect("trash purged");
    assert_eq!(test.count_rows("unit_progress").await.expect("count"), 1);

    test.delete_container(&course.id, &alice)
        .await
        .expect("course trashed");
    assert_eq!(test.count_rows("unit_progress").await.expect("count"), 1);
    test.undelete_container(&course.id, &alice)
        .await
        .expect("course restored");
    test.delete_container(&course.id, &alice)
        .await
        .expect("course trashed again");
    test.system()
        .purge_trash(i64::MAX)
        .await
        .expect("course purged");
    assert_eq!(test.count_rows("unit_progress").await.expect("count"), 0);
    test.teardown().await;
}

/// Summary counts only live units (never chapters), includes only active members with a
/// role, excludes accounts in deletion grace period, and workspace removal wipes the
/// removed member's progress in that workspace.
#[tokio::test]
async fn summary_and_workspace_removal_enforce_active_member_rules() {
    let test = common::setup().await;
    let admin = common::user(&test, "admin").await;
    let learner = common::user(&test, "learner").await;
    let pending_delete = common::user(&test, "pending").await;
    let roleless = common::user(&test, "roleless").await;
    for uid in [&admin, &learner, &pending_delete, &roleless] {
        common::works_in(&test, &test.workspace_id, uid).await;
    }

    let course = create_course(&test, &test.workspace_id, &admin).await;
    create_item(&test, &course.id, UnitKind::Chapter, "", "Ch 1", &admin).await;
    let u1 = create_item(&test, &course.id, UnitKind::Unit, "", "U 1", &admin).await;
    let u2 = create_item(&test, &course.id, UnitKind::Unit, "", "U 2", &admin).await;
    save_role(&test, &course.id, &learner, Some(ContainerRole::Viewer)).await;
    save_role(
        &test,
        &course.id,
        &pending_delete,
        Some(ContainerRole::Editor),
    )
    .await;
    save_role(&test, &course.id, &roleless, None).await;

    let complete = change(CompletionChange::Complete, LastPositionChange::Keep);
    let incomplete = change(CompletionChange::Incomplete, LastPositionChange::Keep);
    test.upsert_unit_progress(&course.id, &u1.id, &learner, &complete)
        .await
        .expect("learner u1");
    test.upsert_unit_progress(&course.id, &u2.id, &learner, &incomplete)
        .await
        .expect("learner u2");
    test.upsert_unit_progress(&course.id, &u1.id, &pending_delete, &complete)
        .await
        .expect("pending u1");
    test.execute_unchecked(&format!(
        "UPDATE users SET deleted_at = 1000 WHERE id = '{pending_delete}'"
    ))
    .await
    .expect("marked pending deletion");

    let mut expected = vec![
        MemberCourseProgress {
            user_id: admin.clone(),
            completed_units: 0,
        },
        MemberCourseProgress {
            user_id: learner.clone(),
            completed_units: 1,
        },
    ];
    expected.sort_by(|a, b| a.user_id.cmp(&b.user_id));
    assert_eq!(
        test.get_course_progress_summary(&course.id)
            .await
            .expect("summary"),
        CourseProgressSummary {
            total_units: 2,
            members: expected,
        }
    );

    let _ = test
        .add_workspace_member(&test.workspace_id, &admin, WorkspaceRole::Admin)
        .await;
    test.execute_unchecked(&format!(
        "UPDATE workspace_members SET role = 'admin' \
         WHERE workspace_id = '{}' AND user_id = '{admin}'",
        test.workspace_id
    ))
    .await
    .expect("admin promotes");
    test.remove_workspace_member(&test.workspace_id, &learner, &admin)
        .await
        .expect("learner removed");
    let remaining: Vec<String> = test
        .system()
        .texts_unchecked(&format!(
            "SELECT CAST(unit_id AS TEXT) FROM unit_progress WHERE user_id = '{learner}'"
        ))
        .await
        .expect("query");
    assert_eq!(remaining, Vec::<String>::new());

    // Write-time lifecycle checks: removed workspace members and accounts pending deletion
    // are refused inside the upsert transaction.
    for blocked in [&learner, &pending_delete] {
        let err = test
            .upsert_unit_progress(&course.id, &u1.id, blocked, &complete)
            .await
            .expect_err("inactive or removed learner refused");
        assert!(err.is_not_found(), "{err:?}");
    }
    test.teardown().await;
}

/// Ending an org-admin joining where the admin was already a workspace member removes
/// progress recorded on courses joined during the joining while preserving progress on
/// courses they belonged to beforehand, and soft-deleting an organization hides progress
/// and unit counts from direct store reads.
#[tokio::test]
async fn org_admin_joining_end_and_soft_deleted_org_enforce_progress_bounds() {
    let test = common::setup().await;
    let owner = common::user(&test, "owner").await;
    let org_admin = common::user(&test, "orgadmin").await;
    common::works_in(&test, &test.workspace_id, &owner).await;
    common::works_in(&test, &test.workspace_id, &org_admin).await;
    test.execute_unchecked(&format!(
        "UPDATE organization_members SET role = 'admin' WHERE user_id = '{org_admin}'"
    ))
    .await
    .expect("org admin");

    let prior_course = create_course(&test, &test.workspace_id, &owner).await;
    let prior_unit = create_item(&test, &prior_course.id, UnitKind::Unit, "", "P1", &owner).await;
    save_role(
        &test,
        &prior_course.id,
        &org_admin,
        Some(ContainerRole::Viewer),
    )
    .await;
    let complete = change(CompletionChange::Complete, LastPositionChange::Keep);
    test.upsert_unit_progress(&prior_course.id, &prior_unit.id, &org_admin, &complete)
        .await
        .expect("prior progress");

    let joined_course = create_course(&test, &test.workspace_id, &owner).await;
    let joined_unit = create_item(&test, &joined_course.id, UnitKind::Unit, "", "J1", &owner).await;
    assert_eq!(
        test.join_workspace_as_org_admin(&test.workspace_id, &org_admin)
            .await
            .expect("join"),
        Joining::Joined
    );
    save_role(
        &test,
        &joined_course.id,
        &org_admin,
        Some(ContainerRole::Admin),
    )
    .await;
    test.upsert_unit_progress(&joined_course.id, &joined_unit.id, &org_admin, &complete)
        .await
        .expect("joining-era progress");
    assert!(
        test.leave_workspace_as_org_admin(&test.workspace_id, &org_admin)
            .await
            .expect("leave joining")
    );
    assert_eq!(
        test.get_my_course_progress(&prior_course.id, &org_admin)
            .await
            .expect("prior kept")
            .len(),
        1
    );
    assert_eq!(
        test.get_my_course_progress(&joined_course.id, &org_admin)
            .await
            .expect("joining-era gone"),
        Vec::<UnitProgress>::new()
    );

    test.execute_unchecked(&format!(
        "UPDATE organizations SET deleted_at = 1000 WHERE id = '{}'",
        test.org_id
    ))
    .await
    .expect("org soft deleted");
    assert_eq!(
        test.get_my_course_progress(&prior_course.id, &org_admin)
            .await
            .expect("deleted org progress"),
        Vec::<UnitProgress>::new()
    );
    assert_eq!(
        test.get_course_progress_summary(&prior_course.id)
            .await
            .expect("deleted org summary"),
        CourseProgressSummary {
            total_units: 0,
            members: Vec::new(),
        }
    );
    test.teardown().await;
}

/// On PostgreSQL, `unit_progress` rows stay strictly behind `org_wall` RLS, an unset
/// tenant sees nothing, and a mistaken row written by org A under org B's unit ID does
/// not interfere with org B's progress or survive system purge.
#[tokio::test]
async fn postgres_org_isolation_and_cross_org_key_safety() {
    if db_type() != DbType::Postgres {
        return;
    }
    let test = common::setup().await;
    let unset = test.scoped(Tenant::Unset);
    let (org_b, ws_b) = common::organization(&unset, "org-b").await;
    let store_b = test.for_org(&org_b).expect("org b");
    let user_a = common::user(&test, "user-a").await;
    let user_b = common::user(&store_b, "user-b").await;
    common::works_in(&test, &test.workspace_id, &user_a).await;
    common::works_in(&store_b, &ws_b, &user_b).await;

    let course_b = create_course(&store_b, &ws_b, &user_b).await;
    let unit_b = create_item(&store_b, &course_b.id, UnitKind::Unit, "", "B1", &user_b).await;

    let err = test
        .upsert_unit_progress(
            &course_b.id,
            &unit_b.id,
            &user_a,
            &change(CompletionChange::Complete, LastPositionChange::Keep),
        )
        .await
        .expect_err("org A cannot touch org B unit");
    assert!(err.is_not_found(), "{err:?}");

    let cross_org_course_err = test
        .execute_unchecked(&format!(
            "INSERT INTO unit_progress (unit_id, container_id, user_id, completed_at, updated_at) \
             VALUES ('{}', '{}', '{user_b}', 100, 100)",
            unit_b.id, course_b.id
        ))
        .await;
    assert!(
        matches!(
            cross_org_course_err,
            Err(fvoci_store::StoreError::Database(_))
        ),
        "org A cannot point unit_progress at org B's course: {cross_org_course_err:?}"
    );

    let course_a = create_course(&test, &test.workspace_id, &user_a).await;
    test.execute_unchecked(&format!(
        "INSERT INTO unit_progress (unit_id, container_id, user_id, completed_at, updated_at) \
         VALUES ('{}', '{}', '{user_b}', 100, 100)",
        unit_b.id, course_a.id
    ))
    .await
    .expect("org A inserts its own row under its own course");
    store_b
        .upsert_unit_progress(
            &course_b.id,
            &unit_b.id,
            &user_b,
            &change(
                CompletionChange::Complete,
                LastPositionChange::Set("b-own".to_owned()),
            ),
        )
        .await
        .expect("org B writes its own progress unimpeded");
    assert_eq!(unset.count_rows("unit_progress").await.expect("unset"), 0);
    assert_eq!(store_b.count_rows("unit_progress").await.expect("org b"), 1);

    test.delete_container(&course_a.id, &user_a)
        .await
        .expect("trash org A course");
    test.system()
        .purge_trash(i64::MAX)
        .await
        .expect("system purge removes org A purged-course row");
    assert_eq!(test.count_rows("unit_progress").await.expect("org a"), 0);
    assert_eq!(store_b.count_rows("unit_progress").await.expect("org b"), 1);
    test.teardown().await;
}

/// `require_live_unit` and `lock_active_learner` re-verify active course enrollment
/// inside the database transaction so an unjoined workspace member cannot write
/// `unit_progress`, `planned_date`, `unit_notes`, `unit_reviews`, or redistribute
/// personal schedules even if called concurrently with course departure.
#[tokio::test]
async fn unjoined_workspace_member_is_refused_inside_unit_and_schedule_transactions() {
    use fvoci_model::{
        DiaryDate, NewUnitNote, NewUnitReview, RedistributeScheduleRequest, UnitNoteKind,
    };

    let test = common::setup().await;
    let admin = common::user(&test, "admin").await;
    let unjoined = common::user(&test, "unjoined").await;
    common::works_in(&test, &test.workspace_id, &admin).await;
    common::works_in(&test, &test.workspace_id, &unjoined).await;

    let course = create_course(&test, &test.workspace_id, &admin).await;
    let unit = create_item(&test, &course.id, UnitKind::Unit, "", "U1", &admin).await;
    let today: DiaryDate = "2026-11-24".parse().expect("date");
    let planned: DiaryDate = "2026-11-25".parse().expect("planned date");

    assert!(
        test.upsert_unit_progress(
            &course.id,
            &unit.id,
            &unjoined,
            &change(CompletionChange::Complete, LastPositionChange::Keep),
        )
        .await
        .is_err_and(|error| error.is_not_found()),
        "unjoined member refused by upsert_unit_progress"
    );

    assert!(
        test.set_unit_planned_date(&course.id, &unit.id, &unjoined, Some(planned))
            .await
            .is_err_and(|error| error.is_not_found()),
        "unjoined member refused by set_unit_planned_date"
    );

    assert!(
        test.create_unit_note(
            &course.id,
            &unit.id,
            &unjoined,
            &NewUnitNote {
                kind: UnitNoteKind::Review,
                content: "unjoined".to_owned(),
            },
        )
        .await
        .is_err_and(|error| error.is_not_found()),
        "unjoined member refused by create_unit_note"
    );

    assert!(
        test.schedule_unit_review(
            &course.id,
            &unit.id,
            &unjoined,
            &NewUnitReview {
                scheduled_date: "2026-11-25".to_owned(),
                interval_days: 1,
            },
            None,
        )
        .await
        .is_err_and(|error| error.is_not_found()),
        "unjoined member refused by schedule_unit_review"
    );

    let redist = RedistributeScheduleRequest {
        from_date: Some("2026-11-24".to_owned()),
        target_date: Some("2026-11-30".to_owned()),
        study_days: Some(0b111_1111),
    };
    assert!(
        test.redistribute_personal_schedule(&course.id, &unjoined, &redist, today)
            .await
            .is_err_and(|error| error.is_not_found()),
        "unjoined member refused by redistribute_personal_schedule"
    );

    assert_eq!(test.count_rows("unit_progress").await.expect("count"), 0);
    assert_eq!(test.count_rows("unit_notes").await.expect("count"), 0);
    assert_eq!(test.count_rows("unit_reviews").await.expect("count"), 0);
    test.teardown().await;
}
