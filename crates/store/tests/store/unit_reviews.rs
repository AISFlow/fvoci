//! Store tests for `unit_reviews`, `unit_notes` and the notifications of kind
//! `'review_due'`: personal review/note CRUD, interval suggestions, trash/restore/purge
//! lifecycle, atomic due-review notification deduplication, and PostgreSQL RLS isolation.

use fvoci_model::{
    Block, Container, ContainerKind, ContainerMember, ContainerRole, ContainerVisibility,
    CourseLimits, DEFAULT_REVIEW_INTERVALS, NewUnit, NewUnitNote, NewUnitReview, NotificationKind,
    ReviewStatusFilter, UnitKind, UnitNoteChange, UnitNoteKind, new_id,
};
use fvoci_store::{
    DbType, Joining, Making, MembershipChange, NotificationReader, Paging, Store, StoreError,
    Tenant,
};

use crate::common::{self, db_type};

async fn create_course(
    store: &Store,
    workspace_id: &str,
    owner_id: &str,
    title: &str,
) -> Container {
    let mut course = Container {
        id: new_id(),
        workspace_id: workspace_id.to_owned(),
        visibility: ContainerVisibility::Private,
        kind: ContainerKind::Course,
        title: title.to_owned(),
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
    title: &str,
    user_id: &str,
) -> Block {
    store
        .create_unit(
            course_id,
            &NewUnit {
                kind,
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

/// Reviews CRUD, status/unit filters, idempotent completion, interval progression,
/// atomic next-review scheduling, and workspace today list.
#[tokio::test]
async fn reviews_crud_completion_suggestion_and_today_list() {
    let test = common::setup().await;
    let alice = common::user(&test, "alice").await;
    let bob = common::user(&test, "bob").await;
    common::works_in(&test, &test.workspace_id, &alice).await;
    common::works_in(&test, &test.workspace_id, &bob).await;

    let c1 = create_course(&test, &test.workspace_id, &alice, "Course A").await;
    let c2 = create_course(&test, &test.workspace_id, &alice, "Course B").await;
    save_role(&test, &c1.id, &bob, Some(ContainerRole::Viewer)).await;

    let chapter = create_item(&test, &c1.id, UnitKind::Chapter, "Ch 1", &alice).await;
    let u1 = create_item(&test, &c1.id, UnitKind::Unit, "Unit 1", &alice).await;
    let u2 = create_item(&test, &c2.id, UnitKind::Unit, "Unit 2", &alice).await;

    // Chapters are rejected.
    let err = test
        .schedule_unit_review(
            &c1.id,
            &chapter.id,
            &alice,
            &NewUnitReview {
                scheduled_date: "2026-11-24".to_owned(),
                interval_days: 1,
            },
            None,
        )
        .await
        .expect_err("chapter rejected");
    assert!(matches!(err, StoreError::NotFound(_)));

    let overdue = test
        .schedule_unit_review(
            &c1.id,
            &u1.id,
            &alice,
            &NewUnitReview {
                scheduled_date: "2026-11-22".to_owned(),
                interval_days: 1,
            },
            None,
        )
        .await
        .expect("overdue review");
    let future = test
        .schedule_unit_review(
            &c1.id,
            &u1.id,
            &alice,
            &NewUnitReview {
                scheduled_date: "2026-12-01".to_owned(),
                interval_days: 7,
            },
            None,
        )
        .await
        .expect("future review");
    let due_today = test
        .schedule_unit_review(
            &c2.id,
            &u2.id,
            &alice,
            &NewUnitReview {
                scheduled_date: "2026-11-24".to_owned(),
                interval_days: 3,
            },
            None,
        )
        .await
        .expect("today review");

    // Today list returns overdue + today in ascending scheduled_date order, excluding future.
    let today_items = test
        .list_workspace_today_reviews(&test.workspace_id, &alice, "2026-11-24")
        .await
        .expect("today items");
    assert_eq!(today_items.len(), 2);
    assert_eq!(today_items[0].id, overdue.id);
    assert_eq!(today_items[0].overdue_days, 2);
    assert_eq!(today_items[0].course_title, "Course A");
    assert_eq!(today_items[0].unit_title, "Unit 1");
    assert_eq!(today_items[1].id, due_today.id);
    assert_eq!(today_items[1].overdue_days, 0);

    assert_other_user_cannot_touch_reviews(&test, &c1.id, &bob, &overdue.id).await;
    assert_complete_filter_and_delete_reviews(
        &test,
        &c1.id,
        &u1.id,
        &alice,
        &overdue.id,
        &future.id,
    )
    .await;

    test.teardown().await;
}

async fn assert_other_user_cannot_touch_reviews(
    test: &common::TestStore,
    course_id: &str,
    bob: &str,
    review_id: &str,
) {
    assert_eq!(
        test.list_my_course_reviews(course_id, bob, ReviewStatusFilter::All, None)
            .await
            .expect("bob reviews"),
        Vec::new(),
        "other member sees no course reviews"
    );
    assert_eq!(
        test.list_workspace_today_reviews(&test.workspace_id, bob, "2026-11-24")
            .await
            .expect("bob today"),
        Vec::new(),
        "other member sees no today reviews"
    );
    assert!(
        matches!(
            test.complete_unit_review(
                course_id,
                review_id,
                bob,
                &DEFAULT_REVIEW_INTERVALS,
                None,
                None,
            )
            .await,
            Err(StoreError::NotFound(_))
        ),
        "other member cannot complete review"
    );
    assert!(
        matches!(
            test.delete_unit_review(course_id, review_id, bob).await,
            Err(StoreError::NotFound(_))
        ),
        "other member cannot delete review"
    );
}

async fn assert_complete_filter_and_delete_reviews(
    test: &common::TestStore,
    course_id: &str,
    unit_id: &str,
    alice: &str,
    overdue_id: &str,
    future_id: &str,
) {
    let done = test
        .complete_unit_review(
            course_id,
            overdue_id,
            alice,
            &DEFAULT_REVIEW_INTERVALS,
            Some(&NewUnitReview {
                scheduled_date: "2026-11-27".to_owned(),
                interval_days: 3,
            }),
            None,
        )
        .await
        .expect("completed");
    assert!(
        done.review.completed_at.is_some(),
        "completed_at must be set"
    );
    assert_eq!(done.suggested_next_interval_days, 3, "1 -> 3 suggestion");
    let next = done.next_review.expect("follow-up scheduled");
    assert_eq!(next.scheduled_date, "2026-11-27", "next scheduled_date");
    assert_eq!(next.interval_days, 3, "next interval_days");
    assert!(next.completed_at.is_none(), "next review starts pending");

    let again = test
        .complete_unit_review(
            course_id,
            overdue_id,
            alice,
            &DEFAULT_REVIEW_INTERVALS,
            Some(&NewUnitReview {
                scheduled_date: "2026-12-01".to_owned(),
                interval_days: 7,
            }),
            None,
        )
        .await
        .expect("re-completed");
    assert_eq!(
        again.review.completed_at, done.review.completed_at,
        "completion must be idempotent"
    );
    assert_eq!(
        again.review.updated_at, done.review.updated_at,
        "idempotent replay must not rewrite updated_at"
    );
    assert!(
        again.next_review.is_none(),
        "idempotent replay must not schedule a duplicate follow-up review"
    );

    let pending = test
        .list_my_course_reviews(course_id, alice, ReviewStatusFilter::Pending, Some(unit_id))
        .await
        .expect("pending");
    assert_eq!(
        pending.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
        vec![next.id.as_str(), future_id],
        "pending reviews ordered by scheduled_date"
    );
    let completed = test
        .list_my_course_reviews(course_id, alice, ReviewStatusFilter::Completed, None)
        .await
        .expect("completed");
    assert_eq!(completed.len(), 1, "one completed review");
    assert_eq!(completed[0].id, overdue_id, "completed review id");

    test.delete_unit_review(course_id, future_id, alice)
        .await
        .expect("deleted");
    assert!(
        matches!(
            test.delete_unit_review(course_id, future_id, alice).await,
            Err(StoreError::NotFound(_))
        ),
        "second delete returns NotFound"
    );
    assert!(
        matches!(
            test.delete_unit_review(course_id, "not-a-uuid", alice)
                .await,
            Err(StoreError::NotFound(_))
        ),
        "non-UUID review id returns NotFound"
    );
}

/// Personal unit notes CRUD, kind filtering, and strict per-user isolation.
#[tokio::test]
async fn notes_crud_and_personal_isolation() {
    let test = common::setup().await;
    let alice = common::user(&test, "alice").await;
    let bob = common::user(&test, "bob").await;
    common::works_in(&test, &test.workspace_id, &alice).await;
    common::works_in(&test, &test.workspace_id, &bob).await;

    let course = create_course(&test, &test.workspace_id, &alice, "Course").await;
    save_role(&test, &course.id, &bob, Some(ContainerRole::Admin)).await;
    let chapter = create_item(&test, &course.id, UnitKind::Chapter, "Ch", &alice).await;
    let unit = create_item(&test, &course.id, UnitKind::Unit, "U1", &alice).await;

    assert!(matches!(
        test.list_my_unit_notes(&course.id, &chapter.id, &alice, None)
            .await,
        Err(StoreError::NotFound(_))
    ));

    let q = test
        .create_unit_note(
            &course.id,
            &unit.id,
            &alice,
            &NewUnitNote {
                kind: UnitNoteKind::Question,
                content: "First question".to_owned(),
            },
        )
        .await
        .expect("question");
    let m = test
        .create_unit_note(
            &course.id,
            &unit.id,
            &alice,
            &NewUnitNote {
                kind: UnitNoteKind::Mistake,
                content: "Off-by-one error".to_owned(),
            },
        )
        .await
        .expect("mistake");

    let all = test
        .list_my_unit_notes(&course.id, &unit.id, &alice, None)
        .await
        .expect("all notes");
    assert_eq!(all.len(), 2);
    let only_mistakes = test
        .list_my_unit_notes(&course.id, &unit.id, &alice, Some(UnitNoteKind::Mistake))
        .await
        .expect("mistakes");
    assert_eq!(only_mistakes, vec![m.clone()]);

    // Bob (even as course admin) sees nothing and cannot patch or delete Alice's notes.
    assert_eq!(
        test.list_my_unit_notes(&course.id, &unit.id, &bob, None)
            .await
            .expect("bob notes"),
        Vec::new()
    );
    assert!(matches!(
        test.patch_unit_note(
            &course.id,
            &q.id,
            &bob,
            &UnitNoteChange {
                kind: Some(UnitNoteKind::Review),
                content: Some("hijack".to_owned()),
            },
        )
        .await,
        Err(StoreError::NotFound(_))
    ));
    assert!(matches!(
        test.delete_unit_note(&course.id, &q.id, &bob).await,
        Err(StoreError::NotFound(_))
    ));

    let patched = test
        .patch_unit_note(
            &course.id,
            &q.id,
            &alice,
            &UnitNoteChange {
                kind: Some(UnitNoteKind::Review),
                content: Some("Answered and reviewed".to_owned()),
            },
        )
        .await
        .expect("patched");
    assert_eq!(patched.kind, UnitNoteKind::Review);
    assert_eq!(patched.content, "Answered and reviewed");

    test.delete_unit_note(&course.id, &q.id, &alice)
        .await
        .expect("deleted");
    assert_eq!(
        test.list_my_unit_notes(&course.id, &unit.id, &alice, None)
            .await
            .expect("remaining"),
        vec![m]
    );

    test.teardown().await;
}

/// Soft-deleting a unit or course hides its reviews and notes while preserving them for
/// restore; permanent trash purge removes them completely.
#[tokio::test]
async fn trash_preserves_and_permanent_purge_cleans_reviews_and_notes() {
    let test = common::setup().await;
    let alice = common::user(&test, "alice").await;
    common::works_in(&test, &test.workspace_id, &alice).await;

    let course = create_course(&test, &test.workspace_id, &alice, "Course").await;
    let unit = create_item(&test, &course.id, UnitKind::Unit, "U1", &alice).await;

    let review = test
        .schedule_unit_review(
            &course.id,
            &unit.id,
            &alice,
            &NewUnitReview {
                scheduled_date: "2026-11-24".to_owned(),
                interval_days: 1,
            },
            None,
        )
        .await
        .expect("review");
    let note = test
        .create_unit_note(
            &course.id,
            &unit.id,
            &alice,
            &NewUnitNote {
                kind: UnitNoteKind::Review,
                content: "Remember invariant".to_owned(),
            },
        )
        .await
        .expect("note");

    // Soft-delete unit: hidden from reads, still in DB.
    test.delete_block(&unit.id, &alice).await.expect("trashed");
    assert_eq!(
        test.list_my_course_reviews(&course.id, &alice, ReviewStatusFilter::All, None)
            .await
            .expect("reviews"),
        Vec::new()
    );
    assert_eq!(
        test.list_workspace_today_reviews(&test.workspace_id, &alice, "2026-11-24")
            .await
            .expect("today"),
        Vec::new()
    );
    assert_eq!(
        test.system()
            .due_unnotified_reviews("2026-11-24", None, 50)
            .await
            .expect("due"),
        Vec::new()
    );

    // Restore unit: reviews & notes come back intact.
    let _ = test
        .undelete_unit(&course.id, &unit.id, CourseLimits::DEFAULT, &alice)
        .await
        .expect("restored");
    assert_eq!(
        test.list_my_course_reviews(&course.id, &alice, ReviewStatusFilter::All, None)
            .await
            .expect("restored reviews"),
        vec![review]
    );
    assert_eq!(
        test.list_my_unit_notes(&course.id, &unit.id, &alice, None)
            .await
            .expect("restored notes"),
        vec![note]
    );

    // Soft-delete again and permanently purge trash: rows must vanish.
    test.delete_block(&unit.id, &alice)
        .await
        .expect("trashed again");
    test.system().purge_trash(i64::MAX).await.expect("purged");
    for table in ["unit_reviews", "unit_notes"] {
        assert_eq!(
            test.count_rows(table).await.expect("count"),
            0,
            "{table} must be empty after permanent purge"
        );
    }

    test.teardown().await;
}

/// `notify_due_review` marks `notified_at` and creates one `review_due` notification
/// atomically, returning `false` on replay with zero duplicate notifications.
#[tokio::test]
async fn notify_due_review_is_atomic_and_deduplicated() {
    let test = common::setup().await;
    let alice = common::user(&test, "alice").await;
    let bob = common::user(&test, "bob").await;
    common::works_in(&test, &test.workspace_id, &alice).await;
    common::works_in(&test, &test.workspace_id, &bob).await;

    let course = create_course(&test, &test.workspace_id, &alice, "Course").await;
    save_role(&test, &course.id, &bob, Some(ContainerRole::Editor)).await;
    let unit = create_item(&test, &course.id, UnitKind::Unit, "U1", &alice).await;

    test.schedule_unit_review(
        &course.id,
        &unit.id,
        &alice,
        &NewUnitReview {
            scheduled_date: "2026-11-24".to_owned(),
            interval_days: 1,
        },
        None,
    )
    .await
    .expect("review");

    // Un-enrolled learners' overdue reviews must be filtered out in SQL so they cannot
    // starve other learners' due-review notifications.
    test.schedule_unit_review(
        &course.id,
        &unit.id,
        &bob,
        &NewUnitReview {
            scheduled_date: "2026-11-20".to_owned(),
            interval_days: 1,
        },
        None,
    )
    .await
    .expect("bob review");
    save_role(&test, &course.id, &bob, None).await;

    let due = test
        .system()
        .due_unnotified_reviews("2026-11-24", None, 50)
        .await
        .expect("due");
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].user_id, alice);
    assert!(
        test.system()
            .notify_due_review(&due[0], 10_000, 10_000)
            .await
            .expect("first notify")
    );
    assert!(
        !test
            .system()
            .notify_due_review(&due[0], 20_000, 10_000)
            .await
            .expect("second notify deduped")
    );
    assert_eq!(
        test.system()
            .due_unnotified_reviews("2026-11-24", None, 50)
            .await
            .expect("none left"),
        Vec::new()
    );

    let alice_reader = NotificationReader {
        user_id: &alice,
        making: Making::with_password(),
    };
    let bob_reader = NotificationReader {
        user_id: &bob,
        making: Making::with_password(),
    };
    let alice_notifs = test
        .notifications_of_user(
            alice_reader,
            Paging {
                page: 0,
                per_page: std::num::NonZeroU32::new(50).expect("a page size"),
            },
        )
        .await
        .expect("alice notifs");
    assert_eq!(alice_notifs.len(), 1);
    assert_eq!(alice_notifs[0].kind, NotificationKind::ReviewDue);
    assert_eq!(alice_notifs[0].block_id, unit.id);
    assert_eq!(
        test.notifications_of_user(
            bob_reader,
            Paging {
                page: 0,
                per_page: std::num::NonZeroU32::new(50).expect("a page size")
            }
        )
        .await
        .expect("bob notifs"),
        Vec::new(),
        "other course members must never receive Alice's review_due notification"
    );

    test.teardown().await;
}

/// Cross-organization RLS isolation on PostgreSQL and cleanup on workspace removal and
/// org-admin unjoining.
#[tokio::test]
async fn cross_org_rls_and_workspace_removal_cleanup() {
    let test = common::setup().await;
    let alice = common::user(&test, "alice").await;
    let admin = common::user(&test, "org-admin").await;
    common::works_in(&test, &test.workspace_id, &alice).await;
    test.execute_unchecked(&format!(
        "UPDATE organization_members SET role = 'admin' WHERE user_id = '{admin}'"
    ))
    .await
    .expect("org admin");
    assert_eq!(
        test.join_workspace_as_org_admin(&test.workspace_id, &admin)
            .await
            .expect("joined"),
        Joining::Joined
    );

    let course = create_course(&test, &test.workspace_id, &alice, "Course").await;
    save_role(&test, &course.id, &admin, Some(ContainerRole::Admin)).await;
    let unit = create_item(&test, &course.id, UnitKind::Unit, "U1", &alice).await;

    for learner in [&alice, &admin] {
        test.schedule_unit_review(
            &course.id,
            &unit.id,
            learner,
            &NewUnitReview {
                scheduled_date: "2026-11-24".to_owned(),
                interval_days: 1,
            },
            None,
        )
        .await
        .expect("review");
        test.create_unit_note(
            &course.id,
            &unit.id,
            learner,
            &NewUnitNote {
                kind: UnitNoteKind::Question,
                content: "personal".to_owned(),
            },
        )
        .await
        .expect("note");
    }

    if db_type() == DbType::Postgres {
        let (other_org, _) = common::organization(&test.system(), "other-org").await;
        let other = test.for_org(&other_org).expect("other org");
        assert_eq!(other.count_rows("unit_reviews").await.expect("rls"), 0);
        assert_eq!(other.count_rows("unit_notes").await.expect("rls"), 0);
        let unset = test.scoped(Tenant::Unset);
        assert_eq!(unset.count_rows("unit_reviews").await.expect("unset"), 0);
        assert_eq!(unset.count_rows("unit_notes").await.expect("unset"), 0);
    }

    // Org admin ends joining: admin's rows are removed, Alice's stay.
    assert!(
        test.leave_workspace_as_org_admin(&test.workspace_id, &admin)
            .await
            .expect("unjoined")
    );
    assert_eq!(test.count_rows("unit_reviews").await.expect("left"), 1);
    assert_eq!(test.count_rows("unit_notes").await.expect("left"), 1);

    // Removing Alice from workspace (by a workspace admin) removes Alice's rows too.
    let ws_admin = common::user(&test, "ws-admin").await;
    common::works_in(&test, &test.workspace_id, &ws_admin).await;
    test.execute_unchecked(&format!(
        "UPDATE workspace_members SET role = 'admin' WHERE workspace_id = '{}' AND user_id = '{ws_admin}'",
        test.workspace_id
    ))
    .await
    .expect("ws admin promoted");
    let removed = test
        .remove_workspace_member(&test.workspace_id, &alice, &ws_admin)
        .await
        .expect("removed");
    assert!(
        matches!(removed, MembershipChange::Done { .. }),
        "expected removal to succeed: {removed:?}"
    );
    assert_eq!(test.count_rows("unit_reviews").await.expect("none"), 0);
    assert_eq!(test.count_rows("unit_notes").await.expect("none"), 0);

    test.teardown().await;
}
