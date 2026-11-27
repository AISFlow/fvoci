//! Store tests for course exams, strictly personal exam scores, aggregate averages,
//! bounds checking, and cleanup on purge, workspace removal, org leave, and erasure.

use fvoci_model::{
    Container, ContainerKind, ContainerMember, ContainerVisibility, CourseExamChange,
    ExamDateChange, NewCourseExam, ValidatedMyExamScore, new_id,
};
use fvoci_store::{ExamPublishError, ExamScoreOutcome, ExamViewer, Store};

use crate::account_erasure::{T0, erase, founder};
use crate::common;

async fn create_course(store: &Store, workspace_id: &str, admin: &str) -> String {
    let mut course = Container {
        id: new_id(),
        workspace_id: workspace_id.to_owned(),
        visibility: ContainerVisibility::Private,
        kind: ContainerKind::Course,
        title: "Algorithms".to_owned(),
        ..Container::default()
    };
    store
        .insert_container_with_admin(&mut course, admin)
        .await
        .expect("course created");
    course.id
}

async fn enroll(store: &Store, course_id: &str, user_id: &str) {
    store
        .save_member(&ContainerMember {
            container_id: course_id.to_owned(),
            user_id: user_id.to_owned(),
            role: fvoci_model::ContainerRole::Viewer,
            synthetic: false,
            inherited_role: None,
        })
        .await
        .expect("enrolled");
}

fn new_exam(title: &str, exam_date: Option<&str>, max_score: f64) -> fvoci_model::ValidatedNewExam {
    NewCourseExam {
        title: title.to_owned(),
        exam_date: exam_date.map(str::to_owned),
        max_score,
        description: "Covers weeks 1-4".to_owned(),
    }
    .check_request()
    .expect("valid exam")
}

/// Exam CRUD, ordering (dated first, undated last), score bounds, per-caller isolation,
/// active-member average/count, and score/exam deletion.
#[tokio::test]
async fn exams_crud_score_isolation_and_active_member_aggregates() {
    let test = common::setup().await;
    let alice = common::user(&test, "alice").await;
    let bob = common::user(&test, "bob").await;
    let carol = common::user(&test, "carol").await;
    common::works_in(&test, &test.workspace_id, &alice).await;
    common::works_in(&test, &test.workspace_id, &bob).await;
    common::works_in(&test, &test.workspace_id, &carol).await;
    let course_id = create_course(&test, &test.workspace_id, &alice).await;
    enroll(&test, &course_id, &bob).await;
    enroll(&test, &course_id, &carol).await;

    let undated = test
        .create_course_exam(&course_id, &new_exam("Undated", None, 50.0))
        .await
        .expect("undated");
    let second = test
        .create_course_exam(&course_id, &new_exam("Final", Some("2026-12-20"), 100.0))
        .await
        .expect("final");
    let first = test
        .create_course_exam(&course_id, &new_exam("Midterm", Some("2026-12-01"), 100.0))
        .await
        .expect("midterm");

    // Score exceeding max_score is rejected cleanly without writing.
    let over = test
        .upsert_my_exam_score(
            &course_id,
            &first.id,
            &alice,
            &ValidatedMyExamScore {
                score: 100.5,
                note: "too high".to_owned(),
            },
        )
        .await
        .expect("outcome");
    assert_eq!(
        over,
        ExamScoreOutcome::ExceedsMaxScore { max_score: 100.0 },
        "score exceeding max_score rejected"
    );

    // Snapshot publication progression and unjoined reader isolation.
    verify_snapshot_publication_progression(&test, &course_id, &first.id, [&alice, &bob, &carol])
        .await;

    let for_alice = test
        .list_course_exams(&course_id, ExamViewer::Manager(&alice))
        .await
        .expect("alice list");
    assert_eq!(
        for_alice.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
        [first.id.as_str(), second.id.as_str(), undated.id.as_str()],
        "dated ascending, undated last"
    );
    assert_eq!(for_alice[0].score_count, 8, "8 total active scorers");
    assert_eq!(
        for_alice[0].average_score, None,
        "unpublished again at end of progression"
    );
    let alice_mine = for_alice[0].my_score.as_ref().expect("alice score");
    assert_eq!(alice_mine.note, "alice private note", "alice note intact");
    assert!(
        (alice_mine.score - 90.0).abs() < 1e-9,
        "alice score preserved"
    );

    // Patching exam preserves scores and clears exam_date via Clear, and deletions clean up.
    verify_exam_patch_and_deletions(&test, &course_id, &first.id, &alice, &bob).await;
    test.teardown().await;
}

async fn verify_snapshot_publication_progression(
    test: &Store,
    course_id: &str,
    exam_id: &str,
    [alice, bob, carol]: [&String; 3],
) {
    for (uid, score, note) in [
        (alice, 90.0, "alice private note"),
        (bob, 70.0, "bob private note"),
        (carol, 80.0, "carol private note"),
    ] {
        let saved = test
            .upsert_my_exam_score(
                course_id,
                exam_id,
                uid,
                &ValidatedMyExamScore {
                    score,
                    note: note.to_owned(),
                },
            )
            .await
            .expect("score saved");
        assert!(
            matches!(saved, ExamScoreOutcome::Saved(_)),
            "valid score saved"
        );
    }

    let dave = common::user(test, "dave").await;
    let erin = common::user(test, "erin").await;
    let frank = common::user(test, "frank").await;
    let grace = common::user(test, "grace").await;
    let heidi = common::user(test, "heidi").await;
    let ws_id = test
        .get_container(course_id)
        .await
        .expect("course")
        .workspace_id;
    for extra in [&dave, &erin, &frank, &grace, &heidi] {
        common::works_in(test, &ws_id, extra).await;
        enroll(test, course_id, extra).await;
    }

    verify_unpublished_and_initial_publish(test, course_id, exam_id, [alice, bob, &dave, &erin])
        .await;
    verify_frozen_snapshot_republish_and_unpublish(
        test,
        course_id,
        exam_id,
        alice,
        bob,
        [&frank, &grace, &heidi],
    )
    .await;
}

async fn verify_unpublished_and_initial_publish(
    test: &Store,
    course_id: &str,
    exam_id: &str,
    [alice, bob, dave, erin]: [&String; 4],
) {
    for (viewer, expected_count) in [
        (ExamViewer::Manager(alice), 3_u32),
        (ExamViewer::Learner(bob), 0_u32),
        (ExamViewer::Learner(dave), 0_u32),
    ] {
        let listed = test
            .list_course_exams(course_id, viewer)
            .await
            .expect("unpublished list");
        assert_eq!(listed[0].score_count, expected_count, "unpublished count");
        assert_eq!(listed[0].average_score, None, "unpublished average is None");
        assert_eq!(
            listed[0].results_published_at, None,
            "unpublished timestamp is None"
        );
        assert_eq!(
            listed[0].published_score_bucket, None,
            "unpublished bucket is None"
        );
    }

    assert_eq!(
        test.publish_course_exam_results(course_id, exam_id, ExamViewer::Manager(alice))
            .await
            .expect("publish check"),
        Err(ExamPublishError::TooFewScorers {
            active_scorers: 3,
            minimum: 5,
        }),
        "3 scorers rejected by k-anonymity minimum"
    );
    test.upsert_my_exam_score(
        course_id,
        exam_id,
        dave,
        &ValidatedMyExamScore {
            score: 81.0,
            note: String::new(),
        },
    )
    .await
    .expect("4th scorer");
    assert_eq!(
        test.publish_course_exam_results(course_id, exam_id, ExamViewer::Manager(alice))
            .await
            .expect("4-scorer publish check"),
        Err(ExamPublishError::TooFewScorers {
            active_scorers: 4,
            minimum: 5,
        }),
        "4 scorers rejected by k-anonymity minimum"
    );

    // 5th scorer (`erin = 80.3`): mean = 401.3 / 5 = 80.26 -> rounded to 80.3, bucket = 5.
    test.upsert_my_exam_score(
        course_id,
        exam_id,
        erin,
        &ValidatedMyExamScore {
            score: 80.3,
            note: String::new(),
        },
    )
    .await
    .expect("5th scorer");
    let published = test
        .publish_course_exam_results(course_id, exam_id, ExamViewer::Manager(alice))
        .await
        .expect("publish call")
        .expect("5 scorers publish succeeds");
    assert!(
        published.results_published_at.is_some(),
        "published timestamp populated"
    );
    assert_eq!(published.published_score_bucket, Some(5), "bucket 5");
    assert!(
        (published.average_score.expect("published avg") - 80.3).abs() < 1e-9,
        "published average rounded to 1 decimal place"
    );
}

async fn verify_frozen_snapshot_republish_and_unpublish(
    test: &Store,
    course_id: &str,
    exam_id: &str,
    alice: &str,
    bob: &str,
    [frank, grace, heidi]: [&String; 3],
) {
    let for_reader = test
        .list_course_exams(course_id, ExamViewer::Unjoined)
        .await
        .expect("reader list");
    assert_eq!(for_reader[0].my_score, None, "reader has no personal score");
    assert_eq!(
        for_reader[0].average_score, None,
        "reader sees no published average"
    );
    assert_eq!(for_reader[0].score_count, 0, "reader sees zero score_count");
    assert_eq!(
        for_reader[0].results_published_at, None,
        "reader sees no published timestamp"
    );
    assert_eq!(
        for_reader[0].published_score_bucket, None,
        "reader sees no published bucket"
    );

    verify_sixth_scorer_freeze_and_unpublish_guard(test, course_id, exam_id, alice, bob, frank)
        .await;

    for extra in [grace, heidi] {
        test.upsert_my_exam_score(
            course_id,
            exam_id,
            extra,
            &ValidatedMyExamScore {
                score: 100.0,
                note: String::new(),
            },
        )
        .await
        .expect("7th/8th scorer");
    }
    let republished = test
        .publish_course_exam_results(course_id, exam_id, ExamViewer::Manager(alice))
        .await
        .expect("republish call")
        .expect("delta 3 republish succeeds");
    assert!(
        (republished.average_score.expect("republished avg") - 87.7).abs() < 1e-9,
        "republished average updated to 87.7"
    );
    assert_eq!(
        republished.published_score_bucket,
        Some(5),
        "bucket floor for 8 scorers is 5"
    );

    let unpublished = test
        .unpublish_course_exam_results(course_id, exam_id, ExamViewer::Manager(alice))
        .await
        .expect("unpublish succeeds");
    assert_eq!(unpublished.average_score, None, "unpublished average None");
    assert_eq!(
        unpublished.results_published_at, None,
        "unpublished timestamp None"
    );
    assert_eq!(
        unpublished.published_score_bucket, None,
        "unpublished bucket None"
    );
}

async fn verify_sixth_scorer_freeze_and_unpublish_guard(
    test: &Store,
    course_id: &str,
    exam_id: &str,
    alice: &str,
    bob: &str,
    frank: &str,
) {
    test.upsert_my_exam_score(
        course_id,
        exam_id,
        frank,
        &ValidatedMyExamScore {
            score: 100.0,
            note: String::new(),
        },
    )
    .await
    .expect("6th scorer");
    let frozen = test
        .list_course_exams(course_id, ExamViewer::Manager(alice))
        .await
        .expect("frozen list");
    assert_eq!(frozen[0].score_count, 6, "manager sees exact count 6");
    let learner_frozen = test
        .list_course_exams(course_id, ExamViewer::Learner(bob))
        .await
        .expect("learner frozen list");
    assert_eq!(
        learner_frozen[0].score_count, 5,
        "learner sees bucketed count 5, never exact 6"
    );
    assert!(
        (frozen[0].average_score.expect("frozen avg") - 80.3).abs() < 1e-9,
        "snapshot average remains frozen at 80.3"
    );
    assert_eq!(
        frozen[0].published_score_bucket,
        Some(5),
        "snapshot bucket remains frozen at 5"
    );
    assert_eq!(
        test.publish_course_exam_results(course_id, exam_id, ExamViewer::Manager(alice))
            .await
            .expect("republish check"),
        Err(ExamPublishError::TooFewChangesSincePublish {
            current_scorers: 6,
            previous_scorers: 5,
            minimum_delta: 3,
        }),
        "re-publish rejected when scorer delta < 3"
    );

    // Unpublish + immediate re-publish with 6 scorers (delta 1 < 3) MUST restore the
    // frozen 5-scorer snapshot (80.3), never recomputing the 6-scorer mean (83.6).
    let temp_unpub = test
        .unpublish_course_exam_results(course_id, exam_id, ExamViewer::Manager(alice))
        .await
        .expect("temp unpublish");
    assert_eq!(temp_unpub.average_score, None, "hidden while unpublished");
    let restored_frozen = test
        .publish_course_exam_results(course_id, exam_id, ExamViewer::Manager(alice))
        .await
        .expect("republish after unpublish")
        .expect("restores frozen snapshot");
    assert!(
        (restored_frozen.average_score.expect("restored avg") - 80.3).abs() < 1e-9,
        "unpublish + publish with delta < 3 restores frozen 80.3 without recomputing"
    );
    assert_eq!(
        restored_frozen.published_score_bucket,
        Some(5),
        "restored frozen bucket remains 5"
    );
}

async fn verify_exam_patch_and_deletions(
    test: &Store,
    course_id: &str,
    exam_id: &str,
    alice: &str,
    bob: &str,
) {
    let patched = test
        .patch_course_exam(
            course_id,
            exam_id,
            ExamViewer::Learner(bob),
            &CourseExamChange {
                title: Some("Midterm Revised".to_owned()),
                exam_date: ExamDateChange::Clear,
                max_score: Some(80.0),
                description: None,
            },
        )
        .await
        .expect("patched");
    assert_eq!(patched.title, "Midterm Revised", "title updated");
    assert_eq!(patched.exam_date, None, "exam_date cleared");
    assert!(
        (patched.max_score - 80.0).abs() < 1e-9,
        "max_score updated to 80.0"
    );
    assert_eq!(patched.score_count, 5, "learner patch view buckets 8 -> 5");
    assert_eq!(
        patched.my_score.as_ref().map(|s| s.note.as_str()),
        Some("bob private note"),
        "bob still sees his own note"
    );

    test.delete_my_exam_score(course_id, exam_id, alice)
        .await
        .expect("alice score deleted");
    let after_del = test
        .list_course_exams(course_id, ExamViewer::Manager(alice))
        .await
        .expect("listed");
    let first_view = after_del
        .iter()
        .find(|e| e.id == exam_id)
        .expect("first exam");
    assert_eq!(first_view.my_score, None, "alice score removed");
    assert_eq!(first_view.score_count, 7, "7 other scorers remain");
    assert_eq!(
        first_view.average_score, None,
        "unpublished average stays None"
    );

    test.delete_course_exam(course_id, exam_id)
        .await
        .expect("exam deleted");
    assert_eq!(
        test.count_rows("exam_scores").await.expect("count"),
        0,
        "deleting exam removes scores"
    );
}

/// Workspace removal, account erasure, and permanent course purge clean up `exam_scores`
/// and immediately exclude departed/erased users from averages and counts.
#[tokio::test]
async fn exam_scores_removed_on_workspace_removal_erasure_and_course_purge() {
    let test = common::setup().await;
    let owner = founder(&test).await;
    let bob = common::user(&test, "bob").await;
    let carol = common::user(&test, "carol").await;
    let dave = common::user(&test, "dave").await;
    common::works_in(&test, &test.workspace_id, &bob).await;
    common::works_in(&test, &test.workspace_id, &carol).await;
    common::works_in(&test, &test.workspace_id, &dave).await;
    let course_id = create_course(&test, &test.workspace_id, &owner).await;
    enroll(&test, &course_id, &bob).await;
    enroll(&test, &course_id, &carol).await;
    enroll(&test, &course_id, &dave).await;

    let exam = test
        .create_course_exam(&course_id, &new_exam("Quiz", Some("2026-12-05"), 100.0))
        .await
        .expect("exam");
    for (uid, score) in [(&owner, 80.0), (&bob, 60.0), (&carol, 100.0)] {
        test.upsert_my_exam_score(
            &course_id,
            &exam.id,
            uid,
            &ValidatedMyExamScore {
                score,
                note: format!("note for {uid}"),
            },
        )
        .await
        .expect("saved");
    }

    // With 3 scorers (owner=80, bob=60, carol=100) and unpublished results, average is None.
    let before = test
        .list_course_exams(&course_id, ExamViewer::Manager(&owner))
        .await
        .expect("before removal");
    assert_eq!(before[0].score_count, 3, "3 scorers before removal");
    assert_eq!(
        before[0].average_score, None,
        "unpublished average None before removal"
    );

    // Remove Bob from workspace -> Bob's score row deleted (`score_count == 2`).
    test.remove_workspace_member(&test.workspace_id, &bob, &owner)
        .await
        .expect("bob removed");
    let exams_owner = test
        .list_course_exams(&course_id, ExamViewer::Manager(&owner))
        .await
        .expect("listed for owner");
    assert_eq!(exams_owner[0].score_count, 2, "2 scorers after bob removed");
    assert_eq!(
        exams_owner[0].average_score, None,
        "unpublished average None for owner"
    );
    let exams_dave = test
        .list_course_exams(&course_id, ExamViewer::Learner(&dave))
        .await
        .expect("listed for dave");
    assert_eq!(
        exams_dave[0].score_count, 0,
        "learner sees bucketed 0 for 2 scorers"
    );
    assert_eq!(
        exams_dave[0].average_score, None,
        "unpublished average None for dave"
    );

    // Erase Carol -> Carol's score row deleted (`score_count == 1`).
    erase(&test, &carol, fvoci_store::Erasing::AtOnce, T0 + 1000).await;
    let exams_owner = test
        .list_course_exams(&course_id, ExamViewer::Manager(&owner))
        .await
        .expect("listed owner after erase");
    assert_eq!(
        exams_owner[0].score_count, 1,
        "1 scorer after carol erased (manager)"
    );
    let exams = test
        .list_course_exams(&course_id, ExamViewer::Learner(&dave))
        .await
        .expect("listed");
    assert_eq!(
        exams[0].score_count, 0,
        "learner sees bucketed 0 after carol erased"
    );
    assert_eq!(
        exams[0].average_score, None,
        "unpublished average None after carol erased"
    );

    // Trash & permanently purge course -> exam and remaining score deleted.
    test.delete_container(&course_id, &owner)
        .await
        .expect("trashed");
    test.purge_trash(i64::MAX).await.expect("purged");
    assert_eq!(
        test.count_rows("course_exams").await.expect("count"),
        0,
        "purging course deletes exams"
    );
    assert_eq!(
        test.count_rows("exam_scores").await.expect("count"),
        0,
        "purging course deletes exam scores"
    );
    test.teardown().await;
}

/// `upsert_my_exam_score` enforces active course enrollment atomically and rejects NaN,
/// `delete_course_exam` returns `NotFound` for malformed IDs, and `end_joining` removes
/// scores only on courses where the org admin has no remaining membership.
#[tokio::test]
async fn exam_score_enrollment_guard_nan_rejection_and_org_admin_end_joining() {
    let test = common::setup().await;
    let org_admin = founder(&test).await;
    let teacher = common::user(&test, "teacher").await;
    let unjoined = common::user(&test, "unjoined").await;
    let ws2 = common::workspace(&test).await;
    common::works_in(&test, &ws2, &teacher).await;
    common::works_in(&test, &ws2, &unjoined).await;

    let course_id = create_course(&test, &ws2, &teacher).await;
    let exam = test
        .create_course_exam(&course_id, &new_exam("Guard Exam", None, 100.0))
        .await
        .expect("exam");

    // Unjoined workspace member cannot store a score directly through the store.
    assert!(
        matches!(
            test.upsert_my_exam_score(
                &course_id,
                &exam.id,
                &unjoined,
                &ValidatedMyExamScore {
                    score: 50.0,
                    note: String::new(),
                },
            )
            .await,
            Err(fvoci_store::StoreError::NotFound(_))
        ),
        "unjoined member rejected by store enrollment guard"
    );
    // Non-finite score (NaN) is rejected even if validated struct is constructed directly.
    assert_eq!(
        test.upsert_my_exam_score(
            &course_id,
            &exam.id,
            &teacher,
            &ValidatedMyExamScore {
                score: f64::NAN,
                note: String::new(),
            },
        )
        .await
        .expect("outcome"),
        ExamScoreOutcome::ExceedsMaxScore { max_score: 100.0 },
        "NaN score rejected"
    );
    assert!(
        matches!(
            test.delete_course_exam(&course_id, "not-a-uuid").await,
            Err(fvoci_store::StoreError::NotFound(_))
        ),
        "malformed exam id returns NotFound"
    );

    // Org admin joins ws2 temporarily (prior = None), enrolls on course, records a score,
    // then ends joining -> score must be removed.
    verify_org_admin_joining_score_cleanup(&test, &ws2, &org_admin, &teacher, &course_id, &exam.id)
        .await;
    test.teardown().await;
}

async fn verify_org_admin_joining_score_cleanup(
    test: &Store,
    ws2: &str,
    org_admin: &str,
    teacher: &str,
    course_id: &str,
    exam_id: &str,
) {
    assert_eq!(
        test.join_workspace_as_org_admin(ws2, org_admin)
            .await
            .expect("joined ws2"),
        fvoci_store::Joining::Joined,
        "org admin joined ws2"
    );
    enroll(test, course_id, org_admin).await;
    test.upsert_my_exam_score(
        course_id,
        exam_id,
        org_admin,
        &ValidatedMyExamScore {
            score: 77.0,
            note: "temp".to_owned(),
        },
    )
    .await
    .expect("org admin score");
    assert!(
        test.leave_workspace_as_org_admin(ws2, org_admin)
            .await
            .expect("ended joining"),
        "ended temporary joining"
    );
    let exams = test
        .list_course_exams(course_id, ExamViewer::Manager(teacher))
        .await
        .expect("listed");
    assert_eq!(exams[0].score_count, 0, "temp joining score cleaned up");

    // Org admin with prior workspace membership (prior = Some) keeps score on enrolled
    // course when joining ends.
    test.add_workspace_member(ws2, org_admin, fvoci_model::WorkspaceRole::Member)
        .await
        .expect("prior ws2 member");
    let enrolled_course = create_course(test, ws2, teacher).await;
    enroll(test, &enrolled_course, org_admin).await;
    let enrolled_exam = test
        .create_course_exam(&enrolled_course, &new_exam("Kept", None, 100.0))
        .await
        .expect("kept exam");
    assert_eq!(
        test.join_workspace_as_org_admin(ws2, org_admin)
            .await
            .expect("re-joined ws2"),
        fvoci_store::Joining::Joined,
        "org admin re-joined ws2"
    );
    test.upsert_my_exam_score(
        &enrolled_course,
        &enrolled_exam.id,
        org_admin,
        &ValidatedMyExamScore {
            score: 88.0,
            note: "kept".to_owned(),
        },
    )
    .await
    .expect("kept score");
    assert!(
        test.leave_workspace_as_org_admin(ws2, org_admin)
            .await
            .expect("ended second joining"),
        "ended second joining"
    );
    let manager_kept = test
        .list_course_exams(&enrolled_course, ExamViewer::Manager(teacher))
        .await
        .expect("listed manager kept");
    assert_eq!(
        manager_kept[0].score_count, 1,
        "enrolled prior-member score kept"
    );
    let kept = test
        .list_course_exams(&enrolled_course, ExamViewer::Learner(org_admin))
        .await
        .expect("listed kept");
    assert_eq!(kept[0].score_count, 0, "learner sees bucketed 0 for 1");
    assert_eq!(
        kept[0].my_score.as_ref().map(|s| s.note.as_str()),
        Some("kept"),
        "enrolled prior-member personal note kept"
    );
}

/// How the three scorers who came after the first snapshot leave again.
#[derive(Debug, Clone, Copy)]
enum Leaving {
    /// They delete their own scores.
    DeleteTheirScores,
    /// The course administrator removes them from the course; their scores stay behind
    /// but no longer count.
    RemovedFromTheCourse,
}

/// A course with an administrator who never scores, and the ten scorers of the
/// snapshot tests: A to E, then X, then P, Q and R.
struct SnapshotCourse {
    admin: String,
    course_id: String,
    exam_id: String,
    first_five: Vec<String>,
    x: String,
    came_and_went: Vec<String>,
}

async fn snapshot_course(test: &Store, workspace_id: &str) -> SnapshotCourse {
    let admin = common::user(test, "admin").await;
    common::works_in(test, workspace_id, &admin).await;
    let course_id = create_course(test, workspace_id, &admin).await;
    let mut scorers = Vec::new();
    for name in ["a", "b", "c", "d", "e", "x", "p", "q", "r"] {
        let user = common::user(test, name).await;
        common::works_in(test, workspace_id, &user).await;
        enroll(test, &course_id, &user).await;
        scorers.push(user);
    }
    let exam = test
        .create_course_exam(&course_id, &new_exam("Midterm", None, 100.0))
        .await
        .expect("exam");
    let came_and_went = scorers.split_off(6);
    let x = scorers.pop().expect("x is the sixth");
    SnapshotCourse {
        admin,
        course_id,
        exam_id: exam.id,
        first_five: scorers,
        x,
        came_and_went,
    }
}

async fn score(test: &Store, course: &SnapshotCourse, user: &str, score: f64) {
    test.upsert_my_exam_score(
        &course.course_id,
        &course.exam_id,
        user,
        &ValidatedMyExamScore {
            score,
            note: String::new(),
        },
    )
    .await
    .expect("score recorded");
}

#[track_caller]
fn assert_average(average: Option<f64>, expected: f64, what: &str) {
    let average = average.unwrap_or_else(|| panic!("{what}: no average"));
    assert!(
        (average - expected).abs() < 1e-9,
        "{what}: {average} is not {expected}"
    );
}

/// The scorer count the stored snapshot counted, which no view shows.
async fn snapshot_scorers(test: &Store, exam_id: &str) -> String {
    let counted = test
        .texts_unchecked(&format!(
            "SELECT CAST(published_submission_count AS TEXT) FROM course_exams WHERE id = '{exam_id}'"
        ))
        .await
        .expect("the snapshot is read");
    assert_eq!(counted.len(), 1, "one exam row");
    counted.into_iter().next().unwrap_or_default()
}

/// Publishes 5 scorers, then 9, then lets 3 of them leave: the third snapshot would
/// count 6, one scorer more than the first, and give X's score away as
/// `6 × avg₃ − 5 × avg₁`. The count fell, so it is refused and the 9-scorer snapshot
/// stays.
async fn a_snapshot_is_not_taken_one_scorer_apart(leaving: Leaving) {
    let test = common::setup().await;
    let course = snapshot_course(&test, &test.workspace_id).await;
    let publish = async || {
        test.publish_course_exam_results(
            &course.course_id,
            &course.exam_id,
            ExamViewer::Manager(&course.admin),
        )
        .await
        .expect("publish call")
    };

    for (user, points) in course.first_five.iter().zip([70.0, 80.0, 90.0, 60.0, 75.0]) {
        score(&test, &course, user, points).await;
    }
    let first = publish().await.expect("5 scorers publish");
    assert_average(first.average_score, 75.0, "first snapshot average");
    score(&test, &course, &course.x, 37.0).await;
    for user in &course.came_and_went {
        score(&test, &course, user, 50.0).await;
    }
    let second = publish().await.expect("9 scorers publish, 4 more than 5");
    // (375 + 37 + 150) / 9 = 62.44…
    assert_average(second.average_score, 62.4, "second snapshot average");
    assert_eq!(
        snapshot_scorers(&test, &course.exam_id).await,
        "9",
        "the second snapshot counts 9"
    );

    for user in &course.came_and_went {
        match leaving {
            Leaving::DeleteTheirScores => test
                .delete_my_exam_score(&course.course_id, &course.exam_id, user)
                .await
                .expect("score deleted"),
            Leaving::RemovedFromTheCourse => test
                .delete_member(&course.course_id, user)
                .await
                .expect("member removed"),
        }
    }
    assert_eq!(
        publish().await,
        Err(ExamPublishError::TooFewChangesSincePublish {
            current_scorers: 6,
            previous_scorers: 9,
            minimum_delta: 3,
        }),
        "a count that fell is no change to publish ({leaving:?})"
    );
    let kept = test
        .list_course_exams(&course.course_id, ExamViewer::Manager(&course.admin))
        .await
        .expect("listed");
    assert_eq!(
        kept[0].score_count, 6,
        "the manager sees the 6 who count now"
    );
    assert_average(kept[0].average_score, 62.4, "the 9-scorer average stays");
    assert_eq!(
        kept[0].published_score_bucket,
        Some(5),
        "the 9-scorer bucket stays"
    );
    assert_eq!(
        snapshot_scorers(&test, &course.exam_id).await,
        "9",
        "the snapshot still counts 9"
    );

    // Hiding the results and showing them again gives the frozen snapshot back, and
    // does not count again.
    let hidden = test
        .unpublish_course_exam_results(
            &course.course_id,
            &course.exam_id,
            ExamViewer::Manager(&course.admin),
        )
        .await
        .expect("unpublished");
    assert_eq!(hidden.average_score, None, "hidden while unpublished");
    let restored = publish().await.expect("the frozen snapshot is shown again");
    assert_average(restored.average_score, 62.4, "the 9-scorer average again");
    assert_eq!(
        restored.published_score_bucket,
        Some(5),
        "the 9-scorer bucket"
    );
    assert!(restored.results_published_at.is_some(), "published again");
    assert_eq!(
        snapshot_scorers(&test, &course.exam_id).await,
        "9",
        "showing it again did not count 6"
    );

    climb_back(&test, &test.workspace_id, &course, leaving).await;
    test.teardown().await;
}

/// Scorers come after the count fell from 9 to 6 (`leaving` them): with 11 the results
/// are not counted again, 11 being too few more than the 9 the snapshot counted; with
/// 12 they are, in a new snapshot.
async fn climb_back(test: &Store, workspace_id: &str, course: &SnapshotCourse, leaving: Leaving) {
    let mut newcomers = Vec::new();
    for name in ["s", "t", "u", "v", "w", "y"] {
        let user = common::user(test, name).await;
        common::works_in(test, workspace_id, &user).await;
        enroll(test, &course.course_id, &user).await;
        newcomers.push(user);
    }
    let (eleventh, twelfth) = newcomers.split_at(5);
    for user in eleventh {
        score(test, course, user, 80.0).await;
    }
    assert_eq!(
        publish(test, course).await,
        Err(ExamPublishError::TooFewChangesSincePublish {
            current_scorers: 11,
            previous_scorers: 9,
            minimum_delta: 3,
        }),
        "11 is not 3 more than the 9 counted ({leaving:?})"
    );
    score(test, course, &twelfth[0], 80.0).await;
    let third = publish(test, course)
        .await
        .expect("12 scorers publish, 3 more than 9");
    // (412 + 6 * 80) / 12 = 74.33…
    assert_average(third.average_score, 74.3, "the 12-scorer average");
    assert_eq!(
        snapshot_scorers(test, &course.exam_id).await,
        "12",
        "the third snapshot counts 12"
    );
}

/// Publishes the results of the course's exam as its manager.
async fn publish(
    test: &Store,
    course: &SnapshotCourse,
) -> Result<fvoci_model::CourseExam, ExamPublishError> {
    test.publish_course_exam_results(
        &course.course_id,
        &course.exam_id,
        ExamViewer::Manager(&course.admin),
    )
    .await
    .expect("publish call")
}

#[tokio::test]
async fn deleted_scores_do_not_take_a_snapshot_one_scorer_apart() {
    a_snapshot_is_not_taken_one_scorer_apart(Leaving::DeleteTheirScores).await;
}

#[tokio::test]
async fn removed_members_do_not_take_a_snapshot_one_scorer_apart() {
    a_snapshot_is_not_taken_one_scorer_apart(Leaving::RemovedFromTheCourse).await;
}
