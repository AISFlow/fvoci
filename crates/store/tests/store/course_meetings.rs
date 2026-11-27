//! Store tests for course study meetings, attendance, course purge,
//! account erasure tombstones, and upcoming-meeting notifications.

use fvoci_model::{
    AttendanceStatus, Container, ContainerKind, ContainerMember, ContainerVisibility, CourseLimits,
    CourseMeetingChange, NewCourseMeeting, NewUnit, NotificationKind, OptionalIdChange,
    WorkspaceRole, new_id,
};
use fvoci_store::{DbType, Paging, Store};

use crate::account_erasure::{T0, erase, founder};
use crate::common::{self, TestStore};

async fn create_course(store: &Store, workspace_id: &str, admin: &str) -> String {
    let mut course = Container {
        id: new_id(),
        workspace_id: workspace_id.to_owned(),
        visibility: ContainerVisibility::Private,
        kind: ContainerKind::Course,
        title: "Rust Study".to_owned(),
        ..Container::default()
    };
    store
        .insert_container_with_admin(&mut course, admin)
        .await
        .expect("course created");
    course.id
}

async fn enroll(store: &Store, course_id: &str, user_id: &str, role: fvoci_model::ContainerRole) {
    let member = ContainerMember::new(course_id.to_owned(), user_id.to_owned(), role);
    store.save_member(&member).await.expect("member enrolled");
}

fn new_meeting(title: &str, starts_at: i64) -> fvoci_model::ValidatedNewMeeting {
    NewCourseMeeting {
        title: title.to_owned(),
        starts_at,
        location: "https://meet.example/study".to_owned(),
        unit_ids: Vec::new(),
        presenter_id: None,
        host_id: None,
        notes: "Bring questions".to_owned(),
    }
    .check_request()
    .expect("valid meeting")
}

/// Full meeting CRUD, chronological ordering, attendance upsert, non-member 404,
/// trash preservation, and permanent purge cleanup.
#[tokio::test]
async fn meetings_crud_attendance_trash_and_purge() {
    let test = common::setup().await;
    let alice = common::user(&test, "alice").await;
    let bob = common::user(&test, "bob").await;
    let outsider = common::user(&test, "outsider").await;
    common::works_in(&test, &test.workspace_id, &alice).await;
    common::works_in(&test, &test.workspace_id, &bob).await;
    common::works_in(&test, &test.workspace_id, &outsider).await;

    let course_id = create_course(&test, &test.workspace_id, &alice).await;
    enroll(&test, &course_id, &bob, fvoci_model::ContainerRole::Viewer).await;
    let unit = test
        .create_unit(
            &course_id,
            &NewUnit {
                title: "Unit 1".to_owned(),
                ..NewUnit::default()
            },
            CourseLimits::DEFAULT,
            &alice,
        )
        .await
        .expect("unit")
        .unit;

    let mut late_req = new_meeting("Session 2", T0 + 7_200_000);
    late_req.unit_ids = vec![unit.id.clone()];
    late_req.presenter_id = Some(bob.clone());
    late_req.host_id = Some(alice.clone());
    let m2 = test
        .create_course_meeting(&course_id, &late_req)
        .await
        .expect("m2");
    let m1 = test
        .create_course_meeting(&course_id, &new_meeting("Session 1", T0 + 3_600_000))
        .await
        .expect("m1");

    // Non-member cannot be assigned as presenter or receive attendance.
    let mut bad_presenter = new_meeting("Bad", T0 + 1000);
    bad_presenter.presenter_id = Some(outsider.clone());
    assert!(
        test.create_course_meeting(&course_id, &bad_presenter)
            .await
            .expect_err("outsider presenter rejected")
            .is_not_found()
    );
    assert!(
        test.upsert_meeting_attendance(&course_id, &m1.id, &outsider, AttendanceStatus::Present)
            .await
            .expect_err("outsider attendance rejected")
            .is_not_found()
    );

    test.upsert_meeting_attendance(&course_id, &m1.id, &bob, AttendanceStatus::Late)
        .await
        .expect("bob late");
    test.upsert_meeting_attendance(&course_id, &m1.id, &alice, AttendanceStatus::Present)
        .await
        .expect("alice present");
    test.upsert_meeting_attendance(&course_id, &m1.id, &bob, AttendanceStatus::Excused)
        .await
        .expect("bob updated to excused");

    let listed = test.list_course_meetings(&course_id).await.expect("listed");
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].id, m1.id, "chronological order");
    assert_eq!(listed[1].id, m2.id);
    assert_eq!(listed[0].attendance.len(), 2);
    let bob_att = listed[0]
        .attendance
        .iter()
        .find(|a| a.user_id == bob)
        .expect("bob attendance");
    assert_eq!(bob_att.status, AttendanceStatus::Excused);

    // Patching meeting updates fields and clears presenter/host via Clear.
    let patched = test
        .patch_course_meeting(
            &course_id,
            &m2.id,
            &CourseMeetingChange {
                title: Some("Session 2 Revised".to_owned()),
                presenter_id: OptionalIdChange::Clear,
                ..CourseMeetingChange::default()
            },
        )
        .await
        .expect("patched");
    assert_eq!(patched.title, "Session 2 Revised");
    assert_eq!(patched.presenter_id, None);
    assert_eq!(patched.host_id.as_deref(), Some(alice.as_str()));

    verify_meeting_trash_and_purge(&test, &course_id, &alice, &m1.id, &m2.id).await;
    test.teardown().await;
}

async fn verify_meeting_trash_and_purge(
    test: &Store,
    course_id: &str,
    alice: &str,
    m1_id: &str,
    m2_id: &str,
) {
    // Soft-deleting and restoring the course preserves meetings and attendance.
    test.delete_container(course_id, alice)
        .await
        .expect("trashed");
    assert_eq!(
        test.list_course_meetings(course_id)
            .await
            .expect("empty while trashed"),
        Vec::<fvoci_model::CourseMeeting>::new(),
        "trashed course lists no meetings"
    );
    test.undelete_container(course_id, alice)
        .await
        .expect("restored");
    assert_eq!(
        test.list_course_meetings(course_id)
            .await
            .expect("restored")
            .len(),
        2,
        "restoring course brings back meetings"
    );

    // Deleting one meeting removes its attendance too.
    test.delete_course_meeting(course_id, m1_id)
        .await
        .expect("m1 deleted");
    assert_eq!(
        test.count_rows("meeting_attendance").await.expect("count"),
        0,
        "deleting meeting removes its attendance"
    );

    // Permanent purge of trashed course removes remaining meetings & attendance.
    test.upsert_meeting_attendance(course_id, m2_id, alice, AttendanceStatus::Present)
        .await
        .expect("m2 attendance");
    test.delete_container(course_id, alice)
        .await
        .expect("trashed again");
    test.purge_trash(i64::MAX).await.expect("purged");
    assert_eq!(
        test.count_rows("course_meetings").await.expect("count"),
        0,
        "purge removes course_meetings"
    );
    assert_eq!(
        test.count_rows("meeting_attendance").await.expect("count"),
        0,
        "purge removes meeting_attendance"
    );
}

/// Upcoming meeting notifications fire once for active enrolled members only, skipping
/// unjoined members, guests without course membership, removed members, meetings outside
/// the 24h window, and personal-space courses.
#[tokio::test]
async fn notify_upcoming_meetings_targets_active_members_once_and_skips_others() {
    let test = common::setup().await;
    let admin = common::user(&test, "admin").await;
    let member = common::user(&test, "member").await;
    let unjoined = common::user(&test, "unjoined").await;
    let guest = common::user(&test, "guest").await;
    for uid in [&admin, &member, &unjoined, &guest] {
        common::works_in(&test, &test.workspace_id, uid).await;
    }
    demote_to_guest(&test, &guest).await;

    let mut course = Container {
        id: new_id(),
        workspace_id: test.workspace_id.clone(),
        visibility: ContainerVisibility::Open,
        kind: ContainerKind::Course,
        title: "Open Course".to_owned(),
        ..Container::default()
    };
    test.insert_container_with_admin(&mut course, &admin)
        .await
        .expect("open course");
    enroll(
        &test,
        &course.id,
        &member,
        fvoci_model::ContainerRole::Viewer,
    )
    .await;

    let window = 24 * 60 * 60 * 1000;
    let due = test
        .create_course_meeting(&course.id, &new_meeting("Due Soon", T0 + 3_600_000))
        .await
        .expect("due meeting");
    let _far = test
        .create_course_meeting(&course.id, &new_meeting("Too Far", T0 + window + 60_000))
        .await
        .expect("far meeting");
    let _past = test
        .create_course_meeting(&course.id, &new_meeting("Already Started", T0 - 1_000))
        .await
        .expect("past meeting");

    // Also create a due meeting in admin's personal space: must produce 0 notifications.
    seed_own_space_meeting(&test, &admin).await;

    let system = test.system();
    let sent = system
        .notify_upcoming_meetings(T0, window)
        .await
        .expect("first run");
    let mut recipients: Vec<&str> = sent.iter().map(|s| s.recipient_id.as_str()).collect();
    recipients.sort_unstable();
    let mut expected = vec![admin.as_str(), member.as_str()];
    expected.sort_unstable();
    assert_eq!(
        recipients, expected,
        "only active enrolled members notified"
    );
    assert!(
        system
            .notify_upcoming_meetings(T0, window)
            .await
            .expect("second run")
            .is_empty(),
        "zero duplicate notifications"
    );

    let rows = test
        .notifications_of_user(
            password_reader(&member),
            Paging {
                page: 0,
                per_page: std::num::NonZeroU32::new(50).expect("a page size"),
            },
        )
        .await
        .expect("notifications read");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].kind, NotificationKind::MeetingUpcoming);
    assert_eq!(rows[0].block_id, due.id);
    assert_eq!(rows[0].container_id, course.id);
    for uid in [&unjoined, &guest] {
        assert_eq!(
            test.unread_notifications_of_user(password_reader(uid))
                .await
                .expect("unread"),
            0
        );
    }

    test.teardown().await;
}

async fn demote_to_guest(test: &TestStore, user_id: &str) {
    let changed = test
        .execute_unchecked(&format!(
            "UPDATE workspace_members SET role = '{}' \
             WHERE workspace_id = '{}' AND user_id = '{user_id}'",
            WorkspaceRole::Guest.as_str(),
            test.workspace_id
        ))
        .await
        .expect("guest role set");
    assert_eq!(changed, 1, "guest must exist in workspace_members");
}

fn password_reader(user_id: &str) -> fvoci_store::NotificationReader<'_> {
    fvoci_store::NotificationReader {
        user_id,
        making: fvoci_store::Making::with_password(),
    }
}

async fn seed_own_space_meeting(test: &TestStore, user_id: &str) {
    let own_ws = common::own_workspace(test, user_id).await;
    let own_org = test
        .system()
        .texts_unchecked(&format!(
            "SELECT CAST(org_id AS TEXT) FROM workspaces WHERE id = '{own_ws}'"
        ))
        .await
        .expect("own org")[0]
        .clone();
    let own_store = test.for_org(&own_org).expect("own scope");
    let own_course = create_course(&own_store, &own_ws, user_id).await;
    own_store
        .create_course_meeting(&own_course, &new_meeting("Own Study", T0 + 1_800_000))
        .await
        .expect("own meeting");
}

/// Account erasure preserves `meeting_attendance` rows as tombstones pointing to the
/// erased user's identifier.
#[tokio::test]
async fn account_erasure_preserves_meeting_attendance_as_tombstone() {
    let test = common::setup().await;
    let owner = founder(&test).await;
    let leaver = common::user(&test, "leaver").await;
    common::works_in(&test, &test.workspace_id, &leaver).await;
    let course_id = create_course(&test, &test.workspace_id, &owner).await;
    enroll(
        &test,
        &course_id,
        &leaver,
        fvoci_model::ContainerRole::Viewer,
    )
    .await;

    let meeting = test
        .create_course_meeting(&course_id, &new_meeting("Session", T0 + 1000))
        .await
        .expect("meeting");
    test.upsert_meeting_attendance(&course_id, &meeting.id, &leaver, AttendanceStatus::Present)
        .await
        .expect("attendance");

    erase(&test, &leaver, fvoci_store::Erasing::AtOnce, T0 + 2000).await;

    let meetings = test.list_course_meetings(&course_id).await.expect("listed");
    assert_eq!(meetings.len(), 1);
    assert_eq!(meetings[0].attendance.len(), 1);
    assert_eq!(meetings[0].attendance[0].user_id, leaver);
    assert_eq!(meetings[0].attendance[0].status, AttendanceStatus::Present);

    if common::db_type() == DbType::Postgres {
        assert_eq!(
            test.count_rows("meeting_attendance").await.expect("count"),
            1
        );
    }
    test.teardown().await;
}

/// Patching `starts_at` resets `notified_at` so a rescheduled meeting reminds again,
/// whereas editing only title/notes preserves `notified_at`. Malformed IDs return
/// `NotFound`, and `meeting_notification_places` enforces org scope on both backends.
#[tokio::test]
async fn reschedule_resets_notified_at_and_places_respect_org_scope() {
    let test = common::setup().await;
    let admin = common::user(&test, "admin").await;
    common::works_in(&test, &test.workspace_id, &admin).await;
    let course_id = create_course(&test, &test.workspace_id, &admin).await;
    let window = 24 * 60 * 60 * 1000;
    let meeting = test
        .create_course_meeting(&course_id, &new_meeting("Session 1", T0 + 3_600_000))
        .await
        .expect("meeting");

    let first = test
        .notify_upcoming_meetings(T0, window)
        .await
        .expect("first notify");
    assert_eq!(first.len(), 1, "first reminder sent");

    // Editing title alone preserves `notified_at` -> no second reminder.
    test.patch_course_meeting(
        &course_id,
        &meeting.id,
        &CourseMeetingChange {
            title: Some("Session 1 Renamed".to_owned()),
            ..CourseMeetingChange::default()
        },
    )
    .await
    .expect("title patched");
    assert!(
        test.notify_upcoming_meetings(T0, window)
            .await
            .expect("second notify")
            .is_empty(),
        "title edit does not re-notify"
    );

    // Rescheduling `starts_at` resets `notified_at` -> reminder fires again.
    test.patch_course_meeting(
        &course_id,
        &meeting.id,
        &CourseMeetingChange {
            starts_at: Some(T0 + 7_200_000),
            ..CourseMeetingChange::default()
        },
    )
    .await
    .expect("starts_at patched");
    let third = test
        .notify_upcoming_meetings(T0, window)
        .await
        .expect("third notify");
    assert_eq!(third.len(), 1, "rescheduled meeting notifies again");

    // Malformed meeting/user IDs return NotFound consistently.
    assert!(matches!(
        test.delete_course_meeting(&course_id, "not-a-uuid").await,
        Err(fvoci_store::StoreError::NotFound(_))
    ));
    assert!(matches!(
        test.upsert_meeting_attendance(
            &course_id,
            &meeting.id,
            "not-a-uuid",
            AttendanceStatus::Present
        )
        .await,
        Err(fvoci_store::StoreError::NotFound(_))
    ));

    // `meeting_notification_places` filters by org_id even on SQLite.
    let own_ws = common::own_workspace(&test, &admin).await;
    let own_org = test
        .system()
        .texts_unchecked(&format!(
            "SELECT CAST(org_id AS TEXT) FROM workspaces WHERE id = '{own_ws}'"
        ))
        .await
        .expect("own org")[0]
        .clone();
    let own_store = test.for_org(&own_org).expect("own scope");
    assert!(
        own_store
            .meeting_notification_places(&[meeting.id.as_str()])
            .await
            .expect("places")
            .is_empty(),
        "another org handle cannot resolve this meeting"
    );
    assert_eq!(
        test.meeting_notification_places(&[meeting.id.as_str()])
            .await
            .expect("places")
            .len(),
        1,
        "owning org resolves its meeting"
    );
    test.teardown().await;
}
