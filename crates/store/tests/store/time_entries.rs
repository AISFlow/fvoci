//! Store-level tests for `time_entries`: single active timer atomic
//! transitions (same-org, cross-org, and PostgreSQL Gate concurrency), closed-organization
//! filtering, and cleanup on organization departure and account erasure.

use std::fmt::Write as _;

use fvoci_model::{
    MAX_TIME_MILLIS, OrgRole, TimeEntryKind, TimeEntryPatch, TimeTargetKind, new_id,
};
use fvoci_store::{
    DbType, Departure, Erasing, ErasureEnd, ErasureStart, Making, NewStoredTimeEntry, Paging,
    Store, TimeEntryFilter, TimeEntryPatchOutcome, TimeReader,
};

use crate::common::{self, TestStore, db_type};
use crate::lock_gate::{Gate, ended};

fn password_reader(user_id: &str) -> TimeReader<'_> {
    TimeReader {
        user_id,
        making: Making::with_password(),
    }
}

fn sample_entry<'a>(
    workspace_id: Option<&'a str>,
    kind: TimeEntryKind,
    note: &'a str,
) -> NewStoredTimeEntry<'a> {
    NewStoredTimeEntry {
        workspace_id,
        container_id: None,
        block_id: None,
        target_kind: None,
        target_title: None,
        kind,
        note,
    }
}

async fn count_active_for_user(store: &Store, user_id: &str) -> usize {
    store
        .system()
        .texts_unchecked(&format!(
            "SELECT CAST(id AS TEXT) FROM time_entries WHERE user_id = '{user_id}' \
             AND ended_at IS NULL"
        ))
        .await
        .expect("read active rows")
        .len()
}

async fn count_all_for_user(store: &Store, user_id: &str) -> usize {
    store
        .system()
        .texts_unchecked(&format!(
            "SELECT CAST(id AS TEXT) FROM time_entries WHERE user_id = '{user_id}'"
        ))
        .await
        .expect("read user rows")
        .len()
}

/// Starting a timer in one organization while another is running in the user's personal
/// space (and vice versa) atomically stops the previous timer and leaves one active timer.
#[tokio::test]
async fn cross_organization_atomic_timer_switch_keeps_one_active() {
    let test = common::setup().await;
    let user_id = common::user(&test, "switcher").await;
    let (own_store, own_ws) = common::in_own_space(&test, &user_id).await;
    let own_org = own_store.tenant().org_id().expect("own org id");
    let reader = password_reader(&user_id);

    let first = test
        .start_time_entry(
            reader,
            &own_org,
            &sample_entry(Some(&own_ws), TimeEntryKind::Study, "own space"),
            1_000,
        )
        .await
        .expect("start in personal space");
    assert_eq!(first.stopped, None, "no prior timer to stop");
    assert_eq!(first.entry.ended_at, None, "first timer is active");

    // Start in shared org with clock skew (900 < 1_000): clamps to >= started_at.
    let second = test
        .start_time_entry(
            reader,
            &test.org_id,
            &sample_entry(Some(&test.workspace_id), TimeEntryKind::Work, "shared org"),
            900,
        )
        .await
        .expect("switch to shared org");
    let stopped_first = second.stopped.expect("previous personal timer stopped");
    assert_eq!(stopped_first.id, first.entry.id, "stopped first timer");
    assert_eq!(stopped_first.ended_at, Some(1_000), "clamped stop time");
    assert_eq!(
        second.entry.started_at, 900,
        "new timer keeps caller timestamp without leaking prior start"
    );
    assert_eq!(
        count_active_for_user(&test, &user_id).await,
        1,
        "one active after cross-org switch"
    );

    let third = test
        .start_time_entry(
            reader,
            &own_org,
            &sample_entry(None, TimeEntryKind::Lecture, "back home"),
            5_000,
        )
        .await
        .expect("switch back to personal space");
    let stopped_second = third.stopped.expect("previous shared org timer stopped");
    assert_eq!(stopped_second.id, second.entry.id, "stopped second timer");
    assert_eq!(stopped_second.ended_at, Some(5_000), "second stop time");

    assert_patch_stop_and_summary(&test, reader, &user_id, &third.entry.id).await;
    test.teardown().await;
}

async fn assert_patch_stop_and_summary(
    test: &TestStore,
    reader: TimeReader<'_>,
    user_id: &str,
    active_id: &str,
) {
    let bad = test
        .patch_time_entry(
            reader,
            active_id,
            &TimeEntryPatch {
                ended_at: Some(4_999),
                ..TimeEntryPatch::default()
            },
            6_000,
        )
        .await
        .expect("patch ran");
    assert_eq!(
        bad,
        TimeEntryPatchOutcome::EndedBeforeStarted,
        "patch ending before start is rejected"
    );
    assert_eq!(
        count_active_for_user(test, user_id).await,
        1,
        "rejected patch leaves timer running"
    );

    let patched = test
        .patch_time_entry(
            reader,
            active_id,
            &TimeEntryPatch {
                ended_at: Some(6_000),
                note: Some("finished".to_owned()),
                ..TimeEntryPatch::default()
            },
            6_000,
        )
        .await
        .expect("patch ran");
    assert!(
        matches!(patched, TimeEntryPatchOutcome::Updated(ref e) if e.ended_at == Some(6_000)),
        "patch with ended_at stops active entry"
    );
    assert_eq!(
        count_active_for_user(test, user_id).await,
        0,
        "no active timers remain after patch stop"
    );

    let listed = test
        .list_time_entries(
            reader,
            &TimeEntryFilter::default(),
            Paging {
                page: 0,
                per_page: std::num::NonZeroU32::new(10).expect("a page size"),
            },
        )
        .await
        .expect("list");
    assert_eq!(listed.entries.len(), 3, "three entries listed");
    assert_eq!(listed.summary.count, 3, "summary counts all three entries");
    // first: 0ms (1000..1000), second: 4100ms (900..5000), third: 1000ms (5000..6000)
    assert_eq!(listed.summary.total_ms, 5_100, "summary sums durations");
    assert_eq!(listed.summary.by_kind.work, 4_100, "work duration matches");
    assert_eq!(
        listed.summary.by_kind.lecture, 1_000,
        "lecture duration matches"
    );
}

/// Summing durations that exceed `i64::MAX` saturates cleanly to `i64::MAX` on both
/// SQLite (no `SQLITE_ERROR: integer overflow`) and PostgreSQL.
#[tokio::test]
async fn summary_saturates_at_i64_max_without_sqlite_overflow() {
    let test = common::setup().await;
    let user_id = common::user(&test, "marathon_user").await;
    // 1,100 * 8.64e15 = 9.504e18 > i64::MAX (9.223e18).
    let rows = 1_100_usize;
    let mut values = String::new();
    for i in 0..rows {
        if i > 0 {
            values.push(',');
        }
        let _ = write!(
            values,
            "('{}', '{}', '{user_id}', 'work', 0, {MAX_TIME_MILLIS}, '', 0, 0)",
            new_id(),
            test.org_id,
        );
    }
    test.execute_unchecked(&format!(
        "INSERT INTO time_entries (id, org_id, user_id, kind, started_at, ended_at, note, \
         created_at, updated_at) VALUES {values}"
    ))
    .await
    .expect("seed large durations");

    let listed = test
        .list_time_entries(
            password_reader(&user_id),
            &TimeEntryFilter::default(),
            Paging {
                page: 0,
                per_page: std::num::NonZeroU32::new(5).expect("a page size"),
            },
        )
        .await
        .expect("summary saturates instead of overflowing");
    assert_eq!(listed.entries.len(), 5, "page size respected");
    assert_eq!(
        listed.summary.count,
        i64::try_from(rows).expect("fits i64"),
        "all rows counted"
    );
    assert_eq!(
        listed.summary.by_kind.work,
        i64::MAX,
        "work total saturated to i64::MAX"
    );
    assert_eq!(
        listed.summary.total_ms,
        i64::MAX,
        "overall total saturated to i64::MAX"
    );

    test.teardown().await;
}

/// PostgreSQL Gate concurrency test: `start_time_entry` racing with `stop_time_entry`
/// serializes on the user's row lock so `stop` sees and stops the newly started timer.
#[tokio::test]
async fn concurrent_start_and_stop_serialize_behind_gate() {
    if db_type() != DbType::Postgres {
        return;
    }
    let test = common::setup().await;
    let user_id = common::user(&test, "start_stop_racer").await;

    let mut gate = Gate::before(&test, "INSERT", "time_entries", "NEW.ended_at IS NULL").await;
    let second_store = gate.second_store(&test).await;

    let (s1, u1, org1, ws1) = (
        test.store.clone(),
        user_id.clone(),
        test.org_id.clone(),
        test.workspace_id.clone(),
    );
    let start_task = tokio::spawn(async move {
        s1.start_time_entry(
            password_reader(&u1),
            &org1,
            &sample_entry(Some(&ws1), TimeEntryKind::Work, "starting"),
            10_000,
        )
        .await
    });
    let holder = gate.stopped(&start_task).await;

    let u2 = user_id.clone();
    let stop_task = tokio::spawn(async move {
        second_store
            .stop_time_entry(password_reader(&u2), 12_000)
            .await
    });

    gate.waits_for(holder, &stop_task).await;
    gate.open().await;

    let started = ended(start_task).await.expect("start succeeds");
    let stopped = ended(stop_task)
        .await
        .expect("stop succeeds")
        .expect("stop observes committed active timer");
    assert_eq!(stopped.id, started.entry.id, "stop closed started timer");
    assert_eq!(stopped.ended_at, Some(12_000), "stop timestamp set");
    assert_eq!(
        count_active_for_user(&test, &user_id).await,
        0,
        "no active timer remains"
    );

    test.teardown().await;
}

/// PostgreSQL Gate concurrency test: `start_time_entry` racing with `leave_organization`
/// never leaves an orphaned active timer in the organization the user just left.
#[tokio::test]
async fn concurrent_start_and_leave_organization_leave_no_orphan_timer() {
    if db_type() != DbType::Postgres {
        return;
    }
    let test = common::setup().await;
    let keeper = common::user(&test, "race_keeper").await;
    let leaver = common::user(&test, "race_leaver").await;
    test.system()
        .execute_unchecked(&format!(
            "UPDATE users SET password_hash = '$argon2id$v=19$m=19456,t=2,p=1$test$test' \
             WHERE id = '{leaver}'"
        ))
        .await
        .expect("leaver password");
    test.execute_unchecked(&format!(
        "UPDATE organization_members SET role = '{}' WHERE user_id = '{keeper}' \
         AND org_id = '{}'",
        OrgRole::Admin.as_str(),
        test.org_id,
    ))
    .await
    .expect("keeper admin");
    common::works_in(&test, &test.workspace_id, &leaver).await;

    // Pause `leave_organization` right before it deletes `organization_members`.
    let mut gate = Gate::before(&test, "DELETE", "organization_members", "true").await;
    let second_store = gate.second_store(&test).await;

    let s1 = test.store.clone();
    let u1 = leaver.clone();
    let leave_task = tokio::spawn(async move { s1.leave_organization(&u1).await });
    let holder = gate.stopped(&leave_task).await;

    let u2 = leaver.clone();
    let org2 = test.org_id.clone();
    let ws2 = test.workspace_id.clone();
    let start_task = tokio::spawn(async move {
        second_store
            .start_time_entry(
                password_reader(&u2),
                &org2,
                &sample_entry(Some(&ws2), TimeEntryKind::Work, "racing leave"),
                20_000,
            )
            .await
    });

    gate.waits_for(holder, &start_task).await;
    gate.open().await;

    let departure = ended(leave_task).await.expect("leave completes");
    assert!(
        matches!(departure, Departure::Left(_)),
        "user left organization"
    );
    let start_res = ended(start_task).await;
    assert!(
        matches!(start_res, Err(fvoci_store::StoreError::NotFound(_))),
        "start after leave must fail with NotFound: {start_res:?}"
    );
    assert_eq!(
        count_all_for_user(&test, &leaver).await,
        0,
        "no orphaned time entry inserted into departed org"
    );

    test.teardown().await;
}

/// PostgreSQL Gate concurrency test: two simultaneous `start_time_entry` calls for the same
/// user (including across personal space and shared org) serialize on the user's row lock,
/// never fail with a unique-index violation, and leave exactly one active timer.
#[tokio::test]
async fn concurrent_starts_serialize_behind_gate_without_unique_violation() {
    if db_type() != DbType::Postgres {
        return;
    }
    let test = common::setup().await;
    let user_id = common::user(&test, "racer").await;
    let (own_store, own_ws) = common::in_own_space(&test, &user_id).await;
    let own_org = own_store.tenant().org_id().expect("own org");

    // Gate stops the first transaction right before its INSERT into `time_entries`.
    let mut gate = Gate::before(&test, "INSERT", "time_entries", "NEW.ended_at IS NULL").await;
    let second_store = gate.second_store(&test).await;

    let (s1, u1, org1, ws1) = (test.store.clone(), user_id.clone(), own_org, own_ws);
    let task1 = tokio::spawn(async move {
        s1.start_time_entry(
            password_reader(&u1),
            &org1,
            &sample_entry(Some(&ws1), TimeEntryKind::Study, "from personal"),
            10_000,
        )
        .await
    });
    let holder = gate.stopped(&task1).await;

    let (u2, org2, ws2) = (
        user_id.clone(),
        test.org_id.clone(),
        test.workspace_id.clone(),
    );
    let task2 = tokio::spawn(async move {
        second_store
            .start_time_entry(
                password_reader(&u2),
                &org2,
                &sample_entry(Some(&ws2), TimeEntryKind::Work, "from shared org"),
                10_000,
            )
            .await
    });

    gate.waits_for(holder, &task2).await;
    gate.open().await;

    let first = ended(task1).await.expect("first start succeeds");
    let second = ended(task2).await.expect("second start succeeds");

    assert_eq!(first.stopped, None, "first concurrent start stops nothing");
    let stopped = second
        .stopped
        .expect("second start stopped the first timer atomically");
    assert_eq!(stopped.id, first.entry.id, "second stopped first entry");
    assert_eq!(stopped.ended_at, Some(10_000), "stopped at switch time");
    assert_eq!(second.entry.ended_at, None, "second entry remains active");
    assert_eq!(
        count_active_for_user(&test, &user_id).await,
        1,
        "exactly one active row after concurrent starts"
    );
    assert_eq!(
        count_all_for_user(&test, &user_id).await,
        2,
        "both rows persisted"
    );
    let active = test
        .active_time_entry(password_reader(&user_id))
        .await
        .expect("read active")
        .expect("one active");
    assert_eq!(active.id, second.entry.id, "active returns second entry");

    test.teardown().await;
}

/// Leaving an organization deletes all time entries of that user in that organization
/// (including an active timer so it never becomes an orphaned blocker), while keeping
/// entries in the user's personal space intact; erasing an account deletes every entry.
#[tokio::test]
async fn org_leaving_and_account_erasure_clean_up_time_entries() {
    let test = common::setup().await;
    let (leaver, own_org, own_ws) = seed_leaver_entries(&test).await;
    let reader = password_reader(&leaver);

    let departure = test.leave_organization(&leaver).await.expect("leave org");
    assert!(
        matches!(departure, Departure::Left(_)),
        "leaver left organization"
    );
    assert_eq!(
        count_active_for_user(&test, &leaver).await,
        0,
        "leaving org cleared active timer in that org"
    );
    assert_eq!(
        count_all_for_user(&test, &leaver).await,
        1,
        "only personal space entry remains"
    );
    let remaining = test
        .list_time_entries(
            reader,
            &TimeEntryFilter::default(),
            Paging {
                page: 0,
                per_page: std::num::NonZeroU32::new(10).expect("a page size"),
            },
        )
        .await
        .expect("list remaining");
    assert_eq!(remaining.entries.len(), 1, "one remaining entry listed");
    assert_eq!(
        remaining.entries[0].note, "keep after leaving shared org",
        "personal entry preserved"
    );

    let started = test
        .start_time_entry(
            reader,
            &own_org,
            &sample_entry(Some(&own_ws), TimeEntryKind::Other, "fresh own"),
            7_000,
        )
        .await
        .expect("start after leaving");
    assert_eq!(started.stopped, None, "no orphaned active timer stopped");
    assert_eq!(
        count_all_for_user(&test, &leaver).await,
        2,
        "two personal entries now exist"
    );

    erase_user(&test, &leaver).await;
    assert_eq!(
        count_all_for_user(&test, &leaver).await,
        0,
        "account erasure removed every time entry"
    );

    test.teardown().await;
}

async fn seed_leaver_entries(test: &TestStore) -> (String, String, String) {
    let keeper = common::user(test, "keeper").await;
    let leaver = common::user(test, "leaver").await;
    test.system()
        .execute_unchecked(&format!(
            "UPDATE users SET password_hash = '$argon2id$v=19$m=19456,t=2,p=1$test$test' \
             WHERE id = '{leaver}'"
        ))
        .await
        .expect("leaver password");
    test.execute_unchecked(&format!(
        "UPDATE organization_members SET role = '{}' WHERE user_id = '{keeper}' \
         AND org_id = '{}'",
        OrgRole::Admin.as_str(),
        test.org_id,
    ))
    .await
    .expect("keeper admin");
    common::works_in(test, &test.workspace_id, &leaver).await;

    let (own_store, own_ws) = common::in_own_space(test, &leaver).await;
    let own_org = own_store.tenant().org_id().expect("own org");
    let reader = password_reader(&leaver);

    own_store
        .create_time_entry(
            &leaver,
            &NewStoredTimeEntry {
                workspace_id: Some(&own_ws),
                container_id: None,
                block_id: None,
                target_kind: Some(TimeTargetKind::Board),
                target_title: Some("personal"),
                kind: TimeEntryKind::Study,
                note: "keep after leaving shared org",
            },
            1_000,
            3_000,
            3_000,
        )
        .await
        .expect("personal entry");
    test.create_time_entry(
        &leaver,
        &sample_entry(Some(&test.workspace_id), TimeEntryKind::Work, "org done"),
        4_000,
        5_000,
        5_000,
    )
    .await
    .expect("shared org completed");
    test.start_time_entry(
        reader,
        &test.org_id,
        &sample_entry(Some(&test.workspace_id), TimeEntryKind::Work, "org active"),
        6_000,
    )
    .await
    .expect("shared org active");
    assert_eq!(
        count_all_for_user(test, &leaver).await,
        3,
        "three entries exist before leaving"
    );
    assert_eq!(
        count_active_for_user(test, &leaver).await,
        1,
        "one active entry exists before leaving"
    );
    (leaver, own_org, own_ws)
}

async fn erase_user(test: &TestStore, user_id: &str) {
    let start = test
        .begin_account_erasure(user_id, 100_000, Erasing::AtOnce)
        .await
        .expect("begin erasure");
    assert!(
        matches!(start, ErasureStart::Begun(_)),
        "account erasure began"
    );
    for org_id in test
        .organizations_of_account(user_id)
        .await
        .expect("orgs of account")
    {
        test.for_org(&org_id)
            .expect("org handle")
            .erase_account_from_organization(user_id, Erasing::AtOnce)
            .await
            .expect("erase from org");
    }
    let end = test
        .finish_account_erasure(user_id, 100_001)
        .await
        .expect("finish erasure");
    assert_eq!(end, ErasureEnd::Erased, "account erasure completed");
}
